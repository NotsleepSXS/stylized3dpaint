//! Photoshop-style layers panel: blend mode + opacity, layer rows with visibility,
//! thumbnails and masks, drag-and-drop reordering and a footer toolbar.

use egui::{Color32, RichText, Sense};
use egui_phosphor::regular as icon;
use s3dp_core::{BlendMode, ChannelValues, Layer, LayerKind};
use s3dp_render::EditTarget;

use crate::app::App;
use crate::theme;
use crate::ui_panels::{starts_edit, to_color32};

const ROW_HEIGHT: f32 = 42.0;
const THUMB: f32 = 32.0;
const EYE_WIDTH: f32 = 28.0;

#[derive(Default)]
pub struct LayersUi {
    /// Layer being renamed and the edit buffer.
    renaming: Option<(u64, String)>,
    /// Index (in the painter's bottom-to-top list) of the row being dragged.
    dragging: Option<usize>,
}

/// Actions collected while drawing and applied afterwards (rows only borrow the layers).
enum Action {
    Select(usize, EditTarget),
    ToggleVisible(usize),
    ToggleMask(usize),
    Move { from: usize, to: usize },
    Duplicate(usize),
    Delete(usize),
    AddMask(usize, bool),
    DeleteMask(usize),
    StartRename(usize),
    Rename(usize, String),
}

pub fn layer_menu(app: &mut App, ui: &mut egui::Ui) {
    let active = app.viewport.paint.active;
    if ui.add(egui::Button::new(format!("{} New paint layer", icon::PLUS)).shortcut_text("Ctrl+Shift+N")).clicked() {
        app.viewport.paint.add_layer(LayerKind::Paint);
    }
    if ui.button(format!("{} New fill layer", icon::PAINT_BUCKET)).clicked() {
        app.viewport.paint.add_layer(LayerKind::Fill(fill_from(app)));
    }
    ui.menu_button(format!("{} Smart layers", icon::MAGIC_WAND), |ui| crate::ui_effects::presets_menu(app, ui));
    ui.separator();
    if ui.add(egui::Button::new(format!("{} Duplicate layer", icon::COPY)).shortcut_text("Ctrl+J")).clicked() {
        app.viewport.paint.duplicate_layer(active);
    }
    if ui.add(egui::Button::new(format!("{} Delete layer", icon::TRASH)).shortcut_text("Del")).clicked() {
        app.viewport.paint.delete_layer(active);
    }
    ui.separator();
    let has_mask = app.viewport.paint.active_layer().is_some_and(|l| l.has_mask);
    if ui.add_enabled(!has_mask, egui::Button::new("Add mask (reveal all)")).clicked() {
        app.add_mask(active, true);
    }
    if ui.add_enabled(!has_mask, egui::Button::new("Add mask (hide all)")).clicked() {
        app.add_mask(active, false);
    }
    if ui.add_enabled(has_mask, egui::Button::new("Delete mask")).clicked() {
        app.viewport.paint.delete_mask(active);
    }
}

/// New fill layers start with the foreground material.
fn fill_from(app: &App) -> ChannelValues {
    app.fg
}

pub fn layers_panel(app: &mut App, ui: &mut egui::Ui) {
    let mut actions = Vec::new();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        blend_and_opacity(app, ui);
        // Layer buttons sit at the top, close to the rest of the panel.
        toolbar(app, ui, &mut actions);
    });
    ui.add(egui::Separator::default().spacing(0.0));

    let list_h = ui.available_height().max(60.0);
    egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let count = app.viewport.paint.layers.len();
        let mut row_rects = Vec::with_capacity(count);
        // Top of the stack first, like Photoshop.
        for index in (0..count).rev() {
            let rect = layer_row(app, ui, index, &mut actions);
            row_rects.push((index, rect));
        }
        drag_and_drop(app, ui, &row_rects, &mut actions);
    });
    apply(app, actions);
}

