//! The 2D layers' effects (`stack::EFFECTS`), applied in order to a layer's picture like After
//! Effects' effect stack: blurs and light (blur, directional and radial blur, glow, drop shadow,
//! outline, echo), colour (correction, levels, tint, tritone, fill, gradient ramp, invert,
//! threshold, posterize, vignette), generators (grain, fractal noise, halftone, scanlines) and
//! distortions (turbulent displace, wave warp, ripple, twirl, bulge, mosaic, chromatic
//! aberration, glitch, mirror, kaleidoscope, motion tile, corner pin, sharpen).
//!
//! Pictures are premultiplied RGBA like tiny-skia's. Each effect works only over the part of the
//! picture with ink (grown by how far it spreads it), rows in parallel; blurs are three box
//! blurs (the cost doesn't grow with the radius) on a smaller copy when the radius is big.
//! Lengths in parameters are project pixels (× `FxCx::k` on the picture); points "from the
//! canvas centre" go through `FxCx::base`.

use kimchi_core::motion::Effect;
use kimchi_core::motion::stack;
use rayon::prelude::*;
use tiny_skia::{BlendMode, Pixmap, PixmapPaint, Point, Transform};

use super::noise;

/// What effects need to know about where the picture sits.
pub(crate) struct FxCx<'a> {
    /// Picture pixels per project pixel.
    pub k: f32,
    /// Project pixels from the canvas centre → picture pixels.
    pub base: Transform,
    /// The canvas (scene or composition), project pixels.
    pub canvas: (f64, f64),
    /// The layer list's time (scene or composition seconds).
    pub t: f64,
    /// Seconds per frame (animated grain).
    pub frame: f64,
    /// The layer's corners on the picture (top left, top right, bottom right, bottom left),
    /// when it has a shape: corner pin moves them, motion tile centres on them.
    pub corners: Option<[[f32; 2]; 4]>,
    /// Draws the layer at another moment, through the effects before this one (echo).
    pub echo: Option<&'a mut dyn FnMut(f64) -> Option<Pixmap>>,
}

/// How far (picture pixels) an effect can spread ink beyond where the layer has it; `None` when
/// it can put ink anywhere (mirrors, tiles, corner pins…). For the margin a layer is drawn with.
pub(crate) fn reach(e: &Effect, k: f32) -> Option<f32> {
    if !e.enabled {
        return Some(0.0);
    }
    let n = |name: &str| e.n(name) as f32 * k;
    Some(match e.kind.as_str() {
        "blur" | "glow" => n("radius") * 1.5,
        "directionalBlur" => n("length") / 2.0,
        "dropShadow" => n("distance") + n("blur") * 1.5,
        "stroke" => n("width") + 2.0,
        "turbulentDisplace" => n("amount"),
        "waveWarp" => n("height"),
        "ripple" => n("amplitude"),
        "chromaticAberration" => n("amount"),
        "mosaic" => n("size"),
        "sharpen" => 2.0 * k,
        "radialBlur" | "echo" | "glitch" | "mirror" | "kaleidoscope" | "motionTile" | "cornerPin" | "twirl" | "bulge" => return None,
        _ => 0.0,
    })
}

