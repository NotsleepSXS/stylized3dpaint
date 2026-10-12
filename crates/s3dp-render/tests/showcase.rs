//! Renders the README screenshots (`docs/images`): the test monkey with a stylized bake and a
//! small effect stack. Not a check; run with `cargo test -p s3dp-render --test showcase -- --ignored`.

use s3dp_core::{BakeSettings, BakeStyle, ChannelId, ChannelValues, NodeKind, OrbitCamera};
use s3dp_render::{ShadingMode, Viewport, wgpu};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

#[test]
#[ignore]
fn showcase() {
    let Some((device, queue)) = device() else { return };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/monkey_crate.glb");
    let mesh = s3dp_core::import::load(&path).unwrap();
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, 2048);
    vp.bake_now(&device, &queue, &mesh, None, &BakeSettings::preset(BakeStyle::Stylized));

    let mut base = ChannelValues::defaults();
    base.set(ChannelId::BaseColor, [0.42, 0.16, 0.07]);
    base.set(ChannelId::Roughness, [0.7; 3]);
    vp.paint.add_node(NodeKind::Fill(base), |n, _| n.name = "Base".into());
    for id in std::env::var("EFFECTS").unwrap_or_else(|_| "metal_edge_wear,edge_highlight,cavity_darken,hue_shift_by_light".into()).split(',') {
        let e = vp.paint.new_effect(id).unwrap_or_else(|| panic!("{id}"));
        vp.paint.add_node(NodeKind::Effect(e), |n, _| n.name = id.into());
    }

    let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/images");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut cam = OrbitCamera { yaw: 0.45, pitch: 0.18, ..Default::default() };
    let b = mesh.bounds();
    cam.frame(b.center(), b.radius(), 16.0 / 9.0);
    cam.distance *= 0.8;
    for (name, mode) in [("lit", ShadingMode::Lit), ("toon", ShadingMode::Toon), ("unlit", ShadingMode::Unlit)] {
        vp.render(&device, &queue, [1600, 900], &cam, mode);
        let (w, h, frame) = vp.read_color(&device, &queue).unwrap();
        image::save_buffer(out_dir.join(format!("showcase_{name}.png")), &frame, w, h, image::ColorType::Rgba8).unwrap();
    }
    for e in vp.paint.errors.values() {
        println!("effect error: {e}");
    }
}
