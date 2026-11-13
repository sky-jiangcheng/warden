//! Phase 5: making a local model work without fiddling.
//!
//! The phase's goal is stated in its first line — 「让 M4 Pro 用户不用折腾也
//! 能跑」— and the hard constraint in its last: **do not open a branch for
//! Ollama**. Local and hosted models are the same thing (provider = one base
//! URL), so everything here is expressed as "does the DEFAULT LOCAL
//! ENDPOINT answer", never as "is the ollama vendor special". If llama.cpp
//! or MLX ever ship a compatible surface, they become another base URL and
//! this module needs no change.
//!
//! Why a dedicated module rather than more checks inside `provider_commands`:
//! those answer "can I talk to the provider the user configured". This one
//! answers a different, earlier question — "is there a model on this machine
//! at all, and if not, what exactly should the user do". The distinction
//! matters because the advice differs: a hosted provider that fails wants
//! its key checked, a local one that fails wants an install or a `serve`
//! command.
//!
//! Two facts are kept apart on purpose, the same discipline as the memory
//! layer's three states:
//!
//! * **installed** — the binary exists on this machine;
//! * **running** — the endpoint answers.
//!
//! "Installed but not running" is the most common local case and it has its
//! own advice (`ollama serve`). Reporting it as "not installed" would send
//! the user to download something they already have.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::llm::{LlmClient, LlmError};
use crate::providers::Provider;

/// The endpoint a default install listens on. Named here once: the default
/// provider config and this probe must agree, or "Ollama is running" and
/// "the provider connects" would disagree about the same program.
pub const LOCAL_ENDPOINT: &str = "http://127.0.0.1:11434/v1";

/// The model a fresh machine should pull. Small enough for a laptop, good
/// enough to judge whether the whole path works.
pub const DEFAULT_MODEL: &str = "qwen3:8b";

/// How long the probe waits. A local endpoint either answers at once or is
/// not running; a long timeout here would stall the first screen for a
/// service the user may not even have installed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalModels {
    /// The endpoint this report is about — the conversation surface is
    /// always "one base URL", so the panel names which one it probed.
    pub endpoint: String,
    /// The binary is on this machine (PATH, then the install-location
    /// fallbacks — the same lookup the MCP binary uses).
    pub installed: bool,
    /// Where it was found, when it was.
    pub binary: Option<String>,
    /// The endpoint answered a real request.
    pub running: bool,
    /// The models the endpoint reports, as ids.
    pub models: Vec<String>,
    /// True when the default model is among them.
    pub has_default: bool,
    /// The model a fresh machine should pull.
    pub default_model: String,
    /// Round-trip time of the probe, when it answered. Shown because on a
    /// local model it is the number a user actually feels.
    pub latency_ms: Option<u64>,
    /// Human-readable next step. None when everything works.
    pub advice: Option<String>,
}

impl LocalModels {
    /// The state this report is in, for the UI's three-way chip. Named so
    /// the frontend never re-derives it from booleans and drifts.
    pub fn state(&self) -> &'static str {
        match (self.installed, self.running) {
            (true, true) if self.has_default => "ready",
            (true, true) => "missing-model",
            (true, false) => "not-running",
            (false, false) => "not-installed",
            // Running but the binary was not found happens with a remote
            // Docker host or a manually started daemon: trust the answer
            // over the file system, because the answer is what the chat uses.
            (false, true) => "ready",
        }
    }
}

/// Probes the local endpoint and reports what the user should do next.
///
/// Never returns an Err: "it is not there" is an ANSWER, and an error would
/// make the frontend show a failure for a perfectly normal state on a fresh
/// machine. The binary lookup is blocking filesystem work, so it runs on the
/// blocking pool; the request itself is async.
pub async fn probe() -> LocalModels {
    let binary = tauri::async_runtime::spawn_blocking(|| crate::binaries::find_binary("ollama"))
        .await
        .unwrap_or_default();
    let provider = Provider {
        id: "local-probe".to_string(),
        name: "local".to_string(),
        base_url: LOCAL_ENDPOINT.to_string(),
        default_model: Some(DEFAULT_MODEL.to_string()),
        headers: Default::default(),
        has_api_key: false,
    };
    let mut report = LocalModels {
        endpoint: LOCAL_ENDPOINT.to_string(),
        installed: binary.is_some(),
        binary: binary.map(|path| path.display().to_string()),
        running: false,
        models: Vec::new(),
        has_default: false,
        default_model: DEFAULT_MODEL.to_string(),
        latency_ms: None,
        advice: None,
    };
    let Ok(client) = LlmClient::new(&provider, None) else {
        report.advice = Some("本地模型客户端初始化失败。".to_string());
        return report;
    };
    let started = Instant::now();
    // The probe budget is per-request and honest: a local endpoint either
    // answers at once or is not running, and a long wait here would stall the
    // first screen for a service the user may not have installed.
    match tokio::time::timeout(PROBE_TIMEOUT, client.list_models()).await {
        Ok(Ok(models)) => {
            report.running = true;
            report.latency_ms = Some(started.elapsed().as_millis() as u64);
            report.models = models.into_iter().map(|model| model.id).collect();
            report.has_default = report.models.iter().any(|id| id == DEFAULT_MODEL);
        }
        Ok(Err(error)) => {
            // The endpoint answered with a failure. Whether that reads as
            // "not installed" or "installed but not running" depends on the
            // binary, which is why both facts are carried separately.
            report.advice = Some(advice_for(report.installed, &error));
        }
        Err(_) => {
            report.advice = Some(advice_for(
                report.installed,
                &LlmError::Timeout(format!("{PROBE_TIMEOUT:?}")),
            ));
        }
    }
    if report.running && !report.has_default && report.advice.is_none() {
        report.advice = Some(format!(
            "本机有模型但没有 {DEFAULT_MODEL}。可以在设置里选一个已有模型，或运行 `ollama pull {DEFAULT_MODEL}`。"
        ));
    }
    report
}

