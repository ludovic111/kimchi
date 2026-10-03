//! Mesh modelling commands: `motion.convertToMesh` (any shape → an editable mesh),
//! `motion.applyModifier` (bake modifiers into the mesh, like Blender's Apply) and
//! `motion.editMesh` (edit-mode operations on a selection).

use std::path::PathBuf;
use std::sync::Arc;

use kimchi_core::mesh::ops::{self, EditOp, Select, Selection};
use kimchi_core::mesh::{PolyMesh, apply_modifiers, shape_mesh};
use kimchi_core::motion::{self, Object3d, Scene, Scene3d, Shape3d};
use kimchi_core::{Id, Project};
use kimchi_media::render::space::{editable_poly, viewport};
use serde_json::{Value, json};

use super::motion::{motion_clip, set_scene};
use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    let p = s.project()?;
    let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
    let id = a.str("id")?.to_string();
    let t = clip.scene_time(a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead).clamp(clip.start, clip.end()));
    let Scene::Space(sp) = &mut scene else {
        return Err(format!("\"{}\" is a 2D scene; meshes are 3D objects (for 2D outlines use path layers).", clip.name));
    };
    let obj = motion::find_object(&sp.objects, &id).cloned().ok_or_else(|| format!("No object \"{id}\" in \"{}\".", clip.name))?;
    let answer = match cx.spec.name {
        "motion.convertToMesh" => {
            let mut poly = base(&p, &obj)?;
            let bake = a.bool_or("applyModifiers", false);
            if bake {
                poly = apply_modifiers(poly, &obj.modifiers, t, &others(sp, t, &id));
            }
            let faces = poly.faces.len();
            let o = motion::find_object_mut(&mut sp.objects, &id).expect("found above");
            o.shape = poly.to_shape();
            if bake {
                o.modifiers.clear();
                drop_keys(o, "modifiers.");
            }
            json!({ "id": id, "vertices": vertex_count(o), "faces": faces })
        }
        "motion.applyModifier" => {
            let upto = match a.opt_str("modifierId") {
                Some(m) => Some(obj.modifiers.iter().position(|x| x.id == m).ok_or_else(|| {
                    let ids: Vec<&str> = obj.modifiers.iter().map(|x| x.id.as_str()).collect();
                    format!("\"{id}\" has no modifier \"{m}\"; modifiers: {}", if ids.is_empty() { "none".to_string() } else { ids.join(", ") })
                })?),
                None if obj.modifiers.is_empty() => return Err(format!("\"{id}\" has no modifiers to apply.")),
                None => Some(obj.modifiers.len() - 1),
            };
            let n = upto.map_or(0, |i| i + 1);
            let poly = apply_modifiers(base(&p, &obj)?, &obj.modifiers[..n], t, &others(sp, t, &id));
            let applied: Vec<String> = obj.modifiers[..n].iter().map(|m| m.id.clone()).collect();
            let o = motion::find_object_mut(&mut sp.objects, &id).expect("found above");
            o.shape = poly.to_shape();
            o.modifiers.drain(..n);
            for m in &applied {
                drop_keys(o, &format!("modifiers.{m}."));
            }
            json!({ "id": id, "applied": applied, "faces": poly.faces.len() })
        }
        "motion.editMesh" => {
            let Shape3d::Mesh { auto_smooth, .. } = &obj.shape else {
                return Err(format!("\"{id}\" is a {}; turn it into a mesh first with motion.convertToMesh.", obj.shape.name()));
            };
            let smooth = *auto_smooth;
            let mut m = shape_mesh(&obj.shape).ok_or("not a mesh")?;
            let sel = selection(&a, &m)?;
            let name = match a.str("op")? {
                "move" => "translate",
                other => other,
            };
            let params = a.object("params").cloned().unwrap_or_default();
            let op = EditOp::new(name, params);
            let new_sel = ops::apply(&mut m, &sel, &op)?;
            m.smooth_angle = smooth;
            let o = motion::find_object_mut(&mut sp.objects, &id).expect("found above");
            o.shape = m.to_shape();
            json!({ "id": id, "selection": new_sel, "vertices": vertex_count(o), "faces": m.faces.len() })
        }
        _ => return Err(crate::commands::unhandled(cx)),
    };
    scene.validate()?;
    let summary = set_scene(s, cx, &a, clip.id, scene, None)?;
    Ok(json!({ "result": answer, "clip": summary }))
}

