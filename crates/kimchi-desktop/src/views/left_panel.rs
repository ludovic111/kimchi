//! The left column: media, generate and text, as tabs on glass tier 1.

use gpui::{Context, Entity, Render, Subscription, Window, div, prelude::*, px};

use crate::store::{LeftTab, Store, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{GlassExt, icon};
use crate::views::{generate_panel::GeneratePanel, media_panel::MediaPanel, text_panel::TextPanel};

pub struct LeftPanel {
    store: Entity<Store>,
    pub media: Entity<MediaPanel>,
    pub generate: Entity<GeneratePanel>,
    pub text: Entity<TextPanel>,
    _sub: Subscription,
}

impl LeftPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let sub = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            media: cx.new(|cx| MediaPanel::new(window, cx)),
            generate: cx.new(|cx| GeneratePanel::new(window, cx)),
            text: cx.new(|cx| TextPanel::new(window, cx)),
            store,
            _sub: sub,
        }
    }
}

impl Render for LeftPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let tab = self.store.read(cx).left_tab;
        let active_jobs = self.store.read(cx).jobs.iter().filter(|j| !j.status.is_done()).count();
        let tab_button = |id: &'static str, label: &'static str, ic: &'static str, which: LeftTab, badge: Option<usize>| {
            let selected = tab == which;
            div()
                .id(id)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(px(30.))
                .rounded(px(sz::R_SM))
                .cursor_pointer()
                .text_size(px(sz::BASE))
                .when(selected, |d| d.bg(t.accent_soft).text_color(t.accent_text))
                .when(!selected, |d| d.text_color(t.text_2).hover(|s| s.bg(t.hover)))
                .child(icon(ic))
                .child(label)
                .when_some(badge.filter(|n| *n > 0), |d, n| {
                    d.child(div().px(px(5.)).rounded_full().bg(t.accent).text_color(t.text_on_accent).text_size(px(10.)).child(n.to_string()))
                })
                .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.set_left_tab(which, cx)))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .glass(t.glass1)
            .border_0()
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .p(px(8.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(tab_button("tab-media", "Media", "film", LeftTab::Media, None))
                    .child(tab_button("tab-generate", "Generate", "sparkles", LeftTab::Generate, Some(active_jobs)))
                    .child(tab_button("tab-text", "Text", "type", LeftTab::Text, None)),
            )
            .child(div().flex_1().min_h_0().child(match tab {
                LeftTab::Media => self.media.clone().into_any_element(),
                LeftTab::Generate => self.generate.clone().into_any_element(),
                LeftTab::Text => self.text.clone().into_any_element(),
            }))
    }
}
