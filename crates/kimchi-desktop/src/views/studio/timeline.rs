//! The Studio's timeline area, over the clip's scene time: a ruler with the playhead (drag to
//! scrub), play / pause looping the clip, and either the dope sheet (a row per thing, opened to
//! its animated properties, with keyframe diamonds to select, drag in time, delete, ease) or the
//! graph editor (the selected property's curves: drag keys up and down or in time, shape a
//! segment's easing with two bezier handles).

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, Render,
    SharedString, Subscription, Window, canvas, div, point, prelude::*, px,
};
use kimchi_core::anim::value_at;
use kimchi_core::{Easing, KeyValue, Keyframe, Scene};
use serde_json::{Value, json};

use super::model;
use super::selection::SelectionOp;
use super::{Area, KeyRef, Studio};
use crate::actions::{self as act, tip};
use crate::store::{MenuEntry, MenuItem, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, drag, icon};

mod retime;
use retime::TimeTransform;

const NAMES_W: f32 = 210.;
const ROW_H: f32 = 22.;
const RULER_H: f32 = 22.;
pub const MAX_TIME: f64 = 1_000_000.;

pub fn valid_range(start: f64, end: f64) -> Result<(f64, f64), String> {
    if !start.is_finite() || !end.is_finite() || start < 0. || end > MAX_TIME || end - start < 1e-6 {
        return Err("Timeline range must run forwards between 0 and 1000000 scene seconds.".into());
    }
    Ok((start, end))
}

pub fn clip_span(clip: &kimchi_core::Clip) -> (f64, f64) {
    let (a, b) = (model::scene_time(clip, clip.start), model::scene_time(clip, clip.end()));
    let (a, b) = (a.min(b), a.max(b));
    (a, if b - a < 0.05 { a + 1.0 } else { b })
}

pub fn zoom_range(span: (f64, f64), factor: f64, anchor: f64) -> (f64, f64) {
    if !factor.is_finite() || factor <= 0. || !anchor.is_finite() { return span; }
    let width = ((span.1 - span.0) / factor).clamp(0.001, MAX_TIME);
    let fraction = ((anchor - span.0) / (span.1 - span.0)).clamp(0., 1.);
    let start = (anchor - fraction * width).clamp(0., MAX_TIME - width);
    (start, start + width)
}

pub fn pan_range(span: (f64, f64), delta: f64) -> (f64, f64) {
    if !delta.is_finite() { return span; }
    let width = (span.1 - span.0).min(MAX_TIME);
    let start = (span.0 + delta).clamp(0., MAX_TIME - width);
    (start, start + width)
}

fn ruler_step(span: (f64, f64), width: f64) -> f64 {
    let wanted = (span.1 - span.0) / (width / 80.).max(2.);
    let magnitude = 10_f64.powf(wanted.log10().floor());
    [1., 2., 5., 10.].into_iter().map(|n| n * magnitude).find(|n| *n >= wanted).unwrap_or(wanted)
}

fn fitted_values(list: &[Keyframe], span: (f64, f64)) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let samples = (0..=120).flat_map(|i| values(value_at(list, span.0 + (span.1 - span.0) * i as f64 / 120.)));
    let keys = list.iter().filter(|k| (span.0..=span.1).contains(&k.time)).flat_map(|k| values(Some(k.value.clone())));
    for v in samples.chain(keys).filter(|v| v.is_finite()) {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if !lo.is_finite() { return (0., 1.); }
    let pad = ((hi - lo) * 0.12).max(1e-3);
    (lo - pad, hi + pad)
}

fn value_label(value: f64, step: f64) -> String {
    let value = if value.abs() < step * 1e-6 { 0. } else { value };
    if value != 0. && (value.abs() >= 1e7 || step < 1e-6) { return format!("{value:.3e}"); }
    let precision = (-step.log10().floor()).clamp(0., 6.) as usize;
    let label = format!("{value:.precision$}");
    if precision > 0 { label.trim_end_matches('0').trim_end_matches('.').to_string() } else { label }
}

fn key_delta(keys: &[KeyRef], delta: f64) -> f64 {
    delta.max(-keys.iter().map(|k| k.time).fold(f64::INFINITY, f64::min))
}

/// Project frames are in timeline time; scene seconds also include the clip's trim and speed.
fn snap_key_time(clip: &kimchi_core::Clip, time: f64, fps: f64, free: bool) -> f64 {
    if free { return time; }
    let fps=fps.max(1.);
    clip.scene_time((model::timeline_time(clip,time)*fps).round()/fps)
}

fn shifted_value(value: &KeyValue, component: Option<usize>, delta: f64) -> KeyValue {
    let mut value = value.clone();
    match &mut value {
        KeyValue::Number(v) => *v += delta,
        KeyValue::Vector(v) => { if let Some(i) = component.and_then(|i| v.get_mut(i)) { *i += delta; } }
        _ => {}
    }
    value
}

/// Match the committed edit: remove sources together, then replace destination collisions.
fn shifted_curve(list: &[Keyframe], picked: &[KeyRef], component: Option<usize>, dt: f64, dv: f64) -> Vec<Keyframe> {
    let selected = |key: &Keyframe| picked.iter().any(|p| (p.time - key.time).abs() < 1e-6);
    let mut preview: Vec<_> = list.iter().filter(|k| !selected(k)).cloned().collect();
    for key in list.iter().filter(|k| selected(k)) {
        let time = key.time + dt;
        preview.retain(|k| (k.time - time).abs() >= 1e-6);
        preview.push(Keyframe { time, value: shifted_value(&key.value, component, dv), easing: key.easing });
    }
    preview.sort_by(|a,b| a.time.total_cmp(&b.time));
    preview
}

/// Easings offered on a keyframe's menu: (easing, label).
pub const EASINGS: &[(&str, &str)] = &[
    ("linear", "Linear"),
    ("easeIn", "Ease in"),
    ("easeOut", "Ease out"),
    ("easeInOut", "Ease in and out"),
    ("ease", "Ease"),
    ("hold", "Hold (jump)"),
    ("easeOutBack", "Back (overshoot)"),
    ("easeOutElastic", "Elastic"),
    ("easeOutBounce", "Bounce"),
    ("spring", "Spring"),
    ("easeInOutExpo", "Expo in and out"),
    ("easeInOutSine", "Sine in and out"),
];

/// One row of the dope sheet: a thing (all its keys) or one of its properties.
#[derive(Clone, Debug)]
struct SheetRow {
    id: String,
    property: Option<String>,
    label: String,
    times: Vec<f64>,
}

#[derive(Clone, Debug)]
enum TDrag {
    Scrub,
    /// Moving the selected keys: where the drag started (scene seconds) and by how much.
    Keys { t0: f64, anchor: f64, dt: f64, moved: bool },
    /// A rubber band over keys (scene seconds, rows).
    Band { from: (f64, f32), to: (f64, f32), selection: SelectionOp },
    /// Graph selection rectangle in scene time and value, on the visible property.
    GraphBand { id: String, property: String, from: (f64, f64), to: (f64, f64), selection: SelectionOp },
    /// Graph: move the picked keys using one grabbed component as the pointer anchor.
    GraphKey { key: KeyRef, comp: Option<usize>, picked: Vec<KeyRef>, from: (f64, f64), to: (f64, f64) },
    /// Graph: a bezier handle of the segment into `key` (0 = first, 1 = second).
    Handle { key: KeyRef, component: usize, which: usize, ctrl: [f64; 4], moved: bool },
}

pub struct StudioTimeline {
    studio: Entity<Studio>,
    bounds: Rc<std::cell::Cell<Bounds<Pixels>>>,
    drag: Option<TDrag>,
    transform: Option<TimeTransform>,
    key_capture: Option<Subscription>,
    box_armed: bool,
    /// Graph editor: the value range shown (fitted when `None`).
    range: Option<(f64, f64)>,
    /// Dope sheet: pixels scrolled down.
    scroll_y: f32,
    last_clip: Option<kimchi_core::Id>,
    last_graph: Option<(kimchi_core::Id, String, String)>,
    last_mode: bool,
    _subs: Vec<Subscription>,
}

impl StudioTimeline {
    pub fn new(studio: Entity<Studio>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![cx.observe(&studio, |_, _, cx| cx.notify()), cx.observe(&store, |_, _, cx| cx.notify()), cx.observe(&playback, |_, _, cx| cx.notify())];
        Self { studio, bounds: Rc::new(std::cell::Cell::new(Bounds::default())), drag: None, transform: None, key_capture: None, box_armed: false, range: None, scroll_y: 0., last_clip: None, last_graph: None, last_mode: false, _subs: subs }
    }

