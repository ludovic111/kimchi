//! The Studio's `motion.*` commands: stacks (modifiers, constraints, effects, operators, masks,
//! text animators), expressions, shared materials, compositions, moving and copying things in
//! a scene, mesh modelling, keyframe shifting, the Studio's view as a picture, and rendering
//! motion clips ahead.

use std::sync::Arc;

use kimchi_core::motion::{self, Composition, Layer, LayerKind, Scene, stack};
use kimchi_core::{ClipContent, ClipPatch, Edit, Id};
use serde_json::{Map, Value, json};

use super::motion::{motion_clip, set_scene};
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session, err};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "motion.stackTypes" => {
            let want = a.opt_str("family");
            let mut out = Map::new();
            for (field, noun, types) in stack::families() {
                if want.is_some_and(|w| w != field) {
                    continue;
                }
                out.insert(field.into(), json!({ "noun": noun, "types": types.iter().map(type_json).collect::<Vec<_>>() }));
            }
            if out.is_empty() {
                let names: Vec<&str> = stack::families().iter().map(|f| f.0).collect();
                return Err(format!("family is one of {}", names.join(", ")));
            }
            Ok(Value::Object(out))
        }
        "motion.setStackItem" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (id, field) = (a.str("id")?, field(a.str("field")?)?);
            let item = a.object("item").ok_or("item is an object like {\"type\": \"blur\", \"radius\": 12}.")?.clone();
            let index = a.opt_u32("index").map(|i| i as usize);
            let mut new_id = String::new();
            edit_item(&mut scene, id, |o| {
                let list = o.entry(field).or_insert_with(|| Value::Array(vec![]));
                let arr = list.as_array_mut().ok_or("not a list")?;
                let given = item.get("id").and_then(Value::as_str).map(str::to_string);
                match given.as_deref().and_then(|g| arr.iter().position(|x| x.get("id").and_then(Value::as_str) == Some(g))) {
                    Some(i) => arr[i] = Value::Object(item.clone()),
                    None => {
                        let at = index.unwrap_or(arr.len()).min(arr.len());
                        arr.insert(at, Value::Object(item.clone()));
                    }
                }
                new_id = given.unwrap_or_default();
                Ok(())
            })
            .map_err(|e| format!("\"{id}\" in \"{}\": {e}", clip.name))?;
            if new_id.is_empty() {
                // Named by normalize: the item with its type that wasn't there before.
                new_id = stack_ids(&scene, id, field).into_iter().rev().find(|i| i.starts_with(item.get("type").and_then(Value::as_str).unwrap_or(""))).unwrap_or_default();
            }
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            Ok(json!({ "itemId": new_id, "clip": summary }))
        }
        "motion.removeStackItem" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (id, field, item_id) = (a.str("id")?, field(a.str("field")?)?, a.str("itemId")?);
            edit_item(&mut scene, id, |o| {
                let arr = o.get_mut(field).and_then(Value::as_array_mut).ok_or_else(|| format!("\"{id}\" has no {field}"))?;
                let before = arr.len();
                arr.retain(|x| x.get("id").and_then(Value::as_str) != Some(item_id));
                if arr.len() == before {
                    return Err(format!("no {field} item \"{item_id}\" on \"{id}\""));
                }
                // Its keyframes and formulas go with it.
                let prefix = format!("{field}.{item_id}.");
                for key in ["keyframes", "expressions"] {
                    if let Some(m) = o.get_mut(key).and_then(Value::as_object_mut) {
                        m.retain(|name, _| !name.starts_with(&prefix));
                    }
                }
                Ok(())
            })?;
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.moveStackItem" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (id, field, item_id) = (a.str("id")?, field(a.str("field")?)?, a.str("itemId")?);
            let to = a.opt_u32("index").ok_or("index is a whole number.")? as usize;
            edit_item(&mut scene, id, |o| {
                let arr = o.get_mut(field).and_then(Value::as_array_mut).ok_or_else(|| format!("\"{id}\" has no {field}"))?;
                let from = arr.iter().position(|x| x.get("id").and_then(Value::as_str) == Some(item_id)).ok_or_else(|| format!("no {field} item \"{item_id}\" on \"{id}\""))?;
                let it = arr.remove(from);
                arr.insert(to.min(arr.len()), it);
                Ok(())
            })?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.setExpression" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (id, property) = (a.str("id")?, a.str("property")?);
            let expr = a.opt_str("expression").map(str::trim).unwrap_or("");
            let ids = scene.ids().join(", ");
            let mut item = scene.item_mut(id).ok_or_else(|| format!("No \"{id}\" in \"{}\". Ids: {ids}.", clip.name))?;
            if item.get(property).is_none() {
                return Err(format!("\"{id}\" has no property `{property}` (motion.guide lists them)."));
            }
            let ex = item.expressions_mut().ok_or("The scene itself can't have expressions; give them to its layers, objects, lights or cameras.")?;
            if expr.is_empty() {
                ex.remove(property);
            } else {
                kimchi_core::expr::check(expr).map_err(|e| format!("The expression for `{property}`: {e}"))?;
                ex.insert(property.to_string(), expr.to_string());
            }
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.setMaterial" => {
            let p = s.project()?;
            let (clip, scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let mut json = scene.to_json();
            if !scene.is_3d() {
                return Err(format!("\"{}\" is a 2D scene; materials are for 3D scenes.", clip.name));
            }
            let m = a.object("material").ok_or("material is an object with an id.")?.clone();
            let mid = m.get("id").and_then(Value::as_str).filter(|i| !i.trim().is_empty()).ok_or("The material needs an id (objects use it: \"material\": \"<id>\").")?.to_string();
            let list = json.as_object_mut().ok_or("bad scene")?.entry("materials").or_insert_with(|| Value::Array(vec![]));
            let arr = list.as_array_mut().ok_or("bad materials")?;
            match arr.iter().position(|x| x.get("id").and_then(Value::as_str) == Some(mid.as_str())) {
                Some(i) => arr[i] = Value::Object(m),
                None => arr.push(Value::Object(m)),
            }
            let scene = Scene::from_json(&json)?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.removeMaterial" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let mid = a.str("materialId")?;
            let Scene::Space(sp) = &mut scene else { return Err(format!("\"{}\" is a 2D scene; it has no materials.", clip.name)) };
            let shared = sp.material(mid).cloned().ok_or_else(|| format!("No material \"{mid}\" in \"{}\".", clip.name))?;
            sp.materials.retain(|m| m.id.as_deref() != Some(mid));
            motion::walk_objects_mut(&mut sp.objects, &mut |o| {
                if o.material.from.as_deref() == Some(mid) {
                    let mut own = shared.clone();
                    own.from = None;
                    own.id = None;
                    o.material = own;
                }
            });
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.setComposition" => {
            let p = s.project()?;
            let (clip, scene, _) = motion_clip(&p, a.str("clipId")?)?;
            if scene.is_3d() {
                return Err(format!("\"{}\" is a 3D scene; compositions are for 2D scenes.", clip.name));
            }
            let mut c = a.object("composition").ok_or("composition is an object with an id.")?.clone();
            let cid = c.get("id").and_then(Value::as_str).filter(|i| !i.trim().is_empty()).ok_or("The composition needs an id.")?.to_string();
            let mut json = scene.to_json();
            let list = json.as_object_mut().ok_or("bad scene")?.entry("compositions").or_insert_with(|| Value::Array(vec![]));
            let arr = list.as_array_mut().ok_or("bad compositions")?;
            match arr.iter().position(|x| x.get("id").and_then(Value::as_str) == Some(cid.as_str())) {
                Some(i) => {
                    if !c.contains_key("layers") {
                        c.insert("layers".into(), arr[i].get("layers").cloned().unwrap_or(json!([])));
                    }
                    let mut merged = arr[i].as_object().cloned().unwrap_or_default();
                    merged.extend(c);
                    arr[i] = Value::Object(merged);
                }
                None => arr.push(Value::Object(c)),
            }
            let scene = Scene::from_json(&json)?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.removeComposition" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let cid = a.str("compositionId")?;
            let Scene::Flat(f) = &mut scene else { return Err(format!("\"{}\" is a 3D scene.", clip.name)) };
            let mut users = vec![];
            let mut see = |l: &Layer| {
                if let LayerKind::Comp { comp, .. } = &l.kind
                    && comp == cid
                {
                    users.push(l.id.clone());
                }
            };
            motion::walk_layers(&f.layers, &mut see);
            for c in &f.compositions {
                motion::walk_layers(&c.layers, &mut see);
            }
            if !users.is_empty() {
                return Err(format!("Composition \"{cid}\" is shown by {}; remove those layers first.", users.join(", ")));
            }
            let before = f.compositions.len();
            f.compositions.retain(|c| c.id != cid);
            if f.compositions.len() == before {
                return Err(format!("No composition \"{cid}\" in \"{}\".", clip.name));
            }
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.precompose" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let ids: Vec<String> = a.array("ids").ok_or("ids is a list of layer ids.")?.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
            let cid = a.str("compositionId")?.trim().to_string();
            if ids.is_empty() || cid.is_empty() {
                return Err("Give the layers (ids) and the new composition's id.".into());
            }
            let Scene::Flat(f) = &mut scene else { return Err(format!("\"{}\" is a 3D scene; pre-compose is for 2D.", clip.name)) };
            if f.composition(&cid).is_some() || f.find_layer(&cid).is_some() {
                return Err(format!("\"{cid}\" is already taken; pick another id."));
            }
            let list = sibling_list_mut(f, &ids[0]).ok_or_else(|| format!("No layer \"{}\".", ids[0]))?;
            if let Some(stray) = ids.iter().find(|i| !list.iter().any(|l| &l.id == *i)) {
                return Err(format!("\"{stray}\" isn't next to \"{}\"; pre-compose layers from the same list.", ids[0]));
            }
            let at = list.iter().position(|l| ids.contains(&l.id)).unwrap_or(0);
            let mut moved = vec![];
            list.retain(|l| {
                if ids.contains(&l.id) {
                    moved.push(l.clone());
                    false
                } else {
                    true
                }
            });
            let comp_layer: Layer = serde_json::from_value(json!({"id": cid, "type": "comp", "comp": cid})).map_err(err)?;
            list.insert(at.min(list.len()), comp_layer);
            f.compositions.push(Composition { id: cid.clone(), layers: moved, ..Default::default() });
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.moveLayer" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let id = a.str("id")?;
            move_item(&mut scene, id, a.opt_str("parent"), a.opt_u32("index").map(|i| i as usize)).map_err(|e| format!("\"{id}\" in \"{}\": {e}", clip.name))?;
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.duplicateLayer" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let id = a.str("id")?;
            let mut json = scene.item_json(id).ok_or_else(|| format!("No \"{id}\" in \"{}\".", clip.name))?;
            if id == "camera" {
                return Err("The main camera can't be copied; add another with motion.setLayer {\"type\": \"camera\", \"id\": …}.".into());
            }
            let taken: Vec<String> = scene.ids();
            let fresh = |base: &str, taken: &[String]| -> String {
                (2..).map(|n| format!("{base}{n}")).find(|c| !taken.contains(c)).expect("a free id")
            };
            let new_id = match a.opt_str("newId") {
                Some(n) if taken.iter().any(|t| t == n) => return Err(format!("\"{n}\" is already taken.")),
                Some(n) => n.to_string(),
                None => fresh(id, &taken),
            };
            json["id"] = Value::from(new_id.clone());
            // Children (groups, child objects) get fresh ids too.
            let mut all = taken.clone();
            all.push(new_id.clone());
            for key in ["layers", "children"] {
                rename_children(&mut json, key, &mut all, &fresh);
            }
            if scene.is_3d() && json.get("type").is_none() {
                json["type"] = Value::from("camera");
            }
            let parent = parent_of(&scene, id);
            scene.upsert(&json, parent.as_deref())?;
            // Next to the original.
            let index = index_of(&scene, id).map(|i| i + 1);
            move_item(&mut scene, &new_id, Some(parent.as_deref().unwrap_or("")), index)?;
            scene.normalize();
            scene.validate()?;
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            Ok(json!({ "id": new_id, "clip": summary }))
        }
        "motion.shiftKeyframes" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let id = a.str("id")?;
            let by = a.f64("by")? * clip.speed;
            let only = a.opt_str("property");
            let times: Option<Vec<f64>> = a.array("times").map(|v| v.iter().filter_map(Value::as_f64).map(|t| clip.scene_time(t)).collect());
            let keys = scene.keyframes_mut(id).ok_or_else(|| format!("No \"{id}\" in \"{}\".", clip.name))?;
            let mut moved = 0;
            for (name, list) in keys.iter_mut() {
                if only.is_some_and(|o| o != name) {
                    continue;
                }
                let picked: Vec<bool> = list.iter().map(|k| times.as_ref().is_none_or(|ts| ts.iter().any(|t| (t - k.time).abs() < 1e-4))).collect();
                let shifted: Vec<f64> = list.iter().zip(&picked).filter(|(_, p)| **p).map(|(k, _)| k.time + by).collect();
                // A moved keyframe replaces one that stays where it lands.
                let mut keep = vec![];
                for (k, p) in list.drain(..).zip(picked) {
                    if p {
                        moved += 1;
                        keep.push(kimchi_core::Keyframe { time: (k.time + by).max(0.0), ..k });
                    } else if !shifted.iter().any(|t| (t - k.time).abs() < 1e-6) {
                        keep.push(k);
                    }
                }
                *list = keep;
            }
            if moved == 0 {
                return Err(format!("No keyframes of \"{id}\" matched."));
            }
            kimchi_core::anim::normalize(keys);
            scene.validate()?;
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            Ok(json!({ "moved": moved, "clip": summary }))
        }
        "motion.view" => {
            let p = s.project()?;
            let (clip, scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let t = clip.scene_time(a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead));
            let w = a.opt_u32("width").unwrap_or(960).clamp(64, 3840);
            let h = ((w as f64 * p.settings.height as f64 / p.settings.width.max(1) as f64).round() as u32).max(2);
            use kimchi_media::render::space::viewport::{Shading, ViewCamera, ViewOptions};
            let mut view: Option<ViewCamera> = match a.object("view") {
                Some(v) => Some(serde_json::from_value(json!({"position": [6, 4.5, 8], "target": [0, 0.5, 0], "fov": 40, "ortho": false, "orthoSize": 6}))
                    .and_then(|base: Value| {
                        let mut m = base.as_object().cloned().unwrap_or_default();
                        m.extend(v.clone());
                        serde_json::from_value(Value::Object(m))
                    })
                    .map_err(|e| format!("view: {e}"))?),
                None => None,
            };
            if let Some(axis) = a.opt_str("axis") {
                if !["front", "back", "left", "right", "top", "bottom"].contains(&axis) {
                    return Err(format!("axis is front, back, left, right, top or bottom, not \"{axis}\""));
                }
                let mut v = view.unwrap_or_default();
                v.align(axis);
                view = Some(v);
            }
            let shading = match a.opt_str("shading").unwrap_or("material") {
                "solid" => Shading::Solid,
                "material" => Shading::Material,
                "rendered" => Shading::Rendered,
                other => return Err(format!("shading is solid, material or rendered, not \"{other}\"")),
            };
            let through = a.opt_bool("throughCamera").unwrap_or(view.is_none());
            let opts = ViewOptions {
                through_camera: through,
                shading,
                grid: a.opt_bool("grid").unwrap_or(!through),
                selected: a.array("selected").map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                helpers: !through,
                ..Default::default()
            };
            let comp = a.opt_str("composition").map(str::to_string);
            if let (Some(c), Scene::Flat(f)) = (&comp, &scene)
                && f.composition(c).is_none()
            {
                return Err(format!("No composition \"{c}\" in \"{}\".", clip.name));
            }
            let tools = s.tools()?;
            let out_dir = s.cache_dir(p.id).join("renders");
            let id = clip.id;
            let path = tokio::task::spawn_blocking(move || -> CmdResult<std::path::PathBuf> {
                let mut r = kimchi_media::render::Renderer::new(&tools, &p, w, h, p.settings.fps);
                let img = r.scene_view(id, t, view.as_ref(), &opts, comp.as_deref()).map_err(err)?;
                std::fs::create_dir_all(&out_dir).map_err(err)?;
                let path = out_dir.join(format!("view-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f")));
                img.save_png(&path).map_err(err)?;
                Ok(path)
            })
            .await
            .map_err(err)??;
            Ok(json!({ "path": path, "time": t }))
        }
        "motion.render" => {
            let p = s.project()?;
            let ids = clip_ids(&p, &a)?;
            let mut started = vec![];
            for id in ids {
                started.push(crate::renders::start(s, id, cx.source)?);
            }
            if a.bool_or("wait", false) || s.headless {
                let mut done = vec![];
                for r in &started {
                    done.push(crate::renders::wait(s, r).await?);
                }
                return Ok(json!({ "renders": done }));
            }
            Ok(json!({ "renderIds": started, "next": "Follow them with motion.renderStatus." }))
        }
        "motion.renderStatus" => {
            if let Some(id) = a.opt_str("renderId") {
                return s.renders().into_iter().find(|r| r.id == id).map(|r| json!(r)).ok_or_else(|| format!("No render `{id}`."));
            }
            let p = s.project()?;
            let clips: Vec<Value> = p
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| matches!(c.content, ClipContent::Motion { .. }))
                .map(|c| json!({ "clipId": c.id, "name": c.name, "state": kimchi_media::render::cache::status(&p, c), "engine": c.rendered.as_ref().map(|r| r.engine.clone()) }))
                .collect();
            Ok(json!({ "renders": s.renders(), "clips": clips }))
        }
        "motion.cancelRender" => {
            let id = a.str("renderId")?;
            if !s.cancel_render(id) {
                return Err(format!("No running render `{id}`."));
            }
            Ok(json!({ "cancelled": id }))
        }
        "motion.unrender" => {
            let p = s.project()?;
            let ids = clip_ids(&p, &a)?;
            s.edit(cx.label(), cx.source, |ed| {
                ed.begin_batch(cx.label(), cx.source.as_str());
                for id in &ids {
                    let patch = ClipPatch { rendered: Some(None), ..Default::default() };
                    if let Err(e) = ed.apply(&Edit::UpdateClip { clip_id: *id, patch }, None) {
                        ed.rollback_batch();
                        return Err(err(e));
                    }
                }
                ed.end_batch();
                Ok(())
            })?;
            Ok(json!({ "live": ids }))
        }
        "motion.convertToMesh" | "motion.applyModifier" | "motion.editMesh" => super::motion_mesh::run(s, cx, a).await,
        _ => Err(crate::commands::unhandled(cx)),
    }
}

