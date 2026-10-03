//! Shape operators (`stack::OPERATORS`) on a shape layer's outline, in order, before it is
//! filled, stroked and trimmed: offset path, zig zag, wiggle path, round corners, twist, pucker &
//! bloat. The repeater isn't a change of outline: [`copies`] lists the copies the layer draws.
//!
//! An outline is worked on as contours of points (curves flattened finely); points that were a
//! segment's end are marked as corners, so "per segment" and "corners" mean what they do in
//! After Effects (a rectangle has four segments, an ellipse four smooth ones).

use kimchi_core::motion::Operator;
use tiny_skia::{Path, PathBuilder, PathSegment, Point, Transform};

use super::noise;

type P = [f32; 2];

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pt {
    p: P,
    /// A segment ends here (a vertex of the outline).
    corner: bool,
}

#[derive(Debug, Clone)]
struct Contour {
    pts: Vec<Pt>,
    closed: bool,
}

/// Most points an operator makes per contour (a guard against absurd parameters).
const MAX_POINTS: usize = 40_000;

/// `path` with every operator except repeaters applied in order. `t` is the layer list's time
/// (wiggle paths move with it).
pub(crate) fn apply(path: Path, ops: &[Operator], t: f64) -> Option<Path> {
    let geo: Vec<&Operator> = ops.iter().filter(|o| o.enabled && o.kind != "repeater").collect();
    if geo.is_empty() {
        return Some(path);
    }
    let mut cs = flatten(&path);
    for o in geo {
        let n = |name: &str| o.n(name) as f32;
        cs = match o.kind.as_str() {
            "offset" => cs.iter().map(|c| offset(c, n("amount"), &o.s("join"))).collect(),
            "zigzag" => cs.iter().map(|c| zigzag(c, n("size"), n("ridges"), o.s("points") == "smooth", o.b("perSegment"))).collect(),
            "wiggle" => {
                let ts = (t * o.n("speed")) as f32;
                cs.iter().enumerate().map(|(i, c)| wiggle(c, n("size"), n("detail"), ts, (o.n("seed") as u32).wrapping_add(i as u32 * 977))).collect()
            }
            "roundCorners" => cs.iter().map(|c| round_corners(c, n("radius"))).collect(),
            "twist" => twist(cs, n("angle"), o.v2("center")),
            "puckerBloat" => pucker_bloat(cs, n("amount") / 100.0),
            other => {
                tracing::debug!(operator = other, "unknown shape operator; skipped");
                cs
            }
        };
    }
    build(&cs)
}

/// The copies a layer's repeaters draw, in drawing order: each one's transform (layer pixels)
/// and opacity. `None` when it has no repeater (one plain copy).
pub(crate) fn copies(ops: &[Operator]) -> Option<Vec<(Transform, f32)>> {
    let reps: Vec<&Operator> = ops.iter().filter(|o| o.enabled && o.kind == "repeater").collect();
    if reps.is_empty() {
        return None;
    }
    let mut out = vec![(Transform::identity(), 1.0f32)];
    for r in reps {
        let n = r.n("copies").max(0.0);
        let count = n.ceil() as usize;
        let frac = (n - n.floor()) as f32;
        let (offset, pos, rot, scale, anchor) = (r.n("offset") as f32, r.v2("position"), r.n("rotation") as f32, r.n("scale") as f32, r.v2("anchor"));
        let (a0, a1) = (r.n("startOpacity") as f32, r.n("endOpacity") as f32);
        let mut step = vec![];
        for i in 0..count {
            let k = i as f32 + offset;
            let (ax, ay) = (anchor[0] as f32, anchor[1] as f32);
            let s = if scale > 0.0 { scale.powf(k) } else if k == 0.0 { 1.0 } else { 0.0 };
            let s = if s.is_finite() { s.min(1e4) } else { 0.0 };
            let ts = Transform::from_translate(ax + pos[0] as f32 * k, ay + pos[1] as f32 * k).pre_rotate(rot * k).pre_scale(s, s).pre_translate(-ax, -ay);
            let mut alpha = if count > 1 { a0 + (a1 - a0) * i as f32 / (count - 1) as f32 } else { a0 };
            if i + 1 == count && frac > 0.0 {
                alpha *= frac;
            }
            step.push((ts, alpha));
        }
        if r.s("composite") == "below" {
            step.reverse();
        }
        let mut next = Vec::with_capacity(step.len() * out.len());
        for (ts, a) in &step {
            for (inner, b) in &out {
                next.push((ts.pre_concat(*inner), a * b));
            }
        }
        out = next;
        if out.len() > 10_000 {
            tracing::warn!(copies = out.len(), "repeaters make too many copies; drawing the first 10000");
            out.truncate(10_000);
        }
    }
    Some(out)
}

