//! The inspector's animation and motion sections.
//!
//! - Animation (every picture clip): a keyframe toggle per property at the playhead, jumps to
//!   the previous and next keyframe, the easing of the keyframes under the playhead, the
//!   ready-made presets, and clearing it all. Changing an animated property elsewhere in the
//!   inspector (or dragging it on the canvas) sets a keyframe at the playhead instead.
//! - Template (template motion clips): its values as fields; changing one re-makes the clip
//!   through `motion.setTemplate`. The scene itself is edited in `scene_editor`.

use gpui::{AnyElement, App, Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};
use kimchi_core::anim::Keyframe;
use kimchi_core::templates::{self, Template};
use kimchi_core::{Clip, ClipContent, Easing, Id, TemplateRef};
use serde_json::{Value, json};

use super::color::{ColorChange, ColorField};
use super::{Inspector, grid2, section};
use crate::store::StoreExt;
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::scrub::{Scrub, ScrubChange, Step};
use crate::ui::{Button, caps, segmented, switch};

/// Properties with a keyframe toggle: (property, label).
const PROPS: [(&str, &str); 6] = [("x", "X"), ("y", "Y"), ("scale", "Scale"), ("rotation", "Rotate"), ("opacity", "Opacity"), ("blur", "Blur")];

/// Easings offered for the keyframes under the playhead.
const EASINGS: [(&str, &str); 5] = [("linear", "Linear"), ("easeOutCubic", "Out"), ("easeInOutCubic", "In-out"), ("easeOutBack", "Back"), ("hold", "Hold")];

/// Preset groups: (label, [(preset, label)]).
const PRESET_GROUPS: [(&str, &[(&str, &str)]); 3] = [
    ("In", &[("fadeIn", "Fade"), ("riseIn", "Rise"), ("slideInLeft", "Slide"), ("popIn", "Pop"), ("zoomIn", "Zoom"), ("blurIn", "Focus")]),
    ("Out", &[("fadeOut", "Fade"), ("sinkOut", "Sink"), ("slideOutRight", "Slide"), ("popOut", "Pop"), ("zoomOut", "Zoom"), ("blurOut", "Blur")]),
    ("Over the clip", &[("kenBurns", "Ken Burns"), ("panLeft", "Pan"), ("pulse", "Pulse"), ("float", "Float"), ("shake", "Shake"), ("spin", "Spin")]),
];

/// A template value's control.
pub enum Field {
    Text(Entity<TextInput>),
    Color(Entity<ColorField>),
    Number(Entity<Scrub>),
    /// Choices and switches are drawn from the value each time.
    Plain,
}

/// The fields of the template clip last shown.
#[derive(Default)]
pub struct TemplateFields {
    clip: Option<Id>,
    fields: Vec<(&'static str, Field)>,
    _subs: Vec<Subscription>,
}

/// The playhead in the clip's own seconds, and how close counts as "on" a keyframe.
fn local(clip: &Clip, playhead: f64, fps: f64) -> (f64, f64) {
    (playhead - clip.start, 0.5 / fps.max(1.0))
}

fn key_at(keys: Option<&Vec<Keyframe>>, local: f64, tol: f64) -> Option<&Keyframe> {
    keys?.iter().find(|k| (k.time - local).abs() <= tol)
}

impl Inspector {
    /// `clip.update` params from a scrub, sent as a keyframe at the playhead when the
    /// property is animated. Returns false when there was nothing animated to set.
    pub(super) fn keyframe_instead(&mut self, params: &Value, key: &str, step: Step, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.single(cx) else { return false };
        let s = self.store.read(cx);
        let Some(clip) = s.clip(id) else { return false };
        let value = match key {
            "x" | "y" | "rotation" | "opacity" | "volume" => params.get(key).cloned(),
            "scale" => params.get("scale").cloned(),
            "fontSize" | "letterSpacing" => params.get("style").and_then(|st| st.get(key)).cloned(),
            "color" => params.get("style").and_then(|st| st.get("color")).cloned(),
            fx if kimchi_core::effects::EFFECT_PROPS.contains(&fx) => params.get(fx).cloned(),
            _ => None,
        };
        let (Some(value), true) = (value, clip.keyframes.contains_key(key)) else { return false };
        let playhead = s.playback.read(cx).playhead;
        let mut p = json!({ "clipId": id, "property": key, "time": playhead, "value": value });
        if let Some(k) = step.coalesce(format!("{id}:{key}:key")) {
            p["coalesce"] = json!(k);
        }
        self.store.update(cx, |s, cx| s.run("clip.addKeyframe", p, cx));
        true
    }

