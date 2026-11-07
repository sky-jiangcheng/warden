// Warden — a local copilot that reads the Cinder memory layer.
//
// The copilot is the window; Cinder is the memory. Everything this window
// knows comes from the seams in this crate (the model providers, the
// memory-layer connection and the wake-up path), and every future capability
// — chat, reading a project, running a command, the memory panels — sits on
// top of them rather than beside them.
// `mcp_client` is public because the end-to-end test in `tests/` drives it
// against a fake server. The other modules stay private: nothing outside
// this crate has a reason to reach into them.
mod binaries;
pub mod conversations;
pub mod llm;
pub mod mcp_client;
pub mod memory_layer;
mod provider_commands;
pub mod providers;
pub mod repos;
mod tray;

use serde::Serialize;
use tauri::{Emitter, Manager};

use tauri::ipc::Channel;

use crate::llm::{ChatMessage, LlmClient, ModelInfo};
use crate::provider_commands::{
    ChatChunk, ConnectionTest, ProviderInput, ProviderList, ProviderView, describe_llm_error,
};
use crate::providers::{
    Provider, ProviderStore, delete_api_key, load_api_key, slugify, store_api_key,
};
use crate::repos::{RepoEntry, RepoScan, RepoStore};
use tauri_plugin_notification::NotificationExt;

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

// The repository commands.
//
// The copilot finds the repositories on this machine by itself: it scans the
// roots the user configured, shows what it found, and remembers what the
// user decided about each one.
//
// None of this goes through Cinder. A copilot that needs another
// product's database to know what is on this disk is not independent of
// it, and the whole point of this product is that it stands on its own.

/// Scans the configured roots and reports what is there.
///
/// The bodies of these commands live in `repos.rs` so that the end-to-end
/// test can drive the very same functions the IPC layer calls. A test that
/// re-implements the command would only be testing itself.
#[tauri::command]
fn repo_scan(store: tauri::State<'_, RepoStore>) -> RepoScan {
    let scan = repos::scan_now(&store);
    // The scan is logged because it is the one thing that happens without
    // the user asking: the window opens, the disk is walked, and a list
    // appears. If that list is wrong, the log is where the answer is.
    eprintln!(
        "[repos] {} repositories under {} roots, {} new",
        scan.repos.len(),
        scan.roots.len(),
        scan.new_repos.len()
    );
    scan
}

/// Adds a scan root and reports the new scan.
#[tauri::command]
fn repo_roots_add(store: tauri::State<'_, RepoStore>, path: String) -> Result<RepoScan, String> {
    repos::add_root_and_scan(&store, &path)
}

/// Removes a scan root and reports the new scan.
#[tauri::command]
fn repo_roots_remove(store: tauri::State<'_, RepoStore>, path: String) -> Result<RepoScan, String> {
    repos::remove_root_and_scan(&store, &path)
}

/// Follows or unfollows a repository.
///
/// Following a repository is the user's answer to "is this one of yours?" —
/// the copilot stops announcing it and starts treating it as the user's
/// work.
#[tauri::command]
fn repo_set_favorite(
    store: tauri::State<'_, RepoStore>,
    path: String,
    favorite: bool,
) -> Result<RepoScan, String> {
    repos::set_favorite_and_scan(&store, &path, favorite)
}

/// Marks a repository as "not this one".
///
/// This is the other half of the same answer, and it matters as much: a
/// dismissed repository is never announced again. Without it, saying "no"
/// once would still be asked again on the next scan.
#[tauri::command]
fn repo_dismiss(store: tauri::State<'_, RepoStore>, path: String) -> Result<RepoScan, String> {
    repos::dismiss_and_scan(&store, &path)
}

/// How often the background scan looks for new repositories.
const RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// The notification action that brings the window back.
const ACTION_NEW_REPOS: &str = "repos-new";

/// Avoid re-announcing unchanged discoveries every five minutes. This is
/// session-local presentation state; favourites/dismissals stay in RepoStore.
#[derive(Default)]
struct AnnouncementState(std::sync::Mutex<std::collections::HashSet<String>>);

impl AnnouncementState {
    fn unseen(&self, repos: &[RepoEntry]) -> Vec<RepoEntry> {
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        repos
            .iter()
            .filter(|repo| seen.insert(repo.path.clone()))
            .cloned()
            .collect()
    }
}

