//! A bounding volume hierarchy over world-space triangles, for the path tracer: built with the
//! surface area heuristic over binned centroids (the top levels in parallel), flattened into one
//! array of 32-byte nodes, and walked nearest child first.

use super::math::V3;

/// Bins per axis when looking for the best split.
const BINS: usize = 12;
/// Below this many triangles a node always becomes a leaf when splitting doesn't pay.
const MAX_LEAF: usize = 4;
/// Nodes with more triangles than this are split even when the heuristic says not to.
const FORCE_SPLIT: usize = 16;
/// Subtrees with more triangles than this are built on another thread.
const PARALLEL: usize = 32_768;

/// A triangle ready for intersection: one corner and the two edges from it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tri {
    pub v0: V3,
    pub e1: V3,
    pub e2: V3,
}

impl Tri {
    pub(crate) fn new(a: V3, b: V3, c: V3) -> Tri {
        Tri { v0: a, e1: b - a, e2: c - a }
    }

    /// Ray–triangle intersection (Möller–Trumbore), both faces: distance and barycentrics of
    /// the second and third corners.
    #[inline]
    pub(crate) fn hit(&self, o: V3, d: V3, tmax: f32) -> Option<(f32, f32, f32)> {
        let p = d.cross(self.e2);
        let det = self.e1.dot(p);
        if det.abs() < 1e-14 {
            return None;
        }
        let inv = 1.0 / det;
        let s = o - self.v0;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = s.cross(self.e1);
        let v = d.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = self.e2.dot(q) * inv;
        (t > 0.0 && t < tmax).then_some((t, u, v))
    }
}

/// Where a ray met a triangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hit {
    pub t: f32,
    /// The triangle's index in the list the hierarchy was built from.
    pub prim: u32,
    /// Where the triangle is kept (for [`Bvh::tri`]).
    pub slot: u32,
    /// Barycentrics of the triangle's second and third corners.
    pub u: f32,
    pub v: f32,
}

/// `count > 0`: a leaf with `count` triangles from `first`. `count == 0`: an inner node whose
/// children are the next node and `first`.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Node {
    lo: [f32; 3],
    first: u32,
    hi: [f32; 3],
    count: u32,
}

pub(crate) struct Bvh {
    nodes: Vec<Node>,
    /// Triangles in leaf order.
    tris: Vec<Tri>,
    /// For each triangle in leaf order, its index in the original list.
    prims: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Aabb {
    lo: V3,
    hi: V3,
}

impl Aabb {
    const EMPTY: Aabb = Aabb { lo: V3(f32::INFINITY, f32::INFINITY, f32::INFINITY), hi: V3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY) };

    fn grow(&mut self, o: &Aabb) {
        self.lo = self.lo.min(o.lo);
        self.hi = self.hi.max(o.hi);
    }

    fn grow_point(&mut self, p: V3) {
        self.lo = self.lo.min(p);
        self.hi = self.hi.max(p);
    }

    fn area(&self) -> f32 {
        let d = self.hi - self.lo;
        if d.0 < 0.0 { 0.0 } else { 2.0 * (d.0 * d.1 + d.1 * d.2 + d.2 * d.0) }
    }
}

fn axis(v: V3, a: usize) -> f32 {
    match a {
        0 => v.0,
        1 => v.1,
        _ => v.2,
    }
}

/// What building reads: each triangle's box and centroid.
struct Build<'a> {
    boxes: &'a [Aabb],
    centers: &'a [V3],
}

