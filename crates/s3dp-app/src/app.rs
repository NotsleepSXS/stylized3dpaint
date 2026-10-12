use std::path::PathBuf;

use anyhow::Context as _;
use s3dp_core::{Mesh, OrbitCamera, import};
use s3dp_render::{ShadingMode, Viewport, wgpu};

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
        };
        app.set_mesh(Mesh::uv_sphere(64, 32), None);
        if let Some(path) = initial {
            app.open(path);
        }
        Ok(app)
    }

    fn set_mesh(&mut self, mesh: Mesh, path: Option<PathBuf>) {
        self.viewport.set_mesh(&self.render_state.device, &mesh);
        self.status = format!(
            "{}: {} tris, {} verts, {} part(s){}",
            mesh.name,
            mesh.triangle_count(),
            mesh.vertices.len(),
            mesh.parts.len(),
            if mesh.has_uvs() { "" } else { " — no UVs!" }
        );
        self.mesh = mesh;
        self.mesh_path = path;
        self.needs_frame = true;
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

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open model…").clicked() {
                    ui.close();
                    self.open_dialog();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
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
        ui.heading("Scene");
        ui.label(format!("Mesh: {}", self.mesh.name));
        if let Some(p) = &self.mesh_path {
            ui.small(p.display().to_string());
        }
        ui.label(format!("Triangles: {}", self.mesh.triangle_count()));
        ui.label(format!("Vertices: {}", self.mesh.vertices.len()));
        if !self.mesh.has_uvs() {
            ui.colored_label(egui::Color32::from_rgb(230, 120, 80), "Mesh has no UVs — painting needs them");
        }
        ui.separator();
        ui.strong("Texture sets");
        for part in &self.mesh.parts {
            ui.label(format!("• {} ({} tris)", part.name, part.index_count / 3));
        }
        ui.separator();
        ui.strong("Shading");
        for mode in ShadingMode::ALL {
            ui.radio_value(&mut self.shading, mode, mode.label());
        }
        ui.separator();
        ui.strong("Camera");
        ui.small("Alt+LMB / MMB — orbit\nAlt+MMB / Shift+MMB / RMB — pan\nWheel / Alt+RMB — zoom\nF — frame model");
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        self.aspect = rect.width() / rect.height().max(1.0);
        if std::mem::take(&mut self.needs_frame) {
            self.frame_mesh();
        }
        self.handle_camera_input(ui, &response);

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
    }

    fn handle_camera_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        let delta = response.drag_delta();
        if delta != egui::Vec2::ZERO {
            let primary = response.dragged_by(egui::PointerButton::Primary);
            let middle = response.dragged_by(egui::PointerButton::Middle);
            let secondary = response.dragged_by(egui::PointerButton::Secondary);
            if (alt && primary) || (middle && !alt && !shift) {
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
        if ui.input(|i| i.key_pressed(egui::Key::F)) && !ui.ctx().egui_wants_keyboard_input() {
            self.frame_mesh();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let dropped = ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        if let Some(path) = dropped {
            self.open(path);
        }
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| ui.label(&self.status));
        egui::Panel::left("side").default_size(260.0).resizable(true).show(ui, |ui| self.side_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.viewport_ui(ui));
    }
}
