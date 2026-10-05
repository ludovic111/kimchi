use std::sync::Arc;

use kimchi_core::{ChromaKey, Clip, ClipContent, ClipMove, ClipPatch, Edge, Edit, Effects, Fit, Id, Lut, Project, TextAlign, TextStyle, TrackClip};
use serde_json::{Map, Value, json};

use crate::commands::project::clip_summary;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "clip.list" => {
            let p = s.project()?;
            let only = a.opt_str("trackId").map(|k| resolve::track(&p, k)).transpose()?;
            Ok(json!(p.tracks.iter().filter(|t| only.is_none_or(|id| id == t.id)).flat_map(|t| t.clips.iter().map(|c| {
                let mut v = clip_summary(&p, c);
                v["trackId"] = json!(t.id);
                v["track"] = json!(t.name);
                v
            }).collect::<Vec<_>>()).collect::<Vec<_>>()))
        }
        "clip.get" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let (ti, _) = p.locate_clip(id).ok_or("clip not found")?;
            let mut v = json!(p.clip(id));
            v["trackId"] = json!(p.tracks[ti].id);
            v["track"] = json!(p.tracks[ti].name);
            Ok(v)
        }
        "clip.insertMedia" => {
            let p = s.project()?;
            let asset_id = resolve::asset(&p, a.str("assetId")?)?;
            let track_id = opt_track(&p, &a)?;
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
            let out = s.apply(cx.label(), cx.source, &Edit::InsertAsset { asset_id, track_id, start }, None)?;
            created(s, &out.created_clips)
        }
        "clip.addText" => {
            let p = s.project()?;
            let mut style = default_text();
            style.content = a.str("text")?.to_string();
            if let Some(o) = a.object("style") {
                patch_text(&mut style, o)?;
            }
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
            let mut clip = Clip::new(text_name(&style.content), start, a.opt_f64("duration").unwrap_or(4.0), ClipContent::Text { style });
            clip.fade_in = 0.2;
            clip.fade_out = 0.2;
            clip.transform.x = a.opt_f64("x").unwrap_or(0.0);
            clip.transform.y = a.opt_f64("y").unwrap_or(0.0);
            let out = s.apply(cx.label(), cx.source, &Edit::AddClip { track_id: opt_track(&p, &a)?, clip }, None)?;
            created(s, &out.created_clips)
        }
        "clip.addSolid" => {
            let p = s.project()?;
            let color = color(a.str("color")?)?;
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
            let clip = Clip::new("Solid", start, a.opt_f64("duration").unwrap_or(5.0), ClipContent::Solid { color });
            let out = s.apply(cx.label(), cx.source, &Edit::AddClip { track_id: opt_track(&p, &a)?, clip }, None)?;
            created(s, &out.created_clips)
        }
        "clip.move" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let (ti, ci) = p.locate_clip(id).ok_or("clip not found")?;
            let track_id = match a.opt_str("trackId") {
                Some(k) => resolve::track(&p, k)?,
                None => p.tracks[ti].id,
            };
            let start = a.opt_f64("start").unwrap_or(p.tracks[ti].clips[ci].start);
            s.apply(cx.label(), cx.source, &Edit::MoveClips { moves: vec![ClipMove { clip_id: id, track_id, start }] }, a.coalesce())?;
            one(s, id)
        }
        "clip.moveMany" => {
            let p = s.project()?;
            let mut moves = vec![];
            for (i, m) in a.array("moves").into_iter().flatten().enumerate() {
                let key = m.get("clipId").and_then(Value::as_str).ok_or_else(|| format!("moves[{i}] needs clipId"))?;
                let id = resolve::clip(&p, key)?;
                let (ti, ci) = p.locate_clip(id).ok_or("clip not found")?;
                let track_id = match m.get("trackId").and_then(Value::as_str) {
                    Some(k) => resolve::track(&p, k)?,
                    None => p.tracks[ti].id,
                };
                let start = m.get("start").and_then(Value::as_f64).unwrap_or(p.tracks[ti].clips[ci].start);
                moves.push(ClipMove { clip_id: id, track_id, start });
            }
            s.apply(cx.label(), cx.source, &Edit::MoveClips { moves }, a.coalesce())?;
            Ok(json!({ "moved": a.array("moves").map(Vec::len).unwrap_or(0) }))
        }
        "clip.trim" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let edge = match a.str("edge")? {
                "start" | "in" | "left" => Edge::Start,
                "end" | "out" | "right" => Edge::End,
                other => return Err(format!("edge is \"start\" or \"end\", not \"{other}\"")),
            };
            s.apply(cx.label(), cx.source, &Edit::TrimClip { clip_id: id, edge, time: a.f64("time")? }, a.coalesce())?;
            one(s, id)
        }
        "clip.split" => {
            let p = s.project()?;
            let ui = s.ui_state();
            let time = a.opt_f64("time").unwrap_or(ui.playhead);
            let ids = match a.array("clipIds") {
                Some(_) => Some(resolve::clips(&p, &a.strings("clipIds"))?),
                // The window splits its selection; everyone else splits whatever is under the time.
                None if cx.source == crate::session::Source::Window && !ui.selection.is_empty() => Some(ui.selection.clone()),
                None => None,
            };
            let out = s.apply(cx.label(), cx.source, &Edit::Split { time, clip_ids: ids }, None)?;
            Ok(json!({ "time": time, "created": out.created_clips }))
        }
        "clip.delete" => {
            let p = s.project()?;
            let ids = resolve::clips(&p, &a.strings("clipIds"))?;
            // The clips as they were, with their track: clip.paste puts them back (a cut).
            let removed: Vec<Value> = ids
                .iter()
                .filter_map(|id| {
                    let (ti, ci) = p.locate_clip(*id)?;
                    let mut v = json!(p.tracks[ti].clips[ci]);
                    v["trackId"] = json!(p.tracks[ti].id);
                    Some(v)
                })
                .collect();
            s.apply(cx.label(), cx.source, &Edit::DeleteClips { clip_ids: ids.clone(), ripple: a.bool_or("ripple", false) }, None)?;
            Ok(json!({ "deleted": ids, "removed": removed }))
        }
        "clip.duplicate" => {
            let p = s.project()?;
            let ids = resolve::clips(&p, &a.strings("clipIds"))?;
            let out = s.apply(cx.label(), cx.source, &Edit::DuplicateClips { clip_ids: ids }, None)?;
            created(s, &out.created_clips)
        }
        "clip.paste" => {
            let p = s.project()?;
            let mut clips: Vec<TrackClip> = vec![];
            for key in a.strings("clipIds") {
                let id = resolve::clip(&p, &key)?;
                let (ti, ci) = p.locate_clip(id).ok_or("clip not found")?;
                clips.push(TrackClip { track_id: p.tracks[ti].id, clip: p.tracks[ti].clips[ci].clone() });
            }
            for (i, v) in a.array("clips").into_iter().flatten().enumerate() {
                let track = v.get("trackId").and_then(Value::as_str).ok_or_else(|| format!("clips[{i}] needs trackId"))?;
                let track_id = resolve::track(&p, track)?;
                let clip: Clip = serde_json::from_value(v.clone()).map_err(|e| format!("clips[{i}] isn't a clip: {e}"))?;
                clips.push(TrackClip { track_id, clip });
            }
            if clips.is_empty() {
                return Err("give clipIds or clips to paste".into());
            }
            let time = a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead).max(0.0);
            let earliest = clips.iter().map(|c| c.clip.start).fold(f64::INFINITY, f64::min);
            let only = opt_track(&p, &a)?;
            for c in &mut clips {
                c.clip.start = c.clip.start - earliest + time;
                if let Some(t) = only {
                    c.track_id = t;
                }
            }
            let out = s.apply(cx.label(), cx.source, &Edit::PasteClips { clips }, None)?;
            created(s, &out.created_clips)
        }
        "clip.update" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(id).ok_or("clip not found")?;
            let patch = patch_of(clip, &a)?;
            s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id: id, patch }, a.coalesce())?;
            one(s, id)
        }
        "clip.setKeyframes" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(id).ok_or("clip not found")?;
            let property = a.str("property")?;
            let keys = crate::commands::motion::keyframes(&a)?;
            for k in &keys {
                kimchi_core::check_clip_key(&clip.content, property, &k.value)?;
            }
            let mut all = clip.keyframes.clone();
            if keys.is_empty() {
                all.remove(property);
            } else {
                all.insert(property.to_string(), keys);
            }
            keyframes_patch(s, cx, &a, id, all)
        }
        "clip.addKeyframe" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(id).ok_or("clip not found")?;
            let property = a.str("property")?;
            let time = a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead);
            let value = match a.get("value") {
                Some(v) => serde_json::from_value::<kimchi_core::KeyValue>(v.clone()).map_err(|_| format!("value {v} should be a number, [x, y] or a colour"))?,
                None => crate::commands::motion::current_value(clip, property, time)?,
            };
            kimchi_core::check_clip_key(&clip.content, property, &value)?;
            let easing = a.opt_str("easing").map(kimchi_core::Easing::parse).transpose()?.unwrap_or_default();
            let mut all = clip.keyframes.clone();
            kimchi_core::anim::set_key(&mut all, property, kimchi_core::Keyframe { time: time - clip.start, value, easing });
            keyframes_patch(s, cx, &a, id, all)
        }
        "clip.removeKeyframe" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(id).ok_or("clip not found")?;
            let property = a.str("property")?;
            if !clip.keyframes.contains_key(property) {
                return Err(format!("\"{}\" has no keyframes on `{property}`. Animated: {}.", clip.name, clip.keyframes.keys().cloned().collect::<Vec<_>>().join(", ")));
            }
            let mut all = clip.keyframes.clone();
            match a.opt_f64("time") {
                Some(t) => {
                    if !kimchi_core::anim::remove_key(&mut all, property, t - clip.start) {
                        return Err(format!("No `{property}` keyframe at {t} s on \"{}\".", clip.name));
                    }
                }
                None => {
                    // Keep the property where it was at the playhead rather than snapping back.
                    let now = s.ui_state().playhead.clamp(clip.start, clip.end());
                    let held = crate::commands::motion::current_value(clip, property, now)?;
                    all.remove(property);
                    return keyframes_and_value(s, cx, &a, id, clip, all, property, held);
                }
            }
            keyframes_patch(s, cx, &a, id, all)
        }
        "clip.animate" => {
            let p = s.project()?;
            let ids = resolve::clips(&p, &a.strings("clipIds"))?;
            let preset = a.str("preset")?;
            let canvas = (p.settings.width as f64, p.settings.height as f64);
            let mut edits = vec![];
            for id in &ids {
                let clip = p.clip(*id).ok_or("clip not found")?;
                let keys = kimchi_core::presets::apply(preset, clip, canvas, a.opt_f64("length"))?;
                edits.push(Edit::UpdateClip { clip_id: *id, patch: ClipPatch { keyframes: Some(keys), ..Default::default() } });
            }
            apply_all(s, cx, &edits, None)?;
            created(s, &ids)
        }
        "clip.setEffects" => {
            let p = s.project()?;
            let ids = resolve::clips(&p, &a.strings("clipIds"))?;
            if ids.is_empty() {
                return Err("`clipIds` is empty".into());
            }
            let mut edits = vec![];
            for id in &ids {
                let clip = p.clip(*id).ok_or("clip not found")?;
                let effects = effects_of(clip, &a)?;
                edits.push(Edit::UpdateClip { clip_id: *id, patch: ClipPatch { effects: Some(effects), ..Default::default() } });
            }
            apply_all(s, cx, &edits, a.coalesce())?;
            // The summaries show the effects (none once they are all off).
            created(s, &ids)
        }
        "clip.looks" => Ok(json!(kimchi_core::effects::LOOKS.iter().map(|l| {
            let values: Map<String, Value> = l.values.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
            json!({ "id": l.id, "label": l.label, "description": l.doc, "values": values })
        }).collect::<Vec<_>>())),
        "clip.freezeFrame" => freeze_frame(s, cx, &a).await,
        _ => Err(crate::commands::unhandled(cx)),
    }
}

