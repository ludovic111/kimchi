//! Transitions: the outgoing layer `a` and the incoming layer `b` (both canvas-sized,
//! transparent where their clip isn't) put together at progress `p` (0 = all `a`, 1 = all `b`).

use kimchi_core::TransitionKind as K;
use rayon::prelude::*;
use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};

/// Draws the transition between `a` and `b` onto `canvas`. `scale` is output pixels per project
/// pixel (blur radius).
pub(crate) fn draw(canvas: &mut Pixmap, a: &Pixmap, b: &Pixmap, kind: K, p: f32, scale: f32) {
    let p = p.clamp(0.0, 1.0);
    let (w, h) = (canvas.width() as f32, canvas.height() as f32);
    let over = |canvas: &mut Pixmap, layer: &Pixmap, ts: Transform, opacity: f32| {
        let quality = if ts.is_identity() { FilterQuality::Nearest } else { FilterQuality::Bilinear };
        canvas.draw_pixmap(0, 0, layer.as_ref(), &PixmapPaint { opacity, quality, ..PixmapPaint::default() }, ts, None);
    };
    match kind {
        K::Dissolve => over(canvas, &lerp(a, b, p), Transform::identity(), 1.0),
        K::DipToBlack | K::DipToWhite => {
            let mut card = Pixmap::new(a.width(), a.height()).expect("non-empty");
            card.fill(if kind == K::DipToBlack { Color::BLACK } else { Color::WHITE });
            let mixed = if p < 0.5 { lerp(a, &card, p * 2.0) } else { lerp(&card, b, p * 2.0 - 1.0) };
            over(canvas, &mixed, Transform::identity(), 1.0);
        }
        K::WipeLeft | K::WipeRight | K::WipeUp | K::WipeDown => {
            // A soft edge 4 % of the frame wide travels across, uncovering `b` behind it.
            let soft = 0.04;
            let edge = p * (1.0 + 2.0 * soft) - soft;
            let k = move |x: f32, y: f32| {
                let along = match kind {
                    K::WipeLeft => 1.0 - x,
                    K::WipeRight => x,
                    K::WipeUp => 1.0 - y,
                    _ => y,
                };
                1.0 - smoothstep(edge - soft, edge + soft, along)
            };
            over(canvas, &masked(a, b, k), Transform::identity(), 1.0);
        }
        K::Iris => {
            // Circle in frame-height units around the centre; it opens until the corners are in.
            let aspect = w / h.max(1.0);
            let soft = 0.02;
            let corner = 0.5 * (aspect * aspect + 1.0).sqrt();
            let r = p * (corner + 2.0 * soft) - soft;
            let k = move |x: f32, y: f32| {
                let (dx, dy) = ((x - 0.5) * aspect, y - 0.5);
                1.0 - smoothstep(r - soft, r + soft, (dx * dx + dy * dy).sqrt())
            };
            over(canvas, &masked(a, b, k), Transform::identity(), 1.0);
        }
        K::SlideLeft | K::SlideRight | K::SlideUp | K::SlideDown | K::PushLeft | K::PushRight | K::PushUp | K::PushDown => {
            // Where `b` comes from (a unit direction), and whether it pushes `a` out.
            let (dx, dy) = match kind {
                K::SlideLeft | K::PushLeft => (1.0, 0.0),
                K::SlideRight | K::PushRight => (-1.0, 0.0),
                K::SlideUp | K::PushUp => (0.0, 1.0),
                _ => (0.0, -1.0),
            };
            let push = matches!(kind, K::PushLeft | K::PushRight | K::PushUp | K::PushDown);
            let q = 1.0 - p;
            let a_ts = if push { Transform::from_translate(-dx * w * p, -dy * h * p) } else { Transform::identity() };
            over(canvas, a, snap(a_ts), 1.0);
            over(canvas, b, snap(Transform::from_translate(dx * w * q, dy * h * q)), 1.0);
        }
        K::Zoom => {
            over(canvas, a, Transform::identity(), 1.0);
            let s = 0.6 + 0.4 * p;
            let ts = Transform::from_translate(w / 2.0, h / 2.0).pre_scale(s, s).pre_translate(-w / 2.0, -h / 2.0);
            over(canvas, b, ts, p);
        }
        K::Blur => {
            let mut mixed = lerp(a, b, p);
            super::paint::blur(&mut mixed, (std::f32::consts::PI * p).sin() * 24.0 * scale);
            over(canvas, &mixed, Transform::identity(), 1.0);
        }
    }
}

/// Whole-pixel translations stay sharp.
fn snap(ts: Transform) -> Transform {
    Transform::from_translate(ts.tx.round(), ts.ty.round())
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `a × (1 − k) + b × k`, per premultiplied channel.
fn lerp(a: &Pixmap, b: &Pixmap, k: f32) -> Pixmap {
    masked(a, b, move |_, _| k)
}

/// `a` and `b` mixed with a weight per pixel: `k(x, y)` (0…1 across the frame) for `b`.
fn masked(a: &Pixmap, b: &Pixmap, k: impl Fn(f32, f32) -> f32 + Sync) -> Pixmap {
    let mut out = a.clone();
    let (w, h) = (a.width() as usize, a.height() as usize);
    out.data_mut().par_chunks_mut(w * 4).zip(b.data().par_chunks(w * 4)).enumerate().for_each(|(y, (row, brow))| {
        let fy = (y as f32 + 0.5) / h as f32;
        for (x, (o, bp)) in row.chunks_exact_mut(4).zip(brow.chunks_exact(4)).enumerate() {
            let m = k((x as f32 + 0.5) / w as f32, fy).clamp(0.0, 1.0);
            if m <= 0.0 {
                continue;
            }
            for c in 0..4 {
                o[c] = (o[c] as f32 + (bp[c] as f32 - o[c] as f32) * m).round() as u8;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(c: Color) -> Pixmap {
        let mut p = Pixmap::new(40, 20).unwrap();
        p.fill(c);
        p
    }

    fn red_at(p: &Pixmap, x: u32) -> u8 {
        p.pixel(x, 10).unwrap().red()
    }

    #[test]
    fn every_kind_starts_on_a_and_ends_on_b() {
        let (a, b) = (layer(Color::from_rgba8(255, 0, 0, 255)), layer(Color::from_rgba8(0, 0, 255, 255)));
        for info in kimchi_core::transition::KINDS {
            for (p, want) in [(0.0, 255), (1.0, 0)] {
                let mut canvas = layer(Color::BLACK);
                draw(&mut canvas, &a, &b, info.kind, p, 1.0);
                for x in [0, 20, 39] {
                    assert_eq!(red_at(&canvas, x), want, "{} at {p}, x {x}", info.id);
                }
            }
        }
    }

    #[test]
    fn wipes_move_across_and_dips_go_through_the_colour() {
        let (a, b) = (layer(Color::from_rgba8(255, 0, 0, 255)), layer(Color::from_rgba8(0, 0, 255, 255)));
        let mut canvas = layer(Color::BLACK);
        draw(&mut canvas, &a, &b, K::WipeRight, 0.5, 1.0);
        assert_eq!((red_at(&canvas, 2), red_at(&canvas, 37)), (0, 255), "b is uncovered from the left");
        let mut canvas = layer(Color::BLACK);
        draw(&mut canvas, &a, &b, K::DipToWhite, 0.5, 1.0);
        assert_eq!(canvas.pixel(20, 10).unwrap().green(), 255);
    }
}
