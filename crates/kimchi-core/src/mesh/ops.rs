//! Edit mode: operations on a selection of a polygon mesh, like Blender's (extrude, inset,
//! bevel, loop cut, bridge, spin, knife…), used by the `motion.editMesh` command and the
//! Studio. Each takes the mesh, a [`Selection`] and its parameters, changes the mesh, and gives
//! back the new selection (what Blender would leave selected: the extruded faces, the new
//! loop…), with indices valid in the changed mesh.
//!
//! A selection is vertex indices and/or face indices. Faces count as selected when listed, or
//! when all their vertices are; edges when both their ends are. [`Select`] describes one in JSON
//! (all, ids, faces facing a direction, inside a box, an edge loop or ring, grown to linked).
//! [`EDIT_OPS`] describes every operation and its parameters for the command, the guide and
//! the window, and [`apply`] runs one ([`EditOp`] in JSON: `{"type": "extrude", "distance": 1}`).

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::motion::stack::{Def, Family, ParamKind, ParamSpec, Stacked, TypeSpec};

use super::PolyMesh;
use super::bevel;
use super::math::*;
use super::modifiers;
use super::primitives::{mend_seam, sphere_uvs};
use super::triangulate::face_triangles;

/// Selected vertices and faces (indices into the mesh).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vertices: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub faces: Vec<u32>,
}

impl Selection {
    pub fn none() -> Selection {
        Selection::default()
    }

    /// Every vertex and face.
    pub fn all(m: &PolyMesh) -> Selection {
        Selection { vertices: (0..m.positions.len() as u32).collect(), faces: (0..m.faces.len() as u32).collect() }
    }

    pub fn of_vertices(v: impl IntoIterator<Item = u32>) -> Selection {
        Selection { vertices: v.into_iter().collect(), faces: vec![] }
    }

    pub fn of_faces(f: impl IntoIterator<Item = u32>) -> Selection {
        Selection { vertices: vec![], faces: f.into_iter().collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() && self.faces.is_empty()
    }

    /// The selected faces: those listed, or (when none are) those whose vertices are all selected.
    pub fn face_list(&self, m: &PolyMesh) -> Vec<usize> {
        if !self.faces.is_empty() {
            let mut f: Vec<usize> = self.faces.iter().map(|&f| f as usize).filter(|&f| f < m.faces.len()).collect();
            f.sort_unstable();
            f.dedup();
            return f;
        }
        let vs: HashSet<u32> = self.vertices.iter().copied().collect();
        if vs.is_empty() {
            return vec![];
        }
        (0..m.faces.len()).filter(|&f| m.faces[f].iter().all(|v| vs.contains(v))).collect()
    }

    /// The selected vertices: those listed and those of listed faces.
    pub fn vertex_set(&self, m: &PolyMesh) -> HashSet<u32> {
        let mut s: HashSet<u32> = self.vertices.iter().copied().filter(|&v| (v as usize) < m.positions.len()).collect();
        for &f in &self.faces {
            if let Some(face) = m.faces.get(f as usize) {
                s.extend(face.iter().copied());
            }
        }
        s
    }

    /// The selected edges (both ends selected), `a < b`, in a stable order.
    pub fn edge_list(&self, m: &PolyMesh) -> Vec<(u32, u32)> {
        let vs = self.vertex_set(m);
        m.edges().into_iter().filter(|e| vs.contains(&e.a) && vs.contains(&e.b)).map(|e| (e.a, e.b)).collect()
    }

    fn normalized(mut self) -> Selection {
        self.vertices.sort_unstable();
        self.vertices.dedup();
        self.faces.sort_unstable();
        self.faces.dedup();
        self
    }
}

// ---- selection helpers ----------------------------------------------------------------------

/// Faces whose normal is within `angle` degrees of `dir` (and their vertices).
pub fn select_facing(m: &PolyMesh, dir: [f64; 3], angle: f64) -> Selection {
    let Some(d) = try_norm(dir) else { return Selection::none() };
    let limit = angle.clamp(0.0, 180.0).to_radians().cos();
    let faces: Vec<u32> = (0..m.faces.len()).filter(|&f| dot(m.face_normal(f), d) >= limit - 1e-9).map(|f| f as u32).collect();
    with_vertices(m, faces)
}

/// Vertices inside the box `min`–`max`, and the faces whose vertices all are.
pub fn select_inside(m: &PolyMesh, min: [f64; 3], max: [f64; 3]) -> Selection {
    let inside = |p: &[f64; 3]| (0..3).all(|k| p[k] >= min[k].min(max[k]) && p[k] <= max[k].max(min[k]));
    let vertices: Vec<u32> = (0..m.positions.len() as u32).filter(|&v| inside(&m.positions[v as usize])).collect();
    let vs: HashSet<u32> = vertices.iter().copied().collect();
    let faces = (0..m.faces.len() as u32).filter(|&f| m.faces[f as usize].iter().all(|v| vs.contains(v))).collect();
    Selection { vertices, faces }
}

/// The faces using vertex `v` (and their vertices).
pub fn faces_of_vertex(m: &PolyMesh, v: u32) -> Selection {
    let faces = (0..m.faces.len() as u32).filter(|&f| m.faces[f as usize].contains(&v)).collect();
    with_vertices(m, faces)
}

/// Everything connected to the selection through shared edges (Blender's "select linked").
pub fn select_linked(m: &PolyMesh, sel: &Selection) -> Selection {
    let vf = m.vertex_faces();
    let seed = sel.vertex_set(m);
    let mut seen_f = vec![false; m.faces.len()];
    let mut queue: VecDeque<usize> = seed.iter().flat_map(|&v| vf[v as usize].iter().copied()).collect();
    for &f in &queue {
        seen_f[f] = true;
    }
    while let Some(f) = queue.pop_front() {
        for &v in &m.faces[f] {
            for &g in &vf[v as usize] {
                if !seen_f[g] {
                    seen_f[g] = true;
                    queue.push_back(g);
                }
            }
        }
    }
    let faces: Vec<u32> = (0..m.faces.len() as u32).filter(|&f| seen_f[f as usize]).collect();
    let mut s = with_vertices(m, faces);
    s.vertices.extend(seed);
    s.normalized()
}

fn with_vertices(m: &PolyMesh, faces: Vec<u32>) -> Selection {
    let mut vertices: Vec<u32> = faces.iter().flat_map(|&f| m.faces[f as usize].iter().copied()).collect();
    vertices.sort_unstable();
    vertices.dedup();
    Selection { vertices, faces }
}

/// The loop of edges through the edge `a`–`b`, continuing straight across vertices with four
/// edges (on quad meshes: a ring around a cylinder, a line across a grid). Its vertices.
pub fn edge_loop(m: &PolyMesh, a: u32, b: u32) -> Selection {
    let ef = m.edge_faces();
    let nb = m.neighbours();
    if !ef.contains_key(&(a.min(b), a.max(b))) {
        return Selection::none();
    }
    let mut verts = vec![a, b];
    let mut seen: HashSet<(u32, u32)> = HashSet::from([(a.min(b), a.max(b))]);
    for (from, to) in [(a, b), (b, a)] {
        let (mut p, mut v) = (from, to);
        while let Some(next) = loop_next(m, &ef, &nb, p, v) {
            let k = (v.min(next), v.max(next));
            if !seen.insert(k) {
                break;
            }
            verts.push(next);
            p = v;
            v = next;
        }
    }
    Selection::of_vertices(verts).normalized()
}

/// The edge after p→v in a loop: the one at `v` sharing no face with p–v (when `v` has four).
fn loop_next(m: &PolyMesh, ef: &HashMap<(u32, u32), Vec<usize>>, nb: &[Vec<u32>], p: u32, v: u32) -> Option<u32> {
    let around = &nb[v as usize];
    if around.len() != 4 {
        return None;
    }
    let faces = ef.get(&(p.min(v), p.max(v)))?;
    let mut options = around.iter().copied().filter(|&c| c != p && !faces.iter().any(|&f| m.faces[f].contains(&c)));
    let first = options.next()?;
    options.next().is_none().then_some(first)
}

/// The ring of edges across quads starting at the edge `a`–`b` (the edges a loop cut would
/// split), as pairs of vertices `(side, other side)` in order. Closed rings don't repeat.
pub fn edge_ring_edges(m: &PolyMesh, a: u32, b: u32) -> (Vec<(u32, u32)>, Vec<usize>) {
    let ef = m.edge_faces();
    let start = (a.min(b), a.max(b));
    let Some(start_faces) = ef.get(&start) else { return (vec![], vec![]) };
    // Walks across quads from the start edge into face f0: the exit edges and the faces crossed,
    // and whether it came back round to the start.
    let walk = |f0: usize| -> (Vec<(u32, u32)>, Vec<usize>, bool) {
        let (mut edges, mut faces) = (vec![], vec![]);
        let (mut s, mut e, mut f) = (a, b, f0);
        let mut visited: HashSet<usize> = HashSet::new();
        loop {
            let face = &m.faces[f];
            if face.len() != 4 || !visited.insert(f) {
                break;
            }
            let Some(k) = face.iter().position(|&x| x == s) else { break };
            // The opposite edge, keeping each end on the same side as before.
            let (s2, e2) = if face[(k + 1) % 4] == e {
                (face[(k + 3) % 4], face[(k + 2) % 4])
            } else if face[(k + 3) % 4] == e {
                (face[(k + 1) % 4], face[(k + 2) % 4])
            } else {
                break;
            };
            faces.push(f);
            let key = (s2.min(e2), s2.max(e2));
            if key == start {
                return (edges, faces, true);
            }
            edges.push((s2, e2));
            let Some(next) = ef.get(&key).filter(|fs| fs.len() == 2).and_then(|fs| fs.iter().copied().find(|&g| g != f)) else { break };
            (s, e, f) = (s2, e2, next);
        }
        (edges, faces, false)
    };
    let (fwd_edges, fwd_faces, closed) = walk(start_faces[0]);
    let mut edges = vec![];
    let mut faces = vec![];
    if !closed && start_faces.len() == 2 {
        let (back_edges, back_faces, _) = walk(start_faces[1]);
        edges.extend(back_edges.into_iter().rev());
        faces.extend(back_faces.into_iter().rev());
    }
    edges.push((a, b));
    edges.extend(fwd_edges);
    faces.extend(fwd_faces);
    (edges, faces)
}

/// The vertices of the ring of edges across quads through `a`–`b` (see [`edge_ring_edges`]).
pub fn edge_ring(m: &PolyMesh, a: u32, b: u32) -> Selection {
    let (edges, _) = edge_ring_edges(m, a, b);
    Selection::of_vertices(edges.iter().flat_map(|&(s, e)| [s, e])).normalized()
}

/// A selection described in JSON (fields combine: everything they pick is selected).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Select {
    /// Everything.
    pub all: bool,
    pub vertices: Vec<u32>,
    pub faces: Vec<u32>,
    /// Faces facing this direction…
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facing: Option<[f64; 3]>,
    /// …within this many degrees (default 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    /// Vertices (and faces) inside a box: `[[minX, minY, minZ], [maxX, maxY, maxZ]]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inside: Option<[[f64; 3]; 2]>,
    /// The edge loop through this edge (two vertex indices).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_loop: Option<[u32; 2]>,
    /// The edge ring through this edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_ring: Option<[u32; 2]>,
    /// Then grow to everything connected.
    pub linked: bool,
}

