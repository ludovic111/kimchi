//! Reading a motion clip's scene for the Studio: the outliner's tree, what a selected key is,
//! values at a scene time, world transforms and bounds for the tools, picking. Nothing here
//! changes the scene; edits are `motion.*` commands.

use std::collections::{HashMap, HashSet};

use kimchi_core::motion::{Layer, LayerKind, Object3d, Scene2d, Scene3d, Shape3d, find_object, walk_objects};
use kimchi_core::{Clip, ClipContent, KeyValue, Keyframes, Project, Scene};

use super::math::{self, Affine, M4, V3};

/// The selection key of a shared material (3D) and of a composition (2D) in the outliner.
pub const MATERIAL: &str = "material:";
pub const COMPOSITION: &str = "comp:";

pub fn motion_clip(p: &Project, id: kimchi_core::Id) -> Option<(&Clip, &Scene)> {
    let c = p.clip(id)?;
    match &c.content {
        ClipContent::Motion { scene, .. } => Some((c, scene)),
        _ => None,
    }
}

/// The scene time a clip shows at timeline time `t` (held at its ends).
pub fn scene_time(clip: &Clip, t: f64) -> f64 {
    clip.scene_time(t.clamp(clip.start, clip.end()))
}

/// The timeline time that shows scene time `st` (the inverse of [`scene_time`]).
pub fn timeline_time(clip: &Clip, st: f64) -> f64 {
    let local = (st - clip.in_point) / clip.speed.max(1e-6);
    let t = if clip.reverse { clip.start + clip.duration - local } else { clip.start + local };
    t.clamp(clip.start, clip.end())
}

// ---------------------------------------------------------------------------------------------
// The outliner's tree

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// The 3D world / the 2D scene itself (`"scene"`).
    Scene,
    /// A title over a group of rows ("Cameras", "Lights"…); not selectable.
    Header,
    Camera { active: bool },
    Light,
    Object,
    Material,
    Layer,
    Composition,
}

#[derive(Clone, Debug)]
pub struct Row {
    /// What selecting it selects: an id, `"scene"`, `"camera"`, `material:<id>`, `comp:<id>`.
    pub key: String,
    pub label: String,
    pub kind: RowKind,
    /// The shape or layer type (`box`, `text`…), the light type, or "".
    pub what: String,
    pub depth: u32,
    pub hidden: Option<bool>,
    pub has_children: bool,
    /// The list it is in, for reordering: "" = the top level, or a group, composition or object id.
    pub parent: String,
    /// Its index in that list (data order: 2D draws later layers on top).
    pub index: usize,
}

impl Row {
    pub fn icon(&self) -> &'static str {
        match self.kind {
            RowKind::Scene => "globe",
            RowKind::Header => "chevron-down",
            RowKind::Camera { .. } => "video",
            RowKind::Light => "sun",
            RowKind::Material => "palette",
            RowKind::Composition => "layers",
            RowKind::Object => shape_icon(&self.what),
            RowKind::Layer => layer_icon(&self.what),
        }
    }

    /// Can be moved in its list or into another (objects and layers).
    pub fn movable(&self) -> bool {
        matches!(self.kind, RowKind::Object | RowKind::Layer)
    }

    /// Can hold others (groups, objects, compositions).
    pub fn container(&self) -> bool {
        match self.kind {
            RowKind::Object | RowKind::Composition => true,
            RowKind::Layer => self.what == "group",
            _ => false,
        }
    }
}

pub fn shape_icon(shape: &str) -> &'static str {
    match shape {
        "box" => "box",
        "sphere" | "icosphere" => "circle",
        "cylinder" | "capsule" => "cylinder",
        "cone" => "triangle",
        "torus" => "circle-dot",
        "plane" | "grid" => "square",
        "text" => "type",
        "extrude" => "pen-tool",
        "lathe" => "amphora",
        "curve" => "spline",
        "mesh" => "hexagon",
        "particles" => "sparkles",
        "model" => "package",
        "image" => "image",
        "group" => "folder",
        _ => "box",
    }
}

pub fn layer_icon(kind: &str) -> &'static str {
    match kind {
        "rect" => "square",
        "ellipse" => "circle",
        "polygon" => "hexagon",
        "star" => "star",
        "path" => "pen-tool",
        "text" => "type",
        "image" => "image",
        "group" => "folder",
        "null" => "crosshair",
        "adjustment" => "sliders-horizontal",
        "comp" => "layers",
        "particles" => "sparkles",
        _ => "square",
    }
}

