//! The Studio's viewport: the scene drawn by the compositor (on a background thread, at the
//! view's size, half resolution while you interact; the last picture stays until the next one
//! arrives), and the tools over it, drawn by GPUI: in 3D the editor camera (orbit, pan, zoom,
//! axis views), picking, the transform gizmo and Blender's G / R / S, box select, edit mode's
//! vertex / edge / face picking and modelling keys; in 2D the canvas (zoom, pan, fit), click and
//! box selection, the bounding box with scale and rotate handles, the anchor point tool, the
//! pen (paths and masks), shape and text tools.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Entity, FontWeight, Hsla, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, PathBuilder, Pixels, Point,
    Render, RenderImage, ScrollWheelEvent, Subscription, Task, Window, canvas, div, img, point, prelude::*, px,
};
use kimchi_core::motion::{Scene2d, Scene3d};
use kimchi_core::{Id, Project, Scene};
use kimchi_media::render::space::viewport::{ViewCamera, ViewOptions};
use serde_json::{Value, json};

use super::gizmo::{self, Handle, Kind, Session, View3};
use super::math::{self, V3};
use super::{Mode, Nav, SelectMode, Studio, Tool, model};
use crate::store::{MenuEntry, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{GlassExt, drag, icon};

/// Largest picture asked for (pixels, longest side).
const MAX_RENDER: f64 = 1600.0;
/// The mesh engine's name for moving a selection.
const MESH_MOVE: &str = "translate";

/// A modal operation started from the keyboard (or a menu): it follows the mouse until a click
/// or Enter confirms it, and Esc or a right click cancels it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKind {
    Grab,
    Rotate,
    Scale,
    /// Edit mode: pull the extruded faces out along their normal.
    Extrude,
    Inset,
    Bevel,
}

/// A mesh operation's amount being chosen with the mouse.
#[derive(Clone, Debug)]
struct MeshModal {
    kind: ModalKind,
    mouse0: [f64; 2],
    normal: V3,
    /// World units per pixel at the pivot.
    unit: f64,
    typed: String,
    amount: f64,
    /// Extrude: how far the new faces have been moved so far, and the drag's undo key.
    applied: f64,
    /// G in edit mode: only moving the selection along its normal (no extrude first).
    moving: bool,
    busy: bool,
    key: String,
}

/// What the pointer does while a button is held.
enum Drag {
    Orbit { last: Point<Pixels> },
    Pan { last: Point<Pixels> },
    /// Up closer, down further (Ctrl+middle-drag, the zoom button).
    Zoom { last: Point<Pixels> },
    /// On the axis ball: a click on an axis looks along it, a drag orbits.
    Ball { start: Point<Pixels>, last: Point<Pixels>, moved: bool, axis: Option<&'static str> },
    /// The right button: a click opens the menu, a drag looks around (and WASD flies) until it
    /// is let go.
    Right { start: Point<Pixels>, last: Point<Pixels>, moved: bool },
    /// A press that becomes a box selection if it moves, a pick if it doesn't.
    Press { start: Point<Pixels>, now: Point<Pixels>, additive: bool, moved: bool, boxing: bool },
    Gizmo,
    /// 2D: moving, scaling, turning layers or their anchor.
    Layer(LayerDrag),
    /// 2D pen: the point just placed, its handle being pulled.
    Pen { at: [f64; 2], handle: [f64; 2] },
    /// 2D shape tools: from the press to the pointer.
    Shape { from: [f64; 2], to: [f64; 2], square: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LayerOp {
    Move,
    /// A handle: which corner/edge (−1, 0, 1 in x and y of the box).
    Scale(i8, i8),
    Rotate,
    Anchor,
}

#[derive(Clone)]
struct LayerStart {
    id: String,
    x: f64,
    y: f64,
    rotation: f64,
    scale: f64,
    scale_x: f64,
    scale_y: f64,
    anchor: [f64; 2],
    /// The layer's own pixels → canvas, and the parent's space → canvas.
    world: math::Affine,
    parent: math::Affine,
}

struct LayerDrag {
    op: LayerOp,
    mouse0: [f64; 2],
    starts: Vec<LayerStart>,
    key: String,
    /// The layer's own values reached (id → props), for the box to follow before the picture.
    reached: Vec<(String, serde_json::Map<String, Value>)>,
}

/// One point of a path being drawn: where, and its curve handle (relative), in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PenPoint {
    p: [f64; 2],
    handle: [f64; 2],
}

#[derive(Clone, PartialEq)]
struct Request {
    project: usize,
    clip: Id,
    t: f64,
    w: u32,
    h: u32,
    view: Option<ViewCamera>,
    opts: ViewOptions,
    comp: Option<String>,
}

/// Fly mode: keys held move the view, the mouse turns it, the wheel sets the speed.
struct Fly {
    /// Movement keys down (w a s d q e and the arrows), and Shift (faster).
    held: Vec<String>,
    fast: bool,
    /// World units a second.
    speed: f64,
    /// Where it started (Esc puts it back), and whether that was through the camera.
    start: ViewCamera,
    through: bool,
    /// Started by holding the right button: letting go ends it, keeping the view.
    hold: bool,
    /// The pointer's last position (looking follows its moves).
    last: Option<[f64; 2]>,
    tick: std::time::Instant,
    _task: Task<()>,
}

/// Something armed by a key, waiting for a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Armed {
    BoxSelect,
    LoopCut,
}

pub struct Viewport {
    studio: Entity<Studio>,
    store: Entity<Store>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    image: Option<Arc<RenderImage>>,
    garbage: Vec<Arc<RenderImage>>,
    shown: Option<Request>,
    rendering: Option<Task<()>>,
    stale: bool,
    error: Option<String>,
    /// Interacting: render at half size until things settle.
    fast: bool,
    _settle: Option<Task<()>>,
    drag: Option<Drag>,
    /// A transform in progress (gizmo drag or G / R / S).
    pub session: Option<Session>,
    modal_key: Option<String>,
    /// A gizmo drag's undo key.
    gizmo_key: Option<String>,
    mesh_modal: Option<MeshModal>,
    _intercept: Option<Subscription>,
    armed: Option<Armed>,
    hover: Option<Handle>,
    hover_edge: Option<(u32, u32)>,
    pen: Vec<PenPoint>,
    mouse: [f64; 2],
    /// A path-traced picture refining: its stop flag and the task showing its pictures.
    refiner: Option<(Arc<std::sync::atomic::AtomicBool>, Task<()>)>,
    /// Samples per pixel so far, of how many (the Rendered view of the path tracer).
    samples: Option<(u32, u32)>,
    /// The display's pixels per point (pictures are rendered for it).
    scale: f32,
    fly: Option<Fly>,
    /// The fitted 2D canvas's zoom, as last drawn (the Studio reads it).
    pub fit_zoom: Cell<f64>,
    /// The selected camera's path over the clip, kept while the scene is the same.
    cam_path: Option<(usize, String, Vec<V3>, Vec<V3>)>,
    _subs: Vec<Subscription>,
}

impl Viewport {
    pub fn new(studio: Entity<Studio>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![
            cx.observe(&studio, |this, _, cx| this.refresh(cx)),
            cx.observe(&store, |this, _, cx| this.refresh(cx)),
            cx.observe(&playback, |this, _, cx| this.refresh(cx)),
        ];
        Self {
            studio,
            store,
            bounds: Rc::new(Cell::new(Bounds::default())),
            image: None,
            garbage: vec![],
            shown: None,
            rendering: None,
            stale: false,
            error: None,
            fast: false,
            _settle: None,
            drag: None,
            session: None,
            modal_key: None,
            gizmo_key: None,
            mesh_modal: None,
            _intercept: None,
            armed: None,
            hover: None,
            hover_edge: None,
            pen: vec![],
            mouse: [0.0; 2],
            refiner: None,
            samples: None,
            scale: 2.0,
            fly: None,
            fit_zoom: Cell::new(0.5),
            cam_path: None,
            _subs: subs,
        }
    }

    /// A tool or modal operation has the keyboard (the Studio's single keys are off).
    pub fn busy(&self) -> bool {
        self.modal_key.is_some() || self.mesh_modal.is_some() || !self.pen.is_empty() || self.fly.is_some()
    }

    /// Where the viewport is in the window (tests aim the pointer at its parts).
    #[cfg(test)]
    pub fn bounds_for_test(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }

    /// In fly mode.
    #[cfg(test)]
    pub fn flying(&self) -> bool {
        self.fly.is_some()
    }

    /// The 2D canvas's zoom on screen (the fitted one too).
    pub fn canvas_zoom(&self, cx: &App) -> Option<f64> {
        let (p, _, scene, _) = self.scene(cx)?;
        match &scene {
            Scene::Flat(s) => Some(self.view2(s, &p, cx).zoom),
            Scene::Space(_) => None,
        }
    }

    // ---- what is shown ------------------------------------------------------------------------

    fn sizes(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }

    /// The 3D view on screen: the editor camera over the whole viewport, or the scene's camera
    /// in a frame of the project's shape.
    fn view3(&self, s: &Scene3d, t: f64, project: &Project, cx: &App) -> View3 {
        let st = self.studio.read(cx);
        let b = self.sizes();
        let (bx, by, bw, bh) = (f32::from(b.origin.x) as f64, f32::from(b.origin.y) as f64, f32::from(b.size.width) as f64, f32::from(b.size.height) as f64);
        if st.through_camera {
            let c = s.camera_at(t);
            let cam = ViewCamera { position: c.position.0, target: c.target.0, fov: c.fov, ortho: c.orthographic(), ortho_size: c.ortho_size };
            let aspect = project.settings.width as f64 / project.settings.height.max(1) as f64;
            let (w, h) = if bw / bh.max(1.0) > aspect { (bh * aspect, bh) } else { (bw, bw / aspect) };
            let (w, h) = (w - 24.0, h - 24.0 / aspect);
            View3 { cam, x: bx + (bw - w) / 2.0, y: by + (bh - h) / 2.0, w: w.max(16.0), h: h.max(16.0) }
        } else {
            View3 { cam: st.view, x: bx, y: by, w: bw.max(16.0), h: bh.max(16.0) }
        }
    }

    /// The 2D canvas on screen: where its centre is and how many screen pixels a project pixel is.
    fn view2(&self, s: &Scene2d, project: &Project, cx: &App) -> View2 {
        let st = self.studio.read(cx);
        let b = self.sizes();
        let (bx, by, bw, bh) = (f32::from(b.origin.x) as f64, f32::from(b.origin.y) as f64, f32::from(b.size.width) as f64, f32::from(b.size.height) as f64);
        let pr = (project.settings.width as f64, project.settings.height as f64);
        let canvas = model::canvas_size(s, st.composition.as_deref(), pr);
        let zoom = if st.canvas.fit { ((bw - 48.0) / canvas.0).min((bh - 48.0) / canvas.1).max(0.01) } else { st.canvas.zoom };
        if st.canvas.fit {
            self.fit_zoom.set(zoom);
        }
        let pan = if st.canvas.fit { [0.0, 0.0] } else { st.canvas.pan };
        View2 { cx: bx + bw / 2.0 + pan[0], cy: by + bh / 2.0 + pan[1], zoom, canvas, project: pr }
    }

    /// Asks for the picture the Studio needs, unless it is shown or on its way.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(req) = self.request(cx) else { return };
        if self.shown.as_ref() == Some(&req) {
            return;
        }
        // Something changed: a path-traced picture still refining stops.
        if let Some((cancel, _)) = self.refiner.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if self.rendering.is_some() {
            self.stale = true;
            return;
        }
        self.render_picture(req, cx);
    }

    fn request(&self, cx: &App) -> Option<Request> {
        let st = self.studio.read(cx);
        let clip = st.clip?;
        let project = self.store.read(cx).project.clone()?;
        let (c, scene) = model::motion_clip(&project, clip)?;
        let t = model::scene_time(c, st.playhead(cx));
        let b = self.sizes();
        if f32::from(b.size.width) < 8.0 {
            return None;
        }
        let scale = self.scale as f64;
        let k = if self.fast { 0.5 } else { 1.0 };
        match scene {
            Scene::Space(s) => {
                let v = self.view3(s, t, &project, cx);
                let (mut w, mut h) = (v.w * scale * k, v.h * scale * k);
                let cap = (MAX_RENDER * k / w.max(h)).min(1.0);
                w *= cap;
                h *= cap;
                let edit = (st.mode == Mode::Edit).then(|| st.active().map(str::to_string)).flatten();
                let mesh_faces = edit.as_ref().and_then(|id| model::edit_mesh(s, &model::worlds(s, t), id)).map(|(_, f)| f).unwrap_or_default();
                let (edit_vertices, edit_faces) = match st.select_mode {
                    SelectMode::Face => (st.edit_sel.all_vertices(&mesh_faces), st.edit_sel.faces.clone()),
                    _ => (st.edit_sel.all_vertices(&mesh_faces), vec![]),
                };
                let opts = ViewOptions {
                    through_camera: st.through_camera,
                    shading: st.shading,
                    grid: st.grid && !st.through_camera,
                    selected: st.selection.clone(),
                    edit,
                    edit_vertices,
                    edit_faces,
                    helpers: st.helpers && !st.through_camera,
                };
                Some(Request { project: Arc::as_ptr(&project) as usize, clip, t, w: (w as u32).max(16), h: (h as u32).max(16), view: (!st.through_camera).then_some(st.view), opts, comp: None })
            }
            Scene::Flat(s) => {
                let v = self.view2(s, &project, cx);
                // The whole project-sized frame around the canvas centre, at the zoom shown.
                let (pw, ph) = v.project;
                let mut w = pw * v.zoom * scale * k;
                let mut h = ph * v.zoom * scale * k;
                let cap = ((2048.0 * k) / w.max(h)).min(1.0);
                w *= cap;
                h *= cap;
                Some(Request { project: Arc::as_ptr(&project) as usize, clip, t, w: (w as u32).max(16), h: (h as u32).max(16), view: None, opts: ViewOptions::default(), comp: st.composition.clone() })
            }
        }
    }

