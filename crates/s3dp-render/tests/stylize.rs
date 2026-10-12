//! Stylization filter on the material channels (not only base color).

use s3dp_core::{BakeSettings, BakeStyle, ChannelId, ChannelValues, Mesh, NodeKind, OrbitCamera, ParamValue};
use s3dp_render::{ShadingMode, Viewport, wgpu};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
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
