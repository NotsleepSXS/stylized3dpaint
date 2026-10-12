//! Layer stack panel: a tree of layers / fills / folders / effect layers, each with a
//! collapsible list of its effects (content and mask), thumbnails, drag and drop into
//! folders, and a toolbar to add layers, masks and effects (spec §8).

use std::collections::HashSet;

use egui::{Color32, RichText, Sense};
use egui_phosphor::regular as icon;
use s3dp_core::effect_def::{Category, Target};
use s3dp_core::{BlendMode, ChannelValues, EffectInstance, MaskItem, MaskItemKind, Node, NodeKind, registry};
use s3dp_render::EditTarget;

use crate::app::App;
use crate::i18n::{pick, t};
use crate::theme;
use crate::ui_panels::starts_edit;

const ROW_HEIGHT: f32 = 40.0;
const SUB_ROW: f32 = 22.0;
const THUMB: f32 = 30.0;
const EYE_WIDTH: f32 = 26.0;
const INDENT: f32 = 14.0;

#[derive(Default)]
pub struct LayersUi {
    /// Node being renamed and the edit buffer.
    renaming: Option<(u64, String)>,
    /// Node being dragged.
    dragging: Option<u64>,
    /// Nodes whose effect list is expanded.
    open_effects: HashSet<u64>,
    /// Search text of the "add effect" popup.
    pub search: String,
    /// Extra selected nodes (Shift / Ctrl+click); the active node is always selected.
    pub selection: Vec<u64>,
    /// Start of a Shift+click range.
    anchor: Option<u64>,
    /// Node rows top to bottom as last drawn (for Shift ranges).
    visible: Vec<u64>,
}

impl App {
    /// Selected layers: the multi-selection when it holds the active node, else just the active one.
    pub fn selected_layers(&self) -> Vec<u64> {
        let paint = &self.viewport.paint;
        let Some(active) = paint.active else { return Vec::new() };
        let sel = &self.layers_ui.selection;
        if sel.contains(&active) {
            sel.iter().copied().filter(|id| paint.doc.find(*id).is_some()).collect()
        } else {
            vec![active]
        }
    }
}

/// Actions collected while drawing and applied afterwards (rows only borrow the document).
enum Action {
    Select(u64, EditTarget),
    SelectEffect(u64, u64),
    ToggleVisible(u64),
    ToggleEffect(u64, u64),
    ToggleFolder(u64),
    ToggleEffectsList(u64),
    Move { node: u64, parent: Option<u64>, index: usize },
    Duplicate(u64),
    AddMask(u64, bool),
    DeleteMask(u64),
    ToggleMask(u64),
    StartRename(u64),
    Rename(u64, String),
    /// Context-menu operation on a content / mask effect: (node, effect, op).
    Effect(u64, u64, EffectOp),
    /// Click on a layer row with the Shift / Ctrl state.
    Click(u64, egui::Modifiers),
    GroupSelected,
    DeleteSelected,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EffectOp {
    Duplicate,
    Delete,
    /// Towards the top of the list (applied later).
    Up,
    Down,
    Anchor,
}

pub fn effect_icon(c: Category) -> &'static str {
    match c {
        Category::Generator => icon::SPARKLE,
        Category::Wear => icon::SCRIBBLE_LOOP,
        Category::Stylize => icon::PALETTE,
        Category::Color => icon::DROP_HALF,
        Category::Blur => icon::DROP,
        Category::Warp => icon::WAVES,
        Category::Relief => icon::MOUNTAINS,
        Category::Filter => icon::FUNNEL,
        Category::Utility => icon::WRENCH,
    }
}

/// Effect picker grouped by category with a search box. Returns the chosen definition id.
pub fn effect_picker(ui: &mut egui::Ui, search: &mut String, target: Target) -> Option<String> {
    let mut chosen = None;
    ui.set_min_width(240.0);
    let r = ui.add(egui::TextEdit::singleline(search).hint_text(format!("{} {}", icon::MAGNIFYING_GLASS, t("Search effects…"))));
    r.request_focus();
    let query = search.to_lowercase();
    let mut defs: Vec<_> = registry().effects.iter().filter(|d| d.targets.contains(&target)).collect();
    defs.sort_by_key(|d| (s3dp_render::engine::effects::category_order(d.category), d.display_name(crate::i18n::is_russian()).to_owned()));
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        let mut last = None;
        for d in defs {
            let name = d.display_name(crate::i18n::is_russian());
            if !query.is_empty() && !name.to_lowercase().contains(&query) && !d.name.to_lowercase().contains(&query) && !d.id.contains(&query) {
                continue;
            }
            if last != Some(d.category) {
                last = Some(d.category);
                ui.label(RichText::new(pick(d.category.label(), Some(d.category.label_ru()))).color(theme::TEXT_FAINT).small());
            }
            let tip = pick(d.description.as_deref().unwrap_or(""), d.description_ru.as_deref());
            let mut b = ui.button(format!("{} {name}", effect_icon(d.category)));
            if !tip.is_empty() {
                b = b.on_hover_text(tip);
            }
            if b.clicked() {
                chosen = Some(d.id.clone());
            }
        }
    });
    if chosen.is_some() {
        search.clear();
        ui.close();
    }
    chosen
}

