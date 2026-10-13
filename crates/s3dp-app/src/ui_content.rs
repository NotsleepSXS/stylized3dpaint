//! Content browser: preset materials (layer / folder stacks, like smart materials), preset
//! masks and the effect library, with rendered thumbnails; saving your own presets from the
//! layer stack; rename / delete. User presets live in `<config>/library/{materials,masks}`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use egui::RichText;
use egui_phosphor::regular as icon;
use s3dp_core::library::{self, MASK_EXT, MATERIAL_EXT, MaskPreset, MaterialPreset, Origin};
use s3dp_core::{BakeSettings, BakeStyle, ChannelId, ChannelValues, Mesh, NodeKind, OrbitCamera};
use s3dp_render::{ShadingMode, Viewport};

use crate::app::App;
use crate::i18n::{pick, t};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContentTab {
    Materials,
    Masks,
    Effects,
}

/// Save-as-preset dialog.
pub struct SaveDialog {
    pub node: u64,
    pub mask: bool,
    pub name: String,
}

pub struct ContentUi {
    pub tab: ContentTab,
    pub search: String,
    materials: Option<Vec<(Origin, MaterialPreset)>>,
    masks: Option<Vec<(Origin, MaskPreset)>>,
    thumbs: HashMap<String, egui::TextureHandle>,
    queue: VecDeque<String>,
    scene: Option<ThumbScene>,
    pub save: Option<SaveDialog>,
    rename: Option<(PathBuf, String)>,
    pub status: Option<String>,
}

impl Default for ContentUi {
    fn default() -> Self {
        Self { tab: ContentTab::Materials, search: String::new(), materials: None, masks: None, thumbs: HashMap::new(), queue: VecDeque::new(), scene: None, save: None, rename: None, status: None }
    }
}

pub fn library_dir(mask: bool) -> Option<PathBuf> {
    Some(crate::paths::config_dir()?.join("library").join(if mask { "masks" } else { "materials" }))
}

fn key_of(origin: &Origin, name: &str, mask: bool) -> String {
    let kind = if mask { "mask" } else { "mat" };
    match origin {
        Origin::Builtin => format!("{kind}:builtin:{name}"),
        Origin::File(p) => format!("{kind}:file:{}", p.display()),
    }
}

impl ContentUi {
    fn materials(&mut self) -> &[(Origin, MaterialPreset)] {
        if self.materials.is_none() {
            let mut v: Vec<(Origin, MaterialPreset)> = library::builtin_materials().into_iter().map(|p| (Origin::Builtin, p)).collect();
            if let Some(d) = library_dir(false) {
                v.extend(library::list(&d, MATERIAL_EXT, library::load_material).into_iter().map(|(p, m)| (Origin::File(p), m)));
            }
            self.materials = Some(v);
        }
        self.materials.as_deref().unwrap_or_default()
    }

    fn masks(&mut self) -> &[(Origin, MaskPreset)] {
        if self.masks.is_none() {
            let mut v: Vec<(Origin, MaskPreset)> = library::builtin_masks().into_iter().map(|p| (Origin::Builtin, p)).collect();
            if let Some(d) = library_dir(true) {
                v.extend(library::list(&d, MASK_EXT, library::load_mask).into_iter().map(|(p, m)| (Origin::File(p), m)));
            }
            self.masks = Some(v);
        }
        self.masks.as_deref().unwrap_or_default()
    }

    pub fn reload(&mut self) {
        self.materials = None;
        self.masks = None;
        // User presets are keyed by file path: a deleted / re-saved file must re-render.
        self.thumbs.retain(|k, _| k.contains(":builtin:"));
        self.queue.clear();
    }
}

/// Small scene that renders preset thumbnails on a baked sphere.
struct ThumbScene {
    vp: Viewport,
    cam: OrbitCamera,
}

