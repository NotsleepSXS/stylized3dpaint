//! The layer stack on the GPU: document tree, paint textures, brush strokes, compositing with
//! effect stacks (content, mask, effect layers), anchors, caches, undo and thumbnails.
//!
//! Compositing (per texture set, per enabled channel, linear 16-bit float, straight alpha):
//! the root accumulator starts opaque from the base material; every node is blended over it
//! bottom to top. A node's content is its paint texture / fill value / folder result followed
//! by its content effects; its mask is a stack of paint, fill and effect items. Effect layers
//! filter the accumulator so far. Expensive results (effect chains, masks, effect layers) are
//! cached by a hash of everything they depend on, so editing one layer only recomputes it and
//! the cheap blends above it.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use egui_wgpu::wgpu;
use glam::Mat4;
use s3dp_core::effect_def::{EffectDef, ParamKind};
use s3dp_core::{
    BakedMap, BlendMode, ChannelId, ChannelSet, ChannelValues, Document, EffectInstance, Mask, MaskItem, MaskItemKind, Mesh, Node,
    NodeKind, OrbitCamera, ParamValue, Vertex, registry,
};

pub use crate::engine::effects::MeshScale;
use crate::engine::effects::{CONTENT_FORMAT, EffectEngine, EffectInputs, MASK_FORMAT, PassTarget};
use crate::gpu::{self, LAYER_USAGE};

/// Painted content channels: linear, premultiplied (rgb * a, a); scalars replicate in rgb.
pub const PAINT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Painted mask items: premultiplied (value * a, a).
pub const MASK_PAINT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
const STROKE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
pub const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const THUMB_SIZE: u32 = 64;
const THUMB_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const MAX_DABS: usize = 1024;
const UNDO_LIMIT: usize = 120;
const POOL_LIMIT: usize = 24;

/// Format of a channel's display texture (sampled by the viewport).
pub fn display_format(c: ChannelId) -> wgpu::TextureFormat {
    match c {
        ChannelId::BaseColor | ChannelId::Emissive => wgpu::TextureFormat::Rgba8UnormSrgb,
        ChannelId::Normal => wgpu::TextureFormat::Rgba8Unorm,
        ChannelId::Height => wgpu::TextureFormat::R16Float,
        _ => wgpu::TextureFormat::R8Unorm,
    }
}

/// Channels the viewport binds directly (in binding order).
pub const VIEW_CHANNELS: [ChannelId; 7] = [
    ChannelId::BaseColor,
    ChannelId::Roughness,
    ChannelId::Metallic,
    ChannelId::Normal,
    ChannelId::Height,
    ChannelId::AmbientOcclusion,
    ChannelId::Emissive,
];

/// Brush settings for one stroke.
#[derive(Clone, Debug)]
pub struct Brush {
    /// Linear values painted into each channel.
    pub values: ChannelValues,
    pub channels: ChannelSet,
    /// Radius in physical viewport pixels.
    pub radius: f32,
    pub hardness: f32,
    pub opacity: f32,
    pub flow: f32,
    pub erase: bool,
    /// Pen pressure scales the dab radius (down to `min_size` of it).
    pub pressure_size: bool,
    /// Pen pressure scales the dab flow.
    pub pressure_flow: bool,
    /// Smallest radius share at zero pressure (0..1).
    pub min_size: f32,
}

impl Default for Brush {
    fn default() -> Self {
        let mut values = ChannelValues::defaults();
        values.set(ChannelId::BaseColor, [0.7, 0.1, 0.05]);
        Self { values, channels: ChannelSet::only(ChannelId::BaseColor), radius: 30.0, hardness: 0.6, opacity: 1.0, flow: 1.0, erase: false, pressure_size: true, pressure_flow: false, min_size: 0.1 }
    }
}

/// What the brush paints on the active node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditTarget {
    #[default]
    Content,
    /// A paint item of the node's mask.
    MaskItem(u64),
}

/// What the viewport's "inspect" slot shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Inspect {
    #[default]
    None,
    Channel(ChannelId),
    Baked(BakedMap),
    /// Mask of a node.
    Mask(u64),
}

pub type ChannelTextures = Vec<Option<wgpu::Texture>>;

#[derive(Clone)]
struct Snapshot {
    doc: Document,
    paint: HashMap<u64, Vec<ChannelTextures>>,
    mask_paint: HashMap<u64, Vec<wgpu::Texture>>,
    active: Option<u64>,
    target: EditTarget,
}

/// Geometry and output textures of one material slot.
pub struct TextureSet {
    pub name: String,
    pub size: u32,
    first_index: u32,
    index_count: u32,
    stroke_tex: wgpu::Texture,
    stroke_mask: wgpu::TextureView,
    /// Final linear result per channel (rgba16f, straight). Export reads these.
    pub finals: HashMap<ChannelId, wgpu::Texture>,
    /// Dilated display texture per channel.
    pub display: HashMap<ChannelId, wgpu::Texture>,
    /// Baked mesh maps, indexed by [`BakedMap::index`].
    pub baked: Vec<wgpu::Texture>,
    /// World position (w = coverage) and normal per texel.
    pub geo_pos: wgpu::Texture,
    pub geo_nrm: wgpu::Texture,
    /// Group 1 of the viewport shader.
    pub(crate) view_bg: Option<wgpu::BindGroup>,
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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vec4x3 {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

struct StrokeTarget {
    after: wgpu::TextureView,
    format: wgpu::TextureFormat,
    bind_group: wgpu::BindGroup,
}

struct Stroke {
    brush: Brush,
    targets: Vec<StrokeTarget>,
    /// Node or mask item whose version is bumped while painting.
    owner: u64,
    channels: ChannelSet,
    _params: wgpu::Buffer,
}

/// A thumbnail texture plus a revision counter so the UI knows when to re-register it.
pub struct Thumb {
    pub texture: wgpu::Texture,
    pub revision: u64,
    source: wgpu::Texture,
}

/// The content on top of a blend: a texture or a constant.
#[derive(Clone)]
enum Top {
    Straight(wgpu::Texture),
    Premultiplied(wgpu::Texture),
    Constant([f32; 3]),
}

fn hash_of(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn hash_debug(v: &impl std::fmt::Debug) -> u64 {
    hash_of(format!("{v:?}"))
}

fn rgb_color(v: [f32; 3], a: f32) -> wgpu::Color {
    wgpu::Color { r: v[0] as f64, g: v[1] as f64, b: v[2] as f64, a: a as f64 }
}

fn uv_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}

fn shader(device: &wgpu::Device, label: &str, src: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) })
}

/// Which compose entry point a pass uses (decides the bindings its auto layout has).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ComposeKind {
    Layer,
    MaskItem,
    Prep,
}

pub struct Painter {
    dab_pipeline: wgpu::RenderPipeline,
    stroke_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    layer_pipeline: wgpu::RenderPipeline,
    mask_item_pipeline: wgpu::RenderPipeline,
    prep_pipeline: wgpu::RenderPipeline,
    display_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    thumb_pipeline: wgpu::RenderPipeline,
    dab_params: wgpu::Buffer,
    dab_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    dummy: wgpu::Texture,
    engine: EffectEngine,
    view_layout: Option<wgpu::BindGroupLayout>,

    pub sets: Vec<TextureSet>,
    pub doc: Document,
    /// Painted channels per node, per texture set.
    paint: HashMap<u64, Vec<ChannelTextures>>,
    /// Painted mask items, per texture set.
    mask_paint: HashMap<u64, Vec<wgpu::Texture>>,
    pub active: Option<u64>,
    pub target: EditTarget,
    /// Effect shown in the properties panel.
    pub selected_effect: Option<u64>,
    pub inspect: Inspect,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    stroke: Option<Stroke>,
    pending: Vec<[f32; 4]>,
    /// Bumped while a node / mask item is painted (its textures change in place).
    versions: HashMap<u64, u64>,

