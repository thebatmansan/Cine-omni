//! Crate error type.

use crate::graph_model::NodeId;
use thiserror::Error;

/// Every failure the compositor can report.
#[derive(Debug, Error)]
pub enum Error {
    /// A node id was not present in the graph.
    #[error("comp node {0} not found")]
    NodeNotFound(NodeId),
    /// A node with the same name already exists in the graph.
    #[error("comp node named `{0}` already exists")]
    DuplicateName(String),
    /// A port index does not exist on the node.
    #[error("node {node} has no {dir} port {port}")]
    NoSuchPort {
        node: NodeId,
        dir: &'static str,
        port: usize,
    },
    /// Two ports of incompatible types were linked.
    #[error("cannot connect {from_ty} output to {to_ty} input")]
    PortTypeMismatch { from_ty: String, to_ty: String },
    /// The link would make the graph cyclic.
    #[error("link {from} -> {to} would create a cycle")]
    Cycle { from: NodeId, to: NodeId },
    /// A required input port is not connected.
    #[error("node `{node}` is missing required input `{port}`")]
    MissingInput { node: String, port: String },
    /// A parameter is missing or has the wrong type.
    #[error("node `{node}`: bad parameter `{param}`: {reason}")]
    BadParam {
        node: String,
        param: String,
        reason: String,
    },
    /// The node produced a deep image where a flat one was expected (or vice versa).
    #[error("node `{node}` produced {got} output but {expected} was expected")]
    OutputMismatch {
        node: String,
        expected: &'static str,
        got: &'static str,
    },
    /// A read source could not be resolved (no pass, no registered file).
    #[error("source not available: {0}")]
    MissingSource(String),
    /// Two images that must agree in size do not.
    #[error("image size mismatch: {0}")]
    SizeMismatch(String),
    /// Expression parse / evaluation error.
    #[error("expression error: {0}")]
    Expr(String),
    /// A node kind string could not be parsed.
    #[error("unknown node kind `{0}`")]
    UnknownKind(String),
    /// The UCG entity has the wrong kind or is malformed.
    #[error("invalid comp entity: {0}")]
    InvalidEntity(String),
    /// The operation is possible in principle but this build refuses it —
    /// with the numbers that made it refuse, never a silent substitution.
    #[error("{0}")]
    Refused(String),
    /// Error bubbled from the UCG.
    #[error(transparent)]
    Ucg(#[from] ucg_core::UcgError),
    /// JSON (de)serialization error.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;
