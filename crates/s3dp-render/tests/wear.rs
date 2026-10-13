//! Headless GPU tests for the spec §4 wear generators: every one compiles and gives a sensible
//! mask on a baked mesh with sharp edges, writes several channels on layer content, wear is
//! uneven along edges with breakup, hand-painted mode is hard-edged, no UV seam, seeds are
//! deterministic. Small textures / views and a cheap bake to keep the GPU cool.

use glam::Vec3;
use s3dp_core::{BakeSettings, BlendMode, ChannelId, ChannelValues, Mask, MaskItem, MaskItemKind, Mesh, NodeKind, OrbitCamera, ParamValue};
use s3dp_render::{ShadingMode, Viewport, wgpu};

const VIEW: [u32; 2] = [96, 96];

/// Spec §4 P0 wear generators.
const WEAR: [&str; 5] = ["metal_edge_wear", "paint_peel", "rust", "wood_edge_wear", "scratches"];
/// P1.
const EXTRA: [&str; 2] = ["stone_chips", "wood_grain"];

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = s3dp_render::gpu::new_instance();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

struct Scene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vp: Viewport,
    cam: OrbitCamera,
    tex: u32,
}

struct Out {
    v: Vec<f32>,
    error: Option<String>,
}

impl Scene {
    fn new(mesh: &Mesh, tex: u32) -> Option<Self> {
        let (device, queue) = device()?;
        let mut vp = Viewport::new(&device);
        vp.paint.throttle = false;
        vp.set_mesh(&device, &queue, mesh, tex);
        let mut cam = OrbitCamera::default();
        cam.frame(Vec3::ZERO, 1.0, 1.0);
        vp.paint.edit_document(false, |d| d.defaults.set(ChannelId::BaseColor, [0.0; 3]));
        let mut s = Self { device, queue, vp, cam, tex };
        s.render();
        Some(s)
    }

    fn baked(mesh: &Mesh, tex: u32) -> Option<Self> {
        let mut s = Self::new(mesh, tex)?;
        let settings = BakeSettings { ao_samples: 8, edge_samples: 8, thickness_samples: 8, sky_samples: 8, ..BakeSettings::default() };
        s.vp.bake_now(&s.device, &s.queue, mesh, None, &settings);
        Some(s)
    }

    fn render(&mut self) {
        self.vp.render(&self.device, &self.queue, VIEW, &self.cam, ShadingMode::Unlit);
    }

    fn instance(&mut self, def: &str, params: &[(&str, ParamValue)]) -> s3dp_core::EffectInstance {
        let mut e = self.vp.paint.new_effect(def).unwrap_or_else(|| panic!("effect {def} not registered"));
        for (k, v) in params {
            assert!(e.params.contains_key(*k), "{def}: unknown param {k}");
            e.params.insert((*k).into(), v.clone());
        }
        e
    }

    /// White fill masked by the generator: returns the mask.
    fn mask(&mut self, def: &str, params: &[(&str, ParamValue)]) -> Out {
        let e = self.instance(def, params);
        let eid = e.id;
        let mut white = ChannelValues::defaults();
        white.set(ChannelId::BaseColor, [1.0; 3]);
        let node = self.vp.paint.add_node(NodeKind::Fill(white), |n, d| {
            n.mask = Some(Mask {
                enabled: true,
                items: vec![MaskItem { id: d.alloc_id(), enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Effect(e) }],
            })
        });
        self.render();
        let data = self.vp.paint.read_final(&self.device, &self.queue, 0, ChannelId::BaseColor).unwrap().1;
        let error = self.vp.paint.errors.get(&eid).cloned();
        self.vp.paint.delete_node(node);
        Out { v: data.chunks_exact(4).map(|p| p[0]).collect(), error }
    }

