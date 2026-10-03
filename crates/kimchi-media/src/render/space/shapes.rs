//! What each object is drawn with: its shape as polygons from kimchi-core
//! ([`kimchi_core::mesh::shape_mesh`]), through its modifier stack
//! ([`kimchi_core::mesh::apply_modifiers`]), triangulated; or, for shapes core doesn't model,
//! the renderer's own meshes (extruded text, and primitives until core has them), converted to
//! polygons when modifiers need them.
//!
//! Meshes are cached by what they are made from (the shape, the modifiers, the scene time when a
//! modifier moves on its own, and for booleans the other object and where it is), so a still
//! scene isn't re-meshed every frame.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::mesh::{PolyMesh, apply_modifiers, shape_mesh};
use kimchi_core::motion::{Modifier, Object3d, Shape3d, find_object};

use super::math::{M4, V3};
use super::mesh::{self, Mesh};

/// An object's evaluated geometry: the triangles drawn and, when it went through core, the
/// polygons they came from (edit mode draws those).
#[derive(Clone)]
pub(crate) struct Built {
    pub mesh: Arc<Mesh>,
    pub poly: Option<Arc<PolyMesh>>,
}

/// Where everything is at this instant, for modifiers that look at other objects.
pub(crate) struct Ctx<'a> {
    pub t: f64,
    /// The evaluated objects (children nested).
    pub objects: &'a [Object3d],
    /// World matrices by object id.
    pub world: &'a HashMap<String, M4>,
}

/// The modifiers that are on.
fn active(o: &Object3d) -> Vec<&Modifier> {
    o.modifiers.iter().filter(|m| m.enabled && m.spec().is_some()).collect()
}

/// Whether the modifiers change with time on their own (waves, boiling jitter).
fn moves(mods: &[&Modifier]) -> bool {
    mods.iter().any(|m| match m.kind.as_str() {
        "wave" => m.n("speed") != 0.0,
        "noise" => m.n("speed") > 0.0,
        _ => false,
    })
}

/// The shape's polygons: core's, or the renderer's mesh welded into polygons.
pub(crate) fn base_poly(shape: &Shape3d) -> Option<PolyMesh> {
    if let Some(p) = shape_mesh(shape) {
        return Some(p);
    }
    let m = mesh::shape(shape)?;
    Some(poly_of(&m, 30.0))
}

/// How a model file's first part looks (what the model shows once merged, with modifiers or as
/// an editable mesh): its colour (`#rrggbb`), metallic and roughness. Pictures it carries aren't
/// included.
pub fn model_look(path: &std::path::Path) -> Option<(String, f64, f64)> {
    let parts = mesh::model(path).ok()?;
    let p = parts.first()?;
    let c = p.color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Some((format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]), p.metallic as f64, p.roughness as f64))
}

/// A shape as polygons to edit (`motion.convertToMesh`): core's primitives, the renderer's text
/// welded into polygons, or a model file's parts (`model` gives its path) merged.
pub fn editable_poly(shape: &Shape3d, model: Option<&std::path::Path>) -> Option<PolyMesh> {
    if let Shape3d::Model { .. } = shape {
        let parts = mesh::model(model?).ok()?;
        return Some(poly_of(&merged(&parts), 30.0));
    }
    base_poly(shape)
}

/// Triangles welded into a polygon mesh (one face per triangle; corners at the same place share a
/// vertex), for modifiers and edit mode.
pub(crate) fn poly_of(m: &Mesh, smooth_angle: f64) -> PolyMesh {
    let mut index: HashMap<[i64; 3], u32> = HashMap::new();
    let mut positions = vec![];
    let mut faces = vec![];
    let mut uvs = vec![];
    let key = |p: &[f32; 3]| p.map(|v| (v as f64 * 1e5).round() as i64);
    for t in m.index.as_chunks::<3>().0.iter() {
        let mut face = Vec::with_capacity(3);
        let mut face_uv = Vec::with_capacity(3);
        for &i in t {
            let p = m.pos[i as usize];
            let id = *index.entry(key(&p)).or_insert_with(|| {
                positions.push(p.map(|v| v as f64));
                (positions.len() - 1) as u32
            });
            face.push(id);
            face_uv.push(m.uv[i as usize].map(|v| v as f64));
        }
        // Triangles squashed onto one point or line by the welding are dropped.
        if face[0] != face[1] && face[1] != face[2] && face[0] != face[2] {
            faces.push(face);
            uvs.push(face_uv);
        }
    }
    PolyMesh { positions, faces, uvs: Some(uvs), smooth_angle }
}

