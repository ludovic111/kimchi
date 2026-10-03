//! The 3D transform gizmo and the maths of moving, turning and scaling things in the view:
//! handles in screen space (move arrows and plane squares, rotate rings, scale ends), hit
//! testing, and turning a mouse position into new positions, rotations and scales, the same for
//! a gizmo drag and for Blender's G / R / S (with X / Y / Z to lock an axis and typed numbers).

use std::collections::HashMap;

use kimchi_core::Keyframes;
use kimchi_media::render::space::viewport::ViewCamera;
use serde_json::{Map, Value, json};

use super::math::{self, M3, M4, V3};
use super::model;

/// Axis colours (the usual red, green, blue of 3D tools).
pub const AXIS_COLORS: [u32; 3] = [0xf0565c, 0x7bd88f, 0x5aa7ff];

/// A 3D view on screen: the camera and the rectangle it fills (screen pixels).
#[derive(Clone, Copy, Debug)]
pub struct View3 {
    pub cam: ViewCamera,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl View3 {
    pub fn project(&self, p: V3) -> Option<[f64; 2]> {
        self.cam.project(self.w, self.h, p).map(|q| [q[0] + self.x, q[1] + self.y])
    }

    pub fn ray(&self, s: [f64; 2]) -> (V3, V3) {
        self.cam.ray(self.w, self.h, s[0] - self.x, s[1] - self.y)
    }

    /// Unit vector from the scene towards the viewer at `p`.
    pub fn toward_viewer(&self, p: V3) -> V3 {
        if self.cam.ortho { math::norm(math::sub(self.cam.position, self.cam.target)) } else { math::norm(math::sub(self.cam.position, p)) }
    }

    /// World units per screen pixel at `p`.
    pub fn units_per_pixel(&self, p: V3) -> f64 {
        let span = if self.cam.ortho {
            self.cam.ortho_size
        } else {
            let d = math::len(math::sub(p, self.cam.position)).max(1e-3);
            2.0 * d * (self.cam.fov.to_radians() / 2.0).tan()
        };
        span / self.h.max(1.0)
    }

    /// The view's right and up directions in the world.
    pub fn basis(&self) -> (V3, V3) {
        let f = math::norm(math::sub(self.cam.target, self.cam.position));
        let up0 = if f[1].abs() > 0.999 { [0.0, 0.0, -1.0] } else { [0.0, 1.0, 0.0] };
        let r = math::norm(math::cross(f, up0));
        (r, math::cross(r, f))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Grab,
    Rotate,
    Scale,
}

/// What part of the gizmo is under the mouse (or locked with X / Y / Z).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Along one axis.
    Axis(usize),
    /// In the plane across one axis (the two others).
    Plane(usize),
    /// Free: in the view's plane (move), around the view's axis (rotate), all axes (scale).
    Free,
}

/// A polyline of the gizmo with what it does.
pub struct Part {
    pub handle: Handle,
    pub points: Vec<[f64; 2]>,
    pub color: u32,
    pub closed: bool,
    /// A filled shape (plane squares, arrow heads, scale cubes).
    pub fill: bool,
}

/// Where the gizmo stands: its pivot and its three axes (unit, world).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub pivot: V3,
    pub axes: [V3; 3],
}

