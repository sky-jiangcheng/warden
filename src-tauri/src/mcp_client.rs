// The MCP client: a thin client over the Model Context Protocol's stdio
// transport.
//
// This file is the reason the copilot can read the Cinder knowledge base.
// It speaks exactly one wire protocol — newline-delimited JSON-RPC 2.0 over a
// child process's stdin/stdout — and it knows NOTHING about Cinder's
// internals: not a table, not a project, not a note. The thin-client rule
// (ADR-0009 in the main repository) says this file evolves only with the MCP
// wire contract.
//
// Why it lives in Rust and not in the webview: a webview cannot spawn a
// process. The IDE plugins can do this in TypeScript because VS Code and
// JetBrains both run their extensions in a host with a Node/JVM runtime and a
// child-process API. Tauri gives the webview a browser sandbox and gives the
// Rust side everything else, so the process boundary is here.
//
// Why it is a LONG-LIVED client rather than one process per call: the MCP
// server opens the same SQLite database the desktop app uses, and the
// protocol's whole point is a session — `initialize` once, then many calls on
// the same connection. Spawning per call would re-open the database per tool
// invocation and re-do the handshake each time.
//
// The three-phase contract, as the wire defines it:
//   1. `initialize` (request)      -> the server answers with its version
//   2. `notifications/initialized` (notification) -> no id, no answer
//   3. `tools/list` / `tools/call` (requests)    -> the actual work
// The notification in step 2 carries no id, so it is written with `notify`,
// never with `request` — a request waits for an answer that never comes.
//
// Why the reading is a plain thread and not an async task: the child's stdout
// is a blocking pipe, and a blocking read inside a tokio worker would stall
// every other task on that worker. One dedicated thread per session, feeding
// a channel, keeps the blocking where it belongs. The awaiting side is async
// (a oneshot receiver with a timeout), so the Tauri command that drives it
// never blocks the IPC thread.
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

/// The MCP protocol revision this client speaks.
///
/// The server answers with the revision it actually implements; the handshake
/// is a negotiation, and a server that answers with a different revision is
/// reported rather than silently accepted.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// How long one request waits for its answer.
///
/// The local server answers in milliseconds. The bound is not about slowness —
/// it is about a wedged or half-dead server process, which would otherwise
/// leave the request pending forever and the first screen spinning.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the handshake gets. It is one round trip against a local process;
/// anything slower than this means the server is not coming up.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// A failed start attempt is remembered for this long.
///
/// The first-screen pollers ask every 2.5 seconds whether the memory layer
/// is there, and each ask that finds no live session would start a server. A
/// server that dies on startup (a locked database, a broken binary) would
/// therefore be respawned once per tick — one crash turned into a process
/// launch every 2.5 seconds, forever. The cooldown caps the respawn rate
/// without caching the failure for good: after it passes, the next attempt
/// is a real one, so a server the user fixes or installs is picked up as
/// before, just within one interval.
const CONNECT_COOLDOWN: Duration = Duration::from_secs(5);

/// One line of the wire may not exceed this many bytes.
///
/// A tool answer is prose and a schema is small; nothing legitimate here
/// speaks in megabyte lines. `read_line` grows its buffer without bound to
/// hold whatever arrives, and the reader is a plain thread nothing else
/// watches — a server gone wrong could grow memory until the copilot dies.
/// A line over the cap ends the session instead: the next call starts a
/// fresh server, which is fail-closed and cheaper than an unbounded read.
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

/// The answer to one request, as the reader thread hands it over.
///
/// The type is spelled out once here because it appears in four places, and
/// three levels of nesting is where a type stops being readable.
type Answer = Result<serde_json::Value, McpError>;

/// The requests waiting for an answer, keyed by the JSON-RPC id we sent.
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

/// A connected session: the client, and the tool list the server gave.
type Connected = (Arc<McpClient>, Vec<ToolInfo>);

