// Provider configuration: where the model comes from.
//
// The Agent does not care WHICH model serves the conversation, and it must
// not hard-code a list of vendors. The de-facto standard for that is the
// OpenAI-compatible API: `GET {base}/models` lists the models and
// `POST {base}/chat/completions` talks to them. OpenRouter, OpenAI,
// DeepSeek, SiliconFlow, vLLM, LM Studio and Ollama's `/v1` surface all
// speak it, so one client covers all of them and a new vendor needs no code —
// only a base URL.
//
// That is the whole point of this module: "a provider is anything that
// conforms to the standard". A local Ollama on `http://127.0.0.1:11434/v1`
// and OpenRouter at `https://openrouter.ai/api/v1` are the same shape of
// thing, and treating them differently (a "local runtime registry" plus a
// separate cloud path) is exactly the special-casing that makes adding a
// vendor a code change.
//
// What is stored where:
//
//   * the non-secret parts of a provider (name, base URL, default model) go
//     to a JSON file in the app config directory — they are configuration and
//     they are meant to be readable;
//   * the API key goes to the operating system's credential store (macOS
//     Keychain, Windows Credential Manager, Linux Secret Service). A key in a
//     plain file is a key in every backup, every sync folder and every
//     screen share that file lands in.
//
// The config file therefore holds a KEY REFERENCE (the credential's account
// name), never the key itself.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The credential service the API keys live under, in the OS credential store.
const KEYRING_SERVICE: &str = "repotory-agent";

/// One configured model provider.
///
/// The `kind` field is deliberately absent: there is only one kind — the
/// OpenAI-compatible standard. A future non-conforming vendor would be a
/// second kind, and it would be added when it exists rather than reserved for
/// it now.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    /// Stable identifier, used as the credential account name and as the
    /// key into the provider list.
    pub id: String,
    /// What the user calls this provider, shown in the UI.
    pub name: String,
    /// The API root, e.g. `https://openrouter.ai/api/v1`. The `/models` and
    /// `/chat/completions` paths are appended to it.
    pub base_url: String,
    /// The model to use when the user has not chosen one.
    #[serde(default)]
    pub default_model: Option<String>,
    /// Extra headers sent with every request. Some providers want an app
    /// identifier (OpenRouter's `HTTP-Referer` / `X-Title`); nothing requires
    /// it, so the map is usually empty.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Whether the API key lives in the credential store. A provider without
    /// a key is legitimate — a local Ollama needs none.
    #[serde(default)]
    pub has_api_key: bool,
}

impl Provider {
    /// The API root with any trailing slash removed.
    ///
    /// The trailing slash matters: `"https://host/v1/"` + `"/models"` gives
    /// `https://host/v1//models`, which some gateways reject.
    pub fn normalized_base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    /// The URL of the model list.
    pub fn models_url(&self) -> String {
        format!("{}/models", self.normalized_base_url())
    }

    /// The URL of the chat endpoint.
    pub fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.normalized_base_url())
    }
}

/// Everything the app knows about providers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// The providers, in the order the UI lists them.
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// The id of the provider the chat uses.
    #[serde(default)]
    pub active_provider_id: Option<String>,
}

impl ProviderConfig {
    /// The active provider, if it is still configured.
    ///
    /// A dangling active id is reported as "no active provider" rather than
    /// as an error: the provider may have been deleted, and the user needs to
    /// pick another — not read a complaint about a broken file.
    pub fn active(&self) -> Option<&Provider> {
        let id = self.active_provider_id.as_ref()?;
        self.providers.iter().find(|p| &p.id == id)
    }

    /// Finds a provider by id.
    pub fn find(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Inserts or replaces a provider by id, keeping the list order stable
    /// for the ones already there.
    pub fn upsert(&mut self, provider: Provider) {
        match self.providers.iter_mut().find(|p| p.id == provider.id) {
            Some(existing) => *existing = provider,
            None => self.providers.push(provider),
        }
    }

    /// Removes a provider by id, and clears the active id when it pointed at
    /// the removed one.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.providers.len();
        self.providers.retain(|p| p.id != id);
        let removed = self.providers.len() != before;
        if removed && self.active_provider_id.as_deref() == Some(id) {
            self.active_provider_id = None;
        }
        removed
    }
}

