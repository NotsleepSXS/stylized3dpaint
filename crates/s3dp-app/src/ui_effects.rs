//! Mask effect stack editor, Stylize filter editor and smart layer presets.

use egui::RichText;
use egui_phosphor::regular as icon;
use s3dp_core::{
    BakedMap, BlendMode, ChannelMask, ChannelValues, EffectKind, GeneratorSource, LayerKind, Levels, MaskBlend,
    MaskEffect, NoiseKind, NoiseParams, StylizeParams, WarpMode, WarpParams,
};

use crate::app::App;
use crate::theme;
use crate::ui_panels::starts_edit;

/// Collects whether widgets changed something and whether that starts a new undo step.
#[derive(Default)]
struct Edit {
    changed: bool,
    checkpoint: bool,
}

impl Edit {
    fn note(&mut self, r: &egui::Response) {
        if r.changed() {
            self.changed = true;
            self.checkpoint |= starts_edit(r);
        }
    }

    fn structural(&mut self) {
        self.changed = true;
        self.checkpoint = true;
    }
}

fn slider(ui: &mut egui::Ui, edit: &mut Edit, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.label(label);
    let r = ui.add(egui::Slider::new(v, range).max_decimals(3));
    edit.note(&r);
    ui.end_row();
}

fn slider_u32(ui: &mut egui::Ui, edit: &mut Edit, label: &str, v: &mut u32, range: std::ops::RangeInclusive<u32>) {
    ui.label(label);
    let r = ui.add(egui::Slider::new(v, range));
    edit.note(&r);
    ui.end_row();
}

fn noise_editor(ui: &mut egui::Ui, edit: &mut Edit, n: &mut NoiseParams) {
    ui.label("Noise");
    let mut kind = n.kind;
    egui::ComboBox::from_id_salt(ui.id().with("noise_kind")).selected_text(kind.label()).show_ui(ui, |ui| {
        for k in NoiseKind::ALL {
            ui.selectable_value(&mut kind, k, k.label());
        }
    });
    if kind != n.kind {
        n.kind = kind;
        edit.structural();
    }
    ui.end_row();
    slider(ui, edit, "Scale", &mut n.scale, 0.005..=1.0);
    if n.kind != NoiseKind::Cellular {
        slider_u32(ui, edit, "Octaves", &mut n.octaves, 1..=8);
    }
    if n.kind == NoiseKind::Strokes {
        slider(ui, edit, "Stretch", &mut n.stretch, 1.0..=12.0);
    }
    ui.label("Seed");
    let r = ui.add(egui::DragValue::new(&mut n.seed).range(0..=9999));
    edit.note(&r);
    ui.end_row();
}

fn levels_editor(ui: &mut egui::Ui, edit: &mut Edit, l: &mut Levels) {
    slider(ui, edit, "Low", &mut l.low, 0.0..=1.0);
    slider(ui, edit, "High", &mut l.high, 0.0..=1.0);
    slider(ui, edit, "Gamma", &mut l.gamma, 0.1..=4.0);
    slider_u32(ui, edit, "Bands", &mut l.steps, 0..=12);
    ui.label("Invert");
    let r = ui.checkbox(&mut l.invert, "");
    edit.note(&r);
    ui.end_row();
}

