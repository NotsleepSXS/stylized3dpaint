//! Polygon Fill (whole triangles into the stroke) and texture-space dabs (painting on the UV view).

use s3dp_core::{ChannelId, ChannelSet, Mesh, OrbitCamera};
use s3dp_render::{Brush, ShadingMode, Viewport, wgpu};

const TEX: u32 = 256;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn red() -> Brush {
    let mut b = Brush::default();
    b.channels = ChannelSet::only(ChannelId::BaseColor);
    b.values.set(ChannelId::BaseColor, [1.0, 0.0, 0.0]);
    b
}

/// Texel (x, y) at the centroid of triangle `t` in UV.
fn centroid_texel(m: &Mesh, t: usize) -> (usize, usize) {
    let uv: glam::Vec2 = (0..3).map(|k| glam::Vec2::from(m.vertices[m.indices[3 * t + k] as usize].uv)).sum::<glam::Vec2>() / 3.0;
    ((uv.x * TEX as f32) as usize, (uv.y * TEX as f32) as usize)
}

#[test]
fn polygon_fill_paints_exactly_the_selected_triangles() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::cube();
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, TEX);
    let cam = OrbitCamera::default();
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Unlit);
    // The first face of the cube (two triangles).
    let topo = s3dp_core::select::Topology::build(&mesh);
    let face = topo.triangles_with(s3dp_core::select::SelectMode::Face, 0);
    assert_eq!(face.len(), 2);
    vp.begin_stroke(&device, &queue, red()).unwrap();
    vp.paint.fill_triangles(0, s3dp_core::select::triangle_indices(&mesh, &face));
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Unlit);
    vp.paint.end_stroke();
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Unlit);
    let bc = vp.paint.read_final(&device, &queue, 0, ChannelId::BaseColor).unwrap().1;
    let at = |(x, y): (usize, usize)| [bc[(y * TEX as usize + x) * 4], bc[(y * TEX as usize + x) * 4 + 1]];
    for t in 0..12 {
        let c = at(centroid_texel(&mesh, t));
        let inside = face.contains(&(t as u32));
        assert_eq!(c[0] > 0.9 && c[1] < 0.05, inside, "triangle {t}: {c:?}");
    }
    // Undo removes the fill.
    vp.paint.undo();
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Unlit);
    let bc = vp.paint.read_final(&device, &queue, 0, ChannelId::BaseColor).unwrap().1;
    let (x, y) = centroid_texel(&mesh, face[0] as usize);
    assert!(bc[(y * TEX as usize + x) * 4 + 1] > 0.1, "undo");
}

#[test]
fn texture_space_dabs_paint_where_asked() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::cube();
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, TEX);
    let cam = OrbitCamera::default();
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Unlit);
    let (cx, cy) = centroid_texel(&mesh, 4);
    vp.begin_stroke(&device, &queue, red()).unwrap();
    vp.paint.add_dabs_uv(0, &[[cx as f32 + 0.5, cy as f32 + 0.5, 1.0]], 6.0);
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Unlit);
    vp.paint.end_stroke();
    vp.render(&device, &queue, [32, 32], &cam, ShadingMode::Unlit);
    let bc = vp.paint.read_final(&device, &queue, 0, ChannelId::BaseColor).unwrap().1;
    let at = |x: usize, y: usize| [bc[(y * TEX as usize + x) * 4], bc[(y * TEX as usize + x) * 4 + 1]];
    let c = at(cx, cy);
    assert!(c[0] > 0.9 && c[1] < 0.1, "dab centre is red: {c:?}");
    let far = at(cx + 12, cy);
    assert!(far[1] > 0.1, "12 texels away untouched: {far:?}");
}

