// Repotory Agent — a local copilot that reads the Repotory memory layer.
//
// The copilot is the window; Repotory is the memory. Everything this window
// knows comes from the two seams in this crate (the model runtime registry
// and the memory-layer probe), and every future capability — chat, reading a
// project, running a command, the memory panels — sits on top of them rather
// than beside them.
mod memory_layer;
mod model_backends;
mod tray;

use serde::Serialize;

/// What the first screen needs: which runtimes exist here, whether the
/// memory layer is installed, and whether the wake-up path is in place.
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

#[tauri::command]
fn shell_status<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> ShellStatus {
    ShellStatus {
        backends: model_backends::detect_backends(),
        default_backend: model_backends::DEFAULT_BACKEND.to_string(),
        default_model: model_backends::DEFAULT_MODEL.to_string(),
        memory: memory_layer::status(),
        tray: TrayStatus {
            installed: tray::is_installed(&app),
        },
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // The tray is the copilot's wake-up entry (ADR-0047 decision 5).
            // It is installed here, in the setup hook, so it exists from the
            // first frame — a tray that shows up a moment late reads as a
            // broken feature.
            tray::install(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![shell_status])
        .run(tauri::generate_context!())
        .expect("error while running the Repotory Agent");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend reads camelCase keys; a rename on the Rust side would
    /// otherwise only show up as an empty first screen at runtime.
    ///
    /// The payload must NOT carry a tool list. It used to, and the hardcoded
    /// copy had already drifted from Repotory's real MCP surface (7 names
    /// listed, 18 registered). The tool names belong to the server and are
    /// fetched over MCP's `tools/list` once the client lands in Phase 1 —
    /// see `memory_layer.rs` for the full reasoning.
    #[test]
    fn shell_status_serves_the_ui_contract() {
        let app = tauri::test::mock_app();
        let payload =
            serde_json::to_string(&shell_status(app.handle().clone())).expect("serialize");
        assert!(payload.contains("\"backends\""));
        assert!(payload.contains("\"defaultBackend\""));
        assert!(payload.contains("\"defaultModel\":\"qwen3:8b\""));
        assert!(payload.contains("\"memory\""));
        assert!(payload.contains("\"available\""));
        assert!(payload.contains("\"path\""));
        assert!(payload.contains("\"tray\""));
        assert!(payload.contains("\"installed\""));
        assert!(
            !payload.contains("repotory_"),
            "no MCP tool name may be hardcoded in the payload: the contract lives in \
             Repotory's main repository and is fetched over the protocol."
        );
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
