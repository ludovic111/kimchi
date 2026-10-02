//! The root view: the lsuite backdrop, home or editor, and everything that
//! floats above them (dialogs, the context menu, toasts). It also carries out
//! the commands only the window can (`ui.*`, playback, selection) for every
//! client of the registry.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, BoxShadow, Context, Entity, ExternalPaths, FocusHandle, Focusable, Hsla, MouseButton, PathPromptOptions, Render,
    Subscription, Window, div, point, prelude::*, px,
};
use kimchi_control::{CmdResult, Session, ToastKind, UiCall};
use serde_json::{Value, json};

use crate::actions::*;
use crate::playback::Playback;
use crate::store::{Dialog, GlobalStore, LeftTab, MAX_PPS, MIN_PPS, Store, StoreEvent, StoreExt};
use crate::theme::{ActiveTheme, Theme, os_reduces_transparency, size as sz};
use crate::ui::{GlassExt, icon};
use crate::views;

pub const SUPPORT_URL: &str = "https://lsuite.xyz/kimchi/support";
pub const HELP_URL: &str = "https://github.com/ludovic111/kimchi/blob/main/docs/AI_CONTROL.md";

pub fn init(session: Arc<Session>, cx: &mut App) {
    let settings = session.settings();
    let mode = Theme::mode_for(&settings.appearance.mode, cx.window_appearance());
    cx.set_global(Theme::new(mode, settings.appearance.transparency && !os_reduces_transparency()));
    let playback = cx.new(|_| Playback::new(session.clone()));
    let store = cx.new(|cx| Store::new(session, playback, cx));
    cx.set_global(GlobalStore(store));
    crate::actions::bind(cx);
    cx.set_menus(crate::actions::menus());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// Re-reads the appearance setting (and the OS) into the theme.
pub fn apply_theme_setting(cx: &mut App) {
    let settings = cx.store().read(cx).settings.clone();
    let mode = Theme::mode_for(&settings.appearance.mode, cx.window_appearance());
    let transparent = settings.appearance.transparency && !os_reduces_transparency();
    let t = cx.global::<Theme>();
    if t.mode != mode || t.transparent != transparent {
        cx.set_global(Theme::new(mode, transparent));
        cx.refresh_windows();
    }
}

pub struct Workspace {
    store: Entity<Store>,
    focus: FocusHandle,
    home: Entity<views::home::Home>,
    editor: Entity<views::editor::Editor>,
    dialogs: Entity<views::dialogs::Dialogs>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let home = cx.new(|cx| views::home::Home::new(window, cx));
        let editor = cx.new(|cx| views::editor::Editor::new(window, cx));
        let dialogs = cx.new(|cx| views::dialogs::Dialogs::new(window, cx));
        let mut subs = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        subs.push(cx.observe_window_appearance(window, |_, _, cx| apply_theme_setting(cx)));
        subs.push(cx.subscribe_in(&store, window, |_, _, e: &StoreEvent, window, cx| {
            if let StoreEvent::FocusPrompt = e {
                window.refresh();
                let _ = cx;
            }
        }));

        // Commands only the window can do, from every client of the registry.
        let mut calls = store.read(cx).session.attach_ui();
        cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while let Some(call) = calls.next().await {
                let UiCall { command, params, reply } = call;
                let result = this.update_in(cx, |ws, window, cx| ws.ui_command(&command, params, window, cx)).unwrap_or_else(|_| Err("the window has closed".into()));
                let _ = reply.send(result);
            }
        })
        .detach();

        store.update(cx, |s, cx| s.sync_ui(cx));
        Self { store, focus, home, editor, dialogs, _subs: subs }
    }

    /// `ui.*`, `timeline.seek/play/pause`, `app.quit`, `app.notify`.
    fn ui_command(&mut self, command: &str, params: Value, window: &mut Window, cx: &mut Context<Self>) -> CmdResult {
        let store = self.store.clone();
        let playback = store.read(cx).playback.clone();
        let ids = |v: &Value| -> Vec<kimchi_core::Id> { v.as_array().into_iter().flatten().filter_map(|s| s.as_str()?.parse().ok()).collect() };
        match command {
            "timeline.seek" => {
                let t = params["time"].as_f64().ok_or("time is required")?;
                playback.update(cx, |p, cx| p.seek(t, cx));
                Ok(json!({ "playhead": playback.read(cx).playhead }))
            }
            "timeline.play" => {
                if store.read(cx).project.is_none() {
                    return Err(kimchi_control::session::NO_PROJECT.into());
                }
                playback.update(cx, |p, cx| p.play(cx));
                Ok(json!({ "playing": true, "from": playback.read(cx).playhead }))
            }
            "timeline.pause" => {
                playback.update(cx, |p, cx| p.pause(cx));
                Ok(json!({ "playing": false, "playhead": playback.read(cx).playhead }))
            }
            "ui.select" => {
                store.update(cx, |s, cx| {
                    if let Some(a) = params.get("assetId").and_then(Value::as_str).and_then(|a| a.parse().ok()) {
                        s.select_asset(Some(a), cx);
                    } else {
                        s.set_selection(ids(&params["clipIds"]), cx);
                    }
                });
                let s = store.read(cx);
                Ok(json!({ "selection": s.selection, "selectedAsset": s.selected_asset }))
            }
            "ui.showPanel" => {
                let panel = params["panel"].as_str().unwrap_or("");
                store.update(cx, |s, cx| match panel {
                    "media" => s.set_left_tab(LeftTab::Media, cx),
                    "generate" => s.set_left_tab(LeftTab::Generate, cx),
                    "text" => s.set_left_tab(LeftTab::Text, cx),
                    "agent" => {
                        s.agent_open = true;
                        cx.notify();
                    }
                    "jobs" => {
                        s.jobs_open = true;
                        cx.notify();
                    }
                    "settings" => s.open_dialog(Dialog::Settings { section: None }, cx),
                    "export" => s.open_dialog(Dialog::Export, cx),
                    "palette" => s.open_dialog(Dialog::Palette, cx),
                    _ => {}
                });
                if panel == "home" {
                    store.update(cx, |s, cx| s.run("project.close", json!({}), cx));
                }
                store.update(cx, |s, cx| s.sync_ui(cx));
                Ok(json!({ "panel": panel }))
            }
            "ui.closeDialogs" => {
                store.update(cx, |s, cx| {
                    s.dialog = None;
                    s.menu = None;
                    s.jobs_open = false;
                    s.sync_ui(cx);
                    cx.notify();
                });
                Ok(json!({ "closed": true }))
            }
            "ui.zoom" => {
                let fit = params["fit"].as_bool().unwrap_or(false);
                let pps = params["pixelsPerSecond"].as_f64();
                store.update(cx, |s, cx| {
                    if fit {
                        let w = self.editor.read(cx).timeline_width(cx);
                        let d = s.duration().max(1.0);
                        s.set_zoom(f64::from(w) / d * 0.92, cx);
                    } else if let Some(p) = pps {
                        s.set_zoom(p, cx);
                    }
                });
                Ok(json!({ "pixelsPerSecond": store.read(cx).pps }))
            }
            "ui.screenshot" => views::screenshot::capture(params["path"].as_str(), window),
            "app.quit" => {
                cx.quit();
                Ok(json!({ "quitting": true }))
            }
            "app.notify" => {
                let kind = match params["kind"].as_str() {
                    Some("error") => ToastKind::Error,
                    Some("success") => ToastKind::Success,
                    _ => ToastKind::Info,
                };
                let text = params["text"].as_str().unwrap_or("").to_string();
                store.update(cx, |s, cx| s.toast(kind, text, cx));
                Ok(json!({ "shown": true }))
            }
            other => Err(format!("the window doesn't handle `{other}`")),
        }
    }

    // ---- actions ------------------------------------------------------------

    fn run(&self, name: &str, params: Value, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.run(name, params, cx));
    }

    fn has_project(&self, cx: &App) -> bool {
        self.store.read(cx).project.is_some()
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.store.read(cx).can_undo {
            self.run("history.undo", json!({}), cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.store.read(cx).can_redo {
            self.run("history.redo", json!({}), cx);
        }
    }

    fn import(&mut self, _: &Import, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            import_dialog(cx);
        }
    }

    fn export(&mut self, _: &Export, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            self.store.update(cx, |s, cx| s.open_dialog(Dialog::Export, cx));
        }
    }

    fn palette(&mut self, _: &Palette, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| if s.dialog == Some(Dialog::Palette) { s.close_dialog(cx) } else { s.open_dialog(Dialog::Palette, cx) });
    }

    fn duplicate(&mut self, _: &Duplicate, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.store.read(cx).selection.clone();
        if ids.is_empty() {
            return;
        }
        self.store.update(cx, |s, cx| {
            s.run_then("clip.duplicate", json!({ "clipIds": ids }), cx, |s, v, cx| s.set_selection(created(&v), cx));
        });
    }

    fn split(&mut self, _: &Split, _: &mut Window, cx: &mut Context<Self>) {
        let (ids, t) = {
            let s = self.store.read(cx);
            (s.selection.clone(), s.playback.read(cx).playhead)
        };
        if !self.has_project(cx) {
            return;
        }
        self.store.update(cx, |s, cx| {
            let params = if ids.is_empty() { json!({ "time": t }) } else { json!({ "time": t, "clipIds": ids }) };
            s.run_then("clip.split", params, cx, move |s, v, cx| {
                if !ids.is_empty() {
                    let mut sel = ids.clone();
                    sel.extend(v["created"].as_array().into_iter().flatten().filter_map(|x| x.as_str()?.parse::<kimchi_core::Id>().ok()));
                    s.set_selection(sel, cx);
                }
            });
        });
    }

    fn focus_generate(&mut self, _: &FocusGenerate, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.set_left_tab(LeftTab::Generate, cx);
            cx.emit(StoreEvent::FocusPrompt);
        });
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            let all = s.project.as_ref().map(|p| p.clips().map(|(_, c)| c.id).collect()).unwrap_or_default();
            s.set_selection(all, cx);
        });
    }

    fn play_pause(&mut self, _: &PlayPause, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            let pb = self.store.read(cx).playback.clone();
            pb.update(cx, |p, cx| p.toggle(cx));
        }
    }

    fn step(&mut self, frames: f64, cx: &mut Context<Self>) {
        let pb = self.store.read(cx).playback.clone();
        pb.update(cx, |p, cx| p.step(frames, cx));
    }

    fn step_back(&mut self, _: &StepBack, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1.0, cx);
    }
    fn step_forward(&mut self, _: &StepForward, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1.0, cx);
    }
    fn step_back_second(&mut self, _: &StepBackSecond, _: &mut Window, cx: &mut Context<Self>) {
        let fps = self.store.read(cx).fps();
        self.step(-fps, cx);
    }
    fn step_forward_second(&mut self, _: &StepForwardSecond, _: &mut Window, cx: &mut Context<Self>) {
        let fps = self.store.read(cx).fps();
        self.step(fps, cx);
    }

    fn go_start(&mut self, _: &GoToStart, _: &mut Window, cx: &mut Context<Self>) {
        let pb = self.store.read(cx).playback.clone();
        pb.update(cx, |p, cx| p.seek(0.0, cx));
    }

    fn go_end(&mut self, _: &GoToEnd, _: &mut Window, cx: &mut Context<Self>) {
        let d = self.store.read(cx).duration();
        let pb = self.store.read(cx).playback.clone();
        pb.update(cx, |p, cx| p.seek(d, cx));
    }

    fn delete_selection(&mut self, ripple: bool, cx: &mut Context<Self>) {
        let (ids, asset) = {
            let s = self.store.read(cx);
            (s.selection.clone(), s.selected_asset)
        };
        let ripple = ripple || self.store.read(cx).ripple;
        self.store.update(cx, |s, cx| {
            if !ids.is_empty() {
                s.run("clip.delete", json!({ "clipIds": ids, "ripple": ripple }), cx);
                s.clear_selection(cx);
            } else if let Some(a) = asset {
                s.run("media.remove", json!({ "assetId": a }), cx);
                s.select_asset(None, cx);
            }
        });
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection(false, cx);
    }

    fn ripple_delete(&mut self, _: &RippleDelete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection(true, cx);
    }

    fn deselect(&mut self, _: &Deselect, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            if s.menu.is_some() {
                s.close_menu(cx);
            } else if s.dialog.is_some() {
                s.close_dialog(cx);
            } else {
                s.clear_selection(cx);
                s.select_asset(None, cx);
            }
        });
    }

    fn toggle_snap(&mut self, _: &ToggleSnap, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.snapping = !s.snapping;
            let msg = if s.snapping { "Snapping on" } else { "Snapping off" };
            s.info(msg, cx);
        });
    }

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.set_zoom((s.pps * 1.25).min(MAX_PPS), cx));
    }

    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.set_zoom((s.pps / 1.25).max(MIN_PPS), cx));
    }

    fn zoom_fit(&mut self, _: &ZoomFit, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.ui_command("ui.zoom", json!({ "fit": true }), window, cx);
    }

    fn add_marker(&mut self, _: &AddMarker, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            let t = self.store.read(cx).playback.read(cx).playhead;
            self.run("timeline.addMarker", json!({ "time": t }), cx);
        }
    }

    fn add_text(&mut self, _: &AddText, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            add_text(json!({}), cx);
        }
    }

    fn toggle_agent(&mut self, _: &ToggleAgent, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.agent_open = !s.agent_open;
            s.sync_ui(cx);
            cx.notify();
        });
    }

    fn toggle_jobs(&mut self, _: &ToggleJobs, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.jobs_open = !s.jobs_open;
            s.sync_ui(cx);
            cx.notify();
        });
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _: &mut Window, cx: &mut Context<Self>) {
        let next = if cx.theme().is_dark() { "light" } else { "dark" };
        self.run("app.setSetting", json!({ "key": "appearance.mode", "value": next }), cx);
    }

    fn open_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: None }, cx));
    }

    fn new_project(&mut self, _: &NewProject, _: &mut Window, cx: &mut Context<Self>) {
        self.run("project.create", json!({ "name": "Untitled" }), cx);
    }

    fn close_project(&mut self, _: &CloseProject, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            self.run("project.close", json!({}), cx);
        }
    }

    fn check_updates(&mut self, _: &CheckUpdates, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.run_then("app.checkUpdates", json!({}), cx, |s, v, cx| match v["available"].as_str() {
                Some(version) => s.info(format!("kimchi {version} is available."), cx),
                None => s.toast(ToastKind::Success, "kimchi is up to date.", cx),
            })
        });
    }

    fn about(&mut self, _: &About, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.info(format!("kimchi {} — part of lsuite. MIT licensed.", env!("CARGO_PKG_VERSION")), cx));
    }

    fn help(&mut self, _: &OpenHelp, _: &mut Window, cx: &mut Context<Self>) {
        cx.open_url(HELP_URL);
    }

    fn support(&mut self, _: &OpenSupport, _: &mut Window, cx: &mut Context<Self>) {
        cx.open_url(SUPPORT_URL);
    }

    fn on_drop_paths(&mut self, paths: &ExternalPaths, _: &mut Window, cx: &mut Context<Self>) {
        let list: Vec<String> = paths.paths().iter().map(|p| p.to_string_lossy().into_owned()).collect();
        self.store.update(cx, |s, cx| {
            s.dropping = false;
            if list.is_empty() || s.project.is_none() {
                cx.notify();
                return;
            }
            s.run_then("media.import", json!({ "paths": list }), cx, |s, v, cx| {
                let n = v["media"].as_array().map(Vec::len).unwrap_or(0);
                s.toast(ToastKind::Success, format!("Imported {n} file{}", if n == 1 { "" } else { "s" }), cx);
                s.set_left_tab(LeftTab::Media, cx);
            });
        });
    }

    fn backdrop(&self, cx: &App) -> AnyElement {
        let t = cx.theme();
        let glow = |c: Hsla, x: f32, y: f32, r: f32| {
            div().absolute().left(px(x)).top(px(y)).size(px(r)).rounded_full().shadow(vec![BoxShadow {
                color: c.opacity(t.aurora_strength),
                offset: point(px(0.), px(0.)),
                blur_radius: px(r * 0.9),
                spread_radius: px(r * 0.25),
                inset: false,
            }])
        };
        // `.ls-backdrop`: the page colour with two soft glows of kimchi's colour. With the native
        // window blur behind it, the page colour stays slightly translucent.
        let bg = if t.transparent { t.bg.opacity(if t.is_dark() { 0.95 } else { 0.93 }) } else { t.bg };
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(bg)
            .child(glow(t.aurora_a, -180., -220., 520.))
            .child(glow(t.aurora_b, 1100., 520., 460.))
            .into_any_element()
    }
}

