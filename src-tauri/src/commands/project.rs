use kimchi_core::{Edit, EditOutcome, Editor, Id, Project, ProjectSettings, ProjectSummary};
use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

use crate::state::{AppState, CmdResult, ProjectView, err, view_of};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EditResponse {
    pub view: ProjectView,
    pub outcome: EditOutcome,
}

#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Vec<ProjectSummary> {
    state.library.list()
}

#[tauri::command]
pub fn create_project(state: State<AppState>, name: String, settings: Option<ProjectSettings>) -> CmdResult<ProjectView> {
    let name = if name.trim().is_empty() { "Untitled".to_string() } else { name.trim().to_string() };
    let project = Project::new(name, settings.unwrap_or_default());
    state.library.save(&project).map_err(err)?;
    let ed = Editor::new(project);
    let view = view_of(&ed);
    *state.editor.lock() = Some(ed);
    Ok(view)
}

#[tauri::command]
pub fn open_project(state: State<AppState>, id: Id) -> CmdResult<ProjectView> {
    let project = state.library.load(id).map_err(|e| format!("Couldn't open the project: {e}"))?;
    let ed = Editor::new(project);
    let view = view_of(&ed);
    *state.editor.lock() = Some(ed);
    Ok(view)
}

#[tauri::command]
pub fn close_project(state: State<AppState>) {
    *state.editor.lock() = None;
}

#[tauri::command]
pub fn delete_project(state: State<AppState>, id: Id) -> CmdResult<()> {
    if state.current_id() == Some(id) {
        *state.editor.lock() = None;
    }
    state.library.delete(id).map_err(err)
}

#[tauri::command]
pub fn duplicate_project(state: State<AppState>, id: Id) -> CmdResult<ProjectSummary> {
    let mut p = state.library.load(id).map_err(err)?;
    p.id = kimchi_core::new_id();
    p.name = format!("{} copy", p.name);
    p.created_at = chrono::Utc::now();
    p.updated_at = p.created_at;
    state.library.save(&p).map_err(err)?;
    Ok(kimchi_core::store::summarize(&p))
}

/// The one entry point for timeline edits from the UI.
#[tauri::command]
pub fn apply_edit(state: State<AppState>, edit: Edit, coalesce: Option<String>) -> CmdResult<EditResponse> {
    let mut guard = state.editor.lock();
    let ed = guard.as_mut().ok_or("no project is open")?;
    let outcome = ed.apply(&edit, coalesce.as_deref()).map_err(err)?;
    state.library.save(ed.project()).map_err(err)?;
    ed.mark_saved();
    Ok(EditResponse { view: view_of(ed), outcome })
}

/// Applies several edits as a single undo step.
#[tauri::command]
pub fn apply_edits(state: State<AppState>, edits: Vec<Edit>) -> CmdResult<EditResponse> {
    let mut guard = state.editor.lock();
    let ed = guard.as_mut().ok_or("no project is open")?;
    let mut outcome = EditOutcome::default();
    // A one-off coalesce key folds the whole batch into one history entry.
    let batch = uuid::Uuid::new_v4().to_string();
    for e in &edits {
        let o = ed.apply(e, Some(&batch)).map_err(err)?;
        outcome.created_clips.extend(o.created_clips);
        outcome.created_tracks.extend(o.created_tracks);
    }
    state.library.save(ed.project()).map_err(err)?;
    ed.mark_saved();
    Ok(EditResponse { view: view_of(ed), outcome })
}

#[tauri::command]
pub fn undo(state: State<AppState>) -> CmdResult<ProjectView> {
    let mut guard = state.editor.lock();
    let ed = guard.as_mut().ok_or("no project is open")?;
    ed.undo();
    state.library.save(ed.project()).map_err(err)?;
    Ok(view_of(ed))
}

#[tauri::command]
pub fn redo(state: State<AppState>) -> CmdResult<ProjectView> {
    let mut guard = state.editor.lock();
    let ed = guard.as_mut().ok_or("no project is open")?;
    ed.redo();
    state.library.save(ed.project()).map_err(err)?;
    Ok(view_of(ed))
}

#[tauri::command]
pub fn current_project(state: State<AppState>) -> Option<ProjectView> {
    state.view().ok()
}
