//! The inspector (right column, glass tier 1): what is selected, and every way to change it.
//!
//! - one clip: name, generation provenance, the AI actions, text style or solid colour,
//!   transform, timing, sound and the source media;
//! - several clips: duplicate, delete, and "bridge" for two;
//! - a media item picked in the media panel: poster, provenance, file facts, insert / reveal;
//! - nothing: the project's canvas, frame rate and background, and the main shortcuts.
//!
//! Every change is a registry command (`clip.update`, `project.setSettings`, …); scrubs and
//! sliders send a `coalesce` key while they move so a drag is one undo step.

pub mod ai;
pub mod color;
pub mod fonts;
pub mod format;
pub mod slider;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FontWeight, ObjectFit, Render, SharedString, Subscription, Window, div, img, prelude::*, px,
};
use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, Fit, Generation, Id, MediaKind, Project, TextAlign, TrackKind};
use serde_json::{Value, json};

use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::{Button, GlassExt, caps, icon, kbd, segmented, switch, tooltip};
use color::{ColorChange, ColorField};
use fonts::{FontPicked, FontPicker};
use format::{ago, bytes, short, timecode};
use slider::Slider;

/// Canvas presets (label, width, height).
const PRESETS: [(&str, u32, u32); 5] = [("1080p", 1920, 1080), ("4K", 3840, 2160), ("Vertical", 1080, 1920), ("Square", 1080, 1080), ("4:5", 1080, 1350)];
const FRAME_RATES: [f64; 5] = [24.0, 25.0, 30.0, 50.0, 60.0];

pub struct Inspector {
    store: Entity<Store>,
    /// The clip the name and text fields were last filled from.
    shown: Option<Id>,
    name: Entity<TextInput>,
    content: Entity<TextInput>,
    font: Entity<FontPicker>,
    text_color: Entity<ColorField>,
    text_box: Entity<ColorField>,
    solid: Entity<ColorField>,
    background: Entity<ColorField>,
    x: Entity<Scrub>,
    y: Entity<Scrub>,
    scale: Entity<Scrub>,
    rotation: Entity<Scrub>,
    opacity: Entity<Slider>,
    speed: Entity<Scrub>,
    fade_in: Entity<Scrub>,
    fade_out: Entity<Scrub>,
    volume: Entity<Slider>,
    font_size: Entity<Scrub>,
    weight: Entity<Scrub>,
    tracking: Entity<Scrub>,
    line: Entity<Scrub>,
    width: Entity<Scrub>,
    height: Entity<Scrub>,
    _subs: Vec<Subscription>,
}

/// A clip field a scrub drives: the `clip.update` params for a value.
type ToParams = fn(f64) -> Value;

