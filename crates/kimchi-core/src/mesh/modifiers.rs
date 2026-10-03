//! The modifier stack (see `stack::MODIFIERS`): each enabled modifier reshapes the mesh in
//! order, at a scene time (wave, displace and jitter move with it). Every step is bounded: one
//! that would make more than [`MAX_FACES`] faces is skipped or reduced, and logged.

use std::collections::{HashMap, HashSet};

use crate::motion::stack::Modifier;

use super::csg::{BoolOp, boolean};
use super::math::*;
use super::noise::fbm;
use super::triangulate::face_triangles;
use super::{MAX_FACES, PolyMesh, bevel, decimate};

pub(super) fn apply(mut mesh: PolyMesh, modifiers: &[Modifier], t: f64, object: &dyn Fn(&str) -> Option<PolyMesh>) -> PolyMesh {
    mesh.sanitize();
    mesh.compact();
    let t = if t.is_finite() { t } else { 0.0 };
    for m in modifiers.iter().filter(|m| m.enabled) {
        if mesh.faces.is_empty() && m.kind != "boolean" {
            continue;
        }
        mesh = match m.kind.as_str() {
            "subdivision" => subdivision(mesh, m.n("levels").round() as usize, m.b("simple")),
            "mirror" => mirror(mesh, axis_index(&m.s("axis")), m.b("merge"), m.b("bisect")),
            "array" => array(mesh, m),
            "bevel" => {
                let edges = bevel::sharp_edges(&mesh, m.n("angle"));
                let mut segments = m.n("segments").round().max(1.0) as usize;
                // Each bevelled edge adds a strip; corners add patches about as big.
                let room = MAX_FACES.saturating_sub(mesh.faces.len()) / (edges.len() * 2).max(1);
                if segments > room {
                    tracing::warn!(segments, room, "bevel segments reduced: too many faces");
                    segments = room;
                }
                if segments == 0 { mesh } else { bevel::bevel_edges(&mesh, &edges, m.n("width"), segments).0 }
            }
            "solidify" => solidify(mesh, m.n("thickness"), m.b("rim")),
            "displace" => displace(mesh, m),
            "twist" => twist(mesh, m.n("angle"), axis_index(&m.s("axis"))),
            "bend" => bend(mesh, m.n("angle"), axis_index(&m.s("axis")), axis_index(&m.s("toward"))),
            "taper" => taper(mesh, m.n("amount"), axis_index(&m.s("axis"))),
            "wave" => wave(mesh, m, t),
            "smooth" => smooth(mesh, m.n("factor"), m.n("iterations").round() as usize, None),
            "wireframe" => wireframe(&mesh, m.n("thickness")),
            "boolean" => match m.opt_s("object").and_then(|id| object(&id)) {
                Some(other) => {
                    let op = match m.s("operation").as_str() {
                        "union" => BoolOp::Union,
                        "intersect" => BoolOp::Intersect,
                        _ => BoolOp::Difference,
                    };
                    boolean(&mesh, &other, op)
                }
                None => mesh,
            },
            "decimate" => decimate::decimate(&mesh, m.n("ratio")),
            "triangulate" => {
                let mut out = triangulated(&mesh, None);
                out.smooth_angle = 0.0;
                out
            }
            "explode" => explode(mesh, m),
            "build" => build(mesh, m),
            "weld" => {
                let mut mesh = mesh;
                mesh.weld(m.n("distance"));
                mesh
            }
            "spherify" => spherify(mesh, m.n("factor")),
            "noise" => jitter(mesh, m.n("amount"), m.n("seed"), m.n("speed"), t),
            other => {
                tracing::warn!("unknown modifier type `{other}` skipped");
                mesh
            }
        };
    }
    mesh
}

