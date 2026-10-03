//! Booleans (union, difference, intersection) of two meshes with BSP trees (the classic csg.js
//! method): each solid is split by the other's planes and the pieces inside or outside are kept.
//! Trees live in arenas and are walked with explicit stacks, so big meshes can't overflow the
//! call stack. Works best on closed meshes; the result's faces are convex pieces, welded.

use super::math::*;
use super::triangulate::face_triangles;
use super::{PolyMesh, weld_map};

/// Which part of the two solids to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Difference,
    Intersect,
}

/// Past this many triangles in the two meshes together, booleans are skipped (and logged).
const MAX_TRIANGLES: usize = 300_000;

#[derive(Clone, Copy)]
struct Vert {
    p: V3,
    uv: [f64; 2],
}

#[derive(Clone, Copy)]
struct Plane {
    n: V3,
    w: f64,
}

impl Plane {
    fn flip(&mut self) {
        self.n = scale(self.n, -1.0);
        self.w = -self.w;
    }
}

#[derive(Clone)]
struct Poly {
    v: Vec<Vert>,
    plane: Plane,
}

impl Poly {
    fn new(v: Vec<Vert>) -> Option<Poly> {
        let n = try_norm(newell(v.iter().map(|x| x.p)))?;
        let w = dot(n, v[0].p);
        Some(Poly { v, plane: Plane { n, w } })
    }

    fn flip(&mut self) {
        self.v.reverse();
        self.plane.flip();
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// Puts `poly` into the lists by which side of `plane` it lies (splitting it if it spans it).
fn split(plane: &Plane, poly: Poly, eps: f64, cf: &mut Vec<Poly>, cb: &mut Vec<Poly>, front: &mut Vec<Poly>, back: &mut Vec<Poly>) {
    let mut kind = 0u8;
    let types: Vec<u8> = poly
        .v
        .iter()
        .map(|v| {
            let t = dot(plane.n, v.p) - plane.w;
            let k = if t < -eps {
                BACK
            } else if t > eps {
                FRONT
            } else {
                COPLANAR
            };
            kind |= k;
            k
        })
        .collect();
    match kind {
        COPLANAR => {
            if dot(plane.n, poly.plane.n) > 0.0 {
                cf.push(poly)
            } else {
                cb.push(poly)
            }
        }
        FRONT => front.push(poly),
        BACK => back.push(poly),
        _ => {
            let (mut f, mut b) = (vec![], vec![]);
            let n = poly.v.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (ti, tj) = (types[i], types[j]);
                let (vi, vj) = (poly.v[i], poly.v[j]);
                if ti != BACK {
                    f.push(vi);
                }
                if ti != FRONT {
                    b.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let den = dot(plane.n, sub(vj.p, vi.p));
                    let t = if den.abs() > 1e-300 { ((plane.w - dot(plane.n, vi.p)) / den).clamp(0.0, 1.0) } else { 0.5 };
                    let v = Vert { p: lerp(vi.p, vj.p, t), uv: [vi.uv[0] + (vj.uv[0] - vi.uv[0]) * t, vi.uv[1] + (vj.uv[1] - vi.uv[1]) * t] };
                    f.push(v);
                    b.push(v);
                }
            }
            if f.len() >= 3 {
                front.push(Poly { v: f, plane: poly.plane });
            }
            if b.len() >= 3 {
                back.push(Poly { v: b, plane: poly.plane });
            }
        }
    }
}

#[derive(Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<usize>,
    back: Option<usize>,
    polys: Vec<Poly>,
}

struct Tree {
    nodes: Vec<Node>,
    eps: f64,
}

impl Tree {
    fn new(polys: Vec<Poly>, eps: f64) -> Tree {
        let mut t = Tree { nodes: vec![Node::default()], eps };
        t.build(polys);
        t
    }

    fn build(&mut self, polys: Vec<Poly>) {
        let mut stack = vec![(0usize, polys)];
        while let Some((ni, polys)) = stack.pop() {
            if polys.is_empty() {
                continue;
            }
            let plane = *self.nodes[ni].plane.get_or_insert(polys[0].plane);
            let (mut cf, mut cb, mut f, mut b) = (vec![], vec![], vec![], vec![]);
            for p in polys {
                split(&plane, p, self.eps, &mut cf, &mut cb, &mut f, &mut b);
            }
            self.nodes[ni].polys.extend(cf);
            self.nodes[ni].polys.extend(cb);
            if !f.is_empty() {
                let c = self.child(ni, true);
                stack.push((c, f));
            }
            if !b.is_empty() {
                let c = self.child(ni, false);
                stack.push((c, b));
            }
        }
    }

