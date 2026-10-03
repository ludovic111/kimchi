//! Draws a 2D motion scene ([`Scene2d`]) at one instant with tiny-skia, like an After Effects
//! composition.
//!
//! Each layer goes through After Effects' order: its content (a shape through its shape
//! operators and repeaters, text with its animators, a picture, a group's layers, a
//! composition's layers at the composition's time, particles), then its masks, then its effects,
//! then its track matte, then it is blended onto what is below with its opacity. Transforms
//! follow parents (position, rotation, scale, skew and anchor; not opacity). Adjustment layers
//! apply their effects to everything drawn below them in their list. Layers with motion blur are
//! drawn at several moments of the shutter and averaged (final quality only).
//!
//! A layer is drawn straight onto the target when nothing applies to it as a whole; otherwise it
//! is drawn on a picture of its own first, with a margin around the target so blurs, shadows
//! and glows reaching in from just outside aren't cut off.
//!
//! Nothing here is remembered between frames that could change what a frame shows, so any
//! moment can be drawn on its own (scrubbing, exports in any order).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::anim::{Easing, value_at};
use kimchi_core::motion::particles::ParticleSystem;
use kimchi_core::motion::{EvalOptions, Layer, LayerKind, Matte, Scene2d, TextLayer, walk_layers};
use kimchi_core::{TextAlign, TextStyle};
use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, LineJoin, Mask, MaskType, Path, Pixmap, PixmapPaint, Point, Rect, Stroke, Transform,
};

use super::effects2d::{self, FxCx};
use super::paint::{self, FillSpec, color, with_alpha};
use super::textfx::{self, GlyphLook, PathWalk};
use super::{masks, particles2d, shapeops};
use crate::text::{Ink, Layout, layout_cached};

/// Pictures for image layers: the frame of `asset` (id, name or path) at scene `time`, and its
/// size in project pixels.
pub(crate) trait Pictures {
    fn picture(&mut self, asset: &str, time: f64) -> Option<(Arc<Pixmap>, f64, f64)>;
}

pub(crate) struct Flat<'a> {
    pub pictures: &'a mut dyn Pictures,
    /// Output pixels per project pixel (blur radii and shadow offsets scale with it).
    pub scale: f32,
    /// Motion blur and other slow touches only for final frames.
    pub quality: super::Quality,
    /// Seconds per frame of the project: motion blur's shutter, animated grain.
    pub frame: f64,
    /// What expressions see: the project fps and the clip's length in scene seconds.
    pub eval: EvalOptions,
}

/// Compositions inside compositions, at most.
const MAX_DEPTH: u32 = 16;
/// Mattes, masks and echoes drawn for other layers, inside each other, at most.
const MAX_NEST: u32 = 12;
/// Widest margin (output pixels) a layer's own picture gets around the target.
const MAX_MARGIN: u32 = 256;
/// Properties that move a layer: particles' trails only need re-evaluating when one changes.
const MOVING: &[&str] = &["x", "y", "position", "anchorX", "anchorY", "scale", "scaleX", "scaleY", "rotation", "skewX"];

/// Draws `scene` at scene time `t` onto `canvas`; `base` maps project pixels from the canvas
/// centre to the canvas.
pub(crate) fn draw(canvas: &mut Pixmap, scene: &Scene2d, t: f64, base: Transform, fx: &mut Flat) {
    let bg = match scene.keyframes.get("background").and_then(|k| value_at(k, t)) {
        Some(v) => v.as_str().map(str::to_string),
        None => scene.background.clone(),
    };
    if let Some(bg) = bg {
        canvas.fill(color(&bg));
    }
    let axis = |a: f32, b: f32| (a * a + b * b).sqrt().max(1e-6) as f64;
    let project = (canvas.width() as f64 / axis(base.sx, base.ky), canvas.height() as f64 / axis(base.kx, base.sy));
    remember_canvas(project, fx.eval);
    let layers = scene.layers_at_with(t, &fx.eval);
    let hidden = hidden_ids(&layers);
    let cx = Cx { scene, comp: None, home: &layers, hidden: &hidden, t, rate: 1.0, depth: 0, nest: 0, canvas: project, project, k: fx.scale.max(1e-4), opts: fx.eval };
    draw_list(canvas, &layers, Place { to: base, home: base }, &cx, fx);
}

/// What a list of layers is drawn in.
#[derive(Clone, Copy)]
struct Cx<'a> {
    scene: &'a Scene2d,
    /// The composition being drawn, or `None` for the scene's own layers.
    comp: Option<&'a str>,
    /// Its layers at `t`, groups included (mattes and masks are found here by id).
    home: &'a [Layer],
    /// Ids of the layers used as mattes or masks there: not drawn themselves.
    hidden: &'a HashSet<String>,
    /// Seconds of the scene or composition.
    t: f64,
    /// Seconds of this list per scene second (compositions' speed), for motion blur.
    rate: f64,
    depth: u32,
    nest: u32,
    /// The canvas the list is placed on (scene or composition), project pixels.
    canvas: (f64, f64),
    /// The scene's canvas, project pixels (compositions default to it).
    project: (f64, f64),
    /// Target pixels per project pixel of this canvas.
    k: f32,
    /// For evaluating layers at other moments (expressions need the fps and length).
    opts: EvalOptions,
}

/// Where a list lands on the target: `to` maps the list's space (where its layers' x and y are),
/// `home` the scene's or composition's space (project pixels from its canvas centre).
#[derive(Clone, Copy)]
struct Place {
    to: Transform,
    home: Transform,
}

impl Place {
    fn shifted(self, d: f32) -> Place {
        Place { to: self.to.post_translate(d, d), home: self.home.post_translate(d, d) }
    }
}

fn hidden_ids(list: &[Layer]) -> HashSet<String> {
    let mut out = HashSet::new();
    walk_layers(list, &mut |l| {
        if let Some(m) = &l.mask {
            out.insert(m.clone());
        }
        if let Some(m) = &l.matte {
            out.insert(m.layer.clone());
        }
    });
    out
}

/// The scene's or a composition's layers at another moment.
fn list_at(cx: &Cx, time: f64) -> Option<Vec<Layer>> {
    match cx.comp {
        None => Some(cx.scene.layers_at_with(time, &cx.opts)),
        Some(c) => cx.scene.comp_layers_at_with(c, time, &cx.opts),
    }
}

/// The layer's own transform: its pixels (from its centre) → its list's space.
fn own_matrix(l: &Layer) -> Transform {
    let skew = (l.skew_x.clamp(-89.0, 89.0) as f32).to_radians().tan();
    let ts = Transform::from_translate(l.x as f32, l.y as f32)
        .pre_rotate(l.rotation as f32)
        .pre_concat(Transform::from_row(1.0, 0.0, skew, 1.0, 0.0, 0.0))
        .pre_scale((l.scale * l.scale_x) as f32, (l.scale * l.scale_y) as f32)
        .pre_translate(-l.anchor_x as f32, -l.anchor_y as f32);
    if ts.is_finite() { ts } else { Transform::from_scale(0.0, 0.0) }
}

