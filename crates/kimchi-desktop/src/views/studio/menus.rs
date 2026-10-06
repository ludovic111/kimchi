//! The Studio's menus: the add menu (Shift+A: every 3D shape, light and camera, or every kind
//! of 2D layer; searchable, grouped), the add menus of stacks (modifiers, effects…, grouped
//! and searchable, from the tables in `kimchi_core::motion::stack`), the mesh delete menu, and
//! the right-click menus of things and of edit mode.

use std::rc::Rc;

use gpui::{App, AppContext as _, Context, Entity, FontWeight, MouseButton, Pixels, Point, SharedString, Subscription, Window, anchored, deferred, div, prelude::*, px};
use kimchi_core::motion::stack::{self, TypeSpec};
use kimchi_core::{MediaKind, Scene};
use serde_json::{Value, json};

use super::{Mode, Studio, model};
use crate::actions as act;
use crate::store::{MenuEntry, MenuItem, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::input::{self, InputEvent, TextInput};
use crate::ui::{GlassExt, icon, kbd, motion};

type Run = Rc<dyn Fn(&mut Window, &mut App)>;

/// One choice of a popover.
#[derive(Clone)]
pub struct Choice {
    pub group: &'static str,
    pub label: SharedString,
    pub icon: &'static str,
    pub hint: Option<SharedString>,
    pub run: Run,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PopKind {
    /// Add a thing; inside `parent` (an object or group) when given.
    Add { three: bool, parent: Option<String> },
    /// Add an item to one stack of a thing.
    Stack { id: String, field: &'static str },
    /// Edit mode X: what to delete.
    DeleteMesh,
}

/// A small searchable menu floating at a point of the Studio.
pub struct Popover {
    pub kind: PopKind,
    pub at: Point<Pixels>,
    search: Option<Entity<TextInput>>,
    pub query: String,
    _sub: Option<Subscription>,
}

impl Popover {
    fn with_search(kind: PopKind, at: Point<Pixels>, placeholder: &str, window: &mut Window, cx: &mut Context<Studio>) -> Popover {
        let ph = placeholder.to_string();
        let input = cx.new(|cx| TextInput::new(cx).placeholder(ph));
        input::focus(&input, window, cx);
        let sub = cx.subscribe_in(&input, window, |this: &mut Studio, _, e: &InputEvent, window, cx| match e {
            InputEvent::Changed(q) => {
                if let Some(p) = this.popover.as_mut() {
                    p.query = q.clone();
                }
                cx.notify();
            }
            InputEvent::Submit => {
                // Enter takes the first match.
                let me = cx.entity();
                let first = this.popover.as_ref().and_then(|p| p.choices(this, me, cx).into_iter().find(|c| p.matches(c)));
                this.popover = None;
                window.focus(&this.focus, cx);
                cx.notify();
                // After this update: the choice reads the Studio.
                if let Some(c) = first {
                    window.defer(cx, move |window, cx| (c.run)(window, cx));
                }
            }
            InputEvent::Cancel => {
                this.popover = None;
                window.focus(&this.focus, cx);
                cx.notify();
            }
            InputEvent::Blur => {}
        });
        Popover { kind, at, search: Some(input), query: String::new(), _sub: Some(sub) }
    }

    pub fn add(at: Point<Pixels>, three: bool, window: &mut Window, cx: &mut Context<Studio>) -> Popover {
        Self::with_search(PopKind::Add { three, parent: None }, at, "Add… (type to search)", window, cx)
    }

    pub fn add_child(at: Point<Pixels>, parent: String, window: &mut Window, cx: &mut Context<Studio>) -> Popover {
        Self::with_search(PopKind::Add { three: true, parent: Some(parent) }, at, "Add a child… (type to search)", window, cx)
    }

    pub fn stack(at: Point<Pixels>, id: String, field: &'static str, window: &mut Window, cx: &mut Context<Studio>) -> Popover {
        Self::with_search(PopKind::Stack { id, field }, at, "Search…", window, cx)
    }

    pub fn delete_mesh(at: Point<Pixels>) -> Popover {
        Popover { kind: PopKind::DeleteMesh, at, search: None, query: String::new(), _sub: None }
    }

    fn matches(&self, c: &Choice) -> bool {
        let q = self.query.trim().to_lowercase();
        q.is_empty() || c.label.to_lowercase().contains(&q) || c.group.to_lowercase().contains(&q)
    }

    /// Every choice, before the search.
    pub fn choices(&self, st: &Studio, studio: Entity<Studio>, cx: &App) -> Vec<Choice> {
        match &self.kind {
            PopKind::Add { three: true, parent } => add_3d(st, &studio, parent.clone(), cx),
            PopKind::Add { three: false, .. } => add_2d(st, &studio, cx),
            PopKind::Stack { id, field } => stack_choices(&studio, id, field),
            PopKind::DeleteMesh => delete_choices(&studio),
        }
    }

    pub fn render(&self, st: &Studio, studio: Entity<Studio>, _window: &mut Window, cx: &mut Context<Studio>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        let choices: Vec<Choice> = self.choices(st, studio.clone(), cx).into_iter().filter(|c| self.matches(c)).collect();
        let mut list = div().id("studio-popover-list").flex().flex_col().max_h(px(420.)).overflow_y_scroll();
        let mut group = "";
        for (i, c) in choices.into_iter().enumerate() {
            if c.group != group {
                group = c.group;
                list = list.child(div().px(px(8.)).pt(px(6.)).pb(px(2.)).child(crate::ui::caps(group, cx)));
            }
            let run = c.run.clone();
            let studio = studio.clone();
            list = list.child(
                div()
                    .id(("studio-choice", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(sz::R_SM))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.accent_soft))
                    .child(icon(c.icon).text_color(t.text_2))
                    .child(div().flex_1().text_size(px(sz::BASE)).child(c.label.clone()))
                    .when_some(c.hint.clone(), |d, h| d.child(kbd(h, cx)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        studio.update(cx, |s, cx| {
                            s.popover = None;
                            window.focus(&s.focus, cx);
                            cx.notify();
                        });
                        run(window, cx);
                    }),
            );
        }
        let title = match &self.kind {
            PopKind::Add { parent: Some(p), .. } => format!("Add inside {p}"),
            PopKind::Add { .. } => "Add".to_string(),
            PopKind::Stack { field, .. } => format!("Add to {field}"),
            PopKind::DeleteMesh => "Delete".to_string(),
        };
        let studio2 = studio.clone();
        deferred(
            anchored().position(self.at).snap_to_window_with_margin(px(8.)).child(motion::enter(
                div()
                    .id("studio-popover")
                    .occlude()
                    .relative()
                    .w(px(260.))
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .rounded(px(sz::R_MD))
                    .glass(t.glass2)
                    .shadow(t.glass_shadow())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down_out(move |_, _, cx| {
                        studio2.update(cx, |s, cx| {
                            s.popover = None;
                            cx.notify();
                        })
                    })
                    .child(div().px(px(4.)).text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child(title))
                    .when_some(self.search.clone(), |d, s| d.child(s))
                    .child(list),
                "studio-popover-in",
                motion::FAST,
                (0., -4.),
            )),
        )
        .with_priority(2)
        .into_any_element()
    }
}

fn choice(group: &'static str, label: impl Into<SharedString>, icon: &'static str, run: impl Fn(&mut Window, &mut App) + 'static) -> Choice {
    Choice { group, label: label.into(), icon, hint: None, run: Rc::new(run) }
}

/// Adds `layer` (an object, light, camera or 2D layer, without its id) under a fresh id from
/// `stem`, then selects it.
pub fn add_thing(studio: &Entity<Studio>, stem: &str, mut layer: Value, parent: Option<String>, cx: &mut App) {
    let Some((_, scene)) = studio.read(cx).clip_scene(cx) else { return };
    let id = model::fresh_id(&scene, stem);
    layer["id"] = json!(id);
    let clip = studio.read(cx).clip;
    let mut p = json!({ "clipId": clip, "layer": layer });
    if let Some(parent) = parent.filter(|p| !p.is_empty()) {
        p["parent"] = json!(parent);
    }
    studio.update(cx, |s, cx| s.run_then("motion.setLayer", p, cx, move |s, _, cx| s.set_selection(vec![id], cx)));
}

fn add_3d(st: &Studio, studio: &Entity<Studio>, parent: Option<String>, cx: &App) -> Vec<Choice> {
    // New things go where the view looks (or at their parent).
    let at = if parent.is_some() { [0.0, 0.0, 0.0] } else { st.view.target.map(|v| (v * 100.0).round() / 100.0) };
    let eye = st.view.position;
    let grey = json!({ "color": "#cfcfcf", "roughness": 0.45 });
    let shapes: Vec<(&'static str, &'static str, &'static str, Value)> = vec![
        ("Mesh", "Box", "box", json!({ "type": "box", "size": 1 })),
        ("Mesh", "Sphere", "sphere", json!({ "type": "sphere", "radius": 0.5 })),
        ("Mesh", "Icosphere", "icosphere", json!({ "type": "icosphere", "radius": 0.5, "detail": 2 })),
        ("Mesh", "Cylinder", "cylinder", json!({ "type": "cylinder", "radius": 0.5, "height": 1 })),
        ("Mesh", "Cone", "cone", json!({ "type": "cone", "radius": 0.5, "height": 1 })),
        ("Mesh", "Capsule", "capsule", json!({ "type": "capsule", "radius": 0.3, "height": 1 })),
        ("Mesh", "Torus", "torus", json!({ "type": "torus", "radius": 0.5, "tube": 0.2 })),
        ("Mesh", "Plane", "plane", json!({ "type": "plane", "width": 2, "height": 2, "rotation": [-90, 0, 0] })),
        ("Mesh", "Grid", "grid", json!({ "type": "grid", "width": 2, "height": 2, "rows": 16, "cols": 16 })),
        ("Text and shapes", "3D text", "text", json!({ "type": "text", "text": "Text", "size": 1, "depth": 0.2 })),
        ("Text and shapes", "Extruded shape (SVG path)", "extrude", json!({ "type": "extrude", "d": "M0 -50 L14 -15 L50 -15 L21 7 L32 45 L0 22 L-32 45 L-21 7 L-50 -15 L-14 -15 Z", "size": 2, "depth": 0.3 })),
        ("Text and shapes", "Lathe (turned profile)", "lathe", json!({ "type": "lathe", "profile": [[0, 0], [0.45, 0], [0.55, 0.3], [0.3, 0.9], [0.32, 1.3]], "segments": 48 })),
        ("Text and shapes", "Curve", "curve", json!({ "type": "curve", "points": [[-1.5, 0, 0], [-0.5, 0.8, 0], [0.5, -0.8, 0], [1.5, 0, 0]], "radius": 0.05 })),
        ("Other", "Empty", "empty", json!({ "type": "group" })),
        ("Other", "Particles", "particles", json!({ "type": "particles", "rate": 40, "color": "#ffb199" })),
    ];
    let mut out = vec![];
    for (group, label, stem, mut v) in shapes {
        if !matches!(stem, "empty" | "particles" | "curve") {
            v["material"] = grey.clone();
        }
        v["position"] = json!(at);
        let (studio, parent) = (studio.clone(), parent.clone());
        out.push(choice(group, label, model::shape_icon(v["type"].as_str().unwrap_or("")), move |_, cx| add_thing(&studio, stem, v.clone(), parent.clone(), cx)));
    }
    // Pictures: the project's images, or one from a file.
    let images: Vec<(String, String)> = st.project(cx).map(|p| p.assets.iter().filter(|a| a.kind == MediaKind::Image).map(|a| (a.id.to_string(), a.name.clone())).collect()).unwrap_or_default();
    for (id, name) in images {
        let (studio, parent) = (studio.clone(), parent.clone());
        let v = json!({ "type": "image", "asset": id, "width": 2, "position": at });
        out.push(choice("Other", format!("Image card: {name}"), "image", move |_, cx| add_thing(&studio, "card", v.clone(), parent.clone(), cx)));
    }
    {
        let (studio, parent) = (studio.clone(), parent.clone());
        out.push(choice("Other", "Model from a file… (glTF, OBJ, STL)", "package", move |_, cx| {
            let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Add model".into()) });
            let (studio, parent) = (studio.clone(), parent.clone());
            cx.spawn(async move |cx| {
                let Ok(Ok(Some(paths))) = rx.await else { return };
                let Some(path) = paths.first().map(|p| p.to_string_lossy().into_owned()) else { return };
                cx.update(|cx| add_thing(&studio, "model", json!({ "type": "model", "src": path, "position": at }), parent, cx));
            })
            .detach();
        }));
    }
    let toward = |from: [f64; 3]| -> [f64; 3] { [at[0] - from[0], at[1] - from[1], at[2] - from[2]] };
    let lights = [
        ("Sun (directional)", "sun", json!({ "type": "directional", "direction": [-0.5, -1, -0.7], "intensity": 1 })),
        ("Point", "point", json!({ "type": "point", "position": [at[0], at[1] + 2.5, at[2] + 1.0], "intensity": 1.5, "range": 12 })),
        ("Spot", "spot", json!({ "type": "spot", "position": [at[0] + 2.0, at[1] + 3.0, at[2] + 2.0], "direction": toward([at[0] + 2.0, at[1] + 3.0, at[2] + 2.0]), "intensity": 2, "angle": 40 })),
        ("Area", "area", json!({ "type": "area", "position": [at[0], at[1] + 3.0, at[2] + 1.5], "direction": [0, -1, -0.4], "size": [2, 2], "intensity": 1.5 })),
    ];
    for (label, stem, v) in lights {
        let studio = studio.clone();
        out.push(choice("Light", label, "sun", move |_, cx| add_thing(&studio, stem, v.clone(), None, cx)));
    }
    {
        let studio = studio.clone();
        let v = json!({ "type": "camera", "position": eye.map(|v| (v * 100.0).round() / 100.0), "target": at });
        out.push(choice("Camera", "Camera (from this view)", "video", move |_, cx| add_thing(&studio, "camera", v.clone(), None, cx)));
    }
    out
}

fn add_2d(st: &Studio, studio: &Entity<Studio>, cx: &App) -> Vec<Choice> {
    let k = st.project(cx).map(|p| p.settings.height as f64 / 1080.0).unwrap_or(1.0);
    let r = move |v: f64| (v * k * 100.0).round() / 100.0;
    let comp = st.composition.clone();
    let shapes: Vec<(&'static str, &'static str, &'static str, &'static str, Value)> = vec![
        ("Shapes", "Rectangle", "rect", "square", json!({ "type": "rect", "width": r(400.0), "height": r(240.0), "radius": r(16.0), "fill": "#ff5a36" })),
        ("Shapes", "Ellipse", "ellipse", "circle", json!({ "type": "ellipse", "width": r(260.0), "height": r(260.0), "fill": "#ffffff" })),
        ("Shapes", "Polygon", "polygon", "hexagon", json!({ "type": "polygon", "sides": 6, "radius": r(140.0), "fill": "#5cc8ff" })),
        ("Shapes", "Star", "star", "star", json!({ "type": "star", "points": 5, "radius": r(140.0), "innerRadius": r(60.0), "fill": "#f0b44c" })),
        ("Shapes", "Line (path)", "line", "pen-tool", json!({ "type": "path", "d": format!("M{} 0 L{} 0", r(-300.0), r(300.0)), "stroke": { "color": "#ffffff", "width": r(8.0) } })),
        ("Text", "Text", "text", "type", json!({ "type": "text", "text": "Your words", "fontSize": r(96.0), "fill": "#ffffff" })),
        ("Layers", "Null (a handle to parent to)", "null", "crosshair", json!({ "type": "null" })),
        ("Layers", "Adjustment layer", "adjustment", "sliders-horizontal", json!({ "type": "adjustment" })),
        ("Layers", "Group", "group", "folder", json!({ "type": "group", "layers": [] })),
        ("Layers", "Particles", "particles", "sparkles", json!({ "type": "particles", "rate": 40, "color": "#ffb199" })),
    ];
    let mut out = vec![];
    for (group, label, stem, ic, v) in shapes {
        let (studio, comp) = (studio.clone(), comp.clone());
        out.push(choice(group, label, ic, move |_, cx| add_thing(&studio, stem, v.clone(), comp.clone(), cx)));
    }
    if let Some((_, Scene::Flat(f))) = st.clip_scene(cx) {
        for c in f.compositions.iter().filter(|c| Some(&c.id) != comp.as_ref()) {
            let (studio, comp, id) = (studio.clone(), comp.clone(), c.id.clone());
            out.push(choice("Compositions", format!("Composition: {id}"), "layers", move |_, cx| add_thing(&studio, &id, json!({ "type": "comp", "comp": id }), comp.clone(), cx)));
        }
        let studio = studio.clone();
        out.push(choice("Compositions", "New composition", "layers", move |_, cx| {
            let Some((_, scene)) = studio.read(cx).clip_scene(cx) else { return };
            let id = model::fresh_id(&scene, "comp");
            let clip = studio.read(cx).clip;
            studio.update(cx, |s, cx| {
                s.run_then("motion.setComposition", json!({ "clipId": clip, "composition": { "id": id, "layers": [] } }), cx, move |s, _, cx| {
                    s.composition = Some(id.clone());
                    s.set_selection(vec![format!("{}{id}", model::COMPOSITION)], cx);
                })
            });
        }));
    }
    let images: Vec<(String, String)> = st.project(cx).map(|p| p.assets.iter().filter(|a| matches!(a.kind, MediaKind::Image | MediaKind::Video)).map(|a| (a.id.to_string(), a.name.clone())).collect()).unwrap_or_default();
    for (id, name) in images {
        let (studio, comp) = (studio.clone(), comp.clone());
        let v = json!({ "type": "image", "asset": id, "width": r(640.0) });
        out.push(choice("Media", format!("Picture: {name}"), "image", move |_, cx| add_thing(&studio, "picture", v.clone(), comp.clone(), cx)));
    }
    {
        let (studio, comp) = (studio.clone(), comp.clone());
        out.push(choice("Media", "Picture from a file…", "image", move |_, cx| {
            let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Add picture".into()) });
            let (studio, comp) = (studio.clone(), comp.clone());
            cx.spawn(async move |cx| {
                let Ok(Ok(Some(paths))) = rx.await else { return };
                let Some(path) = paths.first().map(|p| p.to_string_lossy().into_owned()) else { return };
                cx.update(|cx| add_thing(&studio, "picture", json!({ "type": "image", "asset": path, "width": r(640.0) }), comp, cx));
            })
            .detach();
        }));
    }
    out
}

/// Groups of a stack's types, for the add menu.
fn group_of(field: &str, name: &str) -> &'static str {
    match field {
        "modifiers" => match name {
            "subdivision" | "mirror" | "array" | "bevel" | "solidify" | "wireframe" | "boolean" | "triangulate" | "decimate" | "weld" | "build" => "Generate",
            _ => "Deform",
        },
        "effects" => match name {
            "blur" | "directionalBlur" | "radialBlur" | "glow" | "dropShadow" | "stroke" | "echo" | "sharpen" => "Blur and light",
            "colorCorrect" | "levels" | "tint" | "tritone" | "fill" | "gradientRamp" | "invert" | "threshold" | "posterize" | "vignette" => "Colour",
            "noise" | "fractalNoise" | "halftone" | "scanlines" => "Generate",
            _ => "Distort",
        },
        "constraints" => "Constraints",
        "operators" => "Shape operators",
        "masks" => "Masks",
        "animators" => "Text animators",
        _ => "",
    }
}

/// The table of a stack field.
pub fn types_of(field: &str) -> &'static [TypeSpec] {
    stack::families().into_iter().find(|(f, _, _)| *f == field).map(|(_, _, t)| t).unwrap_or(&[])
}

