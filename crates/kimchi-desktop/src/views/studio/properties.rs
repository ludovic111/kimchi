//! The Properties panel, in tabs like Blender's: the selected thing's fields (transform, its
//! shape or layer kind, looks, links, timing), its stacks (modifiers, constraints, effects,
//! shape operators, masks, text animators: built from the stack tables), its material, its
//! expressions; the world and render settings of a 3D scene; a 2D scene's or composition's.

use std::collections::{HashMap, HashSet};

use gpui::{AnyElement, App, Context, Entity, FontWeight, MouseButton, Render, SharedString, Subscription, Window, div, prelude::*, px};
use kimchi_core::Scene;
use kimchi_core::motion::stack::{ParamKind, TypeSpec};
use serde_json::{Value, json};

use super::fields::{FieldCtx, Tk};
use super::model::{self, Item};
use super::specs::{self, F, Fk, Section};
use super::{Area, Popover, Studio};
use crate::store::StoreExt;
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::TextInput;
use crate::ui::scrub::Scrub;
use crate::ui::{Button, caps, icon};
use crate::views::inspector::color::ColorField;

/// A field's widget and the subscription that sends its changes (kept as long as it is).
#[allow(dead_code)]
pub enum Widget {
    Scrub(Entity<Scrub>, Subscription),
    Color(Entity<ColorField>, Subscription),
    Text(Entity<TextInput>, Subscription),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Item,
    Modifiers,
    Constraints,
    Effects,
    Masks,
    Operators,
    Animators,
    Material,
    Expressions,
    World,
    Render,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Item => "Item",
            Tab::Modifiers => "Modifiers",
            Tab::Constraints => "Constraints",
            Tab::Effects => "Effects",
            Tab::Masks => "Masks",
            Tab::Operators => "Operators",
            Tab::Animators => "Animators",
            Tab::Material => "Material",
            Tab::Expressions => "Expressions",
            Tab::World => "World",
            Tab::Render => "Render",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Item => "square",
            Tab::Modifiers => "wrench",
            Tab::Constraints => "link",
            Tab::Effects => "sparkles",
            Tab::Masks => "scan",
            Tab::Operators => "repeat",
            Tab::Animators => "type",
            Tab::Material => "palette",
            Tab::Expressions => "code",
            Tab::World => "globe",
            Tab::Render => "camera",
        }
    }

    /// The stack field the tab shows.
    pub fn field(self) -> Option<&'static str> {
        Some(match self {
            Tab::Modifiers => "modifiers",
            Tab::Constraints => "constraints",
            Tab::Effects => "effects",
            Tab::Masks => "masks",
            Tab::Operators => "operators",
            Tab::Animators => "animators",
            _ => return None,
        })
    }
}

pub struct Properties {
    pub studio: Entity<Studio>,
    pub tab: Tab,
    pub widgets: HashMap<String, Widget>,
    pub used: HashSet<String>,
    /// Stack items folded closed (by thing/field/id).
    folded: HashSet<String>,
    /// Expression errors, by property, from the last try.
    expr_errors: HashMap<String, String>,
    /// A property picked to get a new expression.
    expr_new: Option<String>,
    focus_text_next: bool,
    _subs: Vec<Subscription>,
}

/// The tabs a selected thing has.
fn tabs_for(scene: &Scene, item: Option<&Item>) -> Vec<Tab> {
    match (scene, item) {
        (Scene::Space(_), None | Some(Item::Scene)) => vec![Tab::World, Tab::Render],
        (Scene::Space(_), Some(Item::Object(_))) => vec![Tab::Item, Tab::Modifiers, Tab::Constraints, Tab::Material, Tab::Expressions],
        (Scene::Space(_), Some(Item::Light | Item::Camera)) => vec![Tab::Item, Tab::Constraints, Tab::Expressions],
        (_, Some(Item::Material(_) | Item::Composition(_))) => vec![Tab::Item],
        (Scene::Flat(_), None | Some(Item::Scene)) => vec![Tab::Item],
        (Scene::Flat(_), Some(Item::Layer(kind))) => {
            let mut v = vec![Tab::Item, Tab::Effects, Tab::Masks];
            if matches!(*kind, "rect" | "ellipse" | "polygon" | "star" | "path" | "group") {
                v.push(Tab::Operators);
            }
            if *kind == "text" {
                v.push(Tab::Animators);
            }
            v.push(Tab::Expressions);
            v
        }
        _ => vec![Tab::Item],
    }
}

impl Properties {
    pub fn new(studio: Entity<Studio>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![cx.observe(&studio, |_, _, cx| cx.notify()), cx.observe(&store, |_, _, cx| cx.notify()), cx.observe(&playback, |_, _, cx| cx.notify())];
        Self { studio, tab: Tab::Item, widgets: HashMap::new(), used: HashSet::new(), folded: HashSet::new(), expr_errors: HashMap::new(), expr_new: None, focus_text_next: false, _subs: subs }
    }

