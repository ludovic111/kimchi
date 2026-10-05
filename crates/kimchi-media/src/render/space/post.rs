//! Camera effects after a 3D frame is drawn, shared by the GPU and CPU renderers (and the path
//! tracer): screen-space ambient occlusion, depth of field, bloom, then exposure, tone mapping
//! and sRGB encoding into the premultiplied picture the compositor takes.
//!
//! Frames without any of these effects never come here: the shaders tone map and encode
//! directly (faster, and exactly what earlier versions drew). With an effect, they write linear
//! light ([`Hdr`]) and the distance of each pixel, and [`finish`] does the rest.

use rayon::prelude::*;
use tiny_skia::Pixmap;

use super::math::V3;
use super::{Frame3d, Quality, linear_to_srgb};

/// A frame before tone mapping.
pub(crate) struct Hdr {
    pub width: u32,
    pub height: u32,
    /// Linear RGB premultiplied by alpha, rows from the top.
    pub rgba: Vec<[f32; 4]>,
    /// Distance from the camera along its viewing direction per pixel (world units), infinite
    /// where nothing was drawn; empty when unknown (then occlusion and depth of field are skipped).
    pub depth: Vec<f32>,
}

/// Glow around bright things.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Bloom {
    /// 0 = off.
    pub intensity: f32,
    /// Linear brightness where the glow starts.
    pub threshold: f32,
    /// Size as a share of the frame height.
    pub radius: f32,
}

/// Highlights above 0.8 roll off smoothly towards 1 (the "standard" look), or a film-like curve.
pub(crate) fn tone(v: f32, filmic: bool) -> f32 {
    if filmic {
        // Narkowicz's fit of the ACES film curve.
        let v = v.max(0.0);
        return ((v * (2.51 * v + 0.03)) / (v * (2.43 * v + 0.59) + 0.14)).clamp(0.0, 1.0);
    }
    super::shoulder(v)
}

/// The linear value `tone` turns into `y` (0–1), for things that must come out exactly as
/// given (unlit materials, background colours) when tone mapping happens later.
pub(crate) fn untone(y: f32, filmic: bool) -> f32 {
    let y = y.clamp(0.0, 0.9999);
    if filmic {
        let (a, b, c) = (2.51 - 2.43 * y, 0.03 - 0.59 * y, -0.14 * y);
        if a.abs() < 1e-6 {
            return -c / b;
        }
        return ((-b + (b * b - 4.0 * a * c).max(0.0).sqrt()) / (2.0 * a)).max(0.0);
    }
    if y <= 0.8 { y } else { 0.8 - 0.2 * (1.0 - (y - 0.8) / 0.2).ln() }
}

/// Stops of exposure as a multiplier.
pub(crate) fn gain(f: &Frame3d) -> f32 {
    2f32.powf(f.exposure)
}

/// Whether `f` needs the linear-light path (an effect that works on the whole picture).
pub(crate) fn needed(f: &Frame3d) -> bool {
    f.bloom.intensity > 0.0 || f.ao > 0.0 || f.camera.aperture > 0.0
}

/// Applies the frame's camera effects and encodes. `hdr` comes from either renderer.
pub(crate) fn finish(f: &Frame3d, mut hdr: Hdr) -> Pixmap {
    let (w, h) = (hdr.width as usize, hdr.height as usize);
    let has_depth = hdr.depth.len() == w * h;
    if f.ao > 0.0 && has_depth {
        ambient_occlusion(f, &mut hdr);
    }
    if f.camera.aperture > 0.0 && has_depth {
        depth_of_field(f, &mut hdr);
    }
    if f.bloom.intensity > 0.0 {
        bloom(f, &mut hdr);
    }
    encode(f, &hdr)
}

