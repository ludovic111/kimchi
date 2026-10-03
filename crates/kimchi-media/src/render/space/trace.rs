//! The path tracer (the `"path"` render engine, like Blender's Cycles): final frames of 3D
//! scenes with real reflections, refraction, soft shadows and bounced light, on the CPU.
//!
//! A frame's items become world-space triangles in a bounding volume hierarchy (kept from one
//! frame to the next while nothing moves). Every pixel sends `samples` camera rays (jittered
//! with a Blackman-Harris filter, through a thin lens for depth of field); each path bounces up
//! to `bounces` times off a principled-like surface (diffuse, GGX specular, rough glass, a clear
//! coat), gathering light at every hit from each lamp (a shadow ray) and from the environment
//! (importance sampled for panoramas, weighted against the surface's own sampling), with
//! Russian roulette after three bounces and bright indirect samples clamped. Rows are shared out
//! to every core; each pixel's random numbers depend only on the pixel, the sample and the seed,
//! so the same scene always gives the same picture.
//!
//! Brightness matches the standard engine (`shade` in `mod.rs`): a lamp's colour is what a
//! white surface facing it reflects, the ambient sky/ground terms become an environment that
//! lights a surface the same way, and pixels go through the same exposure, highlight roll-off
//! and sRGB encoding. [`Progressive`] renders a few samples at a time, for a view that refines.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use kimchi_core::motion::{RenderSettings, Scene3d};
use rayon::prelude::*;
use tiny_skia::Pixmap;

use super::bvh::{Bvh, Hit, Tri};
use super::denoise;
use super::math::V3;
use super::mesh::Mesh;
use super::{CameraRes, Env, EnvKind, Frame3d, Item, LightKind, LightRes, Mat, Pictures, Quality, Space, Texture, linear_to_srgb, shoulder};

const PI: f32 = std::f32::consts::PI;
/// Indirect light brighter than this (luminance) is scaled down: rare bright paths would
/// otherwise leave white specks ("fireflies").
const CLAMP: f32 = 10.0;
/// Rows of pixels per piece of work handed to a thread.
const BAND: usize = 4;
/// See-through surfaces (opacity) a ray may pass before it gives up.
const MAX_PASSES: u32 = 64;
/// Up to this many lamps are all sampled at every hit; with more, one is picked by power.
const ALL_LIGHTS: usize = 8;
/// Clear-coat roughness.
const COAT_ROUGHNESS: f32 = 0.05;

/// How the path tracer renders (from the scene's render settings).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Rays per pixel.
    pub samples: u32,
    /// Bounces after the first hit (0 = direct light only).
    pub bounces: u32,
    /// Smooth the remaining grain away.
    pub denoise: bool,
    /// Another seed gives other (equally good) grain.
    pub seed: u32,
    pub filter: Filter,
}

/// How samples spread over and around their pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Evenly within the pixel.
    Box,
    /// A soft bell 1.5 pixels wide (smoother edges, Cycles' default).
    #[default]
    BlackmanHarris,
}

impl Settings {
    pub fn of(r: &RenderSettings) -> Settings {
        let n = |v: f64, lo: f64, hi: f64, def: u32| if v.is_finite() { v.round().clamp(lo, hi) as u32 } else { def };
        Settings { samples: n(r.samples, 1.0, 65_536.0, 64), bounces: n(r.bounces, 0.0, 64.0, 4), denoise: r.denoise, seed: 0, filter: Filter::default() }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings::of(&RenderSettings::default())
    }
}

/// One frame path traced with `settings`, premultiplied (what the standard engine returns).
pub(crate) fn render(frame: &Frame3d, settings: &Settings) -> Pixmap {
    let t0 = Instant::now();
    let mut p = Progressive::new(frame, *settings);
    p.add(settings.samples);
    let picture = p.picture();
    let (rays, secs) = p.stats();
    tracing::info!(
        "path traced {}×{} at {} samples per pixel in {:.2} s ({:.1} M rays/s{})",
        frame.width,
        frame.height,
        settings.samples,
        t0.elapsed().as_secs_f64(),
        rays as f64 / secs.max(1e-9) / 1e6,
        if settings.denoise { ", denoised" } else { "" }
    );
    picture
}

impl Space {
    /// A path-traced picture of `scene` at `t` that refines a few samples at a time (the
    /// Studio's "Rendered" view): call [`Progressive::add`] while the view is still and show
    /// [`Progressive::picture`].
    pub(crate) fn progressive(&mut self, scene: &Scene3d, t: f64, width: u32, height: u32, pics: &mut dyn Pictures) -> Progressive {
        let frame = self.frame(scene, t, width, height, pics, Quality::Final);
        Progressive::new(&frame, Settings::of(&scene.render))
    }
}

// ---------------------------------------------------------------------------------------------
// Colours

type Rgb = [f32; 3];

