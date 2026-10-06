//! Colour looks and presets made in other apps, read as kimchi [`Effects`].
//!
//! LUT files are used as they are (the compositor reads every [`LUT_EXTENSIONS`] format: see
//! `kimchi_media::render::grade`); presets that describe corrections (Lightroom / Camera Raw
//! XMP and `.lrtemplate`, Premiere's Lumetri `.prfpset`) are mapped onto kimchi's corrections,
//! with a [`Report`] of the settings that have no kimchi equivalent.

use std::path::{Path, PathBuf};

use kimchi_core::Effects;
use serde::Serialize;

use crate::{Report, Result};

/// LUT files the compositor reads.
pub const LUT_EXTENSIONS: &[&str] = &["cube", "3dl", "csp", "spi1d", "spi3d", "png", "tif", "tiff"];

/// A look format.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookFormat {
    pub id: &'static str,
    pub label: &'static str,
    pub extensions: &'static [&'static str],
    /// `apps::App` ids.
    pub apps: &'static [&'static str],
    pub notes: &'static str,
}

pub const LOOK_FORMATS: &[LookFormat] = &[
    LookFormat { id: "cube", label: "Cube LUT (1D or 3D)", extensions: &["cube"], apps: &["resolve", "premiere", "finalcut", "aftereffects", "capcut", "kdenlive", "shotcut", "lightroom"], notes: "The common LUT format (Adobe / Resolve). Used as it is." },
    LookFormat { id: "3dl", label: "3DL LUT", extensions: &["3dl"], apps: &["resolve", "premiere", "nuke"], notes: "Autodesk / Lustre 3D LUT. Used as it is." },
    LookFormat { id: "csp", label: "cineSpace LUT", extensions: &["csp"], apps: &["resolve", "nuke"], notes: "Rising Sun cineSpace LUT with its pre-LUT shaper. Used as it is." },
    LookFormat { id: "spi", label: "Sony Imageworks LUT", extensions: &["spi1d", "spi3d"], apps: &["nuke", "resolve"], notes: "OpenColorIO's own LUT files. Used as they are." },
    LookFormat { id: "hald", label: "Hald CLUT image", extensions: &["png", "tif", "tiff"], apps: &["kdenlive", "shotcut", "lightroom"], notes: "A LUT stored as a picture (darktable, RawTherapee, G'MIC, ffmpeg haldclut). Used as it is." },
    LookFormat { id: "xmp", label: "Lightroom / Camera Raw preset", extensions: &["xmp", "lrtemplate"], apps: &["lightroom", "premiere", "aftereffects"], notes: "Exposure, contrast, highlights, shadows, whites, blacks, temperature, tint, vibrance, saturation, sharpening and vignette, mapped onto kimchi's corrections." },
    LookFormat { id: "lumetri", label: "Premiere Lumetri preset", extensions: &["prfpset"], apps: &["premiere"], notes: "The Basic correction and Creative sections of a Lumetri Color preset, and its LUTs when they are on this computer." },
];

/// A look read from a file.
#[derive(Debug, Clone)]
pub struct LookFile {
    /// The preset's own name, else the file's.
    pub name: String,
    /// The corrections it sets (a LUT file: only `lut`).
    pub effects: Effects,
    /// LUT files it uses that the look library should copy along with it.
    pub luts: Vec<PathBuf>,
    pub report: Report,
}

/// Every extension [`read`] takes.
pub const EXTENSIONS: &[&str] = &["cube", "3dl", "csp", "spi1d", "spi3d", "png", "tif", "tiff", "xmp", "lrtemplate", "prfpset"];

/// The look format of a file, by its extension.
pub fn format_of(path: &Path) -> Option<&'static LookFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    LOOK_FORMATS.iter().find(|f| f.extensions.contains(&ext.as_str()))
}

