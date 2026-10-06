//! The OpenFX host: the image effect plugins made for DaVinci Resolve, Natron, VEGAS, Nuke, Fusion
//! and Scratch (Boris FX, Sapphire, Neat Video, openfx-misc, openfx-arena…). Written in Rust
//! against the OpenFX 1.4/1.5 headers and reference docs (github.com/AcademySoftwareFoundation/
//! openfx, read 2026-10-06): no OpenFX support library; the C structs are in [`ffi`].
//!
//! A plugin comes as a bundle, `NAME.ofx.bundle/Contents/<arch>/NAME.ofx` (`Linux-x86-64`,
//! `Linux-aarch64`, `MacOS`, `Win64`), a shared library exporting `OfxGetNumberOfPlugins` and
//! `OfxGetPlugin` (and `OfxSetHost` since 1.5). kimchi gives each plugin its host descriptor
//! and suites ([`suites`], [`props`], [`objects`]): properties, image effects, parameters (C
//! varargs, through Rust's C-variadic functions), memory, multi-thread (real threads), message
//! (to the log), progress and timeline.
//!
//! Each plugin is used in one context: Filter (an effect; or General with a Source clip),
//! Generator (fills its clip), or Transition (`SourceFrom`, `SourceTo` and the `Transition`
//! parameter, which follows the transition's progress). Pictures are given premultiplied, in
//! 32-bit float when the plugin takes it (else 16 or 8 bit), full frame, bottom row first, with
//! the render scale of the preview; field order none; one depth for every clip.
//!
//! Parameters: numbers, integers, toggles, choices (also 1.5's string choices), colours, text and
//! file paths keep their OpenFX names. Positions (2D doubles of the absolute XY type, or
//! OpenFX 1.1's normalised ones) are kimchi points, 0…1 of the picture from the top-left, turned
//! into canonical coordinates for the plugin. Other 2D/3D numbers (scales, sizes) are split into
//! one number per axis, `<name>.x`, `<name>.y`, `<name>.z`. Groups name the section a parameter
//! is shown under; pages, buttons, custom and secret parameters aren't shown. kimchi animates
//! the values: the plugin reads, at any time it asks for, the value at the frame being drawn.
//!
//! Ids are `ofx:<pluginIdentifier>`; when a bundle has several major versions of a plugin the
//! newest is used. Libraries are loaded once and stay loaded.

pub mod ffi;
mod instance;
mod objects;
mod props;
mod suites;

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::PluginValue;
use libloading::Library;

use self::ffi::*;
use self::objects::{Effect, ValueKind, dimensions, value_kind};
use self::props::PropertySet;
use super::{Format, Host, Instance, ParamInfo, ParamKind, PluginInfo, PluginKind};

/// The last frame of every clip's frame range: kimchi doesn't tell a plugin how long its clip is.
pub(crate) const FRAME_RANGE_END: f64 = 1_000_000.0;
/// A clip's region of definition in canonical coordinates (kimchi's own property on clips).
pub(crate) const PROP_KIMCHI_ROD: &str = "xyz.lsuite.kimchi.RegionOfDefinition";
/// The project size canonical defaults are read against before the real one is known.
const REFERENCE_SIZE: (f64, f64) = (1920.0, 1080.0);

/// The architecture folder(s) inside a bundle's `Contents` for this build.
fn arch_folders() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["MacOS", "MacOS-x86-64"]
    } else if cfg!(windows) {
        if cfg!(target_arch = "aarch64") { &["Win-arm64", "Win-arm64ec"] } else { &["Win64"] }
    } else if cfg!(target_arch = "aarch64") {
        &["Linux-aarch64", "Linux-arm64"]
    } else if cfg!(target_arch = "x86") {
        &["Linux-x86"]
    } else {
        &["Linux-x86-64"]
    }
}

/// The folder name of this build's binaries in a bundle (what the tests make).
pub fn arch_folder() -> &'static str {
    arch_folders()[0]
}

