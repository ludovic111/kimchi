//! Camera moves: the classic shots (orbit, turntable, dolly, truck, crane, zoom, fly-through,
//! handheld) written into a 3D scene as ordinary keyframes, constraints and expressions, so they
//! stay editable afterwards: in the Studio's dope sheet, its Constraints and Expressions tabs, or
//! with any `motion.*` command. `motion.cameraMove` applies them, and so does the Studio's Camera
//! menu.
//!
//! What each move writes on the camera:
//! - orbit / turntable: an invisible circle (a curve object without a radius) around the pivot,
//!   a `followPath` constraint (id `orbit`) on it, and keyframes of its progress; the camera keeps
//!   looking at the pivot (its target, or a `lookAt` constraint, id `aim`, on an object).
//! - dolly, truck, crane: two keyframes of `position` (and `target` for a truck).
//! - zoom: two keyframes of `fov` (or `orthoSize`).
//! - flyThrough: a `followPath` constraint (id `flyThrough`) along a curve, with keyframes of its
//!   progress from 0 to 1.
//! - handheld: `wiggle` expressions on `position`, `target` and `roll`.

use serde_json::{Map, Value, json};

use super::{Camera, Scene3d, Shape3d, find_object, stack};
use crate::anim::{Easing, KeyValue, Keyframe, normalize, set_key};

/// The move names `motion.cameraMove` takes.
pub const MOVES: &[&str] = &["orbit", "turntable", "dolly", "truck", "crane", "zoom", "flyThrough", "handheld", "clear"];

/// The ids of the constraints the moves write (a new move replaces the ones it would fight).
const ORBIT: &str = "orbit";
const FLY: &str = "flyThrough";
const AIM: &str = "aim";