impl Bvh {
    /// The hierarchy over `tris` (world space). Degenerate and non-finite triangles are kept
    /// but never hit.
    pub(crate) fn build(tris: Vec<Tri>) -> Bvh {
        use rayon::prelude::*;
        let boxes: Vec<Aabb> = tris
            .par_iter()
            .map(|t| {
                let mut b = Aabb::EMPTY;
                for p in [t.v0, t.v0 + t.e1, t.v0 + t.e2] {
                    // Non-finite corners would poison every box above them: park them at the origin.
                    let p = if p.0.is_finite() && p.1.is_finite() && p.2.is_finite() { p } else { V3::default() };
                    b.grow_point(p);
                }
                b
            })
            .collect();
        let centers: Vec<V3> = boxes.par_iter().map(|b| (b.lo + b.hi) * 0.5).collect();
        let mut idx: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::with_capacity(tris.len() * 2 / MAX_LEAF.max(1) + 1);
        if !tris.is_empty() {
            let ctx = Build { boxes: &boxes, centers: &centers };
            build_node(&ctx, &mut idx, 0, &mut nodes, 0);
        }
        let ordered: Vec<Tri> = idx.iter().map(|&i| tris[i as usize]).collect();
        Bvh { nodes, tris: ordered, prims: idx }
    }

    /// The triangle a hit is on.
    pub(crate) fn tri(&self, hit: &Hit) -> &Tri {
        &self.tris[hit.slot as usize]
    }

    pub(crate) fn len(&self) -> usize {
        self.tris.len()
    }

    /// The nearest hit along `o + t·d` with `t` in (0, `tmax`).
    pub(crate) fn intersect(&self, o: V3, d: V3, tmax: f32) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut tmax = tmax;
        self.walk(o, d, &mut tmax, |bvh, slot, tmax| {
            if let Some((t, u, v)) = bvh.tris[slot].hit(o, d, *tmax) {
                *tmax = t;
                best = Some(Hit { t, prim: bvh.prims[slot], slot: slot as u32, u, v });
            }
            false
        });
        best
    }

    /// Calls `f` for hits along the ray before `tmax`, in no particular order, until it
    /// returns true (then this does too: the ray is blocked).
    pub(crate) fn any_hit(&self, o: V3, d: V3, tmax: f32, mut f: impl FnMut(Hit) -> bool) -> bool {
        let mut tm = tmax;
        let mut blocked = false;
        self.walk(o, d, &mut tm, |bvh, slot, tm| {
            if let Some((t, u, v)) = bvh.tris[slot].hit(o, d, *tm)
                && f(Hit { t, prim: bvh.prims[slot], slot: slot as u32, u, v })
            {
                blocked = true;
                return true;
            }
            false
        });
        blocked
    }

    /// Visits the leaves' triangles the ray may meet, nearer boxes first; `leaf` may shorten
    /// `tmax` and returns true to stop.
    #[inline]
    fn walk(&self, o: V3, d: V3, tmax: &mut f32, mut leaf: impl FnMut(&Bvh, usize, &mut f32) -> bool) {
        if self.nodes.is_empty() {
            return;
        }
        let inv = V3(1.0 / d.0, 1.0 / d.1, 1.0 / d.2);
        let mut stack = [(0u32, 0.0f32); 64];
        let mut sp = 0usize;
        let mut node = 0usize;
        if slab(&self.nodes[0], o, inv, *tmax) == f32::INFINITY {
            return;
        }
        loop {
            let n = &self.nodes[node];
            if n.count > 0 {
                let first = n.first as usize;
                for slot in first..first + n.count as usize {
                    if leaf(self, slot, tmax) {
                        return;
                    }
                }
            } else {
                let (a, b) = (node + 1, n.first as usize);
                let (ta, tb) = (slab(&self.nodes[a], o, inv, *tmax), slab(&self.nodes[b], o, inv, *tmax));
                let (near, far, tn, tf) = if ta <= tb { (a, b, ta, tb) } else { (b, a, tb, ta) };
                if tn != f32::INFINITY {
                    if tf != f32::INFINITY && sp < stack.len() {
                        stack[sp] = (far as u32, tf);
                        sp += 1;
                    }
                    node = near;
                    continue;
                }
            }
            // Pop the next box, skipping those now beyond the nearest hit.
            loop {
                if sp == 0 {
                    return;
                }
                sp -= 1;
                let (next, t) = stack[sp];
                if t <= *tmax {
                    node = next as usize;
                    break;
                }
            }
        }
    }
}

