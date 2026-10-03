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

pub(crate) struct Fixture {
    pub rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
    pub session: Arc<Session>,
}

impl Fixture {
    pub fn call(&self, name: &str, params: Value) -> Value {
        self.rt.block_on(kimchi_control::call(&self.session, Source::Cli, name, params)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    pub fn project(&self) -> Project {
        self.session.read(|ed| ed.project().clone()).unwrap()
    }

    /// Lets the window and Tokio work until `done` holds (or 3 s pass).
    pub fn settle(&self, cx: &mut VisualTestContext, done: impl Fn(&Project) -> bool) -> Project {
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

/// Lets the window catch up until its store satisfies `done` (or 3 s pass): the store hears
/// about changes through the session's events, a moment after the session has them.
pub(crate) fn store_settles(cx: &mut VisualTestContext, done: impl Fn(&crate::store::Store) -> bool) {
    let start = Instant::now();
    while !cx.update(|_, cx| done(cx.store().read(cx))) && start.elapsed() < Duration::from_secs(3) {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn texts(p: &Project) -> usize {
    p.clips().filter(|(_, c)| matches!(c.content, ClipContent::Text { .. })).count()
}

pub(crate) fn setup(cx: &mut TestAppContext) -> (Fixture, gpui::Entity<Workspace>, &mut VisualTestContext) {
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
    store_settles(cx, |s| s.can_undo);
    cx.simulate_keystrokes(&format!("{M}-z"));
    let p = f.settle(cx, |p| texts(p) == 0);
    assert_eq!(texts(&p), 0, "undo undoes it");
    // Redo: shift-cmd-z on macOS, ctrl-y elsewhere (both are bound).
    store_settles(cx, |s| s.can_redo);
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

/// A person makes a motion clip from the Motion panel and edits its words in the inspector.
#[gpui::test]
fn people_edit_motion_scenes_in_the_inspector(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let motion = |p: &Project| p.clips().find(|(_, c)| matches!(c.content, ClipContent::Motion { .. })).map(|(_, c)| c.clone());
    // "New 2D scene" in the Motion panel: a clip at the playhead, selected.
    cx.update(|_, cx| crate::views::motion_panel::new_scene(false, cx));
    let p = f.settle(cx, |p| motion(p).is_some());
    let clip = motion(&p).expect("a motion clip");
    store_settles(cx, |s| s.selection == vec![clip.id]);
    // It opens in the Studio; back to the edit for the inspector.
    store_settles(cx, |s| s.studio.is_some());
    let studio = cx.update(|_, cx| view.read(cx).editor().read(cx).studio.clone());
    cx.update(|_, cx| studio.update(cx, |s, cx| s.close(cx)));
    cx.run_until_parked();
    // Pick its text layer, then type in the words field.
    let inspector = cx.update(|_, cx| view.read(cx).editor().read(cx).inspector.clone());
    cx.update(|_, cx| inspector.update(cx, |i, cx| i.pick(clip.id, "text1", cx)));
    let start = Instant::now();
    let field = loop {
        cx.run_until_parked();
        if let Some(f) = cx.update(|_, cx| inspector.read(cx).words_field()) {
            break f;
        }
        assert!(start.elapsed() < Duration::from_secs(3), "the words field shows");
        std::thread::sleep(Duration::from_millis(10));
    };
    cx.update(|_, cx| field.update(cx, |_, cx| cx.emit(crate::ui::input::InputEvent::Changed("Bonjour".into()))));
    let words = |p: &Project| match motion(p).map(|c| c.content) {
        Some(ClipContent::Motion { scene: kimchi_core::Scene::Flat(s), .. }) => match &s.layers[0].kind {
            kimchi_core::motion::LayerKind::Text(t) => t.text.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let p = f.settle(cx, |p| words(p) == "Bonjour");
    assert_eq!(words(&p), "Bonjour");
    let steps = f.call("history.list", json!({}));
    assert_eq!((steps["undo"][0]["label"].as_str(), steps["undo"][0]["source"].as_str()), (Some("motion.updateLayer"), Some("window")), "{steps}");
}

fn playhead(cx: &mut VisualTestContext) -> f64 {
    cx.update(|_, cx| cx.store().read(cx).playback.read(cx).playhead)
}

fn seek(cx: &mut VisualTestContext, t: f64) {
    cx.update(|_, cx| {
        let pb = cx.store().read(cx).playback.clone();
        pb.update(cx, |p, cx| p.seek(t, cx));
    });
    cx.run_until_parked();
}

/// Selects the project's only clip, as a click on it would.
fn select_the_clip(f: &Fixture, cx: &mut VisualTestContext) -> kimchi_core::Id {
    let id = f.project().clips().next().unwrap().1.id;
    cx.update(|_, cx| cx.store().update(cx, |s, cx| s.set_selection(vec![id], cx)));
    cx.run_until_parked();
    id
}

#[gpui::test]
fn copy_and_paste_land_at_the_playhead_end_to_end(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    let original = select_the_clip(&f, cx);
    cx.simulate_keystrokes(&format!("{M}-c"));
    seek(cx, 10.);
    cx.simulate_keystrokes(&format!("{M}-v"));
    let p = f.settle(cx, |p| p.clips().count() == 2);
    let mut starts: Vec<f64> = p.clips().map(|(_, c)| c.start).collect();
    starts.sort_by(f64::total_cmp);
    assert_eq!(starts, vec![0., 10.]);
    // The playhead moved past the copy, so pasting again lays the next one after it.
    store_settles(cx, |s| s.selection.first().is_some_and(|id| *id != original));
    let t = playhead(cx);
    assert!((t - 14.).abs() < 1e-6, "playhead after the paste, got {t}");
    let sel = cx.update(|_, cx| cx.store().read(cx).selection.clone());
    assert!(sel.len() == 1 && sel[0] != original, "the copy is selected");
}

#[gpui::test]
fn up_and_down_jump_between_cuts(cx: &mut TestAppContext) {
    let (_f, _, cx) = setup(cx);
    seek(cx, 1.);
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(playhead(cx), 4., "the clip's end");
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    assert_eq!(playhead(cx), 0., "its start");
}

#[gpui::test]
fn q_and_w_trim_to_the_playhead(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    seek(cx, 1.);
    cx.simulate_keystrokes("q");
    let p = f.settle(cx, |p| p.clips().next().unwrap().1.start == 1.);
    assert_eq!(p.clips().next().unwrap().1.start, 1.);
    seek(cx, 3.);
    cx.simulate_keystrokes("w");
    let p = f.settle(cx, |p| p.clips().next().unwrap().1.end() == 3.);
    let c = p.clips().next().unwrap().1;
    assert_eq!((c.start, c.end()), (1., 3.));
}

#[gpui::test]
fn j_k_l_shuttle(cx: &mut TestAppContext) {
    let (_f, _, cx) = setup(cx);
    let state = |cx: &mut VisualTestContext| cx.update(|_, cx| {
        let p = cx.store().read(cx).playback.read(cx);
        (p.playing, p.shuttle)
    });
    cx.simulate_keystrokes("l");
    cx.run_until_parked();
    assert_eq!(state(cx), (true, 0.), "l plays");
    cx.simulate_keystrokes("l");
    cx.run_until_parked();
    assert_eq!(state(cx), (false, 2.), "again: twice as fast");
    cx.simulate_keystrokes("k");
    cx.run_until_parked();
    assert_eq!(state(cx), (false, 0.), "k stops");
    seek(cx, 2.);
    cx.simulate_keystrokes("j");
    cx.run_until_parked();
    assert_eq!(state(cx), (false, -1.), "j plays backwards");
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    assert_eq!(state(cx), (false, 0.), "space stops a shuttle too");
}

#[gpui::test]
fn question_mark_shows_the_shortcuts(cx: &mut TestAppContext) {
    let (_f, _, cx) = setup(cx);
    cx.simulate_keystrokes("?");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).dialog.clone()), Some(Dialog::Shortcuts));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).dialog.clone()), None);
}

#[gpui::test]
fn undo_says_what_it_undid(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    cx.simulate_keystrokes("t");
    f.settle(cx, |p| texts(p) == 1);
    store_settles(cx, |s| s.can_undo);
    cx.simulate_keystrokes(&format!("{M}-z"));
    store_settles(cx, |s| s.toasts.iter().any(|t| t.flash));
    let toasts: Vec<String> = cx.update(|_, cx| cx.store().read(cx).toasts.iter().map(|t| t.text.to_string()).collect());
    assert!(toasts.iter().any(|t| t == "Undid new title"), "{toasts:?}");
}

/// A person grades the selected clip with the inspector's slider: one undo step for the drag.
#[gpui::test]
fn the_colour_sliders_grade_the_clip_in_one_step(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let id = select_the_clip(&f, cx);
    let inspector = cx.update(|_, cx| view.read(cx).editor().read(cx).inspector.clone());
    let slider = cx.update(|_, cx| inspector.read(cx).effect_slider("contrast")).expect("a contrast slider");
    for (v, final_) in [(0.1, false), (0.4, false), (0.4, true)] {
        cx.update(|_, cx| slider.update(cx, |_, cx| cx.emit(crate::ui::scrub::ScrubChange { value: v, final_ })));
        f.settle(cx, |p| p.clip(id).is_some_and(|c| (c.effects.contrast - v).abs() < 1e-9));
    }
    let p = f.settle(cx, |p| p.clip(id).is_some_and(|c| (c.effects.contrast - 0.4).abs() < 1e-9));
    assert!((p.clip(id).unwrap().effects.contrast - 0.4).abs() < 1e-9);
    let steps = f.call("history.list", json!({}));
    let graded: Vec<&Value> = steps["undo"].as_array().unwrap().iter().filter(|s| s["label"] == "clip.setEffects").collect();
    assert_eq!(graded.len(), 1, "the drag is one step: {steps}");
}

/// A person opens the Captions tab (⌘5 / ctrl-5), imports a file through the command the panel
/// runs, and double-clicks a caption: it is selected and the playhead goes there.
#[gpui::test]
fn the_captions_tab_lists_captions_and_goes_to_them(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    cx.simulate_keystrokes(&format!("{M}-5"));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).left_tab), crate::store::LeftTab::Captions);
    let srt = f._dir.path().join("subs.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,000\nOne\n\n2\n00:00:02,500 --> 00:00:03,500\nTwo\n").unwrap();
    f.call("captions.import", json!({ "path": srt }));
    let p = f.settle(cx, |p| p.captions().len() == 2);
    let second = p.captions()[1].1.id;
    store_settles(cx, |s| s.project.as_ref().is_some_and(|p| p.captions().len() == 2));
    // The panel draws a row per caption; clicking the second one goes there.
    cx.update(|_, cx| crate::views::captions_panel::go_to_for_test(second, 2.5, cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).selection.clone()), vec![second]);
    assert!((playhead(cx) - 2.5).abs() < 0.01);
    let _ = view;
}
