use std::path::{Path, PathBuf};

use anyhow::Context as _;
use s3dp_core::{Mesh, OrbitCamera, import};
use s3dp_render::{Brush, ShadingMode, Viewport, wgpu};

const TEXTURE_SIZES: [u32; 3] = [1024, 2048, 4096];

/// Brush settings as edited in the UI (size in logical points).
struct BrushUi {
    color: [u8; 3],
    size: f32,
    hardness: f32,
    opacity: f32,
    flow: f32,
    erase: bool,
}

impl Default for BrushUi {
    fn default() -> Self {
        Self { color: [217, 89, 64], size: 30.0, hardness: 0.6, opacity: 1.0, flow: 1.0, erase: false }
    }
}

impl BrushUi {
    fn to_brush(&self, pixels_per_point: f32) -> Brush {
        Brush {
            color: self.color.map(|c| c as f32 / 255.0),
            radius: self.size * pixels_per_point,
            hardness: self.hardness,
            opacity: self.opacity,
            flow: self.flow,
            erase: self.erase,
        }
    }
}

pub struct App {
    render_state: egui_wgpu::RenderState,
    viewport: Viewport,
    viewport_tex: Option<egui::TextureId>,
    camera: OrbitCamera,
    shading: ShadingMode,
    mesh: Mesh,
    mesh_path: Option<PathBuf>,
    status: String,
    /// Last viewport width / height, used for framing.
    aspect: f32,
    /// Frame the mesh once the viewport size is known.
    needs_frame: bool,
    brush: BrushUi,
    texture_size: u32,
    /// Last dab position of the active stroke, in physical viewport pixels.
    last_dab: Option<egui::Pos2>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> anyhow::Result<Self> {
        let render_state = cc.wgpu_render_state.clone().context("wgpu backend is required")?;
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let viewport = Viewport::new(&render_state.device);
        let mut app = Self {
            render_state,
            viewport,
            viewport_tex: None,
            camera: OrbitCamera::default(),
            shading: ShadingMode::default(),
            mesh: Mesh::default(),
            mesh_path: None,
            status: String::new(),
            aspect: 1.5,
            needs_frame: true,
            brush: BrushUi::default(),
            texture_size: 2048,
            last_dab: None,
        };
        app.set_mesh(Mesh::uv_sphere(64, 32), None);
        if let Some(path) = initial {
            app.open(path);
        }
        Ok(app)
    }

    fn set_mesh(&mut self, mesh: Mesh, path: Option<PathBuf>) {
        let rs = &self.render_state;
        self.viewport.set_mesh(&rs.device, &rs.queue, &mesh, self.texture_size);
        self.status = format!(
            "{}: {} tris, {} verts, {} texture set(s){}",
            mesh.name,
            mesh.triangle_count(),
            mesh.vertices.len(),
            mesh.parts.len(),
            if mesh.has_uvs() { "" } else { " — no UVs!" }
        );
        self.mesh = mesh;
        self.mesh_path = path;
        self.needs_frame = true;
        self.last_dab = None;
    }

    fn frame_mesh(&mut self) {
        let b = self.mesh.bounds();
        self.camera.frame(b.center(), b.radius(), self.aspect);
    }

    fn open(&mut self, path: PathBuf) {
        match import::load(&path) {
            Ok(mesh) => self.set_mesh(mesh, Some(path)),
            Err(e) => {
                log::error!("{e:#}");
                self.status = format!("Error: {e:#}");
            }
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open model")
            .add_filter("3D models", import::SUPPORTED_EXTENSIONS)
            .pick_file()
        {
            self.open(path);
        }
    }

    fn export_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Export textures to folder");
        if let Some(dir) = self.mesh_path.as_ref().and_then(|p| p.parent()) {
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
        for (i, set) in self.viewport.paint.sets.iter().enumerate() {
            let (size, data) = self.viewport.paint.read_display(&rs.device, &rs.queue, i).context("GPU readback failed")?;
            let file = dir.join(format!("{}_{}_BaseColor.png", self.mesh.name, sanitize(&set.name)));
            image::save_buffer(&file, &data, size, size, image::ColorType::Rgba8)
                .with_context(|| format!("writing {}", file.display()))?;
        }
        Ok(self.viewport.paint.sets.len())
    }

    fn undo(&mut self) {
        let rs = &self.render_state;
        self.viewport.paint.undo(&rs.device, &rs.queue);
    }

    fn redo(&mut self) {
        let rs = &self.render_state;
        self.viewport.paint.redo(&rs.device, &rs.queue);
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open model…").clicked() {
                    ui.close();
                    self.open_dialog();
                }
                if ui.button("Export textures…").clicked() {
                    ui.close();
                    self.export_dialog();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(self.viewport.paint.can_undo(), egui::Button::new("Undo (Ctrl+Z)")).clicked() {
                    self.undo();
                }
                if ui.add_enabled(self.viewport.paint.can_redo(), egui::Button::new("Redo (Ctrl+Shift+Z)")).clicked() {
                    self.redo();
                }
            });
            ui.menu_button("View", |ui| {
                for mode in ShadingMode::ALL {
                    ui.radio_value(&mut self.shading, mode, mode.label());
                }
                ui.separator();
                if ui.button("Frame model (F)").clicked() {
                    self.frame_mesh();
                    ui.close();
                }
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Brush");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.brush.erase, false, "Paint (B)");
            ui.selectable_value(&mut self.brush.erase, true, "Erase (E)");
        });
        ui.horizontal(|ui| {
            ui.label("Color");
            ui.color_edit_button_srgb(&mut self.brush.color);
        });
        ui.add(egui::Slider::new(&mut self.brush.size, 1.0..=400.0).logarithmic(true).text("Size  [ ]"));
        ui.add(egui::Slider::new(&mut self.brush.hardness, 0.0..=1.0).text("Hardness"));
        ui.add(egui::Slider::new(&mut self.brush.opacity, 0.0..=1.0).text("Opacity"));
        ui.add(egui::Slider::new(&mut self.brush.flow, 0.01..=1.0).logarithmic(true).text("Flow"));

