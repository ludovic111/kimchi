//! Smooths the path tracer's remaining grain: an edge-avoiding À-trous wavelet filter (five
//! passes of a 5×5 kernel with holes 1, 2, 4, 8 and 16 pixels apart) guided by what the camera
//! rays first hit: its colour (albedo), normal and distance. The light is divided by the albedo
//! before filtering and multiplied back after, so textures stay sharp; weights fall off across
//! normal, depth and albedo edges, and across brightness changes larger than the noise there
//! (from each pixel's sample variance), so shadow edges stay too.

use rayon::prelude::*;

/// B3 spline taps.
const KERNEL: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];
const PASSES: usize = 5;
/// How many standard deviations of noise a brightness difference may be and still be blurred.
const SIGMA_LUM: f32 = 3.0;
const NORMAL_POWER: i32 = 64;
const SIGMA_ALBEDO: f32 = 0.08;

/// What each pixel's first hit was. Pixels with `valid` false (nothing hit) are left alone and
/// never blur into the others.
pub(crate) struct Guides<'a> {
    pub albedo: &'a [[f32; 3]],
    pub normal: &'a [[f32; 3]],
    pub depth: &'a [f32],
    /// Variance of the pixel's mean luminance (how noisy it still is).
    pub variance: &'a [f32],
    pub valid: &'a [bool],
}

fn lum(c: [f32; 3]) -> f32 {
    c[0] * 0.2126 + c[1] * 0.7152 + c[2] * 0.0722
}

/// `color` (linear, `w`×`h`) with its noise smoothed away.
pub(crate) fn denoise(w: usize, h: usize, color: &[[f32; 3]], g: &Guides) -> Vec<[f32; 3]> {
    let n = w * h;
    if n == 0 || color.len() != n || [g.albedo.len(), g.normal.len(), g.depth.len(), g.variance.len(), g.valid.len()].iter().any(|&l| l != n) {
        return color.to_vec();
    }
    let floor = |a: f32| a.max(0.02);
    // Light reaching each pixel, without its surface colour.
    let mut irr: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            let (c, a) = (color[i], g.albedo[i]);
            let c = c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
            if g.valid[i] { [c[0] / floor(a[0]), c[1] / floor(a[1]), c[2] / floor(a[2])] } else { c }
        })
        .collect();
    let mut var: Vec<f32> = (0..n)
        .map(|i| {
            let v = g.variance[i];
            let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
            v / floor(lum(g.albedo[i])).powi(2)
        })
        .collect();
    // The variance of a few samples is noisy itself: smooth it over 3×3 first.
    var = blur3(w, h, &var, g.valid);
    // How fast depth changes across a pixel, so slanted floors aren't mistaken for edges.
    let slope: Vec<f32> = (0..n)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let d = |j: usize| if g.valid[j] { (g.depth[j] - g.depth[i]).abs() } else { 0.0 };
            let mut s: f32 = 0.0;
            if x > 0 {
                s = s.max(d(i - 1));
            }
            if x + 1 < w {
                s = s.max(d(i + 1));
            }
            if y > 0 {
                s = s.max(d(i - w));
            }
            if y + 1 < h {
                s = s.max(d(i + w));
            }
            s
        })
        .collect();

    for pass in 0..PASSES {
        let step = 1usize << pass;
        let mut next_irr = vec![[0.0f32; 3]; n];
        let mut next_var = vec![0.0f32; n];
        next_irr.par_chunks_mut(w).zip(next_var.par_chunks_mut(w)).enumerate().for_each(|(y, (row, vrow))| {
            for x in 0..w {
                let p = y * w + x;
                if !g.valid[p] {
                    row[x] = irr[p];
                    vrow[x] = var[p];
                    continue;
                }
                let (lp, np, zp, ap) = (lum(irr[p]), g.normal[p], g.depth[p], g.albedo[p]);
                let (mut sum, mut wsum, mut vsum) = ([0.0f32; 3], 0.0f32, 0.0f32);
                for (ky, &hy) in KERNEL.iter().enumerate() {
                    let qy = y as isize + (ky as isize - 2) * step as isize;
                    if qy < 0 || qy >= h as isize {
                        continue;
                    }
                    for (kx, &hx) in KERNEL.iter().enumerate() {
                        let qx = x as isize + (kx as isize - 2) * step as isize;
                        if qx < 0 || qx >= w as isize {
                            continue;
                        }
                        let q = qy as usize * w + qx as usize;
                        if !g.valid[q] {
                            continue;
                        }
                        let nq = g.normal[q];
                        let wn = (np[0] * nq[0] + np[1] * nq[1] + np[2] * nq[2]).max(0.0).powi(NORMAL_POWER);
                        let dist = (((kx as isize - 2).pow(2) + (ky as isize - 2).pow(2)) as f32).sqrt() * step as f32;
                        let wz = (-(zp - g.depth[q]).abs() / (slope[p] * dist + 1e-3 * zp.abs() + 1e-5)).exp();
                        let aq = g.albedo[q];
                        let wa = (-((ap[0] - aq[0]).abs() + (ap[1] - aq[1]).abs() + (ap[2] - aq[2]).abs()) / SIGMA_ALBEDO).exp();
                        // The noisier of the two decides: a pixel whose few samples all
                        // missed the light (no variance) still blends with its noisy neighbours.
                        let sigma_l = SIGMA_LUM * var[p].max(var[q]).sqrt() + 1e-4;
                        let wl = (-(lp - lum(irr[q])).abs() / sigma_l).exp();
                        let wgt = hx * hy * wn * wz * wa * wl;
                        if wgt.is_nan() || wgt <= 0.0 {
                            continue;
                        }
                        for k in 0..3 {
                            sum[k] += irr[q][k] * wgt;
                        }
                        wsum += wgt;
                        vsum += wgt * wgt * var[q];
                    }
                }
                // The pixel itself always counts (its own weights are all 1), so wsum > 0.
                row[x] = if wsum > 0.0 { sum.map(|v| v / wsum) } else { irr[p] };
                vrow[x] = if wsum > 0.0 { vsum / (wsum * wsum) } else { var[p] };
            }
        });
        irr = next_irr;
        var = next_var;
    }
    (0..n)
        .map(|i| {
            if !g.valid[i] {
                return irr[i];
            }
            let a = g.albedo[i];
            [irr[i][0] * floor(a[0]), irr[i][1] * floor(a[1]), irr[i][2] * floor(a[2])]
        })
        .collect()
}