/// The gizmo's lines for `kind`, about 90 px tall on screen.
pub fn parts(f: &Frame, kind: Kind, view: &View3) -> Vec<Part> {
    let Some(c) = view.project(f.pivot) else { return vec![] };
    let len = 90.0 * view.units_per_pixel(f.pivot);
    let at = |v: V3| view.project(math::add(f.pivot, v));
    let mut out = vec![];
    match kind {
        Kind::Grab | Kind::Scale => {
            for (i, axis) in f.axes.iter().enumerate() {
                let Some(end) = at(math::scale(*axis, len)) else { continue };
                out.push(Part { handle: Handle::Axis(i), points: vec![c, end], color: AXIS_COLORS[i], closed: false, fill: false });
                // Heads: a triangle for moving, a square for scaling.
                let dir = [end[0] - c[0], end[1] - c[1]];
                let l = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt().max(1e-6);
                let (dx, dy) = (dir[0] / l, dir[1] / l);
                let (nx, ny) = (-dy, dx);
                let head = if kind == Kind::Grab {
                    vec![[end[0] + dx * 12.0, end[1] + dy * 12.0], [end[0] + nx * 5.0, end[1] + ny * 5.0], [end[0] - nx * 5.0, end[1] - ny * 5.0]]
                } else {
                    let s = 5.0;
                    vec![[end[0] - s, end[1] - s], [end[0] + s, end[1] - s], [end[0] + s, end[1] + s], [end[0] - s, end[1] + s]]
                };
                out.push(Part { handle: Handle::Axis(i), points: head, color: AXIS_COLORS[i], closed: true, fill: true });
            }
            if kind == Kind::Grab {
                for (i, &color) in AXIS_COLORS.iter().enumerate() {
                    let (a, b) = (f.axes[(i + 1) % 3], f.axes[(i + 2) % 3]);
                    let q = |u: f64, v: f64| at(math::add(math::scale(a, u * len), math::scale(b, v * len)));
                    let pts: Option<Vec<[f64; 2]>> = [q(0.25, 0.25), q(0.45, 0.25), q(0.45, 0.45), q(0.25, 0.45)].into_iter().collect();
                    if let Some(pts) = pts {
                        out.push(Part { handle: Handle::Plane(i), points: pts, color, closed: true, fill: true });
                    }
                }
            }
            let ring: Vec<[f64; 2]> = (0..24).map(|k| {
                let a = k as f64 / 24.0 * std::f64::consts::TAU;
                [c[0] + a.cos() * 9.0, c[1] + a.sin() * 9.0]
            }).collect();
            out.push(Part { handle: Handle::Free, points: ring, color: 0xffffff, closed: true, fill: false });
        }
        Kind::Rotate => {
            for (i, axis) in f.axes.iter().enumerate() {
                let (a, b) = (f.axes[(i + 1) % 3], f.axes[(i + 2) % 3]);
                let pts: Option<Vec<[f64; 2]>> = (0..64)
                    .map(|k| {
                        let t = k as f64 / 64.0 * std::f64::consts::TAU;
                        at(math::add(math::scale(a, t.cos() * len), math::scale(b, t.sin() * len)))
                    })
                    .collect();
                let _ = axis;
                if let Some(pts) = pts {
                    out.push(Part { handle: Handle::Axis(i), points: pts, color: AXIS_COLORS[i], closed: true, fill: false });
                }
            }
            let r = 90.0 * 1.15;
            let ring: Vec<[f64; 2]> = (0..64).map(|k| {
                let a = k as f64 / 64.0 * std::f64::consts::TAU;
                [c[0] + a.cos() * r, c[1] + a.sin() * r]
            }).collect();
            out.push(Part { handle: Handle::Free, points: ring, color: 0xffffff, closed: true, fill: false });
        }
    }
    out
}

/// The handle under the mouse, if any (fills first, then the nearest line within 7 px).
pub fn hit(parts: &[Part], m: [f64; 2]) -> Option<Handle> {
    for p in parts.iter().filter(|p| p.fill) {
        if math::in_polygon(m, &p.points) {
            return Some(p.handle);
        }
    }
    let mut best: Option<(f64, Handle)> = None;
    for p in parts {
        let n = p.points.len();
        let segs = if p.closed { n } else { n.saturating_sub(1) };
        for i in 0..segs {
            let d = math::seg_dist(m, p.points[i], p.points[(i + 1) % n]);
            if d < 7.0 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, p.handle));
            }
        }
    }
    // Inside the move ring counts as free moving.
    if best.is_none()
        && let Some(ring) = parts.iter().find(|p| p.handle == Handle::Free && p.points.len() == 24)
        && math::in_polygon(m, &ring.points)
    {
        return Some(Handle::Free);
    }
    best.map(|(_, h)| h)
}

// ---------------------------------------------------------------------------------------------
// Moving things

/// What a transformed thing is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThingKind {
    Object,
    Light,
    Camera,
}

/// A thing as it was when the transform started.
#[derive(Clone, Debug)]
pub struct Start {
    pub id: String,
    pub kind: ThingKind,
    /// Own (local) values, keyframes applied.
    pub position: V3,
    pub rotation: V3,
    pub scale: V3,
    /// Cameras' target, lights' direction.
    pub aim: V3,
    pub world: M4,
    pub parent: M4,
    pub keys: Keyframes,
}

