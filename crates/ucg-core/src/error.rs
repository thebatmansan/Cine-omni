use crate::id::EntityId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UcgError {
    #[error("entity {0} not found")]
    NotFound(EntityId),
    #[error("entity {0} already exists")]
    AlreadyExists(EntityId),
    #[error("attribute `{attr}` not found on {entity}")]
    AttrNotFound { entity: EntityId, attr: String },
    #[error("adding edge {from} -> {to} ({kind}) would create a dependency cycle")]
    Cycle {
        from: EntityId,
        to: EntityId,
        kind: String,
    },
    #[error("type mismatch for `{attr}`: expected {expected}, got {actual}")]
    TypeMismatch {
        attr: String,
        expected: &'static str,
        actual: &'static str,
    },
    #[error("transaction aborted: {0}")]
    Aborted(String),
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("nothing to redo")]
    NothingToRedo,
    #[error("serialization: {0}")]
    Serde(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported schema version {found} (this build supports <= {supported})")]
    SchemaVersion { found: u32, supported: u32 },
    #[error("reactor `{reactor}` failed: {message}")]
    Reactor { reactor: String, message: String },
    #[error("reactor cascade exceeded depth {0}")]
    CascadeDepth(u32),
}

impl From<serde_json::Error> for UcgError {
    fn from(e: serde_json::Error) -> Self {
        UcgError::Serde(e.to_string())
    }
}
impl From<rmp_serde::encode::Error> for UcgError {
    fn from(e: rmp_serde::encode::Error) -> Self {
        UcgError::Serde(e.to_string())
    }
}
impl From<rmp_serde::decode::Error> for UcgError {
    fn from(e: rmp_serde::decode::Error) -> Self {
        UcgError::Serde(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, UcgError>;
