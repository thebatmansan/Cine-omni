//! The source timecode a QuickTime movie carries.
//!
//! A camera stamps every take with the time of day, or with a running record
//! count, and that number is what the rest of the industry uses to talk about
//! a frame: an EDL's source-in, a dailies report, a note from the director,
//! a conform against a cut made somewhere else. A frame index counted from
//! zero is *this build's* name for a frame; the timecode is everyone's.
//!
//! ## What a timecode track is
//!
//! It is a real track, with a `tmcd` handler, one sample description and — for
//! the ordinary case of a take with continuous timecode — one sample. The
//! sample description says how to read the number:
//!
//! ```text
//! flags          bit 0 drop frame, bit 1 24-hour wrap, bit 2 negative allowed,
//!                bit 3 counter rather than time
//! time scale     ticks a second
//! frame duration ticks a frame       →  the exact rate is scale / duration
//! frames         the timecode's own labelling rate (30 for 29.97 drop frame)
//! ```
//!
//! and the sample is a single 32-bit frame number. So a 29.97 drop-frame take
//! starting at 10:00:00;00 has a scale of 30000, a frame duration of 1001, a
//! `frames` of 30, the drop-frame flag set, and a sample of 1078920.
//!
//! ## Exactness
//!
//! The rate comes out of the file as the ratio it is — 30000/1001, never
//! 29.97 — and goes straight into [`Rate`], which is the same rate the rest of
//! the project counts in. The frame number is decomposed into a label by
//! [`Timecode`], which owns the drop-frame rules. Nothing here does timecode
//! arithmetic of its own.
//!
//! ## What is not claimed
//!
//! A *counter* track (flag bit 3) counts something that is not time, so it is
//! read and reported as a counter rather than dressed up as a clock. Discontinuous
//! timecode — a track with several samples, one per run — is read as the first
//! run's start, which is what a take's start timecode means; this does not
//! reconstruct a jam-sync break in the middle of a file, and says so rather
//! than pretending the take is continuous.

use crate::isobmff::{child, first_chunk_offset, handler, moov, sample, stbl, traks};
use crate::time::Timecode;
use std::path::Path;
use ucg_core::Rate;

/// What a movie's timecode track says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceTimecode {
    /// The first frame's timecode.
    pub start: Timecode,
    /// The exact rate the track counts at: time scale over frame duration.
    pub rate: Rate,
    /// The frame number the sample held, before it was made into a label.
    pub start_frame: i64,
    /// The track counts something that is not the time of day.
    pub counter: bool,
    /// How many samples the track holds. More than one means the timecode is
    /// discontinuous, and only the first run's start is read.
    pub runs: u32,
}

/// Flags in a `tmcd` sample description.
const DROP_FRAME: u32 = 0x0001;
const COUNTER: u32 = 0x0008;

/// The source timecode of a QuickTime or MP4 file, if it carries one.
///
/// `None` means the file has no timecode track, or has one this cannot read.
/// It never means zero: a take that starts at 00:00:00:00 says so, and a take
/// with no timecode at all is a different fact entirely.
pub fn source_timecode(file: &Path) -> Option<SourceTimecode> {
    let moov = moov(file)?;
    let trak = traks(&moov)
        .into_iter()
        .find(|t| handler(t) == Some(*b"tmcd"))?;
    let stbl = stbl(trak)?;
    let stsd = child(stbl, b"stsd")?;
    // Full box header (4) and entry count (4), then the entries.
    let entry = crate::isobmff::children(stsd.get(8..)?)
        .into_iter()
        .find(|(k, _)| k == b"tmcd")
        .map(|(_, b)| b)?;
    // A sample entry's six reserved bytes and data reference index, then four
    // reserved bytes, then the timecode fields.
    let at = |o: usize| -> Option<u32> {
        Some(u32::from_be_bytes(entry.get(o..o + 4)?.try_into().ok()?))
    };
    let flags = at(12)?;
    let time_scale = at(16)?;
    let frame_duration = at(20)?;
    let frames = *entry.get(24)?;
    if time_scale == 0 || frame_duration == 0 || frames == 0 {
        return None;
    }
    let rate = Rate::new(time_scale, frame_duration)?;

    // The frame number is the track's one sample: four bytes at its first
    // chunk. Reading it does not read any media.
    let offset = first_chunk_offset(stbl)?;
    let bytes = sample(file, offset, 4)?;
    let start_frame = i64::from(u32::from_be_bytes(bytes[..4].try_into().ok()?));

    let drop_frame = flags & DROP_FRAME != 0;
    let counter = flags & COUNTER != 0;
    // The label's rate is the `frames` field — 30 for 29.97 drop frame — and
    // the exact rate is the ratio. `Timecode` is told the exact rate and
    // derives the labelling rate from it, so a file whose two disagree is one
    // this build does not claim to understand.
    if u32::from(frames) != rate.timecode_rate() {
        return None;
    }
    let start = Timecode::from_frame_number(start_frame, rate, drop_frame).ok()?;
    Some(SourceTimecode {
        start,
        rate,
        start_frame,
        counter,
        runs: sample_count(stbl).unwrap_or(1),
    })
}

/// How many samples a track holds, from `stsz`.
fn sample_count(stbl: &[u8]) -> Option<u32> {
    let stsz = child(stbl, b"stsz")?;
    // Full box header (4), sample size (4), then the count.
    Some(u32::from_be_bytes(stsz.get(8..12)?.try_into().ok()?))
}
