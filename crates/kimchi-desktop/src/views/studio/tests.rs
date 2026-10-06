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
    while !task.is_finished() && start.elapsed() < crate::tests::PATIENCE {
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

#[gpui::test]
fn selection_operations_combine_objects_and_mesh_components_without_edits(cx:&mut TestAppContext) {
    let (f,_,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","vertices":[[-1,-1,0],[1,-1,0],[1,1,0],[-1,1,0]],"faces":[[0,1,2],[0,2,3]]},
        {"id":"other","type":"box","position":[3,0,0]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let before=f.project();let history=f.call("history.list",json!({}));
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["mesh"]})).unwrap();
    let state=ui(&f,cx,"ui.studio",json!({"select":["other","other"],"selectionOp":"add"})).unwrap();
    assert_eq!(state["selection"],json!(["mesh","other"]));assert_eq!(state["active"],"other");
    let state=ui(&f,cx,"ui.studio",json!({"select":["mesh"],"selectionOp":"subtract"})).unwrap();
    assert_eq!(state["selection"],json!(["other"]));
    let state=ui(&f,cx,"ui.studio",json!({"select":[],"selectionOp":"add"})).unwrap();
    assert_eq!(state["selection"],json!(["other"]));
    assert!(ui(&f,cx,"ui.studio",json!({"select":["mesh"],"selectionOp":"unknown"})).unwrap_err().contains("selectionOp"));
    assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["selection"],json!(["other"]));
    let state=ui(&f,cx,"ui.studio",json!({"select":["mesh"],"mode":"edit","selectMode":"face","editSelection":{"faces":[0]}})).unwrap();
    assert_eq!(state["selection"],json!(["mesh"]),"selectionOp applies only to its call");
    let state=ui(&f,cx,"ui.studio",json!({"editSelection":{"faces":[1,0]},"selectionOp":"add"})).unwrap();
    assert_eq!(state["editSelection"]["faces"],json!([0,1]));
    let state=ui(&f,cx,"ui.studio",json!({"editSelection":{"faces":[0]},"selectionOp":"subtract"})).unwrap();
    assert_eq!(state["editSelection"]["faces"],json!([1]));
    assert!(ui(&f,cx,"ui.studio",json!({"editSelection":{"faces":[99]},"selectionOp":"subtract"})).is_err());
    assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["editSelection"]["faces"],json!([1]));
    ui(&f,cx,"ui.studio",json!({"selectMode":"edge","editSelection":{"edges":[[0,1],[1,2]]}})).unwrap();
    let state=ui(&f,cx,"ui.studio",json!({"editSelection":{"edges":[[1,0]]},"selectionOp":"subtract"})).unwrap();
    assert_eq!(state["editSelection"]["edges"],json!([[1,2]]));
    let state=ui(&f,cx,"ui.studio",json!({"editSelection":{}})).unwrap();
    assert_eq!(state["editSelection"],json!({"vertices":[],"edges":[],"faces":[]}));
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
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
    while !done(cx) && start.elapsed() < crate::tests::PATIENCE {
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
    cx.update(|_, cx| {
        let sidebar = st.read(cx).sidebar.clone();
        sidebar.update(cx, |s, cx| s.show_properties(cx));
        props.update(cx, |p, cx| p.set_tab(super::properties::Tab::Modifiers, cx));
    });
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
    assert_eq!(steps["undo"][0]["label"], "motion.updateKeyframes");
    // The selection follows the keys.
    wait(cx, |cx| cx.update(|_, cx| (st.read(cx).keys[0].time - (time - 0.5)).abs() < 1e-6));
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
    // The Studio's key context is installed by painting the newly opened view.
    cx.run_until_parked();
    cx.simulate_keystrokes("g");
    let viewport=cx.update(|_,cx| st.read(cx).viewport.clone());
    wait(cx,|cx| cx.update(|_,cx| viewport.read(cx).busy()));
    assert!(cx.update(|_,cx| viewport.read(cx).busy()),"the deferred move tool is ready for numeric input");
    cx.simulate_keystrokes("x 2 enter");
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
fn opening_a_duplicate_project_drops_old_studio_tools_and_queued_edits(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[{"id":"box","type":"box","position":[1,0,0]}]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    let original=f.project();let before=scene_of(&original,clip);
    let duplicate:Id=f.call("project.duplicate",json!({}))["id"].as_str().unwrap().parse().unwrap();
    let st=studio(&view,cx);let vp=cx.update(|_,cx| st.read(cx).viewport.clone());
    for queued in [false,true] {
        f.call("project.open",json!({"projectId":original.id}));
        store_settles(cx,|s| s.project.as_ref().is_some_and(|p|p.id==original.id));
        cx.update(|w,cx| {
            st.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["box".into()],cx);});
            vp.update(cx,|v,cx| v.start_modal_now(super::viewport::ModalKind::Grab,cx));
            if queued {
                st.update(cx,|s,cx| {
                    for x in [2,3] {s.send(vec![("motion.updateLayer".into(),json!({"clipId":clip,"id":"box","props":{"position":[x,0,0]}}))],cx);}
                    assert!(s.sender.queued.is_some());
                });
            }
            // Switch before any deferred Studio command can begin, with the same clip IDs.
            f.call("project.open",json!({"projectId":duplicate}));
        });
        store_settles(cx,|s|s.project.as_ref().is_some_and(|p|p.id==duplicate));
        wait(cx,|cx|cx.update(|_,cx| !st.read(cx).sender.busy && !st.read(cx).is_open()));
        cx.update(|_,cx| {
            assert!(!st.read(cx).is_open());assert!(!vp.read(cx).busy());assert!(st.read(cx).sender.queued.is_none());
        });
        assert_eq!(scene_of(&f.project(),clip),before);
        assert_eq!(scene_of(&f.session.library.load(original.id).unwrap(),clip),before);
        assert_eq!(f.call("history.list",json!({}))["undo"],json!([]));
    }
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["box"]})).unwrap();
    cx.run_until_parked();cx.simulate_keystrokes("g");
    wait(cx,|cx|cx.update(|_,cx|vp.read(cx).busy()));cx.simulate_keystrokes("x 2 enter");
    let moved=|p:&Project| matches!(scene_of(p,clip),Scene::Space(s) if s.objects[0].position.0[0]==3.);
    assert!(moved(&f.settle(cx,moved)),"the new project accepts a fresh gesture");
    assert_eq!(scene_of(&f.session.library.load(original.id).unwrap(),clip),before);
}

#[gpui::test]
fn switching_clips_ends_modal_tools_without_editing_the_next_scene(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let st=studio(&view,cx);let vp=cx.update(|_,cx| st.read(cx).viewport.clone());
    for (offset,three,action,number) in [(0,true,"g","x 2"),(20,false,"r","3 0")] {
        let mut clips=vec![];
        for (start,value) in [(offset,1),(offset+5,10)] {
            let scene=if three {json!({"type":"3d","objects":[{"id":"box","type":"box","position":[value,0,0]}]})}
                else {json!({"layers":[{"id":"box","type":"rect","width":100,"height":100,"rotation":value}]})};
            let added=f.call("motion.add",json!({"start":start,"duration":4,"scene":scene}));
            clips.push(added["clips"][0]["id"].as_str().unwrap().parse::<Id>().unwrap());
        }
        store_settles(cx,|s| s.clip(clips[1]).is_some());
        let next=scene_of(&f.project(),clips[1]);
        ui(&f,cx,"ui.studio",json!({"clipId":clips[0],"select":["box"]})).unwrap();
        cx.run_until_parked();cx.simulate_keystrokes(action);
        wait(cx,|cx| cx.update(|_,cx| vp.read(cx).busy()));
        assert!(cx.update(|_,cx| vp.read(cx).busy()));
        cx.simulate_keystrokes(number);
        let changed=|p:&Project| match scene_of(p,clips[0]) {
            Scene::Space(s)=>s.objects[0].position.0[0]==3.,Scene::Flat(s)=>s.layers[0].rotation==31.
        };
        let transformed=f.settle(cx,changed);
        assert!(changed(&transformed),"the initial {action} transform applies before switching clips: {:?}",scene_of(&transformed,clips[0]));
        let previous=scene_of(&f.project(),clips[0]);
        ui(&f,cx,"ui.studio",json!({"clipId":clips[1],"select":["box"]})).unwrap();
        cx.run_until_parked();
        assert!(!cx.update(|_,cx| vp.read(cx).busy()),"a tool from the previous clip cannot keep capturing input");
        let center=cx.update(|_,cx| vp.read(cx).bounds_for_test().center());
        cx.simulate_mouse_move(center+point(px(40.),px(20.)),None,gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(scene_of(&f.project(),clips[1]),next,"matching object ids do not transfer a gesture between scenes");
        assert_eq!(scene_of(&f.project(),clips[0]),previous,"completed edits stay in their original clip");
        cx.simulate_keystrokes(action);wait(cx,|cx| cx.update(|_,cx| vp.read(cx).busy()));
        assert!(cx.update(|_,cx| vp.read(cx).busy()),"the new scene can start its own tools");
        cx.simulate_keystrokes("escape");cx.run_until_parked();
        assert_eq!(scene_of(&f.project(),clips[1]),next);
    }
}

#[gpui::test]
fn layer_numeric_transforms_preserve_precision_axis_switching_and_animation(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"layers":[
        {"id":"a","type":"rect","width":100,"height":100,"rotation":15,"scale":2,"scaleX":0.75,"scaleY":1.5,
         "keyframes":{"rotation":[[0,10],[2,50]],"scale":[[0,2],[2,4]],"scaleX":[[0,0.5],[2,1.5]]}},
        {"id":"b","type":"rect","width":40,"height":40,"x":150,"rotation":30}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["a","b"],"panel":"none"})).unwrap();
    let st=studio(&view,cx);let vp=cx.update(|_,cx| st.read(cx).viewport.clone());
    let before=scene_of(&f.project(),clip);let history=f.call("history.list",json!({}));
    let start=|key:&str,cx:&mut VisualTestContext| {
        cx.simulate_keystrokes(key);wait(cx,|cx| cx.update(|_,cx| vp.read(cx).busy()));
        assert!(cx.update(|_,cx| vp.read(cx).busy()));
    };
    let layers=|p:&Project| match scene_of(p,clip) {Scene::Flat(s)=>s.layers,_=>panic!()};
    cx.run_until_parked();start("s",cx);cx.simulate_keystrokes("3");
    assert_eq!(layers(&f.settle(cx,|p| layers(p)[0].scale==6.))[1].scale,3.);
    cx.simulate_keystrokes("x");
    let x=layers(&f.settle(cx,|p| layers(p)[0].scale_x==1.5));
    assert_eq!((x[0].scale,x[0].scale_y,x[1].scale,x[1].scale_x),(2.,1.5,1.,3.));
    let Scene::Flat(original)=&before else {panic!()};
    assert_eq!(x[0].keyframes["scale"],original.layers[0].keyframes["scale"]);
    cx.simulate_keystrokes("y");
    let y=layers(&f.settle(cx,|p| layers(p)[0].scale_y==4.5));
    assert_eq!((y[0].scale,y[0].scale_x,y[1].scale_x,y[1].scale_y),(2.,0.75,1.,3.));
    assert_eq!(y[0].keyframes["scaleX"],original.layers[0].keyframes["scaleX"]);
    cx.simulate_keystrokes("y enter");
    let uniform=layers(&f.settle(cx,|p| layers(p)[0].scale==6.));
    assert_eq!((uniform[0].scale_x,uniform[0].scale_y,uniform[1].scale),(0.75,1.5,3.));
    assert_eq!(uniform[0].keyframes["scaleX"],original.layers[0].keyframes["scaleX"]);
    assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    f.call("history.undo",json!({}));store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    start("r",cx);cx.simulate_keystrokes("- enter");
    let center=cx.update(|_,cx| vp.read(cx).bounds_for_test().center());
    cx.simulate_click(center,gpui::Modifiers::none());cx.run_until_parked();
    assert!(cx.update(|_,cx| vp.read(cx).busy()),"neither Enter nor a click confirms incomplete 2D input");
    assert_eq!(scene_of(&f.project(),clip),before);
    cx.simulate_keystrokes("backspace 1 e - 4 enter");
    let rotated=layers(&f.settle(cx,|p| (layers(p)[0].rotation-10.0001).abs()<1e-9));
    assert!((rotated[0].rotation-10.0001).abs()<1e-9 && (rotated[1].rotation-30.0001).abs()<1e-9,"small typed rotations keep their precision");
    f.call("history.undo",json!({}));store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    assert_eq!(scene_of(&f.project(),clip),before);
}

