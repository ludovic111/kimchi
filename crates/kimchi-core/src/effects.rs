//! A clip's colour and picture effects: corrections (brightness, contrast, saturation,
//! temperature, tint), vignette, sharpen, chroma key and a 3D LUT. They are parameters, drawn by
//! the compositor in the preview and the export; the numeric ones take keyframes like any clip
//! property ([`crate::model::CLIP_PROPS`]).

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

/// A 3D colour lookup table (`.cube` file).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lut {
    /// Absolute path of the `.cube` file.
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
        !self.is_default()
    }
}

/// A ready-made look: effect values that can be adjusted afterwards.
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

/// Effects with `look` applied: its corrections, the others at 0; the chroma key and LUT stay.
pub fn apply_look(effects: &Effects, look: &str) -> Result<Effects, String> {
    let Some(l) = LOOKS.iter().find(|l| l.id.eq_ignore_ascii_case(look)) else {
        let ids: Vec<&str> = LOOKS.iter().map(|l| l.id).collect();
        let hint = crate::closest(look, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        return Err(format!("Unknown look `{look}`.{hint} Looks: {}.", ids.join(", ")));
    };
    let mut out = Effects { chroma_key: effects.chroma_key.clone(), lut: effects.lut.clone(), ..Effects::default() };
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
    fn fields_are_clamped() {
        let mut e = Effects::default();
        assert!(e.set("contrast", 3.0));
        assert!(e.set("vignette", -1.0));
        assert!(!e.set("hue", 1.0));
        assert_eq!((e.contrast, e.vignette), (1.0, 0.0));
    }
}
