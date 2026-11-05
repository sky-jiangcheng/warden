// Repotory Agent — a local copilot that reads the Repotory memory layer.
//
// The copilot is the window; Repotory is the memory. Everything this window
// knows comes from the seams in this crate (the model providers, the
// memory-layer connection and the wake-up path), and every future capability
// — chat, reading a project, running a command, the memory panels — sits on
// top of them rather than beside them.
// `mcp_client` is public because the end-to-end test in `tests/` drives it
// against a fake server. The other modules stay private: nothing outside
// this crate has a reason to reach into them.
mod binaries;
pub mod llm;
pub mod mcp_client;
pub mod memory_layer;
mod provider_commands;
pub mod providers;
mod tray;

use serde::Serialize;
use tauri::Manager;

use std::collections::BTreeMap;

use tauri::ipc::Channel;

use crate::llm::{ChatMessage, LlmClient, ModelInfo};
use crate::provider_commands::{
    ChatChunk, ConnectionTest, ProviderInput, ProviderList, ProviderView, describe_llm_error,
};
use crate::providers::{
    Provider, ProviderStore, delete_api_key, load_api_key, slugify, store_api_key,
};

/// What the first screen needs: which model providers are configured, whether
/// the memory layer is installed AND answering, and whether the wake-up path
/// is in place.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellStatus {
    pub providers: provider_commands::ProviderList,
    pub memory: memory_layer::MemoryLayerStatus,
    pub tray: TrayStatus,
}

/// Whether the tray icon — the copilot's wake-up entry — is installed.
///
/// It is reported on the first screen because macOS offers no way to check
/// another process's status items from the outside, and a user who cannot
/// find the tray cannot tell "not installed" from "installed but I did not
/// notice". The app can always answer for itself.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayStatus {
    pub installed: bool,
}

/// The first screen's status.
///
/// It is async because connecting to the memory layer starts a process and
/// does a handshake. A synchronous command would block the IPC thread for as
/// long as the server takes to answer — up to the client's request timeout —
/// and the whole window would freeze with it.
#[tauri::command]
async fn shell_status<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<ShellStatus, String> {
    // The session is fetched through the handle rather than taken as a
    // `State<'_, _>` argument: an async command that holds a reference
    // parameter cannot satisfy Tauri's `'static` bound on the future.
    let session = app.state::<mcp_client::McpSession>();
    let store = app.state::<ProviderStore>();
    let (config, problem) = provider_commands::config_or_default(&store);
    Ok(ShellStatus {
        providers: provider_commands::ProviderList {
            providers: config
                .providers
                .iter()
                .map(|p| {
                    provider_commands::ProviderView::of(p, config.active_provider_id.as_deref())
                })
                .collect(),
            active_provider_id: config.active_provider_id.clone(),
            problem,
        },
        memory: memory_layer::status(&session).await,
        tray: TrayStatus {
            installed: tray::is_installed(&app),
        },
    })
}

/// Calls one memory-layer tool and returns its text.
///
/// This is the data path the copilot is for: the frontend asks for a tool by
/// name, the Rust side owns the connection, and the answer comes back as
/// text. The frontend never sees a process, a pipe or a port.
///
/// The tool NAME comes from the caller, but the tool SET does not: the server
/// decides which tools exist, and the frontend gets that list from
/// `shell_status`. A name the server does not have comes back as an error
/// from the server, which is the honest answer.
#[tauri::command]
async fn mcp_call_tool<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    name: String,
    arguments: serde_json::Value,
) -> Result<String, String> {
    app.state::<mcp_client::McpSession>()
        .call_tool(&name, arguments)
        .await
        .map_err(|err| err.to_string())
}

// The provider commands.
//
// They live here rather than in `provider_commands.rs` because Tauri's
// `generate_handler!` names the generated `__cmd__<name>` macros, and those
// are private to the module that defines them. The logic behind them is in
// `provider_commands.rs`; the shell of each command is here, next to the
// handler list that reaches them.

/// Lists the configured providers.
#[tauri::command]
fn provider_list(store: tauri::State<'_, ProviderStore>) -> ProviderList {
    let (config, problem) = store.load();
    ProviderList {
        providers: config
            .providers
            .iter()
            .map(|p| ProviderView::of(p, config.active_provider_id.as_deref()))
            .collect(),
        active_provider_id: config.active_provider_id.clone(),
        problem,
    }
}

