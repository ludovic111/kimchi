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
    a[0].hypot(a[1]).hypot(a[2])
}
pub fn norm(a: V3) -> V3 {
    if a.iter().any(|v| !v.is_finite()) { return [0.; 3]; }
    let largest = a.iter().copied().map(f64::abs).fold(0., f64::max);
    if largest == 0. { return [0.; 3]; }
    let unit = a.map(|v| v / largest);
    scale(unit, 1. / len(unit))
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
    if a.iter().flatten().any(|v| !v.is_finite()) { return None; }
    // Equilibrate the rows before taking a determinant. Its magnitude must describe a
    // collapsed transform, not the units in which the scene happens to be modelled.
    let sizes = a.map(|row| row.into_iter().map(f64::abs).fold(0., f64::max));
    if sizes.contains(&0.) { return None; }
    let a: M3 = std::array::from_fn(|r| a[r].map(|v| v / sizes[r]));
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-15 {
        return None;
    }
    let k = 1.0 / det;
    let mut inverse = [
        [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) * k, (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * k, (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * k],
        [(a[1][2] * a[2][0] - a[1][0] * a[2][2]) * k, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * k, (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * k],
        [(a[1][0] * a[2][1] - a[1][1] * a[2][0]) * k, (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * k, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * k],
    ];
    for row in &mut inverse {
        for (c, value) in row.iter_mut().enumerate() { *value /= sizes[c]; }
    }
    inverse.iter().flatten().all(|v| v.is_finite()).then_some(inverse)
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
#[cfg(test)]
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
    if [o,d,a,b,c].iter().flatten().any(|v| !v.is_finite()) { return None; }
    let (e1, e2) = (sub(b, a), sub(c, a));
    let extent = e1.into_iter().chain(e2).map(f64::abs).fold(0., f64::max);
    if extent == 0. || !extent.is_finite() { return None; }
    let (e1,e2) = (e1.map(|v|v/extent),e2.map(|v|v/extent));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = (dot(s, p) * inv) / extent;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = (dot(d, q) * inv) / extent;
    if !v.is_finite() || v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t.is_finite() && t > 0.).then_some(t)
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
    if m.iter().any(|v| !v.is_finite()) { return None; }
    let inverse = m3_inverse(&[[m[0],m[2],0.],[m[1],m[3],0.],[0.,0.,1.]])?;
    let (a,b,c,d) = (inverse[0][0],inverse[1][0],inverse[0][1],inverse[1][1]);
    let result = [a,b,c,d,-(a*m[4]+c*m[5]),-(b*m[4]+d*m[5])];
    result.iter().all(|v|v.is_finite()).then_some(result)
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

/// Whether a selection rectangle intersects a convex layer outline, including edge
/// crossings and either shape containing the other. Tests axes from both shapes so the
/// empty corners around a rotated layer are excluded.
pub fn convex_intersects_rect(poly:&[[f64;2]],a:[f64;2],b:[f64;2])->bool {
    if poly.is_empty() || !poly.iter().flatten().chain(a.iter()).chain(b.iter()).all(|v|v.is_finite()) {return false;}
    let (lo,hi)=([a[0].min(b[0]),a[1].min(b[1])],[a[0].max(b[0]),a[1].max(b[1])]);
    let rect=[lo,[hi[0],lo[1]],hi,[lo[0],hi[1]]];
    let separated=|axis:[f64;2]| {
        let extent=axis[0].abs().max(axis[1].abs());
        if extent==0. {return false;}
        let axis=axis.map(|v|v/extent);
        let interval=|points:&[[f64;2]]|points.iter().fold((f64::INFINITY,f64::NEG_INFINITY),|(lo,hi),p| {
            let d=p[0]*axis[0]+p[1]*axis[1];(lo.min(d),hi.max(d))
        });
        let (p,r)=(interval(poly),interval(&rect));p.1<r.0 || r.1<p.0
    };
    !separated([1.,0.]) && !separated([0.,1.]) && !poly.iter().zip(poly.iter().cycle().skip(1)).any(|(a,b)|separated([a[1]-b[1],b[0]-a[0]]))
}

/// Snaps `v` to multiples of `step`.
pub fn snap(v: f64, step: f64) -> f64 {
    if step <= 0.0 { v } else { (v / step).round() * step }
}

/// A compact six-significant-digit readout, without rounding the value stored by a tool.
/// Scientific notation keeps tiny offsets visible and large amounts within the status bar.
pub fn compact_number(value: f64) -> String {
    if value == 0. { return "0".into(); }
    if !value.is_finite() { return value.to_string(); }
    let trim = |s: &str| if s.contains('.') {s.trim_end_matches('0').trim_end_matches('.').to_string()} else {s.to_string()};
    if !(1e-3..1e6).contains(&value.abs()) {
        let text=format!("{value:.5e}");
        let (mantissa,exponent)=text.split_once('e').unwrap_or((&text,"0"));
        format!("{}e{exponent}",trim(mantissa))
    } else {
        let places=(5-value.abs().log10().floor() as i32).max(0) as usize;
        trim(&format!("{value:.places$}"))
    }
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
    fn inverse_and_axes_do_not_depend_on_scene_units() {
        for size in [1e-200, 1e-8, 1., 1e100, 1e200] {
            let m = trs([0.; 3], [23., -41., 67.], [size, size * -2., size * 3.]);
            let back = mul(&inverse(&m), &m);
            for (c, column) in back.iter().enumerate() {
                for (r, value) in column.iter().enumerate() {
                    assert!((value - IDENTITY[c][r]).abs() < 1e-12, "scale {size}: {back:?}");
                }
            }
            let rotation = rotation_of(&m);
            let reference = rotation_of(&trs([0.; 3], [23., -41., 67.], [1., -2., 3.]));
            for (a, b) in rotation.into_iter().zip(reference) { assert!(close(a, b)); }
        }
        let mixed = [[1e-200, 0., 0.], [0., -1e200, 0.], [0., 0., 3.]];
        let back = m3_mul(&m3_inverse(&mixed).unwrap(), &mixed);
        assert!(close(back[0], [1., 0., 0.]));
        assert!(close(back[1], [0., 1., 0.]));
        assert!(close(back[2], [0., 0., 1.]));
    }

    #[test]
    fn inverse_rejects_collapsed_or_unrepresentable_transforms() {
        for a in [
            [[0.; 3]; 3],
            [[1., 2., 3.], [2., 4., 6.], [0., 0., 1.]],
            [[1., 0., 0.], [0., f64::INFINITY, 0.], [0., 0., 1.]],
            [[1., 0., 0.], [0., f64::NAN, 0.], [0., 0., 1.]],
            [[1e-320, 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ] { assert!(m3_inverse(&a).is_none(), "{a:?}"); }
        assert_eq!(norm([0.; 3]), [0.; 3]);
        assert_eq!(norm([f64::INFINITY, 0., 0.]), [0.; 3]);
        assert!(len([3e200, 4e200, 0.]).is_finite());
        assert!(close(norm([3e-200, 4e-200, 0.]), [0.6, 0.8, 0.]));
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
    fn triangle_picking_uses_relative_scale_and_rejects_invalid_rays() {
        for size in [1e-200, 1e-8, 1., 1e100, 1e200] {
            let (a,b,c)=([0.;3],[size,0.,0.],[0.,size,0.]);
            let origin=[size*0.25,size*0.25,size];
            let hit=ray_triangle(origin,[0.,0.,-1.],a,b,c).expect("visible triangle at any scale");
            assert!((hit/size-1.).abs()<1e-12);
            assert!(ray_triangle(origin,[0.,0.,-1.],a,c,b).is_some(),"back faces can be selected");
            assert!(ray_triangle([size*0.75,size*0.75,size],[0.,0.,-1.],a,b,c).is_none());
            assert!(ray_triangle(origin,[0.,0.,1.],a,b,c).is_none());
            assert!(ray_triangle(origin,[1.,0.,0.],a,b,c).is_none());
            assert!(ray_triangle(origin,[0.;3],a,b,c).is_none());
            assert!(ray_triangle(origin,[0.,0.,-1.],a,b,b).is_none());
        }
        assert!(ray_triangle([0.;3],[0.,0.,-1.],[0.,0.,f64::NEG_INFINITY],[1.,0.,0.],[0.,1.,0.]).is_none());
        assert!(ray_triangle([0.,0.,1.],[f64::NAN,0.,-1.],[0.;3],[1.,0.,0.],[0.,1.,0.]).is_none());
    }

    #[test]
    fn affine_inverse_preserves_small_layers_and_rejects_nonfinite_results() {
        for size in [1e-200,1e-8,1.,1e100,1e200] {
            let transform=layer_affine(0.,0.,37.,0.,size,-size*2.,0.,0.);
            let inverse=aff_invert(&transform).expect("finite invertible layer transform");
            let p=[0.3,-0.7];let back=aff_apply(&inverse,aff_apply(&transform,p));
            assert!((back[0]-p[0]).abs()<1e-12 && (back[1]-p[1]).abs()<1e-12,"{size}: {back:?}");
        }
        for invalid in [[0.;6],[1.,2.,2.,4.,0.,0.],[1.,0.,0.,f64::NAN,0.,0.],
            [1.,0.,0.,1.,f64::INFINITY,0.],[1e-200,0.,0.,1e-200,1e200,1e200]] {
            assert!(aff_invert(&invalid).is_none(),"{invalid:?}");
        }
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
