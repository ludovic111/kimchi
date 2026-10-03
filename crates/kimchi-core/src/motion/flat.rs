//! 2D motion graphics, like an After Effects composition: layers of shapes, paths, text,
//! pictures, particles, nulls, adjustment layers and nested compositions; each layer with
//! parenting, masks, a track matte, an effect stack, shape operators (repeater, zig zag…), text
//! animators, motion blur and expressions.

use super::*;
use crate::motion::particles::ParticleSystem;
use crate::motion::stack::{self, Animator, Effect, Mask, Operator};

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
    /// Compositions used by `comp` layers (precomps): their own layers and time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compositions: Vec<Composition>,
    /// Motion blur for layers with `motionBlur`: the share of a frame the shutter is open
    /// (0.5 = a 180° shutter; 0 = off).
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub shutter: f64,
    /// Moments blended per frame for motion blur.
    #[serde(default = "eight", skip_serializing_if = "is_eight")]
    pub motion_blur_samples: f64,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

pub(super) const SCENE2D_KEYS: &[&str] = &["background", "layers", "compositions", "shutter", "motionBlurSamples", "keyframes", "animate"];

/// A composition `comp` layers show (After Effects' precomp): layers with their own time
/// (0 = the composition's start) and canvas.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Composition {
    pub id: String,
    /// Canvas size in pixels (default: the project's). Layers are placed from its centre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Seconds; `loop` layers repeat after it (default: the last keyframe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

pub(super) const COMPOSITION_KEYS: &[&str] = &["id", "width", "height", "duration", "background", "layers"];

impl Composition {
    /// How long it lasts: its duration, or until its last keyframe or layer end (at least 1 s).
    pub fn length(&self) -> f64 {
        if let Some(d) = self.duration {
            return d.max(0.01);
        }
        let mut t = 1.0f64;
        walk_layers(&self.layers, &mut |l| {
            for list in l.keyframes.values() {
                if let Some(k) = list.last() {
                    t = t.max(k.time);
                }
            }
            if let Some(e) = l.end {
                t = t.max(e);
            }
        });
        t
    }
}

/// How a track matte uses its layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Matte {
    /// The layer whose picture decides what shows (it isn't drawn itself).
    pub layer: String,
    /// `"alpha"` (where it is opaque), `"alphaInverted"`, `"luma"` (where it is bright) or `"lumaInverted"`.
    #[serde(default = "alpha_mode")]
    pub mode: String,
}

pub const MATTE_MODES: &[&str] = &["alpha", "alphaInverted", "luma", "lumaInverted"];

fn alpha_mode() -> String {
    "alpha".into()
}

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
    /// A track matte: another layer decides where this one shows (alpha or luma, inverted or not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matte: Option<Matte>,
    /// Masks drawn on the layer (paths, rectangles, ellipses), combined in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<Mask>,
    /// Effects applied to the layer's picture in order (blur, glow, colour, distortions…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// Shape operators applied to its outline in order (repeater, zig zag, offset…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operators: Vec<Operator>,
    /// Another layer whose position, rotation and scale this one follows (After Effects'
    /// parenting); its own values are relative to the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Blur the layer along its motion (see the scene's `shutter`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub motion_blur: bool,
    /// Properties computed from a formula every frame (`{"rotation": "time * 90"}`).
    #[serde(default, skip_serializing_if = "Expressions::is_empty")]
    pub expressions: Expressions,
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

pub(super) const LAYER_KEYS: &[&str] = &[
    "id", "type", "x", "y", "anchorX", "anchorY", "scale", "scaleX", "scaleY", "rotation", "skewX", "opacity", "fill",
    "stroke", "trimStart", "trimEnd", "trimOffset", "blur", "shadow", "glow", "blend", "mask", "maskInvert", "matte", "masks",
    "effects", "operators", "parent", "motionBlur", "expressions", "start", "end", "hidden", "keyframes", "animate",
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
    /// Draws nothing: a handle other layers follow (`parent`).
    Null {},
    /// Applies its effects to everything below it (in its group or composition), within its masks.
    Adjustment {},
    /// Shows a composition (`compositions`), with its own time: comp time = (scene time − start)
    /// × speed + offset, or `time` when that is set (animate it to remap time).
    #[serde(rename_all = "camelCase")]
    Comp {
        comp: String,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        speed: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        offset: f64,
        /// Start again after the composition's length.
        #[serde(default, skip_serializing_if = "is_false", rename = "loop")]
        looped: bool,
        /// Time remap: the composition time shown (animate it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time: Option<f64>,
    },
    /// Particles born from the layer's position (sparks, snow, confetti).
    Particles(ParticleSystem),
}

