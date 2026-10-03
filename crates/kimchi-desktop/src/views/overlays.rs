//! Things that float above the window: the context menu and toasts.

use gpui::{AnyElement, App, FontWeight, MouseButton, anchored, deferred, div, prelude::*, px};
use kimchi_control::ToastKind;

use crate::store::{ContextMenu, MenuEntry, StoreExt, Toast};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{GlassExt, icon, kbd, motion};

pub fn context_menu(menu: ContextMenu, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let items = menu.entries.into_iter().enumerate().map(|(i, e)| match e {
        MenuEntry::Separator => div().my(px(4.)).h(px(1.)).bg(t.line).into_any_element(),
        MenuEntry::Item(item) => {
            let color = if item.danger {
                t.danger
            } else if item.ai {
                t.accent_text
            } else {
                t.text
            };
            let action = item.action.clone();
            div()
                .id(("menu-item", i))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(sz::R_SM))
                .text_color(color)
                .when(item.disabled, |d| d.opacity(0.4))
                .when(!item.disabled, |d| {
                    d.cursor_pointer().hover(|s| s.bg(t.accent_soft)).on_click(move |_, window, cx| {
                        cx.store().update(cx, |s, cx| s.close_menu(cx));
                        action(window, cx);
                    })
                })
                .child(div().w(px(14.)).when_some(item.icon, |d, i| d.child(icon(i).text_color(color))))
                .child(div().flex_1().child(item.label))
                .when_some(item.shortcut, |d, s| d.child(kbd(s, cx)))
                .into_any_element()
        }
    });
    // A new id per opening position, so each menu fades in where it was asked for.
    let key = (f32::from(menu.position.x) as i32 as usize).wrapping_mul(10007) ^ f32::from(menu.position.y) as i32 as usize;
    deferred(
        anchored().position(menu.position).snap_to_window_with_margin(px(8.)).child(motion::enter(
            div()
                .id("context-menu")
                .occlude()
                .relative()
                .min_w(px(220.))
                .p(px(4.))
                .rounded(px(sz::R_MD))
                .glass(t.glass2)
                .shadow(t.glass_shadow())
                .text_size(px(sz::BASE))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(|_, _, cx| cx.store().update(cx, |s, cx| s.close_menu(cx)))
                .children(items),
            ("menu-in", key),
            motion::FAST,
            (0., -4.),
        )),
    )
    .with_priority(2)
    .into_any_element()
}

pub fn toasts(list: Vec<Toast>, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    div()
        .absolute()
        .bottom(px(16.))
        .right(px(16.))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(8.))
        .children(list.into_iter().map(|toast| {
            let (ic, color) = match toast.kind {
                ToastKind::Error => ("circle-alert", t.danger),
                ToastKind::Success => ("circle-check", t.success),
                ToastKind::Info => ("info", t.text_2),
            };
            let id = toast.id;
            let el = div()
                .id(("toast", id as usize))
                .relative()
                .max_w(px(420.))
                .flex()
                .items_start()
                .gap(px(8.))
                .px(px(12.))
                .py(px(9.))
                .rounded(px(sz::R_MD))
                .glass(t.glass2)
                .shadow(t.glass_shadow())
                .cursor_pointer()
                .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.dismiss(id, cx)))
                .child(icon(ic).text_color(color).mt(px(2.)))
                .child(div().text_size(px(sz::BASE)).font_weight(FontWeight::MEDIUM).child(toast.text));
            motion::enter(el, ("toast-in", id as usize), motion::BASE, (18., 0.))
        }))
        .into_any_element()
}