#[gpui::test]
fn viewport_numeric_transforms_reject_incomplete_input_and_preserve_exact_values(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[{"id":"box","type":"box","position":[1,0,0]}]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    assert_eq!(ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["box"],"snapping":true})).unwrap()["snapping"],true);
    let st=studio(&view,cx);
    let vp=cx.update(|_,cx| st.read(cx).viewport.clone());
    let before=scene_of(&f.project(),clip);
    let history=f.call("history.list",json!({}));
    cx.run_until_parked();
    cx.simulate_keystrokes("g");
    wait(cx,|cx| cx.update(|_,cx| vp.read(cx).busy()));
    assert!(cx.update(|_,cx| vp.read(cx).busy()));
    cx.simulate_keystrokes("x - enter");cx.run_until_parked();
    assert!(cx.update(|_,cx| vp.read(cx).busy()),"incomplete input cannot confirm");
    assert_eq!(scene_of(&f.project(),clip),before);
    cx.simulate_keystrokes("2");
    let x=|p:&Project| match scene_of(p,clip) {Scene::Space(s)=>s.objects[0].position.0[0],_=>panic!()};
    assert!((x(&f.settle(cx,|p| x(p)==-1.))+1.).abs()<1e-8);
    cx.simulate_keystrokes("e - enter");cx.run_until_parked();
    assert!(cx.update(|_,cx| vp.read(cx).busy()),"an unfinished exponent also cannot confirm");
    assert_eq!(x(&f.project()),-1.);
    cx.simulate_keystrokes("2 enter");
    assert!((x(&f.settle(cx,|p| (x(p)-0.98).abs()<1e-8))-0.98).abs()<1e-8,"typed scientific notation bypasses grid snapping");
    assert!(!cx.update(|_,cx| vp.read(cx).busy()));
    assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    f.call("history.undo",json!({}));store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    assert_eq!(scene_of(&f.project(),clip),before);
    cx.simulate_keystrokes("g");wait(cx,|cx| cx.update(|_,cx| vp.read(cx).busy()));
    cx.simulate_keystrokes("x 3");
    assert_eq!(x(&f.settle(cx,|p| x(p)==4.)),4.);
    cx.simulate_keystrokes("e 9 9 9 enter");cx.run_until_parked();
    assert!(cx.update(|_,cx| vp.read(cx).busy()),"overflow cannot confirm");
    assert!(x(&f.project()).is_finite());
    cx.simulate_keystrokes("escape");
    assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==before),clip),before,"Escape restores the scene even after invalid input");
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

#[gpui::test]
fn mesh_selection_commands_validate_convert_frame_and_preserve_exact_edges(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let v = f.call("motion.add", json!({ "scene": { "type": "3d", "objects": [{
        "id": "panel", "type": "mesh", "vertices": [[0,0,0],[1,0,0],[1,1,0],[0,1,0]], "faces": [[0,1,2,3]]
    }] }, "duration": 4 }));
    let clip: Id = v["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    ui(&f, cx, "ui.studio", json!({ "clipId": clip, "select": ["panel"], "mode": "edit", "selectMode": "face", "meshSelect": "all", "meshTool": "extrude" })).unwrap();
    let state = ui(&f, cx, "ui.studio", json!({ "selectMode": "edge" })).unwrap();
    assert_eq!(state["editSelection"]["edges"].as_array().unwrap().len(), 4);
    assert!(state["editSelection"]["faces"].as_array().unwrap().is_empty());
    let before = state["editSelection"].clone();
    for selection in [json!({"vertices":[999]}), json!({"edges":[[0,2]]}), json!({"faces":[-1]}), json!({"faces":[4294967296u64]})] {
        assert!(ui(&f, cx, "ui.studio", json!({ "editSelection": selection })).is_err());
        assert_eq!(ui(&f, cx, "ui.studio", json!({})).unwrap()["editSelection"], before);
    }
    let state = ui(&f, cx, "ui.studio", json!({ "editSelection": { "edges": [[0,1],[2,3]] }, "frame": true })).unwrap();
    assert_eq!(state["editSelection"]["edges"], json!([[0,1],[2,3]]));
    let st = studio(&view, cx);
    cx.update(|_, cx| st.update(cx, |s, cx| s.mesh_op("extrude", json!({"offset":[0,0,1]}), cx)));
    let faces = |p: &Project| match scene_of(p, clip) {
        Scene::Space(s) => match &s.objects[0].shape { kimchi_core::motion::Shape3d::Mesh { faces, .. } => faces.len(), _ => 0 }, _ => 0,
    };
    assert_eq!(faces(&f.settle(cx, |p| faces(p) == 3)), 3);
    wait(cx, |cx| cx.update(|_, cx| st.read(cx).mesh_feedback.is_some()));
    assert!(cx.update(|_, cx| st.read(cx).mesh_feedback.as_ref().unwrap().is_ok()));
    f.call("history.undo", json!({}));
    assert_eq!(faces(&f.settle(cx, |p| faces(p) == 1)), 1);
    wait(cx, |cx| cx.update(|_, cx| st.read(cx).edit_sel.vertices.iter().all(|&i| i < 4)));
    assert!(cx.update(|_, cx| st.read(cx).edit_sel.vertices.iter().all(|&i| i < 4)));
    ui(&f, cx, "ui.studio", json!({"editSelection":{"vertices":[0]}, "selectMode":"vertex", "frame":true})).unwrap();
    assert_eq!(cx.update(|_, cx| st.read(cx).view.target), [0.,0.,0.], "frame targets the picked vertex");
}

#[gpui::test]
fn mesh_edge_paths_extend_exact_edges_without_touching_the_project(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let vertices:Vec<_>=(0..3).flat_map(|z| (0..3).map(move |x| [x as f64,0.,z as f64])).collect();
    let added=f.call("motion.add",json!({"scene":{"type":"3d","objects":[{
        "id":"grid","type":"mesh","vertices":vertices,"faces":[[0,1,4,3],[1,2,5,4],[3,4,7,6],[4,5,8,7]]
    }]},"duration":4}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["grid"],"mode":"edit","selectMode":"edge","editSelection":{"edges":[[1,4]]}})).unwrap();
    let before=scene_of(&f.project(),clip);
    let history=f.call("history.list",json!({}));
    let state=ui(&f,cx,"ui.studio",json!({"meshSelect":"edgeLoop"})).unwrap();
    assert_eq!(state["editSelection"],json!({"vertices":[],"faces":[],"edges":[[1,4],[4,7]]}));
    let state=ui(&f,cx,"ui.studio",json!({"editSelection":{"edges":[[1,4],[4,1]]},"meshSelect":"edgeRing"})).unwrap();
    assert_eq!(state["editSelection"],json!({"vertices":[],"faces":[],"edges":[[0,3],[1,4],[2,5]]}));
    let state=ui(&f,cx,"ui.studio",json!({"meshSelect":"edgeRing"})).unwrap();
    assert_eq!(state["editSelection"]["edges"],json!([[0,3],[1,4],[2,5]]),"repeating a ring is stable");
    assert_eq!(scene_of(&f.project(),clip),before);
    assert_eq!(f.call("history.list",json!({})),history);
    let st=studio(&view,cx);
    cx.update(|_,cx| st.update(cx,|s,cx| s.mesh_op("extrude",json!({"offset":[0,1,0]}),cx)));
    let after=f.settle(cx,|p| scene_of(p,clip)!=before);
    let Scene::Space(scene)=scene_of(&after,clip) else {panic!()};
    let kimchi_core::motion::Shape3d::Mesh {faces,..}=&scene.objects[0].shape else {panic!()};
    assert_eq!(faces.len(),7,"extrusion follows only the three ring edges");
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).mesh_feedback.is_some()));
    f.call("history.undo",json!({}));
    assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==before),clip),before);
    ui(&f,cx,"ui.studio",json!({"selectMode":"edge","editSelection":{}})).unwrap();
    assert!(ui(&f,cx,"ui.studio",json!({"meshSelect":"edgeRing"})).unwrap_err().contains("edge first"));
    ui(&f,cx,"ui.studio",json!({"selectMode":"face","editSelection":{"faces":[0]}})).unwrap();
    assert!(ui(&f,cx,"ui.studio",json!({"meshSelect":"edgeLoop"})).unwrap_err().contains("edge selection mode"));
    assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["editSelection"]["faces"],json!([0]));
}

