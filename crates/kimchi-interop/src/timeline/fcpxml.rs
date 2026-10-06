//! Final Cut Pro XML (FCPXML 1.6 to 1.13, `.fcpxml` files and `.fcpxmld` bundles holding
//! `Info.fcpxml`), following Apple's FCPXML reference and the 1.10 DTD: `resources` (`format`,
//! `asset` with `media-rep` or the older `src`, `effect`, `media` for compound clips), the
//! `library › event › project › sequence › spine`, story elements (`asset-clip`, `clip` with
//! `video`/`audio` inside, `ref-clip`, `gap`, `title` with `text-style-def`, `audition` (its
//! first pick), `transition`), connected clips by `lane` (above: video tracks; below: audio
//! tracks) and secondary storylines, `adjust-transform` (position in percent of the frame
//! height, y up; rotation counter-clockwise), `adjust-blend`, `adjust-volume`, `adjust-panner`,
//! `adjust-conform`, `keyframeAnimation`, `timeMap` (speed and reverse), `marker` and
//! `chapter-marker`, and roles (track names).
//!
//! Written files are FCPXML 1.10, which Final Cut Pro 10.5 and later and DaVinci Resolve read:
//! the bottom video track is the primary storyline, the other tracks connected clips (video
//! above, sound below).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use kimchi_core::{Clip, ClipContent, Easing, Fit, Marker, MediaKind, Project, TextAlign, TextStyle, TrackKind, Transition, new_id};

use super::common::{self, Builder, MediaInfo, plural};
use super::time::{Rate, parse_rational, rational};
use super::xml::{self, Element};
use super::{ExportOptions, Imported};
use crate::{Report, Result};

const CROSS_DISSOLVE: &str = "FxPlug:4731E73A-8DAC-4113-9A30-AE85B1761265";
const BASIC_TITLE: &str = ".../Titles.localized/Bumper:Opener.localized/Basic Title.localized/Basic Title.moti";
const CUSTOM_SOLID: &str = ".../Generators.localized/Solids.localized/Custom.localized/Custom.motn";

fn t(e: &Element, a: &str) -> Option<f64> {
    e.attr(a).and_then(parse_rational)
}

struct Asset {
    url: String,
    start: f64,
    info: MediaInfo,
}

struct Lane {
    kind: TrackKind,
    name: String,
    clips: Vec<Clip>,
}

struct Reader<'a> {
    b: Builder,
    rate: Rate,
    canvas: (u32, u32),
    assets: HashMap<String, Asset>,
    effects: HashMap<String, String>,
    media: HashMap<String, &'a Element>,
    lanes: BTreeMap<(i8, i64), Lane>,
    dropped_fx: HashMap<String, usize>,
    disabled: usize,
    depth: usize,
}

pub fn read(path: &Path) -> Result<Imported> {
    let file: PathBuf = if path.is_dir() { path.join("Info.fcpxml") } else { path.to_path_buf() };
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let root = xml::parse(&text)?;
    if root.name != "fcpxml" {
        return Err(format!("This isn't FCPXML (its root is <{}>, not <fcpxml>).", root.name));
    }
    let version = root.attr("version").unwrap_or("?").to_string();
    let projects = root.find_all("project");
    let seq = projects.first().and_then(|p| p.child("sequence")).or_else(|| root.find_all("sequence").into_iter().next()).ok_or("This FCPXML has no project in it (export a project, not only clips).")?;
    let pname = projects.first().and_then(|p| p.attr("name")).unwrap_or("");
    let mut b = Builder::new("fcpxml", pname, &file);
    if projects.len() > 1 {
        b.report.dropped(format!("{} other project{} in the file (the first is opened)", projects.len() - 1, plural(projects.len() - 1)));
    }
    type FormatInfo = (Option<Rate>, Option<u32>, Option<u32>);
    let mut formats: HashMap<String, FormatInfo> = HashMap::new();
    let mut r = Reader { b, rate: Rate::new(30, 1), canvas: (1920, 1080), assets: HashMap::new(), effects: HashMap::new(), media: HashMap::new(), lanes: BTreeMap::new(), dropped_fx: HashMap::new(), disabled: 0, depth: 0 };
    if let Some(res) = root.child("resources") {
        for e in &res.children {
            let Some(id) = e.attr("id") else { continue };
            match e.name.as_str() {
                "format" => {
                    let rate = e.attr("frameDuration").and_then(super::time::RationalSeconds::parse).and_then(|d| Rate::from_frame_duration(d.num, d.den));
                    formats.insert(id.into(), (rate, e.attr_f64("width").map(|v| v as u32), e.attr_f64("height").map(|v| v as u32)));
                }
                "asset" => {
                    let url = e.child("media-rep").and_then(|m| m.attr("src")).or(e.attr("src")).unwrap_or("").to_string();
                    let fmt = e.attr("format").and_then(|f| formats.get(f));
                    let info = MediaInfo {
                        name: e.attr("name").map(str::to_string),
                        duration: t(e, "duration"),
                        width: fmt.and_then(|f| f.1),
                        height: fmt.and_then(|f| f.2),
                        fps: fmt.and_then(|f| f.0).map(Rate::fps),
                        has_video: e.attr("hasVideo").map(|v| v == "1"),
                        has_audio: e.attr("hasAudio").map(|v| v == "1"),
                    };
                    r.assets.insert(id.into(), Asset { url, start: t(e, "start").unwrap_or(0.0), info });
                }
                "effect" => {
                    r.effects.insert(id.into(), e.attr("name").unwrap_or("").to_string());
                }
                "media" => {
                    r.media.insert(id.into(), e);
                }
                _ => {}
            }
        }
    }
    if let Some((rate, w, h)) = seq.attr("format").and_then(|f| formats.get(f)) {
        r.rate = rate.unwrap_or(r.rate);
        r.canvas = (w.unwrap_or(1920).clamp(16, 16384), h.unwrap_or(1080).clamp(16, 16384));
    }
    let s = r.b.settings();
    s.fps = r.rate.fps();
    (s.width, s.height) = r.canvas;
    if let Some(rate) = seq.attr("audioRate") {
        let hz = rate.trim_end_matches('k').parse::<f64>().map(|k| (k * 1000.0) as u32).unwrap_or(48_000);
        r.b.settings().sample_rate = hz;
    }
    let tc_start = t(seq, "tcStart").unwrap_or(0.0);
    let spine = seq.child("spine").ok_or("This FCPXML project has no spine (timeline).")?;
    r.spine(spine, -tc_start, 0, None);
    r.finish(&version);
    let mut b = r.b;
    common::merge_sound(&mut b, false);
    Ok(b.finish())
}

