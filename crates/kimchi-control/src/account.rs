//! lsuite AI: the one lsuite account every app on this computer shares (`AI.md` in the lsuite
//! repository). Signing in from kimchi signs in ryolune too, and the other way round.
//!
//! - The account lives in `~/.lsuite/account.json` (`LSUITE_HOME` replaces `~/.lsuite`), mode
//!   0600, written atomically: `{format: 1, server, email, name, plan, token, signedInAt}`. It is
//!   read whenever it is needed (another app may have changed it). The token is a secret: it is
//!   never logged, never put in a command's answer, and masked in `Debug`.
//! - The server is `https://lsuite.xyz`; `LSUITE_ACCOUNT_SERVER` replaces it (a local demo server,
//!   tests).
//! - Signing in works like native apps do OAuth: kimchi listens on `127.0.0.1:<random port>`,
//!   opens `<server>/account/connect?app=kimchi&port=<port>&state=<random>` in the browser, and
//!   waits for the browser to come back to `/callback?code=…&state=…`; the code is exchanged for a
//!   token (`POST /api/account/token`). CLIs and headless machines paste the key the account page
//!   shows instead (`lsk_…`).
//! - The agent reaches the models with its Anthropic provider at `<server>/api/ai` with the token
//!   as its key (`kimchi_agent`'s `lsuite` provider), and Claude Code through
//!   `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` when the person picks that.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::session::CmdResult;

pub const FORMAT: u32 = 1;
pub const DEFAULT_SERVER: &str = "https://lsuite.xyz";
/// The app's name in the sign-in page's address (`app=kimchi`).
pub const APP: &str = "kimchi";
/// How long a sign-in waits for the browser.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long a question to the account server may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// The shared account file.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub format: u32,
    /// Where the account lives, without a trailing slash.
    pub server: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// `free`, `plus`, `pro`, `studio` (as the server says).
    #[serde(default)]
    pub plan: String,
    pub token: String,
    #[serde(default)]
    pub signed_in_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("server", &self.server)
            .field("email", &self.email)
            .field("plan", &self.plan)
            .field("token", &mask(&self.token))
            .finish()
    }
}

/// `lsk_…a1b2`: enough to tell two keys apart, never enough to use one.
pub fn mask(token: &str) -> String {
    let tail: String = token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    let head: String = token.chars().take_while(|c| *c != '_').take(4).collect();
    if token.len() <= 8 { "…".into() } else if token.contains('_') { format!("{head}_…{tail}") } else { format!("…{tail}") }
}

/// Where tests put the account and which server they talk to ([`testing`]).
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

/// The server named by `LSUITE_ACCOUNT_SERVER` (or a test's).
fn env_server() -> Option<String> {
    if let Some((_, server)) = OVERRIDE.read().clone() {
        return server;
    }
    std::env::var("LSUITE_ACCOUNT_SERVER").ok().map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty())
}

