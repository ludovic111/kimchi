//! Effects: ryolune's stock effects and the CLAP, VST3, Audio Unit and ryolune native plugins
//! found by ryolune's own scan. Its cache (`plugins.json` in ryolune's data folder) is shared:
//! both apps list the same plugins, a scan from either serves both, and kimchi reads the file
//! again whenever ryolune rescans.
//!
//! Scanning loads every plugin bundle, which can crash: like ryolune, each bundle is probed in a
//! child process, kimchi's own executable started with `--scan-plugin <format> <bundle>`
//! ([`scan_child`] answers it; every kimchi binary calls it first thing in `main`).
//!
//! Tests never touch the person's ryolune folder: ryolune's engine sends a test binary's data
//! to a scratch folder (`host::scan::test_sandbox`), and `RYOLUNE_DATA_DIR` moves it anywhere.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use ryolune_engine::host::scan;
use ryolune_engine::plugin::{Descriptor, Format};
use serde::Serialize;

use crate::Result;

/// An effect that can go in a chain.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectInfo {
    /// Descriptor id stored in `Insert::plugin` (`stock:Channel EQ`, `clap:…`, `vst3:…`).
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// `ryolune` (stock), `Native`, `CLAP`, `VST3` or `AU`.
    pub format: String,
    /// Dynamics, EQ & Filter, Distortion, Modulation, Pitch, Space & Time, Utility…
    pub category: String,
    pub description: String,
}

/// One parameter of an effect (plain values: dB, Hz, %, ms).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: String,
    /// 0 for continuous parameters, else the number of steps.
    pub steps: u32,
    pub log: bool,
    pub labels: Vec<String>,
}

impl ParamInfo {
    /// The value as a person reads it ("-6.0 dB", "Hall").
    pub fn text(&self, value: f64) -> String {
        to_ryolune(self).text(value)
    }

    /// A typed value back ("-6 dB", "2.5k", "Hall", "50%"); `None` when it means nothing here.
    pub fn parse(&self, text: &str) -> Option<f64> {
        to_ryolune(self).parse_text(text)
    }

    /// 0..1 along the control's travel (logarithmic for frequencies).
    pub fn normalize(&self, value: f64) -> f64 {
        to_ryolune(self).normalize(value)
    }

    pub fn denormalize(&self, t: f64) -> f64 {
        to_ryolune(self).denormalize(t)
    }
}

fn to_ryolune(p: &ParamInfo) -> ryolune_engine::plugin::ParamInfo {
    ryolune_engine::plugin::ParamInfo {
        id: p.id,
        name: p.name.clone(),
        min: p.min,
        max: p.max,
        default: p.default,
        unit: p.unit.clone(),
        steps: p.steps,
        log: p.log,
        labels: p.labels.clone(),
    }
}

fn from_ryolune(p: ryolune_engine::plugin::ParamInfo) -> ParamInfo {
    ParamInfo { id: p.id, name: p.name, min: p.min, max: p.max, default: p.default, unit: p.unit, steps: p.steps, log: p.log, labels: p.labels }
}

/// The external plugins in ryolune's scan cache, read again when the file changes.
fn external() -> Vec<Descriptor> {
    static CACHE: OnceLock<Mutex<(Option<SystemTime>, Vec<Descriptor>)>> = OnceLock::new();
    let cell = CACHE.get_or_init(|| Mutex::new((None, vec![])));
    let modified = std::fs::metadata(scan::cache_path()).and_then(|m| m.modified()).ok();
    let mut cached = cell.lock().unwrap_or_else(|e| e.into_inner());
    if modified.is_none() {
        cached.1.clear();
    } else if cached.0 != modified {
        *cached = (modified, scan::load_cache().descriptors());
    }
    cached.1.clone()
}

fn info(d: Descriptor) -> EffectInfo {
    let description = match d.format {
        Format::Stock => ryolune_engine::stock::description(&d.name).unwrap_or_default().to_string(),
        _ => String::new(),
    };
    EffectInfo { description, id: d.id, name: d.name, vendor: d.vendor, format: d.format.label().to_string(), category: if d.category.is_empty() { "Plugins".into() } else { d.category } }
}