impl Inspector {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let mut subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&store, window, |this: &mut Self, _, e: &crate::store::StoreEvent, window, cx| {
                if let crate::store::StoreEvent::EditText = e {
                    crate::ui::input::focus(&this.content, window, cx);
                    this.content.update(cx, |i, cx| i.select_all_text(cx));
                }
            }),
        ];

        // A scrub that updates the selected clip.
        let mut clip_scrub = |cx: &mut Context<Self>, s: Scrub, key: &'static str, to: ToParams| {
            let e = cx.new(|_| s);
            subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ScrubChange, cx| this.update_clip(to(ch.value), key, ch.final_, cx)));
            e
        };
        let x = clip_scrub(cx, Scrub::new("X", 1.0, 0), "x", |v| json!({ "x": v }));
        let y = clip_scrub(cx, Scrub::new("Y", 1.0, 0), "y", |v| json!({ "y": v }));
        let scale = clip_scrub(cx, Scrub::new("Scale", 1.0, 0).unit("%").range(1.0, 1000.0), "scale", |v| json!({ "scale": v / 100.0 }));
        let rotation = clip_scrub(cx, Scrub::new("Rotate", 1.0, 0).unit("°").range(-360.0, 360.0), "rotation", |v| json!({ "rotation": v }));
        let speed = clip_scrub(cx, Scrub::new("Speed", 0.05, 2).unit("×").range(0.1, 16.0), "speed", |v| json!({ "speed": v }));
        let fade_in = clip_scrub(cx, Scrub::new("Fade in", 0.05, 2).unit("s").range(0.0, 60.0), "fadeIn", |v| json!({ "fadeIn": v }));
        let fade_out = clip_scrub(cx, Scrub::new("Fade out", 0.05, 2).unit("s").range(0.0, 60.0), "fadeOut", |v| json!({ "fadeOut": v }));
        let font_size = clip_scrub(cx, Scrub::new("Size", 1.0, 0).range(8.0, 800.0), "fontSize", |v| json!({ "style": { "fontSize": v } }));
        let weight = clip_scrub(cx, Scrub::new("Weight", 50.0, 0).range(100.0, 900.0), "fontWeight", |v| json!({ "style": { "fontWeight": v } }));
        let tracking = clip_scrub(cx, Scrub::new("Track", 0.5, 1).range(-20.0, 60.0), "letterSpacing", |v| json!({ "style": { "letterSpacing": v } }));
        let line = clip_scrub(cx, Scrub::new("Line", 0.05, 2).range(0.6, 3.0), "lineHeight", |v| json!({ "style": { "lineHeight": v } }));

        let mut clip_slider = |cx: &mut Context<Self>, max: f64, key: &'static str, to: ToParams| {
            let e = cx.new(|cx| Slider::new(0.0, max, cx));
            subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ScrubChange, cx| this.update_clip(to(ch.value), key, ch.final_, cx)));
            e
        };
        let opacity = clip_slider(cx, 1.0, "opacity", |v| json!({ "opacity": v }));
        let volume = clip_slider(cx, 2.0, "volume", |v| json!({ "volume": v }));

        // Canvas size: only the committed value (every size change re-lays the preview out).
        let mut canvas_scrub = |cx: &mut Context<Self>, label: &'static str, key: &'static str| {
            let e = cx.new(|_| Scrub::new(label, 2.0, 0).range(16.0, 7680.0));
            subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ScrubChange, cx| {
                if ch.final_ {
                    let v = ((ch.value / 2.0).round() * 2.0) as u32;
                    this.set_settings(json!({ key: v }), cx);
                }
            }));
            e
        };
        let width = canvas_scrub(cx, "W", "width");
        let height = canvas_scrub(cx, "H", "height");

        let name = cx.new(|cx| TextInput::new(cx).placeholder("Clip name"));
        subs.push(cx.subscribe(&name, |this: &mut Self, input, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Blur => {
                let text = input.read(cx).text().trim().to_string();
                let Some(id) = this.shown else { return };
                let current = this.store.read(cx).clip(id).map(|c| c.name.clone());
                if !text.is_empty() && current.as_deref() != Some(text.as_str()) {
                    this.store.update(cx, |s, cx| s.run("clip.update", json!({ "clipId": id, "name": text }), cx));
                }
            }
            InputEvent::Cancel => {
                let current = this.shown.and_then(|id| this.store.read(cx).clip(id).map(|c| c.name.clone())).unwrap_or_default();
                input.update(cx, |i, cx| i.set_text(current, cx));
            }
            InputEvent::Changed(_) => {}
        }));

        let content = cx.new(|cx| TextInput::new(cx).multiline(3).placeholder("Your words"));
        subs.push(cx.subscribe(&content, |this: &mut Self, _, e: &InputEvent, cx| {
            if let InputEvent::Changed(text) = e {
                let name = kimchi_control::commands::clip::text_name(text);
                // Typing is one undo step per pause (the coalesce key).
                this.update_clip(json!({ "style": { "content": text }, "name": name }), "content", false, cx);
            }
        }));

        let font = cx.new(FontPicker::new);
        subs.push(cx.subscribe(&font, |this: &mut Self, _, e: &FontPicked, cx| this.update_clip(json!({ "style": { "fontFamily": e.0.to_string() } }), "font", true, cx)));

        let mut color = |cx: &mut Context<Self>, alpha: Option<&'static str>, to: fn(&str) -> Value| {
            let e = cx.new(|cx| ColorField::new(alpha, cx));
            subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ColorChange, cx| this.update_clip(to(&ch.0), "color", true, cx)));
            e
        };
        let text_color = color(cx, None, |c| json!({ "style": { "color": c } }));
        let text_box = color(cx, Some("cc"), |c| json!({ "style": { "background": c } }));
        let solid = color(cx, None, |c| json!({ "color": c }));
        let background = cx.new(|cx| ColorField::new(None, cx));
        subs.push(cx.subscribe(&background, |this: &mut Self, _, ch: &ColorChange, cx| this.set_settings(json!({ "background": ch.0.get(..7).unwrap_or(&ch.0) }), cx)));

        Self {
            store,
            shown: None,
            name,
            content,
            font,
            text_color,
            text_box,
            solid,
            background,
            x,
            y,
            scale,
            rotation,
            opacity,
            speed,
            fade_in,
            fade_out,
            volume,
            font_size,
            weight,
            tracking,
            line,
            width,
            height,
            _subs: subs,
        }
    }

    // ---- changes ------------------------------------------------------------

    /// The one selected clip.
    fn single(&self, cx: &App) -> Option<Id> {
        let s = self.store.read(cx);
        (s.selection.len() == 1).then(|| s.selection[0]).filter(|id| s.clip(*id).is_some())
    }

    /// `clip.update` on the selected clip; while a control moves (`final_` false) the edits
    /// share a coalesce key, so the whole gesture is one undo step.
    fn update_clip(&mut self, mut params: Value, key: &str, final_: bool, cx: &mut Context<Self>) {
        let Some(id) = self.single(cx) else { return };
        params["clipId"] = json!(id);
        if !final_ {
            params["coalesce"] = json!(format!("{id}:{key}"));
        }
        self.store.update(cx, |s, cx| s.run("clip.update", params, cx));
    }

    fn set_settings(&mut self, params: Value, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.run("project.setSettings", params, cx));
    }

    /// Fills the retained controls from the clip (fields being typed in keep their text).
    fn sync_clip(&mut self, clip: &Clip, window: &Window, cx: &mut Context<Self>) {
        let changed = self.shown != Some(clip.id);
        self.shown = Some(clip.id);
        if changed || !self.name.read(cx).is_focused(window) {
            let n = clip.name.clone();
            self.name.update(cx, |i, cx| i.set_text(n, cx));
        }
        let tf = &clip.transform;
        let set = |e: &Entity<Scrub>, v: f64, cx: &mut Context<Self>| e.update(cx, |s, _| s.set_value(v));
        set(&self.x, tf.x, cx);
        set(&self.y, tf.y, cx);
        set(&self.scale, tf.scale * 100.0, cx);
        set(&self.rotation, tf.rotation, cx);
        set(&self.speed, clip.speed, cx);
        let d = clip.duration;
        self.fade_in.update(cx, |s, _| {
            s.max = d;
            s.set_value(clip.fade_in)
        });
        self.fade_out.update(cx, |s, _| {
            s.max = d;
            s.set_value(clip.fade_out)
        });
        self.opacity.update(cx, |s, _| s.set_value(tf.opacity));
        self.volume.update(cx, |s, _| s.set_value(clip.volume));
        match &clip.content {
            ClipContent::Text { style } => {
                if changed || !self.content.read(cx).is_focused(window) {
                    let c = style.content.clone();
                    self.content.update(cx, |i, cx| i.set_text(c, cx));
                }
                set(&self.font_size, style.font_size, cx);
                set(&self.weight, style.font_weight as f64, cx);
                set(&self.tracking, style.letter_spacing, cx);
                set(&self.line, style.line_height, cx);
                self.font.update(cx, |f, _| f.set_value(&style.font_family));
                self.text_color.update(cx, |f, cx| f.set_value(&style.color, window, cx));
                if let Some(bg) = &style.background {
                    self.text_box.update(cx, |f, cx| f.set_value(bg, window, cx));
                }
            }
            ClipContent::Solid { color } => self.solid.update(cx, |f, cx| f.set_value(color, window, cx)),
            _ => {}
        }
    }

    // ---- views --------------------------------------------------------------

    fn clip_view(&mut self, clip: &Clip, project: &Project, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.sync_clip(clip, window, cx);
        let t = cx.theme().clone();
        let store = self.store.read(cx);
        let asset = store.asset_of(clip).cloned();
        let track_kind = store.track_of(clip.id).map(|t| t.kind);
        let fps = project.settings.fps;
        let kind = asset.as_ref().map(|a| kind_name(a.kind)).unwrap_or(match clip.content {
            ClipContent::Text { .. } => "text",
            ClipContent::Solid { .. } => "solid",
            ClipContent::Pending { .. } => "generating",
            ClipContent::Media { .. } => "media",
        });
        let has_picture = matches!(clip.content, ClipContent::Media { .. }) && asset.as_ref().is_some_and(|a| a.kind != MediaKind::Audio);
        let has_sound = asset.as_ref().is_some_and(|a| a.kind == MediaKind::Audio || (a.kind == MediaKind::Video && a.meta.has_audio));

        let mut out = vec![];
        let title = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(10.))
            .border_b_1()
            .border_color(t.line)
            .child(div().flex_1().min_w_0().child(self.name.clone()))
            .child(pill(kind, cx));

        let mut body: Vec<AnyElement> = vec![];
        if let Some(a) = &asset
            && let AssetOrigin::Generated(g) = &a.origin
        {
            let (c1, c2) = (clip.clone(), clip.clone());
            let buttons = pair(
                Button::new("regen", "Regenerate").small().with_icon("refresh-cw").on_click(move |_, _, cx| ai::regenerate_clip(&c1, false, cx)),
                Button::new("variation", "Variation").small().with_icon("shuffle").on_click(move |_, _, cx| ai::regenerate_clip(&c2, true, cx)),
            );
            body.push(self.provenance(g, a, Some(buttons.into_any_element()), cx));
        }

        if has_picture {
            let still = asset.as_ref().is_some_and(|a| matches!(a.kind, MediaKind::Video | MediaKind::Image));
            let (c1, c2, c3) = (clip.clone(), clip.clone(), clip.clone());
            body.push(
                section(cx)
                    .child(caps("With AI", cx))
                    .child(ai_button("ai-animate", "clapperboard", "Animate this frame", cx).on_click(move |_, _, cx| ai::animate_frame(&c1, cx)))
                    .when(still, |d| d.child(ai_button("ai-extend", "arrow-right-to-line", "Extend shot", cx).on_click(move |_, _, cx| ai::extend_clip(&c2, cx))))
                    .child(ai_button("ai-restyle", "wand-sparkles", "Restyle frame", cx).on_click(move |_, _, cx| ai::restyle_frame(&c3, cx)))
                    .into_any_element(),
            );
        }

        match &clip.content {
            ClipContent::Text { style } => {
                let align = style.align;
                let (italic, shadow, boxed) = (style.italic, style.shadow, style.background.is_some());
                let this = cx.entity();
                let (e1, e2, e3, e4) = (this.clone(), this.clone(), this.clone(), this.clone());
                body.push(
                    section(cx)
                        .child(caps("Text", cx))
                        .child(self.content.clone())
                        .child(div().flex().gap(px(6.)).child(self.font.clone()))
                        .child(self.text_color.clone())
                        .child(grid2().child(self.font_size.clone()).child(self.weight.clone()).child(self.tracking.clone()).child(self.line.clone()))
                        .child(icon_segmented(
                            "align",
                            vec![(TextAlign::Left, "align-left", "Left"), (TextAlign::Center, "align-center", "Centre"), (TextAlign::Right, "align-right", "Right")],
                            align,
                            move |a, _, cx| {
                                let v = match a {
                                    TextAlign::Left => "left",
                                    TextAlign::Center => "center",
                                    TextAlign::Right => "right",
                                };
                                e1.update(cx, |this, cx| this.update_clip(json!({ "style": { "align": v } }), "align", true, cx));
                            },
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(switch("italic", "Italic", italic, move |v, _, cx| e2.update(cx, |this, cx| this.update_clip(json!({ "style": { "italic": v } }), "italic", true, cx)), cx))
                                .child(switch("shadow", "Shadow", shadow, move |v, _, cx| e3.update(cx, |this, cx| this.update_clip(json!({ "style": { "shadow": v } }), "shadow", true, cx)), cx))
                                .child(switch(
                                    "box",
                                    "Box behind the words",
                                    boxed,
                                    move |v, _, cx| {
                                        let bg = if v { json!("#000000cc") } else { Value::Null };
                                        e4.update(cx, |this, cx| this.update_clip(json!({ "style": { "background": bg } }), "box", true, cx))
                                    },
                                    cx,
                                )),
                        )
                        .when(boxed, |d| d.child(self.text_box.clone()))
                        .into_any_element(),
                );
            }
            ClipContent::Solid { .. } => body.push(section(cx).child(caps("Colour", cx)).child(self.solid.clone()).into_any_element()),
            ClipContent::Pending { prompt, model_name, .. } => body.push(
                section(cx)
                    .child(div().flex().items_center().gap(px(6.)).text_color(t.accent_text).child(icon("loader-circle").text_color(t.accent_text)).child(caps("Generating", cx)))
                    .child(div().text_size(px(sz::MD)).child(prompt.clone()))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(model_name.clone()))
                    .into_any_element(),
            ),
            ClipContent::Media { .. } => {}
        }

        if !matches!(clip.content, ClipContent::Pending { .. }) && track_kind == Some(TrackKind::Video) {
            let fit = clip.transform.fit;
            let reset_fit = match fit {
                Fit::Contain => "contain",
                Fit::Cover => "cover",
                Fit::Stretch => "stretch",
            };
            let this = cx.entity();
            let reset_this = this.clone();
            let opacity = clip.transform.opacity;
            body.push(
                section(cx)
                    .child(
                        div().flex().items_center().justify_between().child(caps("Transform", cx)).child(
                            Button::icon("reset-transform", "rotate-ccw", "Reset position, scale, rotation and opacity").small().on_click(move |_, _, cx| {
                                reset_this.update(cx, |this, cx| {
                                    this.update_clip(json!({ "x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1, "fit": reset_fit }), "reset", true, cx)
                                })
                            }),
                        ),
                    )
                    .child(grid2().child(self.x.clone()).child(self.y.clone()).child(self.scale.clone()).child(self.rotation.clone()))
                    .child(labeled("Opacity", self.opacity.clone().into_any_element(), format!("{}%", (opacity * 100.0).round()), cx))
                    .when(matches!(clip.content, ClipContent::Media { .. }), |d| {
                        d.child(segmented(
                            "fit",
                            vec![(Fit::Contain, "Fit".into()), (Fit::Cover, "Fill".into()), (Fit::Stretch, "Stretch".into())],
                            fit,
                            move |f, _, cx| {
                                let v = match f {
                                    Fit::Contain => "contain",
                                    Fit::Cover => "cover",
                                    Fit::Stretch => "stretch",
                                };
                                this.update(cx, |this, cx| this.update_clip(json!({ "fit": v }), "fit", true, cx));
                            },
                            cx,
                        ))
                    })
                    .into_any_element(),
            );
        }

        let speedable = matches!(clip.content, ClipContent::Media { .. }) && asset.as_ref().is_some_and(|a| a.kind != MediaKind::Image);
        body.push(
            section(cx)
                .child(caps("Timing", cx))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(10.))
                        .font_family(MONO)
                        .text_size(px(sz::SM))
                        .text_color(t.text_2)
                        .child(timecode(clip.start, fps))
                        .child("→")
                        .child(timecode(clip.end(), fps))
                        .child(div().text_color(t.text_3).child(short(clip.duration))),
                )
                .child(grid2().when(speedable, |d| d.child(self.speed.clone())).child(self.fade_in.clone()).child(self.fade_out.clone()))
                .into_any_element(),
        );

        if has_sound {
            body.push(
                section(cx)
                    .child(caps("Sound", cx))
                    .child(labeled("Volume", self.volume.clone().into_any_element(), format!("{}%", (clip.volume * 100.0).round()), cx))
                    .into_any_element(),
            );
        }

        if let Some(a) = &asset {
            let path = a.path.clone();
            body.push(
                section(cx)
                    .child(caps("Source", cx))
                    .child(div().text_size(px(sz::SM)).truncate().child(a.name.clone()))
                    .child(facts(a, cx))
                    .child(Button::new("reveal-source", "Reveal in Finder").small().with_icon("folder-search").on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&path))))
                    .into_any_element(),
            );
        }

        out.push(title.into_any_element());
        out.push(scroll("inspector-clip", body).into_any_element());
        out
    }

    fn multi_view(&mut self, clips: Vec<Clip>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.theme().clone();
        self.shown = None;
        let n = clips.len();
        let ids: Vec<Id> = clips.iter().map(|c| c.id).collect();
        let mut body = vec![];
        if n == 2 {
            let (a, b) = (clips[0].clone(), clips[1].clone());
            body.push(
                section(cx)
                    .child(caps("With AI", cx))
                    .child(ai_button("ai-bridge", "waypoints", "Bridge these two shots", cx).on_click(move |_, _, cx| ai::bridge(&a, &b, cx)))
                    .child(
                        div()
                            .text_size(px(sz::SM))
                            .text_color(t.text_2)
                            .line_height(px(sz::SM * 1.5))
                            .child("Generates a transition from the end of the first clip to the start of the second. Works best with models that take first and last frames."),
                    )
                    .into_any_element(),
            );
        }
        let total: f64 = clips.iter().map(|c| c.duration).sum();
        let (start, end) = clips.iter().fold((f64::MAX, 0.0f64), |(s, e), c| (s.min(c.start), e.max(c.end())));
        let dup = ids.clone();
        body.push(
            section(cx)
                .child(kv(vec![("Clips".into(), n.to_string()), ("Total length".into(), short(total)), ("Span".into(), short((end - start).max(0.0)))], cx))
                .child(pair(
                    Button::new("dup", "Duplicate").small().with_icon("copy").on_click(move |_, _, cx| {
                        let ids = dup.clone();
                        cx.store().update(cx, |s, cx| s.run_then("clip.duplicate", json!({ "clipIds": ids }), cx, |s, v, cx| s.set_selection(crate::app::created(&v), cx)));
                    }),
                    Button::new("del", "Delete").small().danger().with_icon("trash").on_click(move |_, _, cx| {
                        let ids = ids.clone();
                        cx.store().update(cx, |s, cx| {
                            let ripple = s.ripple;
                            s.run("clip.delete", json!({ "clipIds": ids, "ripple": ripple }), cx);
                            s.clear_selection(cx);
                        });
                    }),
                ))
                .into_any_element(),
        );
        vec![header(format!("{n} clips"), None, cx).into_any_element(), scroll("inspector-multi", body).into_any_element()]
    }

    fn asset_view(&mut self, a: &Asset, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.theme().clone();
        self.shown = None;
        let poster = div()
            .mx(px(10.))
            .mt(px(10.))
            .rounded(px(sz::R_MD))
            .overflow_hidden()
            .bg(t.bg_sunken)
            .border_1()
            .border_color(t.line)
            .h(px(170.))
            .flex()
            .items_center()
            .justify_center()
            .child(match poster_path(a) {
                Some(p) => img(PathBuf::from(p)).size_full().object_fit(ObjectFit::Contain).into_any_element(),
                None if a.kind == MediaKind::Audio => icon("audio-lines").size(px(28.)).text_color(t.success).into_any_element(),
                None => div().text_size(px(sz::SM)).text_color(t.text_2).child("Preparing a preview…").into_any_element(),
            });
        let mut body = vec![poster.into_any_element()];
        if let AssetOrigin::Generated(g) = &a.origin {
            let (a1, a2) = (a.clone(), a.clone());
            let buttons = pair(
                Button::new("regen-asset", "Regenerate").small().with_icon("refresh-cw").on_click(move |_, _, cx| ai::regenerate_asset(&a1, false, cx)),
                Button::new("variation-asset", "Variation").small().with_icon("shuffle").on_click(move |_, _, cx| ai::regenerate_asset(&a2, true, cx)),
            );
            body.push(self.provenance(g, a, Some(buttons.into_any_element()), cx));
        }
        if a.kind == MediaKind::Image {
            let (a1, a2) = (a.clone(), a.clone());
            body.push(
                section(cx)
                    .child(caps("With AI", cx))
                    .child(ai_button("ai-animate-image", "clapperboard", "Animate with AI", cx).on_click(move |_, _, cx| ai::animate_image(&a1, cx)))
                    .child(ai_button("ai-edit-image", "wand-sparkles", "Edit with AI", cx).on_click(move |_, _, cx| ai::edit_image(&a2, cx)))
                    .into_any_element(),
            );
        }
        let (id, path) = (a.id, a.path.clone());
        body.push(section(cx).child(facts(a, cx)).into_any_element());
        body.push(
            section(cx)
                .child(pair(
                    Button::new("insert", "Insert").small().primary().with_icon("plus").on_click(move |_, _, cx| insert_at_playhead(id, cx)),
                    Button::new("reveal", "Reveal").small().with_icon("folder-search").on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&path))),
                ))
                .child(div().text_size(px(sz::SM)).text_color(t.text_3).child("Double-click in the media panel, or drag onto the timeline, to place it."))
                .into_any_element(),
        );
        vec![header(a.name.clone(), Some(kind_name(a.kind)), cx).into_any_element(), scroll("inspector-asset", body).into_any_element()]
    }

    fn project_view(&mut self, project: &Project, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.shown = None;
        let s = &project.settings;
        self.width.update(cx, |sc, _| sc.set_value(s.width as f64));
        self.height.update(cx, |sc, _| sc.set_value(s.height as f64));
        self.background.update(cx, |f, cx| f.set_value(&s.background, window, cx));
        let (w, h, fps) = (s.width, s.height, s.fps);
        let presets = div().flex().flex_wrap().gap(px(4.)).children(PRESETS.iter().map(|&(label, pw, ph)| {
            Button::new(SharedString::from(format!("preset-{label}")), label)
                .small()
                .selected(w == pw && h == ph)
                .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.run("project.setSettings", json!({ "width": pw, "height": ph }), cx)))
        }));
        let rates = div().flex().flex_wrap().gap(px(4.)).children(FRAME_RATES.iter().map(|&r| {
            Button::new(SharedString::from(format!("fps-{r}")), format!("{r}"))
                .small()
                .selected((fps - r).abs() < 0.01)
                .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.run("project.setSettings", json!({ "fps": r }), cx)))
        }));
        use crate::actions as act;
        let keys: [(&str, &dyn gpui::Action); 8] = [
            ("Play / pause", &act::PlayPause),
            ("Split", &act::Split),
            ("Copy / paste", &act::CopyClips),
            ("Previous / next cut", &act::PrevEdit),
            ("Generate", &act::FocusGenerate),
            ("Command palette", &act::Palette),
            ("Add a title", &act::AddText),
            ("Undo", &act::Undo),
        ];
        let t = cx.theme().clone();
        let body = vec![
            section(cx)
                .child(caps("Canvas", cx))
                .child(presets)
                .child(grid2().child(self.width.clone()).child(self.height.clone()))
                .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(format!("{} · {}×{}", s.aspect_ratio(), w, h)))
                .into_any_element(),
            section(cx).child(caps("Frame rate", cx)).child(rates).into_any_element(),
            section(cx).child(caps("Background", cx)).child(self.background.clone()).into_any_element(),
            section(cx)
                .border_b_0()
                .child(caps("Shortcuts", cx))
                .children(keys.iter().map(|(what, a)| {
                    let k = match *what {
                        "Copy / paste" => format!("{} {}", act::keys_label("M-c"), act::keys_label("M-v")),
                        "Previous / next cut" => "↑ ↓".to_string(),
                        _ => act::hint(*a).unwrap_or_default().to_string(),
                    };
                    div().flex().items_center().justify_between().text_size(px(sz::SM)).text_color(t.text_2).child(*what).child(kbd(k, cx))
                }))
                .child(
                    Button::new("all-shortcuts", "All shortcuts")
                        .small()
                        .ghost()
                        .with_icon("keyboard")
                        .tooltip(act::tip("Keyboard shortcuts", &act::ShowShortcuts))
                        .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(crate::store::Dialog::Shortcuts, cx))),
                )
                .into_any_element(),
        ];
        vec![header("Project", None, cx).into_any_element(), scroll("inspector-project", body).into_any_element()]
    }

    /// How a media item was made: model, prompt, provider, seed, inputs, time, cost.
    fn provenance(&self, g: &Generation, a: &Asset, actions: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let store = self.store.read(cx);
        let provider = store.providers.iter().find(|p| p.info.id == g.provider).map(|p| p.info.name.clone()).unwrap_or_else(|| g.provider.clone());
        let inputs: Vec<(Id, String)> = g.inputs.iter().map(|id| (*id, store.asset(*id).map(|a| a.name.clone()).unwrap_or_else(|| "a removed file".into()))).collect();
        let mut rows = vec![("Provider".to_string(), provider), ("Model".into(), g.model_name.clone())];
        if let Some(seed) = g.seed {
            rows.push(("Seed".into(), seed.to_string()));
        }
        rows.push(("Took".into(), format!("{:.0}s", g.elapsed_ms as f64 / 1000.0)));
        if let Some(c) = g.cost_usd {
            rows.push(("Cost".into(), format!("${c:.3}")));
        }
        rows.push(("Made".into(), ago(a.created_at)));
        div()
            .mx(px(10.))
            .mt(px(10.))
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(9.))
            .rounded(px(sz::R_MD))
            .bg(t.accent_soft)
            .border_1()
            .border_color(t.accent_ring)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(sz::SM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.accent_text)
                    .child(icon("sparkles").text_color(t.accent_text))
                    .child(g.model_name.clone()),
            )
            .child(div().text_size(px(sz::MD)).line_height(px(sz::MD * 1.35)).text_color(t.text).child(g.prompt.clone()))
            .when_some(g.negative_prompt.clone().filter(|n| !n.is_empty()), |d, n| {
                d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child(format!("Without: {n}")))
            })
            .child(kv(rows, cx))
            .when(!inputs.is_empty(), |d| {
                d.child(caps("Inputs", cx)).child(div().flex().flex_wrap().gap(px(4.)).children(inputs.into_iter().map(|(id, name)| {
                    div()
                        .id(ElementId::from(id))
                        .px(px(7.))
                        .py(px(2.))
                        .rounded_full()
                        .bg(t.hover)
                        .border_1()
                        .border_color(t.line_strong)
                        .text_size(px(sz::XS))
                        .text_color(t.text)
                        .max_w(px(200.))
                        .truncate()
                        .cursor_pointer()
                        .hover(|s| s.border_color(t.accent_ring))
                        .tooltip(|_, cx| tooltip("Show this file".into(), cx))
                        .child(name)
                        .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.select_asset(Some(id), cx)))
                })))
            })
            .children(actions)
            .into_any_element()
    }
}

