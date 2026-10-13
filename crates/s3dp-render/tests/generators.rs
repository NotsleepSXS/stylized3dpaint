//! Headless GPU tests for the mask generators (spec §3, §13.2): every generator compiles and
//! produces a sensible mask, 3D noise has no UV seam, seeds are deterministic and noise does
//! not depend on the texture resolution. Small textures / views to keep the GPU cool.

use glam::Vec3;
use s3dp_core::{BakeSettings, BlendMode, ChannelId, ChannelValues, Mask, MaskItem, MaskItemKind, Mesh, NodeKind, OrbitCamera, ParamValue};
use s3dp_render::{ShadingMode, Viewport, wgpu};

const VIEW: [u32; 2] = [128, 128];

const ALL: [&str; 11] = [
    "curvature",
    "ambient_occlusion",
    "light",
    "world_space_normal",
    "thickness",
    "dirt",
    "dust",
    "leaks",
    "uv_border",
    "mask_editor",
    "noise",
];

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = s3dp_render::gpu::new_instance();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

struct Scene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vp: Viewport,
    cam: OrbitCamera,
    tex: u32,
}

/// Mask values (linear, one per texel) plus the generator's compile/run error, if any.
struct Out {
    v: Vec<f32>,
    error: Option<String>,
}

impl Scene {
    fn new(mesh: &Mesh, tex: u32) -> Option<Self> {
        let (device, queue) = device()?;
        let mut vp = Viewport::new(&device);
        vp.paint.throttle = false;
        vp.set_mesh(&device, &queue, mesh, tex);
        let mut cam = OrbitCamera::default();
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        // Black base: a white fill masked by the generator then shows the mask itself.
        vp.paint.edit_document(false, |d| d.defaults.set(ChannelId::BaseColor, [0.0; 3]));
        let mut s = Self { device, queue, vp, cam, tex };
        s.render();
        Some(s)
    }

    fn render(&mut self) {
        self.vp.render(&self.device, &self.queue, VIEW, &self.cam, ShadingMode::Unlit);
    }

    /// Renders a white fill whose mask is the generator, returns the mask, removes the layer.
    fn mask(&mut self, def: &str, params: &[(&str, ParamValue)]) -> Out {
        let mut e = self.vp.paint.new_effect(def).unwrap_or_else(|| panic!("effect {def} not registered"));
        for (k, v) in params {
            assert!(e.params.contains_key(*k), "{def}: unknown param {k}");
            e.params.insert((*k).into(), v.clone());
        }
        let eid = e.id;
        let mut white = ChannelValues::defaults();
        white.set(ChannelId::BaseColor, [1.0; 3]);
        let node = self.vp.paint.add_node(NodeKind::Fill(white), |n, d| {
            n.mask = Some(Mask {
                enabled: true,
                items: vec![MaskItem { id: d.alloc_id(), enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(e) }],
            })
        });
        self.render();
        let data = self.vp.paint.read_final(&self.device, &self.queue, 0, ChannelId::BaseColor).unwrap().1;
        let error = self.vp.paint.errors.get(&eid).cloned();
        self.vp.paint.delete_node(node);
        Out { v: data.chunks_exact(4).map(|p| p[0]).collect(), error }
    }

    fn at(&self, v: &[f32], x: u32, y: u32) -> f32 {
        v[(y * self.tex + x) as usize]
    }
}

/// Sphere texel facing a world direction (uv_sphere layout: u = angle around Y, v from the top).
fn sphere_xy(d: Vec3, tex: u32) -> (u32, u32) {
    let d = d.normalize();
    let u = d.z.atan2(d.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    let v = d.y.acos() / std::f32::consts::PI;
    (((u * tex as f32) as u32).min(tex - 1), ((v * tex as f32) as u32).min(tex - 1))
}

struct Stats {
    min: f32,
    max: f32,
    mean: f32,
    std: f32,
}

fn stats(v: &[f32]) -> Stats {
    let n = v.len() as f32;
    let mean = v.iter().sum::<f32>() / n;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    Stats { min: v.iter().cloned().fold(f32::MAX, f32::min), max: v.iter().cloned().fold(f32::MIN, f32::max), mean, std: var.sqrt() }
}

fn check_ok(def: &str, o: &Out) -> Stats {
    assert!(o.error.is_none(), "{def}: {}", o.error.as_deref().unwrap_or(""));
    assert!(o.v.iter().all(|x| x.is_finite()), "{def}: non-finite values");
    let s = stats(&o.v);
    println!("{def:20} min {:.3} max {:.3} mean {:.3} std {:.3}", s.min, s.max, s.mean, s.std);
    assert!(s.min >= -0.002 && s.max <= 1.002, "{def}: out of 0..1 ({} .. {})", s.min, s.max);
    s
}

fn varies(def: &str, s: &Stats) {
    assert!(s.max - s.min > 0.2 && s.std > 0.02, "{def}: mask is (nearly) constant");
}

/// "Star" sphere: twelve sharp vertical ridges with rounded grooves between them, which gives
/// convex edges, cavities and occlusion to bake.
fn wavy_sphere() -> Mesh {
    let mut m = Mesh::uv_sphere(96, 48);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let phi = p.z.atan2(p.x);
        let r = 1.0 - 0.3 * (6.0 * phi).sin().abs() * (1.0 - p.y * p.y);
        v.position = (p * r).into();
    }
    m.compute_normals();
    m.compute_tangents();
    m
}