/// Every effect kimchi can use, stock first, matching `query` (name, vendor, category, format)
/// when given.
pub fn effects(query: Option<&str>) -> Vec<EffectInfo> {
    let q = query.map(str::to_lowercase).filter(|q| !q.trim().is_empty());
    let mut external: Vec<Descriptor> = external().into_iter().filter(|d| d.effect && !d.instrument).collect();
    // Audio Units only load on macOS.
    if !cfg!(target_os = "macos") {
        external.retain(|d| d.format != Format::AudioUnit);
    }
    ryolune_engine::stock::descriptors()
        .into_iter()
        .filter(|d| d.effect && !d.instrument)
        .chain(external)
        .map(info)
        .filter(|e| q.as_ref().is_none_or(|q| [&e.name, &e.vendor, &e.category, &e.format].iter().any(|f| f.to_lowercase().contains(q.as_str()))))
        .collect()
}

/// One effect by id or name (case-insensitive), with a "did you mean" error.
pub fn effect(id_or_name: &str) -> Result<EffectInfo> {
    let all = effects(None);
    if let Some(e) = all.iter().find(|e| e.id == id_or_name || e.name.eq_ignore_ascii_case(id_or_name)) {
        return Ok(e.clone());
    }
    let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
    let hint = kimchi_core::closest(id_or_name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("There is no effect `{id_or_name}`.{hint} audio.effects lists them."))
}

/// An effect's parameters. External plugins are loaded once to ask (then let go, processor
/// first) and remembered.
pub fn params(plugin_id: &str) -> Result<Vec<ParamInfo>> {
    let e = effect(plugin_id)?;
    if let Some(name) = e.id.strip_prefix("stock:") {
        return Ok(ryolune_engine::stock::params(name).into_iter().map(from_ryolune).collect());
    }
    static KNOWN: OnceLock<Mutex<HashMap<String, Vec<ParamInfo>>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(p) = known.lock().unwrap_or_else(|e| e.into_inner()).get(&e.id) {
        return Ok(p.clone());
    }
    let mut instance = ryolune_engine::host::instantiate(&e.id, &e.name, crate::SAMPLE_RATE)?;
    let list: Vec<ParamInfo> = instance.editor.params().iter().cloned().map(from_ryolune).collect();
    drop(instance.processor.take());
    drop(instance);
    known.lock().unwrap_or_else(|e| e.into_inner()).insert(e.id.clone(), list.clone());
    Ok(list)
}

/// A parameter by its id or name (case-insensitive), with a "did you mean" error.
pub fn param(plugin_id: &str, id_or_name: &str) -> Result<ParamInfo> {
    let all = params(plugin_id)?;
    if let Some(p) = all.iter().find(|p| p.id.to_string() == id_or_name || p.name.eq_ignore_ascii_case(id_or_name)) {
        return Ok(p.clone());
    }
    let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
    let hint = kimchi_core::closest(id_or_name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("{plugin_id} has no parameter `{id_or_name}`.{hint} Parameters: {}.", names.join(", ")))
}

/// A new slot for `plugin` (id or name) with its default settings.
pub fn new_insert(plugin: &str, slot_id: impl Into<String>) -> Result<kimchi_core::Insert> {
    let e = effect(plugin)?;
    Ok(kimchi_core::Insert::new(slot_id, &e.id, &e.name))
}

/// Factory presets of a stock effect: (preset name, parameter values by id).
pub fn presets(plugin_id: &str) -> Vec<(String, Vec<(u32, f64)>)> {
    let name = plugin_id.strip_prefix("stock:").unwrap_or(plugin_id);
    ryolune_engine::stock::FACTORY_PRESETS.iter().filter(|(p, _, _)| p.eq_ignore_ascii_case(name)).map(|(_, n, v)| (n.to_string(), v.to_vec())).collect()
}

/// This program answers `--scan-plugin` (see [`scan_child`]), so it can scan.
static CAN_SCAN: AtomicBool = AtomicBool::new(false);

