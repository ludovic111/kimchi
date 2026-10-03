//! Stacks: lists of typed items with parameters that every scene thing can carry, described by
//! one table per family so validation, the guide, the window's forms and keyframes all read the
//! same thing.
//!
//! - 3D objects: `modifiers` (subdivision, mirror, array, bevel, twist… evaluated in order, like
//!   Blender's modifier stack) and `constraints` (lookAt, followPath…).
//! - 2D layers: `effects` (blur, glow, colour, distortions… like After Effects' effect stack),
//!   `operators` (repeater, zigzag, offset… on shapes), `masks`, and text `animators`.
//!
//! In JSON an item is flat: `{"id": "blur1", "type": "blur", "radius": 12}`. The id is optional
//! (one is made from the type) and is how keyframes reach a parameter:
//! `"effects.blur1.radius": [[0, 0], [1, 12]]`. Parameters left out take their default.

use std::fmt;
use std::marker::PhantomData;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::anim::{KeyValue, Rgba};

/// How a parameter is written and edited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    /// A number between `min` and `max` (values outside are clamped when used), edited in `step`s.
    Number { min: f64, max: f64, step: f64 },
    /// A whole number.
    Int { min: f64, max: f64 },
    Bool,
    /// `"#rrggbb"` or `"#rrggbbaa"`.
    Color,
    /// One of a list of words.
    Choice(&'static [&'static str]),
    Text,
    /// `[x, y]`.
    Vec2,
    /// `[x, y, z]` (one number is used for all three).
    Vec3,
    /// The id of another thing in the scene (an object, a layer).
    Ref,
    /// SVG path data in the layer's pixels (`"M0 0 L100 0 …"`).
    Path,
}

impl ParamKind {
    /// Numbers, vectors and colours change smoothly between keyframes; the others hold.
    pub fn animatable(&self) -> bool {
        !matches!(self, ParamKind::Bool | ParamKind::Choice(_) | ParamKind::Ref)
    }

    fn describe(&self) -> String {
        match self {
            ParamKind::Number { min, max, .. } => format!("number {}–{}", fmt_n(*min), fmt_n(*max)),
            ParamKind::Int { min, max } => format!("whole number {}–{}", fmt_n(*min), fmt_n(*max)),
            ParamKind::Bool => "true or false".into(),
            ParamKind::Color => "colour".into(),
            ParamKind::Choice(c) => c.join(" | "),
            ParamKind::Text => "text".into(),
            ParamKind::Vec2 => "[x, y]".into(),
            ParamKind::Vec3 => "[x, y, z]".into(),
            ParamKind::Ref => "id".into(),
            ParamKind::Path => "SVG path data".into(),
        }
    }
}

fn fmt_n(v: f64) -> String {
    if v <= -1e6 {
        "−∞".into()
    } else if v >= 1e6 {
        "∞".into()
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// A parameter's default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Def {
    N(f64),
    B(bool),
    S(&'static str),
    V2([f64; 2]),
    V3([f64; 3]),
    /// No value: the parameter is unset unless given (a reference, a path).
    None,
}

impl Def {
    pub fn value(&self) -> Option<Value> {
        Some(match self {
            Def::N(n) => Value::from(*n),
            Def::B(b) => Value::from(*b),
            Def::S(s) => Value::from(*s),
            Def::V2(v) => Value::from(v.to_vec()),
            Def::V3(v) => Value::from(v.to_vec()),
            Def::None => return None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    pub name: &'static str,
    /// Short label for the window.
    pub label: &'static str,
    pub kind: ParamKind,
    pub default: Def,
    pub doc: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct TypeSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub doc: &'static str,
    pub params: &'static [ParamSpec],
}

impl TypeSpec {
    pub fn param(&self, name: &str) -> Option<&'static ParamSpec> {
        self.params.iter().find(|p| p.name == name)
    }

    /// One line per parameter, for the guide and errors.
    pub fn describe(&self) -> String {
        let mut s = format!("`{}`: {}", self.name, self.doc);
        for p in self.params {
            let def = match p.default.value() {
                Some(v) => format!(", default {v}"),
                None => String::new(),
            };
            s.push_str(&format!("\n  - `{}` ({}{def}): {}", p.name, p.kind.describe(), p.doc));
        }
        s
    }
}

/// A family of stack items: its JSON field and its types.
pub trait Family: 'static {
    /// The field it lives in (`"modifiers"`, `"effects"`…), also the first part of keyframe names.
    const FIELD: &'static str;
    /// What one is called in messages ("modifier", "effect").
    const NOUN: &'static str;
    /// The type when none is given (families with one obvious type).
    const DEFAULT_TYPE: Option<&'static str> = None;
    fn types() -> &'static [TypeSpec];
}

/// One item of a stack: its id, type, whether it is on, and its parameters (as given; defaults
/// are filled in when read).
pub struct Stacked<F: Family> {
    pub id: String,
    pub kind: String,
    pub enabled: bool,
    pub params: Map<String, Value>,
    _f: PhantomData<F>,
}

impl<F: Family> Clone for Stacked<F> {
    fn clone(&self) -> Self {
        Stacked { id: self.id.clone(), kind: self.kind.clone(), enabled: self.enabled, params: self.params.clone(), _f: PhantomData }
    }
}

impl<F: Family> PartialEq for Stacked<F> {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id && self.kind == o.kind && self.enabled == o.enabled && self.params == o.params
    }
}

impl<F: Family> fmt::Debug for Stacked<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(F::NOUN).field("id", &self.id).field("type", &self.kind).field("enabled", &self.enabled).field("params", &self.params).finish()
    }
}

impl<F: Family> Stacked<F> {
    pub fn new(kind: &str, params: Map<String, Value>) -> Self {
        Stacked { id: String::new(), kind: kind.to_string(), enabled: true, params, _f: PhantomData }
    }

