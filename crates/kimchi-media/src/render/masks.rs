//! Layer masks (`stack::MASKS`): paths, rectangles and ellipses in the layer's own pixels,
//! combined in order like After Effects' masks. The first mask that adds starts from nothing,
//! one that subtracts or intersects starts from the whole layer; each is grown or shrunk
//! (expansion), softened (feather), faded (opacity) and may be inverted before it combines.

use kimchi_core::motion::Mask;
use tiny_skia::{BlendMode, Color, FillRule, LineJoin, Paint, Path, Pixmap, Stroke, Transform};

use super::effects2d::blur_plane;
use super::paint;

/// The masks' shape in a layer's own pixels.
pub(crate) fn outline(m: &Mask) -> Option<Path> {
    let v2 = |name: &str| m.v2(name);
    match m.kind.as_str() {
        "rect" => {
            let (c, s) = (v2("center"), v2("size"));
            let (w, h) = (s[0].abs() as f32, s[1].abs() as f32);
            crate::text::round_rect(c[0] as f32 - w / 2.0, c[1] as f32 - h / 2.0, w, h, m.n("radius") as f32)
        }
        "ellipse" => {
            let (c, s) = (v2("center"), v2("size"));
            let (w, h) = (s[0].abs() as f32, s[1].abs() as f32);
            tiny_skia::PathBuilder::from_oval(tiny_skia::Rect::from_xywh(c[0] as f32 - w / 2.0, c[1] as f32 - h / 2.0, w, h)?)
        }
        _ => paint::svg_path(&m.s("d"), &[], true),
    }
}

/// How much of each pixel of a `w`×`h` picture the masks let through (0–1, row by row), or
/// `None` when they don't limit anything (no masks, or all of mode `none`). `ts` maps the
/// layer's pixels to the picture.
pub(crate) fn coverage(masks: &[Mask], ts: Transform, w: u32, h: u32) -> Option<Vec<f32>> {
    let active: Vec<&Mask> = masks.iter().filter(|m| m.enabled && m.s("mode") != "none").collect();
    let first = active.first()?;
    let start = if matches!(first.s("mode").as_str(), "subtract" | "intersect") { 1.0 } else { 0.0 };
    let mut acc = vec![start; w as usize * h as usize];
    let scale = (ts.sx * ts.sy - ts.kx * ts.ky).abs().sqrt();
    for m in active {
        let mut cov = shape(m, ts, scale, w, h);
        let opacity = m.n("opacity") as f32;
        let inverted = m.b("inverted");
        for v in cov.iter_mut() {
            if inverted {
                *v = 1.0 - *v;
            }
            *v *= opacity;
        }
        let mode = m.s("mode");
        for (a, c) in acc.iter_mut().zip(&cov) {
            *a = match mode.as_str() {
                "subtract" => *a * (1.0 - c),
                "intersect" => *a * c,
                "difference" => *a + c - 2.0 * *a * c,
                _ => *a + c - *a * c,
            };
        }
    }
    Some(acc)
}

/// Applies the masks to a layer's picture.
pub(crate) fn apply(p: &mut Pixmap, masks: &[Mask], ts: Transform) {
    let Some(cov) = coverage(masks, ts, p.width(), p.height()) else { return };
    multiply(p, &cov);
}

/// Scales every pixel of `p` by `cov` (0–1 per pixel).
pub(crate) fn multiply(p: &mut Pixmap, cov: &[f32]) {
    for (px, c) in p.data_mut().chunks_exact_mut(4).zip(cov) {
        let c = c.clamp(0.0, 1.0);
        if c >= 1.0 {
            continue;
        }
        for v in px.iter_mut() {
            *v = (*v as f32 * c).round() as u8;
        }
    }
}