/// Adds the effect to every selected layer (one instance each); the active one keeps it selected.
pub fn add_effect_to_selected(app: &mut App, def_id: &str) {
    let active = app.viewport.paint.active;
    let selected_effect = app.viewport.paint.selected_effect;
    let mut active_effect = None;
    for node in app.selected_layers() {
        add_effect_to(app, node, def_id);
        if Some(node) == active {
            active_effect = app.viewport.paint.selected_effect;
        }
    }
    app.viewport.paint.selected_effect = active_effect.or(selected_effect);
}

/// Adds an effect to the active node: to its mask when the mask is being edited, else to its content.
pub fn add_effect_to_active(app: &mut App, def_id: &str) {
    let Some(node) = app.viewport.paint.active else { return };
    add_effect_to(app, node, def_id);
}

fn add_effect_to(app: &mut App, node: u64, def_id: &str) {
    let Some(inst) = app.viewport.paint.new_effect(def_id) else { return };
    let effect_id = inst.id;
    let on_mask = matches!(app.viewport.paint.target, EditTarget::MaskItem(_)) || registry().get(def_id).is_some_and(|d| !d.targets.contains(&Target::Content));
    let item_id = app.viewport.paint.new_id();
    app.viewport.paint.edit_node(node, true, |n| {
        if on_mask {
            let mask = n.mask.get_or_insert_with(|| s3dp_core::Mask { enabled: true, items: Vec::new() });
            mask.items.push(MaskItem { id: item_id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(inst) });
        } else {
            n.effects.push(inst);
        }
    });
    app.viewport.paint.selected_effect = Some(effect_id);
    app.layers_ui.open_effects.insert(node);
}

pub fn add_effect_layer(app: &mut App, def_id: &str) {
    let Some(inst) = app.viewport.paint.new_effect(def_id) else { return };
    let id = inst.id;
    app.viewport.paint.add_node(NodeKind::Effect(inst), |_, _| {});
    app.viewport.paint.selected_effect = Some(id);
}

pub fn layer_menu(app: &mut App, ui: &mut egui::Ui) {
    let active = app.viewport.paint.active;
    if ui.add(egui::Button::new(format!("{} {}", icon::PLUS, t("New paint layer"))).shortcut_text("Ctrl+Shift+N")).clicked() {
        app.viewport.paint.add_node(NodeKind::Paint, |_, _| {});
        ui.close();
    }
    if ui.button(format!("{} {}", icon::PAINT_BUCKET, t("New fill layer"))).clicked() {
        new_fill(app);
        ui.close();
    }
    if ui.button(format!("{} {}", icon::FOLDER_SIMPLE, t("New folder"))).clicked() {
        app.viewport.paint.add_node(NodeKind::Folder { expanded: true }, |_, _| {});
        ui.close();
    }
    ui.menu_button(format!("{} {}", icon::STACK_PLUS, t("New effect layer")), |ui| {
        if let Some(id) = effect_picker(ui, &mut app.layers_ui.search, Target::Content) {
            add_effect_layer(app, &id);
        }
    });
    ui.separator();
    if let Some(a) = active {
        if ui.add(egui::Button::new(format!("{} {}", icon::FOLDER_PLUS, t("Group into folder"))).shortcut_text("Ctrl+G")).clicked() {
            let ids = app.selected_layers();
            if let Some(folder) = app.viewport.paint.group(&ids) {
                app.layers_ui.selection = vec![folder];
            }
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{} {}", icon::COPY, t("Duplicate"))).shortcut_text("Ctrl+J")).clicked() {
            app.viewport.paint.duplicate_node(a);
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{} {}", icon::TRASH, t("Delete"))).shortcut_text("Del")).clicked() {
            let ids = app.selected_layers();
            app.viewport.paint.delete_nodes(&ids);
            app.layers_ui.selection.clear();
            ui.close();
        }
        ui.separator();
        let has_mask = app.viewport.paint.active_node().is_some_and(|n| n.mask.is_some());
        if ui.add_enabled(!has_mask, egui::Button::new(t("Add white mask"))).clicked() {
            app.viewport.paint.add_mask(a, true);
            ui.close();
        }
        if ui.add_enabled(!has_mask, egui::Button::new(t("Add black mask"))).clicked() {
            app.viewport.paint.add_mask(a, false);
            ui.close();
        }
        if ui.add_enabled(has_mask, egui::Button::new(t("Delete mask"))).clicked() {
            app.viewport.paint.delete_mask(a);
            ui.close();
        }
    }
}

