use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use s3dp_core::document::{linear_to_srgb, srgb_to_linear};
use s3dp_core::{Blending, ChannelId, ChannelSet, ChannelValues, Mesh, NodeKind, OrbitCamera, import};
use s3dp_render::{Brush, ShadingMode, Viewport, wgpu};

use crate::i18n::t;
use crate::{project, theme, ui_bake, ui_export, ui_layers, ui_panels};

pub const TEXTURE_SIZES: [u32; 4] = [512, 1024, 2048, 4096];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Eraser,
    Eyedropper,
}

impl Tool {
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Tool::Brush => i::PAINT_BRUSH,
            Tool::Eraser => i::ERASER,
            Tool::Eyedropper => i::EYEDROPPER,
        }
    }

    pub fn label(self) -> String {
        match self {
            Tool::Brush => format!("{} (B)", t("Brush")),
            Tool::Eraser => format!("{} (E)", t("Eraser")),
            Tool::Eyedropper => format!("{} (I) — {}", t("Eyedropper"), t("picks the whole material")),
        }
    }
}

/// Tabs of the lower dock panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropTab {
    Properties,
    Brush,
    Project,
}

/// Brush shape settings shared by brush and eraser (size in logical points).
pub struct BrushSettings {
    pub size: f32,
    pub hardness: f32,
    pub opacity: f32,
    pub flow: f32,
    pub channels: ChannelSet,
    /// Pen pressure controls the size / flow.
    pub pressure_size: bool,
    pub pressure_flow: bool,
    /// Size share at the lightest pressure.
    pub min_size: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 30.0, hardness: 0.6, opacity: 1.0, flow: 1.0, channels: ChannelSet::only(ChannelId::BaseColor), pressure_size: true, pressure_flow: false, min_size: 0.1 }
    }
}

/// Something that would discard unsaved work and needs confirmation first.
enum PendingAction {
    OpenModel(PathBuf),
    OpenProject(PathBuf),
    Quit,
}

pub struct App {
    pub render_state: egui_wgpu::RenderState,
    pub viewport: Viewport,
    viewport_tex: Option<egui::TextureId>,
    camera: OrbitCamera,
    pub shading: ShadingMode,
    pub mesh: Mesh,
    /// Project file for Save, if the document came from / was saved to one.
    pub project_path: Option<PathBuf>,
    saved_revision: u64,
    pub status: String,
    aspect: f32,
    needs_frame: bool,

    pub tool: Tool,
    pub brush: BrushSettings,
    /// Foreground material, LINEAR (base color = foreground color).
    pub fg: ChannelValues,
    /// Background color (sRGB), swapped with X.
    pub bg_color: [f32; 3],
    pub texture_size: u32,
    last_dab: Option<egui::Pos2>,
    /// Pen pressure at the last dab.
    last_pressure: f32,
    /// Native Wayland tablet reader (None on X11 / without the protocol).
    #[cfg(target_os = "linux")]
    tablet: Option<s3dp_tablet::wayland::Tablet>,
    /// The pen in the window: (pressure, eraser end). None for a mouse.
    pub pen: Option<(f32, bool)>,