/// Where a fly-through goes.
#[derive(Clone, Debug, PartialEq)]
pub enum FlyPath {
    /// An existing curve object.
    Curve(String),
    /// A new curve through these points.
    Points(Vec<[f64; 3]>),
    /// A new curve sweeping past what the camera looks at (it keeps looking there).
    Sweep,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Move {
    /// Around `centre` (default: what the camera looks at) by `degrees`; positive: the camera
    /// travels to its right. `aim` keeps it facing an object (a `lookAt`).
    Orbit { centre: Option<[f64; 3]>, aim: Option<String>, degrees: f64 },
    /// Along the view: + in, − out (world units; default a third of the way to the target).
    Dolly { distance: Option<f64> },
    /// Sideways with what it looks at: + right.
    Truck { distance: Option<f64> },
    /// Up (+) or down, still looking at the same point.
    Crane { distance: Option<f64> },
    /// The lens: degrees of field of view added (− = closer).
    Zoom { amount: f64 },
    /// Along a curve (`followPath`), facing along it unless `look_at` names an object to face.
    FlyThrough { path: FlyPath, look_at: Option<String> },
    /// A gentle shake (1 = gentle, 3 = running).
    Handheld { amount: f64 },
    /// Removes the camera's animation: keyframes, expressions, the moves' constraints and paths.
    Clear,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraMove {
    /// The camera's id (`"camera"` is the main one).
    pub camera: String,
    /// Scene seconds the move starts and ends.
    pub from: f64,
    pub to: f64,
    /// How the move arrives (default: ease in and out; linear for a turntable).
    pub easing: Option<Easing>,
    pub kind: Move,
}

/// Applies a move to the scene's camera; says what it wrote.
pub fn apply(s: &mut Scene3d, m: &CameraMove) -> Result<String, String> {
    let id = if m.camera.is_empty() { "camera" } else { m.camera.as_str() }.to_string();
    let Some(src) = s.camera_by_id(&id).cloned() else {
        let mut ids = vec!["camera".to_string()];
        ids.extend(s.cameras.iter().map(|c| c.id.clone()));
        return Err(format!("No camera \"{id}\". Cameras: {}.", ids.join(", ")));
    };
    let (from, to) = (m.from.max(0.0), m.to);
    if !matches!(m.kind, Move::Handheld { .. } | Move::Clear) && !(to > from + 1e-3) {
        return Err(format!("The move needs to end after it starts (from {from} s, to {to} s)."));
    }
    // What it films at the start (constraints and expressions applied), and its own keyframed
    // values (what keyframes continue from).
    let posed = s.camera_by_id_at(&id, from).unwrap_or_else(|| src.at(from));
    let keyed = src.at(from);
    let (eye, look) = (posed.position.0, posed.target.0);
    let dist = len(sub(look, eye)).max(1e-3);
    let fwd = norm(sub(look, eye));
    let right = horizontal_right(fwd);
    let ease = |default: Easing| m.easing.clone().unwrap_or(default);
    let in_out = Easing::parse("easeInOut").unwrap_or_default();
    let summary;
    match &m.kind {
        Move::Orbit { centre, aim, degrees } => {
            let c = centre.unwrap_or(look);
            let (dx, dz) = (eye[0] - c[0], eye[2] - c[2]);
            let mut r = dx.hypot(dz);
            let mut a0 = dz.atan2(dx);
            if r < 0.05 {
                // Straight above or below: start on the camera's right.
                r = (dist * 0.5).max(0.5);
                a0 = right[2].atan2(right[0]);
            }
            // Positive degrees: towards the camera's right.
            let tangent = [-a0.sin(), 0.0, a0.cos()];
            let sign = if dot(tangent, right) >= 0.0 { 1.0 } else { -1.0 };
            const N: usize = 16;
            let points: Vec<[f64; 3]> = (0..N)
                .map(|k| {
                    let a = a0 + sign * k as f64 * std::f64::consts::TAU / N as f64;
                    [round(c[0] + r * a.cos()), round(eye[1]), round(c[2] + r * a.sin())]
                })
                .collect();
            let curve = path_curve(s, &format!("{}Orbit", stem(&id)), &points, true)?;
            let cam = camera_mut(s, &id);
            drop_moves(cam, &[ORBIT, FLY]);
            add_constraint(cam, ORBIT, "followPath", json!({ "path": curve, "align": false, "progress": 0 }));
            let turns = degrees.abs() / 360.0;
            let easing = ease(if (degrees.abs() - 360.0).abs() < 1e-9 { Easing::Linear } else { in_out.clone() });
            keys(cam, &format!("constraints.{ORBIT}.progress"), from, KeyValue::Number(0.0), to, KeyValue::Number(round(turns)), easing);
            aim_at(cam, aim.as_deref(), c, from, to);
            summary = format!(
                "The camera goes {}° around ({:.2}, {:.2}, {:.2}) from {from} s to {to} s: a followPath constraint (\"{ORBIT}\") on the invisible curve \"{curve}\", with keyframes of constraints.{ORBIT}.progress.",
                round(*degrees),
                c[0],
                c[1],
                c[2]
            );
        }
        Move::Dolly { distance } => {
            let d = distance.unwrap_or(dist / 3.0);
            let cam = camera_mut(s, &id);
            drop_moves(cam, &[ORBIT, FLY]);
            let p = keyed.position.0;
            keys(cam, "position", from, vec3(p), to, vec3(add(p, scale(fwd, d))), ease(in_out.clone()));
            summary = format!("The camera moves {} {:.2} along its view from {from} s to {to} s (position keyframes).", if d >= 0.0 { "in" } else { "out" }, d.abs());
        }
        Move::Truck { distance } => {
            let d = distance.unwrap_or(dist / 3.0);
            let cam = camera_mut(s, &id);
            drop_moves(cam, &[ORBIT, FLY]);
            let (p, q) = (keyed.position.0, keyed.target.0);
            let off = scale(right, d);
            let e = ease(in_out.clone());
            keys(cam, "position", from, vec3(p), to, vec3(add(p, off)), e.clone());
            keys(cam, "target", from, vec3(q), to, vec3(add(q, off)), e);
            summary = format!("The camera slides {:.2} to the {} from {from} s to {to} s (position and target keyframes).", d.abs(), if d >= 0.0 { "right" } else { "left" });
        }
        Move::Crane { distance } => {
            let d = distance.unwrap_or(dist / 3.0);
            let cam = camera_mut(s, &id);
            drop_moves(cam, &[ORBIT, FLY]);
            let p = keyed.position.0;
            keys(cam, "position", from, vec3(p), to, vec3(add(p, [0.0, d, 0.0])), ease(in_out.clone()));
            summary = format!("The camera rises {:.2} from {from} s to {to} s, still looking at the same point (position keyframes).", d);
        }
        Move::Zoom { amount } => {
            let cam = camera_mut(s, &id);
            let e = ease(in_out.clone());
            if keyed.orthographic() {
                let k = ((keyed.fov + amount) / keyed.fov.max(1.0)).clamp(0.05, 20.0);
                keys(cam, "orthoSize", from, KeyValue::Number(keyed.ortho_size), to, KeyValue::Number(round(keyed.ortho_size * k)), e);
            } else {
                let fov = (keyed.fov + amount).clamp(5.0, 150.0);
                keys(cam, "fov", from, KeyValue::Number(keyed.fov), to, KeyValue::Number(round(fov)), e);
            }
            summary = format!("The lens zooms {} from {from} s to {to} s (fov keyframes).", if *amount < 0.0 { "in" } else { "out" });
        }
        Move::FlyThrough { path, look_at } => {
            let (curve, sweep) = match path {
                FlyPath::Curve(c) => {
                    match find_object(&s.objects, c).map(|o| &o.shape) {
                        Some(Shape3d::Curve { .. }) => {}
                        Some(other) => return Err(format!("\"{c}\" is a {}; a fly-through follows a curve object.", other.name())),
                        None => return Err(format!("No object \"{c}\" in this scene.")),
                    }
                    (c.clone(), false)
                }
                FlyPath::Points(p) => {
                    if p.len() < 2 {
                        return Err("A fly-through path needs at least two points.".into());
                    }
                    (path_curve(s, &format!("{}Path", stem(&id)), p, false)?, false)
                }
                FlyPath::Sweep => {
                    // A sweep past what it looks at: a third of the way around, a little closer
                    // and lower, starting where the camera is.
                    let (dx, dz) = (eye[0] - look[0], eye[2] - look[2]);
                    let r = dx.hypot(dz).max(dist * 0.5).max(0.5);
                    let a0 = if dx.hypot(dz) < 0.05 { right[2].atan2(right[0]) } else { dz.atan2(dx) };
                    let tangent = [-a0.sin(), 0.0, a0.cos()];
                    let sign = if dot(tangent, right) >= 0.0 { 1.0 } else { -1.0 };
                    let steps = [(0.0, 1.0, 1.0), (40.0, 0.85, 0.75), (80.0, 0.72, 0.55), (120.0, 0.8, 0.5)];
                    let pts: Vec<[f64; 3]> = steps
                        .iter()
                        .map(|(deg, rk, hk)| {
                            let a = a0 + sign * f64::to_radians(*deg);
                            [round(look[0] + r * rk * a.cos()), round(look[1] + (eye[1] - look[1]) * hk), round(look[2] + r * rk * a.sin())]
                        })
                        .collect();
                    (path_curve(s, &format!("{}Path", stem(&id)), &pts, false)?, true)
                }
            };
            if let Some(o) = look_at
                && find_object(&s.objects, o).is_none()
            {
                return Err(format!("No object \"{o}\" to look at."));
            }
            let cam = camera_mut(s, &id);
            drop_moves(cam, &[ORBIT, FLY, AIM]);
            add_constraint(cam, FLY, "followPath", json!({ "path": curve, "align": look_at.is_none() && !sweep, "progress": 0 }));
            keys(cam, &format!("constraints.{FLY}.progress"), from, KeyValue::Number(0.0), to, KeyValue::Number(1.0), ease(in_out.clone()));
            if let Some(o) = look_at {
                add_constraint(cam, AIM, "lookAt", json!({ "target": o }));
            } else if sweep {
                aim_at(cam, None, look, from, to);
            }
            summary = format!(
                "The camera flies along \"{curve}\" from {from} s to {to} s, {}: a followPath constraint (\"{FLY}\") with keyframes of constraints.{FLY}.progress.",
                match (look_at, sweep) {
                    (Some(o), _) => format!("looking at \"{o}\""),
                    (None, true) => "looking at the same point".to_string(),
                    (None, false) => "facing along the path".to_string(),
                }
            );
        }
        Move::Handheld { amount } => {
            let k = amount.clamp(0.0, 10.0);
            let cam = camera_mut(s, &id);
            if k <= 0.0 {
                for p in ["position", "target", "roll"] {
                    cam.expressions.remove(p);
                }
                summary = "The shake is off.".into();
            } else {
                let amp = round(dist * 0.012 * k);
                cam.expressions.insert("position".into(), format!("wiggle(1.3, {amp})"));
                cam.expressions.insert("target".into(), format!("wiggle(0.9, {})", round(amp * 0.6)));
                cam.expressions.insert("roll".into(), format!("wiggle(0.8, {})", round(0.6 * k)));
                summary = format!("The camera shakes gently (wiggle expressions on position, target and roll, about {amp} units).");
            }
        }
        Move::Clear => {
            let cam = camera_mut(s, &id);
            cam.keyframes.clear();
            for p in ["position", "target", "roll", "fov"] {
                cam.expressions.remove(p);
            }
            drop_moves(cam, &[ORBIT, FLY, AIM]);
            // The camera stays where it is now.
            cam.position = posed.position;
            cam.target = posed.target;
            cam.fov = keyed.fov;
            cam.roll = keyed.roll;
            let mut gone = vec![];
            for curve in [format!("{}Orbit", stem(&id)), format!("{}Path", stem(&id))] {
                if matches!(find_object(&s.objects, &curve).map(|o| &o.shape), Some(Shape3d::Curve { radius, .. }) if *radius == 0.0) && !used_elsewhere(s, &curve) {
                    super::remove_object(&mut s.objects, &curve);
                    gone.push(curve);
                }
            }
            summary = format!("The camera's animation is gone (keyframes, expressions, moves){}.", if gone.is_empty() { String::new() } else { format!("; removed the paths {}", gone.join(", ")) });
        }
    }
    Ok(summary)
}

/// The main camera is `camera`; paths are named after it (`cameraOrbit`).
fn stem(id: &str) -> &str {
    id
}

fn camera_mut<'a>(s: &'a mut Scene3d, id: &str) -> &'a mut Camera {
    if id == "camera" {
        return &mut s.camera;
    }
    s.cameras.iter_mut().find(|c| c.id == id).expect("checked")
}

