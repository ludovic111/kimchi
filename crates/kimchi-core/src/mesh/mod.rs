//! Polygon meshes: the shapes of 3D objects as faces you can edit (extrude, inset, bevel…) and
//! the modifier stack that turns a shape into what is drawn (subdivision, mirror, array…).
//!
//! - [`shape_mesh`] builds the polygons of a primitive (box, sphere, extruded path, lathe, tube…).
//! - [`apply_modifiers`] runs an object's modifier stack over them at a scene time.
//! - [`PolyMesh::triangulate`] makes triangles with normals and texture coordinates to draw.
//! - [`ops`] holds the edit-mode operations (extrude, inset, loop cut, bridge…) used by
//!   `motion.editMesh` and the Studio, with selections and picking helpers.
//!
//! Faces are counter-clockwise seen from outside, in a right-handed space with y up. Nothing here
//! panics on odd input: scenes come from people and AI agents, so bad indices, NaNs, empty
//! profiles and silly sizes give empty or partial meshes instead.

pub(crate) mod math;

mod bevel;
mod csg;
mod decimate;
mod extrude;
mod modifiers;
mod noise;
pub mod ops;
mod primitives;
mod triangulate;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::motion::Shape3d;
use crate::motion::stack::Modifier;

use self::math::*;

pub use self::triangulate::{ear_clip, ear_clip_with_holes};

/// The most faces a modifier may make; past it the step is skipped (and logged) so a scene
/// can't freeze the app with, say, subdivision level 4 on a million faces.
pub const MAX_FACES: usize = 2_000_000;

/// The smoothing angle primitives get when nothing else is said (degrees).
pub const DEFAULT_SMOOTH_ANGLE: f64 = 30.0;

/// Faces with any number of corners over shared vertices, counter-clockwise seen from outside.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolyMesh {
    pub positions: Vec<[f64; 3]>,
    pub faces: Vec<Vec<u32>>,
    /// Texture coordinates per face corner (same shape as `faces`), when known.
    pub uvs: Option<Vec<Vec<[f64; 2]>>>,
    /// Edges bent more than this (degrees) are drawn sharp.
    pub smooth_angle: f64,
}

/// Triangles ready to draw: one normal and texture coordinate per vertex.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// One edge of a mesh (`a < b`) and the faces that use it (one on an open border, two inside a
/// closed surface, more where the mesh isn't manifold).
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub faces: Vec<usize>,
}

/// The polygon mesh of a shape (`None` for shapes made elsewhere: text, models, pictures,
/// particles, groups, and curves without a radius, which are invisible paths).
pub fn shape_mesh(shape: &Shape3d) -> Option<PolyMesh> {
    primitives::build(shape)
}

/// `mesh` through `modifiers` at scene time `t`. `object(id)` gives another object's mesh in
/// this object's own space (for booleans).
pub fn apply_modifiers(mesh: PolyMesh, modifiers: &[Modifier], t: f64, object: &dyn Fn(&str) -> Option<PolyMesh>) -> PolyMesh {
    modifiers::apply(mesh, modifiers, t, object)
}

impl PolyMesh {
    /// A mesh from points and faces, without texture coordinates, smooth below 30°.
    pub fn new(positions: Vec<[f64; 3]>, faces: Vec<Vec<u32>>) -> PolyMesh {
        PolyMesh { positions, faces, uvs: None, smooth_angle: DEFAULT_SMOOTH_ANGLE }
    }

    /// Triangles with normals (smooth across edges bent less than `smooth_angle`) and texture
    /// coordinates (the given ones, or a box projection when there are none).
    pub fn triangulate(&self) -> TriMesh {
        triangulate::to_tris(self)
    }

