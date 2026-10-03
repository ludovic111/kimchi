//! Pixel checks of the 2D engine on small canvases.

use super::*;
use kimchi_core::Scene;
use kimchi_core::motion::stack::{EFFECTS, ParamKind};
use serde_json::json;

use crate::render::Quality;

struct Blue;
impl Pictures for Blue {
    fn picture(&mut self, _: &str, _: f64) -> Option<(Arc<Pixmap>, f64, f64)> {
        let mut p = Pixmap::new(4, 2).unwrap();
        p.fill(Color::from_rgba8(0, 0, 255, 255));
        Some((Arc::new(p), 40.0, 20.0))
    }
}

fn scene(v: serde_json::Value) -> Scene2d {
    match Scene::from_json(&v) {
        Ok(Scene::Flat(s)) => s,
        Ok(_) => panic!("2d"),
        Err(e) => panic!("{e}"),
    }
}

fn draw_on(s: &Scene2d, t: f64, w: u32, h: u32, quality: Quality) -> Pixmap {
    let mut canvas = Pixmap::new(w, h).unwrap();
    let mut pics = Blue;
    let mut fx = Flat { pictures: &mut pics, scale: 1.0, quality, frame: 1.0 / 30.0 };
    draw(&mut canvas, s, t, Transform::from_translate(w as f32 / 2.0, h as f32 / 2.0), &mut fx);
    canvas
}

/// A 200×100 canvas, final quality.
fn render(v: serde_json::Value, t: f64) -> Pixmap {
    draw_on(&scene(v), t, 200, 100, Quality::Final)
}

