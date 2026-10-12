//! Menu bar, tool bar, options bar, status bar and the right dock panels.

use egui::{Color32, RichText};
use egui_phosphor::regular as icon;
use s3dp_core::{Channel, ChannelMask, ChannelValues, LayerKind};
use s3dp_render::ShadingMode;

use crate::app::{App, TEXTURE_SIZES, Tool};
use crate::{theme, ui_layers};

pub fn to_color32(c: [f32; 3]) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

pub fn from_color32(c: Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

/// Whether an edit from `response` should start a new undo step: the first frame of a
/// drag, or any non-drag change (click, typing).
pub fn starts_edit(response: &egui::Response) -> bool {
    response.drag_started() || !response.dragged()
}

/// Square icon button; highlighted when `selected`.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, selected: bool, tooltip: &str) -> egui::Response {
    let size = egui::vec2(32.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if selected {
        theme::ACCENT_SOFT
    } else if response.hovered() {
        theme::HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    if selected {
        ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
    }
    let color = if selected { Color32::WHITE } else { theme::TEXT_DIM };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(18.0), color);
    response.on_hover_text(tooltip)
}

/// Small toggle chip (channel toggles).
pub fn chip(ui: &mut egui::Ui, text: &str, on: bool, tooltip: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(11.0), Color32::WHITE);
    let size = egui::vec2(galley.size().x + 12.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = match (on, response.hovered()) {
        (true, _) => theme::ACCENT,
        (false, true) => theme::HOVER,
        (false, false) => theme::FIELD,
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    let color = if on { Color32::WHITE } else { theme::TEXT_FAINT };
    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
    response.on_hover_text(tooltip)
}

pub fn channel_chips(ui: &mut egui::Ui, mask: &mut ChannelMask, what: &str) -> bool {
    let mut changed = false;
    for c in Channel::ALL {
        if chip(ui, c.short(), mask.has(c), &format!("{} — {what}", c.label())).clicked() {
            mask.set(c, !mask.has(c));
            changed = true;
        }
    }
    changed
}

/// Header strip of a dock panel (like Photoshop's panel tabs).
pub fn panel_header(ui: &mut egui::Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::DOCK);
    let tab = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 3.0), egui::vec2(rect.width().min(120.0), 21.0));
    ui.painter().rect_filled(tab, egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, theme::CHROME);
    ui.painter().text(
        tab.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(12.0),
        theme::TEXT,
    );
}

fn panel_body<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::CHROME)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

// ---------------------------------------------------------------------- menu

pub fn menu_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.add(egui::Button::new("New from model…").shortcut_text("Ctrl+N")).clicked() {
                ui.close();
                app.open_model_dialog();
            }
            if ui.add(egui::Button::new("Open project…").shortcut_text("Ctrl+O")).clicked() {
                ui.close();
                app.open_project_dialog();
            }
            ui.separator();
            if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() {
                ui.close();
                app.save(false);
            }
            if ui.add(egui::Button::new("Save as…").shortcut_text("Ctrl+Shift+S")).clicked() {
                ui.close();
                app.save(true);
            }
            ui.separator();
            if ui.add(egui::Button::new("Export textures…").shortcut_text("Ctrl+Shift+E")).clicked() {
                ui.close();
                app.export_dialog();
            }
            ui.separator();
            if ui.add(egui::Button::new("Quit").shortcut_text("Ctrl+Q")).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Edit", |ui| {
            let paint = &app.viewport.paint;
            let (can_undo, can_redo) = (paint.can_undo(), paint.can_redo());
            if ui.add_enabled(can_undo, egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
                app.undo();
            }
            if ui.add_enabled(can_redo, egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z")).clicked() {
                app.redo();
            }
        });
        ui.menu_button("Layer", |ui| ui_layers::layer_menu(app, ui));
        ui.menu_button("View", |ui| {
            for (i, mode) in ShadingMode::ALL.into_iter().enumerate() {
                ui.radio_value(&mut app.shading, mode, format!("{}   F{}", mode.label(), i + 1));
            }
        });
        ui.menu_button("Help", |ui| {
            ui.label(RichText::new("Shortcuts").strong());
            ui.label(
                "B brush · E eraser · I eyedropper\n[ ] brush size · X swap colors · D default colors\n\
                 Alt+LMB / MMB orbit · RMB / Shift+MMB pan · Wheel zoom · F frame\n\
                 Ctrl+Z / Ctrl+Shift+Z undo / redo · Ctrl+J duplicate layer\n\
                 Ctrl+Shift+N new layer · Del delete layer · F1–F8 viewport mode",
            );
        });
    });
}