/// The server a new sign-in goes to.
fn sign_in_server() -> String {
    env_server().unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

/// For tests: the account lives in `home` and the server is `server`, until the guard is dropped.
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

pub fn account_path() -> PathBuf {
    lsuite_home().join("account.json")
}

/// The account server: `LSUITE_ACCOUNT_SERVER`, else the signed-in account's, else lsuite.xyz.
pub fn server() -> String {
    if let Some(s) = env_server() {
        return s;
    }
    load().map(|a| a.server).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

/// The base URL the Anthropic-speaking endpoint lives at (`<server>/api/ai`; requests go to
/// `<base>/v1/messages`).
pub fn ai_base(server: &str) -> String {
    format!("{}/api/ai", server.trim_end_matches('/'))
}

/// Where the person manages their plan.
pub fn manage_url(server: &str) -> String {
    format!("{}/account", server.trim_end_matches('/'))
}

/// The signed-in account, if there is one (a file of another format or without a token is no
/// account).
pub fn load() -> Option<Account> {
    load_from(&account_path())
}

fn load_from(path: &Path) -> Option<Account> {
    let bytes = std::fs::read(path).ok()?;
    let a: Account = serde_json::from_slice(&bytes).ok()?;
    (a.format == FORMAT && !a.token.trim().is_empty()).then_some(a)
}

/// Writes the account atomically, readable by this user only.
pub fn save(account: &Account) -> CmdResult<()> {
    save_to(&account_path(), account)
}

fn save_to(path: &Path, account: &Account) -> CmdResult<()> {
    let dir = path.parent().ok_or("no folder for the account")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(account).map_err(crate::session::err)?;
    let write = || -> std::io::Result<()> {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        use std::io::Write;
        let mut f = o.open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't save the lsuite account in {}: {e}", path.display())
    })
}

/// Forgets the account on this computer (every lsuite app is signed out).
pub fn forget() -> CmdResult<bool> {
    let path = account_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("Couldn't remove {}: {e}", path.display())),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("kimchi/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// What `GET /api/account/me` says, in the shape the window and commands use.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub used: f64,
    pub limit: f64,
    /// When the allowance starts again (as the server writes it, RFC 3339).
    pub resets_at: Option<String>,
}

impl Usage {
    /// Whole percent used (0 with no allowance).
    pub fn percent(&self) -> u32 {
        if self.limit <= 0.0 { 0 } else { ((self.used / self.limit) * 100.0).round().clamp(0.0, 999.0) as u32 }
    }

    pub fn exhausted(&self) -> bool {
        self.limit > 0.0 && self.used >= self.limit
    }
}

/// The account as the window shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub signed_in: bool,
    pub server: String,
    pub email: String,
    pub name: String,
    /// `free`, `plus`, `pro`, `studio`; empty when signed out.
    pub plan: String,
    /// `active`, … as the server says (empty when it couldn't be asked).
    pub status: String,
    pub usage: Option<Usage>,
    /// The models the plan includes, for the agent's model list.
    pub models: Vec<String>,
    /// The plan's default model, when the server names one.
    pub default_model: Option<String>,
    /// `Pro · 38 % used · resets 1 Nov`.
    pub summary: String,
    /// The plan includes models (a paid plan): the agent can run on it.
    pub can_run: bool,
    /// Where to manage the plan.
    pub manage_url: String,
    /// Why the server couldn't be asked, or what is wrong with the sign-in.
    pub error: Option<String>,
    /// The sign-in was refused (expired or revoked): sign in again.
    pub expired: bool,
}

/// The plan's name as people read it (`pro` → `Pro`).
pub fn plan_label(plan: &str) -> String {
    let p = plan.trim();
    let mut c = p.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => "No plan".into(),
    }
}

/// `1 Nov` from an RFC 3339 time.
fn short_date(at: &str) -> Option<String> {
    let t = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(t.format("%-d %b").to_string())
}

/// `Pro · 38 % used · resets 1 Nov`.
pub fn summary(plan: &str, usage: Option<&Usage>) -> String {
    let mut s = plan_label(plan);
    if let Some(u) = usage.filter(|u| u.limit > 0.0) {
        s.push_str(&format!(" · {} % used", u.percent()));
        if let Some(d) = u.resets_at.as_deref().and_then(short_date) {
            s.push_str(&format!(" · resets {d}"));
        }
    }
    s
}

fn is_free(plan: &str) -> bool {
    matches!(plan.trim().to_ascii_lowercase().as_str(), "" | "free" | "none")
}

/// Reads `/api/account/me` for `account`. Network errors come back as `Err`.
pub async fn me(account: &Account) -> Result<Value, (u16, String)> {
    let url = format!("{}/api/account/me", account.server.trim_end_matches('/'));
    let r = client().get(&url).bearer_auth(&account.token).send().await.map_err(|e| (0, format!("Couldn't reach {}: {}", host(&account.server), short_error(&e))))?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err((status, error_text(&text)));
    }
    serde_json::from_str(&text).map_err(|_| (status, "The account server's answer wasn't JSON.".into()))
}

fn short_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "it didn't answer in time".into()
    } else if e.is_connect() {
        "it can't be reached".into()
    } else {
        e.to_string()
    }
}

fn host(server: &str) -> &str {
    server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/')
}

/// The readable part of an error body.
pub fn error_text(text: &str) -> String {
    let v: Option<Value> = serde_json::from_str(text).ok();
    v.as_ref()
        .and_then(|v| v["error"]["message"].as_str().or_else(|| v["error"].as_str()).or_else(|| v["message"].as_str()).map(str::to_string))
        .unwrap_or_else(|| text.trim().chars().take(300).collect())
}

