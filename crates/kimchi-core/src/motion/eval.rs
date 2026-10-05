//! A scene at an instant: what the renderers draw. Keyframes first, then expressions (formulas
//! that can read time, the keyframed value and other things' properties), then constraints
//! (lookAt, followPath…), so a constraint always has the last word.
//!
//! Constraints work in world space across the hierarchy: world matrices are computed here in
//! f64 (the same `translation × rotation z·y·x × scale` the renderer uses, parents first), each
//! constraint changes the world pose, and the result is written back as the object's own local
//! position, rotation and scale, so the renderer draws it unchanged. An object's front is its
//! +z side (where text, pictures and glTF models face): `lookAt` and an aligned `followPath` turn
//! +z towards the target or along the curve, keeping +y as upright as possible. Cameras and lights
//! look down their −z: on them `lookAt` sets the camera's `target` or the light's `direction`.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use super::*;
use crate::anim::Keyframe;
use crate::expr;
use crate::motion::curve::Polyline;

/// How deep `prop()` may read through other things' formulas.
pub const MAX_PROP_DEPTH: usize = 8;

/// What evaluation needs beyond the time: the frame rate (`fps`, `frame`, random numbers change
/// once a frame) and the length `duration` reports (default: until the last keyframe or layer
/// end, at least 1 s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvalOptions {
    pub fps: f64,
    pub duration: Option<f64>,
}

impl Default for EvalOptions {
    fn default() -> Self {
        EvalOptions { fps: 30.0, duration: None }
    }
}

impl EvalOptions {
    fn fps(&self) -> f64 {
        if self.fps.is_finite() && self.fps > 0.0 { self.fps } else { 30.0 }
    }
}

// ---------------------------------------------------------------------------------------------
// 2D

impl Scene2d {
    /// The scene's layers at scene time `t`.
    pub fn layers_at(&self, t: f64) -> Vec<Layer> {
        self.layers_at_with(t, &EvalOptions::default())
    }

    /// The scene's layers at scene time `t`, with the project's frame rate and the clip's length.
    pub fn layers_at_with(&self, t: f64, opts: &EvalOptions) -> Vec<Layer> {
        let world = Flat::new(self, opts);
        flat_layers(&world, &self.layers, t, Dur::Scene)
    }

    /// A composition's layers at its own time `t` (None when there is no such composition).
    pub fn comp_layers_at(&self, comp: &str, t: f64) -> Option<Vec<Layer>> {
        self.comp_layers_at_with(comp, t, &EvalOptions::default())
    }

    pub fn comp_layers_at_with(&self, comp: &str, t: f64, opts: &EvalOptions) -> Option<Vec<Layer>> {
        let c = self.composition(comp)?;
        let world = Flat::new(self, opts);
        Some(flat_layers(&world, &c.layers, t, Dur::Comp(c)))
    }

    /// How long the scene runs on its own: its last keyframe or layer end (at least 1 s).
    pub fn natural_length(&self) -> f64 {
        let mut t = 1.0f64;
        for list in self.keyframes.values() {
            if let Some(k) = list.last() {
                t = t.max(k.time);
            }
        }
        walk_layers(&self.layers, &mut |l| {
            for list in l.keyframes.values() {
                if let Some(k) = list.last() {
                    t = t.max(k.time);
                }
            }
            if let Some(e) = l.end {
                t = t.max(e);
            }
        });
        t
    }
}

fn flat_layers(world: &Flat, list: &[Layer], t: f64, dur: Dur) -> Vec<Layer> {
    list.iter()
        .enumerate()
        .map(|(i, l)| {
            let mut out = l.clone();
            for (name, keys) in &l.keyframes {
                if let Some(v) = value_at(keys, t) {
                    // Validated when the scene was set; a bad key in an old file is just skipped.
                    let _ = out.set(name, &v);
                }
            }
            out.keyframes.clear();
            express(world, Item::Layer(l), &l.id, &l.expressions, &mut out, t, (i + 1) as f64, dur);
            if let LayerKind::Group { layers } = &l.kind {
                out.kind = LayerKind::Group { layers: flat_layers(world, layers, t, dur) };
            }
            out
        })
        .collect()
}

/// A 2D scene as seen by `prop()`.
struct Flat<'a> {
    scene: &'a Scene2d,
    fps: f64,
    duration: Option<f64>,
    length: OnceCell<f64>,
}

impl<'a> Flat<'a> {
    fn new(scene: &'a Scene2d, opts: &EvalOptions) -> Self {
        Flat { scene, fps: opts.fps(), duration: opts.duration, length: OnceCell::new() }
    }
}

impl World for Flat<'_> {
    fn find(&self, id: &str) -> Option<Found<'_>> {
        if id == "scene" {
            return Some(Found { item: Item::Flat(self.scene), index: 1.0, dur: Dur::Scene });
        }
        if let Some((l, i)) = find_indexed(&self.scene.layers, id) {
            return Some(Found { item: Item::Layer(l), index: i as f64, dur: Dur::Scene });
        }
        self.scene.compositions.iter().find_map(|c| find_indexed(&c.layers, id).map(|(l, i)| Found { item: Item::Layer(l), index: i as f64, dur: Dur::Comp(c) }))
    }

    fn keyed(&self, item: &Item, name: &str, t: f64) -> Option<KeyValue> {
        keyed_generic(item, name, t)
    }

    fn ids(&self) -> Vec<String> {
        let mut out = vec![];
        walk_layers(&self.scene.layers, &mut |l| out.push(l.id.clone()));
        for c in &self.scene.compositions {
            walk_layers(&c.layers, &mut |l| out.push(l.id.clone()));
        }
        out
    }

    fn fps(&self) -> f64 {
        self.fps
    }

    fn duration(&self, dur: Dur) -> f64 {
        match dur {
            Dur::Comp(c) => c.length(),
            Dur::Scene => self.duration.unwrap_or_else(|| *self.length.get_or_init(|| self.scene.natural_length())),
        }
    }
}