/// `p` moved by `m`.
pub(crate) fn transform_poly(p: &PolyMesh, m: &M4) -> PolyMesh {
    let mut out = p.clone();
    for v in out.positions.iter_mut() {
        let q = m.point3(V3(v[0] as f32, v[1] as f32, v[2] as f32));
        *v = [q.0 as f64, q.1 as f64, q.2 as f64];
    }
    out
}

/// An object's polygons after its modifiers (without booleans when `booleans` is false, so two
/// objects cutting each other don't loop).
fn evaluated_poly(o: &Object3d, ctx: &Ctx, booleans: bool) -> Option<PolyMesh> {
    let base = base_poly(&o.shape)?;
    let mods: Vec<Modifier> = active(o).into_iter().filter(|m| booleans || m.kind != "boolean").cloned().collect();
    if mods.is_empty() {
        return Some(base);
    }
    let me = ctx.world.get(&o.id).copied().unwrap_or(M4::I);
    let into_me = me.inverse().unwrap_or(M4::I);
    let other = |id: &str| -> Option<PolyMesh> {
        let other = find_object(ctx.objects, id)?;
        let p = evaluated_poly(other, ctx, false)?;
        let world = ctx.world.get(id).copied().unwrap_or(M4::I);
        Some(transform_poly(&p, &(into_me * world)))
    };
    Some(apply_modifiers(base, &mods, ctx.t, &other))
}

/// The cache key of an object's geometry.
fn key(o: &Object3d, ctx: &Ctx) -> u64 {
    let mods = active(o);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&o.shape).unwrap_or_default().hash(&mut h);
    for m in &mods {
        serde_json::to_string(m).unwrap_or_default().hash(&mut h);
        if m.kind == "boolean"
            && let Some(id) = m.opt_s("object")
            && let Some(other) = find_object(ctx.objects, &id)
        {
            serde_json::to_string(&other.shape).unwrap_or_default().hash(&mut h);
            serde_json::to_string(&other.modifiers).unwrap_or_default().hash(&mut h);
            let me = ctx.world.get(&o.id).copied().unwrap_or(M4::I);
            let rel = me.inverse().unwrap_or(M4::I) * ctx.world.get(&id).copied().unwrap_or(M4::I);
            for v in rel.flat() {
                v.to_bits().hash(&mut h);
            }
        }
    }
    if moves(&mods) {
        ctx.t.to_bits().hash(&mut h);
    }
    h.finish()
}

fn cache() -> &'static Mutex<HashMap<u64, Built>> {
    static C: OnceLock<Mutex<HashMap<u64, Built>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// What `o` is drawn with (None for shapes without a mesh of their own: groups, particles,
/// models and pictures are drawn elsewhere; curves without a radius).
pub(crate) fn build(o: &Object3d, ctx: &Ctx) -> Option<Built> {
    if matches!(o.shape, Shape3d::Group {} | Shape3d::Particles(_) | Shape3d::Model { .. } | Shape3d::Image { .. }) {
        return None;
    }
    let k = key(o, ctx);
    if let Some(b) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&k) {
        return Some(b.clone());
    }
    let built = if active(o).is_empty() {
        match shape_mesh(&o.shape) {
            Some(p) => Built { mesh: Arc::new(mesh::from_tris(&p.triangulate())), poly: Some(Arc::new(p)) },
            None => Built { mesh: mesh::shape(&o.shape)?, poly: None },
        }
    } else {
        let p = evaluated_poly(o, ctx, true)?;
        Built { mesh: Arc::new(mesh::from_tris(&p.triangulate())), poly: Some(Arc::new(p)) }
    };
    let mut map = cache().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 256 {
        map.clear();
    }
    map.insert(k, built.clone());
    Some(built)
}

