//! The Studio's 3D view: the scene seen from a free editor camera (orbit, pan, zoom) or through
//! its own camera, with a floor grid, outlines on selected objects and, in edit mode, the edited
//! mesh's vertices and edges; and the maths the window needs to aim its tools (projecting points
//! to the screen, rays from the mouse, picking objects).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kimchi_core::mesh::PolyMesh;
use kimchi_core::motion::{Camera, Object3d, Scene3d, Shape3d, Vec3, find_object, walk_objects};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tiny_skia::Pixmap;

use super::math::{M4, V3};
use super::mesh::{self, Mesh};
use super::{Frame3d, LightKind, LightRes, Mat, Pictures, Quality, Space, shapes};
use crate::MediaResult;

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

    /// The axis view this one looks along (within a degree), if any.
    pub fn aligned_axis(&self) -> Option<&'static str> {
        let d = norm(sub(self.position, self.target));
        [("front", [0.0, 0.0, 1.0]), ("back", [0.0, 0.0, -1.0]), ("right", [1.0, 0.0, 0.0]), ("left", [-1.0, 0.0, 0.0]), ("top", [0.0, 1.0, 0.0]), ("bottom", [0.0, -1.0, 0.0])]
            .into_iter()
            .find(|(_, a)| dot(d, *a) > 0.9998)
            .map(|(n, _)| n)
    }

    /// The scene camera's view (its position, target, lens and projection).
    pub fn from_camera(c: &Camera) -> ViewCamera {
        ViewCamera { position: c.position.0, target: c.target.0, fov: c.fov, ortho: c.orthographic(), ortho_size: c.ortho_size }
    }

    /// The view's direction, its right and its up (unit vectors).
    pub fn axes(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        self.basis()
    }

    /// How far it is from what it looks at.
    pub fn distance(&self) -> f64 {
        len(sub(self.position, self.target))
    }

    /// Turns the view where it stands (looking around, first person): degrees left/right and
    /// up/down; what it looks at moves, the eye stays.
    pub fn look(&mut self, yaw: f64, pitch: f64) {
        let d = sub(self.target, self.position);
        let dist = len(d).max(1e-6);
        let mut az = d[2].atan2(d[0]);
        let mut el = (d[1] / dist).clamp(-1.0, 1.0).asin();
        az += yaw.to_radians();
        el = (el + pitch.to_radians()).clamp(-1.55, 1.55);
        self.target = add(self.position, [dist * el.cos() * az.cos(), dist * el.sin(), dist * el.cos() * az.sin()]);
    }

    /// Moves the eye and what it looks at together, along the view (`forward`), its right and
    /// the world's up, in world units: flying through the scene.
    pub fn fly(&mut self, forward: f64, right: f64, up: f64) {
        let (f, r, _) = self.basis();
        let m = add(add(scale(f, forward), scale(r, right)), [0.0, up, 0.0]);
        self.position = add(self.position, m);
        self.target = add(self.target, m);
    }

    /// The point under pixel (`x`, `y`) of a `w`×`h` view on the plane through the target
    /// facing the view: where zooming to the pointer heads.
    pub fn point_under(&self, w: f64, h: f64, x: f64, y: f64) -> [f64; 3] {
        let (o, d) = self.ray(w, h, x, y);
        let (f, _, _) = self.basis();
        let denom = dot(d, f);
        if denom.abs() < 1e-9 {
            return self.target;
        }
        let t = dot(sub(self.target, o), f) / denom;
        add(o, scale(d, t.max(0.0)))
    }

    /// Zooms like [`zoom`](Self::zoom) but towards `point` (closer when `factor` < 1): the point
    /// stays where it is on the screen.
    pub fn zoom_toward(&mut self, factor: f64, point: [f64; 3]) {
        let factor = factor.clamp(0.05, 20.0);
        let (_, r, u) = self.basis();
        if self.ortho {
            // The picture scales around the point: slide the view by what it no longer covers.
            let off = sub(point, self.target);
            let (x, y) = (dot(off, r), dot(off, u));
            let shift = add(scale(r, x * (1.0 - factor)), scale(u, y * (1.0 - factor)));
            self.position = add(self.position, shift);
            self.target = add(self.target, shift);
            self.ortho_size = (self.ortho_size * factor).max(0.01);
            return;
        }
        // Everything scales around the point: the ray to it keeps its direction.
        let near = len(sub(self.position, self.target)) * factor < 1e-3;
        if near {
            return;
        }
        self.position = add(point, scale(sub(self.position, point), factor));
        self.target = add(point, scale(sub(self.target, point), factor));
    }

    /// The view `k` of the way (0–1) to `other`: what it looks at and how far it is move in
    /// straight lines, the direction turns around (a smooth move between two views).
    pub fn blend(&self, other: &ViewCamera, k: f64) -> ViewCamera {
        let k = k.clamp(0.0, 1.0);
        let lerp3 = |a: [f64; 3], b: [f64; 3]| add(a, scale(sub(b, a), k));
        let (d0, d1) = (sub(self.position, self.target), sub(other.position, other.target));
        let (l0, l1) = (len(d0).max(1e-6), len(d1).max(1e-6));
        let dir = slerp(scale(d0, 1.0 / l0), scale(d1, 1.0 / l1), k);
        // Distances change by the same factor each step (zooming feels even).
        let dist = l0 * (l1 / l0).powf(k);
        let target = lerp3(self.target, other.target);
        ViewCamera {
            position: add(target, scale(dir, dist)),
            target,
            fov: self.fov + (other.fov - self.fov) * k,
            ortho: if k < 0.5 { self.ortho } else { other.ortho },
            ortho_size: self.ortho_size * (other.ortho_size.max(1e-6) / self.ortho_size.max(1e-6)).powf(k),
        }
    }
}

