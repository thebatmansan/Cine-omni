//! Crate error type.

use thiserror::Error;
use ucg_core::{EntityId, UcgError};

/// Errors produced by `cine-audio`.
#[derive(Debug, Error)]
pub enum Error {
    /// A graph operation failed.
    #[error("graph: {0}")]
    Ucg(#[from] UcgError),
    /// An I/O error while reading or writing audio files.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A malformed or unsupported WAV file.
    #[error("wav: {0}")]
    Wav(String),
    /// An argument was out of range or inconsistent.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Two buffers could not be combined (rate or channel mismatch).
    #[error("buffer mismatch: {0}")]
    Mismatch(String),
    /// An entity of the expected kind was not found in the graph.
    #[error("entity {0} is not a {1}")]
    WrongKind(EntityId, &'static str),
    /// A session routing graph contains a bus cycle.
    #[error("bus routing cycle through {0}")]
    BusCycle(EntityId),
    /// (De)serialization of a component failed.
    #[error("serde: {0}")]
    Serde(String),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