    /// After adding a text layer: put the cursor in its words.
    pub fn focus_text(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Item;
        self.focus_text_next = true;
        cx.notify();
    }

    #[cfg(test)]
    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// A field's widget, by thing and property (tests drive them).
    #[cfg(test)]
    pub fn scrub_of(&self, key: &str) -> Option<Entity<Scrub>> {
        match self.widgets.get(key) {
            Some(Widget::Scrub(e, _)) => Some(e.clone()),
            _ => None,
        }
    }

    fn ctx(&self, clip: kimchi_core::Id, scene: &Scene, id: &str, target: Tk, scope: String, json: Value, cx: &App) -> FieldCtx {
        let st = self.studio.read(cx);
        let t = st.scene_time(cx);
        let mut anim: HashSet<String> = model::prop_names(scene, id).into_iter().collect();
        // A vector animatable as a whole is animatable by component too.
        for base in ["position", "rotation", "scale", "target", "direction", "size"] {
            if anim.contains(base) {
                for a in ["x", "y", "z"] {
                    anim.insert(format!("{base}.{a}"));
                }
            }
        }
        FieldCtx {
            clip,
            id: id.to_string(),
            target,
            scope,
            scene: std::rc::Rc::new(scene.clone()),
            t,
            playhead: st.playhead(cx),
            keys: std::rc::Rc::new(model::keyframes(scene, id).unwrap_or_default()),
            anim: std::rc::Rc::new(anim),
            json: std::rc::Rc::new(json),
        }
    }

    fn section(title: &str, body: Vec<AnyElement>, cx: &App) -> AnyElement {
        div().flex().flex_col().gap(px(4.)).px(px(12.)).py(px(10.)).border_b_1().border_color(cx.theme().line).child(div().pb(px(2.)).child(caps(title.to_string(), cx))).children(body).into_any_element()
    }

    fn sections(&mut self, c: &FieldCtx, list: Vec<Section>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        for s in list {
            if s.fields.is_empty() {
                continue;
            }
            let body: Vec<AnyElement> = s.fields.iter().map(|f| self.field(c, f, window, cx)).collect();
            out.push(Self::section(s.title, body, cx));
        }
        out
    }

    // ---- the tabs -------------------------------------------------------------------------------