/// Removes the moves' constraints `ids` and the keyframes of their parameters.
fn drop_moves(cam: &mut Camera, ids: &[&str]) {
    cam.constraints.retain(|c| !ids.contains(&c.id.as_str()));
    cam.keyframes.retain(|name, _| !ids.iter().any(|i| name.starts_with(&format!("constraints.{i}."))));
}

fn add_constraint(cam: &mut Camera, id: &str, kind: &str, params: Value) {
    let params: Map<String, Value> = params.as_object().cloned().unwrap_or_default();
    let mut c = stack::Constraint::new(kind, params);
    c.id = id.to_string();
    cam.constraints.retain(|k| k.id != id);
    cam.constraints.push(c);
}

/// Two keyframes of `name`: `a` at `from`, `b` at `to` (arriving with `easing`); keys between
/// them go (the move replaces what was there).
fn keys(cam: &mut Camera, name: &str, from: f64, a: KeyValue, to: f64, b: KeyValue, easing: Easing) {
    if let Some(list) = cam.keyframes.get_mut(name) {
        list.retain(|k| k.time < from - 1e-6 || k.time > to + 1e-6);
    }
    let first_easing = cam.keyframes.get(name).and_then(|l| l.iter().find(|k| (k.time - from).abs() < 1e-6)).map(|k| k.easing.clone()).unwrap_or_default();
    set_key(&mut cam.keyframes, name, Keyframe { time: from, value: a, easing: first_easing });
    set_key(&mut cam.keyframes, name, Keyframe { time: to, value: b, easing });
    normalize(&mut cam.keyframes);
}

