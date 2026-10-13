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