// ---------------------------------------------------------------------- tool bar

pub fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        for tool in [Tool::Brush, Tool::Eraser, Tool::Eyedropper] {
            if icon_button(ui, tool.icon(), app.tool == tool, tool.label()).clicked() {
                app.tool = tool;
            }
        }
        ui.add_space(12.0);
        color_swatches(app, ui);
    });
}

/// Photoshop-style overlapping foreground / background swatches.
fn color_swatches(app: &mut App, ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 46.0), egui::Sense::hover());
    let fg = egui::Rect::from_min_size(rect.min + egui::vec2(2.0, 2.0), egui::vec2(22.0, 22.0));
    let bg = fg.translate(egui::vec2(11.0, 11.0));
    let bg_resp = ui.interact(bg, ui.id().with("bg_swatch"), egui::Sense::click());
    let fg_resp = ui.interact(fg, ui.id().with("fg_swatch"), egui::Sense::click());
    let painter = ui.painter();
    for (r, c) in [(bg, app.bg_color), (fg, app.fg.base_color)] {
        painter.rect_filled(r, 1.0, to_color32(c));
        painter.rect_stroke(r, 1.0, egui::Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
        painter.rect_stroke(r.shrink(1.0), 1.0, egui::Stroke::new(1.0, Color32::from_white_alpha(90)), egui::StrokeKind::Inside);
    }
    let swap = egui::Rect::from_min_size(rect.min + egui::vec2(25.0, 0.0), egui::vec2(12.0, 12.0));
    let reset = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 34.0), egui::vec2(12.0, 12.0));
    let swap_resp = ui.interact(swap, ui.id().with("swap"), egui::Sense::click()).on_hover_text("Swap colors (X)");
    let reset_resp = ui.interact(reset, ui.id().with("reset"), egui::Sense::click()).on_hover_text("Default colors (D)");
    let small = egui::FontId::proportional(11.0);
    painter.text(swap.center(), egui::Align2::CENTER_CENTER, icon::ARROWS_LEFT_RIGHT, small.clone(), theme::TEXT_DIM);
    painter.text(reset.center(), egui::Align2::CENTER_CENTER, icon::SQUARE_HALF, small, theme::TEXT_DIM);
    if swap_resp.clicked() {
        app.swap_colors();
    }
    if reset_resp.clicked() {
        app.reset_colors();
    }
    fg_resp.on_hover_text("Foreground color — edit it in the Color panel");
    if bg_resp.on_hover_text("Background color — click to swap (X)").clicked() {
        app.swap_colors();
    }
}

// ---------------------------------------------------------------------- options bar

pub fn options_bar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal_centered(|ui| {
        ui.label(RichText::new(app.tool.icon()).size(18.0).color(theme::TEXT));
        ui.separator();
        if app.tool == Tool::Eyedropper {
            ui.label(RichText::new("Click the model to pick every channel of the material under the cursor").color(theme::TEXT_DIM));
            return;
        }
        ui.label("Size");
        ui.add(egui::DragValue::new(&mut app.brush.size).range(1.0..=500.0).speed(0.5).suffix(" px"));
        ui.label("Hardness");
        percent(ui, &mut app.brush.hardness, 0.0);
        ui.label("Opacity");
        percent(ui, &mut app.brush.opacity, 0.0);
        ui.label("Flow");
        percent(ui, &mut app.brush.flow, 0.01);
        ui.separator();
        ui.label("Channels");
        let what = if app.tool == Tool::Eraser { "erased by the eraser" } else { "painted by the brush" };
        channel_chips(ui, &mut app.brush.channels, what);
        let paint = &app.viewport.paint;
        if let Some(layer) = paint.active_layer() {
            ui.separator();
            let target = if layer.has_mask && paint.target == s3dp_render::EditTarget::Mask {
                RichText::new(format!("{} Painting mask of “{}”", icon::SQUARE_HALF, layer.name)).color(theme::ACCENT)
            } else {
                RichText::new(format!("{} Painting “{}”", icon::STACK, layer.name)).color(theme::TEXT_DIM)
            };
            ui.label(target);
        }
    });
}

