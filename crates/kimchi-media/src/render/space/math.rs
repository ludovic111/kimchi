//! The little linear algebra the 3D renderer needs: column vectors, column-major 4×4 matrices
//! (the layout WGSL expects), right-handed, depth 0 (near) to 1 (far).

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct V3(pub f32, pub f32, pub f32);

impl V3 {
    pub(crate) fn from(a: [f64; 3]) -> V3 {
        V3(a[0] as f32, a[1] as f32, a[2] as f32)
    }
    pub(crate) fn dot(self, o: V3) -> f32 {
        self.0 * o.0 + self.1 * o.1 + self.2 * o.2
    }
    pub(crate) fn cross(self, o: V3) -> V3 {
        V3(self.1 * o.2 - self.2 * o.1, self.2 * o.0 - self.0 * o.2, self.0 * o.1 - self.1 * o.0)
    }
    pub(crate) fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub(crate) fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-12 { self * (1.0 / l) } else { V3(0.0, 0.0, 1.0) }
    }
    pub(crate) fn arr(self) -> [f32; 3] {
        [self.0, self.1, self.2]
    }
    pub(crate) fn max(self, o: V3) -> V3 {
        V3(self.0.max(o.0), self.1.max(o.1), self.2.max(o.2))
    }
    pub(crate) fn min(self, o: V3) -> V3 {
        V3(self.0.min(o.0), self.1.min(o.1), self.2.min(o.2))
    }
    pub(crate) fn of(a: [f32; 3]) -> V3 {
        V3(a[0], a[1], a[2])
    }
    pub(crate) fn finite(self) -> bool {
        self.0.is_finite() && self.1.is_finite() && self.2.is_finite()
    }
    /// Any unit vector at right angles to this one.
    pub(crate) fn perpendicular(self) -> V3 {
        let a = if self.0.abs() < 0.9 { V3(1.0, 0.0, 0.0) } else { V3(0.0, 1.0, 0.0) };
        self.cross(a).norm()
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3(self.0 + o.0, self.1 + o.1, self.2 + o.2)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3(self.0 - o.0, self.1 - o.1, self.2 - o.2)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, k: f32) -> V3 {
        V3(self.0 * k, self.1 * k, self.2 * k)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3(-self.0, -self.1, -self.2)
    }
}

/// Column-major: `m[col][row]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct M4(pub [[f32; 4]; 4]);

