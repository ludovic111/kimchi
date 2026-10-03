use std::path::PathBuf;
use std::sync::Arc;

use kimchi_core::Scene;
use serde_json::{Value, json};
use tiny_skia::Pixmap;

use super::*;
use crate::render::space::{Pictures, srgb_to_linear};

struct NoPictures;
impl Pictures for NoPictures {
    fn picture(&mut self, _: &str, _: f64) -> Option<Arc<Pixmap>> {
        None
    }
    fn path(&self, _: &str) -> Option<PathBuf> {
        None
    }
}

fn scene(v: Value) -> Scene3d {
    let Scene::Space(s) = Scene::from_json(&v).unwrap() else { panic!("3d") };
    s
}

fn frame(s: &Scene3d, w: u32, h: u32) -> Frame3d {
    let mut f = Space::cpu().frame(s, 0.0, w, h, &mut NoPictures);
    f.quality = Quality::Final;
    f
}

fn settings(samples: u32, denoise: bool) -> Settings {
    Settings { samples, bounces: 4, denoise, seed: 0, filter: Filter::BlackmanHarris }
}

fn rgba(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = p.pixel(x, y).unwrap().demultiply();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

/// Mean of a channel over a rectangle.
fn mean(p: &Pixmap, x0: u32, y0: u32, x1: u32, y1: u32, ch: usize) -> f32 {
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            s += rgba(p, x, y)[ch] as f32;
        }
    }
    s / ((x1 - x0) * (y1 - y0)) as f32
}

fn dump(name: &str, p: &Pixmap) {
    if let Some(dir) = std::env::var_os("KIMCHI_DUMP") {
        p.save_png(std::path::Path::new(&dir).join(format!("trace-{name}.png"))).unwrap();
    }
}

#[test]
fn settings_come_from_the_scene() {
    let s = scene(json!({"objects": [], "render": {"engine": "path", "samples": 12, "bounces": 2, "denoise": false}}));
    let t = Settings::of(&s.render);
    assert_eq!((t.samples, t.bounces, t.denoise), (12, 2, false));
    // Odd values stay in range.
    let t = Settings::of(&RenderSettings { samples: -5.0, bounces: f64::NAN, ..RenderSettings::default() });
    assert_eq!((t.samples, t.bounces), (1, 4));
}

/// A white matte floor in the sun is as bright in both engines.
#[test]
fn sunlit_floor_matches_the_standard_engine() {
    for (ambient, sun) in [(0.0, [0.0, -1.0, 0.0]), (0.25, [-0.3, -1.0, 0.2])] {
        let s = scene(json!({"background": "#000000", "ambient": ambient, "fog": false,
            "camera": {"position": [0, 4, 4], "target": [0, 0, 0]},
            "lights": [{"id": "sun", "type": "directional", "direction": sun, "intensity": 0.8}],
            "render": {"engine": "path"},
            "objects": [{"id": "floor", "type": "plane", "width": 40, "height": 40, "rotation": [-90, 0, 0],
                         "material": {"color": "#ffffff", "roughness": 0.9}}]}));
        let traced = render(&frame(&s, 64, 36), &settings(16, false));
        let raster = Space::cpu().render(&s, 0.0, 64, 36, &mut NoPictures, Quality::Preview).unwrap();
        dump("sun-traced", &traced);
        dump("sun-raster", &raster);
        let (a, b) = (mean(&traced, 24, 14, 40, 22, 0), mean(&raster, 24, 14, 40, 22, 0));
        assert!(b > 60.0, "lit: {b}");
        assert!((a - b).abs() < 0.03 * b, "ambient {ambient}: path tracer {a}, standard {b}");
    }
}

#[test]
fn unlit_colour_and_transparent_background() {
    let s = scene(json!({"camera": {"position": [0, 0, 3]}, "render": {"engine": "path"},
        "objects": [{"id": "card", "type": "plane", "width": 1, "height": 1, "material": {"color": "#00ff00", "unlit": true}}]}));
    let p = render(&frame(&s, 160, 90), &settings(4, true));
    assert_eq!(rgba(&p, 80, 45), [0, 255, 0, 255]);
    assert_eq!(rgba(&p, 1, 1)[3], 0);
    // Through `Space::render` at final quality, the path tracer is used and agrees.
    let q = Space::cpu().render(&s, 0.0, 160, 90, &mut NoPictures, Quality::Final).unwrap();
    assert_eq!(rgba(&q, 80, 45), [0, 255, 0, 255]);
}