    thumbs: HashMap<(u64, bool), (egui::TextureId, u64)>,
    pub layers_ui: ui_layers::LayersUi,
    pub prop_tab: PropTab,
    pub blend_clipboard: Option<Blending>,
    pending: Option<PendingAction>,
    allow_close: bool,
    pub picker: crate::color_picker::PickerState,
    pub bake: ui_bake::BakeUi,
    pub export: ui_export::ExportUi,
    /// Last window title sent (sending a viewport command every frame forces repaints).
    title: String,
    last_autosave: std::time::Instant,
    /// A direction parameter being set by dragging over the model: (node, effect, param).
    pub direction_drag: Option<(u64, u64, String)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> anyhow::Result<Self> {
        let render_state = cc.wgpu_render_state.clone().context("wgpu backend is required")?;
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        theme::apply(&cc.egui_ctx);
        let viewport = Viewport::new(&render_state.device);
        let mut fg = ChannelValues::defaults();
        fg.set(ChannelId::BaseColor, [0.85, 0.35, 0.25].map(srgb_to_linear));
        let mut app = Self {
            render_state,
            viewport,
            viewport_tex: None,
            camera: OrbitCamera::default(),
            shading: ShadingMode::default(),
            mesh: Mesh::default(),
            project_path: None,
            saved_revision: 0,
            status: String::new(),
            aspect: 1.5,
            needs_frame: true,
            tool: Tool::Brush,
            brush: BrushSettings::default(),
            fg,
            bg_color: [1.0; 3],
            texture_size: 2048,
            last_dab: None,
            last_pressure: 1.0,
            #[cfg(target_os = "linux")]
            tablet: start_tablet(cc),
            pen: None,
            thumbs: HashMap::new(),
            layers_ui: Default::default(),
            prop_tab: PropTab::Properties,
            blend_clipboard: None,
            pending: None,
            allow_close: false,
            picker: Default::default(),
            bake: Default::default(),
            export: Default::default(),
            title: String::new(),
            last_autosave: std::time::Instant::now(),
            direction_drag: None,
        };
        app.set_mesh(Mesh::uv_sphere(64, 32));
        app.saved_revision = app.viewport.paint.revision();
        if let Some(path) = initial {
            app.open_path(path);
        }
        Ok(app)
    }

    pub fn is_modified(&self) -> bool {
        self.viewport.paint.revision() != self.saved_revision
    }

    pub fn fg_srgb(&self) -> [f32; 3] {
        self.fg.get(ChannelId::BaseColor).map(linear_to_srgb)
    }

    pub fn set_fg_srgb(&mut self, c: [f32; 3]) {
        self.fg.set(ChannelId::BaseColor, c.map(srgb_to_linear));
    }

    fn set_mesh(&mut self, mesh: Mesh) {
        let rs = &self.render_state;
        self.viewport.set_mesh(&rs.device, &rs.queue, &mesh, self.texture_size);
        self.status = format!(
            "{}: {} {}, {} {}{}",
            mesh.name,
            mesh.triangle_count(),
            t("tris"),
            mesh.parts.len(),
            t("texture set(s)"),
            if mesh.has_uvs() { String::new() } else { format!(" — {}", t("no UVs!")) }
        );
        self.mesh = mesh;
        self.needs_frame = true;
        self.last_dab = None;
        self.thumbs.clear();
        self.project_path = None;
        self.bake.has_baked = false;
        self.viewport.cancel_bake();
    }

    fn frame_mesh(&mut self) {
        let b = self.mesh.bounds();
        self.camera.frame(b.center(), b.radius(), self.aspect);
    }

    // ------------------------------------------------------------------ files

