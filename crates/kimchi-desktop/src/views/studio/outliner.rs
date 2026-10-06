//! The outliner: the scene as a tree. 3D: the world, cameras (the active one marked), lights,
//! objects with their children, shared materials. 2D: the scene, its layers (top of the list is
//! drawn on top, like After Effects), groups, compositions with their layers. Click to select
//! (Shift/Cmd to add), double-click to rename, the eye to hide, drag to reorder or put inside
//! another, right-click for more.

use std::collections::HashSet;

use gpui::{Context, Entity, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollHandle, SharedString, Subscription, Window, div, prelude::*, px};
use serde_json::json;

use super::model::{self, Row, RowKind};
use super::{Area, Studio};
use crate::store::StoreExt;
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, caps, drag, icon};

const ROW_H: f32 = 24.;

/// Where a dragged row would land.
#[derive(Clone, Debug, PartialEq)]
pub struct Drop {
    /// The row it is dropped on.
    pub target: usize,
    /// −1 above it, 0 inside it, 1 below it.
    pub place: i8,
}

struct RowDrag {
    key: String,
    start: Point<Pixels>,
    moving: bool,
    drop: Option<Drop>,
}

pub struct Outliner {
    studio: Entity<Studio>,
    scroll: ScrollHandle,
    rename: Option<(String, Entity<TextInput>, Subscription)>,
    drag: Option<RowDrag>,
    _subs: Vec<Subscription>,
}

impl Outliner {
    pub fn new(studio: Entity<Studio>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let subs = vec![cx.observe(&studio, |_, _, cx| cx.notify()), cx.observe(&store, |_, _, cx| cx.notify())];
        Self { studio, scroll: ScrollHandle::new(), rename: None, drag: None, _subs: subs }
    }

    fn rows(&self, cx: &gpui::App) -> Vec<Row> {
        let st = self.studio.read(cx);
        match st.clip_scene(cx) {
            Some((_, scene)) => model::rows(&scene, &st.collapsed),
            None => vec![],
        }
    }

    pub fn start_rename(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = key.strip_prefix(model::MATERIAL).or_else(|| key.strip_prefix(model::COMPOSITION)).unwrap_or(key).to_string();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx);
            i.mono = true;
            i.set_text(current.clone(), cx);
            i.select_all_text(cx);
            i
        });
        crate::ui::input::focus(&input, window, cx);
        let key_s = key.to_string();
        let sub = cx.subscribe(&input, move |this: &mut Self, input, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Blur => {
                let name = input.read(cx).text().trim().to_string();
                this.rename = None;
                if !name.is_empty() && name != current {
                    let clip = this.studio.read(cx).clip;
                    let (old_key, current) = (key_s.clone(), current.clone());
                    let new_key = if let Some(p) = [model::MATERIAL, model::COMPOSITION].into_iter().find(|p| key_s.starts_with(*p)) { format!("{p}{name}") } else { name.clone() };
                    this.studio.update(cx, |s, cx| {
                        s.run_then("motion.renameLayer", json!({ "clipId": clip, "id": current, "newId": name }), cx, move |s, _, cx| {
                            for k in s.selection.iter_mut().filter(|k| **k == old_key) {
                                *k = new_key.clone();
                            }
                            if s.composition.as_deref() == old_key.strip_prefix(model::COMPOSITION) {
                                s.composition = new_key.strip_prefix(model::COMPOSITION).map(str::to_string);
                            }
                            s.changed(cx);
                        })
                    });
                }
                cx.notify();
            }
            InputEvent::Cancel => {
                this.rename = None;
                cx.notify();
            }
            InputEvent::Changed(_) => {}
        });
        self.rename = Some((key.to_string(), input, sub));
        cx.notify();
    }

    fn row_down(&mut self, row: &Row, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, _| s.area = Area::Outliner);
        if row.kind == RowKind::Header {
            return;
        }
        if e.click_count == 2 && row.kind != RowKind::Scene && row.key != "camera" {
            window.prevent_default();
            self.start_rename(&row.key, window, cx);
            return;
        }
        let additive = e.modifiers.shift || e.modifiers.platform || e.modifiers.control;
        let key = row.key.clone();
        let selected = self.studio.read(cx).selection.contains(&key);
        if !selected || additive {
            self.studio.update(cx, |s, cx| s.select(&key, additive, cx));
        } else {
            // A click on a selected row makes it the active one (drags keep the selection).
            self.studio.update(cx, |s, cx| {
                s.selection.retain(|k| *k != key);
                s.selection.push(key.clone());
                s.changed(cx);
            });
        }
        if row.kind == RowKind::Composition && e.click_count == 1 {
            // Showing a composition's layers in the canvas is a double-click away; one click selects.
        }
        if row.movable() {
            self.drag = Some(RowDrag { key: row.key.clone(), start: e.position, moving: false, drop: None });
        }
        cx.notify();
    }

    /// A click on a row (tests).
    #[cfg(test)]
    pub fn click(&mut self, key: &str, additive: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows(cx).into_iter().find(|r| r.key == key) else { panic!("no row {key}") };
        let mut e = MouseDownEvent { button: MouseButton::Left, position: Point::default(), modifiers: Default::default(), click_count: 1, first_mouse: false };
        e.modifiers.shift = additive;
        self.row_down(&row, &e, window, cx);
        self.drag = None;
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let top = self.scroll.bounds().origin.y;
        let offset = self.scroll.offset().y;
        let Some(d) = self.drag.as_mut() else { return };
        if !d.moving && f32::from((e.position.y - d.start.y).abs()) < 4. {
            return;
        }
        d.moving = true;
        let y = f32::from(e.position.y - top - offset);
        let i = (y / ROW_H).floor();
        d.drop = None;
        if i >= 0. && (i as usize) < rows.len() {
            let i = i as usize;
            let within = y - i as f32 * ROW_H;
            let r = &rows[i];
            let place = if r.container() && within > ROW_H * 0.3 && within < ROW_H * 0.7 {
                0
            } else if within < ROW_H / 2. {
                -1
            } else {
                1
            };
            let ok = r.key != d.key && (r.movable() || (r.kind == RowKind::Composition && place == 0) || (matches!(r.kind, RowKind::Header | RowKind::Scene) && place == 1));
            if ok {
                d.drop = Some(Drop { target: i, place });
            }
        }
        cx.notify();
    }

    fn drag_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        cx.notify();
        let (Some(drop), true) = (d.drop, d.moving) else { return };
        let rows = self.rows(cx);
        let (Some(from), Some(to)) = (rows.iter().find(|r| r.key == d.key), rows.get(drop.target)) else { return };
        let three = self.studio.read(cx).is_3d(cx);
        let Some((parent, index)) = move_target(from, to, drop.place, three) else { return };
        let clip = self.studio.read(cx).clip;
        let mut p = json!({ "clipId": clip, "id": from.key, "parent": parent });
        if let Some(i) = index {
            p["index"] = json!(i);
        }
        super::run("motion.moveLayer", p, cx);
    }
}

