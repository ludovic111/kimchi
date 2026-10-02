//! Pieces of the Generate tab (the panel itself is `views::generate_panel`):
//! the composer's draft, the model picker, the model's own parameters and the
//! recent generations. Shared helpers for popovers live here too.

pub mod params;
pub mod picker;
pub mod recent;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnimationExt, App, Bounds, Div, IntoElement, Pixels, SharedString, Styled, canvas, div, prelude::*, px};
use kimchi_gen::{ModelInfo, Task};
use serde_json::{Map, Value};

use crate::store::{ComposeRef, ComposeTarget};
use crate::theme::{ActiveTheme, MONO, size as sz};

/// Ratios offered when the model takes any size.
pub const COMMON_RATIOS: [&str; 6] = ["16:9", "9:16", "1:1", "4:3", "3:4", "21:9"];

/// What the composer holds besides the text fields (prompt, negative, seed).
#[derive(Clone, Debug)]
pub struct Draft {
    pub video: bool,
    /// `provider::model` the person chose for this mode (`None`: the default in settings).
    pub model: Option<String>,
    /// `None`: the project's ratio.
    pub aspect: Option<String>,
    /// `None`: the model's first duration.
    pub duration: Option<f64>,
    /// `None`: the model's first resolution.
    pub resolution: Option<String>,
    pub count: u32,
    pub audio: bool,
    pub params: Map<String, Value>,
    pub refs: Vec<ComposeRef>,
    /// Land on the timeline (else the library only).
    pub to_timeline: bool,
    /// A precise spot on the timeline (a gap, after a clip); `None`: the playhead.
    pub target: Option<ComposeTarget>,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            video: false,
            model: None,
            aspect: None,
            duration: None,
            resolution: None,
            count: 1,
            audio: true,
            params: Map::new(),
            refs: vec![],
            to_timeline: true,
            target: None,
        }
    }
}

impl Draft {
    pub fn task(&self) -> Task {
        task_for(self.video, !self.refs.is_empty())
    }

    /// Switches image/video: start and end frames for video, references for images.
    pub fn set_video(&mut self, video: bool) {
        if self.video == video {
            return;
        }
        self.video = video;
        self.model = None;
        self.duration = None;
        self.resolution = None;
        self.params.clear();
        for r in &mut self.refs {
            r.role = if video { if r.role == "reference" { "start_frame" } else { r.role } } else { "reference" };
        }
        self.refs.truncate(if video { 2 } else { 8 });
    }

    /// Adds an input image; a start or end frame replaces the one there was.
    pub fn add_ref(&mut self, r: ComposeRef) {
        if r.role != "reference" {
            self.refs.retain(|x| x.role != r.role);
        }
        self.refs.push(r);
    }

    /// The role the next added image takes.
    pub fn next_role(&self, model: Option<&ModelInfo>) -> &'static str {
        if !self.video {
            return "reference";
        }
        if self.refs.iter().any(|r| r.role == "start_frame") && model.is_some_and(|m| m.end_frame) { "end_frame" } else { "start_frame" }
    }

    /// How many input images the model takes in this mode.
    pub fn max_refs(&self, model: Option<&ModelInfo>) -> usize {
        if self.video {
            if model.is_some_and(|m| m.end_frame) { 2 } else { 1 }
        } else {
            model.map(|m| m.max_images.max(1) as usize).unwrap_or(1)
        }
    }
}

pub fn task_for(video: bool, has_image: bool) -> Task {
    match (video, has_image) {
        (true, true) => Task::ImageToVideo,
        (true, false) => Task::TextToVideo,
        (false, true) => Task::ImageToImage,
        (false, false) => Task::TextToImage,
    }
}

pub fn model_key(m: &ModelInfo) -> String {
    format!("{}::{}", m.provider, m.id)
}

pub fn role_label(role: &str) -> &'static str {
    match role {
        "start_frame" => "Start",
        "end_frame" => "End",
        _ => "Ref",
    }
}

/// `16:9` → (16, 9).
pub fn ratio_parts(r: &str) -> (f32, f32) {
    let mut it = r.split(':').map(|x| x.trim().parse::<f32>().unwrap_or(1.0));
    (it.next().unwrap_or(1.0).max(0.1), it.next().unwrap_or(1.0).max(0.1))
}

/// Records where an element was laid out, so a popover can tell a click on
/// its own trigger (which toggles it) from a click elsewhere (which closes it).
pub fn bounds_probe(cell: Rc<Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(move |b, _, _| cell.set(Some(b)), |_, _, _, _| {}).absolute().inset_0()
}

/// A small option chip (aspect, length, quality, variations).
pub fn chip(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, on: bool, cx: &App) -> gpui::Stateful<Div> {
    chip_base(id, on, cx).child(label.into())
}

/// A chip without its label, for a glyph before it.
pub fn chip_base(id: impl Into<gpui::ElementId>, on: bool, cx: &App) -> gpui::Stateful<Div> {
    let t = cx.theme().clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .px(px(9.))
        .rounded(px(sz::R_SM))
        .font_family(MONO)
        .text_size(px(11.5))
        .border_1()
        .cursor_pointer()
        .when(on, |d| d.bg(t.accent_soft).border_color(t.accent_ring).text_color(t.accent_text))
        .when(!on, |d| d.bg(t.bg_sunken.opacity(0.5)).border_color(t.line).text_color(t.text_2).hover(|s| s.bg(t.hover).text_color(t.text)))
}

/// A caps label over a group of controls.
pub fn eyebrow(text: &str, cx: &App) -> Div {
    crate::ui::caps(text.to_string(), cx)
}

/// `1.2 s`, `48 s`, `3 min`.
pub fn short_duration(secs: f64) -> String {
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 90.0 {
        format!("{secs:.0}s")
    } else {
        format!("{:.0} min", secs / 60.0)
    }
}

/// `just now`, `5 min ago`, `2 h ago`, `3 d ago`.
pub fn ago(t: chrono::DateTime<chrono::Utc>) -> String {
    let s = (chrono::Utc::now() - t).num_seconds().max(0);
    match s {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", (s / 60).max(1)),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// The ref's path as an image source.
pub fn image_path(p: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(p)
}


/// An icon that turns while `spinning` (static when motion is reduced).
pub fn spinner_icon(name: &'static str, spinning: bool, id: &'static str, size: f32) -> gpui::AnyElement {
    let i = crate::ui::icon(name).size(px(size));
    if spinning {
        i.with_animation(id, gpui::Animation::new(std::time::Duration::from_millis(900)).repeat(), |svg, delta| {
            svg.with_transformation(gpui::Transformation::rotate(gpui::percentage(delta)))
        })
        .into_any_element()
    } else {
        i.into_any_element()
    }
}

/// A popover's fill: tier-2 glass composited over the page colour. GPUI has no
/// backdrop blur, so a translucent popover would show the controls under it
/// sharply; this keeps the tint of the glass and the legibility of the blur.
pub fn popover_fill(cx: &App) -> gpui::Hsla {
    let t = cx.theme();
    let (top, bottom): (gpui::Rgba, gpui::Rgba) = (t.glass2.bg.into(), t.bg.into());
    let a = top.a;
    gpui::Rgba { r: top.r * a + bottom.r * (1.0 - a), g: top.g * a + bottom.g * (1.0 - a), b: top.b * a + bottom.b * (1.0 - a), a: 1.0 }.into()
}