fn rgba(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = p.pixel(x, y).unwrap().demultiply();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

fn alpha(p: &Pixmap, x: u32, y: u32) -> u8 {
    p.pixel(x, y).unwrap().alpha()
}

/// Leftmost and rightmost columns with ink (alpha > 128) on row `y`.
fn span(p: &Pixmap, y: u32) -> Option<(u32, u32)> {
    let xs: Vec<u32> = (0..p.width()).filter(|x| alpha(p, *x, y) > 128).collect();
    Some((*xs.first()?, *xs.last()?))
}

/// `KIMCHI_DUMP=dir` writes a test's picture there to look at.
#[allow(dead_code)]
fn dump(name: &str, p: &Pixmap) {
    if let Some(dir) = std::env::var_os("KIMCHI_DUMP") {
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        p.save_png(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn close(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
}

// ---- carried over -------------------------------------------------------------------------

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

// ---- masks and mattes --------------------------------------------------------------------

fn masked(masks: serde_json::Value) -> Pixmap {
    render(json!({"layers": [{"id": "a", "type": "rect", "width": 160, "height": 80, "fill": "#ffffff", "masks": masks}]}), 0.0)
}

#[test]
fn masks_combine_by_mode() {
    let add = masked(json!([{"type": "rect", "size": [40, 40]}]));
    assert_eq!(alpha(&add, 100, 50), 255);
    assert_eq!(alpha(&add, 140, 50), 0);
    let sub = masked(json!([{"type": "rect", "size": [40, 40], "mode": "subtract"}]));
    assert_eq!(alpha(&sub, 100, 50), 0);
    assert_eq!(alpha(&sub, 140, 50), 255);
    let inter = masked(json!([{"type": "rect", "center": [-15, 0], "size": [40, 40]}, {"type": "ellipse", "center": [15, 0], "size": [40, 40], "mode": "intersect"}]));
    assert_eq!(alpha(&inter, 100, 50), 255);
    assert_eq!(alpha(&inter, 82, 50), 0);
    assert_eq!(alpha(&inter, 118, 50), 0);
    let diff = masked(json!([{"type": "rect", "center": [-15, 0], "size": [40, 40]}, {"type": "rect", "center": [15, 0], "size": [40, 40], "mode": "difference"}]));
    assert_eq!(alpha(&diff, 100, 50), 0);
    assert_eq!(alpha(&diff, 82, 50), 255);
    assert_eq!(alpha(&diff, 125, 50), 255);
    let none = masked(json!([{"type": "rect", "size": [40, 40], "mode": "none"}]));
    assert_eq!(alpha(&none, 140, 50), 255);
    let inverted = masked(json!([{"type": "rect", "size": [40, 40], "inverted": true}]));
    assert_eq!(alpha(&inverted, 100, 50), 0);
    assert_eq!(alpha(&inverted, 140, 50), 255);
    // In the layer's own space: they move and turn with it.
    let moved = render(json!({"layers": [{"id": "a", "type": "rect", "width": 160, "height": 80, "fill": "#ffffff", "x": 30, "masks": [{"type": "rect", "size": [40, 40]}]}]}), 0.0);
    assert_eq!(alpha(&moved, 130, 50), 255);
    assert_eq!(alpha(&moved, 100, 50), 0);
}

#[test]
fn masks_feather_and_expand() {
    let soft = masked(json!([{"type": "rect", "size": [40, 40], "feather": 20}]));
    let edge = alpha(&soft, 120, 50);
    assert!((90..170).contains(&edge), "half at the edge: {edge}");
    assert!(alpha(&soft, 127, 50) > 0 && alpha(&soft, 127, 50) < edge);
    assert!(alpha(&soft, 113, 50) > edge && alpha(&soft, 100, 50) > 250);
    let grown = masked(json!([{"type": "rect", "size": [40, 40], "expansion": 10}]));
    assert_eq!(alpha(&grown, 127, 50), 255);
    let opaque = masked(json!([{"type": "rect", "size": [40, 40], "opacity": 0.5}]));
    assert!(alpha(&opaque, 100, 50).abs_diff(128) <= 1);
}

fn matted(mode: &str, matte_fill: &str) -> Pixmap {
    render(
        json!({"layers": [
            {"id": "m", "type": "rect", "width": 100, "height": 100, "x": -50, "fill": matte_fill},
            {"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#ff0000", "matte": {"layer": "m", "mode": mode}}
        ]}),
        0.0,
    )
}

#[test]
fn track_mattes_in_every_mode() {
    let a = matted("alpha", "#ffffff");
    assert_eq!(rgba(&a, 50, 50), [255, 0, 0, 255], "shows where the matte is, and the matte isn't drawn");
    assert_eq!(alpha(&a, 150, 50), 0);
    let ai = matted("alphaInverted", "#ffffff");
    assert_eq!(alpha(&ai, 50, 50), 0);
    assert_eq!(rgba(&ai, 150, 50), [255, 0, 0, 255]);
    let l = matted("luma", "#808080");
    assert!(alpha(&l, 50, 50).abs_diff(128) <= 2, "{}", alpha(&l, 50, 50));
    assert_eq!(alpha(&l, 150, 50), 0);
    let li = matted("lumaInverted", "#ffffff");
    assert_eq!(alpha(&li, 50, 50), 0);
    assert_eq!(alpha(&li, 150, 50), 255);
    // A matte layer's own effects shape the matte (a blurred matte gives a soft edge).
    let soft = render(
        json!({"layers": [
            {"id": "m", "type": "rect", "width": 100, "height": 100, "x": -50, "fill": "#ffffff", "effects": [{"type": "blur", "radius": 16}]},
            {"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#ff0000", "matte": {"layer": "m"}}
        ]}),
        0.0,
    );
    let edge = alpha(&soft, 100, 50);
    assert!(edge > 60 && edge < 200, "{edge}");
}

// ---- effects -----------------------------------------------------------------------------

fn square_with(effects: serde_json::Value) -> Pixmap {
    render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#ff0000", "effects": effects}]}), 0.0)
}

#[test]
fn blurs_spread_and_keep_direction() {
    let plain = square_with(json!([]));
    assert_eq!(alpha(&plain, 125, 50), 0);
    let both = square_with(json!([{"type": "blur", "radius": 12}]));
    assert!(alpha(&both, 125, 50) > 0 && alpha(&both, 100, 75) > 0);
    assert!(alpha(&both, 100, 50) > 240, "the middle stays solid");
    let across = square_with(json!([{"type": "blur", "radius": 12, "dimensions": "horizontal"}]));
    assert!(alpha(&across, 125, 50) > 0);
    assert_eq!(alpha(&across, 100, 75), 0, "nothing up or down");
    let dir = square_with(json!([{"type": "directionalBlur", "length": 40, "angle": 90}]));
    assert!(alpha(&dir, 100, 80) > 0 && alpha(&dir, 135, 50) == 0, "vertical smear");
    let diag = square_with(json!([{"type": "directionalBlur", "length": 40, "angle": 45}]));
    assert!(alpha(&diag, 126, 76) > 0 && alpha(&diag, 74, 24) > 0, "diagonal smear");
    assert_eq!(alpha(&diag, 126, 24), 0, "along its angle only");
    let zoom = render(json!({"layers": [{"id": "a", "type": "rect", "width": 20, "height": 20, "x": 50, "fill": "#ffffff", "effects": [{"type": "radialBlur", "amount": 40}]}]}), 0.0);
    assert!(alpha(&zoom, 168, 50) > 0, "zoom streaks outward from the centre");
    assert_eq!(alpha(&zoom, 150, 70), 0, "but not sideways");
}

/// A big blur reaches past where the layer is cut by the canvas: content just outside shows.
#[test]
fn effects_reach_in_from_outside_the_canvas() {
    let p = render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "x": 125, "fill": "#ffffff", "effects": [{"type": "blur", "radius": 20}]}]}), 0.0);
    assert!(alpha(&p, 199, 50) > 20, "{}", alpha(&p, 199, 50));
}

#[test]
fn glow_shadow_and_outline() {
    let glow = square_with(json!([{"type": "glow", "radius": 16, "intensity": 1}]));
    assert!(alpha(&glow, 128, 50) > 10);
    let dark = render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#202020", "effects": [{"type": "glow", "radius": 16, "threshold": 0.5}]}]}), 0.0);
    assert_eq!(alpha(&dark, 128, 50), 0, "dark parts under the threshold don't glow");
    let tinted = render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#ffffff", "effects": [{"type": "glow", "radius": 16, "color": "#00ff00"}]}]}), 0.0);
    let g = rgba(&tinted, 126, 50);
    assert!(g[1] > g[0] && g[3] > 0, "{g:?}");

    let shadow = square_with(json!([{"type": "dropShadow", "distance": 10, "blur": 0, "color": "#000000ff"}]));
    // Light from the top left: the shadow falls bottom right.
    assert_eq!(rgba(&shadow, 125, 75), [0, 0, 0, 255]);
    assert_eq!(alpha(&shadow, 75, 25), 0);
    assert_eq!(rgba(&shadow, 100, 50), [255, 0, 0, 255], "the layer stays on top");

    let out = square_with(json!([{"type": "stroke", "width": 4, "color": "#00ff00"}]));
    assert_eq!(rgba(&out, 122, 50), [0, 255, 0, 255]);
    assert_eq!(rgba(&out, 118, 50), [255, 0, 0, 255]);
    assert_eq!(alpha(&out, 126, 50), 0);
    let inside = square_with(json!([{"type": "stroke", "width": 4, "color": "#00ff00", "position": "inside"}]));
    assert_eq!(rgba(&inside, 118, 50), [0, 255, 0, 255]);
    assert_eq!(alpha(&inside, 121, 50), 0);
    assert_eq!(rgba(&inside, 100, 50), [255, 0, 0, 255]);
    let centre = square_with(json!([{"type": "stroke", "width": 6, "color": "#00ff00", "position": "center"}]));
    assert_eq!(rgba(&centre, 118, 50), [0, 255, 0, 255]);
    assert_eq!(rgba(&centre, 121, 50), [0, 255, 0, 255]);
    // On text too.
    let text = render(json!({"layers": [{"id": "t", "type": "text", "text": "I", "fontSize": 60, "effects": [{"type": "stroke", "width": 3, "color": "#ff0000"}]}]}), 0.0);
    assert!(text.pixels().iter().any(|p| p.red() > 200 && p.green() < 50 && p.alpha() == 255));
}

