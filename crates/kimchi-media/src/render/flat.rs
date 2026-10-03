//! Draws a 2D motion scene ([`Scene2d`]) at one instant with tiny-skia.
//!
//! Layers are drawn straight onto the target when they can be (a shape with a fill or a stroke),
//! or on a layer of their own first when something applies to the layer as a whole: group
//! opacity, a blend mode, a mask, blur, a shadow or a glow.

use std::collections::HashSet;
use std::sync::Arc;

use kimchi_core::anim::{Easing, value_at};
use kimchi_core::motion::{Layer, LayerKind, Scene2d, TextLayer, find_layer, walk_layers};
use kimchi_core::{TextAlign, TextStyle};
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Mask, MaskType, Path, Pixmap, PixmapPaint, Rect, Stroke, Transform};

use super::paint::{self, FillSpec, color, with_alpha};
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
}

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
    let layers: Vec<Layer> = scene.layers.iter().map(|l| l.at(t)).collect();
    let mut masks = HashSet::new();
    walk_layers(&layers, &mut |l| {
        if let Some(m) = &l.mask {
            masks.insert(m.clone());
        }
    });
    let cx = Cx { all: &layers, masks: &masks, t };
    for l in &layers {
        draw_layer(canvas, l, base, &cx, fx);
    }
}

struct Cx<'a> {
    /// Every layer at this instant (to find masks).
    all: &'a [Layer],
    /// Ids used as masks: not drawn on their own.
    masks: &'a HashSet<String>,
    t: f64,
}

fn local(l: &Layer, parent: Transform) -> Transform {
    let skew = (l.skew_x as f32).to_radians().tan();
    parent
        .pre_translate(l.x as f32, l.y as f32)
        .pre_rotate(l.rotation as f32)
        .pre_concat(Transform::from_row(1.0, 0.0, skew, 1.0, 0.0, 0.0))
        .pre_scale((l.scale * l.scale_x) as f32, (l.scale * l.scale_y) as f32)
        .pre_translate(-l.anchor_x as f32, -l.anchor_y as f32)
}

fn draw_layer(canvas: &mut Pixmap, l: &Layer, parent: Transform, cx: &Cx, fx: &mut Flat) {
    if !l.visible_at(cx.t) || cx.masks.contains(&l.id) || l.opacity <= 0.0 {
        return;
    }
    let opacity = l.opacity.clamp(0.0, 1.0) as f32;
    let ts = local(l, parent);
    let both = l.fill.is_some() && l.stroke.is_some();
    let group = matches!(l.kind, LayerKind::Group { .. });
    let alone = l.blend != Default::default()
        || l.mask.is_some()
        || l.blur > 0.0
        || l.shadow.is_some()
        || l.glow.is_some()
        || ((group || both) && opacity < 1.0);
    if !alone {
        content(canvas, l, ts, opacity, cx, fx);
        return;
    }
    // A strongly blurred layer is drawn small, blurred there and scaled back up: the same picture,
    // many times faster (shadows and glows need the sharp layer, so they stay full size).
    let radius = l.blur as f32 * fx.scale;
    let f = if l.shadow.is_none() && l.glow.is_none() && radius >= 12.0 { ((radius / 6.0).floor() as u32).clamp(1, 8) } else { 1 };
    let (w, h) = ((canvas.width() / f).max(1), (canvas.height() / f).max(1));
    let (kx, ky) = (w as f32 / canvas.width() as f32, h as f32 / canvas.height() as f32);
    let down = Transform::from_scale(kx, ky);
    let Some(mut own) = Pixmap::new(w, h) else { return };
    content(&mut own, l, down.pre_concat(ts), 1.0, cx, fx);
    if let Some(id) = &l.mask
        && let Some(m) = find_layer(cx.all, id)
        && let Some(mut shape) = Pixmap::new(w, h)
    {
        // The mask shares the masked layer's parent (they are siblings).
        let mut m = m.clone();
        m.mask = None;
        // A mask is its shape: without paint of its own, it counts as filled.
        if m.fill.is_none() && m.stroke.is_none() {
            m.fill = Some(kimchi_core::motion::Fill::Color("#ffffff".into()));
        }
        let free = HashSet::new();
        let mcx = Cx { all: cx.all, masks: &free, t: cx.t };
        draw_layer(&mut shape, &m, down.pre_concat(parent), &mcx, fx);
        let mut mask = Mask::from_pixmap(shape.as_ref(), MaskType::Alpha);
        if l.mask_invert {
            mask.invert();
        }
        own.apply_mask(&mask);
    }
    if l.blur > 0.0 {
        paint::blur(&mut own, radius * kx);
    }
    if let Some(s) = &l.shadow {
        let k = fx.scale;
        paint::shadow_under(canvas, &own, with_alpha(color(&s.color), opacity), s.blur as f32 * k, s.x as f32 * k, s.y as f32 * k);
    }
    if let Some(g) = &l.glow {
        paint::glow_under(canvas, &own, color(&g.color), g.radius as f32 * fx.scale, (g.strength * l.opacity) as f32);
    }
    let paint = PixmapPaint { opacity, blend_mode: paint::blend(l.blend), quality: tiny_skia::FilterQuality::Bilinear };
    canvas.draw_pixmap(0, 0, own.as_ref(), &paint, Transform::from_scale(1.0 / kx, 1.0 / ky), None);
}

