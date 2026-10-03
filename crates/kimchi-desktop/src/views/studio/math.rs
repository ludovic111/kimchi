//! Small vector and matrix maths for the Studio's tools: the transform gizmo, picking, the 2D
//! handles. 3D matrices follow the renderer's conventions (column-major, `m[column][row]`,
//! rotations applied x, then y, then z); 2D affine transforms are `[a, b, c, d, e, f]` with
//! x' = a·x + c·y + e, y' = b·x + d·y + f (tiny-skia's order).

pub type V3 = [f64; 3];
pub type M4 = [[f64; 4]; 4];
pub type M3 = [[f64; 3]; 3];
pub type Affine = [f64; 6];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-12 { [0.0, 0.0, 0.0] } else { scale(a, 1.0 / l) }
}
pub fn lerp(a: V3, b: V3, t: f64) -> V3 {
    add(a, scale(sub(b, a), t))
}

pub const IDENTITY: M4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// `a × b` (apply `b` first).
pub fn mul(a: &M4, b: &M4) -> M4 {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

pub fn point(m: &M4, p: V3) -> V3 {
    let w = m[0][3] * p[0] + m[1][3] * p[1] + m[2][3] * p[2] + m[3][3];
    let w = if w.abs() < 1e-12 { 1.0 } else { w };
    [
        (m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0]) / w,
        (m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1]) / w,
        (m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2]) / w,
    ]
}

pub fn dir(m: &M4, d: V3) -> V3 {
    [m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2], m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2], m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2]]
}

pub fn origin(m: &M4) -> V3 {
    [m[3][0], m[3][1], m[3][2]]
}

/// The matrix's three axes (columns), not normalised.
pub fn axes(m: &M4) -> [V3; 3] {
    [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]]
}

pub fn translate(t: V3) -> M4 {
    let mut m = IDENTITY;
    m[3] = [t[0], t[1], t[2], 1.0];
    m
}

pub fn scaling(s: V3) -> M4 {
    let mut m = IDENTITY;
    m[0][0] = s[0];
    m[1][1] = s[1];
    m[2][2] = s[2];
    m
}

fn m3_to_m4(r: &M3) -> M4 {
    let mut m = IDENTITY;
    for c in 0..3 {
        for row in 0..3 {
            m[c][row] = r[row][c];
        }
    }
    m
}

/// Translation, rotation (degrees, x then y then z) and scale, like an object's own transform.
pub fn trs(t: V3, r_deg: V3, s: V3) -> M4 {
    mul(&translate(t), &mul(&m3_to_m4(&euler_to_m3(r_deg)), &scaling(s)))
}

/// Inverse of an affine 4×4 (rotation, scale, translation); identity when singular.
pub fn inverse(m: &M4) -> M4 {
    // 3×3 part, row-major.
    let a = [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]];
    let Some(inv) = m3_inverse(&a) else { return IDENTITY };
    let t = origin(m);
    let it = [
        -(inv[0][0] * t[0] + inv[0][1] * t[1] + inv[0][2] * t[2]),
        -(inv[1][0] * t[0] + inv[1][1] * t[1] + inv[1][2] * t[2]),
        -(inv[2][0] * t[0] + inv[2][1] * t[1] + inv[2][2] * t[2]),
    ];
    let mut out = m3_to_m4(&inv);
    out[3] = [it[0], it[1], it[2], 1.0];
    out
}

// ---- 3×3 rotations (row-major: r[row][col]) -----------------------------------------------

pub fn m3_mul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    out
}

pub fn m3_transpose(a: &M3) -> M3 {
    [[a[0][0], a[1][0], a[2][0]], [a[0][1], a[1][1], a[2][1]], [a[0][2], a[1][2], a[2][2]]]
}

pub fn m3_inverse(a: &M3) -> Option<M3> {
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-15 {
        return None;
    }
    let k = 1.0 / det;
    Some([
        [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) * k, (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * k, (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * k],
        [(a[1][2] * a[2][0] - a[1][0] * a[2][2]) * k, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * k, (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * k],
        [(a[1][0] * a[2][1] - a[1][1] * a[2][0]) * k, (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * k, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * k],
    ])
}

pub fn m3_apply(a: &M3, v: V3) -> V3 {
    [a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2], a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2], a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2]]
}

