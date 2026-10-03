//! The preview: the composited frame at the playhead (rendered by the export's
//! compositor, so it matches the render), playback with sound, pending generations
//! drawn over the frame, and on-canvas handles to move and scale the selected
//! clip (an animated clip gets a keyframe at the playhead). The canvas is work, so
//! it stays solid (never glass).

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Entity, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, RenderImage,
    Subscription, Task, Window, canvas, div, img, point, prelude::*, px, size,
};
use kimchi_core::{Clip, ClipContent, Fit, Id, Project, TrackKind, Transform};
use kimchi_media::preview::Frame;
use serde_json::{Value, json};

use crate::playback::Playback;
use crate::preview::{AudioBuffer, AudioOut};
use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::actions::{self as act, tip};
use crate::ui::{Button, drag, icon, smpte};

/// Largest frame the preview asks for (pixels, longest side).
const MAX_RENDER: f32 = 1280.;
const TRANSPORT_H: f32 = 44.;

/// Where a layer sits on the canvas, in project pixels (mirrors the export graph).
#[derive(Clone, Copy, Debug)]
struct LayerBox {
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    rotation: f64,
}

impl LayerBox {
    fn contains(&self, x: f64, y: f64) -> bool {
        let r = -self.rotation.to_radians();
        let (dx, dy) = (x - self.cx, y - self.cy);
        let lx = dx * r.cos() - dy * r.sin();
        let ly = dx * r.sin() + dy * r.cos();
        lx.abs() <= self.w / 2.0 && ly.abs() <= self.h / 2.0
    }
}

fn fit_box(t: &Transform, iw: f64, ih: f64, w: f64, h: f64) -> LayerBox {
    let (sx, sy) = match t.fit {
        Fit::Stretch => (w / iw, h / ih),
        Fit::Cover => {
            let s = (w / iw).max(h / ih);
            (s, s)
        }
        Fit::Contain => {
            let s = (w / iw).min(h / ih);
            (s, s)
        }
    };
    LayerBox { cx: w / 2.0 + t.x, cy: h / 2.0 + t.y, w: iw * sx * t.scale, h: ih * sy * t.scale, rotation: t.rotation }
}

