//! Texture-space painting: per-material texture sets, brush strokes, undo.
//!
//! A stroke works like this:
//! 1. `begin_stroke` snapshots every set's paint texture (the snapshot is both the
//!    undo entry and the "before" image) and clears the stroke masks.
//! 2. Each frame, queued dabs are rasterized in UV space into the stroke mask
//!    (accumulating coverage), then the mask is composited over the snapshot.
//! 3. The result is dilated into the display texture that the viewport samples.

use egui_wgpu::wgpu;
use glam::Mat4;
use s3dp_core::{Mesh, OrbitCamera, Vertex};

pub const PAINT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
const COVERAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
pub const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_DABS: usize = 1024;
const UNDO_LIMIT: usize = 24;

/// Brush settings for the current stroke.
#[derive(Clone, Copy, Debug)]
pub struct Brush {
    /// sRGB-encoded color, 0..1.
    pub color: [f32; 3],
    /// Radius in physical viewport pixels.
    pub radius: f32,
    /// 0 = soft, 1 = hard edge.
    pub hardness: f32,
    /// Maximum coverage of the whole stroke.
    pub opacity: f32,
    /// Coverage added by each dab.
    pub flow: f32,
    pub erase: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Self { color: [0.85, 0.35, 0.25], radius: 30.0, hardness: 0.6, opacity: 1.0, flow: 1.0, erase: false }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DabParams {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    viewport: [f32; 4],
    brush: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeParams {
    color: [f32; 4],
    params: [f32; 4],
}

/// GPU textures for one material slot of the mesh.
pub struct TextureSet {
    pub name: String,
    pub size: u32,
    first_index: u32,
    index_count: u32,
    paint: wgpu::Texture,
    paint_view: wgpu::TextureView,
    display: wgpu::Texture,
    mask_view: wgpu::TextureView,
    composite_params: wgpu::Buffer,
    composite_bg: Option<wgpu::BindGroup>,
    dilate_bg: wgpu::BindGroup,
    /// Group 1 of the viewport shader (display texture + sampler).
    pub(crate) view_bg: wgpu::BindGroup,
}

pub struct PaintSystem {
    coverage_pipeline: wgpu::RenderPipeline,
    dab_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    dilate_pipeline: wgpu::RenderPipeline,
    dab_params: wgpu::Buffer,
    dab_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    pub sets: Vec<TextureSet>,
    undo: Vec<Vec<wgpu::Texture>>,
    redo: Vec<Vec<wgpu::Texture>>,
    stroke_active: bool,
    pending: Vec<[f32; 4]>,
    brush: Brush,
}

fn uv_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRS,
    }
}

fn texture(device: &wgpu::Device, label: &str, size: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn clear_pass<'a>(encoder: &'a mut wgpu::CommandEncoder, view: &'a wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn copy_full(encoder: &mut wgpu::CommandEncoder, src: &wgpu::Texture, dst: &wgpu::Texture) {
    encoder.copy_texture_to_texture(src.as_image_copy(), dst.as_image_copy(), src.size());
}

