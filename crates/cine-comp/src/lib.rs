//! # cine-comp — the CINE-OMNI node-based compositor (CPU reference)
//!
//! * [`image`] — scene-linear float [`Image`] buffers (premultiplied RGBA and
//!   AOV channel sets), [`MultiLayerImage`] render layers, [`DeepImage`]
//!   deep samples.
//! * [`graph_model`] — the [`CompGraph`] DAG of [`CompNode`]s: node kinds
//!   with port/parameter descriptions, typed [`Param`]s, link validation,
//!   cycle detection, topological order, JSON.
//! * [`ops`] — pure pixel operations behind every node kind (merge, keying,
//!   color, filters, transforms, roto, defocus, deep, procedural).
//! * [`expr`] — the arithmetic expression language of the `Expression` node.
//! * [`provider`] — [`PassProvider`], through which `Read(PrevisPass)` nodes
//!   pull beauty / depth / normal / id / motion passes from the previs stage
//!   ([`SyntheticPassProvider`] renders analytic test passes).
//! * [`eval`] — the cached, parallel, lazy [`Evaluator`].
//! * [`ucg`] — storing comp graphs in the Unified Context Graph, bound to
//!   shots and previs cameras.
//!
//! ```
//! use cine_comp::prelude::*;
//!
//! let mut g = CompGraph::new();
//! let fg = g.add(CompNode::read_generator("fg", GeneratorKind::Solid, 8, 8)
//!     .with_param("color", [0.5f32, 0.0, 0.0, 0.5])).unwrap();
//! let bg = g.add(CompNode::read_generator("bg", GeneratorKind::Solid, 8, 8)
//!     .with_param("color", [0.0f32, 0.0, 1.0, 1.0])).unwrap();
//! let over = g.add(CompNode::merge("over", MergeOp::Over)).unwrap();
//! g.connect(fg, 0, over, 0).unwrap();
//! g.connect(bg, 0, over, 1).unwrap();
//!
//! let ev = Evaluator::default();
//! let out = ev.evaluate(&g, over, 0).unwrap();
//! assert_eq!(out.get_rgba(3, 3), [0.5, 0.0, 0.5, 1.0]);
//! ```

#![forbid(unsafe_code)]

pub mod error;
pub mod eval;
pub mod expr;
pub mod graph_model;
pub mod image;
pub mod ops;
pub mod provider;
pub mod ucg;

pub use error::{Error, Result};
pub use eval::{EvalStats, Evaluator, Output, WriteRecord};
pub use expr::{Env, Expr};
pub use graph_model::{
    BlurKind, CompGraph, CompNode, CopyChannels, FilterKind, GeneratorKind, KeyerKind, Link,
    MergeOp, NodeDesc, NodeId, NodeKind, Param, ParamDesc, PassKind, Port, PortType,
    ProceduralKind, ReadSource, ReadSourceKind, Shape, ShuffleSource,
};
pub use image::{
    ChannelSet, DeepImage, DeepSample, Image, Layer, MultiLayerImage, RegionOfInterest,
};
pub use provider::{NullPassProvider, PassProvider, SyntheticPassProvider};

impl From<Error> for ucg_core::UcgError {
    fn from(e: Error) -> Self {
        match e {
            Error::Ucg(u) => u,
            other => ucg_core::UcgError::Aborted(other.to_string()),
        }
    }
}

/// Everything most callers need.
pub mod prelude {
    pub use crate::error::{Error, Result};
    pub use crate::eval::{EvalStats, Evaluator, Output};
    pub use crate::graph_model::{
        BlurKind, CompGraph, CompNode, CopyChannels, FilterKind, GeneratorKind, KeyerKind, Link,
        MergeOp, NodeId, NodeKind, Param, PassKind, PortType, ProceduralKind, ReadSource, Shape,
        ShuffleSource,
    };
    pub use crate::image::{
        ChannelSet, DeepImage, DeepSample, Image, MultiLayerImage, RegionOfInterest,
    };
    pub use crate::provider::{PassProvider, SyntheticPassProvider};
    pub use crate::ucg::{
        bind_previs_camera, comps_depending_on, ensure_comp_for_shot, load_graph, mark_dirty,
        store_graph, update_frame_range,
    };
}
