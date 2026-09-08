//! Media engine errors.
//!
//! Every variant names something that genuinely happens to real media on real
//! productions: a card that was half-copied, a codec nobody licensed, a file
//! that moved. Nothing here silently degrades — a failure to decode is an error
//! the caller must handle, never a black frame pretending to be picture.

use crate::time::FrameRate;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    // ---- identity and management -----------------------------------------
    #[error("media not found at {0}")]
    NotFound(String),
    #[error("media is offline: {0}")]
    Offline(String),
    #[error("content hash mismatch for {path}: expected {expected}, found {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    // ---- container and streams -------------------------------------------
    #[error("unrecognised container for {0}")]
    UnknownContainer(String),
    #[error("container error: {0}")]
    Container(String),
    #[error("stream {0} not found")]
    NoSuchStream(u32),
    #[error("stream {stream} is {actual}, expected {expected}")]
    WrongStreamKind {
        stream: u32,
        actual: &'static str,
        expected: &'static str,
    },

    // ---- decode -----------------------------------------------------------
    /// The honest answer when a codec is present in the file but this build has
    /// no decoder for it. Never substituted with a blank frame.
    #[error(
        "no decoder available for {codec}. This build decodes: {available}. \
         See docs/blueprint/PHASE-2-MEDIA-ENGINE.md for the codec matrix."
    )]
    NoDecoder { codec: String, available: String },
    #[error("decode failed at {at}: {message}")]
    Decode { at: String, message: String },
    #[error("frame {0} could not be located in the index")]
    FrameNotIndexed(i64),

    // ---- time -------------------------------------------------------------
    #[error("invalid time base {num}/{den}")]
    InvalidTimeBase { num: u32, den: u32 },
    #[error("invalid frame rate {num}/{den}")]
    InvalidFrameRate { num: u32, den: u32 },
    #[error("invalid timecode `{0}`")]
    InvalidTimecode(String),
    #[error("drop-frame timecode is not defined for {0:?}")]
    DropFrameUnsupported(FrameRate),
    #[error("seek to {target} is outside the media range {start}..{end}")]
    SeekOutOfRange {
        target: String,
        start: String,
        end: String,
    },

    // ---- graph ------------------------------------------------------------
    #[error("graph: {0}")]
    Ucg(#[from] ucg_core::UcgError),
    #[error("serde: {0}")]
    Serde(String),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
