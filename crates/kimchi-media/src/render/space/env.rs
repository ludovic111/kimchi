//! The world around a 3D scene ([`Env`]): its colour in every direction (one colour, a
//! gradient, an analytic daylight sky or a 360° panorama picture), and what the renderers need to
//! light with it quickly: a chain of ever blurrier copies for reflections on rougher and rougher
//! surfaces ([`EnvMaps::levels`], equirectangular, one per mip level) and nine spherical-harmonic
//! coefficients for the soft light it gives matte surfaces ([`EnvMaps::sh`]).
//!
//! Directions map to the panorama like Blender's: the picture's middle column looks down −z,
//! u grows turning towards +x, v goes from straight up (0) to straight down (1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::math::V3;
use super::{Env, EnvKind, Texture, srgb_to_linear};

/// Width of the sharpest level (height is half).
const BASE_W: usize = 256;
/// Levels in the chain: roughness 0 → level 0, roughness 1 → the last one.
pub(crate) const LEVELS: usize = 6;

/// One level of the reflection chain: linear RGB (strength applied), alpha unused (1).
#[derive(Debug, Clone)]
pub(crate) struct EnvLevel {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<[f32; 4]>,
}

/// Lighting made from an environment.
#[derive(Debug)]
pub(crate) struct EnvMaps {
    /// Equirectangular radiance, level k is `256 >> k` wide and blurred for roughness k / 5.
    pub levels: Vec<EnvLevel>,
    /// Irradiance as spherical harmonics, already divided by π: a matte surface facing `n`
    /// gets `albedo × sh_eval(n)`.
    pub sh: [[f32; 3]; 9],
}

impl Env {
    /// The world's colour seen in direction `d` (unit, world space), linear, strength applied.
    pub(crate) fn radiance(&self, d: V3) -> [f32; 3] {
        let d = rotate_y(d, -self.rotation);
        match self.kind {
            EnvKind::Color => self.color,
            EnvKind::Gradient => gradient(self.top, self.horizon, self.bottom, d.1),
            EnvKind::Sky => sky(d, self.sun, self.sun_color, self.strength),
            EnvKind::Image => match &self.image {
                Some(img) => {
                    let (u, v) = dir_to_uv(d);
                    let s = sample_srgb_clamped(img, u, v);
                    [s[0] * self.strength, s[1] * self.strength, s[2] * self.strength]
                }
                None => self.color,
            },
        }
    }

    /// The blurred reflection chain and irradiance for this world (cached by what it shows).
    pub(crate) fn build_maps(&self) -> Arc<EnvMaps> {
        static CACHE: OnceLock<Mutex<HashMap<String, Arc<EnvMaps>>>> = OnceLock::new();
        let key = format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}|{:?}|{}",
            self.kind,
            self.color,
            self.top,
            self.horizon,
            self.bottom,
            self.strength,
            self.rotation,
            // The sky follows the sun; other kinds don't.
            if self.kind == EnvKind::Sky { self.sun.arr() } else { [0.0; 3] },
            if self.kind == EnvKind::Sky { self.sun_color } else { [0.0; 3] },
            self.image.as_ref().map_or(0, |i| Arc::as_ptr(i) as usize),
        );
        let cache = CACHE.get_or_init(Default::default);
        if let Some(m) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return m.clone();
        }
        let maps = Arc::new(self.compute_maps());
        let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 8 {
            map.clear();
        }
        map.insert(key, maps.clone());
        maps
    }

    fn compute_maps(&self) -> EnvMaps {
        let (w, h) = (BASE_W, BASE_W / 2);
        // Level 0: the radiance at each texel (pictures averaged over the texel's footprint).
        let sub = if self.kind == EnvKind::Image { 3 } else { 1 };
        let mut rgba = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 3];
                for sy in 0..sub {
                    for sx in 0..sub {
                        let u = (x as f32 + (sx as f32 + 0.5) / sub as f32) / w as f32;
                        let v = (y as f32 + (sy as f32 + 0.5) / sub as f32) / h as f32;
                        // Texels are laid out in the world's own (unrotated) frame; lookups rotate.
                        let c = self.radiance(rotate_y(uv_to_dir(u, v), self.rotation));
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                    }
                }
                let n = (sub * sub) as f32;
                rgba.push([acc[0] / n, acc[1] / n, acc[2] / n, 1.0]);
            }
        }
        let mut levels = vec![EnvLevel { width: w, height: h, rgba }];
        let mut prev_sigma = 0.0f32;
        for k in 1..LEVELS {
            let half = downsample(levels.last().expect("level 0"));
            // The lobe of roughness k/5, as an angle, minus what the previous level already blurred.
            let angle = lobe_angle(k as f32 / (LEVELS - 1) as f32);
            let extra = (angle * angle - prev_sigma * prev_sigma).max(0.0).sqrt();
            prev_sigma = angle;
            levels.push(blur(&half, extra));
        }
        let sh = project_sh(&levels[3.min(LEVELS - 1)]);
        EnvMaps { levels, sh }
    }
}

