//! Lightroom and Camera Raw develop presets: `.xmp` (the `crs:` namespace,
//! `http://ns.adobe.com/camera-raw-settings/1.0/`, as attributes or elements of the
//! `rdf:Description`) and Lightroom Classic's `.lrtemplate` (a Lua table whose
//! `value.settings` has the same names). Settings follow Adobe's Camera Raw process 2012 and
//! later (`Exposure2012`, `Contrast2012`, `Highlights2012`…), as in the XMP Specification Part 2
//! and the presets Lightroom Classic 13 writes.
//!
//! kimchi's corrections are a smaller set: exposure and the four tone sliders become brightness
//! and contrast, vibrance joins saturation, white balance becomes warmth and tint. What has no
//! equivalent (tone curves, HSL, colour grading, clarity, texture, dehaze, grain, profiles) is
//! listed in the report.

use std::collections::BTreeMap;
use std::path::Path;

use kimchi_core::Effects;

use super::{LookFile, lua, xml};
use crate::{Report, Result};

/// A preset's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting {
    Num(f64),
    Text(String),
    List(Vec<f64>),
}

impl Setting {
    fn num(&self) -> Option<f64> {
        match self {
            Setting::Num(n) => Some(*n),
            Setting::Text(t) => t.trim().trim_start_matches('+').parse().ok(),
            Setting::List(_) => None,
        }
    }

    fn text(&self) -> String {
        match self {
            Setting::Num(n) => format!("{n}"),
            Setting::Text(t) => t.clone(),
            Setting::List(v) => format!("{v:?}"),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Setting::Num(n) => *n != 0.0,
            Setting::Text(t) => t.eq_ignore_ascii_case("true"),
            Setting::List(_) => false,
        }
    }
}

pub type Settings = BTreeMap<String, Setting>;

/// A profile (`crs:Look`) the preset uses.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub name: String,
    /// It keeps its colours in a table (`LookTable`, `RGBTable`, `Table_…`).
    pub table: bool,
    pub amount: Option<f64>,
}

/// `+0.35` / `-12` with a sign, two decimals at most.
fn signed(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r > 0.0 { format!("+{r}") } else { format!("{r}") }
}

/// Exposure in stops onto kimchi's brightness (which lifts gamma-encoded values by up to 0.4):
/// a stop brightens mid-grey by about a sixth of the range.
pub(crate) fn exposure_to_brightness(stops: f64) -> f64 {
    stops * 0.4
}

/// Highlights, shadows, whites and blacks (−100…100 each) as (brightness, contrast) changes:
/// raising the bright end brightens and adds contrast, raising the dark end brightens and
/// takes contrast away.
pub(crate) fn tone_sliders(highlights: f64, shadows: f64, whites: f64, blacks: f64) -> (f64, f64) {
    let b = (highlights * 0.12 + shadows * 0.12 + whites * 0.08 + blacks * 0.08) / 100.0;
    let c = (highlights * 0.10 + whites * 0.12 - shadows * 0.10 - blacks * 0.12) / 100.0;
    (b, c)
}

/// White balance in kelvin (raw photos) as warmth relative to daylight (5500 K): an octave of
/// colour temperature is ±0.6.
pub(crate) fn kelvin_to_warmth(k: f64) -> f64 {
    (k.max(1000.0) / 5500.0).log2() * 0.6
}

