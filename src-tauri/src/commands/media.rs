use std::path::{Path, PathBuf};

use base64::Engine;
use kimchi_core::{Asset, AssetOrigin, Edit, Id, MediaKind, new_id};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{AppState, CmdResult, err};

/// Probes files and adds them to the open project's library. Thumbnails,
/// filmstrips, waveforms and proxies are produced in the background.
#[tauri::command]
pub async fn import_media(app: AppHandle, state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<Vec<Asset>> {
    let project_id = state.current_id().ok_or("no project is open")?;
    let tools = state.tools()?;
    let mut added = Vec::new();
    let mut failures = Vec::new();
    for path in paths {
        let p = PathBuf::from(&path);
        match kimchi_media::probe(&tools, &p).await {
            Ok(probe) => {
                let asset = Asset {
                    id: new_id(),
                    name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone()),
                    kind: probe.kind,
                    path: path.clone(),
                    meta: probe.meta,
                    origin: AssetOrigin::Imported,
                    created_at: chrono::Utc::now(),
                    thumbnail: None,
                    filmstrip: None,
                    waveform: None,
                    proxy: None,
                };
                added.push(asset);
            }
            Err(e) => failures.push(format!("{}: {e}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())),
        }
    }
    if added.is_empty() && !failures.is_empty() {
        return Err(failures.join("\n"));
    }
    state.with_project(&app, project_id, |ed| {
        for a in &added {
            let _ = ed.apply(&Edit::AddAsset { asset: a.clone() }, None);
        }
    })?;
    for a in &added {
        spawn_previews(app.clone(), project_id, a.clone());
    }
    if !failures.is_empty() {
        let _ = app.emit("toast", format!("Skipped {} file(s):\n{}", failures.len(), failures.join("\n")));
    }
    Ok(added)
}

/// Adds an already-probed asset and starts its previews. Used by generation.
pub fn add_asset(app: &AppHandle, project_id: Id, asset: Asset) -> CmdResult<()> {
    let state = app.state::<AppState>();
    state.with_project(app, project_id, |ed| ed.apply(&Edit::AddAsset { asset: asset.clone() }, None).map(|_| ()))?.map_err(err)?;
    spawn_previews(app.clone(), project_id, asset);
    Ok(())
}

/// Generates thumbnail → filmstrip/waveform → proxy, publishing each as it lands
/// so the UI fills in progressively.
pub fn spawn_previews(app: AppHandle, project_id: Id, asset: Asset) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let Ok(tools) = state.tools() else { return };
        let cache = state.library.cache_dir(project_id);
        if std::fs::create_dir_all(&cache).is_err() {
            return;
        }
        let src = PathBuf::from(&asset.path);
        let stem = asset.id.to_string();
        let mut a = asset.clone();

        let thumb = cache.join(format!("{stem}-thumb.jpg"));
        if a.kind != MediaKind::Audio && kimchi_media::thumbnail(&tools, &src, a.kind, &thumb, 480).await.unwrap_or(false) {
            a.thumbnail = Some(path_str(&thumb));
            publish(&app, project_id, &a);
        }
        if a.kind == MediaKind::Video
            && let Some(d) = a.meta.duration
            && let Ok(strip) = kimchi_media::filmstrip(&tools, &src, d, &cache.join(format!("{stem}-strip.jpg")), 72).await
        {
            a.filmstrip = Some(strip);
            publish(&app, project_id, &a);
        }
        if a.meta.has_audio
            && let Ok(w) = kimchi_media::waveform(&tools, &src, &cache.join(format!("{stem}-wave.f32")), 100).await
        {
            a.waveform = Some(w);
            publish(&app, project_id, &a);
        }
        if kimchi_media::needs_proxy(&a.meta, &src) {
            let out = cache.join(format!("{stem}-proxy.mp4"));
            match kimchi_media::proxy(&tools, &src, &out).await {
                Ok(()) => {
                    a.proxy = Some(path_str(&out));
                    publish(&app, project_id, &a);
                }
                Err(e) => tracing::warn!("proxy for {} failed: {e}", a.name),
            }
        }
    });
}

fn publish(app: &AppHandle, project_id: Id, asset: &Asset) {
    let state = app.state::<AppState>();
    let _ = state.with_project(app, project_id, |ed| ed.apply(&Edit::UpdateAsset { asset: asset.clone() }, None));
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Saves the frame shown by a clip at timeline time `time` as a PNG and
/// returns its path. Used to feed frames into image-to-video models.
#[tauri::command]
pub async fn clip_frame(state: State<'_, AppState>, clip_id: Id, time: f64) -> CmdResult<String> {
    let (asset, source_time, project_id) = {
        let guard = state.editor.lock();
        let ed = guard.as_ref().ok_or("no project is open")?;
        let p = ed.project();
        let clip = p.clip(clip_id).ok_or("clip not found")?;
        let asset = clip.asset_id().and_then(|id| p.asset(id)).ok_or("this clip has no picture to grab")?.clone();
        let t = time.clamp(clip.start, clip.end() - p.settings.frame());
        (asset, clip.source_time(t), p.id)
    };
    match asset.kind {
        MediaKind::Image => Ok(asset.path),
        MediaKind::Audio => Err("audio clips have no frames".into()),
        MediaKind::Video => {
            let tools = state.tools()?;
            let dir = state.library.cache_dir(project_id).join("frames");
            std::fs::create_dir_all(&dir).map_err(err)?;
            let out = dir.join(format!("{}-{:.3}.png", asset.id, source_time));
            if !out.exists() {
                kimchi_media::grab_frame(&tools, Path::new(&asset.path), source_time, &out).await.map_err(err)?;
            }
            Ok(path_str(&out))
        }
    }
}

/// Stores a PNG rendered by the UI (text layers, captures) and returns its path.
#[tauri::command]
pub fn write_png(state: State<AppState>, name: String, data_url: String) -> CmdResult<String> {
    let project_id = state.current_id().ok_or("no project is open")?;
    let payload = data_url.split_once(',').map(|(_, b)| b).ok_or("bad data url")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(payload).map_err(err)?;
    let dir = state.library.cache_dir(project_id).join("renders");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let out = dir.join(format!("{safe}.png"));
    std::fs::write(&out, bytes).map_err(err)?;
    Ok(path_str(&out))
}

/// Reads an audio peaks file written by `waveform` (avoids asset-protocol CORS quirks).
#[tauri::command]
pub fn read_peaks(path: String) -> CmdResult<tauri::ipc::Response> {
    std::fs::read(path).map(tauri::ipc::Response::new).map_err(err)
}
