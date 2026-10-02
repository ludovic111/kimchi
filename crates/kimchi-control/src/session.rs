//! Everything kimchi keeps in memory: the open project and its one undo
//! history, the generation harness, running exports, settings, and the event
//! stream every client listens to.
//!
//! There is one [`Session`] per process. In the desktop app the window, the
//! built-in agent and every bridge client (CLI, MCP) share it, so they share
//! the undo history too. `kimchi-cli --file` and `kimchi-mcp --file` make their
//! own headless session around a project file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use futures::channel::{mpsc, oneshot};
use kimchi_core::{Edit, EditOutcome, Editor, Id, Library, Project};
use kimchi_gen::{Harness, Job, MemorySecrets, ProviderSettings, SecretStore};
use kimchi_media::Tools;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::settings::Settings;

pub type CmdResult<T = Value> = Result<T, String>;

pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Who is calling a command. Agent and MCP calls are checked against
/// `settings.agent.permissions`; the window and plain CLI calls are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Window,
    Agent,
    Cli,
    Mcp,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Window => "window",
            Source::Agent => "agent",
            Source::Cli => "cli",
            Source::Mcp => "mcp",
        }
    }

    /// Whether agent permissions apply.
    pub fn is_agent(self) -> bool {
        matches!(self, Source::Agent | Source::Mcp)
    }
}

/// Where the open project lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// In the library (`<data>/projects/<id>/project.json`).
    Library,
    /// A project file opened with `--file` or `project.open path=…`.
    File(PathBuf),
}

pub struct OpenDoc {
    pub editor: Editor,
    pub location: Location,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

/// One command any client ran, as shown on the agent panel's cards and in
/// its list of changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRecord {
    pub seq: u64,
    pub source: Source,
    pub command: String,
    pub params: Value,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Whether the command can change the project, files or the app.
    pub mutates: bool,
    pub at: DateTime<Utc>,
}

/// Progress of one export.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportStatus {
    pub id: String,
    pub path: String,
    pub progress: f64,
    pub done: bool,
    #[serde(default)]
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// What the window shows. The window pushes it with [`Session::set_ui_state`];
/// `ui.state` returns it and commands use the playhead and selection as defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    /// `home` or `editor`.
    pub screen: String,
    pub playhead: f64,
    pub playing: bool,
    pub selection: Vec<Id>,
    pub selected_asset: Option<Id>,
    /// Timeline zoom in pixels per second.
    pub zoom: f64,
    /// `media`, `generate` or `text`.
    pub left_tab: String,
    /// Open side panels (`agent`, `jobs`…) and dialogs (`settings`, `export`, `palette`).
    pub open: Vec<String>,
    /// `dark` or `light`.
    pub theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The open project changed (any client, or background work landing).
    ProjectChanged { project_id: Id },
    /// Another project was opened, or the project was closed (`None`).
    ProjectSwitched { project_id: Option<Id> },
    Job { job: Job },
    Export { export: ExportStatus },
    Toast { kind: ToastKind, text: String },
    Command { record: CommandRecord },
    SettingsChanged,
    Update { status: crate::update::UpdateStatus },
}

/// A command only the window can carry out (`ui.*`, playback, selection).
pub struct UiCall {
    pub command: String,
    pub params: Value,
    pub reply: oneshot::Sender<CmdResult>,
}

struct ExportEntry {
    status: ExportStatus,
    cancel: CancellationToken,
}

pub struct SessionOptions {
    /// Library and caches. Defaults to `<OS data dir>/kimchi`.
    pub data_dir: Option<PathBuf>,
    /// Settings files. Defaults to `<OS config dir>/kimchi`.
    pub config_dir: Option<PathBuf>,
    /// API keys. The app uses the OS keychain; tests use memory.
    pub secrets: Option<Arc<dyn SecretStore>>,
    /// No window will attach (CLI or MCP working on a file).
    pub headless: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self { data_dir: None, config_dir: None, secrets: None, headless: true }
    }
}

pub struct Session {
    pub library: Library,
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub harness: Arc<Harness>,
    pub headless: bool,
    doc: Mutex<Option<OpenDoc>>,
    tools: RwLock<Option<Tools>>,
    exports: Mutex<Vec<ExportEntry>>,
    settings: RwLock<Settings>,
    events: broadcast::Sender<Event>,
    ui: Mutex<Option<mpsc::UnboundedSender<UiCall>>>,
    ui_state: RwLock<UiState>,
    seq: AtomicU64,
    runtime: tokio::runtime::Handle,
    pub(crate) update: Mutex<crate::update::UpdateState>,
    pub(crate) bridge_port: Mutex<Option<u16>>,
}

