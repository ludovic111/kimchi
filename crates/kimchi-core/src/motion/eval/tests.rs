use serde_json::json;

use super::*;

fn flat(v: Value) -> Scene2d {
    match Scene::from_json(&v).unwrap_or_else(|e| panic!("{e}")) {
        Scene::Flat(s) => s,
        _ => panic!("2d"),
    }
}

fn space(v: Value) -> Scene3d {
    match Scene::from_json(&v).unwrap_or_else(|e| panic!("{e}")) {
        Scene::Space(s) => s,
        _ => panic!("3d"),
    }
}

fn layer<'a>(list: &'a [Layer], id: &str) -> &'a Layer {
    find_layer(list, id).unwrap_or_else(|| panic!("no layer {id}"))
}

/// An evaluated object's world matrix, through its parents (what the renderer draws).
fn world(list: &[Object3d], id: &str) -> M4 {
    fn look(list: &[Object3d], id: &str, parent: M4) -> Option<M4> {
        for o in list {
            let m = parent.mul(&M4::local(o));
            if o.id == id {
                return Some(m);
            }
            if let Some(f) = look(&o.children, id, m) {
                return Some(f);
            }
        }
        None
    }
    look(list, id, M4::I).unwrap_or_else(|| panic!("no object {id}"))
}

fn pos(list: &[Object3d], id: &str) -> V3 {
    world(list, id).pose().pos
}

/// Degrees between two directions.
fn angle(a: V3, b: V3) -> f64 {
    let (a, b) = (normalize(a), normalize(b));
    (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0).acos().to_degrees()
}

fn near(a: V3, b: V3, eps: f64) -> bool {
    dist(a, b) < eps
}

#[test]
fn layer_expressions_index_and_types() {
    let s = flat(json!({"layers": [
        {"id": "a", "type": "rect", "keyframes": {"x": [[0, 0], [2, 100]]}, "expressions": {"x": "value + 10", "rotation": "time * 90"}},
        {"id": "row", "type": "group", "layers": [
            {"id": "d1", "type": "ellipse", "expressions": {"y": "index * 100"}},
            {"id": "d2", "type": "ellipse", "expressions": {"y": "index * 100"}},
            {"id": "d3", "type": "ellipse", "fill": "#ffffff", "expressions": {"y": "index * 100", "fill": "[1, 0, 0]"}}
        ]},
        {"id": "label", "type": "text", "text": "0", "expressions": {"text": "round(time * 10)", "fill": "hsl(120, 1, 0.5)", "opacity": "[0.5]"}}
    ]}));
    let l = s.layers_at(1.0);
    assert_eq!(layer(&l, "a").x, 60.0, "keyframes then the formula");
    assert_eq!(layer(&l, "a").rotation, 90.0);
    assert!(layer(&l, "a").keyframes.is_empty());
    assert_eq!([layer(&l, "d1").y, layer(&l, "d2").y, layer(&l, "d3").y], [100.0, 200.0, 300.0], "index among siblings, from 1");
    assert_eq!(layer(&l, "d3").fill, Some(Fill::Color("#ff0000".into())), "[r, g, b] for a colour");
    assert_eq!(layer(&l, "label").fill, Some(Fill::Color("#00ff00".into())));
    assert_eq!(layer(&l, "label").opacity, 0.5);
    let later = s.layers_at(1.5);
    let LayerKind::Text(t) = &layer(&later, "label").kind else { panic!("text") };
    assert_eq!(t.text, "15", "a number for a text property becomes its digits");
    // Nothing changes in the scene itself.
    assert_eq!(s.layers[0].x, 0.0);
}

#[test]
fn compositions_have_their_own_time_and_length() {
    let s = flat(json!({
        "layers": [{"id": "shot", "type": "comp", "comp": "inner"}],
        "compositions": [{"id": "inner", "duration": 4, "layers": [
            {"id": "spin", "type": "rect", "expressions": {"rotation": "time * 10", "x": "duration"}}
        ]}]
    }));
    let l = s.comp_layers_at("inner", 2.0).unwrap();
    assert_eq!(l[0].rotation, 20.0);
    assert_eq!(l[0].x, 4.0, "duration is the composition's");
    assert!(s.comp_layers_at("nope", 1.0).is_none());
    let opts = EvalOptions { fps: 25.0, duration: Some(9.0) };
    let s = flat(json!({"layers": [{"id": "f", "type": "rect", "expressions": {"x": "frame", "y": "duration"}}]}));
    let l = s.layers_at_with(2.0, &opts);
    assert_eq!((l[0].x, l[0].y), (50.0, 9.0));
}

