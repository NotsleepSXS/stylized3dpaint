//! Image-based lighting from equirectangular HDRIs (CPU side, testable without a GPU):
//! loading (.hdr / .exr), diffuse irradiance as 9 spherical-harmonic coefficients, and a
//! GGX-prefiltered mip chain for specular reflections (one roughness per mip level).
//!
//! Conventions: direction `d` maps to `u = 0.5 + atan2(d.x, -d.z) / 2π`, `v = acos(d.y) / π`
//! (v = 0 at the zenith, image rows top to bottom), the same in the viewport shader.

use std::f32::consts::PI;

use egui_wgpu::wgpu;
use glam::Vec3;

/// Linear RGB equirectangular image.
#[derive(Clone, Debug)]
pub struct Equirect {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; 3]>,
}

impl Equirect {
    pub fn new(width: usize, height: usize, data: Vec<[f32; 3]>) -> Self {
        Self { width, height, data }
    }

    /// Decodes a .hdr / .exr (or any format the image crate reads) into linear RGB.
    pub fn decode(bytes: &[u8]) -> image::ImageResult<Self> {
        let img = image::load_from_memory(bytes)?.to_rgb32f();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let data = img.pixels().map(|p| {
            let f = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
            [f(p[0]), f(p[1]), f(p[2])]
        });
        Ok(Self::new(w, h, data.collect()))
    }

    pub fn at(&self, x: usize, y: usize) -> [f32; 3] {
        self.data[y.min(self.height - 1) * self.width + x % self.width]
    }

    /// Bilinear sample at a direction (u wraps, v clamps).
    pub fn sample(&self, d: Vec3) -> [f32; 3] {
        let (u, v) = dir_to_uv(d);
        let fx = u * self.width as f32 - 0.5;
        let fy = (v * self.height as f32 - 0.5).clamp(0.0, self.height as f32 - 1.0);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let xi = |x: f32| (x as i64).rem_euclid(self.width as i64) as usize;
        let (xa, xb) = (xi(x0), xi(x0 + 1.0));
        let (ya, yb) = (y0 as usize, (y0 as usize + 1).min(self.height - 1));
        let l = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        l(l(self.at(xa, ya), self.at(xb, ya), tx), l(self.at(xa, yb), self.at(xb, yb), tx), ty)
    }

    /// Box-downsampled copy (each output texel averages the source texels it covers).
    pub fn downsample(&self, width: usize) -> Self {
        let width = width.clamp(1, self.width);
        let height = (width / 2).max(1);
        let (sx, sy) = (self.width as f32 / width as f32, self.height as f32 / height as f32);
        let mut data = vec![[0.0; 3]; width * height];
        for y in 0..height {
            for x in 0..width {
                let x0 = (x as f32 * sx) as usize;
                let x1 = (((x + 1) as f32 * sx) as usize).max(x0 + 1);
                let y0 = (y as f32 * sy) as usize;
                let y1 = (((y + 1) as f32 * sy) as usize).max(y0 + 1);
                let mut acc = [0.0f32; 3];
                let mut n = 0.0;
                for yy in y0..y1.min(self.height) {
                    for xx in x0..x1.min(self.width) {
                        let p = self.at(xx, yy);
                        acc = [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]];
                        n += 1.0;
                    }
                }
                data[y * width + x] = [acc[0] / n, acc[1] / n, acc[2] / n];
            }
        }
        Self::new(width, height, data)
    }

    /// Solid angle of a texel row (sin weighting of the equirect).
    fn row_weight(&self, y: usize) -> f32 {
        let theta = (y as f32 + 0.5) / self.height as f32 * PI;
        theta.sin() * (2.0 * PI / self.width as f32) * (PI / self.height as f32)
    }
}

pub fn dir_to_uv(d: Vec3) -> (f32, f32) {
    let d = d.normalize_or(Vec3::Y);
    let u = 0.5 + d.x.atan2(-d.z) / (2.0 * PI);
    let v = d.y.clamp(-1.0, 1.0).acos() / PI;
    (u, v)
}

pub fn uv_to_dir(u: f32, v: f32) -> Vec3 {
    let phi = (u - 0.5) * 2.0 * PI;
    let theta = v * PI;
    Vec3::new(theta.sin() * phi.sin(), theta.cos(), -theta.sin() * phi.cos())
}