    /// The Animation section for a picture clip.
    pub(super) fn animation_section(&mut self, clip: &Clip, fps: f64, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let playhead = self.store.read(cx).playback.read(cx).playhead;
        let (local, tol) = local(clip, playhead, fps);
        let inside = playhead >= clip.start - tol && playhead <= clip.end() + tol;
        let id = clip.id;

        // Keyframe toggles.
        let chips = div().flex().flex_wrap().gap(px(4.)).children(PROPS.iter().map(|&(prop, label)| {
            let keys = clip.keyframes.get(prop);
            let here = key_at(keys, local, tol).is_some();
            let animated = keys.is_some();
            let tip = match (here, animated) {
                (true, _) => format!("Remove the {label} keyframe here"),
                (false, true) => format!("Add a {label} keyframe at the playhead"),
                (false, false) => format!("Animate {label}: a first keyframe at the playhead"),
            };
            let mut b = Button::new(SharedString::from(format!("key-{prop}")), label).small().with_icon("diamond").selected(here).tooltip(tip).disabled(!inside);
            if animated && !here {
                b = b.color(t.accent_text);
            }
            b.on_click(move |_, _, cx| {
                cx.store().update(cx, |s, cx| {
                    let playhead = s.playback.read(cx).playhead;
                    if here {
                        s.run("clip.removeKeyframe", json!({ "clipId": id, "property": prop, "time": playhead }), cx);
                    } else {
                        s.run("clip.addKeyframe", json!({ "clipId": id, "property": prop, "time": playhead }), cx);
                    }
                })
            })
        }));

        // Previous / next keyframe of any property.
        let mut times: Vec<f64> = clip.keyframes.values().flatten().map(|k| clip.start + k.time).collect();
        times.sort_by(f64::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        let prev = times.iter().rev().find(|t| **t < playhead - tol).copied();
        let next = times.iter().find(|t| **t > playhead + tol).copied();
        let seek = |time: Option<f64>| {
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                if let Some(time) = time {
                    cx.store().update(cx, |s, cx| s.playback.update(cx, |p, cx| p.seek(time, cx)));
                }
            }
        };
        let nav = div()
            .flex()
            .gap(px(2.))
            .child(Button::icon("key-prev", "chevron-left", "Previous keyframe").small().disabled(prev.is_none()).on_click(seek(prev)))
            .child(Button::icon("key-next", "chevron-right", "Next keyframe").small().disabled(next.is_none()).on_click(seek(next)));

        let mut body = self.fold("animation", "Animation", cx).trailing(nav).child(chips);

        // Easing of the keyframes under the playhead.
        let here: Vec<(String, Easing)> =
            clip.keyframes.iter().filter_map(|(name, keys)| key_at(Some(keys), local, tol).map(|k| (name.clone(), k.easing))).collect();
        if !here.is_empty() {
            let current = here.iter().map(|(_, e)| e.to_string()).next().unwrap_or_default();
            let canonical = |name: &str| Easing::parse(name).map(|e| e.to_string()).unwrap_or_default();
            let selected = EASINGS.iter().find(|(n, _)| canonical(n) == current).map(|(n, _)| *n).unwrap_or("custom");
            let keys = clip.keyframes.clone();
            let names: Vec<String> = here.iter().map(|(n, _)| n.clone()).collect();
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Into this keyframe"))
                    .child(segmented(
                        "easing",
                        EASINGS.iter().map(|(n, l)| (*n, SharedString::from(*l))).collect(),
                        selected,
                        move |easing, _, cx| {
                            let commands: Vec<Value> = names
                                .iter()
                                .map(|name| {
                                    let list: Vec<Value> = keys[name]
                                        .iter()
                                        .map(|k| {
                                            let e = if (k.time - local).abs() <= tol { easing.to_string() } else { k.easing.to_string() };
                                            json!({ "time": k.time, "value": k.value, "easing": e })
                                        })
                                        .collect();
                                    json!({ "command": "clip.setKeyframes", "params": { "clipId": id, "property": name, "keyframes": list } })
                                })
                                .collect();
                            cx.store().update(cx, |s, cx| s.run("project.batch", json!({ "commands": commands, "label": "Easing" }), cx));
                        },
                        cx,
                    )),
            );
        }

