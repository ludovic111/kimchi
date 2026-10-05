//! The CPU 3D renderer: triangles rasterised in parallel horizontal bands at twice the output
//! resolution (then averaged down, for smooth edges).
//!
//! Opaque triangles first fill a visibility buffer (nearest triangle and its barycentrics per
//! sample), so each sample is shaded once; transparent ones are then shaded and blended far to
//! near. Shadows come from depth maps seen from the lights (an orthographic box for the main sun,
//! a frustum for spot and area lights), read with a percentage-closer filter whose width grows
//! with the light's size.

use rayon::prelude::*;
use tiny_skia::Pixmap;

use super::math::{M4, V3};
use super::{Drawn, Frame3d, Pixels, ShadowRes, Surface, Want, backdrop, clear_color, finish, shade, view_dir};

/// Rows per band processed by one thread.
const BAND: usize = 16;
/// The main sun's shadow map; spot and area lights get smaller ones.
const SHADOW_SIZE: usize = 1536;
const SPOT_SHADOW_SIZE: usize = 1024;
/// Each face of a point light's cube.
const CUBE_SHADOW_SIZE: usize = 512;

/// A triangle on screen (or in the shadow map), with what is needed to shade it.
#[derive(Clone)]
pub(crate) struct Tri {
    pub item: u32,
    /// Screen x, y, depth (0..1) and 1/w per corner.
    pub s: [[f32; 4]; 3],
    /// World position, normal and uv per corner.
    world: [V3; 3],
    normal: [V3; 3],
    uv: [[f32; 2]; 3],
    /// How the position changes with the (scaled) texture coordinates.
    dpdu: V3,
    dpdv: V3,
    /// Bounding box in pixels.
    pub x0: i32,
    pub x1: i32,
    pub y0: i32,
    pub y1: i32,
    /// 1 / (twice the signed area).
    pub inv_area: f32,
}

/// A depth map seen from a light.
struct ShadowMap {
    res: ShadowRes,
    size: usize,
    depth: Vec<f32>,
}

pub(crate) fn render(f: &Frame3d, want: Want) -> Drawn {
    let ss: usize = if f.width as usize * 2 <= 4096 && f.height as usize * 2 <= 4096 { 2 } else { 1 };
    let (w, h) = (f.width as usize * ss, f.height as usize * ss);
    let maps: Vec<ShadowMap> = f
        .shadows
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let size = if k == 0 && s.ortho {
                SHADOW_SIZE
            } else if s.face.is_some() {
                CUBE_SHADOW_SIZE
            } else {
                SPOT_SHADOW_SIZE
            };
            ShadowMap { res: *s, size, depth: shadow_map(f, &s.viewproj, size) }
        })
        .collect();
    let linear = want.linear;

    let mut opaque = vec![];
    let mut clear: Vec<(f32, Vec<Tri>)> = vec![];
    for (i, it) in f.items.iter().enumerate() {
        let tris = triangles(f, i, &f.viewproj, w, h);
        if it.mat.transparent() {
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

    // The background: a colour, the world, or nothing.
    let bg = clear_color(f, linear);
    let mut color = vec![bg; w * h];
    let ow = f.width as usize;
    if f.env.as_ref().is_some_and(|e| e.visible) && f.background.is_none() {
        color.par_chunks_mut(w * ss).enumerate().for_each(|(oy, rows)| {
            for ox in 0..ow {
                let Some(c) = backdrop(f, view_dir(f, ox as f32 + 0.5, oy as f32 + 0.5)) else { continue };
                let px = finish(f, [c[0], c[1], c[2], 1.0], false, linear);
                for dy in 0..ss {
                    for dx in 0..ss {
                        rows[dy * w + ox * ss + dx] = px;
                    }
                }
            }
        });
    }

    // Shade: once per output pixel when all its samples see the same triangle (most of the
    // picture), per sample only along edges.
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
                let c = shade_at(f, &opaque[first.0 as usize], b1 / n, b2 / n, &maps, linear);
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
                    let c = shade_at(f, &opaque[ti as usize], b1, b2, &maps, linear);
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
                        let c = shade_at(f, t, b1, b2, &maps, linear);
                        cband[i] = over(c, cband[i]);
                    }
                });
            }
        });
    }

    // The nearest opaque distance per output pixel.
    let (ow, oh) = (f.width as usize, f.height as usize);
    let distance = if want.depth {
        (0..ow * oh)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % ow, i / ow);
                let mut z = 1.0f32;
                for dy in 0..ss {
                    for dx in 0..ss {
                        z = z.min(depth[(y * ss + dy) * w + x * ss + dx]);
                    }
                }
                if z >= 1.0 { f32::INFINITY } else { f.camera.distance(z) }
            })
            .collect()
    } else {
        vec![]
    };

    // Average ss×ss samples into each output pixel.
    let n = (ss * ss) as f32;
    let average = |x: usize, y: usize| -> [f32; 4] {
        let mut acc = [0.0f32; 4];
        for dy in 0..ss {
            for dx in 0..ss {
                let c = color[(y * ss + dy) * w + x * ss + dx];
                for k in 0..4 {
                    acc[k] += c[k];
                }
            }
        }
        acc.map(|v| v / n)
    };
    if linear {
        let out: Vec<[f32; 4]> = (0..ow * oh).into_par_iter().map(|i| average(i % ow, i / ow)).collect();
        return Drawn { pixels: Pixels::Linear(out), depth: distance };
    }
    let mut out = vec![0u8; ow * oh * 4];
    out.par_chunks_mut(ow * 4).enumerate().for_each(|(y, row)| {
        for x in 0..ow {
            let acc = average(x, y);
            let a = acc[3].clamp(0.0, 1.0);
            let px = &mut row[x * 4..x * 4 + 4];
            for k in 0..3 {
                px[k] = crate::render::byte(acc[k].clamp(0.0, a) * 255.0);
            }
            px[3] = crate::render::byte(a * 255.0);
        }
    });
    let pixmap = Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(f.width, f.height).expect("non-empty")).expect("sized");
    Drawn { pixels: Pixels::Encoded(pixmap), depth: distance }
}