/// Where the ray enters the box (0 when it starts inside), or infinity when it misses it
/// before `tmax`.
#[inline]
fn slab(n: &Node, o: V3, inv: V3, tmax: f32) -> f32 {
    let tx1 = (n.lo[0] - o.0) * inv.0;
    let tx2 = (n.hi[0] - o.0) * inv.0;
    let ty1 = (n.lo[1] - o.1) * inv.1;
    let ty2 = (n.hi[1] - o.1) * inv.1;
    let tz1 = (n.lo[2] - o.2) * inv.2;
    let tz2 = (n.hi[2] - o.2) * inv.2;
    // `min`/`max` drop NaN (0 × ∞ for rays in a box's plane), keeping the other bound.
    let tmin = tx1.min(tx2).max(ty1.min(ty2)).max(tz1.min(tz2)).max(0.0);
    let tmx = tx1.max(tx2).min(ty1.max(ty2)).min(tz1.max(tz2)).min(tmax);
    if tmin <= tmx { tmin } else { f32::INFINITY }
}

/// Builds the subtree over `idx` (triangle indices; `offset` is where `idx` starts in the whole
/// list) into `out`, its root first.
fn build_node(ctx: &Build, idx: &mut [u32], offset: usize, out: &mut Vec<Node>, depth: u32) {
    let mut bounds = Aabb::EMPTY;
    let mut cb = Aabb::EMPTY;
    for &i in idx.iter() {
        bounds.grow(&ctx.boxes[i as usize]);
        cb.grow_point(ctx.centers[i as usize]);
    }
    let me = out.len();
    out.push(Node { lo: bounds.lo.arr(), first: offset as u32, hi: bounds.hi.arr(), count: idx.len() as u32 });
    let n = idx.len();
    if n <= 1 || depth >= 60 {
        return;
    }
    let mid = match best_split(ctx, idx, &bounds, &cb) {
        Some((axis_i, pos, cost)) => {
            if cost >= n as f32 && n <= FORCE_SPLIT {
                return;
            }
            let m = partition(idx, |i| axis(ctx.centers[i as usize], axis_i) < pos);
            if m == 0 || m == n { n / 2 } else { m }
        }
        // All centroids in one spot: split the list in half.
        None if n > MAX_LEAF => n / 2,
        None => return,
    };
    let (left, right) = idx.split_at_mut(mid);
    if n > PARALLEL {
        let (mut l, mut r) = (Vec::new(), Vec::new());
        rayon::join(|| build_node(ctx, left, offset, &mut l, depth + 1), || build_node(ctx, right, offset + mid, &mut r, depth + 1));
        // Children were numbered from 0 in their own lists: shift them into place.
        let lbase = (me + 1) as u32;
        let rbase = lbase + l.len() as u32;
        out.extend(l.into_iter().map(|mut nd| {
            if nd.count == 0 {
                nd.first += lbase;
            }
            nd
        }));
        out.extend(r.into_iter().map(|mut nd| {
            if nd.count == 0 {
                nd.first += rbase;
            }
            nd
        }));
        out[me].count = 0;
        out[me].first = rbase;
    } else {
        // Sub-builds into a shared list number nodes from its start; in a parallel branch the
        // list starts empty, so indices are relative to that branch and shifted afterwards.
        build_node(ctx, left, offset, out, depth + 1);
        let right_at = out.len() as u32;
        build_node(ctx, right, offset + mid, out, depth + 1);
        out[me].count = 0;
        out[me].first = right_at;
    }
}