/// Saves a provider: its configuration, and its key when one is given.
#[tauri::command]
fn provider_save(
    store: tauri::State<'_, ProviderStore>,
    input: ProviderInput,
) -> Result<ProviderList, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err("the provider needs a name".to_string());
    }
    let base_url = input.base_url.trim().to_string();
    if base_url.is_empty() {
        return Err("the provider needs a base URL".to_string());
    }
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(format!(
            "the base URL must start with http:// or https:// — \"{base_url}\" does not"
        ));
    }

    let (mut config, _) = store.load();

    // A new provider takes its id from its name; an existing one keeps the
    // id it has, because that id is the credential store's account name and
    // changing it would orphan the stored key.
    let id = if input.id.trim().is_empty() {
        let base = slugify(&name);
        let mut candidate = base.clone();
        let mut suffix = 2;
        while config.find(&candidate).is_some() {
            candidate = format!("{base}-{suffix}");
            suffix += 1;
        }
        candidate
    } else {
        input.id.trim().to_string()
    };

    let mut has_api_key = config.find(&id).map(|p| p.has_api_key).unwrap_or(false);
    if input.save_api_key {
        match input.api_key.as_deref() {
            Some(key) if !key.trim().is_empty() => {
                store_api_key(&id, key.trim())?;
                has_api_key = true;
            }
            _ => {
                // An empty key with "save" checked means "remove it".
                delete_api_key(&id)?;
                has_api_key = false;
            }
        }
    }

    config.upsert(Provider {
        id: id.clone(),
        name,
        base_url,
        default_model: input.default_model.filter(|m| !m.trim().is_empty()),
        headers: input.headers,
        has_api_key,
    });

    // The first provider becomes the active one — a fresh install with one
    // provider should be usable without a second click.
    if config.active_provider_id.is_none() {
        config.active_provider_id = Some(id);
    }

    store.save(&config)?;
    Ok(ProviderList {
        providers: config
            .providers
            .iter()
            .map(|p| ProviderView::of(p, config.active_provider_id.as_deref()))
            .collect(),
        active_provider_id: config.active_provider_id.clone(),
        problem: None,
    })
}

/// Removes a provider and its stored key.
#[tauri::command]
fn provider_remove(
    store: tauri::State<'_, ProviderStore>,
    id: String,
) -> Result<ProviderList, String> {
    let (mut config, _) = store.load();
    if !config.remove(&id) {
        return Err(format!("no provider named \"{id}\""));
    }
    // The key is removed too: a credential nobody references is a secret
    // nobody rotates.
    let _ = delete_api_key(&id);
    store.save(&config)?;
    Ok(ProviderList {
        providers: config
            .providers
            .iter()
            .map(|p| ProviderView::of(p, config.active_provider_id.as_deref()))
            .collect(),
        active_provider_id: config.active_provider_id.clone(),
        problem: None,
    })
}

/// Chooses which provider the chat uses.
#[tauri::command]
fn provider_set_active(
    store: tauri::State<'_, ProviderStore>,
    id: String,
) -> Result<ProviderList, String> {
    let (mut config, _) = store.load();
    if config.find(&id).is_none() {
        return Err(format!("no provider named \"{id}\""));
    }
    config.active_provider_id = Some(id);
    store.save(&config)?;
    Ok(ProviderList {
        providers: config
            .providers
            .iter()
            .map(|p| ProviderView::of(p, config.active_provider_id.as_deref()))
            .collect(),
        active_provider_id: config.active_provider_id.clone(),
        problem: None,
    })
}

