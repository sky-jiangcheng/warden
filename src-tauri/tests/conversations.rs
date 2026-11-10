//! Exercise the production conversation path over real MCP pipes and HTTP.
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use repotory_agent_lib::conversations::{self, ChatChunk, ChatRequest, GenerationState, ToolEvent};
use repotory_agent_lib::mcp_client::{McpClient, McpSession};
use repotory_agent_lib::providers::{Provider, ProviderConfig, ProviderStore};
use serde_json::{Value, json};

const THREAD: &str = "10000000-0000-4000-8000-000000000001";
const OTHER: &str = "10000000-0000-4000-8000-000000000002";
static NEXT: AtomicU64 = AtomicU64::new(0);

async fn session(mode: &str) -> McpSession {
    let binary = std::env::var("CARGO_BIN_EXE_fake-mcp-server").unwrap();
    let (client, tools) = McpClient::connect_to(Path::new(&binary), Some(mode))
        .await
        .unwrap();
    McpSession::from_connected(client, tools)
}

struct Fixture {
    llm: Child,
    session: McpSession,
    state: GenerationState,
    store: ProviderStore,
    directory: PathBuf,
}
impl Fixture {
    async fn new(mode: &str) -> Self {
        let mut llm = Command::new(std::env::var("CARGO_BIN_EXE_fake-llm-server").unwrap())
            .env("REPOTORY_AGENT_FAKE_LLM", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut port = String::new();
        BufReader::new(llm.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let directory = std::env::temp_dir().join(format!(
            "warden-conversations-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let store = ProviderStore::new(directory.join("providers.json"));
        store
            .save(&ProviderConfig {
                providers: vec![Provider {
                    id: "fake".into(),
                    name: "Fake".into(),
                    base_url: format!("http://127.0.0.1:{}/v1", port.trim()),
                    default_model: Some("history".into()),
                    headers: Default::default(),
                    has_api_key: false,
                }],
                active_provider_id: Some("fake".into()),
            })
            .unwrap();
        Self {
            llm,
            session: session(mode).await,
            state: GenerationState::default(),
            store,
            directory,
        }
    }
    async fn create(&self, id: &str, path: &str) {
        conversations::create(&self.session, id.into(), "Topic".into(), Some(path.into()))
            .await
            .unwrap();
    }
    async fn send(&self, request: ChatRequest) -> (Result<(), String>, Vec<ChatChunk>) {
        let mut chunks = Vec::new();
        let result =
            conversations::send(&self.session, &self.state, &self.store, request, |chunk| {
                chunks.push(chunk)
            })
            .await;
        (result, chunks)
    }
    async fn messages(&self) -> Vec<Value> {
        conversations::read(&self.session, THREAD.into())
            .await
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.llm.kill();
        let _ = self.llm.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn request(id: &str, message: &str) -> ChatRequest {
    ChatRequest {
        message_id: id.into(),
        message: message.into(),
        thread_id: Some(THREAD.into()),
        model: None,
        provider_id: None,
        watched: Vec::new(),
    }
}
fn answer(chunks: &[ChatChunk]) -> Value {
    let last = chunks.last().unwrap();
    assert!(last.done);
    assert!(last.error.is_none());
    serde_json::from_str(&last.text).unwrap()
}

#[tokio::test]
async fn shared_operations_preserve_json_shapes() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    f.create(OTHER, "/repo/two").await;
    let list = conversations::list(&f.session, Some("/repo/one".into()))
        .await
        .unwrap();
    assert_eq!(list["threads"].as_array().unwrap().len(), 1);
    assert_eq!(list["threads"][0]["id"], THREAD);
    assert!(list["threads"][0]["createdAt"].is_string());
    let renamed = conversations::rename(&f.session, THREAD.into(), "Renamed".into())
        .await
        .unwrap();
    assert_eq!(renamed["thread"]["title"], "Renamed");
    assert!(f.messages().await.is_empty());
}

#[tokio::test]
async fn a_title_wider_than_cinders_own_cap_is_refused_before_the_wire() {
    // The fixture server stores whatever title it is handed, so this test only
    // fails if the local cap is raised — that is the drift being pinned. Warden
    // must refuse at Cinder's number (service.MaxChatTitleLen = 160), not its own.
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let err = conversations::rename(&f.session, THREAD.into(), "标".repeat(161))
        .await
        .unwrap_err();
    assert!(err.contains("1–160"), "{err}");
    assert!(f.messages().await.is_empty());
    let renamed = conversations::rename(&f.session, THREAD.into(), "标".repeat(160))
        .await
        .unwrap();
    assert_eq!(renamed["thread"]["title"], "标".repeat(160));
}

#[tokio::test]
async fn real_provider_receives_ordered_multiturn_history_not_another_topic() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/one").await;
    f.create(OTHER, "/two").await;
    f.session.call_tool("repotory_chat", json!({"operation":"append","thread_id":OTHER,"id":"foreign","role":"user","content":"unrelated secret","source":"vscode"})).await.unwrap();
    let (result, first) = f.send(request("m1", "记住蓝色")).await;
    result.unwrap();
    assert!(first.iter().any(|c| !c.done));
    let (result, second) = f.send(request("m2", "是什么颜色？")).await;
    result.unwrap();
    let payload = answer(&second);
    assert_eq!(payload["requestNumber"], 2);
    let sent = payload["messages"].as_array().unwrap();
    // When tools are offered, Warden's OWN system prompt leads — assembled
    // here, never accepted from the server (the no-injection rule), and it
    // carries the thread's repository scope.
    assert_eq!(sent.len(), 4);
    assert_eq!(sent[0]["role"], "system");
    assert!(
        sent[0]["content"]
            .as_str()
            .unwrap()
            .contains("Current repository: /one")
    );
    assert_eq!(sent[1], json!({"role":"user","content":"记住蓝色"}));
    assert_eq!(
        sent[2],
        json!({"role":"assistant","content":first.last().unwrap().text})
    );
    assert_eq!(sent[3]["content"], "是什么颜色？");
    assert!(!payload.to_string().contains("unrelated secret"));
    let messages = f.messages().await;
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[3]["id"], "m2-reply");
    assert_eq!(messages[3]["replyTo"], "m2");
    assert_eq!(messages[3]["source"], "warden");
}

#[tokio::test]
async fn completed_reply_replays_without_llm_even_after_local_state_is_lost() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/one").await;
    let (result, first) = f.send(request("same", "hello")).await;
    result.unwrap();
    let mut replay = Vec::new();
    conversations::send(
        &f.session,
        &GenerationState::default(),
        &f.store,
        request("same", "hello"),
        |c| replay.push(c),
    )
    .await
    .unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].text, first.last().unwrap().text);
    assert_eq!(f.messages().await.len(), 2);
    let (result, next) = f.send(request("next", "next")).await;
    result.unwrap();
    assert_eq!(answer(&next)["requestNumber"], 2);
    let (result, chunks) = f.send(request("same", "changed")).await;
    assert!(result.unwrap_err().contains("immutable"));
    assert!(chunks.last().unwrap().error.is_some());
}

