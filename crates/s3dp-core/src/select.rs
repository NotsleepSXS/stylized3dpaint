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
        // Collapsed UV triangles (hidden faces) contain nothing.
        if cross(a, b, c).abs() < 1e-12 {
            continue;
        }
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

/// Unique edges by vertex index (for the UV-space wireframe: seam vertices are separate
/// there, so both sides of a seam keep their lines).
pub fn wire_edges_uv(mesh: &Mesh) -> Vec<u32> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for t in mesh.indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if seen.insert((a.min(b), a.max(b))) {
                out.extend([a, b]);
            }
        }
    }
    out
}

/// Edges to draw for `mode` (vertex index pairs): edges between different groups and open
/// borders. Face mode hides the diagonals inside quads, Island mode shows the island borders,
/// Part mode the open borders of each part. `uv_space` keys adjacency by vertex index (seams
/// are borders in the UV view) instead of by 3D position.
pub fn boundary_edges(mesh: &Mesh, topo: &Topology, mode: SelectMode, uv_space: bool) -> Vec<u32> {
    boundary_edges_by_set(mesh, topo, mode, uv_space).concat()
}

/// [`boundary_edges`] split per texture set (index = set); edges of different sets never
/// join, even where their UVs coincide.
pub fn boundary_edges_by_set(mesh: &Mesh, topo: &Topology, mode: SelectMode, uv_space: bool) -> Vec<Vec<u32>> {
    let b = mesh.bounds();
    let q = ((b.max - b.min).max_element() * 1e-5).max(1e-9);
    let key = |v: u32| -> (i64, i64, i64) {
        if uv_space {
            // By UV: vertices split only for hard normals (same UV) stay joined.
            let u = Vec2::from(mesh.vertices[v as usize].uv) * 1e6;
            (u.x.round() as i64, u.y.round() as i64, 0)
        } else {
            let p = Vec3::from(mesh.vertices[v as usize].position) / q;
            (p.x.round() as i64, p.y.round() as i64, p.z.round() as i64)
        }
    };
    let groups = match mode {
        SelectMode::Face => &topo.face,
        SelectMode::Element => &topo.element,
        SelectMode::Island => &topo.island,
    };
    // (set, edge key) -> (first occurrence as vertex pair, groups of the triangles using it)
    type Key = (u32, (i64, i64, i64), (i64, i64, i64));
    let mut edges: HashMap<Key, ((u32, u32), Vec<u32>)> = HashMap::new();
    for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
        let g = groups.get(t).copied().unwrap_or(t as u32);
        let set = topo.set.get(t).copied().unwrap_or(0);
        for (a, c) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let (ka, kc) = (key(a), key(c));
            let e = edges.entry((set, ka.min(kc), ka.max(kc))).or_insert(((a, c), Vec::new()));
            e.1.push(g);
        }
    }
    let mut out = vec![Vec::new(); mesh.parts.len().max(1)];
    for ((set, _, _), ((a, c), gs)) in edges {
        let border = gs.len() == 1 || gs.iter().any(|g| *g != gs[0]);
        if border && let Some(list) = out.get_mut(set as usize) {
            list.extend([a, c]);
        }
    }
    out
}

/// A distinct, stable colour per group (RGBA8) for tinting parts / islands.
pub fn group_color(group: u32) -> [u8; 4] {
    // Golden-ratio hue steps, fixed saturation / value.
    let h = (group as f32 * 0.618_034).fract() * 6.0;
    let (s, v) = (0.55f32, 0.95f32);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let f = |u: f32| ((u + m) * 255.0).round() as u8;
    [f(r), f(g), f(b), 255]
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
    fn boundary_edges_per_mode() {
        let m = Mesh::cube();
        let t = Topology::build(&m);
        // Face mode: 6 quads -> 12 cube edges (diagonals hidden).
        assert_eq!(boundary_edges(&m, &t, SelectMode::Face, false).len() / 2, 12);
        // Atlas islands: every cube edge is an island border too.
        assert_eq!(boundary_edges(&m, &t, SelectMode::Island, false).len() / 2, 12);
        // One closed part: no open border, nothing to draw.
        assert_eq!(boundary_edges(&m, &t, SelectMode::Element, false).len(), 0);
        // Sphere: one island, its seam / poles show in the UV view only.
        let s = Mesh::uv_sphere(16, 8);
        let ts = Topology::build(&s);
        assert!(boundary_edges(&s, &ts, SelectMode::Island, false).is_empty() || boundary_edges(&s, &ts, SelectMode::Island, false).len() < boundary_edges(&s, &ts, SelectMode::Island, true).len());
        assert!(!boundary_edges(&s, &ts, SelectMode::Island, true).is_empty());
        assert_ne!(group_color(1), group_color(2));
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

#[cfg(test)]
mod perf {
    /// `cargo test --release -p s3dp-core pick_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn pick_timing() {
        let m = crate::Mesh::uv_sphere(512, 256); // ~260k triangles
        let t = std::time::Instant::now();
        for _ in 0..10 {
            let _ = super::pick_ray(&m, glam::Vec3::new(0.0, 0.0, 5.0), glam::Vec3::NEG_Z);
        }
        println!("{} tris: {:?} per pick", m.indices.len() / 3, t.elapsed() / 10);
        let t = std::time::Instant::now();
        let topo = super::Topology::build(&m);
        println!("topology {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let _ = topo.triangles_with(super::SelectMode::Face, 0);
        println!("group {:?}", t.elapsed());
    }
}

#[cfg(test)]
mod review_fixes {
    use super::*;

    #[test]
    fn collapsed_uv_triangle_does_not_catch_clicks() {
        let mut m = Mesh::cube();
        // Collapse the first triangle's UVs to one point.
        for k in 0..3 {
            let v = m.indices[k] as usize;
            m.vertices[v].uv = [0.9, 0.9];
        }
        // Duplicate the collapsed vertices so other triangles keep their UVs.
        let t = pick_uv(&m, 0, Vec2::new(0.5, 0.75));
        assert_ne!(t, Some(0), "degenerate triangle 0 must not be picked");
    }

    #[test]
    fn hard_edge_splits_are_not_uv_borders() {
        // Split every vertex per triangle (flat shading, same UVs): island borders in UV must
        // stay the island outline, not every polygon edge.
        let s = Mesh::uv_sphere(12, 6);
        let mut flat = s.clone();
        flat.vertices = s.indices.iter().map(|&i| s.vertices[i as usize]).collect();
        flat.indices = (0..flat.vertices.len() as u32).collect();
        let (ts, tf) = (Topology::build(&s), Topology::build(&flat));
        let a = boundary_edges(&s, &ts, SelectMode::Island, true).len();
        let b = boundary_edges(&flat, &tf, SelectMode::Island, true).len();
        assert_eq!(a, b, "same island outline with and without the normal splits");
    }
}