/// Pulls the model list from a provider.
///
/// The provider may be one that is already configured, or one the user is
/// still setting up — the latter matters, because the model list is how a
/// user finds out whether the base URL and the key are right before saving.
#[tauri::command]
async fn provider_fetch_models(
    store: tauri::State<'_, ProviderStore>,
    base_url: Option<String>,
    id: Option<String>,
    api_key: Option<String>,
) -> Result<Vec<ModelInfo>, String> {
    let (config, _) = store.load();

    let provider = match (&id, &base_url) {
        (Some(id), _) if !id.trim().is_empty() => config
            .find(id.trim())
            .cloned()
            .ok_or_else(|| format!("no provider named \"{id}\""))?,
        (_, Some(url)) if !url.trim().is_empty() => Provider {
            id: "probe".to_string(),
            name: "probe".to_string(),
            base_url: url.trim().to_string(),
            default_model: None,
            headers: BTreeMap::new(),
            has_api_key: false,
        },
        _ => {
            let active = config
                .active()
                .ok_or_else(|| "no provider is configured yet".to_string())?;
            active.clone()
        }
    };

    // A key typed into the form wins over the stored one: the user is
    // testing a key they have not saved yet.
    let key = match api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => Some(key.to_string()),
        None => load_api_key(&provider.id)?,
    };

    let client = LlmClient::new(&provider, key).map_err(|err| err.to_string())?;
    client.list_models().await.map_err(|err| err.to_string())
}

/// Tests that a provider really works.
///
/// The test sends an actual chat request. A ping would only prove that
/// something listens on that host; the question the user is asking is
/// "will this provider serve my conversation", and only a real request
/// answers it — it proves the key is accepted, the model exists, and the
/// provider can generate.
#[tauri::command]
async fn provider_test_connection(
    store: tauri::State<'_, ProviderStore>,
    base_url: Option<String>,
    id: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
) -> Result<ConnectionTest, String> {
    let (config, _) = store.load();

    let stored = match (&id, &base_url) {
        (Some(id), _) if !id.trim().is_empty() => config.find(id.trim()).cloned(),
        _ => None,
    };

    let provider = match (stored, &base_url) {
        (Some(provider), _) => provider,
        (None, Some(url)) if !url.trim().is_empty() => Provider {
            id: "probe".to_string(),
            name: "probe".to_string(),
            base_url: url.trim().to_string(),
            default_model: None,
            headers: BTreeMap::new(),
            has_api_key: false,
        },
        _ => {
            let active = config
                .active()
                .ok_or_else(|| "no provider is configured yet".to_string())?;
            active.clone()
        }
    };

    let key = match api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => Some(key.to_string()),
        None => load_api_key(&provider.id)?,
    };

    // Which model to test: what the user asked for, else the provider's
    // default, else the first model it advertises.
    let client = LlmClient::new(&provider, key).map_err(|err| err.to_string())?;

    let model = match model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => model.to_string(),
        None => match provider.default_model.clone() {
            Some(model) => model,
            None => {
                let models = client.list_models().await.map_err(|err| err.to_string())?;
                models
                    .first()
                    .map(|m| m.id.clone())
                    .ok_or_else(|| "this provider advertises no models".to_string())?
            }
        },
    };

    let started = std::time::Instant::now();
    let outcome = client
        .chat(
            &model,
            &[ChatMessage::user("Reply with the single word: ready")],
        )
        .await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match outcome {
        Ok(answer) => Ok(ConnectionTest {
            ok: true,
            detail: answer.trim().to_string(),
            model,
            latency_ms,
        }),
        Err(err) => Ok(ConnectionTest {
            ok: false,
            detail: describe_llm_error(&err),
            model,
            latency_ms,
        }),
    }
}

