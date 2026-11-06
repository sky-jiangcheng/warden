// The OpenAI-compatible client.
//
// One protocol, many vendors. The standard is small enough to state in full:
//
//   GET  {base}/models             -> {"data": [{"id": "..."}, ...]}
//   POST {base}/chat/completions   -> {"choices": [{"message": {"content": "..."}}]}
//   POST {base}/chat/completions   (with "stream": true) -> SSE lines
//
// OpenRouter, OpenAI, DeepSeek, SiliconFlow, vLLM, LM Studio and Ollama's
// `/v1` surface all answer in that shape, so there is exactly one client
// here and adding a vendor is a configuration change, not a code change.
//
// What this module deliberately does NOT do:
//
//   * It does not know any vendor by name. A provider is a base URL.
//   * It does not retry. A retry hides the error the user needs to see, and
//     a chat turn costs money on a hosted provider — silently sending it
//     twice is worse than failing.
//   * It does not guess a model name. The model list comes from the server.
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::providers::Provider;
/// How long a model-list request may take.
const MODELS_TIMEOUT_SECS: u64 = 20;

/// How long a single non-streaming chat request may take.
const CHAT_TIMEOUT_SECS: u64 = 120;

/// How long the gap between two stream chunks may be before the stream is
/// declared dead. This is not the total request budget — a long answer may
/// legitimately take minutes — it is the "the server went quiet" budget.
const STREAM_IDLE_TIMEOUT_SECS: u64 = 60;

/// One model the provider advertises.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// The id to send back as `model`.
    pub id: String,
    /// The display name, when the provider gives one.
    #[serde(default)]
    pub name: Option<String>,
}

/// Why a request to a provider failed.
///
/// The variants are the distinctions a user can act on: the endpoint was
/// unreachable, the key was refused, the model was unknown, the server
/// answered with an error, or the answer did not arrive.
#[derive(Debug)]
pub enum LlmError {
    /// The request could not be made at all — no network, DNS failed, TLS
    /// could not be established.
    Transport(String),
    /// The provider answered with a non-success status.
    Status { code: u16, message: String },
    /// The provider answered, but not with something this client understands.
    Protocol(String),
    /// The request exceeded its time budget.
    Timeout(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::Transport(msg) => write!(f, "the provider could not be reached: {msg}"),
            LlmError::Status { code, message } => {
                write!(f, "the provider answered {code}: {message}")
            }
            LlmError::Protocol(msg) => write!(f, "the provider's answer was not understood: {msg}"),
            LlmError::Timeout(msg) => write!(f, "the provider did not answer in time ({msg})"),
        }
    }
}

impl std::error::Error for LlmError {}

/// A chat message in the OpenAI-compatible shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// The tool result's id, on a `tool` role message. The explicit rename
    /// overrides the struct's camelCase: the wire field is `tool_call_id`.
    #[serde(rename = "tool_call_id", skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The calls a tool-calling assistant turn made. The echo of that turn
    /// must carry them — a tool message is only defined as the response to
    /// its preceding assistant tool_calls, and strict providers reject the
    /// pairing when the assistant half is missing. The explicit rename keeps
    /// the struct's camelCase rule from turning it into "toolCalls".
    #[serde(rename = "tool_calls", skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallOut>>,
}

/// One tool call, in the wire shape an assistant message echoes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallOut {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// One function the model may call, in the OpenAI-compatible wire shape.
#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    /// Always "function".
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunctionSpec,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolFunctionSpec {
    pub name: String,
    pub description: String,
    /// The JSON-schema parameters, verbatim from the server's tools/list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// One tool call the model asked for, reassembled from the stream.
#[derive(Clone, Debug)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// What one streaming model call produced.
#[derive(Clone, Debug)]
pub struct StreamOutcome {
    /// The text this call wrote. Empty when the call only asked for tools.
    pub content: String,
    /// The tool calls this call asked for, in request order.
    pub tool_calls: Vec<ToolCallRequest>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// The assistant turn that asked for tool calls, echoed verbatim.
    pub fn assistant_with_tool_calls(content: impl Into<String>, calls: Vec<ToolCallOut>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: Some(calls),
        }
    }

    /// The result of one executed tool call.
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_call_id: Some(call_id.into()),
            tool_calls: None,
        }
    }
}