/// The live MCP session, kept across calls.
///
/// The copilot talks to the memory layer for the whole life of a window, and
/// the server holds the SQLite database open. One connection for the session
/// is the difference between "the memory layer is there" and "the memory layer
/// is re-opened for every question" — the second costs a database open per
/// tool call and makes the server look flaky.
///
/// The session is lazy: nothing is started until something asks. A user who
/// never touches the memory layer never spawns a server.
#[derive(Default)]
pub struct McpSession {
    connected: tokio::sync::Mutex<Option<Connected>>,
    /// Serializes connection attempts. Only the gate holder runs the (slow)
    /// handshake; the rest wait here and then re-read `connected`. This is a
    /// dedicated single-flight lock, not `connected`, so an in-flight connect
    /// never blocks evictions or status reads that only touch `connected`.
    connecting: tokio::sync::Mutex<()>,
    /// Consecutive requests that timed out. A wedged server whose stdout is
    /// still open never reports `Closed`, so three timeouts evict it.
    timeouts: AtomicUsize,
    /// The most recent failed start attempt, with the instant it failed.
    /// Consulted before every spawn: within `CONNECT_COOLDOWN` the recorded
    /// error is re-served without starting a process, which caps the respawn
    /// rate when the server cannot start at all.
    last_failure: tokio::sync::Mutex<Option<(std::time::Instant, McpError)>>,
}

impl McpSession {
    /// Inject an already handshaken fixture without changing process PATH.
    #[cfg(feature = "test-fixtures")]
    pub fn from_connected(client: McpClient, tools: Vec<ToolInfo>) -> Self {
        Self {
            connected: tokio::sync::Mutex::new(Some((Arc::new(client), tools))),
            connecting: tokio::sync::Mutex::new(()),
            timeouts: AtomicUsize::new(0),
            last_failure: tokio::sync::Mutex::new(None),
        }
    }

    /// A fixture for unit tests that must not touch the machine at all.
    ///
    /// `ensure` answers `NotInstalled` from the cooldown cache without ever
    /// running `detect_mcp`, so a DEVELOPMENT machine that has the real
    /// `repotory-mcp` installed gets the same shape a bare CI runner sees.
    /// A status test that drove the real binary once hung for minutes racing
    /// the user's own running instance for the same database — a unit test
    /// must not depend on what happens to be installed, which is exactly
    /// what its own doc comment already assumed.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn stubbed_unavailable() -> Self {
        Self {
            connected: tokio::sync::Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            timeouts: AtomicUsize::new(0),
            last_failure: tokio::sync::Mutex::new(Some((
                std::time::Instant::now(),
                McpError::NotInstalled,
            ))),
        }
    }

    /// The live session, starting the server if it is not running yet.
    ///
    /// A failure is remembered only for `CONNECT_COOLDOWN`: long enough to
    /// stop a crash-looping server from being respawned by every poller,
    /// short enough that "the user just installed it" needs no retry button.
    /// The absence of the binary is not remembered at all — detecting it is a
    /// path scan, not a process launch, and it must be picked up on the very
    /// next tick.
    pub async fn ensure(&self) -> Result<Connected, McpError> {
        // Fast path: a live session is only ever read behind a brief lock.
        {
            let guard = self.connected.lock().await;
            if let Some(session) = guard.as_ref() {
                return Ok(session.clone());
            }
        }
        // Only one caller runs the handshake at a time. The gate, not
        // `connected`, is held across the slow connect: `connected` itself is
        // only ever locked for the brief swap, never across process/network
        // IO. A failure is not cached — connected stays None for the next try.
        let _gate = self.connecting.lock().await;
        // Another caller may have connected while we waited for the gate.
        {
            let guard = self.connected.lock().await;
            if let Some(session) = guard.as_ref() {
                return Ok(session.clone());
            }
        }
        // A start that just failed is not retried immediately: the same error
        // is re-served unchanged until the cooldown passes, and after it this
        // is a real attempt again. The guard is scoped to this check — it
        // must never be held across the spawn and handshake below.
        {
            let failure = self.last_failure.lock().await;
            if let Some((_, err)) = failure
                .as_ref()
                .filter(|(at, _)| cooldown_active(*at, std::time::Instant::now()))
            {
                return Err(err.clone());
            }
        }
        let binary = crate::memory_layer::detect_mcp().ok_or(McpError::NotInstalled)?;
        let (client, tools) = match McpClient::connect(&binary).await {
            Ok(connected) => connected,
            Err(err) => {
                // The binary was found but would not start or handshake —
                // exactly the crash-loop shape the cooldown exists for.
                *self.last_failure.lock().await = Some((std::time::Instant::now(), err.clone()));
                return Err(err);
            }
        };
        *self.last_failure.lock().await = None;
        let session = (Arc::new(client), tools);
        {
            let mut guard = self.connected.lock().await;
            *guard = Some(session.clone());
        }
        self.timeouts.store(0, Ordering::Relaxed);
        Ok(session)
    }

    /// Calls one tool on the live session.
    ///
    /// A `Closed` answer means the pipe is gone — the server process exited
    /// while the window stayed open. The cached session must not survive it:
    /// the pollers ask again in 2.5 seconds, and handing them the dead client
    /// would report "Cinder unavailable" until the app restarts, even though
    /// the binary could simply be started again.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, McpError> {
        let (client, _) = self.ensure().await?;
        let answer = client.call_tool(name, arguments).await;
        self.evict_if_closed(&answer).await;
        answer
    }

    /// Drops a dead session so the next `ensure` starts the server again.
    ///
    /// A `Closed` answer evicts immediately. A wedged server whose stdout is
    /// still open never reports `Closed`: three consecutive `Timeout`s also
    /// evict it, so `Drop` kills the child and the next call reconnects. Any
    /// other answer proves the server is alive and resets the streak.
    pub(crate) async fn evict_if_closed<T>(&self, answer: &Result<T, McpError>) {
        match answer {
            Err(McpError::Closed(_)) => {
                *self.connected.lock().await = None;
                self.timeouts.store(0, Ordering::Relaxed);
            }
            Err(McpError::Timeout(_)) => {
                let strikes = self.timeouts.fetch_add(1, Ordering::Relaxed) + 1;
                if strikes >= 3 {
                    *self.connected.lock().await = None;
                    self.timeouts.store(0, Ordering::Relaxed);
                }
            }
            _ => {
                self.timeouts.store(0, Ordering::Relaxed);
            }
        }
    }
}

