//! OpenTimelineIO (`.otio` JSON, `.otioz` zip bundles, `.otiod` folder bundles), following the
//! OTIO 0.17 serialised schema: Timeline.1, Stack.1, Track.1, Clip.1 and Clip.2
//! (`media_references` with `active_media_reference_key`), ExternalReference.1,
//! MissingReference.1, GeneratorReference.1, ImageSequenceReference.1, Gap.1, Transition.1,
//! Marker.2 (string colours) and Marker.3 (Color.1 objects), LinearTimeWarp.1, FreezeFrame.1,
//! Effect.1, SerializableCollection.1. Bundles follow OTIO's file bundle layout: `content.otio`,
//! `version.txt` and the media under `media/`.
//!
//! Written timelines use Clip.2 and Marker.2, which DaVinci Resolve (18.5+), Kdenlive, Nuke
//! Studio and Hiero read. A clip's source range is its in point and its length on the timeline;
//! a speed change is a LinearTimeWarp (negative for a reversed clip), as OTIO's own adapters
//! write them. Everything kimchi has that OTIO doesn't (transforms, effects, titles, sound,
//! the mixer) goes into `metadata.kimchi`, so kimchi → OTIO → kimchi loses nothing; another app
//! keeps that metadata as it is.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use kimchi_core::{Clip, ClipContent, Id, Marker, MediaKind, Project, Track, TrackKind, Transition, new_id};
use serde_json::{Value, json};

use super::common::{self, Builder, MediaInfo, plural};
use super::time::Rate;
use super::{ExportOptions, Imported};
use crate::{Report, Result};

const KIMCHI: &str = "kimchi";

// ---------------------------------------------------------------------------------------------
// Reading

pub fn read(path: &Path) -> Result<Imported> {
    let (text, base, unpacked) = load(path)?;
    let root: Value = serde_json::from_str(&text).map_err(|e| format!("This isn't an OpenTimelineIO file: {e}"))?;
    let mut imp = from_value(&root, path, &base)?;
    if let Some(dir) = unpacked {
        imp.report.approximated(format!("the bundle's media were unpacked to {}", dir.display()));
    }
    Ok(imp)
}

/// The JSON of a `.otio`, `.otiod` or `.otioz`, the folder its relative paths start from, and
/// where a `.otioz`'s media were unpacked.
fn load(path: &Path) -> Result<(String, PathBuf, Option<PathBuf>)> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if path.is_dir() {
        let content = path.join("content.otio");
        let text = std::fs::read_to_string(&content).map_err(|e| format!("{} has no content.otio: {e}", path.display()))?;
        return Ok((text, path.to_path_buf(), None));
    }
    if ext == "otioz" || is_zip(path) {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{} isn't a readable .otioz bundle: {e}", path.display()))?;
        let mut text = String::new();
        zip.by_name("content.otio")
            .map_err(|_| format!("{} has no content.otio inside.", path.display()))?
            .read_to_string(&mut text)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        // Media inside the bundle are unpacked beside it, where kimchi can play them.
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "bundle".into());
        let dir = path.with_file_name(format!("{stem} media"));
        let mut unpacked = false;
        for i in 0..zip.len() {
            let Ok(mut entry) = zip.by_index(i) else { continue };
            let Some(name) = entry.enclosed_name() else { continue };
            if !name.starts_with("media") || entry.is_dir() {
                continue;
            }
            let out = dir.join(&name);
            if out.exists() {
                unpacked = true;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't unpack the bundle's media into {}: {e}", dir.display()))?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| format!("Couldn't unpack {}: {e}", out.display()))?;
            std::io::copy(&mut entry, &mut f).map_err(|e| format!("Couldn't unpack {}: {e}", out.display()))?;
            unpacked = true;
        }
        return Ok((text, dir.clone(), unpacked.then_some(dir)));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((text, path.parent().map(Path::to_path_buf).unwrap_or_default(), None))
}

fn is_zip(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"PK\x03\x04"
}

fn schema(v: &Value) -> &str {
    let s = v.get("OTIO_SCHEMA").and_then(Value::as_str).unwrap_or("");
    s.split('.').next().unwrap_or(s)
}

fn time(v: &Value) -> Option<f64> {
    let rate = v.get("rate")?.as_f64().filter(|r| *r > 0.0)?;
    let value = v.get("value")?.as_f64()?;
    Some(value / rate).filter(|t| t.is_finite())
}

fn range(v: &Value) -> Option<(f64, f64)> {
    let start = time(v.get("start_time")?)?;
    let dur = time(v.get("duration")?)?;
    Some((start, dur.max(0.0)))
}

