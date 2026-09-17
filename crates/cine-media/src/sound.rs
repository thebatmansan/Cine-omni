//! A file's sound, decoded whole, for analysis rather than playback.
//!
//! Playback reads sound in device-sized windows as it plays
//! ([`crate::sequence_audio`]). Some work needs the sound as one piece instead
//! — lining up two cameras by what they both heard is the first — and that is
//! this: the first audio stream of a file, mixed to mono, at a rate the caller
//! chooses, up to a length the caller bounds.

use crate::error::{Error, Result};
use crate::reader::MediaReader;
use crate::video::VideoReader;
use cine_audio::AudioBuffer;
use std::path::Path;
use ucg_core::{Rate, RationalTime, Rounding};

/// A file's sound as one mono piece.
#[derive(Clone, Debug, PartialEq)]
pub struct MonoSound {
    pub samples: Vec<f32>,
    pub rate: u32,
    /// Where the file's first picture sits in its sound, in samples at `rate`:
    /// clip media time is measured from the first picture, and the sound starts
    /// at its own zero. Zero for a file with no pictures.
    pub picture_anchor: i64,
    /// Whether decoding stopped at the caller's bound before the file ended.
    pub truncated: bool,
}

/// Decode the first audio stream of `path` to mono at `rate`, reading at most
/// `max_seconds` of it. `keep_going` is asked between packets; returning false
/// abandons the decode with [`Error::Cancelled`]-like `Unsupported` so a job can
/// be cancelled.
pub fn decode_mono(
    path: &Path,
    rate: u32,
    max_seconds: f64,
    keep_going: &mut dyn FnMut(f32) -> bool,
) -> Result<MonoSound> {
    let mut reader = MediaReader::open_path(path)?;
    let stream = reader
        .info()
        .first_audio()
        .cloned()
        .ok_or_else(|| Error::Unsupported(format!("{} has no sound", path.display())))?;
    let format = stream
        .audio
        .ok_or_else(|| Error::Unsupported("the sound stream declares no format".into()))?;
    let file_rate = format.sample_rate.max(1);
    let limit = (f64::from(file_rate) * max_seconds) as usize;
    let mut mono: Vec<f32> = Vec::new();
    let mut truncated = false;
    while let Some(frame) = reader.next_audio_frame()? {
        let n = frame.frame_count();
        let channels = frame.channels.len().max(1) as f32;
        for i in 0..n {
            let sum: f32 = frame.channels.iter().map(|c| c[i]).sum();
            mono.push(sum / channels);
        }
        if mono.len() >= limit {
            mono.truncate(limit);
            truncated = true;
            break;
        }
        if !keep_going((mono.len() as f32 / limit.max(1) as f32).min(1.0)) {
            return Err(Error::Unsupported("cancelled".into()));
        }
    }
    let samples = if file_rate == rate {
        mono
    } else {
        let mut buffer = AudioBuffer::new(file_rate, 1, mono.len());
        buffer.data = mono;
        buffer.resample(rate).data
    };
    let picture_anchor = match VideoReader::open_path(path) {
        Ok(v) => v
            .extent()
            .map(|(first, _)| {
                first
                    .rescale_checked(
                        Rate::new(rate, 1).expect("non-zero rate"),
                        Rounding::Nearest,
                    )
                    .0
                    .ticks()
            })
            .unwrap_or(0),
        Err(_) => 0,
    };
    Ok(MonoSound {
        samples,
        rate,
        picture_anchor,
        truncated,
    })
}

/// The media offset between two files from the offset between their sounds.
///
/// If `candidate`'s sound at `n + sound_offset` matches `reference`'s at `n`,
/// then in media time — measured from each file's first picture — the candidate
/// is `sound_offset − candidate_anchor + reference_anchor` samples along.
pub fn media_offset(
    reference: &MonoSound,
    candidate: &MonoSound,
    sound_offset_samples: i64,
) -> RationalTime {
    let samples = sound_offset_samples - candidate.picture_anchor + reference.picture_anchor;
    RationalTime::exact(
        samples,
        Rate::new(reference.rate, 1).expect("non-zero rate"),
    )
}