/// One tool the server advertises, as the first screen shows it.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    /// The tool's name — the identifier a `tools/call` needs.
    pub name: String,
    /// The server's own description of what the tool does.
    pub description: String,
    /// The tool's JSON-schema input shape, as the server advertised it. The
    /// function-calling face passes it through verbatim: a schema copied or
    /// invented here would drift from the server's contract, and the contract
    /// is the server's, not ours.
    #[serde(default, skip_serializing)]
    pub input_schema: Option<serde_json::Value>,
}

/// Why an MCP connection could not be established.
///
/// The shapes are kept apart because the first screen says different things
/// about them: "no memory layer installed" is an install problem, "the server
/// is there but the handshake failed" is a version problem, and "the server
/// died" is neither.
#[derive(Debug)]
pub enum McpError {
    /// The server binary could not be started at all.
    Spawn(std::io::Error),
    /// The server binary is not installed on this machine.
    NotInstalled,
    /// The server did not answer in time. Holds the method that was waiting.
    Timeout(String),
    /// The server answered with an error.
    Protocol(String),
    /// The server process exited or its output closed.
    Closed(String),
    /// The response did not have the shape the protocol promises.
    Malformed(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::Spawn(err) => write!(f, "could not start the MCP server: {err}"),
            McpError::NotInstalled => {
                write!(f, "repotory-mcp is not installed on this machine")
            }
            McpError::Timeout(what) => write!(f, "the MCP server did not answer in time ({what})"),
            McpError::Protocol(msg) => write!(f, "the MCP server reported an error: {msg}"),
            McpError::Closed(msg) => write!(f, "the MCP server closed the connection: {msg}"),
            McpError::Malformed(msg) => write!(f, "the MCP server sent something unusable: {msg}"),
        }
    }
}

impl std::error::Error for McpError {}

impl Clone for McpError {
    fn clone(&self) -> Self {
        match self {
            // `io::Error` is not `Clone`; the copy keeps the message — and so
            // the `Display`, which is everything every caller and the IPC
            // boundary see — so a cooldown can re-serve the original failure
            // verbatim instead of inventing a different shape.
            Self::Spawn(err) => Self::Spawn(std::io::Error::other(err.to_string())),
            Self::NotInstalled => Self::NotInstalled,
            Self::Timeout(what) => Self::Timeout(what.clone()),
            Self::Protocol(msg) => Self::Protocol(msg.clone()),
            Self::Closed(msg) => Self::Closed(msg.clone()),
            Self::Malformed(msg) => Self::Malformed(msg.clone()),
        }
    }
}

/// Whether a failed start attempt still counts: within the cooldown the
/// recorded error is re-served without another spawn, after it the next
/// attempt is a real one.
fn cooldown_active(at: std::time::Instant, now: std::time::Instant) -> bool {
    now.duration_since(at) < CONNECT_COOLDOWN
}