/// Maps the settings onto kimchi's corrections, saying what changed on the way.
pub fn map(settings: &Settings, profile: Option<&Profile>, report: &mut Report) -> Effects {
    let mut e = Effects::default();
    let get = |k: &str| settings.get(k).and_then(Setting::num);
    // Exposure, contrast and the tone sliders (process 2012; the older names when that's all
    // there is).
    let pick = |new: &str, old: &str| get(new).or_else(|| get(old));
    let mut brightness = 0.0;
    let mut contrast = 0.0;
    if let Some(x) = pick("Exposure2012", "Exposure").filter(|x| *x != 0.0) {
        brightness += exposure_to_brightness(x);
        report.approximated(format!("Exposure {} became brightness", signed(x)));
    }
    if let Some(x) = pick("Contrast2012", "Contrast").filter(|x| *x != 0.0) {
        contrast += x / 100.0;
        report.kept(format!("Contrast {}", signed(x)));
    }
    let (h, s, w, b) = (pick("Highlights2012", "HighlightRecovery").unwrap_or(0.0), pick("Shadows2012", "FillLight").unwrap_or(0.0), get("Whites2012").unwrap_or(0.0), get("Blacks2012").unwrap_or(0.0));
    if [h, s, w, b].iter().any(|v| *v != 0.0) {
        let (db, dc) = tone_sliders(h, s, w, b);
        brightness += db;
        contrast += dc;
        let named: Vec<String> = [("Highlights", h), ("Shadows", s), ("Whites", w), ("Blacks", b)].iter().filter(|(_, v)| *v != 0.0).map(|(n, v)| format!("{n} {}", signed(*v))).collect();
        report.approximated(format!("{} approximated with brightness and contrast (kimchi has no tone ranges)", named.join(", ")));
    }
    e.brightness = brightness;
    e.contrast = contrast;
    // White balance: kelvin for raw photos, −100…100 for JPEGs and video.
    if let Some(t) = get("IncrementalTemperature").filter(|t| *t != 0.0) {
        e.temperature = t / 100.0;
        report.kept(format!("Temperature {}", signed(t)));
    } else if let Some(k) = get("Temperature") {
        let as_shot = settings.get("WhiteBalance").is_some_and(|w| w.text().eq_ignore_ascii_case("As Shot"));
        if !as_shot {
            e.temperature = kelvin_to_warmth(k);
            report.approximated(format!("White balance {k:.0} K (for raw photos) became warmth {} against daylight", signed(e.temperature)));
        }
    }
    if let Some(t) = get("IncrementalTint").filter(|t| *t != 0.0) {
        e.tint = t / 100.0;
        report.kept(format!("Tint {}", signed(t)));
    } else if let Some(t) = get("Tint").filter(|t| *t != 0.0) {
        e.tint = t / 150.0;
        report.approximated(format!("Tint {} (for raw photos) became tint {}", signed(t), signed(e.tint)));
    }
    // Colour.
    let mut sat = get("Saturation").unwrap_or(0.0) / 100.0;
    if sat != 0.0 {
        report.kept(format!("Saturation {}", signed(sat * 100.0)));
    }
    if let Some(v) = get("Vibrance").filter(|v| *v != 0.0) {
        sat += v / 200.0;
        report.approximated(format!("Vibrance {} became saturation (kimchi's saturation treats every colour alike)", signed(v)));
    }
    if settings.get("ConvertToGrayscale").is_some_and(Setting::truthy) {
        sat = -1.0;
        report.kept("Black and white");
    }
    e.saturation = sat;
    // Detail and effects.
    if let Some(v) = get("Sharpness").filter(|v| *v > 0.0) {
        e.sharpen = (v / 150.0) * 0.6;
        report.approximated(format!("Sharpening {v:.0} became sharpen {:.2}", e.sharpen));
    }
    if let Some(v) = get("PostCropVignetteAmount").filter(|v| *v != 0.0) {
        if v < 0.0 {
            e.vignette = -v / 100.0;
            report.kept(format!("Vignette {}", signed(v)));
        } else {
            report.dropped(format!("A light vignette ({}): kimchi's vignette only darkens", signed(v)));
        }
    }
    // What has no equivalent.
    let nonzero = |prefix: &str| settings.iter().any(|(k, v)| k.starts_with(prefix) && v.num().is_some_and(|n| n != 0.0));
    let curve = |k: &str| match settings.get(k) {
        Some(Setting::List(v)) => !v.is_empty() && !is_linear_curve(v),
        _ => false,
    };
    if ["ToneCurvePV2012", "ToneCurvePV2012Red", "ToneCurvePV2012Green", "ToneCurvePV2012Blue", "ToneCurve"].iter().any(|k| curve(k)) || nonzero("ParametricShadows") || nonzero("ParametricDarks") || nonzero("ParametricLights") || nonzero("ParametricHighlights") {
        report.dropped("The tone curve (kimchi has no curves)");
    }
    if nonzero("HueAdjustment") || nonzero("SaturationAdjustment") || nonzero("LuminanceAdjustment") {
        report.dropped("HSL / colour mixer changes (kimchi changes every colour alike)");
    }
    if nonzero("GrayMixer") && settings.get("ConvertToGrayscale").is_some_and(Setting::truthy) {
        report.dropped("The black and white mix (kimchi's black and white is a plain luma)");
    }
    if nonzero("ColorGrade") || nonzero("SplitToning") {
        report.dropped("Colour grading / split toning (kimchi has no colour wheels)");
    }
    for (key, label) in [("Clarity2012", "Clarity"), ("Texture", "Texture"), ("Dehaze", "Dehaze"), ("GrainAmount", "Grain"), ("LuminanceSmoothing", "Noise reduction"), ("ColorNoiseReduction", "Colour noise reduction"), ("PostCropVignetteHighlightContrast", "Vignette highlights")] {
        if let Some(v) = get(key).filter(|v| *v != 0.0)
            && !(key == "ColorNoiseReduction" && v == 25.0)
        {
            report.dropped(format!("{label} {}", signed(v)));
        }
    }
    if settings.keys().any(|k| k.starts_with("Mask") || k == "MaskGroupBasedCorrections") || settings.contains_key("GradientBasedCorrections") || settings.contains_key("PaintBasedCorrections") || settings.contains_key("CircularGradientBasedCorrections") {
        report.dropped("Masks and local adjustments");
    }
    if let Some(p) = profile.filter(|p| !p.name.is_empty() && !is_standard_profile(&p.name)) {
        if p.table {
            report.dropped(format!(
                "The profile “{}”: its look is a colour table only Adobe apps read. To keep it, apply the preset in Photoshop's Camera Raw Filter and export a .cube with File › Export › Color Lookup Tables, then import that LUT",
                p.name
            ));
        } else {
            report.dropped(format!("The profile “{}”", p.name));
        }
    }
    e.clamped()
}

