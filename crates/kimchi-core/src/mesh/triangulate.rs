//! Polygons → triangles: ear clipping (concave polygons and polygons with holes), and the
//! drawable [`TriMesh`] with normals smooth across gentle bends and split at sharp edges.

use std::collections::HashMap;

use super::math::*;
use super::{PolyMesh, TriMesh, box_project};

/// Triangles covering a simple polygon given as 2D points (either winding), as index triples
/// wound the same way as the polygon. Degenerate input gives fewer (or no) triangles; never
/// panics.
pub fn ear_clip(points: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let ring: Vec<usize> = (0..points.len()).collect();
    clip(points, ring)
}

/// Triangles covering a polygon with holes. Indices point into `outer` followed by every hole
/// in order. The triangles are counter-clockwise (positive area) whatever the input winding.
pub fn ear_clip_with_holes(outer: &[[f64; 2]], holes: &[Vec<[f64; 2]>]) -> Vec<[usize; 3]> {
    let mut pts: Vec<[f64; 2]> = outer.to_vec();
    let mut ring: Vec<usize> = dedupe_ring(&pts, (0..outer.len()).collect());
    if ring.len() < 3 {
        return vec![];
    }
    if area(&pts, &ring) < 0.0 {
        ring.reverse();
    }
    let mut hole_rings: Vec<Vec<usize>> = vec![];
    for h in holes {
        let base = pts.len();
        pts.extend_from_slice(h);
        let mut r = dedupe_ring(&pts, (base..base + h.len()).collect());
        if r.len() < 3 || area(&pts, &r).abs() < 1e-300 {
            continue;
        }
        if area(&pts, &r) > 0.0 {
            r.reverse();
        }
        hole_rings.push(r);
    }
    // Rightmost holes first, each joined to the outline by a bridge to a vertex it can see.
    hole_rings.sort_by(|a, b| {
        let ma = a.iter().map(|&i| pts[i][0]).fold(f64::NEG_INFINITY, f64::max);
        let mb = b.iter().map(|&i| pts[i][0]).fold(f64::NEG_INFINITY, f64::max);
        mb.total_cmp(&ma)
    });
    for hi in 0..hole_rings.len() {
        let hole = hole_rings[hi].clone();
        let mpos = (0..hole.len())
            .max_by(|&a, &b| pts[hole[a]][0].total_cmp(&pts[hole[b]][0]).then(pts[hole[b]][1].total_cmp(&pts[hole[a]][1])))
            .unwrap_or(0);
        let m = pts[hole[mpos]];
        let mut order: Vec<usize> = (0..ring.len()).collect();
        order.sort_by(|&a, &b| d2(pts[ring[a]], m).total_cmp(&d2(pts[ring[b]], m)));
        let others: Vec<&Vec<usize>> = hole_rings[hi..].iter().collect();
        let k = order
            .iter()
            .copied()
            .find(|&k| {
                let v = pts[ring[k]];
                let (a, b) = (pts[ring[(k + ring.len() - 1) % ring.len()]], pts[ring[(k + 1) % ring.len()]]);
                locally_inside(a, v, b, m) && !crosses_any(&pts, &ring, v, m) && others.iter().all(|h| !crosses_any(&pts, h, v, m))
            })
            .or_else(|| order.first().copied());
        let Some(k) = k else { continue };
        let mut spliced = Vec::with_capacity(ring.len() + hole.len() + 2);
        spliced.extend_from_slice(&ring[..=k]);
        for j in 0..=hole.len() {
            spliced.push(hole[(mpos + j) % hole.len()]);
        }
        spliced.push(ring[k]);
        spliced.extend_from_slice(&ring[k + 1..]);
        ring = spliced;
    }
    clip(&pts, ring)
}

/// Triangles of one 3D face (corner indices), clipped in the face's best-fit plane.
/// Picking and drawing share this tessellation, including concave faces and either winding.
pub fn face_triangles(pts: &[V3]) -> Vec<[usize; 3]> {
    if !pts.iter().copied().all(finite) {
        return vec![];
    }
    match pts.len() {
        0..=2 => vec![],
        3 => vec![[0, 1, 2]],
        _ => {
            // Face-local units keep its normal and ear tests stable for very small or large
            // coordinates, and avoid cancellation from a distant scene origin.
            let mut local: Vec<V3> = pts.iter().map(|p| sub(*p,pts[0])).collect();
            let extent = local.iter().flatten().map(|v|v.abs()).fold(0.,f64::max);
            if extent == 0. || !extent.is_finite() { return vec![]; }
            local.iter_mut().for_each(|p| *p=p.map(|v|v/extent));
            let pts=&local;
            let Some(n) = try_norm(newell(pts.iter().copied())) else { return vec![] };
            if pts.len() == 4 {
                // A convex quad splits along its shorter diagonal.
                let convex = (0..4).all(|k| dot(cross(sub(pts[(k + 1) % 4], pts[k]), sub(pts[(k + 2) % 4], pts[(k + 1) % 4])), n) > 0.0);
                if convex {
                    return if d3(pts[0], pts[2]) <= d3(pts[1], pts[3]) { vec![[0, 1, 2], [0, 2, 3]] } else { vec![[0, 1, 3], [1, 2, 3]] };
                }
            }
            let (u, v) = plane_basis(n);
            let flat: Vec<[f64; 2]> = pts.iter().map(|p| [dot(*p, u), dot(*p, v)]).collect();
            ear_clip(&flat)
        }
    }
}

