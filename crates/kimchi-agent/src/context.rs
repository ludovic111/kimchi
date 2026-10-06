//! What the person is looking at when they ask, sent with each request.
//!
//! "Shorten this", "put a title here", "make the selected clips louder": the words point at the
//! window. Each request to the agent starts with a short `<context>` block (the project, the
//! playhead and what is under it, the selection, the Studio), so the agent knows what "this" is
//! without a round trip, and the panel shows the same thing in one line above the composer. It is
//! part of the person's message, so the thread is only ever appended to (prompt caches and
//! thinking blocks stay valid).

use kimchi_control::Session;
use kimchi_core::{ClipContent, Id, MediaKind, Project};
use serde_json::Value;

/// Most selected clips listed by name; the rest are counted.
const LISTED: usize = 8;

/// The person's view of kimchi at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glance {
    /// The `<context>` block's lines.
    pub lines: Vec<String>,
    /// One short line for the panel ("2 clips selected · 12.40 s").
    pub short: String,
}

impl Glance {
    /// The context block, then the request.
    pub fn frame(&self, prompt: &str) -> String {
        if self.lines.is_empty() {
            return prompt.to_string();
        }
        format!("<context>\n{}\n</context>\n\n{prompt}", self.lines.join("\n"))
    }
}

/// `text` without the context blocks [`Glance::frame`] put in front of it.
pub fn unframed(text: &str) -> &str {
    match text.strip_prefix("<context>\n").and_then(|rest| rest.split_once("\n</context>\n\n")) {
        Some((_, prompt)) => prompt,
        None => text,
    }
}

/// What the person sees now.
pub fn glance(session: &Session) -> Glance {
    let ui = session.ui_state();
    // Under the read lock, without copying the project: the panel asks on every redraw.
    session
        .read(|ed| of(ed.project(), &ui))
        .unwrap_or_else(|_| Glance { lines: vec!["No project is open.".into()], short: "No project open".into() })
}

fn of(p: &Project, ui: &kimchi_control::session::UiState) -> Glance {
    let mut lines = vec!["What the person sees in kimchi as they ask (\"this\", \"here\" and \"the selected…\" mean these; project_overview has the rest):".to_string()];
    let clips: usize = p.tracks.iter().map(|t| t.clips.len()).sum();
    lines.push(format!(
        "Project {}: {}×{} at {} fps, {} long, {} track{}, {clips} clip{}, {} media.",
        quoted(&p.name),
        p.settings.width,
        p.settings.height,
        trim_num(p.settings.fps),
        secs(p.duration()),
        p.tracks.len(),
        plural(p.tracks.len()),
        plural(clips),
        p.assets.len()
    ));

    let playhead = ui.playhead;
    let under: Vec<String> = p
        .tracks
        .iter()
        .filter(|t| !t.hidden)
        .flat_map(|t| t.clips.iter().filter(move |c| c.contains(playhead)).map(move |c| format!("{} ({})", quoted(&c.name), quoted(&t.name))))
        .take(4)
        .collect();
    lines.push(format!(
        "Playhead: {}, {}.{}",
        secs(ui.playhead),
        if ui.playing { "playing" } else { "paused" },
        if under.is_empty() { " Nothing is under it.".to_string() } else { format!(" Under it: {}.", under.join(", ")) }
    ));

    let selected: Vec<String> = ui.selection.iter().filter_map(|id| describe_clip(p, *id)).collect();
    if !selected.is_empty() {
        let more = selected.len().saturating_sub(LISTED);
        let mut list = selected.iter().take(LISTED).cloned().collect::<Vec<_>>().join("; ");
        if more > 0 {
            list.push_str(&format!("; and {more} more"));
        }
        lines.push(format!("Selected clip{}: {list}.", plural(selected.len())));
    }
    let asset = ui.selected_asset.and_then(|id| p.asset(id));
    if let Some(a) = asset {
        lines.push(format!("Selected media: {} ({}, id {}).", quoted(&a.name), kind(a.kind), a.id));
    }

    let studio = ui.studio.as_ref().filter(|s| s["open"] == true);
    if let Some(s) = studio {
        let items: Vec<&str> = s["selection"].as_array().into_iter().flatten().filter_map(Value::as_str).take(LISTED).collect();
        lines.push(format!(
            "The Studio is open on {} ({} motion clip, id {}) at scene time {}{}.",
            quoted(s["clipName"].as_str().unwrap_or("")),
            s["kind"].as_str().unwrap_or("2d").to_uppercase(),
            s["clipId"].as_str().unwrap_or("?"),
            secs(s["sceneTime"].as_f64().unwrap_or(0.0)),
            if items.is_empty() { String::new() } else { format!(", items selected: {}", items.join(", ")) }
        ));
    }
    let panels: Vec<&str> = ui.open.iter().map(String::as_str).filter(|o| *o != "agent").collect();
    if !panels.is_empty() {
        lines.push(format!("Also open: {}.", panels.join(", ")));
    }

    let short = match (studio, selected.len(), asset) {
        (Some(s), _, _) => format!("Studio: {}", quoted(s["clipName"].as_str().unwrap_or(""))),
        (None, 0, Some(a)) => format!("{} selected", quoted(&a.name)),
        (None, 0, None) => format!("Playhead {}", secs(ui.playhead)),
        (None, 1, _) => {
            let name = ui.selection.first().and_then(|id| p.clip(*id)).map(|c| c.name.clone()).unwrap_or_default();
            format!("{} selected · {}", quoted(&name), secs(ui.playhead))
        }
        (None, n, _) => format!("{n} clips selected · {}", secs(ui.playhead)),
    };
    Glance { lines, short }
}

/// `"Beach" (id …, video "beach.mp4" on Video 1, 10.00–14.50 s)`.
fn describe_clip(p: &Project, id: Id) -> Option<String> {
    let (track, c) = p.tracks.iter().find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))?;
    let what = match &c.content {
        ClipContent::Media { asset_id } => match p.asset(*asset_id) {
            Some(a) => format!("{} {}", kind(a.kind), quoted(&a.name)),
            None => "missing media".into(),
        },
        ClipContent::Text { style } => format!("title {}", quoted(&style.content)),
        ClipContent::Solid { color } => format!("solid {color}"),
        ClipContent::Pending { .. } => "still generating".into(),
        ClipContent::Motion { scene, .. } => if scene.is_3d() { "3D motion" } else { "2D motion" }.into(),
    };
    Some(format!("{} (id {}, {what} on {}, {}–{})", quoted(&c.name), c.id, quoted(&track.name), secs(c.start), secs(c.end())))
}

fn kind(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Video => "video",
        MediaKind::Image => "image",
        MediaKind::Audio => "audio",
    }
}

/// A name on one line, quoted, at most 60 characters.
fn quoted(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = one.chars().take(60).collect();
    format!("\"{short}{}\"", if short.len() < one.len() { "…" } else { "" })
}

fn secs(t: f64) -> String {
    format!("{t:.2} s")
}

fn trim_num(x: f64) -> String {
    let s = format!("{x:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
