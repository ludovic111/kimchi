//! Modal dialogs (tier-3 glass over the scrim): settings, export, the command
//! palette, the keyboard shortcuts.

pub mod export;
pub mod palette;
pub mod settings;
pub mod shortcuts;

use gpui::{AnyElement, App, Context, Entity, MouseButton, Render, Subscription, Window, deferred, div, prelude::*, px};

use crate::store::{Dialog, Store, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{GlassExt, motion};

pub struct Dialogs {
    store: Entity<Store>,
    settings: Entity<settings::SettingsDialog>,
    export: Entity<export::ExportDialog>,
    palette: Entity<palette::Palette>,
    _sub: Subscription,
}

impl Dialogs {
    #[cfg(test)]
    pub fn palette(&self) -> Entity<palette::Palette> {
        self.palette.clone()
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let sub = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            settings: cx.new(|cx| settings::SettingsDialog::new(window, cx)),
            export: cx.new(|cx| export::ExportDialog::new(window, cx)),
            palette: cx.new(|cx| palette::Palette::new(window, cx)),
            store,
            _sub: sub,
        }
    }
}

/// The frame every dialog sits in: scrim, centred tier-3 glass panel. Clicking the scrim closes it.
/// The scrim fades in and the panel rises into place.
pub fn modal(name: &'static str, width: f32, content: impl IntoElement, top: bool, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let panel = div()
        .id("modal")
        .occlude()
        .relative()
        .w(px(width))
        .max_h(px(760.))
        .rounded(px(sz::R_LG))
        .glass(t.glass3)
        .shadow(t.glass_shadow())
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(content);
    let scrim = div()
        .id("modal-scrim")
        .occlude()
        .absolute()
        .inset_0()
        .bg(t.scrim)
        .flex()
        .justify_center()
        .when(top, |d| d.items_start().pt(px(90.)))
        .when(!top, |d| d.items_center())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))
        .child(motion::enter(panel, (name, 1usize), motion::BASE, (0., if top { -6. } else { 10. })));
    deferred(motion::fade(scrim, (name, 0usize), motion::FAST)).with_priority(1).into_any_element()
}

impl Render for Dialogs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog = self.store.read(cx).dialog.clone();
        // Over the whole window (the scrim and the centring are relative to this).
        div().when(dialog.is_some(), |d| d.absolute().inset_0()).children(match dialog {
            Some(Dialog::Settings { .. }) => Some(modal("settings", 760., self.settings.clone(), false, cx)),
            Some(Dialog::Export) => Some(modal("export", 520., self.export.clone(), false, cx)),
            Some(Dialog::Palette) => Some(modal("palette", 620., self.palette.clone(), true, cx)),
            Some(Dialog::Shortcuts) => Some(modal("shortcuts", 780., shortcuts::sheet(cx), false, cx)),
            None => None,
        })
    }
}
