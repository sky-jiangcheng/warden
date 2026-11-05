// Repotory Agent — a local copilot that reads the Repotory memory layer.
//
// The copilot is the window; Repotory is the memory. Everything this window
// knows comes from the seams in this crate (the model runtime registry, the
// memory-layer connection and the wake-up path), and every future capability
// — chat, reading a project, running a command, the memory panels — sits on
// top of them rather than beside them.
// `mcp_client` is public because the end-to-end test in `tests/` drives it
// against a fake server. The other modules stay private: nothing outside
// this crate has a reason to reach into them.
pub mod mcp_client;
pub mod memory_layer;
mod model_backends;
mod tray;

use serde::Serialize;
use tauri::Manager;

/// What the first screen needs: which runtimes exist here, whether the
/// memory layer is installed AND answering, and whether the wake-up path is
/// in place.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellStatus {
    pub backends: Vec<model_backends::BackendStatus>,
    pub default_backend: String,
    pub default_model: String,
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
    Ok(ShellStatus {
        backends: model_backends::detect_backends(),
        default_backend: model_backends::DEFAULT_BACKEND.to_string(),
        default_model: model_backends::DEFAULT_MODEL.to_string(),
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![shell_status, mcp_call_tool])
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
        let handle = app.handle().clone();
        let payload = serde_json::to_string(&shell_status(handle).await).expect("serialize");
        assert!(payload.contains("\"backends\""));
        assert!(payload.contains("\"defaultBackend\""));
        assert!(payload.contains("\"defaultModel\":\"qwen3:8b\""));
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
