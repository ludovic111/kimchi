//! The Studio: where a motion clip's scene is made, inside the editor's centre. A 3D scene gets
//! a Blender-like editor (outliner, a viewport with an editor camera, the transform gizmo, edit
//! mode for meshes, property tabs), a 2D scene an After Effects-like one (layers, the canvas
//! with handles, the pen and shape tools). Both share a dope sheet and a graph editor over the
//! clip's time.
//!
//! The Studio never changes the project itself: every edit is a `motion.*` command through the
//! store (drags fold into one undo step with a coalesce key), so the agent, MCP and the CLI see
//! and undo the same steps. What the Studio shows (the clip, the selection, the mode, the view…)
//! is its own state, reachable with `ui.studio` and reported in `ui.state`.

pub mod math;
pub mod model;

mod fields;
mod gizmo;
mod menus;
mod outliner;
mod properties;
pub mod render_state;
mod specs;
mod timeline;
mod toolbar;
mod viewport;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, Subscription, Task, Window, div, prelude::*, px};
use kimchi_control::CmdResult;
use kimchi_core::{Clip, Id, Project, Scene};
use kimchi_media::render::space::viewport::{Shading, ViewCamera};
use serde_json::{Value, json};

use crate::actions::*;
use crate::store::{Store, StoreExt};
use crate::theme::ActiveTheme;
use crate::ui::drag;

pub use menus::Popover;
pub use outliner::Outliner;
pub use properties::Properties;
pub use timeline::StudioTimeline;
pub use viewport::Viewport;

const LEFT_W: f32 = 260.;
const RIGHT_W: f32 = 320.;
const BOTTOM_H: f32 = 230.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Object,
    /// Editing a mesh's vertices, edges and faces.
    Edit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectMode {
    Vertex,
    Edge,
    Face,
}

impl SelectMode {
    pub fn name(self) -> &'static str {
        match self {
            SelectMode::Vertex => "vertex",
            SelectMode::Edge => "edge",
            SelectMode::Face => "face",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
    /// 2D: drag a layer's anchor point (After Effects' Y).
    Anchor,
    /// 2D: draw paths and masks.
    Pen,
    Rect,
    Ellipse,
    Star,
    Polygon,
    Text,
}

impl Tool {
    pub const ALL: [Tool; 11] = [Tool::Select, Tool::Move, Tool::Rotate, Tool::Scale, Tool::Anchor, Tool::Pen, Tool::Rect, Tool::Ellipse, Tool::Star, Tool::Polygon, Tool::Text];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Move => "move",
            Tool::Rotate => "rotate",
            Tool::Scale => "scale",
            Tool::Anchor => "anchor",
            Tool::Pen => "pen",
            Tool::Rect => "rect",
            Tool::Ellipse => "ellipse",
            Tool::Star => "star",
            Tool::Polygon => "polygon",
            Tool::Text => "text",
        }
    }

    pub fn parse(s: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.name() == s)
    }

    /// Draws a new layer by dragging on the 2D canvas.
    pub fn is_shape(self) -> bool {
        matches!(self, Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon)
    }
}

/// Which panel was used last (Delete and A act there).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    Viewport,
    Outliner,
    Properties,
    Timeline,
}

/// Selected parts of the mesh being edited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditSel {
    pub vertices: Vec<u32>,
    pub edges: Vec<(u32, u32)>,
    pub faces: Vec<u32>,
}

impl EditSel {
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() && self.edges.is_empty() && self.faces.is_empty()
    }

    /// Every vertex the selection touches (edges' ends included).
    pub fn all_vertices(&self, faces: &[Vec<u32>]) -> Vec<u32> {
        let mut v: Vec<u32> = self.vertices.clone();
        v.extend(self.edges.iter().flat_map(|(a, b)| [*a, *b]));
        for f in &self.faces {
            if let Some(face) = faces.get(*f as usize) {
                v.extend(face.iter().copied());
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    }

    /// `motion.editMesh` selection params.
    pub fn params(&self) -> Value {
        let mut v: Vec<u32> = self.vertices.clone();
        v.extend(self.edges.iter().flat_map(|(a, b)| [*a, *b]));
        v.sort_unstable();
        v.dedup();
        json!({ "vertices": v, "faces": self.faces })
    }
}

/// The 2D canvas's zoom (screen pixels per project pixel) and pan (screen pixels the canvas
/// centre sits from the view's centre); `fit` follows the view's size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Canvas2d {
    pub zoom: f64,
    pub pan: [f64; 2],
    pub fit: bool,
}

/// One keyframe picked in the dope sheet or graph editor: thing, property, scene time.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyRef {
    pub id: String,
    pub property: String,
    pub time: f64,
}

/// One step of moving around the 3D view (the editor's camera, or the scene's when it is locked
/// to the view).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nav {
    /// Around what it looks at, degrees left/right and up/down.
    Orbit(f64, f64),
    /// Sideways and up/down by a share of what it shows.
    Pan(f64, f64),
    /// Closer (< 1) or further (> 1), towards a point when given (zoom to the pointer).
    Zoom(f64, Option<[f64; 3]>),
    /// Through the scene: forward, right, up (world units).
    Fly(f64, f64, f64),
    /// Turning where it stands, degrees left/right and up/down.
    Look(f64, f64),
}

impl Nav {
    pub fn apply(self, v: &mut ViewCamera) {
        match self {
            Nav::Orbit(yaw, pitch) => v.orbit(yaw, pitch),
            Nav::Pan(dx, dy) => v.pan(dx, dy),
            Nav::Zoom(f, Some(p)) => v.zoom_toward(f, p),
            Nav::Zoom(f, None) => v.zoom(f),
            Nav::Fly(f, r, u) => v.fly(f, r, u),
            Nav::Look(yaw, pitch) => v.look(yaw, pitch),
        }
    }
}

/// The scene's camera moved from the view: its undo key and where it has got to (ahead of the
/// project, which follows a moment later).
struct CameraLock {
    key: String,
    id: String,
    pose: ViewCamera,
    last: Instant,
}

/// How long a smooth view change takes.
const VIEW_MOVE: Duration = Duration::from_millis(280);

pub enum StudioEvent {
    /// Back to the edit: the workspace takes the keyboard again.
    Closed,
}

impl EventEmitter<StudioEvent> for Studio {}

/// Edits sent while dragging: one batch at a time, the newest waiting (older ones are dropped,
/// the newest always lands).
#[derive(Default)]
struct Sender {
    busy: bool,
    queued: Option<Vec<(String, Value)>>,
}

pub struct Studio {
    pub store: Entity<Store>,
    pub focus: FocusHandle,
    pub clip: Option<Id>,
    /// Selected keys (ids, `scene`, `material:<id>`, `comp:<id>`); the last is the active one.
    pub selection: Vec<String>,
    pub mode: Mode,
    pub select_mode: SelectMode,
    pub edit_sel: EditSel,
    pub tool: Tool,
    pub shading: Shading,
    pub grid: bool,
    pub helpers: bool,
    pub through_camera: bool,
    pub view: ViewCamera,
    /// Navigating while looking through the camera moves the scene's camera (Blender's "Lock
    /// camera to view"): one undo step per gesture, a keyframe at the playhead when it is animated.
    pub lock_camera: bool,
    /// Side panel shown over the viewport when the window is narrow.
    pub drawer: Option<bool>,
    /// A smooth move of the editor camera in progress: where it ends, and whether it then looks
    /// through the scene's camera.
    view_goal: Option<(ViewCamera, bool)>,
    _view_anim: Option<Task<()>>,
    /// The scene's camera being moved from the view (lock camera to view).
    lock: Option<CameraLock>,
    /// A navigation gesture (drag) is on: the lock's undo step lasts until it ends.
    gesture: bool,
    /// Fly mode is on in the viewport (it says so).
    pub flying: bool,
    /// The gizmo follows the object's own axes.
    pub local: bool,
    /// Snap moves to the grid and turns to 15° (Ctrl also does while dragging).
    pub snapping: bool,
    pub canvas: Canvas2d,
    /// 2D: the composition shown instead of the scene.
    pub composition: Option<String>,
    /// 2D pen: draw a mask on the selected layer instead of a new path layer.
    pub mask_mode: bool,
    pub show_graph: bool,
    /// The graph editor's property (of the active item).
    pub graph_property: Option<String>,
    /// Outliner rows (and dope sheet items) folded closed.
    pub collapsed: HashSet<String>,
    /// Dope sheet rows opened to their properties.
    pub expanded: HashSet<String>,
    pub area: Area,
    pub keys: Vec<KeyRef>,
    pub playing: bool,
    _play: Option<Task<()>>,
    /// Space is held: a drag pans the view; let go without a drag, it plays.
    pub space_down: bool,
    pub space_panned: bool,
    pub popover: Option<Popover>,
    sender: Sender,
    drag_n: u64,
    left_w: f32,
    right_w: f32,
    bottom_h: f32,
    resizing: Option<(u8, Pixels, f32)>,
    pub viewport: Entity<Viewport>,
    pub outliner: Entity<Outliner>,
    pub properties: Entity<Properties>,
    pub timeline: Entity<StudioTimeline>,
    _subs: Vec<Subscription>,
}

