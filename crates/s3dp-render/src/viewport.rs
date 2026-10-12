use egui_wgpu::wgpu;
use glam::Mat4;
use s3dp_core::{Mesh, OrbitCamera, Vertex};
use wgpu::util::DeviceExt;

use crate::paint::{Brush, PaintSystem, SCENE_DEPTH_FORMAT};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadingMode {
    #[default]
    Lit,
    Unlit,
    UvChecker,
    Normals,
}

impl ShadingMode {
    pub const ALL: [ShadingMode; 4] = [Self::Lit, Self::Unlit, Self::UvChecker, Self::Normals];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lit => "Lit",
            Self::Unlit => "Unlit (albedo)",
            Self::UvChecker => "UV Checker",
            Self::Normals => "Normals",
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
    view_layout: wgpu::BindGroupLayout,
    pub paint: PaintSystem,
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
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paint view"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
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
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
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
                buffers: &[Some(vertex_layout)],
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
        Self {
            pipeline,
            depth_pipeline,
            view_layout,
            paint: PaintSystem::new(device),
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
        self.paint.set_mesh(device, queue, mesh, &vertices, &indices, &self.view_layout, texture_size);
        self.mesh = Some(GpuMesh { vertices, indices, index_count: mesh.indices.len() as u32 });
    }

    /// Reads back the last rendered frame as sRGB RGBA8 (width, height, pixels).
    pub fn read_color(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<(u32, u32, Vec<u8>)> {
        let t = self.targets.as_ref()?;
        let data = crate::paint::read_rgba8(device, queue, &t.resolve_tex)?;
        Some((t.size[0], t.size[1], data))
    }

    pub fn texture_size(&self) -> u32 {
        self.texture_size
    }

    pub fn begin_stroke(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, brush: Brush) {
        self.paint.begin_stroke(device, queue, brush);
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
            params: [mode as u32 as f32, 8.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let targets = self.targets.as_ref().unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        if let (Some(mesh), Some((cam, px))) = (&self.mesh, &self.last_frame) {
            // Dabs were placed against the previous frame; its depth is still in scene_depth.
            if *px == size {
                self.paint.flush(device, queue, &mut encoder, &mesh.vertices, &mesh.indices, &targets.scene_depth, cam, size);
            }
        }
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
        self.last_frame = Some((camera.clone(), size));
        (&targets.resolve, recreated)
    }
}
