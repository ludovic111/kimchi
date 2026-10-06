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

    /// Lets the window and Tokio work until `done` holds (or `PATIENCE` passes).
    pub fn settle(&self, cx: &mut VisualTestContext, done: impl Fn(&Project) -> bool) -> Project {
        let start = Instant::now();
        loop {
            cx.run_until_parked();
            let p = self.project();
            if done(&p) || start.elapsed() > PATIENCE {
                return p;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// How long UI tests wait for work to finish: generous, as tests share a busy machine,
/// and a wait ends as soon as its condition holds.
pub(crate) const PATIENCE: Duration = Duration::from_secs(20);

/// The platform's command key, as the shortcuts are bound (`actions.rs`).
#[cfg(target_os = "macos")]
const M: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const M: &str = "ctrl";

/// Lets the window catch up until its store satisfies `done` (or `PATIENCE` passes): the store hears
/// about changes through the session's events, a moment after the session has them.
pub(crate) fn store_settles(cx: &mut VisualTestContext, done: impl Fn(&crate::store::Store) -> bool) {
    let start = Instant::now();
    while !cx.update(|_, cx| done(cx.store().read(cx))) && start.elapsed() < PATIENCE {
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
    let start = Instant::now();
    while start.elapsed() < PATIENCE {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(texts(&f.project()), 0);
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
    while !task.is_finished() && start.elapsed() < PATIENCE {
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
    cx.update(|_, cx| cx.store().update(cx, |s, cx| s.set_left_tab(crate::store::LeftTab::Inspector, cx)));
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
        assert!(start.elapsed() < PATIENCE, "the words field shows");
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

/// Looks imported from other apps show in the inspector's Colour section and go on the clip
/// in one step; the grade is written out as a .cube other apps open.
#[gpui::test]
fn imported_looks_show_in_the_inspector_and_apply(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let id = select_the_clip(&f, cx);
    let dir = tempfile::tempdir().unwrap();
    let cube = dir.path().join("Night Film.cube");
    std::fs::write(&cube, "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
    let added = f.call("looks.import", json!({ "paths": [cube], "folder": "Resolve" }));
    let look = added["added"][0]["id"].as_str().unwrap().to_string();
    let inspector = cx.update(|_, cx| view.read(cx).editor().read(cx).inspector.clone());
    let start = Instant::now();
    while !cx.update(|_, cx| inspector.read(cx).library_look_names()).contains(&"Night Film".to_string()) && start.elapsed() < Duration::from_secs(3) {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
    assert!(cx.update(|_, cx| inspector.read(cx).library_look_names()).contains(&"Night Film".to_string()));
    // What the look's button runs.
    cx.update(|_, cx| cx.store().update(cx, |s, cx| s.run("looks.apply", json!({ "clipIds": [id], "look": look }), cx)));
    let p = f.settle(cx, |p| p.clip(id).is_some_and(|c| c.effects.lut.is_some()));
    assert!(p.clip(id).unwrap().effects.lut.as_ref().unwrap().path.contains("looks"));
    let out = dir.path().join("graded.cube");
    f.call("clip.setEffects", json!({ "clipIds": [id], "contrast": 0.3 }));
    f.call("looks.save", json!({ "clipId": id, "path": out }));
    assert!(std::fs::read_to_string(&out).unwrap().contains("LUT_3D_SIZE 33"));
}

/// The export dialog's presets fit the project: a vertical preset on a 16:9 project keeps 16:9.
#[gpui::test]
fn export_presets_fit_the_project(cx: &mut TestAppContext) {
    let (f, _view, cx) = setup(cx);
    let ps = f.project().settings;
    let short = kimchi_control::commands::export::preset("shorts").unwrap();
    let (w, h) = kimchi_control::commands::export::preset_size(short, &ps).unwrap();
    assert_eq!((w, h), (1080, 608), "a 1920×1080 project in a 1080×1920 box");
    let yt = kimchi_control::commands::export::preset("youtube-4k").unwrap();
    assert_eq!(kimchi_control::commands::export::preset_size(yt, &ps), Some((3840, 2160)));
    let listed = f.call("export.presets", json!({}));
    let shorts = listed.as_array().unwrap().iter().find(|p| p["id"] == "shorts").unwrap();
    assert!(shorts["warning"].as_str().unwrap().contains("1080×608"));
    let _ = cx;
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

/// Runs a command as a script would (from Tokio, through the window when it needs it).
fn remote(f: &Fixture, cx: &mut VisualTestContext, name: &str, params: Value) -> Value {
    let (s, name_owned) = (f.session.clone(), name.to_string());
    let task = f.rt.spawn(async move { kimchi_control::call(&s, Source::Cli, &name_owned, params).await });
    let start = Instant::now();
    while !task.is_finished() && start.elapsed() < PATIENCE {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.run_until_parked();
    f.rt.block_on(task).unwrap().unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// What a script can do in the window: options, shortcuts by name (on the window's own clipboard),
/// panels and their sizes; and the Agent panel draws the agent's conversation, scripts' cards included.
#[gpui::test]
fn scripts_reach_what_the_window_does(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    f.call("clip.addText", json!({ "text": "A", "start": 5, "duration": 1 }));
    f.settle(cx, |p| p.clips().count() == 2);
    let v = remote(&f, cx, "ui.setTimeline", json!({ "snapping": false, "ripple": true, "loop": true }));
    assert_eq!(v, json!({ "snapping": false, "ripple": true, "loop": true }));
    assert!(cx.update(|_, cx| {
        let s = cx.store().read(cx);
        !s.snapping && s.ripple && s.playback.read(cx).looping
    }));
    let state = f.call("ui.state", json!({}));
    assert_eq!((&state["snapping"], &state["ripple"], &state["loop"]), (&json!(false), &json!(true), &json!(true)), "{state}");

    // Select all, copy, move the playhead, paste: as the keys do.
    remote(&f, cx, "ui.action", json!({ "action": "SelectAll" }));
    store_settles(cx, |s| s.selection.len() == 2);
    remote(&f, cx, "ui.action", json!({ "action": "CopyClips" }));
    remote(&f, cx, "timeline.seek", json!({ "time": 10 }));
    remote(&f, cx, "ui.action", json!({ "action": "PasteClips" }));
    let p = f.settle(cx, |p| p.clips().count() == 4);
    assert_eq!(p.clips().count(), 4, "two copies pasted");
    assert!(p.clips().any(|(_, c)| (c.start - 10.0).abs() < 1e-6));

    remote(&f, cx, "ui.showPanel", json!({ "panel": "agent" }));
    assert!(cx.update(|_, cx| cx.store().read(cx).agent_open));
    remote(&f, cx, "ui.showPanel", json!({ "panel": "agent", "open": false }));
    assert!(!cx.update(|_, cx| cx.store().read(cx).agent_open));
    let l = remote(&f, cx, "ui.setLayout", json!({ "left": 9999, "timeline": 250 }));
    assert_eq!((l["left"].as_f64(), l["timeline"].as_f64()), (Some(520.0), Some(250.0)), "kept within limits");
    assert_eq!(f.call("ui.state", json!({}))["layout"]["timeline"], 250.0);

    f.call("timeline.addMarker", json!({ "time": 1, "label": "Here" }));
    let panel = cx.update(|_, cx| view.read(cx).editor().read(cx).agent.clone());
    let has_card = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            panel.read(cx).snap.entries.iter().any(|e| matches!(e, kimchi_agent::Entry::Command { record, run: None, .. } if record.command == "timeline.addMarker"))
        })
    };
    let start = Instant::now();
    while !has_card(cx) && start.elapsed() < PATIENCE {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(has_card(cx), "the script's command is a card in the Agent panel");
}

// ---- the window at every size ----------------------------------------------------------

fn bounds_of(cx: &mut VisualTestContext, name: &'static str) -> Option<gpui::Bounds<gpui::Pixels>> {
    cx.debug_bounds(name)
}

pub(crate) fn resize(cx: &mut VisualTestContext, w: f32, h: f32) {
    cx.simulate_resize(gpui::size(gpui::px(w), gpui::px(h)));
    cx.run_until_parked();
    // A second frame: the editor lays out from the size the first one measured.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn inside(b: gpui::Bounds<gpui::Pixels>, w: f32, h: f32) -> bool {
    let (x0, y0) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (x1, y1) = (x0 + f32::from(b.size.width), y0 + f32::from(b.size.height));
    x0 >= -0.5 && y0 >= -0.5 && x1 <= w + 0.5 && y1 <= h + 0.5
}

/// Overlap area of two boxes, in pixels (splitters overlap their neighbours by 2 px on purpose).
fn overlap(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> f32 {
    let i = a.intersect(&b);
    (f32::from(i.size.width).max(0.)) * (f32::from(i.size.height).max(0.))
}

/// The editor at sizes from the smallest window to 4K, very wide and very tall, with and without
/// the Agent panel: every panel inside the window, none on top of another, the preview and the
/// timeline never squeezed below their minimums.
#[gpui::test]
fn the_editor_fits_every_window_size(cx: &mut TestAppContext) {
    use crate::ui::layout::{PREVIEW_MIN_W, TIMELINE_MIN, WINDOW_MIN_H, WINDOW_MIN_W};
    let (_f, _, cx) = setup(cx);
    for (w, h) in [(WINDOW_MIN_W, WINDOW_MIN_H), (800., 600.), (1024., 640.), (1366., 768.), (1920., 1080.), (3840., 2160.), (2560., 700.), (900., 1400.)] {
        for agent in [false, true] {
            cx.update(|_, cx| cx.store().update(cx, |s, cx| s.set_agent_open(agent, cx)));
            resize(cx, w, h);
            let names = ["top-bar", "rail", "left-panel", "preview", "inspector", "timeline", "agent", "transport"];
            let found: Vec<(&str, gpui::Bounds<gpui::Pixels>)> = names.iter().filter_map(|n| bounds_of(cx, n).map(|b| (*n, b))).collect();
            let get = |n: &str| found.iter().find(|(m, _)| *m == n).map(|(_, b)| *b);
            for n in ["top-bar", "rail", "preview", "timeline", "transport"] {
                assert!(get(n).is_some(), "{w}x{h} agent {agent}: {n} is drawn");
            }
            for (n, b) in &found {
                assert!(inside(*b, w, h), "{w}x{h} agent {agent}: {n} leaves the window: {b:?}");
            }
            // Docked panels side by side: no two cover each other.
            for (i, (a, ba)) in found.iter().enumerate() {
                for (b, bb) in &found[i + 1..] {
                    if matches!((*a, *b), ("preview", "transport")) {
                        continue;
                    }
                    assert!(overlap(*ba, *bb) < 1., "{w}x{h} agent {agent}: {a} and {b} overlap ({ba:?} / {bb:?})");
                }
            }
            let preview = get("preview").unwrap();
            assert!(f32::from(preview.size.width) >= PREVIEW_MIN_W - 1., "{w}x{h} agent {agent}: the preview is {preview:?}");
            assert!(f32::from(get("timeline").unwrap().size.height) >= TIMELINE_MIN - 1.);
            // The agent docks beside a wide editor, else it floats in a drawer.
            if agent {
                assert!(get("agent").is_some() || bounds_of(cx, "agent-drawer").is_some(), "{w}x{h}: the agent shows");
            }
        }
    }
}

/// A narrow window: a tab of the rail opens the left panel as a drawer over the work, Escape
/// closes it; `ui.state` says so.
#[gpui::test]
fn narrow_windows_open_the_left_panel_as_a_drawer(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    resize(cx, 720., 480.);
    assert!(bounds_of(cx, "left-panel").is_none(), "no room to dock it");
    cx.simulate_keystrokes(&format!("{M}-2"));
    cx.run_until_parked();
    let drawer = bounds_of(cx, "left-drawer").expect("the Generate tab opens as a drawer");
    assert!(inside(drawer, 720., 480.));
    assert_eq!(f.call("ui.state", json!({}))["layout"]["overlays"], json!(["left"]));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(bounds_of(cx, "left-drawer").is_none(), "escape closes it");
    // Wide again: the panel docks, at the size it had.
    resize(cx, 1600., 1000.);
    let left = bounds_of(cx, "left-panel").expect("docked");
    assert_eq!(f32::from(left.size.width).round(), crate::ui::layout::LEFT_W);
}

/// Dialogs at the smallest window: inside it, with their buttons in view.
#[gpui::test]
fn dialogs_fit_the_smallest_window(cx: &mut TestAppContext) {
    use crate::ui::layout::{WINDOW_MIN_H, WINDOW_MIN_W};
    let (_f, _, cx) = setup(cx);
    resize(cx, WINDOW_MIN_W, WINDOW_MIN_H);
    for (dialog, name, inner) in [
        (Dialog::Settings { section: None }, "dialog-settings", Some("settings-body")),
        (Dialog::Settings { section: Some("agent".into()) }, "dialog-settings", Some("settings-body")),
        (Dialog::Export, "dialog-export", Some("export-footer")),
        (Dialog::Shortcuts, "dialog-shortcuts", None),
        (Dialog::Palette, "dialog-palette", None),
        (Dialog::WhatsNew { since: None, all: true }, "dialog-whats-new", None),
        (
            Dialog::Interop {
                title: "Opened cut.fcpxml".into(),
                report: json!({"kept": ["12 clips on 3 tracks"], "approximated": ["Iris became a dissolve"], "dropped": ["crops"], "missingMedia": ["/gone/a.mov"]}),
            },
            "dialog-interop-report",
            None,
        ),
    ] {
        cx.update(|_, cx| cx.store().update(cx, |s, cx| s.open_dialog(dialog.clone(), cx)));
        resize(cx, WINDOW_MIN_W, WINDOW_MIN_H);
        let b = bounds_of(cx, name).unwrap_or_else(|| panic!("{dialog:?} is drawn"));
        assert!(inside(b, WINDOW_MIN_W, WINDOW_MIN_H), "{dialog:?} leaves the window: {b:?}");
        if let Some(inner) = inner {
            let i = bounds_of(cx, inner).unwrap_or_else(|| panic!("{inner} is drawn"));
            assert!(f32::from(i.origin.y + i.size.height) <= f32::from(b.origin.y + b.size.height) + 0.5, "{inner} is cut off: {i:?} in {b:?}");
        }
        cx.update(|_, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)));
    }
}

/// The panel shortcuts and `ui.setLayout` close and open the side panels; the preview takes the room.
#[gpui::test]
fn side_panels_close_and_open(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    resize(cx, 1600., 1000.);
    let preview_w = |cx: &mut VisualTestContext| f32::from(bounds_of(cx, "preview").unwrap().size.width);
    let before = preview_w(cx);
    cx.simulate_keystrokes(&format!("{M}-alt-i"));
    resize(cx, 1600., 1000.);
    assert!(bounds_of(cx, "left-panel").is_some(), "the inspector occupies the left sidebar");
    assert!(bounds_of(cx, "inspector").is_none(), "there is no separate inspector dock");
    assert!((preview_w(cx) - before).abs() < 1., "switching a left tab keeps the viewport width");
    assert_eq!(f.call("ui.state", json!({}))["layout"]["inspectorOpen"], true);
    cx.simulate_keystrokes(&format!("{M}-alt-i"));
    resize(cx, 1600., 1000.);
    assert!(preview_w(cx) > before + 200.);
    remote(&f, cx, "ui.setLayout", json!({ "leftOpen": false }));
    resize(cx, 1600., 1000.);
    assert!(bounds_of(cx, "left-panel").is_none() && bounds_of(cx, "rail").is_some(), "the rail stays");
    let state = f.call("ui.state", json!({}));
    assert_eq!((&state["layout"]["leftOpen"], &state["layout"]["inspectorOpen"]), (&json!(false), &json!(false)), "{state}");
    // A tab asked for opens the left panel again; so does the inspector's shortcut.
    cx.simulate_keystrokes(&format!("{M}-3"));
    cx.simulate_keystrokes(&format!("{M}-alt-i"));
    resize(cx, 1600., 1000.);
    assert!(bounds_of(cx, "left-panel").is_some() && bounds_of(cx, "inspector").is_none());
    assert_eq!(f.call("ui.state", json!({}))["layout"]["inspectorOpen"], true);
}

/// The editor's menu offers opening other editors' projects and writing one for each app.
#[test]
fn interop_menu_targets() {
    let targets = crate::views::dialogs::interop::export_targets();
    let names: Vec<&str> = targets.iter().map(|t| t.1).collect();
    for app in ["Premiere Pro", "Final Cut Pro", "DaVinci Resolve", "Avid Media Composer"] {
        assert!(names.iter().any(|n| n.contains(app.split(' ').next().unwrap())), "{app} in {names:?}");
    }
    assert_eq!(crate::views::dialogs::interop::extension("xmeml"), "xml");
    assert_eq!(crate::views::dialogs::interop::extension("fcpxml"), "fcpxml");
}

/// Settings › Agent draws every provider group and follows the chosen one; with the agent
/// turned off, the panel stays closed.
#[gpui::test]
fn the_agent_settings_follow_the_provider_and_the_panel_can_be_turned_off(cx: &mut TestAppContext) {
    let (f, _, cx) = setup(cx);
    remote(&f, cx, "ui.showPanel", json!({ "panel": "settings", "section": "agent" }));
    for provider in ["groq", "bedrock", "lmstudio", "openai-compatible"] {
        remote(&f, cx, "agent.setProvider", json!({ "provider": provider }));
        store_settles(cx, |s| s.settings.agent.provider == provider);
        cx.run_until_parked();
        assert!(cx.debug_bounds("settings-body").is_some(), "{provider}: the section is drawn");
    }
    f.call("app.setSetting", json!({ "key": "agent.enabled", "value": false }));
    store_settles(cx, |s| !s.settings.agent.enabled);
    cx.update(|_, cx| cx.store().update(cx, |s, cx| s.set_agent_open(true, cx)));
    assert!(!cx.update(|_, cx| cx.store().read(cx).agent_open), "off keeps the Agent panel closed");
    f.call("app.setSetting", json!({ "key": "agent.enabled", "value": true }));
    store_settles(cx, |s| s.settings.agent.enabled);
    cx.update(|_, cx| cx.store().update(cx, |s, cx| s.set_agent_open(true, cx)));
    assert!(cx.update(|_, cx| cx.store().read(cx).agent_open));
}
