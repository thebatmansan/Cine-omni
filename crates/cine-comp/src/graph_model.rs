//! The compositing node graph: [`CompGraph`], [`CompNode`], [`Param`],
//! ports, links and the node-kind vocabulary with its [`NodeDesc`] metadata.
//!
//! This is a plain, serializable data model. Evaluation lives in
//! [`crate::eval`], the pixel operations in [`crate::ops`], and the mapping
//! onto the Unified Context Graph in [`crate::ucg`].

use crate::error::{Error, Result};
use indexmap::IndexMap;
use petgraph::algo::{has_path_connecting, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use ucg_core::EntityId;
use uuid::Uuid;

/// Identity of a node inside a [`CompGraph`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub Uuid);

impl NodeId {
    /// Fresh random id.
    pub fn new() -> Self {
        NodeId(Uuid::new_v4())
    }
    /// Deterministic id from a node name (used for template graphs).
    pub fn from_name(name: &str) -> Self {
        NodeId(EntityId::from_key(&format!("comp.node:{name}")).0)
    }
    /// Parse from a uuid string.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s).ok().map(NodeId)
    }
}

impl Default for NodeId {
    fn default() -> Self {
        NodeId::new()
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "n#{}", &self.0.to_string()[..8])
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// ports
// ---------------------------------------------------------------------------

/// Data type carried by a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    Image,
    Deep,
    Mask,
    Camera,
    Geometry,
    Scalar,
}

impl PortType {
    /// Whether an output of type `from` may be connected to an input of this type.
    /// Images and masks are interchangeable (a mask is the alpha of an image).
    pub fn accepts(self, from: PortType) -> bool {
        match (self, from) {
            (PortType::Image, PortType::Image | PortType::Mask) => true,
            (PortType::Mask, PortType::Image | PortType::Mask) => true,
            (a, b) => a == b,
        }
    }
    /// Stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            PortType::Image => "image",
            PortType::Deep => "deep",
            PortType::Mask => "mask",
            PortType::Camera => "camera",
            PortType::Geometry => "geometry",
            PortType::Scalar => "scalar",
        }
    }
}

/// An input or output port of a node kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub ty: PortType,
    /// Optional inputs may stay unconnected.
    pub optional: bool,
}

impl Port {
    fn new(name: &str, ty: PortType, optional: bool) -> Self {
        Port {
            name: name.to_string(),
            ty,
            optional,
        }
    }
}

// ---------------------------------------------------------------------------
// params
// ---------------------------------------------------------------------------

/// A roto shape: a closed polygon or a closed cubic Bezier path.
///
/// For `bezier == true`, `points` holds `p0, c1, c2, p1, c1', c2', p2, …` and
/// the last segment closes back to `p0` (so the length is a multiple of 3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub points: Vec<[f32; 2]>,
    #[serde(default)]
    pub bezier: bool,
    /// Feather width in pixels (soft edge spanning ±feather/2 around the outline).
    #[serde(default)]
    pub feather: f32,
    /// Shape opacity multiplier.
    #[serde(default = "one")]
    pub opacity: f32,
}

fn one() -> f32 {
    1.0
}

impl Shape {
    /// A hard-edged polygon.
    pub fn polygon(points: Vec<[f32; 2]>) -> Self {
        Shape {
            points,
            bezier: false,
            feather: 0.0,
            opacity: 1.0,
        }
    }
    /// Axis-aligned rectangle.
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Self {
        Shape::polygon(vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
    }
    /// Closed cubic Bezier path (see type docs for point layout).
    pub fn bezier(points: Vec<[f32; 2]>) -> Self {
        Shape {
            points,
            bezier: true,
            feather: 0.0,
            opacity: 1.0,
        }
    }
    /// Builder: feather width.
    pub fn with_feather(mut self, feather: f32) -> Self {
        self.feather = feather;
        self
    }
    /// Builder: opacity.
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }
}

/// A node parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Param {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
    Color([f32; 4]),
    Vec2([f32; 2]),
    Enum { options: Vec<String>, index: usize },
    Curve(Vec<[f32; 2]>),
    Shapes(Vec<Shape>),
}

impl Param {
    /// Enum parameter from an option list and the selected option.
    pub fn enum_of(options: &[&str], selected: &str) -> Param {
        let index = options.iter().position(|o| *o == selected).unwrap_or(0);
        Param::Enum {
            options: options.iter().map(|s| s.to_string()).collect(),
            index,
        }
    }
    /// Numeric value as f64 (ints and bools convert).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Param::Float(f) => Some(*f),
            Param::Int(i) => Some(*i as f64),
            Param::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
    /// Numeric value as f32.
    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|f| f as f32)
    }
    /// Integer value (floats truncate, enums give their index).
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Param::Int(i) => Some(*i),
            Param::Float(f) => Some(*f as i64),
            Param::Bool(b) => Some(*b as i64),
            Param::Enum { index, .. } => Some(*index as i64),
            _ => None,
        }
    }
    /// Boolean value.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Param::Bool(b) => Some(*b),
            Param::Int(i) => Some(*i != 0),
            _ => None,
        }
    }
    /// String value (enums give the selected option).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Param::Str(s) => Some(s),
            Param::Enum { options, index } => options.get(*index).map(String::as_str),
            _ => None,
        }
    }
    /// Color value.
    pub fn as_color(&self) -> Option<[f32; 4]> {
        match self {
            Param::Color(c) => Some(*c),
            Param::Float(f) => Some([*f as f32, *f as f32, *f as f32, 1.0]),
            _ => None,
        }
    }
    /// 2D vector value.
    pub fn as_vec2(&self) -> Option<[f32; 2]> {
        match self {
            Param::Vec2(v) => Some(*v),
            Param::Float(f) => Some([*f as f32, *f as f32]),
            _ => None,
        }
    }
    /// Roto shapes.
    pub fn as_shapes(&self) -> Option<&[Shape]> {
        match self {
            Param::Shapes(s) => Some(s),
            _ => None,
        }
    }
    /// Curve points.
    pub fn as_curve(&self) -> Option<&[[f32; 2]]> {
        match self {
            Param::Curve(c) => Some(c),
            _ => None,
        }
    }

    /// Feed the value into a hasher (floats are hashed by their bit pattern).
    pub fn hash_into<H: Hasher>(&self, h: &mut H) {
        match self {
            Param::Float(f) => {
                0u8.hash(h);
                f.to_bits().hash(h);
            }
            Param::Int(i) => {
                1u8.hash(h);
                i.hash(h);
            }
            Param::Bool(b) => {
                2u8.hash(h);
                b.hash(h);
            }
            Param::Str(s) => {
                3u8.hash(h);
                s.hash(h);
            }
            Param::Color(c) => {
                4u8.hash(h);
                c.iter().for_each(|v| v.to_bits().hash(h));
            }
            Param::Vec2(v) => {
                5u8.hash(h);
                v.iter().for_each(|v| v.to_bits().hash(h));
            }
            Param::Enum { options, index } => {
                6u8.hash(h);
                options.hash(h);
                index.hash(h);
            }
            Param::Curve(c) => {
                7u8.hash(h);
                c.iter().flatten().for_each(|v| v.to_bits().hash(h));
            }
            Param::Shapes(s) => {
                8u8.hash(h);
                for sh in s {
                    sh.bezier.hash(h);
                    sh.feather.to_bits().hash(h);
                    sh.opacity.to_bits().hash(h);
                    sh.points.iter().flatten().for_each(|v| v.to_bits().hash(h));
                }
            }
        }
    }
}

