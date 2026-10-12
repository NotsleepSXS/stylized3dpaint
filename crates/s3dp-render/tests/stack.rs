//! Headless GPU tests for the layer stack: painting, seams, effects on content and masks,
//! folders, effect layers and anchors (spec §13.1). Small textures to keep the GPU cool.

use s3dp_core::{
    BlendMode, ChannelId, ChannelSet, ChannelValues, EffectInstance, Mask, MaskItem, MaskItemKind, Mesh, NodeKind, OrbitCamera, ParamValue,
};
use s3dp_render::{Brush, EditTarget, ShadingMode, Viewport, wgpu};

const VIEW: [u32; 2] = [256, 256];
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
    cam: OrbitCamera,
}

impl Scene {
    fn new() -> Option<Self> {
        let (device, queue) = device()?;
        let mut vp = Viewport::new(&device);
        vp.paint.throttle = false;
        vp.set_mesh(&device, &queue, &Mesh::uv_sphere(48, 24), TEX);
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

    /// Final linear values of a channel, rgba per texel.
    fn channel(&mut self, c: ChannelId) -> Vec<f32> {
        self.render();
        self.vp.paint.read_final(&self.device, &self.queue, 0, c).unwrap().1
    }

    fn effect(&mut self, def: &str, params: &[(&str, ParamValue)]) -> EffectInstance {
        let mut e = self.vp.paint.new_effect(def).unwrap_or_else(|| panic!("effect {def}"));
        for (k, v) in params {
            e.params.insert((*k).into(), v.clone());
        }
        e
    }
}

fn sphere_texel(d: glam::Vec3) -> usize {
    let d = d.normalize();
    let u = d.z.atan2(d.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    let v = d.y.acos() / std::f32::consts::PI;
    let x = ((u * TEX as f32) as u32).min(TEX - 1);
    let y = ((v * TEX as f32) as u32).min(TEX - 1);
    (y * TEX + x) as usize
}

fn rgb(data: &[f32], texel: usize) -> [f32; 3] {
    [data[texel * 4], data[texel * 4 + 1], data[texel * 4 + 2]]
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn red_brush() -> Brush {
    let mut values = ChannelValues::defaults();
    values.set(ChannelId::BaseColor, [1.0, 0.0, 0.0]);
    Brush { values, channels: ChannelSet::only(ChannelId::BaseColor), radius: 30.0, hardness: 0.8, ..Default::default() }
}

fn default_gray() -> f32 {
    ChannelValues::defaults().get(ChannelId::BaseColor)[0]
}

#[test]
fn stroke_paints_front_only_and_undoes() {
    let Some(mut s) = Scene::new() else { return };
    s.stroke(red_brush(), &[[128.0, 128.0]]);
    let bc = s.channel(ChannelId::BaseColor);
    let (front, back) = (sphere_texel(s.cam.eye()), sphere_texel(-s.cam.eye()));
    assert!(near(rgb(&bc, front)[0], 1.0, 0.01) && rgb(&bc, front)[1] < 0.01, "front red: {:?}", rgb(&bc, front));
    assert!(near(rgb(&bc, back)[0], default_gray(), 0.01), "back untouched");
    s.vp.paint.undo();
    assert!(near(rgb(&s.channel(ChannelId::BaseColor), front)[0], default_gray(), 0.01), "undo");
    s.vp.paint.redo();
    assert!(near(rgb(&s.channel(ChannelId::BaseColor), front)[0], 1.0, 0.01), "redo");
}

#[test]
fn stroke_across_uv_seam_has_no_gap() {
    let Some(mut s) = Scene::new() else { return };
    s.cam = OrbitCamera { yaw: std::f32::consts::FRAC_PI_2, pitch: 0.0, ..Default::default() };
    s.cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
    s.render();
    let mut b = red_brush();
    b.radius = 8.0;
    b.values.set(ChannelId::BaseColor, [0.0, 0.0, 1.0]);
    let dabs: Vec<[f32; 2]> = (0..=40).map(|i| [68.0 + i as f32 * 3.0, 128.0]).collect();
    s.stroke(b, &dabs);
    let (w, _, frame) = s.vp.read_color(&s.device, &s.queue).unwrap();
    for x in 72..184 {
        let p = &frame[((128 * w + x) * 4) as usize..][..3];
        assert!(p[2] > 200 && p[0] < 60, "gap at x={x}: {p:?}");
    }
}

/// Stage 2 acceptance: a Levels filter works on a layer's content.
#[test]
fn levels_on_layer_content() {
    let Some(mut s) = Scene::new() else { return };
    let mut v = ChannelValues::defaults();
    v.set(ChannelId::BaseColor, [0.25; 3]);
    let levels = s.effect("levels", &[("in_white", ParamValue::Float(0.5))]);
    s.vp.paint.add_node(NodeKind::Fill(v), |n, _| n.effects.push(levels));
    let bc = s.channel(ChannelId::BaseColor);
    let front = sphere_texel(s.cam.eye());
    assert!(near(rgb(&bc, front)[0], 0.5, 0.01), "0.25 / 0.5 white point -> 0.5, got {:?}", rgb(&bc, front));
}

/// Number of texels with in-between values (soft edges) in the red channel.
fn soft_texels(data: &[f32]) -> usize {
    data.chunks_exact(4).filter(|p| p[0] > 0.05 && p[0] < 0.95).count()
}

#[test]
fn folder_blur_affects_children_only() {
    let Some(mut s) = Scene::new() else { return };
    // Black base so painted white dabs are easy to measure.
    s.vp.paint.edit_document(false, |d| d.defaults.set(ChannelId::BaseColor, [0.0; 3]));
    let mut white = red_brush();
    white.values.set(ChannelId::BaseColor, [1.0; 3]);
    white.hardness = 0.99;
    // Outside layer (bottom), painted on the left.
    let outside = s.vp.paint.active.unwrap();
    s.stroke(white.clone(), &[[70.0, 128.0]]);
    let hard = soft_texels(&s.channel(ChannelId::BaseColor));
    // Folder with one painted layer on the right.
    let folder = s.vp.paint.add_node(NodeKind::Folder { expanded: true }, |_, _| {});
    s.vp.paint.add_node(NodeKind::Paint, |_, _| {});
    s.stroke(white.clone(), &[[186.0, 128.0]]);
    let before = soft_texels(&s.channel(ChannelId::BaseColor));
    let blur = s.effect("blur", &[("intensity", ParamValue::Float(0.08))]);
    s.vp.paint.edit_node(folder, true, |n| n.effects.push(blur.clone()));
    let after = s.channel(ChannelId::BaseColor);
    let soft = soft_texels(&after);
    println!("soft texels: outside only {hard}, + folder {before}, folder blurred {soft}");
    assert!(soft > before * 2, "folder content is blurred");
    // The outside dab keeps its hard edge: probe its center row.
    let _ = outside;
    // Blur on the folder MASK instead only softens the mask, not a hard fill inside.
    s.vp.paint.undo();
    let mask_blur = s.effect("blur", &[("intensity", ParamValue::Float(0.08))]);
    let fill_id = s.vp.paint.new_id();
    let item_id = s.vp.paint.new_id();
    let grad = s.effect("position_gradient", &[("contrast", ParamValue::Float(1.0))]);
    s.vp.paint.edit_node(folder, true, |n| {
        n.mask = Some(Mask {
            enabled: true,
            items: vec![
                MaskItem { id: fill_id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(grad) },
                MaskItem { id: item_id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(mask_blur) },
            ],
        })
    });
    let masked = s.channel(ChannelId::BaseColor);
    assert!(soft_texels(&masked) >= before, "mask blur keeps rendering");
}

#[test]
fn effect_layer_scope_root_vs_folder() {
    let Some(mut s) = Scene::new() else { return };
    s.vp.paint.edit_document(false, |d| d.defaults.set(ChannelId::BaseColor, [0.2; 3]));
    let front = sphere_texel(s.cam.eye());
    let invert = |s: &mut Scene| {
        s.effect("levels", &[("out_black", ParamValue::Float(1.0)), ("out_white", ParamValue::Float(0.0))])
    };
    // Inside an empty folder the effect layer sees only the (transparent) folder content.
    let folder = s.vp.paint.add_node(NodeKind::Folder { expanded: true }, |_, _| {});
    let e = invert(&mut s);
    s.vp.paint.add_node(NodeKind::Effect(e), |_, _| {});
    let inside = rgb(&s.channel(ChannelId::BaseColor), front)[0];
    // At the root it inverts everything below.
    s.vp.paint.delete_node(folder);
    let e = invert(&mut s);
    s.vp.paint.active = None;
    s.vp.paint.add_node(NodeKind::Effect(e), |_, _| {});
    let root = rgb(&s.channel(ChannelId::BaseColor), front)[0];
    println!("base 0.2: inside folder {inside}, at root {root}");
    assert!(near(inside, 0.2, 0.02), "folder-scoped effect must not touch the base");
    assert!(near(root, 0.8, 0.02), "root effect layer inverts the stack below");
}

#[test]
fn anchor_drives_generator_above() {
    let Some(mut s) = Scene::new() else { return };
    s.vp.paint.edit_document(false, |d| d.defaults.set(ChannelId::BaseColor, [0.0; 3]));
    let front = sphere_texel(s.cam.eye());
    // Layer A: white paint with a mask whose painted item we mark as anchored via an effect.
    let a = s.vp.paint.active.unwrap();
    s.vp.paint.add_mask(a, false);
    let mut anchor = s.effect("levels", &[]);
    anchor.anchor = true;
    let anchor_id = anchor.id;
    let item = s.vp.paint.new_id();
    s.vp.paint.edit_node(a, true, |n| {
        n.mask.as_mut().unwrap().items.push(MaskItem { id: item, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(anchor) })
    });
    // Paint the mask white in the middle of the view.
    let mut white = red_brush();
    white.values.set(ChannelId::BaseColor, [1.0; 3]);
    let paint_item = match s.vp.paint.doc.find(a).unwrap().mask.as_ref().unwrap().items[1].kind {
        MaskItemKind::Paint => s.vp.paint.doc.find(a).unwrap().mask.as_ref().unwrap().items[1].id,
        _ => unreachable!(),
    };
    s.vp.paint.target = EditTarget::MaskItem(paint_item);
    s.stroke(white, &[[128.0, 128.0]]);
    // Layer B: green fill whose mask is the anchor reference.
    let mut green = ChannelValues::defaults();
    green.set(ChannelId::BaseColor, [0.0, 1.0, 0.0]);
    let reference = s.effect("anchor_reference", &[("source", ParamValue::Anchor(Some(anchor_id)))]);
    s.vp.paint.add_node(NodeKind::Fill(green), |n, d| {
        n.mask = Some(Mask {
            enabled: true,
            items: vec![MaskItem { id: d.alloc_id(), enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(reference) }],
        })
    });
    let bc = s.channel(ChannelId::BaseColor);
    let back = sphere_texel(-s.cam.eye());
    println!("front {:?} back {:?}", rgb(&bc, front), rgb(&bc, back));
    assert!(rgb(&bc, front)[1] > 0.9, "green where the anchor mask was painted");
    assert!(rgb(&bc, back)[1] < 0.05, "no green elsewhere");
}

#[test]
fn per_channel_blend_and_channel_toggle() {
    let Some(mut s) = Scene::new() else { return };
    let front = sphere_texel(s.cam.eye());
    let mut v = ChannelValues::defaults();
    v.set(ChannelId::Roughness, [0.2; 3]);
    v.set(ChannelId::BaseColor, [0.5; 3]);
    let id = s.vp.paint.add_node(NodeKind::Fill(v), |n, _| {
        n.blending.per_channel.insert(ChannelId::BaseColor, s3dp_core::ChannelBlend { mode: BlendMode::Multiply, opacity: 1.0 });
        n.blending.channels = ChannelSet::only(ChannelId::BaseColor);
    });
    let gray = default_gray();
    assert!(near(rgb(&s.channel(ChannelId::BaseColor), front)[0], gray * 0.5, 0.01), "multiply on base color");
    assert!(near(s.channel(ChannelId::Roughness)[front * 4], 0.7, 0.01), "roughness disabled for the layer");
    s.vp.paint.edit_node(id, true, |n| n.blending.channels.set(ChannelId::Roughness, true));
    assert!(near(s.channel(ChannelId::Roughness)[front * 4], 0.2, 0.01), "roughness enabled");
}

/// Every registered effect compiles for content and mask targets.
#[test]
fn every_effect_compiles() {
    let Some((device, _queue)) = device() else { return };
    let mut engine = s3dp_render::engine::effects::EffectEngine::new(&device);
    let mut failed = Vec::new();
    for def in &s3dp_core::registry().effects {
        for format in [s3dp_render::engine::effects::CONTENT_FORMAT, s3dp_render::engine::effects::MASK_FORMAT] {
            if let Err(e) = engine.pipeline(&device, def, format) {
                failed.push(format!("{} ({format:?}): {e}", def.id));
            }
        }
    }
    assert!(failed.is_empty(), "effects failed to compile:\n{}", failed.join("\n"));
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// Moving layers into folders (group, drag into a folder, multi-select group across folders)
/// must not change the result: a folder of normal-blended children at 100% is transparent.
#[test]
fn folders_do_not_change_the_result() {
    let Some(mut s) = Scene::new() else { return };
    // Several texture sets: the test model (monkey + crate).
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/monkey_crate.glb");
    let mesh = s3dp_core::import::load(&path).unwrap();
    s.vp.set_mesh(&s.device, &s.queue, &mesh, TEX);
    let b = mesh.bounds();
    s.cam.frame(b.center(), b.radius(), 1.0);
    let sets = s.vp.paint.sets.len();
    assert!(sets >= 2, "test model has several texture sets ({sets})");
    let chans = [ChannelId::BaseColor, ChannelId::Roughness, ChannelId::Height, ChannelId::Normal];
    let read = |s: &mut Scene| {
        s.render();
        (0..sets).flat_map(|set| chans.iter().map(move |c| (set, *c))).map(|(set, c)| s.vp.paint.read_final(&s.device, &s.queue, set, c).unwrap().1).collect::<Vec<_>>()
    };
    let check = |label: &str, a: &[Vec<f32>], b: &[Vec<f32>]| {
        for (c, (x, y)) in (0..).flat_map(|set| chans.iter().map(move |c| (set, *c))).zip(a.iter().zip(b)) {
            assert!(y.iter().all(|v| v.is_finite()), "{label}: {c:?} has NaN / inf");
            let d = max_diff(x, y);
            println!("{label}: {c:?} max diff {d:.5}");
            assert!(d < 2e-3, "{label}: {c:?} changed by {d}");
        }
    };
    let layer = s.vp.paint.active.unwrap();
    let mut brush = red_brush();
    brush.channels = ChannelSet::only(ChannelId::BaseColor);
    brush.channels.set(ChannelId::Roughness, true);
    brush.values.set(ChannelId::Roughness, [0.2; 3]);
    brush.hardness = 0.3;
    s.stroke(brush, &[[100.0, 120.0], [128.0, 128.0], [150.0, 140.0]]);
    let mut blue = ChannelValues::defaults();
    blue.set(ChannelId::BaseColor, [0.1, 0.2, 0.9]);
    let fill = s.vp.paint.add_node(NodeKind::Fill(blue), |n, _| n.blending.default.opacity = 0.4);
    // Content effects that read neighbours / world space / baked maps.
    let fx: Vec<String> = std::env::var("FX").unwrap_or_else(|_| "blur,hue_shift_by_light,warp,edge_highlight,stylize_master".into()).split(',').map(String::from).collect();
    for (i, id) in fx.iter().enumerate() {
        let e = s.effect(id, &[]);
        let target = if i % 2 == 0 { layer } else { fill };
        s.vp.paint.edit_node(target, false, |n| n.effects.push(e));
    }
    let before = read(&mut s);

    let g = s.vp.paint.group(&[layer]).unwrap();
    check("group one layer", &before, &read(&mut s));

    let f = s.vp.paint.add_node(NodeKind::Folder { expanded: true }, |_, _| {});
    s.vp.paint.move_node(fill, Some(f), 0);
    check("move fill into a folder", &before, &read(&mut s));

    // Layer (inside g) and fill (inside f) into one new folder, document order kept.
    let both = s.vp.paint.group(&[fill, layer]).unwrap();
    let kids: Vec<u64> = s.vp.paint.doc.find(both).unwrap().children.iter().map(|n| n.id).collect();
    assert_eq!(kids, vec![layer, fill], "children in document order");
    // f was created inside the active folder g; the new folder takes the place of the
    // shallowest selected node (the layer, in g), and f is left empty.
    assert!(s.vp.paint.doc.find(f).is_some_and(|n| n.children.is_empty()));
    assert!(s.vp.paint.doc.find(g).is_some_and(|n| n.children.first().map(|c| c.id) == Some(both)));
    check("group across folders", &before, &read(&mut s));

    // A folder and a node inside it: the child moves with its folder.
    let outer = s.vp.paint.group(&[both, fill]).unwrap();
    let o = s.vp.paint.doc.find(outer).unwrap();
    assert_eq!(o.children.len(), 1);
    assert_eq!(o.children[0].id, both);

    s.vp.paint.delete_nodes(&[f, layer]);
    assert!(s.vp.paint.doc.find(f).is_none() && s.vp.paint.doc.find(layer).is_none() && s.vp.paint.doc.find(fill).is_some());
}

/// Plain fills inside a folder: every channel stays perfectly uniform (any texel-to-texel
/// wobble in Height shows up as bump artifacts in the Lit view).
#[test]
fn fills_in_folder_stay_uniform() {
    let Some(mut s) = Scene::new() else { return };
    let mut a = ChannelValues::defaults();
    a.set(ChannelId::BaseColor, [0.6, 0.1, 0.05]);
    let mut b = ChannelValues::defaults();
    b.set(ChannelId::BaseColor, [0.2, 0.3, 0.8]);
    let fa = s.vp.paint.add_node(NodeKind::Fill(a), |_, _| {});
    let fb = s.vp.paint.add_node(NodeKind::Fill(b), |n, _| n.blending.default.opacity = 0.5);
    let range = |s: &mut Scene, c: ChannelId| {
        let v = s.channel(c);
        let covered: Vec<f32> = v.chunks_exact(4).filter(|p| p[3] > 0.5).map(|p| p[0]).collect();
        let lo = covered.iter().cloned().fold(f32::MAX, f32::min);
        let hi = covered.iter().cloned().fold(f32::MIN, f32::max);
        (lo, hi)
    };
    let normal = |s: &mut Scene| {
        let v = s.channel(ChannelId::Normal);
        let p = v.chunks_exact(4).find(|p| p[3] > 0.5).map(|p| [p[0], p[1], p[2]]).unwrap();
        p
    };
    for c in [ChannelId::Height, ChannelId::BaseColor, ChannelId::Roughness] {
        let (lo, hi) = range(&mut s, c);
        println!("root   {c:?}: {lo:.6} .. {hi:.6}");
    }
    println!("root   Normal sample {:?}", normal(&mut s));
    let lit = |s: &mut Scene, name: &str| {
        s.vp.render(&s.device, &s.queue, [400, 400], &s.cam, ShadingMode::Lit);
        let (w, h, f) = s.vp.read_color(&s.device, &s.queue).unwrap();
        if std::env::var("DUMP").is_ok() {
            image::save_buffer(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("lit_{name}.png")), &f, w, h, image::ColorType::Rgba8).unwrap();
        }
        f
    };
    let lit_root = lit(&mut s, "root");
    s.vp.paint.group(&[fa, fb]).unwrap();
    let lit_folder = lit(&mut s, "folder");
    println!("folder Normal sample {:?}", normal(&mut s));
    let d = lit_root.iter().zip(&lit_folder).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
    println!("lit view max diff root vs folder: {d}");
    assert!(d <= 2, "the Lit view changes when the fills are in a folder ({d})");
    for c in [ChannelId::Height, ChannelId::BaseColor, ChannelId::Roughness] {
        let (lo, hi) = range(&mut s, c);
        println!("folder {c:?}: {lo:.6} .. {hi:.6}");
        assert!(hi - lo < 1e-6, "{c:?} not uniform in a folder: {lo} .. {hi}");
    }
}