/// Announces repositories the user has not dealt with yet.
///
/// Two channels, one per situation. When the window is in front, the
/// window asks — a system notification on top of a screen the user is
/// already reading is noise. When the copilot is in the background, a
/// system notification is the only thing that can reach the user at all.
fn announce_new_repos<R: tauri::Runtime>(app: &tauri::AppHandle<R>, new_repos: &[RepoEntry]) {
    let new_repos = app.state::<AnnouncementState>().unseen(new_repos);
    if new_repos.is_empty() {
        return;
    }

    let foreground = app
        .get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);

    if foreground {
        eprintln!(
            "[repos] {} new repositor{} in the foreground",
            new_repos.len(),
            if new_repos.len() == 1 { "y" } else { "ies" }
        );
        let _ = app.emit("repos:new", new_repos);
        return;
    }

    let title = if new_repos.len() == 1 {
        "发现一个新仓库".to_string()
    } else {
        format!("发现 {} 个新仓库", new_repos.len())
    };
    let body = new_repos
        .iter()
        .map(|repo| repo.name.as_str())
        .collect::<Vec<_>>()
        .join("、");

    // A failed notification is logged rather than swallowed: the user
    // would otherwise see "nothing happened" and have no way to tell that
    // the copilot found a repository and failed to say so.
    if let Err(err) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .action_type_id(ACTION_NEW_REPOS)
        .show()
    {
        eprintln!("[notify] the notification could not be shown: {err}");
    }
}

/// Keeps looking for repositories while nobody is looking at the screen.
///
/// This is what makes the copilot a resident rather than a screen: the
/// window can be closed for hours and the scan still runs. The first scan
/// is not here — the window asks for it when it opens, because a window
/// that is not listening yet cannot be told about a new repository.
fn spawn_background_scan<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    store_path: std::path::PathBuf,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(RESCAN_INTERVAL).await;
            let path = store_path.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                let store = RepoStore::new(path);
                let (list, _) = store.load();
                repos::scan_into(&list.roots, &list, None)
            })
            .await
            {
                Ok(scan) => announce_new_repos(&app, &scan.new_repos),
                Err(error) => eprintln!("[repos] background scan failed: {error}"),
            }
        }
    });
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

    let provider =
        crate::provider_commands::probe_target(&config, id.as_deref(), base_url.as_deref())?;

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

    let provider =
        crate::provider_commands::probe_target(&config, id.as_deref(), base_url.as_deref())?;

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
async fn chat_send<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    channel: Channel<ChatChunk>,
    message: String,
    model: Option<String>,
    provider_id: Option<String>,
    thread_id: Option<String>,
    message_id: String,
) -> Result<(), String> {
    conversations::send(
        &app.state::<mcp_client::McpSession>(),
        &app.state::<conversations::GenerationState>(),
        &app.state::<ProviderStore>(),
        conversations::ChatRequest {
            message,
            model,
            provider_id,
            thread_id,
            message_id,
        },
        |chunk| {
            let _ = channel.send(chunk);
        },
    )
    .await
}

#[tauri::command]
async fn conversation_list<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: Option<String>,
) -> Result<serde_json::Value, String> {
    conversations::list(&app.state::<mcp_client::McpSession>(), path).await
}

#[tauri::command]
async fn conversation_create<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    title: String,
    path: Option<String>,
) -> Result<serde_json::Value, String> {
    conversations::create(&app.state::<mcp_client::McpSession>(), id, title, path).await
}

#[tauri::command]
async fn conversation_read<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    thread_id: String,
) -> Result<serde_json::Value, String> {
    conversations::read(&app.state::<mcp_client::McpSession>(), thread_id).await
}

#[tauri::command]
async fn conversation_rename<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    thread_id: String,
    title: String,
) -> Result<serde_json::Value, String> {
    conversations::rename(&app.state::<mcp_client::McpSession>(), thread_id, title).await
}

#[tauri::command]
async fn workspace_metrics<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: Option<String>,
) -> Result<serde_json::Value, String> {
    conversations::metrics(&app.state::<mcp_client::McpSession>(), path).await
}

#[tauri::command]
fn companion_show_chat<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    tray::show_surface(&app, "main", false)
}

#[tauri::command]
fn companion_hide_chat<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    tray::hide_surface(&app, "main")
}

#[tauri::command]
fn companion_hide<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    tray::hide_surface(&app, "companion")
}

#[tauri::command]
fn window_drag<R: tauri::Runtime>(window: tauri::WebviewWindow<R>) -> Result<(), String> {
    tray::check_surface(window.label())?;
    window.start_dragging().map_err(|e| e.to_string())
}