/// Applies edits as one undo step (one edit keeps its coalesce key, so a slider drag over one
/// clip folds into one step).
pub(crate) fn apply_all(s: &Arc<Session>, cx: &Ctx, edits: &[Edit], coalesce: Option<&str>) -> CmdResult<()> {
    if let [one] = edits {
        s.apply(cx.label(), cx.source, one, coalesce)?;
        return Ok(());
    }
    s.edit(cx.label(), cx.source, |ed| {
        ed.begin_batch(cx.label(), cx.source.as_str());
        for e in edits {
            if let Err(e) = ed.apply(e, None) {
                ed.rollback_batch();
                return Err(crate::session::err(e));
            }
        }
        ed.end_batch();
        Ok(())
    })
}

/// The effects `clip.setEffects` asks for, on top of the clip's (or none, with reset).
fn effects_of(clip: &Clip, a: &Args) -> CmdResult<Effects> {
    let mut e = if a.bool_or("reset", false) { Effects::default() } else { clip.effects.clone() };
    if let Some(look) = a.opt_str("look") {
        e = kimchi_core::effects::apply_look(&e, look)?;
    }
    for name in kimchi_core::effects::EFFECT_PROPS {
        if let Some(v) = a.get(name) {
            let v = v.as_f64().ok_or_else(|| format!("{name} should be a number"))?;
            e.set(name, v);
        }
    }
    match a.get("chromaKey") {
        None => {}
        Some(Value::Null) | Some(Value::Bool(false)) => e.chroma_key = None,
        Some(Value::Bool(true)) => e.chroma_key = Some(e.chroma_key.take().unwrap_or_default()),
        Some(Value::String(c)) => e.chroma_key = Some(ChromaKey { color: color(c)?, ..e.chroma_key.take().unwrap_or_default() }),
        Some(Value::Object(o)) => {
            let mut k = e.chroma_key.take().unwrap_or_default();
            for (field, v) in o {
                match field.as_str() {
                    "color" | "colour" => k.color = color(v.as_str().ok_or("chromaKey.color should be #rrggbb")?)?,
                    "similarity" | "softness" | "spill" => {
                        let n = v.as_f64().ok_or_else(|| format!("chromaKey.{field} should be a number from 0 to 1"))?;
                        match field.as_str() {
                            "similarity" => k.similarity = n,
                            "softness" => k.softness = n,
                            _ => k.spill = n,
                        }
                    }
                    other => return Err(format!("Unknown chromaKey field `{other}`. Fields: color, similarity, softness, spill.")),
                }
            }
            e.chroma_key = Some(k);
        }
        Some(other) => return Err(format!("chromaKey takes true, false, a colour or {{color, similarity, softness, spill}}, not {other}")),
    }
    match a.get("lut") {
        None => {}
        Some(Value::Null) | Some(Value::Bool(false)) => e.lut = None,
        Some(v) => {
            let (path, strength) = match v {
                Value::String(p) => (p.clone(), None),
                Value::Object(o) => (
                    o.get("path").and_then(Value::as_str).ok_or("lut needs a path")?.to_string(),
                    o.get("strength").and_then(Value::as_f64),
                ),
                other => return Err(format!("lut takes the path of a .cube file, not {other}")),
            };
            let path = std::path::absolute(&path).map_err(|e| format!("{path}: {e}"))?.to_string_lossy().into_owned();
            kimchi_media::render::grade::cube(std::path::Path::new(&path)).map_err(|e| format!("Can't use {path} as a LUT: {e}"))?;
            e.lut = Some(Lut { path, strength: strength.unwrap_or(1.0) });
        }
    }
    if let Some(v) = a.opt_f64("lutStrength") {
        let lut = e.lut.as_mut().ok_or(format!("\"{}\" has no LUT to set the strength of; give lut too.", clip.name))?;
        lut.strength = v;
    }
    Ok(e.clamped())
}

