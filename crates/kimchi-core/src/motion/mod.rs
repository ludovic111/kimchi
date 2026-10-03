//! Motion clips: animated scenes kimchi draws itself, frame by frame, from plain JSON.
//!
//! A [`Scene`] is either 2D motion graphics ([`Scene2d`], like an After Effects composition:
//! layers of shapes, paths, text, pictures and particles, nested compositions, masks, mattes,
//! effects, shape operators, text animators) or a 3D scene ([`Scene3d`], like a Blender scene:
//! cameras, lights, a world, and objects — primitives, editable meshes, extruded text and logos,
//! curves, particles, models — with materials, modifiers and constraints). Everything has an
//! `id` agents refer to, every number or colour can be animated with `keyframes` (see
//! [`crate::anim`]): `{"opacity": [[0, 0], [0.5, 1, "easeOut"]]}`, and any property can follow
//! an expression (see [`crate::expr`]): `{"rotation": "time * 90"}`.
//!
//! Times inside a scene are scene seconds: 0 is the scene's first frame. The clip shows scene time
//! `in_point + (t − start) × speed`, so trimming the start of a motion clip cuts into it like
//! footage. Positions are project pixels from the canvas centre (2D, y down) or world units (3D,
//! y up, the camera looking at `target`).
//!
//! The renderer draws what [`Scene2d::layers_at`] and [`Scene3d::objects_at`] (and friends) give:
//! copies with keyframes, expressions and constraints applied. [`Scene::validate`] checks ids,
//! property names, colours, easings, references and formulas so an agent gets one clear error
//! instead of a silently wrong picture.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::anim::{KeyValue, Keyframes, Rgba, normalize, value_at};

pub mod curve;
mod eval;
mod flat;
pub mod particles;
mod space;
pub mod stack;

pub use flat::*;
pub use space::*;
pub use stack::{Animator, Constraint, Effect, Mask, Modifier, Operator, Pattern};

/// Formulas by property name (`{"rotation": "time * 90", "x": "wiggle(2, 30)"}`).
pub type Expressions = BTreeMap<String, String>;

// ---------------------------------------------------------------------------------------------
// Scene

#[derive(Debug, Clone, PartialEq)]
pub enum Scene {
    Flat(Scene2d),
    Space(Scene3d),
}

impl Scene {
    pub fn is_3d(&self) -> bool {
        matches!(self, Scene::Space(_))
    }