fn blend_and_opacity(app: &mut App, ui: &mut egui::Ui) {
    let index = app.viewport.paint.active;
    let Some(layer) = app.viewport.paint.active_layer().cloned() else { return };
    ui.horizontal(|ui| {
        let mut blend = layer.blend;
        egui::ComboBox::from_id_salt("blend_mode").selected_text(blend.label()).width(130.0).show_ui(ui, |ui| {
            for entry in BlendMode::MENU {
                match entry {
                    Some(mode) => {
                        ui.selectable_value(&mut blend, mode, mode.label());
                    }
                    None => {
                        ui.separator();
                    }
                }
            }
        });
        if blend != layer.blend {
            app.viewport.paint.edit_layer(index, true, |l| l.blend = blend);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut pct = layer.opacity * 100.0;
            let r = ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).speed(0.5).suffix("%").max_decimals(0));
            if r.changed() {
                app.viewport.paint.edit_layer(index, starts_edit(&r), |l| l.opacity = pct / 100.0);
            }
            ui.label("Opacity:");
        });
    });
}

fn checker(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(200));
    let n = 4;
    let cell = rect.width() / n as f32;
    for y in 0..n {
        for x in 0..n {
            if (x + y) % 2 == 0 {
                let r = egui::Rect::from_min_size(rect.min + egui::vec2(x as f32 * cell, y as f32 * cell), egui::vec2(cell, cell));
                painter.rect_filled(r, 0.0, Color32::from_gray(150));
            }
        }
    }
}

