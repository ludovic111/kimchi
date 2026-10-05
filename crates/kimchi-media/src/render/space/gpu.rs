//! The GPU 3D renderer (wgpu): Metal on macOS (Apple Silicon and Intel), Vulkan or DirectX 12
//! elsewhere. Renders offscreen with 4× multisampling into a half-float target, then reads the
//! picture back (already tone mapped, or linear when camera effects follow), and on request the
//! depth of each pixel (a depth-only pass at the output size).
//!
//! Meshes and textures are uploaded once and kept while the same `Arc` is alive; every frame
//! only writes the camera, lights and per-object uniforms.

use std::collections::HashMap;
use std::sync::Arc;

use rayon::prelude::*;
use tiny_skia::Pixmap;
use wgpu::util::DeviceExt;

use super::env::{EnvMaps, LEVELS};
use super::math::M4;
use super::mesh::Mesh;
use super::{Drawn, EnvKind, Frame3d, LightKind, Pixels, Texture, Want, backdrop, clear_color};

const SHADER: &str = include_str!("gpu.wgsl");
const SAMPLES: u32 = 4;
const SHADOW_SIZE: u32 = 2048;
const SHADOW_LAYERS: u32 = 4;
/// Each face of a point light's cube of shadow maps (six faces per light).
const CUBE_SHADOW_SIZE: u32 = 1024;
const CUBE_LAYERS: u32 = 6 * super::POINT_SHADOWS as u32;
/// Shadow slots in the shader's globals: the 2D maps' layers, then the cube faces'.
const SHADOW_SLOTS: usize = (SHADOW_LAYERS + CUBE_LAYERS) as usize;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Per-object uniform stride (dynamic offsets must be 256-aligned).
const ITEM_STRIDE: u64 = 256;
pub(crate) const MAX_LIGHTS: usize = 8;

pub(crate) struct Gpu {
    name: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    main_layout: wgpu::BindGroupLayout,
    item_layout: wgpu::BindGroupLayout,
    shadow_globals_layout: wgpu::BindGroupLayout,
    shadow_item_layout: wgpu::BindGroupLayout,
    opaque: wgpu::RenderPipeline,
    blended: wgpu::RenderPipeline,
    back: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    env_sampler: wgpu::Sampler,
    shadow_tex: wgpu::Texture,
    /// Point lights' cube faces.
    cube_tex: wgpu::Texture,
    white: (wgpu::Texture, wgpu::TextureView),
    white_linear: (wgpu::Texture, wgpu::TextureView),
    no_env: (wgpu::Texture, wgpu::TextureView),
    meshes: HashMap<usize, (Arc<Mesh>, wgpu::Buffer, wgpu::Buffer, u32)>,
    /// Colour pictures (sRGB) and height maps (raw), by the texture's address.
    textures: HashMap<(usize, bool), (Arc<Texture>, wgpu::Texture, wgpu::TextureView)>,
    env: Option<(Arc<EnvMaps>, wgpu::Texture, wgpu::TextureView)>,
    targets: Option<Targets>,
}

/// Offscreen targets for one output size.
struct Targets {
    size: (u32, u32),
    msaa: wgpu::Texture,
    resolve: wgpu::Texture,
    depth: wgpu::Texture,
    /// Single-sample depth for reading back.
    flat_depth: wgpu::Texture,
    readback: wgpu::Buffer,
    depth_readback: wgpu::Buffer,
    row: u32,
    depth_row: u32,
}

