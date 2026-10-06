use std::sync::Arc;

use kimchi_core::Project;
use kimchi_media::EncoderChoice;
use kimchi_media::export::{AudioFormat, AudioOptions, ExportFormat, ExportSettings, Quality};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, ExportStatus, Session, err};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "export.presets" => {
            let project = s.project().ok();
            Ok(json!(PRESETS.iter().map(|p| describe(p, project.as_ref().map(|p| &p.settings))).collect::<Vec<_>>()))
        }
        "export.formats" => Ok(json!({
            "formats": [
                { "id": "mp4", "label": "MP4 (H.264 + AAC)", "extension": "mp4", "note": "Plays everywhere." },
                { "id": "hevc", "label": "HEVC (H.265 + AAC)", "extension": "mp4", "note": "Smaller files." },
                { "id": "prores", "label": "ProRes 422 HQ", "extension": "mov", "note": "For further editing." },
                { "id": "webm", "label": "WebM (VP9 + Opus)", "extension": "webm", "note": "For the web." },
                { "id": "gif", "label": "Animated GIF", "extension": "gif", "note": "No sound." },
                { "id": "audio", "label": "Audio only", "extension": "m4a", "note": "The mix: AAC in M4A, or the file type audioFormat names (audioFormats)." },
                { "id": "wav", "label": "Audio only (WAV)", "extension": "wav", "note": "Uncompressed 24-bit mix." },
            ],
            "audioFormats": AudioFormat::ALL.iter().map(|f| json!({ "id": f, "extension": f.extension(), "lossy": f.is_lossy() })).collect::<Vec<_>>(),
            "sampleRates": kimchi_media::export::AUDIO_RATES,
            "qualities": ["draft", "standard", "high"],
            "encoders": ["auto", "hardware", "software"],
        })),
        "export.encoders" => {
            let tools = s.tools()?;
            let (hw, formats) = kimchi_media::export::encoders(&tools).await.map_err(err)?;
            let named = |e: &Option<String>| e.as_ref().map(|e| json!({ "id": e, "label": kimchi_media::accel::label(e) }));
            Ok(json!({
                "hardware": hw.encoders.iter().map(|v| json!({
                    "id": v.encoder.name,
                    "label": v.encoder.api.label(),
                    "codec": v.encoder.codec,
                    "constantQuality": v.constant_quality,
                })).collect::<Vec<_>>(),
                "formats": formats.iter().map(|f| json!({
                    "format": f.format,
                    "auto": named(&f.auto),
                    "hardware": named(&f.hardware),
                    "software": named(&f.software),
                })).collect::<Vec<_>>(),
            }))
        }
        "export.start" => {
            // A preset's settings first, the parameters given on top.
            let a = match a.opt_str("preset") {
                Some(id) => {
                    let p = preset(id)?;
                    let mut merged = preset_params(p, &s.project()?.settings);
                    merged.extend(a.0.clone());
                    Args(merged)
                }
                None => a,
            };
            let settings = ExportSettings {
                path: crate::commands::media::path_str(&crate::commands::media::absolute(a.str("path")?)?),
                format: format(a.opt_str("format").unwrap_or("mp4"))?,
                quality: quality(a.opt_str("quality").unwrap_or("standard"))?,
                width: a.opt_u32("width"),
                height: a.opt_u32("height"),
                fps: a.opt_f64("fps"),
                range: match (a.opt_f64("from"), a.opt_f64("to")) {
                    (None, None) => None,
                    (f, t) => Some((f.unwrap_or(0.0), t.unwrap_or(f64::MAX))),
                },
                encoder: encoder(a.opt_str("encoder").unwrap_or("auto"))?,
                audio: audio_options(&a)?,
            };
            let mut project = s.project()?;
            // Captions: in the picture, in a file next to it, both or neither.
            let captions = a.opt_str("captions").unwrap_or("burn");
            let (burn, file) = match captions {
                "burn" => (true, false),
                "file" | "srt" | "sidecar" => (false, true),
                "both" => (true, true),
                "none" => (false, false),
                other => return Err(format!("captions is burn, file, both or none, not \"{other}\"")),
            };
            if !burn {
                for t in project.tracks.iter_mut().filter(|t| t.captions) {
                    t.hidden = true;
                }
            }
            let sidecar = if file && !settings.format.is_audio_only() { crate::commands::captions::sidecar(&project, std::path::Path::new(&settings.path))? } else { None };
            let id = start(s, project, settings)?;
            if let Some(path) = &sidecar {
                tracing::info!("captions written to {}", path.display());
            }
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

/// How a preset sizes the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PresetSize {
    /// The project's size.
    Project,
    /// The short side at this many pixels, the project's aspect kept.
    Short(u32),
    /// As large as fits in this box, the project's aspect kept (a 9:16 box for a 9:16 project
    /// is filled exactly).
    Fit(u32, u32),
}

