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

/// The model-facing tool set: the server's own tools filtered to the
/// whitelist, plus Warden's OWN read-only local face (exec.rs) when the
/// conversation has a repository to run inside. The two whitelists stay
/// separate on purpose: `repotory_*` is the memory layer's surface (schemas
/// verbatim from the server), `warden_*` is this product's own, and
/// confusing them would be confusing a contract copy with a contract.
async fn model_tools(session: &McpSession, local: bool) -> Result<Vec<ToolSpec>, String> {
    let (_, tools) = session
        .ensure()
        .await
        .map_err(|e| format!("Cinder MCP unavailable: {e}"))?;
    let mut specs: Vec<ToolSpec> = tools
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
        .collect();
    if local {
        specs.extend(crate::exec::tool_specs());
        // The write/build set rides with the read-only face because both
        // need a repository to run in — but the loop gates every call to
        // this half behind the user's yes, so offering them costs nothing.
        specs.extend(crate::exec::write_specs());
    }
    Ok(specs)
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

/// The repository scope a generation ran in, and the diff snapshot its
/// prompt was built from. Kept together because the reply checkpoint must
/// describe the SAME tree the model read.
struct GenerationScope {
    repository: String,
    diff: Option<String>,
}

/// Routes one tool call the model asked for, in this order:
///
/// 1. a `warden_*` name in the local face: executed only when the
///    generation HAS a repository scope — no scope, no answer that could
///    have come from anywhere, and the refusal says so;
/// 2. a name in the MCP read-only whitelist: executed on the memory
///    layer, answer verbatim;
/// 3. anything else: a client-side refusal as the tool result — the model
///    is told, nothing runs, the audit stays clean.
///
/// Returns (content handed to the model, ok, error, full command shown to
/// the UI). The local branch runs blocking children on the blocking pool:
/// this path is an async command and a wedged git must not occupy a tokio
/// worker.
async fn route_tool(
    session: &McpSession,
    state: &GenerationState,
    name: &str,
    arguments: Value,
    scope: Option<&GenerationScope>,
    mut ask: impl FnMut(&crate::provider_commands::ApprovalRequest),
) -> Routed {
    if crate::exec::is_local(name) {
        let Some(scope) = scope else {
            // The local face was never offered without a repository (the
            // tool list is assembled per generation); a model inventing a
            // warden_* name here gets the honest sentence, no execution.
            let message = format!("tool {name} is not available in this conversation");
            return Routed::refused(message);
        };
        let repository = std::path::PathBuf::from(&scope.repository);
        let audit = state.audit.clone();
        let repo_text = scope.repository.clone();
        let name_owned = name.to_string();
        let executed = tauri::async_runtime::spawn_blocking(move || {
            let out = crate::exec::run(&name_owned, &arguments, &repository);
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(-1);
            audit.record(&crate::exec::audit_entry(
                at,
                &repo_text,
                &name_owned,
                &out,
                out.text.len(),
            ));
            out
        })
        .await
        .map_err(|e| format!("the local tool task failed: {e}"));
        return match executed {
            Ok(out) => Routed::executed(name, out),
            Err(message) => Routed::refused(message),
        };
    }
    if crate::exec::is_write(name) {
        let Some(scope) = scope else {
            let message = format!("tool {name} is not available in this conversation");
            return Routed::refused(message);
        };
        let repository = std::path::PathBuf::from(&scope.repository);
        // Plan FIRST: the card must show the exact command, and a bad
        // argument is a refusal the model gets back without troubling the
        // user with a dialog for something that cannot run anyway.
        let command = match crate::exec::plan_write(name, &arguments, &repository) {
            Ok(command) => command,
            Err(reason) => return Routed::refused(reason),
        };
        // The card. `approvals` is the single source of truth for whether a
        // card is still open: pending is a state, not a missing entry.
        let id = uuid_like(&format!("{}|{}|{command}", scope.repository, name));
        state.open_card(&id);
        ask(&crate::provider_commands::ApprovalRequest {
            id: id.clone(),
            tool: name.to_string(),
            command: command.clone(),
            seconds: APPROVAL_TIMEOUT.as_secs(),
        });
        let decision = state.await_decision(&id).await;
        state.close_card(&id);
        // A timeout and a "no" are the same answer: nothing runs. The model
        // is told which it was, because "nobody answered" and "the user
        // refused" mean different things to a plan.
        let refusal = match decision {
            Some(Decision::Approved) => {
                let audit = state.audit.clone();
                let repo_text = scope.repository.clone();
                let name_owned = name.to_string();
                let executed = tauri::async_runtime::spawn_blocking(move || {
                    let out = crate::exec::run_write(&name_owned, &arguments, &repository);
                    let at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(-1);
                    audit.record(&crate::exec::audit_entry(
                        at,
                        &repo_text,
                        &name_owned,
                        &out,
                        out.text.len(),
                    ));
                    out
                })
                .await
                .map_err(|e| format!("the write tool task failed: {e}"));
                return match executed {
                    Ok(out) => Routed::executed(name, out),
                    // The task itself failed (a panic in the worker), not
                    // the command: the card's command still belongs on the
                    // chip so the user sees what was approved.
                    Err(message) => Routed {
                        content: json!({"error": message}).to_string(),
                        ok: false,
                        error: Some(message),
                        command: Some(command),
                        output: None,
                    },
                };
            }
            Some(Decision::Refused) => "the user refused this command; nothing was run",
            _ => "nobody approved this command in time; nothing was run",
        };
        return Routed {
            content: json!({"error": refusal}).to_string(),
            ok: false,
            error: Some(refusal.to_string()),
            command: Some(command),
            // Nothing ran, so there is no output to expand. The refusal
            // sentence is already the error shown on the chip.
            output: None,
        };
    }
    if MODEL_TOOLS.contains(&name) {
        // The server's answer travels to the model verbatim — prose from
        // repotory_context, JSON from the rest. The model reads both; a
        // client-side parse would reject the prose contract outright.
        return match call_model_tool(session, name, arguments).await {
            // No `output`: an MCP answer is already in the model's tool
            // result, and copying it here would put the same text in two
            // places that can drift.
            Ok(text) => Routed {
                content: text,
                ok: true,
                error: None,
                command: None,
                output: None,
            },
            Err(error) => Routed {
                content: json!({"error": error.clone()}).to_string(),
                ok: false,
                error: Some(error),
                command: None,
                output: None,
            },
        };
    }
    Routed::refused(format!("tool {name} is not available to the model"))
}

/// A card id that names what it is: repository, tool and the exact command,
/// hashed. Two different commands never share an id, so a stale click can
/// only ever answer the command it was looking at.
fn uuid_like(seed: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut hasher);
    let a = hasher.finish();
    format!("card-{a:016x}")
}