#[test]
fn prop_reads_other_layers() {
    let s = flat(json!({"layers": [
        {"id": "ball", "type": "ellipse", "keyframes": {"x": [[0, 0], [1, 100]]}, "expressions": {"y": "value + 7"}},
        {"id": "shadow", "type": "ellipse", "expressions": {
            "x": "prop(\"ball\", \"x\")",
            "y": "prop(\"ball\", \"y\") + 50",
            "rotation": "prop(\"ball\", \"x\", 0.25)",
            "opacity": "prop(\"comp1\", \"opacity\")"
        }},
        {"id": "trail", "type": "ellipse", "expressions": {"x": "valueAtTime(time) + prop(\"ball\", \"x\", time - 0.5)"}}
    ], "compositions": [{"id": "c", "layers": [{"id": "comp1", "type": "rect", "opacity": 0.25}]}]}));
    let l = s.layers_at(0.5);
    let sh = layer(&l, "shadow");
    assert_eq!(sh.x, 50.0);
    assert_eq!(sh.y, 57.0, "the other layer's formula applies too");
    assert_eq!(sh.rotation, 25.0, "at another time");
    assert_eq!(sh.opacity, 0.25, "layers in compositions too");
    assert_eq!(layer(&l, "trail").x, 0.0);
}

#[test]
fn circles_and_errors_keep_the_keyframed_value() {
    let s = flat(json!({"layers": [
        {"id": "a", "type": "rect", "x": 5, "expressions": {"x": "prop(\"b\", \"x\") + 1"}},
        {"id": "b", "type": "rect", "x": 9, "expressions": {"x": "prop(\"a\", \"x\") + 1"}},
        {"id": "c", "type": "rect", "x": 3, "keyframes": {"y": [[0, 1], [1, 2]]}, "expressions": {"x": "1 / 0", "y": "prop(\"nobody\", \"x\")", "rotation": "prop(\"c\", \"rotation\")"}}
    ]}));
    let l = s.layers_at(0.5);
    assert_eq!((layer(&l, "a").x, layer(&l, "b").x), (5.0, 9.0), "a circle is an error, not a hang");
    assert_eq!(layer(&l, "c").x, 3.0);
    assert_eq!(layer(&l, "c").y, 1.5);
    assert_eq!(layer(&l, "c").rotation, 0.0);
    // The messages.
    let world = Flat::new(&s, &EvalOptions::default());
    let root = Chain { id: "a", name: "x", up: None };
    let e = prop_value(&world, "b", "x", 0.0, &root).unwrap_err();
    assert!(e.contains("circle") && e.contains("a.x → b.x → a.x"), "{e}");
    let e = prop_value(&world, "bb", "x", 0.0, &root).unwrap_err();
    assert!(e.contains("Did you mean \"b\"?"), "{e}");
    let e = prop_value(&world, "b", "posiiton", 0.0, &root).unwrap_err();
    assert!(e.contains("Did you mean `position`?"), "{e}");
    // A long chain stops at the depth limit.
    let mut layers: Vec<Value> = (0..12)
        .map(|i| json!({"id": format!("l{i}"), "type": "rect", "x": i, "expressions": {"x": format!("prop(\"l{}\", \"x\") + 1", i + 1)}}))
        .collect();
    layers.push(json!({"id": "l12", "type": "rect", "x": 12}));
    let s = flat(json!({ "layers": layers }));
    let world = Flat::new(&s, &EvalOptions::default());
    let root = Chain { id: "l0", name: "x", up: None };
    let e = prop_value(&world, "l1", "x", 0.0, &root).unwrap_err();
    assert!(e.contains("more than 8"), "{e}");
    let l = s.layers_at(0.0);
    assert_eq!(layer(&l, "l10").x, 14.0, "l10 reads l11, which reads l12");
    assert_eq!(layer(&l, "l0").x, 0.0, "too deep: keeps its own value");
}

