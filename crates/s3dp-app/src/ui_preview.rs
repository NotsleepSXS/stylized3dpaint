//! Effect preview on a sample shape (sphere / cube / cylinder): a small separate scene with
//! its own document, baked maps and camera, so the preview shows how the effect works on a
//! neutral object instead of the model being edited. Drag the image to orbit.

use egui_phosphor::regular as icon;
use s3dp_core::{BakeSettings, BakeStyle, BlendMode, ChannelId, ChannelValues, EffectInstance, Mask, MaskItem, MaskItemKind, Mesh, NodeKind, OrbitCamera, ParamValue};
use s3dp_render::{ShadingMode, Viewport};

use crate::i18n::t;
use crate::theme;

const TEX: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Sphere,
    Cube,
    Cylinder,
}

impl Shape {
    const ALL: [Shape; 3] = [Shape::Sphere, Shape::Cube, Shape::Cylinder];

    fn mesh(self) -> Mesh {
        match self {
            Shape::Sphere => Mesh::uv_sphere(64, 32),
            Shape::Cube => Mesh::cube(),
            Shape::Cylinder => Mesh::cylinder(64),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Shape::Sphere => icon::CIRCLE,
            Shape::Cube => icon::CUBE,
            Shape::Cylinder => icon::CYLINDER,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Shape::Sphere => "Sphere",
            Shape::Cube => "Cube",
            Shape::Cylinder => "Cylinder",
        }
    }
}

pub struct SampleScene {
    vp: Viewport,
    shape: Shape,
    cam: OrbitCamera,
    /// What the document was built from (effect state + target).
    built: Option<String>,
    /// egui textures of the 3D view and the UV view.
    tex: [Option<egui::TextureId>; 2],
}

impl SampleScene {
    fn new(device: &egui_wgpu::wgpu::Device, queue: &egui_wgpu::wgpu::Queue, shape: Shape) -> Self {
        let mut s = Self { vp: Viewport::new(device), shape, cam: OrbitCamera { yaw: 0.7, pitch: 0.45, ..Default::default() }, built: None, tex: [None, None] };
        s.load_shape(device, queue, shape);
        s
    }

    fn load_shape(&mut self, device: &egui_wgpu::wgpu::Device, queue: &egui_wgpu::wgpu::Queue, shape: Shape) {
        let mesh = shape.mesh();
        self.vp.set_mesh(device, queue, &mesh, TEX);
        // Cheap stylized bake: the wear / edge effects need edges, cavities and AO.
        let settings = BakeSettings { ao_samples: 16, edge_samples: 12, thickness_samples: 8, sky_samples: 16, ..BakeSettings::preset(BakeStyle::Stylized) };
        self.vp.bake_now(device, queue, &mesh, None, &settings);
        // Frame by the true radius (the box diagonal leaves a sphere small).
        let c = mesh.bounds().center();
        let r = mesh.vertices.iter().map(|v| glam::Vec3::from(v.position).distance(c)).fold(0.0f32, f32::max);
        self.cam.frame(c, r, 1.0 / 0.7);
        self.shape = shape;
        self.built = None;
    }