fn type_json(t: &stack::TypeSpec) -> Value {
    json!({
        "type": t.name,
        "label": t.label,
        "doc": t.doc,
        "params": t.params.iter().map(|p| {
            let (kind, extra) = match p.kind {
                stack::ParamKind::Number { min, max, step } => ("number", json!({ "min": min, "max": max, "step": step })),
                stack::ParamKind::Int { min, max } => ("integer", json!({ "min": min, "max": max })),
                stack::ParamKind::Bool => ("boolean", json!({})),
                stack::ParamKind::Color => ("color", json!({})),
                stack::ParamKind::Choice(c) => ("choice", json!({ "options": c })),
                stack::ParamKind::Text => ("text", json!({})),
                stack::ParamKind::Vec2 => ("vec2", json!({})),
                stack::ParamKind::Vec3 => ("vec3", json!({})),
                stack::ParamKind::Ref => ("id", json!({})),
                stack::ParamKind::Path => ("path", json!({})),
            };
            let mut o = json!({ "name": p.name, "label": p.label, "kind": kind, "default": p.default.value(), "doc": p.doc, "animatable": p.kind.animatable() });
            if let (Some(o), Some(e)) = (o.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            o
        }).collect::<Vec<_>>(),
    })
}

fn field(f: &str) -> CmdResult<&'static str> {
    const FIELDS: &[&str] = &["modifiers", "constraints", "effects", "operators", "masks", "animators"];
    FIELDS.iter().find(|x| **x == f).copied().ok_or_else(|| {
        let hint = kimchi_core::closest(f, FIELDS).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        format!("field is one of {}, not `{f}`.{hint}", FIELDS.join(", "))
    })
}

