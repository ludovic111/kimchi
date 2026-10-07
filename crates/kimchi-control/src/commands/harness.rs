//! `harness.*`: the agent harness (lsuite `HARNESS.md`): the brief, the skills, the live context
//! and the best look at the current work.

use std::sync::Arc;

use kimchi_core::{Project, TrackKind};
use serde_json::{Value, json};

use crate::harness;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session, Source, err};

/// Frames in a look at a span, when not given.
const LOOK_FRAMES: usize = 8;

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        // Agents call commands by their tool names (clip_addText): they get the texts written so.
        "harness.brief" => Ok(json!({ "brief": for_caller(cx, harness::brief()) })),
        "harness.skills" => Ok(json!(harness::skills())),
        "harness.skill" => {
            let name = a.str("name")?;
            let text = harness::skill(name)?;
            Ok(json!({ "name": name.trim(), "skill": for_caller(cx, text.to_string()) }))
        }
        "harness.context" => {
            let since = a.opt_i64("since").map(|n| n.max(0) as u64);
            // An agent's own changes are no news to it.
            let own: &[Source] = if since.is_some() && cx.source.is_agent() { &[cx.source] } else { &[] };
            let g = harness::context::glance_since(s, since, own);
            Ok(json!({ "context": g.text(), "summary": g.lines.join("\n"), "changes": g.changes, "short": g.short, "seq": g.seq }))
        }
        "harness.look" => look(s, &a).await,
        _ => Err(crate::commands::unhandled(cx)),
    }
}

fn for_caller(cx: &Ctx, text: String) -> String {
    if cx.source.is_agent() { harness::as_tools(&text) } else { text }
}

/// The span `harness.look` covers: a clip's, `from`–`to`, or the whole cut.
fn span(p: &Project, a: &Args) -> CmdResult<(f64, f64)> {
    let end = p.duration();
    if let Some(k) = a.opt_str("clipId") {
        let id = resolve::clip(p, k)?;
        let c = p.clip(id).ok_or("clip not found")?;
        return Ok((c.start, c.end()));
    }
    let from = a.opt_f64("from").unwrap_or(0.0).max(0.0);
    let to = a.opt_f64("to").unwrap_or(end).min(end.max(from));
    if to <= from {
        return Err(format!("The span is empty: `to` ({to}) must come after `from` ({from}), and the cut is {end:.2} s long."));
    }
    Ok((from, to))
}

/// `harness.look`: a labelled sheet of frames over a span, what each frame holds (blank or not),
/// gaps in the picture, the loudness of the mix there, and the overview's problems.
async fn look(s: &Arc<Session>, a: &Args) -> CmdResult {
    let p = s.project()?;
    let end = p.duration();
    if end <= 0.0 {
        return Err("The timeline is empty: there is nothing to look at yet.".into());
    }
    let (from, to) = span(&p, a)?;
    let times: Vec<f64> = match a.array("times") {
        Some(list) => list.iter().map(|t| t.as_f64().ok_or("times are numbers of seconds")).collect::<Result<_, _>>()?,
        None => {
            let n = a.opt_u32("frames").map(|n| n as usize).unwrap_or(LOOK_FRAMES).clamp(1, 16);
            let step = (to - from) / n as f64;
            (0..n).map(|i| round(from + step * (i as f64 + 0.5))).collect()
        }
    };
    if times.is_empty() || times.len() > 16 {
        return Err("Give between 1 and 16 times.".into());
    }

    // Each frame drawn by the compositor (as the export draws it), measured, then one sheet.
    let tools = s.tools()?;
    let sheet = times.len() > 1;
    let w = a.opt_u32("width").unwrap_or(if sheet { 480 } else { 960 }).clamp(64, 1920);
    let h = ((w as f64 * p.settings.height as f64 / p.settings.width.max(1) as f64).round() as u32).max(2);
    let mut frames = vec![];
    let mut stats = vec![];
    for &t in &times {
        let f = kimchi_media::preview::render_frame(&tools, &p, t, w, h).await.map_err(err)?;
        let st = FrameStats::of(&f.rgba);
        stats.push(json!({
            "time": t,
            "brightness": round1(st.mean),
            "detail": round1(st.spread),
            "blank": st.blank(),
            "showing": showing(&p, t),
        }));
        let px = kimchi_media::tiny_skia::Pixmap::from_vec(f.rgba, kimchi_media::tiny_skia::IntSize::from_wh(f.width, f.height).ok_or("bad size")?).ok_or("bad frame")?;
        frames.push((t, px));
    }
    let image = if sheet { crate::commands::motion::contact_sheet(&frames)? } else { frames.pop().map(|(_, p)| p).ok_or("nothing rendered")? };
    let dir = s.cache_dir(p.id).join("renders");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(format!("look-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f")));
    image.save_png(&path).map_err(err)?;

    let mut notes: Vec<String> = vec![];
    let blank: Vec<String> = stats.iter().filter(|f| f["blank"] == true).map(|f| format!("{} s", f["time"])).collect();
    if !blank.is_empty() {
        notes.push(format!("Blank frames (one flat colour) at {}: nothing there, or a clip that didn't draw.", blank.join(", ")));
    }
    let gaps = picture_gaps(&p, from, to);
    for (a, b) in &gaps {
        notes.push(format!("No picture from {a:.2} to {b:.2} s (only the background shows)."));
    }

    // The mix over the span, when there is sound to measure.
    let loudness = if a.bool_or("measure", true) && has_sound(&p) {
        let range = kimchi_media::audio::Range { span: Some((from, to)), ..Default::default() };
        match kimchi_media::audio::measure(&tools, &p, &range).await {
            Ok(l) => {
                let v = crate::commands::audio::loudness_json(&l, p.mixer.master.loudness);
                loudness_notes(&v, p.mixer.master.loudness, &mut notes);
                v
            }
            Err(e) => {
                notes.push(format!("The sound couldn't be measured: {e}"));
                Value::Null
            }
        }
    } else {
        Value::Null
    };
    let problems = crate::commands::project::overview(s).ok().and_then(|v| v.get("problems").cloned()).unwrap_or(json!([]));
    for pr in problems.as_array().into_iter().flatten().filter_map(Value::as_str) {
        notes.push(pr.to_string());
    }
    Ok(json!({
        "path": path,
        "span": { "from": round(from), "to": round(to) },
        "duration": round(end),
        "canvas": { "width": p.settings.width, "height": p.settings.height, "fps": p.settings.fps },
        "frames": stats,
        "gaps": gaps.iter().map(|(a, b)| json!([round(*a), round(*b)])).collect::<Vec<_>>(),
        "loudness": loudness,
        "notes": notes,
    }))
}