#[test]
fn staggered_by_index() {
    let s = flat(json!({"layers": [
        {"id": "a", "type": "rect", "keyframes": {"y": [[0, 0], [1, 100]]}, "expressions": {"y": "valueAtTime(time - (index - 1) * 0.25)"}},
        {"id": "b", "type": "rect", "keyframes": {"y": [[0, 0], [1, 100]]}, "expressions": {"y": "valueAtTime(time - (index - 1) * 0.25)"}},
        {"id": "c", "type": "rect", "keyframes": {"y": [[0, 0], [1, 100]]}, "expressions": {"y": "loopOut('pingpong')"}}
    ]}));
    let l = s.layers_at(0.5);
    assert_eq!((l[0].y, l[1].y), (50.0, 25.0));
    assert_eq!(s.layers_at(1.25)[2].y, 75.0);
}

fn scene_with(objects: Value) -> Scene3d {
    space(json!({"objects": objects, "lights": [{"id": "sun", "type": "spot", "position": [0, 5, 0], "direction": [0, -1, 0],
        "constraints": [{"type": "lookAt", "target": "hero"}]}],
        "camera": {"position": [0, 2, 10], "target": [0, 0, 0], "constraints": [{"type": "lookAt", "target": "hero"}]}}))
}

#[test]
fn look_at_aims_objects_cameras_and_lights() {
    let s = scene_with(json!([
        {"id": "hero", "type": "box", "position": [3, 1, -2]},
        {"id": "eye", "type": "cone", "position": [-2, 4, 5], "rotation": [10, 20, 30], "constraints": [{"type": "lookAt", "target": "hero"}]},
        {"id": "rig", "type": "group", "position": [1, 0, 1], "rotation": [0, 70, 15], "scale": [2, 2, 2], "children": [
            {"id": "turret", "type": "cylinder", "position": [0.5, 1, 0], "rotation": [0, -30, 0], "constraints": [{"type": "lookAt", "target": "hero", "offset": [0, 1, 0]}]}
        ]}
    ]));
    let objs = s.objects_at(0.0);
    for (id, aim) in [("eye", [3.0, 1.0, -2.0]), ("turret", [3.0, 2.0, -2.0])] {
        let w = world(&objs, id);
        let front = w.dir([0.0, 0.0, 1.0]);
        let a = angle(front, sub(aim, w.pose().pos));
        assert!(a < 0.5, "{id} aims within half a degree: {a}°");
        let up = w.dir([0.0, 1.0, 0.0]);
        assert!(up[1] > 0.0, "{id} stays upright");
    }
    assert_eq!(pos(&objs, "turret"), pos(&s.objects_at(0.0), "turret"), "same every time");
    let cam = s.camera_at(0.0);
    assert_eq!(cam.target.0, [3.0, 1.0, -2.0], "the camera's target is the object");
    assert_eq!(cam.position.0, [0.0, 2.0, 10.0]);
    let sun = &s.lights_at(0.0)[0];
    assert!(angle(sun.direction.0, sub([3.0, 1.0, -2.0], [0.0, 5.0, 0.0])) < 0.5, "the spot light aims");
    assert!((length(sun.direction.0) - 1.0).abs() < 1e-9, "keeps its length");
    let all = s.evaluate_at(0.0, &EvalOptions::default());
    assert_eq!(all.camera, cam);
    assert_eq!(all.objects, objs);
}

#[test]
fn follow_path_on_a_closed_curve() {
    let s = space(json!({"objects": [
        {"id": "loop", "type": "curve", "closed": true, "position": [0, 1, 0], "rotation": [0, 0, 90],
         "points": [[2, 0, 0], [0, 0, 2], [-2, 0, 0], [0, 0, -2]]},
        {"id": "car", "type": "box", "constraints": [{"type": "followPath", "path": "loop"}],
         "keyframes": {"constraints.followPath.progress": [[0, 0], [1, 1]]}}
    ]}));
    let line = Polyline::new(&[[2.0, 0.0, 0.0], [0.0, 0.0, 2.0], [-2.0, 0.0, 0.0], [0.0, 0.0, -2.0]], true, true);
    let curve = M4::trs([0.0, 1.0, 0.0], &rot_euler([0.0, 0.0, 90.0]), [1.0; 3]);
    for t in [0.0, 0.25, 0.6, 1.0] {
        let objs = s.objects_at(t);
        let (p, d) = line.at(t);
        let want = curve.point(p);
        assert!(near(pos(&objs, "car"), want, 1e-9), "on the curve at {t}");
        let front = world(&objs, "car").dir([0.0, 0.0, 1.0]);
        assert!(angle(front, curve.dir(d)) < 0.5, "faces along it at {t}");
    }
    // Wraps on a closed curve: 1.25 is 0.25.
    let mut more = s.clone();
    more.objects[1].keyframes.clear();
    more.objects[1].set("constraints.followPath.progress", &KeyValue::Number(1.25)).unwrap();
    let mut quarter = more.clone();
    quarter.objects[1].set("constraints.followPath.progress", &KeyValue::Number(0.25)).unwrap();
    assert!(near(pos(&more.objects_at(0.0), "car"), pos(&quarter.objects_at(0.0), "car"), 1e-9));
}