/// Is `pt` (layer pixels) inside `path` (non-zero winding)? Open contours count as closed.
pub(crate) fn contains(path: &Path, pt: P) -> bool {
    let mut winding = 0i32;
    for c in flatten(path) {
        let n = c.pts.len();
        for i in 0..n {
            let (a, b) = (c.pts[i].p, c.pts[(i + 1) % n].p);
            if a[1] <= pt[1] {
                if b[1] > pt[1] && cross(sub(b, a), sub(pt, a)) > 0.0 {
                    winding += 1;
                }
            } else if b[1] <= pt[1] && cross(sub(b, a), sub(pt, a)) < 0.0 {
                winding -= 1;
            }
        }
    }
    winding != 0
}

/// The contours of `path` as points (curves flattened), each with whether it is closed.
pub(crate) fn polylines(path: &Path) -> Vec<(Vec<[f32; 2]>, bool)> {
    flatten(path).into_iter().map(|c| (c.pts.iter().map(|q| q.p).collect(), c.closed)).collect()
}

/// Distance from `pt` to the outline itself (layer pixels).
pub(crate) fn distance_to(path: &Path, pt: P) -> f32 {
    let mut best = f32::MAX;
    for c in flatten(path) {
        let n = c.pts.len();
        let edges = if c.closed { n } else { n.saturating_sub(1) };
        for i in 0..edges {
            let (a, b) = (c.pts[i].p, c.pts[(i + 1) % n].p);
            let ab = sub(b, a);
            let t = (dot(sub(pt, a), ab) / dot(ab, ab).max(1e-9)).clamp(0.0, 1.0);
            best = best.min(len(sub(pt, add(a, scale(ab, t)))));
        }
        if n == 1 {
            best = best.min(len(sub(pt, c.pts[0].p)));
        }
    }
    best
}

// ---------------------------------------------------------------------------------------------
// Contours

fn flatten(path: &Path) -> Vec<Contour> {
    let mut out: Vec<Contour> = vec![];
    let mut cur: Option<Contour> = None;
    let mut last: P = [0.0, 0.0];
    let finish = |c: Option<Contour>, out: &mut Vec<Contour>| {
        if let Some(c) = c
            && !c.pts.is_empty()
        {
            out.push(c);
        }
    };
    let p = |q: Point| [q.x, q.y];
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(q) => {
                finish(cur.take(), &mut out);
                last = p(q);
                cur = Some(Contour { pts: vec![Pt { p: last, corner: true }], closed: false });
            }
            PathSegment::LineTo(q) => {
                last = p(q);
                cur.get_or_insert_with(|| Contour { pts: vec![], closed: false }).pts.push(Pt { p: last, corner: true });
            }
            PathSegment::QuadTo(c1, q) => {
                let (a, c1, b) = (last, p(c1), p(q));
                let steps = curve_steps(len(sub(c1, a)) + len(sub(b, c1)));
                let c = cur.get_or_insert_with(|| Contour { pts: vec![], closed: false });
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let m = 1.0 - t;
                    let v = [m * m * a[0] + 2.0 * m * t * c1[0] + t * t * b[0], m * m * a[1] + 2.0 * m * t * c1[1] + t * t * b[1]];
                    c.pts.push(Pt { p: v, corner: i == steps });
                }
                last = b;
            }
            PathSegment::CubicTo(c1, c2, q) => {
                let (a, c1, c2, b) = (last, p(c1), p(c2), p(q));
                let steps = curve_steps(len(sub(c1, a)) + len(sub(c2, c1)) + len(sub(b, c2)));
                let c = cur.get_or_insert_with(|| Contour { pts: vec![], closed: false });
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let m = 1.0 - t;
                    let v: P = std::array::from_fn(|k| m * m * m * a[k] + 3.0 * m * m * t * c1[k] + 3.0 * m * t * t * c2[k] + t * t * t * b[k]);
                    c.pts.push(Pt { p: v, corner: i == steps });
                }
                last = b;
            }
            PathSegment::Close => {
                if let Some(c) = cur.as_mut() {
                    c.closed = true;
                    if c.pts.len() > 1 && len(sub(c.pts[0].p, c.pts[c.pts.len() - 1].p)) < 1e-4 {
                        c.pts.pop();
                    }
                    last = c.pts[0].p;
                }
                finish(cur.take(), &mut out);
            }
        }
    }
    finish(cur, &mut out);
    out
}

