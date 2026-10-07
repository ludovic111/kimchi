//! Automatic updates, through lsuite (lsuite `DISTRIBUTION.md`).
//!
//! kimchi's builds come through the lsuite account: the check reads
//! `<server>/api/apps/kimchi/latest.json` with the account's token (`Authorization: Bearer`, from
//! `~/.lsuite/account.json`; `<server>` is `LSUITE_ACCOUNT_SERVER`, else the account's, else
//! lsuite.xyz), and the archives download through the same server's file route with the token.
//! Signed out (or with a revoked token), the check says [`SIGN_IN`] instead of failing.
//!
//! Every release carries a `latest.json` in the Tauri updater format (kimchi 0.1.x was a Tauri
//! app, and those installs still update through it):
//!
//! ```json
//! { "version": "0.2.0", "notes": "…", "pub_date": "…",
//!   "platforms": { "darwin-aarch64-app": { "signature": "<base64 minisign signature>", "url": "…/kimchi_aarch64.app.tar.gz" }, … } }
//! ```
//!
//! Each archive is signed with the project's minisign (Ed25519) update key, the same key the Tauri
//! builds trust ([`PUBLIC_KEY`]), and verified while it downloads, before anything is replaced. The
//! signature's trusted comment carries the version it was made for, which must match the
//! announced one, so an old signed archive can't be passed off as a new release.
//!
//! What gets replaced, and how:
//! * **macOS**: the running `kimchi.app` is moved aside to `.kimchi.app.previous` next to it and the
//!   new bundle moved in. [`finish_pending`] removes the previous copy once the new one has started.
//! * **Linux AppImage** (`$APPIMAGE`): the same, with `.<name>.previous`.
//! * **Windows**, installed with the installer (`uninstall.exe` beside `kimchi.exe`): the signed
//!   installer is downloaded and verified, then run passively once kimchi has exited, when it
//!   restarts ([`restart`]) or quits ([`apply_on_quit`]).
//! * **Portable Windows copies, Linux packages, development builds**: the update is announced with
//!   `can_install: false` and the file to download by hand (`download_url`): the portable zip
//!   (`windows-x86_64-portable`) or the `.deb` (`linux-x86_64-deb`), else the release page.
//!
//! `KIMCHI_NO_UPDATE=1` or `settings.updates.checkOnStart = false` turn off the checks at start and
//! every [`RECHECK`] ([`run_in_background`], which also installs by itself with
//! `updates.autoInstall`); `app.checkUpdates` and `app.installUpdate` always work. `KIMCHI_UPDATE_URL`
//! points the check at another `latest.json`, without the token unless it is on the lsuite server
//! (signatures are still checked against [`PUBLIC_KEY`];
//! debug builds accept `KIMCHI_UPDATE_PUBKEY` instead, for testing with a throwaway key).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::session::{CmdResult, Event, Session, ToastKind};

/// The latest release's manifest, on the lsuite server (`<server>` + this).
pub const MANIFEST_PATH: &str = "/api/apps/kimchi/latest.json";
/// Where people get kimchi by hand: the lsuite app, which installs and updates the apps.
pub const RELEASES_URL: &str = "https://lsuite.xyz/launcher";
/// What a signed-out check says.
pub const SIGN_IN: &str = "Sign in to lsuite (in the lsuite app) to get updates.";
/// The update key (base64 of a minisign public key file), the same as `plugins.updater.pubkey` in
/// the Tauri builds. Its secret half is the `TAURI_SIGNING_PRIVATE_KEY` release secret.
pub const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRDMDcxNTc4RjA1N0Q5QTcKUldTbjJWZndlQlVIVEZyRWNtU2tqeWxCaFNvTGQxbU1qczFIMGY5cUYzVVdBWkxOL0FvalJocEkK";
/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Refuse downloads larger than this (a release archive is ~100 MB).
const MAX_DOWNLOAD: u64 = 2 << 30;
/// How often a running app checks again.
pub const RECHECK: Duration = Duration::from_secs(6 * 3600);

/// A verified Windows installer waiting for kimchi to exit.
static PENDING_INSTALLER: parking_lot::Mutex<Option<PathBuf>> = parking_lot::Mutex::new(None);

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
    /// Where to get the update by hand: this platform's file, else the release page.
    #[serde(default)]
    pub download_url: Option<String>,
    /// Why this copy can't replace itself, when it can't (Windows, a Linux package, a build run
    /// from the source tree, an app still in Downloads…).
    #[serde(default)]
    pub install_blocked: Option<String>,
    /// No lsuite account is signed in (or its token was refused): updates need one ([`SIGN_IN`]).
    #[serde(default)]
    pub sign_in: bool,
}

#[derive(Debug, Default)]
pub struct UpdateState {
    pub status: UpdateStatus,
    /// This platform's archive in the newer release found by the last check.
    found: Option<Found>,
    /// A download or install is running.
    busy: bool,
}

#[derive(Debug, Clone)]
struct Found {
    version: String,
    url: String,
    signature: String,
    /// The account's token, for downloads from the lsuite server.
    token: Option<String>,
}

/// Where the check goes and with which token.
struct Source {
    url: String,
    /// The lsuite server's address, which gets the token.
    server: String,
    token: Option<String>,
}

/// The manifest to read: `KIMCHI_UPDATE_URL` (tests), else the lsuite server's, with the
/// account's token. `None` when signed out.
fn source() -> Option<Source> {
    let server = crate::account::server();
    let token = crate::account::load().map(|a| a.token).filter(|t| !t.trim().is_empty());
    if let Some(url) = std::env::var("KIMCHI_UPDATE_URL").ok().filter(|u| !u.trim().is_empty()) {
        // The token goes only to the lsuite server, never to another address.
        let token = token.filter(|_| same_origin(&url, &server));
        return Some(Source { url, server, token });
    }
    let token = token?;
    Some(Source { url: format!("{}{MANIFEST_PATH}", server.trim_end_matches('/')), server, token: Some(token) })
}