    /// Reads a scene from JSON. `"type": "2d"` or `"3d"`; without it, a scene with `objects`,
    /// `camera`, `cameras` or `environment` is 3D and anything else 2D.
    pub fn from_json(v: &Value) -> Result<Scene, String> {
        let obj = v.as_object().ok_or("A scene is a JSON object.")?;
        let three = match obj.get("type").and_then(Value::as_str) {
            Some("2d" | "2D" | "flat") => false,
            Some("3d" | "3D" | "space") => true,
            Some(other) => return Err(format!("Scene type is \"2d\" or \"3d\", not \"{other}\".")),
            None => ["objects", "camera", "cameras", "environment", "materials"].iter().any(|k| obj.contains_key(*k)),
        };
        let mut body = v.clone();
        if let Some(o) = body.as_object_mut() {
            o.remove("type");
        }
        // Unknown fields and types first (a typo in a layer), with hints, before serde's errors.
        if three {
            check_scene3d_keys(&body)?;
        } else {
            check_keys(&body, SCENE2D_KEYS, "the 2D scene")?;
            for l in v.get("layers").and_then(Value::as_array).into_iter().flatten() {
                check_layer_keys(l)?;
            }
            for c in v.get("compositions").and_then(Value::as_array).into_iter().flatten() {
                let id = c.get("id").and_then(Value::as_str).unwrap_or("?");
                check_keys(c, COMPOSITION_KEYS, &format!("composition \"{id}\""))?;
                for l in c.get("layers").and_then(Value::as_array).into_iter().flatten() {
                    check_layer_keys(l)?;
                }
            }
        }
        let scene = if three {
            Scene::Space(serde_json::from_value(body).map_err(|e| format!("3D scene: {e}"))?)
        } else {
            Scene::Flat(serde_json::from_value(body).map_err(|e| format!("2D scene: {e}"))?)
        };
        let mut scene = scene;
        scene.normalize();
        scene.validate()?;
        Ok(scene)
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Sorts keyframes and gives stack items (effects, modifiers…) their ids.
    pub fn normalize(&mut self) {
        match self {
            Scene::Flat(s) => {
                normalize(&mut s.keyframes);
                let mut f = |l: &mut Layer| {
                    normalize(&mut l.keyframes);
                    l.name_items();
                };
                walk_layers_mut(&mut s.layers, &mut f);
                for c in &mut s.compositions {
                    walk_layers_mut(&mut c.layers, &mut f);
                }
            }
            Scene::Space(s) => {
                normalize(&mut s.keyframes);
                for c in std::iter::once(&mut s.camera).chain(s.cameras.iter_mut()) {
                    normalize(&mut c.keyframes);
                    stack::name_items(&mut c.constraints);
                }
                for l in &mut s.lights {
                    normalize(&mut l.keyframes);
                    stack::name_items(&mut l.constraints);
                }
                walk_objects_mut(&mut s.objects, &mut |o| {
                    normalize(&mut o.keyframes);
                    o.name_items();
                });
            }
        }
    }

    /// Unique ids, known keyframed properties of the right kind, colours, references that exist,
    /// formulas that read.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Scene::Flat(s) => validate_flat(s),
            Scene::Space(s) => validate_space(s),
        }
    }

    /// Ids of the layers (2D, compositions' layers included) or objects, lights and extra cameras
    /// (3D), depth first.
    pub fn ids(&self) -> Vec<String> {
        let mut out = vec![];
        match self {
            Scene::Flat(s) => {
                walk_layers(&s.layers, &mut |l| out.push(l.id.clone()));
                for c in &s.compositions {
                    walk_layers(&c.layers, &mut |l| out.push(l.id.clone()));
                }
            }
            Scene::Space(s) => {
                out.extend(s.cameras.iter().map(|c| c.id.clone()));
                out.extend(s.lights.iter().map(|l| l.id.clone()));
                walk_objects(&s.objects, &mut |o| out.push(o.id.clone()));
            }
        }
        out
    }

    /// The latest keyframe time anywhere in the scene (0 without keyframes).
    pub fn last_key_time(&self) -> f64 {
        let mut t = 0.0f64;
        let mut see = |k: &Keyframes| {
            for list in k.values() {
                if let Some(last) = list.last() {
                    t = t.max(last.time);
                }
            }
        };
        match self {
            Scene::Flat(s) => {
                see(&s.keyframes);
                walk_layers(&s.layers, &mut |l| see(&l.keyframes));
            }
            Scene::Space(s) => {
                see(&s.keyframes);
                see(&s.camera.keyframes);
                for c in &s.cameras {
                    see(&c.keyframes);
                }
                for l in &s.lights {
                    see(&l.keyframes);
                }
                walk_objects(&s.objects, &mut |o| see(&o.keyframes));
            }
        }
        t
    }

    /// Asset ids, names or paths the scene draws (image layers, textures, models, panoramas,
    /// image particles).
    pub fn media_refs(&self) -> Vec<String> {
        let mut out = vec![];
        match self {
            Scene::Flat(s) => {
                let mut see = |l: &Layer| match &l.kind {
                    LayerKind::Image { asset, .. } => out.push(asset.clone()),
                    LayerKind::Particles(p) => out.extend(p.asset.clone().filter(|_| p.shape.as_deref() == Some("image"))),
                    _ => {}
                };
                walk_layers(&s.layers, &mut see);
                for c in &s.compositions {
                    walk_layers(&c.layers, &mut see);
                }
            }
            Scene::Space(s) => {
                if let Some(img) = s.environment.as_ref().and_then(|e| e.image.clone()) {
                    out.push(img);
                }
                for m in &s.materials {
                    out.extend(m.texture.clone());
                }
                walk_objects(&s.objects, &mut |o| {
                    match &o.shape {
                        Shape3d::Image { asset, .. } => out.push(asset.clone()),
                        Shape3d::Model { src } => out.push(src.clone()),
                        Shape3d::Particles(p) => out.extend(p.asset.clone().filter(|_| p.shape.as_deref() == Some("image"))),
                        _ => {}
                    }
                    if o.material.from.is_none()
                        && let Some(t) = &o.material.texture
                    {
                        out.push(t.clone());
                    }
                })
            }
        }
        out
    }

    /// Inserts `item` (a layer for 2D; an object, light or camera for 3D) or replaces the one
    /// with its id. New 2D layers go on top; `parent` puts it inside a group or composition (2D)
    /// or object (3D).
    pub fn upsert(&mut self, item: &Value, parent: Option<&str>) -> Result<String, String> {
        match self {
            Scene::Flat(s) => {
                check_layer_keys(item)?;
                let mut layer: Layer = serde_json::from_value(item.clone()).map_err(|e| format!("layer: {e}"))?;
                normalize(&mut layer.keyframes);
                layer.name_items();
                let id = layer.id.clone();
                if let Some(slot) = s.find_layer_mut(&id) {
                    *slot = layer;
                } else {
                    let list = match parent {
                        Some(p) => {
                            if let Some(c) = s.compositions.iter_mut().find(|c| c.id == p) {
                                &mut c.layers
                            } else {
                                match s.find_layer_mut(p) {
                                    Some(Layer { kind: LayerKind::Group { layers }, .. }) => layers,
                                    Some(_) => return Err(format!("\"{p}\" isn't a group or a composition, so it can't hold layers.")),
                                    None => return Err(format!("No layer or composition \"{p}\" in this scene.")),
                                }
                            }
                        }
                        None => &mut s.layers,
                    };
                    list.push(layer);
                }
                Ok(id)
            }
            Scene::Space(s) => {
                let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
                if LIGHT_TYPES.contains(&kind) {
                    check_keys(item, LIGHT_KEYS, &format!("light \"{}\"", item.get("id").and_then(Value::as_str).unwrap_or("?")))?;
                    let mut light: Light = serde_json::from_value(item.clone()).map_err(|e| format!("light: {e}"))?;
                    normalize(&mut light.keyframes);
                    stack::name_items(&mut light.constraints);
                    let id = light.id.clone();
                    match s.lights.iter_mut().find(|l| l.id == id) {
                        Some(slot) => *slot = light,
                        None => s.lights.push(light),
                    }
                    return Ok(id);
                }
                if kind == "camera" {
                    let mut body = item.clone();
                    if let Some(o) = body.as_object_mut() {
                        o.remove("type");
                    }
                    check_keys(&body, CAMERA_KEYS, "the camera")?;
                    let mut cam: Camera = serde_json::from_value(body).map_err(|e| format!("camera: {e}"))?;
                    normalize(&mut cam.keyframes);
                    stack::name_items(&mut cam.constraints);
                    if cam.id.is_empty() || cam.id == "camera" {
                        cam.id = String::new();
                        s.camera = cam;
                        return Ok("camera".into());
                    }
                    let id = cam.id.clone();
                    match s.cameras.iter_mut().find(|c| c.id == id) {
                        Some(slot) => *slot = cam,
                        None => s.cameras.push(cam),
                    }
                    return Ok(id);
                }
                check_object_keys(item)?;
                let mut obj: Object3d = serde_json::from_value(item.clone()).map_err(|e| format!("object: {e}"))?;
                normalize(&mut obj.keyframes);
                obj.name_items();
                let id = obj.id.clone();
                if let Some(slot) = find_object_mut(&mut s.objects, &id) {
                    *slot = obj;
                } else {
                    let list = match parent {
                        Some(p) => &mut find_object_mut(&mut s.objects, p).ok_or_else(|| format!("No object \"{p}\" in this scene."))?.children,
                        None => &mut s.objects,
                    };
                    list.push(obj);
                }
                Ok(id)
            }
        }
    }

    /// Removes the layer, object, light or extra camera with `id`; false if there is none.
    pub fn remove(&mut self, id: &str) -> bool {
        match self {
            Scene::Flat(s) => remove_layer(&mut s.layers, id) || s.compositions.iter_mut().any(|c| remove_layer(&mut c.layers, id)),
            Scene::Space(s) => {
                let before = s.lights.len() + s.cameras.len();
                s.lights.retain(|l| l.id != id);
                s.cameras.retain(|c| c.id != id);
                s.lights.len() + s.cameras.len() != before || remove_object(&mut s.objects, id)
            }
        }
    }

    /// The keyframes of one thing in the scene: a layer/object/light/camera id, `"camera"`, or
    /// `"scene"` for the scene's own (background, ambient…).
    pub fn keyframes_mut(&mut self, id: &str) -> Option<&mut Keyframes> {
        match self.item_mut(id)? {
            ItemMut::Layer(l) => Some(&mut l.keyframes),
            ItemMut::Object(o) => Some(&mut o.keyframes),
            ItemMut::Light(l) => Some(&mut l.keyframes),
            ItemMut::Camera(c) => Some(&mut c.keyframes),
            ItemMut::Flat(s) => Some(&mut s.keyframes),
            ItemMut::Space(s) => Some(&mut s.keyframes),
        }
    }

    /// One layer, object, light or camera as JSON.
    pub fn item_json(&self, id: &str) -> Option<Value> {
        match self {
            Scene::Flat(s) => s.find_layer(id).map(|l| serde_json::to_value(l).unwrap_or(Value::Null)),
            Scene::Space(s) => {
                if let Some(c) = s.camera_by_id(id) {
                    return serde_json::to_value(c).ok();
                }
                if let Some(l) = s.lights.iter().find(|l| l.id == id) {
                    return serde_json::to_value(l).ok();
                }
                let mut found = None;
                walk_objects(&s.objects, &mut |o| {
                    if o.id == id && found.is_none() {
                        found = serde_json::to_value(o).ok();
                    }
                });
                found
            }
        }
    }

    /// Animatable property names of one thing (for the window's keyframe lists and hints).
    pub fn prop_names(&self, id: &str) -> Vec<String> {
        let mut copy = self.clone();
        match copy.item_mut(id) {
            Some(ItemMut::Layer(l)) => l.prop_names(),
            Some(ItemMut::Object(o)) => o.prop_names(),
            Some(ItemMut::Light(l)) => {
                let mut v: Vec<String> = LIGHT_PROPS.iter().map(|p| p.to_string()).collect();
                v.extend(stack::animatable_names(&l.constraints));
                v
            }
            Some(ItemMut::Camera(c)) => {
                let mut v: Vec<String> = CAMERA_PROPS.iter().map(|p| p.to_string()).collect();
                v.extend(stack::animatable_names(&c.constraints));
                v
            }
            Some(ItemMut::Flat(_)) => vec!["background".into()],
            Some(ItemMut::Space(_)) => SCENE3D_PROPS.iter().map(|p| p.to_string()).collect(),
            None => vec![],
        }
    }
}