    pub fn spec(&self) -> Option<&'static TypeSpec> {
        F::types().iter().find(|t| t.name == self.kind)
    }

    fn raw(&self, name: &str) -> Option<Value> {
        self.params.get(name).cloned().or_else(|| self.spec()?.param(name)?.default.value())
    }

    /// A number parameter (its default when unset), clamped to its range.
    pub fn n(&self, name: &str) -> f64 {
        let v = self.raw(name).and_then(|v| v.as_f64()).unwrap_or(0.0);
        match self.spec().and_then(|s| s.param(name)).map(|p| p.kind) {
            Some(ParamKind::Number { min, max, .. } | ParamKind::Int { min, max }) => v.clamp(min, max),
            _ => v,
        }
    }

    pub fn b(&self, name: &str) -> bool {
        self.raw(name).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// A text, choice, colour, reference or path parameter ("" when unset).
    pub fn s(&self, name: &str) -> String {
        self.raw(name).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }

    /// A reference or path parameter, if set.
    pub fn opt_s(&self, name: &str) -> Option<String> {
        self.raw(name).and_then(|v| v.as_str().map(str::to_string)).filter(|s| !s.is_empty())
    }

    pub fn v2(&self, name: &str) -> [f64; 2] {
        let v = self.raw(name).and_then(|v| serde_json::from_value::<KeyValue>(v).ok()).and_then(|v| v.as_vec(2));
        v.map_or([0.0; 2], |v| [v[0], v[1]])
    }

    pub fn v3(&self, name: &str) -> [f64; 3] {
        let v = self.raw(name).and_then(|v| serde_json::from_value::<KeyValue>(v).ok()).and_then(|v| v.as_vec(3));
        v.map_or([0.0; 3], |v| [v[0], v[1], v[2]])
    }

    /// A parameter as a keyframe value (with its default).
    pub fn get(&self, name: &str) -> Option<KeyValue> {
        let p = self.spec()?.param(name)?;
        let v = self.raw(name)?;
        match p.kind {
            ParamKind::Bool => Some(KeyValue::Number(if v.as_bool()? { 1.0 } else { 0.0 })),
            _ => serde_json::from_value(v).ok(),
        }
    }

    /// Sets a parameter from a keyframe or a typed value, checked against its kind.
    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        let spec = self.spec().ok_or_else(|| format!("unknown {} type `{}`", F::NOUN, self.kind))?;
        let p = spec.param(name).ok_or_else(|| unknown_param::<F>(spec, name))?;
        let json = match (p.kind, v) {
            (ParamKind::Number { .. } | ParamKind::Int { .. }, _) => Value::from(v.as_f64().ok_or_else(|| format!("{name} takes a number"))?),
            (ParamKind::Bool, KeyValue::Number(n)) => Value::from(*n >= 0.5),
            (ParamKind::Bool, _) => return Err(format!("{name} takes true or false")),
            (ParamKind::Vec2, _) => Value::from(v.as_vec(2).ok_or_else(|| format!("{name} takes [x, y]"))?),
            (ParamKind::Vec3, _) => Value::from(v.as_vec(3).ok_or_else(|| format!("{name} takes [x, y, z] or one number"))?),
            (ParamKind::Color, _) => {
                let c = v.as_str().ok_or_else(|| format!("{name} takes a colour like \"#ff5a36\""))?;
                super::check_color(c, name)?;
                Value::from(c)
            }
            (ParamKind::Choice(options), _) => {
                let c = v.as_str().ok_or_else(|| format!("{name} is one of {}", options.join(", ")))?;
                if !options.contains(&c) {
                    let hint = crate::closest(c, options).map(|h| format!(" Did you mean `{h}`?")).unwrap_or_default();
                    return Err(format!("{name} is one of {}, not `{c}`.{hint}", options.join(", ")));
                }
                Value::from(c)
            }
            (ParamKind::Path, _) => {
                let d = v.as_str().ok_or_else(|| format!("{name} takes SVG path data"))?;
                if !d.trim().is_empty() {
                    crate::path::parse(d).map_err(|e| format!("{name}: {e}"))?;
                }
                Value::from(d)
            }
            (ParamKind::Text | ParamKind::Ref, _) => Value::from(v.as_str().ok_or_else(|| format!("{name} takes text"))?),
        };
        self.params.insert(name.to_string(), json);
        Ok(())
    }

    /// Every parameter checked (types, choices, colours); ids it refers to are pushed to `refs`.
    pub fn check(&self, refs: &mut Vec<String>) -> Result<(), String> {
        let spec = self.spec().ok_or_else(|| unknown_type::<F>(&self.kind))?;
        for (name, v) in &self.params {
            let p = spec.param(name).ok_or_else(|| unknown_param::<F>(spec, name))?;
            let kv = match (p.kind, v) {
                (ParamKind::Bool, Value::Bool(b)) => KeyValue::Number(if *b { 1.0 } else { 0.0 }),
                (ParamKind::Bool, _) => return Err(format!("{} \"{}\": {name} takes true or false", F::NOUN, self.id)),
                _ => serde_json::from_value::<KeyValue>(v.clone()).map_err(|_| format!("{} \"{}\": {name} takes a {}", F::NOUN, self.id, p.kind.describe()))?,
            };
            let mut probe = self.clone();
            probe.set(name, &kv).map_err(|e| format!("{} \"{}\": {e}", F::NOUN, self.id))?;
            if p.kind == ParamKind::Ref
                && let Some(r) = v.as_str().filter(|r| !r.is_empty())
            {
                refs.push(r.to_string());
            }
        }
        Ok(())
    }

    /// The item as JSON with every parameter (defaults filled in), for the window and `motion.get`.
    pub fn full_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("id".into(), Value::from(self.id.clone()));
        out.insert("type".into(), Value::from(self.kind.clone()));
        out.insert("enabled".into(), Value::from(self.enabled));
        if let Some(spec) = self.spec() {
            for p in spec.params {
                if let Some(v) = self.raw(p.name) {
                    out.insert(p.name.into(), v);
                }
            }
        }
        Value::Object(out)
    }
}

fn unknown_type<F: Family>(kind: &str) -> String {
    let names: Vec<&str> = F::types().iter().map(|t| t.name).collect();
    let hint = crate::closest(kind, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    format!("Unknown {} type `{kind}`.{hint} Types: {}.", F::NOUN, names.join(", "))
}

fn unknown_param<F: Family>(spec: &TypeSpec, name: &str) -> String {
    let names: Vec<&str> = spec.params.iter().map(|p| p.name).collect();
    let hint = crate::closest(name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    format!("A {} {} has no parameter `{name}`.{hint} Parameters: {}.", spec.name, F::NOUN, names.join(", "))
}

impl<F: Family> Serialize for Stacked<F> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = Map::new();
        if !self.id.is_empty() {
            out.insert("id".into(), Value::from(self.id.clone()));
        }
        out.insert("type".into(), Value::from(self.kind.clone()));
        if !self.enabled {
            out.insert("enabled".into(), Value::from(false));
        }
        for (k, v) in &self.params {
            out.insert(k.clone(), v.clone());
        }
        out.serialize(s)
    }
}

impl<'de, F: Family> Deserialize<'de> for Stacked<F> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let Value::Object(mut o) = v else {
            return Err(serde::de::Error::custom(format!("a {} is an object like {{\"type\": \"…\"}}", F::NOUN)));
        };
        let kind = match o.remove("type") {
            Some(Value::String(t)) => t,
            Some(_) => return Err(serde::de::Error::custom(format!("a {}'s type is a word", F::NOUN))),
            None => match F::DEFAULT_TYPE {
                Some(t) => t.to_string(),
                None => {
                    let names: Vec<&str> = F::types().iter().map(|t| t.name).collect();
                    return Err(serde::de::Error::custom(format!("a {} needs a type: {}", F::NOUN, names.join(", "))));
                }
            },
        };
        let id = match o.remove("id") {
            Some(Value::String(i)) => i,
            None | Some(Value::Null) => String::new(),
            Some(_) => return Err(serde::de::Error::custom(format!("a {}'s id is text", F::NOUN))),
        };
        let enabled = match o.remove("enabled") {
            Some(Value::Bool(b)) => b,
            None => true,
            Some(_) => return Err(serde::de::Error::custom("enabled takes true or false")),
        };
        Ok(Stacked { id, kind, enabled, params: o, _f: PhantomData })
    }
}

/// Gives every item of a stack an id (`blur`, `blur2`…) and checks they are unique.
pub fn name_items<F: Family>(items: &mut [Stacked<F>]) {
    let mut taken: Vec<String> = items.iter().filter(|i| !i.id.is_empty()).map(|i| i.id.clone()).collect();
    for it in items.iter_mut().filter(|i| i.id.is_empty()) {
        let mut n = 1;
        let id = loop {
            let id = if n == 1 { it.kind.clone() } else { format!("{}{n}", it.kind) };
            if !taken.contains(&id) {
                break id;
            }
            n += 1;
        };
        taken.push(id.clone());
        it.id = id;
    }
}

/// Checks a stack (types, params, unique ids); referenced ids go to `refs`.
pub fn check_items<F: Family>(items: &[Stacked<F>], owner: &str, refs: &mut Vec<String>) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for it in items {
        if !it.id.is_empty() && !seen.insert(it.id.as_str()) {
            return Err(format!("{owner}: two {}s have the id \"{}\"", F::NOUN, it.id));
        }
        it.check(refs).map_err(|e| format!("{owner}: {e}"))?;
    }
    Ok(())
}

/// For keyframe names like `effects.blur.radius`: the item and parameter, when `name` is in
/// this family.
pub fn split_name<'a, F: Family>(name: &'a str) -> Option<(&'a str, &'a str)> {
    let rest = name.strip_prefix(F::FIELD)?.strip_prefix('.')?;
    rest.split_once('.')
}

/// Sets `field.id.param` in a stack; `Ok(false)` when the name isn't this family's.
pub fn set_in<F: Family>(items: &mut [Stacked<F>], name: &str, v: &KeyValue) -> Result<bool, String> {
    let Some((id, param)) = split_name::<F>(name) else { return Ok(false) };
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let it = items.iter_mut().find(|i| i.id == id).ok_or_else(|| {
        format!("no {} \"{id}\"; {}", F::NOUN, if ids.is_empty() { format!("there are no {}", F::FIELD) } else { format!("ids: {}", ids.join(", ")) })
    })?;
    it.set(param, v)?;
    Ok(true)
}