/// Preview shape: a sphere with a groove around its equator and a smaller one above, so
/// edge / cavity based presets and masks have something to show.
fn preview_mesh() -> Mesh {
    let mut m = Mesh::uv_sphere(96, 48);
    for v in &mut m.vertices {
        let y = v.position[1];
        let groove = |c: f32, w: f32, depth: f32| {
            let d = ((y - c).abs() / w).min(1.0);
            depth * (1.0 - d * d * (3.0 - 2.0 * d))
        };
        let k = 1.0 - groove(0.0, 0.11, 0.14) - groove(0.55, 0.06, 0.07);
        v.position[0] *= k;
        v.position[2] *= k;
    }
    m.compute_normals();
    m.compute_tangents();
    m
}

impl ThumbScene {
    fn new(device: &egui_wgpu::wgpu::Device, queue: &egui_wgpu::wgpu::Queue) -> Self {
        let mesh = preview_mesh();
        let mut vp = Viewport::new(device);
        vp.set_mesh(device, queue, &mesh, 256);
        let settings = BakeSettings { ao_samples: 12, edge_samples: 8, thickness_samples: 8, sky_samples: 12, ..BakeSettings::preset(BakeStyle::Stylized) };
        vp.bake_now(device, queue, &mesh, None, &settings);
        let mut cam = OrbitCamera { yaw: 0.6, pitch: 0.3, ..Default::default() };
        cam.frame(glam::Vec3::ZERO, 1.0, 1.0);
        cam.distance *= 0.95;
        Self { vp, cam }
    }

    fn render(&mut self, device: &egui_wgpu::wgpu::Device, queue: &egui_wgpu::wgpu::Queue, build: impl FnOnce(&mut s3dp_render::Painter)) -> egui::ColorImage {
        self.vp.paint.reset_document();
        self.vp.paint.throttle = false;
        build(&mut self.vp.paint);
        let size = [112u32, 112];
        for _ in 0..2 {
            self.vp.render(device, queue, size, &self.cam, ShadingMode::Lit);
        }
        let px = self.vp.read_color(device, queue).map(|r| r.2).unwrap_or_default();
        if px.len() == (size[0] * size[1] * 4) as usize {
            egui::ColorImage::from_rgba_unmultiplied([size[0] as usize, size[1] as usize], &px)
        } else {
            egui::ColorImage::new([1, 1], vec![egui::Color32::DARK_GRAY])
        }
    }
}

/// Renders at most one pending thumbnail per frame.
fn pump_thumbnails(app: &mut App, ctx: &egui::Context) {
    let Some(key) = app.content.queue.pop_front() else { return };
    if app.content.thumbs.contains_key(&key) {
        return;
    }
    let rs = app.render_state.clone();
    let env = app.env.current.clone();
    let scene = app.content.scene.get_or_insert_with(|| ThumbScene::new(&rs.device, &rs.queue));
    if let Some(env) = env {
        scene.vp.set_environment(&rs.device, env);
    }
    let material = app.content.materials().iter().find(|(o, p)| key_of(o, &p.name, false) == key).map(|(_, p)| p.node.clone());
    let mask = app.content.masks().iter().find(|(o, p)| key_of(o, &p.name, true) == key).map(|(_, p)| p.mask.clone());
    let scene = app.content.scene.as_mut().unwrap();
    let img = if let Some(node) = material {
        scene.render(&rs.device, &rs.queue, |p| {
            p.insert_preset(node);
        })
    } else if let Some(mask) = mask {
        scene.render(&rs.device, &rs.queue, |p| {
            let mut dark = ChannelValues::defaults();
            dark.set(ChannelId::BaseColor, [0.05, 0.05, 0.06]);
            p.add_node(NodeKind::Fill(dark), |_, _| {});
            let mut light = ChannelValues::defaults();
            light.set(ChannelId::BaseColor, [0.9, 0.85, 0.75]);
            let id = p.add_node(NodeKind::Fill(light), |_, _| {});
            p.apply_mask_preset(id, mask);
        })
    } else {
        return;
    };
    let tex = ctx.load_texture(key.clone(), img, egui::TextureOptions::LINEAR);
    app.content.thumbs.insert(key, tex);
    ctx.request_repaint();
}

