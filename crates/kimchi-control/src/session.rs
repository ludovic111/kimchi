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

tokio::task_local! {
    /// Set while `project.batch` runs its commands: their edits belong to its open batch.
    static IN_BATCH: ();
    /// A window edit belongs to the document visible when the person started it.
    static PROJECT_SCOPE: (usize, Option<Id>);
}

/// Whether this task is running a `project.batch`'s commands.
pub(crate) fn in_batch_scope() -> bool {
    IN_BATCH.try_with(|_| ()).is_ok()
}

/// Runs `f` as part of the open `project.batch` (see [`in_batch_scope`]).
pub(crate) async fn batch_scope<F: std::future::Future>(f: F) -> F::Output {
    IN_BATCH.scope((), f).await
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
    /// Clips the command created (from its result), so the window can mark them.
    #[serde(default)]
    pub created: Vec<Id>,
    /// The result, when it is small (a few KB); `None` for errors and large answers.
    #[serde(default)]
    pub result: Option<Value>,
    /// For agents and MCP clients: the checkpoint taken before their first change in this
    /// connection, so a whole session from a terminal can be reverted (`history.revertTo`).
    #[serde(default)]
    pub checkpoint: Option<u64>,
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
    /// The ffmpeg video encoder (`h264_videotoolbox`, `libx264`…); the CPU's after a fallback.
    #[serde(default)]
    pub encoder: Option<String>,
}

/// What the window shows. The window pushes it with [`Session::set_ui_state`];
/// `ui.state` returns it and commands use the playhead and selection as defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    /// `home`, `editor` or `studio`.
    pub screen: String,
    pub playhead: f64,
    pub playing: bool,
    pub selection: Vec<Id>,
    pub selected_asset: Option<Id>,
    /// Timeline zoom in pixels per second.
    pub zoom: f64,
    /// `media`, `generate`, `text`, `motion` or `captions`.
    pub left_tab: String,
    /// Open side panels (`agent`, `jobs`…) and dialogs (`settings`, `export`, `palette`).
    pub open: Vec<String>,
    /// `dark` or `light`.
    pub theme: String,
    /// The Studio, while a motion clip is open in it (what `ui.studio` answers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub studio: Option<serde_json::Value>,
    /// Playback starts over at the end.
    #[serde(rename = "loop")]
    pub looping: bool,
    /// Shuttle speed (J/L): 0 when not shuttling, else -8 to 8 (negative plays backwards).
    pub shuttle: f64,
    /// Dragged clips and the playhead stick to cuts, markers and the playhead.
    pub snapping: bool,
    /// Deleting in the window closes the gap.
    pub ripple: bool,
    /// Panel sizes in the editor, in pixels.
    pub layout: UiLayout,
    /// The window's audio views: the mixer (shown, its layout) and the open effect panel
    /// (what `audio.showMixer` answers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<serde_json::Value>,
}

/// The editor's panel sizes as drawn (`ui.setLayout`), and how the window's size placed them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiLayout {
    /// The left panel (media, generate, text, motion, captions).
    pub left: f32,
    /// The inspector, on the right.
    pub inspector: f32,
    pub timeline: f32,
    /// The Agent panel, when open.
    pub agent: f32,
    /// The left panel is open (false: only its rail of tabs shows).
    #[serde(default)]
    pub left_open: bool,
    /// The inspector is open.
    #[serde(default)]
    pub inspector_open: bool,
    /// Panels drawn over the work because the window is too narrow to dock them beside it
    /// (`left`, `inspector`, `agent`).
    #[serde(default)]
    pub overlays: Vec<String>,
    /// The window's size in pixels, `[width, height]`.
    #[serde(default)]
    pub window: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The open project changed (any client, or background work landing).
    ProjectChanged { project_id: Id },
    /// Another project was opened, or the project was closed (`None`).
    ProjectSwitched { project_id: Option<Id> },
    Job { job: Box<Job> },
    Export { export: ExportStatus },
    /// A motion clip being rendered ahead (`motion.render`).
    Render { render: crate::renders::RenderStatus },
    Toast { kind: ToastKind, text: String },
    Command { record: CommandRecord },
    SettingsChanged,
    Update { status: crate::update::UpdateStatus },
}

