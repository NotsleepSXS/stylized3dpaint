//! `.s3dp` project files: a zip with the mesh, layer stack description and layer PNGs.
//!
//! ```text
//! project.json                      ProjectFile (format version, layers, defaults...)
//! mesh.bin                          vertices + indices (little-endian, see `write_mesh`)
//! layers/<id>/<set>/<channel>.png   RGBA (colors, premultiplied) or LA (scalars, premultiplied)
//! layers/<id>/<set>/mask.png        L
//! baked/<set>/<map>.png              L (scalars), RGBA (normal), RGBA16 (world normal, position)
//! ```

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use s3dp_core::{BakeSettings, BakedMap, Channel, ChannelValues, Layer, Mesh, MeshPart, Vertex};
use s3dp_render::paint::{MASK_FORMAT, channel_format};
use s3dp_render::{GpuLayer, LayerTextures, Viewport, gpu, wgpu};
use serde::{Deserialize, Serialize};

/// 1: 32-byte vertices without tangents. 2: tangents, baked mesh maps.
const FORMAT_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: u32,
    mesh_name: String,
    parts: Vec<PartFile>,
    texture_size: u32,
    defaults: ChannelValues,
    active_layer: usize,
    layers: Vec<Layer>,
    /// Settings of the last bake; present when baked maps are stored.
    #[serde(default)]
    bake: Option<BakeSettings>,
}

#[derive(Serialize, Deserialize)]
struct PartFile {
    name: String,
    first_index: u32,
    index_count: u32,
}

fn channel_file(c: Channel) -> &'static str {
    match c {
        Channel::BaseColor => "base_color.png",
        Channel::Roughness => "roughness.png",
        Channel::Metallic => "metallic.png",
        Channel::Height => "height.png",
        Channel::Emissive => "emissive.png",
    }
}

fn png_bytes(data: &[u8], size: u32, color: image::ExtendedColorType) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::ImageEncoder::write_image(image::codecs::png::PngEncoder::new(&mut out), data, size, size, color)?;
    Ok(out)
}

