//! "Bake Mesh Maps" window: detail source, realistic vs stylized style, advanced settings.

use std::path::PathBuf;

use egui::RichText;
use egui_phosphor::regular as icon;
use s3dp_core::{BakeSettings, BakeStyle, Mesh, import};

use crate::app::App;
use crate::i18n::t;
use crate::theme;

/// Bake UI state kept by the app.
#[derive(Default)]
pub struct BakeUi {
    pub open: bool,
    pub settings: BakeSettings,
    /// Loaded high-poly mesh, cached by path.
    high_poly: Option<(PathBuf, Mesh)>,
    pub has_baked: bool,
}

impl BakeUi {
    /// Loads (or reuses) the high-poly mesh for the current settings.
    fn high_poly(&mut self) -> anyhow::Result<Option<&Mesh>> {
        let Some(path) = self.settings.high_poly.clone() else { return Ok(None) };
        if self.high_poly.as_ref().is_none_or(|(p, _)| *p != path) {
            let mesh = import::load(&path)?;
            self.high_poly = Some((path, mesh));
        }
        Ok(self.high_poly.as_ref().map(|(_, m)| m))
    }
}

fn style_card(ui: &mut egui::Ui, width: f32, selected: bool, title: &str, text: &str) -> bool {
    let fill = if selected { theme::ACCENT_SOFT } else { theme::FIELD };
    let stroke = if selected { theme::ACCENT } else { theme::FIELD_BORDER };
    let response = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(4)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width - 18.0);
                ui.set_min_height(86.0);
                ui.label(RichText::new(title).strong().size(13.5));
                ui.add(egui::Label::new(RichText::new(text).color(theme::TEXT_DIM).small()).wrap());
            });
        })
        .response
        .interact(egui::Sense::click());
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn slider(ui: &mut egui::Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, tip: &str) {
    ui.label(label).on_hover_text(tip);
    ui.add(egui::Slider::new(value, range).max_decimals(3));
    ui.end_row();
}

fn samples(ui: &mut egui::Ui, label: &str, value: &mut u32) {
    ui.label(label);
    ui.add(egui::Slider::new(value, 4..=512).logarithmic(true));
    ui.end_row();
}