fn stack_choices(studio: &Entity<Studio>, id: &str, field: &'static str) -> Vec<Choice> {
    let mut list: Vec<&TypeSpec> = types_of(field).iter().collect();
    list.sort_by_key(|t| group_of(field, t.name));
    list.into_iter()
        .map(|t| {
            let (studio, id, kind) = (studio.clone(), id.to_string(), t.name);
            Choice {
                group: group_of(field, t.name),
                label: t.label.into(),
                icon: "plus",
                hint: None,
                run: Rc::new(move |_, cx| {
                    let clip = studio.read(cx).clip;
                    super::run("motion.setStackItem", json!({ "clipId": clip, "id": id, "field": field, "item": { "type": kind } }), cx);
                }),
            }
        })
        .collect()
}

fn delete_choices(studio: &Entity<Studio>) -> Vec<Choice> {
    let item = |label: &'static str, op: &'static str, what: &'static str| {
        let studio = studio.clone();
        choice("Delete", label, "trash", move |_, cx| studio.update(cx, |s, cx| s.mesh_op(op, json!({ "what": what }), cx)))
    };
    vec![
        item("Vertices", "delete", "vertices"),
        item("Edges", "delete", "edges"),
        item("Faces", "delete", "faces"),
        item("Dissolve vertices", "dissolve", "vertices"),
        item("Dissolve edges", "dissolve", "edges"),
        item("Dissolve faces", "dissolve", "faces"),
    ]
}

