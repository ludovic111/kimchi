//! Logs and crash reports, kept in `<data>/logs`:
//!
//! * `kimchi.log` is this run's log, `kimchi.1.log` the previous run's, up to `kimchi.3.log`
//!   (a file that passes [`MAX_LOG`] while running rolls over too). `kimchi-mcp.log` likewise.
//! * `crashes/crash-<time>-<pid>.txt`: written by the panic hook, with the message, where it
//!   happened, a backtrace, the system and the last lines of the log.
//! * `running-<pid>.json`: present while a window process runs and removed when it quits. One
//!   left behind by a process that is gone means it didn't quit properly (a native crash, a
//!   forced quit, the computer stopping): [`init`] then writes a report from its log, so
//!   there is one either way, and the window says so once ([`unseen_reports`]).
//!
//! The level comes from `RUST_LOG` when set, else from `diagnostics.logLevel` and changes
//! live ([`set_level`]). Nothing here leaves the computer: reports are files the person can
//! read, copy or attach to an issue themselves.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

/// A log file rolls over past this size.
pub const MAX_LOG: u64 = 16 << 20;
/// Older logs kept: `<name>.1.log` … `<name>.3.log`.
const KEEP_LOGS: usize = 3;
/// Crash reports kept.
const KEEP_REPORTS: usize = 25;
/// Log lines kept in memory for crash reports.
const RECENT_LINES: usize = 400;
/// The log levels `diagnostics.logLevel` takes.
pub const LEVELS: &[&str] = &["info", "debug", "trace"];

static STATE: OnceLock<State> = OnceLock::new();

struct State {
    dir: PathBuf,
    name: String,
    reload: reload::Handle<EnvFilter, tracing_subscriber::Registry>,
    from_env: bool,
}

/// The last lines written to the log, for crash reports.
static RECENT: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

pub fn logs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

pub fn crashes_dir(data_dir: &Path) -> PathBuf {
    logs_dir(data_dir).join("crashes")
}

/// What [`init`] found about the previous run.
#[derive(Debug, Clone, Default)]
pub struct Started {
    /// Reports written for runs that ended without quitting (see the module docs).
    pub unclean: Vec<PathBuf>,
}

/// Starts logging to `<data>/logs/<name>.log` (and stderr), and installs the panic hook.
/// `window` marks this process as the app, so an unclean end is noticed next time. Call it
/// once, first thing; later calls do nothing.
pub fn init(data_dir: &Path, name: &str, level: &str, window: bool) -> Started {
    let dir = logs_dir(data_dir);
    let _ = fs::create_dir_all(dir.join("crashes"));
    let mut started = Started::default();
    if window {
        started.unclean = collect_unclean(data_dir, name);
    }
    rotate(&dir, name);
    let from_env = std::env::var("RUST_LOG").is_ok_and(|v| !v.trim().is_empty());
    let filter = if from_env { EnvFilter::from_default_env() } else { filter_for(level) };
    let (filter, handle) = reload::Layer::new(filter);
    let file = RollingFile::open(&dir, name);
    let registry = tracing_subscriber::registry().with(filter);
    let stderr = tracing_subscriber::fmt::layer().with_writer(io::stderr);
    let result = match file {
        Some(file) => registry.with(stderr).with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(file)).try_init(),
        None => registry.with(stderr).try_init(),
    };
    if result.is_err() {
        return started;
    }
    let _ = STATE.set(State { dir: dir.clone(), name: name.to_string(), reload: handle, from_env });
    install_panic_hook(data_dir.to_path_buf());
    if window {
        let marker = Marker { pid: std::process::id(), version: env!("CARGO_PKG_VERSION").into(), started: chrono::Utc::now(), name: name.into() };
        if let Ok(json) = serde_json::to_vec_pretty(&marker) {
            let _ = fs::write(marker_path(&dir, std::process::id()), json);
        }
    }
    tracing::info!("{name} {} on {} {} ({})", env!("CARGO_PKG_VERSION"), os_name(), std::env::consts::ARCH, std::env::consts::FAMILY);
    for r in &started.unclean {
        tracing::warn!("the previous run didn't quit properly; report: {}", r.display());
    }
    started
}

/// Notes a clean quit (removes this process's running marker).
pub fn clean_exit() {
    if let Some(st) = STATE.get() {
        let _ = fs::remove_file(marker_path(&st.dir, std::process::id()));
        tracing::info!("{} quit", st.name);
    }
}

/// Applies `diagnostics.logLevel` (`info`, `debug` or `trace`); `RUST_LOG` wins when set.
pub fn set_level(level: &str) {
    if let Some(st) = STATE.get().filter(|st| !st.from_env) {
        let _ = st.reload.reload(filter_for(level));
    }
}