/// Every row, top to bottom, skipping the children of `collapsed` keys.
pub fn rows(scene: &Scene, collapsed: &HashSet<String>) -> Vec<Row> {
    let mut out = vec![];
    let row = |key: &str, label: &str, kind: RowKind, what: &str, depth: u32| Row {
        key: key.into(),
        label: label.into(),
        kind,
        what: what.into(),
        depth,
        hidden: None,
        has_children: false,
        parent: String::new(),
        index: 0,
    };
    match scene {
        Scene::Space(s) => {
            out.push(row("scene", "World", RowKind::Scene, "", 0));
            let active = s.active_camera.clone().unwrap_or_else(|| "camera".into());
            out.push(row("#cameras", "Cameras", RowKind::Header, "", 0));
            out.push(row("camera", "camera", RowKind::Camera { active: active == "camera" }, "", 1));
            for c in &s.cameras {
                out.push(row(&c.id, &c.id, RowKind::Camera { active: active == c.id }, "", 1));
            }
            if !s.lights.is_empty() {
                out.push(row("#lights", "Lights", RowKind::Header, "", 0));
                for l in &s.lights {
                    let mut r = row(&l.id, &l.id, RowKind::Light, &l.kind, 1);
                    r.hidden = Some(l.hidden);
                    out.push(r);
                }
            }
            out.push(row("#objects", "Objects", RowKind::Header, "", 0));
            fn objects(list: &[Object3d], parent: &str, depth: u32, collapsed: &HashSet<String>, out: &mut Vec<Row>) {
                for (i, o) in list.iter().enumerate() {
                    out.push(Row {
                        key: o.id.clone(),
                        label: o.id.clone(),
                        kind: RowKind::Object,
                        what: o.shape.name().into(),
                        depth,
                        hidden: Some(o.hidden),
                        has_children: !o.children.is_empty(),
                        parent: parent.into(),
                        index: i,
                    });
                    if !collapsed.contains(&o.id) {
                        objects(&o.children, &o.id, depth + 1, collapsed, out);
                    }
                }
            }
            objects(&s.objects, "", 1, collapsed, &mut out);
            if !s.materials.is_empty() {
                out.push(row("#materials", "Materials", RowKind::Header, "", 0));
                for m in &s.materials {
                    let id = m.id.clone().unwrap_or_default();
                    out.push(row(&format!("{MATERIAL}{id}"), &id, RowKind::Material, "", 1));
                }
            }
        }
        Scene::Flat(s) => {
            out.push(row("scene", "Scene", RowKind::Scene, "", 0));
            // Top of the list = drawn on top, like After Effects.
            fn layers(list: &[Layer], parent: &str, depth: u32, collapsed: &HashSet<String>, out: &mut Vec<Row>) {
                for (i, l) in list.iter().enumerate().rev() {
                    let children = match &l.kind {
                        LayerKind::Group { layers } => Some(layers),
                        _ => None,
                    };
                    out.push(Row {
                        key: l.id.clone(),
                        label: l.id.clone(),
                        kind: RowKind::Layer,
                        what: l.kind.name().into(),
                        depth,
                        hidden: Some(l.hidden),
                        has_children: children.is_some_and(|c| !c.is_empty()),
                        parent: parent.into(),
                        index: i,
                    });
                    if let Some(c) = children
                        && !collapsed.contains(&l.id)
                    {
                        layers(c, &l.id, depth + 1, collapsed, out);
                    }
                }
            }
            layers(&s.layers, "", 1, collapsed, &mut out);
            if !s.compositions.is_empty() {
                out.push(row("#compositions", "Compositions", RowKind::Header, "", 0));
                for c in &s.compositions {
                    let key = format!("{COMPOSITION}{}", c.id);
                    let mut r = row(&key, &c.id, RowKind::Composition, "", 1);
                    r.has_children = !c.layers.is_empty();
                    out.push(r);
                    if !collapsed.contains(&key) {
                        layers(&c.layers, &c.id, 2, collapsed, &mut out);
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// One thing of the scene

/// What a selection key names.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Scene,
    Camera,
    Light,
    /// An object, with its shape's name.
    Object(&'static str),
    /// A 2D layer, with its type.
    Layer(&'static str),
    Material(String),
    Composition(String),
}

pub fn item(scene: &Scene, key: &str) -> Option<Item> {
    if key == "scene" {
        return Some(Item::Scene);
    }
    if let Some(m) = key.strip_prefix(MATERIAL) {
        return matches!(scene, Scene::Space(s) if s.material(m).is_some()).then(|| Item::Material(m.into()));
    }
    if let Some(c) = key.strip_prefix(COMPOSITION) {
        return matches!(scene, Scene::Flat(s) if s.composition(c).is_some()).then(|| Item::Composition(c.into()));
    }
    match scene {
        Scene::Space(s) => {
            if s.cameras.iter().any(|c| c.id == key) || key == "camera" {
                return Some(Item::Camera);
            }
            if s.lights.iter().any(|l| l.id == key) {
                return Some(Item::Light);
            }
            find_object(&s.objects, key).map(|o| Item::Object(o.shape.name()))
        }
        Scene::Flat(s) => s.find_layer(key).map(|l| Item::Layer(l.kind.name())),
    }
}

/// The active key and what it names.
pub fn active_item(scene: &Scene, key: Option<&str>) -> Option<(String, Item)> {
    let k = key?;
    item(scene, k).map(|i| (k.to_string(), i))
}

/// Things that can be keyframed and transformed (not the scene, materials, compositions).
pub fn is_thing(scene: &Scene, key: &str) -> bool {
    matches!(item(scene, key), Some(Item::Camera | Item::Light | Item::Object(_) | Item::Layer(_)))
}

/// Its keyframes (a copy), for `"scene"` too.
pub fn keyframes(scene: &Scene, id: &str) -> Option<Keyframes> {
    match scene {
        Scene::Flat(s) if id == "scene" => Some(s.keyframes.clone()),
        Scene::Flat(s) => s.find_layer(id).map(|l| l.keyframes.clone()),
        Scene::Space(s) => match id {
            "scene" => Some(s.keyframes.clone()),
            "camera" => Some(s.camera.keyframes.clone()),
            _ => s
                .cameras
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.keyframes.clone())
                .or_else(|| s.lights.iter().find(|l| l.id == id).map(|l| l.keyframes.clone()))
                .or_else(|| find_object(&s.objects, id).map(|o| o.keyframes.clone())),
        },
    }
}

/// Animatable property names of one thing (without copying the scene for layers and objects).
pub fn prop_names(scene: &Scene, id: &str) -> Vec<String> {
    match scene {
        Scene::Flat(s) if id != "scene" => s.find_layer(id).map(|l| l.prop_names()).unwrap_or_default(),
        Scene::Space(s) if !matches!(id, "scene" | "camera") && find_object(&s.objects, id).is_some() => find_object(&s.objects, id).map(|o| o.prop_names()).unwrap_or_default(),
        _ => scene.prop_names(id),
    }
}

/// A property at scene time `t` with keyframes applied (vector keyframes read through `x.y`
/// names too), or its still value.
pub fn value_at(scene: &Scene, id: &str, name: &str, t: f64) -> Option<KeyValue> {
    match scene {
        Scene::Flat(s) => {
            if id == "scene" {
                return scene.value(id, name, t);
            }
            s.find_layer(id).map(|l| l.at(t)).and_then(|l| l.get(name))
        }
        Scene::Space(s) => {
            if id == "scene" {
                return scene.value(id, name, t);
            }
            if let Some(c) = s.camera_by_id(id).filter(|_| id == "camera" || s.cameras.iter().any(|c| c.id == id)) {
                return c.at(t).get(name);
            }
            if let Some(l) = s.lights.iter().find(|l| l.id == id) {
                return l.at(t).get(name);
            }
            find_object(&s.objects, id).map(|o| o.at(t)).and_then(|o| o.get(name))
        }
    }
}

pub fn vec3_at(scene: &Scene, id: &str, name: &str, t: f64) -> Option<V3> {
    let v = value_at(scene, id, name, t)?.as_vec(3)?;
    Some([v[0], v[1], v[2]])
}

/// The keyframe name to use for one component of a vector property: the vector itself when it
/// is animated as a whole, else `base.axis`.
pub fn component_name(keys: &Keyframes, base: &str, axis: usize) -> String {
    if keys.contains_key(base) { base.to_string() } else { format!("{base}.{}", ["x", "y", "z"][axis]) }
}

/// `motion.updateLayer` props setting a vector property to `v`: per component, unless the whole
/// vector is animated.
pub fn vec_props(keys: &Keyframes, base: &str, v: V3) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    if keys.contains_key(base) {
        m.insert(base.into(), serde_json::json!(v));
    } else {
        for (i, a) in ["x", "y", "z"].iter().enumerate() {
            m.insert(format!("{base}.{a}"), serde_json::json!(v[i]));
        }
    }
    m
}

/// Ids of the things (objects, lights, cameras / layers), in the outliner's order.
pub fn thing_ids(scene: &Scene) -> Vec<String> {
    rows(scene, &HashSet::new()).into_iter().filter(|r| matches!(r.kind, RowKind::Object | RowKind::Light | RowKind::Camera { .. } | RowKind::Layer)).map(|r| r.key).collect()
}

/// A new id from `stem` not used yet ("box1", "box2"…).
pub fn fresh_id(scene: &Scene, stem: &str) -> String {
    let mut taken: HashSet<String> = scene.ids().into_iter().collect();
    taken.insert("camera".into());
    taken.insert("scene".into());
    if let Scene::Space(s) = scene {
        taken.extend(s.materials.iter().filter_map(|m| m.id.clone()));
    }
    if let Scene::Flat(s) = scene {
        taken.extend(s.compositions.iter().map(|c| c.id.clone()));
    }
    (1..).map(|n| format!("{stem}{n}")).find(|c| !taken.contains(c)).expect("a free id")
}

// ---------------------------------------------------------------------------------------------
// 3D: where things are

/// World matrices of every object (keyframes, expressions and constraints applied) at `t`,
/// plus lights and cameras (placed at their position, looking at their target or direction).
pub fn worlds(s: &Scene3d, t: f64) -> HashMap<String, M4> {
    let mut out = HashMap::new();
    fn walk(list: &[Object3d], parent: &M4, out: &mut HashMap<String, M4>) {
        for o in list {
            let m = math::mul(parent, &math::trs(o.position.0, o.rotation.0, o.scale.0));
            walk(&o.children, &m, out);
            out.insert(o.id.clone(), m);
        }
    }
    walk(&s.objects_at(t), &math::IDENTITY, &mut out);
    // Lights that are shown come solved (constraints); hidden ones keep their keyframed place.
    for l in s.lights_at(t) {
        out.insert(l.id.clone(), math::translate(l.position.0));
    }
    let hidden: Vec<(String, M4)> = s.lights.iter().filter(|l| !out.contains_key(&l.id)).map(|l| (l.id.clone(), math::translate(l.at(t).position.0))).collect();
    out.extend(hidden);
    let cams = std::iter::once(("camera".to_string(), s.camera.at(t))).chain(s.cameras.iter().map(|c| (c.id.clone(), c.at(t))));
    for (id, c) in cams {
        out.insert(id, look_at(c.position.0, c.target.0));
    }
    out
}

/// A camera-like frame at `eye` looking at `target` (its −z forward, +y up).
pub fn look_at(eye: V3, target: V3) -> M4 {
    let f = math::norm(math::sub(target, eye));
    let f = if math::len(f) < 1e-9 { [0.0, 0.0, -1.0] } else { f };
    let up = if f[1].abs() > 0.999 { [0.0, 0.0, -1.0] } else { [0.0, 1.0, 0.0] };
    let r = math::norm(math::cross(f, up));
    let u = math::cross(r, f);
    [[r[0], r[1], r[2], 0.0], [u[0], u[1], u[2], 0.0], [-f[0], -f[1], -f[2], 0.0], [eye[0], eye[1], eye[2], 1.0]]
}

/// The object containing `id` ("" at the top level), when `id` is an object.
pub fn parent3d(s: &Scene3d, id: &str) -> Option<String> {
    fn look(list: &[Object3d], parent: &str, id: &str) -> Option<String> {
        for o in list {
            if o.id == id {
                return Some(parent.to_string());
            }
            if let Some(p) = look(&o.children, &o.id, id) {
                return Some(p);
            }
        }
        None
    }
    look(&s.objects, "", id)
}

/// A shape's own box (lowest and highest corner) before its transform.
pub fn local_bounds(shape: &Shape3d) -> (V3, V3) {
    let b = |x: f64, y: f64, z: f64| ([-x, -y, -z], [x, y, z]);
    match shape {
        Shape3d::Box { size, .. } => b(size.0[0] / 2.0, size.0[1] / 2.0, size.0[2] / 2.0),
        Shape3d::Sphere { radius, .. } | Shape3d::Icosphere { radius, .. } => b(*radius, *radius, *radius),
        Shape3d::Cylinder { radius, height, .. } | Shape3d::Cone { radius, height, .. } | Shape3d::Capsule { radius, height } => b(*radius, height / 2.0, *radius),
        Shape3d::Torus { radius, tube } => b(radius + tube, *tube, radius + tube),
        Shape3d::Plane { width, height } => b(width / 2.0, height / 2.0, 0.01),
        Shape3d::Grid { width, height, .. } => b(width / 2.0, 0.01, height / 2.0),
        Shape3d::Text { text, size, depth, .. } => {
            let chars = text.lines().map(|l| l.chars().count()).max().unwrap_or(1).max(1) as f64;
            let lines = text.lines().count().max(1) as f64;
            b(size * 0.62 * chars / 2.0, size * 0.8 * lines / 2.0, depth / 2.0)
        }
        Shape3d::Extrude { size, depth, .. } => b(size / 2.0, size / 2.0, depth / 2.0),
        Shape3d::Lathe { profile, .. } => {
            let r = profile.iter().map(|p| p[0].abs()).fold(0.0, f64::max);
            let (lo, hi) = profile.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p[1]), b.max(p[1])));
            ([-r, lo, -r], [r, hi, r])
        }
        Shape3d::Curve { points, radius, .. } => points_box(points.iter().copied(), *radius),
        Shape3d::Mesh { vertices, .. } => points_box(vertices.iter().copied(), 0.0),
        Shape3d::Image { width, .. } => b(width / 2.0, width * 0.28, 0.01),
        Shape3d::Model { .. } => b(1.0, 1.0, 1.0),
        Shape3d::Particles(_) => b(0.25, 0.25, 0.25),
        Shape3d::Group {} => b(0.15, 0.15, 0.15),
    }
}

fn points_box(points: impl Iterator<Item = V3>, pad: f64) -> (V3, V3) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    if lo[0] > hi[0] {
        return ([-0.1; 3], [0.1; 3]);
    }
    (math::sub(lo, [pad; 3]), math::add(hi, [pad; 3]))
}

/// The eight corners of a box.
pub fn corners(lo: V3, hi: V3) -> [V3; 8] {
    let mut out = [[0.0; 3]; 8];
    for (i, c) in out.iter_mut().enumerate() {
        *c = [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
    }
    out
}

/// An object's box in world space at `t` (the drawn mesh, modifiers applied; children's too
/// for groups), or a small box around a light or camera.
pub fn world_bounds(s: &Scene3d, worlds: &HashMap<String, M4>, t: f64, id: &str) -> Option<(V3, V3)> {
    if find_object(&s.objects, id).is_some()
        && let Some(b) = kimchi_media::render::space::viewport::object_bounds(s, t, id)
    {
        return Some(b);
    }
    if let Some(o) = find_object(&s.objects, id) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        let mut add = |o: &Object3d| {
            let Some(m) = worlds.get(&o.id) else { return };
            let (a, b) = local_bounds(&o.shape);
            for c in corners(a, b) {
                let w = math::point(m, c);
                for i in 0..3 {
                    lo[i] = lo[i].min(w[i]);
                    hi[i] = hi[i].max(w[i]);
                }
            }
        };
        add(o);
        walk_objects(&o.children, &mut add);
        return (lo[0] <= hi[0]).then_some((lo, hi));
    }
    let p = math::origin(worlds.get(id)?);
    Some((math::sub(p, [0.3; 3]), math::add(p, [0.3; 3])))
}

/// The nearest object a ray hits (against each object's box in its own space).
pub fn pick3d(s: &Scene3d, worlds: &HashMap<String, M4>, t: f64, o: V3, d: V3) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    walk_objects(&s.objects, &mut |obj| {
        if !obj.visible_at(t) || matches!(obj.shape, Shape3d::Group {}) {
            return;
        }
        let Some(m) = worlds.get(&obj.id) else { return };
        let inv = math::inverse(m);
        let (lo_o, lo_d) = (math::point(&inv, o), math::dir(&inv, d));
        let (a, b) = local_bounds(&obj.shape);
        if let Some(hit) = math::ray_box(lo_o, lo_d, a, b) {
            // Back to a world distance, so objects of different scales compare.
            let w = math::len(math::sub(math::point(m, math::add(lo_o, math::scale(lo_d, hit))), o));
            if best.as_ref().is_none_or(|(bt, _)| w < *bt) {
                best = Some((w, obj.id.clone()));
            }
        }
    });
    best.map(|(_, id)| id)
}