/// `motion.moveLayer`'s parent and index for dropping `from` above (−1), inside (0) or below
/// (1) `to`. 2D lists are shown top layer first, so "above" is later in the list.
pub fn move_target(from: &Row, to: &Row, place: i8, three: bool) -> Option<(String, Option<usize>)> {
    if place == 0 {
        let parent = to.key.strip_prefix(model::COMPOSITION).unwrap_or(&to.key).to_string();
        return Some((parent, None));
    }
    if matches!(to.kind, RowKind::Header | RowKind::Scene) {
        // Under a heading: to the top of the list.
        return Some((String::new(), Some(if three { 0 } else { 100_000 })));
    }
    let mut index = match (three, place) {
        (true, -1) => to.index,
        (true, _) => to.index + 1,
        (false, -1) => to.index + 1,
        (false, _) => to.index,
    };
    // Taken out of the same list first, the ones after it move up by one.
    if from.parent == to.parent && from.index < index {
        index -= 1;
    }
    Some((to.parent.clone(), Some(index)))
}

impl Render for Outliner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let rows = self.rows(cx);
        let st = self.studio.read(cx);
        let selection: HashSet<String> = st.selection.iter().cloned().collect();
        let active = st.active().map(str::to_string);
        let collapsed = st.collapsed.clone();
        let composition = st.composition.clone();
        let clip = st.clip;
        let studio = self.studio.clone();
        let drop = self.drag.as_ref().and_then(|d| d.drop.clone());
        let mut list = div().id("outliner-rows").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).flex().flex_col().pb(px(12.));
        for (i, row) in rows.iter().enumerate() {
            let sel = selection.contains(&row.key);
            let is_active = active.as_deref() == Some(row.key.as_str());
            let header = row.kind == RowKind::Header;
            let r = row.clone();
            let mut el = div()
                .id(SharedString::from(format!("row-{}", row.key)))
                .h(px(ROW_H))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.))
                .pl(px(8. + row.depth as f32 * 14.))
                .pr(px(6.))
                .relative()
                .text_size(px(if header { 10.5 } else { sz::SM }))
                .when(!header, |d| d.cursor_pointer().hover(|s| s.bg(t.hover)))
                .when(sel, |d| d.bg(if is_active { t.accent_soft } else { t.hover }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, w, cx| this.row_down(&r, e, w, cx)));
            if header {
                el = el.child(caps(row.label.clone(), cx));
            } else {
                // Fold.
                let key = row.key.clone();
                let s2 = studio.clone();
                el = el.child(
                    div()
                        .id(SharedString::from(format!("fold-{}", row.key)))
                        .w(px(12.))
                        .flex_none()
                        .text_color(t.text_3)
                        .when(row.has_children, |d| {
                            d.child(icon(if collapsed.contains(&row.key) { "chevron-right" } else { "chevron-down" }).size(px(11.)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| s2.update(cx, |s, cx| {
                                    if !s.collapsed.remove(&key) {
                                        s.collapsed.insert(key.clone());
                                    }
                                    cx.notify();
                                }))
                        }),
                );
                el = el.child(icon(row.icon()).text_color(if sel { t.accent_text } else { t.text_2 }));
                match &self.rename {
                    Some((k, input, _)) if *k == row.key => el = el.child(div().flex_1().min_w_0().child(input.clone())),
                    _ => {
                        el = el.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(if matches!(row.kind, RowKind::Scene) { crate::theme::SANS } else { MONO })
                                .when(row.hidden == Some(true), |d| d.text_color(t.text_3))
                                .when(is_active, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(row.label.clone()),
                        )
                    }
                }
                if !row.what.is_empty() && matches!(row.kind, RowKind::Object | RowKind::Layer | RowKind::Light) {
                    el = el.child(div().flex_none().text_size(px(10.)).text_color(t.text_3).child(row.what.clone()));
                }
                if let RowKind::Camera { active: true } = row.kind {
                    el = el.child(div().flex_none().px(px(5.)).bg(t.accent_soft).text_size(px(9.5)).text_color(t.accent_text).child("active"));
                }
                if row.kind == RowKind::Composition {
                    let open = composition.as_deref() == row.key.strip_prefix(model::COMPOSITION);
                    let s2 = studio.clone();
                    let id = row.key.strip_prefix(model::COMPOSITION).unwrap_or("").to_string();
                    el = el.child(
                        Button::icon(SharedString::from(format!("open-{}", row.key)), if open { "eye" } else { "layers" }, if open { "Showing this composition (click: back to the scene)" } else { "Show this composition in the canvas" })
                            .small()
                            .selected(open)
                            .on_click(move |_, _, cx| s2.update(cx, |s, cx| {
                                s.composition = if open { None } else { Some(id.clone()) };
                                s.changed(cx);
                            })),
                    );
                }
                if let Some(hidden) = row.hidden {
                    let id = row.key.clone();
                    el = el.child(
                        div()
                            .id(SharedString::from(format!("eye-{}", row.key)))
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(if hidden { t.text_3 } else { t.text_2 })
                            .hover(|s| s.bg(t.pressed))
                            .child(icon(if hidden { "eye-off" } else { "eye" }).size(px(12.)))
                            .tooltip(move |_, cx| crate::ui::tooltip(if hidden { "Show" } else { "Hide" }.into(), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                cx.store().update(cx, |s, cx| s.run("motion.updateLayer", json!({ "clipId": clip, "id": id, "props": { "hidden": !hidden } }), cx));
                            }),
                    );
                }
                let r = row.clone();
                let s2 = studio.clone();
                el = el.on_mouse_down(MouseButton::Right, move |e, _, cx| {
                    let key = r.key.clone();
                    if !s2.read(cx).selection.contains(&key) {
                        s2.update(cx, |s, cx| s.select(&key, false, cx));
                    }
                    let sel = s2.read(cx).selection.clone();
                    let entries = super::menus::thing_menu(&s2, sel, cx);
                    super::menus::open_menu(e.position, entries, cx);
                });
            }
            // Where a drag would land.
            if let Some(d) = drop.as_ref().filter(|d| d.target == i) {
                el = match d.place {
                    0 => el.child(div().absolute().inset_0().border_1().border_color(t.accent)),
                    -1 => el.child(div().absolute().top_0().left(px(8. + row.depth as f32 * 14.)).right_0().h(px(2.)).bg(t.accent)),
                    _ => el.child(div().absolute().bottom_0().left(px(8. + row.depth as f32 * 14.)).right_0().h(px(2.)).bg(t.accent)),
                };
            }
            list = list.child(el);
        }
        let s2 = self.studio.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg_raised)
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(div().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).child("Outliner"))
                    .child(Button::icon("outliner-add", "plus", crate::actions::tip("Add", &crate::actions::StudioAdd)).small().on_click(move |e, w, cx| {
                        let at = e.position();
                        s2.update(cx, |s, cx| s.open_add_menu(Some(at), w, cx))
                    })),
            )
            .child(list)
            .when(self.drag.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_end)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drops_land_where_they_show() {
        let s = kimchi_core::Scene::from_json(&json!({"layers": [{"id": "a", "type": "rect"}, {"id": "b", "type": "rect"}, {"id": "c", "type": "rect"}]})).unwrap();
        let rows = model::rows(&s, &Default::default());
        // Shown top first: c, b, a. Drag a above c: it becomes the top (last in the list).
        let (a, c) = (rows.iter().find(|r| r.key == "a").unwrap(), rows.iter().find(|r| r.key == "c").unwrap());
        assert_eq!(move_target(a, c, -1, false), Some((String::new(), Some(2))));
        // Drag c below a: the bottom (first).
        assert_eq!(move_target(c, a, 1, false), Some((String::new(), Some(0))));
        let s = kimchi_core::Scene::from_json(&json!({"objects": [{"id": "a", "type": "box"}, {"id": "b", "type": "box"}]})).unwrap();
        let rows = model::rows(&s, &Default::default());
        let (a, b) = (rows.iter().find(|r| r.key == "a").unwrap(), rows.iter().find(|r| r.key == "b").unwrap());
        assert_eq!(move_target(a, b, 1, true), Some((String::new(), Some(1))));
        assert_eq!(move_target(a, b, 0, true), Some(("b".into(), None)));
    }
}
