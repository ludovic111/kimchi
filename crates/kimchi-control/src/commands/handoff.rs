//! Hand-offs between lsuite apps, as plain files (lsuite STANDARD.md §4):
//!
//! * kimchi → ryolune: the cut's audio as a WAV, plus `<name>.kimchi-cut.json`
//!   next to it with its length and markers. When ryolune is running, kimchi
//!   imports the WAV there and adds the markers through ryolune's own bridge
//!   (`~/.ryolune/control.json`, the same JSON-RPC protocol as kimchi's).
//! * ryolune → kimchi: an audio file onto an audio track; or, when ryolune is
//!   running, a fresh bounce of its open song.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kimchi_core::Edit;
use kimchi_media::export::{ExportFormat, ExportSettings, Quality};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "handoff.apps" => Ok(json!(crate::discovery::installed_apps().into_iter().map(|app| {
            let mut v = json!(app);
            v["isRunning"] = json!(app.running.is_some());
            v
        }).collect::<Vec<_>>())),
        "handoff.toRyolune" => to_ryolune(s, &a).await,
        "handoff.fromRyolune" => from_ryolune(s, cx, &a).await,
        _ => Err(crate::commands::unhandled(cx)),
    }
}

/// `~/.lsuite/handoff/<app>`: where hand-off files go.
fn handoff_dir(app: &str) -> PathBuf {
    crate::discovery::apps_dir().parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir).join("handoff").join(app)
}

fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let n = n.trim_matches('-').to_string();
    if n.is_empty() { "cut".into() } else { n }
}

async fn to_ryolune(s: &Arc<Session>, a: &Args) -> CmdResult {
    let project = s.project()?;
    let end = project.duration();
    if end <= 0.0 {
        return Err("The timeline is empty: there is no cut to send.".into());
    }
    let from = a.opt_f64("from").unwrap_or(0.0).clamp(0.0, end);
    let to = a.opt_f64("to").unwrap_or(end).clamp(from, end);
    if to - from < 0.05 {
        return Err("The range is empty.".into());
    }
    let name = safe_name(a.opt_str("name").unwrap_or(&project.name));
    let dir = handoff_dir("ryolune");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let wav = dir.join(format!("{name}.wav"));
    let settings = ExportSettings { path: wav.to_string_lossy().into_owned(), format: ExportFormat::Wav, quality: Quality::High, width: None, height: None, fps: None, range: Some((from, to)), encoder: Default::default(), audio: Default::default() };
    let id = crate::commands::export::start(s, project.clone(), settings)?;
    crate::commands::export::wait(s, &id).await?;

    let markers: Vec<Value> = project
        .markers
        .iter()
        .filter(|m| m.time >= from && m.time <= to)
        .map(|m| json!({ "time": m.time - from, "label": m.label }))
        .collect();
    let cut = json!({
        "format": 1,
        "from": "kimchi",
        "project": project.name,
        "audio": wav,
        "seconds": to - from,
        "range": { "from": from, "to": to },
        "fps": project.settings.fps,
        "markers": markers,
    });
    let sidecar = dir.join(format!("{name}.kimchi-cut.json"));
    std::fs::write(&sidecar, serde_json::to_vec_pretty(&cut).map_err(crate::session::err)?).map_err(|e| format!("Couldn't write {}: {e}", sidecar.display()))?;

    // Push it into the running ryolune, if any.
    let mut result = json!({ "audio": wav, "cut": sidecar, "seconds": to - from, "markers": markers.len(), "sentToRyolune": false });
    match RyoluneClient::connect().await {
        Ok(mut r) => {
            let info = r.call("session.info", json!({})).await.unwrap_or(Value::Null);
            let tempo = find_number(&info, "tempo").unwrap_or(120.0);
            let (num, den) = (find_number(&info, "numerator").unwrap_or(4.0), find_number(&info, "denominator").unwrap_or(4.0));
            // Bars from seconds at the song's starting tempo (beats are quarter notes).
            let beats_per_bar = num * 4.0 / den.max(1.0);
            let bar = |t: f64| t * tempo / 60.0 / beats_per_bar;
            r.call("session.importAudio", json!({ "path": wav, "startBar": 0 })).await.map_err(|e| format!("ryolune refused the audio: {e}"))?;
            let mut added = 0;
            for m in &markers {
                let label = m["label"].as_str().filter(|l| !l.is_empty()).map(str::to_string);
                let mut params = json!({ "bar": bar(m["time"].as_f64().unwrap_or(0.0)) });
                if let Some(l) = label {
                    params["name"] = json!(l);
                }
                if r.call("marker.add", params).await.is_ok() {
                    added += 1;
                }
            }
            result["sentToRyolune"] = json!(true);
            result["ryolune"] = json!({ "tempo": tempo, "markersAdded": added, "note": "Markers are placed at the song's starting tempo." });
        }
        Err(e) => result["ryoluneNote"] = json!(format!("{e} The files are ready for ryolune: import the WAV there.")),
    }
    Ok(result)
}