impl Select {
    /// The selection this picks in `m` (an error for indices that aren't in the mesh).
    pub fn resolve(&self, m: &PolyMesh) -> Result<Selection, String> {
        let nv = m.positions.len() as u32;
        let nf = m.faces.len() as u32;
        if let Some(v) = self.vertices.iter().find(|&&v| v >= nv) {
            return Err(format!("no vertex {v}: the mesh has {nv} (0–{})", nv.saturating_sub(1)));
        }
        if let Some(f) = self.faces.iter().find(|&&f| f >= nf) {
            return Err(format!("no face {f}: the mesh has {nf} (0–{})", nf.saturating_sub(1)));
        }
        let mut s = Selection { vertices: self.vertices.clone(), faces: self.faces.clone() };
        let mut add = |o: Selection| {
            s.vertices.extend(o.vertices);
            s.faces.extend(o.faces);
        };
        if self.all {
            add(Selection::all(m));
        }
        if let Some(d) = self.facing {
            add(select_facing(m, d, self.angle.unwrap_or(30.0)));
        }
        if let Some([lo, hi]) = self.inside {
            add(select_inside(m, lo, hi));
        }
        for (pair, ring) in [(self.edge_loop, false), (self.edge_ring, true)] {
            if let Some([a, b]) = pair {
                if a >= nv || b >= nv || !m.edge_faces().contains_key(&(a.min(b), a.max(b))) {
                    return Err(format!("{a}–{b} is not an edge of the mesh"));
                }
                add(if ring { edge_ring(m, a, b) } else { edge_loop(m, a, b) });
            }
        }
        let s = s.normalized();
        Ok(if self.linked { select_linked(m, &s) } else { s })
    }

    /// The selection for one operation: a loop cut across `edgeRing` cuts that ring (its
    /// vertices alone can't say which ring: on a box, they are every corner).
    pub fn resolve_for(&self, m: &PolyMesh, op: &str) -> Result<Selection, String> {
        if op == "loopCut"
            && let Some([a, b]) = self.edge_ring
            && self.vertices.is_empty()
            && self.faces.is_empty()
        {
            let nv = m.positions.len() as u32;
            if a >= nv || b >= nv || !m.edge_faces().contains_key(&(a.min(b), a.max(b))) {
                return Err(format!("{a}–{b} is not an edge of the mesh"));
            }
            return Ok(Selection::of_vertices([a, b]).normalized());
        }
        self.resolve(m)
    }
}

// ---- small helpers --------------------------------------------------------------------------

/// Cleans up after an op and renumbers what it selected.
fn tidy(m: &mut PolyMesh, verts: impl IntoIterator<Item = u32>, faces: impl IntoIterator<Item = usize>) -> Selection {
    let moved = m.sanitize();
    let map = m.compact();
    let faces = faces.into_iter().filter_map(|f| moved.get(f).copied().flatten()).map(|f| f as u32).collect();
    let vertices = verts.into_iter().filter_map(|v| map.get(v as usize).copied()).filter(|&v| v != u32::MAX).collect();
    Selection { vertices, faces }.normalized()
}

fn push(m: &mut PolyMesh, p: V3) -> u32 {
    m.positions.push(p);
    (m.positions.len() - 1) as u32
}

fn centre_of(m: &PolyMesh, vs: &HashSet<u32>) -> V3 {
    if vs.is_empty() {
        return [0.0; 3];
    }
    scale(vs.iter().fold([0.0; 3], |a, &v| add(a, m.positions[v as usize])), 1.0 / vs.len() as f64)
}

fn need_faces(m: &PolyMesh, sel: &Selection, what: &str) -> Result<Vec<usize>, String> {
    let f = sel.face_list(m);
    if f.is_empty() { Err(format!("{what} needs faces: select faces (or all the vertices of some)")) } else { Ok(f) }
}

/// The texture coordinate of vertex `v` in face `f` (when the face has it).
fn corner_uv(m: &PolyMesh, f: usize, v: u32) -> Option<[f64; 2]> {
    let k = m.faces.get(f)?.iter().position(|&x| x == v)?;
    m.uvs.as_ref().and_then(|u| u.get(f)).and_then(|u| u.get(k)).copied()
}

/// Area-weighted average normal of faces.
fn average_normal(m: &PolyMesh, faces: &[usize]) -> V3 {
    norm(faces.iter().fold([0.0; 3], |a, &f| mad(a, m.face_normal(f), m.face_area(f).max(1e-300))))
}

// ---- extrude and inset ----------------------------------------------------------------------

/// The part of region extrusion shared by extrude, inset and spin: vertices of `faces` that
/// touch the rest of the mesh (or an open border) are copied, the region's faces move onto the
/// copies, and walls join each border edge to its copy. Returns old → new vertex for copies.
fn split_region(m: &mut PolyMesh, faces: &[usize]) -> HashMap<u32, u32> {
    let fset: HashSet<usize> = faces.iter().copied().collect();
    let dir = m.directed_edges();
    let mut border: Vec<(u32, u32, usize)> = vec![];
    for &f in faces {
        let face = &m.faces[f];
        for k in 0..face.len() {
            let (a, b) = (face[k], face[(k + 1) % face.len()]);
            if dir.get(&(b, a)).is_none_or(|g| !fset.contains(g)) {
                border.push((a, b, f));
            }
        }
    }
    let mut outside: HashSet<u32> = HashSet::new();
    for (fi, face) in m.faces.iter().enumerate() {
        if !fset.contains(&fi) {
            outside.extend(face.iter().copied());
        }
    }
    let mut copies: HashMap<u32, u32> = HashMap::new();
    let mut needs: Vec<u32> = border.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    for &f in faces {
        needs.extend(m.faces[f].iter().copied().filter(|v| outside.contains(v)));
    }
    needs.sort_unstable();
    needs.dedup();
    for v in needs {
        let p = m.positions[v as usize];
        copies.insert(v, push(m, p));
    }
    for &f in faces {
        for v in m.faces[f].iter_mut() {
            if let Some(&c) = copies.get(v) {
                *v = c;
            }
        }
    }
    for (a, b, f) in border {
        // The region walks a→b (now on the copies), the face outside b→a: the wall walks
        // a→b along the old border and back along the copies.
        let (ca, cb) = (copies[&a], copies[&b]);
        let uv = m.uvs.as_ref().map(|_| {
            let (ua, ub) = (corner_uv(m, f, ca).unwrap_or([0.0; 2]), corner_uv(m, f, cb).unwrap_or([0.0; 2]));
            vec![ua, ub, ub, ua]
        });
        m.add_face(vec![a, b, cb, ca], uv);
    }
    copies
}

/// Extrudes the selected faces as one region along their average normal by `distance` (or by
/// `offset` when given). With no faces selected, extrudes the selected edges into new faces.
/// Selects the moved faces (or the new edge).
pub fn extrude(m: &mut PolyMesh, sel: &Selection, distance: f64, offset: Option<[f64; 3]>) -> Result<Selection, String> {
    let faces = sel.face_list(m);
    if faces.is_empty() {
        return extrude_edges(m, sel, distance, offset);
    }
    let shift = offset.unwrap_or_else(|| scale(average_normal(m, &faces), distance));
    split_region(m, &faces);
    let mut moved: HashSet<u32> = HashSet::new();
    for &f in &faces {
        moved.extend(m.faces[f].iter().copied());
    }
    for &v in &moved {
        m.positions[v as usize] = add(m.positions[v as usize], shift);
    }
    Ok(tidy(m, moved, faces))
}

fn extrude_edges(m: &mut PolyMesh, sel: &Selection, distance: f64, offset: Option<[f64; 3]>) -> Result<Selection, String> {
    let edges = sel.edge_list(m);
    if edges.is_empty() {
        return Err("extrude needs faces or edges: select faces, or two vertices sharing an edge".into());
    }
    let ef = m.edge_faces();
    let dir = m.directed_edges();
    let shift = offset.unwrap_or_else(|| {
        let n = edges.iter().flat_map(|e| ef.get(e).into_iter().flatten()).fold([0.0; 3], |a, &f| add(a, m.face_normal(f)));
        scale(norm(n), distance)
    });
    let mut copies: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &edges {
        for v in [a, b] {
            copies.entry(v).or_insert_with(|| {
                let p = add(m.positions[v as usize], shift);
                push(m, p)
            });
        }
    }
    for &(a, b) in &edges {
        // New faces run against the existing face along the edge.
        let (a, b) = if dir.contains_key(&(a, b)) { (a, b) } else { (b, a) };
        m.add_face(vec![b, a, copies[&a], copies[&b]], None);
    }
    let new: Vec<u32> = copies.values().copied().collect();
    Ok(tidy(m, new, []))
}

/// Extrudes each selected face on its own along its normal. Selects the moved faces.
pub fn extrude_individual(m: &mut PolyMesh, sel: &Selection, distance: f64) -> Result<Selection, String> {
    let faces = need_faces(m, sel, "extrude")?;
    let mut moved = vec![];
    for &f in &faces {
        let n = m.face_normal(f);
        split_region(m, &[f]);
        for k in 0..m.faces[f].len() {
            let v = m.faces[f][k];
            m.positions[v as usize] = mad(m.positions[v as usize], n, distance);
            moved.push(v);
        }
    }
    Ok(tidy(m, moved, faces))
}

/// A corner's move per unit inset inside face `f` (mitred between its two edges).
fn miter_in_face(m: &PolyMesh, f: usize, k: usize) -> V3 {
    let face = &m.faces[f];
    let n = face.len();
    let nf = m.face_normal(f);
    let (p, v, q) = (m.positions[face[(k + n - 1) % n] as usize], m.positions[face[k] as usize], m.positions[face[(k + 1) % n] as usize]);
    let l_prev = cross(nf, norm(sub(v, p)));
    let l_next = cross(nf, norm(sub(q, v)));
    scale(add(l_prev, l_next), 1.0 / (1.0 + dot(l_prev, l_next)).max(0.05))
}

