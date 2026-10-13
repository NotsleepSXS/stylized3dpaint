//! Stylization filter on the material channels (not only base color).

use s3dp_core::{BakeSettings, BakeStyle, ChannelId, ChannelValues, Mesh, NodeKind, OrbitCamera, ParamValue};
use s3dp_render::{ShadingMode, Viewport, wgpu};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = s3dp_render::gpu::new_instance();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn spread(v: &[f32]) -> (f32, f32) {
    let px: Vec<f32> = v.chunks_exact(4).filter(|p| p[3] > 0.5).map(|p| p[0]).collect();
    (px.iter().cloned().fold(f32::MAX, f32::min), px.iter().cloned().fold(f32::MIN, f32::max))
}

#[test]
fn stylization_shapes_roughness_metallic_height_ao_and_normals() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::cube();
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, 256);
    let settings = BakeSettings { ao_samples: 16, edge_samples: 12, thickness_samples: 8, sky_samples: 16, ..BakeSettings::preset(BakeStyle::Stylized) };
    vp.bake_now(&device, &queue, &mesh, None, &settings);
    let mut base = ChannelValues::defaults();
    base.set(ChannelId::Metallic, [0.3; 3]);
    let fill = vp.paint.add_node(NodeKind::Fill(base), |_, _| {});
    let mut e = vp.paint.new_effect("stylize_master").unwrap();
    for (k, v) in [("ec_metal_edge", 1.0), ("bl_ao", 1.0), ("ec_ao_cav", 1.0), ("mt_crisp", 0.0), ("br_height", 1.0)] {
        e.params.insert(k.into(), ParamValue::Float(v));
    }
    vp.paint.edit_node(fill, false, |n| n.effects.push(e));
    let mut cam = OrbitCamera::default();
    cam.frame(glam::Vec3::ZERO, 1.2, 1.0);
    vp.render(&device, &queue, [64, 64], &cam, ShadingMode::Lit);
    assert!(vp.paint.errors.is_empty(), "{:?}", vp.paint.errors);
    let read = |vp: &Viewport, c| vp.paint.read_final(&device, &queue, 0, c).unwrap().1;
    for (c, what) in [(ChannelId::Roughness, "roughness"), (ChannelId::Metallic, "metallic"), (ChannelId::Height, "height"), (ChannelId::AmbientOcclusion, "AO")] {
        let v = read(&vp, c);
        let (lo, hi) = spread(&v);
        println!("{what:10} {lo:.3} .. {hi:.3}");
        assert!(v.iter().all(|x| x.is_finite()), "{what}: non-finite");
        assert!(hi - lo > 0.05, "{what} is not shaped by the stylization ({lo} .. {hi})");
    }
    let n = read(&vp, ChannelId::Normal);
    let mut worst: f32 = 0.0;
    for p in n.chunks_exact(4).filter(|p| p[3] > 0.5) {
        assert!(p.iter().all(|x| x.is_finite()), "normal NaN");
        let l = ((p[0] * 2.0 - 1.0).powi(2) + (p[1] * 2.0 - 1.0).powi(2) + (p[2] * 2.0 - 1.0).powi(2)).sqrt();
        worst = worst.max((l - 1.0).abs());
    }
    println!("normal length error {worst:.4}");
    assert!(worst < 0.02, "normals stay unit length ({worst})");
    // Crisp metal snaps metallic to 0 / 1.
    vp.paint.edit_node(fill, false, |n| {
        n.effects[0].params.insert("mt_crisp".into(), ParamValue::Float(1.0));
    });
    vp.render(&device, &queue, [64, 64], &cam, ShadingMode::Lit);
    let m = read(&vp, ChannelId::Metallic);
    // Inside the UV islands only (the padding keeps the fill value).
    let cov = vp.paint.read_coverage(&device, &queue, 0).unwrap();
    let inside: Vec<f32> = m.chunks_exact(4).zip(&cov).filter(|(_, c)| **c).map(|(p, _)| p[0]).collect();
    let mid = inside.iter().filter(|v| **v > 0.1 && **v < 0.9).count();
    let all = inside.len();
    println!("crisp metal: in-between {mid} / {all}");
    assert!((mid as f32) < all as f32 * 0.05, "crisp metal leaves few half-metals");
}

/// Brush strokes must not leave 1-texel steps in Height (they show as jagged bump): a step
/// is a neighbour difference much larger than the differences on both sides of it (a steep
/// but smooth slope has similar consecutive differences).
#[test]
fn stroke_relief_has_no_steps() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::uv_sphere(64, 32);
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, 512);
    let settings = BakeSettings { ao_samples: 8, edge_samples: 8, thickness_samples: 8, sky_samples: 8, ..BakeSettings::preset(BakeStyle::Stylized) };
    vp.bake_now(&device, &queue, &mesh, None, &settings);
    let fill = vp.paint.add_node(NodeKind::Fill(ChannelValues::defaults()), |_, _| {});
    let mut e = vp.paint.new_effect("stylize_master").unwrap();
    e.params.insert("br_height".into(), ParamValue::Float(1.0));
    e.params.insert("ec_strength".into(), ParamValue::Float(0.0));
    // Strokes ~10+ texels wide (like 2K textures with the default scale).
    e.params.insert("br_scale".into(), ParamValue::Float(0.15));
    if let Ok(off) = std::env::var("OFF") {
        for k in off.split(',') {
            e.params.insert(k.into(), ParamValue::Float(0.0));
        }
    }
    vp.paint.edit_node(fill, false, |n| n.effects.push(e));
    let cam = OrbitCamera::default();
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Lit);
    let h = vp.paint.read_final(&device, &queue, 0, ChannelId::Height).unwrap().1;
    let cov = vp.paint.read_coverage(&device, &queue, 0).unwrap();
    let n = 512usize;
    let at = |x: usize, y: usize| h[(y * n + x) * 4];
    let (mut steps, mut total) = (0usize, 0usize);
    for y in 2..n - 2 {
        for x in 2..n - 3 {
            if !(x - 1..=x + 2).all(|xx| cov[y * n + xx]) {
                continue;
            }
            total += 1;
            let d = (at(x + 1, y) - at(x, y)).abs();
            let side = (at(x, y) - at(x - 1, y)).abs().max((at(x + 2, y) - at(x + 1, y)).abs());
            if d > 0.004 && d > 4.0 * side {
                steps += 1;
            }
        }
    }
    let range = { let (lo, hi) = spread(&h); hi - lo };
    if std::env::var("DUMP").is_ok() {
        let (lo, _) = spread(&h);
        let img: Vec<u8> = h.chunks_exact(4).map(|p| (((p[0] - lo) / range.max(1e-6)) * 255.0).clamp(0.0, 255.0) as u8).collect();
        image::save_buffer(std::env::temp_dir().join("s3dp_height.png"), &img, n as u32, n as u32, image::ColorType::L8).unwrap();
    }
    let share = steps as f32 / total.max(1) as f32;
    println!("height range {range:.4}, steps {steps} / {total} ({:.4} %)", share * 100.0);
    assert!(range > 0.01, "strokes make some relief");
    // The old hard-edged plates gave ~2.2 % here.
    assert!(share < 0.005, "1-texel steps in the stroke relief: {steps} ({:.3} %)", share * 100.0);
}
