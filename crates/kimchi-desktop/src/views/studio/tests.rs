//! The Studio in the headless window: opened from another client, things picked in the
//! outliner, added from the add menu, a stack item's parameter changed in Properties, an effect
//! added, keyframes dragged in the dope sheet, Blender's G with an axis and a typed number, the
//! delete key, edit mode and extrude, and the render-ahead states the timeline badges show.

use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext, point, px};
use kimchi_control::Source;
use kimchi_core::{ClipContent, Id, Project, Scene};
use serde_json::{Value, json};

use super::{KeyRef, Mode, Studio, model};
use crate::app::Workspace;
use crate::store::StoreExt;
use crate::tests::{Fixture, setup, store_settles};
use crate::ui::scrub::ScrubChange;

/// A window command from another client (as MCP would send it), answered by the window.
fn ui(f: &Fixture, cx: &mut VisualTestContext, name: &str, params: Value) -> Result<Value, String> {
    let (s, n) = (f.session.clone(), name.to_string());
    let task = f.rt.spawn(async move { kimchi_control::call(&s, Source::Mcp, &n, params).await });
    let start = Instant::now();
    while !task.is_finished() && start.elapsed() < Duration::from_secs(5) {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    f.rt.block_on(task).expect("the call ran")
}

fn studio(view: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<Studio> {
    cx.update(|_, cx| view.read(cx).editor().read(cx).studio.clone())
}

fn scene_of(p: &Project, clip: Id) -> Scene {
    match &p.clip(clip).expect("the clip").content {
        ClipContent::Motion { scene, .. } => scene.clone(),
        _ => panic!("not a motion clip"),
    }
}

/// Adds a template clip and opens it in the Studio.
fn open(f: &Fixture, cx: &mut VisualTestContext, template: &str) -> Id {
    let v = f.call("motion.addTemplate", json!({ "template": template, "start": 0 }));
    let clip: Id = v["clips"][0]["id"].as_str().expect("a clip id").parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    let state = ui(f, cx, "ui.studio", json!({ "clipId": clip.to_string() })).expect("ui.studio");
    assert_eq!(state["open"], true, "{state}");
    clip
}

fn wait(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
    let start = Instant::now();
    while !done(cx) && start.elapsed() < Duration::from_secs(4) {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui::test]
fn the_studio_opens_on_3d_and_2d_clips_and_its_outliner_selects(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let clip = open(&f, cx, "shapes3d");
    let st = studio(&view, cx);
    assert!(cx.update(|_, cx| st.read(cx).is_open()));
    let state = f.call("ui.state", json!({}));
    assert_eq!((state["screen"].as_str(), state["studio"]["kind"].as_str()), (Some("studio"), Some("3d")), "{state}");

    // The outliner: the world, cameras, lights, objects.
    let scene = scene_of(&f.project(), clip);
    let objects: Vec<String> = model::rows(&scene, &Default::default()).into_iter().filter(|r| r.kind == model::RowKind::Object).map(|r| r.key).collect();
    assert!(objects.len() >= 2, "{objects:?}");
    let outliner = cx.update(|_, cx| st.read(cx).outliner.clone());
    cx.update(|w, cx| outliner.update(cx, |o, cx| o.click(&objects[0], false, w, cx)));
    cx.update(|w, cx| outliner.update(cx, |o, cx| o.click(&objects[1], true, w, cx)));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| st.read(cx).selection.clone()), objects[..2].to_vec());
    let state = ui(&f, cx, "ui.studio", json!({ "view": "front", "tool": "rotate" })).unwrap();
    assert_eq!((state["tool"].as_str(), state["selection"].as_array().map(Vec::len)), (Some("rotate"), Some(2)));
    assert!(ui(&f, cx, "ui.studio", json!({ "select": ["nope"] })).unwrap_err().contains("No \"nope\""));
    assert!(ui(&f, cx, "ui.studio", json!({ "tool": "lasso" })).unwrap_err().contains("tool is one of"));

    // Esc goes back to the edit.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!cx.update(|_, cx| st.read(cx).is_open()));
    assert_eq!(f.call("ui.state", json!({}))["screen"], "editor");

    // A 2D clip: layers, top first.
    let flat = open(&f, cx, "lowerThird");
    let state = ui(&f, cx, "ui.studio", json!({})).unwrap();
    assert_eq!((state["kind"].as_str(), state["tool"].as_str()), (Some("2d"), Some("select")), "{state}");
    let scene = scene_of(&f.project(), flat);
    let Scene::Flat(s) = &scene else { panic!("2d") };
    let top = s.layers.last().unwrap().id.clone();
    let rows = model::rows(&scene, &Default::default());
    assert_eq!(rows[1].key, top, "the top layer is listed first");
}

#[gpui::test]
fn the_add_menu_adds_objects_lights_and_layers(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let clip = open(&f, cx, "shapes3d");
    let st = studio(&view, cx);
    let pick = |label: &'static str, cx: &mut VisualTestContext| {
        cx.update(|w, cx| st.update(cx, |s, cx| s.open_add_menu(Some(point(px(300.), px(300.))), w, cx)));
        cx.run_until_parked();
        let choice = cx.update(|_, cx| {
            let s = st.read(cx);
            s.popover.as_ref().expect("the add menu").choices(s, st.clone(), cx).into_iter().find(|c| c.label.as_ref() == label).unwrap_or_else(|| panic!("no {label}"))
        });
        cx.update(|w, cx| {
            st.update(cx, |s, _| s.popover = None);
            (choice.run)(w, cx)
        });
    };
    pick("Box", cx);
    let p = f.settle(cx, |p| matches!(scene_of(p, clip), Scene::Space(s) if kimchi_core::motion::find_object(&s.objects, "box1").is_some()));
    let Scene::Space(s) = scene_of(&p, clip) else { panic!() };
    assert!(kimchi_core::motion::find_object(&s.objects, "box1").is_some(), "a box was added");
    wait(cx, |cx| cx.update(|_, cx| st.read(cx).selection == ["box1"]));
    assert_eq!(cx.update(|_, cx| st.read(cx).selection.clone()), ["box1"], "and selected");
    pick("Spot", cx);
    let p = f.settle(cx, |p| matches!(scene_of(p, clip), Scene::Space(s) if s.lights.iter().any(|l| l.kind == "spot")));
    let Scene::Space(s) = scene_of(&p, clip) else { panic!() };
    assert!(s.lights.iter().any(|l| l.kind == "spot"));
    let steps = f.call("history.list", json!({}));
    assert_eq!((steps["undo"][0]["label"].as_str(), steps["undo"][0]["source"].as_str()), (Some("motion.setLayer"), Some("window")));
}

