//! Video plugins on clips ([`kimchi_core::PluginEffect`]): kimchi's own (lsuite plugins built
//! with the `kimchi-plugin` SDK, [`native`]: the stock examples linked in, and bundles installed
//! in `~/.lsuite/plugins/kimchi/<id>/`) and frei0r ([`frei0r`], the filters Kdenlive, Shotcut and
//! ffmpeg use). Each format is a [`Host`]: it finds plugin files in folders, describes them, and
//! makes [`Instance`]s that draw frames. Both hosts are written in Rust and reach the plugins
//! through their C ABIs.
//!
//! A plugin that fails (its call panics, or the host's glue does) is switched off for the rest of
//! the run and reported through [`set_failure_hook`] (kimchi-control saves it as disabled): the
//! picture is drawn without it, and the app goes on. A plugin switched off by the person
//! ([`set_disabled`]) is skipped the same way. Installing, removing or rebuilding a plugin bumps
//! [`generation`], and renderers drop their instances so the new library is used at once (hot
//! reload).
//!
//! Scanning loads every plugin library, which can crash: like the audio plugin scan, each file is
//! described in a child process (kimchi's own executable with `--scan-video-plugin <format>
//! <path>`) and the results are kept in `<data>/plugins/video.json`.
//!
//! The compositor runs a clip's plugins after its colour effects ([`super::grade`]), on the
//! clip's picture (media) or layer (titles, solids, scenes), first to last. Instances are kept
//! per clip slot between frames ([`Pool`]). A generator draws its clip's picture (put one on a
//! solid); a transition plugin is a clip's [`kimchi_core::Transition::plugin`].
//!
//! - [`catalogue`]: where plugins are looked for, the scan and its cache, lookups by id or name.
//! - [`Pool`]: the instances a renderer uses, with each frame's parameter values.
//! - [`value`]: parameter values as people and agents write them, checked against [`ParamInfo`].

pub mod bundle;
pub mod catalogue;
pub mod frei0r;
pub mod native;
mod pool;
pub mod value;

pub use catalogue::{Failure, Folder, ScanReport, configure, lookup, plugins, rescan, scan_child, scan_in_background};
pub use pool::Pool;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use kimchi_core::PluginValue;
use serde::{Deserialize, Serialize};
use tiny_skia::Pixmap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// kimchi's own: lsuite plugins built with the `kimchi-plugin` SDK.
    Kimchi,
    Frei0r,
}

impl Format {
    /// The prefix of plugin ids (`kimchi:`, `frei0r:`).
    pub fn prefix(self) -> &'static str {
        match self {
            Format::Kimchi => "kimchi",
            Format::Frei0r => "frei0r",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Kimchi => "lsuite",
            Format::Frei0r => "frei0r",
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
    /// The library or bundle it was loaded from ([`BUILT_IN`] for kimchi's own built-in plugins).
    pub path: PathBuf,
    pub params: Vec<ParamInfo>,
    /// Draws the same picture for the same input and values at any time, so the compositor may
    /// keep its result on still pictures. False (drawn every frame) unless the plugin says.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timeless: bool,
    /// For lsuite plugins: the bundle it came in (`plugin.toml`'s id), what `plugin.remove`
    /// takes; empty for stock plugins and other formats.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bundle: String,
    /// For lsuite plugins: the library file itself (`path` is the bundle's folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<PathBuf>,
}

/// [`PluginInfo::path`] of the plugins linked into kimchi.
pub const BUILT_IN: &str = "built-in";

impl Default for PluginInfo {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            vendor: String::new(),
            format: Format::Kimchi,
            kind: PluginKind::Effect,
            category: String::new(),
            description: String::new(),
            version: String::new(),
            path: PathBuf::new(),
            params: vec![],
            timeless: false,
            bundle: String::new(),
            library: None,
        }
    }
}

impl PluginInfo {
    pub fn param(&self, name: &str) -> Option<&ParamInfo> {
        self.params.iter().find(|p| p.name == name)
    }

    pub fn is_built_in(&self) -> bool {
        self.path.as_os_str() == BUILT_IN
    }
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
    /// `[x, y]`: fractions of the picture's width and height from its top left (0…1), for every
    /// format.
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
    /// File parameters: the extensions the file picker offers, without dots (empty: any file).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
}

impl Default for ParamInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            label: String::new(),
            kind: ParamKind::Number,
            default: PluginValue::Number(0.0),
            min: 0.0,
            max: 1.0,
            choices: vec![],
            unit: String::new(),
            group: String::new(),
            hint: String::new(),
            extensions: vec![],
        }
    }
}

impl ParamKind {
    /// Its values take keyframes (they can be interpolated).
    pub fn animates(self) -> bool {
        matches!(self, ParamKind::Number | ParamKind::Integer | ParamKind::Color | ParamKind::Point)
    }

    pub fn label(self) -> &'static str {
        match self {
            ParamKind::Number => "number",
            ParamKind::Integer => "integer",
            ParamKind::Toggle => "toggle",
            ParamKind::Choice => "choice",
            ParamKind::Color => "colour",
            ParamKind::Point => "point",
            ParamKind::Text => "text",
            ParamKind::File => "file",
        }
    }
}