fn curve_steps(rough_len: f32) -> usize {
    ((rough_len / 3.0).ceil() as usize).clamp(6, 64)
}

fn build(cs: &[Contour]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for c in cs {
        let mut pts = c.pts.iter().filter(|q| q.p[0].is_finite() && q.p[1].is_finite());
        let Some(first) = pts.next() else { continue };
        pb.move_to(first.p[0], first.p[1]);
        for q in pts {
            pb.line_to(q.p[0], q.p[1]);
        }
        if c.closed {
            pb.close();
        }
    }
    pb.finish()
}

/// Positive when the contour turns one way, negative the other (twice the area).
fn area(c: &Contour) -> f32 {
    let n = c.pts.len();
    (0..n).map(|i| cross(c.pts[i].p, c.pts[(i + 1) % n].p)).sum()
}

/// The contour's runs: lists of point indices from one corner to the next (both included).
/// A closed contour's last run ends where it started.
fn runs(c: &Contour, per_segment: bool) -> Vec<Vec<usize>> {
    let n = c.pts.len();
    if n < 2 {
        return vec![(0..n).collect()];
    }
    let mut corners: Vec<usize> = if per_segment { (0..n).filter(|&i| c.pts[i].corner).collect() } else { vec![] };
    if !c.closed {
        corners.retain(|&i| i != 0 && i != n - 1);
        let mut out = vec![];
        let mut start = 0;
        for &k in corners.iter().chain(std::iter::once(&(n - 1))) {
            out.push((start..=k).collect());
            start = k;
        }
        return out;
    }
    if corners.is_empty() {
        corners.push(0);
    }
    let mut out = vec![];
    for (j, &a) in corners.iter().enumerate() {
        let b = corners[(j + 1) % corners.len()];
        let mut run = vec![a];
        let mut i = a;
        loop {
            i = (i + 1) % n;
            run.push(i);
            if i == b {
                break;
            }
        }
        out.push(run);
    }
    out
}

/// Cumulative lengths along a run of points.
fn lengths(pts: &[P]) -> Vec<f32> {
    let mut out = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    out.push(0.0);
    for w in pts.windows(2) {
        acc += len(sub(w[1], w[0]));
        out.push(acc);
    }
    out
}

/// The point and unit direction `s` along a run.
fn walk(pts: &[P], cum: &[f32], s: f32) -> (P, P) {
    let total = *cum.last().unwrap_or(&0.0);
    if pts.len() < 2 || total <= 0.0 {
        return (pts.first().copied().unwrap_or([0.0, 0.0]), [1.0, 0.0]);
    }
    let s = s.clamp(0.0, total);
    let i = match cum.binary_search_by(|v| v.total_cmp(&s)) {
        Ok(i) => i.min(pts.len() - 2),
        Err(i) => i.saturating_sub(1).min(pts.len() - 2),
    };
    let span = (cum[i + 1] - cum[i]).max(1e-9);
    let k = ((s - cum[i]) / span).clamp(0.0, 1.0);
    (add(pts[i], scale(sub(pts[i + 1], pts[i]), k)), norm(sub(pts[i + 1], pts[i])))
}