/// A mesh object's vertices in world space at `t` and its faces (for edit mode).
pub fn edit_mesh(s: &Scene3d, worlds: &HashMap<String, M4>, id: &str) -> Option<(Vec<V3>, Vec<Vec<u32>>)> {
    let o = find_object(&s.objects, id)?;
    let Shape3d::Mesh { vertices, faces, .. } = &o.shape else { return None };
    let m = worlds.get(id).copied().unwrap_or(math::IDENTITY);
    Some((vertices.iter().map(|v| math::point(&m, *v)).collect(), faces.clone()))
}

/// The edges of faces, each once (smaller index first).
pub fn mesh_edges(faces: &[Vec<u32>]) -> Vec<(u32, u32)> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for f in faces {
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            let e = (a.min(b), a.max(b));
            if seen.insert(e) {
                out.push(e);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 2D: where layers are

/// The canvas a 2D view shows: the project's, or a composition's.
pub fn canvas_size(s: &Scene2d, comp: Option<&str>, project: (f64, f64)) -> (f64, f64) {
    match comp.and_then(|c| s.composition(c)) {
        Some(c) => (c.width.unwrap_or(project.0), c.height.unwrap_or(project.1)),
        None => project,
    }
}

/// The layer list a view edits.
pub fn view_layers<'a>(s: &'a Scene2d, comp: Option<&str>) -> &'a [Layer] {
    match comp.and_then(|c| s.composition(c)) {
        Some(c) => &c.layers,
        None => &s.layers,
    }
}

