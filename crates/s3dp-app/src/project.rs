//! `.s3dp` project files: a zip with the mesh, layer stack description and layer PNGs.
//!
//! ```text
//! project.json                      ProjectFile (format version, layers, defaults...)
//! mesh.bin                          vertices + indices (little-endian, see `write_mesh`)
//! layers/<id>/<set>/<channel>.png   RGBA (colors, premultiplied) or LA (scalars, premultiplied)
//! layers/<id>/<set>/mask.png        L
//! ```

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use s3dp_core::{Channel, ChannelValues, Layer, Mesh, MeshPart, Vertex};
use s3dp_render::paint::{MASK_FORMAT, channel_format};
use s3dp_render::{GpuLayer, LayerTextures, Viewport, gpu, wgpu};
use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: u32,
    mesh_name: String,
    parts: Vec<PartFile>,
    texture_size: u32,
    defaults: ChannelValues,
    active_layer: usize,
    layers: Vec<Layer>,
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

pub fn save(path: &Path, mesh: &Mesh, viewport: &Viewport, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<()> {
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
    zip.finish()?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Loads a project: replaces the viewport mesh and layer stack. Returns the mesh.
pub fn load(path: &Path, viewport: &mut Viewport, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Mesh> {
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
    let mut mesh = read_mesh(&read(&mut zip, "mesh.bin")?)?;
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
    Ok(mesh)
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

fn read_mesh(bytes: &[u8]) -> Result<Mesh> {
    let header = bytes.get(..8).context("mesh.bin is truncated")?;
    let vcount = u32::from_le_bytes(header[..4].try_into()?) as usize;
    let icount = u32::from_le_bytes(header[4..8].try_into()?) as usize;
    let vbytes = vcount * std::mem::size_of::<Vertex>();
    let body = &bytes[8..];
    if body.len() != vbytes + icount * 4 {
        bail!("mesh.bin is corrupt");
    }
    // Copy out: the zip buffer is not guaranteed to be aligned for `Vertex`.
    let vertices: Vec<Vertex> = body[..vbytes].chunks_exact(std::mem::size_of::<Vertex>()).map(bytemuck::pod_read_unaligned).collect();
    let indices: Vec<u32> = body[vbytes..].chunks_exact(4).map(bytemuck::pod_read_unaligned).collect();
    if indices.iter().any(|&i| i as usize >= vertices.len()) {
        bail!("mesh.bin has invalid indices");
    }
    Ok(Mesh { name: String::new(), vertices, indices, parts: Vec::new() })
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

        let dir = std::env::temp_dir().join(format!("s3dp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ball.s3dp");
        save(&path, &mesh, &vp, &device, &queue).unwrap();

        let mut vp2 = Viewport::new(&device);
        let loaded = load(&path, &mut vp2, &device, &queue).unwrap();
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
