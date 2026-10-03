//! Text animators (`stack::ANIMATORS`) and text on a path, like After Effects' text layers.
//!
//! An animator picks units (letters, words or lines) with a selector — a range (start, end and
//! offset in percent of the text, shaped square, ramp up/down, triangle, round or smooth, in
//! order or shuffled) or a wiggly one (a random amount per unit that drifts with time) — and
//! moves, scales, turns, skews, fades, recolours, blurs or spaces each unit around its own
//! centre, as much as it is selected.

use kimchi_core::TextAlign;
use kimchi_core::motion::Animator;
use kimchi_core::motion::particles::Rand;
use tiny_skia::Transform;

use super::noise;
use crate::text::Layout;

/// What the animators do to one glyph.
#[derive(Debug, Clone)]
pub(crate) struct GlyphLook {
    /// Block pixels to block pixels (moves and turns around the units' centres).
    pub ts: Transform,
    pub alpha: f32,
    /// A colour (straight RGB 0–1) laid over the fill, and how much of it (0–1).
    pub fill: Option<([f32; 3], f32)>,
    /// Blur radius, block pixels.
    pub blur: f32,
    /// How far along its line tracking pushes it, block pixels.
    pub shift: f32,
}

impl Default for GlyphLook {
    fn default() -> Self {
        GlyphLook { ts: Transform::identity(), alpha: 1.0, fill: None, blur: 0.0, shift: 0.0 }
    }
}

/// The look of every glyph of `layout` at time `t` (the layer list's seconds).
pub(crate) fn looks(animators: &[Animator], layout: &Layout, t: f64, align: TextAlign) -> Vec<GlyphLook> {
    let mut out = vec![GlyphLook::default(); layout.glyphs.len()];
    // Tracking of each glyph's unit, per animator, to add up along the lines afterwards.
    let mut tracks: Vec<(Vec<f32>, Vec<usize>)> = vec![];
    for a in animators.iter().filter(|a| a.enabled) {
        let by = a.s("by");
        let unit_of = |g: &crate::text::Glyph| match by.as_str() {
            "word" => g.unit.words,
            "line" => g.unit.lines,
            _ => g.unit.chars,
        };
        let count = match by.as_str() {
            "word" => layout.units.words,
            "line" => layout.units.lines,
            _ => layout.units.chars,
        }
        .max(1);
        // Each unit's centre: the middle of its glyphs' boxes.
        let mut span = vec![(f32::MAX, f32::MIN, 0.0f32); count];
        for g in &layout.glyphs {
            let u = unit_of(g).min(count - 1);
            let s = &mut span[u];
            s.0 = s.0.min(g.center.0 - g.advance / 2.0);
            s.1 = s.1.max(g.center.0 + g.advance / 2.0);
            s.2 = g.center.1;
        }
        let select = selector(a, count, t);
        let n = |name: &str| a.n(name) as f32;
        let (x, y, scale, rot, op, blur, track, skew) = (n("x"), n("y"), n("scale"), n("rotation"), n("opacity"), n("blur"), n("tracking"), n("skew"));
        let fill = rgba(&a.s("fill"));
        let mut unit_track = vec![0.0; layout.glyphs.len()];
        let mut units = vec![0; layout.glyphs.len()];
        for (i, g) in layout.glyphs.iter().enumerate() {
            let u = unit_of(g).min(count - 1);
            units[i] = u;
            let v = select[u] * n("amount");
            if v == 0.0 {
                continue;
            }
            let (cx, cy) = if by == "char" { g.center } else { ((span[u].0 + span[u].1) / 2.0, span[u].2) };
            let sc = (1.0 + (scale - 1.0) * v).max(0.0);
            let k = (skew * v).clamp(-85.0, 85.0).to_radians().tan();
            let m = Transform::from_translate(x * v + cx, y * v + cy)
                .pre_rotate(rot * v)
                .pre_concat(Transform::from_row(1.0, 0.0, k, 1.0, 0.0, 0.0))
                .pre_scale(sc, sc)
                .pre_translate(-cx, -cy);
            let look = &mut out[i];
            look.ts = look.ts.pre_concat(m);
            look.alpha *= (1.0 + (op - 1.0) * v.abs()).clamp(0.0, 1.0);
            look.blur += blur * v.abs();
            if fill[3] > 0.0 {
                let mix = (fill[3] * v.abs()).clamp(0.0, 1.0);
                let rgb = [fill[0], fill[1], fill[2]];
                look.fill = Some(match look.fill {
                    Some((c, m0)) => (std::array::from_fn(|j| c[j] + (rgb[j] - c[j]) * mix), m0 + (1.0 - m0) * mix),
                    None => (rgb, mix),
                });
            }
            unit_track[i] = track * v;
        }
        if track != 0.0 {
            tracks.push((unit_track, units));
        }
    }
    // Tracking: each unit pushes the ones after it on its line; lines re-centre as aligned.
    if !tracks.is_empty() {
        let lines = layout.units.lines.max(1);
        for line in 0..lines {
            let idx: Vec<usize> = (0..layout.glyphs.len()).filter(|&i| layout.glyphs[i].unit.lines == line).collect();
            let mut total = 0.0;
            for (trk, units) in &tracks {
                let mut acc = 0.0;
                let mut prev: Option<(usize, f32)> = None;
                for &i in &idx {
                    if let Some((u, t)) = prev
                        && u != units[i]
                    {
                        acc += t;
                    }
                    prev = Some((units[i], trk[i]));
                    out[i].shift += acc;
                }
                total += acc + prev.map_or(0.0, |p| p.1);
            }
            let back = match align {
                TextAlign::Left => 0.0,
                TextAlign::Center => total / 2.0,
                TextAlign::Right => total,
            };
            for &i in &idx {
                out[i].shift -= back;
            }
        }
    }
    out
}

