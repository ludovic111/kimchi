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
            let mut families: Vec<(&str, &str, &[stack::TypeSpec])> = stack::families().to_vec();
            families.push(("editOps", "mesh operation", kimchi_core::mesh::ops::EDIT_OPS));
            for (field, noun, types) in families {
                if want.is_some_and(|w| w != field) {
                    continue;
                }
                out.insert(field.into(), json!({ "noun": noun, "types": types.iter().map(type_json).collect::<Vec<_>>() }));
            }
            if out.is_empty() {
                let mut names: Vec<&str> = stack::families().iter().map(|f| f.0).collect();
                names.push("editOps");
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
        "motion.moveLayers" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let ids = a.array("ids").ok_or("ids must be a list of layer or object ids")?.iter()
                .map(|v| v.as_str().map(str::to_string).ok_or("Every id must be a string."))
                .collect::<Result<Vec<_>, _>>()?;
            let index=a.get("index").map(|v| v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or("index must be a nonnegative whole number.")).transpose()?;
            let before = scene.to_json();
            let moved = move_items(&mut scene, &ids, a.opt_str("parent"), index)?;
            scene.validate()?;
            if scene.to_json() == before { return Ok(json!({"moved":moved,"changed":false})); }
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            Ok(json!({"moved":moved,"changed":true,"clip":summary}))
        }
        "motion.arrangeObjects" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let Scene::Space(sp) = &scene else { return Err("Arrange objects requires a 3D scene.".into()) };
            let ids = a.array("ids").ok_or("ids must be a list of object ids")?.iter()
                .map(|v| v.as_str().map(str::to_string).ok_or("Every id must be a string."))
                .collect::<Result<Vec<_>, _>>()?;
            let axis = match a.str("axis")? { "x" => 0, "y" => 1, "z" => 2, _ => return Err("axis must be x, y or z".into()) };
            let arrangement = match a.str("operation")? {
                "alignActive" => motion::ObjectArrangement::AlignActive,
                "alignCentre" => motion::ObjectArrangement::AlignCentre,
                "distribute" => motion::ObjectArrangement::Distribute,
                _ => return Err("operation must be alignActive, alignCentre or distribute".into()),
            };
            let t = clip.scene_time(a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead));
            let opts = motion::EvalOptions { fps: p.settings.fps, duration: Some(clip.duration * clip.speed) };
            let edits = sp.arranged_origins(&ids, axis, arrangement, t, &opts)?;
            let mut moved = vec![];
            for edit in &edits {
                let Some(position) = edit.position else { continue };
                let keys = scene.item_mut(&edit.id).ok_or("Object disappeared.")?.keyframes().clone();
                let mut props = Map::new();
                if keys.contains_key("position") {
                    props.insert("position".into(), json!(position));
                }
                // Component channels override a whole-vector channel when both are present.
                for (axis, value) in ["x", "y", "z"].into_iter().zip(position) {
                    let name = format!("position.{axis}");
                    if !keys.contains_key("position") || keys.contains_key(&name) { props.insert(name, json!(value)); }
                    if keys.contains_key(axis) { props.insert(axis.into(), json!(value)); }
                }
                super::motion::update_item(&mut scene, &edit.id, &props, t)?;
                moved.push(edit.id.clone());
            }
            let Scene::Space(sp) = &scene else { unreachable!() };
            sp.verify_arranged_origins(&edits, t, &opts)?;
            if moved.is_empty() { return Ok(json!({ "moved": [], "changed": false })); }
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            Ok(json!({ "moved": moved, "changed": true, "clip": summary }))
        }
        "motion.renameLayer" => {
            let p = s.project()?;
            let (clip, scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (from, to) = (a.str("id")?, a.str("newId")?.trim());
            let renamed = rename(&scene, from, to, a.opt_str("namespace")).map_err(|e| format!("\"{from}\" in \"{}\": {e}", clip.name))?;
            let summary = set_scene(s, cx, &a, clip.id, renamed, None)?;
            Ok(json!({ "id": to, "clip": summary }))
        }
        "motion.duplicateLayer" | "motion.duplicateLayers" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let single = cx.spec.name == "motion.duplicateLayer";
            let ids = if single { vec![a.str("id")?.to_string()] } else { a.strings("ids") };
            let (copies, id_map) = duplicate_items(&mut scene, &ids, if single { a.opt_str("newId") } else { None })?;
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            if single { Ok(json!({ "id": copies[0], "clip": summary })) }
            else { Ok(json!({ "ids": copies, "idMap": id_map, "clip": summary })) }
        }
        "motion.updateKeyframes" | "motion.duplicateKeyframes" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let copy = cx.spec.name == "motion.duplicateKeyframes";
            let field = if copy { "keys" } else { "updates" };
            let by = if copy { a.f64("by")? } else { 0. };
            if copy && (!by.is_finite() || by == 0.) { return Err("by must be finite, nonzero timeline seconds".into()); }
            let updates = a.array(field).filter(|v| !v.is_empty()).ok_or_else(|| format!("{field} must be a nonempty list of keyframes"))?;
            let mut plan: Vec<(String, String, f64, kimchi_core::Keyframe)> = vec![];
            for (i, update) in updates.iter().enumerate() {
                let update = update.as_object().ok_or_else(|| format!("{field}[{i}] must be an object"))?;
                let allowed:&[&str] = if copy { &["id","property","time"] } else { &["id","property","time","newTime","value","easing"] };
                if let Some(name) = update.keys().find(|name| !allowed.contains(&name.as_str())) {
                    return Err(format!("{field}[{i}] has no field `{name}`; use {}",allowed.join(", ")));
                }
                let string = |name: &str| update.get(name).and_then(Value::as_str).ok_or_else(|| format!("{field}[{i}].{name} must be a string"));
                let time = |name: &str| update.get(name).and_then(Value::as_f64).filter(|t| t.is_finite()).map(|t| clip.scene_time(t)).filter(|t| t.is_finite())
                    .ok_or_else(|| format!("{field}[{i}].{name} must be finite timeline seconds"));
                let (id, property, at) = (string("id")?, string("property")?, time("time")?);
                let mut key = scene.keyframes_mut(id).and_then(|keys| keys.get(property)).and_then(|keys| keys.iter().find(|k| (k.time - at).abs() < 1e-6)).cloned()
                    .ok_or_else(|| format!("{field}[{i}]: no {id}.{property} keyframe at the given time"))?;
                let original = key.time;
                if copy {
                    key.time = clip.scene_time(update["time"].as_f64().expect("validated time")+by);
                    if !key.time.is_finite() || (key.time-original).abs() < 1e-6 { return Err(format!("keys[{i}]: the offset must place the copy at a different, finite time")); }
                    if !a.bool_or("replace",false) && scene.keyframes_mut(id).and_then(|k| k.get(property)).is_some_and(|keys| keys.iter().any(|k| (k.time-key.time).abs() < 1e-6)) {
                        return Err(format!("keys[{i}]: {id}.{property} already has a keyframe at the destination; choose another offset or use replace"));
                    }
                }
                if update.contains_key("newTime") { key.time = time("newTime")?; }
                if let Some(value) = update.get("value") {
                    key.value = serde_json::from_value(value.clone()).map_err(|_| format!("updates[{i}].value must be a number, vector or string"))?;
                }
                if update.contains_key("easing") { key.easing = kimchi_core::Easing::parse(string("easing")?)?; }
                for (other_id, other_property, source, target) in &plan {
                    if other_id == id && other_property == property {
                        if (source - original).abs() < 1e-6 { return Err(format!("{field}[{i}] selects the same source key twice")); }
                        if (target.time - key.time).abs() < 1e-6 { return Err(format!("{field}[{i}] would put two selected keys at the same time")); }
                    }
                }
                plan.push((id.to_string(), property.to_string(), original, key));
            }
            // Remove all sources first, before inserting targets that may occupy those times.
            if !copy {
                for (id, property, at, _) in &plan {
                    scene.keyframes_mut(id).expect("validated item").get_mut(property).expect("validated channel").retain(|k| (k.time - at).abs() >= 1e-6);
                }
            }
            for (id, property, _, key) in &plan {
                kimchi_core::anim::set_key(scene.keyframes_mut(id).expect("validated item"), property, key.clone());
            }
            scene.validate()?;
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            if copy {
                let selected:Vec<_> = plan.iter().map(|(id,property,_,key)| json!({"id":id,"property":property,"time":key.time})).collect();
                Ok(json!({"copied":plan.len(),"selectedKeys":selected,"clip":summary}))
            } else { Ok(json!({"updated": plan.len(), "clip": summary})) }
        }
        "motion.shiftKeyframes" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let id = a.str("id")?;
            let by = a.f64("by")? * clip.speed * if clip.reverse { -1. } else { 1. };
            if !by.is_finite() { return Err("by is too large after applying the clip speed".into()); }
            let only = a.opt_str("property");
            let times: Option<Vec<f64>> = a.array("times").map(|v| v.iter().enumerate().map(|(i,v)| v.as_f64().filter(|t| t.is_finite())
                .map(|t| clip.scene_time(t)).filter(|t| t.is_finite()).ok_or_else(|| format!("times[{i}] must be finite timeline seconds")))
                .collect::<CmdResult<Vec<_>>>()).transpose()?;
            let keys = scene.keyframes_mut(id).ok_or_else(|| format!("No \"{id}\" in \"{}\".", clip.name))?;
            let matches_time = |time:f64| times.as_ref().is_none_or(|ts| ts.iter().any(|t| (t-time).abs() < 1e-6));
            let first = keys.iter().filter(|(name,_)| only.is_none_or(|p| p == name.as_str())).flat_map(|(_,list)| list)
                .filter(|k| matches_time(k.time)).map(|k| k.time).min_by(f64::total_cmp)
                .ok_or_else(|| format!("No keyframes of \"{id}\" matched."))?;
            let by = by.max(-first);
            if keys.iter().filter(|(name,_)| only.is_none_or(|p| p == name.as_str())).flat_map(|(_,list)| list)
                .any(|k| matches_time(k.time) && !(k.time+by).is_finite()) { return Err("The shifted keyframe times would be too large.".into()); }
            let mut moved = 0;
            for (name, list) in keys.iter_mut() {
                if only.is_some_and(|o| o != name) {
                    continue;
                }
                let picked: Vec<bool> = list.iter().map(|k| matches_time(k.time)).collect();
                let shifted: Vec<f64> = list.iter().zip(&picked).filter(|(_, p)| **p).map(|(k, _)| k.time + by).collect();
                // A moved keyframe replaces one that stays where it lands.
                let mut keep = vec![];
                for (k, p) in list.drain(..).zip(picked) {
                    if p {
                        moved += 1;
                        keep.push(kimchi_core::Keyframe { time: k.time + by, ..k });
                    } else if !shifted.iter().any(|t| (t - k.time).abs() < 1e-6) {
                        keep.push(k);
                    }
                }
                *list = keep;
            }
            kimchi_core::anim::normalize(keys);
            scene.validate()?;
            let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
            Ok(json!({ "moved": moved, "by": by / clip.speed * if clip.reverse { -1. } else { 1. }, "clip": summary }))
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
        "motion.convertToMesh" | "motion.applyModifier" | "motion.editMesh" | "motion.updateMeshVertices" => Box::pin(super::motion_mesh::run(s, cx, a)).await,
        "motion.cameraMove" => Box::pin(super::motion_camera::run(s, cx, a)).await,
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
    if parent.is_none() {
        // Reordering in place must not resolve a same-named composition as the source group.
        match scene {
            Scene::Flat(f)=>{
                let list=sibling_list_mut(f,id).ok_or("no such layer")?;
                let from=list.iter().position(|l| l.id==id).expect("source list");
                let item=list.remove(from);
                list.insert(index.unwrap_or(list.len()).min(list.len()),item);
            }
            Scene::Space(_)=>{
                let source=parent_of(scene,id);
                let Scene::Space(sp)=scene else {unreachable!()};
                let list=match source {
                    Some(parent)=>&mut motion::find_object_mut(&mut sp.objects,&parent).ok_or("no such object parent")?.children,
                    None=>&mut sp.objects,
                };
                let from=list.iter().position(|o| o.id==id).ok_or("no such object (lights and cameras aren't in the object tree)")?;
                let item=list.remove(from);
                list.insert(index.unwrap_or(list.len()).min(list.len()),item);
            }
        }
        return Ok(());
    }
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

