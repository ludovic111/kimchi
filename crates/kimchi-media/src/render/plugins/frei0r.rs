//! The frei0r host: the video filters, sources and mixers Kdenlive, Shotcut, Flowblade, MLT and
//! ffmpeg use (packages like `frei0r-plugins`). Written against `frei0r.h` 1.2
//! (<https://frei0r.dyne.org/>, the header in github.com/dyne/frei0r, read 2026-10-06).
//!
//! A frei0r plugin is one shared library exporting `f0r_init`, `f0r_get_plugin_info`,
//! `f0r_get_param_info`, `f0r_construct`, `f0r_set_param_value` / `f0r_get_param_value`,
//! `f0r_update` (filters, sources) or `f0r_update2` (mixers) and `f0r_destruct`. Frames are
//! 32-bit pixels with straight (not premultiplied) alpha, in the plugin's colour model (RGBA8888,
//! BGRA8888, or PACKED32 for plugins that only move pixels), with sizes that are multiples of 8:
//! the host pads each frame (repeating its edges) and converts from and to the compositor's
//! premultiplied RGBA.
//!
//! Ids are `frei0r:<file stem>` (`frei0r:glow` for `glow.so`): the name MLT gives the service
//! (`frei0r.glow` in Kdenlive and Shotcut projects) and ffmpeg's `frei0r=filter_name=glow`.
//! Parameters keep the plugin's raw names and 0…1 values, as those projects store them.
//!
//! Filters become effects, sources generators and two-input mixers transitions: the mixer's
//! progress-like parameter (`position`, `fader`, `blend`, `opacity`…) follows the transition,
//! and a mixer without one (a blend mode such as `multiply`) is blended in and out through the
//! transition. Three-input mixers are refused.
//!
//! The spec lets two threads work on two instances at once, but not every plugin keeps that
//! promise (some share globals), so calls into one library are made one at a time. Libraries
//! are loaded once and stay loaded for the life of the process.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::PluginValue;
use libloading::Library;
use rayon::prelude::*;
use tiny_skia::Pixmap;

use super::{Format, Host, Instance, ParamInfo, ParamKind, PluginInfo, PluginKind, RenderCtx, RenderError};

const TYPE_FILTER: c_int = 0;
const TYPE_SOURCE: c_int = 1;
const TYPE_MIXER2: c_int = 2;
const TYPE_MIXER3: c_int = 3;

const MODEL_BGRA8888: c_int = 0;
#[allow(dead_code)] // the default, named for the spec
const MODEL_RGBA8888: c_int = 1;
const MODEL_PACKED32: c_int = 2;

const PARAM_BOOL: c_int = 0;
const PARAM_DOUBLE: c_int = 1;
const PARAM_COLOR: c_int = 2;
const PARAM_POSITION: c_int = 3;
const PARAM_STRING: c_int = 4;

/// The size instances are made at to read the parameters' defaults.
const DESCRIBE_SIZE: (u32, u32) = (320, 240);

/// Names of a mixer's parameter that moves it from its first picture to its second.
const PROGRESS_NAMES: &[&str] = &["position", "progress", "fader", "transition", "blend", "opacity", "mix"];

#[repr(C)]
struct PluginInfoRaw {
    name: *const c_char,
    author: *const c_char,
    plugin_type: c_int,
    color_model: c_int,
    frei0r_version: c_int,
    major_version: c_int,
    minor_version: c_int,
    num_params: c_int,
    explanation: *const c_char,
}

#[repr(C)]
struct ParamInfoRaw {
    name: *const c_char,
    kind: c_int,
    explanation: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Color {
    r: f32,
    g: f32,
    b: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Position {
    x: f64,
    y: f64,
}

/// Room for any parameter value a plugin writes (with slack for sloppy ones).
#[repr(C, align(16))]
struct ParamSlot([u8; 64]);

type InitFn = unsafe extern "C" fn() -> c_int;
type DeinitFn = unsafe extern "C" fn();
type GetPluginInfoFn = unsafe extern "C" fn(*mut PluginInfoRaw);
type GetParamInfoFn = unsafe extern "C" fn(*mut ParamInfoRaw, c_int);
type ConstructFn = unsafe extern "C" fn(c_uint, c_uint) -> *mut c_void;
type DestructFn = unsafe extern "C" fn(*mut c_void);
type ParamValueFn = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int);
type UpdateFn = unsafe extern "C" fn(*mut c_void, f64, *const u32, *mut u32);
type Update2Fn = unsafe extern "C" fn(*mut c_void, f64, *const u32, *const u32, *const u32, *mut u32);

struct Api {
    get_param_info: GetParamInfoFn,
    construct: ConstructFn,
    destruct: DestructFn,
    set_param_value: ParamValueFn,
    get_param_value: ParamValueFn,
    update: Option<UpdateFn>,
    update2: Option<Update2Fn>,
    _deinit: Option<DeinitFn>,
}

/// One parameter as the plugin describes it.
#[derive(Debug, Clone)]
struct RawParam {
    name: String,
    kind: c_int,
    explanation: String,
}

/// A loaded frei0r library and what it says about itself.
struct Lib {
    api: Api,
    name: String,
    author: String,
    plugin_type: c_int,
    color_model: c_int,
    version: (c_int, c_int),
    explanation: String,
    params: Vec<RawParam>,
    /// Calls into the library are made one at a time (see the module docs).
    lock: Mutex<()>,
    _lib: Library,
}

// SAFETY: the library's functions are plain C functions; every call that touches plugin state
// is made while holding `lock`.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

fn libraries() -> &'static Mutex<HashMap<PathBuf, Arc<Lib>>> {
    static LIBS: OnceLock<Mutex<HashMap<PathBuf, Arc<Lib>>>> = OnceLock::new();
    LIBS.get_or_init(Default::default)
}

