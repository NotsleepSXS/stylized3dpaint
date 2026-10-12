//! Pure-CPU image operations: dilation padding, resampling, parallel row helper.

pub(crate) const NONE: u32 = u32::MAX;

/// Runs `f(y, row)` over rows of `data` (row length `row_len`) on all cores.
pub(crate) fn par_rows<T: Send>(data: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if row_len == 0 || data.is_empty() {
        return;
    }
    let rows = data.len() / row_len;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(rows.max(1));
    let per = rows.div_ceil(threads);
    std::thread::scope(|s| {
        for (ci, chunk) in data.chunks_mut(per * row_len).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (ri, row) in chunk.chunks_mut(row_len).enumerate() {
                    f(ci * per + ri, row);
                }
            });
        }
    });
}

/// For every texel: index of the nearest covered texel (8-connected multi-source BFS, O(N)),
/// or [`NONE`] if farther than `max_dist` / no coverage at all. Covered texels map to themselves.
pub(crate) fn nearest_map(w: usize, h: usize, coverage: &[bool], max_dist: Option<u32>) -> Vec<u32> {
    let n = w * h;
    if coverage.is_empty() {
        return (0..n as u32).collect();
    }
    let mut near = vec![NONE; n];
    let mut frontier: Vec<u32> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !coverage[i] {
                continue;
            }
            near[i] = i as u32;
            // Only island borders seed the flood.
            let border = (x > 0 && !coverage[i - 1])
                || (x + 1 < w && !coverage[i + 1])
                || (y > 0 && !coverage[i - w])
                || (y + 1 < h && !coverage[i + w]);
            if border {
                frontier.push(i as u32);
            }
        }
    }
    let mut next = Vec::new();
    let mut level = 0u32;
    while !frontier.is_empty() && max_dist.is_none_or(|m| level < m) {
        next.clear();
        for &i in &frontier {
            let i = i as usize;
            let (x, y) = (i % w, i / w);
            let src = near[i];
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let j = ny * w + nx;
                    if near[j] == NONE {
                        near[j] = src;
                        next.push(j as u32);
                    }
                }
            }
        }
        std::mem::swap(&mut frontier, &mut next);
        level += 1;
    }
    near
}

/// Separable resample: box (area average) when shrinking, bilinear when enlarging.
pub(crate) fn resample(data: &[f32], w: usize, h: usize, nch: usize, nw: usize, nh: usize) -> Vec<f32> {
    if w == nw && h == nh {
        return data.to_vec();
    }
    // Horizontal pass: w×h → nw×h.
    let wx = weights(w, nw);
    let mut tmp = vec![0.0f32; nw * h * nch];
    par_rows(&mut tmp, nw * nch, |y, row| {
        let src = &data[y * w * nch..(y + 1) * w * nch];
        for (x, ws) in wx.iter().enumerate() {
            for c in 0..nch {
                row[x * nch + c] = ws.iter().map(|&(i, k)| src[i * nch + c] * k).sum();
            }
        }
    });
    // Vertical pass: nw×h → nw×nh.
    let wy = weights(h, nh);
    let mut out = vec![0.0f32; nw * nh * nch];
    par_rows(&mut out, nw * nch, |y, row| {
        for (k_i, &(sy, k)) in wy[y].iter().enumerate() {
            let src = &tmp[sy * nw * nch..(sy + 1) * nw * nch];
            if k_i == 0 {
                row.iter_mut().zip(src).for_each(|(o, s)| *o = s * k);
            } else {
                row.iter_mut().zip(src).for_each(|(o, s)| *o += s * k);
            }
        }
    });
    out
}

/// Per destination index: (source index, weight) list summing to 1.
fn weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            if scale >= 1.0 {
                // Box: exact area coverage of [d*scale, (d+1)*scale).
                let a = d as f64 * scale;
                let b = a + scale;
                let mut v = Vec::new();
                let mut i = a.floor() as usize;
                while (i as f64) < b && i < src {
                    let cov = (b.min(i as f64 + 1.0) - a.max(i as f64)).max(0.0);
                    if cov > 0.0 {
                        v.push((i, (cov / scale) as f32));
                    }
                    i += 1;
                }
                v
            } else {
                // Bilinear at the destination texel center.
                let p = ((d as f64 + 0.5) * scale - 0.5).clamp(0.0, (src - 1) as f64);
                let i0 = p.floor() as usize;
                let i1 = (i0 + 1).min(src - 1);
                let t = (p - i0 as f64) as f32;
                if i0 == i1 { vec![(i0, 1.0)] } else { vec![(i0, 1.0 - t), (i1, t)] }
            }
        })
        .collect()
}

/// Cheap integer hash → [0,1).
pub(crate) fn hash01(x: u32, y: u32, c: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ c.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// 8×8 Bayer threshold in (-0.5, 0.5).
pub(crate) fn bayer8(x: u32, y: u32) -> f32 {
    const M: [u8; 64] = [
        0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60, 28,
        52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15, 47, 7, 39,
        13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
    ];
    (M[((y & 7) * 8 + (x & 7)) as usize] as f32 + 0.5) / 64.0 - 0.5
}
