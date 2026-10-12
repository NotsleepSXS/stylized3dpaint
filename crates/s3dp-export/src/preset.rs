//! Export presets (spec §9): JSON-serializable output texture layouts + built-ins.

use std::path::Path;

use anyhow::{Context, Result};
use s3dp_core::{BakedMap, ChannelId};
use serde::{Deserialize, Serialize};

use crate::FileFormat;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalFormat {
    /// +Y (Unity, Godot, glTF, Blender).
    #[default]
    OpenGL,
    /// -Y (Unreal, 3ds Max): green is inverted on export.
    DirectX,
}

/// What a single output component reads.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Source {
    /// Project channel component (0..3 = r,g,b,a; scalar channels live in r).
    Channel { channel: ChannelId, component: u8 },
    /// Baked mesh map component.
    Baked { map: BakedMap, component: u8 },
    /// Baked AO × AO channel.
    MixedAo,
    Constant { value: f32 },
    /// Final normal: project Normal channel RNM-combined over the baked normal (format-aware Y).
    NormalCombined { component: u8 },
    /// Spec/Gloss diffuse: BaseColor × (1 − Metallic).
    PbrDiffuse { component: u8 },
    /// Spec/Gloss specular: mix(0.04, BaseColor, Metallic).
    PbrSpecular { component: u8 },
    /// Unlit diffuse: BaseColor × mixed AO (× SkyLight) + Emissive.
    StylizedUnlit { component: u8, #[serde(default)] sky_light: bool },
}

impl Source {
    /// Holds color (sRGB-authored) data.
    pub fn is_color(&self) -> bool {
        match self {
            Source::Channel { channel, component } => channel.is_srgb() && *component < 3,
            Source::PbrDiffuse { .. } | Source::PbrSpecular { .. } | Source::StylizedUnlit { .. } => true,
            _ => false,
        }
    }
}

fn one() -> f32 {
    1.0
}

/// Source + per-slot adjustment: `v = invert ? 1-v : v; v = mix(min, max, v); v = v^gamma`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub source: Source,
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub remap_min: f32,
    #[serde(default = "one")]
    pub remap_max: f32,
    #[serde(default = "one")]
    pub gamma: f32,
}