fn text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: frei0r strings are 0-terminated UTF-8 owned by the plugin.
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().trim().to_string()
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Loads (once) the library at `path`.
fn load(path: &Path) -> Result<Arc<Lib>, String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut libs = libraries().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(lib) = libs.get(&key) {
        return Ok(lib.clone());
    }
    let lib = Arc::new(open(&key)?);
    libs.insert(key, lib.clone());
    Ok(lib)
}

fn open(path: &Path) -> Result<Lib, String> {
    let shown = path.display();
    // SAFETY: loading a library runs its constructors; the scan does this in a child process
    // first, so a library that crashes is never loaded by the app.
    let library = unsafe { Library::new(path) }.map_err(|e| format!("couldn't load {shown}: {e}"))?;
    macro_rules! symbol {
        ($name:literal, $ty:ty) => {
            // SAFETY: the symbol's type is the one frei0r.h declares.
            unsafe { library.get::<$ty>(concat!($name, "\0").as_bytes()) }.ok().map(|s| *s)
        };
    }
    let missing = |name: &str| format!("{shown} isn't a frei0r plugin (it has no {name})");
    let init = symbol!("f0r_init", InitFn);
    let get_plugin_info = symbol!("f0r_get_plugin_info", GetPluginInfoFn).ok_or_else(|| missing("f0r_get_plugin_info"))?;
    let api = Api {
        get_param_info: symbol!("f0r_get_param_info", GetParamInfoFn).ok_or_else(|| missing("f0r_get_param_info"))?,
        construct: symbol!("f0r_construct", ConstructFn).ok_or_else(|| missing("f0r_construct"))?,
        destruct: symbol!("f0r_destruct", DestructFn).ok_or_else(|| missing("f0r_destruct"))?,
        set_param_value: symbol!("f0r_set_param_value", ParamValueFn).ok_or_else(|| missing("f0r_set_param_value"))?,
        get_param_value: symbol!("f0r_get_param_value", ParamValueFn).ok_or_else(|| missing("f0r_get_param_value"))?,
        update: symbol!("f0r_update", UpdateFn),
        update2: symbol!("f0r_update2", Update2Fn),
        _deinit: symbol!("f0r_deinit", DeinitFn),
    };
    if let Some(init) = init {
        // SAFETY: called once, right after loading, as the spec asks. Its result isn't
        // specified (most plugins return 1), so it isn't checked.
        unsafe { init() };
    }
    let mut info = PluginInfoRaw {
        name: std::ptr::null(),
        author: std::ptr::null(),
        plugin_type: -1,
        color_model: -1,
        frei0r_version: 0,
        major_version: 0,
        minor_version: 0,
        num_params: 0,
        explanation: std::ptr::null(),
    };
    // SAFETY: `info` is a valid f0r_plugin_info_t for the plugin to fill.
    unsafe { get_plugin_info(&mut info) };
    if info.frei0r_version > 1 {
        return Err(format!("{shown} is made for frei0r {}; kimchi runs frei0r 1 plugins", info.frei0r_version));
    }
    if !(TYPE_FILTER..=TYPE_MIXER3).contains(&info.plugin_type) {
        return Err(format!("{shown} is a kind of frei0r plugin kimchi doesn't know ({})", info.plugin_type));
    }
    if !(MODEL_BGRA8888..=MODEL_PACKED32).contains(&info.color_model) {
        return Err(format!("{shown} uses a frei0r colour model kimchi doesn't know ({})", info.color_model));
    }
    if api.update.is_none() && api.update2.is_none() {
        return Err(missing("f0r_update"));
    }
    let count = info.num_params.clamp(0, 1024);
    let params = (0..count)
        .map(|i| {
            let mut raw = ParamInfoRaw { name: std::ptr::null(), kind: -1, explanation: std::ptr::null() };
            // SAFETY: `raw` is a valid f0r_param_info_t; `i` is below num_params.
            unsafe { (api.get_param_info)(&mut raw, i) };
            RawParam { name: text(raw.name), kind: raw.kind, explanation: text(raw.explanation) }
        })
        .collect();
    let name = text(info.name);
    Ok(Lib {
        name: if name.is_empty() { file_stem(path) } else { name },
        author: text(info.author),
        plugin_type: info.plugin_type,
        color_model: info.color_model,
        version: (info.major_version, info.minor_version),
        explanation: text(info.explanation),
        params,
        api,
        lock: Mutex::new(()),
        _lib: library,
    })
}