pub fn bake_window(app: &mut App, ctx: &egui::Context) {
    if !app.bake.open {
        return;
    }
    let mut open = true;
    let mut start = false;
    egui::Window::new(format!("{} {}", icon::CUBE, t("Bake Mesh Maps")))
        .open(&mut open)
        .resizable(false)
        .default_width(440.0)
        .max_width(440.0)
        .collapsible(false)
        .show(ctx, |ui| {
            let s = &mut app.bake.settings;
            ui.spacing_mut().item_spacing.y = 6.0;

            ui.label(RichText::new(t("Detail source")).strong());
            let mut use_high = s.high_poly.is_some();
            ui.radio_value(&mut use_high, false, t("This mesh — no high-poly"));
            ui.radio_value(&mut use_high, true, t("High-poly model — transfers sculpted detail + normal map"));
            if use_high && s.high_poly.is_none() {
                s.high_poly = pick_high_poly();
            }
            if !use_high {
                s.high_poly = None;
            }
            if let Some(path) = s.high_poly.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::FILE).color(theme::TEXT_DIM));
                    ui.label(RichText::new(path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())).monospace());
                    if ui.button(t("Browse…")).clicked()
                        && let Some(p) = pick_high_poly()
                    {
                        s.high_poly = Some(p);
                    }
                });
                egui::Grid::new("cage").num_columns(2).show(ui, |ui| {
                    slider(ui, t("Search in front"), &mut s.cage_front, 0.0..=0.2, t("How far outside the low-poly to look for the high-poly (relative to model size)"));
                    slider(ui, t("Search behind"), &mut s.cage_back, 0.0..=0.2, t("How far inside the low-poly to look for the high-poly"));
                });
            }

            ui.separator();
            ui.label(RichText::new(t("Style")).strong());
            let card_width = (ui.available_width() - 8.0) / 2.0;
            ui.horizontal(|ui| {
                if style_card(
                    ui,
                    card_width,
                    s.style == BakeStyle::Stylized,
                    &format!("{} {}", icon::PALETTE, t("Stylized")),
                    t("Soft wide edges and cavities, painterly AO, baked sky light with soft shadows, smoothed — masks that layer nicely for hand-painted looks."),
                ) {
                    *s = s.with_style(BakeStyle::Stylized);
                }
                if style_card(
                    ui,
                    card_width,
                    s.style == BakeStyle::Realistic,
                    &format!("{} {}", icon::CAMERA, t("Realistic")),
                    t("Sharp contact AO, thin accurate curvature, no smoothing — closer to classic PBR bakers."),
                ) {
                    *s = s.with_style(BakeStyle::Realistic);
                }
            });

            ui.collapsing(t("Advanced"), |ui| {
                egui::Grid::new("bake_advanced").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    slider(ui, t("AO distance"), &mut s.ao_distance, 0.01..=1.0, t("Occlusion search distance (relative to model size)"));
                    samples(ui, t("AO samples"), &mut s.ao_samples);
                    slider(ui, t("Edge width"), &mut s.edge_width, 0.001..=0.1, t("Width of edge highlights and cavities (relative to model size)"));
                    samples(ui, t("Edge samples"), &mut s.edge_samples);
                    slider(ui, t("Thickness distance"), &mut s.thickness_distance, 0.05..=1.0, "");
                    samples(ui, t("Thickness samples"), &mut s.thickness_samples);
                    samples(ui, t("Sky samples"), &mut s.sky_samples);
                    slider(ui, t("Sky softness (°)"), &mut s.sky_softness, 1.0..=89.0, t("Bigger = softer shadows"));
                    let d = glam::Vec3::from(s.sky_direction).normalize_or(glam::Vec3::Y);
                    let mut elevation = d.y.asin().to_degrees();
                    let mut azimuth = d.x.atan2(d.z).to_degrees();
                    let mut changed = false;
                    ui.label(t("Sky elevation (°)"));
                    changed |= ui.add(egui::Slider::new(&mut elevation, 0.0..=90.0)).changed();
                    ui.end_row();
                    ui.label(t("Sky azimuth (°)"));
                    changed |= ui.add(egui::Slider::new(&mut azimuth, -180.0..=180.0)).changed();
                    ui.end_row();
                    if changed {
                        let (e, a) = (elevation.to_radians(), azimuth.to_radians());
                        s.sky_direction = [e.cos() * a.sin(), e.sin(), e.cos() * a.cos()];
                    }
                    slider(ui, t("Smoothing"), &mut s.smoothing, 0.0..=0.1, t("Geometry-aware blur that removes noise and faceting (relative to model size)"));
                });
                if ui.button(t("Reset to preset")).clicked() {
                    *s = s.with_style(s.style);
                }
            });

            ui.separator();
            let size = app.texture_size;
            let sets = app.viewport.paint.sets.len();
            ui.label(
                RichText::new(format!("{sets} {} {size}×{size}. {}", t("texture set(s) at"), t("Results appear under View › Mesh maps.")))
                    .color(theme::TEXT_DIM)
                    .small(),
            );
            ui.horizontal(|ui| {
                match app.viewport.bake_progress() {
                    Some(p) => {
                        ui.add(egui::ProgressBar::new(p).show_percentage().desired_width(300.0));
                        if ui.button(t("Cancel")).clicked() {
                            app.viewport.cancel_bake();
                        }
                    }
                    None => {
                        let label = RichText::new(format!("{} {}", icon::FIRE, t("Bake"))).strong();
                        if ui.add(egui::Button::new(label).fill(theme::ACCENT).min_size(egui::vec2(110.0, 28.0))).clicked() {
                            start = true;
                        }
                    }
                }
            });
        });
    app.bake.open = open;
    if start {
        start_bake(app);
    }
}

fn pick_high_poly() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(t("High-poly model"))
        .add_filter("3D models", import::SUPPORTED_EXTENSIONS)
        .pick_file()
}

pub fn start_bake(app: &mut App) {
    if !app.mesh.has_uvs() {
        app.status = t("The mesh has no UVs — nothing to bake into").to_owned();
        return;
    }
    let high = match app.bake.high_poly() {
        Ok(h) => h.cloned(),
        Err(e) => {
            app.status = format!("{}: {e:#}", t("Could not load the high-poly"));
            return;
        }
    };
    let rs = app.render_state.clone();
    let settings = app.bake.settings.clone();
    app.viewport.start_bake(&rs.device, &app.mesh, high.as_ref(), &settings);
    app.status = t("Baking mesh maps…").to_owned();
}
