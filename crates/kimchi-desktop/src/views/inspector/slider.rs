//! A horizontal slider (opacity, volume, effects): click or drag anywhere on the track; the arrow
//! keys nudge it when it has focus. A slider with a neutral value (`neutral`) fills from it
//! (signed effects fill from the middle) and goes back to it on a double-click.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, Context, EventEmitter, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Window, canvas, div,
    prelude::*, px, relative,
};

use crate::actions::{StepBack, StepBackSecond, StepForward, StepForwardSecond};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::drag;
use crate::ui::scrub::ScrubChange;

pub struct Slider {
    pub min: f64,
    pub max: f64,
    /// Where the fill starts and what a double-click resets to.
    neutral: Option<f64>,
    value: f64,
    dragging: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
}

impl EventEmitter<ScrubChange> for Slider {}

impl Slider {
    pub fn new(min: f64, max: f64, cx: &mut Context<Self>) -> Self {
        Self { min, max, neutral: None, value: min, dragging: false, bounds: Rc::default(), focus: cx.focus_handle() }
    }

    pub fn neutral(mut self, v: f64) -> Self {
        self.neutral = Some(v);
        self
    }

    /// Shows `v` (from the project) unless a drag is in progress.
    pub fn set_value(&mut self, v: f64) {
        if !self.dragging {
            self.value = v;
        }
    }

    fn at(&self, x: Pixels) -> f64 {
        let b = self.bounds.get();
        let w = f32::from(b.size.width).max(1.0);
        let f = (f32::from(x - b.origin.x) / w).clamp(0.0, 1.0) as f64;
        // Lands on whole percents.
        ((self.min + f * (self.max - self.min)) * 100.0).round() / 100.0
    }

    fn set(&mut self, v: f64, final_: bool, cx: &mut Context<Self>) {
        let v = v.clamp(self.min, self.max);
        if v != self.value || final_ {
            self.value = v;
            cx.emit(ScrubChange { value: v, final_ });
        }
        cx.notify();
    }

    fn down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let (Some(n), 2) = (self.neutral, e.click_count) {
            self.set(n, true, cx);
            return;
        }
        self.dragging = true;
        let v = self.at(e.position.x);
        self.set(v, false, cx);
    }

    fn moved(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.dragging {
            let v = self.at(e.position.x);
            self.set(v, false, cx);
        }
    }

    fn up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.dragging {
            self.dragging = false;
            let v = self.at(e.position.x);
            self.set(v, true, cx);
        }
    }

    /// Moves by `steps` hundredths of the range.
    fn nudge(&mut self, steps: f64, cx: &mut Context<Self>) {
        let v = self.value + (self.max - self.min) * steps / 100.0;
        self.set((v * 100.0).round() / 100.0, true, cx);
    }

    fn key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let big = if e.keystroke.modifiers.shift { 10.0 } else { 1.0 };
        match e.keystroke.key.as_str() {
            "down" => self.nudge(-big, cx),
            "up" => self.nudge(big, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl gpui::Render for Slider {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let frac = |v: f64| if self.max > self.min { ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let f = frac(self.value);
        let from = frac(self.neutral.unwrap_or(self.min));
        let (fill0, fill1) = (from.min(f), from.max(f));
        let focused = self.focus.is_focused(window);
        let bounds = self.bounds.clone();
        div()
            .id("slider")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            // Left and right are the playhead's keys; on a focused slider they move the slider.
            .on_action(cx.listener(|this, _: &StepBack, _, cx| this.nudge(-1.0, cx)))
            .on_action(cx.listener(|this, _: &StepForward, _, cx| this.nudge(1.0, cx)))
            .on_action(cx.listener(|this, _: &StepBackSecond, _, cx| this.nudge(-10.0, cx)))
            .on_action(cx.listener(|this, _: &StepForwardSecond, _, cx| this.nudge(10.0, cx)))
            .relative()
            .flex_1()
            .h(px(20.))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::down))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(t.line_strong)
                    .child(div().absolute().left(relative(fill0)).top_0().h_full().w(relative(fill1 - fill0)).rounded_full().bg(t.accent))
                    .child(
                        canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().inset_0(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(-5.))
                            .left(relative(f))
                            .ml(px(-7.))
                            .size(px(14.))
                            .rounded_full()
                            .bg(gpui::white())
                            .border_1()
                            .border_color(if focused || self.dragging { t.accent } else { t.line_strong })
                            .shadow_sm(),
                    ),
            )
            .when(self.dragging, |d| d.child(drag::track(cx.entity(), Self::moved, Self::up)))
            .rounded(px(sz::R_XS))
    }
}