impl Format {
    pub const ALL: [Format; 2] = [Format::Kimchi, Format::Frei0r];

    /// `kimchi` (also `lsuite`) or `frei0r`, any case.
    pub fn parse(s: &str) -> Result<Format, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "kimchi" | "lsuite" | "native" => Ok(Format::Kimchi),
            "frei0r" | "frei0r-1" => Ok(Format::Frei0r),
            other => {
                let hint = kimchi_core::closest(other, &["kimchi", "frei0r"]).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
                Err(format!("Unknown video plugin format `{s}`.{hint} Formats: kimchi (lsuite plugins), frei0r."))
            }
        }
    }
}

impl PluginKind {
    pub fn label(self) -> &'static str {
        match self {
            PluginKind::Effect => "effect",
            PluginKind::Generator => "generator",
            PluginKind::Transition => "transition",
        }
    }
}

/// The hosts, by format: kimchi's own, then frei0r.
pub fn hosts() -> &'static [Arc<dyn Host>] {
    static HOSTS: OnceLock<Vec<Arc<dyn Host>>> = OnceLock::new();
    HOSTS.get_or_init(|| vec![Arc::new(native::NativeHost), Arc::new(frei0r::Frei0rHost)])
}

// ---- switched off, failed, changed ------------------------------------------------------------

struct Switches {
    /// Plugin ids the person switched off (`settings.plugins.disabled`).
    disabled: HashSet<String>,
    /// Plugin ids that failed in this run, with why.
    failed: BTreeMap<String, String>,
}

fn switches() -> &'static RwLock<Switches> {
    static S: OnceLock<RwLock<Switches>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(Switches { disabled: HashSet::new(), failed: BTreeMap::new() }))
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// Changes whenever a plugin is installed, removed, rebuilt, switched on or off: renderers make
/// their instances again (hot reload), previews draw again.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Says that plugins changed (see [`generation`]).
pub fn bump() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// The plugins the person switched off. Bumps [`generation`] when the set changes.
pub fn set_disabled(ids: &[String]) {
    let new: HashSet<String> = ids.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let mut s = switches().write().unwrap_or_else(|e| e.into_inner());
    if s.disabled != new {
        s.disabled = new;
        drop(s);
        bump();
    }
}

/// Switched off by the person, or failed in this run.
pub fn is_off(id: &str) -> bool {
    let s = switches().read().unwrap_or_else(|e| e.into_inner());
    s.disabled.contains(id) || s.failed.contains_key(id)
}

/// Why a plugin was switched off in this run, if it failed.
pub fn failure(id: &str) -> Option<String> {
    switches().read().unwrap_or_else(|e| e.into_inner()).failed.get(id).cloned()
}

type FailureHook = Box<dyn Fn(&str, &str) + Send + Sync>;

fn hook() -> &'static Mutex<Option<FailureHook>> {
    static H: OnceLock<Mutex<Option<FailureHook>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// Called (once per plugin) with its id and why, when a plugin fails and is switched off.
pub fn set_failure_hook(f: impl Fn(&str, &str) + Send + Sync + 'static) {
    *hook().lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

/// A plugin failed badly (it panicked, or its library misbehaved): switched off for the rest of
/// the run, reported once, and the picture is drawn without it.
pub fn report_failure(id: &str, why: &str) {
    let first = switches().write().unwrap_or_else(|e| e.into_inner()).failed.insert(id.to_string(), why.to_string()).is_none();
    if first {
        tracing::error!(plugin = id, "video plugin switched off: {why}");
        bump();
        if let Some(h) = hook().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            h(id, why);
        }
    }
}

/// Forgets a failure (the person switched the plugin on again, or installed a new build).
pub fn clear_failure(id: &str) {
    if switches().write().unwrap_or_else(|e| e.into_inner()).failed.remove(id).is_some() {
        bump();
    }
}

/// The host of a format, if this build has one.
pub fn host(format: Format) -> Option<&'static Arc<dyn Host>> {
    hosts().iter().find(|h| h.format() == format)
}

/// A new instance of a plugin, for one slot: built-in plugins directly, the others through their
/// format's host.
pub fn instantiate(info: &PluginInfo) -> Result<Box<dyn Instance>, String> {
    if info.is_built_in() {
        return native::built_in_instance(info);
    }
    let host = host(info.format).ok_or_else(|| format!("This kimchi can't run {} plugins.", info.format.label()))?;
    host.instantiate(info)
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

/// Why a frame wasn't drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderError {
    pub message: String,
    /// The plugin is broken (it panicked): switch it off.
    pub fatal: bool,
}

impl From<String> for RenderError {
    fn from(message: String) -> Self {
        Self { message, fatal: false }
    }
}

/// A plugin ready to draw. Pixmaps are premultiplied RGBA8 (tiny-skia), all the same size.
pub trait Instance: Send {
    /// Draws `output` from `inputs`: the picture for effects, nothing for generators, the
    /// outgoing then the incoming picture for transitions. `params` holds every parameter's value
    /// at this frame (keyframes applied, defaults filled in).
    fn render(&mut self, params: &BTreeMap<String, PluginValue>, inputs: &[&Pixmap], output: &mut Pixmap, ctx: &RenderCtx) -> Result<(), RenderError>;
}
