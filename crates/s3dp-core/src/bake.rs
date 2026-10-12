//! Mesh map baking settings and the list of baked maps.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Maps produced by the baker, per texture set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BakedMap {
    /// Ambient occlusion: 1 = open, 0 = occluded.
    AmbientOcclusion,
    /// 0.5 = flat, > 0.5 convex (edges), < 0.5 concave (cavities).
    Curvature,
    /// Convex edge mask (stylized edge highlights).
    Edges,
    /// Concave cavity mask (dirt, dark creases).
    Cavity,
    /// 0 = thin, 1 = thick.
    Thickness,
    /// Soft light from above with soft shadows (hand-painted lighting).
    SkyLight,
    /// Tangent-space normal map (flat without a high-poly source).
    Normal,
    /// World-space normal, encoded 0..1.
    WorldNormal,
    /// Position normalized to the mesh bounding box, 0..1.
    Position,
}

impl BakedMap {
    pub const ALL: [BakedMap; 9] = [
        Self::AmbientOcclusion,
        Self::Curvature,
        Self::Edges,
        Self::Cavity,
        Self::Thickness,
        Self::SkyLight,
        Self::Normal,
        Self::WorldNormal,
        Self::Position,
    ];
    pub const COUNT: usize = 9;

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AmbientOcclusion => "Ambient Occlusion",
            Self::Curvature => "Curvature",
            Self::Edges => "Edges",
            Self::Cavity => "Cavity",
            Self::Thickness => "Thickness",
            Self::SkyLight => "Sky Light",
            Self::Normal => "Normal",
            Self::WorldNormal => "World Normal",
            Self::Position => "Position",
        }
    }

    /// File-name friendly name.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::AmbientOcclusion => "ambient_occlusion",
            Self::Curvature => "curvature",
            Self::Edges => "edges",
            Self::Cavity => "cavity",
            Self::Thickness => "thickness",
            Self::SkyLight => "sky_light",
            Self::Normal => "normal",
            Self::WorldNormal => "world_normal",
            Self::Position => "position",
        }
    }

    /// RGB maps; the others are single channel.
    pub fn is_color(self) -> bool {
        matches!(self, Self::Normal | Self::WorldNormal | Self::Position)
    }

    /// Value of an unbaked map (rgb for color maps).
    pub fn default_value(self) -> [f32; 3] {
        match self {
            Self::AmbientOcclusion | Self::SkyLight => [1.0; 3],
            Self::Curvature | Self::Thickness => [0.5; 3],
            Self::Edges | Self::Cavity => [0.0; 3],
            Self::Normal => [0.5, 0.5, 1.0],
            Self::WorldNormal => [0.5, 1.0, 0.5],
            Self::Position => [0.5; 3],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BakeStyle {
    /// Physically plausible maps (sharp AO contact shadows, thin curvature).
    Realistic,
    /// Soft, wide, painterly maps that layer well for hand-painted looks.
    #[default]
    Stylized,
}

/// Distances are relative to the mesh bounding-sphere radius, so presets work at any scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BakeSettings {
    pub style: BakeStyle,
    /// Detail source; `None` bakes from the low-poly mesh itself.
    pub high_poly: Option<PathBuf>,
    /// How far in front of / behind the low-poly surface to search the high-poly.
    pub cage_front: f32,
    pub cage_back: f32,
    pub ao_samples: u32,
    pub ao_distance: f32,
    /// Width of edge / cavity detection.
    pub edge_width: f32,
    pub edge_samples: u32,
    pub thickness_distance: f32,
    pub thickness_samples: u32,
    pub sky_samples: u32,
    /// Light direction (towards the light), world space.
    pub sky_direction: [f32; 3],
    /// Cone half-angle in degrees: bigger = softer shadows.
    pub sky_softness: f32,
    /// Texture-space smoothing radius (relative), 0 = off. Removes noise and faceting.
    pub smoothing: f32,
}

impl BakeSettings {
    pub fn preset(style: BakeStyle) -> Self {
        match style {
            BakeStyle::Realistic => Self {
                style,
                high_poly: None,
                cage_front: 0.02,
                cage_back: 0.02,
                ao_samples: 128,
                ao_distance: 0.15,
                edge_width: 0.008,
                edge_samples: 24,
                thickness_distance: 0.4,
                thickness_samples: 48,
                sky_samples: 64,
                sky_direction: [0.3, 1.0, 0.4],
                sky_softness: 25.0,
                smoothing: 0.0,
            },
            BakeStyle::Stylized => Self {
                style,
                high_poly: None,
                cage_front: 0.02,
                cage_back: 0.02,
                ao_samples: 128,
                ao_distance: 0.35,
                edge_width: 0.025,
                edge_samples: 32,
                thickness_distance: 0.5,
                thickness_samples: 48,
                sky_samples: 96,
                sky_direction: [0.3, 1.0, 0.4],
                sky_softness: 45.0,
                smoothing: 0.02,
            },
        }
    }

    /// Switches style, keeping the high-poly source.
    pub fn with_style(&self, style: BakeStyle) -> Self {
        Self { high_poly: self.high_poly.clone(), ..Self::preset(style) }
    }
}

impl Default for BakeSettings {
    fn default() -> Self {
        Self::preset(BakeStyle::default())
    }
}