/// Insets the selected faces by `thickness` (as one region, or each face on its own with
/// `individual`), then moves the inner faces `depth` along their normal. Selects the inner faces.
pub fn inset(m: &mut PolyMesh, sel: &Selection, thickness: f64, depth: f64, individual: bool) -> Result<Selection, String> {
    let faces = need_faces(m, sel, "inset")?;
    if individual {
        let mut moved = vec![];
        for &f in &faces {
            let n = m.face_normal(f);
            let len = m.faces[f].len();
            let dirs: Vec<V3> = (0..len).map(|k| miter_in_face(m, f, k)).collect();
            // Keep corners from crossing: no edge may shrink past zero.
            let mut t = thickness;
            for k in 0..len {
                let (a, b) = (m.positions[m.faces[f][k] as usize], m.positions[m.faces[f][(k + 1) % len] as usize]);
                let Some(e) = try_norm(sub(b, a)) else { continue };
                let used = dot(dirs[k], e) - dot(dirs[(k + 1) % len], e);
                if used > 1e-12 {
                    t = t.min(0.98 * dist(a, b) / used);
                }
            }
            let olds: Vec<u32> = m.faces[f].clone();
            split_region(m, &[f]);
            for k in 0..len {
                let v = m.faces[f][k];
                let p = m.positions[olds[k] as usize];
                m.positions[v as usize] = mad(mad(p, dirs[k], t), n, depth);
                moved.push(v);
            }
        }
        return Ok(tidy(m, moved, faces));
    }
    // Region: border corners move inwards along the surface; inner vertices only by depth.
    let fset: HashSet<usize> = faces.iter().copied().collect();
    let dir = m.directed_edges();
    let mut inward: HashMap<u32, Vec<V3>> = HashMap::new();
    let mut normals: HashMap<u32, V3> = HashMap::new();
    for &f in &faces {
        let face = m.faces[f].clone();
        let nf = m.face_normal(f);
        for k in 0..face.len() {
            let (a, b) = (face[k], face[(k + 1) % face.len()]);
            normals.entry(a).and_modify(|n| *n = add(*n, nf)).or_insert(nf);
            if dir.get(&(b, a)).is_none_or(|g| !fset.contains(g)) {
                let l = cross(nf, norm(sub(m.positions[b as usize], m.positions[a as usize])));
                inward.entry(a).or_default().push(l);
                inward.entry(b).or_default().push(l);
            }
        }
    }
    let originals: HashMap<u32, V3> = normals.keys().map(|&v| (v, m.positions[v as usize])).collect();
    let copies = split_region(m, &faces);
    let mut moved = vec![];
    for (&v, n) in &normals {
        let n = norm(*n);
        let target = copies.get(&v).copied().unwrap_or(v);
        let mut p = originals[&v];
        if let Some(ls) = inward.get(&v) {
            let sum = ls.iter().fold([0.0; 3], |a, l| add(a, *l));
            if let Some(d) = try_norm(sum) {
                let k = 1.0 / dot(d, ls[0]).max(0.25);
                p = mad(p, d, thickness * k);
            }
        }
        m.positions[target as usize] = mad(p, n, depth);
        moved.push(target);
    }
    Ok(tidy(m, moved, faces))
}

// ---- bevel, subdivide, loop cut -------------------------------------------------------------

/// Bevels the selected edges (or, with `vertices_only` or when no edge is selected, cuts off the
/// selected vertices) by `width` with `segments`. Selects the new faces.
pub fn bevel(m: &mut PolyMesh, sel: &Selection, width: f64, segments: usize, vertices_only: bool) -> Result<Selection, String> {
    let edges = sel.edge_list(m);
    let (out, made) = if vertices_only || edges.is_empty() {
        let vs = sel.vertex_set(m);
        if vs.is_empty() {
            return Err("bevel needs a selection: edges (both their vertices) or vertices".into());
        }
        bevel::bevel_vertices(m, &vs, width, segments)
    } else {
        bevel::bevel_edges(m, &edges.into_iter().collect(), width, segments)
    };
    *m = out;
    Ok(with_vertices(m, made.into_iter().map(|f| f as u32).collect()))
}

/// The vertices along edge a→b (new ones made by `cut_edges`), from a to b, ends excluded.
fn along(cuts: &HashMap<(u32, u32), Vec<u32>>, a: u32, b: u32) -> Vec<u32> {
    match cuts.get(&(a.min(b), a.max(b))) {
        Some(v) if a < b => v.clone(),
        Some(v) => v.iter().rev().copied().collect(),
        None => vec![],
    }
}

/// Puts the new edge vertices into every face using a cut edge (texture coordinates follow).
fn insert_cuts(m: &mut PolyMesh, cuts: &HashMap<(u32, u32), Vec<u32>>, skip: &HashSet<usize>) {
    for fi in 0..m.faces.len() {
        if skip.contains(&fi) {
            continue;
        }
        let face = m.faces[fi].clone();
        let n = face.len();
        if !(0..n).any(|k| cuts.contains_key(&(face[k].min(face[(k + 1) % n]), face[k].max(face[(k + 1) % n])))) {
            continue;
        }
        let uv = m.uvs.as_ref().map(|_| m.corner_uvs(fi));
        let mut nf = vec![];
        let mut nuv = vec![];
        for k in 0..n {
            let (a, b) = (face[k], face[(k + 1) % n]);
            nf.push(a);
            if let Some(u) = &uv {
                nuv.push(u[k]);
            }
            let mids = along(cuts, a, b);
            let (pa, pb) = (m.positions[a as usize], m.positions[b as usize]);
            for v in mids {
                nf.push(v);
                if let Some(u) = &uv {
                    let l = dist(pa, pb);
                    let t = if l > 0.0 { dist(pa, m.positions[v as usize]) / l } else { 0.5 };
                    let (ua, ub) = (u[k], u[(k + 1) % n]);
                    nuv.push([ua[0] + (ub[0] - ua[0]) * t, ua[1] + (ub[1] - ua[1]) * t]);
                }
            }
        }
        m.faces[fi] = nf;
        if let Some(u) = m.uvs.as_mut() {
            u[fi] = nuv;
        }
    }
}

/// Splits each selected face into a grid: quads into (cuts+1)², triangles into (cuts+1)²
/// triangles, other polygons into a fan around their centre. Neighbours get the new edge
/// vertices so nothing cracks. Selects the new faces.
pub fn subdivide(m: &mut PolyMesh, sel: &Selection, cuts: usize) -> Result<Selection, String> {
    let faces = need_faces(m, sel, "subdivide")?;
    let c = cuts.clamp(1, 100);
    let total: usize = faces.iter().map(|&f| (c + 1) * (c + 1) * m.faces[f].len()).sum();
    if total + m.faces.len() > super::MAX_FACES {
        return Err(format!("subdividing would make about {total} faces; fewer cuts or a smaller selection"));
    }
    let mut cuts_map: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &f in &faces {
        let face = m.faces[f].clone();
        for k in 0..face.len() {
            let (a, b) = (face[k], face[(k + 1) % face.len()]);
            let key = (a.min(b), a.max(b));
            if cuts_map.contains_key(&key) {
                continue;
            }
            let (pa, pb) = (m.positions[key.0 as usize], m.positions[key.1 as usize]);
            let ids = (1..=c).map(|i| push(m, lerp(pa, pb, i as f64 / (c + 1) as f64))).collect();
            cuts_map.insert(key, ids);
        }
    }
    let fset: HashSet<usize> = faces.iter().copied().collect();
    insert_cuts(m, &cuts_map, &fset);
    let src = m.clone();
    let n = c + 1;
    let mut made = vec![];
    for &f in &faces {
        let face = src.faces[f].clone();
        let pts: Vec<V3> = face.iter().map(|&v| src.positions[v as usize]).collect();
        let uv_of = |m: &PolyMesh, v: u32| -> Option<[f64; 2]> { m.uvs.as_ref().map(|_| src.uv_at(f, m.positions[v as usize])) };
        let mut new_faces: Vec<Vec<u32>> = vec![];
        if face.len() == 4 {
            let mut g = vec![vec![0u32; n + 1]; n + 1];
            let e = |k: usize| along(&cuts_map, face[k], face[(k + 1) % 4]);
            let (e0, e1, e2, e3) = (e(0), e(1), e(2), e(3));
            for i in 0..=n {
                for j in 0..=n {
                    g[i][j] = match (i, j) {
                        (0, 0) => face[0],
                        (_, 0) if i == n => face[1],
                        (_, _) if i == n && j == n => face[2],
                        (0, _) if j == n => face[3],
                        (_, 0) => e0[i - 1],
                        (_, _) if i == n => e1[j - 1],
                        (_, _) if j == n => e2[n - 1 - i],
                        (0, _) => e3[n - 1 - j],
                        _ => {
                            let (s, t) = (i as f64 / n as f64, j as f64 / n as f64);
                            let p = lerp(lerp(pts[0], pts[1], s), lerp(pts[3], pts[2], s), t);
                            push(m, p)
                        }
                    };
                }
            }
            for i in 0..n {
                for j in 0..n {
                    new_faces.push(vec![g[i][j], g[i + 1][j], g[i + 1][j + 1], g[i][j + 1]]);
                }
            }
        } else if face.len() == 3 {
            let e = |k: usize| along(&cuts_map, face[k], face[(k + 1) % 3]);
            let (e0, e1, e2) = (e(0), e(1), e(2));
            let mut g: HashMap<(usize, usize), u32> = HashMap::new();
            for i in 0..=n {
                for j in 0..=n - i {
                    let id = if i == 0 && j == 0 {
                        face[0]
                    } else if j == 0 && i == n {
                        face[1]
                    } else if i == 0 && j == n {
                        face[2]
                    } else if j == 0 {
                        e0[i - 1]
                    } else if i + j == n {
                        e1[j - 1]
                    } else if i == 0 {
                        e2[n - 1 - j]
                    } else {
                        let p = add(add(pts[0], scale(sub(pts[1], pts[0]), i as f64 / n as f64)), scale(sub(pts[2], pts[0]), j as f64 / n as f64));
                        push(m, p)
                    };
                    g.insert((i, j), id);
                }
            }
            for i in 0..n {
                for j in 0..n - i {
                    new_faces.push(vec![g[&(i, j)], g[&(i + 1, j)], g[&(i, j + 1)]]);
                    if i + j < n - 1 {
                        new_faces.push(vec![g[&(i + 1, j)], g[&(i + 1, j + 1)], g[&(i, j + 1)]]);
                    }
                }
            }
        } else {
            let centre = src.face_center(f);
            let cid = push(m, centre);
            let mut ring = vec![];
            for k in 0..face.len() {
                ring.push(face[k]);
                ring.extend(along(&cuts_map, face[k], face[(k + 1) % face.len()]));
            }
            for k in 0..ring.len() {
                new_faces.push(vec![ring[k], ring[(k + 1) % ring.len()], cid]);
            }
        }
        for (i, nf) in new_faces.into_iter().enumerate() {
            let uv = nf.iter().map(|&v| uv_of(m, v)).collect::<Option<Vec<_>>>();
            if i == 0 {
                m.faces[f] = nf;
                if let (Some(u), Some(uv)) = (m.uvs.as_mut(), uv) {
                    u[f] = uv;
                }
                made.push(f);
            } else {
                made.push(m.add_face(nf, uv));
            }
        }
    }
    let verts: Vec<u32> = made.iter().flat_map(|&f| m.faces[f].clone()).collect();
    Ok(tidy(m, verts, made))
}

