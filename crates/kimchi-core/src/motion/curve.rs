//! 3D curves through points: smooth (Catmull-Rom, passing through every point) or straight,
//! open or closed, walked by length so `0..1` moves at an even speed. Tubes (curve objects with
//! a radius) and the followPath constraint both use this, so an object following a curve stays
//! on the tube drawn along it.

/// A curve sampled into a dense polyline with the length up to each point.
#[derive(Debug, Clone)]
pub struct Polyline {
    pub points: Vec<[f64; 3]>,
    /// Length from the start to each point.
    pub lengths: Vec<f64>,
    pub closed: bool,
}

/// Samples per segment between two control points.
const STEPS: usize = 24;

impl Polyline {
    pub fn new(points: &[[f64; 3]], closed: bool, smooth: bool) -> Polyline {
        let n = points.len();
        let mut out = vec![];
        if n == 0 {
            return Polyline { points: vec![[0.0; 3]], lengths: vec![0.0], closed: false };
        }
        let segs = if closed { n } else { n.saturating_sub(1) };
        let get = |i: isize| -> [f64; 3] {
            if closed {
                points[i.rem_euclid(n as isize) as usize]
            } else {
                points[i.clamp(0, n as isize - 1) as usize]
            }
        };
        for s in 0..segs {
            let (p0, p1, p2, p3) = (get(s as isize - 1), get(s as isize), get(s as isize + 1), get(s as isize + 2));
            for k in 0..STEPS {
                let t = k as f64 / STEPS as f64;
                out.push(if smooth { catmull_rom(p0, p1, p2, p3, t) } else { lerp(p1, p2, t) });
            }
        }
        out.push(if closed { points[0] } else { points[n - 1] });
        let mut lengths = vec![0.0];
        for w in out.windows(2) {
            let l = lengths.last().copied().unwrap_or(0.0) + dist(w[0], w[1]);
            lengths.push(l);
        }
        Polyline { points: out, lengths, closed }
    }

    pub fn length(&self) -> f64 {
        self.lengths.last().copied().unwrap_or(0.0)
    }

    /// The point and unit direction at `u` (0 = start, 1 = end) of the length. Closed curves
    /// wrap; open ones stop at their ends.
    pub fn at(&self, u: f64) -> ([f64; 3], [f64; 3]) {
        let total = self.length();
        if self.points.len() < 2 || total <= 0.0 {
            return (self.points[0], [0.0, 0.0, 1.0]);
        }
        let u = if self.closed { u.rem_euclid(1.0) } else { u.clamp(0.0, 1.0) };
        let want = u * total;
        let i = match self.lengths.binary_search_by(|l| l.total_cmp(&want)) {
            Ok(i) => i.min(self.points.len() - 2),
            Err(i) => i.saturating_sub(1).min(self.points.len() - 2),
        };
        let (a, b) = (self.points[i], self.points[i + 1]);
        let span = (self.lengths[i + 1] - self.lengths[i]).max(1e-12);
        let k = ((want - self.lengths[i]) / span).clamp(0.0, 1.0);
        (lerp(a, b, k), normalize(sub(b, a)))
    }

    /// The part of the curve from `from` to `to` (0–1 of the length), as points.
    pub fn trimmed(&self, from: f64, to: f64) -> Vec<[f64; 3]> {
        let (a, b) = (from.clamp(0.0, 1.0), to.clamp(0.0, 1.0));
        if b <= a {
            return vec![];
        }
        let total = self.length();
        let mut out = vec![self.at(a).0];
        for (p, l) in self.points.iter().zip(&self.lengths) {
            let u = if total > 0.0 { l / total } else { 0.0 };
            if u > a && u < b {
                out.push(*p);
            }
        }
        out.push(self.at(b).0);
        out
    }
}

fn catmull_rom(p0: [f64; 3], p1: [f64; 3], p2: [f64; 3], p3: [f64; 3], t: f64) -> [f64; 3] {
    let (t2, t3) = (t * t, t * t * t);
    std::array::from_fn(|i| {
        0.5 * ((2.0 * p1[i]) + (-p0[i] + p2[i]) * t + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * t2 + (-p0[i] + 3.0 * p1[i] - 3.0 * p2[i] + p3[i]) * t3)
    })
}

fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-12 { [0.0, 0.0, 1.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_by_length_through_the_points() {
        let line = Polyline::new(&[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]], false, false);
        assert!((line.length() - 20.0).abs() < 1e-9);
        let (p, d) = line.at(0.25);
        assert!((p[0] - 5.0).abs() < 1e-9 && d[0] > 0.99);
        let (p, d) = line.at(0.75);
        assert!((p[1] - 5.0).abs() < 1e-9 && d[1] > 0.99);
        let smooth = Polyline::new(&[[0.0, 0.0, 0.0], [5.0, 5.0, 0.0], [10.0, 0.0, 0.0]], false, true);
        assert!(smooth.points.iter().any(|p| (p[0] - 5.0).abs() < 1e-9 && (p[1] - 5.0).abs() < 1e-9), "passes through its points");
        let ring = Polyline::new(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]], true, true);
        let (a, _) = ring.at(0.0);
        let (b, _) = ring.at(1.0);
        assert!(dist(a, b) < 1e-9, "closed curves wrap");
        assert_eq!(line.trimmed(0.0, 0.5).last().copied().map(|p| p[0].round()), Some(10.0));
    }
}