/// Changes one thing of a scene through its JSON (and checks the result).
fn edit_item(scene: &mut Scene, id: &str, f: impl FnOnce(&mut Map<String, Value>) -> CmdResult<()>) -> CmdResult<()> {
    let camera = matches!(scene, Scene::Space(sp) if sp.camera_by_id(id).is_some());
    let mut json = scene.item_json(id).ok_or_else(|| format!("no \"{id}\"; ids: {}", scene.ids().join(", ")))?;
    let o = json.as_object_mut().ok_or("bad item")?;
    f(o)?;
    if camera {
        o.insert("type".into(), Value::from("camera"));
        if id == "camera" {
            o.insert("id".into(), Value::from(""));
        }
    }
    scene.upsert(&json, None)?;
    scene.normalize();
    scene.validate()
}

/// Ids of one stack of a thing, in order.
fn stack_ids(scene: &Scene, id: &str, field: &str) -> Vec<String> {
    scene.item_json(id).and_then(|j| j.get(field).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.get("id").and_then(Value::as_str).map(str::to_string)).collect())).unwrap_or_default()
}

fn clip_ids(p: &kimchi_core::Project, a: &Args) -> CmdResult<Vec<Id>> {
    let list = a.array("clipIds").ok_or("clipIds is a list of clip ids or names.")?;
    let mut out = vec![];
    for v in list {
        let key = v.as_str().ok_or("clipIds holds clip ids or names (text).")?;
        let id = resolve::clip(p, key)?;
        let c = p.clip(id).ok_or("clip not found")?;
        if !matches!(c.content, ClipContent::Motion { .. }) {
            return Err(format!("\"{}\" isn't a motion clip.", c.name));
        }
        out.push(id);
    }
    if out.is_empty() {
        return Err("Give at least one motion clip.".into());
    }
    Ok(out)
}

