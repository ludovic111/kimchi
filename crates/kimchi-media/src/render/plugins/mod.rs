//! Video plugins on clips ([`kimchi_core::PluginEffect`]): kimchi's own (built with the
//! `kimchi-plugin` SDK, [`native`]), frei0r ([`frei0r`], the filters Kdenlive, Shotcut and ffmpeg
//! use) and OpenFX ([`ofx`], the plugins made for DaVinci Resolve, Natron, VEGAS and Nuke). Each
//! format is a [`Host`]: it finds plugin files in folders, describes them, and makes
//! [`Instance`]s that draw frames. All three hosts are written in Rust and reach the plugins
//! through their C ABIs.
//!
//! Scanning loads every plugin library, which can crash: like the audio plugin scan, each file is
//! described in a child process (kimchi's own executable with `--scan-video-plugin <format>
//! <path>`) and the results are kept in `<data>/plugins/video.json`.
//!
//! The compositor runs a clip's plugins after its colour effects ([`super::grade`]), on the
//! clip's picture (media) or layer (titles, solids, scenes), first to last. Instances are kept
//! per clip slot between frames.

pub mod frei0r;
pub mod native;
pub mod ofx;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kimchi_core::PluginValue;
use serde::{Deserialize, Serialize};
use tiny_skia::Pixmap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// kimchi's own (`kimchi-plugin` SDK).
    Kimchi,
    Frei0r,
    Ofx,
}

impl Format {
    /// The prefix of plugin ids (`kimchi:`, `frei0r:`, `ofx:`).
    pub fn prefix(self) -> &'static str {
        match self {
            Format::Kimchi => "kimchi",
            Format::Frei0r => "frei0r",
            Format::Ofx => "ofx",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Kimchi => "kimchi",
            Format::Frei0r => "frei0r",
            Format::Ofx => "OpenFX",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// Changes a picture.
    Effect,
    /// Makes a picture from nothing (fills its clip's layer).
    Generator,
    /// Mixes two pictures (the outgoing and incoming clips).
    Transition,
}

/// A plugin found on this computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    /// `<format prefix>:<the plugin's own id>`: what [`kimchi_core::PluginEffect::plugin`] holds.
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub format: Format,
    pub kind: PluginKind,
    pub category: String,
    pub description: String,
    pub version: String,
    /// The library or bundle it was loaded from.
    pub path: PathBuf,
    pub params: Vec<ParamInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    Number,
    Integer,
    Toggle,
    Choice,
    /// `[r, g, b, a]`, 0…1 (or `#rrggbb[aa]` text).
    Color,
    /// `[x, y]`: for frei0r and OpenFX normalised to the picture (0…1), else pixels.
    Point,
    Text,
    File,
}

/// One parameter of a plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamInfo {
    /// The name values are stored by ([`kimchi_core::PluginEffect::params`]): stable.
    pub name: String,
    /// What the inspector shows (often the same).
    pub label: String,
    pub kind: ParamKind,
    pub default: PluginValue,
    /// Number and Integer ranges (what the slider spans).
    #[serde(default)]
    pub min: f64,
    #[serde(default)]
    pub max: f64,
    /// Choice labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    /// The group or page it is shown under, if the plugin says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

/// What a frame is drawn for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderCtx {
    /// Seconds from the clip's start (scene or source time is the plugin's own business).
    pub time: f64,
    /// Frames per second of the output.
    pub fps: f64,
    /// Output pixels per project pixel (preview and thumbnails draw smaller).
    pub scale: f32,
    /// For transitions: 0…1 through the transition.
    pub progress: f32,
    /// A fast preview frame (playback, scrubbing) rather than a final one.
    pub draft: bool,
}

/// One plugin format.
pub trait Host: Send + Sync {
    fn format(&self) -> Format;

    /// The folders this format's plugins are installed in on this system (they may not exist).
    fn standard_folders(&self) -> Vec<PathBuf>;

    /// Plugin files or bundles in `folder` and its subfolders, without loading them.
    fn find(&self, folder: &Path) -> Vec<PathBuf>;

    /// Loads one file or bundle and describes its plugins (run in the scan's child process).
    fn describe(&self, path: &Path) -> Result<Vec<PluginInfo>, String>;

    /// A new instance of a described plugin, for one clip slot.
    fn instantiate(&self, info: &PluginInfo) -> Result<Box<dyn Instance>, String>;
}

/// A plugin ready to draw. Pixmaps are premultiplied RGBA8 (tiny-skia), all the same size.
pub trait Instance: Send {
    /// Draws `output` from `inputs`: the picture for effects, nothing for generators, the
    /// outgoing then the incoming picture for transitions. `params` holds every parameter's value
    /// at this frame (keyframes applied, defaults filled in).
    fn render(&mut self, params: &BTreeMap<String, PluginValue>, inputs: &[&Pixmap], output: &mut Pixmap, ctx: &RenderCtx) -> Result<(), String>;
}