/// `clip.freezeFrame`: split, push the rest of the track later, and hold a still of the frame.
async fn freeze_frame(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let id = resolve::clip(&p, a.str("clipId")?)?;
    let (ti, ci) = p.locate_clip(id).ok_or("clip not found")?;
    let (track, clip) = (&p.tracks[ti], p.tracks[ti].clips[ci].clone());
    if track.kind != kimchi_core::TrackKind::Video || matches!(clip.content, ClipContent::Pending { .. }) {
        return Err(format!("\"{}\" has no picture to hold.", clip.name));
    }
    let frame = p.settings.frame();
    let time = a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead);
    let time = p.settings.snap_to_frame(time).clamp(clip.start, (clip.end() - frame).max(clip.start));
    let hold = a.opt_f64("duration").unwrap_or(2.0);
    if hold.is_nan() || hold <= 0.0 {
        return Err("duration should be more than 0 seconds".into());
    }
    let png = crate::commands::media::clip_frame(s, id, Some(time)).await?;
    let still = match clip.content {
        // A still keeps its own picture: the hold is a copy of the clip.
        ClipContent::Media { asset_id } if p.asset(asset_id).is_some_and(|x| x.kind == kimchi_core::MediaKind::Image) => asset_id,
        // Named for the person (the cached frame's file is named by ids).
        _ => {
            let name = format!("{} · frame at {:.2} s", clip.name, time);
            crate::commands::media::import_named(s, cx, &[png], Some(&name)).await?.first().ok_or("the frame couldn't be read")?.id
        }
    };
    let mut held = Clip::new(format!("{} (hold)", clip.name), time, hold, ClipContent::Media { asset_id: still });
    // Text, solids and scenes were drawn already placed on the canvas; media keep the clip's place.
    if matches!(clip.content, ClipContent::Media { .. }) {
        held.transform = clip.placement_at(time).transform();
        held.transform.fit = clip.transform.fit;
        held.effects = clip.effects_at(time);
    } else {
        held.transform.fit = kimchi_core::Fit::Stretch;
    }
    let track_id = track.id;
    // Everything from the frame on moves later by the hold (the clip itself when it is its
    // first frame, else the right half of the split).
    let later: Vec<ClipMove> = track.clips.iter().filter(|c| c.start >= time - 1e-6).map(|c| ClipMove { clip_id: c.id, track_id, start: c.start + hold }).collect();
    let split = time > clip.start + 1e-6;
    let mut made = vec![];
    s.edit(cx.label(), cx.source, |ed| {
        ed.begin_batch(cx.label(), cx.source.as_str());
        let mut steps = || -> Result<Vec<Id>, kimchi_core::EditError> {
            let mut moves = later.clone();
            if split {
                let right = ed.apply(&Edit::Split { time, clip_ids: Some(vec![id]) }, None)?.created_clips;
                moves.extend(right.into_iter().map(|c| ClipMove { clip_id: c, track_id, start: time + hold }));
            }
            if !moves.is_empty() {
                ed.apply(&Edit::MoveClips { moves }, None)?;
            }
            Ok(ed.apply(&Edit::AddClip { track_id: Some(track_id), clip: held.clone() }, None)?.created_clips)
        };
        match steps() {
            Ok(ids) => {
                made = ids;
                ed.end_batch();
                Ok(())
            }
            Err(e) => {
                ed.rollback_batch();
                Err(crate::session::err(e))
            }
        }
    })?;
    created(s, &made)
}

