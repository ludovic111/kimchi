//! Smooth, repeatable noise for the 2D engine: fractal noise and turbulent displacement
//! (effects), wiggle paths (shape operators) and wiggly text. The same inputs always give the
//! same values, so any frame can be drawn on its own.

/// A well-mixed 32-bit hash of a lattice point and a seed.
pub(crate) fn hash(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B9) ^ (x as u32).wrapping_mul(0x85EB_CA6B);
    h ^= (y as u32).wrapping_mul(0xC2B2_AE35);
    h ^= (z as u32).wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// 0..1 from a lattice point.
pub(crate) fn unit(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    (hash(x, y, z, seed) >> 8) as f32 / (1u32 << 24) as f32
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn grad(h: u32, x: f32, y: f32, z: f32) -> f32 {
    // Perlin's twelve edge directions.
    match h % 12 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 => -y + z,
        10 => y - z,
        _ => -y - z,
    }
}

/// Gradient noise in three dimensions, about −1..1, smooth everywhere.
pub(crate) fn noise3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return 0.0;
    }
    // Keep the lattice in i32 range for absurd inputs (the pattern just repeats).
    let wrap = |v: f32| v.rem_euclid(65536.0);
    let (x, y, z) = (wrap(x), wrap(y), wrap(z));
    let (xi, yi, zi) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let (xf, yf, zf) = (x - xi as f32, y - yi as f32, z - zi as f32);
    let (u, v, w) = (fade(xf), fade(yf), fade(zf));
    let g = |dx: i32, dy: i32, dz: i32| grad(hash(xi + dx, yi + dy, zi + dz, seed), xf - dx as f32, yf - dy as f32, zf - dz as f32);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = lerp(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = lerp(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = lerp(g(0, 1, 1), g(1, 1, 1), u);
    let n = lerp(lerp(x00, x10, v), lerp(x01, x11, v), w);
    // Gradient noise peaks a little under ±1; stretch it to use the range.
    (n * 1.1).clamp(-1.0, 1.0)
}

/// Noise along one axis (a value that wanders smoothly with `x`), about −1..1.
pub(crate) fn noise1(x: f32, seed: u32) -> f32 {
    noise3(x, 0.37, 0.71, seed)
}

/// Fractal noise: `octaves` layers of noise, each twice as fine and half as strong. About −1..1.
pub(crate) fn fbm(x: f32, y: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for o in 0..octaves.clamp(1, 10) {
        sum += noise3(x * f, y * f, z + o as f32 * 7.31, seed.wrapping_add(o * 101)) * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_smooth_bounded_and_repeatable() {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..2000 {
            let x = i as f32 * 0.173;
            let n = noise3(x, x * 0.5, 1.0, 7);
            lo = lo.min(n);
            hi = hi.max(n);
            assert!((noise3(x + 0.001, x * 0.5, 1.0, 7) - n).abs() < 0.02, "smooth");
            assert_eq!(n, noise3(x, x * 0.5, 1.0, 7));
        }
        assert!(lo < -0.4 && hi > 0.4 && lo >= -1.0 && hi <= 1.0, "{lo}..{hi}");
        assert_ne!(noise3(1.5, 2.5, 0.5, 1), noise3(1.5, 2.5, 0.5, 2), "seeds differ");
        assert_eq!(noise3(f32::NAN, 0.0, 0.0, 0), 0.0);
        assert!(fbm(1e9, -1e9, 3.0, 4, 0).is_finite());
    }
}