/// Applies one effect to `p`.
pub(crate) fn apply(p: &mut Pixmap, e: &Effect, cx: &mut FxCx) {
    if !e.enabled {
        return;
    }
    let k = cx.k.max(1e-4);
    let n = |name: &str| e.n(name) as f32;
    let col = |name: &str| rgba(&e.s(name));
    let base = cx.base;
    let at = |name: &str| {
        let v = e.v2(name);
        at_point(base, v[0] as f32, v[1] as f32)
    };
    let inv = cx.base.invert().unwrap_or_default();
    let project = move |x: f32, y: f32| {
        let mut pt = Point::from_xy(x, y);
        inv.map_point(&mut pt);
        (pt.x, pt.y)
    };
    let t = cx.t as f32;
    match e.kind.as_str() {
        "blur" => {
            let s = n("radius") * k / 2.0;
            match e.s("dimensions").as_str() {
                "horizontal" => gaussian(p, s, 0.0),
                "vertical" => gaussian(p, 0.0, s),
                _ => gaussian(p, s, s),
            }
        }
        "directionalBlur" => directional(p, n("length") * k, n("angle")),
        "radialBlur" => radial(p, n("amount"), e.s("kind") == "spin", at("center")),
        "glow" => glow(p, n("radius") * k, n("intensity"), n("threshold"), col("color")),
        "dropShadow" => {
            let (a, d) = (n("angle").to_radians(), n("distance") * k);
            drop_shadow(p, col("color"), n("blur") * k, -a.cos() * d, a.sin() * d);
        }
        "stroke" => outline(p, col("color"), n("width") * k, &e.s("position")),
        "echo" => {
            let (count, delay, decay) = (e.n("count").round().max(0.0) as usize, e.n("delay"), n("decay"));
            if let Some(redraw) = cx.echo.as_mut() {
                let mut opacity = 1.0;
                for i in 1..=count {
                    opacity *= decay;
                    if opacity <= 0.002 {
                        break;
                    }
                    if let Some(pic) = redraw(cx.t - i as f64 * delay) {
                        let paint = PixmapPaint { opacity, blend_mode: BlendMode::DestinationOver, ..PixmapPaint::default() };
                        p.draw_pixmap(0, 0, pic.as_ref(), &paint, Transform::identity(), None);
                    }
                }
            }
        }
        // Colour
        "colorCorrect" => {
            let (b, c, s, exp, g) = (n("brightness"), n("contrast"), n("saturation"), n("exposure"), n("gamma"));
            let hue = hue_matrix(n("hue"));
            let gain = 2f32.powf(exp);
            map_rgb(p, move |mut v, _, _| {
                for c in &mut v {
                    *c *= gain;
                }
                for x in &mut v {
                    *x = (*x + b - 0.5) * (1.0 + c) + 0.5;
                }
                let l = luma(v);
                for x in &mut v {
                    *x = l + (*x - l) * (1.0 + s);
                }
                let v = mul3(&hue, v);
                v.map(|x| x.max(0.0).powf(1.0 / g.max(0.01)))
            });
        }
        "levels" => {
            let (ib, iw, g, ob, ow) = (n("inBlack"), n("inWhite"), n("gamma"), n("outBlack"), n("outWhite"));
            let span = if (iw - ib).abs() < 1e-4 { 1e-4 } else { iw - ib };
            map_rgb(p, move |v, _, _| v.map(|x| ob + ((x - ib) / span).clamp(0.0, 1.0).powf(1.0 / g.max(0.01)) * (ow - ob)));
        }
        "tint" => {
            let (lo, hi, amount) = (col("black"), col("white"), n("amount"));
            map_rgb(p, move |v, _, _| {
                let l = luma(v).clamp(0.0, 1.0);
                let m: [f32; 3] = std::array::from_fn(|i| lo[i] + (hi[i] - lo[i]) * l);
                mix3(v, m, amount)
            });
        }
        "tritone" => {
            let (s, m, h, amount) = (col("shadows"), col("midtones"), col("highlights"), n("amount"));
            map_rgb(p, move |v, _, _| {
                let l = luma(v).clamp(0.0, 1.0);
                let out: [f32; 3] =
                    if l < 0.5 { std::array::from_fn(|i| s[i] + (m[i] - s[i]) * l * 2.0) } else { std::array::from_fn(|i| m[i] + (h[i] - m[i]) * (l - 0.5) * 2.0) };
                mix3(v, out, amount)
            });
        }
        "fill" => {
            let (c, amount) = (col("color"), n("amount"));
            map_rgba(p, None, move |v, _, _| {
                let rgb = mix3([v[0], v[1], v[2]], [c[0], c[1], c[2]], amount);
                [rgb[0], rgb[1], rgb[2], v[3] * (1.0 - amount + amount * c[3])]
            });
        }
        "gradientRamp" => {
            let (from, to) = (at("from"), at("to"));
            let (ca, cb, amount, radial) = (col("colorFrom"), col("colorTo"), n("amount"), e.s("kind") == "radial");
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let len2 = (dx * dx + dy * dy).max(1e-6);
            map_rgb(p, move |v, x, y| {
                let u = if radial {
                    (((x - from.0).powi(2) + (y - from.1).powi(2)) / len2).sqrt()
                } else {
                    ((x - from.0) * dx + (y - from.1) * dy) / len2
                }
                .clamp(0.0, 1.0);
                let g: [f32; 3] = std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * u);
                mix3(v, g, amount)
            });
        }
        "invert" => {
            let amount = n("amount");
            map_rgb(p, move |v, _, _| mix3(v, v.map(|x| 1.0 - x), amount));
        }
        "threshold" => {
            let level = n("level");
            map_rgb(p, move |v, _, _| if luma(v) >= level { [1.0; 3] } else { [0.0; 3] });
        }
        "posterize" => {
            let steps = (n("levels").round().max(2.0)) - 1.0;
            map_rgb(p, move |v, _, _| v.map(|x| (x.clamp(0.0, 1.0) * steps).round() / steps));
        }
        "vignette" => {
            let (amount, size, soft) = (n("amount"), n("size"), n("softness").max(1e-3));
            let c = at_point(cx.base, 0.0, 0.0);
            let half = ((cx.canvas.0 * cx.canvas.0 + cx.canvas.1 * cx.canvas.1).sqrt() / 2.0) as f32 * k;
            map_rgb(p, move |v, x, y| {
                let d = ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() / half.max(1e-3);
                let dark = amount * smoothstep(size, size + soft, d);
                v.map(|x| x * (1.0 - dark))
            });
        }
        // Generate
        "noise" => {
            let (amount, colored, seed) = (n("amount"), e.b("color"), e.n("seed") as u32);
            let frame = if e.b("animated") && cx.frame > 0.0 { (cx.t / cx.frame).round() as i64 as i32 } else { 0 };
            map_rgb(p, move |v, x, y| {
                let (xi, yi) = (x as i32, y as i32);
                let g = |c: u32| (noise::unit(xi, yi, frame, seed.wrapping_add(c)) - 0.5) * amount;
                if colored { [v[0] + g(0), v[1] + g(1), v[2] + g(2)] } else { v.map(|c| c + g(0)) }
            });
        }
        "fractalNoise" => {
            let (scale, octaves, evo, contrast) = (n("scale").max(1.0), n("complexity").round() as u32, n("evolution"), n("contrast"));
            let (ca, cb, amount, seed) = (col("colorA"), col("colorB"), n("amount"), e.n("seed") as u32);
            map_rgb(p, move |v, x, y| {
                let (px, py) = project(x, y);
                let f = noise::fbm(px / scale, py / scale, evo, octaves, seed);
                let u = (f * contrast * 0.5 + 0.5).clamp(0.0, 1.0);
                let g: [f32; 3] = std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * u);
                mix3(v, g, amount)
            });
        }
        "halftone" => halftone(p, n("size") * k, n("angle"), e.b("color"), at_point(cx.base, 0.0, 0.0)),
        "scanlines" => {
            let (spacing, amount, speed) = (n("spacing").max(1.0), n("amount"), n("speed"));
            map_rgb(p, move |v, x, y| {
                let (_, py) = project(x, y);
                let phase = (py + speed * t) / spacing;
                let f = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
                v.map(|c| c * (1.0 - amount * f))
            });
        }
        // Distort
        "turbulentDisplace" => {
            let (amount, size, evo, seed) = (n("amount") * k, n("size").max(1.0), n("evolution"), e.n("seed") as u32);
            let roi = ink(p).map(|r| r.grow(amount + 1.0, p));
            warp(p, roi, move |x, y| {
                let (px, py) = project(x, y);
                let (qx, qy) = (px / size, py / size);
                (x + amount * noise::fbm(qx, qy, evo, 3, seed), y + amount * noise::fbm(qx + 19.1, qy + 7.7, evo, 3, seed))
            });
        }
        "waveWarp" => {
            let (height, width, speed, a) = (n("height") * k, n("width").max(1.0), n("speed"), n("angle").to_radians());
            let (u, nrm) = ((a.cos(), a.sin()), (-a.sin(), a.cos()));
            let roi = ink(p).map(|r| r.grow(height + 1.0, p));
            warp(p, roi, move |x, y| {
                let (px, py) = project(x, y);
                let along = px * u.0 + py * u.1;
                let d = height * ((along / width - speed * t) * std::f32::consts::TAU).sin();
                (x + nrm.0 * d, y + nrm.1 * d)
            });
        }
        "ripple" => {
            let (amp, wl, speed) = (n("amplitude") * k, n("wavelength").max(1.0), n("speed"));
            let c = at("center");
            let roi = ink(p).map(|r| r.grow(amp + 1.0, p));
            warp(p, roi, move |x, y| {
                let (qx, qy) = (x - c.0, y - c.1);
                let r = (qx * qx + qy * qy).sqrt();
                if r < 1e-3 {
                    return (x, y);
                }
                let d = amp * ((r / (wl * k) - speed * t) * std::f32::consts::TAU).sin();
                (x + qx / r * d, y + qy / r * d)
            });
        }
        "twirl" => {
            let (angle, radius) = (n("angle").to_radians(), n("radius").max(1.0) * k);
            let c = at("center");
            warp(p, None, move |x, y| {
                let (qx, qy) = (x - c.0, y - c.1);
                let r = (qx * qx + qy * qy).sqrt();
                if r >= radius {
                    return (x, y);
                }
                let th = -angle * (1.0 - r / radius).powi(2);
                let (s, co) = th.sin_cos();
                (c.0 + qx * co - qy * s, c.1 + qx * s + qy * co)
            });
        }
        "bulge" => {
            let (amount, radius) = (n("amount"), n("radius").max(1.0) * k);
            let c = at("center");
            warp(p, None, move |x, y| {
                let (qx, qy) = (x - c.0, y - c.1);
                let r2 = (qx * qx + qy * qy) / (radius * radius);
                if r2 >= 1.0 {
                    return (x, y);
                }
                let f = (1.0 - amount * 0.5 * (1.0 - r2).powi(2)).clamp(0.05, 3.0);
                (c.0 + qx * f, c.1 + qy * f)
            });
        }
        "mosaic" => mosaic(p, n("size").max(1.0) * k, at_point(cx.base, 0.0, 0.0)),
        "chromaticAberration" => {
            let (d, a) = (n("amount") * k, n("angle").to_radians());
            split_channels(p, None, d * a.cos(), d * a.sin());
        }
        "glitch" => glitch(p, n("amount"), n("speed"), e.n("seed") as u32, cx.t, k),
        "mirror" => {
            let c = at("center");
            let a = n("angle").to_radians();
            let nrm = (a.cos(), a.sin());
            warp(p, None, move |x, y| {
                let d = (x - c.0) * nrm.0 + (y - c.1) * nrm.1;
                if d >= 0.0 { (x, y) } else { (x - 2.0 * d * nrm.0, y - 2.0 * d * nrm.1) }
            });
        }
        "kaleidoscope" => {
            let c = at("center");
            let (segs, a) = (n("segments").round().max(2.0), n("angle").to_radians());
            let seg = std::f32::consts::TAU / segs;
            warp(p, None, move |x, y| {
                let (qx, qy) = (x - c.0, y - c.1);
                let r = (qx * qx + qy * qy).sqrt();
                let mut m = (qy.atan2(qx) - a).rem_euclid(seg);
                if m > seg / 2.0 {
                    m = seg - m;
                }
                let th = m + a;
                (c.0 + r * th.cos(), c.1 + r * th.sin())
            });
        }
        "motionTile" => {
            let (tw, th) = (n("width").max(1.0) * k, n("height").max(1.0) * k);
            let off = e.v2("offset");
            let centre = match cx.corners {
                Some(q) => ((q[0][0] + q[1][0] + q[2][0] + q[3][0]) / 4.0, (q[0][1] + q[1][1] + q[2][1] + q[3][1]) / 4.0),
                None => at_point(cx.base, 0.0, 0.0),
            };
            let c = (centre.0 + off[0] as f32 * k, centre.1 + off[1] as f32 * k);
            let mirror = e.b("mirror");
            let fold = move |v: f32, size: f32| {
                let i = (v / size).floor();
                let f = v - i * size;
                if mirror && (i as i64).rem_euclid(2) == 1 { size - f } else { f }
            };
            warp(p, None, move |x, y| (c.0 - tw / 2.0 + fold(x - c.0 + tw / 2.0, tw), c.1 - th / 2.0 + fold(y - c.1 + th / 2.0, th)));
        }
        "cornerPin" => {
            let from = cx.corners.or_else(|| ink(p).map(|r| r.corners()));
            if let Some(from) = from {
                let names = ["topLeft", "topRight", "bottomRight", "bottomLeft"];
                let to: [[f32; 2]; 4] = std::array::from_fn(|i| {
                    let d = e.v2(names[i]);
                    [from[i][0] + d[0] as f32 * k, from[i][1] + d[1] as f32 * k]
                });
                corner_pin(p, from, to);
            }
        }
        "sharpen" => sharpen(p, n("amount"), k),
        other => tracing::debug!(effect = other, "unknown effect type; skipped"),
    }
}