/// The account's status: the file, and (unless `offline`) what the server says about the plan and
/// its allowance. Never fails: problems are in `error`.
pub async fn status(offline: bool) -> Status {
    let Some(a) = load() else {
        let server = server();
        return Status { server: server.clone(), manage_url: manage_url(&server), summary: "Signed out".into(), ..Default::default() };
    };
    let mut s = Status {
        signed_in: true,
        server: a.server.clone(),
        email: a.email.clone(),
        name: a.name.clone(),
        plan: a.plan.clone(),
        manage_url: manage_url(&a.server),
        can_run: !is_free(&a.plan),
        ..Default::default()
    };
    if !offline {
        match me(&a).await {
            Ok(v) => {
                let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
                if !text("email").is_empty() {
                    s.email = text("email");
                }
                if !text("name").is_empty() {
                    s.name = text("name");
                }
                let plan = match &v["plan"] {
                    Value::String(p) => p.clone(),
                    Value::Object(o) => o.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    _ => String::new(),
                };
                s.plan = plan;
                s.status = text("status");
                let u = &v["usage"];
                if u.is_object() {
                    s.usage = Some(Usage {
                        used: u["used"].as_f64().unwrap_or(0.0),
                        limit: u["limit"].as_f64().unwrap_or(0.0),
                        resets_at: u["resetsAt"].as_str().map(str::to_string),
                    });
                }
                s.default_model = v["defaultModel"].as_str().map(str::to_string);
                s.models = v["models"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_string).or_else(|| m["id"].as_str().map(str::to_string))).collect();
                s.can_run = !is_free(&s.plan) && !matches!(s.status.as_str(), "none" | "canceled" | "cancelled" | "inactive");
                // Keep the file's plan in step for the other apps' offline view.
                if s.plan != a.plan || s.email != a.email || s.name != a.name {
                    let mut fresh = a.clone();
                    fresh.plan = s.plan.clone();
                    fresh.email = s.email.clone();
                    fresh.name = s.name.clone();
                    if load().is_some_and(|now| now.token == a.token) {
                        let _ = save(&fresh);
                    }
                }
            }
            Err((401 | 403, _)) => {
                s.expired = true;
                s.can_run = false;
                s.error = Some("Your lsuite sign-in has expired or was revoked. Sign in again.".into());
            }
            Err((_, e)) => s.error = Some(e),
        }
    }
    s.summary = summary(&s.plan, s.usage.as_ref());
    s
}

/// `GET /api/ai/plans`, as the server says it (the source of truth for plans and prices).
pub async fn plans() -> CmdResult<Value> {
    let server = server();
    let url = format!("{server}/api/ai/plans");
    let r = client().get(&url).send().await.map_err(|e| format!("Couldn't reach {}: {}", host(&server), short_error(&e)))?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(format!("{} answered {status}: {}", host(&server), error_text(&text)));
    }
    let mut v: Value = serde_json::from_str(&text).map_err(|_| "The plans weren't JSON.".to_string())?;
    if v.is_object() {
        v["manageUrl"] = json!(manage_url(&server));
    }
    Ok(v)
}

/// The account from a sign-in answer (`{token, account}`), or a key and `/me`'s answer.
fn account_from(server: &str, token: &str, info: &Value) -> Account {
    let text = |k: &str| info[k].as_str().unwrap_or("").to_string();
    let plan = match &info["plan"] {
        Value::String(p) => p.clone(),
        Value::Object(o) => o.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    };
    Account { format: FORMAT, server: server.to_string(), email: text("email"), name: text("name"), plan, token: token.to_string(), signed_in_at: Some(chrono::Utc::now()) }
}

/// Signs in with a key from the account page (`lsk_…`): checks it with the server, then saves it.
pub async fn sign_in_with_key(key: &str) -> CmdResult<Account> {
    let key = key.trim();
    if key.is_empty() {
        return Err("The key is empty. Copy it from your account page.".into());
    }
    let server = sign_in_server();
    let probe = Account { format: FORMAT, server: server.clone(), token: key.to_string(), ..Default::default() };
    let info = me(&probe).await.map_err(|(code, e)| match code {
        401 | 403 => format!("{} doesn't know this key. Copy it again from {}.", host(&server), manage_url(&server)),
        _ => e,
    })?;
    let account = account_from(&server, key, &info);
    save(&account)?;
    Ok(account)
}

