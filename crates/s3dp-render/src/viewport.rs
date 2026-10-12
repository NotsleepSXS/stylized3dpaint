use egui_wgpu::wgpu;
use glam::Mat4;
use s3dp_core::{BakeSettings, BakedMap, ChannelId, Mesh, OrbitCamera, Vertex};
use wgpu::util::DeviceExt;

use crate::bake::{BakeJob, Baker};
use crate::paint::{Brush, Inspect, Painter, SCENE_DEPTH_FORMAT};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;
const PICK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadingMode {
    #[default]
    Lit,
    Toon,
    /// Base Color + Emissive without lighting (stylized work is judged this way, spec §6.7).
    Unlit,
    UvChecker,
    Normals,
    /// One material channel.
    Channel(ChannelId),
    /// A baked mesh map.
    Baked(BakedMap),
    /// The mask of the active layer.
    Mask,
}

impl ShadingMode {
    /// Modes on F1–F5; channels, mesh maps and the mask are listed separately.
    pub const MAIN: [ShadingMode; 5] = [Self::Lit, Self::Toon, Self::Unlit, Self::UvChecker, Self::Normals];

    fn gpu_id(self) -> u32 {
        match self {
            Self::Lit => 0,
            Self::Unlit => 1,
            Self::UvChecker => 2,
            Self::Normals => 3,
            Self::Toon => 8,
            Self::Channel(c) if c.is_srgb() => 52,
            Self::Channel(c) if c.is_color() => 51,
            Self::Baked(m) if m.is_color() => 51,
            Self::Channel(_) | Self::Baked(_) | Self::Mask => 50,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lit => "Material (Lit)",
            Self::Toon => "Toon",
            Self::Unlit => "Base Color (Unlit)",
            Self::UvChecker => "UV Checker",
            Self::Normals => "Normals",
            Self::Channel(c) => c.label(),
            Self::Baked(m) => m.label(),
            Self::Mask => "Layer mask",
        }
    }

