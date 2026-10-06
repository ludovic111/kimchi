//! The agent panel's parts (the panel itself is `views::agent_panel`): one
//! card per command, and the Changes tab. What the conversation holds lives in
//! `kimchi_agent::Host`, shared with the `agent.*` commands.

pub mod cards;
pub mod changes;

use chrono::{DateTime, Local};
use gpui::{App, Div, Hsla, div, prelude::*, px};
use kimchi_control::Source;
use serde::Deserialize;
use serde_json::Value;

use crate::theme::{ActiveTheme, MONO};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Conversation,
    Changes,
}

/// One undo step as `history.list` reports it.
#[derive(Clone, Debug, Deserialize)]
pub struct Step {
    pub label: String,
    pub source: String,
}

/// `history.list`: undo steps newest first, redo steps next first.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    #[serde(default)]
    pub undo: Vec<Step>,
    #[serde(default)]
    pub redo: Vec<Step>,
    #[serde(default)]
    pub can_undo: bool,
    #[serde(default)]
    pub can_redo: bool,
}

/// `key="value", n=3, ids=[2]` — a one-line view of a command's parameters.
pub fn params_summary(params: &Value) -> String {
    let Some(o) = params.as_object() else { return String::new() };
    let mut out = vec![];
    for (k, v) in o {
        let shown = match v {
            Value::Null => continue,
            Value::String(s) => {
                let short: String = s.chars().take(28).collect();
                if short.len() < s.len() { format!("\"{short}…\"") } else { format!("\"{s}\"") }
            }
            Value::Number(n) => {
                let f = n.as_f64().unwrap_or(0.0);
                if f.fract() == 0.0 { format!("{f:.0}") } else { format!("{f:.2}").trim_end_matches('0').trim_end_matches('.').to_string() }
            }
            Value::Bool(b) => b.to_string(),
            Value::Array(a) => format!("[{}]", a.len()),
            Value::Object(m) => format!("{{{}}}", m.len()),
        };
        out.push(format!("{k}={shown}"));
    }
    out.join(", ")
}

pub fn source_label(s: Source) -> &'static str {
    match s {
        Source::Window => "you",
        Source::Agent => "agent",
        Source::Cli => "cli",
        Source::Mcp => "mcp",
    }
}

/// A small caps badge naming who ran a command: agents and MCP in the accent
/// (AI), the CLI and the person neutral.
pub fn source_badge(source: &str, cx: &App) -> Div {
    let t = cx.theme();
    let ai = matches!(source, "agent" | "mcp");
    let (bg, fg): (Hsla, Hsla) = if ai { (t.accent_soft, t.accent_text) } else { (t.hover, t.text_2) };
    let label = if source == "window" { "you" } else { source };
    div().flex_none().px(px(5.)).py(px(1.)).bg(bg).text_color(fg).font_family(MONO).text_size(px(9.5)).child(label.to_uppercase())
}

/// `14:03:22` in local time.
pub fn clock(at: DateTime<chrono::Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M:%S").to_string()
}

/// `1.2k`, `830`.
pub fn tokens(n: u64) -> String {
    if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() }
}
