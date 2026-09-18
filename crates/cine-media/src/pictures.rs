//! One way to read pictures, whatever they are stored as.
//!
//! A movie's pictures come from a container with its own timescale; a
//! sequence's come from numbered files at the rate the asset was ingested at.
//! Everything that reads pictures — the monitors, proxies, renders — asks the
//! same three questions of either: how many, at what rate, and give me number
//! `i`. So they ask them of this, and no surface has to know which kind it
//! has.
//!
//! What differs travels with the answer rather than being hidden: a sequence's
//! hole refuses by name, and a frame arrives in the format its source holds
//! (8-bit YUV from a movie, 16-bit or 32-bit float RGBA from a sequence),
//! because quietly converting would be the loss this abstraction exists to
//! avoid.

use crate::error::Result;
use crate::frame::VideoFrame;
use crate::identity::{ContainerKind, MediaFile};
use crate::sequence::SequenceReader;
use crate::time::Timestamp;
use crate::video::VideoReader;
use std::path::Path;
use ucg_core::Rate;

/// A source of pictures: a movie, or a numbered run of files.
pub enum Pictures {
    Movie(Box<VideoReader>),
    Sequence(Box<SequenceReader>),
}

impl Pictures {
    /// Open what this media file describes.
    ///
    /// The probe decided which it is; this does not guess from the path.
    pub fn open(info: MediaFile) -> Result<Pictures> {
        match &info.container {
            ContainerKind::ImageSequence(_) => {
                let rate = info
                    .first_video()
                    .and_then(|s| s.video.as_ref().and_then(|v| v.frame_rate))
                    .unwrap_or(crate::sequence::DEFAULT_RATE);
                Ok(Pictures::Sequence(Box::new(SequenceReader::from_info(
                    info, rate,
                )?)))
            }
            _ => Ok(Pictures::Movie(Box::new(VideoReader::open(info)?))),
        }
    }

    /// Probe a path and open it. A file that is one of a numbered run is read
    /// as that run only when asked for; on its own it is one picture.
    pub fn open_path(path: &Path) -> Result<Pictures> {
        Pictures::open(crate::probe::probe(path)?)
    }

    pub fn info(&self) -> &MediaFile {
        match self {
            Pictures::Movie(r) => r.info(),
            Pictures::Sequence(r) => r.info(),
        }
    }

    pub fn frame_count(&self) -> usize {
        match self {
            Pictures::Movie(r) => r.frame_count(),
            Pictures::Sequence(r) => r.frame_count(),
        }
    }

    pub fn time_base(&self) -> Rate {
        match self {
            Pictures::Movie(r) => r.time_base(),
            Pictures::Sequence(r) => r.time_base(),
        }
    }

    /// The first and last presentation times, when the source states them.
    pub fn extent(&self) -> Option<(Timestamp, Timestamp)> {
        match self {
            Pictures::Movie(r) => r.extent(),
            Pictures::Sequence(r) => {
                let rate = r.time_base();
                let count = r.frame_count() as i64;
                (count > 0).then(|| (Timestamp::exact(0, rate), Timestamp::exact(count, rate)))
            }
        }
    }

    /// Picture `index`, counted from the source's first.
    pub fn frame_at_index(&mut self, index: usize) -> Result<Option<VideoFrame>> {
        match self {
            Pictures::Movie(r) => r.frame_at_index(index),
            Pictures::Sequence(r) => r.frame_at_index(index),
        }
    }

    /// Whether this is a numbered run of files rather than a movie.
    pub fn is_sequence(&self) -> bool {
        matches!(self, Pictures::Sequence(_))
    }

    /// Whether these pictures are unbounded scene light rather than code
    /// values — float EXR, as opposed to a movie or an integer still.
    ///
    /// The distinction decides what may be done without a view transform:
    /// scene light has no display meaning until one is applied, and clamping
    /// it into display output would throw away every highlight.
    pub fn is_scene_float(&self) -> bool {
        match self {
            Pictures::Movie(_) => false,
            Pictures::Sequence(r) => r.sequence().kind == crate::sequence::SequenceKind::Exr,
        }
    }

    /// What it is, for a message that has to name it.
    pub fn describe(&self) -> String {
        match self {
            Pictures::Movie(r) => r.info().path.display().to_string(),
            Pictures::Sequence(r) => format!(
                "the image sequence {} ({})",
                r.sequence().pattern,
                r.sequence().kind.codec()
            ),
        }
    }
}

impl std::fmt::Debug for Pictures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pictures::Movie(r) => write!(f, "Movie({})", r.info().path.display()),
            Pictures::Sequence(r) => write!(f, "Sequence({})", r.sequence().pattern),
        }
    }
}
