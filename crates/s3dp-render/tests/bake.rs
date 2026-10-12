//! Headless GPU tests for the mesh map baker on simple scenes with known answers.

use glam::Vec3;
use s3dp_core::{BakeSettings, BakeStyle, BakedMap, Mesh, MeshPart, Vertex};
use s3dp_render::{Viewport, gpu, wgpu};

const TEX: u32 = 256;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// Axis-aligned box; each face gets its own cell of a 4x4 UV atlas starting at `slot`.
fn add_box(mesh: &mut Mesh, center: Vec3, half: Vec3, slot: &mut u32) {
    let faces = [
        (Vec3::X, Vec3::Z, Vec3::Y),
        (-Vec3::X, -Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, -Vec3::Z),
        (-Vec3::Y, Vec3::X, Vec3::Z),
        (Vec3::Z, -Vec3::X, Vec3::Y),
        (-Vec3::Z, Vec3::X, Vec3::Y),
    ];
    for (n, u, v) in faces {
        let (cx, cy) = ((*slot % 4) as f32 * 0.25, (*slot / 4) as f32 * 0.25);
        *slot += 1;
        let base = mesh.vertices.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = center + (n + u * a + v * b) * half;
            let uv = [cx + 0.02 + (a * 0.5 + 0.5) * 0.21, cy + 0.02 + (0.5 - b * 0.5) * 0.21];
            mesh.vertices.push(Vertex { position: p.into(), normal: n.into(), uv, tangent: [0.0; 4] });
        }
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

fn finish(mut mesh: Mesh) -> Mesh {
    mesh.parts = vec![MeshPart { name: "default".into(), first_index: 0, index_count: mesh.indices.len() as u32 }];
    mesh.compute_tangents();
    mesh
}

/// UV of a point on box face `face` (index in `add_box` order) with face coords a, b in -1..1.
fn face_uv(slot: u32, a: f32, b: f32) -> (u32, u32) {
    let (cx, cy) = ((slot % 4) as f32 * 0.25, (slot / 4) as f32 * 0.25);
    let u = cx + 0.02 + (a * 0.5 + 0.5) * 0.21;
    let v = cy + 0.02 + (0.5 - b * 0.5) * 0.21;
    ((u * TEX as f32) as u32, (v * TEX as f32) as u32)
}

struct Baked {
    maps: Vec<Vec<u8>>,
}

impl Baked {
    fn scalar(&self, m: BakedMap, (x, y): (u32, u32)) -> f32 {
        self.maps[m.index()][(y * TEX + x) as usize] as f32 / 255.0
    }
    fn rgb(&self, m: BakedMap, (x, y): (u32, u32)) -> [u8; 3] {
        let i = ((y * TEX + x) * 4) as usize;
        let d = &self.maps[m.index()];
        [d[i], d[i + 1], d[i + 2]]
    }
}

fn bake(mesh: &Mesh, high: Option<&Mesh>, settings: &BakeSettings) -> Option<Baked> {
    let (device, queue) = device()?;
    let mut vp = Viewport::new(&device);
        vp.paint.throttle_effects = false;
    vp.set_mesh(&device, &queue, mesh, TEX);
    vp.bake_now(&device, &queue, mesh, high, settings);
    let maps = vp.paint.sets[0]
        .baked
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let data = gpu::read_texture(&device, &queue, t).unwrap();
            // Save for eyeballing.
            if !BakedMap::ALL[i].is_color() {
                let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bake_{}.png", BakedMap::ALL[i].file_name()));
                image::save_buffer(out, &data, TEX, TEX, image::ColorType::L8).ok();
            }
            data
        })
        .collect();
    Some(Baked { maps })
}

#[test]
fn cube_edges_and_faces() {
    let mut mesh = Mesh::default();
    let mut slot = 0;
    add_box(&mut mesh, Vec3::ZERO, Vec3::ONE, &mut slot);
    let mesh = finish(mesh);
    let Some(b) = bake(&mesh, None, &BakeSettings::preset(BakeStyle::Stylized)) else { return };
    // Top face (slot 2): center vs near an edge.
    let center = face_uv(2, 0.0, 0.0);
    let near_edge = face_uv(2, 0.97, 0.0);
    let (e_c, e_e) = (b.scalar(BakedMap::Edges, center), b.scalar(BakedMap::Edges, near_edge));
    println!("edges: center {e_c}, near edge {e_e}");
    assert!(e_c < 0.05, "flat face center has no edge, got {e_c}");
    assert!(e_e > 0.4, "texel next to a convex cube edge is an edge, got {e_e}");
    assert!(b.scalar(BakedMap::Curvature, near_edge) > 0.6);
    assert!(b.scalar(BakedMap::AmbientOcclusion, center) > 0.95, "nothing occludes a lone cube");
    assert!(b.scalar(BakedMap::Cavity, center) < 0.05);
    // Sky light: the top face is lit, the bottom face (slot 3) is dark.
    let (top, bottom) = (b.scalar(BakedMap::SkyLight, center), b.scalar(BakedMap::SkyLight, face_uv(3, 0.0, 0.0)));
    println!("sky: top {top}, bottom {bottom}");
    assert!(top > bottom + 0.4);
    // No high-poly: flat normal map.
    let n = b.rgb(BakedMap::Normal, center);
    assert!(n[0].abs_diff(128) <= 2 && n[1].abs_diff(128) <= 2 && n[2] >= 250, "flat normal, got {n:?}");
}

