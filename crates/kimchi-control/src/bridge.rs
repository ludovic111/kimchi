//! Live control of the running app over a loopback socket.
//!
//! The app listens on 127.0.0.1 (never beyond) and writes `{version, port,
//! token, pid}` to `control.json` in its data folder, readable only by the
//! current user (0600). `KIMCHI_CONTROL` overrides the path. Clients send
//! newline-delimited JSON-RPC 2.0: first `auth` with the token (and who they
//! are), then any command name from the registry. The same protocol as
//! ryolune's bridge, so suite tools can drive both.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::session::{CmdResult, Session, Source};

pub const VERSION: u32 = 1;
const MAX_LINE: usize = 64 * 1024 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Discovery {
    pub version: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

/// `$KIMCHI_CONTROL`, else `<data dir>/control.json`.
pub fn control_path(data_dir: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("KIMCHI_CONTROL").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    data_dir.join("control.json")
}

/// Where a client looks for the running app.
pub fn default_control_path() -> PathBuf {
    control_path(&crate::session::default_data_dir())
}

pub fn read_discovery(path: &Path) -> CmdResult<Discovery> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| format!("kimchi isn't running (no control file at {}). Start the app, or work on a file with --file.", path.display()))?;
    let d: Discovery = serde_json::from_str(&text).map_err(|e| format!("Invalid control file {}: {e}", path.display()))?;
    if d.version != VERSION {
        return Err(format!("kimchi's control protocol {} isn't supported by this client ({VERSION}); update one of them.", d.version));
    }
    Ok(d)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

/// A random token from the OS's randomness (through `RandomState`) and the clock.
pub fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    (0..4u64)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            h.write_u128(nanos);
            h.write_u32(std::process::id());
            format!("{:016x}", h.finish())
        })
        .collect()
}

fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The listener in the running app. Dropping it removes the control file.
pub struct Server {
    path: PathBuf,
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    pub async fn start(session: Arc<Session>) -> std::io::Result<Self> {
        let path = control_path(&session.data_dir);
        Self::start_at(session, path).await
    }

    pub async fn start_at(session: Arc<Session>, path: PathBuf) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let token = token();
        let d = Discovery { version: VERSION, port, token: token.clone(), pid: std::process::id() };
        write_private(&path, &serde_json::to_vec_pretty(&d).map_err(std::io::Error::other)?)?;
        *session.bridge_port.lock() = Some(port);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = listener.accept().await else { continue };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let (session, token) = (session.clone(), token.clone());
                tokio::spawn(async move {
                    if let Err(e) = connection(stream, &session, &token).await {
                        tracing::debug!("bridge client: {e}");
                    }
                });
            }
        });
        Ok(Self { path, port, task })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        // Only remove our own file: another instance may have replaced it.
        if read_discovery(&self.path).is_ok_and(|d| d.pid == std::process::id()) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

fn reply(id: &Value, result: CmdResult) -> Value {
    match result {
        Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": e } }),
    }
}

async fn connection(stream: TcpStream, session: &Arc<Session>, token: &str) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    // The first line must authenticate.
    let first = match tokio::time::timeout(AUTH_TIMEOUT, lines.next_line()).await {
        Ok(Ok(Some(l))) => l,
        _ => return Ok(()),
    };
    let req: Request = match serde_json::from_str(&first) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let given = req.params.get("token").and_then(Value::as_str).unwrap_or("");
    if req.method != "auth" || !same_token(given, token) {
        let line = reply(&req.id, Err("Not authorised: the token doesn't match the running kimchi.".into()));
        write.write_all(format!("{line}\n").as_bytes()).await?;
        return Ok(());
    }
    let source = match req.params.get("client").and_then(Value::as_str) {
        Some("mcp") => Source::Mcp,
        Some("agent") => Source::Agent,
        _ if req.params.get("agent").and_then(Value::as_bool) == Some(true) => Source::Agent,
        _ => Source::Cli,
    };
    let hello = json!({ "app": "kimchi", "version": env!("CARGO_PKG_VERSION"), "protocol": VERSION });
    write.write_all(format!("{}\n", reply(&req.id, Ok(hello))).as_bytes()).await?;

    while let Some(line) = lines.next_line().await? {
        if line.len() > MAX_LINE {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let out = match serde_json::from_str::<Request>(&line) {
            Ok(r) => reply(&r.id, crate::registry::call(session, source, &r.method, r.params).await),
            Err(e) => reply(&Value::Null, Err(format!("Invalid request: {e}"))),
        };
        write.write_all(format!("{out}\n").as_bytes()).await?;
    }
    Ok(())
}

/// A connection to the running app.
pub struct Client {
    lines: tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
    write: tokio::net::tcp::OwnedWriteHalf,
    next: u64,
}

impl Client {
    /// Connects using the control file. `client` is `cli`, `mcp` or `agent`.
    pub async fn connect(path: &Path, client: &str) -> CmdResult<Self> {
        let d = read_discovery(path)?;
        let stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(("127.0.0.1", d.port)))
            .await
            .map_err(|_| "kimchi didn't answer on its control port.".to_string())?
            .map_err(|e| format!("kimchi isn't running (its control port refused: {e}). Start the app, or use --file."))?;
        let (read, write) = stream.into_split();
        let mut c = Self { lines: BufReader::new(read).lines(), write, next: 1 };
        c.request("auth", json!({ "token": d.token, "client": client })).await?;
        Ok(c)
    }

    pub async fn call(&mut self, command: &str, params: Value) -> CmdResult {
        self.request(command, params).await
    }

    async fn request(&mut self, method: &str, params: Value) -> CmdResult {
        let id = self.next;
        self.next += 1;
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write.write_all(format!("{line}\n").as_bytes()).await.map_err(|e| format!("Lost the connection to kimchi: {e}"))?;
        let answer = self.lines.next_line().await.map_err(|e| format!("Lost the connection to kimchi: {e}"))?.ok_or("kimchi closed the connection.")?;
        let v: Value = serde_json::from_str(&answer).map_err(|e| format!("Invalid answer from kimchi: {e}"))?;
        if let Some(e) = v.get("error") {
            return Err(e.get("message").and_then(Value::as_str).unwrap_or("error").to_string());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}