/// The sign-in waiting for the browser, so a new one (or a cancel) stops it.
fn pending() -> &'static Mutex<Option<(String, CancellationToken)>> {
    static P: OnceLock<Mutex<Option<(String, CancellationToken)>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Stops a sign-in that waits for the browser. True if there was one.
pub fn cancel_sign_in() -> bool {
    pending().lock().take().map(|(_, t)| t.cancel()).is_some()
}

/// A sign-in is waiting for the browser.
pub fn signing_in() -> bool {
    pending().lock().is_some()
}

/// A loopback sign-in, started: open `url` in the browser, then await `finish`.
pub struct SignIn {
    pub url: String,
    listener: tokio::net::TcpListener,
    state: String,
    server: String,
    cancel: CancellationToken,
}

/// Starts listening and makes the address to open (not opened here).
pub async fn begin_sign_in() -> CmdResult<SignIn> {
    let server = sign_in_server();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| format!("Couldn't listen for the sign-in: {e}"))?;
    let port = listener.local_addr().map_err(crate::session::err)?.port();
    let state = uuid::Uuid::new_v4().simple().to_string();
    let url = format!("{server}/account/connect?app={APP}&port={port}&state={state}");
    let cancel = CancellationToken::new();
    if let Some((_, old)) = pending().lock().replace((state.clone(), cancel.clone())) {
        old.cancel();
    }
    Ok(SignIn { url, listener, state, server, cancel })
}

impl SignIn {
    /// Waits for the browser's callback, exchanges the code and saves the account.
    pub async fn finish(self, timeout: Duration) -> CmdResult<Account> {
        let cancel = self.cancel.clone();
        let self_state = self.state.clone();
        let result = tokio::select! {
            r = self.wait() => r,
            _ = tokio::time::sleep(timeout) => Err("The sign-in wasn't finished in the browser in time. Try again.".into()),
            _ = cancel.cancelled() => Err("Sign-in cancelled.".into()),
        };
        let mut p = pending().lock();
        if p.as_ref().is_some_and(|(state, _)| *state == self_state) {
            *p = None;
        }
        result
    }

    async fn wait(&self) -> CmdResult<Account> {
        loop {
            let (mut stream, _) = self.listener.accept().await.map_err(|e| format!("The sign-in listener stopped: {e}"))?;
            let mut buf = vec![0u8; 8192];
            let mut n = 0;
            // The request line and headers (a callback has no body).
            while n < buf.len() {
                let read = match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf[n..])).await {
                    Ok(Ok(r)) => r,
                    _ => break,
                };
                if read == 0 {
                    break;
                }
                n += read;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("").to_string();
            let Some(query) = target.strip_prefix("/callback") else {
                let _ = respond(&mut stream, 404, "Not here", "This address only finishes an lsuite sign-in.").await;
                continue;
            };
            let params: Vec<(String, String)> = url::form_urlencoded::parse(query.trim_start_matches('?').as_bytes()).into_owned().collect();
            let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            if get("state").as_deref() != Some(self.state.as_str()) {
                let _ = respond(&mut stream, 400, "Sign-in not recognised", "This sign-in wasn't started by kimchi just now. Start it again from kimchi.").await;
                continue;
            }
            if let Some(e) = get("error") {
                let _ = respond(&mut stream, 200, "Sign-in cancelled", "You can close this tab and go back to kimchi.").await;
                return Err(format!("The sign-in was cancelled in the browser ({e})."));
            }
            let Some(code) = get("code").filter(|c| !c.is_empty()) else {
                let _ = respond(&mut stream, 400, "Something is missing", "The browser came back without a code. Start the sign-in again from kimchi.").await;
                return Err("The browser came back without a sign-in code.".into());
            };
            return match exchange(&self.server, &code).await {
                Ok(account) => {
                    let _ = respond(&mut stream, 200, "kimchi is connected", "You can close this tab and go back to kimchi. Your other lsuite apps are signed in too.").await;
                    Ok(account)
                }
                Err(e) => {
                    let _ = respond(&mut stream, 500, "kimchi couldn't connect", &e).await;
                    Err(e)
                }
            };
        }
    }
}

