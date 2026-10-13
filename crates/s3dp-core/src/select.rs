//! Polygon selection for the Polygon Fill tool: which triangles form a face (quads rebuilt from
//! the import triangulation), a mesh element (a connected, unattached part) or a UV island,
//! and picking the triangle under a camera ray or a UV point.

use std::collections::HashMap;

use glam::{Vec2, Vec3};

use crate::Mesh;

/// What one click selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SelectMode {
    /// A polygon: a triangle, or the two triangles of a quad.
    #[default]
    Face,
    /// A connected part of the mesh (not attached to the rest).
    Element,
    /// A UV island.
    Island,
}

/// Per-triangle group ids (triangle `t` = indices `3t..3t+3`).
#[derive(Clone, Debug, Default)]
pub struct Topology {
    pub set: Vec<u32>,
    pub face: Vec<u32>,
    pub element: Vec<u32>,
    pub island: Vec<u32>,
}

struct Dsu(Vec<u32>);

impl Dsu {
    fn new(n: usize) -> Self {
        Self((0..n as u32).collect())
    }
    fn find(&mut self, mut a: u32) -> u32 {
        while self.0[a as usize] != a {
            let p = self.0[self.0[a as usize] as usize];
            self.0[a as usize] = p;
            a = p;
        }
        a
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb) as usize] = ra.min(rb);
        }
    }
    fn roots(mut self) -> Vec<u32> {
        (0..self.0.len() as u32).map(|i| self.find(i)).collect()
    }
}

impl Topology {
    pub fn build(mesh: &Mesh) -> Self {
        let n = mesh.indices.len() / 3;
        let mut set = vec![0u32; n];
        for (si, part) in mesh.parts.iter().enumerate() {
            let (a, b) = (part.first_index as usize / 3, (part.first_index + part.index_count) as usize / 3);
            for s in set.iter_mut().take(b.min(n)).skip(a) {
                *s = si as u32;
            }
        }
        // Vertices split for UV seams / hard normals are welded by position (and by UV for islands).
        let b = mesh.bounds();
        let q = ((b.max - b.min).max_element() * 1e-5).max(1e-9);
        let qp = |v: u32| {
            let p = Vec3::from(mesh.vertices[v as usize].position) / q;
            (p.x.round() as i64, p.y.round() as i64, p.z.round() as i64)
        };
        let qu = |v: u32| {
            let u = Vec2::from(mesh.vertices[v as usize].uv) * 1e6;
            (u.x.round() as i64, u.y.round() as i64)
        };
        let tri = |t: usize| [mesh.indices[3 * t], mesh.indices[3 * t + 1], mesh.indices[3 * t + 2]];

        let mut element = Dsu::new(n);
        let mut island = Dsu::new(n);
        let mut by_pos: HashMap<(u32, (i64, i64, i64)), u32> = HashMap::new();
        let mut by_pos_uv: HashMap<(u32, (i64, i64, i64), (i64, i64)), u32> = HashMap::new();
        for t in 0..n {
            for v in tri(t) {
                let s = set[t];
                match by_pos.get(&(s, qp(v))) {
                    Some(&o) => element.union(o, t as u32),
                    None => {
                        by_pos.insert((s, qp(v)), t as u32);
                    }
                }
                match by_pos_uv.get(&(s, qp(v), qu(v))) {
                    Some(&o) => island.union(o, t as u32),
                    None => {
                        by_pos_uv.insert((s, qp(v), qu(v)), t as u32);
                    }
                }
            }
        }

        // Faces: pair a triangle with the neighbour across its longest edge when that edge is
        // also the neighbour's longest and both are coplanar (a triangulated quad).
        let pos = |v: u32| Vec3::from(mesh.vertices[v as usize].position);
        let normal = |t: usize| {
            let [a, b, c] = tri(t);
            (pos(b) - pos(a)).cross(pos(c) - pos(a)).normalize_or_zero()
        };
        let longest = |t: usize| {
            let [a, b, c] = tri(t);
            let edges = [(a, b), (b, c), (c, a)];
            let (x, y) = edges.into_iter().max_by(|e, f| pos(e.0).distance_squared(pos(e.1)).total_cmp(&pos(f.0).distance_squared(pos(f.1)))).unwrap_or((a, b));
            let (kx, ky) = (qp(x), qp(y));
            (set[t], kx.min(ky), kx.max(ky))
        };
        let mut by_edge: HashMap<(u32, (i64, i64, i64), (i64, i64, i64)), Vec<u32>> = HashMap::new();
        for t in 0..n {
            by_edge.entry(longest(t)).or_default().push(t as u32);
        }
        let mut face = Dsu::new(n);
        for tris in by_edge.values() {
            if let [a, b] = tris[..]
                && normal(a as usize).dot(normal(b as usize)) > 0.999
            {
                face.union(a, b);
            }
        }
        Self { set, face: face.roots(), element: element.roots(), island: island.roots() }
    }

