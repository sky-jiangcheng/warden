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

// Phase 6 open question 7: the three long-knowledge writes. They are offered
// to the model like the read-only face, but every call is gated behind a
// human confirmation card — a note written here enters retrieval immediately,
// so the model writing into the store it will read next is exactly the case
// ADR-0015's gate exists for. The card carries the proposed arguments and the
// user may edit them before approving (partial agreement).
pub(crate) const KNOWLEDGE_WRITE_TOOLS: &[&str] = &[
    "repotory_notes_create",
    "repotory_notes_update",
    "repotory_notes_organize",
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
    // The knowledge writes ride with the read-only face: their schemas still
    // come verbatim from the server, but the description gets one line that
    // tells the model every call is confirmed by the user first. That is a
    // capability note, not a rewrite of the server's contract.
    specs.extend(
        tools
            .iter()
            .filter(|tool| KNOWLEDGE_WRITE_TOOLS.contains(&tool.name.as_str()))
            .map(|tool| {
                let description = format!(
                    "{} This call writes into long-term knowledge and is CONFIRMED BY THE USER before it runs; propose it when it is what the user asked for.",
                    tool.description
                );
                ToolSpec {
                    kind: "function".to_string(),
                    function: ToolFunctionSpec {
                        name: tool.name.clone(),
                        description,
                        parameters: tool.input_schema.clone(),
                    },
                }
            }),
    );
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
#[derive(Clone)]
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
            kind: crate::provider_commands::ApprovalKind::Command,
            arguments: None,
            seconds: APPROVAL_TIMEOUT.as_secs(),
        });
        let decision = state.await_decision(&id).await;
        state.close_card(&id);
        // A timeout and a "no" are the same answer: nothing runs. The model
        // is told which it was, because "nobody answered" and "the user
        // refused" mean different things to a plan.
        let refusal = match decision {
            // The warden_* write tools are local commands: the card shows the
            // exact argv and the user approves it as-is, so any argument
            // payload on the decision is not used here (it is for the
            // knowledge writes below, which are editable).
            Some(Decision::Approved(_)) => {
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
    if KNOWLEDGE_WRITE_TOOLS.contains(&name) {
        // Phase 6 open question 7: the human confirmation gate for a write
        // into long-term knowledge. The card carries the proposed arguments
        // so the user can edit them before approving (partial agreement); the
        // decision's payload is what actually executes. A timeout, a refusal
        // and a closed window are all the same answer — nothing runs, and the
        // refusal is audited. Fail-closed: no implicit yes.
        let summary = summarize_arguments(name, &arguments);
        let id = uuid_like(&format!("knowledge|{name}|{summary}"));
        state.open_card(&id);
        ask(&crate::provider_commands::ApprovalRequest {
            id: id.clone(),
            tool: name.to_string(),
            command: summary.clone(),
            kind: crate::provider_commands::ApprovalKind::KnowledgeWrite,
            arguments: Some(arguments.clone()),
            seconds: APPROVAL_TIMEOUT.as_secs(),
        });
        let decision = state.await_decision(&id).await;
        state.close_card(&id);
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(-1);
        return match decision {
            Some(Decision::Approved(approved)) => {
                // The arguments the user agreed to: the revised payload when
                // they edited, the original otherwise. Both are audited, and
                // the audit says which one ran.
                let revised = approved.is_some();
                let executed_arguments = approved.unwrap_or_else(|| arguments.clone());
                let result = call_model_tool(session, name, executed_arguments.clone()).await;
                state.knowledge_audit.record(&knowledge_audit_entry(
                    at,
                    name,
                    &summary,
                    &executed_arguments,
                    revised,
                    result.is_ok(),
                    result.as_ref().err().map(String::as_str),
                ));
                return match result {
                    Ok(text) => Routed {
                        content: text,
                        ok: true,
                        error: None,
                        command: Some(summary),
                        output: None,
                    },
                    Err(error) => Routed {
                        content: json!({"error": error.clone()}).to_string(),
                        ok: false,
                        error: Some(error),
                        command: Some(summary),
                        output: None,
                    },
                };
            }
            Some(Decision::Refused) => {
                state.knowledge_audit.record(&knowledge_audit_entry(
                    at,
                    name,
                    &summary,
                    &arguments,
                    false,
                    false,
                    Some("refused by the user"),
                ));
                Routed::refused(
                    "the user refused this knowledge write; nothing was stored".to_string(),
                )
            }
            _ => {
                // Timeout (or the card was closed): fail-closed. Audit it so
                // "nobody answered" is distinguishable from "the user said no".
                state.knowledge_audit.record(&knowledge_audit_entry(
                    at,
                    name,
                    &summary,
                    &arguments,
                    false,
                    false,
                    Some("no decision before the timeout"),
                ));
                Routed::refused(
                    "nobody approved this knowledge write in time; nothing was stored".to_string(),
                )
            }
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

/// A one-line, human-readable summary of a knowledge write's arguments, for
/// the confirmation card's title line.
///
/// The schemas are the server's, so the summary is the compact JSON of the
/// proposed arguments, truncated to a readable length. It is a display
/// string, not a parsed contract — the full arguments travel separately in
/// the card and in the audit.
fn summarize_arguments(name: &str, arguments: &Value) -> String {
    let compact = serde_json::to_string(arguments).unwrap_or_else(|_| "<unserialisable>".into());
    let shown = if compact.len() > 160 {
        format!("{}…", &compact[..160])
    } else {
        compact
    };
    format!("{name} · {shown}")
}

/// The audit entry for one knowledge-write decision.
///
/// Records who decided, when, which tool, the exact arguments that were
/// approved (or proposed, on a refusal), whether they were revised, and how
/// it ended. This is the traceable record open question 7 asks for; it lives
/// in the separate append-only knowledge log, never in the retrieval store.
fn knowledge_audit_entry(
    at: i64,
    tool: &str,
    summary: &str,
    arguments: &Value,
    revised: bool,
    ok: bool,
    error: Option<&str>,
) -> Value {
    json!({
        "at": at,
        "who": std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "local".into()),
        "tool": tool,
        "summary": summary,
        "arguments": arguments,
        "revised": revised,
        "ok": ok,
        "error": error,
    })
}

/// The audit entry for one milestone handoff write-back outcome.
///
/// A third append-only log beside `exec` and `knowledge`: whether this reply
/// earned a checkpoint, and if not, which half let it go — unscoped (no
/// repository) or a failed write. "Earned no handoff" must be distinguishable
/// from "the write-back failed", and neither outcome stays only in stderr.
fn handoff_audit_entry(
    at: i64,
    thread_id: &str,
    repository: Option<&str>,
    event: &str,
    ok: bool,
    error: Option<&str>,
) -> Value {
    json!({
        "at": at,
        "thread_id": thread_id,
        "repository": repository,
        "event": event,
        "ok": ok,
        "error": error,
    })
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

/// Reads the arguments of one tool call the model emitted.
///
/// An EMPTY arguments string is legal and means "no arguments": providers
/// differ on whether they send `""` or `{}` for a tool that takes none.
/// Anything else that is not JSON is a malformed call, and it must not be
/// silently turned into `{}` — that made the tool run with defaults the model
/// never asked for (a search with no pattern, a read with no path) and the
/// refusal then read as the tool's fault instead of the call's.
fn parse_tool_arguments(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(raw).map_err(|e| format!("invalid tool arguments JSON: {e}"))
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
    omit_null_fields(&mut arguments);
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

/// Optional MCP strings are OMITTED, never sent as JSON null.
///
/// `json!({"reply_to": reply_to})` renders a `None` as `"reply_to": null`,
/// and Cinder's parameter layer treats a present key as supplied — a null
/// string field there is a validation refusal, not an absent field. This
/// strip is what turns the ergonomic call sites into the wire shape the IDE
/// clients also send: the key simply absent when there is nothing to say.
/// Top-level only — nested tool payloads have no such key in the current
/// face, and stripping inside them would rewrite a server's contract.
fn omit_null_fields(arguments: &mut Value) {
    if let Some(fields) = arguments.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
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

/// Re-attach an existing thread to a repository, or clear it back to unscoped.
///
/// The scope used to be fixed at thread creation; this is the post-creation
/// re-attach path: a thread created without a repository can still be scoped
/// afterwards, and a scope can be cleared the same way. Same optional-path rule
/// as `create`:
/// absent and "" both clear the scope; the server validates the absolute-path
/// shape.
pub async fn set_path(
    session: &McpSession,
    thread_id: String,
    path: Option<String>,
) -> Result<Value, String> {
    nonempty(&thread_id, "threadId", 200)?;
    let value = call(
        session,
        CHAT_TOOL,
        json!({"operation":"set_path", "thread_id":thread_id, "path":path.unwrap_or_default()}),
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
    // The generation's scope, carried so the retried persistence can still
    // earn its checkpoint: a reply whose save failed the first time was
    // never handed off, and the replay is the only chance left to write it.
    // None only in the interrupted marker, whose Err outcome errors out
    // before any checkpoint decision. thread_path keeps the skip reason
    // honest when the scope was absent ("tool face unavailable" vs "no
    // repository scope") on a replay too.
    scope: Option<GenerationScope>,
    thread_path: Option<String>,
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
    /// Phase 6 open question 7: the confirmation record for knowledge
    /// writes. A SEPARATE append-only log from `audit`: a knowledge write
    /// enters retrieval the moment it lands, so its approval decision is
    /// metadata about the memory layer and must not be confused with — or
    /// mixed into — the retrieval store or the command-execution log. It
    /// records who decided, when, which tool, and the exact arguments
    /// (original or revised) that were approved.
    pub knowledge_audit: crate::exec::AuditLog,
    /// Milestone checkpoint write-back outcome. A SEPARATE append-only log from
    /// both the exec log and the knowledge log: a skipped or failed handoff is
    /// metadata about the memory layer, and keeping it apart makes "this reply
    /// was not written back" traceable without mixing it into command
    /// execution or knowledge-write approvals.
    pub handoff_audit: crate::exec::AuditLog,
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
///
/// `Approved` carries the arguments the user approved — `None` means "as the
/// model proposed", `Some` means the user edited them before agreeing. That
/// distinction is the whole point of the editable card: a knowledge write
/// enters retrieval the moment it lands, so "approve a revised version" is a
/// different act from "approve the original", and the audit must record which
/// one happened.
#[derive(Clone, PartialEq)]
pub enum Decision {
    /// The card is open; nobody has answered yet.
    Pending,
    /// Approved, with the arguments the user agreed to (edited or not).
    Approved(Option<Value>),
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
                return Some(decision.clone());
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

/// One send: validate, replay or generate, then persist the reply and its
/// checkpoint.
///
/// The phases live in `prepare_turn` (validation + replay detection),
/// `stream_rounds` (model ⇄ tools) and `finish_turn` (durable reply +
/// handoff audit). This function owns only the ORDER, which is the part the
/// contract is made of: nothing is written to the thread before a replay is
/// ruled out, the interrupted marker lands before the first paid request,
/// and the checkpoint is written only after the reply is durable.
async fn send_inner(
    session: &McpSession,
    state: &GenerationState,
    store: &ProviderStore,
    request: &ChatRequest,
    text: &mut String,
    emit: &mut impl FnMut(ChatChunk),
    cancel: &AtomicBool,
) -> Result<(), String> {
    // Validation comes BEFORE the guard: a bad request must fail with the
    // input error, not with "this thread is already generating".
    validate_request(request)?;
    // Held for the whole turn: two sends for one thread must not interleave.
    let _guard = state.acquire(request)?;
    let key = (request.thread_id.clone(), request.message_id.clone());

    let (turn, already_durable) = match prepare_turn(session, state, store, request, text).await? {
        PreparedTurn::Replay {
            scope,
            thread_path,
            already_durable,
        } => (TurnScope { scope, thread_path }, already_durable),
        PreparedTurn::Fresh {
            client,
            model,
            mut messages,
            tools,
            scope,
            thread_path,
        } => {
            let turn = TurnScope { scope, thread_path };
            // Record the attempt before the first network await. If its task is
            // interrupted, replay cannot silently issue a second paid request.
            mark_interrupted(state, &key, request)?;

            let outcome = stream_rounds(
                &Round {
                    session,
                    state,
                    client: &client,
                    model: &model,
                    tools: &tools,
                    turn: &turn,
                    cancel,
                },
                &mut messages,
                text,
                emit,
            )
            .await;
            let outcome = outcome.and_then(|answer| {
                if answer.trim().is_empty() {
                    Err("the provider returned no answer".into())
                } else {
                    Ok(answer)
                }
            });
            record_outcome(state, &key, request, &outcome, &turn)?;
            *text = outcome?;
            (turn, false)
        }
    };

    // A reply found in the thread is already durable: it earns no second
    // append and no second checkpoint. Every other path — fresh generation and
    // a replay of an attempt whose save failed — still owes both.
    if !already_durable && let Some(thread) = &request.thread_id {
        finish_turn(session, state, request, thread, text, &turn, key).await?;
    }
    Ok(())
}

/// The input limits, checked before anything is written or locked.
fn validate_request(request: &ChatRequest) -> Result<(), String> {
    nonempty(&request.message, "message", MAX_INPUT_CHARS)?;
    nonempty(&request.message_id, "messageId", 190)?;
    if let Some(id) = &request.thread_id {
        nonempty(id, "threadId", 200)?;
    }
    Ok(())
}

/// What one send resolved to before any model call: either the answer already
/// exists, or this turn genuinely has to generate one.
enum PreparedTurn {
    /// The reply is known; `text` holds it. `already_durable` says whether it
    /// is already in Cinder (a thread read-back, which must NOT be appended
    /// twice) or only in the attempt map (a replay after a failed save, which
    /// still owes the append and the checkpoint).
    Replay {
        scope: Option<GenerationScope>,
        thread_path: Option<String>,
        already_durable: bool,
    },
    /// Nothing to replay: provider, history and tool face are ready.
    Fresh {
        client: LlmClient,
        model: String,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolSpec>,
        scope: Option<GenerationScope>,
        thread_path: Option<String>,
    },
}

/// The repository context one generation was built in.
///
/// `scope` is what the local tool face may touch — the thread's repository and
/// the diff snapshot the prompt described. `thread_path` is the repository the
/// thread was persisted with, which is what the audit records when a reply
/// earns no checkpoint. They differ when the thread has a repository but no
/// tool face, and that difference is the skip reason's honesty.
struct TurnScope {
    scope: Option<GenerationScope>,
    thread_path: Option<String>,
}

/// Everything one streaming generation reads that does not change per round.
struct Round<'a> {
    session: &'a McpSession,
    state: &'a GenerationState,
    client: &'a LlmClient,
    model: &'a str,
    tools: &'a [ToolSpec],
    turn: &'a TurnScope,
    cancel: &'a AtomicBool,
}

/// Input validation and replay detection, in the order the contract requires.
///
/// Everything here happens BEFORE the first paid request: input limits, the
/// in-process dedup map, the thread's immutable ids, and — for a fresh turn —
/// the provider configuration and the history the model will see. `text` is
/// filled here only on a replay.
async fn prepare_turn(
    session: &McpSession,
    state: &GenerationState,
    store: &ProviderStore,
    request: &ChatRequest,
    text: &mut String,
) -> Result<PreparedTurn, String> {
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
            return Ok(PreparedTurn::Replay {
                scope: None,
                thread_path: None,
                // The reply came out of Cinder: appending it again would
                // duplicate a message the server already holds.
                already_durable: true,
            });
        }
    }

    if let Some(attempt) = cached {
        *text = attempt.outcome?;
        // The replay restores the generation's scope, not just its text:
        // the reply whose save failed the first time was never handed off,
        // and this append is the write that finally makes it durable —
        // the checkpoint belongs to this generation, not to a fresh one.
        return Ok(PreparedTurn::Replay {
            scope: attempt.scope,
            thread_path: attempt.thread_path,
            // The attempt map is process-local: this reply has not reached
            // Cinder yet, so the append that failed before must run now.
            already_durable: false,
        });
    }

    let store = store.clone();
    let input = request.clone();
    let (client, model) = tauri::async_runtime::spawn_blocking(move || prepare(&store, &input))
        .await
        .map_err(|e| format!("provider configuration task failed: {e}"))??;

    // `scope` and `thread_path` are set only by the threaded branch below.
    let mut scope: Option<GenerationScope> = None;
    let mut thread_path: Option<String> = None;
    let (messages, tools) = if let Some(thread) = &request.thread_id {
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
        thread_path = repository.clone();
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

    Ok(PreparedTurn::Fresh {
        client,
        model,
        messages,
        tools,
        scope,
        thread_path,
    })
}

/// The generation loop: model → (tool calls → execute → feed back)×, each
/// round streamed. The final round's text is the answer — a tool round's
/// preamble restarts the visible text via the tool event's empty `text`.
async fn stream_rounds(
    round: &Round<'_>,
    messages: &mut Vec<ChatMessage>,
    text: &mut String,
    emit: &mut impl FnMut(ChatChunk),
) -> Result<String, String> {
    let mut round_number: usize = 0;
    loop {
        round_number += 1;
        // A disconnected webview must not start (or continue) another
        // paid round. The error is surfaced through the final ChatChunk.
        if round.cancel.load(Ordering::Relaxed) {
            break Err("the chat window disconnected; generation was stopped".to_string());
        }
        // The round cap: once spent, the next call is text-only, so the
        // answer arrives in words instead of the cap silently turning
        // into an empty reply.
        let round_tools: &[ToolSpec] = if round_number > MAX_TOOL_ROUNDS {
            &[]
        } else {
            round.tools
        };
        let streamed = match round
            .client
            .chat_stream(round.model, messages.as_slice(), round_tools, |delta| {
                if round.cancel.load(Ordering::Relaxed) {
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
        if round.cancel.load(Ordering::Relaxed) {
            break Err("the chat window disconnected; generation was stopped".to_string());
        }
        if streamed.tool_calls.is_empty() {
            break if streamed.content.trim().is_empty() {
                Err("the provider returned no answer".to_string())
            } else {
                Ok(streamed.content)
            };
        }
        if round.tools.is_empty() {
            break Err("the model requested tools but none were offered".to_string());
        }
        if round_number > MAX_TOOL_ROUNDS {
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
            let arguments = match parse_tool_arguments(&tool_call.arguments) {
                Ok(arguments) => arguments,
                Err(error) => {
                    // The assistant turn (with its calls) is already in
                    // `messages`; answering with a tool result keeps the
                    // pairing valid, and the model reads the refusal as
                    // information it can correct on the next round.
                    let refused = Routed::refused(error);
                    messages.push(ChatMessage::tool_result(
                        tool_call.id.clone(),
                        refused.content,
                    ));
                    emit(ChatChunk {
                        delta: String::new(),
                        text: String::new(),
                        done: false,
                        error: None,
                        tool_call: Some(ToolEvent {
                            name: tool_call.name.clone(),
                            done: true,
                            ok: refused.ok,
                            error: refused.error,
                            command: refused.command,
                            output: refused.output,
                        }),
                        approval: None,
                    });
                    continue;
                }
            };
            let Routed {
                content,
                ok,
                error,
                command,
                output,
            } = route_tool(
                round.session,
                round.state,
                &tool_call.name,
                arguments,
                round.turn.scope.as_ref(),
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
    }
}

/// The interrupted marker: an Attempt whose outcome is an error so a replay
/// cannot silently issue a second paid request.
fn mark_interrupted(
    state: &GenerationState,
    key: &(Option<String>, String),
    request: &ChatRequest,
) -> Result<(), String> {
    state
        .attempts
        .lock()
        .map_err(|_| "chat generation state is unavailable")?
        .insert(
            key.clone(),
            Attempt {
                message: request.message.clone(),
                outcome: Err("generation was interrupted; use a new messageId to retry".into()),
                scope: None,
                thread_path: None,
            },
        );
    Ok(())
}

/// Records the finished outcome (success or failure) together with the scope
/// the replay will need if the persist step below fails.
fn record_outcome(
    state: &GenerationState,
    key: &(Option<String>, String),
    request: &ChatRequest,
    outcome: &Result<String, String>,
    turn: &TurnScope,
) -> Result<(), String> {
    state
        .attempts
        .lock()
        .map_err(|_| "chat generation state is unavailable")?
        .insert(
            key.clone(),
            Attempt {
                message: request.message.clone(),
                outcome: outcome.clone(),
                scope: turn.scope.clone(),
                thread_path: turn.thread_path.clone(),
            },
        );
    Ok(())
}

/// Persists the reply, then writes the milestone checkpoint.
///
/// Order matters: the checkpoint is written only after the reply is durable —
/// a checkpoint about a reply that failed to save would record a conversation
/// the next session cannot read back.
async fn finish_turn(
    session: &McpSession,
    state: &GenerationState,
    request: &ChatRequest,
    thread: &str,
    text: &str,
    turn: &TurnScope,
    key: (Option<String>, String),
) -> Result<(), String> {
    append(
        session,
        thread,
        &format!("{}-reply", request.message_id),
        "assistant",
        text,
        Some(&request.message_id),
    )
    .await
    .map_err(|e| {
        format!(
            "the answer was generated but could not be saved to Cinder; \
             resend the same messageId to retry saving: {e}"
        )
    })?;
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
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(-1);
    match &turn.scope {
        Some(scope) => {
            match write_handoff(
                session,
                &scope.repository,
                &request.message,
                text,
                scope.diff.as_deref(),
            )
            .await
            {
                Ok(()) => state.handoff_audit.record(&handoff_audit_entry(
                    at,
                    thread,
                    Some(&scope.repository),
                    "written",
                    true,
                    None,
                )),
                Err(error) => {
                    // The reply is saved; the checkpoint is the loss. It must
                    // not turn into a failed send. It must not vanish quietly
                    // either: stderr keeps the degraded-path signal, the
                    // audit line keeps the durable record the other degraded
                    // paths in this app keep too.
                    eprintln!("[handoff] the session checkpoint could not be written: {error}");
                    state.handoff_audit.record(&handoff_audit_entry(
                        at,
                        thread,
                        Some(&scope.repository),
                        "failed",
                        false,
                        Some(&error),
                    ));
                }
            }
        }
        None => {
            // The thread exists but earns no scope: no local face and no
            // milestone checkpoint. Record the skip so "earned no handoff"
            // is distinguishable from "the write-back failed" — a silent
            // skip was the shape that made the missing handoffs invisible
            // on this machine. The reason splits the two ways a scope can
            // be absent: no repository on the thread, or the tool face came
            // back empty.
            let reason = if turn.thread_path.is_none() {
                "no repository scope"
            } else {
                "tool face unavailable"
            };
            state.handoff_audit.record(&handoff_audit_entry(
                at,
                thread,
                turn.thread_path.as_deref(),
                "skipped",
                false,
                Some(reason),
            ));
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
/// One `<file> +<added> -<deleted>` line per changed file, in diff order.
/// The checkpoint's `changes` section used to stay empty with the raw diff
/// stuffed into `next_steps`; a file list is what a later session scans
/// first. Capped at the server's 20-item section limit.
fn changed_files(diff: &str) -> Vec<String> {
    if diff.trim().is_empty() {
        return Vec::new();
    }
    let mut files: Vec<String> = Vec::new();
    let mut current: Option<(String, usize, usize)> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            if let Some((name, added, deleted)) = current.take() {
                files.push(format!("{name} +{added} -{deleted}"));
            }
            // "diff --git a/x b/x": the b/ half is the post-change path, and
            // the a/ half names it too, so the tail after " b/" is enough.
            let name = rest.rsplit_once(" b/").map(|(_, b)| b).unwrap_or(rest);
            current = Some((name.to_string(), 0, 0));
        } else if let Some((_, added, deleted)) = current.as_mut() {
            if line.starts_with("+++") || line.starts_with("---") {
                continue;
            }
            if line.starts_with('+') {
                *added += 1;
            } else if line.starts_with('-') {
                *deleted += 1;
            }
        }
    }
    if let Some((name, added, deleted)) = current.take() {
        files.push(format!("{name} +{added} -{deleted}"));
    }
    files.truncate(20);
    files
}

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
    let changes = diff
        .map(changed_files)
        .unwrap_or_default()
        .into_iter()
        .map(|line| excerpt(&line, 200))
        .collect::<Vec<_>>();
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
            "changes": changes,
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

    /// An optional string that is None leaves the wire as an ABSENT key, not
    /// as null: Cinder's parameter layer treats a present key as supplied and
    /// would refuse null where it wants a string. `json!` renders `None` as
    /// null at the call sites — this strip is what makes the wire shape match
    /// the IDE clients, and the contract is pinned here so a refactor cannot
    /// quietly start shipping nulls again.
    #[test]
    fn changed_files_summarizes_each_file_with_counts_in_order() {
        let diff = "\
diff --git a/src/main.go b/src/main.go
index 111..222 100644
--- a/src/main.go
+++ b/src/main.go
@@ -1,3 +1,4 @@
+added
 context
-removed
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
+doc";
        assert_eq!(
            changed_files(diff),
            ["src/main.go +1 -1", "README.md +1 -0"]
        );
    }

    #[test]
    fn changed_files_is_empty_without_a_diff() {
        assert!(changed_files("").is_empty());
        assert!(changed_files("   \n").is_empty());
        // Prose that merely mentions files is not a diff.
        assert!(changed_files("edit src/main.go by hand").is_empty());
    }

    #[test]
    fn changed_files_caps_at_the_server_section_limit() {
        let diff: String = (0..30)
            .map(|i| {
                format!("diff --git a/f{i}.txt b/f{i}.txt\n--- a/f{i}.txt\n+++ b/f{i}.txt\n+x\n")
            })
            .collect();
        let got = changed_files(&diff);
        assert_eq!(got.len(), 20);
        assert_eq!(got[0], "f0.txt +1 -0");
        assert_eq!(got[19], "f19.txt +1 -0");
    }

    #[test]
    fn an_absent_optional_string_is_omitted_not_sent_as_null() {
        let mut arguments = json!({
            "operation": "append",
            "thread_id": "t",
            "id": "m",
            "role": "user",
            "content": "hello",
            "source": "warden",
            "reply_to": Option::<String>::None,
        });
        omit_null_fields(&mut arguments);
        assert_eq!(
            arguments,
            json!({
                "operation": "append",
                "thread_id": "t",
                "id": "m",
                "role": "user",
                "content": "hello",
                "source": "warden",
            })
        );

        // The strip is deliberately top-level only: a nested null belongs to
        // a payload the server defines, and rewriting it would be editing a
        // contract this repo does not own.
        let mut nested = json!({ "outer": { "inner": Option::<String>::None } });
        omit_null_fields(&mut nested);
        assert_eq!(nested, json!({ "outer": { "inner": null } }));
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
    fn handoff_audit_entry_records_outcome_and_reason() {
        let written =
            handoff_audit_entry(1_763_000_000, "t-1", Some("/repo"), "written", true, None);
        assert_eq!(written["event"], json!("written"));
        assert_eq!(written["ok"], json!(true));
        assert_eq!(written["repository"], json!("/repo"));
        assert!(written["error"].is_null());

        let skipped = handoff_audit_entry(
            1_763_000_000,
            "t-2",
            None,
            "skipped",
            false,
            Some("no repository scope"),
        );
        assert_eq!(skipped["event"], json!("skipped"));
        assert!(skipped["repository"].is_null());
        assert_eq!(skipped["error"], json!("no repository scope"));
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

    // A tool that takes no arguments arrives as "" or "{}" depending on the
    // provider; both must reach the tool as an empty object.
    #[test]
    fn absent_tool_arguments_are_an_empty_object() {
        assert_eq!(parse_tool_arguments("").unwrap(), json!({}));
        assert_eq!(parse_tool_arguments("   ").unwrap(), json!({}));
        assert_eq!(parse_tool_arguments("{}").unwrap(), json!({}));
    }

    // The regression this helper exists for: a malformed call used to fall
    // back to `{}` and execute with defaults the model never asked for.
    #[test]
    fn malformed_tool_arguments_are_a_refusal_not_a_default() {
        let error = parse_tool_arguments("{\"path\": ").unwrap_err();
        assert!(error.contains("invalid tool arguments"), "{error}");
        let refused = Routed::refused(error);
        assert!(!refused.ok);
        assert!(refused.error.is_some());
        let payload: Value = serde_json::from_str(&refused.content).unwrap();
        assert!(payload["error"].as_str().unwrap().contains("invalid tool"));
    }
}
