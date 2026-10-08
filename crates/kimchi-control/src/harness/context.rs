//! The live context: what the person is looking at, and what changed, in a few lines.
//!
//! "Shorten this", "put a title here", "make the selected clips louder": the words point at the
//! window. Agents get a short `<context>` block (the project, the playhead and what is under it,
//! the selection, the Studio) before each model step, so they know what "this" is without a
//! round trip, and what the person changed while they worked. `harness.context` returns the same.

use std::collections::BTreeMap;

use kimchi_core::{ClipContent, Id, MediaKind, Project};
use serde_json::Value;

use crate::session::{RecentEdit, Session, Source, UiState};

/// Most selected clips listed by name; the rest are counted.
const LISTED: usize = 8;

/// The person's view of kimchi at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glance {
    /// The `<context>` block's lines.
    pub lines: Vec<String>,
    /// What others changed since the point asked about (see [`glance_since`]), when they did.
    pub changes: Option<String>,
    /// One short line for the panel ("2 clips selected · 12.40 s").
    pub short: String,
    /// The latest change's sequence number when it was taken: pass it back as `since` to hear
    /// only about later changes.
    pub seq: u64,
}

impl Glance {
    /// The context block, then the request.
    pub fn frame(&self, prompt: &str) -> String {
        if self.lines.is_empty() {
            return prompt.to_string();
        }
        format!("{}\n\n{prompt}", self.block())
    }

    /// The `<context>` block alone.
    pub fn block(&self) -> String {
        format!("<context>\n{}\n</context>", self.text())
    }

    /// The lines, then the changes.
    pub fn text(&self) -> String {
        let mut text = self.lines.join("\n");
        if let Some(c) = &self.changes {
            text.push('\n');
            text.push_str(c);
        }
        text
    }
}

/// `text` without the context block [`Glance::frame`] put in front of it.
pub fn unframed(text: &str) -> &str {
    match text.strip_prefix("<context>\n").and_then(|rest| rest.split_once("\n</context>\n\n")) {
        Some((_, prompt)) => prompt,
        None => text,
    }
}

/// What the person sees now.
pub fn glance(session: &Session) -> Glance {
    glance_since(session, None, &[])
}

/// What the person sees now, and the changes made since `since` (a [`Glance::seq`]) by sources
/// other than `own` (the agent's own changes are no news to it).
pub fn glance_since(session: &Session, since: Option<u64>, own: &[Source]) -> Glance {
    let ui = session.ui_state();
    let (edits, seq) = session.edits_since(since.unwrap_or(u64::MAX));
    // Under the read lock, without copying the project: the panel asks on every redraw.
    let mut g = session
        .read(|ed| of(ed.project(), &ui))
        .unwrap_or_else(|_| Glance { lines: vec!["No project is open.".into()], short: "No project open".into(), ..Default::default() });
    g.seq = seq;
    if since.is_some() {
        g.changes = changes_line(&edits, own);
    }
    g
}

/// "Changed since your last step: clip.trim ×3 (window), clip.addText (cli)."
fn changes_line(edits: &[RecentEdit], own: &[Source]) -> Option<String> {
    // Consecutive runs of the same command from the same source fold into one (a drag is many).
    let mut runs: Vec<(String, Source, usize)> = vec![];
    for e in edits.iter().filter(|e| !own.contains(&e.source)) {
        match runs.last_mut() {
            Some((c, s, n)) if *c == e.command && *s == e.source => *n += 1,
            _ => runs.push((e.command.clone(), e.source, 1)),
        }
    }
    if runs.is_empty() {
        return None;
    }
    let total = runs.len();
    let mut parts: Vec<String> = runs
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|(c, s, n)| format!("{c}{} ({})", if *n > 1 { format!(" ×{n}") } else { String::new() }, who(*s)))
        .collect();
    if total > 8 {
        parts.insert(0, format!("{} earlier changes", total - 8));
    }
    Some(format!("Changed by someone else since your last step: {}. Check before relying on what you read earlier.", parts.join(", ")))
}

fn who(s: Source) -> &'static str {
    match s {
        Source::Window => "the person, in the window",
        Source::Cli => "a script",
        Source::Agent => "the built-in agent",
        Source::Mcp => "another agent",
    }
}

fn of(p: &Project, ui: &UiState) -> Glance {
    let mut lines = vec!["What the person sees in kimchi (\"this\", \"here\" and \"the selected…\" mean these; project.overview has the rest):".to_string()];
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
    if clips > 0 {
        // Track by track, top first: what is on it and where, so the agent sees the shape of the cut.
        let summary: Vec<String> = p
            .tracks
            .iter()
            .take(8)
            .map(|t| {
                let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
                for c in &t.clips {
                    *kinds.entry(content_kind(p, &c.content)).or_default() += 1;
                }
                let what: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
                let span = match (t.clips.iter().map(|c| c.start).reduce(f64::min), t.clips.iter().map(|c| c.end()).reduce(f64::max)) {
                    (Some(a), Some(b)) => format!(" {}–{}", secs_short(a), secs_short(b)),
                    _ => String::new(),
                };
                let flags = [(t.hidden, "hidden"), (t.muted, "muted"), (t.locked, "locked")].iter().filter(|(on, _)| *on).map(|(_, f)| *f).collect::<Vec<_>>();
                format!(
                    "{}{}: {}{span}",
                    quoted(&t.name),
                    if flags.is_empty() { String::new() } else { format!(" ({})", flags.join(", ")) },
                    if what.is_empty() { "empty".to_string() } else { what.join(", ") }
                )
            })
            .collect();
        lines.push(format!("Tracks, top first: {}.", summary.join("; ")));
    }

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
    Glance { lines, short, ..Default::default() }
}

fn content_kind(p: &Project, c: &ClipContent) -> &'static str {
    match c {
        ClipContent::Media { asset_id } => p.asset(*asset_id).map(|a| kind(a.kind)).unwrap_or("missing media"),
        ClipContent::Text { .. } => "title",
        ClipContent::Solid { .. } => "solid",
        ClipContent::Pending { .. } => "generating",
        ClipContent::Motion { scene, .. } => {
            if scene.is_3d() {
                "3D motion"
            } else {
                "2D motion"
            }
        }
    }
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

fn secs_short(t: f64) -> String {
    format!("{} s", trim_num((t * 100.0).round() / 100.0))
}

fn trim_num(x: f64) -> String {
    let s = format!("{x:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