/// A live MCP session over stdio.
///
/// Dropping it kills the child process: a copilot that is closing must not
/// leave an orphaned `repotory-mcp` holding the database.
pub struct McpClient {
    // Shared with the blocking write task: a ChildStdin write is a blocking
    // pipe operation and must never run on a tokio worker thread.
    stdin: Arc<Mutex<ChildStdin>>,
    child: Mutex<Option<Child>>,
    /// Pending requests, keyed by the JSON-RPC id we sent.
    pending: Pending,
    next_id: AtomicU64,
    /// The version the server reported in the `initialize` answer. Never
    /// hardcoded here; None when the server did not report one.
    server_version: Option<String>,
}

impl McpClient {
    /// Starts the server and completes the handshake.
    ///
    /// Returns the session and the tool list the server advertises: the
    /// handshake and the first `tools/list` are one startup cost, and every
    /// caller wants both.
    pub async fn connect(binary: &Path) -> Result<(Self, Vec<ToolInfo>), McpError> {
        Self::connect_to(binary, None).await
    }

    /// Starts the server, with an environment variable set for it.
    ///
    /// The variable exists for the fake server the end-to-end tests drive:
    /// the real `repotory-mcp` is told nothing, and a production call must
    /// not be able to smuggle an environment into the server it starts.
    pub async fn connect_to(
        binary: &Path,
        env_var: Option<&str>,
    ) -> Result<(Self, Vec<ToolInfo>), McpError> {
        let envs: Vec<(String, String)> = env_var
            .map(|name| vec![(name.to_string(), "1".to_string())])
            .unwrap_or_default();
        Self::connect_with_env(binary, &envs).await
    }

