//! Export window (spec §9.1): texture sets, preset with its outputs, settings, export in the
//! background. Packing / padding / file writing live in the `s3dp-export` crate.

use std::path::PathBuf;
use std::sync::mpsc;

use egui::RichText;
use egui_phosphor::regular as icon;
use s3dp_core::{BakedMap, ChannelId};
use s3dp_export::{Dither, ExportPreset, ExportReport, ExportSettings, ExportSource, FileFormat, Padding, builtin_presets};

use crate::app::App;
use crate::i18n::{is_russian, pick, t};
use crate::theme;

pub struct ExportUi {
    pub open: bool,
    presets: Vec<ExportPreset>,
    preset: usize,
    pub settings: ExportSettings,
    /// Texture sets to export (by index).
    sets: Vec<bool>,
    open_folder: bool,
    running: Option<mpsc::Receiver<anyhow::Result<ExportReport>>>,
    last: Option<String>,
    /// An export was configured and run in this session (Ctrl+Shift+E repeats it).
    pub exported_once: bool,
}

impl Default for ExportUi {
    fn default() -> Self {
        let presets = builtin_presets();
        let settings = ExportSettings { output_dir: default_dir(), ..ExportSettings::default() };
        Self { open: false, presets, preset: 0, settings, sets: Vec::new(), open_folder: true, running: None, last: None, exported_once: false }
    }
}

fn default_dir() -> PathBuf {
    crate::paths::home().map(|h| h.join("Exports")).unwrap_or_else(|| PathBuf::from("."))
}

/// Reads every needed buffer from the GPU (on the UI thread), then exports in a thread.
pub fn start(app: &mut App) {
    if app.export.running.is_some() {
        return;
    }
    app.export.exported_once = true;
    let rs = app.render_state.clone();
    let paint = &app.viewport.paint;
    let mut sources = Vec::new();
    for (si, set) in paint.sets.iter().enumerate() {
        if !app.export.sets.get(si).copied().unwrap_or(true) {
            continue;
        }
        let mut src = ExportSource::new(set.name.clone(), set.size, set.size);
        for c in paint.doc.channels.iter() {
            if let Some((_, values)) = paint.read_final(&rs.device, &rs.queue, si, c) {
                src.channels.insert(c, values);
            }
        }
        if app.bake.has_baked {
            for m in BakedMap::ALL {
                if let Some(values) = paint.read_baked(&rs.device, &rs.queue, si, m) {
                    src.baked.insert(m, values);
                }
            }
        } else if let Some(values) = paint.read_baked(&rs.device, &rs.queue, si, BakedMap::Normal) {
            src.baked.insert(BakedMap::Normal, values);
        }
        if let Some(cov) = paint.read_coverage(&rs.device, &rs.queue, si) {
            src.coverage = cov;
        }
        src.uses_opacity = paint.doc.channels.has(ChannelId::Opacity) && src.detect_uses_opacity();
        sources.push(src);
    }
    let preset = app.export.presets[app.export.preset].clone();
    let mut settings = app.export.settings.clone();
    settings.mesh_name = app.mesh.name.clone();
    settings.project_name = app.project_path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| app.mesh.name.clone(), |s| s.to_string_lossy().into_owned());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = std::fs::create_dir_all(&settings.output_dir).map_err(anyhow::Error::from).and_then(|_| s3dp_export::export(&sources, &preset, &settings));
        let _ = tx.send(result);
    });
    app.export.running = Some(rx);
    app.status = t("Exporting textures…").to_owned();
}