    fn render_picture(&mut self, req: Request, cx: &mut Context<Self>) {
        let Some(project) = self.store.read(cx).project.clone() else { return };
        let path_traced = matches!(model::motion_clip(&project, req.clip), Some((_, Scene::Space(s))) if s.render.path_traced());
        if req.opts.shading == kimchi_media::render::space::viewport::Shading::Rendered && path_traced && !self.fast {
            self.refine(req, project, cx);
            return;
        }
        let session = self.store.read(cx).session.clone();
        let tools = session.tools_if_found().unwrap_or_else(|| kimchi_media::Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into() });
        let fps = project.settings.fps;
        let r = req.clone();
        let task = cx.background_spawn(async move { draw(&tools, &project, &r, fps) });
        self.stale = false;
        self.rendering = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.rendering = None;
                match result {
                    Ok(img) => {
                        if let Some(old) = this.image.replace(img) {
                            this.garbage.push(old);
                        }
                        this.error = None;
                    }
                    Err(e) => this.error = Some(e),
                }
                this.shown = if this.stale { None } else { Some(req) };
                this.refresh(cx);
            })
            .ok();
        }));
    }

    /// The "Rendered" view of a path-traced scene: a first picture with a few samples, better
    /// ones as more come in, until the scene's samples are all in or the view changes.
    fn refine(&mut self, req: Request, project: Arc<Project>, cx: &mut Context<Self>) {
        let session = self.store.read(cx).session.clone();
        let tools = session.tools_if_found().unwrap_or_else(|| kimchi_media::Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into() });
        let fps = project.settings.fps;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Result<(Arc<RenderImage>, u32, u32), String>>();
        let (r, stop) = (req.clone(), cancel.clone());
        cx.background_spawn(async move {
            // Half the view's pixels: the path tracer is slow, and the picture refines anyway.
            let mut renderer = kimchi_media::render::Renderer::new(&tools, &project, (r.w / 2).max(16), (r.h / 2).max(16), fps);
            let mut p = match renderer.refining_view(r.clip, r.t, r.view.as_ref()) {
                Ok(Some(p)) => p,
                Ok(None) => return,
                Err(e) => {
                    let _ = tx.unbounded_send(Err(e.to_string()));
                    return;
                }
            };
            let mut step = 1;
            while !p.done() && !stop.load(std::sync::atomic::Ordering::Relaxed) {
                p.add(step.min(p.target() - p.samples()));
                let pic = to_image(p.picture());
                if tx.unbounded_send(pic.map(|i| (i, p.samples(), p.target()))).is_err() {
                    break;
                }
                step = (step * 2).min(16);
            }
        })
        .detach();
        self.shown = Some(req);
        let task = cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(r) = rx.next().await {
                let ok = this
                    .update(cx, |this, cx| {
                        match r {
                            Ok((img, n, of)) => {
                                if let Some(old) = this.image.replace(img) {
                                    this.garbage.push(old);
                                }
                                this.samples = Some((n, of));
                                this.error = None;
                            }
                            Err(e) => this.error = Some(e),
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        });
        self.refiner = Some((cancel, task));
    }

    /// While the pointer works the view, pictures come at half size; full size once it rests
    /// (and then `ui.state` hears where the view is).
    pub fn interacting(&mut self, cx: &mut Context<Self>) {
        self.fast = true;
        self._settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            this.update(cx, |this, cx| {
                this.fast = false;
                this.refresh(cx);
                let studio = this.studio.clone();
                cx.defer(move |cx| studio.update(cx, |s, cx| s.changed(cx)));
            })
            .ok();
        }));
    }

    // ---- reading the scene ---------------------------------------------------------------------

    fn scene(&self, cx: &App) -> Option<(Arc<Project>, kimchi_core::Clip, Scene, f64)> {
        let st = self.studio.read(cx);
        let p = self.store.read(cx).project.clone()?;
        let (c, s) = model::motion_clip(&p, st.clip?)?;
        let t = model::scene_time(c, st.playhead(cx));
        let (c, s) = (c.clone(), s.clone());
        Some((p, c, s, t))
    }

    /// The gizmo's frame for the selection (following a transform in progress).
    fn gizmo_frame(&self, s: &Scene3d, scene: &Scene, t: f64, cx: &App) -> Option<gizmo::Frame> {
        if let Some(sess) = &self.session {
            return Some(gizmo::Frame { pivot: sess.live_pivot(), axes: sess.frame.axes });
        }
        let st = self.studio.read(cx);
        if st.mode != Mode::Object || !matches!(st.tool, Tool::Move | Tool::Rotate | Tool::Scale) {
            return None;
        }
        let w = model::worlds(s, t);
        let starts = gizmo::starts(s, scene, &st.selection, t, &w);
        gizmo::frame(&starts, st.local)
    }

    fn gizmo_kind(&self, cx: &App) -> Kind {
        match (&self.session, self.studio.read(cx).tool) {
            (Some(s), _) => s.kind,
            (None, Tool::Rotate) => Kind::Rotate,
            (None, Tool::Scale) => Kind::Scale,
            _ => Kind::Grab,
        }
    }

    // ---- 3D transforms ---------------------------------------------------------------------------

    /// G: move (3D), the pen (2D).
    pub fn key_grab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.studio.read(cx).is_3d(cx) {
            self.start_modal(ModalKind::Grab, window, cx);
        } else {
            self.studio.update(cx, |s, cx| s.set_tool(Tool::Pen, cx));
        }
    }

    /// Starts G / R / S / E / I / Ctrl+B from the keyboard, at the pointer.
    pub fn start_modal(&mut self, kind: ModalKind, window: &mut Window, cx: &mut Context<Self>) {
        let m = window.mouse_position();
        self.mouse = [f32::from(m.x) as f64, f32::from(m.y) as f64];
        self.start_modal_now(kind, cx);
    }

    pub fn start_modal_now(&mut self, kind: ModalKind, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let Some((p, _, scene, t)) = self.scene(cx) else { return };
        let mode = self.studio.read(cx).mode;
        match (&scene, kind) {
            (Scene::Space(s), ModalKind::Grab | ModalKind::Rotate | ModalKind::Scale) if mode == Mode::Object => {
                let st = self.studio.read(cx);
                let w = model::worlds(s, t);
                let starts = gizmo::starts(s, &scene, &st.selection, t, &w);
                let Some(frame) = gizmo::frame(&starts, st.local) else { return };
                let view = self.view3(s, t, &p, cx);
                let gk = match kind {
                    ModalKind::Rotate => Kind::Rotate,
                    ModalKind::Scale => Kind::Scale,
                    _ => Kind::Grab,
                };
                let mut sess = Session::new(gk, Handle::Free, frame, starts, self.mouse, &view);
                sess.snap = st.snapping;
                self.session = Some(sess);
                let key = self.studio.update(cx, |s, _| s.drag_key());
                self.modal_key = Some(key);
                self.intercept(cx);
            }
            (Scene::Space(_), ModalKind::Grab) if mode == Mode::Edit => self.start_mesh_modal(ModalKind::Extrude, false, cx),
            (Scene::Space(_), ModalKind::Extrude | ModalKind::Inset | ModalKind::Bevel) if mode == Mode::Edit => self.start_mesh_modal(kind, true, cx),
            (Scene::Flat(_), ModalKind::Rotate | ModalKind::Scale) => {
                let op = if kind == ModalKind::Rotate { LayerOp::Rotate } else { LayerOp::Scale(1, 1) };
                if self.start_layer_drag(op, self.mouse, cx) {
                    let key = self.studio.update(cx, |s, _| s.drag_key());
                    self.modal_key = Some(key);
                    self.intercept(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// While a modal operation runs, every key goes to it (numbers, X/Y/Z, Enter, Esc…).
    fn intercept(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        self._intercept = Some(cx.intercept_keystrokes(move |e, window, cx| {
            let Some(this) = me.upgrade() else { return };
            if this.update(cx, |v, cx| v.modal_keystroke(&e.keystroke, window, cx)) {
                cx.stop_propagation();
            }
        }));
    }

    /// A key during a modal operation; true when it was taken.
    fn modal_keystroke(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.busy() {
            return false;
        }
        if self.fly.is_some() {
            return self.fly_keystroke(k, cx);
        }
        // The pen: Enter finishes the path, Esc drops it.
        if !self.pen.is_empty() && self.modal_key.is_none() && self.mesh_modal.is_none() {
            match k.key.as_str() {
                "enter" => self.finish_pen(false, cx),
                "escape" => {
                    self.pen.clear();
                    self._intercept = None;
                }
                "backspace" | "delete" => {
                    self.pen.pop();
                    if self.pen.is_empty() {
                        self._intercept = None;
                    }
                }
                _ => return false,
            }
            cx.notify();
            return true;
        }
        let key = k.key.as_str();
        match key {
            "escape" => self.cancel_modal(cx),
            "enter" | "space" => self.confirm_modal(cx),
            "x" | "y" | "z" => {
                let axis = match key {
                    "x" => 0,
                    "y" => 1,
                    _ => 2,
                };
                if let (true, Some((p, _, Scene::Space(s), t))) = (self.session.is_some(), self.scene(cx)) {
                    let view = self.view3(&s, t, &p, cx);
                    let local = self.studio.read(cx).local;
                    let sess = self.session.as_mut().expect("checked");
                    // Pressed again on the same axis: the object's own axis, like Blender.
                    let same = sess.handle == Handle::Axis(axis) || sess.handle == Handle::Plane(axis);
                    if same && !local && sess.starts.len() == 1 {
                        let [x, y, z] = math::axes(&sess.starts[0].world);
                        sess.frame.axes = [math::norm(x), math::norm(y), math::norm(z)];
                        sess.handle = if k.modifiers.shift { Handle::Plane(axis) } else { Handle::Axis(axis) };
                    } else {
                        if !same {
                            sess.frame.axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
                        }
                        sess.lock(axis, k.modifiers.shift, &view);
                    }
                    self.apply_session(cx);
                }
            }
            "backspace" => {
                if let Some(s) = self.session.as_mut() {
                    s.typed.pop();
                }
                if let Some(m) = self.mesh_modal.as_mut() {
                    m.typed.pop();
                }
                self.apply_any(cx);
            }
            _ => {
                let ch = k.key_char.clone().unwrap_or_else(|| key.to_string());
                if ch.len() == 1 && (ch.chars().all(|c| c.is_ascii_digit()) || ch == "." || ch == "-") {
                    if let Some(s) = self.session.as_mut() {
                        s.typed.push_str(&ch);
                    }
                    if let Some(m) = self.mesh_modal.as_mut() {
                        m.typed.push_str(&ch);
                    }
                    self.apply_any(cx);
                } else if !matches!(key, "shift" | "control" | "alt" | "platform" | "function") {
                    // Anything else is swallowed while the tool runs.
                }
            }
        }
        let _ = window;
        cx.notify();
        true
    }

    fn apply_any(&mut self, cx: &mut Context<Self>) {
        if self.session.is_some() {
            self.apply_session(cx);
        } else if matches!(self.drag, Some(Drag::Layer(_))) {
            let m = self.mouse;
            self.layer_drag_to(m, cx);
        } else if self.mesh_modal.is_some() {
            self.mesh_modal_to(self.mouse, cx);
        }
    }

    /// Recomputes the transform for the pointer and sends it.
    fn apply_session(&mut self, cx: &mut Context<Self>) {
        let Some((p, c, Scene::Space(s), t)) = self.scene(cx) else { return };
        let view = self.view3(&s, t, &p, cx);
        let m = self.mouse;
        let Some(sess) = self.session.as_mut() else { return };
        sess.update(&view, m);
        let key = self.modal_key.clone().unwrap_or_default();
        let time = self.studio.read(cx).playhead(cx);
        let cmds = sess.commands(c.id, time, &key, false);
        self.studio.update(cx, |s, cx| s.send(cmds, cx));
        self.interacting(cx);
    }

    fn confirm_modal(&mut self, cx: &mut Context<Self>) {
        if self.session.take().is_some() || matches!(self.drag, Some(Drag::Layer(_))) {
            self.drag = None;
        }
        if let Some(m) = self.mesh_modal.take() {
            self.finish_mesh_modal(m, cx);
        }
        self.modal_key = None;
        self._intercept = None;
        cx.notify();
    }

    /// Puts everything back as it was before the operation.
    pub fn cancel_modal(&mut self, cx: &mut Context<Self>) {
        if self.fly.is_some() {
            self.end_fly(false, cx);
        }
        let time = self.studio.read(cx).playhead(cx);
        if let (Some(sess), Some(key)) = (self.session.take(), self.modal_key.clone())
            && let Some(clip) = self.studio.read(cx).clip
        {
            let cmds = sess.commands(clip, time, &key, true);
            self.studio.update(cx, |s, cx| s.send(cmds, cx));
        }
        if let Some(Drag::Layer(d)) = self.drag.take() {
            self.send_layer(&d, true, cx);
        }
        if let Some(m) = self.mesh_modal.take()
            && m.kind == ModalKind::Extrude
            && m.applied.abs() > 1e-9
        {
            let off = math::scale(m.normal, -m.applied);
            self.studio.update(cx, |s, cx| s.mesh_op(MESH_MOVE, json!({ "offset": off }), cx));
        }
        self.modal_key = None;
        self._intercept = None;
        cx.notify();
    }

    /// Esc: stops what the viewport is doing; false when there was nothing.
    pub fn escape(&mut self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.fly.is_some() {
            self.end_fly(false, cx);
            return true;
        }
        if self.busy() {
            self.cancel_modal(cx);
            self.pen.clear();
            return true;
        }
        if self.armed.take().is_some() || self.drag.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// Esc would stop something here.
    pub fn escapable(&self) -> bool {
        self.busy() || self.armed.is_some() || self.drag.is_some()
    }

    pub fn tool_changed(&mut self, cx: &mut Context<Self>) {
        if !self.pen.is_empty() {
            self.finish_pen(false, cx);
        }
        cx.notify();
    }

    pub fn arm_box_select(&mut self, cx: &mut Context<Self>) {
        self.armed = Some(Armed::BoxSelect);
        cx.notify();
    }

    pub fn arm_loop_cut(&mut self, cx: &mut Context<Self>) {
        if self.studio.read(cx).mode == Mode::Edit {
            self.armed = Some(Armed::LoopCut);
            cx.notify();
        }
    }

    // ---- fly mode ----------------------------------------------------------------------------------

    /// Fly mode: W/S forward and back, A/D sideways, Q/E down and up (or the arrows), Shift
    /// faster, the mouse looks around, the wheel sets the speed. A click or Enter keeps the view,
    /// Esc or a right click puts it back. `hold`: started by the right button, it ends when the
    /// button is let go (keeping the view).
    pub fn start_fly(&mut self, hold: bool, cx: &mut Context<Self>) {
        if self.fly.is_some() || self.busy() {
            return;
        }
        let Some((_, _, Scene::Space(_), _)) = self.scene(cx) else { return };
        let (start, through) = self.studio.update(cx, |s, cx| {
            s.finish_view_anim(cx);
            s.nav_begin();
            let through = s.through_camera;
            let start = if through { s.camera_view(cx).unwrap_or(s.view) } else { s.view };
            (start, through)
        });
        // About a third of the way to what it looks at, each second.
        let speed = (start.distance() * 0.35).clamp(0.2, 200.0);
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let on = this.update(cx, |v, cx| v.fly_step(cx)).unwrap_or(false);
                if !on {
                    break;
                }
            }
        });
        self.fly = Some(Fly { held: vec![], fast: false, speed, start, through, hold, last: None, tick: std::time::Instant::now(), _task: task });
        self.studio.update(cx, |s, cx| {
            s.flying = true;
            s.changed(cx);
        });
        // Its keys (WASD…) aren't the Studio's while flying.
        self.intercept(cx);
        cx.notify();
    }

    /// Ends fly mode, keeping where it got to or putting the view back.
    pub fn end_fly(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(f) = self.fly.take() else { return };
        if self.modal_key.is_none() && self.mesh_modal.is_none() && self.pen.is_empty() {
            self._intercept = None;
        }
        self.studio.update(cx, |s, cx| {
            s.flying = false;
            if !keep {
                if s.through_camera && s.lock_camera {
                    s.put_camera_back(f.start, cx);
                } else {
                    s.view = f.start;
                    s.through_camera = f.through && !s.lock_camera;
                }
            }
            s.nav_end(cx);
        });
        cx.notify();
    }

    /// One frame of flying: moves by the keys held. False once fly mode is over.
    fn fly_step(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(f) = self.fly.as_mut() else { return false };
        // At least a frame's worth (a late timer still moves), at most a tenth of a second.
        let dt = f.tick.elapsed().as_secs_f64().clamp(1.0 / 120.0, 0.1);
        f.tick = std::time::Instant::now();
        if f.held.is_empty() {
            return true;
        }
        let has = |k: &[&str]| f.held.iter().any(|h| k.contains(&h.as_str()));
        let axis = |plus: &[&str], minus: &[&str]| (has(plus) as i32 - has(minus) as i32) as f64;
        let (fw, rt, up) = (axis(&["w", "up"], &["s", "down"]), axis(&["d", "right"], &["a", "left"]), axis(&["e"], &["q"]));
        let step = f.speed * if f.fast { 3.0 } else { 1.0 } * dt;
        if fw != 0.0 || rt != 0.0 || up != 0.0 {
            self.studio.update(cx, |s, cx| s.navigate(Nav::Fly(fw * step, rt * step, up * step), cx));
            self.interacting(cx);
        }
        true
    }

    /// A key while flying; true when fly mode took it.
    fn fly_keystroke(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(f) = self.fly.as_mut() else { return false };
        let key = k.key.as_str();
        f.fast = k.modifiers.shift;
        match key {
            "w" | "a" | "s" | "d" | "q" | "e" | "up" | "down" | "left" | "right" => {
                if !f.held.iter().any(|h| h == key) {
                    f.held.push(key.to_string());
                }
            }
            "escape" => self.end_fly(false, cx),
            "enter" | "space" => self.end_fly(true, cx),
            _ => {}
        }
        cx.notify();
        true
    }

    /// A key let go (the Studio passes key ups on while flying).
    pub fn fly_key_up(&mut self, key: &str, shift: bool) {
        if let Some(f) = self.fly.as_mut() {
            f.held.retain(|h| h != key);
            f.fast = shift;
        }
    }

    /// The pointer moved while flying: turn the view by as much.
    fn fly_look(&mut self, m: [f64; 2], cx: &mut Context<Self>) {
        let Some(f) = self.fly.as_mut() else { return };
        let Some(last) = f.last.replace(m) else { return };
        let (dx, dy) = (m[0] - last[0], m[1] - last[1]);
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.studio.update(cx, |s, cx| s.navigate(Nav::Look(dx * 0.2, -dy * 0.2), cx));
        self.interacting(cx);
    }

    // ---- edit mode -------------------------------------------------------------------------------

    fn mesh(&self, cx: &App) -> Option<(Vec<V3>, Vec<Vec<u32>>, View3)> {
        let (p, _, scene, t) = self.scene(cx)?;
        let Scene::Space(s) = &scene else { return None };
        let id = self.studio.read(cx).active()?.to_string();
        let (v, f) = model::edit_mesh(s, &model::worlds(s, t), &id)?;
        Some((v, f, self.view3(s, t, &p, cx)))
    }

    pub fn toggle_all_mesh(&mut self, cx: &mut Context<Self>) {
        let Some((v, f, _)) = self.mesh(cx) else { return };
        self.studio.update(cx, |s, cx| {
            if s.edit_sel.is_empty() {
                s.edit_sel = match s.select_mode {
                    SelectMode::Face => super::EditSel { faces: (0..f.len() as u32).collect(), ..Default::default() },
                    SelectMode::Edge => super::EditSel { edges: model::mesh_edges(&f), ..Default::default() },
                    SelectMode::Vertex => super::EditSel { vertices: (0..v.len() as u32).collect(), ..Default::default() },
                };
            } else {
                s.edit_sel = super::EditSel::default();
            }
            s.changed(cx);
        });
    }

    /// The vertex, edge or face under the pointer.
    fn pick_mesh(&self, m: [f64; 2], cx: &App) -> Option<MeshPick> {
        let (verts, faces, view) = self.mesh(cx)?;
        match self.studio.read(cx).select_mode {
            SelectMode::Vertex => {
                let mut best: Option<(f64, u32)> = None;
                for (i, v) in verts.iter().enumerate() {
                    if let Some(q) = view.project(*v) {
                        let d = ((q[0] - m[0]).powi(2) + (q[1] - m[1]).powi(2)).sqrt();
                        if d < 12.0 && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, i as u32));
                        }
                    }
                }
                best.map(|(_, i)| MeshPick::Vertex(i))
            }
            SelectMode::Edge => nearest_edge(&verts, &faces, &view, m).map(|(a, b)| MeshPick::Edge(a, b)),
            SelectMode::Face => {
                let (o, d) = view.ray(m);
                let mut best: Option<(f64, u32)> = None;
                for (i, f) in faces.iter().enumerate() {
                    for k in 1..f.len().saturating_sub(1) {
                        let (a, b, c) = (verts.get(f[0] as usize), verts.get(f[k] as usize), verts.get(f[k + 1] as usize));
                        if let (Some(a), Some(b), Some(c)) = (a, b, c)
                            && let Some(hit) = math::ray_triangle(o, d, *a, *b, *c)
                            && best.is_none_or(|(bt, _)| hit < bt)
                        {
                            best = Some((hit, i as u32));
                        }
                    }
                }
                best.map(|(_, i)| MeshPick::Face(i))
            }
        }
    }

    fn mesh_click(&mut self, m: [f64; 2], additive: bool, cx: &mut Context<Self>) {
        let pick = self.pick_mesh(m, cx);
        self.studio.update(cx, |s, cx| {
            let sel = &mut s.edit_sel;
            if !additive {
                *sel = super::EditSel::default();
            }
            match pick {
                Some(MeshPick::Vertex(i)) => toggle(&mut sel.vertices, i, additive),
                Some(MeshPick::Edge(a, b)) => toggle(&mut sel.edges, (a, b), additive),
                Some(MeshPick::Face(i)) => toggle(&mut sel.faces, i, additive),
                None => {}
            }
            s.changed(cx);
        });
    }

    fn mesh_box(&mut self, a: [f64; 2], b: [f64; 2], additive: bool, cx: &mut Context<Self>) {
        let Some((verts, faces, view)) = self.mesh(cx) else { return };
        let inside = |p: V3| view.project(p).is_some_and(|q| q[0] >= a[0].min(b[0]) && q[0] <= a[0].max(b[0]) && q[1] >= a[1].min(b[1]) && q[1] <= a[1].max(b[1]));
        self.studio.update(cx, |s, cx| {
            let mut sel = if additive { s.edit_sel.clone() } else { super::EditSel::default() };
            match s.select_mode {
                SelectMode::Vertex => {
                    for (i, v) in verts.iter().enumerate() {
                        if inside(*v) && !sel.vertices.contains(&(i as u32)) {
                            sel.vertices.push(i as u32);
                        }
                    }
                }
                SelectMode::Edge => {
                    for (x, y) in model::mesh_edges(&faces) {
                        if inside(verts[x as usize]) && inside(verts[y as usize]) && !sel.edges.contains(&(x, y)) {
                            sel.edges.push((x, y));
                        }
                    }
                }
                SelectMode::Face => {
                    for (i, f) in faces.iter().enumerate() {
                        let n = f.len().max(1) as f64;
                        let c = f.iter().fold([0.0; 3], |acc, v| math::add(acc, verts.get(*v as usize).copied().unwrap_or_default()));
                        if inside(math::scale(c, 1.0 / n)) && !sel.faces.contains(&(i as u32)) {
                            sel.faces.push(i as u32);
                        }
                    }
                }
            }
            s.edit_sel = sel;
            s.changed(cx);
        });
    }

    /// E, I, Ctrl+B: the operation's amount follows the mouse (extrude pulls the new faces out
    /// at once; inset and bevel apply when confirmed).
    fn start_mesh_modal(&mut self, kind: ModalKind, extrude_first: bool, cx: &mut Context<Self>) {
        let Some((verts, faces, view)) = self.mesh(cx) else { return };
        let sel = self.studio.read(cx).edit_sel.clone();
        if sel.is_empty() {
            super::flash("Select some faces, edges or vertices first.", cx);
            return;
        }
        let picked = sel.all_vertices(&faces);
        let pivot = math::scale(picked.iter().fold([0.0; 3], |a, v| math::add(a, verts.get(*v as usize).copied().unwrap_or_default())), 1.0 / picked.len().max(1) as f64);
        // The selection's normal: the faces' (or those around the vertices).
        let mut n = [0.0; 3];
        for (i, f) in faces.iter().enumerate() {
            let chosen = sel.faces.contains(&(i as u32)) || (sel.faces.is_empty() && f.iter().all(|v| picked.contains(v)));
            if chosen && f.len() >= 3 {
                let (a, b, c) = (verts[f[0] as usize], verts[f[1] as usize], verts[f[2] as usize]);
                n = math::add(n, math::cross(math::sub(b, a), math::sub(c, a)));
            }
        }
        let normal = if math::len(n) < 1e-9 { view.toward_viewer(pivot) } else { math::norm(n) };
        let key = self.studio.update(cx, |s, _| s.drag_key());
        let unit = view.units_per_pixel(pivot);
        self.mesh_modal = Some(MeshModal { kind, mouse0: self.mouse, normal, unit, typed: String::new(), amount: 0.0, applied: 0.0, moving: !extrude_first, busy: kind == ModalKind::Extrude, key });
        if kind == ModalKind::Extrude && extrude_first {
            // Extrude in place, then pull the new faces out.
            let st = self.studio.read(cx);
            let clip = st.clip;
            let id = st.active().map(str::to_string);
            // The same undo key as the pull that follows: one step for the whole extrude.
            let key = self.mesh_modal.as_ref().map(|m| m.key.clone()).unwrap_or_default();
            let mut p = json!({ "clipId": clip, "id": id, "op": "extrude", "params": { "distance": 0 }, "coalesce": key });
            let s = sel.params();
            p["vertices"] = s["vertices"].clone();
            p["faces"] = s["faces"].clone();
            let task = super::call("motion.editMesh", p, cx);
            cx.spawn(async move |this, cx| {
                let r = task.await;
                this.update(cx, |this, cx| match r {
                    Ok(v) => {
                        this.studio.update(cx, |s, cx| {
                            s.edit_sel = super::selection_from(&v);
                            s.changed(cx);
                        });
                        if let Some(m) = this.mesh_modal.as_mut() {
                            m.busy = false;
                        }
                        let mouse = this.mouse;
                        this.mesh_modal_to(mouse, cx);
                    }
                    Err(e) => {
                        this.mesh_modal = None;
                        this._intercept = None;
                        this.store.update(cx, |s, cx| s.error(e, cx));
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        } else if let Some(m) = self.mesh_modal.as_mut() {
            m.busy = false;
        }
        self.intercept(cx);
        cx.notify();
    }

    fn mesh_modal_to(&mut self, mouse: [f64; 2], cx: &mut Context<Self>) {
        let snap = self.studio.read(cx).snapping;
        let Some(m) = self.mesh_modal.as_mut() else { return };
        let moved = ((mouse[0] - m.mouse0[0]).powi(2) + (mouse[1] - m.mouse0[1]).powi(2)).sqrt();
        m.amount = match m.typed.parse::<f64>() {
            Ok(v) => v,
            Err(_) => {
                let v = match m.kind {
                    // Up the screen is out along the normal.
                    ModalKind::Extrude => (m.mouse0[1] - mouse[1]) * m.unit,
                    _ => moved * m.unit * 0.5,
                };
                if snap { math::snap(v, 0.05) } else { v }
            }
        };
        if m.kind != ModalKind::Extrude || m.busy || (m.amount - m.applied).abs() < 1e-6 {
            cx.notify();
            return;
        }
        // Extrude: move the new faces by what is still missing (one step at a time, never lost).
        let delta = m.amount - m.applied;
        m.applied = m.amount;
        m.busy = true;
        let off = math::scale(m.normal, delta);
        let key = m.key.clone();
        let st = self.studio.read(cx);
        let mut p = json!({ "clipId": st.clip, "id": st.active(), "op": MESH_MOVE, "params": { "offset": off }, "coalesce": key });
        let s = st.edit_sel.params();
        p["vertices"] = s["vertices"].clone();
        p["faces"] = s["faces"].clone();
        let task = super::call("motion.editMesh", p, cx);
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                if let Err(e) = r {
                    this.store.update(cx, |s, cx| s.error(e, cx));
                }
                if let Some(m) = this.mesh_modal.as_mut() {
                    m.busy = false;
                    let mouse = this.mouse;
                    this.mesh_modal_to(mouse, cx);
                }
            })
            .ok();
        })
        .detach();
        self.interacting(cx);
        cx.notify();
    }

    fn finish_mesh_modal(&mut self, m: MeshModal, cx: &mut Context<Self>) {
        match m.kind {
            ModalKind::Inset => self.studio.update(cx, |s, cx| s.mesh_op("inset", json!({ "thickness": m.amount.abs().max(0.001) }), cx)),
            ModalKind::Bevel => self.studio.update(cx, |s, cx| s.mesh_op("bevel", json!({ "width": m.amount.abs().max(0.001), "segments": 1 }), cx)),
            _ => {}
        }
    }

    // ---- 2D --------------------------------------------------------------------------------------

    /// Begins moving / scaling / turning the selected layers (or their anchor); false when
    /// nothing is selected.
    fn start_layer_drag(&mut self, op: LayerOp, mouse: [f64; 2], cx: &mut Context<Self>) -> bool {
        let Some((p, _, Scene::Flat(s), t)) = self.scene(cx) else { return false };
        let st = self.studio.read(cx);
        let comp = st.composition.clone();
        let mut starts = vec![];
        for id in &st.selection {
            let Some((l, world)) = model::layer_world(&s, comp.as_deref(), t, id) else { continue };
            let parent = math::aff_mul(&world, &math::aff_invert(&model::own_affine(&l)).unwrap_or(math::AFFINE_ID));
            starts.push(LayerStart { id: id.clone(), x: l.x, y: l.y, rotation: l.rotation, scale: l.scale, scale_x: l.scale_x, scale_y: l.scale_y, anchor: [l.anchor_x, l.anchor_y], world, parent });
        }
        if starts.is_empty() {
            return false;
        }
        let v = self.view2(&s, &p, cx);
        let key = self.studio.update(cx, |s, _| s.drag_key());
        self.drag = Some(Drag::Layer(LayerDrag { op, mouse0: v.to_canvas(mouse), starts, key, reached: vec![] }));
        true
    }

    fn layer_drag_to(&mut self, mouse: [f64; 2], cx: &mut Context<Self>) {
        let Some((p, _, Scene::Flat(s), _)) = self.scene(cx) else { return };
        let v = self.view2(&s, &p, cx);
        let m = v.to_canvas(mouse);
        let shift = self.store.read(cx).playback.read(cx).playing && false;
        let _ = shift;
        let mods = SHIFT.with(|c| c.get());
        let Some(Drag::Layer(d)) = self.drag.as_mut() else { return };
        d.reached.clear();
        for st in &d.starts {
            let mut props = serde_json::Map::new();
            let anchor_world = math::aff_apply(&st.world, st.anchor);
            match d.op {
                LayerOp::Move => {
                    let mut delta = [m[0] - d.mouse0[0], m[1] - d.mouse0[1]];
                    if mods {
                        // Shift: only along the bigger direction.
                        if delta[0].abs() > delta[1].abs() { delta[1] = 0.0 } else { delta[0] = 0.0 }
                    }
                    let local = math::aff_invert(&st.parent).map(|inv| math::aff_dir(&inv, delta)).unwrap_or(delta);
                    props.insert("x".into(), json!(round2(st.x + local[0])));
                    props.insert("y".into(), json!(round2(st.y + local[1])));
                }
                LayerOp::Rotate => {
                    let a0 = (d.mouse0[1] - anchor_world[1]).atan2(d.mouse0[0] - anchor_world[0]);
                    let a1 = (m[1] - anchor_world[1]).atan2(m[0] - anchor_world[0]);
                    let mut deg = (a1 - a0).to_degrees();
                    if mods {
                        deg = math::snap(deg, 15.0);
                    }
                    props.insert("rotation".into(), json!(round2(st.rotation + deg)));
                }
                LayerOp::Scale(hx, hy) => {
                    // In the layer's own axes, relative to its anchor.
                    let Some(inv) = math::aff_invert(&st.world) else { continue };
                    let v0 = math::aff_apply(&inv, d.mouse0);
                    let v1 = math::aff_apply(&inv, m);
                    let (r0, r1) = ([v0[0] - st.anchor[0], v0[1] - st.anchor[1]], [v1[0] - st.anchor[0], v1[1] - st.anchor[1]]);
                    let f = |a: f64, b: f64| if a.abs() < 1e-6 { 1.0 } else { b / a };
                    let (mut fx, mut fy) = (if hx != 0 { f(r0[0], r1[0]) } else { 1.0 }, if hy != 0 { f(r0[1], r1[1]) } else { 1.0 });
                    if mods || (hx != 0 && hy != 0 && st.scale_x == st.scale_y && !mods && false) {
                        let k = if hx != 0 && hy != 0 { (fx + fy) / 2.0 } else if hx != 0 { fx } else { fy };
                        fx = k;
                        fy = k;
                    }
                    if (fx - fy).abs() < 1e-9 && st.scale_x == st.scale_y {
                        props.insert("scale".into(), json!(round3(st.scale * fx)));
                    } else {
                        props.insert("scaleX".into(), json!(round3(st.scale_x * fx)));
                        props.insert("scaleY".into(), json!(round3(st.scale_y * fy)));
                    }
                }
                LayerOp::Anchor => {
                    // The anchor moves; the layer stays where it is (its position follows).
                    let Some(inv) = math::aff_invert(&st.world) else { continue };
                    let a = math::aff_apply(&inv, m);
                    let own = math::layer_affine(0.0, 0.0, st.rotation, 0.0, st.scale * st.scale_x, st.scale * st.scale_y, 0.0, 0.0);
                    let shift = math::aff_dir(&own, [a[0] - st.anchor[0], a[1] - st.anchor[1]]);
                    props.insert("anchorX".into(), json!(round2(a[0])));
                    props.insert("anchorY".into(), json!(round2(a[1])));
                    props.insert("x".into(), json!(round2(st.x + shift[0])));
                    props.insert("y".into(), json!(round2(st.y + shift[1])));
                }
            }
            d.reached.push((st.id.clone(), props));
        }
        let snapshot = LayerDrag { op: d.op, mouse0: d.mouse0, starts: vec![], key: d.key.clone(), reached: d.reached.clone() };
        self.send_layer(&snapshot, false, cx);
        self.interacting(cx);
        cx.notify();
    }

    /// Sends a layer drag's values (or puts the starting ones back).
    fn send_layer(&self, d: &LayerDrag, cancel: bool, cx: &mut Context<Self>) {
        let Some(clip) = self.studio.read(cx).clip else { return };
        let time = self.studio.read(cx).playhead(cx);
        let cmds: Vec<(String, Value)> = if cancel {
            d.starts
                .iter()
                .map(|s| {
                    let props = json!({ "x": s.x, "y": s.y, "rotation": s.rotation, "scale": s.scale, "scaleX": s.scale_x, "scaleY": s.scale_y, "anchorX": s.anchor[0], "anchorY": s.anchor[1] });
                    ("motion.updateLayer".to_string(), json!({ "clipId": clip, "id": s.id, "props": props, "time": time, "coalesce": d.key }))
                })
                .collect()
        } else {
            d.reached.iter().map(|(id, props)| ("motion.updateLayer".to_string(), json!({ "clipId": clip, "id": id, "props": props, "time": time, "coalesce": d.key }))).collect()
        };
        self.studio.update(cx, |s, cx| s.send(cmds, cx));
    }

    /// Finishes the path drawn with the pen: a new path layer, or a mask on the selected layer.
    fn finish_pen(&mut self, closed: bool, cx: &mut Context<Self>) {
        let pts = std::mem::take(&mut self.pen);
        self._intercept = None;
        if pts.len() < 2 {
            cx.notify();
            return;
        }
        let Some((_, _, scene, t)) = self.scene(cx) else { return };
        let Scene::Flat(s) = &scene else { return };
        let st = self.studio.read(cx);
        let clip = st.clip;
        let comp = st.composition.clone();
        let mask_on = st.mask_mode.then(|| st.active().map(str::to_string)).flatten().filter(|id| s.find_layer(id).is_some());
        let to_layer = |p: [f64; 2]| -> [f64; 2] {
            match &mask_on {
                Some(id) => model::layer_world(s, comp.as_deref(), t, id).and_then(|(_, w)| math::aff_invert(&w)).map(|inv| math::aff_apply(&inv, p)).unwrap_or(p),
                None => p,
            }
        };
        let d = path_data(&pts, closed, to_layer);
        match mask_on {
            Some(id) => super::run("motion.setStackItem", json!({ "clipId": clip, "id": id, "field": "masks", "item": { "type": "path", "d": d } }), cx),
            None => {
                let new_id = model::fresh_id(&scene, "path");
                let mut layer = json!({ "id": new_id, "type": "path", "d": d, "closed": closed, "stroke": { "color": "#ffffff", "width": 4 } });
                if closed {
                    layer["fill"] = json!("#ff5a36");
                }
                let mut p = json!({ "clipId": clip, "layer": layer });
                if let Some(c) = comp {
                    p["parent"] = json!(c);
                }
                self.studio.update(cx, |s, cx| s.run_then("motion.setLayer", p, cx, move |s, _, cx| s.set_selection(vec![new_id], cx)));
            }
        }
        cx.notify();
    }

    fn add_shape(&mut self, from: [f64; 2], to: [f64; 2], square: bool, cx: &mut Context<Self>) {
        let Some((_, _, scene, _)) = self.scene(cx) else { return };
        let st = self.studio.read(cx);
        let (clip, comp, tool) = (st.clip, st.composition.clone(), st.tool);
        let (mut w, mut h) = ((to[0] - from[0]).abs(), (to[1] - from[1]).abs());
        if square {
            w = w.max(h);
            h = w;
        }
        if w < 4.0 && h < 4.0 {
            // A click: a shape of a useful size.
            w = 200.0;
            h = 200.0;
        }
        let c = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
        let r = w.max(h) / 2.0;
        let (stem, layer) = match tool {
            Tool::Ellipse => ("ellipse", json!({ "type": "ellipse", "width": round2(w), "height": round2(h), "fill": "#ffffff" })),
            Tool::Star => ("star", json!({ "type": "star", "radius": round2(r), "innerRadius": round2(r * 0.45), "points": 5, "fill": "#f0b44c" })),
            Tool::Polygon => ("polygon", json!({ "type": "polygon", "radius": round2(r), "sides": 6, "fill": "#5cc8ff" })),
            _ => ("rect", json!({ "type": "rect", "width": round2(w), "height": round2(h), "fill": "#ff5a36" })),
        };
        let id = model::fresh_id(&scene, stem);
        let mut layer = layer;
        layer["id"] = json!(id);
        layer["x"] = json!(round2(c[0]));
        layer["y"] = json!(round2(c[1]));
        let mut p = json!({ "clipId": clip, "layer": layer });
        if let Some(c) = comp {
            p["parent"] = json!(c);
        }
        self.studio.update(cx, |s, cx| s.run_then("motion.setLayer", p, cx, move |s, _, cx| s.set_selection(vec![id], cx)));
    }

    fn add_text(&mut self, at: [f64; 2], cx: &mut Context<Self>) {
        let Some((p, _, scene, _)) = self.scene(cx) else { return };
        let st = self.studio.read(cx);
        let (clip, comp) = (st.clip, st.composition.clone());
        let k = p.settings.height as f64 / 1080.0;
        let id = model::fresh_id(&scene, "text");
        let layer = json!({ "id": id, "type": "text", "text": "Your words", "fontSize": round2(96.0 * k), "fill": "#ffffff", "x": round2(at[0]), "y": round2(at[1]) });
        let mut params = json!({ "clipId": clip, "layer": layer });
        if let Some(c) = comp {
            params["parent"] = json!(c);
        }
        self.studio.update(cx, |s, cx| {
            s.run_then("motion.setLayer", params, cx, move |s, _, cx| {
                s.set_selection(vec![id], cx);
                s.properties.update(cx, |p, cx| p.focus_text(cx));
            })
        });
    }

    /// The selected layer's box handles on screen: (handle, position).
    fn handles2(&self, s: &Scene2d, v: &View2, t: f64, cx: &App) -> Option<Handles2> {
        let st = self.studio.read(cx);
        let id = st.active()?;
        let corners = model::layer_corners(s, st.composition.as_deref(), t, id, v.project)?;
        let sc: Vec<[f64; 2]> = corners.iter().map(|c| v.to_screen(*c)).collect();
        let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let handles = vec![
            (LayerOp::Scale(-1, -1), sc[0]),
            (LayerOp::Scale(1, -1), sc[1]),
            (LayerOp::Scale(1, 1), sc[2]),
            (LayerOp::Scale(-1, 1), sc[3]),
            (LayerOp::Scale(0, -1), mid(sc[0], sc[1])),
            (LayerOp::Scale(1, 0), mid(sc[1], sc[2])),
            (LayerOp::Scale(0, 1), mid(sc[2], sc[3])),
            (LayerOp::Scale(-1, 0), mid(sc[3], sc[0])),
        ];
        let (l, world) = model::layer_world(s, st.composition.as_deref(), t, id)?;
        let anchor = v.to_screen(math::aff_apply(&world, [l.anchor_x, l.anchor_y]));
        Some((sc, handles, anchor))
    }

    // ---- the pointer -------------------------------------------------------------------------------

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = [f32::from(e.position.x) as f64, f32::from(e.position.y) as f64];
        self.mouse = m;
        SHIFT.with(|c| c.set(e.modifiers.shift));
        self.studio.update(cx, |s, _| s.area = super::Area::Viewport);
        if self.studio.read(cx).popover.is_some() {
            self.studio.update(cx, |s, cx| {
                s.popover = None;
                cx.notify();
            });
        }
        // Flying: a click keeps the view, a right click puts it back.
        if let Some(f) = &self.fly {
            if !f.hold {
                self.end_fly(e.button != MouseButton::Right, cx);
            }
            return;
        }
        // A modal operation: a click confirms it.
        if self.modal_key.is_some() || self.mesh_modal.is_some() {
            if e.button == MouseButton::Right {
                self.cancel_modal(cx);
            } else if e.button == MouseButton::Left {
                self.confirm_modal(cx);
            }
            return;
        }
        let space = self.studio.read(cx).space_down;
        let three = self.studio.read(cx).is_3d(cx);
        let ctrl = e.modifiers.control || e.modifiers.platform;
        match e.button {
            MouseButton::Middle => {
                // Blender's: middle orbits, Shift+middle pans, Ctrl+middle zooms (2D: pans).
                let d = if !three || e.modifiers.shift {
                    Drag::Pan { last: e.position }
                } else if ctrl {
                    Drag::Zoom { last: e.position }
                } else {
                    Drag::Orbit { last: e.position }
                };
                self.begin_nav(d, cx);
                return;
            }
            MouseButton::Right => {
                if !self.pen.is_empty() {
                    self.finish_pen(false, cx);
                    return;
                }
                if three {
                    // A click opens the menu (when let go); a drag looks around and flies.
                    self.drag = Some(Drag::Right { start: e.position, last: e.position, moved: false });
                } else {
                    self.context_menu(e.position, window, cx);
                }
                return;
            }
            _ => {}
        }
        if space {
            self.studio.update(cx, |s, _| s.space_panned = true);
            self.begin_nav(Drag::Pan { last: e.position }, cx);
            return;
        }
        if three && e.modifiers.alt {
            let d = if e.modifiers.shift {
                Drag::Pan { last: e.position }
            } else if ctrl {
                Drag::Zoom { last: e.position }
            } else {
                Drag::Orbit { last: e.position }
            };
            self.begin_nav(d, cx);
            return;
        }
        let additive = e.modifiers.shift || e.modifiers.platform || e.modifiers.control;
        match self.armed.take() {
            Some(Armed::LoopCut) => {
                if let Some((verts, faces, view)) = self.mesh(cx)
                    && let Some((a, b)) = nearest_edge(&verts, &faces, &view, m)
                {
                    let st = self.studio.read(cx);
                    let p = json!({ "clipId": st.clip, "id": st.active(), "op": "loopCut", "vertices": [a, b], "params": { "cuts": 1 } });
                    self.studio.update(cx, |s, cx| {
                        s.run_then("motion.editMesh", p, cx, |s, v, cx| {
                            s.edit_sel = super::selection_from(&v);
                            s.changed(cx);
                        })
                    });
                }
                cx.notify();
                return;
            }
            Some(Armed::BoxSelect) => {
                self.drag = Some(Drag::Press { start: e.position, now: e.position, additive, moved: true, boxing: true });
                cx.notify();
                return;
            }
            None => {}
        }
        let Some((p, _, scene, t)) = self.scene(cx) else { return };
        match &scene {
            Scene::Space(s) => {
                // A gizmo handle?
                if let Some(frame) = self.gizmo_frame(s, &scene, t, cx) {
                    let view = self.view3(s, t, &p, cx);
                    let kind = self.gizmo_kind(cx);
                    if let Some(h) = gizmo::hit(&gizmo::parts(&frame, kind, &view), m) {
                        let st = self.studio.read(cx);
                        let w = model::worlds(s, t);
                        let starts = gizmo::starts(s, &scene, &st.selection, t, &w);
                        let mut sess = Session::new(kind, h, frame, starts, m, &view);
                        sess.snap = st.snapping || e.modifiers.control;
                        self.session = Some(sess);
                        self.modal_key = None;
                        let key = self.studio.update(cx, |s, _| s.drag_key());
                        self.gizmo_key = Some(key);
                        self.drag = Some(Drag::Gizmo);
                        cx.notify();
                        return;
                    }
                }
                self.drag = Some(Drag::Press { start: e.position, now: e.position, additive, moved: false, boxing: false });
            }
            Scene::Flat(s) => {
                let v = self.view2(s, &p, cx);
                let c = v.to_canvas(m);
                let tool = self.studio.read(cx).tool;
                match tool {
                    Tool::Pen => {
                        // Back on the first point: close the path.
                        if self.pen.len() >= 2 && {
                            let f = v.to_screen(self.pen[0].p);
                            (f[0] - m[0]).hypot(f[1] - m[1]) < 9.0
                        } {
                            self.finish_pen(true, cx);
                            return;
                        }
                        if self.pen.is_empty() {
                            self.intercept(cx);
                        }
                        self.pen.push(PenPoint { p: c, handle: [0.0, 0.0] });
                        self.drag = Some(Drag::Pen { at: c, handle: [0.0, 0.0] });
                    }
                    t if t.is_shape() => self.drag = Some(Drag::Shape { from: c, to: c, square: e.modifiers.shift }),
                    Tool::Text => self.add_text(c, cx),
                    Tool::Anchor => {
                        if self.studio.read(cx).selection.is_empty()
                            && let Some(id) = model::hit2d(s, self.studio.read(cx).composition.as_deref(), t, c, v.project)
                        {
                            self.studio.update(cx, |st, cx| st.select(&id, false, cx));
                        }
                        self.start_layer_drag(LayerOp::Anchor, m, cx);
                    }
                    _ => {
                        // Handles of the selected layer first.
                        if let Some((corners, handles, _)) = self.handles2(s, &v, t, cx) {
                            if let Some((op, _)) = handles.iter().find(|(_, h)| (h[0] - m[0]).hypot(h[1] - m[1]) < 7.0) {
                                self.start_layer_drag(*op, m, cx);
                                cx.notify();
                                return;
                            }
                            // Just outside a corner: turn it.
                            if corners.iter().any(|h| (h[0] - m[0]).hypot(h[1] - m[1]) < 22.0) && !math::in_polygon(m, &corners) {
                                self.start_layer_drag(LayerOp::Rotate, m, cx);
                                cx.notify();
                                return;
                            }
                        }
                        let comp = self.studio.read(cx).composition.clone();
                        match model::hit2d(s, comp.as_deref(), t, c, v.project) {
                            Some(id) => {
                                let selected = self.studio.read(cx).selection.contains(&id);
                                if !selected || additive {
                                    self.studio.update(cx, |st, cx| st.select(&id, additive, cx));
                                }
                                if !additive {
                                    self.start_layer_drag(LayerOp::Move, m, cx);
                                }
                            }
                            None => self.drag = Some(Drag::Press { start: e.position, now: e.position, additive, moved: false, boxing: false }),
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = [f32::from(e.position.x) as f64, f32::from(e.position.y) as f64];
        self.mouse = m;
        SHIFT.with(|c| c.set(e.modifiers.shift));
        if self.fly.as_ref().is_some_and(|f| !f.hold) {
            self.fly_look(m, cx);
            return;
        }
        if self.modal_key.is_some() && self.drag.is_none() {
            if let Some(s) = self.session.as_mut() {
                s.snap = s.snap || e.modifiers.control;
            }
            self.apply_any(cx);
            return;
        }
        if self.mesh_modal.is_some() {
            self.mesh_modal_to(m, cx);
            return;
        }
        if let Some(Drag::Layer(_)) = &self.drag
            && self.modal_key.is_some()
        {
            self.layer_drag_to(m, cx);
            return;
        }
        // Hover: gizmo handles, loop cut edges.
        if self.drag.is_none() {
            let before = (self.hover, self.hover_edge);
            self.hover = None;
            self.hover_edge = None;
            if let Some((p, _, Scene::Space(s), t)) = self.scene(cx) {
                let scene = Scene::Space(s.clone());
                if let Some(frame) = self.gizmo_frame(&s, &scene, t, cx) {
                    let view = self.view3(&s, t, &p, cx);
                    self.hover = gizmo::hit(&gizmo::parts(&frame, self.gizmo_kind(cx), &view), m);
                }
                if self.armed == Some(Armed::LoopCut)
                    && let Some((v, f, view)) = self.mesh(cx)
                {
                    self.hover_edge = nearest_edge(&v, &f, &view, m);
                }
            }
            if !self.pen.is_empty() || before != (self.hover, self.hover_edge) {
                cx.notify();
            }
        }
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = [f32::from(e.position.x) as f64, f32::from(e.position.y) as f64];
        self.mouse = m;
        // The pointer on the 2D canvas (pen and shape tools).
        let canvas_at = match self.scene(cx) {
            Some((p, _, Scene::Flat(s), _)) => Some(self.view2(&s, &p, cx).to_canvas(m)),
            _ => None,
        };
        SHIFT.with(|c| c.set(e.modifiers.shift));
        match self.drag.as_mut() {
            Some(Drag::Orbit { last }) => {
                let d = e.position - *last;
                *last = e.position;
                let (dx, dy) = (f32::from(d.x) as f64, f32::from(d.y) as f64);
                self.studio.update(cx, |s, cx| s.navigate(Nav::Orbit(dx * 0.4, dy * 0.4), cx));
                self.interacting(cx);
            }
            Some(Drag::Ball { start, last, moved, .. }) => {
                let d = e.position - *last;
                *last = e.position;
                let far = e.position - *start;
                if !*moved && (f32::from(far.x).abs() > 3. || f32::from(far.y).abs() > 3.) {
                    *moved = true;
                }
                if *moved {
                    let (dx, dy) = (f32::from(d.x) as f64, f32::from(d.y) as f64);
                    self.studio.update(cx, |s, cx| s.navigate(Nav::Orbit(dx * 0.6, dy * 0.6), cx));
                    self.interacting(cx);
                }
            }
            Some(Drag::Zoom { last }) => {
                let d = e.position - *last;
                *last = e.position;
                // Up: closer.
                let k = (f32::from(d.y) as f64 * 0.008).exp();
                if self.studio.read(cx).is_3d(cx) {
                    self.studio.update(cx, |s, cx| s.navigate(Nav::Zoom(k, None), cx));
                } else {
                    let z = self.canvas_zoom(cx).unwrap_or(1.0);
                    self.studio.update(cx, |s, cx| s.zoom_canvas_to(z / k, cx));
                }
                self.interacting(cx);
            }
            Some(Drag::Right { start, last, moved }) => {
                let far = e.position - *start;
                let d = e.position - *last;
                *last = e.position;
                let begins = !*moved && (f32::from(far.x).abs() > 4. || f32::from(far.y).abs() > 4.);
                if begins {
                    *moved = true;
                }
                let moved = *moved;
                if begins {
                    self.start_fly(true, cx);
                }
                if moved {
                    let (dx, dy) = (f32::from(d.x) as f64, f32::from(d.y) as f64);
                    self.studio.update(cx, |s, cx| s.navigate(Nav::Look(dx * 0.2, -dy * 0.2), cx));
                    self.interacting(cx);
                }
            }
            Some(Drag::Pan { last }) => {
                let d = e.position - *last;
                *last = e.position;
                let (dx, dy) = (f32::from(d.x) as f64, f32::from(d.y) as f64);
                self.pan_by(dx, dy, cx);
            }
            Some(Drag::Press { start, now, moved, boxing, .. }) => {
                *now = e.position;
                let d = e.position - *start;
                if !*moved && (f32::from(d.x).abs() > 4. || f32::from(d.y).abs() > 4.) {
                    *moved = true;
                    *boxing = true;
                }
                cx.notify();
            }
            Some(Drag::Gizmo) => {
                if let Some(s) = self.session.as_mut() {
                    s.snap = self.studio.read(cx).snapping || e.modifiers.control;
                }
                let key = self.gizmo_key.clone();
                self.modal_key = key;
                self.apply_session(cx);
                self.modal_key = None;
            }
            Some(Drag::Layer(_)) => self.layer_drag_to(m, cx),
            Some(Drag::Pen { at, handle }) => {
                let Some(c) = canvas_at else { return };
                *handle = [c[0] - at[0], c[1] - at[1]];
                if let Some(last) = self.pen.last_mut() {
                    last.handle = *handle;
                }
                cx.notify();
            }
            Some(Drag::Shape { to, square, .. }) => {
                let Some(c) = canvas_at else { return };
                *to = c;
                *square = e.modifiers.shift;
                cx.notify();
            }
            None => {}
        }
    }

    fn drag_end(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = [f32::from(e.position.x) as f64, f32::from(e.position.y) as f64];
        match self.drag.take() {
            Some(Drag::Orbit { .. } | Drag::Pan { .. } | Drag::Zoom { .. }) => self.studio.update(cx, |s, cx| s.nav_end(cx)),
            Some(Drag::Ball { moved, axis, .. }) => {
                if !moved && let Some(name) = axis {
                    // Clicked again on the axis it looks along: from the other side, like Blender.
                    let shown = self.studio.read(cx).view_shown();
                    let name = if !shown.1 && shown.0.aligned_axis() == Some(name) { opposite(name) } else { name };
                    self.studio.update(cx, |s, cx| {
                        s.gesture = false;
                        s.look_from(name, cx)
                    });
                } else {
                    self.studio.update(cx, |s, cx| s.nav_end(cx));
                }
            }
            Some(Drag::Right { moved, start, .. }) => {
                if moved {
                    self.end_fly(true, cx);
                } else {
                    self.context_menu(start, window, cx);
                }
            }
            Some(Drag::Press { start, now, additive, moved, boxing }) => {
                let a = [f32::from(start.x) as f64, f32::from(start.y) as f64];
                let b = [f32::from(now.x) as f64, f32::from(now.y) as f64];
                if boxing && moved {
                    self.box_select(a, b, additive, cx);
                } else {
                    self.click(m, additive, cx);
                }
            }
            Some(Drag::Gizmo) => {
                self.session = None;
                self.gizmo_key = None;
            }
            Some(Drag::Layer(_)) => {}
            Some(Drag::Shape { from, to, square }) => self.add_shape(from, to, square, cx),
            Some(other) => {
                if matches!(other, Drag::Pen { .. }) {
                    self.drag = None;
                }
            }
            None => {}
        }
        cx.notify();
    }

    fn click(&mut self, m: [f64; 2], additive: bool, cx: &mut Context<Self>) {
        let Some((p, _, scene, t)) = self.scene(cx) else { return };
        if self.studio.read(cx).mode == Mode::Edit {
            self.mesh_click(m, additive, cx);
            return;
        }
        let picked = match &scene {
            Scene::Space(s) => {
                let view = self.view3(s, t, &p, cx);
                let w = model::worlds(s, t);
                // Lights and cameras are picked by their icon (they have no surface).
                let helpers = self.studio.read(cx).helpers;
                let mut icon_hit = None;
                if helpers {
                    let mut ids: Vec<String> = s.lights.iter().map(|l| l.id.clone()).collect();
                    ids.push("camera".into());
                    ids.extend(s.cameras.iter().map(|c| c.id.clone()));
                    for id in ids {
                        if let Some(q) = w.get(&id).and_then(|mm| view.project(math::origin(mm)))
                            && (q[0] - m[0]).hypot(q[1] - m[1]) < 14.0
                        {
                            icon_hit = Some(id);
                        }
                    }
                }
                let (o, d) = view.ray(m);
                icon_hit.or_else(|| engine_pick(s, &w, t, o, d))
            }
            Scene::Flat(s) => {
                let v = self.view2(s, &p, cx);
                model::hit2d(s, self.studio.read(cx).composition.as_deref(), t, v.to_canvas(m), v.project)
            }
        };
        self.studio.update(cx, |s, cx| match picked {
            Some(id) => s.select(&id, additive, cx),
            None if !additive => s.set_selection(vec![], cx),
            None => {}
        });
    }

    fn box_select(&mut self, a: [f64; 2], b: [f64; 2], additive: bool, cx: &mut Context<Self>) {
        if self.studio.read(cx).mode == Mode::Edit {
            self.mesh_box(a, b, additive, cx);
            return;
        }
        let Some((p, _, scene, t)) = self.scene(cx) else { return };
        let (x0, x1, y0, y1) = (a[0].min(b[0]), a[0].max(b[0]), a[1].min(b[1]), a[1].max(b[1]));
        let inside = |q: [f64; 2]| q[0] >= x0 && q[0] <= x1 && q[1] >= y0 && q[1] <= y1;
        let mut ids = vec![];
        match &scene {
            Scene::Space(s) => {
                let view = self.view3(s, t, &p, cx);
                let w = model::worlds(s, t);
                for id in model::thing_ids(&scene) {
                    let at = match model::world_bounds(s, &w, t, &id) {
                        Some((lo, hi)) if model::item(&scene, &id).is_some_and(|i| matches!(i, model::Item::Object(_))) => math::lerp(lo, hi, 0.5),
                        _ => match w.get(&id) {
                            Some(mm) => math::origin(mm),
                            None => continue,
                        },
                    };
                    if view.project(at).is_some_and(inside) {
                        ids.push(id);
                    }
                }
            }
            Scene::Flat(s) => {
                let v = self.view2(s, &p, cx);
                let comp = self.studio.read(cx).composition.clone();
                let mut all = vec![];
                kimchi_core::motion::walk_layers(model::view_layers(s, comp.as_deref()), &mut |l| all.push(l.id.clone()));
                for id in all {
                    if let Some(c) = model::layer_corners(s, comp.as_deref(), t, &id, v.project) {
                        let sc: Vec<[f64; 2]> = c.iter().map(|q| v.to_screen(*q)).collect();
                        let centre = [(sc[0][0] + sc[2][0]) / 2.0, (sc[0][1] + sc[2][1]) / 2.0];
                        if sc.iter().any(|q| inside(*q)) || inside(centre) {
                            ids.push(id);
                        }
                    }
                }
            }
        }
        self.studio.update(cx, |s, cx| {
            let mut sel = if additive { s.selection.clone() } else { vec![] };
            for id in ids {
                if !sel.contains(&id) {
                    sel.push(id);
                }
            }
            s.set_selection(sel, cx);
        });
    }

    /// Starts a drag that moves the view (one undo step when it moves a locked camera).
    fn begin_nav(&mut self, d: Drag, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, _| s.nav_begin());
        self.drag = Some(d);
        cx.notify();
    }

    /// Pans by screen pixels: the 3D view (or a locked camera), or the 2D canvas.
    fn pan_by(&mut self, dx: f64, dy: f64, cx: &mut Context<Self>) {
        let h = f32::from(self.sizes().size.height).max(1.0) as f64;
        if self.studio.read(cx).is_3d(cx) {
            self.studio.update(cx, |s, cx| s.navigate(Nav::Pan(dx / h, dy / h), cx));
        } else {
            let fit_zoom = self.canvas_zoom(cx);
            self.studio.update(cx, |s, cx| {
                if s.canvas.fit {
                    s.canvas = super::Canvas2d { zoom: fit_zoom.unwrap_or(s.canvas.zoom), pan: [0.0, 0.0], fit: false };
                }
                s.canvas.pan = [s.canvas.pan[0] + dx, s.canvas.pan[1] + dy];
                cx.notify();
            });
        }
        self.interacting(cx);
    }

    /// Zooms by `k` (> 1 closer) towards a point of the screen: the 3D view heads for what is
    /// under it, the 2D canvas keeps it under the pointer.
    fn zoom_at(&mut self, k: f64, m: [f64; 2], cx: &mut Context<Self>) {
        let Some((p, _, scene, t)) = self.scene(cx) else { return };
        match &scene {
            Scene::Space(s) => {
                let v = self.view3(s, t, &p, cx);
                let at = (!self.studio.read(cx).through_camera).then(|| v.cam.point_under(v.w, v.h, m[0] - v.x, m[1] - v.y));
                self.studio.update(cx, |s, cx| s.navigate(Nav::Zoom(1.0 / k, at), cx));
            }
            Scene::Flat(s) => {
                let v = self.view2(s, &p, cx);
                let under = v.to_canvas(m);
                let zoom = (v.zoom * k).clamp(0.02, 32.0);
                let b = self.sizes();
                let centre = [f32::from(b.origin.x) as f64 + f32::from(b.size.width) as f64 / 2.0, f32::from(b.origin.y) as f64 + f32::from(b.size.height) as f64 / 2.0];
                let pan = [m[0] - under[0] * zoom - centre[0], m[1] - under[1] * zoom - centre[1]];
                self.studio.update(cx, |s, cx| {
                    s.canvas = super::Canvas2d { zoom, pan, fit: false };
                    cx.notify();
                });
            }
        }
        self.interacting(cx);
    }

    /// The wheel and two-finger scrolling. 3D: a trackpad orbits (like Blender on a Mac), a
    /// mouse wheel zooms to the pointer; Shift pans, Ctrl/Cmd zooms. 2D: a trackpad pans, a
    /// wheel zooms to the pointer; Shift pans sideways, Ctrl/Cmd zooms. Flying: the speed.
    fn scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(px(16.));
        let (dx, dy) = (f32::from(d.x) as f64, f32::from(d.y) as f64);
        if dx.abs() < 0.01 && dy.abs() < 0.01 {
            return;
        }
        if let Some(f) = self.fly.as_mut() {
            f.speed = (f.speed * (1.0 + dy * 0.01).clamp(0.5, 2.0)).clamp(0.01, 1000.0);
            cx.notify();
            return;
        }
        let m = [f32::from(e.position.x) as f64, f32::from(e.position.y) as f64];
        let three = self.studio.read(cx).is_3d(cx);
        let trackpad = e.delta.precise();
        let zoom = e.modifiers.control || e.modifiers.platform;
        // A burst of scrolling that moves a locked camera is one undo step.
        if e.touch_phase == gpui::TouchPhase::Started {
            self.studio.update(cx, |s, _| s.lock = None);
        }
        if zoom {
            self.zoom_at((dy * 0.004).exp(), m, cx);
        } else if e.modifiers.shift {
            // A wheel scrolls one way: Shift+wheel moves sideways on the 2D canvas, up/down in 3D.
            if !three && !trackpad { self.pan_by(dy, 0.0, cx) } else { self.pan_by(dx, dy, cx) }
        } else if trackpad {
            if three {
                self.studio.update(cx, |s, cx| s.navigate(Nav::Orbit(dx * 0.3, dy * 0.3), cx));
                self.interacting(cx);
            } else {
                self.pan_by(dx, dy, cx);
            }
        } else {
            self.zoom_at((dy * 0.002).exp(), m, cx);
        }
    }

    /// A trackpad pinch: zooms where the fingers are.
    fn pinch(&mut self, e: &gpui::PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = [f32::from(e.position.x) as f64, f32::from(e.position.y) as f64];
        if e.phase == gpui::TouchPhase::Started {
            self.studio.update(cx, |s, _| s.lock = None);
        }
        self.zoom_at((1.0 + e.delta as f64).clamp(0.5, 2.0), m, cx);
    }

    /// Right-click: what can be done with the selection here.
    fn context_menu(&mut self, at: Point<Pixels>, _: &mut Window, cx: &mut Context<Self>) {
        let studio = self.studio.clone();
        let st = studio.read(cx);
        let entries = if st.mode == Mode::Edit { super::menus::mesh_menu(&studio, cx) } else { super::menus::thing_menu(&studio, st.selection.clone(), cx) };
        let mut entries = entries;
        if st.mode == Mode::Object {
            let s2 = studio.clone();
            entries.insert(0, MenuItem::new("Add…", move |w, cx| s2.update(cx, |s, cx| s.open_add_menu(Some(at), w, cx))).icon("plus").shortcut_of(&crate::actions::StudioAdd).entry());
            entries.insert(1, MenuEntry::Separator);
        }
        self.store.update(cx, |s, cx| s.open_menu(at, entries, cx));
    }
}

/// Click picking through the 3D engine's `pick` when it has one, or by boxes.
fn engine_pick(s: &Scene3d, _w: &std::collections::HashMap<String, math::M4>, t: f64, o: V3, d: V3) -> Option<String> {
    kimchi_media::render::space::viewport::pick(s, t, (o, d))
}

#[derive(Clone, Copy, Debug)]
enum MeshPick {
    Vertex(u32),
    Edge(u32, u32),
    Face(u32),
}

fn toggle<T: PartialEq>(list: &mut Vec<T>, v: T, additive: bool) {
    match list.iter().position(|x| *x == v) {
        Some(i) if additive => {
            list.remove(i);
        }
        Some(_) => {}
        None => list.push(v),
    }
}

/// The mesh edge nearest the pointer on screen (within 8 px).
fn nearest_edge(verts: &[V3], faces: &[Vec<u32>], view: &View3, m: [f64; 2]) -> Option<(u32, u32)> {
    let mut best: Option<(f64, (u32, u32))> = None;
    for (a, b) in model::mesh_edges(faces) {
        let (Some(pa), Some(pb)) = (verts.get(a as usize).and_then(|v| view.project(*v)), verts.get(b as usize).and_then(|v| view.project(*v))) else { continue };
        let d = math::seg_dist(m, pa, pb);
        if d < 8.0 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, (a, b)));
        }
    }
    best.map(|(_, e)| e)
}