impl<'a> Reader<'a> {
    fn lane(&mut self, lane: i64, kind: TrackKind) -> &mut Vec<Clip> {
        let key = (if kind == TrackKind::Audio { 1 } else { 0 }, lane);
        &mut self.lanes.entry(key).or_insert_with(|| Lane { kind, name: String::new(), clips: vec![] }).clips
    }

    /// A storyline: its items one after the other, the first at timeline time `origin` +
    /// its offset. `window`: only what falls inside (a compound clip's trimmed range).
    fn spine(&mut self, spine: &'a Element, origin: f64, lane: i64, window: Option<(f64, f64)>) {
        let mut pending: Option<(f64, String)> = None;
        for e in &spine.children {
            if e.name == "transition" {
                let name = e.attr("name").unwrap_or("Cross Dissolve").to_string();
                pending = Some((t(e, "duration").unwrap_or(0.0), name));
                continue;
            }
            let at = origin + t(e, "offset").unwrap_or(0.0);
            let before = self.lane_len(lane, e);
            self.element(e, at, lane, window);
            if let Some((len, name)) = pending.take()
                && self.lane_len(lane, e) > before
            {
                let (kind, exact) = common::transition_kind(&name);
                if !exact {
                    self.b.report.approximated(format!("transition \u{201c}{name}\u{201d} became {}", kind.label().to_lowercase()));
                }
                let k = if self.is_audio(e) { TrackKind::Audio } else { TrackKind::Video };
                if let Some(c) = self.lane(lane, k).get_mut(before) {
                    c.transition = Some(Transition::new(kind, len));
                }
            }
        }
    }

    fn lane_len(&mut self, lane: i64, e: &Element) -> usize {
        let k = if self.is_audio(e) { TrackKind::Audio } else { TrackKind::Video };
        self.lane(lane, k).len()
    }

    fn is_audio(&self, e: &Element) -> bool {
        match e.name.as_str() {
            "audio" => true,
            "asset-clip" => {
                e.attr("srcEnable") == Some("audio")
                    || e.attr("ref").and_then(|r| self.assets.get(r)).is_some_and(|a| a.info.has_video == Some(false) || (a.info.has_video.is_none() && common::kind_of(&a.url) == MediaKind::Audio))
            }
            "clip" => e.child("video").is_none() && e.child("audio").is_some(),
            _ => false,
        }
    }

    /// One story element at timeline time `at` (its first frame), and what is anchored to it.
    fn element(&mut self, e: &'a Element, at: f64, lane: i64, window: Option<(f64, f64)>) {
        let dur = t(e, "duration").unwrap_or(0.0);
        let start = t(e, "start").unwrap_or(0.0);
        if e.attr("enabled") == Some("0") {
            self.disabled += 1;
            return;
        }
        match e.name.as_str() {
            "gap" => {}
            "audition" => {
                if let Some(first) = e.children.iter().find(|c| c.attr("duration").is_some()) {
                    self.element(first, at, lane, window);
                    if e.children.len() > 1 {
                        self.b.report.approximated("auditions came as their chosen clip");
                    }
                }
                return;
            }
            "spine" => {
                self.spine(e, at - start, lane, window);
                return;
            }
            "ref-clip" | "sync-clip" | "mc-clip" => {
                let inner = match e.name.as_str() {
                    "ref-clip" => e.attr("ref").and_then(|r| self.media.get(r).copied()).and_then(|m| m.path("sequence/spine")),
                    "sync-clip" => e.child("spine").or(Some(e)),
                    _ => None,
                };
                match inner {
                    Some(sp) if self.depth < 8 => {
                        self.depth += 1;
                        if e.name == "ref-clip" {
                            self.b.report.approximated("compound clips were flattened onto the tracks");
                        }
                        let w = Some((at, at + dur));
                        if e.name == "sync-clip" && std::ptr::eq(sp, e) {
                            for c in e.children.iter().filter(|c| c.attr("duration").is_some()) {
                                let off = t(c, "offset").unwrap_or(0.0);
                                self.element(c, at + off - start, lane + c.attr("lane").and_then(|l| l.parse::<i64>().ok()).unwrap_or(0), w);
                            }
                        } else {
                            self.spine(sp, at - start, lane, w);
                        }
                        self.depth -= 1;
                    }
                    _ => self.b.report.dropped(format!("{} elements", e.name)),
                }
            }
            "asset-clip" | "clip" | "video" | "audio" | "title" => {
                if let Some(mut c) = self.clip(e, at, dur, start) {
                    if let Some((a, z)) = window {
                        if c.end() <= a || c.start >= z {
                            return;
                        }
                        if c.start < a || c.end() > z {
                            c = c.cut(c.start.max(a), c.end().min(z));
                        }
                    }
                    let audio = self.is_audio(e);
                    let kind = if audio { TrackKind::Audio } else { TrackKind::Video };
                    let role = e.attr(if audio { "audioRole" } else { "videoRole" }).or(e.attr("role")).map(|r| r.split('.').next().unwrap_or(r).to_string());
                    let l = self.lanes.entry((if audio { 1 } else { 0 }, lane)).or_insert_with(|| Lane { kind, name: String::new(), clips: vec![] });
                    if l.name.is_empty()
                        && let Some(r) = role
                    {
                        l.name = titlecase(&r);
                    }
                    l.clips.push(c);
                }
            }
            _ => {}
        }
        // Anchored (connected) items, in this element's local time.
        for child in &e.children {
            let Some(l) = child.attr("lane").and_then(|l| l.parse::<i64>().ok()) else { continue };
            if !matches!(child.name.as_str(), "asset-clip" | "clip" | "video" | "audio" | "title" | "ref-clip" | "spine" | "audition" | "sync-clip" | "mc-clip" | "gap") {
                continue;
            }
            let off = t(child, "offset").unwrap_or(0.0);
            self.element(child, at + off - start, lane + l, window);
        }
    }

