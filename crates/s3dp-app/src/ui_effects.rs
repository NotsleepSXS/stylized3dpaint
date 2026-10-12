//! Properties panel for the selected layer / effect. Effect properties are generated from the
//! effect's declarative definition (spec §2.5): grouped parameters, tooltips, reset by right
//! click, seed randomizer, anchor picker, presets, missing mesh map warning.

use egui::RichText;
use egui_phosphor::regular as icon;
use s3dp_core::effect_def::{EffectDef, ParamDef, ParamKind, Target, WidgetDef};
use s3dp_core::{BlendMode, ChannelId, EffectInstance, MaskItem, MaskItemKind, Node, NodeKind, ParamValue, registry};

use crate::app::App;
use crate::i18n::{pick, t};
use crate::theme;
use crate::ui_layers::{effect_icon, effect_picker};
use crate::ui_panels::{chip, fit_sliders, from_color32, label_width, starts_edit, to_color32};

/// Accumulates whether widgets changed something and whether that starts an undo step.
#[derive(Default)]
pub struct Edit {
    pub changed: bool,
    pub checkpoint: bool,
}

impl Edit {
    pub fn note(&mut self, r: &egui::Response) {
        if r.changed() {
            self.changed = true;
            self.checkpoint |= starts_edit(r);
        }
    }

    pub fn structural(&mut self) {
        self.changed = true;
        self.checkpoint = true;
    }
}

fn blend_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, mode: &mut BlendMode) -> bool {
    let before = *mode;
    egui::ComboBox::from_id_salt(id).selected_text(t(mode.label())).width(118.0).show_ui(ui, |ui| {
        for entry in BlendMode::MENU {
            match entry {
                Some(m) => {
                    ui.selectable_value(mode, m, t(m.label()));
                }
                None => {
                    ui.separator();
                }
            }
        }
    });
    *mode != before
}

/// Finds an effect instance anywhere in a node.
fn find_effect<'a>(node: &'a Node, id: u64) -> Option<&'a EffectInstance> {
    node.own_effects().find(|e| e.id == id)
}

fn with_effect(node: &mut Node, id: u64, f: impl FnOnce(&mut EffectInstance)) {
    if let NodeKind::Effect(e) = &mut node.kind
        && e.id == id
    {
        return f(e);
    }
    if let Some(e) = node.effects.iter_mut().find(|e| e.id == id) {
        return f(e);
    }
    for item in node.mask.iter_mut().flat_map(|m| m.items.iter_mut()) {
        if let MaskItemKind::Effect(e) = &mut item.kind
            && e.id == id
        {
            return f(e);
        }
    }
}

/// Properties of the active node, or of its selected effect.
pub fn properties(app: &mut App, ui: &mut egui::Ui) {
    let Some(node) = app.viewport.paint.active_node().cloned() else {
        ui.label(RichText::new(t("Select a layer")).color(theme::TEXT_FAINT));
        return;
    };
    // Effect layers always show their effect.
    let selected = match &node.kind {
        NodeKind::Effect(e) => Some(e.id),
        _ => app.viewport.paint.selected_effect.filter(|id| find_effect(&node, *id).is_some()),
    };
    if let Some(effect_id) = selected {
        if !node.is_effect_layer() && ui.button(format!("{} {}", icon::ARROW_LEFT, t("Layer properties"))).clicked() {
            app.viewport.paint.selected_effect = None;
            return;
        }
        effect_editor(app, ui, &node, effect_id);
        if node.is_effect_layer() {
            ui.separator();
            mask_editor(app, ui, &node);
        }
    } else {
        node_editor(app, ui, &node);
    }
}