/// The `.ofx` binary of a bundle (or `path` itself if it is one).
fn binary_of(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = name.strip_suffix(".bundle").unwrap_or(&name).to_string();
    for arch in arch_folders() {
        let dir = path.join("Contents").join(arch);
        let named = dir.join(&stem);
        if named.is_file() {
            return Ok(named);
        }
        // Some bundles name the binary differently from the folder.
        if let Ok(entries) = std::fs::read_dir(&dir)
            && let Some(any) = entries.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "ofx"))
        {
            return Ok(any);
        }
    }
    Err(format!("{} has no OpenFX plugin for this computer (looked in Contents/{})", path.display(), arch_folders().join(", Contents/")))
}

/// One plugin inside a loaded binary.
pub(crate) struct Plugin {
    ofx: *mut OfxPlugin,
    pub identifier: String,
    pub major: u32,
    pub minor: u32,
    pub file: PathBuf,
    state: Mutex<PluginState>,
    /// Held around renders of plugins that say they aren't thread-safe.
    pub render_lock: Mutex<()>,
}

// SAFETY: the OfxPlugin struct is the plugin's static data; calls into it are serialised by
// `state` (describe actions) or follow the plugin's declared thread safety (renders).
unsafe impl Send for Plugin {}
unsafe impl Sync for Plugin {}

#[derive(Default)]
struct PluginState {
    loaded: Option<Result<(), String>>,
    descriptor: Option<Box<Effect>>,
    contexts: HashMap<String, Box<Effect>>,
}

/// Loaded binaries, by path: their plugins.
fn binaries() -> &'static Mutex<HashMap<PathBuf, Vec<Arc<Plugin>>>> {
    static BINARIES: OnceLock<Mutex<HashMap<PathBuf, Vec<Arc<Plugin>>>>> = OnceLock::new();
    BINARIES.get_or_init(Default::default)
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: OFX strings are 0-terminated.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// Loads (once) the binary of the bundle at `path` and lists its image effect plugins.
fn load(path: &Path) -> Result<Vec<Arc<Plugin>>, String> {
    let binary = binary_of(path)?;
    let key = binary.canonicalize().unwrap_or(binary.clone());
    let mut all = binaries().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(plugins) = all.get(&key) {
        return Ok(plugins.clone());
    }
    let shown = binary.display();
    // SAFETY: loading runs the library's constructors; the scan does it in a child process
    // first so a library that crashes never reaches the app.
    let lib = unsafe { Library::new(&binary) }.map_err(|e| format!("couldn't load {shown}: {e}"))?;
    // SAFETY (symbols): the types are the ones ofxCore.h declares.
    let count_fn = unsafe { lib.get::<GetNumberOfPluginsFn>(b"OfxGetNumberOfPlugins\0") }
        .map(|s| *s)
        .map_err(|_| format!("{shown} isn't an OpenFX plugin (it has no OfxGetNumberOfPlugins)"))?;
    let get_fn = unsafe { lib.get::<GetPluginFn>(b"OfxGetPlugin\0") }
        .map(|s| *s)
        .map_err(|_| format!("{shown} isn't an OpenFX plugin (it has no OfxGetPlugin)"))?;
    let host = suites::host();
    let mut plugins = Vec::new();
    // OpenFX 1.5: OfxSetHost first, when the binary has it; failing means "not for this host".
    let skip = match unsafe { lib.get::<SetHostFn>(b"OfxSetHost\0") } {
        // SAFETY: the host lives for the whole process.
        Ok(set_host) => unsafe { set_host(host) } == STAT_FAILED,
        Err(_) => false,
    };
    if !skip {
        // SAFETY: plain C calls the spec defines.
        let count = unsafe { count_fn() }.max(0);
        for i in 0..count {
            // SAFETY: `i` is below the count.
            let p = unsafe { get_fn(i) };
            if p.is_null() {
                continue;
            }
            // SAFETY: the plugin's static OfxPlugin struct.
            let ofx = unsafe { &*p };
            let api = cstr(ofx.plugin_api);
            let identifier = cstr(ofx.plugin_identifier);
            if api.as_bytes() != IMAGE_EFFECT_PLUGIN_API.to_bytes() || ofx.api_version != 1 {
                tracing::debug!(identifier, api, version = ofx.api_version, "skipping an OpenFX plugin of another API");
                continue;
            }
            if ofx.main_entry.is_none() || ofx.set_host.is_none() {
                continue;
            }
            if let Some(set_host) = ofx.set_host {
                // SAFETY: as above.
                unsafe { set_host(host) };
            }
            plugins.push(Arc::new(Plugin {
                ofx: p,
                identifier,
                major: ofx.plugin_version_major,
                minor: ofx.plugin_version_minor,
                file: path.to_path_buf(),
                state: Mutex::new(PluginState::default()),
                render_lock: Mutex::new(()),
            }));
        }
    }
    // Never unloaded: plugins keep pointers into it.
    std::mem::forget(lib);
    all.insert(key, plugins.clone());
    Ok(plugins)
}