/// One routed tool call: what the model is told, what the chip shows, and
/// what the collapsible panel can expand.
///
/// A struct rather than a four-tuple because the fields now carry distinct
/// meanings that a positional return made easy to swap: `content` goes to
/// the model, `output` goes to the USER, and `command` goes to both the
/// chip and the audit.
struct Routed {
    content: String,
    ok: bool,
    error: Option<String>,
    command: Option<String>,
    /// The literal text the model received from a LOCAL command. `None` for
    /// MCP tools (the answer is already in their tool result) and for
    /// refusals that never ran, so the UI shows the chevron only when there
    /// is something behind it.
    output: Option<String>,
}

impl Routed {
    /// A command that ran: the model and the user get the same bytes.
    fn executed(name: &str, out: crate::exec::Executed) -> Self {
        let ExecutedBranch { content, ok, error } = branch_answer(name, out.text.clone(), out.ok);
        Self {
            content,
            ok,
            error,
            command: Some(out.command),
            output: Some(out.text),
        }
    }

    /// A call that never executed — an unknown tool, a bad argument, a
    /// refusal. There is nothing to expand, so `output` stays None.
    fn refused(message: String) -> Self {
        Self {
            content: json!({"error": message.clone()}).to_string(),
            ok: false,
            error: Some(message),
            command: None,
            output: None,
        }
    }
}

/// Shapes one executed local tool's answer for the model. A refusal is
/// information the model reads — it stays visible as the tool result —
/// while `ok=false` marks it for the UI chip.
struct ExecutedBranch {
    content: String,
    ok: bool,
    error: Option<String>,
}

fn branch_answer(name: &str, text: String, ok: bool) -> ExecutedBranch {
    if ok {
        ExecutedBranch {
            content: text,
            ok: true,
            error: None,
        }
    } else {
        ExecutedBranch {
            content: json!({"error": text}).to_string(),
            ok: false,
            error: Some(format!("{name}: {text}")),
        }
    }
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
pub(crate) async fn call_text(
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
    /// Phase 3: 每次执行记录到本地日志. Shared by every generation; its
    /// path is attached once at setup, next to the provider config. The
    /// OnceLock is why a resident process can wire it after `manage()`.
    pub audit: crate::exec::AuditLog,
    /// The approvals the user has granted or refused, keyed by the id the
    /// confirmation card carried. A write tool sends the card, then WAITS
    /// on this map for exactly this decision — a missing entry is "not
    /// decided yet", which is why the wait has a timeout rather than a
    /// default. Nothing here is persisted: an approval belongs to the one
    /// command the user looked at, and dies with the turn.
    approvals: Mutex<HashMap<String, Decision>>,
    /// How long a card stays open. A field rather than the constant so the
    /// suite can drive the timeout path in milliseconds instead of minutes
    /// — the behaviour under test is "unanswered means refused", not the
    /// exact number the product ships.
    approval_timeout: Mutex<Option<std::time::Duration>>,
}

/// What the user did with a confirmation card.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The card is open; nobody has answered yet.
    Pending,
    Approved,
    Refused,
}