    /// Welds triangles back into a polygon mesh: vertices at the same position become one
    /// (texture coordinates are kept per corner).
    pub fn from_triangles(t: &TriMesh) -> PolyMesh {
        let mut index: HashMap<[u32; 3], u32> = HashMap::new();
        let mut map = Vec::with_capacity(t.positions.len());
        let mut positions = vec![];
        for p in &t.positions {
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            let i = *index.entry(key).or_insert_with(|| {
                positions.push([p[0] as f64, p[1] as f64, p[2] as f64]);
                (positions.len() - 1) as u32
            });
            map.push(i);
        }
        let mut faces = vec![];
        let mut uvs = vec![];
        let has_uv = t.uvs.len() == t.positions.len();
        for tri in t.indices.as_chunks::<3>().0 {
            if tri.iter().any(|&i| i as usize >= map.len()) {
                continue;
            }
            let f: Vec<u32> = tri.iter().map(|&i| map[i as usize]).collect();
            if f[0] == f[1] || f[1] == f[2] || f[0] == f[2] {
                continue;
            }
            faces.push(f);
            if has_uv {
                uvs.push(tri.iter().map(|&i| [t.uvs[i as usize][0] as f64, t.uvs[i as usize][1] as f64]).collect());
            }
        }
        PolyMesh { positions, faces, uvs: has_uv.then_some(uvs), smooth_angle: DEFAULT_SMOOTH_ANGLE }
    }

    /// This mesh as an editable `mesh` shape (what `motion.editMesh` stores).
    pub fn to_shape(&self) -> Shape3d {
        let mut m = self.clone();
        m.sanitize();
        Shape3d::Mesh {
            vertices: m.positions.iter().map(|p| p.map(round_coord)).collect(),
            faces: m.faces.clone(),
            uvs: m.uvs.clone().unwrap_or_default(),
            auto_smooth: m.smooth_angle,
        }
    }