async fn from_ryolune(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let project = s.project()?;
    let path = match a.opt_str("path") {
        Some(p) => crate::commands::media::absolute(p)?,
        None => {
            let mut r = RyoluneClient::connect().await.map_err(|e| format!("{e} Give a path to an audio file ryolune exported instead."))?;
            let dir = handoff_dir("kimchi");
            std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
            let out = dir.join(format!("ryolune-{}.wav", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
            r.call("session.bounce", json!({ "path": out })).await.map_err(|e| format!("ryolune couldn't bounce its song: {e}"))?;
            out
        }
    };
    if !path.is_file() {
        return Err(format!("{} doesn't exist.", path.display()));
    }
    let track = match a.opt_str("trackId") {
        Some(k) => Some(resolve::track(&project, k)?),
        None => None,
    };
    let assets = crate::commands::media::import(s, cx, &[path.to_string_lossy().into_owned()]).await?;
    let asset = assets.first().ok_or("The audio couldn't be imported.")?;
    if asset.kind != kimchi_core::MediaKind::Audio && !asset.meta.has_audio {
        return Err(format!("{} has no sound.", asset.name));
    }
    let start = a.opt_f64("start").unwrap_or(0.0);
    let out = s.apply(cx.label(), cx.source, &Edit::InsertAsset { asset_id: asset.id, track_id: track, start }, None)?;
    Ok(json!({ "path": path, "assetId": asset.id, "clips": out.created_clips }))
}

/// The first number found under `key` anywhere in `v`.
fn find_number(v: &Value, key: &str) -> Option<f64> {
    match v {
        Value::Object(m) => m.get(key).and_then(Value::as_f64).or_else(|| m.values().find_map(|x| find_number(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_number(x, key)),
        _ => None,
    }
}

/// A minimal client for ryolune's bridge: `$RYOLUNE_CONTROL`, else `~/.ryolune/control.json`,
/// else the control file named in `~/.lsuite/apps/ryolune.json`.
struct RyoluneClient {
    lines: tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
    write: tokio::net::tcp::OwnedWriteHalf,
    next: u64,
}

impl RyoluneClient {
    fn control_file() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("RYOLUNE_CONTROL").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        let home = dirs::home_dir()?.join(".ryolune").join("control.json");
        if home.is_file() {
            return Some(home);
        }
        crate::discovery::find("ryolune").and_then(|e| e.running).and_then(|r| r.control_file)
    }

    async fn connect() -> Result<Self, String> {
        let file = Self::control_file().filter(|p| p.is_file()).ok_or("ryolune isn't running.")?;
        let d: Value = serde_json::from_slice(&std::fs::read(&file).map_err(|e| e.to_string())?).map_err(|e| format!("Invalid ryolune control file: {e}"))?;
        let port = d["port"].as_u64().ok_or("Invalid ryolune control file.")? as u16;
        let token = d["token"].as_str().unwrap_or_default().to_string();
        let stream = tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(("127.0.0.1", port)))
            .await
            .map_err(|_| "ryolune didn't answer.".to_string())?
            .map_err(|_| "ryolune isn't running.".to_string())?;
        let (read, write) = stream.into_split();
        let mut c = Self { lines: BufReader::new(read).lines(), write, next: 1 };
        c.call("auth", json!({ "token": token })).await?;
        Ok(c)
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write.write_all(format!("{line}\n").as_bytes()).await.map_err(|e| e.to_string())?;
        let answer = tokio::time::timeout(Duration::from_secs(600), self.lines.next_line())
            .await
            .map_err(|_| "ryolune took too long to answer.".to_string())?
            .map_err(|e| e.to_string())?
            .ok_or("ryolune closed the connection.")?;
        let v: Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
        if let Some(e) = v.get("error") {
            return Err(e.get("message").and_then(Value::as_str).unwrap_or("error").to_string());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}
