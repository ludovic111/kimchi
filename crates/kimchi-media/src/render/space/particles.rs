//! 3D particles ([`Shape3d::Particles`]): the particles alive at an instant (kimchi-core's
//! stateless simulation) as small items: low-poly spheres, cubes and tetrahedra turning as they
//! fly, sparks (thin quads stretched along their motion, facing the camera) and picture cards
//! facing the camera. Each has its own colour, opacity, size and turn; the emitter's material
//! gives the rest (metal, roughness, a glow with `emissive`, unlit).
//!
//! Particles live in world space: born where the emitter was at their birth (trails), moving in
//! world directions.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use kimchi_core::anim::value_at;
use kimchi_core::motion::particles::ParticleSystem;
use kimchi_core::motion::{Object3d, Scene3d};

use super::math::{M4, V3};
use super::mesh::Mesh;
use super::{CameraRes, Item, Mat, Texture, srgb_f};

/// Small meshes particles are drawn with, made once.
fn little(kind: &str) -> Arc<Mesh> {
    static MESHES: OnceLock<HashMap<&'static str, Arc<Mesh>>> = OnceLock::new();
    let all = MESHES.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("sphere", Arc::new(icosphere()));
        m.insert("cube", Arc::new(cube()));
        m.insert("tetra", Arc::new(tetra()));
        m.insert("quad", Arc::new(quad()));
        m
    });
    all.get(kind).or_else(|| all.get("sphere")).expect("made above").clone()
}

/// An icosahedron split once (80 faces), radius 0.5, smooth.
fn icosphere() -> Mesh {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut v: Vec<V3> = [
        (-1.0, t, 0.0), (1.0, t, 0.0), (-1.0, -t, 0.0), (1.0, -t, 0.0), (0.0, -1.0, t), (0.0, 1.0, t),
        (0.0, -1.0, -t), (0.0, 1.0, -t), (t, 0.0, -1.0), (t, 0.0, 1.0), (-t, 0.0, -1.0), (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| V3(x, y, z).norm())
    .collect();
    let faces: [[usize; 3]; 20] = [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    let mut tris = vec![];
    let mut mid: HashMap<(usize, usize), usize> = HashMap::new();
    let mut half = |a: usize, b: usize, v: &mut Vec<V3>| -> usize {
        *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
            v.push(((v[a] + v[b]) * 0.5).norm());
            v.len() - 1
        })
    };
    for [a, b, c] in faces {
        let (ab, bc, ca) = (half(a, b, &mut v), half(b, c, &mut v), half(c, a, &mut v));
        tris.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
    }
    let mut m = Mesh::default();
    for p in &v {
        m.push(*p * 0.5, *p, [0.5 + p.0.atan2(p.2) / std::f32::consts::TAU, 0.5 - p.1 * 0.5]);
    }
    for [a, b, c] in tris {
        m.tri(a as u32, b as u32, c as u32);
    }
    m
}

/// A unit cube, flat faces.
fn cube() -> Mesh {
    let mut m = Mesh::default();
    for (n, u, v) in [
        (V3(1.0, 0.0, 0.0), V3(0.0, 0.0, -1.0), V3(0.0, 1.0, 0.0)),
        (V3(-1.0, 0.0, 0.0), V3(0.0, 0.0, 1.0), V3(0.0, 1.0, 0.0)),
        (V3(0.0, 1.0, 0.0), V3(1.0, 0.0, 0.0), V3(0.0, 0.0, -1.0)),
        (V3(0.0, -1.0, 0.0), V3(1.0, 0.0, 0.0), V3(0.0, 0.0, 1.0)),
        (V3(0.0, 0.0, 1.0), V3(1.0, 0.0, 0.0), V3(0.0, 1.0, 0.0)),
        (V3(0.0, 0.0, -1.0), V3(-1.0, 0.0, 0.0), V3(0.0, 1.0, 0.0)),
    ] {
        let base = m.pos.len() as u32;
        for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            m.push(n * 0.5 + u * a + v * b, n, [a + 0.5, 0.5 - b]);
        }
        m.tri(base, base + 1, base + 2);
        m.tri(base, base + 2, base + 3);
    }
    m
}

