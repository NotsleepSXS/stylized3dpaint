//! Properties preview: material / mask, 3D or flat UV, in its own target.

use s3dp_core::{BlendMode, ChannelId, ChannelValues, Mesh, NodeKind, OrbitCamera};
use s3dp_render::{Preview, ShadingMode, Viewport, wgpu};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = s3dp_render::gpu::new_instance();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// Share of pixels that differ from the background color `bg`.
fn coverage(px: &[u8], bg: &[u8]) -> f32 {
    let n = px.chunks_exact(4).filter(|p| p.iter().zip(bg).take(3).any(|(a, b)| a.abs_diff(*b) > 6)).count();
    n as f32 / (px.len() / 4) as f32
}

#[test]
fn preview_material_mask_and_uv() {
    let Some((device, queue)) = device() else { return };
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &Mesh::uv_sphere(32, 16), 128);
    let mut red = ChannelValues::defaults();
    red.set(ChannelId::BaseColor, [0.9, 0.05, 0.05]);
    let fill = vp.paint.add_node(NodeKind::Fill(red), |n, _| n.blending.default.mode = BlendMode::Normal);
    vp.paint.add_mask(fill, false);
    let mut cam = OrbitCamera::default();
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    vp.render(&device, &queue, [64, 64], &cam, ShadingMode::Lit);
    let main_before = vp.read_color(&device, &queue).unwrap().2;

    // 3D material: a red-ish disc in the middle.
    vp.render_preview(&device, &queue, 0, [96, 64], &cam, Preview { mask_of: None, uv: false, set: 0, shading: None, overlay: false }).unwrap();
    let (w, h, px) = vp.read_preview(&device, &queue, 0).unwrap();
    assert_eq!((w, h), (96, 64));
    let c = &px[((h / 2 * w + w / 2) * 4) as usize..][..4];
    let bg = px[..4].to_vec();
    println!("3D centre {c:?}, coverage {:.2}", coverage(&px, &bg));
    assert!(coverage(&px, &bg) > 0.15 && coverage(&px, &bg) < 0.9);
    // 3D mask (black mask): the sphere shows dark.
    vp.render_preview(&device, &queue, 0, [96, 64], &cam, Preview { mask_of: Some(fill), uv: false, set: 0, shading: None, overlay: false }).unwrap();
    let mpx = vp.read_preview(&device, &queue, 0).unwrap().2;
    let m = &mpx[((h / 2 * w + w / 2) * 4) as usize..][..4];
    println!("mask centre {m:?}");
    assert!(m[0] < c[0], "black mask is darker than the material");
    // UV: the sphere's UV layout fills (almost) the whole square.
    vp.render_preview(&device, &queue, 0, [64, 64], &cam, Preview { mask_of: None, uv: true, set: 0, shading: None, overlay: false }).unwrap();
    let upx = vp.read_preview(&device, &queue, 0).unwrap().2;
    println!("uv coverage {:.2}", coverage(&upx, &bg));
    assert!(coverage(&upx, &bg) > 0.8, "flat UV view covers the texture");
    // The main view is untouched.
    assert_eq!(vp.read_color(&device, &queue).unwrap().2, main_before);
}
