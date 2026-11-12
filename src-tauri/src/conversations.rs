//! Shared conversations are Cinder MCP data, never a second local database.
//! Only the originating Warden window receives token deltas. Other clients
//! poll `read` for completed messages; there is no cross-IDE token stream.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::llm::{
    ChatMessage, LlmClient, ToolCallFunction, ToolCallOut, ToolFunctionSpec, ToolSpec,
};
use crate::mcp_client::McpSession;
pub use crate::provider_commands::{ChatChunk, ToolEvent};
use crate::providers::ProviderStore;

// Capability bindings, not a substitute for the server's tools/list.
const CHAT_TOOL: &str = "repotory_chat";
// Warden's OWN write binding — deliberately outside MODEL_TOOLS. The reply
// path writes this checkpoint itself, so the model can never drive it (the
// double-write rule that keeps repotory_chat off the model face applies the
// same way here). handoff is in the auto-write tier of product.md open
// question 5: a session record, not durable knowledge, so no confirmation
// gate stands between the reply and this write.
const HANDOFF_TOOL: &str = "repotory_handoff";
const MAX_INPUT_CHARS: usize = 12_000;
const CONTEXT_CHARS: usize = 24_000;
const CONTEXT_MESSAGES: usize = 32;
// Must equal Cinder's `service.MaxChatTitleLen`. A local cap looser than the
// server's is worse than none: the title looks valid here and then fails there
// with a message the user cannot act on.
const MAX_TITLE_CHARS: usize = 160;

// The tools the model may call: the memory layer's read-only face, filtered
// from the server's own tools/list at call time. A whitelist is a capability
// GRANT, not a contract copy — it names what Warden is willing to let the
// model drive, while the schemas still come verbatim from the server. Writes
// (notes_create/update, chat) are excluded on purpose: the reply path appends
// the assistant message itself, so a model that could append would double-
// write the very conversation it is answering; handoff and audit are the
// memory layer's governance, not the copilot's. A write-capable face comes
// later, behind a confirmation step.
pub(crate) const MODEL_TOOLS: &[&str] = &[
    "repotory_metrics",
    "repotory_context",
    "repotory_projects_list",
    "repotory_projects_stats",
    "repotory_notes_search",
    "repotory_notes_read",
    "repotory_wiki_search",
    "repotory_wiki_read",
];

/// How many model→tool→model rounds one reply may spend before the next call
/// is forced text-only. The same bound the Cinder desktop loop applies.
const MAX_TOOL_ROUNDS: usize = 5;

/// The model-facing tool set: the server's own tools, filtered to the
/// whitelist. Schemas pass through verbatim — a schema copied here would
/// drift from the server's contract.
async fn model_tools(session: &McpSession) -> Result<Vec<ToolSpec>, String> {
    let (_, tools) = session
        .ensure()
        .await
        .map_err(|e| format!("Cinder MCP unavailable: {e}"))?;
    Ok(tools
        .iter()
        .filter(|tool| MODEL_TOOLS.contains(&tool.name.as_str()))
        .map(|tool| ToolSpec {
            kind: "function".to_string(),
            function: ToolFunctionSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            },
        })
        .collect())
}

