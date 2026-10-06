//! CMX 3600 edit decision lists (`.edl`), as Avid Media Composer, Premiere Pro, Resolve and
//! VEGAS write them: `TITLE:` and `FCM:` (DROP FRAME / NON-DROP FRAME) headers, events
//! `NNN REEL CHANNELS TYPE [DURATION] SRC-IN SRC-OUT REC-IN REC-OUT` with channels V, A, A2,
//! AA, B (picture and sound), AA/V; cuts (C), dissolves (D) and wipes (Wnnn) as an outgoing
//! zero-length C line followed by the transition line; `M2` speed lines; and the comments
//! `* FROM CLIP NAME:` and `* SOURCE FILE:` that name the media.
//!
//! An EDL has one picture track: kimchi writes its bottom video track (and the sound of its
//! clips, plus the first two audio tracks); the other tracks are listed as left out. Source
//! times are written as times from the start of each file (00:00:00:00), record times from
//! 01:00:00:00, and read the same way back.

use std::path::Path;

use kimchi_core::{Clip, ClipContent, MediaKind, Project, Transition, TransitionKind};

use super::common::{self, Builder, MediaInfo, plural};
use super::time::{Rate, parse_timecode, timecode};
use super::{ExportOptions, Imported};
use crate::{Report, Result};

/// Record timecode of the timeline's first frame in written lists.
const RECORD_START: i64 = 3600;

struct Event {
    num: String,
    reel: String,
    channels: String,
    kind: String,
    trans_frames: i64,
    src_in: i64,
    src_out: i64,
    rec_in: i64,
    rec_out: i64,
    clip_name: Option<String>,
    source_file: Option<String>,
    speed: Option<f64>,
}