/// The builder every entry point uses.
///
/// It is a function, not a chain inside `run()`, so that the test can
/// build the same app the production code builds. A plugin that is
/// depended on but not registered here panics at the first call — and the
/// panic only happens in the running app, because `mock_app` never runs
/// the setup hook. Sharing this builder is what lets the test see it.
fn builder<R: tauri::Runtime>() -> tauri::Builder<R> {
    // `Builder::default()` is only implemented for `Builder<Wry>` when the
    // wry feature is on, so it would pin the runtime to the desktop one.
    // `Builder::<R>::new()` is the generic constructor.
    tauri::Builder::<R>::new()
        .plugin(tauri_plugin_opener::init())
        // The notification plugin has to be registered here, not merely
        // depended on: `app.notification()` reads the plugin's managed
        // state, and calling it without the plugin panics with "state()
        // called before manage()". A dependency in Cargo.toml and a
        // permission in the capability file are not the same thing as a
        // registered plugin.
        .plugin(tauri_plugin_notification::init())
        // The page load is logged because a window that shows nothing is
        // indistinguishable from a window that failed to load. In a debug
        // build the window points at the dev server, not at the built
        // files, so "the page is blank" almost always means "the dev
        // server is not running" — and this line is what says so.
        .on_page_load(|_webview, payload| {
            eprintln!("[page] {} {:?}", payload.url(), payload.event());
        })
        .manage(mcp_client::McpSession::default())
        .manage(conversations::GenerationState::default())
        .manage(AnnouncementState::default())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && tray::check_surface(window.label()).is_ok()
            {
                // Keep both webviews and their in-flight chat tasks alive.
                api.prevent_close();
                if let Err(error) = tray::hide_surface(window.app_handle(), window.label()) {
                    eprintln!("[window] {error}");
                    let _ = window.emit("window:error", error);
                }
            }
        })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    builder::<tauri::Wry>()
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

            // The repository watchlist lives beside the provider config.
            let config_dir = app
                .path()
                .app_config_dir()
                .map_err(|err| format!("the app config directory could not be resolved: {err}"))?;
            let repo_path = config_dir.join("repos.json");
            app.manage(RepoStore::new(repo_path.clone()));

            // Clicking the "a new repository appeared" notification brings
            // the window back: the notification is a door into the app, not
            // just a message about it.
            let handle = app.handle().clone();
            app.notification().register_action_types(vec![
                tauri_plugin_notification::ActionType::builder(ACTION_NEW_REPOS).build(),
            ])?;
            app.notification().on_action(move |_performed| {
                tray::show_window(&handle);
            })?;

            spawn_background_scan(app.handle().clone(), repo_path);

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
            conversation_list,
            conversation_create,
            conversation_read,
            conversation_rename,
            workspace_metrics,
            companion_show_chat,
            companion_hide_chat,
            companion_hide,
            window_drag,
            repo_scan,
            repo_roots_add,
            repo_roots_remove,
            repo_set_favorite,
            repo_dismiss,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Warden");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_scan_results_are_announced_only_once() {
        let state = AnnouncementState::default();
        let repo = RepoEntry {
            path: "/repo/one".into(),
            name: "one".into(),
            favorite: false,
            missing: false,
        };
        assert_eq!(state.unseen(std::slice::from_ref(&repo)).len(), 1);
        assert!(state.unseen(&[repo]).is_empty());
        let next = RepoEntry {
            path: "/repo/two".into(),
            name: "two".into(),
            favorite: false,
            missing: false,
        };
        assert_eq!(state.unseen(&[next]).len(), 1);
    }

    /// The frontend reads camelCase keys; a rename on the Rust side would
    /// otherwise only show up as an empty first screen at runtime.
    ///
    /// The payload carries NO hardcoded tool list. It used to, and the
    /// hardcoded copy had already drifted from Cinder's real MCP surface
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

    /// The notification channel has to be registered, not merely depended on.
    ///
    /// `app.notification()` reads the plugin's managed state, and calling
    /// it without the plugin registered panics with "state() called before
    /// manage()". That panic used to happen only in the running app: the
    /// unit tests built their app with `mock_app`, which never runs the
    /// setup hook where the notification channel is wired.
    ///
    /// Building the app through the same `builder()` the production code
    /// uses, and then asking for the notification API, makes that failure
    /// a test failure instead of a crash on the user's screen.
    #[test]
    fn the_notification_channel_is_registered() {
        let app = builder::<tauri::test::MockRuntime>()
            // The macOS default menu is built on the main thread only, and a
            // test runs on a worker thread, so it is turned off here. It has
            // nothing to do with the notification channel.
            .enable_macos_default_menu(false)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("the app builds");

        // Asking for the notification API reads the plugin's managed state.
        // Without the plugin registered this panics with "state() called
        // before manage()", which is exactly the failure the test guards.
        // The notification is not shown: a test has no business popping a
        // banner on the machine that runs it.
        let _channel = app.handle().notification();
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