impl From<f64> for Param {
    fn from(v: f64) -> Self {
        Param::Float(v)
    }
}
impl From<f32> for Param {
    fn from(v: f32) -> Self {
        Param::Float(v as f64)
    }
}
impl From<i64> for Param {
    fn from(v: i64) -> Self {
        Param::Int(v)
    }
}
impl From<i32> for Param {
    fn from(v: i32) -> Self {
        Param::Int(v as i64)
    }
}
impl From<bool> for Param {
    fn from(v: bool) -> Self {
        Param::Bool(v)
    }
}
impl From<&str> for Param {
    fn from(v: &str) -> Self {
        Param::Str(v.to_string())
    }
}
impl From<String> for Param {
    fn from(v: String) -> Self {
        Param::Str(v)
    }
}
impl From<[f32; 4]> for Param {
    fn from(v: [f32; 4]) -> Self {
        Param::Color(v)
    }
}
impl From<[f32; 2]> for Param {
    fn from(v: [f32; 2]) -> Self {
        Param::Vec2(v)
    }
}
impl From<Vec<Shape>> for Param {
    fn from(v: Vec<Shape>) -> Self {
        Param::Shapes(v)
    }
}

/// Description of one parameter of a node kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDesc {
    pub name: String,
    pub default: Param,
    pub doc: String,
}

/// Static description of a node kind: ports and parameters with defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeDesc {
    pub kind: NodeKind,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub params: Vec<ParamDesc>,
    pub doc: String,
}

impl NodeDesc {
    fn new(kind: NodeKind, doc: &str) -> Self {
        NodeDesc {
            kind,
            inputs: Vec::new(),
            outputs: Vec::new(),
            params: Vec::new(),
            doc: doc.to_string(),
        }
    }
    fn input(mut self, name: &str, ty: PortType) -> Self {
        self.inputs.push(Port::new(name, ty, false));
        self
    }
    fn opt_input(mut self, name: &str, ty: PortType) -> Self {
        self.inputs.push(Port::new(name, ty, true));
        self
    }
    fn output(mut self, name: &str, ty: PortType) -> Self {
        self.outputs.push(Port::new(name, ty, false));
        self
    }
    fn param(mut self, name: &str, default: impl Into<Param>, doc: &str) -> Self {
        self.params.push(ParamDesc {
            name: name.to_string(),
            default: default.into(),
            doc: doc.to_string(),
        });
        self
    }
    fn enum_param(self, name: &str, options: &[&str], selected: &str, doc: &str) -> Self {
        self.param(name, Param::enum_of(options, selected), doc)
    }
    /// Index of an input port by name.
    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|p| p.name == name)
    }
    /// Default value of a parameter.
    pub fn default_of(&self, name: &str) -> Option<&Param> {
        self.params
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.default)
    }
    /// The primary output type.
    pub fn output_type(&self) -> PortType {
        self.outputs
            .first()
            .map(|p| p.ty)
            .unwrap_or(PortType::Image)
    }
}

// ---------------------------------------------------------------------------
// typed option enums
// ---------------------------------------------------------------------------

macro_rules! option_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl $name {
            /// All option names, in index order.
            pub const OPTIONS: &'static [&'static str] = &[$($s),+];
            /// All variants, in index order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            /// Stable name.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
            /// Parse from the stable name.
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some($name::$variant),)+ _ => None }
            }
            /// Position in `OPTIONS`.
            pub fn index(self) -> usize {
                Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
            }
            /// Build the enum parameter selecting this variant.
            pub fn param(self) -> Param {
                Param::enum_of(Self::OPTIONS, self.as_str())
            }
            /// Read from a parameter (enum by name/index, or string).
            pub fn from_param(p: &Param) -> Option<Self> {
                match p {
                    Param::Enum { index, options } => {
                        options.get(*index).and_then(|s| Self::parse(s)).or_else(|| Self::ALL.get(*index).copied())
                    }
                    Param::Str(s) => Self::parse(s),
                    Param::Int(i) => Self::ALL.get(*i as usize).copied(),
                    _ => None,
                }
            }
        }
    };
}

option_enum! {
    /// Porter-Duff / arithmetic merge operations (premultiplied inputs).
    MergeOp {
        Over => "over", Under => "under", Plus => "plus", Minus => "minus",
        Multiply => "multiply", Screen => "screen", Difference => "difference",
        Max => "max", Min => "min", Mask => "mask", Stencil => "stencil",
        Xor => "xor", In => "in", Out => "out", Atop => "atop",
    }
}

option_enum! {
    /// Keying algorithm.
    KeyerKind { Chroma => "chroma", Luma => "luma", Difference => "difference", Screen => "screen" }
}

option_enum! {
    /// Where a `Read` node gets its pixels from.
    ReadSourceKind { File => "file", Generator => "generator", PrevisPass => "previs_pass" }
}

option_enum! {
    /// Built-in generator patterns for `Read` (generator source).
    GeneratorKind { Checkerboard => "checkerboard", Ramp => "ramp", Solid => "solid", Black => "black" }
}

