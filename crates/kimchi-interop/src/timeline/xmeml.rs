//! Final Cut Pro 7 XML (xmeml versions 4 and 5), as Premiere Pro writes it with File › Export ›
//! Final Cut Pro XML and as Resolve, VEGAS and Avid read it, following Apple's "Final Cut Pro 7
//! XML Interchange Format" (2017 edition): `sequence` with `rate` (timebase + ntsc), `media`
//! with `video`/`audio` `track`s (V1 first), `clipitem` (`start`/`end` on the timeline, `in`/
//! `out` in the source, `-1` next to a transition meaning "up to the transition"), `file`
//! (`pathurl` as a percent-encoded `file://` URL, defined once and then referenced by id),
//! `transitionitem` (`alignment` center, start, end, start-black, end-black), `generatoritem`
//! (Color, Slug, Text), filters (Basic Motion scale / rotation / center, Opacity, Audio Levels,
//! Audio Pan, Time Remap) with `keyframe`s, and `marker`s.
//!
//! Basic Motion's centre is read as a fraction of the frame (0.5 is the right or bottom edge),
//! its scale as a percentage of the picture's own size (Premiere's default when "Scale to frame
//! size" is off); kimchi converts both from and to its own fit-to-canvas scale and pixel
//! offsets using the media's size when the file gives it.

use std::collections::HashMap;
use std::path::Path;

use kimchi_core::{Clip, ClipContent, Easing, Marker, MediaKind, Project, TextStyle, Track, TrackKind, Transition, new_id};

use super::common::{self, Builder, MediaInfo, plural};
use super::time::Rate;
use super::xml::{self, Element};
use super::{ExportOptions, Imported};
use crate::{Report, Result};

struct FileInfo {
    path: String,
    info: MediaInfo,
}

pub fn read(path: &Path) -> Result<Imported> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let root = xml::parse(&text)?;
    if root.name != "xmeml" {
        return Err(format!("This isn't Final Cut Pro 7 XML (its root is <{}>, not <xmeml>).", root.name));
    }
    let sequences = root.find_all("sequence");
    let seq = sequences.iter().find(|s| s.child("media").is_some()).ok_or("This XML has no sequence in it.")?;
    let mut b = Builder::new("xmeml", seq.text_at("name").unwrap_or(""), path);
    let rate = rate_of(seq).unwrap_or(Rate::new(30, 1));
    b.settings().fps = rate.fps();
    let sc = seq.path("media/video/format/samplecharacteristics");
    if let Some(sc) = sc {
        b.settings().width = sc.i64_at("width").unwrap_or(1920).clamp(16, 16384) as u32;
        b.settings().height = sc.i64_at("height").unwrap_or(1080).clamp(16, 16384) as u32;
    }
    if let Some(sr) = seq.i64_at("media/audio/format/samplecharacteristics/samplerate") {
        b.settings().sample_rate = sr.clamp(8000, 384_000) as u32;
    }
    if sequences.len() > 1 {
        b.report.dropped(format!("{} other sequence{} in the file (the first is opened)", sequences.len() - 1, plural(sequences.len() - 1)));
    }
    // Files are described once and then named by id.
    let mut files: HashMap<String, FileInfo> = HashMap::new();
    for f in root.find_all("file") {
        let Some(id) = f.attr("id") else { continue };
        let url = f.text_at("pathurl");
        if url.is_none() && files.contains_key(id) {
            continue;
        }
        let frate = rate_of(f).unwrap_or(rate);
        let info = MediaInfo {
            name: f.text_at("name").map(str::to_string),
            duration: f.f64_at("duration").map(|d| frate.seconds(d)),
            width: f.i64_at("media/video/samplecharacteristics/width").map(|v| v as u32),
            height: f.i64_at("media/video/samplecharacteristics/height").map(|v| v as u32),
            fps: Some(frate.fps()),
            has_video: Some(f.path("media/video").is_some()),
            has_audio: Some(f.path("media/audio").is_some()),
        };
        let path = url.map(str::to_string).or_else(|| f.text_at("name").map(str::to_string)).unwrap_or_default();
        files.insert(id.to_string(), FileInfo { path, info });
    }
    let mut r = Reader { rate, files, disabled: 0, filters: HashMap::new() };
    let (w, h) = (b.project.settings.width, b.project.settings.height);
    for (kind, list) in [(TrackKind::Video, "media/video"), (TrackKind::Audio, "media/audio")] {
        let Some(media) = seq.path(list) else { continue };
        for (n, t) in media.children_named("track").enumerate() {
            let name = format!("{}{}", if kind == TrackKind::Video { "V" } else { "A" }, n + 1);
            let lane = if kind == TrackKind::Video { b.video_track(&name) } else { b.audio_track(&name) };
            let enabled = t.bool_at("enabled").unwrap_or(true);
            let locked = t.bool_at("locked").unwrap_or(false);
            let clips = r.track(&mut b, t, kind, (w, h));
            let track = if kind == TrackKind::Video { &mut b.video[lane] } else { &mut b.audio[lane] };
            track.locked = locked;
            if !enabled {
                if kind == TrackKind::Video { track.hidden = true } else { track.muted = true }
            }
            track.clips = clips;
        }
    }
    for m in seq.children_named("marker") {
        if let Some(mk) = marker(m, rate, 0.0, 0.0) {
            b.project.markers.push(mk);
        }
    }
    if r.disabled > 0 {
        b.report.dropped(format!("{} switched-off clip{}", r.disabled, plural(r.disabled)));
    }
    let mut fx: Vec<_> = r.filters.into_iter().collect();
    fx.sort();
    for (name, n) in fx {
        b.report.dropped(format!("effect \u{201c}{name}\u{201d} on {n} clip{}", plural(n)));
    }
    common::merge_sound(&mut b, true);
    Ok(b.finish())
}