impl EnvMaps {
    /// Reflection colour in direction `d` (world, unit) for a surface of roughness `rough`,
    /// trilinear between levels like the GPU sampler. `rotation` is the world's turn (radians).
    pub(crate) fn reflect(&self, d: V3, rotation: f32, rough: f32) -> [f32; 3] {
        let (u, v) = dir_to_uv(rotate_y(d, -rotation));
        let lod = (rough.clamp(0.0, 1.0) * (LEVELS - 1) as f32).clamp(0.0, (LEVELS - 1) as f32);
        let l0 = lod.floor() as usize;
        let l1 = (l0 + 1).min(LEVELS - 1);
        let f = lod - l0 as f32;
        let a = sample_level(&self.levels[l0], u, v);
        if f <= 0.0 || l0 == l1 {
            return a;
        }
        let b = sample_level(&self.levels[l1], u, v);
        [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
    }

    /// Light reaching a matte surface facing `n` (world, unit), divided by π.
    pub(crate) fn irradiance(&self, n: V3, rotation: f32) -> [f32; 3] {
        let n = rotate_y(n, -rotation);
        let b = sh_basis(n);
        let mut out = [0.0f32; 3];
        for (c, k) in self.sh.iter().zip(b) {
            for i in 0..3 {
                out[i] += c[i] * k;
            }
        }
        [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
    }
}

/// Half the width of a reflection lobe of `rough`ness, in radians (the same Blinn-Phong lobe
/// the shading uses).
fn lobe_angle(rough: f32) -> f32 {
    let r = rough.clamp(0.04, 1.0);
    let shininess = (2.0 / (r * r * r * r) - 2.0).clamp(1.0, 4096.0);
    (2.0 / (shininess + 2.0)).sqrt()
}

/// Turns `d` by `angle` radians around the vertical axis.
pub(crate) fn rotate_y(d: V3, angle: f32) -> V3 {
    if angle == 0.0 {
        return d;
    }
    let (s, c) = angle.sin_cos();
    V3(d.0 * c + d.2 * s, d.1, -d.0 * s + d.2 * c)
}

/// Direction → panorama coordinates (u right, v down, both 0–1).
pub(crate) fn dir_to_uv(d: V3) -> (f32, f32) {
    let u = d.0.atan2(-d.2) / std::f32::consts::TAU + 0.5;
    let v = d.1.clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
    (u, v)
}

/// Panorama coordinates → direction.
pub(crate) fn uv_to_dir(u: f32, v: f32) -> V3 {
    let phi = (u - 0.5) * std::f32::consts::TAU;
    let theta = v * std::f32::consts::PI;
    V3(theta.sin() * phi.sin(), theta.cos(), -theta.sin() * phi.cos())
}

/// Top above the horizon, bottom below, blending quickly near the horizon.
pub(crate) fn gradient(top: [f32; 3], horizon: [f32; 3], bottom: [f32; 3], y: f32) -> [f32; 3] {
    let (to, k) = if y >= 0.0 { (top, y.min(1.0).sqrt()) } else { (bottom, (-y).min(1.0).sqrt()) };
    [horizon[0] + (to[0] - horizon[0]) * k, horizon[1] + (to[1] - horizon[1]) * k, horizon[2] + (to[2] - horizon[2]) * k]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// A simple daylight sky: deep blue overhead, pale at the horizon, warmer and darker as the sun
/// (`sun`, unit, towards it) sets, a glow around the sun, and a dim ground. Mirrored line for
/// line in `gpu.wgsl` (`sky`).
pub(crate) fn sky(d: V3, sun: V3, sun_color: [f32; 3], strength: f32) -> [f32; 3] {
    let e = sun.1;
    let day = smoothstep(-0.1, 0.35, e);
    let zenith = mix3([0.05, 0.05, 0.12], [0.12, 0.26, 0.62], day);
    let horizon = mix3([0.85, 0.45, 0.25], [0.68, 0.78, 0.92], smoothstep(-0.05, 0.4, e));
    let light = 0.08 + 0.92 * smoothstep(-0.12, 0.2, e);
    let y = d.1;
    let mut c = if y >= 0.0 {
        mix3(horizon, zenith, y.min(1.0).powf(0.45))
    } else {
        let ground = [0.16 * light, 0.15 * light, 0.14 * light];
        mix3([horizon[0] * 0.5, horizon[1] * 0.5, horizon[2] * 0.5], ground, (-y).min(1.0).sqrt())
    };
    // The glow around the sun (strongest low in the sky).
    let cs = d.dot(sun).max(0.0);
    let haze = 0.35 * cs.powi(8) + 1.5 * cs.powi(64);
    let above = smoothstep(-0.15, 0.05, y);
    for k in 0..3 {
        c[k] = (c[k] * light + sun_color[k] * haze * above) * strength;
    }
    c
}

/// Bilinear in u (wrapping) and v (clamped), like the GPU's sampler.
fn sample_level(l: &EnvLevel, u: f32, v: f32) -> [f32; 3] {
    let (w, h) = (l.width as f32, l.height as f32);
    let (x, y) = (u.rem_euclid(1.0) * w - 0.5, v.clamp(0.0, 1.0) * h - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f32, yi: f32| -> [f32; 4] {
        let xi = (xi as i64).rem_euclid(l.width as i64) as usize;
        let yi = (yi as i64).clamp(0, l.height as i64 - 1) as usize;
        l.rgba[yi * l.width + xi]
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    std::array::from_fn(|k| (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy)
}

/// A panorama picture, bilinear, u wrapping and v clamped, as linear RGB.
pub(crate) fn sample_srgb_clamped(t: &Texture, u: f32, v: f32) -> [f32; 3] {
    let (w, h) = (t.width.max(1) as f32, t.height.max(1) as f32);
    let (x, y) = (u.rem_euclid(1.0) * w - 0.5, v.clamp(0.0, 1.0) * h - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f32, yi: f32| -> [f32; 3] {
        let xi = (xi as i64).rem_euclid(t.width.max(1) as i64) as usize;
        let yi = (yi as i64).clamp(0, t.height.max(1) as i64 - 1) as usize;
        let i = (yi * t.width as usize + xi) * 4;
        match t.rgba.get(i..i + 3) {
            Some(p) => [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2])],
            None => [0.0; 3],
        }
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    std::array::from_fn(|k| (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy)
}

/// Half the size, each texel the average of four.
fn downsample(l: &EnvLevel) -> EnvLevel {
    let (w, h) = ((l.width / 2).max(1), (l.height / 2).max(1));
    let mut rgba = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (x * 2 + dx).min(l.width - 1);
                let sy = (y * 2 + dy).min(l.height - 1);
                let c = l.rgba[sy * l.width + sx];
                for k in 0..4 {
                    acc[k] += c[k] * 0.25;
                }
            }
            rgba.push(acc);
        }
    }
    EnvLevel { width: w, height: h, rgba }
}

/// A Gaussian blur of `sigma` radians over the sphere: wider in u near the poles, where the
/// panorama is stretched; u wraps around.
fn blur(l: &EnvLevel, sigma: f32) -> EnvLevel {
    let (w, h) = (l.width, l.height);
    if sigma <= 1e-4 {
        return l.clone();
    }
    let texel = std::f32::consts::PI / h as f32;
    // Horizontal, per row.
    let mut tmp = vec![[0.0f32; 4]; w * h];
    for y in 0..h {
        let theta = (y as f32 + 0.5) / h as f32 * std::f32::consts::PI;
        let s = (sigma / texel / theta.sin().max(0.05)).min(w as f32 / 2.0);
        let r = (s * 3.0).ceil() as i64;
        let weights: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * s * s).max(1e-6)).exp()).collect();
        let total: f32 = weights.iter().sum();
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (i, wt) in (-r..=r).zip(&weights) {
                let sx = (x as i64 + i).rem_euclid(w as i64) as usize;
                let c = l.rgba[y * w + sx];
                for k in 0..4 {
                    acc[k] += c[k] * wt;
                }
            }
            tmp[y * w + x] = acc.map(|v| v / total);
        }
    }
    // Vertical, clamped at the poles.
    let s = sigma / texel;
    let r = (s * 3.0).ceil() as i64;
    let weights: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * s * s).max(1e-6)).exp()).collect();
    let total: f32 = weights.iter().sum();
    let mut rgba = vec![[0.0f32; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (i, wt) in (-r..=r).zip(&weights) {
                let sy = (y as i64 + i).clamp(0, h as i64 - 1) as usize;
                let c = tmp[sy * w + x];
                for k in 0..4 {
                    acc[k] += c[k] * wt;
                }
            }
            rgba[y * w + x] = acc.map(|v| v / total);
        }
    }
    EnvLevel { width: w, height: h, rgba }
}