/// A tetrahedron fitting in a unit ball's diameter, flat faces.
fn tetra() -> Mesh {
    let k = 0.5 / 3f32.sqrt();
    let p = [V3(1.0, 1.0, 1.0) * k, V3(-1.0, -1.0, 1.0) * k, V3(-1.0, 1.0, -1.0) * k, V3(1.0, -1.0, -1.0) * k];
    let mut m = Mesh::default();
    for [a, b, c] in [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]] {
        let n = (p[b] - p[a]).cross(p[c] - p[a]).norm();
        let base = m.pos.len() as u32;
        m.push(p[a], n, [0.5, 0.0]);
        m.push(p[b], n, [0.0, 1.0]);
        m.push(p[c], n, [1.0, 1.0]);
        m.tri(base, base + 1, base + 2);
    }
    m
}

/// A unit card in x/y facing +z, centred.
fn quad() -> Mesh {
    let mut m = Mesh::default();
    m.grid(1, 1, |u, v| (V3(u - 0.5, 0.5 - v, 0.0), V3(0.0, 0.0, 1.0)));
    m
}

/// Where an object (by its chain of parents from the scene's top level) was at any time, for
/// trails. Keyframes only when nothing on the chain has expressions or constraints (cheap);
/// otherwise the whole scene evaluated, sampled 60 times a second and remembered.
pub(crate) struct Origin<'a> {
    scene: &'a Scene3d,
    id: String,
    chain: Vec<&'a Object3d>,
    full: bool,
    memo: RefCell<HashMap<i64, [f64; 3]>>,
}

impl<'a> Origin<'a> {
    pub(crate) fn new(scene: &'a Scene3d, id: &str) -> Origin<'a> {
        let mut chain = vec![];
        fn find<'b>(objects: &'b [Object3d], id: &str, chain: &mut Vec<&'b Object3d>) -> bool {
            for o in objects {
                chain.push(o);
                if o.id == id || find(&o.children, id, chain) {
                    return true;
                }
                chain.pop();
            }
            false
        }
        find(&scene.objects, id, &mut chain);
        let full = chain.iter().any(|o| !o.constraints.is_empty() || !o.expressions.is_empty());
        Origin { scene, id: id.to_string(), chain, full, memo: RefCell::new(HashMap::new()) }
    }

    pub(crate) fn at(&self, t: f64) -> [f64; 3] {
        if !self.full {
            let mut m = M4::I;
            for o in &self.chain {
                m = m * local_at(o, t);
            }
            let p = m.origin();
            return [p.0 as f64, p.1 as f64, p.2 as f64];
        }
        // Sampled on a 60 Hz grid, straight lines between.
        let step = 1.0 / 60.0;
        let k = (t / step).floor();
        let a = self.sampled(k as i64);
        let b = self.sampled(k as i64 + 1);
        let f = t / step - k;
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f)
    }

    fn sampled(&self, k: i64) -> [f64; 3] {
        if let Some(p) = self.memo.borrow().get(&k) {
            return *p;
        }
        let t = k as f64 / 60.0;
        let objects = self.scene.objects_at(t);
        let p = world_of(&objects, &self.id, M4::I).map_or([0.0; 3], |m| {
            let o = m.origin();
            [o.0 as f64, o.1 as f64, o.2 as f64]
        });
        self.memo.borrow_mut().insert(k, p);
        p
    }
}

fn world_of(objects: &[Object3d], id: &str, parent: M4) -> Option<M4> {
    for o in objects {
        let m = parent * M4::trs(V3::from(o.position.0), V3::from(o.rotation.0), V3::from(o.scale.0));
        if o.id == id {
            return Some(m);
        }
        if let Some(found) = world_of(&o.children, id, m) {
            return Some(found);
        }
    }
    None
}

/// An object's own placement at `t` from its keyframes.
fn local_at(o: &Object3d, t: f64) -> M4 {
    let (mut p, mut r, mut s) = (o.position.0, o.rotation.0, o.scale.0);
    for (name, keys) in &o.keyframes {
        let (target, axis) = match name.as_str() {
            "x" => (&mut p, Some(0)),
            "y" => (&mut p, Some(1)),
            "z" => (&mut p, Some(2)),
            n => {
                let (base, axis) = n.split_once('.').map_or((n, None), |(b, a)| (b, Some(a)));
                let target = match base {
                    "position" => &mut p,
                    "rotation" => &mut r,
                    "scale" => &mut s,
                    _ => continue,
                };
                let axis = match axis {
                    None => None,
                    Some("x") => Some(0),
                    Some("y") => Some(1),
                    Some("z") => Some(2),
                    _ => continue,
                };
                (target, axis)
            }
        };
        let Some(v) = value_at(keys, t) else { continue };
        match axis {
            Some(i) => {
                if let Some(n) = v.as_f64() {
                    target[i] = n;
                }
            }
            None => {
                if let Some(vs) = v.as_vec(3) {
                    *target = [vs[0], vs[1], vs[2]];
                }
            }
        }
    }
    M4::trs(V3::from(p), V3::from(r), V3::from(s))
}

