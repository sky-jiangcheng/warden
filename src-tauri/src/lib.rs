// Repotory Agent — a local copilot that reads the Repotory memory layer.
//
// The copilot is the window; Repotory is the memory. Everything this window
// knows comes from the two seams in this crate (the model runtime registry
// and the memory-layer probe), and every future capability — chat, reading a
// project, running a command, the memory panels — sits on top of them rather
// than beside them.
mod memory_layer;
mod model_backends;

use serde::Serialize;

/// What the first screen needs: which runtimes exist here, and whether the
/// memory layer is installed.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellStatus {
    pub backends: Vec<model_backends::BackendStatus>,
    pub default_backend: String,
    pub default_model: String,
    pub memory: memory_layer::MemoryLayerStatus,
}

#[tauri::command]
fn shell_status() -> ShellStatus {
    ShellStatus {
        backends: model_backends::detect_backends(),
        default_backend: model_backends::DEFAULT_BACKEND.to_string(),
        default_model: model_backends::DEFAULT_MODEL.to_string(),
        memory: memory_layer::status(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
        let payload = serde_json::to_string(&shell_status()).expect("serialize");
        assert!(payload.contains("\"backends\""));
        assert!(payload.contains("\"defaultBackend\""));
        assert!(payload.contains("\"defaultModel\":\"qwen3:8b\""));
        assert!(payload.contains("\"memory\""));
        assert!(payload.contains("\"available\""));
        assert!(payload.contains("\"path\""));
        assert!(
            !payload.contains("repotory_"),
            "no MCP tool name may be hardcoded in the payload: the contract lives in \
             Repotory's main repository and is fetched over the protocol."
        );
        println!("{payload}");
    }
}