/// The built-in agent, installed by the app (`kimchi-agent` depends on this crate, so the
/// `agent.*` commands reach it through this trait). Ids and names are already validated.
pub trait AgentHost: Send + Sync {
    /// Carries out one `agent.*` command for `source`.
    fn call(self: Arc<Self>, session: Arc<Session>, source: Source, command: &'static str, args: crate::registry::Args) -> futures::future::BoxFuture<'static, CmdResult>;
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
    secrets: Arc<dyn SecretStore>,
    doc: Mutex<Option<OpenDoc>>,
    tools: RwLock<Option<Tools>>,
    exports: Mutex<Vec<ExportEntry>>,
    pub(crate) renders: Mutex<Vec<crate::renders::RenderEntry>>,
    settings: RwLock<Settings>,
    events: broadcast::Sender<Event>,
    ui: Mutex<Option<mpsc::UnboundedSender<UiCall>>>,
    ui_state: RwLock<UiState>,
    agent: RwLock<Option<Arc<dyn AgentHost>>>,
    seq: AtomicU64,
    runtime: tokio::runtime::Handle,
    pub(crate) update: Mutex<crate::update::UpdateState>,
    pub(crate) bridge_port: Mutex<Option<u16>>,
    /// Held by a running `project.batch`; other changes wait for it (briefly) before starting.
    pub(crate) batch_lock: Arc<tokio::sync::Mutex<()>>,
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
        let harness = Arc::new(Harness::new(secrets.clone()));
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
            secrets,
            doc: Mutex::new(None),
            tools: RwLock::new(Tools::locate().ok()),
            exports: Mutex::new(vec![]),
            renders: Mutex::new(vec![]),
            settings: RwLock::new(settings),
            events,
            ui: Mutex::new(None),
            ui_state: RwLock::new(UiState::default()),
            agent: RwLock::new(None),
            seq: AtomicU64::new(1),
            runtime: tokio::runtime::Handle::current(),
            update: Mutex::new(Default::default()),
            bridge_port: Mutex::new(None),
            batch_lock: Arc::new(tokio::sync::Mutex::new(())),
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
        s.save(&self.config_dir).map_err(|e| format!("Couldn't save settings in {}: {e}", self.config_dir.display()))?;
        crate::diagnostics::set_level(&s.diagnostics.log_level);
        self.emit(Event::SettingsChanged);
        Ok(s)
    }

    /// A stored API key (keychain in the app, memory in tests) by id: `fal`, `anthropic`, `openai`…
    pub fn secret(&self, id: &str) -> Option<String> {
        self.secrets.get(id).filter(|k| !k.trim().is_empty())
    }