#[gpui::test]
fn stacks_are_built_from_their_tables_and_edited_in_properties(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let clip = open(&f, cx, "shapes3d");
    let st = studio(&view, cx);
    let Scene::Space(s) = scene_of(&f.project(), clip) else { panic!() };
    let id = s.objects[0].id.clone();
    ui(&f, cx, "ui.studio", json!({ "select": [id] })).unwrap();
    // The modifier's add menu offers the table's types; add a twist.
    cx.update(|w, cx| st.update(cx, |s, cx| s.popover = Some(super::Popover::stack(point(px(10.), px(10.)), id.clone(), "modifiers", w, cx))));
    let twist = cx.update(|_, cx| {
        let s = st.read(cx);
        let list = s.popover.as_ref().unwrap().choices(s, st.clone(), cx);
        assert_eq!(list.len(), kimchi_core::motion::stack::MODIFIERS.len());
        list.into_iter().find(|c| c.label.as_ref() == "Twist").unwrap()
    });
    cx.update(|w, cx| (twist.run)(w, cx));
    let has = |p: &Project| matches!(scene_of(p, clip), Scene::Space(s) if s.objects[0].modifiers.iter().any(|m| m.kind == "twist"));
    assert!(has(&f.settle(cx, has)), "the twist is on");
    // Its angle, in the Modifiers tab.
    let props = cx.update(|_, cx| st.read(cx).properties.clone());
    cx.update(|_, cx| props.update(cx, |p, cx| p.set_tab(super::properties::Tab::Modifiers, cx)));
    let key = format!("{id}|modifiers.twist.angle");
    wait(cx, |cx| cx.update(|_, cx| props.read(cx).scrub_of(&key).is_some()));
    let scrub = cx.update(|_, cx| props.read(cx).scrub_of(&key)).unwrap_or_else(|| panic!("no field {key}"));
    cx.update(|_, cx| scrub.update(cx, |_, cx| cx.emit(ScrubChange { value: 45.0, final_: true })));
    let angle = |p: &Project| match scene_of(p, clip) {
        Scene::Space(s) => s.objects[0].modifiers.iter().find(|m| m.kind == "twist").map(|m| m.n("angle")),
        _ => None,
    };
    assert_eq!(angle(&f.settle(cx, |p| angle(p) == Some(45.0))), Some(45.0));
}

