//! Runs declarative effects (`s3dp_core::effect_def`) on the GPU.
//!
//! Every effect is compiled from `prelude.wgsl` + generated parameter accessors + the effect's
//! own WGSL body + a generated `fs_main`. All effects share one explicit bind group layout, so
//! the engine can feed any effect the same inputs.

use std::collections::HashMap;

use egui_wgpu::wgpu;
use s3dp_core::effect_def::{Category, EffectDef, ParamKind};
use s3dp_core::{BakedMap, EffectInstance, ParamValue};
use wgpu::util::DeviceExt;

use crate::gpu;

/// Content buffers: linear rgb + coverage alpha.
pub const CONTENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Mask buffers: value in r.
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

const PARAM_SLOTS: usize = 48;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    params: [[f32; 4]; PARAM_SLOTS],
    info: [u32; 4],
    blend: [f32; 4],
    scale: [f32; 4],
    bbox_min: [f32; 4],
    bbox_size: [f32; 4],
}

/// Object size used to turn relative parameters into world units.
#[derive(Clone, Copy, Debug)]
pub struct MeshScale {
    pub radius: f32,
    pub bbox_min: [f32; 3],
    pub bbox_size: [f32; 3],
}

impl Default for MeshScale {
    fn default() -> Self {
        Self { radius: 1.0, bbox_min: [-1.0; 3], bbox_size: [2.0; 3] }
    }
}

/// Per texture set inputs every effect can read.
pub struct EffectInputs<'a> {
    pub geo_pos: &'a wgpu::Texture,
    pub geo_nrm: &'a wgpu::Texture,
    /// Indexed by `BakedMap::index`.
    pub baked: &'a [wgpu::Texture],
    pub scale: MeshScale,
}

/// What a pass works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassTarget {
    /// A material channel (`channel` id, `color` = rgb channel).
    Content { channel: u32, color: bool },
    Mask,
}

impl PassTarget {
    pub fn format(self) -> wgpu::TextureFormat {
        match self {
            Self::Content { .. } => CONTENT_FORMAT,
            Self::Mask => MASK_FORMAT,
        }
    }
}

pub struct EffectEngine {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<(String, wgpu::TextureFormat), Result<wgpu::RenderPipeline, String>>,
    sampler: wgpu::Sampler,
    dilate: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    dummy: wgpu::Texture,
}

fn tex_layout(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

const MAIN_TEMPLATE: &str = r#"
@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(frag.xy);
    var c: Ctx;
    c.xy = xy;
    c.size = tex_size();
    c.uv = (vec2<f32>(xy) + 0.5) / vec2<f32>(c.size);
    let gp = textureLoad(geo_pos, xy, 0);
    c.covered = gp.w > 0.5;
    c.p = gp.xyz;
    c.n = world_normal(xy);
    c.src = load_src(xy);
    if (!c.covered) { return c.src; }
    var out = effect(c);
    let mode = u32(U.blend.x);
    let op = clamp(U.blend.y, 0.0, 1.0);
    __GENERATOR__
    if (is_mask()) {
        let v = blend_mode(vec3<f32>(c.src.r), vec3<f32>(out.r), mode).r;
        return vec4<f32>(clamp(mix(c.src.r, v, op), 0.0, 1.0));
    }
    __CONTENT__
}
"#;

/// Generators shape their value with the shared block; content targets get it as a gray
/// value ("gray") or as a value with coverage ("value": rgb = channel value, a = amount).
const GENERATOR_MAIN: &str = r#"
    out.a = generator_finish(c, out.a);
    if (is_mask()) { out = vec4<f32>(out.a); }
"#;

const CONTENT_FILTER: &str = r#"
    var rgb = blend_mode(c.src.rgb, out.rgb, mode);
    if (!is_color_channel()) { rgb = vec3<f32>(rgb.r); }
    return mix(c.src, vec4<f32>(rgb, out.a), op);
"#;

const CONTENT_GRAY: &str = r#"
    let g = vec4<f32>(out.a, out.a, out.a, 1.0);
    var rgb = blend_mode(c.src.rgb, g.rgb, mode);
    return mix(c.src, vec4<f32>(rgb, max(c.src.a, 1.0)), op);
"#;

const CONTENT_VALUE: &str = r#"
    return composite(c.src, out.rgb, out.a * op, mode);