/// The items for an emitter's particles at scene time `t`. `here` is the emitter's world
/// position now; `mat` its material.
#[allow(clippy::too_many_arguments)]
pub(crate) fn items(
    sys: &ParticleSystem,
    t: f64,
    here: V3,
    origin: &dyn Fn(f64) -> [f64; 3],
    camera: &CameraRes,
    mat: &Mat,
    tint: bool,
    image: Option<Arc<Texture>>,
    id: &Arc<str>,
    cast_shadow: bool,
    out: &mut Vec<Item>,
) {
    let shape = sys.shape.as_deref().unwrap_or("sphere");
    for p in sys.at(t, true, origin) {
        let pos = here + V3(p.pos[0] as f32, p.pos[1] as f32, p.pos[2] as f32);
        let size = p.size as f32;
        let alpha = (p.color.0[3] / 255.0).clamp(0.0, 1.0) as f32;
        if size <= 0.0 || alpha <= 0.0 || !pos.finite() {
            continue;
        }
        let c = [srgb_f((p.color.0[0] / 255.0) as f32), srgb_f((p.color.0[1] / 255.0) as f32), srgb_f((p.color.0[2] / 255.0) as f32)];
        let mut m = mat.clone();
        for k in 0..3 {
            m.base[k] = if tint { c[k] * mat.base[k] } else { c[k] };
            // A glowing material glows in each particle's colour.
            m.emissive[k] = mat.emissive[k] * c[k];
        }
        m.base[3] = alpha * mat.base[3];
        let to_camera = if camera.ortho { -camera.forward } else { (camera.eye - pos).norm() };
        let (mesh, model) = match shape {
            "spark" => {
                let v = V3(p.vel[0] as f32, p.vel[1] as f32, p.vel[2] as f32);
                let speed = v.len();
                let along = if speed > 1e-6 { v * (1.0 / speed) } else { camera.up };
                let len = speed * sys.stretch.max(0.0) as f32 + size;
                let mut side = along.cross(to_camera);
                if side.len() < 1e-6 {
                    side = along.perpendicular();
                }
                let side = side.norm();
                let face = side.cross(along).norm();
                // The head at the particle, the streak behind it.
                let center = pos - along * ((len - size) / 2.0);
                (little("quad"), M4::from_axes(along * len, side * size, face, center))
            }
            "image" => {
                let Some(tex) = image.clone() else { continue };
                let aspect = tex.height as f32 / tex.width.max(1) as f32;
                let turn = (p.rotation as f32).to_radians();
                let (s, co) = turn.sin_cos();
                let (right, up) = (camera.right * co + camera.up * s, camera.up * co - camera.right * s);
                m.texture = Some(tex);
                m.unlit = true;
                (little("quad"), M4::from_axes(right * size, up * (size * aspect), right.cross(up).norm(), pos))
            }
            other => {
                let r = p.rotation as f32;
                (little(other), M4::translate(pos) * M4::rot_y(r) * M4::rot_x(r * 0.7) * M4::scale(V3(size, size, size)))
            }
        };
        let depth = (pos - camera.eye).dot(camera.forward);
        out.push(Item { mesh, model, normal: model.normal_matrix(), mat: m, depth, id: id.clone(), cast_shadow });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_meshes_are_closed_and_sized() {
        for k in ["sphere", "cube", "tetra"] {
            let m = little(k);
            assert!(!m.index.is_empty() && m.index.len() % 3 == 0);
            let (lo, hi) = m.bounds();
            assert!(hi.0 <= 0.5 + 1e-4 && lo.0 >= -0.5 - 1e-4, "{k}: {lo:?} {hi:?}");
        }
        assert_eq!(little("sphere").index.len() / 3, 80);
    }

    #[test]
    fn origins_follow_keyframes_and_parents() {
        let scene: Scene3d = serde_json::from_value(serde_json::json!({
            "objects": [{"id": "g", "type": "group", "position": [0, 1, 0],
                         "children": [{"id": "e", "type": "particles", "keyframes": {"x": [[0, 0], [1, 2]]}}]}]
        }))
        .unwrap();
        let o = Origin::new(&scene, "e");
        assert!(!o.full);
        let at = o.at(0.5);
        assert!((at[0] - 1.0).abs() < 1e-6 && (at[1] - 1.0).abs() < 1e-6, "{at:?}");
    }
}
