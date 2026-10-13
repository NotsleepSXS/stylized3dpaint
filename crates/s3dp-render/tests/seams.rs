//! Seamless (3D) mode of neighbour-reading effects: across a UV seam both sides see the same
//! surface neighbourhood, so values match on the two sides; UV mode stops at the seam.

use s3dp_core::{ChannelId, ChannelValues, Mesh, NodeKind, OrbitCamera, ParamValue};
use s3dp_render::{ShadingMode, Viewport, gpu, wgpu};

const TEX: u32 = 256;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = s3dp_render::gpu::new_instance();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// Mean |a - b| of the base color over seam-linked texel pairs, for an effect stack on a fill.
fn seam_gap(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh, stack: &[(&str, Vec<(&str, ParamValue)>)]) -> (f32, usize) {
    let mut vp = Viewport::new(device);
    vp.paint.throttle = false;
    vp.set_mesh(device, queue, mesh, TEX);
    let mut cam = OrbitCamera::default();
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    let fill = vp.paint.add_node(NodeKind::Fill(ChannelValues::defaults()), |_, _| {});
    for (id, params) in stack {
        let mut e = vp.paint.new_effect(id).unwrap();
        for (k, v) in params {
            assert!(e.params.contains_key(*k), "{id}: no param {k}");
            e.params.insert((*k).into(), v.clone());
        }
        vp.paint.edit_node(fill, false, |n| n.effects.push(e));
    }
    vp.render(device, queue, [64, 64], &cam, ShadingMode::Unlit);
    let errors: Vec<_> = vp.paint.errors.values().cloned().collect();
    assert!(errors.is_empty(), "{errors:?}");
    let bc = vp.paint.read_final(device, queue, 0, ChannelId::BaseColor).unwrap().1;
    let pos_bytes = gpu::read_texture(device, queue, &vp.paint.sets[0].geo_pos).unwrap();
    let pos: &[[f32; 4]] = bytemuck::cast_slice(&pos_bytes);
    let links = s3dp_core::seams::seam_links(pos, TEX as usize, TEX as usize);
    let (mut sum, mut n) = (0.0, 0);
    for (i, l) in links.iter().enumerate() {
        if l[0] < 0.0 {
            continue;
        }
        let j = l[1] as usize * TEX as usize + l[0] as usize;
        sum += (bc[i * 4] - bc[j * 4]).abs();
        n += 1;
    }
    (sum / n.max(1) as f32, n)
}

#[test]
fn blur_is_seamless_across_the_sphere_seam() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::uv_sphere(48, 24);
    let noise = ("noise", vec![("scale", ParamValue::Float(0.05))]);
    let blur = |seamless: bool| ("blur", vec![("intensity", ParamValue::Float(0.08)), ("seamless", ParamValue::Bool(seamless))]);
    let (raw, n) = seam_gap(&device, &queue, &mesh, &[noise.clone()]);
    let (uv, _) = seam_gap(&device, &queue, &mesh, &[noise.clone(), blur(false)]);
    let (seamless, _) = seam_gap(&device, &queue, &mesh, &[noise, blur(true)]);
    println!("seam pairs {n}: noise only {raw:.4} | blur UV {uv:.4} | blur seamless {seamless:.4}");
    assert!(n > 50, "the sphere has a UV seam");
    assert!(seamless < uv * 0.5, "seamless blur should close the seam: {seamless} vs UV {uv}");
}

#[test]
fn warp_and_slope_blur_are_seamless() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::uv_sphere(48, 24);
    let noise = ("noise", vec![("scale", ParamValue::Float(0.05))]);
    for id in ["warp", "slope_blur"] {
        let fx = |seamless: bool| (id, vec![("intensity", ParamValue::Float(0.06)), ("seamless", ParamValue::Bool(seamless))]);
        let (uv, _) = seam_gap(&device, &queue, &mesh, &[noise.clone(), fx(false)]);
        let (seamless, _) = seam_gap(&device, &queue, &mesh, &[noise.clone(), fx(true)]);
        println!("{id}: UV {uv:.4} | seamless {seamless:.4}");
        assert!(seamless < uv * 0.8, "{id}: seamless should reduce the seam: {seamless} vs UV {uv}");
    }
}

/// Every neighbour-reading effect exposes the "seamless" parameter, on by default.
#[test]
fn neighbour_effects_default_to_seamless() {
    let reg = s3dp_core::registry();
    let with: Vec<&str> = reg.effects.iter().filter(|d| d.reads_neighbors()).map(|d| d.id.as_str()).collect();
    println!("seamless-capable: {with:?}");
    for id in ["blur", "warp", "slope_blur", "directional_warp", "stylize_master", "metal_edge_wear", "curvature", "ambient_occlusion", "noise", "position_gradient"] {
        let d = reg.get(id).unwrap();
        assert!(d.reads_neighbors(), "{id}");
        let p = d.all_params().into_iter().find(|p| p.id == "seamless").unwrap_or_else(|| panic!("{id}: no seamless param"));
        assert_eq!(p.default_value(), ParamValue::Bool(true));
    }
}

/// Side-by-side renders of the sphere seam (UV vs seamless) for a visual check:
/// `cargo test -p s3dp-render --test seams seam_preview -- --ignored`.
#[test]
#[ignore]
fn seam_preview() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::uv_sphere(64, 32);
    let yaw: f32 = std::env::var("YAW").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let fx = std::env::var("FX").unwrap_or_else(|_| "slope_blur".into());
    let mut frames = Vec::new();
    for seamless in [false, true] {
        let mut vp = Viewport::new(&device);
        vp.paint.throttle = false;
        vp.set_mesh(&device, &queue, &mesh, 1024);
        let fill = vp.paint.add_node(NodeKind::Fill(ChannelValues::defaults()), |_, _| {});
        let mut n = vp.paint.new_effect("noise").unwrap();
        n.params.insert("scale".into(), ParamValue::Float(0.04));
        n.params.insert("grunge_amount".into(), ParamValue::Float(0.0));
        n.params.insert("g_contrast".into(), ParamValue::Float(0.7));
        let mut b = vp.paint.new_effect(&fx).unwrap();
        b.params.insert("intensity".into(), ParamValue::Float(0.08));
        b.params.insert("seamless".into(), ParamValue::Bool(seamless));
        vp.paint.edit_node(fill, false, |nd| {
            nd.effects.push(n);
            nd.effects.push(b);
        });
        let mut cam = OrbitCamera { yaw, pitch: 0.1, ..Default::default() };
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        vp.render(&device, &queue, [400, 400], &cam, ShadingMode::Unlit);
        vp.render(&device, &queue, [400, 400], &cam, ShadingMode::Unlit);
        frames.push(vp.read_color(&device, &queue).unwrap());
    }
    let (w, h) = (frames[0].0, frames[0].1);
    let mut out = vec![0u8; (w * 2 * h * 4) as usize];
    for (k, (_, _, f)) in frames.iter().enumerate() {
        for y in 0..h as usize {
            let row = &f[y * w as usize * 4..(y + 1) * w as usize * 4];
            let o = (y * w as usize * 2 + k * w as usize) * 4;
            out[o..o + row.len()].copy_from_slice(row);
        }
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("seam_preview.png");
    image::save_buffer(&path, &out, w * 2, h, image::ColorType::Rgba8).unwrap();
    println!("{}", path.display());
}