    fn child(&mut self, ni: usize, front: bool) -> usize {
        let existing = if front { self.nodes[ni].front } else { self.nodes[ni].back };
        existing.unwrap_or_else(|| {
            self.nodes.push(Node::default());
            let c = self.nodes.len() - 1;
            if front {
                self.nodes[ni].front = Some(c);
            } else {
                self.nodes[ni].back = Some(c);
            }
            c
        })
    }

    /// Solid ↔ empty space.
    fn invert(&mut self) {
        for n in &mut self.nodes {
            for p in &mut n.polys {
                p.flip();
            }
            if let Some(pl) = n.plane.as_mut() {
                pl.flip();
            }
            std::mem::swap(&mut n.front, &mut n.back);
        }
    }

    /// The parts of `polys` outside this solid.
    fn clip_polygons(&self, polys: Vec<Poly>) -> Vec<Poly> {
        let mut out = vec![];
        let mut stack = vec![(0usize, polys)];
        while let Some((ni, polys)) = stack.pop() {
            let node = &self.nodes[ni];
            let Some(plane) = node.plane else {
                out.extend(polys);
                continue;
            };
            let (mut f, mut b) = (vec![], vec![]);
            for p in polys {
                let (mut cf, mut cb) = (vec![], vec![]);
                split(&plane, p, self.eps, &mut cf, &mut cb, &mut f, &mut b);
                f.extend(cf);
                b.extend(cb);
            }
            match node.front {
                Some(c) => stack.push((c, f)),
                None => out.extend(f),
            }
            if let Some(c) = node.back {
                stack.push((c, b));
            }
        }
        out
    }

    /// Removes the parts of this tree's polygons inside `other`.
    fn clip_to(&mut self, other: &Tree) {
        for i in 0..self.nodes.len() {
            let polys = std::mem::take(&mut self.nodes[i].polys);
            self.nodes[i].polys = other.clip_polygons(polys);
        }
    }

    fn all(&self) -> Vec<Poly> {
        self.nodes.iter().flat_map(|n| n.polys.iter().cloned()).collect()
    }
}

fn polys_of(m: &PolyMesh) -> Vec<Poly> {
    let mut out = vec![];
    for (fi, f) in m.faces.iter().enumerate() {
        let pts = m.face_points(fi);
        if pts.len() != f.len() || pts.len() < 3 || !pts.iter().all(|p| finite(*p)) {
            continue;
        }
        let uv = m.corner_uvs(fi);
        for t in face_triangles(&pts) {
            if let Some(p) = Poly::new(t.iter().map(|&k| Vert { p: pts[k], uv: uv[k] }).collect()) {
                out.push(p);
            }
        }
    }
    out
}

/// `a` combined with `b` (both in the same space).
pub fn boolean(a: &PolyMesh, b: &PolyMesh, op: BoolOp) -> PolyMesh {
    let (pa, pb) = (polys_of(a), polys_of(b));
    if pa.len() + pb.len() > MAX_TRIANGLES {
        tracing::warn!(a = pa.len(), b = pb.len(), "boolean skipped: too many faces");
        return a.clone();
    }
    if pb.is_empty() {
        return if op == BoolOp::Intersect { PolyMesh { smooth_angle: a.smooth_angle, ..Default::default() } } else { a.clone() };
    }
    if pa.is_empty() {
        return match op {
            BoolOp::Union => b.clone(),
            _ => PolyMesh { smooth_angle: a.smooth_angle, ..Default::default() },
        };
    }
    let scale_ = a.size().max(b.size()).max(1e-9);
    let eps = scale_ * 1e-7;
    let mut ta = Tree::new(pa, eps);
    let mut tb = Tree::new(pb, eps);
    let polys = match op {
        BoolOp::Union => {
            ta.clip_to(&tb);
            tb.clip_to(&ta);
            tb.invert();
            tb.clip_to(&ta);
            tb.invert();
            ta.build(tb.all());
            ta.all()
        }
        BoolOp::Difference => {
            ta.invert();
            ta.clip_to(&tb);
            tb.clip_to(&ta);
            tb.invert();
            tb.clip_to(&ta);
            tb.invert();
            ta.build(tb.all());
            ta.invert();
            ta.all()
        }
        BoolOp::Intersect => {
            ta.invert();
            tb.clip_to(&ta);
            tb.invert();
            ta.clip_to(&tb);
            tb.clip_to(&ta);
            ta.build(tb.all());
            ta.invert();
            ta.all()
        }
    };
    let keep_uv = a.uvs.is_some() || b.uvs.is_some();
    let mut m = PolyMesh { smooth_angle: a.smooth_angle, uvs: keep_uv.then(Vec::new), ..Default::default() };
    for p in polys {
        let base = m.positions.len() as u32;
        m.positions.extend(p.v.iter().map(|v| v.p));
        m.faces.push((0..p.v.len() as u32).map(|k| base + k).collect());
        if let Some(u) = m.uvs.as_mut() {
            u.push(p.v.iter().map(|v| v.uv).collect());
        }
    }
    let map = weld_map(&m.positions, eps * 10.0);
    m.remap(&map);
    m.compact();
    close_t_junctions(&mut m, eps * 10.0);
    m
}

