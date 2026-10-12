//! Headless GPU tests: painting, seams, layers, masks and undo on a UV sphere.

use s3dp_core::{BlendMode, Channel, ChannelMask, ChannelValues, LayerKind, Mesh, OrbitCamera};
use s3dp_render::{Brush, EditTarget, ShadingMode, Viewport, wgpu};

const VIEW: [u32; 2] = [512, 512];
const TEX: u32 = 512;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

struct Scene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vp: Viewport,
    cam: OrbitCamera,
}

impl Scene {
    fn new() -> Option<Self> {
        let (device, queue) = device()?;
        let mut vp = Viewport::new(&device);
        vp.paint.throttle_effects = false;
        vp.set_mesh(&device, &queue, &Mesh::uv_sphere(64, 32), TEX);
        let mut cam = OrbitCamera::default();
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        let mut s = Self { device, queue, vp, cam };
        s.render();
        Some(s)
    }

    fn render(&mut self) {
        self.vp.render(&self.device, &self.queue, VIEW, &self.cam, ShadingMode::Unlit);
    }

    fn stroke(&mut self, brush: Brush, dabs: &[[f32; 2]]) {
        self.vp.begin_stroke(&self.device, &self.queue, brush).unwrap();
        self.vp.paint.add_dabs(dabs);
        self.render();
        self.vp.paint.end_stroke();
        self.render();
    }

    fn channel(&self, c: Channel) -> Vec<u8> {
        self.vp.paint.read_channel(&self.device, &self.queue, 0, c).unwrap().1
    }

    fn save_frame(&self, name: &str) {
        let (w, h, frame) = self.vp.read_color(&self.device, &self.queue).unwrap();
        let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        image::save_buffer(&out, &frame, w, h, image::ColorType::Rgba8).unwrap();
        println!("saved {}", out.display());
    }
}

/// Texel of the sphere point in direction `d` (matches `Mesh::uv_sphere`).
fn sphere_texel(d: glam::Vec3) -> usize {
    let d = d.normalize();
    let u = d.z.atan2(d.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    let v = d.y.acos() / std::f32::consts::PI;
    let x = ((u * TEX as f32) as u32).min(TEX - 1);
    let y = ((v * TEX as f32) as u32).min(TEX - 1);
    (y * TEX + x) as usize
}

fn rgb(data: &[u8], texel: usize) -> [u8; 3] {
    [data[texel * 3], data[texel * 3 + 1], data[texel * 3 + 2]]
}

fn red_brush() -> Brush {
    Brush {
        values: ChannelValues { base_color: [1.0, 0.0, 0.0], ..ChannelValues::DEFAULTS },
        radius: 40.0,
        hardness: 0.8,
        ..Default::default()
    }
}

const GRAY: [u8; 3] = [191, 191, 191];

/// Equality within one 8-bit step (0.5 rounds to 127 or 128).
fn assert_near(a: [u8; 3], b: [u8; 3], msg: &str) {
    assert!(a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 1), "{msg}: {a:?} vs {b:?}");
}

#[test]
fn dab_paints_visible_side_only_and_undoes() {
    let Some(mut s) = Scene::new() else { return };
    s.stroke(red_brush(), &[[256.0, 256.0]]);
    s.save_frame("paint_test.png");
    let bc = s.channel(Channel::BaseColor);
    let (front, back) = (sphere_texel(s.cam.eye()), sphere_texel(-s.cam.eye()));
    assert_eq!(rgb(&bc, front), [255, 0, 0], "front of the sphere should be red");
    assert_near(rgb(&bc, back), GRAY, "back of the sphere must keep the default color");

    s.vp.paint.undo();
    s.render();
    assert_near(rgb(&s.channel(Channel::BaseColor), front), GRAY, "undo should clear the stroke");
    s.vp.paint.redo();
    s.render();
    assert_eq!(rgb(&s.channel(Channel::BaseColor), front), [255, 0, 0], "redo should restore it");
}

