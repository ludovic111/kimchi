//! Grain and dither: the page behind the chrome, and the texture on things that want one.
//!
//! Everything here is a small picture made once, at the screen's own pixels (a texel is one
//! device pixel, so GPUI's linear sampling can't soften it), in the ink of the theme (white on
//! the dark page, black on the light one), and laid out at its device size divided by the scale.
//!
//! - [`backdrop`]: film grain over the whole page, and two corners of ordered-dither light where
//!   the old glows were.
//! - [`dither`]: a dithered fade in a box (empty states, projects without a picture).
//!
//! Each size is drawn once and kept, so callers use a few fixed sizes and clip them to their box.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, App, Div, Hsla, ObjectFit, ParentElement, RenderImage, Rgba, Styled, Window, div, img, prelude::*, px};

use crate::theme::ActiveTheme;

/// Logical size of one grain tile (repeated over the page).
const TILE: f32 = 192.;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Grain,
    /// A corner of dithered light: `(width, height)` logical, the light at `corner` (0 top-left,
    /// 1 bottom-right), `cell` logical pixels per dot.
    Corner { w: u16, h: u16, corner: u8, cell: u8 },
    /// A dithered fade from the top (`w`×`h`, dots of `cell`).
    Fade { w: u16, h: u16, cell: u8 },
}

thread_local! {
    static CACHE: RefCell<HashMap<(Kind, u32, bool), Arc<RenderImage>>> = RefCell::new(HashMap::new());
}

/// A deterministic hash of a pixel (the grain must not crawl between frames).
fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^ (h >> 15)
}

/// The 8×8 Bayer threshold at a cell, in 0..1.
fn bayer(x: u32, y: u32) -> f32 {
    const M: [[u8; 8]; 8] = [
        [0, 32, 8, 40, 2, 34, 10, 42],
        [48, 16, 56, 24, 50, 18, 58, 26],
        [12, 44, 4, 36, 14, 46, 6, 38],
        [60, 28, 52, 20, 62, 30, 54, 22],
        [3, 35, 11, 43, 1, 33, 9, 41],
        [51, 19, 59, 27, 49, 17, 57, 25],
        [15, 47, 7, 39, 13, 45, 5, 37],
        [63, 31, 55, 23, 61, 29, 53, 21],
    ];
    (M[(y % 8) as usize][(x % 8) as usize] as f32 + 0.5) / 64.
}

/// Draws `kind` at `scale` device pixels per logical pixel, in white (`ink`) or black.
fn make(kind: Kind, scale: f32, ink: bool) -> Arc<RenderImage> {
    let s = scale.max(1.);
    let (lw, lh) = match kind {
        Kind::Grain => (TILE, TILE),
        Kind::Corner { w, h, .. } | Kind::Fade { w, h, .. } => (w as f32, h as f32),
    };
    let (w, h) = ((lw * s).round() as u32, (lh * s).round() as u32);
    // One "pixel" of the grain is one logical pixel (two device pixels on a Retina screen).
    let unit = s.round().max(1.) as u32;
    let v = if ink { 255 } else { 0 };
    let mut buf = image::RgbaImage::new(w.max(1), h.max(1));
    for (x, y, p) in buf.enumerate_pixels_mut() {
        let a: f32 = match kind {
            Kind::Grain => {
                let n = hash(x / unit, y / unit, 7) as f32 / u32::MAX as f32;
                // Sparse specks of varied strength, and a faint even tooth under them.
                if n > 0.86 { 0.35 + (n - 0.86) / 0.14 * 0.65 } else if n > 0.55 { 0.12 } else { 0. }
            }
            Kind::Corner { corner, cell, .. } => {
                let c = (cell as f32 * s).round().max(1.) as u32;
                let (cx, cy) = (x / c, y / c);
                // Distance from the lit corner, 0..1 across the box, and a little noise so the
                // edge of the light breaks up like a print.
                let (fx, fy) = ((cx * c) as f32 / w as f32, (cy * c) as f32 / h as f32);
                let (dx, dy) = if corner == 0 { (fx, fy) } else { (1. - fx, 1. - fy) };
                let d = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
                let jitter = (hash(cx, cy, 3) as f32 / u32::MAX as f32 - 0.5) * 0.18;
                let level = (1. - d * 1.35 + jitter).clamp(0., 1.).powf(1.6);
                // A square dot in each lit cell, one device pixel short of the next (a grid you
                // feel more than see).
                let inside = x % c < c.saturating_sub(unit).max(1) && y % c < c.saturating_sub(unit).max(1);
                if inside && level > bayer(cx, cy) { 1. } else { 0. }
            }
            Kind::Fade { cell, .. } => {
                let c = (cell as f32 * s).round().max(1.) as u32;
                let (cx, cy) = (x / c, y / c);
                let level = (1. - (cy * c) as f32 / h as f32).clamp(0., 1.).powf(1.8);
                let inside = x % c < c.saturating_sub(unit).max(1) && y % c < c.saturating_sub(unit).max(1);
                if inside && level > bayer(cx, cy) { 1. } else { 0. }
            }
        };
        *p = image::Rgba([v, v, v, (a * 255.).round() as u8]);
    }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)]))
}