/// The system prompt for a tool-enabled conversation. Short on purpose: the
/// context budget is small and the tools describe themselves.
///
/// `watched` is Warden's own watchlist — the projects the user adopted from
/// the copilot's disk scan. It is a DIFFERENT list from Cinder's starred
/// projects (`repotory_projects_list`), and the two answer different
/// questions: the watchlist is "what the user is working on now", the starred
/// set is "what the knowledge base has governed". Without both names in the
/// prompt the model reads whichever tool it happens to call as THE answer to
/// "which repositories do I watch", and the user is told their own list back
/// wrong. Naming both, and saying which is which, costs one line and removes
/// the confusion.
fn system_prompt(repository: Option<&str>, watched: &[String], diff: Option<&str>) -> String {
    let mut prompt = String::from(
        "You are Warden, a local copilot. Cinder, the memory layer, offers \
         read-only tools: use them for repository facts (metrics, notes, \
         wiki, project stats) instead of guessing. Report what a tool \
         returned; when it returns nothing, say so — never invent numbers. \
         Answer in the user's language.",
    );
    if let Some(path) = repository.filter(|path| !path.is_empty()) {
        prompt.push_str(&format!("\nCurrent repository: {path}"));
    }
    // The working tree the Phase 2 line promised: "附加最近 git diff". The
    // block is the client's own rendering of `git diff --stat` plus an
    // excerpt, and it names its own truncation — the model must know the
    // patch it sees may be a head, not the whole. When there is nothing
    // uncommitted there is nothing to say, and the absence is silent by
    // design (git.rs).
    if let Some(diff) = diff.filter(|diff| !diff.trim().is_empty()) {
        prompt.push_str("\n\n");
        prompt.push_str(diff.trim_end());
    }
    if !watched.is_empty() {
        let shown: Vec<&str> = watched.iter().map(String::as_str).take(20).collect();
        prompt.push_str(&format!(
            "\nThe user's Warden watchlist (their adopted projects from this \
             machine's disk scan — Warden's own list, NOT the same as \
             Cinder's starred projects from repotory_projects_list): {}. \
             When asked which projects the user watches, answer from the \
             watchlist; when citing Cinder's list, say it is the memory \
             layer's.",
            shown.join(", ")
        ));
    }
    prompt
}

/// Runs one `repotory_chat` operation and parses its answer.
///
/// The chat path is the one place a JSON object IS the contract: every
/// operation's answer is read by field (`threads`, `thread.id`, `messages`),
/// so anything else is a broken server and must fail loudly.
async fn call(session: &McpSession, name: &str, arguments: Value) -> Result<Value, String> {
    let text = call_text(session, name, arguments).await?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("Cinder {name} returned invalid JSON: {e}"))?;
    // An object OR an array — never a bare scalar. The shape is the server's
    // business, not ours: `repotory_projects_list` answers with a JSON array,
    // and its own comment says an empty list must serialise as `[]`, not
    // `null`. Requiring an object here rejected that contract outright, and
    // the model then read a client-side error as a broken Cinder. A scalar is
    // still refused: neither the chat path (which reads `threads`) nor the
    // model (which reads records) can do anything with `"ok"`.
    if !value.is_object() && !value.is_array() {
        return Err(format!("Cinder {name} must return a JSON object or array"));
    }
    Ok(value)
}

/// Calls one model-facing tool and hands the answer to the model as text.
///
/// The server's tool face is NOT one JSON contract — it is per-tool, and
/// inventing a client-side shape for it is how "healthy Cinder" turns into
/// a string of client errors the model can only misread:
///
///   * `repotory_context` answers with a PLAIN-TEXT briefing (its handler
///     returns `BuildProjectContext(res) string`), and argument-validation
///     refusals on several tools are plain sentences too;
///   * `repotory_projects_list` answers with a JSON ARRAY;
///   * the rest answer with JSON objects.
///
/// So the text travels to the model verbatim — it reads prose and JSON
/// alike, and it is the one consumer that can. Parsing here would either
/// reject the plain-text contract (this bug, twice) or rewrite the server's
/// answer, which the thin-client rule already forbids for tool lists and
/// versions.
async fn call_model_tool(
    session: &McpSession,
    name: &str,
    arguments: Value,
) -> Result<String, String> {
    call_text(session, name, arguments).await
}

/// The transport both paths share: ensure, advertise-check, call, evict.
async fn call_text(
    session: &McpSession,
    name: &str,
    mut arguments: Value,
) -> Result<String, String> {
    // Optional MCP strings must be omitted, not sent as JSON null: an older
    // server may validate its input schema before invoking the handler.
    if let Some(fields) = arguments.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    let (client, tools) = session
        .ensure()
        .await
        .map_err(|e| format!("Cinder MCP unavailable: {e}"))?;
    if !tools.iter().any(|tool| tool.name == name) {
        return Err(format!(
            "Cinder MCP does not provide {name}; install or upgrade Cinder to use this capability"
        ));
    }
    let answer = client.call_tool(name, arguments).await;
    session.evict_if_closed(&answer).await;
    answer.map_err(|e| format!("Cinder {name}: {e}"))
}