/// A layer by id among `list` and its groups, with its place among its siblings (from 1).
fn find_indexed<'a>(list: &'a [Layer], id: &str) -> Option<(&'a Layer, usize)> {
    for (i, l) in list.iter().enumerate() {
        if l.id == id {
            return Some((l, i + 1));
        }
        if let LayerKind::Group { layers } = &l.kind
            && let Some(found) = find_indexed(layers, id)
        {
            return Some(found);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// 3D

/// A 3D scene at one instant: everything the renderer draws, solved together.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated3d {
    /// Children included; keyframes, shared materials, expressions and constraints applied.
    pub objects: Vec<Object3d>,
    /// The lights that aren't hidden.
    pub lights: Vec<Light>,
    /// The camera filming.
    pub camera: Camera,
}

impl Scene3d {
    /// The objects at scene time `t`, children included, with shared materials put in place.
    pub fn objects_at(&self, t: f64) -> Vec<Object3d> {
        self.objects_at_with(t, &EvalOptions::default())
    }

    pub fn objects_at_with(&self, t: f64, opts: &EvalOptions) -> Vec<Object3d> {
        Solver::new(self, t, opts).objects()
    }

    /// The lights at scene time `t`.
    pub fn lights_at(&self, t: f64) -> Vec<Light> {
        self.lights_at_with(t, &EvalOptions::default())
    }

    pub fn lights_at_with(&self, t: f64, opts: &EvalOptions) -> Vec<Light> {
        Solver::new(self, t, opts).lights()
    }

    /// The camera filming at scene time `t`.
    pub fn camera_at(&self, t: f64) -> Camera {
        self.camera_at_with(t, &EvalOptions::default())
    }

    pub fn camera_at_with(&self, t: f64, opts: &EvalOptions) -> Camera {
        let id = self.active_camera_at(t);
        let mut s = Solver::new(self, t, opts);
        let i = s.camera_index(&id).unwrap_or(0);
        s.camera(i)
    }

    /// The camera `id` (`"camera"` is the main one) at scene time `t`, solved like the one
    /// filming (keyframes, expressions, constraints); `None` when there is no such camera.
    pub fn camera_by_id_at(&self, id: &str, t: f64) -> Option<Camera> {
        let opts = EvalOptions::default();
        let mut s = Solver::new(self, t, &opts);
        let i = s.camera_index(id)?;
        Some(s.camera(i))
    }

    /// Objects, lights and the camera at scene time `t`, solved once (cheaper than the three
    /// calls when constraints tie them together).
    pub fn evaluate_at(&self, t: f64, opts: &EvalOptions) -> Evaluated3d {
        let id = self.active_camera_at(t);
        let mut s = Solver::new(self, t, opts);
        let i = s.camera_index(&id).unwrap_or(0);
        let camera = s.camera(i);
        let lights = s.lights();
        Evaluated3d { objects: s.objects(), lights, camera }
    }

    /// How long the scene runs on its own: its last keyframe (at least 1 s).
    pub fn natural_length(&self) -> f64 {
        let mut t = 1.0f64;
        let mut see = |k: &Keyframes| {
            for list in k.values() {
                if let Some(last) = list.last() {
                    t = t.max(last.time);
                }
            }
        };
        see(&self.keyframes);
        see(&self.camera.keyframes);
        for c in &self.cameras {
            see(&c.keyframes);
        }
        for l in &self.lights {
            see(&l.keyframes);
        }
        walk_objects(&self.objects, &mut |o| see(&o.keyframes));
        t
    }
}

/// A 3D scene as seen by `prop()`: values before constraints.
struct Space<'a> {
    scene: &'a Scene3d,
    fps: f64,
    duration: Option<f64>,
    length: OnceCell<f64>,
}

impl World for Space<'_> {
    fn find(&self, id: &str) -> Option<Found<'_>> {
        let s = self.scene;
        let found = |item, index: usize| Some(Found { item, index: index as f64, dur: Dur::Scene });
        if id == "scene" {
            return found(Item::Space(s), 1);
        }
        if id == "camera" || id.is_empty() {
            return found(Item::Camera(&s.camera), 1);
        }
        if let Some(i) = s.cameras.iter().position(|c| c.id == id) {
            return found(Item::Camera(&s.cameras[i]), i + 2);
        }
        if let Some(i) = s.lights.iter().position(|l| l.id == id) {
            return found(Item::Light(&s.lights[i]), i + 1);
        }
        fn look<'a>(list: &'a [Object3d], id: &str) -> Option<(&'a Object3d, usize)> {
            for (i, o) in list.iter().enumerate() {
                if o.id == id {
                    return Some((o, i + 1));
                }
                if let Some(f) = look(&o.children, id) {
                    return Some(f);
                }
            }
            None
        }
        look(&s.objects, id).and_then(|(o, i)| found(Item::Object(o), i))
    }

    fn keyed(&self, item: &Item, name: &str, t: f64) -> Option<KeyValue> {
        if let Item::Object(o) = item
            && o.material.from.is_some()
            && !o.keyframes.contains_key(name)
            && let Some(v) = self.scene.resolve_material(&o.material).get_value(name)
        {
            return Some(v);
        }
        keyed_generic(item, name, t)
    }

    fn ids(&self) -> Vec<String> {
        let mut out = vec!["camera".to_string()];
        out.extend(self.scene.cameras.iter().map(|c| c.id.clone()));
        out.extend(self.scene.lights.iter().map(|l| l.id.clone()));
        walk_objects(&self.scene.objects, &mut |o| out.push(o.id.clone()));
        out
    }

    fn fps(&self) -> f64 {
        self.fps
    }

    fn duration(&self, _dur: Dur) -> f64 {
        self.duration.unwrap_or_else(|| *self.length.get_or_init(|| self.scene.natural_length()))
    }
}

/// One object of the tree, flattened depth first.
struct Node3<'a> {
    src: &'a Object3d,
    parent: Option<usize>,
    children: Vec<usize>,
    index: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Todo,
    Busy,
    Done,
}

