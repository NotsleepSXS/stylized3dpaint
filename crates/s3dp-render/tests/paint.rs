//! Headless GPU test: paint a dab on a sphere and check where the paint lands.

use s3dp_core::{Mesh, OrbitCamera};
use s3dp_render::{Brush, ShadingMode, Viewport, wgpu};

const VIEW: [u32; 2] = [512, 512];
const TEX: u32 = 512;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// UV of the unit sphere point in direction `d` (matches `Mesh::uv_sphere`).
fn sphere_uv(d: glam::Vec3) -> (u32, u32) {
    let d = d.normalize();
    let u = d.z.atan2(d.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    let v = d.y.acos() / std::f32::consts::PI;
    (((u * TEX as f32) as u32).min(TEX - 1), ((v * TEX as f32) as u32).min(TEX - 1))
}

fn alpha_at(data: &[u8], (x, y): (u32, u32)) -> u8 {
    data[((y * TEX + x) * 4 + 3) as usize]
}

fn painted_texels(data: &[u8]) -> usize {
    data.chunks_exact(4).filter(|p| p[3] > 0).count()
}

#[test]
fn dab_paints_visible_side_only_and_undoes() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mesh = Mesh::uv_sphere(64, 32);
    let mut vp = Viewport::new(&device);
    vp.set_mesh(&device, &queue, &mesh, TEX);
    let mut cam = OrbitCamera::default();
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);

    vp.render(&device, &queue, VIEW, &cam, ShadingMode::Lit);
    let brush = Brush { color: [1.0, 0.0, 0.0], radius: 40.0, hardness: 0.8, ..Default::default() };
    vp.begin_stroke(&device, &queue, brush);
    vp.paint.add_dabs(&[[256.0, 256.0]]);
    vp.render(&device, &queue, VIEW, &cam, ShadingMode::Lit);
    vp.paint.end_stroke();

    let (w, h, frame) = vp.read_color(&device, &queue).unwrap();
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("paint_test.png");
    image::save_buffer(&out, &frame, w, h, image::ColorType::Rgba8).unwrap();
    println!("viewport saved to {}", out.display());

    let (_, tex) = vp.paint.read_display(&device, &queue, 0).unwrap();
    let front = sphere_uv(cam.eye());
    let back = sphere_uv(-cam.eye());
    let painted = painted_texels(&tex);
    println!("painted texels: {painted}, front alpha {}, back alpha {}", alpha_at(&tex, front), alpha_at(&tex, back));
    assert!(alpha_at(&tex, front) > 200, "front of the sphere should be painted");
    assert_eq!(alpha_at(&tex, back), 0, "back of the sphere must stay clean");
    let px = &tex[((front.1 * TEX + front.0) * 4) as usize..][..3];
    assert!(px[0] > 240 && px[1] < 15 && px[2] < 15, "paint color should be red, got {px:?}");
    // Center pixel of the viewport shows red paint.
    let c = &frame[((256 * w + 256) * 4) as usize..][..3];
    assert!(c[0] > 2 * c[1] && c[0] > 2 * c[2], "viewport should show red, got {c:?}");

    vp.paint.undo(&device, &queue);
    let (_, tex) = vp.paint.read_display(&device, &queue, 0).unwrap();
    assert_eq!(painted_texels(&tex), 0, "undo should clear the stroke");

    vp.paint.redo(&device, &queue);
    let (_, tex) = vp.paint.read_display(&device, &queue, 0).unwrap();
    assert_eq!(painted_texels(&tex), painted, "redo should restore the stroke");
}

/// A stroke across the sphere's UV seam (the +X meridian) must not leave a gap.
#[test]
fn stroke_across_uv_seam_has_no_gap() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::uv_sphere(64, 32);
    let mut vp = Viewport::new(&device);
    vp.set_mesh(&device, &queue, &mesh, TEX);
    let mut cam = OrbitCamera { yaw: std::f32::consts::FRAC_PI_2, pitch: 0.0, ..Default::default() };
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);

    vp.render(&device, &queue, VIEW, &cam, ShadingMode::Unlit);
    vp.begin_stroke(&device, &queue, Brush { color: [0.0, 0.0, 1.0], radius: 12.0, hardness: 0.9, ..Default::default() });
    let dabs: Vec<[f32; 2]> = (0..=60).map(|i| [136.0 + i as f32 * 4.0, 256.0]).collect();
    vp.paint.add_dabs(&dabs);
    vp.render(&device, &queue, VIEW, &cam, ShadingMode::Unlit);
    vp.paint.end_stroke();
    vp.render(&device, &queue, VIEW, &cam, ShadingMode::Unlit);

    let (w, h, frame) = vp.read_color(&device, &queue).unwrap();
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("seam_test.png");
    image::save_buffer(&out, &frame, w, h, image::ColorType::Rgba8).unwrap();
    // Every pixel along the stroke centerline must be blue.
    for x in 140..372 {
        let p = &frame[((256 * w + x) * 4) as usize..][..3];
        assert!(p[2] > 200 && p[0] < 60, "gap in stroke at x={x}: {p:?}");
    }
}
