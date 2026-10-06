//! Tests of the mesh engine: primitives, triangles, every modifier and every edit operation.

use serde_json::json;

use super::math::*;
use super::ops::{self, Selection};
use super::*;
use crate::motion::Vec3;
use crate::motion::stack::Modifier;

fn modifier(kind: &str, params: serde_json::Value) -> Modifier {
    let serde_json::Value::Object(map) = params else { panic!("params are an object") };
    Modifier::new(kind, map)
}

fn run(mesh: &PolyMesh, kind: &str, params: serde_json::Value, t: f64) -> PolyMesh {
    apply_modifiers(mesh.clone(), &[modifier(kind, params)], t, &|_| None)
}

fn cube(size: f64) -> PolyMesh {
    shape_mesh(&Shape3d::Box { size: Vec3([size; 3]), bevel: 0.0 }).unwrap()
}

/// Indices in range, faces of 3+ distinct corners, finite numbers, matching texture coordinates.
fn well_formed(m: &PolyMesh) {
    for f in &m.faces {
        assert!(f.len() >= 3, "face with {} corners", f.len());
        assert!(f.iter().all(|&v| (v as usize) < m.positions.len()), "index out of range");
        let mut s = f.clone();
        s.sort_unstable();
        s.dedup();
        assert_eq!(s.len(), f.len(), "repeated corner in {f:?}");
    }
    assert!(m.positions.iter().all(|p| finite(*p)));
    if let Some(uv) = &m.uvs {
        assert_eq!(uv.len(), m.faces.len());
        for (f, u) in m.faces.iter().zip(uv) {
            assert_eq!(f.len(), u.len());
        }
    }
    let t = m.triangulate();
    assert!(t.indices.iter().all(|&i| (i as usize) < t.positions.len()));
    assert!(t.normals.iter().all(|n| n.iter().all(|v| v.is_finite()) && ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 1e-3));
}

/// Every face's normal points away from the centre (for convex shapes around the origin).
fn faces_point_out(m: &PolyMesh) {
    for f in 0..m.faces.len() {
        let n = m.face_normal(f);
        if n == [0.0; 3] {
            continue;
        }
        assert!(dot(n, m.face_center(f)) > -1e-9, "face {f} points in: {:?}", m.face_points(f));
    }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

// ---- primitives -----------------------------------------------------------------------------

#[test]
fn face_tessellation_preserves_concavity_winding_and_relative_scale() {
    let face = [[-2.,-2.,0.],[2.,-2.,0.],[2.,-1.,0.],[-1.,-1.,0.],[-1.,1.,0.],[2.,1.,0.],[2.,2.,0.],[-2.,2.,0.]];
    for reverse in [false,true] {
        let mut points = face.to_vec();
        if reverse { points.reverse(); }
        let expected = face_triangles(&points);
        assert_eq!(expected.len(),points.len()-2);
        for size in [1e-200,1e-8,1.,1e100,1e200] {
            let scaled:Vec<_> = points.iter().map(|p|scale(*p,size)).collect();
            assert_eq!(face_triangles(&scaled),expected,"scale {size}, reversed {reverse}");
        }
        let translated:Vec<_> = points.iter().map(|p|add(*p,[1e12,-1e12,1e12])).collect();
        assert_eq!(face_triangles(&translated),expected,"distant origins keep the same face");
    }
    assert!(face_triangles(&[[0.;3];4]).is_empty());
    assert!(face_triangles(&[[f64::NAN;3];4]).is_empty());
}

#[test]
fn drawable_faces_and_smooth_normals_do_not_depend_on_scene_units() {
    let mut smoothed = cube(1.0);
    smoothed.smooth_angle = 180.0;
    let concave = PolyMesh {
        positions: vec![[0.,0.,0.],[0.,0.,2.],[1.,0.,2.],[1.,0.,1.],[2.,0.,1.],[2.,0.,0.]],
        faces: vec![vec![0,1,2,3,4,5]],
        ..Default::default()
    };
    for reference in [smoothed,concave] {
        let tris = reference.triangulate();
        assert!(!tris.indices.is_empty());
        for size in [1e-20,1e-8,1.,1e8,1e20] {
            let mut mesh = reference.clone();
            mesh.positions.iter_mut().for_each(|p| *p=scale(*p,size));
            let scaled = mesh.triangulate();
            assert_eq!(scaled.indices.len(),tris.indices.len(),"scale {size}: faces must remain drawable");
            for (actual,expected) in scaled.normals.iter().zip(&tris.normals) {
                assert!(actual.iter().zip(expected).all(|(a,b)|(a-b).abs()<1e-6),"scale {size}: {actual:?} vs {expected:?}");
            }
            for face in 0..mesh.faces.len() {
                assert!(dot(mesh.face_normal(face),reference.face_normal(face))>0.999999,"scale {size}: face {face} normal");
            }
        }
    }
    assert_eq!(try_norm([0.;3]),None);
    assert_eq!(try_norm([f64::INFINITY,0.,0.]),None);
    assert_eq!(try_norm([f64::NAN,0.,0.]),None);
    for size in [1e-300,1e300] {
        assert!(dot(try_norm([size,-size,size]).unwrap(),norm([1.,-1.,1.]))>0.999999);
    }
}

#[test]
fn primitives_are_closed_outward_and_sized() {
    let shapes = vec![
        (Shape3d::Box { size: Vec3([2.0, 1.0, 0.5]), bevel: 0.0 }, Some(1.0), [1.0, 0.5, 0.25]),
        (Shape3d::Box { size: Vec3([2.0, 1.0, 1.0]), bevel: 0.1 }, None, [1.0, 0.5, 0.5]),
        (Shape3d::Sphere { radius: 0.5, segments: 0.0 }, Some(4.0 / 3.0 * PI * 0.125), [0.5; 3]),
        (Shape3d::Sphere { radius: 1.0, segments: 8.0 }, None, [1.0; 3]),
        (Shape3d::Icosphere { radius: 0.5, detail: 2.0 }, Some(4.0 / 3.0 * PI * 0.125), [0.5; 3]),
        (Shape3d::Cylinder { radius: 0.5, height: 1.0, segments: 0.0 }, Some(PI * 0.25), [0.5; 3]),
        // A hexagon has corners on z and flat sides across x.
        (Shape3d::Cylinder { radius: 0.5, height: 2.0, segments: 6.0 }, Some(1.5 * 3f64.sqrt() * 0.25 * 2.0), [0.5 * 0.75f64.sqrt(), 1.0, 0.5]),
        (Shape3d::Cone { radius: 0.5, height: 1.0, segments: 0.0 }, Some(PI * 0.25 / 3.0), [0.5; 3]),
        (Shape3d::Capsule { radius: 0.3, height: 1.0 }, Some(PI * 0.09 * 0.4 + 4.0 / 3.0 * PI * 0.027), [0.3, 0.5, 0.3]),
        (Shape3d::Torus { radius: 0.5, tube: 0.2 }, Some(2.0 * PI * PI * 0.5 * 0.04), [0.7, 0.2, 0.7]),
    ];
    for (shape, volume, half) in shapes {
        let m = shape_mesh(&shape).unwrap();
        well_formed(&m);
        assert!(m.is_closed(), "{} is watertight", shape.name());
        assert!(m.volume() > 0.0, "{} faces out", shape.name());
        if shape.name() != "torus" {
            faces_point_out(&m);
        }
        if let Some(v) = volume {
            assert!(close(m.volume(), v, 0.02), "{}: volume {} vs {v}", shape.name(), m.volume());
        }
        let (lo, hi) = m.bounds();
        for k in 0..3 {
            assert!(close(hi[k], half[k], 0.01) && close(-lo[k], half[k], 0.01), "{} bounds {lo:?} {hi:?}", shape.name());
        }
    }
}

#[test]
fn open_primitives_face_their_way() {
    let plane = shape_mesh(&Shape3d::Plane { width: 2.0, height: 1.0 }).unwrap();
    assert_eq!(plane.faces.len(), 1);
    assert!(dot(plane.face_normal(0), [0.0, 0.0, 1.0]) > 0.999, "a plane faces +z");
    assert_eq!(plane.bounds(), ([-1.0, -0.5, 0.0], [1.0, 0.5, 0.0]));
    let grid = shape_mesh(&Shape3d::Grid { width: 2.0, height: 4.0, rows: 8.0, cols: 4.0 }).unwrap();
    assert_eq!(grid.faces.len(), 32);
    assert_eq!(grid.positions.len(), 45);
    assert!((0..grid.faces.len()).all(|f| dot(grid.face_normal(f), [0.0, 1.0, 0.0]) > 0.999), "a grid faces up");
    assert_eq!(grid.bounds(), ([-1.0, 0.0, -2.0], [1.0, 0.0, 2.0]));
    well_formed(&grid);
    // Silly sizes don't explode.
    let long = shape_mesh(&Shape3d::Grid { width: 1.0, height: 1.0, rows: 1e9, cols: 1.0 }).unwrap();
    assert_eq!(long.faces.len(), 2000);
    let nan = shape_mesh(&Shape3d::Sphere { radius: f64::NAN, segments: f64::NAN }).unwrap();
    assert!(nan.faces.is_empty());
}

#[test]
fn uvs_wrap_and_tile() {
    let sphere = shape_mesh(&Shape3d::Sphere { radius: 1.0, segments: 16.0 }).unwrap();
    let uv = sphere.uvs.as_ref().unwrap();
    assert!(uv.iter().flatten().all(|c| (0.0..=1.0).contains(&c[0]) && (0.0..=1.0).contains(&c[1])));
    // No face stretches across the seam.
    for f in uv {
        let (lo, hi) = f.iter().fold((f64::MAX, f64::MIN), |(a, b), c| (a.min(c[0]), b.max(c[0])));
        assert!(hi - lo < 0.2);
    }
    let cube = cube(1.0);
    for f in cube.uvs.as_ref().unwrap() {
        let mut us: Vec<f64> = f.iter().map(|c| c[0]).collect();
        us.sort_by(f64::total_cmp);
        assert!(us[0].abs() < 1e-9 && (us[3] - 1.0).abs() < 1e-9, "each side 0–1");
    }
}

#[test]
fn lathe_turns_a_profile() {
    let vase = shape_mesh(&Shape3d::Lathe { profile: vec![[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]], segments: 32.0, angle: 360.0 }).unwrap();
    well_formed(&vase);
    assert!(vase.is_closed(), "a profile from axis to axis closes");
    assert!(close(vase.volume(), PI * 0.25, 0.02), "{}", vase.volume());
    // Drawn downwards it still faces out.
    let down = shape_mesh(&Shape3d::Lathe { profile: vec![[0.0, 1.0], [0.5, 1.0], [0.5, 0.0], [0.0, 0.0]], segments: 32.0, angle: 360.0 }).unwrap();
    assert!(down.volume() > 0.0);
    let half = shape_mesh(&Shape3d::Lathe { profile: vec![[0.5, 0.0], [0.5, 1.0]], segments: 8.0, angle: 180.0 }).unwrap();
    assert_eq!(half.faces.len(), 8);
    assert!(!half.is_closed());
    assert!(shape_mesh(&Shape3d::Lathe { profile: vec![[1.0, 0.0]], segments: 8.0, angle: 360.0 }).unwrap().faces.is_empty());
}

#[test]
fn curves_become_tubes() {
    let curve = |radius: f64, closed: bool, trim_end: f64| Shape3d::Curve {
        points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 1.0]],
        closed,
        smooth: true,
        radius,
        sides: 8.0,
        trim_start: 0.0,
        trim_end,
    };
    assert!(shape_mesh(&curve(0.0, false, 1.0)).is_none(), "no radius: an invisible path");
    let tube = shape_mesh(&curve(0.05, false, 1.0)).unwrap();
    well_formed(&tube);
    assert!(tube.is_closed(), "capped");
    assert!(tube.volume() > 0.0);
    let ring = shape_mesh(&curve(0.05, true, 1.0)).unwrap();
    assert!(ring.is_closed(), "a closed curve makes a closed ring");
    assert!(ring.volume() > 0.0);
    let half = shape_mesh(&curve(0.05, false, 0.5)).unwrap();
    assert!(half.volume() < tube.volume() * 0.7 && half.volume() > tube.volume() * 0.3);
    assert!(shape_mesh(&curve(0.05, false, 0.0)).unwrap().faces.is_empty());
    // Every ring stays round (no pinching from twisted frames): side faces have similar areas.
    let areas: Vec<f64> = (0..tube.faces.len()).filter(|&f| tube.faces[f].len() == 4).map(|f| tube.face_area(f)).collect();
    let mean = areas.iter().sum::<f64>() / areas.len() as f64;
    assert!(areas.iter().all(|a| *a < mean * 4.0));
}