/// Gather the selection at the destination's end first, then insert the whole block. No
/// intermediate scene is published, so sibling indices and linked roots remain consistent.
fn move_items(scene: &mut Scene, ids: &[String], parent: Option<&str>, index: Option<usize>) -> CmdResult<Vec<String>> {
    if ids.is_empty() { return Err("Select at least one layer or object to move.".into()); }
    for id in ids {
        if index_of(scene,id).is_none() { return Err(format!("\"{id}\" is not a movable layer or object.")); }
    }
    let nested=|id:&str,root:&str| match &*scene {
        Scene::Flat(f)=>matches!(f.find_layer(root),Some(Layer {kind:LayerKind::Group {layers},..}) if motion::find_layer(layers,id).is_some()),
        Scene::Space(sp)=>motion::find_object(&sp.objects,root).is_some_and(|o| motion::find_object(&o.children,id).is_some()),
    };
    let roots: Vec<_> = scene.ids().into_iter().filter(|id| ids.contains(id) && !ids.iter().any(|root| nested(id,root))).collect();
    let target=match parent {
        Some(p)=>p.to_string(),
        None=>{
            let target=parent_of(scene,&roots[0]);
            let same_list=match &*scene {
                Scene::Flat(f)=>{
                    let list=f.siblings(&roots[0]).expect("movable root");
                    roots.iter().all(|id| list.iter().any(|l| l.id==*id))
                }
                Scene::Space(_)=>roots.iter().all(|id| parent_of(scene,id)==target),
            };
            if !same_list {
                return Err("Select siblings or give a destination parent.".into());
            }
            target.unwrap_or_default()
        }
    };
    // Composition ids live separately from layer ids (a precomp commonly shares its name
    // with the comp layer). Only structural children belong to a selected root.
    let composition=matches!(&*scene,Scene::Flat(f) if f.compositions.iter().any(|c| c.id==target));
    if parent.is_some() && !composition && roots.iter().any(|id| *id==target || nested(&target,id)) {
        return Err("A selection can't move inside itself or one of its descendants.".into());
    }
    let destination=parent.map(|_| target.as_str());
    for id in &roots {move_item(scene,id,destination,None)?;}
    if let Some(index)=index {
        let at=index.min(index_of(scene,&roots[0]).expect("moved roots are at the destination's end"));
        for (offset,id) in roots.iter().enumerate() {move_item(scene,id,destination,Some(at+offset))?;}
    }
    Ok(roots)
}

