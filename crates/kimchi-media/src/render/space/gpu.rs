//! The GPU 3D renderer (wgpu): Metal on macOS (Apple Silicon and Intel), Vulkan or DirectX 12
//! elsewhere. Renders offscreen with 4× multisampling, then reads the picture back.
//!
//! Meshes and textures are uploaded once and kept while the same `Arc` is alive; every frame
//! only writes the camera, lights and per-object uniforms.

use std::collections::HashMap;
use std::sync::Arc;

use tiny_skia::Pixmap;
use wgpu::util::DeviceExt;

use super::mesh::Mesh;
use super::{Frame3d, Texture};

const SHADER: &str = include_str!("gpu.wgsl");
const SAMPLES: u32 = 4;
const SHADOW_SIZE: u32 = 2048;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Per-object uniform stride (dynamic offsets must be 256-aligned).
const ITEM_STRIDE: u64 = 256;
const MAX_LIGHTS: usize = 8;

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
    shadow: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    cmp_sampler: wgpu::Sampler,
    shadow_tex: wgpu::Texture,
    white: (wgpu::Texture, wgpu::TextureView),
    meshes: HashMap<usize, (Arc<Mesh>, wgpu::Buffer, wgpu::Buffer, u32)>,
    textures: HashMap<usize, (Arc<Texture>, wgpu::Texture, wgpu::TextureView)>,
    targets: Option<Targets>,
}

