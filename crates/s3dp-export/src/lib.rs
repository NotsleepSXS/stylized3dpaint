//! Texture export (spec §9): pure-CPU packing of project channels + baked maps into engine
//! texture layouts, padding, resampling, color encoding, dithering and file writing.
//!
//! Inputs are LINEAR straight-alpha RGBA f32 buffers ([`ExportSource`]); a preset
//! ([`ExportPreset`]) routes sources into output texture slots; [`export`] writes files.

mod ops;
mod preset;
mod source;
mod write;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use s3dp_core::{BakedMap, ChannelId, document::linear_to_srgb};
use serde::{Deserialize, Serialize};

pub use preset::{
    ExportPreset, NormalFormat, OutputCondition, OutputKind, OutputTexture, Slot, Source, builtin_presets,
    default_preset, load_preset, save_preset,
};
pub use source::ExportSource;

use source::Sampler;

// ---------------------------------------------------------------------------------- settings

/// File format + bit depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileFormat {
    #[default]
    Png8,
    Png16,
    Tga8,
    Tiff8,
    Tiff16,
    /// OpenEXR half float (always linear).
    Exr16,
    /// OpenEXR 32-bit float (always linear).
    Exr32,
    /// Lossy, color only, no alpha.
    Jpg8,
}

impl FileFormat {
    pub const ALL: [FileFormat; 8] = [
        Self::Png8,
        Self::Png16,
        Self::Tga8,
        Self::Tiff8,
        Self::Tiff16,
        Self::Exr16,
        Self::Exr32,
        Self::Jpg8,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png8 | Self::Png16 => "png",
            Self::Tga8 => "tga",
            Self::Tiff8 | Self::Tiff16 => "tif",
            Self::Exr16 | Self::Exr32 => "exr",
            Self::Jpg8 => "jpg",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png8 => "PNG 8",
            Self::Png16 => "PNG 16",
            Self::Tga8 => "TGA 8",
            Self::Tiff8 => "TIFF 8",
            Self::Tiff16 => "TIFF 16",
            Self::Exr16 => "EXR 16f",
            Self::Exr32 => "EXR 32f",
            Self::Jpg8 => "JPG 8",
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::Exr16 | Self::Exr32)
    }

    pub fn is_8bit(self) -> bool {
        matches!(self, Self::Png8 | Self::Tga8 | Self::Tiff8 | Self::Jpg8)
    }
}

/// What happens to texels outside UV islands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Padding {
    /// Every empty texel takes the nearest island texel (no mip halos).
    #[default]
    Infinite,
    /// Dilate this many texels; the rest stays black.
    Pixels(u32),
    /// Infinite dilation of color, alpha 0 outside islands (alpha is added if needed).
    Transparent,
    /// Leave the background black.
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dither {
    None,
    /// 8×8 Bayer (exactly representable values stay exact).
    #[default]
    Ordered,
    /// Triangular-PDF noise, ±1 LSB.
    Triangular,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub output_dir: PathBuf,
    /// `$project $mesh $textureSet $channel/$suffix $udim $res`. `None` = preset default,
    /// else `$mesh_$textureSet_$suffix`.
    pub naming_template: Option<String>,
    pub project_name: String,
    pub mesh_name: String,
    pub format: FileFormat,
    pub padding: Padding,
    /// Texture set name → output [width, height].
    pub resolution_override: BTreeMap<String, [u32; 2]>,
    /// Applied to 8-bit outputs only (alpha excluded).
    pub dither: Dither,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("."),
            naming_template: None,
            project_name: "Project".into(),
            mesh_name: "Mesh".into(),
            format: FileFormat::Png8,
            padding: Padding::Infinite,
            resolution_override: BTreeMap::new(),
            dither: Dither::Ordered,
        }
    }
}

pub const DEFAULT_TEMPLATE: &str = "$mesh_$textureSet_$suffix";

impl ExportSettings {
    pub fn template<'a>(&'a self, preset: &'a ExportPreset) -> &'a str {
        self.naming_template.as_deref().or(preset.naming.as_deref()).unwrap_or(DEFAULT_TEMPLATE)
    }
}

// ---------------------------------------------------------------------------------- report

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningKind {
    EmptySlot,
    SrgbDataInLinear,
    LinearDataInSrgb,
    MissingChannel,
    MissingBakedMap,
    LossyFormat,
    AlphaDropped,
    DuplicateFileName,
    NoOutputs,
}

