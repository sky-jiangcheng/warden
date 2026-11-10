// The end-to-end test of the MCP stdio client.
//
// The unit tests in `src/mcp_client.rs` exercise the protocol's parts: the
// framing, the id matching, the answer shapes. They cannot exercise the
// parts that only exist with a real process on the other end of the pipe:
// the handshake, the reader thread, and the fact that the child is killed
// when the client goes away.
//
// So these tests run the client against `tests/fake_mcp_server.rs` — a
// process that speaks the protocol and nothing else.
//
// What is deliberately NOT tested here: the real `repotory-mcp`. That binary
// belongs to the main repository and is not a dependency of this one, so a
// test that required it would fail on every machine that does not happen to
// have the Cinder desktop app installed. The client's contract with the real
// server is checked against the real server during development, not in CI.
use std::path::PathBuf;
use std::time::Duration;

use repotory_agent_lib::mcp_client::McpClient;

/// The environment variable that turns the fake server binary into a server.
const SERVER_MODE: &str = "REPOTORY_AGENT_FAKE_MCP";

/// The tool list the fake server advertises, as the tests expect it.
const EXPECTED_TOOLS: [&str; 2] = ["repotory_context", "repotory_scan"];

/// The text the fake server answers a tool call with.
const EXPECTED_TEXT: &str = "line one\nline \"two\"\nline three";

/// The fake server's path.
///
/// Cargo puts `CARGO_BIN_EXE_<name>` in the environment of an integration
/// test, and it points at the binary the current build produced — not at
/// whatever happens to be on PATH. A stale binary would make these tests
/// lie. The variable is read at RUN time, not compile time: Cargo only
/// guarantees it for `[[bin]]` targets, and this fake server is a `[[test]]`.
fn fake_server() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_BIN_EXE_fake-mcp-server")
            .expect("Cargo names the fake server binary for the integration test"),
    )
}

/// The client's connection to the fake server, and the tool list it gave.
///
/// The server answers `initialize`, `tools/list` and `tools/call`.
async fn connect() -> (McpClient, Vec<repotory_agent_lib::mcp_client::ToolInfo>) {
    let (client, tools) = McpClient::connect_to(&fake_server(), Some(SERVER_MODE))
        .await
        .expect("the fake server answers the handshake");
    assert_eq!(tools.len(), EXPECTED_TOOLS.len());
    (client, tools)
}

/// The handshake completes and the tool list comes back from the server.
#[tokio::test]
async fn the_handshake_completes_and_the_tool_list_comes_back() {
    let (client, tools) = connect().await;

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, EXPECTED_TOOLS);
    assert_eq!(tools[0].description, "load a project");
    assert!(
        !tools[1].description.is_empty(),
        "the server's own description must be carried over"
    );

    // The list can be asked again on the same session: the server is still
    // there, and the connection is the point.
    let again = client.list_tools().await.expect("a second tool list");
    assert_eq!(again.len(), EXPECTED_TOOLS.len());
}

/// A tool call round trips: the text survives the JSON envelope, quotes and
/// newlines included.
#[tokio::test]
async fn a_tool_call_round_trips() {
    let (client, _) = connect().await;

    let text = client
        .call_tool("repotory_context", serde_json::json!({ "project_id": 1 }))
        .await
        .expect("the tool answers");

    assert_eq!(text, EXPECTED_TEXT);
}

/// A server-side error reaches the caller as an error, not as an empty
/// answer. An empty answer would be read as "the tool found nothing".
#[tokio::test]
async fn a_server_error_reaches_the_caller_as_an_error() {
    let (client, _) = connect().await;

    let err = client
        .call_tool("refuse", serde_json::json!({}))
        .await
        .expect_err("the server refuses");

    assert!(
        err.to_string().contains("refused the request"),
        "the error must carry the server's own message: {err}"
    );
}

/// The protocol's in-band failure — `result.isError` with text, how mcp-go
/// answers an input-schema violation — is an Err too, carrying the server's
/// own sentence. A client that read it as a completed call would show "✓ 已
/// 完成" for a call the server refused.
#[tokio::test]
async fn an_in_band_tool_failure_is_an_error_with_the_server_text() {
    let (client, _) = connect().await;

    let err = client
        .call_tool("refuse_inband", serde_json::json!({}))
        .await
        .expect_err("isError is a failure, not an answer");

    assert!(
        err.to_string().contains("project_id must be a number"),
        "the error must carry the server's own text: {err}"
    );
}

/// The client kills the server when it goes away.
///
/// This matters more than it looks: the server holds the Cinder SQLite
/// database open, and an orphaned one keeps it locked for the desktop app.
/// The check is on the process, not on the object — the object going away
/// says nothing about the child.
#[tokio::test]
async fn the_server_is_killed_when_the_client_goes_away() {
    let pid = {
        let (client, _) = connect().await;
        let pid = client
            .server_pid()
            .expect("the client knows the server's process id");
        assert!(
            is_alive(pid),
            "the server must be running while the client holds it"
        );
        pid
    }; // the client is dropped here

    // The kill is not instantaneous: the client asks, the OS reaps. Waiting
    // for the process to actually go is what makes the assertion honest.
    let mut gone = false;
    for _ in 0..50 {
        if !is_alive(pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the server process {pid} outlived the client");
}

/// Whether a process is still running.
///
/// `kill -0` asks the OS without sending a signal. It is the standard way
/// to check "is this pid alive" from a shell, and it needs no permissions
/// beyond the caller's own processes.
#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 does not deliver a signal; it only asks
    // the kernel whether the process could be signalled. The pid comes from
    // this test's own child process.
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

/// The non-unix build has no `kill` to ask. The process check is skipped
/// there rather than faked: a check that always says "alive" would pass for
/// the wrong reason.
#[cfg(not(unix))]
fn is_alive(_pid: u32) -> bool {
    true
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}