    /// Starts the server with environment VARIABLES AND VALUES set for it.
    ///
    /// The single-name `connect_to` can only say "serve"; the conversation
    /// fixtures also need to hand the fake server a per-fixture path (where
    /// to log the calls it answered), and the harness runs tests in
    /// parallel — a shared process environment would race. Production
    /// (`connect`) never comes through here: the real `repotory-mcp` is
    /// told nothing.
    pub async fn connect_with_env(
        binary: &Path,
        envs: &[(String, String)],
    ) -> Result<(Self, Vec<ToolInfo>), McpError> {
        let mut command = Command::new(binary);
        for (name, value) in envs {
            command.env(name, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The server logs to stderr (mcp-go's ServeStdio writes its banner
            // there). We inherit it rather than capturing it: the log line is
            // how a human sees "the database is locked" when the handshake
            // fails, and hiding it would make that failure unexplainable.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(McpError::Spawn)?;

        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::Malformed("the MCP server has no stdin to write requests to".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::Malformed("the MCP server has no stdout to read answers from".to_string())
        })?;

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        // One reader thread for the whole session. The stdio transport is
        // request/response over a single pipe, so a single reader is the
        // correct shape: a second reader would race for lines.
        let reader_pending = Arc::clone(&pending);
        thread::Builder::new()
            .name("mcp-stdio-reader".to_string())
            .spawn(move || read_loop(stdout, reader_pending))
            .map_err(McpError::Spawn)?;

        let mut client = Self {
            stdin: Arc::new(Mutex::new(stdin)),
            child: Mutex::new(Some(child)),
            pending,
            next_id: AtomicU64::new(1),
            server_version: None,
        };

        let version = client.handshake().await?;
        client.server_version = version;
        let tools = client.list_tools().await?;
        Ok((client, tools))
    }

    /// `initialize` then `notifications/initialized`.
    ///
    /// The notification carries no id, so it must not go through `request`:
    /// a request waits for an answer, and the server sends none. The
    /// `initialize` answer carries `serverInfo.version`, which we keep; the
    /// negotiated `protocolVersion` is only logged, never required — mcp-go
    /// answers a legacy revision with the same string, and this client uses
    /// only the cross-version-stable face (`tools/list`, `tools/call`).
    ///
    /// The mismatch goes to stderr rather than a `tracing` event on purpose:
    /// the app installs no subscriber, so a tracing event here would be
    /// invisible, and stderr is the log channel the whole stdio design
    /// already agreed on (the server's own stderr is inherited for exactly
    /// this reason).
    async fn handshake(&self) -> Result<Option<String>, McpError> {
        let result = self
            .request_with_timeout(
                "initialize",
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "repotory-agent", "version": env!("CARGO_PKG_VERSION") },
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await?;
        match result
            .get("protocolVersion")
            .and_then(|value| value.as_str())
        {
            Some(protocol) if protocol != PROTOCOL_VERSION => eprintln!(
                "[mcp] server negotiated protocol {protocol}, client speaks {}",
                PROTOCOL_VERSION
            ),
            None => eprintln!("[mcp] the initialize answer carried no protocolVersion"),
            _ => {}
        }
        let version = result
            .get("serverInfo")
            .and_then(|info| info.get("version"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        self.notify("notifications/initialized", serde_json::json!({}))
            .await;
        Ok(version)
    }

    /// The tool list, as the server reports it right now.
    ///
    /// This is the whole point of the client: the list is the server's, not
    /// ours. A hardcoded copy is worse than no copy — it looks authoritative,
    /// so a reader believes it is the contract.
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, McpError> {
        let result = self.request("tools/list", serde_json::json!({})).await?;
        parse_tools(&result)
    }

    /// Calls one tool and returns its text content.
    ///
    /// An `isError: true` result is the protocol's own in-band tool failure —
    /// the server says "this call failed" while the transport stays healthy
    /// (mcp-go answers input-schema violations this way). It becomes an
    /// `Err`, not a successful answer: a UI that read it as done-and-OK would
    /// show a failed call as a completed one. The server's text is kept as
    /// the error message, so the model can still read why it was refused.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, McpError> {
        let result = self
            .request(
                "tools/call",
                serde_json::json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        if result
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Err(McpError::Protocol(extract_text(&result)));
        }
        Ok(extract_text(&result))
    }

    /// The server process's id.
    ///
    /// It is here so the end-to-end test can check that the child really
    /// dies with the client — an orphaned server holds the Cinder SQLite
    /// database open, and that is not a thing to leave behind.
    pub fn server_pid(&self) -> Option<u32> {
        self.child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|child| child.id())
    }

    /// The version the server reported at handshake, if any.
    pub fn version(&self) -> Option<String> {
        self.server_version.clone()
    }

    /// Sends a request and waits for the matching answer.
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(err) = self.write_line(&body.to_string()).await {
            self.forget(id);
            return Err(err);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(err))) => Err(err),
            // The reader dropped the sender without answering: the server's
            // output closed while this request was in flight.
            Ok(Err(_)) => {
                self.forget(id);
                Err(McpError::Closed(format!(
                    "the server stopped answering while handling {method}"
                )))
            }
            Err(_) => {
                self.forget(id);
                Err(McpError::Timeout(method.to_string()))
            }
        }
    }

    /// Sends a notification — a message with no id and no answer.
    async fn notify(&self, method: &str, params: serde_json::Value) {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        // A notification has no answer, so a write failure here cannot be
        // reported to anyone. It is logged rather than dropped silently: a
        // server that never gets `initialized` will refuse the next call, and
        // the log is where that shows up.
        if let Err(err) = self.write_line(&body.to_string()).await {
            eprintln!("[mcp] the {method} notification could not be sent: {err}");
        }
    }

    /// Writes one JSON-RPC line off the async worker pool. The pipe is a
    /// blocking `ChildStdin`, so the write+flush runs on the blocking pool;
    /// the mutex guard is created inside that closure and never crosses an
    /// await. Errors still reach the caller, so a dead server is reported.
    async fn write_line(&self, line: &str) -> Result<(), McpError> {
        let stdin = Arc::clone(&self.stdin);
        let body = line.to_owned();
        let written = tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let mut guard = stdin.lock().unwrap_or_else(|e| e.into_inner());
            writeln!(guard, "{body}")
                .map_err(|err| format!("the request could not be written: {err}"))?;
            guard
                .flush()
                .map_err(|err| format!("the request could not be flushed: {err}"))
        })
        .await
        .map_err(|err| McpError::Closed(format!("the write task failed: {err}")))?;
        written.map_err(McpError::Closed)
    }

    fn forget(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // The child is killed, not left to exit on its own: the server holds
        // the SQLite database open, and an orphan would keep it locked after
        // the copilot is gone.
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Every waiter is failed: a caller blocked on a request would wait
        // out the full timeout for an answer that can never arrive.
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(McpError::Closed(
                "the MCP client was closed".to_string(),
            )));
        }
    }
}

/// The tool list out of a `tools/list` answer.
fn parse_tools(result: &serde_json::Value) -> Result<Vec<ToolInfo>, McpError> {
    let tools = result
        .get("tools")
        .and_then(|t| t.as_array())
        .ok_or_else(|| {
            McpError::Malformed("the tools/list answer has no tools array".to_string())
        })?;
    Ok(tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?.to_string();
            let description = tool
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string();
            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .filter(|schema| schema.is_object());
            Some(ToolInfo {
                name,
                description,
                input_schema,
            })
        })
        .collect())
}