impl Lib {
    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn kind(&self) -> PluginKind {
        match self.plugin_type {
            TYPE_SOURCE => PluginKind::Generator,
            TYPE_MIXER2 => PluginKind::Transition,
            _ => PluginKind::Effect,
        }
    }

    /// The index of a mixer's progress parameter, if it has one.
    fn progress_param(&self) -> Option<usize> {
        if self.plugin_type != TYPE_MIXER2 {
            return None;
        }
        PROGRESS_NAMES.iter().find_map(|wanted| {
            self.params.iter().position(|p| p.kind == PARAM_DOUBLE && p.name.eq_ignore_ascii_case(wanted))
        })
    }

    /// Reads parameter `index` of `instance` (the caller holds the lock).
    fn read(&self, instance: *mut c_void, index: usize) -> Option<PluginValue> {
        let mut slot = ParamSlot([0; 64]);
        let ptr = slot.0.as_mut_ptr() as *mut c_void;
        // SAFETY: `slot` is large and aligned enough for every frei0r parameter type.
        unsafe { (self.api.get_param_value)(instance, ptr, index as c_int) };
        // SAFETY (each read below): the plugin wrote a value of the parameter's own type.
        Some(match self.params[index].kind {
            PARAM_BOOL => PluginValue::Bool(unsafe { *(ptr as *const f64) } >= 0.5),
            PARAM_DOUBLE => PluginValue::Number(unsafe { *(ptr as *const f64) }),
            PARAM_COLOR => {
                let c = unsafe { *(ptr as *const Color) };
                PluginValue::Vector(vec![c.r as f64, c.g as f64, c.b as f64, 1.0])
            }
            PARAM_POSITION => {
                let p = unsafe { *(ptr as *const Position) };
                PluginValue::Vector(vec![p.x, p.y])
            }
            PARAM_STRING => {
                let s = unsafe { *(ptr as *const *const c_char) };
                PluginValue::Text(text(s))
            }
            _ => return None,
        })
    }

    /// Sets parameter `index` of `instance` (the caller holds the lock). Values of the wrong
    /// shape are ignored.
    fn write(&self, instance: *mut c_void, index: usize, value: &PluginValue) {
        let kind = self.params[index].kind;
        let set = |ptr: *mut c_void| {
            // SAFETY: `ptr` points at a value of the parameter's type, alive for the call (the
            // plugin copies it).
            unsafe { (self.api.set_param_value)(instance, ptr, index as c_int) }
        };
        match kind {
            PARAM_BOOL => {
                if let Some(on) = as_bool(value) {
                    let mut v: f64 = if on { 1.0 } else { 0.0 };
                    set(&mut v as *mut f64 as *mut c_void);
                }
            }
            PARAM_DOUBLE => {
                if let Some(n) = as_number(value) {
                    let mut v = n;
                    set(&mut v as *mut f64 as *mut c_void);
                }
            }
            PARAM_COLOR => {
                if let Some([r, g, b, _]) = as_color(value) {
                    let mut c = Color { r: r as f32, g: g as f32, b: b as f32 };
                    set(&mut c as *mut Color as *mut c_void);
                }
            }
            PARAM_POSITION => {
                if let PluginValue::Vector(v) = value
                    && v.len() >= 2
                {
                    let mut p = Position { x: v[0], y: v[1] };
                    set(&mut p as *mut Position as *mut c_void);
                }
            }
            PARAM_STRING => {
                let s = match value {
                    PluginValue::Text(s) => s.clone(),
                    PluginValue::Number(n) => n.to_string(),
                    PluginValue::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
                    PluginValue::Vector(_) => return,
                };
                let Ok(c) = CString::new(s) else { return };
                let mut p: *const c_char = c.as_ptr();
                set(&mut p as *mut *const c_char as *mut c_void);
            }
            _ => {}
        }
    }

