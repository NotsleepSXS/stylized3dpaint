//! GPU side of mask effects (generators, warp, blur, levels) and the Stylize filter.

use egui_wgpu::wgpu;
use s3dp_core::{BakedMap, EffectKind, GeneratorSource, MaskEffect, StylizeParams};
use wgpu::util::DeviceExt;

use crate::gpu;

pub const FX_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct EffectUniform {
    kind: [u32; 4],
    levels: [f32; 4],
    misc: [f32; 4],
    noise: [f32; 4],
    dir: [f32; 4],
    bbox_min: [f32; 4],
    bbox_size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct StylizeUniform {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
}

/// Mesh scale information needed to turn relative effect settings into world units.
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

fn uniform(e: &EffectKind, s: &MeshScale) -> EffectUniform {
    let mut u = EffectUniform {
        bbox_min: [s.bbox_min[0], s.bbox_min[1], s.bbox_min[2], 0.0],
        bbox_size: [s.bbox_size[0], s.bbox_size[1], s.bbox_size[2], 0.0],
        levels: [0.0, 1.0, 1.0, 0.0],
        misc: [1.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let noise = |n: &s3dp_core::NoiseParams| [n.scale * s.radius, n.octaves as f32, n.stretch, n.seed as f32];
    match e {
        EffectKind::Generator { source, levels, blend, opacity } => {
            let (src, noise_kind) = match source {
                GeneratorSource::Map(m) => (
                    match m {
                        BakedMap::AmbientOcclusion => 0,
                        BakedMap::Curvature => 1,
                        BakedMap::Edges => 2,
                        BakedMap::Cavity => 3,
                        BakedMap::Thickness => 4,
                        _ => 5,
                    },
                    0,
                ),
                GeneratorSource::HeightGradient { axis } => (10 + (*axis).min(2), 0),
                GeneratorSource::Noise(n) => {
                    u.noise = noise(n);
                    (20, n.kind.gpu_id())
                }
            };
            u.kind = [0, blend.gpu_id(), src, noise_kind];
            u.levels = [levels.low, levels.high, levels.gamma, levels.steps as f32];
            u.misc = [*opacity, if levels.invert { 1.0 } else { 0.0 }, 0.0, 0.0];
        }
        EffectKind::Warp(w) => {
            u.kind = [1, 0, w.mode as u32, w.noise.kind.gpu_id()];
            u.noise = noise(&w.noise);
            u.misc = [1.0, 0.0, w.intensity * s.radius, w.samples as f32];
            u.dir = [w.direction[0], w.direction[1], w.direction[2], w.sharpness];
        }
        EffectKind::Blur { radius } => {
            u.kind = [2, 0, 0, 0];
            u.misc = [1.0, 0.0, *radius, 0.0];
        }
        EffectKind::Levels(l) => {
            u.kind = [3, 0, 0, 0];
            u.levels = [l.low, l.high, l.gamma, l.steps as f32];
            u.misc = [1.0, if l.invert { 1.0 } else { 0.0 }, 0.0, 0.0];
        }
    }
    u
}

pub(crate) fn stylize_uniform(p: &StylizeParams, channel: u32, opacity: f32, use_mask: bool) -> StylizeUniform {
    StylizeUniform {
        a: [p.painterly, p.color_bands as f32, p.band_strength, p.saturation],
        b: [p.temperature, p.edge_highlight, p.cavity_darken, p.roughness_bands as f32],
        c: [p.edge_polish, p.metal_threshold, p.height_bevel, p.height_smooth],
        d: [channel as f32, opacity, if use_mask { 1.0 } else { 0.0 }, 0.0],
    }
}

/// Textures an effect chain reads, for one texture set.
pub(crate) struct EffectInputs<'a> {
    pub geo_pos: &'a wgpu::Texture,
    pub geo_nrm: &'a wgpu::Texture,
    pub baked: &'a [wgpu::Texture],
    pub tmp: &'a [wgpu::Texture; 2],
}

pub(crate) struct EffectRenderer {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl EffectRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
        });
        Self {
            pipeline: gpu::fullscreen_pipeline(device, "mask effect", &module, "fs_effect", FX_FORMAT),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("effects"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    /// Runs the enabled effects over `base` (a mask texture) into a new FX_FORMAT texture.
    pub fn run(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        effects: &[MaskEffect],
        base: &wgpu::Texture,
        inputs: &EffectInputs,
        scale: &MeshScale,
    ) -> wgpu::Texture {
        let size = base.width();
        let out = gpu::texture(device, "effect mask", size, FX_FORMAT, gpu::LAYER_USAGE);
        let enabled: Vec<&MaskEffect> = effects.iter().filter(|e| e.enabled).collect();
        let baked_view = |m: BakedMap| gpu::view(&inputs.baked[m.index()]);
        let (geo_pos, geo_nrm) = (gpu::view(inputs.geo_pos), gpu::view(inputs.geo_nrm));
        let maps = [
            baked_view(BakedMap::AmbientOcclusion),
            baked_view(BakedMap::Curvature),
            baked_view(BakedMap::Edges),
            baked_view(BakedMap::Cavity),
            baked_view(BakedMap::Thickness),
            baked_view(BakedMap::SkyLight),
        ];
        let mut prev = gpu::view(base);
        for (i, effect) in enabled.iter().enumerate() {
            let target = if i + 1 == enabled.len() { &out } else { &inputs.tmp[i % 2] };
            let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("effect params"),
                contents: bytemuck::bytes_of(&uniform(&effect.kind, scale)),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let mut entries = vec![
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                gpu::tex_entry(1, &prev),
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                gpu::tex_entry(3, &geo_pos),
                gpu::tex_entry(4, &geo_nrm),
            ];
            entries.extend(maps.iter().enumerate().map(|(k, v)| gpu::tex_entry(5 + k as u32, v)));
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("effect"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &entries,
            });
            let view = gpu::view(target);
            {
                let mut pass = gpu::pass(encoder, &view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            }
            prev = view;
        }
        if enabled.is_empty() {
            gpu::clear(encoder, &out, wgpu::Color::WHITE);
        }
        out
    }
}