impl Plugin {
    /// Runs one action.
    pub fn call(&self, action: &CStr, handle: *const c_void, in_args: Option<&PropertySet>, out_args: Option<&PropertySet>) -> OfxStatus {
        // SAFETY: the plugin's static struct; checked for a main entry when loaded.
        let Some(main) = (unsafe { &*self.ofx }).main_entry else { return STAT_ERR_FATAL };
        let h = |s: Option<&PropertySet>| s.map_or(std::ptr::null_mut(), |s| s.handle());
        // SAFETY: the action's handle and argument sets are ones kimchi made for it.
        unsafe { main(action.as_ptr(), handle, h(in_args), h(out_args)) }
    }

    fn failed(&self, what: &str, status: OfxStatus, effect: Option<&Effect>) -> String {
        let said = effect.and_then(|e| e.message.lock().unwrap_or_else(|e| e.into_inner()).take());
        let mut text = format!("the OpenFX plugin {} failed to {what} ({})", self.identifier, status_name(status));
        if let Some(said) = said.filter(|s| !s.is_empty()) {
            text.push_str(": ");
            text.push_str(&said);
        }
        text
    }

    /// Loads and describes the plugin (once); runs `f` on its descriptor.
    fn with_descriptor<T>(&self, f: impl FnOnce(&Effect) -> T) -> Result<T, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.loaded.is_none() {
            let status = self.call(ACTION_LOAD, std::ptr::null(), None, None);
            state.loaded = Some(if matches!(status, STAT_OK | STAT_REPLY_DEFAULT) { Ok(()) } else { Err(self.failed("load", status, None)) });
        }
        if let Some(Err(e)) = &state.loaded {
            return Err(e.clone());
        }
        if state.descriptor.is_none() {
            let desc = Effect::new(false);
            objects::effect_props(&desc.props, &self.file.to_string_lossy());
            let status = self.call(ACTION_DESCRIBE, desc.handle(), None, None);
            if !matches!(status, STAT_OK | STAT_REPLY_DEFAULT) {
                return Err(self.failed("describe itself", status, Some(&desc)));
            }
            state.descriptor = Some(desc);
        }
        Ok(f(state.descriptor.as_ref().expect("just set")))
    }

    /// The descriptor for `context` (DescribeInContext, once); runs `f` on it.
    pub fn with_context<T>(&self, context: &str, f: impl FnOnce(&Effect) -> T) -> Result<T, String> {
        self.with_descriptor(|_| ())?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.contexts.contains_key(context) {
            let desc = Effect::new(false);
            desc.props.copy_from(&state.descriptor.as_ref().expect("described").props);
            let args = PropertySet::new();
            args.set_str(prop::CONTEXT, &[context]);
            let status = self.call(ACTION_DESCRIBE_IN_CONTEXT, desc.handle(), Some(&args), None);
            if !matches!(status, STAT_OK | STAT_REPLY_DEFAULT) {
                return Err(self.failed(&format!("describe itself as {}", context_label(context)), status, Some(&desc)));
            }
            state.contexts.insert(context.to_string(), desc);
        }
        Ok(f(&state.contexts[context]))
    }

    /// The context kimchi uses the plugin in, and what that makes it.
    pub fn choose_context(&self) -> Result<(String, PluginKind), String> {
        let contexts = self.with_descriptor(|d| d.props.strings(prop::SUPPORTED_CONTEXTS))?;
        let has = |c: &str| contexts.iter().any(|x| x == c);
        if has(CONTEXT_FILTER) {
            return Ok((CONTEXT_FILTER.into(), PluginKind::Effect));
        }
        if has(CONTEXT_TRANSITION) {
            return Ok((CONTEXT_TRANSITION.into(), PluginKind::Transition));
        }
        if has(CONTEXT_GENERATOR) {
            return Ok((CONTEXT_GENERATOR.into(), PluginKind::Generator));
        }
        if has(CONTEXT_GENERAL) {
            let needs_source = self.with_context(CONTEXT_GENERAL, |d| {
                d.with_clip(CLIP_SOURCE, |c| c.props.int(prop::CLIP_OPTIONAL, 0) != Some(1)).unwrap_or(false)
            })?;
            return Ok((CONTEXT_GENERAL.into(), if needs_source { PluginKind::Effect } else { PluginKind::Generator }));
        }
        Err(format!(
            "the OpenFX plugin {} only works as {}, which kimchi doesn't offer",
            self.identifier,
            contexts.iter().map(|c| context_label(c)).collect::<Vec<_>>().join(" or ")
        ))
    }

    fn info(&self, path: &Path) -> Result<PluginInfo, String> {
        let (context, kind) = self.choose_context()?;
        let (label, description, version_label, grouping, gpu_only) = self.with_descriptor(|d| {
            let p = &d.props;
            (
                p.string(prop::LABEL, 0).unwrap_or_default(),
                p.string(prop::PLUGIN_DESCRIPTION, 0).unwrap_or_default(),
                p.string(prop::VERSION_LABEL, 0).unwrap_or_default(),
                p.string(prop::GROUPING, 0).unwrap_or_default(),
                [prop::OPENGL_RENDER_SUPPORTED, prop::CUDA_RENDER_SUPPORTED, prop::METAL_RENDER_SUPPORTED, prop::OPENCL_RENDER_SUPPORTED]
                    .iter()
                    .any(|g| p.string(g, 0).as_deref() == Some("needed")),
            )
        })?;
        if gpu_only {
            return Err(format!("the OpenFX plugin {} only draws on the GPU (OpenGL, CUDA, Metal or OpenCL), which kimchi doesn't give it", self.identifier));
        }
        let params = self.with_context(&context, |d| bindings(d, &context).into_iter().flat_map(|b| b.infos).collect())?;
        Ok(PluginInfo {
            id: format!("{}:{}", Format::Ofx.prefix(), self.identifier),
            name: if label.trim().is_empty() { self.identifier.clone() } else { label.trim().to_string() },
            vendor: vendor_of(&self.identifier),
            format: Format::Ofx,
            kind,
            category: grouping.trim().to_string(),
            description: description.trim().to_string(),
            version: if version_label.trim().is_empty() { format!("{}.{}", self.major, self.minor) } else { version_label.trim().to_string() },
            path: path.to_path_buf(),
            params,
        })
    }
}

