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
    /// A tool call the model made mid-answer, when there was one. `done`
    /// marks the call finished; a fresh answer round follows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolEvent>,
}

/// One function-calling event, for the UI's tool chip.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvent {
    /// The tool's name, as the model called it.
    pub name: String,
    /// False while the call executes, true once its result came back.
    pub done: bool,
    /// True when the call completed successfully. Meaningful once `done` is
    /// true; the UI reads it to show success or failure.
    pub ok: bool,
    /// The failure reason when the call was refused or errored.
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

/// Picks the provider a probe should talk to.
///
/// The editor form is a DRAFT: it can carry a base URL the user has just
/// typed, while the provider it edits points somewhere else entirely. The
/// draft wins on the base URL — otherwise "test connection" keeps probing
/// the SAVED provider (a local Ollama on 127.0.0.1) while the form shows
/// OpenRouter, and the user is told the wrong thing failed.
///
/// Everything else about a saved provider is kept: the id (so the API key is
/// still found), the headers, the name.
pub fn probe_target(
    config: &ProviderConfig,
    id: Option<&str>,
    base_url: Option<&str>,
) -> Result<Provider, String> {
    let saved = id
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .and_then(|i| config.find(i).cloned());
    let draft = base_url.map(str::trim).filter(|u| !u.is_empty());

    match (saved, draft) {
        (Some(mut provider), Some(url)) => {
            // A different endpoint means the saved default model belongs to
            // the old one; keeping it would only produce "model not found".
            if !same_endpoint(&provider.base_url, url) {
                provider.default_model = None;
            }
            provider.base_url = url.to_string();
            Ok(provider)
        }
        (Some(provider), None) => Ok(provider),
        (None, Some(url)) => Ok(Provider {
            id: "probe".to_string(),
            name: "probe".to_string(),
            base_url: url.to_string(),
            default_model: None,
            headers: BTreeMap::new(),
            has_api_key: false,
        }),
        (None, None) => {
            Err("no base URL given \u{2014} fill in the provider's Base URL first".to_string())
        }
    }
}

/// Whether two base URLs point at the same endpoint (a trailing slash is not
/// a different endpoint).
fn same_endpoint(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
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

    fn provider(id: &str, base_url: &str, model: Option<&str>) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            base_url: base_url.to_string(),
            default_model: model.map(|m| m.to_string()),
            headers: BTreeMap::new(),
            has_api_key: false,
        }
    }

    fn config_with(providers: Vec<Provider>) -> ProviderConfig {
        ProviderConfig {
            providers,
            active_provider_id: None,
        }
    }

    #[test]
    fn the_draft_base_url_wins_over_the_saved_one() {
        // The bug this fixes: editing a local Ollama provider, typing an
        // OpenRouter URL and pressing "test connection" used to probe the
        // SAVED Ollama (127.0.0.1:11434) and report that one as unreachable.
        let config = config_with(vec![provider(
            "ollama",
            "http://127.0.0.1:11434/v1",
            Some("llama3"),
        )]);
        let target = probe_target(
            &config,
            Some("ollama"),
            Some("https://openrouter.ai/api/v1"),
        )
        .expect("target");
        assert_eq!(target.base_url, "https://openrouter.ai/api/v1");
        // The id is kept, so the key is still found.
        assert_eq!(target.id, "ollama");
    }

    #[test]
    fn a_changed_endpoint_drops_the_saved_default_model() {
        let config = config_with(vec![provider(
            "ollama",
            "http://127.0.0.1:11434/v1",
            Some("llama3"),
        )]);
        let target = probe_target(
            &config,
            Some("ollama"),
            Some("https://openrouter.ai/api/v1"),
        )
        .expect("target");
        assert_eq!(target.default_model, None);

        let same = probe_target(&config, Some("ollama"), Some("http://127.0.0.1:11434/v1/"))
            .expect("target");
        assert_eq!(same.default_model, Some("llama3".to_string()));
    }

    #[test]
    fn the_saved_provider_is_used_when_the_draft_is_empty() {
        let config = config_with(vec![provider(
            "ollama",
            "http://127.0.0.1:11434/v1",
            Some("llama3"),
        )]);
        let target = probe_target(&config, Some("ollama"), None).expect("target");
        assert_eq!(target.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(target.default_model, Some("llama3".to_string()));
    }

    #[test]
    fn a_new_provider_probes_the_typed_url() {
        let config = config_with(vec![]);
        let target =
            probe_target(&config, None, Some("https://openrouter.ai/api/v1")).expect("target");
        assert_eq!(target.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(target.id, "probe");
    }

    #[test]
    fn neither_a_saved_provider_nor_a_base_url_is_reported_plainly() {
        // The old wording ("no provider is configured yet") pointed at the
        // wrong thing: the user is IN the add-provider form, and the only
        // thing missing is the Base URL they have to type.
        let config = config_with(vec![]);
        let err = probe_target(&config, None, None).expect_err("must fail");
        assert!(err.contains("Base URL"), "unhelpful message: {err}");
    }

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