/// Cuts loops across the ring of quads through the selected edge: `cuts` new edge loops,
/// shifted towards one side by `slide` (−1 to 1). Selects the new loop's vertices.
pub fn loop_cut(m: &mut PolyMesh, sel: &Selection, cuts: usize, slide: f64) -> Result<Selection, String> {
    let edges = sel.edge_list(m);
    let Some(&(a, b)) = edges.first() else {
        return Err("loop cut needs an edge: select two vertices sharing an edge".into());
    };
    let (ring, ring_faces) = edge_ring_edges(m, a, b);
    if ring_faces.is_empty() {
        return Err(format!("the edge {a}–{b} has no quads beside it to cut across"));
    }
    // Several selected edges must be one ring, or which one to cut is a guess.
    let in_ring: HashSet<(u32, u32)> = ring.iter().map(|&(s, e)| (s.min(e), s.max(e))).collect();
    if let Some(&(x, y)) = edges.iter().find(|&&(x, y)| !in_ring.contains(&(x.min(y), x.max(y)))) {
        return Err(format!(
            "a loop cut crosses one ring of edges, and the selection has edges of several ({a}–{b} and {x}–{y}): select the two vertices of one edge the cut should cross"
        ));
    }
    let c = cuts.clamp(1, 64);
    let slide = if slide.is_finite() { slide.clamp(-0.99, 0.99) } else { 0.0 };
    // New vertices on each ring edge, from its first side to its second.
    let mut on_edge: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    let mut made = vec![];
    for &(s, e) in &ring {
        let (ps, pe) = (m.positions[s as usize], m.positions[e as usize]);
        let ids: Vec<u32> = (1..=c)
            .map(|k| {
                let t = k as f64 / (c + 1) as f64 + slide / (c + 1) as f64;
                push(m, lerp(ps, pe, t))
            })
            .collect();
        made.extend(ids.iter().copied());
        let key = (s.min(e), s.max(e));
        on_edge.insert(key, if s < e { ids } else { ids.into_iter().rev().collect() });
    }
    let fset: HashSet<usize> = ring_faces.iter().copied().collect();
    let side_of: HashMap<(u32, u32), (u32, u32)> = ring.iter().map(|&(s, e)| ((s.min(e), s.max(e)), (s, e))).collect();
    let src = m.clone();
    for &f in &ring_faces {
        let face = src.faces[f].clone();
        // The two ring edges of this quad, each as (side, other side).
        let mut found: Vec<(u32, u32)> = vec![];
        for k in 0..4 {
            let key = (face[k].min(face[(k + 1) % 4]), face[k].max(face[(k + 1) % 4]));
            if let Some(&se) = side_of.get(&key) {
                found.push(se);
            }
        }
        if found.len() != 2 {
            continue;
        }
        let ((s0, e0), (s1, e1)) = (found[0], found[1]);
        let lines = |s: u32, e: u32| -> Vec<u32> {
            let mut l = vec![s];
            l.extend(along(&on_edge, s, e));
            l.push(e);
            l
        };
        let (l0, l1) = (lines(s0, e0), lines(s1, e1));
        let k0 = face.iter().position(|&x| x == s0).unwrap_or(0);
        let forward = face[(k0 + 1) % 4] == e0;
        let mut first = true;
        for k in 0..=c {
            let mut q = vec![l0[k], l0[k + 1], l1[k + 1], l1[k]];
            if !forward {
                q.reverse();
            }
            let uv = m.uvs.as_ref().map(|_| q.iter().map(|&v| src.uv_at(f, m.positions[v as usize])).collect());
            if first {
                m.faces[f] = q;
                if let (Some(u), Some(uv)) = (m.uvs.as_mut(), uv) {
                    u[f] = uv;
                }
                first = false;
            } else {
                m.add_face(q, uv);
            }
        }
    }
    insert_cuts(m, &on_edge, &fset);
    Ok(tidy(m, made, []))
}

// ---- delete, dissolve, merge ----------------------------------------------------------------

/// What `delete` and `dissolve` work on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Vertices,
    Edges,
    Faces,
}

impl Element {
    fn parse(s: &str) -> Element {
        match s {
            "vertices" => Element::Vertices,
            "edges" => Element::Edges,
            _ => Element::Faces,
        }
    }
}

/// Deletes the selected vertices (and the faces using them), edges (and their faces) or faces.
pub fn delete(m: &mut PolyMesh, sel: &Selection, what: Element) -> Result<Selection, String> {
    let kill: HashSet<usize> = match what {
        Element::Vertices => {
            let vs = sel.vertex_set(m);
            (0..m.faces.len()).filter(|&f| m.faces[f].iter().any(|v| vs.contains(v))).collect()
        }
        Element::Edges => {
            let es: HashSet<(u32, u32)> = sel.edge_list(m).into_iter().collect();
            (0..m.faces.len())
                .filter(|&f| {
                    let face = &m.faces[f];
                    (0..face.len()).any(|k| {
                        let (a, b) = (face[k], face[(k + 1) % face.len()]);
                        es.contains(&(a.min(b), a.max(b)))
                    })
                })
                .collect()
        }
        Element::Faces => sel.face_list(m).into_iter().collect(),
    };
    if kill.is_empty() {
        return Err("nothing selected to delete".into());
    }
    m.retain_faces(|f| !kill.contains(&f));
    Ok(tidy(m, [], []))
}

