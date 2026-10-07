//! A clip's colour and picture effects: corrections (brightness, contrast, saturation,
//! temperature, tint), vignette, sharpen, chroma key, a LUT and video plugins. They are
//! parameters, drawn by the compositor in the preview and the export; the numeric ones take
//! keyframes like any clip property ([`crate::model::CLIP_PROPS`]), plugin parameters as
//! `plugins.<slot id>.<parameter>`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Every field at its default leaves the picture unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Effects {
    /// −1…1: darker / brighter (0 = unchanged).
    pub brightness: f64,
    /// −1…1: flat grey at −1, twice the contrast at 1.
    pub contrast: f64,
    /// −1…1: black and white at −1, twice as saturated at 1.
    pub saturation: f64,
    /// −1…1: cooler (blue) / warmer (orange).
    pub temperature: f64,
    /// −1…1: greener / more magenta.
    pub tint: f64,
    /// 0…1: darker corners.
    pub vignette: f64,
    /// 0…1: crisper edges (unsharp mask).
    pub sharpen: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chroma_key: Option<ChromaKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lut: Option<Lut>,
    /// Video plugins run on the picture after the effects above, first to last.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<PluginEffect>,
    /// The highest plugin slot number given on this clip (`p3` → 3), so a removed slot's id is
    /// never given to another plugin ([`Effects::next_plugin_id`]).
    #[serde(skip_serializing_if = "is_zero")]
    pub last_plugin: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One video plugin on a clip: kimchi's own (`kimchi:<id>`, an lsuite plugin built with the
/// `kimchi-plugin` SDK) or frei0r (`frei0r:<name>`). Hosted by `kimchi_media::render::plugins`;
/// listed by `plugin.list`. Other ids (`ofx:…`, from a file made elsewhere) are kept as they are
/// and not drawn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginEffect {
    /// Slot id, unique on the clip (`p1`, `p2`…): keyframes name it, so it never changes.
    pub id: String,
    /// The plugin's id with its format prefix.
    pub plugin: String,
    /// What the inspector shows (the plugin's name when added).
    #[serde(default)]
    pub name: String,
    /// Skipped while drawing; its settings are kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
    /// Parameter values by the plugin's parameter name; the ones not set use its defaults.
    /// Numbers take keyframes as `plugins.<id>.<name>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, PluginValue>,
}

/// A plugin parameter's value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PluginValue {
    Bool(bool),
    Number(f64),
    /// Points, sizes and colours as `[x, y]`, `[r, g, b, a]` (0…1).
    Vector(Vec<f64>),
    /// Text, a choice by its label, a file path, or a colour as `#rrggbb[aa]`.
    Text(String),
}

/// Makes one colour (a green or blue screen) transparent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChromaKey {
    /// The screen's colour, `#rrggbb`.
    pub color: String,
    /// 0…1: how far from the colour still counts as screen (0 = only as strong a colour).
    pub similarity: f64,
    /// 0…1: width of the soft edge after that.
    pub softness: f64,
    /// 0…1: how much of the screen's colour is taken out of what is kept (green fringes).
    pub spill: f64,
}

impl Default for ChromaKey {
    fn default() -> Self {
        Self { color: "#00b140".into(), similarity: 0.5, softness: 0.1, spill: 0.5 }
    }
}

/// A colour lookup table: `.cube` (1D or 3D), `.3dl`, `.csp`, `.spi1d`, `.spi3d` or a Hald CLUT
/// image (`kimchi_media::render::grade::lut`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lut {
    /// Absolute path of the LUT file.
    pub path: String,
    /// 0…1: mix between the original and the graded colour.
    #[serde(default = "one")]
    pub strength: f64,
}

fn one() -> f64 {
    1.0
}

/// Effect fields that take keyframes, as named in commands (camelCase).
pub const EFFECT_PROPS: &[&str] = &["brightness", "contrast", "saturation", "temperature", "tint", "vignette", "sharpen"];

impl Effects {
    pub fn is_default(&self) -> bool {
        *self == Effects::default()
    }

