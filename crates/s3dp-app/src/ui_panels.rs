//! Menu bar, tool bar, options bar, status bar and the right dock (layers on top, properties
//! below, as in Substance — spec §8).

use egui::{Color32, RichText};
use egui_phosphor::regular as icon;
use s3dp_core::{BakedMap, ChannelId};
use s3dp_render::ShadingMode;

use crate::app::{App, PropTab, TEXTURE_SIZES, Tool};
use crate::i18n::t;
use crate::{theme, ui_effects, ui_layers};

pub fn to_color32(c: [f32; 3]) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

pub fn from_color32(c: Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

/// Whether an edit from `response` starts a new undo step: the first frame of a drag, or any
/// non-drag change (click, typing).
pub fn starts_edit(response: &egui::Response) -> bool {
    response.drag_started() || !response.dragged()
}

/// Slider width so a grid row (label + slider + value box) fits the panel; `label` is the
/// width reserved for the widest label of the grid.
pub fn fit_sliders(ui: &mut egui::Ui, label: f32) {
    ui.spacing_mut().slider_width = (ui.available_width() - label - 56.0 - 24.0).max(40.0);
}

/// Width of the widest label (body text, no wrap), at least `min`: the label column of a
/// slider grid, so long (Russian) labels do not push the rows past the panel.
pub fn label_width<'a>(ui: &egui::Ui, labels: impl IntoIterator<Item = &'a str>, min: f32) -> f32 {
    labels
        .into_iter()
        .map(|l| egui::WidgetText::from(l).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x)
        .fold(min, f32::max)
}

/// Square icon button; highlighted when `selected`.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, selected: bool, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 30.0), egui::Sense::click());
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
    let (rect, response) = ui.allocate_exact_size(egui::vec2(galley.size().x + 12.0, 20.0), egui::Sense::click());
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

/// Header strip of a dock panel with tabs. Returns the clicked tab.
fn tab_strip<T: PartialEq + Copy>(ui: &mut egui::Ui, tabs: &[(T, &str)], current: T) -> Option<T> {
    let mut clicked = None;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::DOCK);
    let mut x = rect.left() + 4.0;
    for (tab, title) in tabs {
        let galley = ui.painter().layout_no_wrap(title.to_string(), egui::FontId::proportional(12.0), theme::TEXT);
        let w = galley.size().x + 22.0;
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 3.0), egui::vec2(w, 23.0));
        let resp = ui.interact(r, ui.id().with(("tab", title)), egui::Sense::click());
        let fill = if *tab == current { theme::CHROME } else if resp.hovered() { theme::HOVER.gamma_multiply(0.5) } else { theme::DOCK };
        ui.painter().rect_filled(r, egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, fill);
        let color = if *tab == current { theme::TEXT } else { theme::TEXT_DIM };
        ui.painter().galley_with_override_text_color(r.left_center() + egui::vec2(11.0, -galley.size().y / 2.0), galley, color);
        if resp.clicked() {
            clicked = Some(*tab);
        }
        x += w + 2.0;
    }
    clicked
}

// ---------------------------------------------------------------------- menu