#[gpui::test]
fn effects_go_on_2d_layers(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let clip = open(&f, cx, "lowerThird");
    let st = studio(&view, cx);
    let Scene::Flat(s) = scene_of(&f.project(), clip) else { panic!() };
    let id = s.layers[0].id.clone();
    ui(&f, cx, "ui.studio", json!({ "select": [id] })).unwrap();
    cx.update(|w, cx| st.update(cx, |s, cx| s.popover = Some(super::Popover::stack(point(px(10.), px(10.)), id.clone(), "effects", w, cx))));
    let blur = cx.update(|_, cx| {
        let s = st.read(cx);
        s.popover.as_ref().unwrap().choices(s, st.clone(), cx).into_iter().find(|c| c.label.as_ref() == "Gaussian blur").unwrap()
    });
    cx.update(|w, cx| (blur.run)(w, cx));
    let has = |p: &Project| matches!(scene_of(p, clip), Scene::Flat(s) if s.find_layer(&id).is_some_and(|l| l.effects.iter().any(|e| e.kind == "blur")));
    assert!(has(&f.settle(cx, has)), "the blur is on");
}

#[gpui::test]
fn keyframes_drag_in_the_dope_sheet(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let clip = open(&f, cx, "shapes3d");
    let st = studio(&view, cx);
    // A thing's last keyframe of a property.
    let scene = scene_of(&f.project(), clip);
    let (id, prop, time) = model::thing_ids(&scene)
        .into_iter()
        .find_map(|id| {
            let keys = model::keyframes(&scene, &id)?;
            let (name, list) = keys.iter().find(|(_, l)| l.len() >= 2)?;
            Some((id, name.clone(), list[list.len() - 1].time))
        })
        .expect("an animated thing");
    cx.update(|_, cx| st.update(cx, |s, cx| {
        s.keys = vec![KeyRef { id: id.clone(), property: prop.clone(), time }];
        s.changed(cx);
    }));
    let timeline = cx.update(|_, cx| st.read(cx).timeline.clone());
    cx.update(|_, cx| timeline.update(cx, |t, cx| t.drag_keys_by(-0.5, cx)));
    let moved = |p: &Project| model::keyframes(&scene_of(p, clip), &id).and_then(|k| k.get(&prop).cloned()).is_some_and(|l| l.iter().any(|k| (k.time - (time - 0.5)).abs() < 1e-3));
    assert!(moved(&f.settle(cx, moved)), "{id}.{prop} moved from {time}");
    let steps = f.call("history.list", json!({}));
    assert_eq!(steps["undo"][0]["label"], "motion.shiftKeyframes");
    // The selection follows the keys.
    assert!((cx.update(|_, cx| st.read(cx).keys[0].time) - (time - 0.5)).abs() < 1e-6);
}