#[gpui::test]
fn transform_pivot_from_the_toolbar_state_changes_keyboard_rotations_and_undo(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let value = f.call("motion.add", json!({"scene":{"type":"3d","objects":[
        {"id":"a","type":"box","position":[1,0,0]}, {"id":"b","type":"box","position":[3,0,0]}
    ]},"duration":4}));
    let clip: Id = value["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    let state = ui(&f, cx, "ui.studio", json!({"clipId":clip,"select":["a","b"],"pivot":"active","gizmo":"global"})).unwrap();
    assert_eq!(state["pivot"], "active");
    assert!(ui(&f, cx, "ui.studio", json!({"pivot":"typo"})).is_err());
    let st = studio(&view, cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("r");
    let viewport=cx.update(|_,cx| st.read(cx).viewport.clone());
    wait(cx,|cx| cx.update(|_,cx| viewport.read(cx).session.is_some()));
    assert!(cx.update(|_,cx| viewport.read(cx).session.is_some()),"the rotation tool starts before typing its value");
    cx.run_until_parked();
    cx.simulate_keystrokes("z 9 0");
    assert_eq!(cx.update(|_,cx| viewport.read(cx).session.as_ref().map(|s| s.typed.clone())),Some("90".into()));
    cx.simulate_keystrokes("enter");
    let y = |p: &Project| model::value_at(&scene_of(p, clip), "a", "position.y", 0.).and_then(|v| v.as_f64()).unwrap();
    let p = f.settle(cx, |p| (y(p) + 2.).abs() < 1e-6);
    assert!((y(&p) + 2.).abs() < 1e-6, "y={}, scene={}", y(&p), scene_of(&p, clip).to_json());
    assert_eq!(model::value_at(&scene_of(&p, clip), "b", "position.x", 0.).and_then(|v| v.as_f64()), Some(3.));
    wait(cx, |cx| cx.update(|_, cx| !st.read(cx).sender.busy));
    f.call("history.undo", json!({}));
    let p = f.settle(cx, |p| y(p).abs() < 1e-6);
    assert!(y(&p).abs() < 1e-6, "the whole transform undoes together");
}

#[gpui::test]
fn duplicating_a_hierarchy_selects_only_copied_roots_and_undoes_together(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let value = f.call("motion.add", json!({"scene":{"type":"3d","objects":[
        {"id":"root","type":"box","children":[{"id":"child","type":"sphere"}]},
        {"id":"other","type":"box","position":[3,0,0]}
    ]},"duration":4}));
    let clip: Id = value["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    ui(&f, cx, "ui.studio", json!({"clipId":clip,"select":["child","root","other"]})).unwrap();
    let before = scene_of(&f.project(), clip);
    let st = studio(&view, cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-d");
    let copied = |p: &Project| scene_of(p, clip).ids().len() == 6;
    assert!(copied(&f.settle(cx, copied)), "both roots and the child are copied once");
    wait(cx, |cx| cx.update(|_, cx| st.read(cx).selection == ["root2", "other2"]));
    assert_eq!(cx.update(|_, cx| st.read(cx).selection.clone()), ["root2", "other2"]);
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    wait(cx, |cx| cx.update(|_, cx| !st.read(cx).sender.busy));
    let scene = scene_of(&f.project(), clip);
    assert_eq!(model::value_at(&scene, "other2", "position.x", 0.).and_then(|v| v.as_f64()), Some(3.));
    assert_eq!(f.call("history.list", json!({}))["undo"][0]["label"], "motion.duplicateLayers");
    f.call("history.undo", json!({}));
    let restored = scene_of(&f.settle(cx, |p| scene_of(p, clip) == before), clip);
    assert_eq!(restored.ids(), before.ids(), "history: {}", f.call("history.list", json!({})));
    assert_eq!(restored, before);
    store_settles(cx, |s| s.clip(clip).is_some_and(|c| matches!(&c.content, ClipContent::Motion {scene, ..} if scene.ids().len() == 3)));
    ui(&f, cx, "ui.studio", json!({"select":["root","other"]})).unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-d");
    assert!(copied(&f.settle(cx, copied)));
    let viewport = cx.update(|_, cx| st.read(cx).viewport.clone());
    wait(cx, |cx| cx.update(|_, cx| viewport.read(cx).session.is_some()));
    assert!(cx.update(|_, cx| viewport.read(cx).session.is_some()), "duplication starts moving the new objects");
    cx.run_until_parked();
    cx.simulate_keystrokes("y 2");
    assert_eq!(cx.update(|_, cx| viewport.read(cx).session.as_ref().map(|s| (s.typed.clone(), s.amount))), Some(("2".into(),2.)), "the modal tool receives the typed move");
    cx.simulate_keystrokes("enter");
    wait(cx, |cx| cx.update(|_, cx| !st.read(cx).sender.busy));
    let y = |p: &Project, id: &str| model::value_at(&scene_of(p, clip), id, "position.y", 0.).and_then(|v| v.as_f64());
    let moved = f.settle(cx, |p| y(p, "root2") == Some(2.) && y(p, "other2") == Some(2.));
    assert_eq!((y(&moved, "root2"), y(&moved, "other2")), (Some(2.),Some(2.)));
    assert_eq!((y(&moved, "root"), y(&moved, "other")), (Some(0.),Some(0.)), "originals stay put");
}

#[gpui::test]
fn deleting_linked_selections_is_atomic_and_keeps_selection_on_failure(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let value = f.call("motion.add", json!({"scene":{"layers":[
        {"id":"root","type":"group","layers":[{"id":"child","type":"rect"}]},
        {"id":"follower","type":"ellipse","parent":"root"},
        {"id":"card","type":"comp","comp":"nested"}
    ],"compositions":[{"id":"nested","layers":[]}]},"duration":4}));
    let clip: Id = value["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    let st = studio(&view, cx);
    let selection = json!(["root","child","follower","comp:nested"]);
    ui(&f, cx, "ui.studio", json!({"clipId":clip,"select":selection})).unwrap();
    let before = scene_of(&f.project(), clip);
    cx.update(|w, cx| st.update(cx, |s, cx| s.delete(w, cx)));
    let failed = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.store().read(cx).toasts.iter().any(|t| t.text.contains("Nothing was changed")));
    wait(cx, failed);
    assert!(failed(cx), "removing a composition still in use reports the rollback");
    assert_eq!(scene_of(&f.project(), clip), before, "earlier removals were rolled back");
    assert_eq!(json!(cx.update(|_, cx| st.read(cx).selection.clone())), selection, "selection survives the rollback");
    ui(&f, cx, "ui.studio", json!({"select":["root","child","follower"]})).unwrap();
    cx.update(|w, cx| st.update(cx, |s, cx| s.delete(w, cx)));
    let removed = |p: &Project| scene_of(p, clip).ids() == ["card"];
    assert!(removed(&f.settle(cx, removed)));
    wait(cx, |cx| cx.update(|_, cx| st.read(cx).selection.is_empty()));
    assert!(cx.update(|_, cx| st.read(cx).selection.is_empty()));
    f.call("history.undo", json!({}));
    assert_eq!(scene_of(&f.settle(cx, |p| scene_of(p, clip) == before), clip), before);
}

#[gpui::test]
fn outliner_shift_click_extends_and_shrinks_ranges_from_the_anchor(cx: &mut TestAppContext) {
    use gpui::Modifiers;
    let (f, view, cx) = setup(cx);
    let value = f.call("motion.add", json!({"scene":{"type":"3d","objects":[
        {"id":"a","type":"box"}, {"id":"b","type":"group","children":[{"id":"child","type":"sphere"}]},
        {"id":"c","type":"box"}, {"id":"d","type":"box"}
    ]},"duration":4}));
    let clip: Id = value["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    ui(&f, cx, "ui.studio", json!({"clipId":clip})).unwrap();
    let st = studio(&view, cx);
    cx.update(|_, cx| st.update(cx, |s, cx| { s.collapsed.insert("b".into()); s.changed(cx); }));
    let outliner = cx.update(|_, cx| st.read(cx).outliner.clone());
    let click = |cx: &mut VisualTestContext, key: &str, shift: bool, control: bool| cx.update(|w, cx| outliner.update(cx, |o, cx| {
        o.click_with_modifiers(key, Modifiers { shift, control, ..Default::default() }, w, cx);
    }));
    let history = f.call("history.list", json!({}));
    click(cx, "a", false, false);
    click(cx, "c", true, false);
    assert_eq!(cx.update(|_, cx| st.read(cx).selection.clone()), ["a","b","c"]);
    click(cx, "b", true, false);
    assert_eq!(cx.update(|_, cx| st.read(cx).selection.clone()), ["a","b"], "the original anchor survives repeated range selections");
    click(cx, "d", false, true);
    click(cx, "a", true, true);
    assert_eq!(cx.update(|_, cx| st.read(cx).selection.clone()), ["b","d","c","a"]);
    click(cx, "b", false, true);
    assert_eq!(cx.update(|_, cx| st.read(cx).selection.clone()), ["d","c","a"], "Ctrl-click also removes a selected nonactive item");
    assert_eq!(f.call("history.list", json!({})), history, "selection changes do not edit the project");
}

#[gpui::test]
fn outliner_drags_move_the_selection_in_order_and_undo_together(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let scenes=[json!({"type":"3d","objects":[
        {"id":"a","type":"group","children":[{"id":"child","type":"box"}]},
        {"id":"b","type":"box"},{"id":"c","type":"sphere"},{"id":"target","type":"group"}
    ]}),json!({"layers":[
        {"id":"a","type":"group","layers":[{"id":"child","type":"rect"}]},
        {"id":"b","type":"rect"},{"id":"c","type":"ellipse"},{"id":"target","type":"group"}
    ]})];
    for scene in scenes {
        let value=f.call("motion.add",json!({"scene":scene,"duration":4}));
        let clip:Id=value["clips"][0]["id"].as_str().unwrap().parse().unwrap();
        store_settles(cx,|s| s.clip(clip).is_some());
        ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["c","child","a"]})).unwrap();
        let st=studio(&view,cx);
        let outliner=cx.update(|_,cx| st.read(cx).outliner.clone());
        let before=scene_of(&f.project(),clip);
        let drop=|cx:&mut VisualTestContext,target:&str,place:i8| cx.update(|w,cx| outliner.update(cx,|o,cx| o.drop_for_test("a",target,place,w,cx)));
        cx.update(|w,cx| outliner.update(cx,|o,cx| o.begin_drag_for_test("a","target",0,w,cx)));
        cx.simulate_keystrokes("escape");
        cx.update(|w,cx| outliner.update(cx,|o,cx| o.finish_drag_for_test(w,cx)));
        cx.run_until_parked();
        assert!(cx.update(|_,cx| st.read(cx).clip.is_some()),"Escape cancels the drag before closing Studio");
        assert_eq!(scene_of(&f.project(),clip),before);
        drop(cx,"child",0);
        cx.run_until_parked();
        assert_eq!(scene_of(&f.project(),clip),before,"a descendant is not a valid drop target");
        drop(cx,"target",0);
        let after=f.settle(cx,|p| scene_of(p,clip)!=before);
        let after=scene_of(&after,clip).to_json();
        let (objects,children)=if before.is_3d() {("objects","children")} else {("layers","layers")};
        assert_eq!(after[objects][0]["id"],"b");
        assert_eq!(after[objects][1][children][0]["id"],"a");
        assert_eq!(after[objects][1][children][1]["id"],"c");
        assert_eq!(after[objects][1][children][0][children][0]["id"],"child");
        assert_eq!(cx.update(|_,cx| st.read(cx).selection.clone()),["c","child","a"]);
        f.call("history.undo",json!({}));
        assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==before),clip),before);
        // Reordering a noncontiguous selection counts every removed root, in either display direction.
        drop(cx,"target",if before.is_3d() {-1} else {1});
        let after=f.settle(cx,|p| scene_of(p,clip)!=before);
        let after=scene_of(&after,clip).to_json();
        assert_eq!(after[objects].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(),["b","a","c","target"]);
        f.call("history.undo",json!({}));
        assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==before),clip),before);
    }
}