    /// The value of a numeric effect field by name.
    pub fn get(&self, name: &str) -> Option<f64> {
        Some(match name {
            "brightness" => self.brightness,
            "contrast" => self.contrast,
            "saturation" => self.saturation,
            "temperature" => self.temperature,
            "tint" => self.tint,
            "vignette" => self.vignette,
            "sharpen" => self.sharpen,
            _ => return None,
        })
    }

    /// Sets a numeric effect field by name, clamped to its range. False for unknown names.
    pub fn set(&mut self, name: &str, v: f64) -> bool {
        let signed = v.clamp(-1.0, 1.0);
        let unsigned = v.clamp(0.0, 1.0);
        match name {
            "brightness" => self.brightness = signed,
            "contrast" => self.contrast = signed,
            "saturation" => self.saturation = signed,
            "temperature" => self.temperature = signed,
            "tint" => self.tint = signed,
            "vignette" => self.vignette = unsigned,
            "sharpen" => self.sharpen = unsigned,
            _ => return false,
        }
        true
    }

    /// Clamps every field to its range.
    pub fn clamped(mut self) -> Self {
        for name in EFFECT_PROPS {
            let v = self.get(name).unwrap_or(0.0);
            self.set(name, if v.is_finite() { v } else { 0.0 });
        }
        if let Some(k) = &mut self.chroma_key {
            k.similarity = k.similarity.clamp(0.0, 1.0);
            k.softness = k.softness.clamp(0.0, 1.0);
            k.spill = k.spill.clamp(0.0, 1.0);
        }
        if let Some(l) = &mut self.lut {
            l.strength = l.strength.clamp(0.0, 1.0);
        }
        self
    }

    /// Does anything change the colours or the pixels (not counting keyframes)?
    pub fn is_active(&self) -> bool {
        !self.is_default() && (self.last_plugin == 0 || !Effects { last_plugin: 0, ..self.clone() }.is_default())
    }

    /// Do the corrections, vignette, sharpen, chroma key or LUT change the picture? (Plugins are
    /// run on their own, after these.)
    pub fn grades(&self) -> bool {
        EFFECT_PROPS.iter().any(|n| self.get(n).is_some_and(|v| v != 0.0)) || self.chroma_key.is_some() || self.lut.is_some()
    }

    /// The plugins that are drawn (not bypassed), first to last.
    pub fn active_plugins(&self) -> impl Iterator<Item = &PluginEffect> {
        self.plugins.iter().filter(|p| !p.bypass)
    }