    fn describe(&self, path: &Path) -> Result<PluginInfo, String> {
        if self.plugin_type == TYPE_MIXER3 {
            return Err(format!(
                "\"{}\" ({}) mixes three pictures; kimchi's transitions mix two, so it can't be used",
                self.name,
                path.display()
            ));
        }
        // The defaults are what a fresh instance holds (the spec says construct sets them all).
        let defaults: Vec<Option<PluginValue>> = {
            let _g = self.guard();
            // SAFETY: the size is a positive multiple of 8.
            let instance = unsafe { (self.api.construct)(DESCRIBE_SIZE.0, DESCRIBE_SIZE.1) };
            if instance.is_null() {
                return Err(format!("\"{}\" ({}) failed to start", self.name, path.display()));
            }
            let values = (0..self.params.len()).map(|i| self.read(instance, i)).collect();
            // SAFETY: made just above and not used after this.
            unsafe { (self.api.destruct)(instance) };
            values
        };
        let progress = self.progress_param();
        let params = self
            .params
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != progress)
            .filter_map(|(i, p)| param_info(p, defaults[i].clone()))
            .collect();
        let (category, what) = match self.plugin_type {
            TYPE_SOURCE => ("Generator", "Makes a picture"),
            TYPE_MIXER2 => ("Mixer", "Mixes two pictures"),
            _ => ("Filter", "Changes a picture"),
        };
        Ok(PluginInfo {
            id: format!("{}:{}", Format::Frei0r.prefix(), file_stem(path)),
            name: self.name.clone(),
            vendor: self.author.clone(),
            format: Format::Frei0r,
            kind: self.kind(),
            category: category.into(),
            description: if self.explanation.is_empty() { what.into() } else { self.explanation.clone() },
            version: format!("{}.{}", self.version.0, self.version.1),
            path: path.to_path_buf(),
            params,
            ..Default::default()
        })
    }
}

fn param_info(p: &RawParam, default: Option<PluginValue>) -> Option<ParamInfo> {
    let (kind, default, min, max) = match p.kind {
        PARAM_BOOL => (ParamKind::Toggle, default.unwrap_or(PluginValue::Bool(false)), 0.0, 1.0),
        PARAM_DOUBLE => {
            // Doubles are 0…1 by the spec, but some plugins start outside it: widen the slider.
            let d = default.as_ref().and_then(as_number).unwrap_or(0.0);
            (ParamKind::Number, PluginValue::Number(d), d.min(0.0), d.max(1.0))
        }
        PARAM_COLOR => (ParamKind::Color, default.unwrap_or(PluginValue::Vector(vec![0.0, 0.0, 0.0, 1.0])), 0.0, 1.0),
        PARAM_POSITION => (ParamKind::Point, default.unwrap_or(PluginValue::Vector(vec![0.5, 0.5])), 0.0, 1.0),
        PARAM_STRING => {
            let lower = p.name.to_lowercase();
            let file = ["file", "path", "image", "lut", "classifier"].iter().any(|w| lower.contains(w));
            (if file { ParamKind::File } else { ParamKind::Text }, default.unwrap_or(PluginValue::Text(String::new())), 0.0, 0.0)
        }
        _ => return None,
    };
    Some(ParamInfo {
        name: p.name.clone(),
        label: label(&p.name),
        kind,
        default,
        min,
        max,
        choices: Vec::new(),
        unit: String::new(),
        group: String::new(),
        hint: p.explanation.clone(),
        extensions: Vec::new(),
    })
}

/// "blurAmount" → "Blur amount", "dot_radius" → "Dot radius", "Glow" stays.
fn label(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        prev_lower = c.is_lowercase();
    }
    let mut chars = out.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => name.to_string(),
    }
}

fn as_number(v: &PluginValue) -> Option<f64> {
    match v {
        PluginValue::Number(n) => Some(*n),
        PluginValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        PluginValue::Text(t) => t.trim().parse().ok(),
        PluginValue::Vector(v) => v.first().copied(),
    }
}

fn as_bool(v: &PluginValue) -> Option<bool> {
    match v {
        PluginValue::Bool(b) => Some(*b),
        PluginValue::Number(n) => Some(*n >= 0.5),
        PluginValue::Text(t) => match t.trim().to_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        PluginValue::Vector(_) => None,
    }
}

/// `[r, g, b(, a)]` or `#rrggbb[aa]`.
pub(crate) fn as_color(v: &PluginValue) -> Option<[f64; 4]> {
    match v {
        PluginValue::Vector(v) if v.len() >= 3 => Some([v[0], v[1], v[2], v.get(3).copied().unwrap_or(1.0)]),
        PluginValue::Text(t) => {
            let hex = t.trim().trim_start_matches('#');
            if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
                return None;
            }
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|b| b as f64 / 255.0);
            Some([byte(0)?, byte(2)?, byte(4)?, if hex.len() == 8 { byte(6)? } else { 1.0 }])
        }
        _ => None,
    }
}