/// `[0, 0, 255, 255]` (or any points on the diagonal) changes nothing.
fn is_linear_curve(v: &[f64]) -> bool {
    v.chunks(2).all(|p| p.len() == 2 && (p[0] - p[1]).abs() < 0.5)
}

/// Profiles that only set the camera's colour rendering, which video doesn't have.
fn is_standard_profile(name: &str) -> bool {
    matches!(name, "Adobe Standard" | "Adobe Color" | "Camera Standard" | "Adobe Standard B&W" | "Adobe Monochrome" | "Embedded")
}

// ---- XMP ----------------------------------------------------------------------------------

/// Reads the presets in an XMP file (usually one).
pub fn read_xmp(path: &Path, text: &str) -> Result<Vec<LookFile>> {
    let root = xml::parse(text)?;
    let descriptions: Vec<&xml::Element> = std::iter::once(&root).chain(root.descendants()).filter(|e| e.local() == "Description" && has_crs(e)).collect();
    // A preset's own Description, not the profile's parameters nested in crs:Look.
    let top: Vec<&xml::Element> = descriptions.iter().copied().filter(|d| !is_inside_look(&root, d)).collect();
    if top.is_empty() {
        return Err("no Lightroom or Camera Raw settings in it (no crs: values)".into());
    }
    let mut out = vec![];
    for d in top {
        let mut settings = Settings::new();
        collect(d, &mut settings);
        let name = alt_text(d, "Name").or_else(|| d.attr("crs:Name").map(str::to_string)).unwrap_or_else(|| stem(path));
        let profile = d.child("Look").map(|look| {
            let desc = look.child("Description").unwrap_or(look);
            let name = desc.attr("crs:Name").map(str::to_string).or_else(|| alt_text(desc, "Name")).unwrap_or_default();
            let table = std::iter::once(desc).chain(desc.descendants()).any(|e| {
                e.attrs.iter().any(|(k, _)| is_table_key(k)) || is_table_key(&e.name)
            });
            Profile { name, table, amount: desc.attr("crs:Amount").and_then(|a| a.parse().ok()) }
        });
        // A profile preset (no settings of its own, only crs:Look and its table).
        let mut report = Report::new("xmp");
        let effects = map(&settings, profile.as_ref(), &mut report);
        if let Some(group) = alt_text(d, "Group") {
            report.kept(format!("From the group “{group}”"));
        }
        out.push(LookFile { name, effects, luts: vec![], report });
    }
    Ok(out)
}

fn is_table_key(k: &str) -> bool {
    let local = k.rsplit(':').next().unwrap_or(k);
    local == "LookTable" || local == "RGBTable" || local.starts_with("Table_")
}

fn has_crs(e: &xml::Element) -> bool {
    e.attrs.iter().any(|(k, _)| k.starts_with("crs:")) || e.children.iter().any(|c| c.name.starts_with("crs:"))
}

fn is_inside_look(root: &xml::Element, target: &xml::Element) -> bool {
    fn walk(e: &xml::Element, target: *const xml::Element, in_look: bool) -> Option<bool> {
        if std::ptr::eq(e, target) {
            return Some(in_look);
        }
        let in_look = in_look || e.local() == "Look";
        e.children.iter().find_map(|c| walk(c, target, in_look))
    }
    walk(root, target, false).unwrap_or(false)
}