/// Whether `url` is on `server` (scheme, host and port).
pub fn same_origin(url: &str, server: &str) -> bool {
    match (url::Url::parse(url), url::Url::parse(server)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}

/// Where to send someone who downloads by hand: the lsuite app's page on the account's server.
pub fn download_page() -> String {
    format!("{}/launcher", crate::account::server().trim_end_matches('/'))
}

/// Why a check failed: the server wants a (new) sign-in, or anything else.
enum CheckError {
    SignIn,
    Other(String),
}

/// `latest.json`, in the Tauri updater's format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, PlatformAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlatformAsset {
    /// Base64 of the minisign signature file.
    pub signature: String,
    pub url: String,
}

/// How this copy of kimchi is installed, which decides what an update replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// A macOS app bundle, replaced in place.
    MacBundle(PathBuf),
    /// A Linux AppImage (`$APPIMAGE`), replaced in place.
    AppImage(PathBuf),
    /// Windows, installed by the installer into this folder; the next installer runs over it.
    WindowsInstalled(PathBuf),
    /// Windows, unpacked from the portable zip into this folder: updated by hand with the next zip.
    WindowsPortable(PathBuf),
    /// Linux, installed from the `.deb` package (`/usr/lib/kimchi`): updated by hand with the next one.
    DebPackage,
    /// Anything else (a build run from the source tree, another package): updated by hand.
    Other,
}

// ---- checking -----------------------------------------------------------

/// Checks for a newer release. `manual` reports errors instead of staying quiet.
pub async fn check(s: &Arc<Session>, manual: bool) -> CmdResult<UpdateStatus> {
    {
        let u = s.update.lock();
        if u.busy || u.status.ready {
            return Ok(u.status.clone());
        }
    }
    let src = source();
    let fetched = match &src {
        None => Err(CheckError::SignIn),
        Some(src) => fetch_manifest(&src.url, src.token.as_deref()).await,
    };
    let install = current_install();
    let status = {
        let mut u = s.update.lock();
        if u.busy || u.status.ready {
            return Ok(u.status.clone());
        }
        let now = Some(chrono::Utc::now());
        match &fetched {
            Ok(m) => {
                let newer = is_newer(&m.version, CURRENT);
                let asset = select(m, &platform_keys_for(&install)).filter(|_| newer);
                let blocked = install_support(&install).err();
                let token = src.as_ref().and_then(|src| src.token.clone().filter(|_| asset.is_some_and(|a| same_origin(&a.url, &src.server))));
                // A file on the lsuite server needs the token, which a browser doesn't have: by hand,
                // people get it through the lsuite app.
                let by_hand = asset.map(|a| if token.is_some() { download_page() } else { a.url.clone() });
                u.found = asset.map(|a| Found { version: m.version.clone(), url: a.url.clone(), signature: a.signature.clone(), token: token.clone() });
                u.status = UpdateStatus {
                    current: CURRENT.into(),
                    available: newer.then(|| m.version.clone()),
                    notes: if newer { m.notes.clone() } else { None },
                    progress: None,
                    ready: false,
                    error: None,
                    can_install: asset.is_some() && blocked.is_none(),
                    checked_at: now,
                    download_url: newer.then(|| by_hand.unwrap_or_else(download_page)),
                    install_blocked: if newer { blocked } else { None },
                    sign_in: false,
                };
            }
            Err(CheckError::SignIn) => {
                // Not a failure: updates come with the (free) lsuite account.
                u.found = None;
                u.status = UpdateStatus { current: CURRENT.into(), checked_at: now, error: Some(SIGN_IN.into()), sign_in: true, ..Default::default() };
            }
            Err(CheckError::Other(e)) => {
                tracing::debug!("update check failed: {e}");
                u.status.current = CURRENT.into();
                u.status.checked_at = now;
                u.status.sign_in = false;
                u.status.error = manual.then(|| e.clone());
            }
        }
        u.status.clone()
    };
    s.emit(Event::Update { status: status.clone() });
    match fetched {
        Err(CheckError::Other(e)) if manual => Err(e),
        _ => Ok(status),
    }
}

/// The check the app runs when it starts; does nothing when `KIMCHI_NO_UPDATE` is set or
/// `updates.checkOnStart` is off. Never fails: a failed check stays quiet. With
/// `updates.autoInstall`, a found update is downloaded and installed.
pub async fn check_on_start(s: &Arc<Session>) {
    let settings = s.settings();
    if !settings.update_check_enabled() {
        return;
    }
    if let Ok(st) = check(s, false).await
        && settings.updates.auto_install
        && st.available.is_some()
        && st.can_install
        && !st.ready
    {
        tracing::info!("installing kimchi {} in the background (updates.autoInstall)", st.available.as_deref().unwrap_or_default());
        if let Err(e) = install(s).await {
            tracing::warn!("the automatic update failed: {e}");
        }
    }
}

/// Checks a few seconds after start, then every [`RECHECK`] while the app runs (each time
/// subject to the settings, which may change in between).
pub async fn run_in_background(s: Arc<Session>) {
    tokio::time::sleep(Duration::from_secs(4)).await;
    loop {
        check_on_start(&s).await;
        tokio::time::sleep(RECHECK).await;
    }
}

pub fn status(s: &Session) -> UpdateStatus {
    let mut st = s.update.lock().status.clone();
    if st.current.is_empty() {
        st.current = CURRENT.into();
    }
    st
}

/// The update key. Debug builds take `KIMCHI_UPDATE_PUBKEY` (base64) to test a release signed with
/// a throwaway key; release builds always use [`PUBLIC_KEY`].
fn public_key() -> String {
    #[cfg(debug_assertions)]
    if let Some(k) = std::env::var("KIMCHI_UPDATE_PUBKEY").ok().filter(|k| !k.trim().is_empty()) {
        return k;
    }
    PUBLIC_KEY.to_string()
}

fn client(timeout: Option<Duration>) -> Result<reqwest::Client, String> {
    let mut b = reqwest::Client::builder().user_agent(format!("kimchi/{CURRENT}")).connect_timeout(Duration::from_secs(15));
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    b.build().map_err(|e| e.to_string())
}