/// A layer's own transform (position, rotation, skew, scale, anchor).
pub fn own_affine(l: &Layer) -> Affine {
    math::layer_affine(l.x, l.y, l.rotation, l.skew_x, l.scale * l.scale_x, l.scale * l.scale_y, l.anchor_x, l.anchor_y)
}

/// The layer at `t` (keyframes applied) and its full transform to the view's canvas (parents
/// and groups included), project pixels from the canvas centre.
pub fn layer_world(s: &Scene2d, comp: Option<&str>, t: f64, id: &str) -> Option<(Layer, Affine)> {
    fn chain(list: &[Layer], l: &Layer) -> Affine {
        let mut m = own_affine(l);
        let mut at = l;
        for _ in 0..64 {
            let Some(p) = at.parent.as_deref() else { break };
            let Some(parent) = list.iter().find(|x| x.id == p) else { break };
            m = math::aff_mul(&own_affine(parent), &m);
            at = parent;
        }
        m
    }
    fn look(list: &[Layer], id: &str, outer: Affine) -> Option<(Layer, Affine)> {
        if let Some(l) = list.iter().find(|l| l.id == id) {
            return Some((l.clone(), math::aff_mul(&outer, &chain(list, l))));
        }
        list.iter().find_map(|g| match &g.kind {
            LayerKind::Group { layers } => look(layers, id, math::aff_mul(&outer, &chain(list, g))),
            _ => None,
        })
    }
    let list: Vec<Layer> = view_layers(s, comp).iter().map(|l| l.at(t)).collect();
    let (layer, ours) = look(&list, id, math::AFFINE_ID)?;
    // The engine's transform has expressions applied: what is drawn.
    Some((layer, kimchi_media::render::layer_transform(s, t, comp, id).unwrap_or(ours)))
}