    /// Default fill with the generator on its content: returns the given channels (r only).
    fn content(&mut self, def: &str, params: &[(&str, ParamValue)], channels: &[ChannelId]) -> (Vec<Vec<f32>>, Option<String>) {
        let e = self.instance(def, params);
        let eid = e.id;
        let node = self.vp.paint.add_node(NodeKind::Fill(ChannelValues::defaults()), |n, _| n.effects.push(e));
        self.render();
        let out = channels
            .iter()
            .map(|c| self.vp.paint.read_final(&self.device, &self.queue, 0, *c).map(|d| d.1.chunks_exact(4).map(|p| p[0]).collect()).unwrap_or_default())
            .collect();
        let error = self.vp.paint.errors.get(&eid).cloned();
        self.vp.paint.delete_node(node);
        (out, error)
    }
}

struct Stats {
    min: f32,
    max: f32,
    mean: f32,
    std: f32,
}

fn stats(v: &[f32]) -> Stats {
    let n = v.len().max(1) as f32;
    let mean = v.iter().sum::<f32>() / n;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    Stats { min: v.iter().cloned().fold(f32::MAX, f32::min), max: v.iter().cloned().fold(f32::MIN, f32::max), mean, std: var.sqrt() }
}

fn check_ok(label: &str, o: &Out) -> Stats {
    assert!(o.error.is_none(), "{label}: {}", o.error.as_deref().unwrap_or(""));
    assert!(o.v.iter().all(|x| x.is_finite()), "{label}: non-finite values");
    let s = stats(&o.v);
    println!("{label:32} min {:.3} max {:.3} mean {:.3} std {:.3}", s.min, s.max, s.mean, s.std);
    assert!(s.min >= -0.002 && s.max <= 1.002, "{label}: out of 0..1 ({} .. {})", s.min, s.max);
    s
}

/// Share of texels with in-between values.
fn soft_share(v: &[f32]) -> f32 {
    v.iter().filter(|x| **x > 0.1 && **x < 0.9).count() as f32 / v.len() as f32
}

/// "Star" sphere: twelve sharp vertical ridges with rounded grooves between them. `phase` turns
/// the ridges around Y (radians).
fn star_sphere(phase: f32) -> Mesh {
    let mut m = Mesh::uv_sphere(96, 48);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let phi = p.z.atan2(p.x) + phase;
        let r = 1.0 - 0.3 * (6.0 * phi).sin().abs() * (1.0 - p.y * p.y);
        v.position = (p * r).into();
    }
    m.compute_normals();
    m.compute_tangents();
    m
}

fn all_ids() -> impl Iterator<Item = &'static str> {
    WEAR.into_iter().chain(EXTRA)
}

/// Every wear effect compiles for content and mask targets (full error text on failure).
#[test]
fn wear_effects_compile() {
    let Some((device, _queue)) = device() else { return };
    let mut engine = s3dp_render::engine::effects::EffectEngine::new(&device);
    let mut failed = Vec::new();
    for id in all_ids() {
        let def = s3dp_core::registry().get(id).unwrap_or_else(|| panic!("{id} not registered"));
        assert!(def.presets.len() >= 4, "{id}: Light / Medium / Strong / Stylized presets");
        assert!(def.params.iter().all(|p| p.label_ru.is_some() && p.group_ru.is_some()), "{id}: Russian labels");
        for format in [s3dp_render::engine::effects::CONTENT_FORMAT, s3dp_render::engine::effects::MASK_FORMAT] {
            if let Err(e) = engine.pipeline(&device, def, format) {
                failed.push(format!("{id} ({format:?}): {e}"));
            }
        }
    }
    assert!(failed.is_empty(), "wear effects failed to compile:\n{}", failed.join("\n"));
}