pub fn read(path: &Path) -> Result<Imported> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n").replace('\r', "\n");
    let title = text.lines().find_map(|l| l.trim().strip_prefix("TITLE:")).map(str::trim).unwrap_or("").to_string();
    let drop = text.lines().any(|l| {
        let l = l.trim().to_ascii_uppercase();
        l.starts_with("FCM:") && l.contains("DROP") && !l.contains("NON")
    });
    let fps = guess_rate(&text, drop);
    let rate = Rate::from_fps(fps);
    let mut events: Vec<Event> = vec![];
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if let Some(c) = l.strip_prefix('*') {
            let c = c.trim();
            let up = c.to_ascii_uppercase();
            if let Some(e) = events.last_mut() {
                if up.starts_with("FROM CLIP NAME:") {
                    e.clip_name = Some(c["FROM CLIP NAME:".len()..].trim().to_string());
                } else if up.starts_with("SOURCE FILE:") {
                    e.source_file = Some(c["SOURCE FILE:".len()..].trim().to_string());
                }
            }
            continue;
        }
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.first() == Some(&"M2") && w.len() >= 4 {
            if let (Ok(v), Some(e)) = (w[2].parse::<f64>(), events.last_mut()) {
                // M2 gives the playback rate in frames per second, to a tenth.
                e.speed = Some((v / rate.fps() * 100.0).round() / 100.0);
            }
            continue;
        }
        if w.len() < 8 || !w[0].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let kind = w[3].to_ascii_uppercase();
        let has_dur = kind != "C";
        let n = if has_dur { 5 } else { 4 };
        if w.len() < n + 4 {
            continue;
        }
        let tc = |s: &str| parse_timecode(s, rate, drop);
        let (Some(si), Some(so), Some(ri), Some(ro)) = (tc(w[n]), tc(w[n + 1]), tc(w[n + 2]), tc(w[n + 3])) else { continue };
        events.push(Event {
            num: w[0].to_string(),
            reel: w[1].to_string(),
            channels: w[2].to_ascii_uppercase(),
            kind,
            trans_frames: if has_dur { w[4].parse().unwrap_or(0) } else { 0 },
            src_in: si,
            src_out: so,
            rec_in: ri,
            rec_out: ro,
            clip_name: None,
            source_file: None,
            speed: None,
        });
    }
    if events.is_empty() {
        return Err("No edits found in this EDL (expected CMX 3600 events).".into());
    }
    let mut b = Builder::new("edl", &title, path);
    b.settings().fps = rate.fps();
    // Record times usually start at an hour (01:00:00:00): the timeline starts there.
    let first = events.iter().map(|e| e.rec_in).min().unwrap_or(0);
    let hour = rate.timebase() * 3600;
    let origin = first - first.rem_euclid(hour);
    let v = b.video_track("V1");
    let (mut a1, mut a2) = (None, None);
    let mut wipes = 0;
    let mut extend = vec![];
    for (i, e) in events.iter().enumerate() {
        if e.rec_out <= e.rec_in {
            continue; // the outgoing half of a transition, or an empty event
        }
        let file = e.source_file.clone().or_else(|| e.clip_name.clone()).unwrap_or_else(|| e.reel.clone());
        if matches!(e.reel.as_str(), "BL" | "BLACK" | "BLK") {
            let start = rate.seconds((e.rec_in - origin) as f64);
            let dur = rate.seconds((e.rec_out - e.rec_in) as f64);
            b.video[v].clips.push(Clip::new("Black", start, dur, ClipContent::Solid { color: "#000000".into() }));
            continue;
        }
        let info = MediaInfo { name: e.clip_name.clone(), ..Default::default() };
        let speed = e.speed.unwrap_or(1.0);
        let start = rate.seconds((e.rec_in - origin) as f64);
        let dur = rate.seconds((e.rec_out - e.rec_in) as f64);
        let make = |content: ClipContent| {
            let mut c = Clip::new(e.clip_name.clone().unwrap_or_else(|| common::file_name(&file)), start, dur, content);
            c.in_point = rate.seconds(e.src_in.min(e.src_out) as f64);
            c.speed = speed.abs().clamp(0.1, 16.0);
            c.reverse = speed < 0.0 || e.src_out < e.src_in;
            c
        };
        let ch = e.channels.as_str();
        let picture = ch.contains('V') || ch == "B";
        let sound = ch == "B" || ch.contains('A');
        if picture {
            let asset = b.media(&file, &info);
            let mut c = make(ClipContent::Media { asset_id: asset });
            if b.asset_kind(asset) == Some(MediaKind::Image) {
                c.in_point = 0.0;
            }
            // A dissolve or wipe into this event: the line before is its outgoing half.
            if e.kind != "C" && i > 0 && events[i - 1].num == e.num {
                let len = rate.seconds(e.trans_frames as f64);
                let kind = if e.kind == "D" { TransitionKind::Dissolve } else { wipes += 1; wipe(&e.kind) };
                // The list's transition starts at the event's record in; kimchi centres it on
                // the cut, where the clip then starts.
                c = c.cut(start + len / 2.0, c.end());
                c.transition = Some(Transition::new(kind, len));
                // The outgoing clip plays on to the cut (its sound is merged first).
                if let Some(prev) = b.video[v].clips.last().filter(|p| (p.end() - start).abs() < 1e-6) {
                    extend.push((prev.id, len / 2.0));
                }
            }
            if sound {
                b.keep_sound.insert(c.id);
            }
            b.video[v].clips.push(c);
        }
        if sound && !picture {
            let asset = b.sound(&file, &info);
            let lane = if ch == "A2" {
                *a2.get_or_insert_with(|| b.audio_track("A2"))
            } else {
                *a1.get_or_insert_with(|| b.audio_track("A1"))
            };
            b.audio[lane].clips.push(make(ClipContent::Media { asset_id: asset }));
            if ch == "AA" {
                let lane2 = *a2.get_or_insert_with(|| b.audio_track("A2"));
                let _ = lane2;
            }
        }
    }
    if wipes > 0 {
        b.report.approximated(format!("{wipes} wipe{} became kimchi wipes (SMPTE wipe patterns other than edge wipes look different)", plural(wipes)));
    }
    b.report.approximated("source timecodes are read as times from the start of each file");
    // Picture clips that only list their picture (V) had their sound on other events.
    for c in &mut b.video[v].clips {
        if !b.keep_sound.contains(&c.id) {
            c.audio.muted = true;
        }
    }
    common::merge_sound(&mut b, false);
    for (id, by) in extend {
        if let Some(c) = b.video[v].clips.iter_mut().find(|c| c.id == id) {
            c.duration += by;
        }
    }
    Ok(b.finish())
}