    /// A slot by id (`p2`), by position from 1 (`2`, `"2"`), or by its name or plugin name
    /// (case-insensitive), with "did you mean" when nothing matches.
    pub fn plugin_index(&self, slot: &str) -> Result<usize, String> {
        let s = slot.trim();
        if let Some(i) = self.plugins.iter().position(|p| p.id == s) {
            return Ok(i);
        }
        if let Ok(n) = s.parse::<usize>() {
            return match n {
                1.. if n <= self.plugins.len() => Ok(n - 1),
                _ => Err(format!("There is no plugin {n} on this clip: it has {} (positions start at 1).", self.plugins.len())),
            };
        }
        let named: Vec<usize> = (0..self.plugins.len()).filter(|&i| self.plugins[i].name.eq_ignore_ascii_case(s)).collect();
        let by_plugin: Vec<usize> = (0..self.plugins.len()).filter(|&i| {
            let p = &self.plugins[i].plugin;
            p.eq_ignore_ascii_case(s) || p.split_once(':').is_some_and(|(_, id)| id.eq_ignore_ascii_case(s))
        }).collect();
        match (named.as_slice(), by_plugin.as_slice()) {
            ([i], _) | ([], [i]) => return Ok(*i),
            ([_, _, ..], _) | ([], [_, _, ..]) => {
                let ids: Vec<&str> = named.iter().chain(&by_plugin).map(|&i| self.plugins[i].id.as_str()).collect();
                return Err(format!("More than one plugin on this clip is called `{s}`: name it by slot id ({}).", ids.join(", ")));
            }
            _ => {}
        }
        if self.plugins.is_empty() {
            return Err(format!("This clip has no plugins (looked for `{s}`). Add one with clip.addPlugin."));
        }
        let mut names: Vec<&str> = self.plugins.iter().map(|p| p.id.as_str()).collect();
        names.extend(self.plugins.iter().map(|p| p.name.as_str()));
        let hint = crate::closest(s, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        let list: Vec<String> = self.plugins.iter().map(|p| format!("{} ({})", p.id, p.name)).collect();
        Err(format!("No plugin `{s}` on this clip.{hint} Its plugins: {}.", list.join(", ")))
    }

    /// The id the next plugin added to this clip gets (`p1`, `p2`…; never one given before).
    pub fn next_plugin_id(&self) -> (String, u32) {
        let used = self.plugins.iter().filter_map(|p| p.id.strip_prefix('p')?.parse::<u32>().ok()).max().unwrap_or(0);
        let n = used.max(self.last_plugin) + 1;
        (format!("p{n}"), n)
    }
}

/// The parts of a plugin keyframe name `plugins.<slot>.<parameter>` (parameter names may have
/// dots of their own: OpenFX splits 2D values into `size.x`, `size.y`).
pub fn plugin_key(name: &str) -> Option<(&str, &str)> {
    let mut parts = name.splitn(3, '.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("plugins"), Some(slot), Some(param)) if !slot.is_empty() && !param.is_empty() => Some((slot, param)),
        _ => None,
    }
}

impl PluginValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PluginValue::Number(n) => Some(*n),
            PluginValue::Bool(b) => Some(*b as u8 as f64),
            PluginValue::Vector(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        }
    }

    /// As a keyframe value (numbers, vectors and text; true/false as 1/0).
    pub fn to_key(&self) -> crate::KeyValue {
        match self {
            PluginValue::Bool(b) => crate::KeyValue::Number(*b as u8 as f64),
            PluginValue::Number(n) => crate::KeyValue::Number(*n),
            PluginValue::Vector(v) => crate::KeyValue::Vector(v.clone()),
            PluginValue::Text(t) => crate::KeyValue::Text(t.clone()),
        }
    }

    pub fn from_key(k: &crate::KeyValue) -> Self {
        match k {
            crate::KeyValue::Number(n) => PluginValue::Number(*n),
            crate::KeyValue::Vector(v) => PluginValue::Vector(v.clone()),
            crate::KeyValue::Text(t) => PluginValue::Text(t.clone()),
        }
    }
}

/// A ready-made look: effect values that can be adjusted afterwards.
#[derive(Debug)]
pub struct Look {
    pub id: &'static str,
    pub label: &'static str,
    pub doc: &'static str,
    /// (field, value) pairs; the fields not listed go back to 0.
    pub values: &'static [(&'static str, f64)],
}

pub const LOOKS: &[Look] = &[
    Look { id: "none", label: "None", doc: "Every correction back to neutral (keeps the chroma key and the LUT).", values: &[] },
    Look { id: "punchy", label: "Punchy", doc: "More contrast and colour.", values: &[("contrast", 0.25), ("saturation", 0.3), ("sharpen", 0.2)] },
    Look { id: "warm", label: "Warm", doc: "Golden-hour warmth.", values: &[("temperature", 0.45), ("saturation", 0.1), ("brightness", 0.03)] },
    Look { id: "cool", label: "Cool", doc: "Blue, clean and a little crisp.", values: &[("temperature", -0.45), ("contrast", 0.1)] },
    Look { id: "mono", label: "Mono", doc: "Black and white with some bite.", values: &[("saturation", -1.0), ("contrast", 0.2)] },
    Look { id: "faded", label: "Faded", doc: "Lifted, low-contrast film.", values: &[("contrast", -0.3), ("saturation", -0.25), ("brightness", 0.06)] },
    Look { id: "vintage", label: "Vintage", doc: "Warm, faded and vignetted.", values: &[("temperature", 0.3), ("tint", 0.1), ("contrast", -0.15), ("saturation", -0.35), ("vignette", 0.45)] },
    Look { id: "noir", label: "Noir", doc: "Hard black and white with dark corners.", values: &[("saturation", -1.0), ("contrast", 0.5), ("brightness", -0.05), ("vignette", 0.6)] },
    Look { id: "teal", label: "Teal & orange", doc: "Blockbuster split: cool tint, warm skin.", values: &[("temperature", 0.15), ("tint", -0.2), ("contrast", 0.2), ("saturation", 0.15)] },
    Look { id: "dreamy", label: "Dreamy", doc: "Soft, bright and pastel.", values: &[("brightness", 0.1), ("contrast", -0.2), ("saturation", -0.1), ("tint", 0.08)] },
];

