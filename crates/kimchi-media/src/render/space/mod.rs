//! 3D scenes ([`Scene3d`]): evaluated at an instant into a list of meshes with materials, a
//! camera and lights ([`Frame3d`]), then drawn on the GPU when there is one (wgpu: Metal on
//! Apple Silicon and Intel Macs, Vulkan or DirectX 12 elsewhere) or by the CPU rasteriser
//! otherwise. Both shade the same way (see [`shade`] and `gpu.wgsl`): a physically-inspired
//! model with metallic/roughness, directional and point lights, a soft shadow from the main
//! directional light, a sky/ground ambient that also gives metals something to reflect, and a
//! highlight roll-off.
//!
//! `KIMCHI_GPU=0` forces the CPU renderer; `KIMCHI_GPU=any` accepts a software GPU adapter
//! (llvmpipe), to run the GPU renderer on machines without one.

pub(crate) mod cpu;
pub(crate) mod gpu;
pub(crate) mod math;
pub(crate) mod mesh;
pub mod viewport;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use kimchi_core::anim::value_at;
use kimchi_core::motion::{Object3d, Scene3d, Shape3d, walk_objects};
use tiny_skia::Pixmap;

use self::math::{M4, V3};
use self::mesh::Mesh;
use crate::MediaResult;

/// Pictures for textures and image objects, and files for models.
pub(crate) trait Pictures {
    fn picture(&mut self, asset: &str, time: f64) -> Option<Arc<Pixmap>>;
    fn path(&self, reference: &str) -> Option<PathBuf>;
}