option_enum! {
    /// Render pass pulled from the previs stage.
    PassKind { Beauty => "beauty", Depth => "depth", Normal => "normal", Id => "id", Motion => "motion" }
}

option_enum! {
    /// Resampling filter.
    FilterKind { Bilinear => "bilinear", Bicubic => "bicubic" }
}

option_enum! {
    /// Blur kernel.
    BlurKind { Gaussian => "gaussian", Box => "box" }
}

option_enum! {
    /// Procedural pattern.
    ProceduralKind { Noise => "noise", Gradient => "gradient", Checker => "checker", Radial => "radial", Cellular => "cellular" }
}

option_enum! {
    /// Channel selection for `Copy`.
    CopyChannels { Alpha => "alpha", Rgb => "rgb", Red => "red", Green => "green", Blue => "blue", Rgba => "rgba" }
}

option_enum! {
    /// Channel source for `Shuffle`.
    ShuffleSource { R => "r", G => "g", B => "b", A => "a", Zero => "zero", One => "one" }
}

/// Typed view of a `Read` node's source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadSource {
    /// A file path placeholder (resolved by the evaluator's file registry).
    File { path: String },
    /// A built-in generator.
    Generator {
        generator: GeneratorKind,
        width: usize,
        height: usize,
        color: [f32; 4],
    },
    /// A render pass of a previs scene as seen from a camera.
    PrevisPass {
        scene: EntityId,
        pass: PassKind,
        camera: EntityId,
    },
}

// ---------------------------------------------------------------------------
// node kinds
// ---------------------------------------------------------------------------

/// Every node kind the compositor implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Read,
    Merge,
    Keyer,
    ColorCorrect,
    Grade,
    Transform,
    Crop,
    Reformat,
    Blur,
    Sharpen,
    Erode,
    Dilate,
    EdgeBlur,
    Roto,
    ZDefocus,
    DeepMerge,
    DeepToImage,
    DeepFromImage,
    DeepHoldout,
    Copy,
    Shuffle,
    Premult,
    Unpremult,
    Invert,
    Clamp,
    Expression,
    Procedural,
    Write,
}

