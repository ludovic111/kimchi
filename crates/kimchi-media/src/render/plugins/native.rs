//! kimchi's own plugins: libraries built with the `kimchi-plugin` SDK, loaded through its C ABI
//! (`kimchi_plugin::ffi`, ABI 1), and the built-in plugins linked into kimchi
//! (`kimchi-plugin-examples`), which go through the very same vtables.
//!
//! A library is loaded once and stays loaded for the life of the process, so its vtables are
//! `'static`. Each instance is a `kimchi_plugin::ffi::Handle`, made at the first frame it draws
//! (it is told that frame's size). Parameter values are sent when they change, and frames are lent
//! to the plugin for the call: kimchi's pixmaps are already premultiplied RGBA8, the SDK's format.

use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::PluginValue;
use kimchi_plugin::ffi::{self, Handle, Manifest, ManifestParam, PluginVTable, RawFrame, RawHost, RawRenderCtx, RawSetup, RawValue};
use rayon::prelude::*;
use tiny_skia::Pixmap;

use super::{Format, Host, Instance, ParamInfo, ParamKind, PluginInfo, PluginKind, RenderCtx, value};

/// The host of kimchi's own plugins.
pub struct NativeHost;

/// The platform's extension for libraries.
pub fn library_extension() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(windows) {
        "dll"
    } else {
        "so"
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

impl Host for NativeHost {
    fn format(&self) -> Format {
        Format::Kimchi
    }

    /// `KIMCHI_PLUGIN_PATH`, then the system's places for kimchi plugins. kimchi's own folder in
    /// its data folder (`<data>/plugins/video`) is added by the catalogue.
    fn standard_folders(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::env::var_os("KIMCHI_PLUGIN_PATH").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
        let home = home();
        if cfg!(target_os = "macos") {
            if let Some(h) = &home {
                dirs.push(h.join("Library/Application Support/kimchi/Plugins"));
            }
            dirs.push("/Library/Application Support/kimchi/Plugins".into());
        } else if cfg!(windows) {
            if let Some(p) = std::env::var_os("COMMONPROGRAMFILES") {
                dirs.push(PathBuf::from(p).join("kimchi").join("Plugins"));
            }
        } else {
            if let Some(h) = &home {
                dirs.push(h.join(".local/lib/kimchi/plugins"));
            }
            dirs.push("/usr/local/lib/kimchi/plugins".into());
            dirs.push("/usr/lib/kimchi/plugins".into());
        }
        dirs.retain(|d| !d.as_os_str().is_empty());
        dirs
    }

    fn find(&self, folder: &Path) -> Vec<PathBuf> {
        let mut found = vec![];
        walk(folder, 0, &mut found);
        found.sort();
        found
    }

    fn describe(&self, path: &Path) -> Result<Vec<PluginInfo>, String> {
        let lib = load(path)?;
        Ok(lib.plugins.iter().map(|(m, _)| info(m, path)).collect())
    }

    fn instantiate(&self, info: &PluginInfo) -> Result<Box<dyn Instance>, String> {
        let lib = load(&info.path)?;
        let own = own_id(info)?;
        let (m, table) = lib.plugins.iter().find(|(m, _)| m.id == own).ok_or_else(|| format!("{} no longer has the plugin {own} (rescan plugins).", info.path.display()))?;
        Ok(Box::new(NativeInstance::new(m.clone(), table)))
    }
}

/// Libraries in `dir` and two levels of subfolders.
fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 2 {
                walk(&p, depth + 1, found);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case(library_extension())) {
            found.push(p);
        }
    }
}

/// The plugin's own id (after `kimchi:`).
fn own_id(info: &PluginInfo) -> Result<&str, String> {
    info.id.strip_prefix("kimchi:").ok_or_else(|| format!("`{}` isn't a kimchi plugin id (kimchi:<id>).", info.id))
}