async fn fetch_manifest(url: &str, token: Option<&str>) -> Result<Manifest, CheckError> {
    let offline = |e: reqwest::Error| CheckError::Other(format!("Couldn't reach lsuite to check for updates ({e})."));
    let mut req = client(Some(Duration::from_secs(30))).map_err(CheckError::Other)?.get(url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let res = req.send().await.map_err(offline)?;
    match res.status() {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => return Err(CheckError::SignIn),
        reqwest::StatusCode::NOT_FOUND => return Err(CheckError::Other("No kimchi release with an update manifest has been published yet.".into())),
        s if !s.is_success() => return Err(CheckError::Other(format!("lsuite answered {s} to the update check."))),
        _ => {}
    }
    let bytes = res.bytes().await.map_err(offline)?;
    serde_json::from_slice(&bytes).map_err(|e| CheckError::Other(format!("The update manifest isn't valid: {e}")))
}

/// Whether `candidate` is a newer version than `current` (semver; a leading `v` is allowed).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim().trim_start_matches('v')).ok();
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

/// The `latest.json` keys to look for, most specific first: `{os}-{arch}-{installer}` then
/// `{os}-{arch}` (the Tauri updater's rule). `os` is `darwin`, `linux` or `windows`.
pub fn platform_keys(os: &str, arch: &str, installer: Option<&str>) -> Vec<String> {
    let mut keys = Vec::with_capacity(2);
    if let Some(i) = installer {
        keys.push(format!("{os}-{arch}-{i}"));
    }
    keys.push(format!("{os}-{arch}"));
    keys
}

fn platform_keys_for(install: &Install) -> Vec<String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    install_keys(install, os, std::env::consts::ARCH)
}

/// The `latest.json` keys for an install kind. A `.deb` or a portable zip only takes its own file:
/// falling back to `{os}-{arch}` would offer the AppImage or the installer, so without its own
/// entry it gets the release page instead.
pub fn install_keys(install: &Install, os: &str, arch: &str) -> Vec<String> {
    match install {
        Install::MacBundle(_) => platform_keys(os, arch, Some("app")),
        Install::AppImage(_) => platform_keys(os, arch, Some("appimage")),
        Install::WindowsInstalled(_) => platform_keys(os, arch, Some("nsis")),
        Install::WindowsPortable(_) => vec![format!("{os}-{arch}-portable")],
        Install::DebPackage => vec![format!("{os}-{arch}-deb")],
        Install::Other => platform_keys(os, arch, None),
    }
}

/// The first of `keys` the manifest has an archive for.
pub fn select<'a>(m: &'a Manifest, keys: &[String]) -> Option<&'a PlatformAsset> {
    keys.iter().find_map(|k| m.platforms.get(k))
}

// ---- signatures ---------------------------------------------------------

fn b64_text(what: &str, b64: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| format!("The {what} isn't valid base64."))?;
    String::from_utf8(bytes).map_err(|_| format!("The {what} isn't valid text."))
}

fn decode_key(public_key_b64: &str) -> Result<PublicKey, String> {
    PublicKey::decode(&b64_text("update key", public_key_b64)?).map_err(|e| format!("The update key is invalid: {e}"))
}

fn decode_signature(signature_b64: &str) -> Result<Signature, String> {
    Signature::decode(&b64_text("update signature", signature_b64)?).map_err(|e| format!("The update signature is invalid: {e}"))
}

fn signature_error(e: minisign_verify::Error) -> String {
    match e {
        minisign_verify::Error::UnexpectedKeyId => "The update was signed with a different key than kimchi's; it was not installed.".into(),
        minisign_verify::Error::UnsupportedLegacyMode | minisign_verify::Error::UnexpectedAlgorithm => {
            "The update signature uses an unsupported (legacy) algorithm; it was not installed.".into()
        }
        _ => "The update's signature doesn't match the download; it was not installed.".into(),
    }
}

/// The trusted comment (covered by the signature) must name the announced version.
fn check_signed_version(sig: &Signature, version: &str) -> Result<(), String> {
    let signed = sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:")).map(str::trim);
    let Some(signed) = signed else {
        return Err("The update signature doesn't say which version it was made for; it was not installed.".into());
    };
    let same = match (semver::Version::parse(signed.trim_start_matches('v')), semver::Version::parse(version.trim_start_matches('v'))) {
        (Ok(a), Ok(b)) => a == b,
        _ => signed == version,
    };
    if same { Ok(()) } else { Err(format!("The update was signed for version {signed}, not {version}; it was not installed.")) }
}

/// Verifies `data` against a Tauri-style signature (base64 of a minisign signature file) and
/// public key (base64 of a minisign public key file), and that it was signed for `version`.
pub fn verify(mut data: impl Read, signature_b64: &str, public_key_b64: &str, version: &str) -> Result<(), String> {
    let pk = decode_key(public_key_b64)?;
    let sig = decode_signature(signature_b64)?;
    let mut v = pk.verify_stream(&sig).map_err(signature_error)?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = data.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        v.update(&buf[..n]);
    }
    v.finalize().map_err(signature_error)?;
    check_signed_version(&sig, version)
}

// ---- installing ---------------------------------------------------------

/// Downloads, verifies and installs the update found by [`check`].
pub async fn install(s: &Arc<Session>) -> CmdResult<UpdateStatus> {
    if s.update.lock().status.ready {
        return Ok(status(s));
    }
    if s.update.lock().found.is_none() {
        check(s, true).await?;
    }
    let install = current_install();
    let found = {
        let mut u = s.update.lock();
        if u.status.ready {
            return Ok(u.status.clone());
        }
        if u.busy {
            return Err("An update is already being installed.".into());
        }
        let Some(found) = u.found.clone() else {
            return Err(match &u.status.available {
                Some(v) => format!("kimchi {v} has no download for this platform yet. See {RELEASES_URL}"),
                None => format!("kimchi {CURRENT} is up to date."),
            });
        };
        if let Err(why) = install_support(&install) {
            return Err(format!("{why} Download kimchi {} from {}", found.version, u.status.download_url.as_deref().unwrap_or(RELEASES_URL)));
        }
        u.busy = true;
        u.status.progress = Some(0.0);
        u.status.error = None;
        found
    };
    s.emit(Event::Update { status: status(s) });

    // If this future is dropped (a client went away mid-download), the next install may start.
    struct NotBusy<'a>(&'a Session);
    impl Drop for NotBusy<'_> {
        fn drop(&mut self) {
            let mut u = self.0.update.lock();
            if u.busy {
                u.busy = false;
                u.status.progress = None;
            }
        }
    }
    let guard = NotBusy(s);
    let result = download_and_install(s, &found, &install).await;
    drop(guard);

    let st = {
        let mut u = s.update.lock();
        u.busy = false;
        u.status.progress = None;
        match &result {
            Ok(()) => {
                u.status.ready = true;
                u.status.can_install = false;
            }
            Err(e) => u.status.error = Some(e.clone()),
        }
        u.status.clone()
    };
    s.emit(Event::Update { status: st.clone() });
    match result {
        Ok(()) => {
            s.toast(ToastKind::Success, format!("kimchi {} is installed. Restart kimchi to use it.", found.version));
            Ok(st)
        }
        Err(e) => Err(e),
    }
}

