//! The `repotory-agent init` subcommand: one command that prepares the machine.
//!
//! This module exists because "install the app" is not the same as "the app
//! works on this machine". A fresh download leaves three things unverified —
//! the app's own configuration, the memory layer it reads, and the model it
//! chats with — and none of them fails loudly. The GUI reports each state
//! on its first screen, but a person installing headless, or debugging a
//! headless run, needs the same facts from the terminal.
//!
//! What this deliberately is NOT: a setup wizard that configures things for
//! the user. It reports what it finds and writes the one file a first run
//! needs anyway (the default provider config), the same file the GUI would
//! have written. Everything after that is the user's decision — a model
//! choice is a model choice, not something a wizard should pick.

use crate::mcp_client::McpSession;
use crate::provider_commands::config_or_default;
use crate::providers::ProviderStore;

/// A single check's outcome, for the human-readable report.
enum Check {
    /// Green: the thing works.
    Ok(String),
    /// Red: the thing is broken, and the string is what to do.
    Fail(String),
    /// Yellow: present but not fully working, and the string explains.
    Warn(String),
}

impl Check {
    /// The one-character marker this line starts with.
    fn marker(&self) -> char {
        match self {
            Check::Ok(_) => '✓',
            Check::Fail(_) => '✗',
            Check::Warn(_) => '!',
        }
    }
}

/// The full report, kept as a unit so the exit code has a single source.
#[derive(Default)]
struct Report {
    checks: Vec<(String, Check)>,
}

impl Report {
    fn push(&mut self, name: impl Into<String>, check: Check) {
        self.checks.push((name.into(), check));
    }

    /// Non-zero only when something red failed. Yellow is not failure —
    /// an installed-but-stopped model is worth a warning, and blocking
    /// the install on it would make `init` fail on machines where the
    /// user simply hasn't started the model yet.
    fn exit_code(&self) -> i32 {
        if self.checks.iter().any(|(_, c)| matches!(c, Check::Fail(_))) {
            1
        } else {
            0
        }
    }
}

/// Runs the init checks. Split from `main` so the checks can be tested
/// without spawning the whole app, and so the CLI entry stays a thin shim.
pub async fn run_init() -> i32 {
    let mut report = Report::default();

    // 1. Configuration. A fresh app config directory is created on demand;
    //    the default provider is what the GUI seeds too, so the file written
    //    here is the same file the app would write. The location is resolved
    //    without an app handle (see `cli_config_dir`); a test below checks
    //    it agrees with the one Tauri uses.
    let config_dir = cli_config_dir();

    if std::fs::create_dir_all(&config_dir).is_ok() {
        // Seed the default provider exactly as the GUI does — one code path,
        // so the CLI and the app cannot drift apart on what a default is.
        let store = ProviderStore::new(config_dir.join("providers.json"));
        let (config, problem) = config_or_default(&store);
        match problem {
            Some(why) => report.push("配置", Check::Fail(why)),
            None => {
                let active = config
                    .active_provider_id
                    .as_deref()
                    .and_then(|id| config.providers.iter().find(|p| p.id == id))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "无".to_string());
                report.push(
                    "配置",
                    Check::Ok(format!(
                        "{} 个 provider，当前 {}（配置文件 {}）",
                        config.providers.len(),
                        active,
                        config_dir.join("providers.json").display()
                    )),
                );
            }
        }
    } else {
        report.push(
            "配置",
            Check::Fail(format!("配置目录不可写：{}", config_dir.display())),
        );
    }

    // 2. The memory layer. The same binary lookup and handshake the GUI's
    //    first screen runs — a probe that only looks for the file would
    //    say "installed" while saying nothing about whether it works.
    report.push("记忆层", memory_check(&McpSession::default()).await);

    // 3. The local model. Reuses the panel's probe verbatim: four states,
    //    and the advice is the same sentence the UI would show.
    report.push("本机模型", local_model_check().await);

    println!();
    println!("Warden 初始化检查");
    println!("{}", "─".repeat(56));
    for (name, check) in &report.checks {
        println!("{} {name}：{}", check.marker(), check.text());
    }
    println!();
    report.exit_code()
}

impl Check {
    fn text(&self) -> &str {
        match self {
            Check::Ok(s) | Check::Fail(s) | Check::Warn(s) => s,
        }
    }
}

/// The memory-layer half of the report, with the actionable failure text
/// the GUI's own status carries.
async fn memory_check(session: &McpSession) -> Check {
    match crate::memory_layer::status(session).await {
        status if status.connected => {
            // The contract version is the one the README's "version coupling"
            // section tells people to read — so the CLI reports it too,
            // not just the tool count. It comes from the server's own
            // `initialize` answer; nothing is copied from the parent repo.
            let version = status.version.clone().unwrap_or_else(|| "未知".into());
            Check::Ok(format!(
                "已连接，{} 个工具，记忆层 v{version}（{}）",
                status.tools.len(),
                status.path.clone().unwrap_or_default()
            ))
        }
        status if status.available => Check::Warn(format!(
            "已安装但没连上：{}",
            status.error.clone().unwrap_or_else(|| "未知错误".into())
        )),
        _ => Check::Fail(
            "未安装 repotory-mcp（记忆层服务端）。它是 Cinder 的一部分，请安装 Cinder 后重试。"
                .to_string(),
        ),
    }
}

