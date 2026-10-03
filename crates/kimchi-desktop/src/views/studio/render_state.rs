//! Rendering motion clips ahead, as the window shows it: each motion clip is drawn live, has
//! its frames rendered, or was rendered before its scene changed; a render may be running. The
//! timeline's badges, the inspector and the Studio's toolbar show it and offer Render now,
//! Cancel and Go live (`motion.render`, `motion.cancelRender`, `motion.unrender`).

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{AnyElement, App, IntoElement, ParentElement, SharedString, Styled, div, px};
use kimchi_core::{Clip, ClipContent, Id, Project};
use serde_json::json;

use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::Button;

#[derive(Clone, Debug, PartialEq)]
pub enum RenderState {
    /// Drawn frame by frame (quick in the preview, full quality in the export).
    Live,
    /// Plays its rendered frames (by this engine).
    Rendered(String),
    /// Rendered, but the scene changed since: drawn live until rendered again.
    Outdated,
    /// A render is running: progress 0–1, the render's id.
    Rendering(f64, String),
}

impl RenderState {
    pub fn label(&self) -> String {
        match self {
            RenderState::Live => "Live".into(),
            RenderState::Rendered(_) => "Rendered".into(),
            RenderState::Outdated => "Out of date".into(),
            RenderState::Rendering(p, _) => format!("Rendering {:.0}%", p * 100.0),
        }
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        match self {
            RenderState::Live => "live",
            RenderState::Rendered(_) => "rendered",
            RenderState::Outdated => "outdated",
            RenderState::Rendering(..) => "rendering",
        }
    }
}

thread_local! {
    /// Whether each rendered clip's file still matches its scene, by project version (hashing
    /// a scene each frame would be wasteful).
    static CURRENT: RefCell<(usize, HashMap<Id, bool>)> = RefCell::new((0, HashMap::new()));
}

/// A motion clip's state now (`None` for other clips).
pub fn state(store: &Store, project: &std::sync::Arc<Project>, clip: &Clip) -> Option<RenderState> {
    if !matches!(clip.content, ClipContent::Motion { .. }) {
        return None;
    }
    if let Some(r) = store.renders.iter().rev().find(|r| r.clip_id == clip.id && !r.done) {
        return Some(RenderState::Rendering(r.progress.clamp(0.0, 1.0), r.id.clone()));
    }
    let Some(rendered) = &clip.rendered else { return Some(RenderState::Live) };
    let key = std::sync::Arc::as_ptr(project) as usize;
    let current = CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != key {
            *c = (key, HashMap::new());
        }
        *c.1.entry(clip.id).or_insert_with(|| kimchi_media::render::cache::is_current(project, clip))
    });
    Some(if current { RenderState::Rendered(rendered.engine.clone()) } else { RenderState::Outdated })
}

/// The state of a clip by id, read from the app.
pub fn state_of(clip: Id, cx: &App) -> Option<RenderState> {
    let store = cx.store();
    let s = store.read(cx);
    let p = s.project.clone()?;
    let c = p.clip(clip)?;
    state(s, &p, c)
}

/// Render now / Cancel / Go live, for one clip.
pub fn controls(id_prefix: &str, clip: Id, st: &RenderState, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let id = |s: &str| SharedString::from(format!("{id_prefix}-{s}"));
    let mut row = div().flex().flex_wrap().items_center().gap(px(6.));
    let color = match st {
        RenderState::Live => t.text_2,
        RenderState::Rendered(_) => t.success,
        RenderState::Outdated => t.warning,
        RenderState::Rendering(..) => t.accent_text,
    };
    row = row.child(
        div()
            .flex_none()
            .px(px(7.))
            .py(px(2.))
            .rounded_full()
            .border_1()
            .border_color(color.opacity(0.5))
            .text_size(px(sz::XS))
            .text_color(color)
            .child(st.label()),
    );
    match st {
        RenderState::Rendering(p, render) => {
            let render = render.clone();
            row = row
                .child(div().w(px(60.)).h(px(4.)).rounded_full().bg(t.line_strong).child(div().h_full().rounded_full().bg(t.accent).w(px(60. * *p as f32))))
                .child(Button::new(id("cancel"), "Cancel render").small().with_icon("x").on_click(move |_, _, cx| {
                    let render = render.clone();
                    cx.store().update(cx, |s, cx| s.run("motion.cancelRender", json!({ "renderId": render }), cx))
                }));
        }
        _ => {
            let label = if matches!(st, RenderState::Rendered(_)) { "Render again" } else { "Render now" };
            row = row.child(Button::new(id("render"), label).small().with_icon("clapperboard").tooltip("Render the clip ahead at full quality: smooth playback, fast export").on_click(move |_, _, cx| {
                cx.store().update(cx, |s, cx| s.run("motion.render", json!({ "clipIds": [clip] }), cx))
            }));
            if matches!(st, RenderState::Rendered(_) | RenderState::Outdated) {
                row = row.child(Button::new(id("live"), "Go live").small().with_icon("radio").tooltip("Forget the rendered frames: draw the clip live again").on_click(move |_, _, cx| {
                    cx.store().update(cx, |s, cx| s.run("motion.unrender", json!({ "clipIds": [clip] }), cx))
                }));
            }
        }
    }
    row.into_any_element()
}