#[test]
fn colour_effects() {
    let c = |fx: serde_json::Value| rgba(&square_with(fx), 100, 50);
    assert!(close(c(json!([{"type": "colorCorrect", "saturation": -1}])), [54, 54, 54, 255], 2), "{:?}", c(json!([{"type": "colorCorrect", "saturation": -1}])));
    assert_eq!(c(json!([{"type": "invert"}])), [0, 255, 255, 255]);
    assert_eq!(c(json!([{"type": "fill", "color": "#0000ff"}])), [0, 0, 255, 255]);
    assert_eq!(c(json!([{"type": "threshold", "level": 0.5}])), [0, 0, 0, 255]);
    assert_eq!(c(json!([{"type": "threshold", "level": 0.1}])), [255, 255, 255, 255]);
    assert_eq!(c(json!([{"type": "tint", "black": "#000000", "white": "#00ff00"}]))[0], 0);
    assert!(c(json!([{"type": "colorCorrect", "brightness": -0.5}]))[0] < 140);
    assert!(close(c(json!([{"type": "colorCorrect", "hue": 120}])), [0, 255, 0, 255], 2));
    assert!(close(c(json!([{"type": "levels", "outWhite": 0.5}])), [128, 0, 0, 255], 1));
    let tri = c(json!([{"type": "tritone", "shadows": "#000000", "midtones": "#0000ff", "highlights": "#ffffff"}]));
    assert!(tri[2] > tri[0]);
    let grey = render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#646464", "effects": [{"type": "posterize", "levels": 2}]}]}), 0.0);
    assert_eq!(rgba(&grey, 100, 50), [0, 0, 0, 255]);
    let ramp = render(json!({"layers": [{"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#ffffff", "effects": [{"type": "gradientRamp", "from": [-100, 0], "to": [100, 0], "colorFrom": "#000000", "colorTo": "#ffffff"}]}]}), 0.0);
    assert!(rgba(&ramp, 10, 50)[0] < 30 && rgba(&ramp, 190, 50)[0] > 225 && rgba(&ramp, 100, 50)[0].abs_diff(128) < 6);
    let vig = render(json!({"layers": [{"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#ffffff", "effects": [{"type": "vignette", "amount": 1, "size": 0.3, "softness": 0.3}]}]}), 0.0);
    assert_eq!(rgba(&vig, 100, 50), [255, 255, 255, 255]);
    assert!(rgba(&vig, 2, 2)[0] < 50, "corners darken");
}

#[test]
fn generators() {
    let grain = |t: f64| render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#808080", "effects": [{"type": "noise", "amount": 0.5}]}]}), t);
    let (a, b) = (grain(0.0), grain(0.5));
    let vals: Vec<u8> = (85..115).map(|x| rgba(&a, x, 50)[0]).collect();
    assert!(vals.iter().max().unwrap() - vals.iter().min().unwrap() > 40, "grainy");
    assert_ne!(a.data(), b.data(), "new grain every frame");
    assert_eq!(a.data(), grain(0.0).data(), "the same frame is the same");
    let smoke = |evo: f64| render(json!({"layers": [{"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#ffffff", "effects": [{"type": "fractalNoise", "scale": 40, "evolution": evo}]}]}), 0.0);
    let s = smoke(0.0);
    let vals: Vec<u8> = (0..200).map(|x| rgba(&s, x, 50)[0]).collect();
    assert!(vals.iter().max().unwrap() - vals.iter().min().unwrap() > 60, "cloudy");
    assert_ne!(s.data(), smoke(1.0).data(), "evolves");
    let dots = render(json!({"layers": [{"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#808080", "effects": [{"type": "halftone", "size": 12, "angle": 0}]}]}), 0.0);
    let blacks = dots.pixels().iter().filter(|p| p.red() < 30).count();
    let whites = dots.pixels().iter().filter(|p| p.red() > 225).count();
    assert!(blacks > 1000 && whites > 1000, "{blacks} {whites}");
    let lines = render(json!({"layers": [{"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#ffffff", "effects": [{"type": "scanlines", "spacing": 10, "amount": 1}]}]}), 0.0);
    let col: Vec<u8> = (0..20).map(|y| rgba(&lines, 100, y)[0]).collect();
    assert!(col.iter().max().unwrap() - col.iter().min().unwrap() > 200, "{col:?}");
}

