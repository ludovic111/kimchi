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
use kimchi_core::{Easing, KeyValue, Keyframes, Scene};
use serde_json::{Value, json};

use super::model;
use super::{Area, KeyRef, Studio};
use crate::actions::{self as act, tip};
use crate::store::{MenuEntry, MenuItem, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, drag, icon};

const NAMES_W: f32 = 210.;
const ROW_H: f32 = 22.;
const RULER_H: f32 = 22.;

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
    Keys { t0: f64, dt: f64, moved: bool },
    /// A rubber band over keys (scene seconds, rows).
    Band { from: (f64, f32), to: (f64, f32), additive: bool },
    /// Graph: one key, from its (time, value) to the pointer's.
    GraphKey { key: KeyRef, comp: Option<usize>, from: (f64, f64), to: (f64, f64) },
    /// Graph: a bezier handle of the segment into `key` (0 = first, 1 = second).
    Handle { key: KeyRef, which: usize, ctrl: [f64; 4] },
}

pub struct StudioTimeline {
    studio: Entity<Studio>,
    bounds: Rc<std::cell::Cell<Bounds<Pixels>>>,
    drag: Option<TDrag>,
    /// Graph editor: the value range shown (fitted when `None`).
    range: Option<(f64, f64)>,
    /// Dope sheet: pixels scrolled down.
    scroll_y: f32,
    _subs: Vec<Subscription>,
}

impl StudioTimeline {
    pub fn new(studio: Entity<Studio>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![cx.observe(&studio, |_, _, cx| cx.notify()), cx.observe(&store, |_, _, cx| cx.notify()), cx.observe(&playback, |_, _, cx| cx.notify())];
        Self { studio, bounds: Rc::new(std::cell::Cell::new(Bounds::default())), drag: None, range: None, scroll_y: 0., _subs: subs }
    }