    fn inspect(self, active: Option<u64>) -> Inspect {
        match self {
            Self::Channel(c) => Inspect::Channel(c),
            Self::Baked(m) => Inspect::Baked(m),
            Self::Mask => active.map_or(Inspect::None, Inspect::Mask),
            _ => Inspect::None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    params: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    resolve_tex: wgpu::Texture,
    /// Single-sample camera depth used for paint occlusion.
    scene_depth: wgpu::TextureView,
}

/// Offscreen 3D viewport. Renders into its own texture, which the UI shows as an image.
pub struct Viewport {
    pipeline: wgpu::RenderPipeline,
    depth_pipeline: wgpu::RenderPipeline,
    pick_pipeline: wgpu::RenderPipeline,
    view_layout: wgpu::BindGroupLayout,
    pub paint: Painter,
    baker: Baker,
    bake: Option<BakeJob>,
    bake_finished: bool,
    /// Camera / size / mode of the frame currently in the output texture.
    shown: Option<(String, [u32; 2], ShadingMode)>,
    texture_size: u32,
    /// Camera and size of the last rendered frame (dabs are in its pixel space).
    last_frame: Option<(OrbitCamera, [u32; 2])>,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    mesh: Option<GpuMesh>,
    targets: Option<Targets>,
}

impl Viewport {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport"),
            source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport globals"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        // 7 material channels, baked AO, baked normal, inspect slot, then the sampler.
        let textures = 10u32;
        let mut view_entries: Vec<wgpu::BindGroupLayoutEntry> = (0..textures)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        view_entries.push(wgpu::BindGroupLayoutEntry {
            binding: textures,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paint view"),
            entries: &view_entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport"),
            bind_group_layouts: &[Some(&bgl), Some(&view_layout)],
            immediate_size: 0,
        });
        let depth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene depth"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
        };
        let depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene depth"),
            layout: Some(&depth_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout.clone())],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SCENE_DEPTH_FORMAT,
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
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport mesh"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout.clone())],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(COLOR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let pick_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pick"),
            layout: Some(&depth_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_pick"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout.clone())],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SCENE_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_pick"),
                compilation_options: Default::default(),
                targets: &[Some(PICK_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            depth_pipeline,
            pick_pipeline,
            view_layout,
            paint: Painter::new(device),
            baker: Baker::new(device),
            bake: None,
            bake_finished: false,
            shown: None,
            texture_size: 2048,
            last_frame: None,
            globals,
            bind_group,
            mesh: None,
            targets: None,
        }
    }

    /// Uploads the mesh and creates fresh texture sets of `texture_size`².
    pub fn set_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh, texture_size: u32) {
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.texture_size = texture_size;
        self.paint.set_mesh(device, queue, mesh, &self.view_layout, texture_size);
        let scale = self.paint.scale;
        self.baker.rasterize_geometry(device, queue, &self.paint.sets, &vertices, &indices, scale.bbox_min, scale.bbox_size);
        self.paint.build_seam_links(device, queue);
        self.paint.baked_changed();
        self.bake = None;
        self.mesh = Some(GpuMesh { vertices, indices, index_count: mesh.indices.len() as u32 });
    }

    /// Texture set and UV of the surface at a pixel of the last rendered frame.
    pub fn pick(&self, device: &wgpu::Device, queue: &wgpu::Queue, px: [u32; 2]) -> Option<(usize, [f32; 2])> {
        let mesh = self.mesh.as_ref()?;
        let (_, size) = self.last_frame.as_ref()?;
        if px[0] >= size[0] || px[1] >= size[1] {
            return None;
        }
        use wgpu::TextureUsages as U;
        let color = crate::gpu::texture2d(device, "pick", *size, PICK_FORMAT, U::RENDER_ATTACHMENT | U::COPY_SRC);
        let depth = crate::gpu::texture2d(device, "pick depth", *size, SCENE_DEPTH_FORMAT, U::RENDER_ATTACHMENT);
        let (color_view, depth_view) = (crate::gpu::view(&color), crate::gpu::view(&depth));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pick"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
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
            pass.set_scissor_rect(px[0], px[1], 1, 1);
            pass.set_pipeline(&self.pick_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            for (i, set) in self.paint.sets.iter().enumerate() {
                pass.draw_indexed(set.index_range(), 0, i as u32..i as u32 + 1);
            }
        }
        queue.submit([encoder.finish()]);
        let bytes = crate::gpu::read_region(device, queue, &color, px[0], px[1])?;
        let f: &[f32] = bytemuck::cast_slice(&bytes);
        (f[2] >= 1.0).then(|| (f[2] as usize - 1, [f[0], f[1]]))
    }

    /// Reads back the last rendered frame as sRGB RGBA8 (width, height, pixels).
    pub fn read_color(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<(u32, u32, Vec<u8>)> {
        let t = self.targets.as_ref()?;
        let data = crate::gpu::read_texture(device, queue, &t.resolve_tex)?;
        Some((t.size[0], t.size[1], data))
    }

    /// Starts baking mesh maps for every texture set (advanced during `render`).
    pub fn start_bake(&mut self, device: &wgpu::Device, low: &Mesh, high: Option<&Mesh>, settings: &BakeSettings) {
        self.bake = Some(BakeJob::new(device, low, high, settings, self.texture_size, self.paint.sets.len()));
    }

    /// True once after a background bake completes.
    pub fn take_bake_finished(&mut self) -> bool {
        std::mem::take(&mut self.bake_finished)
    }

    pub fn cancel_bake(&mut self) {
        self.bake = None;
    }

    /// Progress 0..1 of the running bake.
    pub fn bake_progress(&self) -> Option<f32> {
        self.bake.as_ref().map(|b| b.progress())
    }

    /// Bakes synchronously (tests).
    pub fn bake_now(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, low: &Mesh, high: Option<&Mesh>, settings: &BakeSettings) {
        let mut job = BakeJob::new(device, low, high, settings, self.texture_size, self.paint.sets.len());
        if let Some(mesh) = &self.mesh {
            job.run_to_end(device, queue, &self.baker, &self.paint.sets, &mesh.vertices, &mesh.indices);
        }
        self.paint.baked_changed();
    }

    pub fn texture_size(&self) -> u32 {
        self.texture_size
    }

    pub fn begin_stroke(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, brush: Brush) -> Result<(), &'static str> {
        self.paint.begin_stroke(device, queue, brush)
    }

    /// Returns true when the output texture was (re)created and must be re-registered with egui.
    fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        let make_tex = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                view_formats: &[],
            })
        };
        let make = |label, format, samples, usage| make_tex(label, format, samples, usage).create_view(&Default::default());
        let resolve_tex = make_tex(
            "viewport color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        self.targets = Some(Targets {
            size,
            msaa: make("viewport msaa", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT),
            depth: make("viewport depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT),
            resolve: resolve_tex.create_view(&Default::default()),
            resolve_tex,
            scene_depth: make(
                "scene depth",
                SCENE_DEPTH_FORMAT,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
        });
        true
    }

    /// Renders the scene, first applying any queued paint dabs (in the pixel space of the
    /// previous frame). Returns the output view and whether it is new since the last call.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
        camera: &OrbitCamera,
        mode: ShadingMode,
    ) -> (&wgpu::TextureView, bool) {
        let size = [size[0].max(1), size[1].max(1)];
        let recreated = self.ensure_targets(device, size);
        let aspect = size[0] as f32 / size[1] as f32;
        let view_proj: Mat4 = camera.projection(aspect) * camera.view();
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            eye: camera.eye().extend(1.0).into(),
            params: [mode.gpu_id() as f32, 8.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let inspect = mode.inspect(self.paint.active);
        if inspect != self.paint.inspect {
            self.paint.inspect = inspect;
            self.paint.rebuild_view_groups(device);
            self.shown = None;
        }
        if let (Some(job), Some(mesh)) = (&mut self.bake, &self.mesh) {
            job.step(device, queue, &self.baker, &self.paint.sets, &mesh.vertices, &mesh.indices, 4);
            if job.is_done() {
                self.bake = None;
                self.bake_finished = true;
                self.paint.baked_changed();
            }
        }
        let targets = self.targets.as_ref().unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        if let Some(mesh) = &self.mesh {
            // Dabs were placed against the previous frame; its depth is still in scene_depth.
            let scene = match &self.last_frame {
                Some((cam, px)) if *px == size => Some((&targets.scene_depth, cam, size)),
                _ => None,
            };
            self.paint.update(device, queue, &mut encoder, &mesh.vertices, &mesh.indices, scene);
        }
        // Skip drawing when nothing visible changed (egui repaints on every mouse move).
        let shown = (format!("{camera:?}"), size, mode);
        let changed = self.paint.take_changed();
        if !recreated && !changed && self.shown.as_ref() == Some(&shown) {
            queue.submit([encoder.finish()]);
            self.track_recompose(queue);
            return (&self.targets.as_ref().unwrap().resolve, false);
        }
        self.shown = Some(shown);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa,
                    depth_slice: None,
                    resolve_target: Some(&targets.resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.035, g: 0.037, b: 0.042, a: 1.0 }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(mesh) = &self.mesh {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                for set in &self.paint.sets {
                    let range = set.index_range();
                    pass.set_bind_group(1, &set.view_bg, &[]);
                    pass.draw_indexed(range, 0, 0..1);
                }
            }
        }
        if let Some(mesh) = &self.mesh {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.scene_depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.depth_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
        queue.submit([encoder.finish()]);
        self.track_recompose(queue);
        self.last_frame = Some((camera.clone(), size));
        (&self.targets.as_ref().unwrap().resolve, recreated)
    }

    /// Clears the painter's GPU-busy flag once a submitted recomposite has finished.
    fn track_recompose(&mut self, queue: &wgpu::Queue) {
        if let Some(busy) = self.paint.take_submitted_recompose() {
            queue.on_submitted_work_done(move || busy.store(false, std::sync::atomic::Ordering::Release));
        }
    }
}