impl NodeKind {
    /// All kinds.
    pub const ALL: &'static [NodeKind] = &[
        NodeKind::Read,
        NodeKind::Merge,
        NodeKind::Keyer,
        NodeKind::ColorCorrect,
        NodeKind::Grade,
        NodeKind::Transform,
        NodeKind::Crop,
        NodeKind::Reformat,
        NodeKind::Blur,
        NodeKind::Sharpen,
        NodeKind::Erode,
        NodeKind::Dilate,
        NodeKind::EdgeBlur,
        NodeKind::Roto,
        NodeKind::ZDefocus,
        NodeKind::DeepMerge,
        NodeKind::DeepToImage,
        NodeKind::DeepFromImage,
        NodeKind::DeepHoldout,
        NodeKind::Copy,
        NodeKind::Shuffle,
        NodeKind::Premult,
        NodeKind::Unpremult,
        NodeKind::Invert,
        NodeKind::Clamp,
        NodeKind::Expression,
        NodeKind::Procedural,
        NodeKind::Write,
    ];

    /// Stable snake_case name (the one stored in the UCG).
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Read => "read",
            NodeKind::Merge => "merge",
            NodeKind::Keyer => "keyer",
            NodeKind::ColorCorrect => "color_correct",
            NodeKind::Grade => "grade",
            NodeKind::Transform => "transform",
            NodeKind::Crop => "crop",
            NodeKind::Reformat => "reformat",
            NodeKind::Blur => "blur",
            NodeKind::Sharpen => "sharpen",
            NodeKind::Erode => "erode",
            NodeKind::Dilate => "dilate",
            NodeKind::EdgeBlur => "edge_blur",
            NodeKind::Roto => "roto",
            NodeKind::ZDefocus => "z_defocus",
            NodeKind::DeepMerge => "deep_merge",
            NodeKind::DeepToImage => "deep_to_image",
            NodeKind::DeepFromImage => "deep_from_image",
            NodeKind::DeepHoldout => "deep_holdout",
            NodeKind::Copy => "copy",
            NodeKind::Shuffle => "shuffle",
            NodeKind::Premult => "premult",
            NodeKind::Unpremult => "unpremult",
            NodeKind::Invert => "invert",
            NodeKind::Clamp => "clamp",
            NodeKind::Expression => "expression",
            NodeKind::Procedural => "procedural",
            NodeKind::Write => "write",
        }
    }

    /// Parse the stable name.
    pub fn parse(s: &str) -> Result<NodeKind> {
        NodeKind::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| Error::UnknownKind(s.to_string()))
    }

    /// Ports and parameters (with defaults) of this kind.
    pub fn describe(self) -> NodeDesc {
        use PortType::*;
        let white = [1.0f32, 1.0, 1.0, 1.0];
        let black = [0.0f32, 0.0, 0.0, 1.0];
        match self {
            NodeKind::Read => NodeDesc::new(
                self,
                "Source image: file placeholder, generator or previs render pass",
            )
            .output("out", Image)
            .enum_param(
                "source",
                ReadSourceKind::OPTIONS,
                "generator",
                "Where pixels come from",
            )
            .param(
                "path",
                "",
                "File path (file source); resolved via the evaluator's file registry",
            )
            .enum_param(
                "generator",
                GeneratorKind::OPTIONS,
                "checkerboard",
                "Generator pattern",
            )
            .param("width", 256i64, "Generator width")
            .param("height", 256i64, "Generator height")
            .param("color", [0.5f32, 0.5, 0.5, 1.0], "Generator color")
            .param("scene", "", "Previs scene entity id")
            .param("camera", "", "Previs camera entity id")
            .enum_param("pass", PassKind::OPTIONS, "beauty", "Previs render pass"),
            NodeKind::Merge => NodeDesc::new(
                self,
                "Porter-Duff / arithmetic merge of premultiplied A over B",
            )
            .input("A", Image)
            .input("B", Image)
            .opt_input("mask", Mask)
            .output("out", Image)
            .enum_param("operation", MergeOp::OPTIONS, "over", "Merge operation")
            .param("mix", 1.0f64, "Blend between B (0) and the result (1)"),
            NodeKind::Keyer => NodeDesc::new(
                self,
                "Chroma / luma / difference / screen keyer producing alpha and despilled RGB",
            )
            .input("src", Image)
            .opt_input("clean_plate", Image)
            .output("out", Image)
            .enum_param("kind", KeyerKind::OPTIONS, "chroma", "Keying algorithm")
            .param(
                "key_color",
                [0.0f32, 1.0, 0.0, 1.0],
                "Screen color to key out",
            )
            .param(
                "tolerance",
                0.12f64,
                "Chroma distance fully keyed (alpha 0)",
            )
            .param("softness", 0.12f64, "Chroma distance ramp width to alpha 1")
            .param("spill_suppress", 1.0f64, "Amount of spill suppression")
            .param("low", 0.1f64, "Luma below which alpha is 0")
            .param("high", 0.5f64, "Luma above which alpha is 1")
            .param("threshold", 0.2f64, "Difference at which alpha reaches 1")
            .param("invert", false, "Invert the resulting matte"),
            NodeKind::ColorCorrect | NodeKind::Grade => {
                NodeDesc::new(self, "Lift / gain / offset / gamma / contrast / saturation")
                    .input("src", Image)
                    .opt_input("mask", Mask)
                    .output("out", Image)
                    .param("gain", white, "Multiplier")
                    .param("gamma", 1.0f64, "Gamma (value ^ (1/gamma))")
                    .param("lift", black, "Lift (raises blacks)")
                    .param("offset", black, "Additive offset")
                    .param("saturation", 1.0f64, "Saturation")
                    .param("contrast", 1.0f64, "Contrast around 0.18 grey")
            }
            NodeKind::Transform => NodeDesc::new(
                self,
                "2D affine transform (translate, rotate, scale about a center)",
            )
            .input("src", Image)
            .output("out", Image)
            .param("translate", [0.0f32, 0.0], "Translation in pixels")
            .param(
                "rotate_deg",
                0.0f64,
                "Rotation in degrees (clockwise in image space)",
            )
            .param("scale", [1.0f32, 1.0], "Scale")
            .param(
                "center",
                [0.0f32, 0.0],
                "Pivot in pixels (when center_auto is false)",
            )
            .param("center_auto", true, "Use the image center as pivot")
            .enum_param(
                "filter",
                FilterKind::OPTIONS,
                "bilinear",
                "Resampling filter",
            ),
            NodeKind::Crop => NodeDesc::new(self, "Crop to a rectangle")
                .input("src", Image)
                .output("out", Image)
                .param("x", 0i64, "Left")
                .param("y", 0i64, "Top")
                .param("width", 256i64, "Width")
                .param("height", 256i64, "Height"),
            NodeKind::Reformat => NodeDesc::new(self, "Resize to a new format")
                .input("src", Image)
                .output("out", Image)
                .param("width", 1920i64, "Output width")
                .param("height", 1080i64, "Output height")
                .enum_param(
                    "filter",
                    FilterKind::OPTIONS,
                    "bilinear",
                    "Resampling filter",
                ),
            NodeKind::Blur => NodeDesc::new(self, "Separable Gaussian or box blur")
                .input("src", Image)
                .opt_input("mask", Mask)
                .output("out", Image)
                .param("size", 4.0f64, "Gaussian sigma / box radius in pixels")
                .enum_param("kind", BlurKind::OPTIONS, "gaussian", "Kernel"),
            NodeKind::Sharpen => NodeDesc::new(self, "Unsharp mask")
                .input("src", Image)
                .output("out", Image)
                .param("amount", 0.5f64, "Sharpen amount")
                .param("size", 1.0f64, "Blur sigma used for the mask"),
            NodeKind::Erode => NodeDesc::new(self, "Erode the alpha channel (min filter)")
                .input("src", Image)
                .output("out", Image)
                .param("size", 1i64, "Radius in pixels"),
            NodeKind::Dilate => NodeDesc::new(self, "Dilate the alpha channel (max filter)")
                .input("src", Image)
                .output("out", Image)
                .param("size", 1i64, "Radius in pixels"),
            NodeKind::EdgeBlur => NodeDesc::new(self, "Blur only along alpha edges")
                .input("src", Image)
                .output("out", Image)
                .param("size", 2.0f64, "Blur sigma")
                .param("edge_width", 2i64, "Edge band radius in pixels"),
            NodeKind::Roto => NodeDesc::new(self, "Rasterize shapes to a feathered mask")
                .opt_input("src", Image)
                .output("mask", Mask)
                .param("shapes", Vec::<Shape>::new(), "Shapes")
                .param("width", 256i64, "Width when no src is connected")
                .param("height", 256i64, "Height when no src is connected")
                .param("invert", false, "Invert the mask"),
            NodeKind::ZDefocus => {
                NodeDesc::new(self, "Depth-driven defocus (layered circle of confusion)")
                    .input("src", Image)
                    .input("depth", Image)
                    .output("out", Image)
                    .param("focal_distance", 10.0f64, "Depth in focus")
                    .param("f_stop", 4.0f64, "Aperture f-number")
                    .param("max_blur", 8.0f64, "Maximum blur sigma in pixels")
                    .param("slices", 6i64, "Number of blur layers")
            }
            NodeKind::DeepMerge => NodeDesc::new(self, "Merge the samples of two deep images")
                .input("A", Deep)
                .input("B", Deep)
                .output("out", Deep),
            NodeKind::DeepToImage => NodeDesc::new(self, "Flatten a deep image front to back")
                .input("src", Deep)
                .output("out", Image),
            NodeKind::DeepFromImage => {
                NodeDesc::new(self, "Deep image from a flat image plus depth")
                    .input("src", Image)
                    .input("depth", Image)
                    .output("out", Deep)
            }
            NodeKind::DeepHoldout => {
                NodeDesc::new(self, "Hold out samples covered by another deep image")
                    .input("src", Deep)
                    .input("holdout", Deep)
                    .output("out", Deep)
                    .param(
                        "eps",
                        1e-4f64,
                        "Transmittance below which samples are dropped",
                    )
            }
            NodeKind::Copy => NodeDesc::new(self, "Copy channels from B into A")
                .input("A", Image)
                .input("B", Image)
                .output("out", Image)
                .enum_param(
                    "channels",
                    CopyChannels::OPTIONS,
                    "alpha",
                    "Channels copied from B",
                ),
            NodeKind::Shuffle => NodeDesc::new(self, "Rearrange channels")
                .input("src", Image)
                .output("out", Image)
                .enum_param("r", ShuffleSource::OPTIONS, "r", "Source for red")
                .enum_param("g", ShuffleSource::OPTIONS, "g", "Source for green")
                .enum_param("b", ShuffleSource::OPTIONS, "b", "Source for blue")
                .enum_param("a", ShuffleSource::OPTIONS, "a", "Source for alpha"),
            NodeKind::Premult => NodeDesc::new(self, "Multiply RGB by alpha")
                .input("src", Image)
                .output("out", Image),
            NodeKind::Unpremult => NodeDesc::new(self, "Divide RGB by alpha")
                .input("src", Image)
                .output("out", Image),
            NodeKind::Invert => NodeDesc::new(self, "Invert RGB (alpha untouched)")
                .input("src", Image)
                .output("out", Image),
            NodeKind::Clamp => NodeDesc::new(self, "Clamp channel values")
                .input("src", Image)
                .output("out", Image)
                .param("min", 0.0f64, "Minimum")
                .param("max", 1.0f64, "Maximum")
                .param("clamp_alpha", true, "Also clamp alpha"),
            NodeKind::Expression => NodeDesc::new(
                self,
                "Per-channel arithmetic expressions over r,g,b,a,x,y,w,h",
            )
            .input("src", Image)
            .output("out", Image)
            .param("r", "", "Red expression (empty = passthrough)")
            .param("g", "", "Green expression")
            .param("b", "", "Blue expression")
            .param("a", "", "Alpha expression"),
            NodeKind::Procedural => NodeDesc::new(self, "Procedural pattern generator")
                .output("out", Image)
                .enum_param("kind", ProceduralKind::OPTIONS, "noise", "Pattern")
                .param("width", 256i64, "Width")
                .param("height", 256i64, "Height")
                .param("seed", 0i64, "Random seed")
                .param("frequency", 4.0f64, "Base frequency across the image")
                .param("octaves", 4i64, "fBm octaves")
                .param("lacunarity", 2.0f64, "fBm lacunarity")
                .param("gain", 0.5f64, "fBm gain")
                .param("color1", black, "First color")
                .param("color2", white, "Second color")
                .param("angle", 0.0f64, "Gradient angle in degrees")
                .param("center", [0.5f32, 0.5], "Radial center (normalized)")
                .param("radius", 0.5f64, "Radial radius (normalized)")
                .param("softness", 0.5f64, "Radial softness")
                .param("size", 16i64, "Checker square size in pixels"),
            NodeKind::Write => {
                NodeDesc::new(self, "Output sink (recorded in the evaluation result)")
                    .input("src", Image)
                    .output("out", Image)
                    .param("path", "out.exr", "Output path")
            }
        }
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// nodes and links
// ---------------------------------------------------------------------------

/// A node instance in a [`CompGraph`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompNode {
    pub id: NodeId,
    pub name: String,
    pub kind: NodeKind,
    pub params: IndexMap<String, Param>,
    pub position: [f32; 2],
    #[serde(default)]
    pub bypass: bool,
}