fn sh_basis(d: Vec3) -> [f32; 9] {
    [
        0.282095,
        0.488603 * d.y,
        0.488603 * d.z,
        0.488603 * d.x,
        1.092548 * d.x * d.y,
        1.092548 * d.y * d.z,
        0.315392 * (3.0 * d.z * d.z - 1.0),
        1.092548 * d.x * d.z,
        0.546274 * (d.x * d.x - d.y * d.y),
    ]
}

/// Irradiance spherical harmonics (Ramamoorthi & Hanrahan), pre-convolved with the clamped
/// cosine and divided by π: `eval_sh(sh, n)` is the diffuse light for a white Lambert
/// surface with normal `n`.
pub fn irradiance_sh(env: &Equirect) -> [[f32; 3]; 9] {
    let mut sh = [[0.0f32; 3]; 9];
    for y in 0..env.height {
        let w = env.row_weight(y);
        for x in 0..env.width {
            let d = uv_to_dir((x as f32 + 0.5) / env.width as f32, (y as f32 + 0.5) / env.height as f32);
            let c = env.at(x, y);
            for (k, b) in sh_basis(d).iter().enumerate() {
                for ch in 0..3 {
                    sh[k][ch] += c[ch] * b * w;
                }
            }
        }
    }
    // Cosine lobe convolution (A_l) and the 1/π of the Lambert BRDF.
    let a = [PI, 2.0 * PI / 3.0, 2.0 * PI / 3.0, 2.0 * PI / 3.0, PI / 4.0, PI / 4.0, PI / 4.0, PI / 4.0, PI / 4.0];
    for (k, coef) in sh.iter_mut().enumerate() {
        for v in coef.iter_mut() {
            *v *= a[k] / PI;
        }
    }
    sh
}

pub fn eval_sh(sh: &[[f32; 3]; 9], n: Vec3) -> [f32; 3] {
    let b = sh_basis(n.normalize_or(Vec3::Y));
    let mut out = [0.0f32; 3];
    for k in 0..9 {
        for ch in 0..3 {
            out[ch] += sh[k][ch] * b[k];
        }
    }
    out.map(|v| v.max(0.0))
}

fn radical_inverse(mut bits: u32) -> f32 {
    bits = bits.rotate_right(16);
    bits = ((bits & 0x55555555) << 1) | ((bits & 0xAAAAAAAA) >> 1);
    bits = ((bits & 0x33333333) << 2) | ((bits & 0xCCCCCCCC) >> 2);
    bits = ((bits & 0x0F0F0F0F) << 4) | ((bits & 0xF0F0F0F0) >> 4);
    bits = ((bits & 0x00FF00FF) << 8) | ((bits & 0xFF00FF00) >> 8);
    bits as f32 * 2.328_306_4e-10
}