fn rate_of(v: &Value) -> Option<f64> {
    v.get("rate").and_then(Value::as_f64).filter(|r| *r > 0.0)
}

fn name(v: &Value) -> String {
    v.get("name").and_then(Value::as_str).unwrap_or("").to_string()
}

fn kimchi_meta<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get("metadata")?.get(KIMCHI)?.get(key)
}

/// Reads an OTIO document already parsed (a Timeline, or a collection holding timelines).
pub fn from_value(root: &Value, file: &Path, base: &Path) -> Result<Imported> {
    let timeline = match schema(root) {
        "Timeline" => root.clone(),
        "SerializableCollection" => {
            let children = root.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
            let timelines: Vec<Value> = children.into_iter().filter(|c| schema(c) == "Timeline").collect();
            let first = timelines.first().cloned().ok_or("This OpenTimelineIO collection has no timeline in it.")?;
            if timelines.len() > 1 {
                let mut t = first;
                t["metadata"]["kimchi_other_timelines"] = json!(timelines.len() - 1);
                t
            } else {
                first
            }
        }
        "Stack" => json!({"OTIO_SCHEMA": "Timeline.1", "name": name(root), "tracks": root}),
        "Track" => json!({"OTIO_SCHEMA": "Timeline.1", "name": name(root), "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [root]}}),
        "" => return Err("This isn't an OpenTimelineIO file (no OTIO_SCHEMA).".into()),
        other => return Err(format!("This OpenTimelineIO file holds a {other}, not a timeline.")),
    };
    let mut b = Builder::new("otio", &name(&timeline), file);
    b.base = base.to_path_buf();
    if let Some(n) = timeline.get("metadata").and_then(|m| m.get("kimchi_other_timelines")).and_then(Value::as_u64) {
        b.report.dropped(format!("{n} more timeline{} in the file (only the first is opened)", plural(n as usize)));
    }
    // Settings: kimchi's own, else the timeline's rate.
    let saved = kimchi_meta(&timeline, "settings").and_then(|s| serde_json::from_value::<kimchi_core::ProjectSettings>(s.clone()).ok());
    let stack = timeline.get("tracks").cloned().unwrap_or(Value::Null);
    let fps = saved.as_ref().map(|s| s.fps).or_else(|| timeline.get("global_start_time").and_then(rate_of)).or_else(|| first_rate(&stack)).unwrap_or(30.0);
    *b.settings() = saved.unwrap_or_default();
    b.settings().fps = Rate::from_fps(fps).fps();
    if let Some(m) = kimchi_meta(&timeline, "mixer").and_then(|m| serde_json::from_value(m.clone()).ok()) {
        b.project.mixer = m;
    }
    let mut r = Reader { b, rate: Rate::from_fps(fps), disabled: 0, effects: HashMap::new(), freeze: 0, offcentre: 0 };
    r.read_stack(&stack, 0.0, None);
    // Markers on the timeline itself.
    for m in stack.get("markers").and_then(Value::as_array).into_iter().flatten() {
        r.marker(m, 0.0, 0.0, 1.0);
    }
    r.finish_notes();
    let mut b = r.b;
    common::merge_sound(&mut b, true);
    Ok(b.finish())
}

fn first_rate(v: &Value) -> Option<f64> {
    if let Some(r) = v.get("source_range").and_then(|r| r.get("duration")).and_then(rate_of) {
        return Some(r);
    }
    v.get("children").and_then(Value::as_array)?.iter().find_map(first_rate)
}

struct Reader {
    b: Builder,
    rate: Rate,
    disabled: usize,
    effects: HashMap<String, usize>,
    freeze: usize,
    offcentre: usize,
}

impl Reader {
    /// A stack's tracks, its time 0 at timeline time `origin`, keeping only what falls inside
    /// `window` (stack time) when it is nested.
    fn read_stack(&mut self, stack: &Value, origin: f64, window: Option<(f64, f64)>) {
        let children = stack.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
        for t in &children {
            match schema(t) {
                "Track" => self.read_track(t, origin, window),
                "Stack" => {
                    let (s, d) = t.get("source_range").and_then(range).unwrap_or((0.0, f64::INFINITY));
                    self.read_stack(t, origin - s, Some((s, s + d)));
                }
                _ => {}
            }
        }
    }

    fn read_track(&mut self, track: &Value, origin: f64, window: Option<(f64, f64)>) {
        let kind = if track.get("kind").and_then(Value::as_str).is_some_and(|k| k.eq_ignore_ascii_case("audio")) { TrackKind::Audio } else { TrackKind::Video };
        let saved: Option<Track> = kimchi_meta(track, "track").and_then(|t| serde_json::from_value(t.clone()).ok());
        let tname = saved.as_ref().map(|t| t.name.clone()).unwrap_or_else(|| name(track));
        let lane = match kind {
            TrackKind::Video => self.b.video_track(&tname),
            TrackKind::Audio => self.b.audio_track(&tname),
        };
        {
            let t = self.lane(kind, lane);
            if let Some(s) = &saved {
                t.muted = s.muted;
                t.hidden = s.hidden;
                t.locked = s.locked;
                t.captions = s.captions && kind == TrackKind::Video;
                t.mix = s.mix.clone();
            }
            if track.get("enabled").and_then(Value::as_bool) == Some(false) {
                match kind {
                    TrackKind::Video => t.hidden = true,
                    TrackKind::Audio => t.muted = true,
                }
            }
        }
        // A trimmed track (rare) shows only part of itself.
        let (origin, window) = match track.get("source_range").and_then(range) {
            Some((s, d)) => (origin - s, Some(window.map_or((s, s + d), |(a, z)| (a.max(s), z.min(s + d))))),
            None => (origin, window),
        };
        let items = track.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut pos = 0.0;
        let mut pending: Option<(f64, f64, &Value)> = None;
        for item in &items {
            match schema(item) {
                "Transition" => {
                    let off = |k: &str| item.get(k).and_then(time).unwrap_or(0.0).max(0.0);
                    pending = Some((off("in_offset"), off("out_offset"), item));
                }
                "Gap" => {
                    pos += item.get("source_range").and_then(range).map_or(0.0, |r| r.1);
                    for m in item.get("markers").and_then(Value::as_array).into_iter().flatten() {
                        self.marker(m, origin + pos, 0.0, 1.0);
                    }
                }
                "Clip" => {
                    let len = item.get("source_range").and_then(range).or_else(|| active_ref(item).and_then(|r| r.get("available_range")).and_then(range)).map_or(0.0, |r| r.1);
                    let at = pos;
                    pos += len;
                    if let Some((a, z)) = window
                        && (at + len <= a || at >= z)
                    {
                        pending = None;
                        continue;
                    }
                    if let Some(mut c) = self.clip(item, kind, origin + at, len) {
                        if let Some((a, z)) = window {
                            let (lo, hi) = (origin + a, origin + z);
                            if c.start < lo || c.end() > hi {
                                let (s, e) = (c.start.max(lo), c.end().min(hi));
                                c = c.cut(s, e);
                            }
                        }
                        if let Some((i, o, tr)) = pending.take() {
                            c.transition = Some(self.transition(tr, i, o, &c));
                        }
                        self.lane(kind, lane).clips.push(c);
                    }
                }
                "Stack" => {
                    let (s, d) = item.get("source_range").and_then(range).unwrap_or((0.0, 0.0));
                    let at = pos;
                    pos += d;
                    self.b.report.approximated("nested timelines (compound clips) were flattened onto tracks of their own");
                    self.read_stack(item, origin + at - s, Some((s, s + d)));
                    pending = None;
                }
                "Track" => {
                    let d = item.get("source_range").and_then(range).map_or(0.0, |r| r.1);
                    let stack = json!({"children": [item]});
                    self.read_stack(&stack, origin + pos, None);
                    pos += d;
                }
                _ => {}
            }
        }
        for m in track.get("markers").and_then(Value::as_array).into_iter().flatten() {
            self.marker(m, origin, 0.0, 1.0);
        }
    }

    fn lane(&mut self, kind: TrackKind, i: usize) -> &mut Track {
        match kind {
            TrackKind::Video => &mut self.b.video[i],
            TrackKind::Audio => &mut self.b.audio[i],
        }
    }

    fn clip(&mut self, item: &Value, kind: TrackKind, start: f64, len: f64) -> Option<Clip> {
        if item.get("enabled").and_then(Value::as_bool) == Some(false) {
            self.disabled += 1;
            return None;
        }
        let saved: Option<Clip> = kimchi_meta(item, "clip").and_then(|c| serde_json::from_value(c.clone()).ok());
        if kimchi_meta(item, "linkedSound").and_then(Value::as_bool) == Some(true) {
            // kimchi wrote this as the sound of a video clip: the video clip carries it.
            return None;
        }
        let source = item.get("source_range").and_then(range);
        let mut speed = 1.0;
        let mut reverse = false;
        for e in item.get("effects").and_then(Value::as_array).into_iter().flatten() {
            match schema(e) {
                "LinearTimeWarp" => {
                    let s = e.get("time_scalar").and_then(Value::as_f64).unwrap_or(1.0);
                    reverse = s < 0.0;
                    speed = s.abs();
                }
                "FreezeFrame" => {
                    self.freeze += 1;
                    speed = 0.1;
                }
                _ => {
                    let n = e.get("effect_name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| name(e));
                    if saved.is_none() && !n.is_empty() {
                        *self.effects.entry(n).or_default() += 1;
                    }
                }
            }
        }
        if !(0.1..=16.0).contains(&speed) {
            self.b.report.approximated("speeds beyond kimchi's 0.1× to 16× were brought within them");
            speed = speed.clamp(0.1, 16.0);
        }
        let in_point = source.map_or(0.0, |r| r.0).max(0.0);
        let mut clip = saved.clone().unwrap_or_else(|| Clip::new(name(item), start, len, ClipContent::Solid { color: "#000000".into() }));
        clip.id = new_id();
        if saved.is_some() {
            // kimchi's own clip: its sound settings are what they were.
            self.b.keep_sound.insert(clip.id);
        }
        // OTIO's timing wins when another app changed it; else kimchi's own exact values.
        let frame = 0.5 / self.rate.fps();
        if saved.is_none() || (clip.start - start).abs() > frame || (clip.duration - len).abs() > frame || (clip.in_point - in_point).abs() > frame {
            clip.start = start;
            clip.duration = len;
            clip.in_point = in_point;
            if saved.is_none() || (clip.speed - speed).abs() > 1e-6 {
                clip.speed = speed;
                clip.reverse = reverse;
            }
            if saved.is_some() {
                kimchi_core::anim::normalize(&mut clip.keyframes);
            }
        }
        // What the clip shows.
        let keep_own = saved.as_ref().is_some_and(|c| !matches!(c.content, ClipContent::Media { .. }));
        if !keep_own {
            clip.content = self.content(item, kind, saved.as_ref())?;
        }
        if let ClipContent::Media { asset_id } = clip.content
            && self.b.asset_kind(asset_id) == Some(MediaKind::Image)
        {
            clip.in_point = 0.0;
        }
        for m in item.get("markers").and_then(Value::as_array).into_iter().flatten() {
            self.marker(m, start, in_point, speed);
        }
        Some(clip)
    }

    fn content(&mut self, item: &Value, kind: TrackKind, saved: Option<&Clip>) -> Option<ClipContent> {
        let Some(r) = active_ref(item) else {
            self.b.report.dropped("clips without media");
            return None;
        };
        let saved_asset: Option<kimchi_core::Asset> = kimchi_meta(item, "asset").and_then(|a| serde_json::from_value(a.clone()).ok());
        let mut info = MediaInfo { name: saved_asset.as_ref().map(|a| a.name.clone()), ..Default::default() };
        if let Some((s, d)) = r.get("available_range").and_then(range) {
            info.duration = Some(s.max(0.0) + d);
        }
        if let Some(a) = &saved_asset {
            info.duration = info.duration.or(a.meta.duration);
            info.width = a.meta.width;
            info.height = a.meta.height;
            info.fps = a.meta.fps;
            info.has_audio = Some(a.meta.has_audio);
            info.has_video = Some(a.meta.has_video);
        }
        let _ = saved;
        let file = match schema(r) {
            "ExternalReference" => r.get("target_url").and_then(Value::as_str).unwrap_or("").to_string(),
            "MissingReference" => {
                let n = name(r);
                let n = if n.is_empty() { name(item) } else { n };
                if n.is_empty() {
                    self.b.report.dropped("clips whose media the file doesn't name");
                    return None;
                }
                n
            }
            "ImageSequenceReference" => {
                let base = r.get("target_url_base").and_then(Value::as_str).unwrap_or("");
                let prefix = r.get("name_prefix").and_then(Value::as_str).unwrap_or("");
                let suffix = r.get("name_suffix").and_then(Value::as_str).unwrap_or("");
                let pad = r.get("frame_zero_padding").and_then(Value::as_u64).unwrap_or(0);
                self.b.report.approximated("image sequences are read as video files: check that they play");
                let sep = if base.is_empty() || base.ends_with('/') { "" } else { "/" };
                format!("{base}{sep}{prefix}%0{pad}d{suffix}")
            }
            "GeneratorReference" => {
                let k = r.get("generator_kind").and_then(Value::as_str).unwrap_or("");
                let params = r.get("parameters");
                if k.eq_ignore_ascii_case("SolidColor") || k.eq_ignore_ascii_case("solid") {
                    let color = params.and_then(|p| p.get("color")).map(color_value).unwrap_or_else(|| "#000000".into());
                    return Some(ClipContent::Solid { color });
                }
                self.b.report.dropped(format!("generated clips of kind `{k}` (their picture is made by the other app)"));
                return None;
            }
            other => {
                self.b.report.dropped(format!("media references of type {other}"));
                return None;
            }
        };
        if file.trim().is_empty() {
            self.b.report.dropped("clips without media");
            return None;
        }
        let asset = match kind {
            TrackKind::Audio => self.b.sound(&file, &info),
            TrackKind::Video => self.b.media(&file, &info),
        };
        Some(ClipContent::Media { asset_id: asset })
    }

    fn transition(&mut self, tr: &Value, in_off: f64, out_off: f64, clip: &Clip) -> Transition {
        let saved: Option<Transition> = kimchi_meta(tr, "transition").and_then(|t| serde_json::from_value(t.clone()).ok());
        if let Some(t) = saved {
            return Transition { duration: in_off + out_off, ..t };
        }
        let ty = tr.get("transition_type").and_then(Value::as_str).unwrap_or("");
        let label = if ty == "SMPTE_Dissolve" || name(tr).is_empty() { ty.to_string() } else { name(tr) };
        let (kind, exact) = common::transition_kind(&label);
        if !exact {
            self.b.report.approximated(format!("transition \u{201c}{label}\u{201d} became {}", kind.label().to_lowercase()));
        }
        if (in_off - out_off).abs() > 0.5 / self.rate.fps() && in_off > 0.0 {
            self.offcentre += 1;
        }
        let _ = clip;
        Transition::new(kind, in_off + out_off)
    }

    /// A marker on an item whose first frame is at timeline time `at`, showing source time
    /// `source_start` there at `speed`.
    fn marker(&mut self, m: &Value, at: f64, source_start: f64, speed: f64) {
        let Some((t, _)) = m.get("marked_range").and_then(range) else { return };
        let time = (at + (t - source_start) / speed.max(1e-6)).max(0.0);
        let color = kimchi_meta(m, "color").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| match m.get("color") {
            Some(Value::String(s)) => common::marker_color(s),
            Some(c @ Value::Object(_)) => color_value(c),
            _ => common::marker_color("GREEN"),
        });
        let mut label = name(m);
        if let Some(c) = m.get("comment").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) {
            label = if label.is_empty() { c.to_string() } else { format!("{label}: {c}") };
        }
        self.b.project.markers.push(Marker { id: new_id(), time, label, color });
    }

    fn finish_notes(&mut self) {
        if self.disabled > 0 {
            self.b.report.dropped(format!("{} switched-off clip{} (kimchi has no switched-off clips)", self.disabled, plural(self.disabled)));
        }
        if self.freeze > 0 {
            self.b.report.approximated(format!("{} freeze frame{} play at 0.1× (kimchi has no freeze frames)", self.freeze, plural(self.freeze)));
        }
        if self.offcentre > 0 {
            self.b.report.approximated(format!("{} transition{} off the cut were centred on it", self.offcentre, plural(self.offcentre)));
        }
        let mut fx: Vec<_> = self.effects.drain().collect();
        fx.sort();
        for (n, count) in fx {
            self.b.report.dropped(format!("effect \u{201c}{n}\u{201d} on {count} clip{}", plural(count)));
        }
    }
}

/// The media reference a clip uses (Clip.2's active one, or Clip.1's only one).
fn active_ref(item: &Value) -> Option<&Value> {
    if let Some(refs) = item.get("media_references").and_then(Value::as_object) {
        let key = item.get("active_media_reference_key").and_then(Value::as_str).unwrap_or("DEFAULT_MEDIA");
        return refs.get(key).or_else(|| refs.values().next());
    }
    item.get("media_reference").filter(|r| !r.is_null())
}

/// A colour given as `[r, g, b(, a)]` (0…1), `{r, g, b, a}` (Color.1) or `#rrggbb`.
fn color_value(v: &Value) -> String {
    match v {
        Value::String(s) if s.starts_with('#') => s.clone(),
        Value::String(s) => common::marker_color(s),
        Value::Array(a) => {
            let c: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            let scale = if c.iter().any(|v| *v > 1.0) { 255.0 } else { 1.0 };
            common::hex([c.first().copied().unwrap_or(0.0) / scale, c.get(1).copied().unwrap_or(0.0) / scale, c.get(2).copied().unwrap_or(0.0) / scale, c.get(3).copied().unwrap_or(scale) / scale])
        }
        Value::Object(o) => {
            if let Some(n) = o.get("name").and_then(Value::as_str).filter(|n| !n.is_empty())
                && o.get("r").is_none()
            {
                return common::marker_color(n);
            }
            let ch = |k: &str| o.get(k).and_then(Value::as_f64).unwrap_or(if k == "a" { 1.0 } else { 0.0 });
            common::hex([ch("r"), ch("g"), ch("b"), ch("a")])
        }
        _ => "#000000".into(),
    }
}

// ---------------------------------------------------------------------------------------------
// Writing

pub fn write(project: &Project, path: &Path, opts: &ExportOptions) -> Result<Report> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut report = Report::new("otio");
    match ext.as_str() {
        "otioz" | "otiod" => {
            // Bundles carry their media: references point inside, at media/<file>.
            let mut media: Vec<(String, PathBuf)> = vec![];
            let mut names: HashMap<String, String> = HashMap::new();
            let mut taken: Vec<String> = vec![];
            for (file, _) in files(project, opts) {
                if names.contains_key(&file) {
                    continue;
                }
                let name = unique_name(&common::file_name(&file), &mut taken);
                names.insert(file.clone(), format!("media/{name}"));
                media.push((name, PathBuf::from(&file)));
            }
            let doc = to_value(project, opts, &mut report, &|p: &str| names.get(p).cloned().unwrap_or_else(|| common::path_to_url(p)));
            let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
            let missing: Vec<&(String, PathBuf)> = media.iter().filter(|(_, p)| !p.is_file()).collect();
            for (_, p) in &missing {
                report.missing_media.push(p.to_string_lossy().into_owned());
            }
            if ext == "otiod" {
                write_bundle_dir(path, &text, &media)?;
            } else {
                write_bundle_zip(path, &text, &media)?;
            }
            report.kept(format!("{} media file{} inside the bundle", media.len() - missing.len(), plural(media.len() - missing.len())));
        }
        _ => {
            let doc = to_value(project, opts, &mut report, &|p: &str| common::path_to_url(p));
            let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
            super::write_text(path, &text)?;
        }
    }
    Ok(report)
}

