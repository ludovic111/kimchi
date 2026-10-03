//! Drawing helpers shared by the compositor and the 2D motion renderer: colours, fills,
//! shape outlines, path length and trimming, blur, shadow and glow.

use kimchi_core::motion::{Fill, Gradient};
use kimchi_core::path::Seg;
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, PathSegment, Pixmap, PixmapPaint, Point,
    RadialGradient, Rect, Shader, SpreadMode, Transform,
};

use crate::text::{parse_color, try_color};

/// A colour from `#rrggbb[aa]` (or the few CSS forms the text renderer knows); `transparent` too.
pub(crate) fn color(c: &str) -> Color {
    if c.trim().eq_ignore_ascii_case("transparent") {
        return Color::TRANSPARENT;
    }
    try_color(c).unwrap_or_else(|| parse_color(c))
}

pub(crate) fn with_alpha(mut c: Color, a: f32) -> Color {
    c.apply_opacity(a.clamp(0.0, 1.0));
    c
}

/// A fill in a shape's own coordinates: a colour or a gradient across `bounds`.
#[derive(Clone)]
pub(crate) enum FillSpec {
    Solid(Color),
    Linear { from: Point, to: Point, stops: Vec<(f32, Color)> },
    Radial { center: Point, radius: f32, stops: Vec<(f32, Color)> },
}

impl FillSpec {
    /// From a layer's `fill`; gradients default to spanning `bounds` (left to right, or centre out).
    pub(crate) fn of(fill: &Fill, bounds: Rect) -> FillSpec {
        match fill {
            Fill::Color(c) => FillSpec::Solid(color(c)),
            Fill::Gradient(g) => gradient(g, bounds),
        }
    }

    /// A paint with every colour's alpha multiplied by `alpha`.
    pub(crate) fn paint(&self, alpha: f32) -> Paint<'static> {
        let shader = match self {
            FillSpec::Solid(c) => Shader::SolidColor(with_alpha(*c, alpha)),
            FillSpec::Linear { from, to, stops } => {
                LinearGradient::new(*from, *to, stops_of(stops, alpha), SpreadMode::Pad, Transform::identity())
                    .unwrap_or(Shader::SolidColor(with_alpha(stops.first().map_or(Color::BLACK, |s| s.1), alpha)))
            }
            FillSpec::Radial { center, radius, stops } => {
                RadialGradient::new(*center, *center, radius.max(0.01), stops_of(stops, alpha), SpreadMode::Pad, Transform::identity())
                    .unwrap_or(Shader::SolidColor(with_alpha(stops.first().map_or(Color::BLACK, |s| s.1), alpha)))
            }
        };
        Paint { shader, anti_alias: true, ..Paint::default() }
    }
}

fn stops_of(stops: &[(f32, Color)], alpha: f32) -> Vec<GradientStop> {
    stops.iter().map(|(p, c)| GradientStop::new(p.clamp(0.0, 1.0), with_alpha(*c, alpha))).collect()
}

fn gradient(g: &Gradient, b: Rect) -> FillSpec {
    let mut stops: Vec<(f32, Color)> = g.stops.iter().map(|(p, c)| (*p as f32, color(c))).collect();
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let pt = |p: [f64; 2]| Point::from_xy(p[0] as f32, p[1] as f32);
    if g.kind == "radial" {
        let center = g.center.map(pt).unwrap_or(Point::from_xy(b.x() + b.width() / 2.0, b.y() + b.height() / 2.0));
        let radius = g.radius.map(|r| r as f32).unwrap_or(b.width().max(b.height()) / 2.0);
        FillSpec::Radial { center, radius, stops }
    } else {
        let from = g.from.map(pt).unwrap_or(Point::from_xy(b.left(), b.y() + b.height() / 2.0));
        let to = g.to.map(pt).unwrap_or(Point::from_xy(b.right(), b.y() + b.height() / 2.0));
        FillSpec::Linear { from, to, stops }
    }
}

pub(crate) fn blend(b: kimchi_core::motion::Blend) -> BlendMode {
    use kimchi_core::motion::Blend::*;
    match b {
        Normal => BlendMode::SourceOver,
        Multiply => BlendMode::Multiply,
        Screen => BlendMode::Screen,
        Overlay => BlendMode::Overlay,
        Add => BlendMode::Plus,
        Darken => BlendMode::Darken,
        Lighten => BlendMode::Lighten,
        Difference => BlendMode::Difference,
        ColorDodge => BlendMode::ColorDodge,
        ColorBurn => BlendMode::ColorBurn,
        SoftLight => BlendMode::SoftLight,
        HardLight => BlendMode::HardLight,
    }
}