fn new_fill(app: &mut App) {
    let mut v = ChannelValues::defaults();
    v.set(s3dp_core::ChannelId::BaseColor, app.fg.get(s3dp_core::ChannelId::BaseColor));
    app.viewport.paint.add_node(NodeKind::Fill(v), |_, _| {});
}

pub fn layers_panel(app: &mut App, ui: &mut egui::Ui) {
    let mut actions = Vec::new();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        blend_and_opacity(app, ui);
        toolbar(app, ui, &mut actions);
    });
    ui.add(egui::Separator::default().spacing(0.0));
    let height = ui.available_height().max(60.0);
    egui::ScrollArea::vertical().id_salt("layer_tree").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let nodes = app.viewport.paint.doc.nodes.clone();
        let mut rows: Vec<(u64, Option<u64>, usize, egui::Rect, bool)> = Vec::new();
        draw_list(app, ui, &nodes, None, 0, &mut actions, &mut rows);
        app.layers_ui.visible = rows.iter().map(|r| r.0).collect();
        drag_and_drop(app, ui, &rows, &mut actions);
    });
    apply(app, actions);
}

fn blend_and_opacity(app: &mut App, ui: &mut egui::Ui) {
    let Some(node) = app.viewport.paint.active_node().cloned() else { return };
    ui.horizontal(|ui| {
        let mut mode = node.blending.default.mode;
        egui::ComboBox::from_id_salt("blend_mode").selected_text(t(mode.label())).width(128.0).show_ui(ui, |ui| {
            for entry in BlendMode::MENU {
                match entry {
                    Some(m) => {
                        ui.selectable_value(&mut mode, m, t(m.label()));
                    }
                    None => {
                        ui.separator();
                    }
                }
            }
        });
        if mode != node.blending.default.mode {
            app.viewport.paint.edit_node(node.id, true, |n| n.blending.default.mode = mode);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut pct = node.blending.default.opacity * 100.0;
            let r = ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).speed(0.5).suffix("%").max_decimals(0));
            if r.changed() {
                app.viewport.paint.edit_node(node.id, starts_edit(&r), |n| n.blending.default.opacity = pct / 100.0);
            }
            ui.label(t("Opacity:"));
        });
    });
}

