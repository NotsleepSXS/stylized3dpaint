//! Content library: preset materials (a layer or folder with its effects and masks, like
//! "smart materials") and preset masks (a mask stack), stored as JSON files; built-in presets;
//! fresh ids when a preset is inserted (anchor references inside it follow).
//!
//! Painted strokes are not part of a preset: they live in textures tied to one model's UVs, so
//! paint layers / paint mask items come back empty.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::document::{BlendMode, ChannelId, ChannelValues, EffectInstance, Mask, MaskItem, MaskItemKind, Node, NodeKind, ParamValue};
use crate::registry;

pub const MATERIAL_EXT: &str = "s3dmat";
pub const MASK_EXT: &str = "s3dmask";
const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaterialPreset {
    #[serde(default = "format")]
    pub format: u32,
    pub name: String,
    pub node: Node,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaskPreset {
    #[serde(default = "format")]
    pub format: u32,
    pub name: String,
    pub mask: Mask,
}

fn format() -> u32 {
    FORMAT
}

/// Where a preset comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    File(PathBuf),
}

/// A file name for `name` (letters, digits, spaces, - and _; the rest becomes _).
pub fn file_stem(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.trim().is_empty() { "preset".into() } else { s.trim().to_owned() }
}

/// A free path `dir/<stem>.<ext>` (adds " 2", " 3"… when taken).
pub fn free_path(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let stem = file_stem(name);
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem} {n}.{ext}"));
        n += 1;
    }
    p
}

pub fn save_material(path: &Path, preset: &MaterialPreset) -> std::io::Result<()> {
    write_json(path, preset)
}

pub fn save_mask(path: &Path, preset: &MaskPreset) -> std::io::Result<()> {
    write_json(path, preset)
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let json = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

pub fn load_material(path: &Path) -> Option<MaterialPreset> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn load_mask(path: &Path) -> Option<MaskPreset> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// All presets with extension `ext` in `dir`, sorted by name.
pub fn list<T>(dir: &Path, ext: &str, load: impl Fn(&Path) -> Option<T>) -> Vec<(PathBuf, T)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, T)> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == ext)).filter_map(|p| load(&p).map(|v| (p, v))).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Gives every node, effect and mask item in `node` a new id from `alloc`; anchor parameters
/// pointing inside the preset follow, others are cleared. Returns the old -> new effect ids.
pub fn fresh_ids(node: &mut Node, alloc: &mut dyn FnMut() -> u64) -> HashMap<u64, u64> {
    let mut map = HashMap::new();
    fn walk(n: &mut Node, alloc: &mut dyn FnMut() -> u64, map: &mut HashMap<u64, u64>) {
        n.id = alloc();
        if let NodeKind::Effect(e) = &mut n.kind {
            let new = alloc();
            map.insert(e.id, new);
            e.id = new;
        }
        for e in &mut n.effects {
            let new = alloc();
            map.insert(e.id, new);
            e.id = new;
        }
        if let Some(m) = &mut n.mask {
            fresh_mask_ids(m, alloc, map);
        }
        for c in &mut n.children {
            walk(c, alloc, map);
        }
    }
    walk(node, alloc, &mut map);
    remap_anchors_node(node, &map);
    map
}

fn fresh_mask_ids(m: &mut Mask, alloc: &mut dyn FnMut() -> u64, map: &mut HashMap<u64, u64>) {
    for item in &mut m.items {
        item.id = alloc();
        if let MaskItemKind::Effect(e) = &mut item.kind {
            let new = alloc();
            map.insert(e.id, new);
            e.id = new;
        }
    }
}

/// Fresh ids for a mask preset applied to a layer (anchors outside it are cleared).
pub fn fresh_mask(mask: &mut Mask, alloc: &mut dyn FnMut() -> u64) {
    let mut map = HashMap::new();
    fresh_mask_ids(mask, alloc, &mut map);
    for item in &mut mask.items {
        if let MaskItemKind::Effect(e) = &mut item.kind {
            remap_anchors(e, &map);
        }
    }
}

