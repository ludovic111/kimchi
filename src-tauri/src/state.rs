//! Everything the app keeps in memory between commands.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use kimchi_core::{Editor, Id, Library, Project};
use kimchi_gen::{Harness, ProviderSettings};
use kimchi_media::Tools;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::secrets::KeychainSecrets;

pub struct AppState {
    pub library: Library,
    pub config_dir: PathBuf,
    pub editor: Mutex<Option<Editor>>,
    pub harness: Arc<Harness>,
    pub tools: RwLock<Option<Tools>>,
    pub exports: Mutex<HashMap<String, CancellationToken>>,
}

/// What the UI receives after every change.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectView {
    pub project: Project,
    pub can_undo: bool,
    pub can_redo: bool,
}

pub type CmdResult<T> = Result<T, String>;

pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl AppState {
    pub fn new() -> anyhow::Result<Self> {
        let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("kimchi");
        let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("kimchi");
        std::fs::create_dir_all(&data)?;
        std::fs::create_dir_all(&config_dir)?;

        let harness = Arc::new(Harness::new(Arc::new(KeychainSecrets::default())));
        let settings_path = config_dir.join("providers.json");
        if let Ok(bytes) = std::fs::read(&settings_path)
            && let Ok(s) = serde_json::from_slice::<HashMap<String, ProviderSettings>>(&bytes)
        {
            harness.load_settings(s);
        }

        Ok(Self {
            library: Library::new(data),
            config_dir,
            editor: Mutex::new(None),
            harness,
            tools: RwLock::new(Tools::locate().ok()),
            exports: Mutex::new(HashMap::new()),
        })
    }

    pub fn save_provider_settings(&self) -> CmdResult<()> {
        let json = serde_json::to_vec_pretty(&self.harness.settings()).map_err(err)?;
        std::fs::write(self.config_dir.join("providers.json"), json).map_err(err)
    }

    pub fn tools(&self) -> CmdResult<Tools> {
        if let Some(t) = self.tools.read().clone() {
            return Ok(t);
        }
        let t = Tools::locate().map_err(err)?;
        *self.tools.write() = Some(t.clone());
        Ok(t)
    }

    pub fn view(&self) -> CmdResult<ProjectView> {
        let guard = self.editor.lock();
        let ed = guard.as_ref().ok_or("no project is open")?;
        Ok(view_of(ed))
    }

    pub fn current_id(&self) -> Option<Id> {
        self.editor.lock().as_ref().map(|e| e.project().id)
    }

    /// Runs `f` against the open project if it is `id`, otherwise against the
    /// copy on disk. Saves afterwards. Used by background work (imports,
    /// generations) that may finish after the user switched projects.
    pub fn with_project<R>(&self, app: &AppHandle, id: Id, f: impl FnOnce(&mut Editor) -> R) -> CmdResult<R> {
        let mut guard = self.editor.lock();
        if let Some(ed) = guard.as_mut().filter(|e| e.project().id == id) {
            let r = f(ed);
            self.library.save(ed.project()).map_err(err)?;
            ed.mark_saved();
            let _ = app.emit("project-changed", view_of(ed));
            return Ok(r);
        }
        drop(guard);
        let mut ed = Editor::new(self.library.load(id).map_err(err)?);
        let r = f(&mut ed);
        self.library.save(ed.project()).map_err(err)?;
        Ok(r)
    }
}

pub fn view_of(ed: &Editor) -> ProjectView {
    ProjectView { project: ed.project().clone(), can_undo: ed.can_undo(), can_redo: ed.can_redo() }
}