#[test]
fn extrude_shape_is_upright() {
    let m = shape_mesh(&Shape3d::Extrude { d: "M0 0 L100 0 L50 100 Z".into(), size: 2.0, depth: 0.5, bevel: 0.0 }).unwrap();
    well_formed(&m);
    assert!(m.is_closed() && m.volume() > 0.0);
    // SVG y points down: the apex at y = 100 ends up at the bottom.
    let lowest = m.positions.iter().min_by(|a, b| a[1].total_cmp(&b[1])).unwrap();
    assert!(lowest[0].abs() < 1e-9 && (lowest[1] + 1.0).abs() < 1e-9);
}

#[test]
fn texture_seams_are_behind() {
    // A picture wrapped round a sphere, cylinder, capsule, torus or icosphere shows its middle
    // at the front (+z, where the camera usually is) and its seam at the back.
    let shapes = [
        Shape3d::Sphere { radius: 1.0, segments: 32.0 },
        Shape3d::Cylinder { radius: 1.0, height: 1.0, segments: 7.0 },
        Shape3d::Capsule { radius: 0.5, height: 2.0 },
        Shape3d::Torus { radius: 1.0, tube: 0.2 },
        Shape3d::Icosphere { radius: 1.0, detail: 2.0 },
    ];
    for s in shapes {
        let m = shape_mesh(&s).unwrap();
        let uvs = m.uvs.as_ref().unwrap();
        let (mut front, mut best) = (None, f64::NEG_INFINITY);
        for (fi, f) in m.faces.iter().enumerate() {
            // Flat caps have a picture of their own.
            if f.len() > 4 && f.iter().all(|&v| (m.positions[v as usize][1] - m.positions[f[0] as usize][1]).abs() < 1e-9) {
                continue;
            }
            let us: Vec<f64> = uvs[fi].iter().map(|c| c[0]).collect();
            let spread = us.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - us.iter().cloned().fold(f64::INFINITY, f64::min);
            assert!(spread < 0.5, "{s:?}: face {fi} stretches across the picture");
            for (k, &v) in f.iter().enumerate() {
                let p = m.positions[v as usize];
                // The point furthest to the front, nearest the middle.
                let score = p[2] * 10.0 - p[0].abs() - p[1].abs() * 0.1;
                if score > best {
                    best = score;
                    front = Some(uvs[fi][k][0]);
                }
            }
        }
        let u = front.unwrap();
        assert!((u - 0.5).abs() < 0.1, "{s:?}: u at the front is {u}");
    }
}

#[test]
fn bevelled_fronts_stay_flat() {
    // A logo with rounded edges: its front's normals point straight out, right up to the bevel
    // (they used to lean with the bevel's first strip, shading the whole face in a gradient).
    let m = shape_mesh(&Shape3d::Extrude { d: "M0 0 L100 0 L100 100 L0 100 Z".into(), size: 2.0, depth: 0.4, bevel: 0.05 }).unwrap();
    let t = m.triangulate();
    let front = t.positions.iter().zip(&t.normals).filter(|(p, _)| (p[2] - 0.2).abs() < 1e-4 && p[0].abs() < 0.96 && p[1].abs() < 0.96);
    let mut seen = 0;
    for (p, n) in front {
        seen += 1;
        assert!(n[2] > 0.998, "front corner at {p:?} leans: {n:?}");
    }
    assert!(seen >= 4, "front corners found: {seen}");
}

