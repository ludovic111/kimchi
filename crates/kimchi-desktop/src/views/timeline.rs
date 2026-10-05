//! The timeline: a toolbar (transport, clock, edit tools, zoom), the tracks
//! ([`body::TimelineBody`], a cached view) and the playhead drawn over them.
//!
//! Only this shell observes the playback clock, so playing re-renders the
//! toolbar's clock and the playhead line, never the clips.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, BoxShadow, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, StyleRefinement, Subscription, Window,
    canvas, div, point, prelude::*, px,
};
use serde_json::json;

use crate::actions::{AddMarker, AddText, Delete, Duplicate, Split, ToggleSnap, ZoomFit, ZoomIn, ZoomOut, tip};
use crate::playback::Playback;
use crate::store::{ComposeRequest, ComposeTarget, MAX_PPS, MIN_PPS, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, GlassExt, drag, smpte};

mod body;
mod clip;
pub mod dnd;
pub(crate) mod geom;
mod menus;
#[cfg(test)]
mod sound_tests;
#[cfg(test)]
mod tests;
mod waveform;

use body::{TimelineBody, pentagon};
use geom::{HEADER_W, RULER_H, TOOLBAR_H};

pub struct Timeline {
    store: Entity<Store>,
    playback: Entity<Playback>,
    body: Entity<TimelineBody>,
    /// The zoom slider's track, as last drawn, and whether it is being dragged.
    slider: Rc<Cell<Bounds<Pixels>>>,
    sliding: bool,
    width: f32,
    /// The mixer (in the tracks' place or beside them), the effect browser and an effect's panel.
    pub mixer: Entity<crate::views::mixer::MixerView>,
    pub browser: Entity<crate::views::mixer::browser::EffectBrowser>,
    pub effect: Entity<crate::views::mixer::effect_panel::EffectPanel>,
    _subs: Vec<Subscription>,
}