fn context_label(context: &str) -> &str {
    match context {
        CONTEXT_FILTER => "a filter",
        CONTEXT_GENERAL => "a general effect",
        CONTEXT_GENERATOR => "a generator",
        CONTEXT_TRANSITION => "a transition",
        CONTEXT_PAINT => "a paint effect",
        CONTEXT_RETIMER => "a retimer",
        other => other,
    }
}

/// "net.sf.openfx.BlurPlugin" → "openfx", "com.borisfx.Sapphire" → "borisfx".
fn vendor_of(identifier: &str) -> String {
    const SKIP: &[&str] = &["com", "net", "org", "io", "co", "sf", "github", "www", "uk", "fr", "de", "eu", "jp", "us", "ca", "au", "nz", "it", "es", "nl", "se", "ch", "be", "at", "dk", "no", "fi", "pl", "ru", "cn", "kr", "in", "br", "xyz", "tv"];
    let parts: Vec<&str> = identifier.split('.').collect();
    if parts.len() < 2 {
        return String::new();
    }
    parts[..parts.len() - 1].iter().find(|p| !SKIP.contains(&p.to_lowercase().as_str())).map(|p| p.to_string()).unwrap_or_default()
}

/// How one OpenFX parameter is shown in kimchi and fed from its values.
#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub param: String,
    pub shape: Shape,
    /// What kimchi lists (several for split parameters, none for hidden ones).
    pub infos: Vec<ParamInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Shape {
    Number,
    Integer,
    Toggle,
    /// Option labels, by option index.
    Choice(Vec<String>),
    /// Option labels and the values stored for them.
    StrChoice(Vec<String>, Vec<String>),
    /// 3 or 4 components.
    Color(usize),
    /// A position: canonical coordinates, or normalised ones (OpenFX 1.1).
    Point { normalised: bool },
    /// One kimchi number per axis.
    Split { axes: usize, integer: bool },
    Text,
    /// Set by kimchi (the transition's progress) or not shown.
    Hidden,
}

