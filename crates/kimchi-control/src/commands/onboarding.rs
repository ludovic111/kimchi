//! The first-run setup and keyboard layouts: `app.onboarding` (what the setup asks, and what is
//! on this computer to answer with), `app.finishOnboarding` (the answers, saved in settings),
//! `app.keymaps` (other editors' keys, [`crate::keymaps`]) and `captions.downloadModel` (the
//! setup's "download the captions model now").
//!
//! Looking around this computer is quick and read-only: folders and programs that exist, the
//! keys kimchi already has (keychain or environment), local model servers that answer on their
//! usual ports within a fraction of a second. Nothing is installed or sent anywhere.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use kimchi_interop::apps::{self, App, Kind};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::keymaps::{self, Layout};
use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session, err};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "app.keymaps" => keymaps(s, &a),
        "app.onboarding" => overview(s, cx).await,
        "app.finishOnboarding" => finish(s, cx, &a),
        "captions.downloadModel" => download_model(s, &a).await,
        _ => Err(super::unhandled(cx)),
    }
}

// ---- keyboard layouts -----------------------------------------------------------

/// One layout as `app.keymaps` gives it: every action with its keys on this system.
pub fn layout_json(l: &Layout, current: bool) -> Value {
    let mac = cfg!(target_os = "macos");
    let bindings: Vec<Value> = keymaps::resolve(l, mac)
        .into_iter()
        .map(|b| {
            json!({
                "action": b.action,
                "group": b.group,
                "label": b.label,
                "scope": b.scope,
                "keys": b.keys,
                "shown": b.keys.iter().map(|k| keymaps::label(k, mac)).collect::<Vec<_>>(),
                "fromLayout": b.from_layout,
                "dropped": b.dropped.iter().map(|d| json!({ "key": d.key, "shown": keymaps::label(&d.key, mac), "because": d.because })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "id": l.id,
        "name": l.name,
        "app": l.app,
        "source": l.source,
        "notes": l.notes,
        "current": current,
        "bindings": bindings,
    })
}

fn keymaps(s: &Arc<Session>, a: &Args) -> CmdResult {
    let current = s.settings().shortcuts.keymap;
    let current = keymaps::layout(&current).map(|l| l.id).unwrap_or("kimchi");
    if let Some(id) = a.opt_str("keymap") {
        let l = keymaps::layout(id)?;
        return Ok(layout_json(l, l.id == current));
    }
    Ok(json!({
        "current": current,
        "setting": "shortcuts.keymap",
        "layouts": keymaps::LAYOUTS.iter().map(|l| layout_json(l, l.id == current)).collect::<Vec<_>>(),
    }))
}

// ---- what the setup asks ----------------------------------------------------------

/// The setup's steps, in order, with the settings and commands each uses.
pub const STEPS: &[(&str, &str, &[&str])] = &[
    ("welcome", "Welcome: light or dark", &["appearance.mode"]),
    ("comingFrom", "Where are you coming from? Its keys, projects, looks and plugins", &["shortcuts.keymap", "onboarding.comingFrom", "project.importFrom", "looks.import"]),
    ("generativeAi", "Generative AI, and connecting a provider", &["generate.enabled", "generate.setKey", "generate.check"]),
    ("agent", "An AI assistant in kimchi, and other AI apps controlling it", &["agent.enabled", "agent.setProvider", "app.setAgentKey"]),
    ("sound", "Sound and captions: Ryolune, plugins, the captions model", &["captions.downloadModel"]),
    ("done", "Start: a new project, the imported one, or a file", &["onboarding.completed"]),
];

/// The AI coding tools the Agent panel can run, by their program names.
const CODING_TOOLS: &[(&str, &str, &[&str])] = &[("claude-code", "Claude Code", &["claude"]), ("codex", "Codex", &["codex"]), ("gemini-cli", "Gemini CLI", &["gemini"])];

/// Local model servers on their usual ports: (id, name, what for, address, a path that answers).
const LOCAL_SERVERS: &[(&str, &str, &str, &str, &str)] = &[
    ("ollama", "Ollama", "agent, images", "http://127.0.0.1:11434", "/api/tags"),
    ("lmstudio", "LM Studio", "agent", "http://127.0.0.1:1234", "/v1/models"),
    ("comfyui", "ComfyUI", "images, video", "http://127.0.0.1:8188", "/system_stats"),
    ("a1111", "Stable Diffusion WebUI", "images", "http://127.0.0.1:7860", "/sdapi/v1/options"),
];

/// The agent's API providers whose keys kimchi keeps (secret ids).
const AGENT_KEY_PROVIDERS: &[&str] = &["anthropic", "openai", "gemini", "openrouter", "groq", "mistral", "deepseek", "xai", "together", "fireworks", "cerebras", "azure-openai", "bedrock"];

async fn overview(s: &Arc<Session>, cx: &Ctx) -> CmdResult {
    let settings = s.settings();
    // The file system and the keychain block: off the async workers.
    let s2 = s.clone();
    let local = tokio::task::spawn_blocking(move || (scan_apps(), coding_tools(), s2.harness.statuses(), agent_keys(&s2), whisper_models(&s2), ryolune()))
        .await
        .map_err(err)?;
    let (found_apps, tools, providers, agent_keys, models, ryolune) = local;
    let servers = probe_servers().await;
    let video_plugins = count_by(sub(s, cx, "plugins.list").await, "format");
    let audio_plugins = count_by(sub(s, cx, "audio.effects").await.map(|v| if v.is_array() { v } else { v["effects"].clone() }), "format");
    let agent_providers = match s.agent_host() {
        Some(host) => host.call(s.clone(), cx.source, "agent.providers", Args(Map::new())).await.ok(),
        None => None,
    };
    let generation: Vec<Value> = providers
        .iter()
        .map(|p| {
            let local = matches!(p.info.kind, kimchi_gen::ProviderKind::Local);
            json!({
                "id": p.info.id,
                "name": p.info.name,
                "kind": p.info.kind,
                "tagline": p.info.tagline,
                "ready": p.ready,
                "needsKey": p.info.needs_key,
                "hasKey": p.key_preview.is_some(),
                "keySource": p.key_source,
                "keyUrl": p.info.key_url,
                "keyHint": p.info.key_hint,
                "website": p.info.website,
                // A local server answering now on its usual address.
                "running": local && servers.iter().any(|v| v["id"] == p.info.id.as_str() && v["answering"] == true),
                "quickStart": QUICK_START.contains(&p.info.id.as_str()),
            })
        })
        .collect();
    let mut sorted = generation.clone();
    // The quickest to start first: gateways, then local servers found running, then the rest.
    let rank = |v: &Value| {
        let id = v["id"].as_str().unwrap_or("");
        match QUICK_START.iter().position(|q| *q == id) {
            Some(i) => i as i64,
            None if v["running"] == true => 10,
            None if v["ready"] == true => 20,
            None => 30,
        }
    };
    sorted.sort_by_key(rank);
    Ok(json!({
        "done": settings.onboarding.is_done(),
        "completed": settings.onboarding.completed,
        "comingFrom": settings.onboarding.coming_from,
        "version": crate::update::CURRENT,
        "steps": STEPS.iter().map(|(id, title, uses)| json!({ "id": id, "title": title, "uses": uses })).collect::<Vec<_>>(),
        "appearance": { "mode": settings.appearance.mode, "transparency": settings.appearance.transparency },
        "keymap": settings.shortcuts.keymap,
        "apps": found_apps,
        "codingTools": tools,
        "localServers": servers,
        "generation": { "enabled": settings.generate.enabled, "providers": sorted },
        "agent": {
            "enabled": settings.agent.enabled,
            "provider": settings.agent.provider,
            "keys": agent_keys,
            "providers": agent_providers,
        },
        "videoPlugins": video_plugins,
        "audioPlugins": audio_plugins,
        "ryolune": ryolune,
        "captionsModels": models,
    }))
}

/// Generation providers that are quickest to start with: one key reaches many models.
pub const QUICK_START: &[&str] = &["fal", "openrouter", "replicate"];

/// A query run for the overview without a record of its own; `None` when it isn't there (yet)
/// or fails.
async fn sub(s: &Arc<Session>, cx: &Ctx, name: &str) -> Option<Value> {
    let spec = crate::registry::spec(name)?;
    match crate::registry::call_boxed(s, cx.source, spec, json!({})).await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!("app.onboarding: {name} answered {e}");
            None
        }
    }
}

/// `{ "total": n, "<format>": n… }` from a list of objects, or null.
fn count_by(list: Option<Value>, field: &str) -> Value {
    let Some(items) = list.as_ref().and_then(Value::as_array) else { return Value::Null };
    let mut out = Map::new();
    out.insert("total".into(), json!(items.len()));
    for it in items {
        let f = it[field].as_str().unwrap_or("other").to_ascii_lowercase();
        let n = out.get(&f).and_then(Value::as_u64).unwrap_or(0);
        out.insert(f, json!(n + 1));
    }
    Value::Object(out)
}

/// Every app kimchi knows, editors first, with whether it is on this computer and what it
/// brings: its project formats, keyboard layout, looks (with the folders found and how many
/// looks are in them) and plugins.
fn scan_apps() -> Vec<Value> {
    let mut list: Vec<&App> = apps::APPS.iter().collect();
    list.sort_by_key(|a| (a.kind != Kind::Editor) as u8);
    list.into_iter().map(app_json).collect()
}

pub fn app_json(a: &App) -> Value {
    let found: Vec<String> = a.detect.iter().flat_map(apps::resolve).map(|p| p.to_string_lossy().into_owned()).collect();
    let folders: Vec<Value> = a
        .look_folders
        .iter()
        .flat_map(apps::resolve)
        .filter(|p| p.is_dir())
        .map(|p| {
            let n = count_looks(&p);
            json!({ "path": p, "looks": n })
        })
        .collect();
    json!({
        "id": a.id,
        "name": a.name,
        "vendor": a.vendor,
        "kind": a.kind,
        "installed": !found.is_empty(),
        "foundAt": found,
        "keymap": a.keymap,
        "opens": a.opens,
        "writes": a.writes,
        "extensions": open_extensions(a),
        "bring": a.bring,
        "take": a.take,
        "looks": a.looks,
        "lookFolders": folders,
        "plugins": a.plugins,
    })
}

/// The file extensions of the formats kimchi opens from an app (for a file picker).
fn open_extensions(a: &App) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = vec![];
    for f in a.opens {
        if let Ok(f) = kimchi_interop::timeline::format(f) {
            for e in f.extensions {
                if !out.contains(e) {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// Looks in a folder and its subfolders: LUTs and presets (`looks.import` takes them all).
pub fn count_looks(dir: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            seen += 1;
            if seen > 20_000 {
                return n;
            }
            let p = e.path();
            if p.is_dir() {
                if depth < 6 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            if is_look(&p) {
                n += 1;
            }
        }
    }
    n
}

fn is_look(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let name = p.file_name().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        // Pictures are looks only when they say they are Hald CLUTs.
        "png" | "tif" | "tiff" => name.contains("hald") || name.contains("clut"),
        "xmp" | "lrtemplate" | "prfpset" => true,
        e => kimchi_interop::looks::LUT_EXTENSIONS.contains(&e),
    }
}

/// The AI coding tools on this computer.
fn coding_tools() -> Vec<Value> {
    CODING_TOOLS
        .iter()
        .map(|(id, name, programs)| {
            let path = programs.iter().find_map(|p| find_program(p));
            json!({ "id": id, "name": name, "found": path.is_some(), "path": path })
        })
        .collect()
}

/// A program on the `PATH`, or where its installer usually puts it (an app started from the
/// Dock or a desktop file doesn't see the shell's `PATH`).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        for d in [".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".volta/bin", ".cargo/bin", "bin"] {
            dirs.push(home.join(d));
        }
        if cfg!(windows)
            && let Some(appdata) = std::env::var_os("APPDATA")
        {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    let names: Vec<String> = if cfg!(windows) { vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_string()] } else { vec![name.to_string()] };
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Local model servers answering on their usual addresses (each asked once, briefly).
async fn probe_servers() -> Vec<Value> {
    let client = reqwest::Client::builder().timeout(Duration::from_millis(700)).connect_timeout(Duration::from_millis(300)).build().ok();
    let asks = LOCAL_SERVERS.iter().map(|(id, name, what, base, path)| {
        let client = client.clone();
        async move {
            let answering = match client {
                Some(c) => c.get(format!("{base}{path}")).send().await.is_ok(),
                None => false,
            };
            json!({ "id": id, "name": name, "for": what, "url": base, "answering": answering })
        }
    });
    futures::future::join_all(asks).await
}

/// The agent's API keys kimchi has (keychain), or that the environment gives.
fn agent_keys(s: &Session) -> Vec<Value> {
    AGENT_KEY_PROVIDERS
        .iter()
        .map(|id| {
            let env = env_names(id).iter().find(|v| std::env::var(v).is_ok_and(|k| !k.trim().is_empty())).copied();
            json!({ "provider": id, "saved": s.secret(&format!("agent:{id}")).or_else(|| s.secret(id)).is_some(), "env": env })
        })
        .collect()
}

/// The environment variables the agent's API providers usually read.
fn env_names(id: &str) -> &'static [&'static str] {
    match id {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "fireworks" => &["FIREWORKS_API_KEY"],
        "cerebras" => &["CEREBRAS_API_KEY"],
        "azure-openai" => &["AZURE_OPENAI_API_KEY"],
        "bedrock" => &["AWS_ACCESS_KEY_ID", "AWS_PROFILE"],
        _ => &[],
    }
}

/// Speech models for captions: on this computer or not, and the download running now.
fn whisper_models(s: &Session) -> Vec<Value> {
    let dir = super::captions::models_dir(s);
    let running = download_status();
    kimchi_captions::whisper::MODELS
        .iter()
        .map(|m| {
            let downloading = running.as_ref().filter(|d| d.model == m.id);
            json!({
                "id": m.id,
                "label": m.label,
                "sizeMb": m.size_mb,
                "downloaded": m.model.is_downloaded(&dir),
                "default": m.model == kimchi_captions::whisper::Model::Base,
                "downloading": downloading.is_some(),
                "progress": downloading.map(|d| d.progress),
            })
        })
        .collect()
}

/// Ryolune (lsuite's music app): installed (its discovery entry, or the app), and running.
fn ryolune() -> Value {
    let entry = crate::discovery::find("ryolune");
    let app = if cfg!(target_os = "macos") { Some(PathBuf::from("/Applications/ryolune.app")).filter(|p| p.exists()) } else { find_program("ryolune") };
    json!({
        "installed": entry.is_some() || app.is_some(),
        "running": entry.as_ref().is_some_and(|e| e.running.is_some()),
        "version": entry.as_ref().map(|e| e.version.clone()),
        "path": entry.and_then(|e| e.app_path.or(e.executable)).or(app),
    })
}

// ---- the answers -----------------------------------------------------------------

/// What "Where are you coming from?" takes besides an app id.
const NOT_AN_APP: &[&str] = &["none", "new", "other"];

fn finish(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let coming_from = match a.opt_str("comingFrom").map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) if NOT_AN_APP.contains(&c.to_ascii_lowercase().as_str()) => Some(c.to_ascii_lowercase()),
        Some(c) => match apps::app(c) {
            Some(app) => Some(app.id.to_string()),
            None => {
                let mut ids: Vec<&str> = apps::APPS.iter().map(|a| a.id).collect();
                ids.extend(NOT_AN_APP);
                let hint = kimchi_core::closest(c, &ids).map(|x| format!(" Did you mean `{x}`?")).unwrap_or_default();
                return Err(format!("Unknown app `{c}`.{hint} Apps: {}.", ids.join(", ")));
            }
        },
        None => None,
    };
    // The keyboard layout: the one asked for, else the app's (when it has one).
    let keymap = match a.opt_str("keymap") {
        Some(k) => Some(keymaps::layout(k)?.id),
        None => coming_from.as_deref().and_then(apps::app).and_then(|a| a.keymap),
    };
    if cx.source.is_agent() && a.has("agent") {
        return Err("Whether kimchi offers its Agent panel stays with the person.".into());
    }
    let generative = a.opt_bool("generativeAi");
    let agent = a.opt_bool("agent");
    let skipped = a.bool_or("skipped", false);
    let settings = s.update_settings_checked(|st| {
        st.onboarding.completed = crate::update::CURRENT.to_string();
        if let Some(c) = &coming_from {
            st.onboarding.coming_from = c.clone();
        }
        if let Some(k) = keymap {
            st.shortcuts.keymap = k.to_string();
        }
        if let Some(on) = generative {
            st.generate.enabled = on;
        }
        if let Some(on) = agent {
            st.agent.enabled = on;
        }
        Ok(())
    })?;
    tracing::info!(skipped, coming_from = ?coming_from, keymap = ?keymap, "first-run setup finished");
    Ok(json!({
        "completed": settings.onboarding.completed,
        "skipped": skipped,
        "comingFrom": settings.onboarding.coming_from,
        "keymap": settings.shortcuts.keymap,
        "generativeAi": settings.generate.enabled,
        "agent": settings.agent.enabled,
        "again": "Help › Set up kimchi…, or ui.showPanel onboarding.",
    }))
}