#[tokio::test]
async fn provider_failure_keeps_user_and_never_fabricates_or_retries_an_answer() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/one").await;
    let mut failed = request("bad", "hello");
    failed.model = Some("fail".into());
    for _ in 0..2 {
        let (result, chunks) = f.send(failed.clone()).await;
        assert!(result.unwrap_err().contains("fixture provider unavailable"));
        assert!(chunks.last().unwrap().done);
        assert!(chunks.last().unwrap().error.is_some());
    }
    assert_eq!(f.messages().await.len(), 1);
    assert_eq!(f.messages().await[0]["role"], "user");
    let (result, chunks) = f.send(request("new-id", "retry explicitly")).await;
    result.unwrap();
    assert_eq!(answer(&chunks)["requestNumber"], 2);
}

#[tokio::test]
async fn assistant_save_failure_retries_only_persistence() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT_SAVE_FAIL").await;
    f.create(THREAD, "/one").await;
    let (result, chunks) = f.send(request("m", "hello")).await;
    assert!(result.unwrap_err().contains("could not be saved"));
    assert!(!chunks.last().unwrap().text.is_empty());
    assert_eq!(f.messages().await.len(), 1);
    let (result, retry) = f.send(request("m", "hello")).await;
    result.unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(answer(&retry)["requestNumber"], 1);
    assert_eq!(f.messages().await.len(), 2);
    let (result, next) = f.send(request("next", "next")).await;
    result.unwrap();
    assert_eq!(answer(&next)["requestNumber"], 2);
}

#[tokio::test]
async fn validation_fails_before_writing_a_turn_and_reports_on_channel() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/one").await;
    for invalid in [
        request("m", " "),
        request("", "hello"),
        request("m", &"界".repeat(12_001)),
    ] {
        let (result, chunks) = f.send(invalid).await;
        assert!(result.is_err());
        assert!(chunks.last().unwrap().error.is_some());
    }
    let (mut config, _) = f.store.load();
    config.providers[0].base_url = "not-a-url".into();
    f.store.save(&config).unwrap();
    let (result, chunks) = f.send(request("bad-config", "hello")).await;
    assert!(result.is_err());
    assert!(chunks.last().unwrap().error.is_some());
    assert!(f.messages().await.is_empty());
}