/// Effects with `look` applied: its corrections, the others at 0; the chroma key, LUT and plugins stay.
pub fn apply_look(effects: &Effects, look: &str) -> Result<Effects, String> {
    let Some(l) = LOOKS.iter().find(|l| l.id.eq_ignore_ascii_case(look)) else {
        let ids: Vec<&str> = LOOKS.iter().map(|l| l.id).collect();
        let hint = crate::closest(look, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        return Err(format!("Unknown look `{look}`.{hint} Looks: {}.", ids.join(", ")));
    };
    let mut out = Effects { chroma_key: effects.chroma_key.clone(), lut: effects.lut.clone(), plugins: effects.plugins.clone(), last_plugin: effects.last_plugin, ..Effects::default() };
    for (name, v) in l.values {
        out.set(name, *v);
    }
    Ok(out)
}

/// The look whose values `effects` has exactly, if any.
pub fn look_of(effects: &Effects) -> Option<&'static str> {
    LOOKS
        .iter()
        .find(|l| {
            EFFECT_PROPS.iter().all(|name| {
                let want = l.values.iter().find(|(n, _)| n == name).map_or(0.0, |(_, v)| *v);
                (effects.get(name).unwrap_or(0.0) - want).abs() < 1e-6
            })
        })
        .map(|l| l.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_round_trip() {
        let e = apply_look(&Effects::default(), "vintage").unwrap();
        assert_eq!(look_of(&e), Some("vintage"));
        assert_eq!(look_of(&Effects::default()), Some("none"));
        assert!(apply_look(&e, "vintge").unwrap_err().contains("vintage"));
    }

    #[test]
    fn plugin_slots_by_id_position_or_name() {
        let slot = |id: &str, plugin: &str, name: &str| PluginEffect { id: id.into(), plugin: plugin.into(), name: name.into(), bypass: false, params: Default::default() };
        let mut e = Effects { plugins: vec![slot("p1", "kimchi:xyz.lsuite.kimchi.glow", "Glow"), slot("p3", "frei0r:glow", "Soft glow")], ..Default::default() };
        assert_eq!(e.plugin_index("p3"), Ok(1));
        assert_eq!(e.plugin_index("2"), Ok(1));
        assert_eq!(e.plugin_index("glow"), Ok(0));
        assert_eq!(e.plugin_index("frei0r:glow"), Ok(1));
        assert!(e.plugin_index("3").unwrap_err().contains("has 2"));
        assert!(e.plugin_index("Sotf glow").unwrap_err().contains("Did you mean `Soft glow`"));
        assert_eq!(e.next_plugin_id(), ("p4".into(), 4));
        e.plugins.pop();
        e.last_plugin = 3;
        assert_eq!(e.next_plugin_id().0, "p4", "a removed slot's id isn't given again");
        assert_eq!(plugin_key("plugins.p1.size.x"), Some(("p1", "size.x")));
        assert_eq!(plugin_key("plugins.p1"), None);
        // The counter alone doesn't make the effects do anything.
        assert!(!Effects { last_plugin: 4, ..Default::default() }.is_active());
    }

    #[test]
    fn fields_are_clamped() {
        let mut e = Effects::default();
        assert!(e.set("contrast", 3.0));
        assert!(e.set("vignette", -1.0));
        assert!(!e.set("hue", 1.0));
        assert_eq!((e.contrast, e.vignette), (1.0, 0.0));
    }
}
