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
pub mod exec;
mod git;
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
use crate::providers::{Provider, ProviderStore, slugify};
use crate::repos::{RepoScan, RepoStore};

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
/// Only the read-only tools in `MODEL_TOOLS` may be reached from the webview:
/// a write tool such as `repotory_notes_create` must go through the human
/// confirmation gate, never straight from arbitrary JS. A name outside the
/// whitelist is rejected here before any process is touched; a name the
/// server does not advertise still comes back as the server's honest error.
#[tauri::command]
async fn mcp_call_tool<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    name: String,
    arguments: serde_json::Value,
) -> Result<String, String> {
    if !conversations::MODEL_TOOLS.contains(&name.as_str()) {
        return Err(format!("tool {name} is not available from the webview"));
    }
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

/// Runs a repository disk scan off the IPC thread. The recursive walk is
/// synchronous (see repos.rs), so every read/write-and-rescan command hands
/// it to a blocking thread instead of stalling the async command executor.
async fn run_repo_scan<F>(task: F) -> Result<RepoScan, String>
where
    F: FnOnce() -> Result<RepoScan, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|err| format!("the repository scan could not be completed: {err}"))?
}

/// Scans the configured roots and reports what is there.
///
/// The bodies of these commands live in `repos.rs` so that the end-to-end
/// test can drive the very same functions the IPC layer calls. A test that
/// re-implements the command would only be testing itself.
#[tauri::command]
async fn repo_scan(store: tauri::State<'_, RepoStore>) -> Result<RepoScan, String> {
    let store = store.inner().clone();
    let scan = run_repo_scan(move || Ok(repos::scan_now(&store)))
        .await
        .unwrap_or_else(|problem| RepoScan {
            problem: Some(problem),
            ..Default::default()
        });
    // The scan is logged because it is the one thing that happens without
    // the user asking: the window opens, the disk is walked, and a list
    // appears. If that list is wrong, the log is where the answer is.
    eprintln!(
        "[repos] {} repositories under {} roots, {} new",
        scan.repos.len(),
        scan.roots.len(),
        scan.new_repos.len()
    );
    Ok(scan)
}

/// Adds a scan root and reports the new scan.
#[tauri::command]
async fn repo_roots_add(
    store: tauri::State<'_, RepoStore>,
    path: String,
) -> Result<RepoScan, String> {
    let store = store.inner().clone();
    run_repo_scan(move || repos::add_root_and_scan(&store, &path)).await
}

/// Removes a scan root and reports the new scan.
#[tauri::command]
async fn repo_roots_remove(
    store: tauri::State<'_, RepoStore>,
    path: String,
) -> Result<RepoScan, String> {
    let store = store.inner().clone();
    run_repo_scan(move || repos::remove_root_and_scan(&store, &path)).await
}

/// Follows or unfollows a repository.
///
/// Following a repository is the user's answer to "is this one of yours?" —
/// the copilot stops announcing it and starts treating it as the user's
/// work.
#[tauri::command]
async fn repo_set_favorite(
    store: tauri::State<'_, RepoStore>,
    path: String,
    favorite: bool,
) -> Result<RepoScan, String> {
    let store = store.inner().clone();
    run_repo_scan(move || repos::set_favorite_and_scan(&store, &path, favorite)).await
}

/// Marks a repository as "not this one".
///
/// This is the other half of the same answer, and it matters as much: a
/// dismissed repository is never announced again. Without it, saying "no"
/// once would still be asked again on the next scan.
#[tauri::command]
async fn repo_dismiss(
    store: tauri::State<'_, RepoStore>,
    path: String,
) -> Result<RepoScan, String> {
    let store = store.inner().clone();
    run_repo_scan(move || repos::dismiss_and_scan(&store, &path)).await
}

/// Saves the projects the user selected in the settings list.
#[tauri::command]
async fn repo_set_projects(
    store: tauri::State<'_, RepoStore>,
    paths: Vec<String>,
) -> Result<RepoScan, String> {
    let store = store.inner().clone();
    run_repo_scan(move || repos::set_projects_and_scan(&store, &paths)).await
}