/// Through a glass ball, what is behind appears upside down (it is a lens).
#[test]
fn glass_refracts() {
    let wall = |glass: bool| {
        let mut objects = vec![
            json!({"id": "top", "type": "plane", "width": 20, "height": 10, "position": [0, 5, -4], "material": {"color": "#ff0000", "unlit": true}}),
            json!({"id": "bottom", "type": "plane", "width": 20, "height": 10, "position": [0, -5, -4], "material": {"color": "#0000ff", "unlit": true}}),
        ];
        if glass {
            objects.push(json!({"id": "ball", "type": "sphere", "radius": 1,
                "material": {"color": "#ffffff", "transmission": 1, "roughness": 0, "ior": 1.5}}));
        }
        scene(json!({"background": "#000000", "camera": {"position": [0, 0, 5], "fov": 30}, "lights": [{"id": "l", "intensity": 0}],
            "render": {"engine": "path"}, "objects": objects}))
    };
    let (w, h) = (96, 96);
    let with = render(&frame(&wall(true), w, h), &settings(16, false));
    let without = render(&frame(&wall(false), w, h), &settings(4, false));
    dump("glass", &with);
    // Just above the middle: red without the ball, blue through it; just below, the reverse.
    let (above, below) = (h / 2 - 8, h / 2 + 8);
    let a = rgba(&without, w / 2, above);
    assert!(a[0] > 200 && a[2] < 30, "{a:?}");
    let a = rgba(&with, w / 2, above);
    assert!(a[2] > a[0] + 40, "flipped above: {a:?}");
    let b = rgba(&with, w / 2, below);
    assert!(b[0] > b[2] + 40, "flipped below: {b:?}");
}

/// A mirror-like metal ball shows the world: the sky's colour on top, the ground's below.
#[test]
fn metal_reflects_the_environment() {
    let s = scene(json!({"background": "#000000", "camera": {"position": [0, 0, 4]}, "lights": [{"id": "l", "intensity": 0}],
        "render": {"engine": "path"},
        "objects": [{"id": "ball", "type": "sphere", "radius": 1, "material": {"color": "#ffffff", "metallic": 1, "roughness": 0.1}}]}));
    let mut f = frame(&s, 96, 96);
    f.env = Some(Env {
        kind: EnvKind::Gradient,
        color: [0.0; 3],
        top: [0.9, 0.1, 0.05],
        horizon: [0.3, 0.3, 0.3],
        bottom: [0.05, 0.1, 0.9],
        image: None,
        strength: 1.0,
        rotation: 0.0,
        visible: false,
    });
    let p = render(&f, &settings(16, true));
    dump("metal", &p);
    let top = rgba(&p, 48, 48 - 22);
    let bottom = rgba(&p, 48, 48 + 22);
    assert!(top[0] > top[2] + 60, "sky on top: {top:?}");
    assert!(bottom[2] > bottom[0] + 60, "ground below: {bottom:?}");
    // The world isn't visible: the background shows around the ball.
    assert_eq!(rgba(&p, 2, 2), [0, 0, 0, 255]);
    // Made visible, it is.
    f.env.as_mut().unwrap().visible = true;
    let p = render(&f, &settings(2, false));
    let corner = rgba(&p, 2, 2);
    assert!(corner[0] > 100, "{corner:?}");
}