impl Gpu {
    pub(crate) fn new() -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let info = adapter.get_info();
        // A software adapter is slower than our own rasteriser; `KIMCHI_GPU=any` takes it anyway
        // (to exercise this renderer on machines without a GPU).
        let any = std::env::var("KIMCHI_GPU").is_ok_and(|v| v == "any");
        if info.device_type == wgpu::DeviceType::Cpu && !any {
            return Err(format!("only a software adapter ({})", info.name));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("kimchi 3D"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("GPU device: {e}"))?;
        let name = format!("{} via {:?}", info.name, info.backend);
        // Errors become `Err`s below (error scopes); anything else is logged, never a panic.
        device.on_uncaptured_error(Arc::new(|e| tracing::error!("GPU: {e}")));
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let (main_src, rest) = SHADER.split_once("// Shadow pass").expect("shader has a shadow section");
        let shadow_src = rest.split_once('\n').map_or("", |(_, body)| body);
        let structs = SHADER[..SHADER.find("@group(0) @binding(0) var<uniform> g").expect("globals")].to_string();
        let shadow_src = format!("{structs}\n{shadow_src}");
        let main = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("3D"), source: wgpu::ShaderSource::Wgsl(main_src.into()) });
        let shadow_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("3D shadow"), source: wgpu::ShaderSource::Wgsl(shadow_src.into()) });

        let uniform = |binding: u32, dynamic: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: dynamic, min_binding_size: None },
            count: None,
        };
        let texture = |binding: u32, dim: wgpu::TextureViewDimension, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: dim, multisampled: false },
            count: None,
        };
        let filtering = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let main_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                uniform(0, false),
                texture(1, wgpu::TextureViewDimension::D2Array, wgpu::TextureSampleType::Depth),
                texture(2, wgpu::TextureViewDimension::D2, float),
                texture(3, wgpu::TextureViewDimension::D2, float),
                filtering(4),
                texture(5, wgpu::TextureViewDimension::D2Array, wgpu::TextureSampleType::Depth),
            ],
        });
        let item_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("item"),
            entries: &[uniform(0, true), texture(1, wgpu::TextureViewDimension::D2, float), filtering(2), texture(3, wgpu::TextureViewDimension::D2, float)],
        });
        let shadow_globals_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("shadow globals"), entries: &[uniform(0, false)] });
        let shadow_item_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("shadow item"), entries: &[uniform(0, true)] });

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let vertex = [wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vertex_attrs }];
        let pos_only = wgpu::vertex_attr_array![0 => Float32x3];
        let shadow_vertex = [wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &pos_only }];

        let main_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3D"),
            bind_group_layouts: &[Some(&main_layout), Some(&item_layout)],
            immediate_size: 0,
        });
        let back_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("3D world"), bind_group_layouts: &[Some(&main_layout)], immediate_size: 0 });
        let pipeline = |depth_write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("3D"),
                layout: Some(&main_pl),
                vertex: wgpu::VertexState { module: &main, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &vertex },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &main,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = pipeline(true);
        let blended = pipeline(false);
        let back = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3D world"),
            layout: Some(&back_pl),
            vertex: wgpu::VertexState { module: &main, entry_point: Some("vs_back"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &main,
                entry_point: Some("fs_back"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let shadow_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3D shadow"),
            bind_group_layouts: &[Some(&shadow_globals_layout), Some(&shadow_item_layout)],
            immediate_size: 0,
        });
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3D shadow"),
            layout: Some(&shadow_pl),
            vertex: wgpu::VertexState { module: &shadow_mod, entry_point: Some("vs_shadow"), compilation_options: Default::default(), buffers: &shadow_vertex },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Panoramas wrap around but not over the poles.
        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: SHADOW_LAYERS },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let cube_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("point light shadow maps"),
            size: wgpu::Extent3d { width: CUBE_SHADOW_SIZE, height: CUBE_SHADOW_SIZE, depth_or_array_layers: CUBE_LAYERS },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let white_px = Texture { width: 1, height: 1, rgba: vec![255; 4] };
        let white = upload(&device, &queue, &white_px, true);
        let white_linear = upload(&device, &queue, &white_px, false);
        let no_env = upload_env(&device, &queue, None);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("GPU pipelines: {e}"));
        }
        Ok(Gpu {
            name,
            device,
            queue,
            main_layout,
            item_layout,
            shadow_globals_layout,
            shadow_item_layout,
            opaque,
            blended,
            back,
            shadow,
            sampler,
            env_sampler,
            shadow_tex,
            cube_tex,
            white,
            white_linear,
            no_env,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            env: None,
            targets: None,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Why there is no GPU renderer, or which adapter it uses (for diagnostics).
    #[cfg(test)]
    pub fn probe() -> String {
        match Gpu::new() {
            Ok(g) => g.name,
            Err(e) => format!("unavailable: {e}"),
        }
    }

    pub(crate) fn render(&mut self, f: &Frame3d, want: Want) -> Result<Drawn, String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.draw(f, want);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("GPU: {e}"));
        }
        result
    }

    fn texture_view(&mut self, t: &Arc<Texture>, srgb: bool) -> usize {
        let key = (Arc::as_ptr(t) as usize, srgb);
        if !self.textures.contains_key(&key) {
            let (tex, view) = upload(&self.device, &self.queue, t, srgb);
            self.textures.insert(key, (t.clone(), tex, view));
        }
        key.0
    }

    fn draw(&mut self, f: &Frame3d, want: Want) -> Result<Drawn, String> {
        let (w, h) = (f.width.max(1), f.height.max(1));
        if self.targets.as_ref().is_none_or(|t| t.size != (w, h)) {
            self.targets = Some(self.make_targets(w, h));
        }
        // Forget uploads nobody uses any more (pictures: all of them now and then).
        if self.textures.len() > 64 {
            self.textures.clear();
        }
        let live_meshes: std::collections::HashSet<usize> = f.items.iter().map(|i| Arc::as_ptr(&i.mesh) as usize).collect();
        if self.meshes.len() > 512 {
            self.meshes.retain(|k, _| live_meshes.contains(k));
        }
        for it in &f.items {
            let key = Arc::as_ptr(&it.mesh) as usize;
            if !self.meshes.contains_key(&key) {
                let m = &it.mesh;
                let mut vb: Vec<u8> = Vec::with_capacity(m.pos.len() * 32);
                for i in 0..m.pos.len() {
                    let n = m.normal.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                    let uv = m.uv.get(i).copied().unwrap_or([0.0, 0.0]);
                    for v in m.pos[i].iter().chain(&n).chain(&uv) {
                        vb.extend_from_slice(&v.to_le_bytes());
                    }
                }
                let ib: Vec<u8> = m.index.iter().filter(|&&i| (i as usize) < m.pos.len()).flat_map(|i| i.to_le_bytes()).collect();
                if vb.is_empty() || ib.is_empty() {
                    continue;
                }
                let v = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("mesh"), contents: &vb, usage: wgpu::BufferUsages::VERTEX });
                let ix = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("mesh"), contents: &ib, usage: wgpu::BufferUsages::INDEX });
                self.meshes.insert(key, (it.mesh.clone(), v, ix, (ib.len() / 4) as u32));
            }
            if let Some(t) = &it.mat.texture {
                self.texture_view(t, true);
            }
            if let Some((t, _)) = &it.mat.bump {
                self.texture_view(t, false);
            }
        }
        // The world's blurred maps and its panorama.
        let env = f.env.as_ref();
        let env_maps = env.and_then(|e| e.maps.clone());
        match &env_maps {
            Some(m) if !self.env.as_ref().is_some_and(|(have, ..)| Arc::ptr_eq(have, m)) => {
                let (tex, view) = upload_env(&self.device, &self.queue, Some(m));
                self.env = Some((m.clone(), tex, view));
            }
            _ => {}
        }
        let env_image = env.and_then(|e| e.image.clone()).map(|t| self.texture_view(&t, true));

        // Which slot of the shader's shadows each map takes: 2D maps their layer of `shadow_tex`,
        // cube faces 4 + their layer of `cube_tex`.
        let slots = shadow_slots(&f.shadows);
        // Globals.
        let linear = want.linear;
        let mut g: Vec<f32> = vec![];
        g.extend(f.viewproj.flat());
        let n = f.lights.len().min(MAX_LIGHTS);
        let c = &f.camera;
        g.extend([f.eye.0, f.eye.1, f.eye.2, n as f32]);
        g.extend([f.sky[0], f.sky[1], f.sky[2], if c.ortho { 1.0 } else { 0.0 }]);
        g.extend([f.ground[0], f.ground[1], f.ground[2], super::post::gain(f)]);
        g.extend([c.forward.0, c.forward.1, c.forward.2, if linear { 1.0 } else { 0.0 }]);
        g.extend([c.right.0, c.right.1, c.right.2, (c.fov_y / 2.0).tan()]);
        g.extend([c.up.0, c.up.1, c.up.2, c.ortho_size]);
        g.extend([w as f32, h as f32, if f.filmic { 1.0 } else { 0.0 }, 0.0]);
        match f.fog {
            Some((near, far, c)) => {
                g.extend([c[0], c[1], c[2], 1.0]);
                g.extend([near, far, 0.0, 0.0]);
            }
            None => g.extend([0.0; 8]),
        }
        match env {
            Some(e) => {
                let kind = match e.kind {
                    EnvKind::Color => 0.0,
                    EnvKind::Gradient => 1.0,
                    EnvKind::Sky => 2.0,
                    EnvKind::Image if e.image.is_some() => 3.0,
                    // A panorama that couldn't be read shows its colour.
                    EnvKind::Image => 0.0,
                };
                g.extend([kind, e.strength, e.rotation, if backdrop(f, c.forward).is_some() { 1.0 } else { 0.0 }]);
                for col in [e.color, e.top, e.horizon, e.bottom] {
                    g.extend([col[0], col[1], col[2], 0.0]);
                }
                g.extend([e.sun.0, e.sun.1, e.sun.2, 0.0]);
                g.extend([e.sun_color[0], e.sun_color[1], e.sun_color[2], 0.0]);
                let sh = e.maps.as_ref().map_or([[0.0; 3]; 9], |m| m.sh);
                for s in sh {
                    g.extend([s[0], s[1], s[2], 0.0]);
                }
            }
            None => {
                g.extend([-1.0, 0.0, 0.0, 0.0]);
                g.extend([0.0; 4 * 6]);
                g.extend([0.0; 4 * 9]);
            }
        }
        for i in 0..MAX_LIGHTS {
            match f.lights.get(i) {
                Some(l) => {
                    let kind = match l.kind {
                        LightKind::Directional => 0.0,
                        LightKind::Point => 1.0,
                        LightKind::Spot => 2.0,
                        LightKind::Area => 3.0,
                    };
                    let shadow = f.shadows.iter().position(|s| s.light == i).and_then(|k| slots[k]).map_or(-1.0, |s| s as f32);
                    g.extend([l.v.0, l.v.1, l.v.2, kind]);
                    g.extend([l.color[0], l.color[1], l.color[2], l.range]);
                    g.extend([l.dir.0, l.dir.1, l.dir.2, shadow]);
                    g.extend([l.cos_outer, l.cos_inner, l.size[0], l.size[1]]);
                }
                None => g.extend([0.0; 16]),
            }
        }
        let mut by_slot: Vec<Option<&super::ShadowRes>> = vec![None; SHADOW_SLOTS];
        for (k, s) in f.shadows.iter().enumerate() {
            if let Some(slot) = slots[k] {
                by_slot[slot] = Some(s);
            }
        }
        for slot in &by_slot {
            match slot {
                Some(s) => {
                    g.extend(s.viewproj.flat());
                    g.extend([if s.ortho { 1.0 } else { 0.0 }, s.softness, s.near, s.far]);
                    g.extend([s.extent, SHADOW_SIZE as f32, 0.0, 0.0]);
                }
                None => g.extend([0.0; 24]),
            }
        }
        let globals = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("globals"), contents: &bytes(&g), usage: wgpu::BufferUsages::UNIFORM });

        // Per-object uniforms, one 256-byte slot each.
        let mut items: Vec<u8> = vec![0; (f.items.len().max(1) as u64 * ITEM_STRIDE) as usize];
        for (i, it) in f.items.iter().enumerate() {
            let m = &it.mat;
            let mut u: Vec<f32> = vec![];
            u.extend(it.model.flat());
            u.extend(it.normal.flat());
            u.extend(m.base);
            u.extend([m.emissive[0], m.emissive[1], m.emissive[2], if m.unlit { 1.0 } else { 0.0 }]);
            u.extend([m.metallic, m.roughness, m.transmission, m.ior]);
            u.extend([m.clearcoat, m.bump.as_ref().map_or(0.0, |b| b.1), if m.bump.is_some() { 1.0 } else { 0.0 }, 0.0]);
            u.extend([m.texture_scale[0], m.texture_scale[1], 0.0, 0.0]);
            let b = bytes(&u);
            let at = i * ITEM_STRIDE as usize;
            items[at..at + b.len()].copy_from_slice(&b);
        }
        let item_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("items"), contents: &items, usage: wgpu::BufferUsages::UNIFORM });
        let item_binding = wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &item_buf, offset: 0, size: wgpu::BufferSize::new(ITEM_STRIDE) });

        let shadow_view = self.shadow_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let cube_view = self.cube_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let env_view = self.env.as_ref().filter(|_| env_maps.is_some()).map_or(&self.no_env.1, |e| &e.2);
        let image_view = env_image.and_then(|k| self.textures.get(&(k, true))).map_or(&self.white.1, |t| &t.2);
        let main_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &self.main_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(env_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(image_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.env_sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&cube_view) },
            ],
        });
        let shadow_item_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow item"),
            layout: &self.shadow_item_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: item_binding.clone() }],
        });
        // One item bind group per (texture, height map) pair (the uniform slot is a dynamic offset).
        let tex_key = |it: &super::Item| (it.mat.texture.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize), it.mat.bump.as_ref().map_or(0, |b| Arc::as_ptr(&b.0) as usize));
        let mut tex_bgs: HashMap<(usize, usize), wgpu::BindGroup> = HashMap::new();
        for it in &f.items {
            let key = tex_key(it);
            if tex_bgs.contains_key(&key) {
                continue;
            }
            let view = self.textures.get(&(key.0, true)).map_or(&self.white.1, |t| &t.2);
            let bump = self.textures.get(&(key.1, false)).map_or(&self.white_linear.1, |t| &t.2);
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("item"),
                layout: &self.item_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: item_binding.clone() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(bump) },
                ],
            });
            tex_bgs.insert(key, bg);
        }
        let casts = |it: &super::Item| !(it.mat.base[3] < 0.5 || it.mat.unlit || !it.cast_shadow || it.mat.transmission > 0.5);

        let t = self.targets.as_ref().expect("made above");
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("3D") });
        // Depth from each shadowing light, then (when asked) from the camera.
        let mut depth_passes: Vec<(M4, wgpu::TextureView)> = f
            .shadows
            .iter()
            .zip(&slots)
            .filter_map(|(s, slot)| {
                let slot = (*slot)? as u32;
                let (tex, layer) = if slot < SHADOW_LAYERS { (&self.shadow_tex, slot) } else { (&self.cube_tex, slot - SHADOW_LAYERS) };
                let view = tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                Some((s.viewproj, view))
            })
            .collect();
        let camera_depth = want.depth.then(|| t.flat_depth.create_view(&Default::default()));
        if let Some(v) = &camera_depth {
            depth_passes.push((f.viewproj, v.clone()));
        }
        let camera_pass = camera_depth.is_some().then(|| depth_passes.len() - 1);
        for (k, (vp, view)) in depth_passes.iter().enumerate() {
            let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("depth pass"), contents: &bytes(&vp.flat()), usage: wgpu::BufferUsages::UNIFORM });
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("depth pass"),
                layout: &self.shadow_globals_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow);
            pass.set_bind_group(0, &bg, &[]);
            let camera = Some(k) == camera_pass;
            for (i, it) in f.items.iter().enumerate() {
                // The camera's depth is the opaque things'; a light's, the shadow casters'.
                if if camera { it.mat.transparent() } else { !casts(it) } {
                    continue;
                }
                let Some((_, vb, ib, count)) = self.meshes.get(&(Arc::as_ptr(&it.mesh) as usize)) else { continue };
                pass.set_bind_group(1, &shadow_item_bg, &[(i as u64 * ITEM_STRIDE) as u32]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
        }
        {
            let msaa_view = t.msaa.create_view(&Default::default());
            let resolve_view = t.resolve.create_view(&Default::default());
            let depth_view = t.depth.create_view(&Default::default());
            let c = clear_color(f, linear);
            let clear = wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3D"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&resolve_view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &main_bg, &[]);
            if backdrop(f, f.camera.forward).is_some() {
                pass.set_pipeline(&self.back);
                pass.draw(0..3, 0..1);
            }
            let mut order: Vec<usize> = (0..f.items.len()).filter(|&i| !f.items[i].mat.transparent()).collect();
            let opaque_count = order.len();
            let mut clear_items: Vec<usize> = (0..f.items.len()).filter(|&i| f.items[i].mat.transparent()).collect();
            clear_items.sort_by(|a, b| f.items[*b].depth.total_cmp(&f.items[*a].depth));
            order.extend(clear_items);
            for (k, &i) in order.iter().enumerate() {
                let it = &f.items[i];
                let Some((_, vb, ib, count)) = self.meshes.get(&(Arc::as_ptr(&it.mesh) as usize)) else { continue };
                pass.set_pipeline(if k < opaque_count { &self.opaque } else { &self.blended });
                pass.set_bind_group(1, &tex_bgs[&tex_key(it)], &[(i as u64 * ITEM_STRIDE) as u32]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
        }
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.resolve, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &t.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(t.row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        if want.depth {
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture: &t.flat_depth, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::DepthOnly },
                wgpu::TexelCopyBufferInfo {
                    buffer: &t.depth_readback,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(t.depth_row), rows_per_image: Some(h) },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
        self.queue.submit([enc.finish()]);

        let color = read(&self.device, &t.readback)?;
        let (w_, h_) = (w as usize, h as usize);
        let row = t.row as usize;
        // Half floats → f32 from a table, rows on every core.
        let half = half_table();
        let texel = |line: &[u8], x: usize, k: usize| half[u16::from_le_bytes([line[x * 8 + k * 2], line[x * 8 + k * 2 + 1]]) as usize];
        let pixels = if linear {
            let mut out = vec![[0.0f32; 4]; w_ * h_];
            out.par_chunks_mut(w_).zip(color.par_chunks(row)).for_each(|(dst, line)| {
                for (x, o) in dst.iter_mut().enumerate() {
                    *o = std::array::from_fn(|k| texel(line, x, k));
                }
            });
            Pixels::Linear(out)
        } else {
            let mut out = vec![0u8; w_ * h_ * 4];
            out.par_chunks_mut(w_ * 4).zip(color.par_chunks(row)).for_each(|(dst, line)| {
                for (x, o) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let a = texel(line, x, 3).clamp(0.0, 1.0);
                    for (k, v) in o.iter_mut().take(3).enumerate() {
                        *v = crate::render::byte(texel(line, x, k).clamp(0.0, a) * 255.0);
                    }
                    o[3] = crate::render::byte(a * 255.0);
                }
            });
            Pixels::Encoded(Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(w, h).ok_or("size")?).ok_or_else(|| "bad picture".to_string())?)
        };
        let depth = if want.depth {
            let raw = read(&self.device, &t.depth_readback)?;
            let drow = t.depth_row as usize;
            let mut out = vec![0.0f32; w_ * h_];
            out.par_chunks_mut(w_).zip(raw.par_chunks(drow)).for_each(|(dst, line)| {
                for (x, o) in dst.iter_mut().enumerate() {
                    let z = f32::from_le_bytes([line[x * 4], line[x * 4 + 1], line[x * 4 + 2], line[x * 4 + 3]]);
                    *o = if z >= 1.0 { f32::INFINITY } else { f.camera.distance(z) };
                }
            });
            out
        } else {
            vec![]
        };
        Ok(Drawn { pixels, depth })
    }

    fn make_targets(&self, w: u32, h: u32) -> Targets {
        let tex = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let row = (w * 8).div_ceil(256) * 256;
        let depth_row = (w * 4).div_ceil(256) * 256;
        let buffer = |label: &str, size: u32| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        Targets {
            size: (w, h),
            msaa: tex("3D msaa", COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT),
            resolve: tex("3D", COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC),
            depth: tex("3D depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT),
            flat_depth: tex("3D depth readback", DEPTH, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC),
            readback: buffer("3D readback", row * h),
            depth_readback: buffer("3D depth readback", depth_row * h),
            row,
            depth_row,
        }
    }
}

/// The globals slot of each of the frame's shadow maps: 2D maps (the sun's, spot and area
/// lights') in order from 0, cube faces from [`SHADOW_LAYERS`], six per point light; `None` for
/// maps past what the textures hold.
fn shadow_slots(shadows: &[super::ShadowRes]) -> Vec<Option<usize>> {
    let mut flat = 0usize;
    // Lights with cube maps, in the order their faces come.
    let mut cubes: Vec<usize> = vec![];
    shadows
        .iter()
        .map(|s| match s.face {
            None => {
                flat += 1;
                (flat <= SHADOW_LAYERS as usize).then_some(flat - 1)
            }
            Some(face) => {
                let c = cubes.iter().position(|&l| l == s.light).unwrap_or_else(|| {
                    cubes.push(s.light);
                    cubes.len() - 1
                });
                let slot = SHADOW_LAYERS as usize + c * 6 + face as usize;
                (slot < SHADOW_SLOTS).then_some(slot)
            }
        })
        .collect()
}

/// Waits for a buffer the GPU wrote and copies it out.
fn read(device: &wgpu::Device, buf: &wgpu::Buffer) -> Result<Vec<u8>, String> {
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("GPU: {e}"))?;
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("GPU readback: {e}"))?;
    let out = slice.get_mapped_range().to_vec();
    buf.unmap();
    Ok(out)
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, t: &Texture, srgb: bool) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d { width: t.width.max(1), height: t.height.max(1), depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut data = t.rgba.clone();
    data.resize((size.width * size.height * 4) as usize, 255);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size.width), rows_per_image: Some(size.height) },
        size,
    );
    let view = tex.create_view(&Default::default());
    (tex, view)
}