    /// Rebuilds the sample document for a preview stack.
    fn build(&mut self, stack: &PreviewStack) {
        let paint = &mut self.vp.paint;
        paint.reset_document();
        let registry = s3dp_core::registry();
        let is_generator = |e: &EffectInstance| registry.get(&e.def).is_some_and(|d| d.category.is_generator());
        let fresh = |paint: &mut s3dp_render::Painter, e: &EffectInstance| {
            let mut c = e.clone();
            c.id = paint.new_id();
            c.anchor = false;
            c.enabled = true;
            c
        };
        // Filters need something to work on: a noise pattern under them when nothing below
        // produces a pattern (no generator in the stack).
        let pattern = |paint: &mut s3dp_render::Painter, mask: bool| {
            let mut n = paint.new_effect("noise")?;
            n.params.insert("scale".into(), ParamValue::Float(0.08));
            n.params.insert("grunge_amount".into(), ParamValue::Float(0.0));
            n.params.insert("g_contrast".into(), ParamValue::Float(0.4));
            if !mask {
                n.blend = BlendMode::Overlay;
                n.opacity = 0.8;
            }
            Some(n)
        };
        let mut base = ChannelValues::defaults();
        base.set(ChannelId::BaseColor, [0.62, 0.36, 0.24]);
        base.set(ChannelId::Roughness, [0.55; 3]);
        let base = stack.base.clone().unwrap_or(base);
        match &stack.items {
            Items::Mask(items) => {
                // Light layer over a dark base, revealed by the mask stack.
                let mut dark = ChannelValues::defaults();
                dark.set(ChannelId::BaseColor, [0.06, 0.07, 0.09]);
                paint.add_node(NodeKind::Fill(dark), |_, _| {});
                let mut light = ChannelValues::defaults();
                light.set(ChannelId::BaseColor, [0.85, 0.78, 0.68]);
                let has_source = items.iter().any(|i| match &i.kind {
                    MaskItemKind::Effect(e) => is_generator(e),
                    MaskItemKind::Fill(_) => true,
                    MaskItemKind::Paint => false,
                });
                let mut out = Vec::new();
                let first_id = paint.new_id();
                out.push(MaskItem { id: first_id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Fill(if has_source { 0.0 } else { 1.0 }) });
                if !has_source && let Some(n) = pattern(paint, true) {
                    let id = paint.new_id();
                    out.push(MaskItem { id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(n) });
                }
                for item in items {
                    let kind = match &item.kind {
                        MaskItemKind::Effect(e) => MaskItemKind::Effect(fresh(paint, e)),
                        MaskItemKind::Fill(v) => MaskItemKind::Fill(*v),
                        MaskItemKind::Paint => continue,
                    };
                    let id = paint.new_id();
                    out.push(MaskItem { id, enabled: item.enabled, opacity: item.opacity, blend: item.blend, kind });
                }
                paint.add_node(NodeKind::Fill(light), |n, _| n.mask = Some(Mask { enabled: true, items: out }));
            }
            Items::Layer(effect) => {
                let p = pattern(paint, false);
                let e = fresh(paint, effect);
                paint.add_node(NodeKind::Fill(base), |n, _| n.effects.extend(p));
                paint.add_node(NodeKind::Effect(e), |_, _| {});
            }
            Items::Content(effects) => {
                let p = if effects.iter().any(is_generator) { None } else { pattern(paint, false) };
                let list: Vec<EffectInstance> = effects.iter().map(|e| fresh(paint, e)).collect();
                paint.add_node(NodeKind::Fill(base), |n, _| {
                    n.effects.extend(p);
                    n.effects.extend(list);
                });
            }
        }
    }
}

/// The previewed effect with what lies below it in its own stack (effects above it are
/// left out, so the preview shows exactly what this effect does to its input).
#[derive(Clone, Debug)]
pub struct PreviewStack {
    pub items: Items,
    /// Fill values of the layer (content previews); None = a neutral material.
    pub base: Option<ChannelValues>,
}

#[derive(Clone, Debug)]
pub enum Items {
    /// Content effects of the layer, bottom first, ending with the previewed one.
    Content(Vec<EffectInstance>),
    /// Mask items, bottom first, ending with the previewed effect (painted items are skipped:
    /// strokes belong to the model's UVs).
    Mask(Vec<MaskItem>),
    /// An effect layer (filters what is below it).
    Layer(EffectInstance),
}

impl PreviewStack {
    /// Stack of `node` up to the effect `effect_id`, with `current` (the values being edited)
    /// in place of the stored effect.
    pub fn of(node: &s3dp_core::Node, effect_id: u64, current: &EffectInstance) -> Option<Self> {
        if let NodeKind::Effect(e) = &node.kind
            && e.id == effect_id
        {
            return Some(Self { items: Items::Layer(current.clone()), base: None });
        }
        let base = match &node.kind {
            NodeKind::Fill(v) => Some(v.clone()),
            _ => None,
        };
        if let Some(i) = node.effects.iter().position(|e| e.id == effect_id) {
            let mut list: Vec<EffectInstance> = node.effects[..=i].iter().filter(|e| e.enabled).cloned().collect();
            if let Some(last) = list.last_mut().filter(|l| l.id == effect_id) {
                *last = current.clone();
            } else {
                list.push(current.clone());
            }
            return Some(Self { items: Items::Content(list), base });
        }
        let mask = node.mask.as_ref()?;
        let i = mask.items.iter().position(|it| matches!(&it.kind, MaskItemKind::Effect(e) if e.id == effect_id))?;
        let mut items: Vec<MaskItem> = mask.items[..=i].iter().filter(|it| it.enabled).cloned().collect();
        if let Some(MaskItem { kind: MaskItemKind::Effect(e), .. }) = items.last_mut() {
            *e = current.clone();
        }
        Some(Self { items: Items::Mask(items), base: None })
    }
}

