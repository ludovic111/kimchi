//! Sound on the timeline: the volume line over a clip's waveform (drag it to set the level;
//! with keyframes, drag their points in time and level, double-click the line to add one,
//! right-click a point to remove it), fades drawn in their curve, the waveform scaled by the
//! clip's gain, the beats of music as ticks along its bottom, a ryolune mark on song clips,
//! solo and record arm on track headers, and the tracks' meters beside their headers.
//!
//! A volume drag previews locally and ends in one command (`clip.update` volume, or
//! `clip.setKeyframes` volume) with a coalesce key: one undo step.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{AnyElement, App, Bounds, Context, ElementId, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Window, canvas, div, fill, point, prelude::*, px, size};
use kimchi_core::audio::{FadeCurve, gain_to_db};
use kimchi_core::{Asset, Clip, Id, KeyValue, Project, TrackKind};
use serde_json::json;

use super::TimelineBody;
use crate::theme::{ActiveTheme, Theme};
use crate::ui::tooltip;
use crate::views::mixer::{live, widgets};
use crate::views::timeline::geom::{self, HEADER_W, track_h};
use crate::views::timeline::waveform::Peaks;

/// The volume line's band starts under the clip's label.
const LINE_TOP: f32 = 20.;

/// Dragged clips and the playhead also stick to the beats of music (Settings › Audio).
pub(in crate::views::timeline) static SNAP_BEATS: AtomicBool = AtomicBool::new(false);

/// Timeline times of every beat of the project's music clips, for snapping.
pub(in crate::views::timeline) fn beat_points(p: &Project) -> Vec<f64> {
    if !SNAP_BEATS.load(Ordering::Relaxed) {
        return vec![];
    }
    let mut out = vec![];
    for (_, c) in p.clips() {
        if let Some(b) = c.asset_id().and_then(|a| p.asset(a)).and_then(|a| a.beats.as_ref()) {
            out.extend(kimchi_control::commands::audio::clip_beats(c, b, 1, 0));
        }
    }
    out
}

/// A volume drag: the clip, the keyframe (index) or the whole line, and where it started.
pub(in crate::views::timeline) struct VolumeDrag {
    clip: Id,
    key: Option<usize>,
    x0: Pixels,
    y0: Pixels,
    /// Fader position of the level when it started, and the line's band height.
    pos0: f32,
    band: f32,
    /// The keyframe's clip-local time when it started.
    t0: f64,
    /// The level (factor) and time shown now.
    volume: f64,
    time: f64,
    moved: bool,
}

/// The band's y (clip-local) for a level factor.
pub(in crate::views::timeline) fn line_y(volume: f64, h: f32) -> f32 {
    let band = (h - LINE_TOP - 3.).max(6.);
    LINE_TOP + band * (1. - widgets::db_to_pos(gain_to_db(volume)))
}

/// Whether a clip shows its volume line: clips with sound on audio tracks, and selected ones.
pub(in crate::views::timeline) fn shows_line(p: &Project, clip: &Clip, selected: bool) -> bool {
    kimchi_control::commands::audio::has_sound(p, clip) && (selected || p.locate_clip(clip.id).is_some_and(|(t, _)| p.tracks[t].kind == TrackKind::Audio))
}

impl TimelineBody {
    /// The clip as a volume drag shows it.
    pub(super) fn volume_shown(&self, clip: &Clip) -> Option<Clip> {
        let d = self.volume.as_ref().filter(|d| d.clip == clip.id && d.moved)?;
        let mut c = clip.clone();
        match d.key {
            None => c.volume = d.volume,
            Some(i) => {
                if let Some(k) = c.keyframes.get_mut("volume").and_then(|l| l.get_mut(i)) {
                    k.time = d.time;
                    k.value = KeyValue::Number(d.volume);
                }
            }
        }
        Some(c)
    }