impl Timeline {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let body = cx.new(|cx| TimelineBody::new(window, cx));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&playback, |_, _, cx| {
                // Hear the sound where the playhead lands, when it moves while nothing plays.
                crate::views::mixer::scrub::on_playhead(cx);
                cx.notify();
            }),
        ];
        let mixer = cx.new(|cx| crate::views::mixer::MixerView::new(window, cx));
        let browser = cx.new(|cx| crate::views::mixer::browser::EffectBrowser::new(window, cx));
        let effect = cx.new(|cx| crate::views::mixer::effect_panel::EffectPanel::new(window, cx));
        Self { store, playback, body, slider: Rc::new(Cell::new(Bounds::default())), sliding: false, width: 0., mixer, browser, effect, _subs: subs }
    }

    /// Width of the track area (the lanes, without the headers), as last drawn: for zoom to fit.
    /// Zooms to show the whole project from its start (the Fit button and the shortcut alike).
    pub fn fit(this: &Entity<Self>, cx: &mut gpui::App) {
        let body = this.read(cx).body.clone();
        body.update(cx, |b, cx| b.fit(cx));
    }

    // ---- zoom slider ---------------------------------------------------------

    fn slide_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let b = self.slider.get();
        let f = (f32::from(x - b.origin.x) / f32::from(b.size.width).max(1.)).clamp(0., 1.) as f64;
        let pps = (MIN_PPS.ln() + f * (MAX_PPS.ln() - MIN_PPS.ln())).exp();
        self.body.update(cx, |b, cx| b.zoom_to(pps, None, cx));
    }

    fn slide_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.sliding = true;
        self.slide_to(e.position.x, cx);
        cx.notify();
    }

    fn slide_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.slide_to(e.position.x, cx);
    }

    fn slide_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.sliding = false;
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f64, cx: &mut Context<Self>) {
        let pps = self.store.read(cx).pps;
        self.body.update(cx, |b, cx| b.zoom_to(pps * factor, None, cx));
    }

    // ---- drawing -------------------------------------------------------------

    fn toolbar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let pb = self.playback.read(cx);
        let (playing, looping, playhead) = (pb.playing, pb.looping, pb.playhead);
        let (fps, duration, pps) = (s.fps(), s.duration(), s.pps);
        let (has_sel, snapping, ripple) = (!s.selection.is_empty(), s.snapping, s.ripple);
        let sep = || div().w(px(1.)).h(px(16.)).mx(px(6.)).bg(t.line_strong);
        // Narrow timelines (agent panel open, small window) drop the least needed parts first.
        let width = self.width;
        let (compact, narrow, tiny) = (width < 1100., width < 960., width < 820.);
        // The transport lives under the preview; the timeline keeps the time, where the eye is.
        let shuttle = self.playback.read(cx).shuttle;
        let clock = div()
            .flex()
            .items_baseline()
            .gap(px(6.))
            .font_family(MONO)
            .child(div().text_size(px(sz::BASE)).text_color(if playing || shuttle != 0. { t.accent_text } else { t.text }).child(smpte(playhead, fps)))
            .when(!narrow, |d| d.child(div().text_size(px(sz::XS)).text_color(t.text_3).child(format!("/ {}", smpte(duration, fps)))))
            .when(shuttle != 0., |d| d.child(div().text_size(px(sz::XS)).text_color(t.accent_text).child(rate_label(shuttle))))
            .when(looping, |d| d.child(crate::ui::icon("repeat").size(px(11.)).text_color(t.text_3)));
        let tools = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(Button::icon("split", "scissors", tip("Split at the playhead", &Split)).small().on_click(|_, w, cx| w.dispatch_action(Box::new(Split), cx)))
            .child(Button::icon("delete", "trash", tip("Delete", &Delete)).small().disabled(!has_sel).on_click(|_, w, cx| w.dispatch_action(Box::new(Delete), cx)))
            .child(Button::icon("duplicate", "copy", tip("Duplicate", &Duplicate)).small().disabled(!has_sel).on_click(|_, w, cx| w.dispatch_action(Box::new(Duplicate), cx)))
            .child(sep())
            .child(Button::icon("snap", "magnet", tip(if snapping { "Snapping: on" } else { "Snapping: off" }, &ToggleSnap)).small().selected(snapping).on_click(|_, _, cx| {
                cx.store().update(cx, |s, cx| s.set_snapping(!s.snapping, cx))
            }))
            .child(Button::icon("ripple", "wrap-text", if ripple { "Ripple delete: on (deleting closes the gap)" } else { "Ripple delete: off" }).small().selected(ripple).on_click(|_, _, cx| {
                cx.store().update(cx, |s, cx| s.set_ripple(!s.ripple, cx))
            }))
            .child(sep())
            .child(crate::views::mixer::toolbar(narrow, cx))
            .child(sep())
            .child(Button::icon("marker", "map-pin", tip("Add a marker", &AddMarker)).small().on_click(|_, w, cx| w.dispatch_action(Box::new(AddMarker), cx)))
            .child(Button::icon("text", "type", tip("Add a title", &AddText)).small().on_click(|_, w, cx| w.dispatch_action(Box::new(AddText), cx)))
            // Also in the "+" menu above the track headers.
            .when(!tiny, |d| {
                d.child(Button::icon("video-track", "film", "Add video track").small().on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("track.add", json!({ "kind": "video" }), cx))))
                    .child(
                        Button::icon("audio-track", "audio-lines", "Add audio track")
                            .small()
                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("track.add", json!({ "kind": "audio" }), cx))),
                    )
            });

        let frac = ((pps.ln() - MIN_PPS.ln()) / (MAX_PPS.ln() - MIN_PPS.ln())).clamp(0., 1.) as f32;
        let slider = {
            let cell = self.slider.clone();
            div()
                .id("zoom-slider")
                .w(px(100.))
                .h(px(20.))
                .flex_none()
                .relative()
                .cursor_pointer()
                .tooltip(|_, cx| crate::ui::tooltip(if cfg!(target_os = "macos") { "Zoom (pinch, or ⌘ + scroll)" } else { "Zoom (pinch, or Ctrl + scroll)" }.into(), cx))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::slide_down))
                .child(canvas(move |b, _, _| cell.set(b), |_, _, _, _| {}).absolute().inset_0())
                .child(div().absolute().left_0().right_0().top(px(8.5)).h(px(3.)).rounded_full().bg(t.line_strong))
                .child(div().absolute().left_0().top(px(8.5)).h(px(3.)).w(px(frac * 100.)).rounded_full().bg(t.text_3))
                .child(div().absolute().top(px(4.)).left(px(frac * 88.)).size(px(12.)).rounded_full().bg(t.text_2).border_2().border_color(t.bg_raised))
        };
        let this = cx.entity();
        let (z1, z2) = (this.clone(), this);
        let body = self.body.clone();
        let zoom = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                if compact { Button::icon("gen-here", "sparkles", "Generate at playhead") } else { Button::new("gen-here", "Generate at playhead").with_icon("sparkles") }
                    .small()
                    .color(t.accent_text)
                    .tooltip("Make a shot that lands at the playhead")
                    .on_click(|_, _, cx| {
                        let t = cx.store().read(cx).playback.read(cx).playhead;
                        let target = ComposeTarget { track_id: None, start: t, duration: 5., label: "at the playhead".into() };
                        cx.store().update(cx, |s, cx| s.compose(ComposeRequest { video: true, target: Some(target), ..Default::default() }, cx));
                    }),
            )
            .child(sep())
            .child(Button::icon("zoom-out", "zoom-out", tip("Zoom out", &ZoomOut)).small().on_click(move |_, _, cx| z1.update(cx, |this, cx| this.zoom_by(1. / 1.3, cx))))
            .when(!narrow, |d| d.child(slider))
            .child(Button::icon("zoom-in", "zoom-in", tip("Zoom in", &ZoomIn)).small().on_click(move |_, _, cx| z2.update(cx, |this, cx| this.zoom_by(1.3, cx))))
            .child(
                Button::new("fit", "Fit")
                    .small()
                    .ghost()
                    .tooltip(tip("Zoom to fit", &ZoomFit))
                    .on_click(move |_, _, cx| body.update(cx, |b, cx| b.fit(cx))),
            );
        div()
            .id("timeline-toolbar")
            .h(px(TOOLBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .px(px(10.))
            .glass(t.glass1)
            .border_0()
            .border_b_1()
            .border_color(t.line)
            .overflow_hidden()
            .child(clock)
            .child(tools)
            .child(zoom)
            .into_any_element()
    }

    /// The playhead over the tracks: the flag in the ruler and the line through the lanes.
    fn playhead(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let b = self.body.read(cx);
        let x = (self.playback.read(cx).playhead * self.store.read(cx).pps - b.scroll_x) as f32;
        let visible = x >= -8. && x <= f32::from(b.lanes.get().size.width) + 8.;
        div()
            .absolute()
            .left(px(HEADER_W + 1.))
            .top_0()
            .right_0()
            .bottom_0()
            .overflow_hidden()
            .when(visible, |d| {
                d.child(div().absolute().left(px(x - 6.5)).top(px(RULER_H - 16.)).child(pentagon(13., 16., t.accent))).child(
                    div().absolute().left(px(x - 0.75)).top(px(RULER_H - 2.)).bottom_0().w(px(1.5)).bg(t.accent).shadow(vec![BoxShadow {
                        color: t.accent.opacity(0.45),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(10.),
                        spread_radius: px(0.),
                        inset: false,
                    }]),
                )
            })
            .into_any_element()
    }
}

