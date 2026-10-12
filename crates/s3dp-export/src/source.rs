//! Export input: per-texture-set linear buffers + per-texel source evaluation.

use std::collections::HashMap;

use s3dp_core::{BakedMap, ChannelId, ChannelValues};

use crate::preset::{NormalFormat, Slot, Source};

/// One texture set prepared for export. All buffers are straight-alpha, LINEAR, RGBA f32
/// per texel (`width * height * 4`); scalar channels / maps keep their value in R.
/// The project `Normal` channel and the baked `Normal` map are tangent-space normals encoded
/// 0..1 (OpenGL, +Y).
#[derive(Clone, Debug, Default)]
pub struct ExportSource {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub channels: HashMap<ChannelId, Vec<f32>>,
    pub baked: HashMap<BakedMap, Vec<f32>>,
    /// Texel lies inside a UV island. Empty = everything is covered.
    pub coverage: Vec<bool>,
    /// The project uses the Opacity channel (adds alpha to outputs flagged
    /// `alpha_only_if_opacity`). See [`ExportSource::detect_uses_opacity`].
    pub uses_opacity: bool,
}

impl ExportSource {
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self { name: name.into(), width, height, ..Default::default() }
    }

    pub fn texel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn with_channel(mut self, c: ChannelId, rgba: Vec<f32>) -> Self {
        assert_eq!(rgba.len(), self.texel_count() * 4, "channel buffer size");
        self.channels.insert(c, rgba);
        self
    }

    pub fn with_baked(mut self, m: BakedMap, rgba: Vec<f32>) -> Self {
        assert_eq!(rgba.len(), self.texel_count() * 4, "baked buffer size");
        self.baked.insert(m, rgba);
        self
    }

    /// Constant-filled channel (handy for tests / fallbacks).
    pub fn fill_channel(self, c: ChannelId, v: [f32; 4]) -> Self {
        let buf = v.repeat(self.texel_count());
        self.with_channel(c, buf)
    }

    pub fn fill_baked(self, m: BakedMap, v: [f32; 4]) -> Self {
        let buf = v.repeat(self.texel_count());
        self.with_baked(m, buf)
    }

    pub fn with_coverage(mut self, coverage: Vec<bool>) -> Self {
        assert!(coverage.is_empty() || coverage.len() == self.texel_count(), "coverage size");
        self.coverage = coverage;
        self
    }

    pub fn covered(&self, i: usize) -> bool {
        self.coverage.is_empty() || self.coverage[i]
    }

    /// True when the Opacity channel is below 1 anywhere inside the islands.
    pub fn detect_uses_opacity(&self) -> bool {
        let Some(buf) = self.channels.get(&ChannelId::Opacity) else { return false };
        (0..self.texel_count()).any(|i| self.covered(i) && buf[i * 4] < 0.999)
    }
}

/// Evaluates [`Source`]s for one texel.
pub(crate) struct Sampler<'a> {
    src: &'a ExportSource,
    defaults: ChannelValues,
    normal: NormalFormat,
}

impl<'a> Sampler<'a> {
    pub fn new(src: &'a ExportSource, normal: NormalFormat) -> Self {
        Self { src, defaults: ChannelValues::defaults(), normal }
    }

