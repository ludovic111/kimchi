//! The window's view of the session.
//!
//! The window is one client of the command registry, never a privileged one:
//! every change goes through [`Store::run`] (`registry::call` with
//! `Source::Window`), and the project shown is whatever the session holds
//! after the change. The store only adds view state (selection, panels,
//! zoom) and mirrors what the session announces on its event stream.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Pixels, Point, SharedString, Task, Window};
use kimchi_control::{CmdResult, CommandRecord, Event, ExportStatus, Session, Settings, Source, ToastKind, UiState};
use kimchi_core::{Asset, Clip, Id, Project, ProjectSummary, Track};
use kimchi_gen::{Job, ModelInfo, ProviderStatus};
use serde_json::{Value, json};

use crate::playback::Playback;

pub const MIN_PPS: f64 = 4.0;
pub const MAX_PPS: f64 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftTab {
    Media,
    Generate,
    Text,
    Motion,
    Captions,
}

impl LeftTab {
    pub fn as_str(self) -> &'static str {
        match self {
            LeftTab::Media => "media",
            LeftTab::Generate => "generate",
            LeftTab::Text => "text",
            LeftTab::Motion => "motion",
            LeftTab::Captions => "captions",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    Settings { section: Option<String> },
    Export,
    Palette,
    Shortcuts,
    /// Release notes: since a version (after an update) or this one; `all` for every release.
    WhatsNew { since: Option<String>, all: bool },
}

impl Dialog {
    pub fn name(&self) -> &'static str {
        match self {
            Dialog::Settings { .. } => "settings",
            Dialog::Export => "export",
            Dialog::Palette => "palette",
            Dialog::Shortcuts => "shortcuts",
            Dialog::WhatsNew { .. } => "whatsNew",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: SharedString,
    /// A passing status ("Undid split"): the next one replaces it instead of stacking.
    pub flash: bool,
    /// A button on the toast: its label and the dialog it opens.
    pub action: Option<(SharedString, Dialog)>,
}

/// Clips copied or cut in the window, as they were, with the track each was on (a cut
/// can still be pasted after the clips are gone).
#[derive(Clone, Debug, Default)]
pub struct Clipboard {
    pub clips: Vec<(Id, Clip)>,
}

/// One entry of a context menu.
#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

/// What a menu item does when chosen.
pub type MenuAction = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct MenuItem {
    pub label: SharedString,
    pub icon: Option<&'static str>,
    /// A service's logo before the label (`ui::logo` id), e.g. the agent's providers.
    pub logo: Option<&'static str>,
    pub shortcut: Option<SharedString>,
    pub danger: bool,
    /// Uses the accent: an AI action.
    pub ai: bool,
    pub disabled: bool,
    pub action: MenuAction,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { label: label.into(), icon: None, logo: None, shortcut: None, danger: false, ai: false, disabled: false, action: Rc::new(action) }
    }
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn logo(mut self, id: &'static str) -> Self {
        self.logo = Some(id);
        self
    }
    pub fn shortcut(mut self, s: impl Into<SharedString>) -> Self {
        self.shortcut = Some(s.into());
        self
    }
    /// The action's shortcut, as this platform writes it.
    pub fn shortcut_of(mut self, action: &dyn gpui::Action) -> Self {
        self.shortcut = crate::actions::hint(action);
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn ai(mut self) -> Self {
        self.ai = true;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn entry(self) -> MenuEntry {
        MenuEntry::Item(self)
    }
}

#[derive(Clone)]
pub struct ContextMenu {
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry>,
}

/// What the composer in the generate panel should start with (set by the AI
/// actions on clips: animate, extend, bridge, restyle, regenerate).
#[derive(Clone, Debug, Default)]
pub struct ComposeRequest {
    pub video: bool,
    pub prompt: Option<String>,
    pub negative: Option<String>,
    /// `provider::model`.
    pub model: Option<String>,
    pub seed: Option<String>,
    pub duration: Option<f64>,
    pub aspect: Option<String>,
    /// Input images: (role, path, label, asset).
    pub refs: Vec<ComposeRef>,
    /// Where the result lands; `None` = library only.
    pub target: Option<ComposeTarget>,
}

#[derive(Clone, Debug)]
pub struct ComposeRef {
    pub role: &'static str,
    pub path: String,
    pub label: String,
    pub asset_id: Option<Id>,
}

#[derive(Clone, Debug)]
pub struct ComposeTarget {
    pub track_id: Option<Id>,
    pub start: f64,
    pub duration: f64,
    pub label: String,
}

#[derive(Clone, Debug)]
pub enum StoreEvent {
    /// The composer should take this request (and the generate tab is shown).
    Compose(Box<ComposeRequest>),
    /// Focus the prompt field.
    FocusPrompt,
    /// Put the cursor in the selected title's words (double-click on a title).
    EditText,
    /// Ask before removing this media file (Delete with media selected): its clips go too.
    AskRemoveAsset(kimchi_core::Id),
    /// Open this motion clip in the Studio.
    OpenStudio(kimchi_core::Id),
}

pub struct Store {
    pub session: Arc<Session>,
    pub playback: Entity<Playback>,
    /// The built-in agent's conversation and runs (the Agent panel draws it; `agent.*` drives it).
    pub agent: Arc<kimchi_agent::Host>,

    pub project: Option<Arc<Project>>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub library: Vec<ProjectSummary>,
    pub jobs: Vec<Job>,
    pub exports: Vec<ExportStatus>,
    /// Motion clips being rendered ahead (`motion.render`).
    pub renders: Vec<kimchi_control::renders::RenderStatus>,
    pub providers: Vec<ProviderStatus>,
    pub models: Vec<ModelInfo>,
    pub models_loading: bool,
    /// Commands from the agent, MCP and the CLI, newest last (the agent panel's cards).
    pub commands: Vec<CommandRecord>,
    pub settings: Settings,
    pub update: kimchi_control::update::UpdateStatus,

    pub selection: Vec<Id>,
    pub selected_asset: Option<Id>,
    /// Timeline zoom, pixels per second.
    pub pps: f64,
    pub snapping: bool,
    pub ripple: bool,
    pub left_tab: LeftTab,
    /// Counts the times a left tab was asked for (`set_left_tab`): the editor shows the left
    /// panel (or its drawer, in a narrow window) when it changes.
    pub left_reveal: u64,
    pub dialog: Option<Dialog>,
    pub agent_open: bool,
    pub jobs_open: bool,
    pub menu: Option<ContextMenu>,
    pub toasts: Vec<Toast>,
    pub dropping: bool,
    pub clipboard: Clipboard,
    /// What the Studio shows while it is open (its `ui.studio` state), for `ui.state`.
    pub studio: Option<Value>,
    /// The audio views: the mixer, the selected strip, the effect panel and browser, recording.
    pub audio: crate::views::mixer::AudioView,
    pub recorder: crate::views::mixer::recording::Recorder,
    next_toast: u64,
    _pump: Task<()>,
}

impl EventEmitter<StoreEvent> for Store {}

/// The store, reachable from anywhere (`cx.store()`).
pub struct GlobalStore(pub Entity<Store>);
impl Global for GlobalStore {}

pub trait StoreExt {
    fn store(&self) -> Entity<Store>;
}

impl StoreExt for App {
    fn store(&self) -> Entity<Store> {
        self.global::<GlobalStore>().0.clone()
    }
}

impl Store {
    pub fn new(session: Arc<Session>, playback: Entity<Playback>, agent: Arc<kimchi_agent::Host>, cx: &mut Context<Self>) -> Self {
        let mut rx = session.subscribe();
        let pump = cx.spawn(async move |this, cx| {
            loop {
                let event = match rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Missed some: resync everything.
                        if this.update(cx, |s, cx| s.refresh_all(cx)).is_err() {
                            break;
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                if this.update(cx, |s, cx| s.on_event(event, cx)).is_err() {
                    break;
                }
            }
        });
        let settings = session.settings();
        let mut store = Self {
            session: session.clone(),
            playback,
            agent,
            project: None,
            can_undo: false,
            can_redo: false,
            library: vec![],
            jobs: session.harness.jobs(),
            exports: session.exports(),
            renders: session.renders(),
            providers: vec![],
            models: vec![],
            models_loading: false,
            commands: vec![],
            settings,
            update: kimchi_control::update::status(&session),
            selection: vec![],
            selected_asset: None,
            pps: 60.0,
            snapping: true,
            ripple: false,
            left_tab: LeftTab::Media,
            left_reveal: 0,
            dialog: None,
            agent_open: false,
            jobs_open: false,
            menu: None,
            toasts: vec![],
            dropping: false,
            clipboard: Clipboard::default(),
            studio: None,
            audio: Default::default(),
            recorder: Default::default(),
            next_toast: 1,
            _pump: pump,
        };
        store.refresh_project();
        store.library = session.library.list();
        store.refresh_providers(cx);
        store
    }

    /// Provider statuses read keys (the keychain may ask the person), so never on the UI thread.
    pub fn refresh_providers(&mut self, cx: &mut Context<Self>) {
        let session = self.session.clone();
        let task = gpui_tokio::Tokio::spawn(cx, async move { session.harness.statuses() });
        cx.spawn(async move |this, cx| {
            if let Ok(list) = task.await {
                this.update(cx, |s, cx| {
                    s.providers = list;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // ---- running commands ------------------------------------------------

    /// Runs a registry command as the window. Errors become toasts.
    pub fn run(&mut self, name: &str, params: Value, cx: &mut Context<Self>) {
        self.run_then(name, params, cx, |_, _, _| {});
    }

    /// Runs a command and hands its result to `then` on the main thread
    /// (errors become toasts and `then` isn't called).
    pub fn run_then(&mut self, name: &str, params: Value, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static) {
        let task = self.call(name, params, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |s, cx| match result {
                Ok(v) => then(s, v, cx),
                Err(e) => s.error(e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Runs a command and returns its result.
    pub fn call(&self, name: &str, params: Value, cx: &mut Context<Self>) -> Task<CmdResult> {
        let session = self.session.clone();
        let name = name.to_string();
        let task = gpui_tokio::Tokio::spawn(cx, async move { kimchi_control::call(&session, Source::Window, &name, params).await });
        cx.background_spawn(async move { task.await.unwrap_or_else(|e| Err(format!("the command stopped: {e}"))) })
    }

    // ---- events -----------------------------------------------------------

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::ProjectChanged { project_id } => {
                if self.project.as_ref().is_some_and(|p| p.id == project_id) {
                    self.refresh_project();
                }
            }
            Event::ProjectSwitched { .. } => {
                self.selection.clear();
                self.selected_asset = None;
                // Copied clips name this project's tracks and media.
                self.clipboard = Clipboard::default();
                self.dialog = None;
                self.refresh_project();
                self.library = self.session.library.list();
                self.playback.update(cx, |p, cx| p.reset(cx));
                crate::views::mixer::recording::project_switched(self, cx);
                self.audio.forget_project();
                crate::views::mixer::refresh_songs(self, cx);
            }
            Event::Job { job } => match self.jobs.iter_mut().find(|j| j.id == job.id) {
                Some(j) => *j = *job,
                None => self.jobs.push(*job),
            },
            Event::Export { export } => match self.exports.iter_mut().find(|e| e.id == export.id) {
                Some(e) => *e = export,
                None => self.exports.push(export),
            },
            Event::Render { render } => match self.renders.iter_mut().find(|e| e.id == render.id) {
                Some(e) => *e = render,
                None => self.renders.push(render),
            },
            Event::Toast { kind, text } => self.toast(kind, text, cx),
            Event::Command { record } => {
                if record.source != Source::Window {
                    self.commands.push(record);
                    if self.commands.len() > 500 {
                        self.commands.remove(0);
                    }
                }
            }
            Event::SettingsChanged => {
                self.settings = self.session.settings();
                self.refresh_providers(cx);
                // After this update: it reads the store.
                cx.defer(crate::app::apply_theme_setting);
            }
            Event::Update { status } => self.update = status,
        }
        self.sync_ui(cx);
        cx.notify();
    }

    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_project();
        self.jobs = self.session.harness.jobs();
        self.exports = self.session.exports();
        self.renders = self.session.renders();
        self.settings = self.session.settings();
        self.refresh_providers(cx);
        self.library = self.session.library.list();
        cx.notify();
    }

    fn refresh_project(&mut self) {
        match self.session.read(|ed| (ed.project().clone(), ed.can_undo(), ed.can_redo())) {
            Ok((p, u, r)) => {
                // Drop selections that no longer exist (after undo, deletes…).
                self.selection.retain(|id| p.clip(*id).is_some());
                if self.selected_asset.is_some_and(|a| p.asset(a).is_none()) {
                    self.selected_asset = None;
                }
                self.project = Some(Arc::new(p));
                self.can_undo = u;
                self.can_redo = r;
            }
            Err(_) => {
                self.project = None;
                self.can_undo = false;
                self.can_redo = false;
            }
        }
    }

    /// Tells the session what the window shows (`ui.state`, playhead defaults).
    pub fn sync_ui(&self, cx: &App) {
        let pb = self.playback.read(cx);
        let mut open = vec![];
        if self.agent_open {
            open.push("agent".to_string());
        }
        if self.jobs_open {
            open.push("jobs".to_string());
        }
        if let Some(d) = &self.dialog {
            open.push(d.name().to_string());
        }
        self.session.set_ui_state(UiState {
            screen: match (&self.project, &self.studio) {
                (Some(_), Some(_)) => "studio".into(),
                (Some(_), None) => "editor".into(),
                (None, _) => "home".into(),
            },
            studio: self.studio.clone(),
            playhead: pb.playhead,
            playing: pb.playing,
            selection: self.selection.clone(),
            selected_asset: self.selected_asset,
            zoom: self.pps,
            left_tab: self.left_tab.as_str().into(),
            open,
            theme: if crate::theme::ActiveTheme::theme(cx).is_dark() { "dark".into() } else { "light".into() },
            looping: pb.looping,
            shuttle: pb.shuttle,
            snapping: self.snapping,
            ripple: self.ripple,
            // The editor keeps its panel sizes up to date itself.
            layout: self.session.ui_state().layout,
            audio: Some(self.audio.to_json(self.project.as_deref())),
        });
    }

    // ---- toasts ----------------------------------------------------------

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.push_toast(kind, text.into(), false, cx);
    }

    /// A short status that replaces the previous one (undo, redo, copy…).
    pub fn flash(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toasts.retain(|t| !t.flash);
        self.push_toast(ToastKind::Info, text.into(), true, cx);
    }

    /// A toast with a button that opens a dialog; it stays a little longer.
    pub fn toast_with(&mut self, kind: ToastKind, text: impl Into<SharedString>, label: impl Into<SharedString>, open: Dialog, cx: &mut Context<Self>) {
        self.push(Toast { id: 0, kind, text: text.into(), flash: false, action: Some((label.into(), open)) }, cx);
    }

    fn push_toast(&mut self, kind: ToastKind, text: SharedString, flash: bool, cx: &mut Context<Self>) {
        self.push(Toast { id: 0, kind, text, flash, action: None }, cx);
    }

    fn push(&mut self, mut toast: Toast, cx: &mut Context<Self>) {
        let id = self.next_toast;
        self.next_toast += 1;
        toast.id = id;
        // The same message twice in a row (a failing command retried) shows once.
        self.toasts.retain(|t| t.text != toast.text);
        let (kind, flash, action) = (toast.kind, toast.flash, toast.action.is_some());
        self.toasts.push(toast);
        // Never more than a handful on screen.
        while self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        let ms = match kind {
            _ if flash => 1800,
            _ if action => 12000,
            ToastKind::Error => 7000,
            _ => 4200,
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(ms)).await;
            this.update(cx, |s, cx| {
                s.toasts.retain(|t| t.id != id);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn info(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast(ToastKind::Info, text, cx);
    }

    pub fn error(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast(ToastKind::Error, text, cx);
    }

    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.retain(|t| t.id != id);
        cx.notify();
    }

    // ---- lookups ----------------------------------------------------------

    pub fn fps(&self) -> f64 {
        self.project.as_ref().map(|p| p.settings.fps).unwrap_or(30.0)
    }

    pub fn duration(&self) -> f64 {
        self.project.as_ref().map(|p| p.duration()).unwrap_or(0.0)
    }

    pub fn clip(&self, id: Id) -> Option<&Clip> {
        self.project.as_ref()?.clip(id)
    }

    pub fn track_of(&self, clip: Id) -> Option<&Track> {
        let p = self.project.as_ref()?;
        p.locate_clip(clip).map(|(t, _)| &p.tracks[t])
    }

    pub fn asset(&self, id: Id) -> Option<&Asset> {
        self.project.as_ref()?.asset(id)
    }

    pub fn asset_of(&self, clip: &Clip) -> Option<&Asset> {
        clip.asset_id().and_then(|id| self.asset(id))
    }

    pub fn selected_clips(&self) -> Vec<&Clip> {
        self.selection.iter().filter_map(|id| self.clip(*id)).collect()
    }

    /// Ready models (all providers), loaded once and refreshed on demand.
    pub fn load_models(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.models_loading {
            return;
        }
        self.models_loading = true;
        cx.notify();
        let params = json!({ "refresh": refresh });
        let task = self.call("generate.models", params, cx);
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |s, cx| {
                s.models_loading = false;
                match r {
                    Ok(v) => s.models = serde_json::from_value(v).unwrap_or_default(),
                    Err(e) => s.error(e, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- selection --------------------------------------------------------

    pub fn select(&mut self, id: Id, additive: bool, cx: &mut Context<Self>) {
        self.selected_asset = None;
        if additive {
            if let Some(i) = self.selection.iter().position(|x| *x == id) {
                self.selection.remove(i);
            } else {
                self.selection.push(id);
            }
        } else {
            self.selection = vec![id];
        }
        self.sync_ui(cx);
        cx.notify();
    }

    pub fn set_selection(&mut self, ids: Vec<Id>, cx: &mut Context<Self>) {
        self.selection = ids;
        if !self.selection.is_empty() {
            self.selected_asset = None;
        }
        self.sync_ui(cx);
        cx.notify();
    }

    pub fn select_asset(&mut self, id: Option<Id>, cx: &mut Context<Self>) {
        self.selected_asset = id;
        if id.is_some() {
            self.selection.clear();
        }
        self.sync_ui(cx);
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.clear();
        self.sync_ui(cx);
        cx.notify();
    }

    // ---- panels -----------------------------------------------------------

    pub fn open_dialog(&mut self, d: Dialog, cx: &mut Context<Self>) {
        self.dialog = Some(d);
        self.menu = None;
        self.sync_ui(cx);
        cx.notify();
    }

    pub fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        self.sync_ui(cx);
        cx.notify();
    }

    /// The Agent panel, docked on the right.
    pub fn set_agent_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.agent_open = open;
        self.sync_ui(cx);
        cx.notify();
    }

    /// The generation jobs popover.
    pub fn set_jobs_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.jobs_open = open;
        self.sync_ui(cx);
        cx.notify();
    }

    pub fn set_snapping(&mut self, on: bool, cx: &mut Context<Self>) {
        self.snapping = on;
        self.sync_ui(cx);
        cx.notify();
    }

    pub fn set_ripple(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ripple = on;
        self.sync_ui(cx);
        cx.notify();
    }

    /// Shows a tab of the left panel (opening the panel if it was closed).
    pub fn set_left_tab(&mut self, tab: LeftTab, cx: &mut Context<Self>) {
        self.left_tab = tab;
        self.left_reveal += 1;
        self.sync_ui(cx);
        cx.notify();
    }

    pub fn open_menu(&mut self, position: Point<Pixels>, entries: Vec<MenuEntry>, cx: &mut Context<Self>) {
        self.menu = Some(ContextMenu { position, entries });
        cx.notify();
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    /// Opens a motion clip in the Studio (it takes the editor's centre).
    pub fn open_studio(&mut self, clip: Id, cx: &mut Context<Self>) {
        if !matches!(self.clip(clip).map(|c| &c.content), Some(kimchi_core::ClipContent::Motion { .. })) {
            self.flash("Only motion clips open in the Studio: add one from the Motion tab.", cx);
            return;
        }
        self.menu = None;
        cx.emit(StoreEvent::OpenStudio(clip));
        cx.notify();
    }

    /// The Studio's state changed (or it closed: `None`).
    pub fn set_studio_state(&mut self, state: Option<Value>, cx: &mut Context<Self>) {
        if self.studio != state {
            self.studio = state;
            self.sync_ui(cx);
        }
    }

    pub fn compose(&mut self, req: ComposeRequest, cx: &mut Context<Self>) {
        self.left_tab = LeftTab::Generate;
        cx.emit(StoreEvent::Compose(Box::new(req)));
        self.sync_ui(cx);
        cx.notify();
    }

    /// Changes what the audio views show (mixer, strip, effect panel…).
    pub fn set_audio(&mut self, f: impl FnOnce(&mut crate::views::mixer::AudioView), cx: &mut Context<Self>) {
        let before = self.audio.clone();
        f(&mut self.audio);
        if self.audio != before {
            self.sync_ui(cx);
            cx.notify();
        }
    }

    pub fn set_zoom(&mut self, pps: f64, cx: &mut Context<Self>) {
        self.pps = pps.clamp(MIN_PPS, MAX_PPS);
        self.sync_ui(cx);
        cx.notify();
    }
}
