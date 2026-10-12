//! UV seam links: for every texel on the border of a UV island, the texel on the other side of
//! the seam (the nearest point of the same surface in 3D that lies on another island, or far
//! away on the same one). Effects that read neighbouring texels follow these links in
//! "seamless" mode, so blurs / warps / edge growth continue across seams instead of stopping.

use std::collections::HashMap;

/// No link (interior texel, uncovered texel, or an open border).
pub const NO_LINK: [f32; 2] = [-1.0, -1.0];

/// Computes the links from the rasterized world positions (`pos[i] = [x, y, z, coverage]`,
/// row-major, `w`×`h`). Returns one `[x, y]` texel per input texel ([`NO_LINK`] if none).
pub fn seam_links(pos: &[[f32; 4]], w: usize, h: usize) -> Vec<[f32; 2]> {
    let mut out = vec![NO_LINK; w * h];
    if pos.len() < w * h || w == 0 || h == 0 {
        return out;
    }
    let covered = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && pos[y as usize * w + x as usize][3] > 0.5;
    let p3 = |i: usize| [pos[i][0], pos[i][1], pos[i][2]];
    let dist = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();

    // Border texels and their world texel size.
    let mut border: Vec<(usize, f32)> = Vec::new();
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !covered(x, y) {
                continue;
            }
            let n4 = [(1, 0), (-1, 0), (0, 1), (0, -1)];
            if n4.iter().all(|(dx, dy)| covered(x + dx, y + dy)) {
                continue;
            }
            let i = y as usize * w + x as usize;
            let (mut sum, mut cnt) = (0.0, 0);
            for (dx, dy) in n4 {
                if covered(x + dx, y + dy) {
                    sum += dist(p3(i), p3(((y + dy) as usize) * w + (x + dx) as usize));
                    cnt += 1;
                }
            }
            border.push((i, if cnt > 0 { sum / cnt as f32 } else { 0.0 }));
        }
    }
    if border.is_empty() {
        return out;
    }
    let mut sizes: Vec<f32> = border.iter().map(|b| b.1).filter(|s| *s > 0.0 && s.is_finite()).collect();
    if sizes.is_empty() {
        return out;
    }
    sizes.sort_by(|a, b| a.total_cmp(b));
    let cell = (sizes[sizes.len() / 2] * 3.0).max(1e-9);
    let key = |p: [f32; 3]| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64, (p[2] / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for (k, (i, _)) in border.iter().enumerate() {
        grid.entry(key(p3(*i))).or_default().push(k as u32);
    }

    for &(i, tw) in &border {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let p = p3(i);
        let (kx, ky, kz) = key(p);
        let mut best: Option<(f32, usize)> = None;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let Some(list) = grid.get(&(kx + dx, ky + dy, kz + dz)) else { continue };
                    for &k in list {
                        let (j, tj) = border[k as usize];
                        let (jx, jy) = ((j % w) as i64, (j / w) as i64);
                        // The other side of the seam is far away in UV.
                        if (jx - x).abs().max((jy - y).abs()) <= 3 {
                            continue;
                        }
                        let d = dist(p, p3(j));
                        let tol = tw.max(tj).max(1e-9) * 1.5;
                        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, j));
                        }
                    }
                }
            }
        }
        if let Some((_, j)) = best {
            out[i] = [(j % w) as f32, (j / w) as f32];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cylinder side unrolled into the full texture: u = angle, v = height. The seam is
    /// the left / right texture border (u = 0 and u = 1 meet in 3D).
    fn cylinder(w: usize, h: usize) -> Vec<[f32; 4]> {
        let mut v = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let a = (x as f32 + 0.5) / w as f32 * std::f32::consts::TAU;
                v.push([a.cos(), (y as f32 + 0.5) / h as f32, a.sin(), 1.0]);
            }
        }
        v
    }

    #[test]
    fn cylinder_seam_links_left_and_right_columns() {
        let (w, h) = (64, 16);
        let links = seam_links(&cylinder(w, h), w, h);
        for y in 2..h - 2 {
            let l = links[y * w];
            let r = links[y * w + w - 1];
            assert_eq!(l, [(w - 1) as f32, y as f32], "left column links to the right one (row {y})");
            assert_eq!(r, [0.0, y as f32], "right column links to the left one (row {y})");
            // Interior texels have no link.
            assert_eq!(links[y * w + w / 2], NO_LINK);
        }
        // Top / bottom rims are open borders: no partner.
        assert_eq!(links[w / 2], NO_LINK);
        assert_eq!(links[(h - 1) * w + w / 2], NO_LINK);
    }

    #[test]
    fn degenerate_inputs() {
        assert!(seam_links(&[], 0, 0).is_empty());
        assert_eq!(seam_links(&[[0.0; 4]; 4], 2, 2), vec![NO_LINK; 4]);
        assert_eq!(seam_links(&[[0.0; 4]; 3], 2, 2), vec![NO_LINK; 4], "short input");
    }
}
