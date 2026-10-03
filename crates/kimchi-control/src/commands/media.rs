use std::path::{Path, PathBuf};
use std::sync::Arc;

use kimchi_core::{Asset, AssetOrigin, Edit, Id, MediaKind, new_id};
use serde_json::json;

use crate::commands::project::asset_summary;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session, ToastKind, err};

/// File extensions the import dialog offers.
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "webm", "mkv", "avi", "png", "jpg", "jpeg", "webp", "gif", "heic", "avif", "mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "aif", "aiff",
];

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "media.list" => Ok(json!(s.read(|ed| ed.project().assets.iter().map(asset_summary).collect::<Vec<_>>())?)),
        "media.get" => {
            let p = s.project()?;
            let id = resolve::asset(&p, a.str("assetId")?)?;
            Ok(json!(p.asset(id)))
        }
        "media.import" => {
            let paths = a.strings("paths");
            if paths.is_empty() {
                return Err("`paths` is empty".into());
            }
            let added = import(s, cx, &paths).await?;
            let mut placed = vec![];
            if a.bool_or("place", false) {
                let p = s.project()?;
                let track = a.opt_str("trackId").map(|k| resolve::track(&p, k)).transpose()?;
                let mut at = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
                for asset in &added {
                    let out = s.apply(cx.label(), cx.source, &Edit::InsertAsset { asset_id: asset.id, track_id: track, start: at }, None)?;
                    at += asset.duration().unwrap_or(kimchi_core::DEFAULT_STILL_DURATION);
                    placed.extend(out.created_clips);
                }
            }
            Ok(json!({ "media": added.iter().map(asset_summary).collect::<Vec<_>>(), "clips": placed }))
        }
        "media.remove" => {
            let p = s.project()?;
            let id = resolve::asset(&p, a.str("assetId")?)?;
            s.apply(cx.label(), cx.source, &Edit::RemoveAsset { asset_id: id }, None)?;
            Ok(json!({ "removed": id }))
        }
        "media.frame" => {
            let p = s.project()?;
            let clip = resolve::clip(&p, a.str("clipId")?)?;
            let time = a.opt_f64("time");
            Ok(json!({ "path": clip_frame(s, clip, time).await? }))
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

/// Probes files and adds them to the open project's media. Previews are made in the background.
pub async fn import(s: &Arc<Session>, cx: &Ctx, paths: &[String]) -> CmdResult<Vec<Asset>> {
    import_named(s, cx, paths, None).await
}

/// [`import`], naming the media `name` instead of after their files (a still kimchi made).
pub async fn import_named(s: &Arc<Session>, cx: &Ctx, paths: &[String], name: Option<&str>) -> CmdResult<Vec<Asset>> {
    let project_id = s.current_id().ok_or(crate::session::NO_PROJECT)?;
    let tools = s.tools()?;
    let mut added = Vec::new();
    let mut failures = Vec::new();
    for path in paths {
        // Absolute, so the project doesn't depend on the folder kimchi started in and a name
        // like `-take2.mov` never reaches ffprobe looking like an option.
        let p = match absolute(path) {
            Ok(p) => p,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        match kimchi_media::probe(&tools, &p).await {
            Ok(probe) => added.push(Asset {
                id: new_id(),
                name: name.map(str::to_string).or_else(|| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| path.clone()),
                kind: probe.kind,
                path: path_str(&p),
                meta: probe.meta,
                origin: AssetOrigin::Imported,
                created_at: chrono::Utc::now(),
                thumbnail: None,
                filmstrip: None,
                waveform: None,
                proxy: None,
            }),
            Err(e) => failures.push(format!("{}: {e}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())),
        }
    }
    if added.is_empty() && !failures.is_empty() {
        return Err(failures.join("\n"));
    }
    s.edit(cx.label(), cx.source, |ed| {
        for a in &added {
            let _ = ed.apply(&Edit::AddAsset { asset: a.clone() }, None);
        }
        Ok(())
    })?;
    for a in &added {
        spawn_previews(s, project_id, a.clone());
    }
    if !failures.is_empty() {
        s.toast(ToastKind::Error, format!("Skipped {} file(s):\n{}", failures.len(), failures.join("\n")));
    }
    Ok(added)
}

/// Adds an already-probed asset (a generation's result) and starts its previews.
pub fn add_asset(s: &Arc<Session>, project_id: Id, asset: Asset) -> CmdResult<()> {
    s.with_project(project_id, |ed| ed.apply(&Edit::AddAsset { asset: asset.clone() }, None).map(|_| ()))?.map_err(err)?;
    spawn_previews(s, project_id, asset);
    Ok(())
}

/// Thumbnail → filmstrip/waveform → proxy, each published as it lands so the
/// window fills in progressively.
pub fn spawn_previews(s: &Arc<Session>, project_id: Id, asset: Asset) {
    let s = s.clone();
    s.runtime().clone().spawn(async move {
        let Ok(tools) = s.tools() else { return };
        let cache = s.cache_dir(project_id);
        if std::fs::create_dir_all(&cache).is_err() {
            return;
        }
        let src = PathBuf::from(&asset.path);
        let stem = asset.id.to_string();
        let mut a = asset.clone();

        let thumb = cache.join(format!("{stem}-thumb.jpg"));
        if a.kind != MediaKind::Audio && kimchi_media::thumbnail(&tools, &src, a.kind, &thumb, 480).await.unwrap_or(false) {
            a.thumbnail = Some(path_str(&thumb));
            publish(&s, project_id, &a);
        }
        if a.kind == MediaKind::Video
            && let Some(d) = a.meta.duration
            && let Ok(strip) = kimchi_media::filmstrip(&tools, &src, d, &cache.join(format!("{stem}-strip.jpg")), 72).await
        {
            a.filmstrip = Some(strip);
            publish(&s, project_id, &a);
        }
        if a.meta.has_audio
            && let Ok(w) = kimchi_media::waveform(&tools, &src, &cache.join(format!("{stem}-wave.f32")), 100).await
        {
            a.waveform = Some(w);
            publish(&s, project_id, &a);
        }
        if kimchi_media::needs_proxy(&a.meta, &src) {
            let out = cache.join(format!("{stem}-proxy.mp4"));
            match kimchi_media::proxy(&tools, &src, &out).await {
                Ok(()) => {
                    a.proxy = Some(path_str(&out));
                    publish(&s, project_id, &a);
                }
                Err(e) => tracing::warn!("proxy for {} failed: {e}", a.name),
            }
        }
    });
}

fn publish(s: &Session, project_id: Id, asset: &Asset) {
    let _ = s.with_project(project_id, |ed| ed.apply(&Edit::UpdateAsset { asset: asset.clone() }, None));
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A path a command was given, made absolute against kimchi's working folder (clients should
/// send absolute paths: the app's folder isn't theirs).
pub fn absolute(path: &str) -> CmdResult<PathBuf> {
    if path.trim().is_empty() {
        return Err("The path is empty.".into());
    }
    std::path::absolute(path).map_err(|e| format!("{path}: {e}"))
}

/// Saves the frame a clip shows at timeline time `time` as a PNG and returns
/// its path. Without a time: the playhead when it is inside the clip, else the
/// clip's first frame.
pub async fn clip_frame(s: &Arc<Session>, clip_id: Id, time: Option<f64>) -> CmdResult<String> {
    // Titles, solids and motion clips: what kimchi draws for the clip alone at that time.
    let drawn = s.read(|ed| {
        let p = ed.project();
        let clip = p.clip(clip_id)?;
        if matches!(clip.content, kimchi_core::ClipContent::Media { .. } | kimchi_core::ClipContent::Pending { .. }) {
            return None;
        }
        let playhead = s.ui_state().playhead;
        let t = time.unwrap_or(if clip.contains(playhead) { playhead } else { clip.start });
        let t = t.clamp(clip.start, (clip.end() - p.settings.frame()).max(clip.start));
        let mut alone = p.clone();
        alone.tracks = vec![kimchi_core::Track { clips: vec![clip.clone()], ..kimchi_core::Track::new(kimchi_core::TrackKind::Video, "clip") }];
        Some((alone, t))
    })?;
    if let Some((alone, t)) = drawn {
        let path = crate::commands::motion::render_png(s, &alone, &[t], Some(alone.settings.width)).await?;
        return Ok(path_str(&path));
    }
    let (asset, source_time, project_id) = s.read(|ed| {
        let p = ed.project();
        let clip = p.clip(clip_id).ok_or("clip not found")?;
        let asset = clip.asset_id().and_then(|id| p.asset(id)).ok_or("This clip has no picture to grab (only media clips do).")?.clone();
        let playhead = s.ui_state().playhead;
        let t = time.unwrap_or(if clip.contains(playhead) { playhead } else { clip.start });
        let t = t.clamp(clip.start, (clip.end() - p.settings.frame()).max(clip.start));
        Ok::<_, String>((asset, clip.source_time(t), p.id))
    })??;
    match asset.kind {
        MediaKind::Image => Ok(asset.path),
        MediaKind::Audio => Err("Audio clips have no frames.".into()),
        MediaKind::Video => {
            let tools = s.tools()?;
            let dir = s.cache_dir(project_id).join("frames");
            std::fs::create_dir_all(&dir).map_err(err)?;
            let out = dir.join(format!("{}-{:.3}.png", asset.id, source_time));
            if !out.exists() {
                kimchi_media::grab_frame(&tools, Path::new(&asset.path), source_time, &out).await.map_err(err)?;
            }
            Ok(path_str(&out))
        }
    }
}