/// 3×3 Gaussian over the valid pixels.
fn blur3(w: usize, h: usize, v: &[f32], valid: &[bool]) -> Vec<f32> {
    let k = [0.25f32, 0.5, 0.25];
    (0..w * h)
        .map(|i| {
            if !valid[i] {
                return v[i];
            }
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let (mut s, mut ws) = (0.0, 0.0);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (qx, qy) = (x + dx, y + dy);
                    if qx < 0 || qy < 0 || qx >= w as isize || qy >= h as isize {
                        continue;
                    }
                    let q = qy as usize * w + qx as usize;
                    if valid[q] {
                        let wt = k[(dx + 1) as usize] * k[(dy + 1) as usize];
                        s += v[q] * wt;
                        ws += wt;
                    }
                }
            }
            if ws > 0.0 { s / ws } else { v[i] }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn f(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    fn variance(v: impl Iterator<Item = f32>) -> f32 {
        let v: Vec<f32> = v.collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32
    }

    /// A flat noisy picture with a step in it, either in the surface colour (`albedo_step`) or
    /// in the light (a shadow edge, same surface on both sides).
    fn case(albedo_step: bool) {
        let (w, h) = (64usize, 48usize);
        let mut r = Lcg(11);
        let noise = 0.15f32;
        let n = w * h;
        let mut color = vec![[0.0f32; 3]; n];
        let mut albedo = vec![[0.0f32; 3]; n];
        for i in 0..n {
            let left = i % w < w / 2;
            let a = if albedo_step && left { 0.2 } else { 0.8 };
            let light = if !albedo_step && left { 0.3 } else { 1.0 };
            // Uniform noise of variance noise²/3 per sample, 1 sample.
            let v = light * (1.0 + (r.f() * 2.0 - 1.0) * noise * 3f32.sqrt());
            color[i] = [a * v; 3];
            albedo[i] = [a; 3];
        }
        let normal = vec![[0.0, 0.0, 1.0]; n];
        let depth = vec![5.0; n];
        let variance_l: Vec<f32> = (0..n).map(|i| (lum(color[i]) * noise).powi(2)).collect();
        let valid = vec![true; n];
        let g = Guides { albedo: &albedo, normal: &normal, depth: &depth, variance: &variance_l, valid: &valid };
        let out = denoise(w, h, &color, &g);
        // Away from the edge, each half is much smoother.
        let half = |img: &[[f32; 3]], left: bool| variance((0..n).filter(|i| (i % w < w / 2 - 6 && left) || (i % w >= w / 2 + 6 && !left)).map(|i| img[i][0]));
        for left in [true, false] {
            let (before, after) = (half(&color, left), half(&out, left));
            assert!(after < before / 8.0, "step in albedo {albedo_step}, left {left}: variance {before} → {after}");
        }
        // Right at the edge, the columns keep their own side's level.
        let col = |img: &[[f32; 3]], x: usize| (0..h).map(|y| img[y * w + x][0]).sum::<f32>() / h as f32;
        let (lo, hi) = if albedo_step { (0.2, 0.8) } else { (0.24, 0.8) };
        let (l, rr) = (col(&out, w / 2 - 1), col(&out, w / 2));
        assert!((l - lo).abs() < 0.06 * hi, "left of the edge {l} (want {lo})");
        assert!((rr - hi).abs() < 0.06 * hi, "right of the edge {rr} (want {hi})");
    }

    #[test]
    fn smooths_noise_and_keeps_a_colour_edge() {
        case(true);
    }

    #[test]
    fn smooths_noise_and_keeps_a_shadow_edge() {
        case(false);
    }

    #[test]
    fn leaves_empty_pixels_and_odd_input_alone() {
        let color = vec![[1.0, 2.0, 3.0], [f32::NAN, 0.0, 0.0]];
        let valid = vec![false, true];
        let g = Guides { albedo: &[[0.5; 3]; 2], normal: &[[0.0, 1.0, 0.0]; 2], depth: &[1.0, 1.0], variance: &[0.0, f32::INFINITY], valid: &valid };
        let out = denoise(2, 1, &color, &g);
        assert_eq!(out[0], [1.0, 2.0, 3.0]);
        assert!(out[1].iter().all(|v| v.is_finite()));
        // Mismatched sizes: returned as is.
        let two = vec![[0.5; 3], [0.25; 3]];
        assert_eq!(denoise(3, 1, &two, &g), two);
    }
}