/// The world's reflection levels as one half-float texture with a mip per level (a black
/// 1×1 when there is no world).
fn upload_env(device: &wgpu::Device, queue: &wgpu::Queue, maps: Option<&EnvMaps>) -> (wgpu::Texture, wgpu::TextureView) {
    let (w, h, levels) = match maps {
        Some(m) => (m.levels[0].width as u32, m.levels[0].height as u32, m.levels.len().min(LEVELS) as u32),
        None => (1, 1, 1),
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("world"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for k in 0..levels {
        let (lw, lh, data): (u32, u32, Vec<u8>) = match maps {
            Some(m) => {
                let l = &m.levels[k as usize];
                (l.width as u32, l.height as u32, l.rgba.iter().flatten().flat_map(|v| f32_to_f16(*v).to_le_bytes()).collect())
            }
            None => (1, 1, [0.0f32, 0.0, 0.0, 1.0].iter().flat_map(|v| f32_to_f16(*v).to_le_bytes()).collect()),
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: k, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8 * lw), rows_per_image: Some(lh) },
            wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
        );
    }
    let view = tex.create_view(&Default::default());
    (tex, view)
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Every half float's value, by its bits.
fn half_table() -> &'static [f32] {
    static T: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    T.get_or_init(|| (0..=u16::MAX).map(f16_to_f32).collect())
}