impl Render for Inspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (project, selection, selected_asset) = {
            let s = self.store.read(cx);
            (s.project.clone(), s.selection.clone(), s.selected_asset)
        };
        let children: Vec<AnyElement> = match project {
            None => vec![],
            Some(p) => {
                let p: Arc<Project> = p;
                let clips: Vec<Clip> = selection.iter().filter_map(|id| p.clip(*id).cloned()).collect();
                if clips.len() == 1 {
                    self.clip_view(&clips[0], &p, window, cx)
                } else if clips.len() > 1 {
                    self.multi_view(clips, cx)
                } else if let Some(a) = selected_asset.and_then(|id| p.asset(id).cloned()) {
                    self.asset_view(&a, cx)
                } else {
                    self.project_view(&p, window, cx)
                }
            }
        };
        div().size_full().flex().flex_col().glass(t.glass1).border_0().border_l_1().border_color(t.line).children(children)
    }
}

// ---- pieces ------------------------------------------------------------------

fn kind_name(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Video => "video",
        MediaKind::Image => "image",
        MediaKind::Audio => "audio",
    }
}

/// The picture to show for a media item: its thumbnail, or the image itself.
pub fn poster_path(a: &Asset) -> Option<String> {
    a.thumbnail.clone().or_else(|| (a.kind == MediaKind::Image).then(|| a.path.clone()))
}

