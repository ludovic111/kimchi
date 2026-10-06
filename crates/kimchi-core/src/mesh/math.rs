//! Small vector helpers on plain arrays (the mesh engine works in f64, right-handed, y up).

pub type V3 = [f64; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// `a + b * k`.
pub fn mad(a: V3, b: V3, k: f64) -> V3 {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn len(a: V3) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}

pub fn dist(a: V3, b: V3) -> f64 {
    len(sub(a, b))
}

pub fn lerp(a: V3, b: V3, t: f64) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Unit length, or `None` for zero or nonfinite vectors, independent of scene units.
pub fn try_norm(a: V3) -> Option<V3> {
    if !finite(a) {
        return None;
    }
    let magnitude = a.iter().map(|v| v.abs()).fold(0.0, f64::max);
    if magnitude == 0.0 {
        return None;
    }
    let scaled = a.map(|v| v / magnitude);
    Some(scale(scaled, 1.0 / len(scaled)))
}

/// Unit length; zero vectors stay zero.
pub fn norm(a: V3) -> V3 {
    try_norm(a).unwrap_or([0.0; 3])
}

pub fn finite(a: V3) -> bool {
    a.iter().all(|v| v.is_finite())
}

/// Any unit vector perpendicular to `n` (unit).
pub fn perpendicular(n: V3) -> V3 {
    let other = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    norm(cross(n, other))
}

/// `v` turned by `angle` radians around the unit `axis` (right-handed, Rodrigues).
pub fn rotate(v: V3, axis: V3, angle: f64) -> V3 {
    let (s, c) = angle.sin_cos();
    let k = axis;
    add(add(scale(v, c), scale(cross(k, v), s)), scale(k, dot(k, v) * (1.0 - c)))
}

/// A rotation matrix (rows) from Euler angles in degrees, applied x then y then z.
pub fn euler(deg: V3) -> [V3; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    // Rz * Ry * Rx
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

pub fn mat_mul(m: &[V3; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// The index of an axis name (`x`, `y`, `z`; anything else is x).
pub fn axis_index(name: &str) -> usize {
    match name {
        "y" => 1,
        "z" => 2,
        _ => 0,
    }
}

pub fn unit(axis: usize) -> V3 {
    let mut v = [0.0; 3];
    v[axis.min(2)] = 1.0;
    v
}

/// Newell's normal of a polygon (not normalised: its length is twice the area).
pub fn newell(points: impl Iterator<Item = V3> + Clone) -> V3 {
    let mut n = [0.0; 3];
    let mut it = points.clone();
    let Some(first) = it.next() else { return n };
    let mut prev = first;
    for p in it.chain(std::iter::once(first)) {
        n[0] += (prev[1] - p[1]) * (prev[2] + p[2]);
        n[1] += (prev[2] - p[2]) * (prev[0] + p[0]);
        n[2] += (prev[0] - p[0]) * (prev[1] + p[1]);
        prev = p;
    }
    n
}

/// Two unit vectors spanning the plane with unit normal `n` (right-handed: `u × v = n`).
pub fn plane_basis(n: V3) -> (V3, V3) {
    let u = perpendicular(n);
    (u, cross(n, u))
}

/// A deterministic hash of integers to 0..1.
pub fn hash01(parts: &[u64]) -> f64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    for &p in parts {
        h ^= p.wrapping_add(0x9E37_79B9_7F4A_7C15).wrapping_add(h << 6).wrapping_add(h >> 2);
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 31;
    }
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 29;
    (h >> 11) as f64 / (1u64 << 53) as f64
}