/// 0..1 value edited as a percentage.
fn percent(ui: &mut egui::Ui, value: &mut f32, min: f32) -> egui::Response {
    let mut pct = *value * 100.0;
    let r = ui.add(egui::DragValue::new(&mut pct).range(min * 100.0..=100.0).speed(0.5).suffix("%").max_decimals(0));
    if r.changed() {
        *value = pct / 100.0;
    }
    r
}

// ---------------------------------------------------------------------- status bar

pub fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&app.status).color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut size = app.texture_size;
            egui::ComboBox::from_id_salt("texture_size")
                .selected_text(format!("{size}×{size}"))
                .width(90.0)
                .show_ui(ui, |ui| {
                    for s in TEXTURE_SIZES {
                        ui.selectable_value(&mut size, s, format!("{s}×{s}"));
                    }
                })
                .response
                .on_hover_text("Texture resolution (changing it resets the layer stack)");
            app.set_texture_size(size);
            ui.label(RichText::new("Texture").color(theme::TEXT_FAINT));
            if app.is_modified() {
                ui.label(RichText::new("● unsaved").color(theme::TEXT_FAINT));
            }
        });
    });
}

// ---------------------------------------------------------------------- dock

pub fn dock(app: &mut App, ui: &mut egui::Ui) {
    // Content must never widen the panel: clip it to the panel rect.
    let width = ui.max_rect().width();
    ui.set_max_width(width);
    ui.spacing_mut().item_spacing.y = 0.0;
    panel_header(ui, "Color");
    panel_body(ui, |ui| color_panel(app, ui));
    panel_header(ui, "Material");
    panel_body(ui, |ui| material_panel(app, ui));
    panel_header(ui, "Properties");
    panel_body(ui, |ui| properties_panel(app, ui));
    panel_header(ui, "Layers");
    egui::Frame::new().fill(theme::CHROME).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        ui_layers::layers_panel(app, ui);
    });
}

fn color_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = 0.0;
    crate::color_picker::picker(ui, &mut app.picker, &mut app.fg.base_color, 130.0);
}

/// Values the brush paints into the non-color channels.
fn material_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = 5.0;
    ui.spacing_mut().slider_width = (ui.max_rect().width() - 150.0).max(60.0);
    let fg = &mut app.fg;
    let channels = &mut app.brush.channels;
    egui::Grid::new("material").num_columns(3).spacing([6.0, 5.0]).show(ui, |ui| {
        let mut row = |ui: &mut egui::Ui, c: Channel, value: &mut f32| {
            let mut on = channels.has(c);
            if ui.checkbox(&mut on, "").on_hover_text("Paint this channel").changed() {
                channels.set(c, on);
            }
            ui.label(c.label());
            ui.add(egui::Slider::new(value, 0.0..=1.0).max_decimals(2));
            ui.end_row();
        };
        row(ui, Channel::Roughness, &mut fg.roughness);
        row(ui, Channel::Metallic, &mut fg.metallic);
        row(ui, Channel::Height, &mut fg.height);
        let mut on = channels.has(Channel::Emissive);
        if ui.checkbox(&mut on, "").on_hover_text("Paint this channel").changed() {
            channels.set(Channel::Emissive, on);
        }
        ui.label("Emissive");
        let mut e = to_color32(fg.emissive);
        if egui::color_picker::color_edit_button_srgba(ui, &mut e, egui::color_picker::Alpha::Opaque).changed() {
            fg.emissive = from_color32(e);
        }
        ui.end_row();
        let mut on = channels.has(Channel::BaseColor);
        if ui.checkbox(&mut on, "").on_hover_text("Paint this channel").changed() {
            channels.set(Channel::BaseColor, on);
        }
        ui.label("Base Color");
        ui.label(RichText::new("from the Color panel").color(theme::TEXT_FAINT));
        ui.end_row();
    });
}