#[gpui::test]
fn outliner_arrow_navigation_follows_visible_rows_and_scrolls_the_selection(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    crate::tests::resize(cx,1400.,900.);
    let mut objects=vec![json!({"id":"A","type":"group","children":[{"id":"child1","type":"box"},{"id":"child2","type":"sphere"}]}),json!({"id":"B","type":"box"})];
    objects.extend((0..80).map(|i| json!({"id":format!("tail-{i}"),"type":"group"})));
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":objects}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["A"],"panel":"objects"})).unwrap();
    let st=studio(&view,cx);
    let outliner=cx.update(|_,cx| st.read(cx).outliner.clone());
    cx.run_until_parked();
    let row=cx.update(|_,cx| outliner.read(cx).row_position_for_test("A",cx));
    cx.simulate_click(row,gpui::Modifiers::none());cx.run_until_parked();
    let before=scene_of(&f.project(),clip);let history=f.call("history.list",json!({}));
    let press=|key:&str,expected:&[&str],cx:&mut VisualTestContext| {
        cx.simulate_keystrokes(key);cx.run_until_parked();
        assert_eq!(cx.update(|_,cx| st.read(cx).selection.clone()),expected,"after {key}");
    };
    ui(&f,cx,"ui.studio",json!({"selectedKeys":[]})).unwrap();
    cx.simulate_click(row-point(px(62.),px(0.)),gpui::Modifiers::none());cx.run_until_parked();
    assert_eq!(cx.update(|_,cx| st.read(cx).area),super::Area::Outliner,"the fold icon also takes Outliner focus");
    assert!(cx.update(|_,cx| st.read(cx).collapsed.contains("A")));
    press("right",&["A"],cx);
    press("down",&["child1"],cx);
    press("shift-down",&["child1","child2"],cx);
    press("shift-up",&["child1"],cx);
    press("left",&["A"],cx);
    press("left",&["A"],cx);
    assert!(cx.update(|_,cx| st.read(cx).collapsed.contains("A")));
    press("down",&["B"],cx);
    press("up",&["A"],cx);
    press("right",&["A"],cx);
    assert!(!cx.update(|_,cx| st.read(cx).collapsed.contains("A")));
    press("right",&["child1"],cx);
    let search=cx.update(|_,cx| outliner.read(cx).search_for_test());
    cx.update(|_,cx| search.update(cx,|s,cx| s.set_text("child2",cx)));cx.run_until_parked();
    press("down",&["child2"],cx);
    press("shift-up",&["child2","A"],cx);
    press("left",&["child2","A"],cx);
    assert!(!cx.update(|_,cx| st.read(cx).collapsed.contains("A")),"search does not change the saved folding state");
    cx.update(|_,cx| search.update(cx,|s,cx| s.set_text("",cx)));cx.run_until_parked();
    ui(&f,cx,"ui.studio",json!({"select":["tail-78"]})).unwrap();
    press("down",&["tail-79"],cx);
    let (position,bounds)=cx.update(|_,cx| {
        let o=outliner.read(cx);(o.row_position_for_test("tail-79",cx),o.rows_bounds_for_test())
    });
    assert!(bounds.contains(&position),"keyboard selection scrolls into view: {position:?}, {bounds:?}");
    // Outside the Outliner, ordinary frame navigation still reaches the editor's action.
    ui(&f,cx,"ui.studio",json!({"selectedKeys":[]})).unwrap();
    ui(&f,cx,"timeline.seek",json!({"time":2,"exact":true})).unwrap();
    press("left",&["tail-79"],cx);
    assert!((cx.update(|_,cx| st.read(cx).playhead(cx))-(2.-1./f.project().settings.fps)).abs()<1e-6);
    assert_eq!(scene_of(&f.project(),clip),before);
    assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn cancelling_an_animated_transform_restores_base_values_and_original_curves(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let value = f.call("motion.add", json!({"scene":{"type":"3d","objects":[
        {"id":"box","type":"box","position":[7,2,0],"keyframes":{
            "position":[[0,[0,0,0]],[1,[2,0,0],"easeOutBack"]],
            "position.x":[[0,1],[1,3,"easeInOut"]], "rotation.z":[[0,0],[1,90,"easeOut"]]
        }}
    ]},"duration":4}));
    let clip: Id = value["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    ui(&f, cx, "ui.studio", json!({"clipId":clip,"select":["box"]})).unwrap();
    ui(&f, cx, "timeline.seek", json!({"time":0.5})).unwrap();
    let before = scene_of(&f.project(), clip);
    let st = studio(&view, cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("g x 2");
    let keyed = |p: &Project| model::keyframes(&scene_of(p, clip), "box").unwrap()["position"].len() == 3;
    assert!(keyed(&f.settle(cx, keyed)), "the move first adds a key between existing keys");
    cx.simulate_keystrokes("escape");
    wait(cx, |cx| cx.update(|_, cx| !st.read(cx).sender.busy));
    assert_eq!(scene_of(&f.settle(cx, |p| scene_of(p, clip) == before), clip), before, "Escape restores the complete original animation");
}

#[gpui::test]
fn cancelling_an_animated_layer_rotation_restores_its_curve(cx: &mut TestAppContext) {
    use gpui::Modifiers;
    let (f, view, cx) = setup(cx);
    let value = f.call("motion.add", json!({"scene":{"layers":[
        {"id":"rect","type":"rect","width":100,"height":100,"rotation":15,"keyframes":{"rotation":[[0,0],[1,90,"easeInOut"]],"x":[[0,0],[1,20]]}}
    ]},"duration":4}));
    let clip: Id = value["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx, |s| s.clip(clip).is_some());
    ui(&f, cx, "ui.studio", json!({"clipId":clip,"select":["rect"]})).unwrap();
    ui(&f, cx, "timeline.seek", json!({"time":0.5})).unwrap();
    let before = scene_of(&f.project(), clip);
    let st = studio(&view, cx);
    let vp = cx.update(|_, cx| st.read(cx).viewport.clone());
    cx.run_until_parked();
    let centre = cx.update(|_, cx| vp.read(cx).bounds_for_test().center());
    cx.simulate_mouse_move(centre + point(px(80.),px(0.)), None, Modifiers::none());
    cx.simulate_keystrokes("r");
    cx.simulate_mouse_move(centre + point(px(0.),px(80.)), None, Modifiers::none());
    let keyed = |p: &Project| model::keyframes(&scene_of(p, clip), "rect").unwrap()["rotation"].len() == 3;
    assert!(keyed(&f.settle(cx, keyed)), "rotating an animated layer inserts a key at the playhead");
    cx.simulate_keystrokes("escape");
    wait(cx, |cx| cx.update(|_, cx| !st.read(cx).sender.busy));
    assert_eq!(scene_of(&f.settle(cx, |p| scene_of(p, clip) == before), clip), before);
}

#[gpui::test]
fn studio_timeline_navigation_and_edits_preserve_times_outside_the_clip(cx: &mut TestAppContext) {
    use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase};
    let (f, view, cx) = setup(cx);
    let clip = open_box(&f, cx);
    f.call("motion.setKeyframes", json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[1,0],[3,4]]}));
    let st = studio(&view, cx);
    ui(&f, cx, "ui.studio", json!({"timelineRange":[1,3]})).unwrap();
    cx.run_until_parked();
    let bounds = cx.update(|_, cx| st.read(cx).timeline.read(cx).bounds_for_test());
    let at = point(bounds.origin.x + px(8.) + (bounds.size.width - px(16.)) * 0.25, bounds.origin.y + px(12.));
    let before = f.call("history.list", json!({}));
    let wheel = |mods: Modifiers, dy: f32, cx: &mut VisualTestContext| {
        cx.simulate_event(ScrollWheelEvent { position: at, delta: ScrollDelta::Pixels(point(px(0.),px(dy))), modifiers: mods, touch_phase: TouchPhase::Moved });
        cx.run_until_parked();
    };
    wheel(Modifiers { control: true, ..Default::default() }, 40., cx);
    let zoomed = cx.update(|_, cx| st.read(cx).timeline_span(cx));
    assert!(zoomed.1 - zoomed.0 < 2.);
    assert!((zoomed.0 + (zoomed.1 - zoomed.0) * 0.25 - 1.5).abs() < 1e-5, "pointer time remains anchored: {zoomed:?}");
    wheel(Modifiers { shift: true, ..Default::default() }, -40., cx);
    let panned = cx.update(|_, cx| st.read(cx).timeline_span(cx));
    assert!(panned.0 > zoomed.0);
    assert!(((panned.1 - panned.0) - (zoomed.1 - zoomed.0)).abs() < 1e-8);
    cx.update(|_, cx| st.update(cx, |s, cx| {
        s.keys = [1.,3.].into_iter().map(|time| KeyRef { id:"box".into(), property:"position.x".into(), time }).collect();
        s.changed(cx);
    }));
    assert_eq!(ui(&f, cx, "ui.studio", json!({"fitTimeline":"selection"})).unwrap()["timelineRange"], json!([0.75,3.25]));
    assert!(ui(&f, cx, "ui.studio", json!({"timelineRange":[2,1]})).is_err());
    assert_eq!(ui(&f, cx, "ui.studio", json!({"fitTimeline":"clip"})).unwrap()["timelineRange"], json!([0.,4.]));
    assert_eq!(f.call("history.list", json!({})), before, "navigation adds no project edits");
    let timeline = cx.update(|_, cx| st.read(cx).timeline.clone());
    let key_times = |p: &Project| model::keyframes(&scene_of(p, clip), "box").unwrap()["position.x"].iter().map(|k| k.time).collect::<Vec<_>>();
    cx.update(|_, cx| timeline.update(cx, |t, cx| t.drag_keys_by(2., cx)));
    assert_eq!(key_times(&f.settle(cx, |p| key_times(p) == [3.,5.])), [3.,5.]);
    wait(cx, |cx| cx.update(|_, cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>() == [3.,5.]));
    cx.update(|_, cx| timeline.update(cx, |t, cx| t.drag_keys_by(1., cx)));
    assert_eq!(key_times(&f.settle(cx, |p| key_times(p) == [4.,6.])), [4.,6.], "the key beyond the clip's end still moves");
}

#[gpui::test]
fn graph_keys_select_and_drag_together_with_one_undo(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (f, view, cx) = setup(cx);
    let clip = open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position","keyframes":[[1,[1,2,3]],[2,[4,5,6],"easeOutBack"],[3,[7,8,9],"hold"]]}));
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"rotation.y","keyframes":[[0,0],[4,90]]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k| k.contains_key("rotation.y")));
    let before = scene_of(&f.project(),clip);
    ui(&f,cx,"ui.studio",json!({"select":["box"],"showGraph":true,"graphProperty":"position"})).unwrap();
    let st = studio(&view,cx);
    let timeline = cx.update(|_,cx| st.read(cx).timeline.clone());
    cx.run_until_parked();
    let at = |time,value,cx:&mut VisualTestContext| cx.update(|_,cx| timeline.read(cx).graph_point_for_test(time,value,cx));
    let click_key = |p,mods,cx:&mut VisualTestContext| {
        cx.simulate_mouse_down(p,MouseButton::Left,mods);
        cx.simulate_mouse_up(p,MouseButton::Left,mods);
        cx.run_until_parked();
    };
    click_key(at(1.,2.,cx),Modifiers::none(),cx);
    click_key(at(2.,5.,cx),Modifiers { shift:true,..Modifiers::none() },cx);
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>()),[1.,2.]);
    let a = at(1.,2.,cx);
    let b = at(1.5,3.,cx);
    drag(cx,a,b);
    let keys = |p:&Project| model::keyframes(&scene_of(p,clip),"box").unwrap()["position"].clone();
    let moved = f.settle(cx,|p| (keys(p)[0].time-1.5).abs()<1e-6);
    let list = keys(&moved);
    assert_eq!(list.iter().map(|k| k.time).collect::<Vec<_>>(),[1.5,2.5,3.]);
    for (key,expected) in list.iter().zip([[1.,3.,3.],[4.,6.,6.],[7.,8.,9.]]) {
        let kimchi_core::KeyValue::Vector(v) = &key.value else { panic!("vector key") };
        assert!(v.iter().zip(expected).all(|(v,e)| (v-e).abs()<1e-5),"{v:?} vs {expected:?}");
    }
    assert_eq!(list[1].easing,kimchi_core::Easing::parse("easeOutBack").unwrap());
    assert_eq!(scene_of(&moved,clip).item_json("box").unwrap()["position"],before.item_json("box").unwrap()["position"]);
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>()==[1.5,2.5]));
    assert_eq!(f.call("history.list",json!({}))["undo"][0]["label"],"motion.updateKeyframes");
    f.call("history.undo",json!({}));
    assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==before),clip),before);
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).keys.is_empty()));
    assert!(cx.update(|_,cx| st.read(cx).keys.is_empty()),"undo drops references to keys that no longer exist");
    cx.update(|_,cx| timeline.update(cx,|t,cx| t.select_all_keys(cx)));
    let selected = cx.update(|_,cx| st.read(cx).keys.clone());
    assert_eq!(selected.len(),3);
    assert!(selected.iter().all(|k| k.property=="position"),"select all stays on the visible curve");
    cx.update(|_,cx| timeline.update(cx,|t,cx| t.drag_keys_by(-10.,cx)));
    let shifted = f.settle(cx,|p| keys(p).iter().map(|k| k.time).collect::<Vec<_>>()==[0.,1.,2.]);
    assert_eq!(keys(&shifted).iter().map(|k| k.time).collect::<Vec<_>>(),[0.,1.,2.],"clamping at zero preserves the group's spacing");
}

#[gpui::test]
fn outliner_renames_the_selected_namespace_and_keeps_selection(cx: &mut TestAppContext) {
    let (f,view,cx) = setup(cx);
    let v = f.call("motion.add",json!({"scene":{"objects":[{"id":"gold","type":"box","material":"gold"}],"materials":[{"id":"gold","color":"#ffcc00"}]},"start":0,"duration":4}));
    let clip:Id = v["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":[format!("{}gold",model::MATERIAL)]})).unwrap();
    let st = studio(&view,cx);
    let outliner = cx.update(|_,cx| st.read(cx).outliner.clone());
    cx.run_until_parked();
    cx.update(|w,cx| outliner.update(cx,|o,cx| o.start_rename(&format!("{}gold",model::MATERIAL),w,cx)));
    cx.run_until_parked();
    cx.simulate_input("brass");
    cx.simulate_keystrokes("enter");
    let has_brass = |p:&Project| matches!(scene_of(p,clip),Scene::Space(s) if s.material("brass").is_some());
    let p = f.settle(cx,has_brass);
    assert!(has_brass(&p));
    let item = scene_of(&p,clip).item_json("gold").expect("the object's name stays gold");
    assert_eq!(item["material"],"brass");
    let expected = vec![format!("{}brass",model::MATERIAL)];
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).selection==expected));
    assert_eq!(cx.update(|_,cx| st.read(cx).selection.clone()),expected);
    ui(&f,cx,"ui.studio",json!({"select":["gold"]})).unwrap();
    cx.update(|w,cx| outliner.update(cx,|o,cx| o.start_rename("gold",w,cx)));
    cx.run_until_parked();
    cx.simulate_input("brass");
    cx.simulate_keystrokes("enter");
    let renamed = f.settle(cx,|p| scene_of(p,clip).item_json("brass").is_some());
    assert!(scene_of(&renamed,clip).item_json("brass").is_some(),"an object can share a material's name");
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).selection==["brass"]));
    assert_eq!(cx.update(|_,cx| st.read(cx).selection.clone()),["brass"]);
}

