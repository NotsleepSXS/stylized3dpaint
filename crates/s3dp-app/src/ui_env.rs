//! Lighting environments (HDRIs): built-in 1K maps plus a user folder, background loading,
//! thumbnails, the viewport's Environment panel and persistence of the choice.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use egui_phosphor::regular as icon;
use s3dp_render::env::{EnvSettings, Equirect, GpuEnv, PreparedEnv, prepare};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::i18n::t;
use crate::theme;

macro_rules! builtin {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../../assets/hdri/", $name, "_1k.hdr")) as &[u8])),*]
    };
}

/// Built-in HDRIs (Poly Haven, CC0; see assets/hdri/LICENSE.md).
const BUILTIN: &[(&str, &[u8])] = builtin!(
    "studio_kominka_01",
    "ferndale_studio_07",
    "ferndale_studio_12",
    "camdeboo_road",
    "cobblestone_parish_road",
    "minedump_flats",
    "qwantani_dusk_2",
    "sunset_in_the_chalk_quarry",
);

const DEFAULT_ENV: &str = "studio_kominka_01";

#[derive(Clone, Debug)]
pub enum EnvSource {
    Builtin(&'static [u8]),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct EnvEntry {
    /// Name without resolution suffix (`camdeboo_road`).
    pub key: String,
    pub source: EnvSource,
}

impl EnvEntry {
    pub fn label(&self) -> String {
        let mut s: String = self.key.replace('_', " ");
        if let Some(c) = s.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        s
    }

    fn bytes(&self) -> Option<Vec<u8>> {
        match &self.source {
            EnvSource::Builtin(b) => Some(b.to_vec()),
            EnvSource::File(p) => std::fs::read(p).ok(),
        }
    }
}

/// `camdeboo_road_2k` -> `camdeboo_road`.
fn key_of(stem: &str) -> String {
    let lower = stem.to_ascii_lowercase();
    for suf in ["_1k", "_2k", "_4k", "_8k", "_16k"] {
        if let Some(k) = lower.strip_suffix(suf) {
            return k.to_owned();
        }
    }
    lower
}

/// Built-ins plus the folder's .hdr / .exr files; a folder map replaces the built-in of the
/// same name (higher resolution).
pub fn catalog(folder: Option<&Path>) -> Vec<EnvEntry> {
    let mut out: Vec<EnvEntry> = BUILTIN.iter().map(|(k, b)| EnvEntry { key: (*k).to_owned(), source: EnvSource::Builtin(b) }).collect();
    if let Some(dir) = folder
        && let Ok(rd) = std::fs::read_dir(dir)
    {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("hdr") || x.eq_ignore_ascii_case("exr"))).collect();
        files.sort();
        for p in files {
            let key = key_of(&p.file_stem().unwrap_or_default().to_string_lossy());
            match out.iter_mut().find(|e| e.key == key) {
                Some(e) => e.source = EnvSource::File(p),
                None => out.push(EnvEntry { key, source: EnvSource::File(p) }),
            }
        }
    }
    out
}

/// Persisted lighting settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvPrefs {
    pub folder: Option<PathBuf>,
    pub env: String,
    pub rotation_deg: f32,
    pub exposure: f32,
    pub background: bool,
    pub blur: f32,
    pub auto_exposure: bool,
}

impl Default for EnvPrefs {
    fn default() -> Self {
        Self { folder: None, env: DEFAULT_ENV.into(), rotation_deg: 0.0, exposure: 0.0, background: false, blur: 0.35, auto_exposure: true }
    }
}

fn prefs_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("stylized3dpaint").join("settings.ron"))
}