const AXES: [&str; 3] = ["x", "y", "z"];

fn finite(v: f64) -> bool {
    v.is_finite() && v.abs() < 1e30
}

/// A slider span: the display range, else the hard one, else around the default.
fn span(p: &PropertySet, axis: usize, default: f64) -> (f64, f64) {
    let pair = |lo: &str, hi: &str| {
        let (a, b) = (p.double(lo, axis)?, p.double(hi, axis)?);
        (finite(a) && finite(b) && a < b && (a > i32::MIN as f64 || b < i32::MAX as f64)).then_some((a, b))
    };
    let clamp_int = |v: f64| finite(v) && v > i32::MIN as f64 && v < i32::MAX as f64;
    if let Some(r) = pair(prop::PARAM_DISPLAY_MIN, prop::PARAM_DISPLAY_MAX).filter(|(a, b)| clamp_int(*a) && clamp_int(*b)) {
        return r;
    }
    let lo = p.double(prop::PARAM_MIN, axis).filter(|v| clamp_int(*v));
    let hi = p.double(prop::PARAM_MAX, axis).filter(|v| clamp_int(*v));
    let around = default.abs().max(1.0) * 2.0;
    match (lo, hi) {
        (Some(a), Some(b)) if a < b => (a, b),
        (Some(a), None) => (a, a.max(default) + around),
        (None, Some(b)) => (b.min(default) - around, b),
        _ => (default.min(0.0), default.max(0.0) + around),
    }
}