    /// Stores (`Some`) or removes (`None`) an API key. The person's own action, never an agent's.
    pub fn set_secret(&self, id: &str, key: Option<&str>) -> CmdResult<()> {
        match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => self.secrets.set(id, k),
            None => self.secrets.delete(id),
        }
    }

    pub fn save_provider_settings(&self) -> CmdResult<()> {
        let json = serde_json::to_vec_pretty(&self.harness.settings()).map_err(err)?;
        let tmp = self.config_dir.join("providers.json.tmp");
        std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, self.config_dir.join("providers.json"))).map_err(|e| format!("Couldn't save provider settings: {e}"))
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

    /// Changes part of what the window shows (the playhead while playing, for example).
    pub fn update_ui_state(&self, f: impl FnOnce(&mut UiState)) {
        f(&mut self.ui_state.write());
    }

    pub fn ui_state(&self) -> UiState {
        self.ui_state.read().clone()
    }

    // ---- the built-in agent ---------------------------------------------

    /// Called by the app once: `agent.*` commands go to `host`.
    pub fn set_agent_host(&self, host: Arc<dyn AgentHost>) {
        *self.agent.write() = Some(host);
    }

    pub fn agent_host(&self) -> Option<Arc<dyn AgentHost>> {
        self.agent.read().clone()
    }

    // ---- the open project -----------------------------------------------

    pub fn is_open(&self) -> bool {
        self.doc.lock().is_some()
    }

    pub fn current_id(&self) -> Option<Id> {
        self.doc.lock().as_ref().map(|d| d.editor.project().id)
    }

    /// Binds a command's reads and edits to the project its caller displayed. Checks happen
    /// under the document lock, including after any await between reading and applying an edit.
    /// Background work that intentionally targets a saved project uses `with_project` instead.
    pub async fn guard_project<F: std::future::Future>(&self, project: Option<Id>, command: F) -> F::Output {
        PROJECT_SCOPE.scope((self as *const Self as usize,project),command).await
    }

    fn check_project_scope(&self, project: Id) -> CmdResult<()> {
        let changed=PROJECT_SCOPE.try_with(|(session,expected)| *session==self as *const Self as usize && *expected!=Some(project)).unwrap_or(false);
        if changed {Err("The project changed before this edit finished. Start the edit again in the current project.".into())} else {Ok(())}
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
        self.check_project_scope(doc.editor.project().id)?;
        Ok(f(&doc.editor))
    }

    /// Opens `project` (replacing whatever was open) and announces it.
    pub fn open_doc(&self, project: Project, location: Location) {
        let id = project.id;
        *self.doc.lock() = Some(OpenDoc { editor: Editor::new(project), location });
        self.emit(Event::ProjectSwitched { project_id: Some(id) });
    }

    /// Opens a library project. Read under the lock background writes take (see
    /// [`with_project`](Self::with_project)), so none lands between reading and opening it.
    pub fn open_library_project(&self, id: Id) -> CmdResult<Project> {
        let mut guard = self.doc.lock();
        let project = self.library.load(id).map_err(|e| format!("Couldn't open the project: {e}"))?;
        *guard = Some(OpenDoc { editor: Editor::new(project.clone()), location: Location::Library });
        drop(guard);
        self.emit(Event::ProjectSwitched { project_id: Some(id) });
        Ok(project)
    }

    /// Remembers the open project (see `Editor::checkpoint`), without an undo step or an event.
    pub fn checkpoint(&self) -> Option<(Id, u64)> {
        let mut guard = self.doc.lock();
        let doc = guard.as_mut()?;
        Some((doc.editor.project().id, doc.editor.checkpoint()))
    }

    pub fn close_doc(&self) {
        let was_open = self.doc.lock().take().is_some();
        if was_open {
            self.emit(Event::ProjectSwitched { project_id: None });
        }
    }

    /// Runs `f` on the open project's editor, then saves and announces the change.
    /// `label` and `source` are recorded with any undo step `f` makes.
    /// Outside a running `project.batch`, changes are refused while one is open (they would
    /// become part of its step, and go with it if it is rolled back).
    pub fn edit<R>(&self, label: &str, source: Source, f: impl FnOnce(&mut Editor) -> CmdResult<R>) -> CmdResult<R> {
        let mut guard = self.doc.lock();
        let doc = guard.as_mut().ok_or(NO_PROJECT)?;
        self.check_project_scope(doc.editor.project().id)?;
        doc.editor.set_step_info(label, source.as_str());
        // Only a batch already open when this call came in is someone else's (a command's own
        // internal batch, opened inside `f`, is not).
        doc.editor.set_outsider(doc.editor.in_batch() && !in_batch_scope());
        let r = f(&mut doc.editor);
        doc.editor.set_outsider(false);
        let r = r?;
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
    ///
    /// A closed project is read, changed and written back under the open project's lock, so
    /// two background writes (a preview and a generation landing) can't lose one another, and
    /// opening it waits for them.
    pub fn with_project<R>(&self, id: Id, f: impl FnOnce(&mut Editor) -> R) -> CmdResult<R> {
        let mut guard = self.doc.lock();
        if let Some(doc) = guard.as_mut().filter(|d| d.editor.project().id == id) {
            // Only a batch already open when this call came in is someone else's (a command's own
            // internal batch, opened inside `f`, is not).
            doc.editor.set_outsider(doc.editor.in_batch() && !in_batch_scope());
            let r = f(&mut doc.editor);
            doc.editor.set_outsider(false);
            self.persist(doc)?;
            drop(guard);
            self.emit(Event::ProjectChanged { project_id: id });
            return Ok(r);
        }
        let mut ed = Editor::new(self.library.load(id).map_err(err)?);
        let r = f(&mut ed);
        if ed.is_dirty() {
            self.library.save(ed.project()).map_err(err)?;
        }
        drop(guard);
        Ok(r)
    }

    /// Ends (or rolls back) the open project's batch, if the open project is still `id` and the
    /// batch is still open. `project.batch` calls it when it ends or is dropped.
    pub(crate) fn close_batch(&self, id: Id, label: &str, source: Source, rollback: bool) -> CmdResult<()> {
        let mut guard = self.doc.lock();
        let Some(doc) = guard.as_mut().filter(|d| d.editor.project().id == id && d.editor.in_batch()) else { return Ok(()) };
        doc.editor.set_step_info(label, source.as_str());
        if rollback {
            doc.editor.rollback_batch();
        } else {
            doc.editor.end_batch();
        }
        self.persist(doc)?;
        drop(guard);
        self.emit(Event::ProjectChanged { project_id: id });
        Ok(())
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
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project.json".into());
    let tmp = path.with_file_name(kimchi_core::store::tmp_name(&name));
    std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't write {}: {e}", path.display())
    })
}
