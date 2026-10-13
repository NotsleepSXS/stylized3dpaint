//! Dockable panels (egui_dock): the viewport, Layers, Properties, Brush and Project are tabs
//! that can be dragged anywhere, split, resized and floated. The layout is saved to
//! `~/.config/stylized3dpaint/layout.ron` and restored on start (RON: the dock state holds
//! non-finite rects that JSON would turn into nulls).

use std::path::PathBuf;

use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use egui_phosphor::regular as icon;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::i18n::t;
use crate::{theme, ui_effects, ui_layers, ui_panels};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Viewport,
    Layers,
    Properties,
    Brush,
    Project,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Viewport, Tab::Layers, Tab::Properties, Tab::Brush, Tab::Project];

    /// English UI key (translated with `t`).
    pub fn label(self) -> &'static str {
        match self {
            Tab::Viewport => "Viewport",
            Tab::Layers => "Layers",
            Tab::Properties => "Properties",
            Tab::Brush => "Brush",
            Tab::Project => "Project",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Viewport => icon::CUBE,
            Tab::Layers => icon::STACK,
            Tab::Properties => icon::SLIDERS_HORIZONTAL,
            Tab::Brush => icon::PAINT_BRUSH,
            Tab::Project => icon::FOLDER_OPEN,
        }
    }
}

/// Viewport on the left; on the right Properties (with Brush / Project tabs) above Layers.
pub fn default_layout() -> DockState<Tab> {
    let mut state = DockState::new(vec![Tab::Viewport]);
    let tree = state.main_surface_mut();
    let [_, right] = tree.split_right(NodeIndex::root(), 0.76, vec![Tab::Properties, Tab::Brush, Tab::Project]);
    tree.split_below(right, 0.5, vec![Tab::Layers]);
    state
}

fn layout_path() -> Option<PathBuf> {
    Some(crate::paths::config_dir()?.join("layout.ron"))
}

/// The saved layout, or the default when there is none / it is unreadable / lost the viewport.
pub fn load_layout() -> DockState<Tab> {
    let saved = layout_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| ron::from_str::<DockState<Tab>>(&s).ok());
    match saved {
        Some(state) if state.find_tab(&Tab::Viewport).is_some() => state,
        _ => default_layout(),
    }
}

pub fn save_layout(state: &DockState<Tab>) {
    let Some(path) = layout_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match ron::to_string(state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::warn!("cannot save the panel layout: {e}");
            }
        }
        Err(e) => log::warn!("cannot serialize the panel layout: {e}"),
    }
}

/// Shows `tab` (activates it, or reopens it in the focused panel if it was closed).
pub fn open_tab(dock: &mut DockState<Tab>, tab: Tab) {
    if let Some(path) = dock.find_tab(&tab) {
        let _ = dock.set_active_tab(path);
    } else {
        dock.push_to_focused_leaf(tab);
    }
}

pub fn is_open(dock: &DockState<Tab>, tab: Tab) -> bool {
    dock.find_tab(&tab).is_some()
}

pub fn close_tab(dock: &mut DockState<Tab>, tab: Tab) {
    if let Some(path) = dock.find_tab(&tab) {
        let _ = dock.remove_tab(path);
    }
}

struct Viewer<'a> {
    app: &'a mut App,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("dock_tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        format!("{} {}", tab.icon(), t(tab.label())).into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let app = &mut *self.app;
        match tab {
            Tab::Viewport => app.viewport_ui(ui),
            Tab::Layers => {
                egui::Frame::new().fill(theme::CHROME).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    ui_layers::layers_panel(app, ui);
                });
            }
            Tab::Properties => panel_body(ui, "properties", |ui| ui_effects::properties(app, ui)),
            Tab::Brush => panel_body(ui, "brush", |ui| ui_panels::brush_tab(app, ui)),
            Tab::Project => panel_body(ui, "project", |ui| ui_panels::project_tab(app, ui)),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn allowed_in_windows(&self, tab: &mut Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

/// Padded, vertically scrolling panel body.
fn panel_body(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            add(ui);
        });
    });
}

fn style(ui: &egui::Ui) -> Style {
    let mut s = Style::from_egui(ui.style());
    s.tab_bar.bg_fill = theme::DOCK;
    s.tab_bar.height = 26.0;
    s.tab.active.bg_fill = theme::CHROME;
    s.tab.active.text_color = theme::TEXT;
    s.tab.focused.bg_fill = theme::CHROME;
    s.tab.focused.text_color = theme::TEXT;
    s.tab.inactive.bg_fill = theme::DOCK;
    s.tab.inactive.text_color = theme::TEXT_FAINT;
    s.tab.hovered.bg_fill = theme::HOVER;
    s.tab.hovered.text_color = theme::TEXT;
    s.tab.tab_body.bg_fill = theme::CHROME;
    s.tab.tab_body.inner_margin = egui::Margin::ZERO;
    s.separator.width = 3.0;
    s.separator.color_idle = theme::SEPARATOR;
    s.separator.color_hovered = theme::ACCENT;
    s.separator.color_dragged = theme::ACCENT;
    s.main_surface_border_stroke = egui::Stroke::NONE;
    s
}

/// Draws the dock over the remaining area.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let mut dock = std::mem::replace(&mut app.dock, DockState::new(Vec::new()));
    if let Some(tab) = app.focus_tab.take() {
        open_tab(&mut dock, tab);
    }
    DockArea::new(&mut dock)
        .id(egui::Id::new("main_dock"))
        .style(style(ui))
        .show_leaf_collapse_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_inside(ui, &mut Viewer { app });
    app.dock = dock;
}

/// "Window" menu: show / hide panels, reset the layout.
pub fn window_menu(app: &mut App, ui: &mut egui::Ui) {
    for tab in Tab::ALL {
        if tab == Tab::Viewport {
            continue;
        }
        let mut open = is_open(&app.dock, tab);
        if ui.checkbox(&mut open, format!("{} {}", tab.icon(), t(tab.label()))).changed() {
            if open {
                open_tab(&mut app.dock, tab);
            } else {
                close_tab(&mut app.dock, tab);
            }
        }
    }
    ui.separator();
    if ui.button(t("Reset panel layout")).clicked() {
        app.dock = default_layout();
        ui.close();
    }
    ui.label(egui::RichText::new(t("Drag a panel tab to move it; drag the gaps between panels to resize.")).small().color(theme::TEXT_FAINT));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_every_tab_once_and_survives_saving() {
        let d = default_layout();
        for tab in Tab::ALL {
            assert!(d.find_tab(&tab).is_some(), "{tab:?} missing");
        }
        assert_eq!(d.iter_all_tabs().count(), Tab::ALL.len());
        let text = ron::to_string(&d).unwrap();
        let back: DockState<Tab> = ron::from_str(&text).unwrap();
        assert_eq!(back.iter_all_tabs().count(), Tab::ALL.len());
    }

    #[test]
    fn open_close_tabs() {
        let mut d = default_layout();
        close_tab(&mut d, Tab::Brush);
        assert!(!is_open(&d, Tab::Brush));
        open_tab(&mut d, Tab::Brush);
        assert!(is_open(&d, Tab::Brush));
        assert_eq!(d.iter_all_tabs().count(), Tab::ALL.len());
    }
}
