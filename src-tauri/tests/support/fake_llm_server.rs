// A fake OpenAI-compatible server, for the end-to-end test.
//
// It speaks the same protocol a real provider does: `GET /models` answers
// with a `data` array, and `POST /chat/completions` answers with an SSE
// stream. The test drives it over a real TCP socket, so the client's HTTP
// handling — the URL, the headers, the streaming decoder — is exercised for
// real rather than mocked away.
//
// It is a `[[bin]]` with `required-features = ["test-fixtures"]` for the same
// reason the fake MCP server is: Cargo's `CARGO_BIN_EXE_<name>` is set for
// binaries and not for test targets, and a test fixture has no business in a
// shipped build.
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// The model the fake server advertises.
const MODEL: &str = "fake-model-1";

/// The answer the fake server streams, one chunk at a time.
const CHUNKS: [&str; 3] = ["Hello", ", ", "world"];

/// Serves until the process is killed.
fn main() {
    // `cargo test` runs this target. There is no server to be here.
    if std::env::var("REPOTORY_AGENT_FAKE_LLM").is_err() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    // The port is printed so the test can read it: port 0 means the OS
    // picks, and the test needs to know which one it was.
    println!("{}", listener.local_addr().expect("addr").port());
    let mut served = 0usize;
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        served += 1;
        handle(stream, served);
        if served > 32 {
            break;
        }
    }
}

/// Answers one request.
fn handle(mut stream: TcpStream, served: usize) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();

    // Drain the headers, and the body when there is one.
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut request_body = vec![0u8; content_length];
    let _ = std::io::Read::read_exact(&mut reader, &mut request_body);
    let request: serde_json::Value = serde_json::from_slice(&request_body).unwrap_or_default();

    if path == "/v1/models" {
        let body = format!(
            r#"{{"object":"list","data":[{{"id":"{MODEL}","object":"model"}},{{"id":"other-model","object":"model"}}]}}"#
        );
        respond(&mut stream, "200 OK", "application/json", body.as_bytes());
        return;
    }

    if path == "/v1/chat/completions" {
        let model = request["model"].as_str().unwrap_or("");
        match model {
            "fail" => {
                respond(
                    &mut stream,
                    "503 Unavailable",
                    "application/json",
                    br#"{"error":{"message":"fixture provider unavailable"}}"#,
                );
                return;
            }
            "history" => {
                let answer =
                    serde_json::json!({"requestNumber":served,"messages":request["messages"]})
                        .to_string();
                let body = format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    serde_json::json!({"choices":[{"delta":{"content":answer}}]})
                );
                respond(&mut stream, "200 OK", "text/event-stream", body.as_bytes());
                return;
            }
            "truncated" => {
                respond(
                    &mut stream,
                    "200 OK",
                    "text/event-stream",
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
                );
                return;
            }
            // A reasoning model: empty `content` frames carrying `reasoning`
            // arrive before any token, and another empty one carries
            // `finish_reason`. Only the literal `data: [DONE]` ends the
            // stream. This is the shape OpenRouter sends for a reasoning
            // model, and it used to make chat return an empty answer on the
            // very first frame while the connection test passed.
            "reasoning" => {
                let frames = [
                    r#"{"choices":[{"delta":{"content":"","reasoning":"\n"},"finish_reason":null}]}"#,
                    r#"{"choices":[{"delta":{"content":"","reasoning":"thinking"},"finish_reason":null}]}"#,
                    r#"{"choices":[{"delta":{"content":"\nready"},"finish_reason":null}]}"#,
                    r#"{"choices":[{"delta":{"content":""},"finish_reason":"stop"}]}"#,
                ];
                let mut body = String::new();
                for frame in frames {
                    body.push_str(&format!("data: {frame}\n\n"));
                }
                body.push_str("data: [DONE]\n\n");
                respond(&mut stream, "200 OK", "text/event-stream", body.as_bytes());
                return;
            }
            "sse-error" => {
                respond(
                    &mut stream,
                    "200 OK",
                    "text/event-stream",
                    b"data: {\"error\":{\"message\":\"fixture stream failed\"}}\n\n",
                );
                return;
            }
            // The function-calling fixture. `tools` asks for repotory_metrics;
            // `tools:<name>` asks for any tool by name (the whitelist test
            // uses this to name a write tool the model must be refused).
            // The first request is answered with a tool_call; every later one
            // echoes the request's messages and tools, so the test can assert
            // exactly what the caller sent back up the wire.
            m if m == "tools" || m.starts_with("tools:") => {
                let tool = m.strip_prefix("tools:").unwrap_or("repotory_metrics");
                if served == 1 {
                    // Each tool needs arguments its REAL schema accepts —
                    // a fixture that sends `{}` to a tool requiring `message`
                    // tests the refusal path by accident, not on purpose.
                    //
                    // The value is the model's `arguments` field, i.e. a JSON
                    // string CONTAINING JSON. `json!` would escape it again,
                    // so the object is built and stringified explicitly:
                    // double-escaping here silently turned every argument
                    // object into `{}` and made the gate look broken.
                    let arguments = match tool {
                        "repotory_metrics" => json!({"path": "/repo/one"}),
                        "warden_git_commit" => json!({"message": "commit this for me"}),
                        _ => json!({}),
                    }
                    .to_string();
                    let chunk = serde_json::json!({
                        "choices": [{
                            "delta": {"tool_calls": [{
                                "index": 0,
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": tool, "arguments": arguments},
                            }]},
                            "finish_reason": "tool_calls",
                        }]
                    });
                    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                    respond(&mut stream, "200 OK", "text/event-stream", body.as_bytes());
                } else {
                    let answer = serde_json::json!({
                        "messages": request["messages"],
                        "tools": request["tools"],
                    })
                    .to_string();
                    let body = format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        serde_json::json!({"choices":[{"delta":{"content":answer}}]})
                    );
                    respond(&mut stream, "200 OK", "text/event-stream", body.as_bytes());
                }
                return;
            }
            _ => {}
        }
        // The SSE stream. Each chunk is one `data:` line, and the stream
        // ends with the `[DONE]` sentinel — the exact shape the client has
        // to decode.
        let mut body = String::new();
        for chunk in CHUNKS {
            body.push_str(&format!(
                "data: {{\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{chunk}\"}},\"finish_reason\":null}}]}}\n\n"
            ));
        }
        body.push_str("data: [DONE]\n\n");
        respond(&mut stream, "200 OK", "text/event-stream", body.as_bytes());
        return;
    }

    respond(
        &mut stream,
        "404 Not Found",
        "application/json",
        br#"{"error":{"message":"no such endpoint"}}"#,
    );
}

/// Writes a complete HTTP response.
fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