/// `clip.insertMedia` at the playhead, selecting the new clip.
pub fn insert_at_playhead(asset: Id, cx: &mut App) {
    cx.store().update(cx, |s, cx| {
        let t = s.playback.read(cx).playhead;
        s.run_then("clip.insertMedia", json!({ "assetId": asset, "start": t }), cx, |s, v, cx| s.set_selection(crate::app::created(&v), cx));
    });
}

fn section(cx: &App) -> Div {
    div().flex().flex_col().gap(px(9.)).px(px(14.)).py(px(14.)).border_b_1().border_color(cx.theme().line)
}

/// Two buttons sharing a row equally.
fn pair(a: Button, b: Button) -> Div {
    div().flex().gap(px(6.)).child(div().flex_1().min_w_0().child(a.full_width())).child(div().flex_1().min_w_0().child(b.full_width()))
}

fn grid2() -> Div {
    div().grid().grid_cols(2).gap(px(6.))
}

fn scroll(id: &'static str, body: Vec<AnyElement>) -> impl IntoElement {
    div().id(id).flex_1().min_h_0().overflow_y_scroll().pb(px(20.)).children(body)
}

fn pill(text: &'static str, cx: &App) -> Div {
    let t = cx.theme();
    div().flex_none().px(px(7.)).py(px(2.)).rounded_full().bg(t.hover).border_1().border_color(t.line).text_size(px(10.5)).text_color(t.text_2).child(text)
}

