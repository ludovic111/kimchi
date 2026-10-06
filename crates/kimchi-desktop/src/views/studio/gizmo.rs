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

    /// The visible part of an edge, including one that crosses behind the eye, with its
    /// endpoint depths for front-to-back picking. Orthographic clipping stays scale-relative.
    pub fn segment(&self,mut a:V3,mut b:V3)->Option<([f64;2],[f64;2],f64,f64)> {
        if a.iter().chain(&b).any(|v|!v.is_finite()) {return None;}
        let forward=math::norm(math::sub(self.cam.target,self.cam.position));
        let depth=|p|math::dot(math::sub(p,self.cam.position),forward);
        let (mut za,mut zb)=(depth(a),depth(b));
        if !za.is_finite() || !zb.is_finite() {return None;}
        let near=if self.cam.ortho {(za.max(zb)*f64::EPSILON).max(f64::MIN_POSITIVE)} else {1e-4};
        if za<=near && zb<=near {return None;}
        let cut=|p:V3,q:V3,zp:f64,zq:f64| {
            let scale=zp.abs().max(zq.abs());
            let t=((near/scale-zp/scale)/(zq/scale-zp/scale)).clamp(0.,1.);
            std::array::from_fn(|i|p[i]*(1.-t)+q[i]*t)
        };
        if za<=near {a=cut(a,b,za,zb);za=near;}
        else if zb<=near {b=cut(b,a,zb,za);zb=near;}
        let (pa,pb)=(self.project(a)?,self.project(b)?);
        pa.iter().chain(&pb).all(|v|v.is_finite()).then_some((pa,pb,za,zb))
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

/// The centre used for rotations and scales of one or several objects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pivot {
    #[default]
    Median,
    Active,
    Individual,
    Origin,
}

impl Pivot {
    pub const ALL: [Self; 4] = [Self::Median, Self::Active, Self::Individual, Self::Origin];

    pub fn name(self) -> &'static str {
        match self { Self::Median => "median", Self::Active => "active", Self::Individual => "individual", Self::Origin => "origin" }
    }

    pub fn label(self) -> &'static str {
        match self { Self::Median => "Selection centre", Self::Active => "Active object", Self::Individual => "Individual origins", Self::Origin => "World origin" }
    }

    pub fn parse(name: &str) -> Option<Self> { Self::ALL.into_iter().find(|p| p.name() == name) }
}

/// Where the gizmo stands: its pivot and its three axes (unit, world).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub pivot: V3,
    pub pivot_mode: Pivot,
    pub local: bool,
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
    /// Mesh vertices can rotate/scale around their selected island's centre.
    pub individual_pivot: Option<V3>,
    pub keys: Keyframes,
    base: Map<String, Value>,
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
        let props = ["position", "position.x", "position.y", "position.z", "x", "y", "z", "rotation", "rotation.x", "rotation.y", "rotation.z", "scale", "scale.x", "scale.y", "scale.z", "target", "target.x", "target.y", "target.z", "direction", "direction.x", "direction.y", "direction.z"];
        let base = match kind {
            ThingKind::Object => kimchi_core::motion::find_object(&s.objects, id).map(|o| model::base_props(&props, |n| o.get(n))),
            ThingKind::Camera => s.camera_by_id(id).map(|c| model::base_props(&props, |n| c.get(n))),
            ThingKind::Light => s.lights.iter().find(|l| l.id == *id).map(|l| model::base_props(&props, |n| l.get(n))),
        }.unwrap_or_default();
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
            individual_pivot: None,
            keys,
            base,
        });
    }
    out
}

impl Start {
    pub fn mesh_vertex(index:u32,position:V3,parent:M4,individual_pivot:Option<V3>) -> Self {
        Self {id:index.to_string(),kind:ThingKind::Object,position,rotation:[0.;3],scale:[1.;3],aim:[0.;3],
            world:math::mul(&parent,&math::translate(position)),parent,individual_pivot,keys:Default::default(),base:Map::new()}
    }
}