fn rate_of(e: &Element) -> Option<Rate> {
    let r = e.child("rate")?;
    Some(Rate::from_timebase(r.i64_at("timebase")?, r.bool_at("ntsc").unwrap_or(false)))
}

fn marker(m: &Element, rate: Rate, at: f64, source_in: f64) -> Option<Marker> {
    let f = m.i64_at("in")?;
    let mut label = m.text_at("name").unwrap_or("").to_string();
    if let Some(c) = m.text_at("comment") {
        label = if label.is_empty() { c.to_string() } else { format!("{label}: {c}") };
    }
    let color = m.text_at("color").map(common::marker_color).unwrap_or_else(|| common::marker_color("ORANGE"));
    Some(Marker { id: new_id(), time: (at + rate.seconds(f as f64) - source_in).max(0.0), label, color })
}

struct Reader {
    rate: Rate,
    files: HashMap<String, FileInfo>,
    disabled: usize,
    filters: HashMap<String, usize>,
}

/// A transition as listed in a track.
struct Tr {
    start: i64,
    end: i64,
    alignment: String,
    name: String,
}

impl Reader {
    fn track(&mut self, b: &mut Builder, t: &Element, kind: TrackKind, canvas: (u32, u32)) -> Vec<Clip> {
        let items: Vec<&Element> = t.children.iter().filter(|c| matches!(c.name.as_str(), "clipitem" | "transitionitem" | "generatoritem")).collect();
        let trans: Vec<Option<Tr>> = items
            .iter()
            .map(|e| {
                (e.name == "transitionitem").then(|| Tr {
                    start: e.i64_at("start").unwrap_or(0),
                    end: e.i64_at("end").unwrap_or(0),
                    alignment: e.text_at("alignment").unwrap_or("center").to_ascii_lowercase(),
                    name: e.text_at("effect/name").or_else(|| e.text_at("effect/effectid")).unwrap_or("Cross Dissolve").to_string(),
                })
            })
            .collect();
        let mut out: Vec<Clip> = vec![];
        for (i, e) in items.iter().enumerate() {
            if e.name == "transitionitem" {
                continue;
            }
            if e.bool_at("enabled") == Some(false) {
                self.disabled += 1;
                continue;
            }
            let before = i.checked_sub(1).and_then(|j| trans[j].as_ref());
            let after = trans.get(i + 1).and_then(Option::as_ref);
            let mut start = e.i64_at("start").unwrap_or(-1);
            let mut end = e.i64_at("end").unwrap_or(-1);
            if start < 0 {
                start = before.map_or(0, |t| t.start);
            }
            if end < 0 {
                end = after.map_or(start, |t| t.end);
            }
            if end <= start {
                continue;
            }
            let Some(mut clip) = self.item(b, e, kind, start, end, canvas) else { continue };
            // Into this clip from the one before: kimchi starts it at the transition's centre
            // and runs the clip before up to there, both using the same frames as before.
            if let Some(t) = before.filter(|t| t.end > t.start) {
                let (tk, exact) = common::transition_kind(&t.name);
                if !exact {
                    b.report.approximated(format!("transition \u{201c}{}\u{201d} became {}", t.name, tk.label().to_lowercase()));
                }
                let len = self.rate.seconds((t.end - t.start) as f64);
                let ts = self.rate.seconds(t.start as f64);
                match t.alignment.as_str() {
                    "end-black" => {}
                    "start-black" => clip.transition = Some(Transition::new(tk, len)),
                    _ => {
                        let cut = ts + len / 2.0;
                        if let Some(prev) = out.last_mut().filter(|p| p.end() > ts - 1e-6 && p.start < cut)
                            && clip.start < cut
                        {
                            *prev = prev.cut(prev.start, cut);
                            clip = clip.cut(cut, clip.end());
                        }
                        clip.transition = Some(Transition::new(tk, len));
                    }
                }
            }
            if let Some(t) = after.filter(|t| t.alignment == "end-black") {
                clip.fade_out = self.rate.seconds((t.end - t.start) as f64);
                b.report.approximated("transitions to black at the end became fade-outs");
            }
            out.push(clip);
        }
        out
    }