/// Where the provider configuration lives on disk.
#[derive(Clone, Debug)]
pub struct ProviderStore {
    path: PathBuf,
}

impl ProviderStore {
    /// Opens the store at the given config file path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the configuration.
    ///
    /// A missing file is an empty configuration — the first run. A corrupt
    /// file is also an empty configuration, but the reason is worth keeping:
    /// silently discarding a user's provider list would be worse than
    /// reporting it, so the caller is told the file was unreadable.
    pub fn load(&self) -> (ProviderConfig, Option<String>) {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (ProviderConfig::default(), None);
            }
            Err(err) => {
                return (
                    ProviderConfig::default(),
                    Some(format!("the provider config could not be read: {err}")),
                );
            }
        };
        match serde_json::from_str(&raw) {
            Ok(config) => (config, None),
            Err(err) => (
                ProviderConfig::default(),
                Some(format!("the provider config is not valid JSON: {err}")),
            ),
        }
    }

    /// Writes the configuration.
    ///
    /// The file is written with owner-only permissions: it holds no secret
    /// today, but a config file the user may later point at a key-bearing
    /// path should not be world-readable either.
    pub fn save(&self, config: &ProviderConfig) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("the config directory could not be created: {err}"))?;
        }
        let body = serde_json::to_string_pretty(config)
            .map_err(|err| format!("the provider config could not be serialized: {err}"))?;
        std::fs::write(&self.path, body)
            .map_err(|err| format!("the provider config could not be written: {err}"))?;
        restrict_permissions(&self.path);
        Ok(())
    }
}

/// Makes a file owner-only where the platform supports it.
///
/// A failure here is not fatal: the file is already written, and a config
/// with no secret is not an emergency. The permission is a hardening step,
/// not a correctness requirement.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Stores an API key in the operating system's credential store.
pub fn store_api_key(provider_id: &str, key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
        .map_err(|err| format!("the credential store is not available: {err}"))?;
    entry
        .set_password(key)
        .map_err(|err| format!("the API key could not be stored: {err}"))
}

/// Reads an API key from the operating system's credential store.
///
/// A missing credential is not an error: it means "this provider has no key",
/// which is a normal state for a local runtime.
pub fn load_api_key(provider_id: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
        .map_err(|err| format!("the credential store is not available: {err}"))?;
    match entry.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(format!("the API key could not be read: {err}")),
    }
}