    fn channel(&self, c: ChannelId, i: usize) -> [f32; 4] {
        match self.src.channels.get(&c) {
            Some(b) => [b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]],
            None => {
                let d = self.defaults.get(c);
                [d[0], d[1], d[2], 1.0]
            }
        }
    }

    fn baked(&self, m: BakedMap, i: usize) -> [f32; 4] {
        match self.src.baked.get(&m) {
            Some(b) => [b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]],
            None => {
                let d = m.default_value();
                [d[0], d[1], d[2], 1.0]
            }
        }
    }

    fn mixed_ao(&self, i: usize) -> f32 {
        self.baked(BakedMap::AmbientOcclusion, i)[0] * self.channel(ChannelId::AmbientOcclusion, i)[0]
    }

    /// Final tangent-space normal, OpenGL, encoded 0..1.
    fn normal(&self, i: usize) -> [f32; 3] {
        let base = self.src.baked.contains_key(&BakedMap::Normal).then(|| self.baked(BakedMap::Normal, i));
        let detail =
            self.src.channels.contains_key(&ChannelId::Normal).then(|| self.channel(ChannelId::Normal, i));
        let n = match (base, detail) {
            (None, None) => return [0.5, 0.5, 1.0],
            (Some(b), None) => unpack(b),
            (None, Some(d)) => unpack(d),
            (Some(b), Some(d)) => rnm(b, d),
        };
        let n = normalize(n);
        [n[0] * 0.5 + 0.5, n[1] * 0.5 + 0.5, n[2] * 0.5 + 0.5]
    }

    fn flip_y(&self, v: f32) -> f32 {
        match self.normal {
            NormalFormat::OpenGL => v,
            NormalFormat::DirectX => 1.0 - v,
        }
    }

    pub fn eval(&self, s: &Source, i: usize) -> f32 {
        let pick = |v: [f32; 4], c: u8| v[(c as usize).min(3)];
        match *s {
            Source::Channel { channel, component } => {
                let v = pick(self.channel(channel, i), component);
                if channel == ChannelId::Normal && component == 1 { self.flip_y(v) } else { v }
            }
            Source::Baked { map, component } => {
                let v = pick(self.baked(map, i), component);
                if map == BakedMap::Normal && component == 1 { self.flip_y(v) } else { v }
            }
            Source::MixedAo => self.mixed_ao(i),
            Source::Constant { value } => value,
            Source::NormalCombined { component } => {
                let n = self.normal(i);
                match component {
                    0 => n[0],
                    1 => self.flip_y(n[1]),
                    2 => n[2],
                    _ => 1.0,
                }
            }
            Source::PbrDiffuse { component } => {
                if component > 2 {
                    return 1.0;
                }
                let base = self.channel(ChannelId::BaseColor, i)[component as usize];
                base * (1.0 - self.channel(ChannelId::Metallic, i)[0])
            }
            Source::PbrSpecular { component } => {
                if component > 2 {
                    return 1.0;
                }
                let base = self.channel(ChannelId::BaseColor, i)[component as usize];
                let m = self.channel(ChannelId::Metallic, i)[0];
                0.04 + (base - 0.04) * m
            }
            Source::StylizedUnlit { component, sky_light } => {
                if component > 2 {
                    return 1.0;
                }
                let c = component as usize;
                let mut light = self.mixed_ao(i);
                if sky_light {
                    light *= self.baked(BakedMap::SkyLight, i)[0];
                }
                self.channel(ChannelId::BaseColor, i)[c] * light + self.channel(ChannelId::Emissive, i)[c]
            }
        }
    }

    pub fn eval_slot(&self, slot: &Slot, i: usize) -> f32 {
        let mut v = self.eval(&slot.source, i);
        if slot.invert {
            v = 1.0 - v;
        }
        v = slot.remap_min + v * (slot.remap_max - slot.remap_min);
        if slot.gamma != 1.0 {
            v = v.max(0.0).powf(slot.gamma);
        }
        v
    }
}

fn unpack(v: [f32; 4]) -> [f32; 3] {
    [v[0] * 2.0 - 1.0, v[1] * 2.0 - 1.0, v[2] * 2.0 - 1.0]
}

fn normalize(n: [f32; 3]) -> [f32; 3] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l < 1e-8 { [0.0, 0.0, 1.0] } else { [n[0] / l, n[1] / l, n[2] / l] }
}

/// Reoriented normal mapping (Barré-Brisebois & Hill): `detail` re-oriented onto `base`.
/// Inputs are encoded 0..1; returns an unnormalized unpacked vector.
pub(crate) fn rnm(base: [f32; 4], detail: [f32; 4]) -> [f32; 3] {
    let t = [base[0] * 2.0 - 1.0, base[1] * 2.0 - 1.0, base[2] * 2.0];
    let u = [-detail[0] * 2.0 + 1.0, -detail[1] * 2.0 + 1.0, detail[2] * 2.0 - 1.0];
    let tz = if t[2].abs() < 1e-6 { 1e-6 } else { t[2] };
    let d = (t[0] * u[0] + t[1] * u[1] + t[2] * u[2]) / tz;
    [t[0] * d - u[0], t[1] * d - u[1], t[2] * d - u[2]]
}