#[test]
fn mesh_shapes_round_trip() {
    let m = cube(1.0);
    let shape = m.to_shape();
    let back = shape_mesh(&shape).unwrap();
    assert_eq!(back.faces, m.faces);
    assert_eq!(back.positions, m.positions);
    // Bad input is cleaned, not trusted.
    let bad = Shape3d::Mesh { vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], faces: vec![vec![0, 1, 2], vec![0, 1, 7], vec![0, 0, 1]], uvs: vec![vec![[0.0; 2]]], auto_smooth: 30.0 };
    let m = shape_mesh(&bad).unwrap();
    assert_eq!(m.faces.len(), 1);
    well_formed(&m);
}

#[test]
fn from_triangles_welds() {
    let c = cube(1.0);
    let back = PolyMesh::from_triangles(&c.triangulate());
    assert_eq!(back.positions.len(), 8);
    assert_eq!(back.faces.len(), 12);
    assert!(back.is_closed());
    assert!(close(back.volume(), 1.0, 1e-6));
}

#[test]
fn picking_finds_what_is_under_the_ray() {
    let c = cube(2.0);
    let (t, f) = c.raycast([0.0, 0.0, 5.0], [0.0, 0.0, -1.0]).unwrap();
    assert!((t - 4.0).abs() < 1e-9);
    assert!(dot(c.face_normal(f), [0.0, 0.0, 1.0]) > 0.999);
    assert!(c.raycast([0.0, 5.0, 5.0], [0.0, 0.0, -1.0]).is_none());
    // The front-top-right corner, not the one hidden behind it.
    let v = c.nearest_vertex([1.0, 1.0, 5.0], [0.0, 0.0, -1.0], 0.1).unwrap();
    assert_eq!(c.positions[v as usize], [1.0, 1.0, 1.0]);
    let (a, b) = c.nearest_edge([0.0, 1.02, 5.0], [0.0, 0.0, -1.0], 0.1).unwrap();
    let (pa, pb) = (c.positions[a as usize], c.positions[b as usize]);
    assert!(pa[1] == 1.0 && pb[1] == 1.0 && pa[2] == 1.0 && pb[2] == 1.0, "{pa:?} {pb:?}");
    assert!(c.nearest_vertex([5.0, 5.0, 5.0], [0.0, 0.0, -1.0], 0.1).is_none());
}

// ---- modifiers ------------------------------------------------------------------------------

#[test]
fn subdivision_rounds_and_keeps_closed() {
    let c = cube(1.0);
    let s = run(&c, "subdivision", json!({"levels": 2}), 0.0);
    assert_eq!(s.faces.len(), 6 * 16);
    assert!(s.is_closed());
    well_formed(&s);
    let r = s.positions.iter().map(|p| len(*p)).fold(0.0, f64::max);
    assert!(r < 0.8, "corners pulled in: {r}");
    let simple = run(&c, "subdivision", json!({"levels": 2, "simple": true}), 0.0);
    assert!(close(simple.volume(), 1.0, 1e-9), "simple keeps the shape");
    // Level 3 of a box is quick, and big meshes stop instead of exploding.
    let start = std::time::Instant::now();
    let l3 = run(&c, "subdivision", json!({"levels": 3}), 0.0);
    assert_eq!(l3.faces.len(), 6 * 64);
    assert!(start.elapsed().as_secs_f64() < 1.0);
    // Many faces (cheap ones): a level would pass the cap, so it stops before building it.
    let many = PolyMesh::new(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![vec![0, 1, 2]; MAX_FACES / 3 + 1]);
    let capped = run(&many, "subdivision", json!({"levels": 4}), 0.0);
    assert_eq!(capped.faces.len(), many.faces.len());
}

#[test]
fn mirror_doubles_and_welds() {
    let mut half = cube(1.0);
    for p in &mut half.positions {
        p[0] += 0.5;
    }
    let m = run(&half, "mirror", json!({"axis": "x"}), 0.0);
    // Two boxes sharing the x = 0 face: the shared faces go, the rest welds into one box.
    assert_eq!(m.faces.len(), 10);
    assert!(m.is_closed());
    assert!(close(m.volume(), 2.0, 1e-9));
    let (lo, hi) = m.bounds();
    assert!(close(lo[0], -1.0, 1e-9) && close(hi[0], 1.0, 1e-9));
    // Bisect cuts away what crosses the plane first.
    let centred = cube(1.0);
    let b = run(&centred, "mirror", json!({"axis": "x", "bisect": true}), 0.0);
    assert!(b.is_closed(), "bisected halves weld back into a box");
    assert!(close(b.volume(), 1.0, 1e-9));
    well_formed(&b);
}

#[test]
fn array_repeats_in_lines_and_circles() {
    let c = cube(1.0);
    let line = run(&c, "array", json!({"count": 4, "relative": [1.5, 0, 0]}), 0.0);
    assert_eq!(line.faces.len(), 24);
    let (lo, hi) = line.bounds();
    assert!(close(hi[0] - lo[0], 1.0 + 3.0 * 1.5, 1e-9));
    assert!(close(line.volume(), 4.0, 1e-9));
    let merged = run(&c, "array", json!({"count": 3, "relative": [1, 0, 0], "merge": true}), 0.0);
    assert_eq!(merged.positions.len(), 16, "touching copies weld");
    // Radial: offset + rotation walks a circle back to where it started.
    let ring = run(&c, "array", json!({"count": 8, "relative": [0, 0, 0], "offset": [3, 0, 0], "rotation": [0, 45, 0]}), 0.0);
    assert_eq!(ring.faces.len(), 48);
    let (lo, hi) = ring.bounds();
    assert!(hi[0] - lo[0] > 6.0 && hi[2] - lo[2] > 6.0, "spread around: {lo:?} {hi:?}");
    let shrink = run(&c, "array", json!({"count": 3, "scale": 0.5}), 0.0);
    assert!(close(shrink.volume(), 1.0 + 0.125 + 0.125 * 0.125, 1e-9));
}

#[test]
fn bevel_rounds_boxes_cylinders_and_extrusions() {
    let c = cube(2.0);
    for segments in [1, 3] {
        let b = run(&c, "bevel", json!({"width": 0.2, "segments": segments}), 0.0);
        well_formed(&b);
        assert!(b.is_closed(), "bevelled box (segments {segments}) is watertight");
        assert!(b.volume() < 8.0 && b.volume() > 7.5, "volume {}", b.volume());
        faces_point_out(&b);
        let (lo, hi) = b.bounds();
        assert!(close(hi[0], 1.0, 1e-9) && close(lo[1], -1.0, 1e-9), "sides stay put");
    }
    // A chamfered box: 6 shrunk sides + 12 edge strips + 8 corner triangles.
    let ch = run(&c, "bevel", json!({"width": 0.2, "segments": 1}), 0.0);
    assert_eq!(ch.faces.len(), 26);
    // Cylinder: only the rims are sharp.
    let cyl = shape_mesh(&Shape3d::Cylinder { radius: 0.5, height: 1.0, segments: 32.0 }).unwrap();
    let b = run(&cyl, "bevel", json!({"width": 0.05, "segments": 2}), 0.0);
    well_formed(&b);
    assert!(b.is_closed(), "bevelled cylinder is watertight");
    assert!(b.volume() < cyl.volume() && b.volume() > cyl.volume() * 0.95);
    // Hexagonal prism: every edge is sharp.
    let hex = shape_mesh(&Shape3d::Cylinder { radius: 0.5, height: 1.0, segments: 6.0 }).unwrap();
    let b = run(&hex, "bevel", json!({"width": 0.05, "segments": 3}), 0.0);
    assert!(b.is_closed());
    assert!(b.volume() < hex.volume() && b.volume() > hex.volume() * 0.9);
    // Extruded star.
    let star = shape_mesh(&Shape3d::Extrude { d: "M50 0 L61 35 L98 35 L68 57 L79 91 L50 70 L21 91 L32 57 L2 35 L39 35 Z".into(), size: 2.0, depth: 0.4, bevel: 0.0 }).unwrap();
    let b = run(&star, "bevel", json!({"width": 0.03, "segments": 2}), 0.0);
    well_formed(&b);
    assert!(b.is_closed(), "bevelled extrusion is watertight");
    assert!(b.volume() < star.volume() && b.volume() > star.volume() * 0.85);
    // Too wide is clamped, not inside out.
    let wide = run(&c, "bevel", json!({"width": 5.0, "segments": 2}), 0.0);
    assert!(wide.volume() > 0.0);
    well_formed(&wide);
}