/// A ready-made export setting.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    /// Web and social, Editing and finishing, Sound.
    pub group: &'static str,
    pub format: &'static str,
    pub quality: &'static str,
    pub size: PresetSize,
    pub fps: Option<f64>,
    /// LUFS.
    pub loudness: Option<f64>,
    /// Sound-only exports' file type.
    pub audio_format: Option<&'static str>,
    pub note: &'static str,
}

#[allow(clippy::too_many_arguments)]
const fn entry(id: &'static str, label: &'static str, group: &'static str, format: &'static str, quality: &'static str, size: PresetSize, fps: Option<f64>, loudness: Option<f64>, audio_format: Option<&'static str>, note: &'static str) -> Preset {
    Preset { id, label, group, format, quality, size, fps, loudness, audio_format, note }
}

const WEB: &str = "Web and social";
const EDIT: &str = "Editing and finishing";
const SOUND: &str = "Sound";

/// Every export preset, in the order the dialog shows them.
pub const PRESETS: &[Preset] = &[
    entry("youtube-1080p", "YouTube 1080p", WEB, "mp4", "high", PresetSize::Short(1080), None, Some(-14.0), None, "H.264 at 1080p, sound at YouTube's -14 LUFS."),
    entry("youtube-4k", "YouTube 4K", WEB, "mp4", "high", PresetSize::Short(2160), None, Some(-14.0), None, "H.264 at 2160p: YouTube keeps more detail for 4K uploads."),
    entry("shorts", "Shorts, TikTok, Reels", WEB, "mp4", "high", PresetSize::Fit(1080, 1920), Some(30.0), Some(-14.0), None, "1080×1920 at 30 fps for a vertical (9:16) project."),
    entry("instagram-portrait", "Instagram feed 4:5", WEB, "mp4", "high", PresetSize::Fit(1080, 1350), Some(30.0), Some(-14.0), None, "1080×1350 at 30 fps for a 4:5 project."),
    entry("instagram-square", "Instagram feed square", WEB, "mp4", "high", PresetSize::Fit(1080, 1080), Some(30.0), Some(-14.0), None, "1080×1080 at 30 fps for a square project."),
    entry("x", "X (Twitter)", WEB, "mp4", "standard", PresetSize::Fit(1920, 1200), Some(30.0), Some(-14.0), None, "H.264 up to 1920 wide at 30 fps: what X plays without re-encoding badly."),
    entry("vimeo", "Vimeo", WEB, "mp4", "high", PresetSize::Short(1080), None, Some(-16.0), None, "H.264 at 1080p, high quality."),
    entry("linkedin", "LinkedIn", WEB, "mp4", "standard", PresetSize::Short(1080), Some(30.0), Some(-14.0), None, "H.264 at 1080p, 30 fps."),
    entry("gif", "GIF", WEB, "gif", "standard", PresetSize::Fit(720, 720), Some(15.0), None, None, "A looping GIF up to 720 px at 15 fps, no sound."),
    entry("editor", "For Premiere, Resolve or Final Cut", EDIT, "prores", "high", PresetSize::Project, None, None, None, "ProRes 422 HQ at the project's size: every editor opens it."),
    entry("broadcast", "Broadcast", EDIT, "prores", "high", PresetSize::Project, None, Some(-23.0), None, "ProRes 422 HQ with the sound at -23 LUFS (EBU R 128)."),
    entry("archive", "Archive", EDIT, "prores", "high", PresetSize::Project, None, None, None, "ProRes 422 HQ at full size and quality, to keep."),
    entry("podcast", "Podcast audio", SOUND, "audio", "high", PresetSize::Project, None, Some(-16.0), Some("mp3"), "MP3 at -16 LUFS, what podcast apps expect."),
    entry("podcast-aac", "Podcast audio (AAC)", SOUND, "audio", "high", PresetSize::Project, None, Some(-16.0), Some("aac"), "AAC (M4A) at -16 LUFS."),
    entry("broadcast-audio", "Broadcast audio", SOUND, "wav", "high", PresetSize::Project, None, Some(-23.0), None, "24-bit WAV at -23 LUFS (EBU R 128)."),
];

/// The preset with this id or label (case-insensitive).
pub fn preset(id: &str) -> CmdResult<&'static Preset> {
    let k = id.trim();
    PRESETS.iter().find(|p| p.id.eq_ignore_ascii_case(k) || p.label.eq_ignore_ascii_case(k)).ok_or_else(|| {
        let ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        let hint = kimchi_core::closest(k, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        format!("Unknown export preset `{k}`.{hint} Presets: {}.", ids.join(", "))
    })
}

/// Even output size for a preset on a canvas of `ps`'s size, `None` for the project's own.
pub fn preset_size(p: &Preset, ps: &kimchi_core::ProjectSettings) -> Option<(u32, u32)> {
    let (w, h) = (ps.width.max(1) as f64, ps.height.max(1) as f64);
    let k = match p.size {
        PresetSize::Project => return None,
        PresetSize::Short(short) => short as f64 / w.min(h),
        PresetSize::Fit(bw, bh) => (bw as f64 / w).min(bh as f64 / h),
    };
    let even = |v: f64| (((v * k) / 2.0).round() as u32).max(1) * 2;
    Some((even(w), even(h)))
}

