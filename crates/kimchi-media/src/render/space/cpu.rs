//! The CPU 3D renderer: triangles rasterised in parallel horizontal bands at twice the output
//! resolution (then averaged down, for smooth edges).
//!
//! Opaque triangles first fill a visibility buffer (nearest triangle and its barycentrics per
//! sample), so each sample is shaded once; transparent ones are then shaded and blended far to
//! near. The main directional light's shadow comes from a depth map seen from the light, read
//! with a 3×3 filter for soft edges.

use rayon::prelude::*;
use tiny_skia::Pixmap;

use super::math::{M4, V3};
use super::{Frame3d, linear_to_srgb, shade, shoulder};

/// Rows per band processed by one thread.
const BAND: usize = 16;
const SHADOW_SIZE: usize = 1536;

/// A triangle on screen (or in the shadow map), with what is needed to shade it.
#[derive(Clone)]
struct Tri {
    item: u32,
    /// Screen x, y, depth (0..1) and 1/w per corner.
    s: [[f32; 4]; 3],
    /// World position, normal and uv per corner.
    world: [V3; 3],
    normal: [V3; 3],
    uv: [[f32; 2]; 3],
    /// Bounding box in pixels.
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
    /// 1 / (twice the signed area).
    inv_area: f32,
}

pub(crate) fn render(f: &Frame3d) -> Pixmap {
    let ss: usize = if f.width as usize * 2 <= 4096 && f.height as usize * 2 <= 4096 { 2 } else { 1 };
    let (w, h) = (f.width as usize * ss, f.height as usize * ss);
    let shadow = f.shadow.map(|(m, _)| (m, shadow_map(f, &m)));

    let mut opaque = vec![];
    let mut clear: Vec<(f32, Vec<Tri>)> = vec![];
    for (i, it) in f.items.iter().enumerate() {
        let tris = triangles(f, i, &f.viewproj, w, h);
        if it.mat.base[3] < 0.999 || it.mat.texture.as_ref().is_some_and(|t| t.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255)) {
            clear.push((it.depth, tris));
        } else {
            opaque.extend(tris);
        }
    }
    clear.sort_by(|a, b| b.0.total_cmp(&a.0));

    // Visibility: depth + which triangle + its perspective-correct barycentrics.
    let mut depth = vec![1.0f32; w * h];
    let mut vis = vec![(u32::MAX, 0.0f32, 0.0f32); w * h];
    let bands = bin(&opaque, h);
    depth.par_chunks_mut(w * BAND).zip(vis.par_chunks_mut(w * BAND)).enumerate().for_each(|(b, (dband, vband))| {
        let y_start = b * BAND;
        for &ti in &bands[b] {
            let t = &opaque[ti as usize];
            raster(t, w, y_start, dband.len() / w, |i, z, b1, b2| {
                if z < dband[i] {
                    dband[i] = z;
                    vband[i] = (ti, b1, b2);
                }
            });
        }
    });

    // Shade: once per output pixel when all its samples see the same triangle (most of the
    // picture), per sample only along edges.
    let bg = f.background.map(|c| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]).unwrap_or([0.0; 4]);
    let mut color = vec![bg; w * h];
    let ow = f.width as usize;
    color.par_chunks_mut(w * ss).enumerate().for_each(|(oy, rows)| {
        for ox in 0..ow {
            let at = |dx: usize, dy: usize| (oy * ss + dy) * w + ox * ss + dx;
            let first = vis[at(0, 0)];
            let same = first.0 != u32::MAX && (0..ss).all(|dy| (0..ss).all(|dx| vis[at(dx, dy)].0 == first.0));
            if same {
                let n = (ss * ss) as f32;
                let (mut b1, mut b2) = (0.0, 0.0);
                for dy in 0..ss {
                    for dx in 0..ss {
                        let v = vis[at(dx, dy)];
                        b1 += v.1;
                        b2 += v.2;
                    }
                }
                let c = shade_at(f, &opaque[first.0 as usize], b1 / n, b2 / n, shadow.as_ref());
                for dy in 0..ss {
                    for dx in 0..ss {
                        let px = &mut rows[dy * w + ox * ss + dx];
                        *px = over(c, *px);
                    }
                }
                continue;
            }
            for dy in 0..ss {
                for dx in 0..ss {
                    let (ti, b1, b2) = vis[at(dx, dy)];
                    if ti == u32::MAX {
                        continue;
                    }
                    let c = shade_at(f, &opaque[ti as usize], b1, b2, shadow.as_ref());
                    let px = &mut rows[dy * w + ox * ss + dx];
                    *px = over(c, *px);
                }
            }
        }
    });

    // Transparent, far to near, each blended over what is there (and only in front of opaque).
    for (_, tris) in &clear {
        let bands = bin(tris, h);
        color.par_chunks_mut(w * BAND).zip(depth.par_chunks(w * BAND)).enumerate().for_each(|(b, (cband, dband))| {
            let y_start = b * BAND;
            for &ti in &bands[b] {
                let t = &tris[ti as usize];
                raster(t, w, y_start, cband.len() / w, |i, z, b1, b2| {
                    if z < dband[i] {
                        let c = shade_at(f, t, b1, b2, shadow.as_ref());
                        cband[i] = over(c, cband[i]);
                    }
                });
            }
        });
    }

    // Average ss×ss samples into each output pixel.
    let (ow, oh) = (f.width as usize, f.height as usize);
    let mut out = vec![0u8; ow * oh * 4];
    out.par_chunks_mut(ow * 4).enumerate().for_each(|(y, row)| {
        for x in 0..ow {
            let mut acc = [0.0f32; 4];
            for dy in 0..ss {
                for dx in 0..ss {
                    let c = color[(y * ss + dy) * w + x * ss + dx];
                    for k in 0..4 {
                        acc[k] += c[k];
                    }
                }
            }
            let n = (ss * ss) as f32;
            let a = (acc[3] / n).clamp(0.0, 1.0);
            let px = &mut row[x * 4..x * 4 + 4];
            for k in 0..3 {
                px[k] = ((acc[k] / n).clamp(0.0, a) * 255.0).round() as u8;
            }
            px[3] = (a * 255.0).round() as u8;
        }
    });
    Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(f.width, f.height).expect("non-empty")).expect("sized")
}