fn remap_anchors(e: &mut EffectInstance, map: &HashMap<u64, u64>) {
    for v in e.params.values_mut() {
        if let ParamValue::Anchor(a) = v {
            *a = a.and_then(|id| map.get(&id).copied());
        }
    }
}

fn remap_anchors_node(n: &mut Node, map: &HashMap<u64, u64>) {
    if let NodeKind::Effect(e) = &mut n.kind {
        remap_anchors(e, map);
    }
    for e in &mut n.effects {
        remap_anchors(e, map);
    }
    if let Some(m) = &mut n.mask {
        for item in &mut m.items {
            if let MaskItemKind::Effect(e) = &mut item.kind {
                remap_anchors(e, map);
            }
        }
    }
    for c in &mut n.children {
        remap_anchors_node(c, map);
    }
}

// --------------------------------------------------------------------------- built-ins

fn effect(id: &str, params: &[(&str, ParamValue)]) -> Option<EffectInstance> {
    let mut e = registry().get(id)?.instantiate(0);
    for (k, v) in params {
        e.params.insert((*k).into(), v.clone());
    }
    Some(e)
}

fn fill(name: &str, color: [f32; 3], rough: f32, metal: f32, effects: Vec<Option<EffectInstance>>) -> Node {
    let mut v = ChannelValues::defaults();
    v.set(ChannelId::BaseColor, color);
    v.set(ChannelId::Roughness, [rough; 3]);
    v.set(ChannelId::Metallic, [metal; 3]);
    let mut n = Node::new(0, name, NodeKind::Fill(v));
    n.effects = effects.into_iter().flatten().collect();
    n
}

/// Built-in preset materials (Russian names; the UI shows them as they are).
pub fn builtin_materials() -> Vec<MaterialPreset> {
    let m = |name: &str, node: Node| MaterialPreset { format: FORMAT, name: name.into(), node };
    vec![
        m("Окрашенный металл", fill("Окрашенный металл", [0.42, 0.06, 0.04], 0.55, 0.0, vec![effect("metal_edge_wear", &[])])),
        m("Ржавый металл", fill("Ржавый металл", [0.28, 0.28, 0.3], 0.4, 1.0, vec![effect("rust", &[])])),
        m("Облупленная краска", fill("Облупленная краска", [0.12, 0.3, 0.42], 0.6, 0.0, vec![effect("paint_peel", &[])])),
        m("Поцарапанный пластик", fill("Поцарапанный пластик", [0.75, 0.62, 0.08], 0.35, 0.0, vec![effect("scratches", &[])])),
        m("Стилизованное дерево", fill("Стилизованное дерево", [0.35, 0.17, 0.07], 0.7, 0.0, vec![effect("wood_grain", &[]), effect("wood_edge_wear", &[])])),
        m("Hand-painted камень", fill("Hand-painted камень", [0.3, 0.3, 0.32], 0.85, 0.0, vec![effect("stone_chips", &[]), effect("cavity_darken", &[]), effect("edge_highlight", &[])])),
        m("Тун (cel-shading)", fill("Тун", [0.6, 0.2, 0.15], 0.6, 0.0, vec![effect("toon_shading", &[])])),
        m("Мазки кистью", fill("Мазки кистью", [0.55, 0.32, 0.2], 0.65, 0.0, vec![effect("noise", &[]).map(|mut e| {
            e.blend = BlendMode::Overlay;
            e.opacity = 0.6;
            e
        }), effect("stylize_master", &[])])),
        m("Грязный металл", {
            let mut f = Node::new(0, "Грязный металл", NodeKind::Folder { expanded: true });
            let base = fill("Металл", [0.55, 0.55, 0.57], 0.35, 1.0, vec![]);
            let mut dirt = fill("Грязь", [0.12, 0.09, 0.06], 0.9, 0.0, vec![]);
            dirt.mask = Some(mask_with(effect("dirt", &[])));
            f.children = vec![base, dirt];
            f
        }),
    ]
}