fn nonempty(value: &str, name: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(format!("{name} must contain 1–{max} characters"));
    }
    Ok(())
}

pub async fn list(session: &McpSession, path: Option<String>) -> Result<Value, String> {
    let value = call(session, CHAT_TOOL, json!({"operation":"list", "path":path})).await?;
    if !value["threads"].is_array() {
        return Err("Cinder chat list is missing threads".into());
    }
    Ok(value)
}

pub async fn create(
    session: &McpSession,
    id: String,
    title: String,
    path: Option<String>,
) -> Result<Value, String> {
    let valid_uuid = id.len() == 36
        && id.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    if !valid_uuid {
        return Err("conversation id must be a UUID supplied by the caller".into());
    }
    nonempty(&title, "title", MAX_TITLE_CHARS)?;
    // `path` is optional in the current contract (absent or "" = unscoped), but
    // an older `repotory-mcp` on the user's machine still required the key.
    // Sending "" is understood by both versions, so an unscoped conversation
    // works against either — the same reason the two IDE clients do it.
    let value = call(
        session,
        CHAT_TOOL,
        json!({"operation":"create", "id":id, "title":title, "path": path.unwrap_or_default()}),
    )
    .await?;
    check_thread(&value, &id)?;
    Ok(value)
}

fn check_thread(value: &Value, id: &str) -> Result<(), String> {
    if value["thread"]["id"].as_str() != Some(id) {
        return Err("Cinder returned a different or missing conversation".into());
    }
    Ok(())
}

pub async fn read(session: &McpSession, thread_id: String) -> Result<Value, String> {
    nonempty(&thread_id, "threadId", 200)?;
    let mut value = call(
        session,
        CHAT_TOOL,
        json!({"operation":"read", "thread_id":thread_id}),
    )
    .await?;
    check_thread(&value, &thread_id)?;
    let messages = value["messages"]
        .as_array_mut()
        .ok_or("Cinder chat read is missing messages")?;
    if messages.len() > 500 {
        messages.drain(..messages.len() - 500);
    }
    Ok(value)
}

pub async fn rename(
    session: &McpSession,
    thread_id: String,
    title: String,
) -> Result<Value, String> {
    nonempty(&thread_id, "threadId", 200)?;
    nonempty(&title, "title", MAX_TITLE_CHARS)?;
    let value = call(
        session,
        CHAT_TOOL,
        json!({"operation":"rename", "thread_id":thread_id, "title":title}),
    )
    .await?;
    check_thread(&value, &thread_id)?;
    Ok(value)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    id: String,
    role: String,
    content: String,
    #[serde(default)]
    source: String,
    reply_to: Option<String>,
}

async fn append(
    session: &McpSession,
    thread: &str,
    id: &str,
    role: &str,
    content: &str,
    reply_to: Option<&str>,
) -> Result<Message, String> {
    let value = call(
        session,
        CHAT_TOOL,
        json!({"operation":"append", "thread_id":thread,
        "id":id, "role":role, "content":content, "source":"warden", "reply_to":reply_to}),
    )
    .await?;
    let message: Message = serde_json::from_value(value["message"].clone())
        .map_err(|e| format!("invalid Cinder message: {e}"))?;
    if message.id != id
        || message.role != role
        || message.content != content
        || message.source != "warden"
        || message.reply_to.as_deref() != reply_to
    {
        return Err("messageId already belongs to a different immutable message".into());
    }
    Ok(message)
}

