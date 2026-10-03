//! Bevels: edges cut off flat (one segment) or rounded (several), and corners cut off.
//!
//! Edge bevel, for a set of edges: every face shrinks away from its bevelled edges by the width
//! (measured along the face, like Blender's "offset"); each bevelled edge becomes a strip whose
//! cross-section is a circular arc tangent to both faces; at each vertex the hole left between
//! strips is closed by a patch (a flat polygon, or a dome over a sphere fitted to its border).
//! Where a bevelled edge meets edges that aren't, the face corner slides along the unbevelled
//! edge, so smooth walls (cylinders, extrusions) stay connected.
//!
//! Limits: the width is clamped globally so no face edge is eaten from both ends (like Blender's
//! "clamp overlap"); vertices on an open border get no patch; where a bevelled edge simply stops
//! in the middle of a smooth surface the patch closes the gap but leaves a T-junction.

use std::collections::{HashMap, HashSet};

use super::math::*;
use super::{PolyMesh, uv_fit};

/// How a face corner moves.
#[derive(Clone, Copy)]
enum Corner {
    /// Not touched (no bevelled edge at this corner).
    Keep,
    /// Moved into the face by `offset` per unit of width (both edges bevelled).
    Miter(V3),
    /// Slid along the unbevelled edge towards `other`, `k` per unit of width.
    Slide { other: u32, k: f64 },
}

fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// The edges sharper than `angle` degrees (between exactly two faces).
pub(crate) fn sharp_edges(m: &PolyMesh, angle: f64) -> HashSet<(u32, u32)> {
    let normals: Vec<V3> = (0..m.faces.len()).map(|f| m.face_normal(f)).collect();
    let limit = angle.clamp(0.0, 180.0).to_radians().cos();
    m.edge_faces()
        .into_iter()
        .filter(|(_, f)| f.len() == 2 && dot(normals[f[0]], normals[f[1]]) < limit - 1e-9)
        .map(|(k, _)| k)
        .collect()
}