    fn item_tab(&mut self, clip: kimchi_core::Id, scene: &Scene, key: &str, item: &Item, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.theme().clone();
        match item {
            Item::Object(shape) => {
                let c = self.ctx(clip, scene, key, Tk::Item, key.to_string(), scene.item_json(key).unwrap_or_default(), cx);
                let mut list = vec![specs::object_transform()];
                let mut shape_fields = specs::shape_fields(shape);
                let mut out = vec![];
                // A 3D particle system's colour is its own (not the material's).
                if *shape == "particles" {
                    let cr = self.ctx(clip, scene, key, Tk::Replace, format!("{key}#r"), scene.item_json(key).unwrap_or_default(), cx);
                    let (colors, rest): (Vec<F>, Vec<F>) = shape_fields.into_iter().partition(|f| f.name == "color");
                    shape_fields = rest;
                    out.extend(self.sections(&cr, vec![Section { title: "Particle colour", fields: colors }], window, cx));
                }
                list.push(Section { title: shape_title(shape), fields: shape_fields });
                list.push(specs::object_visibility());
                let mut body = self.sections(&c, list, window, cx);
                body.extend(out);
                if *shape == "mesh"
                    && let Scene::Space(s) = scene
                    && let Some(kimchi_core::motion::Shape3d::Mesh { vertices, faces, .. }) = kimchi_core::motion::find_object(&s.objects, key).map(|o| &o.shape)
                {
                    let studio = self.studio.clone();
                    body.insert(
                        1,
                        Self::section(
                            "Editing",
                            vec![
                                div().text_size(px(sz::SM)).text_color(t.text_2).child(format!("{} vertices · {} faces", vertices.len(), faces.len())).into_any_element(),
                                Button::new("edit-mesh", "Edit the mesh").small().with_icon("hexagon").tooltip(crate::actions::tip("Edit mode", &crate::actions::StudioToggleEdit)).on_click(move |_, _, cx| studio.update(cx, |s, cx| s.toggle_edit(cx))).into_any_element(),
                            ],
                            cx,
                        ),
                    );
                } else if !matches!(*shape, "group" | "particles" | "model" | "image") {
                    let studio = self.studio.clone();
                    let id = key.to_string();
                    body.push(Self::section("Modelling", vec![Button::new("to-mesh", "Convert to an editable mesh").small().with_icon("hexagon").on_click(move |_, _, cx| {
                        let clip = studio.read(cx).clip;
                        super::run("motion.convertToMesh", json!({ "clipId": clip, "id": id }), cx)
                    }).into_any_element()], cx));
                }
                body
            }
            Item::Light => {
                let kind = match scene {
                    Scene::Space(s) => s.lights.iter().find(|l| l.id == key).map(|l| l.kind.clone()).unwrap_or_default(),
                    _ => String::new(),
                };
                let c = self.ctx(clip, scene, key, Tk::Item, key.to_string(), scene.item_json(key).unwrap_or_default(), cx);
                self.sections(&c, specs::light_fields(&kind), window, cx)
            }
            Item::Camera => {
                let c = self.ctx(clip, scene, key, Tk::Item, key.to_string(), scene.item_json(key).unwrap_or_default(), cx);
                let mut body = self.sections(&c, specs::camera_fields(), window, cx);
                body.push(self.camera_actions(clip, scene, key, cx));
                body
            }
            Item::Scene => match scene {
                Scene::Flat(_) => {
                    let c = self.ctx(clip, scene, "scene", Tk::Item, "scene".into(), scene.to_json(), cx);
                    self.sections(&c, specs::scene2d_fields(), window, cx)
                }
                Scene::Space(_) => vec![],
            },
            Item::Layer(kind) => {
                let c = self.ctx(clip, scene, key, Tk::Item, key.to_string(), scene.item_json(key).unwrap_or_default(), cx);
                let mut list = vec![specs::layer_transform(), Section { title: layer_title(kind), fields: specs::kind_fields(kind) }];
                list.extend(specs::layer_style(kind));
                let mut body = self.sections(&c, list, window, cx);
                if *kind == "comp"
                    && let Scene::Flat(f) = scene
                    && let Some(kimchi_core::motion::LayerKind::Comp { comp, .. }) = f.find_layer(key).map(|l| &l.kind)
                {
                    let (studio, comp) = (self.studio.clone(), comp.clone());
                    body.insert(1, Self::section("Composition", vec![Button::new("open-comp", format!("Open “{comp}”")).small().with_icon("layers").on_click(move |_, _, cx| studio.update(cx, |s, cx| {
                        s.composition = Some(comp.clone());
                        s.set_selection(vec![], cx);
                    })).into_any_element()], cx));
                }
                if self.focus_text_next && *kind == "text" {
                    self.focus_text_next = false;
                    if let Some(Widget::Text(e, _)) = self.widgets.get(&format!("{key}|text")) {
                        crate::ui::input::focus(e, window, cx);
                        e.update(cx, |i, cx| i.select_all_text(cx));
                    }
                }
                body
            }
            Item::Material(id) => self.shared_material(clip, scene, id, window, cx),
            Item::Composition(id) => {
                let json = match scene {
                    Scene::Flat(f) => f.composition(id).map(|c| serde_json::to_value(c).unwrap_or_default()).unwrap_or_default(),
                    _ => Value::Null,
                };
                let c = self.ctx(clip, scene, "scene", Tk::Composition(id.clone()), format!("comp:{id}"), json, cx);
                let mut body = self.sections(&c, vec![Section { title: "Composition", fields: specs::composition_fields() }], window, cx);
                let (studio, id2) = (self.studio.clone(), id.clone());
                body.push(Self::section("", vec![Button::new("show-comp", "Show it in the canvas").small().with_icon("layers").on_click(move |_, _, cx| studio.update(cx, |s, cx| {
                    s.composition = Some(id2.clone());
                    s.changed(cx);
                })).into_any_element()], cx));
                body
            }
        }
    }

    fn camera_actions(&mut self, clip: kimchi_core::Id, scene: &Scene, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let Scene::Space(s) = scene else { return div().into_any_element() };
        let active = s.active_camera.clone().unwrap_or_else(|| "camera".into()) == key;
        let mut body = vec![];
        let id = key.to_string();
        let _studio = self.studio.clone();
        body.push(
            Button::new("cam-active", if active { "The active camera" } else { "Make it the active camera" })
                .small()
                .with_icon("video")
                .disabled(active)
                .on_click(move |_, _, cx| super::run("motion.updateLayer", json!({ "clipId": clip, "id": "scene", "props": { "activeCamera": id } }), cx))
                .into_any_element(),
        );
        // Focus on an object: the focus distance becomes how far it is.
        let t = self.studio.read(cx).scene_time(cx);
        let w = model::worlds(s, t);
        let eye = w.get(key).map(super::math::origin).unwrap_or_default();
        let objects: Vec<(String, f64)> = model::thing_ids(scene)
            .into_iter()
            .filter(|i| matches!(model::item(scene, i), Some(Item::Object(_))))
            .filter_map(|i| w.get(&i).map(|m| (i, super::math::len(super::math::sub(super::math::origin(m), eye)))))
            .collect();
        let (studio, id) = (self.studio.clone(), key.to_string());
        body.push(super::fields::dropdown("focus-on", "Focus on…".into(), move |_| {
            objects
                .iter()
                .map(|(o, d)| {
                    let (_studio, id, d) = (studio.clone(), id.clone(), *d);
                    crate::store::MenuItem::new(format!("{o} ({d:.2})"), move |_, cx| {
                        super::run("motion.updateLayer", json!({ "clipId": clip, "id": id, "props": { "focusDistance": (d * 1000.0).round() / 1000.0 } }), cx)
                    })
                    .entry()
                })
                .collect()
        }, cx));
        Self::section("Use", body, cx)
    }

