//! Headless GPU tests for mask generators, warp filters and the Stylize layer.

use s3dp_core::{
    BlendMode, Channel, ChannelValues, EffectKind, GeneratorSource, LayerKind, Levels, MaskBlend, MaskEffect, Mesh,
    NoiseKind, NoiseParams, OrbitCamera, StylizeParams, WarpMode, WarpParams,
};
use s3dp_render::{ShadingMode, Viewport, wgpu};

const TEX: u32 = 256;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

struct Scene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vp: Viewport,
}

impl Scene {
    /// Sphere with black defaults and one white fill layer whose mask we shape.
    fn new() -> Option<Self> {
        let (device, queue) = device()?;
        let mut vp = Viewport::new(&device);
        vp.paint.throttle_effects = false;
        vp.set_mesh(&device, &queue, &Mesh::uv_sphere(64, 32), TEX);
        vp.paint.set_defaults(ChannelValues { base_color: [0.0; 3], ..ChannelValues::DEFAULTS }, false);
        vp.paint.add_layer(LayerKind::Fill(ChannelValues { base_color: [1.0; 3], ..ChannelValues::DEFAULTS }));
        Some(Self { device, queue, vp })
    }

    fn set_effects(&mut self, effects: Vec<MaskEffect>) {
        let top = self.vp.paint.active;
        self.vp.paint.edit_layer(top, false, |l| l.effects = effects);
    }

    /// Final base color red channel (= mask value for the white-on-black setup).
    fn mask(&mut self) -> Vec<u8> {
        let mut cam = OrbitCamera::default();
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        self.vp.render(&self.device, &self.queue, [64, 64], &cam, ShadingMode::Unlit);
        let (_, rgb) = self.vp.paint.read_channel(&self.device, &self.queue, 0, Channel::BaseColor).unwrap();
        rgb.chunks_exact(3).map(|p| p[0]).collect()
    }

    fn save(&self, data: &[u8], name: &str) {
        let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        image::save_buffer(out, data, TEX, TEX, image::ColorType::L8).unwrap();
    }
}

fn at(m: &[u8], x: u32, y: u32) -> u8 {
    m[(y * TEX + x) as usize]
}

fn gradient_y() -> MaskEffect {
    MaskEffect::new(EffectKind::Generator {
        source: GeneratorSource::HeightGradient { axis: 1 },
        levels: Levels::default(),
        blend: MaskBlend::Replace,
        opacity: 1.0,
    })
}

/// Hard horizontal border at the sphere's equator.
fn hard_band() -> Vec<MaskEffect> {
    vec![
        gradient_y(),
        MaskEffect::new(EffectKind::Levels(Levels { low: 0.49, high: 0.51, ..Default::default() })),
    ]
}

#[test]
fn height_gradient_runs_bottom_to_top() {
    let Some(mut s) = Scene::new() else { return };
    s.set_effects(vec![gradient_y()]);
    let m = s.mask();
    // Sphere v = 0 is the top (y = +1).
    let (top, mid, bottom) = (at(&m, 40, 4), at(&m, 40, TEX / 2), at(&m, 40, TEX - 5));
    println!("gradient top {top} mid {mid} bottom {bottom}");
    assert!(top > 240 && bottom < 15 && (100..156).contains(&mid));
}

#[test]
fn noise_is_continuous_across_the_uv_seam() {
    let Some(mut s) = Scene::new() else { return };
    let noise = NoiseParams { scale: 0.3, ..Default::default() };
    s.set_effects(vec![MaskEffect::new(EffectKind::generator(GeneratorSource::Noise(noise)))]);
    let m = s.mask();
    s.save(&m, "fx_noise.png");
    let mut worst = 0u8;
    let mut spread = 0u8;
    for y in (32..TEX - 32).step_by(8) {
        // u = 0 and u = 1 are the same meridian.
        worst = worst.max(at(&m, 0, y).abs_diff(at(&m, TEX - 1, y)));
        spread = spread.max(at(&m, TEX / 2, y).abs_diff(at(&m, TEX / 4, y)));
    }
    println!("seam difference {worst}, pattern spread {spread}");
    assert!(spread > 40, "noise should vary");
    assert!(worst < 25, "noise must match across the seam");
}

/// Mean absolute deviation of the border row from its average, across columns.
fn border_wobble(m: &[u8]) -> f32 {
    let rows: Vec<f32> = (8..TEX - 8)
        .step_by(4)
        .map(|x| (0..TEX).find(|&y| at(m, x, y) < 128).unwrap_or(TEX) as f32)
        .collect();
    let mean = rows.iter().sum::<f32>() / rows.len() as f32;
    rows.iter().map(|r| (r - mean).abs()).sum::<f32>() / rows.len() as f32
}