/// The width (pixels) of the half-lit band at a shadow's edge under an area light.
fn penumbra(size: f64) -> usize {
    let draw = |card: bool| {
        let mut objects = vec![json!({"id": "floor", "type": "plane", "width": 20, "height": 20, "rotation": [-90, 0, 0], "material": {"color": "#ffffff", "roughness": 1}})];
        if card {
            objects.push(json!({"id": "card", "type": "box", "size": [3, 0.05, 3], "position": [-1.5, 2, 0], "material": {"color": "#ffffff"}}));
        }
        let s = scene(json!({"background": "#000000", "ambient": 0, "fog": false,
            "camera": {"position": [0, 8, 0.01], "target": [0, 0, 0], "fov": 40},
            "lights": [{"id": "panel", "type": "area", "position": [-1.5, 4, 0], "direction": [0, -1, 0], "size": [size, size], "intensity": 1.2}],
            "render": {"engine": "path"}, "objects": objects}));
        // Direct light only: light bounced off the card would fill the shadow in.
        let p = render(&frame(&s, 160, 90), &Settings { bounces: 0, ..settings(64, false) });
        dump(&format!("area-{size}-{card}"), &p);
        p
    };
    let (shadowed, open) = (draw(true), draw(false));
    // The card's edge (x = 0) casts its shadow's edge at x = 1.5 on the floor; look at the
    // floor right of the card, as a share of the light it gets without the card.
    let share: Vec<f32> = (84..156).map(|x| mean(&shadowed, x, 40, x + 1, 50, 0) / mean(&open, x, 40, x + 1, 50, 0).max(1.0)).collect();
    assert!(share[0] < 0.1 && share[share.len() - 1] > 0.9, "dark next to the card, lit far from it: {share:?}");
    share.iter().filter(|&&v| v > 0.1 && v < 0.9).count()
}

#[test]
fn area_lights_soften_shadows_with_their_size() {
    let (small, big) = (penumbra(0.1), penumbra(2.0));
    assert!(big > small * 3 && big > 8, "penumbra {small} px → {big} px");
}

#[test]
fn the_same_seed_gives_the_same_picture() {
    let s = scene(json!({"background": "#202020", "camera": {"position": [2, 2, 5]}, "render": {"engine": "path"},
        "objects": [
            {"id": "b", "type": "box", "size": 1.4, "rotation": [10, 30, 0], "material": {"color": "#ff5a36", "roughness": 0.3}},
            {"id": "f", "type": "plane", "width": 8, "height": 8, "rotation": [-90, 0, 0], "position": [0, -1, 0]}
        ]}));
    let f = frame(&s, 64, 36);
    let a = render(&f, &settings(4, false));
    let b = render(&f, &settings(4, false));
    assert_eq!(a.data(), b.data());
    let c = render(&f, &Settings { seed: 1, ..settings(4, false) });
    assert_ne!(a.data(), c.data());
    // Rendered in steps, the same as all at once.
    let mut p = Progressive::new(&f, settings(4, false));
    p.add(1);
    p.add(3);
    assert_eq!(p.samples(), 4);
    assert!(p.done());
    assert_eq!(p.picture().data(), a.data());
}