    fn material_tab(&mut self, clip: kimchi_core::Id, scene: &Scene, key: &str, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Scene::Space(s) = scene else { return vec![] };
        let Some(obj) = kimchi_core::motion::find_object(&s.objects, key) else { return vec![] };
        let mut out = vec![];
        let shared: Vec<String> = s.materials.iter().filter_map(|m| m.id.clone()).collect();
        let studio = self.studio.clone();
        let id = key.to_string();
        let current = obj.material.from.clone();
        // Own or shared.
        let mut pick = vec![];
        {
            let (studio, id) = (studio.clone(), id.clone());
            let shown = current.clone().map(|m| format!("Shared: {m}")).unwrap_or_else(|| "Its own material".into());
            let own = obj.material.fields.clone();
            let shared2 = shared.clone();
            let resolved = s.resolve_material(&obj.material);
            pick.push(super::fields::dropdown("mat-pick", shown, move |_| {
                let mut list = vec![];
                {
                    let (_studio, id, own) = (studio.clone(), id.clone(), resolved.fields.clone());
                    list.push(crate::store::MenuItem::new("Its own material (a copy)", move |_, cx| {
                        let mut m = serde_json::to_value(&own).unwrap_or_default();
                        if let Some(o) = m.as_object_mut() {
                            o.remove("id");
                        }
                        super::run("motion.updateLayer", json!({ "clipId": clip, "id": id, "props": { "material": m } }), cx)
                    }).entry());
                }
                for m in &shared2 {
                    let (_studio, id, m) = (studio.clone(), id.clone(), m.clone());
                    list.push(crate::store::MenuItem::new(format!("Shared: {m}"), move |_, cx| super::run("motion.updateLayer", json!({ "clipId": clip, "id": id, "props": { "material": m } }), cx)).entry());
                }
                let _ = &own;
                list
            }, cx));
        }
        match &current {
            Some(m) => {
                let (studio, m) = (studio.clone(), m.clone());
                pick.push(Button::new("edit-shared", format!("Edit “{m}”")).small().with_icon("pencil").on_click(move |_, _, cx| studio.update(cx, |s, cx| s.set_selection(vec![format!("{}{m}", model::MATERIAL)], cx))).into_any_element());
            }
            None => {
                let (studio, id, fields) = (studio.clone(), id.clone(), obj.material.fields.clone());
                let fresh = model::fresh_id(scene, "material");
                pick.push(Button::new("make-shared", "Make it shared").small().with_icon("palette").tooltip("Other objects can then use the same material").on_click(move |_, _, cx| {
                    let mut m = serde_json::to_value(&fields).unwrap_or_default();
                    m["id"] = json!(fresh);
                    let (_studio2, id, fresh) = (studio.clone(), id.clone(), fresh.clone());
                    studio.update(cx, |s, cx| s.run_then("motion.setMaterial", json!({ "clipId": clip, "material": m }), cx, move |_, _, cx| {
                        super::run("motion.updateLayer", json!({ "clipId": clip, "id": id, "props": { "material": fresh } }), cx)
                    }));
                }).into_any_element());
            }
        }
        out.push(Self::section("Material", pick, cx));
        if current.is_none() {
            let json = scene.item_json(key).unwrap_or_default();
            let c = self.ctx(clip, scene, key, Tk::Item, key.to_string(), json.clone(), cx);
            out.extend(self.sections(&c, vec![Section { title: "Surface", fields: specs::material_fields("material.") }], window, cx));
            out.push(self.pattern(&c, json.get("material").and_then(|m| m.get("pattern")).cloned(), "material.pattern", "pattern.", window, cx));
        }
        out
    }

    fn shared_material(&mut self, clip: kimchi_core::Id, scene: &Scene, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Scene::Space(s) = scene else { return vec![] };
        let Some(m) = s.material(id) else { return vec![] };
        let json = serde_json::to_value(m).unwrap_or_default();
        let c = self.ctx(clip, scene, "scene", Tk::Material(id.to_string()), format!("mat:{id}"), json.clone(), cx);
        let users: Vec<String> = {
            let mut v = vec![];
            kimchi_core::motion::walk_objects(&s.objects, &mut |o| {
                if o.material.from.as_deref() == Some(id) {
                    v.push(o.id.clone());
                }
            });
            v
        };
        let t = cx.theme().clone();
        let mut out = vec![Self::section(&format!("Shared material · {id}"), vec![div().text_size(px(sz::SM)).text_color(t.text_2).child(if users.is_empty() { "No object uses it yet.".to_string() } else { format!("Used by {}", users.join(", ")) }).into_any_element()], cx)];
        out.extend(self.sections(&c, vec![Section { title: "Surface", fields: specs::material_fields("") }], window, cx));
        out.push(self.pattern(&c, json.get("pattern").cloned(), "pattern", "pattern.", window, cx));
        out
    }

