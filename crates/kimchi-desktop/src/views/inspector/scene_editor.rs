//! Editing a motion clip's scene by hand, in the inspector: pick a layer, object, light, the
//! camera or the scene; change its properties (numbers, colours, words) where the playhead is,
//! with a keyframe toggle per property; add shapes, text, pictures, 3D objects and lights;
//! delete. Everything goes through `motion.*` commands, like the agent's edits.

use gpui::{AnyElement, Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};
use kimchi_core::motion::{LayerKind, Shape3d, find_layer, walk_objects};
use kimchi_core::{Clip, ClipContent, Id, KeyValue, Scene};
use serde_json::{Value, json};

use super::color::{ColorChange, ColorField};
use super::{Inspector, grid2};
use crate::store::StoreExt;
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::Button;

/// How a property is edited.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// step, decimals, min, max
    Number(f64, usize, f64, f64),
    Color,
    Text,
}

/// A property of the picked item: (keyframe name, label, kind).
type Prop = (&'static str, &'static str, Kind);

const PX: Kind = Kind::Number(1.0, 0, -100_000.0, 100_000.0);
const UNIT: Kind = Kind::Number(0.01, 2, 0.0, 1.0);
const SCALE: Kind = Kind::Number(0.01, 2, 0.0, 100.0);
const DEG: Kind = Kind::Number(1.0, 0, -36_000.0, 36_000.0);
const WORLD: Kind = Kind::Number(0.05, 2, -10_000.0, 10_000.0);
const SIZE: Kind = Kind::Number(1.0, 0, 0.0, 100_000.0);

