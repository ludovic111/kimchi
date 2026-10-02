//! lsuite discovery: each app writes `~/.lsuite/apps/<app>.json` when it starts,
//! so the other apps (and agents) know what is installed and how to drive it.
//!
//! Format 1 (kimchi is the first app to write it; see lsuite's STANDARD.md):
//!
//! ```json
//! {
//!   "format": 1,
//!   "app": "kimchi",
//!   "version": "0.2.0",
//!   "kind": "video",
//!   "appPath": "/Applications/kimchi.app",
//!   "executable": "/Applications/kimchi.app/Contents/MacOS/kimchi",
//!   "cli": "/Applications/kimchi.app/Contents/MacOS/kimchi-cli",
//!   "mcp": "/Applications/kimchi.app/Contents/MacOS/kimchi-mcp",
//!   "dataDir": "~/Library/Application Support/kimchi",
//!   "documents": { "extensions": ["json"], "description": "kimchi project (JSON)" },
//!   "running": { "pid": 4242, "controlFile": ".../control.json", "port": 51234, "since": "2026-10-02T09:00:00Z" },
//!   "updatedAt": "2026-10-02T09:00:00Z"
//! }
//! ```
//!
//! `running` is `null` when the app is closed. A reader checks that `pid` is
//! alive before trusting it. Unknown fields are ignored, so the format can grow.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub format: u32,
    pub app: String,
    pub version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub app_path: Option<PathBuf>,
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub cli: Option<PathBuf>,
    #[serde(default)]
    pub mcp: Option<PathBuf>,
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    #[serde(default)]
    pub documents: Value,
    #[serde(default)]
    pub running: Option<Running>,
    #[serde(default)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Running {
    pub pid: u32,
    #[serde(default)]
    pub control_file: Option<PathBuf>,
    #[serde(default)]
    pub port: Option<u16>,
    pub since: chrono::DateTime<chrono::Utc>,
}

/// `$LSUITE_HOME/apps`, else `~/.lsuite/apps`.
pub fn apps_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("LSUITE_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p).join("apps");
    }
    dirs::home_dir().unwrap_or_else(std::env::temp_dir).join(".lsuite").join("apps")
}

/// The executables that ship next to this one (the app bundle's MacOS folder,
/// or the install folder).
fn sibling(name: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    p.is_file().then_some(p)
}

/// The `.app` bundle this executable lives in, on macOS.
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// This app's entry. `running` describes the bridge when it is up.
pub fn entry(data_dir: &Path, running: Option<Running>) -> AppEntry {
    AppEntry {
        format: FORMAT,
        app: "kimchi".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        kind: "video".into(),
        app_path: bundle(),
        executable: std::env::current_exe().ok(),
        cli: sibling("kimchi-cli"),
        mcp: sibling("kimchi-mcp"),
        data_dir: Some(data_dir.to_path_buf()),
        documents: serde_json::json!({ "extensions": ["json"], "description": "kimchi project (JSON)" }),
        running,
        updated_at: Some(chrono::Utc::now()),
    }
}

/// Writes `~/.lsuite/apps/kimchi.json`.
pub fn write(entry: &AppEntry) -> std::io::Result<PathBuf> {
    let dir = apps_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", entry.app));
    let tmp = dir.join(format!(".{}.json.tmp", entry.app));
    std::fs::write(&tmp, serde_json::to_vec_pretty(entry).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, &path)?;
    Ok(path)
}

/// Every lsuite app that wrote an entry, with `running` cleared for dead processes.
pub fn installed_apps() -> Vec<AppEntry> {
    let Ok(entries) = std::fs::read_dir(apps_dir()) else { return vec![] };
    let mut apps: Vec<AppEntry> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<AppEntry>(&std::fs::read(e.path()).ok()?).ok())
        .map(|mut a| {
            if a.running.as_ref().is_some_and(|r| !alive(r.pid)) {
                a.running = None;
            }
            a
        })
        .collect();
    apps.sort_by(|a, b| a.app.cmp(&b.app));
    apps
}

pub fn find(app: &str) -> Option<AppEntry> {
    installed_apps().into_iter().find(|a| a.app == app)
}

/// Whether a process id is alive.
pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 checks existence without sending anything.
        std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}