#[test]
fn distortions() {
    let wide = render(json!({"layers": [{"id": "a", "type": "rect", "width": 60, "height": 40, "fill": "#ffffff", "effects": [{"type": "turbulentDisplace", "amount": 15, "size": 20}]}]}), 0.0);
    let plain = render(json!({"layers": [{"id": "a", "type": "rect", "width": 60, "height": 40, "fill": "#ffffff"}]}), 0.0);
    assert_ne!(wide.data(), plain.data());
    let wave = |t: f64| render(json!({"layers": [{"id": "a", "type": "rect", "width": 160, "height": 20, "fill": "#ffffff", "effects": [{"type": "waveWarp", "height": 10, "width": 40}]}]}), t);
    assert_ne!(wave(0.0).data(), wave(0.25).data(), "waves move with time");
    assert!((0..100).any(|y| alpha(&wave(0.1), 100, y) > 0 && !(40..60).contains(&y)), "waves reach out of the band");
    let ripple = render(json!({"layers": [{"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#ffffff", "effects": [{"type": "ripple", "amplitude": 6}, {"type": "invert"}]}]}), 0.0);
    assert!(ripple.width() == 200);
    // A twirl turns the bar near the centre.
    let bar = json!({"id": "a", "type": "rect", "width": 160, "height": 10, "fill": "#ffffff"});
    let mut tw = bar.clone();
    tw["effects"] = json!([{"type": "twirl", "angle": 90, "radius": 60}]);
    let twirled = render(json!({"layers": [tw]}), 0.0);
    assert!((0..100).any(|y| !(44..56).contains(&y) && alpha(&twirled, 110, y) > 128));
    let mut bu = bar.clone();
    bu["effects"] = json!([{"type": "bulge", "amount": 2, "radius": 40}]);
    let bulged = render(json!({"layers": [bu]}), 0.0);
    assert!(alpha(&bulged, 100, 42) > 128, "magnified in the middle");
    let mosaic = render(json!({"layers": [
        {"id": "a", "type": "rect", "width": 200, "height": 100, "fill": {"type": "linear", "stops": [[0, "#000000"], [1, "#ffffff"]]}, "effects": [{"type": "mosaic", "size": 20}]}
    ]}), 0.0);
    assert_eq!(rgba(&mosaic, 101, 41), rgba(&mosaic, 118, 58), "one block, one colour");
    assert_ne!(rgba(&mosaic, 101, 41), rgba(&mosaic, 121, 41));
    let ca = square_with(json!([{"type": "chromaticAberration", "amount": 4}]));
    assert!(rgba(&ca, 122, 50)[0] > 200, "red pushed right");
    let mirror = render(json!({"layers": [{"id": "a", "type": "rect", "width": 20, "height": 20, "x": 50, "fill": "#ffffff", "effects": [{"type": "mirror"}]}]}), 0.0);
    assert_eq!(alpha(&mirror, 50, 50), 255, "reflected to the left");
    let kal = render(json!({"layers": [{"id": "a", "type": "rect", "width": 10, "height": 10, "x": 30, "y": 5, "fill": "#ffffff", "effects": [{"type": "kaleidoscope", "segments": 4}]}]}), 0.0);
    assert!(kal.pixels().iter().filter(|p| p.alpha() > 128).count() > 150, "copies around the centre");
    let tile = render(json!({"layers": [{"id": "a", "type": "rect", "width": 20, "height": 20, "fill": "#ffffff", "effects": [{"type": "motionTile", "width": 50, "height": 50}]}]}), 0.0);
    assert_eq!(alpha(&tile, 150, 50), 255, "repeated a tile over");
    assert_eq!(alpha(&tile, 125, 50), 0);
    let pin = square_with(json!([{"type": "cornerPin", "topRight": [0, -20]}]));
    assert!(alpha(&pin, 117, 25) > 128, "top right pulled up");
    assert_eq!(alpha(&pin, 83, 25), 0, "top left stays");
    assert_eq!(rgba(&pin, 100, 50)[0], 255);
    let sharp = render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#808080", "effects": [{"type": "blur", "radius": 4}, {"type": "sharpen", "amount": 3}]}]}), 0.0);
    let soft = render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#808080", "effects": [{"type": "blur", "radius": 4}]}]}), 0.0);
    assert_ne!(sharp.data(), soft.data());
    let glitch = |t: f64| render(json!({"layers": [{"id": "a", "type": "rect", "width": 160, "height": 80, "fill": "#ffffff", "effects": [{"type": "glitch", "amount": 1, "speed": 10}]}]}), t);
    let plain = render(json!({"layers": [{"id": "a", "type": "rect", "width": 160, "height": 80, "fill": "#ffffff"}]}), 0.0);
    assert!((0..10).any(|i| glitch(i as f64 * 0.1).data() != plain.data()), "glitches in bursts");
}

#[test]
fn echoes_trail_behind() {
    let scene = json!({"layers": [{"id": "a", "type": "rect", "width": 10, "height": 10, "fill": "#ffffff",
        "keyframes": {"x": [[0, -80], [1, 80]]}, "effects": [{"type": "echo", "count": 3, "delay": 0.1, "decay": 0.8}]}]});
    let p = render(scene, 0.5);
    // Now at x = 0; 0.1 s earlier at −16, 0.2 s at −32.
    assert_eq!(alpha(&p, 100, 50), 255);
    assert!(alpha(&p, 84, 50) > 150, "{}", alpha(&p, 84, 50));
    assert!(alpha(&p, 68, 50) > 100 && alpha(&p, 68, 50) < alpha(&p, 84, 50));
    assert_eq!(alpha(&p, 116, 50), 0, "nothing ahead");
}