#[test]
fn inner_corner_is_cavity_and_occluded() {
    let mut mesh = Mesh::default();
    let mut slot = 0;
    // A big floor slab and a box standing on it.
    add_box(&mut mesh, Vec3::new(0.0, -0.1, 0.0), Vec3::new(3.0, 0.1, 3.0), &mut slot);
    add_box(&mut mesh, Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.5), &mut slot);
    let mesh = finish(mesh);
    let Some(b) = bake(&mesh, None, &BakeSettings::preset(BakeStyle::Stylized)) else { return };
    // Floor top face is slot 2 (spans -3..3); the box sits in the middle (|x|,|z| < 0.5).
    let by_wall = face_uv(2, 0.53 / 3.0, 0.0);
    let open = face_uv(2, 0.9, 0.9);
    let (c_wall, c_open) = (b.scalar(BakedMap::Cavity, by_wall), b.scalar(BakedMap::Cavity, open));
    let (ao_wall, ao_open) = (b.scalar(BakedMap::AmbientOcclusion, by_wall), b.scalar(BakedMap::AmbientOcclusion, open));
    println!("cavity wall {c_wall} open {c_open}; ao wall {ao_wall} open {ao_open}");
    assert!(c_wall > 0.3 && c_open < 0.05);
    assert!(ao_wall < ao_open - 0.15);
}

#[test]
fn high_poly_gives_normal_detail() {
    let low = Mesh::uv_sphere(24, 12);
    // Bumpy high-poly: radial waves of 3% amplitude.
    let mut high = Mesh::uv_sphere(256, 128);
    for v in &mut high.vertices {
        let p = Vec3::from(v.position);
        let bump = 1.0 + 0.03 * (p.x * 12.0).sin() * (p.y * 12.0).sin() * (p.z * 12.0).sin();
        v.position = (p * bump).into();
    }
    high.compute_normals();
    let mut settings = BakeSettings::preset(BakeStyle::Realistic);
    settings.ao_samples = 8;
    settings.sky_samples = 8;
    settings.cage_front = 0.06;
    settings.cage_back = 0.06;

    let deviation = |b: &Baked| {
        let mut sum = 0u64;
        let mut count = 0u64;
        for y in (16..TEX - 16).step_by(4) {
            for x in (0..TEX).step_by(4) {
                let n = b.rgb(BakedMap::Normal, (x, y));
                sum += (n[0].abs_diff(128) + n[1].abs_diff(128)) as u64;
                count += 1;
            }
        }
        sum as f32 / count as f32
    };
    let Some(flat) = bake(&low, None, &settings) else { return };
    let detailed = bake(&low, Some(&high), &settings).unwrap();
    let (d0, d1) = (deviation(&flat), deviation(&detailed));
    println!("normal map mean xy deviation: low-poly only {d0}, from high-poly {d1}");
    assert!(d0 < 3.0, "without a high-poly the normal map is flat (127/128 rounding only)");
    assert!(d1 > 10.0, "high-poly bumps show up in the normal map");
}

#[test]
fn lone_cube_ao_is_open_on_every_face() {
    let mut mesh = Mesh::default();
    let mut slot = 0;
    add_box(&mut mesh, Vec3::ZERO, Vec3::ONE, &mut slot);
    let mesh = finish(mesh);
    let Some(b) = bake(&mesh, None, &BakeSettings::preset(BakeStyle::Realistic)) else { return };
    let ao: Vec<f32> = (0..6).map(|f| b.scalar(BakedMap::AmbientOcclusion, face_uv(f, 0.0, 0.0))).collect();
    println!("ao per face: {ao:?}");
    assert!(ao.iter().all(|&a| a > 0.95), "{ao:?}");
}

/// Visual check on the test model: `cargo test -p s3dp-render --test bake preview -- --ignored`.
/// Writes viewport renders of several baked maps to target/tmp.
#[test]
#[ignore]
fn preview_monkey() {
    use s3dp_core::OrbitCamera;
    use s3dp_render::ShadingMode;
    let Some((device, queue)) = device() else { return };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/monkey_crate.glb");
    let mesh = s3dp_core::import::load(&path).unwrap();
    let mut vp = Viewport::new(&device);
        vp.paint.throttle_effects = false;
    vp.set_mesh(&device, &queue, &mesh, 1024);
    let t = std::time::Instant::now();
    vp.bake_now(&device, &queue, &mesh, None, &BakeSettings::preset(BakeStyle::Stylized));
    println!("bake took {:?}", t.elapsed());
    let mut cam = OrbitCamera { yaw: 0.5, pitch: 0.25, ..Default::default() };
    let b = mesh.bounds();
    cam.frame(b.center(), b.radius(), 1.5);
    let size = [900, 600];
    for (name, mode) in [
        ("lit", ShadingMode::Lit),
        ("edges", ShadingMode::Baked(BakedMap::Edges)),
        ("cavity", ShadingMode::Baked(BakedMap::Cavity)),
        ("ao", ShadingMode::Baked(BakedMap::AmbientOcclusion)),
        ("sky", ShadingMode::Baked(BakedMap::SkyLight)),
        ("curvature", ShadingMode::Baked(BakedMap::Curvature)),
    ] {
        vp.render(&device, &queue, size, &cam, mode);
        let (w, h, frame) = vp.read_color(&device, &queue).unwrap();
        let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("preview_{name}.png"));
        image::save_buffer(&out, &frame, w, h, image::ColorType::Rgba8).unwrap();
    }
}