fn tile(ui: &mut egui::Ui, thumb: Option<&egui::TextureHandle>, label: &str, user: bool) -> egui::Response {
    let size = egui::vec2(92.0, 92.0);
    ui.vertical(|ui| {
        ui.set_width(size.x);
        let r = match thumb {
            Some(t) => ui.add(egui::Image::new((t.id(), size)).corner_radius(4).sense(egui::Sense::click())),
            None => {
                let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
                ui.painter().rect_filled(rect, 4.0, theme::FIELD);
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::HOURGLASS, egui::FontId::proportional(18.0), theme::TEXT_FAINT);
                r
            }
        };
        if r.hovered() {
            ui.painter().rect_stroke(r.rect, 4.0, egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Outside);
        }
        let mark = if user { format!("{} ", icon::USER) } else { String::new() };
        ui.add(egui::Label::new(RichText::new(format!("{mark}{label}")).small()).truncate());
        r
    })
    .inner
}

enum Act {
    ApplyMaterial(s3dp_core::Node),
    ApplyMask(s3dp_core::Mask),
    Delete(PathBuf),
    StartRename(PathBuf, String),
    AddEffect(String),
}

/// The Content panel body.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    pump_thumbnails(app, ui.ctx());
    let mut act: Option<Act> = None;
    egui::Frame::new().fill(theme::CHROME).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        ui.horizontal(|ui| {
            let c = &mut app.content;
            ui.selectable_value(&mut c.tab, ContentTab::Materials, format!("{} {}", icon::SPHERE, t("Preset materials")));
            ui.selectable_value(&mut c.tab, ContentTab::Masks, format!("{} {}", icon::SQUARE_HALF, t("Preset masks")));
            ui.selectable_value(&mut c.tab, ContentTab::Effects, format!("{} {}", icon::MAGIC_WAND, t("Effects")));
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut c.search).hint_text(format!("{} {}", icon::MAGNIFYING_GLASS, t("Search"))).desired_width(160.0));
            if c.tab != ContentTab::Effects && ui.button(format!("{} {}", icon::FOLDER_OPEN, t("Library folder"))).on_hover_text(t("Your presets are files here; share them by copying")).clicked() {
                if let Some(d) = library_dir(c.tab == ContentTab::Masks) {
                    let _ = std::fs::create_dir_all(&d);
                    open_folder(&d);
                }
            }
            if let Some(s) = &c.status {
                ui.label(RichText::new(s).small().color(theme::TEXT_DIM));
            }
        });
        ui.add_space(4.0);
        let query = app.content.search.to_lowercase();
        let matches = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| match app.content.tab {
                ContentTab::Materials => {
                    let items: Vec<(Origin, MaterialPreset)> = app.content.materials().iter().filter(|(_, p)| matches(&p.name)).cloned().collect();
                    if items.is_empty() {
                        ui.label(RichText::new(t("Nothing found")).color(theme::TEXT_FAINT));
                    }
                    for (origin, p) in items {
                        let key = key_of(&origin, &p.name, false);
                        if !app.content.thumbs.contains_key(&key) && !app.content.queue.contains(&key) {
                            app.content.queue.push_back(key.clone());
                        }
                        let user = matches!(origin, Origin::File(_));
                        let r = tile(ui, app.content.thumbs.get(&key), &p.name, user).on_hover_text(t("Click: add above the selected layer"));
                        if r.clicked() {
                            act = Some(Act::ApplyMaterial(p.node.clone()));
                        }
                        if let Origin::File(path) = &origin {
                            r.context_menu(|ui| {
                                if ui.button(t("Rename")).clicked() {
                                    act = Some(Act::StartRename(path.clone(), p.name.clone()));
                                    ui.close();
                                }
                                if ui.button(format!("{} {}", icon::TRASH, t("Delete"))).clicked() {
                                    act = Some(Act::Delete(path.clone()));
                                    ui.close();
                                }
                            });
                        }
                    }
                }
                ContentTab::Masks => {
                    let items: Vec<(Origin, MaskPreset)> = app.content.masks().iter().filter(|(_, p)| matches(&p.name)).cloned().collect();
                    for (origin, p) in items {
                        let key = key_of(&origin, &p.name, true);
                        if !app.content.thumbs.contains_key(&key) && !app.content.queue.contains(&key) {
                            app.content.queue.push_back(key.clone());
                        }
                        let user = matches!(origin, Origin::File(_));
                        let r = tile(ui, app.content.thumbs.get(&key), &p.name, user).on_hover_text(t("Click: set as the mask of the selected layer"));
                        if r.clicked() {
                            act = Some(Act::ApplyMask(p.mask.clone()));
                        }
                        if let Origin::File(path) = &origin {
                            r.context_menu(|ui| {
                                if ui.button(t("Rename")).clicked() {
                                    act = Some(Act::StartRename(path.clone(), p.name.clone()));
                                    ui.close();
                                }
                                if ui.button(format!("{} {}", icon::TRASH, t("Delete"))).clicked() {
                                    act = Some(Act::Delete(path.clone()));
                                    ui.close();
                                }
                            });
                        }
                    }
                }
                ContentTab::Effects => {
                    let ru = crate::i18n::is_russian();
                    let mut defs: Vec<_> = s3dp_core::registry().effects.iter().filter(|d| matches(d.display_name(ru)) || matches(&d.id)).collect();
                    defs.sort_by_key(|d| (s3dp_render::engine::effects::category_order(d.category), d.display_name(ru).to_owned()));
                    for d in defs {
                        let glyph = crate::ui_layers::effect_icon(d.category);
                        let tip = pick(d.description.as_deref().unwrap_or(""), d.description_ru.as_deref());
                        let b = ui.add_sized([150.0, 30.0], egui::Button::new(format!("{glyph} {}", d.display_name(ru)))).on_hover_text(format!("{tip}\n\n{}", t("Click: add to every selected layer")));
                        if b.clicked() {
                            act = Some(Act::AddEffect(d.id.clone()));
                        }
                    }
                }
            });
        });
    });
    match act {
        Some(Act::ApplyMaterial(node)) => {
            let id = app.viewport.paint.insert_preset(node);
            app.layers_ui.open_effects_for(id);
            app.layers_ui.selection = vec![id];
        }
        Some(Act::ApplyMask(mask)) => match app.viewport.paint.active {
            Some(id) => app.viewport.paint.apply_mask_preset(id, mask),
            None => app.content.status = Some(t("Select a layer first").to_owned()),
        },
        Some(Act::Delete(path)) => {
            let _ = std::fs::remove_file(&path);
            app.content.reload();
        }
        Some(Act::StartRename(path, name)) => app.content.rename = Some((path, name)),
        Some(Act::AddEffect(id)) => crate::ui_layers::add_effect_to_selected(app, &id),
        None => {}
    }
}