    /// A material's procedural pattern: the type and its parameters (from the PATTERNS table).
    fn pattern(&mut self, c: &FieldCtx, pattern: Option<Value>, setting: &'static str, prefix: &'static str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let kind = pattern.as_ref().and_then(|p| p.get("type")).and_then(Value::as_str).unwrap_or("").to_string();
        let types = kimchi_core::motion::stack::PATTERNS;
        let shown = types.iter().find(|t| t.name == kind).map(|t| t.label.to_string()).unwrap_or_else(|| "None".into());
        let (this, cc) = (cx.entity(), c.clone());
        let mut body = vec![super::fields::dropdown(&format!("{}-pattern", c.scope), shown, move |_| {
            let mut list = vec![];
            let (this2, cc2) = (this.clone(), cc.clone());
            list.push(crate::store::MenuItem::new("None", move |_, cx| {
                let cc = cc2.clone();
                this2.update(cx, |p, cx| p.write(&cc, setting, Value::Null, true, cx))
            }).entry());
            for t in types {
                let (this, cc, name) = (this.clone(), cc.clone(), t.name);
                list.push(crate::store::MenuItem::new(t.label, move |_, cx| {
                    let cc = cc.clone();
                    this.update(cx, |p, cx| p.write(&cc, setting, json!({ "type": name }), true, cx))
                }).entry());
            }
            list
        }, cx)];
        if let Some(spec) = types.iter().find(|t| t.name == kind) {
            for p in spec.params {
                // Own materials animate `pattern.<param>`; shared ones set it in the material.
                let name: &'static str = intern(format!("{prefix}{}", p.name));
                body.push(self.param_field(c, name, p, window, cx));
            }
        }
        Self::section("Pattern", body, cx)
    }

    /// A field for one parameter of a stack table.
    fn param_field(&mut self, c: &FieldCtx, name: &'static str, p: &kimchi_core::motion::stack::ParamSpec, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let def: &'static str = intern(p.default.value().map(|v| v.to_string()).unwrap_or_else(|| "null".into()));
        let kind = match p.kind {
            ParamKind::Number { min, max, step } => {
                let dec = if step >= 1.0 { 0 } else if step >= 0.1 { 1 } else if step >= 0.01 { 2 } else { 3 };
                Fk::Num(step, dec, min.max(-1e9), max.min(1e9))
            }
            ParamKind::Int { min, max } => Fk::Num(1.0, 0, min, max),
            ParamKind::Bool => Fk::Bool,
            ParamKind::Color => Fk::Color,
            ParamKind::Choice(o) => Fk::Choice(o),
            ParamKind::Text => Fk::Text,
            ParamKind::Vec2 => Fk::Vec2(0.5, 2),
            ParamKind::Vec3 => Fk::Vec3(0.05, 2),
            ParamKind::Ref => Fk::Sibling,
            ParamKind::Path => Fk::Path,
        };
        if p.kind == ParamKind::Ref {
            return self.ref_field(c, name, p.label, cx);
        }
        let f = F { name, label: p.label, kind, def };
        self.field(c, &f, window, cx)
    }

    /// A reference to another thing of the scene (boolean cutters, look-at targets, paths).
    fn ref_field(&mut self, c: &FieldCtx, name: &'static str, label: &str, cx: &mut Context<Self>) -> AnyElement {
        let current = model::value_at(&c.scene, &c.id, name, c.t).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let ids: Vec<String> = model::thing_ids(&c.scene).into_iter().filter(|i| *i != c.id).collect();
        let (this, cc) = (cx.entity(), c.clone());
        let control = super::fields::dropdown(&format!("{}|{name}", c.scope), if current.is_empty() { "None".into() } else { current }, move |_| {
            ids.iter()
                .map(|i| {
                    let (this, cc, i) = (this.clone(), cc.clone(), i.clone());
                    crate::store::MenuItem::new(i.clone(), move |_, cx| {
                        let (cc, i) = (cc.clone(), i.clone());
                        this.update(cx, |p, cx| p.write(&cc, name, json!(i), true, cx))
                    })
                    .entry()
                })
                .collect()
        }, cx);
        let t = cx.theme();
        div().flex().items_center().gap(px(4.)).min_h(px(26.)).child(div().w(px(104.)).flex_none().text_size(px(sz::XS)).text_color(t.text_2).child(label.to_string())).child(div().flex_1().min_w_0().child(control)).child(div().w(px(22.))).into_any_element()
    }

