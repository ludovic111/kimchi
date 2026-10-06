//! The outliner: the scene as a tree. 3D: the world, cameras (the active one marked), lights,
//! objects with their children, shared materials. 2D: the scene, its layers (top of the list is
//! drawn on top, like After Effects), groups, compositions with their layers. Click to select
//! (Shift for a range, Cmd/Ctrl to add), double-click to rename, the eye to hide, drag to reorder or put inside
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

#[derive(Clone,Copy)]
pub enum KeyNav {
    Previous {extend:bool},
    Next {extend:bool},
    Collapse,
    Expand,
}

/// Where a dragged row would land.
#[derive(Clone, Debug, PartialEq)]
pub struct Drop {
    /// The row it is dropped on.
    pub target: usize,
    /// −1 above it, 0 inside it, 1 below it.
    pub place: i8,
}

struct RowDrag {
    keys: Vec<String>,
    clip: Option<kimchi_core::Id>,
    tree: Vec<(String,String,usize)>,
    start: Point<Pixels>,
    moving: bool,
    drop: Option<Drop>,
}

pub struct Outliner {
    studio: Entity<Studio>,
    scroll: ScrollHandle,
    search: Entity<TextInput>,
    rename: Option<(String, Entity<TextInput>, Subscription)>,
    drag: Option<RowDrag>,
    anchor: Option<(kimchi_core::Id, String)>,
    _subs: Vec<Subscription>,
}

