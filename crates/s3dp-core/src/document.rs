//! Layer stack description (CPU side, serializable). GPU textures live in `s3dp-render`.

use serde::{Deserialize, Serialize};

/// Material channels painted on every texture set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Channel {
    BaseColor,
    Roughness,
    Metallic,
    Height,
    Emissive,
}

impl Channel {
    pub const ALL: [Channel; 5] = [Self::BaseColor, Self::Roughness, Self::Metallic, Self::Height, Self::Emissive];
    pub const COUNT: usize = 5;

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BaseColor => "Base Color",
            Self::Roughness => "Roughness",
            Self::Metallic => "Metallic",
            Self::Height => "Height",
            Self::Emissive => "Emissive",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::BaseColor => "BC",
            Self::Roughness => "R",
            Self::Metallic => "M",
            Self::Height => "H",
            Self::Emissive => "E",
        }
    }

    /// Color channels store RGB; the rest store a single value.
    pub fn is_color(self) -> bool {
        matches!(self, Self::BaseColor | Self::Emissive)
    }
}

/// Photoshop blend modes. Formulas operate on sRGB-encoded values like Photoshop does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    Overlay,
    SoftLight,
    HardLight,
    Difference,
    Subtract,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// Grouped like Photoshop's menu; `None` marks a separator.
    pub const MENU: [Option<BlendMode>; 22] = [
        Some(Self::Normal),
        None,
        Some(Self::Darken),
        Some(Self::Multiply),
        Some(Self::ColorBurn),
        Some(Self::LinearBurn),
        None,
        Some(Self::Lighten),
        Some(Self::Screen),
        Some(Self::ColorDodge),
        Some(Self::LinearDodge),
        None,
        Some(Self::Overlay),
        Some(Self::SoftLight),
        Some(Self::HardLight),
        None,
        Some(Self::Difference),
        Some(Self::Subtract),
        Some(Self::Hue),
        Some(Self::Saturation),
        Some(Self::Color),
        Some(Self::Luminosity),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Darken => "Darken",
            Self::Multiply => "Multiply",
            Self::ColorBurn => "Color Burn",
            Self::LinearBurn => "Linear Burn",
            Self::Lighten => "Lighten",
            Self::Screen => "Screen",
            Self::ColorDodge => "Color Dodge",
            Self::LinearDodge => "Linear Dodge (Add)",
            Self::Overlay => "Overlay",
            Self::SoftLight => "Soft Light",
            Self::HardLight => "Hard Light",
            Self::Difference => "Difference",
            Self::Subtract => "Subtract",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Color => "Color",
            Self::Luminosity => "Luminosity",
        }
    }

    /// Index passed to the GPU blend shader.
    pub fn gpu_id(self) -> u32 {
        self as u32
    }
}

/// One value per channel (fill layers, brush values, document defaults).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelValues {
    /// sRGB-encoded.
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub height: f32,
    /// sRGB-encoded.
    pub emissive: [f32; 3],
}

impl ChannelValues {
    /// A light neutral gray: painted colors read well on it.
    pub const DEFAULTS: ChannelValues = ChannelValues {
        base_color: [0.75, 0.75, 0.75],
        roughness: 0.7,
        metallic: 0.0,
        height: 0.5,
        emissive: [0.0, 0.0, 0.0],
    };

    /// Value as RGB (scalars are replicated).
    pub fn get(&self, c: Channel) -> [f32; 3] {
        match c {
            Channel::BaseColor => self.base_color,
            Channel::Roughness => [self.roughness; 3],
            Channel::Metallic => [self.metallic; 3],
            Channel::Height => [self.height; 3],
            Channel::Emissive => self.emissive,
        }
    }
}

impl Default for ChannelValues {
    fn default() -> Self {
        Self::DEFAULTS
    }
}

/// Which channels a layer (or the brush) affects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelMask(pub [bool; Channel::COUNT]);

impl ChannelMask {
    pub const ALL: ChannelMask = ChannelMask([true; Channel::COUNT]);
    pub const BASE_COLOR: ChannelMask = ChannelMask([true, false, false, false, false]);

    pub fn has(&self, c: Channel) -> bool {
        self.0[c.index()]
    }

    pub fn set(&mut self, c: Channel, on: bool) {
        self.0[c.index()] = on;
    }
}

impl Default for ChannelMask {
    fn default() -> Self {
        Self::ALL
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayerKind {
    /// Pixels painted with the brush.
    Paint,
    /// Uniform values everywhere (shaped by the mask).
    Fill(ChannelValues),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub kind: LayerKind,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    /// Channels this layer contributes to.
    pub channels: ChannelMask,
    pub has_mask: bool,
    pub mask_enabled: bool,
}

impl Layer {
    pub fn new(id: u64, name: impl Into<String>, kind: LayerKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            channels: ChannelMask::ALL,
            has_mask: false,
            mask_enabled: true,
        }
    }

    pub fn is_fill(&self) -> bool {
        matches!(self.kind, LayerKind::Fill(_))
    }
}