    pub fn group_of(&self, mode: SelectMode, t: u32) -> Option<u32> {
        let v = match mode {
            SelectMode::Face => &self.face,
            SelectMode::Element => &self.element,
            SelectMode::Island => &self.island,
        };
        v.get(t as usize).copied()
    }

    /// Triangles in the same group as `t`.
    pub fn triangles_with(&self, mode: SelectMode, t: u32) -> Vec<u32> {
        let Some(g) = self.group_of(mode, t) else { return Vec::new() };
        let v = match mode {
            SelectMode::Face => &self.face,
            SelectMode::Element => &self.element,
            SelectMode::Island => &self.island,
        };
        v.iter().enumerate().filter(|(_, x)| **x == g).map(|(i, _)| i as u32).collect()
    }
}

/// Closest triangle hit by the ray (Möller–Trumbore, both sides): (triangle, distance).
pub fn pick_ray(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<(u32, f32)> {
    let d = dir.normalize_or_zero();
    let mut best: Option<(u32, f32)> = None;
    for t in 0..mesh.indices.len() / 3 {
        let p = |k: usize| Vec3::from(mesh.vertices[mesh.indices[3 * t + k] as usize].position);
        let (a, b, c) = (p(0), p(1), p(2));
        let (e1, e2) = (b - a, c - a);
        let h = d.cross(e2);
        let det = e1.dot(h);
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let s = origin - a;
        let u = s.dot(h) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let qv = s.cross(e1);
        let v = d.dot(qv) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let dist = e2.dot(qv) * inv;
        if dist > 1e-6 && best.is_none_or(|(_, bd)| dist < bd) {
            best = Some((t as u32, dist));
        }
    }
    best
}

/// Triangle of texture set `set` whose UV triangle contains `uv`.
pub fn pick_uv(mesh: &Mesh, set: usize, uv: Vec2) -> Option<u32> {
    let part = mesh.parts.get(set)?;
    let (a0, b0) = (part.first_index as usize / 3, (part.first_index + part.index_count) as usize / 3);
    for t in a0..b0.min(mesh.indices.len() / 3) {
        let p = |k: usize| Vec2::from(mesh.vertices[mesh.indices[3 * t + k] as usize].uv);
        let (a, b, c) = (p(0), p(1), p(2));
        let cross = |o: Vec2, x: Vec2, y: Vec2| (x - o).perp_dot(y - o);
        let (d1, d2, d3) = (cross(a, b, uv), cross(b, c, uv), cross(c, a, uv));
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        if !(neg && pos) {
            return Some(t as u32);
        }
    }
    None
}

/// Vertex indices (3 per triangle) of the given triangles, for drawing / filling them.
pub fn triangle_indices(mesh: &Mesh, tris: &[u32]) -> Vec<u32> {
    tris.iter().flat_map(|&t| (0..3).map(move |k| 3 * t as usize + k)).filter_map(|i| mesh.indices.get(i).copied()).collect()
}

/// Unique edges as vertex index pairs (for the wireframe overlay); edges shared by split
/// vertices are deduplicated by position.
pub fn wire_edges(mesh: &Mesh) -> Vec<u32> {
    let b = mesh.bounds();
    let q = ((b.max - b.min).max_element() * 1e-5).max(1e-9);
    let key = |v: u32| {
        let p = Vec3::from(mesh.vertices[v as usize].position) / q;
        (p.x.round() as i64, p.y.round() as i64, p.z.round() as i64)
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for t in mesh.indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (ka, kb) = (key(a), key(b));
            if seen.insert((ka.min(kb), ka.max(kb))) {
                out.extend([a, b]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two separate cubes: elements = 2, faces = 12 quads, islands = per cube face (atlas).
    fn two_cubes() -> Mesh {
        let a = Mesh::cube();
        let mut m = a.clone();
        let off = m.vertices.len() as u32;
        m.vertices.extend(a.vertices.iter().map(|v| {
            let mut v = *v;
            v.position[0] += 3.0;
            v
        }));
        m.indices.extend(a.indices.iter().map(|i| i + off));
        m.parts[0].index_count = m.indices.len() as u32;
        m
    }

    fn distinct(v: &[u32]) -> usize {
        v.iter().collect::<std::collections::HashSet<_>>().len()
    }

    #[test]
    fn cube_faces_elements_islands() {
        let m = Mesh::cube();
        let t = Topology::build(&m);
        assert_eq!(t.face.len(), 12);
        assert_eq!(distinct(&t.face), 6, "the quads are rebuilt from triangle pairs");
        assert_eq!(distinct(&t.element), 1, "one connected cube");
        assert_eq!(distinct(&t.island), 6, "atlas: one island per face");
        assert_eq!(t.triangles_with(SelectMode::Face, 0).len(), 2);
        assert_eq!(t.triangles_with(SelectMode::Element, 0).len(), 12);
        let m2 = two_cubes();
        let t2 = Topology::build(&m2);
        assert_eq!(distinct(&t2.element), 2, "two unattached parts");
        assert_eq!(t2.triangles_with(SelectMode::Element, 13).len(), 12);
    }

    #[test]
    fn sphere_islands_and_picking() {
        let m = Mesh::uv_sphere(16, 8);
        let t = Topology::build(&m);
        assert_eq!(distinct(&t.island), 1, "one UV island");
        assert_eq!(distinct(&t.element), 1);
        // Ray from +Z hits the front of the sphere at distance ~4.
        let (tri, d) = pick_ray(&m, Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        assert!((d - 4.0).abs() < 0.05, "{d}");
        let c: Vec3 = (0..3).map(|k| Vec3::from(m.vertices[m.indices[3 * tri as usize + k] as usize].position)).sum::<Vec3>() / 3.0;
        assert!(c.z > 0.8, "front triangle {c}");
        assert!(pick_ray(&m, Vec3::new(0.0, 5.0, 5.0), Vec3::Z).is_none());
        // UV pick returns a triangle containing the point.
        let tri = pick_uv(&m, 0, Vec2::new(0.3, 0.6)).unwrap();
        let uv: Vec<Vec2> = (0..3).map(|k| Vec2::from(m.vertices[m.indices[3 * tri as usize + k] as usize].uv)).collect();
        let (lo, hi) = (uv.iter().fold(Vec2::splat(9.0), |a, b| a.min(*b)), uv.iter().fold(Vec2::splat(-9.0), |a, b| a.max(*b)));
        assert!(lo.x <= 0.3 && hi.x >= 0.3 && lo.y <= 0.6 && hi.y >= 0.6);
        assert_eq!(triangle_indices(&m, &[tri]).len(), 3);
        assert!(!wire_edges(&m).is_empty());
    }
}