/// Right-click on things (in the viewport or the outliner).
pub fn thing_menu(studio: &Entity<Studio>, selection: Vec<String>, cx: &App) -> Vec<MenuEntry> {
    let st = studio.read(cx);
    let Some((_, scene)) = st.clip_scene(cx) else { return vec![] };
    let clip = st.clip;
    let active = selection.last().cloned();
    let things: Vec<String> = selection.iter().filter(|k| model::is_thing(&scene, k)).cloned().collect();
    let mut out = vec![];
    let s = studio.clone();
    let run = move |name: &'static str, params: Value| {
        let _s = s.clone();
        move |_: &mut Window, cx: &mut App| super::run(name, params.clone(), cx)
    };
    let item = model::active_item(&scene, active.as_deref());
    match &item {
        Some((id, model::Item::Object(shape))) => {
            let (id, shape) = (id.clone(), *shape);
            if shape == "mesh" {
                let s = studio.clone();
                out.push(MenuItem::new("Edit mesh", move |_, cx| s.update(cx, |s, cx| s.toggle_edit(cx))).icon("hexagon").shortcut_of(&act::StudioToggleEdit).entry());
                out.push(MenuItem::new("Apply modifiers", run("motion.applyModifier", json!({ "clipId": clip, "id": id }))).icon("check").entry());
            } else if !matches!(shape, "group" | "particles" | "image") {
                out.push(MenuItem::new("Convert to mesh", run("motion.convertToMesh", json!({ "clipId": clip, "id": id }))).icon("hexagon").entry());
                out.push(MenuItem::new("Convert to mesh, modifiers applied", run("motion.convertToMesh", json!({ "clipId": clip, "id": id, "applyModifiers": true }))).icon("hexagon").entry());
            }
            let s = studio.clone();
            let pid = id.clone();
            out.push(MenuItem::new("Add child…", move |w, cx| s.update(cx, |s, cx| {
                let at = w.mouse_position();
                s.popover = Some(Popover::add_child(at, pid.clone(), w, cx));
                cx.notify();
            })).icon("plus").entry());
            if let Scene::Space(sp) = &scene
                && model::parent3d(sp, &id).is_some_and(|p| !p.is_empty())
            {
                out.push(MenuItem::new("Move out of its parent", run("motion.moveLayer", json!({ "clipId": clip, "id": id, "parent": "" }))).icon("arrow-up").entry());
            }
        }
        Some((id, model::Item::Camera)) => {
            out.push(MenuItem::new("Set as the active camera", run("motion.updateLayer", json!({ "clipId": clip, "id": "scene", "props": { "activeCamera": id } }))).icon("video").entry());
            out.push(MenuEntry::Separator);
            out.extend(camera_menu(studio, cx));
        }
        Some((id, model::Item::Layer(kind))) => {
            if *kind == "comp"
                && let Scene::Flat(f) = &scene
                && let Some(kimchi_core::motion::LayerKind::Comp { comp, .. }) = f.find_layer(id).map(|l| &l.kind)
            {
                let (s, comp) = (studio.clone(), comp.clone());
                out.push(MenuItem::new("Open its composition", move |_, cx| s.update(cx, |s, cx| {
                    s.composition = Some(comp.clone());
                    s.set_selection(vec![], cx);
                })).icon("layers").entry());
            }
            let fresh = model::fresh_id(&scene, "comp");
            out.push(MenuItem::new("Precompose", run("motion.precompose", json!({ "clipId": clip, "ids": things, "compositionId": fresh }))).icon("layers").entry());
        }
        Some((c, model::Item::Composition(id))) => {
            let _ = c;
            let (s, id) = (studio.clone(), id.clone());
            out.push(MenuItem::new("Open composition", move |_, cx| s.update(cx, |s, cx| {
                s.composition = Some(id.clone());
                s.changed(cx);
            })).icon("layers").entry());
        }
        _ => {}
    }
    if !out.is_empty() {
        out.push(MenuEntry::Separator);
    }
    let can_copy = !things.is_empty() && !(things.len() == 1 && things[0] == "camera");
    let s = studio.clone();
    out.push(MenuItem::new("Duplicate", move |_, cx| s.update(cx, |s, cx| s.duplicate(cx))).icon("copy").shortcut_of(&act::StudioDuplicate).disabled(!can_copy).entry());
    let s = studio.clone();
    out.push(MenuItem::new("Keyframe here", move |_, cx| s.update(cx, |s, cx| s.keyframe_selection(cx))).icon("diamond").shortcut_of(&act::StudioInsert).disabled(things.is_empty()).entry());
    let s = studio.clone();
    out.push(MenuItem::new("Frame", move |_, cx| s.update(cx, |s, cx| s.frame_selection(true, cx))).icon("scan").shortcut_of(&act::StudioFrame).entry());
    if let Some(id) = active.clone().filter(|a| model::is_thing(&scene, a) || a.starts_with(model::MATERIAL) || a.starts_with(model::COMPOSITION)).filter(|a| a != "camera") {
        let s = studio.clone();
        out.push(MenuItem::new("Rename", move |w, cx| {
            let outliner = s.read(cx).outliner.clone();
            let id = id.clone();
            outliner.update(cx, |o, cx| o.start_rename(&id, w, cx));
        }).icon("pencil").entry());
    }
    let s = studio.clone();
    let deletable = selection.iter().any(|k| k != "scene" && k != "camera");
    out.push(MenuItem::new("Delete", move |w, cx| s.update(cx, |s, cx| s.delete(w, cx))).icon("trash").shortcut_of(&act::StudioDelete).danger().disabled(!deletable).entry());
    out
}