/// Keeps the camera looking at `object` (a lookAt) or at the point `c` (its target).
fn aim_at(cam: &mut Camera, object: Option<&str>, c: [f64; 3], from: f64, to: f64) {
    match object {
        Some(o) => add_constraint(cam, AIM, "lookAt", json!({ "target": o })),
        None => {
            cam.constraints.retain(|k| k.id != AIM);
            if cam.keyframes.contains_key("target") {
                keys(cam, "target", from, vec3(c), to, vec3(c), Easing::Linear);
            } else {
                cam.target = super::Vec3(c.map(round));
            }
        }
    }
}

/// Adds (or replaces) an invisible curve `id` through `points` (a path for followPath); gives
/// its id (another one when `id` is already something else).
fn path_curve(s: &mut Scene3d, id: &str, points: &[[f64; 3]], closed: bool) -> Result<String, String> {
    let mut id = id.to_string();
    let mut n = 2;
    while matches!(find_object(&s.objects, &id).map(|o| &o.shape), Some(shape) if !matches!(shape, Shape3d::Curve { .. })) || s.lights.iter().any(|l| l.id == id) || s.cameras.iter().any(|c| c.id == id) {
        id = format!("{}{n}", id.trim_end_matches(char::is_numeric));
        n += 1;
    }
    let curve = json!({ "id": id, "type": "curve", "points": points, "closed": closed });
    let obj: super::Object3d = serde_json::from_value(curve).map_err(|e| format!("path: {e}"))?;
    match super::find_object_mut(&mut s.objects, &id) {
        Some(slot) => *slot = obj,
        None => s.objects.push(obj),
    }
    Ok(id)
}

