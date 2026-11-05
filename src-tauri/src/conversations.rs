//! Shared conversations are Cinder MCP data, never a second local database.
//! Only the originating Warden window receives token deltas. Other clients
//! poll `read` for completed messages; there is no cross-IDE token stream.
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::llm::{ChatMessage, LlmClient};
use crate::mcp_client::McpSession;
pub use crate::provider_commands::ChatChunk;
use crate::providers::{ProviderStore, load_api_key};

// Capability bindings, not a substitute for the server's tools/list.
const CHAT_TOOL: &str = "repotory_chat";
const METRICS_TOOL: &str = "repotory_metrics";
const MAX_INPUT_CHARS: usize = 12_000;
const CONTEXT_CHARS: usize = 24_000;
const CONTEXT_MESSAGES: usize = 32;
// Must equal Cinder's `service.MaxChatTitleLen`. A local cap looser than the
// server's is worse than none: the title looks valid here and then fails there
// with a message the user cannot act on.
const MAX_TITLE_CHARS: usize = 160;

async fn call(session: &McpSession, name: &str, mut arguments: Value) -> Result<Value, String> {
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
    let text = answer.map_err(|e| format!("Cinder {name}: {e}"))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("Cinder {name} returned invalid JSON: {e}"))?;
    if !value.is_object() {
        return Err(format!("Cinder {name} must return a JSON object"));
    }
    Ok(value)
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

pub async fn metrics(session: &McpSession, path: Option<String>) -> Result<Value, String> {
    let value = call(session, METRICS_TOOL, json!({"path":path})).await?;
    for field in [
        "repositoryCount",
        "commitsToday",
        "additionsToday",
        "deletionsToday",
        "notesCount",
        "pendingReviewCount",
    ] {
        if !value[field].is_number() {
            return Err(format!("Cinder metrics is missing numeric {field}"));
        }
    }
    if !value
        .get("totalLines")
        .is_some_and(|v| v.is_null() || v.is_number())
        || !value
            .get("refreshedAt")
            .is_some_and(|v| v.is_null() || v.is_string())
    {
        return Err("Cinder metrics is missing totalLines or refreshedAt".into());
    }
    Ok(value)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    id: String,
    role: String,
    content: String,
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
    // This function runs on the blocking pool: both config and keyring do IO.
    let key = if provider.has_api_key {
        Some(
            load_api_key(&provider.id)?
                .ok_or("the provider's API key is missing from the keyring")?,
        )
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
    mut emit: impl FnMut(ChatChunk),
) -> Result<(), String> {
    let mut text = String::new();
    let outcome = send_inner(session, state, store, &request, &mut text, &mut emit).await;
    emit(ChatChunk {
        delta: String::new(),
        text,
        done: true,
        error: outcome.as_ref().err().cloned(),
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
    if let Some(attempt) = cached {
        *text = attempt.outcome?;
    } else {
        let store = store.clone();
        let input = request.clone();
        let (client, model) = tauri::async_runtime::spawn_blocking(move || prepare(&store, &input))
            .await
            .map_err(|e| format!("provider configuration task failed: {e}"))??;
        let messages = if let Some(thread) = &request.thread_id {
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
            context(&history, &request.message_id)?
        } else {
            vec![ChatMessage::user(&request.message)]
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
        let outcome = client
            .chat_stream(&model, &messages, |delta| {
                text.push_str(delta);
                emit(ChatChunk {
                    delta: delta.to_string(),
                    text: text.clone(),
                    done: false,
                    error: None,
                });
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|answer| {
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
    }
    Ok(())
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
