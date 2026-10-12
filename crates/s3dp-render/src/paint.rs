//! Layered texture painting.
//!
//! The document is a stack of layers shared by every texture set (material slot).
//! Each layer stores, per texture set, one lazily created texture per channel plus an
//! optional mask. Textures are never modified after a stroke ends: a stroke paints into
//! a fresh copy, so undo is just restoring an older (cheaply cloned) layer list.
//!
//! Per frame: queued dabs are rasterized in UV space into a stroke mask, the mask is
//! composited over the pre-stroke texture, then dirty channels of the stack are blended
//! bottom-to-top and dilated into the display textures sampled by the viewport.

use std::collections::HashMap;

use egui_wgpu::wgpu;
use glam::Mat4;
use s3dp_core::{BakedMap, Channel, ChannelMask, ChannelValues, Layer, LayerKind, Mesh, OrbitCamera, Vertex};

use crate::gpu::{self, LAYER_USAGE};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const SCALAR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const ACC_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const STROKE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
const COVERAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
pub const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const THUMB_SIZE: u32 = 64;
const THUMB_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const MAX_DABS: usize = 1024;
const UNDO_LIMIT: usize = 40;
const UNIFORM_STRIDE: u64 = 256;

pub fn channel_format(c: Channel) -> wgpu::TextureFormat {
    if c.is_color() { COLOR_FORMAT } else { SCALAR_FORMAT }
}

/// Brush settings for the current stroke.
#[derive(Clone, Copy, Debug)]
pub struct Brush {
    /// Values painted into each channel (sRGB-encoded colors).
    pub values: ChannelValues,
    /// Channels the brush writes.
    pub channels: ChannelMask,
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
        Self {
            values: ChannelValues { base_color: [0.85, 0.35, 0.25], ..ChannelValues::DEFAULTS },
            channels: ChannelMask::BASE_COLOR,
            radius: 30.0,
            hardness: 0.6,
            opacity: 1.0,
            flow: 1.0,
            erase: false,
        }
    }
}

/// What the brush edits on the active layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditTarget {
    #[default]
    Content,
    Mask,
}

#[derive(Clone, Default)]
pub struct LayerTextures {
    pub channels: [Option<wgpu::Texture>; Channel::COUNT],
    pub mask: Option<wgpu::Texture>,
}

#[derive(Clone)]
pub struct GpuLayer {
    pub meta: Layer,
    /// One entry per texture set.
    pub sets: Vec<LayerTextures>,
}

#[derive(Clone)]
struct Snapshot {
    layers: Vec<GpuLayer>,
    active: usize,
    defaults: ChannelValues,
}

/// Geometry and output textures of one material slot.
pub struct TextureSet {
    pub name: String,
    pub size: u32,
    first_index: u32,
    index_count: u32,
    coverage_view: wgpu::TextureView,
    stroke_tex: wgpu::Texture,
    stroke_mask: wgpu::TextureView,
    zero_color: wgpu::Texture,
    zero_scalar: wgpu::Texture,
    acc: [wgpu::Texture; 2],
    /// Final channel textures (Rgba8 for colors, R8 for scalars), dilated.
    pub display: Vec<wgpu::Texture>,
    /// Baked mesh maps, indexed by [`BakedMap::index`].
    pub baked: Vec<wgpu::Texture>,
    /// Group 1 of the viewport shader.
    pub(crate) view_bg: wgpu::BindGroup,
}