/// Exposure, tone mapping and sRGB, premultiplied 8-bit.
pub(crate) fn encode(f: &Frame3d, hdr: &Hdr) -> Pixmap {
    let (w, h) = (hdr.width as usize, hdr.height as usize);
    let k = gain(f);
    let mut out = vec![0u8; w * h * 4];
    out.par_chunks_mut(w * 4).zip(hdr.rgba.par_chunks(w)).for_each(|(row, src)| {
        for (px, c) in row.as_chunks_mut::<4>().0.iter_mut().zip(src) {
            let a = c[3].clamp(0.0, 1.0);
            if a <= 1e-6 {
                *px = [0; 4];
                continue;
            }
            for i in 0..3 {
                let v = linear_to_srgb(tone(c[i] / a * k, f.filmic));
                px[i] = crate::render::byte(v.clamp(0.0, 1.0) * a * 255.0);
            }
            px[3] = crate::render::byte(a * 255.0);
        }
    });
    Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(hdr.width.max(1), hdr.height.max(1)).expect("non-empty")).expect("sized")
}

/// Where pixel (`x`, `y`) at distance `d` is, in the world.
fn world_at(f: &Frame3d, w: usize, h: usize, x: f32, y: f32, d: f32) -> V3 {
    let c = &f.camera;
    let nx = (x + 0.5) / w as f32 * 2.0 - 1.0;
    let ny = 1.0 - (y + 0.5) / h as f32 * 2.0;
    let aspect = w as f32 / h.max(1) as f32;
    if c.ortho {
        let hh = c.ortho_size / 2.0;
        return c.eye + c.right * (nx * hh * aspect) + c.up * (ny * hh) + c.forward * d;
    }
    let t = (c.fov_y / 2.0).tan();
    c.eye + (c.forward + c.right * (nx * t * aspect) + c.up * (ny * t)) * d
}

/// Darkens creases and contact points from the depth buffer (normals rebuilt from it), with a
/// small blur that keeps edges.
fn ambient_occlusion(f: &Frame3d, hdr: &mut Hdr) {
    let (w, h) = (hdr.width as usize, hdr.height as usize);
    let depth = &hdr.depth;
    let c = &f.camera;
    let radius = (c.focus * 0.06).clamp(0.02, 50.0);
    let samples: usize = if f.quality == Quality::Final { 16 } else { 8 };
    let strength = f.ao.clamp(0.0, 4.0);
    let pos = |x: usize, y: usize| world_at(f, w, h, x as f32, y as f32, depth[y * w + x]);
    let mut occ = vec![0.0f32; w * h];
    occ.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let d = depth[y * w + x];
            if !d.is_finite() {
                continue;
            }
            let p = pos(x, y);
            // The normal from whichever neighbours are on the same surface.
            let pick = |a: Option<(usize, usize)>, b: Option<(usize, usize)>| -> V3 {
                let da = a.map(|(x, y)| (depth[y * w + x] - d).abs()).unwrap_or(f32::MAX);
                let db = b.map(|(x, y)| (depth[y * w + x] - d).abs()).unwrap_or(f32::MAX);
                if da <= db {
                    a.map_or(V3::default(), |(x, y)| p - pos(x, y))
                } else {
                    b.map_or(V3::default(), |(x, y)| pos(x, y) - p)
                }
            };
            let dx = pick((x > 0).then(|| (x - 1, y)), (x + 1 < w).then(|| (x + 1, y)));
            let dy = pick((y > 0).then(|| (x, y - 1)), (y + 1 < h).then(|| (x, y + 1)));
            let mut n = dx.cross(dy).norm();
            if n.dot(c.eye - p) < 0.0 && !c.ortho || c.ortho && n.dot(-c.forward) < 0.0 {
                n = -n;
            }
            let (t, b) = (n.perpendicular(), n.cross(n.perpendicular()));
            // A different turn of the sample pattern per pixel of a 4×4 block (blurred away below).
            let turn = ((x % 4) * 4 + (y % 4)) as f32 * std::f32::consts::FRAC_PI_8;
            let mut hidden = 0.0;
            for i in 0..samples {
                let k = (i as f32 + 0.5) / samples as f32;
                let a = i as f32 * 2.399_963 + turn;
                let r = k.sqrt();
                let z = (1.0 - r * r).sqrt().max(0.1);
                let s = p + (t * (a.cos() * r) + b * (a.sin() * r) + n * z) * (radius * (0.2 + 0.8 * k * k));
                let q = f.viewproj.point(s);
                if q[3] <= 1e-6 {
                    continue;
                }
                let (sx, sy) = ((q[0] / q[3] * 0.5 + 0.5) * w as f32, (0.5 - q[1] / q[3] * 0.5) * h as f32);
                if sx < 0.0 || sy < 0.0 || sx >= w as f32 || sy >= h as f32 {
                    continue;
                }
                let scene = depth[sy as usize * w + sx as usize];
                let sd = (s - c.eye).dot(c.forward);
                if scene < sd - radius * 0.03 {
                    // Nearer things only occlude when close enough to matter.
                    let range = (radius / (d - scene).abs().max(1e-4)).min(1.0);
                    hidden += range;
                }
            }
            *o = hidden / samples as f32;
        }
    });
    // A 4×4 blur that stays on one surface.
    let blurred: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let d = depth[i];
            if !d.is_finite() {
                return 0.0;
            }
            let (mut sum, mut n) = (0.0, 0.0);
            for dy in -2i64..2 {
                for dx in -2i64..2 {
                    let (sx, sy) = (x as i64 + dx, y as i64 + dy);
                    if sx < 0 || sy < 0 || sx >= w as i64 || sy >= h as i64 {
                        continue;
                    }
                    let j = sy as usize * w + sx as usize;
                    if (depth[j] - d).abs() < radius * 0.5 {
                        sum += occ[j];
                        n += 1.0;
                    }
                }
            }
            if n > 0.0 { sum / n } else { 0.0 }
        })
        .collect();
    hdr.rgba.par_iter_mut().zip(blurred).for_each(|(c, o)| {
        let k = (1.0 - strength * o).clamp(0.0, 1.0);
        c[0] *= k;
        c[1] *= k;
        c[2] *= k;
    });
}

