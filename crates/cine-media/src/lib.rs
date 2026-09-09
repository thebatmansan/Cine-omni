//! # cine-media — the CINE-OMNI media foundation
//!
//! Media identity, exact time, container demultiplexing, decoding and caching.
//! Editorial, VFX, colour, audio, dailies and finishing all depend on this
//! layer being correct, so it is built to be honest before it is built to be
//! fast or broad.
//!
//! Three principles govern it:
//!
//! 1. **Time is exact.** Floating-point seconds are never authoritative. See
//!    [`time`].
//! 2. **Source media is never modified.** Everything derived is traceable back
//!    to a content hash of the original.
//! 3. **Missing capability is reported, never faked.** A codec this build
//!    cannot decode returns [`Error::NoDecoder`]; it does not return a blank
//!    frame.

pub mod cache;
pub mod capability;
pub mod clock;
pub mod decoder;
pub mod error;
pub mod frame;
pub mod identity;
pub mod probe;
pub mod reader;
pub mod time;
pub mod ucg;
pub mod video;

pub use cache::{CacheStats, FrameCache, FrameKey};
pub use capability::{BackendKind, Capability, Chroma, Verification};
pub use clock::{PlaybackClock, PlaybackRate, PlaybackTelemetry, TransportState};
pub use decoder::{Packet, VideoDecoder};
pub use error::{Error, Result};
pub use frame::{AlphaMode, AudioFrame, ColorMetadata, Frame, PixelFormat, VideoFrame};
pub use identity::{
    ContainerKind, ContentHash, Derivative, DerivativeKind, MediaFile, PhysicalSource, StreamInfo,
    StreamKind,
};
pub use probe::probe;
pub use reader::MediaReader;
pub use time::{FrameRate, TimeBase, Timecode, Timestamp};
pub use video::VideoReader;