        ui.separator();
        ui.heading("Scene");
        ui.label(format!("Mesh: {}", self.mesh.name));
        if let Some(p) = &self.mesh_path {
            ui.small(p.display().to_string());
        }
        ui.label(format!("Triangles: {}", self.mesh.triangle_count()));
        if !self.mesh.has_uvs() {
            ui.colored_label(egui::Color32::from_rgb(230, 120, 80), "Mesh has no UVs — painting needs them");
        }
        let mut size = self.texture_size;
        egui::ComboBox::from_label("Texture size")
            .selected_text(size.to_string())
            .show_ui(ui, |ui| {
                for s in TEXTURE_SIZES {
                    ui.selectable_value(&mut size, s, s.to_string());
                }
            })
            .response
            .on_hover_text("Changing the size clears the painting");
        if size != self.texture_size {
            self.texture_size = size;
            let mesh = std::mem::take(&mut self.mesh);
            let path = self.mesh_path.take();
            self.set_mesh(mesh, path);
        }
        ui.strong("Texture sets");
        for set in &self.viewport.paint.sets {
            ui.label(format!("• {}", set.name));
        }

        ui.separator();
        ui.strong("Shading");
        for mode in ShadingMode::ALL {
            ui.radio_value(&mut self.shading, mode, mode.label());
        }
        ui.separator();
        ui.strong("Controls");
        ui.small(
            "LMB — paint\nAlt+LMB / MMB — orbit\nAlt+MMB / Shift+MMB / RMB — pan\n\
             Wheel / Alt+RMB — zoom\nF — frame model\nCtrl+Z / Ctrl+Shift+Z — undo / redo",
        );
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        self.aspect = rect.width() / rect.height().max(1.0);
        if std::mem::take(&mut self.needs_frame) {
            self.frame_mesh();
        }
        self.handle_camera_input(ui, &response);
        self.handle_paint_input(ui, &response);

        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        let rs = &self.render_state;
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

        // Brush cursor.
        if let Some(pos) = response.hover_pos().filter(|_| !ui.input(|i| i.modifiers.alt)) {
            let painter = ui.painter_at(rect);
            painter.circle_stroke(pos, self.brush.size, egui::Stroke::new(2.0, egui::Color32::from_black_alpha(140)));
            painter.circle_stroke(pos, self.brush.size, egui::Stroke::new(1.0, egui::Color32::WHITE));
            let inner = self.brush.size * self.brush.hardness;
            if inner > 2.0 {
                painter.circle_stroke(pos, inner, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)));
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    fn handle_paint_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let ppp = ui.ctx().pixels_per_point();
        let to_px = |p: egui::Pos2| ((p - response.rect.min) * ppp).to_pos2();
        let alt = ui.input(|i| i.modifiers.alt);
        let rs = &self.render_state;

        if response.drag_started_by(egui::PointerButton::Primary) && !alt {
            if let Some(pos) = response.interact_pointer_pos() {
                self.viewport.begin_stroke(&rs.device, &rs.queue, self.brush.to_brush(ppp));
                let p = to_px(pos);
                self.viewport.paint.add_dabs(&[[p.x, p.y]]);
                self.last_dab = Some(p);
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

    fn handle_shortcuts(&mut self, ui: &egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let undo = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);
        let redo = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
        let redo_y = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);
        // Check the more specific shortcut first: consume_shortcut ignores extra modifiers.
        if ui.input_mut(|i| i.consume_shortcut(&redo) || i.consume_shortcut(&redo_y)) {
            self.redo();
        } else if ui.input_mut(|i| i.consume_shortcut(&undo)) {
            self.undo();
        }
        ui.input(|i| {
            if i.key_pressed(egui::Key::F) {
                self.needs_frame = true;
            }
            if i.key_pressed(egui::Key::E) {
                self.brush.erase = true;
            }
            if i.key_pressed(egui::Key::B) {
                self.brush.erase = false;
            }
            if i.key_pressed(egui::Key::OpenBracket) {
                self.brush.size = (self.brush.size / 1.15).max(1.0);
            }
            if i.key_pressed(egui::Key::CloseBracket) {
                self.brush.size = (self.brush.size * 1.15).min(400.0);
            }
        });
    }
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let dropped =
            ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        if let Some(path) = dropped {
            self.open(path);
        }
        self.handle_shortcuts(ui);
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| ui.label(&self.status));
        egui::Panel::left("side").default_size(270.0).resizable(true).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.viewport_ui(ui));
    }
}