/// GGX-prefiltered environment for `roughness` at `width` (split-sum, N = V = R), sampling a
/// source pyramid level chosen per sample from its pdf (filtered importance sampling), so a
/// few dozen samples are enough and bright spots do not alias.
pub fn prefilter_ggx(pyramid: &[Equirect], width: usize, roughness: f32, samples: u32) -> Equirect {
    let base = &pyramid[0];
    let height = (width / 2).max(1);
    let a = (roughness * roughness).max(1e-4);
    let texel_sa = 4.0 * PI / (base.width * base.height) as f32;
    // Precomputed sample set (tangent space): half vectors and their source levels.
    let set: Vec<(Vec3, f32)> = (0..samples)
        .filter_map(|i| {
            let (u1, u2) = (i as f32 / samples as f32, radical_inverse(i));
            let cos_t = ((1.0 - u2) / (1.0 + (a * a - 1.0) * u2)).sqrt();
            let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
            let phi = 2.0 * PI * u1;
            let h = Vec3::new(sin_t * phi.cos(), sin_t * phi.sin(), cos_t);
            // pdf of the reflected direction for N = V: D(h) / 4.
            let d = a * a / (PI * ((cos_t * cos_t) * (a * a - 1.0) + 1.0).powi(2));
            let pdf = d / 4.0;
            let sa = 1.0 / (samples as f32 * pdf + 1e-6);
            let level = if roughness <= 0.0 { 0.0 } else { (0.5 * (sa / texel_sa).log2() + 1.0).max(0.0) };
            Some((h, level))
        })
        .collect();
    let mut data = vec![[0.0f32; 3]; width * height];
    for y in 0..height {
        for x in 0..width {
            let n = uv_to_dir((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
            let up = if n.y.abs() < 0.999 { Vec3::Y } else { Vec3::X };
            let t = up.cross(n).normalize();
            let b = n.cross(t);
            let mut acc = [0.0f32; 3];
            let mut wsum = 0.0;
            for (h, level) in &set {
                let hw = t * h.x + b * h.y + n * h.z;
                let l = 2.0 * n.dot(hw) * hw - n;
                let ndl = n.dot(l);
                if ndl <= 0.0 {
                    continue;
                }
                let li = (*level as usize).min(pyramid.len() - 1);
                let c = pyramid[li].sample(l);
                acc = [acc[0] + c[0] * ndl, acc[1] + c[1] * ndl, acc[2] + c[2] * ndl];
                wsum += ndl;
            }
            data[y * width + x] = if wsum > 0.0 { [acc[0] / wsum, acc[1] / wsum, acc[2] / wsum] } else { n_fallback(base, n) };
        }
    }
    Equirect::new(width, height, data)
}

fn n_fallback(env: &Equirect, n: Vec3) -> [f32; 3] {
    env.sample(n)
}

/// Everything the GPU needs for one environment.
pub struct PreparedEnv {
    pub name: String,
    /// Specular mip chain: level k is prefiltered for roughness k / (levels - 1); level 0 is
    /// the (downsampled) mirror image, also used for the background.
    pub levels: Vec<Equirect>,
    pub sh: [[f32; 3]; 9],
    /// Small tone-mapped sRGB thumbnail (width x width/2, RGBA8).
    pub thumb: (usize, usize, Vec<u8>),
}

/// Number of specular levels (roughness 0, 1/6, ..., 1).
pub const ENV_LEVELS: usize = 7;

/// Prepares an environment: mirror level at `top_width` (power of two), prefiltered levels
/// halving in size, irradiance SH and a thumbnail.
pub fn prepare(name: &str, env: &Equirect, top_width: usize) -> PreparedEnv {
    let top = env.downsample(top_width.min(env.width).next_power_of_two().min(top_width));
    // Source pyramid for filtered importance sampling.
    let mut pyramid = vec![top.clone()];
    while pyramid.last().is_some_and(|p| p.width > 8) {
        let next = pyramid.last().unwrap().downsample(pyramid.last().unwrap().width / 2);
        pyramid.push(next);
    }
    // Levels are independent: one thread each.
    let mut levels = vec![top.clone()];
    let rest: Vec<Equirect> = std::thread::scope(|s| {
        let jobs: Vec<_> = (1..ENV_LEVELS)
            .map(|k| {
                let pyramid = &pyramid;
                let w = (top.width >> k).max(4);
                let r = k as f32 / (ENV_LEVELS - 1) as f32;
                s.spawn(move || prefilter_ggx(pyramid, w, r, 96))
            })
            .collect();
        jobs.into_iter().filter_map(|j| j.join().ok()).collect()
    });
    levels.extend(rest);
    let sh = irradiance_sh(&pyramid[pyramid.len().saturating_sub(3).min(pyramid.len() - 1)]);
    let small = top.downsample(128);
    let mut thumb = Vec::with_capacity(small.data.len() * 4);
    for p in &small.data {
        for c in p {
            let t = c / (1.0 + c);
            thumb.push((t.powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8);
        }
        thumb.push(255);
    }
    PreparedEnv { name: name.to_owned(), levels, sh, thumb: (small.width, small.height, thumb) }
}

/// An environment on the GPU: the specular mip chain (Rgba16Float) and the irradiance SH.
pub struct GpuEnv {
    pub name: String,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// SH coefficients (rgb, w unused) for the shader.
    pub sh: [[f32; 4]; 9],
    /// Highest mip level (roughness 1).
    pub max_lod: f32,
    /// Exposure multiplier that brings the average diffuse light to 1 (auto exposure).
    pub auto_exposure: f32,
}

impl GpuEnv {
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, p: &PreparedEnv) -> Self {
        let top = &p.levels[0];
        // Only levels that halve exactly form the mip chain.
        let mips = p.levels.iter().enumerate().take_while(|(k, l)| l.width == (top.width >> k).max(1) && l.height == (top.height >> k).max(1)).count().max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment"),
            size: wgpu::Extent3d { width: top.width as u32, height: top.height as u32, depth_or_array_layers: 1 },
            mip_level_count: mips as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (k, l) in p.levels.iter().take(mips).enumerate() {
            let px: Vec<half::f16> = l.data.iter().flat_map(|c| [c[0], c[1], c[2], 1.0]).map(|v| half::f16::from_f32(v.min(65000.0))).collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: k as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&px),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(l.width as u32 * 8), rows_per_image: None },
                wgpu::Extent3d { width: l.width as u32, height: l.height as u32, depth_or_array_layers: 1 },
            );
        }
        let view = texture.create_view(&Default::default());
        let sh = p.sh.map(|c| [c[0], c[1], c[2], 0.0]);
        Self { name: p.name.clone(), texture, view, sh, max_lod: (mips - 1) as f32, auto_exposure: auto_exposure(&p.sh) }
    }

    /// 1x1 black stand-in (before the first frame can upload a real one).
    pub fn placeholder(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment placeholder"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { name: String::new(), texture, view, sh: [[0.0; 4]; 9], max_lod: 0.0, auto_exposure: 1.0 }
    }

    /// The built-in neutral studio environment.
    pub fn neutral(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self::upload(device, queue, &prepare("Neutral studio", &neutral(), 256))
    }
}