/// Every generator compiles and runs; position based ones vary on a plain (unbaked) sphere.
#[test]
fn generators_compile_and_vary_on_sphere() {
    let Some(mut s) = Scene::new(&Mesh::uv_sphere(48, 24), 256) else { return };
    let tex = s.tex;
    for def in ALL {
        let o = s.mask(def, &[]);
        let st = check_ok(def, &o);
        match def {
            // Mesh-map based: flat (but valid) without a bake.
            "curvature" | "ambient_occlusion" | "thickness" => {}
            _ => varies(def, &st),
        }
    }
    // Spot checks of the direction based generators.
    let top = sphere_xy(Vec3::Y, tex);
    let bottom = sphere_xy(-Vec3::Y, tex);
    let o = s.mask("world_space_normal", &[]);
    assert!(s.at(&o.v, top.0, top.1 + 4) > 0.95 && s.at(&o.v, bottom.0, bottom.1 - 4) < 0.05, "world space normal: up = white");
    let o = s.mask("dust", &[]);
    let side = sphere_xy(Vec3::X, tex);
    assert!(s.at(&o.v, side.0, side.1) < 0.05 && s.at(&o.v, bottom.0, bottom.1 - 4) < 0.05, "dust only on top");
    let o = s.mask("dirt", &[]);
    assert!(s.at(&o.v, bottom.0, bottom.1 - 4) > s.at(&o.v, top.0, top.1 + 4) + 0.2, "dirt gathers at the bottom");
    let o = s.mask("light", &[]);
    let l = Vec3::new(0.5, std::f32::consts::FRAC_1_SQRT_2, 0.5);
    let (lit, dark) = (sphere_xy(l, tex), sphere_xy(-l, tex));
    assert!(s.at(&o.v, lit.0, lit.1) > 0.8 && s.at(&o.v, dark.0, dark.1) < 0.15, "light: lit side bright, shadow side dark");
    let o = s.mask("uv_border", &[]);
    assert!(s.at(&o.v, 0, tex / 2) > 0.7 && s.at(&o.v, tex / 2, tex / 2) < 0.01, "uv border: white at the island edge only");
    // Leaks fall below their source (ledges at the top): lit under the source, never on the top cap.
    let o = s.mask("leaks", &[("amount", ParamValue::Float(1.0))]);
    let row_mean = |y: u32| (0..tex).map(|x| s.at(&o.v, x, y)).sum::<f32>() / tex as f32;
    let (cap, upper, lower) = (row_mean(tex / 16), row_mean(tex * 3 / 8), row_mean(tex * 7 / 8));
    println!("leaks rows: cap {cap:.3} upper {upper:.3} lower {lower:.3}");
    assert!(upper > 0.1 && upper > lower && cap < upper * 0.5, "leaks run down from upward surfaces");
    // Every noise type compiles and varies.
    for kind in 0..5 {
        let o = s.mask("noise", &[("kind", ParamValue::Enum(kind)), ("distortion", ParamValue::Float(0.5))]);
        varies(&format!("noise kind {kind}"), &check_ok(&format!("noise kind {kind}"), &o));
    }
    // Curvature modes / frequencies all compile (the wider ones take the mesh shape into account).
    for (mode, freq) in [(0, 0), (1, 3), (2, 6)] {
        let o = s.mask("curvature", &[("mode", ParamValue::Enum(mode)), ("frequency", ParamValue::Enum(freq))]);
        check_ok(&format!("curvature {mode}/{freq}"), &o);
    }
}