/// SVG path data through pen points (curves where a handle was pulled).
fn path_data(pts: &[PenPoint], closed: bool, map: impl Fn([f64; 2]) -> [f64; 2]) -> String {
    let f = |p: [f64; 2]| {
        let q = map(p);
        format!("{} {}", round2(q[0]), round2(q[1]))
    };
    let mut d = format!("M{}", f(pts[0].p));
    let seg = |a: &PenPoint, b: &PenPoint| -> String {
        if a.handle == [0.0, 0.0] && b.handle == [0.0, 0.0] {
            format!(" L{}", f(b.p))
        } else {
            let c1 = [a.p[0] + a.handle[0], a.p[1] + a.handle[1]];
            let c2 = [b.p[0] - b.handle[0], b.p[1] - b.handle[1]];
            format!(" C{} {} {}", f(c1), f(c2), f(b.p))
        }
    };
    for w in pts.windows(2) {
        d.push_str(&seg(&w[0], &w[1]));
    }
    if closed {
        d.push_str(&seg(&pts[pts.len() - 1], &pts[0]));
        d.push_str(" Z");
    }
    d
}

/// The axis view seen from the other side.
fn opposite(name: &'static str) -> &'static str {
    match name {
        "front" => "back",
        "back" => "front",
        "left" => "right",
        "right" => "left",
        "top" => "bottom",
        "bottom" => "top",
        other => other,
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

thread_local! {
    /// Shift during the current pointer gesture (constrains and keeps proportions).
    static SHIFT: Cell<bool> = const { Cell::new(false) };
    #[allow(dead_code)]
    pub static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The selected layer's box on screen: its corners, its handles, its anchor point.
type Handles2 = (Vec<[f64; 2]>, Vec<(LayerOp, [f64; 2])>, [f64; 2]);

/// The 2D canvas on screen.
#[derive(Clone, Copy, Debug)]
struct View2 {
    cx: f64,
    cy: f64,
    zoom: f64,
    canvas: (f64, f64),
    project: (f64, f64),
}

impl View2 {
    fn to_screen(self, p: [f64; 2]) -> [f64; 2] {
        [self.cx + p[0] * self.zoom, self.cy + p[1] * self.zoom]
    }
    fn to_canvas(self, s: [f64; 2]) -> [f64; 2] {
        [(s[0] - self.cx) / self.zoom, (s[1] - self.cy) / self.zoom]
    }
}

/// Renders one picture of the scene (on a background thread).
fn draw(tools: &kimchi_media::Tools, project: &Project, req: &Request, fps: f64) -> Result<Arc<RenderImage>, String> {
    let mut r = kimchi_media::render::Renderer::new(tools, project, req.w, req.h, fps);
    let pix = r.scene_view(req.clip, req.t, req.view.as_ref(), &req.opts, req.comp.as_deref()).map_err(|e| e.to_string())?;
    to_image(pix)
}

/// A rendered picture (premultiplied RGBA) as a GPUI image.
fn to_image(pix: kimchi_media::tiny_skia::Pixmap) -> Result<Arc<RenderImage>, String> {
    let (w, h) = (pix.width(), pix.height());
    let mut data = pix.take();
    for px in data.as_chunks_mut::<4>().0 {
        // Premultiplied → straight, then BGRA for GPUI.
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in px.iter_mut().take(3) {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
        px.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(w, h, data).ok_or("bad picture size")?;
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)])))
}

// ---- drawing ------------------------------------------------------------------------------------

/// Something drawn over the picture.
enum Mark {
    Line(Vec<[f64; 2]>, bool, Hsla, f32),
    Fill(Vec<[f64; 2]>, Hsla),
    Dot([f64; 2], f64, Hsla),
}

fn paint_marks(marks: Vec<Mark>, window: &mut Window) {
    let pt = |p: [f64; 2]| point(px(p[0] as f32), px(p[1] as f32));
    for m in marks {
        match m {
            Mark::Line(pts, closed, color, width) => {
                if pts.len() < 2 {
                    continue;
                }
                let mut b = PathBuilder::stroke(px(width));
                b.move_to(pt(pts[0]));
                for p in &pts[1..] {
                    b.line_to(pt(*p));
                }
                if closed {
                    b.close();
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, color);
                }
            }
            Mark::Fill(pts, color) => {
                if pts.len() < 3 {
                    continue;
                }
                let mut b = PathBuilder::fill();
                b.move_to(pt(pts[0]));
                for p in &pts[1..] {
                    b.line_to(pt(*p));
                }
                b.close();
                if let Ok(path) = b.build() {
                    window.paint_path(path, color);
                }
            }
            Mark::Dot(c, r, color) => {
                let pts: Vec<[f64; 2]> = (0..16).map(|k| {
                    let a = k as f64 / 16.0 * std::f64::consts::TAU;
                    [c[0] + a.cos() * r, c[1] + a.sin() * r]
                }).collect();
                let mut b = PathBuilder::fill();
                b.move_to(pt(pts[0]));
                for p in &pts[1..] {
                    b.line_to(pt(*p));
                }
                b.close();
                if let Ok(path) = b.build() {
                    window.paint_path(path, color);
                }
            }
        }
    }
}