impl Focusable for Studio {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Studio {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![cx.observe(&store, |this, _, cx| this.project_changed(cx)), cx.observe(&playback, |this, _, cx| if this.clip.is_some() { cx.notify() })];
        let me = cx.entity();
        Self {
            focus: cx.focus_handle(),
            clip: None,
            selection: vec![],
            mode: Mode::Object,
            select_mode: SelectMode::Vertex,
            edit_sel: EditSel::default(),
            tool: Tool::Move,
            shading: Shading::Material,
            grid: true,
            helpers: true,
            through_camera: false,
            view: ViewCamera::default(),
            lock_camera: false,
            drawer: None,
            view_goal: None,
            _view_anim: None,
            lock: None,
            gesture: false,
            flying: false,
            local: false,
            snapping: false,
            canvas: Canvas2d { zoom: 0.5, pan: [0.0, 0.0], fit: true },
            composition: None,
            mask_mode: false,
            show_graph: false,
            graph_property: None,
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            area: Area::Viewport,
            keys: vec![],
            playing: false,
            _play: None,
            space_down: false,
            space_panned: false,
            popover: None,
            sender: Sender::default(),
            drag_n: 0,
            left_w: LEFT_W,
            right_w: RIGHT_W,
            bottom_h: BOTTOM_H,
            resizing: None,
            viewport: cx.new(|cx| Viewport::new(me.clone(), window, cx)),
            outliner: cx.new(|cx| Outliner::new(me.clone(), window, cx)),
            properties: cx.new(|cx| Properties::new(me.clone(), window, cx)),
            timeline: cx.new(|cx| StudioTimeline::new(me.clone(), window, cx)),
            store,
            _subs: subs,
        }
    }

    pub fn is_open(&self) -> bool {
        self.clip.is_some()
    }

    // ---- reading ----------------------------------------------------------------------------

    pub fn project(&self, cx: &App) -> Option<Arc<Project>> {
        self.store.read(cx).project.clone()
    }

    /// The open clip and its scene (cloned out of the project).
    pub fn clip_scene(&self, cx: &App) -> Option<(Clip, Scene)> {
        let p = self.project(cx)?;
        let (c, s) = model::motion_clip(&p, self.clip?)?;
        Some((c.clone(), s.clone()))
    }

    pub fn is_3d(&self, cx: &App) -> bool {
        let Some(p) = self.project(cx) else { return false };
        self.clip.and_then(|id| model::motion_clip(&p, id)).is_some_and(|(_, s)| s.is_3d())
    }

    /// The playhead (timeline seconds).
    pub fn playhead(&self, cx: &App) -> f64 {
        self.store.read(cx).playback.read(cx).playhead
    }

    /// The scene time shown: the clip's at the playhead (held at its ends).
    pub fn scene_time(&self, cx: &App) -> f64 {
        let p = self.project(cx);
        match p.as_ref().and_then(|p| model::motion_clip(p, self.clip?)) {
            Some((c, _)) => model::scene_time(c, self.playhead(cx)),
            None => 0.0,
        }
    }

    /// The active (last selected) thing.
    pub fn active(&self) -> Option<&str> {
        self.selection.last().map(String::as_str)
    }

    // ---- opening and closing ----------------------------------------------------------------

    pub fn open(&mut self, clip: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.project(cx) else { return };
        let Some((c, scene)) = model::motion_clip(&p, clip) else { return };
        let (start, end) = (c.start, c.end());
        let again = self.clip == Some(clip);
        self.clip = Some(clip);
        if !again {
            self.selection.clear();
            self.mode = Mode::Object;
            self.edit_sel = EditSel::default();
            self.composition = None;
            self.keys.clear();
            self.graph_property = None;
            self.through_camera = false;
            self.canvas.fit = true;
            self.tool = if scene.is_3d() { Tool::Move } else { Tool::Select };
            self.view = ViewCamera::default();
            self.stop_view_anim();
            self.lock = None;
            if let Scene::Space(s) = scene {
                let s = s.clone();
                self.frame_all(&s, false, cx);
            }
        }
        // The playhead goes into the clip, so the scene shows.
        let t = self.playhead(cx);
        if t < start || t >= end {
            let pb = self.store.read(cx).playback.clone();
            pb.update(cx, |p, cx| p.seek(start, cx));
        }
        window.focus(&self.focus, cx);
        self.changed(cx);
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.clip.is_none() {
            return;
        }
        // A tool in progress is put back first (it needs the clip).
        if self.viewport.read(cx).busy() {
            let me = cx.entity();
            self.viewport_soon(cx, move |v, cx| {
                v.cancel_modal(cx);
                cx.defer(move |cx| me.update(cx, |s, cx| s.close(cx)));
            });
            return;
        }
        self.stop(cx);
        self.clip = None;
        self.popover = None;
        self.store.update(cx, |s, cx| s.set_studio_state(None, cx));
        cx.emit(StudioEvent::Closed);
        cx.notify();
    }

    /// The project changed: drop what no longer exists; close if the clip went.
    fn project_changed(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.clip else { return };
        let Some(p) = self.project(cx) else {
            self.close(cx);
            return;
        };
        let Some((_, scene)) = model::motion_clip(&p, id) else {
            self.close(cx);
            return;
        };
        let before = self.selection.len();
        self.selection.retain(|k| model::item(scene, k).is_some());
        if let Some(c) = &self.composition
            && !matches!(scene, Scene::Flat(f) if f.composition(c).is_some())
        {
            self.composition = None;
        }
        if self.mode == Mode::Edit && !self.active().is_some_and(|a| matches!(model::item(scene, a), Some(model::Item::Object("mesh")))) {
            self.mode = Mode::Object;
            self.edit_sel = EditSel::default();
        }
        if before != self.selection.len() {
            self.publish(cx);
        }
        cx.notify();
    }

    // ---- state ------------------------------------------------------------------------------

