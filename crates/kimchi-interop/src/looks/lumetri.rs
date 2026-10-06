//! Premiere Pro effect presets (`.prfpset`): Premiere's object XML, where each `FilterPreset`
//! names a `VideoFilterComponent` whose `Params` point (`ObjectRef`) at `VideoComponentParam`s
//! (a `<Name>`, a `<StartKeyframe>` of `time,value,…` or a `<CurrentValue>`) and
//! `ArbVideoComponentParam`s (base64 blobs: LUT file names, curves, colour wheels). The Lumetri
//! Color effect is `AE.ADBE Lumetri`; its parameters are listed in sections (Basic Correction,
//! Creative, Curves, Color Wheels, HSL Secondary, Vignette), as Premiere Pro 2020 to 2025 write
//! them (layout checked against the Lumetri block Adobe's own CEP samples serialise).
//!
//! The Basic correction (exposure, contrast, highlights, shadows, whites, blacks, temperature,
//! tint, saturation), the Creative section (faded film, sharpen, vibrance, saturation, its look
//! LUT and intensity), the input LUT and the vignette's amount are mapped onto kimchi's
//! corrections and LUT; the rest is reported.

use std::path::{Path, PathBuf};

use kimchi_core::{Effects, Lut};

use super::camera_raw::{exposure_to_brightness, tone_sliders};
use super::{LookFile, xml};
use crate::{Report, Result};

/// A Lumetri parameter read from the preset.
#[derive(Debug, Clone)]
struct Param {
    name: String,
    /// The section heading above it.
    section: String,
    value: Option<String>,
    /// Text in an `ArbVideoComponentParam` blob, when it is one.
    blob: Option<Vec<u8>>,
    checksum: Option<String>,
}

/// The default blobs of a fresh Lumetri (by checksum): curves, wheels and keys left alone.
const DEFAULT_BLOBS: &[&str] = &["4294940310", "3099113656", "1088034218", "3402240421", "3924380446", "306753320", "2930646580", "2374864269"];

pub fn read(path: &Path, text: &str) -> Result<Vec<LookFile>> {
    let root = xml::parse(text)?;
    let all = root.descendants();
    let by_id = |id: &str| all.iter().copied().find(|e| e.attr("ObjectID") == Some(id));
    let components: Vec<&xml::Element> = all.iter().copied().filter(|e| e.local() == "VideoFilterComponent" && e.child_text("MatchName") == Some("AE.ADBE Lumetri")).collect();
    if components.is_empty() {
        let any = all.iter().any(|e| e.local() == "VideoFilterComponent" || e.local() == "FilterPreset");
        return Err(if any {
            "this Premiere preset has no Lumetri Color effect (kimchi reads Lumetri presets; other Premiere effects only run in Premiere)".into()
        } else {
            "not a Premiere effect preset (no FilterPreset in it)".into()
        });
    }
    let mut out = vec![];
    for (n, comp) in components.iter().enumerate() {
        let id = comp.attr("ObjectID").unwrap_or("");
        // The preset naming this component, for its name.
        let preset = all.iter().copied().find(|e| e.local() == "FilterPreset" && e.child("Component").and_then(|c| c.attr("ObjectRef")) == Some(id));
        let name = preset
            .and_then(|p| p.child_text("Name").or(p.child_text("PresetName")))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| preset_bin_name(&all, preset))
            .or_else(|| comp.child("Component").and_then(|c| c.child_text("InstanceName")).filter(|s| !s.is_empty() && *s != "Lumetri Color").map(str::to_string))
            .unwrap_or_else(|| {
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Lumetri".into());
                if components.len() > 1 { format!("{stem} {}", n + 1) } else { stem }
            });
        let mut params = vec![];
        let mut section = String::new();
        let list = comp.child("Component").and_then(|c| c.child("Params"));
        let mut refs: Vec<(usize, &str)> = list
            .map(|l| l.children.iter().filter_map(|p| Some((p.attr("Index")?.parse().ok()?, p.attr("ObjectRef")?))).collect())
            .unwrap_or_default();
        refs.sort_by_key(|r| r.0);
        for (_, r) in refs {
            let Some(p) = by_id(r) else { continue };
            let pname = p.child_text("Name").unwrap_or("").to_string();
            if p.child_text("ParameterControlType") == Some("11") && !pname.trim().is_empty() {
                section = pname.clone();
                continue;
            }
            let value = p.child_text("CurrentValue").map(str::to_string).or_else(|| p.child_text("StartKeyframe").and_then(|k| k.split(',').nth(1)).map(str::to_string));
            let blob_el = p.child("StartKeyframeValue");
            let blob = blob_el.and_then(|b| base64(b.text.trim()));
            params.push(Param { name: pname, section: section.clone(), value, blob, checksum: blob_el.and_then(|b| b.attr("Checksum")).map(str::to_string) });
        }
        let mut report = Report::new("lumetri");
        let effects = map(&params, path, &mut report);
        let luts = effects.lut.iter().map(|l| PathBuf::from(&l.path)).collect();
        out.push(LookFile { name, effects, luts, report });
    }
    Ok(out)
}

