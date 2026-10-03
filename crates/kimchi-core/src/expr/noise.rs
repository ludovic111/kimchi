//! Smooth noise: gradient (Perlin) noise in one, two or three dimensions, from a hash rather than
//! a table, so it is the same everywhere and needs no setup. Values are −1..1 and change
//! smoothly; whole-number inputs give 0.

use super::mix;

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lattice(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    mix(mix(mix(seed, x as u64), y as u64), z as u64)
}

/// One-dimensional noise, −1..1 (the gradients at whole numbers are −1..1 slopes).
pub(super) fn noise1(x: f64, seed: u64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    let i = x.floor();
    let f = x - i;
    let i = i as i64;
    let g = |k: i64| (lattice(seed, i + k, 0, 0) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
    let (a, b) = (g(0) * f, g(1) * (f - 1.0));
    // The largest value is 0.5 (at f = ½ with opposite unit slopes): scale to −1..1.
    (lerp(a, b, fade(f)) * 2.0).clamp(-1.0, 1.0)
}

/// The 12 edge directions of a cube (Perlin's improved noise).
const GRAD: [[f64; 3]; 12] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
];

/// Three-dimensional noise, about −1..1.
pub(super) fn noise3(x: f64, y: f64, z: f64, seed: u64) -> f64 {
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return 0.0;
    }
    let (fx, fy, fz) = (x.floor(), y.floor(), z.floor());
    let (ix, iy, iz) = (fx as i64, fy as i64, fz as i64);
    let (x, y, z) = (x - fx, y - fy, z - fz);
    let dot = |dx: i64, dy: i64, dz: i64| {
        let g = GRAD[(lattice(seed, ix + dx, iy + dy, iz + dz) % 12) as usize];
        g[0] * (x - dx as f64) + g[1] * (y - dy as f64) + g[2] * (z - dz as f64)
    };
    let (u, v, w) = (fade(x), fade(y), fade(z));
    let x00 = lerp(dot(0, 0, 0), dot(1, 0, 0), u);
    let x10 = lerp(dot(0, 1, 0), dot(1, 1, 0), u);
    let x01 = lerp(dot(0, 0, 1), dot(1, 0, 1), u);
    let x11 = lerp(dot(0, 1, 1), dot(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_smooth_bounded_and_repeatable() {
        let mut prev = noise1(0.0, 7);
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for i in 1..4000 {
            let x = i as f64 * 0.01;
            let n = noise1(x, 7);
            assert!((n - prev).abs() < 0.05, "smooth at {x}");
            lo = lo.min(n);
            hi = hi.max(n);
            prev = n;
            assert_eq!(n, noise1(x, 7));
            let m = noise3(x, x * 0.7, -x, 3);
            assert!((-1.0..=1.0).contains(&m));
        }
        assert!(lo < -0.4 && hi > 0.4, "uses its range: {lo} {hi}");
        assert_eq!(noise1(3.0, 1), 0.0);
        assert_ne!(noise1(0.5, 1), noise1(0.5, 2), "seeds differ");
    }
}