fn lum(c: Rgb) -> f32 {
    c[0] * 0.2126 + c[1] * 0.7152 + c[2] * 0.0722
}
fn mul(a: Rgb, b: Rgb) -> Rgb {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
fn scale(a: Rgb, k: f32) -> Rgb {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn add(a: Rgb, b: Rgb) -> Rgb {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn finite(c: Rgb) -> bool {
    c.iter().all(|v| v.is_finite())
}

/// Linear light after exposure → display (0–1, still linear): the standard engine's highlight
/// roll-off, or a filmic curve that keeps more range in the highlights.
pub(crate) fn tone_map(v: f32, filmic: bool) -> f32 {
    if !filmic {
        return shoulder(v);
    }
    // Narkowicz's fit of the ACES film curve.
    let x = v.max(0.0) * 0.8;
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------------------------
// Random numbers

/// SplitMix64: small, fast, and good enough for sampling.
struct Rng(u64);

impl Rng {
    fn new(pixel: u64, sample: u32, seed: u32) -> Rng {
        let mut r = Rng(pixel.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ((sample as u64) << 32 | seed as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
        r.next();
        r
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
    }
}

// ---------------------------------------------------------------------------------------------
// Geometry

/// The frame's triangles in world space, and what shading needs to find their corners.
struct Geo {
    bvh: Bvh,
    /// Per triangle: its item, and where its corners are in the item mesh's index list.
    owner: Vec<(u32, u32)>,
    /// Per item: vertex normals in world space (unit).
    normals: Vec<Vec<V3>>,
}

type GeoKey = Vec<(Arc<Mesh>, [f32; 16])>;

/// The geometry of `items`, reused when the previous frame had the same meshes in the same
/// places (the hierarchy is the slow part of a frame of a still scene).
fn geometry(items: &[Item]) -> Arc<Geo> {
    type Cache = Mutex<Option<(GeoKey, Arc<Geo>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key: GeoKey = items.iter().map(|it| (it.mesh.clone(), it.model.flat())).collect();
    let same = |a: &GeoKey| a.len() == key.len() && a.iter().zip(&key).all(|(x, y)| Arc::ptr_eq(&x.0, &y.0) && x.1 == y.1);
    if let Some((k, g)) = &*cache.lock().unwrap_or_else(|e| e.into_inner())
        && same(k)
    {
        return g.clone();
    }
    let t0 = Instant::now();
    let geo = Arc::new(build_geo(items));
    tracing::debug!("path tracer: {} triangles, hierarchy built in {:.0} ms", geo.bvh.len(), t0.elapsed().as_secs_f64() * 1000.0);
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, geo.clone()));
    geo
}

fn build_geo(items: &[Item]) -> Geo {
    /// An item's triangles, their owners and its world normals.
    type Part = (Vec<Tri>, Vec<(u32, u32)>, Vec<V3>);
    let per_item: Vec<Part> = items
        .par_iter()
        .enumerate()
        .map(|(i, it)| {
            let pos: Vec<V3> = it.mesh.pos.par_iter().map(|p| it.model.point3(V3(p[0], p[1], p[2]))).collect();
            let normals: Vec<V3> = it.mesh.normal.par_iter().map(|n| it.normal.dir(V3(n[0], n[1], n[2])).norm()).collect();
            let mut tris = Vec::with_capacity(it.mesh.index.len() / 3);
            let mut owner = Vec::with_capacity(it.mesh.index.len() / 3);
            for (k, t) in it.mesh.index.as_chunks::<3>().0.iter().enumerate() {
                let get = |j: u32| pos.get(j as usize).copied();
                // Indices past the vertices (a broken model) are skipped.
                if let (Some(a), Some(b), Some(c)) = (get(t[0]), get(t[1]), get(t[2])) {
                    tris.push(Tri::new(a, b, c));
                    owner.push((i as u32, (k * 3) as u32));
                }
            }
            (tris, owner, normals)
        })
        .collect();
    let mut tris = Vec::with_capacity(per_item.iter().map(|p| p.0.len()).sum());
    let mut owner = Vec::with_capacity(tris.capacity());
    let mut normals = Vec::with_capacity(items.len());
    for (t, o, n) in per_item {
        tris.extend(t);
        owner.extend(o);
        normals.push(n);
    }
    Geo { bvh: Bvh::build(tris), owner, normals }
}

/// Where a ray hit, ready to shade.
struct Surf {
    p: V3,
    /// Geometric normal (unit, the triangle's winding) and the interpolated, bumped one.
    ng: V3,
    ns: V3,
    /// Base colour and alpha with the texture applied.
    base: [f32; 4],
    item: usize,
}

/// A frame item's material and mesh, and whether light may pass through it.
struct ItemInfo {
    mat: Mat,
    mesh: Arc<Mesh>,
    /// Partly see-through somewhere (opacity below 1 or a texture with transparent pixels).
    clear: bool,
}

// ---------------------------------------------------------------------------------------------
// The world: environment and its importance map

/// An equirectangular picture's brightness as a 2D distribution, for aiming rays where the
/// light comes from.
struct EnvMap {
    w: usize,
    h: usize,
    /// Cumulative over rows (h + 1), then within each row (h × (w + 1)), both normalised.
    rows: Vec<f32>,
    cols: Vec<f32>,
    /// Each cell's density over the unit square of picture coordinates.
    pdf: Vec<f32>,
}

impl EnvMap {
    fn new(tex: &Texture, strength: f32) -> Option<EnvMap> {
        let w = (tex.width as usize).clamp(1, 512);
        let h = (tex.height as usize).clamp(1, 256);
        // Brightness per cell, weighted by how much of the sphere the row covers.
        let mut f: Vec<f32> = vec![0.0; w * h];
        f.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let v = (y as f32 + 0.5) / h as f32;
            let sin = (v * PI).sin();
            for (x, c) in row.iter_mut().enumerate() {
                let s = tex.sample((x as f32 + 0.5) / w as f32, v);
                let l = lum([s[0], s[1], s[2]]) * strength;
                *c = if l.is_finite() { l.max(0.0) * sin } else { 0.0 };
            }
        });
        let mut cols = vec![0.0f32; h * (w + 1)];
        let mut row_sum = vec![0.0f32; h];
        for y in 0..h {
            let c = &mut cols[y * (w + 1)..(y + 1) * (w + 1)];
            for x in 0..w {
                c[x + 1] = c[x] + f[y * w + x];
            }
            row_sum[y] = c[w];
            if c[w] > 0.0 {
                let k = 1.0 / c[w];
                c.iter_mut().for_each(|v| *v *= k);
            } else {
                c.iter_mut().enumerate().for_each(|(x, v)| *v = x as f32 / w as f32);
            }
        }
        let total: f32 = row_sum.iter().sum();
        if total.is_nan() || total <= 0.0 || !total.is_finite() {
            return None;
        }
        let mut rows = vec![0.0f32; h + 1];
        for y in 0..h {
            rows[y + 1] = rows[y] + row_sum[y] / total;
        }
        // Mean over the unit square is total / (w h); density = value / mean.
        let mean = total / (w * h) as f32;
        let pdf = f.iter().map(|v| v / mean).collect();
        Some(EnvMap { w, h, rows, cols, pdf })
    }

    /// A point of the picture (u, v in 0–1) and its density over the unit square.
    fn sample(&self, u1: f32, u2: f32) -> (f32, f32, f32) {
        let y = find(&self.rows, u1).min(self.h - 1);
        let row = &self.cols[y * (self.w + 1)..(y + 1) * (self.w + 1)];
        let x = find(row, u2).min(self.w - 1);
        let within = |cdf: &[f32], i: usize, u: f32| {
            let d = cdf[i + 1] - cdf[i];
            if d > 0.0 { ((u - cdf[i]) / d).clamp(0.0, 1.0) } else { 0.5 }
        };
        let (fx, fy) = (within(row, x, u2), within(&self.rows, y, u1));
        ((x as f32 + fx) / self.w as f32, (y as f32 + fy) / self.h as f32, self.pdf[y * self.w + x])
    }

    fn pdf_at(&self, u: f32, v: f32) -> f32 {
        let x = ((u.rem_euclid(1.0) * self.w as f32) as usize).min(self.w - 1);
        let y = ((v.clamp(0.0, 1.0) * self.h as f32) as usize).min(self.h - 1);
        self.pdf[y * self.w + x]
    }
}

/// The last index `i` with `cdf[i] <= u`.
fn find(cdf: &[f32], u: f32) -> usize {
    cdf.partition_point(|&c| c <= u).saturating_sub(1)
}

/// Equirectangular coordinates of a direction (in the world's own frame): u around, v from the
/// top (0) to the bottom (1).
fn dir_to_uv(d: V3) -> (f32, f32) {
    (0.5 + d.0.atan2(-d.2) / (2.0 * PI), d.1.clamp(-1.0, 1.0).acos() / PI)
}

fn uv_to_dir(u: f32, v: f32) -> V3 {
    let (phi, theta) = ((u - 0.5) * 2.0 * PI, v * PI);
    V3(theta.sin() * phi.sin(), theta.cos(), -theta.sin() * phi.cos())
}

/// Turns a direction around the vertical axis.
fn rot_y(d: V3, a: f32) -> V3 {
    let (s, c) = a.sin_cos();
    V3(c * d.0 + s * d.2, d.1, -s * d.0 + c * d.2)
}

// ---------------------------------------------------------------------------------------------
// The camera

struct Cam {
    eye: V3,
    fwd: V3,
    right: V3,
    up: V3,
    ortho: bool,
    tan_half: f32,
    half_h: f32,
    aspect: f32,
    focus: f32,
    aperture: f32,
}

impl Cam {
    fn new(c: &CameraRes, width: u32, height: u32) -> Cam {
        let fwd = c.forward.norm();
        // Screen right and up as the rasteriser's look-at has them (roll included in `up`).
        let mut right = fwd.cross(c.up);
        if right.len() < 1e-6 {
            right = c.right;
        }
        let right = right.norm();
        let up = right.cross(fwd).norm();
        let fov = if c.fov_y.is_finite() { c.fov_y.clamp(0.01, 3.1) } else { 0.7 };
        Cam {
            eye: c.eye,
            fwd,
            right,
            up,
            ortho: c.ortho,
            tan_half: (fov / 2.0).tan(),
            half_h: if c.ortho_size.is_finite() && c.ortho_size > 0.0 { c.ortho_size / 2.0 } else { 2.5 },
            aspect: width as f32 / height.max(1) as f32,
            focus: if c.focus.is_finite() { c.focus.max(0.0) } else { 0.0 },
            aperture: if c.aperture.is_finite() { c.aperture.max(0.0) } else { 0.0 },
        }
    }

    /// The ray through screen point (`sx`, `sy`) in −1..1 (y up), through lens point `lens`
    /// (two numbers in 0..1).
    fn ray(&self, sx: f32, sy: f32, lens: (f32, f32)) -> (V3, V3) {
        let (mut o, d) = if self.ortho {
            (self.eye + self.right * (sx * self.half_h * self.aspect) + self.up * (sy * self.half_h), self.fwd)
        } else {
            (self.eye, (self.fwd + self.right * (sx * self.tan_half * self.aspect) + self.up * (sy * self.tan_half)).norm())
        };
        if self.aperture > 0.0 && self.focus > 0.0 {
            // Thin lens: every ray through the point in focus, from a point on the aperture.
            let focal = o + d * (self.focus / d.dot(self.fwd).max(1e-4));
            let (r, a) = (lens.0.sqrt() * self.aperture, lens.1 * 2.0 * PI);
            o = o + self.right * (r * a.cos()) + self.up * (r * a.sin());
            return (o, (focal - o).norm());
        }
        (o, d)
    }
}

/// Offsets inside a pixel for a Blackman-Harris filter 1.5 pixels wide, by inverting its
/// cumulative distribution (the filter is separable: x and y each draw from it).
fn blackman_harris(u: f32) -> f32 {
    const N: usize = 256;
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    let cdf = TABLE.get_or_init(|| {
        let mut c = vec![0.0f32; N + 1];
        for i in 0..N {
            let x = (i as f32 + 0.5) / N as f32;
            let w = 0.35875 - 0.48829 * (2.0 * PI * x).cos() + 0.14128 * (4.0 * PI * x).cos() - 0.01168 * (6.0 * PI * x).cos();
            c[i + 1] = c[i] + w.max(0.0);
        }
        let total = c[N];
        c.iter_mut().for_each(|v| *v /= total);
        c
    });
    let i = find(cdf, u).min(N - 1);
    let d = cdf[i + 1] - cdf[i];
    let x = (i as f32 + if d > 0.0 { (u - cdf[i]) / d } else { 0.5 }) / N as f32;
    (x - 0.5) * 1.5
}

// ---------------------------------------------------------------------------------------------
// Surfaces: a principled-like BSDF

/// An orthonormal frame around a normal (`z`).
struct Frame {
    x: V3,
    y: V3,
    z: V3,
}

impl Frame {
    fn new(n: V3) -> Frame {
        // Duff et al., "Building an Orthonormal Basis, Revisited".
        let s = if n.2 >= 0.0 { 1.0 } else { -1.0 };
        let a = -1.0 / (s + n.2);
        let b = n.0 * n.1 * a;
        Frame { x: V3(1.0 + s * n.0 * n.0 * a, s * b, -s * n.0), y: V3(b, s + n.1 * n.1 * a, -n.1), z: n }
    }
    fn local(&self, v: V3) -> V3 {
        V3(v.dot(self.x), v.dot(self.y), v.dot(self.z))
    }
    fn world(&self, v: V3) -> V3 {
        self.x * v.0 + self.y * v.1 + self.z * v.2
    }
}

fn reflect(wo: V3, h: V3) -> V3 {
    h * (2.0 * wo.dot(h)) - wo
}

/// Refracts `wo` (pointing away, on `h`'s side) through a surface with relative index `eta`
/// (inside over outside); none at total internal reflection.
fn refract(wo: V3, h: V3, eta: f32) -> Option<V3> {
    let c = wo.dot(h);
    let sin2_t = (1.0 - c * c).max(0.0) / (eta * eta);
    if sin2_t >= 1.0 {
        return None;
    }
    let cos_t = (1.0 - sin2_t).sqrt();
    Some((-wo * (1.0 / eta) + h * (c / eta - cos_t)).norm())
}

/// Schlick's Fresnel with the standard engine's roughness limit: rough surfaces don't turn
/// into mirrors at grazing angles (keeps dark matte floors from reflecting the whole sky).
fn schlick_rough(f0: f32, c: f32, rough: f32) -> f32 {
    f0 + ((1.0 - rough).max(f0) - f0) * (1.0 - c.clamp(0.0, 1.0)).powi(5)
}

fn schlick(f0: f32, c: f32) -> f32 {
    f0 + (1.0 - f0) * (1.0 - c.clamp(0.0, 1.0)).powi(5)
}

/// Unpolarised Fresnel reflectance of a dielectric; `eta` = transmitted over incident index.
fn fresnel(cos_i: f32, eta: f32) -> f32 {
    let c = cos_i.clamp(0.0, 1.0);
    let sin2_t = (1.0 - c * c) / (eta * eta);
    if sin2_t >= 1.0 {
        return 1.0;
    }
    let ct = (1.0 - sin2_t).sqrt();
    let rs = (c - eta * ct) / (c + eta * ct);
    let rp = (eta * c - ct) / (eta * c + ct);
    (0.5 * (rs * rs + rp * rp)).clamp(0.0, 1.0)
}

/// GGX (Trowbridge-Reitz) microfacets with roughness `a` (α).
fn ggx_d(h: V3, a: f32) -> f32 {
    if h.2 <= 0.0 {
        return 0.0;
    }
    let a2 = a * a;
    let c2 = h.2 * h.2;
    let k = c2 * (a2 - 1.0) + 1.0;
    a2 / (PI * k * k)
}

fn ggx_lambda(w: V3, a: f32) -> f32 {
    let c2 = w.2 * w.2;
    if c2 <= 0.0 {
        return f32::INFINITY;
    }
    let tan2 = ((1.0 - c2) / c2).max(0.0);
    0.5 * (-1.0 + (1.0 + a * a * tan2).sqrt())
}

fn ggx_g1(w: V3, a: f32) -> f32 {
    1.0 / (1.0 + ggx_lambda(w, a))
}

fn ggx_g2(wo: V3, wi: V3, a: f32) -> f32 {
    1.0 / (1.0 + ggx_lambda(wo, a) + ggx_lambda(wi, a))
}

/// Density of normal `h` among those `wo` sees (Heitz 2018).
fn vndf_pdf(wo: V3, h: V3, a: f32) -> f32 {
    if wo.2 <= 0.0 {
        return 0.0;
    }
    ggx_g1(wo, a) * wo.dot(h).max(0.0) * ggx_d(h, a) / wo.2
}

/// A microfacet normal seen from `wo`, drawn in proportion to how much of it `wo` sees.
fn vndf_sample(wo: V3, a: f32, u1: f32, u2: f32) -> V3 {
    let vh = V3(a * wo.0, a * wo.1, wo.2).norm();
    let l2 = vh.0 * vh.0 + vh.1 * vh.1;
    let t1 = if l2 > 0.0 { V3(-vh.1, vh.0, 0.0) * (1.0 / l2.sqrt()) } else { V3(1.0, 0.0, 0.0) };
    let t2 = vh.cross(t1);
    let r = u1.sqrt();
    let phi = 2.0 * PI * u2;
    let p1 = r * phi.cos();
    let s = 0.5 * (1.0 + vh.2);
    let p2 = (1.0 - s) * (1.0 - p1 * p1).max(0.0).sqrt() + s * r * phi.sin();
    let nh = t1 * p1 + t2 * p2 + vh * (1.0 - p1 * p1 - p2 * p2).max(0.0).sqrt();
    V3(a * nh.0, a * nh.1, nh.2.max(1e-6)).norm()
}

fn cosine_sample(u1: f32, u2: f32) -> V3 {
    let r = u1.sqrt();
    let phi = 2.0 * PI * u2;
    V3(r * phi.cos(), r * phi.sin(), (1.0 - u1).max(0.0).sqrt())
}

/// The surface's response, in its local frame with the normal towards the viewer (`wo.z > 0`).
struct Bsdf {
    /// Diffuse albedo (already weighted by what isn't metal or glass).
    diffuse: Rgb,
    /// Opaque specular: reflectance at normal incidence and the lobe's weight.
    f0: Rgb,
    spec: f32,
    /// Microfacet roughness (α) of the specular and glass lobes, and the roughness it came from.
    a: f32,
    rough: f32,
    /// Glass: weight, tint of what passes, index ratio across the surface from the viewer's side.
    glass: f32,
    tint: Rgb,
    eta: f32,
    /// Clear coat weight.
    coat: f32,
    /// Chances of sampling the diffuse, specular, glass and coat lobes.
    p: [f32; 4],
}

struct BsdfSample {
    wi: V3,
    /// f · cos / pdf.
    weight: Rgb,
    pdf: f32,
    /// Went through the surface.
    through: bool,
}

impl Bsdf {
    /// `entering`: the viewer is outside the surface (for glass).
    fn new(m: &Mat, base: Rgb, entering: bool, wo: V3) -> Bsdf {
        let metallic = m.metallic.clamp(0.0, 1.0);
        let trans = m.transmission.clamp(0.0, 1.0);
        let rough = m.roughness.clamp(0.0, 1.0);
        let a = (rough * rough).max(1e-3);
        let ior = if m.ior.is_finite() { m.ior.clamp(1.0, 3.0) } else { 1.45 };
        let ior = if (ior - 1.0).abs() < 1e-3 { 1.001 } else { ior };
        let coat = m.clearcoat.clamp(0.0, 1.0);
        let under = 1.0 - coat * schlick(0.04, wo.2);
        let dielectric = 1.0 - metallic;
        let diffuse = scale(base, dielectric * (1.0 - trans) * (1.0 - schlick_rough(0.04, wo.2, rough)) * under);
        let f0 = lerp([0.04; 3], base, metallic);
        let spec = (1.0 - dielectric * trans) * under;
        let glass = dielectric * trans * under;
        let eta = if entering { ior } else { 1.0 / ior };
        let fs = lum([schlick_rough(f0[0], wo.2, rough), schlick_rough(f0[1], wo.2, rough), schlick_rough(f0[2], wo.2, rough)]);
        let mut p = [lum(diffuse).max(0.0), if spec > 0.0 { (spec * fs).max(0.02) } else { 0.0 }, glass, coat * schlick(0.04, wo.2)];
        let total: f32 = p.iter().sum();
        if total > 0.0 && total.is_finite() {
            p.iter_mut().for_each(|v| *v /= total);
        } else {
            p = [1.0, 0.0, 0.0, 0.0];
        }
        Bsdf { diffuse, f0, spec, a, rough, glass, tint: base, eta, coat, p }
    }

    /// Smooth enough that its reflections or refractions are pictures of other things (the
    /// denoiser looks past it).
    fn mirror_like(&self) -> bool {
        self.a < 0.03 && self.p[0] < 0.2
    }

    /// The colour the denoiser divides by.
    fn albedo(&self) -> Rgb {
        add(add(self.diffuse, scale(self.f0, self.spec)), scale(self.tint, self.glass))
    }

    /// f · |cos θi| and the density of sampling `wi`.
    fn eval(&self, wo: V3, wi: V3) -> (Rgb, f32) {
        let (co, ci) = (wo.2, wi.2);
        let mut f = [0.0f32; 3];
        let mut pdf = 0.0;
        if co <= 0.0 {
            return (f, 0.0);
        }
        if ci > 0.0 {
            if self.p[0] > 0.0 {
                f = add(f, scale(self.diffuse, ci / PI));
                pdf += self.p[0] * ci / PI;
            }
            let h = (wo + wi).norm();
            let woh = wo.dot(h);
            if woh <= 0.0 {
                return (f, pdf);
            }
            if self.spec > 0.0 || self.glass > 0.0 {
                let d = ggx_d(h, self.a);
                let g = ggx_g2(wo, wi, self.a);
                let refl_pdf = vndf_pdf(wo, h, self.a) / (4.0 * woh);
                if self.spec > 0.0 {
                    let fr = [schlick_rough(self.f0[0], woh, self.rough), schlick_rough(self.f0[1], woh, self.rough), schlick_rough(self.f0[2], woh, self.rough)];
                    f = add(f, scale(fr, self.spec * d * g / (4.0 * co)));
                    pdf += self.p[1] * refl_pdf;
                }
                if self.glass > 0.0 {
                    let fr = fresnel(woh, self.eta);
                    f = add(f, [self.glass * fr * d * g / (4.0 * co); 3]);
                    pdf += self.p[2] * fr * refl_pdf;
                }
            }
            if self.coat > 0.0 {
                let ac = COAT_ROUGHNESS * COAT_ROUGHNESS;
                let fc = self.coat * schlick(0.04, woh);
                f = add(f, [fc * ggx_d(h, ac) * ggx_g2(wo, wi, ac) / (4.0 * co); 3]);
                pdf += self.p[3] * vndf_pdf(wo, h, ac) / (4.0 * woh);
            }
        } else if ci < 0.0 && self.glass > 0.0 {
            // Through the surface (Walter et al. 2007), in radiance units.
            let mut h = (wo + wi * self.eta).norm();
            if h.2 < 0.0 {
                h = -h;
            }
            let (woh, wih) = (wo.dot(h), wi.dot(h));
            if woh <= 0.0 || wih >= 0.0 {
                return (f, pdf);
            }
            let fr = fresnel(woh, self.eta);
            let denom = (woh + self.eta * wih).powi(2);
            if denom <= 1e-12 {
                return (f, pdf);
            }
            let d = ggx_d(h, self.a);
            let ft = (1.0 - fr) * d * ggx_g2(wo, wi, self.a) * woh * wih.abs() / (co * denom);
            f = add(f, scale(self.tint, self.glass * ft));
            pdf += self.p[2] * (1.0 - fr) * vndf_pdf(wo, h, self.a) * self.eta * self.eta * wih.abs() / denom;
        }
        (f, pdf)
    }

    fn sample(&self, wo: V3, rng: &mut Rng) -> Option<BsdfSample> {
        if wo.2 <= 0.0 {
            return None;
        }
        let (u0, u1, u2) = (rng.f(), rng.f(), rng.f());
        let wi = if u0 < self.p[0] {
            cosine_sample(u1, u2)
        } else if u0 < self.p[0] + self.p[1] {
            reflect(wo, vndf_sample(wo, self.a, u1, u2))
        } else if u0 < self.p[0] + self.p[1] + self.p[2] {
            let h = vndf_sample(wo, self.a, u1, u2);
            let fr = fresnel(wo.dot(h), self.eta);
            if rng.f() < fr {
                reflect(wo, h)
            } else {
                refract(wo, h, self.eta).unwrap_or_else(|| reflect(wo, h))
            }
        } else {
            reflect(wo, vndf_sample(wo, COAT_ROUGHNESS * COAT_ROUGHNESS, u1, u2))
        };
        let (f, pdf) = self.eval(wo, wi);
        if pdf.is_nan() || pdf <= 1e-12 || !pdf.is_finite() {
            return None;
        }
        let weight = scale(f, 1.0 / pdf);
        if !finite(weight) {
            return None;
        }
        Some(BsdfSample { wi, weight, pdf, through: wi.2 < 0.0 })
    }
}

/// Multiple importance sampling: how much of a sample drawn with density `a` to keep when the
/// same light could also have been found with density `b`.
fn power(a: f32, b: f32) -> f32 {
    let (a2, b2) = (a * a, b * b);
    if a2 + b2 > 0.0 && (a2 + b2).is_finite() { a2 / (a2 + b2) } else if a > b { 1.0 } else { 0.0 }
}

// ---------------------------------------------------------------------------------------------
// The scene as the tracer sees it

struct World {
    width: u32,
    height: u32,
    geo: Arc<Geo>,
    items: Vec<ItemInfo>,
    lights: Vec<LightRes>,
    /// Cumulative chance of picking each lamp when there are too many to sample all.
    light_cdf: Vec<f32>,
    env: Option<Env>,
    env_map: Option<EnvMap>,
    /// Sample the panorama directly (off only to check the two ways agree).
    env_nee: bool,
    sky: Rgb,
    ground: Rgb,
    cam: Cam,
    background: Option<[f32; 4]>,
    fog: Option<(f32, f32, Rgb)>,
    exposure: f32,
    filmic: bool,
    eps: f32,
}

impl World {
    fn new(f: &Frame3d) -> World {
        let geo = geometry(&f.items);
        let items = f
            .items
            .iter()
            .map(|it| {
                let clear = it.mat.base[3] < 0.999 || it.mat.texture.as_ref().is_some_and(|t| t.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255));
                ItemInfo { mat: it.mat.clone(), mesh: it.mesh.clone(), clear }
            })
            .collect();
        let lights: Vec<LightRes> = f.lights.iter().filter(|l| finite(l.color) && lum(l.color) > 0.0).copied().collect();
        let mut light_cdf = Vec::with_capacity(lights.len());
        let total: f32 = lights.iter().map(|l| lum(l.color)).sum();
        let mut acc = 0.0;
        for l in &lights {
            acc += lum(l.color) / total.max(1e-12);
            light_cdf.push(acc);
        }
        let env = f.env.clone().filter(|e| e.strength.is_finite());
        let env_map = env.as_ref().filter(|e| e.kind == EnvKind::Image).and_then(|e| e.image.as_ref().and_then(|t| EnvMap::new(t, e.strength.max(0.0))));
        // Scene scale, for nudging rays off surfaces.
        let mut reach: f32 = f.camera.eye.0.abs().max(f.camera.eye.1.abs()).max(f.camera.eye.2.abs());
        for it in &f.items {
            let t = it.model.0[3];
            reach = reach.max(t[0].abs()).max(t[1].abs()).max(t[2].abs());
        }
        let eps = if reach.is_finite() { 1e-4 * (1.0 + reach) } else { 1e-3 };
        World {
            width: f.width.max(1),
            height: f.height.max(1),
            geo,
            items,
            lights,
            light_cdf,
            env,
            env_map,
            env_nee: true,
            sky: f.sky,
            ground: f.ground,
            cam: Cam::new(&f.camera, f.width.max(1), f.height.max(1)),
            background: f.background,
            fog: f.fog,
            exposure: if f.exposure.is_finite() { f.exposure.clamp(-20.0, 20.0) } else { 0.0 },
            filmic: f.filmic,
            eps,
        }
    }

    /// Light arriving from direction `d` from beyond the scene.
    fn world(&self, d: V3) -> Rgb {
        let Some(env) = &self.env else {
            // The standard engine's ambient (sky above, ground below, blended by the normal):
            // a world brightening linearly with height lights every surface exactly that much.
            let t = 0.5 + 0.75 * d.1;
            return lerp(self.ground, self.sky, t).map(|v| v.max(0.0));
        };
        // The same world the standard engine shows (its sky follows the sun, its gradient eases
        // into the horizon), so switching engines doesn't change the weather.
        let c = env.radiance(d);
        if finite(c) { c } else { [0.0; 3] }
    }

    /// Density (per solid angle) with which the panorama sampling picks direction `d`.
    fn env_pdf(&self, d: V3) -> f32 {
        let (Some(map), Some(env)) = (&self.env_map, &self.env) else { return 0.0 };
        let (u, v) = dir_to_uv(rot_y(d, -env.rotation));
        let sin = (v * PI).sin();
        if sin <= 1e-6 {
            return 0.0;
        }
        map.pdf_at(u, v) / (2.0 * PI * PI * sin)
    }

    fn visible_env(&self) -> bool {
        self.env.as_ref().is_some_and(|e| e.visible)
    }

    /// What a ray hit: position, normals, textured colour.
    fn surface(&self, o: V3, d: V3, hit: &Hit) -> Surf {
        let (item, first) = self.geo.owner[hit.prim as usize];
        let (item, first) = (item as usize, first as usize);
        let info = &self.items[item];
        let mesh = &info.mesh;
        let idx = [mesh.index[first] as usize, mesh.index[first + 1] as usize, mesh.index[first + 2] as usize];
        let (b1, b2) = (hit.u, hit.v);
        let b0 = 1.0 - b1 - b2;
        let p = o + d * hit.t;
        let tri = self.geo.bvh.tri(hit);
        let ng = tri.e1.cross(tri.e2).norm();
        let normals = &self.geo.normals[item];
        let ns = match (normals.get(idx[0]), normals.get(idx[1]), normals.get(idx[2])) {
            (Some(&a), Some(&b), Some(&c)) => {
                let n = a * b0 + b * b1 + c * b2;
                if n.len() > 1e-6 && n.0.is_finite() && n.1.is_finite() && n.2.is_finite() { n.norm() } else { ng }
            }
            _ => ng,
        };
        let uvs = idx.map(|i| mesh.uv.get(i).copied().unwrap_or([0.0, 0.0]));
        let m = &info.mat;
        let ts = m.texture_scale;
        let ts = [if ts[0].is_finite() { ts[0] } else { 1.0 }, if ts[1].is_finite() { ts[1] } else { 1.0 }];
        let uv = [(uvs[0][0] * b0 + uvs[1][0] * b1 + uvs[2][0] * b2) * ts[0], (uvs[0][1] * b0 + uvs[1][1] * b1 + uvs[2][1] * b2) * ts[1]];
        let mut base = m.base;
        if let Some(t) = &m.texture {
            let s = t.sample(uv[0], uv[1]);
            base = [base[0] * s[0], base[1] * s[1], base[2] * s[2], base[3] * s[3]];
        }
        let ns = match &m.bump {
            Some((tex, k)) if *k != 0.0 && k.is_finite() => bump(tex, *k, uv, ns, tri, uvs, ts),
            _ => ns,
        };
        Surf { p, ng, ns, base, item }
    }

    /// How much light gets from `p` along `d` to `dist` away: 0 behind something solid,
    /// partly through see-through surfaces. Unlit things cast no shadow (as in the standard
    /// engine).
    fn transmittance(&self, p: V3, d: V3, dist: f32) -> f32 {
        let mut through = 1.0f32;
        let blocked = self.geo.bvh.any_hit(p, d, dist, |hit| {
            let info = &self.items[self.geo.owner[hit.prim as usize].0 as usize];
            if info.mat.unlit {
                return false;
            }
            if !info.clear {
                return true;
            }
            let s = self.surface(p, d, &hit);
            through *= 1.0 - s.base[3].clamp(0.0, 1.0);
            through < 1e-3
        });
        if blocked { 0.0 } else { through }
    }

    fn offset(&self, p: V3, n: V3, d: V3) -> V3 {
        p + n * if d.dot(n) >= 0.0 { self.eps } else { -self.eps }
    }

    /// A point of lamp `l` seen from `p`: direction, distance and the light it brings (what a
    /// white diffuse surface facing it would reflect, as in the standard engine).
    fn sample_light(&self, l: &LightRes, p: V3, rng: &mut Rng) -> Option<(V3, f32, Rgb)> {
        let (u1, u2) = (rng.f(), rng.f());
        match l.kind {
            LightKind::Directional => {
                let mut d = (-l.v).norm();
                let half = l.size[0].max(0.0) / 2.0;
                if half > 1e-5 && half.is_finite() {
                    // A sun with a size: directions within its disc.
                    let cos_max = half.min(PI / 2.0).cos();
                    let c = 1.0 - u1 * (1.0 - cos_max);
                    let s = (1.0 - c * c).max(0.0).sqrt();
                    let phi = 2.0 * PI * u2;
                    let fr = Frame::new(d);
                    d = fr.world(V3(s * phi.cos(), s * phi.sin(), c)).norm();
                }
                Some((d, f32::INFINITY, scale(l.color, PI)))
            }
            LightKind::Point | LightKind::Spot => {
                let c = l.v;
                let mut q = c;
                let r = l.size[0];
                if r > 0.0 && r.is_finite() {
                    // A bulb: a point on the half of its sphere facing `p`.
                    let z = 1.0 - 2.0 * u1;
                    let s = (1.0 - z * z).max(0.0).sqrt();
                    let phi = 2.0 * PI * u2;
                    let mut off = V3(s * phi.cos(), s * phi.sin(), z);
                    if off.dot(p - c) < 0.0 {
                        off = -off;
                    }
                    q = c + off * r;
                }
                let to = q - p;
                let dist = to.len();
                if dist.is_nan() || dist <= 1e-6 {
                    return None;
                }
                let mut k = falloff(l, (c - p).len());
                if l.kind == LightKind::Spot {
                    let cos = l.dir.norm().dot((p - c).norm());
                    let (lo, hi) = (l.cos_outer, l.cos_inner.max(l.cos_outer + 1e-4));
                    let t = ((cos - lo) / (hi - lo)).clamp(0.0, 1.0);
                    k *= t * t * (3.0 - 2.0 * t);
                }
                (k > 0.0).then(|| (to * (1.0 / dist), dist, scale(l.color, PI * k)))
            }
            LightKind::Area => {
                let n = l.dir.norm();
                let fr = Frame::new(n);
                let q = l.v + fr.x * ((u1 - 0.5) * l.size[0].max(0.0)) + fr.y * ((u2 - 0.5) * l.size[1].max(0.0));
                let to = q - p;
                let dist = to.len();
                if dist.is_nan() || dist <= 1e-6 {
                    return None;
                }
                let wi = to * (1.0 / dist);
                // It shines from its front face only, most straight ahead.
                let facing = n.dot(-wi);
                let k = falloff(l, (l.v - p).len()) * facing;
                (k > 0.0).then(|| (wi, dist, scale(l.color, PI * k)))
            }
        }
    }
}

/// How a lamp fades with distance: as the standard engine's `shade` does (fading out towards
/// `range`, none without one), so switching engines keeps the exposure.
fn falloff(l: &LightRes, dist: f32) -> f32 {
    if l.range > 0.0 { (1.0 - dist / l.range).clamp(0.0, 1.0).powi(2) } else { 1.0 }
}

/// The shading normal tilted by the slope of a height texture at `uv`: `k` = 1 tilts it by
/// a sixteenth of the height change per texture repeat.
fn bump(tex: &Texture, k: f32, uv: [f32; 2], n: V3, tri: &Tri, uvs: [[f32; 2]; 3], ts: [f32; 2]) -> V3 {
    // Surface directions along u and v, from the triangle's corners and texture coordinates.
    let (du1, dv1) = ((uvs[1][0] - uvs[0][0]) * ts[0], (uvs[1][1] - uvs[0][1]) * ts[1]);
    let (du2, dv2) = ((uvs[2][0] - uvs[0][0]) * ts[0], (uvs[2][1] - uvs[0][1]) * ts[1]);
    let det = du1 * dv2 - du2 * dv1;
    let (dpdu, dpdv) = if det.abs() > 1e-12 {
        let inv = 1.0 / det;
        ((tri.e1 * dv2 - tri.e2 * dv1) * inv, (tri.e2 * du1 - tri.e1 * du2) * inv)
    } else {
        let f = Frame::new(n);
        (f.x, f.y)
    };
    let t = (dpdu - n * n.dot(dpdu)).norm();
    let b = (dpdv - n * n.dot(dpdv) - t * t.dot(dpdv)).norm();
    let height = |u: f32, v: f32| {
        let s = tex.sample(u, v);
        lum([s[0], s[1], s[2]])
    };
    let (eu, ev) = (1.0 / tex.width.max(1) as f32, 1.0 / tex.height.max(1) as f32);
    let gu = (height(uv[0] + eu, uv[1]) - height(uv[0] - eu, uv[1])) / (2.0 * eu);
    let gv = (height(uv[0], uv[1] + ev) - height(uv[0], uv[1] - ev)) / (2.0 * ev);
    let out = (n - (t * gu + b * gv) * (k / 16.0)).norm();
    if out.0.is_finite() && out.1.is_finite() && out.2.is_finite() { out } else { n }
}

// ---------------------------------------------------------------------------------------------
// Paths

/// What one camera ray found.
#[derive(Default)]
struct Sample {
    /// Light (linear, before exposure), and unlit colour seen directly (shown as is).
    c: Rgb,
    raw: Rgb,
    /// Coverage: 0 where the camera saw nothing (the background shows).
    a: f32,
    /// First hit, for the denoiser: albedo, normal, distance.
    aov: Option<(Rgb, V3, f32)>,
    rays: u64,
}

impl World {
    fn trace(&self, x: u32, y: u32, s: u32, settings: &Settings) -> Sample {
        let mut rng = Rng::new(y as u64 * self.width as u64 + x as u64, s, settings.seed);
        let (jx, jy) = match settings.filter {
            Filter::Box => (rng.f() - 0.5, rng.f() - 0.5),
            Filter::BlackmanHarris => (blackman_harris(rng.f()), blackman_harris(rng.f())),
        };
        let sx = 2.0 * (x as f32 + 0.5 + jx) / self.width as f32 - 1.0;
        let sy = 1.0 - 2.0 * (y as f32 + 0.5 + jy) / self.height as f32;
        let (mut o, mut d) = self.cam.ray(sx, sy, (rng.f(), rng.f()));

        let mut out = Sample { a: 1.0, ..Sample::default() };
        let mut beta: Rgb = [1.0; 3];
        let mut depth = 0u32;
        let mut passes = 0u32;
        // Only refraction so far: what lies beyond is seen through (or, when nothing is, the
        // background shows through).
        let mut see = true;
        let mut prev_pdf = 0.0f32;
        let mut env_sampled = false;
        let mut fog = 0.0f32;
        let mut aov_tint: Rgb = [1.0; 3];
        let mut aov_dist = 0.0f32;
        let visible = self.visible_env();

        /// Adds light the path brought; bright indirect light is clamped.
        fn emit(out: &mut Sample, c: Rgb, indirect: bool) {
            if !finite(c) {
                return;
            }
            let c = if indirect && lum(c) > CLAMP { scale(c, CLAMP / lum(c)) } else { c };
            out.c = add(out.c, c);
        }

        loop {
            out.rays += 1;
            let Some(hit) = self.geo.bvh.intersect(o, d, f32::INFINITY) else {
                if see && !visible {
                    // Nothing behind: transparent, as much as the light that got here.
                    out.a = (out.a - lum(beta).clamp(0.0, 1.0)).max(0.0);
                } else {
                    let w = if env_sampled && depth > 0 { power(prev_pdf, self.env_pdf(d)) } else { 1.0 };
                    emit(&mut out, scale(mul(beta, self.world(d)), w), depth > 0 && !see);
                }
                if out.aov.is_none() && depth > 0 {
                    out.aov = Some((aov_tint, -d, aov_dist + 1e4));
                }
                break;
            };
            let surf = self.surface(o, d, &hit);
            let info = &self.items[surf.item];
            let mat = &info.mat;

            // See-through (opacity): carry straight on, without a bounce.
            let alpha = surf.base[3].clamp(0.0, 1.0);
            if alpha < 1.0 && rng.f() >= alpha {
                passes += 1;
                if passes > MAX_PASSES {
                    break;
                }
                o = surf.p + d * self.eps;
                continue;
            }
            let base: Rgb = [surf.base[0].max(0.0), surf.base[1].max(0.0), surf.base[2].max(0.0)];
            if depth == 0
                && let Some((near, far, _)) = self.fog
            {
                let k = (((surf.p - self.cam.eye).len() - near) / (far - near).max(1e-3)).clamp(0.0, 1.0);
                fog = k * k * (3.0 - 2.0 * k);
            }
            aov_dist += hit.t;

            if mat.unlit {
                // Seen as it is; lights what it faces like a glowing surface.
                let le = add(base, mat.emissive);
                if depth == 0 {
                    out.raw = add(out.raw, le);
                } else {
                    emit(&mut out, mul(beta, le), !see);
                }
                // Not denoised when seen directly (it has no grain); seen in a mirror, it guides.
                if out.aov.is_none() && depth > 0 {
                    out.aov = Some((mul(aov_tint, base), surf.ns, aov_dist));
                }
                break;
            }
            if mat.emissive.iter().any(|&e| e > 0.0) {
                emit(&mut out, mul(beta, mat.emissive), depth > 0 && !see);
            }

            let wo_w = -d;
            let entering = d.dot(surf.ns) < 0.0;
            let ng = if wo_w.dot(surf.ng) >= 0.0 { surf.ng } else { -surf.ng };
            let ns = if wo_w.dot(surf.ns) >= 0.0 { surf.ns } else { -surf.ns };
            // Interpolated normals can face away from the viewer near silhouettes.
            let n = if wo_w.dot(ns) > 1e-4 { ns } else { ng };
            let frame = Frame::new(n);
            let wo = frame.local(wo_w);
            let bsdf = Bsdf::new(mat, base, entering, wo);

            if out.aov.is_none() {
                if bsdf.mirror_like() && depth < 3 {
                    aov_tint = mul(aov_tint, bsdf.albedo());
                } else {
                    out.aov = Some((mul(aov_tint, bsdf.albedo()), n, aov_dist));
                }
            }

            // Light from the lamps, each through a shadow ray.
            let indirect = depth > 0 && !see;
            let pick = self.lights.len() > ALL_LIGHTS;
            let n_lights = if pick { 1 } else { self.lights.len() };
            for k in 0..n_lights {
                let (l, chance) = if pick {
                    let i = find_light(&self.light_cdf, rng.f());
                    let prev = if i == 0 { 0.0 } else { self.light_cdf[i - 1] };
                    (&self.lights[i], (self.light_cdf[i] - prev).max(1e-6))
                } else {
                    (&self.lights[k], 1.0)
                };
                let Some((wi_w, dist, li)) = self.sample_light(l, surf.p, &mut rng) else { continue };
                if wi_w.dot(ng) <= 0.0 && bsdf.glass <= 0.0 {
                    continue;
                }
                let (f, _) = bsdf.eval(wo, frame.local(wi_w));
                if lum(f) <= 0.0 {
                    continue;
                }
                let shadow = if l.shadows {
                    out.rays += 1;
                    let from = self.offset(surf.p, ng, wi_w);
                    self.transmittance(from, wi_w, if dist.is_finite() { dist * (1.0 - 1e-4) - self.eps } else { f32::INFINITY })
                } else {
                    1.0
                };
                if shadow > 0.0 {
                    emit(&mut out, scale(mul(mul(beta, f), li), shadow / chance), indirect);
                }
            }
            // Light from a panorama, aimed at its bright parts.
            if let (Some(map), Some(env)) = (&self.env_map, &self.env)
                && self.env_nee
            {
                let (u, v, pdf_uv) = map.sample(rng.f(), rng.f());
                let sin = (v * PI).sin();
                if sin > 1e-6 && pdf_uv > 0.0 {
                    let wi_w = rot_y(uv_to_dir(u, v), env.rotation);
                    let pdf_l = pdf_uv / (2.0 * PI * PI * sin);
                    let (f, pdf_b) = bsdf.eval(wo, frame.local(wi_w));
                    if lum(f) > 0.0 && (wi_w.dot(ng) > 0.0 || bsdf.glass > 0.0) {
                        out.rays += 1;
                        let from = self.offset(surf.p, ng, wi_w);
                        let t = self.transmittance(from, wi_w, f32::INFINITY);
                        if t > 0.0 {
                            let le = self.world(wi_w);
                            emit(&mut out, scale(mul(mul(beta, f), le), t * power(pdf_l, pdf_b) / pdf_l), indirect);
                        }
                    }
                }
            }

            if depth >= settings.bounces {
                break;
            }
            let Some(smp) = bsdf.sample(wo, &mut rng) else { break };
            let wi_w = frame.world(smp.wi);
            // Reflections must leave from the side they hit, refractions from the other.
            if (wi_w.dot(ng) > 0.0) == smp.through {
                break;
            }
            beta = mul(beta, smp.weight);
            see = see && smp.through;
            prev_pdf = smp.pdf;
            env_sampled = self.env_map.is_some() && self.env_nee;
            o = self.offset(surf.p, ng, wi_w);
            d = wi_w;
            depth += 1;
            if depth >= 3 {
                let q = lum(beta).clamp(0.0, 0.95);
                if rng.f() >= q {
                    break;
                }
                beta = scale(beta, 1.0 / q);
            }
            if !finite(beta) || lum(beta) <= 0.0 {
                break;
            }
        }
        if fog > 0.0 {
            let (_, _, bg) = self.fog.unwrap_or((0.0, 0.0, [0.0; 3]));
            out.c = lerp(out.c, scale(bg, out.a), fog);
        }
        out
    }
}

fn find_light(cdf: &[f32], u: f32) -> usize {
    cdf.partition_point(|&c| c <= u).min(cdf.len().saturating_sub(1))
}

// ---------------------------------------------------------------------------------------------
// Accumulation and output

/// Running sums for one pixel.
#[derive(Clone, Copy, Default)]
struct Px {
    c: Rgb,
    raw: Rgb,
    a: f32,
    /// Sum of squared luminance (for the noise estimate).
    l2: f32,
    albedo: Rgb,
    normal: Rgb,
    depth: f32,
    hits: f32,
}

/// A path-traced frame that gets better with every call to [`Progressive::add`].
pub struct Progressive {
    world: World,
    settings: Settings,
    px: Vec<Px>,
    samples: u32,
    rays: u64,
    seconds: f64,
}

impl Progressive {
    pub(crate) fn new(frame: &Frame3d, settings: Settings) -> Progressive {
        let world = World::new(frame);
        let n = world.width as usize * world.height as usize;
        Progressive { world, settings, px: vec![Px::default(); n], samples: 0, rays: 0, seconds: 0.0 }
    }

    /// Samples per pixel so far.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Samples per pixel the render settings ask for.
    pub fn target(&self) -> u32 {
        self.settings.samples
    }

    /// The settings' samples are all in.
    pub fn done(&self) -> bool {
        self.samples >= self.settings.samples
    }

    /// Renders `n` more samples per pixel, on every core.
    pub fn add(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        let t0 = Instant::now();
        let (w, h) = (self.world.width as usize, self.world.height as usize);
        let (from, to) = (self.samples, self.samples.saturating_add(n));
        let bands: Vec<Mutex<(usize, &mut [Px])>> = self.px.chunks_mut(w * BAND).enumerate().map(Mutex::new).collect();
        let next = AtomicUsize::new(0);
        let rays = AtomicU64::new(0);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(bands.len().max(1));
        let (world, settings) = (&self.world, &self.settings);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut count = 0u64;
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(band) = bands.get(i) else { break };
                        let mut band = band.lock().unwrap_or_else(|e| e.into_inner());
                        let (b, pixels) = &mut *band;
                        let y0 = *b * BAND;
                        for (k, px) in pixels.iter_mut().enumerate() {
                            let (x, y) = ((k % w) as u32, (y0 + k / w) as u32);
                            for s in from..to {
                                let smp = world.trace(x, y, s, settings);
                                count += smp.rays;
                                px.c = add(px.c, smp.c);
                                px.raw = add(px.raw, smp.raw);
                                px.a += smp.a;
                                px.l2 += lum(smp.c) * lum(smp.c);
                                if let Some((albedo, n, depth)) = smp.aov {
                                    px.albedo = add(px.albedo, albedo);
                                    px.normal = add(px.normal, n.arr());
                                    px.depth += depth;
                                    px.hits += 1.0;
                                }
                            }
                        }
                    }
                    rays.fetch_add(count, Ordering::Relaxed);
                });
            }
        });
        let secs = t0.elapsed().as_secs_f64();
        let r = rays.into_inner();
        self.samples = to;
        self.rays += r;
        self.seconds += secs;
        tracing::debug!(
            "path traced {w}×{h}: {n} samples per pixel in {:.2} s ({:.1} M rays/s, {} so far)",
            secs,
            r as f64 / secs.max(1e-9) / 1e6,
            self.samples
        );
    }

    /// The picture so far (denoised when the settings ask), premultiplied.
    pub fn picture(&self) -> Pixmap {
        let (w, h) = (self.world.width as usize, self.world.height as usize);
        let n = self.samples.max(1) as f32;
        // Mean light where something was seen (not divided by coverage yet).
        let mut shaded: Vec<Rgb> = self.px.iter().map(|p| if p.a > 0.0 { scale(p.c, 1.0 / p.a) } else { [0.0; 3] }).collect();
        if self.settings.denoise && self.samples > 0 {
            let t0 = Instant::now();
            let valid: Vec<bool> = self.px.iter().map(|p| p.hits > 0.0 && p.a > 0.0).collect();
            let albedo: Vec<Rgb> = self.px.iter().map(|p| if p.hits > 0.0 { scale(p.albedo, 1.0 / p.hits) } else { [0.0; 3] }).collect();
            let normal: Vec<Rgb> = self.px.iter().map(|p| V3(p.normal[0], p.normal[1], p.normal[2]).norm().arr()).collect();
            let depth: Vec<f32> = self.px.iter().map(|p| if p.hits > 0.0 { p.depth / p.hits } else { 0.0 }).collect();
            let variance: Vec<f32> = self
                .px
                .iter()
                .map(|p| {
                    let m = lum(p.c) / n;
                    let cover = (p.a / n).max(1e-3);
                    // Of the mean, and in the same units as `shaded`.
                    ((p.l2 / n - m * m).max(0.0) / n) / (cover * cover)
                })
                .collect();
            let g = denoise::Guides { albedo: &albedo, normal: &normal, depth: &depth, variance: &variance, valid: &valid };
            shaded = denoise::denoise(w, h, &shaded, &g);
            tracing::debug!("path tracer: denoised in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
        }
        let bg = self.world.background.map(|c| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]).unwrap_or([0.0; 4]);
        let gain = 2f32.powf(self.world.exposure);
        let filmic = self.world.filmic;
        let mut out = vec![0u8; w * h * 4];
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let i = y * w + x;
                let p = &self.px[i];
                let a = (p.a / n).clamp(0.0, 1.0);
                let mut c = [0.0f32; 4];
                if a > 0.0 {
                    let raw = scale(p.raw, 1.0 / p.a);
                    for k in 0..3 {
                        let v = tone_map(shaded[i][k] * gain, filmic) + raw[k];
                        let v = if v.is_finite() { v } else { 0.0 };
                        c[k] = linear_to_srgb(v) * a;
                    }
                    c[3] = a;
                }
                let px = &mut row[x * 4..x * 4 + 4];
                let k = 1.0 - c[3];
                let oa = (c[3] + bg[3] * k).clamp(0.0, 1.0);
                for j in 0..3 {
                    px[j] = ((c[j] + bg[j] * k).clamp(0.0, oa) * 255.0).round() as u8;
                }
                px[3] = (oa * 255.0).round() as u8;
            }
        });
        Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(w as u32, h as u32).expect("non-empty")).expect("sized")
    }

    /// Rays traced so far and the time it took.
    pub fn stats(&self) -> (u64, f64) {
        (self.rays, self.seconds)
    }
}

#[cfg(test)]
mod tests;
