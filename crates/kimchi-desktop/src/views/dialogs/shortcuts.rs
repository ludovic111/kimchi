//! The keyboard shortcuts sheet (`?`): every shortcut from [`crate::actions::SHORTCUTS`] by
//! group, in this platform's notation, and what the mouse does on the timeline and canvas.

use gpui::{AnyElement, App, FontWeight, div, prelude::*, px};

use crate::actions::{SHORTCUTS, Shortcut, keys_label};
use crate::store::StoreExt;
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{Button, caps, kbd};

/// What the mouse does, for the last column.
const MOUSE: &[(&str, &str)] = &[
    ("Select several clips", "Drag on empty track space"),
    ("Add to the selection", "Shift-click"),
    ("Copy a clip", "Hold ⌥ while dragging it"),
    ("Trim", "Drag a clip's edge"),
    ("Fade in / out", "Drag the round knob on top"),
    ("Move the playhead", "Click or drag the ruler"),
    ("Zoom the timeline", "Pinch, or ⌘ + scroll"),
    ("Scroll sideways", "Shift + scroll"),
    ("Rename a track", "Double-click its name"),
    ("Reset a panel's size", "Double-click its divider"),
    ("More actions", "Right-click anything"),
];

/// A shortcut's keys: the main one, an alias if there is one, and the pair for "previous / next" rows.
fn keys_of(i: usize) -> Vec<String> {
    let s = &SHORTCUTS[i];
    let mut keys = vec![keys_label(s.keys[0]).to_string()];
    match SHORTCUTS.get(i + 1) {
        Some(next) if s.label.contains(" / ") && next.group.is_empty() => keys.push(keys_label(next.keys[0]).to_string()),
        _ => {
            if let Some(alias) = s.keys.get(1).filter(|k| !matches!(**k, "+" | "delete" | "shift-delete")) {
                keys.push(keys_label(alias).to_string());
            }
        }
    }
    keys
}

fn row(label: &str, keys: Vec<String>, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .py(px(3.))
        .text_size(px(sz::SM))
        .child(div().text_color(t.text_2).child(label.to_string()))
        .child(div().flex().flex_none().gap(px(4.)).children(keys.into_iter().map(|k| kbd(k, cx))))
        .into_any_element()
}

fn mouse_label(how: &str) -> String {
    if cfg!(target_os = "macos") { how.to_string() } else { how.replace("⌘", "Ctrl").replace("⌥", "Alt") }
}

pub fn sheet(cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let group = |name: &'static str, cx: &App| {
        div().flex().flex_col().gap(px(2.)).mb(px(14.)).child(div().mb(px(4.)).child(caps(name, cx))).children(
            SHORTCUTS.iter().enumerate().filter(|(_, s): &(usize, &Shortcut)| s.group == name).map(|(i, s)| row(s.label, keys_of(i), cx)).collect::<Vec<_>>(),
        )
    };
    let column = |names: &[&'static str], cx: &App| div().flex_1().min_w_0().flex().flex_col().children(names.iter().map(|n| group(n, cx)).collect::<Vec<_>>());
    let mouse = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(div().mb(px(4.)).child(caps("Mouse", cx)))
        .children(MOUSE.iter().map(|(what, how)| row(what, vec![mouse_label(how)], cx)).collect::<Vec<_>>());
    div()
        .id("shortcuts")
        .role(gpui::Role::Dialog)
        .aria_label("Keyboard shortcuts")
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(20.))
                .py(px(14.))
                .border_b_1()
                .border_color(t.line)
                .child(div().text_size(px(sz::LG)).font_weight(FontWeight::SEMIBOLD).child("Keyboard shortcuts"))
                .child(Button::icon("shortcuts-close", "x", "Close (Esc)").on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))),
        )
        .child(
            div()
                .id("shortcuts-body")
                .overflow_y_scroll()
                .max_h(px(640.))
                .px(px(20.))
                .pt(px(16.))
                .pb(px(6.))
                .flex()
                .gap(px(28.))
                .child(column(&["Playback", "Timeline"], cx))
                .child(column(&["Editing"], cx))
                .child(div().flex_1().min_w_0().flex().flex_col().child(group("Panels", cx)).child(group("Project", cx)).child(mouse)),
        )
        .into_any_element()
}