pub fn menu_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button(t("File"), |ui| {
            if ui.add(egui::Button::new(t("New from model…")).shortcut_text("Ctrl+N")).clicked() {
                ui.close();
                app.open_model_dialog();
            }
            if ui.add(egui::Button::new(t("Open project…")).shortcut_text("Ctrl+O")).clicked() {
                ui.close();
                app.open_project_dialog();
            }
            ui.separator();
            if ui.add(egui::Button::new(t("Save")).shortcut_text("Ctrl+S")).clicked() {
                ui.close();
                app.save(false);
            }
            if ui.add(egui::Button::new(t("Save as…")).shortcut_text("Ctrl+Shift+S")).clicked() {
                ui.close();
                app.save(true);
            }
            ui.separator();
            if ui.add(egui::Button::new(t("Export textures…")).shortcut_text("Ctrl+Shift+E")).clicked() {
                ui.close();
                app.open_export();
            }
            ui.separator();
            if ui.add(egui::Button::new(t("Quit")).shortcut_text("Ctrl+Q")).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button(t("Edit"), |ui| {
            let paint = &app.viewport.paint;
            let (can_undo, can_redo) = (paint.can_undo(), paint.can_redo());
            if ui.add_enabled(can_undo, egui::Button::new(t("Undo")).shortcut_text("Ctrl+Z")).clicked() {
                app.viewport.paint.undo();
            }
            if ui.add_enabled(can_redo, egui::Button::new(t("Redo")).shortcut_text("Ctrl+Shift+Z")).clicked() {
                app.viewport.paint.redo();
            }
        });
        ui.menu_button(t("Layer"), |ui| ui_layers::layer_menu(app, ui));
        ui.menu_button(t("Bake"), |ui| {
            if ui.add(egui::Button::new(format!("{} {}", icon::FIRE, t("Bake mesh maps…"))).shortcut_text("Ctrl+B")).clicked() {
                app.bake.open = true;
                ui.close();
            }
        });
        ui.menu_button(t("View"), |ui| {
            shading_menu(app, ui);
            ui.separator();
            let mut ru = crate::i18n::is_russian();
            if ui.checkbox(&mut ru, "Русский интерфейс / Russian UI").changed() {
                crate::i18n::set_russian(ru);
            }
        });
        ui.menu_button(t("Help"), |ui| {
            ui.label(RichText::new(t("Shortcuts")).strong());
            ui.label(t(
                "B brush · E eraser · I eyedropper · [ ] size · X swap colors · D default colors\n\
                 Alt+LMB / MMB orbit · RMB / Shift+MMB pan · Wheel zoom · F frame\n\
                 F1–F5 viewport mode · Ctrl+Z / Ctrl+Shift+Z undo / redo\n\
                 Ctrl+Shift+N new layer · Ctrl+G group · Ctrl+J duplicate · Del delete\n\
                 Ctrl+B bake · Ctrl+Shift+E export · Ctrl+S save",
            ));
        });
    });
}

pub fn shading_menu(app: &mut App, ui: &mut egui::Ui) {
    for (i, mode) in ShadingMode::MAIN.into_iter().enumerate() {
        ui.radio_value(&mut app.shading, mode, format!("{}   F{}", t(mode.label()), i + 1));
    }
    ui.menu_button(t("Channel"), |ui| {
        for c in app.viewport.paint.doc.channels.iter() {
            ui.radio_value(&mut app.shading, ShadingMode::Channel(c), t(c.label()));
        }
    });
    ui.menu_button(t("Mesh map"), |ui| {
        for m in BakedMap::ALL {
            ui.radio_value(&mut app.shading, ShadingMode::Baked(m), t(m.label()));
        }
    });
    ui.radio_value(&mut app.shading, ShadingMode::Mask, t("Mask of the selected layer"));
}

// ---------------------------------------------------------------------- tool bar

pub fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        for tool in [Tool::Brush, Tool::Eraser, Tool::Eyedropper] {
            if icon_button(ui, tool.icon(), app.tool == tool, &tool.label()).clicked() {
                app.tool = tool;
            }
        }
        ui.add_space(12.0);
        color_swatches(app, ui);
    });
}