#[test]
fn solidify_gives_thickness() {
    let plane = shape_mesh(&Shape3d::Plane { width: 1.0, height: 1.0 }).unwrap();
    let slab = run(&plane, "solidify", json!({"thickness": 0.1}), 0.0);
    assert!(slab.is_closed(), "rim closes the sides");
    assert!(close(slab.volume(), 0.1, 1e-9), "{}", slab.volume());
    let out = run(&plane, "solidify", json!({"thickness": -0.1}), 0.0);
    assert!(close(out.volume(), 0.1, 1e-9), "outward still faces out: {}", out.volume());
    let open = run(&plane, "solidify", json!({"thickness": 0.1, "rim": false}), 0.0);
    assert_eq!(open.faces.len(), 2);
    let shell = run(&cube(2.0), "solidify", json!({"thickness": 0.1}), 0.0);
    assert!(close(shell.volume(), 8.0 - 1.8f64.powi(3), 1e-6), "even thickness: {}", shell.volume());
}

#[test]
fn deformers_move_vertices() {
    let grid = shape_mesh(&Shape3d::Grid { width: 2.0, height: 2.0, rows: 16.0, cols: 16.0 }).unwrap();
    // Displace: bumps, different with evolution and seed, along the normal (up for a grid).
    let d = run(&grid, "displace", json!({"strength": 0.5, "scale": 0.5}), 0.0);
    assert!(d.positions.iter().any(|p| p[1].abs() > 0.01));
    assert!(d.positions.iter().zip(&grid.positions).all(|(a, b)| a[0] == b[0] && a[2] == b[2]));
    let e = run(&grid, "displace", json!({"strength": 0.5, "scale": 0.5, "evolution": 0.7}), 0.0);
    assert_ne!(d.positions, e.positions);
    // Wave moves with time.
    let w0 = run(&grid, "wave", json!({"amplitude": 0.2}), 0.0);
    let w1 = run(&grid, "wave", json!({"amplitude": 0.2}), 0.25);
    assert_ne!(w0.positions, w1.positions);
    assert!(w0.positions.iter().all(|p| p[1].abs() <= 0.2 + 1e-9));
    let ripple = run(&grid, "wave", json!({"amplitude": 0.2, "radial": true}), 0.0);
    let (a, b) = (ripple.positions.iter().find(|p| close(p[0], 0.5, 1e-9) && p[2].abs() < 1e-9).unwrap(), ripple.positions.iter().find(|p| p[0].abs() < 1e-9 && close(p[2], 0.5, 1e-9)).unwrap());
    assert!((a[1] - b[1]).abs() < 1e-9, "ripples are round");
    // Twist, bend and taper on a tall box.
    let tall = shape_mesh(&Shape3d::Box { size: Vec3([0.5, 2.0, 0.5]), bevel: 0.0 }).unwrap();
    // Twisted 90° end to end: each end turns 45°, so the top corners land on the axes.
    let tw = run(&tall, "twist", json!({"angle": 90}), 0.0);
    for p in tw.positions.iter().filter(|p| p[1].abs() > 0.99) {
        assert!(p[0].abs().min(p[2].abs()) < 1e-9 && close(p[0].abs().max(p[2].abs()), 0.125f64.sqrt(), 1e-9), "{p:?}");
    }
    // Bent 90° towards x: the ends lean over, each turned 45°, on an arc of radius 2/(π/2).
    let bent = run(&tall, "bend", json!({"angle": 90, "axis": "y", "toward": "x"}), 0.0);
    let r = 2.0 / (PI / 2.0);
    let top: Vec<&[f64; 3]> = bent.positions.iter().filter(|p| p[1] > 0.0).collect();
    let mean = top.iter().fold([0.0; 3], |a, p| add(a, **p)).map(|v| v / top.len() as f64);
    assert!(close(mean[0], r * (1.0 - (PI / 4.0).cos()), 1e-6) && close(mean[1], r * (PI / 4.0).sin(), 1e-6), "{mean:?}");
    let tp = run(&tall, "taper", json!({"amount": 1.0}), 0.0);
    let top_width = tp.positions.iter().filter(|p| p[1] > 0.99).map(|p| p[0].abs()).fold(0.0, f64::max);
    assert!(top_width < 1e-9, "a point at the top");
    // Smooth shrinks a box towards a ball; spherify makes it round; jitter moves and boils.
    let sm = run(&cube(1.0), "smooth", json!({"factor": 0.5, "iterations": 5}), 0.0);
    assert!(sm.volume() < 1.0);
    let ball = run(&run(&cube(1.0), "subdivision", json!({"levels": 2, "simple": true}), 0.0), "spherify", json!({"factor": 1.0}), 0.0);
    let radii: Vec<f64> = ball.positions.iter().map(|p| len(*p)).collect();
    let (rlo, rhi) = radii.iter().fold((f64::MAX, f64::MIN), |(a, b), r| (a.min(*r), b.max(*r)));
    assert!(rhi - rlo < 1e-9);
    let j0 = run(&cube(1.0), "noise", json!({"amount": 0.05, "speed": 2}), 0.0);
    let j1 = run(&cube(1.0), "noise", json!({"amount": 0.05, "speed": 2}), 0.1);
    let j2 = run(&cube(1.0), "noise", json!({"amount": 0.05, "speed": 2}), 0.6);
    assert_eq!(j0.positions, j1.positions, "same boil frame");
    assert_ne!(j0.positions, j2.positions, "next boil frame");
    assert!(j0.positions.iter().zip(&cube(1.0).positions).all(|(a, b)| dist(*a, *b) <= 0.05 * 3f64.sqrt() + 1e-12));
}

#[test]
fn wireframe_triangulate_decimate_weld() {
    let c = cube(1.0);
    let w = run(&c, "wireframe", json!({"thickness": 0.05}), 0.0);
    assert_eq!(w.faces.len(), 12 * 6);
    assert!(w.volume() > 0.0, "bars face out");
    well_formed(&w);
    let t = run(&c, "triangulate", json!({}), 0.0);
    assert_eq!(t.faces.len(), 12);
    assert_eq!(t.smooth_angle, 0.0);
    let sphere = shape_mesh(&Shape3d::Sphere { radius: 1.0, segments: 32.0 }).unwrap();
    let tri_count = sphere.triangulate().indices.len() / 3;
    let d = run(&sphere, "decimate", json!({"ratio": 0.25}), 0.0);
    well_formed(&d);
    assert!(d.faces.len() as f64 <= tri_count as f64 * 0.3, "{} of {tri_count}", d.faces.len());
    assert!(d.is_closed(), "decimation keeps it closed");
    assert!(close(d.volume(), sphere.volume(), 0.1), "keeps the shape: {} vs {}", d.volume(), sphere.volume());
    assert!(run(&sphere, "decimate", json!({"ratio": 0.0}), 0.0).faces.is_empty());
    // Weld: a box drawn with split corners becomes one piece.
    let split = PolyMesh::from_triangles(&c.triangulate());
    let mut loose = split.clone();
    loose.positions.iter_mut().for_each(|p| p[0] += 0.0);
    let mut apart = PolyMesh::default();
    for f in &loose.faces {
        let base = apart.positions.len() as u32;
        apart.positions.extend(f.iter().map(|&v| loose.positions[v as usize]));
        apart.faces.push((0..f.len() as u32).map(|k| base + k).collect());
    }
    let welded = run(&apart, "weld", json!({"distance": 0.001}), 0.0);
    assert_eq!(welded.positions.len(), 8);
    assert!(welded.is_closed());
}