/// The gizmo's frame for some starts: at their middle, along the world's axes or (local) the
/// active one's own.
pub fn frame(starts: &[Start], local: bool, pivot_mode: Pivot) -> Option<Frame> {
    let last = starts.last()?;
    let n = starts.len() as f64;
    let pivot = starts.iter().fold([0.0; 3], |a, s| math::add(a, math::origin(&s.world)));
    let pivot = match pivot_mode {
        Pivot::Median | Pivot::Individual => math::scale(pivot, 1.0 / n),
        Pivot::Active => math::origin(&last.world),
        Pivot::Origin => [0.0; 3],
    };
    let local = local && last.kind == ThingKind::Object;
    let axes = if local {
        let [x, y, z] = math::axes(&last.world);
        [math::norm(x), math::norm(y), math::norm(z)]
    } else {
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    };
    Some(Frame { pivot, pivot_mode, local, axes })
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
    /// Incomplete or overflowing numeric input keeps the last valid preview and cannot commit.
    pub valid: bool,
    pub snap: bool,
    /// The last values reached: (thing, position, rotation, scale, aim).
    pub reached: Vec<(String, V3, V3, V3, V3)>,
    /// The amount shown (distance, degrees or factor).
    pub amount: f64,
}

const GRID: f64 = 0.25;

impl Session {
    pub fn new(kind: Kind, handle: Handle, frame: Frame, starts: Vec<Start>, mouse0: [f64; 2], view: &View3) -> Session {
        let mut s = Session { kind, handle, frame, starts, mouse0, anchor: None, typed: String::new(), valid: true, snap: false, reached: vec![], amount: 0.0 };
        s.anchor = s.grab_point(view, mouse0);
        s
    }

