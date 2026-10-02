//! A number you can type into or drag sideways to scrub, Figma-style.
//! Shift scrubs 10× faster, Alt 10× finer.

use gpui::{Context, Entity, EventEmitter, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Subscription, Window, div, prelude::*, px};

use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::drag;
use crate::ui::input::{InputEvent, TextInput};

/// `final_` is false while dragging (use a coalesce key), true when committed.
#[derive(Clone, Copy, Debug)]
pub struct ScrubChange {
    pub value: f64,
    pub final_: bool,
}

pub struct Scrub {
    pub label: SharedString,
    pub unit: SharedString,
    pub step: f64,
    pub min: f64,
    pub max: f64,
    pub decimals: usize,
    value: f64,
    drag: Option<(Pixels, f64, bool)>,
    editor: Option<(Entity<TextInput>, Subscription)>,
}

impl EventEmitter<ScrubChange> for Scrub {}

impl Scrub {
    pub fn new(label: impl Into<SharedString>, step: f64, decimals: usize) -> Self {
        Self { label: label.into(), unit: "".into(), step, min: f64::NEG_INFINITY, max: f64::INFINITY, decimals, value: 0.0, drag: None, editor: None }
    }

    pub fn unit(mut self, u: impl Into<SharedString>) -> Self {
        self.unit = u.into();
        self
    }

    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Shows `v` (from the project) unless a drag is in progress.
    pub fn set_value(&mut self, v: f64) {
        if self.drag.is_none() {
            self.value = v;
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    fn fmt(&self, v: f64) -> String {
        if self.decimals == 0 { format!("{}", v.round()) } else { format!("{v:.*}", self.decimals) }
    }

    fn clamp(&self, v: f64) -> f64 {
        v.clamp(self.min, self.max)
    }

    fn down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            return;
        }
        self.drag = Some((e.position.x, self.value, false));
        cx.notify();
    }

    fn moved(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((x0, start, moved)) = self.drag else { return };
        let dx = f32::from(e.position.x - x0) as f64;
        if !moved && dx.abs() <= 2.0 {
            return;
        }
        let mult = if e.modifiers.shift { 10.0 } else if e.modifiers.alt { 0.1 } else { 1.0 };
        let v = self.clamp(start + (dx / 2.0).round() * self.step * mult);
        self.drag = Some((x0, start, true));
        if v != self.value {
            self.value = v;
            cx.emit(ScrubChange { value: v, final_: false });
        }
        cx.notify();
    }

    fn up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, _, moved)) = self.drag.take() else { return };
        if moved {
            cx.emit(ScrubChange { value: self.value, final_: true });
        } else {
            self.edit(window, cx);
        }
        cx.notify();
    }

    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.fmt(self.value);
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx);
            i.mono = true;
            i.set_text(text, cx);
            i.select_all_text(cx);
            i
        });
        crate::ui::input::focus(&input, window, cx);
        let sub = cx.subscribe(&input, |this, input, e: &InputEvent, cx| match e {
            InputEvent::Submit(_) | InputEvent::Blur => {
                let text = input.read(cx).text().to_string();
                if let Ok(v) = text.trim().trim_end_matches(this.unit.as_ref()).trim().parse::<f64>() {
                    this.value = this.clamp(v);
                    cx.emit(ScrubChange { value: this.value, final_: true });
                }
                this.editor = None;
                cx.notify();
            }
            InputEvent::Cancel => {
                this.editor = None;
                cx.notify();
            }
            InputEvent::Changed(_) => {}
        });
        self.editor = Some((input, sub));
    }
}

impl Render for Scrub {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let dragging = self.drag.is_some();
        div()
            .flex()
            .items_center()
            .h(px(26.))
            .rounded(px(sz::R_SM))
            .bg(t.bg_sunken.opacity(0.6))
            .border_1()
            .border_color(if dragging { t.accent_ring } else { t.line })
            .overflow_hidden()
            .child(
                div()
                    .id("scrub-label")
                    .flex_none()
                    .px(px(7.))
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(px(sz::XS))
                    .text_color(t.text_2)
                    .cursor_ew_resize()
                    .hover(|s| s.text_color(t.accent_text))
                    .child(self.label.clone())
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::down)),
            )
            .child(match &self.editor {
                Some((input, _)) => div().flex_1().child(input.clone()).into_any_element(),
                None => div()
                    .id("scrub-value")
                    .flex_1()
                    .flex()
                    .justify_end()
                    .pr(px(7.))
                    .font_family(MONO)
                    .text_size(px(sz::SM))
                    .text_color(t.text)
                    .cursor_ew_resize()
                    .child(format!("{}{}", self.fmt(self.value), self.unit))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::down))
                    .into_any_element(),
            })
            .when(dragging, |d| d.child(drag::track(cx.entity(), Self::moved, Self::up)))
    }
}
