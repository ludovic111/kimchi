//! Triangle meshes: the primitives, extruded 3D text and glTF models.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::TextStyle;
use kimchi_core::motion::Shape3d;

use super::math::V3;

#[derive(Debug, Clone, Default)]
pub(crate) struct Mesh {
    pub pos: Vec<[f32; 3]>,
    pub normal: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub index: Vec<u32>,
}

impl Mesh {
    pub(crate) fn push(&mut self, p: V3, n: V3, uv: [f32; 2]) -> u32 {
        self.pos.push(p.arr());
        self.normal.push(n.norm().arr());
        self.uv.push(uv);
        (self.pos.len() - 1) as u32
    }

    pub(crate) fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.index.extend([a, b, c]);
    }

    /// A rows×cols grid of vertices made by `f(u, v)` (0..1 each), two triangles per cell.
    pub(crate) fn grid(&mut self, rows: usize, cols: usize, f: impl Fn(f32, f32) -> (V3, V3)) {
        let base = self.pos.len() as u32;
        for r in 0..=rows {
            for c in 0..=cols {
                let (u, v) = (c as f32 / cols as f32, r as f32 / rows as f32);
                let (p, n) = f(u, v);
                self.push(p, n, [u, v]);
            }
        }
        let w = cols as u32 + 1;
        for r in 0..rows as u32 {
            for c in 0..cols as u32 {
                let i = base + r * w + c;
                self.tri(i, i + w, i + 1);
                self.tri(i + 1, i + w, i + w + 1);
            }
        }
    }

    pub(crate) fn bounds(&self) -> (V3, V3) {
        let mut lo = V3(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = V3(f32::MIN, f32::MIN, f32::MIN);
        for p in &self.pos {
            lo = lo.min(V3(p[0], p[1], p[2]));
            hi = hi.max(V3(p[0], p[1], p[2]));
        }
        if self.pos.is_empty() { (V3::default(), V3::default()) } else { (lo, hi) }
    }

    /// Recomputes flat normals (each triangle its own vertices).
    pub(crate) fn faceted(&self) -> Mesh {
        let mut out = Mesh::default();
        for t in self.index.as_chunks::<3>().0.iter() {
            let p: Vec<V3> = t.iter().map(|&i| V3(self.pos[i as usize][0], self.pos[i as usize][1], self.pos[i as usize][2])).collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]).norm();
            for (k, &i) in t.iter().enumerate() {
                let v = out.push(p[k], n, self.uv[i as usize]);
                out.index.push(v);
            }
        }
        out
    }
}

/// The mesh for a shape (models are loaded separately), shared between frames.
pub(crate) fn shape(s: &Shape3d) -> Option<Arc<Mesh>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Mesh>>>> = OnceLock::new();
    let key = serde_json::to_string(s).ok()?;
    let cache = CACHE.get_or_init(Default::default);
    if let Some(m) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(m.clone());
    }
    let mesh = match s {
        Shape3d::Box { size, bevel } => rounded_box(V3::from(size.0) * 0.5, *bevel as f32),
        Shape3d::Sphere { radius, segments: 0.0 } => sphere(*radius as f32),
        Shape3d::Cylinder { radius, height, segments: 0.0 } => cylinder(*radius as f32, *radius as f32, *height as f32),
        Shape3d::Cone { radius, height, segments: 0.0 } => cylinder(*radius as f32, 0.0, *height as f32),
        Shape3d::Torus { radius, tube } => torus(*radius as f32, *tube as f32),
        Shape3d::Plane { width, height } => plane(*width as f32, *height as f32),
        Shape3d::Text { text, font_family, font_weight, size, depth, align, letter_spacing, bevel } => {
            text_mesh(text, font_family, *font_weight, *size as f32, *depth as f32, *align, *letter_spacing, *bevel)
        }
        Shape3d::Image { .. } => plane(1.0, 1.0),
        Shape3d::Model { .. } | Shape3d::Group {} | Shape3d::Particles(_) => return None,
        // Everything else is modelled in kimchi-core as polygons.
        other => from_tris(&kimchi_core::mesh::shape_mesh(other)?.triangulate()),
    };
    let mesh = Arc::new(mesh);
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 256 {
        map.clear();
    }
    map.insert(key, mesh.clone());
    Some(mesh)
}