/// The one sentence a user can act on, in the state they are actually in.
///
/// Kept as a pure function of (installed, error) so the wording can be
/// tested: this is the only thing a stuck user reads before giving up.
fn advice_for(installed: bool, error: &LlmError) -> String {
    if installed {
        format!(
            "本机装了 Ollama，但 {LOCAL_ENDPOINT} 没有响应。运行 `ollama serve` 启动它（或打开 Ollama 应用），然后重试。"
        )
    } else if matches!(error, LlmError::Transport(_) | LlmError::Timeout(_)) {
        format!(
            "没有找到本机的模型运行时。装 Ollama 后运行 `ollama pull {DEFAULT_MODEL}`，或在设置里连接一个云端 provider。"
        )
    } else {
        format!("本机模型端点出错：{error}")
    }
}

/// Pulls the default model through the runtime's own API.
///
/// WHY THIS IS THE ONE VENDOR-SPECIFIC CALL IN THE WHOLE PHASE. Everything
/// else here is provider-agnostic ("does this base URL answer"), because
/// downloading a model is not part of the OpenAI-compatible surface: there is
/// no `POST /models` in that spec. A hosted provider never needs it either —
/// its models already exist. So this is not "Ollama gets a branch"; it is the
/// only endpoint that exists for the only situation that needs it, and it is
/// reached by deriving the native root from the configured base URL rather
/// than by hardcoding a second host.
///
/// Blocking-safe: the caller runs it off the async workers (it is a streamed
/// response that can take minutes).
pub async fn pull_default() -> Result<String, String> {
    // `http://127.0.0.1:11434/v1` -> `http://127.0.0.1:11434/api/pull`. The
    // `/v1` suffix is the OpenAI surface; the native API sits beside it.
    let root = LOCAL_ENDPOINT
        .trim_end_matches('/')
        .strip_suffix("/v1")
        .ok_or("the local endpoint is not the expected shape")?;
    let url = format!("{root}/api/pull");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| format!("HTTP 客户端初始化失败：{e}"))?;
    let response = client
        .post(&url)
        .json(&serde_json::json!({ "model": DEFAULT_MODEL, "stream": false }))
        .send()
        .await
        .map_err(|e| format!("拉取失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "拉取失败：{} {}",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or("")
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("拉取返回的不是 JSON：{e}"))?;
    // The native API reports its own failures in the body with a 200 status
    // in some versions, so `status` is checked rather than assumed.
    if let Some(status) = body.get("status").and_then(|v| v.as_str())
        && status != "success"
    {
        return Err(format!("拉取未完成：{status}"));
    }
    Ok(DEFAULT_MODEL.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(installed: bool, running: bool, has_default: bool) -> LocalModels {
        LocalModels {
            installed,
            running,
            has_default,
            ..LocalModels::default()
        }
    }

    #[test]
    fn the_four_states_are_distinct() {
        // The whole point of carrying two booleans: "installed but not
        // running" must not read as "not installed", because the advice
        // differs and one of them wastes the user's download.
        assert_eq!(report(false, false, false).state(), "not-installed");
        assert_eq!(report(true, false, false).state(), "not-running");
        assert_eq!(report(true, true, false).state(), "missing-model");
        assert_eq!(report(true, true, true).state(), "ready");
    }

    #[test]
    fn a_running_endpoint_wins_over_a_missing_binary() {
        // A daemon started by something other than this shell (Docker, a
        // GUI app launched before PATH was set) is still running. The chat
        // cares about the answer, not about the file.
        assert_eq!(report(false, true, true).state(), "ready");
    }

    #[test]
    fn the_advice_matches_the_state() {
        let unreachable = LlmError::Transport("connection refused".to_string());
        let not_installed = advice_for(false, &unreachable);
        assert!(not_installed.contains("ollama pull"), "{not_installed}");
        assert!(!not_installed.contains("ollama serve"), "{not_installed}");

        let not_running = advice_for(true, &unreachable);
        // Installed: telling the user to install again is the failure this
        // test exists to prevent.
        assert!(not_running.contains("ollama serve"), "{not_running}");
        assert!(!not_running.contains("ollama pull"), "{not_running}");
    }

    #[test]
    fn the_probe_endpoint_is_the_one_the_default_provider_uses() {
        // These two must agree, or "Ollama is running" and "the provider
        // connects" would be talking about different ports.
        let default = crate::providers::default_config();
        let local = default.providers.iter().find(|provider| {
            provider.base_url.trim_end_matches('/') == LOCAL_ENDPOINT.trim_end_matches('/')
        });
        assert!(
            local.is_some(),
            "the seeded provider must be the probed endpoint"
        );
        assert_eq!(
            local.and_then(|provider| provider.default_model.as_deref()),
            Some(DEFAULT_MODEL)
        );
    }

    #[tokio::test]
    async fn probing_never_panics_and_always_answers() {
        // On this machine the state is whatever it is; the contract is that
        // the function RETURNS (no Err, no panic) and that the advice agrees
        // with the state it reports.
        let report = probe().await;
        assert!(!report.endpoint.is_empty());
        assert_eq!(report.default_model, DEFAULT_MODEL);
        if !report.running {
            assert!(
                report.advice.is_some(),
                "a state that is not running owes the user a next step"
            );
        }
        assert!(
            ["ready", "missing-model", "not-running", "not-installed"].contains(&report.state())
        );
    }
}
