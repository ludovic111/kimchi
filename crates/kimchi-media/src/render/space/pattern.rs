//! Procedural material patterns (`material.pattern`: checker, stripes, dots, noise, marble, wood,
//! cells, bricks, gradient) baked into pictures the renderers wrap like any texture: one tile
//! covering texture space 0–1 (the pattern's `scale` repeats inside it, `textureScale` repeats
//! the tile), and with `bump`, a matching height map that dents the surface.
//!
//! Noise-based patterns repeat seamlessly across the tile's edges (their scale is rounded to a
//! whole number of repeats); the others do when their scale is whole.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::motion::Pattern;
use rayon::prelude::*;

use super::{Texture, linear_to_srgb, srgb_f};

/// A pattern as pictures: colours (sRGB) and, when it dents the surface, heights (grey, 0–255,
/// read as is) with the bump strength.
#[derive(Clone)]
pub(crate) struct Baked {
    pub color: Arc<Texture>,
    pub height: Option<(Arc<Texture>, f32)>,
}

/// `pattern` baked at `size`×`size` (cached by its parameters).
pub(crate) fn bake(pattern: &Pattern, size: u32) -> Option<Baked> {
    static CACHE: OnceLock<Mutex<HashMap<(String, u32), Baked>>> = OnceLock::new();
    pattern.spec()?;
    if !pattern.enabled {
        return None;
    }
    let size = size.clamp(8, 4096);
    let key = (format!("{}|{}", pattern.kind, serde_json::to_string(&pattern.params).unwrap_or_default()), size);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(b) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(b.clone());
    }
    let p = Params::of(pattern);
    let n = size as usize;
    let mut color = vec![0u8; n * n * 4];
    let mut height = vec![0u8; n * n * 4];
    color.par_chunks_mut(n * 4).zip(height.par_chunks_mut(n * 4)).enumerate().for_each(|(y, (crow, hrow))| {
        for x in 0..n {
            let (u, v) = ((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
            let (mix, h) = p.eval(u, v);
            let c: [f32; 3] = std::array::from_fn(|k| p.a[k] + (p.b[k] - p.a[k]) * mix.clamp(0.0, 1.0));
            let alpha = p.alpha_a + (p.alpha_b - p.alpha_a) * mix.clamp(0.0, 1.0);
            let px = &mut crow[x * 4..x * 4 + 4];
            for k in 0..3 {
                px[k] = (linear_to_srgb(c[k]) * 255.0).round() as u8;
            }
            px[3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            let g = (h.clamp(0.0, 1.0) * 255.0).round() as u8;
            hrow[x * 4..x * 4 + 4].copy_from_slice(&[g, g, g, 255]);
        }
    });
    let bump = pattern.n("bump") as f32;
    let baked = Baked {
        color: Arc::new(Texture { width: size, height: size, rgba: color }),
        height: (bump != 0.0 && p.kind != Kind::Gradient).then(|| (Arc::new(Texture { width: size, height: size, rgba: height }), bump)),
    };
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 24 {
        map.clear();
    }
    map.insert(key, baked.clone());
    Some(baked)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Checker,
    Stripes,
    Dots,
    Noise,
    Marble,
    Wood,
    Voronoi,
    Bricks,
    Gradient,
}

/// A pattern's parameters, read once.
struct Params {
    kind: Kind,
    /// Linear colours (first, second) and their alphas.
    a: [f32; 3],
    b: [f32; 3],
    alpha_a: f32,
    alpha_b: f32,
    scale: f32,
    seed: u32,
    width: f32,
    angle: f32,
    size: f32,
    detail: u32,
    contrast: f32,
    turbulence: f32,
    rings: f32,
    edge: f32,
    mortar: f32,
    ratio: f32,
    direction: String,
}

impl Params {
    fn of(p: &Pattern) -> Params {
        let lin = |name: &str| -> ([f32; 3], f32) {
            let c = crate::render::paint::color(&p.s(name));
            ([srgb_f(c.red()), srgb_f(c.green()), srgb_f(c.blue())], c.alpha())
        };
        let (a, alpha_a) = lin("color");
        let (b, alpha_b) = lin("color2");
        let kind = match p.kind.as_str() {
            "stripes" => Kind::Stripes,
            "dots" => Kind::Dots,
            "noise" => Kind::Noise,
            "marble" => Kind::Marble,
            "wood" => Kind::Wood,
            "voronoi" => Kind::Voronoi,
            "bricks" => Kind::Bricks,
            "gradient" => Kind::Gradient,
            _ => Kind::Checker,
        };
        // `n` gives 0 for parameters a type doesn't have.
        let or = |name: &str, d: f64| if p.spec().and_then(|s| s.param(name)).is_some() { p.n(name) } else { d };
        Params {
            kind,
            a,
            b,
            alpha_a,
            alpha_b,
            scale: or("scale", 4.0).max(0.001) as f32,
            seed: or("seed", 0.0) as u32,
            width: or("width", 0.5) as f32,
            angle: (or("angle", 0.0) as f32).to_radians(),
            size: or("size", 0.35) as f32,
            detail: or("detail", 4.0).clamp(1.0, 8.0) as u32,
            contrast: or("contrast", 1.0) as f32,
            turbulence: or("turbulence", 1.0) as f32,
            rings: or("rings", 12.0) as f32,
            edge: or("edge", 0.1) as f32,
            mortar: or("mortar", 0.05) as f32,
            ratio: or("ratio", 2.0).max(0.1) as f32,
            direction: p.s("direction"),
        }
    }

    /// How much of the second colour at (`u`, `v`), and the surface height there (both 0–1).
    fn eval(&self, u: f32, v: f32) -> (f32, f32) {
        let s = self.scale;
        // Whole repeats so the tile wraps without a seam.
        let period = s.round().max(1.0);
        match self.kind {
            Kind::Checker => {
                let (x, y) = ((u * s).floor() as i64, (v * s).floor() as i64);
                let k = ((x + y).rem_euclid(2)) as f32;
                (k, 1.0 - k)
            }
            Kind::Stripes => {
                let (sn, cs) = self.angle.sin_cos();
                let t = (u * cs + v * sn) * s;
                let f = t - t.floor();
                // Soft edges a texel or so wide, so bumps have a slope to follow.
                let w = self.width.clamp(0.0, 1.0);
                let k = 1.0 - smooth_band(f, w, 0.004 * s.max(1.0));
                (k, 1.0 - k)
            }
            Kind::Dots => {
                let (x, y) = (u * s, v * s);
                let (fx, fy) = (x - x.floor() - 0.5, y - y.floor() - 0.5);
                let d = (fx * fx + fy * fy).sqrt();
                let r = self.size.clamp(0.0, 0.5);
                let inside = 1.0 - smoothstep(r - 0.02, r + 0.02, d);
                // Dots are raised domes.
                let dome = if d < r && r > 0.0 { (1.0 - (d / r).powi(2)).sqrt() } else { 0.0 };
                (1.0 - inside, dome)
            }
            Kind::Noise => {
                let n = fbm(u * period, v * period, period as u32, self.detail, self.seed);
                let k = (0.5 + (n - 0.5) * self.contrast).clamp(0.0, 1.0);
                (k, n)
            }
            Kind::Marble => {
                let n = fbm(u * period, v * period, period as u32, 5, self.seed);
                // Veins: a sine across u, pushed around by the noise.
                let t = (u * period + self.turbulence * (n - 0.5)) * std::f32::consts::PI;
                let k = (0.5 + 0.5 * t.sin()).powf(0.6);
                (1.0 - k, k)
            }
            Kind::Wood => {
                let n = fbm(u * period, v * period * 0.25, period as u32, 3, self.seed);
                let rings = self.rings.max(0.0).round().max(1.0);
                let g = v * rings + self.turbulence * (n - 0.5) * 0.8 + 0.12 * (u * std::f32::consts::TAU).sin();
                let f = g - g.floor();
                let k = smoothstep(0.0, 0.18, f) * (1.0 - smoothstep(0.55, 1.0, f));
                (k, 1.0 - k * 0.6)
            }
            Kind::Voronoi => {
                let (f1, f2, cell) = voronoi(u * period, v * period, period as i64, self.seed);
                if self.edge > 0.0 {
                    let line = 1.0 - smoothstep(self.edge * 0.25, self.edge * 0.25 + 0.03, f2 - f1);
                    (line, (1.0 - line) * (1.0 - f1 * 0.5))
                } else {
                    (cell, 1.0 - f1)
                }
            }
            Kind::Bricks => {
                // `scale` bricks across a unit; rows as tall as a brick's width / ratio; every
                // other row half a brick along. Mortar is as thick both ways (a share of a
                // brick's height).
                let rows = (s * self.ratio).round().max(1.0);
                let (x, y) = (u * s, v * rows);
                let row = y.floor();
                let x = x + if (row as i64).rem_euclid(2) == 1 { 0.5 } else { 0.0 };
                let (fx, fy) = (x - x.floor(), y - row);
                let m = self.mortar.clamp(0.0, 0.5);
                let mortar = fx.min(1.0 - fx) < m / (2.0 * self.ratio) || fy.min(1.0 - fy) < m / 2.0;
                let cols = s.round().max(1.0) as i64;
                let shade = hash((x.floor() as i64).rem_euclid(cols) as u32, (row as i64).rem_euclid(rows as i64) as u32, self.seed, 7) * 0.25;
                if mortar { (1.0, 0.0) } else { (shade, 1.0) }
            }
            Kind::Gradient => {
                let t = match self.direction.as_str() {
                    "u" => u,
                    "radial" => (((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0).min(1.0),
                    _ => v,
                };
                (t, 0.0)
            }
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 1 inside the band `[0, w)` of a repeating 0–1 coordinate, with soft edges `soft` wide.
fn smooth_band(f: f32, w: f32, soft: f32) -> f32 {
    if w <= 0.0 {
        return 0.0;
    }
    if w >= 1.0 {
        return 1.0;
    }
    // Signed distance to the band's nearest edge (positive inside).
    let d = if f < w { f.min(w - f) } else { -(f - w).min(1.0 - f) };
    smoothstep(-soft, soft, d)
}

/// A deterministic random number 0–1 for a lattice point.
fn hash(x: u32, y: u32, seed: u32, salt: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f) ^ salt.wrapping_mul(0x1656_67b1);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 0x0100_0000 as f32
}

/// Smooth value noise 0–1 repeating every `period` units.
fn value_noise(x: f32, y: f32, period: u32, seed: u32, octave: u32) -> f32 {
    let p = period.max(1) as i64;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let q = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (sx, sy) = (q(fx), q(fy));
    let at = |i: i64, j: i64| hash(i.rem_euclid(p) as u32, j.rem_euclid(p) as u32, seed, octave);
    let (i, j) = (x0 as i64, y0 as i64);
    let a = at(i, j) + (at(i + 1, j) - at(i, j)) * sx;
    let b = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * sx;
    a + (b - a) * sy
}

/// Layers of finer noise (each twice the frequency, half the strength), 0–1, repeating every
/// `period` units.
fn fbm(x: f32, y: f32, period: u32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut f) = (0.0, 1.0, 0.0, 1u32);
    for o in 0..octaves.max(1) {
        sum += value_noise(x * f as f32, y * f as f32, period * f, seed, o) * amp;
        total += amp;
        amp *= 0.5;
        f *= 2;
    }
    sum / total
}

/// Distances to the nearest and second-nearest cell centres (in cells) and a random value for
/// the nearest cell, repeating every `period` cells.
fn voronoi(x: f32, y: f32, period: i64, seed: u32) -> (f32, f32, f32) {
    let (cx, cy) = (x.floor() as i64, y.floor() as i64);
    let (mut f1, mut f2, mut id) = (f32::MAX, f32::MAX, 0.0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (i, j) = (cx + dx, cy + dy);
            let (wi, wj) = (i.rem_euclid(period.max(1)) as u32, j.rem_euclid(period.max(1)) as u32);
            let px = i as f32 + 0.1 + 0.8 * hash(wi, wj, seed, 11);
            let py = j as f32 + 0.1 + 0.8 * hash(wi, wj, seed, 12);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hash(wi, wj, seed, 13);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1.min(1.0), f2.min(1.5), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::motion::stack::PATTERNS;

    fn pattern(json: serde_json::Value) -> Pattern {
        serde_json::from_value(json).unwrap()
    }

    fn texel(t: &Texture, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * t.width + x) * 4) as usize;
        [t.rgba[i], t.rgba[i + 1], t.rgba[i + 2], t.rgba[i + 3]]
    }

    #[test]
    fn every_pattern_bakes_with_both_colours() {
        for spec in PATTERNS {
            let p = pattern(serde_json::json!({"type": spec.name, "color": "#ff0000", "color2": "#0000ff", "bump": 1}));
            let b = bake(&p, 256).unwrap_or_else(|| panic!("{} bakes", spec.name));
            assert_eq!((b.color.width, b.color.height), (256, 256));
            let reds = b.color.rgba.as_chunks::<4>().0.iter().filter(|c| c[0] > 128 && c[2] < 128).count();
            let blues = b.color.rgba.as_chunks::<4>().0.iter().filter(|c| c[2] > 128 && c[0] < 128).count();
            assert!(reds > 0 && blues > 0, "{}: {reds} red, {blues} blue texels", spec.name);
            assert_eq!(b.height.is_some(), spec.name != "gradient", "{} has heights", spec.name);
        }
    }

    #[test]
    fn checkers_alternate_and_are_cached() {
        let p = pattern(serde_json::json!({"type": "checker", "color": "#ffffff", "color2": "#000000", "scale": 2}));
        let b = bake(&p, 32).unwrap();
        assert_eq!(texel(&b.color, 4, 4), [255, 255, 255, 255]);
        assert_eq!(texel(&b.color, 20, 4), [0, 0, 0, 255]);
        assert_eq!(texel(&b.color, 20, 20), [255, 255, 255, 255]);
        assert!(b.height.is_none(), "no bump, no heights");
        let again = bake(&p, 32).unwrap();
        assert!(Arc::ptr_eq(&b.color, &again.color), "baked once");
        let other = bake(&pattern(serde_json::json!({"type": "checker", "scale": 3})), 32).unwrap();
        assert!(!Arc::ptr_eq(&b.color, &other.color));
    }

    #[test]
    fn noise_tiles_without_a_seam() {
        let p = pattern(serde_json::json!({"type": "noise", "color": "#000000", "color2": "#ffffff", "scale": 3, "detail": 3}));
        let b = bake(&p, 128).unwrap();
        // The first and last columns are neighbours across the edge: close in value.
        let diff: f64 = (0..128).map(|y| (texel(&b.color, 0, y)[0] as f64 - texel(&b.color, 127, y)[0] as f64).abs()).sum::<f64>() / 128.0;
        assert!(diff < 12.0, "seam {diff}");
        let spread = b.color.rgba.as_chunks::<4>().0.iter().map(|c| c[0]).fold((255u8, 0u8), |(lo, hi), v| (lo.min(v), hi.max(v)));
        assert!(spread.1 - spread.0 > 60, "varied: {spread:?}");
    }
}