"#;

const DILATE_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var geo_pos: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

// Pushes island borders outwards so neighbourhood filters never read empty texels.
@fragment
fn fs_dilate(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    if (textureLoad(geo_pos, xy, 0).w > 0.5) { return textureLoad(src, xy, 0); }
    let last = vec2<i32>(textureDimensions(src)) - 1;
    var best = textureLoad(src, xy, 0);
    var best_d = 1e9;
    for (var r = 1; r <= 8; r = r + 1) {
        for (var dy = -r; dy <= r; dy = dy + 1) {
            for (var dx = -r; dx <= r; dx = dx + 1) {
                if (max(abs(dx), abs(dy)) != r) { continue; }
                let q = clamp(xy + vec2<i32>(dx, dy), vec2<i32>(0), last);
                let d = f32(dx * dx + dy * dy);
                if (d < best_d && textureLoad(geo_pos, q, 0).w > 0.5) {
                    best_d = d;
                    best = textureLoad(src, q, 0);
                }
            }
        }
        if (best_d < 1e9) { break; }
    }
    return best;
}
"#;

/// Full WGSL source of an effect.
pub fn effect_source(def: &EffectDef) -> String {
    let mut s = String::from(include_str!("prelude.wgsl"));
    s.push_str("\n// ---- generated parameter accessors ----\n");
    for (slot, p) in def.all_params().iter().enumerate() {
        s.push_str(&p.wgsl_accessor(slot));
    }
    if def.category.is_generator() {
        s.push_str(include_str!("generator.wgsl"));
    }
    s.push_str("\n// ---- effect body ----\n");
    s.push_str(&def.wgsl);
    let generator = if def.category.is_generator() { GENERATOR_MAIN } else { "" };
    let content = match (def.category.is_generator(), def.content_output.as_str()) {
        (true, "value") => CONTENT_VALUE,
        (true, _) => CONTENT_GRAY,
        (false, _) => CONTENT_FILTER,
    };
    s.push_str(&MAIN_TEMPLATE.replace("__GENERATOR__", generator).replace("__CONTENT__", content));
    s
}