/// Draws one layer row; returns its rect for drag-and-drop.
fn layer_row(app: &mut App, ui: &mut egui::Ui, index: usize, actions: &mut Vec<Action>) -> egui::Rect {
    let paint = &app.viewport.paint;
    let layer: Layer = paint.layers[index].meta.clone();
    let selected = index == paint.active;
    let target = paint.target;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);

    let fill = if selected {
        theme::ROW_SELECTED
    } else if response.hovered() {
        theme::HOVER.gamma_multiply(0.5)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 0.0, fill);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0, theme::SEPARATOR));

    // Visibility eye.
    let eye = egui::Rect::from_min_size(rect.min, egui::vec2(EYE_WIDTH, ROW_HEIGHT));
    let eye_resp = ui.interact(eye, ui.id().with(("eye", layer.id)), Sense::click()).on_hover_text("Show / hide layer");
    if layer.visible {
        painter.text(eye.center(), egui::Align2::CENTER_CENTER, icon::EYE, egui::FontId::proportional(15.0), theme::TEXT_DIM);
    }
    painter.line_segment([eye.right_top(), eye.right_bottom()], egui::Stroke::new(1.0, theme::SEPARATOR));
    if eye_resp.clicked() {
        actions.push(Action::ToggleVisible(index));
    }

    // Content thumbnail.
    let mut x = eye.right() + 6.0;
    let thumb_rect = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - THUMB / 2.0), egui::vec2(THUMB, THUMB));
    match &layer.kind {
        LayerKind::Paint => {
            checker(&painter, thumb_rect);
            if let Some(tex) = app.thumb_texture(layer.id, false) {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(tex, thumb_rect, uv, Color32::WHITE);
            }
        }
        LayerKind::Fill(v) => {
            painter.rect_filled(thumb_rect, 0.0, to_color32(v.base_color));
            let lum = v.base_color.iter().sum::<f32>() / 3.0;
            let ink = if lum > 0.5 { Color32::from_black_alpha(160) } else { Color32::from_white_alpha(180) };
            painter.text(thumb_rect.center(), egui::Align2::CENTER_CENTER, icon::PAINT_BUCKET, egui::FontId::proportional(16.0), ink);
        }
        LayerKind::Stylize(_) => {
            painter.rect_filled(thumb_rect, 0.0, theme::ACCENT_SOFT);
            painter.text(thumb_rect.center(), egui::Align2::CENTER_CENTER, icon::MAGIC_WAND, egui::FontId::proportional(17.0), Color32::WHITE);
        }
    }
    painter.rect_stroke(thumb_rect, 0.0, egui::Stroke::new(1.0, Color32::from_gray(20)), egui::StrokeKind::Outside);
    let thumb_resp = ui.interact(thumb_rect, ui.id().with(("thumb", layer.id)), Sense::click());
    if thumb_resp.clicked() {
        actions.push(Action::Select(index, EditTarget::Content));
    }
    x = thumb_rect.right() + 4.0;

    // Mask thumbnail.
    let mut mask_rect = None;
    let generated = layer.effects.iter().any(|e| e.enabled);
    if layer.has_mask || generated {
        let r = egui::Rect::from_min_size(egui::pos2(x, thumb_rect.top()), egui::vec2(THUMB, THUMB));
        painter.rect_filled(r, 0.0, Color32::BLACK);
        if let Some(tex) = app.thumb_texture(layer.id, true) {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex, r, uv, Color32::WHITE);
        }
        painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0, Color32::from_gray(20)), egui::StrokeKind::Outside);
        if !layer.mask_enabled {
            painter.line_segment([r.left_top(), r.right_bottom()], egui::Stroke::new(2.0, theme::DANGER));
            painter.line_segment([r.right_top(), r.left_bottom()], egui::Stroke::new(2.0, theme::DANGER));
        }
        let resp = ui
            .interact(r, ui.id().with(("mask", layer.id)), Sense::click())
            .on_hover_text("Click: paint the mask · Shift+click: disable / enable mask");
        if resp.clicked() {
            if ui.input(|i| i.modifiers.shift) {
                actions.push(Action::ToggleMask(index));
            } else {
                actions.push(Action::Select(index, EditTarget::Mask));
            }
        }
        mask_rect = Some(r);
        x = r.right() + 4.0;
    }

    // Brackets around the thumbnail the brush paints into.
    if selected {
        let target_rect = match (target, mask_rect) {
            (EditTarget::Mask, Some(r)) => r,
            _ => thumb_rect,
        };
        painter.rect_stroke(target_rect.expand(2.0), 1.0, egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
    }

    // Name (or rename editor).
    let name_rect = egui::Rect::from_min_max(egui::pos2(x + 6.0, rect.top()), egui::pos2(rect.right() - 6.0, rect.bottom()));
    let renaming = app.layers_ui.renaming.as_ref().is_some_and(|(id, _)| *id == layer.id);
    if renaming {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(name_rect.shrink2(egui::vec2(0.0, 10.0))));
        let (_, buffer) = app.layers_ui.renaming.as_mut().unwrap();
        let edit = child.add(egui::TextEdit::singleline(buffer).desired_width(name_rect.width()));
        edit.request_focus();
        if edit.lost_focus() {
            let name = buffer.trim().to_owned();
            actions.push(Action::Rename(index, name));
        }
    } else {
        let color = if layer.visible { theme::TEXT } else { theme::TEXT_FAINT };
        let mut job = egui::text::LayoutJob::simple_singleline(layer.name.clone(), egui::FontId::proportional(12.5), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(name_rect.width());
        let galley = ui.painter().layout_job(job);
        painter.galley(egui::pos2(name_rect.left(), rect.center().y - galley.size().y / 2.0), galley, color);
        if layer.blend != BlendMode::Normal || layer.opacity < 1.0 {
            let info = format!("{} · {:.0}%", layer.blend.label(), layer.opacity * 100.0);
            painter.text(
                egui::pos2(name_rect.left(), rect.bottom() - 4.0),
                egui::Align2::LEFT_BOTTOM,
                info,
                egui::FontId::proportional(9.5),
                theme::TEXT_FAINT,
            );
        }
    }

    if response.double_clicked() {
        actions.push(Action::StartRename(index));
    } else if response.clicked() {
        actions.push(Action::Select(index, EditTarget::Content));
    }
    if response.drag_started_by(egui::PointerButton::Primary) {
        app.layers_ui.dragging = Some(index);
    }
    response.context_menu(|ui| {
        if ui.button(format!("{} Duplicate", icon::COPY)).clicked() {
            actions.push(Action::Duplicate(index));
        }
        if ui.button(format!("{} Delete", icon::TRASH)).clicked() {
            actions.push(Action::Delete(index));
        }
        if ui.button("Rename").clicked() {
            actions.push(Action::StartRename(index));
        }
        ui.separator();
        if layer.has_mask {
            let label = if layer.mask_enabled { "Disable mask" } else { "Enable mask" };
            if ui.button(label).clicked() {
                actions.push(Action::ToggleMask(index));
            }
            if ui.button("Delete mask").clicked() {
                actions.push(Action::DeleteMask(index));
            }
        } else {
            if ui.button("Add mask (reveal all)").clicked() {
                actions.push(Action::AddMask(index, true));
            }
            if ui.button("Add mask (hide all)").clicked() {
                actions.push(Action::AddMask(index, false));
            }
        }
    });
    rect
}

