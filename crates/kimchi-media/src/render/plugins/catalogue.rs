//! The video plugins on this computer: kimchi's stock ones (linked in), and what a scan of the
//! plugin folders found.
//!
//! Folders: each format's standard folders ([`super::Host::standard_folders`]), the lsuite folder
//! for kimchi's own plugins, `~/.lsuite/plugins/kimchi` (bundles, one folder each), and the
//! person's extra folders (`settings.plugins.videoFolders`), where every format looks.
//!
//! Scanning loads plugin libraries, which can crash or hang, so each file is described in a
//! child process (`<kimchi> --scan-video-plugin <format> <path>`, answered by [`scan_child`],
//! 30 s at most). Results go to `<data>/plugins/video.json` with each file's time and size: a
//! rescan only describes files that changed, and files that failed are tried again only when
//! asked ([`rescan`] with `retry_failed`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{Format, PluginInfo, hosts, native};

const CACHE_VERSION: u32 = 1;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// Files described at once during a scan.
const PROBES_AT_ONCE: usize = 3;

/// One file the scan looked at, for one format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    path: PathBuf,
    format: Format,
    modified: u64,
    size: u64,
    #[serde(default)]
    plugins: Vec<PluginInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    version: u32,
    scanned_at: u64,
    entries: Vec<Entry>,
}

/// A file that couldn't be loaded.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub path: PathBuf,
    pub format: Format,
    pub error: String,
}

/// A folder plugins are looked for in.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub path: PathBuf,
    /// The format looked for there; `None`: every format (the person's own folders).
    pub format: Option<Format>,
    pub exists: bool,
    /// kimchi's own folder (where to put kimchi plugins).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub own: bool,
}

/// What the plugin folders hold, after a scan or from the cache.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Plugins per format (built-in ones included).
    pub counts: Vec<(Format, usize)>,
    pub failed: Vec<Failure>,
    pub folders: Vec<Folder>,
    /// Seconds since 1970 of the last scan (0: never).
    pub scanned_at: u64,
    /// Files described in this scan (the others were unchanged).
    pub described: usize,
}

struct Config {
    data_dir: PathBuf,
    /// `~/.lsuite` (or `LSUITE_HOME`).
    lsuite_home: PathBuf,
    extra: Vec<PathBuf>,
}

fn config() -> &'static RwLock<Option<Config>> {
    static C: OnceLock<RwLock<Option<Config>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn cache_cell() -> &'static Mutex<Option<Cache>> {
    static C: OnceLock<Mutex<Option<Cache>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where the catalogue keeps its cache (`data_dir`), the lsuite folder (`lsuite_home`, for
/// kimchi's own plugins in `plugins/kimchi`), and the person's extra folders. Called when a
/// session starts and when the folders change.
pub fn configure(data_dir: &Path, lsuite_home: &Path, extra: &[String]) {
    let extra = extra.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    let new = Config { data_dir: data_dir.to_path_buf(), lsuite_home: lsuite_home.to_path_buf(), extra };
    let mut c = config().write().unwrap_or_else(|e| e.into_inner());
    let same = c.as_ref().is_some_and(|old| old.data_dir == new.data_dir && old.lsuite_home == new.lsuite_home && old.extra == new.extra);
    if !same {
        *c = Some(new);
        drop(c);
        *lock(cache_cell()) = None;
    }
}

/// Where installed lsuite plugins for kimchi live: `~/.lsuite/plugins/kimchi` (if configured).
pub fn own_folder() -> Option<PathBuf> {
    config().read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.lsuite_home.join("plugins").join("kimchi"))
}

/// Where the sources of plugins an agent writes go: `~/.lsuite/plugins-src/kimchi`.
pub fn sources_folder() -> Option<PathBuf> {
    config().read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.lsuite_home.join("plugins-src").join("kimchi"))
}

/// Where libraries are copied before they are loaded (hot reload, see `native`).
pub fn stage_folder() -> Option<PathBuf> {
    config().read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.data_dir.join("plugins").join("loaded"))
}

fn cache_path() -> Option<PathBuf> {
    config().read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.data_dir.join("plugins").join("video.json"))
}

fn load_cache() -> Cache {
    cache_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
        .filter(|c| c.version == CACHE_VERSION)
        .unwrap_or_default()
}