/// `crs:` attributes and simple elements (and `rdf:Seq` lists) of a Description.
fn collect(d: &xml::Element, out: &mut Settings) {
    for (k, v) in &d.attrs {
        if let Some(name) = k.strip_prefix("crs:") {
            out.insert(name.to_string(), Setting::Text(v.clone()));
        }
    }
    for c in &d.children {
        let Some(name) = c.name.strip_prefix("crs:") else { continue };
        if name == "Look" || name == "Name" || name == "Group" || name == "Description" {
            continue;
        }
        if let Some(seq) = c.child("Seq") {
            // Curves: "x, y" pairs.
            let v: Vec<f64> = seq.children.iter().flat_map(|li| li.text.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect::<Vec<_>>()).collect();
            out.insert(name.to_string(), Setting::List(v));
        } else if c.children.is_empty() {
            out.insert(name.to_string(), Setting::Text(c.text.trim().to_string()));
        } else if name.ends_with("Corrections") || name.starts_with("Mask") {
            out.insert(name.to_string(), Setting::Text("present".into()));
        }
    }
}

/// The `x-default` text of an `rdf:Alt` child (`crs:Name`, `crs:Group`).
fn alt_text(d: &xml::Element, name: &str) -> Option<String> {
    let el = d.children.iter().find(|c| c.name == format!("crs:{name}"))?;
    let alt = el.child("Alt");
    let text = match alt {
        Some(alt) => alt.children.iter().find(|li| li.attr("xml:lang") == Some("x-default")).or(alt.children.first()).map(|li| li.text.trim().to_string()),
        None => Some(el.text.trim().to_string()),
    }?;
    (!text.is_empty()).then_some(text)
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Look".into())
}

// ---- .lrtemplate ---------------------------------------------------------------------------

pub fn read_lrtemplate(path: &Path, text: &str) -> Result<Vec<LookFile>> {
    let t = lua::parse(text)?;
    let value = t.get("value").and_then(lua::Value::table);
    let settings_table = value.and_then(|v| v.get("settings")).and_then(lua::Value::table).ok_or("this Lightroom template has no develop settings (value.settings)")?;
    let mut settings = Settings::new();
    for (k, v) in &settings_table.fields {
        let s = match v {
            lua::Value::Number(n) => Setting::Num(*n),
            lua::Value::Bool(b) => Setting::Num(if *b { 1.0 } else { 0.0 }),
            lua::Value::Str(s) => Setting::Text(s.clone()),
            lua::Value::Table(t) => Setting::List(t.list.iter().filter_map(|x| if let lua::Value::Number(n) = x { Some(*n) } else { None }).collect()),
            lua::Value::Nil => continue,
        };
        settings.insert(k.clone(), s);
    }
    let text_of = |v: Option<&lua::Value>| match v {
        Some(lua::Value::Str(s)) if !s.is_empty() => Some(lua::zstr(s)),
        _ => None,
    };
    let name = text_of(t.get("title")).or_else(|| text_of(t.get("internalName"))).unwrap_or_else(|| stem(path));
    let profile = match (settings.get("CameraProfile"), settings.get("LookName")) {
        (_, Some(l)) => Some(Profile { name: lua::zstr(&l.text()), table: true, amount: None }),
        (Some(p), None) => Some(Profile { name: p.text(), table: false, amount: None }),
        _ => None,
    };
    if t.get("type").is_some_and(|ty| !matches!(ty, lua::Value::Str(s) if s == "Develop")) {
        return Err("this Lightroom template isn't a develop preset".into());
    }
    let mut report = Report::new("lrtemplate");
    let effects = map(&settings, profile.as_ref(), &mut report);
    Ok(vec![LookFile { name, effects, luts: vec![], report }])
}

#[cfg(test)]
mod tests {
    use super::*;