fn fresh_id(base: &str, taken: &[String]) -> String {
    (2..).map(|n| format!("{base}{n}")).find(|c| !taken.contains(c)).expect("a free id")
}

/// Gives every child in `json[key]` a fresh id and records it for reference remapping.
fn rename_children(json: &mut Value, key: &str, taken: &mut Vec<String>, id_map: &mut Map<String, Value>) {
    let Some(list) = json.get_mut(key).and_then(Value::as_array_mut) else { return };
    for child in list {
        if let Some(old) = child.get("id").and_then(Value::as_str).map(str::to_string) {
            let new = fresh_id(&old, taken);
            taken.push(new.clone());
            id_map.insert(old, Value::from(new.clone()));
            child["id"] = Value::from(new);
        }
        rename_children(child, key, taken, id_map);
    }
}

/// Build all copies before inserting any: links between roots can then be remapped together.
fn duplicate_items(scene: &mut Scene, ids: &[String], new_id: Option<&str>) -> CmdResult<(Vec<String>, Map<String, Value>)> {
    if ids.is_empty() { return Err("Select at least one thing to duplicate.".into()); }
    let mut roots = vec![];
    for id in ids {
        if matches!(id.as_str(), "camera" | "scene") { return Err(format!("\"{id}\" can't be copied; select an object, layer, light or extra camera.")); }
        if scene.item_json(id).is_none() { return Err(format!("No \"{id}\" in this scene.")); }
        let mut parent = parent_of(scene, id);
        let mut included = false;
        while let Some(p) = parent {
            if ids.contains(&p) { included = true; break; }
            parent = parent_of(scene, &p);
        }
        if !included && !roots.contains(id) { roots.push(id.clone()); }
    }
    let mut taken = scene.ids();
    taken.extend(["camera".to_string(), "scene".to_string()]);
    if let Scene::Flat(f) = scene { taken.extend(f.compositions.iter().map(|c| c.id.clone())); }
    if let Some(id) = new_id {
        if id.trim().is_empty() { return Err("The copy's id is empty.".into()); }
        if taken.iter().any(|t| t == id) { return Err(format!("\"{id}\" is already taken or reserved.")); }
    }
    let mut id_map = Map::new();
    let mut copies = vec![];
    for id in &roots {
        let new = new_id.map(str::to_string).unwrap_or_else(|| fresh_id(id, &taken));
        taken.push(new.clone());
        id_map.insert(id.clone(), Value::from(new.clone()));
        let mut copy = scene.item_json(id).expect("validated above");
        copy["id"] = Value::from(new.clone());
        copies.push((id.clone(), new, copy));
    }
    for (_, _, copy) in &mut copies {
        for key in ["layers", "children"] { rename_children(copy, key, &mut taken, &mut id_map); }
    }
    for (old, new, copy) in &mut copies {
        for (from, to) in &id_map { rename_in(copy, from, to.as_str().expect("new id"), true, false, false); }
        if scene.is_3d() && copy.get("type").is_none() { copy["type"] = Value::from("camera"); }
        let parent = parent_of(scene, old);
        scene.upsert(copy, parent.as_deref())?;
        // Lights and cameras have their own lists outside the object hierarchy.
        if let Scene::Space(sp) = scene {
            if let Some(at) = sp.lights.iter().position(|l| &l.id == old) {
                let copy = sp.lights.remove(sp.lights.iter().position(|l| &l.id == new).expect("inserted light"));
                sp.lights.insert(at + 1, copy);
                continue;
            }
            if let Some(at) = sp.cameras.iter().position(|c| &c.id == old) {
                let copy = sp.cameras.remove(sp.cameras.iter().position(|c| &c.id == new).expect("inserted camera"));
                sp.cameras.insert(at + 1, copy);
                continue;
            }
        }
        let index = index_of(scene, old).map(|i| i + 1);
        move_item(scene, new, Some(parent.as_deref().unwrap_or("")), index)?;
    }
    scene.normalize();
    scene.validate()?;
    Ok((copies.into_iter().map(|(_, new, _)| new).collect(), id_map))
}