    fn item(&mut self, b: &mut Builder, e: &Element, kind: TrackKind, start: i64, end: i64, canvas: (u32, u32)) -> Option<Clip> {
        let rate = self.rate;
        let name = e.text_at("name").unwrap_or("").to_string();
        let ts = rate.seconds(start as f64);
        let dur = rate.seconds((end - start) as f64);
        let in_f = e.i64_at("in").filter(|v| *v >= 0).unwrap_or(0);
        let mut clip;
        if e.name == "generatoritem" {
            let effect = e.child("effect")?;
            let id = effect.text_at("effectid").or_else(|| effect.text_at("name")).unwrap_or("").to_ascii_lowercase();
            let param = |pid: &str| effect.children_named("parameter").find(|p| p.text_at("parameterid").is_some_and(|i| i.eq_ignore_ascii_case(pid)));
            if id.contains("text") || id.contains("title") {
                let mut style = TextStyle { content: param("str").and_then(|p| p.text_at("value")).unwrap_or(&name).to_string(), ..TextStyle::default() };
                if let Some(f) = param("fontname").and_then(|p| p.text_at("value")) {
                    style.font_family = f.to_string();
                }
                if let Some(sz) = param("fontsize").and_then(|p| p.f64_at("value")) {
                    // FCP 7 sizes are points on a 480-line frame.
                    style.font_size = sz * canvas.1 as f64 / 480.0;
                }
                if let Some(c) = param("fontcolor").and_then(|p| p.child("value")) {
                    style.color = rgb255(c);
                }
                clip = Clip::new(if name.is_empty() { "Text".into() } else { name.clone() }, ts, dur, ClipContent::Text { style });
            } else {
                let color = param("fillcolor").and_then(|p| p.child("value")).map(rgb255).unwrap_or_else(|| "#000000".into());
                if !(id.contains("color") || id.contains("slug") || id.contains("matte")) {
                    b.report.approximated(format!("generator \u{201c}{name}\u{201d} became a colour clip"));
                }
                clip = Clip::new(if name.is_empty() { "Colour".into() } else { name.clone() }, ts, dur, ClipContent::Solid { color });
            }
        } else {
            let f = e.child("file")?;
            let id = f.attr("id").unwrap_or("");
            let (path, info) = match self.files.get(id) {
                Some(fi) => (fi.path.clone(), fi.info.clone()),
                None => (f.text_at("pathurl").or(f.text_at("name")).unwrap_or(&name).to_string(), MediaInfo::default()),
            };
            if path.is_empty() {
                return None;
            }
            let asset = if kind == TrackKind::Audio { b.sound(&path, &info) } else { b.media(&path, &info) };
            clip = Clip::new(if name.is_empty() { common::file_name(&path) } else { name.clone() }, ts, dur, ClipContent::Media { asset_id: asset });
            if b.asset_kind(asset) != Some(MediaKind::Image) {
                clip.in_point = rate.seconds(in_f as f64);
            }
        }
        let size = match &clip.content {
            ClipContent::Media { asset_id } => b.project.asset(*asset_id).map_or((None, None), |a| (a.meta.width, a.meta.height)),
            _ => (Some(canvas.0), Some(canvas.1)),
        };
        let fit = common::fit_factor(size.0, size.1, canvas.0, canvas.1);
        for fl in e.children_named("filter") {
            let Some(fx) = fl.child("effect") else { continue };
            if fl.bool_at("enabled") == Some(false) {
                continue;
            }
            self.filter(&mut clip, fx, in_f, fit, canvas);
        }
        for m in e.children_named("marker") {
            if let Some(mk) = marker(m, rate, ts, clip.in_point) {
                b.project.markers.push(mk);
            }
        }
        Some(clip)
    }

    fn keyed(&self, p: &Element, in_f: i64, scale: impl Fn(f64) -> f64) -> Vec<(f64, f64, Easing)> {
        let kfs: Vec<&Element> = p.children_named("keyframe").collect();
        if kfs.is_empty() {
            return p.f64_at("value").map(|v| vec![(0.0, scale(v), Easing::Linear)]).unwrap_or_default();
        }
        kfs.iter()
            .filter_map(|k| {
                let when = k.f64_at("when")?;
                let v = k.f64_at("value")?;
                let smooth = k.text_at("interpolation/name").is_some_and(|n| n.to_ascii_lowercase().contains("curve") || n.to_ascii_lowercase().contains("bez"));
                Some((self.rate.seconds(when - in_f as f64).max(0.0), scale(v), if smooth { Easing::EASE_IN_OUT } else { Easing::Linear }))
            })
            .collect()
    }

