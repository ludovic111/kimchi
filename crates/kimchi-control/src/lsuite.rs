//! Where kimchi finds lsuite: the shared folder every lsuite app on this computer uses
//! (`~/.lsuite`, for plugins and discovery files) and the lsuite server (lsuite.xyz, which serves
//! the apps' builds and updates; no account is needed).
//!
//! - `LSUITE_HOME` replaces `~/.lsuite`.
//! - `LSUITE_SERVER` replaces `https://lsuite.xyz` (a local demo server, tests).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const DEFAULT_SERVER: &str = "https://lsuite.xyz";

/// Where tests put the lsuite folder and which server they talk to ([`testing`]).
static OVERRIDE: parking_lot::RwLock<Option<(PathBuf, Option<String>)>> = parking_lot::RwLock::new(None);

/// `$LSUITE_HOME`, else `~/.lsuite`. A test binary never reads the person's own folder: without
/// `LSUITE_HOME` it gets a scratch one.
pub fn lsuite_home() -> PathBuf {
    if let Some((home, _)) = OVERRIDE.read().clone() {
        return home;
    }
    if let Some(p) = std::env::var_os("LSUITE_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if is_test_binary() {
        return std::env::temp_dir().join(format!("kimchi-test-lsuite-{}", std::process::id()));
    }
    dirs::home_dir().unwrap_or_else(std::env::temp_dir).join(".lsuite")
}

/// A `cargo test` binary (it lives in `target/<profile>/deps`).
fn is_test_binary() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| std::env::current_exe().ok().and_then(|e| e.parent().and_then(|p| p.file_name()).map(|n| n == "deps")).unwrap_or(false))
}

/// The lsuite server, without a trailing slash: `LSUITE_SERVER`, else lsuite.xyz.
pub fn server() -> String {
    if let Some((_, Some(server))) = OVERRIDE.read().clone() {
        return server;
    }
    std::env::var("LSUITE_SERVER")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

/// For tests: the lsuite folder is `home` and the server is `server`, until the guard is dropped.
/// Tests that use it run one at a time.
#[doc(hidden)]
pub fn testing(home: &Path, server: Option<&str>) -> TestGuard {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let lock = ONE.lock().unwrap_or_else(|e| e.into_inner());
    *OVERRIDE.write() = Some((home.to_path_buf(), server.map(|s| s.trim_end_matches('/').to_string())));
    TestGuard { _lock: lock }
}

#[doc(hidden)]
pub struct TestGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        *OVERRIDE.write() = None;
    }
}