/// Reads `field.id.param` from a stack.
pub fn get_in<F: Family>(items: &[Stacked<F>], name: &str) -> Option<KeyValue> {
    let (id, param) = split_name::<F>(name)?;
    items.iter().find(|i| i.id == id)?.get(param)
}

/// The animatable parameter names of a stack (`effects.blur.radius`…), for listings.
pub fn animatable_names<F: Family>(items: &[Stacked<F>]) -> Vec<String> {
    let mut out = vec![];
    for it in items {
        if let Some(spec) = it.spec() {
            for p in spec.params.iter().filter(|p| p.kind.animatable()) {
                out.push(format!("{}.{}.{}", F::FIELD, it.id, p.name));
            }
        }
    }
    out
}

/// A colour parameter as RGBA (white when unset or wrong).
pub fn rgba(s: &str) -> Rgba {
    Rgba::parse(s).unwrap_or(Rgba([255.0; 4]))
}

// ---------------------------------------------------------------------------------------------
// The tables

const fn num(name: &'static str, label: &'static str, min: f64, max: f64, step: f64, default: f64, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Number { min, max, step }, default: Def::N(default), doc }
}
const fn int(name: &'static str, label: &'static str, min: f64, max: f64, default: f64, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Int { min, max }, default: Def::N(default), doc }
}
const fn flag(name: &'static str, label: &'static str, default: bool, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Bool, default: Def::B(default), doc }
}
const fn color(name: &'static str, label: &'static str, default: &'static str, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Color, default: Def::S(default), doc }
}
const fn choice(name: &'static str, label: &'static str, options: &'static [&'static str], default: &'static str, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Choice(options), default: Def::S(default), doc }
}
const fn v2(name: &'static str, label: &'static str, default: [f64; 2], doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Vec2, default: Def::V2(default), doc }
}
const fn v3(name: &'static str, label: &'static str, default: [f64; 3], doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Vec3, default: Def::V3(default), doc }
}
const fn refer(name: &'static str, label: &'static str, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Ref, default: Def::None, doc }
}
const fn path(name: &'static str, label: &'static str, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Path, default: Def::None, doc }
}
#[allow(dead_code)]
const fn text(name: &'static str, label: &'static str, default: &'static str, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Text, default: Def::S(default), doc }
}

const INF: f64 = 1e9;
const AXES: &[&str] = &["x", "y", "z"];

// ---- 3D modifiers ----------------------------------------------------------------------------

pub struct ModifierFamily;
impl Family for ModifierFamily {
    const FIELD: &'static str = "modifiers";
    const NOUN: &'static str = "modifier";
    fn types() -> &'static [TypeSpec] {
        MODIFIERS
    }
}
/// A 3D object's modifier (see [`MODIFIERS`]).
pub type Modifier = Stacked<ModifierFamily>;

pub static MODIFIERS: &[TypeSpec] = &[
    TypeSpec { name: "subdivision", label: "Subdivision", doc: "Smooths the surface by splitting every face (Catmull-Clark), like Blender's Subdivision Surface.", params: &[
        int("levels", "Levels", 0.0, 4.0, 2.0, "Times each face is split in four."),
        flag("simple", "Simple", false, "Split without rounding (keeps the shape, adds detail)."),
    ]},
    TypeSpec { name: "mirror", label: "Mirror", doc: "Adds a mirrored copy across the object's own axis, welded at the middle.", params: &[
        choice("axis", "Axis", AXES, "x", "Mirror across this axis."),
        flag("merge", "Merge", true, "Weld vertices that touch the mirror plane."),
        flag("bisect", "Bisect", false, "Cut away what is already on the other side first."),
    ]},
    TypeSpec { name: "array", label: "Array", doc: "Repeats the object: in a line (offset), around a point (rotation), growing or shrinking (scale). Each copy adds the step to the previous one.", params: &[
        int("count", "Count", 1.0, 1000.0, 3.0, "Copies, the original included."),
        v3("relative", "Relative offset", [1.1, 0.0, 0.0], "Step between copies in multiples of the object's size."),
        v3("offset", "Offset", [0.0, 0.0, 0.0], "Step between copies in world units, added to the relative one."),
        v3("rotation", "Rotation", [0.0, 0.0, 0.0], "Degrees each copy turns more than the previous one (around the object's origin)."),
        num("scale", "Scale", 0.0, 100.0, 0.01, 1.0, "Each copy's size relative to the previous one."),
        flag("merge", "Merge", false, "Weld vertices where copies touch."),
    ]},
    TypeSpec { name: "bevel", label: "Bevel", doc: "Rounds or chamfers sharp edges (those bent more than the angle).", params: &[
        num("width", "Width", 0.0, INF, 0.01, 0.05, "How far the bevel reaches from the edge, world units."),
        int("segments", "Segments", 1.0, 16.0, 3.0, "1 = a flat chamfer; more = rounder."),
        num("angle", "Angle", 0.0, 180.0, 1.0, 30.0, "Only edges sharper than this (degrees)."),
    ]},
    TypeSpec { name: "solidify", label: "Solidify", doc: "Gives a surface thickness (a plane becomes a slab, a shell gets an inside).", params: &[
        num("thickness", "Thickness", -INF, INF, 0.01, 0.1, "World units, along the normals (negative goes outward)."),
        flag("rim", "Fill rim", true, "Close the open edges between the two sides."),
    ]},
    TypeSpec { name: "displace", label: "Displace", doc: "Pushes vertices by smooth noise: rocks, terrain, wobbling blobs (animate evolution).", params: &[
        num("strength", "Strength", -INF, INF, 0.01, 0.2, "Distance at full noise, world units."),
        num("scale", "Noise size", 0.001, INF, 0.05, 1.0, "Size of the bumps, world units."),
        num("evolution", "Evolution", -INF, INF, 0.01, 0.0, "Moves through the noise: animate it for a living surface."),
        choice("direction", "Direction", &["normal", "x", "y", "z"], "normal", "Which way vertices move."),
        int("octaves", "Detail", 1.0, 8.0, 2.0, "Layers of finer noise."),
        int("seed", "Seed", 0.0, 1e6, 0.0, "Another seed gives other bumps."),
    ]},
    TypeSpec { name: "twist", label: "Twist", doc: "Twists the object around an axis (degrees from one end to the other).", params: &[
        num("angle", "Angle", -INF, INF, 1.0, 90.0, "Degrees of turn across the object's length."),
        choice("axis", "Axis", AXES, "y", "The axis it twists around."),
    ]},
    TypeSpec { name: "bend", label: "Bend", doc: "Bends the object along an axis into an arc.", params: &[
        num("angle", "Angle", -INF, INF, 1.0, 90.0, "Degrees of bend across the object's length."),
        choice("axis", "Axis", AXES, "x", "The length that bends."),
        choice("toward", "Toward", AXES, "y", "The direction it bends towards."),
    ]},
    TypeSpec { name: "taper", label: "Taper", doc: "Narrows (or widens) the object towards one end.", params: &[
        num("amount", "Amount", -10.0, 10.0, 0.01, 0.5, "0 = unchanged, 1 = a point at the far end, negative widens."),
        choice("axis", "Axis", AXES, "y", "Along this axis."),
    ]},
    TypeSpec { name: "wave", label: "Wave", doc: "A travelling wave through the surface (water, flags). Moves with time on its own.", params: &[
        num("amplitude", "Height", -INF, INF, 0.01, 0.1, "Height of the waves, world units."),
        num("wavelength", "Wavelength", 0.001, INF, 0.05, 1.0, "Distance between crests, world units."),
        num("speed", "Speed", -INF, INF, 0.05, 1.0, "Crests per second."),
        choice("axis", "Along", AXES, "x", "The direction the waves travel."),
        choice("direction", "Moves", &["normal", "x", "y", "z"], "y", "Which way the surface moves."),
        flag("radial", "Ripples", false, "Rings spreading from the centre instead of straight crests."),
    ]},
    TypeSpec { name: "smooth", label: "Smooth", doc: "Relaxes the shape (averages each vertex with its neighbours).", params: &[
        num("factor", "Factor", 0.0, 1.0, 0.01, 0.5, "How far each step moves."),
        int("iterations", "Repeat", 0.0, 100.0, 5.0, "Steps."),
    ]},
    TypeSpec { name: "wireframe", label: "Wireframe", doc: "Turns every edge into a thin bar: a wireframe that renders.", params: &[
        num("thickness", "Thickness", 0.0, INF, 0.005, 0.02, "Bar width, world units."),
    ]},
    TypeSpec { name: "boolean", label: "Boolean", doc: "Cuts with, joins with, or keeps the overlap with another object (hide that object to see only the result).", params: &[
        refer("object", "Object", "The other object's id."),
        choice("operation", "Operation", &["difference", "union", "intersect"], "difference", "difference cuts it away, union joins, intersect keeps the overlap."),
    ]},
    TypeSpec { name: "decimate", label: "Decimate", doc: "Uses fewer faces (a low-poly look).", params: &[
        num("ratio", "Ratio", 0.0, 1.0, 0.01, 0.5, "Share of faces kept."),
    ]},
    TypeSpec { name: "triangulate", label: "Triangulate", doc: "Splits every face into triangles (with flat shading, a faceted look).", params: &[] },
    TypeSpec { name: "explode", label: "Explode", doc: "Faces fly apart from the centre: animate progress 0 → 1.", params: &[
        num("progress", "Progress", 0.0, INF, 0.01, 0.0, "0 = whole; 1 = every face at its full distance."),
        num("distance", "Distance", 0.0, INF, 0.05, 2.0, "How far faces travel, world units."),
        num("spin", "Spin", -INF, INF, 1.0, 180.0, "Degrees each piece tumbles."),
        num("gravity", "Gravity", -INF, INF, 0.05, 0.0, "Pieces fall by this much at progress 1."),
        int("seed", "Seed", 0.0, 1e6, 0.0, "Another seed, other directions."),
    ]},
    TypeSpec { name: "build", label: "Build", doc: "Faces appear one after another: animate progress 0 → 1 to build the object.", params: &[
        num("progress", "Progress", 0.0, 1.0, 0.01, 1.0, "Share of faces shown."),
        choice("order", "Order", &["index", "random", "x", "y", "z", "distance"], "random", "Which faces come first."),
        int("seed", "Seed", 0.0, 1e6, 0.0, "For random order."),
        flag("reverse", "Reverse", false, "Remove faces instead."),
    ]},
    TypeSpec { name: "weld", label: "Weld", doc: "Merges vertices closer than a distance.", params: &[
        num("distance", "Distance", 0.0, INF, 0.0005, 0.001, "World units."),
    ]},
    TypeSpec { name: "spherify", label: "Cast to sphere", doc: "Pulls the shape towards a sphere.", params: &[
        num("factor", "Factor", -2.0, 2.0, 0.01, 0.5, "0 = unchanged, 1 = a sphere."),
    ]},
    TypeSpec { name: "noise", label: "Jitter", doc: "Moves each vertex by a random amount (a crumpled, hand-made look). Animate seed or set speed for a boil.", params: &[
        num("amount", "Amount", 0.0, INF, 0.005, 0.02, "World units."),
        int("seed", "Seed", 0.0, 1e6, 0.0, "Another seed, another crumple."),
        num("speed", "Boil", 0.0, 60.0, 1.0, 0.0, "New crumples per second (0 = still)."),
    ]},
];

