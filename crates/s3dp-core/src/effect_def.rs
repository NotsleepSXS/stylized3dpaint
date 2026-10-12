//! Declarative effect definitions (spec §2.5).
//!
//! Each effect is `effects/<id>.json` (metadata, parameters) + `effects/<id>.wgsl` (a body defining
//! `fn effect(c: Ctx) -> vec4<f32>`, see `s3dp-render/src/engine/prelude.wgsl`). The properties UI is
//! generated from the parameter list, and parameters reach the shader through generated accessors
//! `p_<id>()`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::document::{EffectInstance, ParamValue};

include!(concat!(env!("OUT_DIR"), "/builtin_effects.rs"));

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Builds a mask / value from mesh maps (gets the shared generator parameters).
    Generator,
    /// Material wear (also a generator).
    Wear,
    Filter,
    Color,
    Blur,
    Warp,
    Relief,
    Stylize,
    Utility,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Generator => "Generators",
            Self::Wear => "Wear",
            Self::Filter => "Filters",
            Self::Color => "Color",
            Self::Blur => "Blur",
            Self::Warp => "Warp",
            Self::Relief => "Relief",
            Self::Stylize => "Stylization",
            Self::Utility => "Utility",
        }
    }

    pub fn label_ru(self) -> &'static str {
        match self {
            Self::Generator => "Генераторы",
            Self::Wear => "Износ",
            Self::Filter => "Фильтры",
            Self::Color => "Цвет",
            Self::Blur => "Размытие",
            Self::Warp => "Искажение",
            Self::Relief => "Рельеф",
            Self::Stylize => "Стилизация",
            Self::Utility => "Служебные",
        }
    }

    /// Generators get the shared generator parameter block (spec §3).
    pub fn is_generator(self) -> bool {
        matches!(self, Self::Generator | Self::Wear)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    Float {
        min: f32,
        max: f32,
        default: f32,
        #[serde(default)]
        logarithmic: bool,
    },
    Int {
        min: i32,
        max: i32,
        default: i32,
    },
    Bool {
        default: bool,
    },
    /// Default given in sRGB 0..1 (as artists pick colors); stored linear.
    Color {
        default: [f32; 3],
    },
    Enum {
        /// (id, label, label_ru)
        options: Vec<(String, String, String)>,
        default: u32,
    },
    /// A world direction (stored normalized).
    Direction {
        default: [f32; 3],
    },
    /// Random seed (UI adds a "random" button).
    Seed {
        default: i32,
    },
    /// Reference to an anchored effect; the shader samples it with `anchor_<n>()` (n = 0/1 by order).
    Anchor,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ParamDef {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub label_ru: Option<String>,
    #[serde(flatten)]
    pub kind: ParamKind,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub group_ru: Option<String>,
    #[serde(default)]
    pub tip: Option<String>,
    #[serde(default)]
    pub tip_ru: Option<String>,
}

impl ParamDef {
    pub fn default_value(&self) -> ParamValue {
        match &self.kind {
            ParamKind::Float { default, .. } => ParamValue::Float(*default),
            ParamKind::Int { default, .. } | ParamKind::Seed { default } => ParamValue::Int(*default),
            ParamKind::Bool { default } => ParamValue::Bool(*default),
            ParamKind::Color { default } => ParamValue::Color(default.map(crate::document::srgb_to_linear)),
            ParamKind::Enum { default, .. } => ParamValue::Enum(*default),
            ParamKind::Direction { default } => {
                let d = glam::Vec3::from(*default).normalize_or(glam::Vec3::Y);
                ParamValue::Vec3(d.into())
            }
            ParamKind::Anchor => ParamValue::Anchor(None),
        }
    }

    /// Number of vec4 slots in the uniform (always one).
    pub fn wgsl_accessor(&self, slot: usize) -> String {
        let id = &self.id;
        match &self.kind {
            ParamKind::Float { .. } => format!("fn p_{id}() -> f32 {{ return U.params[{slot}].x; }}\n"),
            ParamKind::Int { .. } | ParamKind::Seed { .. } | ParamKind::Enum { .. } => {
                format!("fn p_{id}() -> i32 {{ return i32(round(U.params[{slot}].x)); }}\n")
            }
            ParamKind::Bool { .. } => format!("fn p_{id}() -> bool {{ return U.params[{slot}].x > 0.5; }}\n"),
            ParamKind::Color { .. } | ParamKind::Direction { .. } => {
                format!("fn p_{id}() -> vec3<f32> {{ return U.params[{slot}].xyz; }}\n")
            }
            ParamKind::Anchor => format!("fn p_{id}() -> bool {{ return U.params[{slot}].x > 0.5; }}\n"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Content,
    Mask,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EffectDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub name_ru: Option<String>,
    pub category: Category,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub description_ru: Option<String>,
    /// Where it may be placed (default: both).
    #[serde(default = "both_targets")]
    pub targets: Vec<Target>,
    /// Reads neighbouring texels: the engine dilates its input past UV island borders first.
    #[serde(default)]
    pub neighbors: bool,
    /// Mesh maps the effect needs (`BakedMap` file names, e.g. "curvature"); the UI warns when unbaked.
    #[serde(default)]
    pub maps: Vec<String>,
    pub params: Vec<ParamDef>,
    /// Generators on content: "gray" (value replicated, full coverage) or "value"
    /// (the effect returns the channel value in rgb and its amount in a).
    #[serde(default = "gray")]
    pub content_output: String,
    /// Named parameter presets (spec: Light / Medium / Strong / Stylized).
    #[serde(default)]
    pub presets: Vec<Preset>,
    #[serde(skip)]
    pub wgsl: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub name_ru: Option<String>,
    pub values: BTreeMap<String, serde_json::Value>,
}

fn gray() -> String {
    "gray".into()
}

fn both_targets() -> Vec<Target> {
    vec![Target::Content, Target::Mask]
}

impl EffectDef {
    pub fn display_name(&self, ru: bool) -> &str {
        if ru { self.name_ru.as_deref().unwrap_or(&self.name) } else { &self.name }
    }

    /// Parameters including the shared generator block (spec §3) for generators.
    pub fn all_params(&self) -> Vec<ParamDef> {
        let mut out = self.params.clone();
        if self.category.is_generator() {
            out.extend(generator_common_params());
        }
        if self.reads_neighbors() {
            out.push(seamless_param());
        }
        out
    }

    /// Parameters that get a uniform slot and a `p_<id>()` accessor (all but the seamless
    /// switch, which the shader reads through `seamless()`).
    pub fn shader_params(&self) -> Vec<ParamDef> {
        self.all_params().into_iter().filter(|p| p.id != SEAMLESS_PARAM).collect()
    }

    /// Reads neighbouring surface texels (blur, warp, edge growth, ...): such effects get the
    /// shared "Seamless (3D)" parameter that lets them continue across UV seams.
    pub fn reads_neighbors(&self) -> bool {
        self.neighbors || self.wgsl.contains("surface_px") || self.wgsl.contains("blur_world")
    }

    /// Instance with default parameter values.
    pub fn instantiate(&self, id: u64) -> EffectInstance {
        let mut e = EffectInstance::new(id, self.id.clone());
        for p in self.all_params() {
            e.params.insert(p.id.clone(), p.default_value());
        }
        e.blend = if self.category.is_generator() { crate::document::BlendMode::Replace } else { crate::document::BlendMode::Normal };
        e
    }

    /// Value of a parameter for an instance, falling back to the default.
    pub fn value(&self, e: &EffectInstance, p: &ParamDef) -> ParamValue {
        e.params.get(&p.id).cloned().unwrap_or_else(|| p.default_value())
    }

    /// Applies a named preset to an instance.
    pub fn apply_preset(&self, e: &mut EffectInstance, preset: &Preset) {
        for p in self.all_params() {
            let Some(v) = preset.values.get(&p.id) else { continue };
            let value = match (&p.kind, v) {
                (ParamKind::Float { .. }, serde_json::Value::Number(n)) => ParamValue::Float(n.as_f64().unwrap_or(0.0) as f32),
                (ParamKind::Int { .. } | ParamKind::Seed { .. }, serde_json::Value::Number(n)) => {
                    ParamValue::Int(n.as_i64().unwrap_or(0) as i32)
                }
                (ParamKind::Enum { .. }, serde_json::Value::Number(n)) => ParamValue::Enum(n.as_u64().unwrap_or(0) as u32),
                (ParamKind::Bool { .. }, serde_json::Value::Bool(b)) => ParamValue::Bool(*b),
                (ParamKind::Color { .. }, serde_json::Value::Array(a)) if a.len() == 3 => {
                    let c: Vec<f32> = a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
                    ParamValue::Color([c[0], c[1], c[2]].map(crate::document::srgb_to_linear))
                }
                _ => continue,
            };
            e.params.insert(p.id.clone(), value);
        }
    }
}

/// Shared generator parameters (spec §3): global invert / balance / contrast / blur, seed, grunge.
/// Shared parameter of effects that read neighbours: follow UV seam links (default on).
pub fn seamless_param() -> ParamDef {
    ParamDef {
        id: SEAMLESS_PARAM.into(),
        label: "Seamless (3D)".into(),
        label_ru: Some("Бесшовно (3D)".into()),
        kind: ParamKind::Bool { default: true },
        group: None,
        group_ru: None,
        tip: Some("Continue across UV seams (neighbours are found on the surface in 3D). Off: work in UV space only, faster, seams may show.".into()),
        tip_ru: Some("Продолжать через швы развёртки (соседи ищутся по поверхности в 3D). Выкл.: только в UV, быстрее, швы могут быть видны.".into()),
    }
}

/// Id of [`seamless_param`].
pub const SEAMLESS_PARAM: &str = "seamless";

pub fn generator_common_params() -> Vec<ParamDef> {
    let f = |id: &str, label: &str, ru: &str, min: f32, max: f32, default: f32, group: &str, group_ru: &str| ParamDef {
        id: id.into(),
        label: label.into(),
        label_ru: Some(ru.into()),
        kind: ParamKind::Float { min, max, default, logarithmic: false },
        group: Some(group.into()),
        group_ru: Some(group_ru.into()),
        tip: None,
        tip_ru: None,
    };
    let mut v = vec![
        ParamDef {
            id: "g_invert".into(),
            label: "Global Invert".into(),
            label_ru: Some("Инвертировать".into()),
            kind: ParamKind::Bool { default: false },
            group: Some("Global".into()),
            group_ru: Some("Общие".into()),
            tip: None,
            tip_ru: None,
        },
        f("g_balance", "Global Balance", "Баланс", -1.0, 1.0, 0.0, "Global", "Общие"),
        f("g_contrast", "Global Contrast", "Контраст", 0.0, 1.0, 0.0, "Global", "Общие"),
        f("g_blur", "Global Blur", "Размытие", 0.0, 0.1, 0.0, "Global", "Общие"),
        ParamDef {
            id: "seed".into(),
            label: "Seed".into(),
            label_ru: Some("Сид".into()),
            kind: ParamKind::Seed { default: 1 },
            group: Some("Global".into()),
            group_ru: Some("Общие".into()),
            tip: None,
            tip_ru: None,
        },
        ParamDef {
            id: "grunge_type".into(),
            label: "Grunge".into(),
            label_ru: Some("Гранж".into()),
            kind: ParamKind::Enum {
                options: vec![
                    ("none".into(), "None".into(), "Нет".into()),
                    ("clouds".into(), "Clouds".into(), "Облака".into()),
                    ("cells".into(), "Cells".into(), "Ячейки".into()),
                    ("scratches".into(), "Scratches".into(), "Царапины".into()),
                    ("spots".into(), "Spots".into(), "Пятна".into()),
                ],
                default: 0,
            },
            group: Some("Grunge".into()),
            group_ru: Some("Гранж".into()),
            tip: None,
            tip_ru: None,
        },
        f("grunge_amount", "Grunge Amount", "Сила гранжа", 0.0, 1.0, 0.5, "Grunge", "Гранж"),
        f("grunge_scale", "Grunge Scale", "Масштаб гранжа", 0.005, 1.0, 0.1, "Grunge", "Гранж"),
        f("grunge_contrast", "Grunge Contrast", "Контраст гранжа", 0.0, 1.0, 0.5, "Grunge", "Гранж"),
    ];
    for p in &mut v {
        p.tip = Some(common_tip(&p.id).into());
    }
    v
}

fn common_tip(id: &str) -> &'static str {
    match id {
        "g_balance" => "Shifts the mid point towards black (-) or white (+)",
        "g_contrast" => "Sharpens the transition around the mid point",
        "g_blur" => "Blur in object units (fraction of the model size)",
        "grunge_type" => "3D grunge mixed into the result (no UV seams)",
        _ => "",
    }
}

/// All effect definitions known to the app.
pub struct Registry {
    pub effects: Vec<EffectDef>,
}

impl Registry {
    fn load() -> Registry {
        let mut effects = Vec::new();
        for (id, json, wgsl) in BUILTIN_EFFECTS {
            match serde_json::from_str::<EffectDef>(json) {
                Ok(mut def) => {
                    assert_eq!(def.id, *id, "effect id must match its file name");
                    def.wgsl = (*wgsl).to_owned();
                    effects.push(def);
                }
                Err(e) => panic!("effects/{id}.json: {e}"),
            }
        }
        Registry { effects }
    }

    pub fn get(&self, id: &str) -> Option<&EffectDef> {
        self.effects.iter().find(|d| d.id == id)
    }
}

/// The built-in registry (parsed once).
pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Registry::load)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_effects_parse() {
        let r = registry();
        assert!(!r.effects.is_empty());
        for d in &r.effects {
            assert!(d.shader_params().len() <= 48, "{}: too many parameters", d.id);
            assert!(d.wgsl.contains("fn effect("), "{}: shader must define fn effect", d.id);
            let ids: std::collections::HashSet<_> = d.all_params().iter().map(|p| p.id.clone()).collect();
            assert_eq!(ids.len(), d.all_params().len(), "{}: duplicate parameter ids", d.id);
        }
    }
}