    const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:UUID="A1B2C3D4E5F60718293A4B5C6D7E8F90"
   crs:SupportsAmount="False"
   crs:SupportsColor="True"
   crs:SupportsMonochrome="True"
   crs:Version="16.0"
   crs:ProcessVersion="15.4"
   crs:Exposure2012="+0.50"
   crs:Contrast2012="+20"
   crs:Highlights2012="-40"
   crs:Shadows2012="+30"
   crs:Whites2012="0"
   crs:Blacks2012="0"
   crs:IncrementalTemperature="+15"
   crs:IncrementalTint="-5"
   crs:Vibrance="+20"
   crs:Saturation="-10"
   crs:Clarity2012="+15"
   crs:PostCropVignetteAmount="-25"
   crs:HueAdjustmentOrange="-6"
   crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Golden Hour</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:Group>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Travel</rdf:li>
    </rdf:Alt>
   </crs:Group>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 12</rdf:li>
     <rdf:li>128, 140</rdf:li>
     <rdf:li>255, 250</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:Look>
    <rdf:Description
     crs:Name="Film Warm 02"
     crs:Amount="1"
     crs:UUID="0D2B6C1C6B8E4B1A">
    <crs:Parameters>
     <rdf:Description
      crs:Version="16.0"
      crs:ProcessVersion="15.4"
      crs:LookTable="E1095149FDB39D7A057BAB208837E2E1"
      crs:ConvertToGrayscale="False"/>
    </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn xmp_preset_maps_and_reports() {
        let looks = read_xmp(Path::new("/p/golden.xmp"), XMP).unwrap();
        assert_eq!(looks.len(), 1);
        let l = &looks[0];
        assert_eq!(l.name, "Golden Hour");
        let e = &l.effects;
        assert!((e.contrast - (0.2 + tone_sliders(-40.0, 30.0, 0.0, 0.0).1)).abs() < 1e-9);
        assert!((e.brightness - (0.2 + tone_sliders(-40.0, 30.0, 0.0, 0.0).0)).abs() < 1e-9);
        assert!((e.temperature - 0.15).abs() < 1e-9 && (e.tint + 0.05).abs() < 1e-9);
        assert!((e.saturation - (-0.1 + 0.1)).abs() < 1e-9);
        assert!((e.vignette - 0.25).abs() < 1e-9);
        let r = &l.report;
        assert!(r.dropped.iter().any(|d| d.contains("tone curve")), "{r:?}");
        assert!(r.dropped.iter().any(|d| d.contains("HSL")));
        assert!(r.dropped.iter().any(|d| d.contains("Clarity +15")));
        assert!(r.dropped.iter().any(|d| d.contains("Film Warm 02") && d.contains("Color Lookup")), "{r:?}");
        assert!(r.approximated.iter().any(|d| d.contains("Highlights -40, Shadows +30")));
    }

    #[test]
    fn xmp_values_as_elements_and_kelvin() {
        let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
<crs:WhiteBalance>Custom</crs:WhiteBalance><crs:Temperature>11000</crs:Temperature><crs:Tint>+30</crs:Tint>
<crs:ConvertToGrayscale>True</crs:ConvertToGrayscale><crs:Sharpness>75</crs:Sharpness>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let l = &read_xmp(Path::new("/p/Mono Cool.xmp"), x).unwrap()[0];
        assert_eq!(l.name, "Mono Cool");
        assert!((l.effects.temperature - 0.6).abs() < 1e-9);
        assert!((l.effects.tint - 0.2).abs() < 1e-9);
        assert_eq!(l.effects.saturation, -1.0);
        assert!((l.effects.sharpen - 0.3).abs() < 1e-9);
        assert!(l.report.approximated.iter().any(|a| a.contains("11000 K")));
        assert!(read_xmp(Path::new("x.xmp"), "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").is_err());
    }

    #[test]
    fn lrtemplate_preset() {
        let t = r#"s = {
	id = "6F2D4C1E-0B7A-4D8E-9C3F-1A2B3C4D5E6F",
	internalName = "Matte Fade",
	title = ZSTR "$$$/AgDevelop/Presets/MatteFade=Matte Fade",
	type = "Develop",
	value = {
		settings = {
			Blacks2012 = 25,
			Contrast2012 = -30,
			ConvertToGrayscale = false,
			Exposure2012 = 0.1,
			ParametricShadows = 10,
			Saturation = -20,
			ToneCurvePV2012 = {
				0,
				40,
				255,
				255,
			},
			SplitToningShadowHue = 210,
			SplitToningShadowSaturation = 15,
		},
		uuid = "1D9E8C7B-6A5F-4E3D-2C1B-0A9F8E7D6C5B",
	},
	version = 0,
}
"#;
        let l = &read_lrtemplate(Path::new("/p/x.lrtemplate"), t).unwrap()[0];
        assert_eq!(l.name, "Matte Fade");
        assert!(l.effects.contrast < -0.3, "blacks up takes contrast away");
        assert!((l.effects.saturation + 0.2).abs() < 1e-9);
        assert!(l.report.dropped.iter().any(|d| d.contains("tone curve")));
        assert!(l.report.dropped.iter().any(|d| d.contains("split toning")));
        assert!(read_lrtemplate(Path::new("x"), "s = { type = \"Develop\" }").is_err());
    }
}