/// Every effect with every number at its lowest, highest and odd values draws without panicking.
#[test]
fn every_effect_survives_odd_values() {
    for spec in EFFECTS {
        for value in [0.0, -1.0, 1e9, -1e9, 0.5, f64::MAX] {
            let mut fx = serde_json::Map::new();
            fx.insert("type".into(), json!(spec.name));
            for p in spec.params {
                if matches!(p.kind, ParamKind::Number { .. } | ParamKind::Int { .. }) {
                    fx.insert(p.name.into(), json!(value));
                }
                if p.kind == ParamKind::Vec2 {
                    fx.insert(p.name.into(), json!([value, -value]));
                }
            }
            let scene = scene(json!({"layers": [
                {"id": "a", "type": "rect", "width": 30, "height": 20, "fill": "#ff8040", "keyframes": {"x": [[0, -20], [1, 20]]}, "effects": [fx]},
                {"id": "adj", "type": "adjustment", "effects": [fx]}
            ]}));
            let p = draw_on(&scene, 0.5, 64, 32, Quality::Final);
            assert!(p.pixels().iter().all(|c| c.red() <= c.alpha() && c.green() <= c.alpha() && c.blue() <= c.alpha()), "{} at {value}", spec.name);
        }
    }
}

// ---- layers ------------------------------------------------------------------------------

#[test]
fn adjustment_layers_change_what_is_below() {
    let p = render(json!({"background": "#000000", "layers": [
        {"id": "below", "type": "rect", "width": 40, "height": 40, "x": -50, "fill": "#ff0000"},
        {"id": "adj", "type": "adjustment", "effects": [{"type": "invert"}]},
        {"id": "above", "type": "rect", "width": 40, "height": 40, "x": 50, "fill": "#ff0000"}
    ]}), 0.0);
    assert_eq!(rgba(&p, 50, 50), [0, 255, 255, 255], "below: inverted");
    assert_eq!(rgba(&p, 150, 50), [255, 0, 0, 255], "above: untouched");
    assert_eq!(rgba(&p, 100, 10), [255, 255, 255, 255], "the background is below too");
    // Within its masks, and as much as its opacity.
    let p = render(json!({"background": "#000000", "layers": [
        {"id": "adj", "type": "adjustment", "opacity": 0.5, "effects": [{"type": "invert"}], "masks": [{"type": "rect", "size": [100, 100]}]}
    ]}), 0.0);
    assert!(rgba(&p, 100, 50)[0].abs_diff(128) <= 1);
    assert_eq!(rgba(&p, 10, 50), [0, 0, 0, 255]);
    // In a group, only the group's layers below it.
    let p = render(json!({"background": "#000000", "layers": [
        {"id": "outside", "type": "rect", "width": 40, "height": 40, "x": -50, "fill": "#ff0000"},
        {"id": "g", "type": "group", "layers": [
            {"id": "inside", "type": "rect", "width": 40, "height": 40, "x": 50, "fill": "#ff0000"},
            {"id": "adj", "type": "adjustment", "effects": [{"type": "invert"}]}
        ]}
    ]}), 0.0);
    assert_eq!(rgba(&p, 50, 50), [255, 0, 0, 255]);
    assert_eq!(rgba(&p, 150, 50), [0, 255, 255, 255]);
}

#[test]
fn parents_carry_their_children() {
    let s = scene(json!({"layers": [
        {"id": "root", "type": "null", "x": 40, "scale": 2},
        {"id": "mid", "type": "null", "parent": "root", "x": 10, "rotation": 90},
        {"id": "kid", "type": "rect", "width": 4, "height": 4, "fill": "#ffffff", "parent": "mid", "x": 10, "opacity": 0.5}
    ]}));
    let p = draw_on(&s, 0.0, 200, 100, Quality::Final);
    // root (40, 0) × 2 → mid at (60, 0), turned 90° → kid 10 along mid's x = down 20 (scaled).
    assert!(alpha(&p, 160, 70) > 100, "kid at (60, 20) from the centre");
    assert_eq!(alpha(&p, 100 + 60, 50), 0);
    assert!(alpha(&p, 160, 70) < 140, "opacity isn't inherited, the kid's own is kept");
    let m = layer_transform(&s, 0.0, None, "kid").unwrap();
    assert!((m[4] - 60.0).abs() < 1e-3 && (m[5] - 20.0).abs() < 1e-3, "{m:?}");
    let b = layer_bounds(&s, 0.0, None, "kid").unwrap();
    // 4×4 at scale 2, turned: from (56, 16) to (64, 24).
    let xs: Vec<f64> = b.iter().map(|c| c[0]).collect();
    assert!((xs.iter().cloned().fold(f64::MAX, f64::min) - 56.0).abs() < 1e-3);
    assert_eq!(hit_test(&s, 0.0, None, [60.0, 20.0]).as_deref(), Some("kid"));
    assert_eq!(hit_test(&s, 0.0, None, [0.0, 0.0]), None, "nulls aren't hit");
}