/// Premultiplied `src` over `dst`.
fn over(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

fn shade_at(f: &Frame3d, t: &Tri, b1: f32, b2: f32, shadow: Option<&(M4, Vec<f32>)>) -> [f32; 4] {
    let b0 = 1.0 - b1 - b2;
    let p = t.world[0] * b0 + t.world[1] * b1 + t.world[2] * b2;
    let n = (t.normal[0] * b0 + t.normal[1] * b1 + t.normal[2] * b2).norm();
    let uv = [t.uv[0][0] * b0 + t.uv[1][0] * b1 + t.uv[2][0] * b2, t.uv[0][1] * b0 + t.uv[1][1] * b1 + t.uv[2][1] * b2];
    let item = &f.items[t.item as usize];
    let lit = shadow.map_or(1.0, |(m, map)| lit(m, map, p, n, f));
    let c = shade(f, &item.mat, p, n, uv, lit);
    let a = c[3].clamp(0.0, 1.0);
    let enc = |v: f32| linear_to_srgb(if item.mat.unlit { v } else { shoulder(v) });
    [enc(c[0]) * a, enc(c[1]) * a, enc(c[2]) * a, a]
}

/// How lit `p` is by the shadowing light (3×3 percentage-closer filter).
fn lit(m: &M4, map: &[f32], p: V3, n: V3, f: &Frame3d) -> f32 {
    let Some((_, li)) = f.shadow else { return 1.0 };
    let l = f.lights[li].v;
    // Push along the normal a little to avoid the surface shadowing itself.
    let p = p + n * 0.01 + (-l) * 0.005;
    let q = m.point3(p);
    let (sx, sy) = ((q.0 * 0.5 + 0.5) * SHADOW_SIZE as f32, (0.5 - q.1 * 0.5) * SHADOW_SIZE as f32);
    if !(0.0..1.0).contains(&q.2) {
        return 1.0;
    }
    let mut sum = 0.0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (x, y) = ((sx as i32 + dx).clamp(0, SHADOW_SIZE as i32 - 1), (sy as i32 + dy).clamp(0, SHADOW_SIZE as i32 - 1));
            let d = map[y as usize * SHADOW_SIZE + x as usize];
            sum += if q.2 - 0.002 <= d { 1.0 } else { 0.0 };
        }
    }
    sum / 9.0
}