impl Outliner {
    pub fn new(studio: Entity<Studio>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Find objects, lights, materials…"));
        let subs = vec![cx.subscribe(&search, |this: &mut Self, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Changed(_)) { this.drag = None; this.scroll.set_offset(gpui::point(px(0.), px(0.))); cx.notify(); }
        }), cx.observe(&studio, |_, _, cx| cx.notify()), cx.observe(&store, |_, _, cx| cx.notify())];
        Self { studio, scroll: ScrollHandle::new(), search, rename: None, drag: None, anchor: None, _subs: subs }
    }

    fn rows(&self, cx: &gpui::App) -> Vec<Row> {
        let st = self.studio.read(cx);
        match st.clip_scene(cx) {
            Some((_, scene)) => filtered_rows(&scene, &st.collapsed, self.search.read(cx).text()),
            None => vec![],
        }
    }

    pub fn navigate(&mut self, key: KeyNav, cx: &mut Context<Self>) {
        let rows=self.rows(cx);
        let st=self.studio.read(cx);
        let Some((clip,scene))=st.clip_scene(cx) else {return};
        let active=st.active().map(str::to_string);
        let mut current=active.as_ref().and_then(|key| rows.iter().position(|r| &r.key==key));
        // A selected child may have been folded away. Start from its nearest visible parent.
        if current.is_none() && let Some(active)=active.as_deref() {
            let all=model::rows(&scene,&HashSet::new());
            let mut at=all.iter().position(|r| r.key==active);
            while let Some(parent)=at.and_then(|i| parent_row(&all,i)) {
                if let Some(i)=rows.iter().position(|r| r.key==all[parent].key) {current=Some(i);break;}
                at=Some(parent);
            }
        }
        let searching=!self.search.read(cx).text().trim().is_empty();
        let target=match key {
            KeyNav::Previous {..}=>rows.iter().enumerate().rev().find(|(i,r)| current.is_none_or(|at| *i<at) && r.kind!=RowKind::Header).map(|(i,_)| i),
            KeyNav::Next {..}=>rows.iter().enumerate().find(|(i,r)| current.is_none_or(|at| *i>at) && r.kind!=RowKind::Header).map(|(i,_)| i),
            KeyNav::Collapse | KeyNav::Expand=>{
                let Some(i)=current else {return};
                let row=&rows[i];
                let collapse=matches!(key,KeyNav::Collapse);
                let folded=st.collapsed.contains(&row.key);
                if row.has_children && !searching && collapse!=folded {
                    let id=row.key.clone();
                    self.drag=None;
                    self.studio.update(cx,|s,cx| {
                        if collapse {s.collapsed.insert(id);} else {s.collapsed.remove(&id);}
                        s.changed(cx);
                    });
                    self.scroll.scroll_to_item(i);cx.notify();return;
                }
                if collapse {parent_row(&rows,i)} else {
                    rows.get(i+1).filter(|r| r.depth>row.depth && r.kind!=RowKind::Header).map(|_| i+1)
                }
            }
        };
        let Some(target)=target else {return};
        let id=rows[target].key.clone();
        let extend=matches!(key,KeyNav::Previous {extend:true}|KeyNav::Next {extend:true});
        let picked=if extend {
            let anchor=self.anchor.as_ref().filter(|(owner,id)| *owner==clip.id && st.selection.contains(id) && rows.iter().any(|r| r.key==*id))
                .map(|(_,id)| id.clone()).or_else(|| current.map(|i| rows[i].key.clone())).unwrap_or_else(|| id.clone());
            let picked=range_keys(&rows,&anchor,&id);
            self.anchor=Some((clip.id,anchor));picked
        } else {
            self.anchor=Some((clip.id,id.clone()));vec![id]
        };
        self.drag=None;
        self.studio.update(cx,|s,cx| s.set_selection(picked,cx));
        self.scroll.scroll_to_item(target);
        cx.notify();
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
                    let namespace = if key_s.starts_with(model::MATERIAL) { "material" } else { "scene" };
                    this.studio.update(cx, |s, cx| {
                        let before = s.selection.clone();
                        let pruned: Vec<_> = before.iter().filter(|k| **k != old_key).cloned().collect();
                        let composition = s.composition.clone();
                        s.run_then("motion.renameLayer", json!({ "clipId": clip, "id": current, "newId": name, "namespace": namespace }), cx, move |s, _, cx| {
                            if s.clip != clip { return; }
                            // A project refresh can already have pruned the previous id.
                            if s.selection == before || s.selection == pruned {
                                s.selection = before.into_iter().map(|k| if k == old_key { new_key.clone() } else { k }).collect();
                            }
                            if composition.as_deref().is_some_and(|c| Some(c) == old_key.strip_prefix(model::COMPOSITION))
                                && (s.composition.is_none() || s.composition == composition) {
                                s.composition = new_key.strip_prefix(model::COMPOSITION).map(str::to_string);
                            }
                            if namespace == "scene" {
                                for key in s.keys.iter_mut().filter(|k| k.id == current) { key.id = name.clone(); }
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

    fn take_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx,|s,cx| {s.focus_area(Area::Outliner,cx);window.focus(&s.focus,cx);});
        window.prevent_default();
    }

    fn row_down(&mut self, row: &Row, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.take_focus(window,cx);
        if row.kind == RowKind::Header {
            return;
        }
        if e.click_count == 2 && row.kind != RowKind::Scene && row.key != "camera" {
            window.prevent_default();
            self.start_rename(&row.key, window, cx);
            return;
        }
        let additive = e.modifiers.platform || e.modifiers.control;
        let key = row.key.clone();
        let clip = self.studio.read(cx).clip;
        if e.modifiers.shift {
            let st = self.studio.read(cx);
            let anchor = self.anchor.as_ref().filter(|(id, _)| Some(*id) == clip).map(|(_, id)| id.clone())
                .or_else(|| st.active().map(str::to_string)).unwrap_or_else(|| key.clone());
            let picked = range_keys(&self.rows(cx), &anchor, &key);
            let mut selection = if additive { st.selection.clone() } else { vec![] };
            for id in picked { if !selection.contains(&id) { selection.push(id); } }
            selection.retain(|id| id != &key);
            selection.push(key.clone());
            self.anchor = clip.map(|clip| (clip, anchor));
            self.drag = None;
            self.studio.update(cx, |s, cx| s.set_selection(selection, cx));
            cx.notify();
            return;
        }
        self.anchor = clip.map(|clip| (clip, key.clone()));
        let selected = self.studio.read(cx).selection.contains(&key);
        if additive {
            self.studio.update(cx, |s, cx| {
                let mut selection = s.selection.clone();
                if selected { selection.retain(|id| id != &key); } else { selection.push(key.clone()); }
                s.set_selection(selection, cx);
            });
        } else if !selected {
            self.studio.update(cx, |s, cx| s.select(&key, false, cx));
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
        if row.movable() && self.studio.read(cx).selection.contains(&key) && self.search.read(cx).text().trim().is_empty() {
            let st=self.studio.read(cx);
            if let Some((_,scene))=st.clip_scene(cx) {
                let rows=model::rows(&scene,&Default::default());
                let keys=rows.iter().filter(|r| r.movable() && st.selection.contains(&r.key)).map(|r| r.key.clone()).collect();
                self.drag = Some(RowDrag {keys,clip,tree:tree_order(&rows),start:e.position,moving:false,drop:None});
            }
        }
        cx.notify();
    }

    /// A click on a row (tests).
    #[cfg(test)]
    pub fn click(&mut self, key: &str, additive: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.click_with_modifiers(key, gpui::Modifiers { control: additive, ..Default::default() }, window, cx);
    }

    #[cfg(test)]
    pub fn click_with_modifiers(&mut self, key: &str, modifiers: gpui::Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows(cx).into_iter().find(|r| r.key == key) else { panic!("no row {key}") };
        let e = MouseDownEvent { button: MouseButton::Left, position: Point::default(), modifiers, click_count: 1, first_mouse: false };
        self.row_down(&row, &e, window, cx);
        self.drag = None;
    }

    #[cfg(test)]
    pub fn row_position_for_test(&self, key: &str, cx: &gpui::App) -> Point<Pixels> {
        let i=self.rows(cx).iter().position(|r| r.key==key).expect("the visible row");
        self.scroll.bounds().origin+self.scroll.offset()+gpui::point(px(90.),px((i as f32+0.5)*ROW_H))
    }

    #[cfg(test)]
    pub fn search_for_test(&self) -> Entity<TextInput> {self.search.clone()}

    #[cfg(test)]
    pub fn rows_bounds_for_test(&self) -> gpui::Bounds<Pixels> {self.scroll.bounds()}

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if e.pressed_button!=Some(MouseButton::Left) {self.cancel_drag(cx);return;}
        let rows = self.rows(cx);
        let Some((_,scene))=self.studio.read(cx).clip_scene(cx) else {self.cancel_drag(cx);return};
        let all=model::rows(&scene,&Default::default());
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
            if selection_target(&all,&d.keys,r,place,scene.is_3d()).is_some() {
                d.drop = Some(Drop { target: i, place });
            }
        }
        cx.notify();
    }

    fn drag_end(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.button!=MouseButton::Left {self.cancel_drag(cx);return;}
        let Some(d) = self.drag.take() else { return };
        cx.notify();
        let (Some(drop), true) = (d.drop, d.moving) else { return };
        let rows = self.rows(cx);
        let Some(to)=rows.get(drop.target) else {return};
        let st=self.studio.read(cx);
        let Some((clip,scene))=st.clip_scene(cx) else {return};
        let all=model::rows(&scene,&Default::default());
        if Some(clip.id)!=d.clip || tree_order(&all)!=d.tree || !d.keys.iter().all(|id| st.selection.contains(id)) {return;}
        let Some((parent,index))=selection_target(&all,&d.keys,to,drop.place,scene.is_3d()) else {return};
        let mut p = json!({ "clipId": clip.id, "ids": d.keys, "parent": parent });
        if let Some(i) = index {
            p["index"] = json!(i);
        }
        super::run("motion.moveLayers", p, cx);
    }

    pub fn cancel_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let cancelled=self.drag.take().is_some();
        if cancelled {cx.notify();}
        cancelled
    }

    #[cfg(test)]
    pub fn drop_for_test(&mut self, key: &str, target: &str, place: i8, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_drag_for_test(key,target,place,window,cx);
        self.finish_drag_for_test(window,cx);
    }

    #[cfg(test)]
    pub fn begin_drag_for_test(&mut self, key: &str, target: &str, place: i8, window: &mut Window, cx: &mut Context<Self>) {
        let rows=self.rows(cx);
        let from=rows.iter().find(|r| r.key==key).expect("source");
        self.row_down(from,&MouseDownEvent {button:MouseButton::Left,position:Point::default(),modifiers:Default::default(),click_count:1,first_mouse:false},window,cx);
        if let Some(d)=&mut self.drag {d.moving=true;d.drop=Some(Drop {target:rows.iter().position(|r| r.key==target).expect("target"),place});}
    }

    #[cfg(test)]
    pub fn finish_drag_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drag_end(&MouseUpEvent {button:MouseButton::Left,position:Point::default(),modifiers:Default::default(),click_count:1},window,cx);
    }
}