/// Offscreen targets for one output size.
struct Targets {
    size: (u32, u32),
    msaa: wgpu::Texture,
    resolve: wgpu::Texture,
    depth: wgpu::Texture,
    readback: wgpu::Buffer,
    row: u32,
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
        let globals_and_item = SHADER[..SHADER.find("@group(0) @binding(0) var<uniform> g").expect("globals")].to_string();
        let shadow_src = format!("{globals_and_item}\n{shadow_src}");
        let main = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("3D"), source: wgpu::ShaderSource::Wgsl(main_src.into()) });
        let shadow_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("3D shadow"), source: wgpu::ShaderSource::Wgsl(shadow_src.into()) });

        let uniform = |binding: u32, dynamic: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: dynamic, min_binding_size: None },
            count: None,
        };
        let main_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                uniform(0, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let item_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("item"),
            entries: &[
                uniform(0, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
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
        let cmp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let white = upload(&device, &queue, &Texture { width: 1, height: 1, rgba: vec![255; 4] });
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
            shadow,
            sampler,
            cmp_sampler,
            shadow_tex,
            white,
            meshes: HashMap::new(),
            textures: HashMap::new(),
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

    pub(crate) fn render(&mut self, f: &Frame3d) -> Result<Pixmap, String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.draw(f);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("GPU: {e}"));
        }
        result
    }

    fn draw(&mut self, f: &Frame3d) -> Result<Pixmap, String> {
        let (w, h) = (f.width.max(1), f.height.max(1));
        if self.targets.as_ref().is_none_or(|t| t.size != (w, h)) {
            self.targets = Some(self.make_targets(w, h));
        }
        // Forget uploads whose meshes/textures nobody uses any more.
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
                    for v in m.pos[i].iter().chain(&m.normal[i]).chain(&m.uv[i]) {
                        vb.extend_from_slice(&v.to_le_bytes());
                    }
                }
                let ib: Vec<u8> = m.index.iter().flat_map(|i| i.to_le_bytes()).collect();
                if vb.is_empty() || ib.is_empty() {
                    continue;
                }
                let v = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("mesh"), contents: &vb, usage: wgpu::BufferUsages::VERTEX });
                let ix = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("mesh"), contents: &ib, usage: wgpu::BufferUsages::INDEX });
                self.meshes.insert(key, (it.mesh.clone(), v, ix, m.index.len() as u32));
            }
            if let Some(t) = &it.mat.texture {
                let key = Arc::as_ptr(t) as usize;
                if !self.textures.contains_key(&key) {
                    if self.textures.len() > 64 {
                        self.textures.clear();
                    }
                    let (tex, view) = upload(&self.device, &self.queue, t);
                    self.textures.insert(key, (t.clone(), tex, view));
                }
            }
        }

        // Globals.
        let mut g: Vec<f32> = vec![];
        g.extend(f.viewproj.flat());
        g.extend(f.shadow.map(|(m, _)| m.flat()).unwrap_or(super::math::M4::I.flat()));
        let n = f.lights.len().min(MAX_LIGHTS);
        g.extend([f.eye.0, f.eye.1, f.eye.2, n as f32]);
        g.extend([f.sky[0], f.sky[1], f.sky[2], f.shadow.map_or(-1.0, |(_, i)| i as f32)]);
        g.extend([f.ground[0], f.ground[1], f.ground[2], 0.0]);
        match f.fog {
            Some((near, far, c)) => {
                g.extend([c[0], c[1], c[2], 1.0]);
                g.extend([near, far, 0.0, 0.0]);
            }
            None => g.extend([0.0; 8]),
        }
        for i in 0..MAX_LIGHTS {
            match f.lights.get(i) {
                Some(l) => g.extend([l.v.0, l.v.1, l.v.2, if l.point { 1.0 } else { 0.0 }]),
                None => g.extend([0.0; 4]),
            }
        }
        for i in 0..MAX_LIGHTS {
            match f.lights.get(i) {
                Some(l) => g.extend([l.color[0], l.color[1], l.color[2], l.range]),
                None => g.extend([0.0; 4]),
            }
        }
        let globals = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("globals"), contents: &bytes(&g), usage: wgpu::BufferUsages::UNIFORM });
        let shadow_globals = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shadow globals"),
            contents: &bytes(&f.shadow.map(|(m, _)| m.flat()).unwrap_or(super::math::M4::I.flat())),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        // Per-object uniforms, one 256-byte slot each.
        let mut items: Vec<u8> = vec![0; (f.items.len().max(1) as u64 * ITEM_STRIDE) as usize];
        for (i, it) in f.items.iter().enumerate() {
            let mut u: Vec<f32> = vec![];
            u.extend(it.model.flat());
            u.extend(it.normal.flat());
            u.extend(it.mat.base);
            u.extend([it.mat.emissive[0], it.mat.emissive[1], it.mat.emissive[2], if it.mat.unlit { 1.0 } else { 0.0 }]);
            u.extend([it.mat.metallic, it.mat.roughness, 0.0, 0.0]);
            let b = bytes(&u);
            let at = i * ITEM_STRIDE as usize;
            items[at..at + b.len()].copy_from_slice(&b);
        }
        let item_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("items"), contents: &items, usage: wgpu::BufferUsages::UNIFORM });
        let item_binding = wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &item_buf, offset: 0, size: wgpu::BufferSize::new(ITEM_STRIDE) });

        let shadow_view = self.shadow_tex.create_view(&Default::default());
        let main_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &self.main_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.cmp_sampler) },
            ],
        });
        let shadow_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow globals"),
            layout: &self.shadow_globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: shadow_globals.as_entire_binding() }],
        });
        let shadow_item_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow item"),
            layout: &self.shadow_item_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: item_binding.clone() }],
        });
        // One item bind group per texture (the uniform slot is a dynamic offset).
        let mut tex_bgs: HashMap<usize, wgpu::BindGroup> = HashMap::new();
        for it in &f.items {
            let key = it.mat.texture.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize);
            if tex_bgs.contains_key(&key) {
                continue;
            }
            let view = if key == 0 { &self.white.1 } else { &self.textures[&key].2 };
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("item"),
                layout: &self.item_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: item_binding.clone() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            tex_bgs.insert(key, bg);
        }

        let t = self.targets.as_ref().expect("made above");
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("3D") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if f.shadow.is_some() {
                pass.set_pipeline(&self.shadow);
                pass.set_bind_group(0, &shadow_bg, &[]);
                for (i, it) in f.items.iter().enumerate() {
                    if it.mat.base[3] < 0.5 || it.mat.unlit {
                        continue;
                    }
                    let Some((_, vb, ib, count)) = self.meshes.get(&(Arc::as_ptr(&it.mesh) as usize)) else { continue };
                    pass.set_bind_group(1, &shadow_item_bg, &[(i as u64 * ITEM_STRIDE) as u32]);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*count, 0, 0..1);
                }
            }
        }
        {
            let msaa_view = t.msaa.create_view(&Default::default());
            let resolve_view = t.resolve.create_view(&Default::default());
            let depth_view = t.depth.create_view(&Default::default());
            let clear = f.background.map_or(wgpu::Color::TRANSPARENT, |c| {
                let a = c[3] as f64;
                wgpu::Color { r: c[0] as f64 * a, g: c[1] as f64 * a, b: c[2] as f64 * a, a }
            });
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
            let transparent = |it: &super::Item| it.mat.base[3] < 0.999 || it.mat.texture.as_ref().is_some_and(|t| t.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255));
            let mut order: Vec<usize> = (0..f.items.len()).filter(|&i| !transparent(&f.items[i])).collect();
            let opaque_count = order.len();
            let mut clear_items: Vec<usize> = (0..f.items.len()).filter(|&i| transparent(&f.items[i])).collect();
            clear_items.sort_by(|a, b| f.items[*b].depth.total_cmp(&f.items[*a].depth));
            order.extend(clear_items);
            for (k, &i) in order.iter().enumerate() {
                let it = &f.items[i];
                let Some((_, vb, ib, count)) = self.meshes.get(&(Arc::as_ptr(&it.mesh) as usize)) else { continue };
                pass.set_pipeline(if k < opaque_count { &self.opaque } else { &self.blended });
                let key = it.mat.texture.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize);
                pass.set_bind_group(1, &tex_bgs[&key], &[(i as u64 * ITEM_STRIDE) as u32]);
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
        self.queue.submit([enc.finish()]);

        let slice = t.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("GPU: {e}"))?;
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("GPU readback: {e}"))?;
        let mut out = vec![0u8; (w * h * 4) as usize];
        {
            let data = slice.get_mapped_range();
            for y in 0..h as usize {
                let src = &data[y * t.row as usize..y * t.row as usize + w as usize * 4];
                out[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(src);
            }
        }
        t.readback.unmap();
        Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(w, h).ok_or("size")?).ok_or_else(|| "bad picture".to_string())
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
        let row = (w * 4).div_ceil(256) * 256;
        Targets {
            size: (w, h),
            msaa: tex("3D msaa", COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT),
            resolve: tex("3D", COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC),
            depth: tex("3D depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT),
            readback: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("3D readback"),
                size: (row * h) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            row,
        }
    }
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, t: &Texture) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d { width: t.width.max(1), height: t.height.max(1), depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &t.rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * size.width), rows_per_image: Some(size.height) },
        size,
    );
    let view = tex.create_view(&Default::default());
    (tex, view)
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
