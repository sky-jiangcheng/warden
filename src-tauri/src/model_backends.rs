// The Shell's model entry point.
//
// The Shell talks to a local model RUNTIME, not to a model vendor. The
// roadmap's Phase 0 decision is Ollama first, with llama.cpp and MLX to
// follow as pluggable backends — "don't hard-code one runtime". This module
// is the seam that keeps that promise cheap: every backend answers the same
// two questions (is this runtime usable on this machine, and where does it
// live?), and the UI asks one command instead of knowing any backend by name.
//
// Detection is filesystem-only on purpose. Probing a runtime's HTTP endpoint
// would pull a network dependency into the backend for a question Phase 0
// does not need; "is the runtime installed" is the fact the first screen
// shows, and the model manager of a later phase can ask the runtime itself.
use serde::Serialize;
use std::env;
use std::path::{Path, PathBuf};

/// The model the Shell asks for by default: Qwen3 8B Q4 (roadmap Phase 0).
pub const DEFAULT_MODEL: &str = "qwen3:8b";

/// The backend that serves the default model.
pub const DEFAULT_BACKEND: &str = "ollama";

/// One backend's availability, as the UI shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendStatus {
    pub id: String,
    /// Whether the runtime is installed on this machine.
    pub available: bool,
    /// Where the runtime was found; None when it is not installed.
    pub path: Option<String>,
    /// "ready" (implemented) or "planned" (declared, not implemented yet).
    pub state: String,
}

fn binary_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Looks for a binary on PATH, then where the installers drop it when the
/// user's shell PATH was never updated (the macOS .pkg case, and the reason
/// a Dock-launched IDE sees fewer binaries than a terminal does).
pub fn find_binary(name: &str) -> Option<PathBuf> {
    let file = binary_name(name);
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join(&file);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    let home = env::var_os("HOME")?;
    let fallbacks = [
        PathBuf::from(&home).join(".ollama").join("bin").join(&file),
        PathBuf::from("/opt/homebrew/bin").join(&file),
        PathBuf::from("/usr/local/bin").join(&file),
    ];
    fallbacks.into_iter().find(|c| is_executable(c))
}

/// Every backend the Shell knows about, in the order the UI lists them.
///
/// Ollama is implemented; llama.cpp and MLX are declared as planned so the
/// contract stays stable when they arrive, without pretending they work.
pub fn detect_backends() -> Vec<BackendStatus> {
    let ollama = find_binary("ollama");
    vec![
        BackendStatus {
            id: "ollama".to_string(),
            available: ollama.is_some(),
            path: ollama.map(|p| p.display().to_string()),
            state: "ready".to_string(),
        },
        BackendStatus {
            id: "llama.cpp".to_string(),
            available: false,
            path: None,
            state: "planned".to_string(),
        },
        BackendStatus {
            id: "mlx".to_string(),
            available: false,
            path: None,
            state: "planned".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_is_the_only_implemented_backend() {
        let backends = detect_backends();
        let ollama = backends
            .iter()
            .find(|b| b.id == "ollama")
            .expect("ollama entry");
        assert_eq!(ollama.state, "ready");

        for planned in backends.iter().filter(|b| b.id != "ollama") {
            assert_eq!(planned.state, "planned");
            assert!(
                !planned.available,
                "{} must not claim availability",
                planned.id
            );
            assert!(planned.path.is_none());
        }
    }

    #[test]
    fn a_backend_on_path_is_found_with_its_location() {
        // `sh` is on PATH on every platform this app targets; if it cannot be
        // found, the PATH walk itself is broken and the UI would report every
        // runtime as missing.
        let found = find_binary("sh").or_else(|| find_binary("cmd"));
        assert!(found.is_some(), "no shell binary found on PATH");
    }

    #[test]
    fn the_default_model_is_the_roadmap_pick() {
        assert_eq!(DEFAULT_MODEL, "qwen3:8b");
        assert_eq!(DEFAULT_BACKEND, "ollama");
    }
}