#[test]
fn boolean_of_two_boxes() {
    let a = cube(2.0);
    let mut b = cube(2.0);
    for p in &mut b.positions {
        *p = add(*p, [1.0, 1.0, 1.0]);
    }
    let other = b.clone();
    let lookup = move |id: &str| (id == "cutter").then(|| other.clone());
    let with = |op: &str| apply_modifiers(a.clone(), &[modifier("boolean", json!({"object": "cutter", "operation": op}))], 0.0, &lookup);
    let diff = with("difference");
    assert!(close(diff.volume(), 8.0 - 1.0, 1e-6), "difference {}", diff.volume());
    let uni = with("union");
    assert!(close(uni.volume(), 8.0 + 8.0 - 1.0, 1e-6), "union {}", uni.volume());
    let int = with("intersect");
    assert!(close(int.volume(), 1.0, 1e-6), "intersect {}", int.volume());
    for m in [&diff, &uni, &int] {
        well_formed(m);
    }
    // A missing object leaves the mesh alone.
    let none = apply_modifiers(a.clone(), &[modifier("boolean", json!({"object": "nope"}))], 0.0, &|_| None);
    assert_eq!(none, a);
    // A sphere carved out of a box.
    let sphere = shape_mesh(&Shape3d::Sphere { radius: 0.8, segments: 24.0 }).unwrap();
    let carved = super::csg::boolean(&cube(1.0), &sphere, super::csg::BoolOp::Difference);
    assert!(carved.volume() > 0.0 && carved.volume() < 1.0);
}

#[test]
fn solidify_after_a_boolean_stays_even() {
    // A ball with a bite taken out, then given a thickness either way: the rim's corners move to
    // where the offset surfaces meet, not off along a normal that belongs to neither side.
    let ball = shape_mesh(&Shape3d::Sphere { radius: 0.8, segments: 32.0 }).unwrap();
    let mut bite = shape_mesh(&Shape3d::Sphere { radius: 0.75, segments: 32.0 }).unwrap();
    for p in &mut bite.positions {
        *p = add(*p, [0.5, 0.5, 0.5]);
    }
    let cut = super::csg::boolean(&ball, &bite, super::csg::BoolOp::Difference);
    // T-junctions mended: the cracks along every split are closed (a sliver or two folded onto a
    // neighbour may remain).
    let edges = cut.edges();
    let odd = edges.iter().filter(|e| e.faces.len() != 2).count();
    assert!(odd * 500 < edges.len(), "{odd} of {} edges aren't shared by two faces", edges.len());
    well_formed(&cut);
    let n = cut.positions.len();
    for t in [0.1, -0.1] {
        let shell = run(&cut, "solidify", json!({"thickness": t}), 0.0);
        let worst = shell.positions.iter().skip(n).zip(&cut.positions).map(|(p, q)| len(sub(*p, *q))).fold(0.0, f64::max);
        assert!(worst < 0.1 * 2.2, "a vertex moved {worst} for a thickness of {t}");
    }
}

#[test]
fn explode_build_and_disabled() {
    let s = shape_mesh(&Shape3d::Icosphere { radius: 1.0, detail: 1.0 }).unwrap();
    let still = run(&s, "explode", json!({"progress": 0}), 0.0);
    assert_eq!(still, s);
    let e = run(&s, "explode", json!({"progress": 1, "distance": 2}), 0.0);
    assert_eq!(e.faces.len(), s.faces.len());
    assert!(e.positions.iter().map(|p| len(*p)).sum::<f64>() / e.positions.len() as f64 > 1.8, "pieces flew out");
    let g = run(&s, "explode", json!({"progress": 1, "distance": 0, "gravity": 3, "spin": 0}), 0.0);
    let (_, hi) = g.bounds();
    assert!(hi[1] < 1.0 - 2.0, "pieces fell: {hi:?}");
    let half = run(&s, "build", json!({"progress": 0.5, "order": "y"}), 0.0);
    assert_eq!(half.faces.len(), s.faces.len() / 2);
    let lowest_half = half.bounds().1[1];
    assert!(lowest_half < 0.6, "bottom first: {lowest_half}");
    let rev = run(&s, "build", json!({"progress": 0.25, "reverse": true}), 0.0);
    assert_eq!(rev.faces.len(), s.faces.len() - s.faces.len() / 4);
    assert!(run(&s, "build", json!({"progress": 0}), 0.0).faces.is_empty());
    let mut off = modifier("subdivision", json!({"levels": 2}));
    off.enabled = false;
    assert_eq!(apply_modifiers(s.clone(), &[off], 0.0, &|_| None), s);
}

#[test]
fn modifiers_survive_odd_input() {
    let nasty = PolyMesh::new(vec![[0.0; 3], [f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![vec![0, 1, 2], vec![0, 2, 3], vec![0, 2, 99], vec![3, 3, 3]]);
    for spec in stack_types() {
        let out = run(&nasty, spec, json!({}), f64::NAN);
        well_formed(&out);
        let out = run(&PolyMesh::default(), spec, json!({}), 0.0);
        assert!(out.faces.is_empty() || spec == "boolean");
    }
}

fn stack_types() -> Vec<&'static str> {
    crate::motion::stack::MODIFIERS.iter().map(|t| t.name).collect()
}

// ---- edit operations ------------------------------------------------------------------------

fn top_face(m: &PolyMesh) -> Selection {
    ops::select_facing(m, [0.0, 1.0, 0.0], 1.0)
}

#[test]
fn extrude_region_and_individual() {
    let mut m = cube(1.0);
    let sel = top_face(&m);
    assert_eq!(sel.faces.len(), 1);
    let out = ops::extrude(&mut m, &sel, 1.0, None).unwrap();
    assert_eq!(m.faces.len(), 10);
    assert!(m.is_closed());
    assert!(close(m.volume(), 2.0, 1e-9));
    assert_eq!(out.faces.len(), 1);
    assert!(m.face_points(out.faces[0] as usize).iter().all(|p| close(p[1], 1.5, 1e-9)), "the moved face is selected");
    // Two neighbouring faces extruded as one region share their walls.
    let mut m = cube(1.0);
    let sel = ops::select_facing(&m, [1.0, 1.0, 0.0], 50.0);
    assert_eq!(sel.faces.len(), 2);
    ops::extrude(&mut m, &sel, 0.5, Some([0.5, 0.5, 0.0])).unwrap();
    assert!(m.is_closed());
    assert_eq!(m.faces.len(), 6 + 6);
    let mut m = cube(1.0);
    let sel = ops::select_facing(&m, [1.0, 1.0, 0.0], 50.0);
    ops::extrude_individual(&mut m, &sel, 0.5).unwrap();
    assert_eq!(m.faces.len(), 6 + 8);
    assert!(close(m.volume(), 2.0, 1e-9));
    // Edges: a plane's border edge pulled into a new face.
    let mut p = shape_mesh(&Shape3d::Plane { width: 1.0, height: 1.0 }).unwrap();
    let top: Vec<u32> = (0..4).filter(|&v| p.positions[v as usize][1] > 0.0).collect();
    let out = ops::extrude(&mut p, &Selection::of_vertices(top), 0.0, Some([0.0, 0.0, -1.0])).unwrap();
    assert_eq!(p.faces.len(), 2);
    assert_eq!(out.vertices.len(), 2);
    assert!(ops::extrude(&mut p, &Selection::none(), 1.0, None).is_err());
}

#[test]
fn inset_region_and_individual() {
    let mut m = cube(1.0);
    let sel = top_face(&m);
    let out = ops::inset(&mut m, &sel, 0.1, 0.0, false).unwrap();
    assert_eq!(m.faces.len(), 10);
    assert!(m.is_closed());
    assert!(close(m.volume(), 1.0, 1e-9));
    let inner = out.faces[0] as usize;
    assert!(close(m.face_area(inner), 0.64, 1e-9), "{}", m.face_area(inner));
    let mut m = cube(1.0);
    let sel = top_face(&m);
    ops::inset(&mut m, &sel, 0.1, -0.2, true).unwrap();
    // The ring slopes from the outer square down to the inner one: a frustum-shaped pocket.
    assert!(close(m.volume(), 1.0 - 0.2 / 3.0 * (1.0 + 0.64 + 0.8), 1e-9), "{}", m.volume());
    // Two side by side as a region: one ring around both.
    let mut g = shape_mesh(&Shape3d::Grid { width: 2.0, height: 1.0, rows: 1.0, cols: 2.0 }).unwrap();
    let all = Selection::all(&g);
    ops::inset(&mut g, &all, 0.1, 0.0, false).unwrap();
    assert_eq!(g.faces.len(), 2 + 6);
    assert!(close(g.area(), 2.0, 1e-9));
}

#[test]
fn bevel_subdivide_loop_cut() {
    let mut m = cube(1.0);
    let edge = Selection::of_vertices((0..8u32).filter(|&v| {
        let p = m.positions[v as usize];
        p[1] > 0.0 && p[2] > 0.0
    }));
    assert_eq!(edge.edge_list(&m).len(), 1);
    let out = ops::bevel(&mut m, &edge, 0.1, 2, false).unwrap();
    assert!(m.is_closed());
    assert_eq!(out.faces.len(), 2, "two segments");
    assert!(m.volume() < 1.0);
    let mut m = cube(1.0);
    let corner = Selection::of_vertices([0]);
    let out = ops::bevel(&mut m, &corner, 0.2, 1, true).unwrap();
    assert!(m.is_closed());
    assert_eq!(out.faces.len(), 1);
    assert!(close(m.volume(), 1.0 - 0.2f64.powi(3) / 6.0, 1e-9), "{}", m.volume());
    // Subdivide: the top quad into 9, neighbours get the new edge points.
    let mut m = cube(1.0);
    let sel = top_face(&m);
    let out = ops::subdivide(&mut m, &sel, 2).unwrap();
    assert_eq!(out.faces.len(), 9);
    assert!(m.is_closed());
    assert!(close(m.volume(), 1.0, 1e-9));
    let mut t = PolyMesh::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![vec![0, 1, 2]]);
    let out = { let all = Selection::all(&t); ops::subdivide(&mut t, &all, 3).unwrap() };
    assert_eq!(out.faces.len(), 16);
    assert!(close(t.area(), 0.5, 1e-9));
    // Loop cut around a cylinder's side: a new ring of vertices.
    let mut cyl = shape_mesh(&Shape3d::Cylinder { radius: 0.5, height: 1.0, segments: 8.0 }).unwrap();
    let vertical = cyl.edges().into_iter().find(|e| (cyl.positions[e.a as usize][1] - cyl.positions[e.b as usize][1]).abs() > 0.9).unwrap();
    let out = ops::loop_cut(&mut cyl, &Selection::of_vertices([vertical.a, vertical.b]), 1, 0.0).unwrap();
    assert_eq!(out.vertices.len(), 8);
    assert!(out.vertices.iter().all(|&v| cyl.positions[v as usize][1].abs() < 1e-9));
    assert!(cyl.is_closed());
    assert_eq!(cyl.faces.len(), 16 + 2);
    // A grid's open ring, slid towards one side.
    let mut g = shape_mesh(&Shape3d::Grid { width: 1.0, height: 1.0, rows: 2.0, cols: 2.0 }).unwrap();
    let e = g.edges().into_iter().find(|e| {
        let (a, b) = (g.positions[e.a as usize], g.positions[e.b as usize]);
        a[2] == -0.5 && b[2] == -0.5
    });
    let e = e.unwrap();
    let out = ops::loop_cut(&mut g, &Selection::of_vertices([e.a, e.b]), 2, 0.0).unwrap();
    assert_eq!(out.vertices.len(), 6);
    assert_eq!(g.faces.len(), 4 + 4);
    assert!(close(g.area(), 1.0, 1e-9));
}

