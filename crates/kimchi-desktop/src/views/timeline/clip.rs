//! One clip on a lane: filmstrip or stills, waveform, text, solid colour, a motion clip or the
//! placeholder of a generation in flight; fades, keyframe marks, label, speed, reverse and effects
//! badges, trim and fade handles. Only the part of the clip near the viewport is drawn.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Context, ElementId, MouseButton, ObjectFit, Stateful, StyledImage, Div, PathBuilder, canvas, div, img, linear_color_stop,
    linear_gradient, point, prelude::*, px,
};
use kimchi_core::{Asset, Clip, ClipContent, MediaKind};
use kimchi_gen::Job;

use super::body::TimelineBody;
use super::waveform::Peaks;
use crate::theme::{ActiveTheme, MONO, parse_color};
use crate::ui::icon;

const RADIUS: f32 = 0.;
/// Pixels drawn past each side of the viewport.
const OVERDRAW: f64 = 64.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Image,
    Audio,
    Text,
    Solid,
    Pending,
    Motion,
}

pub fn kind(clip: &Clip, asset: Option<&Asset>) -> Kind {
    match &clip.content {
        ClipContent::Media { .. } => match asset.map(|a| a.kind) {
            Some(MediaKind::Image) => Kind::Image,
            Some(MediaKind::Audio) => Kind::Audio,
            _ => Kind::Video,
        },
        ClipContent::Text { .. } => Kind::Text,
        ClipContent::Solid { .. } => Kind::Solid,
        ClipContent::Pending { .. } => Kind::Pending,
        ClipContent::Motion { .. } => Kind::Motion,
    }
}

/// Clip-local times (seconds from the clip's start) of the clip's own keyframes and, for a
/// motion clip, of its scene's keyframes that fall inside it.
pub fn key_times(clip: &Clip) -> (Vec<f64>, Vec<f64>) {
    let mut own: Vec<f64> = clip.keyframes.values().flatten().map(|k| k.time).filter(|t| *t >= -1e-6 && *t <= clip.duration + 1e-6).collect();
    own.sort_by(f64::total_cmp);
    own.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    let mut scene = vec![];
    if let ClipContent::Motion { scene: sc, .. } = &clip.content {
        let mut see = |k: &kimchi_core::Keyframes| {
            for key in k.values().flatten() {
                let local = (key.time - clip.in_point) / clip.speed.max(1e-3);
                if (-1e-6..=clip.duration + 1e-6).contains(&local) {
                    scene.push(local);
                }
            }
        };
        match sc {
            kimchi_core::Scene::Flat(f) => {
                see(&f.keyframes);
                kimchi_core::motion::walk_layers(&f.layers, &mut |l| see(&l.keyframes));
            }
            kimchi_core::Scene::Space(f) => {
                see(&f.keyframes);
                see(&f.camera.keyframes);
                for l in &f.lights {
                    see(&l.keyframes);
                }
                kimchi_core::motion::walk_objects(&f.objects, &mut |o| see(&o.keyframes));
            }
        }
        scene.sort_by(f64::total_cmp);
        scene.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    }
    (own, scene)
}

/// Everything a clip needs to draw itself.
pub struct ClipView<'a> {
    /// The clip as shown (a drag may have moved or trimmed it).
    pub clip: &'a Clip,
    pub asset: Option<&'a Asset>,
    pub job: Option<&'a Job>,
    pub pps: f64,
    pub scroll_x: f64,
    pub view_w: f64,
    /// Top in the lanes area, and height.
    pub top: f32,
    pub h: f32,
    pub selected: bool,
    pub moving: bool,
    /// The copy an ⌥-drag would drop (drawn over the original, not interactive).
    pub ghost: bool,
    /// The track is muted or hidden.
    pub muted: bool,
    pub locked: bool,
    /// Made or changed by the agent (or MCP / CLI) a moment ago.
    pub agent: bool,
    pub peaks: Option<Peaks>,
    /// Motion clips: drawn live, rendered ahead, out of date, or rendering.
    pub render: Option<crate::views::studio::render_state::RenderState>,
}