async fn download_and_install(s: &Arc<Session>, found: &Found, install: &Install) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("kimchi-update-{}-{}", found.version, std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("Couldn't prepare the download: {e}"))?;
    let name = found.url.rsplit('/').next().filter(|n| !n.is_empty() && !n.contains(['?', '#'])).unwrap_or("kimchi-update");
    let file = dir.join(name);
    let result = async {
        download_verified(s, found, &file, &public_key()).await?;
        let (file, install) = (file.clone(), install.clone());
        tokio::task::spawn_blocking(move || match install {
            Install::MacBundle(bundle) => install_bundle(&file, &bundle),
            Install::AppImage(path) => install_appimage(&file, &path),
            // The installer can't replace a running kimchi.exe: it runs once kimchi has exited.
            Install::WindowsInstalled(_) => {
                *PENDING_INSTALLER.lock() = Some(file);
                Ok(())
            }
            Install::WindowsPortable(_) | Install::DebPackage | Install::Other => Err("This copy of kimchi can't replace itself.".into()),
        })
        .await
        .map_err(|e| e.to_string())?
    }
    .await;
    if PENDING_INSTALLER.lock().as_deref().is_none_or(|p| !p.starts_with(&dir)) {
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
    result
}

/// Streams the archive to `file`, hashing it as it arrives, and checks the signature.
async fn download_verified(s: &Arc<Session>, found: &Found, file: &Path, public_key_b64: &str) -> Result<(), String> {
    let pk = decode_key(public_key_b64)?;
    let sig = decode_signature(&found.signature)?;
    // Check the signature's version before spending the bandwidth.
    check_signed_version(&sig, &found.version)?;
    let mut verifier = pk.verify_stream(&sig).map_err(signature_error)?;

    let failed = |e: reqwest::Error| format!("The download failed ({e}).");
    let mut req = client(None)?.get(&found.url).header(reqwest::header::ACCEPT, "application/octet-stream");
    // The lsuite server's file route wants the token; it answers with a redirect to the file's
    // own short-lived address, where the token isn't sent (reqwest drops it across hosts).
    if let Some(t) = &found.token {
        req = req.bearer_auth(t);
    }
    let res = req.send().await.map_err(failed)?;
    if !res.status().is_success() {
        return Err(format!("The download failed: the server answered {}.", res.status()));
    }
    let total = res.content_length().filter(|&n| n > 0);
    if total.is_some_and(|n| n > MAX_DOWNLOAD) {
        return Err("The update is unexpectedly large; it was not downloaded.".into());
    }
    let mut out = tokio::fs::File::create(file).await.map_err(|e| format!("Couldn't save the download: {e}"))?;
    let mut stream = res.bytes_stream();
    let (mut got, mut shown) = (0u64, 0.0f64);
    loop {
        let next = tokio::time::timeout(Duration::from_secs(60), stream.next()).await.map_err(|_| "The download stalled; try again.".to_string())?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(failed)?;
        got += chunk.len() as u64;
        if got > MAX_DOWNLOAD {
            return Err("The update is unexpectedly large; the download was stopped.".into());
        }
        verifier.update(&chunk);
        out.write_all(&chunk).await.map_err(|e| format!("Couldn't save the download: {e}"))?;
        if let Some(total) = total {
            let p = (got as f64 / total as f64).min(1.0);
            if p - shown >= 0.01 {
                shown = p;
                let st = {
                    let mut u = s.update.lock();
                    u.status.progress = Some(p);
                    u.status.clone()
                };
                s.emit(Event::Update { status: st });
            }
        }
    }
    out.flush().await.map_err(|e| e.to_string())?;
    out.sync_all().await.map_err(|e| e.to_string())?;
    drop(out);
    verifier.finalize().map_err(signature_error)
}

/// How the running copy is installed.
pub fn current_install() -> Install {
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return Install::Other;
    };
    install_of(std::env::consts::OS, &exe, std::env::var_os("APPIMAGE").map(PathBuf::from))
}

/// How a copy is installed, from its OS (`std::env::consts::OS`), its executable (canonical) and
/// `$APPIMAGE`. The `.deb` puts kimchi in `/usr/lib/kimchi` (`scripts/bundle-linux.sh`); the
/// Windows installer leaves `uninstall.exe` beside `kimchi.exe`, the portable zip only the bundled
/// `kimchi-ffmpeg.exe` (`scripts/bundle-windows.sh`).
pub fn install_of(os: &str, exe: &Path, appimage: Option<PathBuf>) -> Install {
    match os {
        "macos" => bundle_of(exe).map_or(Install::Other, Install::MacBundle),
        "linux" => match appimage.filter(|p| p.is_file()) {
            Some(path) => Install::AppImage(path),
            None if exe.starts_with("/usr/lib/kimchi") => Install::DebPackage,
            None => Install::Other,
        },
        "windows" => match exe.parent() {
            Some(dir) if dir.join("uninstall.exe").is_file() => Install::WindowsInstalled(dir.to_path_buf()),
            Some(dir) if dir.join("kimchi-ffmpeg.exe").is_file() => Install::WindowsPortable(dir.to_path_buf()),
            _ => Install::Other,
        },
        _ => Install::Other,
    }
}

/// `…/kimchi.app` for an executable at `…/kimchi.app/Contents/MacOS/<exe>`.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
}

