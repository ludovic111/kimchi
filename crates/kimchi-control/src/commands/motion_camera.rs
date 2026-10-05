//! `motion.cameraMove`: the classic camera moves (orbit, turntable, dolly, truck, crane, zoom,
//! fly-through, handheld) written into a 3D scene as keyframes, constraints and expressions
//! (see `kimchi_core::motion::camera_moves`), one undo step each.

use std::sync::Arc;

use kimchi_core::motion::Scene;
use kimchi_core::motion::camera_moves::{self, CameraMove, FlyPath, Move};
use serde_json::{Value, json};

use super::motion::{motion_clip, set_scene};
use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "motion.cameraMove" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let Scene::Space(space) = &mut scene else {
                return Err(format!("\"{}\" is a 2D scene; camera moves are for 3D scenes (animate a 2D layer with motion.setKeyframes).", clip.name));
            };
            let name = a.str("move")?;
            let Some(kind) = camera_moves::MOVES.iter().find(|m| m.eq_ignore_ascii_case(name)).copied() else {
                let hint = crate::registry::closest(name, camera_moves::MOVES).map(|c| format!(" Did you mean {c}?")).unwrap_or_default();
                return Err(format!("move is one of {}, not `{name}`.{hint}", camera_moves::MOVES.join(", ")));
            };
            // The clip's span in scene seconds (a reversed clip runs it backwards).
            let (a0, a1) = (clip.scene_time(clip.start), clip.scene_time(clip.end()));
            let (lo, hi) = (a0.min(a1).max(0.0), a0.max(a1));
            let from = a.opt_f64("from").unwrap_or(lo);
            let to = a.opt_f64("to").unwrap_or(hi);
            let easing = a.opt_str("easing").map(kimchi_core::Easing::parse).transpose()?;
            let camera = a.opt_str("camera").map(str::to_string).unwrap_or_else(|| space.active_camera_at(from));
            // A pivot: a point, or an object (its middle, and the camera keeps facing it).
            let (centre, aim) = match a.get("around") {
                None | Some(Value::Null) => (None, None),
                Some(Value::String(id)) => {
                    let (lo, hi) = kimchi_media::render::space::viewport::object_bounds(space, from, id).ok_or_else(|| format!("No object \"{id}\" to go around. Ids: {}.", scene_ids(space)))?;
                    (Some([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0]), Some(id.clone()))
                }
                Some(v) => (Some(point(v).ok_or("around is an object id or a point [x, y, z]")?), None),
            };
            let kind = match kind {
                "orbit" | "turntable" => Move::Orbit { centre, aim, degrees: a.opt_f64("degrees").unwrap_or(if kind == "turntable" { 360.0 } else { 90.0 }) },
                "dolly" => Move::Dolly { distance: a.opt_f64("distance") },
                "truck" => Move::Truck { distance: a.opt_f64("distance") },
                "crane" => Move::Crane { distance: a.opt_f64("distance") },
                "zoom" => Move::Zoom { amount: a.opt_f64("amount").unwrap_or(-15.0) },
                "flyThrough" => {
                    let path = match (a.opt_str("path"), a.array("points")) {
                        (Some(c), _) => FlyPath::Curve(c.to_string()),
                        (None, Some(list)) => FlyPath::Points(list.iter().map(|v| point(v).ok_or("points are [x, y, z] lists")).collect::<Result<_, _>>()?),
                        (None, None) => FlyPath::Sweep,
                    };
                    Move::FlyThrough { path, look_at: a.opt_str("lookAt").map(str::to_string).or(aim) }
                }
                "handheld" => Move::Handheld { amount: a.opt_f64("amount").unwrap_or(1.0) },
                _ => Move::Clear,
            };
            let said = camera_moves::apply(space, &CameraMove { camera: camera.clone(), from, to, easing, kind })?;
            scene.validate()?;
            let mut out = set_scene(s, cx, &a, clip.id, scene, None)?;
            out["camera"] = json!(camera);
            out["did"] = json!(said);
            Ok(out)
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

fn point(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
}

fn scene_ids(s: &kimchi_core::motion::Scene3d) -> String {
    let mut ids = vec![];
    kimchi_core::motion::walk_objects(&s.objects, &mut |o| ids.push(o.id.clone()));
    ids.join(", ")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use crate::registry;
    use crate::session::{Session, SessionOptions, Source};

    async fn ok(s: &Arc<Session>, name: &str, params: Value) -> Value {
        registry::call(s, Source::Agent, name, params).await.unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn camera_moves_are_one_undo_step_of_editable_animation() {
        let dir = tempfile::tempdir().unwrap();
        let s = Session::new(SessionOptions { data_dir: Some(dir.path().join("data")), config_dir: Some(dir.path().join("config")), secrets: None, headless: true }).unwrap();
        ok(&s, "project.create", json!({ "name": "Moves", "width": 640, "height": 360 })).await;
        let added = ok(&s, "motion.add", json!({ "start": 0, "duration": 4, "scene": { "type": "3d", "camera": { "position": [0, 2, 8], "target": [0, 1, 0] }, "objects": [{ "id": "hero", "type": "box", "position": [0, 1, 0] }] } })).await;
        let clip = added["clips"][0]["id"].as_str().unwrap().to_string();

        let r = ok(&s, "motion.cameraMove", json!({ "clipId": clip, "move": "turntable", "around": "hero" })).await;
        assert!(r["did"].as_str().unwrap().contains("360°"), "{r}");
        let cam = ok(&s, "motion.get", json!({ "clipId": clip, "id": "camera" })).await;
        assert_eq!(cam["constraints"].as_array().map(Vec::len), Some(2), "followPath and lookAt: {cam}");
        assert_eq!(cam["keyframes"]["constraints.orbit.progress"][1]["time"], 4.0, "over the whole clip: {cam}");
        let steps = ok(&s, "history.list", json!({})).await;
        assert_eq!(steps["undo"][0]["label"], "motion.cameraMove");

        // Undo takes the whole move away, path included.
        ok(&s, "history.undo", json!({})).await;
        let scene = ok(&s, "motion.get", json!({ "clipId": clip })).await;
        assert!(scene["scene"]["camera"].get("constraints").is_none(), "{scene}");
        assert_eq!(scene["scene"]["objects"].as_array().map(Vec::len), Some(1));

        ok(&s, "motion.cameraMove", json!({ "clipId": clip, "move": "dolly", "distance": 2, "from": 1, "to": 3, "easing": "easeOutBack" })).await;
        let cam = ok(&s, "motion.get", json!({ "clipId": clip, "id": "camera" })).await;
        assert_eq!(cam["keyframes"]["position"][1]["easing"], "easeOutBack", "{cam}");

        let e = registry::call(&s, Source::Agent, "motion.cameraMove", json!({ "clipId": clip, "move": "orbitt" })).await.unwrap_err();
        assert!(e.contains("Did you mean orbit"), "{e}");
        let e = registry::call(&s, Source::Agent, "motion.cameraMove", json!({ "clipId": clip, "move": "orbit", "around": "ghost" })).await.unwrap_err();
        assert!(e.contains("No object \"ghost\""), "{e}");
        let flat = ok(&s, "motion.addTemplate", json!({ "template": "lowerThird", "start": 5 })).await;
        let e = registry::call(&s, Source::Agent, "motion.cameraMove", json!({ "clipId": flat["clips"][0]["id"], "move": "dolly" })).await.unwrap_err();
        assert!(e.contains("2D scene"), "{e}");
    }
}