    /// The smallest box holding every vertex (`([0; 3], [0; 3])` when empty).
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in self.positions.iter().filter(|p| finite(**p)) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
    }

    /// The length of the bounding box's diagonal (a sense of scale for tolerances).
    pub fn size(&self) -> f64 {
        let (lo, hi) = self.bounds();
        dist(lo, hi)
    }

    /// The face's corner positions.
    pub fn face_points(&self, f: usize) -> Vec<[f64; 3]> {
        self.faces.get(f).map_or(vec![], |face| face.iter().filter_map(|&v| self.positions.get(v as usize).copied()).collect())
    }

    /// The face's unit normal (zero for degenerate faces).
    pub fn face_normal(&self, f: usize) -> [f64; 3] {
        norm(newell(self.face_points(f).into_iter()))
    }

    /// The face's area.
    pub fn face_area(&self, f: usize) -> f64 {
        len(newell(self.face_points(f).into_iter())) / 2.0
    }

    /// The average of the face's corners.
    pub fn face_center(&self, f: usize) -> [f64; 3] {
        let pts = self.face_points(f);
        if pts.is_empty() {
            return [0.0; 3];
        }
        let s = pts.iter().fold([0.0; 3], |a, p| add(a, *p));
        scale(s, 1.0 / pts.len() as f64)
    }

    /// Every edge once, with the faces using it, in a stable order.
    pub fn edges(&self) -> Vec<Edge> {
        let mut index: HashMap<(u32, u32), usize> = HashMap::new();
        let mut out: Vec<Edge> = vec![];
        for (fi, f) in self.faces.iter().enumerate() {
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                if a == b {
                    continue;
                }
                let key = (a.min(b), a.max(b));
                let i = *index.entry(key).or_insert_with(|| {
                    out.push(Edge { a: key.0, b: key.1, faces: vec![] });
                    out.len() - 1
                });
                if out[i].faces.last() != Some(&fi) {
                    out[i].faces.push(fi);
                }
            }
        }
        out
    }

    /// Undirected edge → the faces using it.
    pub(crate) fn edge_faces(&self) -> HashMap<(u32, u32), Vec<usize>> {
        let mut map: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                if a != b {
                    let e = map.entry((a.min(b), a.max(b))).or_default();
                    if e.last() != Some(&fi) {
                        e.push(fi);
                    }
                }
            }
        }
        map
    }

    /// Directed edge `a → b` (as a face walks it) → that face.
    pub(crate) fn directed_edges(&self) -> HashMap<(u32, u32), usize> {
        let mut map = HashMap::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for k in 0..f.len() {
                map.entry((f[k], f[(k + 1) % f.len()])).or_insert(fi);
            }
        }
        map
    }

    /// For each vertex, the faces using it.
    pub(crate) fn vertex_faces(&self) -> Vec<Vec<usize>> {
        let mut out = vec![vec![]; self.positions.len()];
        for (fi, f) in self.faces.iter().enumerate() {
            for &v in f {
                if let Some(list) = out.get_mut(v as usize)
                    && list.last() != Some(&fi)
                {
                    list.push(fi);
                }
            }
        }
        out
    }

    /// For each vertex, the vertices it shares an edge with.
    pub(crate) fn neighbours(&self) -> Vec<Vec<u32>> {
        let mut out: Vec<Vec<u32>> = vec![vec![]; self.positions.len()];
        for e in self.edges() {
            if (e.a as usize) < out.len() && (e.b as usize) < out.len() {
                out[e.a as usize].push(e.b);
                out[e.b as usize].push(e.a);
            }
        }
        out
    }

    /// Every edge has exactly two faces (a watertight surface).
    pub fn is_closed(&self) -> bool {
        !self.faces.is_empty() && self.edge_faces().values().all(|f| f.len() == 2)
    }

    /// The enclosed volume (positive when faces point outwards; meaningful for closed meshes).
    pub fn volume(&self) -> f64 {
        let mut v = 0.0;
        for f in 0..self.faces.len() {
            let p = self.face_points(f);
            for k in 1..p.len().saturating_sub(1) {
                v += dot(p[0], cross(p[k], p[k + 1]));
            }
        }
        v / 6.0
    }

    /// The total area of every face.
    pub fn area(&self) -> f64 {
        (0..self.faces.len()).map(|f| self.face_area(f)).sum()
    }

    /// Drops faces with missing vertices, non-finite corners or fewer than three distinct
    /// corners, collapses repeated corners, and fixes texture coordinates of the wrong shape.
    /// Returns where each face went (`None` when dropped).
    pub fn sanitize(&mut self) -> Vec<Option<usize>> {
        let n = self.positions.len();
        let ok: Vec<bool> = self.positions.iter().map(|p| finite(*p)).collect();
        let mut uvs = self.uvs.take().filter(|u| u.len() == self.faces.len());
        let had_uvs = uvs.is_some();
        let mut faces = Vec::with_capacity(self.faces.len());
        let mut out_uv = vec![];
        let mut moved = Vec::with_capacity(self.faces.len());
        for (fi, f) in std::mem::take(&mut self.faces).into_iter().enumerate() {
            if f.iter().any(|&v| v as usize >= n || !ok[v as usize]) {
                moved.push(None);
                continue;
            }
            let uv = uvs.as_mut().map(|u| std::mem::take(&mut u[fi])).filter(|u| u.len() == f.len());
            let (f, uv) = dedupe_corners(f, uv);
            if f.len() < 3 {
                moved.push(None);
                continue;
            }
            moved.push(Some(faces.len()));
            faces.push(f);
            out_uv.push(uv);
        }
        self.faces = faces;
        if had_uvs {
            let mut all = Vec::with_capacity(out_uv.len());
            for (fi, uv) in out_uv.into_iter().enumerate() {
                all.push(uv.unwrap_or_else(|| self.box_uv(fi)));
            }
            self.uvs = Some(all);
        }
        if !self.smooth_angle.is_finite() {
            self.smooth_angle = DEFAULT_SMOOTH_ANGLE;
        }
        moved
    }

    /// The texture coordinate at point `p` on face `f`, from a flat (affine) fit of the face's
    /// corner coordinates; for new corners made inside or along a face.
    pub(crate) fn uv_at(&self, f: usize, p: [f64; 3]) -> [f64; 2] {
        let pts = self.face_points(f);
        if pts.is_empty() || f >= self.faces.len() {
            return [0.0; 2];
        }
        uv_fit(&pts, &self.corner_uvs(f), p)
    }
}

/// The texture coordinate at `p` from a flat (affine, least squares) fit of a face's corners
/// `pts` and their coordinates `uv`.
pub(crate) fn uv_fit(pts: &[[f64; 3]], uv: &[[f64; 2]], p: [f64; 3]) -> [f64; 2] {
    {
        if pts.is_empty() || uv.len() != pts.len() {
            return [0.0; 2];
        }
        let (u, v) = plane_basis(norm(newell(pts.iter().copied())));
        let o = pts[0];
        let xy: Vec<[f64; 2]> = pts.iter().map(|q| [dot(sub(*q, o), u), dot(sub(*q, o), v)]).collect();
        // Least squares: uv = a·x + b·y + c, for each of the two coordinates.
        let mut m = [[0.0; 3]; 3];
        let mut r = [[0.0; 2]; 3];
        for (q, t) in xy.iter().zip(uv) {
            let row = [q[0], q[1], 1.0];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += row[i] * row[j];
                }
                r[i][0] += row[i] * t[0];
                r[i][1] += row[i] * t[1];
            }
        }
        let det3 = |m: &[[f64; 3]; 3]| {
            m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
        };
        let det = det3(&m);
        let x = [dot(sub(p, o), u), dot(sub(p, o), v)];
        if det.abs() < 1e-18 * (m[0][0] * m[1][1]).abs().max(1e-300) {
            // A degenerate face: the nearest corner's coordinate.
            let k = (0..pts.len()).min_by(|&a, &b| dist(pts[a], p).total_cmp(&dist(pts[b], p))).unwrap_or(0);
            return uv[k];
        }
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let mut coef = [0.0; 3];
            for (col, cf) in coef.iter_mut().enumerate() {
                let mut mm = m;
                for i in 0..3 {
                    mm[i][col] = r[i][c];
                }
                *cf = det3(&mm) / det;
            }
            *o = coef[0] * x[0] + coef[1] * x[1] + coef[2];
        }
        if out[0].is_finite() && out[1].is_finite() { out } else { [0.0; 2] }
    }
}