/// A mesh from kimchi-core's triangles.
pub(crate) fn from_tris(t: &kimchi_core::mesh::TriMesh) -> Mesh {
    Mesh { pos: t.positions.clone(), normal: t.normals.clone(), uv: t.uvs.clone(), index: t.indices.clone() }
}

/// A box with rounded edges (`bevel` is the radius, clamped to half the smallest side).
fn rounded_box(h: V3, bevel: f32) -> Mesh {
    let r = bevel.clamp(0.0, h.0.min(h.1).min(h.2));
    let mut m = Mesh::default();
    if r < 1e-4 {
        // Six flat faces: (normal, u axis, v axis).
        let faces = [
            (V3(1.0, 0.0, 0.0), V3(0.0, 0.0, -1.0), V3(0.0, -1.0, 0.0)),
            (V3(-1.0, 0.0, 0.0), V3(0.0, 0.0, 1.0), V3(0.0, -1.0, 0.0)),
            (V3(0.0, 1.0, 0.0), V3(1.0, 0.0, 0.0), V3(0.0, 0.0, 1.0)),
            (V3(0.0, -1.0, 0.0), V3(1.0, 0.0, 0.0), V3(0.0, 0.0, -1.0)),
            (V3(0.0, 0.0, 1.0), V3(1.0, 0.0, 0.0), V3(0.0, -1.0, 0.0)),
            (V3(0.0, 0.0, -1.0), V3(-1.0, 0.0, 0.0), V3(0.0, -1.0, 0.0)),
        ];
        for (n, u, v) in faces {
            m.grid(1, 1, |a, b| {
                let p = n + u * (a * 2.0 - 1.0) + v * (b * 2.0 - 1.0);
                (V3(p.0 * h.0, p.1 * h.1, p.2 * h.2), n)
            });
        }
        return m;
    }
    // Sample each axis densely across the rounded band, then push points onto the rounded box.
    const B: usize = 5;
    let axis = |half: f32| -> Vec<f32> {
        let inner = half - r;
        let mut v = vec![];
        for k in 0..=B {
            v.push(-inner - r * (std::f32::consts::FRAC_PI_2 * k as f32 / B as f32).cos());
        }
        for k in 0..=B {
            v.push(inner + r * (std::f32::consts::FRAC_PI_2 * k as f32 / B as f32).sin());
        }
        v
    };
    let (ax, ay, az) = (axis(h.0), axis(h.1), axis(h.2));
    let project = |p: V3| -> (V3, V3) {
        let inner = V3(p.0.clamp(-(h.0 - r), h.0 - r), p.1.clamp(-(h.1 - r), h.1 - r), p.2.clamp(-(h.2 - r), h.2 - r));
        let n = (p - inner).norm();
        (inner + n * r, n)
    };
    // (fixed axis, sign, u values, v values, build point)
    let lerp = |vals: &[f32], t: f32| vals[((t * (vals.len() - 1) as f32).round() as usize).min(vals.len() - 1)];
    let n = 2 * B + 1;
    type Face<'a> = (&'a [f32], &'a [f32], Box<dyn Fn(f32, f32) -> V3>);
    let faces: [Face; 6] = [
        (&az, &ay, Box::new(move |u, v| V3(h.0, -v, -u))),
        (&az, &ay, Box::new(move |u, v| V3(-h.0, -v, u))),
        (&ax, &az, Box::new(move |u, v| V3(u, h.1, v))),
        (&ax, &az, Box::new(move |u, v| V3(u, -h.1, -v))),
        (&ax, &ay, Box::new(move |u, v| V3(u, -v, h.2))),
        (&ax, &ay, Box::new(move |u, v| V3(-u, -v, -h.2))),
    ];
    for (us, vs, f) in faces.iter() {
        m.grid(n, n, |a, b| project(f(lerp(us, a), lerp(vs, b))));
    }
    m
}