/// The frame rate: a `* ... FPS` hint some tools write, else the highest frame number seen
/// (25 for PAL lists, 24, 30), else 30.
fn guess_rate(text: &str, drop: bool) -> f64 {
    if drop {
        return 30000.0 / 1001.0;
    }
    let mut max_ff = 0;
    for l in text.lines() {
        for w in l.split_whitespace() {
            let parts: Vec<&str> = w.split([':', ';']).collect();
            if parts.len() == 4
                && let Ok(f) = parts[3].parse::<u32>()
            {
                max_ff = max_ff.max(f);
            }
        }
    }
    match max_ff {
        0..=23 => 24.0,
        24 => 25.0,
        25..=29 => 30.0,
        30..=49 => 50.0,
        _ => 60.0,
    }
}

fn wipe(code: &str) -> TransitionKind {
    // SMPTE wipe codes: 001 horizontal (left to right), 002 vertical (top to bottom).
    match code.trim_start_matches('W').trim_start_matches('0') {
        "2" => TransitionKind::WipeDown,
        _ => TransitionKind::WipeRight,
    }
}

pub fn write(project: &Project, path: &Path, opts: &ExportOptions) -> Result<Report> {
    let rate = Rate::from_fps(project.settings.fps);
    let drop = rate.is_ntsc() && rate.timebase() % 30 == 0;
    let mut report = Report::new("edl");
    let videos = common::video_tracks(project);
    let main = videos.iter().find(|t| !t.clips.is_empty()).copied();
    let others = videos.iter().filter(|t| !t.clips.is_empty()).count().saturating_sub(1);
    if others > 0 {
        report.dropped(format!("{others} more video track{} (an EDL has one picture track)", plural(others)));
    }
    let mut out = format!("TITLE: {}\nFCM: {}\n\n", project.name.chars().take(70).collect::<String>(), if drop { "DROP FRAME" } else { "NON-DROP FRAME" });
    let rec0 = RECORD_START * rate.timebase();
    let tc = |f: i64| timecode(f, rate, drop);
    let n = std::cell::Cell::new(0usize);
    // `same`: the second line of a transition event keeps the event's number.
    let line = |out: &mut String, same: bool, reel: &str, ch: &str, kind: &str, dur: Option<i64>, si: i64, so: i64, ri: i64, ro: i64| {
        if !same {
            n.set(n.get() + 1);
        }
        let d = dur.map_or("   ".to_string(), |d| format!("{d:03}"));
        out.push_str(&format!("{:03}  {reel:<8} {ch:<5} {kind:<4} {d} {} {} {} {}\n", n.get(), tc(si), tc(so), tc(ri), tc(ro)));
    };
    let mut speeds = 0;
    let mut write_clip = |out: &mut String, c: &Clip, ch: &str, prev: Option<&Clip>, report: &mut Report| {
        let (s, e) = (rate.frames(c.start), rate.frames(c.end()));
        let src_in = if matches!(c.content, ClipContent::Media { .. }) { rate.frames(c.in_point) } else { 0 };
        let src_len = rate.frames(c.duration * c.speed).max(1);
        let (file, name) = match (&c.content, opts.rendered.get(&c.id)) {
            (_, Some(r)) => (r.to_string_lossy().into_owned(), c.name.clone()),
            (ClipContent::Media { asset_id }, _) => match project.asset(*asset_id) {
                Some(a) => (a.path.clone(), a.name.clone()),
                None => return,
            },
            (ClipContent::Solid { .. }, _) => {
                line(out, false, "BL", ch, "C", None, 0, e - s, rec0 + s, rec0 + e);
                return;
            }
            _ => {
                report.dropped("titles and motion clips (an EDL only lists media; export with renderMotion)");
                return;
            }
        };
        let reel = "AX";
        let tr = c.transition.as_ref().filter(|_| ch.contains('V') || ch == "B");
        let cut = prev.is_some_and(|p| (p.end() - c.start).abs() <= kimchi_core::transition::CUT_TOLERANCE);
        let (si, so) = if c.reverse { (src_in + src_len, src_in) } else { (src_in, src_in + src_len) };
        match (tr, prev.filter(|_| cut)) {
            (Some(t), Some(p)) => {
                let len = rate.frames(kimchi_core::transition::effective_length(t.duration, c, Some(p))).max(1);
                let start = s - len / 2;
                // The outgoing clip, held for no time, then the transition into this one.
                let p_src = rate.frames(p.in_point + (p.duration - len as f64 / 2.0 / rate.fps()).max(0.0) * p.speed);
                let k = match t.kind {
                    TransitionKind::Dissolve | TransitionKind::DipToBlack | TransitionKind::DipToWhite | TransitionKind::Blur => "D".to_string(),
                    TransitionKind::WipeDown | TransitionKind::WipeUp => "W002".to_string(),
                    _ => "W001".to_string(),
                };
                if k != "D" || t.kind != TransitionKind::Dissolve {
                    report.approximated(format!("{} became {}", t.kind.label(), if k == "D" { "a dissolve" } else { "a wipe" }));
                }
                line(out, false, reel, ch, "C", None, p_src, p_src, rec0 + start, rec0 + start);
                let handle = (len as f64 / 2.0 * c.speed).round() as i64;
                line(out, true, reel, ch, &k, Some(len), si + if c.reverse { 0 } else { -handle }, so, rec0 + start, rec0 + e);
            }
            _ => line(out, false, reel, ch, "C", None, si, so, rec0 + s, rec0 + e),
        }
        if c.speed != 1.0 || c.reverse {
            speeds += 1;
            let v = rate.fps() * c.speed * if c.reverse { -1.0 } else { 1.0 };
            out.push_str(&format!("M2   {reel:<8} {v:>6.1} {}\n", tc(si)));
        }
        out.push_str(&format!("* FROM CLIP NAME: {name}\n* SOURCE FILE: {file}\n\n"));
    };
    if let Some(t) = main {
        for (i, c) in t.clips.iter().enumerate() {
            let ch = if common::plays_sound(project, c) { "B" } else { "V" };
            write_clip(&mut out, c, ch, i.checked_sub(1).map(|j| &t.clips[j]), &mut report);
        }
    }
    let audio = common::audio_tracks(project);
    for (k, t) in audio.iter().take(2).enumerate() {
        for c in &t.clips {
            write_clip(&mut out, c, if k == 0 { "A" } else { "A2" }, None, &mut report);
        }
    }
    let more = audio.iter().skip(2).filter(|t| !t.clips.is_empty()).count();
    if more > 0 {
        report.dropped(format!("{more} audio track{} after the second", plural(more)));
    }
    if speeds > 0 {
        report.kept(format!("{speeds} speed change{}", plural(speeds)));
    }
    let clips = out.matches("FROM CLIP NAME").count();
    report.kept(format!("{clips} event{}", plural(clips)));
    report.dropped("transforms, opacity, volume, fades, titles and markers (an EDL only lists cuts)");
    super::write_text(path, &out)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_premiere_style_list() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cut.edl");
        std::fs::write(
            &p,
            "TITLE: Sequence 01\nFCM: NON-DROP FRAME\n\n001  AX       V     C        00:00:10:00 00:00:15:00 01:00:00:00 01:00:05:00\n* FROM CLIP NAME: beach.mov\n\n002  AX       AA    C        00:00:10:00 00:00:15:00 01:00:00:00 01:00:05:00\n* FROM CLIP NAME: beach.mov\n\n003  AX       V     C        00:00:01:00 00:00:01:00 01:00:05:00 01:00:05:00\n003  AX       V     D    024 00:00:01:00 00:00:06:00 01:00:05:00 01:00:10:00\n* FROM CLIP NAME: city.mov\nM2   AX        048.0                00:00:01:00\n",
        )
        .unwrap();
        let imp = read(&p).unwrap();
        let v = &imp.project.tracks[0];
        assert_eq!(v.clips.len(), 2);
        assert_eq!(v.clips[0].start, 0.0);
        assert!((v.clips[0].in_point - 10.0).abs() < 1e-9);
        // beach's sound lined up with its picture: one clip playing its own sound.
        assert!(!v.clips[0].audio.muted);
        assert!(imp.project.tracks.iter().filter(|t| t.kind == kimchi_core::TrackKind::Audio).all(|t| t.clips.is_empty()));
        let c = &v.clips[1];
        assert_eq!(c.transition.as_ref().unwrap().kind, TransitionKind::Dissolve);
        assert!((c.start - 5.5).abs() < 1e-9, "{}", c.start);
        assert!((c.speed - 2.0).abs() < 1e-9);
    }

    #[test]
    fn junk_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.edl");
        std::fs::write(&p, "hello\n001 nope").unwrap();
        assert!(read(&p).is_err());
    }
}
