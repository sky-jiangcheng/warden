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
    session_env(mode, &[]).await
}

/// Like `session`, with extra environment variables handed to the fake
/// server — per-fixture paths (the handoff log) cannot travel through the
/// process environment, because the harness runs these tests in parallel.
async fn session_env(mode: &str, extra: &[(&str, &str)]) -> McpSession {
    let binary = std::env::var("CARGO_BIN_EXE_fake-mcp-server").unwrap();
    let envs: Vec<(String, String)> = std::iter::once((mode.to_string(), "1".to_string()))
        .chain(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let (client, tools) = McpClient::connect_with_env(Path::new(&binary), &envs)
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
        Self::with_server_env(mode, &[]).await
    }
    async fn with_server_env(mode: &str, extra: &[(&str, &str)]) -> Self {
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
        let state = GenerationState::default();
        // Attach the knowledge-write confirmation sink to a file in the
        // fixture directory so the gating tests can read back what was
        // recorded. `state.audit` (the command log) is deliberately left
        // unattached: the exec tests attach it to their own per-test file,
        // and `AuditLog::attach` is first-wins, so a fixture-level attach
        // would silently win over theirs.
        state
            .knowledge_audit
            .attach(directory.join("knowledge-audit.jsonl"));
        Self {
            llm,
            session: session_env(mode, extra).await,
            state,
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

    /// Sends and answers confirmation cards as they arrive.
    ///
    /// The card is delivered mid-generation, so a test cannot collect the
    /// chunks first and answer after: the generation is blocked on the
    /// answer. This drives the same `GenerationState::decide` path the
    /// approve/refuse commands call, and records every card it saw so the
    /// test can assert what the user was shown.
    async fn send_and_answer(
        &self,
        request: ChatRequest,
        approve: bool,
    ) -> (Result<(), String>, Vec<ChatChunk>, Vec<Value>) {
        let cards = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let seen = std::sync::Arc::clone(&cards);
        let state = &self.state;
        let mut chunks = Vec::new();
        let result = conversations::send(&self.session, state, &self.store, request, |chunk| {
            if let Some(card) = &chunk.approval {
                seen.lock()
                    .unwrap()
                    .push(serde_json::to_value(card).expect("a card serialises"));
                let decision = if approve {
                    conversations::Decision::Approved(None)
                } else {
                    conversations::Decision::Refused
                };
                assert!(state.decide(&card.id, decision), "the card id must be live");
            }
            chunks.push(chunk)
        })
        .await;
        let cards = cards.lock().unwrap().clone();
        (result, chunks, cards)
    }
    async fn messages(&self) -> Vec<Value> {
        conversations::read(&self.session, THREAD.into())
            .await
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// The knowledge-write confirmation log, read back as JSON lines.
    fn knowledge_audit(&self) -> Vec<Value> {
        read_jsonl(&self.directory.join("knowledge-audit.jsonl"))
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
    // The offered tools carried the server's schema through verbatim, and
    // the whitelist offered exactly the read-only face — plus Warden's own
    // local read-only tools, which Phase 3 adds once the thread has a
    // repository to run them in. The memory tools come first: the server's
    // face is appended first, the local face extends it.
    let tools = reply["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        &names[..3],
        &[
            "repotory_metrics",
            "repotory_projects_list",
            "repotory_context"
        ],
        "the memory whitelist leads: {names:?}"
    );
    // The memory tools offered are exactly (what the server advertises) ∩
    // (the whitelist). The fixture advertises four readable tools plus one
    // knowledge write; the read-only count stays four and the write rides
    // along as a gated tool — counting by rule, not by hardcoded number,
    // keeps this assertion about the RULE when a fixture grows a tool.
    let memory_offered: Vec<&&str> = names
        .iter()
        .filter(|n| n.starts_with("repotory_"))
        .collect();
    let read_only = memory_offered
        .iter()
        .filter(|n| ***n != "repotory_notes_create")
        .count();
    assert_eq!(
        read_only, 4,
        "the fixture advertises four readable memory tools: {names:?}"
    );
    assert!(
        memory_offered.contains(&&"repotory_wiki_search"),
        "{names:?}"
    );
    // The knowledge write is offered (the gate is on execution, not
    // visibility) and its description carries the approval note.
    assert!(
        memory_offered.contains(&&"repotory_notes_create"),
        "the knowledge write rides with the face: {names:?}"
    );
    let write_spec = tools
        .iter()
        .find(|t| t["function"]["name"] == "repotory_notes_create")
        .expect("the knowledge write spec");
    assert!(
        write_spec["function"]["description"]
            .as_str()
            .unwrap()
            .contains("CONFIRMED BY THE USER"),
        "the model is told the write is gated: {write_spec:?}"
    );
    assert_eq!(
        names.iter().filter(|n| n.starts_with("warden_")).count(),
        repotory_agent_lib::exec::LOCAL_TOOLS.len() + repotory_agent_lib::exec::WRITE_TOOLS.len(),
        "the read-only face and the approval-gated set both ride along: {names:?}"
    );
    assert!(!names.contains(&"repotory_handoff"), "{names:?}");
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

/// `repotory_context` answers with a PLAIN-TEXT briefing — its server-side
/// handler returns a string, not JSON. Warden used to parse every model tool
/// answer as JSON, so the model was handed "returned invalid JSON: expected
/// value at line 1 column 1" for a perfectly healthy Cinder. The prose must
/// reach the model verbatim.
#[tokio::test]
async fn a_plain_text_tool_answer_reaches_the_model_verbatim() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let mut req = request("m5", "what is this project about?");
    req.model = Some("tools:repotory_context".into());
    let (result, chunks) = f.send(req).await;
    result.expect("send");

    let events: Vec<&ToolEvent> = chunks.iter().filter_map(|c| c.tool_call.as_ref()).collect();
    assert_eq!(events.len(), 2);
    assert!(
        events[1].ok,
        "the prose answer must not be refused: {:?}",
        events[1].error
    );

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result message");
    assert_eq!(
        tool_message["content"].as_str().unwrap(),
        "Project: warden\nTech stack: Rust + TypeScript\nRecent commits: 3 today"
    );
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

/// A knowledge write the server advertises is offered to the model, but every
/// call is gated behind a human confirmation card. When the user refuses,
/// nothing is stored and the refusal is the tool result — plus a traceable
/// audit entry (open question 7, fail-closed).
#[tokio::test]
async fn a_knowledge_write_is_gated_and_a_refusal_stores_nothing() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let mut req = request("m2", "write a note");
    req.model = Some("tools:repotory_notes_create".into());
    // Answer the card with a refusal as it arrives.
    let (result, chunks, cards) = f.send_and_answer(req, false).await;
    result.expect("send");

    let reply = answer(&chunks);
    // The write tool is now offered (it rides with the face) — the gate is
    // on execution, not on visibility.
    let tools = reply["tools"].as_array().unwrap();
    assert!(
        tools
            .iter()
            .any(|t| t["function"]["name"] == "repotory_notes_create"),
        "the knowledge write is offered: {tools:?}"
    );

    // The card the user was shown: the tool, the knowledge-write kind, and
    // the proposed arguments (editable).
    assert_eq!(cards.len(), 1, "exactly one confirmation card: {cards:?}");
    assert_eq!(cards[0]["tool"], "repotory_notes_create");
    assert_eq!(cards[0]["kind"], "knowledgeWrite");
    assert!(
        cards[0]["arguments"].is_object(),
        "editable arguments: {cards:?}"
    );

    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result message");
    let content = tool_message["content"].as_str().unwrap();
    assert!(content.contains("refused"), "{content}");
    assert!(
        !content.contains("created"),
        "the write tool must not have executed: {content}"
    );

    // The refusal is traceable: who, when, which tool, what arguments.
    let audit = f.knowledge_audit();
    assert_eq!(audit.len(), 1, "one refusal recorded: {audit:?}");
    assert_eq!(audit[0]["tool"], "repotory_notes_create");
    assert_eq!(audit[0]["ok"], false);
    assert!(audit[0]["error"].as_str().unwrap().contains("refused"));
    assert!(!audit[0]["who"].as_str().unwrap().is_empty());
}

/// Approving a knowledge write runs it with the agreed arguments and records
/// the approval — the other half of the same gate.
#[tokio::test]
async fn an_approved_knowledge_write_runs_and_is_audited() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, "/repo/one").await;
    let mut req = request("m3", "write a note");
    req.model = Some("tools:repotory_notes_create".into());
    let (result, chunks, _cards) = f.send_and_answer(req, true).await;
    result.expect("send");

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result message");
    let content = tool_message["content"].as_str().unwrap();
    assert!(content.contains("created"), "the write ran: {content}");

    let audit = f.knowledge_audit();
    assert_eq!(audit.len(), 1, "one approval recorded: {audit:?}");
    assert_eq!(audit[0]["tool"], "repotory_notes_create");
    assert_eq!(audit[0]["ok"], true);
    assert_eq!(audit[0]["revised"], false);
}

/// Git fixture: a real repository in a temp dir, dirty or clean.
fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("git is installed for the test suite");
    assert!(status.success(), "git {args:?} failed");
}

fn make_repo(name: &str, dirty: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "warden-m2-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create repo dir");
    git(&dir, &["init", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "first"]);
    if dirty {
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    }
    dir
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// M2's promised "附加最近 git diff": the uncommitted working tree rides in
/// the system prompt of a tool-enabled thread — stat, patch excerpt, and the
/// repository path it came from.
#[tokio::test]
async fn the_uncommitted_diff_rides_in_the_system_prompt() {
    let repo = make_repo("prompt-diff", true);
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, repo.to_str().unwrap()).await;
    let (result, chunks) = f.send(request("d1", "what did I change?")).await;
    result.expect("send");

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let system = messages
        .iter()
        .find(|m| m["role"] == "system")
        .expect("system prompt");
    let prompt = system["content"].as_str().unwrap();
    assert!(prompt.contains("Current uncommitted changes"), "{prompt}");
    assert!(prompt.contains("a.txt"), "{prompt}");
    assert!(prompt.contains("+two"), "{prompt}");
    // The scope line is still there — the diff supplements it, not replaces it.
    assert!(prompt.contains(repo.to_str().unwrap()), "{prompt}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// A clean tree says nothing: no block, no "empty diff" noise in the prompt.
#[tokio::test]
async fn a_clean_tree_adds_no_diff_block() {
    let repo = make_repo("prompt-clean", false);
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, repo.to_str().unwrap()).await;
    let (result, chunks) = f.send(request("d2", "how is it going?")).await;
    result.expect("send");

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let system = messages.iter().find(|m| m["role"] == "system").unwrap();
    let prompt = system["content"].as_str().unwrap();
    assert!(!prompt.contains("Current uncommitted changes"), "{prompt}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// M2's closing loop, "模型回答后写入 repotory_handoff": once the reply is
/// durable in Cinder, Warden writes the structured checkpoint ITSELF — the
/// milestone shape of ADR-0054, attributed by path, naming the exchange and
/// the unfinished working tree as next steps.
#[tokio::test]
async fn a_saved_reply_writes_a_milestone_handoff() {
    let repo = make_repo("handoff-ok", true);
    let log = std::env::temp_dir().join(format!(
        "warden-m2-handoff-{}-{}.jsonl",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let f = Fixture::with_server_env(
        "REPOTORY_AGENT_FAKE_CHAT",
        &[("REPOTORY_AGENT_FAKE_HANDOFF_LOG", log.to_str().unwrap())],
    )
    .await;
    f.create(THREAD, repo.to_str().unwrap()).await;
    let (result, chunks) = f.send(request("h1", "summarise my change")).await;
    result.expect("send");
    let answer_text = answer(&chunks)["requestNumber"].to_string();

    let records = read_jsonl(&log);
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record["name"], "repotory_handoff");
    assert_eq!(record["agent"], "warden");
    assert_eq!(record["milestone"], true);
    assert_eq!(record["path"], repo.to_str().unwrap());
    let summary = record["summary"].as_str().unwrap();
    assert!(summary.contains("Warden answered"), "{summary}");
    assert!(summary.contains("summarise my change"), "{summary}");
    // The same diff snapshot the prompt was built from travels as the
    // unfinished work of the checkpoint.
    let steps = record["next_steps"].as_array().unwrap();
    assert!(!steps.is_empty());
    assert!(steps[0].as_str().unwrap().contains("a.txt"), "{steps:?}");

    // A replay of the finished messageId must not write a second checkpoint:
    // the reply is already durable, the handoff belongs to the generation.
    let (again, _) = f.send(request("h1", "summarise my change")).await;
    again.expect("replay");
    assert_eq!(
        read_jsonl(&log).len(),
        1,
        "the replay re-wrote a checkpoint"
    );
    assert!(!answer_text.is_empty());
    let _ = std::fs::remove_dir_all(&log);
    let _ = std::fs::remove_dir_all(&repo);
}

/// The checkpoint is a side effect, not part of the send contract: when the
/// memory layer refuses it (no collected project matches the path), the
/// reply still saves and the send still succeeds.
#[tokio::test]
async fn a_refused_handoff_does_not_fail_the_send() {
    let f = Fixture::with_server_env(
        "REPOTORY_AGENT_FAKE_CHAT",
        &[("REPOTORY_AGENT_FAKE_HANDOFF_FAIL", "1")],
    )
    .await;
    f.create(THREAD, "/repo/one").await;
    let (result, chunks) = f.send(request("h2", "hello")).await;
    result.expect("the handoff refusal must stay in the log, not the send");
    assert!(!answer(&chunks)["requestNumber"].is_null());
    let saved = f.messages().await;
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[1]["role"], "assistant");
}

/// No thread, no durable conversation, no checkpoint: a temporary exchange
/// is exactly the case that has no session to hand off.
#[tokio::test]
async fn a_temporary_conversation_writes_no_handoff() {
    let log = std::env::temp_dir().join(format!(
        "warden-m2-handoff-none-{}.jsonl",
        std::process::id()
    ));
    let f = Fixture::with_server_env(
        "REPOTORY_AGENT_FAKE_CHAT",
        &[("REPOTORY_AGENT_FAKE_HANDOFF_LOG", log.to_str().unwrap())],
    )
    .await;
    let mut req = request("h3", "quick one-off question");
    req.thread_id = None;
    let (result, _) = f.send(req).await;
    result.expect("send");
    assert!(!log.exists(), "a temporary conversation wrote a checkpoint");
    let _ = std::fs::remove_dir_all(&log);
}

/// Phase 3's first slice, over the real pipes: the model asks for one of
/// Warden's OWN read-only tools, the command really runs in the thread's
/// repository, its answer reaches the model verbatim, the FULL command
/// reaches the UI, and the execution is audited.
#[tokio::test]
async fn a_local_read_only_tool_runs_audited_and_shows_its_command() {
    let repo = make_repo("local-exec", true);
    let audit = std::env::temp_dir().join(format!(
        "warden-m3-audit-{}-{}.jsonl",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let f = Fixture::with_server_env(
        "REPOTORY_AGENT_FAKE_CHAT",
        &[("REPOTORY_AGENT_FAKE_HANDOFF_LOG", "/dev/null")],
    )
    .await;
    f.create(THREAD, repo.to_str().unwrap()).await;
    // The audit sink is process-wide state attached at setup in the app; in
    // this harness each fixture points it at its own file.
    f.state.audit.attach(audit.clone());

    let mut req = request("m3a", "what changed in my working tree?");
    req.model = Some("tools:warden_git_status".into());
    let (result, chunks) = f.send(req).await;
    result.expect("send");

    // The finish event carries the complete command — Phase 3's display rule.
    let finish = chunks
        .iter()
        .filter_map(|c| c.tool_call.as_ref())
        .find(|e| e.done)
        .expect("a finished tool event");
    assert_eq!(finish.name, "warden_git_status");
    assert!(finish.ok, "{:?}", finish.error);
    assert_eq!(
        finish.command.as_deref(),
        Some("git --no-pager status --short --branch")
    );
    // Phase 3: 所有输出必须可折叠查看. The literal output travels to the UI
    // so the panel can expand exactly what the model read — never a re-run,
    // which could differ.
    let output = finish
        .output
        .as_deref()
        .expect("the output must travel to the UI");
    assert!(output.contains("a.txt"), "{output}");

    // The model read the real answer: the dirty file is named in the tool
    // result it was handed back.
    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result message");
    let content = tool_message["content"].as_str().unwrap();
    assert!(content.contains("a.txt"), "{content}");

    // Every execution is recorded: what ran, where, how it ended.
    let lines = read_jsonl(&audit);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["tool"], "warden_git_status");
    assert_eq!(lines[0]["repository"], repo.to_str().unwrap());
    assert!(lines[0]["argv"].as_str().unwrap().contains("status"));
    assert_eq!(lines[0]["exit"], 0);
    assert_eq!(lines[0]["ok"], true);

    let _ = std::fs::remove_file(&audit);
    let _ = std::fs::remove_dir_all(&repo);
}

/// The local face is offered ONLY when the conversation has a repository:
/// a temporary conversation has no cwd to run anything in, so the tools are
/// not advertised at all.
#[tokio::test]
async fn a_temporary_conversation_is_offered_no_local_tools() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    let mut req = request("m3b", "quick question");
    req.thread_id = None;
    let (result, _chunks) = f.send(req).await;
    result.expect("send");
    // No thread means no tools at all (the no-Cinder contract): assert the
    // stronger property rather than a count.
    let tools = {
        let reply = answer(&_chunks);
        reply["tools"].as_array().cloned().unwrap_or_default()
    };
    assert!(tools.is_empty(), "a temporary conversation offers no tools");
}

/// A write-capable name the model invents is still refused client-side, and
/// now with the local face present too: the local whitelist is a closed
/// set, not a prefix rule.
#[tokio::test]
async fn a_local_tool_outside_the_whitelist_is_refused() {
    let repo = make_repo("local-refuse", false);
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, repo.to_str().unwrap()).await;
    let mut req = request("m3c", "delete everything");
    req.model = Some("tools:warden_run_anything".into());
    let (result, chunks) = f.send(req).await;
    result.expect("send");
    let finish = chunks
        .iter()
        .filter_map(|c| c.tool_call.as_ref())
        .find(|e| e.done)
        .expect("a finished tool event");
    assert!(!finish.ok);
    assert!(finish.command.is_none(), "nothing ran, so no command");
    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages.iter().find(|m| m["role"] == "tool").unwrap();
    assert!(
        tool_message["content"]
            .as_str()
            .unwrap()
            .contains("not available"),
        "{tool_message}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

// ---------------------------------------------------------------- Phase 3b
// The confirmation gate: a write/build command runs ONLY after the user says
// yes to the exact command. Three paths, and the two that matter most are
// the ones where nothing runs.

/// An approved command runs, its output reaches the model, the full command
/// is on both the card and the finish chip, and the run is audited.
#[tokio::test]
async fn an_approved_write_command_runs_and_is_audited() {
    let repo = make_repo("write-approved", true);
    let audit = std::env::temp_dir().join(format!(
        "warden-m3b-audit-{}-{}.jsonl",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, repo.to_str().unwrap()).await;
    f.state.audit.attach(audit.clone());

    let mut req = request("w1", "stage my changes");
    req.model = Some("tools:warden_git_add".into());
    let (result, chunks, cards) = f.send_and_answer(req, true).await;
    result.expect("send");

    // The card showed exactly what ran — same string, no re-derivation.
    assert_eq!(cards.len(), 1, "one card for one command: {cards:?}");
    let card = &cards[0];
    assert_eq!(card["tool"], "warden_git_add");
    let card_command = card["command"].as_str().unwrap();
    assert_eq!(card_command, "git add --all");
    assert!(card["seconds"].as_u64().unwrap() > 0);

    let finish = chunks
        .iter()
        .filter_map(|c| c.tool_call.as_ref())
        .find(|e| e.done)
        .expect("a finished tool event");
    assert!(finish.ok, "{:?}", finish.error);
    assert_eq!(finish.command.as_deref(), Some(card_command));
    assert!(
        finish.output.as_ref().is_some_and(|o| !o.is_empty()),
        "an executed command must hand its output back to the UI"
    );

    // It actually staged: the working tree still shows the change, but the
    // index now has it, which is what `git add` means.
    let staged = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["diff", "--cached", "--name-only"])
        .output()
        .unwrap();
    let staged = String::from_utf8_lossy(&staged.stdout);
    assert!(staged.contains("a.txt"), "nothing was staged: {staged}");

    let lines = read_jsonl(&audit);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["tool"], "warden_git_add");
    assert_eq!(lines[0]["argv"], card_command);

    let _ = std::fs::remove_file(&audit);
    let _ = std::fs::remove_dir_all(&repo);
}

/// A refused command does NOT run — the whole point of the gate — and the
/// model is told so, so it can propose something else instead of assuming
/// success.
#[tokio::test]
async fn a_refused_write_command_does_not_run() {
    let repo = make_repo("write-refused", true);
    let its_head = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;

    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, repo.to_str().unwrap()).await;

    let mut req = request("w2", "commit this for me");
    req.model = Some("tools:warden_git_commit".into());
    let (result, chunks, cards) = f.send_and_answer(req, false).await;
    result.expect("a refusal is not a failed send");

    let events: Vec<(&str, bool)> = chunks
        .iter()
        .filter_map(|c| c.tool_call.as_ref())
        .map(|e| (e.name.as_str(), e.ok))
        .collect();
    assert_eq!(cards.len(), 1, "cards: {cards:?} events: {events:?}");
    assert_eq!(cards[0]["tool"], "warden_git_commit");

    // Nothing was committed: HEAD is byte-identical.
    let after = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(its_head, after, "a refused command must not run");

    let finish = chunks
        .iter()
        .filter_map(|c| c.tool_call.as_ref())
        .find(|e| e.done)
        .expect("a finished tool event");
    assert!(!finish.ok);
    // Nothing ran, so there is nothing to expand: the chip shows the
    // refusal, and the panel must not appear offering an empty body.
    assert!(finish.output.is_none(), "a refusal has no output to expand");
    // The card's command still travels to the UI: the user should see what
    // they declined, not a blank chip.
    // The command shown is the argv joined with spaces — no shell quoting,
    // because no shell runs. The message is ONE argument.
    assert_eq!(
        finish.command.as_deref(),
        Some("git commit -m commit this for me")
    );

    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages.iter().find(|m| m["role"] == "tool").unwrap();
    let content = tool_message["content"].as_str().unwrap();
    assert!(content.contains("refused"), "{content}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// The card is not a yes-by-default: with nobody answering, the wait ends
/// and the command is refused. This exercises the same code path the real
/// timeout takes, with a short card so the test stays fast.
#[tokio::test]
async fn an_unanswered_card_refuses_the_command() {
    let repo = make_repo("write-unanswered", true);
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    f.create(THREAD, repo.to_str().unwrap()).await;

    // Nobody calls decide: collect the card and let the turn end. The
    // timeout is shortened here so the test drives the same path the
    // product takes after its real 180 seconds.
    f.state
        .set_approval_timeout(std::time::Duration::from_millis(150));
    let mut req = request("w3", "build it");
    req.model = Some("tools:warden_npm_build".into());
    let (result, chunks) = f.send(req).await;
    // The card never gets an answer, so the send ends with a refusal the
    // model can read — not an error and not a hang.
    result.expect("send");

    let finish = chunks
        .iter()
        .filter_map(|c| c.tool_call.as_ref())
        .find(|e| e.done)
        .expect("a finished tool event");
    assert!(!finish.ok);
    assert_eq!(finish.command.as_deref(), Some("npm run build"));
    let reply = answer(&chunks);
    let messages = reply["messages"].as_array().unwrap();
    let tool_message = messages.iter().find(|m| m["role"] == "tool").unwrap();
    assert!(
        tool_message["content"]
            .as_str()
            .unwrap()
            .contains("nothing was run"),
        "{tool_message}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

// ------------------------------------------------------------------ Phase 4
// The memory panels. Three independent reads, three independent failure
// reports — a dead memory layer must not blank the local panel.

/// All three panels come back from real MCP calls, with the local one
/// describing the actual working tree.
#[tokio::test]
async fn the_memory_panels_read_all_three_sources() {
    let repo = make_repo("panels-ok", true);
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;

    let panels = repotory_agent_lib::panels::load(&f.session, Some(repo.to_str().unwrap())).await;

    // Context: the local tree, not a Cinder fact.
    assert_eq!(
        panels.context.repository.as_deref(),
        Some(repo.to_str().unwrap())
    );
    assert_eq!(panels.context.branch.as_deref(), Some("main"));
    assert!(
        panels
            .context
            .diff_stat
            .as_deref()
            .is_some_and(|d| d.contains("a.txt")),
        "the panel must show the real uncommitted diff: {:?}",
        panels.context.diff_stat
    );
    assert!(panels.context.problem.is_none());

    // Knowledge: the approved browse feed, with the global page marked as
    // global (project_id 0) rather than as missing.
    assert!(panels.knowledge.connected, "{:?}", panels.knowledge.problem);
    assert_eq!(panels.knowledge.pages.len(), 2);
    assert_eq!(panels.knowledge.pages[0].title, "The Knot");
    assert_eq!(panels.knowledge.pages[0].project_id, 0);

    // Memory: Cinder's own briefing, which leads with the last handoff.
    assert!(panels.memory.connected, "{:?}", panels.memory.problem);
    assert!(
        panels
            .memory
            .briefing
            .as_deref()
            .is_some_and(|b| b.contains("Project: warden")),
        "{:?}",
        panels.memory.briefing
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// A conversation with no repository still gets the memory panel: the
/// question "what did the last session leave" is not repository-only.
#[tokio::test]
async fn panels_without_a_repository_still_read_cinder() {
    let f = Fixture::new("REPOTORY_AGENT_FAKE_CHAT").await;
    let panels = repotory_agent_lib::panels::load(&f.session, None).await;
    assert!(panels.context.repository.is_none());
    assert!(
        panels.context.problem.is_some(),
        "the panel must say why it is empty"
    );
    assert!(panels.knowledge.connected);
    assert!(panels.memory.connected);
}

/// A contract change is NOT reported as "nothing approved": a healthy server
/// that answered a different shape must read as a problem, because telling
/// the user they have no knowledge pages when they do is the worse failure.
#[tokio::test]
async fn an_unexpected_knowledge_shape_is_a_problem_not_an_empty_list() {
    let f = Fixture::with_server_env(
        "REPOTORY_AGENT_FAKE_CHAT",
        &[("REPOTORY_AGENT_FAKE_WIKI_BROKEN", "1")],
    )
    .await;
    let panels = repotory_agent_lib::panels::load(&f.session, None).await;
    assert!(panels.knowledge.pages.is_empty());
    assert!(
        panels.knowledge.problem.is_some(),
        "an unexpected shape must be reported, not swallowed"
    );
    // `connected` stays true: the server DID answer, so "install Cinder"
    // would be the wrong advice.
    assert!(panels.knowledge.connected);
}