fn get(kind: Kind, scale: f32, ink: bool) -> Arc<RenderImage> {
    let key = (kind, scale.to_bits(), ink);
    CACHE.with(|c| c.borrow_mut().entry(key).or_insert_with(|| make(kind, scale, ink)).clone())
}

/// A picture laid out at its device size over the scale (one texel, one device pixel).
fn picture(image: Arc<RenderImage>, scale: f32, opacity: f32) -> gpui::Img {
    let size = image.size(0);
    let (w, h) = (u32::from(size.width) as f32 / scale.max(1.), u32::from(size.height) as f32 / scale.max(1.));
    img(image).flex_none().w(px(w)).h(px(h)).object_fit(ObjectFit::Fill).opacity(opacity)
}

fn ink_is_white(ink: Hsla) -> bool {
    let c: Rgba = ink.into();
    c.r > 0.5
}

/// The page: its colour, two corners of dithered light and grain over all of it. Fills its parent.
pub fn backdrop(bg: Hsla, window: &Window, cx: &App) -> AnyElement {
    let t = cx.theme();
    let scale = window.scale_factor();
    let ink = ink_is_white(t.ink);
    let view = window.viewport_size();
    let (vw, vh) = (f32::from(view.width), f32::from(view.height));
    let (cols, rows) = ((vw / TILE).ceil() as usize, (vh / TILE).ceil() as usize);
    let grain = get(Kind::Grain, scale, ink);
    let tl = get(Kind::Corner { w: 760, h: 520, corner: 0, cell: 4 }, scale, ink);
    let br = get(Kind::Corner { w: 900, h: 560, corner: 1, cell: 4 }, scale, ink);
    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .bg(bg)
        .child(div().absolute().top_0().left_0().child(picture(tl, scale, t.dither)))
        .child(div().absolute().bottom_0().right_0().child(picture(br, scale, t.dither * 0.8)))
        .child(
            div().absolute().top_0().left_0().flex().flex_col().children((0..rows).map(|_| {
                div().flex().children((0..cols).map(|_| picture(grain.clone(), scale, t.grain)))
            })),
        )
        .into_any_element()
}

/// A dithered fade in the ink, `w`×`h` logical pixels, densest at the top, at `opacity` (a fixed
/// size: each one is drawn once and kept).
pub fn dither(w: f32, h: f32, opacity: f32, window: &Window, cx: &App) -> gpui::Img {
    let t = cx.theme();
    let scale = window.scale_factor();
    picture(get(Kind::Fade { w: w as u16, h: h as u16, cell: 3 }, scale, ink_is_white(t.ink)), scale, opacity)
}

/// Four corner brackets over the box of a positioned parent (the marks a viewfinder draws).
pub fn brackets(size: f32, inset: f32, color: Hsla) -> Div {
    let arm = |d: Div| d.absolute().size(px(size)).border_color(color);
    div()
        .absolute()
        .inset(px(inset))
        .child(arm(div()).top_0().left_0().border_t_1().border_l_1())
        .child(arm(div()).top_0().right_0().border_t_1().border_r_1())
        .child(arm(div()).bottom_0().left_0().border_b_1().border_l_1())
        .child(arm(div()).bottom_0().right_0().border_b_1().border_r_1())
}