/// A rolling, character-bounded context, ending at this user turn. Never
/// accept a server-injected system role or messages from a different thread.
fn context(messages: &[Message], message_id: &str) -> Result<Vec<ChatMessage>, String> {
    let end = messages.iter().position(|m| m.id == message_id && m.role == "user")
        .ok_or("the user message is outside Cinder's recent history; refusing to generate without its context")?;
    let mut remaining = CONTEXT_CHARS;
    let mut result = Vec::new();
    for message in messages[..=end].iter().rev().take(CONTEXT_MESSAGES) {
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err("Cinder chat history contains an unsupported role".into());
        }
        let chars = message.content.chars().count();
        if chars > remaining {
            break;
        }
        remaining -= chars;
        result.push(ChatMessage {
            role: message.role.clone(),
            content: message.content.clone(),
            tool_call_id: None,
            tool_calls: None,
        });
    }
    result.reverse();
    if result.is_empty() {
        return Err("the user message exceeds the context budget".into());
    }
    Ok(result)
}

#[derive(Clone)]
pub struct ChatRequest {
    pub message: String,
    pub model: Option<String>,
    pub provider_id: Option<String>,
    pub thread_id: Option<String>,
    pub message_id: String,
    /// Warden's own watchlist — the adopted project paths from the copilot's
    /// disk scan. Read-only here: the command layer passes what RepoStore
    /// holds at the moment the request starts.
    pub watched: Vec<String>,
}

#[derive(Clone)]
struct Attempt {
    message: String,
    // A successful generation is kept until MCP persistence succeeds, so a
    // transient append failure can be retried without another paid request.
    outcome: Result<String, String>,
}

/// Process-local concurrency/idempotency only, NOT conversation storage.
/// No-Cinder requests remain single-turn and have no durable local history.
#[derive(Default)]
pub struct GenerationState {
    busy: Mutex<HashSet<String>>,
    attempts: Mutex<HashMap<(Option<String>, String), Attempt>>,
}

struct GenerationGuard<'a> {
    state: &'a GenerationState,
    key: String,
}
impl Drop for GenerationGuard<'_> {
    fn drop(&mut self) {
        self.state
            .busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}
impl GenerationState {
    fn acquire(&self, request: &ChatRequest) -> Result<GenerationGuard<'_>, String> {
        let key = match &request.thread_id {
            Some(id) => format!("thread:{id}"),
            None => format!("single:{}", request.message_id),
        };
        if !self
            .busy
            .lock()
            .map_err(|_| "chat generation state is unavailable")?
            .insert(key.clone())
        {
            return Err(
                "this conversation is already generating a reply; wait for it to finish".into(),
            );
        }
        Ok(GenerationGuard { state: self, key })
    }

    fn forget(&self, key: (Option<String>, String)) {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.remove(&key);
        }
    }
}

fn prepare(store: &ProviderStore, request: &ChatRequest) -> Result<(LlmClient, String), String> {
    let (config, problem) = store.load();
    if let Some(problem) = problem {
        return Err(problem);
    }
    let provider = match request.provider_id.as_deref() {
        Some(id) => config
            .find(id)
            .ok_or_else(|| format!("no provider named {id}"))?,
        None => config.active().ok_or("no provider is configured yet")?,
    };
    let model = request
        .model
        .as_ref()
        .or(provider.default_model.as_ref())
        .ok_or("this provider has no default model — pick one first")?
        .trim()
        .to_string();
    nonempty(&model, "model", 500)?;
    // This function runs on the blocking pool: both config and keys do IO.
    let key = if provider.has_api_key {
        Some(store.load_api_key(&provider.id)?.ok_or(
            "the provider's API key is missing — open the provider settings and save it again",
        )?)
    } else {
        None
    };
    let client = LlmClient::new(provider, key).map_err(|e| e.to_string())?;
    Ok((client, model))
}

/// The IPC command and integration tests share this exact path. All failures,
/// including validation and persistence, end the Channel with an error.
pub async fn send(
    session: &McpSession,
    state: &GenerationState,
    store: &ProviderStore,
    request: ChatRequest,
    emit: impl FnMut(ChatChunk),
) -> Result<(), String> {
    send_with_cancel(
        session,
        state,
        store,
        request,
        emit,
        Arc::new(AtomicBool::new(false)),
    )
    .await
}

