//! `ui.*`: what the window shows. Everything but `ui.state` is carried out by
//! the window itself (see `Session::ui_call`); ids and names are resolved here
//! so the window only ever receives ids.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub const PANELS: &[&str] = &["media", "generate", "text", "motion", "captions", "agent", "jobs", "settings", "export", "palette", "home", "shortcuts", "whatsNew", "diagnostics"];

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "ui.state" => {
            if !s.has_ui() {
                return Ok(json!({ "window": false }));
            }
            let mut v = json!(s.ui_state());
            v["window"] = json!(true);
            Ok(v)
        }
        "ui.select" => {
            let mut params = a.0.clone();
            if a.has("clipIds") {
                let p = s.project()?;
                params.insert("clipIds".into(), json!(resolve::clips(&p, &a.strings("clipIds"))?));
            }
            if let Some(k) = a.opt_str("assetId") {
                let p = s.project()?;
                params.insert("assetId".into(), json!(resolve::asset(&p, k)?));
            }
            s.ui_call(cx.spec.name, Value::Object(params)).await
        }
        "ui.showPanel" => {
            let panel = a.str("panel")?;
            if !PANELS.contains(&panel) {
                let hint = crate::registry::closest(panel, PANELS).map(|c| format!(" Did you mean {c}?")).unwrap_or_default();
                return Err(format!("Unknown panel `{panel}`.{hint} Panels: {}.", PANELS.join(", ")));
            }
            s.ui_call(cx.spec.name, Value::Object(a.0)).await
        }
        "ui.zoom" => {
            if let Some(p) = a.opt_f64("pixelsPerSecond")
                && !(4.0..=600.0).contains(&p)
            {
                return Err("pixelsPerSecond goes from 4 to 600".into());
            }
            s.ui_call(cx.spec.name, Value::Object(a.0)).await
        }
        "ui.closeDialogs" | "ui.screenshot" => s.ui_call(cx.spec.name, Value::Object(a.0)).await,
        _ => Err(crate::commands::unhandled(cx)),
    }
}