fn cached() -> Cache {
    lock(cache_cell()).get_or_insert_with(load_cache).clone()
}

fn store(cache: &Cache) {
    if let Some(path) = cache_path() {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().expect("has a parent"))?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(cache).map_err(std::io::Error::other)?)?;
            std::fs::rename(&tmp, &path)
        };
        if let Err(e) = write() {
            tracing::warn!("couldn't save the video plugin list in {}: {e}", path.display());
        }
    }
    *lock(cache_cell()) = Some(cache.clone());
}

/// Every plugin kimchi can use: the built-in ones, then the scanned ones by format and name. An
/// id found twice is listed once (the first).
pub fn plugins() -> Vec<PluginInfo> {
    let mut all = native::built_ins();
    let mut found: Vec<PluginInfo> = cached().entries.into_iter().flat_map(|e| e.plugins).collect();
    found.sort_by(|a, b| (a.format as u8, a.name.to_lowercase()).cmp(&(b.format as u8, b.name.to_lowercase())));
    all.extend(found);
    let mut seen = HashSet::new();
    all.retain(|p| seen.insert(p.id.clone()));
    all
}

/// A plugin by exact id (`kimchi:…`, `frei0r:…`).
pub fn find(id: &str) -> Option<PluginInfo> {
    plugins().into_iter().find(|p| p.id == id)
}

/// A plugin by id, by its id without the format prefix, or by name (any case); with "did you
/// mean" when nothing matches. A name several formats share must be given as an id.
pub fn lookup(key: &str) -> Result<PluginInfo, String> {
    let all = plugins();
    let k = key.trim();
    if let Some(p) = all.iter().find(|p| p.id == k || p.id.eq_ignore_ascii_case(k)) {
        return Ok(p.clone());
    }
    let short: Vec<&PluginInfo> = all.iter().filter(|p| p.id.split_once(':').is_some_and(|(_, id)| id.eq_ignore_ascii_case(k))).collect();
    let named: Vec<&PluginInfo> = all.iter().filter(|p| p.name.eq_ignore_ascii_case(k)).collect();
    for group in [short, named] {
        match group.as_slice() {
            [one] => return Ok((*one).clone()),
            [_, _, ..] => {
                let ids: Vec<&str> = group.iter().map(|p| p.id.as_str()).collect();
                return Err(format!("More than one plugin is called `{k}`: give its id ({}).", ids.join(", ")));
            }
            [] => {}
        }
    }
    let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).chain(all.iter().map(|p| p.id.as_str())).collect();
    let hint = kimchi_core::closest(k, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("No video plugin `{k}` on this computer.{hint} plugin.list shows them (plugin.rescan looks again)."))
}

/// The folders looked in, by format.
pub fn folders() -> Vec<Folder> {
    let mut out = vec![];
    let own = own_folder();
    let mk = |path: PathBuf, format: Option<Format>, own: bool| Folder { exists: path.is_dir(), path, format, own };
    if let Some(f) = &own {
        out.push(mk(f.clone(), Some(Format::Kimchi), true));
    }
    // Test binaries look only in the folders they are given, never the computer's own.
    for h in hosts().iter().filter(|_| !is_test_binary()) {
        for f in h.standard_folders() {
            out.push(mk(f, Some(h.format()), false));
        }
    }
    let extra = config().read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.extra.clone()).unwrap_or_default();
    for f in extra {
        out.push(mk(f, None, false));
    }
    let mut seen = HashSet::new();
    out.retain(|f| seen.insert((f.path.clone(), f.format)));
    out
}

/// What was found, from the cache (no scan).
pub fn report() -> ScanReport {
    report_of(&cached(), 0)
}

fn report_of(cache: &Cache, described: usize) -> ScanReport {
    let all = plugins();
    let counts = Format::ALL.iter().map(|f| (*f, all.iter().filter(|p| p.format == *f).count())).collect();
    let failed = cache.entries.iter().filter_map(|e| Some(Failure { path: e.path.clone(), format: e.format, error: e.error.clone()? })).collect();
    ScanReport { counts, failed, folders: folders(), scanned_at: cache.scanned_at, described }
}