/// How often the background scan looks for new repositories.
const RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// Tells the settings screen that a scan finished, without interrupting the
/// user. Discoveries are never announced: no system notification, no pop-up.
/// A new repository simply appears in the selectable project list the next
/// time the user opens settings (or immediately, if settings are open).
fn publish_scan<R: tauri::Runtime>(app: &tauri::AppHandle<R>, scan: &RepoScan) {
    let _ = app.emit("repos:changed", scan);
}

/// Keeps the project list current while nobody is looking at the screen.
///
/// This is what makes the copilot a resident rather than a screen: the
/// window can be hidden for hours and the scan still runs. Results are
/// published silently — the user decides what to adopt, in the settings
/// list, after configuring a root.
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
                Ok(scan) => publish_scan(&app, &scan),
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
    // id it has, because that id is the key of its entry in the keys file
    // and changing it would orphan the stored key.
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
                store.store_api_key(&id, key.trim())?;
                has_api_key = true;
            }
            _ => {
                // An empty key with "save" checked means "remove it".
                store.delete_api_key(&id)?;
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
    let _ = store.delete_api_key(&id);
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
        None => store.load_api_key(&provider.id)?,
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
        None => store.load_api_key(&provider.id)?,
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
    // The desk pet shows a thinking state even when the chat overlay has
    // been put away mid-answer. The event is broadcast, not tied to a window.
    let _ = app.emit("warden:thinking", true);
    // A dead IPC channel means the webview is gone. Set a cancel flag so the
    // generation loop stops and the answer is never persisted to Cinder.
    let disconnected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&disconnected);
    // The watchlist travels with the request so the system prompt can name
    // it: Warden's adopted projects are a different list from Cinder's
    // starred projects, and a model that only ever sees one of them answers
    // "which repos do I watch" with the wrong one. Read at send time, not
    // cached — saving a selection takes effect on the next message.
    let watched = app.state::<RepoStore>().load().0.favorites;
    let outcome = conversations::send_with_cancel(
        &app.state::<mcp_client::McpSession>(),
        &app.state::<conversations::GenerationState>(),
        &app.state::<ProviderStore>(),
        conversations::ChatRequest {
            message,
            model,
            provider_id,
            thread_id,
            message_id,
            watched,
        },
        move |chunk| {
            if channel.send(chunk).is_err() {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        },
        disconnected,
    )
    .await;
    let _ = app.emit("warden:thinking", false);
    outcome
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
fn companion_show_chat<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    // The pet is a conversation entry: opening from it goes to the chat
    // page and makes the window key, so typing works immediately.
    tray::show_chat(&app);
    Ok(())
}

#[tauri::command]
fn companion_hide_chat<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    tray::hide_chat(&app);
    Ok(())
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