/// The layer's transform with its parents' (siblings in `list`), like After Effects' parenting.
fn matrix(list: &[Layer], l: &Layer) -> Transform {
    let mut m = own_matrix(l);
    let mut at = l;
    for _ in 0..64 {
        let Some(p) = at.parent.as_deref() else { break };
        let Some(parent) = list.iter().find(|x| x.id == p) else { break };
        m = own_matrix(parent).pre_concat(m);
        at = parent;
    }
    m
}

/// Finds `id` in `list` or its groups: the transform from `list`'s space to the space of the
/// list it is in, that list, and the layer.
fn locate<'l>(list: &'l [Layer], id: &str) -> Option<(Transform, &'l [Layer], &'l Layer)> {
    if let Some(l) = list.iter().find(|l| l.id == id) {
        return Some((Transform::identity(), list, l));
    }
    list.iter().find_map(|g| match &g.kind {
        LayerKind::Group { layers } => locate(layers, id).map(|(ts, sib, l)| (matrix(list, g).pre_concat(ts), sib, l)),
        _ => None,
    })
}

fn device_scale(ts: Transform) -> f32 {
    (ts.sx * ts.sy - ts.kx * ts.ky).abs().sqrt()
}

fn in_time(l: &Layer, t: f64) -> bool {
    t + 1e-9 >= l.start && l.end.is_none_or(|e| t < e - 1e-9)
}

/// Is the layer drawn now?
fn shows(l: &Layer, cx: &Cx) -> bool {
    l.visible_at(cx.t) && !cx.hidden.contains(&l.id) && l.opacity > 0.0 && !matches!(l.kind, LayerKind::Null {})
}

fn draw_list(target: &mut Pixmap, list: &[Layer], place: Place, cx: &Cx, fx: &mut Flat) {
    draw_list_faded(target, list, place, cx, fx, 1.0);
}

/// Draws a list bottom to top; `alpha` fades every layer (a group drawn straight on the target).
fn draw_list_faded(target: &mut Pixmap, list: &[Layer], place: Place, cx: &Cx, fx: &mut Flat, alpha: f32) {
    for l in list {
        if !shows(l, cx) {
            continue;
        }
        if matches!(l.kind, LayerKind::Adjustment {}) {
            adjust(target, l, list, place, cx, fx, alpha);
        } else if alpha < 1.0 {
            let mut faded = l.clone();
            faded.opacity *= alpha as f64;
            draw_layer(target, &faded, list, place, cx, fx);
        } else {
            draw_layer(target, l, list, place, cx, fx);
        }
    }
}

fn motion_blurred(l: &Layer, cx: &Cx, fx: &Flat) -> bool {
    l.motion_blur
        && fx.quality == super::Quality::Final
        && cx.scene.shutter > 0.0
        && cx.scene.motion_blur_samples >= 2.0
        && fx.frame > 0.0
        && fx.frame.is_finite()
}

/// Does the layer need a picture of its own (something applies to it as a whole)?
fn needs_own(l: &Layer, cx: &Cx, fx: &Flat) -> bool {
    let repeats = l.operators.iter().any(|o| o.enabled && o.kind == "repeater");
    let several = matches!(l.kind, LayerKind::Group { .. } | LayerKind::Particles(_)) || (l.fill.is_some() && l.stroke.is_some()) || repeats;
    let adjusts = matches!(&l.kind, LayerKind::Group { layers } if layers.iter().any(|c| matches!(c.kind, LayerKind::Adjustment {})));
    l.blend != Default::default()
        || l.mask.is_some()
        || l.matte.is_some()
        || l.blur > 0.0
        || l.shadow.is_some()
        || l.glow.is_some()
        || l.masks.iter().any(|m| m.enabled && m.s("mode") != "none")
        || l.effects.iter().any(|e| e.enabled)
        || adjusts
        || motion_blurred(l, cx, fx)
        || (several && l.opacity < 1.0)
}

fn draw_layer(target: &mut Pixmap, l: &Layer, list: &[Layer], place: Place, cx: &Cx, fx: &mut Flat) {
    let opacity = l.opacity.clamp(0.0, 1.0) as f32;
    if !needs_own(l, cx, fx) {
        let ts = place.to.pre_concat(matrix(list, l));
        content(target, l, list, ts, opacity, place, cx, fx);
        return;
    }
    let pad = margin(l, cx.k);
    let (w, h) = (target.width() + 2 * pad, target.height() + 2 * pad);
    let inner = place.shifted(pad as f32);
    let pic = if motion_blurred(l, cx, fx) { motion_blur(l, list, inner, cx, fx, w, h) } else { picture(l, list, inner, cx, fx, w, h, usize::MAX) };
    if let Some(pic) = pic {
        composite(target, &pic, l, opacity, pad, cx.k);
    }
}

/// How far (target pixels) the layer's blurs, shadows and glows reach past its ink.
fn margin(l: &Layer, k: f32) -> u32 {
    let mut r = 0.0f32;
    for e in &l.effects {
        r += effects2d::reach(e, k).unwrap_or(0.0);
    }
    r += l.blur as f32 * k * 1.5;
    if let Some(s) = &l.shadow {
        r += (s.x.abs().max(s.y.abs()) + s.blur * 1.5) as f32 * k;
    }
    if let Some(g) = &l.glow {
        r += g.radius as f32 * k * 1.5;
    }
    if r.is_finite() { (r.ceil().max(0.0) as u32).min(MAX_MARGIN) } else { MAX_MARGIN }
}

/// The layer's finished picture (content → masks → effects → matte), without its opacity and
/// blend, on a `w`×`h` picture placed by `place`. Only the first `upto` effects apply (echoes
/// redraw the layer through the effects before them).
#[allow(clippy::too_many_arguments)]
fn picture(l: &Layer, list: &[Layer], place: Place, cx: &Cx, fx: &mut Flat, w: u32, h: u32, upto: usize) -> Option<Pixmap> {
    let ts = place.to.pre_concat(matrix(list, l));
    let mut own = Pixmap::new(w, h)?;
    content(&mut own, l, list, ts, 1.0, place, cx, fx);
    masks::apply(&mut own, &l.masks, ts);
    if let Some(id) = &l.mask {
        legacy_mask(&mut own, id, l.mask_invert, place, cx, fx);
    }
    if l.blur > 0.0 {
        paint::blur(&mut own, l.blur as f32 * cx.k);
    }
    let frame = fx.frame;
    for (i, e) in l.effects.iter().enumerate().take(upto) {
        if !e.enabled {
            continue;
        }
        let corners = local_bounds(l, cx.scene, cx.project, cx.canvas, cx.t, true).map(|r| corners_of(r, ts));
        let id = l.id.clone();
        let mut redraw = |time: f64| echo_at(&id, time, i, place.home, cx, &mut *fx, w, h);
        let mut fcx = FxCx { k: cx.k, base: place.home, canvas: cx.canvas, t: cx.t, frame, corners, echo: Some(&mut redraw) };
        effects2d::apply(&mut own, e, &mut fcx);
    }
    if let Some(m) = &l.matte {
        let values = matte_values(m, place, cx, fx, w, h);
        masks::multiply(&mut own, &values);
    }
    Some(own)
}