/// How long a write tool waits for the user. Long enough to read a command
/// and think, short enough that a card hidden behind another window does not
/// hold the conversation open forever. A timeout is a REFUSAL, never an
/// implicit yes.
const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

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

    /// Records the user's answer to one confirmation card.
    ///
    /// An unknown id is ignored rather than stored: ids are minted per
    /// command by the generation that asked, so anything else is a stale
    /// click on a card whose turn is over — keeping it would be a memory
    /// leak that also happens to grant nothing.
    pub fn decide(&self, id: &str, decision: Decision) -> bool {
        let Ok(mut pending) = self.approvals.lock() else {
            return false;
        };
        if !pending.contains_key(id) {
            return false;
        }
        pending.insert(id.to_string(), decision);
        true
    }

    /// Waits for the decision on `id`. `None` on timeout — which callers
    /// must treat as a refusal. Polling rather than a channel because the
    /// map is the single source of truth for "is a card still open", and a
    /// channel per card would need its own registry with its own leaks.
    async fn await_decision(&self, id: &str) -> Option<Decision> {
        let timeout = self
            .approval_timeout
            .lock()
            .ok()
            .and_then(|value| *value)
            .unwrap_or(APPROVAL_TIMEOUT);
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok(pending) = self.approvals.lock()
                && let Some(decision) = pending.get(id)
                && *decision != Decision::Pending
            {
                return Some(*decision);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Overrides the card timeout (tests only).
    pub fn set_approval_timeout(&self, timeout: std::time::Duration) {
        if let Ok(mut slot) = self.approval_timeout.lock() {
            *slot = Some(timeout);
        }
    }

    /// Mints a card: the id exists as PENDING until `decide` answers it.
    ///
    /// Pending is a distinct state, not a placeholder decision — a map that
    /// only held answers could not tell "nobody has clicked yet" from "the
    /// user said no", and the two must never be conflated: one keeps
    /// waiting, the other refuses immediately.
    fn open_card(&self, id: &str) {
        if let Ok(mut pending) = self.approvals.lock() {
            pending.insert(id.to_string(), Decision::Pending);
        }
    }

    /// Drops a card's entry once its turn is over, decided or not.
    fn close_card(&self, id: &str) {
        if let Ok(mut pending) = self.approvals.lock() {
            pending.remove(id);
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
        approval: None,
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
    // the scope the local face runs in and the diff snapshot the prompt
    // was built from. The reply checkpoint reuses both; a replay leaves it
    // None — a finished answer has already earned its checkpoint.
    let mut scope: Option<GenerationScope> = None;
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
            // Function Calling is a Cinder-thread capability: the memory
            // tools are the server's, so a temporary conversation has none,
            // and the system prompt exists only when tools do — the
            // no-Cinder contract (no server-injected anything) stays
            // intact. The thread's path is the conversation's repository
            // scope, and that scope is ALSO what makes the local read-only
            // face (warden_*) available: a command runs with the repository
            // as its cwd and nowhere else.
            let repository = value["thread"]["path"]
                .as_str()
                .filter(|path| !path.trim().is_empty())
                .map(|path| path.to_string());
            let tools = model_tools(session, repository.is_some()).await?;
            if !tools.is_empty() {
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
                scope = repository.map(|path| GenerationScope {
                    repository: path,
                    diff,
                });
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
                        approval: None,
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
                        // The full argv is known only once the child ran;
                        // the start chip carries the tool name, the finish
                        // chip carries the command (Phase 3's display rule).
                        command: None,
                        output: None,
                    }),
                    approval: None,
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
                let arguments = if tool_call.arguments.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&tool_call.arguments).unwrap_or_else(|_| json!({}))
                };
                let Routed {
                    content,
                    ok,
                    error,
                    command,
                    output,
                } = route_tool(
                    session,
                    state,
                    &tool_call.name,
                    arguments,
                    scope.as_ref(),
                    |card: &crate::provider_commands::ApprovalRequest| {
                        // The card travels on the same channel as the stream:
                        // the window drawing it is the one that asked for the
                        // answer, and no other window may approve a command
                        // this one is waiting on.
                        emit(ChatChunk {
                            delta: String::new(),
                            text: String::new(),
                            done: false,
                            error: None,
                            tool_call: None,
                            approval: Some(card.clone()),
                        });
                    },
                )
                .await;
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
                        command,
                        output,
                    }),
                    approval: None,
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
        if let Some(scope) = &scope
            && let Err(error) = write_handoff(
                session,
                &scope.repository,
                &request.message,
                text,
                scope.diff.as_deref(),
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