/// Bevels `edges` (vertex pairs, either order) by `width` with `segments` (1 = flat chamfer).
/// Returns the mesh and the indices of the faces made by the bevel.
pub(crate) fn bevel_edges(mesh: &PolyMesh, edges: &HashSet<(u32, u32)>, width: f64, segments: usize) -> (PolyMesh, Vec<usize>) {
    let mut m = mesh.clone();
    m.sanitize();
    let segments = segments.clamp(1, 64);
    let ef = m.edge_faces();
    let sharp: HashSet<(u32, u32)> = edges.iter().map(|&(a, b)| key(a, b)).filter(|k| ef.get(k).is_some_and(|f| f.len() == 2)).collect();
    if sharp.is_empty() || !width.is_finite() || width <= 0.0 {
        return (m, vec![]);
    }
    let normals: Vec<V3> = (0..m.faces.len()).map(|f| m.face_normal(f)).collect();
    // Each corner's move per unit width.
    let mut corners: Vec<Vec<Corner>> = Vec::with_capacity(m.faces.len());
    for (fi, f) in m.faces.iter().enumerate() {
        let n = f.len();
        let nf = normals[fi];
        corners.push(
            (0..n)
                .map(|k| {
                    let (p, v, q) = (f[(k + n - 1) % n], f[k], f[(k + 1) % n]);
                    let (sp, sn) = (sharp.contains(&key(p, v)), sharp.contains(&key(v, q)));
                    let pv = m.positions[v as usize];
                    let d_next = norm(sub(m.positions[q as usize], pv));
                    let d_prev = norm(sub(m.positions[p as usize], pv));
                    let l_next = cross(nf, d_next);
                    let l_prev = cross(nf, scale(d_prev, -1.0));
                    match (sp, sn) {
                        (false, false) => Corner::Keep,
                        (true, true) => {
                            let den = (1.0 + dot(l_prev, l_next)).max(0.05);
                            Corner::Miter(scale(add(l_prev, l_next), 1.0 / den))
                        }
                        (false, true) => {
                            let c = dot(d_prev, l_next);
                            Corner::Slide { other: p, k: if c > 0.1 { 1.0 / c } else { 1.0 } }
                        }
                        (true, false) => {
                            let c = dot(d_next, l_prev);
                            Corner::Slide { other: q, k: if c > 0.1 { 1.0 / c } else { 1.0 } }
                        }
                    }
                })
                .collect(),
        );
    }
    // Clamp the width so no face edge is used up from both ends.
    let mut w = width;
    for (fi, f) in m.faces.iter().enumerate() {
        let n = f.len();
        for k in 0..n {
            let (a, b) = (f[k], f[(k + 1) % n]);
            let (pa, pb) = (m.positions[a as usize], m.positions[b as usize]);
            let l = dist(pa, pb);
            let Some(dir) = try_norm(sub(pb, pa)) else { continue };
            let along = |c: Corner, from: u32, dir: V3| -> f64 {
                match c {
                    Corner::Keep => 0.0,
                    Corner::Miter(o) => dot(o, dir),
                    Corner::Slide { other, k } => dot(scale(norm(sub(m.positions[other as usize], m.positions[from as usize])), k), dir),
                }
            };
            let used = along(corners[fi][k], a, dir) + along(corners[fi][(k + 1) % n], b, scale(dir, -1.0));
            if used > 1e-9 {
                w = w.min(0.98 * l / used);
            }
        }
    }
    // Corner vertices.
    let mut slide_req: HashMap<(u32, u32), Vec<f64>> = HashMap::new();
    for (fi, f) in m.faces.iter().enumerate() {
        for (k, c) in corners[fi].iter().enumerate() {
            if let Corner::Slide { other, k: s } = *c {
                slide_req.entry((f[k], other)).or_default().push(s * w);
            }
        }
    }
    let mut slide_ids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut keys: Vec<&(u32, u32)> = slide_req.keys().collect();
    keys.sort();
    for &(v, other) in keys {
        let d = slide_req[&(v, other)].iter().sum::<f64>() / slide_req[&(v, other)].len() as f64;
        let (pv, po) = (m.positions[v as usize], m.positions[other as usize]);
        m.positions.push(mad(pv, norm(sub(po, pv)), d));
        slide_ids.insert((v, other), (m.positions.len() - 1) as u32);
    }
    let mut corner_ids: Vec<Vec<u32>> = vec![];
    for (fi, f) in m.faces.clone().iter().enumerate() {
        let mut ids = vec![];
        for (k, c) in corners[fi].iter().enumerate() {
            ids.push(match *c {
                Corner::Keep => f[k],
                Corner::Miter(o) => {
                    m.positions.push(mad(m.positions[f[k] as usize], o, w));
                    (m.positions.len() - 1) as u32
                }
                Corner::Slide { other, .. } => slide_ids[&(f[k], other)],
            });
        }
        corner_ids.push(ids);
    }
    let corner_of = |f: usize, v: u32, faces: &[Vec<u32>], ids: &[Vec<u32>]| -> u32 {
        let k = faces[f].iter().position(|&x| x == v).unwrap_or(0);
        ids[f][k]
    };
    // Profiles: for each bevelled edge (lo, hi) and each end, points from the corner in the face
    // walking lo → hi (f1) to the corner in the other face (f2).
    let dir_faces = m.directed_edges();
    let mut profiles: HashMap<((u32, u32), u32), Vec<u32>> = HashMap::new();
    let mut sorted: Vec<&(u32, u32)> = sharp.iter().collect();
    sorted.sort();
    let mut strips: Vec<((u32, u32), usize, usize)> = vec![];
    for &(lo, hi) in sorted {
        let (Some(&f1), Some(&f2)) = (dir_faces.get(&(lo, hi)), dir_faces.get(&(hi, lo))) else { continue };
        if f1 == f2 {
            continue;
        }
        let bend = dot(normals[f1], normals[f2]).clamp(-1.0, 1.0).acos();
        let weight = (bend / 2.0).cos();
        let (pa, pb) = (m.positions[lo as usize], m.positions[hi as usize]);
        for end in [lo, hi] {
            let c1 = corner_of(f1, end, &m.faces, &corner_ids);
            let c2 = corner_of(f2, end, &m.faces, &corner_ids);
            let (p1, p2) = (m.positions[c1 as usize], m.positions[c2 as usize]);
            let mid = lerp(p1, p2, 0.5);
            let e = sub(pb, pa);
            let t = if dot(e, e) > 0.0 { dot(sub(mid, pa), e) / dot(e, e) } else { 0.0 };
            let ctrl = mad(pa, e, t);
            let mut ids = vec![c1];
            for s in 1..segments {
                let t = s as f64 / segments as f64;
                let (a, b, c) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t) * weight, t * t);
                let p = scale(add(add(scale(p1, a), scale(ctrl, b)), scale(p2, c)), 1.0 / (a + b + c));
                m.positions.push(p);
                ids.push((m.positions.len() - 1) as u32);
            }
            ids.push(c2);
            profiles.insert(((lo, hi), end), ids);
        }
        strips.push(((lo, hi), f1, f2));
    }
    let has_uv = m.uvs.is_some();
    let old_faces = std::mem::take(&mut m.faces);
    let old_uvs = m.uvs.take();
    let mut out = PolyMesh { positions: m.positions.clone(), smooth_angle: m.smooth_angle, uvs: has_uv.then(Vec::new), faces: vec![] };
    // Shrunk faces, with coordinates carried over.
    for (fi, f) in old_faces.iter().enumerate() {
        let ids = corner_ids[fi].clone();
        let uv = old_uvs.as_ref().map(|u| {
            let pts: Vec<V3> = f.iter().map(|&i| m.positions[i as usize]).collect();
            ids.iter().map(|&v| uv_fit(&pts, &u[fi], m.positions[v as usize])).collect()
        });
        out.add_face(ids, uv);
    }
    let first_new = out.faces.len();
    for ((lo, hi), _, _) in &strips {
        let (pa, pb) = (&profiles[&((*lo, *hi), *lo)], &profiles[&((*lo, *hi), *hi)]);
        for k in 0..segments {
            out.add_face(vec![pb[k], pa[k], pa[k + 1], pb[k + 1]], None);
        }
    }
    // Patches around each vertex touching a bevelled edge.
    let mut touched: Vec<u32> = sharp.iter().flat_map(|&(a, b)| [a, b]).collect();
    touched.sort_unstable();
    touched.dedup();
    let vf = PolyMesh { positions: m.positions.clone(), faces: old_faces.clone(), uvs: None, smooth_angle: 0.0 }.vertex_faces();
    let mut dangling: Vec<usize> = vec![];
    for v in touched {
        let Some(ring) = walk_ring(&old_faces, &dir_faces, v, vf[v as usize].first().copied(), |fi, nxt| {
            let mut part = vec![corner_of(fi, v, &old_faces, &corner_ids)];
            let k = key(v, nxt);
            if let Some(p) = profiles.get(&(k, v)) {
                let forward = dir_faces.get(&(k.0, k.1)) == Some(&fi);
                let inner = &p[1..p.len() - 1];
                if forward {
                    part.extend_from_slice(inner);
                } else {
                    part.extend(inner.iter().rev());
                }
            }
            part
        }) else {
            continue;
        };
        let mut ring: Vec<u32> = ring.into_iter().rev().collect();
        dedupe_cyclic(&mut ring);
        let keeps: Vec<usize> = vf[v as usize].iter().copied().filter(|&f| corner_ids[f].iter().zip(&old_faces[f]).any(|(c, o)| *o == v && *c == v)).collect();
        if let Some(pos) = ring.iter().position(|&x| x == v) {
            if let [g] = keeps.as_slice() {
                // One untouched face at this corner (a single bevelled edge ending at a box
                // corner): the bevel's end notches that face instead of covering it.
                // The ring runs v → (along the edge before v in that face) → the bevel's end →
                // (along the edge after v): exactly what replaces v in the face.
                ring.rotate_left(pos);
                let tail: Vec<u32> = ring[1..].to_vec();
                let g = *g;
                let k = out.faces[g].iter().position(|&x| x == v).unwrap_or(0);
                let mut face = out.faces[g].clone();
                face.splice(k..=k, tail.iter().copied());
                let uv = old_uvs.as_ref().map(|u| {
                    let pts: Vec<V3> = old_faces[g].iter().map(|&i| out.positions[i as usize]).collect();
                    face.iter().map(|&i| uv_fit(&pts, &u[g], out.positions[i as usize])).collect()
                });
                out.faces[g] = face;
                if let (Some(all), Some(uv)) = (out.uvs.as_mut(), uv) {
                    all[g] = uv;
                }
                continue;
            }
            // Several untouched faces: they get the slid corners on their edges so the patch
            // joins them without cracks.
            dangling.extend(keeps.iter().copied());
        }
        let normal = norm(vf[v as usize].iter().fold([0.0; 3], |a, &f| add(a, normals[f])));
        fill_patch(&mut out, ring, m.positions[v as usize], normal, segments);
    }
    for g in dangling {
        let face = out.faces[g].clone();
        let n = face.len();
        let mut nf = Vec::with_capacity(n + 2);
        for k in 0..n {
            let (a, b) = (face[k], face[(k + 1) % n]);
            nf.push(a);
            nf.extend(slide_ids.get(&(a, b)));
            nf.extend(slide_ids.get(&(b, a)));
        }
        if nf.len() != n {
            let uv = old_uvs.as_ref().map(|u| {
                let pts: Vec<V3> = old_faces[g].iter().map(|&i| out.positions[i as usize]).collect();
                nf.iter().map(|&i| uv_fit(&pts, &u[g], out.positions[i as usize])).collect()
            });
            out.faces[g] = nf;
            if let (Some(all), Some(uv)) = (out.uvs.as_mut(), uv) {
                all[g] = uv;
            }
        }
    }
    let total = out.faces.len();
    let moved = out.sanitize();
    let made = (first_new..total).filter_map(|f| moved[f]).collect();
    out.compact();
    (out, made)
}