/// How much each unit is selected (before `amount`): 0–1 for ranges, −1–1 for wiggly.
fn selector(a: &Animator, count: usize, t: f64) -> Vec<f32> {
    let n = count as f32;
    if a.kind == "wiggly" {
        let seed = a.n("seed") as u32;
        let ts = (t * a.n("speed")) as f32;
        let corr = a.n("correlation") as f32;
        let shared = noise::noise1(ts, seed);
        return (0..count)
            .map(|i| {
                let own = noise::noise1(ts + i as f32 * 13.37, seed.wrapping_add(1 + i as u32));
                own + (shared - own) * corr
            })
            .collect();
    }
    let offset = a.n("offset") as f32;
    let (mut s, mut e) = ((a.n("start") as f32 + offset) / 100.0, (a.n("end") as f32 + offset) / 100.0);
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    // Shuffled order: unit i is treated as if it were at place order[i].
    let order: Vec<usize> = if a.b("randomize") {
        let mut r = Rand::new(a.n("seed") as u64, 0x7e47);
        let mut o: Vec<usize> = (0..count).collect();
        for i in (1..count).rev() {
            o.swap(i, (r.u() % (i as u64 + 1)) as usize);
        }
        o
    } else {
        (0..count).collect()
    };
    let shape = a.s("shape");
    let smooth = (a.n("smoothness") / 100.0) as f32;
    order
        .iter()
        .map(|&at| {
            let (lo, hi) = (at as f32 / n, (at + 1) as f32 / n);
            match shape.as_str() {
                "square" => {
                    let cover = ((e.min(hi) - s.max(lo)) * n).clamp(0.0, 1.0);
                    let hard = if cover >= 0.5 { 1.0 } else { 0.0 };
                    hard + (cover - hard) * smooth
                }
                _ => {
                    let u = (lo + hi) / 2.0;
                    let x = if e - s > 1e-6 { (u - s) / (e - s) } else if u < s { -1.0 } else { 2.0 };
                    let inside = (0.0..=1.0).contains(&x);
                    match shape.as_str() {
                        "rampUp" => x.clamp(0.0, 1.0),
                        "rampDown" => 1.0 - x.clamp(0.0, 1.0),
                        "triangle" if inside => 1.0 - (2.0 * x - 1.0).abs(),
                        "round" if inside => (1.0 - (2.0 * x - 1.0).powi(2)).max(0.0).sqrt(),
                        "smooth" if inside => 0.5 - 0.5 * (std::f32::consts::TAU * x).cos(),
                        _ => 0.0,
                    }
                }
            }
        })
        .collect()
}

