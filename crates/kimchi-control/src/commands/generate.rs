//! Bridges the generation harness and the editor: jobs start from any client,
//! placeholders appear on the timeline, and finished media replaces them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, Edit, Generation, Id, MediaKind, new_id};
use kimchi_gen::{GenRequest, ImageRole, InputImage, Job, JobStatus, ModelInfo, OutputKind, Task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::media::{add_asset, clip_frame, path_str};
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Event, Session, ToastKind, err};

/// What the editor stores in [`Job::tag`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobTag {
    project_id: Id,
    input_assets: Vec<Id>,
}

/// Where a generation's result should go.
#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    /// Only into the media library.
    Library,
    /// A placeholder clip on the timeline that becomes the result.
    Timeline { track_id: Option<Id>, start: f64, duration: f64 },
}

/// Everything needed to start one job (built by the commands below or by the window's composer).
#[derive(Debug, Clone)]
pub struct Submit {
    pub provider: String,
    pub request: GenRequest,
    pub placement: Placement,
    /// Library assets used as inputs, recorded for provenance.
    pub input_assets: Vec<Id>,
}

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "generate.providers" => Ok(json!(s.harness.statuses())),
        "generate.models" => {
            let mut models = match a.opt_str("provider") {
                Some(p) => s.harness.models(p, a.bool_or("refresh", false)).await.map_err(err)?,
                None => s.harness.all_models().await,
            };
            if let Some(t) = a.opt_str("task") {
                let task = task(t)?;
                models.retain(|m| m.supports(task));
            }
            Ok(json!(models))
        }
        "generate.check" => Ok(json!({ "message": s.harness.check(a.str("provider")?).await.map_err(err)? })),
        "generate.setKey" => {
            s.harness.set_key(a.str("provider")?, a.opt_str("key").filter(|k| !k.is_empty())).map_err(err)?;
            s.emit(Event::SettingsChanged);
            Ok(json!(s.harness.statuses()))
        }
        "generate.setProvider" => {
            let id = a.str("provider")?;
            let mut settings = s.harness.settings().remove(id).unwrap_or_default();
            if s.harness.statuses().iter().all(|p| p.info.id != id) {
                return Err(format!("Unknown provider `{id}`. generate.providers lists them."));
            }
            if let Some(v) = a.opt_bool("enabled") {
                settings.enabled = v;
            }
            if let Some(v) = a.opt_str("baseUrl") {
                settings.base_url = Some(v.to_string()).filter(|u| !u.trim().is_empty());
            }
            if let Some(o) = a.object("options") {
                settings.options.extend(o.clone());
            }
            s.harness.set_settings(id, settings);
            s.save_provider_settings()?;
            s.emit(Event::SettingsChanged);
            Ok(json!(s.harness.statuses().into_iter().find(|p| p.info.id == id)))
        }
        "generate.submit" => {
            let req = request_from(s, &a).await?;
            let placement = placement_from(s, &a, &req.request)?;
            finish(s, &a, submit(s, Submit { placement, ..req })?).await
        }
        "generate.animateFrame" => {
            let p = s.project()?;
            let clip_id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(clip_id).ok_or("clip not found")?.clone();
            let path = clip_frame(s, clip_id, a.opt_f64("time")).await?;
            let mut sub = model_request(s, &a, Task::ImageToVideo, a.opt_str("prompt").unwrap_or("")).await?;
            sub.request.images.push(image(ImageRole::StartFrame, path));
            sub.input_assets.extend(clip.asset_id());
            let d = sub.request.duration.unwrap_or(5.0);
            sub.placement = Placement::Timeline { track_id: None, start: clip.end(), duration: d };
            finish(s, &a, submit(s, sub)?).await
        }
        "generate.extendClip" => {
            let p = s.project()?;
            let clip_id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(clip_id).ok_or("clip not found")?.clone();
            let track = p.locate_clip(clip_id).map(|(ti, _)| p.tracks[ti].id);
            let path = clip_frame(s, clip_id, Some(clip.end() - p.settings.frame())).await?;
            let prompt = a.opt_str("prompt").map(str::to_string).unwrap_or_else(|| format!("Continue the shot \"{}\"", clip.name));
            let mut sub = model_request(s, &a, Task::ImageToVideo, &prompt).await?;
            sub.request.images.push(image(ImageRole::StartFrame, path));
            sub.input_assets.extend(clip.asset_id());
            let d = sub.request.duration.unwrap_or(5.0);
            sub.placement = Placement::Timeline { track_id: track, start: clip.end(), duration: d };
            finish(s, &a, submit(s, sub)?).await
        }
        "generate.bridge" => {
            let p = s.project()?;
            let x = p.clip(resolve::clip(&p, a.str("fromClipId")?)?).ok_or("clip not found")?.clone();
            let y = p.clip(resolve::clip(&p, a.str("toClipId")?)?).ok_or("clip not found")?.clone();
            let (first, last) = if x.start <= y.start { (x, y) } else { (y, x) };
            let start = clip_frame(s, first.id, Some(first.end() - p.settings.frame())).await?;
            let end = clip_frame(s, last.id, Some(last.start)).await?;
            let gap = (last.start - first.end()).max(0.0);
            let prompt = a.opt_str("prompt").map(str::to_string).unwrap_or_else(|| format!("A smooth transition from \"{}\" to \"{}\"", first.name, last.name));
            let mut sub = model_request(s, &a, Task::ImageToVideo, &prompt).await?;
            if gap > 0.0 && sub.request.duration.is_none() {
                sub.request.duration = Some(gap);
            }
            sub.request.images.push(image(ImageRole::StartFrame, start));
            sub.request.images.push(image(ImageRole::EndFrame, end));
            let track = p.locate_clip(first.id).map(|(ti, _)| p.tracks[ti].id);
            let d = if gap > 0.0 { gap } else { sub.request.duration.unwrap_or(4.0) };
            sub.placement = Placement::Timeline { track_id: track, start: first.end(), duration: d };
            finish(s, &a, submit(s, sub)?).await
        }
        "generate.restyleFrame" => {
            let p = s.project()?;
            let clip_id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(clip_id).ok_or("clip not found")?.clone();
            let time = a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead);
            let path = clip_frame(s, clip_id, Some(time)).await?;
            let mut sub = model_request(s, &a, Task::ImageToImage, a.str("prompt")?).await?;
            sub.request.images.push(image(ImageRole::Reference, path));
            sub.input_assets.extend(clip.asset_id());
            sub.placement = Placement::Timeline { track_id: None, start: time.clamp(clip.start, clip.end()), duration: 3.0 };
            finish(s, &a, submit(s, sub)?).await
        }
        "generate.regenerate" => {
            let p = s.project()?;
            // A clip's result lands after it; a media item's goes to the library.
            let (asset, placement) = match (a.opt_str("clipId"), a.opt_str("assetId")) {
                (Some(k), _) => {
                    let clip_id = resolve::clip(&p, k)?;
                    let clip = p.clip(clip_id).ok_or("clip not found")?;
                    let asset = clip.asset_id().and_then(|id| p.asset(id)).ok_or("Only generated media clips can be regenerated.")?;
                    let track = p.locate_clip(clip_id).map(|(ti, _)| p.tracks[ti].id);
                    (asset, Placement::Timeline { track_id: track, start: clip.end(), duration: clip.duration })
                }
                (None, Some(k)) => {
                    let id = resolve::asset(&p, k)?;
                    (p.asset(id).ok_or("media not found")?, Placement::Library)
                }
                (None, None) => return Err("Give clipId (a generated clip) or assetId (a generated media item).".into()),
            };
            let AssetOrigin::Generated(g) = &asset.origin else { return Err(format!("\"{}\" wasn't generated.", asset.name)) };
            let mut request: GenRequest = serde_json::from_value(g.params.clone()).map_err(|e| format!("This clip's generation settings can't be read back: {e}"))?;
            if let Some(prompt) = a.opt_str("prompt") {
                request.prompt = prompt.to_string();
            }
            request.seed = if a.bool_or("variation", false) { None } else { g.seed.or(request.seed) };
            // Images were saved by path; the harness loads them again.
            for img in &mut request.images {
                img.data = Default::default();
            }
            let sub = Submit { provider: g.provider.clone(), request, placement, input_assets: g.inputs.clone() };
            finish(s, &a, submit(s, sub)?).await
        }
        "generate.jobs" => {
            let mut jobs = s.harness.jobs();
            jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));
            Ok(json!(jobs))
        }
        "generate.wait" => Ok(json!(wait(s, a.str("jobId")?, a.opt_f64("timeout").unwrap_or(600.0)).await?)),
        "generate.cancel" => {
            let id = a.str("jobId")?;
            if s.harness.job(id).is_none() {
                return Err(format!("No job `{id}`. generate.jobs lists them."));
            }
            s.harness.cancel(id);
            Ok(json!({ "cancelled": id }))
        }
        "generate.clearFinished" => {
            s.harness.clear_finished();
            Ok(json!(s.harness.jobs()))
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

fn image(role: ImageRole, path: String) -> InputImage {
    InputImage { role, path, mime: String::new(), data: Default::default() }
}

pub fn task(t: &str) -> CmdResult<Task> {
    Ok(match t {
        "text_to_audio" | "textToAudio" => Task::TextToAudio,
        "text_to_speech" | "textToSpeech" => Task::TextToSpeech,
        "text_to_image" | "textToImage" => Task::TextToImage,
        "image_to_image" | "imageToImage" => Task::ImageToImage,
        "text_to_video" | "textToVideo" => Task::TextToVideo,
        "image_to_video" | "imageToVideo" => Task::ImageToVideo,
        other => return Err(format!("task is text_to_audio, text_to_speech, text_to_image, image_to_image, text_to_video or image_to_video, not \"{other}\"")),
    })
}

/// Waits for the job when asked to (and always without a window, where the
/// process would otherwise exit before the job ends).
async fn finish(s: &Arc<Session>, a: &Args, job: Job) -> CmdResult {
    if a.bool_or("wait", false) || s.headless {
        return Ok(json!(wait(s, &job.id, 1800.0).await?));
    }
    Ok(json!(job))
}

/// Longest `generate.wait`, in seconds (a day).
const MAX_WAIT: f64 = 86_400.0;

/// Waits until a job is done and its result has landed in the project (`timeout`: 1 s to a day).
pub async fn wait(s: &Arc<Session>, id: &str, timeout: f64) -> CmdResult<Job> {
    if !timeout.is_finite() {
        return Err("timeout should be a number of seconds".into());
    }
    let timeout = timeout.clamp(1.0, MAX_WAIT);
    let mut rx = s.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let job = s.harness.job(id).ok_or_else(|| format!("No job `{id}`."))?;
        if job.status.is_done() && !has_pending(s, id) {
            return Ok(job);
        }
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => return Err(format!("Job {id} is still running after {timeout:.0} s; generate.wait waits again.")),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return Err("kimchi is shutting down".into()),
            Ok(_) => {}
        }
    }
}