/// Whether a host is on this machine.
///
/// A loopback host is never proxied. This is not a preference: a local
/// Ollama on `127.0.0.1:11434` reached THROUGH a proxy fails, because the
/// proxy is a different machine and `127.0.0.1` means *itself*. Users who
/// run a proxy (Clash and its many relatives set `HTTP_PROXY` globally)
/// would otherwise find the local model — the one that needs no account and
/// no key — the only provider that cannot be reached.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(|c| c == '[' || c == ']');
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host.starts_with("127.")
        || host.ends_with(".localhost")
}

/// The proxy to use for a provider.
///
/// Two rules, in this order:
//
//   1. A loopback provider is reached directly. The proxy cannot reach it.
///   2. Everything else goes through the proxy named by the environment
///      (`HTTPS_PROXY` for https, `HTTP_PROXY` for http), because that is
///      how a user behind a corporate network or in a region with a
//      firewall reaches OpenRouter. `NO_PROXY` is honoured, so a host the
//      user excluded stays excluded.
//
// The client is built per provider rather than shared, because the decision
/// depends on the provider's host. `Proxy::custom` also disables reqwest's
/// automatic system proxy, which is what we want: the environment variables
// are read here instead, and the rule above is applied on top of them.
fn proxy_for(provider: &Provider) -> reqwest::Proxy {
    let host = host_of(provider.normalized_base_url()).unwrap_or_default();
    let no_proxy = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .ok();

    if is_loopback_host(&host) {
        // `Proxy::custom` returning `None` means "connect directly".
        return reqwest::Proxy::custom(|_url| None::<reqwest::Url>);
    }

    let https_proxy = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .ok();
    let http_proxy = std::env::var("HTTP_PROXY")
        .or_else(|_| std::env::var("http_proxy"))
        .ok();

    reqwest::Proxy::custom(move |url: &reqwest::Url| {
        let scheme = url.scheme();
        let chosen = match scheme {
            "https" => https_proxy.clone().or_else(|| http_proxy.clone()),
            _ => http_proxy.clone().or_else(|| https_proxy.clone()),
        }?;
        // The exclusion list is applied here rather than left to the
        // environment's own matcher, so that a host the user excluded is
        // reached directly.
        if let Some(list) = &no_proxy
            && host_is_excluded(url.host_str().unwrap_or_default(), list)
        {
            return None;
        }
        reqwest::Url::parse(&chosen).ok()
    })
}

/// The host part of a URL.
fn host_of(url: &str) -> Option<String> {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest)?;
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme);
    // Strip the userinfo and the port.
    let authority = authority
        .rsplit_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(authority);
    let host = match authority.rfind(':') {
        // A colon inside the brackets of an IPv6 literal is not a port.
        Some(at) if !authority[at..].contains(']') => authority[..at].to_string(),
        _ => authority.to_string(),
    };
    // The brackets around an IPv6 literal are part of the URL syntax, not
    // part of the address.
    Some(host.trim_matches(|c| c == '[' || c == ']').to_string())
}

/// Whether a host is in a `NO_PROXY` exclusion list.
///
/// The list is comma-separated and may hold a leading dot (".example.com"
/// means "and its subdomains") or a wildcard ("*"). A plain name matches
/// that host exactly.
fn host_is_excluded(host: &str, list: &str) -> bool {
    let host = host.to_ascii_lowercase();
    for entry in list.split(',') {
        let entry = entry.trim().to_ascii_lowercase();
        if entry.is_empty() {
            continue;
        }
        if entry == "*" {
            return true;
        }
        if let Some(domain) = entry.strip_prefix('.') {
            if host == domain || host.ends_with(&format!(".{domain}")) {
                return true;
            }
            continue;
        }
        if host == entry {
            return true;
        }
    }
    false
}