fn open_folder(dir: &std::path::Path) {
    let cmd = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

/// Save-as-preset and rename dialogs.
pub fn dialogs(app: &mut App, ctx: &egui::Context) {
    if let Some(dlg) = &mut app.content.save {
        let mut open = true;
        let mut do_save = false;
        let title = if dlg.mask { t("Save mask as preset") } else { t("Save as preset material") };
        egui::Window::new(title).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            ui.label(t("Name"));
            let r = ui.text_edit_singleline(&mut dlg.name);
            if !r.has_focus() && !r.lost_focus() && dlg.name.is_empty() {
                r.request_focus();
            }
            ui.label(RichText::new(t("Painted strokes are not saved: they belong to this model's UVs.")).small().color(theme::TEXT_FAINT));
            ui.horizontal(|ui| {
                if ui.button(format!("{} {}", icon::FLOPPY_DISK, t("Save"))).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    do_save = true;
                }
            });
        });
        if do_save {
            let (node_id, mask, name) = (dlg.node, dlg.mask, dlg.name.trim().to_owned());
            let name = if name.is_empty() { t("My preset").to_owned() } else { name };
            let result = save_preset(app, node_id, mask, &name);
            app.content.status = Some(match result {
                Ok(()) => format!("{} «{name}»", t("Saved")),
                Err(e) => format!("{}: {e}", t("Could not save")),
            });
            app.content.tab = if mask { ContentTab::Masks } else { ContentTab::Materials };
            app.content.reload();
            app.content.save = None;
            app.focus_tab = Some(crate::ui_dock::Tab::Content);
        } else if !open {
            app.content.save = None;
        }
    }
    if let Some((path, name)) = &mut app.content.rename {
        let mut open = true;
        let mut ok = false;
        egui::Window::new(t("Rename preset")).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            let r = ui.text_edit_singleline(name);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            if ui.button(t("Rename")).clicked() {
                ok = true;
            }
        });
        if ok {
            let (path, name) = (path.clone(), name.trim().to_owned());
            if path.extension().is_some_and(|e| e == MATERIAL_EXT) {
                if let Some(mut p) = library::load_material(&path) {
                    p.name = name;
                    let _ = library::save_material(&path, &p);
                }
            } else if let Some(mut p) = library::load_mask(&path) {
                p.name = name;
                let _ = library::save_mask(&path, &p);
            }
            app.content.rename = None;
            app.content.reload();
        } else if !open {
            app.content.rename = None;
        }
    }
}