/// The editable properties of `id` in `scene`.
fn props(scene: &Scene, id: &str) -> Vec<Prop> {
    match (scene, id) {
        (Scene::Flat(_), "scene") => vec![("background", "Background", Kind::Color)],
        (Scene::Space(_), "scene") => vec![("background", "Background", Kind::Color), ("ambient", "Ambient", UNIT), ("ambientColor", "Ambient colour", Kind::Color)],
        (Scene::Space(_), "camera") => vec![
            ("position.x", "X", WORLD),
            ("position.y", "Y", WORLD),
            ("position.z", "Z", WORLD),
            ("target.x", "Look X", WORLD),
            ("target.y", "Look Y", WORLD),
            ("target.z", "Look Z", WORLD),
            ("fov", "Field of view", Kind::Number(1.0, 0, 1.0, 170.0)),
            ("roll", "Roll", DEG),
        ],
        (Scene::Space(s), _) if s.lights.iter().any(|l| l.id == id) => {
            let point = s.lights.iter().any(|l| l.id == id && l.kind == "point");
            let mut v = vec![("intensity", "Intensity", Kind::Number(0.05, 2, 0.0, 100.0)), ("color", "Colour", Kind::Color)];
            if point {
                v.extend([("position.x", "X", WORLD), ("position.y", "Y", WORLD), ("position.z", "Z", WORLD), ("range", "Range", WORLD)]);
            } else {
                v.extend([("direction.x", "Towards X", WORLD), ("direction.y", "Towards Y", WORLD), ("direction.z", "Towards Z", WORLD)]);
            }
            v
        }
        (Scene::Space(s), _) => {
            let mut shape = None;
            walk_objects(&s.objects, &mut |o| {
                if o.id == id {
                    shape = Some(o.shape.clone());
                }
            });
            let mut v = vec![
                ("position.x", "X", WORLD),
                ("position.y", "Y", WORLD),
                ("position.z", "Z", WORLD),
                ("rotation.x", "Turn X", DEG),
                ("rotation.y", "Turn Y", DEG),
                ("rotation.z", "Turn Z", DEG),
                ("scale", "Scale", Kind::Number(0.01, 2, 0.0, 1000.0)),
                ("opacity", "Opacity", UNIT),
            ];
            match shape {
                Some(Shape3d::Text { .. }) => v.extend([("text", "Words", Kind::Text), ("size", "Size", WORLD), ("depth", "Depth", WORLD)]),
                Some(Shape3d::Sphere { .. } | Shape3d::Cylinder { .. } | Shape3d::Cone { .. }) => v.push(("radius", "Radius", WORLD)),
                Some(Shape3d::Torus { .. }) => v.extend([("radius", "Radius", WORLD), ("tube", "Tube", WORLD)]),
                Some(Shape3d::Box { .. }) => v.push(("bevel", "Bevel", Kind::Number(0.01, 2, 0.0, 10.0))),
                _ => {}
            }
            if !matches!(shape, Some(Shape3d::Image { .. } | Shape3d::Group {} | Shape3d::Model { .. })) {
                v.extend([("color", "Colour", Kind::Color), ("metallic", "Metallic", UNIT), ("roughness", "Roughness", UNIT)]);
            }
            v
        }
        (Scene::Flat(s), _) => {
            let Some(layer) = find_layer(&s.layers, id) else { return vec![] };
            let mut v = vec![("x", "X", PX), ("y", "Y", PX), ("scale", "Scale", SCALE), ("rotation", "Rotate", DEG), ("opacity", "Opacity", UNIT), ("blur", "Blur", SIZE)];
            match &layer.kind {
                LayerKind::Text(_) => {
                    let mut first = vec![("text", "Words", Kind::Text), ("fill", "Colour", Kind::Color), ("fontSize", "Size", SIZE), ("letterSpacing", "Tracking", Kind::Number(0.5, 1, -200.0, 500.0))];
                    first.append(&mut v);
                    v = first;
                }
                LayerKind::Rect { .. } => v.extend([("width", "Width", SIZE), ("height", "Height", SIZE), ("radius", "Corners", SIZE)]),
                LayerKind::Ellipse { .. } => v.extend([("width", "Width", SIZE), ("height", "Height", SIZE)]),
                LayerKind::Polygon { .. } => v.extend([("radius", "Radius", SIZE), ("sides", "Sides", Kind::Number(1.0, 0, 3.0, 64.0))]),
                LayerKind::Star { .. } => v.extend([("radius", "Radius", SIZE), ("innerRadius", "Inner", SIZE), ("points", "Points", Kind::Number(1.0, 0, 2.0, 64.0))]),
                LayerKind::Image { .. } => v.extend([("width", "Width", SIZE), ("radius", "Corners", SIZE)]),
                _ => {}
            }
            if !matches!(layer.kind, LayerKind::Text(_) | LayerKind::Image { .. } | LayerKind::Group { .. }) {
                if layer.fill.as_ref().is_none_or(|f| f.color().is_some()) {
                    v.push(("fill", "Fill", Kind::Color));
                }
                v.extend([("strokeColor", "Outline", Kind::Color), ("strokeWidth", "Outline width", Kind::Number(0.5, 1, 0.0, 1000.0))]);
            }
            if layer.stroke.is_some() {
                v.extend([("trimStart", "Draw from", UNIT), ("trimEnd", "Draw to", UNIT)]);
            }
            v
        }
    }
}

enum Field {
    Number(Entity<Scrub>),
    Color(Entity<ColorField>),
    Text(Entity<TextInput>),
}

/// The controls for the item picked in a motion clip.
#[derive(Default)]
pub struct ItemFields {
    /// Clip and item they were made for.
    key: Option<(Id, String)>,
    fields: Vec<(Prop, Field)>,
    _subs: Vec<Subscription>,
}

