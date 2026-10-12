mod app;
mod color_picker;
mod project;
mod theme;
mod ui_bake;
mod ui_effects;
mod ui_layers;
mod ui_panels;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"))
        .init();
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
