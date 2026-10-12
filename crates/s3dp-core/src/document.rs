//! Layer stack (project format 3): a tree of nodes with effect stacks on content and mask.
//!
//! GPU textures are not part of this model; `s3dp-render` keys them by node / mask-item id.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------- channels

/// Material channels. The project enables a subset (`Document::channels`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChannelId {
    BaseColor,
    Roughness,
    Metallic,
    Normal,
    Height,
    AmbientOcclusion,
    Emissive,
    Opacity,
    User0,
    User1,
    User2,
    User3,
    User4,
    User5,
    User6,
    User7,
}

impl ChannelId {
    pub const ALL: [ChannelId; 16] = [
        Self::BaseColor,
        Self::Roughness,
        Self::Metallic,
        Self::Normal,
        Self::Height,
        Self::AmbientOcclusion,
        Self::Emissive,
        Self::Opacity,
        Self::User0,
        Self::User1,
        Self::User2,
        Self::User3,
        Self::User4,
        Self::User5,
        Self::User6,
        Self::User7,
    ];
    pub const COUNT: usize = 16;
    /// Enabled in new projects (spec §1).
    pub const DEFAULT_SET: [ChannelId; 8] = [
        Self::BaseColor,
        Self::Roughness,
        Self::Metallic,
        Self::Normal,
        Self::Height,
        Self::AmbientOcclusion,
        Self::Emissive,
        Self::Opacity,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<ChannelId> {
        Self::ALL.get(i).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BaseColor => "Base Color",
            Self::Roughness => "Roughness",
            Self::Metallic => "Metallic",
            Self::Normal => "Normal",
            Self::Height => "Height",
            Self::AmbientOcclusion => "AO",
            Self::Emissive => "Emissive",
            Self::Opacity => "Opacity",
            Self::User0 => "User0",
            Self::User1 => "User1",
            Self::User2 => "User2",
            Self::User3 => "User3",
            Self::User4 => "User4",
            Self::User5 => "User5",
            Self::User6 => "User6",
            Self::User7 => "User7",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::BaseColor => "BC",
            Self::Roughness => "R",
            Self::Metallic => "M",
            Self::Normal => "N",
            Self::Height => "H",
            Self::AmbientOcclusion => "AO",
            Self::Emissive => "E",
            Self::Opacity => "Op",
            Self::User0 => "U0",
            Self::User1 => "U1",
            Self::User2 => "U2",
            Self::User3 => "U3",
            Self::User4 => "U4",
            Self::User5 => "U5",
            Self::User6 => "U6",
            Self::User7 => "U7",
        }
    }

    /// RGB channels; the rest hold a single value in R.
    pub fn is_color(self) -> bool {
        matches!(self, Self::BaseColor | Self::Emissive | Self::Normal)
    }

    /// Stored / displayed / exported as sRGB (blending is always linear).
    pub fn is_srgb(self) -> bool {
        matches!(self, Self::BaseColor | Self::Emissive)
    }
}

/// Set of channels (bit per [`ChannelId`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChannelSet(pub u16);

impl ChannelSet {
    pub const ALL: ChannelSet = ChannelSet(u16::MAX);
    pub const NONE: ChannelSet = ChannelSet(0);

    pub fn only(c: ChannelId) -> Self {
        ChannelSet(1 << c.index())
    }

    pub fn has(self, c: ChannelId) -> bool {
        self.0 & (1 << c.index()) != 0
    }

    pub fn set(&mut self, c: ChannelId, on: bool) {
        if on {
            self.0 |= 1 << c.index();
        } else {
            self.0 &= !(1 << c.index());
        }
    }

    pub fn iter(self) -> impl Iterator<Item = ChannelId> {
        ChannelId::ALL.into_iter().filter(move |c| self.has(*c))
    }
}

impl Default for ChannelSet {
    fn default() -> Self {
        Self::ALL
    }
}

impl FromIterator<ChannelId> for ChannelSet {
    fn from_iter<T: IntoIterator<Item = ChannelId>>(iter: T) -> Self {
        let mut s = ChannelSet::NONE;
        for c in iter {
            s.set(c, true);
        }
        s
    }
}

/// One value per channel: rgb (scalars use `.0[0]`). Colors are linear.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelValues(pub BTreeMap<ChannelId, [f32; 3]>);