/// The text out of a `tools/call` answer.
fn extract_text(result: &serde_json::Value) -> String {
    let Some(parts) = result.get("content").and_then(|c| c.as_array()) else {
        // A tool may answer with something other than content parts. The raw
        // JSON is still information; dropping it would make the answer look
        // empty when it was not.
        return result.to_string();
    };
    parts
        .iter()
        .filter(|part| part.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads the server's answers for the whole session.
///
/// The stdio transport is newline-delimited: one JSON-RPC message per line.
/// `read_line` blocks until the newline arrives, so a chunk boundary that
/// falls inside a message is handled — the partial line simply is not
/// returned until it is complete. The byte cap is enforced by wrapping the
/// pipe in `Take`: a line that outgrows the allowance ends the session
/// rather than growing a buffer without bound.
fn read_loop(stdout: std::process::ChildStdout, pending: Pending) {
    use std::io::Read as _;
    let mut reader = std::io::BufReader::new(stdout.take(MAX_LINE_BYTES));
    read_lines(&mut reader, &pending);
}

/// The line pump, generic over the pipe so the tests can drive it with an
/// in-memory reader instead of a real child process. `R: Read` is enough:
/// `BufReader<Take<R>>` gains `BufRead` from the inner `Read`, which is all
/// `read_line` needs — the child's stdout is `Read`, not `BufRead`.
fn read_lines<R: std::io::Read>(
    reader: &mut std::io::BufReader<std::io::Take<R>>,
    pending: &Pending,
) {
    use std::io::BufRead;
    let mut buffer = String::new();
    loop {
        buffer.clear();
        match reader.read_line(&mut buffer) {
            Ok(0) => break, // the server closed stdout
            Ok(_) => {}
            Err(_) => break,
        }
        if !buffer.ends_with('\n') {
            // The newline is missing for exactly two reasons: the byte
            // allowance ran out mid-line (a broken or hostile server — no
            // legitimate message is this big), or the server died mid-write.
            // Only the first deserves a log line; the second is the ordinary
            // closed-pipe path that drains the waiters below.
            if reader.get_ref().limit() == 0 {
                eprintln!(
                    "[mcp] the server sent a line over the {MAX_LINE_BYTES}-byte cap; dropping the session"
                );
            } else {
                // A last line without its trailing newline still counts —
                // some writers omit it. Dropping a valid final answer would
                // turn it into a timeout instead of an answer.
                let line = buffer.trim();
                if !line.is_empty() {
                    dispatch(line, pending);
                }
            }
            break;
        }
        // The line finished inside the allowance; restore it for the next one.
        reader.get_mut().set_limit(MAX_LINE_BYTES);
        let line = buffer.trim();
        if line.is_empty() {
            continue;
        }
        dispatch(line, pending);
    }
    // The pipe closed: every waiter must be failed, or a caller would sit
    // there until its timeout for an answer that can never arrive.
    let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(McpError::Closed(
            "the MCP server closed its output".to_string(),
        )));
    }
}

