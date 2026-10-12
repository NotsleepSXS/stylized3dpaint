//! Document model of project formats 1–2 (flat layer list, mask-only effects).
//! Kept only to migrate old projects; see `s3dp-app/src/project.rs`.

pub mod document;
pub mod effects;

pub use document::{BlendMode, Channel, ChannelMask, ChannelValues, Layer, LayerKind};
pub use effects::{EffectKind, GeneratorSource, Levels, MaskBlend, MaskEffect, NoiseKind, NoiseParams, StylizeParams, WarpMode, WarpParams};