fn toolbar(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let active = app.viewport.paint.active;
    let has_mask = app.viewport.paint.active_node().is_some_and(|n| n.mask.is_some());
    let button = |ui: &mut egui::Ui, glyph: &str, tip: &str| {
        ui.add(egui::Button::new(RichText::new(glyph).size(15.0)).min_size(egui::vec2(26.0, 24.0))).on_hover_text(tip)
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if button(ui, icon::PLUS, t("New paint layer (Ctrl+Shift+N)")).clicked() {
            app.viewport.paint.add_node(NodeKind::Paint, |_, _| {});
        }
        if button(ui, icon::PAINT_BUCKET, t("New fill layer")).clicked() {
            new_fill(app);
        }
        let many = app.selected_layers().len() > 1;
        let folder_tip = if many { t("Group the selected layers into a folder (Ctrl+G)") } else { t("New folder (with several layers selected: group them)") };
        if button(ui, icon::FOLDER_SIMPLE_PLUS, folder_tip).clicked() {
            if many {
                actions.push(Action::GroupSelected);
            } else {
                app.viewport.paint.add_node(NodeKind::Folder { expanded: true }, |_, _| {});
            }
        }
        ui.menu_button(RichText::new(icon::STACK_PLUS).size(15.0), |ui| {
            ui.label(RichText::new(t("Effect over the layers below")).strong());
            if let Some(id) = effect_picker(ui, &mut app.layers_ui.search, Target::Content) {
                add_effect_layer(app, &id);
            }
        })
        .response
        .on_hover_text(t("New effect layer (filters everything below it)"));
        let mask_tip = if has_mask { t("Delete mask") } else { t("Add mask (Shift+click: black)") };
        if button(ui, icon::SQUARE_HALF, mask_tip).clicked()
            && let Some(a) = active
        {
            if has_mask {
                actions.push(Action::DeleteMask(a));
            } else {
                let black = ui.input(|i| i.modifiers.shift);
                actions.push(Action::AddMask(a, !black));
            }
        }
        ui.menu_button(RichText::new(icon::MAGIC_WAND).size(15.0), |ui| {
            let target = if matches!(app.viewport.paint.target, EditTarget::MaskItem(_)) { Target::Mask } else { Target::Content };
            ui.label(RichText::new(if target == Target::Mask { t("Add to the mask") } else { t("Add to the layer content") }).strong());
            if let Some(id) = effect_picker(ui, &mut app.layers_ui.search, target) {
                add_effect_to_selected(app, &id);
            }
        })
        .response
        .on_hover_text(t("Add effect… (to every selected layer or its mask)"));
        if button(ui, icon::COPY, t("Duplicate (Ctrl+J)")).clicked()
            && let Some(a) = active
        {
            actions.push(Action::Duplicate(a));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if button(ui, icon::TRASH, t("Delete (Del)")).clicked() && active.is_some() {
                actions.push(Action::DeleteSelected);
            }
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

/// Draws `nodes` top of the stack first. Records (node, parent, index, rect, is_folder) for drag and drop.
#[allow(clippy::too_many_arguments)]
fn draw_list(
    app: &mut App,
    ui: &mut egui::Ui,
    nodes: &[Node],
    parent: Option<u64>,
    depth: usize,
    actions: &mut Vec<Action>,
    rows: &mut Vec<(u64, Option<u64>, usize, egui::Rect, bool)>,
) {
    for (index, node) in nodes.iter().enumerate().rev() {
        let rect = node_row(app, ui, node, depth, actions);
        rows.push((node.id, parent, index, rect, node.is_folder()));
        if app.layers_ui.open_effects.contains(&node.id) {
            effect_rows(app, ui, node, depth, actions);
        }
        if let NodeKind::Folder { expanded: true } = node.kind {
            draw_list(app, ui, &node.children, Some(node.id), depth + 1, actions, rows);
        }
    }
}

fn node_row(app: &mut App, ui: &mut egui::Ui, node: &Node, depth: usize, actions: &mut Vec<Action>) -> egui::Rect {
    let paint = &app.viewport.paint;
    let selected = paint.active == Some(node.id) || (app.layers_ui.selection.len() > 1 && app.layers_ui.selection.contains(&node.id) && paint.active.is_some_and(|a| app.layers_ui.selection.contains(&a)));
    let target = paint.target;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let fill = if selected { theme::ROW_SELECTED } else if response.hovered() { theme::HOVER.gamma_multiply(0.5) } else { Color32::TRANSPARENT };
    painter.rect_filled(rect, 0.0, fill);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0, theme::SEPARATOR));

    // Visibility.
    let eye = egui::Rect::from_min_size(rect.min, egui::vec2(EYE_WIDTH, ROW_HEIGHT));
    if ui.interact(eye, ui.id().with(("eye", node.id)), Sense::click()).on_hover_text(t("Show / hide")).clicked() {
        actions.push(Action::ToggleVisible(node.id));
    }
    if node.visible {
        painter.text(eye.center(), egui::Align2::CENTER_CENTER, icon::EYE, egui::FontId::proportional(14.0), theme::TEXT_DIM);
    }
    painter.line_segment([eye.right_top(), eye.right_bottom()], egui::Stroke::new(1.0, theme::SEPARATOR));

    let mut x = eye.right() + 4.0 + depth as f32 * INDENT;
    // Folder disclosure.
    if let NodeKind::Folder { expanded } = node.kind {
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(14.0, ROW_HEIGHT));
        let glyph = if expanded { icon::CARET_DOWN } else { icon::CARET_RIGHT };
        painter.text(r.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(12.0), theme::TEXT_DIM);
        if ui.interact(r, ui.id().with(("fold", node.id)), Sense::click()).clicked() {
            actions.push(Action::ToggleFolder(node.id));
        }
        x = r.right() + 2.0;
    }

    // Content thumbnail.
    let thumb_rect = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - THUMB / 2.0), egui::vec2(THUMB, THUMB));
    match &node.kind {
        NodeKind::Paint | NodeKind::Folder { .. } => {
            checker(&painter, thumb_rect);
            if let Some(tex) = app.thumb_texture(node.id, false) {
                painter.image(tex, thumb_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            } else if node.is_folder() {
                painter.rect_filled(thumb_rect, 0.0, theme::FIELD);
                painter.text(thumb_rect.center(), egui::Align2::CENTER_CENTER, icon::FOLDER_SIMPLE, egui::FontId::proportional(16.0), theme::TEXT_DIM);
            }
        }
        NodeKind::Fill(v) => {
            let c = v.get(s3dp_core::ChannelId::BaseColor).map(s3dp_core::document::linear_to_srgb);
            painter.rect_filled(thumb_rect, 0.0, crate::ui_panels::to_color32(c));
            let ink = if c.iter().sum::<f32>() / 3.0 > 0.5 { Color32::from_black_alpha(160) } else { Color32::from_white_alpha(180) };
            painter.text(thumb_rect.center(), egui::Align2::CENTER_CENTER, icon::PAINT_BUCKET, egui::FontId::proportional(15.0), ink);
        }
        NodeKind::Effect(e) => {
            painter.rect_filled(thumb_rect, 0.0, theme::ACCENT_SOFT);
            let glyph = registry().get(&e.def).map_or(icon::QUESTION, |d| effect_icon(d.category));
            painter.text(thumb_rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(16.0), Color32::WHITE);
        }
    }
    painter.rect_stroke(thumb_rect, 0.0, egui::Stroke::new(1.0, Color32::from_gray(20)), egui::StrokeKind::Outside);
    if ui.interact(thumb_rect, ui.id().with(("thumb", node.id)), Sense::click()).clicked() {
        actions.push(Action::Select(node.id, EditTarget::Content));
    }
    x = thumb_rect.right() + 4.0;

    // Mask thumbnail.
    let mut mask_rect = None;
    if let Some(mask) = &node.mask {
        let r = egui::Rect::from_min_size(egui::pos2(x, thumb_rect.top()), egui::vec2(THUMB, THUMB));
        painter.rect_filled(r, 0.0, Color32::BLACK);
        if let Some(tex) = app.thumb_texture(node.id, true) {
            painter.image(tex, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0, Color32::from_gray(20)), egui::StrokeKind::Outside);
        if !mask.enabled {
            painter.line_segment([r.left_top(), r.right_bottom()], egui::Stroke::new(2.0, theme::DANGER));
            painter.line_segment([r.right_top(), r.left_bottom()], egui::Stroke::new(2.0, theme::DANGER));
        }
        let resp = ui.interact(r, ui.id().with(("mask", node.id)), Sense::click()).on_hover_text(t("Click: edit the mask · Ctrl+click: disable / enable"));
        if resp.clicked() {
            if ui.input(|i| i.modifiers.command) {
                actions.push(Action::ToggleMask(node.id));
            } else {
                let paint_item = mask.items.iter().rev().find(|i| matches!(i.kind, MaskItemKind::Paint)).map(|i| i.id);
                actions.push(Action::Select(node.id, EditTarget::MaskItem(paint_item.unwrap_or(0))));
            }
        }
        mask_rect = Some(r);
        x = r.right() + 4.0;
    }

    // Brackets around what the brush edits.
    if selected {
        let r = match (target, mask_rect) {
            (EditTarget::MaskItem(_), Some(r)) => r,
            _ => thumb_rect,
        };
        painter.rect_stroke(r.expand(2.0), 1.0, egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
    }

    // Effects toggle on the right.
    let effect_count = node.effects.len() + node.mask.as_ref().map_or(0, |m| m.items.iter().filter(|i| matches!(i.kind, MaskItemKind::Effect(_))).count());
    let mut name_right = rect.right() - 6.0;
    if effect_count > 0 {
        let open = app.layers_ui.open_effects.contains(&node.id);
        let r = egui::Rect::from_min_size(egui::pos2(rect.right() - 34.0, rect.top()), egui::vec2(30.0, ROW_HEIGHT));
        let glyph = if open { icon::CARET_UP } else { icon::SPARKLE };
        painter.text(r.center(), egui::Align2::CENTER_CENTER, format!("{glyph}{effect_count}"), egui::FontId::proportional(11.0), theme::TEXT_DIM);
        if ui.interact(r, ui.id().with(("fx", node.id)), Sense::click()).on_hover_text(t("Show / hide effects")).clicked() {
            actions.push(Action::ToggleEffectsList(node.id));
        }
        name_right = r.left() - 2.0;
    }

    // Name or rename editor.
    let name_rect = egui::Rect::from_min_max(egui::pos2(x + 4.0, rect.top()), egui::pos2(name_right, rect.bottom()));
    let renaming = app.layers_ui.renaming.as_ref().is_some_and(|(id, _)| *id == node.id);
    if renaming {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(name_rect.shrink2(egui::vec2(0.0, 9.0))));
        let (_, buffer) = app.layers_ui.renaming.as_mut().unwrap();
        let edit = child.add(egui::TextEdit::singleline(buffer).desired_width(name_rect.width()));
        edit.request_focus();
        if edit.lost_focus() {
            actions.push(Action::Rename(node.id, buffer.trim().to_owned()));
        }
    } else {
        let color = if node.visible { theme::TEXT } else { theme::TEXT_FAINT };
        let mut job = egui::text::LayoutJob::simple_singleline(node.name.clone(), egui::FontId::proportional(12.5), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(name_rect.width().max(10.0));
        let galley = ui.painter().layout_job(job);
        painter.galley(egui::pos2(name_rect.left(), rect.center().y - galley.size().y / 2.0 - 4.0), galley, color);
        let b = node.blending.default;
        let info = if b.mode != BlendMode::Normal || b.opacity < 1.0 { format!("{} · {:.0}%", t(b.mode.label()), b.opacity * 100.0) } else { String::new() };
        let kind = match &node.kind {
            NodeKind::Paint => t("Paint"),
            NodeKind::Fill(_) => t("Fill"),
            NodeKind::Folder { .. } => t("Folder"),
            NodeKind::Effect(_) => t("Effect"),
        };
        painter.text(
            egui::pos2(name_rect.left(), rect.bottom() - 5.0),
            egui::Align2::LEFT_BOTTOM,
            if info.is_empty() { kind.to_owned() } else { format!("{kind} · {info}") },
            egui::FontId::proportional(9.5),
            theme::TEXT_FAINT,
        );
    }

    if response.double_clicked() {
        actions.push(Action::StartRename(node.id));
    } else if response.clicked() {
        actions.push(Action::Click(node.id, ui.input(|i| i.modifiers)));
    } else if response.secondary_clicked() && !app.selected_layers().contains(&node.id) {
        actions.push(Action::Click(node.id, egui::Modifiers::NONE));
    }
    if response.drag_started_by(egui::PointerButton::Primary) {
        app.layers_ui.dragging = Some(node.id);
    }
    response.context_menu(|ui| {
        if ui.button(format!("{} {}", icon::COPY, t("Duplicate"))).clicked() {
            actions.push(Action::Duplicate(node.id));
            ui.close();
        }
        if ui.button(format!("{} {}", icon::FOLDER_PLUS, t("Group into folder"))).clicked() {
            actions.push(Action::GroupSelected);
            ui.close();
        }
        if ui.button(t("Rename")).clicked() {
            actions.push(Action::StartRename(node.id));
            ui.close();
        }
        if ui.button(format!("{} {}", icon::TRASH, t("Delete"))).clicked() {
            actions.push(Action::DeleteSelected);
            ui.close();
        }
        ui.separator();
        if node.mask.is_some() {
            if ui.button(t("Enable / disable mask")).clicked() {
                actions.push(Action::ToggleMask(node.id));
                ui.close();
            }
            if ui.button(t("Delete mask")).clicked() {
                actions.push(Action::DeleteMask(node.id));
                ui.close();
            }
        } else {
            if ui.button(t("Add white mask")).clicked() {
                actions.push(Action::AddMask(node.id, true));
                ui.close();
            }
            if ui.button(t("Add black mask")).clicked() {
                actions.push(Action::AddMask(node.id, false));
                ui.close();
            }
        }
    });
    rect
}

/// Small rows listing a node's content effects and mask items.
fn effect_rows(app: &mut App, ui: &mut egui::Ui, node: &Node, depth: usize, actions: &mut Vec<Action>) {
    let selected = app.viewport.paint.selected_effect;
    let mut row = |ui: &mut egui::Ui, e: &EffectInstance, on_mask: bool| {
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, SUB_ROW), Sense::click());
        let painter = ui.painter_at(rect);
        let is_sel = selected == Some(e.id);
        if is_sel {
            painter.rect_filled(rect, 0.0, theme::ACCENT_SOFT);
        } else if resp.hovered() {
            painter.rect_filled(rect, 0.0, theme::HOVER.gamma_multiply(0.4));
        }
        let x = rect.left() + EYE_WIDTH + 18.0 + depth as f32 * INDENT;
        let def = registry().get(&e.def);
        let glyph = def.map_or(icon::QUESTION, |d| effect_icon(d.category));
        let name = e.name.clone().unwrap_or_else(|| def.map_or(e.def.clone(), |d| d.display_name(crate::i18n::is_russian()).to_owned()));
        let error = app.viewport.paint.errors.contains_key(&e.id) || def.is_none();
        let color = if error { theme::DANGER } else if e.enabled { theme::TEXT } else { theme::TEXT_FAINT };
        let place = if on_mask { icon::SQUARE_HALF } else { "" };
        let anchor = if e.anchor { format!(" {}", icon::ANCHOR) } else { String::new() };
        painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, format!("{place}{glyph} {name}{anchor}"), egui::FontId::proportional(11.5), color);
        let eye = egui::Rect::from_min_size(egui::pos2(rect.right() - 24.0, rect.top()), egui::vec2(22.0, SUB_ROW));
        painter.text(eye.center(), egui::Align2::CENTER_CENTER, if e.enabled { icon::EYE } else { icon::EYE_SLASH }, egui::FontId::proportional(12.0), theme::TEXT_FAINT);
        if ui.interact(eye, ui.id().with(("fxeye", e.id)), Sense::click()).clicked() {
            actions.push(Action::ToggleEffect(node.id, e.id));
        } else if resp.clicked() || resp.secondary_clicked() {
            actions.push(Action::SelectEffect(node.id, e.id));
        }
        resp.context_menu(|ui| {
            let toggle = if e.enabled { format!("{} {}", icon::EYE_SLASH, t("Disable")) } else { format!("{} {}", icon::EYE, t("Enable")) };
            let items: [(String, Action); 6] = [
                (toggle, Action::ToggleEffect(node.id, e.id)),
                (format!("{} {}", icon::COPY, t("Duplicate")), Action::Effect(node.id, e.id, EffectOp::Duplicate)),
                (format!("{} {}", icon::CARET_UP, t("Move up")), Action::Effect(node.id, e.id, EffectOp::Up)),
                (format!("{} {}", icon::CARET_DOWN, t("Move down")), Action::Effect(node.id, e.id, EffectOp::Down)),
                (format!("{} {}", icon::ANCHOR, if e.anchor { t("Remove anchor") } else { t("Make anchor") }), Action::Effect(node.id, e.id, EffectOp::Anchor)),
                (format!("{} {}", icon::TRASH, t("Delete")), Action::Effect(node.id, e.id, EffectOp::Delete)),
            ];
            for (i, (label, action)) in items.into_iter().enumerate() {
                if i == 5 {
                    ui.separator();
                }
                if ui.button(label).clicked() {
                    actions.push(action);
                    ui.close();
                }
            }
        });
    };
    for e in node.effects.iter().rev() {
        row(ui, e, false);
    }
    if let Some(mask) = &node.mask {
        for item in mask.items.iter().rev() {
            if let MaskItemKind::Effect(e) = &item.kind {
                row(ui, e, true);
            }
        }
    }
}