/// Replaces a clip's keyframes and answers with the clip and its animation.
fn keyframes_patch(s: &Arc<Session>, cx: &Ctx, a: &Args, id: Id, keys: kimchi_core::Keyframes) -> CmdResult {
    let patch = ClipPatch { keyframes: Some(keys), ..Default::default() };
    s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id: id, patch }, a.coalesce())?;
    animated(s, id)
}

/// Like [`keyframes_patch`], also setting the property's still value (when its animation goes).
#[allow(clippy::too_many_arguments)]
fn keyframes_and_value(s: &Arc<Session>, cx: &Ctx, a: &Args, id: Id, clip: &Clip, keys: kimchi_core::Keyframes, property: &str, value: kimchi_core::KeyValue) -> CmdResult {
    let mut patch = ClipPatch { keyframes: Some(keys), ..Default::default() };
    let n = value.as_f64();
    let mut t = clip.transform.clone();
    match (property, n) {
        ("x", Some(v)) => t.x = v,
        ("y", Some(v)) => t.y = v,
        ("scale", Some(v)) => t.scale = v,
        ("rotation", Some(v)) => t.rotation = v,
        ("opacity", Some(v)) => t.opacity = v,
        ("volume", Some(v)) => patch.volume = Some(v),
        ("pan", Some(v)) => {
            let mut audio = clip.audio.clone();
            audio.pan = v.clamp(-1.0, 1.0);
            patch.audio = Some(audio);
        }
        ("position", _) => {
            if let Some(p) = value.as_vec(2) {
                (t.x, t.y) = (p[0], p[1]);
            }
        }
        _ => {}
    }
    if t != clip.transform {
        patch.transform = Some(t);
    }
    if let (Some(v), true) = (n, kimchi_core::effects::EFFECT_PROPS.contains(&property)) {
        let mut e = clip.effects.clone();
        e.set(property, v);
        patch.effects = Some(e);
    }
    if let (ClipContent::Text { style }, true) = (&clip.content, ["fontSize", "letterSpacing", "color"].contains(&property)) {
        let mut style = style.clone();
        match (property, &value) {
            ("fontSize", kimchi_core::KeyValue::Number(v)) => style.font_size = *v,
            ("letterSpacing", kimchi_core::KeyValue::Number(v)) => style.letter_spacing = *v,
            ("color", kimchi_core::KeyValue::Text(c)) => style.color = c.clone(),
            _ => {}
        }
        patch.text = Some(style);
    }
    s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id: id, patch }, a.coalesce())?;
    animated(s, id)
}