/// The plugin scanner's child mode: when this process was started as
/// `<exe> --scan-plugin <format> <bundle>`, probes that one bundle, prints the result as one
/// JSON line (what ryolune's scanner reads) and returns the exit code. `None` for a normal run.
/// Call it first thing in `main`; it also tells [`rescan`] this program can scan.
pub fn scan_child() -> Option<i32> {
    CAN_SCAN.store(true, Ordering::Relaxed);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--scan-plugin") {
        return None;
    }
    let (Some(format), Some(bundle)) = (args.get(1), args.get(2)) else {
        eprintln!("Usage: --scan-plugin <native|clap|vst3> <bundle>");
        return Some(2);
    };
    let Some((format, _)) = Format::parse(&format!("{format}:x")) else {
        eprintln!("Unknown plugin format {format}");
        return Some(2);
    };
    let result: std::result::Result<Vec<Descriptor>, String> = scan::probe(format, std::path::Path::new(bundle));
    let line = serde_json::to_string(&result).unwrap_or_else(|e| format!("{{\"Err\":\"{e}\"}}"));
    // The scanner reads this from the child's standard output (it's not a log line).
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
    Some(0)
}

/// Looks for plugins again (CLAP, VST3, Audio Units, native) in the standard folders, through
/// ryolune's crash-isolated scanner, and returns how many effects are known afterwards.
pub fn rescan() -> Result<usize> {
    rescan_with(|_| {})
}

/// [`rescan`], telling `progress` the bundle being probed. Bundles are looked for in the
/// standard folders and ryolune's extra folders (Settings › Plugins in ryolune); the shared
/// cache is updated.
pub fn rescan_with(mut progress: impl FnMut(&str)) -> Result<usize> {
    if !CAN_SCAN.load(Ordering::Relaxed) {
        return Err("This program can't scan plugins (scan from the kimchi window, kimchi-cli or ryolune).".into());
    }
    let cache = scan::scan_all(|bundle| {
        tracing::info!(bundle, "scanning plugin");
        progress(bundle);
    });
    for e in cache.entries.iter().filter(|e| e.error.is_some()) {
        tracing::warn!(bundle = %e.path, "plugin not loaded: {}", e.error.as_deref().unwrap_or_default());
    }
    Ok(effects(None).len())
}

/// Bundles the last scan couldn't load, as (path, why).
pub fn scan_problems() -> Vec<(String, String)> {
    scan::load_cache().entries.into_iter().filter_map(|e| Some((e.path, e.error?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_effects_are_listed_with_their_parameters() {
        let all = effects(None);
        assert!(all.len() >= 20, "{all:?}");
        let eq = effect("channel eq").unwrap();
        assert_eq!(eq.id, "stock:Channel EQ");
        assert!(!params(&eq.id).unwrap().is_empty());
        assert!(effect("Chanel EQ").unwrap_err().contains("Channel EQ"));
        assert!(effects(Some("dynamics")).iter().any(|e| e.name == "Limiter"));
        assert!(!presets("stock:ryolune Comp").is_empty());
    }

    #[test]
    fn ryolunes_cache_is_read_and_reread() {
        // A test binary's ryolune data folder is a scratch one (never the person's).
        let path = scan::cache_path();
        assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let cache = |name: &str| {
            serde_json::json!({ "version": 1, "scannedAt": 1, "entries": [{
                "path": "/x/Fancy.clap", "modified": 1, "format": "clap", "error": null,
                "descriptors": [
                    { "id": "clap:com.x.fancy", "format": "clap", "name": name, "vendor": "X", "category": "Space & Time", "effect": true },
                    { "id": "clap:com.x.synth", "format": "clap", "name": "Synth", "vendor": "X", "instrument": true, "effect": false },
                ]
            }]})
        };
        std::fs::write(&path, cache("Fancy Verb").to_string()).unwrap();
        let found = effect("fancy verb").unwrap();
        assert_eq!((found.id.as_str(), found.format.as_str(), found.vendor.as_str()), ("clap:com.x.fancy", "CLAP", "X"));
        assert!(effect("Synth").is_err(), "instruments aren't effects");
        // ryolune rescans: kimchi sees the change (the file's time moves on).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&path, cache("Fancy Hall").to_string()).unwrap();
        assert!(effect("fancy hall").is_ok());
        // Not loadable here: a clear error, nothing kept alive.
        assert!(params("clap:com.x.fancy").is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(effect("fancy hall").is_err());
        // A test binary doesn't answer `--scan-plugin`: no scan, and the shared cache stays as it is.
        assert!(rescan().is_err());
    }
}