fn hex(c: u32) -> Hsla {
    gpui::rgb(c).into()
}

impl Viewport {
    /// The marks over a 3D picture: the gizmo, picked lights and cameras, the box, loop cut.
    fn marks3(&self, s: &Scene3d, scene: &Scene, t: f64, project: &Project, cx: &App) -> Vec<Mark> {
        let t2 = cx.theme();
        let mut out = vec![];
        let view = self.view3(s, t, project, cx);
        if let Some(frame) = self.gizmo_frame(s, scene, t, cx) {
            let kind = self.gizmo_kind(cx);
            let active = self.session.as_ref().map(|s| s.handle);
            for part in gizmo::parts(&frame, kind, &view) {
                let lit = self.hover == Some(part.handle) || active == Some(part.handle);
                let mut color = hex(part.color);
                if part.color == 0xffffff {
                    color = color.opacity(0.7);
                }
                if lit {
                    color = hex(0xffd84d);
                }
                if part.fill {
                    out.push(Mark::Fill(part.points, color.opacity(if lit { 0.95 } else { 0.75 })));
                } else {
                    out.push(Mark::Line(part.points, part.closed, color, if lit { 3.0 } else { 2.0 }));
                }
            }
            // While moving along an axis, the axis line across the view.
            if let Some(sess) = &self.session
                && let Handle::Axis(i) = sess.handle
                && sess.kind == Kind::Grab
            {
                let a = sess.frame.axes[i];
                let far = 1000.0;
                let pts: Vec<[f64; 2]> = (-20..=20).filter_map(|k| view.project(math::add(sess.frame.pivot, math::scale(a, k as f64 * far / 20.0)))).collect();
                out.push(Mark::Line(pts, false, hex(gizmo::AXIS_COLORS[i]).opacity(0.5), 1.0));
            }
        }
        // Selected lights and cameras: a ring around their icon.
        let w = model::worlds(s, t);
        let st = self.studio.read(cx);
        for id in &st.selection {
            if matches!(model::item(scene, id), Some(model::Item::Light | model::Item::Camera))
                && let Some(q) = w.get(id).and_then(|m| view.project(math::origin(m)))
            {
                let ring: Vec<[f64; 2]> = (0..20).map(|k| {
                    let a = k as f64 / 20.0 * std::f64::consts::TAU;
                    [q[0] + a.cos() * 13.0, q[1] + a.sin() * 13.0]
                }).collect();
                out.push(Mark::Line(ring, true, t2.accent, 2.0));
            }
        }
        if let (Some(Armed::LoopCut), Some((a, b)), Some((v, _, view))) = (self.armed, self.hover_edge, self.mesh(cx))
            && let (Some(pa), Some(pb)) = (view.project(v[a as usize]), view.project(v[b as usize]))
        {
            out.push(Mark::Line(vec![pa, pb], false, hex(0xffd84d), 3.0));
        }
        // The selected camera's path over the clip, its keyframes as dots.
        if let Some((_, _, path, keys)) = &self.cam_path {
            let col = t2.accent;
            let mut run: Vec<[f64; 2]> = vec![];
            for p in path {
                match view.project(*p) {
                    Some(q) => run.push(q),
                    None => {
                        if run.len() > 1 {
                            out.push(Mark::Line(std::mem::take(&mut run), false, col.opacity(0.85), 1.5));
                        }
                        run.clear();
                    }
                }
            }
            if run.len() > 1 {
                out.push(Mark::Line(run, false, col.opacity(0.85), 1.5));
            }
            for k in keys {
                if let Some(q) = view.project(*k) {
                    out.push(Mark::Dot(q, 4.5, col));
                    out.push(Mark::Dot(q, 2.5, gpui::white()));
                }
            }
        }
        if through_camera_frame(st.through_camera) {
            out.push(Mark::Line(vec![[view.x, view.y], [view.x + view.w, view.y], [view.x + view.w, view.y + view.h], [view.x, view.y + view.h]], true, t2.accent.opacity(0.8), 1.5));
        }
        out
    }