// ---- 3D constraints ----------------------------------------------------------------------

pub struct ConstraintFamily;
impl Family for ConstraintFamily {
    const FIELD: &'static str = "constraints";
    const NOUN: &'static str = "constraint";
    fn types() -> &'static [TypeSpec] {
        CONSTRAINTS
    }
}
/// A 3D object's (or light's, camera's) constraint (see [`CONSTRAINTS`]).
pub type Constraint = Stacked<ConstraintFamily>;

pub static CONSTRAINTS: &[TypeSpec] = &[
    TypeSpec { name: "lookAt", label: "Look at", doc: "Turns to face another object (cameras and lights aim at it).", params: &[
        refer("target", "Target", "The object to face."),
        v3("offset", "Offset", [0.0, 0.0, 0.0], "Aim this far from the target's centre."),
        num("influence", "Influence", 0.0, 1.0, 0.01, 1.0, "0 = no effect, 1 = full."),
    ]},
    TypeSpec { name: "followPath", label: "Follow path", doc: "Moves along a curve object; animate progress 0 → 1.", params: &[
        refer("path", "Path", "A curve object's id."),
        num("progress", "Progress", -INF, INF, 0.001, 0.0, "0 = the start of the curve, 1 = its end (wraps on closed curves)."),
        flag("align", "Follow direction", true, "Turn to face along the curve."),
        num("influence", "Influence", 0.0, 1.0, 0.01, 1.0, "0 = no effect, 1 = full."),
    ]},
    TypeSpec { name: "copyPosition", label: "Copy position", doc: "Moves with another object.", params: &[
        refer("target", "Target", "The object to follow."),
        v3("offset", "Offset", [0.0, 0.0, 0.0], "Added to the target's position."),
        num("influence", "Influence", 0.0, 1.0, 0.01, 1.0, "0 = no effect, 1 = full."),
    ]},
    TypeSpec { name: "copyRotation", label: "Copy rotation", doc: "Turns like another object.", params: &[
        refer("target", "Target", "The object to copy."),
        num("influence", "Influence", 0.0, 1.0, 0.01, 1.0, "0 = no effect, 1 = full."),
    ]},
    TypeSpec { name: "copyScale", label: "Copy scale", doc: "Scales like another object.", params: &[
        refer("target", "Target", "The object to copy."),
        num("influence", "Influence", 0.0, 1.0, 0.01, 1.0, "0 = no effect, 1 = full."),
    ]},
    TypeSpec { name: "limitPosition", label: "Limit position", doc: "Keeps the object inside a box.", params: &[
        v3("min", "Min", [-1e6, -1e6, -1e6], "Lowest x, y, z."),
        v3("max", "Max", [1e6, 1e6, 1e6], "Highest x, y, z."),
    ]},
    TypeSpec { name: "floor", label: "Floor", doc: "Never goes below a height (objects resting on the ground).", params: &[
        num("height", "Height", -INF, INF, 0.05, 0.0, "World y."),
    ]},
];

// ---- 2D effects ----------------------------------------------------------------------------

pub struct EffectFamily;
impl Family for EffectFamily {
    const FIELD: &'static str = "effects";
    const NOUN: &'static str = "effect";
    fn types() -> &'static [TypeSpec] {
        EFFECTS
    }
}
/// A 2D layer's effect (see [`EFFECTS`]), applied to the layer's picture in order.
pub type Effect = Stacked<EffectFamily>;