/// Masks, content channels, breakup unevenness, hand-painted hardness and seeds on the baked star.
#[test]
fn wear_on_baked_star() {
    let mesh = star_sphere(0.0);
    let Some(mut s) = Scene::baked(&mesh, 256) else { return };

    // Edge texels: sharp convex curvature.
    let edges = s.mask("curvature", &[("mode", ParamValue::Enum(0)), ("frequency", ParamValue::Enum(0))]);
    check_ok("curvature (edge texels)", &edges);
    let edge_idx: Vec<usize> = edges.v.iter().enumerate().filter(|(_, x)| **x > 0.5).map(|(i, _)| i).collect();
    println!("edge texels: {}", edge_idx.len());
    assert!(edge_idx.len() > 500, "the star sphere should have baked edges");

    let mut flat = Vec::new();
    for id in all_ids() {
        // Mask: valid and varying.
        let o = s.mask(id, &[]);
        let st = check_ok(id, &o);
        if !(st.max - st.min > 0.2 && st.std > 0.02) {
            flat.push(id);
        }
        // Edge wear generators wear edges more than the rest.
        if id != "scratches" && id != "wood_grain" {
            let on_edges = edge_idx.iter().map(|i| o.v[*i]).sum::<f32>() / edge_idx.len() as f32;
            println!("{id:32} mean on edges {on_edges:.3} vs overall {:.3}", st.mean);
            assert!(on_edges > st.mean + 0.1, "{id}: wear should concentrate on edges");
        }

        // Content: several channels change.
        let chans = [ChannelId::BaseColor, ChannelId::Roughness, ChannelId::Metallic, ChannelId::Height];
        let defaults = ChannelValues::defaults();
        let (out, err) = s.content(id, &[], &chans);
        assert!(err.is_none(), "{id} content: {}", err.unwrap_or_default());
        let mut changed = Vec::new();
        for (c, v) in chans.iter().zip(&out) {
            assert!(v.iter().all(|x| x.is_finite()), "{id} content {c:?}: non-finite");
            let d0 = defaults.get(*c)[0];
            let dmax = v.iter().map(|x| (x - d0).abs()).fold(0.0f32, f32::max);
            if dmax > 0.02 {
                changed.push(*c);
            }
        }
        println!("{id:32} content changes {changed:?}");
        assert!(changed.len() >= 2, "{id}: content should drive several channels, got {changed:?}");
        if id != "wood_grain" {
            // Hand-painted: no height.
            let (hp, _) = s.content(id, &[("hand_painted", ParamValue::Bool(true))], &[ChannelId::Height]);
            let dh = hp[0].iter().map(|x| (x - 0.5).abs()).fold(0.0f32, f32::max);
            assert!(dh < 0.01, "{id}: hand-painted mode must not change height ({dh})");
        }
    }
    assert!(flat.is_empty(), "constant wear masks on the baked star: {flat:?}");

    // Breakup makes wear uneven along the edges.
    for id in ["metal_edge_wear", "paint_peel", "rust", "wood_edge_wear", "stone_chips"] {
        let along = |o: &Out| stats(&edge_idx.iter().map(|i| o.v[*i]).collect::<Vec<_>>());
        let even = s.mask(id, &[("breakup", ParamValue::Float(0.0)), ("level", ParamValue::Float(0.6))]);
        let patchy = s.mask(id, &[("breakup", ParamValue::Float(1.0)), ("level", ParamValue::Float(0.6))]);
        let (a, b) = (along(&even), along(&patchy));
        println!("{id:32} edges: breakup 0 mean {:.3} std {:.3} | breakup 1 mean {:.3} std {:.3}", a.mean, a.std, b.mean, b.std);
        assert!(b.std > a.std + 0.03, "{id}: breakup should make wear along edges uneven");
    }
    // Scratches: the breakup patches the scratch area (curvature weight 0 = everywhere).
    {
        let even = s.mask("scratches", &[("breakup", ParamValue::Float(0.0)), ("count", ParamValue::Float(1.0))]);
        let patchy = s.mask("scratches", &[("breakup", ParamValue::Float(1.0)), ("count", ParamValue::Float(1.0))]);
        let (a, b) = (stats(&even.v), stats(&patchy.v));
        println!("scratches breakup: 0 mean {:.4} | 1 mean {:.4}", a.mean, b.mean);
        assert!(b.mean < a.mean * 0.85, "scratches: breakup should remove scratches in patches");
    }

    // Hand-painted: hard values (few in-between) compared to the default.
    for id in all_ids() {
        let soft = s.mask(id, &[("contrast", ParamValue::Float(0.3))].iter().filter(|_| id != "wood_grain").cloned().collect::<Vec<_>>());
        let hard = s.mask(id, &[("hand_painted", ParamValue::Bool(true))]);
        check_ok(&format!("{id} hand-painted"), &hard);
        let (fs, fh) = (soft_share(&soft.v), soft_share(&hard.v));
        println!("{id:32} in-between share: default {fs:.3} hand-painted {fh:.3}");
        assert!(fh < 0.08 && fh < fs * 0.6 + 0.005, "{id}: hand-painted mode should be hard-edged");
        // Still something there.
        assert!(stats(&hard.v).max > 0.9, "{id}: hand-painted mask is empty");
    }

    // Hand-painted rim colors the base color around chips.
    {
        let p = [("hand_painted", ParamValue::Bool(true)), ("rim_width", ParamValue::Float(0.0))];
        let (plain, _) = s.content("metal_edge_wear", &p, &[ChannelId::BaseColor]);
        let p = [("hand_painted", ParamValue::Bool(true)), ("rim_width", ParamValue::Float(0.8))];
        let (rim, err) = s.content("metal_edge_wear", &p, &[ChannelId::BaseColor]);
        assert!(err.is_none());
        let diff = plain[0].iter().zip(&rim[0]).filter(|(a, b)| (*a - *b).abs() > 0.05).count();
        println!("rim changes {diff} texels");
        assert!(diff > 100, "rim width should paint a rim");
    }

    // Seeds: deterministic, different seeds differ.
    for id in all_ids() {
        let a = s.mask(id, &[("seed", ParamValue::Int(7))]);
        let b = s.mask(id, &[("seed", ParamValue::Int(7))]);
        let c = s.mask(id, &[("seed", ParamValue::Int(8))]);
        assert!(a.v == b.v, "{id}: same seed must give identical results");
        let diff = a.v.iter().zip(&c.v).filter(|(x, y)| (*x - *y).abs() > 1e-3).count();
        assert!(diff > a.v.len() / 50, "{id}: a different seed must change the mask ({diff} texels differ)");
    }
}

