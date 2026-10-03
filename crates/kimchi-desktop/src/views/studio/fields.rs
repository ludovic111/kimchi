//! The Properties panel's fields: one widget per kind of value (number scrubs with their range,
//! colours, words, choices, switches, vectors, pictures, other things of the scene, point
//! lists), each with a keyframe toggle when it can be animated. Widgets keep their state in the
//! panel (by thing and property); every change is a command (drags fold into one undo step).

use std::collections::HashSet;
use std::rc::Rc;

use gpui::{AnyElement, App, Context, Entity, SharedString, Window, div, prelude::*, px};
use kimchi_core::{Id, KeyValue, Keyframes, Scene};
use serde_json::{Map, Value, json};

use super::model;
use super::properties::{Properties, Widget};
use super::specs::{F, Fk};
use crate::store::MenuItem;
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::{Button, segmented, switch};
use crate::views::inspector::color::{ColorChange, ColorField};

/// Where a field's changes go.
#[derive(Clone, Debug, PartialEq)]
pub enum Tk {
    /// `motion.updateLayer` on a thing (or `"scene"`, `"camera"`).
    Item,
    /// A stack item's parameter (always by its keyframe name, booleans as 1/0).
    Param,
    /// A shared material (`motion.setMaterial`).
    Material(String),
    /// A composition's settings (`motion.setComposition`).
    Composition(String),
    /// The whole thing through `motion.setLayer` (for the few names `updateLayer` reads
    /// otherwise, like a 3D particle system's colour).
    Replace,
}

/// What every field of one section needs to read and write.
#[derive(Clone)]
pub struct FieldCtx {
    pub clip: Id,
    /// The thing (`"scene"` for the scene).
    pub id: String,
    pub target: Tk,
    /// The widgets' cache prefix.
    pub scope: String,
    pub scene: Rc<Scene>,
    /// Scene time and timeline time.
    pub t: f64,
    pub playhead: f64,
    pub keys: Rc<Keyframes>,
    /// Animatable property names of the thing.
    pub anim: Rc<HashSet<String>>,
    /// What is read for settings (the thing's, material's or composition's JSON).
    pub json: Rc<Value>,
}

/// Vectors whose components are keyframed one by one (`position.x`).
const COMPONENT_VECTORS: &[&str] = &["position", "rotation", "scale", "size", "target", "direction"];

impl FieldCtx {
    pub fn animatable(&self, name: &str) -> bool {
        self.anim.contains(name)
    }

    /// The field's value now: animated values at the playhead, settings from the JSON, or the
    /// default.
    pub fn read(&self, f: &F) -> Value {
        let from_json = || {
            let mut v: &Value = &self.json;
            for part in f.name.split('.') {
                v = v.get(part)?;
            }
            Some(v.clone())
        };
        let animated = match self.target {
            Tk::Item | Tk::Param | Tk::Replace => model::value_at(&self.scene, &self.id, f.name, self.t).map(|kv| match kv {
                KeyValue::Number(n) => json!(n),
                KeyValue::Vector(v) => json!(v),
                KeyValue::Text(s) => json!(s),
            }),
            _ => None,
        };
        let v = match f.kind {
            // Settings first: `value_at` gives a default for some that aren't set.
            Fk::OptColor | Fk::OptNum(..) | Fk::Asset | Fk::Sibling | Fk::Points(_) | Fk::Choice(_) | Fk::Bool | Fk::Path => from_json().or(animated),
            _ => animated.or_else(from_json),
        };
        match v {
            Some(Value::Null) | None => serde_json::from_str(f.def).unwrap_or(Value::Null),
            Some(v) => v,
        }
    }

    /// Is there a keyframe of `name` at the playhead? Is `name` animated at all?
    pub fn key_state(&self, name: &str) -> (bool, bool) {
        match self.keys.get(name) {
            Some(list) => (list.iter().any(|k| (k.time - self.t).abs() < 1e-3 + 0.5 / 60.0), true),
            None => (false, false),
        }
    }

    /// The keyframe name of one component of a vector field.
    pub fn component(&self, base: &str, axis: usize) -> Option<String> {
        COMPONENT_VECTORS.contains(&base).then(|| model::component_name(&self.keys, base, axis))
    }

    fn coalesce(&self, name: &str) -> String {
        format!("studio-field:{}:{}:{name}", self.clip, self.id)
    }
}

