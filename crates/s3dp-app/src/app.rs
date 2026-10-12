use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use s3dp_core::{Channel, ChannelMask, ChannelValues, Mesh, OrbitCamera, import};
use s3dp_render::{Brush, ShadingMode, Viewport, wgpu};

use crate::{project, theme, ui_bake, ui_layers, ui_panels};

pub const TEXTURE_SIZES: [u32; 3] = [1024, 2048, 4096];

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

    pub fn label(self) -> &'static str {
        match self {
            Tool::Brush => "Brush (B)",
            Tool::Eraser => "Eraser (E)",
            Tool::Eyedropper => "Eyedropper (I) — picks the whole material",
        }
    }
}

/// Brush shape settings shared by brush and eraser (size in logical points).
pub struct BrushSettings {
    pub size: f32,
    pub hardness: f32,
    pub opacity: f32,
    pub flow: f32,
    pub channels: ChannelMask,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 30.0, hardness: 0.6, opacity: 1.0, flow: 1.0, channels: ChannelMask::BASE_COLOR }
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
    project_path: Option<PathBuf>,
    saved_revision: u64,
    pub status: String,
    /// Last viewport width / height, used for framing.
    aspect: f32,
    /// Frame the mesh once the viewport size is known.
    needs_frame: bool,

    pub tool: Tool,
    pub brush: BrushSettings,
    /// Foreground material: what the brush paints (base color = foreground color).
    pub fg: ChannelValues,
    /// Background color (Photoshop's second swatch, swapped with X).
    pub bg_color: [f32; 3],
    pub texture_size: u32,
    /// Last dab position of the active stroke, in physical viewport pixels.
    last_dab: Option<egui::Pos2>,

    /// Registered layer thumbnails: (layer id, is mask) -> (egui texture, revision).
    thumbs: HashMap<(u64, bool), (egui::TextureId, u64)>,
    pub layers_ui: ui_layers::LayersUi,
    pending: Option<PendingAction>,
    allow_close: bool,
    pub picker: crate::color_picker::PickerState,
    pub bake: ui_bake::BakeUi,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> anyhow::Result<Self> {
        let render_state = cc.wgpu_render_state.clone().context("wgpu backend is required")?;
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        theme::apply(&cc.egui_ctx);
        let viewport = Viewport::new(&render_state.device);
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
            fg: ChannelValues { base_color: [0.85, 0.35, 0.25], ..ChannelValues::DEFAULTS },
            bg_color: [1.0; 3],
            texture_size: 2048,
            last_dab: None,
            thumbs: HashMap::new(),
            layers_ui: Default::default(),
            pending: None,
            allow_close: false,
            picker: Default::default(),
            bake: Default::default(),
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

    fn set_mesh(&mut self, mesh: Mesh) {
        let rs = &self.render_state;
        self.viewport.set_mesh(&rs.device, &rs.queue, &mesh, self.texture_size);
        self.status = format!(
            "{}: {} tris, {} texture set(s){}",
            mesh.name,
            mesh.triangle_count(),
            mesh.parts.len(),
            if mesh.has_uvs() { "" } else { " — no UVs!" }
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

    /// Opens a model or a project, asking first if there are unsaved changes.
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
            PendingAction::Quit => {
                self.allow_close = true;
            }
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
                self.status = format!("Error: {e:#}");
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
                self.status = format!("Opened {}", path.display());
            }
            Err(e) => {
                log::error!("{e:#}");
                self.status = format!("Error: {e:#}");
                // The viewport may be half-loaded; fall back to a clean document.
                let mesh = std::mem::take(&mut self.mesh);
                self.set_mesh(mesh);
            }
        }
    }

    pub fn open_model_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("New project from model")
            .add_filter("3D models", import::SUPPORTED_EXTENSIONS)
            .pick_file()
        {
            self.request(PendingAction::OpenModel(path));
        }
    }

    pub fn open_project_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open project")
            .add_filter("Stylized3DPaint project", &["s3dp"])
            .pick_file()
        {
            self.request(PendingAction::OpenProject(path));
        }
    }

    pub fn save(&mut self, save_as: bool) {
        let path = match (&self.project_path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(mut p) = rfd::FileDialog::new()
                    .set_title("Save project")
                    .add_filter("Stylized3DPaint project", &["s3dp"])
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
        let rs = &self.render_state;
        let bake = self.bake.has_baked.then(|| self.bake.settings.clone());
        match project::save(&path, &self.mesh, &self.viewport, bake.as_ref(), &rs.device, &rs.queue) {
            Ok(()) => {
                self.saved_revision = self.viewport.paint.revision();
                self.status = format!("Saved {}", path.display());
                self.project_path = Some(path);
            }
            Err(e) => self.status = format!("Save failed: {e:#}"),
        }
    }

    pub fn export_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Export textures to folder");
        if let Some(dir) = self.project_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.status = match self.export_textures(&dir) {
                Ok(n) => format!("Exported {n} texture(s) to {}", dir.display()),
                Err(e) => format!("Export failed: {e:#}"),
            };
        }
    }

    fn export_textures(&self, dir: &Path) -> anyhow::Result<usize> {
        let rs = &self.render_state;
        let mut count = 0;
        for (i, set) in self.viewport.paint.sets.iter().enumerate() {
            for c in Channel::ALL {
                let (size, data) =
                    self.viewport.paint.read_channel(&rs.device, &rs.queue, i, c).context("GPU readback failed")?;
                let name = c.label().replace(' ', "");
                let file = dir.join(format!("{}_{}_{name}.png", sanitize(&self.mesh.name), sanitize(&set.name)));
                let color = if c.is_color() { image::ColorType::Rgb8 } else { image::ColorType::L8 };
                image::save_buffer(&file, &data, size, size, color).with_context(|| format!("writing {}", file.display()))?;
                count += 1;
            }
        }
        Ok(count)
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
        self.status = format!("Texture size set to {size} — the layer stack was reset");
    }

    // ------------------------------------------------------------------ editing helpers

    pub fn undo(&mut self) {
        self.viewport.paint.undo();
    }

    pub fn redo(&mut self) {
        self.viewport.paint.redo();
    }

    pub fn swap_colors(&mut self) {
        std::mem::swap(&mut self.fg.base_color, &mut self.bg_color);
    }

    pub fn reset_colors(&mut self) {
        // Photoshop defaults are black/white; white foreground reveals when painting masks.
        self.fg.base_color = [0.0; 3];
        self.bg_color = [1.0; 3];
    }

    pub fn brush_for_stroke(&self, pixels_per_point: f32) -> Brush {
        Brush {
            values: self.fg,
            channels: self.brush.channels,
            radius: self.brush.size * pixels_per_point,
            hardness: self.brush.hardness,
            opacity: self.brush.opacity,
            flow: self.brush.flow,
            erase: self.tool == Tool::Eraser,
        }
    }

    pub fn add_mask(&mut self, index: usize, white: bool) {
        let rs = &self.render_state;
        self.viewport.paint.add_mask(&rs.device, &rs.queue, index, white);
    }

    /// egui texture for a layer thumbnail, if the painter has one.
    pub fn thumb_texture(&self, layer_id: u64, mask: bool) -> Option<egui::TextureId> {
        self.thumbs.get(&(layer_id, mask)).map(|t| t.0)
    }

    fn sync_thumbnails(&mut self) {
        let rs = &self.render_state;
        let mut renderer = rs.renderer.write();
        let paint = &self.viewport.paint;
        let mut live = Vec::new();
        for layer in &paint.layers {
            for mask in [false, true] {
                let key = (layer.meta.id, mask);
                let Some(thumb) = paint.thumb(layer.meta.id, mask) else { continue };
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
        }
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
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(tex, rect, uv, egui::Color32::WHITE);
        self.sync_thumbnails();

        self.viewport_overlay(ui, rect);
        if let Some(pos) = response.hover_pos().filter(|_| !ui.input(|i| i.modifiers.alt)) {
            let painter = ui.painter_at(rect);
            if self.tool == Tool::Eyedropper {
                painter.text(
                    pos + egui::vec2(10.0, -10.0),
                    egui::Align2::LEFT_BOTTOM,
                    egui_phosphor::regular::EYEDROPPER,
                    egui::FontId::proportional(18.0),
                    egui::Color32::WHITE,
                );
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

    /// Shading mode selector floating in the viewport corner.
    fn viewport_overlay(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let area = egui::Area::new(egui::Id::new("viewport_overlay"))
            .fixed_pos(rect.right_top() + egui::vec2(-8.0, 8.0))
            .pivot(egui::Align2::RIGHT_TOP)
            .order(egui::Order::Middle);
        area.show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(150))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(egui_phosphor::regular::CUBE).color(theme::TEXT_DIM));
                        egui::ComboBox::from_id_salt("shading")
                            .selected_text(self.shading.label())
                            .width(130.0)
                            .show_ui(ui, |ui| {
                                for (i, mode) in ShadingMode::ALL.into_iter().enumerate() {
                                    ui.selectable_value(&mut self.shading, mode, format!("{}   F{}", mode.label(), i + 1));
                                }
                                ui.separator();
                                for map in s3dp_core::BakedMap::ALL {
                                    ui.selectable_value(&mut self.shading, ShadingMode::Baked(map), map.label());
                                }
                            });
                    });
                });
        });
    }

    fn handle_tool_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let ppp = ui.ctx().pixels_per_point();
        let to_px = |p: egui::Pos2| ((p - response.rect.min) * ppp).to_pos2();
        let alt = ui.input(|i| i.modifiers.alt);
        let rs = self.render_state.clone();

        if self.tool == Tool::Eyedropper {
            if (response.clicked_by(egui::PointerButton::Primary) || response.dragged_by(egui::PointerButton::Primary))
                && !alt
                && let Some(pos) = response.interact_pointer_pos()
            {
                let p = to_px(pos);
                let picked = self
                    .viewport
                    .pick(&rs.device, &rs.queue, [p.x as u32, p.y as u32])
                    .and_then(|(set, uv)| self.viewport.paint.sample(&rs.device, &rs.queue, set, uv));
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
                        self.viewport.paint.add_dabs(&[[p.x, p.y]]);
                        self.last_dab = Some(p);
                    }
                    Err(msg) => self.status = msg.to_owned(),
                }
            }
        } else if self.last_dab.is_some() && response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(pos), Some(mut last)) = (response.interact_pointer_pos(), self.last_dab) {
                let target = to_px(pos);
                let spacing = (self.brush.size * ppp * 0.12).max(1.0);
                let mut dabs = Vec::new();
                while last.distance(target) >= spacing {
                    last += (target - last).normalized() * spacing;
                    dabs.push([last.x, last.y]);
                }
                self.viewport.paint.add_dabs(&dabs);
                self.last_dab = Some(last);
            }
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
        let cmd_shift = M::COMMAND | M::SHIFT;
        // More specific shortcuts first: consume_shortcut ignores extra modifiers.
        let shortcuts: [(S, fn(&mut App)); 13] = [
            (S::new(M::COMMAND, Key::B), |a| a.bake.open = true),
            (S::new(cmd_shift, Key::Z), App::redo),
            (S::new(M::COMMAND, Key::Y), App::redo),
            (S::new(M::COMMAND, Key::Z), App::undo),
            (S::new(cmd_shift, Key::S), |a| a.save(true)),
            (S::new(M::COMMAND, Key::S), |a| a.save(false)),
            (S::new(cmd_shift, Key::E), App::export_dialog),
            (S::new(cmd_shift, Key::N), |a| a.viewport.paint.add_layer(s3dp_core::LayerKind::Paint)),
            (S::new(M::COMMAND, Key::N), App::open_model_dialog),
            (S::new(M::COMMAND, Key::O), App::open_project_dialog),
            (S::new(M::COMMAND, Key::J), |a| {
                let i = a.viewport.paint.active;
                a.viewport.paint.duplicate_layer(i);
            }),
            (S::new(M::COMMAND, Key::Q), |a| a.request(PendingAction::Quit)),
            (S::new(M::NONE, Key::Delete), |a| {
                let i = a.viewport.paint.active;
                a.viewport.paint.delete_layer(i);
            }),
        ];
        for (shortcut, action) in shortcuts {
            if ui.input_mut(|i| i.consume_shortcut(&shortcut)) {
                action(self);
            }
        }
        if ui.ctx().egui_wants_keyboard_input() {
            return;
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
        let fkeys = [Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6, Key::F7, Key::F8, Key::F9];
        for (k, mode) in fkeys.into_iter().zip(ShadingMode::ALL) {
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
            ui.set_width(320.0);
            ui.heading("Unsaved changes");
            ui.label("The current project has unsaved changes.");
            ui.add_space(8.0);
            let mut choice = None;
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(0);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
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

    fn update_title(&self, ctx: &egui::Context) {
        let name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| self.mesh.name.clone(), |n| n.to_string_lossy().into_owned());
        let star = if self.is_modified() { "*" } else { "" };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name}{star} — Stylized3DPaint")));
    }
}