/// Photoshop-style foreground / background swatches.
fn color_swatches(app: &mut App, ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 46.0), egui::Sense::hover());
    let fg = egui::Rect::from_min_size(rect.min + egui::vec2(2.0, 2.0), egui::vec2(22.0, 22.0));
    let bg = fg.translate(egui::vec2(11.0, 11.0));
    let bg_resp = ui.interact(bg, ui.id().with("bg_swatch"), egui::Sense::click());
    let fg_resp = ui.interact(fg, ui.id().with("fg_swatch"), egui::Sense::click());
    let painter = ui.painter();
    for (r, c) in [(bg, app.bg_color), (fg, app.fg_srgb())] {
        painter.rect_filled(r, 1.0, to_color32(c));
        painter.rect_stroke(r, 1.0, egui::Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
        painter.rect_stroke(r.shrink(1.0), 1.0, egui::Stroke::new(1.0, Color32::from_white_alpha(90)), egui::StrokeKind::Inside);
    }
    let swap = egui::Rect::from_min_size(rect.min + egui::vec2(25.0, 0.0), egui::vec2(12.0, 12.0));
    let reset = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 34.0), egui::vec2(12.0, 12.0));
    let swap_resp = ui.interact(swap, ui.id().with("swap"), egui::Sense::click()).on_hover_text(t("Swap colors (X)"));
    let reset_resp = ui.interact(reset, ui.id().with("reset"), egui::Sense::click()).on_hover_text(t("Default colors (D)"));
    let small = egui::FontId::proportional(11.0);
    painter.text(swap.center(), egui::Align2::CENTER_CENTER, icon::ARROWS_LEFT_RIGHT, small.clone(), theme::TEXT_DIM);
    painter.text(reset.center(), egui::Align2::CENTER_CENTER, icon::SQUARE_HALF, small, theme::TEXT_DIM);
    if swap_resp.clicked() || bg_resp.on_hover_text(t("Background color — click to swap (X)")).clicked() {
        app.swap_colors();
    }
    if reset_resp.clicked() {
        app.reset_colors();
    }
    if fg_resp.on_hover_text(t("Foreground color — edit it in the Brush tab")).clicked() {
        app.prop_tab = PropTab::Brush;
    }
}

// ---------------------------------------------------------------------- options bar

pub fn options_bar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal_centered(|ui| {
        ui.label(RichText::new(app.tool.icon()).size(18.0).color(theme::TEXT));
        ui.separator();
        if app.tool == Tool::Eyedropper {
            ui.label(RichText::new(t("Click the model to pick every channel of the material under the cursor")).color(theme::TEXT_DIM));
            return;
        }
        ui.label(t("Size"));
        ui.add(egui::DragValue::new(&mut app.brush.size).range(1.0..=500.0).speed(0.5).suffix(" px"));
        ui.label(t("Hardness"));
        percent(ui, &mut app.brush.hardness, 0.0);
        ui.label(t("Opacity"));
        percent(ui, &mut app.brush.opacity, 0.0);
        ui.label(t("Flow"));
        percent(ui, &mut app.brush.flow, 0.01);
        ui.separator();
        ui.label(egui_phosphor::regular::PEN_NIB).on_hover_text(t("Pen pressure"));
        if chip(ui, t("Size"), app.brush.pressure_size, t("Pen pressure controls the brush size")).clicked() {
            app.brush.pressure_size = !app.brush.pressure_size;
        }
        if app.brush.pressure_size {
            ui.label(t("Min"));
            percent(ui, &mut app.brush.min_size, 0.0);
        }
        if chip(ui, t("Flow"), app.brush.pressure_flow, t("Pen pressure controls the flow")).clicked() {
            app.brush.pressure_flow = !app.brush.pressure_flow;
        }
        ui.separator();
        ui.label(t("Channels"));
        let what = if app.tool == Tool::Eraser { t("erased by the eraser") } else { t("painted by the brush") };
        for c in app.viewport.paint.doc.channels.iter() {
            let on = app.brush.channels.has(c);
            if chip(ui, c.short(), on, &format!("{} — {what}", t(c.label()))).clicked() {
                app.brush.channels.set(c, !on);
            }
        }
        let paint = &app.viewport.paint;
        if let Some(node) = paint.active_node() {
            ui.separator();
            let text = match paint.target {
                s3dp_render::EditTarget::MaskItem(_) => RichText::new(format!("{} {} «{}»", icon::SQUARE_HALF, t("Painting mask of"), node.name)).color(theme::ACCENT),
                s3dp_render::EditTarget::Content => RichText::new(format!("{} {} «{}»", icon::STACK, t("Painting"), node.name)).color(theme::TEXT_DIM),
            };
            ui.label(text);
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
        if let Some(p) = app.viewport.bake_progress() {
            ui.add(egui::ProgressBar::new(p).desired_width(160.0).show_percentage());
            ui.label(RichText::new(t("Baking mesh maps…")).color(theme::TEXT_DIM));
        } else {
            ui.label(RichText::new(&app.status).color(theme::TEXT_DIM));
        }
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
                .on_hover_text(t("Texture resolution (changing it resets the layer stack)"));
            app.set_texture_size(size);
            ui.label(RichText::new(t("Texture")).color(theme::TEXT_FAINT));
            if !app.viewport.paint.errors.is_empty() {
                ui.colored_label(theme::DANGER, format!("{} {} {}", icon::WARNING, app.viewport.paint.errors.len(), t("effect error(s)")));
            }
            if app.is_modified() {
                ui.label(RichText::new(format!("● {}", t("unsaved"))).color(theme::TEXT_FAINT));
            }
        });
    });
}

