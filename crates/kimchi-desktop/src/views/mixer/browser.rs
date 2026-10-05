//! The effect browser: a popover where the add-effect buttons ask for it, with a search field
//! and every effect grouped by category, its format (ryolune's stock ones, CLAP, VST3, Audio
//! Units, ryolune native) and vendor; the description on hover. Enter adds the first match;
//! the new effect's panel opens. Adding is `audio.addEffect`.

use gpui::{AnyElement, Context, Entity, MouseButton, Render, SharedString, Subscription, Window, anchored, deferred, div, prelude::*, px};
use kimchi_audio::plugins::EffectInfo;
use serde_json::json;

use super::{Target, target_key};
use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{GlassExt, icon, motion, tooltip};

pub struct EffectBrowser {
    store: Entity<Store>,
    query: Entity<TextInput>,
    /// Every effect, read when the browser opens.
    all: Vec<EffectInfo>,
    /// The chain it was last opened for (to focus the field once per opening).
    opened: Option<Target>,
    _subs: Vec<Subscription>,
}

/// Effects matching `query` (every word in the name, vendor, category or format), in the
/// catalogue's order (stock first).
pub fn matching<'a>(all: &'a [EffectInfo], query: &str) -> Vec<&'a EffectInfo> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    all.iter()
        .filter(|e| {
            let hay = format!("{} {} {} {}", e.name, e.vendor, e.category, e.format).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

impl EffectBrowser {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let query = cx.new(|cx| TextInput::new(cx).placeholder("Search effects"));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&query, |this: &mut Self, input, e: &InputEvent, cx| match e {
                InputEvent::Submit => {
                    let q = input.read(cx).text().to_string();
                    if let Some(first) = matching(&this.all, &q).first().map(|e| e.id.clone()) {
                        this.add(&first, cx);
                    }
                }
                InputEvent::Cancel => this.close(cx),
                InputEvent::Changed(_) => cx.notify(),
                InputEvent::Blur => {}
            }),
        ];
        Self { store, query, all: vec![], opened: None, _subs: subs }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.set_audio(|a| a.browser = None, cx));
    }

    /// Adds `effect` to the chain the browser is open for, then opens its panel.
    pub fn add(&mut self, effect: &str, cx: &mut Context<Self>) {
        let Some((target, _)) = self.store.read(cx).audio.browser else { return };
        self.store.update(cx, |s, cx| {
            s.set_audio(|a| a.browser = None, cx);
            s.run_then("audio.addEffect", json!({ "target": target_key(target), "effect": effect }), cx, move |s, v, cx| {
                if let Some(slot) = v["slot"].as_str() {
                    let slot = slot.to_string();
                    s.set_audio(|a| a.effect = Some((target, slot)), cx);
                }
            });
        });
    }
}

impl Render for EffectBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let Some((target, position)) = self.store.read(cx).audio.browser else {
            self.opened = None;
            return div().into_any_element();
        };
        if self.opened != Some(target) {
            self.opened = Some(target);
            self.all = kimchi_audio::plugins::effects(None);
            self.query.update(cx, |i, cx| i.set_text("", cx));
            let input = self.query.clone();
            cx.defer_in(window, move |_, window, cx| crate::ui::input::focus(&input, window, cx));
        }
        let owner = self.store.read(cx).project.as_ref().map(|p| target.name(p)).unwrap_or_default();
        let q = self.query.read(cx).text().to_string();
        let found = matching(&self.all, &q);
        let mut rows: Vec<AnyElement> = vec![];
        let mut category = String::new();
        for (i, e) in found.iter().enumerate() {
            if e.category != category {
                category = e.category.clone();
                rows.push(div().pt(px(if rows.is_empty() { 0. } else { 6. })).pb(px(2.)).px(px(8.)).child(crate::ui::caps(category.clone(), cx)).into_any_element());
            }
            let id = e.id.clone();
            let desc: SharedString = if e.description.is_empty() { format!("{} by {}", e.name, e.vendor).into() } else { e.description.clone().into() };
            let format = if e.format == "ryolune" { "Stock".to_string() } else { e.format.clone() };
            rows.push(
                div()
                    .id(("effect-row", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(sz::R_SM))
                    .cursor_pointer()
                    .when(i == 0 && !q.is_empty(), |d| d.bg(t.accent_soft))
                    .hover(|d| d.bg(t.hover))
                    .tooltip(move |_, cx| tooltip(desc.clone(), cx))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(sz::BASE)).text_color(t.text).child(e.name.clone()))
                    .when(e.vendor != "ryolune", |d| d.child(div().flex_none().max_w(px(90.)).truncate().text_size(px(sz::XS)).text_color(t.text_3).child(e.vendor.clone())))
                    .child(div().flex_none().px(px(5.)).rounded(px(sz::R_XS)).border_1().border_color(t.line).font_family(MONO).text_size(px(9.5)).text_color(t.text_2).child(format))
                    .on_click(cx.listener(move |this, _, _, cx| this.add(&id, cx)))
                    .into_any_element(),
            );
        }
        if found.is_empty() {
            rows.push(div().p(px(12.)).text_size(px(sz::SM)).text_color(t.text_3).child(format!("No effect matches \u{201c}{q}\u{201d}.")).into_any_element());
        }
        let panel = div()
            .id("effect-browser")
            .occlude()
            .relative()
            .w(px(300.))
            .max_h(px(440.))
            .flex()
            .flex_col()
            .rounded(px(sz::R_MD))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .p(px(8.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(icon("search").text_color(t.text_3))
                    .child(div().flex_1().child(self.query.clone())),
            )
            .child(div().px(px(10.)).pt(px(6.)).text_size(px(sz::XS)).text_color(t.text_2).child(format!("Add to {owner}")))
            .child(div().id("effect-list").flex_1().min_h_0().overflow_y_scroll().p(px(4.)).children(rows))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(10.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(t.line)
                    .text_size(px(sz::XS))
                    .text_color(t.text_3)
                    .child(format!("{} effects", self.all.len()))
                    .child(
                        div()
                            .id("rescan-plugins")
                            .cursor_pointer()
                            .hover(|d| d.text_color(t.text))
                            .child("Rescan plugins")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.store.update(cx, |s, cx| {
                                    s.flash("Looking for plugins…", cx);
                                    s.run_then("audio.rescanPlugins", json!({}), cx, |s, v, cx| s.flash(format!("{} effects", v["effects"].as_u64().unwrap_or(0)), cx));
                                });
                                this.opened = None;
                                cx.notify();
                            })),
                    ),
            );
        deferred(anchored().position(position).snap_to_window_with_margin(px(8.)).child(motion::enter(panel, "effect-browser-in", motion::FAST, (0., -4.)))).with_priority(2).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_every_word() {
        let all = kimchi_audio::plugins::effects(None);
        assert!(matching(&all, "").len() == all.len());
        let eq = matching(&all, "channel eq");
        assert!(eq.iter().any(|e| e.name == "Channel EQ"), "{eq:?}");
        assert!(matching(&all, "zzz nothing").is_empty());
    }
}