/// Something a constraint can follow.
#[derive(Clone, Copy)]
enum Target {
    Object(usize),
    Light(usize),
    Camera(usize),
}

/// Solves one instant of a 3D scene on demand: each object, light and camera is evaluated
/// (keyframes, material, expressions), then its constraints are applied after the things they
/// follow (and its parents). A loop of constraints (a follows b, b follows a) is cut by using
/// the unconstrained pose of whichever is already being solved.
struct Solver<'a> {
    scene: &'a Scene3d,
    space: Space<'a>,
    t: f64,
    nodes: Vec<Node3<'a>>,
    targets: OnceCell<HashMap<&'a str, Target>>,
    base: Vec<Option<Object3d>>,
    done: Vec<Option<(Object3d, M4)>>,
    state: Vec<State>,
    lights: Vec<Option<Light>>,
    light_state: Vec<State>,
    cams: Vec<Option<Camera>>,
    cam_state: Vec<State>,
    curves: HashMap<usize, Polyline>,
}

impl<'a> Solver<'a> {
    fn new(scene: &'a Scene3d, t: f64, opts: &EvalOptions) -> Self {
        let mut nodes = vec![];
        fn flatten<'a>(list: &'a [Object3d], parent: Option<usize>, nodes: &mut Vec<Node3<'a>>) -> Vec<usize> {
            let mut ids = vec![];
            for (i, o) in list.iter().enumerate() {
                let me = nodes.len();
                nodes.push(Node3 { src: o, parent, children: vec![], index: i + 1 });
                ids.push(me);
                let kids = flatten(&o.children, Some(me), nodes);
                nodes[me].children = kids;
            }
            ids
        }
        flatten(&scene.objects, None, &mut nodes);
        let n = nodes.len();
        let cams = 1 + scene.cameras.len();
        Solver {
            scene,
            space: Space { scene, fps: opts.fps(), duration: opts.duration, length: OnceCell::new() },
            t,
            nodes,
            targets: OnceCell::new(),
            base: vec![None; n],
            done: vec![None; n],
            state: vec![State::Todo; n],
            lights: vec![None; scene.lights.len()],
            light_state: vec![State::Todo; scene.lights.len()],
            cams: vec![None; cams],
            cam_state: vec![State::Todo; cams],
            curves: HashMap::new(),
        }
    }

    fn target(&self, id: &str) -> Option<Target> {
        let map = self.targets.get_or_init(|| {
            let mut m = HashMap::new();
            for (i, n) in self.nodes.iter().enumerate() {
                m.entry(n.src.id.as_str()).or_insert(Target::Object(i));
            }
            for (i, l) in self.scene.lights.iter().enumerate() {
                m.entry(l.id.as_str()).or_insert(Target::Light(i));
            }
            for (i, c) in self.scene.cameras.iter().enumerate() {
                m.entry(c.id.as_str()).or_insert(Target::Camera(i + 1));
            }
            m.insert("camera", Target::Camera(0));
            m
        });
        map.get(id).copied()
    }

    fn camera_index(&self, id: &str) -> Option<usize> {
        if id == "camera" || id.is_empty() {
            return Some(0);
        }
        self.scene.cameras.iter().position(|c| c.id == id).map(|i| i + 1)
    }

    fn camera_src(&self, i: usize) -> &'a Camera {
        if i == 0 { &self.scene.camera } else { &self.scene.cameras[i - 1] }
    }

    // ---- objects

    /// Keyframes, the shared material and expressions (no children, no constraints).
    fn base(&mut self, i: usize) -> &Object3d {
        if self.base[i].is_none() {
            let node = &self.nodes[i];
            let src = node.src;
            let mut o = Object3d { children: vec![], ..src.clone() };
            for (name, keys) in &src.keyframes {
                if let Some(v) = value_at(keys, self.t) {
                    let _ = o.set(name, &v);
                }
            }
            o.keyframes.clear();
            if o.material.from.is_some() {
                let keyed = o.material.clone();
                let mut m = self.scene.resolve_material(&o.material);
                // The object's own keyframed material values stay on top of the shared one.
                for name in src.keyframes.keys() {
                    if let Some(v) = keyed.get_value(name) {
                        let _ = m.set_value(name, &v);
                    }
                }
                o.material = m;
            }
            express(&self.space, Item::Object(src), &src.id, &src.expressions, &mut o, self.t, node.index as f64, Dur::Scene);
            self.base[i] = Some(o);
        }
        self.base[i].as_ref().expect("just made")
    }

    /// The world matrix from unconstrained values (to cut loops of constraints).
    fn base_world(&mut self, i: usize) -> M4 {
        let parent = match self.nodes[i].parent {
            Some(p) => match self.done[p].as_ref().map(|(_, w)| *w) {
                Some(w) => w,
                None => self.base_world(p),
            },
            None => M4::I,
        };
        parent.mul(&M4::local(self.base(i)))
    }

    /// The object solved, and its world matrix.
    fn object(&mut self, i: usize) -> M4 {
        match self.state[i] {
            State::Done => return self.done[i].as_ref().map_or(M4::I, |(_, w)| *w),
            State::Busy => return self.base_world(i),
            State::Todo => {}
        }
        self.state[i] = State::Busy;
        let parent = match self.nodes[i].parent {
            Some(p) => self.object(p),
            None => M4::I,
        };
        let mut o = self.base(i).clone();
        let mut world = parent.mul(&M4::local(&o));
        if o.constraints.iter().any(|c| c.enabled) {
            let start = world.pose();
            let mut pose = start;
            let constraints = o.constraints.clone();
            for c in constraints.iter().filter(|c| c.enabled) {
                self.constrain(&mut pose, c, Some(i), false);
            }
            write_back(&mut o, &parent, &start, &pose);
            world = parent.mul(&M4::local(&o));
        }
        self.done[i] = Some((o, world));
        self.state[i] = State::Done;
        world
    }

    fn objects(&mut self) -> Vec<Object3d> {
        for i in 0..self.nodes.len() {
            self.object(i);
        }
        let roots: Vec<usize> = (0..self.nodes.len()).filter(|i| self.nodes[*i].parent.is_none()).collect();
        roots.into_iter().map(|i| self.assemble(i)).collect()
    }

    fn assemble(&mut self, i: usize) -> Object3d {
        let mut o = self.done[i].take().map(|(o, _)| o).unwrap_or_else(|| Object3d { children: vec![], ..self.nodes[i].src.clone() });
        let kids = self.nodes[i].children.clone();
        o.children = kids.into_iter().map(|k| self.assemble(k)).collect();
        o
    }

    // ---- lights and cameras

    fn light(&mut self, i: usize) -> Light {
        if self.light_state[i] == State::Done
            && let Some(l) = &self.lights[i]
        {
            return l.clone();
        }
        let src = &self.scene.lights[i];
        let mut l = src.at(self.t);
        express(&self.space, Item::Light(src), &src.id, &src.expressions, &mut l, self.t, (i + 1) as f64, Dur::Scene);
        if self.light_state[i] == State::Busy {
            return l;
        }
        self.light_state[i] = State::Busy;
        if l.constraints.iter().any(|c| c.enabled) {
            let aims = matches!(l.kind.as_str(), "spot" | "directional" | "area");
            let mut pose = Pose { pos: l.position.0, rot: look_rot(neg(l.direction.0)), scale: [1.0; 3] };
            let start = pose;
            for c in l.constraints.clone().iter().filter(|c| c.enabled) {
                self.constrain(&mut pose, c, None, true);
            }
            l.position = Vec3(pose.pos);
            if aims && rot_differs(&start.rot, &pose.rot) {
                let d = neg(column(&pose.rot, 2));
                let len = length(l.direction.0).max(1e-9);
                l.direction = Vec3(scale(d, len));
            }
        }
        self.lights[i] = Some(l.clone());
        self.light_state[i] = State::Done;
        l
    }

    fn lights(&mut self) -> Vec<Light> {
        (0..self.scene.lights.len()).filter(|i| !self.scene.lights[*i].hidden).map(|i| self.light(i)).collect()
    }

    fn camera(&mut self, i: usize) -> Camera {
        if self.cam_state[i] == State::Done
            && let Some(c) = &self.cams[i]
        {
            return c.clone();
        }
        let src = self.camera_src(i);
        let mut c = src.at(self.t);
        let id = if i == 0 { "camera" } else { src.id.as_str() };
        express(&self.space, Item::Camera(src), id, &src.expressions, &mut c, self.t, (i + 1) as f64, Dur::Scene);
        if self.cam_state[i] == State::Busy {
            return c;
        }
        self.cam_state[i] = State::Busy;
        if c.constraints.iter().any(|k| k.enabled) {
            let dist = length(sub(c.target.0, c.position.0)).max(1e-3);
            let mut pose = Pose { pos: c.position.0, rot: look_rot(sub(c.position.0, c.target.0)), scale: [1.0; 3] };
            // Constraints that aim it set the target: a full lookAt the exact point (so the
            // focus distance too), the others a point straight ahead, as far as before.
            let mut retarget = false;
            let mut aim = None;
            for k in c.constraints.clone().iter().filter(|k| k.enabled) {
                let before = pose;
                let point = self.constrain(&mut pose, k, None, true);
                let turns = matches!(k.kind.as_str(), "lookAt" | "copyRotation") || (k.kind == "followPath" && k.b("align"));
                if turns && (point.is_some() || pose != before) {
                    retarget = true;
                    aim = point.filter(|_| k.n("influence") >= 1.0);
                }
            }
            c.position = Vec3(pose.pos);
            if retarget {
                c.target = Vec3(aim.unwrap_or_else(|| add(pose.pos, scale(neg(column(&pose.rot, 2)), dist))));
            }
        }
        self.cams[i] = Some(c.clone());
        self.cam_state[i] = State::Done;
        c
    }

    // ---- constraints

    /// The world pose of something constraints follow.
    fn target_pose(&mut self, id: &str) -> Option<Pose> {
        match self.target(id)? {
            Target::Object(j) => Some(self.object(j).pose()),
            Target::Light(j) => {
                let l = self.light(j);
                Some(Pose { pos: l.position.0, rot: look_rot(neg(l.direction.0)), scale: [1.0; 3] })
            }
            Target::Camera(j) => {
                let c = self.camera(j);
                Some(Pose { pos: c.position.0, rot: look_rot(sub(c.position.0, c.target.0)), scale: [1.0; 3] })
            }
        }
    }

    /// A curve object's line in world space: (point, direction) at `progress`.
    fn along(&mut self, id: &str, progress: f64) -> Option<([f64; 3], [f64; 3])> {
        let Target::Object(j) = self.target(id)? else { return None };
        let world = self.object(j);
        if !self.curves.contains_key(&j) {
            let Shape3d::Curve { points, closed, smooth, .. } = &self.base(j).shape else { return None };
            let line = Polyline::new(points, *closed, *smooth);
            self.curves.insert(j, line);
        }
        let (p, d) = self.curves.get(&j)?.at(progress);
        Some((world.point(p), normalize(world.dir(d))))
    }

    /// Applies one constraint to a world pose. `me` is the object itself (it can't follow
    /// itself); `aimed` is for cameras and lights (they look down −z, and have no scale). Gives
    /// the point a lookAt aimed at.
    fn constrain(&mut self, pose: &mut Pose, c: &Constraint, me: Option<usize>, aimed: bool) -> Option<V3> {
        let target = |s: &mut Self, key: &str| -> Option<Pose> {
            let id = c.opt_s(key)?;
            if me.is_some() && s.target(&id).is_some_and(|t| matches!(t, Target::Object(j) if Some(j) == me)) {
                return None;
            }
            s.target_pose(&id)
        };
        let w = c.n("influence");
        match c.kind.as_str() {
            "lookAt" => {
                let t = target(self, "target")?;
                let q = add(t.pos, c.v3("offset"));
                let f = sub(q, pose.pos);
                if length(f) < 1e-9 {
                    return None;
                }
                let want = if aimed { look_rot(neg(f)) } else { look_rot(f) };
                pose.rot = slerp(&pose.rot, &want, w);
                return Some(q);
            }
            "followPath" => {
                let path = c.opt_s("path")?;
                if me.is_some_and(|i| self.nodes[i].src.id == path) {
                    return None;
                }
                let (p, d) = self.along(&path, c.n("progress"))?;
                pose.pos = lerp(pose.pos, p, w);
                if c.b("align") {
                    let want = if aimed { look_rot(neg(d)) } else { look_rot(d) };
                    pose.rot = slerp(&pose.rot, &want, w);
                }
            }
            "copyPosition" => {
                let t = target(self, "target")?;
                pose.pos = lerp(pose.pos, add(t.pos, c.v3("offset")), w);
            }
            "copyRotation" => {
                let t = target(self, "target")?;
                pose.rot = slerp(&pose.rot, &t.rot, w);
            }
            "copyScale" => {
                if aimed {
                    return None;
                }
                let t = target(self, "target")?;
                pose.scale = lerp(pose.scale, t.scale, w);
            }
            "limitPosition" => {
                let (lo, hi) = (c.v3("min"), c.v3("max"));
                for k in 0..3 {
                    pose.pos[k] = pose.pos[k].max(lo[k]).min(hi[k].max(lo[k]));
                }
            }
            "floor" => pose.pos[1] = pose.pos[1].max(c.n("height")),
            _ => {}
        }
        None
    }
}