/// Rotation by `deg` around a unit `axis`.
pub fn axis_angle(axis: V3, deg: f64) -> M3 {
    let a = norm(axis);
    let (s, c) = deg.to_radians().sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = a;
    [[t * x * x + c, t * x * y - s * z, t * x * z + s * y], [t * x * y + s * z, t * y * y + c, t * y * z - s * x], [t * x * z - s * y, t * y * z + s * x, t * z * z + c]]
}

/// Euler degrees (x, then y, then z: R = Rz·Ry·Rx) to a rotation.
pub fn euler_to_m3(r: V3) -> M3 {
    let rx = axis_angle([1.0, 0.0, 0.0], r[0]);
    let ry = axis_angle([0.0, 1.0, 0.0], r[1]);
    let rz = axis_angle([0.0, 0.0, 1.0], r[2]);
    m3_mul(&rz, &m3_mul(&ry, &rx))
}

/// A rotation back to Euler degrees (x, then y, then z), choosing the one closest to `near`
/// so values don't jump by 360° while dragging.
pub fn m3_to_euler(m: &M3, near: V3) -> V3 {
    // R = Rz·Ry·Rx: m[2][0] = −sin(y).
    let sy = (-m[2][0]).clamp(-1.0, 1.0);
    let y = sy.asin();
    let (x, z) = if sy.abs() < 0.999_999 {
        (m[2][1].atan2(m[2][2]), m[1][0].atan2(m[0][0]))
    } else {
        // Gimbal lock: put everything in x.
        ((-m[1][2]).atan2(m[1][1]), 0.0)
    };
    let out = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
    let mut best = out;
    // The same rotation also as (x+180, 180−y, z+180).
    let alt = [out[0] + 180.0, 180.0 - out[1], out[2] + 180.0];
    let dist = |a: V3| (0..3).map(|i| wrap_near(a[i], near[i]) - near[i]).map(|d| d * d).sum::<f64>();
    if dist(alt) < dist(best) {
        best = alt;
    }
    [wrap_near(best[0], near[0]), wrap_near(best[1], near[1]), wrap_near(best[2], near[2])]
}

/// `a` plus or minus whole turns, as close as possible to `near`.
pub fn wrap_near(a: f64, near: f64) -> f64 {
    a + ((near - a) / 360.0).round() * 360.0
}

/// The rotation part of a world matrix, with scale removed (row-major 3×3).
pub fn rotation_of(m: &M4) -> M3 {
    let [x, y, z] = axes(m);
    let (x, y, z) = (norm(x), norm(y), norm(z));
    [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
}

// ---- rays -----------------------------------------------------------------------------------

/// Where a ray meets a plane (point, normal), if in front.
pub fn ray_plane(o: V3, d: V3, p: V3, n: V3) -> Option<V3> {
    let den = dot(d, n);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = dot(sub(p, o), n) / den;
    (t > 0.0).then(|| add(o, scale(d, t)))
}

/// The parameter along the line (`p`, unit `axis`) closest to the ray.
pub fn ray_line(o: V3, d: V3, p: V3, axis: V3) -> Option<f64> {
    let w = sub(o, p);
    let (a, b, c) = (dot(axis, axis), dot(axis, d), dot(d, d));
    let (dd, e) = (dot(axis, w), dot(d, w));
    let den = a * c - b * b;
    if den.abs() < 1e-9 {
        return None;
    }
    Some((dd * c - b * e) / den)
}

/// Ray against an axis-aligned box: the distance to the first hit.
pub fn ray_box(o: V3, d: V3, lo: V3, hi: V3) -> Option<f64> {
    let (mut t0, mut t1) = (0.0f64, f64::INFINITY);
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return None;
            }
            continue;
        }
        let (mut a, mut b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// Ray against a triangle (Möller–Trumbore): the distance.
pub fn ray_triangle(o: V3, d: V3, a: V3, b: V3, c: V3) -> Option<f64> {
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 1e-9).then_some(t)
}

// ---- 2D -------------------------------------------------------------------------------------