/// Removes edges or vertices while keeping the surface: faces either side of each dissolved
/// edge merge into one; a dissolved vertex disappears into one face made of its neighbours;
/// dissolving faces merges each connected group of them into one face. Selects the new faces.
pub fn dissolve(m: &mut PolyMesh, sel: &Selection, what: Element) -> Result<Selection, String> {
    let ef = m.edge_faces();
    let (edges, verts): (Vec<(u32, u32)>, HashSet<u32>) = match what {
        Element::Faces => {
            let fs: HashSet<usize> = sel.face_list(m).into_iter().collect();
            (ef.iter().filter(|(_, f)| f.len() == 2 && fs.contains(&f[0]) && fs.contains(&f[1])).map(|(k, _)| *k).collect(), HashSet::new())
        }
        Element::Edges => (sel.edge_list(m), HashSet::new()),
        Element::Vertices => {
            let vs = sel.vertex_set(m);
            (ef.keys().filter(|(a, b)| vs.contains(a) || vs.contains(b)).copied().collect(), vs)
        }
    };
    if edges.is_empty() && verts.is_empty() {
        return Err("nothing to dissolve in the selection".into());
    }
    // Groups of faces joined across dissolved edges.
    let mut parent: Vec<usize> = (0..m.faces.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for e in &edges {
        if let Some(f) = ef.get(e).filter(|f| f.len() == 2) {
            let (a, b) = (find(&mut parent, f[0]), find(&mut parent, f[1]));
            parent[a.max(b)] = a.min(b);
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for f in 0..m.faces.len() {
        let r = find(&mut parent, f);
        groups.entry(r).or_default().push(f);
    }
    let mut remove: HashSet<usize> = HashSet::new();
    let mut made: Vec<usize> = vec![];
    let mut roots: Vec<usize> = groups.keys().copied().collect();
    roots.sort_unstable();
    for r in roots {
        let group = &groups[&r];
        if group.len() < 2 {
            continue;
        }
        // The group's outline: its directed edges without those walked both ways inside it.
        let mut dirs: HashMap<(u32, u32), usize> = HashMap::new();
        for &f in group {
            let face = &m.faces[f];
            for k in 0..face.len() {
                dirs.insert((face[k], face[(k + 1) % face.len()]), f);
            }
        }
        let outline: Vec<(u32, u32)> = dirs.keys().filter(|(a, b)| !dirs.contains_key(&(*b, *a))).copied().collect();
        let mut next: HashMap<u32, u32> = HashMap::new();
        if outline.iter().any(|&(a, b)| next.insert(a, b).is_some()) || outline.len() < 3 {
            continue;
        }
        let start = outline.iter().map(|e| e.0).min().unwrap_or(0);
        let mut loop_ = vec![start];
        let mut cur = start;
        while let Some(&n) = next.get(&cur) {
            if n == start {
                break;
            }
            if loop_.len() > outline.len() {
                break;
            }
            loop_.push(n);
            cur = n;
        }
        if loop_.len() != outline.len() {
            continue;
        }
        let uv = m.uvs.as_ref().map(|_| loop_.iter().map(|&v| group.iter().find_map(|&f| corner_uv(m, f, v)).unwrap_or([0.0; 2])).collect::<Vec<_>>());
        let (face, uv) = drop_vertices(loop_, uv, &verts);
        if face.len() < 3 {
            continue;
        }
        remove.extend(group.iter().copied());
        made.push(m.add_face(face, uv));
    }
    // Dissolved vertices on borders: take them out of the faces they are still in.
    if !verts.is_empty() {
        for f in 0..m.faces.len() {
            if remove.contains(&f) || !m.faces[f].iter().any(|v| verts.contains(v)) {
                continue;
            }
            let uv = m.uvs.as_ref().map(|_| m.corner_uvs(f));
            let (face, uv) = drop_vertices(m.faces[f].clone(), uv, &verts);
            if face.len() >= 3 {
                m.faces[f] = face;
                if let (Some(u), Some(uv)) = (m.uvs.as_mut(), uv) {
                    u[f] = uv;
                }
            }
        }
    }
    let count = m.faces.len();
    let mut keep_map = vec![usize::MAX; count];
    let mut n = 0;
    for (f, slot) in keep_map.iter_mut().enumerate() {
        if !remove.contains(&f) {
            *slot = n;
            n += 1;
        }
    }
    m.retain_faces(|f| !remove.contains(&f));
    let made: Vec<usize> = made.into_iter().map(|f| keep_map[f]).filter(|&f| f != usize::MAX).collect();
    let verts: Vec<u32> = made.iter().flat_map(|&f| m.faces[f].clone()).collect();
    Ok(tidy(m, verts, made))
}

fn drop_vertices(face: Vec<u32>, uv: Option<Vec<[f64; 2]>>, gone: &HashSet<u32>) -> (Vec<u32>, Option<Vec<[f64; 2]>>) {
    let keep: Vec<bool> = face.iter().map(|v| !gone.contains(v)).collect();
    let f = face.iter().zip(&keep).filter(|(_, k)| **k).map(|(v, _)| *v).collect();
    let u = uv.map(|u| u.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(c, _)| c).collect());
    (f, u)
}

/// Where merged vertices end up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MergeAt {
    /// All into one, at their centre.
    Center,
    /// All into one, at the first selected vertex.
    First,
    /// Only those closer than this distance.
    Distance(f64),
}

/// Merges the selected vertices. Selects what is left of them.
pub fn merge(m: &mut PolyMesh, sel: &Selection, at: MergeAt) -> Result<Selection, String> {
    let mut list: Vec<u32> = sel.vertices.iter().copied().filter(|&v| (v as usize) < m.positions.len()).collect();
    let mut rest: Vec<u32> = sel.vertex_set(m).into_iter().filter(|v| !list.contains(v)).collect();
    rest.sort_unstable();
    list.extend(rest);
    if list.len() < 2 {
        return Err("merge needs at least two vertices".into());
    }
    let mut map: Vec<u32> = (0..m.positions.len() as u32).collect();
    let mut kept = vec![];
    match at {
        MergeAt::Center | MergeAt::First => {
            let target = if at == MergeAt::Center {
                scale(list.iter().fold([0.0; 3], |a, &v| add(a, m.positions[v as usize])), 1.0 / list.len() as f64)
            } else {
                m.positions[list[0] as usize]
            };
            let keep = list[0];
            m.positions[keep as usize] = target;
            for &v in &list {
                map[v as usize] = keep;
            }
            kept.push(keep);
        }
        MergeAt::Distance(d) => {
            let pts: Vec<V3> = list.iter().map(|&v| m.positions[v as usize]).collect();
            let wm = super::weld_map(&pts, d.max(0.0));
            for (i, &v) in list.iter().enumerate() {
                map[v as usize] = list[wm[i] as usize];
                kept.push(list[wm[i] as usize]);
            }
        }
    }
    m.remap(&map);
    Ok(tidy(m, kept, []))
}

// ---- fill and bridge ------------------------------------------------------------------------

/// Loops of open-border edges among `vs`, each in the direction a new face closing it walks.
fn border_loops(m: &PolyMesh, vs: &HashSet<u32>) -> Result<Vec<Vec<u32>>, String> {
    let ef = m.edge_faces();
    let dir = m.directed_edges();
    let mut next: HashMap<u32, u32> = HashMap::new();
    for (&(a, b), f) in &ef {
        if f.len() != 1 || !vs.contains(&a) || !vs.contains(&b) {
            continue;
        }
        // The existing face walks a→b; the closing face walks b→a.
        let (from, to) = if dir.contains_key(&(a, b)) { (b, a) } else { (a, b) };
        if next.insert(from, to).is_some() {
            return Err(format!("vertex {from} is on more than one open border; select one loop"));
        }
    }
    let mut loops = vec![];
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    let mut used: HashSet<u32> = HashSet::new();
    for s in starts {
        if used.contains(&s) {
            continue;
        }
        let mut l = vec![s];
        used.insert(s);
        let mut cur = s;
        loop {
            let Some(&n) = next.get(&cur) else { return Err("the selected border edges don't close into a loop".into()) };
            if n == s {
                break;
            }
            if !used.insert(n) {
                return Err("the selected border edges don't close into a loop".into());
            }
            l.push(n);
            cur = n;
        }
        loops.push(l);
    }
    Ok(loops)
}

/// Makes a face: closing a hole whose border vertices are selected, or through the selected
/// vertices in order around their centre. Selects it.
pub fn fill(m: &mut PolyMesh, sel: &Selection) -> Result<Selection, String> {
    let vs = sel.vertex_set(m);
    if vs.len() < 3 {
        return Err("fill needs three or more vertices".into());
    }
    // Fill closes holes and joins loose vertices; vertices deep inside a closed surface (every
    // edge round them already has two faces) have nothing open to fill.
    let open: HashSet<u32> = m.edges().iter().filter(|e| e.faces.len() < 2).flat_map(|e| [e.a, e.b]).collect();
    let loose: HashSet<u32> = (0..m.positions.len() as u32).filter(|v| !m.faces.iter().any(|f| f.contains(v))).collect();
    if vs.iter().all(|v| !open.contains(v) && !loose.contains(v)) {
        return Err("fill closes holes (open edges) and joins loose vertices, and the selection is inside a closed surface: there is nothing open to fill".into());
    }
    let loops = border_loops(m, &vs).unwrap_or_default();
    let face = match loops.as_slice() {
        [l] if l.len() == vs.len() => l.clone(),
        _ => {
            // Order the points around their centre in their best plane.
            let mut list: Vec<u32> = vs.iter().copied().collect();
            list.sort_unstable();
            let pts: Vec<V3> = list.iter().map(|&v| m.positions[v as usize]).collect();
            let c = scale(pts.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / pts.len() as f64);
            let mut best = [0.0; 3];
            for i in 0..pts.len().min(64) {
                for j in i + 1..pts.len().min(64) {
                    let n = cross(sub(pts[i], c), sub(pts[j], c));
                    if len(n) > len(best) {
                        best = n;
                    }
                }
            }
            let n = try_norm(best).ok_or("the selected vertices lie on a line: nothing to fill")?;
            let (u, v) = plane_basis(n);
            let mut order: Vec<(f64, u32)> = list.iter().zip(&pts).map(|(&id, p)| (dot(sub(*p, c), v).atan2(dot(sub(*p, c), u)), id)).collect();
            order.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut f: Vec<u32> = order.into_iter().map(|o| o.1).collect();
            // Face away from the rest of the mesh.
            let (lo, hi) = m.bounds();
            let mid = lerp(lo, hi, 0.5);
            let fnrm = norm(newell(f.iter().map(|&v| m.positions[v as usize])));
            if !m.faces.is_empty() && dot(fnrm, sub(c, mid)) < 0.0 {
                f.reverse();
            }
            f
        }
    };
    let mut key = face.clone();
    key.sort_unstable();
    if let Some(i) = m.faces.iter().position(|f| {
        let mut g = f.clone();
        g.sort_unstable();
        g == key
    }) {
        return Err(format!("those vertices already make face {i}"));
    }
    let verts = face.clone();
    let fi = m.add_face(face, None);
    Ok(tidy(m, verts, [fi]))
}

/// Joins two loops of open-border edges with a band of faces (deleting selected faces first,
/// so two facing faces become a tunnel). Selects the new faces.
pub fn bridge(m: &mut PolyMesh, sel: &Selection) -> Result<Selection, String> {
    let vs = sel.vertex_set(m);
    let faces = if sel.faces.is_empty() { vec![] } else { sel.face_list(m) };
    if !faces.is_empty() {
        let fs: HashSet<usize> = faces.iter().copied().collect();
        m.retain_faces(|f| !fs.contains(&f));
    }
    let loops = border_loops(m, &vs)?;
    let [la, lb] = loops.as_slice() else {
        return Err(format!("bridge needs two loops of open edges selected, found {}", loops.len()));
    };
    // Turn the second loop to run alongside the first, starting at the closest point.
    let b: Vec<u32> = lb.iter().rev().copied().collect();
    let (na, nb) = (la.len(), b.len());
    let pa = |i: usize| m.positions[la[i % na] as usize];
    let pb = |j: usize| m.positions[b[j % nb] as usize];
    let best = (0..nb)
        .min_by(|&x, &y| {
            let cost = |off: usize| (0..na).map(|i| dist(pa(i), pb(off + i * nb / na))).sum::<f64>();
            cost(x).total_cmp(&cost(y))
        })
        .unwrap_or(0);
    let mut made = vec![];
    if na == nb {
        for i in 0..na {
            let f = vec![la[i], la[(i + 1) % na], b[(best + i + 1) % nb], b[(best + i) % nb]];
            made.push(m.add_face(f, None));
        }
    } else {
        let (mut i, mut j) = (0usize, 0usize);
        while i < na || j < nb {
            let advance_a = j >= nb || (i < na && (i + 1) as f64 / na as f64 <= (j + 1) as f64 / nb as f64);
            let f = if advance_a {
                let f = vec![la[i % na], la[(i + 1) % na], b[(best + j) % nb]];
                i += 1;
                f
            } else {
                let f = vec![la[i % na], b[(best + j + 1) % nb], b[(best + j) % nb]];
                j += 1;
                f
            };
            made.push(m.add_face(f, None));
        }
    }
    let verts: Vec<u32> = made.iter().flat_map(|&f| m.faces[f].clone()).collect();
    Ok(tidy(m, verts, made))
}

// ---- normals --------------------------------------------------------------------------------

/// Turns the selected faces around.
pub fn flip(m: &mut PolyMesh, sel: &Selection) -> Result<Selection, String> {
    let faces = need_faces(m, sel, "flip")?;
    for &f in &faces {
        m.flip_face(f);
    }
    Ok(sel.clone().normalized())
}

/// Makes the selected faces (all when nothing is selected) agree with their neighbours and
/// point out of the solid they belong to.
pub fn recalc_normals(m: &mut PolyMesh, sel: &Selection) -> Result<Selection, String> {
    let faces: Vec<usize> = if sel.is_empty() { (0..m.faces.len()).collect() } else { sel.face_list(m) };
    let fset: HashSet<usize> = faces.iter().copied().collect();
    let ef = m.edge_faces();
    let mut done = vec![false; m.faces.len()];
    for &start in &faces {
        if done[start] {
            continue;
        }
        let mut component = vec![start];
        done[start] = true;
        let mut queue = VecDeque::from([start]);
        while let Some(f) = queue.pop_front() {
            let face = m.faces[f].clone();
            for k in 0..face.len() {
                let (a, b) = (face[k], face[(k + 1) % face.len()]);
                let Some(list) = ef.get(&(a.min(b), a.max(b))) else { continue };
                if list.len() != 2 {
                    continue;
                }
                let g = if list[0] == f { list[1] } else { list[0] };
                if done[g] || !fset.contains(&g) {
                    continue;
                }
                // g must walk the shared edge b→a.
                let gf = &m.faces[g];
                let walks_same = (0..gf.len()).any(|i| gf[i] == a && gf[(i + 1) % gf.len()] == b);
                if walks_same {
                    m.flip_face(g);
                }
                done[g] = true;
                component.push(g);
                queue.push_back(g);
            }
        }
        // Outwards: positive volume around the component's own centre.
        let vs: HashSet<u32> = component.iter().flat_map(|&f| m.faces[f].clone()).collect();
        let c = centre_of(m, &vs);
        let mut vol = 0.0;
        for &f in &component {
            let p: Vec<V3> = m.face_points(f).into_iter().map(|q| sub(q, c)).collect();
            for k in 1..p.len().saturating_sub(1) {
                vol += dot(p[0], cross(p[k], p[k + 1]));
            }
        }
        if vol < 0.0 {
            for &f in &component {
                m.flip_face(f);
            }
        }
    }
    Ok(sel.clone().normalized())
}

// ---- transforms -----------------------------------------------------------------------------

/// Moves the selected vertices.
pub fn translate(m: &mut PolyMesh, sel: &Selection, offset: [f64; 3]) -> Result<Selection, String> {
    transform(m, sel, None, |p, _| add(p, offset))
}

/// Turns the selection by Euler angles (degrees, x then y then z) around `pivot` (default its
/// centre).
pub fn rotate_sel(m: &mut PolyMesh, sel: &Selection, degrees: [f64; 3], pivot: Option<[f64; 3]>) -> Result<Selection, String> {
    let r = euler(degrees);
    transform(m, sel, pivot, |p, c| add(c, mat_mul(&r, sub(p, c))))
}

/// Scales the selection around `pivot` (default its centre); negative factors mirror it (and
/// turn its faces so they still face out).
pub fn scale_sel(m: &mut PolyMesh, sel: &Selection, factor: [f64; 3], pivot: Option<[f64; 3]>) -> Result<Selection, String> {
    let out = transform(m, sel, pivot, |p, c| {
        let d = sub(p, c);
        add(c, [d[0] * factor[0], d[1] * factor[1], d[2] * factor[2]])
    })?;
    if factor.iter().filter(|f| **f < 0.0).count() % 2 == 1 {
        for f in sel.face_list(m) {
            m.flip_face(f);
        }
    }
    Ok(out)
}

/// Mirrors the selection across the plane through `pivot` (default its centre) across `axis`.
pub fn mirror_sel(m: &mut PolyMesh, sel: &Selection, axis: usize, pivot: Option<[f64; 3]>) -> Result<Selection, String> {
    let mut f = [1.0; 3];
    f[axis.min(2)] = -1.0;
    scale_sel(m, sel, f, pivot)
}

fn transform(m: &mut PolyMesh, sel: &Selection, pivot: Option<[f64; 3]>, f: impl Fn(V3, V3) -> V3) -> Result<Selection, String> {
    let vs = sel.vertex_set(m);
    if vs.is_empty() {
        return Err("nothing selected to move".into());
    }
    let c = pivot.unwrap_or_else(|| centre_of(m, &vs));
    for &v in &vs {
        m.positions[v as usize] = f(m.positions[v as usize], c);
    }
    Ok(sel.clone().normalized())
}

/// Copies the selected faces (with new vertices), moved by `offset`. Selects the copy.
pub fn duplicate(m: &mut PolyMesh, sel: &Selection, offset: [f64; 3]) -> Result<Selection, String> {
    let faces = need_faces(m, sel, "duplicate")?;
    let mut copies: HashMap<u32, u32> = HashMap::new();
    let mut made = vec![];
    for &f in &faces {
        let face: Vec<u32> = m.faces[f]
            .clone()
            .into_iter()
            .map(|v| {
                *copies.entry(v).or_insert_with(|| {
                    let p = add(m.positions[v as usize], offset);
                    push(m, p)
                })
            })
            .collect();
        let uv = m.uvs.as_ref().map(|_| m.corner_uvs(f));
        made.push(m.add_face(face, uv));
    }
    let verts: Vec<u32> = copies.values().copied().collect();
    Ok(tidy(m, verts, made))
}

/// Splits the selected faces into triangles. Selects them.
pub fn triangulate(m: &mut PolyMesh, sel: &Selection) -> Result<Selection, String> {
    let faces = need_faces(m, sel, "triangulate")?;
    let mut made = vec![];
    for &f in &faces {
        let pts = m.face_points(f);
        let face = m.faces[f].clone();
        let uv = m.uvs.as_ref().map(|_| m.corner_uvs(f));
        let tris = face_triangles(&pts);
        if face.len() <= 3 || tris.is_empty() {
            made.push(f);
            continue;
        }
        for (i, t) in tris.iter().enumerate() {
            let nf: Vec<u32> = t.iter().map(|&k| face[k]).collect();
            let nuv = uv.as_ref().map(|u| t.iter().map(|&k| u[k]).collect::<Vec<_>>());
            if i == 0 {
                m.faces[f] = nf;
                if let (Some(all), Some(nuv)) = (m.uvs.as_mut(), nuv) {
                    all[f] = nuv;
                }
                made.push(f);
            } else {
                made.push(m.add_face(nf, nuv));
            }
        }
    }
    let verts: Vec<u32> = made.iter().flat_map(|&f| m.faces[f].clone()).collect();
    Ok(tidy(m, verts, made))
}

/// Adds a vertex in the middle of each selected face (raised `depth` along its normal) and fans
/// triangles to it. Selects the new middle vertices.
pub fn poke(m: &mut PolyMesh, sel: &Selection, depth: f64) -> Result<Selection, String> {
    let faces = need_faces(m, sel, "poke")?;
    let mut centres = vec![];
    for &f in &faces {
        let face = m.faces[f].clone();
        let c = mad(m.face_center(f), m.face_normal(f), depth);
        let uv = m.uvs.as_ref().map(|_| m.corner_uvs(f));
        let cuv = uv.as_ref().map(|u| {
            let s = u.iter().fold([0.0; 2], |a, c| [a[0] + c[0], a[1] + c[1]]);
            [s[0] / u.len() as f64, s[1] / u.len() as f64]
        });
        let cid = push(m, c);
        centres.push(cid);
        let n = face.len();
        for k in 0..n {
            let nf = vec![face[k], face[(k + 1) % n], cid];
            let nuv = uv.as_ref().zip(cuv).map(|(u, cu)| vec![u[k], u[(k + 1) % n], cu]);
            if k == 0 {
                m.faces[f] = nf;
                if let (Some(all), Some(nuv)) = (m.uvs.as_mut(), nuv) {
                    all[f] = nuv;
                }
            } else {
                m.add_face(nf, nuv);
            }
        }
    }
    Ok(tidy(m, centres, []))
}

/// Relaxes the selected vertices towards their neighbours.
pub fn smooth_vertices(m: &mut PolyMesh, sel: &Selection, factor: f64, iterations: usize) -> Result<Selection, String> {
    let vs = sel.vertex_set(m);
    if vs.is_empty() {
        return Err("nothing selected to smooth".into());
    }
    let out = modifiers::smooth(std::mem::take(m), factor, iterations, Some(&vs));
    *m = out;
    Ok(sel.clone().normalized())
}

// ---- spin -----------------------------------------------------------------------------------

/// Sweeps the selection around an axis through `center`, in `steps`: selected faces become a
/// solid (like a lathe), selected edges a surface. A full turn closes up. Selects the last step.
pub fn spin(m: &mut PolyMesh, sel: &Selection, angle: f64, steps: usize, axis: [f64; 3], center: [f64; 3]) -> Result<Selection, String> {
    let axis = try_norm(axis).ok_or("spin needs an axis direction")?;
    let steps = steps.clamp(1, 1024);
    let angle = if angle.is_finite() { angle.clamp(-3600.0, 3600.0) } else { 360.0 };
    let full = (angle.abs() - 360.0).abs() < 1e-9;
    let turn = |p: V3, k: usize| add(center, rotate(sub(p, center), axis, (angle * k as f64 / steps as f64).to_radians()));
    let faces = sel.face_list(m);
    let per_step: usize = faces.iter().map(|&f| m.faces[f].len()).sum::<usize>().max(sel.edge_list(m).len());
    if per_step * steps + m.faces.len() > super::MAX_FACES {
        return Err(format!("spinning {per_step} edges {steps} times would make too many faces; use fewer steps"));
    }
    if !faces.is_empty() {
        let originals: Vec<Vec<u32>> = faces.iter().map(|&f| m.faces[f].clone()).collect();
        // A closed region (a whole solid) has no border to sweep: it would only spin in place
        // and, all the way round, vanish.
        let mut uses: HashMap<(u32, u32), usize> = HashMap::new();
        for f in &originals {
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        if uses.values().all(|&n| n != 1) {
            return Err("spin sweeps the border of the selected faces (or selected edges, like a profile), and these faces close up with no border: select part of the surface, or the edges to turn".into());
        }
        let mut region_verts: Vec<u32> = originals.iter().flatten().copied().collect();
        region_verts.sort_unstable();
        region_verts.dedup();
        let start: HashMap<u32, V3> = region_verts.iter().map(|&v| (v, m.positions[v as usize])).collect();
        // Each step is an extrusion of the faces, turned a little further.
        let first = split_region(m, &faces);
        let mut current: HashMap<u32, u32> = region_verts.iter().map(|&v| (v, first.get(&v).copied().unwrap_or(v))).collect();
        place(m, &current, &start, 1, &turn);
        for k in 2..=steps {
            let copies = split_region(m, &faces);
            current = current.into_iter().map(|(o, c)| (o, copies.get(&c).copied().unwrap_or(c))).collect();
            place(m, &current, &start, k, &turn);
        }
        if full {
            // All the way round: the last ring welds onto the first border; the faces go.
            let mut remap: Vec<u32> = (0..m.positions.len() as u32).collect();
            for (o, c) in &current {
                if first.contains_key(o) {
                    remap[*c as usize] = *o;
                }
            }
            let fs: HashSet<usize> = faces.iter().copied().collect();
            m.retain_faces(|f| !fs.contains(&f));
            m.remap(&remap);
            return Ok(tidy(m, [], []));
        }
        // A cap stays where the faces started, on the border left behind.
        let mut fresh: HashMap<u32, u32> = HashMap::new();
        for (i, face) in originals.iter().enumerate() {
            let uv = m.uvs.as_ref().map(|_| m.corner_uvs(faces[i]));
            let cap: Vec<u32> = face
                .iter()
                .map(|&v| {
                    if first.contains_key(&v) {
                        v
                    } else {
                        *fresh.entry(v).or_insert_with(|| push(m, start[&v]))
                    }
                })
                .rev()
                .collect();
            m.add_face(cap, uv.map(|u| u.into_iter().rev().collect()));
        }
        let verts: Vec<u32> = current.values().copied().collect();
        return Ok(tidy(m, verts, faces));
    }
    let edges = sel.edge_list(m);
    if edges.is_empty() {
        return Err("spin needs faces or edges selected".into());
    }
    let dir = m.directed_edges();
    let verts: Vec<u32> = {
        let mut v: Vec<u32> = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let mut prev: HashMap<u32, u32> = verts.iter().map(|&v| (v, v)).collect();
    let last_step = if full { steps - 1 } else { steps };
    let mut made = vec![];
    for k in 1..=steps {
        let next: HashMap<u32, u32> = if k > last_step {
            verts.iter().map(|&v| (v, v)).collect()
        } else {
            verts.iter().map(|&v| (v, push(m, turn(m.positions[v as usize], k)))).collect()
        };
        for &(a, b) in &edges {
            let (a, b) = if dir.contains_key(&(a, b)) { (a, b) } else { (b, a) };
            made.push(m.add_face(vec![prev[&b], prev[&a], next[&a], next[&b]], None));
        }
        prev = next;
    }
    let last: Vec<u32> = prev.values().copied().collect();
    let _ = made;
    Ok(tidy(m, last, []))
}

fn place(m: &mut PolyMesh, current: &HashMap<u32, u32>, start: &HashMap<u32, V3>, k: usize, turn: &impl Fn(V3, usize) -> V3) {
    for (o, c) in current {
        m.positions[*c as usize] = turn(start[o], k);
    }
}

// ---- knife ----------------------------------------------------------------------------------

/// Part of a face being cut: its corners and their texture coordinates.
type Piece = (Vec<u32>, Option<Vec<[f64; 2]>>);

/// Cuts faces along the plane through `point` with normal `normal` (only `faces` when given):
/// edges crossing it get a new vertex (shared with neighbours), and cut faces split in two.
/// Returns the mesh and the vertices on the cut.
pub(crate) fn cut_plane(mesh: &PolyMesh, point: V3, normal: V3, faces: Option<&HashSet<usize>>) -> (PolyMesh, Vec<u32>) {
    let mut m = mesh.clone();
    let Some(n) = try_norm(normal) else { return (m, vec![]) };
    let eps = (m.size() * 1e-9).max(1e-12);
    let d: Vec<f64> = m.positions.iter().map(|p| dot(sub(*p, point), n)).collect();
    let side = |v: u32| -> i8 {
        let x = d[v as usize];
        if x > eps {
            1
        } else if x < -eps {
            -1
        } else {
            0
        }
    };
    let cutting = |f: usize| faces.is_none_or(|s| s.contains(&f));
    let mut cuts: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for fi in 0..m.faces.len() {
        if !cutting(fi) {
            continue;
        }
        let face = m.faces[fi].clone();
        for k in 0..face.len() {
            let (a, b) = (face[k], face[(k + 1) % face.len()]);
            let key = (a.min(b), a.max(b));
            if side(a) * side(b) < 0 && !cuts.contains_key(&key) {
                let (da, db) = (d[key.0 as usize], d[key.1 as usize]);
                let t = da / (da - db);
                let p = lerp(m.positions[key.0 as usize], m.positions[key.1 as usize], t);
                cuts.insert(key, vec![push(&mut m, p)]);
            }
        }
    }
    insert_cuts(&mut m, &cuts, &HashSet::new());
    let on: HashSet<u32> = (0..m.positions.len() as u32).filter(|&v| (v as usize) >= d.len() || side(v) == 0).collect();
    let mut made_on: Vec<u32> = cuts.values().flatten().copied().collect();
    let count = m.faces.len();
    for fi in 0..count {
        if !cutting(fi) {
            continue;
        }
        let face = m.faces[fi].clone();
        let sides: Vec<i8> = face.iter().map(|&v| if on.contains(&v) { 0 } else { side(v) }).collect();
        if !(sides.contains(&1) && sides.contains(&-1)) {
            continue;
        }
        // Chords between on-plane corners, in order along the cut line.
        let dir = cross(n, m.face_normal(fi));
        let mut ons: Vec<usize> = (0..face.len()).filter(|&k| sides[k] == 0).collect();
        ons.sort_by(|&x, &y| dot(m.positions[face[x] as usize], dir).total_cmp(&dot(m.positions[face[y] as usize], dir)));
        let uv = m.uvs.as_ref().map(|_| m.corner_uvs(fi));
        let mut pieces: Vec<Piece> = vec![(face.clone(), uv)];
        for pair in ons.chunks(2) {
            let [x, y] = pair else { continue };
            let (u, w) = (face[*x], face[*y]);
            let Some(pi) = pieces.iter().position(|(p, _)| {
                let (Some(i), Some(j)) = (p.iter().position(|&q| q == u), p.iter().position(|&q| q == w)) else { return false };
                let gap = i.abs_diff(j);
                gap > 1 && gap < p.len() - 1
            }) else {
                continue;
            };
            let (p, puv) = pieces.remove(pi);
            let (i, j) = (p.iter().position(|&q| q == u).unwrap_or(0), p.iter().position(|&q| q == w).unwrap_or(0));
            let (i, j) = (i.min(j), i.max(j));
            let a: Vec<u32> = p[i..=j].to_vec();
            let b: Vec<u32> = p[j..].iter().chain(&p[..=i]).copied().collect();
            let (ua, ub) = match &puv {
                Some(u) => (Some(u[i..=j].to_vec()), Some(u[j..].iter().chain(&u[..=i]).copied().collect())),
                None => (None, None),
            };
            pieces.push((a, ua));
            pieces.push((b, ub));
        }
        for (k, (p, puv)) in pieces.into_iter().enumerate() {
            if k == 0 {
                m.faces[fi] = p;
                if let (Some(all), Some(puv)) = (m.uvs.as_mut(), puv) {
                    all[fi] = puv;
                }
            } else {
                m.add_face(p, puv);
            }
        }
        made_on.extend(face.iter().copied().filter(|v| on.contains(v)));
    }
    made_on.sort_unstable();
    made_on.dedup();
    (m, made_on)
}

/// Cuts the selected faces (all when nothing is selected) along a plane. Selects the cut.
pub fn knife(m: &mut PolyMesh, sel: &Selection, point: [f64; 3], normal: [f64; 3]) -> Result<Selection, String> {
    if try_norm(normal).is_none() {
        return Err("knife needs a plane normal (a direction)".into());
    }
    let faces: Option<HashSet<usize>> = (!sel.is_empty()).then(|| sel.face_list(m).into_iter().collect());
    let (out, on) = cut_plane(m, point, normal, faces.as_ref());
    *m = out;
    Ok(tidy(m, on, []))
}

// ---- texture coordinates --------------------------------------------------------------------

/// How `unwrap` lays texture coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unwrap {
    /// Each face from the side of a cube it faces.
    Box,
    /// Around the y axis, like a label on a can.
    Cylinder,
    /// Longitude and latitude around the centre.
    Sphere,
    /// Flat along the faces' average direction, fitted to 0–1.
    Planar,
}

/// Lays new texture coordinates on the selected faces (all when nothing is selected).
pub fn unwrap(m: &mut PolyMesh, sel: &Selection, how: Unwrap) -> Result<Selection, String> {
    let faces: Vec<usize> = if sel.is_empty() { (0..m.faces.len()).collect() } else { sel.face_list(m) };
    if faces.is_empty() {
        return Err("nothing to unwrap".into());
    }
    if m.uvs.is_none() {
        m.uvs = Some((0..m.faces.len()).map(|f| m.box_uv(f)).collect());
    }
    let vs: HashSet<u32> = faces.iter().flat_map(|&f| m.faces[f].clone()).collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for &v in &vs {
        let p = m.positions[v as usize];
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let c = lerp(lo, hi, 0.5);
    let height = (hi[1] - lo[1]).max(1e-12);
    let plane = {
        let n = try_norm(faces.iter().fold([0.0; 3], |a, &f| mad(a, m.face_normal(f), m.face_area(f)))).unwrap_or([0.0, 0.0, 1.0]);
        // Keep "up" up when the faces stand upright.
        let up = if n[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, -1.0] };
        let u = norm(cross(up, n));
        let v = cross(n, u);
        let (mut ulo, mut uhi, mut vlo, mut vhi) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for &x in &vs {
            let p = m.positions[x as usize];
            ulo = ulo.min(dot(p, u));
            uhi = uhi.max(dot(p, u));
            vlo = vlo.min(dot(p, v));
            vhi = vhi.max(dot(p, v));
        }
        let span = (uhi - ulo).max(vhi - vlo).max(1e-12);
        (u, v, ulo, vhi, span)
    };
    for &f in &faces {
        let pts = m.face_points(f);
        let uv: Vec<[f64; 2]> = match how {
            Unwrap::Box => m.box_uv(f),
            Unwrap::Sphere => sphere_uvs(&pts.iter().map(|p| sub(*p, c)).collect::<Vec<_>>()),
            Unwrap::Cylinder => {
                let mut uv: Vec<[f64; 2]> = pts
                    .iter()
                    .map(|p| {
                        let u = (p[0] - c[0]).atan2(p[2] - c[2]) / std::f64::consts::TAU;
                        [if u < 0.0 { u + 1.0 } else { u }, (hi[1] - p[1]) / height]
                    })
                    .collect();
                let pole = pts.iter().map(|p| (p[0] - c[0]).hypot(p[2] - c[2]) < 1e-9 * height).collect();
                mend_seam(&mut uv, pole);
                uv
            }
            Unwrap::Planar => {
                let (u, v, ulo, vhi, span) = plane;
                pts.iter().map(|p| [(dot(*p, u) - ulo) / span, (vhi - dot(*p, v)) / span]).collect()
            }
        };
        if let Some(all) = m.uvs.as_mut() {
            all[f] = uv;
        }
    }
    Ok(sel.clone().normalized())
}

// ---- the table, for commands ----------------------------------------------------------------

const fn num(name: &'static str, label: &'static str, min: f64, max: f64, step: f64, default: f64, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Number { min, max, step }, default: Def::N(default), doc }
}
const fn int(name: &'static str, label: &'static str, min: f64, max: f64, default: f64, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Int { min, max }, default: Def::N(default), doc }
}
const fn flag(name: &'static str, label: &'static str, default: bool, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Bool, default: Def::B(default), doc }
}
const fn choice(name: &'static str, label: &'static str, options: &'static [&'static str], default: &'static str, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Choice(options), default: Def::S(default), doc }
}
const fn vec3(name: &'static str, label: &'static str, default: Def, doc: &'static str) -> ParamSpec {
    ParamSpec { name, label, kind: ParamKind::Vec3, default, doc }
}

