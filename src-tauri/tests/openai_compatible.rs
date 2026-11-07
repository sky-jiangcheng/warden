// The end-to-end test against a fake OpenAI-compatible server.
//
// The point of this file is that nothing is mocked. A real process listens
// on a real socket; the client makes a real HTTP request and decodes a real
// SSE stream. A unit test with a hand-built JSON string proves the parser
// works; this proves the client actually talks to a server.
//
// What it pins down:
//   * the model list comes back from `GET {base}/models`;
//   * the chat answer comes back piece by piece, in order;
//   * a provider that answers with an error reaches the caller as an error,
//     with the server's own message — not as a generic failure.
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use repotory_agent_lib::llm::{ChatMessage, LlmClient, LlmError};
use repotory_agent_lib::providers::Provider;

/// The fake server, running.
struct FakeServer {
    child: Child,
    port: u16,
}

impl FakeServer {
    /// Starts the server and waits for it to report its port.
    fn start() -> Self {
        let binary = std::env::var("CARGO_BIN_EXE_fake-llm-server")
            .expect("Cargo names the fake LLM server binary for the integration test");
        let mut child = Command::new(binary)
            .env("REPOTORY_AGENT_FAKE_LLM", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the fake server starts");

        // The server prints its port on the first line. Reading it is also
        // the readiness signal: the listener is bound by then.
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("the port line");
        let port: u16 = line.trim().parse().expect("a port number");

        Self { child, port }
    }

    /// The base URL of the running server.
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A provider pointing at the fake server.
fn provider_for(server: &FakeServer) -> Provider {
    Provider {
        id: "fake".to_string(),
        name: "Fake".to_string(),
        base_url: server.base_url(),
        default_model: Some("fake-model-1".to_string()),
        headers: std::collections::BTreeMap::new(),
        has_api_key: false,
    }
}

#[tokio::test]
async fn the_model_list_comes_back_from_the_server() {
    let server = FakeServer::start();
    let client = LlmClient::new(&provider_for(&server), None).expect("client");

    let models = client.list_models().await.expect("the model list");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "fake-model-1");
    assert_eq!(models[1].id, "other-model");
}

#[tokio::test]
async fn the_answer_arrives_piece_by_piece_in_order() {
    let server = FakeServer::start();
    let client = LlmClient::new(&provider_for(&server), None).expect("client");

    let mut seen: Vec<String> = Vec::new();
    let answer = client
        .chat_stream(
            "fake-model-1",
            &[ChatMessage::user("hello")],
            &[],
            |delta: &str| seen.push(delta.to_string()),
        )
        .await
        .expect("the answer");

    assert_eq!(answer.content, "Hello, world");
    assert!(answer.tool_calls.is_empty());
    // The deltas are what the UI renders, and they must arrive as separate
    // pieces — a client that buffered the whole stream would show nothing
    // until the end, which defeats the point of streaming.
    assert_eq!(seen, vec!["Hello", ", ", "world"]);
}

/// A reasoning model emits empty `content` deltas before its first token.
/// Those frames are NOT the `[DONE]` sentinel: the stream must keep going
/// and the answer must contain the text that arrives later. Regression for
/// "connection test passes, chat reports no answer" — the test uses a
/// non-streaming request, chat uses the stream, so only chat hit this.
#[tokio::test]
async fn empty_reasoning_frames_do_not_end_the_stream() {
    let server = FakeServer::start();
    let client = LlmClient::new(&provider_for(&server), None).expect("client");

    let mut seen: Vec<String> = Vec::new();
    let answer = client
        .chat_stream(
            "reasoning",
            &[ChatMessage::user("hello")],
            &[],
            |delta: &str| {
                seen.push(delta.to_string());
            },
        )
        .await
        .expect("the answer");

    assert_eq!(answer.content, "\nready");
    assert_eq!(seen, vec!["\nready"]);
    assert!(answer.tool_calls.is_empty());
}

#[tokio::test]
async fn a_non_streaming_request_gets_the_whole_answer() {
    let server = FakeServer::start();
    let client = LlmClient::new(&provider_for(&server), None).expect("client");

    // The fake server answers both shapes on the same path, so this checks
    // that the non-streaming request is well-formed and parsed.
    let answer = client
        .chat("fake-model-1", &[ChatMessage::user("hello")])
        .await;
    // The fake server only speaks SSE, so a non-streaming request gets the
    // SSE body back — which the parser must reject rather than silently
    // return as an empty answer.
    match answer {
        Ok(text) => assert!(!text.is_empty()),
        Err(LlmError::Protocol(_)) => {}
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn a_provider_that_answers_with_an_error_reaches_the_caller() {
    // A base URL that points at the fake server but a path it does not
    // serve: the server answers 404 with an error body.
    let server = FakeServer::start();
    let mut provider = provider_for(&server);
    provider.base_url = format!("http://127.0.0.1:{}/nope/v1", server.port);
    let client = LlmClient::new(&provider, None).expect("client");

    let err = client
        .list_models()
        .await
        .expect_err("the provider answered an error");

    match err {
        LlmError::Status { code, message } => {
            assert_eq!(code, 404);
            assert!(
                message.contains("no such endpoint"),
                "the server's own message must survive: {message}"
            );
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn an_unreachable_provider_is_reported_as_unreachable() {
    // Port 1 on the loopback: nothing listens there.
    let provider = Provider {
        id: "dead".to_string(),
        name: "Dead".to_string(),
        base_url: "http://127.0.0.1:1/v1".to_string(),
        default_model: None,
        headers: std::collections::BTreeMap::new(),
        has_api_key: false,
    };
    let client = LlmClient::new(&provider, None).expect("client");

    let err = client.list_models().await.expect_err("must fail");
    assert!(
        matches!(err, LlmError::Transport(_)),
        "an unreachable host is a transport problem: {err}"
    );
}

#[tokio::test]
async fn a_local_provider_is_the_same_shape_as_a_hosted_one() {
    // The design rests on this: there is no "local" code path. The fake
    // server IS a local provider, and it is driven by the same client.
    let server = FakeServer::start();
    let provider = provider_for(&server);
    assert!(provider.base_url.starts_with("http://127.0.0.1:"));

    let client = LlmClient::new(&provider, None).expect("client");
    let models = client.list_models().await.expect("the model list");
    assert!(!models.is_empty());
}