/// The client for one provider.
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    headers: Vec<(String, String)>,
}

impl LlmClient {
    /// Builds a client for a provider.
    ///
    /// `api_key` is None for providers that need none — a local Ollama, for
    /// one. When it is set, it goes into the `Authorization` header, which
    /// is what every OpenAI-compatible provider expects.
    pub fn new(provider: &Provider, api_key: Option<String>) -> Result<Self, LlmError> {
        let url = reqwest::Url::parse(provider.normalized_base_url())
            .map_err(|e| LlmError::Protocol(format!("invalid provider URL: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(LlmError::Protocol(
                "provider URL must use http or https with a host".into(),
            ));
        }
        for (name, value) in &provider.headers {
            reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| LlmError::Protocol("invalid provider header name".into()))?;
            reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| LlmError::Protocol("invalid provider header value".into()))?;
        }
        if let Some(key) = &api_key {
            reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|_| LlmError::Protocol("invalid provider API key header".into()))?;
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(CHAT_TIMEOUT_SECS))
            .proxy(proxy_for(provider))
            .build()
            .map_err(|err| LlmError::Transport(err.to_string()))?;
        Ok(Self {
            http,
            base_url: provider.normalized_base_url().to_string(),
            api_key,
            headers: provider
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }

    /// The headers every request carries.
    fn request(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }
        for (name, value) in &self.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        builder
    }

    /// Fetches the model list.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let url = format!("{}/models", self.base_url);
        let request = self
            .request(self.http.get(&url))
            .header("Accept", "application/json");
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(MODELS_TIMEOUT_SECS),
            request.send(),
        )
        .await
        .map_err(|_| LlmError::Timeout("the model list".to_string()))?
        .map_err(|err| LlmError::Transport(err.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| LlmError::Transport(err.to_string()))?;

        if !status.is_success() {
            return Err(LlmError::Status {
                code: status.as_u16(),
                message: error_message(&body),
            });
        }

        parse_models(&body)
    }

    /// Sends a chat request and returns the whole answer.
    pub async fn chat(&self, model: &str, messages: &[ChatMessage]) -> Result<String, LlmError> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        let request = self
            .request(self.http.post(&url))
            .header("Content-Type", "application/json")
            .json(&body);
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(CHAT_TIMEOUT_SECS),
            request.send(),
        )
        .await
        .map_err(|_| LlmError::Timeout("the chat request".to_string()))?
        .map_err(|err| LlmError::Transport(err.to_string()))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| LlmError::Transport(err.to_string()))?;

        if !status.is_success() {
            return Err(LlmError::Status {
                code: status.as_u16(),
                message: error_message(&text),
            });
        }

        parse_chat(&text)
    }

    /// Sends a chat request and streams the answer.
    ///
    /// The callback is called with each text delta as it arrives. This is the
    /// shape a UI wants: the answer appears while it is being written.
    ///
    /// When `tools` is non-empty the request offers them for function
    /// calling; the returned outcome separates the text from any tool calls
    /// the model asked for, so the caller runs the tool loop and keeps
    /// streaming only the final round's text.
    ///
    /// The stream format is Server-Sent Events: lines starting with `data:`,
    /// each holding one JSON chunk, and the literal `data: [DONE]` ending it.
    /// A comment line (`: keep-alive`) is not data and is skipped.
    pub async fn chat_stream(
        &self,
        model: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        mut on_delta: impl FnMut(&str),
    ) -> Result<StreamOutcome, LlmError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if !tools.is_empty() {
            body["tools"] =
                serde_json::to_value(tools).map_err(|e| LlmError::Protocol(e.to_string()))?;
        }
        let request = self
            .request(self.http.post(&url))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&body);

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(CHAT_TIMEOUT_SECS),
            request.send(),
        )
        .await
        .map_err(|_| LlmError::Timeout("the chat request".to_string()))?
        .map_err(|err| LlmError::Transport(err.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response
                .text()
                .await
                .map_err(|err| LlmError::Transport(err.to_string()))?;
            return Err(LlmError::Status {
                code: status.as_u16(),
                message: error_message(&text),
            });
        }

        // `bytes_stream` needs the `stream` feature; it is what makes the
        // answer arrive piece by piece instead of all at once at the end.
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut answer = String::new();
        let mut finished = false;
        let mut calls: Vec<StreamCallAcc> = Vec::new();

        loop {
            let chunk = tokio::time::timeout(
                std::time::Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS),
                stream.next(),
            )
            .await
            .map_err(|_| LlmError::Timeout("the answer stream".to_string()))?;

            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|err| LlmError::Transport(err.to_string()))?;
            buffer.extend_from_slice(&chunk);

            // SSE frames are separated by a blank line, but many providers
            // send one `data:` line per frame with only a newline. Both are
            // handled: complete lines are consumed as they arrive.
            while let Some(at) = buffer.iter().position(|byte| *byte == b'\n') {
                // Decode only complete lines, not arbitrary TCP chunks that
                // can divide a multibyte UTF-8 character.
                let line = std::str::from_utf8(&buffer[..at])
                    .map_err(|e| LlmError::Protocol(format!("invalid stream UTF-8: {e}")))?
                    .trim_end_matches('\r')
                    .to_string();
                buffer.drain(..=at);
                if let Some(data) = line.strip_prefix("data:").map(str::trim)
                    && data != "[DONE]"
                    && !data.is_empty()
                {
                    let value: serde_json::Value = serde_json::from_str(data)
                        .map_err(|e| LlmError::Protocol(format!("invalid stream JSON: {e}")))?;
                    if value.get("error").is_some() {
                        return Err(LlmError::Protocol(error_message(data)));
                    }
                    finished |= value["choices"][0]["finish_reason"].is_string();
                    // Tool-call deltas accumulate across chunks: the id and
                    // name arrive first, the arguments in fragments.
                    for fragment in parse_stream_tool_fragments(&line) {
                        while calls.len() <= fragment.index {
                            calls.push(StreamCallAcc::default());
                        }
                        let slot = &mut calls[fragment.index];
                        if !fragment.id.is_empty() {
                            slot.id = fragment.id;
                        }
                        if !fragment.function.name.is_empty() {
                            slot.name = fragment.function.name;
                        }
                        slot.arguments.push_str(&fragment.function.arguments);
                    }
                }
                if let Some(delta) = parse_stream_line(&line) {
                    if delta.is_empty() {
                        // `[DONE]` — the stream ended.
                        return Ok(finish_stream(answer, calls));
                    }
                    answer.push_str(&delta);
                    on_delta(&delta);
                }
            }
        }

        // A disconnected stream is not a completed answer to persist in the
        // shared history. Accept providers that send finish_reason but no DONE.
        if finished && buffer.iter().all(u8::is_ascii_whitespace) {
            Ok(finish_stream(answer, calls))
        } else {
            Err(LlmError::Protocol(
                "the answer stream ended before completion".into(),
            ))
        }
    }
}