/// The Polygon Fill overlay: highlighted triangles tint the view, the wireframe darkens edges;
/// both only while `show_overlay` is on.
#[test]
fn overlay_highlight_and_wireframe() {
    let Some((device, queue)) = device() else { return };
    let mesh = Mesh::uv_sphere(24, 12);
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, 64);
    let mut cam = OrbitCamera { yaw: 0.0, pitch: 0.0, ..Default::default() };
    cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    let size = [96u32, 96];
    vp.render(&device, &queue, size, &cam, ShadingMode::Unlit);
    let plain = vp.read_color(&device, &queue).unwrap().2;
    // Highlight every triangle: the centre pixel turns orange.
    let all: Vec<u32> = (0..mesh.indices.len() as u32 / 3).collect();
    vp.set_highlight(&device, &s3dp_core::select::triangle_indices(&mesh, &all));
    vp.render(&device, &queue, size, &cam, ShadingMode::Unlit);
    let off = vp.read_color(&device, &queue).unwrap().2;
    assert_eq!(off, plain, "no overlay while show_overlay is off");
    vp.show_overlay = true;
    vp.render(&device, &queue, size, &cam, ShadingMode::Unlit);
    let on = vp.read_color(&device, &queue).unwrap().2;
    let i = ((48 * 96 + 48) * 4) as usize;
    println!("centre plain {:?} highlighted {:?}", &plain[i..i + 3], &on[i..i + 3]);
    assert!(on[i] as i32 - on[i + 2] as i32 > 30, "orange tint on highlighted triangles");
    let diff = plain.chunks_exact(4).zip(on.chunks_exact(4)).filter(|(a, b)| a != b).count();
    assert!(diff > 1000, "overlay changes the image ({diff} px)");
}

/// Overlay per selection mode, 3D and UV, as one image:
/// `cargo test -p s3dp-render --test polyfill overlay_sheet -- --ignored --nocapture`
#[test]
#[ignore]
fn overlay_sheet() {
    use s3dp_core::select::{SelectMode, Topology, boundary_edges, group_color};
    let Some((device, queue)) = device() else { return };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/monkey_crate.glb");
    let mesh = s3dp_core::import::load(&path).unwrap();
    let topo = Topology::build(&mesh);
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, 256);
    vp.show_overlay = true;
    let mut cam = OrbitCamera { yaw: 0.5, pitch: 0.25, ..Default::default() };
    let b = mesh.bounds();
    cam.frame(b.center(), b.radius() * 0.8, 1.0);
    let (w, h) = (300u32, 300u32);
    let mut frames = Vec::new();
    for mode in [SelectMode::Face, SelectMode::Island, SelectMode::Element] {
        let tint: Vec<s3dp_render::TintVertex> = if mode == SelectMode::Face {
            Vec::new()
        } else {
            let g = if mode == SelectMode::Island { &topo.island } else { &topo.element };
            mesh.indices.chunks_exact(3).enumerate().flat_map(|(t, tri)| tri.iter().map(move |&v| (v, group_color(g[t])))).map(|(v, color)| s3dp_render::TintVertex { position: mesh.vertices[v as usize].position, uv: mesh.vertices[v as usize].uv, color }).collect()
        };
        vp.set_overlay(&device, &boundary_edges(&mesh, &topo, mode, false), &boundary_edges(&mesh, &topo, mode, true), &tint);
        vp.render(&device, &queue, [w, h], &cam, ShadingMode::Unlit);
        frames.push(vp.read_color(&device, &queue).unwrap().2);
        let what = s3dp_render::Preview { mask_of: None, uv: true, set: 0, shading: Some(ShadingMode::Unlit), overlay: true };
        vp.render_preview(&device, &queue, 0, [w, h], &cam, what).unwrap();
        frames.push(vp.read_preview(&device, &queue, 0).unwrap().2);
    }
    let cols = 2usize;
    let rows = frames.len() / cols;
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * cols * h * rows * 4];
    for (n, f) in frames.iter().enumerate() {
        let (cx, cy) = (n % cols, n / cols);
        for y in 0..h {
            let o = ((cy * h + y) * w * cols + cx * w) * 4;
            out[o..o + w * 4].copy_from_slice(&f[y * w * 4..(y + 1) * w * 4]);
        }
    }
    let p = std::env::temp_dir().join("s3dp_overlay_sheet.png");
    image::save_buffer(&p, &out, (w * cols) as u32, (h * rows) as u32, image::ColorType::Rgba8).unwrap();
    println!("{}", p.display());
}