/// The scene with `from` (a thing, a composition or a shared material) called `to`, and every
/// reference to it changed too.
fn rename(scene: &Scene, from: &str, to: &str, namespace: Option<&str>) -> CmdResult<Scene> {
    if to.is_empty() {
        return Err("the new id is empty".into());
    }
    let mut taken: Vec<String> = scene.ids();
    let (mut comp, mut material) = match scene {
        Scene::Flat(f) => {
            taken.extend(f.compositions.iter().map(|c| c.id.clone()));
            (f.composition(from).is_some(), false)
        }
        Scene::Space(sp) => (false, sp.material(from).is_some()),
    };
    let mut thing = scene.ids().iter().any(|i| i == from);
    match namespace.unwrap_or(if thing || comp || matches!(from,"camera" | "scene") { "scene" } else { "material" }) {
        "scene" => {
            if matches!(to,"scene" | "camera") { return Err(format!("\"{to}\" is reserved; pick another id")); }
            if matches!(from,"scene" | "camera") { return Err(format!("\"{from}\" can't be renamed")); }
            material = false;
        }
        "material" => {
            thing = false;
            comp = false;
            taken = match scene { Scene::Space(sp) => sp.materials.iter().filter_map(|m| m.id.clone()).collect(), _ => vec![] };
        }
        other => return Err(format!("Unknown namespace \"{other}\"; use scene or material.")),
    }
    if !thing && !comp && !material {
        return Err(format!("nothing has the id \"{from}\"; ids: {}", scene.ids().join(", ")));
    }
    if from == to { return Ok(scene.clone()); }
    if taken.iter().any(|t| t == to) {
        return Err(format!("\"{to}\" is already taken"));
    }
    let mut json = scene.to_json();
    rename_in(&mut json, from, to, thing, comp, material);
    Scene::from_json(&json)
}