/// Pulls the readable message out of an error body.
///
/// OpenAI-compatible providers answer errors as `{"error": {"message": "..."}}`,
/// but the shape varies: some put the message at the top level, some send
/// plain text, some send HTML (a proxy's error page). The raw body is
/// returned when nothing better is found — a fact is better than a summary
/// that hides it.
pub fn error_message(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "no detail was given".to_string();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(message) = value
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return message.to_string();
        }
        if let Some(message) = value.get("message").and_then(|m| m.as_str()) {
            return message.to_string();
        }
        if let Some(detail) = value.get("detail").and_then(|d| d.as_str()) {
            return detail.to_string();
        }
    }
    // A long HTML page is not a message. Keep the first line only.
    trimmed.lines().next().unwrap_or(trimmed).to_string()
}

/// Parses a model list body.
pub fn parse_models(body: &str) -> Result<Vec<ModelInfo>, LlmError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|err| LlmError::Protocol(err.to_string()))?;
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| LlmError::Protocol("no model list in the answer".to_string()))?;

    let mut models = Vec::with_capacity(data.len());
    for entry in data {
        let Some(id) = entry.get("id").and_then(|id| id.as_str()) else {
            continue;
        };
        models.push(ModelInfo {
            id: id.to_string(),
            name: entry
                .get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.to_string()),
        });
    }
    Ok(models)
}