/// The nine real spherical-harmonic basis functions at `n`.
pub(crate) fn sh_basis(n: V3) -> [f32; 9] {
    let V3(x, y, z) = n;
    [0.282_095, 0.488_603 * y, 0.488_603 * z, 0.488_603 * x, 1.092_548 * x * y, 1.092_548 * y * z, 0.315_392 * (3.0 * z * z - 1.0), 1.092_548 * x * z, 0.546_274 * (x * x - y * y)]
}

/// Irradiance coefficients (cosine-convolved, divided by π) of a level.
fn project_sh(l: &EnvLevel) -> [[f32; 3]; 9] {
    let mut c = [[0.0f32; 3]; 9];
    let (w, h) = (l.width, l.height);
    let d_phi = std::f32::consts::TAU / w as f32;
    let d_theta = std::f32::consts::PI / h as f32;
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        let d_omega = d_phi * d_theta * (v * std::f32::consts::PI).sin();
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let b = sh_basis(uv_to_dir(u, v));
            let px = l.rgba[y * w + x];
            for (ci, bk) in c.iter_mut().zip(b) {
                for k in 0..3 {
                    ci[k] += px[k] * bk * d_omega;
                }
            }
        }
    }
    // Cosine lobe per band (π, 2π/3, π/4), then / π for outgoing radiance of a white surface.
    let band = [1.0, 2.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 0.25, 0.25, 0.25, 0.25, 0.25];
    for (ci, k) in c.iter_mut().zip(band) {
        for v in ci.iter_mut() {
            *v *= k;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(kind: EnvKind) -> Env {
        Env {
            kind,
            color: [0.5, 0.25, 0.125],
            top: [0.2, 0.4, 0.8],
            horizon: [0.8, 0.8, 0.8],
            bottom: [0.1, 0.08, 0.05],
            image: None,
            strength: 1.0,
            rotation: 0.0,
            visible: false,
            sun: V3(0.3, 0.8, 0.5).norm(),
            sun_color: [1.0, 0.95, 0.9],
            maps: None,
        }
    }

    #[test]
    fn directions_round_trip_through_the_panorama() {
        for d in [V3(0.0, 0.0, -1.0), V3(1.0, 0.2, 0.3).norm(), V3(-0.4, -0.7, 0.2).norm()] {
            let (u, v) = dir_to_uv(d);
            let back = uv_to_dir(u, v);
            assert!((back - d).len() < 1e-4, "{d:?} → {back:?}");
        }
        assert!((dir_to_uv(V3(0.0, 0.0, -1.0)).0 - 0.5).abs() < 1e-6, "−z in the middle");
        // A quarter turn brings +x where −z was.
        let r = rotate_y(V3(1.0, 0.0, 0.0), std::f32::consts::FRAC_PI_2);
        assert!((r - V3(0.0, 0.0, -1.0)).len() < 1e-5, "{r:?}");
    }

    #[test]
    fn a_uniform_world_lights_matte_surfaces_evenly() {
        let e = env(EnvKind::Color);
        let m = e.build_maps();
        for n in [V3(0.0, 1.0, 0.0), V3(1.0, 0.0, 0.0), V3(0.0, -1.0, 0.0), V3(0.3, -0.2, 0.9).norm()] {
            let c = m.irradiance(n, 0.0);
            assert!((c[0] - 0.5).abs() < 0.02 && (c[1] - 0.25).abs() < 0.02 && (c[2] - 0.125).abs() < 0.02, "{n:?}: {c:?}");
        }
        let r = m.reflect(V3(0.0, 0.0, 1.0), 0.0, 0.7);
        assert!((r[0] - 0.5).abs() < 1e-3, "{r:?}");
    }

    #[test]
    fn gradients_and_skies_are_brighter_above() {
        let g = env(EnvKind::Gradient);
        let m = g.build_maps();
        let up = m.irradiance(V3(0.0, 1.0, 0.0), 0.0);
        let down = m.irradiance(V3(0.0, -1.0, 0.0), 0.0);
        assert!(up[2] > down[2] * 2.0, "{up:?} vs {down:?}");
        assert!((g.radiance(V3(0.0, 1.0, 0.0))[2] - 0.8).abs() < 1e-5);
        // The sky is blue overhead, brightest towards the sun.
        let s = env(EnvKind::Sky);
        let zenith = s.radiance(V3(0.0, 1.0, 0.0));
        assert!(zenith[2] > zenith[0], "blue overhead: {zenith:?}");
        let toward = s.radiance(V3(0.3, 0.8, 0.5).norm());
        let away = s.radiance(V3(-0.3, 0.8, -0.5).norm());
        assert!(toward[0] > away[0] * 1.5, "glow around the sun: {toward:?} vs {away:?}");
        // Mirror-like reflections keep detail that rough ones blur away.
        let sm = s.build_maps();
        let sharp = sm.reflect(V3(0.3, 0.8, 0.5).norm(), 0.0, 0.0);
        let rough = sm.reflect(V3(0.3, 0.8, 0.5).norm(), 0.0, 1.0);
        assert!(sharp[0] > rough[0], "{sharp:?} vs {rough:?}");
    }

    #[test]
    fn panoramas_are_read_and_turned() {
        // Left half red, right half blue.
        let (w, h) = (64u32, 32u32);
        let mut rgba = vec![];
        for _y in 0..h {
            for x in 0..w {
                rgba.extend(if x < w / 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
            }
        }
        let mut e = env(EnvKind::Image);
        e.image = Some(Arc::new(Texture { width: w, height: h, rgba }));
        // u < 0.5 is towards −x.
        let left = e.radiance(V3(-1.0, 0.0, 0.0));
        let right = e.radiance(V3(1.0, 0.0, 0.0));
        assert!(left[0] > 0.9 && right[2] > 0.9, "{left:?} {right:?}");
        e.rotation = std::f32::consts::PI;
        let turned = e.radiance(V3(-1.0, 0.0, 0.0));
        assert!(turned[2] > 0.9, "half a turn swaps the sides: {turned:?}");
        let m = e.build_maps();
        let l = m.irradiance(V3(-1.0, 0.0, 0.0), e.rotation);
        assert!(l[2] > l[0], "irradiance turns too: {l:?}");
    }
}
