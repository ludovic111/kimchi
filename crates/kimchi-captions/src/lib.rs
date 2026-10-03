//! kimchi-captions: captions as timed lines of text.
//!
//! - [`parse`] reads SRT and WebVTT, [`write`] writes them;
//! - [`cues_from_segments`] turns what a speech recogniser heard (long segments) into captions
//!   people can read: at most two lines of `max_chars`, split at word boundaries, each shown for
//!   a share of its segment's time proportional to its length;
//! - [`whisper`] recognises speech locally (OpenAI's Whisper, run by candle on the CPU).

pub mod whisper;

use serde::{Deserialize, Serialize};

/// One caption: shown from `start` to `end` (seconds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Srt,
    Vtt,
}

impl Format {
    /// From a file name's extension (`.srt`, `.vtt`).
    pub fn of_path(path: &std::path::Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "srt" => Some(Format::Srt),
            "vtt" | "webvtt" => Some(Format::Vtt),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Result<Format, String> {
        match s.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
            "srt" | "subrip" => Ok(Format::Srt),
            "vtt" | "webvtt" => Ok(Format::Vtt),
            other => Err(format!("Caption files are srt or vtt, not `{other}`.")),
        }
    }
}

/// Reads SRT or WebVTT (detected from the text). Styling tags are dropped.
pub fn parse(text: &str) -> Result<Vec<Cue>, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = vec![];
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
        let Some(at) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let (a, b) = lines[at].split_once("-->").expect("checked");
        // WebVTT settings follow the end time ("00:01.000 --> 00:02.000 line:90%").
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (timestamp(a.trim()), timestamp(b)) else {
            return Err(format!("Bad caption time line: `{}`", lines[at]));
        };
        let words: Vec<String> = lines[at + 1..].iter().map(|l| strip_tags(l)).collect();
        let text = words.join("\n").trim().to_string();
        if !text.is_empty() && end > start {
            cues.push(Cue { start, end, text });
        }
    }
    if cues.is_empty() {
        return Err("No captions found (expected SRT or WebVTT).".into());
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(cues)
}

/// `01:02:03,456`, `01:02:03.456`, `02:03.456`.
fn timestamp(s: &str) -> Option<f64> {
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut depth = 0;
    for c in line.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ")
}

/// Writes cues as SRT or WebVTT.
pub fn write(cues: &[Cue], format: Format) -> String {
    let mut out = String::new();
    if format == Format::Vtt {
        out.push_str("WEBVTT\n\n");
    }
    for (i, c) in cues.iter().enumerate() {
        if format == Format::Srt {
            out.push_str(&format!("{}\n", i + 1));
        }
        let sep = if format == Format::Srt { ',' } else { '.' };
        out.push_str(&format!("{} --> {}\n{}\n\n", clock(c.start, sep), clock(c.end, sep), c.text.trim()));
    }
    out
}

fn clock(t: f64, sep: char) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// A stretch of recognised speech.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Captions from recognised segments: each at most two lines of `max_chars`, split at word
/// boundaries (preferring the end of a sentence), across segment breaks when the speech runs on.
/// Each word is timed by its share of its segment's letters; a caption runs from its first
/// word to its last.
pub fn cues_from_segments(segments: &[Segment], max_chars: usize) -> Vec<Cue> {
    let max_chars = max_chars.clamp(12, 120);
    // Timed words.
    let mut words: Vec<(f64, f64, &str)> = vec![];
    for seg in segments.iter().filter(|s| s.end > s.start) {
        let list: Vec<&str> = seg.text.split_whitespace().collect();
        let letters: usize = list.iter().map(|w| w.chars().count() + 1).sum::<usize>().max(1);
        let mut t = seg.start;
        for w in list {
            let d = (seg.end - seg.start) * (w.chars().count() + 1) as f64 / letters as f64;
            words.push((t, t + d, w));
            t += d;
        }
    }
    let mut cues = vec![];
    let mut cur: Vec<(f64, f64, &str)> = vec![];
    let flush = |cur: &mut Vec<(f64, f64, &str)>, cues: &mut Vec<Cue>| {
        if let (Some(first), Some(last)) = (cur.first(), cur.last()) {
            let text: Vec<&str> = cur.iter().map(|w| w.2).collect();
            cues.push(Cue { start: first.0, end: last.1, text: wrap(&text.join(" "), max_chars).join("\n") });
        }
        cur.clear();
    };
    for w in words {
        // A pause of a second or more starts a new caption.
        if cur.last().is_some_and(|l| w.0 - l.1 >= 1.0) {
            flush(&mut cur, &mut cues);
        }
        if !cur.is_empty() {
            let mut text: Vec<&str> = cur.iter().map(|w| w.2).collect();
            text.push(w.2);
            if wrap(&text.join(" "), max_chars).len() > 2 {
                flush(&mut cur, &mut cues);
            }
        }
        cur.push(w);
        // A sentence ending past a line is a good place to stop.
        let len: usize = cur.iter().map(|w| w.2.chars().count() + 1).sum();
        if w.2.ends_with(['.', '?', '!']) && len > max_chars {
            flush(&mut cur, &mut cues);
        }
    }
    flush(&mut cur, &mut cues);
    cues
}