/// Parses a non-streaming chat body.
pub fn parse_chat(body: &str) -> Result<String, LlmError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|err| LlmError::Protocol(err.to_string()))?;
    let content = value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| {
            choice
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
        })
        .or_else(|| {
            // Some providers answer with the plain completion field.
            value
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("text"))
                .and_then(|t| t.as_str())
        })
        .ok_or_else(|| LlmError::Protocol("no answer in the reply".to_string()))?;
    Ok(content.to_string())
}

/// Parses one SSE line.
///
/// Returns `None` for a line that carries no data (a comment, a field other
/// than `data`, an empty line) and `Some(delta)` for a data line. An empty
/// delta means the stream is over (`data: [DONE]`).
pub fn parse_stream_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    let payload = line.strip_prefix("data:")?.trim();
    if payload.is_empty() {
        return None;
    }
    if payload == "[DONE]" {
        return Some(String::new());
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let delta = value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| {
            choice
                .get("delta")
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
        })
        .or_else(|| {
            value
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("text"))
                .and_then(|t| t.as_str())
        });
    delta.map(|d| d.to_string())
}

/// One tool_call fragment inside a streaming delta chunk, wire shape.
#[derive(Debug, Deserialize)]
struct StreamToolFragment {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: String,
    #[serde(default)]
    function: StreamToolFunction,
}

#[derive(Debug, Default, Deserialize)]
struct StreamToolFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

/// Accumulates one tool_call across multiple SSE chunks.
#[derive(Default)]
struct StreamCallAcc {
    id: String,
    name: String,
    arguments: String,
}

/// Extracts the tool_call fragments of one SSE data line. A line without
/// tool_calls (plain content, `[DONE]`, a comment, malformed JSON) yields an
/// empty vec — the same tolerance `parse_stream_line` applies to content.
fn parse_stream_tool_fragments(line: &str) -> Vec<StreamToolFragment> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Vec::new();
    }
    let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
        return Vec::new();
    };
    if payload.is_empty() || payload == "[DONE]" {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return Vec::new();
    };
    let Some(delta) = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("delta"))
    else {
        return Vec::new();
    };
    let Some(calls) = delta.get("tool_calls") else {
        return Vec::new();
    };
    serde_json::from_value(calls.clone()).unwrap_or_default()
}

