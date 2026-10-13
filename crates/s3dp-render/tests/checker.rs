//! UV Checker: the 512 map covers 512 texels, so bigger textures show more tiles.

use s3dp_core::{Mesh, OrbitCamera};
use s3dp_render::{ShadingMode, Viewport, wgpu};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = s3dp_render::gpu::new_instance();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// Orange <-> grey changes along the middle row of the frame.
fn transitions(device: &wgpu::Device, queue: &wgpu::Queue, texture_size: u32) -> usize {
    let mesh = Mesh::cube();
    let mut vp = Viewport::new(device);
    vp.set_mesh(device, queue, &mesh, texture_size);
    let mut cam = OrbitCamera { yaw: 0.0, pitch: 0.0, ..Default::default() };
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    vp.render(device, queue, [512, 512], &cam, ShadingMode::UvChecker);
    let (w, h, frame) = vp.read_color(device, queue).unwrap();
    if let Ok(dir) = std::env::var("DUMP") {
        image::save_buffer(std::path::Path::new(&dir).join(format!("checker_{texture_size}.png")), &frame, w, h, image::ColorType::Rgba8).unwrap();
    }
    let row = (h / 2) as usize * w as usize * 4;
    let orange = |x: usize| {
        let p = &frame[row + x * 4..row + x * 4 + 4];
        p[0] as i32 - p[2] as i32 > 60
    };
    (1..w as usize).filter(|&x| orange(x) != orange(x - 1)).count()
}

#[test]
fn uv_checker_tiles_with_texture_size() {
    let Some((device, queue)) = device() else { return };
    let small = transitions(&device, &queue, 512);
    let big = transitions(&device, &queue, 2048);
    println!("transitions: 512 -> {small}, 2048 -> {big}");
    assert!(small >= 2, "the checker is visible");
    assert!(big >= small * 3, "2048 tiles the 512 checker 4x ({small} vs {big})");
}