/// Writes a constrained world pose back as the object's own values, relative to its parent.
/// Only what changed is written, so untouched values keep their exact numbers.
fn write_back(o: &mut Object3d, parent: &M4, start: &Pose, end: &Pose) {
    let p = parent.pose();
    if dist(start.pos, end.pos) > 1e-12 {
        o.position = Vec3(parent.inverse_point(end.pos));
    }
    if rot_differs(&start.rot, &end.rot) {
        o.rotation = Vec3(euler(&mul3(&transpose(&p.rot), &end.rot)));
    }
    if dist(start.scale, end.scale) > 1e-12 {
        o.scale = Vec3(std::array::from_fn(|k| if p.scale[k].abs() > 1e-12 { end.scale[k] / p.scale[k] } else { end.scale[k] }));
    }
}

// ---------------------------------------------------------------------------------------------
// Expressions on one thing

/// Which length `duration` is.
#[derive(Clone, Copy)]
enum Dur<'a> {
    Scene,
    Comp(&'a Composition),
}

/// A thing in a scene `prop()` can read.
#[derive(Clone, Copy)]
enum Item<'a> {
    Layer(&'a Layer),
    Object(&'a Object3d),
    Light(&'a Light),
    Camera(&'a Camera),
    Flat(&'a Scene2d),
    Space(&'a Scene3d),
}

struct Found<'a> {
    item: Item<'a>,
    index: f64,
    dur: Dur<'a>,
}

impl<'a> Item<'a> {
    fn keyframes(&self) -> &'a Keyframes {
        match self {
            Item::Layer(l) => &l.keyframes,
            Item::Object(o) => &o.keyframes,
            Item::Light(l) => &l.keyframes,
            Item::Camera(c) => &c.keyframes,
            Item::Flat(s) => &s.keyframes,
            Item::Space(s) => &s.keyframes,
        }
    }

    fn expressions(&self) -> Option<&'a Expressions> {
        match self {
            Item::Layer(l) => Some(&l.expressions),
            Item::Object(o) => Some(&o.expressions),
            Item::Light(l) => Some(&l.expressions),
            Item::Camera(c) => Some(&c.expressions),
            Item::Flat(_) | Item::Space(_) => None,
        }
    }

    /// The value without keyframes.
    fn still(&self, name: &str) -> Option<KeyValue> {
        match self {
            Item::Layer(l) => l.get(name),
            Item::Object(o) => o.get(name),
            Item::Light(l) => l.get(name),
            Item::Camera(c) => c.get(name),
            Item::Flat(s) => (name == "background").then(|| s.background.as_deref().map(KeyValue::from)).flatten(),
            Item::Space(s) => get_scene3d(s, name),
        }
    }

    /// The value with every keyframe applied (slow: a copy of the thing).
    fn applied(&self, name: &str, t: f64) -> Option<KeyValue> {
        match self {
            Item::Layer(l) => l.at(t).get(name),
            Item::Object(o) => Object3d { children: vec![], ..(*o).clone() }.at(t).get(name),
            Item::Light(l) => l.at(t).get(name),
            Item::Camera(c) => c.at(t).get(name),
            _ => self.still(name),
        }
    }

    fn names(&self) -> Vec<String> {
        match self {
            Item::Layer(l) => l.prop_names(),
            Item::Object(o) => o.prop_names(),
            Item::Light(_) => LIGHT_PROPS.iter().map(|p| p.to_string()).collect(),
            Item::Camera(_) => CAMERA_PROPS.iter().map(|p| p.to_string()).collect(),
            Item::Flat(_) => vec!["background".into()],
            Item::Space(_) => SCENE3D_PROPS.iter().map(|p| p.to_string()).collect(),
        }
    }
}

/// A property's keyframed value at `t`: its own keyframes when it has them, the still value
/// when the thing has none, else the thing with all its keyframes applied (a keyframed
/// `position` moves `x` too).
fn keyed_generic(item: &Item, name: &str, t: f64) -> Option<KeyValue> {
    let keys = item.keyframes();
    if let Some(list) = keys.get(name).filter(|k| !k.is_empty()) {
        return value_at(list, t);
    }
    if keys.is_empty() { item.still(name) } else { item.applied(name, t) }
}

/// What `prop()` and expressions see of a scene.
trait World {
    fn find(&self, id: &str) -> Option<Found<'_>>;
    fn keyed(&self, item: &Item, name: &str, t: f64) -> Option<KeyValue>;
    fn ids(&self) -> Vec<String>;
    fn fps(&self) -> f64;
    fn duration(&self, dur: Dur) -> f64;
}

/// The chain of properties `prop()` is reading through (for the depth limit and circles).
struct Chain<'a> {
    id: &'a str,
    name: &'a str,
    up: Option<&'a Chain<'a>>,
}

impl Chain<'_> {
    fn depth(&self) -> usize {
        1 + self.up.map_or(0, |u| u.depth())
    }

    fn contains(&self, id: &str, name: &str) -> bool {
        (self.id == id && self.name == name) || self.up.is_some_and(|u| u.contains(id, name))
    }

    fn path(&self) -> String {
        let here = format!("{}.{}", self.id, self.name);
        match self.up {
            Some(u) => format!("{} → {here}", u.path()),
            None => here,
        }
    }
}