/// Insertion line / folder highlight while dragging; moves the node on release.
fn drag_and_drop(app: &mut App, ui: &mut egui::Ui, rows: &[(u64, Option<u64>, usize, egui::Rect, bool)], actions: &mut Vec<Action>) {
    let Some(dragged) = app.layers_ui.dragging else { return };
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let released = ui.input(|i| i.pointer.any_released());
    if let Some(p) = pointer
        && let Some(&(id, parent, index, rect, is_folder)) = rows.iter().find(|r| r.3.y_range().contains(p.y))
        && id != dragged
    {
        let f = (p.y - rect.top()) / rect.height();
        let accent = egui::Stroke::new(2.0, theme::ACCENT);
        let (target_parent, target_index) = if is_folder && (0.3..0.7).contains(&f) {
            ui.painter().rect_stroke(rect.shrink(1.0), 2.0, accent, egui::StrokeKind::Inside);
            (Some(id), usize::MAX)
        } else if f < 0.5 {
            ui.painter().line_segment([rect.left_top(), rect.right_top()], accent);
            (parent, index + 1)
        } else {
            ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], accent);
            (parent, index)
        };
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if released {
            // Indices shift when the node moves within the same list.
            let same_list = app.viewport.paint.doc.path_of(dragged).and_then(|p| p.last().copied());
            let mut idx = target_index;
            if let Some(from) = same_list
                && target_parent == parent_of(app, dragged)
                && from < idx
                && idx != usize::MAX
            {
                idx -= 1;
            }
            actions.push(Action::Move { node: dragged, parent: target_parent, index: idx });
        }
    }
    if released {
        app.layers_ui.dragging = None;
    }
}

