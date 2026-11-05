// The logic behind the provider commands.
//
// Everything the frontend does with a model goes through here: it never
// holds a base URL, an API key or a socket. That is not only a security
// rule — the frontend is a webview, and a key that reaches a webview has
// left the app.
//
// The three capabilities the user asked for by name are here:
//
//   * `provider_fetch_models` — pull the model list from the provider;
//   * `provider_test_connection` — check that the link actually works;
//   * `chat_send` — talk to the model, streaming the answer.
//
// "Test the connection" deserves a word, because the obvious implementation
// is wrong. Pinging the base URL only proves that something answers on that
// host: it does not prove the key is accepted, does not prove the model
// exists, and does not prove the provider can actually answer a prompt. So
// the test makes a real chat request with the smallest possible prompt and
// reports what came back. A provider that passes this test can serve the
// conversation.
//
// WHY THE COMMAND FUNCTIONS LIVE IN `lib.rs`: Tauri's `generate_handler!`
// expands to the generated `__cmd__<name>` macros, and those macros are
// private to the module that defines them — naming one from another module
// is a compile error ("macro import is private"). So the `#[tauri::command]`
// functions have to sit next to the handler list. This module holds the
// logic they call.
use serde::Serialize;
use std::collections::BTreeMap;
use tauri::Manager;

use crate::llm::LlmError;
use crate::providers::{Provider, ProviderConfig, ProviderStore};

/// The provider configuration, as the frontend sees it.
///
/// The API key is NOT in here — only whether one exists. The frontend never
/// needs the key's value, and a payload that carries it is a payload that
/// can leak it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub default_model: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub has_api_key: bool,
    /// Whether this is the provider the chat uses.
    pub active: bool,
}

impl ProviderView {
    pub fn of(provider: &Provider, active_id: Option<&str>) -> Self {
        Self {
            id: provider.id.clone(),
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            default_model: provider.default_model.clone(),
            headers: provider.headers.clone(),
            has_api_key: provider.has_api_key,
            active: active_id == Some(provider.id.as_str()),
        }
    }
}

/// The provider list plus the active selection.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderList {
    pub providers: Vec<ProviderView>,
    pub active_provider_id: Option<String>,
    /// A problem with the stored configuration, if there was one.
    pub problem: Option<String>,
}

/// What the user typed when saving a provider.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    /// The provider's id. Empty for a new provider — one is derived from
    /// the name.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The API key. Empty means "leave the stored one alone"; `None` and
    /// `Some("")` are treated the same, because an empty key is not a key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Whether to store the key at all. A provider without a key is normal
    /// (a local Ollama), so the key is only touched when one is given.
    #[serde(default)]
    pub save_api_key: bool,
}

/// The result of testing a provider.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTest {
    /// Whether the provider answered the test request.
    pub ok: bool,
    /// What came back — the model's own words when it worked, the reason
    /// when it did not.
    pub detail: String,
    /// The model that was tested.
    pub model: String,
    /// How long the round trip took, in milliseconds.
    pub latency_ms: u64,
}

/// One streamed piece of an answer.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatChunk {
    /// The text that just arrived.
    pub delta: String,
    /// The whole answer so far.
    pub text: String,
    /// True on the last chunk.
    pub done: bool,
    /// The error, when the stream failed.
    pub error: Option<String>,
}

/// Turns a client error into something a person can act on.
pub fn describe_llm_error(err: &LlmError) -> String {
    err.to_string()
}

/// The provider store, built from the app's config directory.
pub fn store_for<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<ProviderStore, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| format!("the app config directory could not be located: {err}"))?;
    Ok(ProviderStore::new(dir.join("providers.json")))
}

/// Reads the provider config, seeding the default one on first run.
///
/// The default is a local Ollama on its OpenAI-compatible surface: it needs
/// no account and no key, so a fresh install can talk to a model the moment
/// one is installed there. It is a default, not a commitment — the user can
/// point the app at any other provider.
pub fn config_or_default(store: &ProviderStore) -> (ProviderConfig, Option<String>) {
    let (config, problem) = store.load();
    if config.providers.is_empty() && problem.is_none() {
        let seeded = crate::providers::default_config();
        let _ = store.save(&seeded);
        return (seeded, None);
    }
    (config, problem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_never_carries_the_key() {
        let provider = Provider {
            id: "p".to_string(),
            name: "P".to_string(),
            base_url: "https://x/v1".to_string(),
            default_model: None,
            headers: BTreeMap::new(),
            has_api_key: true,
        };
        let json =
            serde_json::to_string(&ProviderView::of(&provider, Some("p"))).expect("serialize");
        assert!(json.contains("\"hasApiKey\":true"));
        assert!(json.contains("\"active\":true"));
        assert!(!json.contains("apiKey\""));
    }
}
