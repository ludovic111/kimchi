//! The tracks: ruler, track headers, lanes and clips, with every pointer
//! interaction on them (scrub, select, rubber-band select, move, ⌥-drag copy,
//! trim, fades, drops from the media panel and the desktop, scroll, zoom, menus). It is drawn as a cached view, so playback does
//! not re-render it: the playhead is painted over it by [`super::Timeline`].
//!
//! Drags preview locally and end in one registry command (`clip.moveMany`,
//! `clip.trim`, `clip.update`, `transition.set`, `track.move`) with a coalesce key; the preview
//! stays until the session announces the changed project, so nothing jumps.
//! Transitions draw over the clips ([`transitions`]).

mod sound;
mod transitions;

pub(in crate::views::timeline) use sound::{Geometry, beat_points, decorations, fades, header_meters, shows_line, wave};
#[cfg(test)]
pub(in crate::views::timeline) use sound::line_y as line_y_for_tests;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Context, ElementId, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PinchEvent, Pixels, ScrollWheelEvent, Subscription,
    Window, canvas, div, pattern_slash, prelude::*, px,
};
use kimchi_core::{Id, Project, TrackKind};
use serde_json::{Value, json};

use super::clip::ClipView;
use super::dnd::MediaDrag;
use super::geom::{self, HEADER_W, RULER_H, TAIL, track_h};
use super::menus;
use super::waveform::{self, PeaksCache};
use crate::playback::Playback;
use crate::store::{MAX_PPS, MIN_PPS, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, parse_color, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, drag};

/// How long the agent's (or MCP / CLI) new clips keep the accent.
const AGENT_MARK: Duration = Duration::from_secs(60);

enum Drag {
    Scrub { points: Vec<f64> },
    /// Rubber band from empty lane space: (time, content y) where it began and where it is.
    Marquee { from: (f64, f32), to: (f64, f32), x0: Pixels, additive: bool, base: Vec<Id>, started: bool },
    Move(MoveDrag),
    Trim { clip: Id, start_edge: bool, points: Vec<f64>, time: Option<f64> },
    Fade { clip: Id, out: bool, value: Option<f64> },
    /// A transition's edge: its incoming clip, the cut it is centred on (or where it starts).
    Transition { clip: Id, cut: Option<f64>, start: f64, value: Option<f64> },
    Track { id: Id, from: usize, to: usize, y0: Pixels, started: bool },
}

struct MoveDrag {
    ids: Vec<Id>,
    clip_start: f64,
    clip_len: f64,
    t0: f64,
    ti0: usize,
    kind: TrackKind,
    x0: Pixels,
    started: bool,
    dt: f64,
    shift: isize,
    points: Vec<f64>,
    /// ⌥ held: drop copies, leave the originals.
    copy: bool,
}

/// What a drag shows before the project has changed.
#[derive(Clone)]
enum Preview {
    Move { ids: Vec<Id>, dt: f64, shift: isize, copy: bool },
    Trim { clip: Id, start_edge: bool, time: f64 },
    Fade { clip: Id, out: bool, value: f64 },
    Transition { clip: Id, duration: f64 },
}

/// Where dragged media would land.
#[derive(Clone, PartialEq)]
struct DropHint {
    row: Option<usize>,
    time: f64,
    len: f64,
    ok: bool,
}

/// Clips the agent, MCP or the CLI created or touched (from their command records), for the accent mark.
#[derive(Default)]
struct AgentMarks {
    project: Option<Id>,
    marked: HashMap<Id, Instant>,
    last_seq: u64,
}

pub struct TimelineBody {
    store: Entity<Store>,
    playback: Entity<Playback>,
    /// Horizontal scroll in pixels, vertical scroll of the tracks.
    pub scroll_x: f64,
    pub scroll_y: f32,
    last_pps: f64,
    /// The lanes area (right of the headers, under the ruler), in window coordinates, as last drawn.
    pub lanes: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    /// A finished drag's preview, until the project it was made on is replaced.
    committed: Option<(Preview, usize)>,
    guide: Option<f64>,
    drop: Option<DropHint>,
    peaks: PeaksCache,
    rename: Option<(Id, Entity<TextInput>, Subscription)>,
    agent: AgentMarks,
    /// A clip, handle or marker took this mouse down; the lane / ruler under it ignores it.
    /// (Propagation isn't stopped, so the workspace still closes menus and takes focus.)
    consumed: bool,
    /// A clip's volume line being dragged.
    volume: Option<sound::VolumeDrag>,
    /// Taken on any click in the tracks (GPUI focuses a tracked element on mouse down), so the
    /// timeline's shortcuts (Delete, S, Space…) apply to what was clicked.
    focus: gpui::FocusHandle,
    _subs: Vec<Subscription>,
}

fn project_key(p: &Arc<Project>) -> usize {
    Arc::as_ptr(p) as usize
}