pub static EFFECTS: &[TypeSpec] = &[
    // Blur and light
    TypeSpec { name: "blur", label: "Gaussian blur", doc: "Softens the layer.", params: &[
        num("radius", "Radius", 0.0, 1000.0, 0.5, 10.0, "Pixels."),
        choice("dimensions", "Direction", &["both", "horizontal", "vertical"], "both", "Blur in both directions or one."),
    ]},
    TypeSpec { name: "directionalBlur", label: "Directional blur", doc: "Smears the layer along an angle (speed lines).", params: &[
        num("length", "Length", 0.0, 2000.0, 1.0, 30.0, "Pixels."),
        num("angle", "Angle", -INF, INF, 1.0, 0.0, "Degrees, 0 = horizontal."),
    ]},
    TypeSpec { name: "radialBlur", label: "Radial blur", doc: "Zooms or spins the layer's picture around a point.", params: &[
        num("amount", "Amount", 0.0, 100.0, 0.5, 10.0, "Zoom: percent of the distance; spin: degrees."),
        choice("kind", "Kind", &["zoom", "spin"], "zoom", "Rays from the centre or circles around it."),
        v2("center", "Centre", [0.0, 0.0], "From the canvas centre, pixels."),
    ]},
    TypeSpec { name: "glow", label: "Glow", doc: "Bright parts shine.", params: &[
        num("radius", "Radius", 0.0, 1000.0, 0.5, 24.0, "Pixels."),
        num("intensity", "Intensity", 0.0, 10.0, 0.05, 1.0, "How strong."),
        num("threshold", "Threshold", 0.0, 1.0, 0.01, 0.0, "Only parts brighter than this glow."),
        color("color", "Colour", "#00000000", "Tint of the glow (transparent = the layer's own colours)."),
    ]},
    TypeSpec { name: "dropShadow", label: "Drop shadow", doc: "A soft shadow under the layer.", params: &[
        color("color", "Colour", "#000000b3", "Shadow colour."),
        num("blur", "Softness", 0.0, 500.0, 0.5, 16.0, "Pixels."),
        num("distance", "Distance", 0.0, 2000.0, 0.5, 10.0, "Pixels."),
        num("angle", "Angle", -INF, INF, 1.0, 135.0, "Degrees the light comes from (135 = top left)."),
    ]},
    TypeSpec { name: "stroke", label: "Outline", doc: "An outline around the layer's shape (any layer: text, images).", params: &[
        color("color", "Colour", "#ffffff", "Outline colour."),
        num("width", "Width", 0.0, 500.0, 0.5, 4.0, "Pixels."),
        choice("position", "Position", &["outside", "center", "inside"], "outside", "Where the outline goes."),
    ]},
    TypeSpec { name: "echo", label: "Echo", doc: "Trails of the layer from earlier moments (motion echoes).", params: &[
        int("count", "Echoes", 1.0, 32.0, 4.0, "How many."),
        num("delay", "Delay", -10.0, 10.0, 0.005, 0.05, "Seconds between echoes (negative: later moments)."),
        num("decay", "Decay", 0.0, 1.0, 0.01, 0.6, "Each echo's opacity relative to the one before."),
    ]},
    // Colour
    TypeSpec { name: "colorCorrect", label: "Colour correction", doc: "Brightness, contrast, saturation, hue, exposure and gamma.", params: &[
        num("brightness", "Brightness", -1.0, 1.0, 0.01, 0.0, "-1 to 1."),
        num("contrast", "Contrast", -1.0, 1.0, 0.01, 0.0, "-1 to 1."),
        num("saturation", "Saturation", -1.0, 1.0, 0.01, 0.0, "-1 (grey) to 1."),
        num("hue", "Hue", -180.0, 180.0, 1.0, 0.0, "Degrees around the colour wheel."),
        num("exposure", "Exposure", -10.0, 10.0, 0.05, 0.0, "Stops."),
        num("gamma", "Gamma", 0.1, 10.0, 0.01, 1.0, "Midtones (above 1 brighter)."),
    ]},
    TypeSpec { name: "levels", label: "Levels", doc: "Remaps the input range to the output range.", params: &[
        num("inBlack", "Input black", 0.0, 1.0, 0.01, 0.0, ""),
        num("inWhite", "Input white", 0.0, 1.0, 0.01, 1.0, ""),
        num("gamma", "Gamma", 0.1, 10.0, 0.01, 1.0, ""),
        num("outBlack", "Output black", 0.0, 1.0, 0.01, 0.0, ""),
        num("outWhite", "Output white", 0.0, 1.0, 0.01, 1.0, ""),
    ]},
    TypeSpec { name: "tint", label: "Tint", doc: "Maps dark to one colour and light to another (duotone).", params: &[
        color("black", "Dark to", "#000000", ""),
        color("white", "Light to", "#ffffff", ""),
        num("amount", "Amount", 0.0, 1.0, 0.01, 1.0, ""),
    ]},
    TypeSpec { name: "tritone", label: "Tritone", doc: "Shadows, midtones and highlights each to a colour.", params: &[
        color("shadows", "Shadows", "#1a1033", ""),
        color("midtones", "Midtones", "#ff5a36", ""),
        color("highlights", "Highlights", "#fff4e0", ""),
        num("amount", "Amount", 0.0, 1.0, 0.01, 1.0, ""),
    ]},
    TypeSpec { name: "fill", label: "Fill", doc: "Paints the layer one colour, keeping its shape.", params: &[
        color("color", "Colour", "#ff5a36", ""),
        num("amount", "Amount", 0.0, 1.0, 0.01, 1.0, ""),
    ]},
    TypeSpec { name: "gradientRamp", label: "Gradient ramp", doc: "Paints a gradient over the layer's shape.", params: &[
        v2("from", "From", [-500.0, 0.0], "Start point from the canvas centre, pixels."),
        v2("to", "To", [500.0, 0.0], "End point."),
        color("colorFrom", "Start colour", "#ff5a36", ""),
        color("colorTo", "End colour", "#3654ff", ""),
        choice("kind", "Shape", &["linear", "radial"], "linear", ""),
        num("amount", "Amount", 0.0, 1.0, 0.01, 1.0, "How much replaces the layer's colours."),
    ]},
    TypeSpec { name: "invert", label: "Invert", doc: "Negative colours.", params: &[num("amount", "Amount", 0.0, 1.0, 0.01, 1.0, "")] },
    TypeSpec { name: "threshold", label: "Threshold", doc: "Black and white only, split at a level.", params: &[num("level", "Level", 0.0, 1.0, 0.01, 0.5, "")] },
    TypeSpec { name: "posterize", label: "Posterize", doc: "Fewer colour steps.", params: &[int("levels", "Levels", 2.0, 64.0, 5.0, "Steps per channel.")] },
    TypeSpec { name: "vignette", label: "Vignette", doc: "Darker edges.", params: &[
        num("amount", "Amount", 0.0, 1.0, 0.01, 0.5, ""),
        num("size", "Size", 0.0, 2.0, 0.01, 0.75, "Radius of the clear middle (1 = to the corners)."),
        num("softness", "Softness", 0.0, 1.0, 0.01, 0.5, ""),
    ]},
    // Generate
    TypeSpec { name: "noise", label: "Grain", doc: "Film grain over the layer.", params: &[
        num("amount", "Amount", 0.0, 1.0, 0.01, 0.15, ""),
        flag("color", "Colour grain", false, "Separate grain per channel."),
        flag("animated", "Animated", true, "New grain every frame."),
        int("seed", "Seed", 0.0, 1e6, 0.0, ""),
    ]},
    TypeSpec { name: "fractalNoise", label: "Fractal noise", doc: "Fills the layer's shape with cloudy noise (smoke, textures); animate evolution.", params: &[
        num("scale", "Size", 1.0, 10000.0, 1.0, 200.0, "Pixels."),
        int("complexity", "Complexity", 1.0, 10.0, 4.0, "Layers of detail."),
        num("evolution", "Evolution", -INF, INF, 0.01, 0.0, "Moves through the noise; animate it."),
        num("contrast", "Contrast", 0.0, 10.0, 0.05, 1.0, ""),
        color("colorA", "Dark", "#000000", ""),
        color("colorB", "Light", "#ffffff", ""),
        num("amount", "Amount", 0.0, 1.0, 0.01, 1.0, "How much replaces the layer's colours."),
        int("seed", "Seed", 0.0, 1e6, 0.0, ""),
    ]},
    TypeSpec { name: "halftone", label: "Halftone", doc: "Print dots.", params: &[
        num("size", "Dot size", 2.0, 200.0, 0.5, 10.0, "Pixels."),
        num("angle", "Angle", -INF, INF, 1.0, 45.0, "Degrees."),
        flag("color", "Colour", false, "Keep colours (else black dots)."),
    ]},
    TypeSpec { name: "scanlines", label: "Scanlines", doc: "Horizontal lines like an old screen.", params: &[
        num("spacing", "Spacing", 1.0, 100.0, 0.5, 4.0, "Pixels."),
        num("amount", "Amount", 0.0, 1.0, 0.01, 0.3, ""),
        num("speed", "Roll", -1000.0, 1000.0, 1.0, 0.0, "Pixels per second."),
    ]},
    // Distort
    TypeSpec { name: "turbulentDisplace", label: "Turbulent displace", doc: "Warps the layer with flowing noise (heat haze, liquid); animate evolution.", params: &[
        num("amount", "Amount", 0.0, 1000.0, 0.5, 20.0, "Pixels."),
        num("size", "Size", 1.0, 10000.0, 1.0, 100.0, "Pixels."),
        num("evolution", "Evolution", -INF, INF, 0.01, 0.0, ""),
        int("seed", "Seed", 0.0, 1e6, 0.0, ""),
    ]},
    TypeSpec { name: "waveWarp", label: "Wave warp", doc: "Waves through the layer (flags, water); moves with time.", params: &[
        num("height", "Height", 0.0, 1000.0, 0.5, 10.0, "Pixels."),
        num("width", "Wavelength", 1.0, 10000.0, 1.0, 120.0, "Pixels."),
        num("speed", "Speed", -100.0, 100.0, 0.05, 1.0, "Waves per second."),
        num("angle", "Direction", -INF, INF, 1.0, 0.0, "Degrees the waves travel."),
    ]},
    TypeSpec { name: "ripple", label: "Ripple", doc: "Rings spreading from a point.", params: &[
        num("amplitude", "Height", 0.0, 1000.0, 0.5, 8.0, "Pixels."),
        num("wavelength", "Wavelength", 1.0, 10000.0, 1.0, 60.0, "Pixels."),
        num("speed", "Speed", -100.0, 100.0, 0.05, 1.0, "Rings per second."),
        v2("center", "Centre", [0.0, 0.0], "From the canvas centre, pixels."),
    ]},
    TypeSpec { name: "twirl", label: "Twirl", doc: "Twists the picture around a point.", params: &[
        num("angle", "Angle", -INF, INF, 1.0, 90.0, "Degrees at the centre."),
        num("radius", "Radius", 1.0, 10000.0, 1.0, 300.0, "Pixels."),
        v2("center", "Centre", [0.0, 0.0], "From the canvas centre, pixels."),
    ]},
    TypeSpec { name: "bulge", label: "Bulge", doc: "Magnifies (or pinches) around a point.", params: &[
        num("amount", "Amount", -1.0, 4.0, 0.01, 0.5, "Negative pinches."),
        num("radius", "Radius", 1.0, 10000.0, 1.0, 300.0, "Pixels."),
        v2("center", "Centre", [0.0, 0.0], "From the canvas centre, pixels."),
    ]},
    TypeSpec { name: "mosaic", label: "Mosaic", doc: "Big square pixels.", params: &[num("size", "Block size", 1.0, 500.0, 1.0, 16.0, "Pixels.")] },
    TypeSpec { name: "chromaticAberration", label: "Chromatic aberration", doc: "Red and blue split apart (a lens fringe, a glitch).", params: &[
        num("amount", "Amount", 0.0, 200.0, 0.5, 6.0, "Pixels."),
        num("angle", "Angle", -INF, INF, 1.0, 0.0, "Degrees."),
    ]},
    TypeSpec { name: "glitch", label: "Glitch", doc: "Slices of the picture jump sideways and split colours, in bursts.", params: &[
        num("amount", "Amount", 0.0, 1.0, 0.01, 0.5, ""),
        num("speed", "Speed", 0.0, 60.0, 0.5, 8.0, "New glitches per second."),
        int("seed", "Seed", 0.0, 1e6, 0.0, ""),
    ]},
    TypeSpec { name: "mirror", label: "Mirror", doc: "Reflects one side of a line onto the other.", params: &[
        num("angle", "Angle", -INF, INF, 1.0, 0.0, "Degrees of the reflection line (0 = vertical line)."),
        v2("center", "Centre", [0.0, 0.0], "A point on the line, from the canvas centre."),
    ]},
    TypeSpec { name: "kaleidoscope", label: "Kaleidoscope", doc: "Mirrors a wedge around a point.", params: &[
        int("segments", "Segments", 2.0, 64.0, 6.0, ""),
        num("angle", "Angle", -INF, INF, 1.0, 0.0, "Degrees."),
        v2("center", "Centre", [0.0, 0.0], ""),
    ]},
    TypeSpec { name: "motionTile", label: "Motion tile", doc: "Repeats the picture beyond its edges (for moving backgrounds).", params: &[
        num("width", "Tile width", 1.0, 10000.0, 1.0, 400.0, "Pixels."),
        num("height", "Tile height", 1.0, 10000.0, 1.0, 400.0, "Pixels."),
        v2("offset", "Offset", [0.0, 0.0], "Pixels; animate it to scroll."),
        flag("mirror", "Mirror edges", false, "Alternate tiles are mirrored."),
    ]},
    TypeSpec { name: "cornerPin", label: "Corner pin", doc: "Moves the layer's four corners (perspective), in pixels from where they are.", params: &[
        v2("topLeft", "Top left", [0.0, 0.0], ""),
        v2("topRight", "Top right", [0.0, 0.0], ""),
        v2("bottomRight", "Bottom right", [0.0, 0.0], ""),
        v2("bottomLeft", "Bottom left", [0.0, 0.0], ""),
    ]},
    TypeSpec { name: "sharpen", label: "Sharpen", doc: "Crisper edges.", params: &[num("amount", "Amount", 0.0, 5.0, 0.01, 0.5, "")] },
];

