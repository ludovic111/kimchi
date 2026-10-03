//! The editor: top bar, left panel | preview | inspector, the timeline below,
//! and the agent panel docked on the right. Panels resize with splitters.

use gpui::{App, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, Subscription, Window, div, prelude::*, px};
use serde_json::json;

use crate::store::{Dialog, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::actions::{self as act, tip};
use crate::ui::{Button, GlassExt, drag, icon, motion};
use crate::views::{agent_panel::AgentPanel, inspector::Inspector, jobs::JobsPopover, left_panel::LeftPanel, preview::PreviewView, studio::Studio, timeline::Timeline};

pub const TOPBAR_H: f32 = 52.;
// Panel sizes to start with (and to go back to on a double-click on a divider).
const LEFT_W: f32 = 340.;
const RIGHT_W: f32 = 300.;
const TIMELINE_H: f32 = 300.;
const AGENT_W: f32 = 380.;

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
    left_w: f32,
    right_w: f32,
    timeline_h: f32,
    agent_w: f32,
    resizing: Option<(Splitter, Pixels, f32)>,
    _sub: Subscription,
    _studio_subs: Vec<Subscription>,
}

impl Editor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let sub = cx.observe(&store, |_, _, cx| cx.notify());
        let studio = cx.new(|cx| Studio::new(window, cx));
        let open = cx.subscribe_in(&store, window, |this: &mut Self, _, e: &crate::store::StoreEvent, window, cx| {
            if let crate::store::StoreEvent::OpenStudio(clip) = e {
                let clip = *clip;
                this.studio.update(cx, |s, cx| s.open(clip, window, cx));
                cx.notify();
            }
        });
        let watch = cx.observe(&studio, |_, _, cx| cx.notify());
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
            left_w: LEFT_W,
            right_w: RIGHT_W,
            timeline_h: TIMELINE_H,
            agent_w: AGENT_W,
            resizing: None,
            _sub: sub,
        };
        this.publish_layout(cx);
        this
    }

    /// Tells `ui.state` the panel sizes.
    fn publish_layout(&self, cx: &App) {
        let layout = kimchi_control::session::UiLayout { left: self.left_w, inspector: self.right_w, timeline: self.timeline_h, agent: self.agent_w };
        self.store.read(cx).session.update_ui_state(|s| s.layout = layout);
    }

    /// `ui.setLayout`: sizes in pixels, each within what the editor allows.
    pub fn set_layout(&mut self, params: &serde_json::Value, cx: &mut Context<Self>) -> kimchi_control::session::UiLayout {
        if params["reset"].as_bool() == Some(true) {
            for which in [Splitter::Left, Splitter::Right, Splitter::Timeline, Splitter::Agent] {
                self.reset_size(which, cx);
            }
        }
        let get = |k: &str| params[k].as_f64().map(|v| v as f32);
        if let Some(v) = get("left") {
            self.left_w = v.clamp(280., 520.);
        }
        if let Some(v) = get("inspector") {
            self.right_w = v.clamp(260., 440.);
        }
        if let Some(v) = get("timeline") {
            self.timeline_h = v.clamp(180., 620.);
        }
        if let Some(v) = get("agent") {
            self.agent_w = v.clamp(300., 560.);
        }
        self.publish_layout(cx);
        cx.notify();
        kimchi_control::session::UiLayout { left: self.left_w, inspector: self.right_w, timeline: self.timeline_h, agent: self.agent_w }
    }

    /// Width the timeline's tracks have (for zoom to fit).
    pub fn fit_timeline(this: &Entity<Self>, cx: &mut App) {
        let timeline = this.read(cx).timeline.clone();
        crate::views::timeline::Timeline::fit(&timeline, cx);
    }

    fn start_resize(&mut self, which: Splitter, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let (pos, value) = match which {
            Splitter::Timeline => (e.position.y, self.timeline_h),
            Splitter::Left => (e.position.x, self.left_w),
            Splitter::Right => (e.position.x, self.right_w),
            Splitter::Agent => (e.position.x, self.agent_w),
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
        match which {
            Splitter::Left => self.left_w = (value + delta).clamp(280., 520.),
            Splitter::Right => self.right_w = (value - delta).clamp(260., 440.),
            Splitter::Timeline => self.timeline_h = (value - delta).clamp(180., 620.),
            Splitter::Agent => self.agent_w = (value - delta).clamp(300., 560.),
        }
        cx.notify();
    }

    fn resize_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.resizing = None;
        self.publish_layout(cx);
        cx.notify();
    }

    fn splitter(&self, which: Splitter, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let vertical = which != Splitter::Timeline;
        let active = self.resizing.is_some_and(|(w, _, _)| w == which);
        div()
            .id(match which {
                Splitter::Left => "split-left",
                Splitter::Right => "split-right",
                Splitter::Timeline => "split-timeline",
                Splitter::Agent => "split-agent",
            })
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
    }

    fn reset_size(&mut self, which: Splitter, cx: &mut Context<Self>) {
        self.resizing = None;
        match which {
            Splitter::Left => self.left_w = LEFT_W,
            Splitter::Right => self.right_w = RIGHT_W,
            Splitter::Timeline => self.timeline_h = TIMELINE_H,
            Splitter::Agent => self.agent_w = AGENT_W,
        }
        self.publish_layout(cx);
        cx.notify();
    }

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

    fn top_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let p = s.project.clone();
        let (can_undo, can_redo) = (s.can_undo, s.can_redo);
        let active = s.jobs.iter().filter(|j| !j.status.is_done()).count();
        let jobs_open = s.jobs_open;
        let agent_open = s.agent_open;
        let update = s.update.clone();
        let name = p.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let spec = p.as_ref().map(|p| format!("{}×{} · {}fps", p.settings.width, p.settings.height, p.settings.fps)).unwrap_or_default();
        let fullscreen = window.is_fullscreen();
        let controls = crate::ui::window_controls(window, cx);
        div()
            .id("top-bar")
            .h(px(TOPBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            // Room for the macOS traffic lights.
            .pl(px(if fullscreen || !cfg!(target_os = "macos") { 14. } else { 84. }))
            .when(controls.is_none(), |d| d.pr(px(12.)))
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
                    .gap(px(8.))
                    .child(
                        div()
                            .id("home")
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(6.))
                            .h(px(28.))
                            .rounded(px(sz::R_SM))
                            .cursor_pointer()
                            .text_color(t.text_2)
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .tooltip(|_, cx| crate::ui::tooltip("All projects".into(), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("project.close", json!({}), cx)))
                            .child(icon("chevron-left"))
                            .child(crate::views::home::mark(18., cx)),
                    )
                    .child(div().text_color(t.text_3).child("/"))
                    .child(match &self.rename {
                        Some((input, _)) => div().w(px(240.)).child(input.clone()).into_any_element(),
                        None => div()
                            .id("project-name")
                            .min_w_0()
                            .max_w(px(320.))
                            .truncate()
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(sz::R_SM))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .tooltip(|_, cx| crate::ui::tooltip("Rename".into(), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx)))
                            .child(name)
                            .into_any_element(),
                    })
                    .child(div().flex_none().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(spec)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when_some(update.available.clone(), |d, v| {
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
                        d.child(button)
                    })
                    .child(Button::icon("undo", "undo-2", tip("Undo", &act::Undo)).disabled(!can_undo).on_click(|_, _, cx| cx.store().update(cx, |s, cx| crate::app::undo_redo(s, true, cx))))
                    .child(Button::icon("redo", "redo-2", tip("Redo", &act::Redo)).disabled(!can_redo).on_click(|_, _, cx| cx.store().update(cx, |s, cx| crate::app::undo_redo(s, false, cx))))
                    .child(div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(t.line))
                    .child(
                        div()
                            .relative()
                            .child(
                                Button::new("jobs", if active > 0 { format!("{active} generating") } else { "Generations".into() })
                                    .small()
                                    .ghost()
                                    .with_icon(if active > 0 { "loader-circle" } else { "sparkles" })
                                    .selected(jobs_open)
                                    .color(if active > 0 { t.accent_text } else { t.text_2 })
                                    .on_click(|_, _, cx| {
                                        cx.store().update(cx, |s, cx| s.set_jobs_open(!s.jobs_open, cx))
                                    }),
                            )
                            .when(jobs_open, |d| d.child(self.jobs.clone())),
                    )
                    .child(
                        Button::icon("agent", "bot", tip("Agent", &act::ToggleAgent))
                            .selected(agent_open)
                            .on_click(|_, _, cx| {
                                cx.store().update(cx, |s, cx| s.set_agent_open(!s.agent_open, cx))
                            }),
                    )
                    .child(Button::icon("palette", "command", tip("Command palette", &act::Palette)).on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Palette, cx))))
                    .child(Button::icon("shortcuts", "keyboard", tip("Keyboard shortcuts", &act::ShowShortcuts)).on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Shortcuts, cx))))
                    .child(
                        // Until a provider is set up, models and keys are what settings are for.
                        Button::icon("settings", "settings", tip("Settings", &act::OpenSettings)).on_click(|_, _, cx| {
                            cx.store().update(cx, |s, cx| {
                                let section = (!s.providers.iter().any(|p| p.ready)).then(|| "models".to_string());
                                s.open_dialog(Dialog::Settings { section }, cx)
                            })
                        }),
                    )
                    .child(Button::icon("sponsor", "heart", "Support kimchi").on_click(|_, _, cx| cx.open_url(crate::app::SUPPORT_URL)))
                    .child(Button::new("export", "Export").small().primary().with_icon("share").tooltip(tip("Export", &act::Export)).on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Export, cx)))),
            )
            .children(controls)
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let agent_open = self.store.read(cx).agent_open;
        let resizing = self.resizing.is_some();
        let studio_open = self.studio.read(cx).is_open();
        let top = self.top_bar(window, cx);
        div()
            .key_context("Editor")
            .size_full()
            .flex()
            .flex_col()
            .child(top)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(studio_open, |d| d.child(div().flex_1().min_w_0().h_full().child(self.studio.clone())))
                    .when(!studio_open, |d| d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .child(div().w(px(self.left_w)).flex_none().h_full().child(self.left.clone().cached(full())))
                                    .child(self.splitter(Splitter::Left, cx))
                                    // The work: solid, never glass.
                                    .child(div().flex_1().min_w_0().h_full().bg(t.bg_sunken).child(self.preview.clone()))
                                    .child(self.splitter(Splitter::Right, cx))
                                    .child(div().w(px(self.right_w)).flex_none().h_full().child(self.inspector.clone().cached(full()))),
                            )
                            .child(self.splitter(Splitter::Timeline, cx))
                            .child(div().h(px(self.timeline_h)).flex_none().w_full().bg(t.bg_raised).child(self.timeline.clone())),
                    ))
                    .when(agent_open, |d| {
                        d.child(self.splitter(Splitter::Agent, cx)).child(motion::enter(
                            div().relative().w(px(self.agent_w)).flex_none().h_full().child(self.agent.clone().cached(full())),
                            "agent-in",
                            motion::BASE,
                            (24., 0.),
                        ))
                    }),
            )
            .when(resizing, |d| d.child(drag::track(cx.entity(), Self::resize_move, Self::resize_end)))
    }
}

/// Panels are cached views: playback redraws the preview and the playhead, not every panel.
fn full() -> gpui::StyleRefinement {
    gpui::StyleRefinement::default().size_full()
}