/// Picks media files and imports them into the open project.
pub fn import_dialog(cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Import".into()) });
    let store = cx.store();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else { return };
        let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        store
            .update(cx, |s, cx| {
                s.run_then("media.import", json!({ "paths": list }), cx, |s, v, cx| {
                    let n = v["media"].as_array().map(Vec::len).unwrap_or(0);
                    s.toast(ToastKind::Success, format!("Imported {n} file{}", if n == 1 { "" } else { "s" }), cx);
                    s.set_left_tab(LeftTab::Media, cx);
                })
            });
    })
    .detach();
}

/// Adds a title at the playhead (with `style` overrides) and selects it.
pub fn add_text(style: Value, cx: &mut App) {
    let store = cx.store();
    store.update(cx, |s, cx| {
        let t = s.playback.read(cx).playhead;
        let text = style.get("content").and_then(Value::as_str).unwrap_or("Your title").to_string();
        let y = style.get("y").and_then(Value::as_f64).unwrap_or(0.0);
        let mut st = style.clone();
        if let Some(o) = st.as_object_mut() {
            o.remove("content");
            o.remove("y");
        }
        s.run_then("clip.addText", json!({ "text": text, "start": t, "style": st, "y": y }), cx, |s, v, cx| s.set_selection(created(&v), cx));
    });
}