/// Between two unit directions, along the sphere.
fn slerp(a: [f64; 3], b: [f64; 3], k: f64) -> [f64; 3] {
    let c = dot(a, b).clamp(-1.0, 1.0);
    if c > 0.9999 {
        return norm(add(a, scale(sub(b, a), k)));
    }
    if c < -0.9999 {
        // Opposite: go round over the top (or the side when they are up and down).
        let side = if a[1].abs() > 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let mid = norm(sub(side, scale(a, dot(side, a))));
        return if k < 0.5 { slerp(a, mid, k * 2.0) } else { slerp(mid, b, k * 2.0 - 1.0) };
    }
    let w = c.acos();
    let (sa, sb) = (((1.0 - k) * w).sin() / w.sin(), (k * w).sin() / w.sin());
    norm(add(scale(a, sa), scale(b, sb)))
}

// ---------------------------------------------------------------------------------------------
// Drawing the view

/// Overlay colours (straight sRGB).
const SELECTED: [u8; 3] = [255, 154, 40];
const EDGE: [u8; 3] = [24, 24, 28];
const HELPER: [u8; 3] = [236, 228, 170];
const CAMERA: [u8; 3] = [210, 210, 214];

/// The Studio's picture of a 3D scene at scene time `t`: from `view` (the editor's camera) or
/// through the scene's camera, shaded as `opts.shading` asks, with the overlays it asks for
/// (floor grid, selection outlines, the edited mesh's edges and vertices, light and camera icons).
#[allow(clippy::too_many_arguments)]
///
/// `frame` is how many scene seconds one output frame lasts (motion blur in rendered shading,
/// the frame rate expressions see).
pub(crate) fn render_view(space: &mut Space, scene: &Scene3d, t: f64, frame: f64, width: u32, height: u32, pics: &mut dyn Pictures, view: Option<&ViewCamera>, opts: &ViewOptions) -> MediaResult<Pixmap> {
    space.set_frame(frame);
    let shown = match view {
        Some(v) if !opts.through_camera => v.apply(scene),
        _ => scene.clone(),
    };
    let overlays = opts.grid || opts.helpers || !opts.selected.is_empty() || opts.edit.is_some();
    let (mut img, frame, depth) = match opts.shading {
        Shading::Rendered => {
            // The final engine (the path tracer when the scene uses it); depth for the overlays
            // from a quick pass.
            let img = space.render_frame(&shown, t, frame, width, height, pics, Quality::Final)?;
            let frame = space.frame(&shown, t, width, height, pics, Quality::Preview);
            let depth = if overlays { super::cpu::depth(&frame) } else { vec![] };
            (img, frame, depth)
        }
        shading => {
            let mut frame = space.frame(&shown, t, width, height, pics, Quality::Preview);
            if shading == Shading::Solid {
                solid(&mut frame);
            }
            let (img, depth) = space.draw(&frame, overlays);
            (img, frame, depth)
        }
    };
    if !overlays {
        return Ok(img);
    }
    let depth = if depth.len() == (width * height) as usize { depth } else { super::cpu::depth(&frame) };
    if opts.grid {
        grid(&mut img, &frame, &depth);
    }
    let objects = shown.objects_at(t);
    let world = super::world_matrices(&objects);
    if opts.helpers {
        helpers(&mut img, &frame, &shown, t, opts.through_camera);
    }
    if !opts.selected.is_empty() {
        let mut ids: HashSet<String> = HashSet::new();
        for id in &opts.selected {
            if let Some(o) = find_object(&objects, id) {
                walk_objects(std::slice::from_ref(o), &mut |c| {
                    ids.insert(c.id.clone());
                });
            }
        }
        outline(&mut img, &frame, &depth, &ids);
    }
    if let Some(id) = &opts.edit
        && let Some(o) = find_object(&objects, id)
    {
        let ctx = shapes::Ctx { t, objects: &objects, world: &world };
        if let Some(poly) = shapes::edit_poly(o, &ctx) {
            let m = world.get(id).copied().unwrap_or(M4::I);
            edit_overlay(&mut img, &frame, &depth, &poly, &m, opts);
        }
    }
    Ok(img)
}

