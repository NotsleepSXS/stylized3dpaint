//! Effect preview on a sample shape (sphere / cube / cylinder): a small separate scene with
//! its own document, baked maps and camera, so the preview shows how the effect works on a
//! neutral object instead of the model being edited. Drag the image to orbit.

use egui_phosphor::regular as icon;
use s3dp_core::effect_def::EffectDef;
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
    tex: Option<egui::TextureId>,
}

impl SampleScene {
    fn new(device: &egui_wgpu::wgpu::Device, queue: &egui_wgpu::wgpu::Queue, shape: Shape) -> Self {
        let mut s = Self { vp: Viewport::new(device), shape, cam: OrbitCamera { yaw: 0.7, pitch: 0.45, ..Default::default() }, built: None, tex: None };
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

    /// Rebuilds the sample document for `effect` (content effect, mask effect or effect layer).
    fn build(&mut self, def: &EffectDef, effect: &EffectInstance, on_mask: bool, layer: bool) {
        let paint = &mut self.vp.paint;
        paint.reset_document();
        let mut copy = effect.clone();
        copy.id = paint.new_id();
        copy.anchor = false;
        copy.enabled = true;
        // Filters need something to work on: a noise pattern before them.
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
        let needs_input = !def.category.is_generator();
        let mut base = ChannelValues::defaults();
        base.set(ChannelId::BaseColor, [0.62, 0.36, 0.24]);
        base.set(ChannelId::Roughness, [0.55; 3]);
        if on_mask {
            // Light layer over a dark base, revealed by the effect's mask.
            let mut dark = ChannelValues::defaults();
            dark.set(ChannelId::BaseColor, [0.06, 0.07, 0.09]);
            paint.add_node(NodeKind::Fill(dark), |_, _| {});
            let mut light = ChannelValues::defaults();
            light.set(ChannelId::BaseColor, [0.85, 0.78, 0.68]);
            let mut items = Vec::new();
            let mut push = |paint: &mut s3dp_render::Painter, kind: MaskItemKind| {
                let id = paint.new_id();
                items.push(MaskItem { id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind });
            };
            push(paint, MaskItemKind::Fill(if needs_input { 0.0 } else { 1.0 }));
            if needs_input && let Some(n) = pattern(paint, true) {
                push(paint, MaskItemKind::Effect(n));
            }
            let blend = copy.blend;
            let id = paint.new_id();
            items.push(MaskItem { id, enabled: true, opacity: 1.0, blend: if needs_input { BlendMode::Normal } else { blend }, kind: MaskItemKind::Effect(copy) });
            paint.add_node(NodeKind::Fill(light), |n, _| n.mask = Some(Mask { enabled: true, items }));
        } else if layer {
            let p = pattern(paint, false);
            paint.add_node(NodeKind::Fill(base), |n, _| n.effects.extend(p));
            paint.add_node(NodeKind::Effect(copy), |_, _| {});
        } else {
            let p = if needs_input { pattern(paint, false) } else { None };
            paint.add_node(NodeKind::Fill(base), |n, _| {
                n.effects.extend(p);
                n.effects.push(copy);
            });
        }
    }
}

/// Preview of `effect` on the sample shape with the shape switch; `layer` = effect layer.
pub fn show(app: &mut crate::app::App, ui: &mut egui::Ui, def: &EffectDef, effect: &EffectInstance, on_mask: bool, layer: bool) {
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
    let key = format!("{on_mask}{layer}{:?}{:?}{:?}{}{:?}", effect.params, effect.blend, effect.channels, effect.opacity, effect.def);
    if scene.built.as_deref() != Some(key.as_str()) {
        scene.build(def, effect, on_mask, layer);
        scene.built = Some(key);
    }
    let w = ui.available_width().clamp(80.0, 420.0);
    let h = (w * 0.7).round();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        scene.cam.yaw += d.x * 0.01;
        scene.cam.pitch = (scene.cam.pitch + d.y * 0.01).clamp(-1.5, 1.5);
    }
    let ppp = ui.ctx().pixels_per_point();
    let size = [(w * ppp).round() as u32, (h * ppp).round() as u32];
    let (view, recreated) = scene.vp.render(&rs.device, &rs.queue, size, &scene.cam, ShadingMode::Lit);
    let mut renderer = rs.renderer.write();
    let tex = match scene.tex {
        Some(id) => {
            if recreated {
                renderer.update_egui_texture_from_wgpu_texture(&rs.device, view, egui_wgpu::wgpu::FilterMode::Linear, id);
            }
            id
        }
        None => {
            let id = renderer.register_native_texture(&rs.device, view, egui_wgpu::wgpu::FilterMode::Linear);
            scene.tex = Some(id);
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
    resp.on_hover_text(t("Drag to rotate"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use s3dp_core::effect_def::Target;

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
                scene.build(def, &e, on_mask, false);
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
            scene.build(def, &def.instantiate(1), on_mask, false);
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
}