/// Where the pointer is on the screen, in physical pixels, together with the
/// pet window's own position and size.
///
/// The desk pet looks at the cursor, but a webview only learns about the
/// pointer while it is over the window — and the pet is small and always on
/// top, so it is almost never under the pointer. The screen coordinates have
/// to come from the OS instead.
///
/// Both numbers come from the same coordinate system: on macOS the cursor is
/// reported against the main display's top-left corner, with the Y axis
/// already flipped into screen order, and the window position is reported in
/// that same space. Adding the window's origin to the cursor position would
/// count the offset twice.
#[tauri::command]
fn companion_pointer<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
) -> Result<serde_json::Value, String> {
    tray::check_surface(window.label())?;

    let cursor = window
        .cursor_position()
        .map_err(|e| format!("the cursor position could not be read: {e}"))?;
    let origin = window
        .outer_position()
        .map_err(|e| format!("the window position could not be read: {e}"))?;
    let size = window
        .outer_size()
        .map_err(|e| format!("the window size could not be read: {e}"))?;
    let scale = window
        .scale_factor()
        .map_err(|e| format!("the window scale factor could not be read: {e}"))?;

    Ok(serde_json::json!({
        "screenX": cursor.x,
        "screenY": cursor.y,
        "windowX": origin.x as f64,
        "windowY": origin.y as f64,
        "windowWidth": size.width as f64,
        "windowHeight": size.height as f64,
        "scaleFactor": scale,
    }))
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
        // A global shortcut is registered from Rust, never from the webview:
        // the webview has no business deciding which keys belong to the whole
        // machine. The plugin is initialised here so `setup` can register
        // ⌥ Space before the first frame is shown.
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
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
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && tray::check_surface(window.label()).is_ok()
            {
                // Keep both webviews and their in-flight chat tasks alive.
                api.prevent_close();
                if window.label() == "main" {
                    tray::hide_chat(window.app_handle());
                } else if let Err(error) = tray::hide_surface(window.app_handle(), window.label()) {
                    eprintln!("[window] {error}");
                    let _ = window.emit("window:error", error);
                }
            }
            // The chat window is a headless overlay: clicking anywhere else
            // on the screen puts it away. The desk pet is not an overlay and
            // deliberately ignores focus changes — it is the thing that
            // stays on screen. A native modal dialog inside the window keeps
            // focus, so settings and topics are not dismissed by this.
            if let tauri::WindowEvent::Focused(false) = event
                && window.label() == "main"
                && window.is_visible().unwrap_or(false)
            {
                tray::hide_chat(window.app_handle());
            }
        })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    builder::<tauri::Wry>()
        .setup(|app| {
            // A status-bar application, not a Dock application: no icon in
            // the Dock, and closing the last window no longer ends the
            // process. macOS-only; the other platforms keep the tray as the
            // resident entry without an activation policy to change.
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            // The tray is the copilot's wake-up entry (ADR-0047 decision 5).
            // It is installed here, in the setup hook, so it exists from the
            // first frame — a tray that shows up a moment late reads as a
            // broken feature.
            tray::install(app)?;

            // ⌥ Space toggles the chat overlay from anywhere. A failure is
            // logged rather than fatal: the tray and the desk pet remain, and
            // a shortcut another app already owns must not stop Warden.
            use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
            if let Err(error) =
                app.global_shortcut()
                    .on_shortcut(tray::SHORTCUT, |app, _shortcut, event| {
                        if event.state == ShortcutState::Pressed {
                            tray::toggle_chat(app);
                        }
                    })
            {
                eprintln!("[shortcut] ⌥ Space could not be registered: {error}");
            }

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

            // Phase 3's execution log: beside the other app files, owner-only.
            // The state was managed before this hook ran, so the path is
            // attached here rather than passed in (AuditLog::attach is a
            // OnceLock: first wins, and a command that ran before this point
            // is simply unaudited instead of blocking setup).
            app.state::<conversations::GenerationState>()
                .audit
                .attach(config_dir.join("exec-audit.jsonl"));

            // Only the desk pet is visible at startup; the chat overlay stays
            // hidden until the tray, the shortcut or the pet opens it.
            tray::apply_companion_preference(app.handle());

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
            companion_show_chat,
            companion_hide_chat,
            companion_hide,
            companion_pointer,
            window_drag,
            repo_scan,
            repo_roots_add,
            repo_roots_remove,
            repo_set_favorite,
            repo_dismiss,
            repo_set_projects,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Warden");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discoveries are published as data, never as an interruption. A scan
    /// payload must be self-describing so the settings screen can rebuild
    /// the selectable list without asking for a second scan.
    #[test]
    fn a_scan_payload_is_the_only_channel_for_discoveries() {
        use crate::repos::RepoEntry;
        let repo = RepoEntry {
            path: "/work/team/alpha".into(),
            name: "alpha".into(),
            favorite: false,
            missing: false,
        };
        let scan = RepoScan {
            roots: vec!["/work".into()],
            repos: vec![repo.clone()],
            new_repos: vec![repo],
            projects: Vec::new(),
            truncated: false,
            problem: None,
        };
        let value = serde_json::to_value(&scan).expect("serialize");
        assert!(value.get("projects").is_some());
        assert!(value.get("newRepos").is_some());
        // No announcement channel: no notification action id, no prompt.
        assert!(value.get("announcement").is_none());
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

    /// The global-shortcut plugin has to be registered, not merely depended
    /// on. `app.global_shortcut()` reads its managed state and panics with
    /// "state() called before manage()" if the plugin is missing — a panic
    /// that used to happen only in the running app, because `mock_app` never
    /// runs the setup hook. Building through the production `builder()` turns
    /// that into a test failure.
    #[test]
    fn the_global_shortcut_plugin_is_registered() {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        let app = builder::<tauri::test::MockRuntime>()
            .enable_macos_default_menu(false)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("the app builds");
        // Registering in the test runtime is a no-op; asking for the API is
        // the part that would panic without the plugin.
        let _shortcuts = app.handle().global_shortcut();
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