/// `{ "a": { "b": v } }` for `a.b`.
fn nested(name: &str, v: Value) -> Map<String, Value> {
    let mut parts: Vec<&str> = name.split('.').collect();
    let last = parts.pop().unwrap_or(name);
    let mut inner = Map::new();
    inner.insert(last.to_string(), v);
    let mut out = inner;
    while let Some(p) = parts.pop() {
        let mut m = Map::new();
        m.insert(p.to_string(), Value::Object(out));
        out = m;
    }
    out
}

impl Properties {
    /// Sends one field's new value.
    pub fn write(&mut self, c: &FieldCtx, name: &str, v: Value, final_: bool, cx: &mut Context<Self>) {
        let clip = c.clip;
        let mut p = match &c.target {
            Tk::Item => {
                let props = if name == "environment.type" && v == "none" {
                    nested("environment", Value::Null)
                } else if name == "matte.layer" && v.is_null() {
                    nested("matte", Value::Null)
                } else if c.animatable(name) || !name.contains('.') {
                    let mut m = Map::new();
                    m.insert(name.to_string(), v);
                    m
                } else {
                    nested(name, v)
                };
                json!({ "clipId": clip, "id": c.id, "props": props, "time": c.playhead })
            }
            Tk::Param => {
                let v = match v {
                    Value::Bool(b) => json!(if b { 1.0 } else { 0.0 }),
                    other => other,
                };
                json!({ "clipId": clip, "id": c.id, "props": { name: v }, "time": c.playhead })
            }
            Tk::Material(id) => {
                let mut m = (*c.json).clone();
                set_path(&mut m, name, v);
                m["id"] = json!(id);
                json!({ "clipId": clip, "material": m })
            }
            Tk::Composition(id) => {
                let mut m = Map::new();
                m.insert("id".into(), json!(id));
                m.insert(name.into(), v);
                json!({ "clipId": clip, "composition": m })
            }
            Tk::Replace => {
                let mut m = c.scene.item_json(&c.id).unwrap_or(Value::Null);
                set_path(&mut m, name, v);
                json!({ "clipId": clip, "layer": m })
            }
        };
        if !final_ {
            p["coalesce"] = json!(c.coalesce(name));
        }
        let command = match c.target {
            Tk::Item | Tk::Param => "motion.updateLayer",
            Tk::Material(_) => "motion.setMaterial",
            Tk::Composition(_) => "motion.setComposition",
            Tk::Replace => "motion.setLayer",
        };
        let _studio = self.studio.clone();
        super::run(command, p, cx);
    }