/// Faces split into triangles (only those in `only`, when given).
pub(crate) fn triangulated(mesh: &PolyMesh, only: Option<&HashSet<usize>>) -> PolyMesh {
    let mut out = PolyMesh { positions: mesh.positions.clone(), smooth_angle: mesh.smooth_angle, uvs: mesh.uvs.as_ref().map(|_| vec![]), faces: vec![] };
    for (fi, f) in mesh.faces.iter().enumerate() {
        let uv = mesh.uvs.as_ref().map(|_| mesh.corner_uvs(fi));
        if f.len() <= 3 || only.is_some_and(|s| !s.contains(&fi)) {
            out.add_face(f.clone(), uv);
            continue;
        }
        for t in face_triangles(&mesh.face_points(fi)) {
            out.add_face(t.iter().map(|&k| f[k]).collect(), uv.as_ref().map(|u| t.iter().map(|&k| u[k]).collect()));
        }
    }
    out
}

// ---- subdivision ----------------------------------------------------------------------------

fn subdivision(mut mesh: PolyMesh, levels: usize, simple: bool) -> PolyMesh {
    for _ in 0..levels.min(6) {
        let next: usize = mesh.faces.iter().map(|f| f.len()).sum();
        if next > MAX_FACES {
            tracing::warn!(faces = next, "subdivision stopped: too many faces");
            break;
        }
        mesh = catmull_clark(&mesh, simple);
    }
    mesh
}

/// One level of Catmull-Clark (or a plain split when `simple`): every n-gon becomes n quads.
pub(crate) fn catmull_clark(m: &PolyMesh, simple: bool) -> PolyMesh {
    let nv = m.positions.len();
    let edges = m.edges();
    let mut edge_index: HashMap<(u32, u32), usize> = HashMap::with_capacity(edges.len());
    for (i, e) in edges.iter().enumerate() {
        edge_index.insert((e.a, e.b), i);
    }
    let face_pts: Vec<V3> = (0..m.faces.len()).map(|f| m.face_center(f)).collect();
    let mid = |e: &super::Edge| lerp(m.positions[e.a as usize], m.positions[e.b as usize], 0.5);
    let edge_pts: Vec<V3> = edges
        .iter()
        .map(|e| {
            if !simple && e.faces.len() == 2 {
                scale(add(add(m.positions[e.a as usize], m.positions[e.b as usize]), add(face_pts[e.faces[0]], face_pts[e.faces[1]])), 0.25)
            } else {
                mid(e)
            }
        })
        .collect();
    let mut positions = m.positions.clone();
    if !simple {
        // Per vertex: sum of face points, sum of edge midpoints, counts, border neighbours.
        let mut fsum = vec![[0.0; 3]; nv];
        let mut fcount = vec![0usize; nv];
        for (fi, f) in m.faces.iter().enumerate() {
            for &v in f {
                fsum[v as usize] = add(fsum[v as usize], face_pts[fi]);
                fcount[v as usize] += 1;
            }
        }
        let mut esum = vec![[0.0; 3]; nv];
        let mut ecount = vec![0usize; nv];
        let mut border: Vec<Vec<u32>> = vec![vec![]; nv];
        let mut odd = vec![false; nv];
        for e in &edges {
            let md = mid(e);
            for (v, o) in [(e.a, e.b), (e.b, e.a)] {
                esum[v as usize] = add(esum[v as usize], md);
                ecount[v as usize] += 1;
                match e.faces.len() {
                    1 => border[v as usize].push(o),
                    2 => {}
                    _ => odd[v as usize] = true,
                }
            }
        }
        for v in 0..nv {
            let p = m.positions[v];
            positions[v] = if odd[v] || fcount[v] == 0 {
                p
            } else if !border[v].is_empty() {
                if border[v].len() == 2 {
                    let (a, b) = (m.positions[border[v][0] as usize], m.positions[border[v][1] as usize]);
                    add(scale(p, 0.75), scale(add(a, b), 0.125))
                } else {
                    p
                }
            } else {
                let n = fcount[v] as f64;
                if n < 3.0 || ecount[v] == 0 {
                    p
                } else {
                    let f = scale(fsum[v], 1.0 / n);
                    let r = scale(esum[v], 1.0 / ecount[v] as f64);
                    scale(add(add(f, scale(r, 2.0)), scale(p, n - 3.0)), 1.0 / n)
                }
            };
        }
    }
    let ebase = positions.len() as u32;
    positions.extend_from_slice(&edge_pts);
    let fbase = positions.len() as u32;
    positions.extend_from_slice(&face_pts);
    let mut out = PolyMesh { positions, faces: Vec::with_capacity(m.faces.len() * 4), uvs: m.uvs.as_ref().map(|_| vec![]), smooth_angle: m.smooth_angle };
    for (fi, f) in m.faces.iter().enumerate() {
        let n = f.len();
        let uv = m.uvs.as_ref().map(|_| m.corner_uvs(fi));
        let e_of = |a: u32, b: u32| ebase + edge_index[&(a.min(b), a.max(b))] as u32;
        let fuv = uv.as_ref().map(|u| {
            let s = u.iter().fold([0.0; 2], |a, c| [a[0] + c[0], a[1] + c[1]]);
            [s[0] / n as f64, s[1] / n as f64]
        });
        for k in 0..n {
            let (p, v, q) = (f[(k + n - 1) % n], f[k], f[(k + 1) % n]);
            let face = vec![v, e_of(v, q), fbase + fi as u32, e_of(p, v)];
            let corner_uv = uv.as_ref().map(|u| {
                let (up, uc, uq) = (u[(k + n - 1) % n], u[k], u[(k + 1) % n]);
                vec![uc, [(uc[0] + uq[0]) / 2.0, (uc[1] + uq[1]) / 2.0], fuv.unwrap_or(uc), [(uc[0] + up[0]) / 2.0, (uc[1] + up[1]) / 2.0]]
            });
            out.faces.push(face);
            if let (Some(o), Some(c)) = (out.uvs.as_mut(), corner_uv) {
                o.push(c);
            }
        }
    }
    out
}