#[test]
fn influence_blends() {
    let s = space(json!({"objects": [
        {"id": "goal", "type": "sphere", "position": [10, 0, 0]},
        {"id": "half", "type": "box", "constraints": [{"type": "copyPosition", "target": "goal", "influence": 0.5}]},
        {"id": "turn", "type": "box", "position": [0, 0, 0], "constraints": [{"type": "lookAt", "target": "goal", "influence": 0.5}]},
        {"id": "off", "type": "box", "position": [1, 1, 1], "constraints": [{"type": "copyPosition", "target": "goal", "influence": 0}]}
    ]}));
    let objs = s.objects_at(0.0);
    assert!(near(pos(&objs, "half"), [5.0, 0.0, 0.0], 1e-9));
    // Facing +z at first, +x when done: half way is 45°.
    let front = world(&objs, "turn").dir([0.0, 0.0, 1.0]);
    assert!((angle(front, [1.0, 0.0, 0.0]) - 45.0).abs() < 0.5, "{front:?}");
    assert_eq!(objs[3].position.0, [1.0, 1.0, 1.0], "no influence, untouched");
}

#[test]
fn constraints_through_parents() {
    let s = space(json!({"objects": [
        {"id": "beacon", "type": "sphere", "position": [4, 2, -3]},
        {"id": "arm", "type": "group", "position": [1, 1, 1], "rotation": [30, 45, 0], "scale": [2, 1, 1],
         "keyframes": {"rotation.y": [[0, 0], [1, 90]]},
         "children": [
            {"id": "hand", "type": "box", "position": [1, 0, 0], "constraints": [{"type": "copyPosition", "target": "beacon", "offset": [0, 1, 0]}],
             "children": [{"id": "finger", "type": "box", "position": [0, 0.5, 0]}]}
         ]},
        {"id": "low", "type": "box", "position": [0, -3, 9], "constraints": [{"type": "floor", "height": -1}, {"type": "limitPosition", "max": [5, 5, 5]}]},
        {"id": "twin", "type": "box", "constraints": [{"type": "copyRotation", "target": "arm"}, {"type": "copyScale", "target": "arm"}]}
    ]}));
    for t in [0.0, 0.5, 1.0] {
        let objs = s.objects_at(t);
        assert!(near(pos(&objs, "hand"), [4.0, 3.0, -3.0], 1e-9), "world position, whatever the parent does ({t})");
        // Children of a constrained object move with it.
        let hand = world(&objs, "hand");
        assert!(near(pos(&objs, "finger"), hand.point([0.0, 0.5, 0.0]), 1e-9));
        let (arm, twin) = (world(&objs, "arm").pose(), world(&objs, "twin").pose());
        assert!(!rot_differs_by(&arm.rot, &twin.rot, 1e-9), "copies the rotation ({t})");
        assert!(near(arm.scale, twin.scale, 1e-9));
    }
    let objs = s.objects_at(0.0);
    assert_eq!(pos(&objs, "low"), [0.0, -1.0, 5.0]);
}

fn rot_differs_by(a: &R3, b: &R3, eps: f64) -> bool {
    (0..3).any(|r| (0..3).any(|c| (a[r][c] - b[r][c]).abs() > eps))
}