/// Editor for a set of channel values; returns Some(checkpoint) when something changed.
fn values_editor(ui: &mut egui::Ui, id: &str, v: &mut ChannelValues) -> Option<bool> {
    let mut result: Option<bool> = None;
    let mut note = |r: &egui::Response| {
        if r.changed() {
            let start = starts_edit(r);
            result = Some(result.unwrap_or(false) || start);
        }
    };
    ui.spacing_mut().slider_width = (ui.max_rect().width() - 150.0).max(60.0);
    egui::Grid::new(id).num_columns(2).spacing([6.0, 5.0]).show(ui, |ui| {
        ui.label("Base Color");
        let mut c = to_color32(v.base_color);
        let r = egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque);
        if r.changed() {
            v.base_color = from_color32(c);
        }
        note(&r);
        ui.end_row();
        for (label, value) in [("Roughness", &mut v.roughness), ("Metallic", &mut v.metallic), ("Height", &mut v.height)] {
            ui.label(label);
            let r = ui.add(egui::Slider::new(value, 0.0..=1.0).max_decimals(2));
            note(&r);
            ui.end_row();
        }
        ui.label("Emissive");
        let mut e = to_color32(v.emissive);
        let r = egui::color_picker::color_edit_button_srgba(ui, &mut e, egui::color_picker::Alpha::Opaque);
        if r.changed() {
            v.emissive = from_color32(e);
        }
        note(&r);
        ui.end_row();
    });
    result
}

fn properties_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = 5.0;
    let index = app.viewport.paint.active;
    let Some(layer) = app.viewport.paint.active_layer().cloned() else { return };
    ui.horizontal(|ui| {
        let kind = if layer.is_fill() { format!("{} Fill layer", icon::PAINT_BUCKET) } else { format!("{} Paint layer", icon::PAINT_BRUSH) };
        ui.label(RichText::new(kind).strong());
        ui.label(RichText::new(&layer.name).color(theme::TEXT_DIM));
    });
    ui.horizontal(|ui| {
        ui.label("Affects");
        let mut mask = layer.channels;
        if channel_chips(ui, &mut mask, "this layer contributes to it") {
            app.viewport.paint.edit_layer(index, true, |l| l.channels = mask);
        }
    });
    if let LayerKind::Fill(values) = &layer.kind {
        let mut v = *values;
        if let Some(checkpoint) = values_editor(ui, "fill_values", &mut v) {
            app.viewport.paint.edit_layer(index, checkpoint, |l| l.kind = LayerKind::Fill(v));
        }
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new("Base material").id_salt("base_material").show(ui, |ui| {
        ui.label(RichText::new("Values under all layers").color(theme::TEXT_FAINT).small());
        let mut d = app.viewport.paint.defaults;
        if let Some(checkpoint) = values_editor(ui, "defaults", &mut d) {
            app.viewport.paint.set_defaults(d, checkpoint);
        }
    });
    let sets: Vec<String> = app.viewport.paint.sets.iter().map(|s| s.name.clone()).collect();
    if sets.len() > 1 {
        ui.horizontal(|ui| {
            ui.label("Thumbnails of");
            let mut current = app.viewport.paint.thumb_set;
            egui::ComboBox::from_id_salt("thumb_set").selected_text(&sets[current.min(sets.len() - 1)]).show_ui(ui, |ui| {
                for (i, name) in sets.iter().enumerate() {
                    ui.selectable_value(&mut current, i, name);
                }
            });
            if current != app.viewport.paint.thumb_set {
                app.viewport.paint.thumb_set = current;
                app.viewport.paint.invalidate_thumbs();
            }
        });
    }
}