    fn clip(&mut self, e: &Element, at: f64, dur: f64, start: f64) -> Option<Clip> {
        let name = e.attr("name").unwrap_or("").to_string();
        let audio = self.is_audio(e);
        let mut clip = match e.name.as_str() {
            "title" => {
                let mut style = TextStyle::default();
                let defs: HashMap<&str, &Element> = e.children_named("text-style-def").filter_map(|d| Some((d.attr("id")?, d.child("text-style")?))).collect();
                let mut words = String::new();
                let mut first_ref = None;
                for tx in e.children_named("text") {
                    words.push_str(&tx.text);
                    for st in tx.children_named("text-style") {
                        words.push_str(&st.text);
                        first_ref = first_ref.or(st.attr("ref"));
                    }
                }
                if let Some(s) = first_ref.and_then(|r| defs.get(r)) {
                    if let Some(f) = s.attr("font") {
                        style.font_family = f.to_string();
                    }
                    if let Some(sz) = s.attr_f64("fontSize") {
                        style.font_size = sz * self.canvas.1 as f64 / 1080.0;
                    }
                    if let Some(c) = s.attr("fontColor") {
                        style.color = color(c);
                    }
                    style.font_weight = if s.attr("bold") == Some("1") || s.attr("fontFace").is_some_and(|f| f.to_ascii_lowercase().contains("bold")) { 700 } else { 400 };
                    style.italic = s.attr("italic") == Some("1") || s.attr("fontFace").is_some_and(|f| f.to_ascii_lowercase().contains("italic"));
                    style.align = match s.attr("alignment") {
                        Some("left") => TextAlign::Left,
                        Some("right") => TextAlign::Right,
                        _ => TextAlign::Center,
                    };
                    style.shadow = s.attr("shadowColor").is_some();
                }
                style.content = if words.trim().is_empty() { name.clone() } else { words.trim().to_string() };
                Clip::new(if name.is_empty() { "Title".into() } else { name.clone() }, at, dur, ClipContent::Text { style })
            }
            _ => {
                // The media: the element's own ref, or the video/audio inside a `clip`.
                let (inner, inner_off) = match e.name.as_str() {
                    "clip" => {
                        let i = e.child("video").or_else(|| e.child("audio"))?;
                        (i, t(i, "offset").unwrap_or(0.0))
                    }
                    _ => (e, 0.0),
                };
                let r = inner.attr("ref")?;
                if self.effects.contains_key(r) {
                    // A generator (a solid, a Motion template).
                    let n = self.effects.get(r).cloned().unwrap_or_default();
                    if !n.to_ascii_lowercase().contains("solid") && !n.to_ascii_lowercase().contains("custom") {
                        self.b.report.approximated(format!("generator \u{201c}{n}\u{201d} became a colour clip"));
                    }
                    let mut color = "#000000".to_string();
                    for p in inner.children_named("param") {
                        if p.attr("name").is_some_and(|n| n.eq_ignore_ascii_case("color"))
                            && let Some(v) = p.attr("value")
                        {
                            color = color_of(v);
                        }
                    }
                    Clip::new(if name.is_empty() { n } else { name.clone() }, at, dur, ClipContent::Solid { color })
                } else {
                    let a = self.assets.get(r)?;
                    let (url, astart, info) = (a.url.clone(), a.start, a.info.clone());
                    if url.is_empty() {
                        return None;
                    }
                    let id = if audio { self.b.sound(&url, &info) } else { self.b.media(&url, &info) };
                    let inner_start = if e.name == "clip" { t(inner, "start").unwrap_or(0.0) + (start - inner_off) } else { start };
                    let mut c = Clip::new(if name.is_empty() { common::file_name(&url) } else { name.clone() }, at, dur, ClipContent::Media { asset_id: id });
                    if self.b.asset_kind(id) != Some(MediaKind::Image) {
                        c.in_point = (inner_start - astart).max(0.0);
                    }
                    if e.attr("srcEnable") == Some("video") {
                        c.audio.muted = true;
                    }
                    c
                }
            }
        };
        self.adjust(e, &mut clip, start);
        for m in e.children.iter().filter(|m| m.name == "marker" || m.name == "chapter-marker") {
            let Some(ms) = t(m, "start") else { continue };
            let time = (at + (ms - start) / clip.speed.max(1e-6)).max(0.0);
            let mut label = m.attr("value").unwrap_or("").to_string();
            if let Some(n) = m.attr("note").filter(|n| !n.is_empty()) {
                label = format!("{label}: {n}");
            }
            let color = common::marker_color(if m.name == "chapter-marker" { "ORANGE" } else if m.attr("completed").is_some() { "GREEN" } else { "BLUE" });
            self.b.project.markers.push(Marker { id: new_id(), time, label, color });
        }
        Some(clip)
    }