/// A preset inside a bin of presets (`<Name>` of the item holding it).
fn preset_bin_name(all: &[&xml::Element], preset: Option<&xml::Element>) -> Option<String> {
    let id = preset?.attr("ObjectID")?;
    all.iter()
        .find(|e| e.descendants().iter().any(|c| c.attr("ObjectRef") == Some(id)) && e.child_text("Name").is_some_and(|n| !n.is_empty()) && e.local() != "VideoFilterComponent")
        .and_then(|e| e.child_text("Name").map(str::to_string))
}

fn map(params: &[Param], preset: &Path, report: &mut Report) -> Effects {
    let num = |section: &str, name: &str| -> Option<f64> {
        params.iter().find(|p| p.section.eq_ignore_ascii_case(section) && p.name.trim().eq_ignore_ascii_case(name)).and_then(|p| p.value.as_deref()).and_then(|v| v.trim().trim_end_matches('.').parse().ok())
    };
    let basic = |n: &str| num("Tone", n).or_else(|| num("Basic Correction", n)).or_else(|| num("White Balance", n));
    let mut e = Effects::default();
    let mut brightness = 0.0;
    let mut contrast = 0.0;
    if let Some(x) = basic("Exposure").filter(|x| *x != 0.0) {
        brightness += exposure_to_brightness(x);
        report.approximated(format!("Exposure {} became brightness", signed(x)));
    }
    if let Some(x) = basic("Contrast").filter(|x| *x != 0.0) {
        contrast += x / 100.0;
        report.kept(format!("Contrast {}", signed(x)));
    }
    let (h, s, w, b) = (basic("Highlights").unwrap_or(0.0), basic("Shadows").unwrap_or(0.0), basic("Whites").unwrap_or(0.0), basic("Blacks").unwrap_or(0.0));
    if [h, s, w, b].iter().any(|v| *v != 0.0) {
        let (db, dc) = tone_sliders(h, s, w, b);
        brightness += db;
        contrast += dc;
        let named: Vec<String> = [("Highlights", h), ("Shadows", s), ("Whites", w), ("Blacks", b)].iter().filter(|(_, v)| *v != 0.0).map(|(n, v)| format!("{n} {}", signed(*v))).collect();
        report.approximated(format!("{} approximated with brightness and contrast", named.join(", ")));
    }
    if let Some(t) = basic("Temperature").filter(|t| *t != 0.0) {
        e.temperature = t / 100.0;
        report.kept(format!("Temperature {}", signed(t)));
    }
    if let Some(t) = basic("Tint").filter(|t| *t != 0.0) {
        e.tint = t / 100.0;
        report.kept(format!("Tint {}", signed(t)));
    }
    let mut sat = basic("Saturation").map_or(1.0, |s| s / 100.0);
    if sat != 1.0 {
        report.kept(format!("Saturation {:.0}", sat * 100.0));
    }
    // Creative.
    let creative = |n: &str| num("Adjustments", n).or_else(|| num("Creative", n));
    if let Some(f) = creative("Faded Film").filter(|f| *f > 0.0) {
        contrast -= 0.3 * f / 100.0;
        brightness += 0.06 * f / 100.0;
        report.approximated(format!("Faded film {f:.0} became lower contrast and lifted brightness"));
    }
    if let Some(v) = creative("Sharpen").filter(|v| *v != 0.0) {
        if v > 0.0 {
            e.sharpen = v / 100.0;
            report.kept(format!("Sharpen {v:.0}"));
        } else {
            report.dropped(format!("Sharpen {v:.0} (softening): kimchi only sharpens"));
        }
    }
    if let Some(v) = creative("Vibrance").filter(|v| *v != 0.0) {
        sat += v / 200.0;
        report.approximated(format!("Vibrance {} became saturation", signed(v)));
    }
    if let Some(cs) = creative("Saturation").filter(|s| *s != 100.0) {
        sat *= cs / 100.0;
        report.kept(format!("Creative saturation {cs:.0}"));
    }
    e.saturation = sat - 1.0;
    e.brightness = brightness;
    e.contrast = contrast;
    if let Some(a) = num("Vignette", "Amount").filter(|a| *a != 0.0) {
        if a < 0.0 {
            e.vignette = (-a / 3.0).min(1.0);
            report.approximated(format!("Vignette {} (its midpoint, roundness and feather are kimchi's own)", signed(a)));
        } else {
            report.dropped(format!("A light vignette ({}): kimchi's vignette only darkens", signed(a)));
        }
    }
    // LUTs: the blobs name them (a path, or a name from Lumetri's menus).
    let named: Vec<(String, String)> = params.iter().filter_map(|p| Some((p.section.clone(), blob_text(p.blob.as_ref()?)?))).filter(|(_, t)| is_lut_name(t)).collect();
    let input = named.iter().find(|(s, _)| s.eq_ignore_ascii_case("Basic Correction")).map(|x| x.1.clone());
    let look = named.iter().find(|(s, _)| s.eq_ignore_ascii_case("Creative")).map(|x| x.1.clone()).or_else(|| named.iter().map(|x| x.1.clone()).find(|t| Some(t) != input.as_ref()));
    let intensity = creative("Intensity").unwrap_or(100.0);
    // kimchi has one LUT a clip: the creative look wins; an input LUT alone is used.
    let mut take = |which: &str, name: &str, strength: f64| match find_lut(name, preset) {
        Some(p) => {
            e.lut = Some(Lut { path: p.to_string_lossy().into_owned(), strength });
            report.kept(format!("{which} LUT “{}”", file_name(name)));
        }
        None => report.dropped(format!("{which} LUT “{}”: it isn't on this computer (import the .cube itself, or put it next to the preset)", file_name(name))),
    };
    match (&input, &look) {
        (Some(i), Some(l)) => {
            take("Look", l, (intensity / 100.0).clamp(0.0, 1.0));
            report.dropped(format!("Input LUT “{}”: kimchi uses one LUT a clip, the look's", file_name(i)));
        }
        (Some(i), None) => take("Input", i, 1.0),
        (None, Some(l)) => take("Look", l, (intensity / 100.0).clamp(0.0, 1.0)),
        (None, None) => {}
    }
    if intensity > 100.0 && look.is_some() {
        report.approximated(format!("Look intensity {intensity:.0} capped at 100"));
    }
    // What has no equivalent.
    let changed = |sections: &[&str]| params.iter().any(|p| sections.iter().any(|s| p.section.eq_ignore_ascii_case(s)) && p.checksum.as_deref().is_some_and(|c| !DEFAULT_BLOBS.contains(&c)) && !p.blob.as_ref().and_then(|b| blob_text(b)).is_some_and(|t| is_lut_name(&t)));
    if changed(&["RGB Curves", "Curves", "Hue Saturation Curve"]) {
        report.dropped("Curves (kimchi has no curves)");
    }
    if changed(&["Color Wheels"]) || changed(&["Adjustments", "Creative"]) {
        report.dropped("Colour wheels and the shadow / highlight tints (kimchi has no colour wheels)");
    }
    if changed(&["HSL Secondary", "Key", "Correction"]) {
        report.dropped("The HSL secondary (kimchi corrects every colour alike)");
    }
    if let Some(t) = num("Adjustments", "Tint Balance").filter(|t| *t != 0.0) {
        report.dropped(format!("Tint balance {}", signed(t)));
    }
    e.clamped()
}

