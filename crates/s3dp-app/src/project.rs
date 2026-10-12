//! `.s3dp` project files.
//!
//! Format 3 (current):
//! ```text
//! project.json                   ProjectFile { format, mesh, document (layer tree), bake settings }
//! mesh.bin                       vertices + indices (little-endian, see `write_mesh`)
//! paint/<node>/<set>/<ch>.f16    painted channel, raw RGBA f16 premultiplied linear
//! masks/<item>/<set>.f16         painted mask item, raw RG f16 premultiplied
//! baked/<set>/<map>.png          L (scalars), RGBA (normal), RGBA16 (world normal, position)
//! ```
//! Formats 1–2 (flat layer list, 8-bit sRGB layers, mask-only effects) are migrated on load.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use s3dp_core::legacy;
use s3dp_core::{
    BakeSettings, BakedMap, BlendMode, ChannelBlend, ChannelId, ChannelSet, ChannelValues, Document, EffectInstance, Mask, MaskItem,
    MaskItemKind, Mesh, MeshPart, Node, NodeKind, ParamValue, Vertex, registry,
};
use s3dp_render::paint::{ChannelTextures, MASK_PAINT_FORMAT, PAINT_FORMAT};
use s3dp_render::{Viewport, gpu, wgpu};
use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 3;

#[derive(Serialize, Deserialize)]
struct PartFile {
    name: String,
    first_index: u32,
    index_count: u32,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: u32,
    mesh_name: String,
    parts: Vec<PartFile>,
    texture_size: u32,
    document: Document,
    #[serde(default)]
    bake: Option<BakeSettings>,
}

/// Formats 1–2.
#[derive(Deserialize)]
struct LegacyProjectFile {
    format: u32,
    mesh_name: String,
    parts: Vec<PartFile>,
    texture_size: u32,
    defaults: legacy::ChannelValues,
    #[serde(default)]
    active_layer: usize,
    layers: Vec<legacy::Layer>,
    #[serde(default)]
    bake: Option<BakeSettings>,
}

fn png_bytes(data: &[u8], size: u32, color: image::ExtendedColorType) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::ImageEncoder::write_image(image::codecs::png::PngEncoder::new(&mut out), data, size, size, color)?;
    Ok(out)
}