/// Multiplier that maps the environment's mean diffuse light (luminance of the SH DC term)
/// to 0.8, clamped to 1/8..8 so extreme maps stay recognisable.
pub fn auto_exposure(sh: &[[f32; 3]; 9]) -> f32 {
    let dc = sh[0].map(|v| v * 0.282095);
    let lum = 0.2126 * dc[0] + 0.7152 * dc[1] + 0.0722 * dc[2];
    if lum <= 1e-6 { 1.0 } else { (0.8 / lum).clamp(0.125, 8.0) }
}

/// How the environment is used by a view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvSettings {
    /// Rotation around the vertical axis, radians.
    pub rotation: f32,
    /// Exposure in EV (stops).
    pub exposure: f32,
    /// Show the HDRI behind the model.
    pub background: bool,
    /// Background blur 0..1 (fraction of the roughness range).
    pub blur: f32,
    /// Normalise the map's brightness (see [`auto_exposure`]).
    pub auto: bool,
}

impl Default for EnvSettings {
    fn default() -> Self {
        Self { rotation: 0.0, exposure: 0.0, background: false, blur: 0.35, auto: true }
    }
}

/// A neutral studio-like environment (soft top light over a darker floor) used before any
/// HDRI is loaded and in tests.
pub fn neutral() -> Equirect {
    let (w, h) = (256usize, 128usize);
    let mut data = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let d = uv_to_dir((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let sky = 0.35 + 0.65 * (d.y * 0.5 + 0.5);
            let key = (d.dot(Vec3::new(0.5, 0.8, 0.4).normalize()).max(0.0)).powf(8.0) * 6.0;
            let v = sky + key;
            data.push([v * 0.95, v * 0.97, v]);
        }
    }
    Equirect::new(w, h, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(v: f32) -> Equirect {
        Equirect::new(64, 32, vec![[v; 3]; 64 * 32])
    }

    #[test]
    fn uv_round_trip() {
        for d in [Vec3::Y, Vec3::NEG_Y * 0.999 + Vec3::X * 0.01, Vec3::X, Vec3::NEG_Z, Vec3::new(0.3, -0.4, 0.8)] {
            let (u, v) = dir_to_uv(d);
            let back = uv_to_dir(u, v);
            assert!(back.distance(d.normalize()) < 1e-3, "{d} -> {back}");
        }
    }

    #[test]
    fn uniform_env_gives_uniform_irradiance() {
        let sh = irradiance_sh(&uniform(2.0));
        for n in [Vec3::Y, Vec3::NEG_Y, Vec3::X, Vec3::new(0.3, 0.5, -0.8)] {
            let e = eval_sh(&sh, n);
            assert!((e[0] - 2.0).abs() < 0.05, "{n}: {e:?}");
        }
    }

    #[test]
    fn bright_sky_lights_up_facing_normals_more() {
        let mut env = uniform(0.1);
        for y in 0..env.height / 2 {
            for x in 0..env.width {
                env.data[y * env.width + x] = [3.0; 3];
            }
        }
        let sh = irradiance_sh(&env);
        let (up, down) = (eval_sh(&sh, Vec3::Y)[0], eval_sh(&sh, Vec3::NEG_Y)[0]);
        assert!(up > 2.0 && down < 0.6, "up {up} down {down}");
    }

    #[test]
    fn auto_exposure_normalises_brightness() {
        let dim = irradiance_sh(&uniform(0.1));
        let bright = irradiance_sh(&uniform(4.0));
        let (a, b) = (auto_exposure(&dim), auto_exposure(&bright));
        assert!((0.1 * a - 0.8).abs() < 0.05 && (4.0 * b - 0.8).abs() < 0.05, "{a} {b}");
        assert_eq!(auto_exposure(&irradiance_sh(&uniform(1e-5))), 8.0, "clamped");
    }

    #[test]
    fn prefilter_keeps_uniform_and_blurs_a_spot() {
        let flat = uniform(1.5);
        let pyr = vec![flat.clone(), flat.downsample(32), flat.downsample(16)];
        let out = prefilter_ggx(&pyr, 32, 0.6, 64);
        assert!(out.data.iter().all(|p| (p[0] - 1.5).abs() < 1e-3));
        // A bright spot spreads with roughness.
        let mut spot = uniform(0.0);
        let (x, y) = (32, 16);
        spot.data[y * 64 + x] = [1000.0; 3];
        let pyr = vec![spot.clone(), spot.downsample(32), spot.downsample(16), spot.downsample(8)];
        let sharp = prefilter_ggx(&pyr, 32, 0.15, 64);
        let rough = prefilter_ggx(&pyr, 32, 0.9, 64);
        let lit = |e: &Equirect| e.data.iter().filter(|p| p[0] > 0.5).count();
        assert!(lit(&rough) > lit(&sharp), "rough spreads wider: {} vs {}", lit(&rough), lit(&sharp));
    }

    #[test]
    fn prepare_builds_the_chain() {
        let p = prepare("neutral", &neutral(), 256);
        assert_eq!(p.levels.len(), ENV_LEVELS);
        assert_eq!(p.levels[0].width, 256);
        assert!(p.levels.iter().enumerate().all(|(k, l)| l.width == 256 >> k), "a proper mip chain");
        assert!(p.levels.windows(2).all(|w| w[1].width <= w[0].width));
        assert_eq!(p.thumb.2.len(), p.thumb.0 * p.thumb.1 * 4);
        let up = eval_sh(&p.sh, Vec3::Y)[0];
        let down = eval_sh(&p.sh, Vec3::NEG_Y)[0];
        assert!(up > down);
    }
}

#[cfg(test)]
mod timing {
    /// `HDRI=path cargo test --release -p s3dp-render --lib env_prepare_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn env_prepare_timing() {
        let Some(path) = std::env::var_os("HDRI") else { return };
        let t = std::time::Instant::now();
        let eq = super::Equirect::decode(&std::fs::read(path).unwrap()).unwrap();
        let t1 = t.elapsed();
        let p = super::prepare("x", &eq, 1024);
        println!("decode {t1:?}, prepare {:?}, sizes {:?}", t.elapsed() - t1, p.levels.iter().map(|l| l.width).collect::<Vec<_>>());
    }
}

