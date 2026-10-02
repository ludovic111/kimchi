//! "Remove this file?" — the one question the media panel asks before a change, since removing
//! media also takes every clip that uses it off the timeline.

use gpui::{AnyElement, App, Entity, FocusHandle, FontWeight, KeyDownEvent, MouseButton, Window, anchored, deferred, div, point, prelude::*, px};

use crate::actions::Deselect;
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{Button, GlassExt, icon};

/// What the dialog asks about.
#[derive(Clone)]
pub struct Removal {
    pub asset: kimchi_core::Id,
    pub name: String,
    /// Clips on the timeline that use it.
    pub clips: usize,
}

/// The dialog over the whole window (focus `focus` when opening it, for Enter and Escape);
/// `on_answer(true)` removes, `false` keeps.
pub fn dialog<V: 'static>(
    removal: &Removal,
    focus: &FocusHandle,
    entity: Entity<V>,
    on_answer: fn(&mut V, bool, &mut gpui::Context<V>),
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = cx.theme().clone();
    let size = window.viewport_size();
    let detail = match removal.clips {
        0 => "It isn't on the timeline. You can undo this.".to_string(),
        1 => "The clip that uses it leaves the timeline too. You can undo this.".to_string(),
        n => format!("The {n} clips that use it leave the timeline too. You can undo this."),
    };
    let (e1, e2, e3, e4, e5) = (entity.clone(), entity.clone(), entity.clone(), entity.clone(), entity);
    deferred(
        anchored().position(point(px(0.), px(0.))).child(
            div()
                .id("remove-media-scrim")
                .occlude()
                .w(size.width)
                .h(size.height)
                .bg(t.scrim)
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| e1.update(cx, |v, cx| on_answer(v, false, cx)))
                .child(
                    div()
                        .id("remove-media")
                        .track_focus(focus)
                        .key_context("ConfirmDialog")
                        .on_action(move |_: &Deselect, _, cx| e2.update(cx, |v, cx| on_answer(v, false, cx)))
                        .on_key_down(move |e: &KeyDownEvent, _, cx| {
                            if e.keystroke.key == "enter" {
                                cx.stop_propagation();
                                e3.update(cx, |v, cx| on_answer(v, true, cx));
                            }
                        })
                        .occlude()
                        .w(px(400.))
                        .p(px(20.))
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .rounded(px(sz::R_LG))
                        .glass(t.glass3)
                        .shadow(t.glass_shadow())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().flex_none().size(px(32.)).rounded(px(sz::R_SM)).bg(t.danger.opacity(0.14)).flex().items_center().justify_center().child(icon("trash").text_color(t.danger)))
                                .child(div().flex_1().min_w_0().text_size(px(sz::MD)).font_weight(FontWeight::SEMIBOLD).child(format!("Remove “{}”?", removal.name))),
                        )
                        .child(div().text_size(px(sz::BASE)).text_color(t.text_2).line_height(px(sz::BASE * 1.5)).child(detail))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .child(Button::new("remove-cancel", "Keep it").on_click(move |_, _, cx| e4.update(cx, |v, cx| on_answer(v, false, cx))))
                                .child(Button::new("remove-confirm", "Remove").danger().with_icon("trash").on_click(move |_, _, cx| e5.update(cx, |v, cx| on_answer(v, true, cx)))),
                        ),
                ),
        ),
    )
    .with_priority(2)
    .into_any_element()
}
