//! Photoshop-like dark theme. Palette adapted from PhotoCraft's "Pro" theme
//! (MIT OR Apache-2.0, github.com/storytold/photocraft).

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

pub const CHROME: Color32 = Color32::from_gray(50);
pub const DOCK: Color32 = Color32::from_gray(38);
pub const CARD: Color32 = Color32::from_gray(50);
pub const FIELD: Color32 = Color32::from_gray(36);
pub const FIELD_BORDER: Color32 = Color32::from_gray(74);
pub const HOVER: Color32 = Color32::from_gray(66);
pub const PRESSED: Color32 = Color32::from_gray(78);
pub const TEXT: Color32 = Color32::from_gray(222);
pub const TEXT_DIM: Color32 = Color32::from_gray(178);
pub const TEXT_FAINT: Color32 = Color32::from_gray(128);
pub const ACCENT: Color32 = Color32::from_rgb(55, 142, 240);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(40, 70, 105);
pub const SEPARATOR: Color32 = Color32::from_gray(30);
pub const ROW_SELECTED: Color32 = Color32::from_gray(82);
pub const DANGER: Color32 = Color32::from_rgb(236, 91, 98);
pub const WARNING: Color32 = Color32::from_rgb(232, 176, 70);

pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = CHROME;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, SEPARATOR);
    v.extreme_bg_color = FIELD;
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(4);
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 32, spread: 0, color: Color32::from_black_alpha(150) };
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(150) };
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0, SEPARATOR);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    for (state, fill) in [(&mut w.inactive, FIELD), (&mut w.hovered, HOVER), (&mut w.active, PRESSED), (&mut w.open, HOVER)] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, FIELD_BORDER);
        state.fg_stroke = Stroke::new(1.0, TEXT);
        state.corner_radius = CornerRadius::same(3);
        state.expansion = 0.0;
    }
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(60));

    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        s.text_styles = [
            (TextStyle::Heading, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(10.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size = egui::vec2(24.0, 22.0);
        s.spacing.slider_width = 140.0;
        s.spacing.combo_width = 120.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.interaction.tooltip_delay = 0.35;
    });
}