        // Presets.
        for (group, presets) in PRESET_GROUPS {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(group))
                    .child(div().flex().flex_wrap().gap(px(4.)).children(presets.iter().map(|&(preset, label)| {
                        Button::new(SharedString::from(format!("preset-{preset}")), label).small().on_click(move |_, _, cx| {
                            cx.store().update(cx, |s, cx| s.run("clip.animate", json!({ "clipIds": [id], "preset": preset }), cx))
                        })
                    }))),
            );
        }
        if !clip.keyframes.is_empty() {
            let names: Vec<String> = clip.keyframes.keys().cloned().collect();
            body = body.child(Button::new("clear-animation", "Clear animation").small().with_icon("x").on_click(move |_, _, cx| {
                let commands: Vec<Value> = names
                    .iter()
                    .map(|n| json!({ "command": "clip.removeKeyframe", "params": { "clipId": id, "property": n } }))
                    .collect();
                cx.store().update(cx, |s, cx| s.run("project.batch", json!({ "commands": commands, "label": "Clear animation" }), cx));
            }));
        }
        body.into_any_element()
    }

    /// The Motion section for a motion clip: template values and the scene's contents.
    pub(super) fn motion_section(&mut self, clip: &Clip, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let ClipContent::Motion { template, .. } = &clip.content else { return div().into_any_element() };
        let mut out = section(cx);
        if let Some(tref) = template
            && let Some(tpl) = templates::find(&tref.id)
        {
            self.sync_template(clip.id, tpl, tref, window, cx);
            out = out.child(caps(format!("Template · {}", tpl.name), cx)).child(div().text_size(px(sz::SM)).text_color(t.text_2).line_height(px(sz::SM * 1.45)).child(tpl.doc));
            let values = tpl.values(&tref.params).unwrap_or_default();
            let mut numbers = vec![];
            for p in tpl.params {
                let Some((_, field)) = self.template.fields.iter().find(|(n, _)| *n == p.name) else { continue };
                match field {
                    Field::Text(e) => out = out.child(labeled_field(p.name, e.clone().into_any_element(), cx)),
                    Field::Color(e) if p.kind == "colorOrNone" => {
                        // An optional colour: a switch, and the colour when it is on.
                        let (id, name) = (clip.id, p.name);
                        let on = !values.get(p.name).is_none_or(Value::is_null);
                        let default = serde_json::from_str::<Value>(p.default).ok().filter(|v| !v.is_null()).unwrap_or(json!("#000000cc"));
                        out = out.child(switch(SharedString::from(format!("tpl-{name}-on")), humanize(name), on, move |v, _, cx| {
                            set_template(id, name, if v { default.clone() } else { Value::Null }, cx)
                        }, cx));
                        if on {
                            out = out.child(e.clone());
                        }
                    }
                    Field::Color(e) => out = out.child(labeled_field(p.name, e.clone().into_any_element(), cx)),
                    Field::Number(e) => numbers.push(e.clone()),
                    Field::Plain => {
                        let id = clip.id;
                        let name = p.name;
                        if p.kind == "boolean" {
                            let on = values.get(p.name).and_then(Value::as_bool).unwrap_or(false);
                            out = out.child(switch(SharedString::from(format!("tpl-{name}")), humanize(name), on, move |v, _, cx| set_template(id, name, json!(v), cx), cx));
                        } else if let Some(choices) = p.kind.strip_prefix("choice:") {
                            let current = values.get(p.name).and_then(Value::as_str).unwrap_or("").to_string();
                            out = out.child(labeled_field(
                                p.name,
                                segmented(
                                    SharedString::from(format!("tpl-{name}")),
                                    choices.split('|').map(|c| (c.to_string(), SharedString::from(humanize(c)))).collect(),
                                    current,
                                    move |v: &String, _, cx| set_template(id, name, json!(v), cx),
                                    cx,
                                )
                                .into_any_element(),
                                cx,
                            ));
                        } else {
                            let shown = values.get(p.name).map(|v| v.to_string()).unwrap_or_default();
                            out = out.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(humanize(name)))
                                    .child(div().text_size(px(sz::XS)).text_color(t.text).font_family(crate::theme::MONO).child(shown)),
                            );
                        }
                    }
                }
            }
            if !numbers.is_empty() {
                out = out.child(grid2().children(numbers));
            }
        } else {
            self.template = TemplateFields::default();
        }
        out.into_any_element()
    }

    /// Makes (or fills) the fields of a template clip.
    fn sync_template(&mut self, clip: Id, tpl: &'static Template, tref: &TemplateRef, window: &mut Window, cx: &mut Context<Self>) {
        let values = tpl.values(&tref.params).unwrap_or_default();
        if self.template.clip != Some(clip) {
            let mut fields = vec![];
            let mut subs = vec![];
            for p in tpl.params {
                let name = p.name;
                let field = match p.kind {
                    "text" => {
                        let e = cx.new(|cx| TextInput::new(cx).placeholder(humanize(name)));
                        subs.push(cx.subscribe(&e, move |this: &mut Self, _, ev: &InputEvent, cx| {
                            if let InputEvent::Changed(text) = ev {
                                let v = json!(text);
                                this.template_change(name, v, Step::Live, cx);
                            }
                        }));
                        Field::Text(e)
                    }
                    "color" | "colorOrNone" => {
                        let e = cx.new(|cx| ColorField::new(None, cx));
                        subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ColorChange, cx| this.template_change(name, json!(ch.0), Step::Final, cx)));
                        Field::Color(e)
                    }
                    "number" => {
                        let e = cx.new(|_| Scrub::new(humanize(name), 0.05, 2).range(-1e9, 1e9));
                        subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ScrubChange, cx| this.template_change(name, json!(ch.value), ch.step(), cx)));
                        Field::Number(e)
                    }
                    _ => Field::Plain,
                };
                fields.push((name, field));
            }
            self.template = TemplateFields { clip: Some(clip), fields, _subs: subs };
        }
        for (name, field) in &self.template.fields {
            let v = values.get(*name).cloned().unwrap_or(Value::Null);
            match field {
                Field::Text(e) => {
                    if !e.read(cx).is_focused(window) {
                        let s = v.as_str().unwrap_or("").to_string();
                        e.update(cx, |i, cx| i.set_text(s, cx));
                    }
                }
                Field::Color(e) => {
                    if let Some(c) = v.as_str() {
                        e.update(cx, |f, cx| f.set_value(c, window, cx));
                    }
                }
                Field::Number(e) => {
                    if let Some(n) = v.as_f64() {
                        e.update(cx, |s, _| s.set_value(n));
                    }
                }
                Field::Plain => {}
            }
        }
    }

    fn template_change(&mut self, name: &'static str, value: Value, step: Step, cx: &mut Context<Self>) {
        let Some(id) = self.template.clip else { return };
        let mut p = json!({ "clipId": id, "values": { name: value } });
        if let Some(k) = step.coalesce(format!("{id}:tpl:{name}")) {
            p["coalesce"] = json!(k);
        }
        self.store.update(cx, |s, cx| s.run("motion.setTemplate", p, cx));
    }

    /// Forget the template fields (another kind of clip is shown).
    pub(super) fn clear_template(&mut self) {
        if self.template.clip.is_some() {
            self.template = TemplateFields::default();
        }
    }
}

fn set_template(id: Id, name: &'static str, value: Value, cx: &mut App) {
    cx.store().update(cx, |s, cx| s.run("motion.setTemplate", json!({ "clipId": id, "values": { name: value } }), cx));
}

/// `textColor` → "Text colour".
pub fn humanize(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(ch.to_lowercase());
        } else if i == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out.replace("color", "colour").replace("Color", "Colour")
}

fn labeled_field(name: &str, control: AnyElement, cx: &App) -> gpui::Div {
    let t = cx.theme();
    div().flex().flex_col().gap(px(4.)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child(humanize(name))).child(control)
}