pub const AFFINE_ID: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` after `b` (apply `b` first).
pub fn aff_mul(a: &Affine, b: &Affine) -> Affine {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

pub fn aff_apply(m: &Affine, p: [f64; 2]) -> [f64; 2] {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

pub fn aff_invert(m: &Affine) -> Option<Affine> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let k = 1.0 / det;
    let (a, b, c, d) = (m[3] * k, -m[1] * k, -m[2] * k, m[0] * k);
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

/// The linear part applied to a direction.
pub fn aff_dir(m: &Affine, v: [f64; 2]) -> [f64; 2] {
    [m[0] * v[0] + m[2] * v[1], m[1] * v[0] + m[3] * v[1]]
}

/// A 2D layer's own transform: position, rotation (degrees, clockwise with y down), skew,
/// scale, then the anchor.
#[allow(clippy::too_many_arguments)]
pub fn layer_affine(x: f64, y: f64, rotation: f64, skew_x: f64, sx: f64, sy: f64, ax: f64, ay: f64) -> Affine {
    let (s, c) = rotation.to_radians().sin_cos();
    let rot = [c, s, -s, c, 0.0, 0.0];
    let skew = [1.0, 0.0, skew_x.clamp(-89.0, 89.0).to_radians().tan(), 1.0, 0.0, 0.0];
    let m = aff_mul(&[1.0, 0.0, 0.0, 1.0, x, y], &rot);
    let m = aff_mul(&m, &skew);
    let m = aff_mul(&m, &[sx, 0.0, 0.0, sy, 0.0, 0.0]);
    aff_mul(&m, &[1.0, 0.0, 0.0, 1.0, -ax, -ay])
}

/// Distance from `p` to the segment `a`–`b` (screen pixels).
pub fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-12 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) };
    let (qx, qy) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (qx * qx + qy * qy).sqrt()
}

/// Is `p` inside the polygon (even-odd)?
pub fn in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// Snaps `v` to multiples of `step`.
pub fn snap(v: f64, step: f64) -> f64 {
    if step <= 0.0 { v } else { (v / step).round() * step }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6)
    }

    #[test]
    fn euler_round_trips_near_the_old_angles() {
        for r in [[10.0, 20.0, 30.0], [-45.0, 80.0, 170.0], [0.0, 0.0, 0.0], [370.0, 5.0, -720.0]] {
            let m = euler_to_m3(r);
            let back = m3_to_euler(&m, r);
            assert!(close(back, r), "{r:?} came back as {back:?}");
        }
    }

    #[test]
    fn matrices_invert_and_compose() {
        let m = trs([1.0, 2.0, 3.0], [30.0, 40.0, 50.0], [2.0, 2.0, 2.0]);
        let back = mul(&inverse(&m), &m);
        let p = [0.3, -0.7, 1.1];
        assert!(close(point(&back, p), p));
        assert!(close(origin(&m), [1.0, 2.0, 3.0]));
        // Rotating x by 90° around z gives y.
        let r = axis_angle([0.0, 0.0, 1.0], 90.0);
        assert!(close(m3_apply(&r, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn rays_meet_lines_planes_and_boxes() {
        let hit = ray_plane([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]).unwrap();
        assert!(close(hit, [0.0, 0.0, 0.0]));
        let t = ray_line([2.0, 1.0, 5.0], [0.0, 0.0, -1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]).unwrap();
        assert!((t - 2.0).abs() < 1e-9);
        assert!(ray_box([0.0, 0.0, 5.0], [0.0, 0.0, -1.0], [-1.0; 3], [1.0; 3]).is_some_and(|t| (t - 4.0).abs() < 1e-9));
        assert!(ray_box([3.0, 0.0, 5.0], [0.0, 0.0, -1.0], [-1.0; 3], [1.0; 3]).is_none());
    }

    #[test]
    fn affine_layer_transforms_invert() {
        let m = layer_affine(100.0, 50.0, 30.0, 0.0, 2.0, 1.0, 10.0, 0.0);
        let inv = aff_invert(&m).unwrap();
        let p = aff_apply(&inv, aff_apply(&m, [3.0, 4.0]));
        assert!((p[0] - 3.0).abs() < 1e-9 && (p[1] - 4.0).abs() < 1e-9);
        // The anchor lands on the position.
        let a = aff_apply(&m, [10.0, 0.0]);
        assert!((a[0] - 100.0).abs() < 1e-9 && (a[1] - 50.0).abs() < 1e-9);
        assert!(in_polygon([0.5, 0.5], &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]));
        assert!((seg_dist([0.0, 1.0], [-1.0, 0.0], [1.0, 0.0]) - 1.0).abs() < 1e-9);
    }
}
