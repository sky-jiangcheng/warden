// The fake MCP server the end-to-end tests drive.
//
// The unit tests in `src/mcp_client.rs` cover the framing, the id matching
// and the answer shapes. What they cannot cover is the part that only shows
// up with a REAL process on the other end of the pipe: the handshake, the
// reader thread, and the fact that the child is really killed when the
// client goes away.
//
// So this file is a server that speaks just enough of the protocol to be
// believable, and `tests/mcp_stdio.rs` drives it.
//
// Why it is a `harness = false` test target rather than a normal test: the
// Rust test harness writes `running 1 test` to stdout before the first test
// runs. The stdio transport is newline-delimited JSON-RPC, so that line
// would arrive in the middle of the protocol stream. The client tolerates it
// (a line that is not JSON is noise, and there is a test for exactly that),
// but a fake server that prints nothing but protocol is a much better
// thing to test against — a defect in the client cannot hide behind the
// harness's noise.
//
// The other reason: with the harness in charge, the binary would be a test
// runner that also serves. `harness = false` makes this binary what it says
// it is — a server.
//
// One consequence to keep in mind: `cargo test` RUNS this target, so the
// server must not read stdin unless it was asked to. The environment
// variable is that ask; without it the binary exits at once.
use std::io::{BufRead, Write};

/// The environment variable that turns this binary into the MCP server.
///
/// Without it the binary exits immediately — `cargo test` runs this target
/// as part of the suite, and a server that blocked on stdin would hang the
/// whole run.
const SERVER_MODE: &str = "REPOTORY_AGENT_FAKE_MCP";

/// What the fake server answers a `tools/call` with.
///
/// The tests read it back so they can assert the exact text survived the
/// round trip — the payload travels as JSON inside a JSON-RPC envelope, and
/// a naive implementation loses the quotes or the newline.
const TOOL_TEXT: &str = "line one\nline \"two\"\nline three";

/// The error the fake server refuses with, when a tool named `refuse` is
/// called.
const TOOL_ERROR: &str = "the fake server refused the request";

/// The tool list the fake server advertises.
const TOOLS: &str = r#"[{"name":"repotory_context","description":"load a project"},{"name":"repotory_scan","description":"scan repositories"}]"#;

fn main() {
    if std::env::var(SERVER_MODE).is_err() {
        // `cargo test` runs this target. There is no server to be here.
        return;
    }
    serve();
}

/// Serves the protocol on stdin/stdout until the input closes.
///
/// The server answers three methods and ignores everything else:
///
///   - `initialize`  -> the handshake answer
///   - `tools/list`  -> the tool list
///   - `tools/call`  -> the tool text, or an error
///
/// The `notifications/initialized` notification carries no id, so it is
/// recognized by its method name and answered with silence — which is what
/// the protocol says to do, and what the client expects.
fn serve() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // the client closed the pipe
            Ok(_) => {}
            Err(_) => break,
        }
        let request = line.trim();
        if request.is_empty() {
            continue;
        }

        let id = request_id(request);
        let answer = if request.contains("\"method\":\"initialize\"") {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"result":{{"protocolVersion":"2024-11-05","capabilities":{{"tools":{{"listChanged":true}}}},"serverInfo":{{"name":"fake-mcp","version":"0.0.0"}}}}}}"#
            )
        } else if request.contains("\"method\":\"tools/list\"") {
            format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"tools":{TOOLS}}}}}"#)
        } else if request.contains("\"method\":\"tools/call\"") {
            if request.contains("\"name\":\"refuse\"") {
                format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32000,"message":"{TOOL_ERROR}"}}}}"#
                )
            } else {
                format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"result":{{"content":[{{"type":"text","text":{}}}]}}}}"#,
                    json_string(TOOL_TEXT)
                )
            }
        } else {
            // Anything else — a notification, or a method this fake does not
            // know — gets no answer, exactly as the protocol says.
            continue;
        };

        // One message per line, and flushed: the client reads line by line
        // and a buffered answer would look like a hang.
        if writeln!(stdout, "{answer}").is_err() {
            break;
        }
        if stdout.flush().is_err() {
            break;
        }
    }
}

/// The request's id, or 0 when the line carries none.
///
/// A notification has no id, and the fake answers it with silence — but the
/// id is still parsed so the answer lands on the right request.
fn request_id(request: &str) -> u64 {
    let Some(at) = request.find("\"id\":") else {
        return 0;
    };
    let rest = &request[at + 5..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
}

/// A JSON string literal, escaped.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