impl Inspector {
    /// The scene editor of a motion clip: item chips, the picked item's fields, add buttons.
    pub(super) fn scene_section(&mut self, clip: &Clip, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let ClipContent::Motion { scene, .. } = &clip.content else { return div().into_any_element() };
        let id = clip.id;
        // The picked item, if it still exists.
        let mut ids = scene.ids();
        if scene.is_3d() {
            ids.insert(0, "camera".into());
        }
        ids.insert(0, "scene".into());
        let picked = self.scene_item.as_ref().filter(|(c, i)| *c == id && ids.contains(i)).map(|(_, i)| i.clone());
        if picked.is_none() {
            self.scene_item = None;
            self.item_fields = ItemFields::default();
        }
        let chips = div().flex().flex_wrap().gap(px(4.)).children(ids.iter().map(|item| {
            let selected = picked.as_deref() == Some(item.as_str());
            let item = item.clone();
            let label = match item.as_str() {
                "scene" => "Scene".to_string(),
                "camera" => "Camera".to_string(),
                other => other.to_string(),
            };
            let this = cx.entity();
            div()
                .id(SharedString::from(format!("item-{item}")))
                .px(px(7.))
                .py(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .border_1()
                .text_size(px(sz::XS))
                .font_family(MONO)
                .when(selected, |d| d.bg(t.accent_soft).border_color(t.accent_ring).text_color(t.accent_text))
                .when(!selected, |d| d.bg(t.hover).border_color(t.line).text_color(t.text_2).hover(|s| s.border_color(t.accent_ring)))
                .child(label)
                .on_click(move |_, _, cx| {
                    let item = item.clone();
                    this.update(cx, |this, cx| {
                        this.scene_item = if selected { None } else { Some((id, item)) };
                        cx.notify();
                    })
                })
        }));
        let mut out = self.fold("scene", "Scene", cx).child(chips);

        if let Some(item) = picked {
            out = out.child(self.item_editor(clip, scene, &item, window, cx));
        } else {
            out = out.child(div().text_size(px(sz::SM)).text_color(t.text_3).child("Pick a layer to change it, or add one:"));
        }

        // Add buttons.
        let adds: Vec<(&'static str, &'static str, Value)> = if scene.is_3d() {
            vec![
                ("Box", "box", json!({"type": "box", "size": 1, "bevel": 0.05, "material": {"color": "#ff5a36", "roughness": 0.4}})),
                ("Sphere", "sphere", json!({"type": "sphere", "radius": 0.6, "material": {"color": "#f2f2f2", "roughness": 0.3}})),
                ("Cylinder", "cylinder", json!({"type": "cylinder", "material": {"color": "#7a3cff"}})),
                ("Torus", "torus", json!({"type": "torus", "material": {"color": "#00b3a4", "metallic": 0.5, "roughness": 0.3}})),
                ("Plane", "plane", json!({"type": "plane", "width": 10, "height": 10, "rotation": [-90, 0, 0], "position": [0, -1, 0], "material": {"color": "#1a1a1f", "roughness": 0.9}})),
                ("3D text", "text", json!({"type": "text", "text": "Hello", "size": 1, "depth": 0.25, "material": {"color": "#ff5a36", "roughness": 0.35}})),
                ("Light", "light", json!({"type": "point", "position": [2, 3, 3], "intensity": 1.2, "range": 12})),
            ]
        } else {
            let canvas_h = self.store.read(cx).project.as_ref().map(|p| p.settings.height as f64).unwrap_or(1080.0);
            let k = canvas_h / 1080.0;
            vec![
                ("Text", "text", json!({"type": "text", "text": "Your words", "fontSize": 96.0 * k, "fill": "#ffffff"})),
                ("Rectangle", "rect", json!({"type": "rect", "width": 400.0 * k, "height": 160.0 * k, "radius": 16.0 * k, "fill": "#ff5a36"})),
                ("Circle", "circle", json!({"type": "ellipse", "width": 220.0 * k, "height": 220.0 * k, "fill": "#ffffff"})),
                ("Star", "star", json!({"type": "star", "radius": 120.0 * k, "innerRadius": 50.0 * k, "fill": "#f0b44c"})),
                ("Line", "line", json!({"type": "path", "d": format!("M{} 0 L{} 0", -300.0 * k, 300.0 * k), "stroke": {"color": "#ffffff", "width": 8.0 * k}, "keyframes": {"trimEnd": [[0, 0], [0.8, 1, "easeInOutCubic"]]}})),
            ]
        };
        let existing = scene.ids();
        // New 3D objects stand beside what is there, alternating right and left of the centre.
        let beside = {
            let n = match scene {
                Scene::Space(s) => s.objects.len(),
                Scene::Flat(_) => 0,
            } as f64;
            let side = if (n as usize) % 2 == 1 { 1.0 } else { -1.0 };
            side * 1.6 * (n / 2.0).ceil()
        };
        let mut row = div().flex().flex_wrap().gap(px(4.)).children(adds.into_iter().map(|(label, stem, mut v)| {
            let new_id = (1..).map(|n| format!("{stem}{n}")).find(|c| !existing.contains(c)).expect("a free id");
            v["id"] = json!(new_id);
            if scene.is_3d() && !matches!(stem, "plane" | "light") {
                v["position"] = json!([beside, 0.4, 0.0]);
            }
            let this = cx.entity();
            Button::new(SharedString::from(format!("add-{stem}")), label).small().with_icon("plus").on_click(move |_, _, cx| {
                let (v, new_id) = (v.clone(), new_id.clone());
                let this = this.clone();
                cx.store().update(cx, |s, cx| {
                    s.run_then("motion.setLayer", json!({ "clipId": id, "layer": v }), cx, move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            this.scene_item = Some((id, new_id));
                            cx.notify();
                        });
                    })
                })
            })
        }));
        // A picture from the media panel's selection.
        {
            let asset = self.store.read(cx).selected_asset.and_then(|a| self.store.read(cx).asset(a).cloned()).filter(|a| a.kind != kimchi_core::MediaKind::Audio);
            if let Some(a) = asset {
                let new_id = (1..).map(|n| format!("picture{n}")).find(|c| !existing.contains(c)).expect("a free id");
                let layer = if scene.is_3d() {
                    json!({"id": new_id, "type": "image", "asset": a.id, "width": 2})
                } else {
                    json!({"id": new_id, "type": "image", "asset": a.id, "width": 640})
                };
                row = row.child(Button::new("add-picture", format!("Picture: {}", a.name)).small().with_icon("image").on_click(move |_, _, cx| {
                    let layer = layer.clone();
                    cx.store().update(cx, |s, cx| s.run("motion.setLayer", json!({ "clipId": id, "layer": layer }), cx))
                }));
            }
        }
        out.child(row).into_any_element()
    }

    /// Fields and keyframe toggles of one item.
    fn item_editor(&mut self, clip: &Clip, scene: &Scene, item: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let id = clip.id;
        let props = props(scene, item);
        let key = (id, item.to_string());
        if self.item_fields.key.as_ref() != Some(&key) {
            let mut fields = vec![];
            let mut subs = vec![];
            for prop in props.iter().copied() {
                let (name, label, kind) = prop;
                let field = match kind {
                    Kind::Number(step, decimals, min, max) => {
                        let e = cx.new(|_| Scrub::new(label, step, decimals).range(min, max));
                        subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ScrubChange, cx| this.item_change(name, json!(ch.value), ch.final_, cx)));
                        Field::Number(e)
                    }
                    Kind::Color => {
                        let e = cx.new(|cx| ColorField::new(None, cx));
                        subs.push(cx.subscribe(&e, move |this: &mut Self, _, ch: &ColorChange, cx| this.item_change(name, json!(ch.0), true, cx)));
                        Field::Color(e)
                    }
                    Kind::Text => {
                        let e = cx.new(|cx| TextInput::new(cx).multiline(2).placeholder(label));
                        subs.push(cx.subscribe(&e, move |this: &mut Self, _, ev: &InputEvent, cx| {
                            if let InputEvent::Changed(text) = ev {
                                let v = json!(text);
                                this.item_change(name, v, false, cx);
                            }
                        }));
                        Field::Text(e)
                    }
                };
                fields.push((prop, field));
            }
            self.item_fields = ItemFields { key: Some(key), fields, _subs: subs };
        }
        // Values where the playhead is.
        let playhead = self.store.read(cx).playback.read(cx).playhead.clamp(clip.start, clip.end());
        let at = clip.scene_time(playhead);
        let mut animated = std::collections::BTreeSet::new();
        let mut keyed_here = std::collections::BTreeSet::new();
        {
            let mut copy = scene.clone();
            if let Some(it) = copy.item_mut(item) {
                for (name, keys) in it.keyframes() {
                    animated.insert(name.clone());
                    if keys.iter().any(|k| (k.time - at).abs() < 1e-3 + 0.5 / 60.0) {
                        keyed_here.insert(name.clone());
                    }
                }
            }
        }
        for ((name, _, _), field) in &self.item_fields.fields {
            let v = scene.value(item, name, at);
            match (field, v) {
                (Field::Number(e), Some(v)) => {
                    // A vector (a 3D scale) shows its first component; editing sets all three.
                    let n = match &v {
                        KeyValue::Vector(x) => x.first().copied(),
                        other => other.as_f64(),
                    };
                    if let Some(n) = n {
                        e.update(cx, |s, _| s.set_value(n));
                    }
                }
                (Field::Color(e), Some(KeyValue::Text(c))) => e.update(cx, |f, cx| f.set_value(&c, window, cx)),
                (Field::Text(e), Some(KeyValue::Text(s))) if !e.read(cx).is_focused(window) => e.update(cx, |i, cx| i.set_text(s, cx)),
                _ => {}
            }
        }

        let mut body = div().flex().flex_col().gap(px(8.)).pt(px(4.));
        let mut numbers = vec![];
        for ((_, label, _), field) in &self.item_fields.fields {
            match field {
                Field::Number(e) => numbers.push(e.clone().into_any_element()),
                Field::Color(e) => {
                    body = body.child(div().flex().flex_col().gap(px(4.)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child(*label)).child(e.clone()));
                }
                Field::Text(e) => body = body.child(e.clone()),
            }
        }
        if !numbers.is_empty() {
            body = body.child(grid2().children(numbers));
        }
        // Keyframe toggles for the numeric and colour properties.
        let item_s = item.to_string();
        let toggles = div().flex().flex_wrap().gap(px(4.)).children(props.iter().filter(|(_, _, k)| *k != Kind::Text).map(|&(name, label, _)| {
            let here = keyed_here.contains(name);
            let anim = animated.contains(name);
            let item = item_s.clone();
            let mut b = Button::new(SharedString::from(format!("ikey-{name}")), label).small().with_icon("diamond").selected(here).tooltip(if here {
                format!("Remove the {label} keyframe here")
            } else {
                format!("Keyframe {label} at the playhead")
            });
            if anim && !here {
                b = b.color(t.accent_text);
            }
            b.on_click(move |_, _, cx| {
                let item = item.clone();
                cx.store().update(cx, |s, cx| {
                    let time = s.playback.read(cx).playhead;
                    let command = if here { "motion.removeKeyframe" } else { "motion.addKeyframe" };
                    s.run(command, json!({ "clipId": id, "id": item, "property": name, "time": time }), cx)
                })
            })
        }));
        body = body.child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Keyframes at the playhead")).child(toggles);
        if item != "scene" && item != "camera" {
            let item = item.to_string();
            let this = cx.entity();
            body = body.child(Button::new("delete-item", format!("Delete {item}")).small().danger().with_icon("trash").on_click(move |_, _, cx| {
                let item = item.clone();
                cx.store().update(cx, |s, cx| s.run("motion.removeLayer", json!({ "clipId": id, "id": item }), cx));
                this.update(cx, |this, cx| {
                    this.scene_item = None;
                    cx.notify();
                });
            }));
        }
        body.into_any_element()
    }

    fn item_change(&mut self, name: &'static str, value: Value, final_: bool, cx: &mut Context<Self>) {
        let Some((clip, item)) = self.item_fields.key.clone() else { return };
        let time = self.store.read(cx).playback.read(cx).playhead;
        let mut p = json!({ "clipId": clip, "id": item, "props": { name: value }, "time": time });
        if !final_ {
            p["coalesce"] = json!(format!("{clip}:{item}:{name}"));
        }
        self.store.update(cx, |s, cx| s.run("motion.updateLayer", p, cx));
    }
}

#[cfg(test)]
impl Inspector {
    /// Picks `item` of `clip` (what clicking its chip does).
    pub fn pick(&mut self, clip: Id, item: &str, cx: &mut Context<Self>) {
        self.scene_item = Some((clip, item.to_string()));
        cx.notify();
    }

    /// The words field of the picked item, once it is shown.
    pub fn words_field(&self) -> Option<Entity<TextInput>> {
        self.item_fields.fields.iter().find_map(|((name, _, _), f)| match f {
            Field::Text(e) if *name == "text" => Some(e.clone()),
            _ => None,
        })
    }
}