/// Preview of an effect (with what is below it) on the sample shape; 3D or the flat UV of
/// the sample, with the shape switch.
pub fn show(app: &mut crate::app::App, ui: &mut egui::Ui, stack: &PreviewStack, uv: bool) {
    let rs = app.render_state.clone();
    let shape_id = egui::Id::new("sample_shape");
    let mut shape: Shape = ui.data_mut(|d| *d.get_persisted_mut_or(shape_id, Shape::Sphere));
    ui.horizontal(|ui| {
        for s in Shape::ALL {
            ui.selectable_value(&mut shape, s, s.icon()).on_hover_text(t(s.label()));
        }
    });
    ui.data_mut(|d| d.insert_persisted(shape_id, shape));
    let scene = app.sample.get_or_insert_with(|| SampleScene::new(&rs.device, &rs.queue, shape));
    if scene.shape != shape {
        scene.load_shape(&rs.device, &rs.queue, shape);
    }
    let key = format!("{stack:?}");
    if scene.built.as_deref() != Some(key.as_str()) {
        scene.build(stack);
        scene.built = Some(key);
    }
    let w = ui.available_width().clamp(80.0, 420.0);
    let h = if uv { w } else { (w * 0.7).round() };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::drag());
    if resp.dragged() && !uv {
        let d = resp.drag_delta();
        scene.cam.yaw += d.x * 0.01;
        scene.cam.pitch = (scene.cam.pitch + d.y * 0.01).clamp(-1.5, 1.5);
    }
    let ppp = ui.ctx().pixels_per_point();
    let size = [(w * ppp).round() as u32, (h * ppp).round() as u32];
    // The sample's own main view does the 3D render (and applies its recomposite); the UV
    // view is one of its preview slots.
    let (view3d, recreated3d) = scene.vp.render(&rs.device, &rs.queue, if uv { [8, 8] } else { size }, &scene.cam, ShadingMode::Lit);
    let view3d = view3d.clone();
    let (view, recreated, slot) = if uv {
        let what = s3dp_render::Preview { mask_of: None, uv: true, set: 0, shading: Some(ShadingMode::Unlit) };
        match scene.vp.render_preview(&rs.device, &rs.queue, 0, size, &scene.cam, what) {
            Some((v, r)) => (v.clone(), r, 1),
            None => return,
        }
    } else {
        (view3d, recreated3d, 0)
    };
    let mut renderer = rs.renderer.write();
    let tex = match scene.tex[slot] {
        Some(id) => {
            if recreated {
                renderer.update_egui_texture_from_wgpu_texture(&rs.device, &view, egui_wgpu::wgpu::FilterMode::Linear, id);
            }
            id
        }
        None => {
            let id = renderer.register_native_texture(&rs.device, &view, egui_wgpu::wgpu::FilterMode::Linear);
            scene.tex[slot] = Some(id);
            id
        }
    };
    drop(renderer);
    ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, theme::FIELD_BORDER), egui::StrokeKind::Inside);
    // Keep refreshing while the sample stack recomposes.
    if scene.vp.paint.wants_repaint() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }
    if !uv {
        resp.on_hover_text(t("Drag to rotate"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use s3dp_core::effect_def::Target;

    /// The preview stack keeps what is below the effect and drops what is above it.
    #[test]
    fn preview_stack_cuts_above_the_effect() {
        let fx = |id: u64, def: &str| s3dp_core::registry().get(def).unwrap().instantiate(id);
        let mut node = s3dp_core::Node::new(1, "Fill", NodeKind::Fill(ChannelValues::defaults()));
        let mut off = fx(11, "hsl");
        off.enabled = false;
        node.effects = vec![fx(10, "noise"), off, fx(12, "slope_blur"), fx(13, "posterize")];
        let mut edited = fx(12, "slope_blur");
        edited.params.insert("intensity".into(), ParamValue::Float(0.123));
        let st = PreviewStack::of(&node, 12, &edited).unwrap();
        let Items::Content(list) = &st.items else { panic!("content stack") };
        assert_eq!(list.iter().map(|e| e.id).collect::<Vec<_>>(), vec![10, 12], "below + itself; disabled and above dropped");
        assert_eq!(list[1].params.get("intensity"), Some(&ParamValue::Float(0.123)), "current values");
        assert!(st.base.is_some(), "fill values as the base");

        let item = |id: u64, kind: MaskItemKind| MaskItem { id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind };
        node.mask = Some(Mask {
            enabled: true,
            items: vec![item(20, MaskItemKind::Fill(1.0)), item(21, MaskItemKind::Effect(fx(30, "curvature"))), item(22, MaskItemKind::Effect(fx(31, "blur")))],
        });
        let st = PreviewStack::of(&node, 30, &fx(30, "curvature")).unwrap();
        let Items::Mask(items) = &st.items else { panic!("mask stack") };
        assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![20, 21]);
    }

    /// Every built-in effect builds and renders in the sample scene (content and mask).
    #[test]
    fn every_effect_previews_on_the_sample_cube() {
        let instance = egui_wgpu::wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&Default::default())) else { return };
        let mut scene = SampleScene::new(&device, &queue, Shape::Cube);
        let mut failed = Vec::new();
        for def in &s3dp_core::registry().effects {
            for (target, on_mask) in [(Target::Content, false), (Target::Mask, true)] {
                if !def.targets.contains(&target) {
                    continue;
                }
                let e = def.instantiate(1);
                let items = if on_mask {
                    Items::Mask(vec![MaskItem { id: 2, enabled: true, opacity: 1.0, blend: e.blend, kind: MaskItemKind::Effect(e) }])
                } else {
                    Items::Content(vec![e])
                };
                scene.build(&PreviewStack { items, base: None });
                scene.vp.render(&device, &queue, [48, 48], &scene.cam, ShadingMode::Lit);
                let errs: Vec<String> = scene.vp.paint.errors.values().cloned().collect();
                if !errs.is_empty() {
                    failed.push(format!("{} ({}): {}", def.id, if on_mask { "mask" } else { "content" }, errs.join("; ")));
                }
            }
        }
        assert!(failed.is_empty(), "preview failures:\n{}", failed.join("\n"));
    }

    /// Contact sheet of a few previews (`cargo test -p stylized3dpaint sample_sheet -- --ignored`).
    #[test]
    #[ignore]
    fn sample_sheet() {
        let instance = egui_wgpu::wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&Default::default())) else { return };
        let mut frames = Vec::new();
        for (shape, id, on_mask) in [(Shape::Cube, "metal_edge_wear", false), (Shape::Sphere, "slope_blur", false), (Shape::Cylinder, "noise", true), (Shape::Cube, "stylize_master", false)] {
            let mut scene = SampleScene::new(&device, &queue, shape);
            let def = s3dp_core::registry().get(id).unwrap();
            let e = def.instantiate(1);
            let items = if on_mask { Items::Mask(vec![MaskItem { id: 2, enabled: true, opacity: 1.0, blend: e.blend, kind: MaskItemKind::Effect(e) }]) } else { Items::Content(vec![e]) };
            scene.build(&PreviewStack { items, base: None });
            for _ in 0..2 {
                scene.vp.render(&device, &queue, [240, 240], &scene.cam, ShadingMode::Lit);
            }
            frames.push(scene.vp.read_color(&device, &queue).unwrap().2);
        }
        let (w, h) = (240usize, 240usize);
        let mut out = vec![0u8; w * frames.len() * h * 4];
        for (k, f) in frames.iter().enumerate() {
            for y in 0..h {
                let o = (y * w * frames.len() + k * w) * 4;
                out[o..o + w * 4].copy_from_slice(&f[y * w * 4..(y + 1) * w * 4]);
            }
        }
        let path = std::env::temp_dir().join("s3dp_sample_sheet.png");
        image::save_buffer(&path, &out, (w * frames.len()) as u32, h as u32, image::ColorType::Rgba8).unwrap();
        println!("{}", path.display());
    }

    /// Stack preview check image: previewing the middle effect vs the top one.
    #[test]
    #[ignore]
    fn stack_sheet() {
        let instance = egui_wgpu::wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&Default::default())) else { return };
        let fx = |id: u64, def: &str| s3dp_core::registry().get(def).unwrap().instantiate(id);
        let mut node = s3dp_core::Node::new(1, "Fill", NodeKind::Fill(ChannelValues::defaults()));
        let mut post = fx(13, "posterize");
        post.params.insert("levels".into(), ParamValue::Int(3));
        node.effects = vec![fx(10, "noise"), fx(12, "slope_blur"), post];
        let mut frames = Vec::new();
        let mut scene = SampleScene::new(&device, &queue, Shape::Sphere);
        for id in [10u64, 12, 13] {
            let e = node.effects.iter().find(|e| e.id == id).unwrap().clone();
            scene.build(&PreviewStack::of(&node, id, &e).unwrap());
            for _ in 0..2 {
                scene.vp.render(&device, &queue, [200, 200], &scene.cam, ShadingMode::Lit);
            }
            frames.push(scene.vp.read_color(&device, &queue).unwrap().2);
        }
        let (w, h) = (200usize, 200usize);
        let mut out = vec![0u8; w * frames.len() * h * 4];
        for (k, f) in frames.iter().enumerate() {
            for y in 0..h {
                let o = (y * w * frames.len() + k * w) * 4;
                out[o..o + w * 4].copy_from_slice(&f[y * w * 4..(y + 1) * w * 4]);
            }
        }
        let path = std::env::temp_dir().join("s3dp_stack_sheet.png");
        image::save_buffer(&path, &out, (w * frames.len()) as u32, h as u32, image::ColorType::Rgba8).unwrap();
        println!("{}", path.display());
    }
}