    fn filter(&mut self, c: &mut Clip, fx: &Element, in_f: i64, fit: f64, canvas: (u32, u32)) {
        let id = fx.text_at("effectid").or_else(|| fx.text_at("name")).unwrap_or("").to_ascii_lowercase();
        let params: Vec<&Element> = fx.children_named("parameter").collect();
        let param = |pid: &str| params.iter().copied().find(|p| p.text_at("parameterid").is_some_and(|i| i.eq_ignore_ascii_case(pid)));
        match id.as_str() {
            "basic" | "basic motion" => {
                if let Some(p) = param("scale") {
                    let k = self.keyed(p, in_f, |v| v / 100.0 / fit);
                    common::set_keys(c, "scale", k, |c, v| c.transform.scale = v);
                }
                if let Some(p) = param("rotation") {
                    let k = self.keyed(p, in_f, |v| v);
                    common::set_keys(c, "rotation", k, |c, v| c.transform.rotation = v);
                }
                if let Some(p) = param("center") {
                    let point = |e: &Element| (e.f64_at("horiz").unwrap_or(0.0) * canvas.0 as f64, e.f64_at("vert").unwrap_or(0.0) * canvas.1 as f64);
                    let kfs: Vec<&Element> = p.children_named("keyframe").collect();
                    if kfs.is_empty() {
                        if let Some(v) = p.child("value") {
                            (c.transform.x, c.transform.y) = point(v);
                        }
                    } else {
                        let keys: Vec<kimchi_core::Keyframe> = kfs
                            .iter()
                            .filter_map(|k| {
                                let (x, y) = point(k.child("value")?);
                                Some(kimchi_core::Keyframe::new(self.rate.seconds(k.f64_at("when")? - in_f as f64).max(0.0), [x, y], Easing::Linear))
                            })
                            .collect();
                        if !keys.is_empty() {
                            c.keyframes.insert("position".into(), keys);
                        }
                    }
                }
            }
            "opacity" => {
                if let Some(p) = param("opacity") {
                    let k = self.keyed(p, in_f, |v| (v / 100.0).clamp(0.0, 1.0));
                    common::set_keys(c, "opacity", k, |c, v| c.transform.opacity = v);
                }
            }
            "audiolevels" => {
                if let Some(p) = param("level") {
                    // A gain factor (1 = 0 dB); Premiere sometimes writes dB.
                    let db = p.text_at("valuemin").is_some_and(|v| v.starts_with('-'));
                    let k = self.keyed(p, in_f, |v| if db { kimchi_core::audio::db_to_gain(v) } else { v }.clamp(0.0, 4.0));
                    common::set_keys(c, "volume", k, |c, v| c.volume = v);
                }
            }
            "audiopan" => {
                if let Some(p) = param("pan") {
                    let k = self.keyed(p, in_f, |v| (if v.abs() > 1.0 { v / 100.0 } else { v }).clamp(-1.0, 1.0));
                    common::set_keys(c, "pan", k, |c, v| c.audio.pan = v);
                }
            }
            "timeremap" => {
                if let Some(v) = param("speed").and_then(|p| p.f64_at("value")) {
                    c.speed = (v.abs() / 100.0).clamp(0.1, 16.0);
                    if v < 0.0 {
                        c.reverse = true;
                    }
                }
                if param("reverse").and_then(|p| p.bool_at("value")) == Some(true) {
                    c.reverse = true;
                }
            }
            _ => {
                let n = fx.text_at("name").unwrap_or(&id).to_string();
                *self.filters.entry(n).or_default() += 1;
            }
        }
    }
}

fn rgb255(v: &Element) -> String {
    let ch = |k: &str| v.f64_at(k).unwrap_or(0.0) / 255.0;
    common::hex([ch("red"), ch("green"), ch("blue"), 1.0])
}

// ---------------------------------------------------------------------------------------------
// Writing