/// One property's formula as it runs.
struct PropCtx<'w> {
    world: &'w dyn World,
    item: Item<'w>,
    name: &'w str,
    time: f64,
    now: Option<KeyValue>,
    keys: &'w [Keyframe],
    index: f64,
    dur: Dur<'w>,
    seed: u64,
    chain: &'w Chain<'w>,
}

impl expr::Context for PropCtx<'_> {
    fn time(&self) -> f64 {
        self.time
    }
    fn value(&self) -> Option<KeyValue> {
        self.now.clone()
    }
    fn value_at(&self, t: f64) -> Option<KeyValue> {
        if t == self.time {
            return self.now.clone();
        }
        self.world.keyed(&self.item, self.name, t)
    }
    fn keys(&self) -> &[Keyframe] {
        self.keys
    }
    fn prop(&self, id: &str, name: &str, t: Option<f64>) -> Result<KeyValue, String> {
        prop_value(self.world, id, name, t.unwrap_or(self.time), self.chain)
    }
    fn index(&self) -> f64 {
        self.index
    }
    fn fps(&self) -> f64 {
        self.world.fps()
    }
    fn duration(&self) -> f64 {
        self.world.duration(self.dur)
    }
    fn seed(&self) -> u64 {
        self.seed
    }
}

fn seed(id: &str, name: &str) -> u64 {
    expr::hash_str(&format!("{id}\u{1f}{name}"))
}

