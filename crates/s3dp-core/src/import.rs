//! Model import: glTF/GLB, OBJ and FBX, flattened into a single world-space [`Mesh`].

use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Mat3, Mat4, Vec3};

use crate::mesh::{Mesh, MeshPart, Vertex};

pub const SUPPORTED_EXTENSIONS: &[&str] = &["gltf", "glb", "obj", "fbx"];

pub fn load(path: &Path) -> Result<Mesh> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut mesh = match ext.as_str() {
        "gltf" | "glb" => load_gltf(path),
        "obj" => load_obj(path),
        "fbx" => load_fbx(path),
        _ => bail!("unsupported format: .{ext}"),
    }
    .with_context(|| format!("failed to load {}", path.display()))?;
    if mesh.indices.is_empty() {
        bail!("{} contains no triangles", path.display());
    }
    mesh.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh").to_owned();
    if mesh.vertices.iter().all(|v| v.normal == [0.0; 3]) {
        mesh.compute_normals();
    }
    mesh.compute_tangents();
    Ok(mesh)
}

/// Accumulates triangles into parts keyed by material name.
#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    parts: Vec<(String, Vec<u32>)>,
}

impl Builder {
    fn part(&mut self, name: &str) -> usize {
        match self.parts.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                self.parts.push((name.to_owned(), Vec::new()));
                self.parts.len() - 1
            }
        }
    }

    fn finish(self) -> Mesh {
        let mut indices = Vec::new();
        let mut parts = Vec::new();
        for (name, idx) in self.parts {
            if idx.is_empty() {
                continue;
            }
            parts.push(MeshPart { name, first_index: indices.len() as u32, index_count: idx.len() as u32 });
            indices.extend(idx);
        }
        Mesh { name: String::new(), vertices: self.vertices, indices, parts }
    }
}

fn load_gltf(path: &Path) -> Result<Mesh> {
    let (doc, buffers, _images) = gltf::import(path)?;
    let mut b = Builder::default();
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).context("no scene")?;
    let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        let normal_mat = Mat3::from_mat4(world).inverse().transpose();
        if let Some(gm) = node.mesh() {
            for prim in gm.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let reader = prim.reader(|buf| Some(&buffers[buf.index()]));
                let Some(positions) = reader.read_positions() else { continue };
                let positions: Vec<[f32; 3]> = positions.collect();
                let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|n| n.collect());
                let uvs: Option<Vec<[f32; 2]>> = reader.read_tex_coords(0).map(|t| t.into_f32().collect());
                let base = b.vertices.len() as u32;
                for (i, p) in positions.iter().enumerate() {
                    let n = normals.as_ref().map(|n| (normal_mat * Vec3::from(n[i])).normalize_or_zero());
                    b.vertices.push(Vertex {
                        position: world.transform_point3(Vec3::from(*p)).into(),
                        normal: n.unwrap_or(Vec3::ZERO).into(),
                        uv: uvs.as_ref().map_or([0.0; 2], |t| t[i]),
                        tangent: [0.0; 4],
                    });
                }
                let idx: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().map(|i| base + i).collect(),
                    None => (base..base + positions.len() as u32).collect(),
                };
                let mat = prim.material().name().unwrap_or("default").to_owned();
                let flip = world.determinant() < 0.0;
                let p = b.part(&mat);
                push_tris(&mut b.parts[p].1, &idx, flip);
            }
        }
        stack.extend(node.children().map(|c| (c, world)));
    }
    Ok(b.finish())
}

fn load_obj(path: &Path) -> Result<Mesh> {
    let (models, materials) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS)?;
    let materials = materials.unwrap_or_default();
    let mut b = Builder::default();
    for model in models {
        let m = &model.mesh;
        let base = b.vertices.len() as u32;
        let count = m.positions.len() / 3;
        for i in 0..count {
            b.vertices.push(Vertex {
                position: [m.positions[i * 3], m.positions[i * 3 + 1], m.positions[i * 3 + 2]],
                normal: if m.normals.len() >= (i + 1) * 3 {
                    [m.normals[i * 3], m.normals[i * 3 + 1], m.normals[i * 3 + 2]]
                } else {
                    [0.0; 3]
                },
                uv: if m.texcoords.len() >= (i + 1) * 2 {
                    [m.texcoords[i * 2], 1.0 - m.texcoords[i * 2 + 1]]
                } else {
                    [0.0; 2]
                },
                tangent: [0.0; 4],
            });
        }
        let name = m
            .material_id
            .and_then(|id| materials.get(id))
            .map_or_else(|| model.name.clone(), |mat| mat.name.clone());
        let idx: Vec<u32> = m.indices.iter().map(|i| base + i).collect();
        let p = b.part(&name);
        push_tris(&mut b.parts[p].1, &idx, false);
    }
    Ok(b.finish())
}

fn load_fbx(path: &Path) -> Result<Mesh> {
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        generate_missing_normals: true,
        ..Default::default()
    };
    let path_str = path.to_str().context("non-UTF-8 path")?;
    let scene = ufbx::load_file(path_str, opts).map_err(|e| anyhow::anyhow!("{:?}", e.description))?;
    let mut b = Builder::default();
    let mut tri = Vec::new();
    for node in scene.nodes.iter() {
        let Some(mesh) = node.mesh.as_ref() else { continue };
        let to_world = &node.geometry_to_world;
        let normal_mat = ufbx::get_compatible_matrix_for_normals(node);
        let flip = mat_det(to_world) < 0.0;
        tri.resize(mesh.max_face_triangles * 3, 0);
        for (fi, face) in mesh.faces.iter().enumerate() {
            let mat_name = mesh
                .face_material
                .get(fi)
                .and_then(|&m| mesh.materials.get(m as usize))
                .map_or_else(|| "default".to_owned(), |m| m.element.name.to_string());
            let n = ufbx::triangulate_face(&mut tri, mesh, *face) as usize;
            let mut idx = Vec::with_capacity(n * 3);
            for &ix in &tri[..n * 3] {
                let ix = ix as usize;
                let p = ufbx::transform_position(to_world, mesh.vertex_position[ix]);
                let nrm = if mesh.vertex_normal.exists {
                    let n = ufbx::transform_direction(&normal_mat, mesh.vertex_normal[ix]);
                    Vec3::new(n.x as f32, n.y as f32, n.z as f32).normalize_or_zero()
                } else {
                    Vec3::ZERO
                };
                let uv = if mesh.vertex_uv.exists {
                    let t = mesh.vertex_uv[ix];
                    [t.x as f32, 1.0 - t.y as f32]
                } else {
                    [0.0; 2]
                };
                idx.push(b.vertices.len() as u32);
                b.vertices.push(Vertex {
                    position: [p.x as f32, p.y as f32, p.z as f32],
                    normal: nrm.into(),
                    uv,
                    tangent: [0.0; 4],
                });
            }
            let p = b.part(&mat_name);
            push_tris(&mut b.parts[p].1, &idx, flip);
        }
    }
    let mut mesh = b.finish();
    mesh.weld();
    Ok(mesh)
}

fn push_tris(out: &mut Vec<u32>, idx: &[u32], flip: bool) {
    for t in idx.chunks_exact(3) {
        if flip {
            out.extend_from_slice(&[t[0], t[2], t[1]]);
        } else {
            out.extend_from_slice(t);
        }
    }
}

fn mat_det(m: &ufbx::Matrix) -> f64 {
    m.m00 * (m.m11 * m.m22 - m.m12 * m.m21) - m.m01 * (m.m10 * m.m22 - m.m12 * m.m20)
        + m.m02 * (m.m10 * m.m21 - m.m11 * m.m20)
}