impl TimelineBody {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![
            cx.observe(&store, |this, _, cx| {
                this.track_agent(cx);
                cx.notify();
            }),
            // Keep the playhead in view while playing (only re-renders when the view jumps).
            cx.observe(&playback, |this, pb, cx| {
                let p = pb.read(cx);
                if !p.playing || this.drag.is_some() {
                    return;
                }
                let pps = this.store.read(cx).pps;
                let x = p.playhead * pps;
                let w = this.view_w();
                if x > this.scroll_x + w - 40. || x < this.scroll_x {
                    this.scroll_x = (x - 60.).max(0.);
                    cx.notify();
                }
            }),
        ];
        let pps = store.read(cx).pps;
        Self {
            store,
            playback,
            scroll_x: 0.,
            scroll_y: 0.,
            last_pps: pps,
            lanes: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            committed: None,
            guide: None,
            drop: None,
            peaks: PeaksCache::default(),
            rename: None,
            agent: AgentMarks::default(),
            consumed: false,
            volume: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    // ---- coordinates ---------------------------------------------------------

    pub fn view_w(&self) -> f64 {
        f64::from(f32::from(self.lanes.get().size.width)).max(1.)
    }

    fn pps(&self, cx: &App) -> f64 {
        self.store.read(cx).pps
    }

    fn time_at(&self, x: Pixels, cx: &App) -> f64 {
        let b = self.lanes.get();
        ((f32::from(x - b.origin.x) as f64 + self.scroll_x) / self.pps(cx)).max(0.)
    }

    fn content_y(&self, y: Pixels) -> f32 {
        f32::from(y - self.lanes.get().origin.y) + self.scroll_y
    }

    fn content_w(&self, cx: &App) -> f64 {
        let s = self.store.read(cx);
        self.view_w().max((s.duration() + TAIL) * s.pps)
    }

    fn clamp_scroll(&mut self, cx: &App) {
        let max_x = (self.content_w(cx) - self.view_w()).max(0.);
        self.scroll_x = self.scroll_x.clamp(0., max_x);
        let total = self.store.read(cx).project.as_ref().map(|p| geom::rows(&p.tracks).total).unwrap_or(0.) + 40.;
        let view_h = f32::from(self.lanes.get().size.height);
        self.scroll_y = self.scroll_y.clamp(0., (total - view_h).max(0.));
    }

    fn playhead(&self, cx: &App) -> f64 {
        self.playback.read(cx).playhead
    }

    // ---- zoom & scroll -------------------------------------------------------

    /// Zooms keeping the time under `anchor` (a window x) where it is; without one, the playhead
    /// if it is in view, else the left edge.
    pub fn zoom_to(&mut self, pps: f64, anchor: Option<Pixels>, cx: &mut Context<Self>) {
        let old = self.pps(cx);
        let new = pps.clamp(MIN_PPS, MAX_PPS);
        if (new - old).abs() < 1e-9 {
            return;
        }
        let b = self.lanes.get();
        let ax = match anchor {
            Some(x) => f32::from(x - b.origin.x) as f64,
            None => {
                let x = self.playhead(cx) * old - self.scroll_x;
                if (0. ..=self.view_w()).contains(&x) { x } else { 0. }
            }
        };
        let t = (ax + self.scroll_x) / old;
        self.scroll_x = (t * new - ax).max(0.);
        self.last_pps = new;
        self.store.update(cx, |s, cx| s.set_zoom(new, cx));
        self.clamp_scroll(cx);
        cx.notify();
    }

    /// Whole project in view.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        let d = self.store.read(cx).duration().max(5.);
        let pps = ((self.view_w() - 40.) / d).clamp(MIN_PPS, MAX_PPS);
        self.scroll_x = 0.;
        self.last_pps = pps;
        self.store.update(cx, |s, cx| s.set_zoom(pps, cx));
        cx.notify();
    }

    fn on_wheel(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(px(20.));
        let (dx, dy) = (f32::from(d.x) as f64, f32::from(d.y));
        if e.modifiers.control || e.modifiers.platform || e.modifiers.alt {
            let pps = self.pps(cx);
            self.zoom_to(pps * (dy as f64 * 0.01).exp(), Some(e.position.x), cx);
        } else if e.modifiers.shift {
            self.scroll_x -= if dx.abs() > 0. { dx } else { dy as f64 };
        } else {
            self.scroll_x -= dx;
            self.scroll_y -= dy;
        }
        self.clamp_scroll(cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn on_pinch(&mut self, e: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pps = self.pps(cx);
        self.zoom_to(pps * (1. + e.delta as f64), Some(e.position.x), cx);
    }

    // ---- pointer -------------------------------------------------------------

    /// The ruler or the playhead line: move the playhead, and follow the pointer.
    fn scrub_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.consumed) {
            return;
        }
        if self.store.read(cx).project.is_none() {
            return;
        }
        let points = self.store.read(cx).project.as_ref().map(|p| geom::snap_points(p, &[], None)).unwrap_or_default();
        let t = self.time_at(e.position.x, cx);
        self.playback.update(cx, |p, cx| {
            p.pause(cx);
            p.seek(t, cx);
        });
        self.drag = Some(Drag::Scrub { points });
        cx.notify();
    }

