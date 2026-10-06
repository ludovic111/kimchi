//! The root view: the lsuite backdrop, home or editor, and everything that
//! floats above them (dialogs, the context menu, toasts). It also carries out
//! the commands only the window can (`ui.*`, playback, selection) for every
//! client of the registry.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, ExternalPaths, FocusHandle, Focusable, MouseButton, PathPromptOptions, Render,
    Subscription, Window, div, prelude::*, px,
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
    cx.set_reduce_motion(crate::theme::os_reduces_motion());
    let playback = cx.new(|_| Playback::new(session.clone()));
    // The built-in agent answers `agent.*` for every client; the Agent panel draws it.
    let agent = kimchi_agent::Host::install(&session);
    let store = cx.new(|cx| Store::new(session, playback, agent, cx));
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

/// Quits and starts kimchi again (into an installed update, when there is one).
pub fn restart(cx: &mut App) {
    match kimchi_control::update::restart() {
        Ok(()) => cx.quit(),
        Err(e) => cx.store().update(cx, |s, cx| s.error(format!("Couldn't restart kimchi: {e}"), cx)),
    }
}

/// A new GitHub issue with the version, the system and the last crash's summary filled in
/// (nothing else: the person adds what they want to share).
pub fn issue_url(session: &Session) -> String {
    let crash = kimchi_control::diagnostics::reports(&session.data_dir).into_iter().next();
    let mut body = format!(
        "**What happened**\n\n\n**What you expected**\n\n\n**Steps to reproduce**\n1. \n\n---\nkimchi {} on {} ({})\n",
        kimchi_control::update::CURRENT,
        kimchi_control::diagnostics::os_name(),
        std::env::consts::ARCH
    );
    if let Some(c) = crash {
        body.push_str(&format!("Last crash report ({}): {}\n(Attach the file from Settings › Diagnostics if it's related.)\n", c.at.format("%Y-%m-%d %H:%M UTC"), c.summary));
    }
    format!("https://github.com/ludovic111/kimchi/issues/new?body={}", url_encode(&body))
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
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
    onboarding: Entity<views::onboarding::Onboarding>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    #[cfg(test)]
    pub fn dialogs(&self) -> Entity<views::dialogs::Dialogs> {
        self.dialogs.clone()
    }

    #[cfg(test)]
    pub fn onboarding(&self) -> Entity<views::onboarding::Onboarding> {
        self.onboarding.clone()
    }

    #[cfg(test)]
    pub fn editor(&self) -> Entity<views::editor::Editor> {
        self.editor.clone()
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let home = cx.new(|cx| views::home::Home::new(window, cx));
        let editor = cx.new(|cx| views::editor::Editor::new(window, cx));
        let dialogs = cx.new(|cx| views::dialogs::Dialogs::new(window, cx));
        let onboarding = cx.new(|cx| views::onboarding::Onboarding::new(window, cx));
        let mut subs = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        subs.push(cx.observe_window_appearance(window, |_, _, cx| apply_theme_setting(cx)));
        subs.push(cx.subscribe_in(&store, window, |ws: &mut Self, _, e: &StoreEvent, window, cx| match e {
            StoreEvent::FocusPrompt => window.refresh(),
            StoreEvent::SetupClosed => window.focus(&ws.focus, cx),
            _ => {}
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

        // Back from the Studio: the workspace takes the keyboard again.
        let studio = editor.read(cx).studio.clone();
        subs.push(cx.subscribe_in(&studio, window, |ws, _, e: &views::studio::StudioEvent, window, cx| match e {
            views::studio::StudioEvent::Closed => window.focus(&ws.focus, cx),
        }));
        // Back to the front: ryolune songs saved meanwhile are rendered again.
        subs.push(cx.observe_window_activation(window, |ws, window, cx| {
            if window.is_window_active() {
                ws.store.update(cx, views::mixer::refresh_songs);
            }
        }));
        store.update(cx, |s, cx| s.sync_ui(cx));
        Self { store, focus, home, editor, dialogs, onboarding, _subs: subs }
    }

    /// `ui.*`, `timeline.seek/play/pause`, `app.quit`, `app.restart`, `app.notify`.
    fn ui_command(&mut self, command: &str, params: Value, window: &mut Window, cx: &mut Context<Self>) -> CmdResult {
        let store = self.store.clone();
        let playback = store.read(cx).playback.clone();
        let ids = |v: &Value| -> Vec<kimchi_core::Id> { v.as_array().into_iter().flatten().filter_map(|s| s.as_str()?.parse().ok()).collect() };
        match command {
            "timeline.seek" => {
                let t = params["time"].as_f64().ok_or("time is required")?;
                playback.update(cx, |p, cx| if params["exact"].as_bool()==Some(true) {p.seek_exact(t,cx);} else {p.seek(t,cx);});
                Ok(json!({ "playhead": playback.read(cx).playhead }))
            }
            "timeline.play" => {
                if store.read(cx).project.is_none() {
                    return Err(kimchi_control::session::NO_PROJECT.into());
                }
                let speed = params["speed"].as_f64().unwrap_or(1.0);
                let from = playback.read(cx).playhead;
                playback.update(cx, |p, cx| p.play_at(speed, cx));
                Ok(json!({ "playing": true, "from": from, "speed": speed }))
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
            "ui.showPanel" if params["open"] == json!(false) => {
                let panel = params["panel"].as_str().unwrap_or("");
                if panel == "onboarding" {
                    store.update(cx, |s, cx| s.close_setup(cx));
                    return Ok(json!({ "panel": panel, "open": false }));
                }
                // The left panel closes to its rail (whichever tab is named); the inspector closes.
                if matches!(panel, "media" | "generate" | "text" | "motion" | "studio" | "captions" | "inspector") {
                    self.editor.update(cx, |e, cx| if panel == "inspector" { e.set_inspector_open(false, cx) } else { e.set_left_open(false, cx) });
                    return Ok(json!({ "panel": panel, "open": false }));
                }
                store.update(cx, |s, cx| match panel {
                    "agent" => s.set_agent_open(false, cx),
                    "jobs" => s.set_jobs_open(false, cx),
                    "settings" | "diagnostics" | "export" | "palette" | "shortcuts" | "whatsNew" => {
                        let name = if panel == "diagnostics" { "settings" } else { panel };
                        if s.dialog.as_ref().is_some_and(|d| d.name() == name) {
                            s.close_dialog(cx);
                        }
                    }
                    _ => {}
                });
                if !matches!(panel, "agent" | "jobs" | "settings" | "diagnostics" | "export" | "palette" | "shortcuts" | "whatsNew") {
                    return Err(format!("`{panel}` can't be closed: home is left by opening a project."));
                }
                Ok(json!({ "panel": panel, "open": false }))
            }
            "ui.showPanel" => {
                let panel = params["panel"].as_str().unwrap_or("");
                store.update(cx, |s, cx| match panel {
                    "media" => s.set_left_tab(LeftTab::Media, cx),
                    "generate" => s.set_left_tab(LeftTab::Generate, cx),
                    "text" => s.set_left_tab(LeftTab::Text, cx),
                    "motion" => s.set_left_tab(LeftTab::Motion, cx),
                    "studio" => s.set_left_tab(LeftTab::Studio, cx),
                    "captions" => s.set_left_tab(LeftTab::Captions, cx),
                    "agent" => s.set_agent_open(true, cx),
                    "jobs" => s.set_jobs_open(true, cx),
                    "settings" => s.open_dialog(Dialog::Settings { section: params["section"].as_str().map(str::to_string) }, cx),
                    "export" => s.open_dialog(Dialog::Export, cx),
                    "palette" => s.open_dialog(Dialog::Palette, cx),
                    "whatsNew" | "releaseNotes" => s.open_dialog(Dialog::WhatsNew { since: None, all: params["all"].as_bool().unwrap_or(false) }, cx),
                    "shortcuts" => s.open_dialog(Dialog::Shortcuts, cx),
                    "diagnostics" | "logs" => s.open_dialog(Dialog::Settings { section: Some("diagnostics".into()) }, cx),
                    "onboarding" => s.open_setup(params["section"].as_str().map(str::to_string), cx),
                    _ => {}
                });
                if panel == "inspector" {
                    self.editor.update(cx, |e, cx| e.set_inspector_open(true, cx));
                }
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
                if fit {
                    views::editor::Editor::fit_timeline(&self.editor, cx);
                } else if let Some(p) = pps {
                    store.update(cx, |s, cx| s.set_zoom(p, cx));
                }
                Ok(json!({ "pixelsPerSecond": store.read(cx).pps }))
            }
            "ui.setTimeline" => {
                if let Some(on) = params["snapping"].as_bool() {
                    store.update(cx, |s, cx| s.set_snapping(on, cx));
                }
                if let Some(on) = params["ripple"].as_bool() {
                    store.update(cx, |s, cx| s.set_ripple(on, cx));
                }
                if let Some(on) = params["loop"].as_bool() {
                    playback.update(cx, |p, cx| p.set_looping(on, cx));
                }
                let s = store.read(cx);
                Ok(json!({ "snapping": s.snapping, "ripple": s.ripple, "loop": playback.read(cx).looping }))
            }
            "ui.setLayout" => Ok(json!(self.editor.update(cx, |e, cx| e.set_layout(&params, cx)))),
            "ui.action" => {
                let name = params["action"].as_str().unwrap_or("");
                let action = cx.build_action(&format!("kimchi::{name}"), None).map_err(|e| format!("The window has no action `{name}`: {e}"))?;
                // As a key would: on what has focus, bubbling up to the workspace.
                window.dispatch_action(action, cx);
                Ok(json!({ "action": name }))
            }
            "ui.reveal" => {
                let path = params["path"].as_str().unwrap_or("");
                cx.reveal_path(std::path::Path::new(path));
                Ok(json!({ "revealed": path }))
            }
            "ui.screenshot" => views::screenshot::capture(params["path"].as_str(), window),
            c if c.starts_with("audio.") => views::mixer::ui_command(c, params, cx),
            "ui.studio" => {
                if store.read(cx).project.is_none() {
                    return Err(kimchi_control::session::NO_PROJECT.into());
                }
                let studio = self.editor.read(cx).studio.clone();
                let hide_panel=params["panel"].as_str()==Some("none");
                let state=studio.update(cx, |s, cx| s.ui_command(params, window, cx))?;
                if hide_panel {
                    self.editor.update(cx,|e,cx| e.set_left_open(false,cx));
                }
                Ok(state)
            }
            "app.quit" => {
                cx.quit();
                Ok(json!({ "quitting": true }))
            }
            "app.restart" => {
                kimchi_control::update::restart().map_err(|e| format!("Couldn't restart kimchi: {e}"))?;
                // Answer first: the bridge client is waiting for the reply.
                cx.defer(|cx| cx.quit());
                Ok(json!({ "restarting": true }))
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
            self.store.update(cx, |s, cx| undo_redo(s, true, cx));
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.store.read(cx).can_redo {
            self.store.update(cx, |s, cx| undo_redo(s, false, cx));
        }
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            self.store.update(cx, |s, cx| s.flash("Saved. kimchi saves every change as you make it.", cx));
        }
    }

    // ---- clipboard ------------------------------------------------------------

    fn copy(&mut self, _: &CopyClips, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    /// Keeps the selected clips as they are now; returns how many.
    fn copy_selection(&mut self, cx: &mut Context<Self>) -> usize {
        self.store.update(cx, |s, cx| {
            let Some(p) = s.project.clone() else { return 0 };
            let clips: Vec<(kimchi_core::Id, kimchi_core::Clip)> =
                p.tracks.iter().flat_map(|t| t.clips.iter().filter(|c| s.selection.contains(&c.id)).map(|c| (t.id, c.clone()))).collect();
            let n = clips.len();
            if n == 0 {
                return 0;
            }
            s.clipboard = crate::store::Clipboard { clips };
            s.flash(format!("Copied {}", count(n, "clip")), cx);
            n
        })
    }

    fn cut(&mut self, _: &CutClips, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.copy_selection(cx);
        if n > 0 {
            let ids = self.store.read(cx).selection.clone();
            self.store.update(cx, |s, cx| {
                s.run("clip.delete", json!({ "clipIds": ids }), cx);
                s.clear_selection(cx);
                s.flash(format!("Cut {}", count(n, "clip")), cx);
            });
        }
    }

    /// Pastes at the playhead, then moves the playhead past what landed, so pasting again
    /// lays the copies end to end.
    fn paste(&mut self, _: &PasteClips, _: &mut Window, cx: &mut Context<Self>) {
        if !self.has_project(cx) {
            return;
        }
        let (clips, t) = {
            let s = self.store.read(cx);
            (s.clipboard.clips.clone(), s.playback.read(cx).playhead)
        };
        if clips.is_empty() {
            self.store.update(cx, |s, cx| s.flash(format!("Nothing to paste. Select a clip and copy it ({}) first.", hint(&CopyClips).unwrap_or_default()), cx));
            return;
        }
        let list: Vec<Value> = clips
            .iter()
            .map(|(track, c)| {
                let mut v = json!(c);
                v["trackId"] = json!(track);
                v
            })
            .collect();
        let earliest = clips.iter().map(|(_, c)| c.start).fold(f64::INFINITY, f64::min);
        let span = clips.iter().map(|(_, c)| c.end()).fold(0.0, f64::max) - earliest;
        self.store.update(cx, |s, cx| {
            s.run_then("clip.paste", json!({ "clips": list, "time": t }), cx, move |s, v, cx| {
                s.set_selection(created(&v), cx);
                s.playback.clone().update(cx, |p, cx| p.seek(t + span, cx));
            })
        });
    }

    // ---- trims and nudges -----------------------------------------------------

    /// Q / W: the selected clips under the playhead (or every clip there on an unlocked
    /// track) get their start or end moved to it, as one undo step.
    fn trim_to_playhead(&mut self, start: bool, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let Some(p) = s.project.clone() else { return };
        let t = s.playback.read(cx).playhead;
        let inside = |c: &kimchi_core::Clip| t > c.start + 1e-6 && t < c.end() - 1e-6;
        let mut ids: Vec<kimchi_core::Id> = s.selected_clips().into_iter().filter(|c| inside(c)).map(|c| c.id).collect();
        if s.selection.is_empty() {
            ids = p.tracks.iter().filter(|tr| !tr.locked).flat_map(|tr| tr.clips.iter().filter(|c| inside(c)).map(|c| c.id)).collect();
        }
        if ids.is_empty() {
            self.store.update(cx, |s, cx| s.flash("Put the playhead over a clip to trim it there.", cx));
            return;
        }
        let edge = if start { "start" } else { "end" };
        let commands: Vec<Value> = ids.iter().map(|id| json!({ "command": "clip.trim", "params": { "clipId": id, "edge": edge, "time": t } })).collect();
        self.run("project.batch", json!({ "commands": commands, "label": "clip.trim" }), cx);
    }

    fn trim_start(&mut self, _: &TrimStart, _: &mut Window, cx: &mut Context<Self>) {
        self.trim_to_playhead(true, cx);
    }

    fn trim_end(&mut self, _: &TrimEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.trim_to_playhead(false, cx);
    }

    /// Moves the selection by whole frames (repeated nudges are one undo step).
    fn nudge(&mut self, frames: f64, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let Some(p) = s.project.clone() else { return };
        let dt = frames / s.fps();
        let moves: Vec<Value> = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter(|c| s.selection.contains(&c.id)).map(move |c| json!({ "clipId": c.id, "trackId": t.id, "start": (c.start + dt).max(0.) })))
            .collect();
        if !moves.is_empty() {
            self.run("clip.moveMany", json!({ "moves": moves, "coalesce": "nudge" }), cx);
        }
    }

    fn nudge_left(&mut self, _: &NudgeLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.nudge(-1.0, cx);
    }
    fn nudge_right(&mut self, _: &NudgeRight, _: &mut Window, cx: &mut Context<Self>) {
        self.nudge(1.0, cx);
    }
    fn nudge_left_more(&mut self, _: &NudgeLeftMore, _: &mut Window, cx: &mut Context<Self>) {
        self.nudge(-10.0, cx);
    }
    fn nudge_right_more(&mut self, _: &NudgeRightMore, _: &mut Window, cx: &mut Context<Self>) {
        self.nudge(10.0, cx);
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

    fn open_studio(&mut self, _: &OpenStudio, _: &mut Window, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let motion = s.selected_clips().into_iter().find(|c| matches!(c.content, kimchi_core::ClipContent::Motion { .. })).map(|c| c.id);
        self.store.update(cx, |s, cx| match motion {
            Some(id) => s.open_studio(id, cx),
            None => s.flash("Select a motion clip to open it in the Studio.", cx),
        });
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

    fn shuttle_back(&mut self, _: &ShuttleBack, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            let pb = self.store.read(cx).playback.clone();
            pb.update(cx, |p, cx| p.shuttle_back(cx));
        }
    }

    fn shuttle_stop(&mut self, _: &ShuttleStop, _: &mut Window, cx: &mut Context<Self>) {
        let pb = self.store.read(cx).playback.clone();
        pb.update(cx, |p, cx| p.pause(cx));
    }

    fn shuttle_forward(&mut self, _: &ShuttleForward, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            let pb = self.store.read(cx).playback.clone();
            pb.update(cx, |p, cx| p.shuttle_forward(cx));
        }
    }

    fn toggle_loop(&mut self, _: &ToggleLoop, _: &mut Window, cx: &mut Context<Self>) {
        let pb = self.store.read(cx).playback.clone();
        let on = pb.update(cx, |p, cx| {
            p.set_looping(!p.looping, cx);
            p.looping
        });
        self.store.update(cx, |s, cx| s.flash(if on { "Loop on" } else { "Loop off" }, cx));
    }

    /// Up / Down: the previous or next cut (any clip's start or end) or marker.
    fn jump_edit(&mut self, forward: bool, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let Some(p) = s.project.clone() else { return };
        let now = s.playback.read(cx).playhead;
        let half_frame = 0.5 / s.fps();
        let mut points: Vec<f64> = vec![0.0, p.duration()];
        points.extend(p.tracks.iter().flat_map(|t| t.clips.iter().flat_map(|c| [c.start, c.end()])));
        points.extend(p.markers.iter().map(|m| m.time));
        let target = if forward {
            points.into_iter().filter(|x| *x > now + half_frame).fold(f64::INFINITY, f64::min)
        } else {
            points.into_iter().filter(|x| *x < now - half_frame).fold(f64::NEG_INFINITY, f64::max)
        };
        if target.is_finite() {
            let pb = s.playback.clone();
            pb.update(cx, |p, cx| p.seek(target, cx));
        }
    }

    fn prev_edit(&mut self, _: &PrevEdit, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_edit(false, cx);
    }

    fn next_edit(&mut self, _: &NextEdit, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_edit(true, cx);
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
                // The media panel asks first, as its menu does: every clip using it goes too.
                s.set_left_tab(LeftTab::Media, cx);
                cx.emit(StoreEvent::AskRemoveAsset(a));
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
        let (menu, dialog) = {
            let s = self.store.read(cx);
            (s.menu.is_some(), s.dialog.is_some())
        };
        // A drawer over the work (narrow windows) goes before the selection does.
        if !menu && !dialog && self.has_project(cx) && self.editor.update(cx, |e, cx| e.close_drawers(cx)) {
            return;
        }
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
            s.set_snapping(!s.snapping, cx);
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

    fn show_tab(&mut self, tab: LeftTab, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            self.store.update(cx, |s, cx| s.set_left_tab(tab, cx));
        }
    }

    fn show_media(&mut self, _: &ShowMedia, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(LeftTab::Media, cx);
    }

    fn show_generate(&mut self, _: &ShowGenerate, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(LeftTab::Generate, cx);
    }

    fn show_motion(&mut self, _: &ShowMotion, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(LeftTab::Motion, cx);
    }

    fn show_text(&mut self, _: &ShowText, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(LeftTab::Text, cx);
    }

    fn show_captions(&mut self, _: &ShowCaptions, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(LeftTab::Captions, cx);
    }

    fn show_shortcuts(&mut self, _: &ShowShortcuts, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| if s.dialog == Some(Dialog::Shortcuts) { s.close_dialog(cx) } else { s.open_dialog(Dialog::Shortcuts, cx) });
    }

    fn toggle_left_panel(&mut self, _: &ToggleLeftPanel, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            self.editor.update(cx, |e, cx| e.toggle_left(cx));
        }
    }

    fn toggle_inspector(&mut self, _: &ToggleInspector, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_project(cx) {
            self.editor.update(cx, |e, cx| e.toggle_inspector(cx));
        }
    }

    fn toggle_agent(&mut self, _: &ToggleAgent, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.set_agent_open(!s.agent_open, cx));
    }

    fn toggle_jobs(&mut self, _: &ToggleJobs, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.set_jobs_open(!s.jobs_open, cx));
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
        self.store.update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("about".into()) }, cx));
    }

    fn whats_new(&mut self, _: &WhatsNew, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.open_dialog(Dialog::WhatsNew { since: None, all: false }, cx));
    }

    fn set_up(&mut self, _: &SetUpKimchi, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.open_setup(None, cx));
    }

    fn show_diagnostics(&mut self, _: &ShowDiagnostics, _: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("diagnostics".into()) }, cx));
    }

    fn report_problem(&mut self, _: &ReportProblem, _: &mut Window, cx: &mut Context<Self>) {
        let session = self.store.read(cx).session.clone();
        cx.open_url(&issue_url(&session));
    }

    fn restart_app(&mut self, _: &RestartApp, _: &mut Window, cx: &mut Context<Self>) {
        restart(cx);
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

    fn backdrop(&self, window: &Window, cx: &App) -> AnyElement {
        let t = cx.theme();
        // The page colour under grain and two corners of dithered light. With the native window
        // blur behind it, the page colour stays slightly translucent.
        let bg = if t.transparent { t.bg.opacity(if t.is_dark() { 0.95 } else { 0.93 }) } else { t.bg };
        crate::ui::grain::backdrop(bg, window, cx)
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

/// Undoes or redoes, then says what ("Undid split").
pub fn undo_redo(s: &mut Store, undo: bool, cx: &mut Context<Store>) {
    let name = if undo { "history.undo" } else { "history.redo" };
    s.run_then(name, json!({}), cx, move |s, v, cx| {
        let what = step_name(v["step"].as_str().unwrap_or(""));
        let whose = match v["source"].as_str() {
            Some("agent") => "the agent's ",
            Some("mcp" | "cli") => "a script's ",
            _ => "",
        };
        s.flash(format!("{} {whose}{what}", if undo { "Undid" } else { "Redid" }), cx);
    });
}

/// The command that made an undo step, in words.
pub fn step_name(command: &str) -> String {
    let words = match command {
        "clip.split" => "split",
        "clip.move" | "clip.moveMany" => "move",
        "clip.trim" => "trim",
        "clip.delete" => "delete",
        "clip.duplicate" => "duplicate",
        "clip.paste" => "paste",
        "clip.update" => "clip change",
        "clip.addText" => "new title",
        "clip.addSolid" => "new solid",
        "clip.insertMedia" | "media.import" => "media on the timeline",
        "media.remove" => "media removal",
        "track.add" => "new track",
        "track.remove" => "track deletion",
        "track.update" => "track change",
        "track.move" => "track move",
        "timeline.addMarker" => "new marker",
        "timeline.removeMarker" => "marker removal",
        "timeline.closeGap" => "closed gap",
        "project.rename" => "rename",
        "project.setSettings" => "canvas change",
        "project.batch" | "batch" => "batch of edits",
        c if c.starts_with("generate.") => "generation",
        "" => "last step",
        c => return c.rsplit('.').next().unwrap_or(c).to_string(),
    };
    words.to_string()
}

/// "1 clip", "3 clips".
pub fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
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
        // Files dragged in from the desktop (the flag outlives a drag that left the window).
        let dropping = store.dropping && cx.has_active_drag();
        let menu = store.menu.clone();
        let toasts = store.toasts.clone();
        // The setup takes the whole window: the editor's keys wait behind it like behind a dialog.
        let setup = store.setup.is_some();
        let modal = store.dialog.is_some() || setup;
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
            // `Modal` keeps editing keys (Space, Backspace, T…) off the project behind a dialog.
            .key_context(if modal { "Workspace Modal" } else { "Workspace" })
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::trim_start))
            .on_action(cx.listener(Self::trim_end))
            .on_action(cx.listener(Self::nudge_left))
            .on_action(cx.listener(Self::nudge_right))
            .on_action(cx.listener(Self::nudge_left_more))
            .on_action(cx.listener(Self::nudge_right_more))
            .on_action(cx.listener(Self::shuttle_back))
            .on_action(cx.listener(Self::shuttle_stop))
            .on_action(cx.listener(Self::shuttle_forward))
            .on_action(cx.listener(Self::toggle_loop))
            .on_action(cx.listener(Self::prev_edit))
            .on_action(cx.listener(Self::next_edit))
            .on_action(cx.listener(Self::show_media))
            .on_action(cx.listener(Self::show_generate))
            .on_action(cx.listener(Self::show_text))
            .on_action(cx.listener(Self::show_motion))
            .on_action(cx.listener(Self::show_captions))
            .on_action(cx.listener(Self::show_shortcuts))
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::export))
            .on_action(cx.listener(Self::palette))
            .on_action(cx.listener(Self::duplicate))
            .on_action(cx.listener(Self::open_studio))
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
            .on_action(cx.listener(Self::toggle_left_panel))
            .on_action(cx.listener(Self::toggle_inspector))
            .on_action(cx.listener(Self::toggle_jobs))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::close_project))
            .on_action(cx.listener(Self::check_updates))
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::whats_new))
            .on_action(cx.listener(Self::set_up))
            .on_action(cx.listener(Self::show_diagnostics))
            .on_action(cx.listener(Self::report_problem))
            .on_action(cx.listener(Self::restart_app))
            .on_action(cx.listener(Self::help))
            .on_action(cx.listener(Self::support))
            .map(views::mixer::on_actions)
            .on_drop(cx.listener(Self::on_drop_paths))
            .on_drag_move::<ExternalPaths>(cx.listener(|ws, _, _, cx| {
                ws.store.update(cx, |s, cx| {
                    if !s.dropping && s.project.is_some() {
                        s.dropping = true;
                        cx.notify();
                    }
                })
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, cx| {
                ws.store.update(cx, |s, cx| s.close_menu(cx));
                // Clicking anywhere but a text field (which stops the event) takes focus back,
                // so single-key shortcuts work again; unless what was clicked took focus itself
                // (a track name being renamed, a slider) and said so with `prevent_default`.
                if !window.default_prevented() {
                    window.focus(&ws.focus, cx);
                }
            }))
            .relative()
            .size_full()
            .font_family(crate::theme::SANS)
            .text_size(px(sz::BASE))
            .text_color(t.text)
            .child(self.backdrop(window, cx))
            .child(if setup {
                self.onboarding.clone().into_any_element()
            } else if has_project {
                self.editor.clone().into_any_element()
            } else {
                self.home.clone().into_any_element()
            })
            .child(self.dialogs.clone())
            .when_some(menu, |d, m| d.child(views::overlays::context_menu(m, window, cx)))
            .child(views::overlays::toasts(toasts, cx))
            // An outline and a hint; the timeline shows where files dropped on a track will land.
            .when(dropping, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset(px(6.))
                        .rounded(px(sz::R_XL))
                        .border_2()
                        .border_dashed()
                        .border_color(t.accent)
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .mt(px(64.))
                                .h(px(36.))
                                .glass(t.glass2)
                                .shadow(t.glass_shadow())
                                .px(px(16.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(icon("import").text_color(t.accent_text))
                                .child("Drop to import")
                                .child(div().text_color(t.text_2).child("· on a track to place it there")),
                        ),
                )
            })
    }
}