/// The things to transform (the selection's objects, lights and cameras) at scene time `t`.
pub fn starts(s: &kimchi_core::motion::Scene3d, scene: &kimchi_core::Scene, selection: &[String], t: f64, worlds: &HashMap<String, M4>) -> Vec<Start> {
    let mut out = vec![];
    for id in selection {
        let keys = model::keyframes(scene, id).unwrap_or_default();
        let v3 = |name: &str| model::vec3_at(scene, id, name, t);
        let kind = match model::item(scene, id) {
            Some(model::Item::Object(_)) => ThingKind::Object,
            Some(model::Item::Light) => ThingKind::Light,
            Some(model::Item::Camera) => ThingKind::Camera,
            _ => continue,
        };
        // Children of something else selected move with it already.
        if kind == ThingKind::Object
            && selection.iter().any(|other| other != id && kimchi_core::motion::find_object(&s.objects, other).is_some_and(|o| kimchi_core::motion::find_object(&o.children, id).is_some()))
        {
            continue;
        }
        let parent = match model::parent3d(s, id).filter(|p| !p.is_empty()) {
            Some(p) => worlds.get(&p).copied().unwrap_or(math::IDENTITY),
            None => math::IDENTITY,
        };
        let world = worlds.get(id).copied().unwrap_or(math::IDENTITY);
        out.push(Start {
            id: id.clone(),
            kind,
            position: v3("position").unwrap_or_default(),
            rotation: if kind == ThingKind::Object { v3("rotation").unwrap_or_default() } else { [0.0; 3] },
            scale: if kind == ThingKind::Object { v3("scale").unwrap_or([1.0; 3]) } else { [1.0; 3] },
            aim: match kind {
                ThingKind::Camera => v3("target").unwrap_or_default(),
                ThingKind::Light => v3("direction").unwrap_or([0.0, -1.0, 0.0]),
                ThingKind::Object => [0.0; 3],
            },
            world,
            parent,
            keys,
        });
    }
    out
}

/// The gizmo's frame for some starts: at their middle, along the world's axes or (local) the
/// active one's own.
pub fn frame(starts: &[Start], local: bool) -> Option<Frame> {
    let last = starts.last()?;
    let n = starts.len() as f64;
    let pivot = starts.iter().fold([0.0; 3], |a, s| math::add(a, math::origin(&s.world)));
    let pivot = math::scale(pivot, 1.0 / n);
    let axes = if local && last.kind == ThingKind::Object {
        let [x, y, z] = math::axes(&last.world);
        [math::norm(x), math::norm(y), math::norm(z)]
    } else {
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    };
    Some(Frame { pivot, axes })
}

/// A transform in progress.
#[derive(Clone, Debug)]
pub struct Session {
    pub kind: Kind,
    pub handle: Handle,
    pub frame: Frame,
    pub starts: Vec<Start>,
    /// Mouse where it began (screen).
    pub mouse0: [f64; 2],
    /// What the mouse first pointed at, along the axis or in the plane (for moves).
    anchor: Option<V3>,
    pub typed: String,
    pub snap: bool,
    /// The last values reached: (thing, position, rotation, scale, aim).
    pub reached: Vec<(String, V3, V3, V3, V3)>,
    /// The amount shown (distance, degrees or factor).
    pub amount: f64,
}

const GRID: f64 = 0.25;

impl Session {
    pub fn new(kind: Kind, handle: Handle, frame: Frame, starts: Vec<Start>, mouse0: [f64; 2], view: &View3) -> Session {
        let mut s = Session { kind, handle, frame, starts, mouse0, anchor: None, typed: String::new(), snap: false, reached: vec![], amount: 0.0 };
        s.anchor = s.grab_point(view, mouse0);
        s
    }

    /// Locks (or, pressed again, frees) an axis: X / Y / Z; with Shift the plane across it.
    pub fn lock(&mut self, axis: usize, plane: bool, view: &View3) {
        let h = if plane { Handle::Plane(axis) } else { Handle::Axis(axis) };
        self.handle = if self.handle == h { Handle::Free } else { h };
        self.anchor = self.grab_point(view, self.mouse0);
    }

    /// The axis for an axis-locked move or turn.
    fn axis(&self) -> Option<V3> {
        match self.handle {
            Handle::Axis(i) | Handle::Plane(i) => Some(self.frame.axes[i]),
            Handle::Free => None,
        }
    }