/// A loaded library's plugins.
struct Library {
    _lib: Option<libloading::Library>,
    plugins: Vec<(Manifest, &'static PluginVTable)>,
}

fn loaded() -> &'static Mutex<HashMap<PathBuf, Arc<Library>>> {
    static L: OnceLock<Mutex<HashMap<PathBuf, Arc<Library>>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// The plugins of an entry, with their manifests.
///
/// # Safety
/// `entry` comes from a `kimchi_plugin_entry` of a library that stays loaded.
unsafe fn plugins_of(entry: *const ffi::Entry) -> Result<Vec<(Manifest, &'static PluginVTable)>, String> {
    // SAFETY: as the caller promises.
    let tables = unsafe { ffi::tables(entry) }?;
    tables.into_iter().enumerate().map(|(i, t)| ffi::read_manifest(t).map(|m| (m, t)).map_err(|e| format!("Plugin {i}: {e}"))).collect()
}

fn load(path: &Path) -> Result<Arc<Library>, String> {
    let mut map = loaded().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(l) = map.get(path) {
        return Ok(l.clone());
    }
    // SAFETY: loading a plugin runs its initialisers: that is what hosting means. Scans do it in a
    // child process first, so a library that crashes here was found out before.
    let lib = unsafe { libloading::Library::new(path) }.map_err(|e| format!("Couldn't load {}: {e}", path.display()))?;
    let symbol = format!("{}\0", kimchi_plugin::ENTRY_SYMBOL);
    // SAFETY: the SDK's entry type; the library stays loaded (kept in the map below).
    let entry: ffi::EntryFn = unsafe { lib.get::<ffi::EntryFn>(symbol.as_bytes()) }.map(|s| *s).map_err(|_| format!("{} isn't a kimchi plugin: it has no {} (build it with the kimchi-plugin SDK's export_plugins!).", path.display(), kimchi_plugin::ENTRY_SYMBOL))?;
    // SAFETY: as above.
    let plugins = unsafe { plugins_of(entry()) }.map_err(|e| format!("{}: {e}", path.display()))?;
    let l = Arc::new(Library { _lib: Some(lib), plugins });
    map.insert(path.to_path_buf(), l.clone());
    Ok(l)
}

/// The built-in plugins, read through their entry like any library's.
fn built_in() -> &'static Result<Library, String> {
    static B: OnceLock<Result<Library, String>> = OnceLock::new();
    // SAFETY: linked into this program, so it stays loaded.
    B.get_or_init(|| unsafe { plugins_of(kimchi_plugin_examples::entry()) }.map(|plugins| Library { _lib: None, plugins }))
}

/// kimchi's built-in plugins.
pub fn built_ins() -> Vec<PluginInfo> {
    match built_in() {
        Ok(l) => l.plugins.iter().map(|(m, _)| info(m, Path::new(super::BUILT_IN))).collect(),
        Err(e) => {
            tracing::error!("built-in video plugins: {e}");
            vec![]
        }
    }
}

pub(super) fn built_in_instance(info: &PluginInfo) -> Result<Box<dyn Instance>, String> {
    let own = own_id(info)?;
    let lib = built_in().as_ref().map_err(Clone::clone)?;
    let (m, table) = lib.plugins.iter().find(|(m, _)| m.id == own).ok_or_else(|| format!("kimchi has no built-in plugin {own}."))?;
    Ok(Box::new(NativeInstance::new(m.clone(), table)))
}

/// A manifest as the catalogue lists it.
fn info(m: &Manifest, path: &Path) -> PluginInfo {
    PluginInfo {
        id: format!("kimchi:{}", m.id),
        name: m.name.clone(),
        vendor: m.vendor.clone(),
        format: Format::Kimchi,
        kind: match m.kind {
            kimchi_plugin::Kind::Effect => PluginKind::Effect,
            kimchi_plugin::Kind::Generator => PluginKind::Generator,
            kimchi_plugin::Kind::Transition => PluginKind::Transition,
        },
        category: m.category.clone(),
        description: m.description.clone(),
        version: m.version.clone(),
        path: path.to_path_buf(),
        params: m.params.iter().map(param_info).collect(),
        timeless: !m.animated,
    }
}