fn rgba(s: &str) -> [f32; 4] {
    let c = stack::rgba(s).0;
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

fn at_point(base: Transform, x: f32, y: f32) -> (f32, f32) {
    let mut p = Point::from_xy(x, y);
    base.map_point(&mut p);
    (p.x, p.y)
}

fn luma(v: [f32; 3]) -> f32 {
    0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rotation of colours around the grey axis by `deg` degrees.
fn hue_matrix(deg: f32) -> [[f32; 3]; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    let (a, b) = ((1.0 - c) / 3.0, (1.0f32 / 3.0).sqrt() * s);
    [[c + a, a - b, a + b], [a + b, c + a, a - b], [a - b, a + b, c + a]]
}

fn mul3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

// ---------------------------------------------------------------------------------------------
// Regions and pixels

/// A rectangle of pixels; `x1`, `y1` exclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Roi {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Roi {
    pub(crate) fn full(p: &Pixmap) -> Roi {
        Roi { x0: 0, y0: 0, x1: p.width() as usize, y1: p.height() as usize }
    }

    pub(crate) fn w(&self) -> usize {
        self.x1 - self.x0
    }

    pub(crate) fn h(&self) -> usize {
        self.y1 - self.y0
    }

    /// Grown by `m` pixels on every side, within `p`.
    pub(crate) fn grow(self, m: f32, p: &Pixmap) -> Roi {
        let m = if m.is_finite() { m.max(0.0).ceil().min(1e6) as usize } else { 1_000_000 };
        Roi { x0: self.x0.saturating_sub(m), y0: self.y0.saturating_sub(m), x1: (self.x1 + m).min(p.width() as usize), y1: (self.y1 + m).min(p.height() as usize) }
    }

    fn corners(&self) -> [[f32; 2]; 4] {
        let (a, b, c, d) = (self.x0 as f32, self.y0 as f32, self.x1 as f32, self.y1 as f32);
        [[a, b], [c, b], [c, d], [a, d]]
    }
}

/// The smallest rectangle holding every pixel with any alpha.
pub(crate) fn ink(p: &Pixmap) -> Option<Roi> {
    let w = p.width() as usize;
    let data = p.data();
    let rows: Vec<Option<(usize, usize)>> = data
        .par_chunks(w * 4)
        .map(|row| {
            let first = (0..w).find(|&x| row[x * 4 + 3] > 0)?;
            let last = (0..w).rev().find(|&x| row[x * 4 + 3] > 0)?;
            Some((first, last))
        })
        .collect();
    let y0 = rows.iter().position(Option::is_some)?;
    let y1 = rows.iter().rposition(Option::is_some)?;
    let (mut x0, mut x1) = (w, 0);
    for (a, b) in rows[y0..=y1].iter().flatten() {
        x0 = x0.min(*a);
        x1 = x1.max(*b);
    }
    Some(Roi { x0, y0, x1: x1 + 1, y1: y1 + 1 })
}

/// Premultiplied RGBA, 0–255.
type Px = [f32; 4];

fn read(p: &Pixmap, r: Roi) -> Vec<Px> {
    let w = p.width() as usize;
    let data = p.data();
    let mut out = vec![[0.0; 4]; r.w() * r.h()];
    out.par_chunks_mut(r.w().max(1)).enumerate().for_each(|(j, row)| {
        let src = &data[((r.y0 + j) * w + r.x0) * 4..((r.y0 + j) * w + r.x1) * 4];
        for (o, s) in row.iter_mut().zip(src.chunks_exact(4)) {
            *o = [s[0] as f32, s[1] as f32, s[2] as f32, s[3] as f32];
        }
    });
    out
}

fn store(px: Px) -> [u8; 4] {
    let a = px[3].clamp(0.0, 255.0);
    let c = |v: f32| v.clamp(0.0, a).round() as u8;
    [c(px[0]), c(px[1]), c(px[2]), a.round() as u8]
}

fn write(p: &mut Pixmap, r: Roi, buf: &[Px]) {
    let w = p.width() as usize;
    let rw = r.w().max(1);
    rows_mut(p.data_mut(), w, r, |j, row| {
        for (o, s) in row.chunks_exact_mut(4).zip(&buf[(j - r.y0) * rw..]) {
            o.copy_from_slice(&store(*s));
        }
    });
}

/// Runs `f(y, row)` on the rows of `r` in parallel; `row` is the part of the row inside `r`.
fn rows_mut(data: &mut [u8], w: usize, r: Roi, f: impl Fn(usize, &mut [u8]) + Sync) {
    if r.w() == 0 || r.h() == 0 {
        return;
    }
    data.par_chunks_mut(w * 4).enumerate().skip(r.y0).take(r.h()).for_each(|(y, row)| f(y, &mut row[r.x0 * 4..r.x1 * 4]));
}

/// Bilinear sample at (x, y) (pixel centres at .5) through `get`; outside is transparent.
#[inline]
fn bilinear(w: usize, h: usize, x: f32, y: f32, get: impl Fn(usize) -> Px) -> Px {
    let (fx, fy) = (x - 0.5, y - 0.5);
    if !(fx > -1.0 && fy > -1.0 && fx < w as f32 && fy < h as f32) {
        return [0.0; 4];
    }
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let at = |x: isize, y: isize| if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h { get(y as usize * w + x as usize) } else { [0.0; 4] };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    std::array::from_fn(|i| (a[i] * (1.0 - tx) + b[i] * tx) * (1.0 - ty) + (c[i] * (1.0 - tx) + d[i] * tx) * ty)
}

fn sample_u8(src: &[u8], w: usize, h: usize, x: f32, y: f32) -> Px {
    bilinear(w, h, x, y, |i| {
        let s = &src[i * 4..i * 4 + 4];
        [s[0] as f32, s[1] as f32, s[2] as f32, s[3] as f32]
    })
}

/// Recolours every pixel with ink: `f(straight rgb 0–1, x, y)` (pixel centre), alpha kept.
fn map_rgb(p: &mut Pixmap, f: impl Fn([f32; 3], f32, f32) -> [f32; 3] + Sync) {
    map_rgba(p, None, move |v, x, y| {
        let c = f([v[0], v[1], v[2]], x, y);
        [c[0], c[1], c[2], v[3]]
    });
}

/// Every pixel with ink (in `roi`, or everywhere) through `f(straight rgba 0–1, x, y)`.
fn map_rgba(p: &mut Pixmap, roi: Option<Roi>, f: impl Fn([f32; 4], f32, f32) -> [f32; 4] + Sync) {
    let Some(r) = roi.or_else(|| ink(p)) else { return };
    let w = p.width() as usize;
    rows_mut(p.data_mut(), w, r, |y, row| {
        for (i, px) in row.chunks_exact_mut(4).enumerate() {
            let a = px[3];
            if a == 0 {
                continue;
            }
            let af = a as f32;
            let v = [px[0] as f32 / af, px[1] as f32 / af, px[2] as f32 / af, af / 255.0];
            let o = f(v, (r.x0 + i) as f32 + 0.5, y as f32 + 0.5);
            let oa = o[3].clamp(0.0, 1.0) * 255.0;
            px.copy_from_slice(&store([o[0].clamp(0.0, 1.0) * oa, o[1].clamp(0.0, 1.0) * oa, o[2].clamp(0.0, 1.0) * oa, oa]));
        }
    });
}

/// Moves pixels: each pixel of `roi` (or the whole picture) shows the picture at `f(x, y)`.
fn warp(p: &mut Pixmap, roi: Option<Roi>, f: impl Fn(f32, f32) -> (f32, f32) + Sync) {
    let r = roi.unwrap_or_else(|| Roi::full(p));
    let (w, h) = (p.width() as usize, p.height() as usize);
    let src = p.data().to_vec();
    rows_mut(p.data_mut(), w, r, |y, row| {
        for (i, px) in row.chunks_exact_mut(4).enumerate() {
            let (sx, sy) = f((r.x0 + i) as f32 + 0.5, y as f32 + 0.5);
            px.copy_from_slice(&store(sample_u8(&src, w, h, sx, sy)));
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Blur

/// Box blur of radius `r` along each row of `buf` (`w` wide); outside counts as transparent.
fn box_rows<const N: usize>(buf: &mut [[f32; N]], w: usize, r: usize) {
    if r == 0 || w == 0 {
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    buf.par_chunks_mut(w).for_each(|row| {
        let src = row.to_vec();
        let mut acc = [0.0f32; N];
        for s in &src[..(r + 1).min(w)] {
            for c in 0..N {
                acc[c] += s[c];
            }
        }
        for i in 0..w {
            row[i] = acc.map(|v| v * norm);
            if i + r + 1 < w {
                for c in 0..N {
                    acc[c] += src[i + r + 1][c];
                }
            }
            if i >= r {
                for c in 0..N {
                    acc[c] -= src[i - r][c];
                }
            }
        }
    });
}

fn transpose<T: Copy + Send + Sync + Default>(buf: &[T], w: usize, h: usize) -> Vec<T> {
    let mut out = vec![T::default(); w * h];
    out.par_chunks_mut(h.max(1)).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = buf[y * w + x];
        }
    });
    out
}

/// Gaussian blur (standard deviations `sx`, `sy` in pixels) of a `w`×`h` buffer, in place.
fn blur_buf<const N: usize>(buf: &mut Vec<[f32; N]>, w: usize, h: usize, sx: f32, sy: f32)
where
    [f32; N]: Default,
{
    if sx >= 0.3 {
        for r in crate::text::box_radii(sx) {
            box_rows(buf, w, r);
        }
    }
    if sy >= 0.3 {
        let mut t = transpose(buf, w, h);
        for r in crate::text::box_radii(sy) {
            box_rows(&mut t, h, r);
        }
        *buf = transpose(&t, h, w);
    }
}

/// Gaussian blur of a buffer, on a smaller copy when the blur is wide (same look, much faster).
fn blur_scaled<const N: usize>(buf: &mut Vec<[f32; N]>, w: usize, h: usize, sx: f32, sy: f32)
where
    [f32; N]: Default,
{
    let f = ((sx.max(sy) / 3.0).floor() as usize).clamp(1, 16);
    if f < 2 || w < f * 2 || h < f * 2 {
        blur_buf(buf, w, h, sx, sy);
        return;
    }
    let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
    let mut small = vec![[0.0f32; N]; sw * sh];
    small.par_chunks_mut(sw).enumerate().for_each(|(j, row)| {
        for (i, o) in row.iter_mut().enumerate() {
            let mut n = 0.0;
            for y in j * f..((j + 1) * f).min(h) {
                for x in i * f..((i + 1) * f).min(w) {
                    let s = buf[y * w + x];
                    for c in 0..N {
                        o[c] += s[c];
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                *o = o.map(|v| v / n);
            }
        }
    });
    blur_buf(&mut small, sw, sh, sx / f as f32, sy / f as f32);
    let k = 1.0 / f as f32;
    buf.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            // Bilinear, clamped to the edge of the small copy.
            let fx = ((x as f32 + 0.5) * k - 0.5).clamp(0.0, (sw - 1) as f32);
            let fy = ((y as f32 + 0.5) * k - 0.5).clamp(0.0, (sh - 1) as f32);
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let (a, b, c, d) = (small[y0 * sw + x0], small[y0 * sw + x1], small[y1 * sw + x0], small[y1 * sw + x1]);
            *o = std::array::from_fn(|i| (a[i] * (1.0 - tx) + b[i] * tx) * (1.0 - ty) + (c[i] * (1.0 - tx) + d[i] * tx) * ty);
        }
    });
}

/// Gaussian blur of a picture with standard deviations `sx`, `sy` (picture pixels).
pub(crate) fn gaussian(p: &mut Pixmap, sx: f32, sy: f32) {
    let (sx, sy) = (finite(sx), finite(sy));
    if sx < 0.3 && sy < 0.3 {
        return;
    }
    let Some(r) = ink(p).map(|r| r.grow(3.0 * sx.max(sy) + 2.0, p)) else { return };
    let mut buf = read(p, r);
    blur_scaled(&mut buf, r.w(), r.h(), sx, sy);
    write(p, r, &buf);
}

/// Gaussian blur of a single-channel plane (0–1 coverage), in place.
pub(crate) fn blur_plane(plane: &mut [f32], w: usize, h: usize, sigma: f32) {
    let sigma = finite(sigma);
    if sigma < 0.3 || w == 0 || h == 0 {
        return;
    }
    let mut buf: Vec<[f32; 1]> = plane.iter().map(|v| [*v]).collect();
    blur_scaled(&mut buf, w, h, sigma, sigma);
    for (o, v) in plane.iter_mut().zip(buf) {
        *o = v[0];
    }
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1e5) } else { 0.0 }
}

