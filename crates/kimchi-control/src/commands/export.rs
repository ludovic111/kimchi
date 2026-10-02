use std::sync::Arc;

use kimchi_core::Project;
use kimchi_media::export::{ExportFormat, ExportSettings, Quality};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, ExportStatus, Session, err};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "export.formats" => Ok(json!({
            "formats": [
                { "id": "mp4", "label": "MP4 (H.264 + AAC)", "extension": "mp4", "note": "Plays everywhere." },
                { "id": "hevc", "label": "HEVC (H.265 + AAC)", "extension": "mp4", "note": "Smaller files." },
                { "id": "prores", "label": "ProRes 422 HQ", "extension": "mov", "note": "For further editing." },
                { "id": "webm", "label": "WebM (VP9 + Opus)", "extension": "webm", "note": "For the web." },
                { "id": "gif", "label": "Animated GIF", "extension": "gif", "note": "No sound." },
                { "id": "audio", "label": "Audio only (AAC)", "extension": "m4a", "note": "The mix." },
                { "id": "wav", "label": "Audio only (WAV)", "extension": "wav", "note": "Uncompressed 24-bit mix." },
            ],
            "qualities": ["draft", "standard", "high"],
        })),
        "export.start" => {
            let settings = ExportSettings {
                path: a.str("path")?.to_string(),
                format: format(a.opt_str("format").unwrap_or("mp4"))?,
                quality: quality(a.opt_str("quality").unwrap_or("standard"))?,
                width: a.opt_u32("width"),
                height: a.opt_u32("height"),
                fps: a.opt_f64("fps"),
                range: match (a.opt_f64("from"), a.opt_f64("to")) {
                    (None, None) => None,
                    (f, t) => Some((f.unwrap_or(0.0), t.unwrap_or(f64::MAX))),
                },
            };
            let project = s.project()?;
            let id = start(s, project, settings)?;
            if a.bool_or("wait", false) || s.headless {
                return Ok(json!(wait(s, &id).await?));
            }
            Ok(json!({ "exportId": id }))
        }
        "export.status" => match a.opt_str("exportId") {
            Some(id) => s.exports().into_iter().find(|e| e.id == id).map(|e| json!(e)).ok_or_else(|| format!("No export `{id}`.")),
            None => Ok(json!(s.exports())),
        },
        "export.cancel" => {
            let id = a.str("exportId")?;
            if !s.cancel_export(id) {
                return Err(format!("No running export `{id}`."));
            }
            Ok(json!({ "cancelled": id }))
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

pub fn format(f: &str) -> CmdResult<ExportFormat> {
    Ok(match f {
        "mp4" => ExportFormat::Mp4,
        "hevc" => ExportFormat::Hevc,
        "prores" => ExportFormat::Prores,
        "webm" => ExportFormat::Webm,
        "gif" => ExportFormat::Gif,
        "audio" => ExportFormat::Audio,
        "wav" => ExportFormat::Wav,
        other => return Err(format!("format is mp4, hevc, prores, webm, gif, audio or wav, not \"{other}\"")),
    })
}

pub fn quality(q: &str) -> CmdResult<Quality> {
    Ok(match q {
        "draft" => Quality::Draft,
        "standard" => Quality::Standard,
        "high" => Quality::High,
        other => return Err(format!("quality is draft, standard or high, not \"{other}\"")),
    })
}

/// Starts rendering `project` in the background; progress arrives as `Event::Export`.
pub fn start(s: &Arc<Session>, project: Project, settings: ExportSettings) -> CmdResult<String> {
    let tools = s.tools()?;
    let id = uuid::Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();
    let status = ExportStatus { id: id.clone(), path: settings.path.clone(), progress: 0.0, done: false, error: None, started_at: chrono::Utc::now() };
    s.add_export(status.clone(), cancel.clone());
    s.set_export(status.clone());
    let s2 = s.clone();
    s.runtime().spawn(async move {
        let overlays_dir = s2.cache_dir(project.id).join("text");
        let result = async {
            let overlays = kimchi_media::text::rasterize_overlays(&project, &overlays_dir).map_err(err)?;
            let progress_session = s2.clone();
            let base = status.clone();
            kimchi_media::export::export(
                &tools,
                &project,
                &overlays,
                &settings,
                move |p| progress_session.set_export(ExportStatus { progress: p, ..base.clone() }),
                cancel,
            )
            .await
            .map_err(err)
        }
        .await;
        let end = match result {
            Ok(()) => ExportStatus { progress: 1.0, done: true, ..status },
            Err(e) => ExportStatus { done: true, error: Some(e), ..status },
        };
        s2.set_export(end);
    });
    Ok(id)
}

/// Waits for an export to end; an error if it failed.
pub async fn wait(s: &Arc<Session>, id: &str) -> CmdResult<ExportStatus> {
    let mut rx = s.subscribe();
    loop {
        let st = s.exports().into_iter().find(|e| e.id == id).ok_or_else(|| format!("No export `{id}`."))?;
        if st.done {
            return match &st.error {
                Some(e) => Err(format!("Export failed: {e}")),
                None => Ok(st),
            };
        }
        if let Err(tokio::sync::broadcast::error::RecvError::Closed) = rx.recv().await {
            return Err("kimchi is shutting down".into());
        }
    }
}