fn save_preset(app: &App, node_id: u64, mask: bool, name: &str) -> std::io::Result<()> {
    let node = app.viewport.paint.doc.find(node_id).cloned().ok_or_else(|| std::io::Error::other("layer not found"))?;
    let dir = library_dir(mask).ok_or_else(|| std::io::Error::other("no settings folder"))?;
    if mask {
        let m = node.mask.ok_or_else(|| std::io::Error::other("the layer has no mask"))?;
        library::save_mask(&library::free_path(&dir, name, MASK_EXT), &MaskPreset { format: 1, name: name.into(), mask: m })
    } else {
        let mut node = node;
        node.name = name.into();
        library::save_material(&library::free_path(&dir, name, MATERIAL_EXT), &MaterialPreset { format: 1, name: name.into(), node })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every built-in preset renders without effect errors; writes a contact sheet.
    #[test]
    fn builtin_presets_render() {
        let instance = s3dp_render::gpu::new_instance();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&Default::default())) else { return };
        let mut scene = ThumbScene::new(&device, &queue);
        let mut imgs = Vec::new();
        let mut errors = Vec::new();
        for p in library::builtin_materials() {
            let img = scene.render(&device, &queue, |paint| {
                paint.insert_preset(p.node.clone());
            });
            errors.extend(scene.vp.paint.errors.values().map(|e| format!("{}: {e}", p.name)));
            imgs.push(img);
        }
        for p in library::builtin_masks() {
            let img = scene.render(&device, &queue, |paint| {
                let mut dark = ChannelValues::defaults();
                dark.set(ChannelId::BaseColor, [0.05, 0.05, 0.06]);
                paint.add_node(NodeKind::Fill(dark), |_, _| {});
                let mut light = ChannelValues::defaults();
                light.set(ChannelId::BaseColor, [0.9, 0.85, 0.75]);
                let id = paint.add_node(NodeKind::Fill(light), |_, _| {});
                paint.apply_mask_preset(id, p.mask.clone());
            });
            errors.extend(scene.vp.paint.errors.values().map(|e| format!("{}: {e}", p.name)));
            imgs.push(img);
        }
        assert!(errors.is_empty(), "{errors:?}");
        let (w, h) = (112usize, 112usize);
        let cols = 6;
        let rows = imgs.len().div_ceil(cols);
        let mut out = vec![0u8; w * cols * h * rows * 4];
        for (n, img) in imgs.iter().enumerate() {
            let (cx, cy) = (n % cols, n / cols);
            for y in 0..h {
                for x in 0..w {
                    let c = img.pixels[y * w + x];
                    let o = ((cy * h + y) * w * cols + cx * w + x) * 4;
                    out[o..o + 4].copy_from_slice(&[c.r(), c.g(), c.b(), 255]);
                }
            }
        }
        let path = std::env::temp_dir().join("s3dp_presets_sheet.png");
        image::save_buffer(&path, &out, (w * cols) as u32, (h * rows) as u32, image::ColorType::Rgba8).unwrap();
        println!("{}", path.display());
    }
}