#[test]
fn warp_tears_a_straight_border() {
    let Some(mut s) = Scene::new() else { return };
    s.set_effects(hard_band());
    let straight = s.mask();
    let mut effects = hard_band();
    effects.push(MaskEffect::new(EffectKind::Warp(WarpParams {
        intensity: 0.12,
        noise: NoiseParams { scale: 0.25, ..Default::default() },
        ..Default::default()
    })));
    s.set_effects(effects);
    let warped = s.mask();
    s.save(&warped, "fx_warp.png");
    let (a, b) = (border_wobble(&straight), border_wobble(&warped));
    println!("border wobble: straight {a}, warped {b}");
    assert!(a < 1.5, "levels give a straight border");
    assert!(b > a + 2.0, "warp makes the border wavy");
}

#[test]
fn slope_blur_smears_and_strokes_noise_works() {
    let Some(mut s) = Scene::new() else { return };
    let mut effects = hard_band();
    effects.push(MaskEffect::new(EffectKind::Warp(WarpParams {
        mode: WarpMode::SlopeBlur,
        intensity: 0.2,
        samples: 16,
        sharpness: 0.0,
        noise: NoiseParams { kind: NoiseKind::Strokes, scale: 0.2, ..Default::default() },
        ..Default::default()
    })));
    s.set_effects(effects);
    let m = s.mask();
    s.save(&m, "fx_slope_blur.png");
    // Smearing creates in-between values along the border.
    let mid = m.iter().filter(|&&v| (30..225).contains(&v)).count();
    println!("in-between texels: {mid}");
    assert!(mid > 500);
}

#[test]
fn stylize_posterizes_and_crisps_metal() {
    let Some((device, queue)) = device() else { return };
    let mut vp = Viewport::new(&device);
        vp.paint.throttle_effects = false;
    vp.set_mesh(&device, &queue, &Mesh::uv_sphere(64, 32), TEX);
    // Base: a smooth gradient in color and metallic.
    vp.paint.set_defaults(ChannelValues { base_color: [0.0; 3], metallic: 0.0, ..ChannelValues::DEFAULTS }, false);
    vp.paint.add_layer(LayerKind::Fill(ChannelValues { base_color: [1.0, 0.6, 0.3], metallic: 1.0, ..ChannelValues::DEFAULTS }));
    let fill = vp.paint.active;
    vp.paint.edit_layer(fill, false, |l| l.effects = vec![gradient_y()]);
    let render = |vp: &mut Viewport| {
        vp.render(&device, &queue, [64, 64], &OrbitCamera::default(), ShadingMode::Lit);
        let bc = vp.paint.read_channel(&device, &queue, 0, Channel::BaseColor).unwrap().1;
        let metal = vp.paint.read_channel(&device, &queue, 0, Channel::Metallic).unwrap().1;
        let column = |data: &[u8], stride: usize| -> Vec<u8> { (0..TEX).map(|y| data[(y * TEX + 40) as usize * stride]).collect() };
        (column(&bc, 3), column(&metal, 1))
    };
    let (smooth_color, smooth_metal) = render(&mut vp);
    vp.paint.add_layer(LayerKind::Stylize(StylizeParams {
        painterly: 0.0,
        color_bands: 3,
        band_strength: 1.0,
        temperature: 0.0,
        edge_highlight: 0.0,
        cavity_darken: 0.0,
        ..Default::default()
    }));
    let top = vp.paint.active;
    vp.paint.edit_layer(top, false, |l| l.blend = BlendMode::Normal);
    let (color, metal) = render(&mut vp);
    let distinct = |v: &[u8]| {
        let mut d: Vec<u8> = v[8..v.len() - 8].to_vec();
        d.sort();
        d.dedup_by(|a, b| a.abs_diff(*b) <= 2);
        d.len()
    };
    println!("distinct red values: {} -> {}", distinct(&smooth_color), distinct(&color));
    println!("distinct metal values: {} -> {}", distinct(&smooth_metal), distinct(&metal));
    assert!(distinct(&color) <= 4 && distinct(&smooth_color) > 20, "color is banded");
    assert!(distinct(&metal) <= 6, "metal is crisp");
}

