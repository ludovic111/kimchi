//! The Studio's 3D view: the scene seen from a free editor camera (orbit, pan, zoom) or through
//! its own camera, with a floor grid, outlines on selected objects and, in edit mode, the edited
//! mesh's vertices and edges; and the maths the window needs to aim its tools (projecting points
//! to the screen, rays from the mouse, picking objects).

use kimchi_core::motion::{Camera, Scene3d, Vec3};
use serde::{Deserialize, Serialize};

/// Where the editor looks from (not part of the scene).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewCamera {
    pub position: [f64; 3],
    pub target: [f64; 3],
    /// Vertical field of view, degrees.
    pub fov: f64,
    pub ortho: bool,
    /// Height the view covers when orthographic, world units.
    pub ortho_size: f64,
}

impl Default for ViewCamera {
    fn default() -> Self {
        ViewCamera { position: [6.0, 4.5, 8.0], target: [0.0, 0.5, 0.0], fov: 40.0, ortho: false, ortho_size: 6.0 }
    }
}

/// How objects are drawn in the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Shading {
    /// Plain grey shapes lit from the view (fast, shows the forms).
    Solid,
    /// Materials and the scene's lights, quick settings.
    #[default]
    Material,
    /// The final engine (path tracer when the scene uses it), refining while still.
    Rendered,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewOptions {
    /// Look through the scene's active camera instead of the editor's.
    pub through_camera: bool,
    pub shading: Shading,
    /// Floor grid and axes.
    pub grid: bool,
    /// Objects drawn with a selection outline.
    pub selected: Vec<String>,
    /// The object whose mesh is being edited: its edges and vertices are drawn.
    pub edit: Option<String>,
    /// In edit mode: selected vertices (indices into the evaluated mesh).
    pub edit_vertices: Vec<u32>,
    /// In edit mode: selected faces.
    pub edit_faces: Vec<u32>,
    /// Draw lights and cameras as small icons/wires.
    pub helpers: bool,
}

impl ViewCamera {
    /// The scene's camera replaced by this view (for drawing from it).
    pub fn as_camera(&self, base: &Camera) -> Camera {
        let mut c = base.clone();
        c.position = Vec3(self.position);
        c.target = Vec3(self.target);
        c.fov = self.fov;
        c.projection = if self.ortho { "orthographic".into() } else { "perspective".into() };
        c.ortho_size = self.ortho_size;
        c.roll = 0.0;
        c.f_stop = 0.0;
        c.constraints.clear();
        c.expressions.clear();
        c.keyframes.clear();
        c
    }

    /// The scene as filmed from this view.
    pub fn apply(&self, scene: &Scene3d) -> Scene3d {
        let mut s = scene.clone();
        s.camera = self.as_camera(&scene.camera);
        s.active_camera = None;
        s.keyframes.remove("activeCamera");
        s
    }