impl TextureSet {
    pub fn index_range(&self) -> std::ops::Range<u32> {
        self.first_index..self.first_index + self.index_count
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

/// Two vec4s; used by the stroke composite, blend and thumbnail shaders.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vec4Pair {
    a: [f32; 4],
    b: [f32; 4],
}

struct StrokeTarget {
    after: wgpu::TextureView,
    format: wgpu::TextureFormat,
    bind_group: wgpu::BindGroup,
}

struct Stroke {
    brush: Brush,
    targets: Vec<StrokeTarget>,
    channels: ChannelMask,
    on_mask: bool,
    /// Uniform buffer kept alive for the bind groups.
    _params: wgpu::Buffer,
}

/// A thumbnail texture plus a revision counter so the UI knows when to re-register it.
pub struct Thumb {
    pub texture: wgpu::Texture,
    pub revision: u64,
    source: wgpu::Texture,
}

pub struct Painter {
    coverage_pipeline: wgpu::RenderPipeline,
    dab_pipeline: wgpu::RenderPipeline,
    stroke_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    blend_pipeline: wgpu::RenderPipeline,
    dilate_color: wgpu::RenderPipeline,
    dilate_scalar: wgpu::RenderPipeline,
    thumb_pipeline: wgpu::RenderPipeline,
    blend_layout: wgpu::BindGroupLayout,
    blend_params: wgpu::Buffer,
    blend_capacity: u64,
    dab_params: wgpu::Buffer,
    dab_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    dummy: wgpu::Texture,

    pub sets: Vec<TextureSet>,
    /// Bottom to top.
    pub layers: Vec<GpuLayer>,
    pub active: usize,
    pub target: EditTarget,
    pub defaults: ChannelValues,
    next_id: u64,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    stroke: Option<Stroke>,
    pending: Vec<[f32; 4]>,
    dirty: [bool; Channel::COUNT],
    /// Keyed by (layer id, is_mask) for texture set `thumb_set`.
    thumbs: HashMap<(u64, bool), Thumb>,
    thumb_revision: u64,
    pub thumb_set: usize,
    /// Bumped on every document change (for "unsaved changes" tracking).
    revision: u64,
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

fn shader(device: &wgpu::Device, label: &str, src: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    })
}

fn rgb_color(v: [f32; 3]) -> wgpu::Color {
    wgpu::Color { r: v[0] as f64, g: v[1] as f64, b: v[2] as f64, a: 1.0 }
}

fn uniform_slice(buffer: &wgpu::Buffer, offset: u64) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer,
        offset,
        size: wgpu::BufferSize::new(std::mem::size_of::<Vec4Pair>() as u64),
    })
}