// ---- mirror and array -----------------------------------------------------------------------

fn mirror(mut mesh: PolyMesh, axis: usize, merge: bool, bisect: bool) -> PolyMesh {
    if mesh.faces.len() * 2 > MAX_FACES {
        tracing::warn!(faces = mesh.faces.len(), "mirror skipped: too many faces");
        return mesh;
    }
    let tol = (mesh.size() * 1e-5).max(1e-9);
    if bisect {
        // Keep the side holding most of the mesh (the positive side when balanced).
        let c: f64 = mesh.positions.iter().map(|p| p[axis]).sum::<f64>() / mesh.positions.len().max(1) as f64;
        let keep = if c < -tol { -1.0 } else { 1.0 };
        let n = scale(unit(axis), keep);
        mesh = super::ops::cut_plane(&mesh, [0.0; 3], n, None).0;
        let positions = mesh.positions.clone();
        let faces = mesh.faces.clone();
        mesh.retain_faces(|f| faces[f].iter().all(|&v| positions[v as usize][axis] * keep >= -tol));
        mesh.compact();
    }
    let nv = mesh.positions.len() as u32;
    let mut map: Vec<u32> = Vec::with_capacity(nv as usize);
    for i in 0..nv {
        let p = mesh.positions[i as usize];
        if merge && p[axis].abs() <= tol {
            mesh.positions[i as usize][axis] = 0.0;
            map.push(i);
        } else {
            let mut q = p;
            q[axis] = -q[axis];
            mesh.positions.push(q);
            map.push((mesh.positions.len() - 1) as u32);
        }
    }
    let count = mesh.faces.len();
    let on_plane: Vec<bool> = mesh.faces.iter().map(|f| merge && f.iter().all(|&v| map[v as usize] == v)).collect();
    for fi in 0..count {
        if on_plane[fi] {
            continue;
        }
        let f: Vec<u32> = mesh.faces[fi].iter().rev().map(|&v| map[v as usize]).collect();
        let uv = mesh.uvs.as_ref().map(|u| u[fi].iter().rev().copied().collect());
        mesh.add_face(f, uv);
    }
    // Faces lying in the mirror plane would sit inside the welded result.
    mesh.retain_faces(|f| f >= count || !on_plane[f]);
    mesh.sanitize();
    mesh.compact();
    mesh
}