/// What the layer itself draws (children for groups), at `alpha`.
fn content(target: &mut Pixmap, l: &Layer, ts: Transform, alpha: f32, cx: &Cx, fx: &mut Flat) {
    let outline = match &l.kind {
        LayerKind::Rect { width, height, radius } => paint::rect(*width as f32, *height as f32, *radius as f32),
        LayerKind::Ellipse { width, height } => paint::ellipse(*width as f32, *height as f32),
        LayerKind::Polygon { sides, radius, roundness } => paint::polygon(*sides as f32, *radius as f32, *roundness as f32),
        LayerKind::Star { points, radius, inner_radius } => paint::star(*points as f32, *radius as f32, *inner_radius as f32),
        LayerKind::Path { d, points, closed } => paint::svg_path(d, points, *closed),
        // Drawn by the 2D engine's newer parts (compositions, particles); nulls and adjustment
        // layers draw nothing of their own.
        LayerKind::Null {} | LayerKind::Adjustment {} | LayerKind::Comp { .. } | LayerKind::Particles(_) => return,
        LayerKind::Text(t) => {
            text(target, l, t, ts, alpha);
            return;
        }
        LayerKind::Image { asset, width, height, radius } => {
            image(target, asset, *width, *height, *radius, ts, alpha, cx.t, fx);
            return;
        }
        LayerKind::Group { layers } => {
            // The group's opacity is already in `alpha` only when drawn on its own layer.
            for child in layers {
                let mut child = child.clone();
                if alpha < 1.0 {
                    child.opacity *= alpha as f64;
                }
                draw_layer(target, &child, ts, cx, fx);
            }
            return;
        }
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

#[allow(clippy::too_many_arguments)]
fn image(target: &mut Pixmap, asset: &str, width: Option<f64>, height: Option<f64>, radius: f64, ts: Transform, alpha: f32, t: f64, fx: &mut Flat) {
    let Some((pic, nw, nh)) = fx.pictures.picture(asset, t) else { return };
    let aspect = nw / nh.max(1e-6);
    let (w, h) = match (width, height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w / aspect),
        (None, Some(h)) => (h * aspect, h),
        (None, None) => (nw, nh),
    };
    let Some(box_) = paint::rect(w as f32, h as f32, radius as f32) else { return };
    // The picture spans the box: pixmap pixels → layer pixels.
    let to_box = Transform::from_translate(-w as f32 / 2.0, -h as f32 / 2.0)
        .pre_scale(w as f32 / pic.width() as f32, h as f32 / pic.height() as f32);
    let shader = tiny_skia::Pattern::new(pic.as_ref().as_ref(), tiny_skia::SpreadMode::Pad, tiny_skia::FilterQuality::Bilinear, alpha, to_box);
    let paint = tiny_skia::Paint { shader, anti_alias: true, ..tiny_skia::Paint::default() };
    target.fill_path(&box_, &paint, FillRule::Winding, ts, None);
}

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

fn text(target: &mut Pixmap, l: &Layer, t: &TextLayer, ts: Transform, alpha: f32) {
    let style = text_style(t);
    let device = (ts.sx * ts.sy - ts.kx * ts.ky).abs().sqrt().max(0.01);
    let layout = layout_cached(&style, device);
    // The anchor follows the alignment: x is the left edge of left-aligned text.
    let shift = match t.align {
        TextAlign::Left => layout.width as f32 / 2.0,
        TextAlign::Center => 0.0,
        TextAlign::Right => -(layout.width as f32) / 2.0,
    };
    let ts = ts.pre_translate(shift, 0.0);
    let bounds = Rect::from_xywh(-(layout.width as f32) / 2.0, -(layout.height as f32) / 2.0, layout.width.max(1.0) as f32, layout.height.max(1.0) as f32)
        .unwrap_or(Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("unit"));
    let fill = l.fill.as_ref().map_or(FillSpec::Solid(Color::WHITE), |f| FillSpec::of(f, bounds));
    let stroke = l.stroke.as_ref().filter(|s| s.width > 0.0).map(|s| {
        (
            FillSpec::Solid(color(&s.color)),
            Stroke { width: s.width as f32, line_join: LineJoin::Round, line_cap: LineCap::Round, ..Stroke::default() },
        )
    });
    draw_glyphs(target, &layout, ts, &fill, stroke.as_ref(), alpha, t.reveal.as_ref(), t.font_size as f32);
}

/// Draws a laid-out block, each glyph moved and faded by the reveal.
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
) {
    let out_cubic = Easing::EASE_OUT;
    let back = Easing::parse("easeOutBack").expect("named");
    for g in &layout.glyphs {
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
        let a = a * alpha;
        if a <= 0.002 || s <= 0.001 {
            continue;
        }
        let (cx, cy) = g.center;
        let gts = ts.pre_translate(dx + cx, dy + cy).pre_scale(s, s).pre_translate(-cx, -cy);
        match &g.ink {
            Ink::Outline { path, embolden } => {
                let paint = fill.paint(a);
                target.fill_path(path, &paint, FillRule::Winding, gts, None);
                if *embolden > 0.0 {
                    let st = Stroke { width: *embolden, ..Stroke::default() };
                    target.stroke_path(path, &paint, &st, gts, None);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::Scene;
    use serde_json::json;

    struct NoPictures;
    impl Pictures for NoPictures {
        fn picture(&mut self, _: &str, _: f64) -> Option<(Arc<Pixmap>, f64, f64)> {
            let mut p = Pixmap::new(4, 2).unwrap();
            p.fill(Color::from_rgba8(0, 0, 255, 255));
            Some((Arc::new(p), 40.0, 20.0))
        }
    }

    fn render(scene: serde_json::Value, t: f64) -> Pixmap {
        let Scene::Flat(s) = Scene::from_json(&scene).unwrap() else { panic!("2d") };
        let mut canvas = Pixmap::new(200, 100).unwrap();
        let mut pics = NoPictures;
        let mut fx = Flat { pictures: &mut pics, scale: 1.0, quality: super::super::Quality::Final };
        draw(&mut canvas, &s, t, Transform::from_translate(100.0, 50.0), &mut fx);
        canvas
    }

    fn rgba(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap().demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn shapes_move_with_keyframes() {
        let scene = json!({"background": "#000000", "layers": [
            {"id": "box", "type": "rect", "width": 20, "height": 20, "fill": "#ff0000",
             "keyframes": {"x": [[0, -60], [1, 60]]}}
        ]});
        let a = render(scene.clone(), 0.0);
        assert_eq!(rgba(&a, 40, 50), [255, 0, 0, 255]);
        assert_eq!(rgba(&a, 160, 50), [0, 0, 0, 255]);
        let b = render(scene, 1.0);
        assert_eq!(rgba(&b, 160, 50), [255, 0, 0, 255]);
    }

    #[test]
    fn groups_masks_and_opacity() {
        let scene = json!({"layers": [
            {"id": "g", "type": "group", "opacity": 0.5, "layers": [
                {"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#ffffff"},
                {"id": "b", "type": "rect", "width": 40, "height": 40, "fill": "#ffffff", "x": 10}
            ]},
            {"id": "hole", "type": "ellipse", "width": 20, "height": 20, "x": 70},
            {"id": "masked", "type": "rect", "width": 60, "height": 60, "x": 70, "fill": "#00ff00", "mask": "hole"}
        ]});
        let p = render(scene, 0.0);
        // Overlapping children of a half-opaque group don't add up.
        assert_eq!(rgba(&p, 105, 50)[3], 128);
        assert_eq!(rgba(&p, 85, 50)[3], 128);
        // The mask shape isn't drawn; the masked layer shows only inside it.
        assert_eq!(rgba(&p, 170, 50), [0, 255, 0, 255]);
        assert_eq!(rgba(&p, 170 + 25, 50)[3], 0);
    }

    #[test]
    fn trims_strokes_and_draws_images() {
        let scene = json!({"layers": [
            {"id": "line", "type": "path", "d": "M-90 0 L90 0", "stroke": {"color": "#ffffff", "width": 6, "cap": "butt"},
             "keyframes": {"trimEnd": [[0, 0], [1, 1]]}},
            {"id": "pic", "type": "image", "asset": "x", "y": 30}
        ]});
        let half = render(scene.clone(), 0.5);
        assert!(rgba(&half, 50, 50)[3] > 200);
        assert_eq!(rgba(&half, 150, 50)[3], 0);
        assert_eq!(rgba(&render(scene, 1.0), 150, 50)[3], 255);
        // The 4×2 picture drawn at its 40×20 project size.
        assert_eq!(rgba(&half, 100, 80), [0, 0, 255, 255]);
        assert_eq!(rgba(&half, 125, 80)[3], 0);
    }

    #[test]
    fn text_reveals_letter_by_letter() {
        let scene = json!({"layers": [
            {"id": "t", "type": "text", "text": "IIII", "fontSize": 40, "fill": "#ffffff", "align": "left", "x": -90,
             "reveal": {"by": "char", "style": "type", "overlap": 0.01}, "keyframes": {"reveal": [[0, 0], [1, 1]]}}
        ]});
        let ink = |p: &Pixmap| (0..200).filter(|x| (0..100).any(|y| rgba(p, *x, y)[3] > 128)).max();
        assert_eq!(ink(&render(scene.clone(), 0.0)), None);
        let quarter = ink(&render(scene.clone(), 0.3)).unwrap();
        let all = ink(&render(scene, 1.0)).unwrap();
        assert!(quarter < all, "{quarter} < {all}");
        // Left-aligned: the text starts at x = -90 (10 px on the canvas).
        assert!(all < 120);
    }
}