pub fn export_window(app: &mut App, ctx: &egui::Context) {
    // Collect a finished export.
    if let Some(rx) = &app.export.running {
        match rx.try_recv() {
            Ok(result) => {
                app.export.running = None;
                let msg = match result {
                    Ok(report) => {
                        if app.export.open_folder {
                            let _ = std::process::Command::new("xdg-open").arg(&app.export.settings.output_dir).spawn();
                        }
                        let warns = report.warnings.len();
                        format!("{} {} {} → {}{}", t("Exported"), report.files.len(), t("file(s)"), app.export.settings.output_dir.display(), if warns > 0 { format!(" ({warns} {})", t("warning(s)")) } else { String::new() })
                    }
                    Err(e) => format!("{}: {e:#}", t("Export failed")),
                };
                app.status = msg.clone();
                app.export.last = Some(msg);
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            Err(mpsc::TryRecvError::Disconnected) => app.export.running = None,
        }
    }
    if !app.export.open {
        return;
    }
    let set_names: Vec<String> = app.viewport.paint.sets.iter().map(|s| s.name.clone()).collect();
    app.export.sets.resize(set_names.len(), true);
    let mut open = true;
    let mut go = false;
    egui::Window::new(format!("{} {}", icon::EXPORT, t("Export textures"))).open(&mut open).default_width(560.0).collapsible(false).show(ctx, |ui| {
        let e = &mut app.export;
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            ui.label(RichText::new(t("Texture sets")).strong());
            for (i, name) in set_names.iter().enumerate() {
                ui.checkbox(&mut e.sets[i], name);
            }
            ui.add_space(8.0);
            ui.label(RichText::new(t("Preset")).strong());
            let ru = is_russian();
            let names: Vec<String> = e.presets.iter().map(|p| if ru && !p.name_ru.is_empty() { p.name_ru.clone() } else { p.name.clone() }).collect();
            egui::ComboBox::from_id_salt("export_preset").selected_text(&names[e.preset]).width(240.0).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut e.preset, i, n);
                }
            });
            let p = &mut e.presets[e.preset];
            let hint = pick(&p.hint, Some(&p.hint_ru)).to_owned();
            if !hint.is_empty() {
                ui.label(RichText::new(hint).color(theme::TEXT_FAINT).small());
            }
            ui.label(RichText::new(format!("{} {:?}", t("Normal map:"), p.normal)).small());
            for o in &mut p.outputs {
                ui.checkbox(&mut o.enabled, format!("_{} ({:?}{})", o.suffix, o.kind, if o.srgb { ", sRGB" } else { "" }));
            }
            ui.horizontal(|ui| {
                if ui.button(t("Load preset…")).clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file()
                {
                    match s3dp_export::load_preset(&path) {
                        Ok(p) => {
                            e.presets.push(p);
                            e.preset = e.presets.len() - 1;
                        }
                        Err(err) => e.last = Some(format!("{err:#}")),
                    }
                }
                if ui.button(t("Save preset…")).clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name(format!("{}.json", e.presets[e.preset].name)).save_file()
                    && let Err(err) = s3dp_export::save_preset(&path, &e.presets[e.preset])
                {
                    e.last = Some(format!("{err:#}"));
                }
            });

            let ui = &mut cols[1];
            ui.label(RichText::new(t("Settings")).strong());
            ui.horizontal(|ui| {
                ui.label(t("Folder"));
                ui.label(RichText::new(e.settings.output_dir.display().to_string()).monospace().small());
            });
            if ui.button(format!("{} {}", icon::FOLDER_OPEN, t("Choose folder…"))).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_directory(&e.settings.output_dir).pick_folder()
            {
                e.settings.output_dir = dir;
            }
            ui.horizontal(|ui| {
                ui.label(t("Name"));
                let mut tpl = e.settings.naming_template.clone().unwrap_or_default();
                let hint = e.presets[e.preset].naming.clone().unwrap_or_else(|| s3dp_export::DEFAULT_TEMPLATE.to_owned());
                if ui.add(egui::TextEdit::singleline(&mut tpl).hint_text(hint).desired_width(180.0)).changed() {
                    e.settings.naming_template = (!tpl.is_empty()).then_some(tpl);
                }
            });
            ui.label(RichText::new("$project $mesh $textureSet $suffix $res $udim").color(theme::TEXT_FAINT).small());
            ui.horizontal(|ui| {
                ui.label(t("Format"));
                egui::ComboBox::from_id_salt("export_format").selected_text(e.settings.format.label()).show_ui(ui, |ui| {
                    for f in FileFormat::ALL {
                        ui.selectable_value(&mut e.settings.format, f, f.label());
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(t("Padding"));
                let label = match e.settings.padding {
                    Padding::Infinite => t("Infinite"),
                    Padding::Pixels(_) => t("N pixels"),
                    Padding::Transparent => t("Transparent"),
                    Padding::None => t("None"),
                };
                egui::ComboBox::from_id_salt("export_padding").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut e.settings.padding, Padding::Infinite, t("Infinite"));
                    ui.selectable_value(&mut e.settings.padding, Padding::Pixels(16), t("N pixels"));
                    ui.selectable_value(&mut e.settings.padding, Padding::Transparent, t("Transparent"));
                    ui.selectable_value(&mut e.settings.padding, Padding::None, t("None"));
                });
                if let Padding::Pixels(n) = &mut e.settings.padding {
                    ui.add(egui::DragValue::new(n).range(1..=256).suffix(" px"));
                }
            });
            ui.horizontal(|ui| {
                ui.label(t("Dithering (8-bit)"));
                egui::ComboBox::from_id_salt("export_dither").selected_text(format!("{:?}", e.settings.dither)).show_ui(ui, |ui| {
                    for d in [Dither::Ordered, Dither::Triangular, Dither::None] {
                        ui.selectable_value(&mut e.settings.dither, d, format!("{d:?}"));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut e.settings.height_to_normal, t("Bake height into the normal map"))
                    .on_hover_text(t("Game engines do not use the Height map for shading: without this the model looks flat after export"));
                if e.settings.height_to_normal {
                    ui.label(t("Strength"));
                    ui.add(egui::DragValue::new(&mut e.settings.height_normal_strength).range(0.0..=10.0).speed(0.02).max_decimals(2));
                }
            });
            ui.checkbox(&mut e.open_folder, t("Open the folder after export"));
        });
        ui.separator();
        let sources_stub: Vec<ExportSource> = Vec::new();
        let warnings = s3dp_export::validate(&sources_stub, &app.export.presets[app.export.preset], &app.export.settings);
        for w in warnings.iter().take(4) {
            ui.colored_label(theme::WARNING, format!("{} {}", icon::WARNING, w.message));
        }
        ui.horizontal(|ui| {
            let busy = app.export.running.is_some();
            let label = if busy { t("Exporting…") } else { t("Export") };
            if ui.add_enabled(!busy, egui::Button::new(RichText::new(format!("{} {label}", icon::EXPORT)).strong()).fill(theme::ACCENT).min_size(egui::vec2(120.0, 28.0))).clicked() {
                go = true;
            }
            if let Some(last) = &app.export.last {
                ui.label(RichText::new(last).color(theme::TEXT_DIM).small());
            }
        });
    });
    app.export.open = open;
    if go {
        start(app);
    }
}