/// The distance of the nearest opaque thing per output pixel (infinite where there is none),
/// without shading: for the Studio's overlays when the picture came from elsewhere.
pub(crate) fn depth(f: &Frame3d) -> Vec<f32> {
    let (w, h) = (f.width as usize, f.height as usize);
    let mut tris = vec![];
    for (i, it) in f.items.iter().enumerate() {
        if !it.mat.transparent() {
            tris.extend(triangles(f, i, &f.viewproj, w, h));
        }
    }
    let bands = bin(&tris, h);
    let mut z = vec![1.0f32; w * h];
    z.par_chunks_mut(w * BAND).enumerate().for_each(|(b, band)| {
        for &ti in &bands[b] {
            raster(&tris[ti as usize], w, b * BAND, band.len() / w, |i, d, _, _| {
                if d < band[i] {
                    band[i] = d;
                }
            });
        }
    });
    z.into_par_iter().map(|d| if d >= 1.0 { f32::INFINITY } else { f.camera.distance(d) }).collect()
}

/// Premultiplied `src` over `dst`.
fn over(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

fn shade_at(f: &Frame3d, t: &Tri, b1: f32, b2: f32, maps: &[ShadowMap], linear: bool) -> [f32; 4] {
    let b0 = 1.0 - b1 - b2;
    let p = t.world[0] * b0 + t.world[1] * b1 + t.world[2] * b2;
    let n = (t.normal[0] * b0 + t.normal[1] * b1 + t.normal[2] * b2).norm();
    let uv = [t.uv[0][0] * b0 + t.uv[1][0] * b1 + t.uv[2][0] * b2, t.uv[0][1] * b0 + t.uv[1][1] * b1 + t.uv[2][1] * b2];
    let item = &f.items[t.item as usize];
    let lit = |i: usize| match maps.iter().position(|m| m.res.light == i) {
        None => 1.0,
        // A point light's six maps: the one facing `p`.
        Some(k) if maps[k].res.face.is_some() => maps.get(k + super::cube_face(p - f.lights[i].v)).map_or(1.0, |m| lit(m, p, n, f)),
        Some(k) => lit(&maps[k], p, n, f),
    };
    let c = shade(f, &item.mat, &Surface { p, n, uv, dpdu: t.dpdu, dpdv: t.dpdv }, &lit);
    finish(f, c, item.mat.unlit, linear)
}

/// How lit `p` is by a shadow map's light: a percentage-closer filter, 3×3 texels for small
/// lights, 5×5 spread over the light's soft edge for bigger ones. The GPU does the same.
fn lit(m: &ShadowMap, p: V3, n: V3, f: &Frame3d) -> f32 {
    let l = &f.lights[m.res.light];
    let to_light = if l.point { (l.v - p).norm() } else { -l.v };
    // Push along the normal a little to avoid the surface shadowing itself.
    let p = p + n * 0.01 + to_light * 0.005;
    let q4 = m.res.viewproj.point(p);
    if q4[3] <= 1e-6 {
        return 1.0;
    }
    let q = V3(q4[0] / q4[3], q4[1] / q4[3], q4[2] / q4[3]);
    // Outside a light's frustum is lit. The sun's box ends around what the camera sees: past
    // it, fading to lit (its edge texels hold other, nearer ground, which used to cast a
    // shadow band across every far floor).
    let edge = q.0.abs().max(q.1.abs());
    if !(0.0..1.0).contains(&q.2) || edge > 1.0 {
        return 1.0;
    }
    let fade = if m.res.ortho { ((edge - 0.9) / 0.1).clamp(0.0, 1.0) } else { 0.0 };
    let size = m.size as f32;
    let (sx, sy) = ((q.0 * 0.5 + 0.5) * size, (0.5 - q.1 * 0.5) * size);
    // World size of a texel where `p` is, and the filter's reach in texels.
    let texel = if m.res.ortho { m.res.extent / size } else { 2.0 * m.res.distance(q.2) * m.res.extent / size };
    let radius = (m.res.softness / texel.max(1e-6)).clamp(0.0, 12.0);
    let (taps, step) = if radius <= 1.0 { (1i32, 1.0f32) } else { (2, radius / 2.0) };
    // Perspective maps compare distances, with a bias that grows with the texel size and how
    // slanted the surface is to the light (no acne on grazing floors); wide filters reach
    // further across a slanted surface, so they need more.
    let ndl = n.dot(to_light).abs().max(0.05);
    let mut slope = (1.0 - ndl * ndl).sqrt() / ndl;
    if !m.res.ortho {
        // A perspective map stores depth along its axis, which changes across the map with
        // how the surface turns from the axis, not from the light: a floor right under a lamp
        // aimed sideways faces the lamp yet slopes in its map (and the filter's far taps saw
        // the floor itself as an occluder there).
        let vp = &m.res.viewproj.0;
        let axis = V3(vp[0][3], vp[1][3], vp[2][3]).norm();
        let nda = n.dot(axis).abs().max(0.05);
        slope = slope.max((1.0 - nda * nda).sqrt() / nda);
    }
    let slope = slope.min(8.0);
    let reach = if taps > 1 { texel * slope * step * taps as f32 } else { 0.0 };
    // The sun's map is orthographic: depths are distances, so its bias is in world units too,
    // a few texels' worth on slanted surfaces (not a share of its depth range, which on a big
    // scene let light through recesses a hand deep and left their shadows speckled).
    let mine = if m.res.ortho {
        q.2 - (texel * (1.0 + slope) * 1.5 + reach) / (m.res.far - m.res.near).max(1e-6)
    } else {
        m.res.distance(q.2) * 0.995 - texel * (1.0 + slope) * 2.0 - reach
    };
    let mut sum = 0.0;
    for dy in -taps..=taps {
        for dx in -taps..=taps {
            let x = (crate::render::floor(sx + dx as f32 * step) as i32).clamp(0, m.size as i32 - 1);
            let y = (crate::render::floor(sy + dy as f32 * step) as i32).clamp(0, m.size as i32 - 1);
            let d = m.depth[y as usize * m.size + x as usize];
            let d = if m.res.ortho { d } else { m.res.distance(d) };
            sum += if mine <= d { 1.0 } else { 0.0 };
        }
    }
    let lit = sum / ((2 * taps + 1) * (2 * taps + 1)) as f32;
    lit + (1.0 - lit) * fade
}

fn shadow_map(f: &Frame3d, m: &M4, n: usize) -> Vec<f32> {
    let mut tris = vec![];
    for (i, it) in f.items.iter().enumerate() {
        if it.mat.base[3] < 0.5 || it.mat.unlit || !it.cast_shadow || it.mat.transmission > 0.5 {
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
pub(crate) fn bin(tris: &[Tri], h: usize) -> Vec<Vec<u32>> {
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
pub(crate) fn raster(t: &Tri, w: usize, y_start: usize, rows: usize, mut hit: impl FnMut(usize, f32, f32, f32)) {
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
pub(crate) fn triangles(f: &Frame3d, item: usize, m: &M4, w: usize, h: usize) -> Vec<Tri> {
    let it = &f.items[item];
    let mesh = &it.mesh;
    let full = *m * it.model;
    let scale = it.mat.texture_scale;
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
        if t.iter().any(|&i| i as usize >= verts.len()) {
            continue;
        }
        let corners: Vec<Corner> = t
            .iter()
            .map(|&i| {
                let (clip, world, normal) = verts[i as usize];
                Corner { clip, world, normal, uv: mesh.uv.get(i as usize).copied().unwrap_or([0.0, 0.0]) }
            })
            .collect();
        let (dpdu, dpdv) = tangents(&corners, scale);
        for poly in clip_near(&corners) {
            for k in 1..poly.len() - 1 {
                if let Some(tri) = screen(item as u32, [&poly[0], &poly[k], &poly[k + 1]], w, h, dpdu, dpdv) {
                    out.push(tri);
                }
            }
        }
    }
    out
}

/// How the world position changes with each texture coordinate (scaled by the material) across
/// a triangle.
fn tangents(c: &[Corner], scale: [f32; 2]) -> (V3, V3) {
    let (e1, e2) = (c[1].world - c[0].world, c[2].world - c[0].world);
    let (du1, dv1) = ((c[1].uv[0] - c[0].uv[0]) * scale[0], (c[1].uv[1] - c[0].uv[1]) * scale[1]);
    let (du2, dv2) = ((c[2].uv[0] - c[0].uv[0]) * scale[0], (c[2].uv[1] - c[0].uv[1]) * scale[1]);
    let det = du1 * dv2 - du2 * dv1;
    if det.abs() < 1e-12 {
        return (V3::default(), V3::default());
    }
    let k = 1.0 / det;
    ((e1 * dv2 - e2 * dv1) * k, (e2 * du1 - e1 * du2) * k)
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

/// How far past the picture's edges (in picture widths and heights, from its centre) triangles
/// are kept: beyond, they are clipped, so no corner lands thousands of pictures away (a floor
/// clipped at the near plane right next to the eye) where `f32` can't place the pixels near
/// the horizon any more and holes open up.
const GUARD: f32 = 4.0;

/// Keeps the part of a triangle in front of the near plane (clip z ≥ 0) and inside the guard
/// band around the picture.
fn clip_near(c: &[Corner]) -> Vec<Vec<Corner>> {
    // Signed distances to each plane: inside when ≥ 0.
    let planes: [fn(&[f32; 4]) -> f32; 5] = [
        |p| p[2],
        |p| GUARD * p[3] - p[0],
        |p| GUARD * p[3] + p[0],
        |p| GUARD * p[3] - p[1],
        |p| GUARD * p[3] + p[1],
    ];
    let inside_all = |v: &Corner| v.clip[3] > 1e-6 && planes.iter().all(|d| d(&v.clip) >= 0.0);
    if c.iter().all(inside_all) {
        return vec![c.to_vec()];
    }
    let mut poly = c.to_vec();
    for d in planes {
        if poly.len() < 3 {
            return vec![];
        }
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (a, b) = (&poly[i], &poly[(i + 1) % poly.len()]);
            let (da, db) = (d(&a.clip), d(&b.clip));
            if da >= 0.0 {
                out.push(*a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                out.push(lerp(a, b, t.clamp(0.0, 1.0)));
            }
        }
        poly = out;
    }
    // In front of the near plane, w is positive (the projection keeps it ≥ the near distance).
    if poly.len() >= 3 && poly.iter().all(|v| v.clip[3] > 1e-6) { vec![poly] } else { vec![] }
}

fn screen(item: u32, c: [&Corner; 3], w: usize, h: usize, dpdu: V3, dpdv: V3) -> Option<Tri> {
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
        dpdu,
        dpdv,
        x0,
        x1,
        y0,
        y1,
        inv_area: 1.0 / area.abs(),
    })
}
