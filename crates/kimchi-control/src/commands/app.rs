use std::sync::Arc;

use serde_json::{Value, json};

use crate::registry::{self, Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "app.info" => {
            let mut v = info(s);
            // Which 3D renderer draws motion clips (starts it, so not part of `info`).
            v["renderer3d"] = json!(tokio::task::spawn_blocking(kimchi_media::render::Renderer::engine).await.map_err(crate::session::err)?);
            Ok(v)
        }
        "app.commands" => match a.opt_str("command") {
            Some(name) => registry::spec(name).map(registry::describe).ok_or_else(|| format!("Unknown command `{name}`.")),
            None => Ok(json!(registry::commands().iter().map(registry::describe).collect::<Vec<_>>())),
        },
        "app.fonts" => Ok(json!(tokio::task::spawn_blocking(kimchi_media::text::font_families).await.map_err(crate::session::err)?)),
        "app.settings" => Ok(json!(s.settings())),
        "app.setSetting" => {
            // `.agent.permissions` and `agent..x` name the same setting: check the normal form.
            let key = a.str("key")?.split('.').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(".");
            if cx.source.is_agent() && (key == "agent" || key.starts_with("agent.")) {
                return Err("Agent settings and permissions stay with the person.".into());
            }
            let value = a.get("value").cloned().unwrap_or(Value::Null);
            let settings = s.update_settings_checked(|st| st.set(&key, value))?;
            Ok(json!({ "key": key, "value": settings.get(&key) }))
        }
        "app.setAgentKey" => {
            let provider = a.str("provider")?;
            if !matches!(provider, "anthropic" | "openai") {
                return Err(format!("The agent keeps keys for \"anthropic\" and \"openai\", not \"{provider}\"."));
            }
            s.set_secret(provider, a.opt_str("key"))?;
            s.emit(crate::session::Event::SettingsChanged);
            Ok(json!({ "provider": provider, "saved": s.secret(provider).is_some() }))
        }
        "app.checkUpdates" => Ok(json!(crate::update::check(s, true).await?)),
        "app.installUpdate" => Ok(json!(crate::update::install(s).await?)),
        "app.whatsNew" => {
            let releases = if a.opt_bool("all").unwrap_or(false) {
                crate::release_notes::all()
            } else if let Some(since) = a.opt_str("since") {
                crate::release_notes::since(since)
            } else {
                let v = a.opt_str("version").unwrap_or(crate::update::CURRENT);
                vec![crate::release_notes::find(v).ok_or_else(|| format!("No release notes for {v}. With all=true, app.whatsNew lists every release."))?]
            };
            Ok(json!({ "current": crate::update::CURRENT, "releases": releases }))
        }
        "app.diagnostics" => {
            let mut v = diagnostics(s);
            v["renderer3d"] = json!(tokio::task::spawn_blocking(kimchi_media::render::Renderer::engine).await.map_err(crate::session::err)?);
            Ok(v)
        }
        "app.logs" => {
            let lines = a.opt_i64("lines").unwrap_or(100).clamp(1, 2000) as usize;
            let files = crate::diagnostics::log_files(&s.data_dir);
            let path = match a.opt_str("file") {
                Some(name) => files
                    .iter()
                    .map(|(p, _)| p.clone())
                    .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name))
                    .ok_or_else(|| format!("No log file named `{name}`. app.logs lists them."))?,
                None => crate::diagnostics::current_log().or_else(|| files.first().map(|(p, _)| p.clone())).ok_or("kimchi hasn't written a log yet.")?,
            };
            let tail = crate::diagnostics::tail(&path, lines).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
            Ok(json!({
                "folder": crate::diagnostics::logs_dir(&s.data_dir),
                "files": files.iter().map(|(p, n)| json!({ "name": p.file_name().map(|n| n.to_string_lossy()), "path": p, "bytes": n })).collect::<Vec<_>>(),
                "file": path,
                "lines": tail,
            }))
        }
        "app.crashReports" => match a.opt_str("id") {
            Some(id) => Ok(json!({ "id": id, "text": crate::diagnostics::read_report(&s.data_dir, id)? })),
            None => Ok(json!({ "folder": crate::diagnostics::crashes_dir(&s.data_dir), "reports": crate::diagnostics::reports(&s.data_dir) })),
        },
        "app.clearCrashReports" => Ok(json!({ "deleted": crate::diagnostics::clear_reports(&s.data_dir) })),
        "app.quit" | "app.notify" | "app.restart" => s.ui_call(cx.spec.name, Value::Object(a.0)).await,
        _ => Err(crate::commands::unhandled(cx)),
    }
}

