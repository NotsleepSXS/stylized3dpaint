//! wgpu rendering for Stylized3DPaint: the 3D viewport and texture painting.

pub mod paint;
pub mod viewport;

pub use egui_wgpu::wgpu;
pub use paint::Brush;
pub use viewport::{ShadingMode, Viewport};
