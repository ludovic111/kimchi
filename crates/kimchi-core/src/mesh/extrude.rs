//! Flat outlines (SVG path data) given thickness: curves flattened, outlines sorted into solid
//! parts and holes by the nonzero rule (like SVG fills them), caps triangulated with their holes,
//! side walls, and optional rounded front and back edges.

use crate::path::{Seg, parse};

use super::{DEFAULT_SMOOTH_ANGLE, PolyMesh, ear_clip_with_holes, weld_map};

/// Segments in each rounded edge.
const BEVEL_STEPS: usize = 4;

type P2 = [f64; 2];

/// The outline of `d` scaled so its larger side is `size`, centred, upright (SVG's y points
/// down), `depth` thick along z (centred), with `bevel` rounding the front and back edges.
pub(super) fn extrude(d: &str, size: f64, depth: f64, bevel: f64) -> PolyMesh {
    let segs = match parse(d) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("extrude: bad path data: {e}");
            return PolyMesh::default();
        }
    };
    let contours = flatten(&segs);
    // Fit: centre, flip y, larger side = size.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in contours.iter().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let size = if size.is_finite() { size.abs() } else { 2.0 };
    if !extent.is_finite() || extent <= 0.0 || size <= 0.0 {
        return PolyMesh::default();
    }
    let k = size / extent;
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let contours: Vec<Vec<P2>> = contours
        .into_iter()
        .map(|c0| clean(c0.into_iter().map(|p| [(p[0] - c[0]) * k, -(p[1] - c[1]) * k]).collect(), size * 1e-9))
        .filter(|c| c.len() >= 3 && area(c).abs() > size * size * 1e-12)
        .collect();
    let shapes = classify(contours, size);
    if shapes.is_empty() {
        return PolyMesh::default();
    }
    let depth = if depth.is_finite() { depth.abs() } else { 0.0 };
    build(&shapes, depth, if bevel.is_finite() { bevel.max(0.0) } else { 0.0 }, size)
}

/// Subpaths as point lists (curves cut into short lines). Every subpath is closed (a fill).
fn flatten(segs: &[Seg]) -> Vec<Vec<P2>> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut grow = |p: P2| {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    };
    for s in segs {
        match *s {
            Seg::Move(p) | Seg::Line(p) => grow(p),
            Seg::Quad(a, p) => {
                grow(a);
                grow(p)
            }
            Seg::Cubic(a, b, p) => {
                grow(a);
                grow(b);
                grow(p)
            }
            Seg::Close => {}
        }
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let tol = if extent.is_finite() && extent > 0.0 { extent * 0.0008 } else { 1.0 };
    let steps = |dd: f64| -> usize {
        let n = (dd / (4.0 * tol)).sqrt().ceil();
        if n.is_finite() { (n as usize).clamp(1, 64) } else { 1 }
    };
    let mut out = vec![];
    let mut cur: Vec<P2> = vec![];
    let mut at = [0.0, 0.0];
    let mut start = [0.0, 0.0];
    for s in segs {
        match *s {
            Seg::Move(p) => {
                if cur.len() > 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur = vec![p];
                at = p;
                start = p;
            }
            Seg::Line(p) => {
                if cur.is_empty() {
                    cur.push(at);
                }
                cur.push(p);
                at = p;
            }
            Seg::Quad(c1, p) => {
                if cur.is_empty() {
                    cur.push(at);
                }
                let n = steps(len2(sub2(add2(at, p), scale2(c1, 2.0))));
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    cur.push(std::array::from_fn(|k| u * u * at[k] + 2.0 * u * t * c1[k] + t * t * p[k]));
                }
                at = p;
            }
            Seg::Cubic(c1, c2, p) => {
                if cur.is_empty() {
                    cur.push(at);
                }
                let dd = len2(add2(sub2(at, scale2(c1, 2.0)), c2)).max(len2(add2(sub2(c1, scale2(c2, 2.0)), p))) * 1.5;
                let n = steps(dd);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    cur.push(std::array::from_fn(|k| u * u * u * at[k] + 3.0 * u * u * t * c1[k] + 3.0 * u * t * t * c2[k] + t * t * t * p[k]));
                }
                at = p;
            }
            Seg::Close => {
                if cur.len() > 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                at = start;
            }
        }
    }
    if cur.len() > 2 {
        out.push(cur);
    }
    out
}