pub fn info(s: &Session) -> Value {
    json!({
        "app": "kimchi",
        "version": env!("CARGO_PKG_VERSION"),
        "ffmpeg": s.tools_if_found().map(|t| t.ffmpeg),
        "library": s.library.root(),
        "dataDir": s.data_dir,
        "configDir": s.config_dir,
        "window": s.has_ui(),
        "bridgePort": s.bridge_port(),
        "openProject": s.current_id(),
        "commands": registry::commands().len(),
    })
}

/// The facts a bug report needs; no keys, prompts or project content.
pub fn diagnostics(s: &Session) -> Value {
    let settings = s.settings();
    let tools = s.tools_if_found();
    json!({
        "app": "kimchi",
        "version": crate::update::CURRENT,
        "system": crate::diagnostics::os_name(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "install": format!("{:?}", crate::update::current_install()),
        "ffmpeg": tools.as_ref().map(|t| t.ffmpeg.clone()),
        "ffprobe": tools.as_ref().map(|t| t.ffprobe.clone()),
        "dataDir": s.data_dir,
        "configDir": s.config_dir,
        "logs": crate::diagnostics::logs_dir(&s.data_dir),
        "logFile": crate::diagnostics::current_log(),
        "logLevel": std::env::var("RUST_LOG").ok().filter(|v| !v.trim().is_empty()).map(|v| format!("RUST_LOG={v}")).unwrap_or(settings.diagnostics.log_level.clone()),
        "window": s.has_ui(),
        "agentProvider": settings.agent.provider,
        "updates": crate::update::status(s),
        "crashReports": crate::diagnostics::reports(&s.data_dir).into_iter().take(5).collect::<Vec<_>>(),
    })
}

impl Session {
    /// Like [`Session::update_settings`], for a change that can be refused.
    pub fn update_settings_checked(&self, f: impl FnOnce(&mut crate::settings::Settings) -> Result<(), String>) -> CmdResult<crate::settings::Settings> {
        let mut result = Ok(());
        let s = self.update_settings(|st| {
            let mut copy = st.clone();
            result = f(&mut copy);
            if result.is_ok() {
                *st = copy;
            }
        })?;
        result.map(|()| s)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::registry;
    use crate::session::{Session, SessionOptions, Source};

    fn session(dir: &std::path::Path) -> std::sync::Arc<Session> {
        Session::new(SessionOptions { data_dir: Some(dir.join("data")), config_dir: Some(dir.join("config")), secrets: None, headless: true }).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_notes_logs_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path());
        let v = registry::call(&s, Source::Cli, "app.whatsNew", json!({})).await.unwrap();
        assert_eq!(v["releases"][0]["version"], crate::update::CURRENT);
        assert!(v["releases"][0]["notes"].as_str().is_some_and(|n| !n.is_empty()));
        let all = registry::call(&s, Source::Cli, "app.whatsNew", json!({ "all": true })).await.unwrap();
        assert!(all["releases"].as_array().unwrap().len() > 1);
        assert!(registry::call(&s, Source::Cli, "app.whatsNew", json!({ "version": "0.0.1" })).await.is_err());

        // No log written by this headless test session, and no crash yet.
        let e = registry::call(&s, Source::Cli, "app.logs", json!({ "file": "nope.log" })).await.unwrap_err();
        assert!(e.contains("No log file"), "{e}");
        let r = registry::call(&s, Source::Cli, "app.crashReports", json!({})).await.unwrap();
        assert_eq!(r["reports"], json!([]));
        assert!(registry::call(&s, Source::Cli, "app.crashReports", json!({ "id": "../settings.json" })).await.is_err());

        let d = registry::call(&s, Source::Cli, "app.diagnostics", json!({})).await.unwrap();
        assert_eq!(d["version"], crate::update::CURRENT);
        assert!(d["system"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn settings_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path());
        s.update_settings(|st| st.agent.permissions.settings = true).unwrap();
        // A leading dot doesn't get an agent past the guard on its own permissions.
        for key in [".agent.permissions.appControl", "agent..permissions.files", " agent.provider"] {
            let e = registry::call(&s, Source::Agent, "app.setSetting", json!({ "key": key, "value": true })).await.unwrap_err();
            assert!(e.contains("stay with the person"), "{key}: {e}");
        }
        let e = registry::call(&s, Source::Cli, "app.setSetting", json!({ "key": "diagnostics.logLevel", "value": "loud" })).await.unwrap_err();
        assert!(e.contains("\"trace\""), "{e}");
        registry::call(&s, Source::Cli, "app.setSetting", json!({ "key": "diagnostics.logLevel", "value": "trace" })).await.unwrap();
        registry::call(&s, Source::Agent, "app.setSetting", json!({ "key": "updates.autoInstall", "value": true })).await.unwrap();
        assert!(s.settings().updates.auto_install);
        assert_eq!(s.settings().diagnostics.log_level, "trace");
    }
}