const INF: f64 = 1e9;
const ELEMENTS: &[&str] = &["vertices", "edges", "faces"];

/// The edit operations, for `motion.editMesh` (see [`apply`]).
pub struct EditOpFamily;
impl Family for EditOpFamily {
    const FIELD: &'static str = "ops";
    const NOUN: &'static str = "edit";
    fn types() -> &'static [TypeSpec] {
        EDIT_OPS
    }
}
/// One edit operation with its parameters: `{"type": "inset", "thickness": 0.1}`.
pub type EditOp = Stacked<EditOpFamily>;

pub static EDIT_OPS: &[TypeSpec] = &[
    TypeSpec { name: "extrude", label: "Extrude", doc: "Pulls the selected faces out as one block along their average normal (or by an offset), with walls to where they were. With only edges selected, pulls the edges into new faces. Selects the moved faces.", params: &[
        num("distance", "Distance", -INF, INF, 0.01, 0.5, "How far along the normal (world units)."),
        vec3("offset", "Offset", Def::None, "Move by this instead of along the normal."),
    ]},
    TypeSpec { name: "extrudeIndividual", label: "Extrude each", doc: "Pulls each selected face out on its own along its own normal.", params: &[
        num("distance", "Distance", -INF, INF, 0.01, 0.5, "How far."),
    ]},
    TypeSpec { name: "inset", label: "Inset", doc: "Makes a smaller copy of the selected faces inside them, joined by a ring of faces. Selects the inner faces.", params: &[
        num("thickness", "Thickness", 0.0, INF, 0.01, 0.1, "How far the border moves in."),
        num("depth", "Depth", -INF, INF, 0.01, 0.0, "Then moves the inner faces along their normal (negative pushes in)."),
        flag("individual", "Each face", false, "Inset each face on its own instead of the selection as a whole."),
    ]},
    TypeSpec { name: "bevel", label: "Bevel", doc: "Cuts off (one segment) or rounds the selected edges; with `vertices`, the selected corners. Selects the new faces.", params: &[
        num("width", "Width", 0.0, INF, 0.01, 0.1, "How far the bevel reaches along the faces."),
        int("segments", "Segments", 1.0, 64.0, 1.0, "1 = flat; more = round."),
        flag("vertices", "Corners only", false, "Bevel the selected vertices instead of edges."),
    ]},
    TypeSpec { name: "subdivide", label: "Subdivide", doc: "Splits each selected face into a grid of smaller faces.", params: &[
        int("cuts", "Cuts", 1.0, 100.0, 1.0, "Cuts across each edge."),
    ]},
    TypeSpec { name: "loopCut", label: "Loop cut", doc: "Adds edge loops across the ring of quads through the selected edge (two selected vertices). Selects the new loop.", params: &[
        int("cuts", "Cuts", 1.0, 64.0, 1.0, "How many loops."),
        num("slide", "Slide", -1.0, 1.0, 0.01, 0.0, "Moves the loops towards one side (−1 to 1)."),
    ]},
    TypeSpec { name: "delete", label: "Delete", doc: "Deletes the selected vertices, edges or faces (and the faces that need them).", params: &[
        choice("what", "What", ELEMENTS, "faces", "What to delete."),
    ]},
    TypeSpec { name: "dissolve", label: "Dissolve", doc: "Removes the selected vertices, edges or faces but keeps the surface closed: neighbouring faces merge into one.", params: &[
        choice("what", "What", ELEMENTS, "faces", "What to dissolve."),
    ]},
    TypeSpec { name: "merge", label: "Merge", doc: "Merges the selected vertices into one (at their centre or at the first), or those closer than a distance.", params: &[
        choice("at", "At", &["center", "first", "distance"], "center", "Where they meet."),
        num("distance", "Distance", 0.0, INF, 0.0005, 0.001, "For `at: distance`."),
    ]},
    TypeSpec { name: "fill", label: "Fill", doc: "Makes a face from the selected vertices (closing a hole when they are its border).", params: &[] },
    TypeSpec { name: "bridge", label: "Bridge", doc: "Joins two selected loops of open edges with a band of faces (selected faces are deleted first, making a tunnel between them).", params: &[] },
    TypeSpec { name: "flip", label: "Flip normals", doc: "Turns the selected faces to face the other way.", params: &[] },
    TypeSpec { name: "recalcNormals", label: "Recalculate normals", doc: "Makes the selected faces (all if none) agree with each other and face outwards.", params: &[] },
    TypeSpec { name: "translate", label: "Move", doc: "Moves the selection.", params: &[
        vec3("offset", "Offset", Def::V3([0.0, 0.0, 0.0]), "World units."),
    ]},
    TypeSpec { name: "rotate", label: "Rotate", doc: "Turns the selection around a point.", params: &[
        vec3("angle", "Angle", Def::V3([0.0, 0.0, 0.0]), "Degrees around x, y, z."),
        vec3("pivot", "Pivot", Def::None, "The point it turns around (default the selection's centre)."),
    ]},
    TypeSpec { name: "scale", label: "Scale", doc: "Scales the selection around a point (negative mirrors).", params: &[
        vec3("factor", "Factor", Def::V3([1.0, 1.0, 1.0]), "Per axis, or one number."),
        vec3("pivot", "Pivot", Def::None, "Default the selection's centre."),
    ]},
    TypeSpec { name: "mirror", label: "Mirror", doc: "Mirrors the selection across an axis through a point.", params: &[
        choice("axis", "Axis", &["x", "y", "z"], "x", "Across this axis."),
        vec3("pivot", "Pivot", Def::None, "Default the selection's centre."),
    ]},
    TypeSpec { name: "duplicate", label: "Duplicate", doc: "Copies the selected faces, moved by an offset. Selects the copy.", params: &[
        vec3("offset", "Offset", Def::V3([0.0, 0.0, 0.0]), "World units."),
    ]},
    TypeSpec { name: "triangulate", label: "Triangulate", doc: "Splits the selected faces into triangles.", params: &[] },
    TypeSpec { name: "poke", label: "Poke", doc: "Adds a point in the middle of each selected face and fans triangles to it.", params: &[
        num("depth", "Depth", -INF, INF, 0.01, 0.0, "Raises the middle point along the face's normal."),
    ]},
    TypeSpec { name: "smooth", label: "Smooth", doc: "Relaxes the selected vertices towards their neighbours.", params: &[
        num("factor", "Factor", 0.0, 1.0, 0.01, 0.5, "How far each step moves."),
        int("iterations", "Repeat", 1.0, 100.0, 1.0, "Steps."),
    ]},
    TypeSpec { name: "spin", label: "Spin", doc: "Sweeps the selection around an axis (faces become a solid, edges a surface), like a lathe.", params: &[
        num("angle", "Angle", -3600.0, 3600.0, 1.0, 360.0, "Degrees swept."),
        int("steps", "Steps", 1.0, 1024.0, 12.0, "Copies along the sweep."),
        vec3("axis", "Axis", Def::V3([0.0, 1.0, 0.0]), "Direction of the axis."),
        vec3("center", "Centre", Def::V3([0.0, 0.0, 0.0]), "A point on the axis."),
    ]},
    TypeSpec { name: "knife", label: "Knife", doc: "Cuts the selected faces (all if none) along a plane. Selects the cut.", params: &[
        vec3("point", "Point", Def::V3([0.0, 0.0, 0.0]), "A point on the plane."),
        vec3("normal", "Normal", Def::V3([0.0, 1.0, 0.0]), "The plane's direction."),
    ]},
    TypeSpec { name: "unwrap", label: "Unwrap", doc: "Lays new texture coordinates on the selected faces (all if none).", params: &[
        choice("method", "Method", &["box", "cylinder", "sphere", "planar"], "box", "box: each side from a cube; cylinder: around y; sphere: longitude/latitude; planar: flat, fitted to 0–1."),
    ]},
];

