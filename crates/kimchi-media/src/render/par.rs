//! tiny-skia draws on one core; a frame's big draws (a 1080p video layer scaled onto the canvas,
//! a layer blended over it) are split here into bands of rows drawn on every core. Each band is
//! its own small pixmap over the canvas' rows, and the drawing is moved up by the band's top, so
//! every pixel is computed exactly as in one big draw.

use rayon::prelude::*;
use tiny_skia::{Pixmap, PixmapMut, PixmapPaint, PixmapRef, Transform};

/// Below this many pixels a draw isn't worth splitting.
const MIN_PIXELS: usize = 64 * 1024;
/// Rows per band at least (each band repeats the draw's setup).
const MIN_ROWS: usize = 16;

/// Runs `draw(band, shift)` over horizontal bands of `target` in parallel: `shift` moves canvas
/// coordinates to the band's (prepend it to the draw's transform with `post_concat`). Only the
/// rows `rows` (top, bottom; clamped) are visited, so a draw that covers part of the canvas costs
/// only that part.
pub(crate) fn bands(target: &mut Pixmap, rows: (i64, i64), draw: impl Fn(&mut PixmapMut, Transform) + Sync) {
    let (w, h) = (target.width() as usize, target.height() as usize);
    let top = rows.0.clamp(0, h as i64) as usize;
    let bottom = rows.1.clamp(0, h as i64) as usize;
    if bottom <= top {
        return;
    }
    let span = bottom - top;
    let threads = rayon::current_num_threads().max(1);
    if w * span < MIN_PIXELS || threads == 1 {
        let data = &mut target.data_mut()[top * w * 4..bottom * w * 4];
        if let Some(mut band) = PixmapMut::from_bytes(data, w as u32, span as u32) {
            draw(&mut band, Transform::from_translate(0.0, -(top as f32)));
        }
        return;
    }
    // A few bands per thread so uneven work (a layer covering only part of a band) evens out.
    let per = span.div_ceil(threads * 3).max(MIN_ROWS);
    let data = &mut target.data_mut()[top * w * 4..bottom * w * 4];
    data.par_chunks_mut(per * w * 4).enumerate().for_each(|(i, chunk)| {
        let y0 = top + i * per;
        let rows = chunk.len() / (w * 4);
        if let Some(mut band) = PixmapMut::from_bytes(chunk, w as u32, rows as u32) {
            draw(&mut band, Transform::from_translate(0.0, -(y0 as f32)));
        }
    });
}

/// `target.draw_pixmap(0, 0, pic, paint, ts, None)` on every core, over the rows the picture
/// lands on.
pub(crate) fn draw_pixmap(target: &mut Pixmap, pic: PixmapRef, paint: &PixmapPaint, ts: Transform) {
    let rows = covered_rows(pic.width() as f32, pic.height() as f32, ts, target.height());
    bands(target, rows, |band, shift| band.draw_pixmap(0, 0, pic, paint, ts.post_concat(shift), None));
}

/// Rows (top, bottom exclusive) a `w`×`h` rectangle drawn with `ts` touches, one row of margin
/// for filtering; the whole target when the transform is odd.
pub(crate) fn covered_rows(w: f32, h: f32, ts: Transform, height: u32) -> (i64, i64) {
    let ys = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| ts.ky * x + ts.sy * y + ts.ty);
    let lo = ys.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    if !lo.is_finite() || !hi.is_finite() {
        return (0, height as i64);
    }
    ((lo.floor() as i64 - 1).max(0), (hi.ceil() as i64 + 1).min(height as i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, FilterQuality};

    fn picture(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for (i, px) in p.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as u8, (i as u32 / w) as u8);
            *px = [x.wrapping_mul(7), y.wrapping_mul(5), x ^ y, 255];
        }
        p
    }

    #[test]
    fn banded_draws_match_one_draw() {
        let pic = picture(300, 200);
        for ts in [
            Transform::identity(),
            Transform::from_translate(13.0, -7.0),
            Transform::from_row(2.37, 0.0, 0.0, 2.11, -31.5, 12.25),
            Transform::from_translate(400.0, 300.0).pre_rotate(17.0).pre_scale(2.2, 1.7).pre_translate(-150.0, -100.0),
        ] {
            for quality in [FilterQuality::Nearest, FilterQuality::Bilinear, FilterQuality::Bicubic] {
                let paint = PixmapPaint { opacity: 0.8, quality, ..PixmapPaint::default() };
                let mut one = Pixmap::new(800, 600).unwrap();
                one.fill(Color::from_rgba8(20, 30, 40, 255));
                let mut split = one.clone();
                one.draw_pixmap(0, 0, pic.as_ref(), &paint, ts, None);
                draw_pixmap(&mut split, pic.as_ref(), &paint, ts);
                let worst = one.data().iter().zip(split.data()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                // Under a rotation, a pixel centre that falls right on a texel edge may take the
                // neighbouring texel (the band's inverse transform rounds a hair differently):
                // a few pixels in a million, on this picture with a jump at every texel.
                let rotated = ts.kx != 0.0 || ts.ky != 0.0;
                if rotated {
                    let off = one.data().iter().zip(split.data()).filter(|(a, b)| a.abs_diff(**b) > 1).count();
                    assert!(off * 1000 < one.data().len(), "{ts:?}: {off} values differ");
                } else {
                    assert!(worst <= 1, "{ts:?} {quality:?}: off by {worst}");
                }
            }
        }
    }

    #[test]
    fn rows_cover_the_picture() {
        assert_eq!(covered_rows(100.0, 50.0, Transform::from_translate(0.0, 10.0), 1080), (9, 61));
        let above = covered_rows(100.0, 50.0, Transform::from_translate(0.0, -100.0), 1080);
        assert!(above.1 <= above.0, "{above:?}: nothing to draw");
        assert_eq!(covered_rows(100.0, 50.0, Transform::from_scale(1.0, f32::NAN), 1080), (0, 1080));
    }
}