pub fn save(
    path: &Path,
    mesh: &Mesh,
    viewport: &Viewport,
    bake: Option<&BakeSettings>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<()> {
    let paint = &viewport.paint;
    let project = ProjectFile {
        format: FORMAT_VERSION,
        mesh_name: mesh.name.clone(),
        parts: mesh
            .parts
            .iter()
            .map(|p| PartFile { name: p.name.clone(), first_index: p.first_index, index_count: p.index_count })
            .collect(),
        texture_size: viewport.texture_size(),
        defaults: paint.defaults,
        active_layer: paint.active,
        layers: paint.layers.iter().map(|l| l.meta.clone()).collect(),
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

    for layer in &paint.layers {
        for (si, tex) in layer.sets.iter().enumerate() {
            let dir = format!("layers/{}/{si}", layer.meta.id);
            for c in Channel::ALL {
                let Some(t) = &tex.channels[c.index()] else { continue };
                let data = gpu::read_texture(device, queue, t).context("GPU readback failed")?;
                let kind = if c.is_color() { image::ExtendedColorType::Rgba8 } else { image::ExtendedColorType::La8 };
                zip.start_file(format!("{dir}/{}", channel_file(c)), stored)?;
                zip.write_all(&png_bytes(&data, t.width(), kind)?)?;
            }
            if let Some(m) = &tex.mask {
                let data = gpu::read_texture(device, queue, m).context("GPU readback failed")?;
                zip.start_file(format!("{dir}/mask.png"), stored)?;
                zip.write_all(&png_bytes(&data, m.width(), image::ExtendedColorType::L8)?)?;
            }
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
                        let words: Vec<u8> = halves
                            .iter()
                            .flat_map(|h| ((h.to_f32().clamp(0.0, 1.0) * 65535.0).round() as u16).to_ne_bytes())
                            .collect();
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

/// Loads a project: replaces the viewport mesh and layer stack.
/// Returns the mesh and the bake settings (when baked maps were restored).
pub fn load(path: &Path, viewport: &mut Viewport, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(Mesh, Option<BakeSettings>)> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a Stylized3DPaint project")?;
    let read = |zip: &mut zip::ZipArchive<std::fs::File>, name: &str| -> Result<Vec<u8>> {
        let mut f = zip.by_name(name).with_context(|| format!("missing {name}"))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(buf)
    };
    let project: ProjectFile = serde_json::from_slice(&read(&mut zip, "project.json")?)?;
    if project.format > FORMAT_VERSION {
        bail!("project was saved by a newer version (format {})", project.format);
    }
    let mut mesh = read_mesh(&read(&mut zip, "mesh.bin")?, project.format)?;
    mesh.name = project.mesh_name.clone();
    mesh.parts = project
        .parts
        .iter()
        .map(|p| MeshPart { name: p.name.clone(), first_index: p.first_index, index_count: p.index_count })
        .collect();
    viewport.set_mesh(device, queue, &mesh, project.texture_size);

    let size = project.texture_size;
    let mut layers = Vec::new();
    for meta in project.layers {
        let mut sets = Vec::new();
        for si in 0..mesh.parts.len() {
            let dir = format!("layers/{}/{si}", meta.id);
            let mut tex = LayerTextures::default();
            for c in Channel::ALL {
                let name = format!("{dir}/{}", channel_file(c));
                if zip.index_for_name(&name).is_none() {
                    continue;
                }
                let img = image::load_from_memory(&read(&mut zip, &name)?)?;
                let data = if c.is_color() { img.to_rgba8().into_raw() } else { img.to_luma_alpha8().into_raw() };
                tex.channels[c.index()] = Some(upload_checked(device, queue, size, channel_format(c), &data, &name)?);
            }
            let name = format!("{dir}/mask.png");
            if zip.index_for_name(&name).is_some() {
                let data = image::load_from_memory(&read(&mut zip, &name)?)?.to_luma8().into_raw();
                tex.mask = Some(upload_checked(device, queue, size, MASK_FORMAT, &data, &name)?);
            }
            sets.push(tex);
        }
        layers.push(GpuLayer { meta, sets });
    }
    viewport.paint.load_document(layers, project.defaults, project.active_layer);

    let mut bake = project.bake;
    if bake.is_some() {
        for si in 0..mesh.parts.len() {
            for map in BakedMap::ALL {
                let name = format!("baked/{si}/{}.png", map.file_name());
                if zip.index_for_name(&name).is_none() {
                    bake = None;
                    continue;
                }
                let img = image::load_from_memory(&read(&mut zip, &name)?)?;
                let t = &viewport.paint.sets[si].baked[map.index()];
                let data: Vec<u8> = match t.format() {
                    wgpu::TextureFormat::R8Unorm => img.to_luma8().into_raw(),
                    wgpu::TextureFormat::Rgba16Float => img
                        .to_rgba16()
                        .into_raw()
                        .iter()
                        .flat_map(|&w| half::f16::from_f32(w as f32 / 65535.0).to_le_bytes())
                        .collect(),
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
    Ok((mesh, bake))
}

fn upload_checked(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    format: wgpu::TextureFormat,
    data: &[u8],
    name: &str,
) -> Result<wgpu::Texture> {
    let bpp = format.block_copy_size(None).unwrap_or(4) as usize;
    if data.len() != size as usize * size as usize * bpp {
        bail!("{name} has the wrong size");
    }
    Ok(gpu::upload(device, queue, size, format, data))
}

/// `u32 vertex count, u32 index count, vertices (Vertex, 32 bytes each), indices (u32)`.
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
    use s3dp_core::{BlendMode, ChannelMask, LayerKind, OrbitCamera};
    use s3dp_render::{Brush, ShadingMode};

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    fn all_channels(vp: &Viewport, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<Vec<u8>> {
        Channel::ALL.iter().map(|&c| vp.paint.read_channel(device, queue, 0, c).unwrap().1).collect()
    }

    #[test]
    fn save_and_load_roundtrip() {
        let Some((device, queue)) = device() else { return };
        let mut mesh = Mesh::uv_sphere(32, 16);
        mesh.name = "ball".into();
        let mut vp = Viewport::new(&device);
        vp.set_mesh(&device, &queue, &mesh, 256);
        let mut cam = OrbitCamera::default();
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        let view = [256, 256];
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);

        let brush = Brush {
            values: ChannelValues { base_color: [0.1, 0.8, 0.3], roughness: 0.1, ..ChannelValues::DEFAULTS },
            channels: ChannelMask([true, true, false, false, false]),
            radius: 30.0,
            ..Default::default()
        };
        vp.begin_stroke(&device, &queue, brush).unwrap();
        vp.paint.add_dabs(&[[128.0, 128.0], [150.0, 120.0]]);
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        vp.paint.end_stroke();
        vp.paint.add_layer(LayerKind::Fill(ChannelValues { base_color: [0.9, 0.2, 0.9], ..ChannelValues::DEFAULTS }));
        let top = vp.paint.active;
        vp.paint.edit_layer(top, true, |l| {
            l.blend = BlendMode::Overlay;
            l.opacity = 0.6;
            l.name = "Overlay fill".into();
        });
        vp.paint.add_mask(&device, &queue, top, false);
        vp.begin_stroke(&device, &queue, Brush { values: ChannelValues { base_color: [1.0; 3], ..ChannelValues::DEFAULTS }, ..brush })
            .unwrap();
        vp.paint.add_dabs(&[[110.0, 140.0]]);
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        vp.paint.end_stroke();
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        let before = all_channels(&vp, &device, &queue);
        let mut bake = BakeSettings::preset(s3dp_core::BakeStyle::Stylized);
        bake.ao_samples = 8;
        bake.edge_samples = 8;
        bake.thickness_samples = 8;
        bake.sky_samples = 8;
        vp.bake_now(&device, &queue, &mesh, None, &bake);
        let baked_before: Vec<Vec<u8>> =
            vp.paint.sets[0].baked.iter().map(|t| gpu::read_texture(&device, &queue, t).unwrap()).collect();

        let dir = std::env::temp_dir().join(format!("s3dp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ball.s3dp");
        save(&path, &mesh, &vp, Some(&bake), &device, &queue).unwrap();

        let mut vp2 = Viewport::new(&device);
        let (loaded, loaded_bake) = load(&path, &mut vp2, &device, &queue).unwrap();
        assert_eq!(loaded_bake.as_ref(), Some(&bake));
        for (i, t) in vp2.paint.sets[0].baked.iter().enumerate() {
            let after = gpu::read_texture(&device, &queue, t).unwrap();
            if t.format() == wgpu::TextureFormat::Rgba16Float {
                let (a, b): (&[half::f16], &[half::f16]) = (bytemuck::cast_slice(&baked_before[i]), bytemuck::cast_slice(&after));
                let worst = a.iter().zip(b).map(|(x, y)| (x.to_f32() - y.to_f32()).abs()).fold(0.0, f32::max);
                assert!(worst < 1e-3, "{:?} differs by {worst}", BakedMap::ALL[i]);
            } else {
                assert!(after == baked_before[i], "{:?} differs after reload", BakedMap::ALL[i]);
            }
        }
        vp2.render(&device, &queue, view, &cam, ShadingMode::Lit);
        assert_eq!(loaded.indices, mesh.indices);
        assert_eq!(loaded.parts.len(), mesh.parts.len());
        let names: Vec<_> = vp2.paint.layers.iter().map(|l| l.meta.name.clone()).collect();
        assert_eq!(names, ["Layer 1", "Overlay fill"]);
        assert_eq!(vp2.paint.layers[1].meta.blend, BlendMode::Overlay);
        assert_eq!(before, all_channels(&vp2, &device, &queue), "composited maps must match after reload");
        std::fs::remove_dir_all(&dir).ok();
    }
}