/// Walks a scene's JSON: ids, references and formulas naming `from` become `to`.
fn rename_in(v: &mut Value, from: &str, to: &str, thing: bool, comp: bool, material: bool) {
    let set = |slot: &mut Value| {
        if slot.as_str() == Some(from) {
            *slot = Value::from(to);
        }
    };
    match v {
        Value::Array(list) => {
            for x in list {
                rename_in(x, from, to, thing, comp, material);
            }
        }
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                match k.as_str() {
                    "id" if thing || comp => set(x),
                    "parent" | "mask" | "activeCamera" if thing => set(x),
                    "layer" if thing => set(x),
                    "comp" if comp => set(x),
                    // Materials have a separate id namespace and contain no scene-object refs.
                    "material" | "materials" if !material => continue,
                    "material" if material => set(x),
                    "expressions" if thing => {
                        if let Some(ex) = x.as_object_mut() {
                            for f in ex.values_mut() {
                                if let Some(src) = f.as_str() {
                                    *f = Value::from(rename_in_formula(src, from, to));
                                }
                            }
                        }
                    }
                    "keyframes" if thing => {
                        // The scene's activeCamera keyframes name cameras.
                        if let Some(list) = x.get_mut("activeCamera").and_then(Value::as_array_mut) {
                            for key in list.iter_mut() {
                                // `{"time", "value", …}` or `[time, value, easing]`.
                                let val = if key.is_array() { key.get_mut(1) } else { key.get_mut("value") };
                                if let Some(val) = val {
                                    set(val);
                                }
                            }
                        }
                    }
                    "modifiers" | "constraints" | "effects" | "operators" | "masks" | "animators" if thing => {
                        rename_refs(k, x, from, to);
                        // Stack items never hold things themselves, but their ids aren't the thing's.
                        continue;
                    }
                    _ => {}
                }
                if !matches!(k.as_str(), "expressions" | "keyframes") {
                    rename_in(x, from, to, thing, comp, material);
                }
            }
            // A material is shared by its id in the scene's list.
            if material
                && let Some(Value::Array(list)) = o.get_mut("materials")
            {
                for m in list {
                    if let Some(id) = m.get_mut("id") {
                        set(id);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Reference parameters (`Ref` in the stack tables) naming `from` in one stack.
fn rename_refs(field: &str, list: &mut Value, from: &str, to: &str) {
    let Some((_, _, types)) = stack::families().into_iter().find(|(f, _, _)| *f == field) else { return };
    for item in list.as_array_mut().into_iter().flatten() {
        let Some(kind) = item.get("type").and_then(Value::as_str).map(str::to_string) else { continue };
        let Some(spec) = types.iter().find(|t| t.name == kind) else { continue };
        for p in spec.params.iter().filter(|p| p.kind == stack::ParamKind::Ref) {
            if let Some(slot) = item.get_mut(p.name)
                && slot.as_str() == Some(from)
            {
                *slot = Value::from(to);
            }
        }
    }
}

/// `prop("from", …)` / `prop('from', …)` in a formula become `prop("to", …)`.
fn rename_in_formula(src: &str, from: &str, to: &str) -> String {
    kimchi_core::expr::rename_prop_reference(src, from, to).unwrap_or_else(|_| src.to_string())
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn renaming_follows_every_reference() {
        let s = Scene::from_json(&json!({"layers": [
            {"id": "ball", "type": "ellipse"},
            {"id": "shadow", "type": "ellipse", "parent": "ball", "matte": {"layer": "ball"}, "expressions": {"x": "prop('ball', 'x') + prop(\"ball\", \"y\")"}}
        ]}))
        .unwrap();
        let r = rename(&s, "ball", "sun", None).unwrap();
        let j = r.to_json();
        assert_eq!(j["layers"][0]["id"], "sun");
        assert_eq!(j["layers"][1]["parent"], "sun");
        assert_eq!(j["layers"][1]["matte"]["layer"], "sun");
        assert_eq!(j["layers"][1]["expressions"]["x"], "prop('sun', 'x') + prop(\"sun\", \"y\")");
        assert!(rename(&s, "ball", "shadow", None).unwrap_err().contains("taken"));

        let s = Scene::from_json(&json!({"cameras": [{"id": "side"}], "activeCamera": "side", "keyframes": {"activeCamera": [[0, "camera"], [1, "side"]]},
            "materials": [{"id": "gold"}],
            "objects": [{"id": "cutter", "type": "box"}, {"id": "b", "type": "box", "material": "gold",
                "modifiers": [{"type": "boolean", "object": "cutter"}], "constraints": [{"type": "lookAt", "target": "cutter"}]}]}))
        .unwrap();
        let j = rename(&s, "cutter", "knife", None).unwrap().to_json();
        assert_eq!(j["objects"][1]["modifiers"][0]["object"], "knife");
        assert_eq!(j["objects"][1]["constraints"][0]["target"], "knife");
        let j = rename(&s, "side", "wide", None).unwrap().to_json();
        assert_eq!(j["activeCamera"], "wide");
        assert_eq!(rename(&s, "side", "wide", None).unwrap(), {
            let mut j = s.to_json();
            j["activeCamera"] = json!("wide");
            j["cameras"][0]["id"] = json!("wide");
            j["keyframes"]["activeCamera"][1]["value"] = json!("wide");
            Scene::from_json(&j).unwrap()
        });
        let j = rename(&s, "gold", "brass", None).unwrap().to_json();
        assert_eq!((j["materials"][0]["id"].as_str(), j["objects"][1]["material"].as_str()), (Some("brass"), Some("brass")));
    }

    #[test]
    fn object_renaming_and_duplication_preserve_the_material_namespace() {
        let mut scene = Scene::from_json(&json!({"objects":[
            {"id":"gold","type":"box","material":"gold","children":[
                {"id":"child","type":"sphere","material":{"id":"gold","color":"#ffd000"}}
            ]}
        ],"materials":[{"id":"gold","color":"#ffcc00"}]})).unwrap();
        let renamed = rename(&scene, "gold", "box", None).unwrap().to_json();
        assert_eq!(renamed["materials"][0]["id"], "gold");
        assert_eq!(renamed["objects"][0]["material"], "gold");
        assert_eq!(renamed["objects"][0]["children"][0]["material"]["id"], "gold");
        let material = rename(&scene, "gold", "brass", Some("material")).unwrap().to_json();
        assert_eq!(material["objects"][0]["id"], "gold");
        assert_eq!(material["objects"][0]["material"], "brass");
        assert_eq!(material["objects"][0]["children"][0]["material"]["id"], "gold");
        assert_eq!(material["materials"][0]["id"], "brass");
        assert!(rename(&scene, "gold", "box", Some("typo")).is_err());
        assert!(rename(&scene, "child", "box", Some("material")).is_err());
        let camera_material = rename(&scene,"gold","camera",Some("material")).unwrap();
        assert!(rename(&camera_material,"camera","lens",None).is_err(),"the primary camera stays reserved");
        let camera_material = rename(&camera_material,"camera","lens",Some("material")).unwrap().to_json();
        assert_eq!(camera_material["materials"][0]["id"],"lens");
        assert_eq!(camera_material["objects"][0]["material"],"lens");
        duplicate_items(&mut scene, &["gold".into()], None).unwrap();
        let copied = scene.item_json("gold2").unwrap();
        assert_eq!(copied["material"], "gold");
        assert_eq!(copied["children"][0]["material"]["id"], "gold");
    }
}