    fn basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let f = norm(sub(self.target, self.position));
        let up0 = if f[1].abs() > 0.999 { [0.0, 0.0, -1.0] } else { [0.0, 1.0, 0.0] };
        let r = norm(cross(f, up0));
        let u = cross(r, f);
        (f, r, u)
    }

    /// Where a world point lands in a `w`×`h` view, in pixels from the top left (None behind the camera).
    pub fn project(&self, w: f64, h: f64, p: [f64; 3]) -> Option<[f64; 2]> {
        let (f, r, u) = self.basis();
        let d = sub(p, self.position);
        let z = dot(d, f);
        let (x, y) = (dot(d, r), dot(d, u));
        let half_h = if self.ortho {
            self.ortho_size / 2.0
        } else {
            if z <= 1e-6 {
                return None;
            }
            z * (self.fov.to_radians() / 2.0).tan()
        };
        let half_w = half_h * w / h.max(1.0);
        Some([w / 2.0 + x / half_w * w / 2.0, h / 2.0 - y / half_h * h / 2.0])
    }

    /// The ray through pixel (`x`, `y`) of a `w`×`h` view: origin and unit direction.
    pub fn ray(&self, w: f64, h: f64, x: f64, y: f64) -> ([f64; 3], [f64; 3]) {
        let (f, r, u) = self.basis();
        let nx = (x / w.max(1.0)) * 2.0 - 1.0;
        let ny = 1.0 - (y / h.max(1.0)) * 2.0;
        let aspect = w / h.max(1.0);
        if self.ortho {
            let (hh, hw) = (self.ortho_size / 2.0, self.ortho_size / 2.0 * aspect);
            let o = add(self.position, add(scale(r, nx * hw), scale(u, ny * hh)));
            return (o, f);
        }
        let t = (self.fov.to_radians() / 2.0).tan();
        let d = norm(add(f, add(scale(r, nx * t * aspect), scale(u, ny * t))));
        (self.position, d)
    }

    /// Turns the view around its target (degrees: left/right, up/down).
    pub fn orbit(&mut self, yaw: f64, pitch: f64) {
        let d = sub(self.position, self.target);
        let dist = len(d).max(1e-6);
        let mut az = d[2].atan2(d[0]);
        let mut el = (d[1] / dist).clamp(-1.0, 1.0).asin();
        az -= yaw.to_radians();
        el = (el + pitch.to_radians()).clamp(-1.55, 1.55);
        self.position = add(self.target, [dist * el.cos() * az.cos(), dist * el.sin(), dist * el.cos() * az.sin()]);
    }

    /// Slides the view sideways and up/down by a share of what it shows.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let (_, r, u) = self.basis();
        let span = if self.ortho { self.ortho_size } else { len(sub(self.position, self.target)) * (self.fov.to_radians() / 2.0).tan() * 2.0 };
        let m = add(scale(r, -dx * span), scale(u, dy * span));
        self.position = add(self.position, m);
        self.target = add(self.target, m);
    }

    /// Moves closer (factor < 1) or further (> 1).
    pub fn zoom(&mut self, factor: f64) {
        let factor = factor.clamp(0.05, 20.0);
        if self.ortho {
            self.ortho_size = (self.ortho_size * factor).max(0.01);
            return;
        }
        let d = sub(self.position, self.target);
        self.position = add(self.target, scale(d, factor));
    }

    /// Looks along an axis (front, back, left, right, top, bottom) at the same distance.
    pub fn align(&mut self, axis: &str) {
        let dist = len(sub(self.position, self.target)).max(1e-3);
        let dir = match axis {
            "front" => [0.0, 0.0, 1.0],
            "back" => [0.0, 0.0, -1.0],
            "right" => [1.0, 0.0, 0.0],
            "left" => [-1.0, 0.0, 0.0],
            "top" => [0.0, 1.0, 0.0001],
            "bottom" => [0.0, -1.0, 0.0001],
            _ => return,
        };
        self.position = add(self.target, scale(norm(dir), dist));
    }
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = len(a);
    if l < 1e-12 { [0.0, 0.0, -1.0] } else { scale(a, 1.0 / l) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_go_back_through_projected_points() {
        let v = ViewCamera::default();
        let p = [0.7, 1.2, -0.4];
        let [x, y] = v.project(1280.0, 720.0, p).unwrap();
        let (o, d) = v.ray(1280.0, 720.0, x, y);
        let to = norm(sub(p, o));
        assert!(dot(to, d) > 0.99999, "{to:?} vs {d:?}");
        let c = v.project(1280.0, 720.0, v.target).unwrap();
        assert!((c[0] - 640.0).abs() < 1e-6 && (c[1] - 360.0).abs() < 1e-6);
        let mut o = v;
        o.orbit(90.0, 0.0);
        assert!((len(sub(o.position, o.target)) - len(sub(v.position, v.target))).abs() < 1e-9, "orbit keeps the distance");
    }
}