// ---------------------------------------------------------------------------------------------
// Outlines

pub(crate) fn rect(w: f32, h: f32, r: f32) -> Option<Path> {
    crate::text::round_rect(-w / 2.0, -h / 2.0, w, h, r)
}

pub(crate) fn ellipse(w: f32, h: f32) -> Option<Path> {
    PathBuilder::from_oval(Rect::from_xywh(-w / 2.0, -h / 2.0, w, h)?)
}

/// A regular polygon with a point at the top; `roundness` 0–1 rounds its corners.
pub(crate) fn polygon(sides: f32, radius: f32, roundness: f32) -> Option<Path> {
    let n = sides.round().clamp(3.0, 64.0) as usize;
    let pts: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / n as f32;
            (radius * a.cos(), radius * a.sin())
        })
        .collect();
    rounded_closed(&pts, roundness)
}

pub(crate) fn star(points: f32, radius: f32, inner: f32) -> Option<Path> {
    let n = points.round().clamp(2.0, 64.0) as usize;
    let pts: Vec<(f32, f32)> = (0..n * 2)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / n as f32;
            let r = if i % 2 == 0 { radius } else { inner };
            (r * a.cos(), r * a.sin())
        })
        .collect();
    rounded_closed(&pts, 0.0)
}

fn rounded_closed(pts: &[(f32, f32)], roundness: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let k = roundness.clamp(0.0, 1.0) * 0.5;
    if k <= 0.0 {
        pb.move_to(pts[0].0, pts[0].1);
        for p in &pts[1..] {
            pb.line_to(p.0, p.1);
        }
        pb.close();
        return pb.finish();
    }
    let n = pts.len();
    let lerp = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    for i in 0..n {
        let (prev, cur, next) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let a = lerp(cur, prev, k);
        let b = lerp(cur, next, k);
        if i == 0 {
            pb.move_to(a.0, a.1);
        } else {
            pb.line_to(a.0, a.1);
        }
        pb.quad_to(cur.0, cur.1, b.0, b.1);
    }
    pb.close();
    pb.finish()
}

/// SVG path data or a polyline, in the layer's own pixels.
pub(crate) fn svg_path(d: &str, points: &[[f64; 2]], closed: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    if !d.trim().is_empty() {
        for seg in kimchi_core::path::parse(d).ok()? {
            let f = |p: [f64; 2]| (p[0] as f32, p[1] as f32);
            match seg {
                Seg::Move(p) => pb.move_to(f(p).0, f(p).1),
                Seg::Line(p) => pb.line_to(f(p).0, f(p).1),
                Seg::Quad(c, p) => pb.quad_to(f(c).0, f(c).1, f(p).0, f(p).1),
                Seg::Cubic(a, b, p) => pb.cubic_to(f(a).0, f(a).1, f(b).0, f(b).1, f(p).0, f(p).1),
                Seg::Close => pb.close(),
            }
        }
    } else if let Some(first) = points.first() {
        pb.move_to(first[0] as f32, first[1] as f32);
        for p in &points[1..] {
            pb.line_to(p[0] as f32, p[1] as f32);
        }
        if closed {
            pb.close();
        }
    }
    pb.finish()
}

/// Length of every contour of `path` together (curves flattened).
pub(crate) fn length(path: &Path) -> f32 {
    let mut total = 0.0;
    let mut last = Point::zero();
    let mut start = Point::zero();
    let dist = |a: Point, b: Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                last = p;
                start = p;
            }
            PathSegment::LineTo(p) => {
                total += dist(last, p);
                last = p;
            }
            PathSegment::QuadTo(c, p) => {
                let mut prev = last;
                for i in 1..=16 {
                    let t = i as f32 / 16.0;
                    let m = 1.0 - t;
                    let q = Point::from_xy(m * m * last.x + 2.0 * m * t * c.x + t * t * p.x, m * m * last.y + 2.0 * m * t * c.y + t * t * p.y);
                    total += dist(prev, q);
                    prev = q;
                }
                last = p;
            }
            PathSegment::CubicTo(a, b, p) => {
                let mut prev = last;
                for i in 1..=24 {
                    let t = i as f32 / 24.0;
                    let m = 1.0 - t;
                    let q = Point::from_xy(
                        m * m * m * last.x + 3.0 * m * m * t * a.x + 3.0 * m * t * t * b.x + t * t * t * p.x,
                        m * m * m * last.y + 3.0 * m * m * t * a.y + 3.0 * m * t * t * b.y + t * t * t * p.y,
                    );
                    total += dist(prev, q);
                    prev = q;
                }
                last = p;
            }
            PathSegment::Close => {
                total += dist(last, start);
                last = start;
            }
        }
    }
    total
}