impl PolyMesh {
    /// Adds a face with its texture coordinates (made by box projection when the mesh has
    /// coordinates and none are given). Returns its index.
    pub fn add_face(&mut self, face: Vec<u32>, uv: Option<Vec<[f64; 2]>>) -> usize {
        self.faces.push(face);
        let fi = self.faces.len() - 1;
        if self.uvs.is_some() {
            let uv = uv.filter(|u| u.len() == self.faces[fi].len()).unwrap_or_else(|| self.box_uv(fi));
            if let Some(u) = self.uvs.as_mut() {
                u.push(uv);
            }
        }
        fi
    }

    /// Keeps the faces for which `keep(face index)` is true.
    pub fn retain_faces(&mut self, mut keep: impl FnMut(usize) -> bool) {
        let flags: Vec<bool> = (0..self.faces.len()).map(&mut keep).collect();
        let mut i = 0;
        self.faces.retain(|_| {
            i += 1;
            flags[i - 1]
        });
        if let Some(uvs) = self.uvs.as_mut() {
            let mut i = 0;
            uvs.retain(|_| {
                i += 1;
                flags.get(i - 1).copied().unwrap_or(false)
            });
        }
    }

    /// Removes vertices no face uses. Returns old index → new index (`u32::MAX` when removed).
    pub fn compact(&mut self) -> Vec<u32> {
        let mut used = vec![false; self.positions.len()];
        for f in &self.faces {
            for &v in f {
                if let Some(u) = used.get_mut(v as usize) {
                    *u = true;
                }
            }
        }
        let mut map = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::with_capacity(self.positions.len());
        for (i, p) in self.positions.iter().enumerate() {
            if used[i] {
                map[i] = positions.len() as u32;
                positions.push(*p);
            }
        }
        self.positions = positions;
        for f in &mut self.faces {
            for v in f.iter_mut() {
                *v = map.get(*v as usize).copied().unwrap_or(u32::MAX);
            }
        }
        self.faces.retain(|f| f.iter().all(|&v| v != u32::MAX));
        map
    }

    /// Renumbers corners through `map` (old vertex → new vertex), then removes repeated corners
    /// and faces left with fewer than three.
    pub(crate) fn remap(&mut self, map: &[u32]) {
        for f in &mut self.faces {
            for v in f.iter_mut() {
                *v = map.get(*v as usize).copied().unwrap_or(*v);
            }
        }
        self.sanitize();
    }

    /// Merges vertices closer than `distance`. Returns old index → kept index.
    pub fn weld(&mut self, distance: f64) -> Vec<u32> {
        let map = weld_map(&self.positions, distance.max(0.0));
        self.remap(&map);
        let kept = self.compact();
        map.iter().map(|&m| kept.get(m as usize).copied().unwrap_or(u32::MAX)).collect()
    }

    /// Reverses a face (and its texture coordinates), turning it to face the other way.
    pub fn flip_face(&mut self, f: usize) {
        if let Some(face) = self.faces.get_mut(f) {
            face.reverse();
            if let Some(uv) = self.uvs.as_mut().and_then(|u| u.get_mut(f)) {
                uv.reverse();
            }
        }
    }