impl Scene2d {
    /// A layer anywhere: in the scene, its groups or its compositions.
    pub fn find_layer(&self, id: &str) -> Option<&Layer> {
        find_layer(&self.layers, id).or_else(|| self.compositions.iter().find_map(|c| find_layer(&c.layers, id)))
    }

    pub fn find_layer_mut(&mut self, id: &str) -> Option<&mut Layer> {
        if find_layer(&self.layers, id).is_some() {
            return find_layer_mut(&mut self.layers, id);
        }
        self.compositions.iter_mut().find_map(|c| find_layer_mut(&mut c.layers, id))
    }

    pub fn composition(&self, id: &str) -> Option<&Composition> {
        self.compositions.iter().find(|c| c.id == id)
    }

    /// The layer list `id` is in (the scene's, a group's or a composition's), for siblings
    /// (parents, mattes).
    pub fn siblings(&self, id: &str) -> Option<&[Layer]> {
        fn look<'a>(list: &'a [Layer], id: &str) -> Option<&'a [Layer]> {
            if list.iter().any(|l| l.id == id) {
                return Some(list);
            }
            list.iter().find_map(|l| match &l.kind {
                LayerKind::Group { layers } => look(layers, id),
                _ => None,
            })
        }
        look(&self.layers, id).or_else(|| self.compositions.iter().find_map(|c| look(&c.layers, id)))
    }
}

impl Serialize for Scene {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (kind, mut v) = match self {
            Scene::Flat(x) => ("2d", serde_json::to_value(x).map_err(serde::ser::Error::custom)?),
            Scene::Space(x) => ("3d", serde_json::to_value(x).map_err(serde::ser::Error::custom)?),
        };
        if let Some(o) = v.as_object_mut() {
            let mut out = serde_json::Map::new();
            out.insert("type".into(), Value::from(kind));
            out.extend(std::mem::take(o));
            return out.serialize(s);
        }
        v.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Scene {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        // Project files are trusted: no unknown-field check, so older/newer files still open.
        let three = match v.get("type").and_then(Value::as_str) {
            Some(t) => t.eq_ignore_ascii_case("3d"),
            None => ["objects", "camera", "cameras", "environment", "materials"].iter().any(|k| v.get(*k).is_some()),
        };
        let mut body = v;
        if let Some(o) = body.as_object_mut() {
            o.remove("type");
        }
        Ok(if three {
            Scene::Space(serde_json::from_value(body).map_err(serde::de::Error::custom)?)
        } else {
            Scene::Flat(serde_json::from_value(body).map_err(serde::de::Error::custom)?)
        })
    }
}

/// Which template made a motion clip and with what values, so it can be re-made with new ones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateRef {
    pub id: String,
    #[serde(default)]
    pub params: serde_json::Map<String, Value>,
}

// ---------------------------------------------------------------------------------------------
// One thing in a scene, by id: read and change its properties

/// A layer, object, light, camera or the scene itself (`"scene"`), to read and change one
/// property at a time (the inspector, `motion.updateLayer`).
pub enum ItemMut<'a> {
    Layer(&'a mut Layer),
    Object(&'a mut Object3d),
    Light(&'a mut Light),
    Camera(&'a mut Camera),
    Flat(&'a mut Scene2d),
    Space(&'a mut Scene3d),
}

impl Scene {
    pub fn item_mut(&mut self, id: &str) -> Option<ItemMut<'_>> {
        match self {
            Scene::Flat(s) => {
                if id == "scene" {
                    return Some(ItemMut::Flat(s));
                }
                s.find_layer_mut(id).map(ItemMut::Layer)
            }
            Scene::Space(s) => match id {
                "scene" => Some(ItemMut::Space(s)),
                "camera" => Some(ItemMut::Camera(&mut s.camera)),
                _ => {
                    if s.cameras.iter().any(|c| c.id == id) {
                        return s.cameras.iter_mut().find(|c| c.id == id).map(ItemMut::Camera);
                    }
                    if s.lights.iter().any(|l| l.id == id) {
                        return s.lights.iter_mut().find(|l| l.id == id).map(ItemMut::Light);
                    }
                    find_object_mut(&mut s.objects, id).map(ItemMut::Object)
                }
            },
        }
    }

    /// A property's value at scene time `t` (keyframes applied), or `None` if there is no such
    /// thing or property.
    pub fn value(&self, id: &str, name: &str, t: f64) -> Option<KeyValue> {
        let mut copy = self.clone();
        let item = copy.item_mut(id)?;
        let keys = item.keyframes().get(name).and_then(|k| value_at(k, t));
        if keys.is_some() {
            return keys;
        }
        item.get(name)
    }
}

impl ItemMut<'_> {
    pub fn keyframes(&self) -> &Keyframes {
        match self {
            ItemMut::Layer(l) => &l.keyframes,
            ItemMut::Object(o) => &o.keyframes,
            ItemMut::Light(l) => &l.keyframes,
            ItemMut::Camera(c) => &c.keyframes,
            ItemMut::Flat(s) => &s.keyframes,
            ItemMut::Space(s) => &s.keyframes,
        }
    }

    pub fn keyframes_mut(&mut self) -> &mut Keyframes {
        match self {
            ItemMut::Layer(l) => &mut l.keyframes,
            ItemMut::Object(o) => &mut o.keyframes,
            ItemMut::Light(l) => &mut l.keyframes,
            ItemMut::Camera(c) => &mut c.keyframes,
            ItemMut::Flat(s) => &mut s.keyframes,
            ItemMut::Space(s) => &mut s.keyframes,
        }
    }

    /// The thing's expressions (none for the scene itself).
    pub fn expressions_mut(&mut self) -> Option<&mut Expressions> {
        match self {
            ItemMut::Layer(l) => Some(&mut l.expressions),
            ItemMut::Object(o) => Some(&mut o.expressions),
            ItemMut::Light(l) => Some(&mut l.expressions),
            ItemMut::Camera(c) => Some(&mut c.expressions),
            ItemMut::Flat(_) | ItemMut::Space(_) => None,
        }
    }

    /// Sets an animatable property (checked like a keyframe value).
    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        match self {
            ItemMut::Layer(l) => l.set(name, v),
            ItemMut::Object(o) => o.set(name, v),
            ItemMut::Light(l) => l.set(name, v),
            ItemMut::Camera(c) => c.set(name, v),
            ItemMut::Flat(s) => match name {
                "background" => {
                    let c = v.as_str().ok_or("background takes a colour")?;
                    check_color(c, "background")?;
                    s.background = Some(c.to_string());
                    Ok(())
                }
                _ => Err(format!("A 2D scene's own property is background, not `{name}`.")),
            },
            ItemMut::Space(s) => set_scene3d(s, name, v),
        }
    }

    /// The still value of a property (without keyframes).
    pub fn get(&self, name: &str) -> Option<KeyValue> {
        match self {
            ItemMut::Layer(l) => l.get(name),
            ItemMut::Object(o) => o.get(name),
            ItemMut::Light(l) => l.get(name),
            ItemMut::Camera(c) => c.get(name),
            ItemMut::Flat(s) => match name {
                "background" => s.background.as_deref().map(KeyValue::from),
                _ => None,
            },
            ItemMut::Space(s) => get_scene3d(s, name),
        }
    }
}

fn set_scene3d(s: &mut Scene3d, name: &str, v: &KeyValue) -> Result<(), String> {
    let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
    let c = || -> Result<String, String> {
        let c = v.as_str().ok_or_else(|| format!("{name} takes a colour"))?;
        check_color(c, name)?;
        Ok(c.to_string())
    };
    fn env(s: &mut Scene3d) -> &mut Environment {
        s.environment.get_or_insert_with(|| serde_json::from_value(serde_json::json!({})).expect("defaults"))
    }
    match name {
        "background" => s.background = Some(c()?),
        "ambient" => s.ambient = n()?,
        "ambientColor" => s.ambient_color = c()?,
        "activeCamera" => {
            let id = v.as_str().ok_or("activeCamera takes a camera's id")?;
            if s.camera_by_id(id).is_none() {
                let ids: Vec<&str> = std::iter::once("camera").chain(s.cameras.iter().map(|c| c.id.as_str())).collect();
                return Err(format!("no camera \"{id}\"; cameras: {}", ids.join(", ")));
            }
            s.active_camera = Some(id.to_string());
        }
        "environment.strength" => env(s).strength = n()?,
        "environment.rotation" => env(s).rotation = n()?,
        "environment.color" => env(s).color = c()?,
        "environment.top" => env(s).top = c()?,
        "environment.horizon" => env(s).horizon = c()?,
        "environment.bottom" => env(s).bottom = c()?,
        "render.exposure" => s.render.exposure = n()?,
        "render.bloom" => s.render.bloom = n()?,
        "render.bloomThreshold" => s.render.bloom_threshold = n()?,
        "render.bloomRadius" => s.render.bloom_radius = n()?,
        "render.motionBlur" => s.render.motion_blur = n()?,
        "render.ambientOcclusion" => s.render.ambient_occlusion = n()?,
        _ => return Err(format!("A 3D scene's own properties are {}, not `{name}`.", SCENE3D_PROPS.join(", "))),
    }
    Ok(())
}

fn get_scene3d(s: &Scene3d, name: &str) -> Option<KeyValue> {
    let n = |v: f64| Some(KeyValue::Number(v));
    let c = |v: &str| Some(KeyValue::from(v));
    let e = s.environment.as_ref();
    match name {
        "background" => s.background.as_deref().map(KeyValue::from),
        "ambient" => n(s.ambient),
        "ambientColor" => c(&s.ambient_color),
        "activeCamera" => c(s.active_camera.as_deref().unwrap_or("camera")),
        "environment.strength" => n(e.map_or(1.0, |e| e.strength)),
        "environment.rotation" => n(e.map_or(0.0, |e| e.rotation)),
        "environment.color" => e.and_then(|e| c(&e.color)),
        "environment.top" => e.and_then(|e| c(&e.top)),
        "environment.horizon" => e.and_then(|e| c(&e.horizon)),
        "environment.bottom" => e.and_then(|e| c(&e.bottom)),
        "render.exposure" => n(s.render.exposure),
        "render.bloom" => n(s.render.bloom),
        "render.bloomThreshold" => n(s.render.bloom_threshold),
        "render.bloomRadius" => n(s.render.bloom_radius),
        "render.motionBlur" => n(s.render.motion_blur),
        "render.ambientOcclusion" => n(s.render.ambient_occlusion),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Walking

pub fn walk_layers(layers: &[Layer], f: &mut impl FnMut(&Layer)) {
    for l in layers {
        f(l);
        if let LayerKind::Group { layers } = &l.kind {
            walk_layers(layers, f);
        }
    }
}

pub fn walk_layers_mut(layers: &mut [Layer], f: &mut impl FnMut(&mut Layer)) {
    for l in layers {
        f(l);
        if let LayerKind::Group { layers } = &mut l.kind {
            walk_layers_mut(layers, f);
        }
    }
}

pub fn walk_objects(objects: &[Object3d], f: &mut impl FnMut(&Object3d)) {
    for o in objects {
        f(o);
        walk_objects(&o.children, f);
    }
}

pub fn walk_objects_mut(objects: &mut [Object3d], f: &mut impl FnMut(&mut Object3d)) {
    for o in objects {
        f(o);
        walk_objects_mut(&mut o.children, f);
    }
}

pub fn find_layer<'a>(layers: &'a [Layer], id: &str) -> Option<&'a Layer> {
    for l in layers {
        if l.id == id {
            return Some(l);
        }
        if let LayerKind::Group { layers } = &l.kind
            && let Some(found) = find_layer(layers, id)
        {
            return Some(found);
        }
    }
    None
}

fn find_layer_mut<'a>(layers: &'a mut [Layer], id: &str) -> Option<&'a mut Layer> {
    for l in layers {
        if l.id == id {
            return Some(l);
        }
        if let LayerKind::Group { layers } = &mut l.kind
            && let Some(found) = find_layer_mut(layers, id)
        {
            return Some(found);
        }
    }
    None
}

