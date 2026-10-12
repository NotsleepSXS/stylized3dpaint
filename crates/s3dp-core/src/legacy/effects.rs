//! Mask effects (generators + filters) and the Stylize filter layer.
//!
//! A layer's effective mask = painted mask (or white) processed by its effect chain.
//! Generators read baked mesh maps; noise is evaluated in 3D world space so it is
//! continuous across UV seams.

use serde::{Deserialize, Serialize};

use crate::BakedMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseKind {
    /// Smooth fractal gradient noise.
    #[default]
    Perlin,
    /// Cell / stone pattern (Worley F2 - F1).
    Cellular,
    /// Gradient noise stretched along the surface: brush-stroke-like streaks.
    Strokes,
}

impl NoiseKind {
    pub const ALL: [NoiseKind; 3] = [Self::Perlin, Self::Cellular, Self::Strokes];
    pub fn label(self) -> &'static str {
        match self {
            Self::Perlin => "Perlin",
            Self::Cellular => "Cellular",
            Self::Strokes => "Strokes",
        }
    }
    pub fn gpu_id(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoiseParams {
    pub kind: NoiseKind,
    /// Feature size relative to the mesh radius.
    pub scale: f32,
    pub octaves: u32,
    /// Strokes: how much the noise is stretched along the surface (1 = off, round).
    pub stretch: f32,
    pub seed: u32,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self { kind: NoiseKind::Perlin, scale: 0.08, octaves: 4, stretch: 1.0, seed: 1 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarpMode {
    /// Displace the mask along the noise field (torn, flowing borders).
    #[default]
    Warp,
    /// Drag the mask along the noise flow over several steps (painterly smears).
    SlopeBlur,
    /// Smear in one world direction with noise-modulated length.
    Directional,
}

impl WarpMode {
    pub const ALL: [WarpMode; 3] = [Self::Warp, Self::SlopeBlur, Self::Directional];
    pub fn label(self) -> &'static str {
        match self {
            Self::Warp => "Warp",
            Self::SlopeBlur => "Slope Blur",
            Self::Directional => "Directional",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarpParams {
    pub mode: WarpMode,
    pub noise: NoiseParams,
    /// Displacement distance relative to the mesh radius.
    pub intensity: f32,
    /// Steps for slope blur / directional.
    pub samples: u32,
    /// Slope blur: 0 = average (soft smear), 1 = max (bold strokes).
    pub sharpness: f32,
    /// Directional: world direction.
    pub direction: [f32; 3],
}

impl Default for WarpParams {
    fn default() -> Self {
        Self {
            mode: WarpMode::Warp,
            noise: NoiseParams::default(),
            intensity: 0.02,
            samples: 12,
            sharpness: 0.5,
            direction: [0.0, -1.0, 0.0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    pub low: f32,
    pub high: f32,
    pub gamma: f32,
    /// 0 = continuous, otherwise number of flat bands.
    pub steps: u32,
    pub invert: bool,
}

impl Default for Levels {
    fn default() -> Self {
        Self { low: 0.0, high: 1.0, gamma: 1.0, steps: 0, invert: false }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskBlend {
    Replace,
    #[default]
    Multiply,
    Add,
    Subtract,
    Max,
    Min,
}

impl MaskBlend {
    pub const ALL: [MaskBlend; 6] = [Self::Replace, Self::Multiply, Self::Add, Self::Subtract, Self::Max, Self::Min];
    pub fn label(self) -> &'static str {
        match self {
            Self::Replace => "Replace",
            Self::Multiply => "Multiply",
            Self::Add => "Add",
            Self::Subtract => "Subtract",
            Self::Max => "Lighten (max)",
            Self::Min => "Darken (min)",
        }
    }
    pub fn gpu_id(self) -> u32 {
        self as u32
    }
}

/// What a generator reads.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GeneratorSource {
    Map(BakedMap),
    /// Position along a world axis inside the bounding box (0 = min, 1 = max).
    HeightGradient { axis: u32 },
    Noise(NoiseParams),
}

impl GeneratorSource {
    pub fn label(&self) -> String {
        match self {
            Self::Map(m) => m.label().to_owned(),
            Self::HeightGradient { axis } => format!("Gradient {}", ["X", "Y", "Z"][(*axis as usize).min(2)]),
            Self::Noise(n) => format!("{} noise", n.kind.label()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EffectKind {
    Generator { source: GeneratorSource, levels: Levels, blend: MaskBlend, opacity: f32 },
    Warp(WarpParams),
    /// Blur radius in texels.
    Blur { radius: f32 },
    Levels(Levels),
}

impl EffectKind {
    pub fn label(&self) -> String {
        match self {
            Self::Generator { source, .. } => source.label(),
            Self::Warp(w) => w.mode.label().to_owned(),
            Self::Blur { .. } => "Blur".into(),
            Self::Levels(_) => "Levels".into(),
        }
    }

    pub fn generator(source: GeneratorSource) -> Self {
        Self::Generator { source, levels: Levels::default(), blend: MaskBlend::Multiply, opacity: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskEffect {
    pub enabled: bool,
    pub kind: EffectKind,
}

impl MaskEffect {
    pub fn new(kind: EffectKind) -> Self {
        Self { enabled: true, kind }
    }
}

/// Parameters of the Stylize filter layer (applies to everything below it).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StylizeParams {
    /// Painterly simplification radius (texels, Kuwahara filter).
    pub painterly: f32,
    /// Number of value bands for color (0 = off).
    pub color_bands: u32,
    pub band_strength: f32,
    pub saturation: f32,
    /// Warm lights / cool shadows from the baked sky light & AO.
    pub temperature: f32,
    pub edge_highlight: f32,
    pub cavity_darken: f32,
    /// Roughness simplification: bands (0 = off) and edge polish.
    pub roughness_bands: u32,
    pub edge_polish: f32,
    /// Metallic becomes crisp around this threshold.
    pub metal_threshold: f32,
    /// Height: rounded bevel on edges and smoothing.
    pub height_bevel: f32,
    pub height_smooth: f32,
}

impl Default for StylizeParams {
    fn default() -> Self {
        Self {
            painterly: 4.0,
            color_bands: 4,
            band_strength: 0.5,
            saturation: 1.15,
            temperature: 0.35,
            edge_highlight: 0.35,
            cavity_darken: 0.4,
            roughness_bands: 3,
            edge_polish: 0.3,
            metal_threshold: 0.5,
            height_bevel: 0.3,
            height_smooth: 3.0,
        }
    }
}