fn tree_order(rows: &[Row]) -> Vec<(String,String,usize)> {
    rows.iter().filter(|r| r.movable()).map(|r| (r.key.clone(),r.parent.clone(),r.index)).collect()
}

/// Parent and insertion index after removing the selected roots. 2D is shown topmost first.
fn selection_target(rows: &[Row], keys: &[String], to: &Row, place: i8, three: bool) -> Option<(String,Option<usize>)> {
    let selected_ancestor=|key:&str| {
        let Some(index)=rows.iter().position(|r| r.key==key) else {return false};
        let mut depth=rows[index].depth;
        for row in rows[..index].iter().rev() {
            if row.depth<depth {
                if keys.contains(&row.key) {return true;}
                depth=row.depth;
                if depth==0 {break;}
            }
        }
        false
    };
    if keys.is_empty() || keys.contains(&to.key) || selected_ancestor(&to.key) {return None;}
    let roots:Vec<_>=rows.iter().filter(|r| r.movable() && keys.contains(&r.key) && !selected_ancestor(&r.key)).collect();
    if roots.is_empty() {return None;}
    if place == 0 {
        if !to.container() {return None;}
        let parent = to.key.strip_prefix(model::COMPOSITION).unwrap_or(&to.key).to_string();
        return Some((parent, None));
    }
    if matches!(to.kind, RowKind::Header | RowKind::Scene) {
        if place!=1 || (to.kind==RowKind::Header && to.key!="#objects") {return None;}
        // Under a heading: to the top of the list.
        return Some((String::new(), Some(if three { 0 } else { 100_000 })));
    }
    if !to.movable() {return None;}
    let index = match (three, place) {
        (true, -1) => to.index,
        (true, _) => to.index + 1,
        (false, -1) => to.index + 1,
        (false, _) => to.index,
    };
    let list_key=|key:&str| {
        let i=rows.iter().position(|r| r.key==key)?;
        rows[..i].iter().rev().find(|r| r.depth<rows[i].depth).map(|r| r.key.as_str())
    };
    let index=index-roots.iter().filter(|r| r.parent==to.parent && list_key(&r.key)==list_key(&to.key) && r.index<index).count();
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
        let searching = !self.search.read(cx).text().trim().is_empty();
        let drop = self.drag.as_ref().and_then(|d| d.drop.clone());
        let heading=self.drag.as_ref().filter(|d| d.moving).map(|d| format!("Moving {} selected · Esc cancels",d.keys.len())).unwrap_or_else(|| "Outliner".into());
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
                            d.child(icon(if !searching && collapsed.contains(&row.key) { "chevron-right" } else { "chevron-down" }).size(px(11.)))
                                .on_mouse_down(MouseButton::Left, cx.listener(|this,_,window,cx| {this.take_focus(window,cx);cx.stop_propagation();}))
                                .on_click(move |_, _, cx| s2.update(cx, |s, cx| {
                                    if searching { return; }
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
                            .on_mouse_down(MouseButton::Left, cx.listener(|this,_,window,cx| {this.take_focus(window,cx);cx.stop_propagation();}))
                            .on_click(move |_, _, cx| {
                                cx.store().update(cx, |s, cx| s.run("motion.updateLayer", json!({ "clipId": clip, "id": id, "props": { "hidden": !hidden } }), cx));
                            }),
                    );
                }
                let r = row.clone();
                let s2 = studio.clone();
                el = el.on_mouse_down(MouseButton::Right, move |e, _, cx| {
                    s2.update(cx,|s,cx| s.focus_area(Area::Outliner,cx));
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
        if rows.is_empty() {
            list = list.child(div().p(px(12.)).text_size(px(sz::SM)).text_color(t.text_3).child("No matching scene items."));
        }
        let s2 = self.studio.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg_raised)
            .on_mouse_up(MouseButton::Left,cx.listener(Self::drag_end))
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
                    .child(div().id("outliner-heading").min_w_0().truncate().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).child(heading)
                        .tooltip(|_, cx| {
                            use crate::actions::{hint,PrevEdit,NextEdit,StepBack,StepForward,StudioExtendPreviousItem,StudioExtendNextItem};
                            crate::ui::tooltip(format!("{} / {} selects · {} / {} folds groups · {} / {} extends a range · Shift-click selects a range · {} adds or removes items",
                                hint(&PrevEdit).unwrap_or_default(),hint(&NextEdit).unwrap_or_default(),hint(&StepBack).unwrap_or_default(),hint(&StepForward).unwrap_or_default(),
                                hint(&StudioExtendPreviousItem).unwrap_or_default(),hint(&StudioExtendNextItem).unwrap_or_default(),crate::actions::keys_label("M-click")).into(),cx)
                        }))
                    .child(Button::icon("outliner-add", "plus", crate::actions::tip("Add", &crate::actions::StudioAdd)).small().on_click(move |e, w, cx| {
                        let at = e.position();
                        s2.update(cx, |s, cx| s.open_add_menu(Some(at), w, cx))
                    })),
            )
            .child(div().flex_none().p(px(8.)).flex().items_center().gap(px(4.))
                .child(div().flex_1().min_w_0().child(self.search.clone()))
                .when(searching, |d| d.child(Button::icon("clear-scene-search", "x", "Clear search").small().ghost()
                    .on_click(cx.listener(|this, _, _, cx| { this.search.update(cx, |s, cx| s.set_text("", cx)); cx.notify(); })))))
            .child(list)
            .child(div().flex_none().p(px(8.)).border_t_1().border_color(t.line).flex().flex_wrap().gap(px(4.))
                .child(Button::new("outliner-frame", "Frame").small().ghost().with_icon("maximize-2")
                    .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.frame_selection(true, cx)))))
                .child(Button::new("outliner-properties", "Properties").small().ghost().with_icon("sliders-horizontal").disabled(active.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let sidebar = this.studio.read(cx).sidebar.clone();
                        sidebar.update(cx, |s, cx| s.show_properties(cx));
                    }))))
            .when(self.drag.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_end)))
    }
}