fn directional(p: &mut Pixmap, len: f32, angle: f32) {
    let len = finite(len);
    if len < 1.0 {
        return;
    }
    let Some(r) = ink(p).map(|r| r.grow(len / 2.0 + 2.0, p)) else { return };
    let radii: Vec<usize> = if len < 6.0 { vec![(len / 2.0).round() as usize] } else { vec![(len / 4.0).round() as usize; 2] };
    let a = angle.to_radians();
    let (s, c) = a.sin_cos();
    let mut buf = read(p, r);
    let (w, h) = (r.w(), r.h());
    if s.abs() < 1e-3 {
        for &rad in &radii {
            box_rows(&mut buf, w, rad);
        }
        write(p, r, &buf);
        return;
    }
    if c.abs() < 1e-3 {
        let mut t = transpose(&buf, w, h);
        for &rad in &radii {
            box_rows(&mut t, h, rad);
        }
        write(p, r, &transpose(&t, h, w));
        return;
    }
    // Turn the picture so the blur runs along rows, blur, turn it back.
    let d = ((w * w + h * h) as f32).sqrt().ceil() as usize + 2;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let half = d as f32 / 2.0;
    let mut rot = vec![[0.0f32; 4]; d * d];
    rot.par_chunks_mut(d).enumerate().for_each(|(i, row)| {
        for (j, o) in row.iter_mut().enumerate() {
            let (u, v) = (j as f32 + 0.5 - half, i as f32 + 0.5 - half);
            *o = bilinear(w, h, cx + u * c - v * s, cy + u * s + v * c, |k| buf[k]);
        }
    });
    for &rad in &radii {
        box_rows(&mut rot, d, rad);
    }
    let mut out = vec![[0.0f32; 4]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (qx, qy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            *o = bilinear(d, d, qx * c + qy * s + half, -qx * s + qy * c + half, |k| rot[k]);
        }
    });
    write(p, r, &out);
}