    /// Something the Studio shows changed: redraw and tell `ui.state`.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.publish(cx);
        cx.notify();
    }

    fn publish(&self, cx: &mut Context<Self>) {
        let state = self.clip.map(|_| self.state_json(cx));
        self.store.update(cx, |s, cx| s.set_studio_state(state, cx));
    }

    /// The Studio shares the editor row with a docked Agent panel.
    pub(super) fn available_width(&self, window: &Window, cx: &App) -> f32 {
        let store = self.store.read(cx);
        let layout = store.session.ui_state().layout;
        let agent = if store.agent_open && !layout.overlays.iter().any(|p| p == "agent") {
            layout.agent + crate::ui::layout::SPLITTER_W
        } else { 0.0 };
        (f32::from(window.viewport_size().width) - agent).max(1.0)
    }

    /// What `ui.studio` answers.
    pub fn state_json(&self, cx: &App) -> Value {
        let Some(id) = self.clip else { return json!({ "open": false }) };
        let (name, kind) = self.project(cx).and_then(|p| model::motion_clip(&p, id).map(|(c, s)| (c.name.clone(), if s.is_3d() { "3d" } else { "2d" }))).unwrap_or_default();
        let mut v = json!({
            "open": true,
            "clipId": id,
            "clipName": name,
            "kind": kind,
            "panel": self.drawer.map(|right| if right { "properties" } else { "objects" }),
            "selection": self.selection,
            "active": self.active(),
            "mode": if self.mode == Mode::Edit { "edit" } else { "object" },
            "tool": self.tool.name(),
            "sceneTime": (self.scene_time(cx) * 1000.0).round() / 1000.0,
            "playing": self.playing,
            "showGraph": self.show_graph,
            "graphProperty": self.graph_property,
            "selectedKeys": self.keys.iter().map(|k| json!({ "id": k.id, "property": k.property, "time": k.time })).collect::<Vec<_>>(),
        });
        if kind == "3d" {
            v["shading"] = json!(self.shading);
            let (view, through) = self.view_goal.unwrap_or((self.view, self.through_camera));
            v["view"] = if through { json!("camera") } else { json!(view) };
            v["lockCamera"] = json!(self.lock_camera);
            v["flying"] = json!(self.flying);
            v["grid"] = json!(self.grid);
            v["helpers"] = json!(self.helpers);
            v["gizmo"] = json!(if self.local { "local" } else { "global" });
            if self.mode == Mode::Edit {
                v["selectMode"] = json!(self.select_mode.name());
                v["editSelection"] = json!({ "vertices": self.edit_sel.vertices, "edges": self.edit_sel.edges, "faces": self.edit_sel.faces });
            }
        } else {
            v["composition"] = json!(self.composition);
            v["zoom"] = json!((self.canvas.zoom * 1000.0).round() / 1000.0);
            v["fit"] = json!(self.canvas.fit);
            v["pan"] = json!(self.canvas.pan.map(|p| p.round()));
            v["maskMode"] = json!(self.mask_mode);
        }
        v
    }

    /// `ui.studio`: applies each given parameter in order, then answers with the state.
    pub fn ui_command(&mut self, params: Value, window: &mut Window, cx: &mut Context<Self>) -> CmdResult {
        if let Some(id) = params.get("clipId").and_then(Value::as_str) {
            let id: Id = id.parse().map_err(|_| format!("bad clip id {id}"))?;
            self.open(id, window, cx);
            if self.clip != Some(id) {
                return Err("That clip isn't a motion clip.".into());
            }
        }
        if params.get("close").and_then(Value::as_bool) == Some(true) {
            self.close(cx);
            return Ok(self.state_json(cx));
        }
        if self.clip.is_none() {
            return if params.as_object().is_none_or(|o| o.is_empty()) {
                Ok(self.state_json(cx))
            } else {
                Err("The Studio isn't open. Open a motion clip first: ui.studio {\"clipId\": …}.".into())
            };
        }
        if let Some(panel) = params.get("panel") {
            self.drawer = match panel.as_str() {
                Some("objects") => Some(false), Some("properties") => Some(true), Some("none") => None,
                _ => return Err("panel must be objects, properties or none".into()),
            };
        }
        let (_, scene) = self.clip_scene(cx).ok_or("The clip is gone.")?;
        if let Some(c) = params.get("composition") {
            let c = c.as_str().unwrap_or("");
            match &scene {
                Scene::Flat(f) if c.is_empty() || f.composition(c).is_some() => self.composition = (!c.is_empty()).then(|| c.to_string()),
                Scene::Flat(f) => return Err(format!("No composition \"{c}\". Compositions: {}.", f.compositions.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(", "))),
                Scene::Space(_) => return Err("Compositions are 2D; this is a 3D scene.".into()),
            }
        }
        if let Some(list) = params.get("select") {
            let keys: Vec<String> = list.as_array().ok_or("select is a list of ids")?.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
            if let Some(bad) = keys.iter().find(|k| model::item(&scene, k).is_none()) {
                return Err(format!("No \"{bad}\" in this scene. Ids: {}.", scene.ids().join(", ")));
            }
            self.set_selection(keys, cx);
        }
        if let Some(m) = params.get("mode").and_then(Value::as_str) {
            self.set_mode(if m == "edit" { Mode::Edit } else { Mode::Object }, cx)?;
        }
        if let Some(m) = params.get("selectMode").and_then(Value::as_str) {
            self.select_mode = match m {
                "edge" => SelectMode::Edge,
                "face" => SelectMode::Face,
                _ => SelectMode::Vertex,
            };
        }
        if let Some(e) = params.get("editSelection") {
            let ints = |k: &str| -> Vec<u32> { e.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_u64().map(|n| n as u32)).collect() };
            self.edit_sel = EditSel { vertices: ints("vertices"), edges: vec![], faces: ints("faces") };
        }
        if let Some(tool) = params.get("tool").and_then(Value::as_str) {
            self.tool = Tool::parse(tool).ok_or_else(|| format!("unknown tool {tool}"))?;
        }
        if let Some(sh) = params.get("shading").and_then(Value::as_str) {
            self.shading = match sh {
                "solid" => Shading::Solid,
                "rendered" => Shading::Rendered,
                _ => Shading::Material,
            };
        }
        if let Some(v) = params.get("view") {
            match v {
                Value::String(s) => self.set_view(s, cx)?,
                Value::Object(_) => {
                    self.view = serde_json::from_value(v.clone()).map_err(|e| format!("view: {e} (give position, target, fov, ortho, orthoSize)"))?;
                    self.through_camera = false;
                }
                _ => return Err("view is an axis name, \"camera\", or a view object".into()),
            }
        }
        if let Some(b) = params.get("grid").and_then(Value::as_bool) {
            self.grid = b;
        }
        if let Some(b) = params.get("helpers").and_then(Value::as_bool) {
            self.helpers = b;
        }
        if params.get("frame").and_then(Value::as_bool) == Some(true) {
            self.frame_selection(false, cx);
        }
        if let Some(b) = params.get("lockCamera").and_then(Value::as_bool) {
            self.set_lock_camera(b, cx);
            self.finish_view_anim(cx);
        }
        if let Some(n) = params.get("navigate") {
            if !scene.is_3d() {
                return Err("navigate is for 3D scenes; a 2D canvas takes zoom and pan.".into());
            }
            self.navigate_json(n, cx)?;
        }
        if params.get("alignCamera").and_then(Value::as_bool) == Some(true) {
            self.align_camera_to_view(cx)?;
        }
        if params.get("addCamera").and_then(Value::as_bool) == Some(true) {
            self.add_camera_here(cx)?;
        }
        if let Some(b) = params.get("keyframeCamera").and_then(Value::as_bool) {
            self.keyframe_camera(Some(b), cx)?;
        }
        if let Some(b) = params.get("fly").and_then(Value::as_bool) {
            if !scene.is_3d() {
                return Err("Fly mode is for 3D scenes.".into());
            }
            self.viewport_soon(cx, move |v, cx| if b { v.start_fly(false, cx) } else { v.end_fly(true, cx) });
        }
        if let Some(z) = params.get("zoom") {
            if scene.is_3d() {
                return Err("zoom is for the 2D canvas; move the 3D view with navigate {\"zoom\": 2}.".into());
            }
            match z {
                Value::String(w) if w == "fit" => self.canvas.fit = true,
                Value::String(w) if w == "100%" => self.zoom_canvas_to(1.0, cx),
                Value::Number(n) => self.zoom_canvas_to(n.as_f64().unwrap_or(1.0).clamp(0.02, 32.0), cx),
                _ => return Err("zoom is \"fit\", \"100%\" or a number (1 = 100%)".into()),
            }
        }
        if let Some(p) = params.get("pan") {
            let xy = p.as_array().and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])).ok_or("pan is [x, y]: screen pixels the canvas's centre sits from the view's")?;
            if scene.is_3d() {
                return Err("pan is for the 2D canvas; move the 3D view with navigate {\"pan\": [dx, dy]}.".into());
            }
            let zoom = self.shown_zoom(cx);
            self.canvas = Canvas2d { zoom, pan: xy, fit: false };
        }
        if let Some(b) = params.get("showGraph").and_then(Value::as_bool) {
            self.show_graph = b;
        }
        if let Some(p) = params.get("graphProperty").and_then(Value::as_str) {
            self.graph_property = Some(p.to_string());
            self.show_graph = true;
        }
        self.changed(cx);
        Ok(self.state_json(cx))
    }

    // ---- selection and modes ----------------------------------------------------------------

    pub fn set_selection(&mut self, keys: Vec<String>, cx: &mut Context<Self>) {
        if self.selection != keys {
            self.selection = keys;
            if self.mode == Mode::Edit {
                self.mode = Mode::Object;
                self.edit_sel = EditSel::default();
            }
        }
        self.changed(cx);
    }

    /// Click (replace), shift/cmd-click (toggle).
    pub fn select(&mut self, key: &str, additive: bool, cx: &mut Context<Self>) {
        let mut sel = self.selection.clone();
        if additive {
            if let Some(i) = sel.iter().position(|k| k == key) {
                // A second click on a selected (not active) one makes it active; on the active one, deselects.
                if i + 1 == sel.len() {
                    sel.remove(i);
                } else {
                    let k = sel.remove(i);
                    sel.push(k);
                }
            } else {
                sel.push(key.to_string());
            }
        } else {
            sel = vec![key.to_string()];
        }
        self.set_selection(sel, cx);
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) -> Result<(), String> {
        if mode == Mode::Edit {
            let (_, scene) = self.clip_scene(cx).ok_or("no clip")?;
            if !scene.is_3d() {
                return Err("Edit mode is for 3D meshes; 2D paths are drawn with the pen.".into());
            }
            match self.active().and_then(|a| model::item(&scene, a)) {
                Some(model::Item::Object("mesh")) => {}
                Some(model::Item::Object(other)) => {
                    return Err(format!("\"{}\" is a {other}; convert it to a mesh first (motion.convertToMesh, or its right-click menu).", self.active().unwrap_or("")));
                }
                _ => return Err("Select a mesh object to edit it.".into()),
            }
        }
        if self.mode != mode {
            self.edit_sel = EditSel::default();
        }
        self.mode = mode;
        self.changed(cx);
        Ok(())
    }

    /// Tab: into edit mode (offering to convert other shapes), or back out.
    pub fn toggle_edit(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Edit {
            let _ = self.set_mode(Mode::Object, cx);
            return;
        }
        let Some((_, scene)) = self.clip_scene(cx) else { return };
        if !scene.is_3d() {
            return;
        }
        let Some(active) = self.active().map(str::to_string) else {
            flash("Select a mesh object, then press Tab to edit it.", cx);
            return;
        };
        match model::item(&scene, &active) {
            Some(model::Item::Object("mesh")) => {
                let _ = self.set_mode(Mode::Edit, cx);
            }
            Some(model::Item::Object(shape)) if !matches!(shape, "group" | "particles" | "model" | "image") => {
                // Like Blender's "convert to mesh" first: one step, then straight into edit mode.
                let clip = self.clip;
                self.run_then("motion.convertToMesh", json!({ "clipId": clip, "id": active }), cx, |this, _, cx| {
                    this.mode = Mode::Object;
                    let _ = this.set_mode(Mode::Edit, cx);
                });
            }
            _ => flash("Only mesh objects have an edit mode.", cx),
        }
    }

    pub fn set_view(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        self.stop_view_anim();
        match name {
            "camera" => self.through_camera = true,
            "persp" | "perspective" => {
                self.through_camera = false;
                self.view.ortho = false;
            }
            "ortho" | "orthographic" => {
                self.through_camera = false;
                self.view.ortho = true;
            }
            "front" | "back" | "left" | "right" | "top" | "bottom" => {
                self.through_camera = false;
                self.view.align(name);
            }
            other => return Err(format!("view is front, back, left, right, top, bottom, camera, persp or ortho, not `{other}`")),
        }
        self.changed(cx);
        Ok(())
    }

    /// A view from the window (a click on the axis ball, a number key): it glides there.
    pub fn look_from(&mut self, name: &str, cx: &mut Context<Self>) {
        let base = self.view_goal.map(|g| g.0).unwrap_or(self.view);
        match name {
            "camera" => {
                if let Some(cam) = self.camera_view(cx) {
                    self.go_to(cam, true, cx);
                }
            }
            "front" | "back" | "left" | "right" | "top" | "bottom" => {
                let mut v = if self.through_camera { self.camera_view(cx).unwrap_or(base) } else { base };
                v.align(name);
                self.go_to(v, false, cx);
            }
            other => {
                let _ = self.set_view(other, cx);
            }
        }
    }

    /// The scene's filming camera at the playhead, as a view.
    pub fn camera_view(&self, cx: &App) -> Option<ViewCamera> {
        let (_, scene) = self.clip_scene(cx)?;
        let Scene::Space(s) = scene else { return None };
        Some(ViewCamera::from_camera(&s.camera_at(self.scene_time(cx))))
    }

    /// Where the view is going (the end of a smooth move), or where it is.
    pub fn view_shown(&self) -> (ViewCamera, bool) {
        self.view_goal.unwrap_or((self.view, self.through_camera))
    }

    /// Moves the editor camera to `goal` smoothly (at once when the person asks the system to
    /// reduce motion); `through` ends looking through the scene's camera.
    pub fn go_to(&mut self, goal: ViewCamera, through: bool, cx: &mut Context<Self>) {
        self.stop_view_anim();
        let from = if self.through_camera { self.camera_view(cx).unwrap_or(self.view) } else { self.view };
        if cx.reduce_motion() || (from == goal && through == self.through_camera) {
            self.view = goal;
            self.through_camera = through;
            self.changed(cx);
            return;
        }
        self.through_camera = false;
        self.view = from;
        self.view_goal = Some((goal, through));
        let start = Instant::now();
        self._view_anim = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let more = this
                    .update(cx, |this, cx| {
                        let Some((goal, through)) = this.view_goal else { return false };
                        let k = (start.elapsed().as_secs_f64() / VIEW_MOVE.as_secs_f64()).min(1.0);
                        if k >= 1.0 {
                            this.view = goal;
                            this.through_camera = through;
                            this.view_goal = None;
                            this.changed(cx);
                            return false;
                        }
                        // Ease out: quick to start, settling softly.
                        this.view = from.blend(&goal, 1.0 - (1.0 - k).powi(3));
                        this.viewport_soon(cx, |v, cx| v.interacting(cx));
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        }));
        self.changed(cx);
    }

    /// Stops a smooth move where it is (the person took over).
    fn stop_view_anim(&mut self) {
        self.view_goal = None;
        self._view_anim = None;
    }

    /// Ends a smooth move at once (tests, and commands that read the view right after).
    pub fn finish_view_anim(&mut self, cx: &mut Context<Self>) {
        if let Some((goal, through)) = self.view_goal.take() {
            self.view = goal;
            self.through_camera = through;
            self._view_anim = None;
            self.changed(cx);
        }
    }

    /// The editor camera fitted around a box.
    fn framed(&self, lo: [f64; 3], hi: [f64; 3]) -> ViewCamera {
        let base = self.view_goal.map(|g| g.0).unwrap_or(self.view);
        let mut v = base;
        let c = math::lerp(lo, hi, 0.5);
        let r = (math::len(math::sub(hi, lo)) / 2.0).max(0.25);
        let dir = math::norm(math::sub(base.position, base.target));
        let dir = if math::len(dir) < 1e-6 { [0.0, 0.3, 1.0] } else { dir };
        // Room around it: the box's sphere fills about half the view's height.
        let dist = r / (base.fov.to_radians() / 2.0).tan().max(0.05) * 1.9;
        v.target = c;
        v.position = math::add(c, math::scale(dir, dist));
        v.ortho_size = r * 2.4;
        v
    }

    fn frame_box(&mut self, lo: [f64; 3], hi: [f64; 3], animate: bool, cx: &mut Context<Self>) {
        let v = self.framed(lo, hi);
        if animate {
            self.go_to(v, false, cx);
        } else {
            self.stop_view_anim();
            self.view = v;
            self.through_camera = false;
        }
    }

    fn frame_all(&mut self, s: &kimchi_core::motion::Scene3d, animate: bool, cx: &mut Context<Self>) {
        let t = self.scene_time(cx);
        let w = model::worlds(s, t);
        // The subject, not the stage: floors and backdrops (planes, grids) and hidden things
        // (guide curves) only count when there is nothing else.
        use kimchi_core::motion::Shape3d;
        let union = |stage: bool| {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for o in &s.objects {
                let backdrop = o.hidden || matches!(o.shape, Shape3d::Plane { .. } | Shape3d::Grid { .. });
                if backdrop && !stage {
                    continue;
                }
                if let Some((a, b)) = model::world_bounds(s, &w, t, &o.id) {
                    for i in 0..3 {
                        lo[i] = lo[i].min(a[i]);
                        hi[i] = hi[i].max(b[i]);
                    }
                }
            }
            (lo[0] <= hi[0]).then_some((lo, hi))
        };
        if let Some((lo, hi)) = union(false).or_else(|| union(true)) {
            // Look from where the scene's camera looks, so the first view is a familiar one.
            if !animate {
                let cam = s.camera_at(t);
                let dir = math::norm(math::sub(cam.position.0, cam.target.0));
                if math::len(dir) > 1e-6 {
                    self.view.position = math::add(self.view.target, dir);
                }
            }
            self.frame_box(lo, hi, animate, cx);
        }
    }

    /// F / `.`: the selection fills the view (everything when nothing is selected); the view
    /// glides there when `animate`.
    pub fn frame_selection(&mut self, animate: bool, cx: &mut Context<Self>) {
        let Some((_, scene)) = self.clip_scene(cx) else { return };
        match &scene {
            Scene::Space(s) => {
                let t = self.scene_time(cx);
                let w = model::worlds(s, t);
                let mut lo = [f64::INFINITY; 3];
                let mut hi = [f64::NEG_INFINITY; 3];
                for k in &self.selection {
                    if let Some((a, b)) = model::world_bounds(s, &w, t, k) {
                        for i in 0..3 {
                            lo[i] = lo[i].min(a[i]);
                            hi[i] = hi[i].max(b[i]);
                        }
                    }
                }
                if lo[0] <= hi[0] {
                    self.frame_box(lo, hi, animate, cx);
                } else {
                    self.frame_all(s, animate, cx);
                }
            }
            Scene::Flat(_) => self.canvas.fit = true,
        }
        self.changed(cx);
    }

    // ---- moving around ------------------------------------------------------------------------

    /// A navigation gesture begins (a drag): until it ends, moving a locked camera is one undo
    /// step.
    pub fn nav_begin(&mut self) {
        self.stop_view_anim();
        self.lock = None;
        self.gesture = true;
    }

    /// The gesture ended: `ui.state` hears where the view is.
    pub fn nav_end(&mut self, cx: &mut Context<Self>) {
        self.gesture = false;
        self.changed(cx);
    }

    /// One step of moving around: the editor's camera, or with the camera locked to the view
    /// while looking through it, the scene's camera (a command).
    pub fn navigate(&mut self, op: Nav, cx: &mut Context<Self>) {
        self.stop_view_anim();
        if self.through_camera && self.lock_camera {
            self.move_camera(op, cx);
            return;
        }
        if self.through_camera {
            // Off the camera's view, from where the camera is (no jump), like Blender.
            if let Some(v) = self.camera_view(cx) {
                self.view = v;
            }
            self.through_camera = false;
            self.publish(cx);
        }
        op.apply(&mut self.view);
        cx.notify();
    }

    /// `ui.studio`'s navigate: {"orbit": [yaw, pitch], "pan": [dx, dy], "zoom": 2, "fly":
    /// [forward, right, up], "look": [yaw, pitch]}, applied in that order as one step.
    fn navigate_json(&mut self, n: &Value, cx: &mut Context<Self>) -> Result<(), String> {
        let o = n.as_object().ok_or("navigate is an object: {\"orbit\": [yaw, pitch], \"pan\": [dx, dy], \"zoom\": 2, \"fly\": [forward, right, up], \"look\": [yaw, pitch]}")?;
        let nums = |k: &str, len: usize| -> Result<Option<Vec<f64>>, String> {
            match o.get(k) {
                None => Ok(None),
                Some(v) => {
                    let list: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                    if list.len() != len {
                        return Err(format!("navigate.{k} is a list of {len} numbers"));
                    }
                    Ok(Some(list))
                }
            }
        };
        if let Some(bad) = o.keys().find(|k| !matches!(k.as_str(), "orbit" | "pan" | "zoom" | "fly" | "look")) {
            return Err(format!("navigate takes orbit, pan, zoom, fly and look, not `{bad}`"));
        }
        let mut ops = vec![];
        if let Some(v) = nums("orbit", 2)? {
            ops.push(Nav::Orbit(v[0], v[1]));
        }
        if let Some(v) = nums("pan", 2)? {
            ops.push(Nav::Pan(v[0], v[1]));
        }
        if let Some(z) = o.get("zoom") {
            let z = z.as_f64().filter(|z| *z > 0.0).ok_or("navigate.zoom is a number above 0: 2 = twice as close, 0.5 = twice as far")?;
            ops.push(Nav::Zoom(1.0 / z, None));
        }
        if let Some(v) = nums("fly", 3)? {
            ops.push(Nav::Fly(v[0], v[1], v[2]));
        }
        if let Some(v) = nums("look", 2)? {
            ops.push(Nav::Look(v[0], v[1]));
        }
        // One undo step for the whole call.
        self.nav_begin();
        for op in ops {
            self.navigate(op, cx);
        }
        self.gesture = false;
        Ok(())
    }

    /// Lock camera to view; turning it on looks through the camera (where it acts).
    pub fn set_lock_camera(&mut self, on: bool, cx: &mut Context<Self>) {
        self.lock_camera = on;
        self.lock = None;
        if on && !self.view_shown().1 {
            self.look_from("camera", cx);
        }
        self.changed(cx);
    }

    /// The id of the camera filming at the playhead (3D).
    pub fn active_camera(&self, cx: &App) -> Option<String> {
        match self.clip_scene(cx)? {
            (_, Scene::Space(s)) => Some(s.active_camera_at(self.scene_time(cx))),
            _ => None,
        }
    }

    /// Lock camera to view: the scene's camera moves as the view would, through
    /// `motion.updateLayer` (a keyframe at the playhead when it is animated).
    fn move_camera(&mut self, op: Nav, cx: &mut Context<Self>) {
        let Some((c, Scene::Space(s))) = self.clip_scene(cx) else { return };
        let t = self.scene_time(cx);
        let id = s.active_camera_at(t);
        let Some(src) = s.camera_by_id(&id) else { return };
        if src.constraints.iter().any(|k| k.enabled && matches!(k.kind.as_str(), "followPath" | "copyPosition")) {
            flash("This camera follows a path (a constraint): clear its moves (Camera menu) to move it by hand.", cx);
            return;
        }
        let stale = self.lock.as_ref().is_none_or(|l| l.id != id || (!self.gesture && l.last.elapsed() > Duration::from_millis(700)));
        if stale {
            let pose = ViewCamera::from_camera(&s.camera_by_id_at(&id, t).unwrap_or_else(|| s.camera_at(t)));
            self.lock = Some(CameraLock { key: self.drag_key(), id: id.clone(), pose, last: Instant::now() });
        }
        let time = self.playhead(cx);
        let Some(lock) = self.lock.as_mut() else { return };
        op.apply(&mut lock.pose);
        lock.last = Instant::now();
        let r = |v: [f64; 3]| v.map(|x| (x * 10000.0).round() / 10000.0);
        let mut props = json!({ "position": r(lock.pose.position), "target": r(lock.pose.target) });
        if lock.pose.ortho {
            props["orthoSize"] = json!((lock.pose.ortho_size * 10000.0).round() / 10000.0);
        }
        let params = json!({ "clipId": c.id, "id": id, "props": props, "time": time, "coalesce": lock.key });
        self.send(vec![("motion.updateLayer".into(), params)], cx);
        self.viewport_soon(cx, |v, cx| v.interacting(cx));
    }

    /// Puts the locked camera back where the gesture found it (a cancelled fly).
    pub fn put_camera_back(&mut self, pose: ViewCamera, cx: &mut Context<Self>) {
        if let Some(lock) = self.lock.as_mut() {
            lock.pose = pose;
            lock.last = Instant::now();
            // A step that moves nothing sends the pose, with the gesture's undo key.
            self.move_camera(Nav::Pan(0.0, 0.0), cx);
        }
    }

    /// Ctrl+Alt+0: the active camera goes where the view is (and the view looks through it).
    pub fn align_camera_to_view(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let (c, scene) = self.clip_scene(cx).ok_or("no clip")?;
        let Scene::Space(s) = scene else { return Err("Cameras are 3D.".into()) };
        let id = s.active_camera_at(self.scene_time(cx));
        let v = self.view_shown().0;
        let r = |v: [f64; 3]| v.map(|x| (x * 10000.0).round() / 10000.0);
        let mut props = json!({ "position": r(v.position), "target": r(v.target), "fov": (v.fov * 100.0).round() / 100.0, "projection": if v.ortho { "orthographic" } else { "perspective" } });
        if v.ortho {
            props["orthoSize"] = json!((v.ortho_size * 10000.0).round() / 10000.0);
        }
        self.stop_view_anim();
        self.run_then("motion.updateLayer", json!({ "clipId": c.id, "id": id, "props": props, "time": self.playhead(cx) }), cx, |this, _, cx| {
            this.through_camera = true;
            this.changed(cx);
        });
        Ok(())
    }

    /// A new camera where the view is, selected.
    pub fn add_camera_here(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let (_, scene) = self.clip_scene(cx).ok_or("no clip")?;
        if !scene.is_3d() {
            return Err("Cameras are 3D.".into());
        }
        let v = self.view_shown().0;
        let r = |v: [f64; 3]| v.map(|x| (x * 100.0).round() / 100.0);
        let me = cx.entity();
        let camera = json!({ "type": "camera", "position": r(v.position), "target": r(v.target), "fov": (v.fov * 10.0).round() / 10.0 });
        // After this update: adding reads the Studio.
        cx.defer(move |cx| menus::add_thing(&me, "camera", camera, None, cx));
        Ok(())
    }

    /// The camera the camera tools act on: the selected one, else the one filming.
    pub fn camera_in_hand(&self, cx: &App) -> Option<String> {
        let (_, scene) = self.clip_scene(cx)?;
        if !scene.is_3d() {
            return None;
        }
        self.active().filter(|a| matches!(model::item(&scene, a), Some(model::Item::Camera))).map(str::to_string).or_else(|| self.active_camera(cx))
    }

    /// Whether the camera in hand has a position keyframe at the playhead.
    pub fn camera_keyed_here(&self, cx: &App) -> bool {
        let Some((_, scene)) = self.clip_scene(cx) else { return false };
        let Some(id) = self.camera_in_hand(cx) else { return false };
        let t = self.scene_time(cx);
        model::keyframes(&scene, &id).and_then(|k| k.get("position").cloned()).is_some_and(|l| l.iter().any(|k| (k.time - t).abs() < 1e-3))
    }

    /// The camera's position and target keyframed at the playhead (`Some(true)`), the keys there
    /// removed (`Some(false)`), or whichever is the change (`None`, the toggle).
    pub fn keyframe_camera(&mut self, on: Option<bool>, cx: &mut Context<Self>) -> Result<(), String> {
        let id = self.camera_in_hand(cx).ok_or("Cameras are in 3D scenes.")?;
        let on = on.unwrap_or(!self.camera_keyed_here(cx));
        let clip = self.clip;
        let time = self.playhead(cx);
        let commands: Vec<Value> = ["position", "target"]
            .iter()
            .map(|p| {
                if on {
                    json!({ "command": "motion.addKeyframe", "params": { "clipId": clip, "id": id, "property": p, "time": time } })
                } else {
                    json!({ "command": "motion.removeKeyframe", "params": { "clipId": clip, "id": id, "property": p, "time": time } })
                }
            })
            .collect();
        self.run("project.batch", json!({ "commands": commands, "label": if on { "Keyframe the camera" } else { "Remove the camera's keyframes" }, "atomic": false }), cx);
        Ok(())
    }

    /// A camera move from the window (the Camera menu): around the selection's middle when
    /// something is selected, over the whole clip.
    pub fn camera_move(&mut self, mv: &str, extra: Value, cx: &mut Context<Self>) {
        let Some((c, Scene::Space(s))) = self.clip_scene(cx) else { return };
        let t = self.scene_time(cx);
        let w = model::worlds(&s, t);
        let mut params = json!({ "clipId": c.id, "move": mv });
        if let Some(cam) = self.camera_in_hand(cx) {
            params["camera"] = json!(cam);
        }
        if matches!(mv, "orbit" | "turntable") {
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            let scene = Scene::Space(s.clone());
            for k in self.selection.iter().filter(|k| matches!(model::item(&scene, k), Some(model::Item::Object(_)))) {
                if let Some((a, b)) = model::world_bounds(&s, &w, t, k) {
                    for i in 0..3 {
                        lo[i] = lo[i].min(a[i]);
                        hi[i] = hi[i].max(b[i]);
                    }
                }
            }
            if lo[0] <= hi[0] {
                params["around"] = json!(math::lerp(lo, hi, 0.5).map(|v| (v * 1000.0).round() / 1000.0));
            }
        }
        if let (Some(o), Some(e)) = (params.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
            // A share of the way to what the camera looks at, as a distance.
            if let Some(share) = o.remove("share").and_then(|v| v.as_f64()) {
                let id = self.camera_in_hand(cx).unwrap_or_else(|| "camera".into());
                let d = s.camera_by_id_at(&id, t).map(|c| math::len(math::sub(c.target.0, c.position.0))).unwrap_or(3.0);
                o.insert("distance".into(), json!(((share * d) * 1000.0).round() / 1000.0));
            }
        }
        self.run_then("motion.cameraMove", params, cx, |this, v, cx| {
            if let Some(d) = v["did"].as_str() {
                flash(d.to_string(), cx);
            }
            this.changed(cx);
        });
    }

    // ---- running commands -------------------------------------------------------------------

    pub fn run(&self, name: &str, params: Value, cx: &mut App) {
        run(name, params, cx);
    }

    /// Runs a command; `then` gets its answer on the Studio (errors become toasts).
    pub fn run_then(&self, name: &str, params: Value, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static) {
        let task = self.store.update(cx, |s, cx| s.call(name, params, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| match r {
                Ok(v) => then(this, v, cx),
                Err(e) => this.store.update(cx, |s, cx| s.error(e, cx)),
            })
            .ok();
        })
        .detach();
    }

    /// A command's answer, errors included (for fields that show them inline).
    pub fn call(&self, name: &str, params: Value, cx: &mut App) -> Task<CmdResult> {
        call(name, params, cx)
    }

    /// A fresh coalesce key: every edit sent during one drag folds into one undo step.
    pub fn drag_key(&mut self) -> String {
        self.drag_n += 1;
        format!("studio:{}:{}", self.clip.map(|c| c.to_string()).unwrap_or_default(), self.drag_n)
    }

    /// Sends a drag's edits: while one batch runs, only the newest waits.
    pub fn send(&mut self, batch: Vec<(String, Value)>, cx: &mut Context<Self>) {
        if batch.is_empty() {
            return;
        }
        if self.sender.busy {
            self.sender.queued = Some(batch);
            return;
        }
        self.sender.busy = true;
        let tasks: Vec<Task<CmdResult>> = batch.into_iter().map(|(n, p)| self.call(&n, p, cx)).collect();
        cx.spawn(async move |this, cx| {
            let mut error = None;
            for t in tasks {
                if let Err(e) = t.await {
                    error = Some(e);
                }
            }
            this.update(cx, |this, cx| {
                this.sender.busy = false;
                if let Some(e) = error {
                    this.store.update(cx, |s, cx| s.error(e, cx));
                }
                if let Some(next) = this.sender.queued.take() {
                    this.send(next, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- playback (the clip, looping) ---------------------------------------------------------

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.stop(cx);
        } else {
            self.play(cx);
        }
    }

    fn play(&mut self, cx: &mut Context<Self>) {
        let Some((c, _)) = self.clip_scene(cx) else { return };
        let (start, end) = (c.start, c.end());
        let pb = self.store.read(cx).playback.clone();
        pb.update(cx, |p, cx| p.pause(cx));
        let from = self.playhead(cx).clamp(start, end);
        let from = if from >= end - 1e-3 { start } else { from };
        let fps = self.store.read(cx).fps().clamp(1.0, 60.0);
        self.playing = true;
        let clock = Instant::now();
        self._play = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs_f64(1.0 / fps)).await;
                let go = this
                    .update(cx, |this, cx| {
                        if !this.playing {
                            return false;
                        }
                        let span = (end - start).max(1e-3);
                        let t = start + (from - start + clock.elapsed().as_secs_f64()) % span;
                        let pb = this.store.read(cx).playback.clone();
                        pb.update(cx, |p, cx| p.seek(t, cx));
                        true
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
        }));
        self.changed(cx);
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.playing = false;
            self._play = None;
            self.changed(cx);
        }
    }

    // ---- panels -------------------------------------------------------------------------------

    fn splitter(&self, which: u8, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let vertical = which != 2;
        let active = self.resizing.is_some_and(|(w, _, _)| w == which);
        div()
            .id(("studio-split", which as usize))
            .flex_none()
            .when(vertical, |d| d.w(px(5.)).h_full().cursor_ew_resize().mx(px(-2.)))
            .when(!vertical, |d| d.h(px(5.)).w_full().cursor_ns_resize().my(px(-2.)))
            .relative()
            .child(
                div()
                    .absolute()
                    .when(vertical, |d| d.left(px(2.)).w(px(1.)).h_full())
                    .when(!vertical, |d| d.top(px(2.)).h(px(1.)).w_full())
                    .bg(if active { t.accent } else { t.line }),
            )
            .hover(move |s| s.bg(t.accent_soft))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                if e.click_count == 2 {
                    match which {
                        0 => this.left_w = LEFT_W,
                        1 => this.right_w = RIGHT_W,
                        _ => this.bottom_h = BOTTOM_H,
                    }
                    this.resizing = None;
                } else {
                    let (pos, value) = match which {
                        0 => (e.position.x, this.left_w),
                        1 => (e.position.x, this.right_w),
                        _ => (e.position.y, this.bottom_h),
                    };
                    this.resizing = Some((which, pos, value));
                }
                cx.notify();
            }))
    }

    fn resize_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((which, start, value)) = self.resizing else { return };
        match which {
            0 => self.left_w = (value + f32::from(e.position.x - start)).clamp(180., 480.),
            1 => self.right_w = (value - f32::from(e.position.x - start)).clamp(260., 560.),
            _ => self.bottom_h = (value - f32::from(e.position.y - start)).clamp(120., 600.),
        }
        cx.notify();
    }

    fn resize_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.resizing = None;
        cx.notify();
    }

    // ---- keys ---------------------------------------------------------------------------------

    fn on_escape(&mut self, _: &StudioEscape, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        if store.menu.is_some() || store.dialog.is_some() {
            self.store.update(cx, |s, cx| {
                s.close_menu(cx);
                s.close_dialog(cx);
            });
            return;
        }
        if self.popover.take().is_some() {
            cx.notify();
            return;
        }
        if self.viewport.read(cx).escapable() {
            self.viewport_later(window, cx, |v, w, cx| {
                v.escape(w, cx);
            });
            return;
        }
        if self.mode == Mode::Edit {
            let _ = self.set_mode(Mode::Object, cx);
            return;
        }
        self.close(cx);
    }

    fn with_viewport(&mut self, window: &mut Window, cx: &mut Context<Self>, f: impl FnOnce(&mut Viewport, &mut Window, &mut Context<Viewport>) + 'static) {
        self.area = Area::Viewport;
        self.viewport_later(window, cx, f);
    }

    /// Runs `f` on the viewport once the Studio is done updating (the viewport reads the Studio).
    pub fn viewport_later(&self, window: &Window, cx: &mut App, f: impl FnOnce(&mut Viewport, &mut Window, &mut Context<Viewport>) + 'static) {
        let vp = self.viewport.clone();
        window.defer(cx, move |window, cx| vp.update(cx, |v, cx| f(v, window, cx)));
    }

    /// The same without a window.
    pub fn viewport_soon(&self, cx: &mut App, f: impl FnOnce(&mut Viewport, &mut Context<Viewport>) + 'static) {
        let vp = self.viewport.clone();
        cx.defer(move |cx| vp.update(cx, |v, cx| f(v, cx)));
    }

    fn register_actions(&self, el: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        el.on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(|this, _: &StudioPlay, _, cx| {
                // Played when the key comes up without a drag (Space-drag pans, like After Effects).
                if !this.space_down {
                    this.space_down = true;
                    this.space_panned = false;
                }
                cx.notify();
            }))
            .on_key_up(cx.listener(|this, e: &gpui::KeyUpEvent, _, cx| {
                // Fly mode moves while its keys are down.
                if this.flying {
                    let (key, shift) = (e.keystroke.key.clone(), e.keystroke.modifiers.shift);
                    this.viewport.update(cx, |v, _| v.fly_key_up(&key, shift));
                }
                if e.keystroke.key == "space" && this.space_down {
                    this.space_down = false;
                    if !this.space_panned {
                        this.toggle_play(cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &StudioGrab, w, cx| this.with_viewport(w, cx, |v, w, cx| v.key_grab(w, cx))))
            .on_action(cx.listener(|this, _: &StudioRotate, w, cx| this.with_viewport(w, cx, |v, w, cx| v.start_modal(viewport::ModalKind::Rotate, w, cx))))
            .on_action(cx.listener(|this, _: &StudioScale, w, cx| this.with_viewport(w, cx, |v, w, cx| v.start_modal(viewport::ModalKind::Scale, w, cx))))
            .on_action(cx.listener(|this, _: &StudioAdd, w, cx| this.open_add_menu(None, w, cx)))
            .on_action(cx.listener(|this, _: &StudioDelete, w, cx| this.delete(w, cx)))
            .on_action(cx.listener(|this, _: &StudioDuplicate, _, cx| this.duplicate(cx)))
            .on_action(cx.listener(|this, _: &StudioToggleEdit, _, cx| this.toggle_edit(cx)))
            .on_action(cx.listener(|this, _: &StudioSelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &StudioBoxSelect, w, cx| this.with_viewport(w, cx, |v, _, cx| v.arm_box_select(cx))))
            .on_action(cx.listener(|this, _: &StudioHide, _, cx| this.hide_selected(true, cx)))
            .on_action(cx.listener(|this, _: &StudioUnhide, _, cx| this.hide_selected(false, cx)))
            .on_action(cx.listener(|this, _: &StudioInsert, w, cx| {
                if this.mode == Mode::Edit {
                    this.with_viewport(w, cx, |v, w, cx| v.start_modal(viewport::ModalKind::Inset, w, cx));
                } else {
                    this.keyframe_selection(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &StudioKey1, _, cx| this.number_key(1, cx)))
            .on_action(cx.listener(|this, _: &StudioKey2, _, cx| this.number_key(2, cx)))
            .on_action(cx.listener(|this, _: &StudioKey3, _, cx| this.number_key(3, cx)))
            .on_action(cx.listener(|this, _: &StudioKey7, _, cx| this.number_key(7, cx)))
            .on_action(cx.listener(|this, _: &StudioKey0, _, cx| this.number_key(0, cx)))
            .on_action(cx.listener(|this, _: &StudioOrtho, _, cx| this.toggle_ortho(cx)))
            .on_action(cx.listener(|this, _: &StudioFrame, _, cx| this.frame_selection(true, cx)))
            .on_action(cx.listener(|this, _: &StudioFill, _, cx| {
                // F fills in edit mode, like Blender; it frames otherwise.
                if this.mode == Mode::Edit {
                    this.mesh_op("fill", json!({}), cx);
                } else {
                    this.frame_selection(true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &StudioFrameAll, _, cx| {
                if let Some((_, Scene::Space(s))) = this.clip_scene(cx) {
                    this.frame_all(&s, true, cx);
                } else {
                    this.canvas.fit = true;
                }
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &StudioAlignCamera, _, cx| {
                if let Err(e) = this.align_camera_to_view(cx) {
                    flash(e, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &StudioFly, w, cx| {
                if this.is_3d(cx) {
                    this.with_viewport(w, cx, |v, _, cx| v.start_fly(false, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &StudioZoomIn, _, cx| this.zoom_step(1.25, cx)))
            .on_action(cx.listener(|this, _: &StudioZoomOut, _, cx| this.zoom_step(0.8, cx)))
            .on_action(cx.listener(|this, _: &StudioZoom100, _, cx| {
                if !this.is_3d(cx) {
                    this.zoom_canvas_to(1.0, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &StudioFit, _, cx| {
                this.canvas.fit = true;
                this.changed(cx);
            }))
            .on_action(cx.listener(|this, _: &StudioExtrude, w, cx| this.with_viewport(w, cx, |v, w, cx| v.start_modal(viewport::ModalKind::Extrude, w, cx))))
            .on_action(cx.listener(|this, _: &StudioBevel, w, cx| this.with_viewport(w, cx, |v, w, cx| v.start_modal(viewport::ModalKind::Bevel, w, cx))))
            .on_action(cx.listener(|this, _: &StudioLoopCut, w, cx| this.with_viewport(w, cx, |v, _, cx| v.arm_loop_cut(cx))))
            .on_action(cx.listener(|this, _: &StudioMerge, _, cx| this.mesh_op("merge", json!({ "at": "center" }), cx)))
            .on_action(cx.listener(|this, _: &StudioFlip, _, cx| this.mesh_op("flip", json!({}), cx)))
            .on_action(cx.listener(|this, _: &StudioRecalc, _, cx| this.mesh_op("recalcNormals", json!({}), cx)))
            .on_action(cx.listener(|this, _: &StudioToolSelect, _, cx| this.set_tool(Tool::Select, cx)))
            .on_action(cx.listener(|this, _: &StudioToolCycle, _, cx| this.cycle_tool(cx)))
            .on_action(cx.listener(|this, _: &StudioPen, _, cx| this.set_tool(Tool::Pen, cx)))
            .on_action(cx.listener(|this, _: &StudioShape, _, cx| {
                let next = match this.tool {
                    Tool::Rect => Tool::Ellipse,
                    Tool::Ellipse => Tool::Star,
                    Tool::Star => Tool::Polygon,
                    _ => Tool::Rect,
                };
                this.set_tool(next, cx)
            }))
            .on_action(cx.listener(|this, _: &StudioText, _, cx| this.set_tool(Tool::Text, cx)))
            .on_action(cx.listener(|this, _: &StudioAnchor, _, cx| this.set_tool(Tool::Anchor, cx)))
            .on_action(cx.listener(|this, _: &StudioGraph, _, cx| {
                this.show_graph = !this.show_graph;
                this.changed(cx);
            }))
    }

    /// 1 / 2 / 3: select modes in edit mode; views otherwise (1 front, 3 right, 7 top, 0 camera).
    fn number_key(&mut self, n: u8, cx: &mut Context<Self>) {
        if self.mode == Mode::Edit && matches!(n, 1..=3) {
            self.select_mode = [SelectMode::Vertex, SelectMode::Edge, SelectMode::Face][n as usize - 1];
            self.changed(cx);
            return;
        }
        if !self.is_3d(cx) {
            return;
        }
        let view = match n {
            1 => "front",
            3 => "right",
            7 => "top",
            0 => {
                self.toggle_camera_view(cx);
                return;
            }
            _ => return,
        };
        self.look_from(view, cx);
    }

    /// 0 / the camera button: through the scene's camera (gliding into it), or back out where it is.
    pub fn toggle_camera_view(&mut self, cx: &mut Context<Self>) {
        if self.view_shown().1 {
            self.stop_view_anim();
            if let Some(v) = self.camera_view(cx) {
                self.view = v;
            }
            self.through_camera = false;
            self.changed(cx);
        } else {
            self.look_from("camera", cx);
        }
    }

    /// 5 / the projection button.
    pub fn toggle_ortho(&mut self, cx: &mut Context<Self>) {
        self.finish_view_anim(cx);
        if self.through_camera {
            if let Some(v) = self.camera_view(cx) {
                self.view = v;
            }
            self.through_camera = false;
        }
        // The same framing either way: the orthographic height is what the perspective shows at
        // the target.
        if !self.view.ortho {
            self.view.ortho_size = self.view.distance() * (self.view.fov.to_radians() / 2.0).tan() * 2.0;
        }
        self.view.ortho = !self.view.ortho;
        self.changed(cx);
    }

    /// + / −: the 3D view closer or further; the 2D canvas zoomed around its middle.
    fn zoom_step(&mut self, k: f64, cx: &mut Context<Self>) {
        if self.is_3d(cx) {
            self.nav_begin();
            self.navigate(Nav::Zoom(1.0 / k, None), cx);
            self.nav_end(cx);
        } else {
            let z = self.shown_zoom(cx);
            self.zoom_canvas_to(z * k, cx);
        }
    }

    /// The 2D canvas's zoom on screen (fitted, the one the viewport last drew).
    pub fn shown_zoom(&self, cx: &App) -> f64 {
        if self.canvas.fit { self.viewport.read(cx).fit_zoom.get() } else { self.canvas.zoom }
    }

    /// The 2D canvas at a zoom (1 = 100%), keeping what is in the middle of the view there.
    pub fn zoom_canvas_to(&mut self, zoom: f64, cx: &mut Context<Self>) {
        let now = self.shown_zoom(cx).max(1e-6);
        let pan = if self.canvas.fit { [0.0, 0.0] } else { self.canvas.pan };
        let zoom = zoom.clamp(0.02, 32.0);
        let k = zoom / now;
        self.canvas = Canvas2d { zoom, pan: [pan[0] * k, pan[1] * k], fit: false };
        self.changed(cx);
    }

    pub fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        let three = self.is_3d(cx);
        let ok = if three { matches!(tool, Tool::Select | Tool::Move | Tool::Rotate | Tool::Scale) } else { !matches!(tool, Tool::Move | Tool::Rotate | Tool::Scale) };
        if !ok {
            return;
        }
        self.tool = tool;
        self.viewport_soon(cx, |v, cx| v.tool_changed(cx));
        self.changed(cx);
    }

    fn cycle_tool(&mut self, cx: &mut Context<Self>) {
        let list: &[Tool] = if self.is_3d(cx) { &[Tool::Select, Tool::Move, Tool::Rotate, Tool::Scale] } else { &[Tool::Select, Tool::Anchor, Tool::Pen, Tool::Rect, Tool::Ellipse, Tool::Text] };
        let i = list.iter().position(|t| *t == self.tool).map_or(0, |i| (i + 1) % list.len());
        self.set_tool(list[i], cx);
    }

    // ---- edits --------------------------------------------------------------------------------

    /// Delete / X: keyframes in the timeline, the mesh selection in edit mode (a menu), or the
    /// selected things.
    pub fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.area == Area::Timeline && !self.keys.is_empty() {
            self.delete_keys(cx);
            return;
        }
        if self.mode == Mode::Edit {
            let pos = window.mouse_position();
            self.popover = Some(Popover::delete_mesh(pos));
            cx.notify();
            return;
        }
        let Some((_, scene)) = self.clip_scene(cx) else { return };
        let clip = self.clip;
        let mut commands = vec![];
        for key in &self.selection {
            match model::item(&scene, key) {
                Some(model::Item::Material(m)) => commands.push(json!({ "command": "motion.removeMaterial", "params": { "clipId": clip, "materialId": m } })),
                Some(model::Item::Composition(c)) => commands.push(json!({ "command": "motion.removeComposition", "params": { "clipId": clip, "compositionId": c } })),
                Some(model::Item::Scene) | None => {}
                Some(model::Item::Camera) if key == "camera" => {}
                Some(_) => {
                    // A child goes with its parent: skip it when the parent is deleted too.
                    commands.push(json!({ "command": "motion.removeLayer", "params": { "clipId": clip, "id": key } }));
                }
            }
        }
        if commands.is_empty() {
            return;
        }
        self.run("project.batch", json!({ "commands": commands, "label": "Delete", "atomic": false }), cx);
        self.selection.clear();
        self.changed(cx);
    }

    pub fn delete_keys(&mut self, cx: &mut Context<Self>) {
        let Some((c, _)) = self.clip_scene(cx) else { return };
        let clip = self.clip;
        let commands: Vec<Value> = self
            .keys
            .iter()
            .map(|k| json!({ "command": "motion.removeKeyframe", "params": { "clipId": clip, "id": k.id, "property": k.property, "time": model::timeline_time(&c, k.time) } }))
            .collect();
        self.run("project.batch", json!({ "commands": commands, "label": "Delete keyframes" }), cx);
        self.keys.clear();
        self.changed(cx);
    }

    pub fn duplicate(&mut self, cx: &mut Context<Self>) {
        let Some((_, scene)) = self.clip_scene(cx) else { return };
        let ids: Vec<String> = self.selection.iter().filter(|k| model::is_thing(&scene, k) && *k != "camera").cloned().collect();
        if ids.is_empty() {
            return;
        }
        let clip = self.clip;
        let n = ids.len();
        let made = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        for id in ids {
            let made = made.clone();
            self.run_then("motion.duplicateLayer", json!({ "clipId": clip, "id": id }), cx, move |this, v, cx| {
                if let Some(id) = v["id"].as_str() {
                    made.borrow_mut().push(id.to_string());
                }
                if made.borrow().len() == n {
                    let sel = made.borrow().clone();
                    this.set_selection(sel, cx);
                    // Then move them, like Blender's Shift+D.
                    if this.is_3d(cx) {
                        this.viewport_soon(cx, |v, cx| v.start_modal_now(viewport::ModalKind::Grab, cx));
                    }
                }
            });
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Edit {
            self.viewport_soon(cx, |v, cx| v.toggle_all_mesh(cx));
            return;
        }
        if self.area == Area::Timeline {
            let tl = self.timeline.clone();
            cx.defer(move |cx| tl.update(cx, |t, cx| t.select_all_keys(cx)));
            return;
        }
        let Some((_, scene)) = self.clip_scene(cx) else { return };
        let all: Vec<String> = match &scene {
            Scene::Flat(f) => {
                let mut v = vec![];
                kimchi_core::motion::walk_layers(model::view_layers(f, self.composition.as_deref()), &mut |l| v.push(l.id.clone()));
                v
            }
            Scene::Space(_) => model::thing_ids(&scene),
        };
        let keys = if self.selection.len() >= all.len() { vec![] } else { all };
        self.set_selection(keys, cx);
    }

    fn hide_selected(&mut self, hide: bool, cx: &mut Context<Self>) {
        let Some((_, scene)) = self.clip_scene(cx) else { return };
        let clip = self.clip;
        let ids: Vec<String> = if hide {
            self.selection.iter().filter(|k| model::is_thing(&scene, k) && !matches!(model::item(&scene, k), Some(model::Item::Camera))).cloned().collect()
        } else {
            model::rows(&scene, &HashSet::new()).into_iter().filter(|r| r.hidden == Some(true)).map(|r| r.key).collect()
        };
        if ids.is_empty() {
            return;
        }
        let commands: Vec<Value> = ids.iter().map(|id| json!({ "command": "motion.updateLayer", "params": { "clipId": clip, "id": id, "props": { "hidden": hide } } })).collect();
        self.run("project.batch", json!({ "commands": commands, "label": if hide { "Hide" } else { "Show all" } }), cx);
    }

    /// I (object mode): a keyframe of where each selected thing is now, at the playhead.
    pub fn keyframe_selection(&mut self, cx: &mut Context<Self>) {
        let Some((_, scene)) = self.clip_scene(cx) else { return };
        let clip = self.clip;
        let time = self.playhead(cx);
        let mut commands = vec![];
        for id in self.selection.iter().filter(|k| model::is_thing(&scene, k)) {
            let props: &[&str] = match (model::item(&scene, id), scene.is_3d()) {
                (Some(model::Item::Camera), _) => &["position", "target"],
                (Some(model::Item::Light), _) => &["position", "intensity"],
                (_, true) => &["position", "rotation", "scale"],
                (_, false) => &["x", "y", "rotation", "scale", "opacity"],
            };
            for p in props {
                commands.push(json!({ "command": "motion.addKeyframe", "params": { "clipId": clip, "id": id, "property": p, "time": time } }));
            }
        }
        if commands.is_empty() {
            flash("Select something to keyframe.", cx);
            return;
        }
        self.run("project.batch", json!({ "commands": commands, "label": "Insert keyframes" }), cx);
    }

    /// One `motion.editMesh` call on the active mesh with the edit selection; the answer's
    /// selection becomes the new one.
    pub fn mesh_op(&mut self, op: &str, params: Value, cx: &mut Context<Self>) {
        if self.mode != Mode::Edit {
            return;
        }
        let Some(id) = self.active().map(str::to_string) else { return };
        let clip = self.clip;
        let mut p = json!({ "clipId": clip, "id": id, "op": op, "params": params });
        let sel = self.edit_sel.params();
        p["vertices"] = sel["vertices"].clone();
        p["faces"] = sel["faces"].clone();
        self.run_then("motion.editMesh", p, cx, |this, v, cx| {
            this.edit_sel = selection_from(&v);
            this.changed(cx);
        });
    }

    pub fn open_add_menu(&mut self, at: Option<gpui::Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Edit {
            return;
        }
        let pos = at.unwrap_or_else(|| window.mouse_position());
        let three = self.is_3d(cx);
        self.popover = Some(Popover::add(pos, three, window, cx));
        cx.notify();
    }
}

/// Runs a command as the window (errors become toasts).
pub fn run(name: &str, params: Value, cx: &mut App) {
    cx.store().update(cx, |s, cx| s.run(name, params, cx));
}

/// A command's answer, errors included.
pub fn call(name: &str, params: Value, cx: &mut App) -> Task<CmdResult> {
    cx.store().update(cx, |s, cx| s.call(name, params, cx))
}

/// A passing status line.
pub fn flash(text: impl Into<gpui::SharedString>, cx: &mut App) {
    let text = text.into();
    cx.store().update(cx, |s, cx| s.flash(text, cx));
}

/// Moves the playhead to the open clip's scene time `st`.
pub fn seek(studio: &Entity<Studio>, st: f64, cx: &mut App) {
    let Some((c, _)) = studio.read(cx).clip_scene(cx) else { return };
    let t = model::timeline_time(&c, st);
    let pb = cx.store().read(cx).playback.clone();
    pb.update(cx, |p, cx| p.seek(t, cx));
}

/// The selection `motion.editMesh` answers with (`selection: {vertices, faces}` or at the top).
pub fn selection_from(v: &Value) -> EditSel {
    // `{result: {selection: …}}` (motion.editMesh), `{selection: …}`, or the lists themselves.
    let src = v.pointer("/result/selection").or_else(|| v.get("selection")).unwrap_or(v);
    let ints = |k: &str| -> Vec<u32> { src.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(|x| x.as_u64().map(|n| n as u32)).collect() };
    let edges = src
        .get("edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let a = e.as_array()?;
            Some((a.first()?.as_u64()? as u32, a.get(1)?.as_u64()? as u32))
        })
        .collect();
    EditSel { vertices: ints("vertices"), edges, faces: ints("faces") }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        if self.clip.is_none() {
            return div().into_any_element();
        }
        let width = self.available_width(window, cx);
        let height = f32::from(window.viewport_size().height);
        let dock_left = width >= 1200.;
        let dock_right = width >= 960.;
        let right_w = self.right_w.min((width - 550.).max(256.));
        let left_budget = width - if dock_right { right_w + 10. } else { 0. } - 320.;
        let left_w = self.left_w.min(left_budget.max(180.));
        let bottom_h = self.bottom_h.min((height * 0.28).max(100.));
        let busy = self.viewport.read(cx).busy();
        let toolbar = toolbar::render(self, window, cx);
        let me = cx.entity();
        let popover = self.popover.as_ref().map(|p| p.render(self, me, window, cx));
        let root = div()
            .id("studio")
            .key_context(if busy { "Studio StudioBusy" } else { "Studio" })
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg_raised)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                // The Studio keeps the keyboard (its keys win over the editor's).
                if !window.default_prevented() {
                    window.focus(&this.focus, cx);
                    window.prevent_default();
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex().relative().overflow_hidden()
                    .when(dock_left, |d| d.child(div().w(px(left_w)).flex_none().h_full().child(self.outliner.clone())).child(self.splitter(0, cx)))
                    .child(div().flex_1().min_w_0().h_full().bg(t.bg_sunken).child(self.viewport.clone()))
                    .when(dock_right, |d| d.child(self.splitter(1, cx)).child(div().w(px(right_w)).flex_none().h_full().child(self.properties.clone())))
                    .when(self.drawer == Some(false) && !dock_left, |d| d.child(
                        div().absolute().left_0().top_0().bottom_0().w(px(left_w)).bg(t.bg_raised).border_r_1().border_color(t.line).occlude().child(self.outliner.clone())))
                    .when(self.drawer == Some(true) && !dock_right, |d| d.child(
                        div().absolute().right_0().top_0().bottom_0().w(px(right_w)).bg(t.bg_raised).border_l_1().border_color(t.line).occlude().child(self.properties.clone()))),
            )
            .child(self.splitter(2, cx))
            .child(div().h(px(bottom_h)).flex_none().w_full().child(self.timeline.clone()))
            .when(self.resizing.is_some(), |d| d.child(drag::track(cx.entity(), Self::resize_move, Self::resize_end)))
            .children(popover);
        let root: gpui::Div = div().size_full().child(root);
        self.register_actions(root, cx).into_any_element()
    }
}