#[test]
fn expressions_then_constraints_in_3d() {
    let s = space(json!({
        "materials": [{"id": "gold", "color": "#ffcc00", "metallic": 1}],
        "objects": [
            {"id": "cube", "type": "box", "keyframes": {"position.x": [[0, 0], [2, 4]]}, "expressions": {"rotation.y": "time * 90", "position.y": "prop('cube', 'position.x') * 2"}},
            {"id": "moon", "type": "sphere", "material": "gold", "expressions": {"position": "[prop('cube', 'x'), 3, 0]", "roughness": "0.25", "color": "mix(value, '#000000', 0.5)"}},
            {"id": "pinned", "type": "sphere", "expressions": {"position.x": "100"}, "constraints": [{"type": "copyPosition", "target": "cube"}]},
            {"id": "row", "type": "group", "children": [
                {"id": "r1", "type": "box", "expressions": {"x": "index * 2"}},
                {"id": "r2", "type": "box", "expressions": {"x": "index * 2"}}
            ]}
        ],
        "camera": {"expressions": {"fov": "30 + time * 10"}},
        "lights": [{"id": "lamp", "type": "point", "expressions": {"intensity": "2", "color": "[0, 0, 1]"}}]
    }));
    let objs = s.objects_at(1.0);
    let cube = &objs[0];
    assert_eq!(cube.rotation.0[1], 90.0);
    assert_eq!(cube.position.0, [2.0, 4.0, 0.0], "a formula reads another property of the same object");
    assert_eq!(objs[1].position.0, [2.0, 3.0, 0.0]);
    assert_eq!(objs[1].material.roughness, 0.25);
    assert_eq!(objs[1].material.metallic, 1.0, "the shared material is in place");
    assert_eq!(objs[1].material.color, "#806600", "value is the shared colour");
    assert_eq!(objs[2].position.0, [2.0, 4.0, 0.0], "the constraint has the last word");
    assert_eq!((objs[3].children[0].position.0[0], objs[3].children[1].position.0[0]), (2.0, 4.0));
    assert_eq!(s.camera_at(1.0).fov, 40.0);
    let lamp = &s.lights_at(1.0)[0];
    assert_eq!((lamp.intensity, lamp.color.as_str()), (2.0, "#0000ff"));
}

#[test]
fn constraint_loops_end() {
    let s = space(json!({"objects": [
        {"id": "a", "type": "box", "position": [0, 0, 0], "constraints": [{"type": "lookAt", "target": "b"}, {"type": "copyPosition", "target": "b", "influence": 0.5}]},
        {"id": "b", "type": "box", "position": [4, 0, 0], "constraints": [{"type": "lookAt", "target": "a"}, {"type": "copyPosition", "target": "a", "influence": 0.5}]},
        {"id": "self", "type": "box", "position": [1, 2, 3], "constraints": [{"type": "copyPosition", "target": "self"}]}
    ]}));
    let objs = s.objects_at(0.0);
    assert!(objs.iter().all(|o| o.position.0.iter().all(|v| v.is_finite())));
    assert_eq!(objs[2].position.0, [1.0, 2.0, 3.0], "following itself does nothing");
}

#[test]
fn camera_on_a_path_looks_ahead() {
    let s = space(json!({
        "objects": [{"id": "rail", "type": "curve", "smooth": false, "points": [[0, 1, 10], [0, 1, -10]]}],
        "camera": {"position": [0, 0, 0], "target": [0, 0, -1], "constraints": [{"type": "followPath", "path": "rail", "progress": 0.5}]}
    }));
    let cam = s.camera_at(0.0);
    assert!(near(cam.position.0, [0.0, 1.0, 0.0], 1e-9));
    assert!(angle(sub(cam.target.0, cam.position.0), [0.0, 0.0, -1.0]) < 0.5, "looks along the rail: {:?}", cam.target);
}

#[test]
fn rotations_round_trip() {
    for r in [[0.0, 0.0, 0.0], [10.0, 20.0, 30.0], [-80.0, 45.0, 170.0], [0.0, 90.0, 0.0], [30.0, -90.0, 10.0]] {
        let m = rot_euler(r);
        let back = rot_euler(euler(&m));
        assert!(!rot_differs_by(&m, &back, 1e-9), "{r:?}");
        let q = from_quat(quat(&m));
        assert!(!rot_differs_by(&m, &q, 1e-9), "{r:?}");
    }
    // The same convention as the renderer: rotating x by 90° about y gives −z.
    let m = M4::trs([0.0; 3], &rot_euler([0.0, 90.0, 0.0]), [1.0; 3]);
    assert!(near(m.point([1.0, 0.0, 0.0]), [0.0, 0.0, -1.0], 1e-12));
}