impl ChannelValues {
    /// Neutral base material under every stack.
    pub fn defaults() -> Self {
        let mut m = BTreeMap::new();
        let gray = srgb_to_linear(0.75);
        m.insert(ChannelId::BaseColor, [gray; 3]);
        m.insert(ChannelId::Roughness, [0.7; 3]);
        m.insert(ChannelId::Metallic, [0.0; 3]);
        m.insert(ChannelId::Normal, [0.5, 0.5, 1.0]);
        m.insert(ChannelId::Height, [0.5; 3]);
        m.insert(ChannelId::AmbientOcclusion, [1.0; 3]);
        m.insert(ChannelId::Emissive, [0.0; 3]);
        m.insert(ChannelId::Opacity, [1.0; 3]);
        for c in &ChannelId::ALL[8..] {
            m.insert(*c, [0.0; 3]);
        }
        ChannelValues(m)
    }

    pub fn get(&self, c: ChannelId) -> [f32; 3] {
        self.0.get(&c).copied().unwrap_or([0.0; 3])
    }

    pub fn set(&mut self, c: ChannelId, v: [f32; 3]) {
        self.0.insert(c, v);
    }
}

impl Default for ChannelValues {
    fn default() -> Self {
        Self::defaults()
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055 }
}

// ---------------------------------------------------------------------------------- blending

/// Blend modes (spec §2.6). Height uses Lighten/Darken/LinearDodge/Subtract as Max/Min/Add/Subtract;
/// the Normal channel always combines with RNM unless the mode is Replace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Replace,
    Passthrough,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    HardLight,
    LinearLight,
    LinearDodge,
    Subtract,
    Difference,
    Divide,
    Darken,
    Lighten,
    Color,
    Hue,
    Saturation,
    Value,
    /// Reoriented normal mapping (normal channel detail combine).
    NormalCombine,
}

impl BlendMode {
    pub const MENU: [Option<BlendMode>; 23] = [
        Some(Self::Normal),
        Some(Self::Replace),
        Some(Self::Passthrough),
        None,
        Some(Self::Multiply),
        Some(Self::Darken),
        Some(Self::Subtract),
        None,
        Some(Self::Screen),
        Some(Self::Lighten),
        Some(Self::LinearDodge),
        None,
        Some(Self::Overlay),
        Some(Self::SoftLight),
        Some(Self::HardLight),
        Some(Self::LinearLight),
        None,
        Some(Self::Difference),
        Some(Self::Divide),
        Some(Self::Hue),
        Some(Self::Saturation),
        Some(Self::Color),
        Some(Self::Value),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Replace => "Replace",
            Self::Passthrough => "Passthrough",
            Self::Multiply => "Multiply",
            Self::Screen => "Screen",
            Self::Overlay => "Overlay",
            Self::SoftLight => "Soft Light",
            Self::HardLight => "Hard Light",
            Self::LinearLight => "Linear Light",
            Self::LinearDodge => "Linear Dodge (Add)",
            Self::Subtract => "Subtract",
            Self::Difference => "Difference",
            Self::Divide => "Divide",
            Self::Darken => "Darken (Min)",
            Self::Lighten => "Lighten (Max)",
            Self::Color => "Color",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Value => "Value",
            Self::NormalCombine => "Normal Map Combine",
        }
    }

    /// Id used by the GPU blend function (`prelude.wgsl::blend_mode`).
    pub fn gpu_id(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelBlend {
    pub mode: BlendMode,
    pub opacity: f32,
}

impl Default for ChannelBlend {
    fn default() -> Self {
        Self { mode: BlendMode::Normal, opacity: 1.0 }
    }
}

/// Blend of a node: one default plus optional per-channel overrides (spec §2: per channel).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Blending {
    pub default: ChannelBlend,
    #[serde(default)]
    pub per_channel: BTreeMap<ChannelId, ChannelBlend>,
    /// Channels this node contributes to.
    #[serde(default)]
    pub channels: ChannelSet,
}

impl Blending {
    pub fn get(&self, c: ChannelId) -> ChannelBlend {
        let b = self.per_channel.get(&c).copied().unwrap_or(self.default);
        // The normal channel combines details instead of overwriting them.
        if c == ChannelId::Normal && b.mode == BlendMode::Normal {
            ChannelBlend { mode: BlendMode::NormalCombine, ..b }
        } else {
            b
        }
    }

