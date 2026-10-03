//! The polygons of every primitive shape, sized like the 3D renderer always drew them: a box
//! centred with `size`, spheres by `radius`, cylinders and cones standing on y and centred,
//! the torus lying in the xz plane, planes facing +z, grids lying flat (y up).

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use crate::motion::Shape3d;
use crate::motion::curve::Polyline;

use super::math::*;
use super::{DEFAULT_SMOOTH_ANGLE, MAX_FACES, PolyMesh, weld_map};

/// Segments around for round shapes when none are asked for.
const SMOOTH_SEGMENTS: usize = 48;

pub(super) fn build(shape: &Shape3d) -> Option<PolyMesh> {
    let m = match shape {
        Shape3d::Box { size, bevel } => cube(size.0, *bevel),
        Shape3d::Sphere { radius, segments } => sphere(*radius, segments_or(*segments, SMOOTH_SEGMENTS)),
        Shape3d::Icosphere { radius, detail } => icosphere(*radius, *detail),
        Shape3d::Cylinder { radius, height, segments } => cylinder(*radius, *radius, *height, segments_or(*segments, SMOOTH_SEGMENTS)),
        Shape3d::Cone { radius, height, segments } => cylinder(*radius, 0.0, *height, segments_or(*segments, SMOOTH_SEGMENTS)),
        Shape3d::Capsule { radius, height } => capsule(*radius, *height),
        Shape3d::Torus { radius, tube } => torus(*radius, *tube),
        Shape3d::Plane { width, height } => plane(*width, *height),
        Shape3d::Grid { width, height, rows, cols } => grid(*width, *height, *rows, *cols),
        Shape3d::Extrude { d, size, depth, bevel } => super::extrude::extrude(d, *size, *depth, *bevel),
        Shape3d::Lathe { profile, segments, angle } => lathe(profile, *segments, *angle),
        Shape3d::Curve { points, closed, smooth, radius, sides, trim_start, trim_end } => {
            if !radius.is_finite() || *radius <= 0.0 {
                return None;
            }
            tube(points, *closed, *smooth, *radius, *sides, *trim_start, *trim_end)
        }
        Shape3d::Mesh { vertices, faces, uvs, auto_smooth } => {
            let mut m = PolyMesh {
                positions: vertices.clone(),
                faces: faces.clone(),
                uvs: (!uvs.is_empty()).then(|| uvs.clone()),
                smooth_angle: *auto_smooth,
            };
            m.sanitize();
            m
        }
        Shape3d::Text { .. } | Shape3d::Particles(_) | Shape3d::Model { .. } | Shape3d::Image { .. } | Shape3d::Group {} => return None,
    };
    Some(m)
}

fn segments_or(s: f64, default: usize) -> usize {
    if s.is_finite() && s >= 0.5 { (s.round() as usize).clamp(3, 1024) } else { default }
}

fn ok(v: f64) -> f64 {
    if v.is_finite() { v.abs() } else { 0.0 }
}

/// Collects vertices and faces with texture coordinates.
struct Builder {
    m: PolyMesh,
}

impl Builder {
    fn new() -> Builder {
        Builder { m: PolyMesh { uvs: Some(vec![]), smooth_angle: DEFAULT_SMOOTH_ANGLE, ..Default::default() } }
    }

    fn v(&mut self, p: V3) -> u32 {
        self.m.positions.push(p);
        (self.m.positions.len() - 1) as u32
    }

    fn face(&mut self, f: Vec<u32>, uv: Vec<[f64; 2]>) {
        self.m.faces.push(f);
        if let Some(u) = self.m.uvs.as_mut() {
            u.push(uv);
        }
    }

    /// Welds points that coincide (seams, poles, zero sizes) and drops faces that collapsed.
    fn finish(mut self, smooth_angle: f64) -> PolyMesh {
        let tol = (self.m.size() * 1e-9).max(1e-12);
        let map = weld_map(&self.m.positions, tol);
        self.m.remap(&map);
        self.m.compact();
        self.m.smooth_angle = smooth_angle;
        self.m
    }
}