/// Sampling a panorama by its brightness gives the same light as finding it by chance.
#[test]
fn panorama_sampling_agrees_with_plain_sampling() {
    // A dark panorama with one bright patch above and to the side.
    let (tw, th) = (64u32, 32u32);
    let mut rgba = vec![0u8; (tw * th * 4) as usize];
    for y in 0..th {
        for x in 0..tw {
            let i = ((y * tw + x) * 4) as usize;
            let bright = (8..14).contains(&y) && (20..30).contains(&x);
            let v = if bright { 255 } else { 30 };
            rgba[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    let tex = Arc::new(Texture { width: tw, height: th, rgba });
    let s = scene(json!({"background": "#000000", "fog": false, "camera": {"position": [0, 4, 4], "target": [0, 0, 0]},
        "lights": [{"id": "l", "intensity": 0}], "render": {"engine": "path"},
        "objects": [
            {"id": "floor", "type": "plane", "width": 6, "height": 6, "rotation": [-90, 0, 0], "material": {"color": "#ffffff", "roughness": 1}},
            {"id": "ball", "type": "sphere", "radius": 0.8, "position": [0, 0.8, 0], "material": {"color": "#ffffff", "roughness": 0.4}}
        ]}));
    let mut f = frame(&s, 48, 27);
    f.env = Some(Env { kind: EnvKind::Image, color: [0.0; 3], top: [0.0; 3], horizon: [0.0; 3], bottom: [0.0; 3], image: Some(tex), strength: 2.0, rotation: 0.7, visible: false });
    let mean_lin = |p: &Progressive| {
        let n = p.samples() as f32;
        p.px.iter().map(|q| lum(q.c) / n).sum::<f32>() / p.px.len() as f32
    };
    let mut aimed = Progressive::new(&f, settings(64, false));
    aimed.add(64);
    let mut plain = Progressive::new(&f, settings(256, false));
    plain.world.env_nee = false;
    plain.add(256);
    let (a, b) = (mean_lin(&aimed), mean_lin(&plain));
    assert!(a > 0.05, "lit: {a}");
    assert!((a - b).abs() < 0.04 * b, "aimed {a}, by chance {b}");
    // A uniform panorama lights a white floor like a uniform colour does.
    let grey = Arc::new(Texture { width: 8, height: 4, rgba: [128u8, 128, 128, 255].repeat(32) });
    let level = srgb_to_linear(128);
    f.env = Some(Env { image: Some(grey), rotation: 0.0, strength: 1.0, ..f.env.clone().unwrap() });
    let mut img = Progressive::new(&f, settings(32, false));
    img.add(32);
    f.env = Some(Env { kind: EnvKind::Color, color: [level; 3], image: None, ..f.env.clone().unwrap() });
    let mut col = Progressive::new(&f, settings(32, false));
    col.add(32);
    let (a, b) = (mean_lin(&img), mean_lin(&col));
    assert!((a - b).abs() < 0.03 * b, "panorama {a}, colour {b}");
}

/// A point on a white floor lit only by a uniform world gets all of it back (nothing is lost
/// or made up by the sampling), here with a few bounces off a white ball too.
#[test]
fn a_white_floor_under_a_uniform_sky() {
    let s = scene(json!({"background": "#000000", "fog": false, "camera": {"position": [0, 6, 0.01], "target": [0, 0, 0]},
        "lights": [{"id": "l", "intensity": 0}], "render": {"engine": "path"},
        "objects": [{"id": "floor", "type": "plane", "width": 40, "height": 40, "rotation": [-90, 0, 0], "material": {"color": "#ffffff", "roughness": 1}}]}));
    let mut f = frame(&s, 32, 18);
    f.env = Some(Env { kind: EnvKind::Color, color: [0.4; 3], top: [0.0; 3], horizon: [0.0; 3], bottom: [0.0; 3], image: None, strength: 1.0, rotation: 0.0, visible: false });
    let mut p = Progressive::new(&f, settings(64, false));
    p.add(64);
    let n = p.samples() as f32;
    let centre = p.px[9 * 32 + 16].c[1] / n;
    // White diffuse (less the little the glossy coat reflects instead) × 0.4.
    assert!((centre - 0.4).abs() < 0.03, "{centre}");
}

#[test]
fn spot_lights_light_a_cone() {
    let s = scene(json!({"background": "#000000", "ambient": 0, "fog": false,
        "camera": {"position": [0, 8, 0.01], "target": [0, 0, 0]},
        "lights": [{"id": "spot", "type": "spot", "position": [0, 4, 0], "direction": [0, -1, 0], "angle": 30, "intensity": 1}],
        "render": {"engine": "path"},
        "objects": [{"id": "floor", "type": "plane", "width": 20, "height": 20, "rotation": [-90, 0, 0], "material": {"color": "#ffffff", "roughness": 1}}]}));
    let p = render(&frame(&s, 80, 45), &settings(4, false));
    assert!(rgba(&p, 40, 22)[0] > 150, "under the spot: {:?}", rgba(&p, 40, 22));
    assert!(rgba(&p, 75, 22)[0] < 10, "outside the cone: {:?}", rgba(&p, 75, 22));
}

/// With a wide aperture, things far behind the point in focus blur.
#[test]
fn depth_of_field_blurs_out_of_focus() {
    let s = scene(json!({"background": "#000000", "camera": {"position": [0, 0, 5]}, "lights": [{"id": "l", "intensity": 0}],
        "render": {"engine": "path"},
        "objects": [
            {"id": "near", "type": "plane", "width": 1, "height": 4, "position": [-0.5, 0, 0], "material": {"color": "#ffffff", "unlit": true}},
            {"id": "far", "type": "plane", "width": 6, "height": 20, "position": [6, 0, -15], "material": {"color": "#ffffff", "unlit": true}}
        ]}));
    let edges = |aperture: f32| {
        let mut f = frame(&s, 160, 90);
        f.camera.aperture = aperture;
        f.camera.focus = 5.0;
        let p = render(&f, &settings(32, false));
        dump(&format!("dof-{aperture}"), &p);
        let soft = |x0: u32, x1: u32| (x0..x1).filter(|&x| (20..235).contains(&rgba(&p, x, 45)[0])).count();
        (soft(60, 85), soft(85, 160))
    };
    let (near0, far0) = edges(0.0);
    let (near1, far1) = edges(0.3);
    assert!(far1 > far0 + 6, "the far edge blurs: {far0} → {far1}");
    assert!(near1 <= near0 + 2, "the near edge stays sharp: {near0} → {near1}");
}

/// Denoising brings a few samples much closer to many.
#[test]
fn denoising_removes_grain() {
    let s = scene(json!({"background": "#000000", "fog": false, "camera": {"position": [0, 3, 4], "target": [0, 0.5, 0]},
        "lights": [{"id": "l", "intensity": 0}], "render": {"engine": "path"},
        "objects": [
            {"id": "floor", "type": "plane", "width": 30, "height": 30, "rotation": [-90, 0, 0], "material": {"color": "#c0c0c0", "roughness": 1}},
            {"id": "ball", "type": "sphere", "radius": 0.7, "position": [0, 0.7, 0], "material": {"color": "#e05030", "roughness": 0.6}}
        ]}));
    let mut f = frame(&s, 80, 45);
    f.env = Some(Env { kind: EnvKind::Gradient, color: [0.0; 3], top: [0.9, 0.9, 1.0], horizon: [0.4, 0.4, 0.4], bottom: [0.05, 0.05, 0.05], image: None, strength: 1.0, rotation: 0.0, visible: false });
    let reference = render(&f, &settings(512, false));
    let noisy = render(&f, &settings(8, false));
    let clean = render(&f, &settings(8, true));
    dump("dn-ref", &reference);
    dump("dn-noisy", &noisy);
    dump("dn-clean", &clean);
    let err = |p: &Pixmap| p.data().iter().zip(reference.data()).map(|(a, b)| (*a as f32 - *b as f32).powi(2)).sum::<f32>() / p.data().len() as f32;
    let (e_noisy, e_clean) = (err(&noisy), err(&clean));
    assert!(e_clean < e_noisy / 3.0, "squared error: noisy {e_noisy}, denoised {e_clean}");
}

/// A half see-through card over nothing comes out half transparent; a bump texture makes a
/// flat card's shading vary.
#[test]
fn opacity_and_bumps() {
    let s = scene(json!({"fog": false, "camera": {"position": [0, 0, 3]}, "lights": [{"id": "sun", "direction": [0.5, -0.3, -1]}],
        "render": {"engine": "path"},
        "objects": [{"id": "card", "type": "plane", "width": 2, "height": 2, "material": {"color": "#ffffff", "opacity": 0.5, "roughness": 1}}]}));
    let p = render(&frame(&s, 64, 64), &settings(64, false));
    let a = mean(&p, 24, 24, 40, 40, 3);
    assert!((a - 127.5).abs() < 12.0, "alpha {a}");

    // Stripes of height across the card, seen in a raking light.
    let (tw, th) = (64u32, 4u32);
    let rgba: Vec<u8> = (0..th).flat_map(|_| (0..tw).flat_map(|x| {
        let v = if (x / 8) % 2 == 0 { 255 } else { 0 };
        [v, v, v, 255]
    })).collect();
    let tex = Arc::new(Texture { width: tw, height: th, rgba });
    let s = scene(json!({"background": "#000000", "fog": false, "ambient": 0, "camera": {"position": [0, 0, 3]},
        "lights": [{"id": "sun", "direction": [-1, 0, -0.3]}], "render": {"engine": "path"},
        "objects": [{"id": "card", "type": "plane", "width": 2, "height": 2, "material": {"color": "#ffffff", "roughness": 1}}]}));
    let spread = |bumpy: bool| {
        let mut f = frame(&s, 64, 64);
        if bumpy {
            f.items[0].mat.bump = Some((tex.clone(), 1.0));
        }
        let p = render(&f, &settings(8, false));
        let row: Vec<f32> = (16..48).map(|x| mean(&p, x, 30, x + 1, 34, 0)).collect();
        row.iter().cloned().fold(0.0, f32::max) - row.iter().cloned().fold(255.0, f32::min)
    };
    let (flat, bumpy) = (spread(false), spread(true));
    assert!(bumpy > flat + 20.0, "flat {flat}, bumpy {bumpy}");
}

#[test]
fn geometry_is_kept_while_nothing_moves() {
    let s = scene(json!({"camera": {"position": [0, 0, 5]}, "objects": [{"id": "b", "type": "box"}]}));
    let f = frame(&s, 8, 8);
    let (a, b) = (geometry(&f.items), geometry(&f.items));
    assert!(Arc::ptr_eq(&a, &b));
    let moved = frame(&scene(json!({"camera": {"position": [0, 0, 5]}, "objects": [{"id": "b", "type": "box", "position": [1, 0, 0]}]})), 8, 8);
    assert!(!Arc::ptr_eq(&a, &geometry(&moved.items)));
}

#[test]
fn odd_scenes_do_not_break_it() {
    // Nothing at all; a 1×1 frame; NaN-free output for extreme settings.
    let empty = scene(json!({"objects": [], "render": {"engine": "path"}}));
    let p = render(&frame(&empty, 1, 1), &settings(2, true));
    assert_eq!(p.width(), 1);
    let s = scene(json!({"background": "#000000", "camera": {"position": [0, 0, 3], "fov": 170},
        "lights": [{"id": "p", "type": "point", "position": [0, 0, 0], "intensity": 1e6, "size": [0.5, 0.5]}, {"id": "a", "type": "area", "size": [0, 0]}],
        "objects": [{"id": "s", "type": "sphere", "material": {"color": "#ffffff", "opacity": 0.5, "clearcoat": 1, "ior": 1, "transmission": 0.5}}]}));
    let mut f = frame(&s, 16, 16);
    f.camera.aperture = f32::NAN;
    f.exposure = f32::INFINITY;
    let p = render(&f, &Settings { bounces: 64, ..settings(4, true) });
    assert_eq!(p.width(), 16);
}

/// `cargo test --release -p kimchi-media --lib template_speed -- --ignored --nocapture`
#[test]
#[ignore]
fn template_speed() {
    let ctx = kimchi_core::templates::Ctx { width: 1920.0, height: 1080.0, duration: 5.0 };
    for id in kimchi_core::templates::ids() {
        let t = kimchi_core::templates::find(id).unwrap();
        if t.kind != "3d" {
            continue;
        }
        let Ok(Scene::Space(s)) = t.build(&serde_json::Map::new(), ctx) else { continue };
        let f = Space::cpu().frame(&s, 1.0, 640, 360, &mut NoPictures);
        let t0 = std::time::Instant::now();
        let mut p = Progressive::new(&f, settings(32, true));
        p.add(32);
        let traced = t0.elapsed();
        let pic = p.picture();
        let (rays, secs) = p.stats();
        eprintln!("{id}: 640×360, 32 spp in {traced:?} + denoise {:?} ({:.1} M rays/s)", t0.elapsed() - traced, rays as f64 / secs / 1e6);
        dump(&format!("template-{id}"), &pic);
        if std::env::var_os("KIMCHI_DUMP").is_some() {
            dump(&format!("template-{id}-standard"), &Space::cpu().render(&s, 1.0, 640, 360, &mut NoPictures, Quality::Preview).unwrap());
        }
    }
}
