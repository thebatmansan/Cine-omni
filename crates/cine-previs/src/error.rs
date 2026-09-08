//! Crate error type.

use thiserror::Error;
use ucg_core::{EntityId, UcgError};

/// Every failure mode of the previs crate.
#[derive(Debug, Error)]
pub enum Error {
    /// A graph operation failed (missing entity, cycle, serialization...).
    #[error(transparent)]
    Ucg(#[from] UcgError),
    /// A JSON (de)serialization step failed.
    #[error("serialization: {0}")]
    Json(#[from] serde_json::Error),
    /// A USD prim path did not validate (`/World/Cameras/CamA` form required).
    #[error("invalid prim path `{0}`: {1}")]
    InvalidPrimPath(String, &'static str),
    /// A prim was requested that does not exist on the stage.
    #[error("prim `{0}` not found")]
    PrimNotFound(String),
    /// A prim already exists at this path.
    #[error("prim `{0}` already exists")]
    PrimExists(String),
    /// The minimal USDA parser rejected the input.
    #[error("usda parse error at line {line}: {message}")]
    Usda { line: usize, message: String },
    /// A typed component expected on a graph node was missing or malformed.
    #[error("entity {entity} is missing component `{component}`")]
    MissingComponent {
        entity: EntityId,
        component: &'static str,
    },
    /// A node of a different kind was found where `expected` was required.
    #[error("entity {entity} is `{actual}`, expected `{expected}`")]
    WrongKind {
        entity: EntityId,
        expected: &'static str,
        actual: String,
    },
    /// An optics computation received a non-physical parameter.
    #[error("optics: {0}")]
    Optics(String),
    /// An animation/rig definition is invalid (no keys, empty track...).
    #[error("rig: {0}")]
    Rig(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