    pub(in crate::views::timeline) fn volume_down(&mut self, clip: &Clip, key: Option<usize>, h: f32, e: &MouseDownEvent, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.consumed, true) {
            return;
        }
        let id = clip.id;
        if e.click_count == 2 {
            // A keyframe on the line where it was double-clicked.
            let t = self.time_at(e.position.x, cx).clamp(clip.start, clip.end());
            self.store.update(cx, |s, cx| s.run("clip.addKeyframe", json!({ "clipId": id, "property": "volume", "time": t }), cx));
            return;
        }
        self.store.update(cx, |s, cx| s.select(id, false, cx));
        let (volume, t0) = match key.and_then(|i| clip.keyframes.get("volume").and_then(|l| l.get(i))) {
            Some(k) => (k.value.as_f64().unwrap_or(clip.volume), k.time),
            None => (clip.volume, 0.),
        };
        self.volume = Some(VolumeDrag { clip: id, key, x0: e.position.x, y0: e.position.y, pos0: widgets::db_to_pos(gain_to_db(volume)), band: (h - LINE_TOP - 3.).max(6.), t0, volume, time: t0, moved: false });
        cx.notify();
    }

    fn volume_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pps = self.pps(cx);
        let Some(d) = self.volume.as_mut() else { return };
        let (dx, dy) = (f32::from(e.position.x - d.x0), f32::from(d.y0 - e.position.y));
        if !d.moved && dx.abs() < 3. && dy.abs() < 3. {
            return;
        }
        d.moved = true;
        let fine = if e.modifiers.shift { 0.2 } else { 1. };
        let pos = (d.pos0 + dy / d.band * fine).clamp(0., 1.);
        d.volume = kimchi_core::audio::db_to_gain(widgets::pos_to_db(pos)).min(4.);
        if d.key.is_some() {
            d.time = (d.t0 + dx as f64 / pps).max(0.);
        }
        cx.notify();
    }

    fn volume_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.volume.take() else { return };
        cx.notify();
        if !d.moved {
            return;
        }
        let Some(clip) = self.store.read(cx).clip(d.clip).cloned() else { return };
        self.drags += 1;
        let key = format!("volume-line-{}", self.drags);
        match d.key {
            None => self.store.update(cx, |s, cx| s.run("clip.update", json!({ "clipId": d.clip, "volume": d.volume, "coalesce": key }), cx)),
            Some(i) => {
                let mut list = clip.keyframes.get("volume").cloned().unwrap_or_default();
                if let Some(k) = list.get_mut(i) {
                    // Between its neighbours, inside the clip.
                    let lo = if i > 0 { list_time(&clip, i - 1) + 1e-3 } else { 0. };
                    let hi = clip.keyframes.get("volume").and_then(|l| l.get(i + 1)).map(|k| k.time - 1e-3).unwrap_or(clip.duration);
                    k.time = d.time.clamp(lo, hi.max(lo));
                    k.value = KeyValue::Number(d.volume);
                }
                let keys: Vec<serde_json::Value> = list.iter().map(|k| json!({ "time": k.time, "value": k.value.as_f64().unwrap_or(1.), "easing": k.easing })).collect();
                self.store.update(cx, |s, cx| s.run("clip.setKeyframes", json!({ "clipId": d.clip, "property": "volume", "keyframes": keys, "coalesce": key }), cx));
            }
        }
    }

    /// While a volume drag is on, the window's pointer goes to it.
    pub(super) fn sound_tracker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.volume.as_ref().map(|_| crate::ui::drag::track(cx.entity(), Self::volume_move, Self::volume_up))
    }

    pub(super) fn sync_sound_settings(&self, cx: &App) {
        SNAP_BEATS.store(self.store.read(cx).settings.audio.snap_to_beats, Ordering::Relaxed);
    }
}

fn list_time(clip: &Clip, i: usize) -> f64 {
    clip.keyframes.get("volume").and_then(|l| l.get(i)).map(|k| k.time).unwrap_or(0.)
}

/// What the sound decorations of a clip need: its clip-local geometry.
pub(in crate::views::timeline) struct Geometry {
    /// Clip-local pixels drawn, and where the clip's x = 0 sits in the drawn box.
    pub vis: (f32, f32),
    pub off: f32,
    /// The clip's full width and height.
    pub wf: f32,
    pub h: f32,
    pub pps: f64,
}