/// Map based generators vary on a baked mesh with ridges and grooves.
#[test]
fn map_generators_on_baked_mesh() {
    let mesh = wavy_sphere();
    let Some(mut s) = Scene::new(&mesh, 256) else { return };
    let settings = BakeSettings { ao_samples: 8, edge_samples: 8, thickness_samples: 8, sky_samples: 8, ..BakeSettings::default() };
    s.vp.bake_now(&s.device, &s.queue, &mesh, None, &settings);
    let cases: [(&str, &[(&str, ParamValue)]); 8] = [
        ("curvature", &[]),
        ("curvature", &[("mode", ParamValue::Enum(1)), ("frequency", ParamValue::Enum(3))]),
        ("curvature", &[("mode", ParamValue::Enum(2)), ("frequency", ParamValue::Enum(5))]),
        ("ambient_occlusion", &[("spread", ParamValue::Float(0.03))]),
        ("thickness", &[("contrast", ParamValue::Float(0.6))]),
        ("dirt", &[("top_down", ParamValue::Float(0.0))]),
        ("leaks", &[("source", ParamValue::Enum(0)), ("amount", ParamValue::Float(1.0))]),
        ("mask_editor", &[("pos_opacity", ParamValue::Float(0.0)), ("noise_opacity", ParamValue::Float(0.0)), ("ao_opacity", ParamValue::Float(1.0))]),
    ];
    let mut flat = Vec::new();
    for (def, params) in cases {
        let o = s.mask(def, params);
        let st = check_ok(def, &o);
        if !(st.max - st.min > 0.2 && st.std > 0.02) {
            flat.push(def);
        }
    }
    assert!(flat.is_empty(), "constant masks on the baked mesh: {flat:?}");
}

/// Spec §13.2: 3D generators continue across the sphere's UV seam (u = 0 / u = 1 meridian).
#[test]
fn noise_generators_have_no_uv_seam() {
    let Some(mut s) = Scene::new(&Mesh::uv_sphere(48, 24), 256) else { return };
    let tex = s.tex;
    let cases: [(&str, &[(&str, ParamValue)]); 6] = [
        ("noise", &[]),
        ("noise", &[("kind", ParamValue::Enum(1))]),
        ("noise", &[("kind", ParamValue::Enum(4)), ("distortion", ParamValue::Float(0.5))]),
        ("dirt", &[]),
        ("dust", &[("angle", ParamValue::Float(120.0))]),
        ("mask_editor", &[]),
    ];
    for (def, params) in cases {
        let o = s.mask(def, params);
        check_ok(def, &o);
        let rows = tex / 8..tex * 7 / 8;
        let n = rows.len() as f32;
        let (mut seam, mut inner, mut worst, mut worst_inner) = (0.0, 0.0, 0.0f32, 0.0f32);
        for y in rows {
            let d = (s.at(&o.v, 0, y) - s.at(&o.v, tex - 1, y)).abs();
            seam += d / n;
            worst = worst.max(d);
            let di = (s.at(&o.v, tex / 2, y) - s.at(&o.v, tex / 2 + 1, y)).abs();
            inner += di / n;
            worst_inner = worst_inner.max(di);
        }
        println!("{def:12} seam diff mean {seam:.4} max {worst:.4} (neighbours mean {inner:.4} max {worst_inner:.4})");
        assert!(seam <= inner * 2.0 + 0.01 && worst <= worst_inner * 2.0 + 0.02, "{def}: visible UV seam");
    }
}

#[test]
fn seed_is_deterministic() {
    let Some(mut s) = Scene::new(&Mesh::uv_sphere(48, 24), 256) else { return };
    for def in ["noise", "dirt", "leaks"] {
        let a = s.mask(def, &[("seed", ParamValue::Int(7))]);
        let b = s.mask(def, &[("seed", ParamValue::Int(7))]);
        let c = s.mask(def, &[("seed", ParamValue::Int(8))]);
        check_ok(def, &a);
        assert!(a.v == b.v, "{def}: same seed must give identical results");
        let diff = a.v.iter().zip(&c.v).filter(|(x, y)| (*x - *y).abs() > 1e-3).count();
        assert!(diff > a.v.len() / 20, "{def}: a different seed must change the mask ({diff} texels differ)");
    }
}

/// Noise is evaluated in world space: 512 downsampled to 256 matches the 256 render.
#[test]
fn noise_is_resolution_independent() {
    let sphere = Mesh::uv_sphere(48, 24);
    let params = [("scale", ParamValue::Float(0.2)), ("octaves", ParamValue::Int(3))];
    let lo = {
        let Some(mut s) = Scene::new(&sphere, 256) else { return };
        s.mask("noise", &params).v
    };
    let hi = {
        let Some(mut s) = Scene::new(&sphere, 512) else { return };
        s.mask("noise", &params).v
    };
    let mut sum = 0.0;
    for y in 0..256usize {
        for x in 0..256usize {
            let h = |dx: usize, dy: usize| hi[(y * 2 + dy) * 512 + x * 2 + dx];
            let d = (h(0, 0) + h(1, 0) + h(0, 1) + h(1, 1)) * 0.25;
            sum += (d - lo[y * 256 + x]).abs();
        }
    }
    let mad = sum / (256.0 * 256.0);
    println!("noise 256 vs 512 mean abs diff {mad:.4}");
    assert!(mad < 0.02, "noise depends on the texture resolution: {mad}");
}