/// Moves each segment's edges onto the speech in `samples` (mono, `rate` Hz): recognisers often
/// start a segment at a window's edge, in the silence before the first word.
pub fn tighten(segments: &mut [Segment], samples: &[f32], rate: u32) {
    let frame = (rate as usize / 50).max(1); // 20 ms
    let rms: Vec<f32> = samples.chunks(frame).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
    if rms.is_empty() {
        return;
    }
    let mut sorted = rms.clone();
    sorted.sort_by(f32::total_cmp);
    let loud = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
    let threshold = (loud * 0.15).max(0.003);
    let at = |t: f64| ((t * 50.0) as usize).min(rms.len());
    for seg in segments.iter_mut() {
        let (a, b) = (at(seg.start), at(seg.end).max(at(seg.start)));
        let voiced: Vec<usize> = (a..b).filter(|i| rms[*i] > threshold).collect();
        if let (Some(first), Some(last)) = (voiced.first(), voiced.last()) {
            let (s, e) = (*first as f64 / 50.0 - 0.1, (*last + 1) as f64 / 50.0 + 0.2);
            seg.start = seg.start.max(s);
            seg.end = seg.end.min(e).max(seg.start + 0.2);
        }
    }
}

/// Word-wrapped lines of at most `max` characters (a longer word gets a line of its own).
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > max {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    // Two lines read best balanced: move words down while the second is much shorter.
    if lines.len() == 2 {
        let all: Vec<&str> = text.split_whitespace().collect();
        let mut best = (usize::MAX, lines.clone());
        for cut in 1..all.len() {
            let (a, b) = (all[..cut].join(" "), all[cut..].join(" "));
            if a.chars().count() <= max && b.chars().count() <= max {
                let diff = a.chars().count().abs_diff(b.chars().count());
                if diff < best.0 {
                    best = (diff, vec![a, b]);
                }
            }
        }
        lines = best.1;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_and_vtt_round_trip() {
        let cues = vec![Cue { start: 1.5, end: 3.25, text: "Hello there".into() }, Cue { start: 3661.0, end: 3662.004, text: "Two\nlines".into() }];
        for f in [Format::Srt, Format::Vtt] {
            let back = parse(&write(&cues, f)).unwrap();
            assert_eq!(back.len(), 2);
            assert!((back[1].end - 3662.004).abs() < 1e-6 && back[1].text == "Two\nlines", "{back:?}");
        }
        assert!(write(&cues, Format::Srt).contains("01:01:01,000 --> 01:01:02,004"));
    }

    #[test]
    fn parses_real_world_files() {
        let vtt = "WEBVTT\n\nNOTE made by hand\n\n1\n00:01.000 --> 00:02.500 line:90%\n<v Ann>Hi <b>you</b> &amp; me</v>\n\n00:03.000 --> 00:04.000\nBye\n";
        let cues = parse(vtt).unwrap();
        assert_eq!(cues[0], Cue { start: 1.0, end: 2.5, text: "Hi you & me".into() });
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,000\r\nWindows\r\n\r\n";
        assert_eq!(parse(srt).unwrap()[0].text, "Windows");
        assert!(parse("just words").is_err());
    }

    #[test]
    fn long_segments_become_readable_captions() {
        let seg = Segment { start: 0.0, end: 10.0, text: "This is a fairly long sentence that someone said on camera. And then they kept on talking for quite a while longer.".into() };
        let cues = cues_from_segments(&[seg], 32);
        assert!(cues.len() >= 2, "{cues:?}");
        for c in &cues {
            assert!(c.text.lines().count() <= 2 && c.text.lines().all(|l| l.chars().count() <= 32), "{c:?}");
        }
        assert_eq!(cues[0].start, 0.0);
        assert!((cues.last().unwrap().end - 10.0).abs() < 1e-9);
        assert!(cues.windows(2).all(|w| (w[0].end - w[1].start).abs() < 1e-9));
    }

    #[test]
    fn captions_run_across_segment_breaks() {
        let segs = [
            Segment { start: 0.0, end: 6.0, text: "And so my fellow Americans ask not what your country can do for you, ask what you can".into() },
            Segment { start: 6.0, end: 8.0, text: "do for your country.".into() },
        ];
        let cues = cues_from_segments(&segs, 42);
        assert!(cues.iter().all(|c| c.text.split_whitespace().count() > 2), "no orphan words: {cues:?}");
    }

    #[test]
    fn segments_are_tightened_onto_the_speech() {
        // 2 s of silence, 1 s of tone, silence.
        let mut samples = vec![0.0f32; 16_000 * 4];
        for (i, s) in samples[32_000..48_000].iter_mut().enumerate() {
            *s = (i as f32 * 0.1).sin() * 0.5;
        }
        let mut segs = vec![Segment { start: 0.0, end: 4.0, text: "tone".into() }];
        tighten(&mut segs, &samples, 16_000);
        assert!((segs[0].start - 1.9).abs() < 0.05 && (segs[0].end - 3.2).abs() < 0.05, "{segs:?}");
    }

    #[test]
    fn two_lines_are_balanced() {
        let lines = wrap("one two three four five six seven", 24);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].len().abs_diff(lines[1].len()) <= 8, "{lines:?}");
    }
}
