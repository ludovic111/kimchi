//! Motion clips rendered ahead ("Render" on the timeline). A clip's scene is drawn at final
//! quality, frame by frame over the part of the scene the clip shows, into a lossless video with
//! transparency (FFV1 in Matroska) in the project's cache. While the file still matches the
//! scene (same [`key`]), the compositor plays it instead of drawing the scene: scrubbing,
//! playback and exports get heavy 3D or path-traced scenes at the speed of a video. A clip that
//! isn't rendered is drawn live: quick settings in the preview, full quality in the export.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use kimchi_core::{Clip, ClipContent, Id, Project, Rendered};
use tokio::io::AsyncWriteExt;

use super::{Quality, Renderer};
use crate::{MediaError, MediaResult, Tools, process};

/// Changes when anything the rendered frames depend on changes: the scene, the project's size
/// and frame rate, the clip's expression duration and shutter timing, and its media files.
pub fn key(project: &Project, clip: &Clip) -> Option<String> {
    let ClipContent::Motion { scene, .. } = &clip.content else { return None };
    let mut h = Fnv::new();
    h.write(b"kimchi-render-2");
    h.write(serde_json::to_string(scene).ok()?.as_bytes());
    let ps = &project.settings;
    h.write(format!("{}x{}@{:.4}", ps.width, ps.height, ps.fps).as_bytes());
    h.write(&super::scene_length(clip).to_bits().to_le_bytes());
    h.write(&clip.speed.abs().to_bits().to_le_bytes());
    for r in scene.media_refs() {
        h.write(r.as_bytes());
        if let Some(path) = media_path(project, &r)
            && let Ok(meta) = std::fs::metadata(&path)
        {
            let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            h.write(format!("{}|{}|{modified}", path.display(), meta.len()).as_bytes());
        }
    }
    Some(format!("{:016x}", h.0))
}

/// Is the clip's render there and still right?
pub fn is_current(project: &Project, clip: &Clip) -> bool {
    match &clip.rendered {
        Some(r) => Path::new(&r.file).is_file() && key(project, clip).as_deref() == Some(r.key.as_str()),
        None => false,
    }
}

/// Is the clip's render still right, and does it have a frame for every moment the clip shows?
/// A clip whose in point moved since (slipped, or trimmed at the start) shows scene time the file
/// doesn't have: those frames are drawn live, so the clip isn't fully rendered any more.
pub fn is_complete(project: &Project, clip: &Clip) -> bool {
    let Some(r) = &clip.rendered else { return false };
    let (a, b) = (clip.scene_time(clip.start), clip.scene_time(clip.end()));
    r.covers(a.min(b).max(0.0)) && r.covers(a.max(b).max(0.0)) && is_current(project, clip)
}

/// The clip's state for listings: `"live"`, `"rendered"` or `"outdated"` (rendered, but the
/// scene or the part of it the clip shows changed since).
pub fn status(project: &Project, clip: &Clip) -> &'static str {
    match &clip.rendered {
        None => "live",
        Some(_) if is_complete(project, clip) => "rendered",
        Some(_) => "outdated",
    }
}