#[test]
fn loop_cut_across_an_edge_ring_selection() {
    // On a box an edge ring's vertices are all eight corners: the cut must still cross the ring
    // that was asked for (new vertices along x for an edge along x), not any ring.
    let mut m = cube(2.0);
    let along_x = m.edges().into_iter().find(|e| {
        let (a, b) = (m.positions[e.a as usize], m.positions[e.b as usize]);
        (a[0] - b[0]).abs() > 1.0 && a[1] > 0.0 && a[2] > 0.0
    });
    let e = along_x.unwrap();
    let pick: ops::Select = serde_json::from_value(json!({"edgeRing": [e.a, e.b]})).unwrap();
    let sel = pick.resolve_for(&m, "loopCut").unwrap();
    let op: ops::EditOp = serde_json::from_value(json!({"type": "loopCut", "cuts": 2})).unwrap();
    let out = ops::apply(&mut m, &sel, &op).unwrap();
    assert_eq!(out.vertices.len(), 8);
    for &v in &out.vertices {
        let x = m.positions[v as usize][0];
        assert!(close(x.abs(), 1.0 / 3.0, 1e-9), "cut across x: {:?}", m.positions[v as usize]);
    }
    assert!(m.is_closed());
    // Corners of several rings at once can't say where to cut.
    let mut m = cube(2.0);
    let all = Selection::all(&m);
    let err = ops::loop_cut(&mut m, &all, 1, 0.0).unwrap_err();
    assert!(err.contains("one ring"), "{err}");
}

#[test]
fn delete_dissolve_merge() {
    let mut m = cube(1.0);
    { let sel = top_face(&m); ops::delete(&mut m, &sel, ops::Element::Faces).unwrap(); }
    assert_eq!(m.faces.len(), 5);
    assert_eq!(m.positions.len(), 8);
    let mut m = cube(1.0);
    ops::delete(&mut m, &Selection::of_vertices([0]), ops::Element::Vertices).unwrap();
    assert_eq!(m.faces.len(), 3);
    assert_eq!(m.positions.len(), 7);
    // Dissolving the middle edge of two quads makes one face.
    let mut g = shape_mesh(&Shape3d::Grid { width: 2.0, height: 1.0, rows: 1.0, cols: 2.0 }).unwrap();
    let mid: Vec<u32> = (0..g.positions.len() as u32).filter(|&v| g.positions[v as usize][0].abs() < 1e-9).collect();
    let out = ops::dissolve(&mut g, &Selection::of_vertices(mid.clone()), ops::Element::Edges).unwrap();
    assert_eq!(g.faces.len(), 1);
    assert_eq!(g.faces[0].len(), 6);
    assert_eq!(out.faces, vec![0]);
    let mut g = shape_mesh(&Shape3d::Grid { width: 2.0, height: 1.0, rows: 1.0, cols: 2.0 }).unwrap();
    ops::dissolve(&mut g, &Selection::of_vertices(mid), ops::Element::Vertices).unwrap();
    assert_eq!(g.faces.len(), 1);
    assert_eq!(g.faces[0].len(), 4, "the middle vertices are gone");
    let mut g = shape_mesh(&Shape3d::Grid { width: 2.0, height: 2.0, rows: 2.0, cols: 2.0 }).unwrap();
    { let all = Selection::all(&g); ops::dissolve(&mut g, &all, ops::Element::Faces).unwrap(); }
    assert_eq!(g.faces.len(), 1);
    assert!(close(g.area(), 4.0, 1e-9));
    // Merge the top four corners of a box at their centre: a pyramid.
    let mut m = cube(1.0);
    let top: Vec<u32> = (0..8).filter(|&v| m.positions[v as usize][1] > 0.0).collect();
    let out = ops::merge(&mut m, &Selection::of_vertices(top), ops::MergeAt::Center).unwrap();
    assert_eq!(out.vertices.len(), 1);
    assert_eq!(m.positions.len(), 5);
    assert_eq!(m.faces.len(), 5);
    assert!(close(m.volume(), 1.0 / 3.0, 1e-9));
    let mut m = cube(1.0);
    let all = Selection::all(&m);
    ops::merge(&mut m, &all, ops::MergeAt::Distance(0.01)).unwrap();
    assert_eq!(m.positions.len(), 8, "nothing that close");
}