    fn marks2(&self, s: &Scene2d, t: f64, project: &Project, cx: &App) -> Vec<Mark> {
        let th = cx.theme();
        let mut out = vec![];
        let v = self.view2(s, project, cx);
        // The canvas's edge (and a composition's).
        let (w, h) = v.canvas;
        let edge = [[-w / 2.0, -h / 2.0], [w / 2.0, -h / 2.0], [w / 2.0, h / 2.0], [-w / 2.0, h / 2.0]].map(|p| v.to_screen(p));
        out.push(Mark::Line(edge.to_vec(), true, th.line_strong, 1.0));
        let st = self.studio.read(cx);
        // Every other selected layer: its outline.
        for id in st.selection.iter().rev().skip(1) {
            if let Some(c) = model::layer_corners(s, st.composition.as_deref(), t, id, v.project) {
                out.push(Mark::Line(c.iter().map(|q| v.to_screen(*q)).collect(), true, th.accent.opacity(0.7), 1.0));
            }
        }
        if let Some((corners, handles, anchor)) = self.handles2(s, &v, t, cx) {
            out.push(Mark::Line(corners, true, th.accent, 1.5));
            if matches!(st.tool, Tool::Select | Tool::Anchor) {
                for (_, hp) in &handles {
                    let sq = |r: f64| vec![[hp[0] - r, hp[1] - r], [hp[0] + r, hp[1] - r], [hp[0] + r, hp[1] + r], [hp[0] - r, hp[1] + r]];
                    out.push(Mark::Fill(sq(4.5), th.accent));
                    out.push(Mark::Fill(sq(3.0), gpui::white()));
                }
            }
            // The anchor point: a crosshair in a circle.
            let r = 7.0;
            out.push(Mark::Line((0..20).map(|k| {
                let a = k as f64 / 20.0 * std::f64::consts::TAU;
                [anchor[0] + a.cos() * r, anchor[1] + a.sin() * r]
            }).collect(), true, th.accent, 1.5));
            out.push(Mark::Line(vec![[anchor[0] - 11.0, anchor[1]], [anchor[0] + 11.0, anchor[1]]], false, th.accent, 1.0));
            out.push(Mark::Line(vec![[anchor[0], anchor[1] - 11.0], [anchor[0], anchor[1] + 11.0]], false, th.accent, 1.0));
        }
        // The pen's path so far, and where the next segment would go.
        if !self.pen.is_empty() {
            let mut pts = vec![];
            for wnd in self.pen.windows(2) {
                let (a, b) = (wnd[0], wnd[1]);
                let c1 = [a.p[0] + a.handle[0], a.p[1] + a.handle[1]];
                let c2 = [b.p[0] - b.handle[0], b.p[1] - b.handle[1]];
                for k in 0..=16 {
                    let u = k as f64 / 16.0;
                    let q = cubic(a.p, c1, c2, b.p, u);
                    pts.push(v.to_screen(q));
                }
            }
            if let Some(last) = self.pen.last() {
                if pts.is_empty() {
                    pts.push(v.to_screen(last.p));
                }
                pts.push(self.mouse);
            }
            out.push(Mark::Line(pts, false, th.accent, 2.0));
            for (i, p) in self.pen.iter().enumerate() {
                let q = v.to_screen(p.p);
                out.push(Mark::Dot(q, if i == 0 { 5.0 } else { 3.5 }, if i == 0 { th.accent } else { gpui::white() }));
                if p.handle != [0.0, 0.0] {
                    let h1 = v.to_screen([p.p[0] + p.handle[0], p.p[1] + p.handle[1]]);
                    let h0 = v.to_screen([p.p[0] - p.handle[0], p.p[1] - p.handle[1]]);
                    out.push(Mark::Line(vec![h0, h1], false, th.accent.opacity(0.7), 1.0));
                    out.push(Mark::Dot(h0, 2.5, th.accent));
                    out.push(Mark::Dot(h1, 2.5, th.accent));
                }
            }
        }
        if let Some(Drag::Shape { from, to, square }) = &self.drag {
            let (mut a, mut b) = (*from, *to);
            if *square {
                let side = (b[0] - a[0]).abs().max((b[1] - a[1]).abs());
                b = [a[0] + side * (b[0] - a[0]).signum(), a[1] + side * (b[1] - a[1]).signum()];
            }
            if a[0] > b[0] {
                std::mem::swap(&mut a[0], &mut b[0]);
            }
            let r = [a, [b[0], a[1]], b, [a[0], b[1]]].map(|p| v.to_screen(p));
            out.push(Mark::Line(r.to_vec(), true, th.accent, 1.5));
        }
        out
    }
}

