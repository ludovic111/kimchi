use std::sync::Arc;

use kimchi_core::{Edit, TrackKind, TrackPatch};
use serde_json::json;

use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "track.list" => {
            let p = s.project()?;
            Ok(json!(p.tracks.iter().enumerate().map(|(i, t)| json!({
                "id": t.id,
                "index": i,
                "name": t.name,
                "kind": t.kind,
                "muted": t.muted,
                "hidden": t.hidden,
                "locked": t.locked,
                "clips": t.clips.len(),
                "end": crate::commands::project::round(t.end()),
            })).collect::<Vec<_>>()))
        }
        "track.add" => {
            let kind = kind(a.str("kind")?)?;
            let index = a.opt_i64("index").map(|i| i.max(0) as usize);
            let out = s.apply(cx.label(), cx.source, &Edit::AddTrack { kind, index }, None)?;
            Ok(json!({ "trackId": out.created_tracks.first() }))
        }
        "track.remove" => {
            let id = resolve::track(&s.project()?, a.str("trackId")?)?;
            s.apply(cx.label(), cx.source, &Edit::RemoveTrack { track_id: id }, None)?;
            Ok(json!({ "removed": id }))
        }
        "track.update" => {
            let id = resolve::track(&s.project()?, a.str("trackId")?)?;
            let patch = TrackPatch {
                name: a.opt_str("name").map(str::to_string),
                muted: a.opt_bool("muted"),
                hidden: a.opt_bool("hidden"),
                locked: a.opt_bool("locked"),
                captions: a.opt_bool("captions"),
            };
            s.apply(cx.label(), cx.source, &Edit::UpdateTrack { track_id: id, patch }, None)?;
            Ok(json!(s.read(|ed| ed.project().track(id).map(|t| json!({ "id": t.id, "name": t.name, "muted": t.muted, "hidden": t.hidden, "locked": t.locked, "captions": t.captions })))?))
        }
        "track.move" => {
            let id = resolve::track(&s.project()?, a.str("trackId")?)?;
            let index = a.opt_i64("index").unwrap_or(0).max(0) as usize;
            s.apply(cx.label(), cx.source, &Edit::MoveTrack { track_id: id, index }, None)?;
            Ok(json!({ "trackId": id, "index": s.read(|ed| ed.project().tracks.iter().position(|t| t.id == id))? }))
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

pub fn kind(s: &str) -> Result<TrackKind, String> {
    match s.to_ascii_lowercase().as_str() {
        "video" | "picture" | "pictures" => Ok(TrackKind::Video),
        "audio" | "sound" => Ok(TrackKind::Audio),
        other => Err(format!("A track is \"video\" or \"audio\", not \"{other}\".")),
    }
}