/// A layer's four corners (top left, top right, bottom right, bottom left) on the view's canvas
/// (the 2D engine's: expressions, parents and groups included).
pub fn layer_corners(s: &Scene2d, comp: Option<&str>, t: f64, id: &str, _project: (f64, f64)) -> Option<[[f64; 2]; 4]> {
    kimchi_media::render::layer_bounds(s, t, comp, id)
}

/// The topmost visible layer under `point` (project pixels from the canvas centre).
pub fn hit2d(s: &Scene2d, comp: Option<&str>, t: f64, point: [f64; 2], _project: (f64, f64)) -> Option<String> {
    kimchi_media::render::hit_test(s, t, comp, point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rows_follow_the_scene() {
        let s = Scene::from_json(&json!({"layers": [
            {"id": "bg", "type": "rect"},
            {"id": "g", "type": "group", "layers": [{"id": "a", "type": "ellipse"}]},
            {"id": "title", "type": "text", "text": "Hi", "hidden": true}
        ]}))
        .unwrap();
        let r = rows(&s, &HashSet::new());
        let keys: Vec<&str> = r.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["scene", "title", "g", "a", "bg"], "top of the list is drawn on top");
        assert_eq!(r[1].hidden, Some(true));
        assert_eq!((r[3].parent.as_str(), r[3].depth), ("g", 2));
        let collapsed: HashSet<String> = ["g".to_string()].into();
        assert_eq!(rows(&s, &collapsed).len(), 4);

        let s = Scene::from_json(&json!({"cameras": [{"id": "side"}], "activeCamera": "side", "lights": [{"id": "sun", "type": "directional"}],
            "objects": [{"id": "box", "type": "box", "children": [{"id": "ball", "type": "sphere"}]}],
            "materials": [{"id": "gold", "color": "#e8b04a"}]}))
        .unwrap();
        let r = rows(&s, &HashSet::new());
        let keys: Vec<&str> = r.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["scene", "#cameras", "camera", "side", "#lights", "sun", "#objects", "box", "ball", "#materials", "material:gold"]);
        assert_eq!(r[3].kind, RowKind::Camera { active: true });
        assert_eq!(item(&s, "material:gold"), Some(Item::Material("gold".into())));
        assert_eq!(item(&s, "ball"), Some(Item::Object("sphere")));
    }

    #[test]
    fn worlds_follow_parents_and_picks_hit_boxes() {
        let s = Scene::from_json(&json!({"objects": [{"id": "box", "type": "box", "position": [2, 0, 0], "children": [{"id": "ball", "type": "sphere", "position": [0, 1, 0]}]}]})).unwrap();
        let Scene::Space(sp) = &s else { panic!() };
        let w = worlds(sp, 0.0);
        let p = math::origin(&w["ball"]);
        assert!((p[0] - 2.0).abs() < 1e-9 && (p[1] - 1.0).abs() < 1e-9);
        assert_eq!(pick3d(sp, &w, 0.0, [2.0, 0.0, 5.0], [0.0, 0.0, -1.0]).as_deref(), Some("box"));
        assert_eq!(pick3d(sp, &w, 0.0, [2.0, 1.0, 5.0], [0.0, 0.0, -1.0]).as_deref(), Some("ball"));
        assert!(pick3d(sp, &w, 0.0, [-3.0, 0.0, 5.0], [0.0, 0.0, -1.0]).is_none());
        let (lo, hi) = world_bounds(sp, &w, 0.0, "box").unwrap();
        assert!((lo[0] - 1.5).abs() < 1e-3 && (hi[0] - 2.5).abs() < 1e-3, "the drawn box: {lo:?} {hi:?}");
    }

    #[test]
    fn layers_are_found_under_the_pointer() {
        let s = Scene::from_json(&json!({"layers": [
            {"id": "big", "type": "rect", "width": 400, "height": 400},
            {"id": "small", "type": "rect", "width": 50, "height": 50, "x": 100}
        ]}))
        .unwrap();
        let Scene::Flat(f) = &s else { panic!() };
        assert_eq!(hit2d(f, None, 0.0, [100.0, 0.0], (1920.0, 1080.0)).as_deref(), Some("small"));
        assert_eq!(hit2d(f, None, 0.0, [-100.0, 0.0], (1920.0, 1080.0)).as_deref(), Some("big"));
        let c = layer_corners(f, None, 0.0, "small", (1920.0, 1080.0)).unwrap();
        assert_eq!(c[0], [75.0, -25.0]);
    }

    #[test]
    fn scene_and_timeline_times_invert() {
        let mut c = Clip::new("m", 2.0, 4.0, ClipContent::Solid { color: "#000000".into() });
        c.in_point = 1.0;
        c.speed = 2.0;
        let st = scene_time(&c, 3.0);
        assert!((st - 3.0).abs() < 1e-9);
        assert!((timeline_time(&c, st) - 3.0).abs() < 1e-9);
    }
}
