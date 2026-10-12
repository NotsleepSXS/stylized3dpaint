//! Bounding volume hierarchy over the mesh triangles, laid out for GPU traversal.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::Mesh;

/// 32-byte node. Leaf when `count > 0`: triangles `[first, first + count)`.
/// Interior nodes store the right child index in `first`; the left child follows the node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct BvhNode {
    pub min: [f32; 3],
    pub first: u32,
    pub max: [f32; 3],
    pub count: u32,
}

/// Triangle vertices and vertex normals (w unused) in BVH order.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct BvhTriangle {
    pub v0: [f32; 4],
    pub v1: [f32; 4],
    pub v2: [f32; 4],
    pub n0: [f32; 4],
    pub n1: [f32; 4],
    pub n2: [f32; 4],
}

pub struct Bvh {
    pub nodes: Vec<BvhNode>,
    pub triangles: Vec<BvhTriangle>,
}

const LEAF_SIZE: usize = 4;
const BINS: usize = 12;

#[derive(Clone, Copy)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

impl Aabb {
    const EMPTY: Aabb = Aabb { min: Vec3::splat(f32::MAX), max: Vec3::splat(f32::MIN) };

    fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    fn merge(&mut self, o: &Aabb) {
        self.min = self.min.min(o.min);
        self.max = self.max.max(o.max);
    }

    fn area(&self) -> f32 {
        let e = (self.max - self.min).max(Vec3::ZERO);
        e.x * e.y + e.y * e.z + e.z * e.x
    }
}

impl Bvh {
    /// Builds a binned-SAH BVH over all triangles of the mesh.
    pub fn build(mesh: &Mesh) -> Bvh {
        let tris: Vec<[Vec3; 3]> = mesh
            .indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]].map(|i| Vec3::from(mesh.vertices[i as usize].position)))
            .collect();
        let bounds: Vec<Aabb> = tris
            .iter()
            .map(|t| {
                let mut b = Aabb::EMPTY;
                t.iter().for_each(|&p| b.grow(p));
                b
            })
            .collect();
        let centroids: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::with_capacity(tris.len() * 2 / LEAF_SIZE + 1);
        if !tris.is_empty() {
            build_node(&mut nodes, &mut order, 0, &bounds, &centroids);
        }
        let normal = |t: usize, k: usize| {
            let n = mesh.vertices[mesh.indices[t * 3 + k] as usize].normal;
            [n[0], n[1], n[2], 0.0]
        };
        let triangles = order
            .iter()
            .map(|&i| {
                let [a, b, c] = tris[i as usize];
                let t = i as usize;
                BvhTriangle {
                    v0: a.extend(0.0).into(),
                    v1: b.extend(0.0).into(),
                    v2: c.extend(0.0).into(),
                    n0: normal(t, 0),
                    n1: normal(t, 1),
                    n2: normal(t, 2),
                }
            })
            .collect();
        Bvh { nodes, triangles }
    }
}