/// The layer `id` at another moment, through its first `upto` effects (for echoes).
#[allow(clippy::too_many_arguments)]
fn echo_at(id: &str, time: f64, upto: usize, home: Transform, cx: &Cx, fx: &mut Flat, w: u32, h: u32) -> Option<Pixmap> {
    if cx.nest >= MAX_NEST || !time.is_finite() {
        return None;
    }
    let list = list_at(cx, time)?;
    let (chain, sib, l) = locate(&list, id)?;
    if !l.visible_at(time) {
        return None;
    }
    let at = Cx { home: &list, t: time, nest: cx.nest + 1, ..*cx };
    picture(l, sib, Place { to: home.pre_concat(chain), home }, &at, fx, w, h, upto)
}

/// The layer drawn at several moments of the shutter (centred on now) and averaged.
fn motion_blur(l: &Layer, list: &[Layer], place: Place, cx: &Cx, fx: &mut Flat, w: u32, h: u32) -> Option<Pixmap> {
    let n = cx.scene.motion_blur_samples.round().clamp(2.0, 64.0) as usize;
    let span = cx.scene.shutter.clamp(0.0, 2.0) * fx.frame * cx.rate;
    let mut acc = vec![0u32; w as usize * h as usize * 4];
    for i in 0..n {
        let dt = (i as f64 / (n - 1) as f64 - 0.5) * span;
        let pic = if dt.abs() < 1e-12 {
            picture(l, list, place, cx, fx, w, h, usize::MAX)
        } else {
            let time = cx.t + dt;
            list_at(cx, time).and_then(|later| {
                let (chain, sib, at) = locate(&later, &l.id)?;
                if !at.visible_at(time) {
                    return None;
                }
                let moment = Cx { home: &later, t: time, ..*cx };
                picture(at, sib, Place { to: place.home.pre_concat(chain), home: place.home }, &moment, fx, w, h, usize::MAX)
            })
        };
        if let Some(p) = pic {
            for (a, v) in acc.iter_mut().zip(p.data()) {
                *a += *v as u32;
            }
        }
    }
    let mut out = Pixmap::new(w, h)?;
    let half = n as u32 / 2;
    for (o, a) in out.data_mut().iter_mut().zip(&acc) {
        *o = ((*a + half) / n as u32).min(255) as u8;
    }
    // Rounding can leave colour a hair above alpha; keep it premultiplied.
    for px in out.data_mut().as_chunks_mut::<4>().0.iter_mut() {
        let a = px[3];
        px[0] = px[0].min(a);
        px[1] = px[1].min(a);
        px[2] = px[2].min(a);
    }
    Some(out)
}

/// Puts a layer's picture (drawn with a `pad` margin) onto the target: its old-style shadow and
/// glow under it, then the picture with its opacity and blend mode.
fn composite(target: &mut Pixmap, pic: &Pixmap, l: &Layer, opacity: f32, pad: u32, k: f32) {
    // Only the part with ink, and what its shadow and glow reach from it, is worth tinting,
    // blurring and blending: a small layer's picture is mostly empty (dozens of glowing dots
    // used to cost a whole canvas each).
    let Some(ink) = effects2d::ink(pic) else { return };
    let mut reach = 2.0f32;
    if let Some(s) = &l.shadow {
        reach = reach.max((s.x.abs().max(s.y.abs()) + s.blur * 3.0) as f32 * k + 2.0);
    }
    if let Some(g) = &l.glow {
        reach = reach.max(g.radius as f32 * k * 3.0 + 2.0);
    }
    let roi = ink.grow(reach, pic);
    let cropped = (roi.w() * roi.h() * 2 < pic.width() as usize * pic.height() as usize)
        .then(|| tiny_skia::IntRect::from_xywh(roi.x0 as i32, roi.y0 as i32, roi.w() as u32, roi.h() as u32).and_then(|r| pic.clone_rect(r)))
        .flatten();
    let (pic, ox, oy) = match &cropped {
        Some(c) => (c, roi.x0 as f32, roi.y0 as f32),
        None => (pic, 0.0, 0.0),
    };
    let (back_x, back_y) = (ox - pad as f32, oy - pad as f32);
    composite_at(target, pic, l, opacity, (back_x, back_y), k);
}

/// [`composite`] of a picture whose top left corner goes at `back` on the target.
fn composite_at(target: &mut Pixmap, pic: &Pixmap, l: &Layer, opacity: f32, (back_x, back_y): (f32, f32), k: f32) {
    if let Some(s) = &l.shadow {
        let mut sh = paint::tinted(pic, with_alpha(color(&s.color), opacity), 1.0);
        paint::blur(&mut sh, s.blur as f32 * k);
        let at = Transform::from_translate(back_x + s.x as f32 * k, back_y + s.y as f32 * k);
        target.draw_pixmap(0, 0, sh.as_ref(), &PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() }, at, None);
    }
    if let Some(g) = &l.glow {
        let mut gl = paint::tinted(pic, color(&g.color), (g.strength * l.opacity) as f32);
        paint::blur(&mut gl, g.radius as f32 * k);
        let paint = PixmapPaint { blend_mode: BlendMode::Plus, ..PixmapPaint::default() };
        target.draw_pixmap(0, 0, gl.as_ref(), &paint, Transform::from_translate(back_x, back_y), None);
    }
    let paint = PixmapPaint { opacity, blend_mode: paint::blend(l.blend), quality: tiny_skia::FilterQuality::Nearest };
    target.draw_pixmap(0, 0, pic.as_ref(), &paint, Transform::from_translate(back_x, back_y), None);
}