fn node_editor(app: &mut App, ui: &mut egui::Ui, node: &Node) {
    let mut edit = Edit::default();
    let mut n = node.clone();
    let kind = match n.kind {
        NodeKind::Paint => format!("{} {}", icon::PAINT_BRUSH, t("Paint layer")),
        NodeKind::Fill(_) => format!("{} {}", icon::PAINT_BUCKET, t("Fill layer")),
        NodeKind::Folder { .. } => format!("{} {}", icon::FOLDER_SIMPLE, t("Folder")),
        NodeKind::Effect(_) => format!("{} {}", icon::STACK_PLUS, t("Effect layer")),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(kind).strong());
        let r = ui.add(egui::TextEdit::singleline(&mut n.name).desired_width(ui.available_width()));
        edit.note(&r);
    });
    let doc_channels = app.viewport.paint.doc.channels;
    ui.horizontal_wrapped(|ui| {
        ui.label(t("Channels"));
        for c in doc_channels.iter() {
            let on = n.blending.channels.has(c);
            if chip(ui, c.short(), on, &format!("{} — {}", t(c.label()), t("this layer contributes to it"))).clicked() {
                n.blending.channels.set(c, !on);
                edit.structural();
            }
        }
    });
    egui::CollapsingHeader::new(t("Blending per channel")).id_salt(("per_channel", n.id)).show(ui, |ui| {
        for c in doc_channels.iter().filter(|c| n.blending.channels.has(*c)) {
            ui.horizontal(|ui| {
                ui.add_sized([62.0, 18.0], egui::Label::new(t(c.label())).truncate());
                let mut b = n.blending.per_channel.get(&c).copied().unwrap_or(n.blending.default);
                let mut changed = blend_combo(ui, ("pc_mode", n.id, c), &mut b.mode);
                let mut pct = b.opacity * 100.0;
                let r = ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).suffix("%").max_decimals(0));
                if r.changed() {
                    b.opacity = pct / 100.0;
                    changed = true;
                    edit.checkpoint |= starts_edit(&r);
                } else if changed {
                    edit.checkpoint = true;
                }
                if changed {
                    n.blending.per_channel.insert(c, b);
                    edit.changed = true;
                }
                let has_override = n.blending.per_channel.contains_key(&c);
                if has_override && ui.small_button(icon::ARROW_COUNTER_CLOCKWISE).on_hover_text(t("Use the layer default")).clicked() {
                    n.blending.per_channel.remove(&c);
                    edit.structural();
                }
            });
        }
        if ui.small_button(t("Copy blending to clipboard")).clicked() {
            app.blend_clipboard = Some(n.blending.clone());
        }
        if let Some(b) = app.blend_clipboard.clone()
            && ui.small_button(t("Paste blending")).clicked()
        {
            n.blending = b;
            edit.structural();
        }
    });
    if let NodeKind::Fill(values) = &mut n.kind {
        ui.add_space(4.0);
        ui.label(RichText::new(t("Fill values")).strong());
        values_editor(ui, ("fill", node.id), values, doc_channels, &mut edit);
    }
    if let NodeKind::Folder { .. } = n.kind {
        ui.label(
            RichText::new(t("Folder blend mode Passthrough composites the children directly into the stack below."))
                .color(theme::TEXT_FAINT)
                .small(),
        );
    }
    if edit.changed {
        app.viewport.paint.edit_node(node.id, edit.checkpoint, |dst| *dst = n.clone());
    }
    ui.separator();
    content_effects(app, ui, node);
    ui.separator();
    mask_editor(app, ui, node);
}

/// Editor for linear channel values (colors edited in sRGB).
pub fn values_editor(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, values: &mut s3dp_core::ChannelValues, channels: s3dp_core::ChannelSet, edit: &mut Edit) {
    let w = label_width(ui, channels.iter().map(|c| t(c.label())), 60.0);
    fit_sliders(ui, w);
    egui::Grid::new(id).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for c in channels.iter() {
            ui.label(t(c.label()));
            let mut v = values.get(c);
            if c.is_color() && c != ChannelId::Normal {
                let mut col = to_color32(v.map(s3dp_core::document::linear_to_srgb));
                let r = egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque);
                if r.changed() {
                    v = from_color32(col).map(s3dp_core::document::srgb_to_linear);
                }
                edit.note(&r);
            } else if c == ChannelId::Normal {
                ui.label(RichText::new(t("flat")).color(theme::TEXT_FAINT));
            } else {
                let r = ui.add(egui::Slider::new(&mut v[0], 0.0..=1.0).max_decimals(3));
                v = [v[0]; 3];
                edit.note(&r);
            }
            values.set(c, v);
            ui.end_row();
        }
    });
}

