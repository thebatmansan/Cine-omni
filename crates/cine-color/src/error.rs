//! Crate error type.

use thiserror::Error;
use ucg_core::{EntityId, UcgError};

/// Every fallible operation in `cine-color` returns this error.
#[derive(Debug, Error)]
pub enum Error {
    /// A graph operation failed.
    #[error(transparent)]
    Ucg(#[from] UcgError),
    /// Text (OCIO YAML, `.cube`, CDL XML) could not be parsed.
    #[error("parse error at line {line}: {message}")]
    Parse { line: usize, message: String },
    /// A feature that exists in the source format but is not implemented.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A colorspace name that is not in the config or built-in table.
    #[error("unknown colorspace `{0}`")]
    UnknownColorSpace(String),
    /// A display name that is not in the config.
    #[error("unknown display `{0}`")]
    UnknownDisplay(String),
    /// A view name that is not defined for the display.
    #[error("unknown view `{view}` for display `{display}`")]
    UnknownView { display: String, view: String },
    /// A role that is not defined.
    #[error("unknown role `{0}`")]
    UnknownRole(String),
    /// A LUT is malformed or of the wrong dimension.
    #[error("lut: {0}")]
    Lut(String),
    /// Generic invalid argument.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A file could not be read.
    #[error("io `{path}`: {message}")]
    Io { path: String, message: String },
    /// Serialization failure.
    #[error("serde: {0}")]
    Serde(String),
    /// The entity carries no grade stack.
    #[error("entity {0} has no grade stack")]
    NoGrade(EntityId),
    /// The entity carries no `ColorMeta` component.
    #[error("entity {0} has no color metadata")]
    NoColorMeta(EntityId),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

/// Lets `cine-color` calls use `?` inside `Engine::transact` closures
/// (graph errors pass through unchanged, everything else aborts the transaction).
impl From<Error> for UcgError {
    fn from(e: Error) -> Self {
        match e {
            Error::Ucg(u) => u,
            other => UcgError::Aborted(other.to_string()),
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