#[test]
fn fill_bridge_and_normals() {
    // Delete a box's top, then fill the hole back.
    let mut m = cube(1.0);
    { let sel = top_face(&m); ops::delete(&mut m, &sel, ops::Element::Faces).unwrap(); }
    let top = Selection::of_vertices((0..8).filter(|&v| m.positions[v as usize][1] > 0.0));
    ops::fill(&mut m, &top).unwrap();
    assert!(m.is_closed());
    assert!(close(m.volume(), 1.0, 1e-9), "filled facing out");
    // Nothing open to fill on a closed box: no duplicate face, no face through the middle.
    for sel in [top_face(&m), Selection::all(&m)] {
        let mut c = m.clone();
        assert!(ops::fill(&mut c, &sel).unwrap_err().contains("nothing open"));
        assert_eq!(c.faces.len(), m.faces.len());
    }
    // Bridge two facing squares into a tube.
    let mut m = cube(1.0);
    let mut far = cube(1.0);
    for p in &mut far.positions {
        p[1] += 3.0;
    }
    m.append(&far);
    let facing = Selection::of_faces((0..m.faces.len() as u32).filter(|&f| {
        let (n, c) = (m.face_normal(f as usize), m.face_center(f as usize));
        (n[1] > 0.99 && c[1] < 1.0) || (n[1] < -0.99 && c[1] > 2.0)
    }));
    assert_eq!(facing.faces.len(), 2);
    let out = ops::bridge(&mut m, &facing).unwrap();
    assert_eq!(out.faces.len(), 4);
    assert!(m.is_closed());
    assert!(close(m.volume(), 4.0, 1e-9), "{}", m.volume());
    // Flip and recalculate.
    let mut m = cube(1.0);
    { let sel = top_face(&m); ops::flip(&mut m, &sel).unwrap(); }
    assert!(m.volume() < 1.0);
    ops::recalc_normals(&mut m, &Selection::none()).unwrap();
    assert!(close(m.volume(), 1.0, 1e-9));
    let mut inside_out = cube(1.0);
    for f in 0..6 {
        inside_out.flip_face(f);
    }
    ops::recalc_normals(&mut inside_out, &Selection::none()).unwrap();
    assert!(close(inside_out.volume(), 1.0, 1e-9));
}

#[test]
fn transforms_duplicate_poke_smooth() {
    let mut m = cube(1.0);
    let sel = top_face(&m);
    ops::translate(&mut m, &sel, [0.0, 1.0, 0.0]).unwrap();
    assert!(close(m.volume(), 2.0, 1e-9));
    ops::scale_sel(&mut m, &sel, [2.0, 1.0, 2.0], None).unwrap();
    let (lo, hi) = m.bounds();
    assert!(close(hi[0] - lo[0], 2.0, 1e-9));
    { let all = Selection::all(&m); ops::rotate_sel(&mut m, &all, [0.0, 90.0, 0.0], Some([0.0; 3])).unwrap(); }
    assert!(m.volume() > 0.0);
    let mut c = cube(1.0);
    let all = Selection::all(&c);
    ops::mirror_sel(&mut c, &all, 0, Some([1.0, 0.0, 0.0])).unwrap();
    assert!(close(c.volume(), 1.0, 1e-9), "mirrored but still facing out");
    assert!(close(c.bounds().0[0], 1.5, 1e-9));
    let mut d = cube(1.0);
    let out = { let all = Selection::all(&d); ops::duplicate(&mut d, &all, [2.0, 0.0, 0.0]).unwrap() };
    assert_eq!(d.faces.len(), 12);
    assert_eq!(out.faces.len(), 6);
    assert!(close(d.volume(), 2.0, 1e-9));
    let mut p = cube(1.0);
    let sel = top_face(&p);
    let out = ops::poke(&mut p, &sel, 0.5).unwrap();
    assert_eq!(p.faces.len(), 9);
    assert!(close(p.positions[out.vertices[0] as usize][1], 1.0, 1e-9));
    assert!(close(p.volume(), 1.0 + 1.0 * 0.5 / 3.0, 1e-9));
    let mut t = cube(1.0);
    { let all = Selection::all(&t); ops::triangulate(&mut t, &all).unwrap(); }
    assert_eq!(t.faces.len(), 12);
    let mut s = run(&cube(1.0), "subdivision", json!({"levels": 1, "simple": true}), 0.0);
    let before = s.volume();
    let all = Selection::all(&s);
    ops::smooth_vertices(&mut s, &all, 0.5, 3).unwrap();
    assert!(s.volume() < before);
}

#[test]
fn spin_and_knife() {
    // Spin a square standing beside the axis all the way round: a ring with a square section.
    let mut m = PolyMesh::new(vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], [1.0, 1.0, 0.0]], vec![vec![0, 1, 2, 3]]);
    { let all = Selection::all(&m); ops::spin(&mut m, &all, 360.0, 32, [0.0, 1.0, 0.0], [0.0; 3]).unwrap(); }
    assert!(m.is_closed(), "a full turn closes");
    let expected = PI * (4.0 - 1.0);
    assert!(close(m.volume().abs(), expected, 0.02), "{} vs {expected}", m.volume());
    // Half a turn keeps both ends capped.
    let mut h = PolyMesh::new(vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], [1.0, 1.0, 0.0]], vec![vec![0, 1, 2, 3]]);
    { let all = Selection::all(&h); ops::spin(&mut h, &all, 180.0, 16, [0.0, 1.0, 0.0], [0.0; 3]).unwrap(); }
    assert!(h.is_closed(), "capped at both ends");
    assert!(close(h.volume().abs(), expected / 2.0, 0.02));
    // Edges: a profile swept into a surface.
    let mut e = PolyMesh::new(vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], vec![vec![0, 1, 2]]);
    let edge = Selection::of_vertices([0, 1]);
    ops::spin(&mut e, &edge, 360.0, 12, [0.0, 1.0, 0.0], [0.0; 3]).unwrap();
    assert_eq!(e.faces.len(), 1 + 12);
    // A whole closed solid has no border to sweep: an error, not an emptied mesh.
    let mut solid = cube(1.0);
    let all = Selection::all(&solid);
    assert!(ops::spin(&mut solid, &all, 360.0, 12, [0.0, 1.0, 0.0], [0.0; 3]).unwrap_err().contains("border"));
    assert_eq!(solid.faces.len(), 6, "left as it was");
    // Knife across a box's middle: every side face splits.
    let mut k = cube(1.0);
    let out = ops::knife(&mut k, &Selection::none(), [0.0, 0.1, 0.0], [0.0, 1.0, 0.0]).unwrap();
    assert_eq!(out.vertices.len(), 4);
    assert_eq!(k.faces.len(), 10);
    assert!(k.is_closed());
    assert!(close(k.volume(), 1.0, 1e-9));
    well_formed(&k);
}

#[test]
fn unwrap_methods() {
    for (how, name) in [(ops::Unwrap::Box, "box"), (ops::Unwrap::Cylinder, "cylinder"), (ops::Unwrap::Sphere, "sphere"), (ops::Unwrap::Planar, "planar")] {
        let mut m = shape_mesh(&Shape3d::Cylinder { radius: 0.5, height: 1.0, segments: 16.0 }).unwrap();
        m.uvs = None;
        ops::unwrap(&mut m, &Selection::none(), how).unwrap();
        well_formed(&m);
        let uv = m.uvs.as_ref().unwrap();
        assert!(uv.iter().flatten().all(|c| c[0].is_finite() && c[1].is_finite()), "{name}");
        if how == ops::Unwrap::Planar || how == ops::Unwrap::Cylinder {
            assert!(uv.iter().flatten().all(|c| c[1] >= -1e-9 && c[1] <= 1.0 + 1e-9), "{name}");
        }
    }
}

#[test]
fn edge_paths_preserve_exact_edges_through_selection_and_extrusion() {
    let positions=(0..3).flat_map(|z| (0..3).map(move |x| [x as f64,0.,z as f64])).collect();
    let grid=PolyMesh::new(positions,vec![vec![0,1,4,3],vec![1,2,5,4],vec![3,4,7,6],vec![4,5,8,7]]);
    let loop_selection=ops::edge_loop(&grid,1,4);
    assert_eq!(loop_selection.edges,[(1,4),(4,7)]);
    assert_eq!(loop_selection.vertices,[1,4,7]);
    let ring=ops::edge_ring(&grid,1,4);
    assert_eq!(ring.edges,[(0,3),(1,4),(2,5)]);
    assert_eq!(ring.vertices,[0,1,2,3,4,5]);
    assert!(ring.face_list(&grid).is_empty(),"touching every corner must not select faces");
    assert_eq!(ring.edge_list(&grid).len(),3,"horizontal connectors are not part of the ring");
    let spec:ops::Select=serde_json::from_value(json!({"edgeRing":[4,1]})).unwrap();
    assert_eq!(spec.resolve(&grid).unwrap(),ring,"JSON uses the same canonical edge selection");
    let mut extruded=grid.clone();
    ops::extrude(&mut extruded,&ring,1.,Some([0.,1.,0.])).unwrap();
    assert_eq!(extruded.faces.len(),grid.faces.len()+3,"one wall per selected ring edge");
    assert_eq!(&extruded.positions[..grid.positions.len()],grid.positions.as_slice(),"original faces do not move");
    well_formed(&extruded);
    let cube=cube(1.);
    let edge=&cube.edges()[0];
    let ring=ops::edge_ring(&cube,edge.a,edge.b);
    assert_eq!(ring.edges.len(),4);
    assert_eq!(ring.vertices.len(),8);
    assert!(ring.face_list(&cube).is_empty(),"a closed box ring is not the whole box");
    let loop_selection=ops::edge_loop(&cube,edge.a,edge.b);
    assert_eq!(loop_selection.edges.len(),1,"loops stop at three-valence corners");
    let mut junction=grid;
    junction.positions.extend([[0.5,1.,0.],[0.5,1.,1.]]);
    junction.faces.push(vec![1,4,10,9]);
    assert_eq!(ops::edge_ring(&junction,1,4).edges,[(1,4)],"a non-manifold seed does not choose an arbitrary branch");
    assert_eq!(ops::edge_loop(&junction,1,4).edges,[(1,4)]);
    assert!(ops::edge_ring(&junction,0,8).is_empty());
    assert!(ops::edge_loop(&junction,u32::MAX,0).is_empty());
}