#[tokio::test]
async fn missing_capabilities_and_invalid_json_are_explicit_errors() {
    let missing = session("REPOTORY_AGENT_FAKE_MCP").await;
    assert!(
        conversations::list(&missing, None)
            .await
            .unwrap_err()
            .contains("does not provide repotory_chat")
    );
    let malformed = session("REPOTORY_AGENT_FAKE_CHAT_BAD_JSON").await;
    assert!(
        conversations::list(&malformed, None)
            .await
            .unwrap_err()
            .contains("invalid JSON")
    );
}

#[tokio::test]
async fn no_cinder_mode_stays_single_turn_and_deduplicates_within_the_process() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_MCP").await;
    let mut first = request("single", "hello");
    first.thread_id = None;
    let (result, chunks) = f.send(first.clone()).await;
    result.unwrap();
    assert_eq!(answer(&chunks)["messages"].as_array().unwrap().len(), 1);
    f.send(first).await.0.unwrap();
    let mut second = request("second", "different standalone question");
    second.thread_id = None;
    let (result, chunks) = f.send(second).await;
    result.unwrap();
    let payload = answer(&chunks);
    assert_eq!(payload["requestNumber"], 2);
    assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn truncated_and_error_streams_are_not_saved_as_completed_assistants() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/one").await;
    for model in ["truncated", "sse-error"] {
        let mut req = request(model, "hello");
        req.model = Some(model.into());
        let (result, chunks) = f.send(req).await;
        assert!(result.is_err());
        assert!(chunks.last().unwrap().error.is_some());
    }
    let messages = f.messages().await;
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m["role"] == "user"));
}

#[tokio::test]
async fn latest_500_are_ordered_and_an_aged_out_user_still_has_immutable_identity() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/one").await;
    f.send(request("old", "original")).await.0.unwrap();
    for index in 0..499 {
        f.session
            .call_tool(
                "repotory_chat",
                json!({"operation":"append","thread_id":THREAD,
            "id":format!("filler-{index}"),"role":"user","content":"from IDE","source":"vscode"}),
            )
            .await
            .unwrap();
    }
    let messages = f.messages().await;
    assert_eq!(messages.len(), 500);
    assert_eq!(messages[0]["id"], "old-reply");
    assert_eq!(messages[499]["id"], "filler-498");
    let mut chunks = Vec::new();
    conversations::send(
        &f.session,
        &GenerationState::default(),
        &f.store,
        request("old", "original"),
        |c| chunks.push(c),
    )
    .await
    .unwrap();
    assert_eq!(answer(&chunks)["requestNumber"], 1);
    let error = conversations::send(
        &f.session,
        &GenerationState::default(),
        &f.store,
        request("old", "changed"),
        |_| {},
    )
    .await
    .unwrap_err();
    assert!(error.contains("immutable"));
}

#[tokio::test]
async fn missing_shared_chat_does_not_silently_fall_back_to_single_turn() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_MCP").await;
    let (result, chunks) = f.send(request("m", "hello")).await;
    assert!(
        result
            .unwrap_err()
            .contains("does not provide repotory_chat")
    );
    let last = chunks.last().unwrap();
    assert!(last.done && last.error.is_some() && last.text.is_empty());
}

#[tokio::test]
async fn same_thread_parallel_requests_do_not_issue_parallel_generations() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/one").await;
    let (first, second) =
        tokio::join!(f.send(request("a", "hello")), f.send(request("b", "hello")));
    first.0.unwrap();
    assert!(second.0.unwrap_err().contains("already generating"));
    assert_eq!(f.messages().await.len(), 2);
}

