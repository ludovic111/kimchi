use std::sync::Arc;

use kimchi_core::Edit;
use serde_json::json;

use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "timeline.seek" | "timeline.play" | "timeline.pause" => s.ui_call(cx.spec.name, serde_json::Value::Object(a.0)).await,
        "timeline.closeGap" => {
            let id = resolve::track(&s.project()?, a.str("trackId")?)?;
            s.apply(cx.label(), cx.source, &Edit::CloseGap { track_id: id, time: a.f64("time")? }, None)?;
            Ok(json!({ "trackId": id }))
        }
        "timeline.markers" => Ok(json!(s.read(|ed| ed.project().markers.clone())?)),
        "timeline.addMarker" => {
            let time = a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead);
            let label = a.opt_str("label").unwrap_or("").to_string();
            s.apply(cx.label(), cx.source, &Edit::AddMarker { time, label: label.clone() }, None)?;
            let marker = s.read(|ed| {
                ed.project().markers.iter().rfind(|m| m.label == label && (m.time - time.max(0.0)).abs() < 1e-9).cloned()
            })?;
            Ok(json!(marker))
        }
        "timeline.removeMarker" => {
            let id = resolve::marker(&s.project()?, a.str("markerId")?)?;
            s.apply(cx.label(), cx.source, &Edit::RemoveMarker { marker_id: id }, None)?;
            Ok(json!({ "removed": id }))
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}