    /// Where the mouse points on the move's line or plane.
    fn grab_point(&self, view: &View3, m: [f64; 2]) -> Option<V3> {
        let (o, d) = view.ray(m);
        let p = self.frame.pivot;
        match self.handle {
            Handle::Axis(i) => {
                let a = self.frame.axes[i];
                math::ray_line(o, d, p, a).map(|t| math::add(p, math::scale(a, t)))
            }
            Handle::Plane(i) => math::ray_plane(o, d, p, self.frame.axes[i]),
            Handle::Free => math::ray_plane(o, d, p, view.toward_viewer(p)),
        }
    }

    /// The typed number, if any ("-", "." alone don't count).
    fn typed_value(&self) -> Option<f64> {
        self.typed.parse::<f64>().ok()
    }

    /// New values for every thing with the mouse at `m`.
    pub fn update(&mut self, view: &View3, m: [f64; 2]) {
        let pivot = self.frame.pivot;
        let Some(pc) = view.project(pivot) else { return };
        self.reached.clear();
        match self.kind {
            Kind::Grab => {
                let mut d = match (self.typed_value(), self.axis()) {
                    (Some(v), Some(a)) if matches!(self.handle, Handle::Axis(_)) => math::scale(a, v),
                    (Some(v), _) => {
                        let (r, _) = view.basis();
                        math::scale(r, v)
                    }
                    _ => match (self.anchor, self.grab_point(view, m)) {
                        (Some(a), Some(b)) => math::sub(b, a),
                        _ => [0.0; 3],
                    },
                };
                if self.snap {
                    d = [math::snap(d[0], GRID), math::snap(d[1], GRID), math::snap(d[2], GRID)];
                }
                self.amount = math::len(d);
                for s in &self.starts {
                    let local_d = math::dir(&math::inverse(&s.parent), d);
                    let pos = math::add(s.position, local_d);
                    let aim = if s.kind == ThingKind::Camera { math::add(s.aim, local_d) } else { s.aim };
                    self.reached.push((s.id.clone(), pos, s.rotation, s.scale, aim));
                }
            }
            Kind::Rotate => {
                let angle_of = |p: [f64; 2]| (p[1] - pc[1]).atan2(p[0] - pc[0]);
                let mut deg = match self.typed_value() {
                    Some(v) => v,
                    None => {
                        // Screen y points down, so a turn on screen is clockwise for positive angles.
                        let a = (angle_of(m) - angle_of(self.mouse0)).to_degrees();
                        -a
                    }
                };
                if self.snap {
                    deg = math::snap(deg, 15.0);
                }
                let toward = view.toward_viewer(pivot);
                let axis = match self.axis() {
                    Some(a) => {
                        // Turn the way the mouse goes as seen from here.
                        if math::dot(a, toward) < 0.0 && self.typed_value().is_none() { math::scale(a, -1.0) } else { a }
                    }
                    None => toward,
                };
                self.amount = deg;
                let r = math::axis_angle(axis, deg);
                for s in &self.starts {
                    let world_pos = math::origin(&s.world);
                    let moved = if self.starts.len() > 1 { math::add(pivot, math::m3_apply(&r, math::sub(world_pos, pivot))) } else { world_pos };
                    let pos = math::add(s.position, math::dir(&math::inverse(&s.parent), math::sub(moved, world_pos)));
                    match s.kind {
                        ThingKind::Object => {
                            let p = math::rotation_of(&s.parent);
                            let l = math::euler_to_m3(s.rotation);
                            let pt = math::m3_transpose(&p);
                            let new = math::m3_mul(&pt, &math::m3_mul(&r, &math::m3_mul(&p, &l)));
                            let rot = math::m3_to_euler(&new, s.rotation);
                            self.reached.push((s.id.clone(), pos, rot, s.scale, s.aim));
                        }
                        ThingKind::Camera => {
                            // The camera turns on the spot: its target swings around it.
                            let eye = s.position;
                            let aim = math::add(eye, math::m3_apply(&r, math::sub(s.aim, eye)));
                            self.reached.push((s.id.clone(), pos, s.rotation, s.scale, aim));
                        }
                        ThingKind::Light => {
                            let aim = math::m3_apply(&r, s.aim);
                            self.reached.push((s.id.clone(), pos, s.rotation, s.scale, aim));
                        }
                    }
                }
            }
            Kind::Scale => {
                let dist = |p: [f64; 2]| ((p[0] - pc[0]).powi(2) + (p[1] - pc[1]).powi(2)).sqrt();
                let mut k = match self.typed_value() {
                    Some(v) => v,
                    None => dist(m) / dist(self.mouse0).max(1.0),
                };
                if self.snap {
                    k = math::snap(k, 0.1);
                }
                self.amount = k;
                for s in &self.starts {
                    let factor = match self.handle {
                        Handle::Free => [k; 3],
                        Handle::Axis(i) | Handle::Plane(i) => {
                            // The object's own axis closest to the locked one.
                            let world_axis = self.frame.axes[i];
                            let own = math::axes(&s.world);
                            let j = (0..3).max_by(|a, b| math::dot(math::norm(own[*a]), world_axis).abs().total_cmp(&math::dot(math::norm(own[*b]), world_axis).abs())).unwrap_or(i);
                            let mut f = if matches!(self.handle, Handle::Plane(_)) { [k; 3] } else { [1.0; 3] };
                            f[j] = if matches!(self.handle, Handle::Plane(_)) { 1.0 } else { k };
                            f
                        }
                    };
                    let scale = [s.scale[0] * factor[0], s.scale[1] * factor[1], s.scale[2] * factor[2]];
                    let world_pos = math::origin(&s.world);
                    let pos = if self.starts.len() > 1 {
                        let off = math::sub(world_pos, pivot);
                        let moved = match self.axis() {
                            Some(a) if matches!(self.handle, Handle::Axis(_)) => math::add(world_pos, math::scale(a, math::dot(off, a) * (k - 1.0))),
                            _ => math::add(pivot, math::scale(off, k)),
                        };
                        math::add(s.position, math::dir(&math::inverse(&s.parent), math::sub(moved, world_pos)))
                    } else {
                        s.position
                    };
                    self.reached.push((s.id.clone(), pos, s.rotation, scale, s.aim));
                }
            }
        }
    }