fn sub2(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}
fn add2(a: P2, b: P2) -> P2 {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale2(a: P2, k: f64) -> P2 {
    [a[0] * k, a[1] * k]
}
fn len2(a: P2) -> f64 {
    (a[0] * a[0] + a[1] * a[1]).sqrt()
}

/// Drops repeated points (and a closing point equal to the first) and points on a straight run.
fn clean(pts: Vec<P2>, tol: f64) -> Vec<P2> {
    let mut out: Vec<P2> = Vec::with_capacity(pts.len());
    for p in pts {
        if !(p[0].is_finite() && p[1].is_finite()) {
            continue;
        }
        if out.last().is_some_and(|l| len2(sub2(*l, p)) <= tol) {
            continue;
        }
        out.push(p);
    }
    while out.len() > 1 && len2(sub2(out[0], out[out.len() - 1])) <= tol {
        out.pop();
    }
    // Collinear middles add nothing but walls to shade.
    let mut changed = true;
    while changed && out.len() > 3 {
        changed = false;
        let n = out.len();
        for i in 0..n {
            let (a, b, c) = (out[(i + n - 1) % n], out[i], out[(i + 1) % n]);
            let cr = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
            if cr.abs() <= tol * (len2(sub2(b, a)) + len2(sub2(c, b))) && dot2(sub2(b, a), sub2(c, b)) > 0.0 {
                out.remove(i);
                changed = true;
                break;
            }
        }
    }
    out
}

fn dot2(a: P2, b: P2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn area(c: &[P2]) -> f64 {
    let mut s = 0.0;
    for i in 0..c.len() {
        let (a, b) = (c[i], c[(i + 1) % c.len()]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    s / 2.0
}

/// How many times the outlines wind around `p` (counter-clockwise counts up).
fn winding(p: P2, contours: &[Vec<P2>]) -> i32 {
    let mut w = 0;
    for c in contours {
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            let side = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= p[1] {
                if b[1] > p[1] && side > 0.0 {
                    w += 1;
                }
            } else if b[1] <= p[1] && side < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

/// One solid part: an outline (counter-clockwise) and its holes (clockwise).
struct Part {
    outer: Vec<P2>,
    holes: Vec<Vec<P2>>,
}

/// Keeps outlines that separate filled from empty (nonzero rule), each turned so the filled
/// side is on its left, and gives every hole to the smallest outline around it.
fn classify(contours: Vec<Vec<P2>>, size: f64) -> Vec<Part> {
    let eps = size * 1e-5;
    let mut kept: Vec<Vec<P2>> = vec![];
    for c in &contours {
        let n = c.len();
        let tries = [0, n / 3, (2 * n) / 3, n / 2, n - 1];
        let mut verdict = None;
        for &i in &tries {
            let (a, b) = (c[i], c[(i + 1) % n]);
            let Some(dir) = norm2(sub2(b, a)) else { continue };
            let left = [-dir[1], dir[0]];
            let m = scale2(add2(a, b), 0.5);
            let fl = winding(add2(m, scale2(left, eps)), &contours) != 0;
            let fr = winding(sub2(m, scale2(left, eps)), &contours) != 0;
            if fl != fr {
                verdict = Some(fl);
                break;
            }
        }
        match verdict {
            Some(true) => kept.push(c.clone()),
            Some(false) => kept.push(c.iter().rev().copied().collect()),
            None => {}
        }
    }
    let (outers, holes): (Vec<Vec<P2>>, Vec<Vec<P2>>) = kept.into_iter().partition(|c| area(c) > 0.0);
    let mut parts: Vec<Part> = outers.into_iter().map(|outer| Part { outer, holes: vec![] }).collect();
    for h in holes {
        let probe = h[0];
        let owner = parts
            .iter()
            .enumerate()
            .filter(|(_, p)| winding(probe, std::slice::from_ref(&p.outer)) != 0 || inside_or_on(probe, &p.outer))
            .min_by(|a, b| area(&a.1.outer).total_cmp(&area(&b.1.outer)))
            .map(|(i, _)| i);
        if let Some(i) = owner {
            parts[i].holes.push(h);
        }
    }
    parts
}

fn inside_or_on(p: P2, c: &[P2]) -> bool {
    // A hole touching its outline at a point still belongs to it: test a point nudged inwards.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for q in c {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1] && winding(p, std::slice::from_ref(&c.to_vec())) != 0
}

fn norm2(a: P2) -> Option<P2> {
    let l = len2(a);
    (l > 1e-300 && l.is_finite()).then(|| [a[0] / l, a[1] / l])
}

/// `c` moved `d` towards its filled side (the left), corners mitred (spikes limited).
fn inset(c: &[P2], d: f64) -> Vec<P2> {
    if d == 0.0 {
        return c.to_vec();
    }
    let n = c.len();
    (0..n)
        .map(|i| {
            let (a, b, cc) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n]);
            let d0 = norm2(sub2(b, a)).unwrap_or([1.0, 0.0]);
            let d1 = norm2(sub2(cc, b)).unwrap_or(d0);
            let (l0, l1) = ([-d0[1], d0[0]], [-d1[1], d1[0]]);
            let m = norm2(add2(l0, l1)).unwrap_or(l0);
            let k = 1.0 / dot2(m, l0).max(0.25);
            add2(b, scale2(m, d * k))
        })
        .collect()
}

fn build(parts: &[Part], depth: f64, bevel: f64, size: f64) -> PolyMesh {
    let hd = depth / 2.0;
    // Rounding can't be deeper than half the thickness, nor wider than the thinnest part allows.
    let mut b = bevel.min(hd);
    for p in parts {
        for c in std::iter::once(&p.outer).chain(&p.holes) {
            let perim: f64 = (0..c.len()).map(|i| len2(sub2(c[(i + 1) % c.len()], c[i]))).sum();
            if perim > 0.0 {
                b = b.min(0.8 * area(c).abs() / perim);
            }
        }
    }
    if b < size * 1e-6 {
        b = 0.0;
    }
    // Rings from back to front: (inset, z).
    let mut rings: Vec<(f64, f64)> = vec![];
    if depth <= 0.0 {
        rings.push((0.0, 0.0));
    } else if b > 0.0 {
        for k in (0..=BEVEL_STEPS).rev() {
            let t = k as f64 / BEVEL_STEPS as f64 * std::f64::consts::FRAC_PI_2;
            rings.push((b * (1.0 - t.cos()), -(hd - b) - b * t.sin()));
        }
        for k in 0..=BEVEL_STEPS {
            let t = k as f64 / BEVEL_STEPS as f64 * std::f64::consts::FRAC_PI_2;
            rings.push((b * (1.0 - t.cos()), (hd - b) + b * t.sin()));
        }
    } else {
        rings.push((0.0, -hd));
        rings.push((0.0, hd));
    }
    rings.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
    // Planar coordinates for the caps over the whole shape.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in parts.iter().flat_map(|p| p.outer.iter()) {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let (w, h) = ((hi[0] - lo[0]).max(1e-12), (hi[1] - lo[1]).max(1e-12));
    let cap_uv = |p: P2, back: bool| {
        let u = (p[0] - lo[0]) / w;
        [if back { 1.0 - u } else { u }, (hi[1] - p[1]) / h]
    };
    let mut m = PolyMesh { uvs: Some(vec![]), smooth_angle: DEFAULT_SMOOTH_ANGLE, ..Default::default() };
    for part in parts {
        let contours: Vec<&Vec<P2>> = std::iter::once(&part.outer).chain(&part.holes).collect();
        // ids[contour][ring][point]
        let mut ids: Vec<Vec<Vec<u32>>> = vec![];
        let mut flats: Vec<Vec<Vec<P2>>> = vec![];
        for c in &contours {
            let mut per_ring = vec![];
            let mut flat = vec![];
            for &(d, z) in &rings {
                let pts = inset(c, d);
                per_ring.push(
                    pts.iter()
                        .map(|p| {
                            m.positions.push([p[0], p[1], z]);
                            (m.positions.len() - 1) as u32
                        })
                        .collect::<Vec<u32>>(),
                );
                flat.push(pts);
            }
            ids.push(per_ring);
            flats.push(flat);
        }
        // Walls between rings.
        for (ci, c) in contours.iter().enumerate() {
            let n = c.len();
            let mut along = vec![0.0];
            for i in 0..n {
                along.push(along[i] + len2(sub2(c[(i + 1) % n], c[i])));
            }
            let perim = along[n].max(1e-12);
            for r in 0..rings.len().saturating_sub(1) {
                let (z0, z1) = (rings[r].1, rings[r + 1].1);
                let (v0, v1) = ((hd - z0) / depth.max(1e-12), (hd - z1) / depth.max(1e-12));
                for i in 0..n {
                    let j = (i + 1) % n;
                    let (u0, u1) = (along[i] / perim, along[i + 1] / perim);
                    let f = vec![ids[ci][r][i], ids[ci][r][j], ids[ci][r + 1][j], ids[ci][r + 1][i]];
                    m.faces.push(f);
                    if let Some(u) = m.uvs.as_mut() {
                        u.push(vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]]);
                    }
                }
            }
        }
        // Caps: the front faces +z, the back −z.
        let last = rings.len() - 1;
        let caps: Vec<(usize, bool)> = if depth <= 0.0 { vec![(0, false)] } else { vec![(last, false), (0, true)] };
        for (ring, back) in caps {
            let outer = &flats[0][ring];
            if part.holes.is_empty() {
                let mut f: Vec<u32> = ids[0][ring].clone();
                let mut uv: Vec<[f64; 2]> = outer.iter().map(|p| cap_uv(*p, back)).collect();
                if back {
                    f.reverse();
                    uv.reverse();
                }
                m.faces.push(f);
                if let Some(u) = m.uvs.as_mut() {
                    u.push(uv);
                }
                continue;
            }
            let holes: Vec<Vec<P2>> = (1..contours.len()).map(|ci| flats[ci][ring].clone()).collect();
            let mut all_ids: Vec<u32> = ids[0][ring].clone();
            let mut all_pts: Vec<P2> = outer.clone();
            for ci in 1..contours.len() {
                all_ids.extend_from_slice(&ids[ci][ring]);
                all_pts.extend_from_slice(&flats[ci][ring]);
            }
            for t in ear_clip_with_holes(outer, &holes) {
                let mut t = t;
                if back {
                    t.swap(1, 2);
                }
                m.faces.push(t.iter().map(|&i| all_ids[i]).collect());
                if let Some(u) = m.uvs.as_mut() {
                    u.push(t.iter().map(|&i| cap_uv(all_pts[i], back)).collect());
                }
            }
        }
    }
    let map = weld_map(&m.positions, size * 1e-9);
    m.remap(&map);
    m.compact();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_with_a_hole_extrudes_closed() {
        // Outer clockwise in SVG (y down) and the hole drawn the same way: nonzero keeps the
        // hole only when it winds the other way, so draw it reversed.
        let m = extrude("M0 0 H100 V100 H0 Z M30 30 V70 H70 V30 Z", 2.0, 0.5, 0.0);
        assert!(m.is_closed(), "watertight");
        let (lo, hi) = m.bounds();
        assert!((hi[0] - lo[0] - 2.0).abs() < 1e-9 && (hi[2] - 0.25).abs() < 1e-9 && (lo[2] + 0.25).abs() < 1e-9);
        // Volume: (2² − 0.8²) × 0.5.
        assert!((m.volume() - (4.0 - 0.64) * 0.5).abs() < 1e-6, "{}", m.volume());
        // Same winding for the hole: nonzero fills it in.
        let solid = extrude("M0 0 H100 V100 H0 Z M30 30 H70 V70 H30 Z", 2.0, 0.5, 0.0);
        assert!((solid.volume() - 2.0).abs() < 1e-6, "{}", solid.volume());
    }

    #[test]
    fn upright_curved_and_bevelled() {
        // A teardrop pointing up in SVG (small y = top) stays pointing up.
        let d = "M50 0 C80 40 100 60 100 75 A50 50 0 0 1 0 75 C0 60 20 40 50 0 Z";
        let m = extrude(d, 2.0, 0.4, 0.0);
        let top = m.positions.iter().max_by(|a, b| a[1].total_cmp(&b[1])).unwrap();
        assert!(top[0].abs() < 1e-6, "the tip is at the top: {top:?}");
        assert!(m.is_closed());
        let b = extrude(d, 2.0, 0.4, 0.05);
        assert!(b.is_closed());
        assert!(b.volume() < m.volume() && b.volume() > m.volume() * 0.85);
        let (lo, hi) = b.bounds();
        assert!((hi[2] - 0.2).abs() < 1e-9 && (lo[2] + 0.2).abs() < 1e-9);
    }

    #[test]
    fn bad_paths_are_empty() {
        assert!(extrude("M0 0 L", 2.0, 0.5, 0.0).faces.is_empty());
        assert!(extrude("", 2.0, 0.5, 0.0).faces.is_empty());
        assert!(extrude("M0 0 L10 0", 2.0, 0.5, 0.0).faces.is_empty());
        let flat = extrude("M0 0 H10 V10 Z", 1.0, 0.0, 0.3);
        assert_eq!(flat.faces.len(), 1);
    }
}