/// Plain grey shapes lit from the view: materials, the scene's lights and its world ignored.
pub(crate) fn solid(f: &mut Frame3d) {
    for it in f.items.iter_mut() {
        it.mat = Mat {
            base: [0.62, 0.62, 0.62, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            emissive: [0.0; 3],
            unlit: false,
            texture: None,
            texture_scale: [1.0, 1.0],
            bump: None,
            transmission: 0.0,
            ior: 1.45,
            clearcoat: 0.0,
        };
    }
    let c = f.camera;
    // A key light from just above and left of the view, and a soft fill: like a studio matcap.
    let key = (c.forward - c.up * 0.6 + c.right * 0.35).norm();
    let fill = (c.forward + c.up * 0.3 - c.right * 0.6).norm();
    f.lights = vec![
        LightRes { kind: LightKind::Directional, point: false, v: key, dir: key, color: [1.05, 1.05, 1.05], range: 0.0, cos_outer: -1.0, cos_inner: -1.0, size: [0.0; 2], shadows: false },
        LightRes { kind: LightKind::Directional, point: false, v: fill, dir: fill, color: [0.3, 0.32, 0.36], range: 0.0, cos_outer: -1.0, cos_inner: -1.0, size: [0.0; 2], shadows: false },
    ];
    f.sky = [0.32, 0.33, 0.35];
    f.ground = [0.12, 0.12, 0.13];
    f.env = None;
    f.shadows.clear();
    f.shadow = None;
    f.fog = None;
    f.exposure = 0.0;
    f.filmic = false;
    f.bloom = Default::default();
    f.ao = 0.0;
    f.camera.aperture = 0.0;
}

/// `src` (straight sRGB, alpha 0–1) over a premultiplied pixel.
fn blend(px: &mut [u8], src: [u8; 3], a: f32) {
    let a = a.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    for k in 0..3 {
        px[k] = (src[k] as f32 * a + px[k] as f32 * (1.0 - a)).round() as u8;
    }
    px[3] = (255.0 * a + px[3] as f32 * (1.0 - a)).round() as u8;
}

/// The view ray through the middle of pixel (`x`, `y`).
fn pixel_ray(f: &Frame3d, x: f32, y: f32) -> (V3, V3) {
    let c = &f.camera;
    if c.ortho {
        let aspect = f.width as f32 / f.height.max(1) as f32;
        let nx = x / f.width as f32 * 2.0 - 1.0;
        let ny = 1.0 - y / f.height as f32 * 2.0;
        let hh = c.ortho_size / 2.0;
        return (c.eye + c.right * (nx * hh * aspect) + c.up * (ny * hh), c.forward);
    }
    (c.eye, super::view_dir(f, x, y))
}

/// The floor grid (y = 0) with the x axis in red and the z axis in green, lines a pixel wide
/// at any distance (finer lines fade out as they crowd), fading towards the horizon, hidden
/// behind objects.
fn grid(img: &mut Pixmap, f: &Frame3d, depth: &[f32]) {
    let w = f.width as usize;
    let hit = |x: f32, y: f32| -> Option<(f32, f32, f32, f32)> {
        let (o, d) = pixel_ray(f, x, y);
        if d.1.abs() < 1e-6 {
            return None;
        }
        let t = -o.1 / d.1;
        (t > 0.0).then(|| {
            let p = o + d * t;
            (p.0, p.2, (p - f.camera.eye).dot(f.camera.forward), d.1.abs())
        })
    };
    let reach = (f.camera.focus * 12.0).max(20.0);
    let data = img.data_mut();
    data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let Some((gx, gz, dist, grazing)) = hit(px, py) else { continue };
            let next = [hit(px + 1.0, py), hit(px, py + 1.0)];
            // Hidden behind things, not behind a floor of their own at the grid's height: the
            // depth is the nearest of the pixel's samples, which on a floor seen at a grazing
            // angle is well in front of the pixel's centre.
            let across = next.iter().flatten().map(|n| (n.2 - dist).abs()).fold(0.0f32, f32::max);
            if depth.get(y * w + x).is_some_and(|&d| d < dist * 0.998 - across) {
                continue;
            }
            // How much each floor coordinate changes across this pixel (lines stay a pixel wide).
            let (mut wx, mut wz) = (1e-6f32, 1e-6f32);
            for n in next.iter().flatten() {
                wx += (n.0 - gx).abs();
                wz += (n.1 - gz).abs();
            }
            let fp = wx.max(wz);
            // Lines where x (or z) is a multiple of `s`.
            let line = |c: f32, s: f32, w: f32| -> f32 {
                let d = ((c / s + 0.5).rem_euclid(1.0) - 0.5).abs() * s;
                (1.0 - d / w).clamp(0.0, 1.0)
            };
            // Spacing: the finest power of ten that keeps lines a few pixels apart.
            let s1 = 10f32.powf((fp * 6.0).log10().ceil()).max(1.0);
            let fade1 = (1.0 - fp * 12.0 / s1).clamp(0.0, 1.0);
            let s2 = s1 * 10.0;
            let minor = line(gx, s1, wx).max(line(gz, s1, wz)) * fade1 * 0.32;
            let major = line(gx, s2, wx).max(line(gz, s2, wz)) * 0.55;
            let far = (1.0 - dist / reach).clamp(0.0, 1.0) * (grazing / 0.08).min(1.0);
            let a = minor.max(major) * far;
            let px = &mut row[x * 4..x * 4 + 4];
            blend(px, [150, 150, 156], a);
            // The axes through the origin.
            let ax = (1.0 - gz.abs() / (wz * 1.2)).clamp(0.0, 1.0) * far;
            let az = (1.0 - gx.abs() / (wx * 1.2)).clamp(0.0, 1.0) * far;
            blend(px, [230, 70, 70], ax * 0.9);
            blend(px, [110, 200, 80], az * 0.9);
        }
    });
}