pub fn write(project: &Project, path: &Path, opts: &ExportOptions) -> Result<Report> {
    let rate = Rate::from_fps(project.settings.fps);
    let mut report = Report::new("xmeml");
    let mut w = Writer { p: project, rate, opts, files: HashMap::new(), next_item: 0, texts: 0 };
    let (cw, ch) = (project.settings.width, project.settings.height);
    let mut video = Element::new("video");
    video.push(Element::new("format")).push(samplecharacteristics(rate, cw, ch));
    let mut audio_tracks: Vec<Element> = vec![];
    for t in common::video_tracks(project) {
        let (v, sound) = w.track(t, true);
        video.push(v);
        if let Some(s) = sound {
            audio_tracks.push(s);
        }
    }
    let mut own_audio: Vec<Element> = common::audio_tracks(project).into_iter().map(|t| w.track(t, false).0).collect();
    own_audio.extend(audio_tracks);
    let mut audio = Element::new("audio");
    audio.leaf("numOutputChannels", 2);
    let mut af = Element::new("format");
    af.push(Element::new("samplecharacteristics")).leaf("depth", 16).leaf("samplerate", project.settings.sample_rate);
    audio.push(af);
    for t in own_audio {
        audio.push(t);
    }
    let mut seq = Element::new("sequence").with_attr("id", "sequence-1");
    seq.leaf("uuid", project.id).leaf("duration", rate.frames(project.duration())).push(rate_el(rate));
    seq.leaf("name", &project.name);
    let mut tc = Element::new("timecode");
    tc.push(rate_el(rate));
    tc.leaf("string", super::time::timecode(0, rate, false)).leaf("frame", 0).leaf("displayformat", "NDF");
    seq.push(tc);
    let mut media = Element::new("media");
    media.push(video);
    media.push(audio);
    seq.push(media);
    for m in &project.markers {
        let mut mk = Element::new("marker");
        mk.leaf("name", &m.label).leaf("comment", "").leaf("in", rate.frames(m.time)).leaf("out", -1);
        seq.push(mk);
    }
    let root = Element::new("xmeml").with_attr("version", 4).with_child(seq);
    super::write_text(path, &xml::write(&root, Some("<!DOCTYPE xmeml>")))?;
    common::count_kept(project, &mut report);
    if w.texts > 0 {
        report.approximated(format!("{} title{} are Final Cut 7 text generators: Premiere Pro shows them offline (export with renderMotion for video files)", w.texts, plural(w.texts)));
    }
    common::note_unwritable(project, &mut report, &common::Fits::default());
    let eased = project.clips().any(|(_, c)| c.keyframes.values().flatten().any(|k| !matches!(k.easing, Easing::Linear | Easing::Hold)));
    if eased {
        report.approximated("eased keyframes became smooth (Bézier) ones");
    }
    let tr: Vec<&str> = project.clips().filter_map(|(_, c)| c.transition.as_ref()).filter(|t| !matches!(t.kind, kimchi_core::TransitionKind::Dissolve | kimchi_core::TransitionKind::DipToBlack | kimchi_core::TransitionKind::DipToWhite)).map(|t| t.kind.label()).collect();
    if !tr.is_empty() {
        report.approximated("wipes, slides, pushes, zooms, irises and blurs are written by name; the other app picks its closest");
    }
    Ok(report)
}

fn rate_el(rate: Rate) -> Element {
    let mut r = Element::new("rate");
    r.leaf("timebase", rate.timebase()).leaf("ntsc", if rate.is_ntsc() { "TRUE" } else { "FALSE" });
    r
}

fn samplecharacteristics(rate: Rate, w: u32, h: u32) -> Element {
    let mut sc = Element::new("samplecharacteristics");
    sc.push(rate_el(rate));
    sc.leaf("width", w).leaf("height", h).leaf("anamorphic", "FALSE").leaf("pixelaspectratio", "square").leaf("fielddominance", "none");
    sc
}

struct Writer<'a> {
    p: &'a Project,
    rate: Rate,
    opts: &'a ExportOptions,
    /// File ids already described (later clips only name them).
    files: HashMap<String, String>,
    next_item: usize,
    texts: usize,
}

