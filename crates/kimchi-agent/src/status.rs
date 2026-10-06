//! Which providers the panel can use right now, each with a sentence for the person.
//!
//! Local checks only: a CLI found and signed in, a key present, Ollama answering.
//! No model request is sent; the first message is what proves model access.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kimchi_control::Session;
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

use crate::cli::{cli_executable, mcp_executable};
use crate::{AgentConfig, ProviderKind};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: ProviderKind,
    pub label: &'static str,
    /// It can be used now.
    pub ready: bool,
    /// The one chosen in `settings.agent.provider`.
    pub active: bool,
    /// What to tell the person ("Claude Code 2.1 is installed and signed in.").
    pub message: String,
    /// The executable or address that was checked.
    pub detail: String,
    /// Model used when `settings.agent.model` is empty (empty: the provider decides).
    pub default_model: &'static str,
    /// Models Ollama has installed (empty for the others).
    pub models: Vec<String>,
}

/// Every provider with whether it is usable, for the panel and its settings.
pub async fn provider_status(session: &Arc<Session>) -> Vec<ProviderStatus> {
    let settings = session.settings().agent;
    let active = AgentConfig::from_settings(&settings).provider;
    let mut out = vec![];
    for kind in ProviderKind::ALL {
        let mut config = AgentConfig::from_settings(&settings);
        if config.provider != kind {
            config = AgentConfig::new(kind);
        }
        let (ready, message, detail, models) = match kind {
            ProviderKind::Zenith => match crate::zenith::models().await {
                Ok(models) => (true, "Connected to Zenith. Conversations use your lsuite agents and their permissions.".into(), crate::zenith::executable().map(|p| p.display().to_string()).unwrap_or_default(), models),
                Err(e) => (false, e, String::new(), vec![]),
            },
            ProviderKind::ClaudeCode | ProviderKind::Codex => cli_status(session, kind).await,
            ProviderKind::Anthropic | ProviderKind::OpenAi => {
                let (id, env) = kind.key_source().unwrap_or_default();
                let custom = config.base_url() != kind.default_base_url();
                match (session.secret(id).is_some(), std::env::var(env).is_ok_and(|k| !k.trim().is_empty())) {
                    (true, _) => (true, format!("Ready: an {} key is saved in the keychain.", kind.label()), config.base_url(), vec![]),
                    (false, true) => (true, format!("Ready: using {env} from the environment."), config.base_url(), vec![]),
                    (false, false) if custom && kind == ProviderKind::OpenAi => {
                        (true, format!("Ready to try the server at {} (no key).", config.base_url()), config.base_url(), vec![])
                    }
                    _ => (false, format!("Add an {} key in Settings › Agent, or set {env}. It is billed by the provider, separately from any chat subscription.", kind.label()), config.base_url(), vec![]),
                }
            }
            ProviderKind::Ollama => {
                let base = config.base_url();
                match crate::api::ollama_models(&base).await {
                    Ok(models) if models.is_empty() => {
                        (false, "Ollama is running but has no models. Pull one that supports tools, e.g. `ollama pull qwen3`.".to_string(), base, models)
                    }
                    Ok(models) => (true, format!("Ollama is running with {} model{}. Nothing leaves this computer.", models.len(), if models.len() == 1 { "" } else { "s" }), base, models),
                    Err(e) => (false, e, base, vec![]),
                }
            }
        };
        out.push(ProviderStatus { provider: kind, label: kind.label(), ready, active: kind == active, message, detail, default_model: kind.default_model(), models });
    }
    out
}

/// Runs `exe args` for a short check; `None` if it can't start or takes too long.
async fn probe(exe: &Path, args: &[&str]) -> Option<(bool, String)> {
    let mut cmd = Command::new(exe);
    // The same PATH as a run, so an npm script finds `node` from the Dock too.
    cmd.args(args).env("PATH", crate::cli::child_path(exe)).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashing up on each check
    let out = tokio::time::timeout(Duration::from_secs(8), cmd.output()).await.ok()?.ok()?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

async fn cli_status(session: &Session, kind: ProviderKind) -> (bool, String, String, Vec<String>) {
    let label = kind.label();
    let Some(exe) = cli_executable(kind) else {
        let how = match kind {
            ProviderKind::Codex => "Install it (npm install -g @openai/codex) and sign in with `codex login`.",
            _ => "Install it from claude.com/claude-code and sign in by running `claude` once.",
        };
        return (false, format!("{label} isn't installed. {how}"), String::new(), vec![]);
    };
    let detail = exe.display().to_string();
    let version = match probe(&exe, &["--version"]).await {
        Some((true, v)) => v.split_whitespace().find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit())).map(str::to_string),
        _ => return (false, format!("{label} at {detail} doesn't start. Reinstall or update it."), detail, vec![]),
    };
    let name = match &version {
        Some(v) => format!("{label} {v}"),
        None => label.to_string(),
    };
    let signed_in = match kind {
        ProviderKind::ClaudeCode => probe(&exe, &["auth", "status"]).await.map(|(_, s)| serde_json::from_str::<Value>(&s).ok().and_then(|v| v["loggedIn"].as_bool())),
        _ => probe(&exe, &["login", "status"]).await.map(|(ok, _)| Some(ok)),
    }
    .flatten();
    if signed_in == Some(false) {
        let how = if kind == ProviderKind::Codex { "codex login" } else { "claude auth login" };
        return (false, format!("{name} is installed but signed out. Run `{how}` in a terminal, then check again."), detail, vec![]);
    }
    if mcp_executable().is_none() {
        return (false, format!("{name} is installed, but kimchi-mcp, which connects it to kimchi, wasn't found. Reinstall kimchi."), detail, vec![]);
    }
    if session.bridge_port().is_none() {
        return (false, format!("{name} is installed, but kimchi's live bridge isn't running. Restart kimchi."), detail, vec![]);
    }
    let state = if signed_in == Some(true) { "installed and signed in" } else { "installed" };
    (true, format!("{name} is {state}. It uses your own {} account.", if kind == ProviderKind::Codex { "ChatGPT/OpenAI" } else { "Claude" }), detail, vec![])
}