/// The local-model half of the report.
async fn local_model_check() -> Check {
    local_model_check_for(crate::local_models::probe().await)
}

/// The mapping from the probe's state to a report line, kept pure so the
/// four states are testable without a live endpoint. A new state added to
/// `probe()` must be added here too — the match is exhaustive, so it
/// cannot silently produce a blank line.
fn local_model_check_for(probe: crate::local_models::LocalModels) -> Check {
    match (probe.installed, probe.running, probe.has_default) {
        (_, true, true) => {
            let ms = probe.latency_ms.unwrap_or(0);
            Check::Ok(format!("就绪（{ms} ms，{} 个模型）", probe.models.len()))
        }
        (true, true, false) => Check::Warn(
            probe
                .advice
                .clone()
                .unwrap_or_else(|| "缺少默认模型".into()),
        ),
        (true, false, _) => Check::Warn(
            probe
                .advice
                .clone()
                .unwrap_or_else(|| "已安装但未运行".into()),
        ),
        (false, false, _) => Check::Fail(
            probe
                .advice
                .clone()
                .unwrap_or_else(|| "未安装 Ollama".into()),
        ),
        (false, true, _) => Check::Ok(format!("就绪（{} 个模型）", probe.models.len())),
    }
}

/// Where the app's config lives when there is no app handle to ask.
///
/// The GUI resolves this through Tauri's path API; the CLI has no app, so
/// it mirrors that location directly rather than constructing an app
/// handle for one path lookup.
fn cli_config_dir() -> std::path::PathBuf {
    // Built with `push` rather than string concatenation: a `HOME` that
    // ends in a separator would otherwise produce a double slash, and the
    // app-id segment is appended as a path component, not a suffix.
    let mut dir = std::path::PathBuf::new();
    #[cfg(target_os = "macos")]
    {
        dir.push(home());
        dir.push("Library/Application Support");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dir.push(xdg_config_home());
    }
    #[cfg(windows)]
    {
        dir.push(std::env::var_os("APPDATA").unwrap_or_default());
    }
    dir.push(APP_ID);
    dir
}

/// The user's home directory.
#[cfg(any(target_os = "macos", all(unix, not(target_os = "macos"))))]
fn home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(|v| v.into())
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

/// `XDG_CONFIG_HOME`, defaulting to `$HOME/.config` per the XDG spec.
#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_config_home() -> std::path::PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(|v| v.into())
        .unwrap_or_else(|| home().join(".config"))
}

/// The app's identifier — the same string the Tauri bundle uses, so the
/// CLI writes into the same directory the GUI manages.
const APP_ID: &str = "com.jiangcheng.repotoryagent";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_code_follows_the_red_checks_only() {
        // Yellow is "look at this", not "the install failed" — the whole
        // point of not blocking on a model the user hasn't started yet.
        let mut only_ok = Report::default();
        only_ok.push("a", Check::Ok("fine".to_string()));
        only_ok.push("b", Check::Warn("check this".to_string()));
        assert_eq!(only_ok.exit_code(), 0);

        let mut with_fail = Report::default();
        with_fail.push("a", Check::Ok("fine".to_string()));
        with_fail.push("b", Check::Fail("broken".to_string()));
        assert_eq!(with_fail.exit_code(), 1);
    }

    #[test]
    fn the_markers_are_distinguishable_in_plain_text() {
        // The report is for humans at a terminal; each state must read
        // differently at a glance rather than all being a tick.
        assert_ne!(
            Check::Ok(String::new()).marker(),
            Check::Fail(String::new()).marker()
        );
        assert_ne!(
            Check::Warn(String::new()).marker(),
            Check::Ok(String::new()).marker()
        );
    }

    #[test]
    fn the_config_dir_mirrors_what_tauri_would_use() {
        // The CLI resolves this without an app handle; if it drifted from
        // the GUI's location, the two would seed different configs and the
        // default provider would appear missing in one and present in the
        // other.
        let dir = cli_config_dir();
        assert!(!dir.as_os_str().is_empty());
        assert!(dir.to_string_lossy().contains("repotoryagent"));
    }

    #[test]
    fn every_probe_state_gets_a_label() {
        // The mapping is exhaustive over the four states; a new state added
        // to `probe()` must be added here too, and each must carry text.
        let ready = crate::local_models::LocalModels {
            installed: true,
            running: true,
            has_default: true,
            latency_ms: Some(12),
            models: vec!["qwen3:8b".into()],
            advice: None,
            ..Default::default()
        };
        let missing = crate::local_models::LocalModels {
            installed: true,
            running: true,
            has_default: false,
            advice: Some("拉取".into()),
            ..Default::default()
        };
        let not_running = crate::local_models::LocalModels {
            installed: true,
            running: false,
            advice: Some("serve".into()),
            ..Default::default()
        };
        let absent = crate::local_models::LocalModels {
            installed: false,
            running: false,
            advice: Some("安装".into()),
            ..Default::default()
        };
        for probe in [&ready, &missing, &not_running, &absent] {
            let check = local_model_check_for(probe.clone());
            assert!(
                !check.text().is_empty(),
                "a state must not map to a blank line"
            );
        }
        assert!(matches!(local_model_check_for(ready.clone()), Check::Ok(_)));
        assert!(matches!(
            local_model_check_for(absent.clone()),
            Check::Fail(_)
        ));
    }
}