/// One mask's coverage: its shape, grown or shrunk, feathered.
fn shape(m: &Mask, ts: Transform, scale: f32, w: u32, h: u32) -> Vec<f32> {
    let n = w as usize * h as usize;
    let Some(path) = outline(m) else { return vec![0.0; n] };
    let Some(mut pic) = Pixmap::new(w, h) else { return vec![0.0; n] };
    let white = Paint { shader: tiny_skia::Shader::SolidColor(Color::WHITE), anti_alias: true, ..Paint::default() };
    pic.fill_path(&path, &white, FillRule::Winding, ts, None);
    let grow = m.n("expansion") as f32;
    if grow.abs() > 0.01 {
        let stroke = Stroke { width: grow.abs() * 2.0, line_join: LineJoin::Round, ..Stroke::default() };
        let paint = if grow > 0.0 { white } else { Paint { blend_mode: BlendMode::Clear, ..white } };
        pic.stroke_path(&path, &paint, &stroke, ts, None);
    }
    let mut cov: Vec<f32> = pic.data().chunks_exact(4).map(|px| px[3] as f32 / 255.0).collect();
    let feather = m.n("feather") as f32 * scale;
    if feather > 0.3 {
        // The soft edge spans about the feather width (±2σ), centred on the outline.
        blur_plane(&mut cov, w as usize, h as usize, feather / 4.0);
    }
    cov
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn masks(v: serde_json::Value) -> Vec<Mask> {
        serde_json::from_value(v).unwrap()
    }

    /// Coverage of a 100×100 picture whose layer centre is the middle.
    fn cov(v: serde_json::Value) -> Option<Vec<f32>> {
        coverage(&masks(v), Transform::from_translate(50.0, 50.0), 100, 100)
    }

    fn at(c: &[f32], x: usize, y: usize) -> f32 {
        c[y * 100 + x]
    }

    #[test]
    fn modes_combine_in_order() {
        assert!(cov(json!([])).is_none());
        assert!(cov(json!([{"type": "rect", "mode": "none"}])).is_none());
        // Add: only inside.
        let c = cov(json!([{"type": "rect", "size": [40, 40]}])).unwrap();
        assert_eq!(at(&c, 50, 50), 1.0);
        assert_eq!(at(&c, 5, 5), 0.0);
        // Subtract first: everything but the hole.
        let c = cov(json!([{"type": "ellipse", "size": [40, 40], "mode": "subtract"}])).unwrap();
        assert_eq!(at(&c, 50, 50), 0.0);
        assert_eq!(at(&c, 5, 5), 1.0);
        // Add a big rect, subtract a small one: a frame.
        let c = cov(json!([{"type": "rect", "size": [80, 80]}, {"type": "rect", "size": [20, 20], "mode": "subtract"}])).unwrap();
        assert_eq!(at(&c, 50, 50), 0.0);
        assert_eq!(at(&c, 20, 50), 1.0);
        assert_eq!(at(&c, 2, 50), 0.0);
        // Intersect two overlapping rects: only the overlap.
        let c = cov(json!([{"type": "rect", "center": [-10, 0], "size": [40, 40]}, {"type": "rect", "center": [10, 0], "size": [40, 40], "mode": "intersect"}])).unwrap();
        assert_eq!(at(&c, 50, 50), 1.0);
        assert_eq!(at(&c, 30, 50), 0.0);
        assert_eq!(at(&c, 70, 50), 0.0);
        // Difference: both but not the overlap.
        let c = cov(json!([{"type": "rect", "center": [-10, 0], "size": [40, 40]}, {"type": "rect", "center": [10, 0], "size": [40, 40], "mode": "difference"}])).unwrap();
        assert_eq!(at(&c, 50, 50), 0.0);
        assert_eq!(at(&c, 33, 50), 1.0);
        assert_eq!(at(&c, 67, 50), 1.0);
        // Inverted and half opacity.
        let c = cov(json!([{"type": "rect", "size": [40, 40], "inverted": true, "opacity": 0.5}])).unwrap();
        assert_eq!(at(&c, 50, 50), 0.0);
        assert!((at(&c, 5, 5) - 0.5).abs() < 1e-6);
        // A path mask.
        let c = cov(json!([{"d": "M-20 -20 L20 -20 L20 20 L-20 20 Z"}])).unwrap();
        assert_eq!(at(&c, 50, 50), 1.0);
        assert_eq!(at(&c, 80, 50), 0.0);
    }

    #[test]
    fn feather_softens_and_expansion_grows() {
        let hard = cov(json!([{"type": "rect", "size": [40, 40]}])).unwrap();
        let soft = cov(json!([{"type": "rect", "size": [40, 40], "feather": 16}])).unwrap();
        // At the edge, half; inside it fades in, outside it fades out.
        assert!((at(&soft, 70, 50) - 0.5).abs() < 0.15, "{}", at(&soft, 70, 50));
        assert!(at(&soft, 66, 50) < 1.0 && at(&soft, 66, 50) > 0.5);
        assert!(at(&soft, 74, 50) > 0.0 && at(&hard, 74, 50) == 0.0);
        assert!(at(&soft, 50, 50) > 0.99);
        let grown = cov(json!([{"type": "rect", "size": [40, 40], "expansion": 10}])).unwrap();
        assert_eq!(at(&grown, 77, 50), 1.0);
        let shrunk = cov(json!([{"type": "rect", "size": [40, 40], "expansion": -10}])).unwrap();
        assert_eq!(at(&shrunk, 65, 50), 0.0);
        assert_eq!(at(&shrunk, 55, 50), 1.0);
    }
}