impl Writer<'_> {
    /// A track; for video tracks also the track holding the sound of its clips.
    fn track(&mut self, t: &Track, video: bool) -> (Element, Option<Element>) {
        let rate = self.rate;
        let mut te = Element::new("track");
        let mut sound = Element::new("track");
        let mut has_sound = false;
        for (i, c) in t.clips.iter().enumerate() {
            let prev = i.checked_sub(1).map(|j| &t.clips[j]).filter(|p| (p.end() - c.start).abs() <= kimchi_core::transition::CUT_TOLERANCE);
            let next = t.clips.get(i + 1).filter(|n| (c.end() - n.start).abs() <= kimchi_core::transition::CUT_TOLERANCE && n.transition.is_some());
            // Frames each side give to transitions at their cuts.
            let half = |to: &Clip, from: Option<&Clip>| to.transition.as_ref().map_or(0, |tr| rate.frames(kimchi_core::transition::effective_length(tr.duration, to, from)) / 2);
            let lead = if video && prev.is_some() { half(c, prev) } else { 0 };
            let tail = next.map_or(0, |n| if video { half(n, Some(c)) } else { 0 });
            if video && let Some(tr) = &c.transition {
                let len = rate.frames(kimchi_core::transition::effective_length(tr.duration, c, prev));
                if len > 0 {
                    let s = rate.frames(c.start) - if prev.is_some() { len / 2 } else { 0 };
                    te.push(transition(tr, s, s + len, if prev.is_some() { "center" } else { "start-black" }, rate));
                }
            }
            let item = self.item(c, lead, tail, video, prev.is_some() && lead > 0, next.is_some() && tail > 0);
            if let Some(item) = item {
                te.push(item);
            }
            if video && common::plays_sound(self.p, c) {
                has_sound = true;
                if let Some(a) = self.item(c, 0, 0, false, false, false) {
                    sound.push(a);
                }
            }
        }
        te.leaf("enabled", if (video && t.hidden) || (!video && t.muted) { "FALSE" } else { "TRUE" });
        te.leaf("locked", if t.locked { "TRUE" } else { "FALSE" });
        sound.leaf("enabled", if t.hidden { "FALSE" } else { "TRUE" }).leaf("locked", "FALSE");
        (te, has_sound.then_some(sound))
    }

    /// One clip. `lead`/`tail`: frames of handle before and after it used by transitions;
    /// `open_start`/`open_end` write -1 there, as Final Cut 7 does next to a transition.
    fn item(&mut self, c: &Clip, lead: i64, tail: i64, video: bool, open_start: bool, open_end: bool) -> Option<Element> {
        let rate = self.rate;
        self.next_item += 1;
        let (s, e) = (rate.frames(c.start), rate.frames(c.end()));
        let rendered = self.opts.rendered.get(&c.id);
        let asset = c.asset_id().and_then(|id| self.p.asset(id));
        let mut el;
        let in_f;
        if rendered.is_none() && asset.is_none() {
            el = Element::new("generatoritem").with_attr("id", format!("generatoritem-{}", self.next_item));
            el.leaf("name", &c.name).leaf("enabled", "TRUE").leaf("duration", e - s).push(rate_el(rate));
            el.leaf("start", s).leaf("end", e).leaf("in", 0).leaf("out", e - s);
            let mut fx = Element::new("effect");
            match &c.content {
                ClipContent::Text { style } => {
                    self.texts += 1;
                    fx.leaf("name", "Text").leaf("effectid", "Text").leaf("effectcategory", "Text").leaf("effecttype", "generator").leaf("mediatype", "video");
                    fx.push(param_text("str", "Text", &style.content));
                    fx.push(param_text("fontname", "Font", &style.font_family));
                    fx.push(param_num("fontsize", "Size", style.font_size * 480.0 / self.p.settings.height.max(1) as f64));
                    fx.push(param_color("fontcolor", "Font Color", &style.color));
                }
                ClipContent::Solid { color } => {
                    fx.leaf("name", "Color").leaf("effectid", "Color").leaf("effectcategory", "Matte").leaf("effecttype", "generator").leaf("mediatype", "video");
                    fx.push(param_color("fillcolor", "Color", color));
                }
                _ => return None,
            }
            el.push(fx);
            in_f = 0;
        } else {
            el = Element::new("clipitem").with_attr("id", format!("clipitem-{}", self.next_item));
            let (file_path, name, dur, w, h, has_audio, kind) = match (rendered, asset) {
                (Some(r), _) => (r.to_string_lossy().into_owned(), c.name.clone(), Some(c.duration), Some(self.p.settings.width), Some(self.p.settings.height), false, MediaKind::Video),
                (None, Some(a)) => (a.path.clone(), a.name.clone(), a.duration(), a.meta.width, a.meta.height, a.meta.has_audio, a.kind),
                _ => return None,
            };
            in_f = if rendered.is_some() || kind == MediaKind::Image { 0 } else { rate.frames(c.in_point) };
            let src_len = rate.frames(c.duration * c.speed).max(1);
            el.leaf("masterclipid", format!("masterclip-{}", common::file_name(&file_path))).leaf("name", &c.name).leaf("enabled", "TRUE");
            el.leaf("duration", dur.map_or(in_f + src_len + tail, |d| rate.frames(d))).push(rate_el(rate));
            el.leaf("start", if open_start { -1 } else { s }).leaf("end", if open_end { -1 } else { e });
            // Handles are timeline frames; in the source they last `speed` times as long.
            let (lead, tail) = ((lead as f64 * c.speed).round() as i64, (tail as f64 * c.speed).round() as i64);
            el.leaf("in", (in_f - lead).max(0)).leaf("out", in_f + src_len + tail);
            el.push(self.file(&file_path, &name, dur, w, h, has_audio && kind != MediaKind::Image, kind));
            if !video {
                let mut st = Element::new("sourcetrack");
                st.leaf("mediatype", "audio").leaf("trackindex", 1);
                el.push(st);
            }
        }
        // Filters.
        let mut filters = vec![];
        if c.speed != 1.0 || c.reverse {
            let mut fx = effect("Time Remap", "timeremap", "motion", "motion", "video");
            fx.push(param_num("speed", "speed", c.speed * 100.0 * if c.reverse { -1.0 } else { 1.0 }));
            fx.push(param_text("reverse", "reverse", if c.reverse { "TRUE" } else { "FALSE" }));
            filters.push(fx);
        }
        if video {
            let (mw, mh) = if rendered.is_some() || asset.is_none() { (Some(self.p.settings.width), Some(self.p.settings.height)) } else { common::clip_size(self.p, c) };
            let fit = common::fit_factor(mw, mh, self.p.settings.width, self.p.settings.height);
            let t = &c.transform;
            let keyed = |n: &str| c.keyframes.contains_key(n);
            if t.scale != 1.0 || t.rotation != 0.0 || t.x != 0.0 || t.y != 0.0 || fit != 1.0 || keyed("scale") || keyed("rotation") || keyed("position") || keyed("x") || keyed("y") {
                let mut fx = effect("Basic Motion", "basic", "motion", "motion", "video");
                fx.push(self.param_keys(c, "scale", "Scale", t.scale * fit * 100.0, in_f, |v| v * fit * 100.0));
                fx.push(self.param_keys(c, "rotation", "Rotation", t.rotation, in_f, |v| v));
                let (cw, ch) = (self.p.settings.width as f64, self.p.settings.height as f64);
                let mut center = Element::new("parameter");
                center.leaf("parameterid", "center").leaf("name", "Center");
                let point = |x: f64, y: f64| {
                    let mut v = Element::new("value");
                    v.leaf("horiz", round6(x / cw)).leaf("vert", round6(y / ch));
                    v
                };
                center.push(point(t.x, t.y));
                if let Some(keys) = c.keyframes.get("position") {
                    for k in keys {
                        if let Some(p) = k.value.as_vec(2) {
                            let mut kf = Element::new("keyframe");
                            kf.leaf("when", in_f + rate.frames(k.time));
                            kf.push(point(p[0], p[1]));
                            center.push(kf);
                        }
                    }
                }
                fx.push(center);
                filters.push(fx);
            }
            if t.opacity != 1.0 || keyed("opacity") || c.fade_in > 0.0 || c.fade_out > 0.0 {
                let mut fx = effect("Opacity", "opacity", "motion", "motion", "video");
                let mut keys = common::keys(c, "opacity");
                fades(&mut keys, c, t.opacity);
                fx.push(self.param_list("opacity", "Opacity", t.opacity * 100.0, in_f, &keys, |v| v * 100.0));
                filters.push(fx);
            }
        } else {
            let mut fx = effect("Audio Levels", "audiolevels", "audiolevels", "audiolevels", "audio");
            let mut keys = common::keys(c, "volume");
            fades(&mut keys, c, c.volume);
            fx.push(self.param_list("level", "Level", c.volume, in_f, &keys, |v| v));
            filters.push(fx);
            if c.audio.pan != 0.0 || c.keyframes.contains_key("pan") {
                let mut fx = effect("Audio Pan", "audiopan", "audiopan", "audiopan", "audio");
                fx.push(self.param_keys(c, "pan", "Pan", c.audio.pan, in_f, |v| v));
                filters.push(fx);
            }
        }
        for fx in filters {
            el.push(Element::new("filter").with_child(fx));
        }
        Some(el)
    }

    #[allow(clippy::too_many_arguments)]
    fn file(&mut self, path: &str, name: &str, dur: Option<f64>, w: Option<u32>, h: Option<u32>, has_audio: bool, kind: MediaKind) -> Element {
        if let Some(id) = self.files.get(path) {
            return Element::new("file").with_attr("id", id);
        }
        let id = format!("file-{}", self.files.len() + 1);
        self.files.insert(path.to_string(), id.clone());
        let rate = self.rate;
        let mut f = Element::new("file").with_attr("id", &id);
        f.leaf("name", name).leaf("pathurl", common::path_to_url(path)).push(rate_el(rate));
        if let Some(d) = dur {
            f.leaf("duration", rate.frames(d));
        }
        let mut media = Element::new("media");
        if kind != MediaKind::Audio {
            let mut v = Element::new("video");
            let mut sc = Element::new("samplecharacteristics");
            sc.push(rate_el(rate));
            if let (Some(w), Some(h)) = (w, h) {
                sc.leaf("width", w).leaf("height", h);
            }
            v.push(sc);
            media.push(v);
        }
        if has_audio || kind == MediaKind::Audio {
            let mut a = Element::new("audio");
            let mut sc = Element::new("samplecharacteristics");
            sc.leaf("depth", 16).leaf("samplerate", self.p.settings.sample_rate);
            a.push(sc);
            a.leaf("channelcount", 2);
            media.push(a);
        }
        f.push(media);
        f
    }

    fn param_keys(&self, c: &Clip, name: &str, label: &str, value: f64, in_f: i64, conv: impl Fn(f64) -> f64) -> Element {
        self.param_list(name, label, value, in_f, &common::keys(c, name), conv)
    }

    fn param_list(&self, id: &str, label: &str, value: f64, in_f: i64, keys: &[(f64, f64, Easing)], conv: impl Fn(f64) -> f64) -> Element {
        let mut p = param_num(id, label, value);
        for (t, v, e) in keys {
            let mut k = Element::new("keyframe");
            k.leaf("when", in_f + self.rate.frames(*t)).leaf("value", round6(conv(*v)));
            if !matches!(e, Easing::Linear | Easing::Hold) {
                let mut i = Element::new("interpolation");
                i.leaf("name", "FCPCurve");
                k.push(i);
            }
            p.push(k);
        }
        p
    }
}