#[test]
fn compositions_have_their_own_time() {
    let base = |comp_layer: serde_json::Value| {
        json!({"compositions": [{"id": "c", "width": 200, "height": 100, "duration": 1, "layers": [
            {"id": "dot", "type": "rect", "width": 10, "height": 10, "fill": "#ffffff", "keyframes": {"x": [[0, -50], [1, 50]]}}
        ]}], "layers": [comp_layer]})
    };
    let x_of = |p: &Pixmap| span(p, 50).map(|(a, b)| (a + b + 1) as f64 / 2.0 - 100.0);
    // Plain: comp time = scene time − start.
    let plain = base(json!({"id": "l", "type": "comp", "comp": "c", "start": 0.25}));
    assert_eq!(x_of(&render(plain.clone(), 0.75)).map(f64::round), Some(0.0));
    assert_eq!(x_of(&render(plain.clone(), 0.1)), None, "not on before its start");
    // Speed and offset.
    let fast = base(json!({"id": "l", "type": "comp", "comp": "c", "speed": 2, "offset": 0.1}));
    assert_eq!(x_of(&render(fast, 0.2)).map(f64::round), Some(0.0));
    // Time remap.
    let remap = base(json!({"id": "l", "type": "comp", "comp": "c", "time": 0.75}));
    assert_eq!(x_of(&render(remap.clone(), 0.0)).map(f64::round), Some(25.0));
    assert_eq!(x_of(&render(remap, 0.9)).map(f64::round), Some(25.0));
    // Loop: 2.25 s into a 1 s composition shows 0.25.
    let looped = base(json!({"id": "l", "type": "comp", "comp": "c", "loop": true}));
    assert_eq!(x_of(&render(looped, 2.25)).map(f64::round), Some(-25.0));
    // Placed like any layer: moved and scaled, cut to its canvas, with effects.
    let placed = base(json!({"id": "l", "type": "comp", "comp": "c", "x": 20, "scale": 0.5, "time": 0.5, "effects": [{"type": "fill", "color": "#ff0000"}]}));
    let p = render(placed, 0.0);
    assert_eq!(rgba(&p, 120, 50), [255, 0, 0, 255]);
    let w = span(&p, 50).map(|(a, b)| b - a + 1).unwrap();
    assert!((4..=6).contains(&w), "half its 10 px: {w}");
    // Nested compositions.
    let nested = json!({"compositions": [
        {"id": "inner", "layers": [{"id": "dot", "type": "rect", "width": 10, "height": 10, "fill": "#00ff00"}]},
        {"id": "outer", "layers": [{"id": "i", "type": "comp", "comp": "inner", "x": 30}]}
    ], "layers": [{"id": "o", "type": "comp", "comp": "outer", "x": -60}]});
    assert_eq!(rgba(&render(nested, 0.0), 70, 50), [0, 255, 0, 255]);
}

#[test]
fn repeaters_draw_copies() {
    let p = render(json!({"layers": [{"id": "a", "type": "rect", "width": 10, "height": 10, "x": -60, "fill": "#ffffff",
        "operators": [{"type": "repeater", "copies": 3, "position": [50, 0], "startOpacity": 1, "endOpacity": 0.5}]}]}), 0.0);
    assert_eq!(alpha(&p, 40, 50), 255);
    assert_eq!(alpha(&p, 90, 50), 191);
    assert_eq!(alpha(&p, 140, 50), 128);
    assert_eq!(alpha(&p, 65, 50), 0);
    // Groups repeat too, turning around an anchor.
    let p = render(json!({"layers": [{"id": "g", "type": "group", "operators": [{"type": "repeater", "copies": 4, "position": [0, 0], "rotation": 90}],
        "layers": [{"id": "a", "type": "rect", "width": 10, "height": 10, "x": 30, "fill": "#ffffff"}]}]}), 0.0);
    for (x, y) in [(130, 50), (100, 80), (70, 50), (100, 20)] {
        assert_eq!(alpha(&p, x, y), 255, "copy at {x},{y}");
    }
}

#[test]
fn shape_operators_reshape_before_fill_and_stroke() {
    let base = |ops: serde_json::Value| render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "fill": "#ffffff", "operators": ops}]}), 0.0);
    let plain = base(json!([]));
    assert_eq!(alpha(&plain, 124, 50), 0);
    assert_eq!(alpha(&base(json!([{"type": "offset", "amount": 8}])), 124, 50), 255);
    assert_eq!(alpha(&base(json!([{"type": "offset", "amount": -8}])), 115, 50), 0);
    assert_ne!(base(json!([{"type": "zigzag", "size": 6, "ridges": 4}])).data(), plain.data());
    assert_ne!(base(json!([{"type": "wiggle", "size": 5}])).data(), plain.data());
    assert_eq!(alpha(&base(json!([{"type": "roundCorners", "radius": 15}])), 119, 31), 0, "corner rounded off");
    assert_ne!(base(json!([{"type": "twist", "angle": 120}])).data(), plain.data());
    assert_eq!(alpha(&base(json!([{"type": "puckerBloat", "amount": 60}])), 124, 50), 255, "edges bloated out");
    // Trim paths run on the changed outline.
    let trimmed = render(json!({"layers": [{"id": "a", "type": "rect", "width": 40, "height": 40, "stroke": {"color": "#ffffff", "width": 2}, "trimEnd": 0.5,
        "operators": [{"type": "offset", "amount": 10}]}]}), 0.0);
    assert!(trimmed.pixels().iter().any(|p| p.alpha() > 0));
    assert!(alpha(&trimmed, 130, 30).max(alpha(&trimmed, 70, 70)) < 255 || alpha(&trimmed, 70, 70) == 0);
}