pub fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.is_modified() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(PendingAction::Quit);
        }
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let dropped =
            ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        if let Some(path) = dropped {
            self.open_path(path);
        }
        self.handle_shortcuts(ui);
        self.update_title(&ctx);

        egui::Panel::top("menu").frame(egui::Frame::new().fill(theme::DOCK).inner_margin(egui::Margin::symmetric(6, 3))).show(ui, |ui| ui_panels::menu_bar(self, ui));
        egui::Panel::top("options")
            .exact_size(34.0)
            .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| ui_panels::options_bar(self, ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(theme::DOCK).inner_margin(egui::Margin::symmetric(8, 3)))
            .show(ui, |ui| ui_panels::status_bar(self, ui));
        egui::Panel::left("tools")
            .exact_size(44.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(4, 6)))
            .show(ui, |ui| ui_panels::toolbar(self, ui));
        egui::Panel::right("dock")
            .default_size(310.0)
            .size_range(260.0..=520.0)
            .resizable(true)
            .frame(egui::Frame::new().fill(theme::DOCK).inner_margin(egui::Margin::same(0)))
            .show(ui, |ui| ui_panels::dock(self, ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.viewport_ui(ui));
        ui_bake::bake_window(self, &ctx);
        if self.viewport.take_bake_finished() {
            self.bake.has_baked = true;
            self.viewport.paint.touch();
            self.status = "Mesh maps baked — see View › Mesh maps".into();
        }
        if self.viewport.bake_progress().is_some() {
            ctx.request_repaint();
        }
        self.confirm_dialog(ui);
    }
}
