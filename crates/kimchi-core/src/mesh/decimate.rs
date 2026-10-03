//! Fewer faces by edge collapse with quadric error metrics (Garland & Heckbert): the cheapest
//! edge to remove (the one that changes the shape least) goes first, again and again, until
//! the share of faces asked for is left. Open borders are kept in place, faces never flip.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::math::*;
use super::triangulate::face_triangles;
use super::PolyMesh;

/// A symmetric 4×4 matrix: xx xy xz xw yy yz yw zz zw ww.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: V3, d: f64, weight: f64) -> Quadric {
        let (a, b, c) = (n[0], n[1], n[2]);
        Quadric([a * a, a * b, a * c, a * d, b * b, b * c, b * d, c * c, c * d, d * d].map(|v| v * weight))
    }

    fn add(&mut self, o: &Quadric) {
        for k in 0..10 {
            self.0[k] += o.0[k];
        }
    }

    fn error(&self, p: V3) -> f64 {
        let q = &self.0;
        let (x, y, z) = (p[0], p[1], p[2]);
        q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y + 2.0 * q[5] * y * z + 2.0 * q[6] * y + q[7] * z * z + 2.0 * q[8] * z + q[9]
    }

    /// The point of least error, if the system isn't singular.
    fn optimum(&self) -> Option<V3> {
        let q = &self.0;
        let m = [[q[0], q[1], q[2]], [q[1], q[4], q[5]], [q[2], q[5], q[7]]];
        let r = [-q[3], -q[6], -q[8]];
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        let scale_ = q[0].abs() + q[4].abs() + q[7].abs();
        if det.abs() <= 1e-9 * scale_.powi(3).max(1e-300) {
            return None;
        }
        let solve = |col: usize| {
            let mut mm = m;
            for (row, v) in mm.iter_mut().zip(r) {
                row[col] = v;
            }
            mm[0][0] * (mm[1][1] * mm[2][2] - mm[1][2] * mm[2][1]) - mm[0][1] * (mm[1][0] * mm[2][2] - mm[1][2] * mm[2][0]) + mm[0][2] * (mm[1][0] * mm[2][1] - mm[1][1] * mm[2][0])
        };
        let p = [solve(0) / det, solve(1) / det, solve(2) / det];
        finite(p).then_some(p)
    }
}

struct Candidate {
    cost: f64,
    a: u32,
    b: u32,
    stamp: (u32, u32),
    target: V3,
}

