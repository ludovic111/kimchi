//! The inspector's Plugins section (picture clips): the video plugins on the clip, first to last,
//! each with its switch (bypass), its parameters (sliders for numbers, switches, choices) and
//! Remove; "Add" offers the effect and generator plugins on this computer. Everything goes
//! through `clip.addPlugin`, `clip.setPlugin` and `clip.removePlugin`; keyframes for plugin
//! parameters are `clip.setKeyframes` on `plugins.<slot>.<parameter>` (commands only for now).

use std::collections::HashMap;

use gpui::{AnyElement, Context, Entity, SharedString, Subscription, div, prelude::*, px};
use kimchi_core::{Clip, PluginValue};
use kimchi_media::render::plugins::{self as host, ParamKind, PluginKind, catalogue};
use serde_json::json;

use super::Inspector;
use super::slider::Slider;
use crate::store::{MenuEntry, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::scrub::ScrubChange;
use crate::ui::{Button, switch};

/// `{name: value}`.
fn one(name: &str, value: serde_json::Value) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert(name.to_string(), value);
    serde_json::Value::Object(m)
}

/// Sliders made for plugin parameters, by (clip, slot, parameter), with their subscriptions.
#[derive(Default)]
pub struct PluginSliders {
    sliders: HashMap<(kimchi_core::Id, String, String), (Entity<Slider>, Subscription)>,
}

impl Inspector {
    fn plugin_slider(&mut self, clip: kimchi_core::Id, slot: &str, name: &str, min: f64, max: f64, cx: &mut Context<Self>) -> Entity<Slider> {
        let key = (clip, slot.to_string(), name.to_string());
        if let Some((s, _)) = self.plugin_sliders.sliders.get(&key) {
            return s.clone();
        }
        let s = cx.new(|cx| Slider::new(min, max.max(min + 1e-6), cx));
        let (slot, name) = (slot.to_string(), name.to_string());
        let sub = cx.subscribe(&s, move |this: &mut Inspector, _, ch: &ScrubChange, cx| {
            let mut params = json!({ "clipId": clip, "slot": slot, "params": one(&name, json!(ch.value)) });
            if !ch.final_ {
                params["coalesce"] = json!(format!("{clip}:plugin:{slot}:{name}"));
            }
            this.store.update(cx, |s, cx| s.run("clip.setPlugin", params, cx));
        });
        self.plugin_sliders.sliders.insert(key, (s.clone(), sub));
        s
    }