// ---- 2D shape operators --------------------------------------------------------------------

pub struct OperatorFamily;
impl Family for OperatorFamily {
    const FIELD: &'static str = "operators";
    const NOUN: &'static str = "operator";
    fn types() -> &'static [TypeSpec] {
        OPERATORS
    }
}
/// A 2D shape layer's path operator (see [`OPERATORS`]), applied to its outline in order.
pub type Operator = Stacked<OperatorFamily>;

pub static OPERATORS: &[TypeSpec] = &[
    TypeSpec { name: "repeater", label: "Repeater", doc: "Draws copies of the layer's content, each moved, turned and scaled more than the last (After Effects' Repeater). Works on groups too.", params: &[
        num("copies", "Copies", 0.0, 1000.0, 1.0, 3.0, "How many (fractions fade the last one in)."),
        num("offset", "Offset", -1000.0, 1000.0, 0.1, 0.0, "Shifts which copies are drawn (animate for a crawl)."),
        v2("position", "Position", [100.0, 0.0], "Each copy moves this much more, pixels."),
        num("rotation", "Rotation", -INF, INF, 1.0, 0.0, "Each copy turns this much more, degrees."),
        num("scale", "Scale", 0.0, 100.0, 0.01, 1.0, "Each copy's size relative to the previous one."),
        v2("anchor", "Anchor", [0.0, 0.0], "Point copies turn and scale around, pixels."),
        num("startOpacity", "First opacity", 0.0, 1.0, 0.01, 1.0, ""),
        num("endOpacity", "Last opacity", 0.0, 1.0, 0.01, 1.0, ""),
        choice("composite", "Order", &["above", "below"], "above", "Whether each copy goes above the previous."),
    ]},
    TypeSpec { name: "offset", label: "Offset path", doc: "Grows or shrinks the outline.", params: &[
        num("amount", "Amount", -1000.0, 1000.0, 0.5, 10.0, "Pixels (negative shrinks)."),
        choice("join", "Corners", &["miter", "round", "bevel"], "round", ""),
    ]},
    TypeSpec { name: "zigzag", label: "Zig zag", doc: "Zig zags or waves along the outline.", params: &[
        num("size", "Size", -1000.0, 1000.0, 0.5, 10.0, "Pixels."),
        num("ridges", "Ridges", 0.0, 1000.0, 1.0, 10.0, "Per segment (or in total with perSegment off)."),
        choice("points", "Points", &["corner", "smooth"], "corner", "Sharp zig zags or smooth waves."),
        flag("perSegment", "Per segment", true, "Ridges count per segment of the outline."),
    ]},
    TypeSpec { name: "wiggle", label: "Wiggle path", doc: "Jitters the outline (hand-drawn, boiling lines).", params: &[
        num("size", "Size", 0.0, 1000.0, 0.5, 8.0, "Pixels."),
        num("detail", "Detail", 0.0, 1000.0, 1.0, 10.0, "Points added per 100 pixels."),
        num("speed", "Wiggles/second", 0.0, 60.0, 0.1, 2.0, ""),
        int("seed", "Seed", 0.0, 1e6, 0.0, ""),
    ]},
    TypeSpec { name: "roundCorners", label: "Round corners", doc: "Rounds every corner of the outline.", params: &[num("radius", "Radius", 0.0, 10000.0, 0.5, 20.0, "Pixels.")] },
    TypeSpec { name: "twist", label: "Twist", doc: "Twists the outline around its centre (more in the middle).", params: &[
        num("angle", "Angle", -INF, INF, 1.0, 90.0, "Degrees."),
        v2("center", "Centre", [0.0, 0.0], "Pixels."),
    ]},
    TypeSpec { name: "puckerBloat", label: "Pucker & bloat", doc: "Pulls corners in and bulges edges out, or the reverse.", params: &[num("amount", "Amount", -100.0, 100.0, 1.0, 30.0, "Percent: negative puckers, positive bloats.")] },
];

