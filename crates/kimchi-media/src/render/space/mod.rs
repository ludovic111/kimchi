//! 3D scenes ([`Scene3d`]): evaluated at an instant into a list of meshes with materials, a
//! camera, lights and a world ([`Frame3d`]), then drawn on the GPU when there is one (wgpu: Metal
//! on Apple Silicon and Intel Macs, Vulkan or DirectX 12 elsewhere) or by the CPU rasteriser
//! otherwise: the standard engine, like Blender's Eevee. Both shade the same way (see [`shade`]
//! and `gpu.wgsl`): metallic/roughness with clearcoat and glass, directional, point, spot and
//! area lights with soft shadow maps (the main sun and up to three spot or area lights), the
//! environment's light and reflections (or a sky/ground ambient without one), procedural
//! patterns with bumps, fog, and a highlight roll-off. Camera effects (bloom, depth of field,
//! ambient occlusion) and motion blur come after ([`post`], [`Space::render_frame`]).
//!
//! `KIMCHI_GPU=0` forces the CPU renderer; `KIMCHI_GPU=any` accepts a software GPU adapter
//! (llvmpipe), to run the GPU renderer on machines without one.

mod bvh;
pub(crate) mod cpu;
mod denoise;
pub(crate) mod env;
pub(crate) mod gpu;
pub(crate) mod math;
pub(crate) mod mesh;
pub(crate) mod models;
pub(crate) mod particles;
pub(crate) mod pattern;
pub(crate) mod post;
pub(crate) mod shapes;
#[cfg(test)]
mod tests;
pub mod trace;
pub mod viewport;