/// Another thing's property at `t`: keyframes, then its own formula (before constraints).
fn prop_value(world: &dyn World, id: &str, name: &str, t: f64, chain: &Chain) -> Result<KeyValue, String> {
    if chain.contains(id, name) {
        return Err(format!("the formulas go round in a circle ({} → {id}.{name})", chain.path()));
    }
    if chain.depth() >= MAX_PROP_DEPTH {
        return Err(format!("prop() reads through more than {MAX_PROP_DEPTH} formulas ({} → {id}.{name})", chain.path()));
    }
    let Some(found) = world.find(id) else {
        let ids = world.ids();
        let names: Vec<&str> = ids.iter().map(String::as_str).collect();
        let hint = crate::closest(id, &names).map(|c| format!(" Did you mean \"{c}\"?")).unwrap_or_default();
        return Err(format!("nothing has the id \"{id}\".{hint}"));
    };
    let Some(now) = world.keyed(&found.item, name, t) else {
        let all = found.item.names();
        let names: Vec<&str> = all.iter().map(String::as_str).collect();
        let hint = crate::closest(name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        return Err(format!("\"{id}\" has no property `{name}`.{hint}"));
    };
    let Some(src) = found.item.expressions().and_then(|e| e.get(name)) else { return parts_driven(world, &found.item, id, name, now, t, chain) };
    let link = Chain { id, name, up: Some(chain) };
    let keys = found.item.keyframes().get(name).map_or(&[][..], Vec::as_slice);
    let ctx = PropCtx { world, item: found.item, name, time: t, now: Some(now.clone()), keys, index: found.index, dur: found.dur, seed: seed(id, name), chain: &link };
    let out = expr::eval(&*expr::compile(src)?, &ctx).map_err(|e| format!("its formula: {e}"))?;
    Ok(coerce(out, Some(&now)))
}

/// Vector properties whose parts have names of their own (`x` is `position.x` is `position[0]`).
const VECTORS: &[(&str, &[&[&str]])] = &[
    ("position", &[&["x", "position.x"], &["y", "position.y"], &["z", "position.z"]]),
    ("rotation", &[&["rotation.x"], &["rotation.y"], &["rotation.z"]]),
    ("scale", &[&["scale.x"], &["scale.y"], &["scale.z"]]),
    ("target", &[&["target.x"], &["target.y"], &["target.z"]]),
];

/// `name` without a formula of its own, when a formula drives it through another name: a part
/// read while the whole vector has a formula (`x` of a `position` formula), the whole read
/// while parts have formulas, or a part under its other name (`position.x` for `x`).
fn parts_driven(world: &dyn World, item: &Item, id: &str, name: &str, now: KeyValue, t: f64, chain: &Chain) -> Result<KeyValue, String> {
    let Some(exprs) = item.expressions().filter(|e| !e.is_empty()) else { return Ok(now) };
    for (whole, parts) in VECTORS {
        if name == *whole {
            let KeyValue::Vector(mut v) = now else { return Ok(now) };
            for (i, names) in parts.iter().enumerate() {
                if i < v.len()
                    && let Some(part) = names.iter().find(|p| exprs.contains_key(**p))
                    && let Some(n) = prop_value(world, id, part, t, chain)?.as_f64()
                {
                    v[i] = n;
                }
            }
            return Ok(KeyValue::Vector(v));
        }
        if let Some(i) = parts.iter().position(|names| names.contains(&name)) {
            if let Some(alias) = parts[i].iter().find(|p| **p != name && exprs.contains_key(**p)) {
                return prop_value(world, id, alias, t, chain);
            }
            if exprs.contains_key(*whole) {
                return Ok(match prop_value(world, id, whole, t, chain)? {
                    KeyValue::Vector(v) => v.get(i).map_or(now, |n| KeyValue::Number(*n)),
                    _ => now,
                });
            }
            return Ok(now);
        }
    }
    Ok(now)
}

/// Something with properties by keyframe name.
trait Props {
    fn get_prop(&self, name: &str) -> Option<KeyValue>;
    fn set_prop(&mut self, name: &str, v: &KeyValue) -> Result<(), String>;
}

impl Props for Layer {
    fn get_prop(&self, name: &str) -> Option<KeyValue> {
        self.get(name)
    }
    fn set_prop(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        self.set(name, v)
    }
}

impl Props for Object3d {
    fn get_prop(&self, name: &str) -> Option<KeyValue> {
        self.get(name)
    }
    fn set_prop(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        self.set(name, v)
    }
}

impl Props for Light {
    fn get_prop(&self, name: &str) -> Option<KeyValue> {
        self.get(name)
    }
    fn set_prop(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        self.set(name, v)
    }
}

impl Props for Camera {
    fn get_prop(&self, name: &str) -> Option<KeyValue> {
        self.get(name)
    }
    fn set_prop(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        self.set(name, v)
    }
}

/// Runs a thing's formulas on `out` (its keyframed copy). Every formula sees the keyframed
/// values (not each other's results); one that fails leaves its property as keyframed.
#[allow(clippy::too_many_arguments)]
fn express<T: Props>(world: &dyn World, item: Item, id: &str, exprs: &Expressions, out: &mut T, t: f64, index: f64, dur: Dur) {
    if exprs.is_empty() {
        return;
    }
    let mut results = Vec::with_capacity(exprs.len());
    for (name, src) in exprs {
        let now = out.get_prop(name);
        let keys = item.keyframes().get(name).map_or(&[][..], Vec::as_slice);
        let root = Chain { id, name, up: None };
        let ctx = PropCtx { world, item, name, time: t, now: now.clone(), keys, index, dur, seed: seed(id, name), chain: &root };
        match expr::compile(src).and_then(|p| expr::eval(&p, &ctx)) {
            Ok(v) => results.push((name, coerce(v, now.as_ref()))),
            Err(e) => warn_once(id, name, src, &e),
        }
    }
    for (name, v) in results {
        if let Err(e) = set_coerced(out, name, &v) {
            warn_once(id, name, &exprs[name], &e);
        }
    }
}

/// Fits a formula's result to its property: `[r, g, b]` (0–1) for a colour, a number for text.
fn coerce(v: KeyValue, now: Option<&KeyValue>) -> KeyValue {
    match (now, v) {
        (Some(KeyValue::Text(s)), KeyValue::Vector(c)) if Rgba::parse(s).is_some() && (3..=4).contains(&c.len()) => KeyValue::Text(rgb_hex(&c)),
        (Some(KeyValue::Text(s)), KeyValue::Number(n)) if Rgba::parse(s).is_none() => KeyValue::Text(expr::format_number(n)),
        (Some(KeyValue::Number(_)), KeyValue::Vector(x)) if x.len() == 1 => KeyValue::Number(x[0]),
        (_, v) => v,
    }
}

fn set_coerced<T: Props>(out: &mut T, name: &str, v: &KeyValue) -> Result<(), String> {
    match out.set_prop(name, v) {
        Err(e) => match v {
            // A colour property that had no value yet (a fill to come) given [r, g, b].
            KeyValue::Vector(c) if (3..=4).contains(&c.len()) => out.set_prop(name, &KeyValue::Text(rgb_hex(c))).map_err(|_| e),
            _ => Err(e),
        },
        ok => ok,
    }
}

fn rgb_hex(c: &[f64]) -> String {
    let ch = |i: usize, d: f64| c.get(i).copied().unwrap_or(d).clamp(0.0, 1.0) * 255.0;
    Rgba([ch(0, 0.0), ch(1, 0.0), ch(2, 0.0), ch(3, 1.0)]).to_hex()
}

/// Logs a failing formula once (per thing, property and formula), not every frame.
fn warn_once(id: &str, name: &str, src: &str, e: &str) {
    static SEEN: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    let key = expr::hash_str(&format!("{id}\u{1f}{name}\u{1f}{src}"));
    let Ok(mut seen) = SEEN.get_or_init(Default::default).lock() else { return };
    if seen.len() > 4096 {
        seen.clear();
    }
    if seen.insert(key) {
        tracing::warn!(item = id, property = name, "expression `{src}` failed: {e}; the property keeps its keyframed value");
    }
}

// ---------------------------------------------------------------------------------------------
// Shared materials

impl Material {
    /// A material property by keyframe name (`color`, `roughness`, `pattern.scale`…).
    pub fn get_value(&self, name: &str) -> Option<KeyValue> {
        self.get(name)
    }

    pub fn set_value(&mut self, name: &str, v: &KeyValue) -> Result<bool, String> {
        self.set(name, v)
    }
}

// ---------------------------------------------------------------------------------------------
// The little f64 geometry constraints need (the renderer's conventions: column-major 4×4,
// right-handed, rotation x then y then z in degrees).

type V3 = [f64; 3];
/// Row-major 3×3: `r[row][col]`.
type R3 = [[f64; 3]; 3];

/// A world position, rotation and scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    pos: V3,
    rot: R3,
    scale: V3,
}