fn param_info(p: &ManifestParam) -> ParamInfo {
    use kimchi_plugin::ParamKind as K;
    let kind = match p.kind {
        K::Number | K::Angle => ParamKind::Number,
        K::Integer => ParamKind::Integer,
        K::Toggle => ParamKind::Toggle,
        K::Choice => ParamKind::Choice,
        K::Color => ParamKind::Color,
        K::Point => ParamKind::Point,
        K::Text => ParamKind::Text,
        K::File => ParamKind::File,
    };
    let d = &p.default;
    let default = match kind {
        ParamKind::Toggle => PluginValue::Bool(d.as_bool().unwrap_or(false)),
        ParamKind::Choice => PluginValue::Text(d.as_f64().and_then(|i| p.choices.get(i.max(0.0) as usize)).cloned().unwrap_or_default()),
        ParamKind::Color | ParamKind::Point => PluginValue::Vector(d.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default()),
        ParamKind::Text | ParamKind::File => PluginValue::Text(d.as_str().unwrap_or("").into()),
        ParamKind::Number | ParamKind::Integer => PluginValue::Number(d.as_f64().unwrap_or(0.0)),
    };
    ParamInfo {
        name: p.name.clone(),
        label: if p.label.is_empty() { p.name.clone() } else { p.label.clone() },
        kind,
        default,
        min: p.min,
        max: p.max,
        choices: p.choices.clone(),
        unit: p.unit.clone(),
        group: p.group.clone(),
        hint: p.hint.clone(),
        extensions: p.extensions.clone(),
    }
}

/// kimchi's services for plugins: its thread pool (rayon) and its log (tracing).
static HOST: RawHost = RawHost { size: size_of::<RawHost>() as u32, reserved: 0, parallel: host_parallel, log: host_log };

unsafe extern "C" fn host_parallel(_host: *const RawHost, count: u32, job: ffi::Job, data: *mut c_void) {
    let data = data as usize;
    // SAFETY: the SDK's job with its data, valid until this returns; it catches its own panics.
    (0..count).into_par_iter().for_each(|i| unsafe { job(data as *mut c_void, i) });
}

unsafe extern "C" fn host_log(_host: *const RawHost, level: u32, message: *const u8, len: usize) {
    if message.is_null() {
        return;
    }
    // SAFETY: `len` bytes from the plugin, for this call.
    let text = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message, len) });
    match level {
        0 => tracing::error!(target: "kimchi::plugin", "{text}"),
        1 => tracing::warn!(target: "kimchi::plugin", "{text}"),
        2 => tracing::info!(target: "kimchi::plugin", "{text}"),
        _ => tracing::debug!(target: "kimchi::plugin", "{text}"),
    }
}

/// One slot's instance of a kimchi plugin.
pub struct NativeInstance {
    manifest: Manifest,
    table: &'static PluginVTable,
    infos: Vec<ParamInfo>,
    handle: Option<Handle>,
    /// What each parameter was last set to.
    sent: Vec<Option<PluginValue>>,
}

impl NativeInstance {
    fn new(manifest: Manifest, table: &'static PluginVTable) -> Self {
        let infos = manifest.params.iter().map(param_info).collect::<Vec<_>>();
        let sent = vec![None; infos.len()];
        Self { manifest, table, infos, handle: None, sent }
    }

    fn send(&mut self, params: &BTreeMap<String, PluginValue>) -> Result<(), String> {
        let handle = self.handle.as_mut().expect("made before");
        for (i, p) in self.infos.iter().enumerate() {
            let v = params.get(&p.name).map(|v| value::normalize(p, v)).unwrap_or_else(|| value::default_of(p));
            if self.sent[i].as_ref() == Some(&v) {
                continue;
            }
            let kind = self.manifest.params[i].kind;
            let raw = raw_value(p, kind, &v);
            handle.set_param(i as u32, &raw).map_err(|e| format!("{}: {e}", p.name))?;
            self.sent[i] = Some(v);
        }
        Ok(())
    }
}