/// Shuttle speed as shown next to the clock: "2×", "◀ 4×".
pub fn rate_label(rate: f64) -> String {
    let n = format!("{}", rate.abs());
    if rate < 0. { format!("◀ {n}×") } else { format!("{n}×") }
}

impl Render for Timeline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.toolbar(cx);
        let playhead = self.playhead(cx);
        // The tracks' meters beside their headers (redrawn while playing, unlike the tracks).
        let meters = {
            let b = self.body.read(cx);
            let (scroll_y, view_h) = (b.scroll_y, f32::from(b.lanes.get().size.height));
            self.store.read(cx).project.clone().map(|p| div().absolute().top(px(RULER_H)).left_0().bottom_0().w(px(HEADER_W)).child(body::header_meters(&p, scroll_y, view_h, cx)))
        };
        let measured = self.width;
        let this = cx.entity().downgrade();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(canvas(move |b, _, cx| {
                let width = f32::from(b.size.width);
                if (width - measured).abs() > 0.5 {
                    cx.defer(move |cx| { let _ = this.update(cx, |t, cx| { t.width = width; cx.notify(); }); });
                }
            }, |_, _, _, _| {}).absolute().inset_0())
            .child(toolbar)
            .child(div().flex_1().min_h_0().w_full().child(crate::views::mixer::area(
                div().relative().size_full().child(self.body.clone().cached(StyleRefinement::default().size_full())).child(playhead).children(meters).into_any_element(),
                &self.mixer,
                cx,
            )))
            .child(self.browser.clone())
            .child(self.effect.clone())
            .when(self.sliding, |d| d.child(drag::track(cx.entity(), Self::slide_move, Self::slide_up)))
    }
}