/// Zoom (rays from `c`) or spin (circles around it) blur. Each pass averages the picture with
/// itself scaled or turned a little, halving the step: 2ⁿ samples for n passes.
fn radial(p: &mut Pixmap, amount: f32, spin: bool, c: (f32, f32)) {
    if amount <= 0.0 {
        return;
    }
    let (w, h) = (p.width() as usize, p.height() as usize);
    let far = [(0.0, 0.0), (w as f32, 0.0), (0.0, h as f32), (w as f32, h as f32)]
        .iter()
        .map(|(x, y)| ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt())
        .fold(1.0f32, f32::max);
    let span = if spin { amount.to_radians() } else { amount / 100.0 };
    let reach = (span * far).min(4096.0);
    if reach < 0.5 {
        return;
    }
    let passes = (reach.log2().ceil() as i32).clamp(1, 8);
    let mut buf = read(p, Roi::full(p));
    for j in 1..=passes {
        let step = span / 2.0 / 2f32.powi(j - 1) / 2.0;
        let src = buf.clone();
        buf.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let (qx, qy) = (x as f32 + 0.5 - c.0, y as f32 + 0.5 - c.1);
                let (a, b) = if spin {
                    let (s, co) = step.sin_cos();
                    ((c.0 + qx * co - qy * s, c.1 + qx * s + qy * co), (c.0 + qx * co + qy * s, c.1 - qx * s + qy * co))
                } else {
                    let (up, down) = (step.exp(), (-step).exp());
                    ((c.0 + qx * up, c.1 + qy * up), (c.0 + qx * down, c.1 + qy * down))
                };
                let pa = bilinear(w, h, a.0, a.1, |k| src[k]);
                let pb = bilinear(w, h, b.0, b.1, |k| src[k]);
                *o = std::array::from_fn(|i| (pa[i] + pb[i]) * 0.5);
            }
        });
    }
    write(p, Roi::full(p), &buf);
}

