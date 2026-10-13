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

/// Shows a fatal error to the user (there is no console on Windows) and logs it.
fn report_fatal(msg: &str) {
    log::error!("{msg}");
    let log = paths::cache_dir().map(|d| d.join("stylized3dpaint.log").display().to_string()).unwrap_or_default();
    let text = format!("{msg}\n\nLog: {log}\nPlease report it: {REPO_URL}/issues");
    let _ = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("Stylized3DPaint").set_description(text).set_buttons(rfd::MessageButtons::Ok).show();
}

fn main() -> eframe::Result {
    init_logging();
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        log::error!("panic: {info}\n{bt}");
        // Background threads (HDRI loading, export…) report through the UI; only a panic on
        // the main thread stops the app.
        if std::thread::current().name() == Some("main") {
            report_fatal(&format!("Stylized3DPaint stopped because of an error:\n{info}"));
        }
    }));
    log::info!("Stylized3DPaint {} on {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
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
        s3dp_render::gpu::configure_instance(&mut setup.instance_descriptor);
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut desc = base(adapter);
            let supported = adapter.limits();
            desc.required_limits.max_storage_buffer_binding_size = supported.max_storage_buffer_binding_size;
            desc.required_limits.max_buffer_size = supported.max_buffer_size;
            desc
        });
    }
    let result = eframe::run_native(
        "Stylized3DPaint",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)?))),
    );
    if let Err(e) = &result {
        report_fatal(&format!("Stylized3DPaint could not start:\n{e}\n\nThe graphics driver may not support Vulkan / DirectX 12. Updating the GPU driver usually helps."));
    }
    result
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