/// Function Calling over the real pipes: the model asks for a memory tool,
/// Warden executes it on the MCP session, feeds the result back, and the
/// final answer is the second round's text. Everything asserted here is the
/// protocol shape a strict provider validates: the assistant echo carries its
/// tool_calls, the tool result names the same id, and the offered tools carry
/// the server's own schema.
#[tokio::test]
async fn a_tool_round_executes_the_memory_tool_and_feeds_the_result_back() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let mut req = request("m1", "how much code today?");
    req.model = Some("tools".into());
    let (result, chunks) = f.send(req).await;
    result.expect("send");

    // The events the UI renders: one started chip and one finished chip for
    // the call the model made.
    let events: Vec<&ToolEvent> = chunks.iter().filter_map(|c| c.tool_call.as_ref()).collect();
    assert_eq!(events.len(), 2, "expected one start and one finish event");
    assert_eq!(events[0].name, "repotory_metrics");
    assert!(!events[0].done);
    assert_eq!(events[1].name, "repotory_metrics");
    assert!(events[1].done);

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    // The system prompt is Warden's own assembly, and it carries the thread's
    // repository scope (the M2 "context wiring" debt).
    let system = messages
        .iter()
        .find(|m| m["role"] == "system")
        .expect("system prompt");
    assert!(system["content"].as_str().unwrap().contains("/repo/one"));
    // An empty watchlist adds nothing: the prompt must not claim a list the
    // user never adopted.
    assert!(!system["content"].as_str().unwrap().contains("watchlist"));
    // The assistant echo carries the calls it made; the tool result names the
    // same id and holds what the fake MCP answered.
    let echo = messages
        .iter()
        .find(|m| m["role"] == "assistant" && m["tool_calls"].is_array())
        .expect("assistant echo");
    assert_eq!(echo["tool_calls"][0]["id"], "call_1");
    assert_eq!(echo["tool_calls"][0]["type"], "function");
    assert!(
        echo["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap()
            .contains("/repo/one")
    );
    let tool_message = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result message");
    assert_eq!(tool_message["tool_call_id"], "call_1");
    assert!(
        tool_message["content"]
            .as_str()
            .unwrap()
            .contains("repositoryCount")
    );
    // The offered tools carried the server's schema through verbatim, and the
    // whitelist offered exactly the read-only face.
    let tools = reply["tools"].as_array().unwrap();
    assert_eq!(
        tools.len(),
        2,
        "only whitelisted read-only tools are offered"
    );
    assert_eq!(tools[0]["function"]["name"], "repotory_metrics");
    assert_eq!(tools[1]["function"]["name"], "repotory_projects_list");
    assert!(tools[0]["function"]["parameters"]["properties"]["path"].is_object());

    // The persisted reply is that same second-round text.
    let saved = f.messages().await;
    assert_eq!(saved.last().unwrap()["role"], "assistant");
    assert!(
        saved.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("repositoryCount")
    );
}

/// The watchlist is Warden's own list and Cinder's starred projects are the
/// memory layer's; the system prompt names both apart so "which repos do I
/// watch" cannot be answered with the wrong one.
#[tokio::test]
async fn the_watchlist_is_named_in_the_system_prompt_as_wardens_own_list() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let mut req = request("m4", "which repos do I watch?");
    req.model = Some("tools".into());
    req.watched = vec!["/work/alpha".into(), "/work/beta".into()];
    let (result, chunks) = f.send(req).await;
    result.expect("send");

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let system = messages
        .iter()
        .find(|m| m["role"] == "system")
        .expect("system prompt");
    let prompt = system["content"].as_str().unwrap();
    assert!(
        prompt.contains("/work/alpha") && prompt.contains("/work/beta"),
        "{prompt}"
    );
    // The distinction is the point: the two lists must not be conflated.
    assert!(prompt.contains("watchlist"), "{prompt}");
    assert!(prompt.contains("NOT the same"), "{prompt}");
}

/// A tool whose answer is a JSON ARRAY — the real `repotory_projects_list`
/// contract — completes the round and reaches the model verbatim. Warden used
/// to require an object from every tool and reported the array as
/// "must return a JSON object", which the model then read as a broken Cinder.
#[tokio::test]
async fn an_array_tool_answer_reaches_the_model_verbatim() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let mut req = request("m3", "which repos does Cinder know?");
    req.model = Some("tools:repotory_projects_list".into());
    let (result, chunks) = f.send(req).await;
    result.expect("send");

    let events: Vec<&ToolEvent> = chunks.iter().filter_map(|c| c.tool_call.as_ref()).collect();
    assert_eq!(events.len(), 2);
    assert!(events[1].done);
    assert!(
        events[1].ok,
        "the array answer must not be refused: {:?}",
        events[1].error
    );

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result message");
    let content: serde_json::Value =
        serde_json::from_str(tool_message["content"].as_str().unwrap()).expect("tool JSON");
    assert!(
        content.is_array(),
        "the array shape survives to the model: {content}"
    );
    assert_eq!(content[0]["name"], "cinder");
}

/// A write tool the server advertises is never offered to the model, and a
/// model that names one anyway gets a client-side refusal as its result —
/// never an execution on the wire.
#[tokio::test]
async fn a_write_tool_is_never_offered_and_a_refusal_is_its_result() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let mut req = request("m2", "write a note");
    req.model = Some("tools:repotory_notes_create".into());
    let (result, chunks) = f.send(req).await;
    result.expect("send");

    let reply = answer(&chunks);
    let tools = reply["tools"].as_array().unwrap();
    assert!(
        tools
            .iter()
            .any(|t| t["function"]["name"] == "repotory_metrics")
    );
    assert!(
        !tools
            .iter()
            .any(|t| t["function"]["name"] == "repotory_notes_create")
    );

    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result message");
    let content = tool_message["content"].as_str().unwrap();
    assert!(content.contains("not available to the model"), "{content}");
    assert!(
        !content.contains("created"),
        "the write tool must not have executed: {content}"
    );

    let events: Vec<&ToolEvent> = chunks.iter().filter_map(|c| c.tool_call.as_ref()).collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name, "repotory_notes_create");
    assert!(!events[0].done);
    assert!(events[1].done);
}