/// Insertion line while dragging a row; moves the layer on release.
fn drag_and_drop(app: &mut App, ui: &mut egui::Ui, rows: &[(usize, egui::Rect)], actions: &mut Vec<Action>) {
    let Some(from) = app.layers_ui.dragging else { return };
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let released = ui.input(|i| i.pointer.any_released());
    if let Some(p) = pointer {
        // Rows are drawn top-of-stack first; find the gap the pointer is over.
        let mut drop_slot = None;
        for &(index, rect) in rows {
            if p.y < rect.center().y {
                drop_slot = Some((index + 1, rect.top()));
                break;
            }
            drop_slot = Some((index, rect.bottom()));
        }
        if let Some((slot, y)) = drop_slot {
            let left = rows.first().map_or(0.0, |r| r.1.left());
            let right = rows.first().map_or(0.0, |r| r.1.right());
            ui.painter().line_segment([egui::pos2(left, y), egui::pos2(right, y)], egui::Stroke::new(2.0, theme::ACCENT));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if released {
                // `slot` is the insertion index in the original list (above layer `slot - 1`).
                let to = if slot > from { slot - 1 } else { slot };
                if to != from {
                    actions.push(Action::Move { from, to });
                }
            }
        }
    }
    if released {
        app.layers_ui.dragging = None;
    }
}

fn toolbar(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let active = app.viewport.paint.active;
    let has_mask = app.viewport.paint.active_layer().is_some_and(|l| l.has_mask);
    let button = |ui: &mut egui::Ui, glyph: &str, tip: &str| {
        ui.add(egui::Button::new(RichText::new(glyph).size(15.0)).min_size(egui::vec2(28.0, 24.0))).on_hover_text(tip)
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        if button(ui, icon::PLUS, "New paint layer (Ctrl+Shift+N)").clicked() {
            app.viewport.paint.add_layer(LayerKind::Paint);
        }
        if button(ui, icon::PAINT_BUCKET, "New fill layer (uses the foreground material)").clicked() {
            let fill = fill_from(app);
            app.viewport.paint.add_layer(LayerKind::Fill(fill));
        }
        ui.menu_button(RichText::new(icon::MAGIC_WAND).size(15.0), |ui| crate::ui_effects::presets_menu(app, ui))
            .response
            .on_hover_text("Smart layers and the Stylize filter");
        let mask_tip = if has_mask { "Delete mask" } else { "Add mask (Shift+click: hide all)" };
        if button(ui, icon::SQUARE_HALF, mask_tip).clicked() {
            if has_mask {
                actions.push(Action::DeleteMask(active));
            } else {
                let hide = ui.input(|i| i.modifiers.shift);
                actions.push(Action::AddMask(active, !hide));
            }
        }
        if button(ui, icon::COPY, "Duplicate layer (Ctrl+J)").clicked() {
            actions.push(Action::Duplicate(active));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if button(ui, icon::TRASH, "Delete layer (Del)").clicked() {
                actions.push(Action::Delete(active));
            }
        });
    });
}

fn apply(app: &mut App, actions: Vec<Action>) {
    for action in actions {
        let paint = &mut app.viewport.paint;
        match action {
            Action::Select(i, target) => {
                paint.active = i;
                let has_mask = paint.layers.get(i).is_some_and(|l| l.meta.has_mask);
                paint.target = if has_mask { target } else { EditTarget::Content };
            }
            Action::ToggleVisible(i) => paint.edit_layer(i, true, |l| l.visible = !l.visible),
            Action::ToggleMask(i) => paint.edit_layer(i, true, |l| l.mask_enabled = !l.mask_enabled),
            Action::Move { from, to } => paint.move_layer(from, to),
            Action::Duplicate(i) => paint.duplicate_layer(i),
            Action::Delete(i) => paint.delete_layer(i),
            Action::AddMask(i, white) => app.add_mask(i, white),
            Action::DeleteMask(i) => paint.delete_mask(i),
            Action::StartRename(i) => {
                if let Some(l) = paint.layers.get(i) {
                    app.layers_ui.renaming = Some((l.meta.id, l.meta.name.clone()));
                }
            }
            Action::Rename(i, name) => {
                app.layers_ui.renaming = None;
                if !name.is_empty() {
                    paint.edit_layer(i, true, |l| l.name = name);
                }
            }
        }
    }
}
