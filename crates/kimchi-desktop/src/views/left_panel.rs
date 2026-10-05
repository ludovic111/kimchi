//! The left column: media, generate, text, motion and captions on glass tier 1, picked from
//! the rail of tabs along the window's left edge ([`rail`], drawn by the editor).

use gpui::{App, Context, Entity, FontWeight, Render, Subscription, Window, div, prelude::*, px};

use crate::store::{LeftTab, Store, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::layout::RAIL_W;
use crate::ui::{GlassExt, icon, motion};
use crate::views::{captions_panel::CaptionsPanel, generate_panel::GeneratePanel, media_panel::MediaPanel, motion_panel::MotionPanel, text_panel::TextPanel};

pub struct LeftPanel {
    store: Entity<Store>,
    pub media: Entity<MediaPanel>,
    pub generate: Entity<GeneratePanel>,
    pub text: Entity<TextPanel>,
    pub motion: Entity<MotionPanel>,
    pub captions: Entity<CaptionsPanel>,
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
            motion: cx.new(|cx| MotionPanel::new(window, cx)),
            captions: cx.new(|cx| CaptionsPanel::new(window, cx)),
            store,
            _sub: sub,
        }
    }
}

impl Render for LeftPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let tab = self.store.read(cx).left_tab;
        div()
            .size_full()
            .flex()
            .flex_col()
            .glass(t.glass1)
            .border_0()
            .border_r_1()
            .border_color(t.line)
            // A new id per tab, so switching fades the new one in.
            .child(motion::fade(
                div().flex_1().min_h_0().child(match tab {
                    LeftTab::Media => self.media.clone().into_any_element(),
                    LeftTab::Generate => self.generate.clone().into_any_element(),
                    LeftTab::Text => self.text.clone().into_any_element(),
                    LeftTab::Motion => self.motion.clone().into_any_element(),
                    LeftTab::Captions => self.captions.clone().into_any_element(),
                }),
                gpui::ElementId::Name(tab.as_str().into()),
                motion::FAST,
            ))
    }
}

/// The tabs of the left panel, as a rail down the window's left edge (always there, also
/// when the panel is closed or shown as a drawer). The open tab is lit; clicking it again
/// closes the panel. `badge` counts running generations on the Generate tab.
pub fn rail(tab: LeftTab, shown: bool, badge: usize, on_pick: impl Fn(LeftTab, &mut Window, &mut App) + 'static, cx: &App) -> gpui::AnyElement {
    let t = cx.theme().clone();
    let on_pick = std::rc::Rc::new(on_pick);
    let item = |id: &'static str, label: &'static str, ic: &'static str, which: LeftTab, count: usize, action: fn() -> Box<dyn gpui::Action>| {
        let current = tab == which;
        let lit = current && shown;
        let on_pick = on_pick.clone();
        div()
            .id(id)
            .relative()
            .w(px(RAIL_W - 10.))
            .h(px(46.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(3.))
            .rounded(px(sz::R_MD))
            .cursor_pointer()
            .role(gpui::Role::Tab)
            .aria_label(label)
            .when(lit, |d| d.bg(t.accent_soft).text_color(t.accent_text))
            .when(!lit, |d| d.text_color(if current { t.text } else { t.text_2 }).hover(|s| s.bg(t.hover).text_color(t.text)))
            .tooltip(move |_, cx| crate::ui::tooltip(crate::actions::tip(if lit { "Hide the panel" } else { label }, &*action()), cx))
            .child(icon(ic).size(px(18.)))
            .child(div().text_size(px(10.)).font_weight(if lit { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).line_height(px(12.)).child(label))
            .when(count > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(6.))
                        .min_w(px(15.))
                        .h(px(15.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(t.accent)
                        .text_color(t.text_on_accent)
                        .text_size(px(9.5))
                        .font_weight(FontWeight::BOLD)
                        .child(count.to_string()),
                )
            })
            .on_click(move |_, w, cx| on_pick(which, w, cx))
    };
    div()
        .id("rail")
        .debug_selector(|| "rail".into())
        .w(px(RAIL_W))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .pt(px(8.))
        .glass(t.glass1)
        .border_0()
        .border_r_1()
        .border_color(t.line)
        .child(item("tab-media", "Media", "film", LeftTab::Media, 0, || Box::new(crate::actions::ShowMedia)))
        .child(item("tab-generate", "Generate", "sparkles", LeftTab::Generate, badge, || Box::new(crate::actions::ShowGenerate)))
        .child(item("tab-text", "Text", "type", LeftTab::Text, 0, || Box::new(crate::actions::ShowText)))
        .child(item("tab-motion", "Motion", "shapes", LeftTab::Motion, 0, || Box::new(crate::actions::ShowMotion)))
        .child(item("tab-captions", "Captions", "captions", LeftTab::Captions, 0, || Box::new(crate::actions::ShowCaptions)))
        .into_any_element()
}