/// A stroke across the sphere's UV seam (the +X meridian) must not leave a gap.
#[test]
fn stroke_across_uv_seam_has_no_gap() {
    let Some(mut s) = Scene::new() else { return };
    s.cam = OrbitCamera { yaw: std::f32::consts::FRAC_PI_2, pitch: 0.0, ..Default::default() };
    s.cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    s.render();
    let brush = Brush {
        values: ChannelValues { base_color: [0.0, 0.0, 1.0], ..ChannelValues::DEFAULTS },
        radius: 12.0,
        hardness: 0.9,
        ..Default::default()
    };
    let dabs: Vec<[f32; 2]> = (0..=60).map(|i| [136.0 + i as f32 * 4.0, 256.0]).collect();
    s.stroke(brush, &dabs);
    s.save_frame("seam_test.png");
    let (w, _, frame) = s.vp.read_color(&s.device, &s.queue).unwrap();
    for x in 140..372 {
        let p = &frame[((256 * w + x) * 4) as usize..][..3];
        assert!(p[2] > 200 && p[0] < 60, "gap in stroke at x={x}: {p:?}");
    }
}

#[test]
fn layers_blend_mask_and_undo() {
    let Some(mut s) = Scene::new() else { return };
    let front = sphere_texel(s.cam.eye());
    s.stroke(red_brush(), &[[256.0, 256.0]]);

    // A 50% gray multiply fill on top halves the red.
    s.vp.paint.add_layer(LayerKind::Fill(ChannelValues { base_color: [0.5; 3], ..ChannelValues::DEFAULTS }));
    let top = s.vp.paint.active;
    s.vp.paint.edit_layer(top, true, |l| {
        l.blend = BlendMode::Multiply;
        l.channels = ChannelMask::BASE_COLOR;
    });
    s.render();
    let c = rgb(&s.channel(Channel::BaseColor), front);
    assert!((126..=130).contains(&c[0]) && c[1] == 0, "multiply should give ~128,0,0, got {c:?}");

    // A black mask hides the fill again; painting the mask white brings it back.
    s.vp.paint.add_mask(&s.device, &s.queue, top, false);
    s.render();
    assert_eq!(rgb(&s.channel(Channel::BaseColor), front), [255, 0, 0], "black mask hides the layer");
    assert_eq!(s.vp.paint.target, EditTarget::Mask);
    let white = Brush { values: ChannelValues { base_color: [1.0; 3], ..ChannelValues::DEFAULTS }, ..red_brush() };
    s.stroke(white, &[[256.0, 256.0]]);
    let c = rgb(&s.channel(Channel::BaseColor), front);
    assert!((126..=130).contains(&c[0]), "painted mask reveals the layer, got {c:?}");

    // Deleting the layer and undoing restores everything.
    s.vp.paint.delete_layer(top);
    s.render();
    assert_eq!(rgb(&s.channel(Channel::BaseColor), front), [255, 0, 0]);
    s.vp.paint.undo();
    s.render();
    let c = rgb(&s.channel(Channel::BaseColor), front);
    assert!((126..=130).contains(&c[0]), "undo brings the layer back, got {c:?}");
}

#[test]
fn brush_paints_only_enabled_channels() {
    let Some(mut s) = Scene::new() else { return };
    let front = sphere_texel(s.cam.eye());
    let brush = Brush {
        values: ChannelValues { roughness: 0.0, ..ChannelValues::DEFAULTS },
        channels: ChannelMask([false, true, false, false, false]),
        ..red_brush()
    };
    s.stroke(brush, &[[256.0, 256.0]]);
    assert_eq!(s.channel(Channel::Roughness)[front], 0, "roughness painted to 0");
    assert_near(rgb(&s.channel(Channel::BaseColor), front), GRAY, "base color untouched");
    assert!(s.channel(Channel::Roughness)[sphere_texel(-s.cam.eye())].abs_diff(179) <= 1, "default roughness 0.7");
}

#[test]
fn eyedropper_picks_painted_material() {
    let Some(mut s) = Scene::new() else { return };
    let brush = Brush {
        values: ChannelValues { base_color: [1.0, 0.0, 0.0], roughness: 0.2, ..ChannelValues::DEFAULTS },
        channels: ChannelMask([true, true, false, false, false]),
        ..red_brush()
    };
    s.stroke(brush, &[[256.0, 256.0]]);
    let (set, uv) = s.vp.pick(&s.device, &s.queue, [256, 256]).expect("sphere under the center pixel");
    assert_eq!(set, 0);
    let v = s.vp.paint.sample(&s.device, &s.queue, set, uv).unwrap();
    assert_eq!(v.base_color, [1.0, 0.0, 0.0]);
    assert!((v.roughness - 0.2).abs() < 0.01, "roughness {}", v.roughness);
    assert!(s.vp.pick(&s.device, &s.queue, [2, 2]).is_none(), "background has no surface");
}
