//! The motion panel (left column): motion templates that drop at the playhead, each shown
//! with a frame of itself, and a way to ask the agent for a scene of your own.
//!
//! The pictures are drawn once by the same compositor as the preview (in the background, the
//! first time the panel opens) and kept in the data folder.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, Entity, FontWeight, ObjectFit, Render, Subscription, Window, div, img, prelude::*, px};
use kimchi_control::Session;
use kimchi_core::templates::{Ctx, TEMPLATES, Template};
use kimchi_core::{Clip, ClipContent, Project, ProjectSettings, Track, TrackKind};
use serde_json::json;

use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, grey, size as sz};
use crate::ui::{Button, caps, icon};

/// Bumped when the templates' look changes, so their pictures are drawn again.
const THUMBS: u32 = 1;

pub struct MotionPanel {
    store: Entity<Store>,
    /// Pictures of the templates, by id, once drawn.
    thumbs: Vec<(&'static str, PathBuf)>,
    drawing: bool,
    _sub: Subscription,
}

impl MotionPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let sub = cx.observe(&store, |_, _, cx| cx.notify());
        Self { store, thumbs: vec![], drawing: false, _sub: sub }
    }

    /// Draws the templates' pictures that aren't on disk yet (once).
    fn draw_thumbs(&mut self, cx: &mut Context<Self>) {
        if self.drawing {
            return;
        }
        self.drawing = true;
        let session = self.store.read(cx).session.clone();
        let task = gpui_tokio::Tokio::spawn(cx, async move { tokio::task::spawn_blocking(move || thumbs(&session)).await.unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            if let Ok(list) = task.await {
                this.update(cx, |this, cx| {
                    this.thumbs = list;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}

/// A frame of every template on a dark canvas, as PNGs in the data folder.
fn thumbs(session: &Arc<Session>) -> Vec<(&'static str, PathBuf)> {
    let dir = session.data_dir.join("thumbs");
    let _ = std::fs::create_dir_all(&dir);
    let Ok(tools) = session.tools() else { return vec![] };
    let mut out = vec![];
    for t in TEMPLATES {
        let path = dir.join(format!("motion-{}-{THUMBS}.png", t.id));
        if !path.is_file() && draw(&tools, t, &path).is_err() {
            continue;
        }
        out.push((t.id, path));
    }
    out
}

fn draw(tools: &kimchi_media::Tools, t: &Template, path: &std::path::Path) -> Result<(), String> {
    let (w, h) = (1280.0, 720.0);
    let scene = t.build(&Default::default(), Ctx { width: w, height: h, duration: t.duration })?;
    let clip = Clip::new(t.name, 0.0, t.duration, ClipContent::Motion { scene, template: None });
    let mut p = Project::new("thumb", ProjectSettings { width: w as u32, height: h as u32, fps: 30.0, background: "#15151a".into(), sample_rate: 48_000 });
    p.tracks = vec![Track { clips: vec![clip], ..Track::new(TrackKind::Video, "Motion") }];
    let mut r = kimchi_media::render::Renderer::new(tools, &p, 384, 216, 30.0);
    // Most templates have settled a little past the middle.
    let frame = r.frame(t.duration * 0.62).map_err(|e| e.to_string())?;
    frame.save_png(path).map_err(|e| e.to_string())
}

/// Adds a scene to start from at the playhead and selects it.
pub(crate) fn new_scene(three: bool, cx: &mut gpui::App) {
    cx.store().update(cx, |s, cx| {
        let at = s.playback.read(cx).playhead;
        let k = s.project.as_ref().map(|p| p.settings.height as f64 / 1080.0).unwrap_or(1.0);
        let scene = if three {
            json!({
                "type": "3d",
                "background": "#111116",
                "camera": {"position": [0, 1.2, 6], "target": [0, 0.3, 0], "fov": 38},
                "objects": [
                    {"id": "box1", "type": "box", "size": 1.2, "bevel": 0.06, "position": [0, 0.3, 0], "material": {"color": "#ff5a36", "roughness": 0.4},
                     "keyframes": {"rotation.y": [[0, 0], [4, 90, "easeInOutCubic"]]}},
                    {"id": "floor", "type": "plane", "width": 30, "height": 30, "rotation": [-90, 0, 0], "position": [0, -0.3, 0], "material": {"color": "#18181d", "roughness": 0.9}}
                ]
            })
        } else {
            json!({
                "type": "2d",
                "layers": [
                    {"id": "text1", "type": "text", "text": "Your words", "fontSize": 110.0 * k, "fill": "#ffffff",
                     "reveal": {"by": "word", "style": "rise", "progress": 0, "overlap": 1.5}, "keyframes": {"reveal": [[0, 0], [0.8, 1, "easeOutCubic"]]}}
                ]
            })
        };
        s.run_then("motion.add", json!({ "scene": scene, "start": at, "duration": 4 }), cx, |s, v, cx| {
            let made = crate::app::created(&v);
            s.set_selection(made.clone(), cx);
            // Straight into the Studio to make it.
            if let Some(id) = made.first() {
                s.open_studio(*id, cx);
            }
        });
    });
}

impl Render for MotionPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        if self.thumbs.is_empty() {
            self.draw_thumbs(cx);
        }
        let card = |tpl: &'static Template, thumb: Option<PathBuf>| {
            let id = tpl.id;
            let picture = match thumb {
                Some(p) => img(p).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(if tpl.kind == "3d" { "box" } else { "shapes" }).size(px(18.)).text_color(gpui::white().opacity(0.4)))
                    .into_any_element(),
            };
            div()
                .id(id)
                .flex()
                .flex_col()
                .gap(px(5.))
                .cursor_pointer()
                .group("motion-card")
                .child(
                    div()
                        .h(px(78.))
                        .rounded(px(sz::R_MD))
                        .bg(grey(0.05))
                        .border_1()
                        .border_color(t.line)
                        .group_hover("motion-card", |s| s.border_color(t.accent_ring))
                        .overflow_hidden()
                        .relative()
                        .child(picture)
                        .child(
                            div()
                                .absolute()
                                .top(px(5.))
                                .right(px(5.))
                                .px(px(5.))
                                .bg(gpui::black().opacity(0.55))
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(gpui::white().opacity(0.85))
                                .child(if tpl.kind == "3d" { "3D" } else { "2D" }),
                        ),
                )
                .child(div().px(px(2.)).text_size(px(sz::SM)).text_color(t.text_2).truncate().child(tpl.name))
                .tooltip(move |_, cx| crate::ui::tooltip(format!("{} Click to add it at the playhead.", tpl.doc).into(), cx))
                .on_click(move |_, _, cx| {
                    cx.store().update(cx, |s, cx| {
                        let at = s.playback.read(cx).playhead;
                        s.run_then("motion.addTemplate", json!({ "template": id, "start": at }), cx, |s, v, cx| s.set_selection(crate::app::created(&v), cx));
                    })
                })
        };
        let thumb = |id: &str| self.thumbs.iter().find(|(i, _)| *i == id).map(|(_, p)| p.clone());
        let grid = |kind: &'static str| {
            div().px(px(10.)).grid().grid_cols(2).gap(px(8.)).children(TEMPLATES.iter().filter(move |tp| tp.kind == kind).map(|tp| card(tp, thumb(tp.id))))
        };
        div()
            .id("motion-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .pb(px(14.))
            .child(div().flex().items_center().px(px(14.)).pt(px(12.)).pb(px(8.)).child(div().flex_1().text_size(px(sz::BASE)).font_weight(FontWeight::SEMIBOLD).child("Motion")))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .px(px(10.))
                    .pb(px(12.))
                    .child(div().flex_grow_1().flex_basis(px(120.)).child(Button::new("new-2d", "New 2D scene").small().with_icon("shapes").full_width().tooltip("A motion graphics clip at the playhead, opened in the Studio").on_click(|_, _, cx| new_scene(false, cx))))
                    .child(div().flex_grow_1().flex_basis(px(120.)).child(Button::new("new-3d", "New 3D scene").small().with_icon("box").full_width().tooltip("A 3D scene at the playhead with a camera and a floor, opened in the Studio").on_click(|_, _, cx| new_scene(true, cx)))),
            )
            .child(div().px(px(14.)).pb(px(6.)).child(caps("Motion graphics", cx)))
            .child(grid("2d"))
            .child(div().px(px(14.)).pt(px(14.)).pb(px(6.)).child(caps("3D", cx)))
            .child(grid("3d"))
            .child(
                div()
                    .px(px(14.))
                    .pt(px(10.))
                    .text_size(px(sz::SM))
                    .text_color(t.text_2)
                    .line_height(px(sz::SM * 1.45))
                    .child("Click to drop at the playhead; change its words and colours in the inspector. Any clip can be animated from the inspector's Animation section."),
            )
            .child(
                div()
                    .mx(px(10.))
                    .mt(px(14.))
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .rounded(px(sz::R_MD))
                    .bg(t.accent_soft)
                    .border_1()
                    .border_color(t.accent_ring)
                    .child(div().flex().items_center().gap(px(6.)).text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.accent_text).child(icon("bot").text_color(t.accent_text)).child("Your own"))
                    .child(
                        div()
                            .text_size(px(sz::SM))
                            .text_color(t.text)
                            .line_height(px(sz::SM * 1.45))
                            .child("Describe a shot to the agent: kinetic type, an animated chart, a 3D logo or a product turntable. It writes the scene, then checks its frames."),
                    )
                    .child(Button::new("motion-agent", "Ask the agent").small().with_icon("message-square").on_click(|_, _, cx| {
                        cx.store().update(cx, |s, cx| s.set_agent_open(true, cx))
                    })),
            )
    }
}