fn has_pending(s: &Session, job_id: &str) -> bool {
    s.read(|ed| ed.project().clips().any(|(_, c)| matches!(&c.content, ClipContent::Pending { job_id: j, .. } if j == job_id))).unwrap_or(false)
}

/// The request `generate.submit` describes.
async fn request_from(s: &Arc<Session>, a: &Args) -> CmdResult<Submit> {
    let mut images = vec![];
    let mut inputs = vec![];
    let p = s.project()?;
    for (i, v) in a.array("images").into_iter().flatten().enumerate() {
        let role = match v.get("role").and_then(Value::as_str).unwrap_or("reference") {
            "reference" => ImageRole::Reference,
            "start_frame" | "startFrame" | "start" => ImageRole::StartFrame,
            "end_frame" | "endFrame" | "end" => ImageRole::EndFrame,
            other => return Err(format!("images[{i}].role is reference, start_frame or end_frame, not \"{other}\"")),
        };
        let path = if let Some(path) = v.get("path").and_then(Value::as_str) {
            // A frame grabbed from a clip still records where it came from.
            if let Some(key) = v.get("assetId").and_then(Value::as_str) {
                inputs.push(resolve::asset(&p, key)?);
            }
            path.to_string()
        } else if let Some(key) = v.get("assetId").and_then(Value::as_str) {
            let id = resolve::asset(&p, key)?;
            let asset = p.asset(id).ok_or("media not found")?;
            if asset.kind != MediaKind::Image {
                return Err(format!("images[{i}]: \"{}\" isn't an image; give a clipId and time to use one of its frames.", asset.name));
            }
            inputs.push(id);
            asset.path.clone()
        } else if let Some(key) = v.get("clipId").and_then(Value::as_str) {
            let id = resolve::clip(&p, key)?;
            inputs.extend(p.clip(id).and_then(|c| c.asset_id()));
            clip_frame(s, id, v.get("time").and_then(Value::as_f64)).await?
        } else {
            return Err(format!("images[{i}] needs path, assetId or clipId"));
        };
        images.push(image(role, path));
    }
    let video = a.opt_bool("video").unwrap_or(false);
    let task = match a.opt_str("task") {
        Some(t) => task(t)?,
        None => match (video, images.is_empty()) {
            (false, true) => Task::TextToImage,
            (false, false) => Task::ImageToImage,
            (true, true) => Task::TextToVideo,
            (true, false) => Task::ImageToVideo,
        },
    };
    let mut sub = model_request(s, a, task, a.str("prompt")?).await?;
    let r = &mut sub.request;
    r.images = images;
    r.negative_prompt = a.opt_str("negativePrompt").map(str::to_string).filter(|n| !n.is_empty());
    if let Some(v) = a.opt_str("aspectRatio") {
        r.aspect_ratio = Some(v.to_string());
    }
    r.resolution = a.opt_str("resolution").map(str::to_string);
    r.count = a.opt_u32("count").unwrap_or(1).max(1);
    r.audio = a.opt_bool("audio");
    if let Some(o) = a.object("params") {
        r.params = o.clone();
    }
    sub.input_assets = inputs;
    Ok(sub)
}

