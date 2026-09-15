//! # ucg-core — the Unified Context Graph
//!
//! One reactive, transactional property graph shared by every CINE-OMNI
//! module. Every screenplay line, camera, clip, comp node and grade is a node
//! with one UUID; dependency edges form a DAG that drives change propagation.
//!
//! ```
//! use ucg_core::prelude::*;
//!
//! let engine = Engine::new();
//! let scene = engine.transact("add scene", |tx| {
//!     let s = tx.create("script.scene")?;
//!     tx.set(s, "heading", "INT. KITCHEN - NIGHT")?;
//!     Ok(())
//! }).unwrap();
//! assert_eq!(scene.deltas.len(), 1);
//! ```

pub mod engine;
pub mod error;
pub mod exact_time;
pub mod graph;
pub mod id;
pub mod node;
pub mod persist;
pub mod time;
pub mod tx;
pub mod value;

pub use engine::{ChangeSet, Engine, EngineLimits, EngineStats, History, Plan, Reactor, UndoGroup};
pub use error::{Result, UcgError};
pub use exact_time::{Rate, Rounding};
pub use graph::Graph;
pub use id::EntityId;
pub use node::{Edge, EdgeKind, Node, NodeKind};
pub use persist::{Manifest, OplogReport, Project};
pub use time::{RationalTime, TimeRange};
pub use tx::{Delta, Op, Tx};
pub use value::{Color, Value};

pub mod prelude {
    pub use crate::engine::{ChangeSet, Engine, Plan, Reactor, UndoGroup};
    pub use crate::error::{Result, UcgError};
    pub use crate::graph::Graph;
    pub use crate::id::EntityId;
    pub use crate::node::{Edge, EdgeKind, Node, NodeKind};
    pub use crate::time::{RationalTime, TimeRange};
    pub use crate::tx::{Delta, Op, Tx};
    pub use crate::value::{Color, Value};
}