    fn is_project(path: &Path) -> bool {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("s3dp"))
    }

    pub fn open_path(&mut self, path: PathBuf) {
        let action = if Self::is_project(&path) { PendingAction::OpenProject(path) } else { PendingAction::OpenModel(path) };
        self.request(action);
    }

    fn request(&mut self, action: PendingAction) {
        if self.is_modified() {
            self.pending = Some(action);
        } else {
            self.run(action);
        }
    }

    fn run(&mut self, action: PendingAction) {
        match action {
            PendingAction::OpenModel(path) => self.load_model(&path),
            PendingAction::OpenProject(path) => self.load_project(&path),
            PendingAction::Quit => self.allow_close = true,
        }
    }

    fn load_model(&mut self, path: &Path) {
        match import::load(path) {
            Ok(mesh) => {
                self.set_mesh(mesh);
                self.saved_revision = self.viewport.paint.revision();
            }
            Err(e) => {
                log::error!("{e:#}");
                self.status = format!("{}: {e:#}", t("Error"));
            }
        }
    }

    fn load_project(&mut self, path: &Path) {
        let rs = self.render_state.clone();
        match project::load(path, &mut self.viewport, &rs.device, &rs.queue) {
            Ok((mesh, bake)) => {
                self.bake.has_baked = bake.is_some();
                if let Some(settings) = bake {
                    self.bake.settings = settings;
                }
                self.texture_size = self.viewport.texture_size();
                self.mesh = mesh;
                self.needs_frame = true;
                self.thumbs.clear();
                self.project_path = Some(path.to_owned());
                self.saved_revision = self.viewport.paint.revision();
                let missing = self.viewport.paint.missing_effects();
                self.status = if missing.is_empty() {
                    format!("{} {}", t("Opened"), path.display())
                } else {
                    format!("{} {} — {}: {}", t("Opened"), path.display(), t("unknown effects"), missing.join(", "))
                };
            }
            Err(e) => {
                log::error!("{e:#}");
                self.status = format!("{}: {e:#}", t("Error"));
                let mesh = std::mem::take(&mut self.mesh);
                self.set_mesh(mesh);
            }
        }
    }

    pub fn open_model_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().set_title(t("New project from model")).add_filter(t("3D models"), import::SUPPORTED_EXTENSIONS).pick_file() {
            self.request(PendingAction::OpenModel(path));
        }
    }

    pub fn open_project_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().set_title(t("Open project")).add_filter("Stylized3DPaint", &["s3dp"]).pick_file() {
            self.request(PendingAction::OpenProject(path));
        }
    }

    pub fn save(&mut self, save_as: bool) {
        let path = match (&self.project_path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(mut p) = rfd::FileDialog::new()
                    .set_title(t("Save project"))
                    .add_filter("Stylized3DPaint", &["s3dp"])
                    .set_file_name(format!("{}.s3dp", self.mesh.name))
                    .save_file()
                else {
                    return;
                };
                if !Self::is_project(&p) {
                    p.set_extension("s3dp");
                }
                p
            }
        };
        if self.save_to(&path) {
            self.saved_revision = self.viewport.paint.revision();
            self.status = format!("{} {}", t("Saved"), path.display());
            self.project_path = Some(path);
        }
    }

    fn save_to(&mut self, path: &Path) -> bool {
        let rs = &self.render_state;
        let bake = self.bake.has_baked.then(|| self.bake.settings.clone());
        match project::save(path, &self.mesh, &self.viewport, bake.as_ref(), &rs.device, &rs.queue) {
            Ok(()) => true,
            Err(e) => {
                self.status = format!("{}: {e:#}", t("Save failed"));
                false
            }
        }
    }

    /// Autosave every 5 minutes next to the project (or in the cache dir), spec §8.
    fn autosave(&mut self) {
        if self.last_autosave.elapsed() < std::time::Duration::from_secs(300) || !self.is_modified() || self.viewport.paint.is_stroke_active() {
            return;
        }
        self.last_autosave = std::time::Instant::now();
        let path = match &self.project_path {
            Some(p) => p.with_extension("autosave.s3dp"),
            None => {
                let dir = dirs_cache().join("stylized3dpaint");
                let _ = std::fs::create_dir_all(&dir);
                dir.join(format!("{}.autosave.s3dp", self.mesh.name))
            }
        };
        if self.save_to(&path) {
            log::info!("autosaved to {}", path.display());
        }
    }

    pub fn open_export(&mut self) {
        self.export.open = true;
    }

    pub fn set_texture_size(&mut self, size: u32) {
        if size == self.texture_size {
            return;
        }
        self.texture_size = size;
        let mesh = std::mem::take(&mut self.mesh);
        let path = self.project_path.take();
        self.set_mesh(mesh);
        self.project_path = path;
        self.status = format!("{} {size} — {}", t("Texture size set to"), t("the layer stack was reset"));
    }

    // ------------------------------------------------------------------ editing helpers

    pub fn swap_colors(&mut self) {
        let fg = self.fg_srgb();
        self.set_fg_srgb(self.bg_color);
        self.bg_color = fg;
    }

    pub fn reset_colors(&mut self) {
        // Photoshop defaults are black / white; white foreground reveals when painting masks.
        self.set_fg_srgb([0.0; 3]);
        self.bg_color = [1.0; 3];
    }

    /// Current pen pressure (1 for a mouse).
    fn pressure(&self) -> f32 {
        self.pen.map_or(1.0, |(p, _)| p)
    }

    pub fn brush_for_stroke(&self, pixels_per_point: f32) -> Brush {
        Brush {
            values: self.fg.clone(),
            channels: self.brush.channels,
            radius: self.brush.size * pixels_per_point,
            hardness: self.brush.hardness,
            opacity: self.brush.opacity,
            flow: self.brush.flow,
            erase: self.tool == Tool::Eraser || self.pen.is_some_and(|(_, eraser)| eraser),
            pressure_size: self.brush.pressure_size,
            pressure_flow: self.brush.pressure_flow,
            min_size: self.brush.min_size,
        }
    }

    /// egui texture for a layer thumbnail.
    pub fn thumb_texture(&self, node: u64, mask: bool) -> Option<egui::TextureId> {
        self.thumbs.get(&(node, mask)).map(|t| t.0)
    }

    fn sync_thumbnails(&mut self) {
        let rs = &self.render_state;
        let mut renderer = rs.renderer.write();
        let paint = &self.viewport.paint;
        let mut live = Vec::new();
        paint.doc.visit(|n| {
            for mask in [false, true] {
                let key = (n.id, mask);
                let Some(thumb) = paint.thumb(n.id, mask) else { continue };
                live.push(key);
                let view = thumb.texture.create_view(&Default::default());
                match self.thumbs.get_mut(&key) {
                    Some((id, rev)) if *rev != thumb.revision => {
                        renderer.update_egui_texture_from_wgpu_texture(&rs.device, &view, wgpu::FilterMode::Linear, *id);
                        *rev = thumb.revision;
                    }
                    Some(_) => {}
                    None => {
                        let id = renderer.register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear);
                        self.thumbs.insert(key, (id, thumb.revision));
                    }
                }
            }
        });
        self.thumbs.retain(|key, (id, _)| {
            let keep = live.contains(key);
            if !keep {
                renderer.free_texture(id);
            }
            keep
        });
    }

    // ------------------------------------------------------------------ viewport

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        self.aspect = rect.width() / rect.height().max(1.0);
        if std::mem::take(&mut self.needs_frame) {
            self.frame_mesh();
        }
        self.handle_camera_input(ui, &response);
        self.handle_tool_input(ui, &response);

        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        let rs = self.render_state.clone();
        let (view, recreated) = self.viewport.render(&rs.device, &rs.queue, size, &self.camera, self.shading);
        let mut renderer = rs.renderer.write();
        let tex = match self.viewport_tex {
            Some(id) => {
                if recreated {
                    renderer.update_egui_texture_from_wgpu_texture(&rs.device, view, wgpu::FilterMode::Linear, id);
                }
                id
            }
            None => {
                let id = renderer.register_native_texture(&rs.device, view, wgpu::FilterMode::Linear);
                self.viewport_tex = Some(id);
                id
            }
        };
        drop(renderer);
        ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        self.sync_thumbnails();

        self.viewport_overlay(ui, rect);
        if self.direction_drag.is_some() {
            let painter = ui.painter_at(rect);
            let text = format!("{} {}", egui_phosphor::regular::SUN, t("Drag over the model to aim the light · Esc to finish"));
            painter.text(rect.center_top() + egui::vec2(0.0, 20.0), egui::Align2::CENTER_TOP, text, egui::FontId::proportional(14.0), egui::Color32::from_rgb(255, 210, 120));
        }
        if let Some(pos) = response.hover_pos().filter(|_| !ui.input(|i| i.modifiers.alt)) {
            let painter = ui.painter_at(rect);
            if self.tool == Tool::Eyedropper {
                painter.text(pos + egui::vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, egui_phosphor::regular::EYEDROPPER, egui::FontId::proportional(18.0), egui::Color32::WHITE);
            } else {
                painter.circle_stroke(pos, self.brush.size, egui::Stroke::new(2.0, egui::Color32::from_black_alpha(140)));
                painter.circle_stroke(pos, self.brush.size, egui::Stroke::new(1.0, egui::Color32::WHITE));
                let inner = self.brush.size * self.brush.hardness;
                if inner > 2.0 {
                    painter.circle_stroke(pos, inner, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)));
                }
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    fn viewport_overlay(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let area = egui::Area::new(egui::Id::new("viewport_overlay")).fixed_pos(rect.right_top() + egui::vec2(-8.0, 8.0)).pivot(egui::Align2::RIGHT_TOP).order(egui::Order::Middle);
        area.show(ui.ctx(), |ui| {
            egui::Frame::new().fill(egui::Color32::from_black_alpha(150)).corner_radius(4).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(egui_phosphor::regular::CUBE).color(theme::TEXT_DIM));
                    egui::ComboBox::from_id_salt("shading").selected_text(t(self.shading.label())).width(150.0).show_ui(ui, |ui| ui_panels::shading_menu(self, ui));
                });
            });
        });
    }

    fn handle_tool_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let ppp = ui.ctx().pixels_per_point();
        let to_px = |p: egui::Pos2| ((p - response.rect.min) * ppp).to_pos2();
        let alt = ui.input(|i| i.modifiers.alt);
        let rs = self.render_state.clone();

        // Dragging a light direction over the model (spec §6.2): light comes from the surface
        // normal under the cursor.
        if let Some((node, effect, param)) = self.direction_drag.clone() {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) || self.viewport.paint.doc.find(node).is_none() {
                self.direction_drag = None;
                return;
            }
            let dragging = response.dragged_by(egui::PointerButton::Primary) || response.clicked_by(egui::PointerButton::Primary);
            if dragging
                && !alt
                && let Some(pos) = response.interact_pointer_pos()
            {
                let p = to_px(pos);
                if let Some((set, uv)) = self.viewport.pick(&rs.device, &rs.queue, [p.x as u32, p.y as u32])
                    && let Some(n) = self.viewport.paint.world_normal_at(&rs.device, &rs.queue, set, uv)
                {
                    let first = response.drag_started() || response.clicked();
                    self.viewport.paint.edit_node(node, first, |nd| {
                        let set_param = |e: &mut s3dp_core::EffectInstance| {
                            if e.id == effect {
                                e.params.insert(param.clone(), s3dp_core::ParamValue::Vec3(n));
                            }
                        };
                        if let NodeKind::Effect(e) = &mut nd.kind {
                            set_param(e);
                        }
                        nd.effects.iter_mut().for_each(set_param);
                        for item in nd.mask.iter_mut().flat_map(|m| m.items.iter_mut()) {
                            if let s3dp_core::MaskItemKind::Effect(e) = &mut item.kind {
                                set_param(e);
                            }
                        }
                    });
                }
            }
            return;
        }

        if self.tool == Tool::Eyedropper {
            if (response.clicked_by(egui::PointerButton::Primary) || response.dragged_by(egui::PointerButton::Primary))
                && !alt
                && let Some(pos) = response.interact_pointer_pos()
            {
                let p = to_px(pos);
                let picked = self.viewport.pick(&rs.device, &rs.queue, [p.x as u32, p.y as u32]).and_then(|(set, uv)| self.viewport.paint.sample(&rs.device, &rs.queue, set, uv));
                if let Some(values) = picked {
                    self.fg = values;
                }
            }
            return;
        }

        if response.drag_started_by(egui::PointerButton::Primary) && !alt {
            if let Some(pos) = response.interact_pointer_pos() {
                match self.viewport.begin_stroke(&rs.device, &rs.queue, self.brush_for_stroke(ppp)) {
                    Ok(()) => {
                        let p = to_px(pos);
                        let pressure = self.pressure();
                        self.viewport.paint.add_dabs_pressure(&[[p.x, p.y, pressure]]);
                        self.last_dab = Some(p);
                        self.last_pressure = pressure;
                    }
                    Err(msg) => self.status = t(msg).to_owned(),
                }
            }
        } else if self.last_dab.is_some()
            && response.dragged_by(egui::PointerButton::Primary)
            && let (Some(pos), Some(mut last)) = (response.interact_pointer_pos(), self.last_dab)
        {
            let target = to_px(pos);
            let (p0, p1) = (self.last_pressure, self.pressure());
            // Spacing follows the smaller (pressure-scaled) dab so light strokes stay continuous.
            let size_share = if self.brush.pressure_size { self.brush.min_size + (1.0 - self.brush.min_size) * p0.min(p1) } else { 1.0 };
            let spacing = (self.brush.size * ppp * 0.12 * size_share).max(1.0);
            let total = last.distance(target);
            let mut dabs = Vec::new();
            let mut walked = 0.0;
            while last.distance(target) >= spacing {
                last += (target - last).normalized() * spacing;
                walked += spacing;
                let k = if total > 0.0 { walked / total } else { 1.0 };
                dabs.push([last.x, last.y, p0 + (p1 - p0) * k]);
            }
            if !dabs.is_empty() {
                self.last_pressure = p0 + (p1 - p0) * (walked / total.max(1e-6)).min(1.0);
            }
            self.viewport.paint.add_dabs_pressure(&dabs);
            self.last_dab = Some(last);
        }
        if self.last_dab.is_some() && !response.dragged_by(egui::PointerButton::Primary) {
            self.viewport.paint.end_stroke();
            self.last_dab = None;
        }
    }

    fn handle_camera_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        let delta = response.drag_delta();
        if delta != egui::Vec2::ZERO {
            let primary = response.dragged_by(egui::PointerButton::Primary);
            let middle = response.dragged_by(egui::PointerButton::Middle);
            let secondary = response.dragged_by(egui::PointerButton::Secondary);
            if (alt && primary && self.last_dab.is_none()) || (middle && !alt && !shift) {
                self.camera.orbit(delta.x, delta.y);
            } else if (alt && middle) || (shift && middle) || (secondary && !alt) {
                self.camera.pan(delta.x, delta.y, response.rect.height());
            } else if alt && secondary {
                self.camera.zoom((1.0 - delta.x * 0.005).clamp(0.5, 2.0));
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom((-scroll * 0.0015).exp());
            }
        }
    }

    // ------------------------------------------------------------------ shortcuts

    fn handle_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::{Key, KeyboardShortcut as S, Modifiers as M};
        type Action = fn(&mut App);
        let cmd_shift = M::COMMAND | M::SHIFT;
        // More specific shortcuts first: consume_shortcut ignores extra modifiers.
        let shortcuts: [(S, Action); 14] = [
            (S::new(cmd_shift, Key::Z), |a| a.viewport.paint.redo()),
            (S::new(M::COMMAND, Key::Y), |a| a.viewport.paint.redo()),
            (S::new(M::COMMAND, Key::Z), |a| a.viewport.paint.undo()),
            (S::new(cmd_shift, Key::S), |a| a.save(true)),
            (S::new(M::COMMAND, Key::S), |a| a.save(false)),
            (S::new(cmd_shift, Key::E), |a| {
                // Repeat the last export with the same settings, or open the window.
                if a.export.exported_once { ui_export::start(a) } else { a.open_export() }
            }),
            (S::new(cmd_shift, Key::N), |a| {
                a.viewport.paint.add_node(NodeKind::Paint, |_, _| {});
            }),
            (S::new(M::COMMAND, Key::N), App::open_model_dialog),
            (S::new(M::COMMAND, Key::O), App::open_project_dialog),
            (S::new(M::COMMAND, Key::B), |a| a.bake.open = true),
            (S::new(M::COMMAND, Key::G), |a| {
                let ids = a.selected_layers();
                if let Some(folder) = a.viewport.paint.group(&ids) {
                    a.layers_ui.selection = vec![folder];
                }
            }),
            (S::new(M::COMMAND, Key::J), |a| {
                if let Some(id) = a.viewport.paint.active {
                    a.viewport.paint.duplicate_node(id);
                }
            }),
            (S::new(M::COMMAND, Key::Q), |a| a.request(PendingAction::Quit)),
            (S::new(M::NONE, Key::Delete), |a| {
                let ids = a.selected_layers();
                a.viewport.paint.delete_nodes(&ids);
                a.layers_ui.selection.clear();
            }),
        ];
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        for (shortcut, action) in shortcuts {
            if ui.input_mut(|i| i.consume_shortcut(&shortcut)) {
                action(self);
            }
        }
        let pressed = |k: Key| ui.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        if pressed(Key::B) {
            self.tool = Tool::Brush;
        }
        if pressed(Key::E) {
            self.tool = Tool::Eraser;
        }
        if pressed(Key::I) {
            self.tool = Tool::Eyedropper;
        }
        if pressed(Key::X) {
            self.swap_colors();
        }
        if pressed(Key::D) {
            self.reset_colors();
        }
        if pressed(Key::F) {
            self.needs_frame = true;
        }
        if pressed(Key::OpenBracket) {
            self.brush.size = (self.brush.size / 1.15).max(1.0);
        }
        if pressed(Key::CloseBracket) {
            self.brush.size = (self.brush.size * 1.15).min(500.0);
        }
        for (k, mode) in [Key::F1, Key::F2, Key::F3, Key::F4, Key::F5].into_iter().zip(ShadingMode::MAIN) {
            if pressed(k) {
                self.shading = mode;
            }
        }
    }

    fn confirm_dialog(&mut self, ui: &mut egui::Ui) {
        if self.pending.is_none() {
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("discard_changes")).show(ui.ctx(), |ui| {
            ui.set_width(340.0);
            ui.heading(t("Unsaved changes"));
            ui.label(t("The current project has unsaved changes."));
            ui.add_space(8.0);
            let mut choice = None;
            ui.horizontal(|ui| {
                if ui.button(t("Save")).clicked() {
                    choice = Some(0);
                }
                if ui.button(t("Don't save")).clicked() {
                    choice = Some(1);
                }
                if ui.button(t("Cancel")).clicked() {
                    choice = Some(2);
                }
            });
            choice
        });
        let choice = if modal.should_close() { Some(2) } else { modal.inner };
        match choice {
            Some(0) => {
                self.save(false);
                if !self.is_modified()
                    && let Some(a) = self.pending.take()
                {
                    self.run(a);
                }
            }
            Some(1) => {
                if let Some(a) = self.pending.take() {
                    self.run(a);
                }
            }
            Some(_) => self.pending = None,
            None => {}
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self.project_path.as_ref().and_then(|p| p.file_name()).map_or_else(|| self.mesh.name.clone(), |n| n.to_string_lossy().into_owned());
        let star = if self.is_modified() { "*" } else { "" };
        let title = format!("{name}{star} — Stylized3DPaint");
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

fn dirs_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).unwrap_or_else(std::env::temp_dir)
}