fn effect_body(ui: &mut egui::Ui, edit: &mut Edit, kind: &mut EffectKind) {
    crate::ui_panels::fit_sliders(ui, 100.0);
    egui::Grid::new(ui.id().with("grid")).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| match kind {
        EffectKind::Generator { source, levels, blend, opacity } => {
            match source {
                GeneratorSource::Noise(n) => noise_editor(ui, edit, n),
                GeneratorSource::HeightGradient { axis } => {
                    ui.label("Axis");
                    ui.horizontal(|ui| {
                        for (i, name) in ["X", "Y", "Z"].iter().enumerate() {
                            if ui.selectable_label(*axis == i as u32, *name).clicked() && *axis != i as u32 {
                                *axis = i as u32;
                                edit.structural();
                            }
                        }
                    });
                    ui.end_row();
                }
                GeneratorSource::Map(_) => {}
            }
            ui.label("Blend");
            let mut b = *blend;
            egui::ComboBox::from_id_salt(ui.id().with("mask_blend")).selected_text(b.label()).show_ui(ui, |ui| {
                for m in MaskBlend::ALL {
                    ui.selectable_value(&mut b, m, m.label());
                }
            });
            if b != *blend {
                *blend = b;
                edit.structural();
            }
            ui.end_row();
            slider(ui, edit, "Opacity", opacity, 0.0..=1.0);
            levels_editor(ui, edit, levels);
        }
        EffectKind::Warp(w) => {
            ui.label("Mode");
            let mut mode = w.mode;
            egui::ComboBox::from_id_salt(ui.id().with("warp_mode")).selected_text(mode.label()).show_ui(ui, |ui| {
                for m in WarpMode::ALL {
                    ui.selectable_value(&mut mode, m, m.label());
                }
            });
            if mode != w.mode {
                w.mode = mode;
                edit.structural();
            }
            ui.end_row();
            slider(ui, edit, "Intensity", &mut w.intensity, 0.0..=0.3);
            if w.mode != WarpMode::Warp {
                slider_u32(ui, edit, "Steps", &mut w.samples, 2..=48);
                slider(ui, edit, "Sharpness", &mut w.sharpness, 0.0..=1.0);
            }
            if w.mode == WarpMode::Directional {
                let d = glam::Vec3::from(w.direction).normalize_or(glam::Vec3::NEG_Y);
                let mut elevation = d.y.asin().to_degrees();
                let mut azimuth = d.x.atan2(d.z).to_degrees();
                ui.label("Elevation (°)");
                let r1 = ui.add(egui::Slider::new(&mut elevation, -90.0..=90.0));
                ui.end_row();
                ui.label("Azimuth (°)");
                let r2 = ui.add(egui::Slider::new(&mut azimuth, -180.0..=180.0));
                ui.end_row();
                if r1.changed() || r2.changed() {
                    let (e, a) = (elevation.to_radians(), azimuth.to_radians());
                    w.direction = [e.cos() * a.sin(), e.sin(), e.cos() * a.cos()];
                }
                edit.note(&r1);
                edit.note(&r2);
            }
            noise_editor(ui, edit, &mut w.noise);
        }
        EffectKind::Blur { radius } => slider(ui, edit, "Radius (texels)", radius, 0.0..=48.0),
        EffectKind::Levels(l) => levels_editor(ui, edit, l),
    });
}

fn add_effect_menu(ui: &mut egui::Ui) -> Option<EffectKind> {
    let mut added = None;
    ui.menu_button(format!("{} Add effect", icon::PLUS), |ui| {
        ui.label(RichText::new("Generators").color(theme::TEXT_FAINT).small());
        for m in [
            BakedMap::Edges,
            BakedMap::Cavity,
            BakedMap::AmbientOcclusion,
            BakedMap::SkyLight,
            BakedMap::Curvature,
            BakedMap::Thickness,
        ] {
            if ui.button(m.label()).clicked() {
                added = Some(EffectKind::generator(GeneratorSource::Map(m)));
            }
        }
        if ui.button("Height gradient").clicked() {
            added = Some(EffectKind::generator(GeneratorSource::HeightGradient { axis: 1 }));
        }
        if ui.button("Noise").clicked() {
            added = Some(EffectKind::generator(GeneratorSource::Noise(NoiseParams::default())));
        }
        ui.separator();
        ui.label(RichText::new("Filters").color(theme::TEXT_FAINT).small());
        for mode in WarpMode::ALL {
            if ui.button(format!("{} {}", icon::WAVES, mode.label())).clicked() {
                added = Some(EffectKind::Warp(WarpParams { mode, ..Default::default() }));
            }
        }
        if ui.button("Blur").clicked() {
            added = Some(EffectKind::Blur { radius: 6.0 });
        }
        if ui.button("Levels").clicked() {
            added = Some(EffectKind::Levels(Levels::default()));
        }
        if added.is_some() {
            ui.close();
        }
    });
    added
}