fn signed(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r > 0.0 { format!("+{r}") } else { format!("{r}") }
}

fn file_name(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_string()
}

fn is_lut_name(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    super::LUT_EXTENSIONS.iter().chain(&["look", "itx"]).any(|e| l.ends_with(&format!(".{e}")))
}

/// The text in a parameter blob: UTF-16 (with its byte-order mark) or UTF-8.
fn blob_text(b: &[u8]) -> Option<String> {
    let s = match b {
        [0xff, 0xfe, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        [0xfe, 0xff, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        _ => String::from_utf8(b.to_vec()).ok()?,
    };
    let s = s.trim_matches(char::from(0)).trim().to_string();
    (!s.is_empty() && !s.chars().any(|c| c.is_control())).then_some(s)
}

/// Standard base64 (as Premiere writes its blobs).
fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut acc = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            acc |= val(*c)? << (18 - 6 * i);
        }
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

/// A LUT the preset names: the path when it is there, else a file of that name next to the
/// preset or in Premiere's LUT folders on this computer.
fn find_lut(name: &str, preset: &Path) -> Option<PathBuf> {
    let p = PathBuf::from(name.replace('\\', "/"));
    if p.is_absolute() && p.is_file() {
        return Some(p);
    }
    let file = file_name(name).to_ascii_lowercase();
    let mut dirs: Vec<PathBuf> = preset.parent().map(Path::to_path_buf).into_iter().collect();
    if let Some(app) = crate::apps::app("premiere") {
        dirs.extend(app.look_folders.iter().flat_map(crate::apps::resolve));
        // Lumetri's own LUTs, inside each installed Premiere Pro.
        for install in app.detect.iter().flat_map(crate::apps::resolve) {
            dirs.push(install.join("Lumetri/LUTs"));
            if let Ok(entries) = std::fs::read_dir(&install) {
                dirs.extend(entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "app")).map(|p| p.join("Contents/Lumetri/LUTs")));
            }
        }
    }
    dirs.into_iter().find_map(|d| search(&d, &file, 4))
}