// ---------------------------------------------------------------------- dock

/// Width of the right dock (fixed: widgets size themselves from it).
pub const DOCK_WIDTH: f32 = 350.0;

pub fn dock(app: &mut App, ui: &mut egui::Ui) {
    // Contents go into a child Ui that never grows the panel: egui sizes a right panel whose
    // content overflows from the content's right edge, which slid the viewport over the left
    // part of the dock. Anything too wide is clipped instead.
    let rect = ui.max_rect();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    dock_contents(app, &mut child);
    if child.min_rect().width() > rect.width() + 1.0 {
        log::debug!("dock content overflows: {:.0} > {:.0}", child.min_rect().width(), rect.width());
    }
    ui.advance_cursor_after_rect(rect);
}

fn dock_contents(app: &mut App, ui: &mut egui::Ui) {
    let width = DOCK_WIDTH;
    ui.set_max_width(width);
    ui.spacing_mut().item_spacing.y = 0.0;
    let total = ui.available_height();
    let layers_h = (total * 0.5).clamp(220.0, total - 220.0);
    tab_strip(ui, &[(0, t("Layers"))], 0);
    egui::Frame::new().fill(theme::CHROME).show(ui, |ui| {
        ui.set_width(width);
        ui.set_height(layers_h - 26.0);
        ui_layers::layers_panel(app, ui);
    });
    let tabs = [(PropTab::Properties, t("Properties")), (PropTab::Brush, t("Brush")), (PropTab::Project, t("Project"))];
    if let Some(tab) = tab_strip(ui, &tabs, app.prop_tab) {
        app.prop_tab = tab;
    }
    egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(width - 20.0);
        ui.set_min_height(ui.available_height());
        egui::ScrollArea::vertical().id_salt("properties").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            match app.prop_tab {
                PropTab::Properties => ui_effects::properties(app, ui),
                PropTab::Brush => brush_tab(app, ui),
                PropTab::Project => project_tab(app, ui),
            }
        });
    });
}

fn brush_tab(app: &mut App, ui: &mut egui::Ui) {
    ui.label(RichText::new(t("Color")).strong());
    let mut srgb = app.fg_srgb();
    if crate::color_picker::picker(ui, &mut app.picker, &mut srgb, 120.0) {
        app.set_fg_srgb(srgb);
    }
    ui.add_space(6.0);
    ui.label(RichText::new(t("Brush material")).strong());
    ui.label(RichText::new(t("Values the brush paints into each checked channel.")).color(theme::TEXT_FAINT).small());
    fit_sliders(ui, 110.0);
    let channels = app.viewport.paint.doc.channels;
    egui::Grid::new("brush_material").num_columns(3).spacing([6.0, 5.0]).show(ui, |ui| {
        for c in channels.iter().filter(|c| *c != ChannelId::BaseColor) {
            let mut on = app.brush.channels.has(c);
            if ui.checkbox(&mut on, "").on_hover_text(t("Paint this channel")).changed() {
                app.brush.channels.set(c, on);
            }
            ui.label(t(c.label()));
            let mut v = app.fg.get(c);
            if c == ChannelId::Emissive {
                let mut col = to_color32(v.map(s3dp_core::document::linear_to_srgb));
                if egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque).changed() {
                    v = from_color32(col).map(s3dp_core::document::srgb_to_linear);
                }
            } else if c == ChannelId::Normal {
                ui.label(RichText::new(t("flat")).color(theme::TEXT_FAINT));
            } else {
                ui.add(egui::Slider::new(&mut v[0], 0.0..=1.0).max_decimals(2));
                v = [v[0]; 3];
            }
            app.fg.set(c, v);
            ui.end_row();
        }
    });
}