/// Arrange object origins. The labels distinguish origins from the visible mesh's bounds.
pub fn arrange_menu(studio: &Entity<Studio>, cx: &App) -> Vec<MenuEntry> {
    let st = studio.read(cx);
    let Some((_, Scene::Space(scene))) = st.clip_scene(cx) else { return vec![] };
    let objects: Vec<_> = st.selection.iter().filter(|id| kimchi_core::motion::find_object(&scene.objects, id).is_some()).cloned().collect();
    let count = objects.iter().filter(|id| !objects.iter().any(|parent| parent != *id && kimchi_core::motion::find_object(&scene.objects, parent)
        .is_some_and(|o| kimchi_core::motion::find_object(&o.children, id).is_some()))).count();
    let mut entries = vec![];
    for (operation, label, needed) in [("alignActive", "Align origins to active", 2), ("alignCentre", "Align origins to centre", 2), ("distribute", "Space origins evenly", 3)] {
        if !entries.is_empty() { entries.push(MenuEntry::Separator); }
        for axis in ["x", "y", "z"] {
            let params = json!({ "clipId": st.clip, "ids": objects, "operation": operation, "axis": axis, "time": st.playhead(cx) });
            entries.push(MenuItem::new(format!("{label} · {}", axis.to_uppercase()), move |_, cx| {
                super::run("motion.arrangeObjects", params.clone(), cx);
            }).disabled(count < needed).entry());
        }
    }
    entries
}