/// The object's shape as polygons (before its modifiers).
fn base(p: &Project, o: &Object3d) -> CmdResult<PolyMesh> {
    let model = match &o.shape {
        Shape3d::Model { src } => Some(model_path(p, src).ok_or_else(|| format!("The model file \"{src}\" isn't there any more."))?),
        _ => None,
    };
    editable_poly(&o.shape, model.as_deref()).ok_or_else(|| match &o.shape {
        Shape3d::Group {} => format!("\"{}\" is an empty (a group); it has no shape to turn into a mesh.", o.id),
        Shape3d::Particles(_) => format!("\"{}\" is a particle system; it has no single shape.", o.id),
        Shape3d::Image { .. } => format!("\"{}\" is a picture card; use a plane with the picture as its texture instead.", o.id),
        Shape3d::Curve { .. } => format!("\"{}\" is a curve without a radius; give it a radius first.", o.id),
        other => format!("\"{}\": a {} can't be turned into a mesh.", o.id, other.name()),
    })
}

fn model_path(p: &Project, src: &str) -> Option<PathBuf> {
    let asset = src.parse::<Id>().ok().and_then(|id| p.asset(id)).or_else(|| p.assets.iter().find(|a| a.name.eq_ignore_ascii_case(src)));
    match asset {
        Some(a) => Some(PathBuf::from(&a.path)),
        None => Some(PathBuf::from(src)).filter(|p| p.is_file()),
    }
}

/// Other objects' meshes in `id`'s own space, for boolean modifiers.
fn others<'a>(sp: &'a Scene3d, t: f64, id: &'a str) -> impl Fn(&str) -> Option<PolyMesh> + 'a {
    move |other: &str| {
        let o = motion::find_object(&sp.objects, other)?;
        let mesh = editable_poly(&o.shape, None)?;
        let mesh = apply_modifiers(mesh, &o.modifiers, t, &|_| None);
        let mine = viewport::world_matrix(sp, t, id)?;
        let theirs = viewport::world_matrix(sp, t, other)?;
        let to_mine = mul(&invert(&mine)?, &theirs);
        let mut out = mesh;
        for v in out.positions.iter_mut() {
            *v = apply(&to_mine, *v);
        }
        Some(out)
    }
}

/// The selection the command names: indices, or a helper (`select`).
fn selection(a: &Args, m: &PolyMesh) -> CmdResult<Selection> {
    if let Some(v) = a.get("select") {
        let pick: Select = serde_json::from_value(v.clone()).map_err(|e| format!("select: {e} (fields: all, vertices, faces, facing, angle, inside, edgeLoop, edgeRing, linked)"))?;
        return pick.resolve(m);
    }
    let list = |name: &str, max: usize| -> CmdResult<Vec<u32>> {
        let Some(arr) = a.array(name) else { return Ok(vec![]) };
        arr.iter()
            .map(|v| {
                let i = v.as_u64().ok_or_else(|| format!("{name} holds whole numbers (indices)"))?;
                if i as usize >= max {
                    return Err(format!("{name}: {i} isn't in the mesh (0–{})", max.saturating_sub(1)));
                }
                Ok(i as u32)
            })
            .collect()
    };
    let sel = Selection { vertices: list("vertices", m.positions.len())?, faces: list("faces", m.faces.len())? };
    Ok(sel)
}

fn vertex_count(o: &Object3d) -> usize {
    match &o.shape {
        Shape3d::Mesh { vertices, .. } => vertices.len(),
        _ => 0,
    }
}

/// Removes keyframes and formulas whose names start with `prefix`.
fn drop_keys(o: &mut Object3d, prefix: &str) {
    o.keyframes.retain(|k, _| !k.starts_with(prefix));
    o.expressions.retain(|k, _| !k.starts_with(prefix));
}

// Column-major 4×4 (translation in m[3]), as the renderer gives them.
fn mul(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

fn apply(m: &[[f64; 4]; 4], p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

/// The inverse of an affine matrix (None when it squashes space flat).
fn invert(m: &[[f64; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    // The 3×3 part (rows r, columns c) and its inverse by cofactors.
    let a = |r: usize, c: usize| m[c][r];
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0)) + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = |r: usize, c: usize| -> f64 {
        let (r1, r2) = ((c + 1) % 3, (c + 2) % 3);
        let (c1, c2) = ((r + 1) % 3, (r + 2) % 3);
        (a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)) / det
    };
    let mut out = [[0.0; 4]; 4];
    for r in 0..3 {
        for c in 0..3 {
            out[c][r] = inv(r, c);
        }
    }
    let t = [m[3][0], m[3][1], m[3][2]];
    for r in 0..3 {
        out[3][r] = -(0..3).map(|c| out[c][r] * t[c]).sum::<f64>();
    }
    out[3][3] = 1.0;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_undoes_a_transform() {
        // Scale 2, then translate (1, 2, 3), column-major.
        let m = [[2.0, 0.0, 0.0, 0.0], [0.0, 0.0, 2.0, 0.0], [0.0, -2.0, 0.0, 0.0], [1.0, 2.0, 3.0, 1.0]];
        let i = invert(&m).unwrap();
        let p = [0.3, -0.7, 1.1];
        let q = apply(&i, apply(&m, p));
        assert!(p.iter().zip(q).all(|(a, b)| (a - b).abs() < 1e-9), "{q:?}");
        let _ = Value::Null;
    }
}