#[gpui::test]
fn graph_handles_follow_the_clicked_component_and_escape_cancels_drags(cx: &mut TestAppContext) {
    use gpui::{Modifiers,MouseButton};
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position","keyframes":[[0,[0,10,100]],[2,[20,30,140],"easeOutBack"]]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k| k.contains_key("position")));
    ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position"})).unwrap();
    let st=studio(&view,cx);
    let timeline=cx.update(|_,cx| st.read(cx).timeline.clone());
    cx.run_until_parked();
    let at=|time,value,cx:&mut VisualTestContext| cx.update(|_,cx| timeline.read(cx).graph_point_for_test(time,value,cx));
    let click=|p,cx:&mut VisualTestContext| {
        cx.simulate_mouse_down(p,MouseButton::Left,Modifiers::none());
        cx.simulate_mouse_up(p,MouseButton::Left,Modifiers::none());
        cx.run_until_parked();
    };
    let before=scene_of(&f.project(),clip);
    let history=f.call("history.list",json!({}));
    click(at(2.,140.,cx),cx);
    assert!(!cx.update(|_,cx| timeline.read(cx).dragging_for_test()),"a fast click releases before the next paint and must still end the gesture");
    assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["graphComponent"],2,"clicking Z moves handles onto Z");
    assert!(ui(&f,cx,"ui.studio",json!({"graphComponent":-1})).is_err());
    click(at(0.666,113.32,cx),cx);
    assert!(!cx.update(|_,cx| timeline.read(cx).dragging_for_test()));
    assert_eq!(scene_of(&f.project(),clip),before,"clicking a handle does not replace the named easing");
    assert_eq!(f.call("history.list",json!({})),history);
    let from=at(2.,140.,cx);
    let to=at(2.5,120.,cx);
    cx.simulate_mouse_down(from,MouseButton::Left,Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(to,Some(MouseButton::Left),Modifiers::none());
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.simulate_mouse_up(to,MouseButton::Left,Modifiers::none());
    cx.run_until_parked();
    assert!(cx.update(|_,cx| st.read(cx).is_open()),"Escape cancels the drag before leaving Studio");
    assert_eq!(scene_of(&f.project(),clip),before);
    assert_eq!(f.call("history.list",json!({})),history,"cancelled previews add no history");
    let a=at(0.666,113.32,cx);
    let b=at(0.8,122.,cx);
    drag(cx,a,b);
    let easing=|p:&Project| model::keyframes(&scene_of(p,clip),"box").unwrap()["position"][1].easing;
    let expected=kimchi_core::Easing::Bezier(0.4,0.55,0.667,0.667);
    let after=f.settle(cx,|p| easing(p)==expected);
    assert_eq!(easing(&after),expected,"handle coordinates come from Z, not X");
    let mut restored=scene_of(&after,clip);
    restored.keyframes_mut("box").unwrap().get_mut("position").unwrap()[1].easing=kimchi_core::Easing::parse("easeOutBack").unwrap();
    assert_eq!(restored,before,"handle drags only change easing");
}

#[gpui::test]
fn key_selection_recovers_after_failed_edits_and_external_key_removal(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[1,2],[3,4]]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k| k.contains_key("position.x")));
    let st=studio(&view,cx);
    cx.update(|_,cx| st.update(cx,|s,cx| {
        s.selection=vec!["box".into()];
        s.area=super::Area::Timeline;
        s.keys=[1.,99.].into_iter().map(|time| KeyRef {id:"box".into(),property:"position.x".into(),time}).collect();
        s.run_then("motion.updateKeyframes",json!({"clipId":clip,"updates":[{"id":"box","property":"position.x","time":99,"newTime":100}]}),cx,|_,_,_| panic!("invalid key must fail"));
    }));
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).pending_edits==0));
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>()),[1.]);
    f.call("motion.removeKeyframe",json!({"clipId":clip,"id":"box","property":"position.x","time":1}));
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).keys.is_empty()));
    assert!(cx.update(|_,cx| st.read(cx).keys.is_empty()),"external removal clears the stale selection");
    let before=scene_of(&f.project(),clip);
    let history=f.call("history.list",json!({}));
    cx.simulate_keystrokes("x");
    cx.run_until_parked();
    assert_eq!(scene_of(&f.project(),clip),before,"Delete in an empty key selection must not delete the selected object");
    assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn right_click_cancels_timeline_drags_without_edits_or_context_menus(cx:&mut TestAppContext) {
    use gpui::{Modifiers,MouseButton};
    let (f,view,cx)=setup(cx);let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[0,0],[1,1],[2,2]]}));
    store_settles(cx,|s|model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k|k.contains_key("position.x")));
    let st=studio(&view,cx);let timeline=cx.update(|_,cx|st.read(cx).timeline.clone());
    let before=f.project();let history=f.call("history.list",json!({}));
    let keys=json!([{"id":"box","property":"position.x","time":1.0}]);
    for graph in [false,true] {
        for boxing in [false,true] {
            ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","showGraph":graph,"selectedKeys":keys})).unwrap();
            cx.run_until_parked();
            if boxing {cx.simulate_keystrokes("b");cx.run_until_parked();}
            let from=cx.update(|_,cx| {
                let t=timeline.read(cx);let mut p=t.graph_point_for_test(1.,1.,cx);
                if !graph {p.y=t.bounds_for_test().origin.y+px(33.);}p
            });
            let to=from+point(px(30.),px(15.));
            cx.simulate_mouse_down(from,MouseButton::Left,Modifiers::none());cx.run_until_parked();
            cx.simulate_mouse_move(to,Some(MouseButton::Left),Modifiers::none());cx.run_until_parked();
            assert!(cx.update(|_,cx|timeline.read(cx).dragging_for_test()));
            cx.simulate_mouse_down(to,MouseButton::Right,Modifiers::none());cx.run_until_parked();
            cx.simulate_mouse_up(to,MouseButton::Right,Modifiers::none());cx.run_until_parked();
            assert!(!cx.update(|_,cx|timeline.read(cx).dragging_for_test()),"graph={graph}, boxing={boxing}");
            cx.simulate_mouse_up(to,MouseButton::Left,Modifiers::none());cx.run_until_parked();
            assert!(cx.update(|_,cx|st.read(cx).store.read(cx).menu.is_none()),"cancelling must not open a context menu");
            assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["selectedKeys"],keys);
            assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
            // An ordinary right click still opens the keyframe menu.
            cx.simulate_mouse_down(from,MouseButton::Right,Modifiers::none());cx.simulate_mouse_up(from,MouseButton::Right,Modifiers::none());cx.run_until_parked();
            assert!(cx.update(|_,cx|st.read(cx).store.read(cx).menu.is_some()));
            cx.update(|_,cx|st.read(cx).store.clone()).update(cx,|s,cx|s.close_menu(cx));
        }
        cx.simulate_keystrokes("b");cx.run_until_parked();
        let at=cx.update(|_,cx|timeline.read(cx).bounds_for_test().center());
        cx.simulate_mouse_down(at,MouseButton::Right,Modifiers::none());cx.simulate_mouse_up(at,MouseButton::Right,Modifiers::none());cx.run_until_parked();
        assert!(!cx.update(|_,cx|timeline.read(cx).box_armed_for_test()),"right click also disarms an unused box tool");
    }
}

#[gpui::test]
fn changing_studio_areas_disarms_tools_waiting_in_the_previous_area(cx:&mut TestAppContext) {
    use super::Area;
    let (f,view,cx)=setup(cx);open_box(&f,cx);
    ui(&f,cx,"ui.studio",json!({"tool":"select","selectedKeys":[],"panel":"objects"})).unwrap();
    let st=studio(&view,cx);
    let (timeline,viewport,outliner)=cx.update(|_,cx| {let s=st.read(cx);(s.timeline.clone(),s.viewport.clone(),s.outliner.clone())});
    let before=f.project();let history=f.call("history.list",json!({}));
    cx.simulate_keystrokes("b");cx.run_until_parked();
    assert!(cx.update(|_,cx|timeline.read(cx).box_armed_for_test()));
    let centre=cx.update(|_,cx|viewport.read(cx).bounds_for_test().center());
    cx.simulate_click(centre,gpui::Modifiers::none());cx.run_until_parked();
    assert_eq!(cx.update(|_,cx|st.read(cx).area),Area::Viewport);
    assert!(!cx.update(|_,cx|timeline.read(cx).box_armed_for_test()),"clicking the viewport disarms the timeline box");
    cx.simulate_keystrokes("b");cx.run_until_parked();
    assert!(cx.update(|_,cx|viewport.read(cx).escapable()));
    let ruler=cx.update(|_,cx| {let b=timeline.read(cx).bounds_for_test();point(b.origin.x+px(50.),b.origin.y+px(10.))});
    cx.simulate_click(ruler,gpui::Modifiers::none());cx.run_until_parked();
    assert_eq!(cx.update(|_,cx|st.read(cx).area),Area::Timeline);
    assert!(!cx.update(|_,cx|viewport.read(cx).escapable()),"timeline interaction disarms the viewport box");
    for was_timeline in [true,false] {
        if !was_timeline {cx.simulate_click(centre,gpui::Modifiers::none());cx.run_until_parked();}
        cx.simulate_keystrokes("b");cx.run_until_parked();
        cx.update(|w,cx|outliner.update(cx,|o,cx|o.click("box",false,w,cx)));cx.run_until_parked();
        assert_eq!(cx.update(|_,cx|st.read(cx).area),Area::Outliner);
        assert!(!cx.update(|_,cx|timeline.read(cx).box_armed_for_test()));
        assert!(!cx.update(|_,cx|viewport.read(cx).escapable()));
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
    let added=f.call("motion.add",json!({"start":10,"duration":4,"scene":{"layers":[{"id":"card","type":"rect"}]}}));
    let next:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(next).is_some());
    ui(&f,cx,"ui.studio",json!({"selectedKeys":[]})).unwrap();cx.simulate_keystrokes("b");
    ui(&f,cx,"ui.studio",json!({"clipId":next})).unwrap();cx.run_until_parked();
    assert_eq!(cx.update(|_,cx|st.read(cx).area),Area::Viewport,"new scenes start with viewport shortcuts");
    assert!(!cx.update(|_,cx|timeline.read(cx).box_armed_for_test()));
}