    /// `motion.updateLayer` calls for the values reached (or the starting ones, to cancel).
    pub fn commands(&self, clip: kimchi_core::Id, time: f64, key: &str, cancel: bool) -> Vec<(String, Value)> {
        let mut out = vec![];
        for s in &self.starts {
            let (pos, rot, scale, aim) = if cancel {
                (s.position, s.rotation, s.scale, s.aim)
            } else {
                match self.reached.iter().find(|r| r.0 == s.id) {
                    Some((_, p, r, sc, a)) => (*p, *r, *sc, *a),
                    None => continue,
                }
            };
            let mut props = Map::new();
            match self.kind {
                Kind::Grab => {
                    props.extend(model::vec_props(&s.keys, "position", pos));
                    if s.kind == ThingKind::Camera {
                        props.extend(model::vec_props(&s.keys, "target", aim));
                    }
                }
                Kind::Rotate => {
                    if self.starts.len() > 1 {
                        props.extend(model::vec_props(&s.keys, "position", pos));
                    }
                    match s.kind {
                        ThingKind::Object => props.extend(model::vec_props(&s.keys, "rotation", rot)),
                        ThingKind::Camera => props.extend(model::vec_props(&s.keys, "target", aim)),
                        ThingKind::Light => props.extend(model::vec_props(&s.keys, "direction", aim)),
                    }
                }
                Kind::Scale => {
                    if s.kind != ThingKind::Object {
                        continue;
                    }
                    props.extend(model::vec_props(&s.keys, "scale", scale));
                    if self.starts.len() > 1 {
                        props.extend(model::vec_props(&s.keys, "position", pos));
                    }
                }
            }
            out.push(("motion.updateLayer".to_string(), json!({ "clipId": clip, "id": s.id, "props": props, "time": time, "coalesce": key })));
        }
        out
    }

    /// The live frame for the gizmo (it follows the mouse before the picture does).
    pub fn live_pivot(&self) -> V3 {
        if self.kind != Kind::Grab {
            return self.frame.pivot;
        }
        let n = self.starts.len().max(1) as f64;
        let mut sum = [0.0; 3];
        for s in &self.starts {
            let moved = self.reached.iter().find(|r| r.0 == s.id).map(|r| r.1).unwrap_or(s.position);
            let d = math::dir(&s.parent, math::sub(moved, s.position));
            sum = math::add(sum, math::add(math::origin(&s.world), d));
        }
        math::scale(sum, 1.0 / n)
    }