fn array(mesh: PolyMesh, m: &Modifier) -> PolyMesh {
    let mut count = m.n("count").round().max(1.0) as usize;
    if count * mesh.faces.len() > MAX_FACES {
        let fit = (MAX_FACES / mesh.faces.len().max(1)).max(1);
        tracing::warn!(count, fit, "array reduced: too many faces");
        count = fit;
    }
    if count <= 1 {
        return mesh;
    }
    let (lo, hi) = mesh.bounds();
    let size = sub(hi, lo);
    let rel = m.v3("relative");
    let step = add([rel[0] * size[0], rel[1] * size[1], rel[2] * size[2]], m.v3("offset"));
    let rot = euler(m.v3("rotation"));
    let s = m.n("scale");
    let mut out = mesh.clone();
    let mut copy = mesh.clone();
    for _ in 1..count {
        for p in &mut copy.positions {
            *p = add(scale(mat_mul(&rot, *p), s), step);
        }
        out.append(&copy);
    }
    if m.b("merge") {
        out.weld((mesh.size() * 1e-4).max(1e-7));
    }
    out
}

// ---- solidify -------------------------------------------------------------------------------

/// Angle-weighted vertex normals.
pub(crate) fn vertex_normals(m: &PolyMesh) -> Vec<V3> {
    let mut out = vec![[0.0; 3]; m.positions.len()];
    for (fi, f) in m.faces.iter().enumerate() {
        let n = m.face_normal(fi);
        let k = f.len();
        for i in 0..k {
            let p = m.positions[f[i] as usize];
            let a = norm(sub(m.positions[f[(i + k - 1) % k] as usize], p));
            let b = norm(sub(m.positions[f[(i + 1) % k] as usize], p));
            let w = dot(a, b).clamp(-1.0, 1.0).acos();
            out[f[i] as usize] = mad(out[f[i] as usize], n, if w.is_finite() { w } else { 0.0 });
        }
    }
    out.into_iter().map(norm).collect()
}

/// For each vertex, the move (per unit of thickness) that puts it one unit from the plane of
/// every face around it, as near as can be: on flat and smooth parts its normal, on an edge or a
/// corner where the offset faces meet (a cube's corner moves by [1, 1, 1]). The faces' normals
/// are grouped by direction (within about 11°) and each direction counts once, so a sliver
/// left by a boolean counts as much as a big face; least squares over them, and in directions
/// they barely constrain (along an edge, across faces less than about 25° apart) the vertex
/// normal decides. No move is longer than three units. (Pushing along the averaged normal alone
/// went out through the other side of sharp edges: a bowl cut by a boolean grew spikes.)
fn even_offsets(mesh: &PolyMesh) -> Vec<V3> {
    let normals = vertex_normals(mesh);
    let n = mesh.positions.len();
    let mut around: Vec<Vec<V3>> = vec![vec![]; n];
    for (fi, f) in mesh.faces.iter().enumerate() {
        let fnorm = mesh.face_normal(fi);
        if !finite(fnorm) || len(fnorm) < 0.5 {
            continue;
        }
        for &v in f {
            let groups = &mut around[v as usize];
            match groups.iter_mut().find(|g| dot(norm(**g), fnorm) > 0.98) {
                Some(g) => *g = add(*g, fnorm),
                None => groups.push(fnorm),
            }
        }
    }
    (0..n)
        .map(|v| {
            let nv = normals[v];
            if !finite(nv) || len(nv) < 0.5 || around[v].is_empty() {
                return nv;
            }
            let mut a = [[0.0f64; 3]; 3];
            let mut b = [0.0f64; 3];
            for g in &around[v] {
                let g = norm(*g);
                for r in 0..3 {
                    for c in 0..3 {
                        a[r][c] += g[r] * g[c];
                    }
                    b[r] += g[r];
                }
            }
            let (values, vectors) = eigen3(a);
            let top = values.iter().cloned().fold(0.0, f64::max);
            if top <= 0.0 {
                return nv;
            }
            // In the eigenbasis each well-constrained part comes from the faces, the rest from
            // the normal.
            let mut d = [0.0; 3];
            for k in 0..3 {
                let e = vectors[k];
                let part = if values[k] > top * 0.05 { dot(e, b) / values[k] } else { dot(e, nv) };
                d = mad(d, e, part);
            }
            if !finite(d) {
                return nv;
            }
            let l = len(d);
            if l > 3.0 { scale(d, 3.0 / l) } else { d }
        })
        .collect()
}