fn d3(a: V3, b: V3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

fn d2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn same(a: [f64; 2], b: [f64; 2]) -> bool {
    a[0] == b[0] && a[1] == b[1]
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn area(pts: &[[f64; 2]], ring: &[usize]) -> f64 {
    let mut s = 0.0;
    for k in 0..ring.len() {
        let (a, b) = (pts[ring[k]], pts[ring[(k + 1) % ring.len()]]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    s / 2.0
}

fn dedupe_ring(pts: &[[f64; 2]], ring: Vec<usize>) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(ring.len());
    for i in ring {
        let p = pts[i];
        if !(p[0].is_finite() && p[1].is_finite()) {
            continue;
        }
        if out.last().is_some_and(|&l| same(pts[l], p)) {
            continue;
        }
        out.push(i);
    }
    while out.len() > 1 && same(pts[out[0]], pts[out[out.len() - 1]]) {
        out.pop();
    }
    out
}

/// Whether the diagonal from `v` towards `m` starts inside the polygon at `v` (neighbours
/// `a` before and `b` after, counter-clockwise).
fn locally_inside(a: [f64; 2], v: [f64; 2], b: [f64; 2], m: [f64; 2]) -> bool {
    if orient(a, v, b) >= 0.0 {
        // Convex corner: m must be left of both edges.
        orient(v, b, m) >= 0.0 && orient(a, v, m) >= 0.0
    } else {
        // Reflex corner: m must not be right of both.
        orient(v, b, m) >= 0.0 || orient(a, v, m) >= 0.0
    }
}

/// Whether the segment p–q properly crosses an edge of the ring (touching shared ends is fine).
fn crosses_any(pts: &[[f64; 2]], ring: &[usize], p: [f64; 2], q: [f64; 2]) -> bool {
    for k in 0..ring.len() {
        let (a, b) = (pts[ring[k]], pts[ring[(k + 1) % ring.len()]]);
        if same(a, p) || same(a, q) || same(b, p) || same(b, q) {
            continue;
        }
        let (o1, o2) = (orient(p, q, a), orient(p, q, b));
        let (o3, o4) = (orient(a, b, p), orient(a, b, q));
        if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
            return true;
        }
    }
    false
}

/// Ear clipping of one ring of point indices.
fn clip(pts: &[[f64; 2]], ring: Vec<usize>) -> Vec<[usize; 3]> {
    let mut ring = dedupe_ring(pts, ring);
    if ring.len() < 3 {
        return vec![];
    }
    let a = area(pts, &ring);
    if !a.is_finite() || a == 0.0 {
        // Zero area: a fan still covers it (all slivers), which keeps faces from vanishing.
        return (1..ring.len() - 1).map(|k| [ring[0], ring[k], ring[k + 1]]).collect();
    }
    let flipped = a < 0.0;
    if flipped {
        ring.reverse();
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for &i in &ring {
        for k in 0..2 {
            lo[k] = lo[k].min(pts[i][k]);
            hi[k] = hi[k].max(pts[i][k]);
        }
    }
    let eps = 1e-14 * ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2));
    let mut out = Vec::with_capacity(ring.len() - 2);
    let mut start = 0;
    while ring.len() > 3 {
        let n = ring.len();
        let ear = (0..n).map(|k| (start + k) % n).find(|&c| is_ear(pts, &ring, c, eps));
        let c = ear.unwrap_or_else(|| {
            // No clean ear (self-touching or degenerate outline): clip the most convex corner.
            (0..n)
                .max_by(|&x, &y| corner(pts, &ring, x).total_cmp(&corner(pts, &ring, y)))
                .unwrap_or(0)
        });
        let (p, q) = ((c + n - 1) % n, (c + 1) % n);
        out.push([ring[p], ring[c], ring[q]]);
        ring.remove(c);
        start = if c == 0 { 0 } else { c - 1 };
    }
    out.push([ring[0], ring[1], ring[2]]);
    if flipped {
        for t in &mut out {
            t.swap(1, 2);
        }
    }
    out
}

fn corner(pts: &[[f64; 2]], ring: &[usize], c: usize) -> f64 {
    let n = ring.len();
    orient(pts[ring[(c + n - 1) % n]], pts[ring[c]], pts[ring[(c + 1) % n]])
}

fn is_ear(pts: &[[f64; 2]], ring: &[usize], c: usize, eps: f64) -> bool {
    let n = ring.len();
    let (ip, iq) = ((c + n - 1) % n, (c + 1) % n);
    let (a, b, cc) = (pts[ring[ip]], pts[ring[c]], pts[ring[iq]]);
    if orient(a, b, cc) <= eps {
        return false;
    }
    for (k, &i) in ring.iter().enumerate() {
        if k == ip || k == c || k == iq {
            continue;
        }
        let p = pts[i];
        if same(p, a) || same(p, b) || same(p, cc) {
            continue;
        }
        // Inside or on the border of the candidate triangle: not an ear.
        if orient(a, b, p) >= -eps && orient(b, cc, p) >= -eps && orient(cc, a, p) >= -eps {
            return false;
        }
    }
    true
}

/// The drawable triangles of a polygon mesh (see [`PolyMesh::triangulate`]).
pub(crate) fn to_tris(mesh: &PolyMesh) -> TriMesh {
    let nv = mesh.positions.len();
    let valid = |f: &Vec<u32>| f.len() >= 3 && f.iter().all(|&v| (v as usize) < nv && finite(mesh.positions[v as usize]));
    // Face normals (None for faces that can't be drawn) and each corner's angle (its weight).
    // Corners weigh by their angle and their face's area, so a big flat face (a logo's front, a
    // tabletop) keeps its own normal next to the narrow strips of a bevel instead of being
    // tilted across its whole width.
    let mut normals: Vec<Option<V3>> = Vec::with_capacity(mesh.faces.len());
    let mut angles: Vec<Vec<f64>> = Vec::with_capacity(mesh.faces.len());
    for f in &mesh.faces {
        if !valid(f) {
            normals.push(None);
            angles.push(vec![]);
            continue;
        }
        let pts: Vec<V3> = f.iter().map(|&v| mesh.positions[v as usize]).collect();
        let nw = newell(pts.iter().copied());
        let area = len(nw) / 2.0;
        normals.push(try_norm(nw));
        let n = pts.len();
        angles.push(
            (0..n)
                .map(|k| {
                    let (a, b) = (norm(sub(pts[(k + n - 1) % n], pts[k])), norm(sub(pts[(k + 1) % n], pts[k])));
                    let c = dot(a, b).clamp(-1.0, 1.0).acos();
                    let w = if c.is_finite() { c.max(1e-6) } else { 1e-6 };
                    if area.is_finite() && area > 0.0 { w * area } else { w * 1e-12 }
                })
                .collect(),
        );
    }
    let smooth = mesh.smooth_angle;
    let cos_limit = if smooth.is_nan() || smooth <= 0.0 {
        f64::INFINITY
    } else if smooth >= 180.0 {
        f64::NEG_INFINITY
    } else {
        smooth.to_radians().cos()
    };
    let mut corners: Vec<Vec<(usize, usize)>> = vec![vec![]; nv];
    if cos_limit.is_finite() || cos_limit == f64::NEG_INFINITY {
        for (fi, f) in mesh.faces.iter().enumerate() {
            if normals[fi].is_none() {
                continue;
            }
            for (k, &v) in f.iter().enumerate() {
                corners[v as usize].push((fi, k));
            }
        }
    }
    let uvs = mesh.uvs.as_ref().filter(|u| u.len() == mesh.faces.len());
    let mut out = TriMesh::default();
    let mut index: HashMap<(u32, [u32; 3], [u32; 2]), u32> = HashMap::new();
    for (fi, f) in mesh.faces.iter().enumerate() {
        let Some(nf) = normals[fi] else { continue };
        let pts: Vec<V3> = f.iter().map(|&v| mesh.positions[v as usize]).collect();
        let tris = face_triangles(&pts);
        if tris.is_empty() {
            continue;
        }
        let given = uvs.and_then(|u| u.get(fi)).filter(|u| u.len() == f.len() && u.iter().all(|c| c[0].is_finite() && c[1].is_finite()));
        let mut ids = Vec::with_capacity(f.len());
        for (k, &v) in f.iter().enumerate() {
            let mut n = [0.0; 3];
            for &(g, c) in &corners[v as usize] {
                let Some(ng) = normals[g] else { continue };
                if g == fi || dot(ng, nf) > cos_limit {
                    n = mad(n, ng, angles[g][c]);
                }
            }
            let n = try_norm(n).unwrap_or(nf);
            let uv = match given {
                Some(u) => u[k],
                None => box_project(pts[k], nf),
            };
            let n32 = [n[0] as f32, n[1] as f32, n[2] as f32];
            let uv32 = [uv[0] as f32, uv[1] as f32];
            let key = (v, n32.map(f32::to_bits), uv32.map(f32::to_bits));
            let id = *index.entry(key).or_insert_with(|| {
                let p = pts[k];
                out.positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
                out.normals.push(n32);
                out.uvs.push(uv32);
                (out.positions.len() - 1) as u32
            });
            ids.push(id);
        }
        for t in tris {
            out.indices.extend([ids[t[0]], ids[t[1]], ids[t[2]]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_area(p: &[[f64; 2]], t: [usize; 3]) -> f64 {
        orient(p[t[0]], p[t[1]], p[t[2]]) / 2.0
    }

    #[test]
    fn clips_concave_polygons() {
        // An L shape (area 3) and a star.
        let l = [[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0], [1.0, 2.0], [0.0, 2.0]];
        let tris = ear_clip(&l);
        assert_eq!(tris.len(), 4);
        let total: f64 = tris.iter().map(|t| tri_area(&l, *t)).sum();
        assert!((total - 3.0).abs() < 1e-9);
        assert!(tris.iter().all(|t| tri_area(&l, *t) > 0.0), "same winding as the polygon");
        let star: Vec<[f64; 2]> = (0..10)
            .map(|k| {
                let a = k as f64 * std::f64::consts::PI / 5.0;
                let r = if k % 2 == 0 { 1.0 } else { 0.4 };
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        let tris = ear_clip(&star);
        assert_eq!(tris.len(), 8);
        assert!(tris.iter().all(|t| tri_area(&star, *t) > 0.0));
        // Clockwise input gives clockwise triangles.
        let cw: Vec<[f64; 2]> = l.iter().rev().copied().collect();
        assert!(ear_clip(&cw).iter().all(|t| tri_area(&cw, *t) < 0.0));
    }

    #[test]
    fn clips_polygons_with_holes() {
        let outer = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let holes = vec![vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]], vec![[2.5, 2.5], [3.5, 2.5], [3.5, 3.5], [2.5, 3.5]]];
        let tris = ear_clip_with_holes(&outer, &holes);
        let mut all = outer.to_vec();
        for h in &holes {
            all.extend_from_slice(h);
        }
        let total: f64 = tris.iter().map(|t| tri_area(&all, *t)).sum();
        assert!((total - 14.0).abs() < 1e-9, "16 minus two unit holes, got {total}");
        assert!(tris.iter().all(|t| tri_area(&all, *t) >= -1e-12));
        // No triangle covers a hole's centre.
        for c in [[1.5, 1.5], [3.0, 3.0]] {
            for t in &tris {
                let inside = orient(all[t[0]], all[t[1]], c) > 0.0 && orient(all[t[1]], all[t[2]], c) > 0.0 && orient(all[t[2]], all[t[0]], c) > 0.0;
                assert!(!inside);
            }
        }
    }

    #[test]
    fn degenerate_input_never_panics() {
        assert!(ear_clip(&[]).is_empty());
        assert!(ear_clip(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
        let _ = ear_clip(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]);
        let _ = ear_clip(&[[0.0, 0.0], [f64::NAN, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        // A bow tie (self-intersecting).
        let t = ear_clip(&[[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]]);
        assert!(t.len() <= 2);
        let m = PolyMesh::new(vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0]], vec![vec![0, 1, 2], vec![0, 1, 3], vec![0, 9, 1], vec![]]);
        let t = m.triangulate();
        assert!(t.positions.iter().flatten().all(|v| v.is_finite()));
        assert!(t.normals.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn normals_split_at_sharp_edges() {
        // A unit cube: smooth angle 30 gives 24 vertices with axis normals; 180 gives 8.
        let mut cube = super::super::shape_mesh(&crate::motion::Shape3d::Box { size: crate::motion::Vec3::one(), bevel: 0.0 }).unwrap();
        let t = cube.triangulate();
        assert_eq!(t.indices.len(), 36);
        assert_eq!(t.positions.len(), 24);
        for (p, n) in t.positions.iter().zip(&t.normals) {
            let axis = (0..3).max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs())).unwrap();
            assert!((n[axis].abs() - 1.0).abs() < 1e-6 && n[axis].signum() == p[axis].signum());
        }
        cube.smooth_angle = 180.0;
        cube.uvs = None;
        let t = cube.triangulate();
        assert!(t.normals.iter().all(|n| (n[0].abs() - n[1].abs()).abs() < 1e-6 && (n[1].abs() - n[2].abs()).abs() < 1e-6));
        // Triangles wind counter-clockwise from outside: (b - a) × (c - a) points along the normal.
        for tri in t.indices.chunks(3) {
            let p: Vec<V3> = tri.iter().map(|&i| t.positions[i as usize].map(|v| v as f64)).collect();
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let c = scale(add(add(p[0], p[1]), p[2]), 1.0 / 3.0);
            assert!(dot(n, c) > 0.0);
        }
    }
}