impl CompNode {
    /// A node with the kind's default parameters and a deterministic id
    /// derived from its name.
    pub fn new(name: impl Into<String>, kind: NodeKind) -> Self {
        let name = name.into();
        let params = kind
            .describe()
            .params
            .into_iter()
            .map(|p| (p.name, p.default))
            .collect();
        CompNode {
            id: NodeId::from_name(&name),
            name,
            kind,
            params,
            position: [0.0, 0.0],
            bypass: false,
        }
    }

    /// Builder: override a parameter.
    pub fn with_param(mut self, name: &str, value: impl Into<Param>) -> Self {
        self.set_param(name, value);
        self
    }

    /// Builder: editor position.
    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.position = [x, y];
        self
    }

    /// Builder: random id instead of the name-derived one.
    pub fn with_random_id(mut self) -> Self {
        self.id = NodeId::new();
        self
    }

    /// Set a parameter. Enum parameters given as strings select that option.
    pub fn set_param(&mut self, name: &str, value: impl Into<Param>) {
        let value = value.into();
        let value = match (self.params.get(name), value) {
            (Some(Param::Enum { options, .. }), Param::Str(s)) => {
                let index = options.iter().position(|o| *o == s).unwrap_or(0);
                Param::Enum {
                    options: options.clone(),
                    index,
                }
            }
            (_, v) => v,
        };
        self.params.insert(name.to_string(), value);
    }

    /// Read a parameter.
    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.get(name)
    }

    fn param_or_default(&self, name: &str) -> Option<Param> {
        self.params
            .get(name)
            .cloned()
            .or_else(|| self.kind.describe().default_of(name).cloned())
    }

    /// Float parameter (falls back to the kind default, then 0).
    pub fn f32(&self, name: &str) -> f32 {
        self.param_or_default(name)
            .and_then(|p| p.as_f32())
            .unwrap_or(0.0)
    }
    /// Integer parameter (falls back to the kind default, then 0).
    pub fn int(&self, name: &str) -> i64 {
        self.param_or_default(name)
            .and_then(|p| p.as_int())
            .unwrap_or(0)
    }
    /// Boolean parameter.
    pub fn bool(&self, name: &str) -> bool {
        self.param_or_default(name)
            .and_then(|p| p.as_bool())
            .unwrap_or(false)
    }
    /// String parameter (enums give the selected option name).
    pub fn str(&self, name: &str) -> String {
        self.param_or_default(name)
            .and_then(|p| p.as_str().map(str::to_string))
            .unwrap_or_default()
    }
    /// Color parameter.
    pub fn color(&self, name: &str) -> [f32; 4] {
        self.param_or_default(name)
            .and_then(|p| p.as_color())
            .unwrap_or([0.0, 0.0, 0.0, 1.0])
    }
    /// 2D vector parameter.
    pub fn vec2(&self, name: &str) -> [f32; 2] {
        self.param_or_default(name)
            .and_then(|p| p.as_vec2())
            .unwrap_or([0.0, 0.0])
    }
    /// Roto shapes parameter.
    pub fn shapes(&self, name: &str) -> Vec<Shape> {
        self.param_or_default(name)
            .and_then(|p| p.as_shapes().map(|s| s.to_vec()))
            .unwrap_or_default()
    }

    /// Typed view of a `Read` node's source parameters.
    pub fn read_source(&self) -> ReadSource {
        let kind = self
            .param("source")
            .and_then(ReadSourceKind::from_param)
            .unwrap_or(ReadSourceKind::Generator);
        match kind {
            ReadSourceKind::File => ReadSource::File {
                path: self.str("path"),
            },
            ReadSourceKind::Generator => ReadSource::Generator {
                generator: self
                    .param("generator")
                    .and_then(GeneratorKind::from_param)
                    .unwrap_or(GeneratorKind::Checkerboard),
                width: self.int("width").max(0) as usize,
                height: self.int("height").max(0) as usize,
                color: self.color("color"),
            },
            ReadSourceKind::PrevisPass => ReadSource::PrevisPass {
                scene: EntityId::parse(&self.str("scene")).unwrap_or(EntityId::nil()),
                pass: self
                    .param("pass")
                    .and_then(PassKind::from_param)
                    .unwrap_or(PassKind::Beauty),
                camera: EntityId::parse(&self.str("camera")).unwrap_or(EntityId::nil()),
            },
        }
    }

    /// Store a typed source into the parameters of a `Read` node.
    pub fn set_read_source(&mut self, source: &ReadSource) {
        match source {
            ReadSource::File { path } => {
                self.set_param("source", ReadSourceKind::File.param());
                self.set_param("path", path.as_str());
            }
            ReadSource::Generator {
                generator,
                width,
                height,
                color,
            } => {
                self.set_param("source", ReadSourceKind::Generator.param());
                self.set_param("generator", generator.param());
                self.set_param("width", *width as i64);
                self.set_param("height", *height as i64);
                self.set_param("color", *color);
            }
            ReadSource::PrevisPass {
                scene,
                pass,
                camera,
            } => {
                self.set_param("source", ReadSourceKind::PrevisPass.param());
                self.set_param("scene", scene.to_string());
                self.set_param("pass", pass.param());
                self.set_param("camera", camera.to_string());
            }
        }
    }

    // ---- typed constructors --------------------------------------------

    /// `Read` node with a typed source.
    pub fn read(name: impl Into<String>, source: ReadSource) -> Self {
        let mut n = CompNode::new(name, NodeKind::Read);
        n.set_read_source(&source);
        n
    }
    /// `Read` node pulling a previs pass.
    pub fn read_previs(
        name: impl Into<String>,
        scene: EntityId,
        pass: PassKind,
        camera: EntityId,
    ) -> Self {
        CompNode::read(
            name,
            ReadSource::PrevisPass {
                scene,
                pass,
                camera,
            },
        )
    }
    /// `Read` node with a generator source.
    pub fn read_generator(
        name: impl Into<String>,
        generator: GeneratorKind,
        width: usize,
        height: usize,
    ) -> Self {
        CompNode::read(
            name,
            ReadSource::Generator {
                generator,
                width,
                height,
                color: [0.5, 0.5, 0.5, 1.0],
            },
        )
    }
    /// `Merge` node.
    pub fn merge(name: impl Into<String>, op: MergeOp) -> Self {
        CompNode::new(name, NodeKind::Merge).with_param("operation", op.param())
    }
    /// `Keyer` node.
    pub fn keyer(name: impl Into<String>, kind: KeyerKind) -> Self {
        CompNode::new(name, NodeKind::Keyer).with_param("kind", kind.param())
    }
    /// `Blur` node.
    pub fn blur(name: impl Into<String>, size: f32, kind: BlurKind) -> Self {
        CompNode::new(name, NodeKind::Blur)
            .with_param("size", size)
            .with_param("kind", kind.param())
    }
    /// `Transform` node.
    pub fn transform(
        name: impl Into<String>,
        translate: [f32; 2],
        rotate_deg: f32,
        scale: [f32; 2],
    ) -> Self {
        CompNode::new(name, NodeKind::Transform)
            .with_param("translate", translate)
            .with_param("rotate_deg", rotate_deg)
            .with_param("scale", scale)
    }
    /// `Roto` node.
    pub fn roto(name: impl Into<String>, shapes: Vec<Shape>, width: usize, height: usize) -> Self {
        CompNode::new(name, NodeKind::Roto)
            .with_param("shapes", shapes)
            .with_param("width", width as i64)
            .with_param("height", height as i64)
    }
    /// `Procedural` node.
    pub fn procedural(
        name: impl Into<String>,
        kind: ProceduralKind,
        width: usize,
        height: usize,
    ) -> Self {
        CompNode::new(name, NodeKind::Procedural)
            .with_param("kind", kind.param())
            .with_param("width", width as i64)
            .with_param("height", height as i64)
    }
    /// `Expression` node with per-channel expressions (empty = passthrough).
    pub fn expression(name: impl Into<String>, r: &str, g: &str, b: &str, a: &str) -> Self {
        CompNode::new(name, NodeKind::Expression)
            .with_param("r", r)
            .with_param("g", g)
            .with_param("b", b)
            .with_param("a", a)
    }
    /// `Write` node.
    pub fn write(name: impl Into<String>, path: &str) -> Self {
        CompNode::new(name, NodeKind::Write).with_param("path", path)
    }

    /// Hash of kind, parameters and bypass flag (not upstream).
    pub fn local_hash(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.kind.as_str().hash(&mut h);
        self.bypass.hash(&mut h);
        for (k, v) in &self.params {
            k.hash(&mut h);
            v.hash_into(&mut h);
        }
        h.finish()
    }
}

