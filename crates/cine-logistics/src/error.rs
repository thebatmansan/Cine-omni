//! Crate error type.

use thiserror::Error;
use ucg_core::{EntityId, UcgError};

/// Errors produced by cine-logistics.
#[derive(Debug, Error)]
pub enum Error {
    /// A graph operation failed.
    #[error(transparent)]
    Ucg(#[from] UcgError),
    /// An entity the operation needs does not exist in the graph.
    #[error("entity {0} not found")]
    NotFound(EntityId),
    /// An entity exists but has an unexpected kind.
    #[error("entity {id} has kind `{actual}`, expected `{expected}`")]
    WrongKind {
        id: EntityId,
        expected: &'static str,
        actual: String,
    },
    /// A date string or component was invalid.
    #[error("invalid date: {0}")]
    InvalidDate(String),
    /// A time-of-day string or component was invalid.
    #[error("invalid time: {0}")]
    InvalidTime(String),
}

impl Error {
    /// Convert into the core error so logistics calls can use `?` inside
    /// `Engine::transact` closures (graph errors pass through unchanged,
    /// everything else aborts the transaction with a message).
    pub fn into_ucg(self) -> UcgError {
        match self {
            Error::Ucg(e) => e,
            other => UcgError::Aborted(other.to_string()),
        }
    }
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;