fn filter_for(level: &str) -> EnvFilter {
    let level = if LEVELS.contains(&level) { level } else { "debug" };
    // `kimchi` covers every kimchi crate (targets match by prefix); the GPU stack is chatty.
    EnvFilter::new(format!("info,kimchi={level},wgpu_core=warn,wgpu_hal=warn,naga=warn,cosmic_text=warn"))
}

// ---- files ---------------------------------------------------------------------

fn log_path(dir: &Path, name: &str, n: usize) -> PathBuf {
    if n == 0 { dir.join(format!("{name}.log")) } else { dir.join(format!("{name}.{n}.log")) }
}

/// `name.log` → `name.1.log` → … → dropped after [`KEEP_LOGS`].
fn rotate(dir: &Path, name: &str) {
    let _ = fs::remove_file(log_path(dir, name, KEEP_LOGS));
    for n in (0..KEEP_LOGS).rev() {
        let from = log_path(dir, name, n);
        if from.exists() {
            let _ = fs::rename(&from, log_path(dir, name, n + 1));
        }
    }
}

/// The log file: appends, rolls over past [`MAX_LOG`], and keeps the recent lines.
struct RollingFile {
    dir: PathBuf,
    name: String,
    file: Mutex<(File, u64)>,
}

impl RollingFile {
    fn open(dir: &Path, name: &str) -> Option<&'static Self> {
        let path = log_path(dir, name, 0);
        let file = OpenOptions::new().create(true).append(true).open(&path).ok()?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        // Lives as long as the process, like the subscriber that writes to it.
        Some(Box::leak(Box::new(Self { dir: dir.to_path_buf(), name: name.to_string(), file: Mutex::new((file, len)) })))
    }
}

struct LogWriter(&'static RollingFile);

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        remember(buf);
        let mut f = self.0.file.lock();
        if f.1 + buf.len() as u64 > MAX_LOG {
            rotate(&self.0.dir, &self.0.name);
            if let Ok(file) = OpenOptions::new().create(true).append(true).open(log_path(&self.0.dir, &self.0.name, 0)) {
                *f = (file, 0);
            }
        }
        f.0.write_all(buf)?;
        f.1 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.file.lock().0.flush()
    }
}

impl<'a> MakeWriter<'a> for &'static RollingFile {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> LogWriter {
        LogWriter(self)
    }
}

fn remember(buf: &[u8]) {
    let text = String::from_utf8_lossy(buf);
    let mut recent = RECENT.lock();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if recent.len() == RECENT_LINES {
            recent.pop_front();
        }
        recent.push_back(line.to_string());
    }
}

/// The last `n` lines of this run's log (from memory).
pub fn recent_lines(n: usize) -> Vec<String> {
    let recent = RECENT.lock();
    recent.iter().skip(recent.len().saturating_sub(n)).cloned().collect()
}

/// The last `n` lines of a file, without reading all of a big one.
pub fn tail(path: &Path, n: usize) -> io::Result<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub((n as u64).saturating_mul(400).max(64 << 10));
    f.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // probably cut in half
    }
    Ok(lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect())
}

// ---- crashes -------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    pid: u32,
    version: String,
    started: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    name: String,
}

fn marker_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("running-{pid}.json"))
}

/// Markers left by processes that are gone: writes a report for each (unless the panic hook
/// already did) and removes the marker.
fn collect_unclean(data_dir: &Path, name: &str) -> Vec<PathBuf> {
    let dir = logs_dir(data_dir);
    let mut reports = vec![];
    let Ok(entries) = fs::read_dir(&dir) else { return reports };
    for e in entries.flatten() {
        let path = e.path();
        let Some(pid) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("running-")?.strip_suffix(".json")?.parse::<u32>().ok()) else {
            continue;
        };
        if pid == std::process::id() || pid_alive(pid) {
            continue;
        }
        let marker: Option<Marker> = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let _ = fs::remove_file(&path);
        if let Some(existing) = report_for_pid(data_dir, pid) {
            reports.push(existing);
            continue;
        }
        // The log of that run is `name.log` until this run rotates it.
        let log = tail(&log_path(&dir, name, 0), 200).unwrap_or_default();
        let started = marker.as_ref().map(|m| format!("{} (kimchi {})", m.started.to_rfc3339(), m.version)).unwrap_or_else(|| "unknown".into());
        // The last thing it logged says the most about where it stopped.
        let last = log.iter().rev().map(|l| last_words(l)).find(|l| !l.is_empty()).unwrap_or_else(|| "nothing in its log".into());
        let body = format!(
            "kimchi didn't quit properly\n\nLast in its log: {last}\n\nThe process (pid {pid}, started {started}) ended without quitting: a crash outside Rust \
             (a graphics driver, ffmpeg, the system), a forced quit, or the computer stopping.\n\n{}\n\n---- last lines of its log ----\n{}\n",
            system_summary(),
            log.join("\n")
        );
        if let Some(p) = write_report(data_dir, "unclean", pid, &body) {
            reports.push(p);
        }
    }
    reports
}