/// Column-major: `m[col][row]`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct M4([[f64; 4]; 4]);

impl M4 {
    const I: M4 = M4([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);

    /// Translation × rotation × scale, like the renderer's `M4::trs`.
    fn trs(pos: V3, rot: &R3, s: V3) -> M4 {
        let mut m = M4::I;
        for (c, col) in m.0.iter_mut().take(3).enumerate() {
            for (r, v) in col.iter_mut().take(3).enumerate() {
                *v = rot[r][c] * s[c];
            }
        }
        m.0[3] = [pos[0], pos[1], pos[2], 1.0];
        m
    }

    fn local(o: &Object3d) -> M4 {
        M4::trs(o.position.0, &rot_euler(o.rotation.0), o.scale.0)
    }

    fn mul(&self, o: &M4) -> M4 {
        let mut r = [[0.0f64; 4]; 4];
        for (c, col) in r.iter_mut().enumerate() {
            for (row, v) in col.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[k][row] * o.0[c][k]).sum();
            }
        }
        M4(r)
    }

    fn point(&self, v: V3) -> V3 {
        std::array::from_fn(|r| self.0[0][r] * v[0] + self.0[1][r] * v[1] + self.0[2][r] * v[2] + self.0[3][r])
    }

    fn dir(&self, v: V3) -> V3 {
        std::array::from_fn(|r| self.0[0][r] * v[0] + self.0[1][r] * v[1] + self.0[2][r] * v[2])
    }

    /// Position, rotation (columns normalised) and scale (column lengths).
    fn pose(&self) -> Pose {
        let scale: V3 = std::array::from_fn(|c| length([self.0[c][0], self.0[c][1], self.0[c][2]]));
        let mut rot = [[0.0; 3]; 3];
        for (c, s) in scale.iter().enumerate() {
            for (r, row) in rot.iter_mut().enumerate() {
                row[c] = if *s > 1e-12 { self.0[c][r] / s } else if r == c { 1.0 } else { 0.0 };
            }
        }
        Pose { pos: [self.0[3][0], self.0[3][1], self.0[3][2]], rot, scale }
    }

    /// The point whose image is `p` (the inverse of the affine map), or `p` if it collapses.
    fn inverse_point(&self, p: V3) -> V3 {
        let a: R3 = std::array::from_fn(|r| std::array::from_fn(|c| self.0[c][r]));
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if det.abs() < 1e-15 {
            return p;
        }
        let k = 1.0 / det;
        let inv = [
            [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) * k, (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * k, (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * k],
            [(a[1][2] * a[2][0] - a[1][0] * a[2][2]) * k, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * k, (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * k],
            [(a[1][0] * a[2][1] - a[1][1] * a[2][0]) * k, (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * k, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * k],
        ];
        let d = sub(p, [self.0[3][0], self.0[3][1], self.0[3][2]]);
        std::array::from_fn(|r| inv[r][0] * d[0] + inv[r][1] * d[1] + inv[r][2] * d[2])
    }
}

/// Rotation x, then y, then z (degrees): `Rz · Ry · Rx`.
fn rot_euler(deg: V3) -> R3 {
    let [x, y, z] = deg.map(f64::to_radians);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
        [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
        [-sy, sx * cy, cx * cy],
    ]
}

/// Degrees (x, y, z) of a rotation made x, then y, then z.
fn euler(r: &R3) -> V3 {
    let sy = (-r[2][0]).clamp(-1.0, 1.0);
    let y = sy.asin();
    let (x, z) = if y.cos().abs() > 1e-6 { (r[2][1].atan2(r[2][2]), r[1][0].atan2(r[0][0])) } else { ((-r[1][2]).atan2(r[1][1]), 0.0) };
    [x.to_degrees(), y.to_degrees(), z.to_degrees()].map(|a| if a.abs() < 1e-9 { 0.0 } else { a })
}

/// The rotation turning +z towards `f`, with +y as close to straight up as it can be.
fn look_rot(f: V3) -> R3 {
    let f = normalize(f);
    let up = if f[1].abs() > 0.9999 { [0.0, 0.0, -f[1].signum()] } else { [0.0, 1.0, 0.0] };
    let x = normalize(cross(up, f));
    let y = cross(f, x);
    [[x[0], y[0], f[0]], [x[1], y[1], f[1]], [x[2], y[2], f[2]]]
}

fn column(r: &R3, c: usize) -> V3 {
    [r[0][c], r[1][c], r[2][c]]
}

fn mul3(a: &R3, b: &R3) -> R3 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn transpose(a: &R3) -> R3 {
    std::array::from_fn(|r| std::array::from_fn(|c| a[c][r]))
}

fn rot_differs(a: &R3, b: &R3) -> bool {
    (0..3).any(|r| (0..3).any(|c| (a[r][c] - b[r][c]).abs() > 1e-12))
}

/// From rotation `a` towards `b` by `w` (0–1), along the shortest arc.
fn slerp(a: &R3, b: &R3, w: f64) -> R3 {
    if w >= 1.0 {
        return *b;
    }
    if w <= 0.0 {
        return *a;
    }
    let (qa, mut qb) = (quat(a), quat(b));
    let mut d: f64 = (0..4).map(|i| qa[i] * qb[i]).sum();
    if d < 0.0 {
        qb = qb.map(|v| -v);
        d = -d;
    }
    let q: [f64; 4] = if d > 0.9995 {
        std::array::from_fn(|i| qa[i] + (qb[i] - qa[i]) * w)
    } else {
        let th = d.clamp(-1.0, 1.0).acos();
        let s = th.sin();
        let (ka, kb) = (((1.0 - w) * th).sin() / s, (w * th).sin() / s);
        std::array::from_fn(|i| qa[i] * ka + qb[i] * kb)
    };
    from_quat(q)
}

/// (w, x, y, z) of a rotation matrix.
fn quat(m: &R3) -> [f64; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [0.25 * s, (m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [(m[2][1] - m[1][2]) / s, 0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [(m[0][2] - m[2][0]) / s, (m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [(m[1][0] - m[0][1]) / s, (m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s]
    };
    let l = q.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12);
    q.map(|v| v / l)
}

fn from_quat(q: [f64; 4]) -> R3 {
    let l = q.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12);
    let [w, x, y, z] = q.map(|v| v / l);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] + b[i])
}

fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] - b[i])
}

fn neg(a: V3) -> V3 {
    a.map(|v| -v)
}

fn scale(a: V3, k: f64) -> V3 {
    a.map(|v| v * k)
}

fn lerp(a: V3, b: V3, w: f64) -> V3 {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * w)
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn length(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn dist(a: V3, b: V3) -> f64 {
    length(sub(a, b))
}

fn normalize(a: V3) -> V3 {
    let l = length(a);
    if l < 1e-12 { [0.0, 0.0, 1.0] } else { scale(a, 1.0 / l) }
}

#[cfg(test)]
mod tests;