/// Mask effect stack of the active layer.
pub fn effects_editor(app: &mut App, ui: &mut egui::Ui) {
    let index = app.viewport.paint.active;
    let Some(layer) = app.viewport.paint.active_layer().cloned() else { return };
    let mut effects = layer.effects.clone();
    let mut edit = Edit::default();
    let mut remove = None;
    let mut swap = None;
    let count = effects.len();
    // The add button comes first so it is always within reach.
    let added = add_effect_menu(ui);
    if count == 0 {
        ui.label(
            RichText::new("Generators and filters shape this layer's mask. Bake mesh maps for Edges / Cavity / AO / Sky.")
                .color(theme::TEXT_FAINT)
                .small(),
        );
    }
    for (i, effect) in effects.iter_mut().enumerate() {
        egui::Frame::new()
            .fill(theme::FIELD)
            .corner_radius(3)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let id = ui.id().with((layer.id, i));
                let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
                state
                    .show_header(ui, |ui| {
                        let r = ui.checkbox(&mut effect.enabled, "");
                        if r.changed() {
                            edit.structural();
                        }
                        let glyph = match effect.kind {
                            EffectKind::Generator { .. } => icon::SPARKLE,
                            EffectKind::Warp(_) => icon::WAVES,
                            EffectKind::Blur { .. } => icon::DROP,
                            EffectKind::Levels(_) => icon::SLIDERS_HORIZONTAL,
                        };
                        ui.label(RichText::new(format!("{glyph} {}", effect.kind.label())).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = |ui: &mut egui::Ui, g: &str, tip: &str| ui.add(egui::Button::new(g).frame(false)).on_hover_text(tip);
                            if b(ui, icon::X, "Remove").clicked() {
                                remove = Some(i);
                            }
                            if i + 1 < count && b(ui, icon::CARET_DOWN, "Move down").clicked() {
                                swap = Some((i, i + 1));
                            }
                            if i > 0 && b(ui, icon::CARET_UP, "Move up").clicked() {
                                swap = Some((i - 1, i));
                            }
                        });
                    })
                    .body(|ui| effect_body(ui, &mut edit, &mut effect.kind));
            });
        ui.add_space(3.0);
    }
    if let Some(i) = remove {
        effects.remove(i);
        edit.structural();
    }
    if let Some((a, b)) = swap {
        effects.swap(a, b);
        edit.structural();
    }
    if let Some(kind) = added {
        effects.push(MaskEffect::new(kind));
        edit.structural();
    }
    if edit.changed {
        app.viewport.paint.edit_layer(index, edit.checkpoint, |l| l.effects = effects);
    }
}

/// Parameters of a Stylize filter layer.
pub fn stylize_editor(app: &mut App, ui: &mut egui::Ui, params: &StylizeParams) {
    let index = app.viewport.paint.active;
    let mut p = *params;
    let mut edit = Edit::default();
    crate::ui_panels::fit_sliders(ui, 115.0);
    egui::Grid::new("stylize").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new("Color").strong());
        ui.end_row();
        slider(ui, &mut edit, "Painterly", &mut p.painterly, 0.0..=8.0);
        slider_u32(ui, &mut edit, "Value bands", &mut p.color_bands, 0..=8);
        slider(ui, &mut edit, "Band strength", &mut p.band_strength, 0.0..=1.0);
        slider(ui, &mut edit, "Saturation", &mut p.saturation, 0.0..=2.0);
        slider(ui, &mut edit, "Warm / cool", &mut p.temperature, 0.0..=1.0);
        slider(ui, &mut edit, "Edge highlight", &mut p.edge_highlight, 0.0..=1.0);
        slider(ui, &mut edit, "Cavity darken", &mut p.cavity_darken, 0.0..=1.0);
        ui.label(RichText::new("Material").strong());
        ui.end_row();
        slider_u32(ui, &mut edit, "Roughness bands", &mut p.roughness_bands, 0..=8);
        slider(ui, &mut edit, "Edge polish", &mut p.edge_polish, 0.0..=1.0);
        slider(ui, &mut edit, "Metal threshold", &mut p.metal_threshold, 0.0..=1.0);
        slider(ui, &mut edit, "Height bevel", &mut p.height_bevel, 0.0..=1.0);
        slider(ui, &mut edit, "Height smooth", &mut p.height_smooth, 0.0..=8.0);
    });
    if edit.changed {
        app.viewport.paint.edit_layer(index, edit.checkpoint, |l| l.kind = LayerKind::Stylize(p));
    }
}