/// A connection from an output port to an input port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Link {
    /// `(node, output port index)`.
    pub from: (NodeId, usize),
    /// `(node, input port index)`.
    pub to: (NodeId, usize),
}

/// A directed acyclic compositing graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompGraph {
    pub nodes: IndexMap<NodeId, CompNode>,
    pub links: Vec<Link>,
}

impl CompGraph {
    /// Empty graph.
    pub fn new() -> Self {
        CompGraph::default()
    }

    /// Add a node; names must be unique.
    pub fn add(&mut self, node: CompNode) -> Result<NodeId> {
        if self
            .nodes
            .values()
            .any(|n| n.name == node.name && n.id != node.id)
        {
            return Err(Error::DuplicateName(node.name));
        }
        let id = node.id;
        self.nodes.insert(id, node);
        Ok(id)
    }

    /// Remove a node and all its links.
    pub fn remove(&mut self, id: NodeId) -> Option<CompNode> {
        let node = self.nodes.shift_remove(&id)?;
        self.links.retain(|l| l.from.0 != id && l.to.0 != id);
        Some(node)
    }

    /// Look up a node.
    pub fn node(&self, id: NodeId) -> Result<&CompNode> {
        self.nodes.get(&id).ok_or(Error::NodeNotFound(id))
    }