/// Clip ids in a `{ "clips": [...] }` result.
pub fn created(v: &Value) -> Vec<kimchi_core::Id> {
    v["clips"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_str()?.parse().ok()).collect()
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let store = self.store.read(cx);
        let has_project = store.project.is_some();
        let dropping = store.dropping;
        let menu = store.menu.clone();
        let toasts = store.toasts.clone();
        let pb = store.playback.clone();
        let project = store.project.as_ref().map(|p| (p.duration(), p.settings.fps, p.name.clone()));
        match project {
            Some((d, fps, name)) => {
                pb.update(cx, |p, _| p.set_project(d, fps));
                window.set_window_title(&format!("{name} — kimchi"));
            }
            None => window.set_window_title("kimchi"),
        }

        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::export))
            .on_action(cx.listener(Self::palette))
            .on_action(cx.listener(Self::duplicate))
            .on_action(cx.listener(Self::split))
            .on_action(cx.listener(Self::focus_generate))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::play_pause))
            .on_action(cx.listener(Self::step_back))
            .on_action(cx.listener(Self::step_forward))
            .on_action(cx.listener(Self::step_back_second))
            .on_action(cx.listener(Self::step_forward_second))
            .on_action(cx.listener(Self::go_start))
            .on_action(cx.listener(Self::go_end))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::ripple_delete))
            .on_action(cx.listener(Self::deselect))
            .on_action(cx.listener(Self::toggle_snap))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_fit))
            .on_action(cx.listener(Self::add_marker))
            .on_action(cx.listener(Self::add_text))
            .on_action(cx.listener(Self::toggle_agent))
            .on_action(cx.listener(Self::toggle_jobs))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::close_project))
            .on_action(cx.listener(Self::check_updates))
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::help))
            .on_action(cx.listener(Self::support))
            .on_drop(cx.listener(Self::on_drop_paths))
            .drag_over::<ExternalPaths>(|s, _, _, _| s)
            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| {
                ws.store.update(cx, |s, cx| s.close_menu(cx));
                // Clicking empty space takes focus back from text fields.
                if !window.focused(cx).is_some_and(|f| f != ws.focus) {
                    window.focus(&ws.focus, cx);
                }
            }))
            .relative()
            .size_full()
            .font_family(crate::theme::SANS)
            .text_size(px(sz::BASE))
            .text_color(t.text)
            .child(self.backdrop(cx))
            .child(if has_project { self.editor.clone().into_any_element() } else { self.home.clone().into_any_element() })
            .child(self.dialogs.clone())
            .when_some(menu, |d, m| d.child(views::overlays::context_menu(m, cx)))
            .child(views::overlays::toasts(toasts, cx))
            .when(dropping, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset(px(8.))
                        .rounded(px(sz::R_XL))
                        .border_2()
                        .border_dashed()
                        .border_color(t.accent)
                        .bg(t.accent_soft)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().glass(t.glass2).rounded_full().px(px(16.)).py(px(10.)).flex().gap(px(8.)).child(icon("import")).child("Drop to import")),
                )
            })
    }
}