/// A box, centred; `bevel` rounds its edges (world units, at most half the smallest side).
pub(super) fn cube(size: V3, bevel: f64) -> PolyMesh {
    let h = size.map(|s| ok(s) / 2.0);
    let r = ok(bevel).min(h[0]).min(h[1]).min(h[2]);
    // (normal axis, its sign, u axis, u sign, v axis, v sign): u runs left to right and v top to
    // bottom seen from outside, so every side shows a picture the right way up.
    const SIDES: [(usize, f64, usize, f64, usize, f64); 6] =
        [(0, 1.0, 2, -1.0, 1, -1.0), (0, -1.0, 2, 1.0, 1, -1.0), (1, 1.0, 0, 1.0, 2, 1.0), (1, -1.0, 0, 1.0, 2, -1.0), (2, 1.0, 0, 1.0, 1, -1.0), (2, -1.0, 0, -1.0, 1, -1.0)];
    let mut b = Builder::new();
    // Sample positions across each axis: the corners only, or dense across the rounded bands.
    const BAND: usize = 4;
    let axis_values = |half: f64| -> Vec<f64> {
        if r < 1e-6 {
            return vec![-half, half];
        }
        let inner = half - r;
        let mut v: Vec<f64> = (0..=BAND).map(|k| -inner - r * (FRAC_PI_2 * k as f64 / BAND as f64).cos()).collect();
        v.extend((0..=BAND).map(|k| inner + r * (FRAC_PI_2 * k as f64 / BAND as f64).sin()));
        v
    };
    let values: [Vec<f64>; 3] = [axis_values(h[0]), axis_values(h[1]), axis_values(h[2])];
    let project = |p: V3| -> V3 {
        if r < 1e-6 {
            return p;
        }
        let inner: V3 = std::array::from_fn(|k| p[k].clamp(-(h[k] - r), h[k] - r));
        let d = sub(p, inner);
        match try_norm(d) {
            Some(n) => mad(inner, n, r),
            None => p,
        }
    };
    for (axis, sign, ua, us, va, vs) in SIDES {
        let (uvals, vvals) = (&values[ua], &values[va]);
        let mut ids = vec![vec![0u32; vvals.len()]; uvals.len()];
        let mut uv = vec![vec![[0.0; 2]; vvals.len()]; uvals.len()];
        for (i, &u) in uvals.iter().enumerate() {
            for (j, &v) in vvals.iter().enumerate() {
                let mut p = [0.0; 3];
                p[axis] = sign * h[axis];
                p[ua] = us * u;
                p[va] = vs * v;
                ids[i][j] = b.v(project(p));
                let fu = if h[ua] > 0.0 { (u / h[ua] + 1.0) / 2.0 } else { 0.5 };
                let fv = if h[va] > 0.0 { (v / h[va] + 1.0) / 2.0 } else { 0.5 };
                uv[i][j] = [fu, fv];
            }
        }
        for i in 0..uvals.len() - 1 {
            for j in 0..vvals.len() - 1 {
                b.face(vec![ids[i][j], ids[i][j + 1], ids[i + 1][j + 1], ids[i + 1][j]], vec![uv[i][j], uv[i][j + 1], uv[i + 1][j + 1], uv[i + 1][j]]);
            }
        }
    }
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// A UV sphere: `segments` around, half as many rings, single vertices at the poles.
pub(super) fn sphere(radius: f64, segments: usize) -> PolyMesh {
    let r = ok(radius);
    let seg = segments.clamp(3, 1024);
    let rings = (seg / 2).max(2);
    let mut b = Builder::new();
    let at = |k: usize, j: usize| -> V3 {
        let (theta, phi) = (k as f64 / seg as f64 * TAU, j as f64 / rings as f64 * PI);
        [r * phi.sin() * theta.sin(), r * phi.cos(), r * phi.sin() * theta.cos()]
    };
    let top = b.v([0.0, r, 0.0]);
    let mut rows = vec![];
    for j in 1..rings {
        rows.push((0..seg).map(|k| b.v(at(k, j))).collect::<Vec<u32>>());
    }
    let bottom = b.v([0.0, -r, 0.0]);
    let (fs, fr) = (seg as f64, rings as f64);
    for k in 0..seg {
        let k1 = (k + 1) % seg;
        let (u0, u1, um) = (k as f64 / fs, (k + 1) as f64 / fs, (k as f64 + 0.5) / fs);
        b.face(vec![top, rows[0][k], rows[0][k1]], vec![[um, 0.0], [u0, 1.0 / fr], [u1, 1.0 / fr]]);
        for j in 0..rows.len() - 1 {
            let (v0, v1) = ((j + 1) as f64 / fr, (j + 2) as f64 / fr);
            b.face(vec![rows[j][k], rows[j + 1][k], rows[j + 1][k1], rows[j][k1]], vec![[u0, v0], [u0, v1], [u1, v1], [u1, v0]]);
        }
        let last = rows.len() - 1;
        let v0 = (rings - 1) as f64 / fr;
        b.face(vec![rows[last][k], bottom, rows[last][k1]], vec![[u0, v0], [um, 1.0], [u1, v0]]);
    }
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// An icosahedron split `detail` times (each triangle into four), on a sphere.
pub(super) fn icosphere(radius: f64, detail: f64) -> PolyMesh {
    let r = ok(radius);
    let detail = if detail.is_finite() { detail.round().clamp(0.0, 6.0) as usize } else { 2 };
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut pos: Vec<V3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| norm(*p))
    .collect();
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for f in &mut faces {
        let (a, b, c) = (pos[f[0] as usize], pos[f[1] as usize], pos[f[2] as usize]);
        if dot(cross(sub(b, a), sub(c, a)), add(add(a, b), c)) < 0.0 {
            f.swap(1, 2);
        }
    }
    for _ in 0..detail {
        let mut mid: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        let mut next = Vec::with_capacity(faces.len() * 4);
        let mut midpoint = |a: u32, b: u32, pos: &mut Vec<V3>| -> u32 {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                pos.push(norm(lerp(pos[a as usize], pos[b as usize], 0.5)));
                (pos.len() - 1) as u32
            })
        };
        for f in &faces {
            let ab = midpoint(f[0], f[1], &mut pos);
            let bc = midpoint(f[1], f[2], &mut pos);
            let ca = midpoint(f[2], f[0], &mut pos);
            next.extend([[f[0], ab, ca], [f[1], bc, ab], [f[2], ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    let mut b = Builder::new();
    for p in &pos {
        b.v(scale(*p, r));
    }
    for f in faces {
        let uv = sphere_uvs(&[pos[f[0] as usize], pos[f[1] as usize], pos[f[2] as usize]]);
        b.face(f.to_vec(), uv);
    }
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// Spherical texture coordinates for one face's corners (unit directions), mended across the
/// seam behind (−z) and at the poles.
pub(crate) fn sphere_uvs(dirs: &[V3]) -> Vec<[f64; 2]> {
    let mut uv: Vec<[f64; 2]> = dirs
        .iter()
        .map(|d| {
            let d = norm(*d);
            let u = d[0].atan2(d[2]) / TAU;
            let u = if u < 0.0 { u + 1.0 } else { u };
            [u, d[1].clamp(-1.0, 1.0).acos() / PI]
        })
        .collect();
    mend_seam(&mut uv, dirs.iter().map(|d| norm(*d)[1].abs() > 1.0 - 1e-9).collect());
    uv
}

/// Corners straddling the u = 0/1 seam get u + 1 so the face doesn't stretch across the whole
/// picture; corners on the axis (`pole`) take the others' average u.
pub(crate) fn mend_seam(uv: &mut [[f64; 2]], pole: Vec<bool>) {
    let us: Vec<f64> = uv.iter().zip(&pole).filter(|(_, p)| !**p).map(|(c, _)| c[0]).collect();
    if us.is_empty() {
        return;
    }
    let (lo, hi) = us.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &u| (a.min(u), b.max(u)));
    if hi - lo > 0.5 {
        for (c, p) in uv.iter_mut().zip(&pole) {
            if !p && c[0] < 0.5 {
                c[0] += 1.0;
            }
        }
    }
    let others: Vec<f64> = uv.iter().zip(&pole).filter(|(_, p)| !**p).map(|(c, _)| c[0]).collect();
    let avg = others.iter().sum::<f64>() / others.len() as f64;
    for (c, p) in uv.iter_mut().zip(&pole) {
        if *p {
            c[0] = avg;
        }
    }
}

/// A cylinder standing on y, centred (`top` 0 makes a cone), with flat caps.
pub(super) fn cylinder(bottom: f64, top: f64, height: f64, segments: usize) -> PolyMesh {
    let (rb, rt, hh) = (ok(bottom), ok(top), ok(height) / 2.0);
    let seg = segments.clamp(3, 1024);
    let mut b = Builder::new();
    let ring = |b: &mut Builder, r: f64, y: f64| -> Vec<u32> {
        if r <= 0.0 {
            return vec![b.v([0.0, y, 0.0])];
        }
        (0..seg)
            .map(|k| {
                let a = k as f64 / seg as f64 * TAU;
                b.v([r * a.sin(), y, r * a.cos()])
            })
            .collect()
    };
    let topr = ring(&mut b, rt, hh);
    let botr = ring(&mut b, rb, -hh);
    let at = |r: &Vec<u32>, k: usize| r[if r.len() == 1 { 0 } else { k % seg }];
    let fs = seg as f64;
    for k in 0..seg {
        let (u0, u1) = (k as f64 / fs, (k + 1) as f64 / fs);
        let um = (u0 + u1) / 2.0;
        match (topr.len() == 1, botr.len() == 1) {
            (true, true) => {}
            (true, false) => b.face(vec![topr[0], at(&botr, k), at(&botr, k + 1)], vec![[um, 0.0], [u0, 1.0], [u1, 1.0]]),
            (false, true) => b.face(vec![at(&topr, k), botr[0], at(&topr, k + 1)], vec![[u0, 0.0], [um, 1.0], [u1, 0.0]]),
            (false, false) => b.face(vec![at(&topr, k), at(&botr, k), at(&botr, k + 1), at(&topr, k + 1)], vec![[u0, 0.0], [u0, 1.0], [u1, 1.0], [u1, 0.0]]),
        }
    }
    let cap_uv = |k: usize| {
        let a = k as f64 / fs * TAU;
        [0.5 + a.sin() * 0.5, 0.5 + a.cos() * 0.5]
    };
    if topr.len() > 1 {
        b.face(topr.clone(), (0..seg).map(cap_uv).collect());
    }
    if botr.len() > 1 {
        b.face(botr.iter().rev().copied().collect(), (0..seg).rev().map(cap_uv).collect());
    }
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// A capsule: a cylinder with half-sphere ends, `height` tall in all.
pub(super) fn capsule(radius: f64, height: f64) -> PolyMesh {
    let total = ok(height);
    let r = ok(radius).min(total / 2.0);
    let hc = (total / 2.0 - r).max(0.0);
    let (seg, quarter) = (SMOOTH_SEGMENTS, 12usize);
    let mut b = Builder::new();
    let top_y = hc + r;
    let v_of = |y: f64| if total > 0.0 { (top_y - y) / total } else { 0.0 };
    // Rows from the top pole to the bottom pole: (y, ring radius).
    let mut rows: Vec<(f64, f64)> = vec![];
    for j in 0..=quarter {
        let phi = j as f64 / quarter as f64 * FRAC_PI_2;
        rows.push((hc + r * phi.cos(), r * phi.sin()));
    }
    for j in 0..=quarter {
        let phi = FRAC_PI_2 + j as f64 / quarter as f64 * FRAC_PI_2;
        rows.push((-hc + r * phi.cos(), r * phi.sin()));
    }
    let ids: Vec<Vec<u32>> = rows
        .iter()
        .map(|&(y, rr)| {
            (0..seg)
                .map(|k| {
                    let a = k as f64 / seg as f64 * TAU;
                    b.v([rr * a.sin(), y, rr * a.cos()])
                })
                .collect()
        })
        .collect();
    for j in 0..rows.len() - 1 {
        let (v0, v1) = (v_of(rows[j].0), v_of(rows[j + 1].0));
        for k in 0..seg {
            let k1 = (k + 1) % seg;
            let (u0, u1) = (k as f64 / seg as f64, (k + 1) as f64 / seg as f64);
            b.face(vec![ids[j][k], ids[j + 1][k], ids[j + 1][k1], ids[j][k1]], vec![[u0, v0], [u0, v1], [u1, v1], [u1, v0]]);
        }
    }
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// A ring doughnut lying in the xz plane: `radius` to the tube's centre, `tube` its thickness.
pub(super) fn torus(radius: f64, tube: f64) -> PolyMesh {
    let (rr, tr) = (ok(radius), ok(tube));
    let (around, sides) = (64usize, 24usize);
    let mut b = Builder::new();
    let ids: Vec<Vec<u32>> = (0..around)
        .map(|i| {
            let a = i as f64 / around as f64 * TAU;
            (0..sides)
                .map(|j| {
                    let t = j as f64 / sides as f64 * TAU;
                    let d = rr + tr * t.cos();
                    b.v([a.sin() * d, tr * t.sin(), a.cos() * d])
                })
                .collect()
        })
        .collect();
    for i in 0..around {
        for j in 0..sides {
            let (i1, j1) = ((i + 1) % around, (j + 1) % sides);
            let (u0, u1) = (i as f64 / around as f64, (i + 1) as f64 / around as f64);
            let (v0, v1) = (j as f64 / sides as f64, (j + 1) as f64 / sides as f64);
            b.face(vec![ids[i][j], ids[i1][j], ids[i1][j1], ids[i][j1]], vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]]);
        }
    }
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// A flat card facing +z, centred.
pub(super) fn plane(width: f64, height: f64) -> PolyMesh {
    let (w, h) = (ok(width) / 2.0, ok(height) / 2.0);
    let mut b = Builder::new();
    let ids = [b.v([-w, h, 0.0]), b.v([-w, -h, 0.0]), b.v([w, -h, 0.0]), b.v([w, h, 0.0])];
    b.face(ids.to_vec(), vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]);
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// A flat grid lying on y = 0 facing up: `width` along x, `height` along z, rows × cols faces.
pub(super) fn grid(width: f64, height: f64, rows: f64, cols: f64) -> PolyMesh {
    let count = |n: f64| if n.is_finite() { n.round().clamp(1.0, 2000.0) as usize } else { 1 };
    let (mut rows, mut cols) = (count(rows), count(cols));
    while rows * cols > MAX_FACES {
        tracing::warn!(rows, cols, "grid too fine; halving it");
        rows = rows.div_ceil(2);
        cols = cols.div_ceil(2);
    }
    let (w, h) = (ok(width), ok(height));
    let mut b = Builder::new();
    let mut ids = vec![vec![0u32; rows + 1]; cols + 1];
    for (i, col) in ids.iter_mut().enumerate() {
        for (j, id) in col.iter_mut().enumerate() {
            *id = b.v([-w / 2.0 + w * i as f64 / cols as f64, 0.0, -h / 2.0 + h * j as f64 / rows as f64]);
        }
    }
    for i in 0..cols {
        for j in 0..rows {
            let (u0, u1) = (i as f64 / cols as f64, (i + 1) as f64 / cols as f64);
            let (v0, v1) = (j as f64 / rows as f64, (j + 1) as f64 / rows as f64);
            b.face(vec![ids[i][j], ids[i][j + 1], ids[i + 1][j + 1], ids[i + 1][j]], vec![[u0, v0], [u0, v1], [u1, v1], [u1, v0]]);
        }
    }
    b.finish(DEFAULT_SMOOTH_ANGLE)
}

/// A profile of `[distance from the axis, height]` points turned `angle` degrees around y.
pub(super) fn lathe(profile: &[[f64; 2]], segments: f64, angle: f64) -> PolyMesh {
    let pts: Vec<[f64; 2]> = profile.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).map(|p| [p[0].abs(), p[1]]).collect();
    let angle = if angle.is_finite() { angle.clamp(0.0, 360.0) } else { 360.0 };
    if pts.len() < 2 || angle <= 1e-6 {
        return PolyMesh::default();
    }
    let seg = segments_or(segments, SMOOTH_SEGMENTS);
    let full = angle >= 360.0 - 1e-9;
    let cols = if full { seg } else { seg + 1 };
    let span = pts.iter().fold(0.0f64, |m, p| m.max(p[0]).max(p[1].abs())).max(1e-9);
    let on_axis = |p: [f64; 2]| p[0] <= span * 1e-9;
    let mut b = Builder::new();
    let ids: Vec<Vec<u32>> = pts
        .iter()
        .map(|&p| {
            if on_axis(p) {
                vec![b.v([0.0, p[1], 0.0]); cols]
            } else {
                (0..cols)
                    .map(|k| {
                        let a = (k as f64 / seg as f64 * angle).to_radians();
                        b.v([p[0] * a.sin(), p[1], p[0] * a.cos()])
                    })
                    .collect()
            }
        })
        .collect();
    let mut along = vec![0.0];
    for w in pts.windows(2) {
        let l = along.last().copied().unwrap_or(0.0) + ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        along.push(l);
    }
    let total = along.last().copied().unwrap_or(0.0).max(1e-12);
    for j in 0..pts.len() - 1 {
        let (v0, v1) = (1.0 - along[j] / total, 1.0 - along[j + 1] / total);
        for k in 0..seg {
            let k1 = if full { (k + 1) % seg } else { k + 1 };
            let (u0, u1) = (k as f64 / seg as f64, (k + 1) as f64 / seg as f64);
            b.face(vec![ids[j][k], ids[j][k1], ids[j + 1][k1], ids[j + 1][k]], vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]]);
        }
    }
    let mut m = b.finish(DEFAULT_SMOOTH_ANGLE);
    // A profile drawn downwards would face inwards: turn it out.
    if m.volume() < 0.0 {
        for f in 0..m.faces.len() {
            m.flip_face(f);
        }
    }
    m
}

/// A round tube of `radius` with `sides` along a curve, between `trim_start` and `trim_end`
/// (0–1 of its length). Frames are rotation-minimising so the tube doesn't twist.
pub(super) fn tube(points: &[[f64; 3]], closed: bool, smooth: bool, radius: f64, sides: f64, trim_start: f64, trim_end: f64) -> PolyMesh {
    let pts: Vec<V3> = points.iter().copied().filter(|p| finite(*p)).collect();
    if pts.len() < 2 {
        return PolyMesh::default();
    }
    let sides = if sides.is_finite() { sides.round().clamp(3.0, 64.0) as usize } else { 8 };
    let line = Polyline::new(&pts, closed, smooth);
    let (ts, te) = (if trim_start.is_finite() { trim_start } else { 0.0 }, if trim_end.is_finite() { trim_end } else { 1.0 });
    let ring = closed && ts <= 0.0 && te >= 1.0;
    let mut path: Vec<V3> = if ring { line.points.clone() } else { line.trimmed(ts, te) };
    let tol = line.length().max(1e-9) * 1e-9;
    path.dedup_by(|a, b| dist(*a, *b) <= tol);
    if ring && path.len() > 1 && dist(path[0], path[path.len() - 1]) <= tol {
        path.pop();
    }
    let n = path.len();
    if n < 2 || (ring && n < 3) {
        return PolyMesh::default();
    }
    // Tangents, then normals carried along by double reflection.
    let tangent = |i: usize| -> V3 {
        let (a, b) = if ring {
            (path[(i + n - 1) % n], path[(i + 1) % n])
        } else {
            (path[i.saturating_sub(1)], path[(i + 1).min(n - 1)])
        };
        try_norm(sub(b, a)).unwrap_or([0.0, 0.0, 1.0])
    };
    let ts: Vec<V3> = (0..n).map(tangent).collect();
    let up = [0.0, 1.0, 0.0];
    let mut normals = vec![[0.0; 3]; n];
    normals[0] = try_norm(sub(up, scale(ts[0], dot(ts[0], up)))).unwrap_or_else(|| perpendicular(ts[0]));
    let steps = if ring { n } else { n - 1 };
    let mut carried = normals[0];
    for i in 0..steps {
        let (j, k) = (i, (i + 1) % n);
        let v1 = sub(path[k], path[j]);
        let c1 = dot(v1, v1);
        let r = if c1 > 0.0 { sub(normals[j], scale(v1, 2.0 / c1 * dot(v1, normals[j]))) } else { normals[j] };
        let t = if c1 > 0.0 { sub(ts[j], scale(v1, 2.0 / c1 * dot(v1, ts[j]))) } else { ts[j] };
        let v2 = sub(ts[k], t);
        let c2 = dot(v2, v2);
        let next = if c2 > 1e-30 { sub(r, scale(v2, 2.0 / c2 * dot(v2, r))) } else { r };
        let next = try_norm(sub(next, scale(ts[k], dot(ts[k], next)))).unwrap_or_else(|| perpendicular(ts[k]));
        if k == 0 {
            carried = next;
        } else {
            normals[k] = next;
        }
    }
    if ring {
        // Spread the leftover twist evenly so the tube closes without a seam.
        let b0 = cross(ts[0], normals[0]);
        let mismatch = dot(carried, b0).atan2(dot(carried, normals[0]));
        for (i, nrm) in normals.iter_mut().enumerate().skip(1) {
            *nrm = rotate(*nrm, ts[i], -mismatch * i as f64 / n as f64);
        }
    }
    let mut b = Builder::new();
    let ids: Vec<Vec<u32>> = (0..n)
        .map(|i| {
            let bn = cross(ts[i], normals[i]);
            (0..sides)
                .map(|k| {
                    let a = k as f64 / sides as f64 * TAU;
                    b.v(add(path[i], add(scale(normals[i], radius * a.cos()), scale(bn, radius * a.sin()))))
                })
                .collect()
        })
        .collect();
    let mut along = vec![0.0];
    for i in 1..n {
        along.push(along[i - 1] + dist(path[i - 1], path[i]));
    }
    let total = (along[n - 1] + if ring { dist(path[n - 1], path[0]) } else { 0.0 }).max(1e-12);
    for i in 0..steps {
        let i1 = (i + 1) % n;
        let (v0, v1) = (along[i] / total, if i + 1 == n { 1.0 } else { along[i + 1] / total });
        for k in 0..sides {
            let k1 = (k + 1) % sides;
            let (u0, u1) = (k as f64 / sides as f64, (k + 1) as f64 / sides as f64);
            b.face(vec![ids[i][k], ids[i][k1], ids[i1][k1], ids[i1][k]], vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]]);
        }
    }
    if !ring {
        let cap = |k: usize| {
            let a = k as f64 / sides as f64 * TAU;
            [0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()]
        };
        b.face(ids[n - 1].clone(), (0..sides).map(cap).collect());
        b.face(ids[0].iter().rev().copied().collect(), (0..sides).rev().map(cap).collect());
    }
    b.finish(60.0)
}
