mod app;
mod color_picker;
mod project;
mod theme;
mod ui_layers;
mod ui_panels;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"))
        .init();
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Stylized3DPaint")
            .with_app_id("stylized3dpaint")
            .with_inner_size([1600.0, 1000.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Stylized3DPaint",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)?))),
    )
}