    pub fn affects(&self, c: ChannelId) -> bool {
        self.channels.has(c) && self.get(c).opacity > 0.0
    }
}

// ---------------------------------------------------------------------------------- effects

/// Value of an effect parameter (types match `ParamKind`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    /// Linear RGB.
    Color([f32; 3]),
    Enum(u32),
    Vec3([f32; 3]),
    /// Id of an anchored effect instance.
    Anchor(Option<u64>),
}

impl ParamValue {
    pub fn as_f32(&self) -> f32 {
        match self {
            Self::Float(v) => *v,
            Self::Int(v) => *v as f32,
            Self::Bool(v) => *v as u32 as f32,
            Self::Enum(v) => *v as f32,
            Self::Color(c) | Self::Vec3(c) => c[0],
            Self::Anchor(a) => a.is_some() as u32 as f32,
        }
    }
}

/// An effect placed in a stack (or as an effect layer).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectInstance {
    pub id: u64,
    /// `EffectDef::id`.
    pub def: String,
    pub enabled: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    /// Channels it acts on (content effects).
    pub channels: ChannelSet,
    pub params: BTreeMap<String, ParamValue>,
    /// Other effects may read this one's output.
    #[serde(default)]
    pub anchor: bool,
    /// User-visible name (defaults to the effect name).
    #[serde(default)]
    pub name: Option<String>,
}

impl EffectInstance {
    pub fn new(id: u64, def: impl Into<String>) -> Self {
        Self {
            id,
            def: def.into(),
            enabled: true,
            opacity: 1.0,
            blend: BlendMode::Replace,
            channels: ChannelSet::ALL,
            params: BTreeMap::new(),
            anchor: false,
            name: None,
        }
    }

    pub fn with(mut self, key: &str, v: ParamValue) -> Self {
        self.params.insert(key.to_owned(), v);
        self
    }
}

// ---------------------------------------------------------------------------------- masks

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MaskItemKind {
    /// Brush strokes (texture owned by the renderer, keyed by the item id).
    Paint,
    Fill(f32),
    Effect(EffectInstance),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskItem {
    pub id: u64,
    pub enabled: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub kind: MaskItemKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub enabled: bool,
    /// Applied bottom to top over black.
    pub items: Vec<MaskItem>,
}

// ---------------------------------------------------------------------------------- nodes

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    /// Brush-painted channels (textures owned by the renderer, keyed by node id).
    Paint,
    /// Uniform values per channel.
    Fill(ChannelValues),
    /// Children blended together and treated as one layer.
    Folder { expanded: bool },
    /// Filter over everything below it inside the parent (passthrough).
    Effect(EffectInstance),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorLabel {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Violet,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub kind: NodeKind,
    pub blending: Blending,
    pub mask: Option<Mask>,
    /// Effects on the node's content (layer pixels / folder result), applied in order.
    #[serde(default)]
    pub effects: Vec<EffectInstance>,
    /// Bottom to top; only for folders.
    #[serde(default)]
    pub children: Vec<Node>,
    #[serde(default)]
    pub label: ColorLabel,
}

impl Node {
    pub fn new(id: u64, name: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            kind,
            blending: Blending::default(),
            mask: None,
            effects: Vec::new(),
            children: Vec::new(),
            label: ColorLabel::None,
        }
    }

    pub fn is_folder(&self) -> bool {
        matches!(self.kind, NodeKind::Folder { .. })
    }

    pub fn is_effect_layer(&self) -> bool {
        matches!(self.kind, NodeKind::Effect(_))
    }

    /// Calls `f` on this node and all descendants (depth first).
    pub fn visit<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        for c in &self.children {
            c.visit(f);
        }
    }

    /// All effect instances in this node (content, mask, effect layer), not descendants.
    pub fn own_effects(&self) -> impl Iterator<Item = &EffectInstance> {
        let layer = match &self.kind {
            NodeKind::Effect(e) => Some(e),
            _ => None,
        };
        let mask = self.mask.iter().flat_map(|m| m.items.iter()).filter_map(|i| match &i.kind {
            MaskItemKind::Effect(e) => Some(e),
            _ => None,
        });
        layer.into_iter().chain(self.effects.iter()).chain(mask)
    }
}