/// Search includes matching items, their descendants and the ancestors that explain the
/// hierarchy. It expands matches temporarily without changing the person's folded branches.
fn filtered_rows(scene: &kimchi_core::Scene, collapsed: &HashSet<String>, query: &str) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    if query.is_empty() { return model::rows(scene, collapsed); }
    let rows = model::rows(scene, &HashSet::new());
    let mut keep = vec![false; rows.len()];
    let mut ancestors: Vec<usize> = vec![];
    let mut matched_depth = None;
    for (i, row) in rows.iter().enumerate() {
        while ancestors.last().is_some_and(|&j| rows[j].depth >= row.depth) { ancestors.pop(); }
        if matched_depth.is_some_and(|depth| row.depth <= depth) { matched_depth = None; }
        let matches = format!("{} {} {:?}", row.label, row.what, row.kind).to_lowercase().contains(&query);
        if matches || matched_depth.is_some() {
            keep[i] = true;
            for &a in &ancestors { keep[a] = true; }
            if matches && matched_depth.is_none() { matched_depth = Some(row.depth); }
        }
        ancestors.push(i);
    }
    rows.into_iter().zip(keep).filter_map(|(row, keep)| keep.then_some(row)).collect()
}

/// Only visible selectable rows participate; walking toward the click makes it active last.
fn range_keys(rows: &[Row], anchor: &str, target: &str) -> Vec<String> {
    let Some(end) = rows.iter().position(|r| r.key == target) else { return vec![] };
    let start = rows.iter().position(|r| r.key == anchor).unwrap_or(end);
    let mut picked: Vec<_> = rows[start.min(end)..=start.max(end)].iter().filter(|r| r.kind != RowKind::Header).map(|r| r.key.clone()).collect();
    if start > end { picked.reverse(); }
    picked
}