/// `POST /api/account/token {code}` → `{token, account}`, saved.
async fn exchange(server: &str, code: &str) -> CmdResult<Account> {
    let url = format!("{server}/api/account/token");
    let r = client().post(&url).json(&json!({ "code": code })).send().await.map_err(|e| format!("Couldn't reach {}: {}", host(server), short_error(&e)))?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(format!("{} refused the sign-in ({status}): {}", host(server), error_text(&text)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| "The sign-in answer wasn't JSON.".to_string())?;
    let token = v["token"].as_str().filter(|t| !t.is_empty()).ok_or("The sign-in answer had no token.")?;
    let account = account_from(server, token, &v["account"]);
    save(&account)?;
    Ok(account)
}

/// A small page for the browser tab, in the lsuite look (black and white).
async fn respond(stream: &mut tokio::net::TcpStream, code: u16, title: &str, text: &str) -> std::io::Result<()> {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{t} · lsuite</title><style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#050505;color:#f2f2f2;font:15px/1.5 system-ui,sans-serif}}main{{border:1px solid #2a2a2a;padding:32px 36px;max-width:420px;box-shadow:4px 4px 0 #000}}h1{{font-size:22px;margin:0 0 8px}}p{{color:#a8a8a8;margin:0}}@media (prefers-color-scheme:light){{body{{background:#f0f0f0;color:#0a0a0a}}main{{border-color:#c8c8c8;background:#fbfbfb;box-shadow:4px 4px 0 #0a0a0a}}p{{color:#4d4d4d}}}}</style></head><body><main><h1>{t}</h1><p>{x}</p></main></body></html>",
        t = esc(title),
        x = esc(text)
    );
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n", body.len());
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await
}

/// Signs out: tells the server (best effort) and removes the account file.
pub async fn sign_out() -> CmdResult<bool> {
    cancel_sign_in();
    let Some(a) = load() else { return Ok(false) };
    let url = format!("{}/api/account/signout", a.server.trim_end_matches('/'));
    if let Err(e) = client().post(&url).bearer_auth(&a.token).send().await {
        tracing::info!("lsuite sign-out: the server wasn't told ({})", short_error(&e));
    }
    forget()
}