fn through_camera_frame(on: bool) -> bool {
    on
}

fn cubic(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], u: f64) -> [f64; 2] {
    let v = 1.0 - u;
    let k = [v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u];
    [k[0] * a[0] + k[1] * b[0] + k[2] * c[0] + k[3] * d[0], k[0] * a[1] + k[1] * b[1] + k[2] * c[1] + k[3] * d[1]]
}

impl Render for Viewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for old in self.garbage.drain(..) {
            let _ = window.drop_image(old);
        }
        self.scale = window.scale_factor().clamp(1.0, 3.0);
        self.update_camera_path(cx);
        let t = cx.theme().clone();
        let bounds = self.bounds.clone();
        let entity = cx.entity();
        let scene = self.scene(cx);
        let (marks, picture_box, label) = match &scene {
            Some((p, _, sc @ Scene::Space(s), tt)) => {
                let v = self.view3(s, *tt, p, cx);
                let b = self.sizes();
                let (bx, by) = (f32::from(b.origin.x) as f64, f32::from(b.origin.y) as f64);
                let st = self.studio.read(cx);
                let mut label = if st.through_camera {
                    format!("Camera · {}{}", s.active_camera_at(*tt), if st.lock_camera { " · locked to the view" } else { "" })
                } else {
                    format!("User {}", if st.view.ortho { "orthographic" } else { "perspective" })
                };
                if let (Some((n, of)), true) = (self.samples, self.refiner.is_some()) {
                    label.push_str(&format!(" · path traced {n}/{of}"));
                }
                (self.marks3(s, sc, *tt, p, cx), Some((v.x - bx, v.y - by, v.w, v.h)), label)
            }
            Some((p, _, Scene::Flat(s), tt)) => {
                let v = self.view2(s, p, cx);
                let b = self.sizes();
                let (bx, by) = (f32::from(b.origin.x) as f64, f32::from(b.origin.y) as f64);
                let (pw, ph) = v.project;
                let tl = v.to_screen([-pw / 2.0, -ph / 2.0]);
                let st = self.studio.read(cx);
                let label = match &st.composition {
                    Some(c) => format!("Composition · {c} · {:.0}%", v.zoom * 100.0),
                    None => format!("Canvas · {:.0}%", v.zoom * 100.0),
                };
                (self.marks2(s, *tt, p, cx), Some((tl[0] - bx, tl[1] - by, pw * v.zoom, ph * v.zoom)), label)
            }
            None => (vec![], None, String::new()),
        };
        let three = matches!(scene, Some((_, _, Scene::Space(_), _)));
        let status = self.status_line(cx);
        let busy_drag = self.drag.is_some() || self.modal_key.is_some() || self.mesh_modal.is_some() || !self.pen.is_empty();
        let cursor = match (&self.armed, self.studio.read(cx).tool, three) {
            (Some(_), _, _) => gpui::CursorStyle::Crosshair,
            (_, Tool::Pen | Tool::Rect | Tool::Ellipse | Tool::Star | Tool::Polygon, false) => gpui::CursorStyle::Crosshair,
            (_, Tool::Text, false) => gpui::CursorStyle::IBeam,
            _ => gpui::CursorStyle::Arrow,
        };
        let canvas_zoom = if three { None } else { self.canvas_zoom(cx) };
        let axis = match canvas_zoom {
            None if three => self.nav_widget(cx).into_any_element(),
            Some(z) => self.canvas_controls(z, cx).into_any_element(),
            None => div().into_any_element(),
        };
        let box_rect = match &self.drag {
            Some(Drag::Press { start, now, boxing: true, .. }) => Some((*start, *now)),
            _ => None,
        };
        div()
            .id("studio-viewport")
            .size_full()
            .relative()
            .overflow_hidden()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            // A release right after the press (before the window-wide tracker exists) ends the drag too.
            .on_mouse_up(MouseButton::Left, cx.listener(Self::drag_end))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::drag_end))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::drag_end))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_pinch(cx.listener(Self::pinch))
            .child(
                canvas(
                    move |b, _, cx| {
                        if bounds.get() != b {
                            bounds.set(b);
                            entity.update(cx, |this, cx| this.refresh(cx));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when_some(picture_box, |d, (x, y, w, h)| {
                let mut frame = div().absolute().left(px(x as f32)).top(px(y as f32)).w(px(w as f32)).h(px(h as f32));
                if !three {
                    // A checkerboard-free canvas: the project's background shows under a transparent scene.
                    let bg = self.store.read(cx).project.as_ref().map(|p| crate::theme::parse_color(&p.settings.background)).unwrap_or(t.bg_sunken);
                    frame = frame.bg(bg).shadow(t.glass_shadow());
                }
                d.child(frame.when_some(self.image.clone(), |f, img_| f.child(img(img_).size_full().object_fit(ObjectFit::Fill))))
            })
            .child(canvas(|_, _, _| (), move |_, _, window, _| paint_marks(marks, window)).absolute().size_full())
            .when_some(box_rect, |d, (a, b)| {
                let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
                let (w, h) = ((a.x - b.x).abs(), (a.y - b.y).abs());
                let o = self.sizes().origin;
                d.child(div().absolute().left(x0 - o.x).top(y0 - o.y).w(w).h(h).border_1().border_dashed().border_color(t.accent).bg(t.accent_soft.opacity(0.4)))
            })
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(10.))
                    .px(px(7.))
                    .py(px(2.))
                    .rounded(px(sz::R_XS))
                    .bg(gpui::black().opacity(0.45))
                    .text_size(px(sz::XS))
                    .font_family(MONO)
                    .text_color(gpui::white().opacity(0.85))
                    .child(label),
            )
            .child(axis)
            .when_some(status, |d, s| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(8.))
                        .left(px(10.))
                        .right(px(10.))
                        .flex()
                        .justify_center()
                        .child(div().px(px(10.)).py(px(4.)).rounded(px(sz::R_SM)).bg(gpui::black().opacity(0.6)).text_size(px(sz::XS)).text_color(gpui::white()).child(s)),
                )
            })
            .when_some(self.error.clone(), |d, e| d.child(div().absolute().top(px(32.)).left(px(10.)).right(px(10.)).text_size(px(sz::XS)).text_color(t.danger).child(format!("Couldn't draw the scene: {e}"))))
            .when(busy_drag && self.drag.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_end)))
            .when(self.modal_key.is_some() && self.drag.is_none(), |d| d.child(drag::track(cx.entity(), Self::mouse_move, |_, _, _, _| {})))
    }
}