/// Starts the native Wayland tablet reader on eframe's display (None elsewhere).
#[cfg(target_os = "linux")]
fn start_tablet(cc: &eframe::CreationContext<'_>) -> Option<s3dp_tablet::wayland::Tablet> {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    let RawDisplayHandle::Wayland(h) = cc.display_handle().ok()?.as_raw() else { return None };
    let ctx = cc.egui_ctx.clone();
    // SAFETY: winit's display lives as long as the event loop; the reader is dropped in
    // `on_exit`, before eframe tears the loop down.
    match unsafe { s3dp_tablet::wayland::start(h.display, move || ctx.request_repaint()) } {
        Ok(t) => Some(t),
        Err(e) => {
            log::info!("{e}");
            None
        }
    }
}

impl eframe::App for App {
    /// Pen events (the compositor stops emulating the pointer for a bound tablet): injected as
    /// pointer events; pressure / eraser kept for the brush.
    #[cfg(target_os = "linux")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        use s3dp_tablet::{PenButton, PenEventKind};
        let Some(tablet) = &self.tablet else { return };
        let zoom = ctx.zoom_factor().max(1e-3);
        let modifiers = ctx.input(|i| i.modifiers);
        for e in tablet.drain() {
            let pos = egui::pos2(e.pos[0] / zoom, e.pos[1] / zoom);
            let button = |button, pressed| egui::Event::PointerButton { pos, button, pressed, modifiers };
            match e.kind {
                PenEventKind::Enter | PenEventKind::Motion => raw_input.events.push(egui::Event::PointerMoved(pos)),
                PenEventKind::Down => raw_input.events.push(button(egui::PointerButton::Primary, true)),
                PenEventKind::Up => raw_input.events.push(button(egui::PointerButton::Primary, false)),
                // Lower barrel button orbits (middle drag), upper one acts as right click.
                PenEventKind::Button { button: b, pressed } => {
                    let b = match b {
                        PenButton::First => egui::PointerButton::Middle,
                        _ => egui::PointerButton::Secondary,
                    };
                    raw_input.events.push(button(b, pressed));
                }
                PenEventKind::Leave => raw_input.events.push(egui::Event::PointerGone),
            }
            self.pen = (e.kind != PenEventKind::Leave).then_some((e.pressure, e.eraser));
        }
    }

    fn on_exit(&mut self) {
        // Stop the tablet thread while winit's Wayland display is still connected.
        #[cfg(target_os = "linux")]
        {
            self.tablet = None;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.is_modified() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(PendingAction::Quit);
        }
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let dropped = ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        if let Some(path) = dropped {
            self.open_path(path);
        }
        self.handle_shortcuts(ui);
        self.update_title(&ctx);
        self.autosave();

        egui::Panel::top("menu").frame(egui::Frame::new().fill(theme::DOCK).inner_margin(egui::Margin::symmetric(6, 3))).show(ui, |ui| ui_panels::menu_bar(self, ui));
        egui::Panel::top("options").exact_size(34.0).frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(8, 4))).show(ui, |ui| ui_panels::options_bar(self, ui));
        egui::Panel::bottom("status").frame(egui::Frame::new().fill(theme::DOCK).inner_margin(egui::Margin::symmetric(8, 3))).show(ui, |ui| ui_panels::status_bar(self, ui));
        egui::Panel::left("tools").exact_size(44.0).resizable(false).frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(4, 6))).show(ui, |ui| ui_panels::toolbar(self, ui));
        // Fixed width: dock widgets size themselves from the panel width, so a resizable
        // panel would grow in a feedback loop whenever a row overflows.
        egui::Panel::right("dock").exact_size(ui_panels::DOCK_WIDTH).resizable(false).frame(egui::Frame::new().fill(theme::DOCK).inner_margin(egui::Margin::same(0))).show(ui, |ui| ui_panels::dock(self, ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.viewport_ui(ui));
        ui_bake::bake_window(self, &ctx);
        ui_export::export_window(self, &ctx);
        if self.viewport.take_bake_finished() {
            self.bake.has_baked = true;
            self.viewport.paint.touch();
            self.status = t("Mesh maps baked — see View › Mesh map").to_owned();
        }
        if self.viewport.bake_progress().is_some() {
            ctx.request_repaint();
        } else if self.viewport.paint.wants_repaint() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.confirm_dialog(ui);
    }
}