/// Pixels where the objects in `ids` are the nearest thing (or nearly).
fn coverage(f: &Frame3d, depth: &[f32], ids: &HashSet<String>) -> Vec<bool> {
    let (w, h) = (f.width as usize, f.height as usize);
    let mut mask = vec![false; w * h];
    for (i, it) in f.items.iter().enumerate() {
        if !ids.contains(&*it.id) {
            continue;
        }
        for t in super::cpu::triangles(f, i, &f.viewproj, w, h) {
            super::cpu::raster(&t, w, 0, h, |k, z, _, _| {
                let d = f.camera.distance(z);
                // Generous: surfaces seen edge-on get depths from the two rasterisers that differ
                // by more than a hair, which would punch speckles into the outline.
                if depth.get(k).is_none_or(|&s| d <= s * 1.015 + 0.01) {
                    mask[k] = true;
                }
            });
        }
    }
    close_holes(&mask, w, h)
}

/// Fills pinholes in a mask (a pixel surrounded by at least six covered neighbours is covered).
fn close_holes(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut out = mask.to_vec();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            if mask[y * w + x] {
                continue;
            }
            let around = (-1i64..=1).flat_map(|dy| (-1i64..=1).map(move |dx| (dx, dy))).filter(|&(dx, dy)| (dx, dy) != (0, 0) && mask[(y as i64 + dy) as usize * w + (x as i64 + dx) as usize]).count();
            if around >= 6 {
                out[y * w + x] = true;
            }
        }
    }
    out
}

/// An orange line around the visible silhouettes of the selected objects.
fn outline(img: &mut Pixmap, f: &Frame3d, depth: &[f32], ids: &HashSet<String>) {
    let (w, h) = (f.width as usize, f.height as usize);
    let mask = coverage(f, depth, ids);
    let data = img.data_mut();
    for y in 0..h {
        for x in 0..w {
            let inside = mask[y * w + x];
            // Edge pixels on both sides of the silhouette: a line about two pixels wide.
            let mut edge = false;
            'n: for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    let other = nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && mask[ny as usize * w + nx as usize];
                    if other != inside {
                        edge = true;
                        break 'n;
                    }
                }
            }
            if edge {
                let i = (y * w + x) * 4;
                blend(&mut data[i..i + 4], SELECTED, if inside { 1.0 } else { 0.85 });
            }
        }
    }
}