fn content_effects(app: &mut App, ui: &mut egui::Ui, node: &Node) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} {}", icon::SPARKLE, t("Content effects"))).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(format!("{} {}", icon::PLUS, t("Add")), |ui| {
                if let Some(id) = effect_picker(ui, &mut app.layers_ui.search, Target::Content) {
                    app.viewport.paint.target = s3dp_render::EditTarget::Content;
                    crate::ui_layers::add_effect_to_active(app, &id);
                }
            });
        });
    });
    if node.effects.is_empty() {
        ui.label(RichText::new(t("Filters and generators applied to this layer's own pixels.")).color(theme::TEXT_FAINT).small());
    }
    let mut remove = None;
    let mut swap = None;
    let count = node.effects.len();
    for (i, e) in node.effects.iter().enumerate() {
        effect_list_row(app, ui, node, e, i, count, &mut remove, &mut swap);
    }
    if let Some(i) = remove {
        app.viewport.paint.edit_node(node.id, true, |n| {
            n.effects.remove(i);
        });
    }
    if let Some((a, b)) = swap {
        app.viewport.paint.edit_node(node.id, true, |n| n.effects.swap(a, b));
    }
}

#[allow(clippy::too_many_arguments)]
fn effect_list_row(app: &mut App, ui: &mut egui::Ui, node: &Node, e: &EffectInstance, i: usize, count: usize, remove: &mut Option<usize>, swap: &mut Option<(usize, usize)>) {
    let def = registry().get(&e.def);
    ui.horizontal(|ui| {
        let glyph = def.map_or(icon::QUESTION, |d| effect_icon(d.category));
        let name = e.name.clone().unwrap_or_else(|| def.map_or(e.def.clone(), |d| d.display_name(crate::i18n::is_russian()).to_owned()));
        let err = app.viewport.paint.errors.contains_key(&e.id) || def.is_none();
        let text = RichText::new(format!("{glyph} {name}")).color(if err { theme::DANGER } else { theme::TEXT });
        if ui.selectable_label(app.viewport.paint.selected_effect == Some(e.id), text).clicked() {
            app.viewport.paint.selected_effect = Some(e.id);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button(icon::X).on_hover_text(t("Remove")).clicked() {
                *remove = Some(i);
            }
            if i + 1 < count && ui.small_button(icon::CARET_UP).on_hover_text(t("Move up")).clicked() {
                *swap = Some((i, i + 1));
            }
            if i > 0 && ui.small_button(icon::CARET_DOWN).on_hover_text(t("Move down")).clicked() {
                *swap = Some((i - 1, i));
            }
        });
    });
    let _ = node;
}

