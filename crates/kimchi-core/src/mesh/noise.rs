//! Smooth noise for displacement: 4D gradient (Perlin) noise, the fourth dimension being
//! "evolution", so animating it makes a surface churn instead of sliding.

fn hash(ix: i64, iy: i64, iz: i64, iw: i64, seed: u64) -> u64 {
    let mut h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD6E8_FEB8_6659_FD93;
    for v in [ix, iy, iz, iw] {
        h ^= (v as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = h.rotate_left(27).wrapping_mul(0x94D0_49BB_1331_11EB);
    }
    h ^= h >> 31;
    h
}

/// The 32 gradients of 4D Perlin noise: permutations of (0, ±1, ±1, ±1).
fn grad(h: u64, x: f64, y: f64, z: f64, w: f64) -> f64 {
    let h = (h & 31) as u32;
    let (a, b, c) = match h >> 3 {
        0 => (y, z, w),
        1 => (x, z, w),
        2 => (x, y, w),
        _ => (x, y, z),
    };
    let a = if h & 4 == 0 { a } else { -a };
    let b = if h & 2 == 0 { b } else { -b };
    let c = if h & 1 == 0 { c } else { -c };
    a + b + c
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Gradient noise at a 4D point, roughly in −1..1, smooth everywhere.
pub fn noise4(p: [f64; 4], seed: u64) -> f64 {
    if !p.iter().all(|v| v.is_finite()) {
        return 0.0;
    }
    let p = p.map(|v| v.clamp(-1e12, 1e12));
    let i = p.map(|v| v.floor());
    let f: [f64; 4] = std::array::from_fn(|k| p[k] - i[k]);
    let i = i.map(|v| v as i64);
    let u = f.map(fade);
    let mut acc = [0.0f64; 16];
    for (c, a) in acc.iter_mut().enumerate() {
        let o = [c & 1, (c >> 1) & 1, (c >> 2) & 1, (c >> 3) & 1];
        let h = hash(i[0] + o[0] as i64, i[1] + o[1] as i64, i[2] + o[2] as i64, i[3] + o[3] as i64, seed);
        *a = grad(h, f[0] - o[0] as f64, f[1] - o[1] as f64, f[2] - o[2] as f64, f[3] - o[3] as f64);
    }
    // Blend along x, then y, z, w.
    let mut n = 16;
    let mut level = 0;
    while n > 1 {
        n /= 2;
        for k in 0..n {
            acc[k] = acc[2 * k] + (acc[2 * k + 1] - acc[2 * k]) * u[level];
        }
        level += 1;
    }
    (acc[0] * 0.6).clamp(-1.0, 1.0)
}

/// Layers of finer noise (`octaves`), each half as strong, normalised to about −1..1.
pub fn fbm(p: [f64; 3], w: f64, octaves: usize, seed: u64) -> f64 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut total = 0.0;
    let mut freq = 1.0;
    for o in 0..octaves.clamp(1, 8) {
        sum += amp * noise4([p[0] * freq, p[1] * freq, p[2] * freq, w], seed.wrapping_add(o as u64 * 1013));
        total += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if total > 0.0 { sum / total } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_bounded_and_seeded() {
        let mut lo: f64 = 0.0;
        let mut hi: f64 = 0.0;
        for i in 0..2000 {
            let p = [i as f64 * 0.137, i as f64 * 0.071, i as f64 * 0.013, 0.3];
            let v = noise4(p, 1);
            lo = lo.min(v);
            hi = hi.max(v);
            // Continuity: a tiny step changes it a little.
            let q = [p[0] + 1e-4, p[1], p[2], p[3]];
            assert!((noise4(q, 1) - v).abs() < 1e-2);
        }
        assert!(lo < -0.2 && hi > 0.2, "{lo} {hi}");
        assert_eq!(noise4([0.5, 0.5, 0.5, 0.5], 3), noise4([0.5, 0.5, 0.5, 0.5], 3));
        assert_ne!(noise4([0.5, 0.25, 0.5, 0.5], 3), noise4([0.5, 0.25, 0.5, 0.5], 4));
        assert_eq!(noise4([f64::NAN, 0.0, 0.0, 0.0], 0), 0.0);
    }
}
