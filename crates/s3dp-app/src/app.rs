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
    /// Fill whole polygons / elements / UV islands (masks: reveal or hide them).
    PolygonFill,
}

impl Tool {
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Tool::Brush => i::PAINT_BRUSH,
            Tool::Eraser => i::ERASER,
            Tool::Eyedropper => i::EYEDROPPER,
            Tool::PolygonFill => i::POLYGON,
        }
    }

    pub fn label(self) -> String {
        match self {
            Tool::Brush => format!("{} (B)", t("Brush")),
            Tool::Eraser => format!("{} (E)", t("Eraser")),
            Tool::Eyedropper => format!("{} (I) — {}", t("Eyedropper"), t("picks the whole material")),
            Tool::PolygonFill => format!("{} (P) — {}", t("Polygon Fill"), t("faces, parts or UV islands")),
        }
    }
}

/// Tabs of the lower dock panel.
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
    /// Sample-shape scene of the effect preview (created on first use).
    pub sample: Option<crate::ui_preview::SampleScene>,
    /// Lighting environment (HDRI) state.
    pub env: crate::ui_env::EnvUi,
    /// Polygon Fill: what a click selects, and add (reveal / paint) vs remove (hide / erase).
    pub fill_mode: s3dp_core::select::SelectMode,
    pub fill_add: bool,
    topology: Option<s3dp_core::select::Topology>,
    /// Groups already filled in the current fill stroke, the frame it started, hovered group.
    fill_done: std::collections::HashSet<u32>,
    fill_stroke: Option<u64>,
    fill_hover: Option<(s3dp_core::select::SelectMode, u32)>,
    /// Mode the overlay geometry was built for (rebuilt on mode / mesh change).
    fill_overlay: Option<s3dp_core::select::SelectMode>,
    frame: u64,
    /// Last texture-space dab of a brush stroke on the UV view (texels).
    last_dab_uv: Option<egui::Pos2>,
    /// Viewport mode: 3D only, or 3D next to the flat UV texture.
    pub split_uv: bool,
    uv_tex: Option<egui::TextureId>,
    uv_key: Option<(u64, ShadingMode, [u32; 2], usize)>,
    /// Texture set shown in the UV panel.
    pub uv_set: usize,
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
    /// Dockable panel layout.
    pub dock: egui_dock::DockState<crate::ui_dock::Tab>,
    /// Panel to bring to front on the next frame.
    pub focus_tab: Option<crate::ui_dock::Tab>,
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
            sample: None,
            env: crate::ui_env::EnvUi::new(),
            fill_mode: Default::default(),
            fill_add: true,
            topology: None,
            fill_done: Default::default(),
            fill_stroke: None,
            fill_hover: None,
            fill_overlay: None,
            frame: 0,
            last_dab_uv: None,
            split_uv: false,
            uv_tex: None,
            uv_key: None,
            uv_set: 0,
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
            dock: crate::ui_dock::load_layout(),
            focus_tab: None,
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
        self.topology = None;
        self.fill_hover = None;
        self.fill_overlay = None;
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
                self.topology = None;
                self.fill_hover = None;
                self.fill_overlay = None;
                self.needs_frame = true;
                self.thumbs.clear();
                self.layers_ui.reset_for(&self.viewport.paint.doc);
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
                let dir = crate::paths::cache_dir().unwrap_or_else(|| std::env::temp_dir().join("stylized3dpaint"));
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

    fn topology(&mut self) -> &s3dp_core::select::Topology {
        if self.topology.is_none() {
            self.topology = Some(s3dp_core::select::Topology::build(&self.mesh));
        }
        self.topology.as_ref().unwrap()
    }

    /// Wireframe / tint for the current selection mode: face borders (quad diagonals hidden),
    /// island borders + island colours, or part borders + part colours.
    fn build_fill_overlay(&mut self) {
        use s3dp_core::select::{SelectMode, boundary_edges, group_color};
        let mode = self.fill_mode;
        self.topology();
        let topo = self.topology.as_ref().unwrap();
        let wire3d = boundary_edges(&self.mesh, topo, mode, false);
        let wire_uv = boundary_edges(&self.mesh, topo, mode, true);
        let tint: Vec<s3dp_render::TintVertex> = if mode == SelectMode::Face {
            Vec::new()
        } else {
            let groups = if mode == SelectMode::Island { &topo.island } else { &topo.element };
            self.mesh
                .indices
                .chunks_exact(3)
                .enumerate()
                .flat_map(|(t, tri)| {
                    let color = group_color(groups[t]);
                    tri.iter().map(move |&v| (v, color)).collect::<Vec<_>>()
                })
                .map(|(v, color)| {
                    let vx = &self.mesh.vertices[v as usize];
                    s3dp_render::TintVertex { position: vx.position, uv: vx.uv, color }
                })
                .collect()
        };
        let rs = self.render_state.clone();
        self.viewport.set_overlay(&rs.device, &wire3d, &wire_uv, &tint);
        self.fill_overlay = Some(mode);
        self.fill_hover = None;
    }

    /// Triangle under a viewport position (camera ray).
    fn pick_triangle_3d(&self, pos: egui::Pos2, rect: egui::Rect) -> Option<u32> {
        let ndc = egui::vec2((pos.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (pos.y - rect.top()) / rect.height() * 2.0);
        let (o, d) = camera_ray(&self.camera, rect.width() / rect.height().max(1.0), [ndc.x, ndc.y]);
        s3dp_core::select::pick_ray(&self.mesh, o, d).map(|h| h.0)
    }

    /// Brush of a Polygon Fill stroke: masks get white (add, reveal) or black (remove, hide);
    /// layers get the foreground material (add) or are erased (remove).
    fn fill_brush(&self, add: bool) -> Brush {
        let mut b = self.brush_for_stroke(1.0);
        b.hardness = 1.0;
        b.opacity = 1.0;
        b.flow = 1.0;
        b.pressure_size = false;
        b.pressure_flow = false;
        if matches!(self.viewport.paint.target, s3dp_render::EditTarget::MaskItem(_)) {
            b.values.set(ChannelId::BaseColor, if add { [1.0; 3] } else { [0.0; 3] });
            b.erase = false;
        } else {
            b.erase = !add;
        }
        b
    }

    /// Hover highlight and click / drag filling for a picked triangle (3D view or UV view).
    pub(crate) fn polygon_fill_input(&mut self, ui: &egui::Ui, response: &egui::Response, hit: Option<u32>) {
        let mode = self.fill_mode;
        let rs = self.render_state.clone();
        // Highlight what a click would fill.
        let group = hit.and_then(|t| self.topology().group_of(mode, t));
        let hover = group.map(|g| (mode, g));
        if hover != self.fill_hover {
            self.fill_hover = hover;
            let idx = match hit {
                Some(t) => {
                    let tris = self.topology().triangles_with(mode, t);
                    s3dp_core::select::triangle_indices(&self.mesh, &tris)
                }
                None => Vec::new(),
            };
            self.viewport.set_highlight(&rs.device, &idx);
        }
        let alt = ui.input(|i| i.modifiers.alt);
        let invert = ui.input(|i| i.modifiers.command);
        let pressing = (response.dragged_by(egui::PointerButton::Primary) || response.clicked_by(egui::PointerButton::Primary)) && !alt;
        if pressing {
            if self.fill_stroke.is_none() {
                let add = self.fill_add != invert;
                match self.viewport.begin_stroke(&rs.device, &rs.queue, self.fill_brush(add)) {
                    Ok(()) => {
                        self.fill_stroke = Some(self.frame);
                        self.fill_done.clear();
                    }
                    Err(msg) => {
                        self.status = t(msg).to_owned();
                        return;
                    }
                }
            }
            if let (Some(t), Some(g)) = (hit, group)
                && self.fill_done.insert(g)
            {
                let tris = self.topology().triangles_with(mode, t);
                let set = self.topology().set.get(t as usize).copied().unwrap_or(0) as usize;
                let idx = s3dp_core::select::triangle_indices(&self.mesh, &tris);
                self.viewport.paint.fill_triangles(set, idx);
            }
        } else if let Some(started) = self.fill_stroke
            && started < self.frame
            && !response.dragged()
        {
            // End a frame later, after the queued fills were applied.
            self.viewport.paint.end_stroke();
            self.fill_stroke = None;
        }
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

    pub(crate) fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        // "3D + UV": the flat texture as a square on the right, the 3D view in the rest.
        let mut rect = full;
        if self.split_uv && full.width() > 200.0 {
            let side = full.height().min(full.width() * 0.5).floor();
            rect.max.x = full.max.x - side - 3.0;
            let uv_rect = egui::Rect::from_min_size(egui::pos2(full.max.x - side, full.center().y - side / 2.0), egui::vec2(side, side));
            ui.painter().rect_filled(full, 0.0, egui::Color32::from_rgb(9, 9, 11));
            self.uv_panel(ui, uv_rect);
        }
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
            } else if self.tool == Tool::PolygonFill {
                let sign = if self.fill_add != ui.input(|i| i.modifiers.command) { "+" } else { "−" };
                painter.text(pos + egui::vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, format!("{} {sign}", egui_phosphor::regular::POLYGON), egui::FontId::proportional(16.0), egui::Color32::WHITE);
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

    /// Flat UV texture of a texture set, shaded like the viewport (material modes: base color).
    fn uv_panel(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        self.uv_input(ui, &resp);
        let sets = self.viewport.paint.sets.len();
        self.uv_set = self.uv_set.min(sets.saturating_sub(1));
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        let key = (self.viewport.frame_id(), self.shading, size, self.uv_set);
        if self.uv_key != Some(key) || self.uv_tex.is_none() {
            let rs = self.render_state.clone();
            let what = s3dp_render::Preview { mask_of: None, uv: true, set: self.uv_set, shading: Some(self.shading), overlay: true };
            if let Some((view, recreated)) = self.viewport.render_preview(&rs.device, &rs.queue, 1, size, &self.camera, what) {
                let mut renderer = rs.renderer.write();
                match self.uv_tex {
                    Some(id) if recreated => renderer.update_egui_texture_from_wgpu_texture(&rs.device, view, wgpu::FilterMode::Linear, id),
                    Some(_) => {}
                    None => self.uv_tex = Some(renderer.register_native_texture(&rs.device, view, wgpu::FilterMode::Linear)),
                }
                self.uv_key = Some(key);
            }
        }
        if let Some(tex) = self.uv_tex {
            ui.painter().image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, theme::FIELD_BORDER), egui::StrokeKind::Outside);
        // Texture set switch when there are several.
        if sets > 1 {
            let area = egui::Area::new(egui::Id::new("uv_overlay")).fixed_pos(rect.left_top() + egui::vec2(8.0, 8.0)).order(egui::Order::Middle);
            area.show(ui.ctx(), |ui| {
                egui::Frame::new().fill(egui::Color32::from_black_alpha(150)).corner_radius(4).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                    let name = self.viewport.paint.sets.get(self.uv_set).map_or(String::new(), |s| s.name.clone());
                    egui::ComboBox::from_id_salt("uv_set").selected_text(format!("UV · {name}")).show_ui(ui, |ui| {
                        for i in 0..sets {
                            let n = self.viewport.paint.sets[i].name.clone();
                            ui.selectable_value(&mut self.uv_set, i, n);
                        }
                    });
                });
            });
        }
    }

    /// Polygon Fill and brush / eraser strokes on the flat UV view.
    fn uv_input(&mut self, ui: &egui::Ui, resp: &egui::Response) {
        let rect = resp.rect;
        let set = self.uv_set.min(self.viewport.paint.sets.len().saturating_sub(1));
        let to_uv = |p: egui::Pos2| glam::Vec2::new((p.x - rect.left()) / rect.width(), (p.y - rect.top()) / rect.height());
        let alt = ui.input(|i| i.modifiers.alt);
        match self.tool {
            Tool::PolygonFill => {
                if resp.hovered() || resp.dragged() || resp.clicked() {
                    let hit = resp.hover_pos().or_else(|| resp.interact_pointer_pos()).and_then(|p| s3dp_core::select::pick_uv(&self.mesh, set, to_uv(p)));
                    self.polygon_fill_input(ui, resp, hit);
                }
            }
            Tool::Brush | Tool::Eraser => {
                let Some(tex) = self.viewport.paint.sets.get(set).map(|s| s.size as f32) else { return };
                let to_texel = |p: egui::Pos2| {
                    let uv = to_uv(p);
                    egui::pos2(uv.x * tex, uv.y * tex)
                };
                // Same on-screen brush size as in 3D.
                let radius = self.brush.size * tex / rect.width().max(1.0);
                let rs = self.render_state.clone();
                if resp.drag_started_by(egui::PointerButton::Primary) && !alt {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        match self.viewport.begin_stroke(&rs.device, &rs.queue, self.brush_for_stroke(1.0)) {
                            Ok(()) => {
                                let p = to_texel(pos);
                                self.viewport.paint.add_dabs_uv(set, &[[p.x, p.y, self.pressure()]], radius);
                                self.last_dab_uv = Some(p);
                                self.last_pressure = self.pressure();
                            }
                            Err(msg) => self.status = t(msg).to_owned(),
                        }
                    }
                } else if let (true, Some(pos), Some(mut last)) = (resp.dragged_by(egui::PointerButton::Primary), resp.interact_pointer_pos(), self.last_dab_uv) {
                    let target = to_texel(pos);
                    let (p0, p1) = (self.last_pressure, self.pressure());
                    let spacing = (radius * 0.12).max(0.75);
                    let total = last.distance(target);
                    let mut walked = 0.0;
                    let mut dabs = Vec::new();
                    while last.distance(target) >= spacing {
                        last += (target - last).normalized() * spacing;
                        walked += spacing;
                        let k = if total > 0.0 { walked / total } else { 1.0 };
                        dabs.push([last.x, last.y, p0 + (p1 - p0) * k]);
                    }
                    if !dabs.is_empty() {
                        self.last_pressure = p0 + (p1 - p0) * (walked / total.max(1e-6)).min(1.0);
                    }
                    self.viewport.paint.add_dabs_uv(set, &dabs, radius);
                    self.last_dab_uv = Some(last);
                }
                if self.last_dab_uv.is_some() && !resp.dragged_by(egui::PointerButton::Primary) {
                    self.viewport.paint.end_stroke();
                    self.last_dab_uv = None;
                }
                if let Some(pos) = resp.hover_pos() {
                    // On top of the UV image (drawn after this).
                    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("uv_cursor"))).with_clip_rect(rect);
                    painter.circle_stroke(pos, self.brush.size, egui::Stroke::new(2.0, egui::Color32::from_black_alpha(140)));
                    painter.circle_stroke(pos, self.brush.size, egui::Stroke::new(1.0, egui::Color32::WHITE));
                }
            }
            Tool::Eyedropper => {}
        }
    }

    fn viewport_overlay(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let area = egui::Area::new(egui::Id::new("viewport_overlay")).fixed_pos(rect.right_top() + egui::vec2(-8.0, 8.0)).pivot(egui::Align2::RIGHT_TOP).order(egui::Order::Middle);
        area.show(ui.ctx(), |ui| {
            egui::Frame::new().fill(egui::Color32::from_black_alpha(150)).corner_radius(4).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.split_uv, false, "3D").on_hover_text(t("Model only (U)"));
                    ui.selectable_value(&mut self.split_uv, true, "3D + UV").on_hover_text(t("Model and its flat texture side by side (U)"));
                    ui.separator();
                    if ui.selectable_label(self.env.open, egui_phosphor::regular::SUN_HORIZON).on_hover_text(t("Environment (HDRI lighting) · Shift + right-drag rotates it")).clicked() {
                        self.env.open = !self.env.open;
                    }
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

        if self.tool == Tool::PolygonFill {
            if response.hovered() || response.dragged() || response.clicked() {
                let hit = response.hover_pos().or_else(|| response.interact_pointer_pos()).and_then(|pos| self.pick_triangle_3d(pos, response.rect));
                self.polygon_fill_input(ui, response, hit);
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
            if shift && secondary && !alt {
                // Rotate the lighting environment (like Substance: Shift + right drag).
                let r = self.env.prefs.rotation_deg + delta.x * 0.4;
                self.env.prefs.rotation_deg = (r + 180.0).rem_euclid(360.0) - 180.0;
            } else if (alt && primary && self.last_dab.is_none()) || (middle && !alt && !shift) {
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
        let shortcuts: [(S, Action); 15] = [
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
            (S::new(M::COMMAND, Key::J), App::duplicate_selected),
            (S::new(M::COMMAND, Key::Q), |a| a.request(PendingAction::Quit)),
            (S::new(M::NONE, Key::U), |a| a.split_uv = !a.split_uv),
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
        if pressed(Key::P) {
            self.tool = Tool::PolygonFill;
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
        crate::ui_dock::save_layout(&self.dock);
        crate::ui_env::save_prefs(&self.env.prefs);
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
        crate::ui_env::poll(self, &ctx);
        self.frame += 1;
        self.viewport.show_overlay = self.tool == Tool::PolygonFill;
        if self.tool == Tool::PolygonFill && self.fill_overlay != Some(self.fill_mode) {
            self.build_fill_overlay();
        }
        // A fill stroke ends when the button is up, wherever it was released.
        if self.fill_stroke.is_some_and(|f| f < self.frame) && !ctx.input(|i| i.pointer.primary_down()) {
            self.viewport.paint.end_stroke();
            self.fill_stroke = None;
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
        // Viewport, Layers, Properties, Brush, Project: dockable tabs.
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| crate::ui_dock::show(self, ui));
        ui_bake::bake_window(self, &ctx);
        ui_export::export_window(self, &ctx);
        crate::ui_env::window(self, &ctx);
        if self.viewport.take_bake_finished() {
            self.bake.has_baked = true;
            self.viewport.paint.touch();
            self.status = t("Mesh maps baked — see View › Mesh map").to_owned();
        }
        if self.viewport.bake_progress().is_some() {
            ctx.request_repaint();
        } else if self.viewport.paint.wants_repaint() {
            // Deferred behind GPU work: check again soon (painting should feel live).
            ctx.request_repaint_after(std::time::Duration::from_millis(8));
        }
        self.confirm_dialog(ui);
    }
}

/// World ray (origin at the eye, direction) through a point in normalized device coordinates.
pub fn camera_ray(camera: &OrbitCamera, aspect: f32, ndc: [f32; 2]) -> (glam::Vec3, glam::Vec3) {
    let inv = (camera.projection(aspect) * camera.view()).inverse();
    let far = inv.project_point3(glam::Vec3::new(ndc[0], ndc[1], 1.0));
    let eye = camera.eye();
    (eye, (far - eye).normalize_or_zero())
}

#[cfg(test)]
mod ray_tests {
    use super::*;

    #[test]
    fn centre_ray_hits_the_framed_sphere() {
        let mesh = Mesh::uv_sphere(24, 12);
        let mut cam = OrbitCamera::default();
        cam.frame(glam::Vec3::ZERO, 1.0, 1.5);
        let (o, d) = camera_ray(&cam, 1.5, [0.0, 0.0]);
        assert!(d.dot((glam::Vec3::ZERO - o).normalize()) > 0.999, "centre ray looks at the target");
        let (_, dist) = s3dp_core::select::pick_ray(&mesh, o, d).expect("hits");
        assert!((dist - (o.length() - 1.0)).abs() < 0.05, "front surface: {dist}");
        // A corner ray misses the unit sphere.
        let (o, d) = camera_ray(&cam, 1.5, [0.98, 0.98]);
        assert!(s3dp_core::select::pick_ray(&mesh, o, d).is_none());
    }
}