/// Circle of confusion (radius in pixels) of something `d` away, for a `h`-pixel-high frame.
pub(crate) fn coc(f: &Frame3d, d: f32, h: usize) -> f32 {
    let c = &f.camera;
    let s = c.focus.max(1e-3);
    // The blur circle at the focus plane (world units), then its size on screen.
    let (world, per_unit) = if c.ortho {
        (c.aperture * (d - s).abs() / s, h as f32 / c.ortho_size.max(1e-3))
    } else {
        let d = d.max(1e-3);
        (c.aperture * if d.is_finite() { (d - s).abs() / d } else { 1.0 }, h as f32 / 2.0 / (s * (c.fov_y / 2.0).tan()).max(1e-6))
    };
    (world * per_unit).min(max_coc(h))
}

fn max_coc(h: usize) -> f32 {
    (h as f32 * 0.03).max(2.0)
}

/// Depth of field: each pixel gathers the neighbours whose blur circle reaches it, along a
/// golden-angle spiral; things behind the pixel can't spill over it more than it is blurred
/// itself, so sharp things in front keep their edges and there is no halo around them.
fn depth_of_field(f: &Frame3d, hdr: &mut Hdr) {
    let (w, h) = (hdr.width as usize, hdr.height as usize);
    let sizes: Vec<f32> = hdr.depth.par_iter().map(|&d| coc(f, d, h)).collect();
    let largest = sizes.iter().cloned().fold(0.0f32, f32::max);
    if largest < 0.5 {
        return;
    }
    // About this many samples reach the largest circle (the spiral keeps them evenly spread:
    // each step adds the same area); fewer in previews.
    let count = if f.quality == Quality::Final { 160.0 } else { 48.0 };
    let spacing = largest * largest / (2.0 * count);
    // The spiral is the same for every pixel: its offsets once.
    let mut spiral: Vec<(f32, f32, f32)> = vec![];
    let (mut radius, mut angle) = (spacing.sqrt(), 0.0f32);
    while radius < largest {
        spiral.push((angle.cos() * radius, angle.sin() * radius, radius));
        radius += spacing / radius;
        angle += 2.399_963;
    }
    // A sample only counts when the circle of the pixel it lands on reaches back this far, so a
    // pixel can stop at the largest circle around it: per tile, the largest circle within reach.
    let reach = tile_reach(&sizes, w, h, largest);
    let tiles_w = w.div_ceil(DOF_TILE);
    let src = &hdr.rgba;
    let depth = &hdr.depth;
    let out: Vec<[f32; 4]> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (xi, yi) = (i % w, i / w);
            let (x, y) = (xi as f32, yi as f32);
            let (cd, cs) = (depth[i], sizes[i]);
            let stop = reach[(yi / DOF_TILE) * tiles_w + xi / DOF_TILE] + 0.5;
            // The running mean of what was gathered, and how many samples it holds.
            let mut mean = src[i];
            let mut tot = 1.0f32;
            for &(dx, dy, radius) in &spiral {
                if radius >= stop {
                    // Every sample from here on has no weight.
                    break;
                }
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0.0 && sy >= 0.0 && sx < w as f32 && sy < h as f32 {
                    let j = sy as usize * w + sx as usize;
                    let mut ss = sizes[j];
                    if depth[j] > cd {
                        ss = ss.min(cs * 2.0);
                    }
                    tot += 1.0;
                    let e = (ss - radius + 0.5).clamp(0.0, 1.0);
                    if e > 0.0 {
                        let m = e * e * (3.0 - 2.0 * e) / tot;
                        let s = src[j];
                        for k in 0..4 {
                            mean[k] += (s[k] - mean[k]) * m;
                        }
                    }
                }
            }
            mean
        })
        .collect();
    hdr.rgba = out;
}