fn animated(s: &Session, id: Id) -> CmdResult {
    s.read(|ed| {
        let p = ed.project();
        let Some(c) = p.clip(id) else { return Value::Null };
        let mut v = clip_summary(p, c);
        v["keyframes"] = crate::commands::motion::keys_json(&c.keyframes);
        v
    })
}

fn opt_track(p: &Project, a: &Args) -> CmdResult<Option<Id>> {
    a.opt_str("trackId").map(|k| resolve::track(p, k)).transpose()
}

fn one(s: &Session, id: Id) -> CmdResult {
    s.read(|ed| ed.project().clip(id).map(|c| clip_summary(ed.project(), c)).unwrap_or(Value::Null))
}

fn created(s: &Session, ids: &[Id]) -> CmdResult {
    s.read(|ed| {
        let p = ed.project();
        json!({ "clips": ids.iter().filter_map(|id| p.clip(*id)).map(|c| clip_summary(p, c)).collect::<Vec<_>>() })
    })
}

/// The patch `clip.update` asks for, built on the clip's current values.
pub fn patch_of(clip: &Clip, a: &Args) -> CmdResult<ClipPatch> {
    let mut patch = ClipPatch { name: a.opt_str("name").map(str::to_string), ..Default::default() };
    if ["x", "y", "scale", "rotation", "opacity", "fit"].iter().any(|k| a.has(k)) {
        let mut t = clip.transform.clone();
        t.x = a.opt_f64("x").unwrap_or(t.x);
        t.y = a.opt_f64("y").unwrap_or(t.y);
        t.scale = a.opt_f64("scale").unwrap_or(t.scale);
        t.rotation = a.opt_f64("rotation").unwrap_or(t.rotation);
        t.opacity = a.opt_f64("opacity").unwrap_or(t.opacity);
        if let Some(f) = a.opt_str("fit") {
            t.fit = match f {
                "contain" => Fit::Contain,
                "cover" => Fit::Cover,
                "stretch" => Fit::Stretch,
                other => return Err(format!("fit is contain, cover or stretch, not \"{other}\"")),
            };
        }
        patch.transform = Some(t);
    }
    patch.volume = a.opt_f64("volume");
    patch.fade_in = a.opt_f64("fadeIn");
    patch.fade_out = a.opt_f64("fadeOut");
    patch.speed = a.opt_f64("speed");
    patch.reverse = a.opt_bool("reverse");
    if let Some(o) = a.object("style") {
        let ClipContent::Text { style } = &clip.content else { return Err(format!("\"{}\" isn't a text clip, so it has no style.", clip.name)) };
        let mut style = style.clone();
        patch_text(&mut style, o)?;
        patch.text = Some(style);
    }
    if let Some(c) = a.opt_str("color") {
        if !matches!(clip.content, ClipContent::Solid { .. }) {
            return Err(format!("\"{}\" isn't a solid clip, so it has no colour.", clip.name));
        }
        patch.color = Some(color(c)?);
    }
    Ok(patch)
}