// ---- 2D masks ------------------------------------------------------------------------------

pub struct MaskFamily;
impl Family for MaskFamily {
    const FIELD: &'static str = "masks";
    const NOUN: &'static str = "mask";
    const DEFAULT_TYPE: Option<&'static str> = Some("path");
    fn types() -> &'static [TypeSpec] {
        MASKS
    }
}
/// A mask on a 2D layer (see [`MASKS`]): only what is inside shows (combined in order).
pub type Mask = Stacked<MaskFamily>;

const MASK_COMMON: [ParamSpec; 5] = [
    choice("mode", "Mode", &["add", "subtract", "intersect", "difference", "none"], "add", "How it combines with the masks before it."),
    num("feather", "Feather", 0.0, 1000.0, 0.5, 0.0, "Soft edge, pixels."),
    num("expansion", "Expansion", -1000.0, 1000.0, 0.5, 0.0, "Grows (or shrinks) the mask, pixels."),
    num("opacity", "Opacity", 0.0, 1.0, 0.01, 1.0, ""),
    flag("inverted", "Inverted", false, "Show what is outside instead."),
];

pub static MASKS: &[TypeSpec] = &[
    TypeSpec { name: "path", label: "Path mask", doc: "Any shape as SVG path data in the layer's own pixels (from its centre); animate d to morph it.", params: &[
        path("d", "Path", "SVG path data, e.g. \"M-100 -50 L100 -50 L100 50 L-100 50 Z\"."),
        MASK_COMMON[0], MASK_COMMON[1], MASK_COMMON[2], MASK_COMMON[3], MASK_COMMON[4],
    ]},
    TypeSpec { name: "rect", label: "Rectangle mask", doc: "A rectangle in the layer's own pixels.", params: &[
        v2("center", "Centre", [0.0, 0.0], "From the layer's centre, pixels."),
        v2("size", "Size", [200.0, 200.0], "Pixels."),
        num("radius", "Corners", 0.0, 10000.0, 0.5, 0.0, "Corner radius."),
        MASK_COMMON[0], MASK_COMMON[1], MASK_COMMON[2], MASK_COMMON[3], MASK_COMMON[4],
    ]},
    TypeSpec { name: "ellipse", label: "Ellipse mask", doc: "An ellipse in the layer's own pixels.", params: &[
        v2("center", "Centre", [0.0, 0.0], "From the layer's centre, pixels."),
        v2("size", "Size", [200.0, 200.0], "Pixels."),
        MASK_COMMON[0], MASK_COMMON[1], MASK_COMMON[2], MASK_COMMON[3], MASK_COMMON[4],
    ]},
];

// ---- text animators ------------------------------------------------------------------------

pub struct AnimatorFamily;
impl Family for AnimatorFamily {
    const FIELD: &'static str = "animators";
    const NOUN: &'static str = "text animator";
    const DEFAULT_TYPE: Option<&'static str> = Some("range");
    fn types() -> &'static [TypeSpec] {
        ANIMATORS
    }
}
/// A text layer's animator (see [`ANIMATORS`]): moves, turns, scales, fades or recolours the
/// letters, words or lines a selector picks, like After Effects' text animators.
pub type Animator = Stacked<AnimatorFamily>;

const ANIMATED_PROPS: [ParamSpec; 10] = [
    num("x", "Move x", -INF, INF, 1.0, 0.0, "Pixels each selected unit moves."),
    num("y", "Move y", -INF, INF, 1.0, 0.0, "Pixels (down)."),
    num("scale", "Scale", 0.0, 100.0, 0.01, 1.0, "Each unit's size."),
    num("rotation", "Rotation", -INF, INF, 1.0, 0.0, "Degrees."),
    num("opacity", "Opacity", 0.0, 1.0, 0.01, 1.0, "Each unit's opacity."),
    color("fill", "Colour", "#00000000", "Colour (transparent = unchanged)."),
    num("blur", "Blur", 0.0, 500.0, 0.5, 0.0, "Pixels."),
    num("tracking", "Tracking", -INF, INF, 0.5, 0.0, "Extra space after each unit, pixels."),
    num("skew", "Skew", -85.0, 85.0, 1.0, 0.0, "Degrees."),
    num("amount", "Amount", -2.0, 2.0, 0.01, 1.0, "How much of all the above applies (0 = none)."),
];

pub static ANIMATORS: &[TypeSpec] = &[
    TypeSpec { name: "range", label: "Range", doc: "The units between start and end (percent of the text, moved by offset) get the properties; animate offset or start/end to sweep across.", params: &[
        choice("by", "Based on", &["char", "word", "line"], "char", "What counts as a unit."),
        num("start", "Start", 0.0, 100.0, 0.5, 0.0, "Percent."),
        num("end", "End", 0.0, 100.0, 0.5, 100.0, "Percent."),
        num("offset", "Offset", -200.0, 200.0, 0.5, 0.0, "Percent, added to start and end."),
        choice("shape", "Shape", &["square", "rampUp", "rampDown", "triangle", "round", "smooth"], "square", "How strongly units inside the range are affected."),
        num("smoothness", "Smoothness", 0.0, 100.0, 1.0, 100.0, "Percent (square shape): soft edges."),
        flag("randomize", "Random order", false, "Units are picked in a shuffled order."),
        int("seed", "Seed", 0.0, 1e6, 0.0, ""),
        ANIMATED_PROPS[0], ANIMATED_PROPS[1], ANIMATED_PROPS[2], ANIMATED_PROPS[3], ANIMATED_PROPS[4],
        ANIMATED_PROPS[5], ANIMATED_PROPS[6], ANIMATED_PROPS[7], ANIMATED_PROPS[8], ANIMATED_PROPS[9],
    ]},
    TypeSpec { name: "wiggly", label: "Wiggly", doc: "Each unit gets a random amount of the properties that changes over time (jittery, nervous type).", params: &[
        choice("by", "Based on", &["char", "word", "line"], "char", ""),
        num("speed", "Wiggles/second", 0.0, 60.0, 0.1, 2.0, ""),
        num("correlation", "Correlation", 0.0, 1.0, 0.01, 0.5, "1 = units move together."),
        int("seed", "Seed", 0.0, 1e6, 0.0, ""),
        ANIMATED_PROPS[0], ANIMATED_PROPS[1], ANIMATED_PROPS[2], ANIMATED_PROPS[3], ANIMATED_PROPS[4],
        ANIMATED_PROPS[5], ANIMATED_PROPS[6], ANIMATED_PROPS[7], ANIMATED_PROPS[8], ANIMATED_PROPS[9],
    ]},
];