/// Tiles (pixels a side) of the depth-of-field reach map.
const DOF_TILE: usize = 16;

/// The largest blur circle any pixel within `largest` of each `DOF_TILE` tile has.
fn tile_reach(sizes: &[f32], w: usize, h: usize, largest: f32) -> Vec<f32> {
    let (tw, th) = (w.div_ceil(DOF_TILE), h.div_ceil(DOF_TILE));
    let own: Vec<f32> = (0..tw * th)
        .into_par_iter()
        .map(|t| {
            let (tx, ty) = (t % tw, t / tw);
            let mut m = 0.0f32;
            for y in ty * DOF_TILE..((ty + 1) * DOF_TILE).min(h) {
                for &s in &sizes[y * w + tx * DOF_TILE..y * w + ((tx + 1) * DOF_TILE).min(w)] {
                    m = m.max(s);
                }
            }
            m
        })
        .collect();
    let r = (largest / DOF_TILE as f32).ceil() as isize + 1;
    (0..tw * th)
        .into_par_iter()
        .map(|t| {
            let (tx, ty) = ((t % tw) as isize, (t / tw) as isize);
            let mut m = 0.0f32;
            for y in (ty - r).max(0)..(ty + r + 1).min(th as isize) {
                for x in (tx - r).max(0)..(tx + r + 1).min(tw as isize) {
                    m = m.max(own[y as usize * tw + x as usize]);
                }
            }
            m
        })
        .collect()
}