fn sphere(r: f32) -> Mesh {
    let mut m = Mesh::default();
    m.grid(32, 64, |u, v| {
        let (theta, phi) = (u * std::f32::consts::TAU, v * std::f32::consts::PI);
        let n = V3(phi.sin() * theta.sin(), phi.cos(), phi.sin() * theta.cos());
        (n * r, n)
    });
    m
}

/// A cylinder (or a cone when the top radius is 0) standing on y, centred.
fn cylinder(bottom: f32, top: f32, height: f32) -> Mesh {
    let mut m = Mesh::default();
    let hh = height / 2.0;
    let slope = (bottom - top) / height.max(1e-6);
    m.grid(1, 64, |u, v| {
        let a = u * std::f32::consts::TAU;
        let r = bottom + (top - bottom) * (1.0 - v);
        let (s, c) = a.sin_cos();
        (V3(s * r, hh - v * height, c * r), V3(s, slope, c))
    });
    for (y, r, up) in [(hh, top, 1.0f32), (-hh, bottom, -1.0)] {
        if r <= 1e-5 {
            continue;
        }
        let n = V3(0.0, up, 0.0);
        let center = m.push(V3(0.0, y, 0.0), n, [0.5, 0.5]);
        let first = m.pos.len() as u32;
        for k in 0..=64 {
            let a = k as f32 / 64.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            m.push(V3(s * r, y, c * r), n, [0.5 + s * 0.5, 0.5 + c * 0.5]);
        }
        for k in 0..64 {
            if up > 0.0 {
                m.tri(center, first + k, first + k + 1);
            } else {
                m.tri(center, first + k + 1, first + k);
            }
        }
    }
    m
}

fn torus(r: f32, tube: f32) -> Mesh {
    let mut m = Mesh::default();
    m.grid(24, 72, |u, v| {
        let (a, b) = (u * std::f32::consts::TAU, v * std::f32::consts::TAU);
        let center = V3(a.sin() * r, 0.0, a.cos() * r);
        let n = V3(a.sin() * b.cos(), b.sin(), a.cos() * b.cos());
        (center + n * tube, n)
    });
    m
}

/// A card facing +z, centred.
fn plane(w: f32, h: f32) -> Mesh {
    let mut m = Mesh::default();
    m.grid(1, 1, |u, v| (V3((u - 0.5) * w, (0.5 - v) * h, 0.0), V3(0.0, 0.0, 1.0)));
    m
}