    /// A line of help for the status (what is being done, the amount, the keys).
    pub fn status(&self) -> String {
        let what = match self.kind {
            Kind::Grab => "Move",
            Kind::Rotate => "Rotate",
            Kind::Scale => "Scale",
        };
        let along = match self.handle {
            Handle::Axis(i) => format!(" along {}", ["X", "Y", "Z"][i]),
            Handle::Plane(i) => format!(" across {}", ["X", "Y", "Z"][i]),
            Handle::Free => String::new(),
        };
        let amount = match self.kind {
            Kind::Grab => format!("{:.3}", self.amount),
            Kind::Rotate => format!("{:.1}°", self.amount),
            Kind::Scale => format!("×{:.3}", self.amount),
        };
        let typed = if self.typed.is_empty() { String::new() } else { format!(" [{}]", self.typed) };
        format!("{what}{along}: {amount}{typed} · X/Y/Z lock an axis · type a number · Enter or click to confirm · Esc or right-click to cancel · Ctrl snaps")
    }
}

/// Rotation of a frame's axes as a 3×3 (for tests and the local gizmo).
#[allow(dead_code)]
pub fn frame_rotation(f: &Frame) -> M3 {
    let [x, y, z] = f.axes;
    [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view() -> View3 {
        View3 { cam: ViewCamera { position: [0.0, 0.0, 10.0], target: [0.0, 0.0, 0.0], fov: 40.0, ortho: false, ortho_size: 6.0 }, x: 0.0, y: 0.0, w: 800.0, h: 600.0 }
    }

    fn scene() -> kimchi_core::Scene {
        kimchi_core::Scene::from_json(&json!({"objects": [{"id": "box", "type": "box", "position": [1, 0, 0]}]})).unwrap()
    }

    #[test]
    fn moving_along_an_axis_follows_the_mouse() {
        let sc = scene();
        let kimchi_core::Scene::Space(s) = &sc else { panic!() };
        let w = model::worlds(s, 0.0);
        let st = starts(s, &sc, &["box".into()], 0.0, &w);
        let f = frame(&st, false).unwrap();
        let v = view();
        let p0 = v.project(f.pivot).unwrap();
        let mut sess = Session::new(Kind::Grab, Handle::Axis(0), f, st, p0, &v);
        // One unit to the right on screen.
        let p1 = v.project([2.0, 0.0, 0.0]).unwrap();
        sess.update(&v, [p1[0], p1[1] + 30.0]);
        let (_, pos, ..) = &sess.reached[0];
        assert!((pos[0] - 2.0).abs() < 0.01 && pos[1].abs() < 1e-9, "{pos:?}");
        // Typed numbers win.
        sess.typed = "-3".into();
        sess.update(&v, p1);
        assert!((sess.reached[0].1[0] + 2.0).abs() < 1e-9);
        let cmds = sess.commands(kimchi_core::Id::new_v4(), 0.0, "k", false);
        assert_eq!(cmds[0].1["props"]["position.x"], -2.0);
        assert_eq!(cmds[0].1["coalesce"], "k");
    }

    #[test]
    fn rotating_and_scaling_change_the_right_values() {
        let sc = scene();
        let kimchi_core::Scene::Space(s) = &sc else { panic!() };
        let w = model::worlds(s, 0.0);
        let st = starts(s, &sc, &["box".into()], 0.0, &w);
        let f = frame(&st, false).unwrap();
        let v = view();
        let mut sess = Session::new(Kind::Rotate, Handle::Axis(2), f, st.clone(), [500.0, 300.0], &v);
        sess.typed = "90".into();
        sess.update(&v, [0.0, 0.0]);
        assert!((sess.reached[0].2[2] - 90.0).abs() < 1e-6, "{:?}", sess.reached[0].2);
        let mut sess = Session::new(Kind::Scale, Handle::Free, f, st, [500.0, 300.0], &v);
        sess.typed = "2".into();
        sess.update(&v, [0.0, 0.0]);
        assert_eq!(sess.reached[0].3, [2.0; 3]);
        let parts = parts(&f, Kind::Grab, &v);
        let end = parts.iter().find(|p| p.handle == Handle::Axis(0) && !p.fill).unwrap().points[1];
        assert_eq!(hit(&parts, [end[0] - 20.0, end[1]]), Some(Handle::Axis(0)));
    }
}
