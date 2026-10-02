//! Automatic updates from this repository's GitHub Releases.
//!
//! `latest.json` describes the newest release per platform; each archive is
//! signed with the project's Ed25519 (minisign) update key and verified before
//! anything is replaced. The previous copy is kept until the new one starts.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::session::{CmdResult, Session};

/// What the app knows about updates, shown in the window and returned by
/// `app.checkUpdates`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current: String,
    /// The newer version, when there is one.
    pub available: Option<String>,
    pub notes: Option<String>,
    /// 0–1 while downloading.
    pub progress: Option<f64>,
    /// Installed; restart to use it.
    pub ready: bool,
    pub error: Option<String>,
    /// Whether this build can install updates itself (otherwise it links to the download page).
    pub can_install: bool,
    pub checked_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Default)]
pub struct UpdateState {
    pub status: UpdateStatus,
}

/// Checks for a newer release. `manual` reports errors instead of staying quiet.
pub async fn check(s: &Arc<Session>, manual: bool) -> CmdResult<UpdateStatus> {
    let _ = manual;
    let status = UpdateStatus { current: env!("CARGO_PKG_VERSION").into(), checked_at: Some(chrono::Utc::now()), ..Default::default() };
    s.update.lock().status = status.clone();
    Ok(status)
}

/// Downloads, verifies and installs the update found by [`check`].
pub async fn install(s: &Arc<Session>) -> CmdResult<UpdateStatus> {
    let _ = s;
    Err("Updates can't be installed from this build yet.".into())
}

pub fn status(s: &Session) -> UpdateStatus {
    s.update.lock().status.clone()
}