/// Letters extruded `depth` along z, `size` units high (em), centred on the origin.
#[allow(clippy::too_many_arguments)]
fn text_mesh(text: &str, family: &str, weight: f64, size: f32, depth: f32, align: kimchi_core::TextAlign, spacing: f64, bevel: f64) -> Mesh {
    use lyon_tessellation::path::Path as LPath;
    use lyon_tessellation::path::iterator::PathIterator;
    use lyon_tessellation::{BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers};
    const EM: f32 = 100.0;
    let style = TextStyle {
        content: text.to_string(),
        font_family: family.to_string(),
        font_size: EM as f64,
        font_weight: weight.clamp(100.0, 900.0) as u16,
        italic: false,
        color: "#ffffff".into(),
        background: None,
        align,
        line_height: 1.1,
        letter_spacing: spacing * EM as f64 / size.max(1e-3) as f64,
        shadow: false,
    };
    let layout = crate::text::layout_cached(&style, 1.0);
    let k = size / EM;
    // Glyph outlines (y down, px) → one lyon path (y up, world units).
    let mut b = LPath::builder();
    let pt = |x: f32, y: f32| lyon_tessellation::math::point(x * k, -y * k);
    let mut open = false;
    // The same outlines as SVG path data (y down, world units), for rounded edges.
    let mut d = String::new();
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for g in &layout.glyphs {
        let crate::text::Ink::Outline { path, .. } = &g.ink else { continue };
        if bevel > 0.0 {
            use std::fmt::Write;
            for seg in path.segments() {
                let mut at = |p: tiny_skia::Point| {
                    lo = [lo[0].min(p.x), lo[1].min(p.y)];
                    hi = [hi[0].max(p.x), hi[1].max(p.y)];
                    format!("{} {}", p.x * k, p.y * k)
                };
                let _ = match seg {
                    tiny_skia::PathSegment::MoveTo(p) => write!(d, "M{} ", at(p)),
                    tiny_skia::PathSegment::LineTo(p) => write!(d, "L{} ", at(p)),
                    tiny_skia::PathSegment::QuadTo(c, p) => write!(d, "Q{} {} ", at(c), at(p)),
                    tiny_skia::PathSegment::CubicTo(c1, c2, p) => write!(d, "C{} {} {} ", at(c1), at(c2), at(p)),
                    tiny_skia::PathSegment::Close => write!(d, "Z "),
                };
            }
        }
        for seg in path.segments() {
            match seg {
                tiny_skia::PathSegment::MoveTo(p) => {
                    if open {
                        b.end(true);
                    }
                    b.begin(pt(p.x, p.y));
                    open = true;
                }
                tiny_skia::PathSegment::LineTo(p) => {
                    b.line_to(pt(p.x, p.y));
                }
                tiny_skia::PathSegment::QuadTo(c, p) => {
                    b.quadratic_bezier_to(pt(c.x, c.y), pt(p.x, p.y));
                }
                tiny_skia::PathSegment::CubicTo(c1, c2, p) => {
                    b.cubic_bezier_to(pt(c1.x, c1.y), pt(c2.x, c2.y), pt(p.x, p.y));
                }
                tiny_skia::PathSegment::Close => {
                    if open {
                        b.end(true);
                        open = false;
                    }
                }
            }
        }
    }
    if open {
        b.end(true);
    }
    let path = b.build();
    // Rounded edges: kimchi-core's extruder (it fits the outline centred to its larger side, so
    // move it back where the letters are).
    if bevel > 0.0 && depth > 0.0 && lo[0] <= hi[0] {
        let extent = ((hi[0] - lo[0]).max(hi[1] - lo[1]) * k) as f64;
        let bevel = bevel.min(depth as f64 / 2.0);
        if let Some(poly) = kimchi_core::mesh::shape_mesh(&Shape3d::Extrude { d, size: extent, depth: depth as f64, bevel }) {
            let mut m = from_tris(&poly.triangulate());
            let centre = [(lo[0] + hi[0]) / 2.0 * k, -(lo[1] + hi[1]) / 2.0 * k];
            for p in &mut m.pos {
                p[0] += centre[0];
                p[1] += centre[1];
            }
            let (w, h) = ((layout.width as f32 * k).max(1e-3), (layout.height as f32 * k).max(1e-3));
            m.uv = m.pos.iter().map(|p| [p[0] / w + 0.5, 0.5 - p[1] / h]).collect();
            return m;
        }
    }
    let tolerance = (size * 0.002).max(1e-4);
    let mut m = Mesh::default();
    let hd = depth.max(0.0) / 2.0;

    // Front and back caps.
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let mut tess = FillTessellator::new();
    let ok = tess
        .tessellate_path(
            &path,
            &FillOptions::tolerance(tolerance).with_fill_rule(lyon_tessellation::FillRule::NonZero),
            &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| v.position().to_array()),
        )
        .is_ok();
    if ok {
        let (w, h) = ((layout.width as f32 * k).max(1e-3), (layout.height as f32 * k).max(1e-3));
        for (z, nz) in [(hd, 1.0f32), (-hd, -1.0)] {
            let base = m.pos.len() as u32;
            for v in &buffers.vertices {
                m.push(V3(v[0], v[1], z), V3(0.0, 0.0, nz), [v[0] / w + 0.5, 0.5 - v[1] / h]);
            }
            for t in buffers.indices.as_chunks::<3>().0.iter() {
                // Counter-clockwise seen from the side the cap faces (lyon's winding is y down).
                let [a, b, c] = t.map(|i| buffers.vertices[i as usize]);
                let ccw = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0;
                if ccw == (nz > 0.0) {
                    m.tri(base + t[0], base + t[1], base + t[2]);
                } else {
                    m.tri(base + t[0], base + t[2], base + t[1]);
                }
            }
        }
    }
    if hd <= 0.0 {
        return m;
    }
    // Side walls from the flattened outlines; normals smooth across gentle bends.
    let mut contour: Vec<[f32; 2]> = vec![];
    let mut contours = vec![];
    for e in path.iter().flattened(tolerance) {
        match e {
            lyon_tessellation::path::PathEvent::Begin { at } => {
                contour = vec![at.to_array()];
            }
            lyon_tessellation::path::PathEvent::Line { to, .. } => contour.push(to.to_array()),
            lyon_tessellation::path::PathEvent::End { .. } => {
                contours.push(std::mem::take(&mut contour));
            }
            _ => {}
        }
    }
    let contours: Vec<Vec<[f32; 2]>> = contours
        .into_iter()
        .map(|mut pts| {
            pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
            if pts.len() > 1 && (pts[0][0] - pts[pts.len() - 1][0]).abs() < 1e-6 && (pts[0][1] - pts[pts.len() - 1][1]).abs() < 1e-6 {
                pts.pop();
            }
            pts
        })
        .filter(|pts| pts.len() >= 3)
        .collect();
    for pts in &contours {
        let n = pts.len();
        // Walls face away from the letter: fonts wind outlines either way round, so look at
        // which side of the outline is filled (non-zero winding, like the caps).
        let out = {
            let (a, b) = (pts[0], pts[1]);
            let right = V3(b[1] - a[1], -(b[0] - a[0]), 0.0).norm();
            let probe = [(a[0] + b[0]) / 2.0 + right.0 * size * 1e-3, (a[1] + b[1]) / 2.0 + right.1 * size * 1e-3];
            if winding(&contours, probe) != 0 { -1.0 } else { 1.0 }
        };
        let edge_n: Vec<V3> = (0..n)
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                V3(b[1] - a[1], -(b[0] - a[0]), 0.0).norm() * out
            })
            .collect();
        for i in 0..n {
            let j = (i + 1) % n;
            let ne = edge_n[i];
            let (prev, next) = (edge_n[(i + n - 1) % n], edge_n[j]);
            let na = if ne.dot(prev) > 0.82 { (ne + prev).norm() } else { ne };
            let nb = if ne.dot(next) > 0.82 { (ne + next).norm() } else { ne };
            let (a, b) = (pts[i], pts[j]);
            let v0 = m.push(V3(a[0], a[1], hd), na, [0.0, 0.0]);
            let v1 = m.push(V3(b[0], b[1], hd), nb, [1.0, 0.0]);
            let v2 = m.push(V3(a[0], a[1], -hd), na, [0.0, 1.0]);
            let v3 = m.push(V3(b[0], b[1], -hd), nb, [1.0, 1.0]);
            if out > 0.0 {
                m.tri(v0, v2, v1);
                m.tri(v1, v2, v3);
            } else {
                m.tri(v0, v1, v2);
                m.tri(v1, v3, v2);
            }
        }
    }
    m
}