impl PaintSystem {
    pub fn new(device: &wgpu::Device) -> Self {
        let uv = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paint_uv"),
            source: wgpu::ShaderSource::Wgsl(include_str!("paint_uv.wgsl").into()),
        });
        let composite = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let dilate = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dilate"),
            source: wgpu::ShaderSource::Wgsl(include_str!("dilate.wgsl").into()),
        });

        let uv_pipeline = |label, fs, format, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &uv,
                    entry_point: Some("vs_uv"),
                    compilation_options: Default::default(),
                    buffers: &[Some(uv_vertex_layout())],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &uv,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let coverage_pipeline = uv_pipeline("coverage", "fs_coverage", COVERAGE_FORMAT, None);
        // Accumulate: dst = src * (1 - dst) + dst.
        let accumulate = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let dab_pipeline = uv_pipeline(
            "dab",
            "fs_dab",
            MASK_FORMAT,
            Some(wgpu::BlendState { color: accumulate, alpha: accumulate }),
        );

        let full_pipeline = |label, module: &wgpu::ShaderModule, fs, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: None,
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let composite_pipeline = full_pipeline("composite", &composite, "fs_composite", PAINT_FORMAT);
        let dilate_pipeline = full_pipeline("dilate", &dilate, "fs_dilate", DISPLAY_FORMAT);

        let dab_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dab params"),
            size: std::mem::size_of::<DabParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dab_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dabs"),
            size: (MAX_DABS * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("paint display"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        Self {
            coverage_pipeline,
            dab_pipeline,
            composite_pipeline,
            dilate_pipeline,
            dab_params,
            dab_buffer,
            sampler,
            sets: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            stroke_active: false,
            pending: Vec::new(),
            brush: Brush::default(),
        }
    }

    /// Creates one texture set per mesh part and bakes UV coverage.
    pub fn set_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        view_layout: &wgpu::BindGroupLayout,
        size: u32,
    ) {
        use wgpu::TextureUsages as U;
        self.sets.clear();
        self.undo.clear();
        self.redo.clear();
        self.stroke_active = false;
        self.pending.clear();
        let mut encoder = device.create_command_encoder(&Default::default());
        for part in &mesh.parts {
            let paint = texture(device, "paint", size, PAINT_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC | U::COPY_DST);
            let display = texture(device, "paint display", size, DISPLAY_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC);
            let mask = texture(device, "stroke mask", size, MASK_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
            let coverage = texture(device, "uv coverage", size, COVERAGE_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
            let paint_view = paint.create_view(&Default::default());
            let display_view = display.create_view(&Default::default());
            let mask_view = mask.create_view(&Default::default());
            let coverage_view = coverage.create_view(&Default::default());

            clear_pass(&mut encoder, &paint_view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
            {
                let mut pass = clear_pass(&mut encoder, &coverage_view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(&self.coverage_pipeline);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(part.first_index..part.first_index + part.index_count, 0, 0..1);
            }

            let dilate_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dilate"),
                layout: &self.dilate_pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&paint_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&coverage_view) },
                ],
            });
            let view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("paint view"),
                layout: view_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&display_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            let composite_params = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("composite params"),
                size: std::mem::size_of::<CompositeParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.sets.push(TextureSet {
                name: part.name.clone(),
                size,
                first_index: part.first_index,
                index_count: part.index_count,
                paint,
                paint_view,
                display,
                mask_view,
                composite_params,
                composite_bg: None,
                dilate_bg,
                view_bg,
            });
        }
        for set in &self.sets {
            Self::dilate(&self.dilate_pipeline, &mut encoder, set);
        }
        queue.submit([encoder.finish()]);
    }

    pub fn is_stroke_active(&self) -> bool {
        self.stroke_active
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn snapshot(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Vec<wgpu::Texture> {
        self.sets
            .iter()
            .map(|set| {
                let tex = texture(
                    device,
                    "paint snapshot",
                    set.size,
                    PAINT_FORMAT,
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                );
                copy_full(encoder, &set.paint, &tex);
                tex
            })
            .collect()
    }

    pub fn begin_stroke(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, brush: Brush) {
        if self.sets.is_empty() {
            return;
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        let snapshot = self.snapshot(device, &mut encoder);
        for (set, before) in self.sets.iter_mut().zip(&snapshot) {
            clear_pass(&mut encoder, &set.mask_view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
            let before_view = before.create_view(&Default::default());
            set.composite_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout: &self.composite_pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: set.composite_params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&before_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&set.mask_view) },
                ],
            }));
            let params = CompositeParams {
                color: [brush.color[0], brush.color[1], brush.color[2], 1.0],
                params: [brush.opacity, if brush.erase { 1.0 } else { 0.0 }, 0.0, 0.0],
            };
            queue.write_buffer(&set.composite_params, 0, bytemuck::bytes_of(&params));
        }
        queue.submit([encoder.finish()]);
        self.undo.push(snapshot);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.brush = brush;
        self.stroke_active = true;
    }

    /// Queues dab centers (physical viewport pixels) for the next `flush`.
    pub fn add_dabs(&mut self, centers: &[[f32; 2]]) {
        if !self.stroke_active {
            return;
        }
        let room = MAX_DABS.saturating_sub(self.pending.len());
        let (r, f) = (self.brush.radius.max(0.5), self.brush.flow);
        self.pending.extend(centers.iter().take(room).map(|c| [c[0], c[1], r, f]));
    }

    pub fn end_stroke(&mut self) {
        self.stroke_active = false;
    }

    /// Applies pending dabs. `scene_depth` must hold the camera depth for `camera`.
    pub fn flush(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        scene_depth: &wgpu::TextureView,
        camera: &OrbitCamera,
        viewport_px: [u32; 2],
    ) {
        if self.pending.is_empty() {
            return;
        }
        let aspect = viewport_px[0] as f32 / viewport_px[1].max(1) as f32;
        let view_proj: Mat4 = camera.projection(aspect) * camera.view();
        let (near, far) = camera.clip_planes();
        let params = DabParams {
            view_proj: view_proj.to_cols_array_2d(),
            eye: camera.eye().extend(1.0).into(),
            viewport: [viewport_px[0] as f32, viewport_px[1] as f32, near, far],
            brush: [self.brush.hardness.clamp(0.0, 0.99), self.pending.len() as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.dab_params, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&self.dab_buffer, 0, bytemuck::cast_slice(&self.pending));
        self.pending.clear();

        let dab_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dab"),
            layout: &self.dab_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.dab_params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(scene_depth) },
                wgpu::BindGroupEntry { binding: 2, resource: self.dab_buffer.as_entire_binding() },
            ],
        });
        for set in &self.sets {
            {
                let mut pass = clear_pass(encoder, &set.mask_view, wgpu::LoadOp::Load);
                pass.set_pipeline(&self.dab_pipeline);
                pass.set_bind_group(0, &dab_bg, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(set.first_index..set.first_index + set.index_count, 0, 0..1);
            }
            if let Some(bg) = &set.composite_bg {
                let mut pass = clear_pass(encoder, &set.paint_view, wgpu::LoadOp::Load);
                pass.set_pipeline(&self.composite_pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..3, 0..1);
            }
            Self::dilate(&self.dilate_pipeline, encoder, set);
        }
    }

    fn dilate(pipeline: &wgpu::RenderPipeline, encoder: &mut wgpu::CommandEncoder, set: &TextureSet) {
        let view = set.display.create_view(&Default::default());
        let mut pass = clear_pass(encoder, &view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &set.dilate_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    fn restore(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, from_undo: bool) {
        let (src, dst) = if from_undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(state) = src.pop() else { return };
        let mut encoder = device.create_command_encoder(&Default::default());
        let current = self
            .sets
            .iter()
            .map(|set| {
                let tex = texture(
                    device,
                    "paint snapshot",
                    set.size,
                    PAINT_FORMAT,
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                );
                copy_full(&mut encoder, &set.paint, &tex);
                tex
            })
            .collect();
        dst.push(current);
        for (set, tex) in self.sets.iter().zip(&state) {
            copy_full(&mut encoder, tex, &set.paint);
            Self::dilate(&self.dilate_pipeline, &mut encoder, set);
        }
        queue.submit([encoder.finish()]);
    }

    pub fn undo(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if !self.stroke_active {
            self.restore(device, queue, true);
        }
    }

    pub fn redo(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if !self.stroke_active {
            self.restore(device, queue, false);
        }
    }

    /// Reads back a set's display texture as straight-alpha RGBA8 (sRGB-encoded).
    pub fn read_display(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize) -> Option<(u32, Vec<u8>)> {
        let set = self.sets.get(set)?;
        let mut data = read_rgba8(device, queue, &set.display)?;
        for px in data.chunks_exact_mut(4) {
            let a = px[3] as f32 / 255.0;
            if a > 0.0 {
                for c in &mut px[..3] {
                    *c = ((*c as f32 / a).round()).min(255.0) as u8;
                }
            }
        }
        Some((set.size, data))
    }
}

/// Blocking readback of a 4-byte-per-texel texture into tightly packed rows.
pub(crate) fn read_rgba8(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Option<Vec<u8>> {
    let size = texture.size();
    let row = size.width * 4;
    let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded * size.height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let mapped = buffer.get_mapped_range(..).ok()?;
    let mut out = Vec::with_capacity((row * size.height) as usize);
    for y in 0..size.height {
        let start = (y * padded) as usize;
        out.extend_from_slice(&mapped[start..start + row as usize]);
    }
    Some(out)
}

impl TextureSet {
    pub fn index_range(&self) -> std::ops::Range<u32> {
        self.first_index..self.first_index + self.index_count
    }
}