pub(super) const KIND_KEYS: &[(&str, &[&str])] = &[
    ("rect", &["width", "height", "radius"]),
    ("ellipse", &["width", "height"]),
    ("polygon", &["sides", "radius", "roundness"]),
    ("star", &["points", "radius", "innerRadius"]),
    ("path", &["d", "points", "closed"]),
    ("text", &["text", "fontFamily", "fontSize", "fontWeight", "italic", "align", "lineHeight", "letterSpacing", "value", "decimals", "reveal", "animators", "path", "pathOffset"]),
    ("image", &["asset", "width", "height", "radius"]),
    ("group", &["layers"]),
    ("null", &[]),
    ("adjustment", &[]),
    ("comp", &["comp", "speed", "offset", "loop", "time"]),
    ("particles", crate::motion::particles::PARTICLE_KEYS),
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
    /// Text animators: move, turn, scale, fade or recolour the letters a range picks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animators: Vec<Animator>,
    /// Text on a path: SVG path data (layer pixels) the baseline follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Where along the path the text starts (0–1 of its length; animate to slide it along).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub path_offset: f64,
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

pub(super) fn group_thousands(n: &str) -> String {
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
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    pub(super) fn is_normal(&self) -> bool {
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
            _ => {
                if stack::set_in(&mut self.effects, name, v)? || stack::set_in(&mut self.masks, name, v)? || stack::set_in(&mut self.operators, name, v)? {
                    return Ok(());
                }
                return self.set_kind(name, v, n, s);
            }
        }
        Ok(())
    }

    pub(super) fn set_kind(
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
            (LayerKind::Text(t), "pathOffset") => t.path_offset = n()?,
            (LayerKind::Text(t), _) if name.starts_with("animators.") => {
                stack::set_in(&mut t.animators, name, v)?;
            }
            (LayerKind::Comp { time, .. }, "time") => *time = Some(n()?),
            (LayerKind::Comp { speed, .. }, "speed") => *speed = n()?,
            (LayerKind::Comp { offset, .. }, "offset") => *offset = n()?,
            (LayerKind::Particles(p), _) if crate::motion::particles::PARTICLE_KEYS.contains(&name) => {
                if !p.set(name, v)? {
                    return Err(format!("particles can't animate `{name}`; they can animate {}", crate::motion::particles::PARTICLE_PROPS.join(", ")));
                }
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

pub(super) const COMMON_PROPS: &[&str] = &[
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
            LayerKind::Null {} => "null",
            LayerKind::Adjustment {} => "adjustment",
            LayerKind::Comp { .. } => "comp",
            LayerKind::Particles(_) => "particles",
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
            LayerKind::Text(_) => &["text", "fontSize", "fontWeight", "letterSpacing", "lineHeight", "value", "reveal", "pathOffset", "animators.<id>.<param>"],
            LayerKind::Image { .. } => &["width", "height", "radius"],
            LayerKind::Group { .. } | LayerKind::Null {} | LayerKind::Adjustment {} => &[],
            LayerKind::Comp { .. } => &["time", "speed", "offset"],
            LayerKind::Particles(_) => crate::motion::particles::PARTICLE_PROPS,
        }
    }
}


impl Layer {
    /// The still value of a property (without keyframes), by its keyframe name.
    pub fn get(&self, name: &str) -> Option<KeyValue> {
        let l = self;
        let n = |v: f64| Some(KeyValue::Number(v));
        let c = |v: &str| Some(KeyValue::Text(v.to_string()));
        match name {
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
            (LayerKind::Text(t), "pathOffset") => n(t.path_offset),
            (LayerKind::Text(t), _) if name.starts_with("animators.") => stack::get_in(&t.animators, name),
            (LayerKind::Comp { time, .. }, "time") => time.and_then(n),
            (LayerKind::Comp { speed, .. }, "speed") => n(*speed),
            (LayerKind::Comp { offset, .. }, "offset") => n(*offset),
            (LayerKind::Particles(p), _) => p.get(name),
            _ => stack::get_in(&l.effects, name).or_else(|| stack::get_in(&l.masks, name)).or_else(|| stack::get_in(&l.operators, name)),
        },
        }
    }

    /// Every animatable property name it has now (for the window's keyframe lists).
    pub fn prop_names(&self) -> Vec<String> {
        let mut out: Vec<String> = COMMON_PROPS.iter().map(|p| p.to_string()).collect();
        out.extend(self.kind.props().iter().filter(|p| !p.contains('<')).map(|p| p.to_string()));
        if let LayerKind::Text(t) = &self.kind {
            out.extend(stack::animatable_names(&t.animators));
        }
        out.extend(stack::animatable_names(&self.effects));
        out.extend(stack::animatable_names(&self.masks));
        out.extend(stack::animatable_names(&self.operators));
        out
    }

    /// Gives effects, masks, operators and animators ids.
    pub(super) fn name_items(&mut self) {
        stack::name_items(&mut self.effects);
        stack::name_items(&mut self.masks);
        stack::name_items(&mut self.operators);
        if let LayerKind::Text(t) = &mut self.kind {
            stack::name_items(&mut t.animators);
        }
    }
}

pub(super) fn is_half(v: &f64) -> bool {
    *v == 0.5
}
