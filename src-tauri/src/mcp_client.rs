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
use std::sync::atomic::{AtomicU64, Ordering};
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
}

impl McpSession {
    /// The live session, starting the server if it is not running yet.
    ///
    /// A failure is not cached: if the server was not installed a moment ago
    /// and is now, the next attempt should succeed. Caching the failure
    /// would need a retry button to be fair to the user.
    pub async fn ensure(&self) -> Result<Connected, McpError> {
        let mut guard = self.connected.lock().await;
        if let Some(session) = guard.as_ref() {
            return Ok(session.clone());
        }
        let binary = crate::memory_layer::detect_mcp().ok_or(McpError::NotInstalled)?;
        let (client, tools) = McpClient::connect(&binary).await?;
        let session = (Arc::new(client), tools);
        *guard = Some(session.clone());
        Ok(session)
    }

    /// Calls one tool on the live session.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, McpError> {
        let (client, _) = self.ensure().await?;
        client.call_tool(name, arguments).await
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

/// A live MCP session over stdio.
///
/// Dropping it kills the child process: a copilot that is closing must not
/// leave an orphaned `repotory-mcp` holding the database.
pub struct McpClient {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Option<Child>>,
    /// Pending requests, keyed by the JSON-RPC id we sent.
    pending: Pending,
    next_id: AtomicU64,
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
        let mut command = Command::new(binary);
        if let Some(name) = env_var {
            command.env(name, "1");
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

        let client = Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(Some(child)),
            pending,
            next_id: AtomicU64::new(1),
        };

        client.handshake().await?;
        let tools = client.list_tools().await?;
        Ok((client, tools))
    }

    /// `initialize` then `notifications/initialized`.
    ///
    /// The notification carries no id, so it must not go through `request`:
    /// a request waits for an answer, and the server sends none.
    async fn handshake(&self) -> Result<(), McpError> {
        self.request_with_timeout(
            "initialize",
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "repotory-agent", "version": env!("CARGO_PKG_VERSION") },
            }),
            HANDSHAKE_TIMEOUT,
        )
        .await?;
        self.notify("notifications/initialized", serde_json::json!({}));
        Ok(())
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
            .expect("the MCP child lock is not poisoned")
            .as_ref()
            .map(|child| child.id())
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
            .expect("the pending-request map is not poisoned")
            .insert(id, tx);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(err) = self.write_line(&body.to_string()) {
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
    fn notify(&self, method: &str, params: serde_json::Value) {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        // A notification has no answer, so a write failure here cannot be
        // reported to anyone. It is logged rather than dropped silently: a
        // server that never gets `initialized` will refuse the next call, and
        // the log is where that shows up.
        if let Err(err) = self.write_line(&body.to_string()) {
            eprintln!("[mcp] the {method} notification could not be sent: {err}");
        }
    }

    fn write_line(&self, line: &str) -> Result<(), McpError> {
        let mut stdin = self
            .stdin
            .lock()
            .expect("the MCP stdin lock is not poisoned");
        use std::io::Write;
        writeln!(stdin, "{line}")
            .map_err(|err| McpError::Closed(format!("the request could not be written: {err}")))?;
        stdin
            .flush()
            .map_err(|err| McpError::Closed(format!("the request could not be flushed: {err}")))
    }

    fn forget(&self, id: u64) {
        self.pending
            .lock()
            .expect("the pending-request map is not poisoned")
            .remove(&id);
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // The child is killed, not left to exit on its own: the server holds
        // the SQLite database open, and an orphan would keep it locked after
        // the copilot is gone.
        if let Some(mut child) = self
            .child
            .lock()
            .expect("the MCP child lock is not poisoned")
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Every waiter is failed: a caller blocked on a request would wait
        // out the full timeout for an answer that can never arrive.
        let mut pending = self
            .pending
            .lock()
            .expect("the pending-request map is not poisoned");
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
            Some(ToolInfo { name, description })
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
/// returned until it is complete.
fn read_loop(stdout: std::process::ChildStdout, pending: Pending) {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(stdout);
    let mut buffer = String::new();
    loop {
        buffer.clear();
        match reader.read_line(&mut buffer) {
            Ok(0) => break, // the server closed stdout
            Ok(_) => {}
            Err(_) => break,
        }
        let line = buffer.trim();
        if line.is_empty() {
            continue;
        }
        dispatch(line, &pending);
    }
    // The pipe closed: every waiter must be failed, or a caller would sit
    // there until its timeout for an answer that can never arrive.
    let mut pending = pending
        .lock()
        .expect("the pending-request map is not poisoned");
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

    // A message with no id is a notification or a server-initiated request.
    // This client asks questions and waits; it has nothing to do with those.
    let Some(id) = message.get("id").and_then(|id| id.as_u64()) else {
        return;
    };

    let waiting = pending
        .lock()
        .expect("the pending-request map is not poisoned")
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