/// A segment's ends on screen (cut where it passes behind the camera), with their distances.
fn segment(f: &Frame3d, a: V3, b: V3) -> Option<([f32; 2], [f32; 2], f32, f32)> {
    let (ca, cb) = (f.viewproj.point(a), f.viewproj.point(b));
    let eps = 1e-4;
    if ca[3] < eps && cb[3] < eps {
        return None;
    }
    let cut = |p: [f32; 4], q: [f32; 4]| -> [f32; 4] {
        let t = (eps - p[3]) / (q[3] - p[3]);
        std::array::from_fn(|k| p[k] + (q[k] - p[k]) * t)
    };
    let (ca, cb) = if ca[3] < eps { (cut(ca, cb), cb) } else if cb[3] < eps { (ca, cut(cb, ca)) } else { (ca, cb) };
    let screen = |c: [f32; 4]| [(c[0] / c[3] * 0.5 + 0.5) * f.width as f32, (0.5 - c[1] / c[3] * 0.5) * f.height as f32];
    let dist = |c: [f32; 4]| f.camera.distance((c[2] / c[3]).clamp(0.0, 1.0));
    Some((screen(ca), screen(cb), dist(ca), dist(cb)))
}

fn stroke(img: &mut Pixmap, lines: &[([f32; 2], [f32; 2])], color: [u8; 3], alpha: f32, width: f32) {
    if lines.is_empty() {
        return;
    }
    let mut pb = tiny_skia::PathBuilder::new();
    for (a, b) in lines {
        if a.iter().chain(b).all(|v| v.is_finite() && v.abs() < 1e6) {
            pb.move_to(a[0], a[1]);
            pb.line_to(b[0], b[1]);
        }
    }
    let Some(path) = pb.finish() else { return };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8);
    paint.anti_alias = true;
    img.stroke_path(&path, &paint, &tiny_skia::Stroke { width, ..Default::default() }, tiny_skia::Transform::identity(), None);
}

fn dots(img: &mut Pixmap, points: &[[f32; 2]], color: [u8; 3], radius: f32) {
    let mut pb = tiny_skia::PathBuilder::new();
    for p in points {
        if p.iter().all(|v| v.is_finite() && v.abs() < 1e6) {
            pb.push_circle(p[0], p[1], radius);
        }
    }
    let Some(path) = pb.finish() else { return };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], 255);
    paint.anti_alias = true;
    img.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
}

/// Edit mode: the edited mesh's faces (selected ones tinted), edges and vertices; what is
/// hidden behind the surface is drawn faintly.
fn edit_overlay(img: &mut Pixmap, f: &Frame3d, depth: &[f32], poly: &PolyMesh, m: &M4, opts: &ViewOptions) {
    let (w, h) = (f.width as usize, f.height as usize);
    let pts: Vec<V3> = poly.positions.iter().map(|p| m.point3(V3(p[0] as f32, p[1] as f32, p[2] as f32))).collect();
    let visible = |s: [f32; 2], d: f32| -> bool {
        let (x, y) = (s[0].floor() as i64, s[1].floor() as i64);
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            return true;
        }
        depth.get(y as usize * w + x as usize).is_none_or(|&z| d <= z * 1.01 + 0.01)
    };
    // Selected faces, tinted.
    let mut pb = tiny_skia::PathBuilder::new();
    for &fi in &opts.edit_faces {
        let Some(face) = poly.faces.get(fi as usize) else { continue };
        let screen: Vec<[f32; 2]> = face.iter().filter_map(|&v| pts.get(v as usize)).filter_map(|&p| segment(f, p, p).map(|s| s.0)).collect();
        if screen.len() >= 3 {
            pb.move_to(screen[0][0], screen[0][1]);
            for s in &screen[1..] {
                pb.line_to(s[0], s[1]);
            }
            pb.close();
        }
    }
    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(SELECTED[0], SELECTED[1], SELECTED[2], 70);
        paint.anti_alias = true;
        img.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    // Edges, each once.
    let selected: HashSet<u32> = opts.edit_vertices.iter().copied().collect();
    let mut seen = HashSet::new();
    let (mut front, mut back, mut chosen) = (vec![], vec![], vec![]);
    for face in &poly.faces {
        for k in 0..face.len() {
            let (a, b) = (face[k], face[(k + 1) % face.len()]);
            if !seen.insert((a.min(b), a.max(b))) {
                continue;
            }
            let (Some(&pa), Some(&pb)) = (pts.get(a as usize), pts.get(b as usize)) else { continue };
            let Some((sa, sb, da, db)) = segment(f, pa, pb) else { continue };
            let mid = [(sa[0] + sb[0]) / 2.0, (sa[1] + sb[1]) / 2.0];
            if selected.contains(&a) && selected.contains(&b) {
                chosen.push((sa, sb));
            } else if visible(mid, (da + db) / 2.0) || visible(sa, da) && visible(sb, db) {
                front.push((sa, sb));
            } else {
                back.push((sa, sb));
            }
        }
    }
    stroke(img, &back, EDGE, 0.18, 1.0);
    stroke(img, &front, EDGE, 0.9, 1.0);
    stroke(img, &chosen, SELECTED, 1.0, 1.5);
    // Vertices.
    let (mut plain, mut picked) = (vec![], vec![]);
    for (i, &p) in pts.iter().enumerate() {
        let Some((s, _, d, _)) = segment(f, p, p) else { continue };
        if selected.contains(&(i as u32)) {
            picked.push(s);
        } else if visible(s, d) {
            plain.push(s);
        }
    }
    dots(img, &plain, EDGE, 1.8);
    dots(img, &picked, SELECTED, 2.6);
}