fn shadow_map(f: &Frame3d, m: &M4) -> Vec<f32> {
    let n = SHADOW_SIZE;
    let mut tris = vec![];
    for (i, it) in f.items.iter().enumerate() {
        if it.mat.base[3] < 0.5 || it.mat.unlit {
            continue;
        }
        tris.extend(triangles(f, i, m, n, n));
    }
    let bands = bin(&tris, n);
    let mut map = vec![1.0f32; n * n];
    map.par_chunks_mut(n * BAND).enumerate().for_each(|(b, band)| {
        for &ti in &bands[b] {
            raster(&tris[ti as usize], n, b * BAND, band.len() / n, |i, z, _, _| {
                if z < band[i] {
                    band[i] = z;
                }
            });
        }
    });
    map
}

/// Triangle indices per band of rows.
fn bin(tris: &[Tri], h: usize) -> Vec<Vec<u32>> {
    let n = h.div_ceil(BAND);
    let mut bands = vec![vec![]; n];
    for (i, t) in tris.iter().enumerate() {
        let (a, b) = ((t.y0.max(0) as usize) / BAND, (t.y1.max(0) as usize).min(h.saturating_sub(1)) / BAND);
        for band in bands.iter_mut().take(b + 1).skip(a) {
            band.push(i as u32);
        }
    }
    bands
}

/// Calls `hit(index in band, depth, b1, b2)` for every sample of `t` inside the band's rows,
/// barycentrics perspective-correct.
fn raster(t: &Tri, w: usize, y_start: usize, rows: usize, mut hit: impl FnMut(usize, f32, f32, f32)) {
    let y0 = t.y0.max(y_start as i32);
    let y1 = t.y1.min((y_start + rows) as i32 - 1);
    let x0 = t.x0.max(0);
    let x1 = t.x1.min(w as i32 - 1);
    if y0 > y1 || x0 > x1 {
        return;
    }
    let [a, b, c] = t.s;
    let edge = |p: [f32; 4], q: [f32; 4], x: f32, y: f32| (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0]);
    for y in y0..=y1 {
        let py = y as f32 + 0.5;
        for x in x0..=x1 {
            let px = x as f32 + 0.5;
            let w0 = edge(b, c, px, py) * t.inv_area;
            let w1 = edge(c, a, px, py) * t.inv_area;
            let w2 = edge(a, b, px, py) * t.inv_area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
            if !(0.0..=1.0).contains(&z) {
                continue;
            }
            // Perspective-correct weights from 1/w.
            let (p0, p1, p2) = (w0 * a[3], w1 * b[3], w2 * c[3]);
            let sum = p0 + p1 + p2;
            if sum <= 0.0 {
                continue;
            }
            hit((y as usize - y_start) * w + x as usize, z, p1 / sum, p2 / sum);
        }
    }
}