    /// Mutable node lookup.
    pub fn node_mut(&mut self, id: NodeId) -> Result<&mut CompNode> {
        self.nodes.get_mut(&id).ok_or(Error::NodeNotFound(id))
    }

    /// Find a node by name.
    pub fn by_name(&self, name: &str) -> Option<&CompNode> {
        self.nodes.values().find(|n| n.name == name)
    }

    /// Id of a node by name.
    pub fn id_of(&self, name: &str) -> Option<NodeId> {
        self.by_name(name).map(|n| n.id)
    }

    /// Set a parameter on a node by id.
    pub fn set_param(&mut self, id: NodeId, name: &str, value: impl Into<Param>) -> Result<()> {
        self.node_mut(id)?.set_param(name, value);
        Ok(())
    }

    /// Would linking `from -> to` create a cycle?
    pub fn would_cycle(&self, from: NodeId, to: NodeId) -> bool {
        if from == to {
            return true;
        }
        let (g, idx) = self.petgraph();
        match (idx.get(&to), idx.get(&from)) {
            (Some(&t), Some(&f)) => has_path_connecting(&g, t, f, None),
            _ => false,
        }
    }

    /// Connect `from`'s output port to `to`'s input port, validating ports,
    /// types and acyclicity. An existing link into the same input is replaced.
    pub fn connect(
        &mut self,
        from: NodeId,
        out_port: usize,
        to: NodeId,
        in_port: usize,
    ) -> Result<()> {
        let from_desc = self.node(from)?.kind.describe();
        let to_desc = self.node(to)?.kind.describe();
        let out = from_desc.outputs.get(out_port).ok_or(Error::NoSuchPort {
            node: from,
            dir: "output",
            port: out_port,
        })?;
        let inp = to_desc.inputs.get(in_port).ok_or(Error::NoSuchPort {
            node: to,
            dir: "input",
            port: in_port,
        })?;
        if !inp.ty.accepts(out.ty) {
            return Err(Error::PortTypeMismatch {
                from_ty: out.ty.as_str().into(),
                to_ty: inp.ty.as_str().into(),
            });
        }
        if self.would_cycle(from, to) {
            return Err(Error::Cycle { from, to });
        }
        self.links.retain(|l| l.to != (to, in_port));
        self.links.push(Link {
            from: (from, out_port),
            to: (to, in_port),
        });
        Ok(())
    }

    /// Connect by node names (output port 0).
    pub fn connect_names(&mut self, from: &str, to: &str, in_port: usize) -> Result<()> {
        let f = self
            .id_of(from)
            .ok_or_else(|| Error::InvalidEntity(format!("no node named {from}")))?;
        let t = self
            .id_of(to)
            .ok_or_else(|| Error::InvalidEntity(format!("no node named {to}")))?;
        self.connect(f, 0, t, in_port)
    }

    /// Remove the link into an input port.
    pub fn disconnect(&mut self, to: NodeId, in_port: usize) -> bool {
        let before = self.links.len();
        self.links.retain(|l| l.to != (to, in_port));
        self.links.len() != before
    }

    /// The link feeding an input port, if any.
    pub fn input_link(&self, to: NodeId, in_port: usize) -> Option<&Link> {
        self.links.iter().find(|l| l.to == (to, in_port))
    }

    /// Sources feeding each input port of a node (`None` when unconnected).
    pub fn inputs(&self, id: NodeId) -> Result<Vec<Option<(NodeId, usize)>>> {
        let desc = self.node(id)?.kind.describe();
        Ok((0..desc.inputs.len())
            .map(|i| self.input_link(id, i).map(|l| l.from))
            .collect())
    }