/// Opens `url` in the person's browser. `KIMCHI_NO_BROWSER=1` (tests, scripts) only says it.
pub fn open_browser(url: &str) -> bool {
    if is_test_binary() || std::env::var("KIMCHI_NO_BROWSER").is_ok_and(|v| !v.is_empty() && v != "0") {
        return false;
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().is_ok()
}

/// What an account's answer may show (never the token).
pub fn public(a: &Account) -> Value {
    json!({ "server": a.server, "email": a.email, "name": a.name, "plan": a.plan, "signedInAt": a.signed_in_at, "key": mask(&a.token) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_masked_and_the_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.json");
        let a = Account { format: FORMAT, server: "http://127.0.0.1:1".into(), email: "a@b.c".into(), name: "A".into(), plan: "pro".into(), token: "lsk_secretsecret1234".into(), signed_in_at: None };
        save_to(&path, &a).unwrap();
        assert_eq!(load_from(&path).unwrap(), a);
        let shown = format!("{a:?}");
        assert!(!shown.contains("secretsecret") && shown.contains("lsk_…1234"), "{shown}");
        assert!(!public(&a).to_string().contains("secretsecret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Another format, or no token, is no account.
        std::fs::write(&path, r#"{"format":2,"server":"x","token":"t"}"#).unwrap();
        assert!(load_from(&path).is_none());
        std::fs::write(&path, r#"{"format":1,"server":"x","token":""}"#).unwrap();
        assert!(load_from(&path).is_none());
    }

    #[test]
    fn summaries_read_like_the_contract() {
        let u = Usage { used: 380.0, limit: 1000.0, resets_at: Some("2026-11-01T00:00:00Z".into()) };
        assert_eq!(summary("pro", Some(&u)), "Pro · 38 % used · resets 1 Nov");
        assert_eq!(summary("", None), "No plan");
        assert!(Usage { used: 1000.0, limit: 1000.0, resets_at: None }.exhausted());
        assert_eq!(ai_base("https://lsuite.xyz/"), "https://lsuite.xyz/api/ai");
    }

    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn me_body() -> Value {
        json!({ "email": "ada@example.com", "name": "Ada", "plan": "pro", "status": "active", "usage": { "used": 1520, "limit": 4000, "resetsAt": "2026-11-01T00:00:00Z" }, "models": ["claude-sonnet-5-5", "claude-opus-5-5"] })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_loopback_sign_in_saves_the_account_for_every_app() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/account/token"))
            .and(body_json(json!({ "code": "c0de" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "token": "lsk_live_abcdef123456", "account": { "email": "ada@example.com", "name": "Ada", "plan": "pro" } })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/api/account/me")).and(header("authorization", "Bearer lsk_live_abcdef123456")).respond_with(ResponseTemplate::new(200).set_body_json(me_body())).mount(&server).await;
        Mock::given(method("POST")).and(path("/api/account/signout")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true }))).expect(1).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let _env = testing(dir.path(), Some(&server.uri()));

        let pending = begin_sign_in().await.unwrap();
        assert!(signing_in());
        let url = url::Url::parse(&pending.url).unwrap();
        assert_eq!(url.path(), "/account/connect");
        let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["app"], "kimchi");
        let (port, state) = (q["port"].clone(), q["state"].clone());
        let finish = tokio::spawn(pending.finish(Duration::from_secs(20)));
        // The browser: a stranger's callback is turned away, then the real one comes.
        let http = reqwest::Client::new();
        let wrong = http.get(format!("http://127.0.0.1:{port}/callback?code=evil&state=nope")).send().await.unwrap();
        assert_eq!(wrong.status().as_u16(), 400);
        let page = http.get(format!("http://127.0.0.1:{port}/callback?code=c0de&state={state}")).send().await.unwrap();
        assert_eq!(page.status().as_u16(), 200);
        assert!(page.text().await.unwrap().contains("kimchi is connected"));
        let account = finish.await.unwrap().unwrap();
        assert_eq!((account.email.as_str(), account.plan.as_str()), ("ada@example.com", "pro"));
        assert!(!signing_in());
        assert_eq!(load().unwrap().token, "lsk_live_abcdef123456");
        assert!(dir.path().join("account.json").is_file());

        let st = status(false).await;
        assert!(st.signed_in && st.can_run && !st.expired);
        assert_eq!(st.summary, "Pro · 38 % used · resets 1 Nov");
        assert_eq!(st.models, ["claude-sonnet-5-5", "claude-opus-5-5"]);
        assert_eq!(st.manage_url, format!("{}/account", server.uri()));
        assert!(!serde_json::to_string(&st).unwrap().contains("lsk_live"), "the status never carries the key");

        assert!(sign_out().await.unwrap());
        assert!(load().is_none());
        assert!(!status(true).await.signed_in);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pasted_key_is_checked_before_it_is_kept() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/account/me")).and(header("authorization", "Bearer lsk_good")).respond_with(ResponseTemplate::new(200).set_body_json(me_body())).mount(&server).await;
        Mock::given(method("GET")).and(path("/api/account/me")).respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": { "message": "unknown key" } }))).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let _env = testing(dir.path(), Some(&server.uri()));
        let e = sign_in_with_key("lsk_bad").await.unwrap_err();
        assert!(e.contains("doesn't know this key"), "{e}");
        assert!(load().is_none());
        let a = sign_in_with_key(" lsk_good ").await.unwrap();
        assert_eq!((a.token.as_str(), a.plan.as_str()), ("lsk_good", "pro"));
        // A revoked key: the status says to sign in again.
        let mut revoked = a.clone();
        revoked.token = "lsk_revoked".into();
        save(&revoked).unwrap();
        let st = status(false).await;
        assert!(st.signed_in && st.expired && !st.can_run);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancelled_sign_in_stops_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let _env = testing(dir.path(), Some("http://127.0.0.1:9"));
        let pending = begin_sign_in().await.unwrap();
        let finish = tokio::spawn(pending.finish(Duration::from_secs(20)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(cancel_sign_in());
        assert!(finish.await.unwrap().unwrap_err().contains("cancelled"));
        assert!(load().is_none());
    }
}