/// A stored value as it crosses the ABI (borrows text from `v`).
fn raw_value(p: &ParamInfo, kind: kimchi_plugin::ParamKind, v: &PluginValue) -> RawValue {
    use kimchi_plugin::ParamKind as K;
    let n = v.as_f64().unwrap_or(0.0);
    let vec = |len: usize, fill: f64| -> Vec<f64> {
        match v {
            PluginValue::Vector(c) => c.iter().copied().chain(std::iter::repeat(fill)).take(len).collect(),
            _ => vec![fill; len],
        }
    };
    match kind {
        K::Number | K::Angle => RawValue::number(n),
        K::Integer => RawValue::integer(n.round() as i64),
        K::Toggle => RawValue::toggle(matches!(v, PluginValue::Bool(true)) || n >= 0.5),
        K::Choice => RawValue::choice(value::choice_index(p, v).unwrap_or(0)),
        K::Color => {
            let c = vec(4, 1.0);
            RawValue::color([c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32])
        }
        K::Point => {
            let c = vec(2, 0.5);
            RawValue::point([c[0] as f32, c[1] as f32])
        }
        K::Text | K::File => match v {
            PluginValue::Text(t) => RawValue::text(t),
            _ => RawValue::text(""),
        },
    }
}

fn raw(p: &Pixmap) -> RawFrame {
    RawFrame { data: p.data().as_ptr() as *mut u8, width: p.width(), height: p.height(), stride: p.width() * 4, format: ffi::FORMAT_RGBA8_PREMULTIPLIED }
}

impl Instance for NativeInstance {
    fn render(&mut self, params: &BTreeMap<String, PluginValue>, inputs: &[&Pixmap], output: &mut Pixmap, ctx: &RenderCtx) -> Result<(), String> {
        if self.handle.is_none() {
            self.handle = Some(Handle::new(self.table, &RawSetup::new(output.width(), output.height(), ctx.fps))?);
            self.sent.iter_mut().for_each(|s| *s = None);
        }
        self.send(params)?;
        let frames: Vec<RawFrame> = inputs.iter().map(|p| raw(p)).collect();
        let out = RawFrame { data: output.data_mut().as_mut_ptr(), ..raw(output) };
        let rc = RawRenderCtx::new(ctx.time, ctx.fps, ctx.scale, ctx.progress, ctx.draft, &HOST);
        self.handle.as_mut().expect("made above").render(&frames, &out, &rc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_are_read_through_the_abi() {
        let all = built_ins();
        let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
        for want in ["Glow", "Pixelate", "RGB split", "Film grain", "Duotone", "Gradient", "Checker", "Noise", "Radial wipe"] {
            assert!(names.contains(&want), "{want} in {names:?}");
        }
        let grain = all.iter().find(|p| p.name == "Film grain").unwrap();
        assert!(!grain.timeless && all.iter().find(|p| p.name == "Glow").unwrap().timeless);
        assert_eq!(all.iter().find(|p| p.name == "Radial wipe").unwrap().kind, PluginKind::Transition);
        let shape = all.iter().find(|p| p.name == "Pixelate").unwrap().param("Shape").unwrap();
        assert_eq!((shape.kind, &shape.default), (ParamKind::Choice, &PluginValue::Text("Squares".into())));
        let angle = all.iter().find(|p| p.name == "RGB split").unwrap().param("Angle").unwrap();
        assert_eq!((angle.kind, angle.unit.as_str()), (ParamKind::Number, "°"));
    }

    #[test]
    fn an_instance_draws_with_its_values() {
        let info = built_ins().into_iter().find(|p| p.name == "Duotone").unwrap();
        let mut inst = built_in_instance(&info).unwrap();
        let mut white = Pixmap::new(8, 8).unwrap();
        white.fill(tiny_skia::Color::WHITE);
        let mut out = Pixmap::new(8, 8).unwrap();
        let params = BTreeMap::from([("Highlights".to_string(), PluginValue::Text("#00ff00".into()))]);
        let ctx = RenderCtx { time: 0.0, fps: 30.0, scale: 1.0, progress: 0.0, draft: false };
        inst.render(&params, &[&white], &mut out, &ctx).unwrap();
        assert_eq!(&out.data()[..4], &[0, 255, 0, 255]);
        // A frame of the wrong size is refused, not drawn out of bounds.
        let mut small = Pixmap::new(4, 4).unwrap();
        assert!(inst.render(&params, &[&white], &mut small, &ctx).unwrap_err().contains("differ in size"));
    }
}