/// Ok when this copy can replace itself, else why not.
fn install_support(install: &Install) -> Result<(), String> {
    let target: PathBuf = match install {
        Install::MacBundle(b) => {
            if b.to_string_lossy().contains("/AppTranslocation/") {
                return Err("kimchi is running from a temporary location (macOS App Translocation). Move kimchi.app to Applications and open it from there to update in one click.".into());
            }
            b.clone()
        }
        Install::AppImage(p) => p.clone(),
        Install::WindowsInstalled(dir) => dir.join("kimchi.exe"),
        Install::WindowsPortable(_) => return Err("This portable copy of kimchi is updated by hand: unpack the new portable zip over it. The installer version updates itself.".into()),
        Install::DebPackage => return Err("kimchi was installed from the .deb package, so it's updated by hand: install the new .deb. The AppImage updates itself.".into()),
        Install::Other => return Err("This copy of kimchi (a package or a build from source) is updated by hand.".into()),
    };
    let dir = target.parent().ok_or("kimchi's folder can't be found.")?;
    let probe = dir.join(format!(".kimchi-write-test-{}", uuid::Uuid::new_v4()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => Err(format!("kimchi can't write to {}, so it can't replace itself.", dir.display())),
    }
}

/// Where the replaced copy waits until the new one has started: `.<name>.previous` beside it.
pub fn previous_path(target: &Path) -> Option<PathBuf> {
    let name = target.file_name()?.to_string_lossy();
    Some(target.with_file_name(format!(".{name}.previous")))
}

/// Swaps `new` in for `target`, keeping `target` as [`previous_path`]; puts it back on failure.
fn swap_in(new: &Path, target: &Path) -> Result<(), String> {
    let previous = previous_path(target).ok_or("kimchi's location can't be found.")?;
    if previous.exists() {
        remove_any(&previous).map_err(|e| format!("Couldn't remove the copy left by the last update ({}): {e}", previous.display()))?;
    }
    std::fs::rename(target, &previous).map_err(|e| format!("Couldn't move {} aside ({e}).", target.display()))?;
    if let Err(e) = std::fs::rename(new, target) {
        let _ = std::fs::rename(&previous, target);
        return Err(format!("Couldn't move the new version into place ({e}); kimchi is unchanged."));
    }
    Ok(())
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    if path.is_dir() && !path.is_symlink() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}

/// Unpacks a verified `.app.tar.gz` (one `*.app` folder at its root) beside `bundle` and swaps it in.
pub fn install_bundle(archive: &Path, bundle: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("kimchi's folder can't be found.")?;
    let staging = parent.join(format!(".kimchi-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging).map_err(|e| format!("Couldn't unpack the update in {} ({e}).", parent.display()))?;
    let result = (|| {
        let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        tar.set_overwrite(false);
        tar.unpack(&staging).map_err(|e| format!("The update archive couldn't be unpacked ({e})."))?;
        let apps: Vec<PathBuf> = std::fs::read_dir(&staging)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.extension().is_some_and(|x| x == "app"))
            .collect();
        let [app] = apps.as_slice() else {
            return Err("The update archive doesn't hold one app bundle.".to_string());
        };
        if !app.join("Contents/Info.plist").is_file() || !app.join("Contents/MacOS/kimchi").is_file() {
            return Err("The update archive doesn't hold a complete kimchi.app.".to_string());
        }
        swap_in(app, bundle)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    // Let Launch Services see the new bundle (Info.plist, icon) on next launch.
    let _ = std::process::Command::new("touch").arg(bundle).status();
    Ok(())
}

/// Copies a verified AppImage beside `appimage` and swaps it in.
pub fn install_appimage(new: &Path, appimage: &Path) -> Result<(), String> {
    let parent = appimage.parent().ok_or("kimchi's folder can't be found.")?;
    let staged = parent.join(format!(".kimchi-update-{}.AppImage", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::copy(new, &staged).map_err(|e| format!("Couldn't write the update to {} ({e}).", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        swap_in(&staged, appimage)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

/// Removes what the last update left beside the running copy: the previous version (this one has
/// started, so it is no longer needed) and unfinished unpacking. Call once at startup, after the
/// window is up. Returns what was removed.
pub fn finish_pending() -> Vec<PathBuf> {
    match current_install() {
        Install::MacBundle(target) | Install::AppImage(target) => cleanup_beside(&target),
        Install::WindowsInstalled(_) | Install::WindowsPortable(_) | Install::DebPackage | Install::Other => vec![],
    }
}

fn cleanup_beside(target: &Path) -> Vec<PathBuf> {
    let mut removed = vec![];
    let mut leftovers: Vec<PathBuf> = previous_path(target).into_iter().filter(|p| p.exists()).collect();
    if let Some(dir) = target.parent()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        leftovers.extend(entries.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".kimchi-update-"))));
    }
    for p in leftovers {
        match remove_any(&p) {
            Ok(()) => removed.push(p),
            Err(e) => tracing::warn!("couldn't remove {}: {e}", p.display()),
        }
    }
    removed
}

/// Starts the installed copy once this process has exited (on Windows, after running a downloaded
/// installer). Call it, then quit the app.
pub fn restart() -> std::io::Result<()> {
    let install = current_install();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Wait for this process to exit so the new one doesn't race it for the bridge port.
        let (target, launch) = match &install {
            Install::MacBundle(b) => (b.clone(), r#"exec /usr/bin/open -n "$0""#),
            Install::AppImage(p) => (p.clone(), r#"exec "$0""#),
            Install::WindowsInstalled(_) | Install::WindowsPortable(_) | Install::DebPackage | Install::Other => (std::env::current_exe()?, r#"exec "$0""#),
        };
        let script = format!(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; {launch}"#);
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg(&target)
            .arg(std::process::id().to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(unix))]
    {
        let _ = install;
        match PENDING_INSTALLER.lock().take() {
            // Passive, then start kimchi again (`/R`).
            Some(setup) => after_exit(&setup, &["/P", "/R"]),
            None => after_exit(&std::env::current_exe()?, &[]),
        }
    }
}

/// When the app quits with a downloaded Windows installer waiting: runs it (passive, without
/// starting kimchi again). Does nothing elsewhere, or after [`restart`] took the installer.
pub fn apply_on_quit() {
    #[cfg(windows)]
    if let Some(setup) = PENDING_INSTALLER.lock().take() {
        tracing::info!("installing the downloaded update on quit");
        if let Err(e) = after_exit(&setup, &["/P"]) {
            tracing::warn!("couldn't start the installer: {e}");
        }
    }
}

/// Starts `program` with `args` once this process has exited, from a hidden PowerShell.
#[cfg(windows)]
fn after_exit(program: &Path, args: &[&str]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let args = if args.is_empty() { String::new() } else { format!(" -ArgumentList {}", args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(",")) };
    let script = format!(
        "try {{ Wait-Process -Id {} -Timeout 120 -ErrorAction SilentlyContinue }} catch {{}}; Start-Process -FilePath {}{args}",
        std::process::id(),
        quote(&program.to_string_lossy())
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(unix, windows)))]
fn after_exit(program: &Path, args: &[&str]) -> std::io::Result<()> {
    std::process::Command::new(program).args(args).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn versions_compare_as_semver() {
        assert!(is_newer("0.2.0", "0.1.1"));
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "1.0.0-beta.2"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.2.0-rc.1", "0.2.0"));
        assert!(!is_newer("latest", "0.1.1"));
        assert!(!is_newer("0.2.0", "dev"));
    }

    fn manifest() -> Manifest {
        let asset = |name: &str| PlatformAsset { signature: format!("sig-{name}"), url: format!("https://example.com/{name}") };
        Manifest {
            version: "0.2.0".into(),
            notes: Some("notes".into()),
            pub_date: None,
            platforms: BTreeMap::from([
                ("darwin-aarch64-app".into(), asset("kimchi_aarch64.app.tar.gz")),
                ("darwin-aarch64".into(), asset("kimchi_aarch64.app.tar.gz")),
                ("darwin-x86_64".into(), asset("kimchi_x64.app.tar.gz")),
                ("linux-x86_64".into(), asset("kimchi_amd64.AppImage")),
                ("linux-x86_64-deb".into(), asset("kimchi_amd64.deb")),
                ("windows-x86_64-nsis".into(), asset("kimchi_x64-setup.exe")),
            ]),
        }
    }

    #[test]
    fn platform_keys_go_from_specific_to_generic() {
        assert_eq!(platform_keys("darwin", "aarch64", Some("app")), ["darwin-aarch64-app", "darwin-aarch64"]);
        assert_eq!(platform_keys("linux", "x86_64", None), ["linux-x86_64"]);
        let m = manifest();
        let url = |keys: Vec<String>| select(&m, &keys).map(|a| a.url.rsplit('/').next().unwrap().to_string());
        assert_eq!(url(platform_keys("darwin", "aarch64", Some("app"))).as_deref(), Some("kimchi_aarch64.app.tar.gz"));
        // Falls back to `{os}-{arch}` when the installer has no entry of its own.
        assert_eq!(url(platform_keys("darwin", "x86_64", Some("app"))).as_deref(), Some("kimchi_x64.app.tar.gz"));
        assert_eq!(url(platform_keys("linux", "x86_64", Some("appimage"))).as_deref(), Some("kimchi_amd64.AppImage"));
        assert_eq!(url(platform_keys("linux", "x86_64", Some("deb"))).as_deref(), Some("kimchi_amd64.deb"));
        assert_eq!(url(platform_keys("windows", "x86_64", Some("nsis"))).as_deref(), Some("kimchi_x64-setup.exe"));
        assert_eq!(url(platform_keys("linux", "aarch64", None)), None);
    }

    #[test]
    fn each_install_kind_gets_its_own_file() {
        let mut m = manifest();
        let url = |m: &Manifest, install: Install, os: &str| select(m, &install_keys(&install, os, "x86_64")).map(|a| a.url.rsplit('/').next().unwrap().to_string());
        let (p, dir) = (PathBuf::from("/x"), PathBuf::from("C:/kimchi"));
        assert_eq!(url(&m, Install::AppImage(p.clone()), "linux").as_deref(), Some("kimchi_amd64.AppImage"));
        assert_eq!(url(&m, Install::DebPackage, "linux").as_deref(), Some("kimchi_amd64.deb"));
        assert_eq!(url(&m, Install::WindowsInstalled(dir.clone()), "windows").as_deref(), Some("kimchi_x64-setup.exe"));
        // A release without the portable zip: the release page, never the installer.
        assert_eq!(url(&m, Install::WindowsPortable(dir.clone()), "windows"), None);
        m.platforms.insert("windows-x86_64-portable".into(), PlatformAsset { signature: "s".into(), url: "https://example.com/kimchi_x64-portable.zip".into() });
        assert_eq!(url(&m, Install::WindowsPortable(dir), "windows").as_deref(), Some("kimchi_x64-portable.zip"));
        // Nor does a .deb fall back to the AppImage.
        m.platforms.remove("linux-x86_64-deb");
        assert_eq!(url(&m, Install::DebPackage, "linux"), None);
    }

    #[test]
    fn tells_the_install_kinds_apart() {
        assert_eq!(install_of("linux", Path::new("/usr/lib/kimchi/bin/kimchi"), None), Install::DebPackage);
        assert_eq!(install_of("linux", Path::new("/home/me/kimchi/target/release/kimchi"), None), Install::Other);
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("kimchi_amd64.AppImage");
        std::fs::write(&image, b"").unwrap();
        assert_eq!(install_of("linux", Path::new("/tmp/.mount_kimchi/usr/bin/kimchi"), Some(image.clone())), Install::AppImage(image));
        assert_eq!(install_of("macos", Path::new("/Applications/kimchi.app/Contents/MacOS/kimchi"), None), Install::MacBundle(PathBuf::from("/Applications/kimchi.app")));
        let exe = dir.path().join("kimchi.exe");
        assert_eq!(install_of("windows", &exe, None), Install::Other, "a build from source");
        std::fs::write(dir.path().join("kimchi-ffmpeg.exe"), b"").unwrap();
        assert_eq!(install_of("windows", &exe, None), Install::WindowsPortable(dir.path().to_path_buf()));
        std::fs::write(dir.path().join("uninstall.exe"), b"").unwrap();
        assert_eq!(install_of("windows", &exe, None), Install::WindowsInstalled(dir.path().to_path_buf()));
    }

    #[test]
    fn reads_the_tauri_manifest_format() {
        let json = r#"{"version":"0.1.1","notes":"n","pub_date":"2026-10-01T20:57:38.022Z","platforms":{"darwin-aarch64":{"signature":"c2ln","url":"https://api.github.com/repos/ludovic111/kimchi/releases/assets/604122959"}}}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.version, "0.1.1");
        assert_eq!(m.platforms["darwin-aarch64"].signature, "c2ln");
    }

    #[test]
    fn finds_the_app_bundle() {
        assert_eq!(bundle_of(Path::new("/Applications/kimchi.app/Contents/MacOS/kimchi")), Some(PathBuf::from("/Applications/kimchi.app")));
        assert_eq!(bundle_of(Path::new("/Applications/kimchi.app/Contents/MacOS/kimchi-cli")), Some(PathBuf::from("/Applications/kimchi.app")));
        assert_eq!(bundle_of(Path::new("/repo/target/release/kimchi")), None);
        assert_eq!(previous_path(Path::new("/Applications/kimchi.app")), Some(PathBuf::from("/Applications/.kimchi.app.previous")));
    }

    struct Keys {
        sk: minisign::SecretKey,
        pk_b64: String,
    }

    fn keys() -> Keys {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_b64 = base64::engine::general_purpose::STANDARD.encode(kp.pk.to_box().unwrap().to_string());
        Keys { sk: kp.sk, pk_b64 }
    }

    /// Signs like `kimchi-release sign` and the Tauri CLI: base64 of the signature file.
    fn sign(k: &Keys, data: &[u8], trusted: &str) -> String {
        let sig = minisign::sign(None, &k.sk, data, Some(trusted), Some("signature from tauri secret key")).unwrap();
        base64::engine::general_purpose::STANDARD.encode(sig.to_string())
    }

    fn session(dir: &Path) -> Arc<Session> {
        Session::new(crate::SessionOptions { data_dir: Some(dir.join("data")), config_dir: Some(dir.join("config")), secrets: None, headless: true }).unwrap()
    }

    fn lsuite_manifest(server: &str) -> serde_json::Value {
        let file = |name: &str| serde_json::json!({ "signature": "c2ln", "url": format!("{server}/api/apps/kimchi/files/kimchi-v99.0.0/{name}") });
        serde_json::json!({ "version": "99.0.0", "notes": "Everything is new.", "platforms": {
            "linux-x86_64": file("kimchi_amd64.AppImage"), "linux-aarch64": file("kimchi_arm64.AppImage"),
            "darwin-aarch64": file("kimchi_aarch64.app.tar.gz"), "darwin-x86_64": file("kimchi_x64.app.tar.gz"),
            "windows-x86_64": file("kimchi_x64-setup.exe"), "windows-aarch64": file("kimchi_arm64-setup.exe"),
        } })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn updates_come_from_lsuite_with_the_account_token() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/apps/kimchi/latest.json"))
            .and(header("authorization", "Bearer lsk_good"))
            .respond_with(ResponseTemplate::new(200).set_body_json(lsuite_manifest(&server.uri())))
            .mount(&server)
            .await;
        // Any other token (or none) is refused, as lsuite.xyz does.
        Mock::given(method("GET"))
            .and(path("/api/apps/kimchi/latest.json"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({ "error": { "type": "authentication_error", "message": "Sign in to lsuite to get the apps: the account is free." } })))
            .with_priority(10)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::account::testing(&dir.path().join("lsuite"), Some(&server.uri()));
        let s = session(dir.path());

        // Signed out: no request, and no failure either, even when asked by hand.
        let st = check(&s, true).await.unwrap();
        assert!(st.sign_in && st.available.is_none(), "{st:?}");
        assert_eq!(st.error.as_deref(), Some(SIGN_IN));

        let account = |token: &str| crate::account::Account { format: crate::account::FORMAT, server: server.uri(), token: token.into(), ..Default::default() };
        crate::account::save(&account("lsk_good")).unwrap();
        let st = check(&s, true).await.unwrap();
        assert_eq!(st.available.as_deref(), Some("99.0.0"), "{st:?}");
        assert!(!st.sign_in && st.error.is_none());
        assert_eq!(st.notes.as_deref(), Some("Everything is new."));
        // Downloading by hand goes through the lsuite app: the file route needs the token.
        assert_eq!(st.download_url.as_deref(), Some(format!("{}/launcher", server.uri()).as_str()));
        let found = s.update.lock().found.clone().unwrap();
        assert_eq!(found.token.as_deref(), Some("lsk_good"));
        assert!(found.url.starts_with(&server.uri()));

        // A revoked token: sign in again.
        crate::account::save(&account("lsk_revoked")).unwrap();
        let st = check(&s, true).await.unwrap();
        assert!(st.sign_in && st.available.is_none(), "{st:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_from_lsuite_carry_the_token_and_keep_the_signature_check() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let data = b"the new kimchi".repeat(1000);
        // The file route redirects to the file's own address, as lsuite.xyz does with GitHub's.
        Mock::given(method("GET"))
            .and(path("/api/apps/kimchi/files/kimchi-v99.0.0/kimchi_amd64.AppImage"))
            .and(header("authorization", "Bearer lsk_good"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", format!("{}/signed/kimchi_amd64.AppImage", server.uri())))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/signed/kimchi_amd64.AppImage")).respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone())).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path());
        let k = keys();
        let url = format!("{}/api/apps/kimchi/files/kimchi-v99.0.0/kimchi_amd64.AppImage", server.uri());
        let found = Found { version: "99.0.0".into(), url: url.clone(), signature: sign(&k, &data, "timestamp:1\tfile:kimchi_amd64.AppImage\tversion:99.0.0"), token: Some("lsk_good".into()) };
        let file = dir.path().join("kimchi.AppImage");
        download_verified(&s, &found, &file, &k.pk_b64).await.unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), data);
        // Signatures are unchanged: a file the key didn't sign is refused.
        let forged = Found { signature: sign(&keys(), &data, "timestamp:1\tfile:x\tversion:99.0.0"), ..found.clone() };
        assert!(download_verified(&s, &forged, &file, &k.pk_b64).await.is_err());
        let tampered = Found { signature: sign(&k, b"other bytes", "timestamp:1\tfile:x\tversion:99.0.0"), ..found };
        assert!(download_verified(&s, &tampered, &file, &k.pk_b64).await.unwrap_err().contains("doesn't match"));
    }

    #[test]
    fn the_token_goes_only_to_the_lsuite_server() {
        assert!(same_origin("https://lsuite.xyz/api/apps/kimchi/latest.json", "https://lsuite.xyz"));
        assert!(same_origin("http://127.0.0.1:4331/x", "http://127.0.0.1:4331/"));
        assert!(!same_origin("https://example.com/latest.json", "https://lsuite.xyz"));
        assert!(!same_origin("http://127.0.0.1:4332/x", "http://127.0.0.1:4331"));
    }

    #[test]
    fn accepts_a_good_signature() {
        let k = keys();
        let data = b"the new kimchi".repeat(10_000);
        let sig = sign(&k, &data, "timestamp:1790886104\tfile:kimchi_aarch64.app.tar.gz\tversion:0.2.0");
        verify(&data[..], &sig, &k.pk_b64, "0.2.0").unwrap();
        verify(&data[..], &sig, &k.pk_b64, "v0.2.0").unwrap();
    }

    #[test]
    fn refuses_bad_signatures() {
        let k = keys();
        let data = b"the new kimchi".to_vec();
        let sig = sign(&k, &data, "timestamp:1\tfile:x\tversion:0.2.0");
        // Tampered data.
        let err = verify(&b"the new kimchi!"[..], &sig, &k.pk_b64, "0.2.0").unwrap_err();
        assert!(err.contains("doesn't match"), "{err}");
        // Signed by someone else.
        let err = verify(&data[..], &sig, &keys().pk_b64, "0.2.0").unwrap_err();
        assert!(err.contains("different key"), "{err}");
        // A real archive replayed as another version.
        let err = verify(&data[..], &sig, &k.pk_b64, "0.3.0").unwrap_err();
        assert!(err.contains("signed for version 0.2.0"), "{err}");
        // No version in the trusted comment.
        let unversioned = sign(&k, &data, "timestamp:1\tfile:x");
        assert!(verify(&data[..], &unversioned, &k.pk_b64, "0.2.0").is_err());
        // Garbage.
        assert!(verify(&data[..], "not base64!", &k.pk_b64, "0.2.0").is_err());
    }

    /// kimchi 0.1.1's Windows installer signature, from its `latest.json`: it must be checked against
    /// the key built in here (wrong data, so the signature fails, but not the key id).
    #[test]
    fn the_built_in_key_is_the_release_key() {
        const SIG_0_1_1: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTbjJWZndlQlVIVEJlaTd5UjZvWHM3Q0d4cFZvdTlnSTF0cVlNSHFhQU03NnV5TEZLOWNDejROd3lCaUlrVS84eHJkYTY2YlM4TmxIR0V1akZZSXJkRmxEZFRoV2VuY0E0PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkwODg2MTA0CWZpbGU6a2ltY2hpXzAuMS4xX3g2NF9lbi1VUy5tc2kJdmVyc2lvbjowLjEuMQo4b0xmTDE2TXA5Y3VqUnZBWDJvd1Q4bVRkbG1GQmpGNGE5cGxKWng4MnpwR0p5UUdjcVFpOHZZMWVPTHVKbUs1TzdTa2RLRDNhZnM2RENKZkYzdkJBUT09Cg==";
        let err = verify(&b"not the msi"[..], SIG_0_1_1, PUBLIC_KEY, "0.1.1").unwrap_err();
        assert!(err.contains("doesn't match"), "{err}");
    }

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(vec![], flate2::Compression::fast()));
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, path, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn write(path: &Path, data: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(path).unwrap().write_all(data).unwrap();
    }

    #[test]
    fn replaces_the_bundle_and_keeps_the_previous_copy() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("kimchi.app");
        write(&bundle.join("Contents/MacOS/kimchi"), b"old");
        write(&bundle.join("Contents/Info.plist"), b"old plist");
        let archive = dir.path().join("kimchi_aarch64.app.tar.gz");
        std::fs::write(&archive, tar_gz(&[("kimchi.app/Contents/Info.plist", b"new plist"), ("kimchi.app/Contents/MacOS/kimchi", b"new")])).unwrap();

        install_bundle(&archive, &bundle).unwrap();
        assert_eq!(std::fs::read(bundle.join("Contents/MacOS/kimchi")).unwrap(), b"new");
        let previous = dir.path().join(".kimchi.app.previous");
        assert_eq!(std::fs::read(previous.join("Contents/MacOS/kimchi")).unwrap(), b"old");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(bundle.join("Contents/MacOS/kimchi")).unwrap().permissions().mode() & 0o111, 0o111);
        }

        // The new version started: the previous copy goes.
        assert_eq!(cleanup_beside(&bundle), vec![previous.clone()]);
        assert!(!previous.exists() && bundle.exists());
    }

    #[test]
    fn a_broken_archive_leaves_the_bundle_alone() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("kimchi.app");
        write(&bundle.join("Contents/MacOS/kimchi"), b"old");
        let archive = dir.path().join("update.tar.gz");
        std::fs::write(&archive, tar_gz(&[("README.txt", b"no app here")])).unwrap();
        assert!(install_bundle(&archive, &bundle).is_err());
        std::fs::write(&archive, b"not a tarball").unwrap();
        assert!(install_bundle(&archive, &bundle).is_err());
        assert_eq!(std::fs::read(bundle.join("Contents/MacOS/kimchi")).unwrap(), b"old");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.starts_with('.')), "{names:?}");
    }

    #[test]
    fn replaces_an_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let appimage = dir.path().join("kimchi.AppImage");
        std::fs::write(&appimage, b"old").unwrap();
        let new = dir.path().join("download");
        std::fs::write(&new, b"new").unwrap();
        install_appimage(&new, &appimage).unwrap();
        assert_eq!(std::fs::read(&appimage).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.path().join(".kimchi.AppImage.previous")).unwrap(), b"old");
        assert_eq!(cleanup_beside(&appimage).len(), 1);
    }
}