fn unique_name(name: &str, taken: &mut Vec<String>) -> String {
    let mut n = name.to_string();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    let mut i = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&n)) {
        n = format!("{stem} ({i}){ext}");
        i += 1;
    }
    taken.push(n.clone());
    n
}

/// Every media file the written timeline points at.
fn files(project: &Project, opts: &ExportOptions) -> Vec<(String, Id)> {
    let mut out = vec![];
    for (_, c) in project.clips() {
        if let Some(r) = opts.rendered.get(&c.id) {
            out.push((r.to_string_lossy().into_owned(), c.id));
        } else if let Some(a) = c.asset_id().and_then(|id| project.asset(id)) {
            out.push((a.path.clone(), a.id));
        }
    }
    out
}

fn write_bundle_dir(dir: &Path, text: &str, media: &[(String, PathBuf)]) -> Result<()> {
    std::fs::create_dir_all(dir.join("media")).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    std::fs::write(dir.join("content.otio"), text).map_err(|e| format!("Couldn't write {}: {e}", dir.display()))?;
    std::fs::write(dir.join("version.txt"), "1.0.0").map_err(|e| e.to_string())?;
    for (name, src) in media.iter().filter(|(_, p)| p.is_file()) {
        let to = dir.join("media").join(name);
        if !to.exists() {
            std::fs::copy(src, &to).map_err(|e| format!("Couldn't copy {} into the bundle: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn write_bundle_zip(path: &Path, text: &str, media: &[(String, PathBuf)]) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let tmp = path.with_extension("otioz.tmp");
    let fail = |e: &dyn std::fmt::Display| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't write {}: {e}", path.display())
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    let file = std::fs::File::create(&tmp).map_err(|e| fail(&e))?;
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let deflated = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("version.txt", stored).map_err(|e| fail(&e))?;
    zip.write_all(b"1.0.0").map_err(|e| fail(&e))?;
    zip.start_file("content.otio", deflated).map_err(|e| fail(&e))?;
    zip.write_all(text.as_bytes()).map_err(|e| fail(&e))?;
    for (name, src) in media.iter().filter(|(_, p)| p.is_file()) {
        zip.start_file(format!("media/{name}"), stored).map_err(|e| fail(&e))?;
        let mut f = std::fs::File::open(src).map_err(|e| fail(&e))?;
        std::io::copy(&mut f, &mut zip).map_err(|e| fail(&e))?;
    }
    zip.finish().map_err(|e| fail(&e))?;
    std::fs::rename(&tmp, path).map_err(|e| fail(&e))
}

fn rt(frames: f64, rate: Rate) -> Value {
    json!({"OTIO_SCHEMA": "RationalTime.1", "rate": rate.fps(), "value": frames})
}

fn tr(start: f64, dur: f64, rate: Rate) -> Value {
    json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(start, rate), "duration": rt(dur, rate)})
}

/// Seconds as a frame count at `rate`, kept to the microframe (source times needn't be whole
/// frames).
fn fine(seconds: f64, rate: Rate) -> f64 {
    (seconds * rate.fps() * 1e6).round() / 1e6
}

fn gap(frames: i64, rate: Rate) -> Value {
    json!({"OTIO_SCHEMA": "Gap.1", "name": "", "source_range": tr(0.0, frames as f64, rate), "effects": [], "markers": [], "enabled": true, "metadata": {}})
}

/// The whole timeline as OTIO JSON; `url` turns a media path into the reference's target.
pub fn to_value(project: &Project, opts: &ExportOptions, report: &mut Report, url: &dyn Fn(&str) -> String) -> Value {
    let rate = Rate::from_fps(project.settings.fps);
    let mut children = vec![];
    let mut sounds = vec![];
    for t in common::video_tracks(project) {
        children.push(track(project, t, rate, opts, url, false));
        if t.clips.iter().any(|c| common::plays_sound(project, c)) {
            sounds.push(track(project, t, rate, opts, url, true));
        }
    }
    for t in common::audio_tracks(project) {
        children.push(track(project, t, rate, opts, url, false));
    }
    // The sound of video clips, on audio tracks of their own after the others.
    children.extend(sounds);
    let markers: Vec<Value> = project
        .markers
        .iter()
        .map(|m| {
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": m.label,
                "color": common::nearest(&m.color, common::MARKER_COLORS),
                "comment": "",
                "marked_range": tr(rate.frames(m.time) as f64, 0.0, rate),
                "metadata": {KIMCHI: {"color": m.color}},
            })
        })
        .collect();
    let without_motion = project.clips().filter(|(_, c)| !matches!(c.content, ClipContent::Media { .. }) && !opts.rendered.contains_key(&c.id)).count();
    if without_motion > 0 {
        report.approximated(format!(
            "{without_motion} title{}, colour or motion clip{} are generators other apps show as missing (kimchi keeps them; export with renderMotion for video files)",
            plural(without_motion),
            plural(without_motion)
        ));
    }
    let n = project.clips().count();
    report.kept(format!("{n} clip{} on {} track{}", plural(n), project.tracks.len(), plural(project.tracks.len())));
    if !project.markers.is_empty() {
        report.kept(format!("{} marker{}", project.markers.len(), plural(project.markers.len())));
    }
    report.kept("transforms, effects, titles and sound settings in kimchi's metadata (kimchi reads them back; other apps keep them)");
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": project.name,
        "global_start_time": rt(0.0, rate),
        "metadata": {KIMCHI: {"version": 1, "settings": project.settings, "mixer": project.mixer}},
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": children,
            "effects": [],
            "markers": markers,
            "enabled": true,
            "source_range": null,
            "metadata": {},
        },
    })
}