/// The cheapest split by the surface area heuristic: axis, centroid position, cost (in
/// triangle tests; a leaf costs `n`).
fn best_split(ctx: &Build, idx: &[u32], bounds: &Aabb, cb: &Aabb) -> Option<(usize, f32, f32)> {
    let area = bounds.area().max(1e-30);
    let mut best: Option<(usize, f32, f32)> = None;
    for a in 0..3 {
        let (lo, hi) = (axis(cb.lo, a), axis(cb.hi, a));
        if !(hi - lo > 1e-12 * (lo.abs() + hi.abs() + 1.0)) {
            continue;
        }
        let scale = BINS as f32 / (hi - lo);
        let mut bins = [(Aabb::EMPTY, 0u32); BINS];
        for &i in idx {
            let b = (((axis(ctx.centers[i as usize], a) - lo) * scale) as usize).min(BINS - 1);
            bins[b].0.grow(&ctx.boxes[i as usize]);
            bins[b].1 += 1;
        }
        // Areas and counts left of each plane, then right of it.
        let mut left = [(0.0f32, 0u32); BINS - 1];
        let (mut acc, mut cnt) = (Aabb::EMPTY, 0);
        for k in 0..BINS - 1 {
            acc.grow(&bins[k].0);
            cnt += bins[k].1;
            left[k] = (acc.area(), cnt);
        }
        let (mut acc, mut cnt) = (Aabb::EMPTY, 0);
        for k in (1..BINS).rev() {
            acc.grow(&bins[k].0);
            cnt += bins[k].1;
            let (la, lc) = left[k - 1];
            if lc == 0 || cnt == 0 {
                continue;
            }
            // One box test per child, against triangle tests weighted by the chance of entering.
            let cost = 0.5 + (la * lc as f32 + acc.area() * cnt as f32) / area;
            if best.is_none_or(|b| cost < b.2) {
                best = Some((a, lo + k as f32 / scale, cost));
            }
        }
    }
    best
}