impl EffectEngine {
    pub fn new(device: &wgpu::Device) -> Self {
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            tex_layout(1, true),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            tex_layout(3, false),
        ];
        for b in 4..=13 {
            entries.push(tex_layout(b, true));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("effect"), entries: &entries });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effect"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let dilate_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effect dilate"),
            source: wgpu::ShaderSource::Wgsl(DILATE_WGSL.into()),
        });
        let dilate = [CONTENT_FORMAT, MASK_FORMAT]
            .into_iter()
            .map(|f| (f, gpu::fullscreen_pipeline(device, "effect dilate", &dilate_module, "fs_dilate", f)))
            .collect();
        Self {
            layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("effect"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            dilate,
            dummy: gpu::texture(device, "effect dummy", 1, CONTENT_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING),
        }
    }

    /// Compiles (once) the pipeline for an effect; compile errors are returned, not panicked,
    /// so a broken effect only marks itself red (spec §11).
    pub fn pipeline(&mut self, device: &wgpu::Device, def: &EffectDef, format: wgpu::TextureFormat) -> Result<&wgpu::RenderPipeline, String> {
        let key = (def.id.clone(), format);
        if !self.pipelines.contains_key(&key) {
            let source = effect_source(def);
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&def.id),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&def.id),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            });
            let error = pollster_block(scope.pop());
            let result = match error {
                Some(e) => {
                    log::error!("effect {} failed to compile: {e}", def.id);
                    Err(e.to_string())
                }
                None => Ok(pipeline),
            };
            self.pipelines.insert(key.clone(), result);
        }
        self.pipelines[&key].as_ref().map_err(|e| e.clone())
    }

    /// Copies `src` into `dst`, extending island borders (for neighbourhood filters).
    pub fn dilate(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, src: &wgpu::Texture, dst: &wgpu::Texture, geo_pos: &wgpu::Texture) {
        let pipeline = &self.dilate[&dst.format()];
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("effect dilate"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[gpu::tex_entry(0, &gpu::view(src)), gpu::tex_entry(1, &gpu::view(geo_pos))],
        });
        let view = gpu::view(dst);
        let mut pass = gpu::pass(encoder, &view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Runs one effect instance: reads `src`, writes `dst` (same size; formats per target).
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        def: &EffectDef,
        effect: &EffectInstance,
        target: PassTarget,
        src: &wgpu::Texture,
        dst: &wgpu::Texture,
        inputs: &EffectInputs,
        anchors: [Option<&wgpu::Texture>; 2],
    ) -> Result<(), String> {
        let uniforms = self.uniforms(def, effect, target, inputs, &anchors);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("effect params"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let dummy = gpu::view(&self.dummy);
        let baked = |m: BakedMap| gpu::view(&inputs.baked[m.index()]);
        let views = [
            gpu::view(src),
            gpu::view(inputs.geo_pos),
            gpu::view(inputs.geo_nrm),
            baked(BakedMap::AmbientOcclusion),
            baked(BakedMap::Curvature),
            baked(BakedMap::Edges),
            baked(BakedMap::Cavity),
            baked(BakedMap::Thickness),
            baked(BakedMap::SkyLight),
            baked(BakedMap::Normal),
            anchors[0].map(gpu::view).unwrap_or_else(|| dummy.clone()),
            anchors[1].map(gpu::view).unwrap_or_else(|| dummy.clone()),
        ];
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
            gpu::tex_entry(1, &views[0]),
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ];
        for (i, v) in views[1..].iter().enumerate() {
            entries.push(gpu::tex_entry(3 + i as u32, v));
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("effect"), layout: &self.layout, entries: &entries });
        let pipeline = self.pipeline(device, def, dst.format())?;
        let view = gpu::view(dst);
        let mut pass = gpu::pass(encoder, &view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }

    fn uniforms(
        &self,
        def: &EffectDef,
        effect: &EffectInstance,
        target: PassTarget,
        inputs: &EffectInputs,
        anchors: &[Option<&wgpu::Texture>; 2],
    ) -> Uniforms {
        let mut u = Uniforms {
            params: [[0.0; 4]; PARAM_SLOTS],
            info: [0; 4],
            blend: [effect.blend.gpu_id() as f32, effect.opacity, 0.0, 0.0],
            scale: [inputs.scale.radius, 0.0, 0.0, 0.0],
            bbox_min: [inputs.scale.bbox_min[0], inputs.scale.bbox_min[1], inputs.scale.bbox_min[2], 0.0],
            bbox_size: [inputs.scale.bbox_size[0], inputs.scale.bbox_size[1], inputs.scale.bbox_size[2], 0.0],
        };
        let mut anchor_slot = 0;
        for (slot, p) in def.all_params().iter().enumerate().take(PARAM_SLOTS) {
            let v = def.value(effect, p);
            u.params[slot] = match (&p.kind, &v) {
                (_, ParamValue::Color(c)) | (_, ParamValue::Vec3(c)) => [c[0], c[1], c[2], 0.0],
                (ParamKind::Anchor, _) => {
                    let bound = anchors.get(anchor_slot).is_some_and(|a| a.is_some());
                    anchor_slot += 1;
                    [bound as u32 as f32, 0.0, 0.0, 0.0]
                }
                _ => [v.as_f32(), 0.0, 0.0, 0.0],
            };
            if p.id == "seed" {
                u.scale[1] = v.as_f32().max(0.0);
            }
        }
        u.blend[2] = anchors[0].is_some() as u32 as f32;
        u.blend[3] = anchors[1].is_some() as u32 as f32;
        u.info = match target {
            PassTarget::Content { channel, color } => [channel, color as u32, 0, def.category.is_generator() as u32],
            PassTarget::Mask => [0, 0, 1, def.category.is_generator() as u32],
        };
        u
    }
}

/// Category helper for UI ordering.
pub fn category_order(c: Category) -> usize {
    match c {
        Category::Generator => 0,
        Category::Wear => 1,
        Category::Stylize => 2,
        Category::Color => 3,
        Category::Blur => 4,
        Category::Warp => 5,
        Category::Relief => 6,
        Category::Filter => 7,
        Category::Utility => 8,
    }
}

fn pollster_block<F: std::future::Future>(f: F) -> F::Output {
    // Error scopes resolve immediately on native backends; a tiny executor avoids a dependency.
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop_raw() -> RawWaker {
        fn clone(_: *const ()) -> RawWaker {
            noop_raw()
        }
        fn noop(_: *const ()) {}
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}