/// IEEE half → single.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;
    let bits = match exp {
        0 if frac == 0 => sign,
        0 => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((f & 0x3ff) << 13)
        }
        0x1f => sign | 0x7f80_0000 | (frac << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (frac << 13),
    };
    f32::from_bits(bits)
}

/// Single → IEEE half (rounded to nearest, clamped to the largest half).
pub(crate) fn f32_to_f16(v: f32) -> u16 {
    if v.is_nan() {
        return 0x7e00;
    }
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let a = v.abs().min(65504.0);
    if a < 6.103_515_6e-5 {
        // Subnormal halves (or zero).
        let m = (a / 5.960_464_5e-8).round() as u16;
        return sign | m;
    }
    let b = a.to_bits();
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let mut mant = (b >> 13) & 0x3ff;
    let rest = b & 0x1fff;
    let mut exp = exp as u32;
    if rest > 0x1000 || (rest == 0x1000 && mant & 1 == 1) {
        mant += 1;
        if mant == 0x400 {
            mant = 0;
            exp += 1;
        }
    }
    sign | ((exp as u16) << 10) | mant as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_trip() {
        for v in [0.0f32, 1.0, -2.5, 0.5, 1.0 / 3.0, 1000.0, 6e-5, 1e-7, 65504.0] {
            let back = f16_to_f32(f32_to_f16(v));
            assert!((back - v).abs() <= v.abs() * 1e-3 + 6e-8, "{v} → {back}");
        }
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f32_to_f16(1.0), 0x3c00);
    }
}