fn project_tab(app: &mut App, ui: &mut egui::Ui) {
    ui.label(RichText::new(t("Base material")).strong());
    ui.label(RichText::new(t("Values under all layers")).color(theme::TEXT_FAINT).small());
    let mut d = app.viewport.paint.doc.defaults.clone();
    let mut edit = ui_effects::Edit::default();
    ui_effects::values_editor(ui, "defaults", &mut d, app.viewport.paint.doc.channels, &mut edit);
    if edit.changed {
        app.viewport.paint.set_defaults(d, edit.checkpoint);
    }
    ui.add_space(6.0);
    ui.label(RichText::new(t("Channels")).strong());
    let mut channels = app.viewport.paint.doc.channels;
    ui.horizontal_wrapped(|ui| {
        for c in ChannelId::ALL {
            let on = channels.has(c);
            if chip(ui, c.short(), on, t(c.label())).clicked() {
                channels.set(c, !on);
            }
        }
    });
    channels.set(ChannelId::BaseColor, true);
    if channels != app.viewport.paint.doc.channels {
        app.viewport.paint.edit_document(true, |d| d.channels = channels);
        let rs = app.render_state.clone();
        app.viewport.paint.channels_changed(&rs.device, &rs.queue);
    }
    let sets: Vec<String> = app.viewport.paint.sets.iter().map(|s| s.name.clone()).collect();
    if sets.len() > 1 {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(t("Thumbnails of"));
            let mut current = app.viewport.paint.thumb_set;
            egui::ComboBox::from_id_salt("thumb_set").selected_text(&sets[current.min(sets.len() - 1)]).show_ui(ui, |ui| {
                for (i, name) in sets.iter().enumerate() {
                    ui.selectable_value(&mut current, i, name);
                }
            });
            if current != app.viewport.paint.thumb_set {
                app.viewport.paint.thumb_set = current;
                app.viewport.paint.invalidate_thumbs();
                app.viewport.paint.mark_all_dirty();
            }
        });
    }
}

#[cfg(test)]
mod dock_layout_tests {
    /// Central panel rect next to a fixed 350 px right panel whose content is 530 px wide.
    fn central_max_x(isolate: bool) -> (f32, f32) {
        let ctx = egui::Context::default();
        let mut out = (0.0, 0.0);
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), ..Default::default() };
        for _ in 0..3 {
            let mut full = ctx.run_ui(input.clone(), |ui| {
                let dock = egui::Panel::right("dock").exact_size(350.0).resizable(false).show(ui, |ui| {
                    let wide = |ui: &mut egui::Ui| {
                        ui.allocate_exact_size(egui::vec2(530.0, 20.0), egui::Sense::hover());
                    };
                    if isolate {
                        let rect = ui.max_rect();
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                        child.set_clip_rect(rect);
                        wide(&mut child);
                        ui.advance_cursor_after_rect(rect);
                    } else {
                        wide(ui);
                    }
                });
                let central = egui::CentralPanel::default().show(ui, |ui| ui.available_rect_before_wrap());
                out = (central.inner.max.x, dock.response.rect.min.x);
            });
            full.textures_delta.clear();
        }
        out
    }

    #[test]
    fn overflowing_dock_does_not_slide_under_the_viewport() {
        let (central, dock) = central_max_x(false);
        println!("plain: central ends {central}, dock starts {dock}");
        let (central, dock) = central_max_x(true);
        println!("isolated: central ends {central}, dock starts {dock}");
        assert!(central <= dock + 0.5 && dock <= 1200.0 - 349.0, "central panel overlaps the dock: {central} > {dock}");
    }
}