    /// A stack: its items with their parameters, and Add.
    fn stack_tab(&mut self, clip: kimchi_core::Id, scene: &Scene, key: &str, field: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.theme().clone();
        let json = scene.item_json(key).unwrap_or_default();
        let items: Vec<Value> = json.get(field).and_then(Value::as_array).cloned().unwrap_or_default();
        let types = super::menus::types_of(field);
        let c = self.ctx(clip, scene, key, Tk::Param, key.to_string(), json.clone(), cx);
        let mut out = vec![];
        let studio = self.studio.clone();
        let id = key.to_string();
        let noun = match field {
            "modifiers" => "modifier",
            "constraints" => "constraint",
            "effects" => "effect",
            "operators" => "operator",
            "masks" => "mask",
            _ => "animator",
        };
        out.push(
            div()
                .px(px(12.))
                .py(px(10.))
                .border_b_1()
                .border_color(t.line)
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(if items.is_empty() { format!("No {field} yet.") } else { format!("{} {field}, run in order", items.len()) }))
                .child(Button::new(SharedString::from(format!("add-{field}")), format!("Add {noun}")).small().with_icon("plus").on_click(move |e, w, cx| {
                    let at = e.position();
                    let id = id.clone();
                    studio.update(cx, |s, cx| {
                        s.popover = Some(Popover::stack(at, id, field, w, cx));
                        cx.notify();
                    })
                }))
                .into_any_element(),
        );
        let n = items.len();
        for (i, item) in items.iter().enumerate() {
            let item_id = item.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("").to_string();
            let enabled = item.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let spec: Option<&TypeSpec> = types.iter().find(|s| s.name == kind);
            let fold_key = format!("{key}/{field}/{item_id}");
            let folded = self.folded.contains(&fold_key);
            let this = cx.entity();
            let header = {
                let (fk, this2) = (fold_key.clone(), this.clone());
                let full = scene_item_full(scene, key, field, &item_id);
                let (_studio_e, _studio_u, _studio_d, _studio_x) = (self.studio.clone(), self.studio.clone(), self.studio.clone(), self.studio.clone());
                let (k1, k2, k3, k4) = (key.to_string(), key.to_string(), key.to_string(), key.to_string());
                let (i1, i2, i3) = (item_id.clone(), item_id.clone(), item_id.clone());
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .id(SharedString::from(format!("fold-{fold_key}")))
                            .cursor_pointer()
                            .text_color(t.text_2)
                            .child(icon(if folded { "chevron-right" } else { "chevron-down" }))
                            .on_click(move |_, _, cx| this2.update(cx, |p, cx| {
                                if !p.folded.remove(&fk) {
                                    p.folded.insert(fk.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("on-{fold_key}")))
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if enabled { t.accent } else { t.line_strong })
                            .bg(if enabled { t.accent_soft } else { gpui::transparent_black() })
                            .cursor_pointer()
                            .when(enabled, |d| d.child(icon("check").size(px(10.)).text_color(t.accent_text)))
                            .tooltip(move |_, cx| crate::ui::tooltip(if enabled { "On (click to turn off)" } else { "Off (click to turn on)" }.into(), cx))
                            .on_click(move |_, _, cx| {
                                let mut it = full.clone();
                                it["enabled"] = json!(!enabled);
                                super::run("motion.setStackItem", json!({ "clipId": clip, "id": k1, "field": field, "item": it }), cx)
                            }),
                    )
                    .child(div().flex_1().min_w_0().flex().items_baseline().gap(px(6.)).child(div().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).child(spec.map(|s| s.label).unwrap_or(kind.as_str()).to_string())).child(div().font_family(MONO).text_size(px(10.)).text_color(t.text_3).truncate().child(item_id.clone())))
                    .child(Button::icon(SharedString::from(format!("up-{fold_key}")), "chevron-up", "Earlier in the stack").small().disabled(i == 0).on_click(move |_, _, cx| {
                        super::run("motion.moveStackItem", json!({ "clipId": clip, "id": k2, "field": field, "itemId": i1, "index": i.saturating_sub(1) }), cx)
                    }))
                    .child(Button::icon(SharedString::from(format!("down-{fold_key}")), "chevron-down", "Later in the stack").small().disabled(i + 1 >= n).on_click(move |_, _, cx| {
                        super::run("motion.moveStackItem", json!({ "clipId": clip, "id": k3, "field": field, "itemId": i2, "index": i + 1 }), cx)
                    }))
                    .child(Button::icon(SharedString::from(format!("rm-{fold_key}")), "trash", "Remove").small().on_click(move |_, _, cx| {
                        super::run("motion.removeStackItem", json!({ "clipId": clip, "id": k4, "field": field, "itemId": i3 }), cx)
                    }))
            };
            let mut body = vec![header.into_any_element()];
            if !folded && let Some(spec) = spec {
                if !spec.doc.is_empty() {
                    body.push(div().text_size(px(sz::XS)).text_color(t.text_3).line_height(px(sz::XS * 1.4)).child(spec.doc).into_any_element());
                }
                for p in spec.params {
                    let name: &'static str = intern(format!("{field}.{item_id}.{}", p.name));
                    body.push(self.param_field(&c, name, p, window, cx));
                }
            }
            out.push(div().flex().flex_col().gap(px(4.)).px(px(12.)).py(px(8.)).border_b_1().border_color(t.line).when(!enabled, |d| d.opacity(0.6)).children(body).into_any_element());
        }
        out
    }

    fn expressions_tab(&mut self, clip: kimchi_core::Id, scene: &Scene, key: &str, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.theme().clone();
        let json = scene.item_json(key).unwrap_or_default();
        let mut exprs: Vec<(String, String)> = json.get("expressions").and_then(Value::as_object).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
        if let Some(n) = &self.expr_new
            && !exprs.iter().any(|(k, _)| k == n)
        {
            exprs.push((n.clone(), String::new()));
        }
        let mut out = vec![Self::section(
            "Expressions",
            vec![div().text_size(px(sz::XS)).text_color(t.text_2).line_height(px(sz::XS * 1.45)).child("A formula drives a property every frame: time, value (the keyframed value), wiggle(), loopOut(), prop('id', 'x')… Empty removes it.").into_any_element()],
            cx,
        )];
        for (prop, src) in exprs {
            let ekey = format!("expr|{key}|{prop}");
            let (_studio, id, p2) = (self.studio.clone(), key.to_string(), prop.clone());
            let this = cx.entity();
            let input = self.text(&ekey, 1, true, false, "time * 90", move |this, text, _fin, cx| {
                let (id, p2) = (id.clone(), p2.clone());
                let task = super::call("motion.setExpression", json!({ "clipId": clip, "id": id, "property": p2, "expression": text }), cx);
                cx.spawn(async move |this, cx| {
                    let r = task.await;
                    this.update(cx, |this, cx| {
                        match r {
                            Ok(_) => {
                                this.expr_errors.remove(&p2);
                                if this.expr_new.as_deref() == Some(p2.as_str()) {
                                    this.expr_new = None;
                                }
                            }
                            Err(e) => {
                                this.expr_errors.insert(p2.clone(), e);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
                let _ = this;
            }, cx);
            let _ = this;
            if !self.expr_errors.contains_key(&prop) && input.read(cx).text() != src && !input.read(cx).is_focused(window) {
                let s = src.clone();
                input.update(cx, |i, cx| i.set_text(s, cx));
            }
            let snippets = {
                let input = input.clone();
                super::fields::dropdown(&format!("snip-{ekey}"), "Insert…".into(), move |_| {
                    specs::SNIPPETS
                        .iter()
                        .map(|(label, text)| {
                            let input = input.clone();
                            crate::store::MenuItem::new(format!("{label}: {text}"), move |w, cx| {
                                let cur = input.read(cx).text().to_string();
                                let next = if cur.trim().is_empty() { text.to_string() } else { format!("{cur} + {text}") };
                                input.update(cx, |i, cx| i.set_text(next, cx));
                                crate::ui::input::focus(&input, w, cx);
                            })
                            .entry()
                        })
                        .collect()
                }, cx)
            };
            let (_studio, id, p3) = (self.studio.clone(), key.to_string(), prop.clone());
            let this = cx.entity();
            let mut body = vec![
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_family(MONO).text_size(px(sz::SM)).child(prop.clone()))
                    .child(div().flex().gap(px(4.)).child(div().w(px(90.)).child(snippets)).child(Button::icon(SharedString::from(format!("rm-{ekey}")), "trash", "Remove the expression").small().on_click(move |_, _, cx| {
                        let p3 = p3.clone();
                        this.update(cx, |p, cx| {
                            p.expr_errors.remove(&p3);
                            if p.expr_new.as_deref() == Some(p3.as_str()) {
                                p.expr_new = None;
                            }
                            cx.notify();
                        });
                        super::run("motion.setExpression", json!({ "clipId": clip, "id": id, "property": p3, "expression": "" }), cx)
                    })))
                    .into_any_element(),
                input.into_any_element(),
            ];
            if let Some(e) = self.expr_errors.get(&prop) {
                body.push(div().text_size(px(sz::XS)).text_color(t.danger).child(e.clone()).into_any_element());
            }
            out.push(div().flex().flex_col().gap(px(4.)).px(px(12.)).py(px(8.)).border_b_1().border_color(t.line).children(body).into_any_element());
        }
        // Add one.
        let names: Vec<String> = model::prop_names(scene, key).into_iter().filter(|n| !json.get("expressions").and_then(|e| e.get(n)).is_some()).collect();
        let this = cx.entity();
        out.push(div().px(px(12.)).py(px(10.)).child(super::fields::dropdown("expr-add", "Add an expression to…".into(), move |_| {
            names
                .iter()
                .map(|n| {
                    let (this, n) = (this.clone(), n.clone());
                    crate::store::MenuItem::new(n.clone(), move |_, cx| this.update(cx, |p, cx| {
                        p.expr_new = Some(n.clone());
                        cx.notify();
                    }))
                    .entry()
                })
                .collect()
        }, cx)).into_any_element());
        out
    }
}

/// A stack item's JSON with every parameter (to replace it).
fn scene_item_full(scene: &Scene, id: &str, field: &str, item: &str) -> Value {
    scene.item_json(id).and_then(|j| j.get(field)?.as_array()?.iter().find(|x| x.get("id").and_then(Value::as_str) == Some(item)).cloned()).unwrap_or(Value::Null)
}

fn shape_title(shape: &str) -> &'static str {
    match shape {
        "box" => "Box",
        "sphere" => "Sphere",
        "icosphere" => "Icosphere",
        "cylinder" => "Cylinder",
        "cone" => "Cone",
        "capsule" => "Capsule",
        "torus" => "Torus",
        "plane" => "Plane",
        "grid" => "Grid",
        "text" => "3D text",
        "extrude" => "Extruded shape",
        "lathe" => "Lathe",
        "curve" => "Curve",
        "mesh" => "Mesh",
        "particles" => "Particles",
        "model" => "Model",
        "image" => "Image card",
        _ => "Shape",
    }
}

fn layer_title(kind: &str) -> &'static str {
    match kind {
        "rect" => "Rectangle",
        "ellipse" => "Ellipse",
        "polygon" => "Polygon",
        "star" => "Star",
        "path" => "Path",
        "text" => "Text",
        "image" => "Picture",
        "group" => "Group",
        "null" => "Null",
        "adjustment" => "Adjustment layer",
        "comp" => "Composition layer",
        "particles" => "Particles",
        _ => "Layer",
    }
}

/// Property names made at run time (stack items' parameters) as the `&'static str` field specs
/// use: each distinct name is kept once.
fn intern(s: String) -> &'static str {
    thread_local! {
        static NAMES: std::cell::RefCell<HashSet<&'static str>> = Default::default();
    }
    NAMES.with(|n| {
        let mut n = n.borrow_mut();
        match n.get(s.as_str()) {
            Some(x) => x,
            None => {
                let x: &'static str = Box::leak(s.into_boxed_str());
                n.insert(x);
                x
            }
        }
    })
}

impl Render for Properties {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (found, key, more) = {
            let st = self.studio.read(cx);
            (st.clip_scene(cx), st.active().map(str::to_string).unwrap_or_else(|| "scene".into()), st.selection.len().saturating_sub(1))
        };
        let Some((clip, scene)) = found else { return div().into_any_element() };
        let item = model::item(&scene, &key);
        let tabs = tabs_for(&scene, item.as_ref());
        if !tabs.contains(&self.tab) {
            self.tab = tabs[0];
        }
        self.used.clear();
        let tab = self.tab;
        let body: Vec<AnyElement> = match (tab, &item) {
            (Tab::World, _) => {
                let c = self.ctx(clip.id, &scene, "scene", Tk::Item, "scene".into(), scene.to_json(), cx);
                let mut b = self.sections(&c, specs::world_fields(), window, cx);
                b.insert(0, Self::section("3D scene", vec![div().text_size(px(sz::XS)).text_color(t.text_2).child("The world around the objects: what lights them from everywhere and what shiny things reflect.").into_any_element()], cx));
                b
            }
            (Tab::Render, _) => {
                let c = self.ctx(clip.id, &scene, "scene", Tk::Item, "scene".into(), scene.to_json(), cx);
                self.sections(&c, specs::render_fields(), window, cx)
            }
            (Tab::Material, _) => self.material_tab(clip.id, &scene, &key, window, cx),
            (Tab::Expressions, _) => self.expressions_tab(clip.id, &scene, &key, window, cx),
            (tab, _) if tab.field().is_some() => self.stack_tab(clip.id, &scene, &key, tab.field().unwrap_or("effects"), window, cx),
            (_, Some(item)) => self.item_tab(clip.id, &scene, &key, item, window, cx),
            (_, None) => self.item_tab(clip.id, &scene, "scene", &Item::Scene, window, cx),
        };
        let used = std::mem::take(&mut self.used);
        self.widgets.retain(|k, _| used.contains(k) || k.starts_with("expr|"));
        self.used = used;
        let title = match &item {
            Some(Item::Scene) | None => if scene.is_3d() { "World".to_string() } else { "Scene".to_string() },
            Some(Item::Material(m)) => format!("Material · {m}"),
            Some(Item::Composition(c)) => format!("Composition · {c}"),
            Some(_) => key.clone(),
        };
        let this = cx.entity();
        div()
            .id("studio-properties")
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg_raised)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.studio.update(cx, |s, _| s.area = Area::Properties)))
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .when(more > 0, |d| d.child(div().text_size(px(sz::XS)).text_color(t.text_3).child(format!("+{more} selected")))),
            )
            .child(
                div().flex_none().flex().flex_wrap().gap(px(2.)).px(px(6.)).py(px(4.)).border_b_1().border_color(t.line).children(tabs.into_iter().map(|tb| {
                    let this = this.clone();
                    Button::new(SharedString::from(format!("ptab-{}", tb.label())), tb.label()).small().ghost().with_icon(tb.icon()).selected(tb == tab).on_click(move |_, _, cx| this.update(cx, |p, cx| {
                        p.tab = tb;
                        cx.notify();
                    }))
                })),
            )
            .child(div().id("studio-properties-body").flex_1().min_h_0().overflow_y_scroll().pb(px(24.)).children(body))
            .into_any_element()
    }
}
