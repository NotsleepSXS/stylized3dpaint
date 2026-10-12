//! GPU mesh map baker.
//!
//! A [`BakeJob`] bakes one texture set at a time and advances a little every frame
//! (`step`), so the UI stays responsive:
//! 1. rasterize a UV-space G-buffer of the low-poly mesh,
//! 2. `project`: find the detail surface (high-poly along the normal, or the mesh itself)
//!    and write the tangent-space normal map,
//! 3. `maps`: trace AO, edges, cavity, thickness and sky light tile by tile,
//! 4. optional geometry-aware smoothing, then extract + dilate into the texture set's
//!    baked maps (sampled by the viewport and, later, by generators).

use egui_wgpu::wgpu;
use s3dp_core::bvh::Bvh;
use s3dp_core::{BakeSettings, BakeStyle, BakedMap, Mesh, Vertex};
use wgpu::util::DeviceExt;

use crate::gpu;
use crate::paint::TextureSet;

const TILE: u32 = 128;
const POS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const HALF_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Display format of a baked map.
pub fn baked_format(m: BakedMap) -> wgpu::TextureFormat {
    match m {
        BakedMap::WorldNormal | BakedMap::Position => wgpu::TextureFormat::Rgba16Float,
        BakedMap::Normal => wgpu::TextureFormat::Rgba8Unorm,
        _ => wgpu::TextureFormat::R8Unorm,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct BakeParams {
    tile: [u32; 4],
    counts: [u32; 4],
    dist: [f32; 4],
    sky: [f32; 4],
    cage: [f32; 4],
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ExtractParams {
    mode: [f32; 4],
    bbox_min: [f32; 4],
    bbox_size: [f32; 4],
}

pub struct Baker {
    gbuffer_a: wgpu::RenderPipeline,
    gbuffer_b: wgpu::RenderPipeline,
    project: wgpu::ComputePipeline,
    maps: wgpu::ComputePipeline,
    smooth: wgpu::ComputePipeline,
    extract: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

impl Baker {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = |label, src: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };
        let gb = module("bake gbuffer", include_str!("bake_gbuffer.wgsl"));
        let bake = module("bake", include_str!("bake.wgsl"));
        let extract = module("bake extract", include_str!("bake_extract.wgsl"));
        const ATTRS: [wgpu::VertexAttribute; 4] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
        let gbuffer = |fs, formats: [wgpu::TextureFormat; 2]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("bake gbuffer"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &gb,
                    entry_point: Some("vs_uv"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &ATTRS,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &gb,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(formats[0].into()), Some(formats[1].into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let compute = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &bake,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let extract_pipelines = [wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Rgba16Float]
            .into_iter()
            .map(|f| (f, gpu::fullscreen_pipeline(device, "bake extract", &extract, "fs_extract", f)))
            .collect();
        Self {
            gbuffer_a: gbuffer("fs_a", [POS_FORMAT, HALF_FORMAT]),
            gbuffer_b: gbuffer("fs_b", [HALF_FORMAT, HALF_FORMAT]),
            project: compute("project"),
            maps: compute("maps"),
            smooth: compute("soften"),
            extract: extract_pipelines,
        }
    }

    /// Rasterizes world position / normal of every texture set into its geometry textures
    /// and fills the Position and World Normal maps (no ray tracing, instant).
    pub fn rasterize_geometry(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sets: &[TextureSet],
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        bbox_min: [f32; 3],
        bbox_size: [f32; 3],
    ) {
        let mut encoder = device.create_command_encoder(&Default::default());
        for set in sets {
            let (vp, vn) = (gpu::view(&set.geo_pos), gpu::view(&set.geo_nrm));
            {
                let attachment = |view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("geometry"),
                    color_attachments: &[attachment(&vp), attachment(&vn)],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.gbuffer_a);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(set.index_range(), 0, 0..1);
            }
            for (map, src, mode) in [(BakedMap::WorldNormal, &set.geo_nrm, 2.0), (BakedMap::Position, &set.geo_pos, 3.0)] {
                let params = ExtractParams {
                    mode: [mode, 0.0, 0.0, 0.0],
                    bbox_min: [bbox_min[0], bbox_min[1], bbox_min[2], 0.0],
                    bbox_size: [bbox_size[0], bbox_size[1], bbox_size[2], 0.0],
                };
                self.extract_into(device, &mut encoder, &params, src, &set.geo_pos, &set.baked[map.index()]);
            }
        }
        queue.submit([encoder.finish()]);
    }

    fn extract_into(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        params: &ExtractParams,
        src: &wgpu::Texture,
        coverage: &wgpu::Texture,
        out: &wgpu::Texture,
    ) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("extract params"),
            contents: bytemuck::bytes_of(params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let pipeline = self.extract_pipeline(out.format());
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("extract"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                gpu::tex_entry(1, &gpu::view(src)),
                gpu::tex_entry(2, &gpu::view(coverage)),
            ],
        });
        let view = gpu::view(out);
        let mut pass = gpu::pass(encoder, &view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    fn extract_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        &self.extract.iter().find(|(f, _)| *f == format).expect("extract pipeline").1
    }
}

/// Intermediate textures for the texture set being baked.
struct SetWork {
    g_pos: wgpu::Texture,
    g_nrm: wgpu::Texture,
    g_tan: wgpu::Texture,
    g_face: wgpu::Texture,
    surf_pos: wgpu::Texture,
    surf_nrm: wgpu::Texture,
    normal_map: wgpu::Texture,
    maps_a: wgpu::Texture,
    maps_b: wgpu::Texture,
    tmp: wgpu::Texture,
    next_tile: u32,
}

pub struct BakeJob {
    params: BakeParams,
    smoothing: f32,
    params_buffer: wgpu::Buffer,
    nodes: wgpu::Buffer,
    tris: wgpu::Buffer,
    bbox_min: [f32; 3],
    bbox_size: [f32; 3],
    set: usize,
    set_count: usize,
    tiles_per_set: u32,
    work: Option<SetWork>,
    done: bool,
}

impl BakeJob {
    /// Prepares a bake of every texture set. `high` is the optional detail mesh.
    pub fn new(
        device: &wgpu::Device,
        low: &Mesh,
        high: Option<&Mesh>,
        settings: &BakeSettings,
        size: u32,
        set_count: usize,
    ) -> Self {
        let bounds = low.bounds();
        let radius = bounds.radius();
        let source = high.unwrap_or(low);
        let bvh = Bvh::build(source);
        let stylized = settings.style == BakeStyle::Stylized;
        let params = BakeParams {
            tile: [0, 0, TILE, TILE],
            counts: [settings.ao_samples, settings.edge_samples, settings.thickness_samples, settings.sky_samples],
            dist: [
                settings.ao_distance * radius,
                settings.edge_width * radius,
                settings.thickness_distance * radius,
                radius * 2e-4,
            ],
            sky: {
                let d = glam::Vec3::from(settings.sky_direction).normalize_or(glam::Vec3::Y);
                [d.x, d.y, d.z, settings.sky_softness.clamp(0.0, 89.0).to_radians().cos()]
            },
            cage: [
                settings.cage_front * radius,
                settings.cage_back * radius,
                if stylized { 1.0 } else { 0.0 },
                if high.is_some() { 1.0 } else { 0.0 },
            ],
            extra: [settings.smoothing * radius, radius, 0.0, 0.0],
        };
        let storage = |label, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let tiles = size.div_ceil(TILE);
        Self {
            params,
            smoothing: settings.smoothing * radius,
            params_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bake params"),
                size: std::mem::size_of::<BakeParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            nodes: storage("bvh nodes", bytemuck::cast_slice(&bvh.nodes)),
            tris: storage("bvh triangles", bytemuck::cast_slice(&bvh.triangles)),
            bbox_min: bounds.min.into(),
            bbox_size: (bounds.max - bounds.min).max(glam::Vec3::splat(1e-6)).into(),
            set: 0,
            set_count,
            tiles_per_set: tiles * tiles,
            work: None,
            done: set_count == 0,
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// 0..1 over all texture sets.
    pub fn progress(&self) -> f32 {
        if self.done || self.set_count == 0 {
            return 1.0;
        }
        let in_set = self.work.as_ref().map_or(0.0, |w| w.next_tile as f32 / self.tiles_per_set as f32);
        (self.set as f32 + in_set) / self.set_count as f32
    }

    /// Advances the bake by up to `max_tiles` tiles. Writes finished maps into `sets`.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        baker: &Baker,
        sets: &[TextureSet],
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        max_tiles: u32,
    ) {
        if self.done {
            return;
        }
        let Some(set) = sets.get(self.set) else {
            self.done = true;
            return;
        };
        if self.work.is_none() {
            self.work = Some(self.prepare(device, queue, baker, set, vertices, indices));
            return;
        }
        let tiles_x = set.size.div_ceil(TILE);
        let bind = self.maps_bind_groups(device, baker);
        let mut next = self.work.as_ref().unwrap().next_tile;
        for _ in 0..max_tiles {
            if next >= self.tiles_per_set {
                break;
            }
            let (tx, ty) = (next % tiles_x, next / tiles_x);
            let mut p = self.params;
            p.tile = [tx * TILE, ty * TILE, TILE, TILE];
            queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&p));
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&baker.maps);
                for (i, bg) in bind.iter().enumerate() {
                    pass.set_bind_group(i as u32, bg, &[]);
                }
                pass.dispatch_workgroups(TILE / 8, TILE / 8, 1);
            }
            queue.submit([encoder.finish()]);
            next += 1;
        }
        self.work.as_mut().unwrap().next_tile = next;
        if next >= self.tiles_per_set {
            self.finish(device, queue, baker, set);
            self.work = None;
            self.set += 1;
            self.done = self.set >= self.set_count;
        }
    }

    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        baker: &Baker,
        set: &TextureSet,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
    ) -> SetWork {
        use wgpu::TextureUsages as U;
        let size = set.size;
        let target = |label, format| gpu::texture(device, label, size, format, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
        let storage = |label, format| gpu::texture(device, label, size, format, U::STORAGE_BINDING | U::TEXTURE_BINDING);
        let work = SetWork {
            g_pos: target("gbuffer pos", POS_FORMAT),
            g_nrm: target("gbuffer normal", HALF_FORMAT),
            g_tan: target("gbuffer tangent", HALF_FORMAT),
            g_face: target("gbuffer face normal", HALF_FORMAT),
            surf_pos: storage("surface pos", POS_FORMAT),
            surf_nrm: storage("surface normal", HALF_FORMAT),
            normal_map: storage("normal map", HALF_FORMAT),
            maps_a: storage("maps a", HALF_FORMAT),
            maps_b: storage("maps b", HALF_FORMAT),
            tmp: storage("maps tmp", HALF_FORMAT),
            next_tile: 0,
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        for (pipeline, a, b) in [(&baker.gbuffer_a, &work.g_pos, &work.g_nrm), (&baker.gbuffer_b, &work.g_tan, &work.g_face)] {
            let (va, vb) = (gpu::view(a), gpu::view(b));
            let attachment = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bake gbuffer"),
                color_attachments: &[attachment(&va), attachment(&vb)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(set.index_range(), 0, 0..1);
        }
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&self.params));
        {
            let p = &baker.project;
            let groups = [
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("project 0"),
                    layout: &p.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: self.params_buffer.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: self.nodes.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: self.tris.as_entire_binding() },
                    ],
                }),
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("project 1"),
                    layout: &p.get_bind_group_layout(1),
                    entries: &[
                        gpu::tex_entry(0, &gpu::view(&work.g_pos)),
                        gpu::tex_entry(1, &gpu::view(&work.g_nrm)),
                        gpu::tex_entry(2, &gpu::view(&work.g_tan)),
                    ],
                }),
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("project 2"),
                    layout: &p.get_bind_group_layout(2),
                    entries: &[
                        gpu::tex_entry(0, &gpu::view(&work.surf_pos)),
                        gpu::tex_entry(1, &gpu::view(&work.surf_nrm)),
                        gpu::tex_entry(2, &gpu::view(&work.normal_map)),
                    ],
                }),
            ];
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(p);
            for (i, bg) in groups.iter().enumerate() {
                pass.set_bind_group(i as u32, bg, &[]);
            }
            pass.dispatch_workgroups(size.div_ceil(8), size.div_ceil(8), 1);
        }
        queue.submit([encoder.finish()]);
        work
    }

    fn maps_bind_groups(&self, device: &wgpu::Device, baker: &Baker) -> [wgpu::BindGroup; 3] {
        let work = self.work.as_ref().unwrap();
        let p = &baker.maps;
        [
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("maps 0"),
                layout: &p.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.nodes.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.tris.as_entire_binding() },
                ],
            }),
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("maps 1"),
                layout: &p.get_bind_group_layout(1),
                entries: &[gpu::tex_entry(3, &gpu::view(&work.g_face))],
            }),
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("maps 2"),
                layout: &p.get_bind_group_layout(2),
                entries: &[
                    gpu::tex_entry(3, &gpu::view(&work.surf_pos)),
                    gpu::tex_entry(4, &gpu::view(&work.surf_nrm)),
                    gpu::tex_entry(5, &gpu::view(&work.maps_a)),
                    gpu::tex_entry(6, &gpu::view(&work.maps_b)),
                ],
            }),
        ]
    }

    /// Smooths (optionally), then extracts and dilates every map into the set.
    fn finish(&self, device: &wgpu::Device, queue: &wgpu::Queue, baker: &Baker, set: &TextureSet) {
        let work = self.work.as_ref().unwrap();
        let size = set.size;
        if self.smoothing > 0.0 {
            for (index, target) in [&work.maps_a, &work.maps_b].into_iter().enumerate() {
                // Separable blur: target -> tmp along x, tmp -> target along y.
                for (axis, src, dst) in [(0.0, target, &work.tmp), (1.0, &work.tmp, target)] {
                    let mut p = self.params;
                    p.extra[2] = axis;
                    p.extra[3] = index as f32;
                    queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&p));
                    let s = &baker.smooth;
                    let groups = [
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("smooth 0"),
                            layout: &s.get_bind_group_layout(0),
                            entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.params_buffer.as_entire_binding() }],
                        }),
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("smooth 1"),
                            layout: &s.get_bind_group_layout(1),
                            entries: &[gpu::tex_entry(0, &gpu::view(&work.g_pos)), gpu::tex_entry(1, &gpu::view(&work.g_nrm))],
                        }),
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("smooth 2"),
                            layout: &s.get_bind_group_layout(2),
                            entries: &[gpu::tex_entry(7, &gpu::view(src)), gpu::tex_entry(8, &gpu::view(dst))],
                        }),
                    ];
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_compute_pass(&Default::default());
                        pass.set_pipeline(s);
                        for (i, bg) in groups.iter().enumerate() {
                            pass.set_bind_group(i as u32, bg, &[]);
                        }
                        pass.dispatch_workgroups(size.div_ceil(8), size.div_ceil(8), 1);
                    }
                    queue.submit([encoder.finish()]);
                }
            }
        }

        let mut encoder = device.create_command_encoder(&Default::default());
        for map in BakedMap::ALL {
            let (src, mode, channel) = match map {
                BakedMap::AmbientOcclusion => (&work.maps_a, 0.0, 0.0),
                BakedMap::Edges => (&work.maps_a, 0.0, 1.0),
                BakedMap::Cavity => (&work.maps_a, 0.0, 2.0),
                BakedMap::Thickness => (&work.maps_a, 0.0, 3.0),
                BakedMap::SkyLight => (&work.maps_b, 0.0, 0.0),
                BakedMap::Curvature => (&work.maps_b, 0.0, 1.0),
                BakedMap::Normal => (&work.normal_map, 1.0, 0.0),
                BakedMap::WorldNormal => (&work.g_nrm, 2.0, 0.0),
                BakedMap::Position => (&work.g_pos, 3.0, 0.0),
            };
            let params = ExtractParams {
                mode: [mode, channel, 0.0, 0.0],
                bbox_min: [self.bbox_min[0], self.bbox_min[1], self.bbox_min[2], 0.0],
                bbox_size: [self.bbox_size[0], self.bbox_size[1], self.bbox_size[2], 0.0],
            };
            baker.extract_into(device, &mut encoder, &params, src, &work.g_pos, &set.baked[map.index()]);
        }
        queue.submit([encoder.finish()]);
    }

    /// Runs the whole bake synchronously (tests, CLI).
    #[allow(clippy::too_many_arguments)]
    pub fn run_to_end(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        baker: &Baker,
        sets: &[TextureSet],
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
    ) {
        while !self.done {
            self.step(device, queue, baker, sets, vertices, indices, 16);
            device.poll(wgpu::PollType::wait_indefinitely()).ok();
        }
    }
}