/// The `export.start` parameters a preset sets for this project.
pub fn preset_params(p: &Preset, ps: &kimchi_core::ProjectSettings) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("format".into(), json!(p.format));
    m.insert("quality".into(), json!(p.quality));
    if let Some((w, h)) = preset_size(p, ps) {
        m.insert("width".into(), json!(w));
        m.insert("height".into(), json!(h));
    }
    if let Some(f) = p.fps {
        m.insert("fps".into(), json!(f));
    }
    if let Some(l) = p.loudness {
        m.insert("loudness".into(), json!(l));
    }
    if let Some(f) = p.audio_format {
        m.insert("audioFormat".into(), json!(f));
    }
    m
}

/// How `export.presets` shows a preset (with the size it gives the open project).
fn describe(p: &Preset, ps: Option<&kimchi_core::ProjectSettings>) -> serde_json::Value {
    let mut v = json!({
        "id": p.id,
        "label": p.label,
        "group": p.group,
        "note": p.note,
        "extension": extension(p.format, p.audio_format),
    });
    if let Some(ps) = ps {
        let params = preset_params(p, ps);
        v["params"] = json!(params);
        if let PresetSize::Fit(bw, bh) = p.size
            && let Some((w, h)) = preset_size(p, ps)
            && (w, h) != (bw, bh)
        {
            v["warning"] = json!(format!("This project is {}×{}: it comes out {w}×{h}. Make a {bw}×{bh} project (project.setSettings) to fill the frame.", ps.width, ps.height));
        }
    } else {
        v["format"] = json!(p.format);
    }
    v
}

/// The file extension a format (and sound-only file type) is written with.
pub fn extension(format: &str, audio_format: Option<&str>) -> &'static str {
    match format {
        "mp4" | "hevc" => "mp4",
        "prores" => "mov",
        "webm" => "webm",
        "gif" => "gif",
        "wav" => "wav",
        _ => audio_format.and_then(|f| AudioFormat::parse(f).ok()).map_or("m4a", |f| f.extension()),
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

/// The sound options of `export.start`.
pub fn audio_options(a: &Args) -> CmdResult<AudioOptions> {
    let sample_rate = a.opt_u32("sampleRate");
    if let Some(r) = sample_rate
        && !kimchi_media::export::AUDIO_RATES.contains(&r)
    {
        return Err(format!("sampleRate is 44100, 48000 or 96000, not {r}"));
    }
    let bit_depth = a.opt_u32("bitDepth");
    if let Some(d) = bit_depth
        && ![16, 24, 32].contains(&d)
    {
        return Err(format!("bitDepth is 16, 24 or 32, not {d}"));
    }
    let bitrate_kbps = a.opt_u32("bitrate");
    if let Some(b) = bitrate_kbps
        && !(32..=512).contains(&b)
    {
        return Err(format!("bitrate goes from 32 to 512 kbit/s, not {b}"));
    }
    Ok(AudioOptions {
        format: a.opt_str("audioFormat").map(AudioFormat::parse).transpose()?,
        sample_rate,
        bit_depth,
        bitrate_kbps,
        stems: a.bool_or("stems", false),
        stems_master: a.bool_or("stemsMaster", false),
        loudness: match a.0.get("loudness") {
            None => None,
            Some(v) => crate::commands::audio::loudness(v)?,
        },
    })
}

pub fn encoder(e: &str) -> CmdResult<EncoderChoice> {
    Ok(match e {
        "auto" => EncoderChoice::Auto,
        "hardware" | "gpu" => EncoderChoice::Hardware,
        "software" | "cpu" => EncoderChoice::Software,
        other => return Err(format!("encoder is auto, hardware or software, not \"{other}\"")),
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
    let status = ExportStatus { id: id.clone(), path: settings.path.clone(), progress: 0.0, done: false, error: None, started_at: chrono::Utc::now(), encoder: None };
    s.add_export(status.clone(), cancel.clone());
    s.set_export(status.clone());
    let s2 = s.clone();
    s.runtime().spawn(async move {
        let mut status = status;
        // Which encoder it starts on (the GPU's, usually), shown while it runs.
        if let Ok(encoder) = kimchi_media::export::planned_encoder(&tools, &project, &settings).await {
            status.encoder = encoder;
            s2.set_export(status.clone());
        }
        let result = async {
            let progress_session = s2.clone();
            let base = status.clone();
            kimchi_media::export::export(
                &tools,
                &project,
                &settings,
                move |p| progress_session.set_export(ExportStatus { progress: p, ..base.clone() }),
                cancel,
            )
            .await
            .map_err(err)
        }
        .await;
        let end = match result {
            Ok(done) => ExportStatus { progress: 1.0, done: true, encoder: done.encoder, ..status },
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
