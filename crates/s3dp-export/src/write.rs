//! Quantization (+ dithering) and file encoding.

use std::path::Path;

use anyhow::Result;
use half::f16;
use image::{DynamicImage, ImageBuffer, ImageFormat, Luma, LumaA, Rgb, Rgba};

use crate::{Dither, FileFormat, RenderedImage, ops};

/// Drops alpha (JPEG) or expands LA → RGBA (TIFF/TGA-safe) as the format requires.
fn adapt_channels(img: &RenderedImage, fmt: FileFormat) -> (usize, Vec<f32>) {
    let n = img.channels;
    let remap = |map: &dyn Fn(&[f32], &mut Vec<f32>), out_n: usize| {
        let mut out = Vec::with_capacity(img.data.len() / n * out_n);
        img.data.chunks_exact(n).for_each(|px| map(px, &mut out));
        (out_n, out)
    };
    match (fmt, n) {
        (FileFormat::Jpg8, 2) => remap(&|px, o| o.push(px[0]), 1),
        (FileFormat::Jpg8, 4) => remap(&|px, o| o.extend_from_slice(&px[..3]), 3),
        (FileFormat::Tiff8 | FileFormat::Tiff16, 2) => remap(&|px, o| o.extend_from_slice(&[px[0], px[0], px[0], px[1]]), 4),
        _ => (n, img.data.clone()),
    }
}

fn quantize8(data: &[f32], w: u32, nch: usize, dither: Dither) -> Vec<u8> {
    let alpha = nch == 2 || nch == 4;
    data.iter()
        .enumerate()
        .map(|(k, &v)| {
            let c = k % nch;
            let t = k / nch;
            let (x, y) = ((t % w as usize) as u32, (t / w as usize) as u32);
            let is_alpha = alpha && c == nch - 1;
            let noise = if is_alpha {
                0.0
            } else {
                match dither {
                    Dither::None => 0.0,
                    Dither::Ordered => ops::bayer8(x + c as u32 * 3, y + c as u32 * 5),
                    Dither::Triangular => ops::hash01(x, y, c as u32) + ops::hash01(x, y, c as u32 + 17) - 1.0,
                }
            };
            (v.clamp(0.0, 1.0) * 255.0 + noise).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

fn quantize16(data: &[f32]) -> Vec<u16> {
    data.iter().map(|&v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16).collect()
}

fn dynamic8(w: u32, h: u32, nch: usize, d: Vec<u8>) -> DynamicImage {
    match nch {
        1 => DynamicImage::ImageLuma8(ImageBuffer::<Luma<u8>, _>::from_raw(w, h, d).unwrap()),
        2 => DynamicImage::ImageLumaA8(ImageBuffer::<LumaA<u8>, _>::from_raw(w, h, d).unwrap()),
        3 => DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_raw(w, h, d).unwrap()),
        _ => DynamicImage::ImageRgba8(ImageBuffer::<Rgba<u8>, _>::from_raw(w, h, d).unwrap()),
    }
}

fn dynamic16(w: u32, h: u32, nch: usize, d: Vec<u16>) -> DynamicImage {
    match nch {
        1 => DynamicImage::ImageLuma16(ImageBuffer::<Luma<u16>, _>::from_raw(w, h, d).unwrap()),
        2 => DynamicImage::ImageLumaA16(ImageBuffer::<LumaA<u16>, _>::from_raw(w, h, d).unwrap()),
        3 => DynamicImage::ImageRgb16(ImageBuffer::<Rgb<u16>, _>::from_raw(w, h, d).unwrap()),
        _ => DynamicImage::ImageRgba16(ImageBuffer::<Rgba<u16>, _>::from_raw(w, h, d).unwrap()),
    }
}

pub(crate) fn write_image(path: &Path, img: &RenderedImage, fmt: FileFormat, dither: Dither) -> Result<()> {
    let (w, h) = (img.width, img.height);
    if fmt.is_float() {
        return write_exr(path, img, fmt == FileFormat::Exr16);
    }
    let (nch, data) = adapt_channels(img, fmt);
    let (dynimg, iformat) = match fmt {
        FileFormat::Png8 => (dynamic8(w, h, nch, quantize8(&data, w, nch, dither)), ImageFormat::Png),
        FileFormat::Tga8 => (dynamic8(w, h, nch, quantize8(&data, w, nch, dither)), ImageFormat::Tga),
        FileFormat::Tiff8 => (dynamic8(w, h, nch, quantize8(&data, w, nch, dither)), ImageFormat::Tiff),
        FileFormat::Jpg8 => (dynamic8(w, h, nch, quantize8(&data, w, nch, dither)), ImageFormat::Jpeg),
        FileFormat::Png16 => (dynamic16(w, h, nch, quantize16(&data)), ImageFormat::Png),
        FileFormat::Tiff16 => (dynamic16(w, h, nch, quantize16(&data)), ImageFormat::Tiff),
        FileFormat::Exr16 | FileFormat::Exr32 => unreachable!(),
    };
    dynimg.save_with_format(path, iformat)?;
    Ok(())
}

fn write_exr(path: &Path, img: &RenderedImage, half: bool) -> Result<()> {
    let (w, n) = (img.width as usize, img.channels);
    let d = &img.data;
    let px = |x: usize, y: usize| -> [f32; 4] {
        let p = &d[(y * w + x) * n..(y * w + x + 1) * n];
        match n {
            1 => [p[0], p[0], p[0], 1.0],
            2 => [p[0], p[0], p[0], p[1]],
            3 => [p[0], p[1], p[2], 1.0],
            _ => [p[0], p[1], p[2], p[3]],
        }
    };
    let (ww, hh) = (img.width as usize, img.height as usize);
    let alpha = img.has_alpha();
    match (half, alpha) {
        (true, true) => exr::prelude::write_rgba_file(path, ww, hh, |x, y| {
            let p = px(x, y);
            (f16::from_f32(p[0]), f16::from_f32(p[1]), f16::from_f32(p[2]), f16::from_f32(p[3]))
        })?,
        (true, false) => exr::prelude::write_rgb_file(path, ww, hh, |x, y| {
            let p = px(x, y);
            (f16::from_f32(p[0]), f16::from_f32(p[1]), f16::from_f32(p[2]))
        })?,
        (false, true) => exr::prelude::write_rgba_file(path, ww, hh, |x, y| {
            let p = px(x, y);
            (p[0], p[1], p[2], p[3])
        })?,
        (false, false) => exr::prelude::write_rgb_file(path, ww, hh, |x, y| {
            let p = px(x, y);
            (p[0], p[1], p[2])
        })?,
    }
    Ok(())
}