/// Builds the stream outcome: content plus reassembled tool calls. A fragment
/// slot with no function name is stream noise and is dropped; a call whose id
/// never arrived gets a stable synthetic one — the pairing only has to agree
/// within this conversation.
fn finish_stream(content: String, calls: Vec<StreamCallAcc>) -> StreamOutcome {
    let tool_calls = calls
        .into_iter()
        .enumerate()
        .filter(|(_, call)| !call.name.is_empty())
        .map(|(index, call)| ToolCallRequest {
            id: if call.id.is_empty() {
                format!("call_{index}")
            } else {
                call.id
            },
            name: call.name,
            arguments: call.arguments,
        })
        .collect();
    StreamOutcome {
        content,
        tool_calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_list_is_read_from_the_data_array() {
        let body = r#"{"data":[{"id":"gpt-4o"},{"id":"claude-3","name":"Claude 3"}]}"#;
        let models = parse_models(body).expect("parse");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(models[1].name.as_deref(), Some("Claude 3"));
    }

    /// A provider that answers with an empty list is a valid answer, and it
    /// is not an error. The UI has to be able to say "this provider has no
    /// models" without pretending the request failed.
    #[test]
    fn an_empty_model_list_is_not_an_error() {
        let models = parse_models(r#"{"data":[]}"#).expect("parse");
        assert!(models.is_empty());
    }

    #[test]
    fn a_body_without_a_data_array_is_a_protocol_error() {
        let err = parse_models(r#"{"models":[]}"#).expect_err("must fail");
        assert!(matches!(err, LlmError::Protocol(_)));
    }

    #[test]
    fn the_chat_answer_is_read_from_the_first_choice() {
        let body = r#"{"choices":[{"message":{"content":"hello"}}]}"#;
        assert_eq!(parse_chat(body).expect("parse"), "hello");
    }

    #[test]
    fn a_chat_body_without_a_choice_is_a_protocol_error() {
        let err = parse_chat(r#"{"id":"x"}"#).expect_err("must fail");
        assert!(matches!(err, LlmError::Protocol(_)));
    }

    #[test]
    fn a_stream_line_yields_its_delta() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#;
        assert_eq!(parse_stream_line(line).as_deref(), Some("Hel"));
    }

    /// The stream's end is a sentinel, not an empty line, and it must not be
    /// mistaken for content.
    #[test]
    fn the_done_sentinel_is_recognized() {
        assert_eq!(parse_stream_line("data: [DONE]").as_deref(), Some(""));
    }

    /// SSE keep-alive comments are not data. Feeding one to the JSON parser
    /// would abort the whole stream.
    #[test]
    fn a_comment_line_is_not_data() {
        assert!(parse_stream_line(": keep-alive").is_none());
        assert!(parse_stream_line("").is_none());
        assert!(parse_stream_line("event: message").is_none());
    }

    #[test]
    fn a_malformed_data_line_is_skipped_rather_than_fatal() {
        assert!(parse_stream_line("data: {not json").is_none());
    }

    #[test]
    fn tool_call_fragments_are_read_from_a_stream_line() {
        let line = r##"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"repotory_metrics","arguments":"{\"pa"}}]}}]}"##;
        let fragments = parse_stream_tool_fragments(line);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].index, 0);
        assert_eq!(fragments[0].id, "call_1");
        assert_eq!(fragments[0].function.name, "repotory_metrics");
        assert_eq!(fragments[0].function.arguments, "{\"pa");
    }

    #[test]
    fn a_line_without_tool_calls_yields_no_fragments() {
        assert!(
            parse_stream_tool_fragments(r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#)
                .is_empty()
        );
        assert!(parse_stream_tool_fragments("data: [DONE]").is_empty());
        assert!(parse_stream_tool_fragments(": keep-alive").is_empty());
        assert!(parse_stream_tool_fragments("data: {not json").is_empty());
    }

    /// The echo must serialize as the protocol spells it: tool_call_id in
    /// snake_case, tool_calls with a lowercase "type". A camelCase leak here
    /// reads fine in Rust and is rejected by every strict provider.
    #[test]
    fn the_tool_message_wire_shape_matches_the_openai_contract() {
        let message = ChatMessage::tool_result("call_1", "{}");
        let json = serde_json::to_value(&message).expect("serialize");
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_1");
        assert!(json.get("toolCalls").is_none());

        let assistant = ChatMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallOut {
                id: "call_1".into(),
                kind: "function".into(),
                function: ToolCallFunction {
                    name: "repotory_metrics".into(),
                    arguments: "{}".into(),
                },
            }],
        );
        let json = serde_json::to_value(&assistant).expect("serialize");
        assert_eq!(json["tool_calls"][0]["id"], "call_1");
        assert_eq!(json["tool_calls"][0]["type"], "function");
        assert!(json.get("toolCallId").is_none());
    }

    /// A tool_call reassembled from fragments carries every part; a nameless
    /// slot is dropped and an id-less call gets a stable synthetic id.
    #[test]
    fn finish_stream_reassembles_and_sanitizes_calls() {
        let mut calls = vec![
            StreamCallAcc {
                id: "call_1".into(),
                name: "repotory_metrics".into(),
                arguments: "{\"path\":".into(),
            },
            StreamCallAcc::default(),
            StreamCallAcc {
                id: String::new(),
                name: "repotory_wiki_search".into(),
                arguments: "{}".into(),
            },
        ];
        calls[1].name = String::new();
        let outcome = finish_stream(String::new(), calls);
        assert_eq!(outcome.tool_calls.len(), 2);
        assert_eq!(outcome.tool_calls[0].id, "call_1");
        assert_eq!(outcome.tool_calls[0].arguments, "{\"path\":");
        assert_eq!(outcome.tool_calls[1].id, "call_2");
    }

    #[test]
    fn the_error_message_is_read_from_the_error_object() {
        assert_eq!(
            error_message(r#"{"error":{"message":"model not found"}}"#),
            "model not found"
        );
    }

    #[test]
    fn a_plain_text_error_body_is_kept() {
        assert_eq!(error_message("upstream refused"), "upstream refused");
    }

    /// A proxy's HTML error page must not arrive as one enormous line.
    #[test]
    fn an_html_error_body_is_cut_to_its_first_line() {
        let body = "<html>\n<head><title>502</title></head>\n</html>";
        assert_eq!(error_message(body), "<html>");
    }

    #[test]
    fn an_empty_error_body_says_so() {
        assert_eq!(error_message("   "), "no detail was given");
    }

    /// The loopback rule is what makes a local Ollama reachable on a machine
    /// that has a proxy configured. Without it, `127.0.0.1` is handed to the
    /// proxy, which connects to ITS own loopback and fails.
    #[test]
    fn a_loopback_host_is_recognized() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.0.0.53"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("[::1]"));
        assert!(is_loopback_host("ollama.localhost"));
    }

    #[test]
    fn a_remote_host_is_not_loopback() {
        assert!(!is_loopback_host("openrouter.ai"));
        assert!(!is_loopback_host("api.openai.com"));
        assert!(!is_loopback_host("192.168.1.10"));
        assert!(!is_loopback_host("1270.0.0.1"));
        assert!(!is_loopback_host("notlocalhost.example.com"));
    }

    #[test]
    fn the_host_is_read_out_of_the_url() {
        assert_eq!(
            host_of("https://openrouter.ai/api/v1").as_deref(),
            Some("openrouter.ai")
        );
        assert_eq!(
            host_of("http://127.0.0.1:11434/v1").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            host_of("https://example.com:8443/v1").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("https://user:pw@example.com/v1").as_deref(),
            Some("example.com")
        );
        assert_eq!(host_of("not a url").as_deref(), None);
    }

    /// An IPv6 literal's colons are part of the address, not a port.
    #[test]
    fn an_ipv6_literal_keeps_its_address() {
        assert_eq!(host_of("http://[::1]:11434/v1").as_deref(), Some("::1"));
    }

    #[test]
    fn the_exclusion_list_is_honored() {
        assert!(host_is_excluded("example.com", "example.com"));
        assert!(host_is_excluded("example.com", "other.com,example.com"));
        assert!(host_is_excluded("api.example.com", ".example.com"));
        assert!(host_is_excluded("example.com", ".example.com"));
        assert!(host_is_excluded("anything", "*"));
        assert!(!host_is_excluded("example.com", "other.com"));
        assert!(!host_is_excluded("notexample.com", "example.com"));
    }
}