/// Every parameter of a context descriptor, in definition order.
pub(crate) fn bindings(desc: &Effect, context: &str) -> Vec<Binding> {
    let params = desc.params.params.lock().unwrap_or_else(|e| e.into_inner());
    let label_of = |name: &str| params.iter().find(|p| p.name == name).and_then(|p| p.props.string(prop::LABEL, 0)).unwrap_or_default();
    params
        .iter()
        .map(|p| {
            let props = &p.props;
            let kind = p.kind.as_str();
            let label = props.string(prop::LABEL, 0).filter(|l| !l.trim().is_empty()).unwrap_or_else(|| p.name.clone());
            let hint = props.string(prop::PARAM_HINT, 0).unwrap_or_default();
            let group = props.string(prop::PARAM_PARENT, 0).filter(|g| !g.is_empty()).map(|g| label_of(&g)).unwrap_or_default();
            let double_type = props.string(prop::PARAM_DOUBLE_TYPE, 0).unwrap_or_default();
            let normalised_default = props.string(prop::PARAM_DEFAULT_COORDINATE_SYSTEM, 0).as_deref() == Some(COORDINATES_NORMALISED);
            let secret = props.int(prop::PARAM_SECRET, 0) == Some(1);
            let transition = context == CONTEXT_TRANSITION && p.name == PARAM_TRANSITION;
            let info = |name: String, label: String, kind: ParamKind, default: PluginValue, (min, max): (f64, f64), choices: Vec<String>, unit: &str| ParamInfo {
                name,
                label,
                kind,
                default,
                min,
                max,
                choices,
                unit: unit.to_string(),
                group: group.clone(),
                hint: hint.clone(),
            };
            let unit = match double_type.as_str() {
                DOUBLE_TYPE_ANGLE => "°",
                DOUBLE_TYPE_TIME | DOUBLE_TYPE_ABSOLUTE_TIME => "frames",
                DOUBLE_TYPE_X | DOUBLE_TYPE_Y | DOUBLE_TYPE_XY | DOUBLE_TYPE_X_ABSOLUTE | DOUBLE_TYPE_Y_ABSOLUTE => "px",
                _ => "",
            };
            // A spatial default given as a fraction of the project, read against a 1080p one.
            let canonical_default = |d: f64, axis: usize| {
                let is_spatial = matches!(double_type.as_str(), DOUBLE_TYPE_X | DOUBLE_TYPE_Y | DOUBLE_TYPE_XY | DOUBLE_TYPE_X_ABSOLUTE | DOUBLE_TYPE_Y_ABSOLUTE);
                if normalised_default && is_spatial {
                    let y = matches!(double_type.as_str(), DOUBLE_TYPE_Y | DOUBLE_TYPE_Y_ABSOLUTE) || axis == 1;
                    d * if y { REFERENCE_SIZE.1 } else { REFERENCE_SIZE.0 }
                } else {
                    d
                }
            };
            let hidden = secret
                || transition
                || matches!(kind, PARAM_TYPE_GROUP | PARAM_TYPE_PAGE | PARAM_TYPE_PUSH_BUTTON | PARAM_TYPE_CUSTOM | PARAM_TYPE_BYTES | PARAM_TYPE_PARAMETRIC)
                || (kind == PARAM_TYPE_STRING && props.string(prop::PARAM_STRING_MODE, 0).as_deref() == Some(STRING_LABEL));
            let (shape, infos) = if hidden {
                (Shape::Hidden, Vec::new())
            } else {
                match kind {
                    PARAM_TYPE_DOUBLE => {
                        let d = canonical_default(props.double(prop::PARAM_DEFAULT, 0).unwrap_or(0.0), 0);
                        let mut range = span(props, 0, d);
                        if normalised_default && unit == "px" && range.1 <= 1.0 {
                            range = (0.0, REFERENCE_SIZE.0);
                        }
                        (Shape::Number, vec![info(p.name.clone(), label, ParamKind::Number, PluginValue::Number(d), range, vec![], unit)])
                    }
                    PARAM_TYPE_INTEGER => {
                        let d = props.int(prop::PARAM_DEFAULT, 0).unwrap_or(0) as f64;
                        (Shape::Integer, vec![info(p.name.clone(), label, ParamKind::Integer, PluginValue::Number(d), span(props, 0, d), vec![], "")])
                    }
                    PARAM_TYPE_BOOLEAN => {
                        let d = props.int(prop::PARAM_DEFAULT, 0).unwrap_or(0) != 0;
                        (Shape::Toggle, vec![info(p.name.clone(), label, ParamKind::Toggle, PluginValue::Bool(d), (0.0, 1.0), vec![], "")])
                    }
                    PARAM_TYPE_CHOICE | PARAM_TYPE_STR_CHOICE => {
                        let options = props.strings(prop::PARAM_CHOICE_OPTION);
                        let order = props.ints(prop::PARAM_CHOICE_ORDER);
                        let mut shown: Vec<(i32, usize)> = (0..options.len()).map(|i| (order.get(i).copied().unwrap_or(i as i32), i)).collect();
                        shown.sort();
                        let choices: Vec<String> = shown.iter().map(|(_, i)| options[*i].clone()).collect();
                        let (shape, default) = if kind == PARAM_TYPE_CHOICE {
                            let d = props.int(prop::PARAM_DEFAULT, 0).unwrap_or(0).max(0) as usize;
                            (Shape::Choice(options.clone()), options.get(d).cloned().unwrap_or_default())
                        } else {
                            let enums = props.strings(prop::PARAM_CHOICE_ENUM);
                            let d = props.string(prop::PARAM_DEFAULT, 0).unwrap_or_default();
                            let label = enums.iter().position(|e| *e == d).and_then(|i| options.get(i).cloned()).unwrap_or(d);
                            (Shape::StrChoice(options.clone(), enums), label)
                        };
                        (shape, vec![info(p.name.clone(), label, ParamKind::Choice, PluginValue::Text(default), (0.0, 0.0), choices, "")])
                    }
                    PARAM_TYPE_RGB | PARAM_TYPE_RGBA => {
                        let n = dimensions(kind);
                        let mut d = props.doubles(prop::PARAM_DEFAULT);
                        d.resize(4, 1.0);
                        if n == 3 {
                            d[3] = 1.0;
                        }
                        (Shape::Color(n), vec![info(p.name.clone(), label, ParamKind::Color, PluginValue::Vector(d), (0.0, 1.0), vec![], "")])
                    }
                    PARAM_TYPE_DOUBLE_2D
                        if matches!(double_type.as_str(), DOUBLE_TYPE_XY_ABSOLUTE | DOUBLE_TYPE_NORMALISED_XY_ABSOLUTE) =>
                    {
                        let normalised = double_type == DOUBLE_TYPE_NORMALISED_XY_ABSOLUTE;
                        let mut d = props.doubles(prop::PARAM_DEFAULT);
                        d.resize(2, 0.0);
                        let (x, y) = if normalised || normalised_default { (d[0], d[1]) } else { (d[0] / REFERENCE_SIZE.0, d[1] / REFERENCE_SIZE.1) };
                        let default = PluginValue::Vector(vec![x, 1.0 - y]);
                        (Shape::Point { normalised }, vec![info(p.name.clone(), label, ParamKind::Point, default, (0.0, 1.0), vec![], "")])
                    }
                    PARAM_TYPE_DOUBLE_2D | PARAM_TYPE_DOUBLE_3D | PARAM_TYPE_INTEGER_2D | PARAM_TYPE_INTEGER_3D => {
                        let axes = dimensions(kind);
                        let integer = value_kind(kind) == ValueKind::Int;
                        let labels = props.strings(prop::PARAM_DIMENSION_LABEL);
                        let infos = (0..axes)
                            .map(|a| {
                                let d = canonical_default(props.double(prop::PARAM_DEFAULT, a).unwrap_or(0.0), a);
                                let axis = labels.get(a).filter(|l| !l.is_empty()).cloned().unwrap_or_else(|| AXES[a].to_string());
                                info(
                                    format!("{}.{}", p.name, AXES[a]),
                                    format!("{label} {axis}"),
                                    if integer { ParamKind::Integer } else { ParamKind::Number },
                                    PluginValue::Number(d),
                                    span(props, a, d),
                                    vec![],
                                    unit,
                                )
                            })
                            .collect();
                        (Shape::Split { axes, integer }, infos)
                    }
                    PARAM_TYPE_STRING => {
                        let mode = props.string(prop::PARAM_STRING_MODE, 0).unwrap_or_default();
                        let file = matches!(mode.as_str(), STRING_FILE_PATH | STRING_DIRECTORY_PATH);
                        let d = props.string(prop::PARAM_DEFAULT, 0).unwrap_or_default();
                        (Shape::Text, vec![info(p.name.clone(), label, if file { ParamKind::File } else { ParamKind::Text }, PluginValue::Text(d), (0.0, 0.0), vec![], "")])
                    }
                    _ => (Shape::Hidden, Vec::new()),
                }
            };
            Binding { param: p.name.clone(), shape, infos }
        })
        .collect()
}