#[cfg(test)]
mod assets {
    /// Writes 1K copies of the HDRIs in `$HDRI_SRC` to `$HDRI_DST` (built-in assets):
    /// `HDRI_SRC=dir HDRI_DST=assets/hdri cargo test --release -p s3dp-render --lib make_builtin_hdri -- --ignored`
    #[test]
    #[ignore]
    fn make_builtin_hdri() {
        let (Some(src), Some(dst)) = (std::env::var_os("HDRI_SRC"), std::env::var_os("HDRI_DST")) else { return };
        std::fs::create_dir_all(&dst).unwrap();
        for e in std::fs::read_dir(src).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "hdr") {
                continue;
            }
            let eq = super::Equirect::decode(&std::fs::read(&p).unwrap()).unwrap();
            let small = eq.downsample(1024);
            let stem = p.file_stem().unwrap().to_string_lossy().replace("_2k", "").replace("_4k", "");
            let out = std::path::Path::new(&dst).join(format!("{stem}_1k.hdr"));
            let px: Vec<image::Rgb<f32>> = small.data.iter().map(|c| image::Rgb(*c)).collect();
            let f = std::io::BufWriter::new(std::fs::File::create(&out).unwrap());
            image::codecs::hdr::HdrEncoder::new(f).encode(&px, small.width, small.height).unwrap();
            println!("{}", out.display());
        }
    }
}