#[test]
fn text_animators_move_selected_letters() {
    let base = json!({"id": "t", "type": "text", "text": "IIII", "fontSize": 40, "fill": "#ffffff", "letterSpacing": 10});
    let mut moved = base.clone();
    moved["animators"] = json!([{"type": "range", "start": 0, "end": 25, "y": -30, "fill": "#ff0000"}]);
    let a = render(json!({"layers": [base]}), 0.0);
    let b = render(json!({"layers": [moved]}), 0.0);
    // Columns with ink: the first letter's moved up and turned red, the others stayed.
    let cols = |p: &Pixmap| -> Vec<u32> { (0..200).filter(|x| (0..100).any(|y| alpha(p, *x, y) > 128)).collect() };
    let first = cols(&a)[0];
    let top = |p: &Pixmap, x: u32| (0..100).find(|y| alpha(p, x, *y) > 128);
    let last = *cols(&a).last().unwrap();
    assert!(top(&b, first + 1).unwrap() + 25 < top(&a, first + 1).unwrap(), "first letter up");
    assert_eq!(top(&b, last - 1), top(&a, last - 1), "last letter stays");
    let y = top(&b, first + 1).unwrap() + 5;
    assert!(rgba(&b, first + 1, y)[0] > 200 && rgba(&b, first + 1, y)[1] < 60, "first letter red");
    // Opacity by word, offset sweeping the range.
    let words = json!({"layers": [{"id": "t", "type": "text", "text": "AA BB", "fontSize": 40,
        "animators": [{"type": "range", "by": "word", "start": 0, "end": 50, "opacity": 0}]}]});
    let p = render(words, 0.0);
    let inked = (0..200).filter(|x| (0..100).any(|y| alpha(&p, *x, y) > 128)).collect::<Vec<_>>();
    assert!(*inked.first().unwrap() > 100, "the first word is gone");
    // Wiggly letters move with time.
    let wig = |t: f64| render(json!({"layers": [{"id": "t", "type": "text", "text": "ABC", "fontSize": 40, "animators": [{"type": "wiggly", "y": 20, "speed": 3}]}]}), t);
    assert_ne!(wig(0.0).data(), wig(0.4).data());
    // Tracking spreads the letters.
    let wide = render(json!({"layers": [{"id": "t", "type": "text", "text": "IIII", "fontSize": 40, "animators": [{"type": "range", "tracking": 20}]}]}), 0.0);
    let narrow = cols(&render(json!({"layers": [{"id": "t", "type": "text", "text": "IIII", "fontSize": 40}]}), 0.0));
    let spread = cols(&wide);
    assert!(spread.last().unwrap() - spread[0] > narrow.last().unwrap() - narrow[0] + 40);
    // Blur per letter.
    let blur = render(json!({"layers": [{"id": "t", "type": "text", "text": "I", "fontSize": 60, "animators": [{"type": "range", "blur": 6}]}]}), 0.0);
    assert!(blur.pixels().iter().any(|p| p.alpha() > 10 && p.alpha() < 200));
}

#[test]
fn text_follows_a_path() {
    let ring = "M0 -35 A35 35 0 1 1 0 35 A35 35 0 1 1 0 -35 Z";
    // Left-aligned: from the path's start (the top) clockwise round the right to the bottom.
    let p = render(json!({"layers": [{"id": "t", "type": "text", "text": "OOOOOOOOOO", "fontSize": 14, "align": "left", "path": ring}]}), 0.0);
    dump("text-path", &p);
    // Ink around the ring: right of centre, below it, and none in the middle.
    let ink_near = |x: i32, y: i32| (-8..=8).any(|dy| (-8..=8).any(|dx| alpha(&p, (100 + x + dx) as u32, (50 + y + dy) as u32) > 128));
    assert!(ink_near(35, 0) || ink_near(30, 15), "right side");
    assert!(ink_near(0, 35) || ink_near(-15, 30) || ink_near(15, 30), "bottom");
    assert!(!ink_near(0, 0), "nothing in the middle");
    assert!(!ink_near(-35, 0), "it hasn't come round to the left yet");
    // pathOffset slides it along.
    let q = render(json!({"layers": [{"id": "t", "type": "text", "text": "OO", "fontSize": 14, "path": ring, "pathOffset": 0.5}]}), 0.0);
    let r = render(json!({"layers": [{"id": "t", "type": "text", "text": "OO", "fontSize": 14, "path": ring}]}), 0.0);
    assert_ne!(q.data(), r.data());
}

#[test]
fn particles_burst_and_trail() {
    let p = render(json!({"layers": [{"id": "p", "type": "particles", "x": 40, "burst": 40, "rate": 0, "speed": 0, "size": 6, "color": "#ffffff", "fadeIn": 0, "fadeOut": 0}]}), 0.5);
    assert_eq!(alpha(&p, 140, 50), 255, "a burst at the layer's position");
    assert_eq!(alpha(&p, 100, 50), 0);
    // Born along the emitter's path, they stay where they were born.
    let trail = render(json!({"layers": [{"id": "p", "type": "particles", "rate": 60, "speed": 0, "lifetime": 2, "size": 6, "color": "#ffffff", "fadeIn": 0, "fadeOut": 0,
        "keyframes": {"x": [[0, -80], [1, 80]]}}]}), 1.0);
    assert!(alpha(&trail, 20, 50) > 0 && alpha(&trail, 100, 50) > 0 && alpha(&trail, 178, 50) > 0, "a line of particles");
    // Parented: the parent's position moves the emitter.
    let parented = render(json!({"layers": [{"id": "n", "type": "null", "x": -40},
        {"id": "p", "type": "particles", "parent": "n", "burst": 10, "rate": 0, "speed": 0, "size": 6, "shape": "square", "fadeIn": 0, "fadeOut": 0}]}), 0.2);
    assert_eq!(alpha(&parented, 60, 50), 255);
    for shape in ["circle", "square", "triangle", "star", "spark", "image"] {
        let mut l = json!({"id": "p", "type": "particles", "burst": 20, "rate": 10, "size": 8, "shape": shape});
        if shape == "image" {
            l["asset"] = json!("x");
        }
        let p = render(json!({"layers": [l]}), 0.3);
        assert!(p.pixels().iter().any(|c| c.alpha() > 0), "{shape}");
    }
}