fn stamp(path: &Path) -> (u64, u64) {
    let one = |p: &Path| {
        let meta = std::fs::metadata(p).ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
        (modified, meta.map_or(0, |m| m.len()))
    };
    // A bundle changes when its manifest or any file in it does (a rebuilt library).
    if path.is_dir() {
        let mut out = one(&path.join(super::bundle::FILE));
        for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
            let (m, n) = one(&e.path());
            out = (out.0.max(m), out.1.wrapping_add(n));
        }
        return out;
    }
    one(path)
}

/// This program answers `--scan-video-plugin`.
static CAN_SCAN: AtomicBool = AtomicBool::new(false);

/// A `cargo test` binary (in `target/<profile>/deps`): it scans in-process, only its own folders.
fn is_test_binary() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| std::env::current_exe().ok().and_then(|e| e.parent().and_then(|p| p.file_name()).map(|n| n == "deps")).unwrap_or(false))
}

/// Looks in the plugin folders again and describes what is new or changed (failed files again
/// too with `retry_failed`). `progress` gets each file as it is described. Needs a program that
/// answers `--scan-video-plugin` (kimchi, kimchi-cli, kimchi-mcp).
pub fn rescan(retry_failed: bool, progress: &(dyn Fn(&Path) + Sync)) -> Result<ScanReport, String> {
    if !CAN_SCAN.load(Ordering::Relaxed) && !is_test_binary() {
        return Err("This program can't scan plugins (scan from the kimchi window, kimchi-cli or kimchi-mcp).".into());
    }
    if cache_path().is_none() {
        return Err("Video plugins aren't set up in this program (no data folder).".into());
    }
    static SCANNING: Mutex<()> = Mutex::new(());
    let _one_at_a_time = lock(&SCANNING);
    let previous: HashMap<(PathBuf, Format), Entry> = load_cache().entries.into_iter().map(|e| ((e.path.clone(), e.format), e)).collect();
    // Every (format, file) to look at.
    let mut todo: Vec<(Format, PathBuf)> = vec![];
    for folder in folders().into_iter().filter(|f| f.exists) {
        for h in hosts().iter().filter(|h| folder.format.is_none_or(|f| f == h.format())) {
            todo.extend(h.find(&folder.path).into_iter().map(|p| (h.format(), p)));
        }
    }
    let mut seen = HashSet::new();
    todo.retain(|k| seen.insert(k.clone()));
    let mut entries = vec![];
    let mut to_probe = vec![];
    for (format, path) in todo {
        let (modified, size) = stamp(&path);
        match previous.get(&(path.clone(), format)) {
            Some(e) if e.modified == modified && e.size == size && (e.error.is_none() || !retry_failed) => entries.push(e.clone()),
            _ => to_probe.push(Entry { path, format, modified, size, plugins: vec![], error: None }),
        }
    }
    let described = to_probe.len();
    let queue = Mutex::new(to_probe);
    let done = Mutex::new(vec![]);
    std::thread::scope(|s| {
        for _ in 0..PROBES_AT_ONCE {
            s.spawn(|| {
                while let Some(mut e) = lock(&queue).pop() {
                    progress(&e.path);
                    match probe(e.format, &e.path) {
                        Ok(p) => e.plugins = p,
                        Err(err) => e.error = Some(err),
                    }
                    lock(&done).push(e);
                }
            });
        }
    });
    entries.extend(done.into_inner().unwrap_or_else(|e| e.into_inner()));
    // A library in a folder every format looks in may be offered to several hosts: when one
    // describes it, the others' refusals aren't failures.
    let described_paths: HashSet<PathBuf> = entries.iter().filter(|e| e.error.is_none() && !e.plugins.is_empty()).map(|e| e.path.clone()).collect();
    entries.retain(|e| e.error.is_none() || !described_paths.contains(&e.path));
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    for e in entries.iter().filter(|e| e.error.is_some()) {
        tracing::warn!(path = %e.path.display(), "video plugin not loaded: {}", e.error.as_deref().unwrap_or_default());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let cache = Cache { version: CACHE_VERSION, scanned_at: now, entries };
    let changed = load_cache().entries != cache.entries;
    store(&cache);
    if changed {
        // New or rebuilt libraries: renderers make their instances again.
        super::bump();
    }
    Ok(report_of(&cache, described))
}

/// [`rescan`] on a thread of its own (new and changed files only), when the window starts.
pub fn scan_in_background() {
    if !CAN_SCAN.load(Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new().name("kimchi-plugin-scan".into()).spawn(|| match rescan(false, &|_| {}) {
        Ok(r) if r.described > 0 => tracing::info!(described = r.described, failed = r.failed.len(), "video plugins scanned"),
        Ok(_) => {}
        Err(e) => tracing::warn!("video plugin scan: {e}"),
    });
}

/// Watches the lsuite plugin folder (`~/.lsuite/plugins/kimchi`) while the app runs: when a bundle
/// is added, removed or rebuilt there (by an agent, a script, another copy of kimchi), rescans and
/// calls `changed`, so the new build is used without a restart. A stat of a few files every two
/// seconds.
pub fn watch(changed: impl Fn() + Send + 'static) {
    if !CAN_SCAN.load(Ordering::Relaxed) {
        return;
    }
    let signature = || -> Vec<(PathBuf, (u64, u64))> {
        let Some(root) = own_folder() else { return vec![] };
        super::bundle::find(&root).into_iter().map(|b| {
            let s = stamp(&b);
            (b, s)
        }).collect()
    };
    let _ = std::thread::Builder::new().name("kimchi-plugin-watch".into()).spawn(move || {
        let mut last = signature();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = signature();
            if now != last {
                last = now;
                match rescan(false, &|_| {}) {
                    Ok(_) => changed(),
                    Err(e) => tracing::warn!("video plugin rescan: {e}"),
                }
            }
        }
    });
}

/// Describes one file or bundle: in a child process when this program can scan, else (tests) in
/// this one.
pub fn probe(format: Format, path: &Path) -> Result<Vec<PluginInfo>, String> {
    if CAN_SCAN.load(Ordering::Relaxed) {
        probe_isolated(format, path)
    } else {
        super::host(format).ok_or_else(|| format!("No {} host.", format.label()))?.describe(path)
    }
}

/// Describes one file in a child process, with a time limit.
fn probe_isolated(format: Format, path: &Path) -> Result<Vec<PluginInfo>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe)
        .arg("--scan-video-plugin")
        .arg(format.prefix())
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start the plugin scan: {e}"))?;
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(s) = stdout.as_mut() {
            use std::io::Read;
            let _ = s.read_to_string(&mut out);
        }
        out
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().unwrap_or_default();
                // Plugins may print while loading: the answer is the last line that parses.
                if let Some(r) = out.lines().rev().find_map(|l| serde_json::from_str::<Result<Vec<PluginInfo>, String>>(l.trim()).ok()) {
                    return r;
                }
                return Err(if status.success() { "The plugin scan gave no answer.".into() } else { format!("It crashed while loading ({status}).") });
            }
            Ok(None) if started.elapsed() > PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("It didn't answer within {} s while loading.", PROBE_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The scanner's child: when this process was started as
/// `<exe> --scan-video-plugin <format> <path>`, describes that file, prints the answer as one
/// JSON line and returns the exit code; `None` for a normal start. Call it first thing in `main`
/// (it also tells [`rescan`] this program can scan).
pub fn scan_child() -> Option<i32> {
    CAN_SCAN.store(true, Ordering::Relaxed);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--scan-video-plugin") {
        return None;
    }
    let (Some(format), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("Usage: --scan-video-plugin <kimchi|frei0r> <path>");
        return Some(2);
    };
    let result: Result<Vec<PluginInfo>, String> = Format::parse(format).and_then(|f| super::host(f).ok_or_else(|| format!("This kimchi has no {} host.", f.label()))).and_then(|h| h.describe(Path::new(path)));
    // The scanner reads this from the child's standard output (it is not a log line).
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::to_string(&result).unwrap_or_else(|e| format!("{{\"Err\":\"{e}\"}}")));
    let _ = out.flush();
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_are_always_there_and_found_by_name() {
        assert!(plugins().iter().any(|p| p.id == "kimchi:xyz.lsuite.kimchi.halftone"));
        assert_eq!(lookup("halftone").unwrap().id, "kimchi:xyz.lsuite.kimchi.halftone");
        assert_eq!(lookup("xyz.lsuite.kimchi.radial-wipe").unwrap().name, "Radial wipe");
        assert!(lookup("Haltone").unwrap_err().contains("Did you mean `Halftone`"));

    }
}