fn remove_layer(layers: &mut Vec<Layer>, id: &str) -> bool {
    let before = layers.len();
    layers.retain(|l| l.id != id);
    if layers.len() != before {
        return true;
    }
    layers.iter_mut().any(|l| match &mut l.kind {
        LayerKind::Group { layers } => remove_layer(layers, id),
        _ => false,
    })
}

pub fn find_object<'a>(objects: &'a [Object3d], id: &str) -> Option<&'a Object3d> {
    for o in objects {
        if o.id == id {
            return Some(o);
        }
        if let Some(found) = find_object(&o.children, id) {
            return Some(found);
        }
    }
    None
}

pub fn find_object_mut<'a>(objects: &'a mut [Object3d], id: &str) -> Option<&'a mut Object3d> {
    for o in objects {
        if o.id == id {
            return Some(o);
        }
        if let Some(found) = find_object_mut(&mut o.children, id) {
            return Some(found);
        }
    }
    None
}

fn remove_object(objects: &mut Vec<Object3d>, id: &str) -> bool {
    let before = objects.len();
    objects.retain(|o| o.id != id);
    objects.len() != before || objects.iter_mut().any(|o| remove_object(&mut o.children, id))
}

// ---------------------------------------------------------------------------------------------
// Checking

fn validate_flat(s: &Scene2d) -> Result<(), String> {
    check_color_opt(&s.background, "background")?;
    check_scene_keys(&s.keyframes, &["background"])?;
    let mut ids = HashSet::new();
    let mut comps = HashSet::new();
    for c in &s.compositions {
        if c.id.trim().is_empty() || c.id == "scene" {
            return Err("Every composition needs an id (not \"scene\").".into());
        }
        if !comps.insert(c.id.as_str()) {
            return Err(format!("Two compositions have the id \"{}\".", c.id));
        }
        check_color_opt(&c.background, &format!("composition \"{}\" background", c.id))?;
    }
    validate_layers(&s.layers, &mut ids, &comps)?;
    for c in &s.compositions {
        validate_layers(&c.layers, &mut ids, &comps)?;
    }
    // Compositions can't show themselves, directly or through others.
    for c in &s.compositions {
        let mut path = vec![c.id.as_str()];
        check_comp_loop(s, c, &mut path)?;
    }
    Ok(())
}

fn check_comp_loop<'a>(s: &'a Scene2d, c: &'a Composition, path: &mut Vec<&'a str>) -> Result<(), String> {
    let mut result = Ok(());
    walk_layers(&c.layers, &mut |l| {
        if result.is_err() {
            return;
        }
        if let LayerKind::Comp { comp, .. } = &l.kind
            && let Some(inner) = s.composition(comp)
        {
            if path.contains(&inner.id.as_str()) {
                result = Err(format!("Composition \"{}\" shows itself (through {}); compositions can't contain themselves.", inner.id, path.join(" → ")));
                return;
            }
            path.push(inner.id.as_str());
            result = check_comp_loop(s, inner, path);
            path.pop();
        }
    });
    result
}

