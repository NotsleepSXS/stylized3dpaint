//! Stroke frame cost (not a check): `cargo test --release -p s3dp-render --test perf -- --ignored --nocapture`.

use s3dp_core::{ChannelId, ChannelValues, Mesh, NodeKind, OrbitCamera, ParamValue};
use s3dp_render::{Brush, ShadingMode, Viewport, wgpu};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn stroke_frames(device: &wgpu::Device, queue: &wgpu::Queue, fx: &[&str], seamless: bool, place: u8) -> f64 {
    let tex: u32 = std::env::var("TEX").ok().and_then(|v| v.parse().ok()).unwrap_or(2048);
    let mut vp = Viewport::new(device);
    vp.set_mesh(device, queue, &Mesh::uv_sphere(64, 32), tex);
    let mut cam = OrbitCamera::default();
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    let layer = vp.paint.active.unwrap();
    let fill = vp.paint.add_node(NodeKind::Fill(ChannelValues::defaults()), |n, _| n.blending.default.opacity = 0.5);
    for id in fx {
        let mut e = vp.paint.new_effect(id).unwrap();
        e.params.insert("seamless".into(), ParamValue::Bool(seamless));
        match place {
            // 0: on the fill above, 1: on the painted layer, 2: effect layer on top.
            0 => vp.paint.edit_node(fill, false, |n| n.effects.push(e)),
            1 => vp.paint.edit_node(layer, false, |n| n.effects.push(e)),
            _ => {
                vp.paint.add_node(NodeKind::Effect(e), |_, _| {});
            }
        }
    }
    vp.paint.active = Some(layer);
    let view = [1200, 800];
    vp.render(device, queue, view, &cam, ShadingMode::Lit);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let mut brush = Brush::default();
    brush.channels = [ChannelId::BaseColor].into_iter().collect();
    vp.begin_stroke(device, queue, brush).unwrap();
    let frames = 30;
    let t = std::time::Instant::now();
    for i in 0..frames {
        let x = 400.0 + i as f32 * 10.0;
        vp.paint.add_dabs(&[[x, 400.0], [x + 5.0, 402.0]]);
        vp.render(device, queue, view, &cam, ShadingMode::Lit);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }
    vp.paint.end_stroke();
    t.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

#[test]
#[ignore]
fn stroke_frame_cost() {
    let Some((device, queue)) = device() else { return };
    for place in [1u8, 2] {
        for (label, fx, seamless) in [("hsl", vec!["hsl"], true), ("blur UV", vec!["blur"], false), ("blur seamless", vec!["blur"], true), ("stylize UV", vec!["stylize_master"], false), ("stylize seamless", vec!["stylize_master"], true)] {
            let ms = stroke_frames(&device, &queue, &fx, seamless, place);
            let at = if place == 1 { "on painted layer" } else { "effect layer" };
            println!("{at:17} {label:18} {ms:7.2} ms / stroke frame");
        }
    }
}

/// App-like loop: a frame every 16 ms without waiting for the GPU; reports how much GPU work
/// is still queued at the end (input lag) with and without the busy gate.
#[test]
#[ignore]
fn stroke_backlog() {
    let Some((device, queue)) = device() else { return };
    for gate in [false, true] {
        let mut vp = Viewport::new(&device);
        vp.paint.throttle = gate;
        vp.set_mesh(&device, &queue, &Mesh::uv_sphere(64, 32), 2048);
        let mut cam = OrbitCamera::default();
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        let layer = vp.paint.active.unwrap();
        let e = vp.paint.new_effect("stylize_master").unwrap();
        vp.paint.edit_node(layer, false, |n| n.effects.push(e));
        let view = [1200, 800];
        vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mut brush = Brush::default();
        brush.channels = [ChannelId::BaseColor].into_iter().collect();
        vp.begin_stroke(&device, &queue, brush).unwrap();
        let start = std::time::Instant::now();
        for i in 0..90 {
            let tick = start + std::time::Duration::from_millis(16 * i);
            if let Some(d) = tick.checked_duration_since(std::time::Instant::now()) {
                std::thread::sleep(d);
            }
            let x = 300.0 + i as f32 * 6.0;
            vp.paint.add_dabs(&[[x, 400.0]]);
            vp.render(&device, &queue, view, &cam, ShadingMode::Lit);
            let _ = device.poll(wgpu::PollType::Poll);
        }
        let t = std::time::Instant::now();
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        println!("gate {gate}: loop {:?}, GPU backlog at the end {:?}", start.elapsed(), t.elapsed());
        vp.paint.end_stroke();
    }
}