fn generator(source: GeneratorSource, low: f32, high: f32, blend: MaskBlend) -> MaskEffect {
    MaskEffect::new(EffectKind::Generator { source, levels: Levels { low, high, ..Default::default() }, blend, opacity: 1.0 })
}

/// Smart layers: fill layers with ready-made mask effects.
pub fn presets_menu(app: &mut App, ui: &mut egui::Ui) {
    let paint = &mut app.viewport.paint;
    let fill = |c: [f32; 3]| LayerKind::Fill(ChannelValues { base_color: c, ..ChannelValues::DEFAULTS });
    if ui.button(format!("{} Stylize filter", icon::MAGIC_WAND)).on_hover_text("Stylizes everything below it").clicked() {
        paint.add_layer(LayerKind::Stylize(StylizeParams::default()));
        ui.close();
    }
    ui.separator();
    if ui.button("Edge highlight").on_hover_text("Painted light on convex edges").clicked() {
        paint.add_layer_with(fill([1.0, 0.93, 0.75]), |l| {
            l.name = "Edge highlight".into();
            l.channels = ChannelMask::BASE_COLOR;
            l.opacity = 0.85;
            l.effects = vec![
                generator(GeneratorSource::Map(BakedMap::Edges), 0.1, 0.5, MaskBlend::Replace),
                MaskEffect::new(EffectKind::Warp(WarpParams {
                    intensity: 0.012,
                    noise: NoiseParams { scale: 0.04, ..Default::default() },
                    ..Default::default()
                })),
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
        ui.close();
    }
    if ui.button("Cavity dirt").on_hover_text("Dark grime in creases").clicked() {
        paint.add_layer_with(fill([0.18, 0.12, 0.08]), |l| {
            l.name = "Cavity dirt".into();
            l.channels = ChannelMask::BASE_COLOR;
            l.blend = BlendMode::Multiply;
            l.opacity = 0.8;
            l.effects = vec![
                generator(GeneratorSource::Map(BakedMap::Cavity), 0.05, 0.5, MaskBlend::Replace),
                generator(GeneratorSource::Noise(NoiseParams { scale: 0.1, ..Default::default() }), 0.3, 0.7, MaskBlend::Multiply),
                MaskEffect::new(EffectKind::Warp(WarpParams { intensity: 0.01, ..Default::default() })),
            ];
        });
        ui.close();
    }
    if ui.button("Top light").on_hover_text("Hand-painted light from above").clicked() {
        paint.add_layer_with(fill([1.0, 0.92, 0.7]), |l| {
            l.name = "Top light".into();
            l.channels = ChannelMask::BASE_COLOR;
            l.blend = BlendMode::Overlay;
            l.opacity = 0.6;
            l.effects = vec![
                generator(GeneratorSource::Map(BakedMap::SkyLight), 0.0, 1.0, MaskBlend::Replace),
                generator(GeneratorSource::HeightGradient { axis: 1 }, 0.2, 1.0, MaskBlend::Multiply),
            ];
        });
        ui.close();
    }
    if ui.button("Bottom shadow").on_hover_text("Cool gradient darkening towards the bottom").clicked() {
        paint.add_layer_with(fill([0.3, 0.25, 0.5]), |l| {
            l.name = "Bottom shadow".into();
            l.channels = ChannelMask::BASE_COLOR;
            l.blend = BlendMode::Multiply;
            l.opacity = 0.5;
            let mut g = generator(GeneratorSource::HeightGradient { axis: 1 }, 0.0, 0.6, MaskBlend::Replace);
            if let EffectKind::Generator { levels, .. } = &mut g.kind {
                levels.invert = true;
            }
            l.effects = vec![g];
        });
        ui.close();
    }
}