fn box_of(p: &Project, clip: &Clip, t: &Transform) -> LayerBox {
    let (w, h) = (p.settings.width as f64, p.settings.height as f64);
    match &clip.content {
        ClipContent::Media { asset_id } => {
            let a = p.asset(*asset_id);
            let iw = a.and_then(|a| a.meta.width).map(f64::from).unwrap_or(w);
            let ih = a.and_then(|a| a.meta.height).map(f64::from).unwrap_or(h);
            fit_box(t, iw, ih, w, h)
        }
        ClipContent::Text { style } => {
            let m = kimchi_media::text::measure(style);
            let pad = if style.background.is_some() { style.font_size * 0.35 } else { 0.0 };
            LayerBox { cx: w / 2.0 + t.x, cy: h / 2.0 + t.y, w: (m.width + pad * 2.0) * t.scale, h: (m.height + pad) * t.scale, rotation: t.rotation }
        }
        ClipContent::Pending { .. } => LayerBox { cx: w / 2.0, cy: h / 2.0, w, h, rotation: 0.0 },
        // A motion scene is drawn on the whole canvas, then placed like a full-frame picture.
        ClipContent::Solid { .. } | ClipContent::Motion { .. } => fit_box(t, w, h, w, h),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    Move,
    Scale,
}

struct CanvasDrag {
    clip: Id,
    mode: DragMode,
    start: Point<Pixels>,
    t0: Transform,
    center: Point<Pixels>,
    d0: f32,
}

struct Playing {
    frames: futures::channel::mpsc::Receiver<(f64, Frame)>,
    queue: VecDeque<(f64, Frame)>,
    synced: bool,
    _task: Task<()>,
    _audio: Option<AudioOut>,
    generation: u64,
}

pub struct PreviewView {
    store: Entity<Store>,
    playback: Entity<Playback>,
    image: Option<Arc<RenderImage>>,
    garbage: Vec<Arc<RenderImage>>,
    /// What the shown frame is: (playhead, project, size).
    shown: Option<(f64, usize, (u32, u32))>,
    rendering: Option<Task<()>>,
    /// A newer frame was asked for while one was rendering.
    stale: bool,
    playing: Option<Playing>,
    viewport: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<CanvasDrag>,
    /// The transform of the clip being dragged, before it is committed.
    live: Option<(Id, Transform)>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl PreviewView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![
            cx.observe(&store, |this, _, cx| this.refresh(cx)),
            cx.observe(&playback, |this, _, cx| this.refresh(cx)),
        ];
        Self {
            store,
            playback,
            image: None,
            garbage: vec![],
            shown: None,
            rendering: None,
            stale: false,
            playing: None,
            viewport: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            live: None,
            error: None,
            _subs: subs,
        }
    }

    /// Size of the stage on screen for the project's aspect ratio.
    fn stage(&self, p: &Project) -> (Bounds<Pixels>, f32) {
        let vp = self.viewport.get();
        let (w, h) = (p.settings.width as f32, p.settings.height as f32);
        let avail_w = (f32::from(vp.size.width) - 48.).max(40.);
        let avail_h = (f32::from(vp.size.height) - 40.).max(40.);
        let scale = (avail_w / w).min(avail_h / h).max(0.02);
        let size = size(px(w * scale), px(h * scale));
        let origin = point(vp.origin.x + (vp.size.width - size.width) / 2., vp.origin.y + (vp.size.height - size.height) / 2.);
        (Bounds { origin, size }, scale)
    }

    /// Pixel size to render at: the stage at the display's scale, capped.
    fn render_size(&self, p: &Project, window_scale: f32) -> (u32, u32) {
        let (stage, _) = self.stage(p);
        let w = f32::from(stage.size.width) * window_scale;
        let h = f32::from(stage.size.height) * window_scale;
        let k = (MAX_RENDER / w.max(h)).min(1.0);
        (((w * k) as u32).max(16) & !1, ((h * k) as u32).max(16) & !1)
    }

    /// Asks for the frame the playhead needs (paused), or starts/stops the stream (playing).
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.store.read(cx).project.clone() else {
            self.playing = None;
            self.image = None;
            self.shown = None;
            cx.notify();
            return;
        };
        let pb = self.playback.read(cx);
        let (playhead, playing, generation) = (pb.playhead, pb.playing, pb.generation);
        if playing {
            if self.playing.as_ref().is_none_or(|p| p.generation != generation) {
                self.start_stream(project, playhead, generation, cx);
            }
            cx.notify();
            return;
        }
        self.playing = None;
        let key = (playhead, Arc::as_ptr(&project) as usize, self.render_size(&project, 2.0));
        if self.shown.is_some_and(|s| s == key) {
            return;
        }
        if self.rendering.is_some() {
            self.stale = true;
            return;
        }
        self.render_frame(project, key, cx);
    }

    fn render_frame(&mut self, project: Arc<Project>, key: (f64, usize, (u32, u32)), cx: &mut Context<Self>) {
        let session = self.store.read(cx).session.clone();
        let (t, _, (w, h)) = key;
        let task = gpui_tokio::Tokio::spawn(cx, crate::preview::render(session, project, t, w, h));
        self.stale = false;
        self.rendering = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.rendering = None;
                match result {
                    Ok(Ok(frame)) => {
                        this.set_image(frame);
                        this.error = None;
                    }
                    Ok(Err(e)) => this.error = Some(e),
                    Err(e) => this.error = Some(e.to_string()),
                }
                // Done with this key, even on failure: a failed frame is retried only when
                // something changes, never in a loop. Something changed while rendering:
                // forget the key so `refresh` catches up with the latest.
                this.shown = if this.stale { None } else { Some(key) };
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_image(&mut self, frame: Frame) {
        if let Some(img) = crate::preview::to_image(frame)
            && let Some(old) = self.image.replace(img)
        {
            self.garbage.push(old);
        }
    }

    fn start_stream(&mut self, project: Arc<Project>, from: f64, generation: u64, cx: &mut Context<Self>) {
        let session = self.store.read(cx).session.clone();
        let (w, h) = self.render_size(&project, 1.0);
        let (tx, rx) = futures::channel::mpsc::channel(4);
        let audio = AudioBuffer::new();
        let audio_out = AudioOut::open(audio.clone());
        if audio_out.is_none() {
            // No output device: play silently instead of piling decoded sound up in memory.
            tracing::debug!("no audio output device; playing without sound");
            audio.close();
        }
        let task = gpui_tokio::Tokio::spawn(cx, crate::preview::stream(session, project, from, w.max(320), h.max(180), tx, audio));
        let task = cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = task.await {
                this.update(cx, |this, cx| {
                    this.error = Some(e);
                    cx.notify();
                })
                .ok();
            }
        });
        self.playing = Some(Playing { frames: rx, queue: VecDeque::new(), synced: false, _task: task, _audio: audio_out, generation });
    }

    /// While playing: advance the clock and show the latest frame that is due.
    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(play) = self.playing.as_mut() else { return };
        let ended = loop {
            match play.frames.try_recv() {
                Ok(f) => play.queue.push_back(f),
                Err(e) => break e.is_closed(),
            }
        };
        // Start the clock when the first frame is there, so picture and sound begin together.
        // A stream that ends (or fails) before any frame just lets the clock run.
        if !play.synced {
            if let Some((pts, _)) = play.queue.front() {
                let pts = *pts;
                play.synced = true;
                self.playback.update(cx, |p, _| p.resync(pts));
            } else if ended {
                play.synced = true;
                let now = self.playback.read(cx).playhead;
                self.playback.update(cx, |p, _| p.resync(now));
            } else {
                window.request_animation_frame();
                return;
            }
        }
        let still = self.playback.update(cx, |p, cx| p.tick(cx));
        let now = self.playback.read(cx).playhead;
        let mut due = None;
        if let Some(play) = self.playing.as_mut() {
            while play.queue.front().is_some_and(|(pts, _)| *pts <= now + 1e-3) {
                due = play.queue.pop_front();
            }
        }
        if let Some((pts, frame)) = due {
            self.set_image(frame);
            self.shown = Some((pts, 0, (0, 0)));
        }
        if still {
            window.request_animation_frame();
        }
    }

    // ---- on-canvas handles ---------------------------------------------------

    /// Visible picture layers at the playhead, bottom first.
    fn layers(&self, p: &Project, t: f64) -> Vec<(Clip, bool, LayerBox)> {
        let mut out = vec![];
        for track in p.tracks.iter().rev().filter(|tr| tr.kind == TrackKind::Video && !tr.hidden) {
            for c in track.clips.iter().filter(|c| t >= c.start && t < c.end()) {
                // Where the clip is at the playhead (keyframes applied).
                let tf = match &self.live {
                    Some((id, tf)) if *id == c.id => tf.clone(),
                    _ => c.placement_at(t).transform(),
                };
                out.push((c.clone(), track.locked, box_of(p, c, &tf)));
            }
        }
        out
    }

    fn to_project(&self, p: &Project, pos: Point<Pixels>) -> (f64, f64) {
        let (stage, scale) = self.stage(p);
        (f32::from(pos.x - stage.origin.x) as f64 / scale as f64, f32::from(pos.y - stage.origin.y) as f64 / scale as f64)
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.store.read(cx).project.clone() else { return };
        let t = self.playback.read(cx).playhead;
        let (x, y) = self.to_project(&p, e.position);
        let hit = self.layers(&p, t).into_iter().rev().find(|(c, _, b)| !matches!(c.content, ClipContent::Pending { .. }) && b.contains(x, y));
        let Some((clip, locked, b)) = hit else {
            self.store.update(cx, |s, cx| s.clear_selection(cx));
            return;
        };
        let additive = e.modifiers.shift;
        self.store.update(cx, |s, cx| {
            if !s.selection.contains(&clip.id) {
                s.select(clip.id, additive, cx);
            }
        });
        if !locked {
            self.begin_drag(clip, DragMode::Move, e.position, b, &p, t);
        }
        cx.notify();
    }

    fn begin_drag(&mut self, clip: Clip, mode: DragMode, at: Point<Pixels>, b: LayerBox, p: &Project, playhead: f64) {
        let (stage, scale) = self.stage(p);
        let center = point(stage.origin.x + px(b.cx as f32 * scale), stage.origin.y + px(b.cy as f32 * scale));
        let d0 = ((at - center).magnitude() as f32).max(1.0);
        // Start from where the clip is now, keyframes included.
        let t0 = clip.placement_at(playhead).transform();
        self.drag = Some(CanvasDrag { clip: clip.id, mode, start: at, t0: t0.clone(), center, d0 });
        self.live = Some((clip.id, t0));
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.drag else { return };
        let Some(p) = self.store.read(cx).project.clone() else { return };
        let (_, scale) = self.stage(&p);
        let mut t = d.t0.clone();
        match d.mode {
            DragMode::Move => {
                let mut x = d.t0.x + f32::from(e.position.x - d.start.x) as f64 / scale as f64;
                let mut y = d.t0.y + f32::from(e.position.y - d.start.y) as f64 / scale as f64;
                // Snap to the canvas centre lines.
                if x.abs() < 12.0 / scale as f64 {
                    x = 0.0;
                }
                if y.abs() < 12.0 / scale as f64 {
                    y = 0.0;
                }
                t.x = x.round();
                t.y = y.round();
            }
            DragMode::Scale => {
                let dist = (e.position - d.center).magnitude() as f32;
                t.scale = ((d.t0.scale * (dist / d.d0) as f64) * 100.0).round().max(5.0) / 100.0;
            }
        }
        self.live = Some((d.clip, t));
        cx.notify();
    }

    fn drag_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        if let Some((id, t)) = self.live.clone()
            && (t.x != d.t0.x || t.y != d.t0.y || t.scale != d.t0.scale)
        {
            // An animated property gets a keyframe at the playhead; a still one changes.
            let animated = self.store.read(cx).clip(id).map(|c| c.keyframes.clone()).unwrap_or_default();
            let playhead = self.playback.read(cx).playhead;
            let key = |name: &str, value: Value| json!({ "command": "clip.addKeyframe", "params": { "clipId": id, "property": name, "time": playhead, "value": value } });
            let mut commands = vec![];
            let mut still = serde_json::Map::new();
            if t.x != d.t0.x || t.y != d.t0.y {
                if animated.contains_key("position") {
                    commands.push(key("position", json!([t.x, t.y])));
                } else {
                    for (name, v) in [("x", t.x), ("y", t.y)] {
                        if animated.contains_key(name) {
                            commands.push(key(name, json!(v)));
                        } else {
                            still.insert(name.into(), json!(v));
                        }
                    }
                }
            }
            if t.scale != d.t0.scale {
                if animated.contains_key("scale") {
                    commands.push(key("scale", json!(t.scale)));
                } else {
                    still.insert("scale".into(), json!(t.scale));
                }
            }
            if !still.is_empty() {
                still.insert("clipId".into(), json!(id));
                commands.push(json!({ "command": "clip.update", "params": Value::Object(still) }));
            }
            self.store.update(cx, |s, cx| match commands.len() {
                1 => {
                    let c = commands.remove(0);
                    s.run_then(c["command"].as_str().unwrap_or("clip.update"), c["params"].clone(), cx, |_, _, _| {});
                }
                _ => s.run_then("project.batch", json!({ "commands": commands, "label": "Move clip" }), cx, |_, _, _| {}),
            });
            // Keep the live transform until the project reflects it (avoids a jump back).
            self.live = Some((id, t));
            let store = self.store.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                this.update(cx, |this, cx| {
                    let _ = &store;
                    this.live = None;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        } else {
            self.live = None;
        }
        cx.notify();
    }

    fn transport(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let pb = self.playback.read(cx);
        let s = self.store.read(cx);
        let fps = s.fps();
        let (now, total, playing, looping, shuttle) = (pb.playhead, s.duration(), pb.moving(), pb.looping, pb.shuttle);
        let scale_pct = s.project.as_ref().map(|p| (self.stage(p).1 * 100.0).round() as i32).unwrap_or(100);
        let pb_entity = self.playback.clone();
        let seek = move |time: f64| {
            let pb = pb_entity.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| pb.update(cx, |p, cx| p.seek(time, cx))
        };
        let pb_step = self.playback.clone();
        let step = move |frames: f64| {
            let pb = pb_step.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| pb.update(cx, |p, cx| p.step(frames, cx))
        };
        let pb_toggle = self.playback.clone();
        let pb_loop = self.playback.clone();
        div()
            .h(px(TRANSPORT_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .border_t_1()
            .border_color(t.line)
            .child(
                div()
                    .w(px(200.))
                    .flex()
                    .gap(px(6.))
                    .font_family(MONO)
                    .text_size(px(sz::SM))
                    .child(div().text_color(t.text).child(smpte(now, fps)))
                    .child(div().text_color(t.text_3).child(format!("/ {}", smpte(total, fps))))
                    .when(shuttle != 0., |d| d.child(div().text_color(t.accent_text).child(crate::views::timeline::rate_label(shuttle)))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(Button::icon("tp-start", "skip-back", tip("Go to start", &act::GoToStart)).on_click(seek(0.0)))
                    .child(Button::icon("tp-prev", "step-back", tip("Previous frame", &act::StepBack)).on_click(step(-1.0)))
                    .child(
                        div()
                            .id("tp-play")
                            .size(px(34.))
                            .mx(px(4.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(t.accent)
                            .text_color(t.text_on_accent)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.accent_hover))
                            .tooltip(move |_, cx| crate::ui::tooltip(tip(if playing { "Pause" } else { "Play" }, &act::PlayPause), cx))
                            .on_click(move |_, _, cx| pb_toggle.update(cx, |p, cx| p.toggle(cx)))
                            .child(icon(if playing { "pause" } else { "play" }).size(px(16.)).text_color(t.text_on_accent)),
                    )
                    .child(Button::icon("tp-next", "step-forward", tip("Next frame", &act::StepForward)).on_click(step(1.0)))
                    .child(Button::icon("tp-end", "skip-forward", tip("Go to end", &act::GoToEnd)).on_click(seek(total))),
            )
            .child(
                div()
                    .w(px(200.))
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(Button::icon("tp-loop", "repeat", tip(if looping { "Loop: on" } else { "Loop: off" }, &act::ToggleLoop)).selected(looping).on_click(move |_, _, cx| {
                        pb_loop.update(cx, |p, cx| {
                            p.looping = !p.looping;
                            cx.notify();
                        })
                    }))
                    .child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(format!("{scale_pct}%"))),
            )
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for old in self.garbage.drain(..) {
            let _ = window.drop_image(old);
        }
        if self.playing.is_some() {
            self.pump(window, cx);
        }
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let project = s.project.clone();
        let selection = s.selection.clone();
        let jobs = s.jobs.clone();
        let empty = project.as_ref().is_none_or(|p| p.tracks.iter().all(|t| t.clips.is_empty()));
        let playhead = self.playback.read(cx).playhead;
        let viewport = self.viewport.clone();
        let entity = cx.entity();
        let accent = t.accent;

        let stage = project.as_ref().map(|p| {
            let (b, scale) = self.stage(p);
            let vp = self.viewport.get();
            let (left, top) = (b.origin.x - vp.origin.x, b.origin.y - vp.origin.y);
            let layers = self.layers(p, playhead);
            let bg = crate::theme::parse_color(&p.settings.background);
            let mut el = div().absolute().left(left).top(top).w(b.size.width).h(b.size.height).bg(bg).rounded(px(4.)).overflow_hidden().border_1().border_color(t.line_strong).shadow(t.glass_shadow());
            if let Some(image) = self.image.clone() {
                el = el.child(img(image).absolute().inset_0().size_full().object_fit(ObjectFit::Fill));
            }
            // Pending generations aren't in the render: show what is coming.
            for (c, _, lb) in layers.iter().filter(|(c, _, _)| matches!(c.content, ClipContent::Pending { .. })) {
                let ClipContent::Pending { prompt, model_name, job_id, .. } = &c.content else { continue };
                let job = jobs.iter().find(|j| &j.id == job_id);
                let frac = job.and_then(|j| j.progress.fraction).unwrap_or(0.04) as f32;
                let msg = job.and_then(|j| j.progress.message.clone()).unwrap_or_else(|| "Queued".into());
                el = el.child(
                    div()
                        .absolute()
                        .left(px((lb.cx - lb.w / 2.0) as f32 * scale))
                        .top(px((lb.cy - lb.h / 2.0) as f32 * scale))
                        .w(px(lb.w as f32 * scale))
                        .h(px(lb.h as f32 * scale))
                        .bg(t.bg_sunken.opacity(0.85))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .p(px(24.))
                        .child(icon("sparkles").size(px(22.)).text_color(t.accent_text))
                        .child(div().text_size(px(sz::MD)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).text_center().child(prompt.clone()))
                        .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(format!("{model_name} · {msg}")))
                        .child(div().w(px(180.)).h(px(4.)).rounded_full().bg(t.line_strong).child(div().h_full().rounded_full().bg(t.accent).w(px(180. * frac.clamp(0.02, 1.0))))),
                );
            }
            // The selected clip's box with its scale handles.
            if let Some((c, locked, lb)) = layers.iter().rev().find(|(c, _, _)| selection.contains(&c.id)).cloned()
                && !locked
                && !matches!(c.content, ClipContent::Pending { .. })
            {
                let (x, y, w, h) = ((lb.cx - lb.w / 2.0) as f32 * scale, (lb.cy - lb.h / 2.0) as f32 * scale, lb.w as f32 * scale, lb.h as f32 * scale);
                let mut sel = div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).border_1().border_color(t.accent);
                for (i, (hx, hy)) in [(0., 0.), (1., 0.), (0., 1.), (1., 1.)].into_iter().enumerate() {
                    let clip = c.clone();
                    let p = p.clone();
                    sel = sel.child(
                        div()
                            .id(("handle", i))
                            .absolute()
                            .left(px(w * hx - 5.))
                            .top(px(h * hy - 5.))
                            .size(px(10.))
                            .rounded(px(2.))
                            .bg(gpui::white())
                            .border_1()
                            .border_color(t.accent)
                            .cursor(if (hx + hy) as i32 % 2 == 0 { gpui::CursorStyle::ResizeUpLeftDownRight } else { gpui::CursorStyle::ResizeUpRightDownLeft })
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                let playhead = this.playback.read(cx).playhead;
                                this.begin_drag(clip.clone(), DragMode::Scale, e.position, lb, &p, playhead);
                                cx.notify();
                            })),
                    );
                }
                el = el.child(sel);
            }
            el
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("preview-viewport")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    // Media dropped on the picture goes on the timeline at the playhead.
                    .drag_over::<crate::views::timeline::dnd::MediaDrag>(move |st, _, _, _| st.border_2().border_color(accent))
                    .on_drop::<crate::views::timeline::dnd::MediaDrag>(|d, _, cx| {
                        let asset = d.asset_id;
                        cx.store().update(cx, |s, cx| s.run_then("clip.insertMedia", json!({ "assetId": asset }), cx, |s, v, cx| s.set_selection(crate::app::created(&v), cx)));
                    })
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                if viewport.get() != bounds {
                                    viewport.set(bounds);
                                    entity.update(cx, |this, cx| this.refresh(cx));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(stage)
                    .when(empty && project.is_some(), |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(24.))
                                .w_full()
                                .flex()
                                .justify_center()
                                .child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Drop media here, or generate something from the left panel.")),
                        )
                    })
                    .when_some(self.error.clone(), |d, e| {
                        d.child(div().absolute().top(px(10.)).left(px(10.)).right(px(10.)).text_size(px(sz::XS)).text_color(t.danger).child(format!("Preview: {e}")))
                    }),
            )
            .child(self.transport(cx))
            .when(self.drag.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_end)))
    }
}
