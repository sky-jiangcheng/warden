// Finding an executable on this machine.
//
// This module answers one question: is there a binary with this name, and
// where is it. It is used to locate Cinder's MCP server, which is a
// program the user installs separately.
//
// It deliberately does NOT probe anything over the network. "The binary is
// there" and "the program works" are different facts, and only the second
// one matters to a user. The memory layer answers the second one by
// actually connecting (see `memory_layer.rs`); the model providers answer it
// by making a real request (see `provider_commands.rs`).
use std::env;
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

    /// `sh` is on PATH on every platform this app targets; if it cannot be
    /// found, the PATH walk itself is broken and the MCP server would be
    /// reported as missing on every machine.
    #[test]
    fn a_binary_on_path_is_found_with_its_location() {
        let found = find_binary("sh").or_else(|| find_binary("cmd"));
        assert!(found.is_some(), "no shell binary found on PATH");
    }
}