/// Walks the faces around `v` from face `start` (across the edge leaving `v` in each face),
/// collecting what `part(face, next vertex)` gives. `None` on an open border or a tangle.
fn walk_ring(faces: &[Vec<u32>], dir_faces: &HashMap<(u32, u32), usize>, v: u32, start: Option<usize>, mut part: impl FnMut(usize, u32) -> Vec<u32>) -> Option<Vec<u32>> {
    let start = start?;
    let mut out = vec![];
    let mut fi = start;
    for _ in 0..=faces.len().min(4096) {
        let f = &faces[fi];
        let k = f.iter().position(|&x| x == v)?;
        let nxt = f[(k + 1) % f.len()];
        out.extend(part(fi, nxt));
        let g = *dir_faces.get(&(nxt, v))?;
        if g == start {
            return Some(out);
        }
        fi = g;
    }
    None
}

fn dedupe_cyclic(ring: &mut Vec<u32>) {
    ring.dedup();
    while ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
}

/// Closes a hole bordered by `ring` (counter-clockwise from outside): one face, or with several
/// segments a dome over the sphere best fitting the border, centred along `normal` from `v`.
fn fill_patch(m: &mut PolyMesh, ring: Vec<u32>, v: V3, normal: V3, segments: usize) {
    if ring.len() < 3 {
        return;
    }
    if ring.len() == 3 || segments <= 1 {
        m.add_face(ring, None);
        return;
    }
    let pts: Vec<V3> = ring.iter().map(|&i| m.positions[i as usize]).collect();
    let g = scale(pts.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / pts.len() as f64);
    // Sphere centre on the line v − t·normal making the border points as equally far as can be.
    let a: Vec<f64> = pts.iter().map(|q| dot(sub(*q, v), sub(*q, v))).collect();
    let b: Vec<f64> = pts.iter().map(|q| dot(sub(*q, v), normal)).collect();
    let n = pts.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let cov: f64 = a.iter().zip(&b).map(|(x, y)| (x - ma) * (y - mb)).sum::<f64>() / n;
    let var: f64 = b.iter().map(|y| (y - mb).powi(2)).sum::<f64>() / n;
    let sphere = (var > 1e-18 && normal != [0.0; 3]).then(|| {
        let t = (-cov / (2.0 * var)).max(0.0);
        let c0 = mad(v, normal, -t);
        let r = pts.iter().map(|q| dist(*q, c0)).sum::<f64>() / n;
        (c0, r)
    });
    let project = |p: V3| match sphere {
        Some((c0, r)) => try_norm(sub(p, c0)).map_or(p, |d| mad(c0, d, r)),
        None => p,
    };
    let rows = (segments / 2).max(1);
    let mut prev = ring.clone();
    for j in 1..rows {
        let t = j as f64 / rows as f64;
        let row: Vec<u32> = pts
            .iter()
            .map(|p| {
                m.positions.push(project(lerp(*p, g, t)));
                (m.positions.len() - 1) as u32
            })
            .collect();
        for i in 0..row.len() {
            let i1 = (i + 1) % row.len();
            m.add_face(vec![prev[i], prev[i1], row[i1], row[i]], None);
        }
        prev = row;
    }
    m.positions.push(project(g));
    let c = (m.positions.len() - 1) as u32;
    for i in 0..prev.len() {
        m.add_face(vec![prev[i], prev[(i + 1) % prev.len()], c], None);
    }
}

