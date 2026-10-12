mod app;
mod color_picker;
mod i18n;
mod project;
mod theme;
mod ui_bake;
mod ui_dock;
mod ui_export;
mod ui_effects;
mod ui_layers;
mod ui_panels;
mod ui_preview;
mod ui_widgets;

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
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .map(|d| d.join("stylized3dpaint"));
    let file = dir.and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        std::fs::File::create(d.join("stylized3dpaint.log")).ok()
    });
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"))
        .target(env_logger::Target::Pipe(Box::new(Tee(file))))
        .init();
}