/// Lights and cameras as small wire icons.
fn helpers(img: &mut Pixmap, f: &Frame3d, scene: &Scene3d, t: f64, through: bool) {
    type Lines = Vec<([f32; 2], [f32; 2])>;
    let mut light_lines = vec![];
    let mut push = |lines: &mut Lines, a: V3, b: V3| {
        if let Some((sa, sb, ..)) = segment(f, a, b) {
            lines.push((sa, sb));
        }
    };
    // A small circle around a point, facing the camera.
    let ring = |lines: &mut Lines, c: V3, r: f32, push: &mut dyn FnMut(&mut Lines, V3, V3)| {
        let (x, y) = (f.camera.right * r, f.camera.up * r);
        for k in 0..16 {
            let (a0, a1) = (k as f32 / 16.0 * std::f32::consts::TAU, (k + 1) as f32 / 16.0 * std::f32::consts::TAU);
            push(lines, c + x * a0.cos() + y * a0.sin(), c + x * a1.cos() + y * a1.sin());
        }
    };
    for l in scene.lights_at(t) {
        let p = V3::from(l.position.0);
        let dir = V3::from(l.direction.0).norm();
        let size = ((p - f.camera.eye).len() * 0.03).clamp(0.05, 2.0);
        ring(&mut light_lines, p, size, &mut push);
        match l.kind.as_str() {
            "directional" => {
                push(&mut light_lines, p, p + dir * (size * 8.0));
                let side = dir.perpendicular() * size;
                let tip = p + dir * (size * 8.0);
                push(&mut light_lines, tip, tip - dir * (size * 1.5) + side);
                push(&mut light_lines, tip, tip - dir * (size * 1.5) - side);
            }
            "spot" => {
                let half = (l.angle.clamp(1.0, 179.0) as f32).to_radians() / 2.0;
                let len = size * 8.0;
                let (a, b) = (dir.perpendicular(), dir.cross(dir.perpendicular()));
                let rad = len * half.tan();
                let end = p + dir * len;
                for k in 0..4 {
                    let ang = k as f32 * std::f32::consts::FRAC_PI_2;
                    push(&mut light_lines, p, end + (a * ang.cos() + b * ang.sin()) * rad);
                }
                for k in 0..24 {
                    let (a0, a1) = (k as f32 / 24.0 * std::f32::consts::TAU, (k + 1) as f32 / 24.0 * std::f32::consts::TAU);
                    push(&mut light_lines, end + (a * a0.cos() + b * a0.sin()) * rad, end + (a * a1.cos() + b * a1.sin()) * rad);
                }
            }
            "area" => {
                let lr = LightRes { kind: LightKind::Area, point: true, v: p, dir, color: [0.0; 3], range: 0.0, cos_outer: 0.0, cos_inner: 0.0, size: [0.0; 2], shadows: false };
                let (ax, ay) = lr.area_axes();
                let s = l.size.unwrap_or([1.0, 1.0]);
                let (hw, hh) = ((s[0] as f32 / 2.0).max(size), (s[1] as f32 / 2.0).max(size));
                let c = [p + ax * hw + ay * hh, p - ax * hw + ay * hh, p - ax * hw - ay * hh, p + ax * hw - ay * hh];
                for k in 0..4 {
                    push(&mut light_lines, c[k], c[(k + 1) % 4]);
                }
                push(&mut light_lines, p, p + dir * (size * 5.0));
            }
            _ => {
                for k in 0..8 {
                    let a = k as f32 / 8.0 * std::f32::consts::TAU;
                    let d = f.camera.right * a.cos() + f.camera.up * a.sin();
                    push(&mut light_lines, p + d * (size * 1.5), p + d * (size * 2.5));
                }
            }
        }
    }
    stroke(img, &light_lines, HELPER, 0.95, 1.2);
    // Cameras, except the one being looked through.
    let active = scene.active_camera_at(t);
    let mut cam_lines = vec![];
    let aspect = f.width as f32 / f.height.max(1) as f32;
    for cam in std::iter::once(&scene.camera).chain(&scene.cameras) {
        let id = if cam.id.is_empty() { "camera" } else { cam.id.as_str() };
        if through && id == active {
            continue;
        }
        let c = cam.at(t);
        let eye = V3::from(c.position.0);
        let fwd = (V3::from(c.target.0) - eye).norm();
        let up0 = if fwd.1.abs() > 0.999 { V3(0.0, 0.0, -1.0) } else { V3(0.0, 1.0, 0.0) };
        let right = fwd.cross(up0).norm();
        let up = right.cross(fwd);
        let len = ((eye - f.camera.eye).len() * 0.08).clamp(0.2, 4.0);
        let half_h = if c.orthographic() { (c.ortho_size as f32 / 2.0).min(len) } else { len * ((c.fov.clamp(1.0, 170.0) as f32).to_radians() / 2.0).tan() };
        let half_w = half_h * aspect;
        let center = eye + fwd * len;
        let corners = [center + right * half_w + up * half_h, center - right * half_w + up * half_h, center - right * half_w - up * half_h, center + right * half_w - up * half_h];
        for k in 0..4 {
            push(&mut cam_lines, if c.orthographic() { corners[k] - fwd * len } else { eye }, corners[k]);
            push(&mut cam_lines, corners[k], corners[(k + 1) % 4]);
        }
        // Which way is up.
        let top = center + up * (half_h * 1.5);
        push(&mut cam_lines, corners[0] * 0.8 + corners[1] * 0.2, top);
        push(&mut cam_lines, corners[1] * 0.8 + corners[0] * 0.2, top);
    }
    stroke(img, &cam_lines, CAMERA, 0.9, 1.2);
}

