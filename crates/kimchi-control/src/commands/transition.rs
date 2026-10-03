use std::sync::Arc;

use kimchi_core::transition::{self, CUT_TOLERANCE, DEFAULT_LENGTH, KINDS};
use kimchi_core::{ClipPatch, Easing, Edit, Id, Project, Transition, TransitionKind};
use serde_json::{Value, json};

use crate::commands::clip::apply_all;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "transition.kinds" => Ok(json!(KINDS.iter().map(|k| json!({ "kind": k.id, "label": k.label, "description": k.doc })).collect::<Vec<_>>())),
        "transition.list" => Ok(json!(list(&s.project()?))),
        "transition.set" => {
            let p = s.project()?;
            let ids = targets(&p, &a, true)?;
            let kind = a.opt_str("kind").map(TransitionKind::parse).transpose()?;
            let duration = a.opt_f64("duration");
            if duration.is_some_and(|d| d.is_nan() || d <= 0.0) {
                return Err("duration should be more than 0 seconds".into());
            }
            let easing = a.opt_str("easing").map(Easing::parse).transpose()?;
            let edits: Vec<Edit> = ids
                .iter()
                .filter_map(|id| p.clip(*id))
                .map(|c| {
                    // Unset fields keep what a transition already there has.
                    let mut tr = c.transition.clone().unwrap_or_else(|| Transition::new(TransitionKind::Dissolve, DEFAULT_LENGTH));
                    tr.kind = kind.unwrap_or(tr.kind);
                    tr.duration = duration.unwrap_or(tr.duration);
                    tr.easing = easing.unwrap_or(tr.easing);
                    Edit::UpdateClip { clip_id: c.id, patch: ClipPatch { transition: Some(Some(tr)), ..Default::default() } }
                })
                .collect();
            apply_all(s, cx, &edits, a.coalesce())?;
            let p = s.project()?;
            Ok(json!(list(&p).into_iter().filter(|v| ids.iter().any(|id| v["clipId"] == json!(id))).collect::<Vec<_>>()))
        }
        "transition.remove" => {
            let p = s.project()?;
            let ids: Vec<Id> = targets(&p, &a, false)?.into_iter().filter(|id| p.clip(*id).is_some_and(|c| c.transition.is_some())).collect();
            if ids.is_empty() {
                return Err("None of those clips has a transition.".into());
            }
            let edits: Vec<Edit> = ids.iter().map(|id| Edit::UpdateClip { clip_id: *id, patch: ClipPatch { transition: Some(None), ..Default::default() } }).collect();
            apply_all(s, cx, &edits, None)?;
            Ok(json!({ "removed": ids }))
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

/// The clips a command acts on: `clipIds`, or the clips of `trackId` (with `cuts`, only those
/// that start where another ends).
fn targets(p: &Project, a: &Args, cuts: bool) -> CmdResult<Vec<Id>> {
    if a.array("clipIds").is_some() {
        let ids = resolve::clips(p, &a.strings("clipIds"))?;
        if ids.is_empty() {
            return Err("`clipIds` is empty".into());
        }
        return Ok(ids);
    }
    let Some(track) = a.opt_str("trackId") else { return Err("Give clipIds (the clips the transitions lead into) or a trackId.".into()) };
    let t = p.track(resolve::track(p, track)?).ok_or("track not found")?;
    let ids: Vec<Id> = t
        .clips
        .iter()
        .enumerate()
        .filter(|(i, c)| !cuts || i.checked_sub(1).is_some_and(|j| (t.clips[j].end() - c.start).abs() <= CUT_TOLERANCE))
        .map(|(_, c)| c.id)
        .collect();
    if ids.is_empty() {
        return Err(format!("\"{}\" has no {}.", t.name, if cuts { "cuts (clips that start where another ends)" } else { "clips" }));
    }
    Ok(ids)
}

/// Every transition with where it plays.
fn list(p: &Project) -> Vec<Value> {
    let mut out = vec![];
    for t in &p.tracks {
        for span in transition::spans(t) {
            let to = &t.clips[span.to];
            let tr = to.transition.as_ref().expect("a span has a transition");
            let mut v = json!({
                "clipId": to.id,
                "clip": to.name,
                "track": t.name,
                "kind": tr.kind,
                "duration": round(span.duration()),
                "start": round(span.start),
                "end": round(span.end),
            });
            if (span.duration() - tr.duration).abs() > 1e-6 {
                v["asked"] = json!(round(tr.duration));
                v["note"] = json!("shortened to fit the clips");
            }
            match span.from {
                Some(i) => v["from"] = json!(t.clips[i].name),
                None => v["from"] = json!(null),
            }
            out.push(v);
        }
    }
    out
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}