/// One list of sibling layers (and their groups): ids, references, parents.
fn validate_layers(layers: &[Layer], ids: &mut HashSet<String>, comps: &HashSet<&str>) -> Result<(), String> {
    let mut all = HashSet::new();
    walk_layers(layers, &mut |l| {
        all.insert(l.id.clone());
    });
    let mut result = Ok(());
    walk_layers(layers, &mut |l| {
        if result.is_err() {
            return;
        }
        result = validate_layer(l, ids, &all, comps);
    });
    result?;
    check_parents(layers)?;
    let mut nested = Ok(());
    walk_layers(layers, &mut |l| {
        if nested.is_ok()
            && let LayerKind::Group { layers } = &l.kind
        {
            nested = check_parents(layers);
        }
    });
    nested
}

/// Parents are siblings, and no layer is its own ancestor.
fn check_parents(list: &[Layer]) -> Result<(), String> {
    for l in list {
        let mut seen = vec![l.id.as_str()];
        let mut at = l;
        while let Some(p) = &at.parent {
            let Some(next) = list.iter().find(|x| x.id == *p) else {
                return Err(format!("layer \"{}\" has the parent \"{p}\", which isn't a layer next to it (parents are in the same group or composition)", at.id));
            };
            if seen.contains(&next.id.as_str()) {
                return Err(format!("layer \"{}\": parents go round in a circle ({} → {})", l.id, seen.join(" → "), next.id));
            }
            seen.push(next.id.as_str());
            at = next;
        }
    }
    Ok(())
}

fn validate_layer(l: &Layer, ids: &mut HashSet<String>, all: &HashSet<String>, comps: &HashSet<&str>) -> Result<(), String> {
    let id = &l.id;
    if id.trim().is_empty() {
        return Err("Every layer needs an id.".into());
    }
    if id == "scene" {
        return Err("\"scene\" is the scene itself; give the layer another id.".into());
    }
    if !ids.insert(id.clone()) {
        return Err(format!("Two layers have the id \"{id}\"; ids must be unique."));
    }
    if let Some(Fill::Color(c)) = &l.fill {
        check_color(c, &format!("layer \"{id}\" fill"))?;
    }
    if let Some(Fill::Gradient(g)) = &l.fill {
        if !matches!(g.kind.as_str(), "linear" | "radial") {
            return Err(format!("layer \"{id}\": a gradient's type is \"linear\" or \"radial\""));
        }
        if g.stops.len() < 2 {
            return Err(format!("layer \"{id}\": a gradient needs at least two stops, like [[0, \"#ff5a36\"], [1, \"#1a1a1a\"]]"));
        }
        for (_, c) in &g.stops {
            check_color(c, &format!("layer \"{id}\" gradient stop"))?;
        }
    }
    if let Some(s) = &l.stroke {
        check_color(&s.color, &format!("layer \"{id}\" stroke"))?;
        if !matches!(s.cap.as_str(), "butt" | "round" | "square") {
            return Err(format!("layer \"{id}\": stroke cap is butt, round or square"));
        }
        if !matches!(s.join.as_str(), "miter" | "round" | "bevel") {
            return Err(format!("layer \"{id}\": stroke join is miter, round or bevel"));
        }
    }
    if let Some(s) = &l.shadow {
        check_color(&s.color, &format!("layer \"{id}\" shadow"))?;
    }
    if let Some(g) = &l.glow {
        check_color(&g.color, &format!("layer \"{id}\" glow"))?;
    }
    if let Some(m) = &l.mask {
        if m == id {
            return Err(format!("layer \"{id}\" can't mask itself"));
        }
        if !all.contains(m) {
            return Err(format!("layer \"{id}\" is masked by \"{m}\", which isn't a layer next to it"));
        }
    }
    if let Some(m) = &l.matte {
        if m.layer == *id {
            return Err(format!("layer \"{id}\" can't be its own matte"));
        }
        if !all.contains(&m.layer) {
            return Err(format!("layer \"{id}\" uses \"{}\" as its matte, which isn't a layer next to it", m.layer));
        }
        if !MATTE_MODES.contains(&m.mode.as_str()) {
            return Err(format!("layer \"{id}\": matte mode is one of {}", MATTE_MODES.join(", ")));
        }
    }
    let owner = format!("layer \"{id}\"");
    let mut refs = vec![];
    stack::check_items(&l.masks, &owner, &mut refs)?;
    stack::check_items(&l.effects, &owner, &mut refs)?;
    stack::check_items(&l.operators, &owner, &mut refs)?;
    match &l.kind {
        LayerKind::Text(t) => {
            if let Some(r) = &t.reveal {
                if !matches!(r.by.as_str(), "char" | "word" | "line") {
                    return Err(format!("layer \"{id}\": reveal.by is char, word or line"));
                }
                if !REVEAL_STYLES.contains(&r.style.as_str()) {
                    return Err(format!("layer \"{id}\": reveal.style is one of {}", REVEAL_STYLES.join(", ")));
                }
            }
            stack::check_items(&t.animators, &owner, &mut refs)?;
            if let Some(p) = &t.path {
                crate::path::parse(p).map_err(|e| format!("layer \"{id}\": text path: {e}"))?;
            }
        }
        LayerKind::Path { d, .. } if !d.is_empty() => {
            crate::path::parse(d).map_err(|e| format!("layer \"{id}\": path data: {e}"))?;
        }
        LayerKind::Comp { comp, .. } if !comps.contains(comp.as_str()) => {
            let names: Vec<&str> = comps.iter().copied().collect();
            let hint = crate::closest(comp, &names).map(|c| format!(" Did you mean \"{c}\"?")).unwrap_or_default();
            return Err(format!("layer \"{id}\" shows composition \"{comp}\", which isn't in the scene's compositions.{hint}"));
        }
        LayerKind::Particles(p) => p.check().map_err(|e| format!("layer \"{id}\": {e}"))?,
        _ => {}
    }
    check_expressions(&l.expressions, &owner, |name| {
        let v = l.get(name).ok_or_else(|| format!("no property `{name}`"))?;
        l.clone().set(name, &v)
    })?;
    let mut probe = l.clone();
    for (name, keys) in &l.keyframes {
        for k in keys {
            probe.set(name, &k.value).map_err(|e| format!("layer \"{id}\" keyframes: {e}"))?;
            if let (LayerKind::Path { .. }, "d", Some(d)) = (&l.kind, name.as_str(), k.value.as_str()) {
                crate::path::parse(d).map_err(|e| format!("layer \"{id}\" keyframe d: {e}"))?;
            }
        }
    }
    Ok(())
}

pub const REVEAL_STYLES: &[&str] = &["fade", "rise", "drop", "slide", "pop", "type", "blur"];