fn mask_editor(app: &mut App, ui: &mut egui::Ui, node: &Node) {
    let Some(mask) = node.mask.clone() else {
        ui.label(RichText::new(format!("{} {}", icon::SQUARE_HALF, t("Mask"))).strong());
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("{} {}", icon::PLUS, t("White"))).on_hover_text(t("Add white mask")).clicked() {
                app.viewport.paint.add_mask(node.id, true);
            }
            if ui.button(format!("{} {}", icon::PLUS, t("Black"))).on_hover_text(t("Add black mask")).clicked() {
                app.viewport.paint.add_mask(node.id, false);
            }
        });
        return;
    };
    let mut edit = Edit::default();
    let mut m = mask.clone();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} {}", icon::SQUARE_HALF, t("Mask"))).strong());
        let r = ui.checkbox(&mut m.enabled, t("enabled"));
        edit.note(&r);
        if r.changed() {
            edit.structural();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(format!("{} {}", icon::PLUS, t("Add")), |ui| {
                if ui.button(format!("{} {}", icon::PAINT_BRUSH, t("Paint"))).clicked() {
                    let id = app.viewport.paint.new_id();
                    m.items.push(MaskItem { id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Paint });
                    app.viewport.paint.target = s3dp_render::EditTarget::MaskItem(id);
                    edit.structural();
                    ui.close();
                }
                for (label, v) in [(t("Fill white"), 1.0), (t("Fill black"), 0.0)] {
                    if ui.button(format!("{} {label}", icon::PAINT_BUCKET)).clicked() {
                        let id = app.viewport.paint.new_id();
                        m.items.push(MaskItem { id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Fill(v) });
                        edit.structural();
                        ui.close();
                    }
                }
                ui.separator();
                if let Some(def_id) = effect_picker(ui, &mut app.layers_ui.search, Target::Mask)
                    && let Some(inst) = app.viewport.paint.new_effect(&def_id)
                {
                    let id = app.viewport.paint.new_id();
                    app.viewport.paint.selected_effect = Some(inst.id);
                    m.items.push(MaskItem { id, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(inst) });
                    edit.structural();
                }
            });
        });
    });
    let target = app.viewport.paint.target;
    let mut remove = None;
    let mut swap = None;
    let count = m.items.len();
    for (i, item) in m.items.iter_mut().enumerate().rev() {
        ui.horizontal(|ui| {
            let r = ui.checkbox(&mut item.enabled, "");
            if r.changed() {
                edit.structural();
            }
            match &mut item.kind {
                MaskItemKind::Paint => {
                    let active = target == s3dp_render::EditTarget::MaskItem(item.id);
                    let label = RichText::new(format!("{} {}", icon::PAINT_BRUSH, t("Paint"))).color(if active { theme::ACCENT } else { theme::TEXT });
                    if ui.selectable_label(active, label).on_hover_text(t("Click to paint this item")).clicked() {
                        app.viewport.paint.target = s3dp_render::EditTarget::MaskItem(item.id);
                    }
                }
                MaskItemKind::Fill(v) => {
                    ui.label(format!("{} {}", icon::PAINT_BUCKET, t("Fill")));
                    let r = ui.add(egui::DragValue::new(v).range(0.0..=1.0).speed(0.01).max_decimals(2));
                    edit.note(&r);
                }
                MaskItemKind::Effect(e) => {
                    let def = registry().get(&e.def);
                    let glyph = def.map_or(icon::QUESTION, |d| effect_icon(d.category));
                    let name = def.map_or(e.def.clone(), |d| d.display_name(crate::i18n::is_russian()).to_owned());
                    let err = app.viewport.paint.errors.contains_key(&e.id) || def.is_none();
                    let text = RichText::new(format!("{glyph} {name}")).color(if err { theme::DANGER } else { theme::TEXT });
                    if ui.selectable_label(app.viewport.paint.selected_effect == Some(e.id), text).clicked() {
                        app.viewport.paint.selected_effect = Some(e.id);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(icon::X).on_hover_text(t("Remove")).clicked() {
                    remove = Some(i);
                }
                if i + 1 < count && ui.small_button(icon::CARET_UP).clicked() {
                    swap = Some((i, i + 1));
                }
                if i > 0 && ui.small_button(icon::CARET_DOWN).clicked() {
                    swap = Some((i - 1, i));
                }
                if !matches!(item.kind, MaskItemKind::Effect(_)) {
                    let mut pct = item.opacity * 100.0;
                    let r = ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).suffix("%").max_decimals(0));
                    if r.changed() {
                        item.opacity = pct / 100.0;
                    }
                    edit.note(&r);
                    if blend_combo(ui, ("mask_item_blend", item.id), &mut item.blend) {
                        edit.structural();
                    }
                }
            });
        });
    }
    if let Some(i) = remove {
        m.items.remove(i);
        edit.structural();
    }
    if let Some((a, b)) = swap {
        m.items.swap(a, b);
        edit.structural();
    }
    if edit.changed {
        app.viewport.paint.edit_node(node.id, edit.checkpoint, |n| n.mask = Some(m.clone()));
    }
}