    /// Adds another mesh's faces (its vertices renumbered after this one's).
    pub fn append(&mut self, other: &PolyMesh) {
        let base = self.positions.len() as u32;
        if self.uvs.is_none() && other.uvs.is_some() && !self.faces.is_empty() {
            self.uvs = Some((0..self.faces.len()).map(|f| self.box_uv(f)).collect());
        } else if self.uvs.is_none() && other.uvs.is_some() {
            self.uvs = Some(vec![]);
        }
        self.positions.extend_from_slice(&other.positions);
        for (fi, f) in other.faces.iter().enumerate() {
            let uv = other.uvs.as_ref().and_then(|u| u.get(fi)).cloned();
            self.add_face(f.iter().map(|v| v + base).collect(), uv);
        }
    }

    /// Texture coordinates for a face by box projection: the face's main axis picks the side of
    /// a cube it is painted from, one world unit per picture (a unit box gets 0–1 on each side).
    pub fn box_uv(&self, f: usize) -> Vec<[f64; 2]> {
        let n = self.face_normal(f);
        self.face_points(f).into_iter().map(|p| box_project(p, n)).collect()
    }

    /// The texture coordinates of a face corner (given or box-projected).
    pub(crate) fn corner_uvs(&self, f: usize) -> Vec<[f64; 2]> {
        match self.uvs.as_ref().and_then(|u| u.get(f)) {
            Some(uv) if uv.len() == self.faces[f].len() => uv.clone(),
            _ => self.box_uv(f),
        }
    }

    /// The first face hit by a ray from `origin` along `dir` (in the mesh's own space): the
    /// distance along the ray (in multiples of `dir`) and the face index.
    pub fn raycast(&self, origin: [f64; 3], dir: [f64; 3]) -> Option<(f64, usize)> {
        if !finite(origin) || !finite(dir) || len(dir) < 1e-300 {
            return None;
        }
        let mut best: Option<(f64, usize)> = None;
        for (fi, f) in self.faces.iter().enumerate() {
            let pts = self.face_points(fi);
            if pts.len() != f.len() || pts.len() < 3 {
                continue;
            }
            for t in triangulate::face_triangles(&pts) {
                if let Some(d) = ray_triangle(origin, dir, pts[t[0]], pts[t[1]], pts[t[2]])
                    && best.is_none_or(|(b, _)| d < b)
                {
                    best = Some((d, fi));
                }
            }
        }
        best
    }

    /// The vertex closest to a ray (within `max_dist` of it), skipping those hidden behind the
    /// first surface the ray hits. For picking in the Studio (mesh space; transform the ray).
    pub fn nearest_vertex(&self, origin: [f64; 3], dir: [f64; 3], max_dist: f64) -> Option<u32> {
        let d = try_norm(dir)?;
        let limit = self.visible_limit(origin, d, max_dist);
        let mut best: Option<(f64, f64, u32)> = None;
        for (i, p) in self.positions.iter().enumerate() {
            let rel = sub(*p, origin);
            let along = dot(rel, d);
            if along < 0.0 || along > limit {
                continue;
            }
            let off = len(sub(rel, scale(d, along)));
            if off <= max_dist && best.is_none_or(|(o, a, _)| off < o - 1e-12 || (off <= o + 1e-12 && along < a)) {
                best = Some((off, along, i as u32));
            }
        }
        best.map(|b| b.2)
    }

    /// The edge closest to a ray (within `max_dist`), skipping hidden ones, as its two vertices.
    pub fn nearest_edge(&self, origin: [f64; 3], dir: [f64; 3], max_dist: f64) -> Option<(u32, u32)> {
        let d = try_norm(dir)?;
        let limit = self.visible_limit(origin, d, max_dist);
        let mut best: Option<(f64, f64, (u32, u32))> = None;
        for e in self.edges() {
            let (Some(&a), Some(&b)) = (self.positions.get(e.a as usize), self.positions.get(e.b as usize)) else { continue };
            let Some((off, along)) = ray_segment(origin, d, a, b) else { continue };
            if along < 0.0 || along > limit || off > max_dist {
                continue;
            }
            if best.is_none_or(|(o, al, _)| off < o - 1e-12 || (off <= o + 1e-12 && along < al)) {
                best = Some((off, along, (e.a, e.b)));
            }
        }
        best.map(|b| b.2)
    }

    /// How far along a (unit) ray things still count as visible: just past the first hit.
    fn visible_limit(&self, origin: [f64; 3], d: [f64; 3], max_dist: f64) -> f64 {
        match self.raycast(origin, d) {
            Some((t, _)) => t + max_dist.max(self.size() * 1e-6) * 2.0,
            None => f64::INFINITY,
        }
    }
}