fn track(project: &Project, t: &Track, rate: Rate, opts: &ExportOptions, url: &dyn Fn(&str) -> String, sound: bool) -> Value {
    let mut items = vec![];
    let mut at = 0i64; // frames written so far
    let mut prev_end: Option<f64> = None;
    for (i, c) in t.clips.iter().enumerate() {
        if sound && !common::plays_sound(project, c) {
            continue;
        }
        let (s, e) = (rate.frames(c.start), rate.frames(c.end()).max(rate.frames(c.start) + 1));
        if s > at {
            items.push(gap(s - at, rate));
        }
        let s = s.max(at);
        if let Some(tr) = &c.transition
            && !sound
        {
            let before = i.checked_sub(1).map(|j| &t.clips[j]);
            let cut = prev_end.is_some_and(|pe| (pe - c.start).abs() <= kimchi_core::transition::CUT_TOLERANCE);
            let len = kimchi_core::transition::effective_length(tr.duration, c, before.filter(|_| cut));
            let frames = rate.frames(len);
            if frames > 0 {
                let (i_off, o_off) = if cut { (frames as f64 / 2.0, frames as f64 / 2.0) } else { (0.0, frames as f64) };
                items.push(json!({
                    "OTIO_SCHEMA": "Transition.1",
                    "name": tr.kind.label(),
                    "transition_type": if tr.kind == kimchi_core::TransitionKind::Dissolve { "SMPTE_Dissolve" } else { "Custom_Transition" },
                    "in_offset": rt(i_off, rate),
                    "out_offset": rt(o_off, rate),
                    "enabled": true,
                    "metadata": {KIMCHI: {"transition": tr}},
                }));
            }
        }
        items.push(clip(project, c, s, e, rate, opts, url, sound));
        at = e;
        prev_end = Some(c.end());
    }
    let mut meta_track = t.clone();
    meta_track.clips.clear();
    let kind = if t.kind == TrackKind::Audio || sound { "Audio" } else { "Video" };
    let mut meta = json!({KIMCHI: {"track": meta_track}});
    if sound {
        meta = json!({KIMCHI: {"linkedSound": true}});
    }
    json!({
        "OTIO_SCHEMA": "Track.1",
        "name": if sound { format!("{} sound", t.name) } else { t.name.clone() },
        "kind": kind,
        "children": items,
        "effects": [],
        "markers": [],
        "enabled": !(t.hidden && t.kind == TrackKind::Video || t.muted && (t.kind == TrackKind::Audio || sound)),
        "source_range": null,
        "metadata": meta,
    })
}