fn glow(p: &mut Pixmap, radius: f32, intensity: f32, threshold: f32, tint: [f32; 4]) {
    let Some(r) = ink(p).map(|r| r.grow(finite(radius) * 1.5 + 2.0, p)) else { return };
    let mut light = read(p, r);
    light.par_iter_mut().for_each(|v| {
        let a = v[3];
        if a <= 0.0 {
            return;
        }
        let l = luma([v[0] / a, v[1] / a, v[2] / a]);
        let keep = if threshold <= 0.0 { 1.0 } else { smoothstep(threshold, threshold + 0.15, l) };
        *v = if tint[3] > 0.0 {
            let k = a * keep * tint[3];
            [tint[0] * k, tint[1] * k, tint[2] * k, k]
        } else {
            v.map(|c| c * keep)
        };
    });
    blur_scaled(&mut light, r.w(), r.h(), finite(radius) / 2.0, finite(radius) / 2.0);
    let mut buf = read(p, r);
    for (o, g) in buf.iter_mut().zip(&light) {
        for c in 0..4 {
            o[c] += g[c] * intensity;
        }
        // Light only adds: colour can't go past what the alpha holds.
        o[3] = o[3].min(255.0);
        for c in 0..3 {
            o[c] = o[c].min(o[3]);
        }
    }
    write(p, r, &buf);
}

/// A copy of the picture's alpha in `tint` (straight 0–1).
fn silhouette(p: &Pixmap, tint: [f32; 4]) -> Pixmap {
    let mut s = Pixmap::new(p.width(), p.height()).expect("same size");
    let k = tint[3];
    for (o, src) in s.data_mut().chunks_exact_mut(4).zip(p.data().chunks_exact(4)) {
        let a = src[3] as f32 * k;
        if a > 0.0 {
            o.copy_from_slice(&store([tint[0] * a, tint[1] * a, tint[2] * a, a]));
        }
    }
    s
}