/// Runs one edit operation on the selection; gives the new selection. Unknown types and bad
/// parameters are errors with "did you mean" hints.
pub fn apply(m: &mut PolyMesh, sel: &Selection, op: &EditOp) -> Result<Selection, String> {
    op.check(&mut vec![])?;
    let opt3 = |name: &str| op.params.contains_key(name).then(|| op.v3(name));
    let count = |name: &str| op.n(name).round().max(0.0) as usize;
    match op.kind.as_str() {
        "extrude" => extrude(m, sel, op.n("distance"), opt3("offset")),
        "extrudeIndividual" => extrude_individual(m, sel, op.n("distance")),
        "inset" => inset(m, sel, op.n("thickness"), op.n("depth"), op.b("individual")),
        "bevel" => bevel(m, sel, op.n("width"), count("segments"), op.b("vertices")),
        "subdivide" => subdivide(m, sel, count("cuts")),
        "loopCut" => loop_cut(m, sel, count("cuts"), op.n("slide")),
        "delete" => delete(m, sel, Element::parse(&op.s("what"))),
        "dissolve" => dissolve(m, sel, Element::parse(&op.s("what"))),
        "merge" => merge(
            m,
            sel,
            match op.s("at").as_str() {
                "first" => MergeAt::First,
                "distance" => MergeAt::Distance(op.n("distance")),
                _ => MergeAt::Center,
            },
        ),
        "fill" => fill(m, sel),
        "bridge" => bridge(m, sel),
        "flip" => flip(m, sel),
        "recalcNormals" => recalc_normals(m, sel),
        "translate" => translate(m, sel, op.v3("offset")),
        "rotate" => rotate_sel(m, sel, op.v3("angle"), opt3("pivot")),
        "scale" => scale_sel(m, sel, op.v3("factor"), opt3("pivot")),
        "mirror" => mirror_sel(m, sel, axis_index(&op.s("axis")), opt3("pivot")),
        "duplicate" => duplicate(m, sel, op.v3("offset")),
        "triangulate" => triangulate(m, sel),
        "poke" => poke(m, sel, op.n("depth")),
        "smooth" => smooth_vertices(m, sel, op.n("factor"), count("iterations")),
        "spin" => spin(m, sel, op.n("angle"), count("steps"), op.v3("axis"), op.v3("center")),
        "knife" => knife(m, sel, op.v3("point"), op.v3("normal")),
        "unwrap" => unwrap(
            m,
            sel,
            match op.s("method").as_str() {
                "cylinder" => Unwrap::Cylinder,
                "sphere" => Unwrap::Sphere,
                "planar" => Unwrap::Planar,
                _ => Unwrap::Box,
            },
        ),
        other => Err(format!("unknown edit `{other}`")),
    }
}
