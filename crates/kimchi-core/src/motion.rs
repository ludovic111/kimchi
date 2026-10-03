//! Motion clips: animated scenes kimchi draws itself, frame by frame, from plain JSON.
//!
//! A [`Scene`] is either 2D motion graphics ([`Scene2d`]: shapes, paths, text and images in
//! layers, like an After Effects composition) or a 3D scene ([`Scene3d`]: a camera, lights and
//! objects). Everything has an `id` agents refer to, and every number or colour can be animated
//! with `keyframes` (see [`crate::anim`]): `{"opacity": [[0, 0], [0.5, 1, "easeOut"]]}`.
//!
//! Times inside a scene are scene seconds: 0 is the scene's first frame. The clip shows scene time
//! `in_point + (t − start) × speed`, so trimming the start of a motion clip cuts into it like
//! footage. Positions are project pixels from the canvas centre (2D, y down) or world units (3D,
//! y up, the camera looking at `target`).
//!
//! [`Layer::at`] / [`Object3d::at`] give a copy with the keyframes applied, which is what the
//! renderer draws. [`Scene::validate`] checks ids, property names, colours and easings so an agent
//! gets one clear error instead of a silently wrong picture.

use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::anim::{KeyValue, Keyframes, Rgba, normalize, value_at};

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

    /// Reads a scene from JSON. `"type": "2d"` or `"3d"`; without it, a scene with `objects` or
    /// a `camera` is 3D and anything else 2D.
    pub fn from_json(v: &Value) -> Result<Scene, String> {
        let obj = v.as_object().ok_or("A scene is a JSON object.")?;
        let three = match obj.get("type").and_then(Value::as_str) {
            Some("2d" | "2D" | "flat") => false,
            Some("3d" | "3D" | "space") => true,
            Some(other) => return Err(format!("Scene type is \"2d\" or \"3d\", not \"{other}\".")),
            None => obj.contains_key("objects") || obj.contains_key("camera"),
        };
        let mut body = v.clone();
        if let Some(o) = body.as_object_mut() {
            o.remove("type");
        }
        // Unknown fields and types first (a typo in a layer), with hints, before serde's errors.
        if three {
            check_keys(&body, SCENE3D_KEYS, "the 3D scene")?;
            for o in v.get("objects").and_then(Value::as_array).into_iter().flatten() {
                check_object_keys(o)?;
            }
        } else {
            check_keys(&body, SCENE2D_KEYS, "the 2D scene")?;
            for l in v.get("layers").and_then(Value::as_array).into_iter().flatten() {
                check_layer_keys(l)?;
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

    /// Sorts keyframes.
    pub fn normalize(&mut self) {
        match self {
            Scene::Flat(s) => {
                normalize(&mut s.keyframes);
                walk_layers_mut(&mut s.layers, &mut |l| normalize(&mut l.keyframes));
            }
            Scene::Space(s) => {
                normalize(&mut s.keyframes);
                normalize(&mut s.camera.keyframes);
                for l in &mut s.lights {
                    normalize(&mut l.keyframes);
                }
                walk_objects_mut(&mut s.objects, &mut |o| normalize(&mut o.keyframes));
            }
        }
    }

    /// Unique ids, known keyframed properties of the right kind, colours, masks that exist.
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        match self {
            Scene::Flat(s) => {
                check_color_opt(&s.background, "background")?;
                check_scene_keys(&s.keyframes, &["background"])?;
                validate_layers(&s.layers, &mut ids)
            }
            Scene::Space(s) => {
                check_color_opt(&s.background, "background")?;
                check_color(&s.ambient_color, "ambientColor")?;
                check_scene_keys(&s.keyframes, &["background", "ambient", "ambientColor"])?;
                let mut cam = s.camera.clone();
                for (name, keys) in &s.camera.keyframes {
                    for k in keys {
                        cam.set(name, &k.value).map_err(|e| format!("camera keyframes: {e}"))?;
                    }
                }
                for l in &s.lights {
                    if !ids.insert(l.id.clone()) {
                        return Err(format!("Two things have the id \"{}\"; ids must be unique.", l.id));
                    }
                    check_color(&l.color, &format!("light {} color", l.id))?;
                    let mut probe = l.clone();
                    for (name, keys) in &l.keyframes {
                        for k in keys {
                            probe.set(name, &k.value).map_err(|e| format!("light \"{}\": {e}", l.id))?;
                        }
                    }
                }
                validate_objects(&s.objects, &mut ids)
            }
        }
    }

    /// Ids of the layers (2D) or objects and lights (3D), depth first.
    pub fn ids(&self) -> Vec<String> {
        let mut out = vec![];
        match self {
            Scene::Flat(s) => walk_layers(&s.layers, &mut |l| out.push(l.id.clone())),
            Scene::Space(s) => {
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
                for l in &s.lights {
                    see(&l.keyframes);
                }
                walk_objects(&s.objects, &mut |o| see(&o.keyframes));
            }
        }
        t
    }

    /// Asset ids, names or paths the scene draws (image layers, textures, models).
    pub fn media_refs(&self) -> Vec<String> {
        let mut out = vec![];
        match self {
            Scene::Flat(s) => walk_layers(&s.layers, &mut |l| {
                if let LayerKind::Image { asset, .. } = &l.kind {
                    out.push(asset.clone());
                }
            }),
            Scene::Space(s) => walk_objects(&s.objects, &mut |o| {
                match &o.shape {
                    Shape3d::Image { asset, .. } => out.push(asset.clone()),
                    Shape3d::Model { src } => out.push(src.clone()),
                    _ => {}
                }
                if let Some(t) = &o.material.texture {
                    out.push(t.clone());
                }
            }),
        }
        out
    }

    /// Inserts `item` (a layer for 2D, an object or light for 3D) or replaces the one with its
    /// id. New 2D layers go on top; `parent` puts it inside a group (2D) or object (3D).
    pub fn upsert(&mut self, item: &Value, parent: Option<&str>) -> Result<String, String> {
        match self {
            Scene::Flat(s) => {
                check_layer_keys(item)?;
                let mut layer: Layer = serde_json::from_value(item.clone()).map_err(|e| format!("layer: {e}"))?;
                normalize(&mut layer.keyframes);
                let id = layer.id.clone();
                if let Some(slot) = find_layer_mut(&mut s.layers, &id) {
                    *slot = layer;
                } else {
                    let list = match parent {
                        Some(p) => match find_layer_mut(&mut s.layers, p) {
                            Some(Layer { kind: LayerKind::Group { layers }, .. }) => layers,
                            Some(_) => return Err(format!("\"{p}\" isn't a group, so it can't hold layers.")),
                            None => return Err(format!("No layer \"{p}\" in this scene.")),
                        },
                        None => &mut s.layers,
                    };
                    list.push(layer);
                }
                Ok(id)
            }
            Scene::Space(s) => {
                let is_light = item.get("type").and_then(Value::as_str).is_some_and(|t| LIGHT_TYPES.contains(&t));
                if is_light {
                    let light: Light = serde_json::from_value(item.clone()).map_err(|e| format!("light: {e}"))?;
                    let id = light.id.clone();
                    match s.lights.iter_mut().find(|l| l.id == id) {
                        Some(slot) => *slot = light,
                        None => s.lights.push(light),
                    }
                    return Ok(id);
                }
                check_object_keys(item)?;
                let mut obj: Object3d = serde_json::from_value(item.clone()).map_err(|e| format!("object: {e}"))?;
                normalize(&mut obj.keyframes);
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

    /// Removes the layer, object or light with `id`; false if there is none.
    pub fn remove(&mut self, id: &str) -> bool {
        match self {
            Scene::Flat(s) => remove_layer(&mut s.layers, id),
            Scene::Space(s) => {
                let before = s.lights.len();
                s.lights.retain(|l| l.id != id);
                s.lights.len() != before || remove_object(&mut s.objects, id)
            }
        }
    }

    /// The keyframes of one thing in the scene: a layer/object/light id, `"camera"`, or
    /// `"scene"` for the scene's own (background, ambient).
    pub fn keyframes_mut(&mut self, id: &str) -> Option<&mut Keyframes> {
        match self {
            Scene::Flat(s) => {
                if id == "scene" {
                    return Some(&mut s.keyframes);
                }
                find_layer_mut(&mut s.layers, id).map(|l| &mut l.keyframes)
            }
            Scene::Space(s) => match id {
                "scene" => Some(&mut s.keyframes),
                "camera" => Some(&mut s.camera.keyframes),
                _ => {
                    if let Some(l) = s.lights.iter_mut().find(|l| l.id == id) {
                        return Some(&mut l.keyframes);
                    }
                    find_object_mut(&mut s.objects, id).map(|o| &mut o.keyframes)
                }
            },
        }
    }

    /// One layer, object or light as JSON.
    pub fn item_json(&self, id: &str) -> Option<Value> {
        match self {
            Scene::Flat(s) => find_layer(&s.layers, id).map(|l| serde_json::to_value(l).unwrap_or(Value::Null)),
            Scene::Space(s) => {
                if id == "camera" {
                    return serde_json::to_value(&s.camera).ok();
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
            None => v.get("objects").is_some() || v.get("camera").is_some(),
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
// 2D

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scene2d {
    /// Fills the canvas behind the layers; none = transparent (what's below shows through).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Bottom to top: later layers draw over earlier ones.
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

const SCENE2D_KEYS: &[&str] = &["background", "layers", "keyframes", "animate"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: String,
    #[serde(flatten)]
    pub kind: LayerKind,
    /// Position of the anchor from the canvas centre (or the group's origin), y down.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub x: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub y: f64,
    /// The point (in the layer's own pixels, from its centre) it rotates and scales around.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub anchor_x: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub anchor_y: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_x: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_y: f64,
    /// Degrees, clockwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// Degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skew_x: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    /// Draw only part of the outline (0–1 along its length): animate `trimEnd` 0 → 1 to draw it on.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trim_start: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub trim_end: f64,
    /// Slides the trimmed part along the outline (0–1, wraps).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trim_offset: f64,
    /// Gaussian blur radius in pixels.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub blur: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glow: Option<Glow>,
    #[serde(default, skip_serializing_if = "Blend::is_normal")]
    pub blend: Blend,
    /// Id of another layer whose shape this one shows through (that layer isn't drawn itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mask_invert: bool,
    /// Scene seconds when the layer appears and disappears.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub start: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

const LAYER_KEYS: &[&str] = &[
    "id", "type", "x", "y", "anchorX", "anchorY", "scale", "scaleX", "scaleY", "rotation", "skewX", "opacity", "fill",
    "stroke", "trimStart", "trimEnd", "trimOffset", "blur", "shadow", "glow", "blend", "mask", "maskInvert", "start",
    "end", "hidden", "keyframes", "animate",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayerKind {
    #[serde(rename_all = "camelCase")]
    Rect {
        #[serde(default = "hundred")]
        width: f64,
        #[serde(default = "hundred")]
        height: f64,
        /// Corner radius.
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: f64,
    },
    #[serde(rename_all = "camelCase")]
    Ellipse {
        #[serde(default = "hundred")]
        width: f64,
        #[serde(default = "hundred")]
        height: f64,
    },
    #[serde(rename_all = "camelCase")]
    Polygon {
        #[serde(default = "three")]
        sides: f64,
        #[serde(default = "fifty")]
        radius: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        roundness: f64,
    },
    #[serde(rename_all = "camelCase")]
    Star {
        #[serde(default = "five")]
        points: f64,
        #[serde(default = "fifty")]
        radius: f64,
        #[serde(default = "twenty")]
        inner_radius: f64,
    },
    /// SVG path data (`"M0 0 L100 0 C…"`), or a list of points joined by straight lines.
    #[serde(rename_all = "camelCase")]
    Path {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        d: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        points: Vec<[f64; 2]>,
        #[serde(default, skip_serializing_if = "is_false")]
        closed: bool,
    },
    Text(TextLayer),
    /// A media item (image, or a video's frame at scene time) by id, name or file path.
    #[serde(rename_all = "camelCase")]
    Image {
        asset: String,
        /// Size in pixels; one alone keeps the aspect ratio; none = the image's own size.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<f64>,
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: f64,
    },
    Group {
        #[serde(default)]
        layers: Vec<Layer>,
    },
}

const KIND_KEYS: &[(&str, &[&str])] = &[
    ("rect", &["width", "height", "radius"]),
    ("ellipse", &["width", "height"]),
    ("polygon", &["sides", "radius", "roundness"]),
    ("star", &["points", "radius", "innerRadius"]),
    ("path", &["d", "points", "closed"]),
    ("text", &["text", "fontFamily", "fontSize", "fontWeight", "italic", "align", "lineHeight", "letterSpacing", "value", "decimals", "reveal"]),
    ("image", &["asset", "width", "height", "radius"]),
    ("group", &["layers"]),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextLayer {
    /// The words; `\n` starts a line; `{value}` shows `value` (for counters).
    pub text: String,
    #[serde(default = "manrope")]
    pub font_family: String,
    #[serde(default = "seventy_two")]
    pub font_size: f64,
    #[serde(default = "bold")]
    pub font_weight: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default)]
    pub align: crate::TextAlign,
    #[serde(default = "line_height")]
    pub line_height: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub letter_spacing: f64,
    /// A number shown where the text says `{value}`; animate it for counters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub decimals: f64,
    /// Letters, words or lines appearing one after another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal: Option<Reveal>,
}

impl TextLayer {
    /// The text with `{value}` filled in.
    pub fn shown(&self) -> String {
        match self.value {
            Some(v) if self.text.contains("{value}") => {
                let d = self.decimals.clamp(0.0, 6.0) as usize;
                let n = format!("{:.*}", d, v);
                self.text.replace("{value}", &group_thousands(&n))
            }
            _ => self.text.clone(),
        }
    }
}

fn group_thousands(n: &str) -> String {
    let (sign, rest) = n.strip_prefix('-').map_or(("", n), |r| ("-", r));
    let (int, frac) = rest.split_once('.').map_or((rest, None), |(a, b)| (a, Some(b)));
    if int.len() <= 4 {
        return n.to_string();
    }
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reveal {
    /// `"char"`, `"word"` or `"line"`.
    #[serde(default = "by_char")]
    pub by: String,
    /// `"fade"`, `"rise"`, `"drop"`, `"slide"`, `"pop"`, `"type"` or `"blur"`.
    #[serde(default = "rise")]
    pub style: String,
    /// 0 = nothing shown, 1 = everything; animate it (property `reveal`).
    #[serde(default = "one")]
    pub progress: f64,
    /// How many units move at once (smoothness of the wave).
    #[serde(default = "three")]
    pub overlap: f64,
    /// Travel for rise/drop/slide, in pixels (default: half the font size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
}

/// A colour (`"#ff5a36"`), or a gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fill {
    Color(String),
    Gradient(Gradient),
}

impl Fill {
    pub fn color(&self) -> Option<&str> {
        match self {
            Fill::Color(c) => Some(c),
            Fill::Gradient(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gradient {
    /// `"linear"` (from → to) or `"radial"` (around `center`, out to `radius`).
    #[serde(rename = "type", default = "linear")]
    pub kind: String,
    /// `[[0, "#ff5a36"], [1, "#ffb199"]]`.
    pub stops: Vec<(f64, String)>,
    /// Linear: start and end points in layer pixels (default: across the shape's width).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    #[serde(default = "white")]
    pub color: String,
    #[serde(default = "four")]
    pub width: f64,
    /// `"butt"`, `"round"` (default) or `"square"`.
    #[serde(default = "round")]
    pub cap: String,
    /// `"miter"`, `"round"` (default) or `"bevel"`.
    #[serde(default = "round")]
    pub join: String,
    /// Dash and gap lengths, e.g. `[20, 10]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dash_offset: f64,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke { color: white(), width: 4.0, cap: round(), join: round(), dash: vec![], dash_offset: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shadow {
    #[serde(default = "shadow_color")]
    pub color: String,
    #[serde(default = "twelve")]
    pub blur: f64,
    #[serde(default)]
    pub x: f64,
    #[serde(default = "six")]
    pub y: f64,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { color: shadow_color(), blur: 12.0, x: 0.0, y: 6.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Glow {
    #[serde(default = "white")]
    pub color: String,
    #[serde(default = "twenty")]
    pub radius: f64,
    /// 0–2.
    #[serde(default = "one")]
    pub strength: f64,
}

impl Default for Glow {
    fn default() -> Self {
        Glow { color: white(), radius: 20.0, strength: 1.0 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    #[serde(alias = "plus")]
    Add,
    Darken,
    Lighten,
    Difference,
    ColorDodge,
    ColorBurn,
    SoftLight,
    HardLight,
}

impl Blend {
    fn is_normal(&self) -> bool {
        *self == Blend::Normal
    }
}

impl Layer {
    /// Is the layer on screen at scene time `t`?
    pub fn visible_at(&self, t: f64) -> bool {
        !self.hidden && t + 1e-9 >= self.start && self.end.is_none_or(|e| t < e - 1e-9)
    }

    /// The layer with its keyframes applied at scene time `t` (children included).
    pub fn at(&self, t: f64) -> Layer {
        let mut l = self.clone();
        for (name, keys) in &self.keyframes {
            if let Some(v) = value_at(keys, t) {
                // Validated when the scene was set; a bad key in an old file is just skipped.
                let _ = l.set(name, &v);
            }
        }
        if let LayerKind::Group { layers } = &mut l.kind {
            for child in layers.iter_mut() {
                *child = child.at(t);
            }
        }
        l.keyframes.clear();
        l
    }

    /// Sets one property by its keyframe name.
    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        let s = || v.as_str().map(str::to_string).ok_or_else(|| format!("{name} takes a string"));
        let c = || -> Result<String, String> {
            let s = v.as_str().ok_or_else(|| format!("{name} takes a colour like \"#ff5a36\""))?;
            check_color(s, name)?;
            Ok(s.to_string())
        };
        match name {
            "x" => self.x = n()?,
            "y" => self.y = n()?,
            "position" => {
                let p = v.as_vec(2).ok_or("position takes [x, y]")?;
                (self.x, self.y) = (p[0], p[1]);
            }
            "anchorX" => self.anchor_x = n()?,
            "anchorY" => self.anchor_y = n()?,
            "scale" => match v {
                KeyValue::Vector(p) if p.len() == 2 => {
                    self.scale = 1.0;
                    (self.scale_x, self.scale_y) = (p[0], p[1]);
                }
                _ => self.scale = n()?,
            },
            "scaleX" => self.scale_x = n()?,
            "scaleY" => self.scale_y = n()?,
            "rotation" => self.rotation = n()?,
            "skewX" => self.skew_x = n()?,
            "opacity" => self.opacity = n()?,
            "trimStart" => self.trim_start = n()?,
            "trimEnd" => self.trim_end = n()?,
            "trimOffset" => self.trim_offset = n()?,
            "blur" => self.blur = n()?,
            "fill" | "color" => self.fill = Some(Fill::Color(c()?)),
            "strokeColor" => self.stroke.get_or_insert_with(Stroke::default).color = c()?,
            "strokeWidth" => self.stroke.get_or_insert_with(Stroke::default).width = n()?,
            "dashOffset" => self.stroke.get_or_insert_with(Stroke::default).dash_offset = n()?,
            "shadowColor" => self.shadow.get_or_insert_with(Shadow::default).color = c()?,
            "shadowBlur" => self.shadow.get_or_insert_with(Shadow::default).blur = n()?,
            "shadowX" => self.shadow.get_or_insert_with(Shadow::default).x = n()?,
            "shadowY" => self.shadow.get_or_insert_with(Shadow::default).y = n()?,
            "glowColor" => self.glow.get_or_insert_with(Glow::default).color = c()?,
            "glowRadius" => self.glow.get_or_insert_with(Glow::default).radius = n()?,
            "glowStrength" => self.glow.get_or_insert_with(Glow::default).strength = n()?,
            _ => return self.set_kind(name, v, n, s),
        }
        Ok(())
    }

    fn set_kind(
        &mut self,
        name: &str,
        v: &KeyValue,
        n: impl Fn() -> Result<f64, String>,
        s: impl Fn() -> Result<String, String>,
    ) -> Result<(), String> {
        let kind = self.kind.name();
        match (&mut self.kind, name) {
            (LayerKind::Rect { width, .. } | LayerKind::Ellipse { width, .. }, "width") => *width = n()?,
            (LayerKind::Rect { height, .. } | LayerKind::Ellipse { height, .. }, "height") => *height = n()?,
            (LayerKind::Rect { width, height, .. } | LayerKind::Ellipse { width, height }, "size") => {
                let p = v.as_vec(2).ok_or("size takes [width, height]")?;
                (*width, *height) = (p[0], p[1]);
            }
            (LayerKind::Rect { radius, .. } | LayerKind::Polygon { radius, .. } | LayerKind::Star { radius, .. }, "radius") => {
                *radius = n()?
            }
            (LayerKind::Image { radius, .. }, "radius") => *radius = n()?,
            (LayerKind::Image { width, .. }, "width") => *width = Some(n()?),
            (LayerKind::Image { height, .. }, "height") => *height = Some(n()?),
            (LayerKind::Polygon { sides, .. }, "sides") => *sides = n()?,
            (LayerKind::Polygon { roundness, .. }, "roundness") => *roundness = n()?,
            (LayerKind::Star { points, .. }, "points") => *points = n()?,
            (LayerKind::Star { inner_radius, .. }, "innerRadius") => *inner_radius = n()?,
            (LayerKind::Path { d, .. }, "d") => *d = s()?,
            (LayerKind::Text(t), "text") => t.text = s()?,
            (LayerKind::Text(t), "fontSize") => t.font_size = n()?,
            (LayerKind::Text(t), "fontWeight") => t.font_weight = n()?,
            (LayerKind::Text(t), "letterSpacing") => t.letter_spacing = n()?,
            (LayerKind::Text(t), "lineHeight") => t.line_height = n()?,
            (LayerKind::Text(t), "value") => t.value = Some(n()?),
            (LayerKind::Text(t), "reveal") => {
                t.reveal.get_or_insert_with(|| Reveal {
                    by: by_char(),
                    style: rise(),
                    progress: 1.0,
                    overlap: 3.0,
                    distance: None,
                })
                .progress = n()?
            }
            _ => {
                return Err(format!(
                    "a {kind} layer has no animatable property `{name}`. Common: {}. {kind}: {}.",
                    COMMON_PROPS.join(", "),
                    self.kind.props().join(", ")
                ));
            }
        }
        Ok(())
    }
}

const COMMON_PROPS: &[&str] = &[
    "x", "y", "position", "anchorX", "anchorY", "scale", "scaleX", "scaleY", "rotation", "skewX", "opacity", "fill",
    "strokeColor", "strokeWidth", "dashOffset", "trimStart", "trimEnd", "trimOffset", "blur", "shadowColor",
    "shadowBlur", "shadowX", "shadowY", "glowColor", "glowRadius", "glowStrength",
];

impl LayerKind {
    pub fn name(&self) -> &'static str {
        match self {
            LayerKind::Rect { .. } => "rect",
            LayerKind::Ellipse { .. } => "ellipse",
            LayerKind::Polygon { .. } => "polygon",
            LayerKind::Star { .. } => "star",
            LayerKind::Path { .. } => "path",
            LayerKind::Text(_) => "text",
            LayerKind::Image { .. } => "image",
            LayerKind::Group { .. } => "group",
        }
    }

    /// Animatable properties of this kind (beyond the common ones).
    pub fn props(&self) -> &'static [&'static str] {
        match self {
            LayerKind::Rect { .. } => &["width", "height", "size", "radius"],
            LayerKind::Ellipse { .. } => &["width", "height", "size"],
            LayerKind::Polygon { .. } => &["sides", "radius", "roundness"],
            LayerKind::Star { .. } => &["points", "radius", "innerRadius"],
            LayerKind::Path { .. } => &["d"],
            LayerKind::Text(_) => &["text", "fontSize", "fontWeight", "letterSpacing", "lineHeight", "value", "reveal"],
            LayerKind::Image { .. } => &["width", "height", "radius"],
            LayerKind::Group { .. } => &[],
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3D

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scene3d {
    /// Behind everything; none = transparent (what's below shows through).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default)]
    pub camera: Camera,
    /// Light that reaches everywhere (0–1).
    #[serde(default = "ambient")]
    pub ambient: f64,
    #[serde(default = "white")]
    pub ambient_color: String,
    /// Without lights, a key light and a soft fill are used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    #[serde(default)]
    pub objects: Vec<Object3d>,
    /// Soft shadows of objects on the others (directional lights only).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub shadows: bool,
    /// With a background, distant things fade into it (a soft horizon for floors).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub fog: bool,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

impl Default for Scene3d {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

const SCENE3D_KEYS: &[&str] = &["background", "camera", "ambient", "ambientColor", "lights", "objects", "shadows", "fog", "keyframes", "animate"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Camera {
    #[serde(default = "camera_position")]
    pub position: Vec3,
    #[serde(default)]
    pub target: Vec3,
    /// Vertical field of view in degrees.
    #[serde(default = "forty")]
    pub fov: f64,
    /// Degrees around the viewing direction.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub roll: f64,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { position: camera_position(), target: Vec3::default(), fov: 40.0, roll: 0.0, keyframes: Keyframes::new() }
    }
}

impl Camera {
    pub fn at(&self, t: f64) -> Camera {
        let mut c = self.clone();
        for (name, keys) in &self.keyframes {
            if let Some(v) = value_at(keys, t) {
                let _ = c.set(name, &v);
            }
        }
        c.keyframes.clear();
        c
    }

    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        match name {
            "fov" => self.fov = n()?,
            "roll" => self.roll = n()?,
            _ => {
                if self.position.set_named(name, "position", v)? || self.target.set_named(name, "target", v)? {
                    return Ok(());
                }
                return Err(format!("the camera has no property `{name}`; use position, position.x/y/z, target, target.x/y/z, fov or roll"));
            }
        }
        Ok(())
    }
}

pub const LIGHT_TYPES: &[&str] = &["directional", "point", "spot"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Light {
    pub id: String,
    /// `"directional"` (like the sun: only `direction` matters) or `"point"` (a bulb at `position`).
    #[serde(rename = "type", default = "directional")]
    pub kind: String,
    #[serde(default = "white")]
    pub color: String,
    #[serde(default = "one")]
    pub intensity: f64,
    #[serde(default = "light_position")]
    pub position: Vec3,
    /// Where a directional light shines towards.
    #[serde(default = "light_direction")]
    pub direction: Vec3,
    /// Point lights: distance where the light has faded out (0 = never).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub range: f64,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

impl Light {
    pub fn at(&self, t: f64) -> Light {
        let mut l = self.clone();
        for (name, keys) in &self.keyframes {
            if let Some(v) = value_at(keys, t) {
                let _ = l.set(name, &v);
            }
        }
        l.keyframes.clear();
        l
    }

    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        match name {
            "intensity" => self.intensity = v.as_f64().ok_or("intensity takes a number")?,
            "range" => self.range = v.as_f64().ok_or("range takes a number")?,
            "color" => {
                let c = v.as_str().ok_or("color takes a colour")?;
                check_color(c, "color")?;
                self.color = c.to_string();
            }
            _ => {
                if self.position.set_named(name, "position", v)? || self.direction.set_named(name, "direction", v)? {
                    return Ok(());
                }
                return Err(format!("a light has no property `{name}`; use intensity, color, range, position(.x/y/z) or direction(.x/y/z)"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Object3d {
    pub id: String,
    #[serde(flatten)]
    pub shape: Shape3d,
    #[serde(default, skip_serializing_if = "Vec3::is_zero")]
    pub position: Vec3,
    /// Degrees around x, then y, then z.
    #[serde(default, skip_serializing_if = "Vec3::is_zero")]
    pub rotation: Vec3,
    #[serde(default = "Vec3::one", skip_serializing_if = "Vec3::is_one")]
    pub scale: Vec3,
    #[serde(default)]
    pub material: Material,
    /// Objects that move with this one (their position is relative to it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Object3d>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub start: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

const OBJECT_KEYS: &[&str] = &[
    "id", "type", "position", "rotation", "scale", "material", "children", "start", "end", "hidden", "keyframes", "animate",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Shape3d {
    #[serde(rename_all = "camelCase")]
    Box {
        #[serde(default = "Vec3::one")]
        size: Vec3,
        /// Rounded edges (0–0.5 of the smallest side).
        #[serde(default, skip_serializing_if = "is_zero")]
        bevel: f64,
    },
    #[serde(rename_all = "camelCase")]
    Sphere {
        #[serde(default = "half")]
        radius: f64,
    },
    #[serde(rename_all = "camelCase")]
    Cylinder {
        #[serde(default = "half")]
        radius: f64,
        #[serde(default = "one")]
        height: f64,
    },
    #[serde(rename_all = "camelCase")]
    Cone {
        #[serde(default = "half")]
        radius: f64,
        #[serde(default = "one")]
        height: f64,
    },
    #[serde(rename_all = "camelCase")]
    Torus {
        #[serde(default = "half")]
        radius: f64,
        #[serde(default = "tube")]
        tube: f64,
    },
    #[serde(rename_all = "camelCase")]
    Plane {
        #[serde(default = "one")]
        width: f64,
        #[serde(default = "one")]
        height: f64,
    },
    /// Extruded 3D letters, centred on the object's position.
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        #[serde(default = "manrope")]
        font_family: String,
        #[serde(default = "bold")]
        font_weight: f64,
        /// Letter height (em size) in world units.
        #[serde(default = "one")]
        size: f64,
        /// Thickness in world units.
        #[serde(default = "tube")]
        depth: f64,
        #[serde(default)]
        align: crate::TextAlign,
        #[serde(default, skip_serializing_if = "is_zero")]
        letter_spacing: f64,
    },
    /// A glTF/GLB model file (absolute path, or a media item), centred and scaled to 2 units.
    Model { src: String },
    /// A flat picture (media item by id, name or path) facing the camera's default direction.
    #[serde(rename_all = "camelCase")]
    Image {
        asset: String,
        /// Width in world units; the height follows the picture.
        #[serde(default = "two")]
        width: f64,
    },
    /// Holds children only.
    Group {},
}

const SHAPE_KEYS: &[(&str, &[&str])] = &[
    ("box", &["size", "bevel"]),
    ("sphere", &["radius"]),
    ("cylinder", &["radius", "height"]),
    ("cone", &["radius", "height"]),
    ("torus", &["radius", "tube"]),
    ("plane", &["width", "height"]),
    ("text", &["text", "fontFamily", "fontWeight", "size", "depth", "align", "letterSpacing"]),
    ("model", &["src"]),
    ("image", &["asset", "width"]),
    ("group", &[]),
];

impl Shape3d {
    pub fn name(&self) -> &'static str {
        match self {
            Shape3d::Box { .. } => "box",
            Shape3d::Sphere { .. } => "sphere",
            Shape3d::Cylinder { .. } => "cylinder",
            Shape3d::Cone { .. } => "cone",
            Shape3d::Torus { .. } => "torus",
            Shape3d::Plane { .. } => "plane",
            Shape3d::Text { .. } => "text",
            Shape3d::Model { .. } => "model",
            Shape3d::Image { .. } => "image",
            Shape3d::Group {} => "group",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    #[serde(default = "grey")]
    pub color: String,
    /// 0 = plastic/paint, 1 = metal.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub metallic: f64,
    /// 0 = mirror-smooth, 1 = matte.
    #[serde(default = "half")]
    pub roughness: f64,
    /// Glows with this colour regardless of lights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive: Option<String>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub emissive_intensity: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    /// A picture wrapped on the surface (media item by id, name or path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// Faceted look instead of smooth.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flat: bool,
    /// No lighting: the colour as is.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

const MATERIAL_KEYS: &[&str] = &["color", "metallic", "roughness", "emissive", "emissiveIntensity", "opacity", "texture", "flat", "unlit"];

impl Object3d {
    pub fn visible_at(&self, t: f64) -> bool {
        !self.hidden && t + 1e-9 >= self.start && self.end.is_none_or(|e| t < e - 1e-9)
    }

    pub fn at(&self, t: f64) -> Object3d {
        let mut o = self.clone();
        for (name, keys) in &self.keyframes {
            if let Some(v) = value_at(keys, t) {
                let _ = o.set(name, &v);
            }
        }
        o.keyframes.clear();
        for c in o.children.iter_mut() {
            *c = c.at(t);
        }
        o
    }

    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        let c = || -> Result<String, String> {
            let s = v.as_str().ok_or_else(|| format!("{name} takes a colour like \"#ff5a36\""))?;
            check_color(s, name)?;
            Ok(s.to_string())
        };
        let alias = match name {
            "x" => "position.x",
            "y" => "position.y",
            "z" => "position.z",
            other => other,
        };
        if self.position.set_named(alias, "position", v)?
            || self.rotation.set_named(alias, "rotation", v)?
            || self.scale.set_named(alias, "scale", v)?
        {
            return Ok(());
        }
        if let Shape3d::Box { size, .. } = &mut self.shape
            && size.set_named(alias, "size", v)?
        {
            return Ok(());
        }
        match (&mut self.shape, alias) {
            (_, "color") => self.material.color = c()?,
            (_, "opacity") => self.material.opacity = n()?,
            (_, "metallic") => self.material.metallic = n()?,
            (_, "roughness") => self.material.roughness = n()?,
            (_, "emissive") => self.material.emissive = Some(c()?),
            (_, "emissiveIntensity") => self.material.emissive_intensity = n()?,
            (Shape3d::Box { bevel, .. }, "bevel") => *bevel = n()?,
            (
                Shape3d::Sphere { radius } | Shape3d::Cylinder { radius, .. } | Shape3d::Cone { radius, .. } | Shape3d::Torus { radius, .. },
                "radius",
            ) => *radius = n()?,
            (Shape3d::Cylinder { height, .. } | Shape3d::Cone { height, .. } | Shape3d::Plane { height, .. }, "height") => *height = n()?,
            (Shape3d::Torus { tube, .. }, "tube") => *tube = n()?,
            (Shape3d::Plane { width, .. } | Shape3d::Image { width, .. }, "width") => *width = n()?,
            (Shape3d::Text { text, .. }, "text") => *text = v.as_str().ok_or("text takes a string")?.to_string(),
            (Shape3d::Text { size, .. }, "size") => *size = n()?,
            (Shape3d::Text { depth, .. }, "depth") => *depth = n()?,
            (Shape3d::Text { letter_spacing, .. }, "letterSpacing") => *letter_spacing = n()?,
            (shape, _) => {
                let own = SHAPE_KEYS.iter().find(|(k, _)| *k == shape.name()).map(|(_, p)| p.join(", ")).unwrap_or_default();
                return Err(format!(
                    "a {} has no animatable property `{name}`. Every object: position, position.x/y/z (or x/y/z), rotation, rotation.x/y/z, scale, scale.x/y/z, color, opacity, metallic, roughness, emissive, emissiveIntensity. {}: {own}.",
                    shape.name(),
                    shape.name()
                ));
            }
        }
        Ok(())
    }
}

/// Three numbers; in JSON `[x, y, z]`, or one number for all three (`"scale": 2`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3(pub [f64; 3]);

impl Vec3 {
    pub fn one() -> Vec3 {
        Vec3([1.0; 3])
    }

    fn is_zero(&self) -> bool {
        self.0 == [0.0; 3]
    }

    fn is_one(&self) -> bool {
        self.0 == [1.0; 3]
    }

    /// Sets it when `name` is `base` or `base.x|y|z`; false when `name` is something else.
    fn set_named(&mut self, name: &str, base: &str, v: &KeyValue) -> Result<bool, String> {
        if name == base {
            let p = v.as_vec(3).ok_or_else(|| format!("{base} takes [x, y, z] or one number"))?;
            self.0 = [p[0], p[1], p[2]];
            return Ok(true);
        }
        let Some(axis) = name.strip_prefix(base).and_then(|r| r.strip_prefix('.')) else { return Ok(false) };
        let i = match axis {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => return Err(format!("{name}: the axes are x, y and z")),
        };
        self.0[i] = v.as_f64().ok_or_else(|| format!("{name} takes a number"))?;
        Ok(true)
    }
}

impl Serialize for Vec3 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Vec3 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = KeyValue::deserialize(d)?;
        v.as_vec(3).map(|p| Vec3([p[0], p[1], p[2]])).ok_or_else(|| serde::de::Error::custom("expected [x, y, z] or a number"))
    }
}

// ---------------------------------------------------------------------------------------------
// One thing in a scene, by id: read and change its properties

/// A layer, object, light, the camera or the scene itself (`"scene"`), to read and change one
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
                find_layer_mut(&mut s.layers, id).map(ItemMut::Layer)
            }
            Scene::Space(s) => match id {
                "scene" => Some(ItemMut::Space(s)),
                "camera" => Some(ItemMut::Camera(&mut s.camera)),
                _ => {
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
            ItemMut::Space(s) => match name {
                "background" => {
                    let c = v.as_str().ok_or("background takes a colour")?;
                    check_color(c, "background")?;
                    s.background = Some(c.to_string());
                    Ok(())
                }
                "ambient" => {
                    s.ambient = v.as_f64().ok_or("ambient takes a number")?;
                    Ok(())
                }
                "ambientColor" => {
                    let c = v.as_str().ok_or("ambientColor takes a colour")?;
                    check_color(c, "ambientColor")?;
                    s.ambient_color = c.to_string();
                    Ok(())
                }
                _ => Err(format!("A 3D scene's own properties are background, ambient and ambientColor, not `{name}`.")),
            },
        }
    }

    /// The still value of a property (without keyframes).
    pub fn get(&self, name: &str) -> Option<KeyValue> {
        let n = |v: f64| Some(KeyValue::Number(v));
        let c = |v: &str| Some(KeyValue::Text(v.to_string()));
        let v3 = |name: &str, v: &Vec3, base: &str| -> Option<Option<KeyValue>> {
            if name == base {
                return Some(Some(KeyValue::Vector(v.0.to_vec())));
            }
            let axis = name.strip_prefix(base)?.strip_prefix('.')?;
            let i = ["x", "y", "z"].iter().position(|a| *a == axis)?;
            Some(Some(KeyValue::Number(v.0[i])))
        };
        match self {
            ItemMut::Layer(l) => match name {
                "x" => n(l.x),
                "y" => n(l.y),
                "position" => Some(KeyValue::Vector(vec![l.x, l.y])),
                "anchorX" => n(l.anchor_x),
                "anchorY" => n(l.anchor_y),
                "scale" => n(l.scale),
                "scaleX" => n(l.scale_x),
                "scaleY" => n(l.scale_y),
                "rotation" => n(l.rotation),
                "skewX" => n(l.skew_x),
                "opacity" => n(l.opacity),
                "trimStart" => n(l.trim_start),
                "trimEnd" => n(l.trim_end),
                "trimOffset" => n(l.trim_offset),
                "blur" => n(l.blur),
                "fill" | "color" => match &l.fill {
                    Some(Fill::Color(v)) => c(v),
                    None if matches!(l.kind, LayerKind::Text(_)) => c("#ffffff"),
                    _ => None,
                },
                "strokeColor" => l.stroke.as_ref().and_then(|s| c(&s.color)),
                "strokeWidth" => n(l.stroke.as_ref().map_or(0.0, |s| s.width)),
                "dashOffset" => l.stroke.as_ref().and_then(|s| n(s.dash_offset)),
                "shadowColor" => l.shadow.as_ref().and_then(|s| c(&s.color)),
                "shadowBlur" => l.shadow.as_ref().and_then(|s| n(s.blur)),
                "shadowX" => l.shadow.as_ref().and_then(|s| n(s.x)),
                "shadowY" => l.shadow.as_ref().and_then(|s| n(s.y)),
                "glowColor" => l.glow.as_ref().and_then(|g| c(&g.color)),
                "glowRadius" => l.glow.as_ref().and_then(|g| n(g.radius)),
                "glowStrength" => l.glow.as_ref().and_then(|g| n(g.strength)),
                _ => match (&l.kind, name) {
                    (LayerKind::Rect { width, .. } | LayerKind::Ellipse { width, .. }, "width") => n(*width),
                    (LayerKind::Rect { height, .. } | LayerKind::Ellipse { height, .. }, "height") => n(*height),
                    (LayerKind::Rect { width, height, .. } | LayerKind::Ellipse { width, height }, "size") => Some(KeyValue::Vector(vec![*width, *height])),
                    (LayerKind::Rect { radius, .. } | LayerKind::Polygon { radius, .. } | LayerKind::Star { radius, .. }, "radius") => n(*radius),
                    (LayerKind::Image { radius, .. }, "radius") => n(*radius),
                    (LayerKind::Image { width, .. }, "width") => width.and_then(n),
                    (LayerKind::Image { height, .. }, "height") => height.and_then(n),
                    (LayerKind::Polygon { sides, .. }, "sides") => n(*sides),
                    (LayerKind::Polygon { roundness, .. }, "roundness") => n(*roundness),
                    (LayerKind::Star { points, .. }, "points") => n(*points),
                    (LayerKind::Star { inner_radius, .. }, "innerRadius") => n(*inner_radius),
                    (LayerKind::Path { d, .. }, "d") => c(d),
                    (LayerKind::Text(t), "text") => c(&t.text),
                    (LayerKind::Text(t), "fontSize") => n(t.font_size),
                    (LayerKind::Text(t), "fontWeight") => n(t.font_weight),
                    (LayerKind::Text(t), "letterSpacing") => n(t.letter_spacing),
                    (LayerKind::Text(t), "lineHeight") => n(t.line_height),
                    (LayerKind::Text(t), "value") => t.value.and_then(n),
                    (LayerKind::Text(t), "reveal") => n(t.reveal.as_ref().map_or(1.0, |r| r.progress)),
                    _ => None,
                },
            },
            ItemMut::Object(o) => {
                let alias = match name {
                    "x" => "position.x",
                    "y" => "position.y",
                    "z" => "position.z",
                    other => other,
                };
                let name = alias;
                for (v, base) in [(&o.position, "position"), (&o.rotation, "rotation"), (&o.scale, "scale")] {
                    if let Some(r) = v3(name, v, base) {
                        return r;
                    }
                }
                let m = &o.material;
                match (&o.shape, name) {
                    (_, "color") => c(&m.color),
                    (_, "opacity") => n(m.opacity),
                    (_, "metallic") => n(m.metallic),
                    (_, "roughness") => n(m.roughness),
                    (_, "emissive") => m.emissive.as_deref().and_then(c),
                    (_, "emissiveIntensity") => n(m.emissive_intensity),
                    (Shape3d::Box { size, .. }, _) if v3(name, size, "size").is_some() => v3(name, size, "size").flatten(),
                    (Shape3d::Box { bevel, .. }, "bevel") => n(*bevel),
                    (Shape3d::Sphere { radius } | Shape3d::Cylinder { radius, .. } | Shape3d::Cone { radius, .. } | Shape3d::Torus { radius, .. }, "radius") => n(*radius),
                    (Shape3d::Cylinder { height, .. } | Shape3d::Cone { height, .. } | Shape3d::Plane { height, .. }, "height") => n(*height),
                    (Shape3d::Torus { tube, .. }, "tube") => n(*tube),
                    (Shape3d::Plane { width, .. } | Shape3d::Image { width, .. }, "width") => n(*width),
                    (Shape3d::Text { text, .. }, "text") => c(text),
                    (Shape3d::Text { size, .. }, "size") => n(*size),
                    (Shape3d::Text { depth, .. }, "depth") => n(*depth),
                    (Shape3d::Text { letter_spacing, .. }, "letterSpacing") => n(*letter_spacing),
                    _ => None,
                }
            }
            ItemMut::Light(l) => match name {
                "intensity" => n(l.intensity),
                "range" => n(l.range),
                "color" => c(&l.color),
                _ => v3(name, &l.position, "position").or_else(|| v3(name, &l.direction, "direction")).flatten(),
            },
            ItemMut::Camera(cam) => match name {
                "fov" => n(cam.fov),
                "roll" => n(cam.roll),
                _ => v3(name, &cam.position, "position").or_else(|| v3(name, &cam.target, "target")).flatten(),
            },
            ItemMut::Flat(s) => match name {
                "background" => s.background.as_deref().and_then(c),
                _ => None,
            },
            ItemMut::Space(s) => match name {
                "background" => s.background.as_deref().and_then(c),
                "ambient" => n(s.ambient),
                "ambientColor" => c(&s.ambient_color),
                _ => None,
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Walking and checking

pub fn walk_layers(layers: &[Layer], f: &mut impl FnMut(&Layer)) {
    for l in layers {
        f(l);
        if let LayerKind::Group { layers } = &l.kind {
            walk_layers(layers, f);
        }
    }
}

fn walk_layers_mut(layers: &mut [Layer], f: &mut impl FnMut(&mut Layer)) {
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

fn walk_objects_mut(objects: &mut [Object3d], f: &mut impl FnMut(&mut Object3d)) {
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

fn find_object_mut<'a>(objects: &'a mut [Object3d], id: &str) -> Option<&'a mut Object3d> {
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

fn validate_layers(layers: &[Layer], ids: &mut HashSet<String>) -> Result<(), String> {
    let mut all = HashSet::new();
    walk_layers(layers, &mut |l| {
        all.insert(l.id.clone());
    });
    let mut result = Ok(());
    walk_layers(layers, &mut |l| {
        if result.is_err() {
            return;
        }
        result = validate_layer(l, ids, &all);
    });
    result
}

fn validate_layer(l: &Layer, ids: &mut HashSet<String>, all: &HashSet<String>) -> Result<(), String> {
    let id = &l.id;
    if id.trim().is_empty() {
        return Err("Every layer needs an id.".into());
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
            return Err(format!("layer \"{id}\" is masked by \"{m}\", which isn't a layer in this scene"));
        }
    }
    if let LayerKind::Text(t) = &l.kind
        && let Some(r) = &t.reveal
    {
        if !matches!(r.by.as_str(), "char" | "word" | "line") {
            return Err(format!("layer \"{id}\": reveal.by is char, word or line"));
        }
        if !REVEAL_STYLES.contains(&r.style.as_str()) {
            return Err(format!("layer \"{id}\": reveal.style is one of {}", REVEAL_STYLES.join(", ")));
        }
    }
    if let LayerKind::Path { d, .. } = &l.kind
        && !d.is_empty()
        && let Err(e) = crate::path::parse(d)
    {
        return Err(format!("layer \"{id}\": path data: {e}"));
    }
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

fn validate_objects(objects: &[Object3d], ids: &mut HashSet<String>) -> Result<(), String> {
    let mut result = Ok(());
    walk_objects(objects, &mut |o| {
        if result.is_err() {
            return;
        }
        result = (|| {
            if o.id.trim().is_empty() {
                return Err("Every object needs an id.".to_string());
            }
            if !ids.insert(o.id.clone()) {
                return Err(format!("Two things have the id \"{}\"; ids must be unique.", o.id));
            }
            check_color(&o.material.color, &format!("object \"{}\" color", o.id))?;
            if let Some(e) = &o.material.emissive {
                check_color(e, &format!("object \"{}\" emissive", o.id))?;
            }
            let mut probe = o.clone();
            for (name, keys) in &o.keyframes {
                for k in keys {
                    probe.set(name, &k.value).map_err(|e| format!("object \"{}\" keyframes: {e}", o.id))?;
                }
            }
            Ok(())
        })();
    });
    result
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
    if kind == "group" {
        for child in v.get("layers").and_then(Value::as_array).into_iter().flatten() {
            check_layer_keys(child)?;
        }
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
        return Err(format!("object \"{id}\": unknown type `{kind}`.{hint} Types: {}; lights go in `lights`.", names.join(", ")));
    };
    let allowed: Vec<&str> = OBJECT_KEYS.iter().chain(own.iter()).copied().collect();
    check_keys(v, &allowed, &format!("{kind} \"{id}\""))?;
    if let Some(m) = v.get("material") {
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