/// Removes an API key from the operating system's credential store.
pub fn delete_api_key(provider_id: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
        .map_err(|err| format!("the credential store is not available: {err}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(format!("the API key could not be removed: {err}")),
    }
}

/// The default provider a fresh install starts with.
///
/// It is Ollama on its OpenAI-compatible surface, because that is the one
/// provider that needs no account and no key — the app can talk to it the
/// moment it is installed. A user who wants a hosted model configures one
/// themselves; the app does not assume a vendor on their behalf.
pub fn default_provider() -> Provider {
    Provider {
        id: "ollama".to_string(),
        name: "Ollama (本机)".to_string(),
        base_url: "http://127.0.0.1:11434/v1".to_string(),
        default_model: Some("qwen3:8b".to_string()),
        headers: BTreeMap::new(),
        has_api_key: false,
    }
}

/// The configuration a fresh install starts with: the default provider, active.
pub fn default_config() -> ProviderConfig {
    let provider = default_provider();
    ProviderConfig {
        active_provider_id: Some(provider.id.clone()),
        providers: vec![provider],
    }
}

/// Makes an id from a display name: lowercase, dashes, no spaces.
///
/// Ids end up in the credential store as account names, so they have to be
/// predictable: the same name always maps to the same credential.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "provider".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base: &str) -> Provider {
        Provider {
            id: "p".to_string(),
            name: "P".to_string(),
            base_url: base.to_string(),
            default_model: None,
            headers: BTreeMap::new(),
            has_api_key: false,
        }
    }

    #[test]
    fn the_endpoints_follow_the_openai_compatible_layout() {
        let p = provider("https://openrouter.ai/api/v1");
        assert_eq!(p.models_url(), "https://openrouter.ai/api/v1/models");
        assert_eq!(
            p.chat_url(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    /// A trailing slash would produce a double slash, and some gateways
    /// answer that with a 404 that looks like a wrong base URL.
    #[test]
    fn a_trailing_slash_does_not_produce_a_double_slash() {
        let p = provider("https://openrouter.ai/api/v1/");
        assert_eq!(p.models_url(), "https://openrouter.ai/api/v1/models");
        assert_eq!(
            p.chat_url(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    #[test]
    fn a_local_runtime_is_the_same_shape_as_a_hosted_one() {
        // The whole design rests on this: no special case for "local".
        let local = provider("http://127.0.0.1:11434/v1");
        let hosted = provider("https://openrouter.ai/api/v1");
        assert!(local.models_url().ends_with("/models"));
        assert!(hosted.models_url().ends_with("/models"));
    }

    #[test]
    fn a_dangling_active_provider_is_reported_as_no_active_one() {
        let config = ProviderConfig {
            active_provider_id: Some("gone".to_string()),
            ..Default::default()
        };
        assert!(config.active().is_none());
    }

    #[test]
    fn removing_the_active_provider_clears_the_active_id() {
        let mut config = default_config();
        assert!(config.remove("ollama"));
        assert!(config.active_provider_id.is_none());
        assert!(config.providers.is_empty());
    }

    #[test]
    fn upsert_replaces_in_place_and_keeps_the_order() {
        let mut config = default_config();
        config.upsert(Provider {
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            default_model: None,
            headers: BTreeMap::new(),
            has_api_key: true,
        });
        config.upsert(Provider {
            id: "openrouter".to_string(),
            name: "OpenRouter 改名".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            default_model: Some("x".to_string()),
            headers: BTreeMap::new(),
            has_api_key: true,
        });
        assert_eq!(config.providers.len(), 2);
        assert_eq!(config.providers[1].name, "OpenRouter 改名");
    }

    #[test]
    fn a_slug_is_stable_and_safe_for_a_credential_account() {
        assert_eq!(slugify("OpenRouter"), "openrouter");
        assert_eq!(slugify("  My  Provider! "), "my-provider");
        assert_eq!(slugify("中文"), "provider");
        assert_eq!(slugify(""), "provider");
    }

    /// The config file must never hold the key itself — only whether one
    /// exists. A key in a plain file is a key in every backup.
    #[test]
    fn the_serialized_config_holds_no_key() {
        let config = default_config();
        let body = serde_json::to_string(&config).expect("serialize");
        assert!(body.contains("\"hasApiKey\":false"));
        assert!(!body.contains("\"apiKey\""));
        assert!(!body.contains("sk-"));
    }

    #[test]
    fn a_missing_config_file_is_an_empty_config_not_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "repotory-agent-provider-test-{}",
            std::process::id()
        ));
        let path = dir.join("providers.json");
        let _ = std::fs::remove_file(&path);
        let store = ProviderStore::new(path.clone());
        let (config, problem) = store.load();
        assert!(config.providers.is_empty());
        assert!(problem.is_none());
    }

    #[test]
    fn a_saved_config_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "repotory-agent-provider-roundtrip-{}",
            std::process::id()
        ));
        let path = dir.join("providers.json");
        let store = ProviderStore::new(path.clone());
        let mut config = default_config();
        config.upsert(Provider {
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            default_model: Some("anthropic/claude-sonnet-4".to_string()),
            headers: BTreeMap::from([("X-Title".to_string(), "Repotory Agent".to_string())]),
            has_api_key: true,
        });
        store.save(&config).expect("save");

        let (loaded, problem) = store.load();
        assert!(problem.is_none(), "{problem:?}");
        assert_eq!(loaded.providers.len(), 2);
        assert_eq!(loaded.active_provider_id.as_deref(), Some("ollama"));
        assert_eq!(
            loaded
                .find("openrouter")
                .and_then(|p| p.default_model.clone()),
            Some("anthropic/claude-sonnet-4".to_string())
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A corrupt file must be reported, not silently replaced by an empty
    /// configuration: the user's provider list is in it.
    #[test]
    fn a_corrupt_config_file_is_reported() {
        let dir = std::env::temp_dir().join(format!(
            "repotory-agent-provider-corrupt-{}",
            std::process::id()
        ));
        let path = dir.join("providers.json");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&path, "{not json").expect("write");
        let store = ProviderStore::new(path.clone());
        let (config, problem) = store.load();
        assert!(config.providers.is_empty());
        assert!(problem.is_some(), "a broken config file must be reported");
        let _ = std::fs::remove_file(&path);
    }
}
