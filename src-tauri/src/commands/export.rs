use std::collections::HashMap;
use std::path::PathBuf;

use kimchi_core::Id;
use kimchi_media::export::{ExportSettings, Overlays};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::state::{AppState, CmdResult};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportEvent {
    pub id: String,
    pub progress: f64,
    pub done: bool,
    pub error: Option<String>,
    pub path: String,
}

/// Starts rendering the open project. `overlays` maps text clip ids to PNGs
/// the UI rasterised with `write_png`. Progress arrives as `export-progress`.
#[tauri::command]
pub fn export_start(app: AppHandle, state: State<AppState>, settings: ExportSettings, overlays: HashMap<Id, String>) -> CmdResult<String> {
    let project = state.view()?.project;
    let tools = state.tools()?;
    let id = uuid::Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();
    state.exports.lock().insert(id.clone(), cancel.clone());
    let overlays: Overlays = overlays.into_iter().map(|(k, v)| (k, PathBuf::from(v))).collect();

    let job = id.clone();
    tauri::async_runtime::spawn(async move {
        let emit = |progress: f64, done: bool, error: Option<String>| {
            let _ = app.emit("export-progress", ExportEvent { id: job.clone(), progress, done, error, path: settings.path.clone() });
        };
        let progress_app = app.clone();
        let progress_job = job.clone();
        let path = settings.path.clone();
        let result = kimchi_media::export::export(
            &tools,
            &project,
            &overlays,
            &settings,
            move |p| {
                let _ = progress_app.emit("export-progress", ExportEvent { id: progress_job.clone(), progress: p, done: false, error: None, path: path.clone() });
            },
            cancel,
        )
        .await;
        match result {
            Ok(()) => emit(1.0, true, None),
            Err(e) => emit(0.0, true, Some(e.to_string())),
        }
        app.state::<AppState>().exports.lock().remove(&job);
    });
    Ok(id)
}

#[tauri::command]
pub fn export_cancel(state: State<AppState>, id: String) {
    if let Some(c) = state.exports.lock().get(&id) {
        c.cancel();
    }
}