/// A log line without its timestamp (`2026-…Z  WARN target: message` → `WARN target: message`).
fn last_words(line: &str) -> String {
    let l = line.trim();
    let l = match l.split_once(char::is_whitespace) {
        Some((first, rest)) if first.len() > 18 && first.as_bytes()[4] == b'-' => rest.trim_start(),
        _ => l,
    };
    l.chars().take(240).collect()
}

fn report_for_pid(data_dir: &Path, pid: u32) -> Option<PathBuf> {
    let suffix = format!("-{pid}.txt");
    fs::read_dir(crashes_dir(data_dir)).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&suffix)))
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        return Path::new(&format!("/proc/{pid}")).exists();
    }
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
}

#[cfg(not(any(unix, windows)))]
fn pid_alive(_pid: u32) -> bool {
    false
}

fn write_report(data_dir: &Path, kind: &str, pid: u32, body: &str) -> Option<PathBuf> {
    let dir = crashes_dir(data_dir);
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{kind}-{}-{pid}.txt", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
    fs::write(&path, body).ok()?;
    prune_reports(&dir);
    Some(path)
}

fn prune_reports(dir: &Path) {
    let mut all: Vec<(std::time::SystemTime, PathBuf)> =
        fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if all.len() > KEEP_REPORTS {
        all.sort();
        for (_, p) in &all[..all.len() - KEEP_REPORTS] {
            let _ = fs::remove_file(p);
        }
    }
}

fn install_panic_hook(data_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed").to_string();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_else(|| "unknown".into());
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(target: "kimchi::panic", "panic on thread '{thread}' at {location}: {message}");
        let body = format!(
            "kimchi crash report\n\nPanic on thread '{thread}' at {location}:\n{message}\n\n{}\n\n---- backtrace ----\n{backtrace}\n\n---- last lines of the log ----\n{}\n",
            system_summary(),
            recent_lines(150).join("\n")
        );
        write_report(&data_dir, "crash", std::process::id(), &body);
        previous(info);
    }));
}

/// Version, system, architecture: the lines every report starts with.
pub fn system_summary() -> String {
    format!(
        "kimchi {}\nSystem: {} ({} {})\nTime: {}\nExecutable: {}",
        env!("CARGO_PKG_VERSION"),
        os_name(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        chrono::Utc::now().to_rfc3339(),
        std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
    )
}

/// A readable OS name and version (`macOS 15.1`, `Ubuntu 24.04.1 LTS`, `Windows 10.0.26100`).
pub fn os_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            let v = std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            return format!("macOS {}", v.unwrap_or_default()).trim().to_string();
        }
        #[cfg(target_os = "linux")]
        {
            let pretty = fs::read_to_string("/etc/os-release").ok().and_then(|t| {
                t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
            });
            let session = std::env::var("XDG_SESSION_TYPE").ok().filter(|s| !s.is_empty());
            return match (pretty, session) {
                (Some(p), Some(s)) => format!("{p}, {s}"),
                (Some(p), None) => p,
                (None, _) => "Linux".into(),
            };
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let v = std::process::Command::new("cmd").args(["/C", "ver"]).creation_flags(0x0800_0000).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            return v.filter(|v| !v.is_empty()).unwrap_or_else(|| "Windows".into());
        }
        #[allow(unreachable_code)]
        std::env::consts::OS.to_string()
    })
    .clone()
}

// ---- listing -------------------------------------------------------------------

/// One crash report, as `app.crashReports` lists them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The file name (`crash-20261003-101500-1234.txt`).
    pub id: String,
    pub path: PathBuf,
    /// `crash` (a panic kimchi caught) or `unclean` (the process ended without quitting).
    pub kind: String,
    pub at: chrono::DateTime<chrono::Utc>,
    /// The first line that says what happened.
    pub summary: String,
}

/// Reports, newest first.
pub fn reports(data_dir: &Path) -> Vec<Report> {
    let mut out: Vec<Report> = fs::read_dir(crashes_dir(data_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let id = path.file_name()?.to_str()?.to_string();
            if !id.ends_with(".txt") {
                return None;
            }
            let kind = id.split('-').next()?.to_string();
            let at: chrono::DateTime<chrono::Utc> = e.metadata().ok()?.modified().ok()?.into();
            let text = fs::read_to_string(&path).unwrap_or_default();
            let summary = summarize(&text);
            Some(Report { id, path, kind, at, summary })
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.at));
    out
}