/// The volume line (and its keyframe points), its hit areas, beat ticks and the ryolune mark.
pub(in crate::views::timeline) fn decorations(_p: &Project, clip: &Clip, asset: Option<&Asset>, g: &Geometry, line: bool, interactive: bool, cx: &mut Context<TimelineBody>) -> Vec<AnyElement> {
    let t = cx.theme().clone();
    let mut out = vec![];
    // Beats of music, as ticks along the bottom (downbeats taller).
    if let Some(b) = asset.and_then(|a| a.beats.as_ref())
        && g.pps * 60. / b.tempo.max(1.) >= 4.
    {
        let times = kimchi_control::commands::audio::clip_beats(clip, b, 1, 0);
        let bars = kimchi_control::commands::audio::clip_beats(clip, b, b.beats_per_bar as usize, 0);
        let (off, h, pps, start) = (g.off, g.h, g.pps, clip.start);
        let color = t.text.opacity(0.35);
        let strong = t.accent_text.opacity(0.8);
        out.push(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, _| {
                    for (list, len, c) in [(&times, 4., color), (&bars, 7., strong)] {
                        for tm in list.iter() {
                            let x = off + ((tm - start) * pps) as f32;
                            window.paint_quad(fill(Bounds::new(point(bounds.origin.x + px(x), bounds.origin.y + px(h - len)), size(px(1.), px(len))), c));
                        }
                    }
                },
            )
            .absolute()
            .inset_0()
            .into_any_element(),
        );
    }
    // A ryolune song: its mark at the right end.
    if asset.is_some_and(|a| matches!(a.origin, kimchi_core::AssetOrigin::Song(_))) && g.wf >= 60. {
        let right = (g.vis.1 - g.vis.0).min(g.wf + g.off) - 4.;
        out.push(
            div()
                .absolute()
                .top(px(4.))
                .left(px((right - 18.).max(2.)))
                .size(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(gpui::black().opacity(0.55))
                .child(crate::views::mixer::ryolune_mark(11., cx))
                .into_any_element(),
        );
    }
    if !line || g.wf < 16. || g.h < LINE_TOP + 12. {
        return out;
    }
    // The line itself.
    let keys: Vec<(f64, f64)> = clip.keyframes.get("volume").map(|l| l.iter().map(|k| (k.time, k.value.as_f64().unwrap_or(1.))).collect()).unwrap_or_default();
    let c = clip.clone();
    let (off, h, pps, vis) = (g.off, g.h, g.pps, g.vis);
    let color = t.accent;
    out.push(
        canvas(
            |_, _, _| (),
            move |bounds: Bounds<Pixels>, _, window, _| {
                let o = bounds.origin;
                let mut path = PathBuilder::stroke(px(1.5));
                let mut x = vis.0.max(0.);
                let mut first = true;
                while x <= vis.1 {
                    let tm = c.start + (x / pps as f32) as f64;
                    let y = line_y(c.volume_at(tm), h);
                    let p = point(o.x + px(x + off), o.y + px(y));
                    if first {
                        path.move_to(p);
                        first = false;
                    } else {
                        path.line_to(p);
                    }
                    x += 3.;
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .absolute()
        .inset_0()
        .into_any_element(),
    );
    if !interactive {
        return out;
    }
    let id = clip.id;
    if keys.is_empty() {
        // Drag the line up or down; double-click it for a first keyframe.
        let y = line_y(clip.volume, h);
        let c2 = clip.clone();
        out.push(
            div()
                .id(ElementId::NamedChild(std::sync::Arc::new(ElementId::Uuid(id)), "volume-line".into()))
                .absolute()
                .left(px(vis.0.max(0.) + off))
                .w(px((vis.1 - vis.0.max(0.)).max(0.)))
                .top(px(y - 4.))
                .h(px(8.))
                .cursor(gpui::CursorStyle::ResizeUpDown)
                .tooltip(move |_, cx| tooltip(format!("Volume {} dB · drag · double-click for a keyframe", widgets::db_text(gain_to_db(c2.volume))).into(), cx))
                .on_mouse_down(MouseButton::Left, {
                    let c = clip.clone();
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| this.volume_down(&c, None, h, e, cx))
                })
                .into_any_element(),
        );
    } else {
        // Hit boxes along the line (double-click adds a keyframe there), then the points.
        let mut x = (vis.0.max(0.) / 16.).floor() * 16.;
        let mut i = 0;
        while x < vis.1 {
            let tm = clip.start + (x as f64 + 8.) / pps;
            let y = line_y(clip.volume_at(tm), h);
            let c = clip.clone();
            out.push(
                div()
                    .id(ElementId::NamedChild(std::sync::Arc::new(ElementId::Uuid(id)), format!("volume-hit-{i}").into()))
                    .absolute()
                    .left(px(x + off))
                    .w(px(16.))
                    .top(px(y - 4.))
                    .h(px(8.))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        if e.click_count == 2 {
                            this.volume_down(&c, None, h, e, cx);
                        }
                    }))
                    .into_any_element(),
            );
            x += 16.;
            i += 1;
        }
        for (i, (time, v)) in keys.iter().enumerate() {
            let x = (*time * pps) as f32;
            if x < vis.0 - 8. || x > vis.1 + 8. {
                continue;
            }
            let y = line_y(*v, h);
            let c = clip.clone();
            let (time, v) = (*time, *v);
            out.push(
                div()
                    .id(ElementId::NamedChild(std::sync::Arc::new(ElementId::Uuid(id)), format!("volume-key-{i}").into()))
                    .absolute()
                    .left(px(x + off - 5.))
                    .top(px(y - 5.))
                    .size(px(10.))
                    .rounded_full()
                    .bg(gpui::white())
                    .border_2()
                    .border_color(t.accent)
                    .cursor(gpui::CursorStyle::PointingHand)
                    .tooltip(move |_, cx| tooltip(format!("{} dB at {:.2} s · drag · right-click to remove", widgets::db_text(gain_to_db(v)), c.start + time).into(), cx))
                    .on_mouse_down(MouseButton::Left, {
                        let c = clip.clone();
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| this.volume_down(&c, Some(i), h, e, cx))
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let start = clip.start;
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.consumed = true;
                            this.store.update(cx, |s, cx| s.run("clip.removeKeyframe", json!({ "clipId": id, "property": "volume", "time": start + time }), cx));
                        })
                    })
                    .into_any_element(),
            );
        }
    }
    out
}