/// Spec §13.2: no visible seam at the sphere's u = 0 / u = 1 meridian (ridges turned away from
/// the seam so it runs over a smooth slope).
#[test]
fn wear_has_no_uv_seam() {
    let mesh = star_sphere(std::f32::consts::PI / 12.0);
    let Some(mut s) = Scene::baked(&mesh, 256) else { return };
    let tex = s.tex;
    for id in all_ids() {
        let params: Vec<(&str, ParamValue)> = if id == "wood_grain" {
            vec![]
        } else {
            // Grunge-only wear (curvature weight 0) so the 3D noise path covers the whole seam.
            vec![("curv_weight", ParamValue::Float(0.0)), ("level", ParamValue::Float(0.6)), ("ao_mask", ParamValue::Float(0.0))]
        };
        for edge_weight in [0.0, 1.0] {
            if edge_weight > 0.0 && id == "wood_grain" {
                continue;
            }
            let mut p = params.clone();
            if edge_weight > 0.0 {
                p[0].1 = ParamValue::Float(edge_weight);
            }
            let o = s.mask(id, &p);
            check_ok(&format!("{id} seam cw {edge_weight}"), &o);
            let at = |x: u32, y: u32| o.v[(y * tex + x) as usize];
            let rows = tex / 8..tex * 7 / 8;
            let n = rows.len() as f32;
            let (mut seam, mut inner) = (0.0, 0.0);
            for y in rows {
                seam += (at(0, y) - at(tex - 1, y)).abs() / n;
                // Reference: neighbours one column apart around the middle and a quarter.
                inner += ((at(tex / 2, y) - at(tex / 2 + 1, y)).abs() + (at(tex / 4, y) - at(tex / 4 + 1, y)).abs()) * 0.5 / n;
            }
            println!("{id:20} cw {edge_weight}: seam diff mean {seam:.4} (neighbours {inner:.4})");
            assert!(seam <= inner * 2.0 + 0.02, "{id}: visible UV seam");
        }
    }
}