/// A dash pattern that shows only `[start, end]` (fractions of the outline, after `offset`, which
/// wraps). `None` when the whole outline shows; `Some(None)` when nothing does.
pub(crate) fn trim_dash(path: &Path, start: f64, end: f64, offset: f64) -> Option<Option<tiny_skia::StrokeDash>> {
    let (mut a, mut b) = (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0));
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    if a <= 1e-6 && b >= 1.0 - 1e-6 && offset.abs() < 1e-9 {
        return None;
    }
    if b - a <= 1e-6 {
        return Some(None);
    }
    let len = length(path).max(1e-3) as f64;
    let visible = (b - a) * len;
    let total = visible + len;
    let shift = (a + offset.rem_euclid(1.0)) * len;
    let phase = (total - shift).rem_euclid(total);
    Some(tiny_skia::StrokeDash::new(vec![visible as f32, len as f32], phase as f32))
}

// ---------------------------------------------------------------------------------------------
// Blur and its uses

/// Gaussian blur (radius ≈ 2σ) of a premultiplied pixmap, in place. Large radii are blurred on a
/// smaller copy and scaled back, which looks the same and is much faster.
pub(crate) fn blur(p: &mut Pixmap, radius: f32) {
    if radius < 0.5 {
        return;
    }
    let factor = ((radius / 6.0).floor() as u32).clamp(1, 8);
    if factor > 1 {
        let (w, h) = ((p.width() / factor).max(1), (p.height() / factor).max(1));
        let Some(mut small) = Pixmap::new(w, h) else { return };
        let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
        let k = 1.0 / factor as f32;
        small.draw_pixmap(0, 0, p.as_ref(), &paint, Transform::from_scale(k, k), None);
        blur_exact(&mut small, radius / factor as f32);
        p.fill(Color::TRANSPARENT);
        let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, blend_mode: BlendMode::Source, ..PixmapPaint::default() };
        p.draw_pixmap(0, 0, small.as_ref(), &paint, Transform::from_scale(p.width() as f32 / w as f32, p.height() as f32 / h as f32), None);
    } else {
        blur_exact(p, radius);
    }
}