pub fn load_prefs() -> EnvPrefs {
    prefs_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| ron::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_prefs(p: &EnvPrefs) {
    let Some(path) = prefs_path() else { return };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = ron::ser::to_string_pretty(p, Default::default()) {
        let _ = std::fs::write(path, s);
    }
}

enum Loaded {
    Env(String, Box<PreparedEnv>),
    Thumb(String, usize, usize, Vec<u8>),
    Failed(String, String),
}

/// Environment state of the app.
pub struct EnvUi {
    pub prefs: EnvPrefs,
    pub entries: Vec<EnvEntry>,
    pub open: bool,
    /// Key being prepared in the background.
    loading: Option<String>,
    tx: mpsc::Sender<Loaded>,
    rx: mpsc::Receiver<Loaded>,
    thumbs: std::collections::HashMap<String, egui::TextureHandle>,
    thumbs_requested: bool,
    pub current: Option<Arc<GpuEnv>>,
    pub error: Option<String>,
}

impl EnvUi {
    pub fn new() -> Self {
        let prefs = load_prefs();
        let entries = catalog(prefs.folder.as_deref());
        let (tx, rx) = mpsc::channel();
        let mut s = Self { prefs, entries, open: false, loading: None, tx, rx, thumbs: Default::default(), thumbs_requested: false, current: None, error: None };
        let key = s.prefs.env.clone();
        s.select(&key);
        s
    }

    pub fn settings(&self) -> EnvSettings {
        EnvSettings { rotation: self.prefs.rotation_deg.to_radians(), exposure: self.prefs.exposure, background: self.prefs.background, blur: self.prefs.blur, auto: self.prefs.auto_exposure }
    }

    /// Starts preparing `key` in the background (falls back to the default map).
    pub fn select(&mut self, key: &str) {
        let Some(entry) = self.entries.iter().find(|e| e.key == key).or_else(|| self.entries.iter().find(|e| e.key == DEFAULT_ENV)).cloned() else { return };
        self.prefs.env = entry.key.clone();
        self.loading = Some(entry.key.clone());
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("s3dp-env".into())
            .spawn(move || {
                let t = std::time::Instant::now();
                let msg = match entry.bytes().map(|b| Equirect::decode(&b)) {
                    Some(Ok(eq)) => {
                        let top = if eq.width >= 2048 { 2048 } else { 1024 };
                        let p = prepare(&entry.label(), &eq, top);
                        log::info!("environment {} ({}x{}) prepared in {:?}", entry.key, eq.width, eq.height, t.elapsed());
                        Loaded::Env(entry.key.clone(), Box::new(p))
                    }
                    Some(Err(e)) => Loaded::Failed(entry.key.clone(), e.to_string()),
                    None => Loaded::Failed(entry.key.clone(), "cannot read the file".into()),
                };
                let _ = tx.send(msg);
            })
            .ok();
    }

    fn request_thumbs(&mut self) {
        if std::mem::replace(&mut self.thumbs_requested, true) {
            return;
        }
        let entries = self.entries.clone();
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("s3dp-env-thumbs".into())
            .spawn(move || {
                for e in entries {
                    let Some(Ok(eq)) = e.bytes().map(|b| Equirect::decode(&b)) else { continue };
                    let small = eq.downsample(160);
                    let px: Vec<u8> = small
                        .data
                        .iter()
                        .flat_map(|c| {
                            let m = |v: f32| ((v / (1.0 + v)).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
                            [m(c[0]), m(c[1]), m(c[2]), 255]
                        })
                        .collect();
                    let _ = tx.send(Loaded::Thumb(e.key.clone(), small.width, small.height, px));
                }
            })
            .ok();
    }

    pub fn set_folder(&mut self, folder: Option<PathBuf>) {
        self.prefs.folder = folder;
        self.entries = catalog(self.prefs.folder.as_deref());
        self.thumbs.clear();
        self.thumbs_requested = false;
        let key = self.prefs.env.clone();
        self.select(&key);
    }
}

/// Picks up finished loads (call every frame): uploads a prepared environment to the GPU and
/// gives it to the main and sample views.
pub fn poll(app: &mut App, ctx: &egui::Context) {
    while let Ok(msg) = app.env.rx.try_recv() {
        match msg {
            Loaded::Env(key, prepared) => {
                if app.env.loading.as_deref() != Some(key.as_str()) {
                    continue;
                }
                app.env.loading = None;
                app.env.error = None;
                let rs = app.render_state.clone();
                let gpu = Arc::new(GpuEnv::upload(&rs.device, &rs.queue, &prepared));
                app.viewport.set_environment(&rs.device, gpu.clone());
                if let Some(s) = &mut app.sample {
                    s.set_environment(&rs.device, gpu.clone());
                }
                app.env.current = Some(gpu);
            }
            Loaded::Thumb(key, w, h, px) => {
                let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &px);
                let tex = ctx.load_texture(format!("env_thumb_{key}"), img, egui::TextureOptions::LINEAR);
                app.env.thumbs.insert(key, tex);
            }
            Loaded::Failed(key, err) => {
                if app.env.loading.as_deref() == Some(key.as_str()) {
                    app.env.loading = None;
                }
                app.env.error = Some(format!("{key}: {err}"));
            }
        }
    }
    if app.env.loading.is_some() || !app.env.thumbs_requested || app.env.thumbs.len() < app.env.entries.len() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    let settings = app.env.settings();
    app.viewport.env_settings = settings;
}

/// The Environment panel (opened from the viewport overlay).
pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.env.open {
        return;
    }
    app.env.request_thumbs();
    let mut open = app.env.open;
    let mut pick: Option<String> = None;
    let mut folder_action: Option<Option<PathBuf>> = None;
    egui::Window::new(format!("{} {}", icon::SUN_HORIZON, t("Environment")))
        .open(&mut open)
        .resizable(true)
        .default_width(360.0)
        .show(ctx, |ui| {
            let e = &mut app.env;
            if let Some(k) = &e.loading {
                ui.label(egui::RichText::new(format!("{} {k}…", t("Loading"))).color(theme::TEXT_DIM));
            }
            if let Some(err) = &e.error {
                ui.colored_label(theme::DANGER, err);
            }
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for entry in &e.entries {
                        let selected = e.prefs.env == entry.key;
                        let size = egui::vec2(104.0, 52.0);
                        let resp = ui
                            .vertical(|ui| {
                                ui.set_width(size.x);
                                let r = match e.thumbs.get(&entry.key) {
                                    Some(tex) => ui.add(egui::Image::new((tex.id(), size)).sense(egui::Sense::click())),
                                    None => {
                                        let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click());
                                        ui.painter().rect_filled(rect, 2.0, theme::FIELD);
                                        r
                                    }
                                };
                                if selected {
                                    ui.painter().rect_stroke(r.rect.expand(1.0), 2.0, egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Outside);
                                }
                                let src = if matches!(entry.source, EnvSource::File(_)) { icon::FOLDER_SIMPLE } else { "" };
                                ui.label(egui::RichText::new(format!("{src} {}", entry.label())).small());
                                r
                            })
                            .inner;
                        if resp.clicked() {
                            pick = Some(entry.key.clone());
                        }
                    }
                });
            });
            ui.separator();
            egui::Grid::new("env_settings").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(t("Rotation"));
                ui.add(egui::Slider::new(&mut e.prefs.rotation_deg, -180.0..=180.0).suffix("°")).on_hover_text(t("Also: Shift + right-drag in the viewport"));
                ui.end_row();
                ui.label(t("Exposure"));
                ui.add(egui::Slider::new(&mut e.prefs.exposure, -3.0..=3.0).suffix(" EV"));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut e.prefs.auto_exposure, t("Auto exposure")).on_hover_text(t("Brings every map to the same average brightness"));
                ui.end_row();
                ui.label(t("Background"));
                ui.checkbox(&mut e.prefs.background, t("Show the HDRI behind the model"));
                ui.end_row();
                if e.prefs.background {
                    ui.label(t("Blur"));
                    ui.add(egui::Slider::new(&mut e.prefs.blur, 0.0..=1.0));
                    ui.end_row();
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(t("HDRI folder:"));
                let shown = e.prefs.folder.as_ref().map_or(t("(built-in only)").to_owned(), |p| p.display().to_string());
                ui.label(egui::RichText::new(shown).color(theme::TEXT_DIM).small());
            });
            ui.horizontal(|ui| {
                if ui.button(format!("{} {}", icon::FOLDER_OPEN, t("Choose folder…"))).clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    folder_action = Some(Some(dir));
                }
                if e.prefs.folder.is_some() && ui.button(t("Built-in only")).clicked() {
                    folder_action = Some(None);
                }
            });
            ui.label(egui::RichText::new(t(".hdr / .exr equirectangular maps; a map with a built-in's name replaces it (e.g. a 2K version).")).small().color(theme::TEXT_FAINT));
        });
    app.env.open = open;
    if let Some(f) = folder_action {
        app.env.set_folder(f);
    }
    if let Some(k) = pick {
        app.env.select(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_merges_folder_over_builtins() {
        assert_eq!(key_of("camdeboo_road_2k"), "camdeboo_road");
        assert_eq!(key_of("My_Sky"), "my_sky");
        let dir = std::env::temp_dir().join(format!("s3dp-env-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("camdeboo_road_2k.hdr"), b"x").unwrap();
        std::fs::write(dir.join("my_sky.exr"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let c = catalog(Some(&dir));
        assert_eq!(c.len(), BUILTIN.len() + 1);
        assert!(matches!(&c.iter().find(|e| e.key == "camdeboo_road").unwrap().source, EnvSource::File(_)), "folder map replaces the built-in");
        assert!(c.iter().any(|e| e.key == "my_sky"));
        assert!(matches!(&c.iter().find(|e| e.key == "studio_kominka_01").unwrap().source, EnvSource::Builtin(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn builtin_maps_decode() {
        for (k, b) in BUILTIN {
            let eq = Equirect::decode(b).unwrap_or_else(|e| panic!("{k}: {e}"));
            assert_eq!((eq.width, eq.height), (1024, 512), "{k}");
        }
    }

    /// Contact sheet: the test model lit by every built-in map (auto exposure), with the
    /// background shown. `cargo test --release -p stylized3dpaint env_sheet -- --ignored`
    #[test]
    #[ignore]
    fn env_sheet() {
        let instance = egui_wgpu::wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&Default::default())) else { return };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/monkey_crate.glb");
        let mesh = s3dp_core::import::load(&path).unwrap();
        let mut vp = s3dp_render::Viewport::new(&device);
        vp.set_mesh(&device, &queue, &mesh, 256);
        let mut cam = s3dp_core::OrbitCamera { yaw: 0.5, pitch: 0.2, ..Default::default() };
        let b = mesh.bounds();
        cam.frame(b.center(), b.radius(), 1.0);
        let (w, h) = (220usize, 220usize);
        let mut frames = Vec::new();
        for (k, bytes) in BUILTIN {
            let eq = Equirect::decode(bytes).unwrap();
            let env = Arc::new(GpuEnv::upload(&device, &queue, &prepare(k, &eq, 512)));
            vp.set_environment(&device, env);
            vp.env_settings = EnvSettings { background: true, blur: 0.2, ..Default::default() };
            vp.render(&device, &queue, [w as u32, h as u32], &cam, s3dp_render::ShadingMode::Lit);
            frames.push(vp.read_color(&device, &queue).unwrap().2);
        }
        let cols = 4;
        let rows = frames.len().div_ceil(cols);
        let mut out = vec![0u8; w * cols * h * rows * 4];
        for (n, f) in frames.iter().enumerate() {
            let (cx, cy) = (n % cols, n / cols);
            for y in 0..h {
                let o = ((cy * h + y) * w * cols + cx * w) * 4;
                out[o..o + w * 4].copy_from_slice(&f[y * w * 4..(y + 1) * w * 4]);
            }
        }
        let p = std::env::temp_dir().join("s3dp_env_sheet.png");
        image::save_buffer(&p, &out, (w * cols) as u32, (h * rows) as u32, image::ColorType::Rgba8).unwrap();
        println!("{}", p.display());
    }
}
