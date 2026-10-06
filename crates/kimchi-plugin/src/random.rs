//! Randomness that repeats. A frame must come out the same every time it is drawn (the preview,
//! a second look at the same frame, the export), so plugins never use the system's random
//! numbers: they hash what the frame is (its number, a seed parameter, the pixel) into numbers
//! that look random.
//!
//! ```
//! use kimchi_plugin::random::{self, Rng};
//! # let ctx = kimchi_plugin::RenderCtx::new(1.0, 30.0);
//! // Per pixel: noise that changes every frame, the same in preview and export.
//! let seed = ctx.seed(7);
//! let n = random::unit(seed, 12, 34); // 0…1 for pixel (12, 34)
//! assert_eq!(n, random::unit(ctx.seed(7), 12, 34));
//! // A sequence: sparks, scratches, dust.
//! let mut rng = Rng::new(seed);
//! let (x, y) = (rng.unit(), rng.unit());
//! # let _ = (x, y);
//! ```

/// A 64-bit hash of `seed` and `value` (SplitMix64's finaliser): equal inputs, equal outputs,
/// and nearby inputs give unrelated outputs.
#[inline]
pub fn hash(seed: u64, value: u64) -> u64 {
    let mut z = seed ^ value.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A 32-bit hash of a seed and a pixel, cheap enough for every pixel of every frame.
#[inline]
pub fn hash2(seed: u64, x: u32, y: u32) -> u32 {
    // Two rounds of a 32-bit integer hash (Chris Wellons' "lowbias32") over the coordinates.
    let mut h = (seed as u32) ^ ((seed >> 32) as u32).rotate_left(16) ^ x.wrapping_mul(0x27d4_eb2d) ^ y.wrapping_mul(0x1656_67b1);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// 0…1 (1 excluded) for a pixel.
#[inline]
pub fn unit(seed: u64, x: u32, y: u32) -> f32 {
    (hash2(seed, x, y) >> 8) as f32 / (1u32 << 24) as f32
}

/// -1…1 for a pixel.
#[inline]
pub fn signed(seed: u64, x: u32, y: u32) -> f32 {
    unit(seed, x, y) * 2.0 - 1.0
}

/// Smooth noise at a point (value noise on a grid of `cell`-sized squares, smoothly
/// interpolated), 0…1. For clouds, flicker maps, wobble.
pub fn smooth(seed: u64, x: f32, y: f32, cell: f32) -> f32 {
    let cell = cell.max(1e-3);
    let (fx, fy) = (x / cell, y / cell);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (ix, iy) = (x0 as i64 as u32, y0 as i64 as u32);
    let v = |dx: u32, dy: u32| unit(seed, ix.wrapping_add(dx), iy.wrapping_add(dy));
    let top = v(0, 0) + (v(1, 0) - v(0, 0)) * sx;
    let bottom = v(0, 1) + (v(1, 1) - v(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// A small, fast generator for sequences (xoshiro-like quality is not needed for pictures):
/// SplitMix64.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        hash(self.0, 0)
    }

    /// 0…1 (1 excluded).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// `lo`…`hi`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// A normally distributed number (mean 0, deviation 1), Box–Muller.
    pub fn gauss(&mut self) -> f32 {
        let u = self.unit().max(1e-7);
        let v = self.unit();
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_and_spreads() {
        assert_eq!(hash2(5, 1, 2), hash2(5, 1, 2));
        assert_ne!(hash2(5, 1, 2), hash2(6, 1, 2));
        let mean: f32 = (0..10_000).map(|i| unit(3, i % 100, i / 100)).sum::<f32>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
        let mut a = Rng::new(9);
        let mut b = Rng::new(9);
        assert_eq!(a.unit(), b.unit());
        let g: f32 = (0..10_000).map(|_| a.gauss()).sum::<f32>() / 10_000.0;
        assert!(g.abs() < 0.05, "{g}");
        let s = smooth(1, 10.3, 4.2, 8.0);
        assert!((0.0..=1.0).contains(&s));
    }
}