impl Session {
    /// Must be called inside a Tokio runtime: background work (imports,
    /// previews, generations, exports) runs on it.
    pub fn new(opts: SessionOptions) -> std::io::Result<Arc<Self>> {
        let data_dir = opts.data_dir.unwrap_or_else(default_data_dir);
        let config_dir = opts.config_dir.unwrap_or_else(default_config_dir);
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(&config_dir)?;

        let secrets = opts.secrets.unwrap_or_else(|| Arc::new(MemorySecrets::default()));
        let harness = Arc::new(Harness::new(secrets));
        if let Ok(bytes) = std::fs::read(config_dir.join("providers.json"))
            && let Ok(s) = serde_json::from_slice::<HashMap<String, ProviderSettings>>(&bytes)
        {
            harness.load_settings(s);
        }
        let settings = Settings::load(&config_dir);
        let (events, _) = broadcast::channel(512);

        let session = Arc::new(Self {
            library: Library::new(&data_dir),
            data_dir,
            config_dir,
            harness,
            headless: opts.headless,
            doc: Mutex::new(None),
            tools: RwLock::new(Tools::locate().ok()),
            exports: Mutex::new(vec![]),
            settings: RwLock::new(settings),
            events,
            ui: Mutex::new(None),
            ui_state: RwLock::new(UiState::default()),
            seq: AtomicU64::new(1),
            runtime: tokio::runtime::Handle::current(),
            update: Mutex::new(Default::default()),
            bridge_port: Mutex::new(None),
        });
        crate::commands::generate::spawn_job_listener(&session);
        Ok(session)
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    // ---- events ---------------------------------------------------------

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    pub fn toast(&self, kind: ToastKind, text: impl Into<String>) {
        self.emit(Event::Toast { kind, text: text.into() });
    }

    pub(crate) fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    // ---- settings -------------------------------------------------------

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> CmdResult<Settings> {
        let s = {
            let mut s = self.settings.write();
            f(&mut s);
            s.clone()
        };
        s.save(&self.config_dir).map_err(err)?;
        self.emit(Event::SettingsChanged);
        Ok(s)
    }

    pub fn save_provider_settings(&self) -> CmdResult<()> {
        let json = serde_json::to_vec_pretty(&self.harness.settings()).map_err(err)?;
        std::fs::write(self.config_dir.join("providers.json"), json).map_err(err)
    }

    // ---- tools ----------------------------------------------------------

    pub fn tools(&self) -> CmdResult<Tools> {
        if let Some(t) = self.tools.read().clone() {
            return Ok(t);
        }
        let t = Tools::locate().map_err(err)?;
        *self.tools.write() = Some(t.clone());
        Ok(t)
    }

    pub fn tools_if_found(&self) -> Option<Tools> {
        self.tools().ok()
    }

    // ---- the window -----------------------------------------------------

    /// Called by the window once: commands that need it arrive on the returned channel.
    pub fn attach_ui(&self) -> mpsc::UnboundedReceiver<UiCall> {
        let (tx, rx) = mpsc::unbounded();
        *self.ui.lock() = Some(tx);
        rx
    }

    pub fn has_ui(&self) -> bool {
        self.ui.lock().as_ref().is_some_and(|tx| !tx.is_closed())
    }

    /// Hands a command to the window and waits for its answer.
    pub async fn ui_call(&self, command: &str, params: Value) -> CmdResult {
        let tx = self.ui.lock().clone().filter(|tx| !tx.is_closed()).ok_or_else(|| {
            format!("`{command}` needs the kimchi window. Start the app and use the CLI without --file, or kimchi-mcp --live.")
        })?;
        let (reply, rx) = oneshot::channel();
        tx.unbounded_send(UiCall { command: command.into(), params, reply }).map_err(|_| "the kimchi window has closed".to_string())?;
        rx.await.map_err(|_| "the kimchi window didn't answer".to_string())?
    }

    pub fn set_ui_state(&self, state: UiState) {
        *self.ui_state.write() = state;
    }

    pub fn ui_state(&self) -> UiState {
        self.ui_state.read().clone()
    }

    // ---- the open project -----------------------------------------------

    pub fn is_open(&self) -> bool {
        self.doc.lock().is_some()
    }

    pub fn current_id(&self) -> Option<Id> {
        self.doc.lock().as_ref().map(|d| d.editor.project().id)
    }

    pub fn location(&self) -> Option<Location> {
        self.doc.lock().as_ref().map(|d| d.location.clone())
    }

    /// A copy of the open project.
    pub fn project(&self) -> CmdResult<Project> {
        self.read(|ed| ed.project().clone())
    }

    /// Reads the open project's editor.
    pub fn read<R>(&self, f: impl FnOnce(&Editor) -> R) -> CmdResult<R> {
        let guard = self.doc.lock();
        let doc = guard.as_ref().ok_or(NO_PROJECT)?;
        Ok(f(&doc.editor))
    }

    /// Opens `project` (replacing whatever was open) and announces it.
    pub fn open_doc(&self, project: Project, location: Location) {
        let id = project.id;
        *self.doc.lock() = Some(OpenDoc { editor: Editor::new(project), location });
        self.emit(Event::ProjectSwitched { project_id: Some(id) });
    }

    pub fn close_doc(&self) {
        let was_open = self.doc.lock().take().is_some();
        if was_open {
            self.emit(Event::ProjectSwitched { project_id: None });
        }
    }

    /// Runs `f` on the open project's editor, then saves and announces the change.
    /// `label` and `source` are recorded with any undo step `f` makes.
    pub fn edit<R>(&self, label: &str, source: Source, f: impl FnOnce(&mut Editor) -> CmdResult<R>) -> CmdResult<R> {
        let mut guard = self.doc.lock();
        let doc = guard.as_mut().ok_or(NO_PROJECT)?;
        doc.editor.set_step_info(label, source.as_str());
        let r = f(&mut doc.editor)?;
        self.persist(doc)?;
        let id = doc.editor.project().id;
        drop(guard);
        self.emit(Event::ProjectChanged { project_id: id });
        Ok(r)
    }

    /// Applies one edit to the open project (see [`edit`](Self::edit)).
    pub fn apply(&self, label: &str, source: Source, edit: &Edit, coalesce: Option<&str>) -> CmdResult<EditOutcome> {
        self.edit(label, source, |ed| ed.apply(edit, coalesce).map_err(err))
    }

    /// Runs `f` against the open project if it is `id`, otherwise against the
    /// copy in the library. Saves afterwards. Used by background work (imports,
    /// previews, generations) that may finish after the person switched projects.
    pub fn with_project<R>(&self, id: Id, f: impl FnOnce(&mut Editor) -> R) -> CmdResult<R> {
        let mut guard = self.doc.lock();
        if let Some(doc) = guard.as_mut().filter(|d| d.editor.project().id == id) {
            let r = f(&mut doc.editor);
            self.persist(doc)?;
            drop(guard);
            self.emit(Event::ProjectChanged { project_id: id });
            return Ok(r);
        }
        drop(guard);
        let mut ed = Editor::new(self.library.load(id).map_err(err)?);
        let r = f(&mut ed);
        self.library.save(ed.project()).map_err(err)?;
        Ok(r)
    }

    fn persist(&self, doc: &mut OpenDoc) -> CmdResult<()> {
        if !doc.editor.is_dirty() {
            return Ok(());
        }
        match &doc.location {
            Location::Library => self.library.save(doc.editor.project()).map_err(err)?,
            Location::File(path) => write_project_file(path, doc.editor.project())?,
        }
        doc.editor.mark_saved();
        Ok(())
    }

    /// Cache folder for the open project's previews and frames.
    pub fn cache_dir(&self, id: Id) -> PathBuf {
        self.library.cache_dir(id)
    }

    // ---- exports --------------------------------------------------------

    pub(crate) fn add_export(&self, status: ExportStatus, cancel: CancellationToken) {
        self.exports.lock().push(ExportEntry { status, cancel });
    }

    pub(crate) fn set_export(&self, status: ExportStatus) {
        if let Some(e) = self.exports.lock().iter_mut().find(|e| e.status.id == status.id) {
            e.status = status.clone();
        }
        self.emit(Event::Export { export: status });
    }

    pub fn exports(&self) -> Vec<ExportStatus> {
        self.exports.lock().iter().map(|e| e.status.clone()).collect()
    }

    pub fn cancel_export(&self, id: &str) -> bool {
        match self.exports.lock().iter().find(|e| e.status.id == id && !e.status.done) {
            Some(e) => {
                e.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn bridge_port(&self) -> Option<u16> {
        *self.bridge_port.lock()
    }
}

pub const NO_PROJECT: &str = "No project is open. Open one with project.open or create one with project.create.";

pub fn default_data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("KIMCHI_DATA_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("kimchi")
}

pub fn default_config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("KIMCHI_CONFIG_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("kimchi")
}

/// Reads a project file (the library's `project.json` format).
pub fn read_project_file(path: &Path) -> CmdResult<Project> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{} isn't a kimchi project: {e}", path.display()))
}

/// Writes a project file atomically (write, then rename).
pub fn write_project_file(path: &Path, project: &Project) -> CmdResult<()> {
    let json = serde_json::to_vec_pretty(project).map_err(err)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}
