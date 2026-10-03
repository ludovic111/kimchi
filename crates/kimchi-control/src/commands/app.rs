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
            let key = a.str("key")?.to_string();
            if cx.source.is_agent() && key.starts_with("agent") {
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
        "app.quit" | "app.notify" => s.ui_call(cx.spec.name, Value::Object(a.0)).await,
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