/// Collapsible preview of what the effect changes: the layer mask for mask effects, the
/// material otherwise; 3D (main camera) or the flat texture (UV).
fn effect_preview(app: &mut App, ui: &mut egui::Ui, node: &Node, effect_id: u64) {
    let on_mask = node.mask.as_ref().is_some_and(|m| m.items.iter().any(|i| matches!(&i.kind, MaskItemKind::Effect(x) if x.id == effect_id)));
    let uv_id = egui::Id::new("effect_preview_uv");
    let mut uv: bool = ui.data_mut(|d| *d.get_persisted_mut_or(uv_id, false));
    egui::CollapsingHeader::new(RichText::new(format!("{} {}", icon::EYE, t("Preview"))).strong()).id_salt("effect_preview").default_open(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut uv, false, "3D").on_hover_text(t("This effect with what is below it (effects above are left out), on a sample shape"));
            ui.selectable_value(&mut uv, true, "UV").on_hover_text(t("The same on the sample's flat texture"));
            ui.label(RichText::new(if on_mask { t("Mask up to this effect") } else { t("Layer up to this effect") }).small().color(theme::TEXT_FAINT));
        });
        // The effect with what is below it in its stack (not the effects above), on a sample shape.
        if let Some(e) = find_effect(node, effect_id).cloned()
            && let Some(stack) = crate::ui_preview::PreviewStack::of(node, effect_id, &e)
        {
            crate::ui_preview::show(app, ui, &stack, uv);
        }
    });
    ui.data_mut(|d| d.insert_persisted(uv_id, uv));
}

fn effect_editor(app: &mut App, ui: &mut egui::Ui, node: &Node, effect_id: u64) {
    let Some(e) = find_effect(node, effect_id).cloned() else { return };
    let Some(def) = registry().get(&e.def) else {
        ui.colored_label(theme::DANGER, format!("{} '{}'", t("Unknown effect"), e.def));
        return;
    };
    let mut x = e.clone();
    let mut edit = Edit::default();
    let ru = crate::i18n::is_russian();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} {}", effect_icon(def.category), def.display_name(ru))).strong().size(13.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.selectable_label(x.anchor, icon::ANCHOR).on_hover_text(t("Anchor: other effects can read this result")).clicked() {
                x.anchor = !x.anchor;
                edit.structural();
            }
            let r = ui.checkbox(&mut x.enabled, "").on_hover_text(t("Enabled"));
            if r.changed() {
                edit.structural();
            }
        });
    });
    if let Some(d) = pick(def.description.as_deref().unwrap_or(""), def.description_ru.as_deref()).to_owned().into() {
        if !d.is_empty() {
            ui.label(RichText::new(d).color(theme::TEXT_FAINT).small());
        }
    }
    effect_preview(app, ui, node, effect_id);
    if let Some(err) = app.viewport.paint.errors.get(&e.id) {
        ui.colored_label(theme::DANGER, format!("{} {}", icon::WARNING, t("Effect failed:")));
        ui.label(RichText::new(err.lines().take(4).collect::<Vec<_>>().join("\n")).small().color(theme::DANGER));
    }
    if !def.maps.is_empty() && !app.bake.has_baked {
        ui.horizontal(|ui| {
            ui.colored_label(theme::WARNING, format!("{} {}", icon::WARNING, t("Needs baked mesh maps")));
            if ui.small_button(t("Bake…")).clicked() {
                app.bake.open = true;
            }
        });
    }
    // Common: blend, opacity, channels, presets.
    ui.horizontal(|ui| {
        if blend_combo(ui, ("fx_blend", e.id), &mut x.blend) {
            edit.structural();
        }
        let mut pct = x.opacity * 100.0;
        let r = ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).suffix("%").max_decimals(0));
        if r.changed() {
            x.opacity = pct / 100.0;
        }
        edit.note(&r);
        if !def.presets.is_empty() {
            egui::ComboBox::from_id_salt(("presets", e.id)).selected_text(t("Preset")).width(90.0).show_ui(ui, |ui| {
                for p in &def.presets {
                    if ui.button(pick(&p.name, p.name_ru.as_deref())).clicked() {
                        def.apply_preset(&mut x, p);
                        edit.structural();
                    }
                }
            });
        }
    });
    let on_content = node.effects.iter().any(|c| c.id == e.id) || node.is_effect_layer();
    if on_content {
        let doc_channels = app.viewport.paint.doc.channels;
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Acts on"));
            for c in doc_channels.iter() {
                let on = x.channels.has(c);
                if chip(ui, c.short(), on, t(c.label())).clicked() {
                    x.channels.set(c, !on);
                    edit.structural();
                }
            }
        });
    }
    ui.separator();
    let mut drag = app.direction_drag.clone();
    params_editor(app, ui, def, &mut x, &mut edit, node.id, &mut drag);
    app.direction_drag = drag;
    if edit.changed {
        let id = e.id;
        app.viewport.paint.edit_node(node.id, edit.checkpoint, |n| with_effect(n, id, |dst| *dst = x.clone()));
    }
}

