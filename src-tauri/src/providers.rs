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
//     to `providers.json` in the app config directory — they are
//     configuration and they are meant to be readable;
//   * the API keys go to `secrets.json` in the SAME directory, written with
//     owner-only permissions (0600) at creation time.
//
// Why the keys are not in the OS credential store any more.
//
// They used to be (macOS Keychain, Windows Credential Manager, Linux Secret
// Service). On macOS that turned into a prompt for the LOGIN KEYCHAIN PASSWORD
// on every launch: a keychain item's access control list is keyed to the code
// signature of the app that created it, and this app ships ad-hoc signed
// (`codesign -s -`, because there is no Apple Developer ID in the release
// workflow). An ad-hoc signature is a hash of the exact binary, so every new
// build is a different application as far as the keychain is concerned — the
// "always allow" answer cannot stick, and the user is asked again.
//
// A credential store that asks for the account password every single time is
// not protecting the secret, it is training the user to dismiss the dialog.
// The trade-off taken here is stated plainly instead of hidden: a key in a
// file is a key in every backup and every sync folder that file reaches. In
// exchange it is read without a prompt, it can be inspected and deleted with
// `open` / `rm`, and it is protected by the same 0600 bit as the rest of the
// user's per-app state. A user who does not want any key on disk runs the
// local Ollama, which needs none.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file holding the provider API keys, next to `providers.json`.
const SECRETS_FILE: &str = "secrets.json";

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
    /// Whether an API key is on file for this provider. A provider without a
    /// key is legitimate — a local Ollama needs none.
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
    /// (the keys live in `secrets.json` beside it), but a list of the
    /// providers a person uses is nobody else's business either.
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

/// Writes a file that is owner-only FROM THE FIRST BYTE.
///
/// `fs::write` creates with the process umask (0644 on a default macOS
/// account) and only a later `chmod` narrows it, which leaves a window where
/// any local user can read the file. These files hold API keys, so the mode
/// is applied at creation instead of after it.
fn write_owner_only(path: &Path, body: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| format!("the file could not be opened: {err}"))?;
        file.write_all(body.as_bytes())
            .map_err(|err| format!("the file could not be written: {err}"))
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, body).map_err(|err| format!("the file could not be written: {err}"))
    }
}

impl ProviderStore {
    /// The API keys file: same directory as the config, never the same file.
    fn secrets_path(&self) -> PathBuf {
        self.path.with_file_name(SECRETS_FILE)
    }

    fn read_secrets(&self) -> Result<BTreeMap<String, String>, String> {
        let raw = match std::fs::read_to_string(self.secrets_path()) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(err) => return Err(format!("the API keys could not be read: {err}")),
        };
        serde_json::from_str(&raw)
            .map_err(|err| format!("the API keys file is not valid JSON: {err}"))
    }

    fn write_secrets(&self, secrets: &BTreeMap<String, String>) -> Result<(), String> {
        let path = self.secrets_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("the config directory could not be created: {err}"))?;
        }
        let body = serde_json::to_string_pretty(secrets)
            .map_err(|err| format!("the API keys could not be serialized: {err}"))?;
        write_owner_only(&path, &body)
    }

    /// Stores an API key for a provider.
    pub fn store_api_key(&self, provider_id: &str, key: &str) -> Result<(), String> {
        let mut secrets = self.read_secrets()?;
        secrets.insert(provider_id.to_string(), key.to_string());
        self.write_secrets(&secrets)
    }

    /// Reads a provider's API key.
    ///
    /// A missing key is not an error: it means "this provider has no key",
    /// which is a normal state for a local runtime.
    pub fn load_api_key(&self, provider_id: &str) -> Result<Option<String>, String> {
        Ok(self.read_secrets()?.get(provider_id).cloned())
    }

    /// Removes a provider's API key. Removing what is not there is a success:
    /// the caller asked for the key to be gone, and it is.
    pub fn delete_api_key(&self, provider_id: &str) -> Result<(), String> {
        let mut secrets = self.read_secrets()?;
        if secrets.remove(provider_id).is_none() {
            return Ok(());
        }
        self.write_secrets(&secrets)
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
/// Ids end up as the keys of the entries in `secrets.json`, so they have to
/// be predictable: the same name always maps to the same stored key.
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
            headers: BTreeMap::from([("X-Title".to_string(), "Warden".to_string())]),
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

    /// A scratch store whose directory is unique to the test that asked for
    /// it: these write real files, and two tests sharing one path would pass
    /// or fail depending on the order they ran in.
    fn scratch_store(name: &str) -> (PathBuf, ProviderStore) {
        let dir = std::env::temp_dir().join(format!(
            "repotory-agent-secrets-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (dir.clone(), ProviderStore::new(dir.join("providers.json")))
    }

    #[test]
    fn a_stored_key_round_trips_and_goes_away_when_deleted() {
        let (dir, store) = scratch_store("roundtrip");
        assert_eq!(store.load_api_key("openrouter").expect("load"), None);

        store
            .store_api_key("openrouter", "sk-or-123")
            .expect("store");
        assert_eq!(
            store.load_api_key("openrouter").expect("load"),
            Some("sk-or-123".to_string())
        );

        // A second provider does not overwrite the first: the file is a map.
        store.store_api_key("deepseek", "sk-ds-456").expect("store");
        assert_eq!(
            store.load_api_key("openrouter").expect("load"),
            Some("sk-or-123".to_string())
        );

        store.delete_api_key("openrouter").expect("delete");
        assert_eq!(store.load_api_key("openrouter").expect("load"), None);
        assert_eq!(
            store.load_api_key("deepseek").expect("load"),
            Some("sk-ds-456".to_string())
        );

        // Deleting what is not there is a success, not an error.
        store.delete_api_key("never-existed").expect("idempotent");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The keys must not land in the configuration file: that file is meant to
    /// be readable, and it is the one people paste into an issue.
    #[test]
    fn the_keys_are_written_beside_the_config_not_into_it() {
        let (dir, store) = scratch_store("separate");
        let mut config = default_config();
        config.upsert(Provider {
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            default_model: None,
            headers: BTreeMap::new(),
            has_api_key: true,
        });
        store.save(&config).expect("save");
        store
            .store_api_key("openrouter", "sk-or-secret")
            .expect("store");

        let config_body = std::fs::read_to_string(dir.join("providers.json")).expect("read");
        assert!(!config_body.contains("sk-or-secret"));
        let secrets_body = std::fs::read_to_string(dir.join(SECRETS_FILE)).expect("read");
        assert!(secrets_body.contains("sk-or-secret"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 0600 at creation: an API key readable by every local account is a key
    /// that has already leaked.
    #[cfg(unix)]
    #[test]
    fn the_keys_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, store) = scratch_store("mode");
        store
            .store_api_key("openrouter", "sk-or-123")
            .expect("store");
        let mode = std::fs::metadata(dir.join(SECRETS_FILE))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the keys file must be 0600, got {mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A half-written or hand-edited keys file must be reported, not read as
    /// "this provider has no key" — that would send the user looking at their
    /// provider settings instead of at the file.
    #[test]
    fn a_corrupt_keys_file_is_reported() {
        let (dir, store) = scratch_store("corrupt");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(SECRETS_FILE), "{not json").expect("write");
        assert!(store.load_api_key("openrouter").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
