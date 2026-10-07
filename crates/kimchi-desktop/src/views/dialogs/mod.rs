//! Modal dialogs (tier-3 glass over the scrim): settings, export, the command
//! palette, the keyboard shortcuts, what's new.

pub mod export;
pub mod interop;
pub mod palette;
pub mod plugins;
pub mod settings;
pub mod shortcuts;
pub mod whats_new;

use gpui::{AnyElement, App, Context, Entity, MouseButton, Render, Subscription, Window, deferred, div, prelude::*, px};

use crate::store::{Dialog, Store, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{GlassExt, layout, motion};

pub struct Dialogs {
    store: Entity<Store>,
    settings: Entity<settings::SettingsDialog>,
    export: Entity<export::ExportDialog>,
    palette: Entity<palette::Palette>,
    plugins: Entity<plugins::PluginsDialog>,
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
            plugins: cx.new(|cx| plugins::PluginsDialog::new(window, cx)),
            store,
            _sub: sub,
        }
    }
}

/// The frame every dialog sits in: scrim, centred tier-3 panel inside corner brackets. Clicking the scrim closes it.
/// The scrim fades in and the panel rises into place. The panel is never larger than the window
/// (`layout::dialog_size`): a dialog lays itself out as a column whose middle part scrolls
/// (`flex_1().min_h_0()` and a scroll), so its header and buttons stay in view at any size.
pub fn modal(name: &'static str, width: f32, content: impl IntoElement, top: bool, window: &Window, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let viewport = window.viewport_size();
    let (w, max_h) = layout::dialog_size(width, 760., viewport);
    let top_gap = (f32::from(viewport.height) * 0.12).clamp(layout::DIALOG_MARGIN, 90.);
    let panel = div()
        .id("modal")
        .debug_selector(move || format!("dialog-{name}"))
        .occlude()
        .relative()
        .w(px(w))
        .max_h(px(if top { max_h.min(f32::from(viewport.height) - top_gap - layout::DIALOG_MARGIN) } else { max_h }))
        .flex()
        .flex_col()
        .rounded(px(sz::R_LG))
        .glass(t.glass3)
        .shadow(t.glass_shadow())
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().flex_1().min_h_0().flex().flex_col().child(content));
    // Framed like a viewfinder, the marks just outside the panel.
    let framed = div().relative().child(panel).child(crate::ui::grain::brackets(14., -9., t.text_3));
    let scrim = div()
        .id("modal-scrim")
        .occlude()
        .absolute()
        .inset_0()
        .bg(t.scrim)
        .flex()
        .justify_center()
        .when(top, |d| d.items_start().pt(px(top_gap)))
        .when(!top, |d| d.items_center())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))
        .child(motion::enter(framed, (name, 1usize), motion::BASE, (0., if top { -6. } else { 10. })));
    deferred(motion::fade(scrim, (name, 0usize), motion::FAST)).with_priority(1).into_any_element()
}

impl Render for Dialogs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog = self.store.read(cx).dialog.clone();
        // Over the whole window (the scrim and the centring are relative to this).
        div().when(dialog.is_some(), |d| d.absolute().inset_0()).children(match dialog {
            Some(Dialog::Settings { .. }) => Some(modal("settings", 760., self.settings.clone(), false, window, cx)),
            Some(Dialog::Export) => Some(modal("export", 520., self.export.clone(), false, window, cx)),
            Some(Dialog::Palette) => Some(modal("palette", 620., self.palette.clone(), true, window, cx)),
            Some(Dialog::Shortcuts) => Some(modal("shortcuts", 900., shortcuts::sheet(cx), false, window, cx)),
            Some(Dialog::WhatsNew { since, all }) => Some(modal("whats-new", 600., whats_new::sheet(since, all, cx), false, window, cx)),
            Some(Dialog::Interop { title, report }) => Some(modal("interop-report", 560., interop::sheet(title, report, cx), false, window, cx)),
            Some(Dialog::Plugins { .. }) => Some(modal("plugins", 860., self.plugins.clone(), false, window, cx)),
            None => None,
        })
    }
}