/// The Camera menu (the toolbar's, a camera's right-click and its Properties): looking through
/// it, locking it to the view, putting it where the view is, keyframing it, and the classic
/// moves written as editable animation (`motion.cameraMove`).
pub fn camera_menu(studio: &Entity<Studio>, cx: &App) -> Vec<MenuEntry> {
    let st = studio.read(cx);
    let (through, lock, keyed) = (st.view_shown().1, st.lock_camera, st.camera_keyed_here(cx));
    let cam = st.camera_in_hand(cx).unwrap_or_else(|| "camera".into());
    let curve = st.clip_scene(cx).and_then(|(_, scene)| st.active().filter(|a| matches!(model::item(&scene, a), Some(model::Item::Object("curve")))).map(str::to_string));
    let check = |on: bool, otherwise: &'static str| if on { "check" } else { otherwise };
    let mut out = vec![];
    let s = studio.clone();
    out.push(MenuItem::new("Look through the camera", move |_, cx| s.update(cx, |s, cx| s.toggle_camera_view(cx))).icon(check(through, "video")).shortcut_of(&act::StudioKey0).entry());
    let s = studio.clone();
    out.push(MenuItem::new("Lock the camera to the view", move |_, cx| s.update(cx, |s, cx| s.set_lock_camera(!s.lock_camera, cx))).icon(check(lock, "lock-open")).entry());
    let s = studio.clone();
    out.push(MenuItem::new("Align the camera to the view", move |_, cx| s.update(cx, |s, cx| {
        if let Err(e) = s.align_camera_to_view(cx) {
            super::flash(e, cx);
        }
    })).icon("scan-eye").shortcut_of(&act::StudioAlignCamera).entry());
    let s = studio.clone();
    out.push(MenuItem::new("Add a camera here", move |_, cx| s.update(cx, |s, cx| {
        let _ = s.add_camera_here(cx);
    })).icon("plus").entry());
    let s = studio.clone();
    out.push(MenuItem::new(if keyed { format!("Remove {cam}'s keyframe here") } else { format!("Keyframe {cam} here") }, move |_, cx| s.update(cx, |s, cx| {
        if let Err(e) = s.keyframe_camera(None, cx) {
            super::flash(e, cx);
        }
    })).icon("diamond").entry());
    out.push(MenuEntry::Separator);
    let moves: Vec<(String, &'static str, &'static str, Value)> = vec![
        ("Orbit around the selection (90°)".into(), "orbit", "orbit", json!({})),
        ("Turntable (a whole turn)".into(), "refresh-cw", "turntable", json!({})),
        ("Dolly in".into(), "zoom-in", "dolly", json!({ "share": 0.35 })),
        ("Dolly out".into(), "zoom-out", "dolly", json!({ "share": -0.5 })),
        ("Truck left".into(), "move-horizontal", "truck", json!({ "share": -0.3 })),
        ("Truck right".into(), "move-horizontal", "truck", json!({ "share": 0.3 })),
        ("Crane up".into(), "move-vertical", "crane", json!({ "share": 0.35 })),
        ("Crane down".into(), "move-vertical", "crane", json!({ "share": -0.25 })),
        ("Zoom the lens in".into(), "aperture", "zoom", json!({})),
        (match &curve {
            Some(c) => format!("Fly along “{c}”"),
            None => "Fly-by (a new path you can edit)".into(),
        }, "route", "flyThrough", curve.as_ref().map(|c| json!({ "path": c })).unwrap_or(json!({}))),
        ("Handheld shake".into(), "vibrate", "handheld", json!({})),
    ];
    for (label, ic, mv, extra) in moves {
        let s = studio.clone();
        out.push(MenuItem::new(format!("Move: {label}"), move |_, cx| s.update(cx, |s, cx| s.camera_move(mv, extra.clone(), cx))).icon(ic).entry());
    }
    let s = studio.clone();
    out.push(MenuItem::new("Clear the camera's moves", move |_, cx| s.update(cx, |s, cx| s.camera_move("clear", json!({}), cx))).icon("trash").danger().entry());
    out.push(MenuEntry::Separator);
    let s = studio.clone();
    out.push(MenuItem::new("Fly through the scene", move |_, cx| {
        let vp = s.read(cx).viewport.clone();
        vp.update(cx, |v, cx| v.start_fly(false, cx));
    }).icon("plane").shortcut_of(&act::StudioFly).entry());
    let s = studio.clone();
    out.push(MenuItem::new("Frame the selection", move |_, cx| s.update(cx, |s, cx| s.frame_selection(true, cx))).icon("focus").shortcut_of(&act::StudioFrame).entry());
    out
}

