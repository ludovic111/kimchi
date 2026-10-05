//! The editor: the top bar; the rail of tabs, the left panel | preview | inspector with the
//! timeline below; and the Agent panel on the right. Panels resize with splitters.
//!
//! Where each panel goes is decided by [`crate::ui::layout::solve`] at every frame from the
//! window's size and the sizes the person chose ([`Prefs`], saved): side panels that don't
//! fit beside the preview become drawers over the work, the Agent panel floats over a
//! narrow editor, and the top bar folds its secondary buttons into a "more" menu.

use gpui::{App, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, Subscription, Window, div, prelude::*, px};
use serde_json::json;

use crate::actions::{self as act, tip};
use crate::store::{Dialog, LeftTab, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::layout::{self, Breakpoint, Dock, Open, Prefs, Solved};
use crate::ui::{Button, GlassExt, drag, icon, motion};
use crate::views::{agent_panel::AgentPanel, inspector::Inspector, jobs::JobsPopover, left_panel::LeftPanel, preview::PreviewView, studio::Studio, timeline::Timeline};

pub const TOPBAR_H: f32 = layout::TOPBAR_H;

#[derive(Clone, Copy, PartialEq)]
enum Splitter {
    Left,
    Right,
    Timeline,
    Agent,
}

pub struct Editor {
    store: Entity<Store>,
    pub left: Entity<LeftPanel>,
    pub preview: Entity<PreviewView>,
    pub inspector: Entity<Inspector>,
    pub timeline: Entity<Timeline>,
    pub agent: Entity<AgentPanel>,
    /// The motion clips' editor; it takes the centre while open.
    pub studio: Entity<Studio>,
    jobs: Entity<JobsPopover>,
    rename: Option<(Entity<TextInput>, Subscription)>,
    /// The panel sizes the person chose (drawn clamped to the window).
    prefs: Prefs,
    /// Side panels shown as drawers over the work (narrow windows).
    left_drawer: bool,
    inspector_drawer: bool,
    /// The layout as last drawn.
    solved: Solved,
    /// The store's `left_reveal` as last seen: a tab asked for opens the left panel.
    reveal_seen: u64,
    resizing: Option<(Splitter, Pixels, f32)>,
    _sub: Subscription,
    _studio_subs: Vec<Subscription>,
}

impl Editor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let sub = cx.observe(&store, |this: &mut Self, store, cx| {
            let reveal = store.read(cx).left_reveal;
            if reveal != this.reveal_seen {
                this.reveal_seen = reveal;
                this.show_left(cx);
            }
            cx.notify()
        });
        let studio = cx.new(|cx| Studio::new(window, cx));
        let open = cx.subscribe_in(&store, window, |this: &mut Self, _, e: &crate::store::StoreEvent, window, cx| {
            if let crate::store::StoreEvent::OpenStudio(clip) = e {
                let clip = *clip;
                this.studio.update(cx, |s, cx| s.open(clip, window, cx));
                cx.notify();
            }
        });
        let watch = cx.observe(&studio, |_, _, cx| cx.notify());
        let prefs = Prefs::load(&store.read(cx).session.config_dir);
        let reveal_seen = store.read(cx).left_reveal;
        let solved = layout::solve(window.viewport_size(), &prefs, Open::default());
        let this = Self {
            studio,
            _studio_subs: vec![open, watch],
            left: cx.new(|cx| LeftPanel::new(window, cx)),
            preview: cx.new(|cx| PreviewView::new(window, cx)),
            inspector: cx.new(|cx| Inspector::new(window, cx)),
            timeline: cx.new(|cx| Timeline::new(window, cx)),
            agent: cx.new(|cx| AgentPanel::new(window, cx)),
            jobs: cx.new(|cx| JobsPopover::new(window, cx)),
            store,
            rename: None,
            prefs,
            left_drawer: false,
            inspector_drawer: false,
            solved,
            reveal_seen,
            resizing: None,
            _sub: sub,
        };
        this.publish_layout(cx);
        this
    }

    /// The layout as `ui.state` and `ui.setLayout` report it.
    fn ui_layout(&self) -> kimchi_control::session::UiLayout {
        let s = &self.solved;
        let overlays = [("left", s.left), ("inspector", s.inspector), ("agent", s.agent)].into_iter().filter(|(_, d)| d.is_drawer()).map(|(n, _)| n.to_string()).collect();
        kimchi_control::session::UiLayout {
            left: s.left.width().round(),
            inspector: s.inspector.width().round(),
            timeline: s.timeline_h.round(),
            agent: if s.agent == Dock::Hidden { self.prefs.agent } else { s.agent.width() }.round(),
            left_open: s.left != Dock::Hidden,
            inspector_open: s.inspector != Dock::Hidden,
            overlays,
            window: [s.window.0.round(), s.window.1.round()],
        }
    }

    /// Tells `ui.state` the panel sizes.
    fn publish_layout(&self, cx: &App) {
        let layout = self.ui_layout();
        let session = self.store.read(cx).session.clone();
        if session.ui_state().layout != layout {
            session.update_ui_state(|s| s.layout = layout);
        }
    }

    fn save_prefs(&self, cx: &App) {
        self.prefs.save(&self.store.read(cx).session.config_dir);
    }

    fn resolve(&mut self, cx: &App) {
        let agent = self.store.read(cx).agent_open;
        let (w, h) = self.solved.window;
        self.solved = layout::solve(gpui::size(px(w), px(h)), &self.prefs, Open { agent, left_drawer: self.left_drawer, inspector_drawer: self.inspector_drawer });
    }

    /// `ui.setLayout`: sizes in pixels, each within what the editor allows.
    pub fn set_layout(&mut self, params: &serde_json::Value, cx: &mut Context<Self>) -> kimchi_control::session::UiLayout {
        if params["reset"].as_bool() == Some(true) {
            self.prefs = Prefs::default();
            self.left_drawer = false;
            self.inspector_drawer = false;
        }
        let get = |k: &str| params[k].as_f64().map(|v| v as f32).filter(|v| v.is_finite());
        if let Some(v) = get("left") {
            self.prefs.left = v.clamp(layout::LEFT_MIN, layout::LEFT_MAX);
        }
        if let Some(v) = get("inspector") {
            self.prefs.inspector = v.clamp(layout::INSPECTOR_MIN, layout::INSPECTOR_MAX);
        }
        if let Some(v) = get("timeline") {
            self.prefs.timeline = layout::timeline_share(v, self.solved.window.1);
        }
        if let Some(v) = get("agent") {
            self.prefs.agent = v.clamp(layout::AGENT_MIN, layout::AGENT_MAX);
        }
        if let Some(open) = params["leftOpen"].as_bool() {
            self.set_left_open(open, cx);
        }
        if let Some(open) = params["inspectorOpen"].as_bool() {
            self.set_inspector_open(open, cx);
        }
        self.resolve(cx);
        self.save_prefs(cx);
        self.publish_layout(cx);
        cx.notify();
        self.ui_layout()
    }

    /// Width the timeline's tracks have (for zoom to fit).
    pub fn fit_timeline(this: &Entity<Self>, cx: &mut App) {
        let timeline = this.read(cx).timeline.clone();
        crate::views::timeline::Timeline::fit(&timeline, cx);
    }

    // ---- side panels ------------------------------------------------------------

    /// Whether the left panel's content shows (docked or as a drawer).
    fn left_shown(&self) -> bool {
        self.solved.left != Dock::Hidden
    }

    /// Opens the left panel: in the row when it fits, else as a drawer.
    fn show_left(&mut self, cx: &mut Context<Self>) {
        self.set_left_open(true, cx);
    }

    pub fn set_left_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if open {
            if self.solved.left_fits {
                self.prefs.left_open = true;
                self.left_drawer = false;
            } else {
                self.left_drawer = true;
                self.inspector_drawer = false;
            }
        } else if self.left_drawer {
            self.left_drawer = false;
        } else {
            self.prefs.left_open = false;
        }
        self.after_toggle(cx);
    }

    pub fn set_inspector_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if open {
            if self.solved.inspector_fits {
                self.prefs.inspector_open = true;
                self.inspector_drawer = false;
            } else {
                self.inspector_drawer = true;
                self.left_drawer = false;
            }
        } else if self.inspector_drawer {
            self.inspector_drawer = false;
        } else {
            self.prefs.inspector_open = false;
        }
        self.after_toggle(cx);
    }

    fn after_toggle(&mut self, cx: &mut Context<Self>) {
        self.resolve(cx);
        self.save_prefs(cx);
        self.publish_layout(cx);
        cx.notify();
    }

    /// A click on a tab of the rail: shows it, or closes the panel when it is the one showing.
    fn pick_tab(&mut self, tab: LeftTab, cx: &mut Context<Self>) {
        if self.store.read(cx).left_tab == tab && self.left_shown() {
            self.set_left_open(false, cx);
        } else {
            // `set_left_tab` asks for the panel to show (see `left_reveal`).
            self.store.update(cx, |s, cx| s.set_left_tab(tab, cx));
        }
    }

    /// Closes the drawers (Escape); whether there was one.
    pub fn close_drawers(&mut self, cx: &mut Context<Self>) -> bool {
        let any = self.left_drawer || self.inspector_drawer || (self.solved.agent.is_drawer());
        if self.solved.agent.is_drawer() {
            self.store.update(cx, |s, cx| s.set_agent_open(false, cx));
        }
        self.left_drawer = false;
        self.inspector_drawer = false;
        if any {
            self.after_toggle(cx);
        }
        any
    }

    // ---- splitters ----------------------------------------------------------------

    fn start_resize(&mut self, which: Splitter, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let s = &self.solved;
        let (pos, value) = match which {
            Splitter::Timeline => (e.position.y, s.timeline_h),
            Splitter::Left => (e.position.x, s.left.width()),
            Splitter::Right => (e.position.x, s.inspector.width()),
            Splitter::Agent => (e.position.x, s.agent.width()),
        };
        self.resizing = Some((which, pos, value));
        cx.notify();
    }

    fn resize_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((which, start, value)) = self.resizing else { return };
        let delta = match which {
            Splitter::Timeline => f32::from(e.position.y - start),
            _ => f32::from(e.position.x - start),
        };
        // Within the panel's limits and what the window leaves the preview.
        let s = self.solved;
        match which {
            Splitter::Left => {
                let max = layout::max_side(&s, s.inspector.row_width(), layout::LEFT_MAX).max(layout::LEFT_MIN);
                self.prefs.left = (value + delta).clamp(layout::LEFT_MIN, max);
            }
            Splitter::Right => {
                let max = layout::max_side(&s, s.left.row_width(), layout::INSPECTOR_MAX).max(layout::INSPECTOR_MIN);
                self.prefs.inspector = (value - delta).clamp(layout::INSPECTOR_MIN, max);
            }
            Splitter::Timeline => {
                let body = s.window.1 - layout::TOPBAR_H;
                let h = (value - delta).clamp(layout::TIMELINE_MIN, (body - layout::PREVIEW_MIN_H).max(layout::TIMELINE_MIN));
                self.prefs.timeline = layout::timeline_share(h, s.window.1);
            }
            Splitter::Agent => {
                let max = (s.window.0 - layout::EDITOR_MIN_BESIDE_AGENT).min(layout::AGENT_MAX).max(layout::AGENT_MIN);
                self.prefs.agent = (value - delta).clamp(layout::AGENT_MIN, max);
            }
        }
        self.resolve(cx);
        cx.notify();
    }

    fn resize_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.resizing = None;
        self.save_prefs(cx);
        self.publish_layout(cx);
        cx.notify();
    }

    fn splitter(&self, which: Splitter, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let vertical = which != Splitter::Timeline;
        let active = self.resizing.is_some_and(|(w, _, _)| w == which);
        let name = match which {
            Splitter::Left => "split-left",
            Splitter::Right => "split-right",
            Splitter::Timeline => "split-timeline",
            Splitter::Agent => "split-agent",
        };
        div()
            .id(name)
            .debug_selector(|| name.into())
            .flex_none()
            .when(vertical, |d| d.w(px(5.)).h_full().cursor_ew_resize().mx(px(-2.)))
            .when(!vertical, |d| d.h(px(5.)).w_full().cursor_ns_resize().my(px(-2.)))
            .relative()
            .child(
                div()
                    .absolute()
                    .when(vertical, |d| d.left(px(2.)).w(px(1.)).h_full())
                    .when(!vertical, |d| d.top(px(2.)).h(px(1.)).w_full())
                    .bg(if active { t.accent } else { gpui::transparent_black() }),
            )
            .hover(move |s| s.bg(t.accent_soft))
            .tooltip(|_, cx| crate::ui::tooltip("Drag to resize · double-click to reset".into(), cx))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                if e.click_count == 2 {
                    this.reset_size(which, cx);
                } else {
                    this.start_resize(which, e, cx);
                }
            }))
            .into_any_element()
    }

    fn reset_size(&mut self, which: Splitter, cx: &mut Context<Self>) {
        self.resizing = None;
        let d = Prefs::default();
        match which {
            Splitter::Left => self.prefs.left = d.left,
            Splitter::Right => self.prefs.inspector = d.inspector,
            Splitter::Timeline => self.prefs.timeline = d.timeline,
            Splitter::Agent => self.prefs.agent = d.agent,
        }
        self.after_toggle(cx);
    }

    // ---- top bar ------------------------------------------------------------------

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.store.read(cx).project.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx);
            i.set_text(name, cx);
            i.select_all_text(cx);
            i
        });
        crate::ui::input::focus(&input, window, cx);
        let sub = cx.subscribe(&input, |this, input, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Blur => {
                let name = input.read(cx).text().trim().to_string();
                let current = this.store.read(cx).project.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                if !name.is_empty() && name != current {
                    this.store.update(cx, |s, cx| s.run("project.rename", json!({ "name": name }), cx));
                }
                this.rename = None;
                cx.notify();
            }
            InputEvent::Cancel => {
                this.rename = None;
                cx.notify();
            }
            InputEvent::Changed(_) => {}
        });
        self.rename = Some((input, sub));
        cx.notify();
    }

    /// The "more" menu: what the top bar has no room for, and the less used things.
    fn more_menu(&self, position: gpui::Point<Pixels>, bp: Breakpoint, cx: &mut Context<Self>) {
        let entries = vec![
            MenuItem::new("Command palette", |_, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Palette, cx))).icon("command").shortcut_of(&act::Palette).entry(),
            MenuItem::new("Keyboard shortcuts", |_, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Shortcuts, cx))).icon("keyboard").shortcut_of(&act::ShowShortcuts).entry(),
            MenuItem::new("Settings", |_, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: None }, cx))).icon("settings").shortcut_of(&act::OpenSettings).entry(),
            crate::store::MenuEntry::Separator,
            MenuItem::new("What's new", |_, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::WhatsNew { since: None, all: false }, cx))).icon("gift").entry(),
            MenuItem::new("Help", |_, cx| cx.open_url(crate::app::HELP_URL)).icon("info").entry(),
            MenuItem::new("Support kimchi", |_, cx| cx.open_url(crate::app::SUPPORT_URL)).icon("heart").entry(),
        ];
        // Settings has its own button from the medium width up.
        let entries = entries.into_iter().filter(|e| !(bp > Breakpoint::Compact && matches!(e, crate::store::MenuEntry::Item(i) if i.label.as_ref() == "Settings"))).collect();
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn top_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let bp = layout::breakpoint(self.solved.window.0);
        let s = self.store.read(cx);
        let p = s.project.clone();
        let (can_undo, can_redo) = (s.can_undo, s.can_redo);
        let active = s.jobs.iter().filter(|j| !j.status.is_done()).count();
        let jobs_open = s.jobs_open;
        let agent_open = s.agent_open;
        let update = s.update.clone();
        let studio_open = self.studio.read(cx).is_open();
        let name: gpui::SharedString = p.as_ref().map(|p| p.name.clone()).unwrap_or_default().into();
        let spec = p.as_ref().map(|p| format!("{}×{} · {}fps", p.settings.width, p.settings.height, p.settings.fps)).unwrap_or_default();
        let fullscreen = window.is_fullscreen();
        let controls = crate::ui::window_controls(window, cx);
        let (left_on, insp_on) = (self.left_shown(), self.solved.inspector != Dock::Hidden);
        let this = cx.entity();
        let sep = || div().flex_none().w(px(1.)).h(px(18.)).mx(px(4.)).bg(t.line);
        let name_tip = name.clone();
        div()
            .id("top-bar")
            .debug_selector(|| "top-bar".into())
            .h(px(TOPBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            // Room for the macOS traffic lights.
            .pl(px(if fullscreen || !cfg!(target_os = "macos") { 10. } else { 84. }))
            .when(controls.is_none(), |d| d.pr(px(10.)))
            .window_control_area(gpui::WindowControlArea::Drag)
            .glass(t.glass1)
            .border_0()
            .border_b_1()
            .border_color(t.line)
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("home")
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(4.))
                            .px(px(6.))
                            .h(px(30.))
                            .rounded(px(sz::R_SM))
                            .cursor_pointer()
                            .text_color(t.text_2)
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .tooltip(|_, cx| crate::ui::tooltip(tip("All projects", &act::CloseProject), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("project.close", json!({}), cx)))
                            .child(icon("chevron-left"))
                            .child(crate::views::home::mark(18., cx)),
                    )
                    .child(div().flex_none().text_color(t.text_3).child("/"))
                    .child(match &self.rename {
                        Some((input, _)) => div().w(px(240.)).min_w_0().child(input.clone()).into_any_element(),
                        None => div()
                            .id("project-name")
                            .min_w(px(40.))
                            .max_w(px(360.))
                            .truncate()
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(sz::R_SM))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .tooltip(move |_, cx| crate::ui::tooltip(format!("{name_tip} · click to rename").into(), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx)))
                            .child(name)
                            .into_any_element(),
                    })
                    .when(bp >= Breakpoint::Wide, |d| d.child(div().flex_none().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(spec))),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when_some(update.available.clone().filter(|_| bp > Breakpoint::Compact), |d, v| {
                        let button = if update.ready {
                            Button::new("update", "Restart to update").small().primary().with_icon("refresh-cw").on_click(|_, _, cx| crate::app::restart(cx))
                        } else if let Some(p) = update.progress {
                            Button::new("update", format!("Updating… {:.0}%", p * 100.0)).small().disabled(true)
                        } else if update.can_install {
                            Button::new("update", format!("Update to {v}"))
                                .small()
                                .with_icon("download")
                                .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("app.installUpdate", json!({}), cx)))
                        } else {
                            let url = update.download_url.clone().unwrap_or_else(|| kimchi_control::update::RELEASES_URL.into());
                            Button::new("update", format!("kimchi {v} is out")).small().with_icon("external-link").on_click(move |_, _, cx| cx.open_url(&url))
                        };
                        d.child(div().mr(px(6.)).child(button))
                    })
                    .child(Button::icon("undo", "undo-2", tip("Undo", &act::Undo)).disabled(!can_undo).on_click(|_, _, cx| cx.store().update(cx, |s, cx| crate::app::undo_redo(s, true, cx))))
                    .child(Button::icon("redo", "redo-2", tip("Redo", &act::Redo)).disabled(!can_redo).on_click(|_, _, cx| cx.store().update(cx, |s, cx| crate::app::undo_redo(s, false, cx))))
                    .child(sep())
                    .child(
                        div()
                            .relative()
                            .child({
                                let label = if active > 0 { format!("{active} generating") } else { "Generations".into() };
                                let b = if bp >= Breakpoint::Wide || active > 0 && bp > Breakpoint::Compact {
                                    Button::new("jobs", label).small().ghost().with_icon(if active > 0 { "loader-circle" } else { "sparkles" })
                                } else {
                                    Button::icon("jobs", if active > 0 { "loader-circle" } else { "sparkles" }, label)
                                };
                                b.selected(jobs_open).color(if active > 0 { t.accent_text } else { t.text_2 }).tooltip(tip("Generations", &act::ToggleJobs)).on_click(|_, _, cx| {
                                    cx.store().update(cx, |s, cx| s.set_jobs_open(!s.jobs_open, cx))
                                })
                            })
                            .when(jobs_open, |d| d.child(self.jobs.clone())),
                    )
                    .child(
                        Button::icon("agent", "bot", tip("Agent", &act::ToggleAgent))
                            .selected(agent_open)
                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.set_agent_open(!s.agent_open, cx))),
                    )
                    .when(!studio_open, |d| {
                        let (e1, e2) = (this.clone(), this.clone());
                        d.child(sep())
                            .child(Button::icon("toggle-left", "panel-left", if left_on { "Hide the left panel" } else { "Show the left panel" }).selected(left_on).on_click(move |_, _, cx| {
                                e1.update(cx, |e, cx| e.set_left_open(!left_on, cx))
                            }))
                            .child(Button::icon("toggle-inspector", "panel-right", if insp_on { "Hide the inspector" } else { "Show the inspector" }).selected(insp_on).on_click(move |_, _, cx| {
                                e2.update(cx, |e, cx| e.set_inspector_open(!insp_on, cx))
                            }))
                    })
                    .child(sep())
                    .when(bp > Breakpoint::Compact, |d| {
                        d.child(
                            // Until a provider is set up, models and keys are what settings are for.
                            Button::icon("settings", "settings", tip("Settings", &act::OpenSettings)).on_click(|_, _, cx| {
                                cx.store().update(cx, |s, cx| {
                                    let section = (!s.providers.iter().any(|p| p.ready)).then(|| "models".to_string());
                                    s.open_dialog(Dialog::Settings { section }, cx)
                                })
                            }),
                        )
                    })
                    .child(Button::icon("more", "ellipsis", "More: palette, shortcuts, help…").on_click(move |e, _, cx| {
                        let at = e.position();
                        this.update(cx, |ed, cx| ed.more_menu(gpui::point(at.x - px(200.), at.y + px(18.)), bp, cx))
                    }))
                    .child(
                        div().ml(px(6.)).child(
                            Button::new("export", "Export")
                                .small()
                                .primary()
                                .with_icon("share")
                                .tooltip(tip("Export", &act::Export))
                                .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Export, cx))),
                        ),
                    ),
            )
            .children(controls)
            .into_any_element()
    }

    // ---- drawers -------------------------------------------------------------------

    /// A panel over the work, from one side, with a quiet scrim behind that closes it.
    fn drawer(&self, id: &'static str, right: bool, offset: f32, width: f32, content: gpui::AnyElement, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let panel = div()
            .id(id)
            .occlude()
            .relative()
            .size_full()
            .bg(t.bg_raised)
            .shadow(t.glass_shadow())
            .border_color(t.line_strong)
            .when(right, |d| d.border_l_1())
            .when(!right, |d| d.border_r_1())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(content);
        div()
            .absolute()
            .inset_0()
            .child(
                div().id((id, 1usize)).absolute().inset_0().left(px(offset)).bg(t.scrim.opacity(0.35)).on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.close_drawers(cx);
                })),
            )
            // The slot holds the place; the panel slides within it.
            .child(
                div()
                    .debug_selector(move || id.into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(px(width))
                    .when(right, |d| d.right_0())
                    .when(!right, |d| d.left(px(offset)))
                    .child(motion::enter(panel, (id, 2usize), motion::BASE, (if right { 24. } else { -24. }, 0.))),
            )
            .into_any_element()
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let agent_open = self.store.read(cx).agent_open;
        // Lay the panels out for the window as it is now.
        let viewport = window.viewport_size();
        let solved = layout::solve(viewport, &self.prefs, Open { agent: agent_open, left_drawer: self.left_drawer, inspector_drawer: self.inspector_drawer });
        if solved != self.solved {
            self.solved = solved;
            // A drawer whose panel now fits goes back into the row.
            if solved.left_fits && self.left_drawer {
                self.left_drawer = false;
                self.prefs.left_open = true;
            }
            if solved.inspector_fits && self.inspector_drawer {
                self.inspector_drawer = false;
                self.prefs.inspector_open = true;
            }
            self.resolve(cx);
            self.publish_layout(cx);
        }
        let s = self.solved;
        let resizing = self.resizing.is_some();
        let studio_open = self.studio.read(cx).is_open();
        let tab = self.store.read(cx).left_tab;
        let active_jobs = self.store.read(cx).jobs.iter().filter(|j| !j.status.is_done()).count();
        let top = self.top_bar(window, cx);
        let this = cx.entity();
        let rail = crate::views::left_panel::rail(tab, self.left_shown(), active_jobs, move |tab, _, cx| this.update(cx, |e, cx| e.pick_tab(tab, cx)), cx);

        let work = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when_some(match s.left {
                        Dock::Docked(w) => Some(w),
                        _ => None,
                    }, |d, w| d.child(div().w(px(w)).flex_none().h_full().debug_selector(|| "left-panel".into()).child(self.left.clone().cached(full()))).child(self.splitter(Splitter::Left, cx)))
                    // The work: solid, never glass.
                    .child(div().flex_1().min_w_0().h_full().bg(t.bg_sunken).debug_selector(|| "preview".into()).child(self.preview.clone()))
                    .when_some(match s.inspector {
                        Dock::Docked(w) => Some(w),
                        _ => None,
                    }, |d, w| d.child(self.splitter(Splitter::Right, cx)).child(div().w(px(w)).flex_none().h_full().debug_selector(|| "inspector".into()).child(self.inspector.clone().cached(full())))),
            )
            .child(self.splitter(Splitter::Timeline, cx))
            .child(div().h(px(s.timeline_h)).flex_none().w_full().bg(t.bg_raised).debug_selector(|| "timeline".into()).child(self.timeline.clone()));

        let body = div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .when(studio_open, |d| d.child(div().flex_1().min_w_0().h_full().child(self.studio.clone())))
            .when(!studio_open, |d| d.child(rail).child(work))
            .when_some(match s.agent {
                Dock::Docked(w) => Some(w),
                _ => None,
            }, |d, w| {
                d.child(self.splitter(Splitter::Agent, cx)).child(motion::enter(
                    div().relative().w(px(w)).flex_none().h_full().debug_selector(|| "agent".into()).child(self.agent.clone().cached(full())),
                    "agent-in",
                    motion::BASE,
                    (24., 0.),
                ))
            })
            // Panels the window is too narrow to dock, over the work.
            .when(!studio_open, |d| {
                d.when_some(match s.left {
                    Dock::Drawer(w) => Some(w),
                    _ => None,
                }, |d, w| {
                    let content = self.left.clone().cached(full()).into_any_element();
                    d.child(self.drawer("left-drawer", false, layout::RAIL_W, w, content, cx))
                })
                .when_some(match s.inspector {
                    Dock::Drawer(w) => Some(w),
                    _ => None,
                }, |d, w| {
                    let content = self.inspector.clone().cached(full()).into_any_element();
                    d.child(self.drawer("inspector-drawer", true, 0., w, content, cx))
                })
            })
            .when_some(match s.agent {
                Dock::Drawer(w) => Some(w),
                _ => None,
            }, |d, w| {
                let content = self.agent.clone().cached(full()).into_any_element();
                d.child(self.drawer("agent-drawer", true, 0., w, content, cx))
            });

        div()
            .key_context("Editor")
            .size_full()
            .flex()
            .flex_col()
            .child(top)
            .child(body)
            .when(resizing, |d| d.child(drag::track(cx.entity(), Self::resize_move, Self::resize_end)))
    }
}

/// Panels are cached views: playback redraws the preview and the playhead, not every panel.
fn full() -> gpui::StyleRefinement {
    gpui::StyleRefinement::default().size_full()
}