/// Points added along straight stretches so none is longer than `step`.
fn densify(c: &Contour, step: f32) -> Contour {
    let n = c.pts.len();
    let edges = if c.closed { n } else { n.saturating_sub(1) };
    let mut total = 0.0;
    for i in 0..edges {
        total += len(sub(c.pts[(i + 1) % n].p, c.pts[i].p));
    }
    let step = step.max(total / MAX_POINTS as f32).max(0.05);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(c.pts[i]);
        if i < edges {
            let (a, b) = (c.pts[i].p, c.pts[(i + 1) % n].p);
            let pieces = (len(sub(b, a)) / step).floor() as usize;
            for k in 1..pieces.min(MAX_POINTS) {
                out.push(Pt { p: add(a, scale(sub(b, a), k as f32 / pieces as f32)), corner: false });
            }
        }
    }
    Contour { pts: out, closed: c.closed }
}

// ---------------------------------------------------------------------------------------------
// Operators

fn offset(c: &Contour, d: f32, join: &str) -> Contour {
    let n = c.pts.len();
    if n < 2 || d.abs() < 1e-3 || !d.is_finite() {
        return c.clone();
    }
    // Outward normals: which side is out depends on which way the contour turns.
    let turn = if c.closed && area(c) < 0.0 { -1.0 } else { 1.0 };
    let normal = |a: P, b: P| {
        let t = norm(sub(b, a));
        [t[1] * turn, -t[0] * turn]
    };
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let p = c.pts[i].p;
        let prev = if i > 0 { Some(c.pts[i - 1].p) } else if c.closed { Some(c.pts[n - 1].p) } else { None };
        let next = if i + 1 < n { Some(c.pts[i + 1].p) } else if c.closed { Some(c.pts[0].p) } else { None };
        let (n_in, n_out) = match (prev, next) {
            (Some(a), Some(b)) => (normal(a, p), normal(p, b)),
            (Some(a), None) => (normal(a, p), normal(a, p)),
            (None, Some(b)) => (normal(p, b), normal(p, b)),
            (None, None) => continue,
        };
        let corner = c.pts[i].corner;
        let m = norm(add(n_in, n_out));
        let cos = dot(n_in, n_out);
        if cos > 0.995 || len(add(n_in, n_out)) < 1e-6 && cos > 0.0 {
            out.push(Pt { p: add(p, scale(m, d)), corner });
            continue;
        }
        let miter_len = d / dot(m, n_in).max(0.05);
        // Is this corner's outside on the side we move to?
        let (ti, to) = (norm([-n_in[1] * turn, n_in[0] * turn]), norm([-n_out[1] * turn, n_out[0] * turn]));
        // Turning the way the contour turns overall is a convex corner: its outside is where a
        // positive offset goes.
        let outer = cross(ti, to) * turn * d > 0.0;
        if !outer {
            let l = miter_len.clamp(-4.0 * d.abs(), 4.0 * d.abs());
            out.push(Pt { p: add(p, scale(m, l)), corner });
            continue;
        }
        match join {
            "miter" if miter_len.abs() <= 4.0 * d.abs() => out.push(Pt { p: add(p, scale(m, miter_len)), corner: true }),
            "round" => {
                let a0 = n_in[1].atan2(n_in[0]);
                let mut a1 = n_out[1].atan2(n_out[0]);
                let mut sweep = a1 - a0;
                while sweep > std::f32::consts::PI {
                    sweep -= std::f32::consts::TAU;
                }
                while sweep < -std::f32::consts::PI {
                    sweep += std::f32::consts::TAU;
                }
                a1 = a0 + sweep;
                let steps = ((sweep.abs() / 10f32.to_radians()).ceil() as usize).max(1);
                for k in 0..=steps {
                    let a = a0 + (a1 - a0) * k as f32 / steps as f32;
                    // The normals point outward for positive d; inward offsets use the opposite side.
                    out.push(Pt { p: add(p, scale([a.cos(), a.sin()], d)), corner: k == 0 || k == steps });
                }
            }
            _ => {
                out.push(Pt { p: add(p, scale(n_in, d)), corner: true });
                out.push(Pt { p: add(p, scale(n_out, d)), corner: true });
            }
        }
    }
    Contour { pts: out, closed: c.closed }
}