/// Brightness and spread of a frame (0–255 luma).
struct FrameStats {
    mean: f64,
    /// Standard deviation of the luma: near 0 for a flat colour.
    spread: f64,
}

impl FrameStats {
    fn of(rgba: &[u8]) -> Self {
        let (mut sum, mut sq, mut n) = (0.0f64, 0.0f64, 0.0f64);
        // Every 4th pixel is plenty for a mean and a spread.
        for px in rgba.chunks_exact(4).step_by(4) {
            let y = 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64;
            sum += y;
            sq += y * y;
            n += 1.0;
        }
        if n == 0.0 {
            return Self { mean: 0.0, spread: 0.0 };
        }
        let mean = sum / n;
        Self { mean, spread: (sq / n - mean * mean).max(0.0).sqrt() }
    }

    /// One flat colour: nothing to see.
    fn blank(&self) -> bool {
        self.spread < 1.5
    }
}

/// The names of the clips that make the picture at `t`, top first.
fn showing(p: &Project, t: f64) -> Vec<String> {
    p.tracks
        .iter()
        .filter(|tr| tr.kind == TrackKind::Video && !tr.hidden)
        .flat_map(|tr| tr.clips.iter().filter(move |c| c.contains(t)).map(|c| c.name.clone()))
        .take(4)
        .collect()
}

/// Spans of at least a frame where no visible video track has a clip.
fn picture_gaps(p: &Project, from: f64, to: f64) -> Vec<(f64, f64)> {
    let mut spans: Vec<(f64, f64)> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video && !t.hidden)
        .flat_map(|t| t.clips.iter().map(|c| (c.start, c.end())))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let min = 1.0 / p.settings.fps.max(1.0);
    let mut gaps = vec![];
    let mut covered = from;
    for (a, b) in spans {
        if a > covered + min && a < to {
            gaps.push((covered, a.min(to)));
        }
        covered = covered.max(b);
        if covered >= to {
            break;
        }
    }
    if covered + min < to {
        gaps.push((covered, to));
    }
    gaps
}

fn has_sound(p: &Project) -> bool {
    p.tracks.iter().any(|t| !t.muted && t.clips.iter().any(|c| crate::commands::audio::has_sound(p, c)))
}

/// What the loudness numbers mean for a video's delivery.
fn loudness_notes(v: &Value, target: Option<f64>, notes: &mut Vec<String>) {
    let Some(i) = v["integrated"].as_f64() else {
        notes.push("The mix is silent here.".into());
        return;
    };
    if let Some(tp) = v["truePeak"].as_f64().filter(|tp| *tp > -1.0) {
        notes.push(format!("The true peak reaches {tp:.1} dBTP: above -1, it may distort once encoded (audio.setMaster limiter on, ceilingDb -1)."));
    }
    match target {
        Some(t) => notes.push(format!("The mix measures {i:.1} LUFS here; exports are brought to the master's {t:.0} LUFS.")),
        None if !(-17.0..=-13.0).contains(&i) => notes.push(format!(
            "The mix measures {i:.1} LUFS and the master has no loudness target: web video wants -14 (social, YouTube) or -16 (Vimeo, podcasts): audio.setMaster loudness."
        )),
        None => {}
    }
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_frames_are_blank_and_pictures_are_not() {
        let flat = vec![20u8; 64 * 64 * 4];
        assert!(FrameStats::of(&flat).blank());
        let stripes: Vec<u8> = (0..64 * 64).flat_map(|i| if (i / 8) % 2 == 0 { [250, 250, 250, 255] } else { [5, 5, 5, 255] }).collect();
        let st = FrameStats::of(&stripes);
        assert!(!st.blank() && st.spread > 50.0, "{}", st.spread);
    }
}