    /// One component of a vector field changed.
    fn write_component(&mut self, c: &FieldCtx, f: &F, axis: usize, n: f64, final_: bool, cx: &mut Context<Self>) {
        let mut v: Vec<f64> = c.read(f).as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let len = if matches!(f.kind, Fk::Vec2(..)) { 2 } else { 3 };
        if v.len() == 1 {
            v = vec![v[0]; len];
        }
        v.resize(len, 0.0);
        v[axis] = n;
        match c.component(f.name, axis).filter(|_| c.target == Tk::Item) {
            Some(name) if name != f.name => self.write(c, &name, json!(n), final_, cx),
            _ => self.write(c, f.name, json!(v), final_, cx),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scrub(&mut self, key: &str, label: &str, step: f64, decimals: usize, min: f64, max: f64, on: impl Fn(&mut Self, f64, bool, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Entity<Scrub> {
        self.used.insert(key.to_string());
        if let Some(Widget::Scrub(e, _)) = self.widgets.get(key) {
            return e.clone();
        }
        let label = label.to_string();
        let e = cx.new(|_| Scrub::new(label, step, decimals).range(min, max));
        let sub = cx.subscribe(&e, move |this: &mut Self, _, ch: &ScrubChange, cx| on(this, ch.value, ch.final_, cx));
        self.widgets.insert(key.to_string(), Widget::Scrub(e.clone(), sub));
        e
    }

    fn color(&mut self, key: &str, on: impl Fn(&mut Self, String, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Entity<ColorField> {
        self.used.insert(key.to_string());
        if let Some(Widget::Color(e, _)) = self.widgets.get(key) {
            return e.clone();
        }
        let e = cx.new(|cx| {
            let mut c = ColorField::new(None, cx);
            c.compact = true;
            c
        });
        let sub = cx.subscribe(&e, move |this: &mut Self, _, ch: &ColorChange, cx| on(this, ch.0.clone(), cx));
        self.widgets.insert(key.to_string(), Widget::Color(e.clone(), sub));
        e
    }

    /// A text field: `live` sends while typing, else when done (Enter or leaving it).
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, key: &str, lines: usize, mono: bool, live: bool, placeholder: &str, on: impl Fn(&mut Self, String, bool, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Entity<TextInput> {
        self.used.insert(key.to_string());
        if let Some(Widget::Text(e, _)) = self.widgets.get(key) {
            return e.clone();
        }
        let ph = placeholder.to_string();
        let e = cx.new(|cx| {
            let mut i = TextInput::new(cx).placeholder(ph);
            if lines > 1 {
                i = i.multiline(lines);
            }
            i.mono = mono;
            i
        });
        let sub = cx.subscribe(&e, move |this: &mut Self, input, ev: &InputEvent, cx| match ev {
            InputEvent::Changed(t) if live => on(this, t.clone(), false, cx),
            InputEvent::Submit | InputEvent::Blur if !live => {
                let t = input.read(cx).text().to_string();
                on(this, t, true, cx)
            }
            _ => {}
        });
        self.widgets.insert(key.to_string(), Widget::Text(e.clone(), sub));
        e
    }

    /// The keyframe toggle of an animatable property.
    fn diamond(&self, c: &FieldCtx, name: &str, cx: &App) -> AnyElement {
        if !c.animatable(name) || !matches!(c.target, Tk::Item | Tk::Param) {
            return div().w(px(22.)).flex_none().into_any_element();
        }
        let t = cx.theme();
        let (here, animated) = c.key_state(name);
        let (clip, id, prop, time) = (c.clip, c.id.clone(), name.to_string(), c.playhead);
        let _studio = self.studio.clone();
        div()
            .id(SharedString::from(format!("key-{}-{name}", c.scope)))
            .flex_none()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .text_color(if here { t.accent } else if animated { t.accent_text } else { t.text_3 })
            .child(if here { crate::ui::icon("diamond").size(px(12.)) } else { crate::ui::icon("diamond").size(px(10.)) })
            .when(here, |d| d.child(div().absolute().size(px(6.)).rounded(px(1.)).bg(t.accent)))
            .relative()
            .tooltip(move |_, cx| {
                crate::ui::tooltip(
                    if here {
                        "Remove the keyframe here"
                    } else if animated {
                        "Add a keyframe here"
                    } else {
                        "Animate: a first keyframe at the playhead"
                    }
                    .into(),
                    cx,
                )
            })
            .on_click(move |_, _, cx| {
                let command = if here { "motion.removeKeyframe" } else { "motion.addKeyframe" };
                super::run(command, json!({ "clipId": clip, "id": id, "property": prop, "time": time }), cx);
            })
            .into_any_element()
    }

    fn row(label: &str, control: AnyElement, diamond: AnyElement, cx: &App) -> AnyElement {
        let t = cx.theme();
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .min_h(px(26.))
            .child(div().w(px(104.)).flex_none().text_size(px(sz::XS)).text_color(t.text_2).child(label.to_string()))
            .child(div().flex_1().min_w_0().overflow_hidden().child(control))
            .child(diamond)
            .into_any_element()
    }

    /// One field, its widget and its keyframe toggle.
    pub fn field(&mut self, c: &FieldCtx, f: &F, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("{}|{}", c.scope, f.name);
        let value = c.read(f);
        let name = f.name;
        match f.kind {
            Fk::Num(step, dec, min, max) | Fk::OptNum(step, dec, min, max) => {
                let opt = matches!(f.kind, Fk::OptNum(..));
                let on = !(opt && value.is_null());
                let cc = c.clone();
                let e = self.scrub(&key, f.label, step, dec, min, max, move |this, v, fin, cx| this.write(&cc, name, json!(v), fin, cx), cx);
                if let Some(n) = value.as_f64().or_else(|| value.as_array().and_then(|a| a.first()).and_then(Value::as_f64)) {
                    e.update(cx, |s, _| s.set_value(n));
                }
                let diamond = self.diamond(c, name, cx);
                if opt {
                    let (cc, def) = (c.clone(), if name == "end" || name == "emitUntil" { (c.t + 1.0).max(1.0) } else { 1.0 });
                    let this = cx.entity();
                    let sw = switch(SharedString::from(format!("opt-{key}")), "", on, move |v, _, cx| {
                        let cc = cc.clone();
                        this.update(cx, |p, cx| p.write(&cc, name, if v { json!(def) } else { Value::Null }, true, cx))
                    }, cx);
                    let body = div().flex().items_center().gap(px(6.)).child(sw).when(on, |d| d.child(div().flex_1().child(e))).when(!on, |d| d.child(div().text_size(px(sz::XS)).text_color(cx.theme().text_3).child("Off")));
                    return Self::row(f.label, body.into_any_element(), diamond, cx);
                }
                div().flex().items_center().gap(px(4.)).child(div().flex_1().min_w_0().child(e)).child(diamond).into_any_element()
            }
            Fk::Color | Fk::OptColor => {
                let cc = c.clone();
                let e = self.color(&key, move |this, v, cx| this.write(&cc, name, json!(v), true, cx), cx);
                let on = !(f.kind == Fk::OptColor && value.is_null());
                if let Some(s) = value.as_str() {
                    let s = s.to_string();
                    e.update(cx, |cf, cx| cf.set_value(&s, window, cx));
                }
                let diamond = self.diamond(c, name, cx);
                let control = if f.kind == Fk::OptColor {
                    let cc = c.clone();
                    let this = cx.entity();
                    let sw = switch(SharedString::from(format!("opt-{key}")), "", on, move |v, _, cx| {
                        let cc = cc.clone();
                        this.update(cx, |p, cx| p.write(&cc, name, if v { json!("#ffffff") } else { Value::Null }, true, cx))
                    }, cx);
                    div().flex().items_center().gap(px(6.)).child(sw).when(on, |d| d.child(div().flex_1().child(e))).into_any_element()
                } else {
                    e.into_any_element()
                };
                Self::row(f.label, control, diamond, cx)
            }
            Fk::Text | Fk::Long | Fk::Path => {
                let live = f.kind != Fk::Path;
                let cc = c.clone();
                let lines = if f.kind == Fk::Long { 2 } else { 1 };
                let kind = f.kind;
                let e = self.text(&key, lines, f.kind == Fk::Path, live, f.label, move |this, v, fin, cx| {
                    // An empty path is no path (text on a path, masks).
                    let v = if v.trim().is_empty() && kind == Fk::Path { Value::Null } else { json!(v) };
                    this.write(&cc, name, v, fin, cx)
                }, cx);
                if !e.read(cx).is_focused(window) {
                    let s = value.as_str().unwrap_or("").to_string();
                    if e.read(cx).text() != s {
                        e.update(cx, |i, cx| i.set_text(s, cx));
                    }
                }
                let diamond = self.diamond(c, name, cx);
                Self::row(f.label, e.into_any_element(), diamond, cx)
            }
            Fk::Bool => {
                let on = value.as_bool().unwrap_or_else(|| value.as_f64().is_some_and(|n| n >= 0.5));
                let cc = c.clone();
                let this = cx.entity();
                let sw = switch(SharedString::from(format!("b-{key}")), "", on, move |v, _, cx| {
                    let cc = cc.clone();
                    this.update(cx, |p, cx| p.write(&cc, name, json!(v), true, cx))
                }, cx);
                // Like every other row: the label on the left, the switch at the end.
                Self::row(f.label, div().flex().justify_end().child(sw).into_any_element(), div().w(px(22.)).into_any_element(), cx)
            }
            Fk::Choice(options) => {
                let current = value.as_str().unwrap_or("").to_string();
                let cc = c.clone();
                let this = cx.entity();
                let control = if options.len() <= 3 && options.iter().all(|o| o.len() <= 12) {
                    segmented(
                        SharedString::from(format!("c-{key}")),
                        options.iter().map(|o| (o.to_string(), SharedString::from(crate::views::inspector::animation::humanize(o)))).collect(),
                        current,
                        move |v: &String, _, cx| {
                            let (cc, v) = (cc.clone(), v.clone());
                            this.update(cx, |p, cx| p.write(&cc, name, json!(v), true, cx))
                        },
                        cx,
                    )
                    .into_any_element()
                } else {
                    let items: Vec<(String, String)> = options.iter().map(|o| (o.to_string(), crate::views::inspector::animation::humanize(o))).collect();
                    let shown = items.iter().find(|(o, _)| *o == current).map(|(_, l)| l.clone()).unwrap_or(current.clone());
                    dropdown(&key, shown, move |_cx| {
                        items
                            .iter()
                            .map(|(o, l)| {
                                let (cc, o, this) = (cc.clone(), o.clone(), this.clone());
                                MenuItem::new(l.clone(), move |_, cx| {
                                    let (cc, o) = (cc.clone(), o.clone());
                                    this.update(cx, |p, cx| p.write(&cc, name, json!(o), true, cx))
                                })
                                .entry()
                            })
                            .collect()
                    }, cx)
                };
                let diamond = self.diamond(c, name, cx);
                Self::row(f.label, control, diamond, cx)
            }
            Fk::Vec3(step, dec) | Fk::Vec2(step, dec) => {
                let n = if matches!(f.kind, Fk::Vec2(..)) { 2 } else { 3 };
                let v: Vec<f64> = match &value {
                    Value::Array(a) => a.iter().filter_map(Value::as_f64).collect(),
                    Value::Number(x) => vec![x.as_f64().unwrap_or(0.0); n],
                    _ => vec![0.0; n],
                };
                let mut row = div().flex().flex_col().gap(px(3.)).child(div().text_size(px(sz::XS)).text_color(cx.theme().text_2).child(f.label.to_string()));
                for axis in 0..n {
                    let cc = c.clone();
                    let f2 = *f;
                    let label = ["X", "Y", "Z"][axis];
                    let e = self.scrub(&format!("{key}|{axis}"), label, step, dec, -1e9, 1e9, move |this, x, fin, cx| this.write_component(&cc, &f2, axis, x, fin, cx), cx);
                    if let Some(x) = v.get(axis).copied().or(v.first().copied()) {
                        e.update(cx, |s, _| s.set_value(x));
                    }
                    let comp = c.component(f.name, axis).unwrap_or_else(|| f.name.to_string());
                    let diamond = if c.component(f.name, axis).is_some() || axis == 0 { self.diamond(c, &comp, cx) } else { div().w(px(22.)).into_any_element() };
                    row = row.child(div().flex().items_center().gap(px(4.)).child(div().flex_1().child(e)).child(diamond));
                }
                row.into_any_element()
            }
            Fk::Asset | Fk::Sibling | Fk::Composition | Fk::Camera => {
                let current = value.as_str().unwrap_or("").to_string();
                let options = self.options(c, f.kind, cx);
                let shown = options.iter().find(|(v, _)| v.as_str() == Some(current.as_str())).map(|(_, l)| l.clone()).unwrap_or_else(|| if current.is_empty() { "None".into() } else { current.clone() });
                let cc = c.clone();
                let this = cx.entity();
                let control = dropdown(&key, shown, move |_| {
                    options
                        .iter()
                        .map(|(v, l)| {
                            let (cc, v, this) = (cc.clone(), v.clone(), this.clone());
                            MenuItem::new(l.clone(), move |_, cx| {
                                let (cc, v) = (cc.clone(), v.clone());
                                this.update(cx, |p, cx| p.write(&cc, name, v, true, cx))
                            })
                            .entry()
                        })
                        .collect()
                }, cx);
                Self::row(f.label, control, div().w(px(22.)).into_any_element(), cx)
            }
            Fk::Points(dims) => self.points(c, f, &value, dims, cx),
        }
    }

    /// Choices for pictures, layers next to it, compositions and cameras: (value, label).
    fn options(&self, c: &FieldCtx, kind: Fk, cx: &App) -> Vec<(Value, String)> {
        let mut out = vec![(Value::Null, "None".to_string())];
        match kind {
            Fk::Asset => {
                if let Some(p) = self.studio.read(cx).project(cx) {
                    for a in p.assets.iter().filter(|a| a.kind != kimchi_core::MediaKind::Audio) {
                        out.push((json!(a.id.to_string()), a.name.clone()));
                    }
                }
            }
            Fk::Sibling => {
                if let Scene::Flat(s) = &*c.scene
                    && let Some(list) = s.siblings(&c.id)
                {
                    out.extend(list.iter().filter(|l| l.id != c.id).map(|l| (json!(l.id), l.id.clone())));
                }
            }
            Fk::Composition => {
                out.clear();
                if let Scene::Flat(s) = &*c.scene {
                    out.extend(s.compositions.iter().map(|k| (json!(k.id), k.id.clone())));
                }
            }
            Fk::Camera => {
                out.clear();
                if let Scene::Space(s) = &*c.scene {
                    out.push((json!("camera"), "camera".into()));
                    out.extend(s.cameras.iter().map(|k| (json!(k.id), k.id.clone())));
                }
            }
            _ => {}
        }
        out
    }

    /// A list of points: a row of numbers each, with remove, and add at the end.
    fn points(&mut self, c: &FieldCtx, f: &F, value: &Value, dims: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let pts: Vec<Vec<f64>> = value.as_array().map(|a| a.iter().map(|p| p.as_array().map(|q| q.iter().filter_map(Value::as_f64).collect()).unwrap_or_default()).collect()).unwrap_or_default();
        let name = f.name;
        let mut col = div().flex().flex_col().gap(px(3.)).child(div().text_size(px(sz::XS)).text_color(t.text_2).child(format!("{} · {}", f.label, pts.len())));
        for (i, p) in pts.iter().enumerate() {
            let mut row = div().flex().items_center().gap(px(3.)).child(div().w(px(16.)).font_family(MONO).text_size(px(10.)).text_color(t.text_3).child(format!("{i}")));
            for axis in 0..dims {
                let (cc, all) = (c.clone(), pts.clone());
                let e = self.scrub(&format!("{}|{name}|{i}|{axis}", c.scope), ["X", "Y", "Z"][axis], 0.05, 2, -1e6, 1e6, move |this, v, fin, cx| {
                    let mut all = all.clone();
                    if let Some(q) = all.get_mut(i) {
                        q.resize(dims, 0.0);
                        q[axis] = v;
                    }
                    this.write(&cc, name, json!(all), fin, cx)
                }, cx);
                e.update(cx, |s, _| s.set_value(p.get(axis).copied().unwrap_or(0.0)));
                row = row.child(div().flex_1().min_w_0().child(e));
            }
            let (cc, all) = (c.clone(), pts.clone());
            let this = cx.entity();
            row = row.child(Button::icon(SharedString::from(format!("rm-{}-{name}-{i}", c.scope)), "x", "Remove this point").small().disabled(pts.len() <= 2).on_click(move |_, _, cx| {
                let mut all = all.clone();
                all.remove(i);
                let cc = cc.clone();
                this.update(cx, |p, cx| p.write(&cc, name, json!(all), true, cx))
            }));
            col = col.child(row);
        }
        let (cc, all) = (c.clone(), pts.clone());
        let this = cx.entity();
        col = col.child(Button::new(SharedString::from(format!("add-{}-{name}", c.scope)), "Add a point").small().with_icon("plus").on_click(move |_, _, cx| {
            let mut all = all.clone();
            // After the last, one step further along the last segment.
            let next = match (all.len(), all.last().cloned()) {
                (n, Some(last)) if n >= 2 => {
                    let prev = all[n - 2].clone();
                    last.iter().zip(prev.iter()).map(|(a, b)| a + (a - b)).collect()
                }
                (_, Some(last)) => last.iter().map(|v| v + 0.5).collect(),
                _ => vec![0.0; dims],
            };
            all.push(next);
            let cc = cc.clone();
            this.update(cx, |p, cx| p.write(&cc, name, json!(all), true, cx))
        }));
        col.into_any_element()
    }
}

/// A button showing the current choice, opening a menu of the others.
pub fn dropdown(key: &str, shown: String, entries: impl Fn(&mut App) -> Vec<crate::store::MenuEntry> + 'static, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .id(SharedString::from(format!("dd-{key}")))
        .h(px(26.))
        .px(px(8.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.))
        .rounded(px(sz::R_SM))
        .border_1()
        .border_color(t.line)
        .bg(t.bg_sunken.opacity(0.6))
        .cursor_pointer()
        .hover(|s| s.border_color(t.accent_ring))
        .text_size(px(sz::SM))
        .child(div().min_w_0().truncate().child(shown))
        .child(crate::ui::icon("chevron-down").size(px(12.)).text_color(t.text_2))
        .on_click(move |e, _, cx| {
            let list = entries(cx);
            super::menus::open_menu(e.position(), list, cx);
        })
        .into_any_element()
}

/// Sets `a.b.c` in a JSON object (making the objects on the way).
pub fn set_path(v: &mut Value, name: &str, x: Value) {
    let parts: Vec<&str> = name.split('.').collect();
    let mut at = v;
    for p in &parts[..parts.len() - 1] {
        if !at.get(*p).is_some_and(Value::is_object) {
            at[*p] = json!({});
        }
        at = &mut at[*p];
    }
    at[parts[parts.len() - 1]] = x;
}