impl Painter {
    pub fn new(device: &wgpu::Device) -> Self {
        let uv = shader(device, "paint_uv", include_str!("paint_uv.wgsl"));
        let composite = shader(device, "composite", include_str!("composite.wgsl"));
        let dilate = shader(device, "dilate", include_str!("dilate.wgsl"));
        let blend = shader(device, "blend", include_str!("blend.wgsl"));
        let thumb = shader(device, "thumb", include_str!("thumb.wgsl"));

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
        let dab_pipeline =
            uv_pipeline("dab", "fs_dab", STROKE_FORMAT, Some(wgpu::BlendState { color: accumulate, alpha: accumulate }));

        let stroke_pipelines = [COLOR_FORMAT, SCALAR_FORMAT, MASK_FORMAT]
            .into_iter()
            .map(|f| (f, gpu::fullscreen_pipeline(device, "stroke composite", &composite, "fs_composite", f)))
            .collect();

        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let blend_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blend"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Vec4Pair>() as u64),
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
            ],
        });
        let blend_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blend"),
            bind_group_layouts: &[Some(&blend_layout)],
            immediate_size: 0,
        });
        let blend_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer blend"),
            layout: Some(&blend_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blend,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blend,
                entry_point: Some("fs_blend"),
                compilation_options: Default::default(),
                targets: &[Some(ACC_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let blend_capacity = 1024;
        let blend_params = Self::make_blend_buffer(device, blend_capacity);
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
        let dummy = gpu::texture(device, "dummy", 1, COLOR_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        Self {
            coverage_pipeline,
            dab_pipeline,
            stroke_pipelines,
            blend_pipeline,
            dilate_color: gpu::fullscreen_pipeline(device, "dilate color", &dilate, "fs_dilate", COLOR_FORMAT),
            dilate_scalar: gpu::fullscreen_pipeline(device, "dilate scalar", &dilate, "fs_dilate", MASK_FORMAT),
            thumb_pipeline: gpu::fullscreen_pipeline(device, "thumb", &thumb, "fs_thumb", THUMB_FORMAT),
            blend_layout,
            blend_params,
            blend_capacity,
            dab_params,
            dab_buffer,
            sampler,
            dummy,
            sets: Vec::new(),
            layers: Vec::new(),
            active: 0,
            target: EditTarget::Content,
            defaults: ChannelValues::DEFAULTS,
            next_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            stroke: None,
            pending: Vec::new(),
            dirty: [true; Channel::COUNT],
            thumbs: HashMap::new(),
            thumb_revision: 0,
            thumb_set: 0,
            revision: 0,
        }
    }

    fn make_blend_buffer(device: &wgpu::Device, draws: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blend params"),
            size: draws * UNIFORM_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    // ---------------------------------------------------------------- mesh / document

    /// Creates texture sets for the mesh parts and resets the document to one empty layer.
    #[allow(clippy::too_many_arguments)]
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
        self.stroke = None;
        self.pending.clear();
        let mut encoder = device.create_command_encoder(&Default::default());
        for part in &mesh.parts {
            let coverage =
                gpu::texture(device, "uv coverage", size, COVERAGE_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
            let coverage_view = gpu::view(&coverage);
            {
                let mut pass = gpu::pass(&mut encoder, &coverage_view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(&self.coverage_pipeline);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(part.first_index..part.first_index + part.index_count, 0, 0..1);
            }
            let stroke_tex =
                gpu::texture(device, "stroke mask", size, STROKE_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
            let zero_color = gpu::texture(device, "zero", size, COLOR_FORMAT, LAYER_USAGE);
            let zero_scalar = gpu::texture(device, "zero", size, SCALAR_FORMAT, LAYER_USAGE);
            gpu::clear(&mut encoder, &zero_color, wgpu::Color::TRANSPARENT);
            gpu::clear(&mut encoder, &zero_scalar, wgpu::Color::TRANSPARENT);
            let acc = [0, 1].map(|_| {
                gpu::texture(device, "layer acc", size, ACC_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING)
            });
            let display: Vec<wgpu::Texture> = Channel::ALL
                .iter()
                .map(|c| {
                    let format = if c.is_color() { COLOR_FORMAT } else { MASK_FORMAT };
                    gpu::texture(device, c.label(), size, format, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC)
                })
                .collect();
            let baked: Vec<wgpu::Texture> = BakedMap::ALL
                .iter()
                .map(|&m| {
                    let t = gpu::texture(device, m.label(), size, crate::bake::baked_format(m), LAYER_USAGE);
                    gpu::clear(&mut encoder, &t, rgb_color(m.default_value()));
                    t
                })
                .collect();
            let views: Vec<wgpu::TextureView> = display.iter().chain(&baked).map(gpu::view).collect();
            let mut entries: Vec<wgpu::BindGroupEntry> =
                views.iter().enumerate().map(|(i, v)| gpu::tex_entry(i as u32, v)).collect();
            entries.push(wgpu::BindGroupEntry {
                binding: (Channel::COUNT + BakedMap::COUNT) as u32,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            });
            let view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("paint view"),
                layout: view_layout,
                entries: &entries,
            });
            self.sets.push(TextureSet {
                name: part.name.clone(),
                size,
                first_index: part.first_index,
                index_count: part.index_count,
                coverage_view,
                stroke_mask: gpu::view(&stroke_tex),
                stroke_tex,
                zero_color,
                zero_scalar,
                acc,
                display,
                baked,
                view_bg,
            });
        }
        queue.submit([encoder.finish()]);
        self.reset_document();
    }

    /// One empty paint layer, no history.
    pub fn reset_document(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.thumbs.clear();
        self.next_id = 1;
        self.layers.clear();
        self.defaults = ChannelValues::DEFAULTS;
        let meta = self.new_layer_meta("Layer 1", LayerKind::Paint);
        let layer = self.gpu_layer(meta);
        self.layers.push(layer);
        self.active = 0;
        self.target = EditTarget::Content;
        self.thumb_set = 0;
        self.revision += 1;
        self.mark_all_dirty();
    }

    /// Replaces the layer stack (project loading).
    pub fn load_document(&mut self, layers: Vec<GpuLayer>, defaults: ChannelValues, active: usize) {
        self.undo.clear();
        self.redo.clear();
        self.thumbs.clear();
        self.next_id = layers.iter().map(|l| l.meta.id).max().unwrap_or(0) + 1;
        self.layers = layers;
        self.defaults = defaults;
        self.active = active.min(self.layers.len().saturating_sub(1));
        self.target = EditTarget::Content;
        self.revision += 1;
        self.mark_all_dirty();
    }

    fn new_layer_meta(&mut self, name: &str, kind: LayerKind) -> Layer {
        let id = self.next_id;
        self.next_id += 1;
        Layer::new(id, name, kind)
    }

    fn gpu_layer(&self, meta: Layer) -> GpuLayer {
        GpuLayer { meta, sets: vec![LayerTextures::default(); self.sets.len()] }
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty = [true; Channel::COUNT];
    }

    /// Saves the current state for undo. Call before every document change.
    pub fn checkpoint(&mut self) {
        self.revision += 1;
        self.undo.push(Snapshot { layers: self.layers.clone(), active: self.active, defaults: self.defaults });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Marks the document as changed without an undo step (e.g. after baking).
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    /// Changes whenever the document changes; compare with a saved value to detect edits.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() && self.stroke.is_none()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty() && self.stroke.is_none()
    }

    fn swap_state(&mut self, from_undo: bool) {
        if self.stroke.is_some() {
            return;
        }
        let (src, dst) = if from_undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(state) = src.pop() else { return };
        self.revision += 1;
        dst.push(Snapshot { layers: std::mem::take(&mut self.layers), active: self.active, defaults: self.defaults });
        self.layers = state.layers;
        self.active = state.active.min(self.layers.len().saturating_sub(1));
        self.defaults = state.defaults;
        if !self.layers[self.active].meta.has_mask {
            self.target = EditTarget::Content;
        }
        self.mark_all_dirty();
    }

    pub fn undo(&mut self) {
        self.swap_state(true);
    }

    pub fn redo(&mut self) {
        self.swap_state(false);
    }

    // ---------------------------------------------------------------- layer operations

    pub fn active_layer(&self) -> Option<&Layer> {
        self.layers.get(self.active).map(|l| &l.meta)
    }

    /// Adds a layer above the active one and selects it.
    pub fn add_layer(&mut self, kind: LayerKind) {
        self.checkpoint();
        let fill = matches!(kind, LayerKind::Fill(_));
        let count = self.layers.iter().filter(|l| l.meta.is_fill() == fill).count() + 1;
        let name = if fill { format!("Fill {count}") } else { format!("Layer {count}") };
        let meta = self.new_layer_meta(&name, kind);
        let at = if self.layers.is_empty() { 0 } else { self.active + 1 };
        let layer = self.gpu_layer(meta);
        self.layers.insert(at, layer);
        self.active = at;
        self.target = EditTarget::Content;
        self.mark_all_dirty();
    }

    pub fn delete_layer(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        self.checkpoint();
        self.layers.remove(index);
        if self.layers.is_empty() {
            let meta = self.new_layer_meta("Layer 1", LayerKind::Paint);
            let layer = self.gpu_layer(meta);
            self.layers.push(layer);
        }
        self.active = self.active.min(self.layers.len() - 1);
        self.target = EditTarget::Content;
        self.mark_all_dirty();
    }

    pub fn duplicate_layer(&mut self, index: usize) {
        let Some(src) = self.layers.get(index).cloned() else { return };
        self.checkpoint();
        let mut meta = src.meta.clone();
        meta.id = self.next_id;
        self.next_id += 1;
        meta.name = format!("{} copy", meta.name);
        // Textures are immutable once a stroke ends, so sharing them is safe.
        self.layers.insert(index + 1, GpuLayer { meta, sets: src.sets });
        self.active = index + 1;
        self.mark_all_dirty();
    }

    /// Moves layer `from` so that it ends up at index `to` (in the final list).
    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || from == to {
            return;
        }
        self.checkpoint();
        let layer = self.layers.remove(from);
        let to = to.min(self.layers.len());
        self.layers.insert(to, layer);
        self.active = to;
        self.mark_all_dirty();
    }

    /// Edits layer metadata (visibility, opacity, blend, name...). Pass `checkpoint=false`
    /// for continuous edits like slider drags after the first change.
    pub fn edit_layer(&mut self, index: usize, checkpoint: bool, f: impl FnOnce(&mut Layer)) {
        if index >= self.layers.len() {
            return;
        }
        let before = self.layers[index].meta.clone();
        let mut after = before.clone();
        f(&mut after);
        if after == before {
            return;
        }
        if checkpoint {
            self.checkpoint();
        }
        self.layers[index].meta = after;
        self.mark_all_dirty();
    }

    pub fn set_defaults(&mut self, defaults: ChannelValues, checkpoint: bool) {
        if defaults == self.defaults {
            return;
        }
        if checkpoint {
            self.checkpoint();
        }
        self.defaults = defaults;
        self.mark_all_dirty();
    }

    /// Adds a mask filled with white (reveal all) or black (hide all) and targets it.
    pub fn add_mask(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, index: usize, white: bool) {
        if index >= self.layers.len() || self.layers[index].meta.has_mask {
            return;
        }
        self.checkpoint();
        let mut encoder = device.create_command_encoder(&Default::default());
        let v = if white { 1.0 } else { 0.0 };
        for (set, tex) in self.sets.iter().zip(self.layers[index].sets.iter_mut()) {
            let mask = gpu::texture(device, "layer mask", set.size, MASK_FORMAT, LAYER_USAGE);
            gpu::clear(&mut encoder, &mask, wgpu::Color { r: v, g: v, b: v, a: 1.0 });
            tex.mask = Some(mask);
        }
        queue.submit([encoder.finish()]);
        let layer = &mut self.layers[index].meta;
        layer.has_mask = true;
        layer.mask_enabled = true;
        self.active = index;
        self.target = EditTarget::Mask;
        self.mark_all_dirty();
    }

    pub fn delete_mask(&mut self, index: usize) {
        if index >= self.layers.len() || !self.layers[index].meta.has_mask {
            return;
        }
        self.checkpoint();
        for tex in &mut self.layers[index].sets {
            tex.mask = None;
        }
        self.layers[index].meta.has_mask = false;
        self.target = EditTarget::Content;
        self.mark_all_dirty();
    }

    // ---------------------------------------------------------------- strokes

    pub fn is_stroke_active(&self) -> bool {
        self.stroke.is_some()
    }

    /// Starts a stroke on the active layer. Returns a user-facing reason if it can't.
    pub fn begin_stroke(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, brush: Brush) -> Result<(), &'static str> {
        let Some(layer) = self.layers.get(self.active) else { return Err("No layer selected") };
        if !layer.meta.visible {
            return Err("The active layer is hidden");
        }
        let on_mask = layer.meta.has_mask && self.target == EditTarget::Mask;
        let channels = if on_mask {
            ChannelMask::ALL
        } else {
            if layer.meta.is_fill() {
                return Err("Fill layers can't be painted — add a mask and paint the mask");
            }
            let mut m = brush.channels;
            for c in Channel::ALL {
                m.set(c, m.has(c) && layer.meta.channels.has(c));
            }
            if !m.0.iter().any(|&b| b) {
                return Err("No channel is enabled for both the brush and the layer");
            }
            m
        };
        self.checkpoint();

        struct Job {
            set: usize,
            before: wgpu::Texture,
            after: wgpu::Texture,
            value: [f32; 3],
            mode: f32,
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut jobs = Vec::new();
        for (si, set) in self.sets.iter().enumerate() {
            gpu::clear(&mut encoder, &set.stroke_tex, wgpu::Color::TRANSPARENT);
            let tex = &mut self.layers[self.active].sets[si];
            if on_mask {
                let Some(before) = tex.mask.clone() else { continue };
                let after = gpu::duplicate(device, &mut encoder, &before);
                tex.mask = Some(after.clone());
                let c = brush.values.base_color;
                let luma = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
                // Erasing a mask reveals the layer again.
                let v = if brush.erase { 1.0 } else { luma };
                jobs.push(Job { set: si, before, after, value: [v; 3], mode: 3.0 });
            } else {
                for c in Channel::ALL.into_iter().filter(|&c| channels.has(c)) {
                    let zero = if c.is_color() { &set.zero_color } else { &set.zero_scalar };
                    let before = tex.channels[c.index()].clone().unwrap_or_else(|| zero.clone());
                    let after = gpu::duplicate(device, &mut encoder, &before);
                    tex.channels[c.index()] = Some(after.clone());
                    let mode = if brush.erase { 2.0 } else if c.is_color() { 0.0 } else { 1.0 };
                    jobs.push(Job { set: si, before, after, value: brush.values.get(c), mode });
                }
            }
        }
        queue.submit([encoder.finish()]);

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stroke params"),
            size: (jobs.len().max(1) as u64) * UNIFORM_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut targets = Vec::new();
        for (i, job) in jobs.into_iter().enumerate() {
            let p = Vec4Pair { a: [job.value[0], job.value[1], job.value[2], 1.0], b: [brush.opacity, job.mode, 0.0, 0.0] };
            let offset = i as u64 * UNIFORM_STRIDE;
            queue.write_buffer(&params, offset, bytemuck::bytes_of(&p));
            let format = job.after.format();
            let before_view = gpu::view(&job.before);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("stroke composite"),
                layout: &self.stroke_pipelines[&format].get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform_slice(&params, offset) },
                    gpu::tex_entry(1, &before_view),
                    gpu::tex_entry(2, &self.sets[job.set].stroke_mask),
                ],
            });
            targets.push(StrokeTarget { after: gpu::view(&job.after), format, bind_group });
        }
        self.stroke = Some(Stroke { brush, targets, channels, on_mask, _params: params });
        Ok(())
    }

    /// Queues dab centers (physical viewport pixels) for the next `update`.
    pub fn add_dabs(&mut self, centers: &[[f32; 2]]) {
        let Some(stroke) = &self.stroke else { return };
        let room = MAX_DABS.saturating_sub(self.pending.len());
        let (r, f) = (stroke.brush.radius.max(0.5), stroke.brush.flow);
        self.pending.extend(centers.iter().take(room).map(|c| [c[0], c[1], r, f]));
    }

    pub fn end_stroke(&mut self) {
        self.stroke = None;
    }

    /// Applies pending dabs and recomposites dirty channels. `scene` is the camera depth,
    /// camera and pixel size the queued dabs refer to.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        scene: Option<(&wgpu::TextureView, &OrbitCamera, [u32; 2])>,
    ) {
        match scene {
            Some((depth, camera, px)) => self.flush_dabs(device, queue, encoder, vertices, indices, depth, camera, px),
            None => self.pending.clear(),
        }
        self.composite(device, queue, encoder);
        self.update_thumbs(device, queue, encoder);
    }

    #[allow(clippy::too_many_arguments)]
    fn flush_dabs(
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
        let Some(stroke) = &self.stroke else {
            self.pending.clear();
            return;
        };
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
            brush: [stroke.brush.hardness.clamp(0.0, 0.99), self.pending.len() as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.dab_params, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&self.dab_buffer, 0, bytemuck::cast_slice(&self.pending));
        self.pending.clear();

        let dab_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dab"),
            layout: &self.dab_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.dab_params.as_entire_binding() },
                gpu::tex_entry(1, scene_depth),
                wgpu::BindGroupEntry { binding: 2, resource: self.dab_buffer.as_entire_binding() },
            ],
        });
        for set in &self.sets {
            let mut pass = gpu::pass(encoder, &set.stroke_mask, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.dab_pipeline);
            pass.set_bind_group(0, &dab_bg, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(set.index_range(), 0, 0..1);
        }
        for t in &stroke.targets {
            let mut pass = gpu::pass(encoder, &t.after, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.stroke_pipelines[&t.format]);
            pass.set_bind_group(0, &t.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        if stroke.on_mask {
            let layer = &self.layers[self.active].meta;
            for c in Channel::ALL {
                self.dirty[c.index()] |= layer.channels.has(c);
            }
        } else {
            for c in Channel::ALL {
                self.dirty[c.index()] |= stroke.channels.has(c);
            }
        }
    }

    /// Blends the layer stack for every dirty channel into the display textures.
    fn composite(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        if !self.dirty.iter().any(|&d| d) || self.sets.is_empty() {
            return;
        }
        // Plan all draws first so their parameters go into one dynamic uniform buffer.
        // Each draw: (source texture, mask texture, parameter index).
        type Draw<'a> = (Option<&'a wgpu::Texture>, Option<&'a wgpu::Texture>, usize);
        let mut params: Vec<Vec4Pair> = Vec::new();
        let mut plan: Vec<(usize, Channel, Vec<Draw>)> = Vec::new();
        for si in 0..self.sets.len() {
            for c in Channel::ALL.into_iter().filter(|c| self.dirty[c.index()]) {
                let mut draws = Vec::new();
                for layer in &self.layers {
                    let m = &layer.meta;
                    if !m.visible || !m.channels.has(c) || m.opacity <= 0.0 {
                        continue;
                    }
                    let tex = &layer.sets[si];
                    let (src, kind, fill) = match &m.kind {
                        LayerKind::Paint => match &tex.channels[c.index()] {
                            Some(t) => (Some(t), if c.is_color() { 0.0 } else { 1.0 }, [0.0; 3]),
                            None => continue,
                        },
                        LayerKind::Fill(v) => (None, 2.0, v.get(c)),
                    };
                    let mask = if m.has_mask && m.mask_enabled { tex.mask.as_ref() } else { None };
                    let blend = if c.is_color() { m.blend.gpu_id() } else { 0 };
                    draws.push((src, mask, params.len()));
                    params.push(Vec4Pair {
                        a: [fill[0], fill[1], fill[2], 1.0],
                        b: [m.opacity, blend as f32, kind, if mask.is_some() { 1.0 } else { 0.0 }],
                    });
                }
                plan.push((si, c, draws));
            }
        }
        if params.len() as u64 > self.blend_capacity {
            self.blend_capacity = (params.len() as u64).next_power_of_two();
            self.blend_params = Self::make_blend_buffer(device, self.blend_capacity);
        }
        if !params.is_empty() {
            let mut bytes = vec![0u8; params.len() * UNIFORM_STRIDE as usize];
            for (i, p) in params.iter().enumerate() {
                let at = i * UNIFORM_STRIDE as usize;
                bytes[at..at + std::mem::size_of::<Vec4Pair>()].copy_from_slice(bytemuck::bytes_of(p));
            }
            queue.write_buffer(&self.blend_params, 0, &bytes);
        }

        let dummy = gpu::view(&self.dummy);
        for (si, c, draws) in plan {
            let set = &self.sets[si];
            let acc = [gpu::view(&set.acc[0]), gpu::view(&set.acc[1])];
            gpu::pass(encoder, &acc[0], wgpu::LoadOp::Clear(rgb_color(self.defaults.get(c))));
            let mut cur = 0;
            for (src, mask, pi) in draws {
                let src = src.map(gpu::view);
                let mask = mask.map(gpu::view);
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("blend"),
                    layout: &self.blend_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniform_slice(&self.blend_params, 0) },
                        gpu::tex_entry(1, &acc[cur]),
                        gpu::tex_entry(2, src.as_ref().unwrap_or(&dummy)),
                        gpu::tex_entry(3, mask.as_ref().unwrap_or(&dummy)),
                    ],
                });
                let mut pass = gpu::pass(encoder, &acc[1 - cur], wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                pass.set_pipeline(&self.blend_pipeline);
                pass.set_bind_group(0, &bg, &[(pi as u64 * UNIFORM_STRIDE) as u32]);
                pass.draw(0..3, 0..1);
                cur = 1 - cur;
            }
            let pipeline = if c.is_color() { &self.dilate_color } else { &self.dilate_scalar };
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dilate"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[gpu::tex_entry(0, &acc[cur]), gpu::tex_entry(1, &set.coverage_view)],
            });
            let out = gpu::view(&set.display[c.index()]);
            let mut pass = gpu::pass(encoder, &out, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.dirty = [false; Channel::COUNT];
    }

    // ---------------------------------------------------------------- thumbnails

    /// Thumbnail of a layer's base color (or mask) in texture set `thumb_set`.
    pub fn thumb(&self, layer_id: u64, mask: bool) -> Option<&Thumb> {
        self.thumbs.get(&(layer_id, mask))
    }

    fn update_thumbs(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        if self.sets.is_empty() || self.stroke.is_some() {
            // Thumbnails refresh when the stroke ends.
            return;
        }
        let si = self.thumb_set.min(self.sets.len() - 1);
        let ids: Vec<u64> = self.layers.iter().map(|l| l.meta.id).collect();
        self.thumbs.retain(|k, _| ids.contains(&k.0));
        for layer in &self.layers {
            let tex = &layer.sets[si];
            let sources = [
                (false, tex.channels[Channel::BaseColor.index()].clone(), 0.0),
                (true, if layer.meta.has_mask { tex.mask.clone() } else { None }, 2.0),
            ];
            for (is_mask, source, mode) in sources {
                let key = (layer.meta.id, is_mask);
                let Some(source) = source else {
                    self.thumbs.remove(&key);
                    continue;
                };
                if self.thumbs.get(&key).is_some_and(|t| t.source == source) {
                    continue;
                }
                let texture = match self.thumbs.remove(&key) {
                    Some(t) => t.texture,
                    None => gpu::texture(
                        device,
                        "thumb",
                        THUMB_SIZE,
                        THUMB_FORMAT,
                        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    ),
                };
                let taps = (source.width() / THUMB_SIZE).clamp(1, 8) as f32;
                let params = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("thumb params"),
                    size: std::mem::size_of::<Vec4Pair>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&params, 0, bytemuck::bytes_of(&Vec4Pair { a: [mode, taps, 0.0, 0.0], b: [0.0; 4] }));
                let src_view = gpu::view(&source);
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("thumb"),
                    layout: &self.thumb_pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                        gpu::tex_entry(1, &src_view),
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                });
                let out = gpu::view(&texture);
                {
                    let mut pass = gpu::pass(encoder, &out, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
                    pass.set_pipeline(&self.thumb_pipeline);
                    pass.set_bind_group(0, &bg, &[]);
                    pass.draw(0..3, 0..1);
                }
                self.thumb_revision += 1;
                self.thumbs.insert(key, Thumb { texture, revision: self.thumb_revision, source });
            }
        }
    }

    /// Forces thumbnails to be rebuilt (e.g. after changing `thumb_set`).
    pub fn invalidate_thumbs(&mut self) {
        self.thumbs.clear();
    }

    // ---------------------------------------------------------------- readback

    /// Samples all final channels of a texture set at `uv` (eyedropper).
    pub fn sample(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, uv: [f32; 2]) -> Option<ChannelValues> {
        let set = self.sets.get(set)?;
        let x = ((uv[0].rem_euclid(1.0) * set.size as f32) as u32).min(set.size - 1);
        let y = ((uv[1].rem_euclid(1.0) * set.size as f32) as u32).min(set.size - 1);
        let read = |c: Channel| gpu::read_texel(device, queue, &set.display[c.index()], x, y);
        let to3 = |p: [u8; 4]| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0];
        Some(ChannelValues {
            base_color: to3(read(Channel::BaseColor)?),
            roughness: read(Channel::Roughness)?[0] as f32 / 255.0,
            metallic: read(Channel::Metallic)?[0] as f32 / 255.0,
            height: read(Channel::Height)?[0] as f32 / 255.0,
            emissive: to3(read(Channel::Emissive)?),
        })
    }

    /// Reads a final channel map of a texture set: RGB8 for colors, L8 for scalars.
    pub fn read_channel(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, c: Channel) -> Option<(u32, Vec<u8>)> {
        let set = self.sets.get(set)?;
        let data = gpu::read_texture(device, queue, &set.display[c.index()])?;
        let data = if c.is_color() { data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect() } else { data };
        Some((set.size, data))
    }
}