// ---- 3D material patterns ------------------------------------------------------------------

pub struct PatternFamily;
impl Family for PatternFamily {
    const FIELD: &'static str = "pattern";
    const NOUN: &'static str = "pattern";
    fn types() -> &'static [TypeSpec] {
        PATTERNS
    }
}
/// A procedural surface pattern of a 3D material (see [`PATTERNS`]): two colours laid out by a
/// pattern over the surface, optionally bumpy. Keyframes reach its parameters as
/// `pattern.<param>` (one per material, so no id).
pub type Pattern = Stacked<PatternFamily>;

const PATTERN_COMMON: [ParamSpec; 5] = [
    color("color", "Colour", "#d9d9d9", "First colour."),
    color("color2", "Colour 2", "#3a3a3a", "Second colour."),
    num("scale", "Scale", 0.001, 10000.0, 0.1, 4.0, "Repeats per unit of texture space (per side of a box, around a sphere…)."),
    num("bump", "Bump", -10.0, 10.0, 0.01, 0.0, "How much the pattern dents the surface (0 = flat)."),
    int("seed", "Seed", 0.0, 1e6, 0.0, "Another seed, another variation."),
];

pub static PATTERNS: &[TypeSpec] = &[
    TypeSpec { name: "checker", label: "Checker", doc: "Squares of the two colours.", params: &[PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3]] },
    TypeSpec { name: "stripes", label: "Stripes", doc: "Bands of the two colours.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3],
        num("width", "Width", 0.0, 1.0, 0.01, 0.5, "Share of each band in the first colour."),
        num("angle", "Angle", -INF, INF, 1.0, 0.0, "Degrees."),
    ]},
    TypeSpec { name: "dots", label: "Dots", doc: "Polka dots.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3],
        num("size", "Size", 0.0, 1.0, 0.01, 0.35, "Dot radius as a share of the spacing."),
    ]},
    TypeSpec { name: "noise", label: "Noise", doc: "Cloudy noise between the two colours.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3], PATTERN_COMMON[4],
        int("detail", "Detail", 1.0, 8.0, 4.0, "Layers of finer noise."),
        num("contrast", "Contrast", 0.0, 10.0, 0.05, 1.0, ""),
    ]},
    TypeSpec { name: "marble", label: "Marble", doc: "Veined stone.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3], PATTERN_COMMON[4],
        num("turbulence", "Turbulence", 0.0, 20.0, 0.1, 4.0, "How wavy the veins are."),
    ]},
    TypeSpec { name: "wood", label: "Wood", doc: "Wood grain rings.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3], PATTERN_COMMON[4],
        num("rings", "Rings", 0.0, 200.0, 0.5, 12.0, "Rings across the texture."),
        num("turbulence", "Turbulence", 0.0, 20.0, 0.1, 1.0, ""),
    ]},
    TypeSpec { name: "voronoi", label: "Cells", doc: "Cells like scales, stone or foam.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3], PATTERN_COMMON[4],
        num("edge", "Edge width", 0.0, 1.0, 0.01, 0.1, "Lines between cells in the second colour (0 = shaded cells)."),
    ]},
    TypeSpec { name: "bricks", label: "Bricks", doc: "Rows of bricks with mortar.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1], PATTERN_COMMON[2], PATTERN_COMMON[3],
        num("mortar", "Mortar", 0.0, 0.5, 0.005, 0.05, "Mortar width as a share of a brick."),
        num("ratio", "Brick ratio", 0.1, 10.0, 0.05, 2.0, "Width over height."),
    ]},
    TypeSpec { name: "gradient", label: "Gradient", doc: "From the first colour to the second across the surface.", params: &[
        PATTERN_COMMON[0], PATTERN_COMMON[1],
        choice("direction", "Direction", &["u", "v", "radial"], "v", "Across, along, or out from the middle of the texture."),
    ]},
];

/// The families, for listings: (field, noun, types).
pub fn families() -> [(&'static str, &'static str, &'static [TypeSpec]); 7] {
    [
        (ModifierFamily::FIELD, ModifierFamily::NOUN, MODIFIERS),
        (ConstraintFamily::FIELD, ConstraintFamily::NOUN, CONSTRAINTS),
        (EffectFamily::FIELD, EffectFamily::NOUN, EFFECTS),
        (OperatorFamily::FIELD, OperatorFamily::NOUN, OPERATORS),
        (MaskFamily::FIELD, MaskFamily::NOUN, MASKS),
        (AnimatorFamily::FIELD, AnimatorFamily::NOUN, ANIMATORS),
        (PatternFamily::FIELD, PatternFamily::NOUN, PATTERNS),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn items_read_check_and_animate() {
        let mut fx: Vec<Effect> = serde_json::from_value(json!([{"type": "blur", "radius": 4}, {"type": "blur"}, {"id": "g", "type": "glow"}])).unwrap();
        name_items(&mut fx);
        assert_eq!(fx.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["blur", "blur2", "g"]);
        assert_eq!(fx[0].n("radius"), 4.0);
        assert_eq!(fx[1].n("radius"), 10.0, "default");
        assert!(set_in(&mut fx, "effects.blur2.radius", &KeyValue::Number(30.0)).unwrap());
        assert_eq!(get_in(&fx, "effects.blur2.radius"), Some(KeyValue::Number(30.0)));
        assert!(!set_in(&mut fx, "x", &KeyValue::Number(1.0)).unwrap());
        assert!(set_in(&mut fx, "effects.nope.radius", &KeyValue::Number(1.0)).unwrap_err().contains("ids: blur, blur2, g"));
        assert!(fx[0].set("radios", &KeyValue::Number(1.0)).unwrap_err().contains("Did you mean `radius`"));
        assert!(fx[0].set("dimensions", &KeyValue::from("diagonal")).is_err());
        let back: Vec<Effect> = serde_json::from_value(serde_json::to_value(&fx).unwrap()).unwrap();
        assert_eq!(back, fx);
        let mut refs = vec![];
        assert!(check_items(&fx, "layer \"a\"", &mut refs).is_ok());
        let bad: Vec<Effect> = serde_json::from_value(json!([{"type": "blurr"}])).unwrap();
        assert!(check_items(&bad, "a", &mut refs).unwrap_err().contains("Did you mean `blur`"));
        let m: Vec<Modifier> = serde_json::from_value(json!([{"type": "boolean", "object": "cutter"}])).unwrap();
        check_items(&m, "o", &mut refs).unwrap();
        assert_eq!(refs, ["cutter"]);
        let masks: Vec<Mask> = serde_json::from_value(json!([{"d": "M0 0 L10 0 L10 10 Z", "feather": 4}])).unwrap();
        assert_eq!(masks[0].kind, "path");
    }

    #[test]
    fn every_default_fits_its_kind() {
        for (_, noun, types) in families() {
            for t in types {
                for p in t.params {
                    let mut it: Stacked<EffectFamily> = Stacked::new("x", Map::new());
                    let _ = &mut it;
                    match (p.kind, p.default) {
                        (ParamKind::Number { min, max, .. } | ParamKind::Int { min, max }, Def::N(n)) => {
                            assert!(n >= min && n <= max, "{noun} {} {}: default {n} outside {min}..{max}", t.name, p.name)
                        }
                        (ParamKind::Bool, Def::B(_)) | (ParamKind::Vec2, Def::V2(_)) | (ParamKind::Vec3, Def::V3(_)) => {}
                        (ParamKind::Color, Def::S(c)) => assert!(Rgba::parse(c).is_some(), "{noun} {} {}", t.name, p.name),
                        (ParamKind::Choice(o), Def::S(c)) => assert!(o.contains(&c), "{noun} {} {}", t.name, p.name),
                        (ParamKind::Text, Def::S(_)) | (ParamKind::Ref | ParamKind::Path, Def::None) => {}
                        (k, d) => panic!("{noun} {} {}: {k:?} with default {d:?}", t.name, p.name),
                    }
                }
            }
        }
    }
}