/// Fades as keyframes on opacity or volume (kimchi fades both): 0 → `full` over the fade in,
/// back to 0 over the fade out, when the property isn't animated already.
fn fades(keys: &mut Vec<(f64, f64, Easing)>, c: &Clip, full: f64) {
    if !keys.is_empty() || (c.fade_in <= 0.0 && c.fade_out <= 0.0) {
        return;
    }
    if c.fade_in > 0.0 {
        keys.push((0.0, 0.0, Easing::Linear));
        keys.push((c.fade_in, full, Easing::Linear));
    }
    if c.fade_out > 0.0 {
        keys.push(((c.duration - c.fade_out).max(c.fade_in), full, Easing::Linear));
        keys.push((c.duration, 0.0, Easing::Linear));
    }
    if c.fade_in <= 0.0 {
        keys.insert(0, (0.0, full, Easing::Linear));
    }
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn effect(name: &str, id: &str, category: &str, ty: &str, media: &str) -> Element {
    let mut e = Element::new("effect");
    e.leaf("name", name).leaf("effectid", id).leaf("effectcategory", category).leaf("effecttype", ty).leaf("mediatype", media);
    e
}

fn param_num(id: &str, name: &str, v: f64) -> Element {
    let mut p = Element::new("parameter");
    p.leaf("parameterid", id).leaf("name", name).leaf("value", round6(v));
    p
}

fn param_text(id: &str, name: &str, v: &str) -> Element {
    let mut p = Element::new("parameter");
    p.leaf("parameterid", id).leaf("name", name).leaf("value", v);
    p
}

fn param_color(id: &str, name: &str, hex: &str) -> Element {
    let c = common::rgba(hex);
    let mut v = Element::new("value");
    v.leaf("alpha", (c[3] * 255.0).round()).leaf("red", (c[0] * 255.0).round()).leaf("green", (c[1] * 255.0).round()).leaf("blue", (c[2] * 255.0).round());
    let mut p = Element::new("parameter");
    p.leaf("parameterid", id).leaf("name", name);
    p.push(v);
    p
}

fn transition(tr: &Transition, s: i64, e: i64, alignment: &str, rate: Rate) -> Element {
    use kimchi_core::TransitionKind::*;
    let (name, cat) = match tr.kind {
        Dissolve | Blur => ("Cross Dissolve", "Dissolve"),
        DipToBlack => ("Dip to Color Dissolve", "Dissolve"),
        DipToWhite => ("Dip to White", "Dissolve"),
        WipeLeft | WipeRight | WipeUp | WipeDown => ("Edge Wipe", "Wipe"),
        SlideLeft | SlideRight | SlideUp | SlideDown => ("Slide", "Slide"),
        PushLeft | PushRight | PushUp | PushDown => ("Push", "Slide"),
        Zoom => ("Cross Zoom", "Zoom"),
        Iris => ("Iris Round", "Iris"),
    };
    let mut t = Element::new("transitionitem");
    t.push(rate_el(rate));
    t.leaf("start", s).leaf("end", e).leaf("alignment", alignment);
    let mut fx = effect(name, name, cat, "transition", "video");
    fx.leaf("wipecode", 0).leaf("wipeaccuracy", 100).leaf("startratio", 0).leaf("endratio", 1).leaf("reverse", "FALSE");
    t.push(fx);
    t
}
