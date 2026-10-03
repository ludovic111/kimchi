//! The standard 3D engine on known scenes: lights, worlds, materials, camera effects, particles,
//! models, the Studio's overlays and picking, and the GPU drawing what the CPU draws.
//! `KIMCHI_DUMP=dir` writes the pictures; `KIMCHI_GPU=any` runs the GPU parts on llvmpipe.

use std::collections::HashMap;

use super::viewport::{Shading, ViewCamera, ViewOptions};
use super::*;
use kimchi_core::Scene;
use serde_json::json;

struct None_;
impl Pictures for None_ {
    fn picture(&mut self, _: &str, _: f64) -> Option<Arc<Pixmap>> {
        None
    }
    fn path(&self, _: &str) -> Option<PathBuf> {
        None
    }
}

/// Pictures by name, and model files by name.
#[derive(Default)]
struct Pics {
    pictures: HashMap<String, Arc<Pixmap>>,
    files: HashMap<String, PathBuf>,
}
impl Pictures for Pics {
    fn picture(&mut self, asset: &str, _: f64) -> Option<Arc<Pixmap>> {
        self.pictures.get(asset).cloned()
    }
    fn path(&self, reference: &str) -> Option<PathBuf> {
        self.files.get(reference).cloned()
    }
}

fn scene(v: serde_json::Value) -> Scene3d {
    let Scene::Space(s) = Scene::from_json(&v).unwrap() else { panic!("3d") };
    s
}