/// Where a node lives: path of child indices from the root list.
pub type NodePath = Vec<usize>;

/// The whole stack plus project-wide settings shared by every texture set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Root nodes, bottom to top.
    pub nodes: Vec<Node>,
    /// Base material under the stack.
    pub defaults: ChannelValues,
    /// Enabled channels.
    pub channels: ChannelSet,
    pub next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            defaults: ChannelValues::defaults(),
            channels: ChannelId::DEFAULT_SET.into_iter().collect(),
            next_id: 1,
        }
    }
}

impl Document {
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn find(&self, id: u64) -> Option<&Node> {
        let mut found = None;
        for n in &self.nodes {
            n.visit(&mut |n| {
                if n.id == id && found.is_none() {
                    found = Some(n);
                }
            });
        }
        found
    }

    pub fn path_of(&self, id: u64) -> Option<NodePath> {
        fn walk(nodes: &[Node], id: u64, path: &mut NodePath) -> bool {
            for (i, n) in nodes.iter().enumerate() {
                path.push(i);
                if n.id == id || walk(&n.children, id, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        walk(&self.nodes, id, &mut path).then_some(path)
    }

    pub fn find_mut(&mut self, id: u64) -> Option<&mut Node> {
        let path = self.path_of(id)?;
        let mut list = &mut self.nodes;
        for (depth, &i) in path.iter().enumerate() {
            if depth + 1 == path.len() {
                return list.get_mut(i);
            }
            list = &mut list.get_mut(i)?.children;
        }
        None
    }

    /// The list containing the node and its index there.
    pub fn parent_list_mut(&mut self, id: u64) -> Option<(&mut Vec<Node>, usize)> {
        let path = self.path_of(id)?;
        let (last, parents) = path.split_last()?;
        let mut list = &mut self.nodes;
        for &i in parents {
            list = &mut list.get_mut(i)?.children;
        }
        Some((list, *last))
    }

    /// Removes and returns a node.
    pub fn remove(&mut self, id: u64) -> Option<Node> {
        let (list, i) = self.parent_list_mut(id)?;
        Some(list.remove(i))
    }

    /// Inserts above `anchor` (same parent), or into `parent` at the top, or at the root top.
    pub fn insert_above(&mut self, anchor: Option<u64>, node: Node) {
        match anchor.and_then(|a| self.parent_list_mut(a)) {
            Some((list, i)) => list.insert(i + 1, node),
            None => self.nodes.push(node),
        }
    }

    /// Visits every node (depth first).
    pub fn visit<'a>(&'a self, mut f: impl FnMut(&'a Node)) {
        for n in &self.nodes {
            n.visit(&mut f);
        }
    }

    /// Every anchored effect instance with the node that owns it.
    pub fn anchors(&self) -> Vec<(u64, &EffectInstance)> {
        let mut out = Vec::new();
        self.visit(|n| {
            for e in n.own_effects() {
                if e.anchor {
                    out.push((n.id, e));
                }
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_find_insert_remove() {
        let mut d = Document::default();
        let a = d.alloc_id();
        let f = d.alloc_id();
        let b = d.alloc_id();
        d.nodes.push(Node::new(a, "A", NodeKind::Paint));
        let mut folder = Node::new(f, "F", NodeKind::Folder { expanded: true });
        folder.children.push(Node::new(b, "B", NodeKind::Paint));
        d.nodes.push(folder);
        assert_eq!(d.path_of(b), Some(vec![1, 0]));
        let c = d.alloc_id();
        d.insert_above(Some(b), Node::new(c, "C", NodeKind::Paint));
        assert_eq!(d.path_of(c), Some(vec![1, 1]));
        assert_eq!(d.remove(b).unwrap().name, "B");
        assert_eq!(d.find(f).unwrap().children.len(), 1);
        assert_eq!(d.find_mut(c).unwrap().name, "C");
    }

    #[test]
    fn normal_channel_combines_by_default() {
        let b = Blending::default();
        assert_eq!(b.get(ChannelId::Normal).mode, BlendMode::NormalCombine);
        assert_eq!(b.get(ChannelId::BaseColor).mode, BlendMode::Normal);
    }

    #[test]
    fn srgb_roundtrip() {
        for v in [0.0, 0.01, 0.2, 0.5, 0.9, 1.0] {
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
    }
}