    /// Empty lane space: grabbing the playhead line scrubs; anything else starts a rubber band
    /// (a plain click, without dragging, moves the playhead there and clears the selection).
    fn lanes_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        // A clip, handle or marker took it.
        if std::mem::take(&mut self.consumed) || self.store.read(cx).project.is_none() {
            return;
        }
        let head = self.playhead(cx) * self.pps(cx) - self.scroll_x;
        let x = f32::from(e.position.x - self.lanes.get().origin.x) as f64;
        if (x - head).abs() <= 5. {
            return self.scrub_down(e, cx);
        }
        let additive = e.modifiers.shift || e.modifiers.platform || e.modifiers.control;
        let base = if additive { self.store.read(cx).selection.clone() } else { vec![] };
        let at = (self.time_at(e.position.x, cx), self.content_y(e.position.y));
        self.drag = Some(Drag::Marquee { from: at, to: at, x0: e.position.x, additive, base, started: false });
        cx.notify();
    }

    /// Clips the rubber band touches.
    fn in_band(&self, p: &Project, (t0, y0): (f64, f32), (t1, y1): (f64, f32)) -> Vec<Id> {
        let (ta, tb) = (t0.min(t1), t0.max(t1));
        let (ya, yb) = (y0.min(y1), y0.max(y1));
        let rows = geom::rows(&p.tracks);
        let mut out = vec![];
        for (i, track) in p.tracks.iter().enumerate() {
            let (top, bottom) = (rows.tops[i] + 2., rows.tops[i] + 2. + track_h(track.kind));
            if bottom < ya || top > yb {
                continue;
            }
            out.extend(track.clips.iter().filter(|c| c.end() > ta && c.start < tb).map(|c| c.id));
        }
        out
    }

    pub fn clip_down(&mut self, id: Id, e: &MouseDownEvent, cx: &mut Context<Self>) {
        // A trim or fade handle inside the clip already took it.
        if self.consumed {
            return;
        }
        self.consumed = true;
        if e.click_count == 2 {
            // A motion clip: into the Studio.
            if matches!(self.store.read(cx).clip(id).map(|c| &c.content), Some(kimchi_core::ClipContent::Motion { .. })) {
                self.store.update(cx, |s, cx| {
                    s.select(id, false, cx);
                    s.open_studio(id, cx);
                });
                return;
            }
            // A title: straight to its words.
            if matches!(self.store.read(cx).clip(id).map(|c| &c.content), Some(kimchi_core::ClipContent::Text { .. })) {
                self.store.update(cx, |s, cx| {
                    s.select(id, false, cx);
                    cx.emit(crate::store::StoreEvent::EditText);
                });
                return;
            }
        }
        let additive = e.modifiers.shift || e.modifiers.platform || e.modifiers.control;
        let selected = self.store.read(cx).selection.contains(&id);
        if !selected || additive {
            self.store.update(cx, |s, cx| s.select(id, additive, cx));
        }
        let s = self.store.read(cx);
        let Some(p) = s.project.clone() else { return };
        let Some((ti, ci)) = p.locate_clip(id) else { return };
        let track = &p.tracks[ti];
        if track.locked {
            return;
        }
        let ids = if s.selection.contains(&id) { s.selection.clone() } else { vec![id] };
        let clip = &track.clips[ci];
        let playhead = self.playhead(cx);
        self.drag = Some(Drag::Move(MoveDrag {
            points: geom::snap_points(&p, &ids, Some(playhead)),
            ids,
            clip_start: clip.start,
            clip_len: clip.duration,
            t0: self.time_at(e.position.x, cx),
            ti0: ti,
            kind: track.kind,
            x0: e.position.x,
            started: false,
            dt: 0.,
            shift: 0,
            copy: e.modifiers.alt,
        }));
        cx.notify();
    }

    pub fn edge_down(&mut self, id: Id, start_edge: bool, _: &MouseDownEvent, cx: &mut Context<Self>) {
        // A transition badge or "+" drawn over the edge took it.
        if std::mem::replace(&mut self.consumed, true) {
            return;
        }
        self.store.update(cx, |s, cx| s.select(id, false, cx));
        let Some(p) = self.store.read(cx).project.clone() else { return };
        let playhead = self.playhead(cx);
        self.drag = Some(Drag::Trim { clip: id, start_edge, points: geom::snap_points(&p, &[id], Some(playhead)), time: None });
        cx.notify();
    }

    pub fn fade_down(&mut self, id: Id, out: bool, _: &MouseDownEvent, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.consumed, true) {
            return;
        }
        self.store.update(cx, |s, cx| s.select(id, false, cx));
        self.drag = Some(Drag::Fade { clip: id, out, value: None });
        cx.notify();
    }

    fn header_down(&mut self, id: Id, index: usize, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.click_count == 2 {
            // The rename field keeps the focus (the tracks would take it on this mouse down).
            window.prevent_default();
            self.start_rename(id, window, cx);
            return;
        }
        self.drag = Some(Drag::Track { id, from: index, to: index, y0: e.position.y, started: false });
        cx.notify();
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pps = self.pps(cx);
        let time = self.time_at(e.position.x, cx);
        let snapping = self.store.read(cx).snapping;
        let project = self.store.read(cx).project.clone();
        let cy = self.content_y(e.position.y);
        match self.drag.as_mut() {
            Some(Drag::Scrub { points }) => {
                let t = if snapping { geom::snap_time(time, points, pps).0 } else { time };
                self.playback.update(cx, |p, cx| p.seek(t, cx));
                return;
            }
            Some(Drag::Marquee { from, to, x0, base, started, .. }) => {
                let Some(p) = project else { return };
                if !*started && f32::from(e.position.x - *x0).abs() < 4. && (cy - from.1).abs() < 4. {
                    return;
                }
                *started = true;
                *to = (time, cy);
                let (from, to, base) = (*from, *to, base.clone());
                let mut sel = base;
                for id in self.in_band(&p, from, to) {
                    if !sel.contains(&id) {
                        sel.push(id);
                    }
                }
                if self.store.read(cx).selection != sel {
                    self.store.update(cx, |s, cx| s.set_selection(sel, cx));
                }
            }
            Some(Drag::Move(m)) => {
                m.copy = e.modifiers.alt;
                let Some(p) = project else { return };
                let rows = geom::rows(&p.tracks);
                let over = geom::row_at(&p.tracks, &rows, cy);
                if !m.started && f32::from(e.position.x - m.x0).abs() < 4. && over == Some(m.ti0) {
                    return;
                }
                m.started = true;
                let raw = (m.clip_start + (time - m.t0)).max(0.);
                let (start, guide) = if snapping { geom::snap_span(raw, m.clip_len, &m.points, pps) } else { (raw, None) };
                let ti = over.filter(|&i| p.tracks[i].kind == m.kind && !p.tracks[i].locked).unwrap_or(m.ti0);
                m.dt = start - m.clip_start;
                m.shift = ti as isize - m.ti0 as isize;
                self.guide = guide;
            }
            Some(Drag::Trim { points, time: tr, .. }) => {
                let (t, guide) = if snapping { geom::snap_time(time, points, pps) } else { (time, None) };
                *tr = Some(t);
                self.guide = guide;
            }
            Some(Drag::Fade { clip, out, value }) => {
                let Some(c) = project.as_ref().and_then(|p| p.clip(*clip)) else { return };
                let v = if *out { c.end() - time } else { time - c.start };
                let other = if *out { c.fade_in } else { c.fade_out };
                *value = Some(v.clamp(0., (c.duration - other).max(0.)));
            }
            Some(Drag::Transition { cut, start, value, .. }) => {
                *value = Some(Self::transition_len(*cut, *start, time));
            }
            Some(Drag::Track { to, y0, started, .. }) => {
                let Some(p) = project else { return };
                if !*started && f32::from(e.position.y - *y0).abs() < 4. {
                    return;
                }
                *started = true;
                let rows = geom::rows(&p.tracks);
                *to = geom::insertion_at(&p.tracks, &rows, cy);
            }
            None => return,
        }
        cx.notify();
    }

    fn drag_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        self.guide = None;
        // The drag's commands are one undo step, however slowly they land.
        let key = format!("gesture:timeline-drag-{}", crate::ui::scrub::new_gesture());
        let Some(p) = self.store.read(cx).project.clone() else {
            cx.notify();
            return;
        };
        match drag {
            Drag::Scrub { .. } => {}
            Drag::Marquee { from, additive, started: false, .. } => {
                // Just a click: move the playhead there.
                if !additive {
                    self.store.update(cx, |s, cx| s.clear_selection(cx));
                }
                self.playback.update(cx, |pb, cx| {
                    pb.pause(cx);
                    pb.seek(from.0, cx);
                });
            }
            Drag::Marquee { .. } => {}
            Drag::Move(m) if m.copy && m.started => {
                // ⌥-drag: copies land where the clips were dropped; the originals stay.
                let mut clips = vec![];
                for (ti, t) in p.tracks.iter().enumerate() {
                    for c in t.clips.iter().filter(|c| m.ids.contains(&c.id)) {
                        let dest = usize::try_from(ti as isize + m.shift).ok().and_then(|i| p.tracks.get(i)).filter(|d| d.kind == t.kind && !d.locked).unwrap_or(t);
                        let mut v = json!(c);
                        v["trackId"] = json!(dest.id);
                        v["start"] = json!((c.start + m.dt).max(0.));
                        clips.push(v);
                    }
                }
                let time = clips.iter().filter_map(|v| v["start"].as_f64()).fold(f64::INFINITY, f64::min);
                if !clips.is_empty() {
                    let preview = Preview::Move { ids: m.ids, dt: m.dt, shift: m.shift, copy: true };
                    self.commit_then(preview, &p, "clip.paste", json!({ "clips": clips, "time": time }), cx, |s, v, cx| s.set_selection(crate::app::created(&v), cx));
                }
            }
            Drag::Move(m) => {
                if m.started && (m.dt.abs() > 1e-6 || m.shift != 0) {
                    let mut moves = vec![];
                    for (ti, t) in p.tracks.iter().enumerate() {
                        for c in t.clips.iter().filter(|c| m.ids.contains(&c.id)) {
                            let dest = usize::try_from(ti as isize + m.shift).ok().and_then(|i| p.tracks.get(i)).filter(|d| d.kind == t.kind && !d.locked).unwrap_or(t);
                            moves.push(json!({ "clipId": c.id, "trackId": dest.id, "start": (c.start + m.dt).max(0.) }));
                        }
                    }
                    self.commit(Preview::Move { ids: m.ids, dt: m.dt, shift: m.shift, copy: false }, &p, "clip.moveMany", json!({ "moves": moves, "coalesce": key }), cx);
                }
            }
            Drag::Trim { clip, start_edge, time: Some(time), .. } => {
                let params = json!({ "clipId": clip, "edge": if start_edge { "start" } else { "end" }, "time": time, "coalesce": key });
                self.commit(Preview::Trim { clip, start_edge, time }, &p, "clip.trim", params, cx);
            }
            Drag::Fade { clip, out, value: Some(value) } => {
                let field = if out { "fadeOut" } else { "fadeIn" };
                self.commit(Preview::Fade { clip, out, value }, &p, "clip.update", json!({ "clipId": clip, field: value, "coalesce": key }), cx);
            }
            Drag::Transition { clip, value: Some(duration), .. } => {
                self.commit(Preview::Transition { clip, duration }, &p, "transition.set", json!({ "clipIds": [clip], "duration": duration, "coalesce": key }), cx);
            }
            Drag::Track { id, from, to, started: true, .. } => {
                let index = if to > from { to - 1 } else { to };
                if index != from {
                    self.store.update(cx, |s, cx| s.run("track.move", json!({ "trackId": id, "index": index }), cx));
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Runs the command a drag ends in, keeping its preview until the project changes.
    fn commit(&mut self, preview: Preview, p: &Arc<Project>, name: &str, params: Value, cx: &mut Context<Self>) {
        self.commit_then(preview, p, name, params, cx, |_, _, _| {});
    }

    fn commit_then(&mut self, preview: Preview, p: &Arc<Project>, name: &str, params: Value, cx: &mut Context<Self>, then: impl FnOnce(&mut Store, Value, &mut Context<Store>) + 'static) {
        self.committed = Some((preview, project_key(p)));
        let task = self.store.update(cx, |s, cx| s.call(name, params, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(v) => this.store.update(cx, |s, cx| then(s, v, cx)),
                    Err(e) => {
                        this.committed = None;
                        this.store.update(cx, |s, cx| s.error(e, cx));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn preview(&self, p: &Arc<Project>) -> Option<Preview> {
        match &self.drag {
            Some(Drag::Move(m)) if m.started => Some(Preview::Move { ids: m.ids.clone(), dt: m.dt, shift: m.shift, copy: m.copy }),
            Some(Drag::Trim { clip, start_edge, time: Some(t), .. }) => Some(Preview::Trim { clip: *clip, start_edge: *start_edge, time: *t }),
            Some(Drag::Fade { clip, out, value: Some(v) }) => Some(Preview::Fade { clip: *clip, out: *out, value: *v }),
            Some(Drag::Transition { clip, value: Some(v), .. }) => Some(Preview::Transition { clip: *clip, duration: *v }),
            _ => self.committed.as_ref().filter(|(_, k)| *k == project_key(p)).map(|(pr, _)| pr.clone()),
        }
    }

    // ---- media dropped from the media panel ---------------------------------

    fn drop_move(&mut self, e: &gpui::DragMoveEvent<MediaDrag>, cx: &mut Context<Self>) {
        let pos = e.event.position;
        let b = self.lanes.get();
        let hint = (|| {
            if !b.contains(&pos) {
                return None;
            }
            let p = self.store.read(cx).project.clone()?;
            let a = p.asset(e.drag(cx).asset_id)?;
            let want = if a.kind == kimchi_core::MediaKind::Audio { TrackKind::Audio } else { TrackKind::Video };
            let len = a.duration().unwrap_or(kimchi_core::DEFAULT_STILL_DURATION);
            let rows = geom::rows(&p.tracks);
            let row = geom::row_at(&p.tracks, &rows, self.content_y(pos.y));
            let points = geom::snap_points(&p, &[], Some(self.playhead(cx)));
            let t = self.time_at(pos.x, cx);
            let pps = self.pps(cx);
            let time = if self.store.read(cx).snapping { geom::snap_span(t, len, &points, pps).0 } else { t };
            let ok = row.is_none_or(|r| p.tracks[r].kind == want && !p.tracks[r].locked);
            Some(DropHint { row, time, len, ok })
        })();
        if hint != self.drop {
            self.drop = hint;
            cx.notify();
        }
    }

    fn dropped(&mut self, d: &MediaDrag, cx: &mut Context<Self>) {
        let Some(hint) = self.drop.take() else { return };
        cx.notify();
        if !hint.ok {
            return;
        }
        // The row was measured during the drag; the tracks may have changed since (an agent, undo).
        let track = hint.row.and_then(|r| self.store.read(cx).project.as_ref().and_then(|p| p.tracks.get(r).map(|t| t.id)));
        let mut params = json!({ "assetId": d.asset_id, "start": hint.time });
        if let Some(t) = track {
            params["trackId"] = json!(t);
        }
        self.store.update(cx, |s, cx| s.run_then("clip.insertMedia", params, cx, |s, v, cx| s.set_selection(crate::app::created(&v), cx)));
    }

    // ---- files dropped from the desktop -------------------------------------------

    fn files_move(&mut self, e: &gpui::DragMoveEvent<gpui::ExternalPaths>, cx: &mut Context<Self>) {
        let pos = e.event.position;
        let b = self.lanes.get();
        let hint = (|| {
            if !b.contains(&pos) {
                return None;
            }
            let p = self.store.read(cx).project.clone()?;
            let rows = geom::rows(&p.tracks);
            let row = geom::row_at(&p.tracks, &rows, self.content_y(pos.y));
            let points = geom::snap_points(&p, &[], Some(self.playhead(cx)));
            let t = self.time_at(pos.x, cx);
            let time = if self.store.read(cx).snapping { geom::snap_time(t, &points, self.pps(cx)).0 } else { t };
            Some(DropHint { row: row.filter(|r| !p.tracks[*r].locked), time, len: 0., ok: true })
        })();
        if hint != self.drop {
            self.drop = hint;
            cx.notify();
        }
    }

    /// Files from the desktop: imported, and placed one after the other where they were dropped.
    fn files_dropped(&mut self, paths: &gpui::ExternalPaths, cx: &mut Context<Self>) {
        let hint = self.drop.take();
        cx.notify();
        let list: Vec<String> = paths.paths().iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let Some(p) = self.store.read(cx).project.clone() else { return };
        if list.is_empty() {
            return;
        }
        let time = hint.as_ref().map(|h| h.time).unwrap_or_else(|| self.playhead(cx));
        let mut params = json!({ "paths": list, "place": true, "start": time });
        if let Some(t) = hint.and_then(|h| h.row).and_then(|r| p.tracks.get(r)) {
            params["trackId"] = json!(t.id);
        }
        self.store.update(cx, |s, cx| {
            s.dropping = false;
            s.run_then("media.import", params, cx, |s, v, cx| {
                let n = v["media"].as_array().map(Vec::len).unwrap_or(0);
                s.toast(kimchi_control::ToastKind::Success, format!("Imported and placed {}", crate::app::count(n, "file")), cx);
            });
        });
    }

    // ---- renaming ------------------------------------------------------------

    pub fn start_rename(&mut self, track: Id, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.store.read(cx).project.as_ref().and_then(|p| p.tracks.iter().find(|t| t.id == track)).map(|t| t.name.clone()).unwrap_or_default();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx);
            i.set_text(name.clone(), cx);
            i.select_all_text(cx);
            i
        });
        crate::ui::input::focus(&input, window, cx);
        let sub = cx.subscribe(&input, move |this, input, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Blur => {
                let text = input.read(cx).text().trim().to_string();
                if !text.is_empty() && text != name {
                    this.store.update(cx, |s, cx| s.run("track.update", json!({ "trackId": track, "name": text }), cx));
                }
                this.rename = None;
                cx.notify();
            }
            InputEvent::Cancel => {
                this.rename = None;
                cx.notify();
            }
            InputEvent::Changed(_) => {}
        });
        self.rename = Some((track, input, sub));
        cx.notify();
    }

    // ---- the agent's recent work ---------------------------------------------

    fn track_agent(&mut self, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let Some(p) = s.project.clone() else {
            self.agent = AgentMarks::default();
            return;
        };
        let last_seq = s.commands.last().map(|r| r.seq).unwrap_or(0);
        let a = &mut self.agent;
        if a.project != Some(p.id) {
            *a = AgentMarks { project: Some(p.id), last_seq, ..Default::default() };
            return;
        }
        let now = Instant::now();
        let mut marked = false;
        for r in s.commands.iter().filter(|r| r.seq > a.last_seq && r.ok && r.mutates) {
            // What the command created (from its result) and the clips it named.
            let mut ids: Vec<Id> = r.created.clone();
            let one = |v: &Value| v.as_str().and_then(|s| s.parse::<Id>().ok());
            ids.extend(one(&r.params["clipId"]));
            ids.extend(r.params["clipIds"].as_array().into_iter().flatten().filter_map(one));
            ids.extend(r.params["moves"].as_array().into_iter().flatten().filter_map(|m| one(&m["clipId"])));
            for id in ids.into_iter().filter(|id| p.clip(*id).is_some()) {
                a.marked.insert(id, now);
                marked = true;
            }
        }
        a.last_seq = last_seq.max(a.last_seq);
        a.marked.retain(|_, at| now.duration_since(*at) < AGENT_MARK);
        if marked {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(AGENT_MARK + Duration::from_millis(50)).await;
                this.update(cx, |this, cx| {
                    this.agent.marked.retain(|_, at| at.elapsed() < AGENT_MARK);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn peaks_for(&mut self, path: &str, cx: &mut Context<Self>) -> Option<waveform::Peaks> {
        let (peaks, load) = self.peaks.get(path);
        if load {
            let path = path.to_string();
            cx.spawn(async move |this, cx| {
                let p = path.clone();
                let data = cx.background_spawn(async move { waveform::read(&p) }).await;
                this.update(cx, |this, cx| {
                    this.peaks.put(path, data);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        peaks
    }

    // ---- drawing -------------------------------------------------------------

    fn ruler(&self, p: Option<&Arc<Project>>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let (pps, fps) = (s.pps, s.fps());
        let (ticks, _) = geom::ticks(pps, self.scroll_x, self.view_w());
        let markers = p.map(|p| p.markers.clone()).unwrap_or_default();
        div()
            .h(px(RULER_H))
            .flex_none()
            .flex()
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .w(px(HEADER_W))
                    .flex_none()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(10.))
                    .pr(px(4.))
                    .border_r_1()
                    .border_color(t.line)
                    .child(crate::ui::caps("Tracks", cx))
                    .child(crate::ui::stop(
                        Button::new("add-track", "Add track").with_icon("plus").small().ghost().tooltip("Add a video or audio track").on_click(|e, _, cx| menus::add_track_menu(e.position(), cx)),
                    )),
            )
            .child(
                div()
                    .id("ruler")
                    .role(gpui::Role::Slider)
                    .aria_label("Playhead position")
                    .flex_1()
                    .h_full()
                    .relative()
                    .overflow_hidden()
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, e, _, cx| this.scrub_down(e, cx)))
                    .children(ticks.into_iter().map(|tk| {
                        let x = (tk.t * pps - self.scroll_x) as f32;
                        div()
                            .absolute()
                            .left(px(x))
                            .bottom_0()
                            .w(px(1.))
                            .h(px(if tk.major { 10. } else { 5. }))
                            .bg(if tk.major { t.text_3 } else { t.line_strong })
                            .when(tk.major, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .left(px(5.))
                                        .bottom(px(8.))
                                        .font_family(MONO)
                                        .text_size(px(10.))
                                        .text_color(t.text_3)
                                        .whitespace_nowrap()
                                        .child(geom::tick_label(tk.t, pps, fps)),
                                )
                            })
                    }))
                    .children(markers.into_iter().map(|m| {
                        let x = (m.time * pps - self.scroll_x) as f32;
                        let color = parse_color(&m.color);
                        let (mid, time) = (m.id, m.time);
                        let label: gpui::SharedString = if m.label.is_empty() { "Marker".into() } else { m.label.clone().into() };
                        div()
                            .id(ElementId::Uuid(m.id))
                            .absolute()
                            .left(px(x - 5.))
                            .bottom_0()
                            .w(px(10.))
                            .h(px(14.))
                            .cursor_pointer()
                            .tooltip(move |_, cx| crate::ui::tooltip(format!("{label} — right-click to remove").into(), cx))
                            .child(pentagon(10., 12., color))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                this.consumed = true;
                                this.playback.update(cx, |p, cx| p.seek(time, cx));
                            }))
                            .on_mouse_down(MouseButton::Right, move |e, _, cx| {
                                cx.stop_propagation();
                                menus::marker_menu(mid, e.position, cx);
                            })
                    })),
            )
            .into_any_element()
    }

    fn headers(&self, p: &Arc<Project>, rows: &geom::Rows, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let view_h = f32::from(self.lanes.get().size.height);
        let renaming = self.rename.as_ref().map(|(id, input, _)| (*id, input.clone()));
        let reorder = match &self.drag {
            Some(Drag::Track { to, started: true, .. }) => Some(*to),
            _ => None,
        };
        let count = p.tracks.len();
        div()
            .id("track-headers")
            .w(px(HEADER_W))
            .flex_none()
            .h_full()
            .relative()
            .overflow_hidden()
            .border_r_1()
            .border_color(t.line)
            .children(p.tracks.iter().enumerate().filter_map(|(i, track)| {
                let top = rows.tops[i] - self.scroll_y;
                let h = track_h(track.kind);
                if top + h < 0. || top > view_h + 4. {
                    return None;
                }
                let id = track.id;
                let toggle = |name: &'static str, on: bool, icon_on: &'static str, icon_off: &'static str, tip_on: &'static str, tip_off: &'static str, key: &'static str| {
                    Button::icon(name, if on { icon_on } else { icon_off }, if on { tip_on } else { tip_off })
                        .small()
                        .variant(crate::ui::Variant::Secondary)
                        .selected(on)
                        .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.run("track.update", json!({ "trackId": id, key: !on }), cx)))
                };
                let name_el = match &renaming {
                    Some((rid, input)) if *rid == id => div().flex_1().min_w_0().child(crate::ui::stop(input.clone())).into_any_element(),
                    _ => {
                        let full: gpui::SharedString = track.name.clone().into();
                        div()
                            .id(("track-name", i))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(sz::SM))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            // The whole name when the header cuts it short.
                            .tooltip(move |_, cx| crate::ui::tooltip(full.clone(), cx))
                            .child(track.name.clone())
                            .into_any_element()
                    }
                };
                // V1 is the bottom picture track, A1 the top sound track (as editors count them).
                let code = if track.captions {
                    "CC".to_string()
                } else if track.kind == TrackKind::Video {
                    format!("V{}", p.tracks[i..].iter().filter(|o| o.kind == TrackKind::Video).count())
                } else {
                    format!("A{}", p.tracks[..=i].iter().filter(|o| o.kind == TrackKind::Audio).count())
                };
                let chip = div()
                    .flex_none()
                    .min_w(px(24.))
                    .h(px(18.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(t.text)
                    .text_color(t.bg)
                    .font_family(crate::theme::MONO)
                    .text_size(px(10.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(code);
                Some(
                    div()
                        .id(ElementId::Uuid(id))
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(top + 2.))
                        .h(px(h))
                        // Two lines: what the track is, then its switches, always in view.
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(5.))
                        .pl(px(10.))
                        .pr(px(14.))
                        .border_l_2()
                        .border_color(gpui::transparent_black())
                        .cursor_grab()
                        .hover(|s| s.bg(t.hover).border_color(t.line_strong))
                        .child(div().flex().items_center().gap(px(7.)).min_w_0().child(chip).child(name_el))
                        .child(crate::ui::stop(
                            div()
                                .flex()
                                .gap(px(3.))
                                .when(track.kind == TrackKind::Video, |d| d.child(toggle("hide", track.hidden, "eye-off", "eye", "Show", "Hide", "hidden")))
                                .child(toggle("mute", track.muted, "volume-x", "volume-2", "Unmute", "Mute", "muted"))
                                .children(sound::header_toggles(track, cx))
                                .child(toggle("lock", track.locked, "lock", "lock-open", "Unlock", "Lock", "locked")),
                        ))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, e, window, cx| this.header_down(id, i, e, window, cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| menus::track_menu(this, i, e.position, cx))),
                )
            }))
            .when_some(reorder, |d, to| {
                let y = if to < count { rows.tops[to] } else { rows.total } - self.scroll_y;
                d.child(div().absolute().left_0().right_0().top(px(y)).h(px(2.)).bg(t.accent))
            })
            .into_any_element()
    }

    fn lanes(&mut self, p: &Arc<Project>, rows: &geom::Rows, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let pps = s.pps;
        let selection: HashSet<Id> = s.selection.iter().copied().collect();
        let jobs = s.jobs.clone();
        let view_w = self.view_w();
        let view_h = f32::from(self.lanes.get().size.height);
        let preview = self.preview(p);
        let (scroll_x, scroll_y) = (self.scroll_x, self.scroll_y);
        let visible_row = |top: f32, h: f32| top - scroll_y + h >= 0. && top - scroll_y <= view_h + 4.;

        // Where the project ends: past it, the lanes are hatched.
        let end_x = (p.duration() * pps - scroll_x) as f32;
        // Lane backgrounds: click to move the playhead (and clear the selection), right-click for the gap menu.
        let lane_els: Vec<_> = p
            .tracks
            .iter()
            .enumerate()
            .filter(|(i, tr)| visible_row(rows.tops[*i], track_h(tr.kind)))
            .map(|(i, tr)| {
                div()
                    .id(("lane", i))
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(rows.tops[i] - scroll_y + 2.))
                    .h(px(track_h(tr.kind)))
                    .rounded(px(sz::R_SM + 2.))
                    .bg(t.bg_sunken)
                    .when(tr.locked, |d| d.bg(pattern_slash(t.line, 1., 8.)))
                    .when(!tr.locked && end_x < view_w as f32, |d| {
                        d.child(div().absolute().top_0().bottom_0().right_0().left(px(end_x.max(0.))).border_l_1().border_color(t.line_strong).bg(pattern_slash(t.line_strong, 1., 6.)))
                    })
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        let time = this.time_at(e.position.x, cx);
                        menus::lane_menu(this, i, time, e.position, cx);
                    }))
            })
            .collect();

        // What to draw: each clip as shown (a drag may have moved or trimmed it), on which row;
        // an ⌥-drag draws the originals in place and their copies where they would land.
        let mut draws: Vec<(usize, std::borrow::Cow<kimchi_core::Clip>, usize, bool, bool)> = vec![];
        for (ti, track) in p.tracks.iter().enumerate() {
            for clip in &track.clips {
                let mut shown = std::borrow::Cow::Borrowed(clip);
                let mut row = ti;
                let mut moving = false;
                match &preview {
                    Some(Preview::Move { ids, dt, shift, copy }) if ids.contains(&clip.id) => {
                        let mut moved = clip.clone();
                        moved.start = (clip.start + dt).max(0.);
                        let dest = ti as isize + shift;
                        let to = if dest >= 0 && (dest as usize) < p.tracks.len() && p.tracks[dest as usize].kind == track.kind && !p.tracks[dest as usize].locked { dest as usize } else { ti };
                        if *copy {
                            draws.push((ti, std::borrow::Cow::Owned(moved), to, true, true));
                        } else {
                            shown = std::borrow::Cow::Owned(moved);
                            row = to;
                            moving = self.drag.is_some();
                        }
                    }
                    Some(Preview::Trim { clip: id, start_edge, time }) if *id == clip.id => {
                        shown = std::borrow::Cow::Owned(geom::trimmed(track, clip, &p.assets, *start_edge, *time));
                    }
                    Some(Preview::Fade { clip: id, out, value }) if *id == clip.id => {
                        if *out {
                            shown.to_mut().fade_out = *value;
                        } else {
                            shown.to_mut().fade_in = *value;
                        }
                    }
                    _ => {}
                }
                if let Some(c) = self.volume_shown(&shown) {
                    shown = std::borrow::Cow::Owned(c);
                }
                draws.push((ti, shown, row, moving, false));
            }
        }
        // Moving clips, copies and the selection last, so they draw on top.
        let mut items = vec![];
        let mut on_top = vec![];
        for (ti, shown, row, moving, ghost) in draws {
            let track = &p.tracks[ti];
            let h = track_h(p.tracks[row].kind);
            if !visible_row(rows.tops[row], h) {
                continue;
            }
            let (x, w) = (shown.start * pps, shown.duration * pps);
            if x + w < scroll_x - 64. || x > scroll_x + view_w + 64. {
                continue;
            }
            let asset = shown.asset_id().and_then(|a| p.asset(a));
            let peaks = asset.and_then(|a| a.waveform.as_ref()).map(|w| w.path.clone()).and_then(|path| self.peaks_for(&path, cx));
            let job = match &shown.content {
                kimchi_core::ClipContent::Pending { job_id, .. } => jobs.iter().find(|j| &j.id == job_id),
                _ => None,
            };
            let id = shown.id;
            let render = crate::views::studio::render_state::state(self.store.read(cx), p, &shown);
            let view = ClipView {
                clip: &shown,
                asset,
                job,
                pps,
                scroll_x,
                view_w,
                top: rows.tops[row] - scroll_y + 2.,
                h,
                selected: selection.contains(&id) && !ghost,
                moving,
                ghost,
                muted: track.muted || track.hidden,
                locked: track.locked,
                agent: self.agent.marked.contains_key(&id),
                peaks,
                render,
            };
            let Some(el) = view.render(cx) else { continue };
            if ghost {
                on_top.push(el);
                continue;
            }
            let el = el
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e, _, cx| this.clip_down(id, e, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    menus::clip_menu(this, id, e.position, cx);
                }));
            if moving || selection.contains(&id) { on_top.push(el) } else { items.push(el) }
        }
        items.extend(on_top);
        let transition_els = self.transitions(p, rows, preview.as_ref(), cx);

        let drop_el = self.drop.clone().filter(|_| cx.has_active_drag()).map(|d| {
            let (top, h) = match d.row {
                Some(r) => (rows.tops[r] + 2., track_h(p.tracks[r].kind)),
                None => (rows.total + 2., 50.),
            };
            div()
                .absolute()
                .left(px((d.time * pps - scroll_x) as f32))
                .top(px(top - scroll_y))
                .w(px(((d.len * pps) as f32).max(3.)))
                .h(px(h))
                .rounded(px(sz::R_SM + 1.))
                .border_2()
                .when(d.ok, |e| e.bg(t.accent_soft).border_color(t.accent))
                .when(!d.ok, |e| e.bg(t.danger.opacity(0.10)).border_color(t.danger.opacity(0.6)))
        });
        let guide = self.guide.map(|g| div().absolute().top_0().bottom_0().left(px((g * pps - scroll_x) as f32)).w(px(1.)).bg(t.text.opacity(0.7)));
        let band = match &self.drag {
            Some(Drag::Marquee { from, to, started: true, .. }) => {
                let (x0, x1) = ((from.0.min(to.0) * pps - scroll_x) as f32, (from.0.max(to.0) * pps - scroll_x) as f32);
                let (y0, y1) = (from.1.min(to.1) - scroll_y, from.1.max(to.1) - scroll_y);
                Some(div().absolute().left(px(x0)).top(px(y0)).w(px(x1 - x0)).h(px(y1 - y0)).border_1().border_color(t.accent).bg(t.accent_soft.opacity(0.5)))
            }
            _ => None,
        };

        let measure = {
            let cell = self.lanes.clone();
            let this = cx.entity().downgrade();
            canvas(
                move |bounds, _, cx| {
                    if cell.get() != bounds {
                        cell.set(bounds);
                        cx.defer(move |cx| {
                            this.update(cx, |this, cx| {
                                this.clamp_scroll(cx);
                                cx.notify();
                            })
                            .ok();
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        div()
            .id("lanes")
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e, _, cx| this.lanes_down(e, cx)))
            .child(measure)
            .children(lane_els)
            .children(items)
            .children(transition_els)
            .children(drop_el)
            .children(guide)
            .children(band)
            .when(p.tracks.iter().all(|t| t.clips.is_empty()), |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(rows.total + 16. - scroll_y))
                        .flex()
                        .justify_center()
                        .text_size(px(sz::SM))
                        .text_color(t.text_3)
                        .child("Drag media or files here, generate a shot, or press T for a title"),
                )
            })
            .into_any_element()
    }
}