/// Hidden (back) edges must not show through the model: pixels on the projection of the
/// edges around the cube corner farthest from the camera stay as without the overlay.
#[test]
fn overlay_does_not_show_through() {
    use s3dp_core::select::{SelectMode, Topology, boundary_edges};
    let Some((device, queue)) = device() else { return };
    // A thin slab (like a crowbar's cross-section seen from far): back edges sit just behind
    // the front face.
    let mut mesh = Mesh::cube();
    for v in &mut mesh.vertices {
        v.position[2] *= 0.2;
    }
    let topo = Topology::build(&mesh);
    let mut vp = Viewport::new(&device);
    vp.paint.throttle = false;
    vp.set_mesh(&device, &queue, &mesh, 64);
    let mut cam = OrbitCamera { yaw: 0.35, pitch: 0.3, ..Default::default() };
    cam.frame(glam::Vec3::ZERO, 1.2, 1.0);
    cam.distance *= 6.0; // far away: depth precision matters
    cam.fov_y /= 6.0;
    let size = [128u32, 128];
    vp.render(&device, &queue, size, &cam, ShadingMode::Unlit);
    let plain = vp.read_color(&device, &queue).unwrap().2;
    vp.set_overlay(&device, &boundary_edges(&mesh, &topo, SelectMode::Face, false), &[], &[]);
    vp.show_overlay = true;
    vp.render(&device, &queue, size, &cam, ShadingMode::Unlit);
    let on = vp.read_color(&device, &queue).unwrap().2;
    // Back face corners: z = -0.7 * 0.03 side seen from +z: farthest is the corner away from the eye.
    let eye = cam.eye();
    let corners: Vec<glam::Vec3> = (0..8).map(|i| glam::Vec3::new(if i & 1 == 0 { -0.7 } else { 0.7 }, if i & 2 == 0 { -0.7 } else { 0.7 }, if i & 4 == 0 { -0.14 } else { 0.14 })).collect();
    let far = *corners.iter().max_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye))).unwrap();
    let vpm = cam.projection(1.0) * cam.view();
    let mut changed = 0;
    let mut tested = 0;
    for c in &corners {
        // Edges from the far corner to its neighbours (differ in one axis).
        let diff = (*c - far).abs();
        let axes = (diff.x > 1e-3) as u32 + (diff.y > 1e-3) as u32 + (diff.z > 1e-3) as u32;
        if axes != 1 || diff.z > 1e-3 {
            continue; // skip the short thickness edge (it is near the silhouette)
        }
        for k in 1..8 {
            let p = far + (*c - far) * (k as f32 / 8.0);
            let n = vpm.project_point3(p);
            let (x, y) = (((n.x * 0.5 + 0.5) * 128.0) as usize, ((0.5 - n.y * 0.5) * 128.0) as usize);
            if x < 4 || y < 4 || x > 123 || y > 123 {
                continue;
            }
            // Skip where the matching front edge (visible) projects within 3 px.
            let front = vpm.project_point3(glam::Vec3::new(p.x, p.y, -p.z));
            let (fx, fy) = ((front.x * 0.5 + 0.5) * 128.0, (0.5 - front.y * 0.5) * 128.0);
            if (fx - x as f32).hypot(fy - y as f32) < 3.0 {
                continue;
            }
            tested += 1;
            let i = (y * 128 + x) * 4;
            if (0..3).any(|ch| (on[i + ch] as i32 - plain[i + ch] as i32).abs() > 12) {
                changed += 1;
            }
        }
    }
    println!("hidden edge pixels tested {tested}, changed {changed}");
    assert!(tested >= 6);
    assert!(changed <= 1, "back edges show through the model ({changed} / {tested})");
}