pub use shapes::editable_poly;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use kimchi_core::anim::value_at;
use kimchi_core::motion::{EvalOptions, Object3d, Scene3d, Shape3d, walk_objects};
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
        self.bilinear(u, v, |p| [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3] as f32 / 255.0])
    }

    /// Bilinear sample, repeating, the stored values as they are (0–1): height maps.
    pub(crate) fn sample_raw(&self, u: f32, v: f32) -> [f32; 4] {
        self.bilinear(u, v, |p| p.map(|c| c as f32 / 255.0))
    }

    fn bilinear(&self, u: f32, v: f32, read: impl Fn(&[u8; 4]) -> [f32; 4]) -> [f32; 4] {
        let (w, h) = (self.width as f32, self.height as f32);
        if self.width == 0 || self.height == 0 || !u.is_finite() || !v.is_finite() {
            return [1.0; 4];
        }
        let (x, y) = (u.rem_euclid(1.0) * w - 0.5, v.rem_euclid(1.0) * h - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| -> [f32; 4] {
            let xi = (xi as i64).rem_euclid(self.width as i64) as usize;
            let yi = (yi as i64).rem_euclid(self.height as i64) as usize;
            let i = (yi * self.width as usize + xi) * 4;
            match self.rgba.get(i..i + 4) {
                Some(p) => read(&[p[0], p[1], p[2], p[3]]),
                None => [1.0; 4],
            }
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        std::array::from_fn(|k| (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy)
    }

    /// Whether any texel is see-through.
    pub(crate) fn has_alpha(&self) -> bool {
        self.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255)
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
    /// Heights for bumps (a pattern's, read raw 0–1 with [`Texture::sample_raw`]), and how strong.
    pub bump: Option<(Arc<Texture>, f32)>,
    /// Glass: 0 opaque … 1 clear, with its index of refraction.
    pub transmission: f32,
    pub ior: f32,
    /// A clear varnish layer, 0–1.
    pub clearcoat: f32,
}

impl Mat {
    /// Drawn after the opaque things, far to near, blended over them.
    pub(crate) fn transparent(&self) -> bool {
        self.base[3] < 0.999 || self.transmission > 0.0 || self.texture.as_ref().is_some_and(|t| t.has_alpha())
    }
}

pub(crate) struct Item {
    pub mesh: Arc<Mesh>,
    pub model: M4,
    pub normal: M4,
    pub mat: Mat,
    /// Distance from the camera (transparent items are drawn far to near).
    pub depth: f32,
    /// The object it belongs to (particles share their emitter's).
    pub id: Arc<str>,
    /// Whether it shadows other things.
    pub cast_shadow: bool,
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
    /// Point and spot: distance where the light has faded out (0 = never).
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

    /// An area light's sides (unit vectors across its width and height).
    pub(crate) fn area_axes(&self) -> (V3, V3) {
        let up = if self.dir.1.abs() > 0.99 { V3(0.0, 0.0, 1.0) } else { V3(0.0, 1.0, 0.0) };
        let x = self.dir.cross(up).norm();
        (x, x.cross(self.dir).norm())
    }
}

/// A shadow map: the scene's depth seen from one light.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShadowRes {
    /// Which light (index into `Frame3d::lights`).
    pub light: usize,
    /// World → the light's clip space.
    pub viewproj: M4,
    /// A sun's box (true) or a spot/area light's frustum.
    pub ortho: bool,
    /// Width of the soft edge, world units at the receiver.
    pub softness: f32,
    /// Frustum depth range (perspective maps compare distances).
    pub near: f32,
    pub far: f32,
    /// Orthographic: world size of the whole map; perspective: tan of half its field of view.
    pub extent: f32,
}

impl ShadowRes {
    /// Distance from the light of a perspective map's depth value.
    pub(crate) fn distance(&self, z: f32) -> f32 {
        let r = self.far / (self.near - self.far);
        r * self.near / (z + r)
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
    /// Towards the sun (unit): the main directional light, for skies.
    pub sun: V3,
    /// The sun's linear colour × intensity.
    pub sun_color: [f32; 3],
    /// Blurred reflections and irradiance, made from the above (see [`env`]).
    pub maps: Option<Arc<env::EnvMaps>>,
}

/// The camera a frame is filmed with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CameraRes {
    #[allow(dead_code)] // for the path tracer
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

impl CameraRes {
    /// Distance along the view direction of a depth-buffer value (0 near … 1 far).
    pub(crate) fn distance(&self, z: f32) -> f32 {
        if self.ortho {
            return self.near + z * (self.far - self.near);
        }
        let r = self.far / (self.near - self.far);
        r * self.near / (z + r)
    }
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
    /// Light-space transform of the main directional light, and which light it is (the first of
    /// `shadows` when it is a sun).
    pub shadow: Option<(M4, usize)>,
    /// Every shadow map: the main sun's first, then spot and area lights (four at most).
    pub shadows: Vec<ShadowRes>,
    /// Fog: distance where it starts, where it is complete, and its colour (linear).
    pub fog: Option<(f32, f32, [f32; 3])>,
    pub items: Vec<Item>,
    pub camera: CameraRes,
    pub env: Option<Env>,
    /// Stops, applied before tone mapping.
    pub exposure: f32,
    pub filmic: bool,
    pub quality: Quality,
    /// Glow around bright things.
    pub bloom: post::Bloom,
    /// Screen-space ambient occlusion strength (0 = off).
    pub ao: f32,
}

/// What a renderer is asked for besides the colours.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Want {
    /// Linear light (for [`post`]) instead of finished 8-bit colours.
    pub linear: bool,
    /// The distance of each pixel.
    pub depth: bool,
}

/// What a renderer drew.
pub(crate) enum Pixels {
    Encoded(Pixmap),
    Linear(Vec<[f32; 4]>),
}

pub(crate) struct Drawn {
    pub pixels: Pixels,
    /// Distance along the view direction per pixel (infinite where nothing opaque is); empty
    /// unless asked for.
    pub depth: Vec<f32>,
}

enum Engine {
    Gpu(Box<gpu::Gpu>),
    Cpu,
}

/// Draws 3D scenes; holds the GPU device when there is one.
pub struct Space {
    engine: Engine,
    faceted: HashMap<usize, (Arc<Mesh>, Arc<Mesh>)>,
    /// Pictures made into textures, by the picture (kept alive alongside, so its address isn't reused).
    textures: HashMap<PictureKey, (Arc<Pixmap>, Arc<Texture>)>,
    /// How scenes are evaluated (the frame rate of the clip being drawn).
    eval: EvalOptions,
}

/// A picture's address and size.
type PictureKey = (usize, u32, u32);

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
        Space { engine, faceted: HashMap::new(), textures: HashMap::new(), eval: EvalOptions::default() }
    }

    /// A CPU-only renderer.
    pub fn cpu() -> Space {
        Space { engine: Engine::Cpu, faceted: HashMap::new(), textures: HashMap::new(), eval: EvalOptions::default() }
    }

    pub fn describe(&self) -> String {
        match &self.engine {
            Engine::Gpu(g) => format!("gpu ({})", g.name()),
            Engine::Cpu => "cpu".into(),
        }
    }

    /// `scene` at scene time `t`, `width`×`height`, premultiplied.
    pub(crate) fn render(&mut self, scene: &Scene3d, t: f64, width: u32, height: u32, pics: &mut dyn Pictures, quality: Quality) -> MediaResult<Pixmap> {
        let frame = self.frame(scene, t, width, height, pics, quality);
        if quality == Quality::Final && scene.render.path_traced() {
            return Ok(trace::render(&frame, &trace::Settings::of(&scene.render)));
        }
        Ok(self.draw(&frame, false).0)
    }

    /// A frame of a clip: `scene` at scene time `t`, where one output frame lasts `frame` scene
    /// seconds. Final quality with motion blur blends moments across the shutter
    /// (`render.motionBlur` × one frame, centred on `t`, `render.motionBlurSamples` of them);
    /// otherwise the same as [`Space::render`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_frame(&mut self, scene: &Scene3d, t: f64, frame: f64, width: u32, height: u32, pics: &mut dyn Pictures, quality: Quality) -> MediaResult<Pixmap> {
        self.set_frame(frame);
        let shutter = scene.render.motion_blur.clamp(0.0, 4.0) * frame.abs();
        if quality != Quality::Final || shutter <= 1e-9 || !shutter.is_finite() {
            return self.render(scene, t, width, height, pics, quality);
        }
        let n = scene.render.motion_blur_samples.clamp(2.0, 64.0) as usize;
        let mut acc = vec![0.0f32; width as usize * height as usize * 4];
        for k in 0..n {
            let at = t - shutter / 2.0 + shutter * (k as f64 + 0.5) / n as f64;
            let p = self.render(scene, at, width, height, pics, quality)?;
            for (a, v) in acc.iter_mut().zip(p.data()) {
                *a += *v as f32;
            }
        }
        let out: Vec<u8> = acc.iter().map(|v| (v / n as f32).round().clamp(0.0, 255.0) as u8).collect();
        Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(width.max(1), height.max(1)).ok_or_else(|| crate::MediaError::Unsupported("empty frame".into()))?)
            .ok_or_else(|| crate::MediaError::Unsupported("bad frame".into()))
    }

    /// Expressions see the frame rate one output frame of `frame` scene seconds makes.
    pub(crate) fn set_frame(&mut self, frame: f64) {
        if frame.is_finite() && frame.abs() > 1e-6 {
            self.eval.fps = (1.0 / frame.abs()).clamp(1.0, 1000.0);
        }
    }

    /// Draws a frame on the GPU (falling back to the CPU for good if it fails), camera effects
    /// included; with `depth`, also the distance of each pixel.
    pub(crate) fn draw(&mut self, f: &Frame3d, depth: bool) -> (Pixmap, Vec<f32>) {
        let linear = post::needed(f);
        let want = Want { linear, depth: depth || linear };
        let mut drawn = None;
        if let Engine::Gpu(g) = &mut self.engine {
            match g.render(f, want) {
                Ok(d) => drawn = Some(d),
                Err(e) => {
                    tracing::warn!("GPU 3D failed, using the CPU from now on: {e}");
                    self.engine = Engine::Cpu;
                }
            }
        }
        let d = drawn.unwrap_or_else(|| cpu::render(f, want));
        match d.pixels {
            Pixels::Encoded(p) => (p, d.depth),
            Pixels::Linear(rgba) => {
                let keep = if depth { d.depth.clone() } else { vec![] };
                let hdr = post::Hdr { width: f.width, height: f.height, rgba, depth: d.depth };
                (post::finish(f, hdr), keep)
            }
        }
    }

    /// The frame `scene` makes at scene time `t`: what the renderers (and the path tracer) draw.
    pub(crate) fn frame(&mut self, scene: &Scene3d, t: f64, width: u32, height: u32, pics: &mut dyn Pictures, quality: Quality) -> Frame3d {
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
        let (width, height) = (width.max(1), height.max(1));
        let aspect = width as f32 / height as f32;

        // The whole scene at `t` at once: keyframes, expressions, constraints.
        let eval = self.eval;
        let solved = scene.evaluate_at(t, &eval);
        // The camera's place and axes (its projection waits for the scene's depth range).
        let cam = solved.camera;
        let eye = V3::from(cam.position.0);
        let target = V3::from(cam.target.0);
        let fwd = (target - eye).norm();
        let up0 = if fwd.1.abs() > 0.999 { V3(0.0, 0.0, -1.0) } else { V3(0.0, 1.0, 0.0) };
        // Roll turns the up vector around the viewing direction.
        let roll = (cam.roll as f32).to_radians();
        let side = fwd.cross(up0).norm();
        let up = (up0 * roll.cos() + side * roll.sin()).norm();
        let view = M4::look_at(eye, target, up);
        let ortho = cam.orthographic();
        let fov = cam.fov.clamp(1.0, 170.0) as f32;
        let ortho_size = (cam.ortho_size as f32).max(1e-3);
        // `look_at` makes its own right/up from `up`: use the same so post effects agree.
        let right = fwd.cross(up).norm();
        let up = right.cross(fwd);
        let focus = if cam.focus_distance > 0.0 { cam.focus_distance as f32 } else { (target - eye).len().max(1e-3) };
        // A 36 mm-wide sensor: the lens' focal length from the field of view, its opening from
        // the f-number (world units are metres).
        let aperture = if cam.f_stop > 0.0 {
            let focal_mm = 18.0 / ((fov.to_radians() / 2.0).tan() * aspect.max(1.0));
            focal_mm / cam.f_stop as f32 / 1000.0 / 2.0
        } else {
            0.0
        };
        let mut camera = CameraRes {
            view,
            proj: M4::I,
            eye,
            forward: fwd,
            right,
            up,
            ortho,
            fov_y: fov.to_radians(),
            ortho_size,
            near: 0.05,
            far: 2000.0,
            focus,
            aperture,
        };

        let mut lights: Vec<LightRes> = if scene.lights.is_empty() {
            vec![
                LightRes::sun(V3(-0.5, -1.0, -0.7).norm(), [1.5, 1.45, 1.4]),
                LightRes::sun(V3(0.8, -0.3, -0.5).norm(), [0.35, 0.38, 0.45]),
            ]
        } else {
            solved
                .lights
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
                    let size = l.size.unwrap_or([0.0, 0.0]).map(|s| s.max(0.0) as f32);
                    let size = match kind {
                        LightKind::Directional => [size[0].to_radians().min(1.0), 0.0],
                        LightKind::Area => size,
                        _ => [size[0], 0.0],
                    };
                    LightRes {
                        kind,
                        point,
                        v: if point { V3::from(l.position.0) } else { V3::from(l.direction.0).norm() },
                        dir: V3::from(l.direction.0).norm(),
                        color: [c[0] * k, c[1] * k, c[2] * k],
                        range: l.range.max(0.0) as f32,
                        cos_outer: half.cos(),
                        cos_inner: inner.cos(),
                        size,
                        shadows: l.cast_shadows,
                    }
                })
                .collect()
        };
        lights.truncate(gpu::MAX_LIGHTS);

        // Objects, where they are, and what they are drawn with.
        let objects: Vec<Object3d> = solved.objects;
        let world = world_matrices(&objects);
        let mut items = vec![];
        {
            let ctx = shapes::Ctx { t, objects: &objects, world: &world };
            let mut c = Collect { space: self, scene, t, ctx: &ctx, camera: &camera, pics, quality, out: &mut items };
            for o in &objects {
                c.object(o, M4::I);
            }
        }

        // Near and far planes around what is drawn, so big and small scenes both keep depth precision.
        let (lo, hi) = bounds(&items);
        if lo.0 <= hi.0 {
            let (mut dmin, mut dmax) = (f32::MAX, f32::MIN);
            for c in corners(lo, hi) {
                let d = (c - eye).dot(fwd);
                dmin = dmin.min(d);
                dmax = dmax.max(d);
            }
            let far = (dmax * 1.05 + 0.01).max(0.1);
            let near = if dmin > 0.0 { (dmin * 0.9).max(far * 1e-5) } else { (far * 1e-4).max(1e-3) };
            camera.near = near.min(far * 0.99);
            camera.far = far;
        }
        camera.proj = if ortho {
            let (hh, hw) = (ortho_size / 2.0, ortho_size / 2.0 * aspect);
            M4::ortho(-hw, hw, -hh, hh, camera.near, camera.far)
        } else {
            M4::perspective(fov, aspect, camera.near, camera.far)
        };
        let viewproj = camera.proj * view;
        for it in items.iter_mut() {
            it.depth = -view.point3(it.model.point3(V3::default())).2;
        }

        // Shadow maps: the main sun (as before), then spot and area lights.
        let mut shadows = vec![];
        if scene.shadows && !items.is_empty() {
            let main = lights.iter().enumerate().filter(|(_, l)| !l.point && l.shadows).max_by(|a, b| lum(a.1.color).total_cmp(&lum(b.1.color)));
            if let Some((i, l)) = main {
                let (lo, hi) = bounds(&items);
                // Large floors would spread the shadow map thin: keep it around what the camera sees.
                let center = (lo + hi) * 0.5;
                let reach = ((hi - lo).len() / 2.0).min((eye - target).len() * 1.6 + 2.0).max(0.5);
                let center = V3(center.0.clamp(target.0 - reach, target.0 + reach), center.1.clamp(target.1 - reach, target.1 + reach), center.2.clamp(target.2 - reach, target.2 + reach));
                let dir = l.v;
                let up = if dir.1.abs() > 0.99 { V3(0.0, 0.0, 1.0) } else { V3(0.0, 1.0, 0.0) };
                let lview = M4::look_at(center - dir * (reach * 4.0), center, up);
                let lproj = M4::ortho(-reach, reach, -reach, reach, 0.01, reach * 8.0);
                // A sun of `size` degrees blurs its shadows over a share of the scene's size.
                let softness = l.size[0].tan() * reach * 0.15;
                shadows.push(ShadowRes { light: i, viewproj: lproj * lview, ortho: true, softness, near: 0.01, far: reach * 8.0, extent: reach * 2.0 });
            }
            let mut spots: Vec<(usize, &LightRes)> = lights.iter().enumerate().filter(|(_, l)| matches!(l.kind, LightKind::Spot | LightKind::Area) && l.shadows).collect();
            spots.sort_by(|a, b| lum(b.1.color).total_cmp(&lum(a.1.color)));
            let (lo, hi) = bounds(&items);
            for (i, l) in spots.into_iter().take(4 - shadows.len()) {
                let half = if l.kind == LightKind::Spot { l.cos_outer.clamp(-1.0, 1.0).acos().min(1.4) + 0.05 } else { 65f32.to_radians() };
                let up = if l.dir.1.abs() > 0.99 { V3(0.0, 0.0, 1.0) } else { V3(0.0, 1.0, 0.0) };
                let lview = M4::look_at(l.v, l.v + l.dir, up);
                let far = corners(lo, hi).iter().map(|c| (*c - l.v).len()).fold(0.1f32, f32::max) * 1.05;
                let near = (far * 1e-3).max(0.02);
                let lproj = M4::perspective((half * 2.0).to_degrees().min(170.0), 1.0, near, far);
                let softness = match l.kind {
                    LightKind::Area => l.size[0].max(l.size[1]) * 0.25,
                    _ => l.size[0] * 0.5,
                };
                shadows.push(ShadowRes { light: i, viewproj: lproj * lview, ortho: false, softness, near, far, extent: half.tan() });
            }
        }
        let shadow = shadows.first().filter(|s| s.ortho).map(|s| (s.viewproj, s.light));

        let fog = match (&background, scene.fog) {
            (Some(bg), true) => {
                let d = (eye - target).len().max(0.5);
                Some((d * 1.6, d * 4.5, [srgb_f(bg[0]), srgb_f(bg[1]), srgb_f(bg[2])]))
            }
            _ => None,
        };

        // The world: its own colours, the sun for skies, and the blurred maps lighting uses.
        let env = scene.environment.as_ref().map(|e| {
            let strength = num("environment.strength", e.strength).max(0.0) as f32;
            let col = |name: &str, base: &str| {
                let c = linear_of(&text(name, Some(base)).unwrap_or_default());
                [c[0] * strength, c[1] * strength, c[2] * strength]
            };
            let kind = match e.kind.as_str() {
                "color" => EnvKind::Color,
                "sky" => EnvKind::Sky,
                "image" => EnvKind::Image,
                _ => EnvKind::Gradient,
            };
            let image = if kind == EnvKind::Image { e.image.as_deref().and_then(|r| self.texture(pics, r, t)) } else { None };
            let sun_light = lights.iter().filter(|l| l.kind == LightKind::Directional).max_by(|a, b| lum(a.color).total_cmp(&lum(b.color)));
            let (sun, sun_color) = match sun_light {
                Some(l) => {
                    let m = l.color[0].max(l.color[1]).max(l.color[2]).max(1e-6);
                    (-l.v, l.color.map(|c| c / m))
                }
                None => (V3(0.3, 0.8, 0.5).norm(), [1.0, 0.95, 0.9]),
            };
            let mut env = Env {
                kind,
                color: col("environment.color", &e.color),
                top: col("environment.top", &e.top),
                horizon: col("environment.horizon", &e.horizon),
                bottom: col("environment.bottom", &e.bottom),
                image,
                strength,
                rotation: (num("environment.rotation", e.rotation) as f32).to_radians(),
                visible: e.visible,
                sun,
                sun_color,
                maps: None,
            };
            env.maps = Some(env.build_maps());
            env
        });

        let render = &scene.render;
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
            shadows,
            fog,
            items,
            camera,
            env,
            exposure: num("render.exposure", render.exposure) as f32,
            filmic: render.tone_mapping == "filmic",
            quality,
            bloom: post::Bloom {
                intensity: num("render.bloom", render.bloom).max(0.0) as f32,
                threshold: num("render.bloomThreshold", render.bloom_threshold).max(0.0) as f32,
                radius: num("render.bloomRadius", render.bloom_radius).max(0.0) as f32,
            },
            ao: num("render.ambientOcclusion", render.ambient_occlusion).max(0.0) as f32,
        }
    }

    fn faceted(&mut self, m: &Arc<Mesh>) -> Arc<Mesh> {
        let key = Arc::as_ptr(m) as usize;
        if self.faceted.len() > 512 {
            self.faceted.clear();
        }
        // The source is kept alongside, so its address can't be reused by another mesh.
        self.faceted.entry(key).or_insert_with(|| (m.clone(), Arc::new(m.faceted()))).1.clone()
    }

    fn texture(&mut self, pics: &mut dyn Pictures, reference: &str, t: f64) -> Option<Arc<Texture>> {
        let p = pics.picture(reference, t)?;
        let key = (Arc::as_ptr(&p) as usize, p.width(), p.height());
        if self.textures.len() > 32 {
            self.textures.clear();
        }
        Some(self.textures.entry(key).or_insert_with(|| (p.clone(), Arc::new(Texture::of(&p)))).1.clone())
    }
}

