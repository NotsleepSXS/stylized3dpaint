//! Core data model for Stylized3DPaint: meshes, import and camera math.

pub mod bake;
pub mod bvh;
pub mod camera;
pub mod document;
pub mod effects;
pub mod import;
pub mod mesh;

pub use bake::{BakeSettings, BakeStyle, BakedMap};
pub use camera::OrbitCamera;
pub use effects::{EffectKind, GeneratorSource, Levels, MaskBlend, MaskEffect, NoiseKind, NoiseParams, StylizeParams, WarpMode, WarpParams};
pub use document::{BlendMode, Channel, ChannelMask, ChannelValues, Layer, LayerKind};
pub use mesh::{Aabb, Mesh, MeshPart, Vertex};