/// Fades shaded in their curve: the part of the clip a fade keeps quiet.
pub(in crate::views::timeline) fn fades(curve: FadeCurve, off: f32, wf: f32, h: f32, fade_in_w: f32, fade_out_w: f32) -> impl IntoElement {
    let shade = gpui::black().opacity(0.32);
    let edge = gpui::white().opacity(0.55);
    canvas(
        |_, _, _| (),
        move |b: Bounds<Pixels>, _, window, _| {
            let o = b.origin;
            let at = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
            let steps = 16;
            let mut draw = |from: f32, w: f32, out: bool| {
                if w <= 2. {
                    return;
                }
                let mut shape = PathBuilder::fill();
                let mut line = PathBuilder::stroke(px(1.));
                let y_of = |i: usize| {
                    let f = i as f32 / steps as f32;
                    let g = curve.gain(if out { 1. - f as f64 } else { f as f64 }) as f32;
                    (from + w * f, h * (1. - g))
                };
                shape.move_to(at(from, 0.));
                for i in 0..=steps {
                    let (x, y) = y_of(i);
                    shape.line_to(at(x, y));
                    if i == 0 { line.move_to(at(x, y)) } else { line.line_to(at(x, y)) }
                }
                shape.line_to(at(from + w, 0.));
                shape.close();
                if let Ok(p) = shape.build() {
                    window.paint_path(p, shade);
                }
                if let Ok(p) = line.build() {
                    window.paint_path(p, edge);
                }
            };
            draw(off, fade_in_w, false);
            draw(off + wf - fade_out_w, fade_out_w, true);
        },
    )
    .absolute()
    .inset_0()
}