    /// Transforms, opacity, volume, pan, speed and filters of an element.
    fn adjust(&mut self, e: &Element, c: &mut Clip, start: f64) {
        let h = self.canvas.1 as f64;
        let rate = self.rate;
        let keys = |p: Option<&Element>, conv: &dyn Fn(&str) -> Option<f64>| -> Vec<(f64, f64, Easing)> {
            p.and_then(|p| p.child("keyframeAnimation"))
                .map(|ka| {
                    ka.children_named("keyframe")
                        .filter_map(|k| {
                            let time = t(k, "time")? - start;
                            let v = conv(k.attr("value")?)?;
                            let ease = match k.attr("interp") {
                                Some("ease") => Easing::EASE_IN_OUT,
                                Some("easeIn") => Easing::EASE_IN,
                                Some("easeOut") => Easing::EASE_OUT,
                                _ => Easing::Linear,
                            };
                            Some((rate.snap(time.max(0.0)), v, ease))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let param = |e: &'_ Element, n: &str| e.children_named("param").find(|p| p.attr("name").is_some_and(|x| x.eq_ignore_ascii_case(n))).cloned();
        for adj in &e.children {
            match adj.name.as_str() {
                "adjust-transform" => {
                    let pair = |s: &str| -> Option<(f64, f64)> {
                        let mut it = s.split_whitespace().filter_map(|v| v.parse::<f64>().ok());
                        Some((it.next()?, it.next().unwrap_or(0.0)))
                    };
                    if let Some((x, y)) = adj.attr("position").and_then(pair) {
                        c.transform.x = x * h / 100.0;
                        c.transform.y = -y * h / 100.0;
                    }
                    if let Some((sx, sy)) = adj.attr("scale").and_then(pair) {
                        c.transform.scale = (sx.abs() + sy.abs()) / 2.0;
                        if (sx - sy).abs() > 1e-6 {
                            self.b.report.approximated("scales different across and down became one scale");
                        }
                    }
                    if let Some(r) = adj.attr_f64("rotation") {
                        c.transform.rotation = -r;
                    }
                    if let Some(p) = param(adj, "position") {
                        let ks: Vec<kimchi_core::Keyframe> = p
                            .child("keyframeAnimation")
                            .into_iter()
                            .flat_map(|ka| ka.children_named("keyframe"))
                            .filter_map(|k| {
                                let (x, y) = pair(k.attr("value")?)?;
                                Some(kimchi_core::Keyframe::new(rate.snap((t(k, "time")? - start).max(0.0)), [x * h / 100.0, -y * h / 100.0], Easing::Linear))
                            })
                            .collect();
                        if !ks.is_empty() {
                            c.keyframes.insert("position".into(), ks);
                        }
                    }
                    let k = keys(param(adj, "scale").as_ref(), &|v| pair(v).map(|(a, b)| (a + b) / 2.0));
                    common::set_keys(c, "scale", k, |c, v| c.transform.scale = v);
                    let k = keys(param(adj, "rotation").as_ref(), &|v| v.parse::<f64>().ok().map(|r| -r));
                    common::set_keys(c, "rotation", k, |c, v| c.transform.rotation = v);
                }
                "adjust-conform" => match adj.attr("type") {
                    Some("fill") => c.transform.fit = Fit::Cover,
                    Some("none") => {
                        let (w, hh) = common::clip_size(&self.b.project, c);
                        c.transform.scale /= common::fit_factor(w, hh, self.canvas.0, self.canvas.1);
                    }
                    _ => {}
                },
                "adjust-blend" => {
                    if let Some(a) = adj.attr_f64("amount") {
                        c.transform.opacity = a.clamp(0.0, 1.0);
                    }
                    let k = keys(param(adj, "amount").as_ref(), &|v| v.parse::<f64>().ok().map(|v| v.clamp(0.0, 1.0)));
                    common::set_keys(c, "opacity", k, |c, v| c.transform.opacity = v);
                    if adj.attr("mode").is_some_and(|m| !m.starts_with('0')) {
                        self.b.report.dropped("blend modes");
                    }
                }
                "adjust-volume" => {
                    if let Some(db) = adj.attr("amount").and_then(db) {
                        c.volume = kimchi_core::audio::db_to_gain(db).clamp(0.0, 4.0);
                    }
                    let k = keys(param(adj, "amount").as_ref(), &|v| db(v).map(|d| kimchi_core::audio::db_to_gain(d).clamp(0.0, 4.0)));
                    common::set_keys(c, "volume", k, |c, v| c.volume = v);
                }
                "adjust-panner" => {
                    if let Some(a) = adj.attr_f64("amount") {
                        c.audio.pan = (a / 100.0).clamp(-1.0, 1.0);
                    }
                }
                "adjust-crop" => self.b.report.dropped("crops (kimchi has no crop; scale the clip instead)"),
                "timeMap" => {
                    let pts: Vec<(f64, f64)> = adj.children_named("timept").filter_map(|p| Some((t(p, "time")?, t(p, "value")?))).collect();
                    if let (Some(a), Some(z)) = (pts.first(), pts.last())
                        && z.0 > a.0
                    {
                        let s = (z.1 - a.1) / (z.0 - a.0);
                        if pts.len() > 2 {
                            self.b.report.approximated("speed ramps became one steady speed");
                        }
                        c.speed = s.abs().clamp(0.1, 16.0);
                        c.reverse = s < 0.0;
                        // The source time shown at the clip's first frame.
                        let src = a.1 + (start - a.0) * s;
                        if let ClipContent::Media { .. } = c.content {
                            let src_lo = if c.reverse { src - c.duration * c.speed } else { src };
                            c.in_point = (c.in_point - start + src_lo).max(0.0);
                        }
                    }
                }
                "filter-video" | "filter-audio" => {
                    let n = adj.attr("name").map(str::to_string).or_else(|| adj.attr("ref").and_then(|r| self.effects.get(r).cloned())).unwrap_or_default();
                    *self.dropped_fx.entry(n).or_default() += 1;
                }
                _ => {}
            }
        }
    }

    fn finish(&mut self, version: &str) {
        // Lanes in order: video from the bottom (negative lanes) up, then sound.
        let lanes = std::mem::take(&mut self.lanes);
        let mut primary_named = false;
        for ((_, lane), l) in lanes {
            let name = if !l.name.is_empty() && l.kind == TrackKind::Audio {
                l.name.clone()
            } else if l.kind == TrackKind::Video && lane == 0 && !primary_named {
                primary_named = true;
                "Primary".to_string()
            } else {
                String::new()
            };
            let i = match l.kind {
                TrackKind::Video => self.b.video_track(&name),
                TrackKind::Audio => self.b.audio_track(&name),
            };
            let track = if l.kind == TrackKind::Video { &mut self.b.video[i] } else { &mut self.b.audio[i] };
            track.clips = l.clips;
        }
        if self.disabled > 0 {
            self.b.report.dropped(format!("{} switched-off clip{}", self.disabled, plural(self.disabled)));
        }
        let mut fx: Vec<_> = self.dropped_fx.drain().collect();
        fx.sort();
        for (n, k) in fx {
            self.b.report.dropped(format!("effect \u{201c}{n}\u{201d} on {k} clip{}", plural(k)));
        }
        self.b.report.kept(format!("FCPXML {version}"));
    }
}

fn titlecase(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn db(s: &str) -> Option<f64> {
    s.trim().trim_end_matches("dB").trim().parse().ok()
}

/// `"1 0.5 0 1"` (0…1 channels) → `#ff8000`.
fn color(s: &str) -> String {
    let v: Vec<f64> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    common::hex([v.first().copied().unwrap_or(1.0), v.get(1).copied().unwrap_or(1.0), v.get(2).copied().unwrap_or(1.0), v.get(3).copied().unwrap_or(1.0)])
}

fn color_of(s: &str) -> String {
    if s.starts_with('#') { s.to_string() } else { color(s) }
}

fn color_attr(hex: &str) -> String {
    let c = common::rgba(hex);
    format!("{} {} {} {}", r4(c[0]), r4(c[1]), r4(c[2]), r4(c[3]))
}

fn r4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

// ---------------------------------------------------------------------------------------------
// Writing

pub fn write(project: &Project, path: &Path, opts: &ExportOptions) -> Result<Report> {
    let mut w = Writer::new(project, opts);
    let root = w.document();
    let text = xml::write(&root, Some("<!DOCTYPE fcpxml>"));
    let file = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("fcpxmld")) {
        std::fs::create_dir_all(path).map_err(|e| format!("Couldn't make {}: {e}", path.display()))?;
        path.join("Info.fcpxml")
    } else {
        path.to_path_buf()
    };
    super::write_text(&file, &text)?;
    let mut report = std::mem::take(&mut w.report);
    common::count_kept(project, &mut report);
    common::note_unwritable(project, &mut report, &common::Fits::default());
    Ok(report)
}

struct Writer<'a> {
    p: &'a Project,
    opts: &'a ExportOptions,
    rate: Rate,
    report: Report,
    resources: Element,
    /// Asset resource id by media path.
    assets: HashMap<String, String>,
    effects: HashMap<&'static str, String>,
    next: usize,
    styles: usize,
}

impl<'a> Writer<'a> {
    fn new(p: &'a Project, opts: &'a ExportOptions) -> Self {
        Writer { p, opts, rate: Rate::from_fps(p.settings.fps), report: Report::new("fcpxml"), resources: Element::new("resources"), assets: HashMap::new(), effects: HashMap::new(), next: 1, styles: 0 }
    }

    fn id(&mut self) -> String {
        self.next += 1;
        format!("r{}", self.next)
    }

    fn ts(&self, s: f64) -> String {
        rational(s, self.rate)
    }

    fn document(&mut self) -> Element {
        let rate = self.rate;
        let (w, h) = (self.p.settings.width, self.p.settings.height);
        let frame = super::time::RationalSeconds::of_frames(1, rate).to_string();
        self.resources.push(Element::new("format").with_attr("id", "r1").with_attr("name", format!("FFVideoFormat{h}p{}", rate.timebase())).with_attr("frameDuration", frame).with_attr("width", w).with_attr("height", h).with_attr("colorSpace", "1-1-1 (Rec. 709)"));
        let tracks = common::video_tracks(self.p);
        let primary = tracks.first().copied();
        let mut spine = Element::new("spine");
        // The primary storyline: (position, spine element index) so connected clips can find
        // the element they sit on.
        let mut slots: Vec<(f64, f64, usize)> = vec![];
        let mut at = 0.0;
        if let Some(t) = primary {
            for (i, c) in t.clips.iter().enumerate() {
                if c.start > at + 1e-6 {
                    slots.push((at, c.start, spine.children.len()));
                    spine.push(self.gap(at, c.start - at));
                }
                let prev = i.checked_sub(1).map(|j| &t.clips[j]).filter(|p| (p.end() - c.start).abs() <= kimchi_core::transition::CUT_TOLERANCE);
                if let (Some(tr), Some(p)) = (&c.transition, prev) {
                    let len = kimchi_core::transition::effective_length(tr.duration, c, Some(p));
                    spine.push(self.transition(tr, c.start - len / 2.0, len));
                } else if c.transition.is_some() {
                    self.report.dropped("transitions with no clip before them on the primary storyline (use a fade)");
                }
                if let Some(el) = self.clip(c, c.start, None, true) {
                    slots.push((c.start, c.end(), spine.children.len()));
                    spine.push(el);
                }
                at = c.end();
            }
        }
        // Connected clips: other video tracks above (lanes 1, 2…), sound below (-1, -2…).
        let mut connected: Vec<(i64, &Clip)> = vec![];
        for (k, t) in tracks.iter().enumerate().skip(1) {
            connected.extend(t.clips.iter().map(|c| (k as i64, c)));
        }
        for (k, t) in common::audio_tracks(self.p).iter().enumerate() {
            connected.extend(t.clips.iter().map(|c| (-(k as i64) - 1, c)));
        }
        let mut lost_tr = 0;
        for (lane, c) in connected {
            lost_tr += c.transition.is_some() as usize;
            // The primary element under the clip's start (a gap is added past the end).
            let slot = match slots.iter().find(|(s, e, _)| c.start >= *s - 1e-9 && c.start < *e - 1e-9) {
                Some(s) => *s,
                None => {
                    let end = slots.last().map_or(0.0, |s| s.1);
                    let len = (c.end() - end).max(c.start - end + self.rate.seconds(1.0)).max(self.rate.seconds(1.0));
                    let gap_start = if c.start >= end { end } else { c.start };
                    slots.push((gap_start, gap_start + len, spine.children.len()));
                    spine.push(self.gap(gap_start, len));
                    *slots.last().expect("just pushed")
                }
            };
            let parent_start = t(&spine.children[slot.2], "start").unwrap_or(0.0);
            let offset = parent_start + (c.start - slot.0);
            if let Some(el) = self.clip(c, offset, Some(lane), false) {
                spine.children[slot.2].push(el);
            }
        }
        if lost_tr > 0 {
            self.report.dropped(format!("{lost_tr} transition{} on tracks above the first (Final Cut only has them in storylines)", plural(lost_tr)));
        }
        let mut seq = Element::new("sequence").with_attr("format", "r1").with_attr("duration", self.ts(self.p.duration())).with_attr("tcStart", "0s").with_attr("tcFormat", if rate.is_ntsc() { "DF" } else { "NDF" });
        seq.set_attr("audioLayout", "stereo");
        seq.set_attr("audioRate", if self.p.settings.sample_rate == 44_100 { "44.1k" } else { "48k" });
        // Markers sit on the primary element under them.
        let markers: Vec<(f64, &Marker)> = self.p.markers.iter().map(|m| (m.time, m)).collect();
        for (time, m) in markers {
            if let Some(slot) = slots.iter().find(|(s, e, _)| time >= *s - 1e-9 && time < *e) {
                let st = t(&spine.children[slot.2], "start").unwrap_or(0.0) + (time - slot.0);
                let mk = Element::new("marker").with_attr("start", self.ts(st)).with_attr("duration", super::time::RationalSeconds::of_frames(1, rate).to_string()).with_attr("value", &m.label);
                // Markers go before anchored clips in the DTD's order; Final Cut accepts either.
                let el = &mut spine.children[slot.2];
                let pos = el.children.iter().position(|c| c.attr("lane").is_some()).unwrap_or(el.children.len());
                el.children.insert(pos, mk);
            } else {
                self.report.dropped("markers past the end of the primary storyline");
            }
        }
        seq.push(spine);
        let mut proj = Element::new("project").with_attr("name", &self.p.name).with_attr("uid", self.p.id.to_string().to_uppercase());
        proj.push(seq);
        let event = Element::new("event").with_attr("name", "kimchi").with_child(proj);
        let library = Element::new("library").with_child(event);
        let resources = std::mem::replace(&mut self.resources, Element::new("resources"));
        Element::new("fcpxml").with_attr("version", "1.10").with_child(resources).with_child(library)
    }

    fn gap(&self, _at: f64, len: f64) -> Element {
        Element::new("gap").with_attr("name", "Gap").with_attr("offset", self.ts(_at)).with_attr("start", "0s").with_attr("duration", self.ts(len))
    }

    fn effect(&mut self, key: &'static str, name: &str, uid: &str) -> String {
        if let Some(id) = self.effects.get(key) {
            return id.clone();
        }
        let id = self.id();
        self.resources.push(Element::new("effect").with_attr("id", &id).with_attr("name", name).with_attr("uid", uid));
        self.effects.insert(key, id.clone());
        id
    }

    fn transition(&mut self, tr: &Transition, at: f64, len: f64) -> Element {
        let id = self.effect("dissolve", "Cross Dissolve", CROSS_DISSOLVE);
        if tr.kind != kimchi_core::TransitionKind::Dissolve {
            self.report.approximated(format!("{} became a cross dissolve (change it in Final Cut)", tr.kind.label()));
        }
        let mut el = Element::new("transition").with_attr("name", "Cross Dissolve").with_attr("offset", self.ts(at)).with_attr("duration", self.ts(len));
        el.push(Element::new("filter-video").with_attr("ref", id).with_attr("name", "Cross Dissolve"));
        el
    }

    fn asset(&mut self, path: &str, name: &str, dur: f64, has_video: bool, has_audio: bool, size: (Option<u32>, Option<u32>)) -> String {
        if let Some(id) = self.assets.get(path) {
            return id.clone();
        }
        let id = self.id();
        let mut a = Element::new("asset").with_attr("id", &id).with_attr("name", name).with_attr("uid", format!("kimchi-{}", common::percent_encode(path)).chars().take(200).collect::<String>()).with_attr("start", "0s").with_attr("duration", self.ts(dur));
        a.set_attr("hasVideo", if has_video { "1" } else { "0" });
        if has_video {
            // Its own format when its size differs from the timeline's.
            let fmt = match size {
                (Some(w), Some(h)) if (w, h) != (self.p.settings.width, self.p.settings.height) => {
                    let fid = self.id();
                    self.resources.push(Element::new("format").with_attr("id", &fid).with_attr("width", w).with_attr("height", h));
                    fid
                }
                _ => "r1".to_string(),
            };
            a.set_attr("format", fmt);
            a.set_attr("videoSources", "1");
        }
        a.set_attr("hasAudio", if has_audio { "1" } else { "0" });
        if has_audio {
            a.set_attr("audioSources", "1");
            a.set_attr("audioChannels", "2");
            a.set_attr("audioRate", self.p.settings.sample_rate);
        }
        a.push(Element::new("media-rep").with_attr("kind", "original-media").with_attr("src", common::path_to_url(path)));
        self.resources.push(a);
        self.assets.insert(path.to_string(), id.clone());
        id
    }

    /// A clip at `offset` in its parent's time; `lane` for connected clips.
    fn clip(&mut self, c: &Clip, offset: f64, lane: Option<i64>, primary: bool) -> Option<Element> {
        let rendered = self.opts.rendered.get(&c.id);
        let asset = c.asset_id().and_then(|id| self.p.asset(id));
        let mut el = match (rendered, asset, &c.content) {
            (Some(r), _, _) => {
                let p = r.to_string_lossy().into_owned();
                let id = self.asset(&p, &c.name, c.duration, true, false, (Some(self.p.settings.width), Some(self.p.settings.height)));
                Element::new("asset-clip").with_attr("ref", id).with_attr("start", "0s")
            }
            (None, Some(a), _) => {
                // Files used past what kimchi knows of their length still need room.
                let used = c.in_point + c.duration * c.speed;
                let dur = a.duration().unwrap_or(used).max(used);
                let has_video = a.kind != MediaKind::Audio;
                let has_audio = a.kind != MediaKind::Image && a.meta.has_audio;
                // Sound-only assets share the video file's resource.
                let id = self.asset(&a.path, &a.name, if a.kind == MediaKind::Image { c.duration } else { dur }, has_video || self.assets.contains_key(&a.path), has_audio, (a.meta.width, a.meta.height));
                let mut e = Element::new("asset-clip").with_attr("ref", id).with_attr("start", self.ts(if a.kind == MediaKind::Image { 0.0 } else { c.in_point }));
                if a.kind == MediaKind::Audio && a.meta.has_video {
                    e.set_attr("srcEnable", "audio");
                } else if a.kind == MediaKind::Video && (c.audio.muted || !a.meta.has_audio) && a.meta.has_audio {
                    e.set_attr("srcEnable", "video");
                }
                if a.kind == MediaKind::Audio {
                    e.set_attr("audioRole", "dialogue");
                }
                e
            }
            (None, None, ClipContent::Text { style }) => {
                let id = self.effect("title", "Basic Title", BASIC_TITLE);
                self.styles += 1;
                let ts = format!("ts{}", self.styles);
                let mut e = Element::new("title").with_attr("ref", id).with_attr("start", "0s");
                e.push(Element::new("text").with_child(Element::new("text-style").with_attr("ref", &ts).with_text(&style.content)));
                let mut st = Element::new("text-style")
                    .with_attr("font", &style.font_family)
                    .with_attr("fontSize", r4(style.font_size * 1080.0 / self.p.settings.height.max(1) as f64))
                    .with_attr("fontFace", match (style.font_weight >= 600, style.italic) {
                        (true, true) => "Bold Italic",
                        (true, false) => "Bold",
                        (false, true) => "Italic",
                        _ => "Regular",
                    })
                    .with_attr("fontColor", color_attr(&style.color))
                    .with_attr("alignment", match style.align {
                        TextAlign::Left => "left",
                        TextAlign::Right => "right",
                        TextAlign::Center => "center",
                    });
                if style.font_weight >= 600 {
                    st.set_attr("bold", "1");
                }
                if style.italic {
                    st.set_attr("italic", "1");
                }
                if style.shadow {
                    st.set_attr("shadowColor", "0 0 0 0.75");
                    st.set_attr("shadowOffset", "5 315");
                }
                e.push(Element::new("text-style-def").with_attr("id", ts).with_child(st));
                e
            }
            (None, None, ClipContent::Solid { color }) => {
                let id = self.effect("solid", "Custom", CUSTOM_SOLID);
                let mut v = Element::new("video").with_attr("ref", id).with_attr("start", "0s");
                v.push(Element::new("param").with_attr("name", "Color").with_attr("key", "9999/10003/10003/2/353/113/111").with_attr("value", color_attr(color)));
                v
            }
            _ => {
                self.report.dropped("motion clips (export with renderMotion to bring them as video files)");
                return None;
            }
        };
        el.set_attr("name", &c.name);
        if let Some(l) = lane {
            el.set_attr("lane", l);
        }
        el.set_attr("offset", self.ts(offset));
        el.set_attr("duration", self.ts(c.duration));
        let local0 = t(&el, "start").unwrap_or(0.0);
        // Children in the DTD's order: timing, then video and audio adjustments.
        if (c.speed != 1.0 || c.reverse) && rendered.is_none() && asset.is_some() {
            let (a, z) = (c.in_point, c.in_point + c.duration * c.speed);
            let (v0, v1) = if c.reverse { (z, a) } else { (a, z) };
            let mut tm = Element::new("timeMap");
            tm.push(Element::new("timept").with_attr("time", self.ts(local0)).with_attr("value", self.ts(v0)).with_attr("interp", "linear"));
            tm.push(Element::new("timept").with_attr("time", self.ts(local0 + c.duration)).with_attr("value", self.ts(v1)).with_attr("interp", "linear"));
            el.push(tm);
        }
        let video = !matches!(asset.map(|a| a.kind), Some(MediaKind::Audio));
        if video {
            let h = self.p.settings.height as f64;
            let tf = &c.transform;
            if tf.fit == Fit::Cover {
                el.push(Element::new("adjust-conform").with_attr("type", "fill"));
            }
            let keyed = ["position", "x", "y", "scale", "rotation"].iter().any(|k| c.keyframes.contains_key(*k));
            if tf.x != 0.0 || tf.y != 0.0 || tf.scale != 1.0 || tf.rotation != 0.0 || keyed {
                let mut at = Element::new("adjust-transform").with_attr("position", format!("{} {}", r4(tf.x / h * 100.0), r4(-tf.y / h * 100.0))).with_attr("scale", format!("{0} {0}", r4(tf.scale))).with_attr("rotation", r4(-tf.rotation));
                if let Some(keys) = c.keyframes.get("position") {
                    let mut ka = Element::new("keyframeAnimation");
                    for k in keys {
                        if let Some(p) = k.value.as_vec(2) {
                            ka.push(self.keyframe(local0 + k.time, format!("{} {}", r4(p[0] / h * 100.0), r4(-p[1] / h * 100.0)), k.easing));
                        }
                    }
                    at.push(Element::new("param").with_attr("name", "position").with_child(ka));
                }
                for (name, conv) in [("scale", 1.0), ("rotation", -1.0)] {
                    let ks = common::keys(c, name);
                    if !ks.is_empty() {
                        let mut ka = Element::new("keyframeAnimation");
                        for (time, v, e) in ks {
                            let value = if name == "scale" { format!("{0} {0}", r4(v)) } else { r4(v * conv).to_string() };
                            ka.push(self.keyframe(local0 + time, value, e));
                        }
                        at.push(Element::new("param").with_attr("name", name).with_child(ka));
                    }
                }
                el.push(at);
            }
            if tf.opacity != 1.0 || c.keyframes.contains_key("opacity") {
                let mut b = Element::new("adjust-blend").with_attr("amount", r4(tf.opacity));
                let ks = common::keys(c, "opacity");
                if !ks.is_empty() {
                    let mut ka = Element::new("keyframeAnimation");
                    for (time, v, e) in ks {
                        ka.push(self.keyframe(local0 + time, r4(v).to_string(), e));
                    }
                    b.push(Element::new("param").with_attr("name", "amount").with_child(ka));
                }
                el.push(b);
            }
        }
        let sound = asset.is_some_and(|a| a.kind == MediaKind::Audio || a.meta.has_audio);
        if sound && (c.volume != 1.0 || c.keyframes.contains_key("volume") || c.fade_in > 0.0 || c.fade_out > 0.0) {
            let mut v = Element::new("adjust-volume").with_attr("amount", format!("{}dB", r4(kimchi_core::audio::gain_to_db(c.volume))));
            let ks = common::keys(c, "volume");
            let mut p = Element::new("param").with_attr("name", "amount");
            if c.fade_in > 0.0 {
                p.push(Element::new("fadeIn").with_attr("type", "linear").with_attr("duration", self.ts(c.fade_in)));
            }
            if c.fade_out > 0.0 {
                p.push(Element::new("fadeOut").with_attr("type", "linear").with_attr("duration", self.ts(c.fade_out)));
            }
            if !ks.is_empty() {
                let mut ka = Element::new("keyframeAnimation");
                for (time, g, e) in ks {
                    ka.push(self.keyframe(local0 + time, format!("{}dB", r4(kimchi_core::audio::gain_to_db(g))), e));
                }
                p.push(ka);
            }
            if !p.children.is_empty() {
                v.push(p);
            }
            el.push(v);
        }
        if sound && c.audio.pan != 0.0 {
            el.push(Element::new("adjust-panner").with_attr("amount", r4(c.audio.pan * 100.0)));
        }
        if (c.fade_in > 0.0 || c.fade_out > 0.0) && video {
            self.report.approximated("picture fades: Final Cut fades only the sound; fade the picture there with a cross dissolve");
        }
        let _ = primary;
        Some(el)
    }

    fn keyframe(&self, time: f64, value: String, e: Easing) -> Element {
        let interp = match e {
            Easing::Linear | Easing::Hold => "linear",
            Easing::Curve(_, kimchi_core::anim::Mode::In) => "easeIn",
            Easing::Curve(_, kimchi_core::anim::Mode::Out) => "easeOut",
            _ => "ease",
        };
        Element::new("keyframe").with_attr("time", self.ts(time)).with_attr("value", value).with_attr("interp", interp)
    }
}
