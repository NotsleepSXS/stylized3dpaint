//! Core data model for Stylized3DPaint: meshes, import and camera math.

pub mod camera;
pub mod import;
pub mod mesh;

pub use camera::OrbitCamera;
pub use mesh::{Aabb, Mesh, MeshPart, Vertex};