fn validate_space(s: &Scene3d) -> Result<(), String> {
    check_color_opt(&s.background, "background")?;
    check_color(&s.ambient_color, "ambientColor")?;
    for (name, list) in &s.keyframes {
        if !SCENE3D_PROPS.contains(&name.as_str()) {
            return Err(format!("The scene's own keyframes can animate {}, not `{name}`.", SCENE3D_PROPS.join(", ")));
        }
        let mut probe = s.clone();
        for k in list {
            set_scene3d(&mut probe, name, &k.value).map_err(|e| format!("scene keyframes: {e}"))?;
        }
    }
    if let Some(e) = &s.environment {
        if !ENVIRONMENT_TYPES.contains(&e.kind.as_str()) {
            return Err(format!("environment type is one of {}", ENVIRONMENT_TYPES.join(", ")));
        }
        for (c, what) in [(&e.color, "color"), (&e.top, "top"), (&e.horizon, "horizon"), (&e.bottom, "bottom")] {
            check_color(c, &format!("environment {what}"))?;
        }
        if e.kind == "image" && e.image.is_none() {
            return Err("an image environment needs an image (a panorama picture)".into());
        }
    }
    if !matches!(s.render.engine.as_str(), "standard" | "path") {
        return Err(format!("render.engine is \"standard\" or \"path\", not \"{}\"", s.render.engine));
    }
    if !matches!(s.render.tone_mapping.as_str(), "standard" | "filmic") {
        return Err(format!("render.toneMapping is \"standard\" or \"filmic\", not \"{}\"", s.render.tone_mapping));
    }
    let mut refs = vec![];
    let mut mats = HashSet::new();
    for m in &s.materials {
        let Some(id) = m.id.as_deref().filter(|i| !i.trim().is_empty()) else {
            return Err("Every shared material needs an id (objects use it: \"material\": \"gold\").".into());
        };
        if !mats.insert(id) {
            return Err(format!("Two materials have the id \"{id}\"."));
        }
        m.check(&format!("material \"{id}\""), &mut refs)?;
    }
    let mut ids: HashSet<String> = HashSet::from(["camera".to_string(), "scene".to_string()]);
    let cams = std::iter::once(&s.camera).chain(s.cameras.iter());
    for (i, c) in cams.enumerate() {
        let id = if i == 0 { "camera".to_string() } else { c.id.clone() };
        if i > 0 && (id.trim().is_empty() || !ids.insert(id.clone())) {
            return Err(format!("Every extra camera needs its own id (\"{id}\" is empty or taken)."));
        }
        if !matches!(c.projection.as_str(), "perspective" | "orthographic") {
            return Err(format!("camera \"{id}\": projection is perspective or orthographic"));
        }
        stack::check_items(&c.constraints, &format!("camera \"{id}\""), &mut refs)?;
        let mut probe = c.clone();
        for (name, keys) in &c.keyframes {
            for k in keys {
                probe.set(name, &k.value).map_err(|e| format!("camera \"{id}\" keyframes: {e}"))?;
            }
        }
        check_expressions(&c.expressions, &format!("camera \"{id}\""), |n| c.clone().set(n, &c.get(n).unwrap_or(KeyValue::Number(0.0))))?;
    }
    if let Some(a) = &s.active_camera
        && s.camera_by_id(a).is_none()
    {
        return Err(format!("activeCamera \"{a}\" isn't a camera in this scene"));
    }
    for l in &s.lights {
        if l.id.trim().is_empty() || !ids.insert(l.id.clone()) {
            return Err(format!("Every light needs its own id (\"{}\" is empty or taken).", l.id));
        }
        if !LIGHT_TYPES.contains(&l.kind.as_str()) {
            return Err(format!("light \"{}\": type is one of {}", l.id, LIGHT_TYPES.join(", ")));
        }
        check_color(&l.color, &format!("light {} color", l.id))?;
        stack::check_items(&l.constraints, &format!("light \"{}\"", l.id), &mut refs)?;
        let mut probe = l.clone();
        for (name, keys) in &l.keyframes {
            for k in keys {
                probe.set(name, &k.value).map_err(|e| format!("light \"{}\": {e}", l.id))?;
            }
        }
        check_expressions(&l.expressions, &format!("light \"{}\"", l.id), |n| l.clone().set(n, &l.get(n).unwrap_or(KeyValue::Number(0.0))))?;
    }
    let mut result = Ok(());
    walk_objects(&s.objects, &mut |o| {
        if result.is_err() {
            return;
        }
        result = validate_object(o, &mut ids, &mut refs);
    });
    result?;
    // References: materials by id, objects by id (curves for followPath).
    for r in refs {
        if let Some(m) = r.strip_prefix("material:") {
            if !mats.contains(m) {
                let names: Vec<&str> = mats.iter().copied().collect();
                let hint = crate::closest(m, &names).map(|c| format!(" Did you mean \"{c}\"?")).unwrap_or_default();
                return Err(format!("Material \"{m}\" isn't in the scene's materials.{hint}"));
            }
        } else if find_object(&s.objects, &r).is_none() && s.camera_by_id(&r).is_none() && !s.lights.iter().any(|l| l.id == r) {
            return Err(format!("\"{r}\" is used (by a modifier or constraint) but isn't an object in this scene"));
        }
    }
    let mut paths = Ok(());
    walk_objects(&s.objects, &mut |o| {
        for c in o.constraints.iter().filter(|c| c.kind == "followPath") {
            if let Some(p) = c.opt_s("path")
                && !matches!(find_object(&s.objects, &p).map(|x| &x.shape), Some(Shape3d::Curve { .. }))
                && paths.is_ok()
            {
                paths = Err(format!("object \"{}\" follows \"{p}\", which isn't a curve", o.id));
            }
        }
    });
    paths
}

fn validate_object(o: &Object3d, ids: &mut HashSet<String>, refs: &mut Vec<String>) -> Result<(), String> {
    if o.id.trim().is_empty() {
        return Err("Every object needs an id.".to_string());
    }
    if !ids.insert(o.id.clone()) {
        return Err(format!("Two things have the id \"{}\"; ids must be unique.", o.id));
    }
    let owner = format!("object \"{}\"", o.id);
    o.material.check(&owner, refs)?;
    o.shape.check(&o.id)?;
    stack::check_items(&o.modifiers, &owner, refs)?;
    stack::check_items(&o.constraints, &owner, refs)?;
    if let Some(m) = o.modifiers.iter().find(|m| m.kind == "boolean" && m.opt_s("object").as_deref() == Some(o.id.as_str())) {
        return Err(format!("{owner}: the boolean \"{}\" can't use the object itself", m.id));
    }
    check_expressions(&o.expressions, &owner, |n| {
        let mut p = o.clone();
        let v = o.get(n).ok_or_else(|| format!("no property `{n}`"))?;
        p.set(n, &v)
    })?;
    let mut probe = o.clone();
    for (name, keys) in &o.keyframes {
        for k in keys {
            probe.set(name, &k.value).map_err(|e| format!("{owner} keyframes: {e}"))?;
        }
    }
    Ok(())
}

/// Formulas read, and each drives a property that exists (`known` checks the name).
fn check_expressions(ex: &Expressions, owner: &str, known: impl Fn(&str) -> Result<(), String>) -> Result<(), String> {
    for (name, src) in ex {
        known(name).map_err(|e| format!("{owner}: the expression for `{name}`: {e}"))?;
        crate::expr::check(src).map_err(|e| format!("{owner}: the expression for `{name}`: {e}"))?;
    }
    Ok(())
}

fn check_scene_keys(keys: &Keyframes, allowed: &[&str]) -> Result<(), String> {
    for name in keys.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(format!("The scene's own keyframes can animate {}, not `{name}`.", allowed.join(", ")));
        }
    }
    for (name, list) in keys {
        for k in list {
            match name.as_str() {
                "background" | "ambientColor" => check_color(k.value.as_str().ok_or("background takes a colour")?, name)?,
                _ => {
                    k.value.as_f64().ok_or_else(|| format!("{name} takes a number"))?;
                }
            }
        }
    }
    Ok(())
}

pub fn check_color(c: &str, what: &str) -> Result<(), String> {
    if Rgba::parse(c).is_some() || c.eq_ignore_ascii_case("transparent") {
        Ok(())
    } else {
        Err(format!("{what}: \"{c}\" isn't a colour; use #rrggbb or #rrggbbaa"))
    }
}