/// Moves the elements for which `left` holds to the front; returns how many there are.
fn partition(idx: &mut [u32], left: impl Fn(u32) -> bool) -> usize {
    let mut m = 0;
    for k in 0..idx.len() {
        if left(idx[k]) {
            idx.swap(k, m);
            m += 1;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn f(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        fn v(&mut self, k: f32) -> V3 {
            V3((self.f() - 0.5) * k, (self.f() - 0.5) * k, (self.f() - 0.5) * k)
        }
    }

    fn soup(n: usize, r: &mut Lcg) -> Vec<Tri> {
        (0..n)
            .map(|_| {
                let c = r.v(10.0);
                Tri::new(c + r.v(1.0), c + r.v(1.0), c + r.v(1.0))
            })
            .collect()
    }

    #[test]
    fn same_hits_as_brute_force() {
        let mut r = Lcg(7);
        let tris = soup(3000, &mut r);
        let bvh = Bvh::build(tris.clone());
        assert_eq!(bvh.len(), 3000);
        let mut hits = 0;
        for _ in 0..4000 {
            let o = r.v(16.0);
            let d = (r.v(2.0) - o * 0.05).norm();
            let mut brute: Option<(f32, u32)> = None;
            for (i, t) in tris.iter().enumerate() {
                if let Some((t, _, _)) = t.hit(o, d, brute.map_or(f32::INFINITY, |b| b.0)) {
                    brute = Some((t, i as u32));
                }
            }
            let got = bvh.intersect(o, d, f32::INFINITY);
            match (brute, got) {
                (None, None) => {}
                (Some((t, i)), Some(h)) => {
                    hits += 1;
                    assert!((t - h.t).abs() < 1e-4, "{t} vs {}", h.t);
                    // Ties between triangles at the same distance may pick either.
                    assert!(i == h.prim || (tris[h.prim as usize].hit(o, d, f32::INFINITY).unwrap().0 - t).abs() < 1e-4);
                }
                (b, g) => panic!("brute force {b:?}, hierarchy {g:?}"),
            }
            // Blocked between the origin and the hit (or anywhere), the same as brute force.
            let any = bvh.any_hit(o, d, f32::INFINITY, |_| true);
            assert_eq!(any, brute.is_some());
        }
        assert!(hits > 400, "enough rays hit something: {hits}");
    }

    #[test]
    fn odd_input() {
        // Empty, one triangle, a degenerate one, a non-finite one, many in the same place.
        assert!(Bvh::build(vec![]).intersect(V3(0.0, 0.0, 0.0), V3(0.0, 0.0, 1.0), f32::INFINITY).is_none());
        let one = Tri::new(V3(-1.0, -1.0, 5.0), V3(1.0, -1.0, 5.0), V3(0.0, 1.0, 5.0));
        let flat = Tri::new(V3(0.0, 0.0, 3.0), V3(0.0, 0.0, 3.0), V3(0.0, 0.0, 3.0));
        let nan = Tri::new(V3(f32::NAN, 0.0, 0.0), V3(1.0, 0.0, 0.0), V3(0.0, f32::INFINITY, 0.0));
        let mut tris = vec![one, flat, nan];
        tris.extend(std::iter::repeat_n(Tri::new(V3(-1.0, -1.0, 9.0), V3(1.0, -1.0, 9.0), V3(0.0, 1.0, 9.0)), 100));
        let bvh = Bvh::build(tris);
        let h = bvh.intersect(V3(0.0, 0.0, 0.0), V3(0.0, 0.0, 1.0), f32::INFINITY).unwrap();
        assert_eq!(h.prim, 0);
        assert!((h.t - 5.0).abs() < 1e-5);
        assert!(bvh.intersect(V3(0.0, 0.0, 0.0), V3(0.0, 0.0, 1.0), 4.0).is_none(), "tmax");
        let mut n = 0;
        bvh.any_hit(V3(0.0, 0.0, 0.0), V3(0.0, 0.0, 1.0), f32::INFINITY, |_| {
            n += 1;
            false
        });
        assert_eq!(n, 101, "every triangle along the ray is reported once");
    }

    /// `cargo test --release -p kimchi-media --lib build_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn build_speed() {
        // A bumpy terrain of a million triangles, and rays down onto it from a camera-like spot.
        let n = 708usize;
        let height = |x: usize, z: usize| ((x as f32 * 0.07).sin() * (z as f32 * 0.05).cos()) * 3.0;
        let at = |x: usize, z: usize| V3(x as f32 / n as f32 * 100.0 - 50.0, height(x, z), z as f32 / n as f32 * 100.0 - 50.0);
        let mut tris = Vec::with_capacity(n * n * 2);
        for z in 0..n {
            for x in 0..n {
                tris.push(Tri::new(at(x, z), at(x + 1, z), at(x, z + 1)));
                tris.push(Tri::new(at(x + 1, z), at(x + 1, z + 1), at(x, z + 1)));
            }
        }
        let (t0, c0) = (std::time::Instant::now(), cpu());
        let bvh = Bvh::build(tris);
        let (built, c1) = (t0.elapsed(), cpu());
        let mut r = Lcg(3);
        let t1 = std::time::Instant::now();
        let mut hits = 0;
        let eye = V3(0.0, 30.0, 60.0);
        for _ in 0..1_000_000 {
            let target = V3((r.f() - 0.5) * 100.0, 0.0, (r.f() - 0.5) * 100.0);
            if bvh.intersect(eye, (target - eye).norm(), f32::INFINITY).is_some() {
                hits += 1;
            }
        }
        let c2 = cpu();
        eprintln!(
            "{} triangles: built in {built:?} ({:.2} s of CPU, {} nodes); 1M rays in {:?} ({:.2} s of CPU, {hits} hits)",
            bvh.len(),
            c1 - c0,
            bvh.nodes.len(),
            t1.elapsed(),
            c2 - c1
        );
        // On a busy machine the clock lies; the CPU time doesn't (spread over 4 cores).
        assert!((c1 - c0) / 4.0 < 1.0 || built.as_secs_f32() < 1.0);
    }

    /// Seconds of CPU this process has used (Linux; 0 elsewhere).
    fn cpu() -> f64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let f: Vec<&str> = stat.rsplit(')').next().unwrap_or("").split_whitespace().collect();
        f.get(11).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 100.0
    }
}