/// The polygons edit mode shows for `o` (made from its triangles when core has no polygons for
/// its shape).
pub(crate) fn edit_poly(o: &Object3d, ctx: &Ctx) -> Option<Arc<PolyMesh>> {
    let b = build(o, ctx)?;
    Some(b.poly.unwrap_or_else(|| Arc::new(poly_of(&b.mesh, 30.0))))
}

/// A model's parts merged into one mesh (for modifiers on models).
pub(crate) fn merged(parts: &[mesh::Part]) -> Mesh {
    let mut out = Mesh::default();
    for p in parts {
        let base = out.pos.len() as u32;
        out.pos.extend_from_slice(&p.mesh.pos);
        out.normal.extend_from_slice(&p.mesh.normal);
        out.uv.extend_from_slice(&p.mesh.uv);
        out.index.extend(p.mesh.index.iter().map(|i| i + base));
    }
    out
}

/// A model (merged) through its object's modifiers.
pub(crate) fn model_with_modifiers(o: &Object3d, parts: &[mesh::Part], ctx: &Ctx) -> Arc<Mesh> {
    let mods: Vec<Modifier> = active(o).into_iter().cloned().collect();
    let base = poly_of(&merged(parts), 30.0);
    let me = ctx.world.get(&o.id).copied().unwrap_or(M4::I);
    let into_me = me.inverse().unwrap_or(M4::I);
    let other = |id: &str| -> Option<PolyMesh> {
        let other = find_object(ctx.objects, id)?;
        let p = evaluated_poly(other, ctx, false)?;
        Some(transform_poly(&p, &(into_me * ctx.world.get(id).copied().unwrap_or(M4::I))))
    };
    Arc::new(mesh::from_tris(&apply_modifiers(base, &mods, ctx.t, &other).triangulate()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::motion::Vec3;

    fn object(json: serde_json::Value) -> Object3d {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn welding_shares_corners() {
        let m = mesh::shape(&Shape3d::Box { size: Vec3::one(), bevel: 0.0 }).unwrap();
        let p = poly_of(&m, 30.0);
        assert_eq!(p.positions.len(), 8, "a box has eight corners");
        assert_eq!(p.faces.len(), 12);
        assert!(p.faces.iter().flatten().all(|&i| (i as usize) < p.positions.len()));
    }

    #[test]
    fn meshes_are_cached_until_something_changes() {
        let world = HashMap::new();
        let objects = vec![];
        let ctx = Ctx { t: 0.0, objects: &objects, world: &world };
        let a = object(serde_json::json!({"id": "a", "type": "sphere", "radius": 0.7}));
        let first = build(&a, &ctx).unwrap();
        let again = build(&a, &Ctx { t: 1.0, ..ctx }).unwrap();
        assert!(Arc::ptr_eq(&first.mesh, &again.mesh), "still shapes keep their mesh over time");
        let b = object(serde_json::json!({"id": "a", "type": "sphere", "radius": 0.8}));
        assert!(!Arc::ptr_eq(&first.mesh, &build(&b, &ctx).unwrap().mesh));
        // A wave moves with time: a new mesh each instant.
        let w = object(serde_json::json!({"id": "w", "type": "grid", "modifiers": [{"type": "wave", "speed": 1}]}));
        if let (Some(x), Some(y)) = (build(&w, &ctx), build(&w, &Ctx { t: 0.5, ..ctx })) {
            assert!(!Arc::ptr_eq(&x.mesh, &y.mesh));
        }
        // Groups and particles have no mesh of their own.
        assert!(build(&object(serde_json::json!({"id": "g", "type": "group"})), &ctx).is_none());
        // Edit mode has polygons for any meshed shape.
        assert!(edit_poly(&a, &ctx).is_some_and(|p| !p.faces.is_empty()));
    }
}