fn parent_of(app: &App, id: u64) -> Option<u64> {
    let path = app.viewport.paint.doc.path_of(id)?;
    if path.len() < 2 {
        return None;
    }
    let mut list = &app.viewport.paint.doc.nodes;
    let mut parent = None;
    for &i in &path[..path.len() - 1] {
        parent = Some(list[i].id);
        list = &list[i].children;
    }
    parent
}

fn apply(app: &mut App, actions: Vec<Action>) {
    for action in actions {
        let paint = &mut app.viewport.paint;
        match action {
            Action::Select(id, target) => {
                paint.active = Some(id);
                paint.selected_effect = None;
                paint.target = target;
            }
            Action::SelectEffect(node, effect) => {
                paint.active = Some(node);
                paint.selected_effect = Some(effect);
            }
            Action::ToggleVisible(id) => paint.edit_node(id, true, |n| n.visible = !n.visible),
            Action::ToggleEffect(node, effect) => paint.edit_node(node, true, |n| {
                if let NodeKind::Effect(e) = &mut n.kind
                    && e.id == effect
                {
                    e.enabled = !e.enabled;
                }
                for e in n.effects.iter_mut().filter(|e| e.id == effect) {
                    e.enabled = !e.enabled;
                }
                for item in n.mask.iter_mut().flat_map(|m| m.items.iter_mut()) {
                    if let MaskItemKind::Effect(e) = &mut item.kind
                        && e.id == effect
                    {
                        e.enabled = !e.enabled;
                    }
                }
            }),
            Action::Effect(node, effect, op) => {
                paint.edit_document(true, |d| {
                    let ids = (op == EffectOp::Duplicate).then(|| (d.alloc_id(), d.alloc_id()));
                    let Some(n) = d.find_mut(node) else { return };
                    if let Some(i) = n.effects.iter().position(|e| e.id == effect) {
                        let list = &mut n.effects;
                        match op {
                            EffectOp::Duplicate => {
                                let mut c = list[i].clone();
                                c.id = ids.map_or(c.id, |x| x.0);
                                c.anchor = false;
                                list.insert(i + 1, c);
                            }
                            EffectOp::Delete => {
                                list.remove(i);
                            }
                            EffectOp::Up if i + 1 < list.len() => list.swap(i, i + 1),
                            EffectOp::Down if i > 0 => list.swap(i, i - 1),
                            EffectOp::Anchor => list[i].anchor = !list[i].anchor,
                            _ => {}
                        }
                    } else if let Some(m) = &mut n.mask
                        && let Some(i) = m.items.iter().position(|it| matches!(&it.kind, MaskItemKind::Effect(e) if e.id == effect))
                    {
                        let list = &mut m.items;
                        match op {
                            EffectOp::Duplicate => {
                                let mut c = list[i].clone();
                                if let Some((item_id, effect_id)) = ids {
                                    c.id = item_id;
                                    if let MaskItemKind::Effect(e) = &mut c.kind {
                                        e.id = effect_id;
                                        e.anchor = false;
                                    }
                                }
                                list.insert(i + 1, c);
                            }
                            EffectOp::Delete => {
                                list.remove(i);
                            }
                            EffectOp::Up if i + 1 < list.len() => list.swap(i, i + 1),
                            EffectOp::Down if i > 0 => list.swap(i, i - 1),
                            EffectOp::Anchor => {
                                if let MaskItemKind::Effect(e) = &mut list[i].kind {
                                    e.anchor = !e.anchor;
                                }
                            }
                            _ => {}
                        }
                    }
                });
                if op == EffectOp::Delete && paint.selected_effect == Some(effect) {
                    paint.selected_effect = None;
                }
            }
            Action::ToggleFolder(id) => paint.edit_node(id, false, |n| {
                if let NodeKind::Folder { expanded } = &mut n.kind {
                    *expanded = !*expanded;
                }
            }),
            Action::ToggleEffectsList(id) => {
                if !app.layers_ui.open_effects.remove(&id) {
                    app.layers_ui.open_effects.insert(id);
                }
            }
            Action::Move { node, parent, index } => paint.move_node(node, parent, index),
            Action::Duplicate(id) => paint.duplicate_node(id),
            Action::AddMask(id, white) => paint.add_mask(id, white),
            Action::DeleteMask(id) => paint.delete_mask(id),
            Action::ToggleMask(id) => paint.edit_node(id, true, |n| {
                if let Some(m) = &mut n.mask {
                    m.enabled = !m.enabled;
                }
            }),
            Action::StartRename(id) => {
                if let Some(n) = paint.doc.find(id) {
                    app.layers_ui.renaming = Some((id, n.name.clone()));
                }
            }
            Action::Rename(id, name) => {
                app.layers_ui.renaming = None;
                if !name.is_empty() {
                    paint.edit_node(id, true, |n| n.name = name);
                }
            }
            Action::DeleteSelected => {
                let ids = app.selected_layers();
                app.viewport.paint.delete_nodes(&ids);
                app.layers_ui.selection.clear();
            }
            Action::GroupSelected => {
                let ids = app.selected_layers();
                if let Some(folder) = app.viewport.paint.group(&ids) {
                    app.layers_ui.selection = vec![folder];
                    app.layers_ui.anchor = Some(folder);
                }
            }
            Action::Click(id, mods) => {
                let ui_state = &mut app.layers_ui;
                let active = paint.active;
                if mods.shift {
                    let from = ui_state.anchor.or(active).unwrap_or(id);
                    let pos = |x: u64| ui_state.visible.iter().position(|v| *v == x);
                    if let (Some(a), Some(b)) = (pos(from), pos(id)) {
                        let (lo, hi) = (a.min(b), a.max(b));
                        ui_state.selection = ui_state.visible[lo..=hi].to_vec();
                    } else {
                        ui_state.selection = vec![id];
                    }
                    ui_state.anchor = Some(from);
                    paint.active = Some(id);
                } else if mods.command {
                    // Start from what is selected now (a lone active node counts).
                    if !active.is_some_and(|a| ui_state.selection.contains(&a)) {
                        ui_state.selection = active.into_iter().collect();
                    }
                    if let Some(i) = ui_state.selection.iter().position(|x| *x == id) {
                        ui_state.selection.remove(i);
                        if active == Some(id) {
                            paint.active = ui_state.selection.last().copied();
                        }
                    } else {
                        ui_state.selection.push(id);
                        paint.active = Some(id);
                    }
                    ui_state.anchor = Some(id);
                } else {
                    ui_state.selection = vec![id];
                    ui_state.anchor = Some(id);
                    paint.active = Some(id);
                }
                paint.selected_effect = None;
                paint.target = EditTarget::Content;
            }
        }
    }
}
