//! The bar over the Studio: back to the edit, the clip, the mode and tools, how the view is
//! drawn, the view's buttons, Add, and rendering the clip ahead.

use gpui::{AnyElement, App, Context, Div, FontWeight, SharedString, Window, div, prelude::*, px};
use kimchi_core::Scene;
use kimchi_media::render::space::viewport::Shading;
use serde_json::json;

use super::render_state;
use super::{Mode, SelectMode, Studio, Tool};
use crate::actions::{self as act, tip};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, segmented};

fn sep(cx: &App) -> Div {
    div().flex_none().w(px(1.)).h(px(18.)).mx(px(4.)).bg(cx.theme().line)
}

pub fn render(this: &mut Studio, _window: &mut Window, cx: &mut Context<Studio>) -> AnyElement {
    let t = cx.theme().clone();
    let me = cx.entity();
    let Some((clip, scene)) = this.clip_scene(cx) else { return div().into_any_element() };
    let three = scene.is_3d();
    let tool_button = |tool: Tool, icon: &'static str, label: &str, action: &dyn gpui::Action| {
        let me = me.clone();
        Button::icon(SharedString::from(format!("tool-{}", tool.name())), icon, tip(label, action)).selected(this.tool == tool).on_click(move |_, _, cx| me.update(cx, |s, cx| s.set_tool(tool, cx)))
    };
    let mut bar = div()
        .id("studio-toolbar")
        .h(px(40.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(8.))
        .border_b_1()
        .border_color(t.line)
        .bg(t.bg_raised)
        .overflow_x_hidden();
    {
        let me = me.clone();
        bar = bar.child(Button::new("studio-back", "Back to the edit").small().with_icon("chevron-left").tooltip(tip("Back to the edit", &act::StudioEscape)).on_click(move |_, _, cx| me.update(cx, |s, cx| s.close(cx))));
    }
    bar = bar.child(
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(6.))
            .min_w_0()
            .child(crate::ui::icon(if three { "box" } else { "shapes" }).text_color(t.accent_text))
            .child(div().max_w(px(180.)).truncate().font_weight(FontWeight::SEMIBOLD).text_size(px(sz::BASE)).child(clip.name.clone()))
            .child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(if three { "3D" } else { "2D" })),
    );
    bar = bar.child(sep(cx));
    if three {
        let me2 = me.clone();
        bar = bar.child(div().w(px(140.)).child(segmented(
            "studio-mode",
            vec![(Mode::Object, "Object".into()), (Mode::Edit, "Edit".into())],
            this.mode,
            move |m, _, cx| {
                let m = *m;
                me2.update(cx, |s, cx| {
                    if m == Mode::Edit {
                        s.toggle_edit(cx);
                    } else {
                        let _ = s.set_mode(Mode::Object, cx);
                    }
                })
            },
            cx,
        )));
        if this.mode == Mode::Edit {
            for (m, icon, label, a) in [
                (SelectMode::Vertex, "dot", "Vertices", &act::StudioKey1 as &dyn gpui::Action),
                (SelectMode::Edge, "minus", "Edges", &act::StudioKey2),
                (SelectMode::Face, "square", "Faces", &act::StudioKey3),
            ] {
                let me = me.clone();
                bar = bar.child(Button::icon(SharedString::from(format!("selmode-{}", m.name())), icon, tip(label, a)).selected(this.select_mode == m).on_click(move |_, _, cx| {
                    me.update(cx, |s, cx| {
                        s.select_mode = m;
                        s.changed(cx);
                    })
                }));
            }
            let me = me.clone();
            bar = bar.child(Button::new("mesh-ops", "Mesh").small().ghost().icon_after("chevron-down").tooltip("Every modelling operation (or right-click in the view)").on_click(move |e, _, cx| {
                let entries = super::menus::mesh_menu(&me, cx);
                super::menus::open_menu(e.position(), entries, cx);
            }));
        } else {
            bar = bar
                .child(tool_button(Tool::Select, "mouse-pointer-2", "Select", &act::StudioToolSelect))
                .child(tool_button(Tool::Move, "move", "Move", &act::StudioGrab))
                .child(tool_button(Tool::Rotate, "rotate-cw", "Rotate", &act::StudioRotate))
                .child(tool_button(Tool::Scale, "scaling", "Scale", &act::StudioScale));
            let me_l = me.clone();
            bar = bar.child(Button::new("gizmo-space", if this.local { "Local" } else { "Global" }).small().ghost().tooltip("Gizmo axes: the world's (global) or the object's own (local)").on_click(move |_, _, cx| {
                me_l.update(cx, |s, cx| {
                    s.local = !s.local;
                    s.changed(cx);
                })
            }));
        }
        let me_s = me.clone();
        bar = bar.child(Button::icon("snap", "magnet", "Snap: moves to the grid, turns to 15° (Ctrl while dragging)").selected(this.snapping).on_click(move |_, _, cx| {
            me_s.update(cx, |s, cx| {
                s.snapping = !s.snapping;
                s.changed(cx);
            })
        }));
        bar = bar.child(sep(cx));
        let me_sh = me.clone();
        bar = bar.child(div().w(px(210.)).child(segmented(
            "studio-shading",
            vec![(Shading::Solid, "Solid".into()), (Shading::Material, "Material".into()), (Shading::Rendered, "Rendered".into())],
            this.shading,
            move |sh, _, cx| {
                let sh = *sh;
                me_sh.update(cx, |s, cx| {
                    s.shading = sh;
                    s.changed(cx);
                })
            },
            cx,
        )));
        let (me_g, me_h, me_o, me_c) = (me.clone(), me.clone(), me.clone(), me.clone());
        bar = bar
            .child(Button::icon("grid", "grid-3x3", "Floor grid and axes").selected(this.grid).on_click(move |_, _, cx| {
                me_g.update(cx, |s, cx| {
                    s.grid = !s.grid;
                    s.changed(cx);
                })
            }))
            .child(Button::icon("helpers", "lightbulb", "Lights and cameras in the view").selected(this.helpers).on_click(move |_, _, cx| {
                me_h.update(cx, |s, cx| {
                    s.helpers = !s.helpers;
                    s.changed(cx);
                })
            }))
            .child(Button::icon("ortho", if this.view.ortho { "square" } else { "box" }, tip(if this.view.ortho { "Orthographic (switch to perspective)" } else { "Perspective (switch to orthographic)" }, &act::StudioOrtho)).on_click(move |_, _, cx| {
                me_o.update(cx, |s, cx| {
                    s.view.ortho = !s.view.ortho;
                    s.through_camera = false;
                    s.changed(cx);
                })
            }))
            .child(Button::icon("cam-view", "video", tip("Through the camera", &act::StudioKey0)).selected(this.through_camera).on_click(move |_, _, cx| {
                me_c.update(cx, |s, cx| {
                    s.through_camera = !s.through_camera;
                    s.changed(cx);
                })
            }));
    } else {
        bar = bar
            .child(tool_button(Tool::Select, "mouse-pointer-2", "Select", &act::StudioToolSelect))
            .child(tool_button(Tool::Anchor, "crosshair", "Anchor point", &act::StudioAnchor))
            .child(tool_button(Tool::Pen, "pen-tool", "Pen", &act::StudioPen))
            .child(tool_button(Tool::Rect, "square", "Rectangle", &act::StudioShape))
            .child(tool_button(Tool::Ellipse, "circle", "Ellipse", &act::StudioShape))
            .child(tool_button(Tool::Star, "star", "Star", &act::StudioShape))
            .child(tool_button(Tool::Polygon, "hexagon", "Polygon", &act::StudioShape))
            .child(tool_button(Tool::Text, "type", "Text", &act::StudioText));
        if this.tool == Tool::Pen {
            let me = me.clone();
            bar = bar.child(Button::new("mask-mode", if this.mask_mode { "Drawing a mask" } else { "Drawing a path" }).small().ghost().selected(this.mask_mode).tooltip("The pen draws a new path layer, or a mask on the selected layer").on_click(move |_, _, cx| {
                me.update(cx, |s, cx| {
                    s.mask_mode = !s.mask_mode;
                    s.changed(cx);
                })
            }));
        }
        bar = bar.child(sep(cx));
        // Where the canvas is: the scene, or a composition opened from the outliner.
        if let Scene::Flat(f) = &scene {
            let me = me.clone();
            let label = match &this.composition {
                Some(c) => format!("Scene › {c}"),
                None => "Scene".to_string(),
            };
            let comps: Vec<String> = f.compositions.iter().map(|c| c.id.clone()).collect();
            bar = bar.child(Button::new("comp-pick", label).small().ghost().with_icon("layers").icon_after("chevron-down").tooltip("Show the scene or one of its compositions").on_click(move |e, _, cx| {
                let mut entries = vec![];
                let m = me.clone();
                entries.push(crate::store::MenuItem::new("Scene", move |_, cx| m.update(cx, |s, cx| {
                    s.composition = None;
                    s.changed(cx);
                })).icon("shapes").entry());
                for c in &comps {
                    let (m, c) = (me.clone(), c.clone());
                    entries.push(crate::store::MenuItem::new(c.clone(), move |_, cx| m.update(cx, |s, cx| {
                        s.composition = Some(c.clone());
                        s.changed(cx);
                    })).icon("layers").entry());
                }
                super::menus::open_menu(e.position(), entries, cx);
            }));
        }
        let me_f = me.clone();
        bar = bar.child(Button::icon("fit", "maximize-2", tip("Fit the canvas", &act::StudioFit)).on_click(move |_, _, cx| {
            me_f.update(cx, |s, cx| {
                s.canvas.fit = true;
                s.changed(cx);
            })
        }));
    }
    {
        let me = me.clone();
        bar = bar.child(sep(cx)).child(Button::new("studio-add", "Add").small().with_icon("plus").tooltip(tip("Add", &act::StudioAdd)).disabled(this.mode == Mode::Edit).on_click(move |e, w, cx| {
            let at = e.position();
            me.update(cx, |s, cx| s.open_add_menu(Some(at), w, cx))
        }));
    }
    bar = bar.child(div().flex_1());
    // Rendering ahead, and the engine final frames use.
    if let Scene::Space(s) = &scene {
        let path = s.render.path_traced();
        let _me = me.clone();
        let clip_id = clip.id;
        bar = bar.child(
            Button::new("engine", if path { "Path tracer" } else { "Standard" })
                .small()
                .ghost()
                .with_icon(if path { "sparkles" } else { "zap" })
                .tooltip("The engine for final frames (exports and renders): the quick standard one or the path tracer. Click to switch.")
                .on_click(move |_, _, cx| {
                    let engine = if path { "standard" } else { "path" };
                    super::run("motion.updateLayer", json!({ "clipId": clip_id, "id": "scene", "props": { "render": { "engine": engine } } }), cx);
                }),
        );
    }
    if let Some(st) = render_state::state_of(clip.id, cx) {
        bar = bar.child(render_state::controls("studio-render", clip.id, &st, cx));
    }
    bar.into_any_element()
}