/// Non-blocking export warning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    pub kind: WarningKind,
    pub texture_set: Option<String>,
    pub output: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportedFile {
    pub path: PathBuf,
    pub texture_set: String,
    pub suffix: String,
    pub width: u32,
    pub height: u32,
    pub format: FileFormat,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExportReport {
    pub files: Vec<ExportedFile>,
    /// (texture set, suffix, reason) of enabled outputs that were not written.
    pub skipped: Vec<(String, String, String)>,
    pub warnings: Vec<Warning>,
}

// ---------------------------------------------------------------------------------- render

/// Rendered output: encoded (sRGB applied when flagged, not for EXR), padded, resampled
/// float values, `channels` components per texel (1 = L, 2 = LA, 3 = RGB, 4 = RGBA).
#[derive(Clone, Debug)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub data: Vec<f32>,
}

impl RenderedImage {
    /// 8-bit RGBA for previews (no dithering).
    pub fn to_rgba8(&self) -> Vec<u8> {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut out = Vec::with_capacity(self.data.len() / self.channels * 4);
        for px in self.data.chunks_exact(self.channels) {
            let (rgb, a) = match self.channels {
                1 => ([px[0]; 3], 1.0),
                2 => ([px[0]; 3], px[1]),
                3 => ([px[0], px[1], px[2]], 1.0),
                _ => ([px[0], px[1], px[2]], px[3]),
            };
            out.extend_from_slice(&[q(rgb[0]), q(rgb[1]), q(rgb[2]), q(a)]);
        }
        out
    }

    pub fn has_alpha(&self) -> bool {
        self.channels == 2 || self.channels == 4
    }

    /// Any RGB value above half an 8-bit step.
    fn is_non_black(&self) -> bool {
        let n = if self.has_alpha() { self.channels - 1 } else { self.channels };
        self.data.chunks_exact(self.channels).any(|px| px[..n].iter().any(|v| v.abs() > 0.5 / 255.0))
    }
}

fn effective_format(output: &OutputTexture, settings: &ExportSettings) -> FileFormat {
    output.format.unwrap_or(settings.format)
}

fn padding_map(source: &ExportSource, padding: Padding) -> Option<Vec<u32>> {
    let (w, h) = (source.width as usize, source.height as usize);
    match padding {
        Padding::None => None,
        _ if source.coverage.is_empty() => None,
        Padding::Infinite | Padding::Transparent => Some(ops::nearest_map(w, h, &source.coverage, None)),
        Padding::Pixels(n) => Some(ops::nearest_map(w, h, &source.coverage, Some(n))),
    }
}

/// Renders one output texture of one texture set (for previews or custom writers).
pub fn render_output(
    source: &ExportSource,
    preset: &ExportPreset,
    output: &OutputTexture,
    settings: &ExportSettings,
) -> RenderedImage {
    let near = padding_map(source, settings.padding);
    render_with(source, preset, output, settings, near.as_deref())
}

fn render_with(
    source: &ExportSource,
    preset: &ExportPreset,
    output: &OutputTexture,
    settings: &ExportSettings,
    near: Option<&[u32]>,
) -> RenderedImage {
    let (w, h) = (source.width as usize, source.height as usize);
    let ncomp = output.component_count(source.uses_opacity);
    let transparent = settings.padding == Padding::Transparent;
    let add_alpha = transparent && (ncomp == 1 || ncomp == 3);
    let nch = if add_alpha { ncomp + 1 } else { ncomp };
    let sampler = Sampler::new(source, preset.normal);

    let mut data = vec![0.0f32; w * h * nch];
    ops::par_rows(&mut data, w * nch, |y, row| {
        for x in 0..w {
            let i = y * w + x;
            if !source.covered(i) {
                continue;
            }
            let px = &mut row[x * nch..(x + 1) * nch];
            for (c, v) in px.iter_mut().enumerate().take(ncomp) {
                *v = match &output.slots[c] {
                    Some(s) => sampler.eval_slot(s, i),
                    None if c == 3 => 1.0,
                    None => 0.0,
                };
            }
            if add_alpha {
                px[nch - 1] = 1.0;
            }
        }
    });

    // Padding: copy the nearest island texel.
    if let Some(near) = near {
        for (i, &j) in near.iter().enumerate() {
            if j != ops::NONE && j as usize != i {
                let j = j as usize;
                data.copy_within(j * nch..(j + 1) * nch, i * nch);
            }
        }
    }
    if transparent && !source.coverage.is_empty() {
        for i in 0..w * h {
            if !source.covered(i) {
                data[i * nch + nch - 1] = 0.0;
            }
        }
    }

    // Resolution override.
    let [nw, nh] = settings
        .resolution_override
        .get(&source.name)
        .copied()
        .filter(|r| r[0] > 0 && r[1] > 0)
        .unwrap_or([source.width, source.height]);
    if (nw, nh) != (source.width, source.height) {
        data = ops::resample(&data, w, h, nch, nw as usize, nh as usize);
    }

    // Color encoding (EXR stays linear).
    if output.srgb && !effective_format(output, settings).is_float() {
        let ncolor = if nch == 2 || nch == 4 { nch - 1 } else { nch };
        ops::par_rows(&mut data, nw as usize * nch, |_, row| {
            for px in row.chunks_exact_mut(nch) {
                for v in &mut px[..ncolor] {
                    *v = linear_to_srgb(v.clamp(0.0, 1.0));
                }
            }
        });
    }

    RenderedImage { width: nw, height: nh, channels: nch, data }
}