    pub fn cancel_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let cancelled = self.drag.take().is_some();
        let cancelled = self.transform.take().is_some() || cancelled;
        self.key_capture = None;
        let cancelled = std::mem::take(&mut self.box_armed) || cancelled;
        if cancelled { cx.notify(); }
        cancelled
    }

    pub fn arm_box_select(&mut self, cx: &mut Context<Self>) {
        self.cancel_drag(cx);
        self.box_armed = true;
        cx.notify();
    }

    pub fn disarm_box_select(&mut self,cx:&mut Context<Self>) {
        if std::mem::take(&mut self.box_armed) {cx.notify();}
    }

    #[cfg(test)]
    pub fn box_armed_for_test(&self)->bool {self.box_armed}

    fn span(&self, cx: &App) -> (f64, f64) {
        self.studio.read(cx).timeline_span(cx)
    }

    /// The track area (right of the names), screen pixels.
    fn track_box(&self) -> (f64, f64, f64, f64) {
        let b = self.bounds.get();
        (f32::from(b.origin.x) as f64, f32::from(b.origin.y) as f64, f32::from(b.size.width) as f64, f32::from(b.size.height) as f64)
    }

    fn x_of(&self, t: f64, span: (f64, f64)) -> f64 {
        let (x, _, w, _) = self.track_box();
        x + 8.0 + (t - span.0) / (span.1 - span.0) * (w - 16.0).max(1.0)
    }

    fn t_of(&self, x: f64, span: (f64, f64)) -> f64 {
        let (bx, _, w, _) = self.track_box();
        span.0 + (x - bx - 8.0) / (w - 16.0).max(1.0) * (span.1 - span.0)
    }

    fn rows(&self, scene: &Scene, cx: &App) -> Vec<SheetRow> {
        let st = self.studio.read(cx);
        let mut ids = model::thing_ids(scene);
        ids.insert(0, "scene".into());
        let mut out = vec![];
        for id in ids {
            let keys = model::keyframes(scene, &id).unwrap_or_default();
            if keys.is_empty() && !st.selection.contains(&id) {
                continue;
            }
            let mut times: Vec<f64> = keys.values().flatten().map(|k| k.time).collect();
            times.sort_by(f64::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            out.push(SheetRow { id: id.clone(), property: None, label: if id == "scene" { "Scene".into() } else { id.clone() }, times });
            if st.expanded.contains(&id) {
                for (name, list) in &keys {
                    out.push(SheetRow { id: id.clone(), property: Some(name.clone()), label: name.clone(), times: list.iter().map(|k| k.time).collect() });
                }
            }
        }
        out
    }

    pub fn select_all_keys(&mut self, cx: &mut Context<Self>) {
        self.cancel_drag(cx);
        let Some((_, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let graph = self.studio.read(cx).show_graph;
        let target = self.graph_target(&scene, cx);
        let mut all = vec![];
        for id in std::iter::once("scene".to_string()).chain(model::thing_ids(&scene)) {
            for (name, list) in model::keyframes(&scene, &id).unwrap_or_default() {
                if graph && target.as_ref().is_none_or(|(i,p)| *i != id || *p != name) { continue; }
                all.extend(list.iter().map(|k| KeyRef { id: id.clone(), property: name.clone(), time: k.time }));
            }
        }
        self.studio.update(cx, |s, cx| {
            s.keys = if s.keys.len() == all.len() && all.iter().all(|k| s.keys.contains(k)) { vec![] } else { all };
            s.changed(cx);
        });
    }

    /// Navigate the visible graph channel, selected dope-sheet items, or the scene when none are selected.
    fn adjacent_key(&self, forward: bool, clip: &kimchi_core::Clip, scene: &Scene, cx: &App) -> Option<f64> {
        let st=self.studio.read(cx);
        let now=st.scene_time(cx);
        let (start,end)=(clip.scene_time(clip.start),clip.scene_time(clip.end()));
        let extent=(start.min(end),start.max(end));
        let graph=if st.show_graph {Some(self.graph_target(scene,cx)?)} else {None};
        let mut ids=match &graph {
            Some((id,_)) => vec![id.clone()],
            None => st.selection.iter().filter(|id| model::is_thing(scene,id) || id.as_str()=="scene").cloned().collect(),
        };
        if ids.is_empty() {ids=std::iter::once("scene".into()).chain(model::thing_ids(scene)).collect();}
        let mut found:Option<f64>=None;
        for id in ids {
            for (property,keys) in model::keyframes(scene,&id).unwrap_or_default() {
                if graph.as_ref().is_some_and(|(_,p)| *p!=property) {continue;}
                let index=keys.partition_point(|key| if forward {key.time<=now+1e-6} else {key.time<now-1e-6});
                let key=if forward {keys.get(index)} else {index.checked_sub(1).and_then(|i| keys.get(i))};
                let Some(key)=key.filter(|k| k.time>=extent.0-1e-6 && k.time<=extent.1+1e-6) else {continue};
                found=Some(found.map_or(key.time,|at| if forward {at.min(key.time)} else {at.max(key.time)}));
            }
        }
        found
    }

    pub fn step_keyframe(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some((clip,scene))=self.studio.read(cx).clip_scene(cx) else {return};
        let Some(time)=self.adjacent_key(forward,&clip,&scene,cx) else {return};
        self.cancel_drag(cx);
        self.studio.update(cx,|s,cx| {s.focus_area(Area::Timeline,cx); s.stop(cx);});
        let playback=self.studio.read(cx).store.read(cx).playback.clone();
        let time=model::timeline_time(&clip,time).clamp(clip.start,clip.end());
        playback.update(cx,|p,cx| {p.pause(cx); p.seek_exact(time,cx);});
        self.studio.update(cx,|s,cx| s.changed(cx));
        cx.notify();
    }

    /// The keys a row's diamond at `t` stands for.
    fn keys_at(scene: &Scene, row: &SheetRow, t: f64) -> Vec<KeyRef> {
        let keys = model::keyframes(scene, &row.id).unwrap_or_default();
        keys.iter()
            .filter(|(name, _)| row.property.as_ref().is_none_or(|p| p == *name))
            .flat_map(|(name, list)| list.iter().filter(|k| (k.time - t).abs() < 1e-4).map(|k| KeyRef { id: row.id.clone(), property: name.clone(), time: k.time }))
            .collect()
    }

    fn key_down(&mut self, row: &SheetRow, t: f64, e: &MouseDownEvent, cx: &mut Context<Self>) {
        if e.button==MouseButton::Right && self.cancel_drag(cx) {return;}
        if self.transform.is_some() {return;}
        if self.box_armed && e.button == MouseButton::Left {
            self.box_armed = false;
            let from = (self.t_of(f32::from(e.position.x) as f64,self.span(cx)),f32::from(e.position.y));
            self.drag = Some(TDrag::Band {from,to:from,selection:SelectionOp::from_modifiers(e.modifiers)});
            cx.notify();
            return;
        }
        let Some((_, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let picked = Self::keys_at(&scene, row, t);
        let additive = e.modifiers.shift || e.modifiers.platform || e.modifiers.control;
        let span = self.span(cx);
        self.studio.update(cx, |s, cx| {
            s.focus_area(Area::Timeline,cx);
            let already = picked.iter().all(|k| s.keys.contains(k));
            if additive {
                if already {
                    s.keys.retain(|k| !picked.contains(k));
                } else {
                    for key in &picked { if !s.keys.contains(key) { s.keys.push(key.clone()); } }
                }
            } else if !already {
                s.keys = picked.clone();
            }
            s.changed(cx);
        });
        if e.button == MouseButton::Right {
            self.easing_menu(e.position, cx);
            return;
        }
        let t0 = self.t_of(f32::from(e.position.x) as f64, span);
        self.drag = Some(TDrag::Keys { t0, anchor: t, dt: 0.0, moved: false });
        cx.notify();
    }

    /// Right-click on keys: how they are reached.
    fn easing_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let studio = self.studio.clone();
        let mut entries: Vec<MenuEntry> = EASINGS
            .iter()
            .map(|(e, label)| {
                let studio = studio.clone();
                MenuItem::new(*label, move |_, cx| set_easing(&studio, e, cx)).entry()
            })
            .collect();
        let s=studio.clone();
        entries.insert(0,MenuItem::new("Keyframe at playhead",move |_,cx| s.update(cx,|s,cx| s.keyframe_selection(cx))).icon("diamond").shortcut_of(&act::StudioInsert).entry());
        entries.insert(1,MenuEntry::Separator);
        for (forward,label,action) in [(false,"Previous keyframe",&act::StudioPreviousKey as &dyn gpui::Action),(true,"Next keyframe",&act::StudioNextKey as &dyn gpui::Action)] {
            let timeline=cx.entity();
            entries.push(MenuItem::new(label,move |_,cx| timeline.update(cx,|t,cx| t.step_keyframe(forward,cx))).shortcut_of(action).entry());
        }
        entries.push(MenuEntry::Separator);
        let timeline=cx.entity();
        entries.push(MenuItem::new("Move keyframes",move |w,cx| timeline.update(cx,|t,cx| t.start_retime(false,w,cx))).icon("move").shortcut_of(&act::StudioGrab).entry());
        let timeline=cx.entity();
        entries.push(MenuItem::new("Scale key timing",move |w,cx| timeline.update(cx,|t,cx| t.start_retime(true,w,cx))).icon("maximize-2").shortcut_of(&act::StudioScale).entry());
        let s = studio.clone();
        entries.push(MenuItem::new("Duplicate keyframes",move |_,cx| s.update(cx,|s,cx| s.duplicate_keys(cx))).icon("copy").shortcut_of(&act::StudioDuplicate).entry());
        let s = studio.clone();
        entries.push(MenuItem::new("Delete keyframes", move |_, cx| s.update(cx, |s, cx| s.delete_keys(cx))).icon("trash").shortcut_of(&act::StudioDelete).danger().entry());
        cx.store().update(cx, |s, cx| s.open_menu(at, entries, cx));
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.transform.is_some() {self.retime_move(e,cx); return;}
        // Wayland synthesizes a buttonless move when the pointer re-enters the window.
        // Only the release or explicit cancellation ends a drag; hover refreshes don't.
        if e.pressed_button != Some(MouseButton::Left) { return; }
        let Some((clip, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let span = self.span(cx);
        let x = f32::from(e.position.x) as f64;
        let y = f32::from(e.position.y);
        let t = self.t_of(x, span);
        let fps = self.studio.read(cx).store.read(cx).fps().max(1.0);
        let snap = |v: f64| snap_key_time(&clip,v,fps,e.modifiers.alt);
        let v_here = self.value_of(y as f64, &scene, cx);
        let selected = self.studio.read(cx).keys.clone();
        match self.drag.as_mut() {
            Some(TDrag::Scrub) => {
                let extent = clip_span(&clip);
                let st = t.clamp(extent.0, extent.1);
                super::seek(&self.studio, st, cx);
            }
            Some(TDrag::Keys { t0, anchor, dt, moved }) => {
                *dt = key_delta(&selected, snap(*anchor + t - *t0) - *anchor);
                *moved = *moved || dt.abs() > 1e-6;
                cx.notify();
            }
            Some(TDrag::Band { to, .. }) => {
                *to = (t, y);
                cx.notify();
            }
            Some(TDrag::GraphKey { picked, from, to, .. }) => {
                *to = (from.0 + key_delta(picked, snap(t) - from.0), v_here);
                cx.notify();
            }
            Some(TDrag::GraphBand { to,.. }) => {
                *to = (t,v_here);
                cx.notify();
            }
            Some(TDrag::Handle { key, component, which, ctrl, moved }) => {
                // The segment from the previous key to this one, in (0..1, 0..1).
                if let Some((a, b)) = segment(&scene, key, *component) {
                    let v = v_here;
                    let fx = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
                    let dv = b.1 - a.1;
                    let fy = if dv.abs() < 1e-9 { 0.0 } else { (v - a.1) / dv };
                    let (x,y) = ((fx * 1000.0).round() / 1000.0,(fy * 1000.0).round() / 1000.0);
                    *moved |= (ctrl[*which*2]-x).abs() > 1e-6 || (ctrl[*which*2+1]-y).abs() > 1e-6;
                    ctrl[*which * 2] = x;
                    ctrl[*which * 2 + 1] = y;
                }
                cx.notify();
            }
            None => {}
        }
    }

    fn drag_end(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.transform.is_some() {
            if e.button==MouseButton::Left {self.finish_retime(cx);}
            else if e.button==MouseButton::Right {self.cancel_drag(cx);}
            return;
        }
        if e.button == MouseButton::Left { self.finish_drag(cx); }
        else if e.button==MouseButton::Right {self.cancel_drag(cx);}
    }

    fn update_keys(&self, clip: kimchi_core::Id, updates: Vec<Value>, next: Vec<KeyRef>, cx: &mut Context<Self>) {
        if updates.is_empty() { return; }
        self.studio.update(cx, |s, cx| {
            let previous = s.keys.clone();
            s.run_then("motion.updateKeyframes", json!({"clipId":clip,"updates":updates}), cx, move |s, _, cx| {
                if s.clip == Some(clip) && s.keys == previous {
                    s.keys = next;
                    s.changed(cx);
                }
            });
        });
    }

    /// Moves the selected keys by `dt` scene seconds, as a drag in the dope sheet does.
    #[cfg(test)]
    pub fn bounds_for_test(&self) -> Bounds<Pixels> { self.bounds.get() }

    #[cfg(test)]
    pub fn dragging_for_test(&self) -> bool { self.drag.is_some() }

    #[cfg(test)]
    pub fn graph_point_for_test(&self, time: f64, value: f64, cx: &App) -> Point<Pixels> {
        let (_, scene) = self.studio.read(cx).clip_scene(cx).unwrap();
        point(px(self.x_of(time, self.span(cx)) as f32), px(self.y_of(value, &scene, cx) as f32))
    }

    #[cfg(test)]
    pub fn drag_keys_by(&mut self, dt: f64, cx: &mut Context<Self>) {
        self.drag = Some(TDrag::Keys { t0: 0.0, anchor: 0.0, dt, moved: true });
        self.finish_drag(cx);
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        cx.notify();
        let Some((clip, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let selected = self.studio.read(cx).keys.clone();
        let clip_id = clip.id;
        match d {
            TDrag::Keys { dt, moved: true, .. } => {
                let dt = key_delta(&selected, dt);
                if dt.abs() < 1e-9 { return; }
                let updates = selected.iter().map(|k| json!({"id":k.id,"property":k.property,"time":model::timeline_time(&clip,k.time),"newTime":model::timeline_time(&clip,k.time+dt)})).collect();
                let next = selected.into_iter().map(|mut k| { k.time += dt; k }).collect();
                self.update_keys(clip_id, updates, next, cx);
            }
            TDrag::Band { from, to, selection } => {
                let rows = self.rows(&scene, cx);
                let (_, by, _, _) = self.track_box();
                let (t0, t1) = (from.0.min(to.0), from.0.max(to.0));
                let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
                let mut picked = vec![];
                for (i, r) in rows.iter().enumerate() {
                    let top = by as f32 + RULER_H + i as f32 * ROW_H - self.scroll_y;
                    if top + ROW_H < y0 || top > y1 {
                        continue;
                    }
                    for tt in r.times.iter().filter(|tt| **tt >= t0 && **tt <= t1) {
                        picked.extend(Self::keys_at(&scene, r, *tt));
                    }
                }
                self.studio.update(cx, |s, cx| {
                    s.keys = selection.apply_keys(&s.keys,picked);
                    s.changed(cx);
                });
            }
            TDrag::GraphKey { comp, picked, from, to, .. } => {
                let (dt,dv) = (key_delta(&picked,to.0-from.0),to.1-from.1);
                if dt.abs() < 1e-9 && dv.abs() < 1e-9 { return; }
                let mut updates = vec![];
                for key in &picked {
                    let keys = model::keyframes(&scene, &key.id).unwrap_or_default();
                    let Some(k) = keys.get(&key.property).and_then(|list| list.iter().find(|k| (k.time-key.time).abs() < 1e-6)) else { return };
                    updates.push(json!({"id":key.id,"property":key.property,"time":model::timeline_time(&clip,key.time),"newTime":model::timeline_time(&clip,key.time+dt),"value":shifted_value(&k.value,comp,dv)}));
                }
                let next = selected.into_iter().map(|mut k| { if picked.contains(&k) { k.time += dt; } k }).collect();
                self.update_keys(clip_id, updates, next, cx);
            }
            TDrag::GraphBand { id,property,from,to,selection } => {
                let keys = model::keyframes(&scene,&id).unwrap_or_default();
                let (times,range) = (from.0.min(to.0)..=from.0.max(to.0),from.1.min(to.1)..=from.1.max(to.1));
                let picked = keys.get(&property).into_iter().flatten().filter(|k| times.contains(&k.time)
                    && values(Some(k.value.clone())).iter().any(|v| range.contains(v)))
                    .map(|k| KeyRef {id:id.clone(),property:property.clone(),time:k.time}).collect::<Vec<_>>();
                self.studio.update(cx,|s,cx| {
                    s.keys = selection.apply_keys(&s.keys,picked);
                    s.changed(cx);
                });
            }
            TDrag::Handle { key, ctrl, moved: true, .. } => {
                let keys = model::keyframes(&scene, &key.id).unwrap_or_default();
                let Some(k) = keys.get(&key.property).and_then(|l| l.iter().find(|k| (k.time - key.time).abs() < 1e-4)).cloned() else { return };
                let easing = format!("cubicBezier({}, {}, {}, {})", ctrl[0], ctrl[1], ctrl[2], ctrl[3]);
                if k.easing == Easing::Bezier(ctrl[0], ctrl[1], ctrl[2], ctrl[3]) { return; }
                self.update_keys(clip_id, vec![json!({"id":key.id,"property":key.property,"time":model::timeline_time(&clip,key.time),"easing":easing})], selected, cx);
            }
            _ => {}
        }
    }

    // ---- the graph -------------------------------------------------------------------------------

    /// The graphed property: the chosen one, or the active thing's first animated number.
    fn graph_target(&self, scene: &Scene, cx: &App) -> Option<(String, String)> {
        self.studio.read(cx).graph_target(scene)
    }

    /// The value range shown: fitted to the curve.
    fn graph_range(&self, scene: &Scene, cx: &App) -> (f64, f64) {
        if let Some(r) = self.range {
            return r;
        }
        let Some((id, prop)) = self.graph_target(scene, cx) else { return (0.0, 1.0) };
        let keys = model::keyframes(scene, &id).unwrap_or_default();
        let Some(list) = keys.get(&prop) else { return (0.0, 1.0) };
        fitted_values(list, self.span(cx))
    }

    pub fn fit_values(&mut self, cx: &mut Context<Self>) {
        self.range = None;
        cx.notify();
    }

    fn y_of(&self, v: f64, scene: &Scene, cx: &App) -> f64 {
        let (_, by, _, h) = self.track_box();
        let (lo, hi) = self.graph_range(scene, cx);
        let top = by + RULER_H as f64 + 6.0;
        let hh = (h - RULER_H as f64 - 12.0).max(1.0);
        top + (1.0 - (v - lo) / (hi - lo).max(1e-9)) * hh
    }

    fn value_of(&self, y: f64, scene: &Scene, cx: &App) -> f64 {
        let (_, by, _, h) = self.track_box();
        let (lo, hi) = self.graph_range(scene, cx);
        let top = by + RULER_H as f64 + 6.0;
        let hh = (h - RULER_H as f64 - 12.0).max(1.0);
        lo + (1.0 - (y - top) / hh) * (hi - lo)
    }

    fn graph_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        if e.button==MouseButton::Right && self.cancel_drag(cx) {return;}
        let Some((_, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let span = self.span(cx);
        let m = (f32::from(e.position.x) as f64, f32::from(e.position.y) as f64);
        self.studio.update(cx, |s, cx| s.focus_area(Area::Timeline,cx));
        let Some((id, prop)) = self.graph_target(&scene, cx) else { return };
        let keys = model::keyframes(&scene, &id).unwrap_or_default();
        let Some(list) = keys.get(&prop) else { return };
        if self.box_armed && e.button == MouseButton::Left {
            self.box_armed = false;
            let from = (self.t_of(m.0,span),self.value_of(m.1,&scene,cx));
            self.drag = Some(TDrag::GraphBand {id,property:prop,from,to:from,selection:SelectionOp::from_modifiers(e.modifiers)});
            cx.notify();
            return;
        }
        let component = self.studio.read(cx).graph_component.min(values(list.first().map(|k| k.value.clone())).len().saturating_sub(1));
        // Handles of the selected key's segment first.
        let selected = self.studio.read(cx).keys.iter().find(|k| k.id == id && k.property == prop).cloned();
        if e.button == MouseButton::Left && self.studio.read(cx).keys.len() == 1 && let Some(sel) = &selected
            && let Some((a, b)) = segment(&scene, sel, component)
        {
            let ctrl = bezier_of(list.iter().find(|k| (k.time - sel.time).abs() < 1e-4).map(|k| k.easing).unwrap_or_default());
            for which in 0..2 {
                let (hx, hy) = (a.0 + ctrl[which * 2] * (b.0 - a.0), a.1 + ctrl[which * 2 + 1] * (b.1 - a.1));
                let (sx, sy) = (self.x_of(hx, span), self.y_of(hy, &scene, cx));
                if (sx - m.0).hypot(sy - m.1) < 7.0 {
                    self.drag = Some(TDrag::Handle { key: sel.clone(), component, which, ctrl, moved: false });
                    cx.notify();
                    return;
                }
            }
        }
        for k in list {
            for (ci, v) in values(Some(k.value.clone())).into_iter().enumerate() {
                let (sx, sy) = (self.x_of(k.time, span), self.y_of(v, &scene, cx));
                if (sx - m.0).hypot(sy - m.1) < 7.0 {
                    let key = KeyRef { id: id.clone(), property: prop.clone(), time: k.time };
                    let comp = matches!(k.value, KeyValue::Vector(_)).then_some(ci);
                    let additive = e.modifiers.shift || e.modifiers.platform || e.modifiers.control;
                    let mut removed = false;
                    self.studio.update(cx, |s, cx| {
                        s.graph_component = ci;
                        s.keys.retain(|k| k.id == id && k.property == prop);
                        if additive && e.button == MouseButton::Left {
                            if s.keys.contains(&key) { s.keys.retain(|k| *k != key); removed = true; }
                            else { s.keys.push(key.clone()); }
                        } else if !s.keys.contains(&key) { s.keys = vec![key.clone()]; }
                        s.changed(cx);
                    });
                    if removed { return; }
                    if e.button == MouseButton::Right {
                        self.easing_menu(e.position, cx);
                        return;
                    }
                    let picked = self.studio.read(cx).keys.clone();
                    self.drag = Some(TDrag::GraphKey { key, comp, picked, from: (k.time, v), to: (k.time, v) });
                    cx.notify();
                    return;
                }
            }
        }
        // Shift-drag empty space selects keys; Ctrl/Cmd adds the rectangle to selection.
        if e.button != MouseButton::Left { return; }
        if e.modifiers.shift {
            let from = (self.t_of(m.0,span),self.value_of(m.1,&scene,cx));
            self.drag = Some(TDrag::GraphBand {id,property:prop,from,to:from,selection:if e.modifiers.control || e.modifiers.platform {SelectionOp::Add} else {SelectionOp::Replace}});
            cx.notify();
            return;
        }
        // Ordinary empty-space clicks scrub.
        self.drag = Some(TDrag::Scrub);
        let t = self.t_of(m.0, span).clamp(span.0, span.1);
        super::seek(&self.studio, t, cx);
    }
}

/// The numbers of a value (one per component).
fn values(v: Option<KeyValue>) -> Vec<f64> {
    match v {
        Some(KeyValue::Number(n)) => vec![n],
        Some(KeyValue::Vector(v)) => v,
        _ => vec![],
    }
}

/// The segment into `key` on the chosen component.
fn segment(scene: &Scene, key: &KeyRef, component: usize) -> Option<((f64, f64), (f64, f64))> {
    let keys = model::keyframes(scene, &key.id)?;
    let list = keys.get(&key.property)?;
    let i = list.iter().position(|k| (k.time - key.time).abs() < 1e-4)?;
    if i == 0 {
        return None;
    }
    let (a, b) = (&list[i - 1], &list[i]);
    Some(((a.time, *values(Some(a.value.clone())).get(component)?), (b.time, *values(Some(b.value.clone())).get(component)?)))
}

/// An easing as bezier control points (the closest for named ones).
pub fn bezier_of(e: Easing) -> [f64; 4] {
    match e {
        Easing::Bezier(a, b, c, d) => [a, b, c, d],
        Easing::EASE_IN => [0.32, 0.0, 0.67, 0.0],
        Easing::EASE_OUT => [0.33, 1.0, 0.68, 1.0],
        Easing::EASE_IN_OUT => [0.65, 0.0, 0.35, 1.0],
        _ => [0.333, 0.333, 0.667, 0.667],
    }
}

/// Change selected easings together without overwriting the unanimated property values.
fn set_easing(studio: &Entity<Studio>, easing: &str, cx: &mut App) {
    let (found, keys) = {
        let st = studio.read(cx);
        (st.clip_scene(cx), st.keys.clone())
    };
    let Some((clip, _)) = found else { return };
    if keys.is_empty() { return; }
    let updates: Vec<_> = keys.iter().map(|k| json!({"id":k.id,"property":k.property,"time":model::timeline_time(&clip,k.time),"easing":easing})).collect();
    super::run("motion.updateKeyframes", json!({"clipId":clip.id,"updates":updates}), cx);
}

fn paint_line(window: &mut Window, pts: &[(f64, f64)], color: Hsla, width: f32) {
    if pts.len() < 2 {
        return;
    }
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(point(px(pts[0].0 as f32), px(pts[0].1 as f32)));
    for p in &pts[1..] {
        b.line_to(point(px(p.0 as f32), px(p.1 as f32)));
    }
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

fn paint_diamond(window: &mut Window, x: f64, y: f64, r: f64, color: Hsla) {
    let mut b = PathBuilder::fill();
    let p = |a: f64, c: f64| point(px(a as f32), px(c as f32));
    b.move_to(p(x, y - r));
    b.line_to(p(x + r, y));
    b.line_to(p(x, y + r));
    b.line_to(p(x - r, y));
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

impl Render for StudioTimeline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let st = self.studio.read(cx);
        let Some((clip, scene)) = st.clip_scene(cx) else { return div().into_any_element() };
        let span = self.span(cx);
        let now = st.scene_time(cx);
        let playing = st.playing;
        let show_graph = st.show_graph;
        let graph_target = self.graph_target(&scene, cx);
        let graph_id = graph_target.as_ref().map(|(id, prop)| (clip.id, id.clone(), prop.clone()));
        if graph_id != self.last_graph { self.range = None; self.drag = None; self.box_armed = false; self.transform=None; self.key_capture=None; self.last_graph = graph_id; }
        if self.last_clip != Some(clip.id) { self.scroll_y = 0.; self.drag = None; self.box_armed = false; self.transform=None; self.key_capture=None; self.last_clip = Some(clip.id); }
        if show_graph != self.last_mode { self.drag = None; self.box_armed = false; self.transform=None; self.key_capture=None; self.last_mode = show_graph; }
        let rows = self.rows(&scene, cx);
        let selected_keys = st.keys.clone();
        let studio = self.studio.clone();
        let bounds = self.bounds.clone();
        let (accent, text3, line, text) = (t.accent, t.text_3, t.line, t.text);

        // Header: transport and the view switch.
        let (s1, s2, s3) = (studio.clone(), studio.clone(), studio.clone());
        let compact = self.track_box().2 + (NAMES_W as f64) < 850.;
        let previous=self.adjacent_key(false,&clip,&scene,cx).is_some();
        let next=self.adjacent_key(true,&clip,&scene,cx).is_some();
        let mut header = div()
            .min_h(px(32.)).py(px(2.))
            .flex_none()
            .flex().flex_wrap()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .border_b_1()
            .border_color(t.line)
            .when(!compact,|d| {
                let timeline=cx.entity();
                d.child(Button::icon("st-previous-key","chevron-left",tip("Previous keyframe",&act::StudioPreviousKey)).disabled(!previous)
                    .on_click(move |_,_,cx| timeline.update(cx,|t,cx| t.step_keyframe(false,cx))))
            })
            .child(Button::icon("st-play", if playing { "pause" } else { "play" }, tip(if playing { "Pause" } else { "Play the clip (loops)" }, &act::StudioPlay)).on_click(move |_, _, cx| s1.update(cx, |s, cx| s.toggle_play(cx))))
            .when(!compact,|d| {
                let timeline=cx.entity();
                d.child(Button::icon("st-next-key","chevron-right",tip("Next keyframe",&act::StudioNextKey)).disabled(!next)
                    .on_click(move |_,_,cx| timeline.update(cx,|t,cx| t.step_keyframe(true,cx))))
            })
            .child(div().font_family(MONO).text_size(px(sz::SM)).text_color(t.text).child(format!("{:.2} s", now)))
            .when(!compact, |d| d.child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(format!("View {:.2}–{:.2} s", span.0, span.1))))
            .child(div().w(px(8.)))
            .when(!compact || selected_keys.is_empty(),|d| d.child(Button::new("st-key", if compact { "" } else { "Keyframe" }).small().with_icon("diamond").tooltip(tip(if show_graph {"Keyframe the visible curve at the playhead"} else {"Keyframe the selection at the playhead"}, &act::StudioInsert)).on_click(move |_, _, cx| s2.update(cx, |s, cx| s.keyframe_selection(cx)))))
            .when(compact && !selected_keys.is_empty(),|d| {
                let timeline=cx.entity();
                d.child(Button::icon("st-keys-compact","ellipsis","Keyframes: insert, move, scale, duplicate or ease")
                    .on_click(move |e,_,cx| timeline.update(cx,|t,cx| t.easing_menu(e.position(),cx))))
            })
            .child(div().flex_1())
            .child(div().w(px(190.)).child(crate::ui::segmented(
                "st-view",
                vec![(false, "Dope sheet".into()), (true, "Graph".into())],
                show_graph,
                move |g, _, cx| {
                    let g = *g;
                    s3.update(cx, |s, cx| {
                        s.show_graph = g;
                        s.changed(cx);
                    })
                },
                cx,
            )))
            .when(show_graph, |d| {
                let this = cx.entity();
                d.child(Button::icon("st-fit", "maximize-2", "Fit the curve").on_click(move |_, _, cx| this.update(cx, |t, cx| t.fit_values(cx))))
            });

        for (id, symbol, label, factor) in [("st-time-out", "minus", "Zoom out in time", 0.5), ("st-time-in", "plus", "Zoom in around the playhead", 2.)] {
            let studio = studio.clone();
            let label = if factor < 1. { tip(label,&act::StudioZoomOut) } else { tip(label,&act::StudioZoomIn) };
            header = header.child(Button::icon(id, symbol, label).on_click(move |_, _, cx| studio.update(cx, |s, cx| s.zoom_timeline(factor, None, cx))));
        }
        for (id, symbol, label, fit) in [("st-time-fit", "move-horizontal", "Fit the clip in time", "clip"), ("st-keys-fit", "scan", "Frame selected keyframes", "selection")] {
            let studio = studio.clone();
            let label = if fit == "clip" { tip(label,&act::StudioFrameAll) } else { tip(label,&act::StudioFrame) };
            header = header.child(Button::icon(id, symbol, label).disabled(fit == "selection" && selected_keys.is_empty())
                .on_click(move |_, _, cx| studio.update(cx, |s, cx| { let _ = s.fit_timeline(fit, cx); })));
        }

        // Left column: rows' names (dope sheet) or the graphed properties (graph).
        let timeline=cx.entity();
        header=header.when(!compact,|d| d.child(Button::new("st-keys-menu","Keys").small().ghost().icon_after("chevron-down")
            .disabled(selected_keys.is_empty()).tooltip("Move, scale, duplicate or ease selected keyframes")
            .on_click(move |e,_,cx| timeline.update(cx,|t,cx| t.easing_menu(e.position(),cx)))));
        let box_status=match &self.drag {
            Some(TDrag::Band {selection,..} | TDrag::GraphBand {selection,..})=>Some(match selection {
                SelectionOp::Replace=>"Box select · Replace selection · Release to finish · Esc or right-click cancels",
                SelectionOp::Add=>"Box select · Add to selection · Release to finish · Esc or right-click cancels",
                SelectionOp::Subtract=>"Box select · Remove from selection · Release to finish · Esc or right-click cancels",
            }),
            _ if self.box_armed=>Some("Box select · Drag to replace · Shift adds · Ctrl/Cmd+Shift removes · Esc or right-click cancels"),
            _=>None,
        };
        if let Some(status)=self.transform.as_ref().map(|t|t.label.as_str()).or(box_status) {
            // Preserve the transport's measured height, including wrapping in narrow windows.
            // Changing the track height would move graph keys underneath an active gesture.
            header=div().relative().flex_none().child(header.invisible())
                .child(div().absolute().inset_0().px(px(10.)).flex().items_center().bg(t.accent_soft)
                    .child(div().min_w_0().truncate().text_size(px(sz::XS)).text_color(t.accent_text).child(status.to_string())));
        }

        let name_heading = || div().h(px(RULER_H)).flex_none().border_b_1().border_color(t.line);
        let mut names = div().w(px(NAMES_W)).flex_none().h_full().flex().flex_col().border_r_1().border_color(t.line);
        if show_graph {
            if let Some((id, _)) = &graph_target {
                let keys = model::keyframes(&scene, id).unwrap_or_default();
                let mut properties = div().id("graph-properties").flex_1().min_h_0().overflow_y_scroll();
                for (name, list) in &keys {
                    if list.first().is_some_and(|k| matches!(k.value, KeyValue::Text(_))) {
                        continue;
                    }
                    let on = graph_target.as_ref().is_some_and(|(_, p)| p == name);
                    let (s, n, owner) = (studio.clone(), name.clone(), id.clone());
                    let this = cx.entity();
                    properties = properties.child(
                        div()
                            .id(SharedString::from(format!("gp-{name}")))
                            .h(px(ROW_H))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .text_size(px(sz::XS))
                            .font_family(MONO)
                            .when(on, |d| d.bg(t.accent_soft).text_color(t.accent_text))
                            .when(!on, |d| d.text_color(t.text_2).hover(|s| s.bg(t.hover)))
                            .child(format!("{id} · {name}"))
                            .on_click(move |_, _, cx| {
                                this.update(cx, |t, _| t.range = None);
                                s.update(cx, |s, cx| {
                                    s.keys.retain(|key| key.id == owner && key.property == n);
                                    s.graph_property = Some(n.clone());
                                    s.changed(cx);
                                })
                            }),
                    );
                }
                let components = graph_target.as_ref().and_then(|(_,p)| keys.get(p)).and_then(|list| list.first()).map(|k| values(Some(k.value.clone())).len()).unwrap_or(0);
                if components > 1 {
                    let mut controls = div().id("graph-components").flex_1().min_w_0().overflow_x_scroll().flex().items_center().gap(px(4.));
                    for component in 0..components {
                        let label = ["X","Y","Z","W"].get(component).map(|s| s.to_string()).unwrap_or_else(|| (component+1).to_string());
                        let s = studio.clone();
                        let selected = st.graph_component.min(components-1)==component;
                        controls = controls.child(div().id(SharedString::from(format!("graph-component-{component}")))
                            .h(px(18.)).min_w(px(22.)).px(px(4.)).flex_none().flex().items_center().justify_center()
                            .text_size(px(sz::XS)).cursor_pointer()
                            .bg(if selected {t.accent_soft} else {t.bg_raised}).text_color(if selected {t.accent_text} else {t.text_2})
                            .hover(|s| s.bg(t.hover)).child(label)
                            .tooltip(|_,cx| crate::ui::tooltip("Edit values and show handles on this component. Vector components share the same easing.".into(),cx))
                            .on_click(move |_,_,cx| s.update(cx,|s,cx| { s.graph_component=component; s.changed(cx); })));
                    }
                    names = names.child(name_heading().px(px(8.)).flex().items_center().gap(px(4.))
                        .child(div().text_size(px(sz::XS)).text_color(t.text_3).child("Component")).child(controls));
                } else { names = names.child(name_heading()); }
                names = names.child(properties);
                if self.track_box().3 > 140. {
                    names = names.child(div().flex_none().px(px(8.)).py(px(4.)).text_size(px(sz::XS))
                        .text_color(if self.box_armed { t.accent_text } else { t.text_3 })
                        .child(if self.box_armed { "Drag to select · Esc or right-click cancels".to_string() } else { format!("{} or Shift-drag to select",act::hint(&act::StudioBoxSelect).unwrap_or_default()) })
                        .child(div().mt(px(2.)).child("Alt-drag for subframes")));
                }
            } else {
                names = names.child(name_heading()).child(div().p(px(10.)).text_size(px(sz::XS)).text_color(t.text_3).child("Select an animated thing to see its curves."));
            }
        } else {
            names = names.child(name_heading());
            let mut list = div().mt(px(-self.scroll_y)).flex().flex_col();
            for row in &rows {
                let open = st.expanded.contains(&row.id);
                let is_item = row.property.is_none();
                let selected = is_item && st.selection.contains(&row.id);
                let (s, id) = (studio.clone(), row.id.clone());
                let el = div()
                    .id(SharedString::from(format!("dsn-{}-{}", row.id, row.property.clone().unwrap_or_default())))
                    .h(px(ROW_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(if is_item { 6. } else { 24. }))
                    .pr(px(6.))
                    .text_size(px(sz::XS))
                    .when(selected, |d| d.bg(t.accent_soft))
                    .when(is_item, |d| {
                        let id2 = id.clone();
                        let s2 = s.clone();
                        d.child(
                            div()
                                .id(SharedString::from(format!("dsx-{id}")))
                                .cursor_pointer()
                                .text_color(t.text_2)
                                .child(icon(if open { "chevron-down" } else { "chevron-right" }).size(px(11.)))
                                .on_click(move |_, _, cx| s2.update(cx, |s, cx| {
                                    if !s.expanded.remove(&id2) {
                                        s.expanded.insert(id2.clone());
                                    }
                                    cx.notify();
                                })),
                        )
                        .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!is_item, |d| d.font_family(MONO).text_color(t.text_2))
                    .child(div().min_w_0().truncate().child(row.label.clone()))
                    .when(is_item && row.id != "scene", |d| {
                        let s3 = s.clone();
                        let id3 = id.clone();
                        d.cursor_pointer().on_click(move |e, _, cx| {
                            let add = e.modifiers().shift;
                            s3.update(cx, |s, cx| s.select(&id3, add, cx))
                        })
                    });
                list = list.child(el);
            }
            names = names.child(div().flex_1().min_h_0().overflow_hidden().child(list));
            if rows.is_empty() {
                names = names.child(div().p(px(10.)).text_size(px(sz::XS)).text_color(t.text_3).line_height(px(sz::XS * 1.4)).child("Nothing is animated yet. Select something and press I, or click a diamond in Properties."));
            }
        }

        // The track area, painted.
        let entity = cx.entity();
        let x_of = {
            let b = self.bounds.get();
            let (bx, w) = (f32::from(b.origin.x) as f64, f32::from(b.size.width) as f64);
            move |tt: f64| bx + 8.0 + (tt - span.0) / (span.1 - span.0) * (w - 16.0).max(1.0)
        };
        let drag_dt = match &self.drag {
            Some(TDrag::Keys { dt, .. }) => *dt,
            _ => 0.0,
        };
        let band = match &self.drag {
            Some(TDrag::Band { from, to, .. }) => Some((x_of(from.0), from.1 as f64, x_of(to.0), to.1 as f64)),
            _ => None,
        };
        let (_, by, _, bh) = self.track_box();
        // Fit once per frame, not once for every sampled curve point.
        let value_range = if show_graph { self.graph_range(&scene, cx) } else { (0., 1.) };
        let y_of = |value: f64| by + RULER_H as f64 + 6. + (1. - (value - value_range.0) / (value_range.1 - value_range.0).max(1e-9)) * (bh - RULER_H as f64 - 12.).max(1.);
        let band = match &self.drag {
            Some(TDrag::GraphBand {from,to,..}) => Some((x_of(from.0),y_of(from.1),x_of(to.0),y_of(to.1))),
            _ => band,
        };
        let mut value_ticks = vec![];
        if show_graph {
            let step = ruler_step(value_range, (bh - RULER_H as f64).max(1.) * 2.);
            let mut value = (value_range.0 / step).ceil() * step;
            while value <= value_range.1 {
                value_ticks.push((y_of(value), value, value_label(value, step)));
                value += step;
            }
        }
        let value_labels: Vec<AnyElement> = value_ticks.iter().map(|(y, _, label)| div().absolute()
            .left(px(4.)).top(px((y - by - 6.) as f32)).text_size(px(10.)).font_family(MONO)
            .text_color(text3).bg(t.bg_raised.opacity(0.8)).child(label.clone()).into_any_element()).collect();
        // What to paint: diamonds (dope sheet) or curves (graph).
        let mut diamonds: Vec<(f64, f64, bool, bool)> = vec![];
        let mut curves: Vec<(Vec<(f64, f64)>, Hsla)> = vec![];
        let mut points: Vec<(f64, f64, bool)> = vec![];
        let mut handles: Vec<((f64, f64), (f64, f64))> = vec![];
        if show_graph {
            if let Some((id, prop)) = &graph_target {
                let keys = model::keyframes(&scene, id).unwrap_or_default();
                if let Some(list) = keys.get(prop) {
                    let preview = match &self.drag {
                        Some(TDrag::GraphKey { key, picked, comp, from, to }) if key.id == *id && key.property == *prop => Some(shifted_curve(list, picked, *comp, to.0-from.0, to.1-from.1)),
                        Some(TDrag::Handle { key, ctrl, .. }) if key.id == *id && key.property == *prop => {
                            let mut preview = list.clone();
                            if let Some(k) = preview.iter_mut().find(|k| (k.time-key.time).abs() < 1e-6) { k.easing = Easing::Bezier(ctrl[0],ctrl[1],ctrl[2],ctrl[3]); }
                            Some(preview)
                        }
                        _ => self.transform.as_ref().map(|m| m.curve(id,prop,list)),
                    };
                    let curve = preview.as_deref().unwrap_or(list);
                    let n = values(list.first().map(|k| k.value.clone())).len();
                    let colors = [0xf0565c, 0x7bd88f, 0x5aa7ff];
                    for c in 0..n {
                        let pts: Vec<(f64, f64)> = (0..=200)
                            .filter_map(|i| {
                                let tt = span.0 + (span.1 - span.0) * i as f64 / 200.0;
                                values(value_at(curve, tt)).get(c).map(|v| (x_of(tt), y_of(*v)))
                            })
                            .collect();
                        curves.push((pts, if n == 1 { accent } else { gpui::rgb(colors[c % 3]).into() }));
                    }
                    for k in curve {
                        let sel = selected_keys.iter().any(|s| {
                            let time = match &self.drag {
                                Some(TDrag::GraphKey { picked,from,to,.. }) if picked.contains(s) => s.time+to.0-from.0,
                                _ => self.transform.as_ref().map_or(s.time,|m| m.time(s)),
                            };
                            s.id == *id && s.property == *prop && (time-k.time).abs() < 1e-6
                        });
                        for v in values(Some(k.value.clone())) {
                            points.push((x_of(k.time),y_of(v),sel));
                        }
                    }
                    if self.transform.is_none() && !matches!(self.drag, Some(TDrag::GraphKey { .. })) && selected_keys.len() == 1 && let Some(sel) = selected_keys.iter().find(|s| s.id == *id && s.property == *prop)
                        && let Some((a, b)) = segment(&scene, sel, st.graph_component.min(n.saturating_sub(1)))
                    {
                        let ctrl = match &self.drag {
                            Some(TDrag::Handle { ctrl, .. }) => *ctrl,
                            _ => bezier_of(list.iter().find(|k| (k.time - sel.time).abs() < 1e-4).map(|k| k.easing).unwrap_or_default()),
                        };
                        let pa = (x_of(a.0), y_of(a.1));
                        let pb = (x_of(b.0), y_of(b.1));
                        let h1 = (x_of(a.0 + ctrl[0] * (b.0 - a.0)), y_of(a.1 + ctrl[1] * (b.1 - a.1)));
                        let h2 = (x_of(a.0 + ctrl[2] * (b.0 - a.0)), y_of(a.1 + ctrl[3] * (b.1 - a.1)));
                        handles.push((pa, h1));
                        handles.push((pb, h2));
                    }
                }
            }
        } else {
            for (i, row) in rows.iter().enumerate() {
                let y = by + RULER_H as f64 + i as f64 * ROW_H as f64 + ROW_H as f64 / 2.0 - self.scroll_y as f64;
                if y < by + RULER_H as f64 {
                    continue;
                }
                for tt in &row.times {
                    let refs = Self::keys_at(&scene, row, *tt);
                    let sel = !refs.is_empty() && refs.iter().all(|k| selected_keys.contains(k));
                    let shown = if sel { self.transform.as_ref().and_then(|m| refs.first().map(|k| m.time(k))).unwrap_or(tt+drag_dt) } else { *tt };
                    diamonds.push((x_of(shown), y, sel, row.property.is_none()));
                }
            }
        }
        let now_x = x_of(now);
        let ticks: Vec<(f64, String)> = {
            let step = ruler_step(span, self.track_box().2);
            let mut out = vec![];
            let mut tt = (span.0 / step).ceil() * step;
            while tt <= span.1 + 1e-9 {
                let precision = if step < 0.01 { 4 } else if step < 0.1 { 3 } else { 2 };
                out.push((x_of(tt), format!("{tt:.precision$}").trim_end_matches('0').trim_end_matches('.').to_string()));
                tt += step;
            }
            out
        };
        let tick_labels: Vec<AnyElement> = {
            let ox = f32::from(self.bounds.get().origin.x) as f64;
            ticks.iter().map(|(x, l)| div().absolute().top(px(4.)).left(px((x - ox + 3.0) as f32)).text_size(px(10.)).font_family(MONO).text_color(text3).child(l.clone()).into_any_element()).collect()
        };
        let ticks_x: Vec<f64> = ticks.iter().map(|(x, _)| *x).collect();
        let rows2 = rows.clone();
        let scroll = self.scroll_y;
        let painter = canvas(
            move |b, _, cx| {
                if bounds.get() != b {
                    bounds.set(b);
                    entity.update(cx, |_, cx| cx.notify());
                }
            },
            move |b, _, window, _| {
                let (ox, oy, h) = (f32::from(b.origin.x) as f64, f32::from(b.origin.y) as f64, f32::from(b.size.height) as f64);
                let w = f32::from(b.size.width) as f64;
                // Ruler and grid.
                paint_line(window, &[(ox, oy + RULER_H as f64), (ox + w, oy + RULER_H as f64)], line, 1.0);
                for x in &ticks_x {
                    paint_line(window, &[(*x, oy), (*x, oy + h)], line.opacity(0.6), 1.0);
                }
                for (y, value, _) in &value_ticks {
                    paint_line(window, &[(ox, *y), (ox + w, *y)], line.opacity(if value.abs() < 1e-9 { 1. } else { 0.5 }), 1.0);
                }
                if !show_graph {
                    for i in 0..rows2.len() {
                        let y = oy + RULER_H as f64 + (i + 1) as f64 * ROW_H as f64 - scroll as f64;
                        if y <= oy + RULER_H as f64 {
                            continue;
                        }
                        paint_line(window, &[(ox, y), (ox + w, y)], line.opacity(0.5), 1.0);
                    }
                }
                for (pts, c) in &curves {
                    paint_line(window, pts, *c, 2.0);
                }
                for ((a, bx), (c, d)) in handles.iter().map(|(p, q)| ((p.0, p.1), (q.0, q.1))) {
                    paint_line(window, &[(a, bx), (c, d)], text.opacity(0.5), 1.0);
                    paint_diamond(window, c, d, 4.0, text);
                }
                for (x, y, sel) in &points {
                    paint_diamond(window, *x, *y, if *sel { 6.0 } else { 4.5 }, if *sel { accent } else { text });
                }
                for (x, y, sel, item) in &diamonds {
                    let r = if *item { 5.0 } else { 4.0 };
                    paint_diamond(window, *x, *y, r + 1.0, gpui::black().opacity(0.4));
                    paint_diamond(window, *x, *y, r, if *sel { accent } else if *item { text.opacity(0.9) } else { text.opacity(0.65) });
                }
                if let Some((x0, y0, x1, y1)) = band {
                    let (a, c) = (x0.min(x1), x0.max(x1));
                    let (bb, d) = (y0.min(y1), y0.max(y1));
                    paint_line(window, &[(a, bb), (c, bb), (c, d), (a, d), (a, bb)], accent, 1.0);
                }
                // The playhead.
                paint_line(window, &[(now_x, oy), (now_x, oy + h)], accent, 1.5);
                paint_diamond(window, now_x, oy + 6.0, 5.0, accent);
            },
        )
        .absolute()
        .size_full();

        // Hit areas for diamonds (dope sheet).
        let mut hits: Vec<AnyElement> = vec![];
        if !show_graph {
            let ox = f32::from(self.bounds.get().origin.x) as f64;
            for (i, row) in rows.iter().enumerate() {
                for tt in &row.times {
                    let x = x_of(*tt) - ox;
                    let y = RULER_H + i as f32 * ROW_H - self.scroll_y;
                    if y < RULER_H - 4. {
                        continue;
                    }
                    let (row2, tt2) = (row.clone(), *tt);
                    let (row3, tt3) = (row.clone(), *tt);
                    hits.push(
                        div()
                            .id(SharedString::from(format!("kd-{}-{}-{tt}", row.id, row.property.clone().unwrap_or_default())))
                            .absolute()
                            .left(px(x as f32 - 7.))
                            .top(px(y + 4.))
                            .size(px(14.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.key_down(&row2, tt2, e, cx)
                            }))
                            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.key_down(&row3, tt3, e, cx)
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        let _ = (by, bh);
        let track = div()
            .id("st-track")
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .overflow_hidden()
            .when(self.box_armed,|d| d.cursor(gpui::CursorStyle::Crosshair))
            .child(painter)
            .children(tick_labels)
            .children(value_labels)
            .children(hits)
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                if this.transform.is_some() {cx.stop_propagation(); return;}
                this.studio.update(cx, |s, cx| s.focus_area(Area::Timeline,cx));
                let (_, by, _, _) = this.track_box();
                let y = f32::from(e.position.y);
                let Some((clip, _)) = this.studio.read(cx).clip_scene(cx) else { return };
                let span = this.span(cx);
                if this.studio.read(cx).show_graph && y > by as f32 + RULER_H {
                    this.graph_down(e, cx);
                    return;
                }
                let t = this.t_of(f32::from(e.position.x) as f64, span);
                if y <= by as f32 + RULER_H {
                    this.box_armed = false;
                    this.drag = Some(TDrag::Scrub);
                    let extent = clip_span(&clip);
                    super::seek(&this.studio, t.clamp(extent.0, extent.1), cx);
                } else {
                    this.box_armed = false;
                    this.drag = Some(TDrag::Band { from: (t, y), to: (t, y), selection: SelectionOp::from_modifiers(e.modifiers) });
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                if this.cancel_drag(cx) {cx.stop_propagation(); return;}
                if this.studio.read(cx).show_graph {
                    this.graph_down(e, cx);
                }
            }))
            // A quick click can release before the window-wide drag tracker is painted.
            .on_mouse_up(MouseButton::Left,cx.listener(Self::drag_end))
            .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, _, cx| {
                let delta = e.delta.pixel_delta(px(16.));
                let (dx, dy) = (f32::from(delta.x) as f64, f32::from(delta.y) as f64);
                let span = this.span(cx);
                if e.modifiers.control || e.modifiers.platform {
                    let anchor = this.t_of(f32::from(e.position.x) as f64, span).clamp(span.0, span.1);
                    let factor = (dy * 0.01).clamp(-2., 2.).exp();
                    this.studio.update(cx, |s, cx| s.zoom_timeline(factor, Some(anchor), cx));
                    cx.stop_propagation();
                    return;
                }
                if e.modifiers.shift || dx.abs() > dy.abs() {
                    let movement = if e.modifiers.shift && dy.abs() > dx.abs() { dy } else { dx };
                    let by = -movement / this.track_box().2.max(1.) * (span.1 - span.0);
                    this.studio.update(cx, |s, cx| s.pan_timeline(by, cx));
                    cx.stop_propagation();
                    return;
                }
                // Dope sheet: scroll rows. Graph: zoom values around the pointer.
                if !this.studio.read(cx).show_graph {
                    let Some((_, scene)) = this.studio.read(cx).clip_scene(cx) else { return };
                    let n = this.rows(&scene, cx).len() as f32;
                    let (_, _, _, h) = this.track_box();
                    let max = (n * ROW_H + RULER_H - h as f32 + 8.).max(0.);
                    this.scroll_y = (this.scroll_y - dy as f32).clamp(0., max);
                    cx.notify();
                    return;
                }
                let Some((_, scene)) = this.studio.read(cx).clip_scene(cx) else { return };
                let (lo, hi) = this.graph_range(&scene, cx);
                let k = (1.0 - dy * 0.003).clamp(0.5, 2.0);
                let mid = this.value_of(f32::from(e.position.y) as f64, &scene, cx).clamp(lo, hi);
                this.range = Some((mid - (mid - lo) * k, mid + (hi - mid) * k));
                cx.notify();
            }));
        div()
            .id("studio-timeline")
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg_raised)
            .on_mouse_down(MouseButton::Left,cx.listener(|this,_,_,cx| this.studio.update(cx,|s,cx| s.focus_area(Area::Timeline,cx))))
            .on_mouse_down(MouseButton::Right,cx.listener(|this,_,_,cx| {if this.cancel_drag(cx) {cx.stop_propagation();}}))
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(names).child(track))
            .when(self.drag.is_some() || self.transform.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_end)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::Keyframe;

    #[test]
    fn timeline_navigation_keeps_the_pointer_anchor_and_bounds_the_range() {
        assert_eq!(zoom_range((2., 10.), 2., 4.), (3., 7.));
        assert_eq!(zoom_range((2., 10.), 0.5, 4.), (0., 16.));
        assert_eq!(pan_range((2., 10.), -20.), (0., 8.));
        assert_eq!(pan_range((2., 10.), MAX_TIME), (MAX_TIME - 8., MAX_TIME));
        assert_eq!(zoom_range((2., 10.), f64::NAN, 4.), (2., 10.));
        for (a, b) in [(0.,0.),(-1.,4.),(3.,2.),(0.,f64::INFINITY)] { assert!(valid_range(a,b).is_err()); }
        assert_eq!(ruler_step((0., 600.), 800.), 100.);
        assert!(ruler_step((1.,1.002), 800.) < 0.001, "subframe zoom remains legible");
    }

    #[test]
    fn key_snapping_uses_project_frames_after_trim_speed_and_reverse() {
        let mut clip=kimchi_core::Clip::new("animation",2.013,4.,kimchi_core::ClipContent::Solid {color:"#000000".into()});
        clip.in_point=0.217;
        for speed in [0.25,1.,2.,3.] {
            clip.speed=speed;
            for reverse in [false,true] {
                clip.reverse=reverse;
                for fps in [24.,30.,29.97,60.] {
                    for time in [0.43,1.097,5.123] {
                        let target=snap_key_time(&clip,time,fps,false);
                        let frame=model::timeline_time(&clip,target)*fps;
                        assert!((frame-frame.round()).abs()<1e-7,"{speed}×, reverse={reverse}, {fps} fps: {frame}");
                        assert!((target-time).abs() <= speed/fps*0.500001,"nearest project frame");
                        assert_eq!(snap_key_time(&clip,time,fps,true),time,"Alt allows exact subframe positioning");
                    }
                }
            }
        }
    }

    #[test]
    fn named_easings_have_bezier_handles() {
        assert_eq!(bezier_of(Easing::parse("cubicBezier(0.1, 0.2, 0.3, 0.4)").unwrap()), [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(bezier_of(Easing::EASE_OUT)[1], 1.0);
        for (e, _) in EASINGS {
            Easing::parse(e).unwrap_or_else(|err| panic!("{e}: {err}"));
        }
        let _ = Keyframe { time: 0.0, value: KeyValue::Number(0.0), easing: Easing::Linear };
    }

    #[test]
    fn graph_framing_fits_visible_values_and_labels_small_intervals() {
        let keys = [(0.,0.),(1.,1.),(100.,10000.)].into_iter().map(|(time, value)| Keyframe { time, value: KeyValue::Number(value), easing: Easing::Linear }).collect::<Vec<_>>();
        assert_eq!(fitted_values(&keys, (0.,1.)), (-0.12,1.12));
        let range = fitted_values(&keys, (0.2,0.8));
        assert!((range.0 - 0.128).abs() < 1e-9 && (range.1 - 0.872).abs() < 1e-9);
        assert_eq!(value_label(0.0002,0.0001), "0.0002");
        assert_eq!(value_label(-1e-15,0.0001), "0");
        assert_eq!(value_label(1000.,100.), "1000");
        assert_eq!(value_label(1e-8,1e-9), "1.000e-8");
    }

    #[test]
    fn graph_key_preview_preserves_spacing_components_and_destination_collisions() {
        let picked: Vec<_> = [1.,2.].into_iter().map(|time| KeyRef { id:"box".into(),property:"position".into(),time }).collect();
        assert_eq!(key_delta(&picked,-9.),-1.);
        let keys = vec![Keyframe::new(1.,[1.,2.,3.],Easing::Linear),Keyframe::new(2.,[4.,5.,6.],Easing::EASE_OUT),Keyframe::new(3.,[7.,8.,9.],Easing::Hold)];
        let preview = shifted_curve(&keys,&picked,Some(1),1.,2.);
        assert_eq!(preview.len(),2,"the unselected destination is replaced");
        assert_eq!(preview[0],Keyframe::new(2.,[1.,4.,3.],Easing::Linear));
        assert_eq!(preview[1],Keyframe::new(3.,[4.,7.,6.],Easing::EASE_OUT));
        assert_ne!(value_at(&preview,2.5),value_at(&keys,2.5),"the curve follows the preview, not just its markers");
    }
}
