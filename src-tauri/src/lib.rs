//! kimchi desktop app: Tauri shell around the Rust editing core.

mod commands;
mod secrets;
mod state;

use serde::Serialize;
use tauri::{Manager, State};
use ts_rs::TS;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub ffmpeg: Option<String>,
    pub library: String,
}

#[tauri::command]
fn app_info(state: State<AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        ffmpeg: state.tools().ok().map(|t| t.ffmpeg.to_string_lossy().into_owned()),
        library: state.library.root().to_string_lossy().into_owned(),
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,kimchi=debug".into()))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = AppState::new()?;
            // The asset protocol must be able to serve the library (generated media, caches).
            let _ = app.asset_protocol_scope().allow_directory(state.library.root(), true);
            app.manage(state);
            commands::generate::spawn_job_listener(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            commands::project::list_projects,
            commands::project::create_project,
            commands::project::open_project,
            commands::project::close_project,
            commands::project::delete_project,
            commands::project::duplicate_project,
            commands::project::current_project,
            commands::project::apply_edit,
            commands::project::apply_edits,
            commands::project::undo,
            commands::project::redo,
            commands::media::import_media,
            commands::media::clip_frame,
            commands::media::write_png,
            commands::media::read_peaks,
            commands::generate::gen_providers,
            commands::generate::gen_set_key,
            commands::generate::gen_set_settings,
            commands::generate::gen_check,
            commands::generate::gen_models,
            commands::generate::gen_jobs,
            commands::generate::gen_cancel,
            commands::generate::gen_clear,
            commands::generate::gen_submit,
            commands::export::export_start,
            commands::export::export_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running kimchi");
}