    /// Direct predecessors of a node.
    pub fn direct_upstream(&self, id: NodeId) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self
            .links
            .iter()
            .filter(|l| l.to.0 == id)
            .map(|l| l.from.0)
            .collect();
        v.dedup();
        v
    }

    /// Direct successors of a node.
    pub fn direct_downstream(&self, id: NodeId) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self
            .links
            .iter()
            .filter(|l| l.from.0 == id)
            .map(|l| l.to.0)
            .collect();
        v.dedup();
        v
    }

    /// All transitive ancestors of a node, in topological order.
    pub fn upstream(&self, id: NodeId) -> Vec<NodeId> {
        self.reach(id, false)
    }

    /// All transitive descendants of a node, in topological order.
    pub fn downstream(&self, id: NodeId) -> Vec<NodeId> {
        self.reach(id, true)
    }

    fn reach(&self, id: NodeId, forward: bool) -> Vec<NodeId> {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let next = if forward {
                self.direct_downstream(n)
            } else {
                self.direct_upstream(n)
            };
            for m in next {
                if seen.insert(m) {
                    stack.push(m);
                }
            }
        }
        let order = self.topological_order().unwrap_or_default();
        order.into_iter().filter(|n| seen.contains(n)).collect()
    }

    fn petgraph(&self) -> (DiGraph<NodeId, ()>, HashMap<NodeId, NodeIndex>) {
        let mut g = DiGraph::new();
        let mut idx = HashMap::new();
        for id in self.nodes.keys() {
            idx.insert(*id, g.add_node(*id));
        }
        for l in &self.links {
            if let (Some(&a), Some(&b)) = (idx.get(&l.from.0), idx.get(&l.to.0)) {
                g.add_edge(a, b, ());
            }
        }
        (g, idx)
    }

    /// Nodes in dependency order (sources first). Errors if the graph has a cycle.
    pub fn topological_order(&self) -> Result<Vec<NodeId>> {
        let (g, _) = self.petgraph();
        match toposort(&g, None) {
            Ok(order) => Ok(order.into_iter().map(|i| g[i]).collect()),
            Err(c) => {
                let id = g[c.node_id()];
                Err(Error::Cycle { from: id, to: id })
            }
        }
    }

    /// Structural problems: dangling links, bad ports, type mismatches, cycles,
    /// unconnected required inputs.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for l in &self.links {
            match (self.nodes.get(&l.from.0), self.nodes.get(&l.to.0)) {
                (Some(a), Some(b)) => {
                    let ad = a.kind.describe();
                    let bd = b.kind.describe();
                    match (ad.outputs.get(l.from.1), bd.inputs.get(l.to.1)) {
                        (Some(o), Some(i)) => {
                            if !i.ty.accepts(o.ty) {
                                problems.push(format!(
                                    "{} -> {}: {} into {}",
                                    a.name,
                                    b.name,
                                    o.ty.as_str(),
                                    i.ty.as_str()
                                ));
                            }
                        }
                        _ => problems.push(format!("{} -> {}: bad port index", a.name, b.name)),
                    }
                }
                _ => problems.push("dangling link".to_string()),
            }
        }
        for n in self.nodes.values() {
            for (i, p) in n.kind.describe().inputs.iter().enumerate() {
                if !p.optional && self.input_link(n.id, i).is_none() {
                    problems.push(format!("{}: input `{}` not connected", n.name, p.name));
                }
            }
        }
        if let Err(e) = self.topological_order() {
            problems.push(e.to_string());
        }
        problems
    }

    /// Per-node content hash: own parameters plus the hashes of every
    /// upstream node (so a change anywhere upstream changes the hash).
    pub fn hashes(&self) -> Result<HashMap<NodeId, u64>> {
        let mut out = HashMap::new();
        for id in self.topological_order()? {
            let node = self.node(id)?;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            node.local_hash().hash(&mut h);
            for (port, link) in self.inputs(id)?.into_iter().enumerate() {
                port.hash(&mut h);
                match link {
                    Some((src, out_port)) => {
                        out_port.hash(&mut h);
                        out.get(&src).copied().unwrap_or(0).hash(&mut h);
                    }
                    None => u64::MAX.hash(&mut h),
                }
            }
            out.insert(id, h.finish());
        }
        Ok(out)
    }

    /// Hash of one node including its upstream.
    pub fn node_hash(&self, id: NodeId) -> Result<u64> {
        self.hashes()?
            .get(&id)
            .copied()
            .ok_or(Error::NodeNotFound(id))
    }

    /// Pretty JSON of the whole graph.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse a graph from JSON.
    pub fn from_json(s: &str) -> Result<CompGraph> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_covers_every_kind() {
        for k in NodeKind::ALL {
            let d = k.describe();
            assert_eq!(d.kind, *k);
            assert!(!d.outputs.is_empty(), "{k} has outputs");
            assert_eq!(NodeKind::parse(k.as_str()).unwrap(), *k);
        }
        assert!(NodeKind::parse("nope").is_err());
    }

    #[test]
    fn connect_validates_ports_and_types() {
        let mut g = CompGraph::new();
        let a = g.add(CompNode::new("a", NodeKind::Read)).unwrap();
        let m = g.add(CompNode::merge("m", MergeOp::Over)).unwrap();
        let d = g.add(CompNode::new("d", NodeKind::DeepMerge)).unwrap();
        g.connect(a, 0, m, 0).unwrap();
        assert!(matches!(
            g.connect(a, 1, m, 0),
            Err(Error::NoSuchPort { .. })
        ));
        assert!(matches!(
            g.connect(a, 0, m, 9),
            Err(Error::NoSuchPort { .. })
        ));
        assert!(matches!(
            g.connect(a, 0, d, 0),
            Err(Error::PortTypeMismatch { .. })
        ));
        assert!(g
            .add(CompNode::new("a", NodeKind::Blur).with_random_id())
            .is_err());
    }

    #[test]
    fn cycles_rejected_and_order() {
        let mut g = CompGraph::new();
        let a = g.add(CompNode::new("a", NodeKind::Read)).unwrap();
        let b = g.add(CompNode::new("b", NodeKind::Blur)).unwrap();
        let c = g.add(CompNode::new("c", NodeKind::Blur)).unwrap();
        g.connect(a, 0, b, 0).unwrap();
        g.connect(b, 0, c, 0).unwrap();
        assert!(matches!(g.connect(c, 0, b, 0), Err(Error::Cycle { .. })));
        assert!(matches!(g.connect(b, 0, b, 0), Err(Error::Cycle { .. })));
        assert_eq!(g.topological_order().unwrap(), vec![a, b, c]);
        assert_eq!(g.upstream(c), vec![a, b]);
        assert_eq!(g.downstream(a), vec![b, c]);
        assert!(g.validate().is_empty());
        g.remove(b);
        assert!(g.links.is_empty());
    }

    #[test]
    fn hashes_change_downstream_only() {
        let mut g = CompGraph::new();
        let a = g.add(CompNode::new("a", NodeKind::Read)).unwrap();
        let b = g.add(CompNode::new("b", NodeKind::Blur)).unwrap();
        let c = g.add(CompNode::new("c", NodeKind::Blur)).unwrap();
        g.connect(a, 0, b, 0).unwrap();
        g.connect(b, 0, c, 0).unwrap();
        let h1 = g.hashes().unwrap();
        g.set_param(b, "size", 9.0f64).unwrap();
        let h2 = g.hashes().unwrap();
        assert_eq!(h1[&a], h2[&a]);
        assert_ne!(h1[&b], h2[&b]);
        assert_ne!(h1[&c], h2[&c]);
    }

    #[test]
    fn json_round_trip() {
        let mut g = CompGraph::new();
        let r = g
            .add(CompNode::roto(
                "r",
                vec![Shape::rect(1.0, 2.0, 3.0, 4.0).with_feather(2.0)],
                8,
                8,
            ))
            .unwrap();
        let b = g.add(CompNode::new("b", NodeKind::Blur)).unwrap();
        g.connect(r, 0, b, 0).unwrap();
        let s = g.to_json().unwrap();
        let back = CompGraph::from_json(&s).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn read_source_round_trip() {
        let scene = EntityId::new();
        let cam = EntityId::new();
        let n = CompNode::read_previs("r", scene, PassKind::Depth, cam);
        assert_eq!(
            n.read_source(),
            ReadSource::PrevisPass {
                scene,
                pass: PassKind::Depth,
                camera: cam
            }
        );
        let mut n = CompNode::new("m", NodeKind::Merge);
        n.set_param("operation", "screen");
        assert_eq!(
            MergeOp::from_param(n.param("operation").unwrap()),
            Some(MergeOp::Screen)
        );
    }
}