    pub(super) fn plugins_section(&mut self, clip: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let id = clip.id;
        let playhead = self.store.read(cx).playback.read(cx).playhead;
        let now = clip.effects_at(playhead.clamp(clip.start, clip.end()));
        // Sliders of slots no longer on this clip go.
        let slots: Vec<String> = clip.effects.plugins.iter().map(|p| p.id.clone()).collect();
        self.plugin_sliders.sliders.retain(|(c, s, _), _| *c != id || slots.contains(s));

        let add = Button::new("plugin-add", "Add").small().with_icon("plus").on_click(move |e: &gpui::ClickEvent, _, cx| {
            let at = e.position();
            let entries: Vec<MenuEntry> = catalogue::plugins()
                .into_iter()
                .filter(|p| p.kind != PluginKind::Transition && !host::is_off(&p.id))
                .map(|p| {
                    let pid = p.id.clone();
                    crate::store::MenuItem::new(p.name.clone(), move |_, cx| cx.store().update(cx, |s, cx| s.run("clip.addPlugin", json!({ "clipIds": [id], "plugin": pid }), cx)))
                        .logo(if p.format == host::Format::Frei0r { "frei0r" } else { "lsuite" })
                        .entry()
                })
                .chain(std::iter::once(MenuEntry::Separator))
                .chain(std::iter::once(crate::store::MenuItem::new("Plugins…", |_, cx| cx.store().update(cx, |s, cx| s.open_dialog(crate::store::Dialog::Plugins { part: None }, cx))).icon("plug-zap").entry()))
                .collect();
            cx.store().update(cx, |s, cx| s.open_menu(at, entries, cx));
        });

        let mut fold = self.fold("plugins", "Plugins", cx).trailing(add);
        if clip.effects.plugins.is_empty() {
            fold = fold.child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Effects from plugins: kimchi's, ones your agent builds, frei0r."));
            return fold.into_any_element();
        }
        for (i, slot) in clip.effects.plugins.clone().into_iter().enumerate() {
            let info = catalogue::find(&slot.plugin);
            let values = now.plugins.iter().find(|p| p.id == slot.id).map(|p| p.params.clone()).unwrap_or_default();
            let sid = slot.id.clone();
            let on = !slot.bypass;
            let off = host::is_off(&slot.plugin);
            let mut block = div().flex().flex_col().gap(px(6.)).py(px(6.)).border_t_1().border_color(t.line).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(crate::ui::logo(if slot.plugin.starts_with("frei0r:") { "frei0r" } else { "lsuite" }, px(14.)))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(sz::BASE)).child(slot.name.clone()))
                    .child(switch(SharedString::from(format!("plugin-on-{i}")), "", on, {
                        let sid = sid.clone();
                        move |v, _, cx| cx.store().update(cx, |s, cx| s.run("clip.setPlugin", json!({ "clipId": id, "slot": sid, "bypass": !v }), cx))
                    }, cx))
                    .child(Button::icon(SharedString::from(format!("plugin-remove-{i}")), "x", "Remove this plugin").small().on_click({
                        let sid = sid.clone();
                        move |_, _, cx| cx.store().update(cx, |s, cx| s.run("clip.removePlugin", json!({ "clipId": id, "slot": sid }), cx))
                    })),
            );
            match &info {
                None => block = block.child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Not on this computer: drawn without it, its settings kept.")),
                Some(_) if off => block = block.child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Switched off in Plugins: drawn without it.")),
                Some(_) => {}
            }
            if let Some(info) = info.filter(|_| on) {
                for p in &info.params {
                    let v = values.get(&p.name).cloned().unwrap_or_else(|| p.default.clone());
                    let label = div().w(px(70.)).flex_none().truncate().text_size(px(sz::SM)).text_color(t.text_2).child(p.label.clone());
                    let row = div().flex().items_center().gap(px(6.)).child(label);
                    let row = match p.kind {
                        ParamKind::Number | ParamKind::Integer => {
                            let n = v.as_f64().unwrap_or(0.0);
                            let s = self.plugin_slider(id, &slot.id, &p.name, p.min, p.max, cx);
                            s.update(cx, |s, _| s.set_value(n));
                            let shown = if p.kind == ParamKind::Integer { format!("{}", n.round()) } else if (p.max - p.min) > 20.0 { format!("{n:.0}") } else { format!("{n:.2}") };
                            row.child(div().flex_1().flex().child(s)).child(div().w(px(44.)).flex_none().flex().justify_end().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(format!("{shown}{}", p.unit)))
                        }
                        ParamKind::Toggle => {
                            let on = matches!(v, PluginValue::Bool(true));
                            let (sid, name) = (slot.id.clone(), p.name.clone());
                            row.child(div().flex_1()).child(switch(SharedString::from(format!("plugin-{i}-{}", p.name)), "", on, move |b, _, cx| {
                                cx.store().update(cx, |s, cx| s.run("clip.setPlugin", json!({ "clipId": id, "slot": sid, "params": one(&name, json!(b)) }), cx))
                            }, cx))
                        }
                        ParamKind::Choice => {
                            let current = match &v {
                                PluginValue::Text(t) => t.clone(),
                                _ => String::new(),
                            };
                            let chips = p.choices.iter().map(|c| {
                                let (sid, name, choice) = (slot.id.clone(), p.name.clone(), c.clone());
                                Button::new(SharedString::from(format!("plugin-{i}-{}-{c}", p.name)), c.clone()).small().selected(*c == current).on_click(move |_, _, cx| {
                                    cx.store().update(cx, |s, cx| s.run("clip.setPlugin", json!({ "clipId": id, "slot": sid, "params": one(&name, json!(choice)) }), cx))
                                })
                            });
                            row.child(div().flex_1().flex().flex_wrap().gap(px(3.)).children(chips))
                        }
                        _ => row.child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(kimchi_media::render::plugins::value::describe(p, &v))),
                    };
                    block = block.child(row);
                }
            }
            fold = fold.child(block);
        }
        fold.into_any_element()
    }
}