/// Sends a chat request and streams the answer back.
///
/// The answer goes through a `Channel` rather than a return value: a return
/// value would make the frontend wait for the whole answer, and the point
/// of streaming is that the text appears while it is written.
#[tauri::command]
async fn chat_send(
    store: tauri::State<'_, ProviderStore>,
    channel: Channel<ChatChunk>,
    message: String,
    model: Option<String>,
    provider_id: Option<String>,
) -> Result<(), String> {
    let (config, _) = store.load();
    // The chat form can point at any provider, not only the active one — the
    // user picks the model in the same place they pick the provider.
    let provider = match provider_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => config
            .find(id)
            .cloned()
            .ok_or_else(|| format!("no provider named \"{id}\""))?,
        None => config
            .active()
            .ok_or_else(|| "no provider is configured yet".to_string())?
            .clone(),
    };

    let key = load_api_key(&provider.id)?;
    let client = LlmClient::new(&provider, key).map_err(|err| err.to_string())?;

    let model = match model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => model.to_string(),
        None => provider
            .default_model
            .clone()
            .ok_or_else(|| "this provider has no default model — pick one first".to_string())?,
    };

    let messages = vec![ChatMessage::user(message)];
    let mut answer = String::new();

    let outcome = client
        .chat_stream(&model, &messages, |delta| {
            answer.push_str(delta);
            let chunk = ChatChunk {
                delta: delta.to_string(),
                text: answer.clone(),
                done: false,
                error: None,
            };
            // A send failure means the window is gone; there is nobody left
            // to tell, and the request itself is already on its way.
            let _ = channel.send(chunk);
        })
        .await;

    match outcome {
        Ok(_) => {
            let _ = channel.send(ChatChunk {
                delta: String::new(),
                text: answer,
                done: true,
                error: None,
            });
            Ok(())
        }
        Err(err) => {
            let _ = channel.send(ChatChunk {
                delta: String::new(),
                text: answer,
                done: true,
                error: Some(describe_llm_error(&err)),
            });
            Err(describe_llm_error(&err))
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(mcp_client::McpSession::default())
        .setup(|app| {
            // The tray is the copilot's wake-up entry (ADR-0047 decision 5).
            // It is installed here, in the setup hook, so it exists from the
            // first frame — a tray that shows up a moment late reads as a
            // broken feature.
            tray::install(app)?;

            // The provider configuration lives in the app config directory.
            // The store is managed so the commands can reach it through the
            // app handle, the same way the MCP session is.
            let store = provider_commands::store_for(app.handle())?;
            app.manage(store);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            shell_status,
            mcp_call_tool,
            provider_list,
            provider_save,
            provider_remove,
            provider_set_active,
            provider_fetch_models,
            provider_test_connection,
            chat_send,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Repotory Agent");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend reads camelCase keys; a rename on the Rust side would
    /// otherwise only show up as an empty first screen at runtime.
    ///
    /// The payload carries NO hardcoded tool list. It used to, and the
    /// hardcoded copy had already drifted from Repotory's real MCP surface
    /// (7 names listed, 18 registered). The tool names belong to the server:
    /// they are fetched over MCP's `tools/list` — see `memory_layer.rs` for
    /// the full reasoning — so the payload must not carry any unless a server
    /// actually answered.
    ///
    /// Without a server installed there is no list, which is exactly what this
    /// asserts. The positive case (a list that came over the wire) is covered
    /// by the MCP client's own tests and by the end-to-end run against the
    /// real `repotory-mcp`.
    #[tokio::test]
    async fn shell_status_serves_the_ui_contract() {
        let app = tauri::test::mock_app();
        app.manage(mcp_client::McpSession::default());
        app.manage(ProviderStore::new(
            std::env::temp_dir()
                .join(format!(
                    "repotory-agent-shell-status-{}",
                    std::process::id()
                ))
                .join("providers.json"),
        ));
        let handle = app.handle().clone();
        let payload = serde_json::to_string(&shell_status(handle).await).expect("serialize");
        assert!(payload.contains("\"providers\""));
        assert!(payload.contains("\"memory\""));
        assert!(payload.contains("\"available\""));
        assert!(payload.contains("\"path\""));
        assert!(payload.contains("\"connected\""));
        assert!(payload.contains("\"tools\""));
        assert!(payload.contains("\"tray\""));
        assert!(payload.contains("\"installed\""));
        println!("{payload}");
    }

    /// The tray is the copilot's only wake-up path (ADR-0047 decision 5), and
    /// macOS offers no way to check another process's status items from the
    /// outside — so "is the tray really installed?" has to be answerable
    /// from inside the app, and this is that answer.
    ///
    /// Only the negative case is asserted here. The positive one cannot be:
    /// AppKit menus may only be created on the main thread, and the test
    /// runtime runs `run_on_main_thread` closures inline on the test thread
    /// (`tauri::test`'s `RuntimeContext::send_message` executes a task
    /// immediately when the runtime is not running). Building the menu there
    /// panics inside `muda`, not inside this crate.
    ///
    /// So the positive case is verified where it can be: the first screen
    /// reports `tray.installed`, and the tray module logs an explicit
    /// `[tray] the tray icon could not be installed` line if the install
    /// fails. A missing tray is visible, not silent.
    #[test]
    fn tray_status_is_false_before_the_tray_is_installed() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        assert!(
            !tray::is_installed(&handle),
            "a fresh app has no tray icon yet"
        );
    }
}