#[gpui::test]
fn g_x_and_a_number_move_like_blender_and_x_deletes(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let v = f.call("motion.add", json!({ "scene": { "type": "3d", "objects": [{ "id": "box", "type": "box", "position": [1, 0, 0] }, { "id": "ball", "type": "sphere" }] }, "start": 0, "duration": 4 }));
    let clip: Id = v["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    ui(&f, cx, "ui.studio", json!({ "clipId": clip.to_string(), "select": ["box"] })).unwrap();
    let st = studio(&view, cx);
    cx.simulate_keystrokes("g x 2 enter");
    let x = |p: &Project| model::value_at(&scene_of(p, clip), "box", "position.x", 0.0).and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
    let p = f.settle(cx, |p| (x(p) - 3.0).abs() < 1e-6);
    assert!((x(&p) - 3.0).abs() < 1e-6, "box at x {}", x(&p));
    // One undo step for the whole move.
    let steps = f.call("history.list", json!({}));
    assert_eq!(steps["undo"][0]["label"], "motion.updateLayer");
    assert_eq!(steps["undo"][1]["label"], "motion.add");
    // X deletes the selection; Ctrl/Cmd+Z brings it back.
    cx.simulate_keystrokes("x");
    let gone = |p: &Project| matches!(scene_of(p, clip), Scene::Space(s) if s.objects.len() == 1);
    assert!(gone(&f.settle(cx, gone)));
    assert!(cx.update(|_, cx| st.read(cx).selection.is_empty()));
}

#[gpui::test]
fn edit_mode_extrudes_faces(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let cube = json!({ "id": "cube", "type": "mesh",
        "vertices": [[-0.5,-0.5,-0.5],[0.5,-0.5,-0.5],[0.5,0.5,-0.5],[-0.5,0.5,-0.5],[-0.5,-0.5,0.5],[0.5,-0.5,0.5],[0.5,0.5,0.5],[-0.5,0.5,0.5]],
        "faces": [[0,3,2,1],[4,5,6,7],[0,1,5,4],[2,3,7,6],[1,2,6,5],[0,4,7,3]] });
    let v = f.call("motion.add", json!({ "scene": { "type": "3d", "objects": [cube] }, "start": 0, "duration": 4 }));
    let clip: Id = v["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    let state = ui(&f, cx, "ui.studio", json!({ "clipId": clip.to_string(), "select": ["cube"], "mode": "edit", "selectMode": "face", "editSelection": { "faces": [1] } })).unwrap();
    assert_eq!((state["mode"].as_str(), state["selectMode"].as_str()), (Some("edit"), Some("face")), "{state}");
    let st = studio(&view, cx);
    cx.update(|_, cx| st.update(cx, |s, cx| s.mesh_op("extrude", json!({ "distance": 0.5 }), cx)));
    let faces = |p: &Project| match scene_of(p, clip) {
        Scene::Space(s) => match &s.objects[0].shape {
            kimchi_core::motion::Shape3d::Mesh { faces, .. } => faces.len(),
            _ => 0,
        },
        _ => 0,
    };
    assert_eq!(faces(&f.settle(cx, |p| faces(p) > 6)), 10, "a face extruded: four walls more");
    // The new selection is the moved face.
    wait(cx, |cx| cx.update(|_, cx| !st.read(cx).edit_sel.faces.is_empty()));
    assert_eq!(cx.update(|_, cx| st.read(cx).edit_sel.faces.len()), 1);
    // Tab leaves edit mode.
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| st.read(cx).mode), Mode::Object);
}

#[gpui::test]
fn motion_clips_say_whether_they_are_live_rendered_or_out_of_date(cx: &mut TestAppContext) {
    use super::render_state::{RenderState, state};
    let (f, _view, cx) = setup(cx);
    let v = f.call("motion.addTemplate", json!({ "template": "lowerThird", "start": 0 }));
    let clip: Id = v["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    let now = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let s = cx.store();
            let s = s.read(cx);
            let p = s.project.clone().unwrap();
            state(s, &p, p.clip(clip).unwrap())
        })
    };
    assert_eq!(now(cx), Some(RenderState::Live));
    // Rendered once, but the file is gone (or the scene changed): out of date.
    let rendered = kimchi_core::Rendered { file: "/nowhere/render.mov".into(), key: "old".into(), from: 0.0, fps: 30.0, frames: 10, width: 64, height: 36, engine: "standard".into() };
    f.session
        .apply("test", Source::Cli, &kimchi_core::Edit::UpdateClip { clip_id: clip, patch: kimchi_core::ClipPatch { rendered: Some(Some(rendered)), ..Default::default() } }, None)
        .unwrap();
    store_settles(cx, |s| s.clip(clip).is_some_and(|c| c.rendered.is_some()));
    assert_eq!(now(cx), Some(RenderState::Outdated));
    // A render running: its progress.
    cx.update(|_, cx| {
        cx.store().update(cx, |s, _| {
            s.renders.push(kimchi_control::renders::RenderStatus {
                id: "r1".into(),
                clip_id: clip,
                clip_name: "x".into(),
                progress: 0.4,
                done: false,
                error: None,
                engine: "standard".into(),
                started_at: chrono::Utc::now(),
            })
        })
    });
    assert_eq!(now(cx), Some(RenderState::Rendering(0.4, "r1".into())));
    assert_eq!(RenderState::Outdated.label(), "Out of date");
}