fn dump(name: &str, p: &Pixmap) {
    if let Some(dir) = std::env::var_os("KIMCHI_DUMP") {
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        p.save_png(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn draw(v: serde_json::Value, t: f64) -> Pixmap {
    Space::cpu().render(&scene(v), t, 160, 90, &mut None_, Quality::Preview).unwrap()
}

fn draw_q(v: serde_json::Value, quality: Quality, pics: &mut dyn Pictures) -> Pixmap {
    Space::cpu().render(&scene(v), 0.0, 160, 90, pics, quality).unwrap()
}

fn rgba(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = p.pixel(x, y).unwrap().demultiply();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

/// Mean brightness (0–255) of a box of pixels.
fn mean(p: &Pixmap, x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let c = p.pixel(x, y).unwrap();
            sum += (c.red() as f64 + c.green() as f64 + c.blue() as f64) / 3.0;
            n += 1.0;
        }
    }
    sum / n
}

fn mean_diff(a: &Pixmap, b: &Pixmap) -> f64 {
    a.pixels().iter().zip(b.pixels()).map(|(a, b)| (a.red().abs_diff(b.red()) as f64 + a.green().abs_diff(b.green()) as f64 + a.blue().abs_diff(b.blue()) as f64) / 3.0).sum::<f64>()
        / a.pixels().len() as f64
}

/// A floor seen from above-front, lit by `light`.
fn floor_lit_by(light: serde_json::Value) -> serde_json::Value {
    json!({"background": "#000000", "ambient": 0, "fog": false, "camera": {"position": [0, 6, 6], "target": [0, 0, 0]},
        "lights": [light],
        "objects": [{"id": "floor", "type": "plane", "width": 12, "height": 12, "rotation": [-90, 0, 0], "material": {"color": "#ffffff", "roughness": 1}}]})
}

// ---- What earlier versions already drew ----

#[test]
fn draws_a_lit_box_in_front_of_the_background() {
    let p = draw(json!({"background": "#000000", "camera": {"position": [0, 0, 5]},
        "objects": [{"id": "b", "type": "box", "size": 1.5, "rotation": [20, 30, 0], "material": {"color": "#ff0000"}}]}), 0.0);
    let mid = rgba(&p, 80, 45);
    assert!(mid[0] > 60 && mid[1] < 40 && mid[2] < 40, "red box in the middle: {mid:?}");
    assert_eq!(rgba(&p, 2, 2), [0, 0, 0, 255], "background in the corner");
}

#[test]
fn transparent_background_and_unlit_colour() {
    let p = draw(json!({"camera": {"position": [0, 0, 3]},
        "objects": [{"id": "card", "type": "plane", "width": 1, "height": 1, "material": {"color": "#00ff00", "unlit": true}}]}), 0.0);
    assert_eq!(rgba(&p, 80, 45), [0, 255, 0, 255]);
    assert_eq!(rgba(&p, 1, 1)[3], 0);
}

#[test]
fn keyframes_move_objects() {
    let scene = json!({"background": "#000000", "camera": {"position": [0, 0, 6]},
        "objects": [{"id": "s", "type": "sphere", "radius": 0.5, "material": {"color": "#ffffff", "unlit": true},
                      "keyframes": {"x": [[0, -2], [1, 2]]}}]});
    let (a, b) = (draw(scene.clone(), 0.0), draw(scene, 1.0));
    let x_of = |p: &Pixmap| (0..160).filter(|x| rgba(p, *x, 45)[0] > 128).sum::<u32>() as f32 / (0..160).filter(|x| rgba(p, *x, 45)[0] > 128).count().max(1) as f32;
    assert!(x_of(&a) < 60.0 && x_of(&b) > 100.0, "{} → {}", x_of(&a), x_of(&b));
}

#[test]
fn objects_cast_shadows_on_a_floor() {
    let scene = |shadows: bool| {
        json!({"background": "#000000", "shadows": shadows, "camera": {"position": [0, 6, 0.01], "target": [0, 0, 0]},
            "lights": [{"id": "sun", "type": "directional", "direction": [0, -1, 0]}],
            "objects": [
                {"id": "floor", "type": "plane", "width": 10, "height": 10, "rotation": [-90, 0, 0], "material": {"color": "#ffffff"}},
                {"id": "box", "type": "box", "size": [1, 0.2, 1], "position": [0, 1, 0], "material": {"color": "#ffffff"}}
            ]})
    };
    let lit = draw(scene(true), 0.0);
    let corner = rgba(&lit, 20, 10);
    assert!(corner[0] > 100, "floor lit: {corner:?}");
    let side = |shadows: bool| {
        let mut s = scene(shadows);
        s["camera"] = json!({"position": [0, 3, 6], "target": [0, 0, 0]});
        s["objects"][1]["position"] = json!([0, 1.5, 0]);
        draw(s, 0.0)
    };
    let (on, off) = (side(true), side(false));
    dump("shadow-on", &on);
    let under = |p: &Pixmap| (72..88).flat_map(|x| (40..50).map(move |y| (x, y))).map(|(x, y)| rgba(p, x, y)[0] as u64).sum::<u64>();
    assert!(under(&on) < under(&off) * 3 / 4, "shadow darkens the floor: {} vs {}", under(&on), under(&off));
}

#[test]
fn far_floor_past_the_suns_box_is_lit() {
    // A floor running to the horizon under a low sun: past the sun's shadow box it used to be
    // darkened by the box's edge, a band across the distance.
    let v = |shadows: bool| {
        json!({"background": "#000000", "fog": false, "shadows": shadows,
            "camera": {"position": [0.5, 1.9, 5.2], "target": [0, 0.5, 0], "fov": 38},
            "lights": [{"id": "sun", "type": "directional", "direction": [-0.6, -0.55, -0.5], "intensity": 2}],
            "objects": [{"id": "floor", "type": "plane", "width": 400, "height": 400, "rotation": [-90, 0, 0], "material": {"color": "#808080", "roughness": 1}},
                        {"id": "b", "type": "box", "size": 1, "position": [0, 0.5, 0]}]})
    };
    let draw = |s: bool| Space::cpu().render(&scene(v(s)), 0.0, 320, 180, &mut None_, Quality::Preview).unwrap();
    let (on, off) = (draw(true), draw(false));
    dump("far-floor", &on);
    // The far floor: the rows just under the horizon.
    let (a, b) = (mean(&on, 0, 14, 320, 34), mean(&off, 0, 14, 320, 34));
    assert!((a - b).abs() < 3.0, "far floor {a:.1} with shadows, {b:.1} without");
}

#[test]
fn small_overhangs_shadow_in_big_scenes() {
    // A thin ledge sticking out a tenth of a unit from a wall, in a scene with a big floor (the
    // sun's shadow map covers many units): the band of wall under it is in shadow, evenly.
    let v = |shadows: bool| {
        json!({"background": "#000000", "shadows": shadows, "fog": false,
            "camera": {"position": [0, 1.44, 3], "target": [0, 1.44, 0], "fov": 20},
            "lights": [{"id": "sun", "type": "directional", "direction": [0, -1, -1]}],
            "objects": [
                {"id": "floor", "type": "plane", "width": 30, "height": 30, "rotation": [-90, 0, 0]},
                {"id": "wall", "type": "plane", "width": 2, "height": 2, "position": [0, 1, 0], "material": {"color": "#ffffff", "roughness": 1}},
                {"id": "ledge", "type": "box", "size": [2, 0.02, 0.1], "position": [0, 1.5, 0.05], "material": {"color": "#ffffff", "roughness": 1}}
            ]})
    };
    let draw = |s: bool| Space::cpu().render(&scene(v(s)), 0.0, 320, 180, &mut None_, Quality::Preview).unwrap();
    let (on, off) = (draw(true), draw(false));
    dump("ledge-shadow", &on);
    let band = mean(&on, 60, 86, 260, 95);
    let lit = mean(&on, 60, 120, 260, 170);
    assert!(mean(&off, 60, 86, 260, 95) > lit * 0.9, "without shadows the band is as lit as the wall below");
    assert!(band < lit * 0.6, "the ledge shadows the wall under it: {band:.0} vs {lit:.0}");
}

/// The standard look is unchanged: no exposure, standard tone mapping encode like before.
#[test]
fn standard_tone_mapping_is_the_shoulder() {
    let f = Space::cpu().frame(&scene(json!({"objects": []})), 0.0, 16, 16, &mut None_, Quality::Preview);
    for v in [0.0f32, 0.1, 0.5, 0.8, 0.95, 2.0] {
        let c = finish(&f, [v, v, v, 1.0], false, false);
        assert_eq!(c[0], linear_to_srgb(shoulder(v)));
        // Linear output comes back the same after post's encoding.
        let back = linear_to_srgb(post::tone(finish(&f, [v, v, v, 1.0], true, true)[0], false));
        assert!((back - linear_to_srgb(v.min(1.0))).abs() < 2e-3, "unlit {v}: {back}");
    }
}

// ---- Lights ----

#[test]
fn point_lights_fade_out_at_their_range() {
    let near = draw(floor_lit_by(json!({"id": "p", "type": "point", "position": [0, 1, 0], "range": 3})), 0.0);
    dump("point", &near);
    let centre = mean(&near, 75, 40, 85, 50);
    let edge = mean(&near, 0, 0, 10, 10);
    assert!(centre > 60.0 && edge < 5.0, "bright under the bulb ({centre}), dark beyond its range ({edge})");
}

#[test]
fn spot_lights_light_a_cone() {
    let p = draw(floor_lit_by(json!({"id": "s", "type": "spot", "position": [0, 4, 0], "direction": [0, -1, 0], "angle": 30, "blend": 0.3})), 0.0);
    dump("spot", &p);
    let inside = mean(&p, 76, 41, 84, 49);
    let outside = mean(&p, 10, 40, 30, 50);
    assert!(inside > 80.0 && outside < 3.0, "a pool of light: {inside} inside, {outside} outside");
    // A softer edge spreads the light further out.
    let soft = draw(floor_lit_by(json!({"id": "s", "type": "spot", "position": [0, 4, 0], "direction": [0, -1, 0], "angle": 30, "blend": 1})), 0.0);
    assert!(mean(&soft, 76, 41, 84, 49) <= inside + 1.0);
}

#[test]
fn area_lights_shine_from_their_front_only() {
    let front = draw(floor_lit_by(json!({"id": "a", "type": "area", "position": [0, 2, 0], "direction": [0, -1, 0], "size": [2, 2]})), 0.0);
    let back = draw(floor_lit_by(json!({"id": "a", "type": "area", "position": [0, 2, 0], "direction": [0, 1, 0], "size": [2, 2]})), 0.0);
    dump("area", &front);
    assert!(mean(&front, 70, 35, 90, 55) > 50.0, "lit below the panel");
    assert!(mean(&back, 70, 35, 90, 55) < 2.0, "dark behind it");
    // Bigger panels light a wider area.
    let big = draw(floor_lit_by(json!({"id": "a", "type": "area", "position": [0, 2, 0], "direction": [0, -1, 0], "size": [6, 6]})), 0.0);
    assert!(mean(&big, 20, 35, 40, 55) > mean(&front, 20, 35, 40, 55));
}

#[test]
fn spot_lights_cast_soft_shadows() {
    let scene = |size: f64| {
        json!({"background": "#000000", "ambient": 0, "fog": false, "camera": {"position": [0, 6, 6], "target": [0, 0, 0]},
            "lights": [{"id": "s", "type": "spot", "position": [0, 5, 0], "direction": [0, -1, 0], "angle": 80, "size": [size, size]}],
            "objects": [
                {"id": "floor", "type": "plane", "width": 12, "height": 12, "rotation": [-90, 0, 0], "material": {"color": "#ffffff", "roughness": 1}},
                {"id": "box", "type": "box", "size": [1.2, 0.2, 1.2], "position": [0, 2, 0], "material": {"color": "#ffffff"}}
            ]})
    };
    let hard = draw(scene(0.0), 0.0);
    let mut no = scene(0.0);
    no["lights"][0]["castShadows"] = json!(false);
    let unshadowed = draw(no, 0.0);
    dump("spot-shadow", &hard);
    // The floor beside the box, where its shadow falls.
    let row = |p: &Pixmap| (0..160).map(|x| rgba(p, x, 46)[0] as f64).collect::<Vec<_>>();
    let with = row(&hard);
    let darker = hard.pixels().iter().zip(unshadowed.pixels()).filter(|(a, b)| a.red() as i32 + 20 < b.red() as i32).count();
    assert!(darker > 5, "the box shadows the floor ({darker} pixels darker)");
    // A bigger bulb blurs the shadow's edge: fewer pixels change abruptly.
    let soft = row(&draw(scene(0.6), 0.0));
    let jumps = |r: &[f64]| r.windows(2).map(|w| (w[0] - w[1]).abs()).fold(0.0, f64::max);
    assert!(jumps(&soft) < jumps(&with), "soft edge {} vs hard {}", jumps(&soft), jumps(&with));
}

// ---- Worlds ----

fn sphere_in(env: serde_json::Value) -> serde_json::Value {
    json!({"camera": {"position": [0, 0, 4]}, "lights": [{"id": "off", "type": "point", "intensity": 0}], "environment": env,
        "objects": [{"id": "s", "type": "sphere", "radius": 1, "material": {"color": "#ffffff", "roughness": 1}}]})
}

#[test]
fn a_coloured_world_lights_matte_things_in_its_colour() {
    let p = draw(sphere_in(json!({"type": "color", "color": "#ff0000"})), 0.0);
    let c = rgba(&p, 80, 45);
    assert!(c[0] > 150 && c[1] < 30 && c[2] < 30, "{c:?}");
    assert_eq!(rgba(&p, 2, 2)[3], 0, "not visible: transparent behind");
    let shown = draw(sphere_in(json!({"type": "color", "color": "#ff0000", "visible": true})), 0.0);
    let bg = rgba(&shown, 2, 2);
    assert!(bg[3] == 255 && bg[0] > 200, "visible world behind: {bg:?}");
}

#[test]
fn gradients_light_from_above_and_skies_follow_the_sun() {
    let p = draw(sphere_in(json!({"type": "gradient", "top": "#ffffff", "horizon": "#808080", "bottom": "#000000", "visible": true})), 0.0);
    dump("gradient", &p);
    assert!(mean(&p, 75, 12, 85, 18) > mean(&p, 75, 72, 85, 78) + 30.0, "the top of the sphere is brighter");
    assert!(mean(&p, 0, 0, 160, 5) > mean(&p, 0, 85, 160, 90), "the world above is brighter behind");
    let sky = |dir: [f64; 3]| {
        let mut s = sphere_in(json!({"type": "sky", "visible": true}));
        s["lights"] = json!([{"id": "sun", "type": "directional", "direction": dir, "intensity": 0}]);
        draw(s, 0.0)
    };
    let (noon, dusk) = (sky([0.0, -1.0, -0.2]), sky([0.0, -0.02, -1.0]));
    dump("sky-noon", &noon);
    dump("sky-dusk", &dusk);
    let blue = |p: &Pixmap| {
        let c = rgba(p, 80, 2);
        c[2] as i32 - c[0] as i32
    };
    assert!(blue(&noon) > 20, "a blue sky at noon: {:?}", rgba(&noon, 80, 2));
    assert!(mean(&noon, 0, 0, 160, 10) > mean(&dusk, 0, 0, 160, 10), "darker at dusk");
}

#[test]
fn panoramas_light_reflect_and_show() {
    // A panorama: the top half bright blue, the bottom dark.
    let mut pano = Pixmap::new(64, 32).unwrap();
    for y in 0..32 {
        for x in 0..64 {
            let c = if y < 16 { tiny_skia::ColorU8::from_rgba(40, 90, 255, 255) } else { tiny_skia::ColorU8::from_rgba(20, 15, 10, 255) };
            pano.pixels_mut()[y * 64 + x] = c.premultiply();
        }
    }
    let mut pics = Pics::default();
    pics.pictures.insert("pano".into(), Arc::new(pano));
    let mut s = sphere_in(json!({"type": "image", "image": "pano", "visible": true}));
    s["objects"][0]["material"] = json!({"color": "#ffffff", "metallic": 1, "roughness": 0.05});
    let p = draw_q(s, Quality::Preview, &mut pics);
    dump("panorama", &p);
    let top = rgba(&p, 80, 20);
    assert!(top[2] > top[0] + 40, "the mirror reflects the blue above: {top:?}");
    assert!(rgba(&p, 2, 2)[2] > 150, "the panorama behind");
}

// ---- Materials ----

#[test]
fn glass_lets_the_background_through() {
    let glass = |transmission: f64| {
        let p = draw(json!({"background": "#ff0000", "fog": false, "camera": {"position": [0, 0, 4]},
            "objects": [{"id": "g", "type": "sphere", "radius": 1, "material": {"color": "#ffffff", "transmission": transmission, "roughness": 0.05}}]}), 0.0);
        rgba(&p, 80, 45)
    };
    let (opaque, clear) = (glass(0.0), glass(1.0));
    assert!(clear[0] as i32 - (clear[1] as i32) > opaque[0] as i32 - (opaque[1] as i32) + 40, "red shows through: {clear:?} vs {opaque:?}");
}

#[test]
fn clearcoat_adds_a_second_highlight() {
    let coat = |cc: f64| {
        draw(json!({"background": "#000000", "fog": false, "camera": {"position": [0, 0, 4]}, "lights": [{"id": "k", "type": "directional", "direction": [0, 0, -1]}],
            "objects": [{"id": "s", "type": "sphere", "radius": 1, "material": {"color": "#202060", "roughness": 0.8, "clearcoat": cc}}]}), 0.0)
    };
    let (plain, coated) = (coat(0.0), coat(1.0));
    let peak = |p: &Pixmap| (0..160).flat_map(|x| (0..90).map(move |y| (x, y))).map(|(x, y)| rgba(p, x, y)[0]).max().unwrap();
    assert!(peak(&coated) > peak(&plain) + 30, "a sharp varnish highlight: {} vs {}", peak(&coated), peak(&plain));
}

#[test]
fn patterns_show_both_colours_and_bumps_change_shading() {
    let plane = |pattern: serde_json::Value| {
        draw(json!({"background": "#000000", "fog": false, "camera": {"position": [0, 0, 3]}, "lights": [{"id": "k", "type": "directional", "direction": [-0.6, -0.3, -1]}],
            "objects": [{"id": "p", "type": "plane", "width": 2.4, "height": 2.4, "material": {"color": "#ffffff", "roughness": 0.7, "pattern": pattern}}]}), 0.0)
    };
    let p = plane(json!({"type": "checker", "color": "#ff0000", "color2": "#0000ff", "scale": 4}));
    dump("checker", &p);
    let (mut red, mut blue) = (0, 0);
    for x in 40..120 {
        for y in 10..80 {
            let c = rgba(&p, x, y);
            if c[0] as i32 > c[2] as i32 + 40 {
                red += 1;
            } else if c[2] as i32 > c[0] as i32 + 40 {
                blue += 1;
            }
        }
    }
    assert!(red > 500 && blue > 500, "{red} red, {blue} blue");
    let flat = plane(json!({"type": "bricks", "color": "#c0c0c0", "color2": "#c0c0c0"}));
    let bumpy = plane(json!({"type": "bricks", "color": "#c0c0c0", "color2": "#c0c0c0", "bump": 3}));
    dump("bricks-bump", &bumpy);
    assert!(mean_diff(&flat, &bumpy) > 1.0, "bumps change the shading: {}", mean_diff(&flat, &bumpy));
}

// ---- Cameras and camera effects ----

#[test]
fn orthographic_cameras_keep_sizes_with_distance() {
    let width_at = |z: f64, ortho: bool| {
        let cam = if ortho { json!({"position": [0, 0, z], "projection": "orthographic", "orthoSize": 4}) } else { json!({"position": [0, 0, z]}) };
        let p = draw(json!({"background": "#000000", "camera": cam, "objects": [{"id": "c", "type": "plane", "width": 1, "height": 1, "material": {"color": "#ffffff", "unlit": true}}]}), 0.0);
        (0..160).filter(|x| rgba(&p, *x, 45)[0] > 128).count()
    };
    let (near, far) = (width_at(3.0, true), width_at(30.0, true));
    assert!(near > 10 && (near as i64 - far as i64).abs() <= 1, "orthographic: {near} vs {far}");
    assert!(width_at(3.0, false) > width_at(30.0, false) * 3, "perspective shrinks with distance");
    // Near and far planes follow the scene: huge and tiny scenes both draw.
    let huge = draw(json!({"background": "#000000", "camera": {"position": [0, 0, 5000]}, "objects": [{"id": "c", "type": "sphere", "radius": 1000, "material": {"color": "#ffffff", "unlit": true}}]}), 0.0);
    assert!(rgba(&huge, 80, 45)[0] > 200);
    let tiny = draw(json!({"background": "#000000", "camera": {"position": [0, 0, 0.005]}, "objects": [{"id": "c", "type": "sphere", "radius": 0.001, "material": {"color": "#ffffff", "unlit": true}}]}), 0.0);
    assert!(rgba(&tiny, 80, 45)[0] > 200);
}

#[test]
fn camera_settings_reach_the_frame() {
    let s = scene(json!({"camera": {"position": [0, 0, 10], "fov": 40, "fStop": 2, "focusDistance": 6}, "objects": [{"id": "b", "type": "box"}]}));
    let f = Space::cpu().frame(&s, 0.0, 160, 90, &mut None_, Quality::Preview);
    assert_eq!(f.camera.focus, 6.0);
    // 36 mm sensor: a 40° vertical view at 16:9 is about a 27.8 mm lens; at f/2 its opening is
    // 13.9 mm, a radius of 6.95 mm.
    assert!((f.camera.aperture - 0.00695).abs() < 0.0003, "{}", f.camera.aperture);
    assert!(f.camera.near > 8.0 && f.camera.far < 12.0, "depth range hugs the box: {} – {}", f.camera.near, f.camera.far);
}

#[test]
fn exposure_and_filmic_change_the_picture() {
    let base = json!({"background": "#000000", "fog": false, "camera": {"position": [0, 0, 4]}, "objects": [{"id": "s", "type": "sphere", "radius": 1, "material": {"color": "#808080"}}]});
    let mut brighter = base.clone();
    brighter["render"] = json!({"exposure": 1});
    let mut filmic = base.clone();
    filmic["render"] = json!({"toneMapping": "filmic"});
    let (a, b, c) = (draw(base, 0.0), draw(brighter, 0.0), draw(filmic, 0.0));
    assert!(mean(&b, 70, 35, 90, 55) > mean(&a, 70, 35, 90, 55) + 15.0);
    assert!(mean_diff(&a, &c) > 1.0);
}

#[test]
fn bloom_glows_around_bright_things() {
    let scene = |bloom: f64| {
        json!({"background": "#000000", "fog": false, "camera": {"position": [0, 0, 5]}, "render": {"bloom": bloom, "bloomRadius": 0.1},
            "objects": [{"id": "s", "type": "sphere", "radius": 0.3, "material": {"color": "#000000", "emissive": "#ffffff", "emissiveIntensity": 6}}]})
    };
    let (off, on) = (draw(scene(0.0), 0.0), draw(scene(1.0), 0.0));
    dump("bloom", &on);
    // Beside the sphere (radius ~ 9 px), black without bloom, glowing with it.
    let beside = |p: &Pixmap| mean(p, 96, 42, 100, 48);
    assert!(beside(&off) < 2.0 && beside(&on) > 8.0, "{} → {}", beside(&off), beside(&on));
    assert!(rgba(&on, 80, 45)[0] > 240, "the bright thing stays bright");
}

#[test]
fn depth_of_field_blurs_what_is_out_of_focus() {
    // Stripes far behind a sharp card in focus.
    let scene = |f_stop: f64| {
        json!({"background": "#000000", "fog": false, "camera": {"position": [0, 0, 0.3], "target": [0, 0, 0], "fStop": f_stop, "focusDistance": 0.3},
            "objects": [
                {"id": "near", "type": "plane", "width": 0.03, "height": 0.03, "material": {"color": "#ffffff", "unlit": true}},
                {"id": "far", "type": "plane", "width": 80, "height": 80, "position": [0, 0, -40],
                 "material": {"color": "#ffffff", "unlit": true, "pattern": {"type": "stripes", "color": "#ffffff", "color2": "#000000", "scale": 40}}}
            ]})
    };
    let (sharp, blurry) = (draw(scene(0.0), 0.0), draw(scene(1.0), 0.0));
    dump("dof", &blurry);
    let contrast = |p: &Pixmap, y: u32| (0..40).map(|x| rgba(p, x, y)[0]).max().unwrap() as i32 - (0..40).map(|x| rgba(p, x, y)[0]).min().unwrap() as i32;
    assert!(contrast(&sharp, 20) > 150, "sharp stripes: {}", contrast(&sharp, 20));
    assert!(contrast(&blurry, 20) < contrast(&sharp, 20) / 2, "blurred stripes: {}", contrast(&blurry, 20));
    // The card in focus keeps a crisp edge.
    let edge = |p: &Pixmap| (60..100).map(|x| rgba(p, x, 45)[0] as i32).collect::<Vec<_>>().windows(2).map(|w| (w[0] - w[1]).abs()).max().unwrap();
    assert!(edge(&blurry) > 100, "the card stays sharp: {}", edge(&blurry));
}

#[test]
fn ambient_occlusion_darkens_corners() {
    let scene = |ao: f64| {
        json!({"background": "#000000", "fog": false, "ambient": 1, "lights": [{"id": "off", "type": "point", "intensity": 0}],
            "camera": {"position": [0, 1.5, 4], "target": [0, 0.5, 0]}, "render": {"ambientOcclusion": ao},
            "objects": [
                {"id": "floor", "type": "plane", "width": 8, "height": 8, "rotation": [-90, 0, 0], "material": {"color": "#ffffff", "roughness": 1}},
                {"id": "wall", "type": "plane", "width": 8, "height": 4, "position": [0, 2, -1], "material": {"color": "#ffffff", "roughness": 1}}
            ]})
    };
    let (off, on) = (draw(scene(0.0), 0.0), draw(scene(1.0), 0.0));
    dump("ao", &on);
    // Down the middle, the crease where the floor meets the wall gets darker.
    let drop = (20..80).map(|y| rgba(&off, 80, y)[0] as i32 - rgba(&on, 80, y)[0] as i32).max().unwrap();
    assert!(drop > 8, "the corner darkens by {drop}");
    assert!((mean(&on, 40, 85, 120, 90) - mean(&off, 40, 85, 120, 90)).abs() < 6.0, "open floor barely changes");
}

#[test]
fn motion_blur_smears_fast_things_in_final_frames() {
    let s = scene(json!({"background": "#000000", "camera": {"position": [0, 0, 6]}, "render": {"motionBlur": 1, "motionBlurSamples": 8},
        "objects": [{"id": "s", "type": "box", "size": 0.6, "material": {"color": "#ffffff", "unlit": true}, "keyframes": {"x": [[0, -3], [1, 3]]}}]}));
    let mut space = Space::cpu();
    let sharp = space.render_frame(&s, 0.5, 0.25, 160, 90, &mut None_, Quality::Preview).unwrap();
    let blurred = space.render_frame(&s, 0.5, 0.25, 160, 90, &mut None_, Quality::Final).unwrap();
    dump("motion-blur", &blurred);
    let partial = |p: &Pixmap| (0..160).filter(|x| (20..235).contains(&rgba(p, *x, 45)[0])).count();
    assert!(partial(&blurred) > partial(&sharp) + 10, "smeared: {} vs {}", partial(&blurred), partial(&sharp));
}

// ---- Particles and models ----

#[test]
fn particles_draw_in_every_shape() {
    for shape in ["sphere", "cube", "tetra", "spark"] {
        let p = draw(json!({"background": "#000000", "camera": {"position": [0, 1, 6], "target": [0, 1, 0]},
            "objects": [{"id": "e", "type": "particles", "burst": 60, "rate": 0, "size": 0.2, "speed": 2, "spread": 60, "shape": shape,
                         "material": {"color": "#ffffff", "emissive": "#ffffff"}}]}), 0.5);
        dump(&format!("particles-{shape}"), &p);
        let lit = p.pixels().iter().filter(|c| c.red() > 40).count();
        assert!(lit > 30, "{shape}: {lit} pixels");
    }
    // Picture cards.
    let mut pics = Pics::default();
    let mut dot = Pixmap::new(8, 8).unwrap();
    dot.fill(tiny_skia::Color::from_rgba8(0, 255, 0, 255));
    pics.pictures.insert("dot".into(), Arc::new(dot));
    let s = scene(json!({"background": "#000000", "camera": {"position": [0, 1, 6], "target": [0, 1, 0]},
        "objects": [{"id": "e", "type": "particles", "burst": 30, "rate": 0, "size": 0.3, "shape": "image", "asset": "dot"}]}));
    let p = Space::cpu().render(&s, 0.5, 160, 90, &mut pics, Quality::Preview).unwrap();
    assert!(p.pixels().iter().filter(|c| c.green() > 200 && c.red() < 40).count() > 30, "green cards");
}

#[test]
fn obj_and_stl_models_load_centred() {
    let dir = tempfile::tempdir().unwrap();
    let obj = dir.path().join("tri.obj");
    std::fs::write(&obj, "mtllib tri.mtl\nv 10 10 10\nv 14 10 10\nv 10 14 10\nusemtl r\nf 1 2 3\n").unwrap();
    std::fs::write(dir.path().join("tri.mtl"), "newmtl r\nKd 1 0 0\n").unwrap();
    let parts = mesh::model(&obj).unwrap();
    let (lo, hi) = parts[0].mesh.bounds();
    assert!((hi.0 - lo.0 - 2.0).abs() < 1e-4 && (lo.0 + hi.0).abs() < 1e-4, "centred, 2 units: {lo:?} {hi:?}");
    let stl = dir.path().join("t.stl");
    std::fs::write(&stl, "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 3 0 0\nvertex 0 3 0\nendloop\nendfacet\nendsolid t\n").unwrap();
    assert!(mesh::model(&stl).is_ok());
    // Drawn in a scene, in the material's colour.
    let mut pics = Pics::default();
    pics.files.insert("tri".into(), obj.clone());
    let p = draw_q(json!({"background": "#000000", "camera": {"position": [0, 0, 4]}, "objects": [{"id": "m", "type": "model", "src": "tri", "material": {"unlit": true}}]}), Quality::Preview, &mut pics);
    let red = p.pixels().iter().filter(|c| c.red() > 150 && c.green() < 40).count();
    assert!(red > 100, "{red} red pixels");
}

// ---- The Studio ----

fn studio(v: serde_json::Value, opts: &ViewOptions) -> Pixmap {
    let s = scene(v);
    let view = ViewCamera::default();
    viewport::render_view(&mut Space::cpu(), &s, 0.0, 1.0 / 30.0, 160, 90, &mut None_, Some(&view), opts).unwrap()
}

#[test]
fn the_floor_grid_shows_its_axes_and_hides_behind_things() {
    let empty = json!({"background": "#202020", "objects": []});
    let plain = studio(empty.clone(), &ViewOptions::default());
    let grid = studio(empty, &ViewOptions { grid: true, ..Default::default() });
    dump("grid", &grid);
    assert!(mean_diff(&plain, &grid) > 1.0, "lines drawn");
    let reds = grid.pixels().iter().filter(|c| c.red() > c.green() + 40).count();
    let greens = grid.pixels().iter().filter(|c| c.green() > c.red() + 40).count();
    assert!(reds > 10 && greens > 10, "x axis red ({reds}), z axis green ({greens})");
    // A big box over the origin hides the axes there.
    let boxed = studio(json!({"background": "#202020", "objects": [{"id": "b", "type": "box", "size": [3, 6, 3], "material": {"color": "#202020", "unlit": true}}]}), &ViewOptions { grid: true, ..Default::default() });
    let centre_reds = (70..90).flat_map(|x| (35..55).map(move |y| (x, y))).filter(|&(x, y)| {
        let c = rgba(&boxed, x, y);
        c[0] > c[1] + 40
    });
    assert_eq!(centre_reds.count(), 0, "no axis drawn over the box");
}

#[test]
fn the_grid_shows_on_a_floor_at_its_height() {
    // A floor plane at y = 0 is where the grid is: seen low, nearly edge on, the grid must not
    // flicker away into it.
    let opts = ViewOptions { grid: true, ..Default::default() };
    let view = ViewCamera { position: [6.0, 1.0, 7.0], target: [0.0, 0.8, 0.0], ..Default::default() };
    let draw = |objects: serde_json::Value| {
        let s = scene(json!({"background": "#000000", "objects": objects}));
        viewport::render_view(&mut Space::cpu(), &s, 0.0, 1.0 / 30.0, 320, 180, &mut None_, Some(&view), &opts).unwrap()
    };
    let floor = json!([{"id": "floor", "type": "plane", "width": 20, "height": 20, "rotation": [-90, 0, 0], "material": {"color": "#000000", "unlit": true}}]);
    let (bare, on) = (draw(json!([])), draw(floor));
    dump("grid-on-floor", &on);
    let lit = |p: &Pixmap| p.pixels().iter().filter(|c| c.red() > 20 || c.green() > 20).count();
    assert!(lit(&on) * 10 > lit(&bare) * 8, "grid on the floor: {} pixels, {} without it", lit(&on), lit(&bare));
}

#[test]
fn selection_outlines_edit_wires_and_helpers() {
    let s = json!({"background": "#000000", "lights": [{"id": "sun", "type": "directional"}, {"id": "spot", "type": "spot", "position": [-2, 3, 1]}],
        "cameras": [{"id": "side", "position": [4, 1, 0]}],
        "objects": [{"id": "b", "type": "box", "size": 1.5, "material": {"color": "#404040"}}, {"id": "c", "type": "sphere", "position": [2.5, 0, 0], "radius": 0.5}]});
    let none = studio(s.clone(), &ViewOptions::default());
    let selected = studio(s.clone(), &ViewOptions { selected: vec!["b".into()], ..Default::default() });
    dump("outline", &selected);
    let orange = |p: &Pixmap| p.pixels().iter().filter(|c| c.red() > 200 && c.green() > 100 && c.green() < 190 && c.blue() < 90).count();
    assert_eq!(orange(&none), 0);
    assert!(orange(&selected) > 20, "an orange outline: {}", orange(&selected));
    let edit = studio(s.clone(), &ViewOptions { edit: Some("b".into()), edit_vertices: vec![0], ..Default::default() });
    dump("edit", &edit);
    assert!(mean_diff(&none, &edit) > 0.3, "edges and vertices drawn");
    assert!(orange(&edit) > 3, "the selected vertex is highlighted");
    let helpers = studio(s, &ViewOptions { helpers: true, ..Default::default() });
    dump("helpers", &helpers);
    assert!(mean_diff(&none, &helpers) > 0.3, "light and camera icons drawn");
}

#[test]
fn solid_shading_ignores_materials_and_lights() {
    let s = json!({"background": "#000000", "lights": [{"id": "red", "type": "directional", "color": "#ff0000"}],
        "objects": [{"id": "b", "type": "box", "size": 2, "material": {"color": "#00ff00"}}]});
    let p = studio(s, &ViewOptions { shading: Shading::Solid, ..Default::default() });
    let c = rgba(&p, 80, 45);
    assert!(c[0].abs_diff(c[1]) < 6 && c[1].abs_diff(c[2]) < 6 && c[0] > 60, "grey: {c:?}");
}

#[test]
fn picking_finds_the_nearest_object() {
    let s = scene(json!({"objects": [
        {"id": "front", "type": "box", "position": [0, 0, 2]},
        {"id": "back", "type": "box", "position": [0, 0, -2]},
        {"id": "g", "type": "group", "position": [5, 0, 0], "children": [{"id": "child", "type": "sphere", "radius": 0.5}]}
    ]}));
    assert_eq!(viewport::pick(&s, 0.0, ([0.0, 0.0, 10.0], [0.0, 0.0, -1.0])).as_deref(), Some("front"));
    assert_eq!(viewport::pick(&s, 0.0, ([0.0, 0.0, -10.0], [0.0, 0.0, 1.0])).as_deref(), Some("back"));
    assert_eq!(viewport::pick(&s, 0.0, ([5.0, 0.0, 10.0], [0.0, 0.0, -1.0])).as_deref(), Some("child"), "children through their parent");
    assert_eq!(viewport::pick(&s, 0.0, ([0.0, 5.0, 10.0], [0.0, 0.0, -1.0])), None);
    // A ray from the editor's view through the projection of an object hits it.
    let v = ViewCamera::default();
    let [x, y] = v.project(1280.0, 720.0, [5.0, 0.0, 0.0]).unwrap();
    assert_eq!(viewport::pick(&s, 0.0, v.ray(1280.0, 720.0, x, y)).as_deref(), Some("child"));
    let (lo, hi) = viewport::object_bounds(&s, 0.0, "child").unwrap();
    assert!((lo[0] - 4.5).abs() < 1e-3 && (hi[0] - 5.5).abs() < 1e-3, "{lo:?} {hi:?}");
    let (glo, ghi) = viewport::object_bounds(&s, 0.0, "g").unwrap();
    assert!((glo[0] - 4.5).abs() < 1e-3 && (ghi[0] - 5.5).abs() < 1e-3, "a group spans its children");
    let m = viewport::world_matrix(&s, 0.0, "child").unwrap();
    assert_eq!(m[3][0], 5.0);
}

// ---- Both renderers ----

/// The GPU draws what the CPU draws (when this machine has a GPU; `KIMCHI_GPU=any` on llvmpipe).
#[test]
fn gpu_matches_cpu() {
    eprintln!("GPU: {}", gpu::Gpu::probe());
    let mut gpu = Space::new();
    eprintln!("3D engine: {}", gpu.describe());
    if !gpu.describe().starts_with("gpu") {
        return;
    }
    let scenes = [
        ("basic", json!({"background": "#101014", "camera": {"position": [2, 2, 5]},
            "objects": [
                {"id": "b", "type": "box", "size": 1.4, "bevel": 0.1, "rotation": [10, 30, 0], "material": {"color": "#ff5a36", "roughness": 0.3}},
                {"id": "t", "type": "torus", "position": [0, -1.2, 0], "material": {"color": "#ffffff", "metallic": 1, "roughness": 0.2}},
                {"id": "f", "type": "plane", "width": 8, "height": 8, "rotation": [-90, 0, 0], "position": [0, -1.6, 0]}
            ]})),
        ("lights", json!({"background": "#000000", "camera": {"position": [0, 4, 6], "target": [0, 0, 0]},
            "lights": [
                {"id": "s", "type": "spot", "position": [-1, 4, 1], "direction": [0.2, -1, -0.2], "angle": 50, "color": "#ffcc88", "intensity": 1.5},
                {"id": "a", "type": "area", "position": [2, 2, 2], "direction": [-0.5, -0.5, -0.5], "size": [1.5, 1], "intensity": 0.8},
                {"id": "p", "type": "point", "position": [0, 1, 2], "range": 5, "color": "#88aaff"}
            ],
            "objects": [
                {"id": "f", "type": "plane", "width": 10, "height": 10, "rotation": [-90, 0, 0], "material": {"color": "#d0d0d0", "roughness": 0.6}},
                {"id": "b", "type": "box", "size": 1, "position": [0, 0.5, 0], "material": {"color": "#ff5a36", "clearcoat": 1, "roughness": 0.5}}
            ]})),
        ("world", json!({"camera": {"position": [0, 1, 4]}, "environment": {"type": "sky", "visible": true},
            "lights": [{"id": "sun", "type": "directional", "direction": [-0.3, -0.6, -0.5]}],
            "objects": [
                {"id": "s", "type": "sphere", "radius": 0.8, "position": [-1, 0, 0], "material": {"color": "#ffffff", "metallic": 1, "roughness": 0.25}},
                {"id": "c", "type": "box", "size": 1.2, "position": [1, 0, 0], "material": {"color": "#ffffff", "pattern": {"type": "checker", "color": "#ff5a36", "color2": "#202020", "bump": 1}}}
            ]})),
    ];
    for (name, json) in scenes {
        let s = scene(json);
        let g = gpu.render(&s, 0.0, 160, 90, &mut None_, Quality::Preview).unwrap();
        let c = Space::cpu().render(&s, 0.0, 160, 90, &mut None_, Quality::Preview).unwrap();
        dump(&format!("gpu-{name}"), &g);
        dump(&format!("cpu-{name}"), &c);
        // Same picture, give or take edge anti-aliasing and shadow-map resolution.
        let diff = mean_diff(&g, &c);
        assert!(diff < 6.0, "{name}: mean difference {diff}");
    }
    // Camera effects on both (the GPU reads its depth back for them).
    let s = scene(json!({"background": "#000000", "camera": {"position": [0, 0, 3], "fStop": 1, "focusDistance": 3}, "render": {"bloom": 0.5, "ambientOcclusion": 0.5},
        "objects": [{"id": "a", "type": "sphere", "radius": 0.5, "material": {"emissive": "#ffffff", "emissiveIntensity": 3}},
                    {"id": "b", "type": "box", "size": 1, "position": [1, 0, -3]}]}));
    let g = gpu.render(&s, 0.0, 160, 90, &mut None_, Quality::Preview).unwrap();
    let c = Space::cpu().render(&s, 0.0, 160, 90, &mut None_, Quality::Preview).unwrap();
    dump("gpu-post", &g);
    dump("cpu-post", &c);
    assert!(mean_diff(&g, &c) < 6.0, "post: {}", mean_diff(&g, &c));
    // The Studio's overlays over the GPU's picture and depth.
    let s = scene(json!({"background": "#202020", "objects": [{"id": "b", "type": "box", "size": 1.5}]}));
    let opts = ViewOptions { grid: true, selected: vec!["b".into()], edit: Some("b".into()), ..Default::default() };
    let view = ViewCamera::default();
    let g = viewport::render_view(&mut gpu, &s, 0.0, 1.0 / 30.0, 160, 90, &mut None_, Some(&view), &opts).unwrap();
    let c = viewport::render_view(&mut Space::cpu(), &s, 0.0, 1.0 / 30.0, 160, 90, &mut None_, Some(&view), &opts).unwrap();
    dump("gpu-studio", &g);
    assert!(mean_diff(&g, &c) < 6.0, "studio: {}", mean_diff(&g, &c));
}

#[test]
fn a_big_floor_running_behind_the_camera_has_no_holes() {
    // The floor's corners behind the camera are clipped at the near plane; what is left reaches
    // far off screen, and every pixel of it up to the horizon must be drawn.
    let v = json!({"background": "#ff00ff", "fog": false, "shadows": false,
        "camera": {"position": [6, 4, 8], "target": [0, 1, 0], "fov": 40},
        "objects": [{"id": "floor", "type": "plane", "width": 30, "height": 30, "rotation": [-90, 0, 0], "material": {"color": "#40a040", "unlit": true}}]});
    let p = Space::cpu().render(&scene(v), 0.0, 640, 360, &mut None_, Quality::Preview).unwrap();
    dump("big-floor", &p);
    // Below the floor's far edges (the highest is about a third of the way down) it is all floor.
    let mut holes = 0;
    for y in 150..360 {
        for x in 0..640 {
            let c = rgba(&p, x, y);
            if c[0] > 128 && c[2] > 128 {
                holes += 1;
            }
        }
    }
    assert_eq!(holes, 0, "pixels of the background showing through the floor");
}