/// The style new titles start with.
pub fn default_text() -> TextStyle {
    TextStyle {
        content: "Your title".into(),
        font_family: "Manrope".into(),
        font_size: 120.0,
        font_weight: 700,
        italic: false,
        color: "#ffffff".into(),
        background: None,
        align: TextAlign::Center,
        line_height: 1.1,
        letter_spacing: -1.0,
        shadow: true,
    }
}

pub fn text_name(content: &str) -> String {
    let first: String = content.lines().next().unwrap_or("").chars().take(32).collect();
    if first.trim().is_empty() { "Text".into() } else { first }
}

/// Applies `{fontSize: 80, color: "#fff", …}` (camelCase or snake_case) to a text style.
pub fn patch_text(style: &mut TextStyle, o: &Map<String, Value>) -> CmdResult<()> {
    for (k, v) in o {
        let key: String = k.chars().filter(|c| *c != '_').collect::<String>().to_ascii_lowercase();
        let num = || v.as_f64().ok_or_else(|| format!("style.{k} should be a number"));
        let text = || v.as_str().map(str::to_string).ok_or_else(|| format!("style.{k} should be a string"));
        match key.as_str() {
            "content" | "text" => style.content = text()?,
            "fontfamily" | "font" => style.font_family = text()?,
            "fontsize" | "size" => style.font_size = num()?.clamp(4.0, 2000.0),
            "fontweight" | "weight" => style.font_weight = num()?.clamp(100.0, 900.0) as u16,
            "italic" => style.italic = v.as_bool().ok_or("style.italic should be true or false")?,
            "color" => style.color = color(&text()?)?,
            "background" => style.background = if v.is_null() { None } else { Some(color(&text()?)?) },
            "align" => {
                style.align = match text()?.as_str() {
                    "left" => TextAlign::Left,
                    "center" | "centre" => TextAlign::Center,
                    "right" => TextAlign::Right,
                    other => return Err(format!("style.align is left, center or right, not \"{other}\"")),
                }
            }
            "lineheight" => style.line_height = num()?.clamp(0.5, 4.0),
            "letterspacing" => style.letter_spacing = num()?,
            "shadow" => style.shadow = v.as_bool().ok_or("style.shadow should be true or false")?,
            _ => {
                return Err(format!(
                    "Unknown text style field `{k}`. Fields: fontFamily, fontSize, fontWeight, italic, color, background, align, lineHeight, letterSpacing, shadow."
                ));
            }
        }
    }
    Ok(())
}

/// Accepts `#rgb`, `#rrggbb` and `#rrggbbaa`; returns lowercase.
pub fn color(c: &str) -> CmdResult<String> {
    let c = c.trim();
    let hex = c.strip_prefix('#').unwrap_or(c);
    let ok = matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit());
    if !ok {
        return Err(format!("\"{c}\" isn't a colour; use #rrggbb."));
    }
    let hex = if hex.len() == 3 { hex.chars().flat_map(|ch| [ch, ch]).collect() } else { hex.to_string() };
    Ok(format!("#{}", hex.to_ascii_lowercase()))
}
