//! One Studio sidebar: scene hierarchy, properties and a parameterized modelling
//! workbench. Every operation uses the command registry and the shared undo stack.
use super::{Area, Mode, SelectMode, Studio};
use crate::store::MenuItem;
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::{Button, segmented};
use gpui::{AnyElement, Context, Entity, MouseButton, Render, ScrollHandle, Subscription, Window, div, prelude::*, px};
use kimchi_core::mesh::ops::EDIT_OPS;
use kimchi_core::motion::stack::ParamKind;
use serde_json::{Map, Value, json};

pub struct Sidebar {
    studio: Entity<Studio>,
    page: u8,
    operation: &'static str,
    values: Map<String, Value>,
    numbers: Vec<(&'static str, Entity<Scrub>)>,
    fields: Vec<Subscription>,
    search: Entity<TextInput>,
    picking: bool,
    workbench_scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl Sidebar {
    pub fn new(studio: Entity<Studio>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Find a modelling tool…"));
        let subs = vec![
            cx.observe(&studio, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Changed(_)) {
                    cx.notify();
                }
            }),
        ];
        let mut this = Self { studio, page: 0, operation: "extrude", values: Map::new(), numbers: vec![], fields: vec![], search, picking: false, workbench_scroll: ScrollHandle::new(), _subs: subs };
        this.select_op("extrude", cx);
        this.page = 0;
        this
    }

    pub fn select_op(&mut self, name: &'static str, cx: &mut Context<Self>) {
        let Some(op) = EDIT_OPS.iter().find(|o| o.name == name) else { return };
        self.page = 2;
        self.operation = name;
        self.picking = false;
        self.workbench_scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.values = op.params.iter().filter_map(|p| p.default.value().map(|v| (p.name.to_string(), v))).collect();
        self.fields.clear();
        self.numbers.clear();
        for p in op.params {
            let spec = match p.kind {
                ParamKind::Number { min, max, step } => Some((min, max, step, 3, 1)),
                ParamKind::Int { min, max } => Some((min, max, 1., 0, 1)),
                ParamKind::Vec3 => Some((-1e9, 1e9, 0.01, 3, 3)),
                _ => None,
            };
            let Some((min, max, step, decimals, components)) = spec else { continue };
            for component in 0..components {
                let initial =
                    self.values.get(p.name).and_then(|v| if components == 3 { v.get(component) } else { Some(v) }).and_then(Value::as_f64).unwrap_or(0.);
                let label = if components == 3 { format!("{} {}", p.label, ["X", "Y", "Z"][component]) } else { p.label.into() };
                let input = cx.new(|_| {
                    let mut s = Scrub::new(label, step, decimals).range(min, max);
                    s.set_value(initial);
                    s
                });
                let key = p.name;
                self.fields.push(cx.subscribe(&input, move |this, _, e: &ScrubChange, cx| {
                    if components == 3 {
                        let vector = this.values.entry(key).or_insert_with(|| json!([0., 0., 0.]));
                        vector[component] = json!(e.value);
                    } else {
                        this.values.insert(key.into(), if decimals == 0 { json!(e.value.round() as i64) } else { json!(e.value) });
                    }
                    cx.notify();
                }));
                self.numbers.push((p.name, input));
            }
        }
        cx.notify();
    }

    pub fn show_properties(&mut self, cx: &mut Context<Self>) {
        self.page = 1;
        cx.notify();
    }

    pub fn show_scene(&mut self, cx: &mut Context<Self>) {
        self.page = 0;
        cx.notify();
    }