/// An item's triangles through `m` onto a `w`×`h` target, clipped at the near plane.
fn triangles(f: &Frame3d, item: usize, m: &M4, w: usize, h: usize) -> Vec<Tri> {
    let it = &f.items[item];
    let mesh = &it.mesh;
    let full = *m * it.model;
    let verts: Vec<([f32; 4], V3, V3)> = mesh
        .pos
        .iter()
        .zip(&mesh.normal)
        .map(|(p, n)| {
            let p = V3(p[0], p[1], p[2]);
            (full.point(p), it.model.point3(p), it.normal.dir(V3(n[0], n[1], n[2])))
        })
        .collect();
    let mut out = vec![];
    for t in mesh.index.as_chunks::<3>().0.iter() {
        let corners: Vec<Corner> = t
            .iter()
            .map(|&i| {
                let (clip, world, normal) = verts[i as usize];
                Corner { clip, world, normal, uv: mesh.uv[i as usize] }
            })
            .collect();
        for poly in clip_near(&corners) {
            for k in 1..poly.len() - 1 {
                if let Some(tri) = screen(item as u32, [&poly[0], &poly[k], &poly[k + 1]], w, h) {
                    out.push(tri);
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
struct Corner {
    clip: [f32; 4],
    world: V3,
    normal: V3,
    uv: [f32; 2],
}

fn lerp(a: &Corner, b: &Corner, t: f32) -> Corner {
    Corner {
        clip: std::array::from_fn(|k| a.clip[k] + (b.clip[k] - a.clip[k]) * t),
        world: a.world + (b.world - a.world) * t,
        normal: a.normal + (b.normal - a.normal) * t,
        uv: [a.uv[0] + (b.uv[0] - a.uv[0]) * t, a.uv[1] + (b.uv[1] - a.uv[1]) * t],
    }
}

/// Keeps the part of a triangle in front of the near plane (clip z ≥ 0).
fn clip_near(c: &[Corner]) -> Vec<Vec<Corner>> {
    let inside = |v: &Corner| v.clip[2] >= 0.0 && v.clip[3] > 1e-6;
    if c.iter().all(inside) {
        return vec![c.to_vec()];
    }
    if !c.iter().any(inside) {
        return vec![];
    }
    let mut out = vec![];
    for i in 0..c.len() {
        let (a, b) = (&c[i], &c[(i + 1) % c.len()]);
        if inside(a) {
            out.push(*a);
        }
        if inside(a) != inside(b) {
            let t = a.clip[2] / (a.clip[2] - b.clip[2]);
            out.push(lerp(a, b, t.clamp(0.0, 1.0)));
        }
    }
    if out.len() >= 3 { vec![out] } else { vec![] }
}

fn screen(item: u32, c: [&Corner; 3], w: usize, h: usize) -> Option<Tri> {
    let s: [[f32; 4]; 3] = c.map(|v| {
        let iw = 1.0 / v.clip[3];
        [(v.clip[0] * iw * 0.5 + 0.5) * w as f32, (0.5 - v.clip[1] * iw * 0.5) * h as f32, v.clip[2] * iw, iw]
    });
    let area = (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[1][1] - s[0][1]) * (s[2][0] - s[0][0]);
    if area.abs() < 1e-9 || !area.is_finite() {
        return None;
    }
    // Wind every triangle the same way (no back-face culling: surfaces are two-sided).
    let (s, c) = if area < 0.0 { ([s[0], s[2], s[1]], [c[0], c[2], c[1]]) } else { (s, c) };
    let xs = [s[0][0], s[1][0], s[2][0]];
    let ys = [s[0][1], s[1][1], s[2][1]];
    let x0 = xs.iter().cloned().fold(f32::MAX, f32::min).floor() as i32;
    let x1 = xs.iter().cloned().fold(f32::MIN, f32::max).ceil() as i32;
    let y0 = ys.iter().cloned().fold(f32::MAX, f32::min).floor() as i32;
    let y1 = ys.iter().cloned().fold(f32::MIN, f32::max).ceil() as i32;
    if x1 < 0 || y1 < 0 || x0 >= w as i32 || y0 >= h as i32 {
        return None;
    }
    Some(Tri {
        item,
        s,
        world: c.map(|v| v.world),
        normal: c.map(|v| v.normal),
        uv: c.map(|v| v.uv),
        x0,
        x1,
        y0,
        y1,
        inv_area: 1.0 / area.abs(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::*;
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

    fn draw(scene: serde_json::Value, t: f64) -> Pixmap {
        let Scene::Space(s) = Scene::from_json(&scene).unwrap() else { panic!("3d") };
        Space::cpu().render(&s, t, 160, 90, &mut None_).unwrap()
    }

    fn rgba(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap().demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

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

    /// The GPU draws what the CPU draws (when this machine has a GPU).
    #[test]
    fn gpu_matches_cpu() {
        eprintln!("GPU: {}", super::super::gpu::Gpu::probe());
        let mut gpu = Space::new();
        eprintln!("3D engine: {}", gpu.describe());
        if !gpu.describe().starts_with("gpu") {
            return;
        }
        let Scene::Space(s) = Scene::from_json(&json!({"background": "#101014", "camera": {"position": [2, 2, 5]},
            "objects": [
                {"id": "b", "type": "box", "size": 1.4, "bevel": 0.1, "rotation": [10, 30, 0], "material": {"color": "#ff5a36", "roughness": 0.3}},
                {"id": "t", "type": "torus", "position": [0, -1.2, 0], "material": {"color": "#ffffff", "metallic": 1, "roughness": 0.2}},
                {"id": "f", "type": "plane", "width": 8, "height": 8, "rotation": [-90, 0, 0], "position": [0, -1.6, 0]}
            ]}))
        .unwrap() else { panic!("3d") };
        let g = gpu.render(&s, 0.0, 160, 90, &mut None_).unwrap();
        let c = Space::cpu().render(&s, 0.0, 160, 90, &mut None_).unwrap();
        if let Some(dir) = std::env::var_os("KIMCHI_DUMP") {
            g.save_png(std::path::Path::new(&dir).join("gpu.png")).unwrap();
            c.save_png(std::path::Path::new(&dir).join("cpu.png")).unwrap();
        }
        // Same picture, give or take edge anti-aliasing and shadow-map resolution.
        let diff: f64 = g.pixels().iter().zip(c.pixels()).map(|(a, b)| (a.red().abs_diff(b.red()) as f64 + a.green().abs_diff(b.green()) as f64 + a.blue().abs_diff(b.blue()) as f64) / 3.0).sum::<f64>()
            / g.pixels().len() as f64;
        assert!(diff < 6.0, "mean difference {diff}");
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
        // From above, the floor around the box is lit; with shadows the box's own top is lit too,
        // so compare a point just outside the box's footprint diagonally: lit either way.
        let lit = draw(scene(true), 0.0);
        let corner = rgba(&lit, 20, 10);
        assert!(corner[0] > 100, "floor lit: {corner:?}");
        // Seen from the side, the floor right under the box is darker with shadows on.
        let side = |shadows: bool| {
            let mut s = scene(shadows);
            s["camera"] = json!({"position": [0, 3, 6], "target": [0, 0, 0]});
            s["objects"][1]["position"] = json!([0, 1.5, 0]);
            draw(s, 0.0)
        };
        let (on, off) = (side(true), side(false));
        if let Some(dir) = std::env::var_os("KIMCHI_DUMP") {
            on.save_png(std::path::Path::new(&dir).join("shadow-on.png")).unwrap();
            off.save_png(std::path::Path::new(&dir).join("shadow-off.png")).unwrap();
        }
        // The floor under the box.
        let under = |p: &Pixmap| (72..88).flat_map(|x| (40..50).map(move |y| (x, y))).map(|(x, y)| rgba(p, x, y)[0] as u64).sum::<u64>();
        assert!(under(&on) < under(&off) * 3 / 4, "shadow darkens the floor: {} vs {}", under(&on), under(&off));
    }
}