/// Glow: what is brighter than the threshold, blurred at two sizes, added back.
fn bloom(f: &Frame3d, hdr: &mut Hdr) {
    let (w, h) = (hdr.width as usize, hdr.height as usize);
    let b = f.bloom;
    let k = gain(f);
    let radius = (b.radius.max(0.0) * h as f32).max(1.0);
    // Work at a reduced size so the blur stays a few pixels wide there.
    let ds = ((radius / 6.0).floor() as usize).clamp(1, 16);
    let (lw, lh) = (w.div_ceil(ds), h.div_ceil(ds));
    let mut bright = vec![[0.0f32; 3]; lw * lh];
    let knee = (b.threshold * 0.5).max(1e-3);
    bright.par_chunks_mut(lw).enumerate().for_each(|(ly, row)| {
        for (lx, out) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 3];
            let mut n = 0.0f32;
            for y in ly * ds..((ly + 1) * ds).min(h) {
                for x in lx * ds..((lx + 1) * ds).min(w) {
                    let c = hdr.rgba[y * w + x];
                    let rgb = [c[0] * k, c[1] * k, c[2] * k];
                    let l = rgb[0].max(rgb[1]).max(rgb[2]);
                    // A soft knee around the threshold.
                    let soft = ((l - b.threshold + knee).clamp(0.0, 2.0 * knee)).powi(2) / (4.0 * knee);
                    let keep = soft.max(l - b.threshold) / l.max(1e-5);
                    for i in 0..3 {
                        acc[i] += rgb[i] * keep.max(0.0);
                    }
                    n += 1.0;
                }
            }
            *out = acc.map(|v| v / n.max(1.0));
        }
    });
    let near = gaussian(&bright, lw, lh, radius / 3.0 / ds as f32);
    let far = gaussian(&bright, lw, lh, radius / ds as f32);
    let intensity = b.intensity.max(0.0) / k;
    hdr.rgba.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, c) in row.iter_mut().enumerate() {
            // Bilinear from the small glow.
            let (fx, fy) = (((x as f32 + 0.5) / ds as f32 - 0.5).max(0.0), ((y as f32 + 0.5) / ds as f32 - 0.5).max(0.0));
            let (x0, y0) = ((fx as usize).min(lw - 1), (fy as usize).min(lh - 1));
            let (x1, y1) = ((x0 + 1).min(lw - 1), (y0 + 1).min(lh - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let at = |v: &[[f32; 3]], i: usize| -> f32 {
                let s = |xx: usize, yy: usize| v[yy * lw + xx][i];
                (s(x0, y0) * (1.0 - tx) + s(x1, y0) * tx) * (1.0 - ty) + (s(x0, y1) * (1.0 - tx) + s(x1, y1) * tx) * ty
            };
            let mut glow = [0.0f32; 3];
            for i in 0..3 {
                glow[i] = (0.6 * at(&near, i) + 0.4 * at(&far, i)) * intensity;
                c[i] += glow[i];
            }
            // Glow over a transparent background makes it a little less transparent.
            let g = tone(glow[0].max(glow[1]).max(glow[2]) * k, f.filmic);
            c[3] = (c[3] + (1.0 - c[3]) * g).clamp(0.0, 1.0);
        }
    });
}

/// A separable Gaussian blur, `sigma` in pixels, edges clamped.
fn gaussian(src: &[[f32; 3]], w: usize, h: usize, sigma: f32) -> Vec<[f32; 3]> {
    let sigma = sigma.max(0.3);
    let r = (sigma * 3.0).ceil() as i64;
    let weights: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let total: f32 = weights.iter().sum();
    let mut tmp = vec![[0.0f32; 3]; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 3];
            for (i, k) in (-r..=r).zip(&weights) {
                let sx = (x as i64 + i).clamp(0, w as i64 - 1) as usize;
                let c = src[y * w + sx];
                for j in 0..3 {
                    acc[j] += c[j] * k;
                }
            }
            *o = acc.map(|v| v / total);
        }
    });
    let mut out = vec![[0.0f32; 3]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 3];
            for (i, k) in (-r..=r).zip(&weights) {
                let sy = (y as i64 + i).clamp(0, h as i64 - 1) as usize;
                let c = tmp[sy * w + x];
                for j in 0..3 {
                    acc[j] += c[j] * k;
                }
            }
            *o = acc.map(|v| v / total);
        }
    });
    out
}