fn rgba(s: &str) -> [f32; 4] {
    let c = kimchi_core::motion::stack::rgba(s).0;
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

/// A path text follows, walked by length.
pub(crate) struct PathWalk {
    pts: Vec<[f32; 2]>,
    cum: Vec<f32>,
    closed: bool,
}

impl PathWalk {
    /// The first contour of SVG path data (layer pixels); `None` when there is nothing to follow.
    pub(crate) fn new(d: &str) -> Option<PathWalk> {
        let path = super::paint::svg_path(d, &[], false)?;
        let (mut pts, closed) = super::shapeops::polylines(&path).into_iter().next()?;
        if closed && let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        if pts.len() < 2 {
            return None;
        }
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            let l = cum.last().copied().unwrap_or(0.0) + ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
            cum.push(l);
        }
        (cum.last().copied().unwrap_or(0.0) > 1e-3).then_some(PathWalk { pts, cum, closed })
    }

    pub(crate) fn length(&self) -> f32 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// The point `s` along the path and the angle (degrees) of its direction there. Closed
    /// paths wrap; open ones carry on straight past their ends.
    pub(crate) fn at(&self, s: f32) -> ([f32; 2], f32) {
        let total = self.length();
        let s = if self.closed { s.rem_euclid(total) } else { s };
        let last = self.pts.len() - 2;
        let i = if s <= 0.0 {
            0
        } else if s >= total {
            last
        } else {
            match self.cum.binary_search_by(|v| v.total_cmp(&s)) {
                Ok(i) => i.min(last),
                Err(i) => i.saturating_sub(1).min(last),
            }
        };
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        let span = (self.cum[i + 1] - self.cum[i]).max(1e-6);
        let k = (s - self.cum[i]) / span;
        let p = [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
        (p, (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn animator(v: serde_json::Value) -> Animator {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn range_shapes_select_the_right_units() {
        let sel = |v: serde_json::Value| selector(&animator(v), 4, 0.0);
        assert_eq!(sel(json!({"start": 0, "end": 25})), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(sel(json!({"start": 0, "end": 25, "offset": 50})), [0.0, 0.0, 1.0, 0.0]);
        // Half of a unit inside: hard with no smoothness, half with full.
        assert_eq!(sel(json!({"start": 0, "end": 37.5, "smoothness": 0}))[1], 1.0);
        assert_eq!(sel(json!({"start": 0, "end": 37.5}))[1], 0.5);
        let up = sel(json!({"shape": "rampUp"}));
        assert!(up[0] < up[1] && up[1] < up[2] && up[2] < up[3]);
        let down = sel(json!({"shape": "rampDown"}));
        assert!(down[0] > down[3]);
        let tri = sel(json!({"shape": "triangle"}));
        assert!(tri[1] > tri[0] && tri[2] > tri[3]);
        assert!(sel(json!({"shape": "round"})).iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(sel(json!({"shape": "smooth"})).iter().all(|v| (0.0..=1.0).contains(v)));
        let shuffled = sel(json!({"start": 0, "end": 25, "randomize": true, "seed": 3}));
        assert_eq!(shuffled.iter().filter(|v| **v == 1.0).count(), 1);
        let w = |t: f64| selector(&animator(json!({"type": "wiggly", "speed": 2})), 4, t);
        assert_ne!(w(0.3), w(0.9));
        assert_eq!(w(0.3), w(0.3));
        let together = selector(&animator(json!({"type": "wiggly", "correlation": 1})), 4, 0.4);
        assert!(together.windows(2).all(|p| (p[0] - p[1]).abs() < 1e-6));
    }

    #[test]
    fn paths_walk_by_length() {
        let p = PathWalk::new("M0 0 L100 0 L100 100").unwrap();
        assert!((p.length() - 200.0).abs() < 1e-3);
        let (pt, a) = p.at(150.0);
        assert!((pt[0] - 100.0).abs() < 1e-3 && (pt[1] - 50.0).abs() < 1e-3 && (a - 90.0).abs() < 1e-3);
        let (past, _) = p.at(250.0);
        assert!((past[1] - 150.0).abs() < 1e-3, "open paths carry on");
        let ring = PathWalk::new("M50 0 A50 50 0 1 1 -50 0 A50 50 0 1 1 50 0 Z").unwrap();
        let (a, _) = ring.at(0.0);
        let (b, _) = ring.at(ring.length());
        assert!((a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2, "closed paths wrap");
        assert!(PathWalk::new("").is_none());
    }
}