fn placement_from(s: &Session, a: &Args, r: &GenRequest) -> CmdResult<Placement> {
    match a.opt_str("place").unwrap_or("timeline") {
        "library" | "media" => Ok(Placement::Library),
        "timeline" => {
            let p = s.project()?;
            let track_id = a.opt_str("trackId").map(|k| resolve::track(&p, k)).transpose()?;
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
            let fallback = if r.task.output() != OutputKind::Image { r.duration.unwrap_or(5.0) } else { kimchi_core::DEFAULT_STILL_DURATION };
            Ok(Placement::Timeline { track_id, start, duration: a.opt_f64("length").unwrap_or(fallback) })
        }
        other => Err(format!("place is \"timeline\" or \"library\", not \"{other}\"")),
    }
}

/// A request for `task` on the chosen (or default) model, sized for the project.
async fn model_request(s: &Arc<Session>, a: &Args, task: Task, prompt: &str) -> CmdResult<Submit> {
    let (provider, model) = pick_model(s, a.opt_str("provider"), a.opt_str("model"), task).await?;
    let p = s.project()?;
    let mut r = GenRequest::new(model.id.clone(), task, prompt);
    r.aspect_ratio = Some(p.settings.aspect_ratio());
    r.width = Some(p.settings.width);
    r.height = Some(p.settings.height);
    r.seed = a.opt_i64("seed");
    if task.output() != OutputKind::Image {
        r.duration = a.opt_f64("duration").or_else(|| model.durations.first().copied());
    }
    Ok(Submit { provider, request: r, placement: Placement::Library, input_assets: vec![] })
}

