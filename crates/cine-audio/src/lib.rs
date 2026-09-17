//! # cine-audio — the CINE-OMNI DAW engine
//!
//! Multi-channel audio for the unified pipeline: buffers and WAV I/O, a DSP
//! toolkit, VBAP spatial panning into cinema layouts, a session/track/clip
//! model with automation, a stem-aware renderer, automated dialogue isolation
//! and the UCG bindings that keep dialogue placeholders in sync with the
//! screenplay.
//!
//! ```
//! use cine_audio::prelude::*;
//!
//! let sine = AudioBuffer::sine(48_000, 1, 1000.0, 0.5, 1.0);
//! let gains = VbapPanner::new(ChannelLayout::Stereo).gains(0.0, 0.0);
//! assert!((gains[0] - gains[1]).abs() < 1e-6);
//! assert!((sine.rms() - 0.5 / 2f32.sqrt()).abs() < 1e-3);
//! ```

pub mod buffer;
pub mod dsp;
pub mod error;
pub mod isolate;
pub mod mix;
pub mod pan;
pub mod session;
pub mod speed;
pub mod ucg;
pub mod wav;

pub use buffer::{db_to_lin, lin_to_db, AudioBuffer};
pub use dsp::{
    Biquad, BiquadKind, Compressor, CompressorParams, Delay, Envelope, Fade, FadeCurve, Gate,
    GateParams, Limiter, LimiterParams, Spectrogram, Stft,
};
pub use error::{Error, Result};
pub use isolate::{
    dialogue_segments_to_words_hint, isolate_dialogue, IsolationParams, IsolationResult,
};
pub use mix::{render, render_stems, Renderer, BLOCK_SIZE};
pub use pan::{downmix, ChannelLayout, Downmix, ObjectPanParams, PanLaw, Speaker, VbapPanner};
pub use session::{
    Automation, Bus, BusId, Clip, ClipId, ClipSource, Insert, Interpolation, Lane, PanState, Param,
    Send, Session, Stem, Track, TrackId, TrackKind,
};
pub use ucg::{
    clips_synced_to, ensure_dialogue_track_for_scene, ensure_session, load_session, store_session,
    sync_dialogue_placeholders, DialogueSyncReport,
};
pub use wav::{decode_wav, encode_wav, read_wav, write_wav, WavFormat};

/// Commonly used items.
pub mod prelude {
    pub use crate::buffer::{db_to_lin, lin_to_db, AudioBuffer};
    pub use crate::dsp::*;
    pub use crate::error::{Error, Result};
    pub use crate::isolate::{
        dialogue_segments_to_words_hint, isolate_dialogue, IsolationParams, IsolationResult,
    };
    pub use crate::mix::{render, render_stems, Renderer, BLOCK_SIZE};
    pub use crate::pan::*;
    pub use crate::session::*;
    pub use crate::ucg::{
        clips_synced_to, ensure_dialogue_track_for_scene, ensure_session, load_session,
        store_session, sync_dialogue_placeholders, DialogueSyncReport,
    };
    pub use crate::wav::{decode_wav, encode_wav, read_wav, write_wav, WavFormat};
}