    /// Scene seconds the area shows: the clip's.
    fn span(clip: &kimchi_core::Clip) -> (f64, f64) {
        let (a, b) = (model::scene_time(clip, clip.start), model::scene_time(clip, clip.end()));
        let (a, b) = (a.min(b), a.max(b));
        (a, if b - a < 0.05 { a + 1.0 } else { b })
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
        let Some((_, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let mut all = vec![];
        for id in std::iter::once("scene".to_string()).chain(model::thing_ids(&scene)) {
            for (name, list) in model::keyframes(&scene, &id).unwrap_or_default() {
                all.extend(list.iter().map(|k| KeyRef { id: id.clone(), property: name.clone(), time: k.time }));
            }
        }
        self.studio.update(cx, |s, cx| {
            s.keys = if s.keys.len() == all.len() { vec![] } else { all };
            s.changed(cx);
        });
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
        let Some((clip, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let picked = Self::keys_at(&scene, row, t);
        let additive = e.modifiers.shift || e.modifiers.platform;
        let span = Self::span(&clip);
        self.studio.update(cx, |s, cx| {
            s.area = Area::Timeline;
            let already = picked.iter().all(|k| s.keys.contains(k));
            if additive {
                if already {
                    s.keys.retain(|k| !picked.contains(k));
                } else {
                    s.keys.extend(picked.clone());
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
        self.drag = Some(TDrag::Keys { t0, dt: 0.0, moved: false });
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
        entries.push(MenuEntry::Separator);
        let s = studio.clone();
        entries.push(MenuItem::new("Delete keyframes", move |_, cx| s.update(cx, |s, cx| s.delete_keys(cx))).icon("trash").shortcut_of(&act::StudioDelete).danger().entry());
        cx.store().update(cx, |s, cx| s.open_menu(at, entries, cx));
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((clip, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let span = Self::span(&clip);
        let x = f32::from(e.position.x) as f64;
        let y = f32::from(e.position.y);
        let t = self.t_of(x, span);
        let fps = self.studio.read(cx).store.read(cx).fps().max(1.0);
        let snap = |v: f64| (v * fps).round() / fps;
        let v_here = self.value_of(y as f64, &scene, cx);
        match self.drag.as_mut() {
            Some(TDrag::Scrub) => {
                let st = t.clamp(span.0, span.1);
                super::seek(&self.studio, st, cx);
            }
            Some(TDrag::Keys { t0, dt, moved }) => {
                *dt = snap(t - *t0);
                *moved = *moved || dt.abs() > 1e-6;
                cx.notify();
            }
            Some(TDrag::Band { to, .. }) => {
                *to = (t, y);
                cx.notify();
            }
            Some(TDrag::GraphKey { to, .. }) => {
                *to = (snap(t), v_here);
                cx.notify();
            }
            Some(TDrag::Handle { key, which, ctrl }) => {
                // The segment from the previous key to this one, in (0..1, 0..1).
                if let Some((a, b)) = segment(&scene, key) {
                    let v = v_here;
                    let fx = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
                    let dv = b.1 - a.1;
                    let fy = if dv.abs() < 1e-9 { 0.0 } else { (v - a.1) / dv };
                    ctrl[*which * 2] = (fx * 1000.0).round() / 1000.0;
                    ctrl[*which * 2 + 1] = (fy * 1000.0).round() / 1000.0;
                }
                cx.notify();
            }
            None => {}
        }
    }

    fn drag_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_drag(cx);
    }

    /// Moves the selected keys by `dt` scene seconds, as a drag in the dope sheet does.
    #[cfg(test)]
    pub fn drag_keys_by(&mut self, dt: f64, cx: &mut Context<Self>) {
        self.drag = Some(TDrag::Keys { t0: 0.0, dt, moved: true });
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
                // One shiftKeyframes per property, all one undo step.
                let key = self.studio.update(cx, |s, _| s.drag_key());
                let mut groups: Vec<(String, String, Vec<f64>)> = vec![];
                for k in &selected {
                    let tl = model::timeline_time(&clip, k.time);
                    match groups.iter_mut().find(|g| g.0 == k.id && g.1 == k.property) {
                        Some(g) => g.2.push(tl),
                        None => groups.push((k.id.clone(), k.property.clone(), vec![tl])),
                    }
                }
                let by = dt / clip.speed.max(1e-6);
                let cmds: Vec<(String, Value)> = groups
                    .into_iter()
                    .map(|(id, prop, times)| ("motion.shiftKeyframes".to_string(), json!({ "clipId": clip_id, "id": id, "property": prop, "times": times, "by": by, "coalesce": key })))
                    .collect();
                self.studio.update(cx, |s, cx| {
                    for k in s.keys.iter_mut() {
                        k.time = (k.time + dt).max(0.0);
                    }
                    s.send(cmds, cx);
                    s.changed(cx);
                });
            }
            TDrag::Band { from, to, additive } => {
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
                    if !additive {
                        s.keys.clear();
                    }
                    for k in picked {
                        if !s.keys.contains(&k) {
                            s.keys.push(k);
                        }
                    }
                    s.changed(cx);
                });
            }
            TDrag::GraphKey { key, comp, from, to } => {
                let keys = model::keyframes(&scene, &key.id).unwrap_or_default();
                let Some(k) = keys.get(&key.property).and_then(|l| l.iter().find(|k| (k.time - key.time).abs() < 1e-4)).cloned() else { return };
                let coalesce = self.studio.update(cx, |s, _| s.drag_key());
                let mut cmds = vec![];
                let dt = to.0 - from.0;
                if dt.abs() > 1e-6 {
                    cmds.push(("motion.shiftKeyframes".to_string(), json!({ "clipId": clip_id, "id": key.id, "property": key.property, "times": [model::timeline_time(&clip, key.time)], "by": dt / clip.speed.max(1e-6), "coalesce": coalesce })));
                }
                if (to.1 - from.1).abs() > 1e-9 {
                    let value = match (&k.value, comp) {
                        (KeyValue::Vector(v), Some(i)) => {
                            let mut v = v.clone();
                            if let Some(x) = v.get_mut(i) {
                                *x = to.1;
                            }
                            json!(v)
                        }
                        _ => json!(to.1),
                    };
                    let at = model::timeline_time(&clip, (key.time + dt).max(0.0));
                    cmds.push(("motion.addKeyframe".to_string(), json!({ "clipId": clip_id, "id": key.id, "property": key.property, "time": at, "value": value, "easing": k.easing.to_string(), "coalesce": coalesce })));
                }
                self.studio.update(cx, |s, cx| {
                    for kk in s.keys.iter_mut().filter(|kk| **kk == key) {
                        kk.time = (key.time + dt).max(0.0);
                    }
                    s.send(cmds, cx);
                    s.changed(cx);
                });
            }
            TDrag::Handle { key, ctrl, .. } => {
                let keys = model::keyframes(&scene, &key.id).unwrap_or_default();
                let Some(k) = keys.get(&key.property).and_then(|l| l.iter().find(|k| (k.time - key.time).abs() < 1e-4)).cloned() else { return };
                let easing = format!("cubicBezier({}, {}, {}, {})", ctrl[0], ctrl[1], ctrl[2], ctrl[3]);
                let value = serde_json::to_value(&k.value).unwrap_or(Value::Null);
                super::run("motion.addKeyframe", json!({ "clipId": clip_id, "id": key.id, "property": key.property, "time": model::timeline_time(&clip, key.time), "value": value, "easing": easing }), cx);
            }
            _ => {}
        }
    }

    // ---- the graph -------------------------------------------------------------------------------

    /// The graphed property: the chosen one, or the active thing's first animated number.
    fn graph_target(&self, scene: &Scene, cx: &App) -> Option<(String, String)> {
        let st = self.studio.read(cx);
        let id = st.active().map(str::to_string).filter(|a| model::is_thing(scene, a)).or_else(|| st.keys.first().map(|k| k.id.clone()))?;
        let keys = model::keyframes(scene, &id)?;
        let numeric = |name: &str| keys.get(name).and_then(|l| l.first()).is_some_and(|k| !matches!(k.value, KeyValue::Text(_)));
        let prop = st.graph_property.clone().filter(|p| numeric(p)).or_else(|| keys.keys().find(|n| numeric(n)).cloned())?;
        Some((id, prop))
    }

    /// The value range shown: fitted to the curve.
    fn graph_range(&self, scene: &Scene, cx: &App) -> (f64, f64) {
        if let Some(r) = self.range {
            return r;
        }
        let Some((id, prop)) = self.graph_target(scene, cx) else { return (0.0, 1.0) };
        let keys = model::keyframes(scene, &id).unwrap_or_default();
        let Some(list) = keys.get(&prop) else { return (0.0, 1.0) };
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        let Some((clip, _)) = self.studio.read(cx).clip_scene(cx) else { return (0.0, 1.0) };
        let span = Self::span(&clip);
        for i in 0..=120 {
            let t = span.0 + (span.1 - span.0) * i as f64 / 120.0;
            for v in values(value_at(list, t)) {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        for k in list {
            for v in values(Some(k.value.clone())) {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        if !lo.is_finite() {
            return (0.0, 1.0);
        }
        let pad = ((hi - lo) * 0.12).max(1e-3);
        (lo - pad, hi + pad)
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
        let Some((clip, scene)) = self.studio.read(cx).clip_scene(cx) else { return };
        let span = Self::span(&clip);
        let m = (f32::from(e.position.x) as f64, f32::from(e.position.y) as f64);
        self.studio.update(cx, |s, _| s.area = Area::Timeline);
        let Some((id, prop)) = self.graph_target(&scene, cx) else { return };
        let keys = model::keyframes(&scene, &id).unwrap_or_default();
        let Some(list) = keys.get(&prop) else { return };
        // Handles of the selected key's segment first.
        let selected = self.studio.read(cx).keys.iter().find(|k| k.id == id && k.property == prop).cloned();
        if let Some(sel) = &selected
            && let Some((a, b)) = segment(&scene, sel)
        {
            let ctrl = bezier_of(list.iter().find(|k| (k.time - sel.time).abs() < 1e-4).map(|k| k.easing).unwrap_or_default());
            for which in 0..2 {
                let (hx, hy) = (a.0 + ctrl[which * 2] * (b.0 - a.0), a.1 + ctrl[which * 2 + 1] * (b.1 - a.1));
                let (sx, sy) = (self.x_of(hx, span), self.y_of(hy, &scene, cx));
                if (sx - m.0).hypot(sy - m.1) < 7.0 {
                    self.drag = Some(TDrag::Handle { key: sel.clone(), which, ctrl });
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
                    self.studio.update(cx, |s, cx| {
                        s.keys = vec![key.clone()];
                        s.changed(cx);
                    });
                    if e.button == MouseButton::Right {
                        self.easing_menu(e.position, cx);
                        return;
                    }
                    self.drag = Some(TDrag::GraphKey { key, comp, from: (k.time, v), to: (k.time, v) });
                    cx.notify();
                    return;
                }
            }
        }
        // Empty graph: scrub.
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

/// The segment into `key`: (time, value) of the previous key and of this one (first component).
fn segment(scene: &Scene, key: &KeyRef) -> Option<((f64, f64), (f64, f64))> {
    let keys = model::keyframes(scene, &key.id)?;
    let list = keys.get(&key.property)?;
    let i = list.iter().position(|k| (k.time - key.time).abs() < 1e-4)?;
    if i == 0 {
        return None;
    }
    let (a, b) = (&list[i - 1], &list[i]);
    Some(((a.time, *values(Some(a.value.clone())).first()?), (b.time, *values(Some(b.value.clone())).first()?)))
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

/// Sets the easing of the selected keys (`motion.addKeyframe` with the same value).
fn set_easing(studio: &Entity<Studio>, easing: &str, cx: &mut App) {
    let (found, keys) = {
        let st = studio.read(cx);
        (st.clip_scene(cx), st.keys.clone())
    };
    let Some((clip, scene)) = found else { return };
    let mut commands = vec![];
    for k in &keys {
        let keys: Keyframes = model::keyframes(&scene, &k.id).unwrap_or_default();
        let Some(kf) = keys.get(&k.property).and_then(|l| l.iter().find(|x| (x.time - k.time).abs() < 1e-4)).cloned() else { continue };
        let value = serde_json::to_value(&kf.value).unwrap_or(Value::Null);
        commands.push(json!({ "command": "motion.addKeyframe", "params": { "clipId": clip.id, "id": k.id, "property": k.property, "time": model::timeline_time(&clip, k.time), "value": value, "easing": easing } }));
    }
    if !commands.is_empty() {
        super::run("project.batch", json!({ "commands": commands, "label": "Easing" }), cx);
    }
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
        let span = Self::span(&clip);
        let now = st.scene_time(cx);
        let playing = st.playing;
        let show_graph = st.show_graph;
        let rows = self.rows(&scene, cx);
        let selected_keys = st.keys.clone();
        let studio = self.studio.clone();
        let bounds = self.bounds.clone();
        let (accent, text3, line, text) = (t.accent, t.text_3, t.line, t.text);

        // Header: transport and the view switch.
        let (s1, s2, s3) = (studio.clone(), studio.clone(), studio.clone());
        let header = div()
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .border_b_1()
            .border_color(t.line)
            .child(Button::icon("st-play", if playing { "pause" } else { "play" }, tip(if playing { "Pause" } else { "Play the clip (loops)" }, &act::StudioPlay)).on_click(move |_, _, cx| s1.update(cx, |s, cx| s.toggle_play(cx))))
            .child(div().font_family(MONO).text_size(px(sz::SM)).text_color(t.text).child(format!("{:.2} s", now)))
            .child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(format!("of {:.2}–{:.2} s", span.0, span.1)))
            .child(div().w(px(8.)))
            .child(Button::new("st-key", "Keyframe").small().with_icon("diamond").tooltip(tip("Keyframe the selection at the playhead", &act::StudioInsert)).on_click(move |_, _, cx| s2.update(cx, |s, cx| s.keyframe_selection(cx))))
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
                d.child(Button::icon("st-fit", "maximize-2", "Fit the curve").on_click(move |_, _, cx| this.update(cx, |t, cx| {
                    t.range = None;
                    cx.notify();
                })))
            });

        // Left column: rows' names (dope sheet) or the graphed properties (graph).
        let mut names = div().w(px(NAMES_W)).flex_none().h_full().flex().flex_col().border_r_1().border_color(t.line).child(div().h(px(RULER_H)).flex_none().border_b_1().border_color(t.line));
        let graph_target = self.graph_target(&scene, cx);
        if show_graph {
            if let Some((id, _)) = &graph_target {
                let keys = model::keyframes(&scene, id).unwrap_or_default();
                for (name, list) in &keys {
                    if list.first().is_some_and(|k| matches!(k.value, KeyValue::Text(_))) {
                        continue;
                    }
                    let on = graph_target.as_ref().is_some_and(|(_, p)| p == name);
                    let (s, n) = (studio.clone(), name.clone());
                    let this = cx.entity();
                    names = names.child(
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
                                    s.graph_property = Some(n.clone());
                                    s.changed(cx);
                                })
                            }),
                    );
                }
            } else {
                names = names.child(div().p(px(10.)).text_size(px(sz::XS)).text_color(t.text_3).child("Select an animated thing to see its curves."));
            }
        } else {
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
        // What to paint: diamonds (dope sheet) or curves (graph).
        let mut diamonds: Vec<(f64, f64, bool, bool)> = vec![];
        let mut curves: Vec<(Vec<(f64, f64)>, Hsla)> = vec![];
        let mut points: Vec<(f64, f64, bool)> = vec![];
        let mut handles: Vec<((f64, f64), (f64, f64))> = vec![];
        if show_graph {
            if let Some((id, prop)) = &graph_target {
                let keys = model::keyframes(&scene, id).unwrap_or_default();
                if let Some(list) = keys.get(prop) {
                    let n = values(list.first().map(|k| k.value.clone())).len();
                    let colors = [0xf0565c, 0x7bd88f, 0x5aa7ff];
                    for c in 0..n {
                        let pts: Vec<(f64, f64)> = (0..=200)
                            .filter_map(|i| {
                                let tt = span.0 + (span.1 - span.0) * i as f64 / 200.0;
                                values(value_at(list, tt)).get(c).map(|v| (x_of(tt), self.y_of(*v, &scene, cx)))
                            })
                            .collect();
                        curves.push((pts, if n == 1 { accent } else { gpui::rgb(colors[c % 3]).into() }));
                    }
                    for k in list {
                        let sel = selected_keys.iter().any(|s| s.id == *id && s.property == *prop && (s.time - k.time).abs() < 1e-4);
                        let (kt, kv) = match &self.drag {
                            Some(TDrag::GraphKey { key, to, .. }) if key.id == *id && key.property == *prop && (key.time - k.time).abs() < 1e-4 => (to.0, Some(to.1)),
                            _ => (k.time, None),
                        };
                        for (ci, v) in values(Some(k.value.clone())).into_iter().enumerate() {
                            let v = match (kv, &self.drag) {
                                (Some(nv), Some(TDrag::GraphKey { comp, .. })) if comp.is_none_or(|c| c == ci) => nv,
                                _ => v,
                            };
                            points.push((x_of(kt), self.y_of(v, &scene, cx), sel));
                        }
                    }
                    if let Some(sel) = selected_keys.iter().find(|s| s.id == *id && s.property == *prop)
                        && let Some((a, b)) = segment(&scene, sel)
                    {
                        let ctrl = match &self.drag {
                            Some(TDrag::Handle { ctrl, .. }) => *ctrl,
                            _ => bezier_of(list.iter().find(|k| (k.time - sel.time).abs() < 1e-4).map(|k| k.easing).unwrap_or_default()),
                        };
                        let pa = (x_of(a.0), self.y_of(a.1, &scene, cx));
                        let pb = (x_of(b.0), self.y_of(b.1, &scene, cx));
                        let h1 = (x_of(a.0 + ctrl[0] * (b.0 - a.0)), self.y_of(a.1 + ctrl[1] * (b.1 - a.1), &scene, cx));
                        let h2 = (x_of(a.0 + ctrl[2] * (b.0 - a.0)), self.y_of(a.1 + ctrl[3] * (b.1 - a.1), &scene, cx));
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
                    let shown = if sel { tt + drag_dt } else { *tt };
                    diamonds.push((x_of(shown), y, sel, row.property.is_none()));
                }
            }
        }
        let now_x = x_of(now);
        let ticks: Vec<(f64, String)> = {
            let len = span.1 - span.0;
            let step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| len / s <= 12.0).unwrap_or(60.0);
            let mut out = vec![];
            let mut tt = (span.0 / step).ceil() * step;
            while tt <= span.1 + 1e-9 {
                out.push((x_of(tt), format!("{tt:.2}").trim_end_matches('0').trim_end_matches('.').to_string()));
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
            .child(painter)
            .children(tick_labels)
            .children(hits)
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                this.studio.update(cx, |s, _| s.area = Area::Timeline);
                let (_, by, _, _) = this.track_box();
                let y = f32::from(e.position.y);
                let Some((clip, _)) = this.studio.read(cx).clip_scene(cx) else { return };
                let span = Self::span(&clip);
                if this.studio.read(cx).show_graph && y > by as f32 + RULER_H {
                    this.graph_down(e, cx);
                    return;
                }
                let t = this.t_of(f32::from(e.position.x) as f64, span);
                if y <= by as f32 + RULER_H {
                    this.drag = Some(TDrag::Scrub);
                    super::seek(&this.studio, t.clamp(span.0, span.1), cx);
                } else {
                    this.drag = Some(TDrag::Band { from: (t, y), to: (t, y), additive: e.modifiers.shift });
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                if this.studio.read(cx).show_graph {
                    this.graph_down(e, cx);
                }
            }))
            .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, _, cx| {
                // Dope sheet: scroll the rows. Graph: zoom the values around the middle.
                if !this.studio.read(cx).show_graph {
                    let Some((_, scene)) = this.studio.read(cx).clip_scene(cx) else { return };
                    let n = this.rows(&scene, cx).len() as f32;
                    let (_, _, _, h) = this.track_box();
                    let max = (n * ROW_H + RULER_H - h as f32 + 8.).max(0.);
                    let dy = f32::from(e.delta.pixel_delta(px(16.)).y);
                    this.scroll_y = (this.scroll_y - dy).clamp(0., max);
                    cx.notify();
                    return;
                }
                let Some((_, scene)) = this.studio.read(cx).clip_scene(cx) else { return };
                let (lo, hi) = this.graph_range(&scene, cx);
                let dy = f32::from(e.delta.pixel_delta(px(16.)).y) as f64;
                let k = (1.0 - dy * 0.003).clamp(0.5, 2.0);
                let mid = (lo + hi) / 2.0;
                this.range = Some((mid - (mid - lo) * k, mid + (hi - mid) * k));
                cx.notify();
            }));
        div()
            .id("studio-timeline")
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg_raised)
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(names).child(track))
            .when(self.drag.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_end)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::Keyframe;

    #[test]
    fn named_easings_have_bezier_handles() {
        assert_eq!(bezier_of(Easing::parse("cubicBezier(0.1, 0.2, 0.3, 0.4)").unwrap()), [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(bezier_of(Easing::EASE_OUT)[1], 1.0);
        for (e, _) in EASINGS {
            Easing::parse(e).unwrap_or_else(|err| panic!("{e}: {err}"));
        }
        let _ = Keyframe { time: 0.0, value: KeyValue::Number(0.0), easing: Easing::Linear };
    }
}