#[gpui::test]
fn keyframe_selection_operations_refine_channels_in_both_editors_without_edits(cx:&mut TestAppContext) {
    use gpui::{Modifiers,MouseButton};
    let (f,view,cx)=setup(cx);
    crate::tests::resize(cx,1400.,900.);
    let clip=open_box(&f,cx);
    for (property,keys) in [("position.x",json!([[0,0],[1,1],[2,2]])),("position.y",json!([[2,4]]))] {
        f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":property,"keyframes":keys}));
    }
    store_settles(cx,|s|model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k|k.contains_key("position.y")));
    let before=f.project();let history=f.call("history.list",json!({}));
    let key=|property:&str,time:f64|json!({"id":"box","property":property,"time":time});
    let x0=key("position.x",0.);let x1=key("position.x",1.);let x2=key("position.x",2.);let y2=key("position.y",2.);
    ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","timelineRange":[0,4],"selectedKeys":[x0,y2]})).unwrap();
    let state=ui(&f,cx,"ui.studio",json!({"selectionOp":"add","selectedKeys":[key("position.x",1.00000001),x1,x0]})).unwrap();
    assert_eq!(state["selectedKeys"],json!([x0,y2,x1]),"add canonicalizes times and keeps other channels");
    let state=ui(&f,cx,"ui.studio",json!({"selectionOp":"subtract","selectedKeys":[key("position.x",-0.0)]})).unwrap();
    assert_eq!(state["selectedKeys"],json!([y2,x1]),"signed zero refers to the same key");
    for operation in ["add","subtract"] {
        let state=ui(&f,cx,"ui.studio",json!({"selectionOp":operation,"selectedKeys":[]})).unwrap();
        assert_eq!(state["selectedKeys"],json!([y2,x1]));
        assert!(ui(&f,cx,"ui.studio",json!({"selectionOp":operation,"selectedKeys":[x1,key("position.x",9.)]})).is_err());
        assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["selectedKeys"],json!([y2,x1]),"invalid references never partially change selection");
    }
    let st=studio(&view,cx);let timeline=cx.update(|_,cx|st.read(cx).timeline.clone());
    for graph in [false,true] {
        ui(&f,cx,"ui.studio",json!({"showGraph":graph,"selectedKeys":[x0,y2,x2]})).unwrap();
        cx.run_until_parked();
        let region=|cx:&mut VisualTestContext|cx.update(|_,cx| {
            let t=timeline.read(cx);
            let mut a=t.graph_point_for_test(0.8,1.2,cx);let mut b=t.graph_point_for_test(1.2,0.8,cx);
            if !graph {a.y=t.bounds_for_test().origin.y+px(24.);b.y=t.bounds_for_test().origin.y+px(42.);}
            (a,b)
        });
        for (mods,expected) in [
            (Modifiers {shift:true,..Modifiers::none()},json!([x0,y2,x2,x1])),
            (Modifiers {shift:true,control:true,..Modifiers::none()},json!([x0,y2,x2])),
            (Modifiers {shift:true,platform:true,..Modifiers::none()},json!([x0,y2,x2])),
            (Modifiers::none(),json!([x1])),
        ] {
            let bounds=cx.update(|_,cx|timeline.read(cx).bounds_for_test());
            cx.simulate_keystrokes("b");cx.run_until_parked();
            assert_eq!(cx.update(|_,cx|timeline.read(cx).bounds_for_test()),bounds,"arming selection keeps key positions stable");
            let (a,b)=region(cx);
            cx.simulate_mouse_down(a,MouseButton::Left,mods);cx.run_until_parked();
            cx.simulate_mouse_move(b,Some(MouseButton::Left),mods);cx.run_until_parked();
            assert_eq!(cx.update(|_,cx|timeline.read(cx).bounds_for_test()),bounds,"the drag status keeps key positions stable");
            cx.simulate_mouse_move(b,None,Modifiers::none());cx.run_until_parked();
            assert!(cx.update(|_,cx|timeline.read(cx).dragging_for_test()),"pointer re-entry preserves the pending rectangle");
            cx.simulate_mouse_up(b,MouseButton::Left,mods);cx.run_until_parked();
            assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["selectedKeys"],expected,"graph={graph}, {mods:?}");
        }
        cx.simulate_keystrokes("b");cx.run_until_parked();
        let (a,b)=region(cx);let mods=Modifiers {shift:true,control:true,..Modifiers::none()};
        cx.simulate_mouse_down(a,MouseButton::Left,mods);cx.run_until_parked();
        cx.simulate_mouse_move(b,Some(MouseButton::Left),mods);cx.run_until_parked();
        cx.simulate_mouse_move(b,None,Modifiers::none());cx.run_until_parked();
        cx.simulate_keystrokes("escape");cx.run_until_parked();
        cx.simulate_mouse_up(b,MouseButton::Left,Modifiers::none());cx.run_until_parked();
        assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["selectedKeys"],json!([x1]),"Escape keeps the previous selection");
    }
    ui(&f,cx,"ui.studio",json!({"panel":"none"})).unwrap();
    for width in [720.,420.] {
        crate::tests::resize(cx,width,900.);cx.run_until_parked();
        let position=|cx:&mut VisualTestContext|cx.update(|_,cx|timeline.read(cx).graph_point_for_test(1.,1.,cx));
        let before=position(cx);
        for tool in ["b","g"] {
            cx.simulate_keystrokes(tool);cx.run_until_parked();
            assert_eq!(position(cx),before,"{tool} must not shift a key in a {width}px window");
            cx.simulate_keystrokes("escape");cx.run_until_parked();
            assert_eq!(position(cx),before,"cancellation preserves graph framing");
        }
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn graph_box_selection_and_explicit_key_selection_are_precise_view_changes(cx: &mut TestAppContext) {
    use gpui::{Modifiers,MouseButton};
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[1,1],[2,3],[3,2]]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k| k.contains_key("position.x")));
    let before=f.call("history.list",json!({}));
    let selection=json!([{"id":"box","property":"position.x","time":3.0}]);
    let state=ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","selectedKeys":[
        {"id":"box","property":"position.x","time":3.00000001},{"id":"box","property":"position.x","time":3}
    ]})).unwrap();
    assert_eq!(state["selectedKeys"],selection,"nearby times resolve to the exact key and duplicates collapse");
    for invalid in [json!([{"id":"box","property":"position.x","time":1},{"id":"box","property":"position.x","time":5}]),
        json!([{"id":"box","property":"position.x","time":3,"typo":true}]),json!([{"id":"box","time":3}]),json!(null)] {
        assert!(ui(&f,cx,"ui.studio",json!({"selectedKeys":invalid})).is_err());
        assert_eq!(ui(&f,cx,"ui.studio",json!({})).unwrap()["selectedKeys"],selection);
    }
    let st=studio(&view,cx);
    let timeline=cx.update(|_,cx| st.read(cx).timeline.clone());
    cx.run_until_parked();
    let at=|time,value,cx:&mut VisualTestContext| cx.update(|_,cx| timeline.read(cx).graph_point_for_test(time,value,cx));
    let box_drag=|a,b,mods,cx:&mut VisualTestContext| {
        cx.simulate_mouse_down(a,MouseButton::Left,mods);
        cx.run_until_parked();
        cx.simulate_mouse_move(b,Some(MouseButton::Left),mods);
        cx.run_until_parked();
        cx.simulate_mouse_up(b,MouseButton::Left,mods);
        cx.run_until_parked();
    };
    let a=at(0.5,0.8,cx);
    let b=at(2.5,3.2,cx);
    box_drag(a,b,Modifiers {shift:true,..Modifiers::none()},cx);
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>()),[1.,2.]);
    let a=at(2.8,1.8,cx);
    let b=at(3.2,2.2,cx);
    box_drag(a,b,Modifiers {shift:true,control:true,..Modifiers::none()},cx);
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>()),[1.,2.,3.]);
    ui(&f,cx,"ui.studio",json!({"view":"camera","lockCamera":true})).unwrap();
    let camera=cx.update(|_,cx| st.read(cx).view);
    cx.simulate_keystrokes(".");
    assert_eq!(cx.update(|_,cx| st.read(cx).timeline_span(cx)),(0.75,3.25),"frame targets selected keys in the timeline");
    cx.simulate_keystrokes("home");
    assert_eq!(cx.update(|_,cx| st.read(cx).timeline_span(cx)),(0.,4.));
    cx.simulate_keystrokes("=");
    let span=cx.update(|_,cx| st.read(cx).timeline_span(cx));
    assert!((span.1-span.0-3.2).abs()<1e-6,"keyboard zoom changes time");
    assert_eq!(cx.update(|_,cx| st.read(cx).view),camera,"timeline shortcuts leave the viewport alone");
    cx.simulate_keystrokes("home b escape");
    assert!(cx.update(|_,cx| st.read(cx).is_open()),"Escape disarms selection before leaving Studio");
    cx.simulate_keystrokes("b");
    let a=at(0.5,0.8,cx);
    let b=at(2.5,3.2,cx);
    box_drag(a,b,Modifiers::none(),cx);
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>()),[1.,2.],"B arms a rectangle without holding Shift");
    assert_eq!(f.call("history.list",json!({})),before,"selection is view state, never a project edit");
    assert!(ui(&f,cx,"ui.studio",json!({"selectedKeys":[]})).unwrap()["selectedKeys"].as_array().unwrap().is_empty());
}

#[gpui::test]
fn timeline_duplicate_copies_keys_into_free_space_and_selects_them(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    let frame=1./f.project().settings.fps;
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[1,10,"hold"],[2,20,"easeOut"],[2.+frame,30]]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k| k.contains_key("position.x")));
    let before=scene_of(&f.project(),clip);
    let history=f.call("history.list",json!({}));
    ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","selectedKeys":[
        {"id":"box","property":"position.x","time":1},{"id":"box","property":"position.x","time":2}
    ]})).unwrap();
    let st=studio(&view,cx);
    cx.simulate_keystrokes("shift-d");
    let after=scene_of(&f.settle(cx,|p| model::keyframes(&scene_of(p,clip),"box").is_some_and(|k| k["position.x"].len()==5)),clip);
    let keys=model::keyframes(&after,"box").unwrap()["position.x"].clone();
    let originals=model::keyframes(&before,"box").unwrap()["position.x"].clone();
    assert_eq!(&keys[..3],originals.as_slice(),"existing keys are never overwritten by the shortcut");
    for (copy,source) in keys[3..].iter().zip(&originals) {
        assert!((copy.time-source.time-(1.+2.*frame)).abs()<1e-6,"copies skip occupied destinations");
        assert_eq!(copy.value,source.value);
        assert_eq!(copy.easing,source.easing);
    }
    let mut restored=after.clone();
    restored.keyframes_mut("box").unwrap().insert("position.x".into(),originals);
    assert_eq!(restored,before,"duplication changes only the selected curve");
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).eq(keys[3..].iter().map(|k| k.time))));
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.iter().map(|k| k.time).collect::<Vec<_>>()),keys[3..].iter().map(|k| k.time).collect::<Vec<_>>());
    let span=cx.update(|_,cx| st.read(cx).timeline_span(cx));
    assert!(span.0<keys[3].time && span.1>keys[4].time,"new keys are framed");
    assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    f.call("history.undo",json!({}));
    store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    assert_eq!(scene_of(&f.project(),clip),before,"one undo restores the entire scene");
    ui(&f,cx,"ui.studio",json!({"selectedKeys":[]})).unwrap();
    let history=f.call("history.list",json!({}));
    cx.simulate_keystrokes("shift-d");
    cx.run_until_parked();
    assert_eq!(scene_of(&f.project(),clip),before,"an empty key selection must not duplicate the selected object");
    assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn timeline_drags_snap_to_project_frames_and_alt_allows_subframes(cx: &mut TestAppContext) {
    use gpui::{Modifiers,MouseButton};
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[1,1],[2,3]]}));
    f.call("clip.update",json!({"clipId":clip,"speed":2}));
    f.call("clip.trim",json!({"clipId":clip,"edge":"start","time":0.05}));
    store_settles(cx,|s| s.clip(clip).is_some_and(|c| c.speed==2. && c.start==0.05));
    let before=scene_of(&f.project(),clip);
    let st=studio(&view,cx);
    let timeline=cx.update(|_,cx| st.read(cx).timeline.clone());
    for graph in [true,false] {
        for free in [false,true] {
            ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","showGraph":graph,"timelineRange":[0,4],
                "selectedKeys":[{"id":"box","property":"position.x","time":1},{"id":"box","property":"position.x","time":2}]})).unwrap();
            cx.run_until_parked();
            let (a,b)=cx.update(|_,cx| {
                let tl=timeline.read(cx);
                let mut a=tl.graph_point_for_test(1.,1.,cx);
                let mut b=tl.graph_point_for_test(1.09,1.,cx);
                if !graph { a.y=tl.bounds_for_test().origin.y+px(33.); b.y=a.y; }
                (a,b)
            });
            let mods=Modifiers {alt:free,..Modifiers::none()};
            cx.simulate_mouse_down(a,MouseButton::Left,mods);
            cx.run_until_parked();
            cx.simulate_mouse_move(b,Some(MouseButton::Left),mods);
            cx.run_until_parked();
            cx.simulate_mouse_up(b,MouseButton::Left,mods);
            let expected=if free {1.09} else {16./15.};
            let keys=|p:&Project| model::keyframes(&scene_of(p,clip),"box").unwrap()["position.x"].clone();
            let moved=|p:&Project| (keys(p)[0].time-expected).abs()<1e-5;
            // Wait for both keys: the drag's last preview can land before its commit.
            let spaced=|p:&Project| (keys(p)[1].time-keys(p)[0].time-1.).abs()<1e-6;
            let after=f.settle(cx,|p| moved(p) && spaced(p));
            assert!(moved(&after),"graph={graph}, free={free}");
            assert!(spaced(&after),"selection spacing stays exact");
            f.call("history.undo",json!({}));
            store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
        }
    }
}

