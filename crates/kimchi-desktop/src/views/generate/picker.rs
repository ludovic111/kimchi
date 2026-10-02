//! The model picker: the chosen model in the composer, and a searchable list
//! of the models that can do the task, grouped by provider, featured first.

use std::collections::BTreeMap;

use gpui::{AnyElement, Context, ElementId, FontWeight, SharedString, Window, anchored, deferred, div, prelude::*, px, relative};
use kimchi_gen::ModelInfo;

use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{GlassExt, icon};
use crate::views::generate::{bounds_probe, eyebrow, model_key};
use crate::views::generate_panel::GeneratePanel;

impl GeneratePanel {
    /// Models matching the search, by provider; groups with a featured model first.
    pub(crate) fn picker_groups(&self, cx: &gpui::App) -> Vec<(String, Vec<ModelInfo>)> {
        let q = self.picker_query.read(cx).text().trim().to_lowercase();
        let mut by: BTreeMap<String, Vec<ModelInfo>> = BTreeMap::new();
        for m in self.available(cx) {
            let hit = q.is_empty()
                || m.name.to_lowercase().contains(&q)
                || m.id.to_lowercase().contains(&q)
                || self.provider_name(&m.provider, cx).to_lowercase().contains(&q);
            if hit {
                by.entry(m.provider.clone()).or_default().push(m);
            }
        }
        let mut groups: Vec<(String, Vec<ModelInfo>)> = by
            .into_iter()
            .map(|(p, mut list)| {
                list.sort_by(|a, b| b.featured.cmp(&a.featured).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                (p, list)
            })
            .collect();
        groups.sort_by_key(|(_, list)| !list.first().is_some_and(|m| m.featured));
        groups
    }

    fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = !self.picker_open;
        if self.picker_open {
            crate::ui::input::focus(&self.picker_query, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn picker(&mut self, model: Option<&ModelInfo>, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let loading = self.store.read(cx).models_loading;
        let (title, sub, local): (SharedString, Option<String>, Option<bool>) = match model {
            Some(m) => {
                let provider = self.provider_name(&m.provider, cx);
                let sub = match &m.price {
                    Some(p) => format!("{provider} · {p}"),
                    None => provider,
                };
                (m.name.clone().into(), Some(sub), Some(self.provider_is_local(&m.provider, cx)))
            }
            None if loading => ("Loading models…".into(), None, None),
            None => ("No model for this".into(), Some("Connect a provider".into()), None),
        };
        let trigger = div()
            .id("model-picker")
            .flex()
            .items_center()
            .gap(px(8.))
            .w_full()
            .h(px(36.))
            .pl(px(6.))
            .pr(px(10.))
            .rounded(px(sz::R_MD))
            .text_color(t.text_2)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .when(self.picker_open, |d| d.bg(t.hover))
            .tooltip(|_, cx| crate::ui::tooltip("Choose a model".into(), cx))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_picker(window, cx)))
            .when_some(local, |d, local| {
                d.child(
                    div()
                        .flex_none()
                        .size(px(24.))
                        .rounded(px(7.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if local { t.success.opacity(0.16) } else { t.hover })
                        .text_color(if local { t.success } else { t.text_2 })
                        .child(icon(if local { "cpu" } else { "cloud" }).size(px(12.))),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(title))
                    .when_some(sub, |d, s| d.child(div().truncate().text_size(px(sz::XS)).text_color(t.text_2).child(s))),
            )
            .child(icon("chevron-down"));

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .child(bounds_probe(self.picker_anchor.clone()))
            .child(trigger)
            .when(self.picker_open, |d| d.child(div().absolute().left_0().top(relative(1.)).child(deferred(anchored().snap_to_window_with_margin(px(8.)).child(self.picker_popover(model, cx))).with_priority(3))))
            .into_any_element()
    }

    fn picker_popover(&mut self, current: Option<&ModelInfo>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let loading = self.store.read(cx).models_loading;
        let total = self.available(cx).len();
        let current = current.map(model_key);
        let groups = self.picker_groups(cx);
        let anchor = self.picker_anchor.clone();
        if self.picker_total != Some(total) {
            self.picker_total = Some(total);
            let placeholder = format!("Search {total} model{}", if total == 1 { "" } else { "s" });
            self.picker_query.update(cx, |i, cx| i.set_placeholder(placeholder, cx));
        }

        let list = if groups.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .py(px(18.))
                .px(px(12.))
                .text_color(t.text_2)
                .child(if loading { "Loading models…" } else { "No connected model can do this yet." })
                .child(
                    div()
                        .id("picker-add-provider")
                        .text_color(t.accent_text)
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child("Add a provider →")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.picker_open = false;
                            this.store.update(cx, |s, cx| s.open_dialog(crate::store::Dialog::Settings { section: Some("models".into()) }, cx));
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(groups.into_iter().map(|(provider, models)| {
                    let name = self.provider_name(&provider, cx);
                    div().flex().flex_col().child(eyebrow(&name, cx).px(px(8.)).pt(px(10.)).pb(px(4.))).children(models.into_iter().map(|m| {
                        let key = model_key(&m);
                        let on = current.as_ref() == Some(&key);
                        let tip: SharedString = m.description.clone().unwrap_or_else(|| m.id.clone()).into();
                        let mut caps: Vec<(String, bool)> = vec![];
                        if let (Some(a), Some(b)) = (m.durations.first(), m.durations.last()) {
                            caps.push((if a == b { format!("{a}s") } else { format!("{a}–{b}s") }, false));
                        }
                        if let Some(r) = m.resolutions.last() {
                            caps.push((r.clone(), false));
                        }
                        if m.end_frame {
                            caps.push(("first+last".into(), false));
                        }
                        if m.max_images > 1 {
                            caps.push((format!("{} refs", m.max_images), false));
                        }
                        if m.audio {
                            caps.push(("sound".into(), true));
                        }
                        let m2 = m.clone();
                        div()
                            .id(ElementId::Name(key.clone().into()))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(8.))
                            .py(px(7.))
                            .rounded(px(sz::R_SM))
                            .cursor_pointer()
                            .when(on, |d| d.bg(t.accent_soft))
                            .when(!on, |d| d.hover(|s| s.bg(t.hover)))
                            .tooltip(move |_, cx| crate::ui::tooltip(tip.clone(), cx))
                            .on_click(cx.listener(move |this, _, _, cx| this.pick_model(&m2, cx)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(5.))
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(t.text)
                                            .when(m.featured, |d| d.child(icon("star").size(px(10.)).text_color(t.warning)))
                                            .child(div().truncate().child(m.name.clone())),
                                    )
                                    .when(!caps.is_empty(), |d| {
                                        d.child(div().flex().flex_wrap().gap(px(4.)).children(caps.into_iter().map(|(c, sound)| {
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(3.))
                                                .px(px(5.))
                                                .rounded(px(sz::R_XS))
                                                .bg(t.hover)
                                                .text_size(px(10.))
                                                .text_color(if sound { t.success } else { t.text_2 })
                                                .when(sound, |d| d.child(icon("audio-lines").size(px(10.))))
                                                .child(c)
                                        })))
                                    }),
                            )
                            .when_some(m.price.clone(), |d, p| d.child(div().flex_none().text_size(px(sz::XS)).text_color(t.text_2).child(p)))
                            .when(on, |d| d.child(icon("check").text_color(t.accent_text)))
                    }))
                }))
                .into_any_element()
        };

        div()
            .id("model-popover")
            .occlude()
            .mt(px(10.))
            .w(px(400.))
            .max_h(px(460.))
            .flex()
            .flex_col()
            .rounded(px(sz::R_LG))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .overflow_hidden()
            .text_size(px(sz::BASE))
            .on_mouse_down_out(cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                if anchor.get().is_some_and(|b| b.contains(&e.position)) {
                    return;
                }
                this.picker_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .py(px(4.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_color(t.text_2)
                    .child(icon("search"))
                    .child(div().flex_1().child(self.picker_query.clone()))
                    .child(
                        div()
                            .id("picker-refresh")
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(sz::R_SM))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .tooltip(|_, cx| crate::ui::tooltip("Refresh model lists".into(), cx))
                            .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.load_models(true, cx))))
                            .child(crate::views::generate::spinner_icon("refresh-cw", loading, "picker-refresh-spin", 13.)),
                    ),
            )
            .child(div().id("picker-list").flex_1().min_h_0().overflow_y_scroll().px(px(6.)).pt(px(4.)).pb(px(8.)).child(list))
            .into_any_element()
    }
}