fn params_editor(app: &App, ui: &mut egui::Ui, def: &EffectDef, e: &mut EffectInstance, edit: &mut Edit, node_id: u64, drag: &mut Option<(u64, u64, String)>) {
    let params = def.all_params();
    let mut groups: Vec<(Option<String>, Option<String>)> = Vec::new();
    for p in &params {
        let g = (p.group.clone(), p.group_ru.clone());
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    let eid = e.id;
    // Parameters drawn by a graphical widget (gradient, levels, range) are not rows.
    let owned: std::collections::HashSet<&str> = def.widgets.iter().flat_map(|w| w.params()).collect();
    for (group, group_ru) in groups {
        let in_group: Vec<&ParamDef> = params.iter().filter(|p| p.group == group && !owned.contains(p.id.as_str())).collect();
        // A widget sits in the group of its first parameter.
        let widgets: Vec<(usize, &WidgetDef)> = def
            .widgets
            .iter()
            .enumerate()
            .filter(|(_, w)| w.params().first().and_then(|id| params.iter().find(|p| p.id == *id)).is_some_and(|p| p.group == group))
            .collect();
        let body = |ui: &mut egui::Ui| {
            for (i, w) in &widgets {
                widget(ui, def, e, *i, w, edit);
                ui.add_space(4.0);
            }
            if in_group.is_empty() {
                return;
            }
            let labels = in_group.iter().map(|p| pick(&p.label, p.label_ru.as_deref()));
            let w = label_width(ui, labels, 60.0);
            fit_sliders(ui, w);
            egui::Grid::new(("params", eid, group.clone())).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                for p in in_group {
                    param_row(app, ui, def, e, p, edit, node_id, drag);
                    ui.end_row();
                }
            });
        };
        match &group {
            Some(g) => {
                let title = pick(g, group_ru.as_deref()).to_owned();
                // The shared generator blocks start collapsed.
                let open = !matches!(g.as_str(), "Global" | "Grunge");
                egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt(("group", eid, g)).default_open(open).show(ui, |ui| body(ui));
            }
            None => body(ui),
        }
    }
}

