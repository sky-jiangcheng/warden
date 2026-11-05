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
        handle(stream);
        if served > 32 {
            break;
        }
    }
}

/// Answers one request.
fn handle(mut stream: TcpStream) {
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
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        let _ = std::io::Read::read_exact(&mut reader, &mut body);
    }

    if path == "/v1/models" {
        let body = format!(
            r#"{{"object":"list","data":[{{"id":"{MODEL}","object":"model"}},{{"id":"other-model","object":"model"}}]}}"#
        );
        respond(&mut stream, "200 OK", "application/json", body.as_bytes());
        return;
    }

    if path == "/v1/chat/completions" {
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
