//! Small wgpu helpers shared by the renderer.

use egui_wgpu::wgpu;

pub fn texture(
    device: &wgpu::Device,
    label: &str,
    size: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
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

/// Non-square 2D texture.
pub fn texture2d(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// Usage for textures that are painted, composited from and copied.
pub const LAYER_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

pub fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&Default::default())
}

pub fn pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'a> {
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

pub fn clear(encoder: &mut wgpu::CommandEncoder, t: &wgpu::Texture, color: wgpu::Color) {
    let v = view(t);
    pass(encoder, &v, wgpu::LoadOp::Clear(color));
}

pub fn copy_full(encoder: &mut wgpu::CommandEncoder, src: &wgpu::Texture, dst: &wgpu::Texture) {
    encoder.copy_texture_to_texture(src.as_image_copy(), dst.as_image_copy(), src.size());
}

/// Copy of `src` in a fresh texture with the same format and size.
pub fn duplicate(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, src: &wgpu::Texture) -> wgpu::Texture {
    let t = texture(device, "layer channel", src.width(), src.format(), LAYER_USAGE);
    copy_full(encoder, src, &t);
    t
}

/// Blocking readback into tightly packed rows (8-bit formats only).
pub fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Option<Vec<u8>> {
    let size = texture.size();
    let bpp = texture.format().block_copy_size(None)?;
    let row = size.width * bpp;
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

/// Reads one texel (up to 4 bytes) of an 8-bit or float texture; float formats are not converted.
pub fn read_texel(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, x: u32, y: u32) -> Option<[u8; 4]> {
    let bpp = texture.format().block_copy_size(None)? as usize;
    let mut out = [0u8; 4];
    let bytes = read_region(device, queue, texture, x, y)?;
    out[..bpp.min(4)].copy_from_slice(&bytes[..bpp.min(4)]);
    Some(out)
}

/// Reads the raw bytes of a single texel.
pub fn read_region(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, x: u32, y: u32) -> Option<Vec<u8>> {
    let bpp = texture.format().block_copy_size(None)?;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("texel readback"),
        size: wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let mapped = buffer.get_mapped_range(..).ok()?;
    Some(mapped[..bpp as usize].to_vec())
}

/// Uploads tightly packed 8-bit texel rows into a new texture.
pub fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    format: wgpu::TextureFormat,
    data: &[u8],
) -> wgpu::Texture {
    let t = texture(device, "layer channel", size, format, LAYER_USAGE);
    let bpp = format.block_copy_size(None).unwrap_or(4);
    queue.write_texture(
        t.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * bpp), rows_per_image: None },
        t.size(),
    );
    t
}

/// Fullscreen-triangle pipeline with an auto layout.
pub fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    fs: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
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
}

pub fn tex_entry(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(v) }
}