/// Straight sRGB RGBA.
#[derive(Debug)]
pub(crate) struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    fn of(p: &Pixmap) -> Texture {
        let rgba = p.pixels().iter().flat_map(|c| {
            let c = c.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        });
        Texture { width: p.width(), height: p.height(), rgba: rgba.collect() }
    }

    /// Bilinear sample, repeating, as linear RGB + alpha.
    pub(crate) fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let (w, h) = (self.width as f32, self.height as f32);
        let (x, y) = (u.rem_euclid(1.0) * w - 0.5, v.rem_euclid(1.0) * h - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| -> [f32; 4] {
            let xi = (xi as i64).rem_euclid(self.width as i64) as usize;
            let yi = (yi as i64).rem_euclid(self.height as i64) as usize;
            let i = (yi * self.width as usize + xi) * 4;
            let p = &self.rgba[i..i + 4];
            [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3] as f32 / 255.0]
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        std::array::from_fn(|k| (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Mat {
    /// Linear RGB and alpha.
    pub base: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub unlit: bool,
    /// A picture or a pattern baked to a picture, multiplied with `base`.
    pub texture: Option<Arc<Texture>>,
    /// Repeats of the texture across the surface.
    pub texture_scale: [f32; 2],
    /// Heights for bumps (a pattern's), and how strong.
    pub bump: Option<(Arc<Texture>, f32)>,
    /// Glass: 0 opaque … 1 clear, with its index of refraction.
    pub transmission: f32,
    pub ior: f32,
    /// A clear varnish layer, 0–1.
    pub clearcoat: f32,
}

pub(crate) struct Item {
    pub mesh: Arc<Mesh>,
    pub model: M4,
    pub normal: M4,
    pub mat: Mat,
    /// Distance from the camera (transparent items are drawn far to near).
    pub depth: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LightKind {
    Directional,
    Point,
    Spot,
    Area,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LightRes {
    pub kind: LightKind,
    /// At a position (point, spot, area) rather than a direction.
    pub point: bool,
    /// Direction the light travels (directional) or its position (point, spot, area).
    pub v: V3,
    /// Where spot and area lights face (unit).
    pub dir: V3,
    /// Linear colour × intensity.
    pub color: [f32; 3],
    pub range: f32,
    /// Spot: cosines of the cone's edge and of where the soft edge begins.
    pub cos_outer: f32,
    pub cos_inner: f32,
    /// Area: width and height; point/spot: bulb radius in `[0]`; sun: softness angle (radians) in `[0]`.
    pub size: [f32; 2],
    pub shadows: bool,
}

impl LightRes {
    fn sun(v: V3, color: [f32; 3]) -> LightRes {
        LightRes { kind: LightKind::Directional, point: false, v, dir: v, color, range: 0.0, cos_outer: -1.0, cos_inner: -1.0, size: [0.0, 0.0], shadows: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EnvKind {
    Color,
    Gradient,
    Sky,
    Image,
}

/// The world around the scene (linear colours, strength applied).
#[derive(Debug, Clone)]
pub(crate) struct Env {
    pub kind: EnvKind,
    pub color: [f32; 3],
    pub top: [f32; 3],
    pub horizon: [f32; 3],
    pub bottom: [f32; 3],
    /// An equirectangular panorama.
    pub image: Option<Arc<Texture>>,
    pub strength: f32,
    /// Radians around the vertical axis.
    pub rotation: f32,
    /// Drawn behind the objects.
    pub visible: bool,
}

/// The camera a frame is filmed with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CameraRes {
    pub view: M4,
    pub proj: M4,
    pub eye: V3,
    pub forward: V3,
    pub right: V3,
    pub up: V3,
    pub ortho: bool,
    /// Vertical field of view (radians), or the height orthographic frames cover.
    pub fov_y: f32,
    pub ortho_size: f32,
    pub near: f32,
    pub far: f32,
    /// Depth of field: distance in focus and lens radius (world units; 0 = everything sharp).
    pub focus: f32,
    pub aperture: f32,
}

/// How good frames must be: quick while editing, the scene's own render settings for exports
/// and renders on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    #[default]
    Preview,
    Final,
}

pub(crate) struct Frame3d {
    pub width: u32,
    pub height: u32,
    /// sRGB, straight; none = transparent.
    pub background: Option<[f32; 4]>,
    pub viewproj: M4,
    pub eye: V3,
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    pub lights: Vec<LightRes>,
    /// Light-space transform of the main directional light, and which light it is.
    pub shadow: Option<(M4, usize)>,
    /// Fog: distance where it starts, where it is complete, and its colour (linear).
    pub fog: Option<(f32, f32, [f32; 3])>,
    pub items: Vec<Item>,
    pub camera: CameraRes,
    pub env: Option<Env>,
    /// Stops, applied before tone mapping.
    pub exposure: f32,
    pub filmic: bool,
    pub quality: Quality,
}

enum Engine {
    Gpu(Box<gpu::Gpu>),
    Cpu,
}

/// Draws 3D scenes; holds the GPU device when there is one.
pub struct Space {
    engine: Engine,
    faceted: HashMap<usize, Arc<Mesh>>,
    textures: HashMap<(usize, u32, u32), Arc<Texture>>,
}

/// The one 3D renderer of the process (one GPU device, shared by the preview and exports).
pub fn shared() -> &'static std::sync::Mutex<Space> {
    static S: std::sync::OnceLock<std::sync::Mutex<Space>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Space::new()))
}

impl Default for Space {
    fn default() -> Self {
        Self::new()
    }
}

impl Space {
    /// The GPU renderer when an adapter is available (and `KIMCHI_GPU` isn't 0), else the CPU one.
    pub fn new() -> Space {
        let off = std::env::var("KIMCHI_GPU").is_ok_and(|v| matches!(v.as_str(), "0" | "off" | "false" | "cpu"));
        let engine = if off {
            Engine::Cpu
        } else {
            match gpu::Gpu::new() {
                Ok(g) => {
                    tracing::info!("3D on the GPU: {}", g.name());
                    Engine::Gpu(Box::new(g))
                }
                Err(e) => {
                    tracing::info!("3D on the CPU ({e})");
                    Engine::Cpu
                }
            }
        };
        Space { engine, faceted: HashMap::new(), textures: HashMap::new() }
    }

    /// A CPU-only renderer.
    pub fn cpu() -> Space {
        Space { engine: Engine::Cpu, faceted: HashMap::new(), textures: HashMap::new() }
    }

    pub fn describe(&self) -> String {
        match &self.engine {
            Engine::Gpu(g) => format!("gpu ({})", g.name()),
            Engine::Cpu => "cpu".into(),
        }
    }

    /// `scene` at scene time `t`, `width`×`height`, premultiplied.
    pub(crate) fn render(&mut self, scene: &Scene3d, t: f64, width: u32, height: u32, pics: &mut dyn Pictures, quality: Quality) -> MediaResult<Pixmap> {
        let mut frame = self.frame(scene, t, width, height, pics);
        frame.quality = quality;
        if let Engine::Gpu(g) = &mut self.engine {
            match g.render(&frame) {
                Ok(p) => return Ok(p),
                Err(e) => {
                    tracing::warn!("GPU 3D failed, using the CPU from now on: {e}");
                    self.engine = Engine::Cpu;
                }
            }
        }
        Ok(cpu::render(&frame))
    }

    fn frame(&mut self, scene: &Scene3d, t: f64, width: u32, height: u32, pics: &mut dyn Pictures) -> Frame3d {
        let num = |name: &str, base: f64| scene.keyframes.get(name).and_then(|k| value_at(k, t)).and_then(|v| v.as_f64()).unwrap_or(base);
        let text = |name: &str, base: Option<&str>| -> Option<String> {
            scene.keyframes.get(name).and_then(|k| value_at(k, t)).and_then(|v| v.as_str().map(str::to_string)).or(base.map(str::to_string))
        };
        let background = text("background", scene.background.as_deref()).map(|c| {
            let c = crate::render::paint::color(&c);
            [c.red(), c.green(), c.blue(), c.alpha()]
        });
        let ambient = num("ambient", scene.ambient).max(0.0) as f32;
        let ac = linear_of(&text("ambientColor", Some(&scene.ambient_color)).unwrap_or_default());
        let sky = [ac[0] * ambient, ac[1] * ambient, ac[2] * ambient];
        let ground = [sky[0] * 0.3, sky[1] * 0.3, sky[2] * 0.3];

        let cam = scene.camera_at(t);
        let eye = V3::from(cam.position.0);
        let target = V3::from(cam.target.0);
        let fwd = (target - eye).norm();
        let up0 = if fwd.1.abs() > 0.999 { V3(0.0, 0.0, -1.0) } else { V3(0.0, 1.0, 0.0) };
        // Roll turns the up vector around the viewing direction.
        let roll = (cam.roll as f32).to_radians();
        let side = fwd.cross(up0).norm();
        let up = (up0 * roll.cos() + side * roll.sin()).norm();
        let view = M4::look_at(eye, target, up);
        let proj = M4::perspective(cam.fov.clamp(1.0, 170.0) as f32, width as f32 / height.max(1) as f32, 0.05, 2000.0);
        let viewproj = proj * view;
        let camera = CameraRes {
            view,
            proj,
            eye,
            forward: fwd,
            right: side,
            up,
            ortho: false,
            fov_y: (cam.fov.clamp(1.0, 170.0) as f32).to_radians(),
            ortho_size: cam.ortho_size as f32,
            near: 0.05,
            far: 2000.0,
            focus: (target - eye).len(),
            aperture: 0.0,
        };

        let lights: Vec<LightRes> = if scene.lights.is_empty() {
            vec![
                LightRes::sun(V3(-0.5, -1.0, -0.7).norm(), [1.5, 1.45, 1.4]),
                LightRes::sun(V3(0.8, -0.3, -0.5).norm(), [0.35, 0.38, 0.45]),
            ]
        } else {
            scene
                .lights_at(t)
                .into_iter()
                .map(|l| {
                    let c = linear_of(&l.color);
                    let k = l.intensity.max(0.0) as f32;
                    let kind = match l.kind.as_str() {
                        "point" => LightKind::Point,
                        "spot" => LightKind::Spot,
                        "area" => LightKind::Area,
                        _ => LightKind::Directional,
                    };
                    let point = kind != LightKind::Directional;
                    let half = (l.angle.clamp(1.0, 179.0) as f32).to_radians() / 2.0;
                    let inner = half * (1.0 - l.blend.clamp(0.0, 1.0) as f32);
                    let size = l.size.unwrap_or([0.0, 0.0]);
                    LightRes {
                        kind,
                        point,
                        v: if point { V3::from(l.position.0) } else { V3::from(l.direction.0).norm() },
                        dir: V3::from(l.direction.0).norm(),
                        color: [c[0] * k, c[1] * k, c[2] * k],
                        range: l.range.max(0.0) as f32,
                        cos_outer: half.cos(),
                        cos_inner: inner.cos(),
                        size: [size[0] as f32, size[1] as f32],
                        shadows: l.cast_shadows,
                    }
                })
                .collect()
        };

        let mut items = vec![];
        let objects: Vec<Object3d> = scene.objects_at(t);
        for o in &objects {
            self.collect(o, M4::I, t, &view, pics, &mut items);
        }

        // The main directional light casts a shadow over everything drawn.
        let shadow = if scene.shadows && !items.is_empty() {
            lights.iter().enumerate().filter(|(_, l)| !l.point).max_by(|a, b| lum(a.1.color).total_cmp(&lum(b.1.color))).map(|(i, l)| {
                let (mut lo, mut hi) = (V3(f32::MAX, f32::MAX, f32::MAX), V3(f32::MIN, f32::MIN, f32::MIN));
                for it in &items {
                    let (a, b) = it.mesh.bounds();
                    for corner in [V3(a.0, a.1, a.2), V3(b.0, b.1, b.2), V3(a.0, b.1, a.2), V3(b.0, a.1, b.2), V3(a.0, a.1, b.2), V3(b.0, b.1, a.2), V3(a.0, b.1, b.2), V3(b.0, a.1, a.2)] {
                        let w = it.model.point3(corner);
                        lo = lo.min(w);
                        hi = hi.max(w);
                    }
                }
                // Large floors would spread the shadow map thin: keep it around what the camera sees.
                let center = (lo + hi) * 0.5;
                let reach = ((hi - lo).len() / 2.0).min((eye - target).len() * 1.6 + 2.0).max(0.5);
                let center = V3(center.0.clamp(target.0 - reach, target.0 + reach), center.1.clamp(target.1 - reach, target.1 + reach), center.2.clamp(target.2 - reach, target.2 + reach));
                let dir = l.v;
                let up = if dir.1.abs() > 0.99 { V3(0.0, 0.0, 1.0) } else { V3(0.0, 1.0, 0.0) };
                let lview = M4::look_at(center - dir * (reach * 4.0), center, up);
                let lproj = M4::ortho(-reach, reach, -reach, reach, 0.01, reach * 8.0);
                (lproj * lview, i)
            })
        } else {
            None
        };
        let fog = match (&background, scene.fog) {
            (Some(bg), true) => {
                let d = (eye - target).len().max(0.5);
                Some((d * 1.6, d * 4.5, [srgb_f(bg[0]), srgb_f(bg[1]), srgb_f(bg[2])]))
            }
            _ => None,
        };
        Frame3d {
            width,
            height,
            background,
            viewproj,
            eye,
            sky,
            ground,
            lights,
            shadow,
            fog,
            items,
            camera,
            env: None,
            exposure: scene.render.exposure as f32,
            filmic: scene.render.tone_mapping == "filmic",
            quality: Quality::Preview,
        }
    }

    fn collect(&mut self, o: &Object3d, parent: M4, t: f64, view: &M4, pics: &mut dyn Pictures, out: &mut Vec<Item>) {
        if !o.visible_at(t) {
            return;
        }
        let model = parent * M4::trs(V3::from(o.position.0), V3::from(o.rotation.0), V3::from(o.scale.0));
        let m = &o.material;
        let mut base = linear_of(&m.color);
        base[3] *= m.opacity.clamp(0.0, 1.0) as f32;
        let emissive = m.emissive.as_deref().map(linear_of).map_or([0.0; 3], |e| {
            let k = m.emissive_intensity.max(0.0) as f32;
            [e[0] * k, e[1] * k, e[2] * k]
        });
        let texture = m.texture.as_deref().and_then(|r| self.texture(pics, r, t));
        let texture_scale = m.texture_scale.map_or([1.0, 1.0], |s| [s[0] as f32, s[1] as f32]);
        let mat = Mat {
            base,
            metallic: m.metallic.clamp(0.0, 1.0) as f32,
            roughness: m.roughness.clamp(0.0, 1.0) as f32,
            emissive,
            unlit: m.unlit,
            texture,
            texture_scale,
            bump: None,
            transmission: m.transmission.clamp(0.0, 1.0) as f32,
            ior: m.ior.clamp(1.0, 3.0) as f32,
            clearcoat: m.clearcoat.clamp(0.0, 1.0) as f32,
        };
        let depth = |model: &M4| -view.point3(model.point3(V3::default())).2;
        match &o.shape {
            Shape3d::Group {} => {}
            Shape3d::Model { src } => match pics.path(src).ok_or_else(|| format!("no model file `{src}`")).and_then(|p| mesh::model(&p)) {
                Ok(parts) => {
                    let tint = m.color != "#d9d9d9";
                    for p in parts.iter() {
                        let mut base = srgb_factor_to_linear(p.color);
                        if tint {
                            for (b, t) in base.iter_mut().zip(mat.base) {
                                *b *= t;
                            }
                        }
                        base[3] *= m.opacity.clamp(0.0, 1.0) as f32;
                        let pm = Mat {
                            base,
                            metallic: p.metallic,
                            roughness: p.roughness,
                            emissive: [p.emissive[0] + emissive[0], p.emissive[1] + emissive[1], p.emissive[2] + emissive[2]],
                            texture: p.texture.clone().or_else(|| mat.texture.clone()),
                            ..mat.clone()
                        };
                        let mesh = if m.flat { self.faceted(&p.mesh) } else { p.mesh.clone() };
                        out.push(Item { mesh, model, normal: model.normal_matrix(), mat: pm, depth: depth(&model) });
                    }
                }
                Err(e) => tracing::warn!("3D model: {e}"),
            },
            Shape3d::Image { asset, width } => {
                if let Some(tex) = self.texture(pics, asset, t) {
                    let aspect = tex.height as f32 / tex.width.max(1) as f32;
                    let w = *width as f32;
                    let model = model * M4::scale(V3(w, w * aspect, 1.0));
                    let mesh = mesh::shape(&Shape3d::Plane { width: 1.0, height: 1.0 }).expect("plane");
                    let mut im = mat.clone();
                    im.texture = Some(tex);
                    im.base = [1.0, 1.0, 1.0, base[3]];
                    im.unlit = true;
                    out.push(Item { mesh, model, normal: model.normal_matrix(), mat: im, depth: depth(&model) });
                }
            }
            shape => {
                if let Some(mesh) = mesh::shape(shape) {
                    let mesh = if m.flat { self.faceted(&mesh) } else { mesh };
                    out.push(Item { mesh, model, normal: model.normal_matrix(), mat, depth: depth(&model) });
                }
            }
        }
        for c in &o.children {
            self.collect(c, model, t, view, pics, out);
        }
    }

    fn faceted(&mut self, m: &Arc<Mesh>) -> Arc<Mesh> {
        let key = Arc::as_ptr(m) as usize;
        self.faceted.entry(key).or_insert_with(|| Arc::new(m.faceted())).clone()
    }

    fn texture(&mut self, pics: &mut dyn Pictures, reference: &str, t: f64) -> Option<Arc<Texture>> {
        let p = pics.picture(reference, t)?;
        let key = (Arc::as_ptr(&p) as usize, p.width(), p.height());
        if self.textures.len() > 32 {
            self.textures.clear();
        }
        Some(self.textures.entry(key).or_insert_with(|| Arc::new(Texture::of(&p))).clone())
    }
}

fn lum(c: [f32; 3]) -> f32 {
    c[0] * 0.2126 + c[1] * 0.7152 + c[2] * 0.0722
}

/// `#rrggbb[aa]` → linear RGB and alpha.
pub(crate) fn linear_of(c: &str) -> [f32; 4] {
    let c = crate::render::paint::color(c);
    [srgb_f(c.red()), srgb_f(c.green()), srgb_f(c.blue()), c.alpha()]
}

fn srgb_factor_to_linear(c: [f32; 4]) -> [f32; 4] {
    // glTF factors are already linear.
    c
}

pub(crate) fn srgb_f(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub(crate) fn srgb_to_linear(v: u8) -> f32 {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| srgb_f(i as f32 / 255.0)))[v as usize]
}

pub(crate) fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Highlights above 0.8 roll off smoothly towards 1 instead of clipping.
pub(crate) fn shoulder(v: f32) -> f32 {
    if v <= 0.8 { v.max(0.0) } else { 0.8 + 0.2 * (1.0 - (-(v - 0.8) / 0.2).exp()) }
}

/// The shading model (the WGSL shader is the same, line for line). `n` is the unit normal,
/// `lit` how much of the main shadowing light reaches the point (0–1). Returns linear RGB and
/// alpha, before the shoulder and sRGB encoding.
pub(crate) fn shade(f: &Frame3d, mat: &Mat, p: V3, n: V3, uv: [f32; 2], lit: f32) -> [f32; 4] {
    let mut base = mat.base;
    if let Some(t) = &mat.texture {
        let s = t.sample(uv[0], uv[1]);
        base = [base[0] * s[0], base[1] * s[1], base[2] * s[2], base[3] * s[3]];
    }
    if mat.unlit {
        return [base[0] + mat.emissive[0], base[1] + mat.emissive[1], base[2] + mat.emissive[2], base[3]];
    }
    let v = (f.eye - p).norm();
    let n = if n.dot(v) < 0.0 { -n } else { n };
    let metallic = mat.metallic;
    let rough = mat.roughness.clamp(0.04, 1.0);
    let f0 = [0.04 + (base[0] - 0.04) * metallic, 0.04 + (base[1] - 0.04) * metallic, 0.04 + (base[2] - 0.04) * metallic];
    let diffuse = [base[0] * (1.0 - metallic), base[1] * (1.0 - metallic), base[2] * (1.0 - metallic)];
    let shininess = (2.0 / (rough * rough * rough * rough) - 2.0).clamp(1.0, 4096.0);
    // Normalised Blinn-Phong; rough surfaces lose some of it to self-shadowing.
    let norm = (shininess + 8.0) / 8.0 * (1.0 - 0.6 * rough);
    let mut out = [0.0f32; 3];
    let main = f.shadow.map(|(_, i)| i);
    for (i, l) in f.lights.iter().enumerate() {
        let (dir, k) = if l.point {
            let d = l.v - p;
            let dist = d.len();
            let fall = if l.range > 0.0 { (1.0 - dist / l.range).clamp(0.0, 1.0).powi(2) } else { 1.0 };
            (d.norm(), fall)
        } else {
            (-l.v, 1.0)
        };
        let ndl = n.dot(dir).max(0.0);
        if ndl <= 0.0 || k <= 0.0 {
            continue;
        }
        let h = (dir + v).norm();
        let ndh = n.dot(h).max(0.0);
        let vdh = v.dot(h).max(0.0);
        let fr = (1.0 - vdh).powi(5);
        let spec = norm * ndh.powf(shininess);
        let s = if Some(i) == main { lit } else { 1.0 };
        for c in 0..3 {
            // Schlick with roughness: rough surfaces don't turn into mirrors at grazing angles.
            let fc = f0[c] + ((1.0 - rough).max(f0[c]) - f0[c]) * fr;
            out[c] += (diffuse[c] * (1.0 - fc) + fc * spec) * l.color[c] * ndl * k * s;
        }
    }
    // Sky above, ground below: soft fill light, and something for metals to reflect.
    let up = n.1 * 0.5 + 0.5;
    let ndv = n.dot(v).max(0.0);
    let r = n * (2.0 * n.dot(v)) - v;
    let rup = r.1 * 0.5 + 0.5;
    let gloss = (1.0 - rough) * (1.0 - rough);
    for c in 0..3 {
        let amb = f.ground[c] + (f.sky[c] - f.ground[c]) * up;
        let env = (f.ground[c] + (f.sky[c] - f.ground[c]) * rup) * 2.0;
        let fr = f0[c] + ((1.0 - rough).max(f0[c]) - f0[c]) * (1.0 - ndv).powi(5) * gloss;
        out[c] += diffuse[c] * amb + env * fr * gloss + mat.emissive[c];
    }
    // Distant things fade into the background.
    if let Some((near, far, bg)) = f.fog {
        let k = (((f.eye - p).len() - near) / (far - near).max(1e-3)).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        for c in 0..3 {
            out[c] += (bg[c] - out[c]) * k;
        }
    }
    [out[0], out[1], out[2], base[3]]
}

/// Ids in a 3D scene that draw something (for listings).
pub fn object_ids(scene: &Scene3d) -> Vec<String> {
    let mut out = vec![];
    walk_objects(&scene.objects, &mut |o| out.push(o.id.clone()));
    out
}