fn drop_shadow(p: &mut Pixmap, color: [f32; 4], softness: f32, dx: f32, dy: f32) {
    if color[3] <= 0.0 || ink(p).is_none() {
        return;
    }
    let mut s = silhouette(p, color);
    gaussian(&mut s, finite(softness) / 2.0, finite(softness) / 2.0);
    let (dx, dy) = (if dx.is_finite() { dx } else { 0.0 }, if dy.is_finite() { dy } else { 0.0 });
    let paint = PixmapPaint { blend_mode: BlendMode::DestinationOver, quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    p.draw_pixmap(0, 0, s.as_ref(), &paint, Transform::from_translate(dx, dy), None);
}

/// Euclidean distance (pixels, between centres) from every pixel of a `w`×`h` grid to the
/// nearest one where `inside` is true (Felzenszwalb & Huttenlocher's linear-time transform).
fn distance(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    const BIG: f64 = 1e12;
    fn pass(f: &[f64], out: &mut [f64]) {
        let n = f.len();
        if n == 0 {
            return;
        }
        let mut v = vec![0usize; n];
        let mut z = vec![0f64; n + 1];
        let mut k = 0usize;
        z[0] = f64::NEG_INFINITY;
        z[1] = f64::INFINITY;
        let meet = |q: usize, p: usize| ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
        for q in 1..n {
            let mut s = meet(q, v[k]);
            while k > 0 && s <= z[k] {
                k -= 1;
                s = meet(q, v[k]);
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
        }
        k = 0;
        for (q, o) in out.iter_mut().enumerate() {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let d = q as f64 - v[k] as f64;
            *o = d * d + f[v[k]];
        }
    }
    let grid: Vec<f64> = inside.iter().map(|&b| if b { 0.0 } else { BIG }).collect();
    // Columns (as rows of the transposed grid), then rows.
    let mut cols = transpose(&grid, w, h);
    cols.par_chunks_mut(h.max(1)).for_each(|col| {
        let f = col.to_vec();
        pass(&f, col);
    });
    let mut rows = transpose(&cols, h, w);
    rows.par_chunks_mut(w.max(1)).for_each(|row| {
        let f = row.to_vec();
        pass(&f, row);
    });
    rows.into_iter().map(|d| (d.min(BIG)).sqrt() as f32).collect()
}

/// An outline around the picture's shape (where it is at least half opaque).
fn outline(p: &mut Pixmap, color: [f32; 4], width: f32, position: &str) {
    let width = finite(width);
    if width <= 0.0 || color[3] <= 0.0 {
        return;
    }
    let Some(r) = ink(p).map(|r| r.grow(width + 2.0, p)) else { return };
    let mut buf = read(p, r);
    let (w, h) = (r.w(), r.h());
    let solid: Vec<bool> = buf.iter().map(|v| v[3] >= 127.5).collect();
    let (outside_w, inside_w) = match position {
        "inside" => (0.0, width),
        "center" => (width / 2.0, width / 2.0),
        _ => (width, 0.0),
    };
    let paint = |cov: f32| -> Px {
        let a = cov * color[3] * 255.0;
        [color[0] * a, color[1] * a, color[2] * a, a]
    };
    if inside_w > 0.0 {
        let holes: Vec<bool> = solid.iter().map(|b| !b).collect();
        let d = distance(&holes, w, h);
        for (i, o) in buf.iter_mut().enumerate() {
            if o[3] <= 0.0 {
                continue;
            }
            let cov = (inside_w + 0.5 - d[i]).clamp(0.0, 1.0) * o[3] / 255.0;
            if cov > 0.0 {
                let s = paint(cov);
                // Over the layer, within its shape.
                *o = std::array::from_fn(|c| s[c] + o[c] * (1.0 - s[3] / 255.0));
            }
        }
    }
    if outside_w > 0.0 {
        let d = distance(&solid, w, h);
        for (i, o) in buf.iter_mut().enumerate() {
            let cov = (outside_w + 0.5 - d[i]).clamp(0.0, 1.0);
            if cov > 0.0 {
                let s = paint(cov);
                // Under the layer.
                *o = std::array::from_fn(|c| o[c] + s[c] * (1.0 - o[3] / 255.0));
            }
        }
    }
    write(p, r, &buf);
}

fn halftone(p: &mut Pixmap, size: f32, angle: f32, colored: bool, origin: (f32, f32)) {
    let size = finite(size).max(1.0);
    let Some(r) = ink(p).map(|r| r.grow(size, p)) else { return };
    let (w, h) = (p.width() as usize, p.height() as usize);
    let src = p.data().to_vec();
    let (s, c) = angle.to_radians().sin_cos();
    rows_mut(p.data_mut(), w, r, |y, row| {
        for (i, px) in row.chunks_exact_mut(4).enumerate() {
            let (x, yy) = ((r.x0 + i) as f32 + 0.5 - origin.0, y as f32 + 0.5 - origin.1);
            // Into the dot grid's own (turned) space.
            let (gx, gy) = (x * c + yy * s, -x * s + yy * c);
            let (cx, cy) = (((gx / size).floor() + 0.5) * size, ((gy / size).floor() + 0.5) * size);
            let (px_, py_) = (cx * c - cy * s + origin.0, cx * s + cy * c + origin.1);
            let v = sample_u8(&src, w, h, px_, py_);
            let a = v[3] / 255.0;
            let dist = ((gx - cx).powi(2) + (gy - cy).powi(2)).sqrt();
            let full = size * std::f32::consts::FRAC_1_SQRT_2;
            let out = if colored {
                let level = if v[3] > 0.0 { v[0].max(v[1]).max(v[2]) / v[3] * a } else { 0.0 };
                let cov = (full * level.sqrt() - dist + 0.5).clamp(0.0, 1.0);
                if v[3] > 0.0 { std::array::from_fn(|k| if k < 3 { v[k] / v[3] * 255.0 * cov } else { 255.0 * cov }) } else { [0.0; 4] }
            } else {
                let here = px[3] as f32;
                let dark = if v[3] > 0.0 { 1.0 - luma([v[0] / v[3], v[1] / v[3], v[2] / v[3]]) } else { 0.0 };
                let cov = (full * dark.max(0.0).sqrt() - dist + 0.5).clamp(0.0, 1.0);
                let paper = (1.0 - cov) * here;
                [paper, paper, paper, here]
            };
            px.copy_from_slice(&store(out));
        }
    });
}

fn mosaic(p: &mut Pixmap, size: f32, origin: (f32, f32)) {
    let s = finite(size).max(1.0);
    if s < 1.5 {
        return;
    }
    let Some(r) = ink(p).map(|r| r.grow(s, p)) else { return };
    let (w, h) = (p.width() as usize, p.height() as usize);
    // Blocks line up with the canvas centre.
    let ox = origin.0.rem_euclid(s);
    let oy = origin.1.rem_euclid(s);
    let block = |v: usize, o: f32| (((v as f32 + 0.5 - o) / s).floor()) as i64;
    let (bx0, bx1) = (block(r.x0, ox), block(r.x1 - 1, ox));
    let (by0, by1) = (block(r.y0, oy), block(r.y1 - 1, oy));
    let (bw, bh) = ((bx1 - bx0 + 1) as usize, (by1 - by0 + 1) as usize);
    let span = |b: i64, o: f32, max: usize| {
        let a = ((b as f32 * s + o).ceil().max(0.0) as usize).min(max);
        let z = (((b + 1) as f32 * s + o).ceil().max(0.0) as usize).min(max);
        (a, z)
    };
    let data = p.data();
    let mut avg = vec![[0.0f32; 4]; bw * bh];
    avg.par_chunks_mut(bw).enumerate().for_each(|(j, row)| {
        let (ya, yz) = span(by0 + j as i64, oy, h);
        for (i, o) in row.iter_mut().enumerate() {
            let (xa, xz) = span(bx0 + i as i64, ox, w);
            let mut n = 0.0;
            for y in ya..yz {
                for x in xa..xz {
                    let s = &data[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    for c in 0..4 {
                        o[c] += s[c] as f32;
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                *o = o.map(|v| v / n);
            }
        }
    });
    rows_mut(p.data_mut(), w, r, |y, row| {
        let j = (block(y, oy) - by0).clamp(0, bh as i64 - 1) as usize;
        for (i, px) in row.chunks_exact_mut(4).enumerate() {
            let b = (block(r.x0 + i, ox) - bx0).clamp(0, bw as i64 - 1) as usize;
            px.copy_from_slice(&store(avg[j * bw + b]));
        }
    });
}

/// Red one way, blue the other, green in place (over `roi`, or the ink grown by the shift).
fn split_channels(p: &mut Pixmap, roi: Option<Roi>, dx: f32, dy: f32) {
    let (dx, dy) = (if dx.is_finite() { dx } else { 0.0 }, if dy.is_finite() { dy } else { 0.0 });
    if dx.abs() < 0.05 && dy.abs() < 0.05 {
        return;
    }
    let Some(r) = roi.or_else(|| ink(p).map(|r| r.grow(dx.abs().max(dy.abs()) + 1.0, p))) else { return };
    let (w, h) = (p.width() as usize, p.height() as usize);
    let src = p.data().to_vec();
    rows_mut(p.data_mut(), w, r, |y, row| {
        for (i, px) in row.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((r.x0 + i) as f32 + 0.5, y as f32 + 0.5);
            let red = sample_u8(&src, w, h, x - dx, y - dy);
            let green = sample_u8(&src, w, h, x, y);
            let blue = sample_u8(&src, w, h, x + dx, y + dy);
            let a = red[3].max(green[3]).max(blue[3]);
            px.copy_from_slice(&store([red[0], green[1], blue[2], a]));
        }
    });
}

fn glitch(p: &mut Pixmap, amount: f32, speed: f32, seed: u32, t: f64, k: f32) {
    if amount <= 0.0 {
        return;
    }
    let slot = if speed > 0.0 { (t * speed as f64).floor() as i64 as i32 } else { 0 };
    // Bursts: some moments glitch, the others are clean.
    if noise::unit(slot, 0, 1, seed) > amount.clamp(0.0, 1.0) * 0.8 + 0.2 {
        return;
    }
    let (w, h) = (p.width() as usize, p.height() as usize);
    let bands = 2 + (noise::unit(slot, 1, 1, seed) * 6.0) as i32;
    for b in 0..bands {
        let y0 = (noise::unit(slot, b, 2, seed) * h as f32) as usize;
        let tall = ((noise::unit(slot, b, 3, seed) * 0.12 + 0.01) * h as f32).max(1.0) as usize;
        let shift = (noise::unit(slot, b, 4, seed) - 0.5) * 2.0 * amount * w as f32 * 0.12;
        let r = Roi { x0: 0, y0: y0.min(h), x1: w, y1: (y0 + tall).min(h) };
        if r.h() == 0 {
            continue;
        }
        let src = p.data().to_vec();
        rows_mut(p.data_mut(), w, r, |y, row| {
            for (i, px) in row.chunks_exact_mut(4).enumerate() {
                px.copy_from_slice(&store(sample_u8(&src, w, h, i as f32 + 0.5 - shift, y as f32 + 0.5)));
            }
        });
        split_channels(p, Some(r), amount * 10.0 * k, 0.0);
    }
}

/// The projective map taking the unit square's corners to `q` (row-major 3×3).
fn square_to_quad(q: [[f64; 2]; 4]) -> [f64; 9] {
    let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = q;
    let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
    if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
        return [x1 - x0, x2 - x1, x0, y1 - y0, y2 - y1, y0, 0.0, 0.0, 1.0];
    }
    let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
    let den = dx1 * dy2 - dx2 * dy1;
    let den = if den.abs() < 1e-12 { 1e-12 } else { den };
    let g = (sx * dy2 - dx2 * sy) / den;
    let h = (dx1 * sy - sx * dy1) / den;
    [x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h, 1.0]
}

fn invert3(m: [f64; 9]) -> Option<[f64; 9]> {
    let [a, b, c, d, e, f, g, h, i] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-15 || !det.is_finite() {
        return None;
    }
    let k = 1.0 / det;
    Some([
        (e * i - f * h) * k,
        (c * h - b * i) * k,
        (b * f - c * e) * k,
        (f * g - d * i) * k,
        (a * i - c * g) * k,
        (c * d - a * f) * k,
        (d * h - e * g) * k,
        (b * g - a * h) * k,
        (a * e - b * d) * k,
    ])
}

/// Moves the corners `from` to `to` (top left, top right, bottom right, bottom left) with a
/// true perspective warp.
fn corner_pin(p: &mut Pixmap, from: [[f32; 2]; 4], to: [[f32; 2]; 4]) {
    if from == to {
        return;
    }
    let f64q = |q: [[f32; 2]; 4]| q.map(|c| [c[0] as f64, c[1] as f64]);
    // Unit square order: (0,0) (1,0) (1,1) (0,1).
    let src = square_to_quad(f64q(from));
    let dst = square_to_quad(f64q(to));
    let Some(dst_inv) = invert3(dst) else { return };
    warp(p, None, move |x, y| {
        let (x, y) = (x as f64, y as f64);
        // Output pixel → the unit square (behind the viewer: nothing) → the source picture.
        let u = [dst_inv[0] * x + dst_inv[1] * y + dst_inv[2], dst_inv[3] * x + dst_inv[4] * y + dst_inv[5], dst_inv[6] * x + dst_inv[7] * y + dst_inv[8]];
        if u[2] <= 1e-12 {
            return (-1e9, -1e9);
        }
        let (uu, vv) = (u[0] / u[2], u[1] / u[2]);
        let s = [src[0] * uu + src[1] * vv + src[2], src[3] * uu + src[4] * vv + src[5], src[6] * uu + src[7] * vv + src[8]];
        if s[2].abs() < 1e-12 || !(s[0].is_finite() && s[1].is_finite()) {
            return (-1e9, -1e9);
        }
        ((s[0] / s[2]) as f32, (s[1] / s[2]) as f32)
    });
}

fn sharpen(p: &mut Pixmap, amount: f32, k: f32) {
    if amount <= 0.0 {
        return;
    }
    let Some(r) = ink(p).map(|r| r.grow(3.0 * k + 2.0, p)) else { return };
    let mut soft = read(p, r);
    let sharp = soft.clone();
    blur_buf(&mut soft, r.w(), r.h(), k.max(0.5), k.max(0.5));
    let out: Vec<Px> = sharp
        .iter()
        .zip(&soft)
        .map(|(s, b)| {
            let a = s[3];
            [s[0] + (s[0] - b[0]) * amount, s[1] + (s[1] - b[1]) * amount, s[2] + (s[2] - b[2]) * amount, a]
        })
        .collect();
    write(p, r, &out);
}

