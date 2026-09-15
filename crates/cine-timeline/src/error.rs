//! Crate error type.

use thiserror::Error;
use ucg_core::{EntityId, RationalTime, UcgError};

/// Errors produced by the timeline model, the OTIO reader, the editing
/// operations and the graph integration.
#[derive(Debug, Error)]
pub enum Error {
    /// An edit was attempted on a locked track. The track is unchanged.
    #[error("track `{0}` is locked")]
    TrackLocked(String),

    /// A failure in the Unified Context Graph.
    #[error(transparent)]
    Ucg(#[from] UcgError),
    /// Malformed JSON.
    #[error("json: {0}")]
    Json(String),
    /// Structurally valid JSON that is not valid OTIO.
    #[error("otio: {0}")]
    Otio(String),
    /// An item index outside the track.
    #[error("index {index} out of range (track has {len} items)")]
    IndexOutOfRange { index: usize, len: usize },
    /// A time outside the track.
    #[error("time {0} is outside the track")]
    TimeOutOfRange(RationalTime),
    /// The operation needs a clip or gap but the item at the index is a transition.
    #[error("item {0} is a transition; expected a clip or gap")]
    NotAnItem(usize),
    /// The operation would have to cut through a transition.
    #[error("a transition overlaps the cut point {0}")]
    TransitionAtCut(RationalTime),
    /// `join` was asked to merge items that do not continue each other in media.
    #[error("items {0} and {1} are not continuous in media")]
    NotContinuous(usize, usize),
    /// An edit that cannot be carried out as asked, with the reason.
    #[error("edit refused: {0}")]
    Edit(String),
    /// A track violates a structural invariant.
    #[error("track invariant violated: {0}")]
    Invariant(String),
    /// A graph node had an unexpected kind.
    #[error("entity {id} is `{actual}`, expected `{expected}`")]
    WrongKind {
        id: EntityId,
        expected: &'static str,
        actual: String,
    },
    /// A graph node lacks a required attribute or component.
    #[error("entity {id} is missing `{attr}`")]
    MissingAttr { id: EntityId, attr: &'static str },
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