// ---------------------------------------------------------------------------------------------
// Aiming tools at objects

/// The evaluated objects at `t` and their world matrices.
fn evaluated(scene: &Scene3d, t: f64) -> (Vec<Object3d>, HashMap<String, M4>) {
    let objects = scene.objects_at(t);
    let world = super::world_matrices(&objects);
    (objects, world)
}

/// An object's own mesh at `t`, if it has one the editor can aim at (models only by file path).
fn pick_mesh(o: &Object3d, ctx: &shapes::Ctx) -> Option<Arc<Mesh>> {
    match &o.shape {
        Shape3d::Model { src } => {
            let path = std::path::Path::new(src);
            let parts = path.is_file().then(|| mesh::model(path).ok()).flatten()?;
            Some(Arc::new(shapes::merged(&parts)))
        }
        // Pictures are a card as wide as asked (their height isn't known here: square).
        Shape3d::Image { width, .. } => {
            mesh::shape(&Shape3d::Plane { width: *width, height: *width })
        }
        _ => shapes::build(o, ctx).map(|b| b.mesh),
    }
}

/// Nearest hit of a ray with a triangle (Möller–Trumbore), as the ray's parameter.
fn ray_triangle(o: V3, d: V3, a: V3, b: V3, c: V3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-6).then_some(t)
}

/// The id of the nearest object a ray (origin, unit direction; world space, e.g. from
/// [`ViewCamera::ray`]) hits at scene time `t`, tested against the drawn meshes (modifiers
/// applied). Lights, cameras, particles and groups themselves aren't hit.
pub fn pick(scene: &Scene3d, t: f64, ray: ([f64; 3], [f64; 3])) -> Option<String> {
    let (objects, world) = evaluated(scene, t);
    let ctx = shapes::Ctx { t, objects: &objects, world: &world };
    let (o, d) = (V3::from(ray.0), V3::from(ray.1));
    let mut best: Option<(f32, String)> = None;
    fn walk(objects: &[Object3d], f: &mut dyn FnMut(&Object3d)) {
        for o in objects {
            f(o);
            walk(&o.children, f);
        }
    }
    // Only what is shown at `t` (a hidden parent hides its children).
    fn visible_ids(objects: &[Object3d], t: f64, out: &mut HashSet<String>) {
        for o in objects.iter().filter(|o| o.visible_at(t)) {
            out.insert(o.id.clone());
            visible_ids(&o.children, t, out);
        }
    }
    let mut shown = HashSet::new();
    visible_ids(&objects, t, &mut shown);
    walk(&objects, &mut |obj| {
        if !shown.contains(&obj.id) {
            return;
        }
        let Some(mesh) = pick_mesh(obj, &ctx) else { return };
        let m = world.get(&obj.id).copied().unwrap_or(M4::I);
        let Some(inv) = m.inverse() else { return };
        let (lo, ld) = (inv.point3(o), inv.dir(d));
        for tri in mesh.index.as_chunks::<3>().0.iter() {
            let p = |i: u32| mesh.pos.get(i as usize).map(|p| V3(p[0], p[1], p[2]));
            let (Some(a), Some(b), Some(c)) = (p(tri[0]), p(tri[1]), p(tri[2])) else { continue };
            if let Some(hit) = ray_triangle(lo, ld, a, b, c)
                && best.as_ref().is_none_or(|(bt, _)| hit < *bt)
            {
                best = Some((hit, obj.id.clone()));
            }
        }
    });
    best.map(|(_, id)| id)
}

