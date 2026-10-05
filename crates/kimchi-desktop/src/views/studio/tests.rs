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
    assert!(task.is_finished(), "{name} did not answer while the window was being pumped");
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

// ---- moving around ------------------------------------------------------------------------------

/// The open clip's camera (`id`) as the project has it now.
fn camera_of(p: &Project, clip: Id, id: &str) -> kimchi_core::motion::Camera {
    match scene_of(p, clip) {
        Scene::Space(s) => s.camera_by_id(id).cloned().expect("the camera"),
        _ => panic!("3d"),
    }
}

/// A 3D clip with a box and the camera at (0, 2, 8) looking at (0, 1, 0), open in the Studio.
fn open_box(f: &Fixture, cx: &mut VisualTestContext) -> Id {
    let v = f.call("motion.add", json!({ "scene": { "type": "3d", "camera": { "position": [0, 2, 8], "target": [0, 1, 0] }, "objects": [{ "id": "box", "type": "box", "position": [0, 0.5, 0] }] }, "start": 0, "duration": 4 }));
    let clip: Id = v["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    ui(f, cx, "ui.studio", json!({ "clipId": clip.to_string() })).unwrap();
    cx.run_until_parked();
    clip
}

fn len3(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A drag with the left button from `a` to `b`, as the pointer would do it.
fn drag(cx: &mut VisualTestContext, a: gpui::Point<gpui::Pixels>, b: gpui::Point<gpui::Pixels>) {
    use gpui::{Modifiers, MouseButton};
    cx.simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    for k in 1..=4 {
        let p = a + (b - a) * (k as f32 / 4.0);
        cx.simulate_mouse_move(p, Some(MouseButton::Left), Modifiers::none());
    }
    cx.simulate_mouse_up(b, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn the_navigation_gizmo_and_gestures_move_the_view(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TouchPhase};
    let (f, view, cx) = setup(cx);
    open_box(&f, cx);
    let st = studio(&view, cx);
    let vp = cx.update(|_, cx| st.read(cx).viewport.clone());
    let b = cx.update(|_, cx| vp.read(cx).bounds_for_test());
    assert!(b.size.width > px(200.) && b.size.height > px(200.), "{b:?}");
    let (right, top) = (b.origin.x + b.size.width, b.origin.y);
    let shown = |cx: &mut VisualTestContext| cx.update(|_, cx| st.read(cx).view_shown());
    let settle = |cx: &mut VisualTestContext| cx.update(|_, cx| st.update(cx, |s, cx| s.finish_view_anim(cx)));

    // A click on the ball's Y: the view from the top; again: from the bottom.
    let ball = point(right - px(8. + 84.), top + px(8.));
    let y_dot = |v: kimchi_media::render::space::viewport::ViewCamera| {
        let (_, r, u) = v.axes();
        point(ball.x + px((42.0 + r[1] * 29.0) as f32), ball.y + px((42.0 - u[1] * 29.0) as f32))
    };
    let v0 = shown(cx).0;
    cx.simulate_click(y_dot(v0), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(shown(cx).0.aligned_axis(), Some("top"), "the Y axis looks from the top");
    settle(cx);
    let v1 = shown(cx).0;
    assert!((v1.distance() - v0.distance()).abs() < 1e-6, "same distance");
    cx.simulate_click(y_dot(v1), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(shown(cx).0.aligned_axis(), Some("bottom"), "and again from below");
    settle(cx);

    // Dragging the ball orbits.
    let before = shown(cx).0;
    let centre = point(ball.x + px(42.), ball.y + px(60.));
    // (Up and down: it looks from below, where left and right only spin it in place.)
    drag(cx, centre, centre + point(px(20.), px(40.)));
    let after = shown(cx).0;
    assert!(len3(after.position, before.position) > 0.1 && (after.distance() - before.distance()).abs() < 1e-6, "turned around the same point: {before:?} → {after:?}");

    // The buttons under it: orbit, pan, zoom.
    let column = |i: f32| point(ball.x + px(42.), top + px(8. + 84. + 8. + 3. + 15. + 32. * i));
    let before = shown(cx).0;
    drag(cx, column(0.), column(0.) + point(px(-30.), px(10.)));
    let after = shown(cx).0;
    assert!(len3(after.position, before.position) > 0.1 && len3(after.target, before.target) < 1e-9, "the orbit button orbits");
    drag(cx, column(1.), column(1.) + point(px(30.), px(0.)));
    let panned = shown(cx).0;
    assert!(len3(panned.target, after.target) > 0.01, "the hand pans");
    drag(cx, column(2.), column(2.) - point(px(0.), px(40.)));
    let zoomed = shown(cx).0;
    assert!(zoomed.distance() < panned.distance() * 0.9, "dragging the magnifier up comes closer");

    // The wheel zooms to the pointer, Shift+wheel pans, two fingers orbit, a pinch zooms.
    let mid = b.center();
    let scroll = |cx: &mut VisualTestContext, delta: ScrollDelta, modifiers: Modifiers| {
        cx.simulate_event(ScrollWheelEvent { position: mid, delta, modifiers, touch_phase: TouchPhase::Moved });
        cx.run_until_parked();
    };
    let a = shown(cx).0;
    scroll(cx, ScrollDelta::Lines(point(0., 3.)), Modifiers::none());
    let b2 = shown(cx).0;
    assert!(b2.distance() < a.distance(), "the wheel up comes closer");
    scroll(cx, ScrollDelta::Lines(point(0., 3.)), Modifiers::shift());
    let c = shown(cx).0;
    assert!(len3(c.target, b2.target) > 1e-6 && (c.distance() - b2.distance()).abs() < 1e-6, "Shift+wheel pans");
    scroll(cx, ScrollDelta::Pixels(point(px(30.), px(0.))), Modifiers::none());
    let d = shown(cx).0;
    assert!(len3(d.position, c.position) > 1e-3 && len3(d.target, c.target) < 1e-9, "two fingers orbit");
    scroll(cx, ScrollDelta::Pixels(point(px(0.), px(20.))), Modifiers::secondary_key());
    let e = shown(cx).0;
    assert!(e.distance() < d.distance(), "Ctrl/Cmd+scroll zooms");
    cx.simulate_event(gpui::PinchEvent { position: mid, delta: 0.25, modifiers: Modifiers::none(), phase: TouchPhase::Moved });
    cx.run_until_parked();
    assert!(shown(cx).0.distance() < e.distance(), "a pinch zooms");

    // Keys: 7 glides to the top (the state says where it is going), Home frames everything.
    cx.simulate_keystrokes("7");
    cx.run_until_parked();
    assert_eq!(shown(cx).0.aligned_axis(), Some("top"));
    let state = ui(&f, cx, "ui.studio", json!({})).unwrap();
    assert!(state["view"]["position"][1].as_f64().unwrap() > 0.5, "{state}");
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    settle(cx);
    let all = shown(cx).0;
    assert!(len3(all.target, [0.0, 0.5, 0.0]) < 0.1, "the box in the middle: {all:?}");

    // The middle button orbits too, and a right-click (no drag) still opens the menu.
    let before = shown(cx).0;
    cx.simulate_mouse_down(mid, MouseButton::Middle, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(mid + point(px(30.), px(0.)), Some(MouseButton::Middle), Modifiers::none());
    cx.simulate_mouse_up(mid + point(px(30.), px(0.)), MouseButton::Middle, Modifiers::none());
    cx.run_until_parked();
    assert!(len3(shown(cx).0.position, before.position) > 0.05);
    cx.simulate_mouse_down(mid, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_up(mid, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.update(|_, cx| cx.store().read(cx).menu.is_some()), "the right-click menu");
}

#[gpui::test]
fn fly_mode_moves_with_the_keys_and_esc_puts_the_view_back(cx: &mut TestAppContext) {
    use gpui::{KeyDownEvent, KeyUpEvent, Keystroke};
    let (f, view, cx) = setup(cx);
    open_box(&f, cx);
    let st = studio(&view, cx);
    let start = cx.update(|_, cx| st.read(cx).view);
    let state = ui(&f, cx, "ui.studio", json!({ "fly": true })).unwrap();
    cx.run_until_parked();
    assert!(cx.update(|_, cx| st.read(cx).viewport.read(cx).flying()), "{state}");
    let key = |cx: &mut VisualTestContext, k: &str, down: bool| {
        let keystroke = Keystroke::parse(k).unwrap();
        if down {
            cx.simulate_event(KeyDownEvent { keystroke, is_held: false, prefer_character_input: false });
        } else {
            cx.simulate_event(KeyUpEvent { keystroke });
        }
        cx.run_until_parked();
    };
    key(cx, "w", true);
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    key(cx, "w", false);
    let moved = cx.update(|_, cx| st.read(cx).view);
    let (fwd, _, _) = start.axes();
    let step = [moved.position[0] - start.position[0], moved.position[1] - start.position[1], moved.position[2] - start.position[2]];
    assert!(step.iter().zip(fwd).map(|(a, b)| a * b).sum::<f64>() > 0.01, "W flies forward: {step:?}");
    assert!((moved.distance() - start.distance()).abs() < 1e-6, "the eye and what it looks at move together");
    // S isn't Scale while flying, and Esc goes back.
    key(cx, "escape", true);
    assert!(!cx.update(|_, cx| st.read(cx).viewport.read(cx).flying()));
    assert_eq!(cx.update(|_, cx| st.read(cx).view), start, "Esc puts it back");
    assert_eq!(cx.update(|_, cx| st.read(cx).tool), super::Tool::Move);
    // Shift+` starts it; Enter keeps where it went.
    cx.simulate_keystrokes("shift-`");
    cx.run_until_parked();
    assert!(cx.update(|_, cx| st.read(cx).viewport.read(cx).flying()));
    key(cx, "e", true);
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    key(cx, "e", false);
    key(cx, "enter", true);
    let kept = cx.update(|_, cx| st.read(cx).view);
    assert!(kept.position[1] > start.position[1] + 0.01, "E rises, and Enter keeps it: {kept:?}");
}

#[gpui::test]
fn lock_camera_to_view_moves_the_scene_camera_one_step_at_a_time(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let clip = open_box(&f, cx);
    let st = studio(&view, cx);
    let state = ui(&f, cx, "ui.studio", json!({ "lockCamera": true })).unwrap();
    assert_eq!((state["view"].as_str(), state["lockCamera"].as_bool()), (Some("camera"), Some(true)), "locking looks through the camera: {state}");

    // Navigating moves the scene's camera: a command, one undo step.
    ui(&f, cx, "ui.studio", json!({ "navigate": { "orbit": [90, 0] } })).unwrap();
    let x = |p: &Project| camera_of(p, clip, "camera").position.0[0];
    let p = f.settle(cx, |p| x(p).abs() > 7.0);
    let cam = camera_of(&p, clip, "camera");
    assert!((x(&p).abs() - 8.0).abs() < 0.05 && cam.target.0 == [0.0, 1.0, 0.0], "a quarter turn around what it looks at: {:?}", cam.position);
    let steps = f.call("history.list", json!({}));
    assert_eq!((steps["undo"][0]["label"].as_str(), steps["undo"][1]["label"].as_str()), (Some("motion.updateLayer"), Some("motion.add")));

    // A drag on the orbit button while locked: the camera again, one more step.
    let vp = cx.update(|_, cx| st.read(cx).viewport.clone());
    let b = cx.update(|_, cx| vp.read(cx).bounds_for_test());
    let orbit = point(b.origin.x + b.size.width - px(8. + 42.), b.origin.y + px(8. + 84. + 8. + 3. + 15.));
    let before = camera_of(&f.project(), clip, "camera").position.0;
    {
        // One quick move: a slow machine can let the edits of a long drag drift apart in time.
        use gpui::{Modifiers, MouseButton};
        cx.simulate_mouse_down(orbit, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_mouse_move(orbit + point(px(50.), px(0.)), Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(orbit + point(px(50.), px(0.)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }
    let p = f.settle(cx, |p| len3(camera_of(p, clip, "camera").position.0, before) > 0.1);
    assert!(len3(camera_of(&p, clip, "camera").position.0, before) > 0.1, "the camera turned");
    let steps = f.call("history.list", json!({}));
    assert_eq!(steps["undo"][1]["label"], "motion.updateLayer");
    assert_eq!(steps["undo"][2]["label"], "motion.add", "one step for the whole drag");

    // An animated camera gets a keyframe at the playhead.
    f.call("motion.setKeyframes", json!({ "clipId": clip.to_string(), "id": "camera", "property": "position", "keyframes": [[0, [0, 2, 8]], [2, [0, 2, 6]]] }));
    ui(&f, cx, "timeline.seek", json!({ "time": 1.0 })).unwrap();
    cx.run_until_parked();
    ui(&f, cx, "ui.studio", json!({ "navigate": { "zoom": 2 } })).unwrap();
    let keys = |p: &Project| camera_of(p, clip, "camera").keyframes.get("position").map(Vec::len).unwrap_or(0);
    assert_eq!(keys(&f.settle(cx, |p| keys(p) == 3)), 3, "a keyframe at 1 s");

    // Align the camera to a view, add one here, keyframe it.
    ui(&f, cx, "ui.studio", json!({ "lockCamera": false, "view": { "position": [5, 3, 5], "target": [0, 0, 0], "fov": 30, "ortho": false, "orthoSize": 5 } })).unwrap();
    ui(&f, cx, "ui.studio", json!({ "alignCamera": true })).unwrap();
    let p = f.settle(cx, |p| (camera_of(p, clip, "camera").fov - 30.0).abs() < 1e-6);
    assert_eq!(camera_of(&p, clip, "camera").fov, 30.0);
    ui(&f, cx, "ui.studio", json!({ "addCamera": true })).unwrap();
    let cams = |p: &Project| match scene_of(p, clip) {
        Scene::Space(s) => s.cameras.len(),
        _ => 0,
    };
    assert_eq!(cams(&f.settle(cx, |p| cams(p) == 1)), 1);
    let id = match scene_of(&f.project(), clip) {
        Scene::Space(s) => s.cameras[0].id.clone(),
        _ => unreachable!(),
    };
    wait(cx, |cx| cx.update(|_, cx| st.read(cx).selection == [id.clone()]));
    ui(&f, cx, "ui.studio", json!({ "keyframeCamera": true })).unwrap();
    let keyed = |p: &Project| camera_of(p, clip, &id).keyframes.contains_key("target");
    assert!(keyed(&f.settle(cx, keyed)), "the selected camera keyframed");

    // A move from the Camera menu: a turntable, editable keyframes on a constraint.
    cx.update(|_, cx| st.update(cx, |s, cx| s.camera_move("turntable", json!({}), cx)));
    let orbits = |p: &Project| camera_of(p, clip, &id).keyframes.contains_key("constraints.orbit.progress");
    assert!(orbits(&f.settle(cx, orbits)));
}

#[gpui::test]
fn the_2d_canvas_zooms_pans_and_fits(cx: &mut TestAppContext) {
    use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase};
    let (f, view, cx) = setup(cx);
    open(&f, cx, "lowerThird");
    let st = studio(&view, cx);
    let state = ui(&f, cx, "ui.studio", json!({ "zoom": "100%" })).unwrap();
    assert_eq!((state["zoom"].as_f64(), state["fit"].as_bool()), (Some(1.0), Some(false)), "{state}");
    cx.simulate_keystrokes("=");
    cx.run_until_parked();
    assert!((cx.update(|_, cx| st.read(cx).canvas.zoom) - 1.25).abs() < 1e-9, "= zooms in");
    cx.simulate_keystrokes("/");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| st.read(cx).canvas.zoom), 1.0, "/ is 100%");
    let vp = cx.update(|_, cx| st.read(cx).viewport.clone());
    let mid = cx.update(|_, cx| vp.read(cx).bounds_for_test()).center();
    // Two fingers pan; Ctrl/Cmd+scroll zooms to the pointer.
    cx.simulate_event(ScrollWheelEvent { position: mid, delta: ScrollDelta::Pixels(point(px(40.), px(-20.))), modifiers: Modifiers::none(), touch_phase: TouchPhase::Moved });
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| st.read(cx).canvas.pan), [40.0, -20.0]);
    cx.simulate_event(ScrollWheelEvent { position: mid, delta: ScrollDelta::Pixels(point(px(0.), px(50.))), modifiers: Modifiers::secondary_key(), touch_phase: TouchPhase::Moved });
    cx.run_until_parked();
    assert!(cx.update(|_, cx| st.read(cx).canvas.zoom) > 1.1);
    let state = ui(&f, cx, "ui.studio", json!({ "zoom": "fit" })).unwrap();
    assert_eq!(state["fit"], true);
    assert!(ui(&f, cx, "ui.studio", json!({ "navigate": { "orbit": [1, 1] } })).unwrap_err().contains("3D"));
}

#[gpui::test]
fn the_studio_keeps_room_for_the_view_when_resized(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    open_box(&f, cx);
    let st = studio(&view, cx);
    cx.update(|_, cx| st.update(cx, |s, _| { s.left_w = 480.; s.right_w = 560.; }));
    for (w, h) in [(640., 480.), (800., 600.), (1024., 768.), (1200., 800.), (1600., 1000.)] {
        crate::tests::resize(cx, w, h);
        let b = cx.update(|_, cx| st.read(cx).viewport.read(cx).bounds_for_test());
        assert!(b.size.width >= px(320.) && b.size.height >= px(170.), "{w}×{h}: {b:?}");
        assert!(b.origin.x >= px(0.) && b.origin.x + b.size.width <= px(w), "{b:?}");
        if w < 960. {
            assert_eq!(ui(&f, cx, "ui.studio", json!({"panel": "properties"})).unwrap()["panel"], "properties");
            assert_eq!(ui(&f, cx, "ui.studio", json!({"panel": "objects"})).unwrap()["panel"], "objects");
            ui(&f, cx, "ui.studio", json!({"panel": "none"})).unwrap();
        }
    }
    ui(&f, cx, "ui.action", json!({"action": "ToggleAgent"})).unwrap();
    crate::tests::resize(cx, 1200., 800.);
    let b = cx.update(|_, cx| st.read(cx).viewport.read(cx).bounds_for_test());
    assert!(b.size.width >= px(320.), "the docked agent must leave room for the Studio: {b:?}");
}
