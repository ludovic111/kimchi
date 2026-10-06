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

/// Reads one look or preset file (a LUT, an XMP or `.lrtemplate`, a `.prfpset`). A preset file
/// holding several looks gives them all.
pub fn read(path: &Path) -> Result<Vec<LookFile>> {
    let _ = path;
    Err("Reading looks from other apps isn't built yet.".into())
}