impl Viewport {
    /// Keeps the selected camera's path (positions over the clip, and where its keyframes are)
    /// for the overlay.
    fn update_camera_path(&mut self, cx: &App) {
        let Some((p, c, Scene::Space(s), _)) = self.scene(cx) else {
            self.cam_path = None;
            return;
        };
        let st = self.studio.read(cx);
        let scene = Scene::Space(s.clone());
        let Some(id) = st.selection.iter().rev().find(|k| matches!(model::item(&scene, k), Some(model::Item::Camera))).cloned() else {
            self.cam_path = None;
            return;
        };
        let key = Arc::as_ptr(&p) as usize;
        if self.cam_path.as_ref().is_some_and(|(k, i, ..)| *k == key && *i == id) {
            return;
        }
        let (a, b) = (model::scene_time(&c, c.start), model::scene_time(&c, c.end()));
        let (lo, hi) = (a.min(b), a.max(b));
        const N: usize = 96;
        let path: Vec<V3> = (0..=N).filter_map(|i| s.camera_by_id_at(&id, lo + (hi - lo) * i as f64 / N as f64).map(|c| c.position.0)).collect();
        // Still: no path to draw.
        let moves = path.windows(2).any(|w| math::len(math::sub(w[0], w[1])) > 1e-6);
        let mut times: Vec<f64> = s.camera_by_id(&id).map(|cam| cam.keyframes.iter().filter(|(n, _)| n.starts_with("position") || n.ends_with(".progress")).flat_map(|(_, l)| l.iter().map(|k| k.time)).collect()).unwrap_or_default();
        times.sort_by(f64::total_cmp);
        times.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
        let keys: Vec<V3> = times.into_iter().filter_map(|t| s.camera_by_id_at(&id, t).map(|c| c.position.0)).collect();
        self.cam_path = Some((key, id, if moves { path } else { vec![] }, if moves { keys } else { vec![] }));
    }