/// The frei0r host.
#[derive(Debug, Default, Clone, Copy)]
pub struct Frei0rHost;

impl Frei0rHost {
    pub fn new() -> Self {
        Self
    }
}

/// File extensions of plugin libraries on this system.
fn extensions() -> &'static [&'static str] {
    if cfg!(windows) {
        &["dll"]
    } else if cfg!(target_os = "macos") {
        &["so", "dylib"]
    } else {
        &["so"]
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

impl Host for Frei0rHost {
    fn format(&self) -> Format {
        Format::Frei0r
    }

    fn standard_folders(&self) -> Vec<PathBuf> {
        // The spec: FREI0R_PATH, when set, replaces the default list.
        if let Some(list) = std::env::var_os("FREI0R_PATH").filter(|v| !v.is_empty()) {
            return std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()).collect();
        }
        let mut folders = Vec::new();
        // The user's own first: the spec gives them precedence.
        if let Some(home) = home() {
            folders.push(home.join(".frei0r-1").join("lib"));
        }
        if cfg!(windows) {
            if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
                folders.push(exe_dir.join("frei0r-1"));
                folders.push(exe_dir.join("lib").join("frei0r-1"));
            }
            for var in ["ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(pf) = std::env::var_os(var).map(PathBuf::from) {
                    folders.push(pf.join("frei0r-1"));
                    // Shotcut and Kdenlive bring their own copies.
                    folders.push(pf.join("Shotcut").join("lib").join("frei0r-1"));
                    folders.push(pf.join("kdenlive").join("lib").join("frei0r-1"));
                }
            }
        } else if cfg!(target_os = "macos") {
            folders.push("/opt/homebrew/lib/frei0r-1".into());
            folders.push("/usr/local/lib/frei0r-1".into());
            folders.push("/Applications/Shotcut.app/Contents/PlugIns/frei0r-1".into());
            folders.push("/Applications/kdenlive.app/Contents/PlugIns/frei0r-1".into());
        } else {
            folders.push("/usr/local/lib/frei0r-1".into());
            folders.push("/usr/lib/frei0r-1".into());
            folders.push("/usr/lib64/frei0r-1".into());
            for triple in ["x86_64-linux-gnu", "aarch64-linux-gnu"] {
                folders.push(Path::new("/usr/lib").join(triple).join("frei0r-1"));
            }
        }
        folders
    }

    fn find(&self, folder: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        walk(folder, 0, &mut found);
        found.sort();
        found
    }

    fn describe(&self, path: &Path) -> Result<Vec<PluginInfo>, String> {
        let lib = load(path)?;
        Ok(vec![lib.describe(path)?])
    }

    fn instantiate(&self, info: &PluginInfo) -> Result<Box<dyn Instance>, String> {
        let lib = load(&info.path)?;
        let progress = lib.progress_param();
        let names = lib.params.iter().map(|p| p.name.clone()).collect();
        Ok(Box::new(Frei0rInstance {
            lib,
            name: info.name.clone(),
            instance: std::ptr::null_mut(),
            size: (0, 0),
            frames: Vec::new(),
            progress,
            names,
            set: Vec::new(),
        }))
    }
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = std::fs::metadata(&path) else { continue };
        if kind.is_dir() {
            // Vendor folders, one level or two (`<vendor>/<plugin>.so`).
            if depth < 3 {
                walk(&path, depth + 1, found);
            }
        } else if path.extension().is_some_and(|e| extensions().iter().any(|x| e.eq_ignore_ascii_case(x))) {
            found.push(path);
        }
    }
}

/// A frame buffer frei0r accepts: 16-byte aligned, `w * h` pixels.
struct Frame {
    data: Vec<[u32; 4]>,
}

impl Frame {
    fn new(pixels: usize) -> Self {
        // `[u32; 4]` is 16 bytes; Vec of it is only 4-aligned, so over-allocate by one and
        // offset (see `ptr`).
        Self { data: vec![[0; 4]; pixels.div_ceil(4) + 1] }
    }

    fn offset(&self) -> usize {
        let addr = self.data.as_ptr() as usize;
        (16 - addr % 16) % 16 / 4
    }

    fn pixels(&mut self, len: usize) -> &mut [u32] {
        let off = self.offset();
        // SAFETY: the allocation holds `len + 4` u32s; the slice starts at a 16-byte boundary
        // within its first 4.
        unsafe { std::slice::from_raw_parts_mut((self.data.as_mut_ptr() as *mut u32).add(off), len) }
    }
}

