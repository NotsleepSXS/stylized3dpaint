//! wgpu rendering for Stylized3DPaint: viewport, layer stack, effects and baking.

pub mod bake;
pub mod engine;
pub mod env;
pub mod gpu;
pub mod paint;
pub mod viewport;

pub use egui_wgpu::wgpu;
pub use paint::{Brush, ChannelTextures, EditTarget, Inspect, Painter};
pub use viewport::{Preview, ShadingMode, Viewport};