/// The splits leave T-junctions: a vertex in the middle of a neighbour's edge that the
/// neighbour doesn't have, so the result has cracks (open edges) where it should be closed, and
/// what reads its edges (solidify's rims, smooth normals, edit mode) sees holes. Puts each such
/// vertex into the edge it lies on.
fn close_t_junctions(m: &mut PolyMesh, tol: f64) {
    use std::collections::HashMap;
    /// Per edge of a face: the vertices to put into it, by where along it they are.
    type EdgeInserts = HashMap<(u32, u32), Vec<(f64, u32)>>;
    // A split can uncover another; a few rounds are plenty.
    for _ in 0..4 {
        let open: Vec<(u32, u32, usize)> = m.edges().into_iter().filter(|e| e.faces.len() == 1).map(|e| (e.a, e.b, e.faces[0])).collect();
        if open.is_empty() {
            return;
        }
        let mut ends: Vec<u32> = open.iter().flat_map(|&(a, b, _)| [a, b]).collect();
        ends.sort_unstable();
        ends.dedup();
        if open.len().saturating_mul(ends.len()) > 20_000_000 {
            tracing::debug!(edges = open.len(), "boolean: too many open edges to mend");
            return;
        }
        // Per face: the vertices to put into each of its edges, in order along it.
        let mut into: HashMap<usize, EdgeInserts> = HashMap::new();
        for &(a, b, f) in &open {
            let (pa, pb) = (m.positions[a as usize], m.positions[b as usize]);
            let d = sub(pb, pa);
            let l2 = dot(d, d);
            if l2 <= tol * tol {
                continue;
            }
            let mut on: Vec<(f64, u32)> = ends
                .iter()
                .filter(|&&v| v != a && v != b)
                .filter_map(|&v| {
                    let p = m.positions[v as usize];
                    let t = dot(sub(p, pa), d) / l2;
                    (t > 1e-9 && t < 1.0 - 1e-9 && dist(p, mad(pa, d, t)) <= tol).then_some((t, v))
                })
                .collect();
            if !on.is_empty() {
                on.sort_by(|x, y| x.0.total_cmp(&y.0));
                into.entry(f).or_default().insert((a, b), on);
            }
        }
        if into.is_empty() {
            return;
        }
        for (f, edges) in into {
            let face = m.faces[f].clone();
            let uv = m.uvs.as_ref().map(|u| u[f].clone());
            let (mut nf, mut nuv) = (vec![], vec![]);
            for k in 0..face.len() {
                let (a, b) = (face[k], face[(k + 1) % face.len()]);
                nf.push(a);
                if let Some(u) = &uv {
                    nuv.push(u[k]);
                }
                let (list, forward) = match edges.get(&(a.min(b), a.max(b))) {
                    Some(l) => (l, a < b),
                    None => continue,
                };
                let mut add = |t: f64, v: u32| {
                    // A face never goes through one of its own corners twice.
                    if face.contains(&v) || nf.contains(&v) {
                        return;
                    }
                    nf.push(v);
                    if let Some(u) = &uv {
                        let (ua, ub) = (u[k], u[(k + 1) % face.len()]);
                        nuv.push([ua[0] + (ub[0] - ua[0]) * t, ua[1] + (ub[1] - ua[1]) * t]);
                    }
                };
                if forward {
                    list.iter().for_each(|&(t, v)| add(t, v));
                } else {
                    list.iter().rev().for_each(|&(t, v)| add(1.0 - t, v));
                }
            }
            m.faces[f] = nf;
            if let Some(u) = m.uvs.as_mut() {
                u[f] = nuv;
            }
        }
    }
}