pub fn save(path: &Path, mesh: &Mesh, viewport: &Viewport, bake: Option<&BakeSettings>, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<()> {
    let paint = &viewport.paint;
    let project = ProjectFile {
        format: FORMAT_VERSION,
        mesh_name: mesh.name.clone(),
        parts: mesh.parts.iter().map(|p| PartFile { name: p.name.clone(), first_index: p.first_index, index_count: p.index_count }).collect(),
        texture_size: viewport.texture_size(),
        document: paint.doc.clone(),
        bake: bake.cloned(),
    };
    // Write to a temp file first so a failed save never destroys the old project.
    let tmp = path.with_extension("s3dp.tmp");
    let file = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    zip.start_file("project.json", deflate)?;
    zip.write_all(&serde_json::to_vec_pretty(&project)?)?;
    zip.start_file("mesh.bin", deflate)?;
    write_mesh(&mut zip, mesh)?;

    // Only textures of nodes / items that still exist.
    let mut live = std::collections::HashSet::new();
    paint.doc.visit(|n| {
        live.insert(n.id);
        for item in n.mask.iter().flat_map(|m| m.items.iter()) {
            live.insert(item.id);
        }
    });
    for (node, sets) in paint.paint_textures() {
        if !live.contains(node) {
            continue;
        }
        for (si, channels) in sets.iter().enumerate() {
            for (ci, tex) in channels.iter().enumerate() {
                let (Some(t), Some(c)) = (tex, ChannelId::from_index(ci)) else { continue };
                let data = gpu::read_texture(device, queue, t).context("GPU readback failed")?;
                zip.start_file(format!("paint/{node}/{si}/{}.f16", c.short()), deflate)?;
                zip.write_all(&data)?;
            }
        }
    }
    for (item, sets) in paint.mask_paint_textures() {
        if !live.contains(item) {
            continue;
        }
        for (si, t) in sets.iter().enumerate() {
            let data = gpu::read_texture(device, queue, t).context("GPU readback failed")?;
            zip.start_file(format!("masks/{item}/{si}.f16"), deflate)?;
            zip.write_all(&data)?;
        }
    }
    if bake.is_some() {
        for (si, set) in paint.sets.iter().enumerate() {
            for map in BakedMap::ALL {
                let t = &set.baked[map.index()];
                let data = gpu::read_texture(device, queue, t).context("GPU readback failed")?;
                let png = match t.format() {
                    wgpu::TextureFormat::R8Unorm => png_bytes(&data, t.width(), image::ExtendedColorType::L8)?,
                    wgpu::TextureFormat::Rgba16Float => {
                        let halves: &[half::f16] = bytemuck::cast_slice(&data);
                        let words: Vec<u8> =
                            halves.iter().flat_map(|h| ((h.to_f32().clamp(0.0, 1.0) * 65535.0).round() as u16).to_ne_bytes()).collect();
                        png_bytes(&words, t.width(), image::ExtendedColorType::Rgba16)?
                    }
                    _ => png_bytes(&data, t.width(), image::ExtendedColorType::Rgba8)?,
                };
                zip.start_file(format!("baked/{si}/{}.png", map.file_name()), stored)?;
                zip.write_all(&png)?;
            }
        }
    }
    zip.finish()?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

type Zip = zip::ZipArchive<std::fs::File>;

fn read_entry(zip: &mut Zip, name: &str) -> Result<Vec<u8>> {
    let mut f = zip.by_name(name).with_context(|| format!("missing {name}"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Loads a project: replaces the viewport mesh and layer stack.
/// Returns the mesh and the bake settings (when baked maps were restored).
pub fn load(path: &Path, viewport: &mut Viewport, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(Mesh, Option<BakeSettings>)> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a Stylized3DPaint project")?;
    let json = read_entry(&mut zip, "project.json")?;
    let header: serde_json::Value = serde_json::from_slice(&json)?;
    let format = header.get("format").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    if format > FORMAT_VERSION {
        bail!("project was saved by a newer version (format {format})");
    }
    if format < 3 {
        return load_legacy(&json, &mut zip, viewport, device, queue);
    }
    let project: ProjectFile = serde_json::from_slice(&json)?;
    let mesh = load_mesh(&mut zip, &project.mesh_name, &project.parts, format)?;
    viewport.set_mesh(device, queue, &mesh, project.texture_size);
    let size = project.texture_size;
    let sets = mesh.parts.len();

    let mut paint: HashMap<u64, Vec<ChannelTextures>> = HashMap::new();
    let mut mask_paint: HashMap<u64, Vec<Option<wgpu::Texture>>> = HashMap::new();
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    for name in names {
        let parts: Vec<&str> = name.split('/').collect();
        match parts.as_slice() {
            ["paint", node, set, file] => {
                let (Ok(node), Ok(si)) = (node.parse::<u64>(), set.parse::<usize>()) else { continue };
                let Some(c) = ChannelId::ALL.into_iter().find(|c| format!("{}.f16", c.short()) == *file) else { continue };
                if si >= sets {
                    continue;
                }
                let data = read_entry(&mut zip, &name)?;
                let t = upload_checked(device, queue, size, PAINT_FORMAT, &data, &name)?;
                let list = paint.entry(node).or_insert_with(|| vec![vec![None; ChannelId::COUNT]; sets]);
                list[si][c.index()] = Some(t);
            }
            ["masks", item, file] => {
                let (Ok(item), Some(Ok(si))) = (item.parse::<u64>(), file.strip_suffix(".f16").map(str::parse::<usize>)) else { continue };
                if si >= sets {
                    continue;
                }
                let data = read_entry(&mut zip, &name)?;
                let t = upload_checked(device, queue, size, MASK_PAINT_FORMAT, &data, &name)?;
                let list = mask_paint.entry(item).or_insert_with(|| vec![None; sets]);
                list[si] = Some(t);
            }
            _ => {}
        }
    }
    // Sets without a painted mask texture get an empty one so indices line up.
    let mask_paint: HashMap<u64, Vec<wgpu::Texture>> = mask_paint
        .into_iter()
        .map(|(k, list)| {
            let full = list
                .into_iter()
                .map(|t| {
                    t.unwrap_or_else(|| {
                        let t = gpu::texture(device, "mask paint", size, MASK_PAINT_FORMAT, gpu::LAYER_USAGE);
                        gpu::write_full(queue, &t, &vec![0u8; (size * size * 4) as usize]);
                        t
                    })
                })
                .collect();
            (k, full)
        })
        .collect();
    viewport.paint.load_document(project.document, paint, mask_paint);
    viewport.paint.channels_changed(device, queue);
    let bake = load_baked(&mut zip, viewport, queue, sets, size, project.bake)?;
    Ok((mesh, bake))
}

fn load_mesh(zip: &mut Zip, name: &str, parts: &[PartFile], format: u32) -> Result<Mesh> {
    let mut mesh = read_mesh(&read_entry(zip, "mesh.bin")?, format)?;
    mesh.name = name.to_owned();
    mesh.parts = parts.iter().map(|p| MeshPart { name: p.name.clone(), first_index: p.first_index, index_count: p.index_count }).collect();
    Ok(mesh)
}

fn load_baked(zip: &mut Zip, viewport: &mut Viewport, queue: &wgpu::Queue, sets: usize, size: u32, bake: Option<BakeSettings>) -> Result<Option<BakeSettings>> {
    let mut bake = bake;
    if bake.is_some() {
        'sets: for si in 0..sets {
            for map in BakedMap::ALL {
                let name = format!("baked/{si}/{}.png", map.file_name());
                if zip.index_for_name(&name).is_none() {
                    bake = None;
                    break 'sets;
                }
                let img = image::load_from_memory(&read_entry(zip, &name)?)?;
                let t = &viewport.paint.sets[si].baked[map.index()];
                let data: Vec<u8> = match t.format() {
                    wgpu::TextureFormat::R8Unorm => img.to_luma8().into_raw(),
                    wgpu::TextureFormat::Rgba16Float => {
                        img.to_rgba16().into_raw().iter().flat_map(|&w| half::f16::from_f32(w as f32 / 65535.0).to_le_bytes()).collect()
                    }
                    _ => img.to_rgba8().into_raw(),
                };
                let bpp = t.format().block_copy_size(None).unwrap_or(4) as usize;
                if data.len() != (size * size) as usize * bpp {
                    bail!("{name} has the wrong size");
                }
                gpu::write_full(queue, t, &data);
            }
        }
    }
    viewport.paint.baked_changed();
    Ok(bake)
}

fn upload_checked(device: &wgpu::Device, queue: &wgpu::Queue, size: u32, format: wgpu::TextureFormat, data: &[u8], name: &str) -> Result<wgpu::Texture> {
    let bpp = format.block_copy_size(None).unwrap_or(4) as usize;
    if data.len() != size as usize * size as usize * bpp {
        bail!("{name} has the wrong size");
    }
    Ok(gpu::upload(device, queue, size, format, data))
}

fn f16_bytes(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    values.into_iter().flat_map(|v| half::f16::from_f32(v).to_le_bytes()).collect()
}

// ------------------------------------------------------------------------------------- migration

fn legacy_channel(c: legacy::Channel) -> ChannelId {
    match c {
        legacy::Channel::BaseColor => ChannelId::BaseColor,
        legacy::Channel::Roughness => ChannelId::Roughness,
        legacy::Channel::Metallic => ChannelId::Metallic,
        legacy::Channel::Height => ChannelId::Height,
        legacy::Channel::Emissive => ChannelId::Emissive,
    }
}

fn legacy_values(v: &legacy::ChannelValues) -> ChannelValues {
    let lin = |c: [f32; 3]| c.map(s3dp_core::document::srgb_to_linear);
    let mut out = ChannelValues::defaults();
    out.set(ChannelId::BaseColor, lin(v.base_color));
    out.set(ChannelId::Roughness, [v.roughness; 3]);
    out.set(ChannelId::Metallic, [v.metallic; 3]);
    out.set(ChannelId::Height, [v.height; 3]);
    out.set(ChannelId::Emissive, lin(v.emissive));
    out
}

fn legacy_blend(b: legacy::BlendMode) -> BlendMode {
    use legacy::BlendMode as L;
    match b {
        L::Normal => BlendMode::Normal,
        L::Darken => BlendMode::Darken,
        L::Multiply | L::ColorBurn | L::LinearBurn => BlendMode::Multiply,
        L::Lighten => BlendMode::Lighten,
        L::Screen => BlendMode::Screen,
        L::ColorDodge | L::LinearDodge => BlendMode::LinearDodge,
        L::Overlay => BlendMode::Overlay,
        L::SoftLight => BlendMode::SoftLight,
        L::HardLight => BlendMode::HardLight,
        L::Difference => BlendMode::Difference,
        L::Subtract => BlendMode::Subtract,
        L::Hue => BlendMode::Hue,
        L::Saturation => BlendMode::Saturation,
        L::Color => BlendMode::Color,
        L::Luminosity => BlendMode::Value,
    }
}

/// Old mask effects -> new effect instances (kinds without an equivalent are dropped).
fn legacy_effect(doc: &mut Document, e: &legacy::MaskEffect) -> Option<EffectInstance> {
    use legacy::{EffectKind, GeneratorSource};
    let mut make = |def: &str| -> Option<EffectInstance> {
        let id = doc.alloc_id();
        registry().get(def).map(|d| d.instantiate(id))
    };
    let mut inst = match &e.kind {
        EffectKind::Generator { source, levels, blend, opacity } => {
            let def = match source {
                GeneratorSource::HeightGradient { .. } => "position_gradient",
                GeneratorSource::Noise(_) => "noise",
                GeneratorSource::Map(BakedMap::AmbientOcclusion) => "ambient_occlusion",
                GeneratorSource::Map(BakedMap::Thickness) => "thickness",
                GeneratorSource::Map(_) => "curvature",
            };
            let mut inst = make(def)?;
            inst.opacity = *opacity;
            inst.blend = match blend {
                legacy::MaskBlend::Replace => BlendMode::Replace,
                legacy::MaskBlend::Multiply => BlendMode::Multiply,
                legacy::MaskBlend::Add => BlendMode::LinearDodge,
                legacy::MaskBlend::Subtract => BlendMode::Subtract,
                legacy::MaskBlend::Max => BlendMode::Lighten,
                legacy::MaskBlend::Min => BlendMode::Darken,
            };
            if levels.invert {
                inst.params.insert("g_invert".into(), ParamValue::Bool(true));
            }
            inst
        }
        EffectKind::Warp(w) => {
            let def = match w.mode {
                legacy::WarpMode::Warp => "warp",
                legacy::WarpMode::SlopeBlur => "slope_blur",
                legacy::WarpMode::Directional => "directional_warp",
            };
            let mut inst = make(def)?;
            inst.params.insert("intensity".into(), ParamValue::Float(w.intensity));
            inst
        }
        EffectKind::Blur { radius } => {
            let mut inst = make("blur")?;
            inst.params.insert("intensity".into(), ParamValue::Float((radius / 512.0).clamp(0.0, 0.2)));
            inst
        }
        EffectKind::Levels(l) => {
            let mut inst = make("levels")?;
            inst.params.insert("in_black".into(), ParamValue::Float(l.low));
            inst.params.insert("in_white".into(), ParamValue::Float(l.high));
            inst.params.insert("gamma".into(), ParamValue::Float(l.gamma));
            if l.invert {
                inst.params.insert("out_black".into(), ParamValue::Float(1.0));
                inst.params.insert("out_white".into(), ParamValue::Float(0.0));
            }
            inst
        }
    };
    inst.enabled = e.enabled;
    Some(inst)
}

fn load_legacy(json: &[u8], zip: &mut Zip, viewport: &mut Viewport, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(Mesh, Option<BakeSettings>)> {
    let project: LegacyProjectFile = serde_json::from_slice(json)?;
    let mesh = load_mesh(zip, &project.mesh_name, &project.parts, project.format)?;
    viewport.set_mesh(device, queue, &mesh, project.texture_size);
    let size = project.texture_size;
    let sets = mesh.parts.len();
    let mut doc = Document { defaults: legacy_values(&project.defaults), ..Document::default() };
    let mut paint: HashMap<u64, Vec<ChannelTextures>> = HashMap::new();
    let mut mask_paint: HashMap<u64, Vec<wgpu::Texture>> = HashMap::new();
    let channel_file = |c: legacy::Channel| match c {
        legacy::Channel::BaseColor => "base_color.png",
        legacy::Channel::Roughness => "roughness.png",
        legacy::Channel::Metallic => "metallic.png",
        legacy::Channel::Height => "height.png",
        legacy::Channel::Emissive => "emissive.png",
    };
    let mut active = None;
    for (index, layer) in project.layers.iter().enumerate() {
        let id = doc.alloc_id();
        let kind = match &layer.kind {
            legacy::LayerKind::Paint => NodeKind::Paint,
            legacy::LayerKind::Fill(v) => NodeKind::Fill(legacy_values(v)),
            legacy::LayerKind::Stylize(_) => {
                let eid = doc.alloc_id();
                match registry().get("stylize_master") {
                    Some(d) => NodeKind::Effect(d.instantiate(eid)),
                    None => {
                        log::warn!("migration: Stylize layer skipped (stylize_master effect missing)");
                        continue;
                    }
                }
            }
        };
        let mut node = Node::new(id, layer.name.clone(), kind);
        node.visible = layer.visible;
        node.blending.default = ChannelBlend { mode: legacy_blend(layer.blend), opacity: layer.opacity };
        // Old layers used their blend mode only on color channels.
        for c in [ChannelId::Roughness, ChannelId::Metallic, ChannelId::Height] {
            node.blending.per_channel.insert(c, ChannelBlend { mode: BlendMode::Normal, opacity: layer.opacity });
        }
        node.blending.channels = legacy::Channel::ALL.iter().filter(|c| layer.channels.has(**c)).map(|c| legacy_channel(*c)).collect::<ChannelSet>();
        // Painted channels: 8-bit sRGB premultiplied -> linear premultiplied f16.
        for si in 0..sets {
            for c in legacy::Channel::ALL {
                let name = format!("layers/{}/{si}/{}", layer.id, channel_file(c));
                if zip.index_for_name(&name).is_none() {
                    continue;
                }
                let img = image::load_from_memory(&read_entry(zip, &name)?)?;
                let data: Vec<f32> = if c.is_color() {
                    img.to_rgba8()
                        .pixels()
                        .flat_map(|p| {
                            let a = p[3] as f32 / 255.0;
                            let rgb = [0, 1, 2].map(|i| {
                                let straight = if a > 0.0 { p[i] as f32 / 255.0 / a } else { 0.0 };
                                s3dp_core::document::srgb_to_linear(straight.min(1.0)) * a
                            });
                            [rgb[0], rgb[1], rgb[2], a]
                        })
                        .collect()
                } else {
                    img.to_luma_alpha8()
                        .pixels()
                        .flat_map(|p| {
                            let (v, a) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0);
                            [v, v, v, a]
                        })
                        .collect()
                };
                let t = upload_checked(device, queue, size, PAINT_FORMAT, &f16_bytes(data), &name)?;
                let list = paint.entry(id).or_insert_with(|| vec![vec![None; ChannelId::COUNT]; sets]);
                list[si][legacy_channel(c).index()] = Some(t);
            }
        }
        // Mask: white base, painted texture, then the old mask effects.
        if layer.has_mask || !layer.effects.is_empty() {
            let mut items = Vec::new();
            let fill = doc.alloc_id();
            items.push(MaskItem { id: fill, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Fill(1.0) });
            if layer.has_mask {
                let item = doc.alloc_id();
                for si in 0..sets {
                    let name = format!("layers/{}/{si}/mask.png", layer.id);
                    let data: Vec<f32> = if zip.index_for_name(&name).is_some() {
                        let img = image::load_from_memory(&read_entry(zip, &name)?)?;
                        img.to_luma8().pixels().flat_map(|p| [p[0] as f32 / 255.0, 1.0]).collect()
                    } else {
                        vec![1.0; (size * size * 2) as usize]
                    };
                    let t = upload_checked(device, queue, size, MASK_PAINT_FORMAT, &f16_bytes(data), &name)?;
                    mask_paint.entry(item).or_default().push(t);
                }
                items.push(MaskItem { id: item, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Paint });
            }
            for e in &layer.effects {
                match legacy_effect(&mut doc, e) {
                    Some(inst) => {
                        let id = doc.alloc_id();
                        items.push(MaskItem { id, enabled: true, opacity: inst.opacity, blend: inst.blend, kind: MaskItemKind::Effect(inst) });
                    }
                    None => log::warn!("migration: dropped an old mask effect ({})", e.kind.label()),
                }
            }
            node.mask = Some(Mask { enabled: layer.mask_enabled || !layer.effects.is_empty(), items });
        }
        if index == project.active_layer {
            active = Some(id);
        }
        doc.nodes.push(node);
    }
    viewport.paint.load_document(doc, paint, mask_paint);
    viewport.paint.active = active.or(viewport.paint.active);
    viewport.paint.channels_changed(device, queue);
    let bake = load_baked(zip, viewport, queue, sets, size, project.bake)?;
    Ok((mesh, bake))
}

/// `u32 vertex count, u32 index count, vertices (Vertex, 48 bytes each), indices (u32)`.
fn write_mesh(w: &mut impl Write, mesh: &Mesh) -> Result<()> {
    w.write_all(&(mesh.vertices.len() as u32).to_le_bytes())?;
    w.write_all(&(mesh.indices.len() as u32).to_le_bytes())?;
    w.write_all(bytemuck::cast_slice(&mesh.vertices))?;
    w.write_all(bytemuck::cast_slice(&mesh.indices))?;
    Ok(())
}

fn read_mesh(bytes: &[u8], format: u32) -> Result<Mesh> {
    let header = bytes.get(..8).context("mesh.bin is truncated")?;
    let vcount = u32::from_le_bytes(header[..4].try_into()?) as usize;
    let icount = u32::from_le_bytes(header[4..8].try_into()?) as usize;
    // Format 1 stored position, normal, uv (no tangent).
    let stride = if format < 2 { 32 } else { std::mem::size_of::<Vertex>() };
    let vbytes = vcount * stride;
    let body = &bytes[8..];
    if body.len() != vbytes + icount * 4 {
        bail!("mesh.bin is corrupt");
    }
    // Copy out: the zip buffer is not guaranteed to be aligned for `Vertex`.
    let floats = |chunk: &[u8]| -> Vec<f32> { chunk.chunks_exact(4).map(bytemuck::pod_read_unaligned).collect() };
    let vertices: Vec<Vertex> = body[..vbytes]
        .chunks_exact(stride)
        .map(|c| {
            let f = floats(c);
            Vertex {
                position: [f[0], f[1], f[2]],
                normal: [f[3], f[4], f[5]],
                uv: [f[6], f[7]],
                tangent: if f.len() >= 12 { [f[8], f[9], f[10], f[11]] } else { [0.0; 4] },
            }
        })
        .collect();
    let indices: Vec<u32> = body[vbytes..].chunks_exact(4).map(bytemuck::pod_read_unaligned).collect();
    if indices.iter().any(|&i| i as usize >= vertices.len()) {
        bail!("mesh.bin has invalid indices");
    }
    let mut mesh = Mesh { name: String::new(), vertices, indices, parts: Vec::new() };
    if format < 2 {
        mesh.compute_tangents();
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use s3dp_core::OrbitCamera;
    use s3dp_render::{Brush, ShadingMode};

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    fn finals(vp: &Viewport, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<Vec<f32>> {
        vp.paint.doc.channels.iter().map(|c| vp.paint.read_final(device, queue, 0, c).unwrap().1).collect()
    }

    /// Spec §13.1: save, close, open -> identical result.
    #[test]
    fn save_and_load_roundtrip() {
        let Some((device, queue)) = device() else { return };
        let mut mesh = Mesh::uv_sphere(32, 16);
        mesh.name = "ball".into();
        let mut vp = Viewport::new(&device);
        vp.paint.throttle = false;
        vp.set_mesh(&device, &queue, &mesh, 128);
        let mut cam = OrbitCamera::default();
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        let view = [128, 128];
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        let mut brush = Brush::default();
        brush.channels = [ChannelId::BaseColor, ChannelId::Roughness].into_iter().collect();
        brush.values.set(ChannelId::Roughness, [0.1; 3]);
        vp.begin_stroke(&device, &queue, brush.clone()).unwrap();
        vp.paint.add_dabs(&[[64.0, 64.0], [80.0, 60.0]]);
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        vp.paint.end_stroke();
        // Folder with a fill + painted mask, and a blur on the folder.
        let folder = vp.paint.add_node(NodeKind::Folder { expanded: true }, |_, _| {});
        let mut fill = ChannelValues::defaults();
        fill.set(ChannelId::BaseColor, [0.6, 0.1, 0.6]);
        let fill_id = vp.paint.add_node(NodeKind::Fill(fill), |n, _| n.blending.default.mode = BlendMode::Overlay);
        vp.paint.add_mask(fill_id, false);
        let mut white = brush.clone();
        white.values.set(ChannelId::BaseColor, [1.0; 3]);
        vp.begin_stroke(&device, &queue, white).unwrap();
        vp.paint.add_dabs(&[[56.0, 70.0]]);
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        vp.paint.end_stroke();
        let blur = vp.paint.new_effect("blur").unwrap();
        vp.paint.edit_node(folder, true, |n| n.effects.push(blur));
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        let before = finals(&vp, &device, &queue);

        let dir = std::env::temp_dir().join(format!("s3dp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ball.s3dp");
        save(&path, &mesh, &vp, None, &device, &queue).unwrap();

        let mut vp2 = Viewport::new(&device);
        vp2.paint.throttle = false;
        let (loaded, _) = load(&path, &mut vp2, &device, &queue).unwrap();
        vp2.render(&device, &queue, view, &cam, ShadingMode::Lit);
        assert_eq!(loaded.indices, mesh.indices);
        assert_eq!(vp2.paint.doc, vp.paint.doc);
        assert_eq!(before, finals(&vp2, &device, &queue), "composited maps must match after reload");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Writes a small demo project (fill with content + mask effects, a folder) to `$S3DP_DEMO`
    /// for UI screenshots. `cargo test -p stylized3dpaint demo_project -- --ignored`.
    #[test]
    #[ignore]
    fn demo_project() {
        let Some(path) = std::env::var_os("S3DP_DEMO").map(std::path::PathBuf::from) else { return };
        let Some((device, queue)) = device() else { return };
        let mesh = Mesh::uv_sphere(48, 24);
        let mut vp = Viewport::new(&device);
        vp.paint.throttle = false;
        vp.set_mesh(&device, &queue, &mesh, 256);
        let mut v = ChannelValues::defaults();
        v.set(ChannelId::BaseColor, [0.7, 0.2, 0.1]);
        let fill = vp.paint.add_node(NodeKind::Fill(v), |_, _| {});
        for id in ["hue_shift_by_light", "edge_highlight"] {
            let e = vp.paint.new_effect(id).unwrap();
            vp.paint.edit_node(fill, false, |n| n.effects.push(e));
        }
        vp.paint.add_mask(fill, true);
        let e = vp.paint.new_effect("noise").unwrap();
        let item = vp.paint.new_id();
        vp.paint.edit_node(fill, false, |n| {
            n.mask.as_mut().unwrap().items.push(s3dp_core::MaskItem { id: item, enabled: true, opacity: 1.0, blend: BlendMode::Multiply, kind: s3dp_core::MaskItemKind::Effect(e) })
        });
        let folder = vp.paint.add_node(NodeKind::Folder { expanded: true }, |_, _| {});
        let mut v2 = ChannelValues::defaults();
        v2.set(ChannelId::BaseColor, [0.2, 0.4, 0.8]);
        let inner = vp.paint.add_node(NodeKind::Fill(v2), |_, _| {});
        vp.paint.move_node(inner, Some(folder), 0);
        let e = vp.paint.new_effect("posterize").unwrap();
        vp.paint.edit_node(inner, false, |n| n.effects.push(e));
        save(&path, &mesh, &vp, None, &device, &queue).unwrap();
    }
}