/// The list holding layer `id` (the scene's, a group's or a composition's), mutably.
fn sibling_list_mut<'a>(f: &'a mut motion::Scene2d, id: &str) -> Option<&'a mut Vec<Layer>> {
    fn look<'a>(list: &'a mut Vec<Layer>, id: &str) -> Option<&'a mut Vec<Layer>> {
        if list.iter().any(|l| l.id == id) {
            return Some(list);
        }
        for l in list.iter_mut() {
            if let LayerKind::Group { layers } = &mut l.kind
                && let Some(found) = look(layers, id)
            {
                return Some(found);
            }
        }
        None
    }
    if motion::find_layer(&f.layers, id).is_some() {
        return look(&mut f.layers, id);
    }
    f.compositions.iter_mut().find_map(|c| look(&mut c.layers, id))
}

/// The group, composition (2D) or object (3D) that holds `id`; None at the top level.
fn parent_of(scene: &Scene, id: &str) -> Option<String> {
    match scene {
        Scene::Flat(f) => {
            fn look(list: &[Layer], id: &str, parent: Option<&str>) -> Option<Option<String>> {
                for l in list {
                    if l.id == id {
                        return Some(parent.map(str::to_string));
                    }
                    if let LayerKind::Group { layers } = &l.kind
                        && let Some(p) = look(layers, id, Some(&l.id))
                    {
                        return Some(p);
                    }
                }
                None
            }
            look(&f.layers, id, None).or_else(|| f.compositions.iter().find_map(|c| look(&c.layers, id, Some(&c.id)))).flatten()
        }
        Scene::Space(sp) => {
            fn look(list: &[motion::Object3d], id: &str, parent: Option<&str>) -> Option<Option<String>> {
                for o in list {
                    if o.id == id {
                        return Some(parent.map(str::to_string));
                    }
                    if let Some(p) = look(&o.children, id, Some(&o.id)) {
                        return Some(p);
                    }
                }
                None
            }
            look(&sp.objects, id, None).flatten()
        }
    }
}