// ---------------------------------------------------------------------------------- naming

/// Expands the naming template (without extension).
pub fn expand_name(template: &str, settings: &ExportSettings, set: &str, suffix: &str, res: [u32; 2], udim: u32) -> String {
    let res_s = if res[0] == res[1] { res[0].to_string() } else { format!("{}x{}", res[0], res[1]) };
    let s = template
        .replace("$textureSet", set)
        .replace("$project", &settings.project_name)
        .replace("$mesh", &settings.mesh_name)
        .replace("$channel", suffix)
        .replace("$suffix", suffix)
        .replace("$udim", &udim.to_string())
        .replace("$res", &res_s);
    s.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect()
}

fn output_res(source: &ExportSource, settings: &ExportSettings) -> [u32; 2] {
    settings
        .resolution_override
        .get(&source.name)
        .copied()
        .filter(|r| r[0] > 0 && r[1] > 0)
        .unwrap_or([source.width, source.height])
}

/// File path an output of a texture set would be written to.
pub fn output_path(
    source: &ExportSource,
    index: usize,
    preset: &ExportPreset,
    output: &OutputTexture,
    settings: &ExportSettings,
) -> PathBuf {
    let name = expand_name(
        settings.template(preset),
        settings,
        &source.name,
        &output.suffix,
        output_res(source, settings),
        1001 + index as u32,
    );
    settings.output_dir.join(format!("{name}.{}", effective_format(output, settings).extension()))
}

// ---------------------------------------------------------------------------------- validate

