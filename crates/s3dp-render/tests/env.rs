//! Image-based lighting in the viewport: environment upload, lighting direction, rotation,
//! background.

use std::sync::Arc;

use s3dp_core::{Mesh, OrbitCamera};
use s3dp_render::env::{Equirect, GpuEnv, prepare};
use s3dp_render::{ShadingMode, Viewport, wgpu};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = s3dp_render::gpu::new_instance();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn luma(px: &[u8], w: u32, x: u32, y: u32) -> f32 {
    let i = ((y * w + x) * 4) as usize;
    0.3 * px[i] as f32 + 0.59 * px[i + 1] as f32 + 0.11 * px[i + 2] as f32
}

#[test]
fn hdri_lights_the_model_and_rotates() {
    let Some((device, queue)) = device() else { return };
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &Mesh::uv_sphere(48, 24), 128);
    // Bright sky (+Y) and a bright spot on +X, dark ground.
    let (w, h) = (128usize, 64usize);
    let mut data = vec![[0.02f32; 3]; w * h];
    for y in 0..h / 2 {
        for x in 0..w {
            data[y * w + x] = [2.0, 2.0, 2.0];
        }
    }
    let env = Arc::new(GpuEnv::upload(&device, &queue, &prepare("test", &Equirect::new(w, h, data), 128)));
    vp.set_environment(&device, env);
    let mut cam = OrbitCamera { yaw: 0.0, pitch: 0.0, ..Default::default() };
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    let size = [96u32, 96];
    vp.render(&device, &queue, size, &cam, ShadingMode::Lit);
    let (_, _, px) = vp.read_color(&device, &queue).unwrap();
    let (top, bottom) = (luma(&px, 96, 48, 28), luma(&px, 96, 48, 68));
    println!("top {top} bottom {bottom}");
    assert!(top > bottom + 30.0, "the bright sky lights the top of the sphere");

    // Background on: the corners show the environment instead of the clear color.
    let corner_before = luma(&px, 96, 2, 2);
    vp.env_settings.background = true;
    vp.env_settings.blur = 0.0;
    vp.render(&device, &queue, size, &cam, ShadingMode::Lit);
    let (_, _, bg) = vp.read_color(&device, &queue).unwrap();
    println!("corner {corner_before} -> {}", luma(&bg, 96, 2, 2));
    assert!(luma(&bg, 96, 2, 2) > corner_before + 30.0, "top-left corner looks at the bright sky");
    assert!(luma(&bg, 96, 2, 93) < 40.0, "bottom corner looks at the dark ground");

    // Exposure brightens.
    vp.env_settings.background = false;
    vp.env_settings.exposure = 1.5;
    vp.render(&device, &queue, size, &cam, ShadingMode::Lit);
    let (_, _, ex) = vp.read_color(&device, &queue).unwrap();
    assert!(luma(&ex, 96, 48, 48) > luma(&px, 96, 48, 48) + 10.0, "exposure brightens");
}

#[test]
fn rotation_moves_a_side_light() {
    let Some((device, queue)) = device() else { return };
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &Mesh::uv_sphere(48, 24), 128);
    // Light from one side only: the u < 0.5 half (directions with x < 0) is bright.
    let (w, h) = (128usize, 64usize);
    let data: Vec<[f32; 3]> = (0..w * h).map(|i| if (i % w) < w / 2 { [3.0; 3] } else { [0.02; 3] }).collect();
    let env = Arc::new(GpuEnv::upload(&device, &queue, &prepare("side", &Equirect::new(w, h, data), 128)));
    vp.set_environment(&device, env);
    let mut cam = OrbitCamera { yaw: 0.0, pitch: 0.0, ..Default::default() };
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    let size = [96u32, 96];
    vp.render(&device, &queue, size, &cam, ShadingMode::Lit);
    let (_, _, a) = vp.read_color(&device, &queue).unwrap();
    vp.env_settings.rotation = std::f32::consts::PI;
    vp.render(&device, &queue, size, &cam, ShadingMode::Lit);
    let (_, _, b) = vp.read_color(&device, &queue).unwrap();
    let (al, ar) = (luma(&a, 96, 24, 48), luma(&a, 96, 72, 48));
    let (bl, br) = (luma(&b, 96, 24, 48), luma(&b, 96, 72, 48));
    println!("before L {al} R {ar} | rotated L {bl} R {br}");
    assert!((al - ar).abs() > 30.0, "one side is lit");
    assert!((al > ar) != (bl > br), "rotating by 180 degrees swaps the lit side");
}