impl M4 {
    pub(crate) const I: M4 = M4([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);

    pub(crate) fn translate(v: V3) -> M4 {
        let mut m = M4::I;
        m.0[3] = [v.0, v.1, v.2, 1.0];
        m
    }

    pub(crate) fn scale(v: V3) -> M4 {
        let mut m = M4::I;
        m.0[0][0] = v.0;
        m.0[1][1] = v.1;
        m.0[2][2] = v.2;
        m
    }

    pub(crate) fn rot_x(deg: f32) -> M4 {
        let (s, c) = deg.to_radians().sin_cos();
        M4([[1.0, 0.0, 0.0, 0.0], [0.0, c, s, 0.0], [0.0, -s, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    pub(crate) fn rot_y(deg: f32) -> M4 {
        let (s, c) = deg.to_radians().sin_cos();
        M4([[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    pub(crate) fn rot_z(deg: f32) -> M4 {
        let (s, c) = deg.to_radians().sin_cos();
        M4([[c, s, 0.0, 0.0], [-s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    /// Translation × rotation (x, then y, then z, in degrees) × scale.
    pub(crate) fn trs(t: V3, r: V3, s: V3) -> M4 {
        M4::translate(t) * M4::rot_z(r.2) * M4::rot_y(r.1) * M4::rot_x(r.0) * M4::scale(s)
    }

    /// Right-handed perspective, depth 0..1.
    pub(crate) fn perspective(fov_y_deg: f32, aspect: f32, near: f32, far: f32) -> M4 {
        let f = 1.0 / (fov_y_deg.to_radians() / 2.0).tan();
        let r = far / (near - far);
        M4([[f / aspect, 0.0, 0.0, 0.0], [0.0, f, 0.0, 0.0], [0.0, 0.0, r, -1.0], [0.0, 0.0, r * near, 0.0]])
    }

    /// Right-handed orthographic, depth 0..1.
    pub(crate) fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> M4 {
        M4([
            [2.0 / (r - l), 0.0, 0.0, 0.0],
            [0.0, 2.0 / (t - b), 0.0, 0.0],
            [0.0, 0.0, 1.0 / (n - f), 0.0],
            [-(r + l) / (r - l), -(t + b) / (t - b), n / (n - f), 1.0],
        ])
    }

    pub(crate) fn look_at(eye: V3, target: V3, up: V3) -> M4 {
        let f = (target - eye).norm();
        let mut s = f.cross(up);
        if s.len() < 1e-6 {
            // Looking straight up or down: any sideways axis will do.
            s = f.cross(V3(0.0, 0.0, 1.0));
        }
        let s = s.norm();
        let u = s.cross(f);
        M4([
            [s.0, u.0, -f.0, 0.0],
            [s.1, u.1, -f.1, 0.0],
            [s.2, u.2, -f.2, 0.0],
            [-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0],
        ])
    }

    pub(crate) fn point(&self, v: V3) -> [f32; 4] {
        let m = &self.0;
        let x = m[0][0] * v.0 + m[1][0] * v.1 + m[2][0] * v.2 + m[3][0];
        let y = m[0][1] * v.0 + m[1][1] * v.1 + m[2][1] * v.2 + m[3][1];
        let z = m[0][2] * v.0 + m[1][2] * v.1 + m[2][2] * v.2 + m[3][2];
        let w = m[0][3] * v.0 + m[1][3] * v.1 + m[2][3] * v.2 + m[3][3];
        [x, y, z, w]
    }

    pub(crate) fn point3(&self, v: V3) -> V3 {
        let [x, y, z, w] = self.point(v);
        let w = if w.abs() > 1e-12 { w } else { 1.0 };
        V3(x / w, y / w, z / w)
    }

    pub(crate) fn dir(&self, v: V3) -> V3 {
        let m = &self.0;
        V3(
            m[0][0] * v.0 + m[1][0] * v.1 + m[2][0] * v.2,
            m[0][1] * v.0 + m[1][1] * v.1 + m[2][1] * v.2,
            m[0][2] * v.0 + m[1][2] * v.1 + m[2][2] * v.2,
        )
    }

    /// Inverse-transpose of the upper 3×3 (for normals), as a 4×4.
    pub(crate) fn normal_matrix(&self) -> M4 {
        let m = &self.0;
        let (a, b, c) = (m[0][0], m[1][0], m[2][0]);
        let (d, e, f) = (m[0][1], m[1][1], m[2][1]);
        let (g, h, i) = (m[0][2], m[1][2], m[2][2]);
        let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
        if det.abs() < 1e-12 {
            return M4::I;
        }
        let k = 1.0 / det;
        // inverse (row-major cofactors / det), then transposed into our column-major layout.
        let inv = [
            [(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k],
            [(f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k],
            [(d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k],
        ];
        // transpose(inv) in column-major: column j = row j of inv.
        M4([
            [inv[0][0], inv[0][1], inv[0][2], 0.0],
            [inv[1][0], inv[1][1], inv[1][2], 0.0],
            [inv[2][0], inv[2][1], inv[2][2], 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    /// A matrix whose columns are the given axes and origin.
    pub(crate) fn from_axes(x: V3, y: V3, z: V3, origin: V3) -> M4 {
        M4([[x.0, x.1, x.2, 0.0], [y.0, y.1, y.2, 0.0], [z.0, z.1, z.2, 0.0], [origin.0, origin.1, origin.2, 1.0]])
    }

    /// Where the origin goes.
    pub(crate) fn origin(&self) -> V3 {
        V3(self.0[3][0], self.0[3][1], self.0[3][2])
    }

    /// The general inverse (None when the matrix squashes space flat).
    pub(crate) fn inverse(&self) -> Option<M4> {
        // Row-major copy, Gauss-Jordan in f64 for precision.
        let mut a = [[0.0f64; 8]; 4];
        for (r, row) in a.iter_mut().enumerate() {
            for c in 0..4 {
                row[c] = self.0[c][r] as f64;
            }
            row[4 + r] = 1.0;
        }
        for col in 0..4 {
            let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
            if a[pivot][col].abs() < 1e-12 {
                return None;
            }
            a.swap(col, pivot);
            let k = 1.0 / a[col][col];
            for v in a[col].iter_mut() {
                *v *= k;
            }
            for r in 0..4 {
                if r != col {
                    let f = a[r][col];
                    if f != 0.0 {
                        for c in 0..8 {
                            a[r][c] -= f * a[col][c];
                        }
                    }
                }
            }
        }
        let mut out = [[0.0f32; 4]; 4];
        for (c, col) in out.iter_mut().enumerate() {
            for (r, v) in col.iter_mut().enumerate() {
                *v = a[r][4 + c] as f32;
            }
        }
        Some(M4(out))
    }

    /// In f64, column-major like this one.
    pub(crate) fn to_f64(self) -> [[f64; 4]; 4] {
        self.0.map(|c| c.map(|v| v as f64))
    }

    pub(crate) fn flat(&self) -> [f32; 16] {
        let mut out = [0.0; 16];
        for c in 0..4 {
            out[c * 4..c * 4 + 4].copy_from_slice(&self.0[c]);
        }
        out
    }
}

impl Mul for M4 {
    type Output = M4;
    fn mul(self, o: M4) -> M4 {
        let mut r = [[0.0f32; 4]; 4];
        for (c, col) in r.iter_mut().enumerate() {
            for (row, v) in col.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[k][row] * o.0[c][k]).sum();
            }
        }
        M4(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (a - b).len() < 1e-4
    }

    #[test]
    fn transforms_points() {
        // Rotating x by 90° about y gives -z (right-handed).
        assert!(close(M4::rot_y(90.0).point3(V3(1.0, 0.0, 0.0)), V3(0.0, 0.0, -1.0)));
        assert!(close(M4::rot_x(90.0).point3(V3(0.0, 1.0, 0.0)), V3(0.0, 0.0, 1.0)));
        assert!(close(M4::rot_z(90.0).point3(V3(1.0, 0.0, 0.0)), V3(0.0, 1.0, 0.0)));
        let m = M4::trs(V3(1.0, 2.0, 3.0), V3(0.0, 0.0, 0.0), V3(2.0, 2.0, 2.0));
        assert!(close(m.point3(V3(1.0, 1.0, 1.0)), V3(3.0, 4.0, 5.0)));
        // The camera looks down -z; a point in front lands mid-screen at depth 0..1.
        let view = M4::look_at(V3(0.0, 0.0, 5.0), V3(0.0, 0.0, 0.0), V3(0.0, 1.0, 0.0));
        let proj = M4::perspective(60.0, 1.0, 0.1, 100.0);
        let p = (proj * view).point3(V3(0.0, 0.0, 0.0));
        assert!(p.0.abs() < 1e-5 && p.1.abs() < 1e-5 && p.2 > 0.0 && p.2 < 1.0, "{p:?}");
        let up = (proj * view).point3(V3(0.0, 1.0, 0.0));
        assert!(up.1 > 0.0, "y up on screen");
        // Normal matrix of a non-uniform scale keeps normals perpendicular.
        let s = M4::scale(V3(2.0, 1.0, 1.0));
        assert!(close(s.normal_matrix().dir(V3(1.0, 1.0, 0.0)).norm(), V3(0.5, 1.0, 0.0).norm()));
        // Inverses undo.
        let m = M4::trs(V3(1.0, -2.0, 3.0), V3(10.0, 20.0, 30.0), V3(2.0, 0.5, 1.5));
        let inv = m.inverse().unwrap();
        assert!(close(inv.point3(m.point3(V3(0.3, 0.7, -1.1))), V3(0.3, 0.7, -1.1)));
        assert!(M4::scale(V3(1.0, 0.0, 1.0)).inverse().is_none());
    }
}
