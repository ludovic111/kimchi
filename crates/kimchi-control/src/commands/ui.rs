//! `ui.*`: what the window shows. Everything but `ui.state` is carried out by
//! the window itself (see `Session::ui_call`); ids and names are resolved here
//! so the window only ever receives ids.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::registry::{Args, Ctx, Perm};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub const PANELS: &[&str] = &["media", "generate", "text", "motion", "captions", "agent", "jobs", "settings", "export", "palette", "home", "shortcuts", "whatsNew", "diagnostics"];

/// The window's shortcut and menu actions `ui.action` runs, each with the permission an agent
/// needs for it: what the action ends up doing (it runs as the window, so the registry wouldn't
/// check it again).
pub const ACTIONS: &[(&str, Perm)] = &[
    ("PlayPause", Perm::Edit),
    ("ShuttleBack", Perm::Edit),
    ("ShuttleStop", Perm::Edit),
    ("ShuttleForward", Perm::Edit),
    ("ToggleLoop", Perm::Edit),
    ("StepBack", Perm::Edit),
    ("StepForward", Perm::Edit),
    ("StepBackSecond", Perm::Edit),
    ("StepForwardSecond", Perm::Edit),
    ("PrevEdit", Perm::Edit),
    ("NextEdit", Perm::Edit),
    ("GoToStart", Perm::Edit),
    ("GoToEnd", Perm::Edit),
    ("Undo", Perm::Edit),
    ("Redo", Perm::Edit),
    ("CopyClips", Perm::Edit),
    ("CutClips", Perm::Edit),
    ("PasteClips", Perm::Edit),
    ("Duplicate", Perm::Edit),
    ("Split", Perm::Edit),
    ("TrimStart", Perm::Edit),
    ("TrimEnd", Perm::Edit),
    ("NudgeLeft", Perm::Edit),
    ("NudgeRight", Perm::Edit),
    ("NudgeLeftMore", Perm::Edit),
    ("NudgeRightMore", Perm::Edit),
    ("Delete", Perm::Edit),
    ("RippleDelete", Perm::Edit),
    ("SelectAll", Perm::Edit),
    ("Deselect", Perm::Edit),
    ("AddText", Perm::Edit),
    ("AddMarker", Perm::Edit),
    ("ToggleSnap", Perm::Edit),
    ("ZoomIn", Perm::Edit),
    ("ZoomOut", Perm::Edit),
    ("ZoomFit", Perm::Edit),
    ("Palette", Perm::Edit),
    ("FocusGenerate", Perm::Edit),
    ("ShowMedia", Perm::Edit),
    ("ShowGenerate", Perm::Edit),
    ("ShowText", Perm::Edit),
    ("ShowMotion", Perm::Edit),
    ("ShowCaptions", Perm::Edit),
    ("ToggleAgent", Perm::Edit),
    ("ToggleJobs", Perm::Edit),
    ("ShowShortcuts", Perm::Edit),
    ("OpenSettings", Perm::Edit),
    ("WhatsNew", Perm::Edit),
    ("ShowDiagnostics", Perm::Edit),
    ("About", Perm::Edit),
    ("Save", Perm::Edit),
    ("CheckUpdates", Perm::Edit),
    ("OpenHelp", Perm::Edit),
    ("OpenSupport", Perm::Edit),
    ("ReportProblem", Perm::Edit),
    ("Import", Perm::Files),
    ("Export", Perm::Files),
    ("NewProject", Perm::Projects),
    ("CloseProject", Perm::Projects),
    ("ToggleTheme", Perm::Settings),
    ("RestartApp", Perm::AppControl),
    ("Quit", Perm::AppControl),
];

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
        "ui.action" => {
            let name = a.str("action")?;
            let bare = name.trim().trim_start_matches("kimchi::");
            let Some(&(action, perm)) = ACTIONS.iter().find(|(n, _)| n.eq_ignore_ascii_case(bare)) else {
                let names: Vec<&str> = ACTIONS.iter().map(|(n, _)| *n).collect();
                let hint = crate::registry::closest(bare, &names).map(|c| format!(" Did you mean {c}?")).unwrap_or_default();
                return Err(format!("Unknown action `{name}`.{hint} Actions: {}.", names.join(", ")));
            };
            // The action runs as the window: hold an agent to what it does.
            let as_spec = crate::registry::Spec { name: action, perm, ..*cx.spec };
            crate::registry::allowed(s, cx.source, &as_spec)?;
            s.ui_call(cx.spec.name, json!({ "action": action })).await
        }
        "ui.setTimeline" | "ui.setLayout" => s.ui_call(cx.spec.name, Value::Object(a.0)).await,
        "ui.reveal" => {
            let path = match (a.opt_str("path"), a.opt_str("assetId")) {
                (Some(p), None) => std::path::PathBuf::from(p),
                (None, Some(k)) => {
                    let p = s.project()?;
                    let id = resolve::asset(&p, k)?;
                    std::path::PathBuf::from(&p.asset(id).ok_or("media not found")?.path)
                }
                _ => return Err("give path or assetId".into()),
            };
            if !path.exists() {
                return Err(format!("{} doesn't exist", path.display()));
            }
            s.ui_call(cx.spec.name, json!({ "path": path })).await
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
