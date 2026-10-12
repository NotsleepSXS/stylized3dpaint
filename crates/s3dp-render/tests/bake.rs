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
    vp.paint.throttle = false;
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
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, 1024);
    let t = std::time::Instant::now();
    let mut st = BakeSettings::preset(BakeStyle::Stylized);
    if std::env::var("NOSMOOTH").is_ok() {
        st.smoothing = 0.0;
    }
    vp.bake_now(&device, &queue, &mesh, None, &st);
    println!("bake took {:?}", t.elapsed());
    for m in [BakedMap::Edges, BakedMap::Cavity, BakedMap::Curvature] {
        let data = gpu::read_texture(&device, &queue, &vp.paint.sets[0].baked[m.index()]).unwrap();
        let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("preview_uv_{}.png", m.file_name()));
        image::save_buffer(out, &data, 1024, 1024, image::ColorType::L8).unwrap();
    }
    let mut cam = OrbitCamera { yaw: 0.5, pitch: 0.25, ..Default::default() };
    let b = mesh.bounds();
    cam.frame(b.center(), b.radius(), 1.5);
    let size = [900, 600];
    // Close-up of the monkey (at the origin) to spot per-polygon artifacts.
    let mut close = OrbitCamera { yaw: 0.35, pitch: 0.2, ..Default::default() };
    close.frame(glam::Vec3::ZERO, 1.2, 1.5);
    for (name, mode) in [
        ("edges", ShadingMode::Baked(BakedMap::Edges)),
        ("cavity", ShadingMode::Baked(BakedMap::Cavity)),
        ("curvature", ShadingMode::Baked(BakedMap::Curvature)),
    ] {
        vp.render(&device, &queue, size, &close, mode);
        let (w, h, frame) = vp.read_color(&device, &queue).unwrap();
        let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("preview_close_{name}.png"));
        image::save_buffer(&out, &frame, w, h, image::ColorType::Rgba8).unwrap();
    }
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

/// Light settings for the curvature tests (only edges / cavity / curvature matter).
fn light(style: BakeStyle) -> BakeSettings {
    let mut s = BakeSettings::preset(style);
    s.ao_samples = 8;
    s.sky_samples = 4;
    s.thickness_samples = 4;
    s.edge_samples = 32;
    s
}

/// Unit-sphere direction of texel (x, y) of `Mesh::uv_sphere`.
fn sphere_dir(x: u32, y: u32) -> Vec3 {
    let (u, v) = ((x as f32 + 0.5) / TEX as f32, (y as f32 + 0.5) / TEX as f32);
    let (theta, phi) = (v * std::f32::consts::PI, u * std::f32::consts::TAU);
    Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin())
}

/// Texels away from the poles and the u seam.
fn sphere_texels() -> impl Iterator<Item = (u32, u32)> {
    (24..TEX - 24).step_by(2).flat_map(|y| (4..TEX - 4).step_by(2).map(move |x| (x, y)))
}

struct Stats {
    mean: f32,
    std: f32,
    min: f32,
    max: f32,
}

fn stats(b: &Baked, m: BakedMap) -> Stats {
    let v: Vec<f32> = sphere_texels().map(|t| b.scalar(m, t)).collect();
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let std = (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt();
    let min = v.iter().cloned().fold(f32::MAX, f32::min);
    let max = v.iter().cloned().fold(f32::MIN, f32::max);
    Stats { mean, std, min, max }
}

#[test]
fn smooth_sphere_curvature_is_clean() {
    let mesh = Mesh::uv_sphere(64, 32);
    for style in [BakeStyle::Stylized, BakeStyle::Realistic] {
        let Some(b) = bake(&mesh, None, &light(style)) else { return };
        let c = stats(&b, BakedMap::Curvature);
        let e = stats(&b, BakedMap::Edges);
        let k = stats(&b, BakedMap::Cavity);
        println!(
            "{style:?} sphere: curvature mean {:.3} std {:.4} [{:.3}, {:.3}]; edges mean {:.3} [{:.3}, {:.3}]; cavity mean {:.3} max {:.3}",
            c.mean, c.std, c.min, c.max, e.mean, e.min, e.max, k.mean, k.max
        );
        assert!(c.mean > 0.5 && c.mean < 0.65, "convex sphere reads slightly convex, got {}", c.mean);
        assert!(c.min >= 0.5, "no concave texels on a convex sphere, got {}", c.min);
        assert!(c.max - c.min < 0.06 && c.std < 0.012, "no per-triangle pattern in curvature");
        assert!(e.max < 0.2 && e.max - e.min < 0.06, "no polygon edges in the edge map");
        assert!(k.max < 0.03, "no cavities on a convex sphere, got {}", k.max);
    }
}

#[test]
fn bumpy_sphere_curvature_follows_bumps() {
    // Smooth waves on a sphere, baked from itself (no high-poly).
    let wave = |d: Vec3| (d.x * 6.0).sin() * (d.y * 6.0).sin() * (d.z * 6.0).sin();
    let mut mesh = Mesh::uv_sphere(128, 64);
    for v in &mut mesh.vertices {
        let d = Vec3::from(v.position);
        v.position = (d * (1.0 + 0.06 * wave(d))).into();
    }
    mesh.compute_normals();
    mesh.compute_tangents();
    for style in [BakeStyle::Stylized, BakeStyle::Realistic] {
        let Some(b) = bake(&mesh, None, &light(style)) else { return };
        // Peaks of the wave are convex, troughs concave.
        let (mut peak, mut np, mut dip, mut nd) = (0.0, 0, 0.0, 0);
        let (mut sxy, mut sx, mut sy, mut sxx, mut syy, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for (x, y) in sphere_texels() {
            let g = wave(sphere_dir(x, y));
            let c = b.scalar(BakedMap::Curvature, (x, y));
            if g > 0.6 {
                peak += c;
                np += 1;
            } else if g < -0.6 {
                dip += c;
                nd += 1;
            }
            let (g, c) = (g as f64, c as f64);
            sxy += g * c;
            sx += g;
            sy += c;
            sxx += g * g;
            syy += c * c;
            n += 1.0;
        }
        let corr = (n * sxy - sx * sy) / ((n * sxx - sx * sx).sqrt() * (n * syy - sy * sy).sqrt());
        let (peak, dip) = (peak / np as f32, dip / nd as f32);
        let k = stats(&b, BakedMap::Cavity);
        let e = stats(&b, BakedMap::Edges);
        println!("{style:?} bumpy: curvature peaks {peak:.3} dips {dip:.3} corr {corr:.3}; edges mean {:.3} cavity mean {:.3}", e.mean, k.mean);
        assert!(peak > 0.52 && dip < 0.495 && peak - dip > 0.04, "peaks convex, dips concave");
        assert!(corr > 0.6, "curvature follows the bumps, corr {corr}");
    }
}