/// Resolves the model a command should use: the one named, else the default
/// in settings, else the first featured model of a ready provider.
pub async fn pick_model(s: &Arc<Session>, provider: Option<&str>, model: Option<&str>, task: Task) -> CmdResult<(String, ModelInfo)> {
    let (provider, model) = match (provider, model) {
        (_, Some(m)) if m.contains("::") => {
            let (p, m) = m.split_once("::").expect("checked");
            (Some(p.to_string()), Some(m.to_string()))
        }
        (p, m) => (p.map(str::to_string), m.map(str::to_string)),
    };
    let (provider, model) = match (provider, model) {
        (p, None) => {
            let settings = s.settings().generate;
            let default = match task {
                Task::TextToSpeech => settings.speech_model,
                Task::TextToAudio => settings.audio_model,
                _ if task.output() == OutputKind::Video => settings.video_model,
                _ => settings.image_model,
            };
            match default.split_once("::") {
                Some((dp, dm)) if p.as_deref().is_none_or(|p| p == dp) => (Some(dp.to_string()), Some(dm.to_string())),
                _ => (p, None),
            }
        }
        other => other,
    };
    let candidates: Vec<ModelInfo> = match &provider {
        Some(p) => s.harness.models(p, false).await.map_err(err)?,
        None => s.harness.all_models().await,
    };
    let fits = |m: &&ModelInfo| m.supports(task);
    let found = match &model {
        Some(id) => candidates.iter().find(|m| &m.id == id || m.name.eq_ignore_ascii_case(id)).cloned().ok_or_else(|| {
            let list: Vec<&str> = candidates.iter().filter(fits).take(8).map(|m| m.id.as_str()).collect();
            format!("Unknown model `{id}`. Models for {}: {}.", task.as_str(), if list.is_empty() { "none".into() } else { list.join(", ") })
        })?,
        // The first featured model (lists come featured-first), else the first that fits.
        None => candidates
            .iter()
            .filter(fits)
            .find(|m| m.featured)
            .or_else(|| candidates.iter().find(fits))
            .cloned()
            .ok_or_else(|| format!("No ready provider has a model for {}. Add a key in Settings › Models & keys (or generate.setKey).", task.as_str()))?,
    };
    if !found.supports(task) {
        return Err(format!("{} can't do {}; it does {}.", found.name, task.as_str(), found.tasks.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")));
    }
    Ok((found.provider.clone(), found))
}

/// Starts a job and, for timeline placement, puts its placeholder clip down. The placement is
/// tried first, and a job whose placeholder can't go down is cancelled: nothing runs (and
/// bills) without a place to land.
pub fn submit(s: &Arc<Session>, sub: Submit) -> CmdResult<Job> {
    let project_id = s.current_id().ok_or("Open a project first.")?;
    let out_dir = s.library.generated_dir(project_id);
    let tag = serde_json::to_value(JobTag { project_id, input_assets: sub.input_assets.clone() }).map_err(err)?;
    let placeholder = |job_id: String, model_name: String| match sub.placement {
        Placement::Library => None,
        Placement::Timeline { track_id, start, duration } => {
            let kind = match sub.request.task.output() {
                OutputKind::Image => MediaKind::Image,
                OutputKind::Video => MediaKind::Video,
                OutputKind::Audio => MediaKind::Audio,
            };
            let content = ClipContent::Pending { job_id, kind, prompt: sub.request.prompt.clone(), model_name };
            Some(Edit::AddClip { track_id, clip: Clip::new(short_label(&sub.request.prompt), start, duration.max(0.5), content) })
        }
    };
    if let Some(edit) = placeholder(String::new(), String::new()) {
        let mut p = s.project()?;
        p.apply(&edit).map_err(|e| format!("The result can't go there: {e}"))?;
    }
    let job = s.harness.submit(&sub.provider, sub.request.clone(), out_dir, tag).map_err(err)?;
    if let Some(edit) = placeholder(job.id.clone(), job.model_name.clone()) {
        let placed = s.with_project(project_id, |ed| ed.apply(&edit, None)).and_then(|r| r.map_err(err));
        if let Err(e) = placed {
            s.harness.cancel(&job.id);
            return Err(format!("The generation was cancelled: its placeholder couldn't go on the timeline ({e})."));
        }
    }
    Ok(job)
}

pub fn short_label(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(6).collect();
    let s = words.join(" ");
    if s.is_empty() { "Generated".into() } else { s }
}

/// Forwards job updates to every client and lands finished results in their project.
pub fn spawn_job_listener(s: &Arc<Session>) {
    let mut rx = s.harness.subscribe();
    let weak = Arc::downgrade(s);
    s.runtime().spawn(async move {
        loop {
            let job = match rx.recv().await {
                Ok(j) => j,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let Some(s) = weak.upgrade() else { break };
            s.emit(Event::Job { job: Box::new(job.clone()) });
            if job.status.is_done() {
                let s2 = s.clone();
                s.runtime().spawn(async move {
                    if let Err(e) = land(&s2, &job).await {
                        tracing::warn!("couldn't land job {}: {e}", job.id);
                        s2.toast(ToastKind::Error, format!("Generation finished but couldn't be added: {e}"));
                        // Never leave a placeholder (and a `generate.wait`) waiting for it.
                        if let Ok(tag) = serde_json::from_value::<JobTag>(job.tag.clone()) {
                            let _ = s2.with_project(tag.project_id, |ed| ed.apply(&Edit::DropPending { job_id: job.id.clone() }, None));
                        }
                    }
                    // Wake anyone in `wait` now that the placeholder is gone.
                    s2.emit(Event::Job { job: Box::new(job) });
                });
            }
        }
    });
}

async fn land(s: &Arc<Session>, job: &Job) -> CmdResult<()> {
    let Ok(tag) = serde_json::from_value::<JobTag>(job.tag.clone()) else { return Ok(()) };
    if job.status != JobStatus::Succeeded {
        s.with_project(tag.project_id, |ed| ed.apply(&Edit::DropPending { job_id: job.id.clone() }, None))?.map_err(err)?;
        return Ok(());
    }
    let tools = s.tools()?;
    let mut first: Option<Id> = None;
    let mut failures = vec![];
    for (i, out) in job.outputs.iter().enumerate() {
        let path = PathBuf::from(&out.path);
        // One unreadable output doesn't lose the others.
        let probe = match kimchi_media::probe(&tools, &path).await {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let suffix = if job.outputs.len() > 1 { format!(" ({})", i + 1) } else { String::new() };
        let asset = Asset {
            id: new_id(),
            name: format!("{}{suffix}", short_label(&job.request.prompt)),
            kind: probe.kind,
            path: path_str(&path),
            meta: probe.meta,
            origin: AssetOrigin::Generated(Generation {
                job_id: job.id.clone(),
                provider: job.provider.clone(),
                model: job.model.clone(),
                model_name: job.model_name.clone(),
                task: job.request.task.as_str().to_string(),
                prompt: job.request.prompt.clone(),
                negative_prompt: job.request.negative_prompt.clone(),
                seed: job.seed,
                params: serde_json::to_value(&job.request).unwrap_or_default(),
                inputs: tag.input_assets.clone(),
                elapsed_ms: job.elapsed_ms,
                cost_usd: job.cost_usd,
            }),
            created_at: chrono::Utc::now(),
            thumbnail: None,
            filmstrip: None,
            waveform: None,
            proxy: None,
            beats: None,
        };
        first.get_or_insert(asset.id);
        add_asset(s, tag.project_id, asset)?;
    }
    match first {
        Some(asset_id) => {
            s.with_project(tag.project_id, |ed| ed.apply(&Edit::ResolvePending { job_id: job.id.clone(), asset_id }, None))?.map_err(err)?;
            if !failures.is_empty() {
                s.toast(ToastKind::Error, format!("Some results couldn't be read:\n{}", failures.join("\n")));
            }
            Ok(())
        }
        None if failures.is_empty() => Err("the model returned nothing".into()),
        None => Err(failures.join("\n")),
    }
}