fn round_coord(v: f64) -> f64 {
    // Keeps stored JSON short (1e-9 is far below anything visible) and turns -0 into 0.
    let r = (v * 1e9).round() / 1e9;
    if r == 0.0 { 0.0 } else { r }
}

/// Removes repeated neighbouring corners (and their coordinates).
fn dedupe_corners(f: Vec<u32>, uv: Option<Vec<[f64; 2]>>) -> (Vec<u32>, Option<Vec<[f64; 2]>>) {
    let n = f.len();
    if n == 0 {
        return (f, uv);
    }
    let mut out = Vec::with_capacity(n);
    let mut out_uv = uv.as_ref().map(|_| Vec::with_capacity(n));
    for k in 0..n {
        if f[k] != f[(k + 1) % n] || n == 1 {
            out.push(f[k]);
            if let (Some(o), Some(u)) = (out_uv.as_mut(), uv.as_ref()) {
                o.push(u[k]);
            }
        }
    }
    (out, out_uv)
}

/// Box projection of one point given its face's normal (see [`PolyMesh::box_uv`]).
pub(crate) fn box_project(p: [f64; 3], n: [f64; 3]) -> [f64; 2] {
    let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
    let (u, v) = if ax >= ay && ax >= az {
        if n[0] >= 0.0 { (-p[2], -p[1]) } else { (p[2], -p[1]) }
    } else if ay >= az {
        if n[1] >= 0.0 { (p[0], p[2]) } else { (p[0], -p[2]) }
    } else if n[2] >= 0.0 {
        (p[0], -p[1])
    } else {
        (-p[0], -p[1])
    };
    [u + 0.5, v + 0.5]
}

/// For each point, the index of the point it merges into (points closer than `distance` join,
/// transitively; the lowest index is kept). Linear time through a grid of `distance` cells.
pub(crate) fn weld_map(points: &[[f64; 3]], distance: f64) -> Vec<u32> {
    let n = points.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    let cell = if distance > 0.0 { distance } else { 1e-12 };
    let key = |p: [f64; 3]| -> [i64; 3] { p.map(|c| (c / cell).floor().clamp(-1e15, 1e15) as i64) };
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        if !finite(*p) {
            continue;
        }
        let k = key(*p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else { continue };
                    for &j in list {
                        if dist(*p, points[j as usize]) <= distance {
                            let (a, b) = (find(&mut parent, i as u32), find(&mut parent, j));
                            if a != b {
                                let (lo, hi) = (a.min(b), a.max(b));
                                parent[hi as usize] = lo;
                            }
                        }
                    }
                }
            }
        }
        grid.entry(k).or_default().push(i as u32);
    }
    (0..n as u32).map(|i| find(&mut parent, i)).collect()
}

/// Möller–Trumbore: the distance along `dir` to the triangle, if hit in front.
pub(crate) fn ray_triangle(o: [f64; 3], d: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<f64> {
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-14 * len(e1).max(1e-300) * len(e2).max(1e-300) * len(d) {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = dot(s, p) * inv;
    if !(-1e-12..=1.0 + 1e-12).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < -1e-12 || u + v > 1.0 + 1e-12 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t >= 0.0 && t.is_finite()).then_some(t)
}

/// The distance between a ray (unit `d`) and a segment, and how far along the ray it is.
fn ray_segment(o: [f64; 3], d: [f64; 3], a: [f64; 3], b: [f64; 3]) -> Option<(f64, f64)> {
    // Minimise |o + s·d − (a + u·e)| with s ≥ 0 and 0 ≤ u ≤ 1 (d is unit).
    let e = sub(b, a);
    let w = sub(o, a);
    let (de, dw, ee, ew) = (dot(d, e), dot(d, w), dot(e, e), dot(e, w));
    let u = if ee < 1e-300 {
        0.0
    } else {
        let den = ee - de * de;
        let u = if den > 1e-12 * ee { (ew - dw * de) / den } else { 0.0 };
        u.clamp(0.0, 1.0)
    };
    let s = (u * de - dw).max(0.0);
    let u = if ee < 1e-300 { 0.0 } else { ((ew + s * de) / ee).clamp(0.0, 1.0) };
    let off = dist(add(o, scale(d, s)), add(a, scale(e, u)));
    off.is_finite().then_some((off, s))
}