/// The IPC command uses this variant so a disconnected webview stops the
/// generation: when `cancel` is set the stream loop breaks, no answer is
/// persisted, and the error travels back through the final ChatChunk.
pub async fn send_with_cancel(
    session: &McpSession,
    state: &GenerationState,
    store: &ProviderStore,
    request: ChatRequest,
    mut emit: impl FnMut(ChatChunk),
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut text = String::new();
    let outcome = send_inner(
        session, state, store, &request, &mut text, &mut emit, &cancel,
    )
    .await;
    emit(ChatChunk {
        delta: String::new(),
        text,
        done: true,
        error: outcome.as_ref().err().cloned(),
        tool_call: None,
    });
    outcome
}

async fn send_inner(
    session: &McpSession,
    state: &GenerationState,
    store: &ProviderStore,
    request: &ChatRequest,
    text: &mut String,
    emit: &mut impl FnMut(ChatChunk),
    cancel: &AtomicBool,
) -> Result<(), String> {
    nonempty(&request.message, "message", MAX_INPUT_CHARS)?;
    nonempty(&request.message_id, "messageId", 190)?;
    if let Some(id) = &request.thread_id {
        nonempty(id, "threadId", 200)?;
    }
    let _guard = state.acquire(request)?;
    let key = (request.thread_id.clone(), request.message_id.clone());
    let cached = state
        .attempts
        .lock()
        .map_err(|_| "chat generation state is unavailable")?
        .get(&key)
        .cloned();
    if cached
        .as_ref()
        .is_some_and(|attempt| attempt.message != request.message)
    {
        return Err("messageId already belongs to a different immutable message".into());
    }

    // Validate configuration before writing a new user turn. A completed reply
    // can still be replayed after its provider was removed.
    if let Some(thread) = &request.thread_id {
        let value = read(session, thread.clone()).await?;
        let messages: Vec<Message> = serde_json::from_value(value["messages"].clone())
            .map_err(|e| format!("invalid Cinder history: {e}"))?;
        if let Some(user) = messages.iter().find(|m| m.id == request.message_id)
            && (user.role != "user"
                || user.content != request.message
                || user.source != "warden"
                || user.reply_to.is_some())
        {
            return Err("messageId already belongs to a different immutable message".into());
        }
        if let Some(reply) = messages
            .iter()
            .find(|m| m.id == format!("{}-reply", request.message_id))
        {
            if reply.role != "assistant" || reply.reply_to.as_deref() != Some(&request.message_id) {
                return Err("the immutable reply id belongs to a different message".into());
            }
            // The oldest user may have fallen out of the 500-message window
            // while its reply is still present. Let Cinder verify immutable
            // identity rather than replaying an answer for changed input.
            if !messages.iter().any(|m| m.id == request.message_id) {
                append(
                    session,
                    thread,
                    &request.message_id,
                    "user",
                    &request.message,
                    None,
                )
                .await?;
            }
            *text = reply.content.clone();
            return Ok(());
        }
    }
    // Set when a tool-enabled thread had a repository during GENERATION:
    // (path, the diff snapshot the prompt was built from). The reply
    // checkpoint reuses it, and a replay leaves it None — a finished answer
    // has already earned its checkpoint.
    let mut checkpoint: Option<(String, Option<String>)> = None;
    if let Some(attempt) = cached {
        *text = attempt.outcome?;
    } else {
        let store = store.clone();
        let input = request.clone();
        let (client, model) = tauri::async_runtime::spawn_blocking(move || prepare(&store, &input))
            .await
            .map_err(|e| format!("provider configuration task failed: {e}"))??;
        let (mut messages, tools) = if let Some(thread) = &request.thread_id {
            append(
                session,
                thread,
                &request.message_id,
                "user",
                &request.message,
                None,
            )
            .await?;
            // Read AFTER append: these are the server's actual ordered messages,
            // not an optimistic local transcript or another topic's history.
            let value = read(session, thread.clone()).await?;
            let history: Vec<Message> = serde_json::from_value(value["messages"].clone())
                .map_err(|e| format!("invalid Cinder history: {e}"))?;
            let mut context = context(&history, &request.message_id)?;
            // Function Calling is a Cinder-thread capability: the tools are
            // the memory layer's, so a temporary conversation has none, and
            // the system prompt exists only when tools do — the no-Cinder
            // contract (no server-injected anything) stays intact. The
            // thread's path is the conversation's repository scope.
            let tools = model_tools(session).await?;
            if !tools.is_empty() {
                let repository = value["thread"]["path"]
                    .as_str()
                    .filter(|path| !path.trim().is_empty())
                    .map(|path| path.to_string());
                // One snapshot per generation, shared by the injected prompt
                // and the reply checkpoint: what the model read and what the
                // handoff recorded describe the SAME tree, not whatever the
                // user saved between the two calls. The git child runs on the
                // blocking pool — this path is an async command and a wedged
                // git must not occupy a tokio worker (AGENTS: no blocking IO
                // in async commands).
                let diff = match &repository {
                    Some(path) => {
                        let path = path.clone();
                        tauri::async_runtime::spawn_blocking(move || crate::git::recent_diff(&path))
                            .await
                            .map_err(|e| format!("the git diff task failed: {e}"))?
                    }
                    None => None,
                };
                context.insert(
                    0,
                    ChatMessage::system(system_prompt(
                        repository.as_deref(),
                        &request.watched,
                        diff.as_deref(),
                    )),
                );
                checkpoint = repository.map(|path| (path, diff));
            }
            (context, tools)
        } else {
            (vec![ChatMessage::user(&request.message)], Vec::new())
        };
        // Record the attempt before the first network await. If its task is
        // interrupted, replay cannot silently issue a second paid request.
        state
            .attempts
            .lock()
            .map_err(|_| "chat generation state is unavailable")?
            .insert(
                key.clone(),
                Attempt {
                    message: request.message.clone(),
                    outcome: Err("generation was interrupted; use a new messageId to retry".into()),
                },
            );
        // The generation loop: model → (tool calls → execute → feed back)×,
        // each round streamed. The final round's text is the answer — a tool
        // round's preamble restarts the visible text via the tool event's
        // empty `text`.
        let mut round: usize = 0;
        let outcome = loop {
            round += 1;
            // A disconnected webview must not start (or continue) another
            // paid round. The error is surfaced through the final ChatChunk.
            if cancel.load(Ordering::Relaxed) {
                break Err("the chat window disconnected; generation was stopped".to_string());
            }
            // The round cap: once spent, the next call is text-only, so the
            // answer arrives in words instead of the cap silently turning
            // into an empty reply.
            let round_tools: &[ToolSpec] = if round > MAX_TOOL_ROUNDS { &[] } else { &tools };
            let streamed = match client
                .chat_stream(&model, &messages, round_tools, |delta| {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    text.push_str(delta);
                    emit(ChatChunk {
                        delta: delta.to_string(),
                        text: text.clone(),
                        done: false,
                        error: None,
                        tool_call: None,
                    });
                })
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => break Err(error.to_string()),
            };
            // The stream may have been cancelled mid-answer: stop before the
            // outcome is recorded, and never persist it.
            if cancel.load(Ordering::Relaxed) {
                break Err("the chat window disconnected; generation was stopped".to_string());
            }
            if streamed.tool_calls.is_empty() {
                break if streamed.content.trim().is_empty() {
                    Err("the provider returned no answer".to_string())
                } else {
                    Ok(streamed.content)
                };
            }
            if tools.is_empty() {
                break Err("the model requested tools but none were offered".to_string());
            }
            if round > MAX_TOOL_ROUNDS {
                break Err("the model kept requesting tools after the final round".to_string());
            }
            // The preamble was not the answer: the visible text restarts, and
            // the UI learns what is being called.
            text.clear();
            for call in &streamed.tool_calls {
                emit(ChatChunk {
                    delta: String::new(),
                    text: String::new(),
                    done: false,
                    error: None,
                    tool_call: Some(ToolEvent {
                        name: call.name.clone(),
                        done: false,
                        ok: false,
                        error: None,
                    }),
                });
            }
            // Echo the assistant turn WITH its calls — the protocol pairs every
            // tool result with the preceding assistant tool_calls — then
            // execute each. A model that names a tool outside the whitelist
            // gets a refusal as its result, never an execution.
            let echo: Vec<ToolCallOut> = streamed
                .tool_calls
                .iter()
                .map(|call| ToolCallOut {
                    id: call.id.clone(),
                    kind: "function".to_string(),
                    function: ToolCallFunction {
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    },
                })
                .collect();
            messages.push(ChatMessage::assistant_with_tool_calls(
                streamed.content.clone(),
                echo,
            ));
            for tool_call in &streamed.tool_calls {
                let allowed = MODEL_TOOLS.contains(&tool_call.name.as_str());
                let arguments = if tool_call.arguments.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&tool_call.arguments).unwrap_or_else(|_| json!({}))
                };
                // The done event reports the real outcome: the UI reads `ok`
                // and `error` to show success or a refusal/call failure.
                let (content, ok, error) = if !allowed {
                    let message = format!("tool {} is not available to the model", tool_call.name);
                    (
                        json!({"error": message.clone()}).to_string(),
                        false,
                        Some(message),
                    )
                } else {
                    // The server's answer travels to the model verbatim —
                    // prose from repotory_context, JSON from the rest. The
                    // model reads both; a client-side parse would reject the
                    // prose contract outright.
                    match call_model_tool(session, &tool_call.name, arguments).await {
                        Ok(text) => (text, true, None),
                        Err(error) => (
                            json!({"error": error.clone()}).to_string(),
                            false,
                            Some(error),
                        ),
                    }
                };
                messages.push(ChatMessage::tool_result(tool_call.id.clone(), content));
                emit(ChatChunk {
                    delta: String::new(),
                    text: String::new(),
                    done: false,
                    error: None,
                    tool_call: Some(ToolEvent {
                        name: tool_call.name.clone(),
                        done: true,
                        ok,
                        error,
                    }),
                });
            }
        };
        let outcome = outcome.and_then(|answer| {
            if answer.trim().is_empty() {
                Err("the provider returned no answer".into())
            } else {
                Ok(answer)
            }
        });
        state
            .attempts
            .lock()
            .map_err(|_| "chat generation state is unavailable")?
            .insert(
                key.clone(),
                Attempt {
                    message: request.message.clone(),
                    outcome: outcome.clone(),
                },
            );
        *text = outcome?;
    }
    if let Some(thread) = &request.thread_id {
        append(session, thread, &format!("{}-reply", request.message_id), "assistant", text, Some(&request.message_id)).await
            .map_err(|e| format!("the answer was generated but could not be saved to Cinder; resend the same messageId to retry saving: {e}"))?;
        // The recorded attempt exists so a failed persist can be replayed
        // without a second paid request. Once the answer is in Cinder, the
        // durable store has taken over: a replay reads it back instead of
        // this map. The key is a fresh client UUID no later request reuses, so
        // keeping it would grow the map for the life of the process.
        // Without a thread there is no store to take over, so the entry stays —
        // that mode's whole dedup contract is "within this process".
        state.forget(key);
        // M2's last loop: "模型回答后写入 repotory_handoff". Only after the
        // answer is durable — a checkpoint about a reply that failed to save
        // would record a conversation the next session cannot read back.
        if let Some((repository, diff)) = checkpoint
            && let Err(error) = write_handoff(
                session,
                &repository,
                &request.message,
                text,
                diff.as_deref(),
            )
            .await
        {
            // The reply is saved; the checkpoint is the loss. It must not
            // turn into a failed send, and it must not vanish quietly
            // either: stderr is where this app's degraded paths speak.
            eprintln!("[handoff] the session checkpoint could not be written: {error}");
        }
    }
    Ok(())
}