/// An object's box in world space at scene time `t` (lowest and highest corner): its own mesh,
/// or for groups and empties everything under them, or just its position.
pub fn object_bounds(scene: &Scene3d, t: f64, id: &str) -> Option<([f64; 3], [f64; 3])> {
    let (objects, world) = evaluated(scene, t);
    let ctx = shapes::Ctx { t, objects: &objects, world: &world };
    let obj = find_object(&objects, id)?;
    let mut b = (V3(f32::MAX, f32::MAX, f32::MAX), V3(f32::MIN, f32::MIN, f32::MIN));
    let add = |o: &Object3d, b: &mut (V3, V3)| {
        let Some(mesh) = pick_mesh(o, &ctx) else { return };
        if mesh.pos.is_empty() {
            return;
        }
        let m = world.get(&o.id).copied().unwrap_or(M4::I);
        let (lo, hi) = mesh.bounds();
        for c in super::corners(lo, hi) {
            let w = m.point3(c);
            b.0 = b.0.min(w);
            b.1 = b.1.max(w);
        }
    };
    add(obj, &mut b);
    if b.0.0 > b.1.0 {
        walk_objects(&obj.children, &mut |c| add(c, &mut b));
    }
    if b.0.0 > b.1.0 {
        let p = world.get(id)?.origin();
        b = (p, p);
    }
    let (lo, hi) = b;
    let f = |v: V3| [v.0 as f64, v.1 as f64, v.2 as f64];
    Some((f(lo), f(hi)))
}

/// An object's world matrix at scene time `t` (column-major: `m[column][row]`, translation in
/// `m[3]`; through its parents, constraints applied).
pub fn world_matrix(scene: &Scene3d, t: f64, id: &str) -> Option<[[f64; 4]; 4]> {
    let (_, world) = evaluated(scene, t);
    world.get(id).map(|m| m.to_f64())
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

    #[test]
    fn the_view_flies_looks_zooms_to_a_point_and_blends() {
        let v = ViewCamera::default();
        // Zooming towards a point keeps it under the same pixel.
        let at = [900.0, 200.0];
        let p = v.point_under(1280.0, 720.0, at[0], at[1]);
        for ortho in [false, true] {
            let mut z = ViewCamera { ortho, ..v };
            let p = z.point_under(1280.0, 720.0, at[0], at[1]);
            z.zoom_toward(0.5, p);
            let q = z.project(1280.0, 720.0, p).unwrap();
            assert!((q[0] - at[0]).abs() < 1e-6 && (q[1] - at[1]).abs() < 1e-6, "{ortho}: {q:?}");
        }
        let mut z = v;
        z.zoom_toward(0.5, p);
        assert!((z.distance() - v.distance() * 0.5).abs() < 1e-9);
        // Flying moves both ends; looking turns around the eye.
        let mut f = v;
        f.fly(1.0, 0.0, 0.5);
        let expected = add(v.basis().0, [0.0, 0.5, 0.0]);
        assert!(len(sub(sub(f.position, v.position), expected)) < 1e-9 && (f.distance() - v.distance()).abs() < 1e-9);
        let mut l = v;
        l.look(30.0, 0.0);
        assert_eq!(l.position, v.position);
        let turned = dot(norm(sub(l.target, l.position)), norm(sub(v.target, v.position)));
        let forward = v.basis().0;
        let expected_dot = forward[1].powi(2) + (1.0 - forward[1].powi(2)) * 30f64.to_radians().cos();
        assert!((turned - expected_dot).abs() < 1e-6, "{turned}");
        // Blends: the ends are the views, the middle keeps a sane distance.
        let mut top = v;
        top.align("top");
        top.target = [1.0, 0.0, 0.0];
        top.position = add(top.target, [0.0, 2.0, 0.0001]);
        let a = v.blend(&top, 0.0);
        let b = v.blend(&top, 1.0);
        assert!(len(sub(a.position, v.position)) < 1e-9 && len(sub(b.position, top.position)) < 1e-6, "{b:?}");
        let mid = v.blend(&top, 0.5);
        assert!(mid.distance() < v.distance() && mid.distance() > top.distance());
        let mut back = v;
        back.position = add(v.target, scale(sub(v.position, v.target), -1.0));
        assert!(v.blend(&back, 0.5).distance() > v.distance() * 0.99, "opposite views go round, not through");
        assert_eq!(top.aligned_axis(), Some("top"));
        assert_eq!(v.aligned_axis(), None);
    }
}