impl ClipView<'_> {
    /// `None` when the clip is out of view.
    pub fn render(self, cx: &mut Context<TimelineBody>) -> Option<Stateful<Div>> {
        let t = cx.theme().clone();
        let c = self.clip;
        let x = c.start * self.pps;
        let w = (c.duration * self.pps).max(2.);
        let x0 = x.max(self.scroll_x - OVERDRAW);
        let x1 = (x + w).min(self.scroll_x + self.view_w + OVERDRAW);
        if x1 <= x0 {
            return None;
        }
        // Clip-local pixels visible, and where the clip's own x = 0 sits in the drawn box.
        let (vis0, vis1) = ((x0 - x) as f32, (x1 - x) as f32);
        let off = -vis0;
        let (wf, h) = (w as f32, self.h);
        let (cut_l, cut_r) = (x0 > x, x1 < x + w);
        let k = kind(c, self.asset);
        let generated = self.asset.is_some_and(Asset::is_generated);
        let base = match k {
            Kind::Video | Kind::Image => t.clip_video,
            Kind::Audio => t.clip_audio,
            Kind::Text => t.clip_text,
            Kind::Solid => match &c.content {
                ClipContent::Solid { color } => parse_color(color),
                _ => t.clip_video,
            },
            Kind::Pending => t.clip_generated,
            Kind::Motion => t.clip_motion,
        };
        let id = c.id;
        let group: gpui::SharedString = format!("clip-{id}").into();

        let el_id = if self.ghost { ElementId::NamedChild(std::sync::Arc::new(ElementId::Uuid(id)), "copy".into()) } else { ElementId::Uuid(id) };
        let mut el = div()
            .id(el_id)
            .role(gpui::Role::Button)
            .aria_label(format!("{} clip, {:.1} s", c.name, c.duration))
            .group(group.clone())
            .absolute()
            .left(px((x0 - self.scroll_x) as f32))
            .top(px(self.top))
            .w(px(vis1 - vis0))
            .h(px(h))
            .overflow_hidden()
            .bg(base)
            .when(!cut_l, |d| d.rounded_l(px(RADIUS)))
            .when(!cut_r, |d| d.rounded_r(px(RADIUS)))
            .when(self.muted, |d| d.opacity(0.45))
            .when(self.moving || self.ghost, |d| d.opacity(0.85).shadow(t.glass_shadow()))
            .cursor(if self.locked { gpui::CursorStyle::Arrow } else { gpui::CursorStyle::PointingHand });

        // Pictures.
        if k == Kind::Video
            && let Some(strip) = self.asset.and_then(|a| a.filmstrip.as_ref())
        {
            let tile_w = h * strip.frame_width as f32 / strip.frame_height.max(1) as f32;
            let span = c.duration * c.speed;
            let first = (vis0 / tile_w).floor().max(0.) as u32;
            let last = (vis1 / tile_w).ceil() as u32;
            let strip_w = strip.frames as f32 * tile_w;
            let path = PathBuf::from(&strip.path);
            for i in first..last {
                let left = i as f32 * tile_w;
                // A reversed clip shows its source from the end.
                let along = (left / wf) as f64;
                let src = c.in_point + (if c.reverse { 1. - along } else { along }) * span;
                let idx = ((src / strip.interval.max(1e-6)).floor().max(0.) as u32).min(strip.frames.saturating_sub(1));
                el = el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(left + off))
                        .w(px(tile_w))
                        .h(px(h))
                        .overflow_hidden()
                        .opacity(0.85)
                        .child(img(path.clone()).absolute().top_0().left(px(-(idx as f32) * tile_w)).w(px(strip_w)).h(px(h)).object_fit(ObjectFit::Fill)),
                );
            }
        } else if k == Kind::Image
            && let Some(a) = self.asset
        {
            let aspect = match (a.meta.width, a.meta.height) {
                (Some(w), Some(h)) if h > 0 => w as f32 / h as f32,
                _ => 16. / 9.,
            };
            let tile_w = (h * aspect).max(8.);
            let path = PathBuf::from(a.thumbnail.as_ref().unwrap_or(&a.path));
            let first = (vis0 / tile_w).floor().max(0.) as u32;
            let last = (vis1 / tile_w).ceil() as u32;
            for i in first..last {
                el = el.child(img(path.clone()).absolute().top_0().left(px(i as f32 * tile_w + off)).w(px(tile_w)).h(px(h)).object_fit(ObjectFit::Cover).opacity(0.75));
            }
        }

        // Sound.
        if let (Some(a), Some(peaks)) = (self.asset, self.peaks.clone())
            && let Some(wave) = &a.waveform
            && wf > 8.
        {
            let wave_h = if k == Kind::Audio { h - 18. } else { (h * 0.32).round() };
            let color = if k == Kind::Audio {
                t.text.opacity(0.7)
            } else if a.filmstrip.is_some() {
                gpui::white().opacity(0.55)
            } else {
                t.text.opacity(0.45)
            };
            // Scaled by the clip's level (its volume and keyframes).
            el = el.child(super::body::wave(peaks, wave.peaks_per_second, c, wf, (vis0, vis1), wave_h, color, self.pps));
        }

        // A generation in flight: progress along the bottom and a slow shimmer.
        if k == Kind::Pending {
            let fraction = self.job.and_then(|j| j.progress.fraction).unwrap_or(0.).clamp(0., 1.) as f32;
            let band = (wf * 0.5).clamp(40., 160.);
            let travel = vis1 - vis0 + band;
            let shimmer = |a: f32| {
                div()
                    .absolute()
                    .top_0()
                    .h(px(h))
                    .w(px(band))
                    .flex()
                    .child(div().flex_1().h_full().bg(linear_gradient(90., linear_color_stop(gpui::transparent_white(), 0.), linear_color_stop(gpui::white().opacity(a), 1.))))
                    .child(div().flex_1().h_full().bg(linear_gradient(90., linear_color_stop(gpui::white().opacity(a), 0.), linear_color_stop(gpui::transparent_white(), 1.))))
            };
            el = el
                .child(div().absolute().inset_0().bg(t.accent_soft))
                .child(shimmer(0.08).with_animation(
                    ElementId::NamedChild(std::sync::Arc::new(ElementId::Uuid(id)), "shimmer".into()),
                    Animation::new(Duration::from_millis(1800)).repeat(),
                    move |d, delta| d.left(px(delta * travel - band)),
                ))
                .child(div().absolute().left(px(off)).bottom_0().h(px(3.)).w(px(fraction * wf)).bg(t.accent));
        }

        // Fades: a shaded wedge over the start and the end.
        let fade_in_w = (c.fade_in / c.duration.max(1e-6)) as f32 * wf;
        let fade_out_w = (c.fade_out / c.duration.max(1e-6)) as f32 * wf;
        if fade_in_w > 2. || fade_out_w > 2. {
            // In the clip's fade curve.
            el = el.child(super::body::fades(c.audio.fade_curve, off, wf, h, fade_in_w, fade_out_w));
        }

        // Keyframes: diamonds along the bottom edge, the clip's own in the accent, a scene's dimmer.
        let (own_keys, scene_keys) = key_times(c);
        if (!own_keys.is_empty() || !scene_keys.is_empty()) && wf >= 12. {
            let pps = self.pps as f32;
            let (accent, quiet) = (t.accent, gpui::white().opacity(0.55));
            let (own_x, scene_x): (Vec<f32>, Vec<f32>) = (
                own_keys.iter().map(|k| *k as f32 * pps).collect(),
                scene_keys.iter().map(|k| *k as f32 * pps).collect(),
            );
            el = el.child(
                canvas(
                    |_, _, _| (),
                    move |b, _, window, _| {
                        let o = b.origin;
                        let y = h - 6.;
                        let mut diamond = |x: f32, r: f32, color: gpui::Hsla| {
                            if x + off < vis0 - 8. || x + off > vis1 + 8. {
                                return;
                            }
                            let cx = x + off;
                            let mut p = PathBuilder::fill();
                            p.move_to(point(o.x + px(cx), o.y + px(y - r)));
                            p.line_to(point(o.x + px(cx + r), o.y + px(y)));
                            p.line_to(point(o.x + px(cx), o.y + px(y + r)));
                            p.line_to(point(o.x + px(cx - r), o.y + px(y)));
                            p.close();
                            if let Ok(path) = p.build() {
                                window.paint_path(path, color);
                            }
                        };
                        for x in &scene_x {
                            diamond(*x, 2.5, quiet);
                        }
                        for x in &own_x {
                            diamond(*x, 3.5, accent);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );
        }

        // Sound: the volume line and its keyframes, beats, the ryolune mark.
        if let Some(p) = crate::store::StoreExt::store(&**cx).read(cx).project.clone() {
            let line = super::body::shows_line(&p, c, self.selected);
            let g = super::body::Geometry { vis: (vis0, vis1), off, wf, h, pps: self.pps };
            for d in super::body::decorations(&p, c, self.asset, &g, line, !self.locked && !self.ghost, cx) {
                el = el.child(d);
            }
        }

        // Motion clips: whether they are drawn live or play rendered frames.
        if let Some(st) = &self.render
            && wf >= 60.
        {
            use crate::views::studio::render_state::RenderState;
            let (dot, label) = match st {
                RenderState::Live => (gpui::white().opacity(0.6), "Live"),
                RenderState::Rendered(_) => (t.success, "Rendered"),
                RenderState::Outdated => (t.warning, "Out of date"),
                RenderState::Rendering(..) => (t.accent, "Rendering"),
            };
            let right = ((x + w).min(self.scroll_x + self.view_w) - x0) as f32;
            el = el.child(
                div()
                    .absolute()
                    .bottom(px(4.))
                    .left(px((right - 84.).max(4.)))
                    .w(px(80.))
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(5.))
                            .bg(gpui::black().opacity(0.55))
                            .text_size(px(9.5))
                            .text_color(gpui::white().opacity(0.9))
                            .child(div().size(px(6.)).bg(dot))
                            .child(label),
                    ),
            );
            if let RenderState::Rendering(p, _) = st {
                el = el.child(div().absolute().left(px(off)).bottom_0().h(px(3.)).w(px(*p as f32 * wf)).bg(t.accent));
            }
        }

        // Generated media wears the accent along its top.
        if generated || k == Kind::Pending {
            el = el.child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(t.accent));
        }

        // Label, kept in view when the clip starts off to the left.
        if wf >= 36. {
            let label = match &c.content {
                ClipContent::Text { style } => style.content.lines().next().unwrap_or("").to_string(),
                ClipContent::Pending { prompt, .. } => prompt.clone(),
                _ => c.name.clone(),
            };
            let status = (k == Kind::Pending).then(|| self.job.and_then(|j| j.progress.message.clone()).unwrap_or_else(|| "Queued".into()));
            let kind_icon = match k {
                Kind::Text => Some("type"),
                Kind::Solid => Some("square"),
                Kind::Audio => Some("audio-lines"),
                Kind::Motion => Some(if matches!(&c.content, ClipContent::Motion { scene, .. } if scene.is_3d()) { "box" } else { "shapes" }),
                _ => None,
            };
            let fg = gpui::white();
            // The pill is dark in both themes: white reads on it.
            let hot = gpui::white();
            let seen = ((self.scroll_x - x).max(0.) as f32).min(wf);
            el = el.child(
                div()
                    .absolute()
                    .top(px(4.))
                    .left(px(seen + 5. + off))
                    .max_w(px((wf - seen - 10.).max(0.)))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(6.))
                    .py(px(1.))
                    .bg(gpui::black().opacity(0.55))
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(fg)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(self.agent, |d| d.child(icon("bot").size(px(11.)).text_color(hot)))
                    .when(generated || k == Kind::Pending, |d| d.child(icon("sparkles").size(px(11.)).text_color(hot)))
                    .when_some(kind_icon, |d, i| d.child(icon(i).size(px(11.)).text_color(fg.opacity(0.8))))
                    .child(div().min_w_0().truncate().child(label))
                    .when_some(status, |d, s| d.child(div().flex_none().font_weight(gpui::FontWeight::MEDIUM).text_color(fg.opacity(0.7)).child(s)))
                    .when(!c.effects.is_default(), |d| d.child(icon("palette").size(px(11.)).text_color(fg.opacity(0.8))))
                    .when(c.reverse, |d| d.child(div().flex_none().font_family(MONO).text_size(px(10.)).text_color(hot).child("◀")))
                    .when((c.speed - 1.).abs() > 1e-6, |d| {
                        let s = format!("{:.2}", c.speed);
                        let s = s.trim_end_matches('0').trim_end_matches('.');
                        d.child(div().flex_none().font_family(MONO).text_size(px(10.)).text_color(hot).child(format!("{s}×")))
                    }),
            );
        }

        // Outline: the accent when selected or moving, a quiet ring otherwise; the agent's work in the accent ring.
        let (ring, ring_w) = if self.selected || self.moving || self.ghost {
            (t.accent, 2.)
        } else if self.agent {
            (t.accent_ring, 1.5)
        } else {
            (if t.is_dark() { gpui::white().opacity(0.10) } else { gpui::black().opacity(0.12) }, 1.)
        };
        el = el.child(
            div()
                .absolute()
                .inset_0()
                .when(!cut_l, |d| d.rounded_l(px(RADIUS)))
                .when(!cut_r, |d| d.rounded_r(px(RADIUS)))
                .border(px(ring_w))
                .when(cut_l, |d| d.border_l_0())
                .when(cut_r, |d| d.border_r_0())
                .border_color(ring),
        );

        if !self.locked && !self.ghost {
            el = self.handles(el, (vis0, vis1, off, wf, h), (cut_l, cut_r), (fade_in_w, fade_out_w), group, &t, cx);
        }
        Some(el)
    }

    /// Trim edges and fade handles.
    #[allow(clippy::too_many_arguments)]
    fn handles(
        &self,
        mut el: Stateful<Div>,
        (vis0, vis1, off, wf, h): (f32, f32, f32, f32, f32),
        (cut_l, cut_r): (bool, bool),
        (fade_in_w, fade_out_w): (f32, f32),
        group: gpui::SharedString,
        t: &crate::theme::Theme,
        cx: &mut Context<TimelineBody>,
    ) -> Stateful<Div> {
        let id = self.clip.id;
        let shown = self.selected;
        let bar = |left: bool| {
            div()
                .absolute()
                .top(px(h * 0.3))
                .h(px(h * 0.4))
                .w(px(2.))
                .bg(t.text)
                .when(left, |d| d.left(px(2.)))
                .when(!left, |d| d.right(px(2.)))
                .opacity(if shown { 0.8 } else { 0. })
                .group_hover(group.clone(), |s| s.opacity(0.8))
        };
        if !cut_l {
            el = el.child(
                div()
                    .id("trim-start")
                    .absolute()
                    .left(px(off))
                    .top_0()
                    .h(px(h))
                    .w(px(7.))
                    .cursor_ew_resize()
                    .child(bar(true))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e, _, cx| this.edge_down(id, true, e, cx))),
            );
        }
        if !cut_r {
            el = el.child(
                div()
                    .id("trim-end")
                    .absolute()
                    .left(px(off + wf - 7.))
                    .top_0()
                    .h(px(h))
                    .w(px(7.))
                    .cursor_ew_resize()
                    .child(bar(false))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e, _, cx| this.edge_down(id, false, e, cx))),
            );
        }
        // Fade handles sit on the top edge where each fade ends; shown on hover.
        if wf >= 40. {
            let knob = |name: &'static str, x: f32, out: bool, tip: &'static str| {
                div()
                    .id(name)
                    .absolute()
                    .top(px(1.))
                    .left(px(x - 5.))
                    .size(px(10.))
                    .bg(gpui::white())
                    .border_1()
                    .border_color(t.accent)
                    .cursor_ew_resize()
                    .opacity(if shown { 0.9 } else { 0. })
                    .group_hover(group.clone(), |s| s.opacity(0.9))
                    .tooltip(move |_, cx| crate::ui::tooltip(tip.into(), cx))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e, _, cx| this.fade_down(id, out, e, cx)))
            };
            let xi = off + fade_in_w.max(9.);
            let xo = off + wf - fade_out_w.max(9.);
            if xi - off >= vis0 && xi - off <= vis1 {
                el = el.child(knob("fade-in", xi, false, "Fade in"));
            }
            if xo - off >= vis0 && xo - off <= vis1 {
                el = el.child(knob("fade-out", xo, true, "Fade out"));
            }
        }
        el
    }
}