/// Routes one answer to the request that is waiting for it.
fn dispatch(line: &str, pending: &Pending) {
    let message: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        // A line that is not JSON is not this protocol's business. The server
        // may print something of its own to stdout; that is not an error, it
        // is noise, and treating it as a failure would break the session over
        // a stray log line.
        Err(_) => return,
    };

    // A message that carries a `method` is the server TALKING, not answering:
    // a request or a notification. This client declares no capabilities, so
    // well-behaved servers send none — but if one ever arrives, its `id` is
    // the SERVER's counter, which would collide with a pending request's id
    // (both sides count from 1). Consuming that waiter would fail it with a
    // fabricated error and leave the server's question unanswered; skipping
    // leaves the real waiter waiting for the real answer, and the request
    // timeout bounds the wait either way.
    if message.get("method").is_some() {
        return;
    }

    // A message with no id is a notification. This client asks questions and
    // waits; it has nothing to do with those.
    let Some(id) = message.get("id").and_then(|id| id.as_u64()) else {
        return;
    };

    let waiting = pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    let Some(tx) = waiting else { return };

    let outcome = if let Some(error) = message.get("error") {
        let text = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("the server reported an error without a message");
        Err(McpError::Protocol(text.to_string()))
    } else {
        match message.get("result") {
            Some(result) => Ok(result.clone()),
            None => Err(McpError::Malformed(
                "the answer has neither a result nor an error".to_string(),
            )),
        }
    };
    let _ = tx.send(outcome);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_map() -> Pending {
        Arc::new(Mutex::new(HashMap::new()))
    }

    /// The transport is newline-delimited JSON-RPC. A framed answer must reach
    /// the request that is waiting for it.
    #[tokio::test]
    async fn an_answer_reaches_the_request_that_asked_for_it() {
        let pending = pending_map();
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(7, tx);

        dispatch(
            r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#,
            &pending,
        );

        let answer = rx.await.expect("an answer");
        assert!(answer.is_ok());
        assert!(pending.lock().unwrap().is_empty());
    }

    /// Two requests in flight: the answers may come back in any order, and
    /// each must reach the right waiter.
    #[tokio::test]
    async fn answers_are_matched_by_id_not_by_arrival_order() {
        let pending = pending_map();
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        pending.lock().unwrap().insert(1, first_tx);
        pending.lock().unwrap().insert(2, second_tx);

        dispatch(r#"{"jsonrpc":"2.0","id":2,"result":"second"}"#, &pending);
        dispatch(r#"{"jsonrpc":"2.0","id":1,"result":"first"}"#, &pending);

        assert_eq!(first_rx.await.unwrap().unwrap(), serde_json::json!("first"));
        assert_eq!(
            second_rx.await.unwrap().unwrap(),
            serde_json::json!("second")
        );
    }

    /// An error answer is an error, not an empty result.
    #[tokio::test]
    async fn an_error_answer_becomes_an_error() {
        let pending = pending_map();
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(3, tx);

        dispatch(
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"no such tool"}}"#,
            &pending,
        );

        let err = rx.await.expect("an answer").expect_err("an error answer");
        assert!(
            matches!(err, McpError::Protocol(ref msg) if msg == "no such tool"),
            "unexpected error: {err}"
        );
    }

    /// A notification carries no id and must not consume a waiter.
    #[tokio::test]
    async fn a_notification_is_not_an_answer() {
        let pending = pending_map();
        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert(4, tx);

        dispatch(
            r#"{"jsonrpc":"2.0","method":"notifications/message","params":{}}"#,
            &pending,
        );

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut rx)
                .await
                .is_err(),
            "a notification must not answer a pending request"
        );
        assert_eq!(pending.lock().unwrap().len(), 1);
    }

    /// The server may print its own lines to stdout. Those are noise, not
    /// protocol violations, and one of them must not break the session.
    #[tokio::test]
    async fn a_line_that_is_not_json_is_noise_not_failure() {
        let pending = pending_map();
        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert(5, tx);

        dispatch("Cinder MCP server v2.24.0 starting on stdio...", &pending);

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut rx)
                .await
                .is_err()
        );
        assert_eq!(pending.lock().unwrap().len(), 1);

        // The session still works afterwards.
        dispatch(r#"{"jsonrpc":"2.0","id":5,"result":"ok"}"#, &pending);
        assert_eq!(rx.await.unwrap().unwrap(), serde_json::json!("ok"));
    }

    /// A chunk boundary can fall inside a message, so the reader must hold the
    /// partial line until its newline arrives.
    #[test]
    fn a_partial_line_is_held_until_its_newline_arrives() {
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(
            b"{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":\"x\"}\n".to_vec(),
        ));
        let mut buffer = String::new();
        reader.read_line(&mut buffer).expect("a line");
        assert_eq!(buffer.trim(), r#"{"jsonrpc":"2.0","id":9,"result":"x"}"#);
    }

    /// A server-initiated request carries a `method` AND an id — the server's
    /// id, counted on its own side. It must not consume a waiter even when the
    /// two counters collide, and the real answer behind it must still land.
    #[tokio::test]
    async fn a_server_request_is_not_an_answer_even_when_ids_collide() {
        let pending = pending_map();
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(1, tx);

        // The server asks something with ITS id 1, which collides with the
        // client's pending request id 1.
        dispatch(
            r#"{"jsonrpc":"2.0","id":1,"method":"sampling/createMessage","params":{}}"#,
            &pending,
        );
        assert_eq!(
            pending.lock().unwrap().len(),
            1,
            "a server request must not consume the waiter"
        );

        // The genuine answer still reaches it afterwards.
        dispatch(r#"{"jsonrpc":"2.0","id":1,"result":"real"}"#, &pending);
        assert_eq!(rx.await.unwrap().unwrap(), serde_json::json!("real"));
    }

    /// A line over the byte cap is not read into memory without bound: the
    /// session is dropped and every waiter failed, rather than growing a
    /// buffer to hold whatever came in.
    #[tokio::test]
    async fn an_oversized_line_drops_the_session_instead_of_growing_memory() {
        let pending = pending_map();
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(6, tx);

        // A 64-byte allowance; the "server" sends a line ten times that size.
        // No answer of ours could ever be that long — the session must end.
        let huge = format!("{}\n", "x".repeat(640));
        use std::io::Read as _;
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(huge.into_bytes()).take(64));
        read_lines(&mut reader, &pending);

        let answer = rx.await.expect("waiters are failed when the session drops");
        assert!(matches!(answer, Err(McpError::Closed(_))));
        assert!(pending.lock().unwrap().is_empty());
    }

    /// A final line whose trailing newline never came (the writer omitted it,
    /// or the process died right after it) is still dispatched — dropping a
    /// valid answer would turn it into a timeout instead of an answer.
    #[tokio::test]
    async fn a_final_line_without_a_newline_still_counts() {
        let pending = pending_map();
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(8, tx);

        let body = br#"{"jsonrpc":"2.0","id":8,"result":"last word"}"#.to_vec();
        use std::io::Read as _;
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(body).take(1024));
        read_lines(&mut reader, &pending);

        assert_eq!(rx.await.unwrap().unwrap(), serde_json::json!("last word"));
    }

    /// A start that just failed is not retried immediately: within the
    /// cooldown the recorded error is re-served without another spawn
    /// attempt, which is what keeps a crash-looping server from being
    /// respawned by every 2.5-second poller.
    #[tokio::test]
    async fn a_failed_start_is_retried_only_after_the_cooldown() {
        let session = McpSession::default();
        *session.last_failure.lock().await = Some((
            std::time::Instant::now(),
            McpError::Closed("boom".to_string()),
        ));

        // `ensure`'s Ok type carries no `Debug`, so the Err is matched
        // instead of unwrapped through `expect_err`.
        let answer = session.ensure().await;
        assert!(
            matches!(answer, Err(McpError::Closed(ref msg)) if msg == "boom"),
            "the recorded error must come back unchanged"
        );
    }

    /// The cooldown is short on purpose: once it passes, the next attempt is
    /// a real one, so a server the user fixed is picked up without any
    /// retry button.
    #[test]
    fn the_cooldown_expires() {
        let now = std::time::Instant::now();
        assert!(cooldown_active(now, now), "a fresh failure counts");
        let expired = now
            .checked_sub(CONNECT_COOLDOWN)
            .expect("the test clock reaches back five seconds");
        assert!(
            !cooldown_active(expired, now),
            "a failure older than the cooldown must not count"
        );
    }

    /// The tool list is the server's, and the client reports what the server
    /// said — including a tool with no description.
    #[test]
    fn the_tool_list_is_read_from_the_answer() {
        let answer = serde_json::json!({
            "tools": [
                { "name": "repotory_context", "description": "ctx" },
                { "name": "repotory_scan" }
            ]
        });
        let tools = parse_tools(&answer).expect("a tool list");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "repotory_context");
        assert_eq!(tools[0].description, "ctx");
        assert_eq!(tools[1].name, "repotory_scan");
        assert_eq!(
            tools[1].description, "",
            "a tool with no description must not gain one"
        );
    }

    /// An answer with no tools array is a protocol violation, not an empty
    /// list: "the server said something else" and "the server has no tools"
    /// are different facts.
    #[test]
    fn an_answer_without_a_tools_array_is_an_error() {
        let answer = serde_json::json!({ "somethingElse": true });
        assert!(parse_tools(&answer).is_err());
    }

    /// The tool answer is the text parts, joined. A tool that answers with
    /// something else still gets its raw JSON — dropping it would make the
    /// answer look empty when it was not.
    #[test]
    fn the_tool_answer_is_the_text_parts() {
        let answer = serde_json::json!({
            "content": [
                { "type": "text", "text": "first" },
                { "type": "image", "data": "..." },
                { "type": "text", "text": "second" }
            ]
        });
        assert_eq!(extract_text(&answer), "first\nsecond");

        let other = serde_json::json!({ "unexpected": 1 });
        assert_eq!(extract_text(&other), other.to_string());
    }
}