fn search(dir: &Path, file: &str, depth: u32) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = vec![];
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase() == file) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs.sort();
    subdirs.into_iter().find_map(|d| search(&d, file, depth - 1))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A Lumetri preset as Premiere writes one, with the parameters it lists (a shortened
    /// layout: the sections and names of Lumetri's real ones).
    pub(crate) fn sample(lut: &str, values: &[(&str, &str, &str)]) -> String {
        let mut params = vec![];
        let mut objects = String::new();
        let mut id = 10;
        let mut add = |kind: &str, body: String| {
            id += 1;
            objects.push_str(&format!("\t<{kind} ObjectID=\"{id}\" ClassID=\"cc12343e-f113-4d3b-ae05-b287db77d461\" Version=\"9\">\n{body}\t</{kind}>\n"));
            params.push(id);
        };
        let value = |section: &str, name: &str, default: &str| values.iter().find(|v| v.0 == section && v.1 == name).map_or(default.to_string(), |v| v.2.to_string());
        let slider = |name: &str, v: String| format!("\t\t<StartKeyframe>-91445760000000000,{v},0,0,0,0,0,0</StartKeyframe>\n\t\t<ParameterControlType>8</ParameterControlType>\n\t\t<Name>{name}</Name>\n");
        let header = |name: &str| format!("\t\t<StartKeyframe>-91445760000000000,false,0,0,0,0,0,0</StartKeyframe>\n\t\t<ParameterControlType>11</ParameterControlType>\n\t\t<Name>{name}</Name>\n");
        let utf16 = |s: &str| -> String {
            let mut b = vec![0xff, 0xfe];
            for u in s.encode_utf16() {
                b.extend(u.to_le_bytes());
            }
            encode64(&b)
        };
        let blob = |data: String, sum: &str| format!("\t\t<StartKeyframeValue Encoding=\"base64\" Checksum=\"{sum}\">{data}</StartKeyframeValue>\n\t\t<ParameterControlType>10</ParameterControlType>\n\t\t<Name> </Name>\n");
        add("VideoComponentParam", header("Basic Correction"));
        add("ArbVideoComponentParam", blob("/v4=".into(), "4294940310"));
        add("VideoComponentParam", header("White Balance"));
        for n in ["Temperature", "Tint"] {
            add("VideoComponentParam", slider(n, value("White Balance", n, "0.")));
        }
        add("VideoComponentParam", header("Tone"));
        for n in ["Exposure", "Contrast", "Highlights", "Shadows", "Whites", "Blacks"] {
            add("VideoComponentParam", slider(n, value("Tone", n, "0.")));
        }
        add("VideoComponentParam", slider("Saturation", value("Tone", "Saturation", "100.")));
        add("VideoComponentParam", header("Creative"));
        add("ArbVideoComponentParam", blob(if lut.is_empty() { "/v4=".into() } else { utf16(lut) }, "12345"));
        add("VideoComponentParam", slider("Intensity", value("Creative", "Intensity", "100.")));
        add("VideoComponentParam", header("Adjustments"));
        for (n, d) in [("Faded Film", "0."), ("Sharpen", "0."), ("Vibrance", "0."), ("Saturation", "100.")] {
            add("VideoComponentParam", slider(n, value("Adjustments", n, d)));
        }
        add("VideoComponentParam", header("Curves"));
        add("VideoComponentParam", header("RGB Curves"));
        add("ArbVideoComponentParam", blob("AAAAAAIAAAAAAA==".into(), if values.iter().any(|v| v.0 == "curve") { "777" } else { "1088034218" }));
        add("VideoComponentParam", header("Vignette"));
        add("VideoComponentParam", slider("Amount", value("Vignette", "Amount", "0.")));
        let refs: String = params.iter().enumerate().map(|(i, r)| format!("\t\t\t\t<Param Index=\"{i}\" ObjectRef=\"{r}\"/>\n")).collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<PremiereData Version=\"3\">\n\t<FilterPreset ObjectID=\"2\" ClassID=\"ee52a7d2-069e-47f7-aa30-e3e3286b65a3\" Version=\"3\">\n\t\t<Name>Teal Night</Name>\n\t\t<Component ObjectRef=\"3\"/>\n\t\t<FilterMatchName>AE.ADBE Lumetri</FilterMatchName>\n\t</FilterPreset>\n\t<VideoFilterComponent ObjectID=\"3\" ClassID=\"d10da199-beea-4dd1-b941-ed3a78766d50\" Version=\"7\">\n\t\t<VideoFilterType>2</VideoFilterType>\n\t\t<MatchName>AE.ADBE Lumetri</MatchName>\n\t\t<Component Version=\"5\">\n\t\t\t<Params Version=\"1\">\n{refs}\t\t\t</Params>\n\t\t\t<DisplayName>Lumetri Color</DisplayName>\n\t\t</Component>\n\t</VideoFilterComponent>\n{objects}</PremiereData>\n"
        )
    }

    fn encode64(b: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in b.chunks(3) {
            let n = ((c[0] as u32) << 16) | ((*c.get(1).unwrap_or(&0) as u32) << 8) | (*c.get(2).unwrap_or(&0) as u32);
            for i in 0..4 {
                if i <= c.len() {
                    out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn base64_round_trip() {
        for s in [&b""[..], b"a", b"ab", b"abc", b"\xff\xfeL\0U\0T\0"] {
            assert_eq!(base64(&encode64(s)).unwrap(), s);
        }
        assert_eq!(blob_text(&base64("/v4=").unwrap()), None);
    }

    #[test]
    fn lumetri_basic_and_creative() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Teal Night.cube"), "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
        let text = sample(
            "Teal Night.cube",
            &[
                ("Tone", "Exposure", "0.5"),
                ("Tone", "Contrast", "25."),
                ("Tone", "Shadows", "-20."),
                ("White Balance", "Temperature", "-30."),
                ("Tone", "Saturation", "120."),
                ("Creative", "Intensity", "60."),
                ("Adjustments", "Faded Film", "40."),
                ("Adjustments", "Vibrance", "20."),
                ("Vignette", "Amount", "-1.5"),
                ("curve", "", ""),
            ],
        );
        let preset = dir.path().join("teal.prfpset");
        let looks = read(&preset, &text).unwrap();
        assert_eq!(looks.len(), 1);
        let l = &looks[0];
        assert_eq!(l.name, "Teal Night");
        let e = &l.effects;
        let (db, dc) = tone_sliders(0.0, -20.0, 0.0, 0.0);
        assert!((e.brightness - (0.2 + db + 0.024)).abs() < 1e-9, "{e:?}");
        assert!((e.contrast - (0.25 + dc - 0.12)).abs() < 1e-9);
        assert!((e.temperature + 0.3).abs() < 1e-9);
        assert!((e.saturation - (1.2 + 0.1 - 1.0)).abs() < 1e-9);
        assert!((e.vignette - 0.5).abs() < 1e-9);
        let lut = e.lut.as_ref().unwrap();
        assert!(lut.path.ends_with("Teal Night.cube") && (lut.strength - 0.6).abs() < 1e-9);
        assert_eq!(l.luts.len(), 1);
        assert!(l.report.dropped.iter().any(|d| d.contains("Curves")), "{:?}", l.report);
        // The same preset without its LUT file next to it.
        let lone = tempfile::tempdir().unwrap();
        let l = &read(&lone.path().join("teal.prfpset"), &text).unwrap()[0];
        assert!(l.effects.lut.is_none());
        assert!(l.report.dropped.iter().any(|d| d.contains("Teal Night.cube") && d.contains("isn't on this computer")));
    }

    #[test]
    fn not_lumetri() {
        let other = "<PremiereData Version=\"3\"><FilterPreset ObjectID=\"1\"><Component ObjectRef=\"2\"/></FilterPreset><VideoFilterComponent ObjectID=\"2\"><MatchName>AE.ADBE Gaussian Blur 2</MatchName></VideoFilterComponent></PremiereData>";
        assert!(read(Path::new("b.prfpset"), other).unwrap_err().contains("no Lumetri"));
        assert!(read(Path::new("b.prfpset"), "<a/>").unwrap_err().contains("not a Premiere"));
    }
}