impl PartialEq for Candidate {
    fn eq(&self, o: &Self) -> bool {
        self.cost == o.cost
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Candidate {
    fn cmp(&self, o: &Self) -> Ordering {
        // Cheapest first in a max-heap.
        o.cost.total_cmp(&self.cost)
    }
}

/// The vertices of a vertex's living triangles (itself included).
fn ring(vt: &[Vec<usize>], alive: &[bool], tris: &[[u32; 3]], v: usize) -> std::collections::HashSet<u32> {
    vt[v].iter().filter(|t| alive[**t]).flat_map(|t| tris[*t]).collect()
}

/// Keeps about `ratio` of the triangles (faces become triangles; texture coordinates are
/// dropped and made again by box projection).
pub fn decimate(mesh: &PolyMesh, ratio: f64) -> PolyMesh {
    let ratio = if ratio.is_finite() { ratio.clamp(0.0, 1.0) } else { 1.0 };
    if ratio >= 1.0 {
        return mesh.clone();
    }
    let mut pos = mesh.positions.clone();
    let mut tris: Vec<[u32; 3]> = vec![];
    for (fi, f) in mesh.faces.iter().enumerate() {
        let pts = mesh.face_points(fi);
        if pts.len() != f.len() {
            continue;
        }
        for t in face_triangles(&pts) {
            tris.push([f[t[0]], f[t[1]], f[t[2]]]);
        }
    }
    let target = (tris.len() as f64 * ratio).round() as usize;
    let empty = PolyMesh { smooth_angle: mesh.smooth_angle, ..Default::default() };
    if target == 0 {
        return empty;
    }
    let nv = pos.len();
    let mut vt: Vec<Vec<usize>> = vec![vec![]; nv];
    for (ti, t) in tris.iter().enumerate() {
        for &v in t {
            vt[v as usize].push(ti);
        }
    }
    let mut quad = vec![Quadric::default(); nv];
    let tri_normal = |pos: &[V3], t: &[u32; 3]| cross(sub(pos[t[1] as usize], pos[t[0] as usize]), sub(pos[t[2] as usize], pos[t[0] as usize]));
    for t in &tris {
        let raw = tri_normal(&pos, t);
        let Some(n) = try_norm(raw) else { continue };
        let area = len(raw) / 2.0;
        let q = Quadric::plane(n, -dot(n, pos[t[0] as usize]), area);
        for &v in t {
            quad[v as usize].add(&q);
        }
    }
    // Open borders: a steep wall of error across each border edge keeps it in place.
    let mut edge_count: std::collections::HashMap<(u32, u32), (usize, usize)> = std::collections::HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            edge_count.entry((a.min(b), a.max(b))).or_insert((0, ti)).0 += 1;
        }
    }
    for (&(a, b), &(count, ti)) in &edge_count {
        if count != 1 {
            continue;
        }
        let Some(fnrm) = try_norm(tri_normal(&pos, &tris[ti])) else { continue };
        let e = sub(pos[b as usize], pos[a as usize]);
        let Some(n) = try_norm(cross(e, fnrm)) else { continue };
        let q = Quadric::plane(n, -dot(n, pos[a as usize]), dot(e, e) * 1000.0);
        quad[a as usize].add(&q);
        quad[b as usize].add(&q);
    }
    let mut alive = vec![true; tris.len()];
    let mut alive_count = tris.len();
    let mut stamp = vec![0u32; nv];
    let mut heap = BinaryHeap::new();
    let plan = |a: u32, b: u32, pos: &[V3], quad: &[Quadric], stamp: &[u32]| -> Candidate {
        let mut q = quad[a as usize];
        q.add(&quad[b as usize]);
        let (pa, pb) = (pos[a as usize], pos[b as usize]);
        let mid = lerp(pa, pb, 0.5);
        let reach = dist(pa, pb) * 2.0;
        let mut best = (q.error(mid), mid);
        for p in [pa, pb] {
            let e = q.error(p);
            if e < best.0 {
                best = (e, p);
            }
        }
        if let Some(p) = q.optimum().filter(|p| dist(*p, mid) <= reach) {
            let e = q.error(p);
            if e < best.0 {
                best = (e, p);
            }
        }
        Candidate { cost: best.0.max(0.0), a, b, stamp: (stamp[a as usize], stamp[b as usize]), target: best.1 }
    };
    for &(a, b) in edge_count.keys() {
        heap.push(plan(a, b, &pos, &quad, &stamp));
    }
    while alive_count > target {
        let Some(c) = heap.pop() else { break };
        let (a, b) = (c.a as usize, c.b as usize);
        if stamp[a] != c.stamp.0 || stamp[b] != c.stamp.1 || vt[a].is_empty() || vt[b].is_empty() {
            continue;
        }
        // Topology: the edge's two ends may share only the neighbours of the faces across it.
        let shared: Vec<usize> = vt[a].iter().copied().filter(|t| alive[*t] && tris[*t].contains(&c.b)).collect();
        if shared.is_empty() {
            continue;
        }
        let (ra, rb) = (ring(&vt, &alive, &tris, a), ring(&vt, &alive, &tris, b));
        let common = ra.iter().filter(|v| **v != c.a && **v != c.b && rb.contains(v)).count();
        if common != shared.len() {
            continue;
        }
        // Geometry: no remaining face may turn over.
        let flips = [a, b].iter().any(|&v| {
            vt[v].iter().filter(|t| alive[**t] && !shared.contains(t)).any(|&t| {
                let before = tri_normal(&pos, &tris[t]);
                let mut moved = tris[t];
                let mut p2 = [pos[moved[0] as usize], pos[moved[1] as usize], pos[moved[2] as usize]];
                for k in 0..3 {
                    if moved[k] as usize == v {
                        p2[k] = c.target;
                        moved[k] = c.a;
                    }
                }
                let after = cross(sub(p2[1], p2[0]), sub(p2[2], p2[0]));
                dot(before, after) <= 0.2 * len(before) * len(after)
            })
        });
        if flips {
            continue;
        }
        pos[a] = c.target;
        let qb = quad[b];
        quad[a].add(&qb);
        for &t in &shared {
            alive[t] = false;
            alive_count -= 1;
        }
        let moved: Vec<usize> = std::mem::take(&mut vt[b]);
        for t in moved {
            if !alive[t] {
                continue;
            }
            for v in tris[t].iter_mut() {
                if *v == c.b {
                    *v = c.a;
                }
            }
            vt[a].push(t);
        }
        vt[a].retain(|t| alive[*t]);
        vt[a].sort_unstable();
        vt[a].dedup();
        stamp[a] += 1;
        stamp[b] += 1;
        let ns: Vec<u32> = ring(&vt, &alive, &tris, a).into_iter().filter(|&v| v != c.a).collect();
        for nb in ns {
            heap.push(plan(c.a, nb, &pos, &quad, &stamp));
        }
    }
    let mut out = PolyMesh { positions: pos, smooth_angle: mesh.smooth_angle, ..Default::default() };
    out.faces = tris.iter().zip(&alive).filter(|(_, a)| **a).map(|(t, _)| t.to_vec()).collect();
    out.sanitize();
    out.compact();
    out
}