/// Timing of the heaviest effect at 2K: `cargo test --release -p s3dp-render --test effects timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_slope_blur_2k() {
    let Some((device, queue)) = device() else { return };
    let mut vp = Viewport::new(&device);
        vp.paint.throttle_effects = false;
    vp.set_mesh(&device, &queue, &Mesh::uv_sphere(64, 32), 2048);
    vp.paint.add_layer(LayerKind::Fill(ChannelValues::DEFAULTS));
    let top = vp.paint.active;
    let mut effects = hard_band();
    let heavy = std::env::var("HEAVY").is_ok();
    effects.push(MaskEffect::new(EffectKind::Warp(WarpParams {
        mode: WarpMode::SlopeBlur,
        samples: if heavy { 48 } else { 16 },
        noise: NoiseParams { kind: NoiseKind::Strokes, octaves: if heavy { 8 } else { 4 }, ..Default::default() },
        ..Default::default()
    })));
    for i in 0..4 {
        let mut e = effects.clone();
        if let EffectKind::Warp(w) = &mut e[2].kind {
            w.intensity = 0.02 + i as f32 * 0.01;
        }
        vp.paint.edit_layer(top, false, |l| l.effects = e);
        let t = std::time::Instant::now();
        vp.render(&device, &queue, [64, 64], &OrbitCamera::default(), ShadingMode::Lit);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        println!("frame with recompute: {:?}", t.elapsed());
    }
}

/// Visual check of a stylized stack on the test model:
/// `cargo test --release -p s3dp-render --test effects stack -- --ignored`.
#[test]
#[ignore]
fn preview_stylized_stack() {
    use s3dp_core::{BakeSettings, BakeStyle, BakedMap, ChannelMask};
    let Some((device, queue)) = device() else { return };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/monkey_crate.glb");
    let mesh = s3dp_core::import::load(&path).unwrap();
    let mut vp = Viewport::new(&device);
        vp.paint.throttle_effects = false;
    vp.set_mesh(&device, &queue, &mesh, 1024);
    vp.bake_now(&device, &queue, &mesh, None, &BakeSettings::preset(BakeStyle::Stylized));
    let p = &mut vp.paint;
    p.set_defaults(ChannelValues { base_color: [0.45, 0.62, 0.35], ..ChannelValues::DEFAULTS }, false);
    let fill = |c: [f32; 3]| LayerKind::Fill(ChannelValues { base_color: c, ..ChannelValues::DEFAULTS });
    let generator = |m, low, high, blend| {
        MaskEffect::new(EffectKind::Generator { source: GeneratorSource::Map(m), levels: Levels { low, high, ..Default::default() }, blend, opacity: 1.0 })
    };
    p.add_layer_with(fill([0.3, 0.25, 0.5]), |l| {
        l.channels = ChannelMask::BASE_COLOR;
        l.blend = BlendMode::Multiply;
        l.opacity = 0.5;
        let mut g = MaskEffect::new(EffectKind::generator(GeneratorSource::HeightGradient { axis: 1 }));
        if let EffectKind::Generator { levels, blend, .. } = &mut g.kind {
            levels.invert = true;
            levels.high = 0.6;
            *blend = MaskBlend::Replace;
        }
        l.effects = vec![g];
    });
    p.add_layer_with(fill([0.18, 0.12, 0.08]), |l| {
        l.channels = ChannelMask::BASE_COLOR;
        l.blend = BlendMode::Multiply;
        l.opacity = 0.8;
        l.effects = vec![
            generator(BakedMap::Cavity, 0.05, 0.5, MaskBlend::Replace),
            MaskEffect::new(EffectKind::Warp(WarpParams { intensity: 0.01, ..Default::default() })),
        ];
    });
    p.add_layer_with(fill([1.0, 0.93, 0.75]), |l| {
        l.channels = ChannelMask::BASE_COLOR;
        l.opacity = 0.85;
        l.effects = vec![
            generator(BakedMap::Edges, 0.1, 0.5, MaskBlend::Replace),
            MaskEffect::new(EffectKind::Warp(WarpParams { intensity: 0.012, noise: NoiseParams { scale: 0.04, ..Default::default() }, ..Default::default() })),
            MaskEffect::new(EffectKind::Warp(WarpParams {
                mode: WarpMode::SlopeBlur,
                intensity: 0.02,
                samples: 10,
                sharpness: 0.6,
                noise: NoiseParams { kind: NoiseKind::Strokes, scale: 0.06, ..Default::default() },
                ..Default::default()
            })),
        ];
    });
    p.add_layer(LayerKind::Stylize(StylizeParams::default()));
    let mut cam = OrbitCamera { yaw: 0.5, pitch: 0.25, ..Default::default() };
    let b = mesh.bounds();
    cam.frame(b.center(), b.radius(), 1.5);
    for (name, mode) in [("lit", ShadingMode::Lit), ("toon", ShadingMode::Toon), ("unlit", ShadingMode::Unlit)] {
        vp.render(&device, &queue, [900, 600], &cam, mode);
        let (w, h, frame) = vp.read_color(&device, &queue).unwrap();
        let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stack_{name}.png"));
        image::save_buffer(&out, &frame, w, h, image::ColorType::Rgba8).unwrap();
    }
}
