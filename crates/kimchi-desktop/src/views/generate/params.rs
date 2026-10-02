//! A model's own parameters (`ModelInfo::params`): numbers scrub, switches
//! toggle, choices open a menu, text is typed. Values go to `generate.submit`
//! as `params`, keyed by the spec's `key`.

use gpui::{AnyElement, Context, Entity, SharedString, Subscription, div, prelude::*, px};
use kimchi_gen::{ParamKind, ParamSpec};
use serde_json::{Value, json};

use crate::store::{MenuItem, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::{icon, switch};
use crate::views::generate_panel::GeneratePanel;

pub enum ParamWidget {
    Number(Entity<Scrub>),
    Text(Entity<TextInput>),
    /// Drawn from the value alone (switches, choices).
    Plain,
}

pub struct ParamField {
    pub spec: ParamSpec,
    pub widget: ParamWidget,
    _sub: Option<Subscription>,
}

fn decimals_for(step: f64) -> usize {
    if step <= 0.0 || step >= 1.0 { 0 } else { (-step.log10().floor()).max(0.0) as usize }
}

impl ParamField {
    pub fn new(spec: ParamSpec, cx: &mut Context<GeneratePanel>) -> Self {
        let key = spec.key.clone();
        let (widget, sub) = match &spec.kind {
            ParamKind::Int { min, max, step } => {
                let v = spec.default.as_f64().unwrap_or(*min as f64);
                let scrub = cx.new(|_| {
                    let mut s = Scrub::new(spec.label.clone(), (*step).max(1) as f64, 0).range(*min as f64, *max as f64);
                    s.set_value(v);
                    s
                });
                let sub = cx.subscribe(&scrub, move |this, _, e: &ScrubChange, cx| {
                    this.draft.params.insert(key.clone(), json!(e.value.round() as i64));
                    cx.notify();
                });
                (ParamWidget::Number(scrub), Some(sub))
            }
            ParamKind::Float { min, max, step } => {
                let v = spec.default.as_f64().unwrap_or(*min);
                let scrub = cx.new(|_| {
                    let mut s = Scrub::new(spec.label.clone(), if *step > 0.0 { *step } else { 0.01 }, decimals_for(*step)).range(*min, *max);
                    s.set_value(v);
                    s
                });
                let sub = cx.subscribe(&scrub, move |this, _, e: &ScrubChange, cx| {
                    this.draft.params.insert(key.clone(), json!(e.value));
                    cx.notify();
                });
                (ParamWidget::Number(scrub), Some(sub))
            }
            ParamKind::Text { multiline } => {
                let text = spec.default.as_str().unwrap_or("").to_string();
                let multiline = *multiline;
                let input = cx.new(|cx| {
                    let mut i = if multiline { TextInput::new(cx).multiline(3) } else { TextInput::new(cx) };
                    i.set_text(text, cx);
                    i
                });
                let sub = cx.subscribe(&input, move |this, _, e: &InputEvent, cx| {
                    if let InputEvent::Changed(v) = e {
                        this.draft.params.insert(key.clone(), json!(v));
                        cx.notify();
                    }
                });
                (ParamWidget::Text(input), Some(sub))
            }
            ParamKind::Bool | ParamKind::Select { .. } => (ParamWidget::Plain, None),
        };
        Self { spec, widget, _sub: sub }
    }
}

impl GeneratePanel {
    pub(crate) fn render_param(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let f = &self.param_fields[i];
        let spec = f.spec.clone();
        let value = self.draft.params.get(&spec.key).cloned().unwrap_or_else(|| spec.default.clone());
        let help: Option<SharedString> = spec.help.clone().map(Into::into);
        let el = match (&f.widget, &spec.kind) {
            (ParamWidget::Number(scrub), _) => div().child(scrub.clone()).into_any_element(),
            (ParamWidget::Text(input), _) => div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(div().text_size(px(sz::XS)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.text_2).child(spec.label.clone()))
                .child(input.clone())
                .into_any_element(),
            (_, ParamKind::Bool) => {
                let key = spec.key.clone();
                let this = cx.entity().downgrade();
                switch(
                    ("param-bool", i),
                    spec.label.clone(),
                    value.as_bool().unwrap_or(false),
                    move |on, _, cx| {
                        let key = key.clone();
                        this.update(cx, |p, cx| {
                            p.draft.params.insert(key, Value::Bool(on));
                            cx.notify();
                        })
                        .ok();
                    },
                    cx,
                )
                .into_any_element()
            }
            (_, ParamKind::Select { options }) => {
                let current = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                let shown = options.iter().find(|o| o.value == current).map(|o| o.label.clone()).unwrap_or(current.clone());
                let options = options.clone();
                let key = spec.key.clone();
                let this = cx.entity().downgrade();
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .min_h(px(28.))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(spec.label.clone()))
                    .child(
                        div()
                            .id(("param-select", i))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .max_w(px(180.))
                            .h(px(26.))
                            .px(px(8.))
                            .rounded(px(sz::R_SM))
                            .bg(t.bg_sunken.opacity(0.6))
                            .border_1()
                            .border_color(t.line_strong)
                            .text_size(px(sz::SM))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .child(div().min_w_0().truncate().child(shown))
                            .child(icon("chevron-down").text_color(t.text_2))
                            .on_click(move |e, _, cx| {
                                let entries = options
                                    .iter()
                                    .map(|o| {
                                        let (this, key, v) = (this.clone(), key.clone(), o.value.clone());
                                        let mut item = MenuItem::new(o.label.clone(), move |_, cx| {
                                            let (key, v) = (key.clone(), v.clone());
                                            this.update(cx, |p, cx| {
                                                p.draft.params.insert(key, Value::String(v));
                                                cx.notify();
                                            })
                                            .ok();
                                        });
                                        if o.value == current {
                                            item = item.icon("check");
                                        }
                                        item.entry()
                                    })
                                    .collect();
                                let pos = e.position();
                                cx.store().update(cx, |s, cx| s.open_menu(pos, entries, cx));
                            }),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        };
        match help {
            Some(h) => div().id(("param", i)).tooltip(move |_, cx| crate::ui::tooltip(h.clone(), cx)).child(el).into_any_element(),
            None => el,
        }
    }
}