/// One UTF-8-safe excerpt, in BYTES, for a bounded field. Cinder's handoff
/// caps are byte caps (summary 4000, item 1000) — a character count would
/// let CJK text collect a server refusal this client could have prevented.
fn excerpt(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut cut = max_bytes;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_string()
}

/// Writes the structured session record Cinder's `repotory_context` leads a
/// future session with (ADR-0054 rolling checkpoint).
///
/// The shape follows the tool's own contract: `summary` is required, and at
/// least one section must be non-empty. `milestone=true` because this is a
/// checkpoint per work unit, not a session-end summary — and because the
/// server dedupes a re-sent identical milestone onto the same note, so
/// retrying this call is idempotent. `path` attributes the record to the
/// repository the conversation is scoped to; the server resolves the project
/// and refuses cleanly if the directory was never scanned — refusal lands
/// in the stderr log, it is not a user-facing failure.
///
/// Content rules: the exchange (question + answer, bounded) and the unfinished
/// working tree from the SAME diff snapshot the prompt was built from. The
/// diff is what `next_steps` means under the milestone contract — "currently
/// unfinished" — and it is what the next session standing in this repository
/// wants to lead with.
async fn write_handoff(
    session: &McpSession,
    repository: &str,
    question: &str,
    answer: &str,
    diff: Option<&str>,
) -> Result<(), String> {
    let summary = format!(
        "Warden answered in {repository} — Q: {} A: {}",
        excerpt(question, 300),
        excerpt(answer, 2_500)
    );
    let next_steps = match diff.filter(|diff| !diff.trim().is_empty()) {
        Some(diff) => vec![excerpt(diff, 950)],
        None => vec!["No uncommitted changes at checkpoint time.".to_string()],
    };
    // Handoff section caps are the server's: 20 items / 1000 bytes each; the
    // summary cap is 4000 bytes. This client stays inside them instead of
    // collecting a refusal — a truncated checkpoint is still a checkpoint.
    call_text(
        session,
        HANDOFF_TOOL,
        json!({
            "agent": "warden",
            "path": repository,
            "milestone": true,
            "summary": excerpt(&summary, 1_200),
            "next_steps": next_steps,
        }),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn msg(id: &str, role: &str, content: &str) -> Message {
        Message {
            id: id.into(),
            role: role.into(),
            content: content.into(),
            source: "warden".into(),
            reply_to: None,
        }
    }
    #[test]
    fn context_is_multiturn_ordered_and_stops_at_the_requested_turn() {
        let messages = vec![
            msg("1", "user", "first"),
            msg("2", "assistant", "answer"),
            msg("3", "user", "followup"),
            msg("4", "user", "future"),
        ];
        let got = context(&messages, "3").unwrap();
        assert_eq!(
            got.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
            ["first", "answer", "followup"]
        );
        assert_eq!(got[1].role, "assistant");
        assert!(context(&messages, "missing").is_err());
    }
    #[test]
    fn context_is_bounded_in_unicode_characters_and_message_count() {
        let messages = vec![
            msg("1", "user", &"界".repeat(CONTEXT_CHARS)),
            msg("2", "user", "next"),
        ];
        assert_eq!(context(&messages, "2").unwrap().len(), 1);
        let messages: Vec<_> = (0..100)
            .map(|i| msg(&i.to_string(), "user", "hi"))
            .collect();
        assert_eq!(context(&messages, "99").unwrap().len(), CONTEXT_MESSAGES);
    }
    #[test]
    fn same_thread_is_locked_and_other_threads_are_independent() {
        let state = GenerationState::default();
        let mut request = ChatRequest {
            message: "hi".into(),
            message_id: "m".into(),
            thread_id: Some("a".into()),
            model: None,
            provider_id: None,
            watched: Vec::new(),
        };
        let guard = state.acquire(&request).unwrap();
        assert!(state.acquire(&request).is_err());
        request.thread_id = Some("b".into());
        assert!(state.acquire(&request).is_ok());
        drop(guard);
        request.thread_id = Some("a".into());
        assert!(state.acquire(&request).is_ok());
    }
}