fn summarize(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or_default();
    if first == "kimchi crash report" {
        // "Panic on thread … at file:line:" then the message.
        let _where = lines.next();
        return lines.next().unwrap_or("A panic").chars().take(240).collect();
    }
    lines.next().map(|l| l.chars().take(240).collect()).unwrap_or_else(|| first.to_string())
}

/// Reads one report by file name (no paths: `id` must be a report in the crashes folder).
pub fn read_report(data_dir: &Path, id: &str) -> Result<String, String> {
    if id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(format!("`{id}` isn't a crash report name."));
    }
    fs::read_to_string(crashes_dir(data_dir).join(id)).map_err(|_| format!("No crash report named `{id}`. app.crashReports lists them."))
}

/// Deletes every crash report.
pub fn clear_reports(data_dir: &Path) -> usize {
    let mut n = 0;
    for e in fs::read_dir(crashes_dir(data_dir)).into_iter().flatten().flatten() {
        if fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// The log files, newest first: `(path, bytes)`.
pub fn log_files(data_dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out: Vec<(std::time::SystemTime, PathBuf, u64)> = fs::read_dir(logs_dir(data_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let m = e.metadata().ok()?;
            if !m.is_file() || p.extension().is_none_or(|x| x != "log") {
                return None;
            }
            Some((m.modified().ok()?, p, m.len()))
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.0));
    out.into_iter().map(|(_, p, n)| (p, n)).collect()
}

/// The current log file of this process, when logging to a file.
pub fn current_log() -> Option<PathBuf> {
    STATE.get().map(|st| log_path(&st.dir, &st.name, 0))
}

/// Reports not shown in the window yet (newer than `<config>/reports-seen`), and marks them seen.
pub fn unseen_reports(data_dir: &Path, config_dir: &Path) -> Vec<Report> {
    let seen_file = config_dir.join("reports-seen");
    let seen = fs::read_to_string(&seen_file).ok().and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok()).map(|d| d.to_utc());
    let all = reports(data_dir);
    let _ = fs::write(&seen_file, chrono::Utc::now().to_rfc3339());
    all.into_iter().filter(|r| seen.is_none_or(|s| r.at > s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_roll_over_and_keep_three() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..6 {
            fs::write(log_path(dir.path(), "kimchi", 0), format!("run {i}")).unwrap();
            rotate(dir.path(), "kimchi");
        }
        assert!(!log_path(dir.path(), "kimchi", 0).exists());
        assert_eq!(fs::read_to_string(log_path(dir.path(), "kimchi", 1)).unwrap(), "run 5");
        assert_eq!(fs::read_to_string(log_path(dir.path(), "kimchi", 3)).unwrap(), "run 3");
        assert!(!log_path(dir.path(), "kimchi", 4).exists());
    }

    #[test]
    fn tail_reads_the_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.log");
        fs::write(&p, (0..1000).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        assert_eq!(tail(&p, 2).unwrap(), vec!["line 998", "line 999"]);
        assert!(tail(&dir.path().join("missing.log"), 2).is_err());
    }

    #[test]
    fn a_marker_left_by_a_dead_process_becomes_a_report() {
        let data = tempfile::tempdir().unwrap();
        let logs = logs_dir(data.path());
        fs::create_dir_all(&logs).unwrap();
        fs::write(log_path(&logs, "kimchi", 0), "INFO starting\nWARN last words\n").unwrap();
        // No process has pid u32::MAX - 1.
        let pid = u32::MAX - 1;
        let m = Marker { pid, version: "0.5.0".into(), started: chrono::Utc::now(), name: "kimchi".into() };
        fs::write(marker_path(&logs, pid), serde_json::to_vec(&m).unwrap()).unwrap();
        let found = collect_unclean(data.path(), "kimchi");
        assert_eq!(found.len(), 1);
        let text = fs::read_to_string(&found[0]).unwrap();
        assert!(text.contains("didn't quit properly") && text.contains("last words"), "{text}");
        assert_eq!(summarize(&text), "Last in its log: WARN last words");
        assert!(!marker_path(&logs, pid).exists());
        let listed = reports(data.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, "unclean");
        assert!(read_report(data.path(), &listed[0].id).is_ok());
        assert!(read_report(data.path(), "../settings.json").is_err());
        // Shown once.
        let config = tempfile::tempdir().unwrap();
        assert_eq!(unseen_reports(data.path(), config.path()).len(), 1);
        assert_eq!(unseen_reports(data.path(), config.path()).len(), 0);
        assert_eq!(clear_reports(data.path()), 1);
    }

    #[test]
    fn crash_summaries_name_the_message() {
        let text = "kimchi crash report\n\nPanic on thread 'main' at src/a.rs:1:2:\nindex out of bounds\n\nkimchi 0.5.0";
        assert_eq!(summarize(text), "index out of bounds");
    }
}