fn mask_with(e: Option<EffectInstance>) -> Mask {
    let mut items = vec![MaskItem { id: 0, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Fill(0.0) }];
    if let Some(e) = e {
        let blend = e.blend;
        items.push(MaskItem { id: 0, enabled: true, opacity: 1.0, blend, kind: MaskItemKind::Effect(e) });
    }
    items.push(MaskItem { id: 0, enabled: true, opacity: 1.0, blend: BlendMode::Normal, kind: MaskItemKind::Paint });
    Mask { enabled: true, items }
}

/// Built-in preset masks.
pub fn builtin_masks() -> Vec<MaskPreset> {
    let m = |name: &str, id: &str| MaskPreset { format: FORMAT, name: name.into(), mask: mask_with(effect(id, &[])) };
    vec![
        m("Износ по краям", "curvature"),
        m("Грязь в полостях", "dirt"),
        m("Пыль сверху", "dust"),
        m("Подтёки", "leaks"),
        m("Затенение (AO)", "ambient_occlusion"),
        m("Освещённые места", "light"),
        m("Толщина", "thickness"),
        m("Шум", "noise"),
        m("Градиент по высоте", "position_gradient"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_use_registered_effects() {
        for p in builtin_materials() {
            let mut count = 0;
            p.node.visit(&mut |n| count += n.effects.len() + n.mask.as_ref().map_or(0, |m| m.items.len()));
            assert!(count > 0, "{}: has effects", p.name);
        }
        for p in builtin_masks() {
            assert!(p.mask.items.iter().any(|i| matches!(i.kind, MaskItemKind::Effect(_))), "{}: generator", p.name);
        }
    }

    #[test]
    fn fresh_ids_are_unique_and_anchors_follow() {
        let mut node = builtin_materials().pop().unwrap().node;
        // Anchor inside: the dirt mask effect anchors the base layer's (fake) effect.
        let mut a = effect("noise", &[]).unwrap();
        a.id = 77;
        a.anchor = true;
        node.children[0].effects.push(a);
        let mut r = effect("anchor_reference", &[]).unwrap();
        for v in r.params.values_mut() {
            if let ParamValue::Anchor(x) = v {
                *x = Some(77);
            }
        }
        r.params.insert("outside".into(), ParamValue::Anchor(Some(999)));
        node.children[1].effects.push(r);
        let mut next = 1000u64;
        let map = fresh_ids(&mut node, &mut || {
            next += 1;
            next
        });
        let mut ids = Vec::new();
        node.visit(&mut |n| {
            ids.push(n.id);
            ids.extend(n.effects.iter().map(|e| e.id));
            ids.extend(n.mask.iter().flat_map(|m| m.items.iter().map(|i| i.id)));
        });
        let uniq: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(uniq.len(), ids.len(), "unique ids");
        let new_anchor = map[&77];
        let refp = &node.children[1].effects.last().unwrap().params;
        assert!(refp.values().any(|v| *v == ParamValue::Anchor(Some(new_anchor))), "inside anchor follows");
        assert_eq!(refp.get("outside"), Some(&ParamValue::Anchor(None)), "outside anchor cleared");
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("s3dp-lib-{}", std::process::id()));
        let p = builtin_materials().remove(0);
        let path = free_path(&dir, &p.name, MATERIAL_EXT);
        save_material(&path, &p).unwrap();
        let path2 = free_path(&dir, &p.name, MATERIAL_EXT);
        assert_ne!(path, path2, "free_path avoids overwriting");
        let back = load_material(&path).unwrap();
        assert_eq!(back.node, p.node);
        assert_eq!(list(&dir, MATERIAL_EXT, load_material).len(), 1);
        assert_eq!(file_stem("a/b:c?"), "a_b_c_");
        std::fs::remove_dir_all(&dir).ok();
    }
}
