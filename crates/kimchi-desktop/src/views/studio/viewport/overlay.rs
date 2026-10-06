//! Clip viewport decorations before converting scene-sized f64 coordinates to GPUI's f32 paths.

use super::Mark;
use gpui::{Bounds, Hsla, PathBuilder, Pixels, Window, point, px};

type Point = [f64; 2];
type Rect = [Point; 2];

fn finite(p: Point) -> bool {
    p.iter().all(|n| n.is_finite())
}

fn inside(p: Point, r: Rect) -> bool {
    finite(p) && (0..2).all(|i| p[i] >= r[0][i] && p[i] <= r[1][i])
}

fn intersection(a: Point, b: Point, axis: usize, bound: f64) -> Point {
    // Normalise first: subtracting two large but finite endpoints can overflow.
    let scale = a[axis].abs().max(b[axis].abs()).max(bound.abs()).max(1.);
    let t = ((bound / scale - a[axis] / scale) / (b[axis] / scale - a[axis] / scale)).clamp(0., 1.);
    let mut p = std::array::from_fn(|i| a[i] * (1. - t) + b[i] * t);
    p[axis] = bound;
    p
}

fn segment(mut a: Point, mut b: Point, r: Rect) -> Option<[Point; 2]> {
    if !finite(a) || !finite(b) {
        return None;
    }
    for axis in 0..2 {
        for (bound, lower) in [(r[0][axis], true), (r[1][axis], false)] {
            let outside = |p: Point| {
                if lower {
                    p[axis] < bound
                } else {
                    p[axis] > bound
                }
            };
            match (outside(a), outside(b)) {
                (true, true) => return None,
                (true, false) => a = intersection(a, b, axis, bound),
                (false, true) => b = intersection(a, b, axis, bound),
                _ => {}
            }
        }
    }
    (inside(a, r) && inside(b, r)).then_some([a, b])
}

fn polygon(mut points: Vec<Point>, r: Rect) -> Vec<Point> {
    if points.iter().any(|&p| !finite(p)) {
        return vec![];
    }
    for axis in 0..2 {
        for (bound, lower) in [(r[0][axis], true), (r[1][axis], false)] {
            let Some(mut previous) = points.last().copied() else {
                return points;
            };
            let kept = |p: Point| {
                if lower {
                    p[axis] >= bound
                } else {
                    p[axis] <= bound
                }
            };
            let mut out = vec![];
            for p in points {
                if kept(previous) != kept(p) {
                    out.push(intersection(previous, p, axis, bound));
                }
                if kept(p) {
                    out.push(p);
                }
                previous = p;
            }
            points = out;
        }
    }
    if points.iter().all(|&p| inside(p, r)) {
        points
    } else {
        vec![]
    }
}

fn pt(p: Point) -> gpui::Point<Pixels> {
    point(px(p[0] as f32), px(p[1] as f32))
}

fn fill(points: Vec<Point>, color: Hsla, rect: Rect, window: &mut Window) {
    let points = polygon(points, rect);
    if points.len() < 3 {
        return;
    }
    let mut b = PathBuilder::fill();
    b.move_to(pt(points[0]));
    for p in &points[1..] {
        b.line_to(pt(*p));
    }
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

pub(super) fn paint_marks(marks: Vec<Mark>, bounds: Bounds<Pixels>, window: &mut Window) {
    // Leave room for stroke widths. The canvas's content mask handles the exact edge.
    let rect = [
        [
            f32::from(bounds.origin.x) as f64 - 8.,
            f32::from(bounds.origin.y) as f64 - 8.,
        ],
        [
            f32::from(bounds.right()) as f64 + 8.,
            f32::from(bounds.bottom()) as f64 + 8.,
        ],
    ];
    for mark in marks {
        match mark {
            Mark::Line(points, closed, color, width) => {
                if points.len() < 2 {
                    continue;
                }
                let mut b = PathBuilder::stroke(px(width));
                if points.iter().all(|&p| inside(p, rect)) {
                    b.move_to(pt(points[0]));
                    for p in &points[1..] {
                        b.line_to(pt(*p));
                    }
                    if closed {
                        b.close();
                    }
                } else {
                    let mut previous = None;
                    let count = points.len() - usize::from(!closed);
                    for i in 0..count {
                        if let Some([a, z]) =
                            segment(points[i], points[(i + 1) % points.len()], rect)
                        {
                            if previous != Some(a) {
                                b.move_to(pt(a));
                            }
                            b.line_to(pt(z));
                            previous = Some(z);
                        } else {
                            previous = None;
                        }
                    }
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, color);
                }
            }
            Mark::Fill(points, color) => fill(points, color, rect, window),
            Mark::Dot(c, r, color) => {
                if !finite(c) || !r.is_finite() || r <= 0. {
                    continue;
                }
                let points = (0..16)
                    .map(|k| {
                        let a = k as f64 / 16. * std::f64::consts::TAU;
                        [c[0] + a.cos() * r, c[1] + a.sin() * r]
                    })
                    .collect();
                fill(points, color, rect, window);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_and_non_finite_decorations_never_reach_the_path_builder() {
        let rect = [[0., 0.], [800., 600.]];
        for distance in [1e6, 1e99, 1e308] {
            assert_eq!(
                segment([-distance, 300.], [distance, 300.], rect),
                Some([[0., 300.], [800., 300.]])
            );
            assert_eq!(segment([distance, 100.], [distance, 200.], rect), None);
            let p = polygon(
                vec![
                    [-distance, -distance],
                    [distance, -distance],
                    [distance, distance],
                    [-distance, distance],
                ],
                rect,
            );
            assert_eq!(p.len(), 4);
            assert!(p.iter().all(|&v| inside(v, rect)));
        }
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(segment([invalid, 20.], [40., 20.], rect), None);
            assert!(polygon(vec![[invalid, 20.], [40., 20.], [10., 10.]], rect).is_empty());
        }
        let triangle = vec![[20., 30.], [40., 70.], [60., 30.]];
        assert_eq!(polygon(triangle.clone(), rect), triangle);
    }
}