/// Where `id` is in its list.
fn index_of(scene: &Scene, id: &str) -> Option<usize> {
    match scene {
        Scene::Flat(f) => f.siblings(id)?.iter().position(|l| l.id == id),
        Scene::Space(sp) => {
            let parent = parent_of(scene, id);
            let list = match &parent {
                Some(p) => &motion::find_object(&sp.objects, p)?.children,
                None => &sp.objects,
            };
            list.iter().position(|o| o.id == id)
        }
    }
}

/// Takes `id` out of its list and puts it in `parent`'s (`Some("")` = the top level; None =
/// the same list) at `index` (default: last).
fn move_item(scene: &mut Scene, id: &str, parent: Option<&str>, index: Option<usize>) -> CmdResult<()> {
    let target = match parent {
        Some("") => None,
        Some(p) => {
            if p == id {
                return Err("can't go inside itself".into());
            }
            Some(p.to_string())
        }
        None => parent_of(scene, id),
    };
    match scene {
        Scene::Flat(f) => {
            let layer = f.find_layer(id).cloned().ok_or("no such layer")?;
            if let (Some(t), LayerKind::Group { layers }) = (&target, &layer.kind)
                && motion::find_layer(layers, t).is_some()
            {
                return Err("can't go inside its own group".into());
            }
            let list = sibling_list_mut(f, id).ok_or("no such layer")?;
            list.retain(|l| l.id != id);
            let dest: &mut Vec<Layer> = match &target {
                None => &mut f.layers,
                Some(t) => {
                    if f.compositions.iter().any(|c| &c.id == t) {
                        &mut f.compositions.iter_mut().find(|c| &c.id == t).expect("checked").layers
                    } else {
                        match f.find_layer_mut(t) {
                            Some(Layer { kind: LayerKind::Group { layers }, .. }) => layers,
                            Some(_) => return Err(format!("\"{t}\" isn't a group or a composition")),
                            None => return Err(format!("no group or composition \"{t}\"")),
                        }
                    }
                }
            };
            let at = index.unwrap_or(dest.len()).min(dest.len());
            dest.insert(at, layer);
        }
        Scene::Space(sp) => {
            let obj = motion::find_object(&sp.objects, id).cloned().ok_or("no such object (lights and cameras aren't in the object tree)")?;
            if let Some(t) = &target
                && motion::find_object(&obj.children, t).is_some()
            {
                return Err("can't go inside one of its own children".into());
            }
            fn take(list: &mut Vec<motion::Object3d>, id: &str) -> bool {
                let before = list.len();
                list.retain(|o| o.id != id);
                list.len() != before || list.iter_mut().any(|o| take(&mut o.children, id))
            }
            take(&mut sp.objects, id);
            let dest = match &target {
                None => &mut sp.objects,
                Some(t) => &mut motion::find_object_mut(&mut sp.objects, t).ok_or_else(|| format!("no object \"{t}\""))?.children,
            };
            let at = index.unwrap_or(dest.len()).min(dest.len());
            dest.insert(at, obj);
        }
    }
    Ok(())
}

/// Gives every child in `json[key]` (recursively) a fresh id.
fn rename_children(json: &mut Value, key: &str, taken: &mut Vec<String>, fresh: &dyn Fn(&str, &[String]) -> String) {
    let Some(list) = json.get_mut(key).and_then(Value::as_array_mut) else { return };
    for child in list {
        if let Some(old) = child.get("id").and_then(Value::as_str).map(str::to_string) {
            let new = fresh(&old, taken);
            taken.push(new.clone());
            child["id"] = Value::from(new);
        }
        rename_children(child, key, taken, fresh);
    }
}