/// A graphical editor over some of the effect's parameters (see `WidgetDef`).
fn widget(ui: &mut egui::Ui, def: &EffectDef, e: &mut EffectInstance, index: usize, w: &WidgetDef, edit: &mut Edit) {
    use crate::ui_widgets as uw;
    let params = def.all_params();
    let find = |id: &str| params.iter().find(|p| p.id == id);
    let get = |e: &EffectInstance, id: &str| find(id).map(|p| def.value(e, p));
    let float = |e: &EffectInstance, id: &str| get(e, id).map_or(0.0, |v| v.as_f32());
    let id = egui::Id::new(("fx_widget", e.id, index));
    match w {
        WidgetDef::Gradient { colors, positions, count, interp } => {
            let mut stops: Vec<uw::Stop> = colors
                .iter()
                .enumerate()
                .map(|(i, c)| uw::Stop {
                    color: match get(e, c) {
                        Some(ParamValue::Color(v)) => v,
                        _ => [0.5; 3],
                    },
                    pos: positions.get(i).map(|p| float(e, p)),
                })
                .collect();
            let mut n = count.as_deref().map(|c| float(e, c) as i32);
            let limits = count.as_deref().and_then(find).map_or((1, colors.len() as i32), |p| match p.kind {
                ParamKind::Int { min, max, .. } => (min, max.min(colors.len() as i32)),
                _ => (1, colors.len() as i32),
            });
            let mode = interp.as_deref().map_or(0, |i| float(e, i) as u32);
            let resp = uw::gradient(ui, id, &mut stops, n.as_mut().map(|c| (c, limits.0, limits.1)), mode);
            edit.note(&resp);
            if resp.changed() {
                for (i, s) in stops.iter().enumerate() {
                    e.params.insert(colors[i].clone(), ParamValue::Color(s.color));
                    if let (Some(pid), Some(v)) = (positions.get(i), s.pos) {
                        e.params.insert(pid.clone(), ParamValue::Float(v));
                    }
                }
                if let (Some(c), Some(v)) = (count, n) {
                    e.params.insert(c.clone(), ParamValue::Int(v));
                }
            }
        }
        WidgetDef::Levels { in_black, in_white, gamma, out_black, out_white } => {
            let ids = [in_black, in_white, gamma, out_black, out_white];
            let mut v = ids.map(|i| float(e, i));
            let resp = uw::levels(ui, id, &mut v);
            edit.note(&resp);
            if resp.changed() {
                for (pid, val) in ids.iter().zip(v) {
                    e.params.insert((*pid).clone(), ParamValue::Float(val));
                }
            }
        }
        WidgetDef::Range { min, max, label, label_ru } => {
            if let Some(l) = label.as_deref() {
                ui.label(pick(l, label_ru.as_deref()));
            }
            let (lo_lim, hi_lim) = find(min).map_or((0.0, 1.0), |p| match p.kind {
                ParamKind::Float { min, max, .. } => (min, max),
                _ => (0.0, 1.0),
            });
            let (mut lo, mut hi) = (float(e, min), float(e, max));
            let resp = uw::range(ui, id, &mut lo, &mut hi, lo_lim, hi_lim, true, [theme::TEXT_DIM, theme::TEXT]);
            edit.note(&resp);
            if resp.changed() {
                e.params.insert(min.clone(), ParamValue::Float(lo));
                e.params.insert(max.clone(), ParamValue::Float(hi));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn param_row(app: &App, ui: &mut egui::Ui, def: &EffectDef, e: &mut EffectInstance, p: &ParamDef, edit: &mut Edit, node_id: u64, drag: &mut Option<(u64, u64, String)>) {
    let label = pick(&p.label, p.label_ru.as_deref());
    let tip = pick(p.tip.as_deref().unwrap_or(""), p.tip_ru.as_deref());
    let mut l = ui.label(label);
    if !tip.is_empty() {
        l = l.on_hover_text(tip);
    }
    let mut value = def.value(e, p);
    let response = match (&p.kind, &mut value) {
        (ParamKind::Float { min, max, logarithmic, .. }, ParamValue::Float(v)) => ui.add(egui::Slider::new(v, *min..=*max).logarithmic(*logarithmic).max_decimals(3)),
        (ParamKind::Int { min, max, .. }, ParamValue::Int(v)) => ui.add(egui::Slider::new(v, *min..=*max)),
        (ParamKind::Seed { .. }, ParamValue::Int(v)) => {
            ui.horizontal(|ui| {
                let r = ui.add(egui::DragValue::new(v).range(0..=99999));
                if ui.small_button(icon::DICE_FIVE).on_hover_text(t("Random")).clicked() {
                    *v = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.subsec_nanos()) % 99999) as i32;
                    edit.structural();
                }
                r
            })
            .inner
        }
        (ParamKind::Bool { .. }, ParamValue::Bool(v)) => ui.checkbox(v, ""),
        (ParamKind::Color { .. }, ParamValue::Color(c)) => {
            let mut col = to_color32(c.map(s3dp_core::document::linear_to_srgb));
            let r = egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque);
            if r.changed() {
                *c = from_color32(col).map(s3dp_core::document::srgb_to_linear);
            }
            r
        }
        (ParamKind::Enum { options, .. }, ParamValue::Enum(v)) => {
            let before = *v;
            let current = options.get(*v as usize).map_or("?", |o| pick(&o.1, Some(&o.2)));
            let r = egui::ComboBox::from_id_salt(("enum", e.id, &p.id))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, o) in options.iter().enumerate() {
                        ui.selectable_value(v, i as u32, pick(&o.1, Some(&o.2)));
                    }
                })
                .response;
            if *v != before {
                edit.structural();
            }
            r
        }
        (ParamKind::Direction { .. }, ParamValue::Vec3(d)) => {
            let active = drag.as_ref().is_some_and(|(_, eid, pid)| *eid == e.id && *pid == p.id);
            // One grid cell: the direction sphere, the viewport-aim toggle and both angles.
            ui.horizontal(|ui| {
                let sphere = crate::ui_widgets::direction_sphere(ui, d, 46.0);
                let r = ui
                    .vertical(|ui| {
                        if ui.selectable_label(active, format!("{} {}", icon::SUN, t("Aim in viewport"))).on_hover_text(t("Drag in the viewport to aim")).clicked() {
                            *drag = if active { None } else { Some((node_id, e.id, p.id.clone())) };
                        }
                        let dir = glam::Vec3::from(*d).normalize_or(glam::Vec3::Y);
                        let mut elevation = dir.y.clamp(-1.0, 1.0).asin().to_degrees();
                        let mut azimuth = dir.x.atan2(dir.z).to_degrees();
                        let r = ui
                            .horizontal(|ui| {
                                let a = ui.add(egui::DragValue::new(&mut elevation).range(-90.0..=90.0).suffix("°").speed(1.0)).on_hover_text(t("Elevation"));
                                let b = ui.add(egui::DragValue::new(&mut azimuth).range(-180.0..=180.0).suffix("°").speed(1.0)).on_hover_text(t("Azimuth"));
                                a.union(b)
                            })
                            .inner;
                        if r.changed() {
                            let (el, az) = (elevation.to_radians(), azimuth.to_radians());
                            *d = [el.cos() * az.sin(), el.sin(), el.cos() * az.cos()];
                        }
                        r
                    })
                    .inner;
                sphere.union(r)
            })
            .inner
        }
        (ParamKind::Anchor, ParamValue::Anchor(a)) => {
            let anchors: Vec<(u64, String)> = app
                .viewport
                .paint
                .doc
                .anchors()
                .into_iter()
                .filter(|(_, x)| x.id != e.id)
                .map(|(node, x)| {
                    let node_name = app.viewport.paint.doc.find(node).map_or(String::new(), |n| n.name.clone());
                    let fx = registry().get(&x.def).map_or(x.def.clone(), |d| d.display_name(crate::i18n::is_russian()).to_owned());
                    (x.id, format!("{node_name} › {fx}"))
                })
                .collect();
            let before = *a;
            let current = a.and_then(|id| anchors.iter().find(|x| x.0 == id)).map_or(t("None").to_owned(), |x| x.1.clone());
            let r = egui::ComboBox::from_id_salt(("anchor", e.id, &p.id))
                .selected_text(current)
                .width(150.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(a, None, t("None"));
                    for (id, name) in &anchors {
                        ui.selectable_value(a, Some(*id), name);
                    }
                })
                .response;
            if *a != before {
                edit.structural();
            }
            r
        }
        _ => ui.label(RichText::new("?").color(theme::DANGER)),
    };
    edit.note(&response);
    // Right click on the label resets to the default (spec §8).
    if l.secondary_clicked() || response.secondary_clicked() {
        value = p.default_value();
        edit.structural();
    }
    let _ = l.on_hover_text(t("Right click: reset"));
    e.params.insert(p.id.clone(), value);
}