/// Eigenvalues and unit eigenvectors of a symmetric 3×3 matrix (Jacobi rotations).
fn eigen3(mut m: [[f64; 3]; 3]) -> ([f64; 3], [V3; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        let off = m[0][1].abs() + m[0][2].abs() + m[1][2].abs();
        if off < 1e-15 * (m[0][0].abs() + m[1][1].abs() + m[2][2].abs()).max(1e-300) {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if m[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
            let t = theta.signum().max(0.0) * 2.0 - 1.0;
            let t = t / (theta.abs() + (theta * theta + 1.0).sqrt());
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            // m ← Jᵀ m J with J the rotation in the p–q plane.
            for row in m.iter_mut() {
                let (mkp, mkq) = (row[p], row[q]);
                row[p] = c * mkp - s * mkq;
                row[q] = s * mkp + c * mkq;
            }
            let (rp, rq) = (m[p], m[q]);
            m[p] = std::array::from_fn(|k| c * rp[k] - s * rq[k]);
            m[q] = std::array::from_fn(|k| s * rp[k] + c * rq[k]);
            for row in v.iter_mut() {
                let (a, b) = (row[p], row[q]);
                row[p] = c * a - s * b;
                row[q] = s * a + c * b;
            }
        }
    }
    let col = |k: usize| [v[0][k], v[1][k], v[2][k]];
    ([m[0][0], m[1][1], m[2][2]], [col(0), col(1), col(2)])
}

fn solidify(mesh: PolyMesh, thickness: f64, rim: bool) -> PolyMesh {
    if mesh.faces.len() * 3 > MAX_FACES || thickness == 0.0 {
        return mesh;
    }
    let offsets = even_offsets(&mesh);
    let nv = mesh.positions.len() as u32;
    let mut out = mesh.clone();
    for (i, p) in mesh.positions.iter().enumerate() {
        out.positions.push(mad(*p, offsets[i], -thickness));
    }
    for (fi, f) in mesh.faces.iter().enumerate() {
        let inner: Vec<u32> = f.iter().rev().map(|v| v + nv).collect();
        let uv = mesh.uvs.as_ref().map(|u| u[fi].iter().rev().copied().collect());
        out.add_face(inner, uv);
    }
    if rim {
        for e in mesh.edges().iter().filter(|e| e.faces.len() == 1) {
            // The outer face walks a → b; the rim walks back.
            let f = &mesh.faces[e.faces[0]];
            let k = f.iter().position(|&v| v == e.a).unwrap_or(0);
            let (a, b) = if f[(k + 1) % f.len()] == e.b { (e.a, e.b) } else { (e.b, e.a) };
            out.add_face(vec![b, a, a + nv, b + nv], None);
        }
    }
    if thickness < 0.0 {
        // Pushed outward: the copy is the outside, so everything turns around.
        for f in 0..out.faces.len() {
            out.flip_face(f);
        }
    }
    out
}

// ---- deformers ------------------------------------------------------------------------------

fn displace(mut mesh: PolyMesh, m: &Modifier) -> PolyMesh {
    let strength = m.n("strength");
    let size = m.n("scale").max(1e-6);
    let evolution = m.n("evolution");
    let octaves = m.n("octaves").round() as usize;
    let seed = m.n("seed").round() as u64;
    let dir = m.s("direction");
    let normals = if dir == "normal" { vertex_normals(&mesh) } else { vec![] };
    for (i, p) in mesh.positions.iter_mut().enumerate() {
        let n = fbm(scale(*p, 1.0 / size), evolution, octaves, seed);
        let d = match dir.as_str() {
            "x" => [1.0, 0.0, 0.0],
            "y" => [0.0, 1.0, 0.0],
            "z" => [0.0, 0.0, 1.0],
            _ => normals[i],
        };
        *p = mad(*p, d, strength * n);
    }
    mesh
}

/// Where each vertex sits along `axis` from 0 (lowest) to 1 (highest), and the length.
fn spans(mesh: &PolyMesh, axis: usize) -> (f64, f64) {
    let (lo, hi) = mesh.bounds();
    (lo[axis], hi[axis] - lo[axis])
}

fn twist(mut mesh: PolyMesh, angle: f64, axis: usize) -> PolyMesh {
    let (lo, length) = spans(&mesh, axis);
    if length <= 0.0 || !angle.is_finite() {
        return mesh;
    }
    let a = unit(axis);
    for p in &mut mesh.positions {
        let t = (p[axis] - lo) / length - 0.5;
        *p = rotate(*p, a, (angle * t).to_radians());
    }
    mesh
}

fn bend(mut mesh: PolyMesh, angle: f64, axis: usize, toward: usize) -> PolyMesh {
    let toward = if toward == axis { (axis + 1) % 3 } else { toward };
    let (lo, length) = spans(&mesh, axis);
    let total = angle.to_radians();
    if length <= 0.0 || !total.is_finite() || total.abs() < 1e-9 {
        return mesh;
    }
    let mid = lo + length / 2.0;
    let r = length / total;
    for p in &mut mesh.positions {
        let s = p[axis] - mid;
        let h = p[toward];
        let phi = s / r;
        p[axis] = mid + (r - h) * phi.sin();
        p[toward] = r - (r - h) * phi.cos();
    }
    mesh
}

fn taper(mut mesh: PolyMesh, amount: f64, axis: usize) -> PolyMesh {
    let (lo, length) = spans(&mesh, axis);
    if length <= 0.0 {
        return mesh;
    }
    let (blo, bhi) = mesh.bounds();
    let centre = lerp(blo, bhi, 0.5);
    for p in &mut mesh.positions {
        let k = 1.0 - amount * (p[axis] - lo) / length;
        for c in 0..3 {
            if c != axis {
                p[c] = centre[c] + (p[c] - centre[c]) * k;
            }
        }
    }
    mesh
}

fn wave(mut mesh: PolyMesh, m: &Modifier, t: f64) -> PolyMesh {
    let amp = m.n("amplitude");
    let wl = m.n("wavelength").max(1e-6);
    let speed = m.n("speed");
    let along = axis_index(&m.s("axis"));
    let dir = m.s("direction");
    let radial = m.b("radial");
    let normals = if dir == "normal" { vertex_normals(&mesh) } else { vec![] };
    let moves = match dir.as_str() {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        _ => None,
    };
    // Ripples spread in the plane across the direction of movement (across y for normals).
    let flat = moves.unwrap_or(1);
    for (i, p) in mesh.positions.iter_mut().enumerate() {
        let s = if radial {
            (0..3).filter(|&c| c != flat).map(|c| p[c] * p[c]).sum::<f64>().sqrt()
        } else {
            p[along]
        };
        let off = amp * (TAU * (s / wl - speed * t)).sin();
        match moves {
            Some(c) => p[c] += off,
            None => *p = mad(*p, normals[i], off),
        }
    }
    mesh
}

use std::f64::consts::TAU;

/// Laplacian smoothing (of the `only` vertices, when given).
pub(crate) fn smooth(mut mesh: PolyMesh, factor: f64, iterations: usize, only: Option<&HashSet<u32>>) -> PolyMesh {
    let nb = mesh.neighbours();
    let factor = factor.clamp(0.0, 1.0);
    for _ in 0..iterations.min(100) {
        let prev = mesh.positions.clone();
        for (v, list) in nb.iter().enumerate() {
            if list.is_empty() || only.is_some_and(|s| !s.contains(&(v as u32))) {
                continue;
            }
            let avg = scale(list.iter().fold([0.0; 3], |a, &u| add(a, prev[u as usize])), 1.0 / list.len() as f64);
            mesh.positions[v] = lerp(prev[v], avg, factor);
        }
    }
    mesh
}

fn spherify(mut mesh: PolyMesh, factor: f64) -> PolyMesh {
    let (lo, hi) = mesh.bounds();
    let c = lerp(lo, hi, 0.5);
    let n = mesh.positions.len().max(1) as f64;
    let r = mesh.positions.iter().map(|p| dist(*p, c)).sum::<f64>() / n;
    for p in &mut mesh.positions {
        if let Some(d) = try_norm(sub(*p, c)) {
            *p = lerp(*p, mad(c, d, r), factor);
        }
    }
    mesh
}

fn jitter(mut mesh: PolyMesh, amount: f64, seed: f64, speed: f64, t: f64) -> PolyMesh {
    let frame = if speed > 0.0 { (t * speed).floor() } else { 0.0 };
    let seed = seed.round() as i64 as u64;
    let q = (mesh.size() * 1e-7).max(1e-12);
    for p in &mut mesh.positions {
        // Keyed by position so vertices split at seams move together.
        let k = p.map(|c| (c / q).round() as i64 as u64);
        let r: V3 = std::array::from_fn(|i| hash01(&[k[0], k[1], k[2], seed, frame as i64 as u64, i as u64]) * 2.0 - 1.0);
        *p = mad(*p, r, amount);
    }
    mesh
}

// ---- wireframe ------------------------------------------------------------------------------

fn wireframe(mesh: &PolyMesh, thickness: f64) -> PolyMesh {
    let edges = mesh.edges();
    let mut out = PolyMesh { smooth_angle: 30.0, ..Default::default() };
    if thickness <= 0.0 || edges.len() * 6 > MAX_FACES {
        if thickness > 0.0 {
            tracing::warn!(edges = edges.len(), "wireframe skipped: too many edges");
        }
        return out;
    }
    let normals: Vec<V3> = (0..mesh.faces.len()).map(|f| mesh.face_normal(f)).collect();
    let h = thickness / 2.0;
    for e in &edges {
        let (a, b) = (mesh.positions[e.a as usize], mesh.positions[e.b as usize]);
        let Some(d) = try_norm(sub(b, a)) else { continue };
        let up = e.faces.iter().fold([0.0; 3], |s, &f| add(s, normals[f]));
        let up = try_norm(sub(up, scale(d, dot(up, d)))).unwrap_or_else(|| perpendicular(d));
        let side = cross(d, up);
        // A square bar a little longer than the edge so corners close.
        let (a, b) = (mad(a, d, -h), mad(b, d, h));
        let corners = [add(scale(up, h), scale(side, h)), add(scale(up, h), scale(side, -h)), add(scale(up, -h), scale(side, -h)), add(scale(up, -h), scale(side, h))];
        let base = out.positions.len() as u32;
        for c in corners {
            out.positions.push(add(a, c));
        }
        for c in corners {
            out.positions.push(add(b, c));
        }
        for k in 0..4u32 {
            let k1 = (k + 1) % 4;
            out.faces.push(vec![base + k, base + 4 + k, base + 4 + k1, base + k1]);
        }
        // The corners go clockwise around d: the start cap faces −d as listed, the end cap +d.
        out.faces.push(vec![base, base + 1, base + 2, base + 3]);
        out.faces.push(vec![base + 7, base + 6, base + 5, base + 4]);
    }
    out
}

// ---- explode and build ----------------------------------------------------------------------

fn explode(mesh: PolyMesh, m: &Modifier) -> PolyMesh {
    let progress = m.n("progress");
    if progress <= 0.0 {
        return mesh;
    }
    let distance = m.n("distance");
    let spin = m.n("spin");
    let gravity = m.n("gravity");
    let seed = m.n("seed").round() as u64;
    let (lo, hi) = mesh.bounds();
    let centre = lerp(lo, hi, 0.5);
    let mut out = PolyMesh { smooth_angle: mesh.smooth_angle, uvs: mesh.uvs.as_ref().map(|_| vec![]), ..Default::default() };
    for (fi, f) in mesh.faces.iter().enumerate() {
        let r = |k: u64| hash01(&[seed, fi as u64, k]);
        let c = mesh.face_center(fi);
        let n = mesh.face_normal(fi);
        let away = try_norm(sub(c, centre)).unwrap_or(n);
        let jitter = [r(1) - 0.5, r(2) - 0.5, r(3) - 0.5];
        let dir = norm(add(away, scale(jitter, 0.6)));
        let speed = 0.5 + 0.5 * r(4);
        let travel = scale(dir, distance * progress * speed);
        let fall = [0.0, -gravity * progress * progress, 0.0];
        let axis = try_norm([r(5) - 0.5, r(6) - 0.5, r(7) - 0.5]).unwrap_or([0.0, 1.0, 0.0]);
        let turn = (spin * progress * (r(8) * 2.0 - 1.0)).to_radians();
        let base = out.positions.len() as u32;
        for &v in f {
            let p = sub(mesh.positions[v as usize], c);
            out.positions.push(add(add(add(c, rotate(p, axis, turn)), travel), fall));
        }
        let uv = mesh.uvs.as_ref().map(|_| mesh.corner_uvs(fi));
        out.add_face((0..f.len() as u32).map(|k| base + k).collect(), uv);
    }
    out
}

fn build(mut mesh: PolyMesh, m: &Modifier) -> PolyMesh {
    let progress = m.n("progress").clamp(0.0, 1.0);
    let n = mesh.faces.len();
    let seed = m.n("seed").round() as u64;
    let (lo, hi) = mesh.bounds();
    let centre = lerp(lo, hi, 0.5);
    let mut order: Vec<(f64, usize)> = (0..n)
        .map(|f| {
            let c = mesh.face_center(f);
            let k = match m.s("order").as_str() {
                "index" => f as f64,
                "x" => c[0],
                "y" => c[1],
                "z" => c[2],
                "distance" => dist(c, centre),
                _ => hash01(&[seed, f as u64]),
            };
            (k, f)
        })
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let shown = (progress * n as f64).floor() as usize;
    let mut keep = vec![false; n];
    let range = if m.b("reverse") { shown..n } else { 0..shown };
    for &(_, f) in &order[range] {
        keep[f] = true;
    }
    mesh.retain_faces(|f| keep[f]);
    mesh.compact();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen3_finds_the_axes() {
        // A symmetric matrix built from known axes and values comes apart again.
        let x = norm([1.0, 1.0, 0.0]);
        let z = norm(cross(x, norm([1.0, -1.0, 0.5])));
        let axes = [x, norm(cross(z, x)), z];
        let values = [3.0, 1.0, 0.25];
        let mut m = [[0.0; 3]; 3];
        for (a, l) in axes.iter().zip(values) {
            for (r, row) in m.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v += l * a[r] * a[c];
                }
            }
        }
        let (got, vecs) = eigen3(m);
        for (a, l) in axes.iter().zip(values) {
            let k = (0..3).min_by(|&i, &j| (got[i] - l).abs().total_cmp(&(got[j] - l).abs())).unwrap();
            assert!((got[k] - l).abs() < 1e-9, "{got:?}");
            assert!((dot(vecs[k], *a).abs() - 1.0).abs() < 1e-9, "axis {a:?} vs {:?}", vecs[k]);
        }
    }
}