fn header(title: impl Into<SharedString>, kind: Option<&'static str>, cx: &App) -> Div {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(14.))
        .h(px(52.))
        .flex_none()
        .border_b_1()
        .border_color(t.line)
        .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).text_size(px(sz::BASE)).child(title.into()))
        .children(kind.map(|k| pill(k, cx)))
}

/// Label, control, value.
fn labeled(name: &'static str, control: AnyElement, value: String, cx: &App) -> Div {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().w(px(58.)).flex_none().text_size(px(sz::SM)).text_color(t.text_2).child(name))
        .child(div().flex_1().flex().child(control))
        .child(div().w(px(40.)).flex_none().flex().justify_end().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(value))
}

/// Two columns: quiet labels, values in mono on the right.
fn kv(rows: Vec<(String, String)>, cx: &App) -> Div {
    let t = cx.theme();
    div().flex().flex_col().gap(px(6.)).children(rows.into_iter().map(|(k, v)| {
        div()
            .flex()
            .justify_between()
            .gap(px(12.))
            .text_size(px(sz::SM))
            .child(div().flex_none().text_color(t.text_2).child(k))
            .child(div().min_w_0().truncate().font_family(MONO).text_color(t.text).child(v))
    }))
}

/// A media item's file facts.
fn facts(a: &Asset, cx: &App) -> Div {
    let m = &a.meta;
    let mut rows = vec![];
    if let (Some(w), Some(h)) = (m.width, m.height) {
        rows.push(("Size".into(), format!("{w}×{h}")));
    }
    if let Some(d) = m.duration.filter(|_| a.kind != MediaKind::Image) {
        rows.push(("Length".into(), timecode(d, m.fps.unwrap_or(30.0))));
    }
    if let Some(f) = m.fps {
        rows.push(("Frame rate".into(), format!("{f:.2}")));
    }
    if let Some(c) = &m.video_codec {
        rows.push(("Video".into(), c.clone()));
    }
    if let Some(c) = &m.audio_codec {
        rows.push(("Audio".into(), c.clone()));
    }
    rows.push(("File".into(), bytes(m.size_bytes)));
    kv(rows, cx)
}