/// How many times the outlines wind around `p` (counter-clockwise positive).
fn winding(contours: &[Vec<[f32; 2]>], p: [f32; 2]) -> i32 {
    let mut w = 0;
    for c in contours {
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            let side = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= p[1] {
                if b[1] > p[1] && side > 0.0 {
                    w += 1;
                }
            } else if b[1] <= p[1] && side < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

/// One drawable piece of a model: its mesh (already placed in the model) and its own material.
pub(crate) struct Part {
    pub mesh: Arc<Mesh>,
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    /// sRGB RGBA, straight.
    pub texture: Option<Arc<super::Texture>>,
}

/// A model file (glTF/GLB, OBJ or STL, by extension) as parts, centred and scaled so its
/// largest side is 2 units.
pub(crate) fn model(path: &Path) -> Result<Arc<Vec<Part>>, String> {
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Vec<Part>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(m) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return Ok(m.clone());
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let parts = match ext.as_str() {
        "obj" => super::models::obj(path)?,
        "stl" => super::models::stl(path)?,
        _ => gltf_parts(path)?,
    };
    let parts = Arc::new(fit(parts));
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 32 {
        map.clear();
    }
    map.insert(path.to_path_buf(), parts.clone());
    Ok(parts)
}

/// A glTF/GLB file as parts, as the file places them.
fn gltf_parts(path: &Path) -> Result<Vec<Part>, String> {
    let (doc, buffers, images) = gltf::import(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut parts = vec![];
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or("the model has no scene")?;
    let mut textures: HashMap<usize, Arc<super::Texture>> = HashMap::new();
    fn walk<'a>(node: gltf::Node<'a>, parent: super::math::M4, out: &mut Vec<(gltf::Node<'a>, super::math::M4)>) {
        let local = super::math::M4(node.transform().matrix());
        let world = parent * local;
        out.push((node.clone(), world));
        for c in node.children() {
            walk(c, world, out);
        }
    }
    let mut nodes = vec![];
    for n in scene.nodes() {
        walk(n, super::math::M4::I, &mut nodes);
    }
    for (node, world) in nodes {
        let Some(mesh) = node.mesh() else { continue };
        let nm = world.normal_matrix();
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = reader.read_positions() else { continue };
            let pos: Vec<[f32; 3]> = positions.map(|p| world.point3(V3(p[0], p[1], p[2])).arr()).collect();
            let normal: Vec<[f32; 3]> = match reader.read_normals() {
                Some(ns) => ns.map(|n| nm.dir(V3(n[0], n[1], n[2])).norm().arr()).collect(),
                None => vec![[0.0, 1.0, 0.0]; pos.len()],
            };
            let uv: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                Some(t) => t.into_f32().collect(),
                None => vec![[0.0, 0.0]; pos.len()],
            };
            let index: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            let had_normals = reader.read_normals().is_some();
            let mut m = Mesh { pos, normal, uv, index };
            if !had_normals {
                m = m.faceted();
            }
            let mat = prim.material();
            let pbr = mat.pbr_metallic_roughness();
            let texture = pbr.base_color_texture().and_then(|info| {
                let i = info.texture().source().index();
                if let Some(t) = textures.get(&i) {
                    return Some(t.clone());
                }
                let img = images.get(i)?;
                let rgba = to_rgba(img)?;
                let t = Arc::new(super::Texture { width: img.width, height: img.height, rgba });
                textures.insert(i, t.clone());
                Some(t)
            });
            let e = mat.emissive_factor();
            parts.push(Part {
                mesh: Arc::new(m),
                color: pbr.base_color_factor(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                emissive: e,
                texture,
            });
        }
    }
    if parts.is_empty() {
        return Err(format!("{} has no triangles to draw", path.display()));
    }
    Ok(parts)
}

/// Parts centred together and scaled so the largest side is 2 units.
pub(crate) fn fit(mut parts: Vec<Part>) -> Vec<Part> {
    let (mut lo, mut hi) = (V3(f32::MAX, f32::MAX, f32::MAX), V3(f32::MIN, f32::MIN, f32::MIN));
    for p in &parts {
        let (a, b) = p.mesh.bounds();
        lo = lo.min(a);
        hi = hi.max(b);
    }
    let center = (lo + hi) * 0.5;
    let extent = (hi - lo).0.max((hi - lo).1).max((hi - lo).2).max(1e-6);
    let k = 2.0 / extent;
    for p in &mut parts {
        let mut m = (*p.mesh).clone();
        for v in &mut m.pos {
            *v = ((V3(v[0], v[1], v[2]) - center) * k).arr();
        }
        p.mesh = Arc::new(m);
    }
    parts
}

fn to_rgba(img: &gltf::image::Data) -> Option<Vec<u8>> {
    use gltf::image::Format;
    let px = (img.width * img.height) as usize;
    Some(match img.format {
        Format::R8G8B8A8 => img.pixels.clone(),
        Format::R8G8B8 => img.pixels.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        Format::R8G8 => img.pixels.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        Format::R8 => img.pixels.iter().flat_map(|&c| [c, c, c, 255]).collect(),
        _ => vec![255; px * 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::motion::Vec3;

    fn closed_enough(m: &Mesh) {
        assert!(!m.index.is_empty());
        assert_eq!(m.index.len() % 3, 0);
        assert!(m.index.iter().all(|&i| (i as usize) < m.pos.len()));
        assert!(m.normal.iter().all(|n| ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 1e-3));
    }

    #[test]
    fn primitives_are_well_formed() {
        for s in [
            Shape3d::Box { size: Vec3([2.0, 1.0, 1.0]), bevel: 0.0 },
            Shape3d::Box { size: Vec3([2.0, 1.0, 1.0]), bevel: 0.1 },
            Shape3d::Sphere { radius: 0.5, segments: 0.0 },
            Shape3d::Cylinder { radius: 0.5, height: 1.0, segments: 0.0 },
            Shape3d::Cone { radius: 0.5, height: 1.0, segments: 0.0 },
            Shape3d::Torus { radius: 0.5, tube: 0.2 },
            Shape3d::Plane { width: 1.0, height: 1.0 },
        ] {
            let m = shape(&s).unwrap();
            closed_enough(&m);
        }
        let (lo, hi) = rounded_box(V3(1.0, 0.5, 0.5), 0.1).bounds();
        assert!((hi.0 - 1.0).abs() < 1e-4 && (lo.1 + 0.5).abs() < 1e-4, "{lo:?} {hi:?}");
    }

    #[test]
    fn text_is_extruded_and_centred() {
        let m = text_mesh("Hi", "Manrope", 800.0, 1.0, 0.3, kimchi_core::TextAlign::Center, 0.0, 0.0);
        closed_enough(&m);
        let (lo, hi) = m.bounds();
        assert!((hi.2 - 0.15).abs() < 1e-4 && (lo.2 + 0.15).abs() < 1e-4);
        assert!((lo.0 + hi.0).abs() < 0.2, "roughly centred: {lo:?} {hi:?}");
        assert!(hi.1 - lo.1 > 0.5 && hi.1 - lo.1 < 1.2, "about one unit tall: {lo:?} {hi:?}");
    }

    #[test]
    fn text_bevels_round_its_edges_in_place() {
        let flat = text_mesh("Bo", "Manrope", 700.0, 1.0, 0.3, kimchi_core::TextAlign::Center, 0.0, 0.0);
        let round = text_mesh("Bo", "Manrope", 700.0, 1.0, 0.3, kimchi_core::TextAlign::Center, 0.0, 0.05);
        closed_enough(&round);
        let (a, b) = (flat.bounds(), round.bounds());
        for (p, q) in [(a.0, b.0), (a.1, b.1)] {
            assert!((p - q).len() < 0.02, "same place and size: {a:?} vs {b:?}");
        }
        let slanted = |m: &Mesh| m.normal.iter().filter(|n| n[2].abs() > 0.2 && n[2].abs() < 0.9).count();
        assert_eq!(slanted(&flat), 0, "square edges without a bevel");
        assert!(slanted(&round) > 50, "rounded edges with one: {}", slanted(&round));
    }

    #[test]
    fn text_faces_wind_outwards() {
        // Booleans and solidify read the winding: every triangle turns counter-clockwise seen
        // from outside, the way its normals point, letters with holes too.
        for text in ["KIM", "O", "Bo8"] {
            let m = text_mesh(text, "Manrope", 700.0, 1.0, 0.3, kimchi_core::TextAlign::Center, 0.0, 0.0);
            let mut volume = 0.0f64;
            for t in m.index.as_chunks::<3>().0.iter() {
                let v = |a: [f32; 3]| V3(a[0], a[1], a[2]);
                let p = t.map(|i| v(m.pos[i as usize]));
                let n = (p[1] - p[0]).cross(p[2] - p[0]);
                if n.len() < 1e-9 {
                    continue;
                }
                let stored = v(m.normal[t[0] as usize]) + v(m.normal[t[1] as usize]) + v(m.normal[t[2] as usize]);
                assert!(n.dot(stored) > 0.0, "{text}: a triangle winds against its normals at {:?}", p[0]);
                volume += p[0].dot(p[1].cross(p[2])) as f64 / 6.0;
            }
            assert!(volume > 0.0, "{text}: inside out ({volume})");
        }
    }
}