/// Right-click in edit mode: every mesh operation (with the engine's defaults).
pub fn mesh_menu(studio: &Entity<Studio>, _cx: &App) -> Vec<MenuEntry> {
    let mut out = vec![];
    let keyed: [(&str, &'static dyn gpui::Action); 7] = [
        ("extrude", &act::StudioExtrude),
        ("inset", &act::StudioInsert),
        ("bevel", &act::StudioBevel),
        ("loopCut", &act::StudioLoopCut),
        ("merge", &act::StudioMerge),
        ("flip", &act::StudioFlip),
        ("recalcNormals", &act::StudioRecalc),
    ];
    for op in kimchi_core::mesh::ops::EDIT_OPS {
        let s = studio.clone();
        let name = op.name;
        let interactive = matches!(name, "extrude" | "inset" | "bevel" | "loopCut");
        let mut item = MenuItem::new(op.label, move |w, cx| {
            let vp = s.read(cx).viewport.clone();
            match name {
                "extrude" => vp.update(cx, |v, cx| v.start_modal(super::viewport::ModalKind::Extrude, w, cx)),
                "inset" => vp.update(cx, |v, cx| v.start_modal(super::viewport::ModalKind::Inset, w, cx)),
                "bevel" => vp.update(cx, |v, cx| v.start_modal(super::viewport::ModalKind::Bevel, w, cx)),
                "loopCut" => vp.update(cx, |v, cx| v.arm_loop_cut(cx)),
                _ => s.update(cx, |s, cx| { let _ = s.open_mesh_tool(name, cx); }),
            }
        })
        .icon(if interactive { "mouse-pointer-2" } else { "hexagon" });
        if let Some((_, a)) = keyed.iter().find(|(n, _)| *n == name) {
            item = item.shortcut_of(*a);
        }
        out.push(item.entry());
        if matches!(name, "loopCut" | "dissolve" | "recalcNormals" | "duplicate") {
            out.push(MenuEntry::Separator);
        }
    }
    let s = studio.clone();
    out.push(MenuEntry::Separator);
    out.push(MenuItem::new("Leave edit mode", move |_, cx| s.update(cx, |s, cx| {
        let _ = s.set_mode(Mode::Object, cx);
    })).shortcut_of(&act::StudioToggleEdit).entry());
    out
}

/// Opens the store's context menu at `at` (used by panels for dropdowns).
pub fn open_menu(at: Point<Pixels>, entries: Vec<MenuEntry>, cx: &mut App) {
    cx.store().update(cx, |s, cx| s.open_menu(at, entries, cx));
}