/// Non-blocking checks: empty slots, color-space mismatches, missing data, lossy formats,
/// file name collisions.
pub fn validate(sources: &[ExportSource], preset: &ExportPreset, settings: &ExportSettings) -> Vec<Warning> {
    let mut out = Vec::new();
    let enabled: Vec<&OutputTexture> = preset.outputs.iter().filter(|o| o.enabled).collect();
    if enabled.is_empty() {
        out.push(Warning {
            kind: WarningKind::NoOutputs,
            texture_set: None,
            output: None,
            message: format!("Preset '{}' has no enabled outputs", preset.name),
        });
    }
    let warn = |out: &mut Vec<Warning>, kind, set: Option<&str>, o: &OutputTexture, message: String| {
        out.push(Warning { kind, texture_set: set.map(Into::into), output: Some(o.suffix.clone()), message })
    };

    // Preset-level checks.
    for o in &enabled {
        let full = match o.kind {
            OutputKind::Gray => 1,
            OutputKind::Rgb => 3,
            OutputKind::Rgba => 4,
        };
        for (c, slot) in o.slots.iter().enumerate().take(full) {
            let comp = ["R", "G", "B", "A"][c];
            let Some(slot) = slot else {
                warn(&mut out, WarningKind::EmptySlot, None, o, format!("{}: slot {comp} is empty", o.suffix));
                continue;
            };
            if c < 3 && !o.srgb && slot.source.is_color() {
                warn(
                    &mut out,
                    WarningKind::SrgbDataInLinear,
                    None,
                    o,
                    format!("{}: color data in {comp} of a linear (non-sRGB) texture", o.suffix),
                );
            }
            if c < 3 && o.srgb && !slot.source.is_color() && !matches!(slot.source, Source::Constant { .. }) {
                warn(
                    &mut out,
                    WarningKind::LinearDataInSrgb,
                    None,
                    o,
                    format!("{}: non-color data in {comp} of an sRGB texture", o.suffix),
                );
            }
        }
        let fmt = effective_format(o, settings);
        if fmt == FileFormat::Jpg8 {
            let color = o.srgb && o.kind != OutputKind::Gray;
            warn(
                &mut out,
                WarningKind::LossyFormat,
                None,
                o,
                if color {
                    format!("{}: JPEG is lossy (8-bit)", o.suffix)
                } else {
                    format!("{}: JPEG is lossy — not suitable for data/mask textures", o.suffix)
                },
            );
            if o.kind == OutputKind::Rgba || settings.padding == Padding::Transparent {
                warn(&mut out, WarningKind::AlphaDropped, None, o, format!("{}: JPEG has no alpha; alpha dropped", o.suffix));
            }
        }
    }

    // Per texture set.
    for (si, s) in sources.iter().enumerate() {
        let mut missing_c: HashSet<ChannelId> = HashSet::new();
        let mut missing_b: HashSet<BakedMap> = HashSet::new();
        let mut names = HashSet::new();
        for o in &enabled {
            let ncomp = o.component_count(s.uses_opacity);
            for (c, slot) in o.slots.iter().enumerate() {
                let used = match o.kind {
                    OutputKind::Gray => c == 0,
                    OutputKind::Rgb => c < 3,
                    OutputKind::Rgba => c < 3 || ncomp == 4,
                };
                let Some(slot) = slot.filter(|_| used) else { continue };
                match slot.source {
                    Source::Channel { channel, .. } if !s.channels.contains_key(&channel) => {
                        missing_c.insert(channel);
                    }
                    Source::Baked { map, .. } if !s.baked.contains_key(&map) => {
                        missing_b.insert(map);
                    }
                    Source::MixedAo if !s.baked.contains_key(&BakedMap::AmbientOcclusion) => {
                        missing_b.insert(BakedMap::AmbientOcclusion);
                    }
                    Source::StylizedUnlit { sky_light: true, .. } if !s.baked.contains_key(&BakedMap::SkyLight) => {
                        missing_b.insert(BakedMap::SkyLight);
                    }
                    _ => {}
                }
            }
            let p = output_path(s, si, preset, o, settings);
            if !names.insert(p.clone()) {
                warn(
                    &mut out,
                    WarningKind::DuplicateFileName,
                    Some(&s.name),
                    o,
                    format!("{}: file name {} is used twice", o.suffix, p.display()),
                );
            }
        }
        let mut mc: Vec<_> = missing_c.into_iter().collect();
        mc.sort();
        for c in mc {
            out.push(Warning {
                kind: WarningKind::MissingChannel,
                texture_set: Some(s.name.clone()),
                output: None,
                message: format!("{}: channel {} is missing, default value used", s.name, c.label()),
            });
        }
        let mut mb: Vec<_> = missing_b.into_iter().collect();
        mb.sort_by_key(|m| m.index());
        for m in mb {
            out.push(Warning {
                kind: WarningKind::MissingBakedMap,
                texture_set: Some(s.name.clone()),
                output: None,
                message: format!("{}: baked map {} is missing, default value used", s.name, m.label()),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------- export

/// Renders and writes every enabled output of every texture set.
pub fn export(sources: &[ExportSource], preset: &ExportPreset, settings: &ExportSettings) -> Result<ExportReport> {
    std::fs::create_dir_all(&settings.output_dir)
        .with_context(|| format!("create {}", settings.output_dir.display()))?;
    let mut report = ExportReport { warnings: validate(sources, preset, settings), ..Default::default() };
    for (si, s) in sources.iter().enumerate() {
        anyhow::ensure!(
            s.coverage.is_empty() || s.coverage.len() == s.texel_count(),
            "texture set {}: coverage size mismatch",
            s.name
        );
        let near = padding_map(s, settings.padding);
        for o in preset.outputs.iter().filter(|o| o.enabled) {
            let img = render_with(s, preset, o, settings, near.as_deref());
            if o.condition == OutputCondition::NonBlack && !img.is_non_black() {
                report.skipped.push((s.name.clone(), o.suffix.clone(), "black".into()));
                continue;
            }
            let fmt = effective_format(o, settings);
            let path = output_path(s, si, preset, o, settings);
            write::write_image(&path, &img, fmt, settings.dither)
                .with_context(|| format!("write {}", path.display()))?;
            report.files.push(ExportedFile {
                path,
                texture_set: s.name.clone(),
                suffix: o.suffix.clone(),
                width: img.width,
                height: img.height,
                format: fmt,
            });
        }
    }
    Ok(report)
}

#[doc(hidden)]
pub mod __bench {
    /// Exposes the dilation map for benchmarks/tests.
    pub fn nearest_map(w: usize, h: usize, coverage: &[bool]) -> Vec<u32> {
        crate::ops::nearest_map(w, h, coverage, None)
    }
}