/// Three box blurs per channel over the area that has any ink.
fn blur_exact(p: &mut Pixmap, radius: f32) {
    let sigma = radius / 2.0;
    if sigma < 0.3 {
        return;
    }
    let (w, h) = (p.width() as usize, p.height() as usize);
    let alpha: Vec<u8> = p.pixels().iter().map(|c| c.alpha()).collect();
    let Some((x0, y0, x1, y1)) = crate::text::ink_bounds(&alpha, w, h) else { return };
    let margin = (sigma * 3.0).ceil() as usize + 2;
    let (x0, y0, x1, y1) = (x0.saturating_sub(margin), y0.saturating_sub(margin), (x1 + margin).min(w - 1), (y1 + margin).min(h - 1));
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let radii = crate::text::box_radii(sigma);
    let data = p.data_mut();
    let mut chan = vec![0f32; bw * bh];
    for c in 0..4 {
        for (i, v) in chan.iter_mut().enumerate() {
            *v = data[((y0 + i / bw) * w + x0 + i % bw) * 4 + c] as f32;
        }
        for r in radii {
            crate::text::box_blur(&mut chan, bw, bh, r);
        }
        for (i, v) in chan.iter().enumerate() {
            data[((y0 + i / bw) * w + x0 + i % bw) * 4 + c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    // Keep premultiplied invariants (colour ≤ alpha) after rounding.
    for px in data.as_chunks_mut::<4>().0.iter_mut() {
        let a = px[3];
        px[0] = px[0].min(a);
        px[1] = px[1].min(a);
        px[2] = px[2].min(a);
    }
}

/// A copy of `layer`'s alpha in `tint`, times `opacity`.
pub(crate) fn tinted(layer: &Pixmap, tint: Color, opacity: f32) -> Pixmap {
    let mut out = Pixmap::new(layer.width(), layer.height()).expect("same size");
    let c = tint.premultiply().to_color_u8();
    let k = opacity.clamp(0.0, 4.0);
    for (dst, src) in out.pixels_mut().iter_mut().zip(layer.pixels()) {
        let a = src.alpha() as f32 / 255.0 * k;
        if a > 0.0 {
            let m = |v: u8| (v as f32 * a.min(1.0)).round().clamp(0.0, 255.0) as u8;
            if let Some(px) = tiny_skia::PremultipliedColorU8::from_rgba(m(c.red()), m(c.green()), m(c.blue()), m(c.alpha())) {
                *dst = px;
            }
        }
    }
    out
}

/// Draws `layer` onto `canvas` with a soft shadow (colour, blur, offset in canvas pixels) under it.
pub(crate) fn shadow_under(canvas: &mut Pixmap, layer: &Pixmap, tint: Color, radius: f32, dx: f32, dy: f32) {
    let mut s = tinted(layer, tint, 1.0);
    blur(&mut s, radius);
    canvas.draw_pixmap(0, 0, s.as_ref(), &PixmapPaint::default(), Transform::from_translate(dx, dy), None);
}

/// A blurred, coloured copy of `layer`, added (light) under it.
pub(crate) fn glow_under(canvas: &mut Pixmap, layer: &Pixmap, tint: Color, radius: f32, strength: f32) {
    let mut g = tinted(layer, tint, strength);
    blur(&mut g, radius);
    let paint = PixmapPaint { blend_mode: BlendMode::Plus, ..PixmapPaint::default() };
    canvas.draw_pixmap(0, 0, g.as_ref(), &paint, Transform::identity(), None);
}

/// Fills `path` with `fill` (non-zero rule).
pub(crate) fn fill(canvas: &mut Pixmap, path: &Path, fill: &FillSpec, alpha: f32, ts: Transform, mask: Option<&tiny_skia::Mask>) {
    canvas.fill_path(path, &fill.paint(alpha), FillRule::Winding, ts, mask);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_measures() {
        let line = svg_path("M0 0 L100 0", &[], false).unwrap();
        assert!((length(&line) - 100.0).abs() < 1e-3);
        let circle = ellipse(100.0, 100.0).unwrap();
        assert!((length(&circle) - std::f32::consts::PI * 100.0).abs() < 1.0);
        assert!(trim_dash(&line, 0.0, 1.0, 0.0).is_none());
        assert!(matches!(trim_dash(&line, 0.5, 0.5, 0.0), Some(None)));

        // Draw the first half of a horizontal line: only the left half has ink.
        let mut p = Pixmap::new(120, 20).unwrap();
        let dash = trim_dash(&line, 0.0, 0.5, 0.0).unwrap().unwrap();
        let stroke = tiny_skia::Stroke { width: 6.0, dash: Some(dash), ..Default::default() };
        p.stroke_path(&line, &FillSpec::Solid(Color::WHITE).paint(1.0), &stroke, Transform::from_translate(10.0, 10.0), None);
        assert!(p.pixel(30, 10).unwrap().alpha() > 200);
        assert_eq!(p.pixel(90, 10).unwrap().alpha(), 0);
    }

    #[test]
    fn blurs_spread_ink() {
        for radius in [4.0, 40.0] {
            let mut p = Pixmap::new(200, 200).unwrap();
            p.fill_rect(Rect::from_xywh(90.0, 90.0, 20.0, 20.0).unwrap(), &FillSpec::Solid(Color::WHITE).paint(1.0), Transform::identity(), None);
            blur(&mut p, radius);
            assert!(p.pixel(100, 100).unwrap().alpha() > 0);
            assert!(p.pixel(100, 100 - radius as u32 / 2 - 10).unwrap().alpha() > 0, "spreads at {radius}");
            assert_eq!(p.pixel(2, 2).unwrap().alpha(), 0);
        }
    }

    #[test]
    fn shapes_have_outlines() {
        assert!(polygon(6.0, 50.0, 0.3).is_some());
        assert!(star(5.0, 50.0, 20.0).is_some());
        assert!(rect(100.0, 50.0, 10.0).is_some());
        assert!(svg_path("", &[[0.0, 0.0], [10.0, 10.0], [20.0, 0.0]], true).is_some());
        assert!(svg_path("M0 0 Q", &[], false).is_none());
    }
}