struct Frei0rInstance {
    lib: Arc<Lib>,
    name: String,
    instance: *mut c_void,
    /// The padded size the instance was made at.
    size: (u32, u32),
    /// Inputs then the output.
    frames: Vec<Frame>,
    progress: Option<usize>,
    names: Vec<String>,
    /// The values last set, so unchanged ones aren't set again.
    set: Vec<Option<PluginValue>>,
}

// SAFETY: the instance pointer is only used through `&mut self`, with the library's lock held.
unsafe impl Send for Frei0rInstance {}

impl Drop for Frei0rInstance {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl Frei0rInstance {
    fn destroy(&mut self) {
        if !self.instance.is_null() {
            let _g = self.lib.guard();
            // SAFETY: made by construct, destroyed once.
            unsafe { (self.lib.api.destruct)(self.instance) };
            self.instance = std::ptr::null_mut();
        }
    }

    fn ensure(&mut self, w: u32, h: u32) -> Result<(), String> {
        let padded = (w.div_ceil(8) * 8, h.div_ceil(8) * 8);
        if !self.instance.is_null() && self.size == padded {
            return Ok(());
        }
        self.destroy();
        let instance = {
            let _g = self.lib.guard();
            // SAFETY: positive multiples of 8.
            unsafe { (self.lib.api.construct)(padded.0, padded.1) }
        };
        if instance.is_null() {
            return Err(format!("the frei0r plugin \"{}\" couldn't start at {w}×{h}", self.name));
        }
        self.instance = instance;
        self.size = padded;
        self.set = vec![None; self.names.len()];
        let pixels = padded.0 as usize * padded.1 as usize;
        self.frames = (0..4).map(|_| Frame::new(pixels)).collect();
        Ok(())
    }
}

impl Instance for Frei0rInstance {
    fn render(&mut self, params: &BTreeMap<String, PluginValue>, inputs: &[&Pixmap], output: &mut Pixmap, ctx: &RenderCtx) -> Result<(), RenderError> {
        self.draw(params, inputs, output, ctx).map_err(RenderError::from)
    }
}

impl Frei0rInstance {
    fn draw(&mut self, params: &BTreeMap<String, PluginValue>, inputs: &[&Pixmap], output: &mut Pixmap, ctx: &RenderCtx) -> Result<(), String> {
        let lib = self.lib.clone();
        let needed = match lib.plugin_type {
            TYPE_SOURCE => 0,
            TYPE_MIXER2 => 2,
            TYPE_MIXER3 => return Err(format!("\"{}\" mixes three pictures, which kimchi can't give it", self.name)),
            _ => 1,
        };
        if inputs.len() < needed {
            return Err(format!("\"{}\" needs {needed} picture(s) and was given {}", self.name, inputs.len()));
        }
        let (w, h) = (output.width(), output.height());
        if inputs.iter().any(|p| p.width() != w || p.height() != h) {
            return Err(format!("\"{}\" was given pictures of different sizes", self.name));
        }
        self.ensure(w, h)?;
        let (pw, ph) = self.size;
        let len = pw as usize * ph as usize;
        let bgra = lib.color_model == MODEL_BGRA8888;
        let mut opaque_in = true;
        for (i, input) in inputs.iter().take(needed).enumerate() {
            opaque_in &= to_straight(input, self.frames[i].pixels(len), pw as usize, ph as usize, bgra);
        }
        let blend_through = lib.plugin_type == TYPE_MIXER2 && self.progress.is_none();
        let ptrs: Vec<*mut u32> = self.frames.iter_mut().map(|f| f.pixels(len).as_mut_ptr()).collect();
        {
            let _g = lib.guard();
            for (i, name) in self.names.iter().enumerate() {
                let value = if Some(i) == self.progress {
                    Some(PluginValue::Number(ctx.progress.clamp(0.0, 1.0) as f64))
                } else if blend_through && lib.params[i].kind == PARAM_DOUBLE && PROGRESS_NAMES.contains(&name.to_lowercase().as_str()) {
                    None
                } else {
                    params.get(name).cloned()
                };
                if let Some(value) = value
                    && self.set[i].as_ref() != Some(&value)
                {
                    lib.write(self.instance, i, &value);
                    self.set[i] = Some(value);
                }
            }
            let null = std::ptr::null();
            let out = ptrs[3];
            // SAFETY: every buffer holds `pw * ph` 16-byte aligned pixels; the instance was made
            // at that size.
            unsafe {
                match (lib.plugin_type, lib.api.update, lib.api.update2) {
                    (TYPE_MIXER2, _, Some(update2)) => update2(self.instance, ctx.time, ptrs[0], ptrs[1], null, out),
                    (TYPE_MIXER2, _, None) => {
                        return Err(format!("\"{}\" is a mixer without f0r_update2", self.name));
                    }
                    (TYPE_SOURCE, Some(update), _) => update(self.instance, ctx.time, null, out),
                    (TYPE_SOURCE, None, Some(update2)) => update2(self.instance, ctx.time, null, null, null, out),
                    (_, Some(update), _) => update(self.instance, ctx.time, ptrs[0], out),
                    (_, None, Some(update2)) => update2(self.instance, ctx.time, ptrs[0], null, null, out),
                    (_, None, None) => unreachable!("checked when loading"),
                }
            }
        }
        let (ins, outs) = self.frames.split_at_mut(3);
        let out = outs[0].pixels(len);
        // Sources, and filters given an opaque picture, that leave alpha at 0 don't handle
        // alpha: show their colours. A filter that drops an alpha it was given gets it back.
        let alpha_unset = out.par_chunks(pw as usize).all(|row| row.iter().all(|p| p.to_le_bytes()[3] == 0));
        let restore = alpha_unset && needed > 0 && !opaque_in;
        let source = if restore { Some(&*ins[0].pixels(len)) } else { None };
        from_straight(out, source, alpha_unset && !restore, output, pw as usize, bgra);
        if blend_through {
            blend_through_transition(inputs[0], inputs[1], output, ctx.progress);
        }
        Ok(())
    }
}

/// 65536 / a, rounded, for unpremultiplying.
fn reciprocals() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (a, v) in t.iter_mut().enumerate().skip(1) {
            *v = ((255u32 << 16) + a as u32 / 2) / a as u32;
        }
        t
    })
}

