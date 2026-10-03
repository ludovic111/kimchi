//! Captions: titles on a captions track (a video track marked `captions`), made by listening to
//! the cut (Whisper, locally), read from or written to SRT / WebVTT, styled together.
//!
//! A transcription runs one at a time; `captions.status` says where it is (the window shows it).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_captions::whisper::{self, MODELS, Model};
use kimchi_captions::{Cue, Format};
use kimchi_core::{Clip, ClipContent, ClipPatch, Edit, Id, Project, TextStyle, TrackKind, TrackPatch};
use serde::Serialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::commands::clip::patch_text;
use crate::commands::project::round;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session, ToastKind, err};

/// Default longest caption line, in characters.
const MAX_CHARS: usize = 42;

/// The transcription in progress, if any.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// "downloading" (the model), "mixing" (the sound), "listening".
    pub stage: &'static str,
    /// 0–1 within the stage.
    pub progress: f64,
    pub model: &'static str,
    #[serde(skip)]
    cancel: CancellationToken,
}

fn status() -> &'static Mutex<Option<Status>> {
    static S: OnceLock<Mutex<Option<Status>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn set_progress(stage: &'static str, p: f64) {
    if let Some(st) = status().lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        st.stage = stage;
        st.progress = p;
    }
}

/// The transcription running now (for the window).
pub fn current() -> Option<Status> {
    status().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Where speech models are kept.
pub fn models_dir(s: &Session) -> PathBuf {
    s.data_dir.join("models")
}

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "captions.list" => {
            let p = s.project()?;
            Ok(json!(p.captions().into_iter().map(|(_, c, text)| json!({ "clipId": c.id, "start": round(c.start), "end": round(c.end()), "text": text })).collect::<Vec<_>>()))
        }
        "captions.models" => {
            let dir = models_dir(s);
            Ok(json!(MODELS.iter().map(|m| json!({ "id": m.id, "label": m.label, "sizeMb": m.size_mb, "downloaded": m.model.is_downloaded(&dir), "description": m.doc, "default": m.model == Model::Base })).collect::<Vec<_>>()))
        }
        "captions.status" => Ok(match current() {
            Some(st) => json!({ "running": true, "stage": st.stage, "progress": round(st.progress), "model": st.model }),
            None => json!({ "running": false }),
        }),
        "captions.cancel" => match current() {
            Some(st) => {
                st.cancel.cancel();
                Ok(json!({ "cancelled": true }))
            }
            None => Err("No transcription is running.".into()),
        },
        "captions.transcribe" => transcribe(s, cx, &a).await,
        "captions.import" => {
            let path = crate::commands::media::absolute(a.str("path")?)?;
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut cues = kimchi_captions::parse(&kimchi_captions::decode(&bytes))?;
            let offset = a.opt_f64("offset").unwrap_or(0.0);
            for c in &mut cues {
                c.start = (c.start + offset).max(0.0);
                c.end = (c.end + offset).max(c.start);
            }
            let p = s.project()?;
            let span = (cues.first().map_or(0.0, |c| c.start), cues.last().map_or(0.0, |c| c.end));
            let n = cues.len();
            let track = place(s, cx, &p, &cues, a.bool_or("replace", false).then_some(span))?;
            Ok(json!({ "captions": n, "trackId": track }))
        }
        "captions.export" => {
            let p = s.project()?;
            let path = crate::commands::media::absolute(a.str("path")?)?;
            let format = match a.opt_str("format") {
                Some(f) => Format::parse(f)?,
                None => Format::of_path(&path).unwrap_or(Format::Srt),
            };
            let cues = cues_of(&p);
            if cues.is_empty() {
                return Err("There are no captions yet: transcribe, import or add some first.".into());
            }
            write_file(&path, &cues, format)?;
            Ok(json!({ "path": path, "captions": cues.len(), "format": format }))
        }
        "captions.add" => {
            let p = s.project()?;
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
            let cue = Cue { start, end: start + a.opt_f64("duration").unwrap_or(2.5).max(0.1), text: a.str("text")?.to_string() };
            let track = place(s, cx, &p, &[cue], None)?;
            let p = s.project()?;
            let clip = p.track(track).and_then(|t| t.clips.iter().find(|c| (c.start - start).abs() < 1e-6)).map(|c| c.id);
            Ok(json!({ "trackId": track, "clipId": clip }))
        }
        "captions.setStyle" => {
            let p = s.project()?;
            let caps = p.captions();
            if caps.is_empty() {
                return Err("There are no captions to style yet.".into());
            }
            let mut edits = vec![];
            for (_, c, _) in &caps {
                let ClipContent::Text { style } = &c.content else { continue };
                let mut style = style.clone();
                if let Some(o) = a.object("style") {
                    patch_text(&mut style, o)?;
                }
                let mut patch = ClipPatch { text: Some(style), ..Default::default() };
                if let Some(y) = a.opt_f64("y") {
                    let mut t = c.transform.clone();
                    t.y = y;
                    patch.transform = Some(t);
                }
                edits.push(Edit::UpdateClip { clip_id: c.id, patch });
            }
            crate::commands::clip::apply_all(s, cx, &edits, a.coalesce())?;
            Ok(json!({ "styled": edits.len() }))
        }
        "captions.clear" => {
            let p = s.project()?;
            let ids: Vec<Id> = p.captions().into_iter().map(|(_, c, _)| c.id).collect();
            if ids.is_empty() {
                return Err("There are no captions.".into());
            }
            s.apply(cx.label(), cx.source, &Edit::DeleteClips { clip_ids: ids.clone(), ripple: false }, None)?;
            Ok(json!({ "removed": ids.len() }))
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

/// The captions as cues, by time.
pub fn cues_of(p: &Project) -> Vec<Cue> {
    p.captions().into_iter().map(|(_, c, text)| Cue { start: c.start, end: c.end(), text: text.to_string() }).collect()
}

/// Writes the project's captions next to an export (`movie.mp4` → `movie.srt`), if it has any.
pub fn sidecar(p: &Project, video: &Path) -> CmdResult<Option<PathBuf>> {
    let cues = cues_of(p);
    if cues.is_empty() {
        return Ok(None);
    }
    let path = video.with_extension("srt");
    write_file(&path, &cues, Format::Srt)?;
    Ok(Some(path))
}

fn write_file(path: &Path, cues: &[Cue], format: Format) -> CmdResult<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::write(path, kimchi_captions::write(cues, format)).map_err(|e| format!("{}: {e}", path.display()))
}

/// How new captions look: like the first caption already there, else white on a dark box, a
/// tenth of the frame up from the bottom.
fn look(p: &Project) -> (TextStyle, f64) {
    if let Some((_, c, _)) = p.captions().first()
        && let ClipContent::Text { style } = &c.content
    {
        return (style.clone(), c.transform.y);
    }
    let h = p.settings.height as f64;
    let style = TextStyle {
        content: String::new(),
        font_family: "Manrope".into(),
        font_size: (h * 0.048).round(),
        font_weight: 600,
        italic: false,
        color: "#ffffff".into(),
        background: Some("#000000b3".into()),
        align: kimchi_core::TextAlign::Center,
        line_height: 1.25,
        letter_spacing: 0.0,
        shadow: false,
    };
    (style, (h * 0.37).round())
}

/// Puts cues on the captions track (made if there is none) as one undo step, first removing the
/// captions inside `replace` (a time span). Returns the track.
fn place(s: &Arc<Session>, cx: &Ctx, p: &Project, cues: &[Cue], replace: Option<(f64, f64)>) -> CmdResult<Id> {
    let (style, y) = look(p);
    let existing = p.caption_track().map(|t| t.id);
    let old: Vec<Id> = match replace {
        Some((a, b)) => p.captions().into_iter().filter(|(_, c, _)| c.end() > a + 1e-6 && c.start < b - 1e-6).map(|(_, c, _)| c.id).collect(),
        None => vec![],
    };
    let clips: Vec<Clip> = cues
        .iter()
        .filter(|c| c.end > c.start && !c.text.trim().is_empty())
        .map(|c| {
            let mut st = style.clone();
            st.content = c.text.trim().to_string();
            let mut clip = Clip::new(crate::commands::clip::text_name(&st.content), c.start, c.end - c.start, ClipContent::Text { style: st });
            clip.transform.y = y;
            clip
        })
        .collect();
    s.edit(cx.label(), cx.source, |ed| {
        ed.begin_batch(cx.label(), cx.source.as_str());
        let steps = |ed: &mut kimchi_core::Editor| -> Result<Id, kimchi_core::EditError> {
            let track = match existing {
                Some(t) => t,
                None => {
                    let t = ed.apply(&Edit::AddTrack { kind: TrackKind::Video, index: Some(0) }, None)?.created_tracks[0];
                    let patch = TrackPatch { name: Some("Captions".into()), captions: Some(true), ..Default::default() };
                    ed.apply(&Edit::UpdateTrack { track_id: t, patch }, None)?;
                    t
                }
            };
            if !old.is_empty() {
                ed.apply(&Edit::DeleteClips { clip_ids: old.clone(), ripple: false }, None)?;
            }
            for c in &clips {
                ed.apply(&Edit::AddClip { track_id: Some(track), clip: c.clone() }, None)?;
            }
            Ok(track)
        };
        match steps(ed) {
            Ok(t) => {
                ed.end_batch();
                Ok(t)
            }
            Err(e) => {
                ed.rollback_batch();
                Err(err(e))
            }
        }
    })
}

/// `captions.transcribe`: mix the sound, listen, put the captions on.
async fn transcribe(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let model = Model::parse(a.opt_str("model").unwrap_or("base"))?;
    let language = a.opt_str("language").map(str::to_string);
    let max_chars = a.opt_u32("maxChars").map_or(MAX_CHARS, |n| n as usize);
    // What to listen to: one clip's sound, or the mix over a range (the whole cut by default).
    let (source, range) = match a.opt_str("clipId") {
        Some(key) => {
            let id = resolve::clip(&p, key)?;
            let clip = p.clip(id).ok_or("clip not found")?.clone();
            let mut alone = p.clone();
            for t in &mut alone.tracks {
                t.clips.retain(|c| c.id == id);
                t.muted = false;
            }
            (alone, (clip.start, clip.end()))
        }
        None => {
            let end = p.duration();
            let (from, to) = (a.opt_f64("from").unwrap_or(0.0).max(0.0), a.opt_f64("to").unwrap_or(end).min(end));
            if to - from < 0.5 {
                return Err("There is less than half a second to listen to.".into());
            }
            // The captions themselves make no sound, but they are on the timeline already.
            (p.clone(), (from, to))
        }
    };
    let cancel = CancellationToken::new();
    {
        let mut st = status().lock().unwrap_or_else(|e| e.into_inner());
        if st.is_some() {
            return Err("A transcription is already running (captions.status).".into());
        }
        *st = Some(Status { stage: "mixing", progress: 0.0, model: model.info().id, cancel: cancel.clone() });
    }
    // Cleared however this ends, even when the caller stops waiting (the future is dropped):
    // then the download or the listening thread is stopped too.
    struct Running(CancellationToken);
    impl Drop for Running {
        fn drop(&mut self) {
            self.0.cancel();
            *status().lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
    let running = Running(cancel.clone());
    let result = async {
        let dir = models_dir(s);
        if !model.is_downloaded(&dir) {
            set_progress("downloading", 0.0);
            whisper::download(&dir, model, |p| set_progress("downloading", p), &cancel).await?;
        }
        set_progress("mixing", 0.0);
        let tools = s.tools()?;
        let samples = kimchi_media::speech_samples(&tools, &source, Some(range)).await.map_err(err)?;
        set_progress("listening", 0.0);
        let model_dir = model.dir(&dir);
        let c2 = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let mut w = whisper::Whisper::load(&model_dir)?;
            let mut t = w.transcribe(&samples, language.as_deref(), &|p| set_progress("listening", p), &c2)?;
            kimchi_captions::tighten(&mut t.segments, &samples, whisper::SAMPLE_RATE);
            Ok::<_, String>(t)
        })
        .await
        .map_err(err)?
    }
    .await;
    drop(running);
    let transcript = result.map_err(|e| if e == "cancelled" { "The transcription was cancelled.".to_string() } else { e })?;
    let mut cues = kimchi_captions::cues_from_segments(&transcript.segments, max_chars);
    for c in &mut cues {
        c.start += range.0;
        c.end = (c.end + range.0).min(range.1);
    }
    cues.retain(|c| c.end - c.start > 0.05);
    let n = cues.len();
    if n == 0 {
        s.toast(ToastKind::Info, "No speech was heard.");
        return Ok(json!({ "captions": 0, "language": transcript.language }));
    }
    let p = s.project()?;
    let track = place(s, cx, &p, &cues, a.bool_or("replace", true).then_some(range))?;
    s.toast(ToastKind::Success, format!("{n} caption{} added", if n == 1 { "" } else { "s" }));
    Ok(json!({ "captions": n, "language": transcript.language, "trackId": track, "text": cues.iter().map(|c| c.text.replace('\n', " ")).collect::<Vec<_>>().join(" ") }))
}
