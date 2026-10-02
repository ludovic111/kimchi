//! The real window in GPUI's headless test platform: a session with a project,
//! the workspace as the root view, and simulated keystrokes. Commands run on
//! Tokio as in the app, so the checks wait for the session to settle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{TestAppContext, VisualTestContext};
use kimchi_control::{Session, SessionOptions, Source};
use kimchi_core::{ClipContent, Project};
use serde_json::{Value, json};

use crate::app::Workspace;
use crate::store::{Dialog, StoreExt};

struct Fixture {
    rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
    session: Arc<Session>,
}

impl Fixture {
    fn call(&self, name: &str, params: Value) -> Value {
        self.rt.block_on(kimchi_control::call(&self.session, Source::Cli, name, params)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn project(&self) -> Project {
        self.session.read(|ed| ed.project().clone()).unwrap()
    }

    /// Lets the window and Tokio work until `done` holds (or 3 s pass).
    fn settle(&self, cx: &mut VisualTestContext, done: impl Fn(&Project) -> bool) -> Project {
        let start = Instant::now();
        loop {
            cx.run_until_parked();
            let p = self.project();
            if done(&p) || start.elapsed() > Duration::from_secs(3) {
                return p;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The platform's command key, as the shortcuts are bound (`actions.rs`).
#[cfg(target_os = "macos")]
const M: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const M: &str = "ctrl";

fn texts(p: &Project) -> usize {
    p.clips().filter(|(_, c)| matches!(c.content, ClipContent::Text { .. })).count()
}

fn setup(cx: &mut TestAppContext) -> (Fixture, gpui::Entity<Workspace>, &mut VisualTestContext) {
    cx.executor().allow_parking();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let session = {
        let _g = rt.enter();
        Session::new(SessionOptions { data_dir: Some(dir.path().join("data")), config_dir: Some(dir.path().join("config")), secrets: None, headless: false }).unwrap()
    };
    let f = Fixture { rt, _dir: dir, session };
    f.call("project.create", json!({ "name": "Keys" }));
    f.call("clip.addSolid", json!({ "color": "#202020", "start": 0, "duration": 4 }));
    let (handle, session) = (f.rt.handle().clone(), f.session.clone());
    cx.update(|cx| {
        gpui_tokio::init_from_handle(cx, handle);
        crate::app::init(session, cx);
    });
    let (view, vcx) = cx.add_window_view(Workspace::new);
    vcx.run_until_parked();
    (f, view, vcx)
}

#[gpui::test]
fn shortcuts_edit_through_the_registry_and_undo(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    cx.simulate_keystrokes("t");
    let p = f.settle(cx, |p| texts(p) == 1);
    assert_eq!(texts(&p), 1, "t adds a title");
    // The window's edits are undo steps like any other client's.
    cx.simulate_keystrokes(&format!("{M}-z"));
    let p = f.settle(cx, |p| texts(p) == 0);
    assert_eq!(texts(&p), 0, "undo undoes it");
    // Redo: shift-cmd-z on macOS, ctrl-y elsewhere (both are bound).
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-shift-z" } else { "ctrl-y" });
    let p = f.settle(cx, |p| texts(p) == 1);
    assert_eq!(texts(&p), 1, "redo redoes it");
    let steps = f.call("history.list", json!({}));
    assert_eq!(steps["undo"][0]["source"], "window");
}

#[gpui::test]
fn space_plays_and_pauses(cx: &mut TestAppContext) {
    let (_f, _, cx) = setup(cx);
    let playing = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.store().read(cx).playback.read(cx).playing);
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    assert!(playing(cx));
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    assert!(!playing(cx));
}

#[gpui::test]
fn typing_in_a_field_never_triggers_shortcuts(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    cx.simulate_keystrokes(&format!("{M}-k"));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).dialog.clone()), Some(Dialog::Palette));
    // "t" adds a title and space plays, except while a text field has focus.
    cx.simulate_input("t s");
    cx.run_until_parked();
    let palette = cx.update(|_, cx| view.read(cx).dialogs().read(cx).palette());
    assert_eq!(cx.update(|_, cx| palette.read(cx).query().to_string()), "t s");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(texts(&f.settle(cx, |_| false)), 0);
    assert!(!cx.update(|_, cx| cx.store().read(cx).playback.read(cx).playing));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).dialog.clone()), None);
}

#[gpui::test]
fn window_commands_from_other_clients_reach_the_window(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    let s = f.session.clone();
    let task = f.rt.spawn(async move { kimchi_control::call(&s, Source::Mcp, "ui.showPanel", json!({ "panel": "settings", "section": "agent" })).await });
    let start = Instant::now();
    while !task.is_finished() && start.elapsed() < Duration::from_secs(3) {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    f.rt.block_on(task).unwrap().unwrap();
    let dialog = cx.update(|_, cx| cx.store().read(cx).dialog.clone());
    assert_eq!(dialog, Some(Dialog::Settings { section: Some("agent".into()) }));
    let state = f.call("ui.state", json!({}));
    assert!(state["open"].as_array().unwrap().iter().any(|o| o == "settings"), "{state}");
}