/// Premultiplied RGBA8 into a padded straight-alpha frame (RGBA, or BGRA when `bgra`), the
/// padding repeating the last column and row. Returns whether the picture is fully opaque.
fn to_straight(src: &Pixmap, dst: &mut [u32], pw: usize, ph: usize, bgra: bool) -> bool {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let data = src.data();
    let recip = reciprocals();
    let opaque = std::sync::atomic::AtomicBool::new(true);
    dst.par_chunks_mut(pw).enumerate().for_each(|(y, row)| {
        let sy = y.min(h - 1);
        let line = &data[sy * w * 4..(sy + 1) * w * 4];
        let mut all = true;
        for (x, px) in row.iter_mut().enumerate() {
            let s = &line[x.min(w - 1) * 4..x.min(w - 1) * 4 + 4];
            let a = s[3];
            all &= a == 255;
            let un = |c: u8| -> u8 {
                if a == 255 {
                    c
                } else if a == 0 {
                    0
                } else {
                    ((c as u32 * recip[a as usize] + 32768) >> 16).min(255) as u8
                }
            };
            let (r, g, b) = (un(s[0]), un(s[1]), un(s[2]));
            *px = u32::from_le_bytes(if bgra { [b, g, r, a] } else { [r, g, b, a] });
        }
        if !all {
            opaque.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    });
    let _ = ph;
    opaque.into_inner()
}

/// A padded straight-alpha frame back into premultiplied RGBA8, cropped. With `alpha_from`, the
/// alpha comes from that frame instead; with `opaque`, alpha is 255.
fn from_straight(src: &[u32], alpha_from: Option<&[u32]>, opaque: bool, dst: &mut Pixmap, pw: usize, bgra: bool) {
    let w = dst.width() as usize;
    dst.data_mut().par_chunks_mut(w * 4).enumerate().for_each(|(y, line)| {
        let row = &src[y * pw..y * pw + w];
        let alpha_row = alpha_from.map(|a| &a[y * pw..y * pw + w]);
        for (x, px) in row.iter().enumerate() {
            let [c0, g, c2, mut a] = px.to_le_bytes();
            if opaque {
                a = 255;
            } else if let Some(ar) = alpha_row {
                a = ar[x].to_le_bytes()[3];
            }
            let (r, b) = if bgra { (c2, c0) } else { (c0, c2) };
            let pre = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
            line[x * 4..x * 4 + 4].copy_from_slice(&[pre(r), pre(g), pre(b), a]);
        }
    });
}

/// For mixers with no progress of their own (blend modes): from the first picture into their
/// mix by the middle of the transition, then from the mix into the second picture.
fn blend_through_transition(from: &Pixmap, to: &Pixmap, mixed: &mut Pixmap, progress: f32) {
    let p = progress.clamp(0.0, 1.0);
    let (other, t) = if p < 0.5 { (from, 1.0 - p * 2.0) } else { (to, p * 2.0 - 1.0) };
    let t = (t * 256.0).round() as u32;
    mixed.data_mut().par_chunks_mut(4096).zip(other.data().par_chunks(4096)).for_each(|(m, o)| {
        for (m, o) in m.iter_mut().zip(o) {
            *m = ((*m as u32 * (256 - t) + *o as u32 * t + 128) >> 8) as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_like_words() {
        assert_eq!(label("blurAmount"), "Blur amount");
        assert_eq!(label("dot_radius"), "Dot radius");
        assert_eq!(label("Glow"), "Glow");
        assert_eq!(label("1_speed"), "1 speed");
        assert_eq!(label("Neutral Color"), "Neutral Color");
    }

    #[test]
    fn straight_alpha_round_trips() {
        let mut src = Pixmap::new(3, 2).unwrap();
        // Premultiplied: half-transparent red, opaque green, transparent.
        let px = [[128u8, 0, 0, 128], [0, 255, 0, 255], [0, 0, 0, 0], [10, 20, 30, 40], [1, 1, 1, 255], [255, 255, 255, 255]];
        for (i, p) in px.iter().enumerate() {
            src.data_mut()[i * 4..i * 4 + 4].copy_from_slice(p);
        }
        let (pw, ph) = (8, 8);
        let mut frame = Frame::new(pw * ph);
        let buf = frame.pixels(pw * ph);
        assert_eq!(buf.as_ptr() as usize % 16, 0);
        let opaque = to_straight(&src, buf, pw, ph, false);
        assert!(!opaque);
        assert_eq!(buf[0].to_le_bytes(), [255, 0, 0, 128]);
        // Padding repeats the last column and row.
        assert_eq!(buf[7], buf[2]);
        assert_eq!(buf[7 * pw + 1], buf[pw + 1]);
        let mut back = Pixmap::new(3, 2).unwrap();
        from_straight(buf, None, false, &mut back, pw, false);
        for (a, b) in src.data().iter().zip(back.data()) {
            assert!((*a as i32 - *b as i32).abs() <= 1, "{a} vs {b}");
        }
        // BGRA swaps red and blue both ways.
        to_straight(&src, buf, pw, ph, true);
        assert_eq!(buf[0].to_le_bytes(), [0, 0, 255, 128]);
        from_straight(buf, None, false, &mut back, pw, true);
        assert_eq!(&back.data()[0..4], &[128, 0, 0, 128]);
    }

    #[test]
    fn colours_read_from_vectors_and_hex() {
        assert_eq!(as_color(&PluginValue::Text("#ff0080".into())), Some([1.0, 0.0, 128.0 / 255.0, 1.0]));
        assert_eq!(as_color(&PluginValue::Vector(vec![0.1, 0.2, 0.3])), Some([0.1, 0.2, 0.3, 1.0]));
        assert_eq!(as_color(&PluginValue::Text("red".into())), None);
    }

    #[test]
    fn standard_folders_follow_frei0r_path() {
        // FREI0R_PATH replaces the list (spec 1.2); checked without touching the environment.
        let folders = Frei0rHost.standard_folders();
        if std::env::var_os("FREI0R_PATH").is_none() {
            assert!(folders.iter().any(|f| f.ends_with(".frei0r-1/lib")));
        }
    }

    /// A real frei0r library when this computer has the frei0r-plugins package (skipped otherwise).
    fn installed(name: &str) -> Option<PathBuf> {
        ["/usr/lib/frei0r-1", "/usr/lib/x86_64-linux-gnu/frei0r-1", "/usr/local/lib/frei0r-1", "/opt/homebrew/lib/frei0r-1"].iter().map(|d| Path::new(d).join(name)).find(|p| p.is_file())
    }

    #[test]
    fn a_real_frei0r_filter_loads_and_draws() {
        let Some(path) = installed(&format!("invert0r.{}", if cfg!(target_os = "macos") { "dylib" } else { "so" })).or_else(|| installed("invert0r.so")) else {
            eprintln!("frei0r-plugins isn't installed: skipped");
            return;
        };
        let infos = Frei0rHost.describe(&path).unwrap();
        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert_eq!((info.id.as_str(), info.kind, info.format), ("frei0r:invert0r", PluginKind::Effect, Format::Frei0r));
        let mut inst = Frei0rHost.instantiate(info).unwrap();
        let mut white = Pixmap::new(10, 6).unwrap();
        white.fill(tiny_skia::Color::WHITE);
        let mut out = Pixmap::new(10, 6).unwrap();
        let ctx = RenderCtx { time: 0.0, fps: 30.0, scale: 1.0, progress: 0.0, draft: false };
        inst.render(&BTreeMap::new(), &[&white], &mut out, &ctx).unwrap();
        assert_eq!(&out.data()[..4], &[0, 0, 0, 255], "white inverted is black, and opaque");
    }
}