/// How much of each pixel a track matte lets through (0–1).
fn matte_values(m: &Matte, place: Place, cx: &Cx, fx: &mut Flat, w: u32, h: u32) -> Vec<f32> {
    let n = w as usize * h as usize;
    let pic = locate(cx.home, &m.layer).filter(|_| cx.nest < MAX_NEST).and_then(|(chain, sib, ml)| {
        // The matte layer counts even when hidden (it usually is), but not outside its time.
        if !in_time(ml, cx.t) {
            return None;
        }
        let at = Cx { nest: cx.nest + 1, ..*cx };
        let mut p = picture(ml, sib, Place { to: place.home.pre_concat(chain), home: place.home }, &at, fx, w, h, usize::MAX)?;
        let o = ml.opacity.clamp(0.0, 1.0) as f32;
        if o < 1.0 {
            masks::multiply(&mut p, &vec![o; n]);
        }
        Some(p)
    });
    let inverted = m.mode.ends_with("Inverted");
    let luma = m.mode.starts_with("luma");
    let Some(p) = pic else { return vec![if inverted { 1.0 } else { 0.0 }; n] };
    p.data()
        .as_chunks::<4>().0.iter()
        .map(|px| {
            let v = if luma {
                (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0
            } else {
                px[3] as f32 / 255.0
            };
            if inverted { 1.0 - v } else { v }
        })
        .collect()
}

/// The old `mask` field: the layer shows only where a sibling's shape is (that shape counts as
/// filled when it has no paint of its own).
fn legacy_mask(own: &mut Pixmap, id: &str, invert: bool, place: Place, cx: &Cx, fx: &mut Flat) {
    if cx.nest >= MAX_NEST {
        return;
    }
    let Some(mut shape) = Pixmap::new(own.width(), own.height()) else { return };
    if let Some((chain, sib, m)) = locate(cx.home, id) {
        let mut m = m.clone();
        m.mask = None;
        if m.fill.is_none() && m.stroke.is_none() {
            m.fill = Some(kimchi_core::motion::Fill::Color("#ffffff".into()));
        }
        let free = HashSet::new();
        let mcx = Cx { hidden: &free, nest: cx.nest + 1, ..*cx };
        if shows(&m, &mcx) {
            draw_layer(&mut shape, &m, sib, Place { to: place.home.pre_concat(chain), home: place.home }, &mcx, fx);
        }
    }
    let mut mask = Mask::from_pixmap(shape.as_ref(), MaskType::Alpha);
    if invert {
        mask.invert();
    }
    own.apply_mask(&mask);
}

/// An adjustment layer: its effects on everything below it in its list, within its masks (and
/// matte), as much as its opacity.
fn adjust(target: &mut Pixmap, l: &Layer, list: &[Layer], place: Place, cx: &Cx, fx: &mut Flat, alpha: f32) {
    if !l.effects.iter().any(|e| e.enabled) {
        return;
    }
    let ts = place.to.pre_concat(matrix(list, l));
    let mut done = target.clone();
    for e in l.effects.iter().filter(|e| e.enabled) {
        let mut fcx = FxCx { k: cx.k, base: place.home, canvas: cx.canvas, t: cx.t, frame: fx.frame, corners: None, echo: None };
        effects2d::apply(&mut done, e, &mut fcx);
    }
    let (w, h) = (target.width(), target.height());
    let mut weight = masks::coverage(&l.masks, ts, w, h);
    if let Some(m) = &l.matte {
        let values = matte_values(m, place, cx, fx, w, h);
        weight = Some(match weight {
            Some(c) => c.iter().zip(&values).map(|(a, b)| a * b).collect(),
            None => values,
        });
    }
    let opacity = l.opacity.clamp(0.0, 1.0) as f32 * alpha;
    let data = target.data_mut();
    for (i, (o, d)) in data.as_chunks_mut::<4>().0.iter_mut().zip(done.data().as_chunks::<4>().0.iter()).enumerate() {
        let k = opacity * weight.as_ref().map_or(1.0, |c| c[i].clamp(0.0, 1.0));
        if k <= 0.0 {
            continue;
        }
        for c in 0..4 {
            o[c] = (o[c] as f32 + (d[c] as f32 - o[c] as f32) * k).round() as u8;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Content

/// What the layer itself draws (children for groups), at `alpha`, with its repeaters' copies.
#[allow(clippy::too_many_arguments)]
fn content(target: &mut Pixmap, l: &Layer, list: &[Layer], ts: Transform, alpha: f32, place: Place, cx: &Cx, fx: &mut Flat) {
    if let LayerKind::Particles(ps) = &l.kind {
        particles(target, l, ps, list, place, alpha, cx, fx);
        return;
    }
    match shapeops::copies(&l.operators) {
        None => content_once(target, l, ts, alpha, place, cx, fx),
        Some(copies) => {
            for (c, a) in copies {
                content_once(target, l, ts.pre_concat(c), alpha * a, place, cx, fx);
            }
        }
    }
}

fn content_once(target: &mut Pixmap, l: &Layer, ts: Transform, alpha: f32, place: Place, cx: &Cx, fx: &mut Flat) {
    if alpha <= 0.0 {
        return;
    }
    let outline = match &l.kind {
        LayerKind::Null {} | LayerKind::Adjustment {} | LayerKind::Particles(_) => return,
        LayerKind::Text(t) => {
            text(target, l, t, ts, alpha, cx.t);
            return;
        }
        LayerKind::Image { asset, width, height, radius } => {
            image(target, asset, *width, *height, *radius, ts, alpha, cx.t, fx);
            return;
        }
        LayerKind::Group { layers } => {
            // The group's opacity is already in `alpha` only when drawn on its own picture.
            draw_list_faded(target, layers, Place { to: ts, home: place.home }, cx, fx, alpha);
            return;
        }
        LayerKind::Comp { comp, speed, offset, looped, time } => {
            composition(target, l, comp, *speed, *offset, *looped, *time, ts, alpha, cx, fx);
            return;
        }
        _ => shape_outline(l, cx.t),
    };
    let Some(path) = outline else { return };
    if let Some(f) = &l.fill {
        let spec = FillSpec::of(f, path.bounds());
        paint::fill(target, &path, &spec, alpha, ts, None);
    }
    if let Some(s) = &l.stroke {
        stroke(target, &path, l, s, ts, alpha);
    }
}

/// A shape layer's outline in its own pixels, through its shape operators.
fn shape_outline(l: &Layer, t: f64) -> Option<Path> {
    let path = match &l.kind {
        LayerKind::Rect { width, height, radius } => paint::rect(*width as f32, *height as f32, *radius as f32),
        LayerKind::Ellipse { width, height } => paint::ellipse(*width as f32, *height as f32),
        LayerKind::Polygon { sides, radius, roundness } => paint::polygon(*sides as f32, *radius as f32, *roundness as f32),
        LayerKind::Star { points, radius, inner_radius } => paint::star(*points as f32, *radius as f32, *inner_radius as f32),
        LayerKind::Path { d, points, closed } => paint::svg_path(d, points, *closed),
        _ => None,
    }?;
    shapeops::apply(path, &l.operators, t)
}

fn stroke(target: &mut Pixmap, path: &Path, l: &Layer, s: &kimchi_core::motion::Stroke, ts: Transform, alpha: f32) {
    if s.width <= 0.0 {
        return;
    }
    let dash = match paint::trim_dash(path, l.trim_start, l.trim_end, l.trim_offset) {
        Some(None) => return,
        Some(Some(d)) => Some(d),
        None if s.dash.len() >= 2 => {
            let mut arr: Vec<f32> = s.dash.iter().map(|v| v.max(0.0) as f32).collect();
            if arr.len() % 2 == 1 {
                arr.extend(arr.clone());
            }
            tiny_skia::StrokeDash::new(arr, s.dash_offset as f32)
        }
        None => None,
    };
    let stroke = Stroke {
        width: s.width as f32,
        line_cap: match s.cap.as_str() {
            "butt" => LineCap::Butt,
            "square" => LineCap::Square,
            _ => LineCap::Round,
        },
        line_join: match s.join.as_str() {
            "miter" => LineJoin::Miter,
            "bevel" => LineJoin::Bevel,
            _ => LineJoin::Round,
        },
        dash,
        ..Stroke::default()
    };
    let paint = FillSpec::Solid(color(&s.color)).paint(alpha);
    target.stroke_path(path, &paint, &stroke, ts, None);
}

/// Natural sizes of the pictures image layers showed, for the Studio's handles.
fn image_sizes() -> &'static Mutex<HashMap<String, (f64, f64)>> {
    static S: OnceLock<Mutex<HashMap<String, (f64, f64)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn image_box(asset: &str, width: Option<f64>, height: Option<f64>, natural: Option<(f64, f64)>) -> (f64, f64) {
    let natural = natural.or_else(|| image_sizes().lock().unwrap_or_else(|e| e.into_inner()).get(asset).copied()).unwrap_or((100.0, 100.0));
    let aspect = natural.0 / natural.1.max(1e-6);
    match (width, height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w / aspect),
        (None, Some(h)) => (h * aspect, h),
        (None, None) => natural,
    }
}

#[allow(clippy::too_many_arguments)]
fn image(target: &mut Pixmap, asset: &str, width: Option<f64>, height: Option<f64>, radius: f64, ts: Transform, alpha: f32, t: f64, fx: &mut Flat) {
    let Some((pic, nw, nh)) = fx.pictures.picture(asset, t) else { return };
    {
        let mut sizes = image_sizes().lock().unwrap_or_else(|e| e.into_inner());
        if sizes.len() > 1024 {
            sizes.clear();
        }
        sizes.insert(asset.to_string(), (nw, nh));
    }
    let (w, h) = image_box(asset, width, height, Some((nw, nh)));
    let Some(box_) = paint::rect(w as f32, h as f32, radius as f32) else { return };
    // The picture spans the box: pixmap pixels → layer pixels.
    let to_box = Transform::from_translate(-w as f32 / 2.0, -h as f32 / 2.0).pre_scale(w as f32 / pic.width() as f32, h as f32 / pic.height() as f32);
    let shader = tiny_skia::Pattern::new(pic.as_ref().as_ref(), tiny_skia::SpreadMode::Pad, tiny_skia::FilterQuality::Bilinear, alpha, to_box);
    let paint = tiny_skia::Paint { shader, anti_alias: true, ..tiny_skia::Paint::default() };
    target.fill_path(&box_, &paint, FillRule::Winding, ts, None);
}

/// A composition layer: the composition's layers at its own time, on a canvas of its size
/// centred on the layer's anchor, drawn sharp for the size it shows at.
#[allow(clippy::too_many_arguments)]
fn composition(target: &mut Pixmap, l: &Layer, id: &str, speed: f64, offset: f64, looped: bool, time: Option<f64>, ts: Transform, alpha: f32, cx: &Cx, fx: &mut Flat) {
    if cx.depth >= MAX_DEPTH {
        tracing::warn!(layer = %l.id, "compositions nested too deep; not drawn");
        return;
    }
    let Some(c) = cx.scene.composition(id) else { return };
    let mut ct = time.unwrap_or((cx.t - l.start) * speed + offset);
    if looped {
        ct = ct.rem_euclid(c.length());
    }
    if !ct.is_finite() {
        return;
    }
    let (cw, ch) = (c.width.unwrap_or(cx.project.0).max(1.0), c.height.unwrap_or(cx.project.1).max(1.0));
    // As many pixels as it covers on the target, within reason.
    let budget = (4.0 * target.width() as f64 * target.height() as f64).max(1e6);
    let k = (device_scale(ts) as f64).clamp(0.01, 64.0).min(4096.0 / cw).min(4096.0 / ch).min((budget / (cw * ch)).sqrt());
    let (pw, ph) = (((cw * k).ceil() as u32).max(1), ((ch * k).ceil() as u32).max(1));
    let Some(mut pic) = Pixmap::new(pw, ph) else { return };
    if let Some(bg) = &c.background {
        pic.fill(color(bg));
    }
    let Some(list) = cx.scene.comp_layers_at_with(id, ct, &cx.opts) else { return };
    let hidden = hidden_ids(&list);
    let (kx, ky) = (pw as f64 / cw, ph as f64 / ch);
    let base = Transform::from_translate(pw as f32 / 2.0, ph as f32 / 2.0).pre_scale(kx as f32, ky as f32);
    let inner = Cx {
        scene: cx.scene,
        comp: Some(&c.id),
        home: &list,
        hidden: &hidden,
        t: ct,
        rate: if time.is_some() { cx.rate } else { cx.rate * speed },
        depth: cx.depth + 1,
        nest: cx.nest,
        canvas: (cw, ch),
        project: cx.project,
        k: kx.min(ky) as f32,
        opts: cx.opts,
    };
    draw_list(&mut pic, &list, Place { to: base, home: base }, &inner, fx);
    let at = ts.pre_scale((cw / pw as f64) as f32, (ch / ph as f64) as f32).pre_translate(-(pw as f32) / 2.0, -(ph as f32) / 2.0);
    let paint = PixmapPaint { opacity: alpha, quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    target.draw_pixmap(0, 0, pic.as_ref(), &paint, at, None);
}

/// A particle layer: particles born where the layer is (after parenting) at each moment.
#[allow(clippy::too_many_arguments)]
fn particles(target: &mut Pixmap, l: &Layer, ps: &ParticleSystem, list: &[Layer], place: Place, alpha: f32, cx: &Cx, fx: &mut Flat) {
    let anchor = |list: &[Layer], l: &Layer| {
        let mut p = Point::from_xy(l.anchor_x as f32, l.anchor_y as f32);
        matrix(list, l).map_point(&mut p);
        [p.x as f64, p.y as f64, 0.0]
    };
    let now = anchor(list, l);
    let still = holds_still(cx, &l.id);
    let seen: RefCell<HashMap<i64, [f64; 3]>> = RefCell::new(HashMap::new());
    let origin = |time: f64| -> [f64; 3] {
        if still || (time - cx.t).abs() < 1e-9 || !time.is_finite() {
            return now;
        }
        let key = (time * 2000.0).round() as i64;
        if let Some(p) = seen.borrow().get(&key) {
            return *p;
        }
        let p = list_at(cx, time).and_then(|later| locate(&later, &l.id).map(|(_, sib, at)| anchor(sib, at))).unwrap_or(now);
        seen.borrow_mut().insert(key, p);
        p
    };
    let parts = ps.at(cx.t, false, &origin);
    let pic = match (ps.shape.as_deref(), &ps.asset) {
        (Some("image"), Some(asset)) => fx.pictures.picture(asset, cx.t),
        _ => None,
    };
    particles2d::draw(target, ps, &parts, place.to.pre_translate(now[0] as f32, now[1] as f32), alpha, pic.as_ref());
}

/// Does layer `id` stay where it is (no keyframes or expressions moving it or its parents)?
fn holds_still(cx: &Cx, id: &str) -> bool {
    let raw = match cx.comp {
        None => &cx.scene.layers[..],
        Some(c) => match cx.scene.composition(c) {
            Some(c) => &c.layers[..],
            None => return true,
        },
    };
    let Some((_, sib, mut l)) = locate(raw, id) else { return true };
    for _ in 0..64 {
        if !l.expressions.is_empty() || l.keyframes.keys().any(|k| MOVING.contains(&k.as_str())) {
            return false;
        }
        match l.parent.as_deref().and_then(|p| sib.iter().find(|x| x.id == p)) {
            Some(p) => l = p,
            None => break,
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Text

/// The style the text engine lays out for a text layer.
pub(crate) fn text_style(t: &TextLayer) -> TextStyle {
    TextStyle {
        content: t.shown(),
        font_family: t.font_family.clone(),
        font_size: t.font_size.max(0.0),
        font_weight: t.font_weight.clamp(100.0, 900.0) as u16,
        italic: t.italic,
        color: "#ffffff".into(),
        background: None,
        align: t.align,
        line_height: t.line_height,
        letter_spacing: t.letter_spacing,
        shadow: false,
    }
}

/// Where the block's centre sits from the layer's anchor: the anchor follows the alignment (x is
/// the left edge of left-aligned text). Text on a path sits on the path instead.
fn text_shift(t: &TextLayer, width: f64) -> f32 {
    if t.path.is_some() {
        return 0.0;
    }
    match t.align {
        TextAlign::Left => width as f32 / 2.0,
        TextAlign::Center => 0.0,
        TextAlign::Right => -(width as f32) / 2.0,
    }
}

fn text(target: &mut Pixmap, l: &Layer, t: &TextLayer, ts: Transform, alpha: f32, time: f64) {
    let style = text_style(t);
    let device = device_scale(ts).max(0.01);
    let layout = layout_cached(&style, device);
    let walk = t.path.as_deref().and_then(PathWalk::new);
    let ts = ts.pre_translate(text_shift(t, layout.width), 0.0);
    let bounds = Rect::from_xywh(-(layout.width as f32) / 2.0, -(layout.height as f32) / 2.0, layout.width.max(1.0) as f32, layout.height.max(1.0) as f32)
        .unwrap_or(Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("unit"));
    let fill = l.fill.as_ref().map_or(FillSpec::Solid(Color::WHITE), |f| FillSpec::of(f, bounds));
    let stroke = l.stroke.as_ref().filter(|s| s.width > 0.0).map(|s| {
        (FillSpec::Solid(color(&s.color)), Stroke { width: s.width as f32, line_join: LineJoin::Round, line_cap: LineCap::Round, ..Stroke::default() })
    });
    let looks = textfx::looks(&t.animators, &layout, time, t.align);
    let on_path = walk.as_ref().map(|w| OnPath { walk: w, offset: t.path_offset, align: t.align });
    draw_glyphs(target, &layout, ts, &fill, stroke.as_ref(), alpha, t.reveal.as_ref(), t.font_size as f32, &looks, on_path);
}

/// Text laid along a path: where it starts (0–1 of the path) and how it is aligned there.
pub(crate) struct OnPath<'a> {
    pub walk: &'a PathWalk,
    pub offset: f64,
    pub align: TextAlign,
}

/// Draws a laid-out block, each glyph moved and faded by the reveal and its animators, and placed
/// along a path when there is one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_glyphs(
    target: &mut Pixmap,
    layout: &Layout,
    ts: Transform,
    fill: &FillSpec,
    stroke: Option<&(FillSpec, Stroke)>,
    alpha: f32,
    reveal: Option<&kimchi_core::motion::Reveal>,
    font_size: f32,
    looks: &[GlyphLook],
    path: Option<OnPath>,
) {
    let out_cubic = Easing::EASE_OUT;
    let back = Easing::parse("easeOutBack").expect("named");
    let plain = GlyphLook::default();
    let first_baseline = layout.glyphs.first().map_or(0.0, |g| g.baseline);
    for (i, g) in layout.glyphs.iter().enumerate() {
        let (mut a, mut dx, mut dy, mut s) = (1.0f32, 0.0f32, 0.0f32, 1.0f32);
        if let Some(r) = reveal {
            let (n, i) = match r.by.as_str() {
                "word" => (layout.units.words, g.unit.words),
                "line" => (layout.units.lines, g.unit.lines),
                _ => (layout.units.chars, g.unit.chars),
            };
            let w = r.overlap.max(0.01);
            let u = ((r.progress * (n as f64 + w) - i as f64) / w).clamp(0.0, 1.0);
            let e = out_cubic.apply(u) as f32;
            let dist = r.distance.unwrap_or(font_size as f64 * 0.5) as f32;
            match r.style.as_str() {
                "fade" => a = e,
                "drop" => {
                    a = e;
                    dy = -(1.0 - e) * dist;
                }
                "slide" => {
                    a = e;
                    dx = (1.0 - e) * dist;
                }
                "pop" => {
                    a = (u as f32 * 3.0).min(1.0);
                    s = back.apply(u) as f32;
                }
                "type" => a = if u > 0.0 { 1.0 } else { 0.0 },
                "blur" => {
                    a = e;
                    s = 1.0 + (1.0 - e) * 0.25;
                }
                _ => {
                    a = e;
                    dy = (1.0 - e) * dist;
                }
            }
        }
        let look = looks.get(i).unwrap_or(&plain);
        let a = a * alpha * look.alpha;
        if a <= 0.002 || s <= 0.001 {
            continue;
        }
        let (cx, cy) = g.center;
        let place = match &path {
            Some(p) => {
                let start = match p.align {
                    TextAlign::Left => -(layout.width as f32) / 2.0,
                    TextAlign::Center => 0.0,
                    TextAlign::Right => layout.width as f32 / 2.0,
                };
                let along = p.offset as f32 * p.walk.length() + cx + look.shift - start;
                let (at, angle) = p.walk.at(along);
                Transform::from_translate(at[0], at[1]).pre_rotate(angle).pre_translate(-cx, -first_baseline)
            }
            None => Transform::from_translate(look.shift, 0.0),
        };
        let gts = ts.pre_concat(place).pre_concat(look.ts).pre_translate(dx + cx, dy + cy).pre_scale(s, s).pre_translate(-cx, -cy);
        if !gts.is_finite() {
            continue;
        }
        let tinted;
        let (fill, over) = match (look.fill, fill) {
            (Some((rgb, mix)), FillSpec::Solid(c)) => {
                let lerp = |a: f32, b: f32| a + (b - a) * mix;
                tinted = FillSpec::Solid(Color::from_rgba(lerp(c.red(), rgb[0]), lerp(c.green(), rgb[1]), lerp(c.blue(), rgb[2]), c.alpha()).unwrap_or(*c));
                (&tinted, None)
            }
            (Some((rgb, mix)), _) => (fill, Some((Color::from_rgba(rgb[0], rgb[1], rgb[2], 1.0).unwrap_or(Color::WHITE), mix))),
            (None, _) => (fill, None),
        };
        let blur = look.blur * device_scale(gts);
        if blur > 0.5 {
            blurred_glyph(target, &g.ink, gts, fill, over, stroke, a, blur);
        } else {
            glyph(target, &g.ink, gts, fill, over, stroke, a);
        }
    }
}

/// One glyph's ink: its fill (and a colour laid over it), synthetic bold and outline.
#[allow(clippy::too_many_arguments)]
fn glyph(target: &mut Pixmap, ink: &Ink, gts: Transform, fill: &FillSpec, over: Option<(Color, f32)>, stroke: Option<&(FillSpec, Stroke)>, a: f32) {
    match ink {
        Ink::Outline { path, embolden } => {
            let mut paints = vec![fill.paint(a)];
            if let Some((c, mix)) = over {
                paints.push(FillSpec::Solid(c).paint(a * mix));
            }
            for paint in &paints {
                target.fill_path(path, paint, FillRule::Winding, gts, None);
                if *embolden > 0.0 {
                    let st = Stroke { width: *embolden, ..Stroke::default() };
                    target.stroke_path(path, paint, &st, gts, None);
                }
            }
            if let Some((spec, st)) = stroke {
                target.stroke_path(path, &spec.paint(a), st, gts, None);
            }
        }
        Ink::Image { pixmap, x, y, size } => {
            let at = gts.pre_translate(*x, *y).pre_scale(1.0 / size, 1.0 / size);
            let paint = PixmapPaint { opacity: a, quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
            target.draw_pixmap(0, 0, pixmap.as_ref(), &paint, at, None);
        }
    }
}

/// A glyph blurred on its own (text animators' blur): drawn on a small picture around it.
#[allow(clippy::too_many_arguments)]
fn blurred_glyph(target: &mut Pixmap, ink: &Ink, gts: Transform, fill: &FillSpec, over: Option<(Color, f32)>, stroke: Option<&(FillSpec, Stroke)>, a: f32, blur: f32) {
    let local = match ink {
        Ink::Outline { path, .. } => path.bounds(),
        Ink::Image { pixmap, x, y, size } => {
            Rect::from_xywh(*x, *y, pixmap.width() as f32 / size, pixmap.height() as f32 / size).unwrap_or(Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("unit"))
        }
    };
    let c = corners_of(local, gts);
    let reach = blur * 1.5 + 4.0;
    let (x0, y0) = (c.iter().map(|p| p[0]).fold(f32::MAX, f32::min) - reach, c.iter().map(|p| p[1]).fold(f32::MAX, f32::min) - reach);
    let (x1, y1) = (c.iter().map(|p| p[0]).fold(f32::MIN, f32::max) + reach, c.iter().map(|p| p[1]).fold(f32::MIN, f32::max) + reach);
    // Only the part on the target.
    let (x0, y0) = (x0.max(-reach).floor(), y0.max(-reach).floor());
    let (x1, y1) = (x1.min(target.width() as f32 + reach).ceil(), y1.min(target.height() as f32 + reach).ceil());
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let Some(mut small) = Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32) else { return };
    glyph(&mut small, ink, gts.post_translate(-x0, -y0), fill, over, stroke, a);
    paint::blur(&mut small, blur);
    target.draw_pixmap(0, 0, small.as_ref(), &PixmapPaint::default(), Transform::from_translate(x0, y0), None);
}

// ---------------------------------------------------------------------------------------------
// Geometry, for effects and the Studio

/// The four corners of `r` through `ts` (top left, top right, bottom right, bottom left).
fn corners_of(r: Rect, ts: Transform) -> [[f32; 2]; 4] {
    let mut pts = [Point::from_xy(r.left(), r.top()), Point::from_xy(r.right(), r.top()), Point::from_xy(r.right(), r.bottom()), Point::from_xy(r.left(), r.bottom())];
    ts.map_points(&mut pts);
    pts.map(|p| [p.x, p.y])
}

fn centred(w: f64, h: f64) -> Option<Rect> {
    let (w, h) = (w.abs() as f32, h.abs() as f32);
    Rect::from_ltrb(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Rect::from_ltrb(a.left().min(b.left()), a.top().min(b.top()), a.right().max(b.right()), a.bottom().max(b.bottom())),
        (a, None) => a,
        (None, b) => b,
    }
}

fn mapped(r: Rect, ts: Transform) -> Option<Rect> {
    let c = corners_of(r, ts);
    let xs = c.map(|p| p[0]);
    let ys = c.map(|p| p[1]);
    Rect::from_ltrb(
        xs.iter().copied().fold(f32::MAX, f32::min),
        ys.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
        ys.iter().copied().fold(f32::MIN, f32::max),
    )
}

/// What a layer covers in its own pixels (repeater copies included); `stroked` adds half the
/// stroke. `project` is the scene's canvas, `canvas` the one the layer is on.
fn local_bounds(l: &Layer, scene: &Scene2d, project: (f64, f64), canvas: (f64, f64), t: f64, stroked: bool) -> Option<Rect> {
    let one = bounds_once(l, scene, project, canvas, t, stroked)?;
    match shapeops::copies(&l.operators) {
        Some(copies) if !matches!(l.kind, LayerKind::Particles(_)) => copies.iter().fold(None, |acc, (ts, _)| union(acc, mapped(one, *ts))),
        _ => Some(one),
    }
}

/// [`local_bounds`] of one copy (repeaters left out).
fn bounds_once(l: &Layer, scene: &Scene2d, project: (f64, f64), canvas: (f64, f64), t: f64, stroked: bool) -> Option<Rect> {
    match &l.kind {
        LayerKind::Rect { .. } | LayerKind::Ellipse { .. } | LayerKind::Polygon { .. } | LayerKind::Star { .. } | LayerKind::Path { .. } => {
            let b = shape_outline(l, t)?.bounds();
            let s = if stroked { l.stroke.as_ref().map_or(0.0, |s| s.width.max(0.0) as f32 / 2.0) } else { 0.0 };
            Rect::from_ltrb(b.left() - s, b.top() - s, b.right() + s, b.bottom() + s)
        }
        LayerKind::Text(tl) => {
            let layout = layout_cached(&text_style(tl), 0.0);
            match tl.path.as_deref().and_then(|d| paint::svg_path(d, &[], false)) {
                Some(p) => {
                    let (b, f) = (p.bounds(), tl.font_size as f32);
                    Rect::from_ltrb(b.left() - f, b.top() - f, b.right() + f, b.bottom() + f)
                }
                None => {
                    let shift = text_shift(tl, layout.width);
                    let (w, h) = (layout.width as f32, layout.height as f32);
                    Rect::from_ltrb(shift - w / 2.0, -h / 2.0, shift + w / 2.0, h / 2.0)
                }
            }
        }
        LayerKind::Image { asset, width, height, .. } => {
            let (w, h) = image_box(asset, *width, *height, None);
            centred(w, h)
        }
        LayerKind::Group { layers } => {
            let mut out = None;
            for c in layers {
                if let Some(b) = local_bounds(c, scene, project, canvas, t, stroked) {
                    out = union(out, mapped(b, matrix(layers, c)));
                }
            }
            out
        }
        LayerKind::Comp { comp, .. } => {
            let c = scene.composition(comp);
            centred(c.and_then(|c| c.width).unwrap_or(project.0), c.and_then(|c| c.height).unwrap_or(project.1))
        }
        LayerKind::Null {} => centred(20.0, 20.0),
        LayerKind::Adjustment {} => centred(canvas.0, canvas.1),
        LayerKind::Particles(p) => {
            let s = p.emitter_size.clone().unwrap_or_default();
            let get = |i: usize, d: f64| s.get(i).or(s.first()).copied().unwrap_or(d);
            match p.emitter.as_str() {
                "line" => centred(get(0, 100.0).max(20.0), 20.0),
                "rect" | "box" => centred(get(0, 100.0).max(20.0), get(1, 100.0).max(20.0)),
                "circle" | "disc" | "ring" | "sphere" => centred((get(0, 50.0) * 2.0).max(20.0), (get(0, 50.0) * 2.0).max(20.0)),
                _ => centred(20.0, 20.0),
            }
        }
    }
}

/// The project canvas last drawn (compositions without a size use it) and what expressions saw
/// then, for the Studio.
fn last_canvas() -> &'static Mutex<((f64, f64), EvalOptions)> {
    static C: OnceLock<Mutex<((f64, f64), EvalOptions)>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(((1920.0, 1080.0), EvalOptions::default())))
}

fn remember_canvas(size: (f64, f64), opts: EvalOptions) {
    if size.0.is_finite() && size.1.is_finite() && size.0 > 0.0 && size.1 > 0.0 {
        *last_canvas().lock().unwrap_or_else(|e| e.into_inner()) = (size, opts);
    }
}

/// The layers the Studio looks at: the scene's or a composition's, at `t`, with their canvas.
/// Layers, the canvas they are on and the project's canvas.
type StudioList = (Vec<Layer>, (f64, f64), (f64, f64));

fn studio_list(scene: &Scene2d, t: f64, comp: Option<&str>) -> Option<StudioList> {
    let (project, opts) = *last_canvas().lock().unwrap_or_else(|e| e.into_inner());
    match comp {
        None => Some((scene.layers_at_with(t, &opts), project, project)),
        Some(id) => {
            let c = scene.composition(id)?;
            let canvas = (c.width.unwrap_or(project.0), c.height.unwrap_or(project.1));
            Some((scene.comp_layers_at_with(id, t, &opts)?, canvas, project))
        }
    }
}

/// A layer's full transform at scene time `t` (parents and groups included): its own pixels
/// (from its centre) → project pixels from the canvas centre (or the composition's, with
/// `comp`). As `[a, b, c, d, e, f]`: x' = a·x + c·y + e, y' = b·x + d·y + f.
pub fn layer_transform(scene: &Scene2d, t: f64, comp: Option<&str>, id: &str) -> Option<[f64; 6]> {
    let (list, ..) = studio_list(scene, t, comp)?;
    let (chain, sib, l) = locate(&list, id)?;
    let m = chain.pre_concat(matrix(sib, l));
    Some([m.sx, m.ky, m.kx, m.sy, m.tx, m.ty].map(f64::from))
}

/// A layer's four corners at scene time `t` (top left, top right, bottom right, bottom left of
/// what it covers), in project pixels from the canvas centre (or the composition's), after its
/// full transform: for the Studio's handles.
pub fn layer_bounds(scene: &Scene2d, t: f64, comp: Option<&str>, id: &str) -> Option<[[f64; 2]; 4]> {
    let (list, canvas, project) = studio_list(scene, t, comp)?;
    let (chain, sib, l) = locate(&list, id)?;
    let r = local_bounds(l, scene, project, canvas, t, false)?;
    let c = corners_of(r, chain.pre_concat(matrix(sib, l)));
    Some(c.map(|p| [p[0] as f64, p[1] as f64]))
}

/// The topmost visible layer whose shape holds `point` (project pixels from the canvas centre,
/// or the composition's) at scene time `t`. Inside a group, the group's layer under the point.
pub fn hit_test(scene: &Scene2d, t: f64, comp: Option<&str>, point: [f64; 2]) -> Option<String> {
    let (list, canvas, project) = studio_list(scene, t, comp)?;
    let hidden = hidden_ids(&list);
    hit(&list, Transform::identity(), [point[0] as f32, point[1] as f32], t, &hidden, scene, canvas, project)
}

#[allow(clippy::too_many_arguments)]
fn hit(list: &[Layer], ts: Transform, pt: [f32; 2], t: f64, hidden: &HashSet<String>, scene: &Scene2d, canvas: (f64, f64), project: (f64, f64)) -> Option<String> {
    for l in list.iter().rev() {
        if !l.visible_at(t) || hidden.contains(&l.id) || l.opacity <= 0.0 || matches!(l.kind, LayerKind::Null {} | LayerKind::Adjustment {}) {
            continue;
        }
        let m = ts.pre_concat(matrix(list, l));
        if let LayerKind::Group { layers } = &l.kind {
            let copies = shapeops::copies(&l.operators).unwrap_or_else(|| vec![(Transform::identity(), 1.0)]);
            if let Some(id) = copies.iter().rev().find_map(|(c, _)| hit(layers, m.pre_concat(*c), pt, t, hidden, scene, canvas, project)) {
                return Some(id);
            }
            continue;
        }
        let copies = shapeops::copies(&l.operators).unwrap_or_else(|| vec![(Transform::identity(), 1.0)]);
        for (c, _) in copies.iter().rev() {
            let Some(inv) = m.pre_concat(*c).invert() else { continue };
            let mut p = Point::from_xy(pt[0], pt[1]);
            inv.map_point(&mut p);
            if holds(l, [p.x, p.y], t, scene, canvas, project) {
                return Some(l.id.clone());
            }
        }
    }
    None
}

/// Is a point (the layer's own pixels) on the layer's shape?
fn holds(l: &Layer, p: [f32; 2], t: f64, scene: &Scene2d, canvas: (f64, f64), project: (f64, f64)) -> bool {
    let stroke = l.stroke.as_ref().map_or(0.0, |s| s.width.max(0.0) as f32 / 2.0);
    match &l.kind {
        LayerKind::Rect { width, height, .. } => p[0].abs() <= *width as f32 / 2.0 + stroke && p[1].abs() <= *height as f32 / 2.0 + stroke,
        LayerKind::Ellipse { width, height } => {
            let (rx, ry) = ((*width as f32 / 2.0 + stroke).max(1e-3), (*height as f32 / 2.0 + stroke).max(1e-3));
            (p[0] / rx).powi(2) + (p[1] / ry).powi(2) <= 1.0
        }
        LayerKind::Polygon { .. } | LayerKind::Star { .. } | LayerKind::Path { .. } => {
            let Some(path) = shape_outline(l, t) else { return false };
            let filled = l.fill.is_some() || !matches!(l.kind, LayerKind::Path { .. });
            (filled && shapeops::contains(&path, p)) || shapeops::distance_to(&path, p) <= stroke.max(3.0)
        }
        _ => bounds_once(l, scene, project, canvas, t, true).is_some_and(|r| p[0] >= r.left() && p[0] <= r.right() && p[1] >= r.top() && p[1] <= r.bottom()),
    }
}

#[cfg(test)]
#[path = "flat_tests.rs"]
mod tests;
