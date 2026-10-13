//! Core data model for Stylized3DPaint: meshes, import and camera math.

pub mod bake;
pub mod bvh;
pub mod camera;
pub mod document;
pub mod effect_def;
pub mod legacy;
pub mod import;
pub mod mesh;
pub mod seams;
pub mod select;

pub use bake::{BakeSettings, BakeStyle, BakedMap};
pub use camera::OrbitCamera;
pub use document::{
    BlendMode, Blending, ChannelBlend, ChannelId, ChannelSet, ChannelValues, ColorLabel, Document, EffectInstance, Mask, MaskItem,
    MaskItemKind, Node, NodeKind, ParamValue,
};
pub use effect_def::{EffectDef, ParamDef, ParamKind, registry};
pub use mesh::{Aabb, Mesh, MeshPart, Vertex};