fn zigzag(c: &Contour, size: f32, ridges: f32, smooth: bool, per_segment: bool) -> Contour {
    let r = ridges.round().max(0.0) as usize;
    if r == 0 || size.abs() < 1e-3 || c.pts.len() < 2 {
        return c.clone();
    }
    let mut out: Vec<Pt> = vec![];
    for run in runs(c, per_segment) {
        let pts: Vec<P> = run.iter().map(|&i| c.pts[i].p).collect();
        let cum = lengths(&pts);
        let total = *cum.last().unwrap_or(&0.0);
        out.push(c.pts[run[0]]);
        if total <= 1e-6 {
            continue;
        }
        let r = r.min(MAX_POINTS / 16);
        let samples: Vec<(f32, f32, bool)> = if smooth {
            let m = (r * 12).clamp(16, MAX_POINTS);
            (1..m).map(|j| {
                let s = j as f32 / m as f32;
                (s * total, size * (std::f32::consts::TAU * r as f32 * s).sin(), false)
            })
            .collect()
        } else {
            (0..2 * r).map(|k| ((k as f32 + 0.5) / (2 * r) as f32 * total, if k % 2 == 0 { size } else { -size }, true)).collect()
        };
        for (s, off, corner) in samples {
            let (p, t) = walk(&pts, &cum, s);
            out.push(Pt { p: add(p, scale([t[1], -t[0]], off)), corner });
        }
        if !c.closed && run.last() == Some(&(c.pts.len() - 1)) {
            out.push(c.pts[c.pts.len() - 1]);
        }
    }
    Contour { pts: out, closed: c.closed }
}

fn wiggle(c: &Contour, size: f32, detail: f32, ts: f32, seed: u32) -> Contour {
    if size.abs() < 1e-3 || c.pts.is_empty() {
        return c.clone();
    }
    let mut pts = if detail > 0.0 { densify(c, 100.0 / detail) } else { c.clone() };
    for (i, q) in pts.pts.iter_mut().enumerate() {
        let x = i as f32 * 0.618;
        q.p[0] += size * noise::noise3(x, ts, 0.5, seed);
        q.p[1] += size * noise::noise3(x, ts, 5.5, seed);
    }
    pts.closed = c.closed;
    pts
}

fn round_corners(c: &Contour, radius: f32) -> Contour {
    let n = c.pts.len();
    if radius <= 0.0 || n < 3 {
        return c.clone();
    }
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let q = c.pts[i];
        let ends = !c.closed && (i == 0 || i == n - 1);
        if !q.corner || ends {
            out.push(q);
            continue;
        }
        let (a, b) = (c.pts[(i + n - 1) % n].p, c.pts[(i + 1) % n].p);
        let (din, dout) = (norm(sub(q.p, a)), norm(sub(b, q.p)));
        if dot(din, dout) > 0.995 {
            out.push(q);
            continue;
        }
        let d = radius.min(len(sub(q.p, a)) / 2.0).min(len(sub(b, q.p)) / 2.0);
        let (from, to) = (sub(q.p, scale(din, d)), add(q.p, scale(dout, d)));
        let steps = 8;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let m = 1.0 - t;
            let p: P = std::array::from_fn(|j| m * m * from[j] + 2.0 * m * t * q.p[j] + t * t * to[j]);
            out.push(Pt { p, corner: false });
        }
    }
    Contour { pts: out, closed: c.closed }
}

