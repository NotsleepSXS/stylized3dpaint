use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};

/// GPU-ready vertex. Layout must match `s3dp-render` shaders.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// MikkTSpace tangent; w is the bitangent sign.
    pub tangent: [f32; 4],
}

/// A contiguous index range sharing one material (a future texture set).
#[derive(Clone, Debug)]
pub struct MeshPart {
    pub name: String,
    pub first_index: u32,
    pub index_count: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn radius(&self) -> f32 {
        ((self.max - self.min).length() * 0.5).max(1e-4)
    }
}

/// Triangle mesh flattened into world space.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub name: String,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub parts: Vec<MeshPart>,
}

impl Mesh {
    pub fn bounds(&self) -> Aabb {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for v in &self.vertices {
            let p = Vec3::from(v.position);
            min = min.min(p);
            max = max.max(p);
        }
        if self.vertices.is_empty() {
            min = Vec3::ZERO;
            max = Vec3::ZERO;
        }
        Aabb { min, max }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn has_uvs(&self) -> bool {
        self.vertices.iter().any(|v| v.uv != [0.0, 0.0])
    }

    /// Merges bit-identical vertices (FBX import yields one vertex per corner).
    pub fn weld(&mut self) {
        let mut map = std::collections::HashMap::with_capacity(self.vertices.len());
        let mut vertices = Vec::with_capacity(self.vertices.len());
        let mut remap = Vec::with_capacity(self.vertices.len());
        for v in &self.vertices {
            let key: [u32; 12] = bytemuck::cast(*v);
            let idx = *map.entry(key).or_insert_with(|| {
                vertices.push(*v);
                vertices.len() as u32 - 1
            });
            remap.push(idx);
        }
        for i in &mut self.indices {
            *i = remap[*i as usize];
        }
        self.vertices = vertices;
    }

    /// Recomputes smooth area-weighted normals (used when the file has none).
    pub fn compute_normals(&mut self) {
        let mut acc = vec![Vec3::ZERO; self.vertices.len()];
        for tri in self.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let pa = Vec3::from(self.vertices[a].position);
            let pb = Vec3::from(self.vertices[b].position);
            let pc = Vec3::from(self.vertices[c].position);
            let n = (pb - pa).cross(pc - pa);
            acc[a] += n;
            acc[b] += n;
            acc[c] += n;
        }
        for (v, n) in self.vertices.iter_mut().zip(acc) {
            v.normal = n.normalize_or(Vec3::Y).into();
        }
    }

    /// UV sphere used as the default preview mesh.
    pub fn uv_sphere(segments: u32, rings: u32) -> Mesh {
        let mut vertices = Vec::new();
        for r in 0..=rings {
            let v = r as f32 / rings as f32;
            let theta = v * std::f32::consts::PI;
            for s in 0..=segments {
                let u = s as f32 / segments as f32;
                let phi = u * std::f32::consts::TAU;
                let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
                vertices.push(Vertex { position: n.into(), normal: n.into(), uv: Vec2::new(u, v).into(), tangent: [0.0; 4] });
            }
        }
        let mut indices = Vec::new();
        let stride = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let i0 = r * stride + s;
                let i1 = i0 + stride;
                indices.extend_from_slice(&[i0, i0 + 1, i1, i0 + 1, i1 + 1, i1]);
            }
        }
        let index_count = indices.len() as u32;
        let mut mesh = Mesh {
            name: "Sphere".into(),
            vertices,
            indices,
            parts: vec![MeshPart { name: "default".into(), first_index: 0, index_count }],
        };
        mesh.compute_tangents();
        mesh
    }

    /// Generates MikkTSpace tangents (matching Blender/Unreal/Unity bakers) and re-welds.
    /// Triangle order is preserved, so part index ranges stay valid.
    pub fn compute_tangents(&mut self) {
        struct Corners<'a> {
            mesh: &'a Mesh,
            out: Vec<Vertex>,
        }
        impl mikktspace::Geometry for Corners<'_> {
            fn num_faces(&self) -> usize {
                self.mesh.indices.len() / 3
            }
            fn num_vertices_of_face(&self, _face: usize) -> usize {
                3
            }
            fn position(&self, face: usize, vert: usize) -> [f32; 3] {
                self.out[face * 3 + vert].position
            }
            fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
                self.out[face * 3 + vert].normal
            }
            fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
                self.out[face * 3 + vert].uv
            }
            fn set_tangent(&mut self, tangent: Option<mikktspace::TangentSpace>, face: usize, vert: usize) {
                self.out[face * 3 + vert].tangent = tangent.unwrap_or_default().tangent_encoded();
            }
        }
        let out: Vec<Vertex> = self.indices.iter().map(|&i| self.vertices[i as usize]).collect();
        let mut corners = Corners { mesh: self, out };
        if let Err(e) = mikktspace::generate_tangents(&mut corners) {
            log::warn!("tangent generation failed: {e:?}");
        }
        let out = corners.out;
        self.indices = (0..out.len() as u32).collect();
        self.vertices = out;
        self.weld();
    }
}