fn build_node(nodes: &mut Vec<BvhNode>, order: &mut [u32], offset: usize, bounds: &[Aabb], centroids: &[Vec3]) -> usize {
    let index = nodes.len();
    let mut node_box = Aabb::EMPTY;
    let mut centroid_box = Aabb::EMPTY;
    for &t in order.iter() {
        node_box.merge(&bounds[t as usize]);
        centroid_box.grow(centroids[t as usize]);
    }
    nodes.push(BvhNode { min: node_box.min.into(), max: node_box.max.into(), first: offset as u32, count: order.len() as u32 });
    if order.len() <= LEAF_SIZE {
        return index;
    }

    // Binned SAH split along each axis.
    let extent = centroid_box.max - centroid_box.min;
    let mut best: Option<(f32, usize, f32)> = None; // (cost, axis, split position)
    for axis in 0..3 {
        if extent[axis] <= 1e-12 {
            continue;
        }
        let mut bins = [(Aabb::EMPTY, 0usize); BINS];
        let scale = BINS as f32 / extent[axis];
        for &t in order.iter() {
            let b = (((centroids[t as usize][axis] - centroid_box.min[axis]) * scale) as usize).min(BINS - 1);
            bins[b].0.merge(&bounds[t as usize]);
            bins[b].1 += 1;
        }
        let mut left = [(0.0f32, 0usize); BINS - 1];
        let (mut acc, mut n) = (Aabb::EMPTY, 0);
        for i in 0..BINS - 1 {
            acc.merge(&bins[i].0);
            n += bins[i].1;
            left[i] = (acc.area(), n);
        }
        let (mut acc, mut n) = (Aabb::EMPTY, 0);
        for i in (1..BINS).rev() {
            acc.merge(&bins[i].0);
            n += bins[i].1;
            let (la, ln) = left[i - 1];
            if ln == 0 || n == 0 {
                continue;
            }
            let cost = la * ln as f32 + acc.area() * n as f32;
            if best.is_none_or(|b| cost < b.0) {
                best = Some((cost, axis, centroid_box.min[axis] + i as f32 / scale));
            }
        }
    }

    let leaf_cost = node_box.area() * order.len() as f32;
    let mid = match best {
        Some((cost, axis, split)) if cost < leaf_cost || order.len() > 32 => {
            let mut i = 0;
            for j in 0..order.len() {
                if centroids[order[j] as usize][axis] < split {
                    order.swap(i, j);
                    i += 1;
                }
            }
            i
        }
        // Degenerate centroids: split the list in half.
        None if order.len() > 32 => order.len() / 2,
        _ => return index,
    };
    if mid == 0 || mid == order.len() {
        if order.len() <= 32 {
            return index;
        }
    }
    let mid = if mid == 0 || mid == order.len() { order.len() / 2 } else { mid };

    let (left, right) = order.split_at_mut(mid);
    build_node(nodes, left, offset, bounds, centroids);
    let right_index = build_node(nodes, right, offset + mid, bounds, centroids);
    nodes[index].first = right_index as u32;
    nodes[index].count = 0;
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force closest hit, used to validate traversal.
    fn intersect(o: Vec3, d: Vec3, t: &BvhTriangle) -> Option<f32> {
        let (a, b, c) = (Vec3::from_slice(&t.v0), Vec3::from_slice(&t.v1), Vec3::from_slice(&t.v2));
        let (e1, e2) = (b - a, c - a);
        let p = d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let s = o - a;
        let u = s.dot(p) * inv;
        let q = s.cross(e1);
        let v = d.dot(q) * inv;
        let t = e2.dot(q) * inv;
        (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.0).then_some(t)
    }

    fn slab(o: Vec3, inv: Vec3, n: &BvhNode) -> bool {
        let t0 = (Vec3::from(n.min) - o) * inv;
        let t1 = (Vec3::from(n.max) - o) * inv;
        t0.min(t1).max_element() <= t0.max(t1).min_element() && t0.max(t1).min_element() >= 0.0
    }

    fn traverse(bvh: &Bvh, o: Vec3, d: Vec3) -> Option<f32> {
        let inv = d.recip();
        let mut stack = vec![0usize];
        let mut best: Option<f32> = None;
        while let Some(i) = stack.pop() {
            let n = &bvh.nodes[i];
            if !slab(o, inv, n) {
                continue;
            }
            if n.count > 0 {
                for t in &bvh.triangles[n.first as usize..(n.first + n.count) as usize] {
                    if let Some(h) = intersect(o, d, t) {
                        best = Some(best.map_or(h, |b: f32| b.min(h)));
                    }
                }
            } else {
                stack.push(i + 1);
                stack.push(n.first as usize);
            }
        }
        best
    }

    #[test]
    fn traversal_matches_brute_force() {
        let mesh = Mesh::uv_sphere(48, 24);
        let bvh = Bvh::build(&mesh);
        assert_eq!(bvh.triangles.len(), mesh.triangle_count());
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
        };
        for _ in 0..500 {
            let o = Vec3::new(rnd(), rnd(), rnd()) * 3.0;
            let d = Vec3::new(rnd(), rnd(), rnd()).normalize_or(Vec3::X);
            let brute = bvh.triangles.iter().filter_map(|t| intersect(o, d, t)).fold(None, |b: Option<f32>, h| Some(b.map_or(h, |b| b.min(h))));
            let fast = traverse(&bvh, o, d);
            match (brute, fast) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-4),
                (a, b) => assert_eq!(a.is_some(), b.is_some()),
            }
        }
    }
}
