// No console window on Windows (logs go to %LOCALAPPDATA%\stylized3dpaint).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod color_picker;
mod i18n;
mod paths;
mod project;
mod theme;
mod ui_bake;
mod ui_content;
mod ui_dock;
mod ui_export;
mod ui_effects;
mod ui_env;
mod ui_layers;
mod ui_panels;
mod ui_preview;
mod ui_widgets;

/// Support link (Boosty) shown in Help and the README.
pub const BOOSTY_URL: &str = "https://boosty.to/nosleeproom";
pub const REPO_URL: &str = "https://github.com/NotsleepSXS/stylized3dpaint";

/// Window / taskbar icon.
fn app_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../../../assets/icon/stylized3dpaint.png")).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

fn main() -> eframe::Result {
    init_logging();
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Stylized3DPaint")
            .with_app_id("stylized3dpaint")
            .with_inner_size([1600.0, 1000.0]),
        ..Default::default()
    };
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(std::sync::Arc::new(icon));
    }
    // Baking high-poly meshes needs big storage buffers: ask for what the GPU supports.
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut desc = base(adapter);
            let supported = adapter.limits();
            desc.required_limits.max_storage_buffer_binding_size = supported.max_storage_buffer_binding_size;
            desc.required_limits.max_buffer_size = supported.max_buffer_size;
            desc
        });
    }
    eframe::run_native(
        "Stylized3DPaint",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)?))),
    )
}

/// Logs to stderr and to `~/.cache/stylized3dpaint/stylized3dpaint.log` (spec §11).
fn init_logging() {
    struct Tee(Option<std::fs::File>);
    impl std::io::Write for Tee {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let _ = std::io::stderr().write_all(buf);
            if let Some(f) = &mut self.0 {
                let _ = f.write_all(buf);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if let Some(f) = &mut self.0 {
                f.flush()?;
            }
            Ok(())
        }
    }
    let dir = paths::cache_dir();
    let file = dir.and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        std::fs::File::create(d.join("stylized3dpaint.log")).ok()
    });
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"))
        .target(env_logger::Target::Pipe(Box::new(Tee(file))))
        .init();
}