#[test]
fn motion_blur_widens_moving_things() {
    let v = json!({"layers": [{"id": "a", "type": "rect", "width": 20, "height": 20, "fill": "#ffffff", "motionBlur": true,
        "keyframes": {"x": [[0, -60], [0.1, 60]]}}]});
    let s = scene(v);
    // 1200 px/s; a 180° shutter at 30 fps is 1/60 s: 20 px of travel.
    let sharp = draw_on(&s, 0.05, 200, 100, Quality::Preview);
    let blurred = draw_on(&s, 0.05, 200, 100, Quality::Final);
    let width = |p: &Pixmap| (0..200).filter(|x| alpha(p, *x, 50) > 0).count();
    assert_eq!(width(&sharp), 20);
    assert!(width(&blurred) >= 36, "{}", width(&blurred));
    assert!(alpha(&blurred, 100, 50) > 200, "solid in the middle");
    assert!(alpha(&blurred, 115, 50) > 0 && alpha(&blurred, 115, 50) < 200, "fading at the ends");
    // A still layer stays sharp.
    let still = scene(json!({"layers": [{"id": "a", "type": "rect", "width": 20, "height": 20, "fill": "#ffffff", "motionBlur": true}]}));
    assert_eq!(width(&draw_on(&still, 0.05, 200, 100, Quality::Final)), 20);
}

#[test]
fn new_blend_modes() {
    let over = |blend: &str| {
        let p = render(json!({"background": "#ff0000", "layers": [{"id": "a", "type": "rect", "width": 200, "height": 100, "fill": "#3366cc", "blend": blend}]}), 0.0);
        rgba(&p, 100, 50)
    };
    let normal = over("normal");
    assert_eq!(normal, [0x33, 0x66, 0xcc, 255]);
    // Exclusion: a + b − 2ab per channel.
    assert!(close(over("exclusion"), [0xcc, 0x66, 0xcc, 255], 2), "{:?}", over("exclusion"));
    for mode in ["hue", "saturation", "color", "luminosity"] {
        let c = over(mode);
        assert_ne!(c, normal, "{mode}");
        assert_ne!(c, [255, 0, 0, 255], "{mode}");
    }
    // Luminosity keeps the backdrop's hue: still red-ish.
    let l = over("luminosity");
    assert!(l[0] > l[1] && l[0] > l[2], "{l:?}");
    // Hue takes the layer's: blue-ish.
    let h = over("hue");
    assert!(h[2] > h[0], "{h:?}");
}

#[test]
fn studio_bounds_and_hits() {
    let s = scene(json!({"compositions": [{"id": "c", "width": 100, "height": 50, "layers": [{"id": "in", "type": "ellipse", "width": 20, "height": 20, "fill": "#fff"}]}],
        "layers": [
        {"id": "back", "type": "rect", "width": 200, "height": 100, "fill": "#000000"},
        {"id": "box", "type": "rect", "width": 40, "height": 20, "x": 30, "rotation": 90, "fill": "#ffffff"},
        {"id": "g", "type": "group", "x": -50, "layers": [{"id": "child", "type": "ellipse", "width": 20, "height": 20, "fill": "#ff0000"}]},
        {"id": "m", "type": "rect", "width": 200, "height": 100},
        {"id": "matted", "type": "rect", "width": 1, "height": 1, "x": 90, "matte": {"layer": "m"}, "fill": "#fff"},
        {"id": "hid", "type": "rect", "width": 10, "height": 10, "x": -90, "hidden": true, "fill": "#fff"}
    ]}));
    let b = layer_bounds(&s, 0.0, None, "box").unwrap();
    // 40×20 turned 90°: 20 wide, 40 tall, around (30, 0).
    let (xs, ys): (Vec<f64>, Vec<f64>) = b.iter().map(|c| (c[0], c[1])).unzip();
    let (x0, x1) = (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max));
    let (y0, y1) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
    assert!((x0 - 20.0).abs() < 1e-3 && (x1 - 40.0).abs() < 1e-3 && (y0 + 20.0).abs() < 1e-3 && (y1 - 20.0).abs() < 1e-3, "{b:?}");
    assert_eq!(hit_test(&s, 0.0, None, [30.0, 15.0]).as_deref(), Some("box"));
    assert_eq!(hit_test(&s, 0.0, None, [-50.0, 5.0]).as_deref(), Some("child"));
    assert_eq!(hit_test(&s, 0.0, None, [-62.0, 9.0]).as_deref(), Some("back"), "outside the circle, on the background");
    assert_eq!(hit_test(&s, 0.0, None, [-90.0, 0.0]).as_deref(), Some("back"), "hidden layers aren't hit");
    assert_eq!(hit_test(&s, 0.0, None, [500.0, 0.0]), None);
    let t = layer_transform(&s, 0.0, None, "child").unwrap();
    assert_eq!((t[4], t[5]), (-50.0, 0.0));
    assert_eq!(hit_test(&s, 0.0, Some("c"), [0.0, 0.0]).as_deref(), Some("in"));
    assert!(layer_bounds(&s, 0.0, Some("c"), "in").is_some());
    assert!(layer_bounds(&s, 0.0, None, "nope").is_none());
}
