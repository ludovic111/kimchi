use std::sync::Arc;

use kimchi_core::{Clip, ClipContent, ClipMove, ClipPatch, Edge, Edit, Fit, Id, Project, TextAlign, TextStyle, TrackClip};
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
            s.apply(cx.label(), cx.source, &Edit::DeleteClips { clip_ids: ids.clone(), ripple: a.bool_or("ripple", false) }, None)?;
            Ok(json!({ "deleted": ids }))
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
        _ => Err(crate::commands::unhandled(cx)),
    }
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