/// A full-width row button for an AI action (icon in the accent).
fn ai_button(id: &'static str, ic: &'static str, label: &'static str, cx: &App) -> gpui::Stateful<Div> {
    let t = cx.theme();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(9.))
        .h(px(32.))
        .px(px(10.))
        .rounded(px(sz::R_SM))
        .bg(t.hover)
        .border_1()
        .border_color(t.line)
        .text_size(px(sz::BASE))
        .text_color(t.text)
        .cursor_pointer()
        .hover(|s| s.bg(t.pressed).border_color(t.accent_ring))
        .active(|s| s.opacity(0.85))
        .child(icon(ic).text_color(t.accent_text))
        .child(label)
}

/// A segmented control of icons (text alignment).
fn icon_segmented<T: Clone + PartialEq + 'static>(
    id: &'static str,
    options: Vec<(T, &'static str, &'static str)>,
    value: T,
    on_change: impl Fn(&T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> gpui::Stateful<Div> {
    let t = cx.theme().clone();
    let on_change = std::rc::Rc::new(on_change);
    div()
        .id(id)
        .flex()
        .w(px(132.))
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(sz::R_SM + 2.))
        .bg(t.bg_sunken.opacity(0.6))
        .border_1()
        .border_color(t.line)
        .children(options.into_iter().enumerate().map(move |(i, (v, ic, tip))| {
            let selected = v == value;
            let on_change = on_change.clone();
            div()
                .id((id, i))
                .flex_1()
                .flex()
                .justify_center()
                .py(px(4.))
                .rounded(px(sz::R_SM))
                .cursor_pointer()
                .when(selected, |d| d.bg(t.accent_soft).text_color(t.accent_text))
                .when(!selected, |d| d.text_color(t.text_2).hover(|s| s.bg(t.hover)))
                .child(icon(ic).text_color(if selected { t.accent_text } else { t.text_2 }))
                .tooltip(move |_, cx| tooltip(tip.into(), cx))
                .on_click(move |_, w, cx| on_change(&v, w, cx))
        }))
}