/// Gathers the items of a frame.
struct Collect<'a, 'p> {
    space: &'a mut Space,
    scene: &'a Scene3d,
    t: f64,
    ctx: &'a shapes::Ctx<'a>,
    camera: &'a CameraRes,
    pics: &'p mut dyn Pictures,
    quality: Quality,
    out: &'a mut Vec<Item>,
}

impl Collect<'_, '_> {
    fn object(&mut self, o: &Object3d, parent: M4) {
        if !o.visible_at(self.t) {
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
        // A pattern takes the picture's place; its heights dent the surface.
        let size = if self.quality == Quality::Final { 1024 } else { 512 };
        let baked = m.pattern.as_ref().and_then(|p| pattern::bake(p, size));
        let (texture, bump) = match &baked {
            Some(b) => (Some(b.color.clone()), b.height.clone()),
            None => (m.texture.as_deref().and_then(|r| self.space.texture(self.pics, r, self.t)), None),
        };
        let texture_scale = m.texture_scale.map_or([1.0, 1.0], |s| [s[0] as f32, s[1] as f32]);
        let mat = Mat {
            base,
            metallic: m.metallic.clamp(0.0, 1.0) as f32,
            roughness: m.roughness.clamp(0.0, 1.0) as f32,
            emissive,
            unlit: m.unlit,
            texture,
            texture_scale,
            bump,
            transmission: m.transmission.clamp(0.0, 1.0) as f32,
            ior: m.ior.clamp(1.0, 3.0) as f32,
            clearcoat: m.clearcoat.clamp(0.0, 1.0) as f32,
        };
        let id: Arc<str> = Arc::from(o.id.as_str());
        let item = |mesh: Arc<Mesh>, model: M4, mat: Mat| Item { mesh, model, normal: model.normal_matrix(), mat, depth: 0.0, id: id.clone(), cast_shadow: o.cast_shadow };
        match &o.shape {
            Shape3d::Group {} => {}
            Shape3d::Model { src } => match self.pics.path(src).or_else(|| Some(PathBuf::from(src)).filter(|p| p.is_file())).ok_or_else(|| format!("no model file `{src}`")).and_then(|p| mesh::model(&p)) {
                Ok(parts) => {
                    let tint = m.color != "#d9d9d9";
                    let has_mods = o.modifiers.iter().any(|m| m.enabled);
                    for (k, p) in parts.iter().enumerate() {
                        if has_mods && k > 0 {
                            break;
                        }
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
                        // With modifiers, the parts are merged into one mesh (the first part's material).
                        let mesh = if has_mods { shapes::model_with_modifiers(o, &parts, self.ctx) } else { p.mesh.clone() };
                        let mesh = if m.flat { self.space.faceted(&mesh) } else { mesh };
                        self.out.push(item(mesh, model, pm));
                    }
                }
                Err(e) => tracing::warn!("3D model: {e}"),
            },
            Shape3d::Image { asset, width } => {
                if let Some(tex) = self.space.texture(self.pics, asset, self.t) {
                    let aspect = tex.height as f32 / tex.width.max(1) as f32;
                    let w = *width as f32;
                    let model = model * M4::scale(V3(w, w * aspect, 1.0));
                    let mesh = mesh::shape(&Shape3d::Plane { width: 1.0, height: 1.0 }).expect("plane");
                    let mut im = mat.clone();
                    im.texture = Some(tex);
                    im.bump = None;
                    im.base = [1.0, 1.0, 1.0, base[3]];
                    im.unlit = true;
                    self.out.push(item(mesh, model, im));
                }
            }
            Shape3d::Particles(sys) => {
                let origin = particles::Origin::new(self.scene, &o.id, self.space.eval);
                let image = match (sys.shape.as_deref(), &sys.asset) {
                    (Some("image"), Some(a)) => self.space.texture(self.pics, a, self.t),
                    _ => None,
                };
                let tint = m.color != "#d9d9d9";
                particles::items(sys, self.t, model.origin(), &|time| origin.at(time), self.camera, &mat, tint, image, &id, o.cast_shadow, self.out);
            }
            _ => {
                if let Some(b) = shapes::build(o, self.ctx) {
                    let mesh = if m.flat { self.space.faceted(&b.mesh) } else { b.mesh };
                    self.out.push(item(mesh, model, mat));
                }
            }
        }
        for c in &o.children {
            self.object(c, model);
        }
    }
}

/// Every object's world matrix (children through their parents), by id.
pub(crate) fn world_matrices(objects: &[Object3d]) -> HashMap<String, M4> {
    fn place(objects: &[Object3d], parent: M4, out: &mut HashMap<String, M4>) {
        for o in objects {
            let m = parent * M4::trs(V3::from(o.position.0), V3::from(o.rotation.0), V3::from(o.scale.0));
            out.insert(o.id.clone(), m);
            place(&o.children, m, out);
        }
    }
    let mut out = HashMap::new();
    place(objects, M4::I, &mut out);
    out
}

/// World bounds of everything drawn (lo > hi when there is nothing).
fn bounds(items: &[Item]) -> (V3, V3) {
    let (mut lo, mut hi) = (V3(f32::MAX, f32::MAX, f32::MAX), V3(f32::MIN, f32::MIN, f32::MIN));
    for it in items {
        if it.mesh.pos.is_empty() {
            continue;
        }
        let (a, b) = it.mesh.bounds();
        for c in corners(a, b) {
            let w = it.model.point3(c);
            if w.finite() {
                lo = lo.min(w);
                hi = hi.max(w);
            }
        }
    }
    (lo, hi)
}

pub(crate) fn corners(a: V3, b: V3) -> [V3; 8] {
    [V3(a.0, a.1, a.2), V3(b.0, b.1, b.2), V3(a.0, b.1, a.2), V3(b.0, a.1, b.2), V3(a.0, a.1, b.2), V3(b.0, b.1, a.2), V3(a.0, b.1, b.2), V3(b.0, a.1, a.2)]
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

/// A point on a surface being shaded.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Surface {
    pub p: V3,
    /// Unit normal (either side; shading turns it to the viewer).
    pub n: V3,
    /// Texture coordinates (before the material's `texture_scale`).
    pub uv: [f32; 2],
    /// How the position changes with the scaled texture coordinates (for bumps).
    pub dpdu: V3,
    pub dpdv: V3,
}

fn smooth01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Blinn-Phong shininess for a roughness, and its normalisation.
fn lobe(rough: f32) -> (f32, f32) {
    let shininess = (2.0 / (rough * rough * rough * rough) - 2.0).clamp(1.0, 4096.0);
    (shininess, (shininess + 8.0) / 8.0 * (1.0 - 0.6 * rough))
}

/// The normal dented by a height map (`amount` = the pattern's bump), from the surface
/// gradient of the height.
fn bumped(h: &Texture, amount: f32, uv: [f32; 2], n: V3, dpdu: V3, dpdv: V3) -> V3 {
    let (du, dv) = (1.0 / h.width.max(1) as f32, 1.0 / h.height.max(1) as f32);
    let hu = (h.sample_raw(uv[0] + du, uv[1])[0] - h.sample_raw(uv[0] - du, uv[1])[0]) / (2.0 * du);
    let hv = (h.sample_raw(uv[0], uv[1] + dv)[0] - h.sample_raw(uv[0], uv[1] - dv)[0]) / (2.0 * dv);
    let bn = dpdv.cross(n);
    let tn = n.cross(dpdu);
    let det = dpdu.dot(bn);
    if det.abs() < 1e-12 || !det.is_finite() {
        return n;
    }
    let g = (bn * hu + tn * hv) * (1.0 / det);
    let out = (n - g * (amount * 0.02)).norm();
    if out.finite() { out } else { n }
}

/// What the world shows in direction `d` for a surface of roughness `rough`: the environment's
/// blurred reflection, or the sky/ground ambient without one.
fn world_light(f: &Frame3d, d: V3, rough: f32) -> [f32; 3] {
    match f.env.as_ref().and_then(|e| e.maps.as_ref().map(|m| (e, m))) {
        Some((e, m)) => m.reflect(d, e.rotation, rough),
        None => {
            let up = d.1 * 0.5 + 0.5;
            std::array::from_fn(|c| (f.ground[c] + (f.sky[c] - f.ground[c]) * up) * 2.0)
        }
    }
}

/// The shading model (the WGSL shader is the same, line for line). `lit(i)` is how much of
/// light `i` reaches the point past shadows (0–1). Returns linear RGB (straight) and alpha,
/// before tone mapping and sRGB encoding.
pub(crate) fn shade(f: &Frame3d, mat: &Mat, s: &Surface, lit: &dyn Fn(usize) -> f32) -> [f32; 4] {
    let uv = [s.uv[0] * mat.texture_scale[0], s.uv[1] * mat.texture_scale[1]];
    let mut base = mat.base;
    if let Some(t) = &mat.texture {
        let c = t.sample(uv[0], uv[1]);
        base = [base[0] * c[0], base[1] * c[1], base[2] * c[2], base[3] * c[3]];
    }
    if mat.unlit {
        return [base[0] + mat.emissive[0], base[1] + mat.emissive[1], base[2] + mat.emissive[2], base[3]];
    }
    let p = s.p;
    let v = if f.camera.ortho { -f.camera.forward } else { (f.eye - p).norm() };
    let mut n = if s.n.dot(v) < 0.0 { -s.n } else { s.n };
    if let Some((h, amount)) = &mat.bump {
        n = bumped(h, *amount, uv, n, s.dpdu, s.dpdv);
    }
    let metallic = mat.metallic;
    let rough = mat.roughness.clamp(0.04, 1.0);
    let trans = mat.transmission;
    // Glass reflects by its index of refraction; everything else like 4% plastic.
    let f0g = ((mat.ior - 1.0) / (mat.ior + 1.0)).powi(2);
    let f0d = 0.04 + (f0g - 0.04) * trans;
    let f0 = [f0d + (base[0] - f0d) * metallic, f0d + (base[1] - f0d) * metallic, f0d + (base[2] - f0d) * metallic];
    let kd = (1.0 - metallic) * (1.0 - trans);
    let diffuse = [base[0] * kd, base[1] * kd, base[2] * kd];
    let cc = mat.clearcoat;
    let (cc_shin, cc_norm) = lobe(0.06);
    let mut out = [0.0f32; 3];
    let r = n * (2.0 * n.dot(v)) - v;
    for (i, l) in f.lights.iter().enumerate() {
        // Direction to the light, how much reaches (fall-off, cone, emitter side), its distance,
        // and the direction highlights come from (an area light's nearest point to the reflection).
        let (dir, mut k, dist, sdir) = match l.kind {
            LightKind::Directional => (-l.v, 1.0, f32::MAX, -l.v),
            LightKind::Area => {
                let (ax, ay) = l.area_axes();
                let (hw, hh) = (l.size[0] / 2.0, l.size[1] / 2.0);
                let local = p - l.v;
                let q = l.v + ax * local.dot(ax).clamp(-hw, hw) + ay * local.dot(ay).clamp(-hh, hh);
                let d = q - p;
                let dist = d.len().max(1e-4);
                let dir = d * (1.0 / dist);
                // Only the front shines, more straight on.
                let k = (-dir).dot(l.dir).max(0.0);
                let denom = r.dot(l.dir);
                let sdir = if denom < -1e-4 {
                    let tr = (l.v - p).dot(l.dir) / denom;
                    let hit = p + r * tr.max(0.0) - l.v;
                    ((l.v + ax * hit.dot(ax).clamp(-hw, hw) + ay * hit.dot(ay).clamp(-hh, hh)) - p).norm()
                } else {
                    dir
                };
                (dir, k, dist, sdir)
            }
            _ => {
                let d = l.v - p;
                let dist = d.len();
                let fall = if l.range > 0.0 { (1.0 - dist / l.range).clamp(0.0, 1.0).powi(2) } else { 1.0 };
                let dir = d.norm();
                (dir, fall, dist, dir)
            }
        };
        if l.kind == LightKind::Spot {
            let c = (-dir).dot(l.dir);
            k *= smooth01((c - l.cos_outer) / (l.cos_inner - l.cos_outer).max(1e-4));
        }
        let ndl = n.dot(dir).max(0.0);
        if ndl <= 0.0 || k <= 0.0 {
            continue;
        }
        // Big lights make broader highlights.
        let spread = match l.kind {
            LightKind::Directional => l.size[0] * 0.5,
            LightKind::Area => 0.5 * l.size[0].max(l.size[1]) / dist,
            _ => 0.5 * l.size[0] / dist.max(1e-4),
        };
        let rl = (rough + spread).min(1.0);
        let (shininess, norm) = lobe(rl);
        let h = (sdir + v).norm();
        let ndh = n.dot(h).max(0.0);
        let vdh = v.dot(h).max(0.0);
        let fr = (1.0 - vdh).powi(5);
        let spec = norm * ndh.powf(shininess);
        let sh = lit(i);
        let coat = cc * (0.04 + 0.96 * fr);
        let coat_spec = cc_norm * ndh.powf(cc_shin);
        for c in 0..3 {
            // Schlick with roughness: rough surfaces don't turn into mirrors at grazing angles.
            let fc = f0[c] + ((1.0 - rough).max(f0[c]) - f0[c]) * fr;
            let lobe = diffuse[c] * (1.0 - fc) + fc * spec;
            out[c] += (lobe * (1.0 - coat) + coat * coat_spec) * l.color[c] * ndl * k * sh;
        }
    }
    let ndv = n.dot(v).max(0.0);
    let gloss = (1.0 - rough) * (1.0 - rough);
    let maps = f.env.as_ref().and_then(|e| e.maps.as_ref().map(|m| (e, m)));
    match maps {
        // The world lights matte surfaces from all around and is reflected by shiny ones
        // (pre-blurred by roughness; Karis' fit of the split-sum for the Fresnel part).
        Some((e, m)) => {
            let irr = m.irradiance(n, e.rotation);
            let refl = m.reflect(r, e.rotation, rough);
            let c0 = [-1.0, -0.0275, -0.572, 0.022];
            let c1 = [1.0, 0.0425, 1.04, -0.04];
            let rr: [f32; 4] = std::array::from_fn(|i| rough * c0[i] + c1[i]);
            let a004 = (rr[0] * rr[0]).min((-9.28 * ndv).exp2()) * rr[0] + rr[1];
            let (ea, eb) = (-1.04 * a004 + rr[2], 1.04 * a004 + rr[3]);
            for c in 0..3 {
                out[c] += diffuse[c] * irr[c] + refl[c] * (f0[c] * ea + eb) + mat.emissive[c];
            }
        }
        // Sky above, ground below: soft fill light, and something for metals to reflect.
        None => {
            let up = n.1 * 0.5 + 0.5;
            let rup = r.1 * 0.5 + 0.5;
            for c in 0..3 {
                let amb = f.ground[c] + (f.sky[c] - f.ground[c]) * up;
                let env = (f.ground[c] + (f.sky[c] - f.ground[c]) * rup) * 2.0;
                let fr = f0[c] + ((1.0 - rough).max(f0[c]) - f0[c]) * (1.0 - ndv).powi(5) * gloss;
                out[c] += diffuse[c] * amb + env * fr * gloss + mat.emissive[c];
            }
        }
    }
    // The varnish reflects the world too, over what is below it.
    if cc > 0.0 {
        let fc = cc * (0.04 + 0.96 * (1.0 - ndv).powi(5));
        let w = world_light(f, r, 0.06);
        for c in 0..3 {
            out[c] = out[c] * (1.0 - fc) + w[c] * fc;
        }
    }
    // Glass lets what is behind through (blending, tinted on average) and bends the world.
    let mut alpha = base[3];
    if trans > 0.0 {
        let fv = f0g + (1.0 - f0g) * (1.0 - ndv).powi(5);
        let through = trans * (1.0 - fv);
        let eta = 1.0 / mat.ior.max(1.0);
        let cosi = ndv;
        let k2 = 1.0 - eta * eta * (1.0 - cosi * cosi);
        let refr = if k2 > 0.0 { ((-v) * eta + n * (eta * cosi - k2.sqrt())).norm() } else { r };
        let bent = world_light(f, refr, rough);
        let avg = (base[0] + base[1] + base[2]) / 3.0;
        for c in 0..3 {
            out[c] += through * base[c] * bent[c] * 0.3;
        }
        alpha = (base[3] * (1.0 - through * avg)).max(0.02);
    }
    // Distant things fade into the background.
    if let Some((near, far, bg)) = f.fog {
        let k = (((f.eye - p).len() - near) / (far - near).max(1e-3)).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        for c in 0..3 {
            out[c] += (bg[c] - out[c]) * k;
        }
    }
    // What is drawn is multiplied by alpha later: keep the surface's own opacity on reflections.
    let keep = base[3] / alpha.max(1e-6);
    [out[0] * keep, out[1] * keep, out[2] * keep, alpha]
}

/// The world's colour behind everything in direction `d` (linear).
pub(crate) fn backdrop(f: &Frame3d, d: V3) -> Option<[f32; 3]> {
    let e = f.env.as_ref()?;
    (e.visible && f.background.is_none()).then(|| e.radiance(d))
}

/// A shaded colour (linear, straight) as the renderers write it: tone mapped and sRGB-encoded,
/// or linear for [`post`] (then unlit colours are pre-distorted so they come out exactly as
/// given), premultiplied by `a`.
pub(crate) fn finish(f: &Frame3d, c: [f32; 4], unlit: bool, linear: bool) -> [f32; 4] {
    let a = c[3].clamp(0.0, 1.0);
    let k = post::gain(f);
    let one = |v: f32| -> f32 {
        if linear {
            if unlit { post::untone(v, f.filmic) / k } else { v }
        } else if unlit {
            linear_to_srgb(v)
        } else {
            linear_to_srgb(post::tone(v * k, f.filmic))
        }
    };
    [one(c[0]) * a, one(c[1]) * a, one(c[2]) * a, a]
}

/// The background colour (straight sRGB) premultiplied, as the renderers clear to it.
pub(crate) fn clear_color(f: &Frame3d, linear: bool) -> [f32; 4] {
    match f.background {
        Some(c) if linear => {
            let k = post::gain(f);
            let a = c[3];
            [post::untone(srgb_f(c[0]), f.filmic) / k * a, post::untone(srgb_f(c[1]), f.filmic) / k * a, post::untone(srgb_f(c[2]), f.filmic) / k * a, a]
        }
        Some(c) => [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]],
        None => [0.0; 4],
    }
}

/// The view ray's direction through pixel (`x`, `y`) (centres at .5).
pub(crate) fn view_dir(f: &Frame3d, x: f32, y: f32) -> V3 {
    let c = &f.camera;
    if c.ortho {
        return c.forward;
    }
    let nx = x / f.width as f32 * 2.0 - 1.0;
    let ny = 1.0 - y / f.height as f32 * 2.0;
    let t = (c.fov_y / 2.0).tan();
    let aspect = f.width as f32 / f.height.max(1) as f32;
    (c.forward + c.right * (nx * t * aspect) + c.up * (ny * t)).norm()
}

/// Ids in a 3D scene that draw something (for listings).
pub fn object_ids(scene: &Scene3d) -> Vec<String> {
    let mut out = vec![];
    walk_objects(&scene.objects, &mut |o| out.push(o.id.clone()));
    out
}