fn twist(cs: Vec<Contour>, angle: f32, center: [f64; 2]) -> Vec<Contour> {
    if angle.abs() < 1e-3 || !angle.is_finite() {
        return cs;
    }
    let c = [center[0] as f32, center[1] as f32];
    let mut cs: Vec<Contour> = cs.iter().map(|k| densify(k, 3.0)).collect();
    let far = cs.iter().flat_map(|k| k.pts.iter()).map(|q| len(sub(q.p, c))).fold(1e-3f32, f32::max);
    for k in &mut cs {
        for q in &mut k.pts {
            let v = sub(q.p, c);
            let a = angle.to_radians() * (1.0 - len(v) / far);
            let (s, co) = a.sin_cos();
            q.p = add(c, [v[0] * co - v[1] * s, v[0] * s + v[1] * co]);
        }
    }
    cs
}

fn pucker_bloat(cs: Vec<Contour>, a: f32) -> Vec<Contour> {
    if a.abs() < 1e-4 || !a.is_finite() {
        return cs;
    }
    let all = cs.iter().flat_map(|k| k.pts.iter());
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in all {
        for j in 0..2 {
            lo[j] = lo[j].min(q.p[j]);
            hi[j] = hi[j].max(q.p[j]);
        }
    }
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let mut out = vec![];
    for c in &cs {
        let c = densify(c, 3.0);
        let mut pts = c.pts.clone();
        for run in runs(&c, true) {
            let ps: Vec<P> = run.iter().map(|&i| c.pts[i].p).collect();
            let cum = lengths(&ps);
            let total = cum.last().copied().unwrap_or(0.0).max(1e-6);
            // Runs share their end points, which get the same factor from either side.
            for (j, &i) in run.iter().enumerate() {
                let w = (std::f32::consts::PI * cum[j] / total).sin();
                let f = (1.0 + a * (w - 0.5)).max(0.0);
                pts[i].p = add(centre, scale(sub(c.pts[i].p, centre), f));
            }
        }
        out.push(Contour { pts, closed: c.closed });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Vectors

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}
fn scale(a: P, k: f32) -> P {
    [a[0] * k, a[1] * k]
}
fn dot(a: P, b: P) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: P, b: P) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}
fn len(a: P) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: P) -> P {
    let l = len(a);
    if l < 1e-12 { [1.0, 0.0] } else { scale(a, 1.0 / l) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ops(v: serde_json::Value) -> Vec<Operator> {
        serde_json::from_value(v).unwrap()
    }

    fn square() -> Path {
        super::super::paint::rect(100.0, 100.0, 0.0).unwrap()
    }

    fn bounds(p: &Path) -> (f32, f32, f32, f32) {
        let b = p.bounds();
        (b.left(), b.top(), b.right(), b.bottom())
    }

    #[test]
    fn offset_grows_and_shrinks() {
        for join in ["miter", "round", "bevel"] {
            let grown = apply(square(), &ops(json!([{"type": "offset", "amount": 10, "join": join}])), 0.0).unwrap();
            let (l, t, r, b) = bounds(&grown);
            assert!((l + 60.0).abs() < 0.5 && (r - 60.0).abs() < 0.5 && (t + 60.0).abs() < 0.5 && (b - 60.0).abs() < 0.5, "{join}: {l} {t} {r} {b}");
            assert!(contains(&grown, [55.0, 0.0]));
            // Round corners pull in at the diagonal; miters reach the corner.
            assert_eq!(contains(&grown, [58.0, 58.0]), join == "miter", "{join}");
        }
        let shrunk = apply(square(), &ops(json!([{"type": "offset", "amount": -10}])), 0.0).unwrap();
        let (l, _, r, _) = bounds(&shrunk);
        assert!((l + 40.0).abs() < 0.5 && (r - 40.0).abs() < 0.5, "{l} {r}");
        // The other winding grows too.
        let ccw = super::super::paint::svg_path("M-50 -50 L-50 50 L50 50 L50 -50 Z", &[], true).unwrap();
        let (l, ..) = bounds(&apply(ccw, &ops(json!([{"type": "offset", "amount": 10}])), 0.0).unwrap());
        assert!((l + 60.0).abs() < 0.5);
    }

    #[test]
    fn zigzag_wiggle_round_twist_bloat() {
        let z = apply(square(), &ops(json!([{"type": "zigzag", "size": 10, "ridges": 3}])), 0.0).unwrap();
        let (l, ..) = bounds(&z);
        assert!((l + 60.0).abs() < 0.5, "zig zags reach out by size: {l}");
        // 4 segments × 6 zig points + 4 corners.
        assert_eq!(flatten(&z)[0].pts.len(), 28);
        let smooth = apply(square(), &ops(json!([{"type": "zigzag", "size": 10, "ridges": 3, "points": "smooth"}])), 0.0).unwrap();
        assert!(flatten(&smooth)[0].pts.len() > 100);
        let total = apply(square(), &ops(json!([{"type": "zigzag", "size": 10, "ridges": 2, "perSegment": false}])), 0.0).unwrap();
        assert_eq!(flatten(&total)[0].pts.len(), 5);

        let w = |t: f64| apply(square(), &ops(json!([{"type": "wiggle", "size": 8, "detail": 10, "speed": 2}])), t).unwrap();
        assert_ne!(bounds(&w(0.0)), bounds(&square()), "wiggles");
        assert_eq!(bounds(&w(0.3)), bounds(&w(0.3)), "repeatable");
        assert_ne!(bounds(&w(0.0)), bounds(&w(0.7)), "moves with time");

        let r = apply(square(), &ops(json!([{"type": "roundCorners", "radius": 20}])), 0.0).unwrap();
        assert!(!contains(&r, [48.0, 48.0]) && contains(&r, [40.0, 40.0]) && contains(&r, [49.0, 0.0]));
        // A circle has no corners to round.
        let circle = super::super::paint::ellipse(100.0, 100.0).unwrap();
        let rc = apply(circle.clone(), &ops(json!([{"type": "roundCorners", "radius": 20}])), 0.0).unwrap();
        assert!((rc.bounds().width() - circle.bounds().width()).abs() < 0.5);

        let tw = apply(square(), &ops(json!([{"type": "twist", "angle": 90}])), 0.0).unwrap();
        assert_ne!(bounds(&tw), bounds(&square()));
        let bloat = apply(square(), &ops(json!([{"type": "puckerBloat", "amount": 50}])), 0.0).unwrap();
        assert!(contains(&bloat, [55.0, 0.0]), "edges bulge out");
        assert!(!contains(&bloat, [45.0, 45.0]), "corners pull in");
        let pucker = apply(square(), &ops(json!([{"type": "puckerBloat", "amount": -50}])), 0.0).unwrap();
        assert!(!contains(&pucker, [45.0, 0.0]) && contains(&pucker, [55.0, 55.0]));
    }

    #[test]
    fn repeater_lists_copies() {
        assert!(copies(&ops(json!([{"type": "offset"}]))).is_none());
        let c = copies(&ops(json!([{"type": "repeater", "copies": 3, "position": [10, 0], "startOpacity": 1, "endOpacity": 0.5}]))).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!((c[2].0.tx, c[2].1), (20.0, 0.5));
        let below = copies(&ops(json!([{"type": "repeater", "copies": 2.5, "composite": "below"}]))).unwrap();
        assert_eq!(below.len(), 3);
        assert_eq!((below[0].0.tx, below[0].1), (200.0, 0.5), "the last, half there, is drawn first");
        let nested = copies(&ops(json!([{"type": "repeater", "copies": 3}, {"type": "repeater", "copies": 2, "position": [0, 50]}]))).unwrap();
        assert_eq!(nested.len(), 6);
        assert!(copies(&ops(json!([{"type": "repeater", "copies": 0}]))).unwrap().is_empty());
        let odd = copies(&ops(json!([{"type": "repeater", "copies": 1000, "scale": 100, "rotation": -1e9}]))).unwrap();
        assert!(odd.iter().all(|(t, a)| t.is_finite() && a.is_finite()));
    }
}