/// Cuts off each vertex in `verts` by `width` along its edges (rounded with several segments).
/// Returns the mesh and the faces made at the corners.
pub(crate) fn bevel_vertices(mesh: &PolyMesh, verts: &HashSet<u32>, width: f64, segments: usize) -> (PolyMesh, Vec<usize>) {
    let mut m = mesh.clone();
    m.sanitize();
    if verts.is_empty() || !width.is_finite() || width <= 0.0 {
        return (m, vec![]);
    }
    let dir_faces = m.directed_edges();
    let mut cut: HashMap<(u32, u32), u32> = HashMap::new();
    let mut cut_point = |m: &mut PolyMesh, v: u32, u: u32| -> u32 {
        *cut.entry((v, u)).or_insert_with(|| {
            let (pv, pu) = (m.positions[v as usize], m.positions[u as usize]);
            let l = dist(pv, pu);
            let limit = if verts.contains(&u) { 0.45 * l } else { 0.9 * l };
            m.positions.push(mad(pv, norm(sub(pu, pv)), width.min(limit)));
            (m.positions.len() - 1) as u32
        })
    };
    let old = m.faces.clone();
    let normals: Vec<V3> = (0..old.len()).map(|f| m.face_normal(f)).collect();
    let mut faces = vec![];
    let mut uvs = vec![];
    for (fi, f) in old.iter().enumerate() {
        let n = f.len();
        let mut nf = vec![];
        for k in 0..n {
            let v = f[k];
            if verts.contains(&v) {
                let (p, q) = (f[(k + n - 1) % n], f[(k + 1) % n]);
                nf.push(cut_point(&mut m, v, p));
                nf.push(cut_point(&mut m, v, q));
            } else {
                nf.push(v);
            }
        }
        if let Some(u) = m.uvs.as_ref() {
            let pts: Vec<V3> = f.iter().map(|&i| m.positions[i as usize]).collect();
            uvs.push(nf.iter().map(|&i| uv_fit(&pts, &u[fi], m.positions[i as usize])).collect::<Vec<_>>());
        }
        faces.push(nf);
    }
    m.faces = faces;
    m.uvs = m.uvs.is_some().then_some(uvs);
    let first = m.faces.len();
    let mut sorted: Vec<u32> = verts.iter().copied().collect();
    sorted.sort_unstable();
    let vf = PolyMesh { positions: m.positions.clone(), faces: old.clone(), uvs: None, smooth_angle: 0.0 }.vertex_faces();
    for v in sorted {
        let Some(ring) = walk_ring(&old, &dir_faces, v, vf[v as usize].first().copied(), |_, nxt| cut.get(&(v, nxt)).map_or(vec![], |&c| vec![c])) else { continue };
        let mut ring: Vec<u32> = ring.into_iter().rev().collect();
        dedupe_cyclic(&mut ring);
        let normal = norm(vf[v as usize].iter().fold([0.0; 3], |a, &f| add(a, normals[f])));
        let at = m.positions[v as usize];
        fill_patch(&mut m, ring, at, normal, segments);
    }
    let total = m.faces.len();
    let moved = m.sanitize();
    let made = (first..total).filter_map(|f| moved[f]).collect();
    m.compact();
    (m, made)
}