#[gpui::test]
fn keyboard_retiming_previews_cancels_and_commits_atomically(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[1.013,1,"hold"],[2.013,3,"easeOut"]]}));
    f.call("clip.update",json!({"clipId":clip,"speed":2}));
    store_settles(cx,|s| s.clip(clip).is_some_and(|c| c.speed==2.));
    let before=scene_of(&f.project(),clip);
    let st=studio(&view,cx);
    let timeline=cx.update(|_,cx| st.read(cx).timeline.clone());
    let select=|cx:&mut VisualTestContext| {
        ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","selectedKeys":[
            {"id":"box","property":"position.x","time":1.013},{"id":"box","property":"position.x","time":2.013}]})).unwrap();
        cx.run_until_parked();
    };
    let start=|key:&str,cx:&mut VisualTestContext| {
        cx.simulate_keystrokes(key);
        wait(cx,|cx| cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
        assert!(cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
        cx.run_until_parked();
    };
    select(cx);
    let history=f.call("history.list",json!({}));
    start("g",cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(f.call("history.list",json!({})),history,"confirming without a move preserves off-frame keys");
    start("g",cx);
    cx.simulate_keystrokes("1 5");
    assert_eq!(scene_of(&f.project(),clip),before,"typing only previews the edit");
    assert_eq!(f.call("history.list",json!({})),history);
    cx.simulate_keystrokes("escape");
    assert!(!cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
    assert!(cx.update(|_,cx| st.read(cx).is_open()));
    for (action,typed,expected) in [("g","1 5",[2.013,3.013]),("s","2",[1.013,3.013]),("g","- 1 0 0",[0.,1.])] {
        select(cx);
        start(action,cx);
        cx.simulate_keystrokes(typed);
        cx.simulate_keystrokes("enter");
        let matches=|p:&Project| model::keyframes(&scene_of(p,clip),"box").unwrap()["position.x"].iter().zip(expected).all(|(k,t)| (k.time-t).abs()<1e-6);
        let mut after=scene_of(&f.settle(cx,matches),clip);
        assert!(matches(&f.project()),"{action} {typed}");
        let keys=after.keyframes_mut("box").unwrap().get_mut("position.x").unwrap();
        assert_eq!(keys.len(),2);
        keys[0].time=1.013; keys[1].time=2.013;
        assert_eq!(after,before,"retiming preserves values, easing and base properties");
        f.call("history.undo",json!({}));
        store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    }
    select(cx);
    start("s",cx);
    cx.simulate_keystrokes("0 enter");
    assert!(cx.update(|_,cx| timeline.read(cx).retiming_for_test()),"zero scale must not collapse keys");
    assert_eq!(scene_of(&f.project(),clip),before);
    cx.simulate_keystrokes("escape");
    start("g",cx);
    cx.simulate_keystrokes("1 5");
    f.call("motion.updateKeyframes",json!({"clipId":clip,"updates":[{"id":"box","property":"position.x","time":1.013/2.,"value":99}]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").unwrap()["position.x"][0].value==kimchi_core::KeyValue::Number(99.));
    let external=scene_of(&f.project(),clip);
    let history=f.call("history.list",json!({}));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(scene_of(&f.project(),clip),external,"another client's edit invalidates the preview");
    assert_eq!(f.call("history.list",json!({})),history);
    start("g",cx);
    cx.simulate_keystrokes("1 5");
    f.call("motion.addKeyframe",json!({"clipId":clip,"id":"box","property":"position.x","time":3.013/2.,"value":7}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").unwrap()["position.x"].len()==3);
    let external=scene_of(&f.project(),clip);
    let history=f.call("history.list",json!({}));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(scene_of(&f.project(),clip),external,"new destination keys also invalidate the preview");
    assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn studio_panel_commands_control_the_shared_sidebar_without_reopening_it(cx: &mut TestAppContext) {
    let (f,_,cx)=setup(cx);
    let clip=open_box(&f,cx);
    let history=f.call("history.list",json!({}));
    for (w,h) in [(640.,480.),(1600.,1000.)] {
        crate::tests::resize(cx,w,h);
        for panel in ["objects","properties"] {
            ui(&f,cx,"ui.studio",json!({"panel":panel})).unwrap();
            cx.run_until_parked();
            let state=f.call("ui.state",json!({}));
            assert_eq!(state["layout"]["leftOpen"],true,"{panel} at {w}×{h}");
            assert_eq!(state["leftTab"],if panel=="objects" {"studio"} else {"inspector"});
            ui(&f,cx,"ui.studio",json!({"panel":"none"})).unwrap();
            cx.run_until_parked();
            assert_eq!(f.call("ui.state",json!({}))["layout"]["leftOpen"],false,"none closes the actual sidebar");
        }
        // Opening a clip requests the Studio tab; the explicit close in the same call wins.
        ui(&f,cx,"ui.studio",json!({"clipId":clip,"panel":"none"})).unwrap();
        cx.run_until_parked();
        let state=f.call("ui.state",json!({}));
        assert_eq!(state["layout"]["leftOpen"],false,"a deferred reveal must not reopen it");
        assert_eq!(state["screen"],"studio");
        assert_eq!(state["studio"]["panel"],Value::Null);
    }
    assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn keyframe_navigation_respects_the_visible_channel_selection_and_clip_trim(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[0,0],[1,1],[2,2],[4,4],[6,6]]}));
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"rotation.y","keyframes":[[1.5,45]]}));
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"scene","property":"ambient","keyframes":[[0.8,0.5]]}));
    f.call("clip.update",json!({"clipId":clip,"speed":2}));
    f.call("clip.trim",json!({"clipId":clip,"edge":"start","time":0.3}));
    store_settles(cx,|s| s.clip(clip).is_some_and(|c| c.start==0.3));
    let history=f.call("history.list",json!({}));
    let before=scene_of(&f.project(),clip);
    ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","view":"camera","lockCamera":true})).unwrap();
    ui(&f,cx,"timeline.seek",json!({"time":0.3})).unwrap();
    let st=studio(&view,cx);
    let step=|key:&str,expected:f64,cx:&mut VisualTestContext| {
        cx.simulate_keystrokes(key);
        wait(cx,|cx| cx.update(|_,cx| (st.read(cx).scene_time(cx)-expected).abs()<1e-6));
        let state=cx.update(|_,cx| st.read(cx).state_json(cx));
        assert!(cx.update(|_,cx| (st.read(cx).scene_time(cx)-expected).abs()<1e-6),"{key}: expected scene time {expected}, {state}");
    };
    for expected in [1.,2.,4.,4.] {step("alt-right",expected,cx);}
    step("alt-left",2.,cx);
    step("alt-left",1.,cx);
    step("alt-left",1.,cx);
    ui(&f,cx,"ui.studio",json!({"graphProperty":"rotation.y"})).unwrap();
    step("alt-right",1.5,cx);
    ui(&f,cx,"ui.studio",json!({"showGraph":false})).unwrap();
    ui(&f,cx,"timeline.seek",json!({"time":0.3})).unwrap();
    for expected in [1.,1.5,2.] {step("alt-right",expected,cx);}
    ui(&f,cx,"ui.studio",json!({"select":[]})).unwrap();
    ui(&f,cx,"timeline.seek",json!({"time":0.3})).unwrap();
    ui(&f,cx,"ui.action",json!({"action":"StudioNextKey"})).unwrap();
    wait(cx,|cx| cx.update(|_,cx| (st.read(cx).scene_time(cx)-0.8).abs()<1e-6));
    assert!(cx.update(|_,cx| (st.read(cx).scene_time(cx)-0.8).abs()<1e-6));
    assert_eq!(f.call("ui.state",json!({}))["studio"]["sceneTime"],0.8);
    assert_eq!(scene_of(&f.project(),clip),before,"navigation never moves objects or a locked camera");
    assert_eq!(f.call("history.list",json!({})),history);
    // A very short source range gets extra space in the timeline view, but not in playback.
    f.call("clip.update",json!({"clipId":clip,"speed":0.1}));
    f.call("clip.trim",json!({"clipId":clip,"edge":"end","time":0.32}));
    store_settles(cx,|s| s.clip(clip).is_some_and(|c| c.speed==0.1 && c.end()<0.33));
    ui(&f,cx,"timeline.seek",json!({"time":0.3})).unwrap();
    let now=cx.update(|_,cx| st.read(cx).scene_time(cx));
    step("alt-right",now,cx);
    cx.run_until_parked();
    assert!((cx.update(|_,cx| st.read(cx).scene_time(cx))-now).abs()<1e-6,"view padding must not expose unreachable keys to navigation");
    assert_eq!(ui(&f,cx,"timeline.seek",json!({"time":0.311,"exact":true})).unwrap()["playhead"],0.311);
    assert_eq!(ui(&f,cx,"timeline.seek",json!({"time":0.311})).unwrap()["playhead"],0.3,"ordinary seeking keeps frame snapping");
}

#[gpui::test]
fn timeline_shortcuts_keep_modelling_separate_and_insert_only_the_visible_curve(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let added=f.call("motion.add",json!({"start":5,"duration":4,"scene":{"type":"3d","objects":[
        {"id":"panel","type":"mesh","vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[0,1,2,3]],
            "position":[7,0,0],"keyframes":{"position.x":[[0,0],[2,2]],"rotation":[[0,[0,0,0]],[2,[0,90,0]]]}},
        {"id":"hidden","type":"box","hidden":true}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    f.call("clip.update",json!({"clipId":clip,"speed":2}));
    store_settles(cx,|s| s.clip(clip).is_some_and(|c| c.speed==2.));
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["panel"],"mode":"edit","selectMode":"face","editSelection":{"faces":[0]},
        "graphProperty":"rotation","selectedKeys":[{"id":"panel","property":"rotation","time":0}],"view":"camera","lockCamera":true})).unwrap();
    let st=studio(&view,cx);
    let viewport=cx.update(|_,cx| st.read(cx).viewport.clone());
    let before=scene_of(&f.project(),clip);let history=f.call("history.list",json!({}));
    for key in ["r","h","alt-h","e","ctrl-b","ctrl-r","m","alt-n","shift-n","1","2","3","7","0","5","tab","ctrl-alt-0"] {
        cx.simulate_keystrokes(key);cx.run_until_parked();
        assert!(!cx.update(|_,cx| viewport.read(cx).escapable()),"{key} must not start a viewport tool from the timeline");
        assert_eq!(cx.update(|_,cx| st.read(cx).area),super::Area::Timeline);
        assert_eq!(cx.update(|_,cx| st.read(cx).select_mode),super::SelectMode::Face);
        assert_eq!(scene_of(&f.project(),clip),before,"{key} must not edit geometry, visibility or the locked camera");
    }
    assert_eq!(cx.update(|_,cx| st.read(cx).graph_component),2);
    assert_eq!(f.call("history.list",json!({})),history);
    ui(&f,cx,"ui.studio",json!({"graphProperty":"position.x"})).unwrap();
    ui(&f,cx,"timeline.seek",json!({"time":5.5,"exact":true})).unwrap();
    cx.simulate_keystrokes("i");
    let matches=|p:&Project| model::keyframes(&scene_of(p,clip),"panel").unwrap()["position.x"].len()==3;
    let mut after=scene_of(&f.settle(cx,matches),clip);
    assert!(matches(&f.project()));
    let keys=after.keyframes_mut("panel").unwrap().get_mut("position.x").unwrap();
    assert_eq!(keys[1].time,1.);assert_eq!(keys[1].value,kimchi_core::KeyValue::Number(1.));
    keys.remove(1);
    assert_eq!(after,before,"insertion affects only the displayed curve, preserving geometry and base values");
    assert!(!cx.update(|_,cx| viewport.read(cx).busy()));
    assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).pending_edits==0));
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.clone()),vec![KeyRef {id:"panel".into(),property:"position.x".into(),time:1.}]);
    f.call("history.undo",json!({}));
    assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==before),clip),before);
}