/// Reads one look or preset file (a LUT, an XMP or `.lrtemplate`, a `.prfpset`). A preset file
/// holding several looks gives them all.
pub fn read(path: &Path) -> Result<Vec<LookFile>> {
    let fail = |e: String| format!("{}: {e}", path.display());
    let format = format_of(path).ok_or_else(|| {
        fail(format!(
            "kimchi doesn't read this kind of file as a look. Looks are LUTs (.cube, .3dl, .csp, .spi1d, .spi3d, Hald CLUT .png / .tif), Lightroom presets (.xmp, .lrtemplate) and Premiere Lumetri presets (.prfpset)"
        ))
    })?;
    let text = || -> Result<String> {
        let bytes = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
        Ok(match String::from_utf8(bytes) {
            Ok(s) => s,
            // UTF-16 XMP files (Windows tools), else Latin-1.
            Err(e) => {
                let b = e.into_bytes();
                match b.as_slice() {
                    [0xff, 0xfe, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
                    [0xfe, 0xff, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>()),
                    _ => b.iter().map(|c| *c as char).collect(),
                }
            }
        })
    };
    let looks = match format.id {
        "xmp" if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lrtemplate")) => camera_raw::read_lrtemplate(path, &text()?),
        "xmp" => camera_raw::read_xmp(path, &text()?),
        "lumetri" => lumetri::read(path, &text()?),
        _ => lut_look(path, format),
    }
    .map_err(fail)?;
    Ok(looks)
}

/// A LUT file as a look: the LUT at full strength, nothing else.
fn lut_look(path: &Path, format: &LookFormat) -> Result<Vec<LookFile>> {
    if !path.is_file() {
        return Err("the file isn't there".into());
    }
    if format.id == "hald" && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        // A picture is a Hald CLUT only at a Hald size (level³ × level³): say so before copying it.
        let (w, h) = png_size(path).ok_or("not a PNG picture")?;
        let level = (w as f64).cbrt().round() as u32;
        if w != h || level < 2 || level.pow(3) != w {
            return Err(format!("a {w}×{h} picture isn't a Hald CLUT (those are square, 512×512 for level 8)"));
        }
    }
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "LUT".into());
    let mut report = Report::new(format.id);
    report.kept(format!("{} used as it is", format.label));
    let effects = Effects { lut: Some(kimchi_core::Lut { path: path.to_string_lossy().into_owned(), strength: 1.0 }), ..Effects::default() };
    Ok(vec![LookFile { name, effects, luts: vec![path], report }])
}

/// A PNG's size from its header.
fn png_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" || &head[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(head[16..20].try_into().ok()?), u32::from_be_bytes(head[20..24].try_into().ok()?)))
}

/// The look files in a folder and its subfolders, sorted (what `looks.import` takes from a
/// folder). Hidden files and folders are skipped; pictures only count at a Hald CLUT size.
pub fn files_in(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                continue;
            }
            if p.is_dir() {
                if depth < 8 {
                    walk(&p, depth + 1, out);
                }
                continue;
            }
            let Some(f) = format_of(&p) else { continue };
            let is_png = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
            if f.id == "hald" && is_png && png_size(&p).is_none_or(|(w, h)| w != h || ((w as f64).cbrt().round() as u32).pow(3) != w) {
                continue;
            }
            out.push(p);
        }
    }
    let mut out = vec![];
    walk(dir, 0, &mut out);
    out
}

mod camera_raw;
mod lua;
mod lumetri;
mod xml;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_luts_and_finds_files() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("Film");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("Kodak.cube"), "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hi").unwrap();
        std::fs::write(dir.path().join(".hidden.cube"), "x").unwrap();
        std::fs::write(dir.path().join("night.prfpset"), lumetri::tests::sample("", &[("Tone", "Contrast", "10.")])).unwrap();
        let found = files_in(dir.path());
        assert_eq!(found.len(), 2, "{found:?}");
        let l = &read(&sub.join("Kodak.cube")).unwrap()[0];
        assert_eq!(l.name, "Kodak");
        assert!(l.effects.lut.as_ref().unwrap().path.ends_with("Kodak.cube"));
        assert_eq!(l.luts.len(), 1);
        assert!(l.report.is_exact());
        assert!(read(&dir.path().join("notes.txt")).unwrap_err().contains("doesn't read"));
        let p = &read(&dir.path().join("night.prfpset")).unwrap()[0];
        assert!((p.effects.contrast - 0.1).abs() < 1e-9);
    }
}