/// The OpenFX host.
#[derive(Debug, Default, Clone, Copy)]
pub struct OfxHost;

impl OfxHost {
    pub fn new() -> Self {
        Self
    }
}

impl Host for OfxHost {
    fn format(&self) -> Format {
        Format::Ofx
    }

    fn standard_folders(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = std::env::var_os("OFX_PLUGIN_PATH")
            .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
            .unwrap_or_default();
        if cfg!(windows) {
            for var in ["CommonProgramFiles", "CommonProgramW6432"] {
                if let Some(dir) = std::env::var_os(var) {
                    folders.push(PathBuf::from(dir).join("OFX").join("Plugins"));
                }
            }
            folders.push(PathBuf::from(r"C:\Program Files\Common Files\OFX\Plugins"));
        } else if cfg!(target_os = "macos") {
            folders.push("/Library/OFX/Plugins".into());
        } else {
            folders.push("/usr/OFX/Plugins".into());
        }
        let mut seen = std::collections::HashSet::new();
        folders.retain(|f| seen.insert(f.clone()));
        folders
    }

    fn find(&self, folder: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        walk(folder, 0, &mut found);
        found.sort();
        found
    }

    fn describe(&self, path: &Path) -> Result<Vec<PluginInfo>, String> {
        let plugins = load(path)?;
        let mut newest: HashMap<String, (u32, PluginInfo)> = HashMap::new();
        let mut errors = Vec::new();
        for p in &plugins {
            if newest.get(&p.identifier).is_some_and(|(major, _)| *major >= p.major) {
                continue;
            }
            match p.info(path) {
                Ok(info) => {
                    newest.insert(p.identifier.clone(), (p.major, info));
                }
                Err(e) => {
                    tracing::info!("{e}");
                    errors.push(e);
                }
            }
        }
        if newest.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let mut infos: Vec<PluginInfo> = newest.into_values().map(|(_, i)| i).collect();
        infos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(infos)
    }