#[test]
fn selection_helpers() {
    let c = cube(1.0);
    assert_eq!(ops::select_facing(&c, [0.0, 1.0, 0.0], 10.0).faces.len(), 1);
    assert_eq!(ops::select_facing(&c, [0.0, 1.0, 0.0], 91.0).faces.len(), 5);
    let inside = ops::select_inside(&c, [-1.0, 0.0, -1.0], [1.0, 1.0, 1.0]);
    assert_eq!(inside.vertices.len(), 4);
    assert_eq!(inside.faces.len(), 1);
    assert_eq!(ops::faces_of_vertex(&c, 0).faces.len(), 3);
    let mut two = cube(1.0);
    let mut far = cube(1.0);
    far.positions.iter_mut().for_each(|p| p[0] += 5.0);
    two.append(&far);
    assert_eq!(ops::select_linked(&two, &Selection::of_vertices([0])).faces.len(), 6);
    // Loops and rings on a cylinder's side.
    let cyl = shape_mesh(&Shape3d::Cylinder { radius: 0.5, height: 1.0, segments: 8.0 }).unwrap();
    let mut cut = cyl.clone();
    let vertical = cyl.edges().into_iter().find(|e| (cyl.positions[e.a as usize][1] - cyl.positions[e.b as usize][1]).abs() > 0.9).unwrap();
    ops::loop_cut(&mut cut, &Selection::of_vertices([vertical.a, vertical.b]), 1, 0.0).unwrap();
    let mid: Vec<u32> = (0..cut.positions.len() as u32).filter(|&v| cut.positions[v as usize][1].abs() < 1e-9).collect();
    let a = mid[0];
    let b = *mid.iter().find(|&&b| cut.edges().iter().any(|e| (e.a, e.b) == (a.min(b), a.max(b)))).unwrap();
    assert_eq!(ops::edge_loop(&cut, a, b).vertices.len(), 8, "the loop goes round");
    assert_eq!(ops::edge_ring(&cyl, vertical.a, vertical.b).vertices.len(), 16, "the ring goes round");
    // JSON selections.
    let s: ops::Select = serde_json::from_value(json!({"facing": [0, 1, 0], "angle": 5})).unwrap();
    assert_eq!(s.resolve(&c).unwrap().faces.len(), 1);
    let s: ops::Select = serde_json::from_value(json!({"vertices": [99]})).unwrap();
    assert!(s.resolve(&c).unwrap_err().contains("no vertex 99"));
    assert!(serde_json::from_value::<ops::Select>(json!({"facin": [0, 1, 0]})).is_err());
}

#[test]
fn ops_from_json_with_hints() {
    let mut m = cube(1.0);
    let op: ops::EditOp = serde_json::from_value(json!({"type": "extrude", "distance": 2})).unwrap();
    let sel = top_face(&m);
    let out = ops::apply(&mut m, &sel, &op).unwrap();
    assert!(close(m.volume(), 3.0, 1e-9));
    assert_eq!(out.faces.len(), 1);
    let bad: ops::EditOp = serde_json::from_value(json!({"type": "extrud"})).unwrap();
    let err = ops::apply(&mut m, &sel, &bad).unwrap_err();
    assert!(err.contains("Did you mean `extrude`"), "{err}");
    let bad: ops::EditOp = serde_json::from_value(json!({"type": "inset", "thicknes": 2})).unwrap();
    assert!(ops::apply(&mut m, &sel, &bad).unwrap_err().contains("thickness"));
    // Every op runs on a box with everything selected (or reports a clear error).
    for spec in ops::EDIT_OPS {
        let mut c = cube(1.0);
        let all = Selection::all(&c);
        let op = ops::EditOp::new(spec.name, Default::default());
        match ops::apply(&mut c, &all, &op) {
            Ok(_) => well_formed(&c),
            Err(e) => assert!(!e.is_empty()),
        }
        let mut empty = PolyMesh::default();
        let _ = ops::apply(&mut empty, &Selection::none(), &op);
    }
}

use std::f64::consts::PI;

#[test]
fn bevel_of_some_edges_stays_watertight() {
    // One edge of a box, rounded: the side faces get notched by the bevel's ends.
    let mut m = cube(1.0);
    let edge = Selection::of_vertices((0..8u32).filter(|&v| {
        let p = m.positions[v as usize];
        p[1] > 0.0 && p[2] > 0.0
    }));
    ops::bevel(&mut m, &edge, 0.1, 8, false).unwrap();
    assert!(m.is_closed());
    faces_point_out(&m);
    // A quarter circle of radius 0.1 cut along a unit edge.
    let cut = 0.01 * (1.0 - PI / 4.0);
    assert!(close(m.volume(), 1.0 - cut, 1e-4), "{} vs {}", m.volume(), 1.0 - cut);
    // A crease ending inside a flat grid: the faces around its end get the new corners.
    let mut g = shape_mesh(&Shape3d::Grid { width: 4.0, height: 4.0, rows: 4.0, cols: 4.0 }).unwrap();
    let inner = |p: [f64; 3]| p[0].abs() < 1.5 && p[2].abs() < 1.5;
    let e = g.edges().into_iter().find(|e| inner(g.positions[e.a as usize]) && inner(g.positions[e.b as usize])).unwrap();
    let border_before = g.edges().iter().filter(|e| e.faces.len() == 1).count();
    ops::bevel(&mut g, &Selection::of_vertices([e.a, e.b]), 0.2, 3, false).unwrap();
    well_formed(&g);
    let border_after: f64 = g.edges().iter().filter(|e| e.faces.len() == 1).map(|e| dist(g.positions[e.a as usize], g.positions[e.b as usize])).sum();
    assert!(close(border_after, 16.0, 1e-9), "no cracks inside: {border_after} (was {border_before} edges)");
    assert!(close(g.area(), 16.0, 1e-9));
}

#[test]
fn explicit_edges_do_not_expand_to_incidental_edges_or_faces() {
    let mut mesh = PolyMesh::new(vec![[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]], vec![vec![0, 1, 2, 3]]);
    let pick = ops::Select { edges: vec![(1, 0), (2, 3), (0, 1)], ..Default::default() };
    let selection = pick.resolve(&mesh).unwrap();
    assert_eq!(selection.edge_list(&mesh), [(0, 1), (2, 3)]);
    assert!(selection.face_list(&mesh).is_empty(), "opposite edges must not select the face");
    assert_eq!(selection.vertex_set(&mesh).len(), 4);
    assert!(!selection.is_empty());
    let original = mesh.faces[0].clone();
    ops::extrude(&mut mesh, &selection, 1., Some([0., 0., 1.])).unwrap();
    assert_eq!(mesh.faces.len(), 3, "only the two selected edges get new faces");
    assert_eq!(mesh.faces[0], original, "the original face stays in place");
    assert_eq!(&mesh.positions[..4], &[[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]]);
    assert!(ops::Select { edges: vec![(0, 2)], ..Default::default() }.resolve(&mesh).is_err());
}