fn parent_row(rows: &[Row], index: usize) -> Option<usize> {
    rows[..index].iter().rposition(|r| r.depth<rows[index].depth).filter(|i| rows[*i].kind!=RowKind::Header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_reveals_collapsed_descendants_and_keeps_their_ancestors() {
        let scene = kimchi_core::Scene::from_json(&json!({ "type": "3d", "objects": [
            { "id": "assembly", "type": "group", "children": [
                { "id": "wheel", "type": "sphere" }, { "id": "axle", "type": "cylinder" }
            ] }, { "id": "other", "type": "box" }
        ] })).unwrap();
        let collapsed = HashSet::from(["assembly".to_string()]);
        let keys = |q: &str| filtered_rows(&scene, &collapsed, q).into_iter().map(|r| r.key).collect::<Vec<_>>();
        assert_eq!(keys(" WHEEL "), ["#objects", "assembly", "wheel"]);
        assert_eq!(keys("cylinder"), ["#objects", "assembly", "axle"]);
        assert_eq!(keys("assembly"), ["#objects", "assembly", "wheel", "axle"]);
        assert!(!keys("").contains(&"wheel".into()), "search did not change the folded state");
        assert!(keys("no such item").is_empty());
    }

    #[test]
    fn drops_land_where_they_show() {
        let s = kimchi_core::Scene::from_json(&json!({"layers": [{"id": "a", "type": "rect"}, {"id": "b", "type": "rect"}, {"id": "c", "type": "rect"}]})).unwrap();
        let rows = model::rows(&s, &Default::default());
        // Shown top first: c, b, a. Drag a above c: it becomes the top (last in the list).
        let (a, c) = (rows.iter().find(|r| r.key == "a").unwrap(), rows.iter().find(|r| r.key == "c").unwrap());
        assert_eq!(selection_target(&rows, std::slice::from_ref(&a.key), c, -1, false), Some((String::new(), Some(2))));
        // Drag c below a: the bottom (first).
        assert_eq!(selection_target(&rows, std::slice::from_ref(&c.key), a, 1, false), Some((String::new(), Some(0))));
        let s = kimchi_core::Scene::from_json(&json!({"objects": [{"id": "a", "type": "box"}, {"id": "b", "type": "box"}]})).unwrap();
        let rows = model::rows(&s, &Default::default());
        let (a, b) = (rows.iter().find(|r| r.key == "a").unwrap(), rows.iter().find(|r| r.key == "b").unwrap());
        assert_eq!(selection_target(&rows, std::slice::from_ref(&a.key), b, 1, true), Some((String::new(), Some(1))));
        assert_eq!(selection_target(&rows, std::slice::from_ref(&a.key), b, 0, true), Some(("b".into(), None)));
    }

    #[test]
    fn range_selection_uses_visible_rows_and_keeps_the_clicked_end_active() {
        let scene = kimchi_core::Scene::from_json(&json!({"objects":[
            {"id":"a","type":"box"}, {"id":"group","type":"group","children":[{"id":"hidden","type":"box"}]},
            {"id":"b","type":"sphere"}, {"id":"c","type":"box"}
        ]})).unwrap();
        let rows = filtered_rows(&scene, &HashSet::from(["group".into()]), "");
        assert_eq!(range_keys(&rows, "a", "c"), ["a","group","b","c"]);
        assert_eq!(range_keys(&rows, "camera", "a"), ["camera","a"], "section headings are not selectable");
        assert_eq!(range_keys(&rows, "c", "a"), ["c","b","group","a"]);
        assert_eq!(range_keys(&rows, "hidden", "b"), ["b"]);
        let rows = filtered_rows(&scene, &HashSet::new(), "box");
        assert_eq!(range_keys(&rows, "a", "c"), ["a","group","hidden","c"]);
        assert!(!range_keys(&rows, "camera", "c").iter().any(|id| id.starts_with('#')));
    }
}