    fn instantiate(&self, info: &PluginInfo) -> Result<Box<dyn Instance>, String> {
        let identifier = info.id.strip_prefix("ofx:").unwrap_or(&info.id);
        let plugins = load(&info.path)?;
        let plugin = plugins
            .iter()
            .filter(|p| p.identifier == identifier)
            .max_by_key(|p| p.major)
            .cloned()
            .ok_or_else(|| format!("{} has no OpenFX plugin {identifier} any more", info.path.display()))?;
        Ok(Box::new(instance::OfxInstance::new(plugin, info.name.clone())?))
    }
}

/// Bundles (`*.ofx.bundle`) in `dir` and below, skipping names starting with `@` (the spec).
fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('@') || name.starts_with('.') || !path.is_dir() {
            continue;
        }
        if name.to_lowercase().ends_with(".ofx.bundle") {
            found.push(path);
        } else if depth < 4 {
            walk(&path, depth + 1, found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendors_come_from_identifiers() {
        assert_eq!(vendor_of("net.sf.openfx.BlurPlugin"), "openfx");
        assert_eq!(vendor_of("com.borisfx.Sapphire.Glow"), "borisfx");
        assert_eq!(vendor_of("fr.inria.openfx.Shadertoy"), "inria");
        assert_eq!(vendor_of("Blur"), "");
    }

    #[test]
    fn standard_folders_include_the_system_one() {
        let f = OfxHost.standard_folders();
        assert!(f.iter().any(|p| p.ends_with("Plugins")));
    }
}