    content_cache: HashMap<(u64, usize, ChannelId), (u64, wgpu::Texture)>,
    mask_cache: HashMap<(u64, usize), (u64, wgpu::Texture)>,
    layer_cache: HashMap<(u64, usize, ChannelId), (u64, wgpu::Texture)>,
    folder_thumbs: HashMap<u64, wgpu::Texture>,
    /// Anchored effect outputs: (effect id, set, channel or None for masks).
    anchors: HashMap<(u64, usize, Option<ChannelId>), wgpu::Texture>,
    anchor_hashes: HashMap<u64, u64>,
    /// Effects that failed to compile / run (shown red in the UI).
    pub errors: HashMap<u64, String>,
    composed: HashMap<(usize, ChannelId), u64>,
    force: ChannelSet,
    pool: Vec<wgpu::Texture>,

    thumbs: HashMap<(u64, bool), Thumb>,
    thumb_revision: u64,
    pub thumb_set: usize,
    revision: u64,
    baked_revision: u64,
    pub scale: MeshScale,
    /// Throttle heavy recomposites to ~10/s while editing.
    pub throttle: bool,
    last_heavy: Option<std::time::Instant>,
    last_cost: std::time::Duration,
    changed: bool,
}

impl Painter {
    pub fn new(device: &wgpu::Device) -> Self {
        let uv = shader(device, "paint_uv", include_str!("paint_uv.wgsl"));
        let composite = shader(device, "composite", include_str!("composite.wgsl"));
        let compose = shader(device, "compose", include_str!("engine/compose.wgsl"));
        let display = shader(device, "display", include_str!("engine/display.wgsl"));
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
        let accumulate = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let full = |label, module: &wgpu::ShaderModule, fs, format| gpu::fullscreen_pipeline(device, label, module, fs, format);
        Self {
            dab_pipeline: uv_pipeline("dab", "fs_dab", STROKE_FORMAT, Some(wgpu::BlendState { color: accumulate, alpha: accumulate })),
            stroke_pipelines: [PAINT_FORMAT, MASK_PAINT_FORMAT]
                .into_iter()
                .map(|f| (f, full("stroke composite", &composite, "fs_composite", f)))
                .collect(),
            layer_pipeline: full("layer", &compose, "fs_layer", CONTENT_FORMAT),
            mask_item_pipeline: full("mask item", &compose, "fs_mask_item", MASK_FORMAT),
            prep_pipeline: full("content prep", &compose, "fs_prep", CONTENT_FORMAT),
            display_pipelines: [
                wgpu::TextureFormat::Rgba8UnormSrgb,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::R16Float,
                wgpu::TextureFormat::R8Unorm,
            ]
            .into_iter()
            .map(|f| (f, full("display", &display, "fs_display", f)))
            .collect(),
            thumb_pipeline: full("thumb", &thumb, "fs_thumb", THUMB_FORMAT),
            dab_params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("dab params"),
                size: std::mem::size_of::<DabParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            dab_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("dabs"),
                size: (MAX_DABS * 16) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("paint"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                ..Default::default()
            }),
            dummy: gpu::texture(device, "dummy", 1, CONTENT_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING),
            engine: EffectEngine::new(device),
            view_layout: None,
            sets: Vec::new(),
            doc: Document::default(),
            paint: HashMap::new(),
            mask_paint: HashMap::new(),
            active: None,
            target: EditTarget::Content,
            selected_effect: None,
            inspect: Inspect::None,
            undo: Vec::new(),
            redo: Vec::new(),
            stroke: None,
            pending: Vec::new(),
            versions: HashMap::new(),
            content_cache: HashMap::new(),
            mask_cache: HashMap::new(),
            layer_cache: HashMap::new(),
            folder_thumbs: HashMap::new(),
            anchors: HashMap::new(),
            anchor_hashes: HashMap::new(),
            errors: HashMap::new(),
            composed: HashMap::new(),
            force: ChannelSet::NONE,
            pool: Vec::new(),
            thumbs: HashMap::new(),
            thumb_revision: 0,
            thumb_set: 0,
            revision: 0,
            baked_revision: 0,
            scale: MeshScale::default(),
            throttle: true,
            last_heavy: None,
            last_cost: std::time::Duration::ZERO,
            changed: true,
        }
    }

    // ============================================================== mesh / document

    /// Creates texture sets for the mesh parts and resets the document.
    pub fn set_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh, view_layout: &wgpu::BindGroupLayout, size: u32) {
        use wgpu::TextureUsages as U;
        self.view_layout = Some(view_layout.clone());
        self.sets.clear();
        self.stroke = None;
        self.pending.clear();
        self.pool.clear();
        let mut encoder = device.create_command_encoder(&Default::default());
        for part in &mesh.parts {
            let stroke_tex = gpu::texture(device, "stroke mask", size, STROKE_FORMAT, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
            let baked: Vec<wgpu::Texture> = BakedMap::ALL
                .iter()
                .map(|&m| {
                    let t = gpu::texture(device, m.label(), size, crate::bake::baked_format(m), LAYER_USAGE);
                    gpu::clear(&mut encoder, &t, rgb_color(m.default_value(), 1.0));
                    t
                })
                .collect();
            self.sets.push(TextureSet {
                name: part.name.clone(),
                size,
                first_index: part.first_index,
                index_count: part.index_count,
                stroke_mask: gpu::view(&stroke_tex),
                stroke_tex,
                finals: HashMap::new(),
                display: HashMap::new(),
                baked,
                geo_pos: gpu::texture(device, "geometry position", size, wgpu::TextureFormat::Rgba32Float, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING),
                geo_nrm: gpu::texture(device, "geometry normal", size, wgpu::TextureFormat::Rgba16Float, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING),
                view_bg: None,
            });
        }
        queue.submit([encoder.finish()]);
        self.scale = {
            let b = mesh.bounds();
            MeshScale { radius: b.radius(), bbox_min: b.min.into(), bbox_size: (b.max - b.min).max(glam::Vec3::splat(1e-6)).into() }
        };
        self.reset_document();
        self.ensure_outputs(device, queue);
    }

    /// A new document with one empty paint layer.
    pub fn reset_document(&mut self) {
        let mut doc = Document::default();
        let id = doc.alloc_id();
        doc.nodes.push(Node::new(id, "Layer 1", NodeKind::Paint));
        self.load_document(doc, HashMap::new(), HashMap::new());
        self.active = Some(id);
    }

    /// Replaces the document and its painted textures (project loading).
    pub fn load_document(&mut self, doc: Document, paint: HashMap<u64, Vec<ChannelTextures>>, mask_paint: HashMap<u64, Vec<wgpu::Texture>>) {
        self.undo.clear();
        self.redo.clear();
        self.thumbs.clear();
        self.clear_caches();
        self.active = doc.nodes.last().map(|n| n.id);
        self.doc = doc;
        self.paint = paint;
        self.mask_paint = mask_paint;
        self.target = EditTarget::Content;
        self.selected_effect = None;
        self.revision += 1;
        self.mark_all_dirty();
    }

    fn clear_caches(&mut self) {
        self.content_cache.clear();
        self.mask_cache.clear();
        self.layer_cache.clear();
        self.folder_thumbs.clear();
        self.anchors.clear();
        self.anchor_hashes.clear();
        self.composed.clear();
    }

    /// Display textures for every enabled channel; rebuilds the viewport bind groups.
    fn ensure_outputs(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        use wgpu::TextureUsages as U;
        let mut encoder = device.create_command_encoder(&Default::default());
        let channels = self.doc.channels;
        for set in &mut self.sets {
            for c in ChannelId::ALL {
                let wanted = channels.has(c) || VIEW_CHANNELS.contains(&c);
                if wanted && !set.display.contains_key(&c) {
                    let t = gpu::texture(device, c.label(), set.size, display_format(c), U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC);
                    gpu::clear(&mut encoder, &t, rgb_color(self.doc.defaults.get(c), 1.0));
                    set.display.insert(c, t);
                }
            }
        }
        queue.submit([encoder.finish()]);
        self.rebuild_view_groups(device);
    }

    /// Viewport group 1: 7 channels, baked AO, baked normal, inspect, sampler.
    pub fn rebuild_view_groups(&mut self, device: &wgpu::Device) {
        let Some(layout) = self.view_layout.clone() else { return };
        for si in 0..self.sets.len() {
            let inspect = self.inspect_texture(si).unwrap_or_else(|| self.dummy.clone());
            let set = &self.sets[si];
            let mut views: Vec<wgpu::TextureView> = VIEW_CHANNELS.iter().map(|c| gpu::view(&set.display[c])).collect();
            views.push(gpu::view(&set.baked[BakedMap::AmbientOcclusion.index()]));
            views.push(gpu::view(&set.baked[BakedMap::Normal.index()]));
            views.push(gpu::view(&inspect));
            let mut entries: Vec<wgpu::BindGroupEntry> = views.iter().enumerate().map(|(i, v)| gpu::tex_entry(i as u32, v)).collect();
            entries.push(wgpu::BindGroupEntry { binding: views.len() as u32, resource: wgpu::BindingResource::Sampler(&self.sampler) });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("paint view"), layout: &layout, entries: &entries });
            self.sets[si].view_bg = Some(bg);
        }
    }

    fn inspect_texture(&self, si: usize) -> Option<wgpu::Texture> {
        let set = self.sets.get(si)?;
        match self.inspect {
            Inspect::None => None,
            Inspect::Channel(c) => set.display.get(&c).cloned(),
            Inspect::Baked(m) => Some(set.baked[m.index()].clone()),
            Inspect::Mask(id) => self.mask_cache.get(&(id, si)).map(|c| c.1.clone()),
        }
    }

    pub fn mark_all_dirty(&mut self) {
        self.composed.clear();
        self.changed = true;
    }

    /// Baked maps or geometry changed: generators must recompute.
    pub fn baked_changed(&mut self) {
        self.baked_revision += 1;
        self.mark_all_dirty();
    }

    /// Marks the document as changed without an undo step (e.g. after baking).
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    /// Changes whenever the document changes; compare with a saved value to detect edits.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns (and clears) whether anything visible changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// True while a throttled recomposite is pending.
    pub fn wants_repaint(&self) -> bool {
        (0..self.sets.len()).any(|si| self.doc.channels.iter().any(|c| !self.composed.contains_key(&(si, c))))
    }

    // ============================================================== undo

    pub fn checkpoint(&mut self) {
        self.revision += 1;
        self.undo.push(self.snapshot());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { doc: self.doc.clone(), paint: self.paint.clone(), mask_paint: self.mask_paint.clone(), active: self.active, target: self.target }
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
        let current = self.snapshot();
        let (src, dst) = if from_undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(state) = src.pop() else { return };
        dst.push(current);
        self.revision += 1;
        self.doc = state.doc;
        self.paint = state.paint;
        self.mask_paint = state.mask_paint;
        self.active = state.active.filter(|id| self.doc.find(*id).is_some());
        self.target = state.target;
        self.mark_all_dirty();
    }

    pub fn undo(&mut self) {
        self.swap_state(true);
    }

    pub fn redo(&mut self) {
        self.swap_state(false);
    }

    // ============================================================== editing

    pub fn active_node(&self) -> Option<&Node> {
        self.active.and_then(|id| self.doc.find(id))
    }

    /// Edits a node. `checkpoint=false` for continuous edits (slider drags after the first frame).
    pub fn edit_node(&mut self, id: u64, checkpoint: bool, f: impl FnOnce(&mut Node)) {
        let Some(before) = self.doc.find(id).cloned() else { return };
        let mut after = before.clone();
        f(&mut after);
        if after == before {
            return;
        }
        if checkpoint {
            self.checkpoint();
        } else {
            self.revision += 1;
        }
        if let Some(n) = self.doc.find_mut(id) {
            *n = after;
        }
        self.mark_all_dirty();
    }

    /// Edits document-wide settings (base material, channels).
    pub fn edit_document(&mut self, checkpoint: bool, f: impl FnOnce(&mut Document)) {
        let mut doc = self.doc.clone();
        f(&mut doc);
        if doc == self.doc {
            return;
        }
        if checkpoint {
            self.checkpoint();
        } else {
            self.revision += 1;
        }
        self.doc = doc;
        self.mark_all_dirty();
    }

    /// Adds a node: inside the active folder (top) or above the active node. Selects it.
    pub fn add_node(&mut self, kind: NodeKind, setup: impl FnOnce(&mut Node, &mut Document)) -> u64 {
        self.checkpoint();
        let id = self.doc.alloc_id();
        let mut count = 0;
        self.doc.visit(|_| count += 1);
        let name = match &kind {
            NodeKind::Paint => format!("Layer {}", count + 1),
            NodeKind::Fill(_) => format!("Fill {}", count + 1),
            NodeKind::Folder { .. } => format!("Folder {}", count + 1),
            NodeKind::Effect(e) => registry().get(&e.def).map_or_else(|| e.def.clone(), |d| d.name.clone()),
        };
        let mut node = Node::new(id, name, kind);
        setup(&mut node, &mut self.doc);
        let into_folder = self.active_node().filter(|n| n.is_folder()).map(|n| n.id);
        match into_folder {
            Some(folder) => self.doc.find_mut(folder).unwrap().children.push(node),
            None => self.doc.insert_above(self.active, node),
        }
        self.active = Some(id);
        self.target = EditTarget::Content;
        self.mark_all_dirty();
        id
    }

    pub fn delete_node(&mut self, id: u64) {
        if self.doc.find(id).is_none() {
            return;
        }
        self.checkpoint();
        self.doc.remove(id);
        if self.active == Some(id) {
            self.active = self.doc.nodes.last().map(|n| n.id);
            self.target = EditTarget::Content;
        }
        self.mark_all_dirty();
    }

    /// Deep copy with fresh ids; painted textures are shared (they are immutable).
    pub fn duplicate_node(&mut self, id: u64) {
        let Some(node) = self.doc.find(id).cloned() else { return };
        self.checkpoint();
        let mut copy = node;
        self.reassign_ids(&mut copy);
        copy.name = format!("{} copy", copy.name);
        let new_id = copy.id;
        self.doc.insert_above(Some(id), copy);
        self.active = Some(new_id);
        self.mark_all_dirty();
    }

    fn reassign_ids(&mut self, node: &mut Node) {
        let old = node.id;
        node.id = self.doc.alloc_id();
        if let Some(t) = self.paint.get(&old).cloned() {
            self.paint.insert(node.id, t);
        }
        if let NodeKind::Effect(e) = &mut node.kind {
            e.id = self.doc.alloc_id();
        }
        for e in &mut node.effects {
            e.id = self.doc.alloc_id();
        }
        if let Some(mask) = &mut node.mask {
            for item in &mut mask.items {
                let old_item = item.id;
                item.id = self.doc.alloc_id();
                if let Some(t) = self.mask_paint.get(&old_item).cloned() {
                    self.mask_paint.insert(item.id, t);
                }
                if let MaskItemKind::Effect(e) = &mut item.kind {
                    e.id = self.doc.alloc_id();
                }
            }
        }
        let mut children = std::mem::take(&mut node.children);
        for c in &mut children {
            self.reassign_ids(c);
        }
        node.children = children;
    }

    /// Moves a node into `parent` (None = root) at `index` (bottom = 0).
    pub fn move_node(&mut self, id: u64, parent: Option<u64>, index: usize) {
        if let Some(p) = parent {
            let mut inside = false;
            if let Some(n) = self.doc.find(id) {
                n.visit(&mut |c| inside |= c.id == p);
            }
            if inside || self.doc.find(p).is_none_or(|n| !n.is_folder()) {
                return;
            }
        }
        self.checkpoint();
        let Some(node) = self.doc.remove(id) else { return };
        let list = match parent.and_then(|p| self.doc.find_mut(p)) {
            Some(p) => &mut p.children,
            None => &mut self.doc.nodes,
        };
        let index = index.min(list.len());
        list.insert(index, node);
        self.mark_all_dirty();
    }

    /// Groups the given nodes into a new folder (Ctrl+G). They may live in different folders:
    /// the folder takes the place of the shallowest, topmost one, and children keep their
    /// document order. Nodes inside another selected node move with it.
    pub fn group(&mut self, ids: &[u64]) -> Option<u64> {
        let order = self.selection_roots(ids);
        let doc = &self.doc;
        let anchor = *order.iter().rev().min_by_key(|id| doc.path_of(**id).map_or(usize::MAX, |p| p.len()))?;
        self.checkpoint();
        let folder_id = self.doc.alloc_id();
        let mut count = 0;
        self.doc.visit(|_| count += 1);
        let mut folder = Node::new(folder_id, format!("Folder {}", count + 1), NodeKind::Folder { expanded: true });
        let mut taken = Vec::new();
        for &id in &order {
            if id != anchor {
                taken.push((id, self.doc.remove(id)?));
            }
        }
        let (list, index) = self.doc.parent_list_mut(anchor)?;
        let anchor_node = list.remove(index);
        folder.children = order
            .iter()
            .filter_map(|id| if *id == anchor { Some(anchor_node.clone()) } else { taken.iter().find(|(t, _)| t == id).map(|(_, n)| n.clone()) })
            .collect();
        list.insert(index.min(list.len()), folder);
        self.active = Some(folder_id);
        self.target = EditTarget::Content;
        self.mark_all_dirty();
        Some(folder_id)
    }

    /// Deletes several nodes as one undo step.
    pub fn delete_nodes(&mut self, ids: &[u64]) {
        let roots = self.selection_roots(ids);
        if roots.is_empty() {
            return;
        }
        self.checkpoint();
        for id in &roots {
            self.doc.remove(*id);
        }
        if self.active.is_none_or(|a| self.doc.find(a).is_none()) {
            self.active = self.doc.nodes.last().map(|n| n.id);
            self.target = EditTarget::Content;
        }
        self.mark_all_dirty();
    }

    /// Existing ids in document order, without those inside another listed node.
    fn selection_roots(&self, ids: &[u64]) -> Vec<u64> {
        let mut order = Vec::new();
        self.doc.visit(|n| {
            if ids.contains(&n.id) {
                order.push(n.id);
            }
        });
        let inside = |id: u64| {
            order.iter().any(|&o| {
                o != id
                    && self.doc.find(o).is_some_and(|n| {
                        let mut found = false;
                        n.visit(&mut |c| found |= c.id == id && c.id != o);
                        found
                    })
            })
        };
        order.iter().copied().filter(|&id| !inside(id)).collect()
    }

    /// Adds a mask (fill white / black + a paint item) and targets the paint item.
    pub fn add_mask(&mut self, id: u64, white: bool) {
        if self.doc.find(id).is_none_or(|n| n.mask.is_some()) {
            return;
        }
        self.checkpoint();
        let fill_id = self.doc.alloc_id();
        let paint_id = self.doc.alloc_id();
        let v = if white { 1.0 } else { 0.0 };
        let mask = Mask {
            enabled: true,
            items: vec![
                MaskItem { id: fill_id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Fill(v) },
                MaskItem { id: paint_id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Paint },
            ],
        };
        self.doc.find_mut(id).unwrap().mask = Some(mask);
        self.active = Some(id);
        self.target = EditTarget::MaskItem(paint_id);
        self.mark_all_dirty();
    }

    pub fn delete_mask(&mut self, id: u64) {
        if self.doc.find(id).is_none_or(|n| n.mask.is_none()) {
            return;
        }
        self.checkpoint();
        self.doc.find_mut(id).unwrap().mask = None;
        self.target = EditTarget::Content;
        self.mark_all_dirty();
    }

    /// New effect instance with default parameters.
    pub fn new_effect(&mut self, def_id: &str) -> Option<EffectInstance> {
        let def = registry().get(def_id)?;
        let id = self.doc.alloc_id();
        Some(def.instantiate(id))
    }

    pub fn new_id(&mut self) -> u64 {
        self.doc.alloc_id()
    }

    pub fn set_defaults(&mut self, defaults: ChannelValues, checkpoint: bool) {
        self.edit_document(checkpoint, |d| d.defaults = defaults);
    }

    // ============================================================== strokes

    pub fn is_stroke_active(&self) -> bool {
        self.stroke.is_some()
    }

    fn zero_paint(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, size: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
        let t = gpu::texture(device, "paint", size, format, LAYER_USAGE);
        gpu::clear(encoder, &t, wgpu::Color::TRANSPARENT);
        t
    }

    /// Starts a stroke on the active node. Returns a user-facing reason if it can't.
    pub fn begin_stroke(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, brush: Brush) -> Result<(), &'static str> {
        let Some(node) = self.active_node().cloned() else { return Err("No layer selected") };
        if !node.visible {
            return Err("The active layer is hidden");
        }
        let mask_item = match self.target {
            EditTarget::MaskItem(item) => node
                .mask
                .as_ref()
                .and_then(|m| m.items.iter().find(|i| i.id == item))
                .filter(|i| matches!(i.kind, MaskItemKind::Paint))
                .map(|i| i.id),
            EditTarget::Content => None,
        };
        if matches!(self.target, EditTarget::MaskItem(_)) && mask_item.is_none() {
            return Err("Select a paint item of the mask to paint it");
        }
        let channels: ChannelSet = if mask_item.is_some() {
            ChannelSet::ALL
        } else {
            if !matches!(node.kind, NodeKind::Paint) {
                return Err("Only paint layers can be painted — add a mask and paint the mask");
            }
            brush.channels.iter().filter(|c| self.doc.channels.has(*c) && node.blending.channels.has(*c)).collect()
        };
        if channels == ChannelSet::NONE {
            return Err("No channel is enabled for both the brush and the layer");
        }
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
        let set_count = self.sets.len();
        for si in 0..set_count {
            let size = self.sets[si].size;
            gpu::clear(&mut encoder, &self.sets[si].stroke_tex, wgpu::Color::TRANSPARENT);
            if let Some(item) = mask_item {
                let list = self.mask_paint.entry(item).or_default();
                while list.len() < set_count {
                    list.push(Self::zero_paint(device, &mut encoder, size, MASK_PAINT_FORMAT));
                }
                let before = list[si].clone();
                let after = gpu::duplicate(device, &mut encoder, &before);
                list[si] = after.clone();
                // Mask value follows the perceived brightness of the brush color (white reveals).
                let c = brush.values.get(ChannelId::BaseColor);
                let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                let v = if brush.erase { 1.0 } else { s3dp_core::document::linear_to_srgb(luma) };
                jobs.push(Job { set: si, before, after, value: [v; 3], mode: 1.0 });
            } else {
                let list = self.paint.entry(node.id).or_default();
                while list.len() < set_count {
                    list.push(vec![None; ChannelId::COUNT]);
                }
                for c in channels.iter() {
                    let before = match &list[si][c.index()] {
                        Some(t) => t.clone(),
                        None => Self::zero_paint(device, &mut encoder, size, PAINT_FORMAT),
                    };
                    let after = gpu::duplicate(device, &mut encoder, &before);
                    list[si][c.index()] = Some(after.clone());
                    let v = brush.values.get(c);
                    let value = if c.is_color() { v } else { [v[0]; 3] };
                    jobs.push(Job { set: si, before, after, value, mode: if brush.erase { 2.0 } else { 0.0 } });
                }
            }
        }
        queue.submit([encoder.finish()]);

        const STRIDE: u64 = 256;
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stroke params"),
            size: (jobs.len().max(1) as u64) * STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut targets = Vec::new();
        for (i, job) in jobs.into_iter().enumerate() {
            let p = [[job.value[0], job.value[1], job.value[2], 1.0], [brush.opacity, job.mode, 0.0, 0.0]];
            let offset = i as u64 * STRIDE;
            queue.write_buffer(&params, offset, bytemuck::cast_slice(&p));
            let format = job.after.format();
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("stroke composite"),
                layout: &self.stroke_pipelines[&format].get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &params, offset, size: wgpu::BufferSize::new(32) }),
                    },
                    gpu::tex_entry(1, &gpu::view(&job.before)),
                    gpu::tex_entry(2, &self.sets[job.set].stroke_mask),
                ],
            });
            targets.push(StrokeTarget { after: gpu::view(&job.after), format, bind_group });
        }
        self.stroke = Some(Stroke { brush, targets, owner: mask_item.unwrap_or(node.id), channels, _params: params });
        Ok(())
    }

    /// Dabs at full pressure (mouse).
    pub fn add_dabs(&mut self, centers: &[[f32; 2]]) {
        let with_pressure: Vec<[f32; 3]> = centers.iter().map(|c| [c[0], c[1], 1.0]).collect();
        self.add_dabs_pressure(&with_pressure);
    }

    /// Dabs as `[x, y, pressure 0..1]`: pressure scales radius and / or flow per the brush.
    pub fn add_dabs_pressure(&mut self, dabs: &[[f32; 3]]) {
        let Some(stroke) = &self.stroke else { return };
        let room = MAX_DABS.saturating_sub(self.pending.len());
        let b = &stroke.brush;
        self.pending.extend(dabs.iter().take(room).map(|d| {
            let p = if d[2].is_finite() { d[2].clamp(0.0, 1.0) } else { 1.0 };
            let r = if b.pressure_size { b.radius * (b.min_size.clamp(0.0, 1.0) + (1.0 - b.min_size.clamp(0.0, 1.0)) * p) } else { b.radius };
            let f = if b.pressure_flow { b.flow * p } else { b.flow };
            [d[0], d[1], r.max(0.5), f]
        }));
    }

    pub fn end_stroke(&mut self) {
        self.stroke = None;
        self.changed = true;
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
        *self.versions.entry(stroke.owner).or_default() += 1;
        self.force = ChannelSet(self.force.0 | stroke.channels.0);
        self.changed = true;
    }

    // ============================================================== frame update

    /// Applies pending dabs and recomposites what changed. `scene` = camera depth, camera and
    /// pixel size that the queued dabs refer to.
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
        self.compose(device, encoder);
        self.update_thumbs(device, queue, encoder);
    }

    fn acquire(&mut self, device: &wgpu::Device, size: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
        if let Some(i) = self.pool.iter().position(|t| t.width() == size && t.format() == format) {
            return self.pool.swap_remove(i);
        }
        gpu::texture(device, "stack temp", size, format, LAYER_USAGE)
    }

    /// Returns a temporary to the pool unless something else still holds it.
    fn release(&mut self, t: wgpu::Texture) {
        let held = self.content_cache.values().any(|c| c.1 == t)
            || self.mask_cache.values().any(|c| c.1 == t)
            || self.layer_cache.values().any(|c| c.1 == t)
            || self.folder_thumbs.values().any(|f| *f == t)
            || self.sets.iter().any(|s| s.finals.values().any(|f| *f == t));
        if !held && self.pool.len() < POOL_LIMIT {
            self.pool.push(t);
        }
    }

    /// Recomposites every (set, channel) whose inputs changed.
    fn compose(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        if self.sets.is_empty() {
            return;
        }
        let force = std::mem::replace(&mut self.force, ChannelSet::NONE);
        let doc_hash = hash_debug(&self.doc);
        let paint_hash = {
            let mut versions: Vec<(u64, u64)> = self.versions.iter().map(|(k, v)| (*k, *v)).collect();
            versions.sort();
            let mut ids: Vec<u64> = self.paint.keys().chain(self.mask_paint.keys()).copied().collect();
            ids.sort();
            let textures: Vec<u64> = ids
                .iter()
                .flat_map(|id| {
                    let paint = self.paint.get(id).into_iter().flatten().flatten().flatten().map(hash_of);
                    let masks = self.mask_paint.get(id).into_iter().flatten().map(hash_of);
                    paint.chain(masks).collect::<Vec<_>>()
                })
                .collect();
            hash_of((versions, textures))
        };
        let mut todo = Vec::new();
        for si in 0..self.sets.len() {
            for c in self.doc.channels.iter() {
                let key = hash_of((doc_hash, paint_hash, self.baked_revision, si, c));
                if force.has(c) || self.composed.get(&(si, c)) != Some(&key) {
                    todo.push((si, c, key));
                }
            }
        }
        if todo.is_empty() {
            return;
        }
        // Expensive stacks (effects) recompose at most ~10x per second while editing.
        if self.throttle
            && self.stroke.is_none()
            && self.last_cost > std::time::Duration::from_millis(25)
            && self.last_heavy.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(100))
        {
            return;
        }
        let started = std::time::Instant::now();
        self.last_heavy = Some(started);
        let nodes = self.doc.nodes.clone();
        // Masks first: they are channel independent and can be anchors for content.
        for si in 0..self.sets.len() {
            for n in &nodes {
                self.compose_masks(device, encoder, n, si);
            }
        }
        for (si, c, key) in todo {
            let size = self.sets[si].size;
            let base = self.acquire(device, size, CONTENT_FORMAT);
            gpu::clear(encoder, &base, rgb_color(self.doc.defaults.get(c), 1.0));
            let (acc, _) = self.compose_list(device, encoder, &nodes, si, c, base, hash_of((si, c)));
            self.write_display(device, encoder, si, c, &acc);
            if let Some(old) = self.sets[si].finals.insert(c, acc) {
                self.release(old);
            }
            self.composed.insert((si, c), key);
        }
        self.rebuild_view_groups(device);
        self.last_cost = started.elapsed();
        self.changed = true;
    }

    fn compose_masks(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, node: &Node, si: usize) {
        for c in &node.children {
            self.compose_masks(device, encoder, c, si);
        }
        let _ = self.node_mask(device, encoder, node, si);
    }

    /// Composites `nodes` over `acc`. Returns the new accumulator and a hash of its state.
    #[allow(clippy::too_many_arguments)]
    fn compose_list(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        nodes: &[Node],
        si: usize,
        c: ChannelId,
        mut acc: wgpu::Texture,
        mut h: u64,
    ) -> (wgpu::Texture, u64) {
        let target = PassTarget::Content { channel: c.index() as u32, color: c.is_color() };
        for node in nodes {
            if !node.visible || !node.blending.affects(c) {
                continue;
            }
            let mask = self.node_mask(device, encoder, node, si);
            let mask_tex = mask.as_ref().map(|m| m.0.clone());
            let mask_hash = mask.as_ref().map(|m| m.1);
            let blend = node.blending.get(c);
            match &node.kind {
                NodeKind::Effect(e) => {
                    if !e.enabled || !e.channels.has(c) {
                        continue;
                    }
                    let key = hash_of((h, hash_debug(e), self.effect_inputs_hash(e), self.baked_revision));
                    let out = match self.layer_cache.get(&(node.id, si, c)) {
                        Some((k, t)) if *k == key => t.clone(),
                        _ => {
                            let src = self.copy_of(device, encoder, &acc);
                            let out = self.run_chain(device, encoder, std::slice::from_ref(e), src, si, target, Some(c));
                            self.layer_cache.insert((node.id, si, c), (key, out.clone()));
                            out
                        }
                    };
                    acc = self.layer_pass(device, encoder, acc, &Top::Straight(out), mask_tex.as_ref(), BlendMode::Replace, blend.opacity, c);
                    h = hash_of((h, key, mask_hash, blend.opacity.to_bits()));
                }
                NodeKind::Folder { .. } if blend.mode == BlendMode::Passthrough => {
                    let start = self.copy_of(device, encoder, &acc);
                    let (after, fh) = self.compose_list(device, encoder, &node.children, si, c, start, h);
                    let after = self.apply_content_effects(device, encoder, node, si, c, after);
                    acc = self.layer_pass(device, encoder, acc, &Top::Straight(after.clone()), mask_tex.as_ref(), BlendMode::Replace, blend.opacity, c);
                    self.release(after);
                    h = hash_of((fh, hash_debug(&node.effects), mask_hash, blend.opacity.to_bits()));
                }
                _ => {
                    let Some((top, th)) = self.node_content(device, encoder, node, si, c) else { continue };
                    acc = self.layer_pass(device, encoder, acc, &top, mask_tex.as_ref(), blend.mode, blend.opacity, c);
                    if let Top::Straight(t) = top {
                        self.release(t);
                    }
                    h = hash_of((h, th, mask_hash, blend.mode, blend.opacity.to_bits()));
                }
            }
        }
        (acc, h)
    }

    fn copy_of(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, t: &wgpu::Texture) -> wgpu::Texture {
        let c = self.acquire(device, t.width(), t.format());
        gpu::copy_full(encoder, t, &c);
        c
    }

    /// Content of a paint / fill / folder node after its content effects.
    fn node_content(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, node: &Node, si: usize, c: ChannelId) -> Option<(Top, u64)> {
        let size = self.sets[si].size;
        let (base, h) = match &node.kind {
            NodeKind::Paint => {
                let t = self.paint.get(&node.id)?.get(si)?.get(c.index())?.clone()?;
                let v = self.versions.get(&node.id).copied().unwrap_or(0);
                let h = hash_of((hash_of(&t), v));
                (Top::Premultiplied(t), h)
            }
            NodeKind::Fill(values) => {
                let v = values.get(c);
                (Top::Constant(v), hash_of(v.map(f32::to_bits)))
            }
            NodeKind::Folder { .. } => {
                let start = self.acquire(device, size, CONTENT_FORMAT);
                gpu::clear(encoder, &start, wgpu::Color::TRANSPARENT);
                let (acc, h) = self.compose_list(device, encoder, &node.children, si, c, start, hash_of((node.id, si, c)));
                if c == ChannelId::BaseColor && si == self.thumb_set {
                    let copy = gpu::duplicate(device, encoder, &acc);
                    self.folder_thumbs.insert(node.id, copy);
                }
                (Top::Straight(acc), h)
            }
            NodeKind::Effect(_) => return None,
        };
        if !node.effects.iter().any(|e| e.enabled && e.channels.has(c)) {
            return Some((base, h));
        }
        let inputs: Vec<u64> = node.effects.iter().map(|e| self.effect_inputs_hash(e)).collect();
        let key = hash_of((h, hash_debug(&node.effects), inputs, self.baked_revision));
        if let Some((k, t)) = self.content_cache.get(&(node.id, si, c))
            && *k == key
        {
            let t = t.clone();
            if let Top::Straight(b) = base {
                self.release(b);
            }
            return Some((Top::Premultiplied(t).into_cached(), key));
        }
        let src = self.prep(device, encoder, &base, size);
        if let Top::Straight(b) = base {
            self.release(b);
        }
        let out = self.apply_content_effects(device, encoder, node, si, c, src);
        self.content_cache.insert((node.id, si, c), (key, out.clone()));
        Some((Top::Straight(out).into_cached(), key))
    }

    fn apply_content_effects(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, node: &Node, si: usize, c: ChannelId, src: wgpu::Texture) -> wgpu::Texture {
        let effects: Vec<EffectInstance> = node.effects.iter().filter(|e| e.enabled && e.channels.has(c)).cloned().collect();
        if effects.is_empty() {
            return src;
        }
        self.run_chain(device, encoder, &effects, src, si, PassTarget::Content { channel: c.index() as u32, color: c.is_color() }, Some(c))
    }

    /// Straight rgba buffer from a content source.
    fn prep(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, top: &Top, size: u32) -> wgpu::Texture {
        let out = self.acquire(device, size, CONTENT_FORMAT);
        let (kind, value, tex) = top.parts();
        let params = Vec4x3 { a: [value[0], value[1], value[2], 1.0], b: [0.0, 1.0, kind, 0.0], c: [0.0; 4] };
        self.compose_pass(device, encoder, ComposeKind::Prep, &params, None, tex, None, &out);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        kind: ComposeKind,
        params: &Vec4x3,
        acc: Option<&wgpu::Texture>,
        top: Option<&wgpu::Texture>,
        mask: Option<&wgpu::Texture>,
        out: &wgpu::Texture,
    ) {
        use wgpu::util::DeviceExt;
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compose params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let pipeline = match kind {
            ComposeKind::Layer => &self.layer_pipeline,
            ComposeKind::MaskItem => &self.mask_item_pipeline,
            ComposeKind::Prep => &self.prep_pipeline,
        };
        let dummy = gpu::view(&self.dummy);
        let view = |t: Option<&wgpu::Texture>| t.map(gpu::view).unwrap_or_else(|| dummy.clone());
        let views = [view(acc), view(top), view(mask)];
        // Auto layouts contain only the bindings each entry point uses.
        let used: &[u32] = match kind {
            ComposeKind::Prep => &[2],
            ComposeKind::MaskItem => &[1, 2],
            ComposeKind::Layer => &[1, 2, 3],
        };
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }];
        for &b in used {
            entries.push(gpu::tex_entry(b, &views[b as usize - 1]));
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("compose"), layout: &pipeline.get_bind_group_layout(0), entries: &entries });
        let out_view = gpu::view(out);
        let mut pass = gpu::pass(encoder, &out_view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Blends `top` over `acc` into a new accumulator (the old one goes back to the pool).
    #[allow(clippy::too_many_arguments)]
    fn layer_pass(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        acc: wgpu::Texture,
        top: &Top,
        mask: Option<&wgpu::Texture>,
        mode: BlendMode,
        opacity: f32,
        c: ChannelId,
    ) -> wgpu::Texture {
        let out = self.acquire(device, acc.width(), CONTENT_FORMAT);
        let (kind, value, tex) = top.parts();
        let params = Vec4x3 {
            a: [value[0], value[1], value[2], 1.0],
            b: [mode.gpu_id() as f32, opacity, kind, mask.is_some() as u32 as f32],
            c: [c.is_color() as u32 as f32, 0.0, 0.0, 0.0],
        };
        self.compose_pass(device, encoder, ComposeKind::Layer, &params, Some(&acc), tex, mask, &out);
        self.release(acc);
        out
    }

    /// Computes (or reuses) a node's mask. Returns the texture and its hash.
    fn node_mask(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, node: &Node, si: usize) -> Option<(wgpu::Texture, u64)> {
        let mask = node.mask.as_ref().filter(|m| m.enabled)?;
        let paint: Vec<(u64, u64)> = mask
            .items
            .iter()
            .filter_map(|i| self.mask_paint.get(&i.id).and_then(|l| l.get(si)).map(|t| (hash_of(t), self.versions.get(&i.id).copied().unwrap_or(0))))
            .collect();
        let anchors: Vec<u64> = mask
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                MaskItemKind::Effect(e) => Some(self.effect_inputs_hash(e)),
                _ => None,
            })
            .collect();
        let key = hash_of((hash_debug(mask), paint, anchors, self.baked_revision, si));
        if let Some((k, t)) = self.mask_cache.get(&(node.id, si))
            && *k == key
        {
            return Some((t.clone(), key));
        }
        let size = self.sets[si].size;
        let mut cur = self.acquire(device, size, MASK_FORMAT);
        gpu::clear(encoder, &cur, wgpu::Color::BLACK);
        for item in mask.items.iter().filter(|i| i.enabled) {
            match &item.kind {
                MaskItemKind::Fill(v) => {
                    let out = self.acquire(device, size, MASK_FORMAT);
                    let params = Vec4x3 { a: [*v, *v, *v, 1.0], b: [item.blend.gpu_id() as f32, item.opacity, 0.0, 0.0], c: [0.0; 4] };
                    self.compose_pass(device, encoder, ComposeKind::MaskItem, &params, Some(&cur), None, None, &out);
                    let old = std::mem::replace(&mut cur, out);
                    self.release(old);
                }
                MaskItemKind::Paint => {
                    let Some(tex) = self.mask_paint.get(&item.id).and_then(|l| l.get(si)).cloned() else { continue };
                    let out = self.acquire(device, size, MASK_FORMAT);
                    let params = Vec4x3 { a: [0.0; 4], b: [item.blend.gpu_id() as f32, item.opacity, 1.0, 0.0], c: [0.0; 4] };
                    self.compose_pass(device, encoder, ComposeKind::MaskItem, &params, Some(&cur), Some(&tex), None, &out);
                    let old = std::mem::replace(&mut cur, out);
                    self.release(old);
                }
                MaskItemKind::Effect(e) => {
                    if e.enabled {
                        cur = self.run_chain(device, encoder, std::slice::from_ref(e), cur, si, PassTarget::Mask, None);
                    }
                }
            }
        }
        self.mask_cache.insert((node.id, si), (key, cur.clone()));
        Some((cur, key))
    }

    /// Hash of the anchors an effect reads (so references update when the anchor changes).
    fn effect_inputs_hash(&self, e: &EffectInstance) -> u64 {
        let refs: Vec<u64> = e
            .params
            .values()
            .filter_map(|v| match v {
                ParamValue::Anchor(Some(id)) => Some(self.anchor_hashes.get(id).copied().unwrap_or(*id)),
                _ => None,
            })
            .collect();
        hash_of(refs)
    }

    /// Runs effects in order over `src` (consumed). Returns the result.
    #[allow(clippy::too_many_arguments)]
    fn run_chain(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        effects: &[EffectInstance],
        src: wgpu::Texture,
        si: usize,
        target: PassTarget,
        channel: Option<ChannelId>,
    ) -> wgpu::Texture {
        let mut cur = src;
        let size = cur.width();
        let format = target.format();
        for e in effects {
            let Some(def) = registry().get(&e.def) else {
                self.errors.insert(e.id, format!("unknown effect '{}'", e.def));
                continue;
            };
            let input = if def.neighbors {
                let d = self.acquire(device, size, format);
                self.engine.dilate(device, encoder, &cur, &d, &self.sets[si].geo_pos);
                Some(d)
            } else {
                None
            };
            let anchors = self.resolve_anchors(def, e, si, channel);
            let g_blur = if def.category.is_generator() {
                def.all_params().iter().find(|p| p.id == "g_blur").map_or(0.0, |p| def.value(e, p).as_f32())
            } else {
                0.0
            };
            let dst = self.acquire(device, size, format);
            let result = if g_blur > 0.0 {
                self.run_generator_blurred(device, encoder, def, e, target, input.as_ref().unwrap_or(&cur), &dst, si, &anchors, g_blur)
            } else {
                let set = &self.sets[si];
                let inputs = EffectInputs { geo_pos: &set.geo_pos, geo_nrm: &set.geo_nrm, baked: &set.baked, scale: self.scale };
                self.engine.run(device, encoder, def, e, target, input.as_ref().unwrap_or(&cur), &dst, &inputs, [anchors[0].as_ref(), anchors[1].as_ref()])
            };
            if let Some(d) = input {
                self.release(d);
            }
            match result {
                Ok(()) => {
                    self.errors.remove(&e.id);
                    let old = std::mem::replace(&mut cur, dst);
                    self.release(old);
                    if e.anchor {
                        let copy = gpu::duplicate(device, encoder, &cur);
                        self.anchors.insert((e.id, si, channel), copy);
                        self.anchor_hashes.insert(e.id, hash_of((hash_debug(e), self.versions.values().sum::<u64>(), self.baked_revision)));
                    }
                }
                Err(err) => {
                    self.errors.insert(e.id, err);
                    self.release(dst);
                }
            }
        }
        cur
    }

    /// Generator with Global Blur: raw value -> blur -> blend over the input.
    #[allow(clippy::too_many_arguments)]
    fn run_generator_blurred(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        def: &EffectDef,
        e: &EffectInstance,
        target: PassTarget,
        src: &wgpu::Texture,
        dst: &wgpu::Texture,
        si: usize,
        anchors: &[Option<wgpu::Texture>; 2],
        g_blur: f32,
    ) -> Result<(), String> {
        let size = src.width();
        let format = target.format();
        let raw = self.acquire(device, size, format);
        let dil = self.acquire(device, size, format);
        let blurred = self.acquire(device, size, format);
        let mut plain = e.clone();
        plain.blend = BlendMode::Replace;
        plain.opacity = 1.0;
        let blur_def = registry().get("blur").ok_or("blur effect missing")?;
        let mut blur = blur_def.instantiate(0);
        blur.params.insert("intensity".into(), ParamValue::Float(g_blur));
        blur.blend = BlendMode::Replace;
        let result = {
            let set = &self.sets[si];
            let inputs = EffectInputs { geo_pos: &set.geo_pos, geo_nrm: &set.geo_nrm, baked: &set.baked, scale: self.scale };
            self.engine
                .run(device, encoder, def, &plain, target, src, &raw, &inputs, [anchors[0].as_ref(), anchors[1].as_ref()])
                .and_then(|_| {
                    self.engine.dilate(device, encoder, &raw, &dil, &set.geo_pos);
                    self.engine.run(device, encoder, blur_def, &blur, target, &dil, &blurred, &inputs, [None, None])
                })
        };
        if result.is_ok() {
            // Blend the blurred value over the input with the instance's mode and opacity.
            match target {
                PassTarget::Mask => {
                    let params = Vec4x3 { a: [0.0; 4], b: [e.blend.gpu_id() as f32, e.opacity, 2.0, 0.0], c: [0.0; 4] };
                    self.compose_pass(device, encoder, ComposeKind::MaskItem, &params, Some(src), Some(&blurred), None, dst);
                }
                PassTarget::Content { color, .. } => {
                    let params = Vec4x3 { a: [0.0; 4], b: [e.blend.gpu_id() as f32, e.opacity, 0.0, 0.0], c: [color as u32 as f32, 0.0, 0.0, 0.0] };
                    self.compose_pass(device, encoder, ComposeKind::Layer, &params, Some(src), Some(&blurred), None, dst);
                }
            }
        }
        self.release(raw);
        self.release(dil);
        self.release(blurred);
        result
    }

    fn resolve_anchors(&self, def: &EffectDef, e: &EffectInstance, si: usize, channel: Option<ChannelId>) -> [Option<wgpu::Texture>; 2] {
        let mut out = [None, None];
        let mut slot = 0;
        for p in def.all_params() {
            if !matches!(p.kind, ParamKind::Anchor) {
                continue;
            }
            if slot >= 2 {
                break;
            }
            if let ParamValue::Anchor(Some(id)) = def.value(e, &p) {
                out[slot] = self
                    .anchors
                    .get(&(id, si, channel))
                    .or_else(|| self.anchors.get(&(id, si, None)))
                    .or_else(|| self.anchors.get(&(id, si, Some(ChannelId::BaseColor))))
                    .cloned();
            }
            slot += 1;
        }
        out
    }

    fn write_display(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, si: usize, c: ChannelId, acc: &wgpu::Texture) {
        let set = &self.sets[si];
        let Some(out) = set.display.get(&c) else { return };
        let pipeline = &self.display_pipelines[&out.format()];
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[gpu::tex_entry(0, &gpu::view(acc)), gpu::tex_entry(1, &gpu::view(&set.geo_pos))],
        });
        let view = gpu::view(out);
        let mut pass = gpu::pass(encoder, &view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    // ============================================================== thumbnails

    pub fn thumb(&self, node: u64, mask: bool) -> Option<&Thumb> {
        self.thumbs.get(&(node, mask))
    }

    pub fn invalidate_thumbs(&mut self) {
        self.thumbs.clear();
    }

    fn update_thumbs(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        if self.sets.is_empty() || self.stroke.is_some() {
            return;
        }
        let si = self.thumb_set.min(self.sets.len() - 1);
        let mut sources: Vec<((u64, bool), wgpu::Texture, f32)> = Vec::new();
        self.doc.visit(|n| {
            let content = match &n.kind {
                NodeKind::Paint => match self.content_cache.get(&(n.id, si, ChannelId::BaseColor)) {
                    Some((_, t)) if n.effects.iter().any(|e| e.enabled) => Some((t.clone(), 0.0)),
                    _ => self.paint.get(&n.id).and_then(|l| l.get(si)).and_then(|c| c[ChannelId::BaseColor.index()].clone()).map(|t| (t, 1.0)),
                },
                NodeKind::Folder { .. } => self.folder_thumbs.get(&n.id).cloned().map(|t| (t, 0.0)),
                _ => None,
            };
            if let Some((t, mode)) = content {
                sources.push(((n.id, false), t, mode));
            }
            if let Some((_, t)) = self.mask_cache.get(&(n.id, si))
                && n.mask.as_ref().is_some_and(|m| m.enabled)
            {
                sources.push(((n.id, true), t.clone(), 2.0));
            }
        });
        let live: Vec<(u64, bool)> = sources.iter().map(|s| s.0).collect();
        self.thumbs.retain(|k, _| live.contains(k));
        for (key, source, mode) in sources {
            if self.thumbs.get(&key).is_some_and(|t| t.source == source) {
                continue;
            }
            let texture = match self.thumbs.remove(&key) {
                Some(t) => t.texture,
                None => gpu::texture(device, "thumb", THUMB_SIZE, THUMB_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING),
            };
            let taps = (source.width() / THUMB_SIZE).clamp(1, 8) as f32;
            let params = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("thumb params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&params, 0, bytemuck::cast_slice(&[mode, taps, 0.0f32, 0.0]));
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("thumb"),
                layout: &self.thumb_pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                    gpu::tex_entry(1, &gpu::view(&source)),
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

    // ============================================================== project data access

    /// Painted channel textures (for saving).
    pub fn paint_textures(&self) -> &HashMap<u64, Vec<ChannelTextures>> {
        &self.paint
    }

    pub fn mask_paint_textures(&self) -> &HashMap<u64, Vec<wgpu::Texture>> {
        &self.mask_paint
    }

    // ============================================================== readback

    /// Final linear values of a channel (rgba f32 per texel, straight alpha).
    pub fn read_final(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, c: ChannelId) -> Option<(u32, Vec<f32>)> {
        let s = self.sets.get(set)?;
        let t = s.finals.get(&c)?;
        let bytes = gpu::read_texture(device, queue, t)?;
        let halves: &[half::f16] = bytemuck::cast_slice(&bytes);
        Some((s.size, halves.iter().map(|h| h.to_f32()).collect()))
    }

    /// Texels covered by UV islands (from the geometry position map).
    pub fn read_coverage(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize) -> Option<Vec<bool>> {
        let s = self.sets.get(set)?;
        let bytes = gpu::read_texture(device, queue, &s.geo_pos)?;
        let f: &[f32] = bytemuck::cast_slice(&bytes);
        Some(f.chunks_exact(4).map(|p| p[3] > 0.5).collect())
    }

    /// A baked map as linear rgba f32 per texel (scalars replicated in rgb).
    pub fn read_baked(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, map: BakedMap) -> Option<Vec<f32>> {
        let t = &self.sets.get(set)?.baked[map.index()];
        let bytes = gpu::read_texture(device, queue, t)?;
        Some(match t.format() {
            wgpu::TextureFormat::R8Unorm => bytes.iter().flat_map(|&b| {
                let v = b as f32 / 255.0;
                [v, v, v, 1.0]
            }).collect(),
            wgpu::TextureFormat::Rgba16Float => bytemuck::cast_slice::<u8, half::f16>(&bytes).iter().map(|h| h.to_f32()).collect(),
            _ => bytes.iter().map(|&b| b as f32 / 255.0).collect(),
        })
    }

    /// A channel as 8-bit: RGB (sRGB-encoded for Base Color / Emissive) or gray.
    pub fn read_channel(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, c: ChannelId) -> Option<(u32, Vec<u8>)> {
        let (size, values) = self.read_final(device, queue, set, c)?;
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let data = if c.is_color() {
            values
                .chunks_exact(4)
                .flat_map(|p| {
                    let rgb = [p[0], p[1], p[2]];
                    if c.is_srgb() { rgb.map(|v| to8(s3dp_core::document::linear_to_srgb(v))) } else { rgb.map(to8) }
                })
                .collect()
        } else {
            values.chunks_exact(4).map(|p| to8(p[0])).collect()
        };
        Some((size, data))
    }

    /// Samples final channel values at `uv` (eyedropper). Colors are linear.
    pub fn sample(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, uv: [f32; 2]) -> Option<ChannelValues> {
        let s = self.sets.get(set)?;
        let x = ((uv[0].rem_euclid(1.0) * s.size as f32) as u32).min(s.size - 1);
        let y = ((uv[1].rem_euclid(1.0) * s.size as f32) as u32).min(s.size - 1);
        let mut values = ChannelValues::defaults();
        for c in self.doc.channels.iter() {
            let Some(t) = s.finals.get(&c) else { continue };
            let bytes = gpu::read_region(device, queue, t, x, y)?;
            let h: &[half::f16] = bytemuck::cast_slice(&bytes);
            values.set(c, [h[0].to_f32(), h[1].to_f32(), h[2].to_f32()]);
        }
        Some(values)
    }

    /// World-space normal of the surface at `uv` (for dragging light directions in the viewport).
    pub fn world_normal_at(&self, device: &wgpu::Device, queue: &wgpu::Queue, set: usize, uv: [f32; 2]) -> Option<[f32; 3]> {
        let s = self.sets.get(set)?;
        let x = ((uv[0].rem_euclid(1.0) * s.size as f32) as u32).min(s.size - 1);
        let y = ((uv[1].rem_euclid(1.0) * s.size as f32) as u32).min(s.size - 1);
        let bytes = gpu::read_region(device, queue, &s.baked[BakedMap::WorldNormal.index()], x, y)?;
        let h: &[half::f16] = bytemuck::cast_slice(&bytes);
        let n = glam::Vec3::new(h[0].to_f32(), h[1].to_f32(), h[2].to_f32()) * 2.0 - 1.0;
        Some(n.normalize_or(glam::Vec3::Y).into())
    }

    /// Effect definitions referenced by the document that are unknown.
    pub fn missing_effects(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.doc.visit(|n| {
            for e in n.own_effects() {
                if registry().get(&e.def).is_none() && !out.contains(&e.def) {
                    out.push(e.def.clone());
                }
            }
        });
        out
    }

    /// Ensures outputs exist for newly enabled channels.
    pub fn channels_changed(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.ensure_outputs(device, queue);
        self.mark_all_dirty();
    }
}

impl Top {
    fn parts(&self) -> (f32, [f32; 3], Option<&wgpu::Texture>) {
        match self {
            Top::Straight(t) => (0.0, [0.0; 3], Some(t)),
            Top::Premultiplied(t) => (1.0, [0.0; 3], Some(t)),
            Top::Constant(v) => (2.0, *v, None),
        }
    }

    /// Cached results are straight; the wrapper keeps the pool from reclaiming them.
    fn into_cached(self) -> Top {
        match self {
            Top::Premultiplied(t) | Top::Straight(t) => Top::Straight(t),
            c => c,
        }
    }
}