    /// Cycle an axis or plane through the preferred orientation, the other orientation, then
    /// unconstrained. Local axes belong to the active selected object.
    pub fn lock(&mut self, axis: usize, plane: bool, preferred_local: bool, view: &View3) {
        let h = if plane { Handle::Plane(axis) } else { Handle::Axis(axis) };
        let can_local = self.starts.last().is_some_and(|s| s.kind == ThingKind::Object);
        let preferred_local = preferred_local && can_local;
        let (handle, local) = if self.handle != h {
            (h, preferred_local)
        } else if can_local && self.frame.local == preferred_local {
            (h, !preferred_local)
        } else {
            (Handle::Free, preferred_local)
        };
        if let Some(frame) = frame(&self.starts, local, self.frame.pivot_mode) {
            self.frame.axes = frame.axes;
            self.frame.local = frame.local;
        }
        self.handle = handle;
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

    /// A complete, finite typed number, including scientific notation.
    fn typed_value(&self) -> Option<f64> {
        self.typed.parse::<f64>().ok().filter(|n| n.is_finite())
    }

    /// New values for every thing with the mouse at `m`.
    pub fn update(&mut self, view: &View3, m: [f64; 2]) {
        self.valid=self.typed.is_empty() || self.typed_value().is_some();
        if !self.valid {return;}
        let previous=std::mem::take(&mut self.reached);
        let amount=self.amount;
        self.update_values(view,m);
        self.valid=self.amount.is_finite() && self.reached.iter().all(|(_,p,r,s,a)| p.iter().chain(r).chain(s).chain(a).all(|n| n.is_finite()));
        if !self.valid || self.reached.is_empty() {self.reached=previous;self.amount=amount;}
    }

    fn update_values(&mut self, view: &View3, m: [f64; 2]) {
        let pivot = self.frame.pivot;
        match self.kind {
            Kind::Grab => {
                let mut d = match (self.typed_value(), self.axis()) {
                    (Some(v), Some(a)) if matches!(self.handle, Handle::Axis(_)) => math::scale(a, v),
                    (Some(v), Some(a)) if matches!(self.handle, Handle::Plane(_)) => {
                        let (right, up) = view.basis();
                        let project = |d| math::sub(d, math::scale(a, math::dot(d, a)));
                        let right = project(right);
                        math::scale(math::norm(if math::len(right) > 1e-9 { right } else { project(up) }), v)
                    }
                    (Some(v), _) => {
                        let (r, _) = view.basis();
                        math::scale(r, v)
                    }
                    _ => match (self.anchor, self.grab_point(view, m)) {
                        (Some(a), Some(b)) => math::sub(b, a),
                        _ => [0.0; 3],
                    },
                };
                if self.snap && self.typed.is_empty() {
                    d = match self.handle {
                        Handle::Axis(i) => {
                            let a = self.frame.axes[i];
                            math::scale(a, math::snap(math::dot(d, a), GRID))
                        }
                        Handle::Plane(i) => {
                            let normal = self.frame.axes[i];
                            let x = self.frame.axes[(i + 1) % 3];
                            let x = math::norm(math::sub(x, math::scale(normal, math::dot(x, normal))));
                            let y = math::cross(normal, x);
                            math::add(math::scale(x, math::snap(math::dot(d, x), GRID)), math::scale(y, math::snap(math::dot(d, y), GRID)))
                        }
                        Handle::Free => [math::snap(d[0], GRID), math::snap(d[1], GRID), math::snap(d[2], GRID)],
                    };
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
                let mut deg = match self.typed_value() {
                    Some(v) => v,
                    None => {
                        let Some(pc)=view.project(pivot) else {return};
                        let angle_of = |p: [f64; 2]| (p[1] - pc[1]).atan2(p[0] - pc[0]);
                        // Screen y points down, so a turn on screen is clockwise for positive angles.
                        let a = (angle_of(m) - angle_of(self.mouse0)).to_degrees();
                        -a
                    }
                };
                if self.snap && self.typed.is_empty() {
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
                    let pivot = if self.frame.pivot_mode == Pivot::Individual { s.individual_pivot.unwrap_or(world_pos) } else { pivot };
                    let moved = math::add(pivot, math::m3_apply(&r, math::sub(world_pos, pivot)));
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
                            // Turn its view direction and move the target with its eye.
                            let aim = math::add(pos, math::m3_apply(&r, math::sub(s.aim, s.position)));
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
                let mut k = match self.typed_value() {
                    Some(v) => v,
                    None => {
                        let Some(pc)=view.project(pivot) else {return};
                        let dist = |p: [f64; 2]| ((p[0] - pc[0]).powi(2) + (p[1] - pc[1]).powi(2)).sqrt();
                        dist(m) / dist(self.mouse0).max(1.0)
                    }
                };
                if self.snap && self.typed.is_empty() {
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
                    let pivot = if self.frame.pivot_mode == Pivot::Individual { s.individual_pivot.unwrap_or(world_pos) } else { pivot };
                    let off = math::sub(world_pos, pivot);
                    let moved = match self.axis() {
                        Some(a) if matches!(self.handle, Handle::Axis(_)) => math::add(world_pos, math::scale(a, math::dot(off, a) * (k - 1.0))),
                        Some(a) if matches!(self.handle, Handle::Plane(_)) => math::add(world_pos, math::scale(math::sub(off, math::scale(a, math::dot(off, a))), k - 1.0)),
                        _ => math::add(pivot, math::scale(off, k)),
                    };
                    let pos = math::add(s.position, math::dir(&math::inverse(&s.parent), math::sub(moved, world_pos)));
                    self.reached.push((s.id.clone(), pos, s.rotation, scale, s.aim));
                }
            }
        }
    }

    /// `motion.updateLayer` calls for the values reached (or the starting ones, to cancel).
    pub fn commands(&self, clip: kimchi_core::Id, time: f64, key: &str, cancel: bool) -> Vec<(String, Value)> {
        if !cancel && !self.valid {return vec![];}
        // Starting and escaping a modal tool without moving must not enqueue a scene write.
        if cancel && self.reached.is_empty() { return vec![]; }
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
                    if self.frame.pivot_mode != Pivot::Individual && (self.starts.len() > 1 || self.frame.pivot_mode == Pivot::Origin) {
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
                    if self.frame.pivot_mode != Pivot::Individual && (self.starts.len() > 1 || self.frame.pivot_mode == Pivot::Origin) {
                        props.extend(model::vec_props(&s.keys, "position", pos));
                    }
                }
            }
            if cancel { props = model::restore_props(&s.base, &s.keys, &props); }
            out.push(("motion.updateLayer".to_string(), json!({ "clipId": clip, "id": s.id, "props": props, "time": time, "coalesce": key })));
        }
        out
    }

    /// The live frame for the gizmo (it follows the mouse before the picture does).
    pub fn live_pivot(&self) -> V3 {
        if self.kind != Kind::Grab {
            return self.frame.pivot;
        }
        let Some(first) = self.starts.first() else { return self.frame.pivot };
        let moved = self.reached.iter().find(|r| r.0 == first.id).map(|r| r.1).unwrap_or(first.position);
        let delta = math::dir(&first.parent, math::sub(moved, first.position));
        math::add(self.frame.pivot, delta)
    }

    /// A line of help for the status (what is being done, the amount, the keys).
    pub fn status(&self) -> String {
        let what = match self.kind {
            Kind::Grab => "Move",
            Kind::Rotate => "Rotate",
            Kind::Scale => "Scale",
        };
        let space = if self.frame.local { "local " } else { "" };
        let along = match self.handle {
            Handle::Axis(i) => format!(" along {space}{}", ["X", "Y", "Z"][i]),
            Handle::Plane(i) => format!(" across {space}{}", ["X", "Y", "Z"][i]),
            Handle::Free => String::new(),
        };
        if !self.valid {
            return format!("{what}{along} [{}] · Enter a finite number within the transform's range · Backspace corrects · Esc cancels",self.typed);
        }
        let amount = match self.kind {
            Kind::Grab => math::compact_number(self.amount),
            Kind::Rotate => format!("{}°", math::compact_number(self.amount)),
            Kind::Scale => format!("×{}", math::compact_number(self.amount)),
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

    fn close(a: V3, b: V3) { assert!(math::len(math::sub(a, b)) < 1e-8, "{a:?} != {b:?}"); }

    #[test]
    fn numeric_transforms_work_when_the_selection_is_behind_the_view() {
        let scene=scene();
        let kimchi_core::Scene::Space(s)=&scene else {panic!()};
        let starts=starts(s,&scene,&["box".into()],0.,&model::worlds(s,0.));
        let frame=frame(&starts,false,Pivot::Median).unwrap();
        let mut view=view();view.cam.target=[0.,0.,20.];
        assert!(view.project(frame.pivot).is_none());
        for (kind,typed) in [(Kind::Grab,"2"),(Kind::Rotate,"90"),(Kind::Scale,"2")] {
            let mut session=Session::new(kind,Handle::Axis(0),frame,starts.clone(),[400.,300.],&view);
            session.typed=typed.into();session.update(&view,[0.,0.]);
            assert_eq!(session.reached.len(),1,"{kind:?} still applies a typed amount");
            let (_,position,rotation,scale,_)=&session.reached[0];
            match kind {
                Kind::Grab=>close(*position,[3.,0.,0.]),
                Kind::Rotate=>close(*rotation,[90.,0.,0.]),
                Kind::Scale=>close(*scale,[2.,1.,1.]),
            }
        }
    }

    #[test]
    fn transform_axis_locks_follow_local_orientation_cycle_and_keep_snapping_on_axis() {
        let scene = kimchi_core::Scene::from_json(&json!({"objects":[{"id":"box","type":"box","rotation":[0,0,30]}]})).unwrap();
        let kimchi_core::Scene::Space(s) = &scene else { panic!() };
        let starts = starts(s, &scene, &["box".into()], 0., &model::worlds(s, 0.));
        let local_x = [30_f64.to_radians().cos(), 0.5, 0.];
        for preferred in [false, true] {
            let frame = frame(&starts, preferred, Pivot::Median).unwrap();
            let mut move_ = Session::new(Kind::Grab, Handle::Free, frame, starts.clone(), [400.,300.], &view());
            move_.snap = true;
            for expected in [preferred, !preferred] {
                move_.lock(0, false, preferred, &view());
                let axis=if expected {local_x} else {[1.,0.,0.]};
                let mouse=view().project(math::scale(axis,1.1)).unwrap();
                move_.update(&view(), mouse);
                close(move_.reached[0].1, if expected { local_x } else { [1.,0.,0.] });
                assert_eq!(move_.frame.local, expected);
            }
            move_.lock(0, false, preferred, &view());
            assert_eq!(move_.handle, Handle::Free);
            // A plane lock projects typed motion and snapping into that plane.
            move_.lock(0, true, preferred, &view());
            move_.typed="1.1".into();
            move_.update(&view(), [0.,0.]);
            assert!(math::dot(move_.reached[0].1, move_.frame.axes[0]).abs() < 1e-8);
        }
    }

    #[test]
    fn numeric_transforms_keep_precision_and_hold_the_last_valid_preview() {
        let scene=scene();
        let kimchi_core::Scene::Space(s)=&scene else {panic!()};
        let starts=starts(s,&scene,&["box".into()],0.,&model::worlds(s,0.));
        let frame=frame(&starts,false,Pivot::Median).unwrap();
        let clip=kimchi_core::Id::new_v4();
        for kind in [Kind::Grab,Kind::Rotate,Kind::Scale] {
            let mut session=Session::new(kind,Handle::Axis(0),frame,starts.clone(),[400.,300.],&view());
            session.snap=true;
            session.typed="-3e-2".into();session.update(&view(),[10.,20.]);
            assert!(session.valid);
            let (_,pos,rot,scale,_)=&session.reached[0];
            match kind {
                Kind::Grab=>close(*pos,[0.97,0.,0.]),
                Kind::Rotate=>close(*rot,[-0.03,0.,0.]),
                Kind::Scale=>close(*scale,[-0.03,1.,1.]),
            }
            let previous=session.reached.clone();
            for typed in ["-",".","2e-","1e999","NaN","--2"] {
                session.typed=typed.into();session.update(&view(),[500.,700.]);
                assert!(!session.valid,"{typed}");
                assert_eq!(session.reached,previous,"incomplete input does not fall back to the mouse");
                assert!(session.commands(clip,0.,"gesture:test",false).is_empty());
                assert!(!session.commands(clip,0.,"gesture:test",true).is_empty(),"Escape can still restore the original state");
            }
            session.typed="+2.5e-4".into();session.update(&view(),[0.,0.]);
            assert!(session.valid);
            assert!(session.status().contains("+2.5e-4"));
            session.typed.clear();session.update(&view(),[500.,350.]);
            assert!(session.valid,"clearing the input resumes pointer control");
        }
        let mut scaled=starts;scaled[0].scale=[3.,1.,1.];
        let mut session=Session::new(Kind::Scale,Handle::Axis(0),frame,scaled,[400.,300.],&view());
        session.typed="1e308".into();session.update(&view(),[0.,0.]);
        assert!(!session.valid,"finite input can still overflow the resulting transform");
        assert!(session.commands(clip,0.,"gesture:test",false).is_empty());
    }

    #[test]
    fn transform_pivots_rotate_and_scale_the_selection_about_the_chosen_centre() {
        let scene = kimchi_core::Scene::from_json(&json!({"objects":[
            {"id":"a","type":"box","position":[1,0,0]}, {"id":"b","type":"box","position":[3,0,0]}
        ]})).unwrap();
        let kimchi_core::Scene::Space(s) = &scene else { panic!() };
        let starts = starts(s, &scene, &["a".into(), "b".into()], 0., &model::worlds(s, 0.));
        for (pivot, turned, scaled) in [
            (Pivot::Median, [[2.,-1.,0.],[2.,1.,0.]], [[0.,0.,0.],[4.,0.,0.]]),
            (Pivot::Active, [[3.,-2.,0.],[3.,0.,0.]], [[-1.,0.,0.],[3.,0.,0.]]),
            (Pivot::Individual, [[1.,0.,0.],[3.,0.,0.]], [[1.,0.,0.],[3.,0.,0.]]),
            (Pivot::Origin, [[0.,1.,0.],[0.,3.,0.]], [[2.,0.,0.],[6.,0.,0.]]),
        ] {
            let f = frame(&starts, false, pivot).unwrap();
            let mut rotate = Session::new(Kind::Rotate, Handle::Axis(2), f, starts.clone(), [500.,300.], &view());
            rotate.typed = "90".into(); rotate.update(&view(), [0.,0.]);
            let mut scale = Session::new(Kind::Scale, Handle::Free, f, starts.clone(), [500.,300.], &view());
            scale.typed = "2".into(); scale.update(&view(), [0.,0.]);
            for i in 0..2 { close(rotate.reached[i].1, turned[i]); close(scale.reached[i].1, scaled[i]); }
        }
    }

    #[test]
    fn transform_pivot_handles_single_objects_plane_scaling_and_cancel() {
        let scene = kimchi_core::Scene::from_json(&json!({"objects":[{"id":"a","type":"box","position":[1,2,3]}]})).unwrap();
        let kimchi_core::Scene::Space(s) = &scene else { panic!() };
        let starts = starts(s, &scene, &["a".into()], 0., &model::worlds(s, 0.));
        let f = frame(&starts, false, Pivot::Origin).unwrap();
        let mut scale = Session::new(Kind::Scale, Handle::Plane(2), f, starts.clone(), [500.,300.], &view());
        scale.typed = "2".into(); scale.update(&view(), [0.,0.]);
        close(scale.reached[0].1, [2.,4.,3.]); close(scale.reached[0].3, [2.,2.,1.]);
        let commands = scale.commands(kimchi_core::Id::new_v4(), 0., "pivot", false);
        assert_eq!(commands[0].1["props"]["position.x"], 2.);
        let cancel = scale.commands(kimchi_core::Id::new_v4(), 0., "pivot", true);
        assert_eq!(cancel[0].1["props"]["position.x"], 1.);
        assert_eq!(cancel[0].1["props"]["scale.x"], 1.);
        let f = frame(&starts, false, Pivot::Active).unwrap();
        let mut grab = Session::new(Kind::Grab, Handle::Axis(0), f, starts, [500.,300.], &view());
        grab.typed = "2".into(); grab.update(&view(), [0.,0.]);
        close(grab.live_pivot(), [3.,2.,3.]);
    }

    #[test]
    fn transform_pivot_moves_a_cameras_target_with_its_eye() {
        let scene = kimchi_core::Scene::from_json(&json!({"objects":[],"camera":{"position":[2,0,0],"target":[2,0,-1]}})).unwrap();
        let kimchi_core::Scene::Space(s) = &scene else { panic!() };
        let starts = starts(s, &scene, &["camera".into()], 0., &model::worlds(s, 0.));
        let f = frame(&starts, false, Pivot::Origin).unwrap();
        let mut rotate = Session::new(Kind::Rotate, Handle::Axis(2), f, starts, [500.,300.], &view());
        rotate.typed = "90".into(); rotate.update(&view(), [0.,0.]);
        close(rotate.reached[0].1, [0.,2.,0.]); close(rotate.reached[0].4, [0.,2.,-1.]);
    }

    #[test]
    fn moving_along_an_axis_follows_the_mouse() {
        let sc = scene();
        let kimchi_core::Scene::Space(s) = &sc else { panic!() };
        let w = model::worlds(s, 0.0);
        let st = starts(s, &sc, &["box".into()], 0.0, &w);
        let f = frame(&st, false, Pivot::Median).unwrap();
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
        let f = frame(&st, false, Pivot::Median).unwrap();
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