#[allow(clippy::too_many_arguments)]
fn clip(project: &Project, c: &Clip, s: i64, e: i64, rate: Rate, opts: &ExportOptions, url: &dyn Fn(&str) -> String, sound: bool) -> Value {
    let mut effects = vec![];
    if c.speed != 1.0 || c.reverse {
        effects.push(json!({
            "OTIO_SCHEMA": "LinearTimeWarp.1",
            "name": "",
            "effect_name": "LinearTimeWarp",
            "time_scalar": if c.reverse { -c.speed } else { c.speed },
            "enabled": true,
            "metadata": {},
        }));
    }
    let asset = c.asset_id().and_then(|id| project.asset(id));
    let mut meta = serde_json::Map::new();
    if sound {
        meta.insert("linkedSound".into(), json!(true));
    } else {
        meta.insert("clip".into(), json!(c));
        if let Some(a) = asset {
            let mut a = a.clone();
            // Previews are made again where the project is opened.
            a.thumbnail = None;
            a.filmstrip = None;
            a.waveform = None;
            a.proxy = None;
            meta.insert("asset".into(), json!(a));
        }
    }
    let reference = if let Some(file) = opts.rendered.get(&c.id) {
        json!({
            "OTIO_SCHEMA": "ExternalReference.1",
            "name": common::file_name(&file.to_string_lossy()),
            "target_url": url(&file.to_string_lossy()),
            "available_range": tr(0.0, (e - s) as f64, rate),
            "available_image_bounds": null,
            "metadata": {},
        })
    } else if let Some(a) = asset {
        let avail = a.duration().map_or(Value::Null, |d| tr(0.0, fine(d, rate), rate));
        json!({
            "OTIO_SCHEMA": "ExternalReference.1",
            "name": a.name,
            "target_url": url(&a.path),
            "available_range": avail,
            "available_image_bounds": null,
            "metadata": {},
        })
    } else {
        let (kind, params) = match &c.content {
            ClipContent::Solid { color } => ("SolidColor", json!({"color": common::rgba(color)})),
            ClipContent::Text { style } => ("kimchi.title", json!({"text": style.content})),
            ClipContent::Motion { .. } => ("kimchi.motion", json!({})),
            _ => ("kimchi.placeholder", json!({})),
        };
        json!({
            "OTIO_SCHEMA": "GeneratorReference.1",
            "name": c.name,
            "generator_kind": kind,
            "parameters": params,
            "available_range": null,
            "available_image_bounds": null,
            "metadata": {},
        })
    };
    let in_point = if opts.rendered.contains_key(&c.id) || asset.is_some_and(|a| a.kind == MediaKind::Image) { 0.0 } else { c.in_point };
    json!({
        "OTIO_SCHEMA": "Clip.2",
        "name": c.name,
        "source_range": tr(fine(in_point, rate), (e - s) as f64, rate),
        "effects": effects,
        "markers": [],
        "enabled": true,
        "media_references": {"DEFAULT_MEDIA": reference},
        "active_media_reference_key": "DEFAULT_MEDIA",
        "metadata": {KIMCHI: Value::Object(meta)},
    })
}