    fn status_line(&self, cx: &App) -> Option<String> {
        if let Some(f) = &self.fly {
            let how = if f.hold { "let go of the right button to stop" } else { "click or Enter to keep · Esc or right-click to go back" };
            return Some(format!("Flying · W A S D move · Q E down and up · Shift faster · the mouse looks · the wheel sets the speed ({:.1} units/s) · {how}", f.speed));
        }
        if let Some(s) = &self.session
            && self.modal_key.is_some()
        {
            return Some(s.status());
        }
        if let Some(m) = &self.mesh_modal {
            let what = match m.kind {
                ModalKind::Extrude if m.moving => "Move along the normal",
                ModalKind::Extrude => "Extrude",
                ModalKind::Inset => "Inset",
                ModalKind::Bevel => "Bevel",
                _ => "",
            };
            let typed = if m.typed.is_empty() { String::new() } else { format!(" [{}]", m.typed) };
            return Some(format!("{what}: {:.3}{typed} · move the mouse or type a number · Enter or click to confirm · Esc to cancel", m.amount));
        }
        if !self.pen.is_empty() {
            return Some("Pen: click for corners, drag for curves · click the first point to close · Enter to finish · Backspace removes the last point".into());
        }
        match self.armed {
            Some(Armed::BoxSelect) => return Some("Box select: drag a rectangle (Shift adds)".into()),
            Some(Armed::LoopCut) => return Some("Loop cut: click an edge to cut across its ring".into()),
            None => {}
        }
        if let Some(Drag::Layer(d)) = &self.drag
            && let Some((_, p)) = d.reached.first()
        {
            let s: Vec<String> = p.iter().map(|(k, v)| format!("{k} {v}")).collect();
            return Some(s.join(" · "));
        }
        let st = self.studio.read(cx);
        if st.mode == Mode::Edit {
            return Some(format!("Edit mode · {} · 1/2/3 vertex/edge/face · E extrude · I inset · Ctrl+B bevel · Ctrl+R loop cut · right-click for more · Tab to leave", st.select_mode.name()));
        }
        None
    }

    /// The navigation gizmo in the corner, like Blender's: the axis ball (click an axis to look
    /// along it, click it again for the other side, drag the ball to orbit), and under it
    /// buttons to drag (orbit, pan, zoom) and toggles (fly, through the camera, camera locked
    /// to the view, perspective / orthographic, frame).
    fn nav_widget(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::actions::{self as act, tip};
        let t = cx.theme().clone();
        let st = self.studio.read(cx);
        // The ball turns with what is seen (the camera's view when looking through it).
        let cam = if st.through_camera { st.camera_view(cx).unwrap_or(st.view) } else { st.view };
        let (through, lock, ortho, flying) = (st.view_shown().1, st.lock_camera, cam.ortho, self.fly.is_some());
        let f = math::norm(math::sub(cam.target, cam.position));
        let up0 = if f[1].abs() > 0.999 { [0.0, 0.0, -1.0] } else { [0.0, 1.0, 0.0] };
        let r = math::norm(math::cross(f, up0));
        let u = math::cross(r, f);
        let (c, k) = (42.0, 29.0);
        let mut dots: Vec<(usize, bool, f64, f64, f64)> = vec![];
        for i in 0..3 {
            for neg in [false, true] {
                let mut e = [0.0; 3];
                e[i] = if neg { -1.0 } else { 1.0 };
                dots.push((i, neg, c + math::dot(e, r) * k, c - math::dot(e, u) * k, math::dot(e, f)));
            }
        }
        // Far ones first, so near ones are on top.
        dots.sort_by(|a, b| b.4.total_cmp(&a.4));
        let lines: Vec<Mark> = dots.iter().filter(|d| !d.1).map(|d| Mark::Line(vec![[c, c], [d.2, d.3]], false, hex(gizmo::AXIS_COLORS[d.0]), 2.0)).collect();
        let names = [("right", "left"), ("top", "bottom"), ("front", "back")];
        let ball_hover = t.hover;
        let ball = div()
            .id("nav-ball")
            .relative()
            .size(px(84.))
            .rounded_full()
            .bg(gpui::black().opacity(0.18))
            .hover(move |s| s.bg(ball_hover))
            .cursor(gpui::CursorStyle::OpenHand)
            .tooltip(|_, cx| crate::ui::tooltip("Drag to orbit · click an axis to look along it (again: from the other side)".into(), cx))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.begin_nav(Drag::Ball { start: e.position, last: e.position, moved: false, axis: None }, cx);
            }))
            .child(canvas(|_, _, _| (), move |b, _, window, _| {
                let o = [f32::from(b.origin.x) as f64, f32::from(b.origin.y) as f64];
                let shifted = lines
                    .into_iter()
                    .map(|m| match m {
                        Mark::Line(p, c, col, w) => Mark::Line(p.into_iter().map(|q| [q[0] + o[0], q[1] + o[1]]).collect(), c, col, w),
                        other => other,
                    })
                    .collect();
                paint_marks(shifted, window)
            }).absolute().size_full())
            .children(dots.into_iter().map(|(i, neg, x, y, _)| {
                let name = if neg { names[i].1 } else { names[i].0 };
                let col = hex(gizmo::AXIS_COLORS[i]);
                let size = if neg { 11. } else { 17. };
                div()
                    .id(("axis-dot", i * 2 + neg as usize))
                    .absolute()
                    .left(px(x as f32 - size / 2.))
                    .top(px(y as f32 - size / 2.))
                    .size(px(size))
                    .rounded_full()
                    .bg(if neg { col.opacity(0.45) } else { col })
                    .hover(|s| s.border_2().border_color(gpui::white()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(gpui::black())
                    .cursor_pointer()
                    .when(!neg, |d| d.child(["X", "Y", "Z"][i]))
                    .tooltip(move |_, cx| crate::ui::tooltip(format!("Look from the {name}").into(), cx))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.begin_nav(Drag::Ball { start: e.position, last: e.position, moved: false, axis: Some(name) }, cx);
                    }))
            }));
        // A round button in the column.
        let button = |id: &'static str, ic: &'static str, on: bool, tooltip: gpui::SharedString| {
            let (hover, accent, fg, on_fg) = (t.hover, t.accent, t.text, t.text_on_accent);
            div()
                .id(id)
                .size(px(30.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(if on { on_fg } else { fg })
                .when(on, |d| d.bg(accent))
                .when(!on, move |d| d.hover(move |s| s.bg(hover)))
                .child(icon(ic).size(px(15.)))
                .tooltip(move |_, cx| crate::ui::tooltip(tooltip.clone(), cx))
        };
        let drag_button = |id: &'static str, ic: &'static str, tooltip: gpui::SharedString, kind: fn(Point<Pixels>) -> Drag, cx: &mut Context<Self>| {
            button(id, ic, false, tooltip).cursor(gpui::CursorStyle::OpenHand).on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.begin_nav(kind(e.position), cx);
            }))
        };
        let studio = self.studio.clone();
        let sep = div().mx(px(7.)).my(px(2.)).h(px(1.)).bg(t.line);
        let column = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(18.))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(drag_button("nav-orbit", "orbit", "Orbit: drag here · in the view: middle-drag or Alt+drag, or scroll with two fingers on a trackpad".into(), |p| Drag::Orbit { last: p }, cx))
            .child(drag_button("nav-pan", "hand", "Pan: drag here · in the view: Shift+middle-drag, Shift+scroll, or Space+drag".into(), |p| Drag::Pan { last: p }, cx))
            .child(drag_button("nav-zoom", "zoom-in", "Zoom: drag up and down here · the wheel zooms to the pointer, Ctrl+scroll, a pinch, + and −".into(), |p| Drag::Zoom { last: p }, cx))
            .child(button("nav-fly", "plane", flying, tip("Fly: W A S D or the arrows move, Q and E down and up, the mouse looks, the wheel sets the speed; click to keep, Esc to go back. Or hold the right button and drag", &act::StudioFly)).cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                if this.fly.is_some() {
                    this.end_fly(true, cx);
                } else {
                    this.start_fly(false, cx);
                }
            })))
            .child(sep)
            .child(button("nav-camera", "video", through, tip("Look through the camera", &act::StudioKey0)).cursor_pointer().on_click({
                let studio = studio.clone();
                move |_, _, cx| studio.update(cx, |s, cx| s.toggle_camera_view(cx))
            }))
            .child(button("nav-lock", if lock { "lock" } else { "lock-open" }, lock, "Lock the camera to the view: looking through it, moving around moves the camera (a keyframe at the playhead when it is animated)".into()).cursor_pointer().on_click({
                let studio = studio.clone();
                move |_, _, cx| studio.update(cx, |s, cx| s.set_lock_camera(!s.lock_camera, cx))
            }))
            .child(button("nav-ortho", if ortho { "square" } else { "box" }, ortho, tip(if ortho { "Orthographic (click for perspective)" } else { "Perspective (click for orthographic)" }, &act::StudioOrtho)).cursor_pointer().on_click({
                let studio = studio.clone();
                move |_, _, cx| studio.update(cx, |s, cx| s.toggle_ortho(cx))
            }))
            .child(button("nav-frame", "focus", false, tip("Frame the selection (everything when nothing is; Home frames everything)", &act::StudioFrame)).cursor_pointer().on_click({
                let studio = studio.clone();
                move |_, _, cx| studio.update(cx, |s, cx| s.frame_selection(true, cx))
            }));
        div().absolute().top(px(8.)).right(px(8.)).flex().flex_col().items_center().gap(px(8.)).child(ball).child(column)
    }

    /// The 2D canvas's zoom controls, bottom right: out, the zoom (click: 100%), in, fit.
    fn canvas_controls(&self, zoom: f64, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::actions::{self as act, tip};
        let t = cx.theme().clone();
        let studio = self.studio.clone();
        let fit = studio.read(cx).canvas.fit;
        let (hover, accent, fg, on_fg) = (t.hover, t.accent, t.text, t.text_on_accent);
        let button = move |id: &'static str, ic: &'static str, on: bool, tooltip: gpui::SharedString| {
            div()
                .id(id)
                .size(px(26.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(if on { on_fg } else { fg })
                .when(on, |d| d.bg(accent))
                .when(!on, move |d| d.hover(move |s| s.bg(hover)))
                .child(icon(ic).size(px(14.)))
                .tooltip(move |_, cx| crate::ui::tooltip(tooltip.clone(), cx))
        };
        let (s1, s2, s3, s4) = (studio.clone(), studio.clone(), studio.clone(), studio.clone());
        div()
            .absolute()
            .bottom(px(10.))
            .right(px(10.))
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(16.))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(button("canvas-out", "zoom-out", false, tip("Zoom out (the wheel, Ctrl+scroll or a pinch zoom to the pointer)", &act::StudioZoomOut)).on_click(move |_, _, cx| s1.update(cx, |s, cx| s.zoom_canvas_to(zoom * 0.8, cx))))
            .child(
                div()
                    .id("canvas-100")
                    .px(px(6.))
                    .h(px(26.))
                    .min_w(px(52.))
                    .rounded(px(13.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .font_family(MONO)
                    .text_size(px(sz::XS))
                    .text_color(fg)
                    .hover(move |s| s.bg(hover))
                    .child(format!("{:.0}%", zoom * 100.0))
                    .tooltip(|_, cx| crate::ui::tooltip(tip("Show it at 100%", &crate::actions::StudioZoom100), cx))
                    .on_click(move |_, _, cx| s2.update(cx, |s, cx| s.zoom_canvas_to(1.0, cx))),
            )
            .child(button("canvas-in", "zoom-in", false, tip("Zoom in", &act::StudioZoomIn)).on_click(move |_, _, cx| s3.update(cx, |s, cx| s.zoom_canvas_to(zoom * 1.25, cx))))
            .child(div().w(px(1.)).h(px(16.)).mx(px(2.)).bg(t.line))
            .child(button("canvas-fit", "maximize-2", fit, tip("Fit the canvas · pan with Space+drag, middle-drag or two fingers", &act::StudioFit)).on_click(move |_, _, cx| s4.update(cx, |s, cx| {
                s.canvas.fit = true;
                s.changed(cx);
            })))
    }
}
