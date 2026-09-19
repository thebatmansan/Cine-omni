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

pub mod av;
pub mod budget;
pub mod cache;
pub mod capability;
pub mod clock;
pub mod colr;
pub mod decode_ahead;
pub mod decoder;
pub mod ebml;
pub mod edit_list;
pub mod encode;
pub mod error;
pub mod frame;
pub mod h264_sps;
pub mod identity;
pub mod isobmff;
pub mod locator;
pub mod mhl;
pub mod mov;
pub mod offload;
pub mod pictures;
pub mod probe;
pub mod reader;
pub mod schedule;
pub mod sequence;
pub mod sequence_audio;
pub mod sound;
pub mod testsignal;
pub mod time;
pub mod tmcd;
pub mod ucg;
pub mod video;
pub mod waveform;

pub use av::{AudioScheduler, AudioSink, AudioSource, AudioTimeSource, AvStats, AvStatus};
pub use cache::{CacheStats, FrameCache, FrameKey};
pub use capability::{BackendKind, Capability, Chroma, Verification};
pub use clock::{PlaybackClock, PlaybackRate, PlaybackTelemetry, TransportState};
pub use decode_ahead::{AheadConfig, AheadStats, DecodeAhead};
pub use decoder::{Packet, VideoDecoder};
pub use edit_list::{EditEntry, EditListSource};
pub use error::{Error, Result};
pub use frame::{AlphaMode, AudioFrame, ColorMetadata, Frame, PixelFormat, VideoFrame};
pub use identity::{
    ContainerKind, ContentHash, Derivative, DerivativeKind, MediaFile, PhysicalSource, StreamInfo,
    StreamKind,
};
pub use pictures::Pictures;
pub use probe::probe;
pub use reader::MediaReader;
pub use schedule::{FrameSource, Presentation, Presented, ScheduleStats, Scheduler};
pub use sequence_audio::{SequenceAudio, SoundEntry};
pub use time::{Rate, Rounding, Timecode, Timestamp};
pub use video::VideoReader;