/// Another thing than the cameras' moves refers to this curve.
fn used_elsewhere(s: &Scene3d, id: &str) -> bool {
    let mut used = false;
    super::walk_objects(&s.objects, &mut |o| {
        if o.constraints.iter().any(|c| c.opt_s("path").as_deref() == Some(id)) {
            used = true;
        }
    });
    let path = |list: &[stack::Constraint]| list.iter().any(|c| c.opt_s("path").as_deref() == Some(id));
    used || s.lights.iter().any(|l| path(&l.constraints)) || path(&s.camera.constraints) || s.cameras.iter().any(|c| path(&c.constraints))
}

/// The view's right, kept level (a camera looking straight down still has a right).
fn horizontal_right(fwd: [f64; 3]) -> [f64; 3] {
    let up = if fwd[1].abs() > 0.999 { [0.0, 0.0, -1.0] } else { [0.0, 1.0, 0.0] };
    norm(cross(fwd, up))
}

fn vec3(v: [f64; 3]) -> KeyValue {
    KeyValue::Vector(v.map(round).to_vec())
}

fn round(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = len(a);
    if l < 1e-12 { [0.0, 0.0, -1.0] } else { scale(a, 1.0 / l) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::Scene;

    fn scene() -> Scene3d {
        let s = Scene::from_json(&json!({ "type": "3d", "camera": { "position": [0, 2, 8], "target": [0, 1, 0] }, "objects": [{ "id": "box", "type": "box" }] })).unwrap();
        match s {
            Scene::Space(s) => s,
            _ => unreachable!(),
        }
    }

    fn mv(kind: Move) -> CameraMove {
        CameraMove { camera: "camera".into(), from: 0.0, to: 4.0, easing: None, kind }
    }

    fn valid(s: &Scene3d) {
        Scene::Space(s.clone()).validate().expect("the scene stays valid");
    }

    #[test]
    fn an_orbit_goes_around_the_target_at_the_same_distance() {
        let mut s = scene();
        apply(&mut s, &mv(Move::Orbit { centre: None, aim: None, degrees: 90.0 })).unwrap();
        valid(&s);
        assert!(s.camera.constraints.iter().any(|c| c.id == "orbit" && c.kind == "followPath"));
        let start = s.camera_at(0.0);
        let end = s.camera_at(4.0);
        let mid = s.camera_at(2.0);
        let r = |c: &Camera| (c.position.0[0]).hypot(c.position.0[2]);
        assert!((r(&start) - 8.0).abs() < 0.02, "starts where it was: {:?}", start.position);
        assert!((start.position.0[2] - 8.0).abs() < 0.02 && start.position.0[0].abs() < 0.02, "{:?}", start.position);
        assert!((r(&end) - 8.0).abs() < 0.05 && (r(&mid) - 8.0).abs() < 0.05, "a circle: {:?} {:?}", mid.position, end.position);
        // A quarter turn, towards the camera's right (+x when looking down −z).
        assert!((end.position.0[0] - 8.0).abs() < 0.05 && end.position.0[2].abs() < 0.05, "{:?}", end.position);
        assert_eq!(end.target.0, [0.0, 1.0, 0.0], "still looking at the target");
        assert!((end.position.0[1] - 2.0).abs() < 1e-6, "same height");
        // Again: replaced, not stacked.
        apply(&mut s, &mv(Move::Orbit { centre: Some([0.0, 0.0, 0.0]), aim: Some("box".into()), degrees: -360.0 })).unwrap();
        valid(&s);
        assert_eq!(s.camera.constraints.iter().filter(|c| c.kind == "followPath").count(), 1);
        assert!(s.camera.constraints.iter().any(|c| c.kind == "lookAt"));
        let back = s.camera_at(4.0);
        assert!((back.position.0[0] - start.position.0[0]).abs() < 0.05 && (back.position.0[2] - start.position.0[2]).abs() < 0.05, "a whole turn ends where it began");
    }

    #[test]
    fn dolly_truck_crane_and_zoom_write_two_keyframes() {
        let mut s = scene();
        apply(&mut s, &mv(Move::Dolly { distance: Some(2.0) })).unwrap();
        let end = s.camera_at(4.0);
        assert!((end.position.0[2] - (8.0 - 2.0 * 8.0 / 65f64.sqrt())).abs() < 1e-3, "{:?}", end.position);
        assert_eq!(s.camera.keyframes["position"].len(), 2);
        let mut s = scene();
        apply(&mut s, &mv(Move::Truck { distance: Some(1.0) })).unwrap();
        let end = s.camera_at(4.0);
        assert!((end.position.0[0] - 1.0).abs() < 1e-6 && (end.target.0[0] - 1.0).abs() < 1e-6);
        let mut s = scene();
        apply(&mut s, &mv(Move::Crane { distance: Some(-1.0) })).unwrap();
        assert!((s.camera_at(4.0).position.0[1] - 1.0).abs() < 1e-6);
        assert_eq!(s.camera_at(4.0).target.0, [0.0, 1.0, 0.0]);
        apply(&mut s, &mv(Move::Zoom { amount: -15.0 })).unwrap();
        assert!((s.camera_at(4.0).fov - 25.0).abs() < 1e-6);
        valid(&s);
        assert!(apply(&mut s, &CameraMove { from: 3.0, to: 1.0, ..mv(Move::Dolly { distance: None }) }).unwrap_err().contains("end after"));
    }

    #[test]
    fn fly_throughs_follow_curves_and_handheld_shakes() {
        let mut s = scene();
        apply(&mut s, &mv(Move::FlyThrough { path: FlyPath::Points(vec![[0.0, 1.0, 6.0], [3.0, 1.0, 0.0], [0.0, 1.0, -6.0]]), look_at: Some("box".into()) })).unwrap();
        valid(&s);
        assert!(find_object(&s.objects, "cameraPath").is_some(), "a path was made");
        let end = s.camera_at(4.0);
        assert!((end.position.0[2] + 6.0).abs() < 0.05, "{:?}", end.position);
        // An existing curve, and only curves.
        assert!(apply(&mut s, &mv(Move::FlyThrough { path: FlyPath::Curve("box".into()), look_at: None })).unwrap_err().contains("box"));
        apply(&mut s, &mv(Move::FlyThrough { path: FlyPath::Sweep, look_at: None })).unwrap();
        valid(&s);
        apply(&mut s, &mv(Move::Handheld { amount: 1.0 })).unwrap();
        valid(&s);
        let (a, b) = (s.camera_at(1.0), s.camera_at(1.5));
        assert!(a.position != b.position, "it moves");
        // Clear: back to a still camera, the made paths gone.
        apply(&mut s, &mv(Move::Clear)).unwrap();
        valid(&s);
        assert!(s.camera.keyframes.is_empty() && s.camera.expressions.is_empty() && s.camera.constraints.is_empty());
        assert!(find_object(&s.objects, "cameraPath").is_none());
        assert!(apply(&mut s, &CameraMove { camera: "nope".into(), ..mv(Move::Clear) }).unwrap_err().contains("No camera"));
    }
}