impl Slot {
    pub fn new(source: Source) -> Self {
        Self { source, invert: false, remap_min: 0.0, remap_max: 1.0, gamma: 1.0 }
    }
    pub fn inverted(mut self) -> Self {
        self.invert = true;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputKind {
    Gray,
    #[default]
    Rgb,
    Rgba,
}

/// When an enabled output is actually written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputCondition {
    #[default]
    Always,
    /// Skip when every RGB value is (near) zero, e.g. emissive.
    NonBlack,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputTexture {
    /// `$suffix` / `$channel` in the naming template, e.g. "BaseColor".
    pub suffix: String,
    pub kind: OutputKind,
    /// Overrides the file format / bit depth from the settings.
    #[serde(default)]
    pub format: Option<FileFormat>,
    /// Encode RGB with linear→sRGB before quantization (alpha never).
    pub srgb: bool,
    pub slots: [Option<Slot>; 4],
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub condition: OutputCondition,
    /// `Rgba` output drops alpha unless the texture set uses opacity.
    #[serde(default)]
    pub alpha_only_if_opacity: bool,
}

fn yes() -> bool {
    true
}

impl OutputTexture {
    pub fn new(suffix: impl Into<String>, kind: OutputKind, srgb: bool, slots: [Option<Slot>; 4]) -> Self {
        Self {
            suffix: suffix.into(),
            kind,
            format: None,
            srgb,
            slots,
            enabled: true,
            condition: OutputCondition::Always,
            alpha_only_if_opacity: false,
        }
    }
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
    pub fn when_non_black(mut self) -> Self {
        self.condition = OutputCondition::NonBlack;
        self
    }
    pub fn alpha_if_opacity(mut self) -> Self {
        self.alpha_only_if_opacity = true;
        self
    }

    /// Number of data components written (before transparent padding adds alpha).
    pub fn component_count(&self, uses_opacity: bool) -> usize {
        match self.kind {
            OutputKind::Gray => 1,
            OutputKind::Rgb => 3,
            OutputKind::Rgba if self.alpha_only_if_opacity && !uses_opacity => 3,
            OutputKind::Rgba => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    #[serde(default)]
    pub name_ru: String,
    #[serde(default)]
    pub normal: NormalFormat,
    /// Preset default naming template (used when settings have none).
    #[serde(default)]
    pub naming: Option<String>,
    #[serde(default)]
    pub outputs: Vec<OutputTexture>,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub hint_ru: String,
}

pub fn load_preset(path: impl AsRef<Path>) -> Result<ExportPreset> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse preset {}", path.display()))
}

pub fn save_preset(path: impl AsRef<Path>, preset: &ExportPreset) -> Result<()> {
    let path = path.as_ref();
    let text = serde_json::to_string_pretty(preset)?;
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

// ------------------------------------------------------------------------------- built-ins

fn ch(c: ChannelId, comp: u8) -> Option<Slot> {
    Some(Slot::new(Source::Channel { channel: c, component: comp }))
}
fn src(s: Source) -> Option<Slot> {
    Some(Slot::new(s))
}
fn gray(s: Option<Slot>) -> [Option<Slot>; 4] {
    [s, None, None, None]
}
fn rgb_of(c: ChannelId) -> [Option<Slot>; 4] {
    [ch(c, 0), ch(c, 1), ch(c, 2), None]
}
fn derived(f: impl Fn(u8) -> Source) -> [Option<Slot>; 4] {
    [src(f(0)), src(f(1)), src(f(2)), None]
}

fn base_color(suffix: &str) -> OutputTexture {
    let mut s = rgb_of(ChannelId::BaseColor);
    s[3] = ch(ChannelId::Opacity, 0);
    OutputTexture::new(suffix, OutputKind::Rgba, true, s).alpha_if_opacity()
}
fn normal(suffix: &str) -> OutputTexture {
    OutputTexture::new(suffix, OutputKind::Rgb, false, derived(|c| Source::NormalCombined { component: c }))
}
fn emissive(suffix: &str) -> OutputTexture {
    OutputTexture::new(suffix, OutputKind::Rgb, true, rgb_of(ChannelId::Emissive)).when_non_black()
}
fn scalar(suffix: &str, c: ChannelId) -> OutputTexture {
    OutputTexture::new(suffix, OutputKind::Gray, false, gray(ch(c, 0)))
}
fn ao(suffix: &str) -> OutputTexture {
    OutputTexture::new(suffix, OutputKind::Gray, false, gray(src(Source::MixedAo)))
}
fn orm(suffix: &str) -> OutputTexture {
    OutputTexture::new(
        suffix,
        OutputKind::Rgb,
        false,
        [src(Source::MixedAo), ch(ChannelId::Roughness, 0), ch(ChannelId::Metallic, 0), None],
    )
}
fn smoothness() -> Option<Slot> {
    Some(Slot::new(Source::Channel { channel: ChannelId::Roughness, component: 0 }).inverted())
}

fn preset(
    name: &str,
    name_ru: &str,
    normal: NormalFormat,
    naming: Option<&str>,
    outputs: Vec<OutputTexture>,
    hint: &str,
    hint_ru: &str,
) -> ExportPreset {
    ExportPreset {
        name: name.into(),
        name_ru: name_ru.into(),
        normal,
        naming: naming.map(Into::into),
        outputs,
        hint: hint.into(),
        hint_ru: hint_ru.into(),
    }
}

/// Built-in presets; the first one is the default.
pub fn builtin_presets() -> Vec<ExportPreset> {
    use ChannelId::*;
    use NormalFormat::*;
    vec![
        preset(
            "Unreal Engine (Packed ORM)",
            "Unreal Engine (упакованный ORM)",
            DirectX,
            Some("T_$textureSet_$suffix"),
            vec![
                base_color("BaseColor"),
                normal("Normal"),
                orm("ORM"),
                emissive("Emissive"),
                scalar("Height", Height).disabled(),
            ],
            "ORM: sRGB off, Compression = Masks. Normal: Compression = Normalmap (DirectX, no flip). \
             ORM.R → Ambient Occlusion, G → Roughness, B → Metallic.",
            "ORM: выключить sRGB, Compression = Masks. Normal: Compression = Normalmap (DirectX, без \
             переворота). ORM.R → Ambient Occlusion, G → Roughness, B → Metallic.",
        ),
        preset(
            "Unreal Engine (separate)",
            "Unreal Engine (раздельные карты)",
            DirectX,
            Some("T_$textureSet_$suffix"),
            vec![
                base_color("BaseColor"),
                normal("Normal"),
                scalar("Roughness", Roughness),
                scalar("Metallic", Metallic),
                ao("AO"),
                emissive("Emissive"),
                scalar("Height", Height).disabled(),
            ],
            "Grayscale maps: sRGB off, Compression = Grayscale/Masks. Normal: Compression = Normalmap.",
            "Серые карты: выключить sRGB, Compression = Grayscale/Masks. Normal: Compression = Normalmap.",
        ),
        preset(
            "Unity URP / Built-in (Metallic)",
            "Unity URP / Built-in (Metallic)",
            OpenGL,
            None,
            vec![
                {
                    let mut s = rgb_of(BaseColor);
                    s[3] = ch(Opacity, 0);
                    OutputTexture::new("Albedo", OutputKind::Rgba, true, s)
                },
                OutputTexture::new(
                    "MetallicSmoothness",
                    OutputKind::Rgba,
                    false,
                    [ch(Metallic, 0), src(Source::Constant { value: 0.0 }), src(Source::Constant { value: 0.0 }), smoothness()],
                ),
                normal("Normal"),
                ao("Occlusion"),
                scalar("Height", Height),
                emissive("Emission"),
            ],
            "Lit shader, Workflow = Metallic, Smoothness Source = Metallic Alpha. MetallicSmoothness and \
             Occlusion: sRGB (Color Texture) off. Normal: Texture Type = Normal map.",
            "Шейдер Lit, Workflow = Metallic, Smoothness Source = Metallic Alpha. MetallicSmoothness и \
             Occlusion: снять галку sRGB (Color Texture). Normal: Texture Type = Normal map.",
        ),
        preset(
            "Unity HDRP",
            "Unity HDRP",
            OpenGL,
            None,
            vec![
                base_color("BaseColor"),
                OutputTexture::new(
                    "MaskMap",
                    OutputKind::Rgba,
                    false,
                    [ch(Metallic, 0), src(Source::MixedAo), src(Source::Constant { value: 1.0 }), smoothness()],
                ),
                normal("Normal"),
                scalar("Height", Height),
                emissive("Emissive"),
            ],
            "HDRP/Lit: MaskMap (R Metallic, G AO, B Detail mask, A Smoothness) with sRGB off. \
             Normal: Texture Type = Normal map.",
            "HDRP/Lit: MaskMap (R Metallic, G AO, B маска деталей, A Smoothness), sRGB выключить. \
             Normal: Texture Type = Normal map.",
        ),
        preset(
            "Godot 4 (ORM)",
            "Godot 4 (ORM)",
            OpenGL,
            None,
            vec![
                base_color("BaseColor"),
                orm("ORM"),
                normal("Normal"),
                emissive("Emission"),
                scalar("Height", Height).disabled(),
            ],
            "Use ORMMaterial3D: Albedo = BaseColor, ORM texture = ORM, Normal Map enabled \
             (import as Normal Map, OpenGL).",
            "Использовать ORMMaterial3D: Albedo = BaseColor, ORM = ORM, включить Normal Map \
             (импорт как Normal Map, OpenGL).",
        ),
        preset(
            "glTF 2.0 / Blender / Three.js",
            "glTF 2.0 / Blender / Three.js",
            OpenGL,
            None,
            vec![
                base_color("BaseColor"),
                orm("MetallicRoughness"),
                normal("Normal"),
                ao("Occlusion"),
                emissive("Emissive"),
            ],
            "glTF metallicRoughnessTexture reads G = Roughness, B = Metallic (R = AO, can be reused as \
             occlusionTexture). Blender: Non-Color for all but BaseColor/Emissive; Separate Color node.",
            "glTF metallicRoughnessTexture: G = Roughness, B = Metallic (R = AO, можно использовать как \
             occlusionTexture). Blender: Non-Color для всех, кроме BaseColor/Emissive; узел Separate Color.",
        ),
        preset(
            "Stylized Unlit",
            "Стилизованный Unlit",
            OpenGL,
            None,
            vec![{
                let mut s = derived(|c| Source::StylizedUnlit { component: c, sky_light: false });
                s[3] = ch(Opacity, 0);
                OutputTexture::new("Diffuse", OutputKind::Rgba, true, s).alpha_if_opacity()
            }],
            "Single lit-baked diffuse (BaseColor × AO + Emissive) for unlit shaders. Set sky_light = true \
             in the slot sources to also multiply by the baked Sky Light.",
            "Одна «запечённая» диффузная карта (BaseColor × AO + Emissive) для unlit-шейдеров. \
             sky_light = true в источниках слотов дополнительно умножает на запечённый Sky Light.",
        ),
        preset(
            "PBR Spec/Gloss (legacy)",
            "PBR Spec/Gloss (устаревший)",
            OpenGL,
            None,
            vec![
                OutputTexture::new("Diffuse", OutputKind::Rgb, true, derived(|c| Source::PbrDiffuse { component: c })),
                OutputTexture::new("Specular", OutputKind::Rgb, true, derived(|c| Source::PbrSpecular { component: c })),
                OutputTexture::new("Glossiness", OutputKind::Gray, false, gray(smoothness())),
                normal("Normal"),
                ao("AO"),
            ],
            "Specular workflow shaders: Diffuse and Specular are sRGB, Glossiness and AO linear. Flip the \
             normal green channel if the engine expects DirectX.",
            "Шейдеры Specular-workflow: Diffuse и Specular в sRGB, Glossiness и AO линейные. Если движок \
             ждёт DirectX-нормали — перевернуть зелёный канал.",
        ),
        preset(
            "Sketchfab",
            "Sketchfab",
            OpenGL,
            None,
            vec![
                base_color("BaseColor"),
                scalar("Metallic", Metallic),
                scalar("Roughness", Roughness),
                normal("Normal"),
                ao("AO"),
                emissive("Emissive"),
            ],
            "Upload with the model; in 3D Settings → Materials assign Base Color, Metalness, Roughness, \
             Normal Map, Ambient Occlusion, Emission (Base Color/Emission are sRGB).",
            "Загрузить вместе с моделью; в 3D Settings → Materials назначить Base Color, Metalness, \
             Roughness, Normal Map, Ambient Occlusion, Emission.",
        ),
        preset(
            "Custom",
            "Свой",
            OpenGL,
            None,
            vec![],
            "Empty preset: add outputs and route channels into R/G/B/A slots.",
            "Пустой пресет: добавьте текстуры и разведите каналы по слотам R/G/B/A.",
        ),
    ]
}

/// The default preset ("Unreal Engine (Packed ORM)").
pub fn default_preset() -> ExportPreset {
    builtin_presets().remove(0)
}
