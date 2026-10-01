//! Bridges the generation harness and the editor: jobs start from the UI,
//! placeholders appear on the timeline, and finished media replaces them.

use std::path::PathBuf;

use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, Edit, Generation, Id, MediaKind, new_id};
use kimchi_gen::{GenRequest, Job, JobStatus, ModelInfo, OutputKind, ProviderSettings, ProviderStatus};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::commands::media::{add_asset, path_str};
use crate::state::{AppState, CmdResult, err};

/// Where a generation's result should go.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum Placement {
    /// Only into the media library.
    Library,
    /// A placeholder clip on the timeline that becomes the result.
    Timeline { track_id: Option<Id>, start: f64, duration: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SubmitArgs {
    pub provider: String,
    pub request: GenRequest,
    pub placement: Placement,
    /// Library assets used as inputs, recorded for provenance.
    #[serde(default)]
    pub input_assets: Vec<Id>,
}

/// What the editor stores in [`Job::tag`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobTag {
    project_id: Id,
    input_assets: Vec<Id>,
}

#[tauri::command]
pub fn gen_providers(state: State<AppState>) -> Vec<ProviderStatus> {
    state.harness.statuses()
}

#[tauri::command]
pub fn gen_set_key(state: State<AppState>, provider: String, key: Option<String>) -> CmdResult<Vec<ProviderStatus>> {
    state.harness.set_key(&provider, key.as_deref()).map_err(err)?;
    Ok(state.harness.statuses())
}

#[tauri::command]
pub fn gen_set_settings(state: State<AppState>, provider: String, settings: ProviderSettings) -> CmdResult<Vec<ProviderStatus>> {
    state.harness.set_settings(&provider, settings);
    state.save_provider_settings()?;
    Ok(state.harness.statuses())
}

#[tauri::command]
pub async fn gen_check(state: State<'_, AppState>, provider: String) -> CmdResult<String> {
    state.harness.check(&provider).await.map_err(err)
}

/// Models of one provider, or of every ready provider.
#[tauri::command]
pub async fn gen_models(state: State<'_, AppState>, provider: Option<String>, refresh: bool) -> CmdResult<Vec<ModelInfo>> {
    match provider {
        Some(p) => state.harness.models(&p, refresh).await.map_err(err),
        None => Ok(state.harness.all_models().await),
    }
}

#[tauri::command]
pub fn gen_jobs(state: State<AppState>) -> Vec<Job> {
    state.harness.jobs()
}

#[tauri::command]
pub fn gen_cancel(state: State<AppState>, job_id: String) {
    state.harness.cancel(&job_id);
}

#[tauri::command]
pub fn gen_clear(state: State<AppState>) -> Vec<Job> {
    state.harness.clear_finished();
    state.harness.jobs()
}

#[tauri::command]
pub fn gen_submit(app: AppHandle, state: State<AppState>, args: SubmitArgs) -> CmdResult<Job> {
    let project_id = state.current_id().ok_or("Open a project first.")?;
    let out_dir = state.library.generated_dir(project_id);
    let tag = serde_json::to_value(JobTag { project_id, input_assets: args.input_assets.clone() }).map_err(err)?;
    let job = state.harness.submit(&args.provider, args.request.clone(), out_dir, tag).map_err(err)?;

    if let Placement::Timeline { track_id, start, duration } = args.placement {
        let kind = match args.request.task.output() {
            OutputKind::Image => MediaKind::Image,
            OutputKind::Video => MediaKind::Video,
            OutputKind::Audio => MediaKind::Audio,
        };
        let label = short_label(&args.request.prompt);
        let clip = Clip::new(
            label,
            start,
            duration.max(0.5),
            ClipContent::Pending { job_id: job.id.clone(), kind, prompt: args.request.prompt.clone(), model_name: job.model_name.clone() },
        );
        state.with_project(&app, project_id, |ed| ed.apply(&Edit::AddClip { track_id, clip }, None)).map_err(err)?.map_err(err)?;
    }
    Ok(job)
}

fn short_label(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(6).collect();
    let s = words.join(" ");
    if s.is_empty() { "Generated".into() } else { s }
}

/// Forwards job updates to the UI and lands finished results in their project.
pub fn spawn_job_listener(app: AppHandle) {
    let mut rx = app.state::<AppState>().harness.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let job = match rx.recv().await {
                Ok(j) => j,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let _ = app.emit("gen-job", &job);
            if job.status.is_done() {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = land(&app, &job).await {
                        tracing::warn!("couldn't land job {}: {e}", job.id);
                        let _ = app.emit("toast", format!("Generation finished but couldn't be added: {e}"));
                    }
                });
            }
        }
    });
}

async fn land(app: &AppHandle, job: &Job) -> CmdResult<()> {
    let Ok(tag) = serde_json::from_value::<JobTag>(job.tag.clone()) else { return Ok(()) };
    let state = app.state::<AppState>();
    if job.status != JobStatus::Succeeded {
        state.with_project(app, tag.project_id, |ed| ed.apply(&Edit::DropPending { job_id: job.id.clone() }, None))?.map_err(err)?;
        return Ok(());
    }

    let tools = state.tools()?;
    let mut first: Option<Id> = None;
    for (i, out) in job.outputs.iter().enumerate() {
        let path = PathBuf::from(&out.path);
        let probe = kimchi_media::probe(&tools, &path).await.map_err(err)?;
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
        };
        first.get_or_insert(asset.id);
        add_asset(app, tag.project_id, asset)?;
    }
    if let Some(asset_id) = first {
        state
            .with_project(app, tag.project_id, |ed| ed.apply(&Edit::ResolvePending { job_id: job.id.clone(), asset_id }, None))?
            .map_err(err)?;
    }
    Ok(())
}