#[gpui::test]
fn inserted_transform_keys_follow_the_displayed_frame_and_become_the_selection(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let added=f.call("motion.add",json!({"start":5,"duration":4,"scene":{"layers":[
        {"id":"a","type":"rect","width":10,"height":10,"x":9,"keyframes":{"x":[[0,0],[10,100,"easeOutBack"]]}},
        {"id":"b","type":"rect","width":10,"height":10,"x":20}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    f.call("clip.trim",json!({"clipId":clip,"edge":"start","time":6}));
    f.call("clip.update",json!({"clipId":clip,"speed":2}));
    store_settles(cx,|s| s.clip(clip).is_some_and(|c| c.in_point==1. && c.speed==2.));
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["a","b"],"showGraph":false,"selectedKeys":[]})).unwrap();
    let st=studio(&view,cx);
    let before=scene_of(&f.project(),clip);
    for (playhead,expected_time) in [(0.,1.),(20.,4.)] {
        ui(&f,cx,"timeline.seek",json!({"time":playhead,"exact":true})).unwrap();
        let history=f.call("history.list",json!({}));
        cx.simulate_keystrokes("i");
        wait(cx,|cx| cx.update(|_,cx| st.read(cx).pending_edits==0 && st.read(cx).keys.len()==10));
        let selected=cx.update(|_,cx| st.read(cx).keys.clone());
        assert_eq!(selected.len(),10,"all inserted transform keys are selected");
        assert!(selected.iter().all(|k| k.time==expected_time));
        let after=scene_of(&f.project(),clip);
        for key in selected {
            let keys=model::keyframes(&after,&key.id).unwrap();
            let inserted=keys[&key.property].iter().find(|k| k.time==expected_time).unwrap();
            assert_eq!(Some(inserted.value.clone()),model::value_at(&before,&key.id,&key.property,expected_time));
        }
        assert_eq!(after.to_json()["layers"][0]["x"],9.,"the unanimated base stays intact");
        assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
        f.call("history.undo",json!({}));
        store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
        wait(cx,|cx| cx.update(|_,cx| st.read(cx).keys.is_empty()));
        assert_eq!(scene_of(&f.project(),clip),before);
        assert!(cx.update(|_,cx| st.read(cx).keys.is_empty()));
    }
}

#[gpui::test]
fn shared_scene_sidebar_keeps_studio_shortcuts_and_search_keeps_typing(cx: &mut TestAppContext) {
    use gpui::Modifiers;
    let (f,view,cx)=setup(cx);
    crate::tests::resize(cx,1400.,900.);
    let clip=open_box(&f,cx);
    ui(&f,cx,"ui.studio",json!({"panel":"objects"})).unwrap();
    let st=studio(&view,cx);
    let outliner=cx.update(|_,cx| st.read(cx).outliner.clone());
    cx.run_until_parked();
    let row=cx.update(|_,cx| outliner.read(cx).row_position_for_test("box",cx));
    cx.simulate_click(row,Modifiers::none());cx.run_until_parked();
    assert!(cx.update(|w,cx| st.read(cx).focus.is_focused(w)),"a sidebar row keeps the Studio keyboard context");
    assert_eq!(cx.update(|_,cx| st.read(cx).area),super::Area::Outliner);
    let before=scene_of(&f.project(),clip);
    cx.simulate_keystrokes("g");
    let viewport=cx.update(|_,cx| st.read(cx).viewport.clone());
    wait(cx,|cx| cx.update(|_,cx| viewport.read(cx).busy()));
    assert!(cx.update(|_,cx| viewport.read(cx).busy()));
    cx.simulate_keystrokes("x 2 enter");
    let moved=|p:&Project| model::value_at(&scene_of(p,clip),"box","position.x",0.)==Some(kimchi_core::KeyValue::Number(2.));
    assert!(moved(&f.settle(cx,moved)));
    f.call("history.undo",json!({}));
    store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    // Search is immediately above the first scene row. Its input consumes the click.
    let search_at=cx.update(|_,cx| outliner.read(cx).row_position_for_test("scene",cx))-point(px(0.),px(36.));
    cx.simulate_click(search_at,Modifiers::none());
    cx.simulate_input("box");cx.run_until_parked();
    let search=cx.update(|_,cx| outliner.read(cx).search_for_test());
    assert_eq!(cx.update(|_,cx| search.read(cx).text().to_string()),"box");
    assert_eq!(scene_of(&f.project(),clip),before,"typing X in search must not delete the object");
    assert!(!cx.update(|w,cx| st.read(cx).focus.is_focused(w)));
    let row=cx.update(|_,cx| outliner.read(cx).row_position_for_test("box",cx));
    cx.simulate_click(row,Modifiers::none());cx.run_until_parked();
    assert!(cx.update(|w,cx| st.read(cx).focus.is_focused(w)),"clicking a result restores Studio shortcuts");
}

#[gpui::test]
fn graph_numeric_value_transforms_preserve_other_components_and_key_times(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position","keyframes":[[1,[1,2,3],"easeOutBack"],[2,[4,5,6],"hold"],[3,[7,8,9]]]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k| k.contains_key("position")));
    let before=scene_of(&f.project(),clip);
    let st=studio(&view,cx);
    let timeline=cx.update(|_,cx| st.read(cx).timeline.clone());
    let select=|cx:&mut VisualTestContext| {
        ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position","graphComponent":1,
            "selectedKeys":[{"id":"box","property":"position","time":1},{"id":"box","property":"position","time":2}]})).unwrap();
        cx.run_until_parked();
    };
    let start=|key:&str,cx:&mut VisualTestContext| {
        cx.simulate_keystrokes(key);
        wait(cx,|cx| cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
        assert!(cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
        cx.run_until_parked();
    };
    for (action,typed,expected) in [("g","y 2 . 5",[4.5,7.5]),("s","y - 2",[-4.,-10.]),("s","y 0",[0.,0.]),
        ("g","y 4 e - 4",[2.0004,5.0004]),("s","y 1 e - 3",[0.002,0.005])] {
        select(cx);
        let history=f.call("history.list",json!({}));
        start(action,cx);
        cx.simulate_keystrokes(typed);
        assert_eq!(scene_of(&f.project(),clip),before,"values preview without writing");
        assert_eq!(f.call("history.list",json!({})),history);
        cx.simulate_keystrokes("enter");
        let matches=|p:&Project| model::keyframes(&scene_of(p,clip),"box").unwrap()["position"].iter().take(2).zip(expected)
            .all(|(key,y)| matches!(&key.value,kimchi_core::KeyValue::Vector(v) if v[1]==y));
        let mut after=scene_of(&f.settle(cx,matches),clip);
        assert!(matches(&f.project()),"{action} {typed}");
        let keys=after.keyframes_mut("box").unwrap().get_mut("position").unwrap();
        for (key,old) in keys.iter_mut().take(2).zip([2.,5.]) {
            let kimchi_core::KeyValue::Vector(v)=&mut key.value else {panic!("vector key")};
            v[1]=old;
        }
        assert_eq!(after,before,"other components, times, easing and base properties stay intact");
        assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
        f.call("history.undo",json!({}));
        store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    }
    select(cx);
    let history=f.call("history.list",json!({}));
    start("g",cx);
    cx.simulate_keystrokes("y 1 e 3 0 9 enter");
    assert!(cx.update(|_,cx| timeline.read(cx).retiming_for_test()),"overflow must not apply");
    assert_eq!(scene_of(&f.project(),clip),before);
    assert_eq!(f.call("history.list",json!({})),history);
    cx.simulate_keystrokes("escape");
    start("g",cx);
    cx.simulate_keystrokes("y 9 9 9 escape");
    assert_eq!(scene_of(&f.project(),clip),before);
    assert_eq!(f.call("history.list",json!({})),history);
    start("g",cx);
    cx.simulate_keystrokes("y 2 x enter");
    let matches=|p:&Project| (model::keyframes(&scene_of(p,clip),"box").unwrap()["position"][0].time-1.-2./p.settings.fps).abs()<1e-6;
    let after=scene_of(&f.settle(cx,matches),clip);
    assert!(matches(&f.project()));
    let keys=model::keyframes(&after,"box").unwrap()["position"].clone();
    assert_eq!(keys.iter().map(|k| &k.value).collect::<Vec<_>>(),model::keyframes(&before,"box").unwrap()["position"].iter().map(|k| &k.value).collect::<Vec<_>>(),"X switches back to time without carrying the value preview");
}

#[gpui::test]
fn world_animation_can_be_inserted_selected_and_edited_in_the_graph(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","ambient":0.25,
        "keyframes":{"ambient":[[0,0.2],[2,0.8]]},
        "objects":[{"id":"box","type":"box","keyframes":{"rotation.y":[[0,0],[2,90]]}}]
    }}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    ui(&f,cx,"ui.studio",json!({"clipId":clip,"select":["scene"],"graphProperty":"ambient","selectedKeys":[]})).unwrap();
    let st=studio(&view,cx);
    let timeline=cx.update(|_,cx| st.read(cx).timeline.clone());
    let before=scene_of(&f.project(),clip);
    assert_eq!(cx.update(|_,cx| st.read(cx).graph_target(&before)),Some(("scene".into(),"ambient".into())));
    ui(&f,cx,"timeline.seek",json!({"time":1,"exact":true})).unwrap();
    cx.simulate_keystrokes("i");
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).pending_edits==0 && st.read(cx).keys.len()==1));
    assert_eq!(cx.update(|_,cx| st.read(cx).keys.clone()),vec![KeyRef {id:"scene".into(),property:"ambient".into(),time:1.}]);
    let inserted=scene_of(&f.project(),clip);
    let mut original=inserted.clone();original.keyframes_mut("scene").unwrap().get_mut("ambient").unwrap().remove(1);
    assert_eq!(original,before,"insertion preserves objects and the world's base ambient value");
    cx.simulate_keystrokes("g");
    wait(cx,|cx| cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
    assert!(cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
    cx.simulate_keystrokes("y . 1 enter");
    let moved=|p:&Project| model::keyframes(&scene_of(p,clip),"scene").unwrap()["ambient"][1].value.as_f64().is_some_and(|v| (v-0.6).abs()<1e-8);
    assert!(moved(&f.settle(cx,moved)));
    wait(cx,|cx| cx.update(|_,cx| st.read(cx).pending_edits==0));
    ui(&f,cx,"ui.studio",json!({"selectedKeys":[]})).unwrap();
    cx.simulate_keystrokes("a");cx.run_until_parked();
    let selected=cx.update(|_,cx| st.read(cx).keys.clone());
    assert_eq!(selected.len(),3);
    assert!(selected.iter().all(|k| k.id=="scene" && k.property=="ambient"));
    f.call("history.undo",json!({}));
    assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==inserted),clip),inserted);
    f.call("history.undo",json!({}));
    assert_eq!(scene_of(&f.settle(cx,|p| scene_of(p,clip)==before),clip),before);
}

#[gpui::test]
fn graph_numeric_scalar_edits_and_hidden_selection_guard(cx: &mut TestAppContext) {
    let (f,view,cx)=setup(cx);
    let clip=open_box(&f,cx);
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","keyframes":[[1,10,"easeIn"],[2,20,"hold"]]}));
    f.call("motion.setKeyframes",json!({"clipId":clip,"id":"box","property":"rotation","keyframes":[[1,[0,45,0]],[2,[0,90,0]]]}));
    store_settles(cx,|s| model::keyframes(&scene_of(s.project.as_ref().unwrap(),clip),"box").is_some_and(|k| k.contains_key("rotation")));
    let before=scene_of(&f.project(),clip);
    let st=studio(&view,cx);
    let timeline=cx.update(|_,cx| st.read(cx).timeline.clone());
    let start=|cx:&mut VisualTestContext| {
        cx.simulate_keystrokes("g");
        wait(cx,|cx| cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
        assert!(cx.update(|_,cx| timeline.read(cx).retiming_for_test()));
        cx.run_until_parked();
    };
    ui(&f,cx,"ui.studio",json!({"select":["box"],"graphProperty":"position.x","graphComponent":2,
        "selectedKeys":[{"id":"box","property":"position.x","time":1},{"id":"box","property":"position.x","time":2}]})).unwrap();
    cx.run_until_parked();
    start(cx);
    cx.simulate_keystrokes("y - 3 enter");
    let matches=|p:&Project| model::keyframes(&scene_of(p,clip),"box").unwrap()["position.x"][0].value==kimchi_core::KeyValue::Number(7.);
    let mut after=scene_of(&f.settle(cx,matches),clip);
    assert!(matches(&f.project()));
    let keys=after.keyframes_mut("box").unwrap().get_mut("position.x").unwrap();
    assert_eq!(keys[1].value,kimchi_core::KeyValue::Number(17.));
    keys[0].value=kimchi_core::KeyValue::Number(10.); keys[1].value=kimchi_core::KeyValue::Number(20.);
    assert_eq!(after,before,"scalar editing ignores the preferred vector component");
    f.call("history.undo",json!({}));
    store_settles(cx,|s| scene_of(s.project.as_ref().unwrap(),clip)==before);
    ui(&f,cx,"ui.studio",json!({"graphProperty":"position.x","selectedKeys":[
        {"id":"box","property":"position.x","time":1},{"id":"box","property":"rotation","time":1}
    ]})).unwrap();
    let history=f.call("history.list",json!({}));
    start(cx);
    cx.simulate_keystrokes("y 5 enter");
    assert!(cx.update(|_,cx| timeline.read(cx).retiming_for_test()),"value edits require keys on the visible numeric curve");
    assert_eq!(scene_of(&f.project(),clip),before,"a mixed selection must not modify a hidden curve");
    assert_eq!(f.call("history.list",json!({})),history);
    cx.simulate_keystrokes("escape");
}
