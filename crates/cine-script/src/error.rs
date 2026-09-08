//! Crate error type.

use thiserror::Error;
use ucg_core::{EntityId, UcgError};

/// Errors produced by cine-script.
#[derive(Debug, Error)]
pub enum Error {
    /// A graph operation failed.
    #[error(transparent)]
    Ucg(#[from] UcgError),
    /// The XML could not be parsed.
    #[error("xml: {0}")]
    Xml(String),
    /// The document is well-formed but not a screenplay we understand.
    #[error("invalid document: {0}")]
    InvalidDocument(String),
    /// A node of an unexpected kind was passed to a graph function.
    #[error("entity {id} is `{actual}`, expected `{expected}`")]
    WrongKind {
        id: EntityId,
        expected: String,
        actual: String,
    },
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