fn check_color_opt(c: &Option<String>, what: &str) -> Result<(), String> {
    c.as_deref().map_or(Ok(()), |c| check_color(c, what))
}

fn check_keys(v: &Value, allowed: &[&str], what: &str) -> Result<(), String> {
    let Some(o) = v.as_object() else { return Ok(()) };
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            let hint = crate::closest(k, allowed).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
            return Err(format!("Unknown field `{k}` in {what}.{hint} Fields: {}.", allowed.join(", ")));
        }
    }
    Ok(())
}

fn check_layer_keys(v: &Value) -> Result<(), String> {
    let id = v.get("id").and_then(Value::as_str).unwrap_or("?");
    let Some(kind) = v.get("type").and_then(Value::as_str) else {
        return Err(format!(
            "layer \"{id}\" needs a type: {}.",
            KIND_KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
        ));
    };
    let Some((_, own)) = KIND_KEYS.iter().find(|(k, _)| *k == kind) else {
        let names: Vec<&str> = KIND_KEYS.iter().map(|(k, _)| *k).collect();
        let hint = crate::closest(kind, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        return Err(format!("layer \"{id}\": unknown type `{kind}`.{hint} Types: {}.", names.join(", ")));
    };
    let allowed: Vec<&str> = LAYER_KEYS.iter().chain(own.iter()).copied().collect();
    check_keys(v, &allowed, &format!("{kind} layer \"{id}\""))?;
    if let Some(r) = v.get("reveal") {
        check_keys(r, &["by", "style", "progress", "overlap", "distance"], &format!("the reveal of \"{id}\""))?;
    }
    if let Some(s) = v.get("stroke").filter(|s| s.is_object()) {
        check_keys(s, &["color", "width", "cap", "join", "dash", "dashOffset"], &format!("the stroke of \"{id}\""))?;
    }
    if let Some(s) = v.get("shadow") {
        check_keys(s, &["color", "blur", "x", "y"], &format!("the shadow of \"{id}\""))?;
    }
    if let Some(s) = v.get("glow") {
        check_keys(s, &["color", "radius", "strength"], &format!("the glow of \"{id}\""))?;
    }
    if let Some(m) = v.get("matte").filter(|m| m.is_object()) {
        check_keys(m, &["layer", "mode"], &format!("the matte of \"{id}\""))?;
    }
    if kind == "group" {
        for child in v.get("layers").and_then(Value::as_array).into_iter().flatten() {
            check_layer_keys(child)?;
        }
    }
    Ok(())
}

fn check_scene3d_keys(body: &Value) -> Result<(), String> {
    check_keys(body, SCENE3D_KEYS, "the 3D scene")?;
    if let Some(c) = body.get("camera") {
        check_keys(c, CAMERA_KEYS, "the camera")?;
    }
    for c in body.get("cameras").and_then(Value::as_array).into_iter().flatten() {
        check_keys(c, CAMERA_KEYS, &format!("camera \"{}\"", c.get("id").and_then(Value::as_str).unwrap_or("?")))?;
    }
    for l in body.get("lights").and_then(Value::as_array).into_iter().flatten() {
        check_keys(l, LIGHT_KEYS, &format!("light \"{}\"", l.get("id").and_then(Value::as_str).unwrap_or("?")))?;
    }
    if let Some(e) = body.get("environment") {
        check_keys(e, ENVIRONMENT_KEYS, "the environment")?;
    }
    if let Some(r) = body.get("render") {
        check_keys(r, RENDER_KEYS, "the render settings")?;
    }
    for m in body.get("materials").and_then(Value::as_array).into_iter().flatten() {
        check_keys(m, MATERIAL_KEYS, &format!("material \"{}\"", m.get("id").and_then(Value::as_str).unwrap_or("?")))?;
    }
    for o in body.get("objects").and_then(Value::as_array).into_iter().flatten() {
        check_object_keys(o)?;
    }
    Ok(())
}

fn check_object_keys(v: &Value) -> Result<(), String> {
    let id = v.get("id").and_then(Value::as_str).unwrap_or("?");
    let names: Vec<&str> = SHAPE_KEYS.iter().map(|(k, _)| *k).collect();
    let Some(kind) = v.get("type").and_then(Value::as_str) else {
        return Err(format!("object \"{id}\" needs a type: {}.", names.join(", ")));
    };
    let Some((_, own)) = SHAPE_KEYS.iter().find(|(k, _)| *k == kind) else {
        let hint = crate::closest(kind, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        let extra = if LIGHT_TYPES.contains(&kind) { " Lights go in `lights`." } else if kind == "camera" { " Cameras go in `camera` / `cameras`." } else { "" };
        return Err(format!("object \"{id}\": unknown type `{kind}`.{hint} Types: {}.{extra}", names.join(", ")));
    };
    let allowed: Vec<&str> = OBJECT_KEYS.iter().chain(own.iter()).copied().collect();
    check_keys(v, &allowed, &format!("{kind} \"{id}\""))?;
    if let Some(m) = v.get("material").filter(|m| m.is_object()) {
        check_keys(m, MATERIAL_KEYS, &format!("the material of \"{id}\""))?;
    }
    for child in v.get("children").and_then(Value::as_array).into_iter().flatten() {
        check_object_keys(child)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// serde defaults

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}
fn is_false(v: &bool) -> bool {
    !*v
}
fn is_true(v: &bool) -> bool {
    *v
}
fn yes() -> bool {
    true
}
fn one() -> f64 {
    1.0
}
fn two() -> f64 {
    2.0
}
fn three() -> f64 {
    3.0
}
fn four() -> f64 {
    4.0
}
fn five() -> f64 {
    5.0
}
fn six() -> f64 {
    6.0
}
fn twelve() -> f64 {
    12.0
}
fn twenty() -> f64 {
    20.0
}
fn forty() -> f64 {
    40.0
}
fn fifty() -> f64 {
    50.0
}
fn hundred() -> f64 {
    100.0
}
fn half() -> f64 {
    0.5
}
fn tube() -> f64 {
    0.2
}
fn ambient() -> f64 {
    0.25
}
fn seventy_two() -> f64 {
    72.0
}
fn bold() -> f64 {
    700.0
}
fn line_height() -> f64 {
    1.15
}
fn manrope() -> String {
    "Manrope".into()
}
fn white() -> String {
    "#ffffff".into()
}
fn grey() -> String {
    "#d9d9d9".into()
}
fn shadow_color() -> String {
    "#00000080".into()
}
fn round() -> String {
    "round".into()
}
fn linear() -> String {
    "linear".into()
}
fn by_char() -> String {
    "char".into()
}
fn rise() -> String {
    "rise".into()
}
fn directional() -> String {
    "directional".into()
}
fn camera_position() -> Vec3 {
    Vec3([0.0, 0.0, 8.0])
}
fn light_position() -> Vec3 {
    Vec3([3.0, 4.0, 5.0])
}
fn light_direction() -> Vec3 {
    Vec3([-0.5, -1.0, -0.7])
}

fn eight() -> f64 {
    8.0
}
fn is_eight(v: &f64) -> bool {
    *v == 8.0
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_2d_scene_and_animates_it() {
        let s = Scene::from_json(&json!({
            "background": "#101014",
            "layers": [
                {"id": "card", "type": "rect", "width": 600, "height": 160, "radius": 24, "fill": "#ff5a36",
                 "keyframes": {"x": [[0, -900], [0.6, 0, "easeOutBack"]], "opacity": [[0, 0], [0.3, 1]]}},
                {"id": "title", "type": "text", "text": "Hello", "fontSize": 96, "fill": "#ffffff",
                 "reveal": {"by": "char", "style": "rise"}, "keyframes": {"reveal": [[0.2, 0], [1.2, 1, "easeOut"]]}}
            ]
        }))
        .unwrap();
        let Scene::Flat(flat) = &s else { panic!("2d") };
        let card = flat.layers[0].at(0.1);
        assert!(card.x > -900.0 && card.x < 0.0);
        assert!(flat.layers[0].at(0.3).x > 0.0, "easeOutBack overshoots");
        assert_eq!(flat.layers[0].at(0.3).opacity, 1.0);
        assert_eq!(flat.layers[0].at(5.0).x, 0.0);
        let LayerKind::Text(t) = &flat.layers[1].at(0.2).kind else { panic!("text") };
        assert_eq!(t.reveal.as_ref().unwrap().progress, 0.0);
        assert!((s.last_key_time() - 1.2).abs() < 1e-9);
        // Round trip through the project file format.
        let back: Scene = serde_json::from_value(s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(s.to_json()["type"], "2d");
    }

    #[test]
    fn explains_mistakes() {
        let err = |v: Value| Scene::from_json(&v).unwrap_err();
        assert!(err(json!({"layers": [{"id": "a", "type": "rectangle"}]})).contains("Did you mean `rect`"));
        assert!(err(json!({"layers": [{"id": "a", "type": "rect", "colour": "#fff"}]})).contains("Unknown field `colour`"));
        assert!(err(json!({"layers": [{"id": "a", "type": "rect"}, {"id": "a", "type": "ellipse"}]})).contains("unique"));
        assert!(err(json!({"layers": [{"id": "a", "type": "rect", "keyframes": {"wobble": [[0, 1]]}}]})).contains("no animatable property `wobble`"));
        assert!(err(json!({"layers": [{"id": "a", "type": "rect", "fill": "red-ish"}]})).contains("isn't a colour"));
        assert!(err(json!({"layers": [{"id": "a", "type": "rect", "keyframes": {"x": [[0, 0], [1, 5, "boing"]]}}]})).contains("easing"));
        assert!(err(json!({"layers": [{"id": "a", "type": "rect", "mask": "b"}]})).contains("isn't a layer"));
        assert!(err(json!({"layers": [{"id": "p", "type": "path", "d": "M 0 0 Q"}]})).contains("path data"));
    }

    #[test]
    fn reads_a_3d_scene() {
        let s = Scene::from_json(&json!({
            "camera": {"position": [0, 2, 8], "keyframes": {"position.x": [[0, -2], [4, 2, "easeInOut"]]}},
            "lights": [{"id": "sun", "type": "directional", "direction": [-1, -1, -1]}],
            "objects": [
                {"id": "logo", "type": "text", "text": "KIMCHI", "size": 1.2, "depth": 0.3,
                 "material": {"color": "#ff5a36", "metallic": 0.3, "roughness": 0.3},
                 "keyframes": {"rotation.y": [[0, -30], [4, 30]], "scale": [[0, 0.8], [1, 1, "easeOutBack"]]}},
                {"id": "floor", "type": "plane", "width": 20, "height": 20, "rotation": [-90, 0, 0], "scale": 1}
            ]
        }))
        .unwrap();
        assert!(s.is_3d());
        let Scene::Space(sp) = &s else { panic!("3d") };
        assert_eq!(sp.camera.at(2.0).position.0[0], 0.0);
        let logo = sp.objects[0].at(2.0);
        assert!((logo.rotation.0[1]).abs() < 1e-9);
        assert_eq!(logo.scale, Vec3::one());
        assert_eq!(s.ids(), vec!["sun", "logo", "floor"]);
        let back: Scene = serde_json::from_value(s.to_json()).unwrap();
        assert_eq!(back, s);
        let err = Scene::from_json(&json!({"objects": [{"id": "c", "type": "cube"}]})).unwrap_err();
        assert!(err.contains("unknown type `cube`"), "{err}");
    }

    #[test]
    fn variant_fields_are_camel_case() {
        let s = Scene::from_json(&json!({"layers": [{"id": "s", "type": "star", "innerRadius": 50}]})).unwrap();
        assert_eq!(s.value("s", "innerRadius", 0.0), Some(KeyValue::Number(50.0)));
        assert_eq!(s.item_json("s").unwrap()["innerRadius"], 50.0);
        let t = Scene::from_json(&json!({"objects": [{"id": "t", "type": "text", "text": "A", "fontFamily": "IBM Plex Mono", "fontWeight": 400, "letterSpacing": 0.1}]})).unwrap();
        let o = t.item_json("t").unwrap();
        assert_eq!((o["fontFamily"].as_str(), o["fontWeight"].as_f64(), o["letterSpacing"].as_f64()), (Some("IBM Plex Mono"), Some(400.0), Some(0.1)));
    }

    #[test]
    fn reads_and_changes_one_property() {
        let mut s = Scene::from_json(&json!({"layers": [
            {"id": "t", "type": "text", "text": "Hi", "x": 10, "keyframes": {"opacity": [[0, 0], [1, 1]]}}
        ]}))
        .unwrap();
        assert_eq!(s.value("t", "x", 0.0), Some(KeyValue::Number(10.0)));
        assert_eq!(s.value("t", "opacity", 0.5), Some(KeyValue::Number(0.5)));
        assert_eq!(s.value("t", "text", 0.0), Some(KeyValue::from("Hi")));
        assert_eq!(s.value("t", "fill", 0.0), Some(KeyValue::from("#ffffff")));
        assert!(s.value("t", "wobble", 0.0).is_none() && s.value("nope", "x", 0.0).is_none());
        s.item_mut("t").unwrap().set("fontSize", &KeyValue::Number(90.0)).unwrap();
        assert_eq!(s.value("t", "fontSize", 0.0), Some(KeyValue::Number(90.0)));

        let mut three = Scene::from_json(&json!({"objects": [{"id": "b", "type": "box", "position": [1, 2, 3]}]})).unwrap();
        assert_eq!(three.value("b", "position.y", 0.0), Some(KeyValue::Number(2.0)));
        assert_eq!(three.value("b", "z", 0.0), Some(KeyValue::Number(3.0)));
        assert_eq!(three.value("camera", "fov", 0.0), Some(KeyValue::Number(40.0)));
        assert_eq!(three.value("camera", "position.z", 0.0), Some(KeyValue::Number(8.0)));
        three.item_mut("scene").unwrap().set("ambient", &KeyValue::Number(0.5)).unwrap();
        assert_eq!(three.value("scene", "ambient", 0.0), Some(KeyValue::Number(0.5)));
        assert!(three.item_mut("scene").unwrap().set("wobble", &KeyValue::Number(1.0)).is_err());
    }

    #[test]
    fn upserts_and_removes() {
        let mut s = Scene::from_json(&json!({"layers": [{"id": "g", "type": "group", "layers": []}]})).unwrap();
        s.upsert(&json!({"id": "dot", "type": "ellipse", "width": 20, "height": 20}), Some("g")).unwrap();
        assert_eq!(s.ids(), vec!["g", "dot"]);
        s.upsert(&json!({"id": "dot", "type": "ellipse", "width": 40}), None).unwrap();
        assert_eq!(s.ids(), vec!["g", "dot"], "replaced in place");
        assert!(s.remove("dot"));
        assert!(!s.remove("dot"));
        assert_eq!(group_thousands("1234567.50"), "1,234,567.50");
        let t = TextLayer { value: Some(12345.4), ..serde_json::from_value(json!({"text": "{value} views"})).unwrap() };
        assert_eq!(t.shown(), "12,345 views");
    }
}