/// The marker / playhead flag: a box with a point at the bottom.
pub fn pentagon(w: f32, h: f32, color: gpui::Hsla) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let o = b.origin;
            let at = |x: f32, y: f32| gpui::point(o.x + px(x), o.y + px(y));
            let mut p = gpui::PathBuilder::fill();
            p.move_to(at(0., 0.));
            p.line_to(at(w, 0.));
            p.line_to(at(w, h * 0.62));
            p.line_to(at(w / 2., h));
            p.line_to(at(0., h * 0.62));
            p.close();
            if let Ok(path) = p.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w(px(w))
    .h(px(h))
    .into_any_element()
}

impl Render for TimelineBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let project = s.project.clone();
        self.sync_sound_settings(cx);
        // Zoom changed elsewhere (keys, menus, ui.zoom): keep the playhead where it is if it is in
        // view, else the time at the left edge.
        if (s.pps - self.last_pps).abs() > 1e-9 {
            let at = self.playback.read(cx).playhead * self.last_pps - self.scroll_x;
            let anchor = if (0. ..=self.view_w()).contains(&at) { at } else { 0. };
            self.scroll_x = (anchor + self.scroll_x) / self.last_pps * s.pps - anchor;
            self.last_pps = s.pps;
        }
        self.clamp_scroll(cx);
        if self.committed.as_ref().is_some_and(|(_, k)| project.as_ref().is_none_or(|p| project_key(p) != *k)) {
            self.committed = None;
        }
        if !cx.has_active_drag() {
            self.drop = None;
        }
        let rows = project.as_ref().map(|p| geom::rows(&p.tracks)).unwrap_or(geom::Rows { tops: vec![], total: 0. });
        let ruler = self.ruler(project.as_ref(), cx);
        let main = project.as_ref().map(|p| {
            let headers = self.headers(p, &rows, cx);
            let lanes = self.lanes(p, &rows, cx);
            div().flex_1().min_h_0().flex().child(headers).child(lanes)
        });
        let dragging = self.drag.is_some();
        let cursor = match &self.drag {
            Some(Drag::Trim { .. }) | Some(Drag::Fade { .. }) | Some(Drag::Transition { .. }) => Some(gpui::CursorStyle::ResizeLeftRight),
            Some(Drag::Move(m)) if m.copy && m.started => Some(gpui::CursorStyle::DragCopy),
            Some(Drag::Track { started: true, .. }) => Some(gpui::CursorStyle::ClosedHand),
            _ => None,
        };
        div()
            .id("timeline-body")
            .track_focus(&self.focus)
            .role(gpui::Role::Group)
            .aria_label("Timeline")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(t.bg_raised)
            .when_some(cursor, |d, c| d.cursor(c))
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .on_pinch(cx.listener(Self::on_pinch))
            .on_drag_move::<MediaDrag>(cx.listener(|this, e, _, cx| this.drop_move(e, cx)))
            .on_drop::<MediaDrag>(cx.listener(|this, d, _, cx| this.dropped(d, cx)))
            .on_drag_move::<gpui::ExternalPaths>(cx.listener(|this, e, _, cx| this.files_move(e, cx)))
            .on_drop::<gpui::ExternalPaths>(cx.listener(|this, paths, _, cx| this.files_dropped(paths, cx)))
            .child(ruler)
            .children(main)
            .when(dragging, |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_up)))
            .children(self.sound_tracker(cx))
    }
}