// ---- the captions model -----------------------------------------------------------

#[derive(Clone)]
struct Download {
    model: &'static str,
    progress: f64,
    error: Option<String>,
    cancel: CancellationToken,
}

fn downloads() -> &'static Mutex<Option<Download>> {
    static D: OnceLock<Mutex<Option<Download>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

fn download_status() -> Option<Download> {
    downloads().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Starts downloading a speech model in the background (or, while one is downloading, says how
/// far it is). With `wait`, answers when it is there. `cancel` stops it.
async fn download_model(s: &Arc<Session>, a: &Args) -> CmdResult {
    use kimchi_captions::whisper::{self, Model};
    let model = match a.opt_str("model") {
        Some(m) => Model::parse(m)?,
        None => Model::Base,
    };
    let dir = super::captions::models_dir(s);
    let info = model.info();
    let state = |d: Option<&Download>| {
        json!({
            "model": info.id,
            "sizeMb": info.size_mb,
            "downloaded": model.is_downloaded(&dir),
            "downloading": d.is_some_and(|d| d.error.is_none()),
            "progress": d.map(|d| (d.progress * 100.0).round() / 100.0),
            "error": d.and_then(|d| d.error.clone()),
        })
    };
    if a.bool_or("cancel", false) {
        if let Some(d) = download_status() {
            d.cancel.cancel();
        }
        return Ok(state(None));
    }
    if model.is_downloaded(&dir) {
        return Ok(state(None));
    }
    let running = download_status();
    match &running {
        Some(d) if d.model != info.id && d.error.is_none() => return Err(format!("The {} model is downloading; wait for it, or cancel it first.", d.model)),
        Some(d) if d.error.is_none() => {}
        _ => {
            let cancel = CancellationToken::new();
            *downloads().lock().unwrap_or_else(|e| e.into_inner()) = Some(Download { model: info.id, progress: 0.0, error: None, cancel: cancel.clone() });
            let dir = dir.clone();
            tokio::spawn(async move {
                let progress = |p: f64| {
                    if let Some(d) = downloads().lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        d.progress = p;
                    }
                };
                let result = whisper::download(&dir, model, progress, &cancel).await;
                let mut d = downloads().lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => {
                        tracing::info!("downloaded the {} speech model", model.info().id);
                        *d = None;
                    }
                    Err(e) if cancel.is_cancelled() => {
                        tracing::info!("the speech model download was cancelled: {e}");
                        *d = None;
                    }
                    Err(e) => {
                        tracing::warn!("couldn't download the speech model: {e}");
                        if let Some(d) = d.as_mut() {
                            d.error = Some(format!("Couldn't download the captions model: {e}. Check the connection and try again."));
                        }
                    }
                }
            });
        }
    }
    if a.bool_or("wait", false) {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            match download_status() {
                None => break,
                Some(d) if d.error.is_some() => {
                    *downloads().lock().unwrap_or_else(|e| e.into_inner()) = None;
                    return Err(d.error.unwrap_or_default());
                }
                Some(_) => {}
            }
        }
    }
    let d = download_status();
    // A failure is said once, then the next call starts over.
    if d.as_ref().is_some_and(|d| d.error.is_some()) {
        *downloads().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    Ok(state(d.as_ref()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use std::sync::Arc;

    use crate::registry;
    use crate::session::{Session, SessionOptions, Source};

    fn session() -> (Arc<Session>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = Session::new(SessionOptions { data_dir: Some(dir.path().join("data")), config_dir: Some(dir.path().join("config")), secrets: None, headless: true }).unwrap();
        (s, dir)
    }

    #[tokio::test]
    async fn the_setup_reports_this_computer_and_saves_the_answers() {
        let (s, _dir) = session();
        let v = registry::call(&s, Source::Cli, "app.onboarding", json!({})).await.unwrap();
        assert_eq!(v["steps"][0]["id"], "welcome");
        assert_eq!(v["steps"].as_array().unwrap().len(), super::STEPS.len());
        // Every app, editors first, each saying whether it is here.
        let apps = v["apps"].as_array().unwrap();
        assert_eq!(apps.len(), kimchi_interop::apps::APPS.len());
        assert_eq!(apps[0]["kind"], "editor");
        assert!(apps.iter().all(|a| a["installed"].is_boolean()));
        let premiere = apps.iter().find(|a| a["id"] == "premiere").unwrap();
        assert_eq!(premiere["keymap"], "premiere");
        assert!(premiere["extensions"].as_array().unwrap().iter().any(|e| e == "xml"));
        assert_eq!(v["localServers"].as_array().unwrap().len(), 4);
        assert_eq!(v["codingTools"][0]["id"], "claude-code");
        // The quickest providers first.
        assert_eq!(v["generation"]["providers"][0]["id"], super::QUICK_START[0]);
        assert!(v["captionsModels"].as_array().unwrap().iter().any(|m| m["default"] == true));

        // The person came from Premiere and doesn't want generation.
        let r = registry::call(&s, Source::Window, "app.finishOnboarding", json!({ "comingFrom": "Premiere", "generativeAi": false })).await.unwrap();
        assert_eq!(r["keymap"], "premiere");
        assert_eq!(r["comingFrom"], "premiere");
        let st = s.settings();
        assert!(st.onboarding.is_done());
        assert_eq!(st.onboarding.completed, crate::update::CURRENT);
        assert!(!st.generate.enabled);
        assert!(st.agent.enabled, "not asked: unchanged");
        assert_eq!(st.shortcuts.keymap, "premiere");
        // A keymap given wins over the app's.
        registry::call(&s, Source::Window, "app.finishOnboarding", json!({ "comingFrom": "resolve", "keymap": "kimchi" })).await.unwrap();
        assert_eq!(s.settings().shortcuts.keymap, "kimchi");
        let e = registry::call(&s, Source::Window, "app.finishOnboarding", json!({ "comingFrom": "premeire" })).await.unwrap_err();
        assert!(e.contains("Did you mean `premiere`?"), "{e}");
        let e = registry::call(&s, Source::Window, "app.finishOnboarding", json!({ "keymap": "vegass" })).await.unwrap_err();
        assert!(e.contains("Did you mean `vegas`?"), "{e}");
        // An agent can't switch the Agent panel on or off.
        registry::call(&s, Source::Cli, "app.setSetting", json!({ "key": "agent.permissions.settings", "value": true })).await.unwrap();
        let e = registry::call(&s, Source::Agent, "app.finishOnboarding", json!({ "agent": false })).await.unwrap_err();
        assert!(e.contains("stays with the person"), "{e}");
        // Not an app: new to editing.
        registry::call(&s, Source::Window, "app.finishOnboarding", json!({ "comingFrom": "new", "skipped": true })).await.unwrap();
        assert_eq!(s.settings().onboarding.coming_from, "new");
    }

    #[tokio::test]
    async fn keymaps_list_every_layout_with_its_keys() {
        let (s, _dir) = session();
        let v = registry::call(&s, Source::Cli, "app.keymaps", json!({})).await.unwrap();
        assert_eq!(v["current"], "kimchi");
        let layouts = v["layouts"].as_array().unwrap();
        assert_eq!(layouts.len(), crate::keymaps::LAYOUTS.len());
        let one = registry::call(&s, Source::Cli, "app.keymaps", json!({ "keymap": "Final Cut Pro" })).await.unwrap();
        assert_eq!(one["id"], "finalcut");
        let split = one["bindings"].as_array().unwrap().iter().find(|b| b["action"] == "Split").unwrap();
        assert_eq!(split["keys"][0], "M-b");
        assert_eq!(split["fromLayout"], true);
        // The setting takes a layout, checked.
        registry::call(&s, Source::Cli, "app.setSetting", json!({ "key": "shortcuts.keymap", "value": "avid" })).await.unwrap();
        let v = registry::call(&s, Source::Cli, "app.keymaps", json!({ "keymap": "avid" })).await.unwrap();
        assert_eq!(v["current"], true);
        assert!(registry::call(&s, Source::Cli, "app.setSetting", json!({ "key": "shortcuts.keymap", "value": "avidd" })).await.is_err());
    }

    #[test]
    fn looks_are_counted_in_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Film/Kodak")).unwrap();
        for f in ["a.cube", "Film/b.3dl", "Film/Kodak/c.CUBE", "preset.xmp", "photo.png", "hald_8.png", "readme.txt"] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        assert_eq!(super::count_looks(dir.path()), 5);
    }
}