/// Renders a motion clip's scene to `dir` at the project's size and frame rate, over the scene
/// time the clip shows. `progress` gets 0–1; setting `cancel` stops it (nothing is left behind).
pub async fn render(tools: &Tools, project: &Project, clip_id: Id, dir: &Path, progress: &(dyn Fn(f64) + Sync), cancel: &AtomicBool) -> MediaResult<Rendered> {
    let clip = project.clip(clip_id).cloned().ok_or_else(|| MediaError::Unsupported(format!("no clip {clip_id}")))?;
    let ClipContent::Motion { scene, .. } = &clip.content else {
        return Err(MediaError::Unsupported(format!("\"{}\" isn't a motion clip", clip.name)));
    };
    let engine = match scene {
        kimchi_core::Scene::Space(s) => s.render.engine.clone(),
        kimchi_core::Scene::Flat(_) => "standard".into(),
    };
    let key = key(project, &clip).ok_or_else(|| MediaError::Unsupported("no scene".into()))?;
    let ps = &project.settings;
    let (w, h) = ((ps.width.max(2) / 2) * 2, (ps.height.max(2) / 2) * 2);
    let fps = if ps.fps.is_finite() && ps.fps > 0.0 { crate::snap_fps(ps.fps) } else { 30.0 };
    // The scene time the clip shows, one frame either side for rounding.
    let a = clip.scene_time(clip.start).max(0.0);
    let b = clip.scene_time(clip.end()).max(a);
    let from = ((a * fps).floor() / fps - 1.0 / fps).max(0.0);
    let frames = (((b - from) * fps).ceil() as u64 + 2).max(1);
    std::fs::create_dir_all(dir)?;
    let file = dir.join(format!("{}-{key}.mkv", clip.id));
    let part = dir.join(format!("{}-{key}.part.mkv", clip.id));
    let size = format!("{w}x{h}");
    let rate = crate::rate(fps);
    let args: Vec<String> = [
        "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &size, "-r", &rate, "-i", "-", "-an", "-c:v", "ffv1", "-level", "3",
        "-pix_fmt", "bgra", "-f", "matroska",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([part.to_string_lossy().into_owned()])
    .collect();
    let mut child = process::spawn_with_stdin(&tools.ffmpeg, &args, false, true)?;
    let stderr = process::collect_stderr(&mut child);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MediaResult<Vec<u8>>>(3);
    let (tools2, project2) = (tools.clone(), project.clone());
    let draw = tokio::task::spawn_blocking(move || {
        let mut r = Renderer::for_export(&tools2, &project2, w, h, fps).with_quality(Quality::Final);
        for n in 0..frames {
            let t = from + n as f64 / fps;
            let frame = r.scene_frame(clip_id, t).map(|p| straight(p.take()));
            if tx.blocking_send(frame).is_err() {
                return;
            }
        }
    });
    let mut done = 0u64;
    let mut failed = None;
    while let Some(frame) = rx.recv().await {
        if cancel.load(Ordering::Relaxed) {
            failed = Some(MediaError::Cancelled);
            break;
        }
        match frame {
            Ok(bytes) => {
                if stdin.write_all(&bytes).await.is_err() {
                    break;
                }
                done += 1;
                progress((done as f64 / frames as f64).min(0.999));
            }
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    drop(rx);
    drop(stdin);
    let _ = draw.await;
    if let Some(e) = failed {
        let _ = child.kill().await;
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let status = child.wait().await?;
    if !status.success() || done < frames {
        let _ = std::fs::remove_file(&part);
        let why = process::summarize(&stderr.await.unwrap_or_default(), status);
        return Err(MediaError::Ffmpeg(format!("rendering \"{}\": {why}", clip.name)));
    }
    std::fs::rename(&part, &file)?;
    progress(1.0);
    Ok(Rendered { file: file.to_string_lossy().into_owned(), key, from, fps, frames, width: w, height: h, engine })
}

/// Rendered files in `dir` no clip of `project` uses any more (for clean-ups).
pub fn unused(project: &Project, dir: &Path) -> Vec<PathBuf> {
    let used: Vec<&str> = project.tracks.iter().flat_map(|t| &t.clips).filter_map(|c| c.rendered.as_ref().map(|r| r.file.as_str())).collect();
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mkv") && !used.iter().any(|u| Path::new(u) == p))
        .collect()
}

/// Premultiplied (tiny-skia) → straight RGBA (ffmpeg), in place.
fn straight(mut px: Vec<u8>) -> Vec<u8> {
    for p in px.as_chunks_mut::<4>().0.iter_mut() {
        let a = p[3] as u32;
        if a == 0 || a == 255 {
            continue;
        }
        for c in p.iter_mut().take(3) {
            *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    px
}

fn media_path(project: &Project, reference: &str) -> Option<PathBuf> {
    let asset = reference.parse::<Id>().ok().and_then(|id| project.asset(id)).or_else(|| project.assets.iter().find(|a| a.name.eq_ignore_ascii_case(reference)));
    match asset {
        Some(a) => Some(PathBuf::from(&a.path)),
        None => Some(PathBuf::from(reference)).filter(|p| p.is_file()),
    }
}

/// FNV-1a, 64 bits: stable across runs and builds (unlike `DefaultHasher`).
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        // Separates fields so "ab"+"c" ≠ "a"+"bc".
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }
}