    fn take_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        // The shared sidebar is outside the Studio's element tree. Keep its shortcuts active
        // after a click, unless an input or scrub has already claimed keyboard focus.
        if window.default_prevented() { return; }
        let area=match self.page {0=>Area::Outliner,1=>Area::Properties,_=>Area::Viewport};
        self.studio.update(cx,|s,cx| {
            s.focus_area(area,cx);
            window.focus(&s.focus,cx);
        });
        window.prevent_default();
    }

    fn workbench(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let st = self.studio.read(cx);
        let editing = st.mode == Mode::Edit;
        let mode = st.select_mode;
        let selection = st.edit_sel.clone();
        let mesh = st.mesh_topology(cx);
        let busy = st.mesh_busy || st.viewport.read(cx).busy();
        let feedback = st.mesh_feedback.clone();
        let active = st.active().unwrap_or("No object selected").to_string();
        let selected = mesh.as_ref().map(|m| m.convert(&selection, mode)).unwrap_or_default();
        let count = match mode {
            SelectMode::Vertex => selected.vertices.len(),
            SelectMode::Edge => selected.edges.len(),
            SelectMode::Face => selected.faces.len(),
        };
        let selected = format!(
            "{count} {} selected",
            match mode {
                SelectMode::Vertex => if count == 1 { "vertex" } else { "vertices" },
                SelectMode::Edge => if count == 1 { "edge" } else { "edges" },
                SelectMode::Face => if count == 1 { "face" } else { "faces" },
            }
        );
        let three = st.is_3d(cx);
        let shape = st.clip_scene(cx).and_then(|(_, scene)| {
            st.active().and_then(|id| match super::model::item(&scene, id) {
                Some(super::model::Item::Object(shape)) => Some(shape),
                _ => None,
            })
        });
        let can_edit = three && shape.is_some_and(|s| !matches!(s, "group" | "particles" | "image"));
        let enter_label = if shape.is_some_and(|s| s != "mesh") { "Convert to mesh & edit" } else { "Enter edit mode" };
        let op = EDIT_OPS.iter().find(|o| o.name == self.operation).expect("registered operation");
        let mut body = div()
            .id("studio-workbench-content")
            .flex_none()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(sz::BASE)).font_weight(gpui::FontWeight::SEMIBOLD).truncate().child(active))
            .when_some(mesh.as_ref(), |d, m| {
                d.child(div().text_size(px(sz::XS)).text_color(t.text_3).child(format!(
                    "{} vertices · {} edges · {} faces",
                    m.vertices,
                    m.edges.len(),
                    m.faces.len()
                )))
            })
            .child(div().text_size(px(sz::XS)).text_color(t.text_2).child(if editing {
                selected
            } else {
                "Select a 3D object, then enter edit mode to work on its mesh.".into()
            }))
            .when(!editing, |d| {
                d.child(
                    Button::new("workbench-edit", enter_label)
                        .small()
                        .disabled(!can_edit)
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.toggle_edit(cx)))),
                )
            });
        if editing {
            let me = self.studio.clone();
            body = body.child(segmented(
                "workbench-select-mode",
                vec![(SelectMode::Vertex, "Vertex".into()), (SelectMode::Edge, "Edge".into()), (SelectMode::Face, "Face".into())],
                mode,
                move |m, _, cx| me.update(cx, |s, cx| s.set_select_mode(*m, cx)),
                cx,
            ));
            let mut actions = div().flex().flex_wrap().gap(px(4.));
            for &(action, label, tip) in super::selection::ACTIONS.iter().chain(super::selection::EDGE_ACTIONS) {
                let studio = self.studio.clone();
                let edge_path=super::selection::EDGE_ACTIONS.iter().any(|(name,_,_)| *name==action);
                actions =
                    actions.child(Button::new(format!("mesh-select-{action}"), label).small().ghost().tooltip(tip)
                        .disabled(busy || (edge_path && (mode!=SelectMode::Edge || count==0))).on_click(move |_, _, cx| {
                        studio.update(cx, |s, cx| {
                            if let Err(e) = s.select_mesh(action, cx) {
                                super::flash(e, cx);
                            }
                        })
                    }));
            }
            let studio = self.studio.clone();
            actions = actions.child(
                Button::new("mesh-frame", "Frame")
                    .small()
                    .ghost()
                    .with_icon("maximize-2")
                    .tooltip("Frame the selected components")
                    .on_click(move |_, _, cx| studio.update(cx, |s, cx| s.frame_selection(true, cx))),
            );
            body = body.child(actions);
        }
        body = body.child(div().h(px(1.)).bg(t.line)).child(
            Button::new("workbench-operation", op.label).with_icon("wrench").icon_after("chevron-down").on_click(cx.listener(|this, _, _, cx| {
                this.picking = !this.picking;
                cx.notify();
            })),
        );
        if self.picking {
            let query = self.search.read(cx).text().to_lowercase();
            body = body.child(self.search.clone());
            let mut matches = 0;
            for group in ["Build", "Reshape", "Transform", "Repair", "UV mapping"] {
                let choices: Vec<_> = EDIT_OPS
                    .iter()
                    .filter(|o| tool_group(o.name) == group && format!("{} {} {group}", o.label, o.doc).to_lowercase().contains(&query))
                    .collect();
                if choices.is_empty() {
                    continue;
                }
                body = body.child(crate::ui::caps(group, cx));
                for choice in choices {
                    matches += 1;
                    let name = choice.name;
                    body = body.child(
                        Button::new(format!("operation-{name}"), choice.label)
                            .small()
                            .ghost()
                            .selected(name == self.operation)
                            .tooltip(choice.doc)
                            .on_click(cx.listener(move |this, _, _, cx| this.select_op(name, cx))),
                    );
                }
            }
            if matches == 0 {
                body = body.child(div().text_size(px(sz::SM)).text_color(t.text_3).child("No matching tools. Try extrude, bevel or normals."));
            }
            return body.into_any_element();
        }
        body = body.child(div().text_size(px(sz::SM)).text_color(t.text_2).child(op.doc));
        for p in op.params {
            let key = p.name;
            if p.default.value().is_none() {
                let enabled = self.values.contains_key(key);
                body = body.child(
                    Button::new(format!("optional-{key}"), format!("Custom {}", p.label.to_lowercase()))
                        .small()
                        .ghost()
                        .selected(enabled)
                        .with_icon(if enabled { "check" } else { "square" })
                        .tooltip(p.doc)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.values.remove(key);
                            } else {
                                let values: Vec<_> = this.numbers.iter().filter(|(name, _)| *name == key).map(|(_, s)| s.read(cx).value()).collect();
                                this.values.insert(key.into(), json!(values));
                            }
                            cx.notify();
                        })),
                );
                if !enabled {
                    continue;
                }
            }
            if self.operation == "extrude" && key == "distance" && self.values.contains_key("offset") {
                continue;
            }
            if self.operation == "merge" && key == "distance" && self.values.get("at").and_then(Value::as_str) != Some("distance") {
                continue;
            }
            for (index, (_, input)) in self.numbers.iter().filter(|(name, _)| *name == key).enumerate() {
                body = body.child(
                    div()
                        .id((gpui::SharedString::from(format!("op-param-{key}")), index))
                        .tooltip(move |_, cx| crate::ui::tooltip(p.doc.into(), cx))
                        .child(input.clone()),
                );
            }
            match p.kind {
                ParamKind::Bool => {
                    let on = self.values.get(key).and_then(Value::as_bool).unwrap_or(false);
                    body = body.child(Button::new(format!("op-{key}"), p.label).small().selected(on).with_icon(if on { "check" } else { "square" }).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.values.insert(key.into(), json!(!on));
                            cx.notify();
                        }),
                    ));
                }
                ParamKind::Choice(choices) => {
                    let value = self.values.get(key).and_then(Value::as_str).unwrap_or(choices[0]);
                    body = body.child(Button::new(format!("op-{key}"), format!("{}: {value}", p.label)).small().icon_after("chevron-down").on_click(
                        cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            let me = cx.entity().downgrade();
                            let entries = choices
                                .iter()
                                .map(|&value| {
                                    let me = me.clone();
                                    MenuItem::new(value, move |_, cx| {
                                        me.update(cx, |this, cx| {
                                            this.values.insert(key.into(), json!(value));
                                            cx.notify();
                                        })
                                        .ok();
                                    })
                                    .entry()
                                })
                                .collect();
                            let store = this.studio.read(cx).store.clone();
                            store.update(cx, |s, cx| s.open_menu(e.position(), entries, cx));
                        }),
                    ));
                }
                _ => {}
            }
        }
        let whole_mesh = selection.is_empty() && matches!(self.operation, "knife" | "unwrap" | "recalcNormals");
        if editing && selection.is_empty() {
            body = body.child(div().text_size(px(sz::XS)).text_color(t.text_2).child(if whole_mesh {
                "No components selected. This tool will affect the whole mesh."
            } else {
                "Select components in the viewport or use the selection controls above."
            }));
        }
        if let Some(result) = feedback {
            body = body.child(
                div().p(px(8.)).bg(t.bg_raised).text_size(px(sz::SM)).text_color(if result.is_ok() { t.text_2 } else { t.accent_text }).child(
                    match result {
                        Ok(s) | Err(s) => s,
                    },
                ),
            );
        }
        body.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .child(
                    Button::new(
                        "workbench-apply",
                        if busy {
                            "Applying…"
                        } else if whole_mesh {
                            "Apply to whole mesh"
                        } else {
                            "Apply to selection"
                        },
                    )
                    .small()
                    .primary()
                    .disabled(!editing || busy || (selection.is_empty() && !whole_mesh))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let op = this.operation;
                        let values = Value::Object(this.values.clone());
                        this.studio.update(cx, |s, cx| s.mesh_op(op, values, cx));
                    })),
                )
                .child(Button::new("workbench-reset", "Reset").small().ghost().on_click(cx.listener(|this, _, _, cx| this.select_op(this.operation, cx)))),
        )
        .into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity().downgrade();
        let outliner = self.studio.read(cx).outliner.clone();
        let properties = self.studio.read(cx).properties.clone();
        let three = self.studio.read(cx).is_3d(cx);
        if !three && self.page == 2 {
            self.page = 0;
        }
        let content = match self.page {
            2 => div().id("studio-workbench").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.workbench_scroll).child(self.workbench(cx)).into_any_element(),
            1 => div().flex_1().min_h_0().child(properties).into_any_element(),
            _ => div().flex_1().min_h_0().child(outliner).into_any_element(),
        };
        let mut pages = vec![(0u8, "Scene".into()), (1, "Properties".into())];
        if three {
            pages.push((2, "Model".into()));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left,cx.listener(|this,_,window,cx| this.take_focus(window,cx)))
            .on_mouse_down(MouseButton::Right,cx.listener(|this,_,window,cx| this.take_focus(window,cx)))
            .child(div().p(px(10.)).child(segmented(
                "studio-sidebar-mode",
                pages,
                self.page,
                move |page, _, cx| {
                    me.update(cx, |s, cx| {
                        s.page = *page;
                        let area=match s.page {0=>Area::Outliner,1=>Area::Properties,_=>Area::Viewport};
                        s.studio.update(cx,|s,cx| s.focus_area(area,cx));
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            )))
            .child(content)
    }
}

fn tool_group(name: &str) -> &'static str {
    match name {
        "extrude" | "extrudeIndividual" | "inset" | "subdivide" | "loopCut" | "bridge" | "fill" | "spin" | "duplicate" => "Build",
        "bevel" | "smooth" | "knife" | "poke" => "Reshape",
        "translate" | "rotate" | "scale" | "mirror" => "Transform",
        "unwrap" => "UV mapping",
        _ => "Repair",
    }
}