/// The waveform, each bar scaled by the clip's level there (volume and its keyframes).
#[allow(clippy::too_many_arguments)]
pub(in crate::views::timeline) fn wave(peaks: Peaks, per_second: u32, clip: &Clip, full_w: f32, vis: (f32, f32), h: f32, color: Hsla, pps: f64) -> impl IntoElement {
    let rate = per_second.max(1) as f64;
    let c = clip.clone();
    let (from, span) = (c.in_point, (c.duration * c.speed).max(1e-6));
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window, _| {
            let mid = h / 2.;
            let mut x = (vis.0 / 2.).floor() * 2.;
            while x < vis.1 {
                let along = (x / full_w) as f64;
                let src = from + (if c.reverse { 1. - along } else { along }) * span;
                let i0 = (src * rate).floor().max(0.) as usize;
                let i1 = ((src + 2. / full_w as f64 * span) * rate).floor().max(i0 as f64 + 1.) as usize;
                let peak = peaks.get(i0..i1.min(peaks.len())).map(|s| s.iter().copied().fold(0f32, f32::max)).unwrap_or(0.);
                let gain = c.volume_at(c.start + x as f64 / pps).min(4.) as f32;
                let bar = (peak * gain).clamp(0., 1.) * (h - 2.);
                let bar = bar.max(1.);
                window.paint_quad(fill(Bounds::new(point(bounds.origin.x + px(x - vis.0), bounds.origin.y + px(mid - bar / 2.)), size(px(1.4), px(bar))), color));
                x += 2.;
            }
        },
    )
    .absolute()
    .bottom_0()
    .left_0()
    .w(px((vis.1 - vis.0).max(0.)))
    .h(px(h))
}

/// Solo and record arm, beside a track header's other toggles.
pub(in crate::views::timeline) fn header_toggles(track: &kimchi_core::Track, cx: &App) -> Vec<AnyElement> {
    let t = cx.theme().clone();
    let id = track.id;
    let mut out = vec![];
    let key = |name: &'static str, label: &'static str, on: bool, color: Hsla, tip: &'static str, field: &'static str| {
        div()
            .id(name)
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(10.))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(if on { color } else { t.text_3 })
            .bg(if on { color.opacity(0.2) } else { gpui::transparent_black() })
            .cursor_pointer()
            .hover(|d| d.bg(t.hover))
            .tooltip(move |_, cx| tooltip(tip.into(), cx))
            .child(label)
            .on_click(move |_, _, cx| crate::views::mixer::run_cmd(cx, "audio.setTrack", json!({ "trackId": id, field: !on })))
            .into_any_element()
    };
    out.push(key("solo", "S", track.mix.solo, t.accent_text, if track.mix.solo { "Unsolo" } else { "Solo" }, "solo"));
    if track.kind == TrackKind::Audio {
        out.push(key("arm", "R", track.mix.armed, t.danger, if track.mix.armed { "Disarm" } else { "Arm for a voice-over take" }, "armed"));
    }
    out
}

/// Each track's level, a thin meter on the right of its header (drawn by the timeline's shell,
/// which redraws while playing; the tracks themselves are a cached view).
pub(in crate::views::timeline) fn header_meters(p: &Project, scroll_y: f32, view_h: f32, cx: &mut App) -> AnyElement {
    let t: Theme = cx.theme().clone();
    let store = crate::store::StoreExt::store(cx);
    let (playhead, playing) = {
        let pb = store.read(cx).playback.read(cx);
        (pb.playhead, pb.playing)
    };
    let snap = live::snapshot(cx, playhead, playing);
    let rows = geom::rows(&p.tracks);
    let mut els = vec![];
    for (i, tr) in p.tracks.iter().enumerate() {
        let top = rows.tops[i] - scroll_y + 2.;
        let h = track_h(tr.kind);
        if top + h < 0. || top > view_h || !(tr.kind == TrackKind::Audio || tr.clips.iter().any(|c| kimchi_control::commands::audio::has_sound(p, c))) {
            continue;
        }
        let level = snap.as_ref().and_then(|s| s.tracks.get(&tr.id));
        let (readings, _) = live::reading(cx, &live::key_of("track", Some(tr.id)), level);
        els.push(div().absolute().top(px(top + 6.)).h(px(h - 12.)).left(px(HEADER_W - 6.)).w(px(4.)).child(widgets::meter(readings, true, &t).size_full()));
    }
    div().absolute().left_0().top_0().w(px(HEADER_W)).h_full().overflow_hidden().children(els).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn louder_sits_higher() {
        assert!(line_y(2.0, 50.) < line_y(1.0, 50.));
        assert!(line_y(1.0, 50.) < line_y(0.25, 50.));
        assert!(line_y(0.0, 50.) <= 47.0 + 1e-3);
    }
}
