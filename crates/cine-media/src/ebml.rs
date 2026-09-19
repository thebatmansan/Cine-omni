//! What a Matroska file says its tracks are.
//!
//! The streaming demuxer this build uses for Matroska is an audio library. It
//! surfaces *every* track — video included — but fills in parameters only for
//! the ones that carry sound, so a picture track arrives with no codec, no
//! sample rate and no channel count. Reading that as a sound track produced a
//! phantom: a 48 kHz stereo stream, invented out of two `unwrap_or` defaults,
//! for a track that has no sound in it at all.
//!
//! So the track list is read here, from the file. EBML is a tagged tree like
//! ISO base media's boxes, with variable-length ids and sizes:
//!
//! ```text
//! Segment → Info   → TimestampScale, Duration
//!         → Tracks → TrackEntry → TrackNumber, TrackType, CodecID,
//!                                 DefaultDuration,
//!                                 Video → PixelWidth, PixelHeight
//!                                 Audio → SamplingFrequency, Channels, BitDepth
//! ```
//!
//! Only the header is read; no cluster is touched, so this costs a seek and a
//! few kilobytes whatever the file's length.
//!
//! ## What this is not
//!
//! It is not a demuxer. Nothing here decodes a Matroska picture, and nothing
//! here claims it can: a video track is described, and whether this build has
//! a decoder for its codec is the capability registry's answer, not this
//! module's. Reading what is in a file and being able to play it are different
//! questions, and the honest failure is to describe the track and refuse it.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Element ids, as they appear on the wire including their length marker.
mod id {
    pub const SEGMENT: u64 = 0x1853_8067;
    pub const INFO: u64 = 0x1549_A966;
    pub const TIMESTAMP_SCALE: u64 = 0x2AD7B1;
    pub const DURATION: u64 = 0x4489;
    pub const TRACKS: u64 = 0x1654_AE6B;
    pub const TRACK_ENTRY: u64 = 0xAE;
    pub const TRACK_NUMBER: u64 = 0xD7;
    pub const TRACK_TYPE: u64 = 0x83;
    pub const CODEC_ID: u64 = 0x86;
    pub const DEFAULT_DURATION: u64 = 0x23E383;
    pub const VIDEO: u64 = 0xE0;
    pub const PIXEL_WIDTH: u64 = 0xB0;
    pub const PIXEL_HEIGHT: u64 = 0xBA;
    pub const AUDIO: u64 = 0xE1;
    pub const SAMPLING_FREQUENCY: u64 = 0xB5;
    pub const CHANNELS: u64 = 0x9F;
    pub const BIT_DEPTH: u64 = 0x6264;
}

/// What kind of thing a track carries, as Matroska numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackType {
    Video,
    Audio,
    Subtitle,
    /// Complex, logo, buttons, control, metadata — described, not interpreted.
    Other(u64),
}

impl TrackType {
    fn of(n: u64) -> TrackType {
        match n {
            1 => TrackType::Video,
            2 => TrackType::Audio,
            0x11 => TrackType::Subtitle,
            other => TrackType::Other(other),
        }
    }
}

/// One track, as the file describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct MatroskaTrack {
    pub number: u64,
    pub kind: TrackType,
    /// The Matroska codec id, verbatim — `V_MPEG4/ISO/AVC`, `A_PCM/INT/LIT`.
    pub codec_id: String,
    /// Nanoseconds a frame lasts, where the file states it.
    pub default_duration_ns: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_rate: Option<f64>,
    pub channels: Option<u16>,
    pub bit_depth: Option<u16>,
}

/// A Matroska file's header.
#[derive(Clone, Debug, PartialEq)]
pub struct Matroska {
    pub tracks: Vec<MatroskaTrack>,
    /// Nanoseconds a timestamp tick is worth. One millisecond by default.
    pub timestamp_scale: u64,
    /// Length in timestamp ticks, where the file states it.
    pub duration: Option<f64>,
}

/// How far into a file the Tracks element will be looked for.
///
/// Headers live at the front. A file that puts them past this is one this does
/// not read, rather than one this reads slowly.
const MAX_HEADER: u64 = 64 * 1024 * 1024;

/// A single element's size cap: a header element larger than this is a file
/// saying something untrue about itself.
const MAX_ELEMENT: u64 = 8 * 1024 * 1024;

/// Read the header of a Matroska or WebM file.
///
/// `None` where the file is not Matroska, or is malformed in a way that means
/// the track list cannot be located. Nothing is guessed.
pub fn read_header(path: &Path) -> Option<Matroska> {
    let mut f = BufReader::new(File::open(path).ok()?);
    let len = f.get_ref().metadata().ok()?.len();

    // The top level is an EBML header followed by one or more Segments.
    let (mut at, end) = (0u64, len.min(MAX_HEADER));
    let mut segment = None;
    while at < end {
        let (id, size, header) = element(&mut f, at)?;
        if id == id::SEGMENT {
            // An unknown-length Segment runs to the end of the file.
            let body = at + header;
            segment = Some((body, if size == UNKNOWN { len } else { body + size }));
            break;
        }
        at = at.checked_add(header)?.checked_add(size)?;
    }
    let (from, to) = segment?;

    let mut out = Matroska {
        tracks: Vec::new(),
        timestamp_scale: 1_000_000,
        duration: None,
    };
    let mut at = from;
    let to = to.min(len);
    let mut seen_tracks = false;
    while at < to && at < MAX_HEADER {
        let (id, size, header) = element(&mut f, at)?;
        let body = at + header;
        match id {
            id::INFO => read_info(&mut f, body, body + size, &mut out),
            id::TRACKS => {
                read_tracks(&mut f, body, body + size, &mut out);
                seen_tracks = true;
            }
            _ => {}
        }
        if id == id::TRACKS && seen_tracks && !out.tracks.is_empty() {
            // Everything after the header is clusters; nothing else is read.
            break;
        }
        if size == UNKNOWN {
            return seen_tracks.then_some(out);
        }
        at = at.checked_add(header)?.checked_add(size)?;
    }
    seen_tracks.then_some(out)
}

fn read_info(f: &mut BufReader<File>, from: u64, to: u64, out: &mut Matroska) {
    let mut at = from;
    while at < to {
        let Some((id, size, header)) = element(f, at) else {
            return;
        };
        if size == UNKNOWN || size > MAX_ELEMENT {
            return;
        }
        match id {
            id::TIMESTAMP_SCALE => {
                if let Some(v) = uint(f, at + header, size) {
                    out.timestamp_scale = v.max(1);
                }
            }
            id::DURATION => out.duration = float(f, at + header, size),
            _ => {}
        }
        at = at + header + size;
    }
}

fn read_tracks(f: &mut BufReader<File>, from: u64, to: u64, out: &mut Matroska) {
    let mut at = from;
    while at < to {
        let Some((id, size, header)) = element(f, at) else {
            return;
        };
        if size == UNKNOWN || size > MAX_ELEMENT {
            return;
        }
        if id == id::TRACK_ENTRY {
            if let Some(t) = read_track_entry(f, at + header, at + header + size) {
                out.tracks.push(t);
            }
        }
        at = at + header + size;
    }
}

fn read_track_entry(f: &mut BufReader<File>, from: u64, to: u64) -> Option<MatroskaTrack> {
    let mut t = MatroskaTrack {
        number: 0,
        kind: TrackType::Other(0),
        codec_id: String::new(),
        default_duration_ns: None,
        width: None,
        height: None,
        sample_rate: None,
        channels: None,
        bit_depth: None,
    };
    let mut at = from;
    while at < to {
        let (id, size, header) = element(f, at)?;
        if size == UNKNOWN || size > MAX_ELEMENT {
            return None;
        }
        let body = at + header;
        match id {
            id::TRACK_NUMBER => t.number = uint(f, body, size)?,
            id::TRACK_TYPE => t.kind = TrackType::of(uint(f, body, size)?),
            id::CODEC_ID => t.codec_id = text(f, body, size)?,
            id::DEFAULT_DURATION => t.default_duration_ns = uint(f, body, size),
            id::VIDEO => {
                let (mut i, stop) = (body, body + size);
                while i < stop {
                    let (vid, vsize, vheader) = element(f, i)?;
                    if vsize == UNKNOWN || vsize > MAX_ELEMENT {
                        return None;
                    }
                    match vid {
                        id::PIXEL_WIDTH => t.width = uint(f, i + vheader, vsize).map(|v| v as u32),
                        id::PIXEL_HEIGHT => {
                            t.height = uint(f, i + vheader, vsize).map(|v| v as u32)
                        }
                        _ => {}
                    }
                    i = i + vheader + vsize;
                }
            }
            id::AUDIO => {
                let (mut i, stop) = (body, body + size);
                while i < stop {
                    let (aid, asize, aheader) = element(f, i)?;
                    if asize == UNKNOWN || asize > MAX_ELEMENT {
                        return None;
                    }
                    match aid {
                        id::SAMPLING_FREQUENCY => t.sample_rate = float(f, i + aheader, asize),
                        id::CHANNELS => t.channels = uint(f, i + aheader, asize).map(|v| v as u16),
                        id::BIT_DEPTH => {
                            t.bit_depth = uint(f, i + aheader, asize).map(|v| v as u16)
                        }
                        _ => {}
                    }
                    i = i + aheader + asize;
                }
            }
            _ => {}
        }
        at = body + size;
    }
    (t.number > 0).then_some(t)
}

/// A size field of all ones: the element runs to the end of its parent.
const UNKNOWN: u64 = u64::MAX;

/// Read one element's `(id, size, header length)` at `at`.
fn element(f: &mut BufReader<File>, at: u64) -> Option<(u64, u64, u64)> {
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut head = [0u8; 16];
    // An id is at most 4 bytes and a size at most 8, so 12 always covers both;
    // a short read at the end of the file simply fails to parse.
    let read = read_upto(f, &mut head[..12])?;
    let head = &head[..read];
    let (id, id_len) = vint(head, true)?;
    let (size, size_len) = vint(head.get(id_len..)?, false)?;
    Some((id, size, (id_len + size_len) as u64))
}

fn read_upto(f: &mut BufReader<File>, buf: &mut [u8]) -> Option<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match f.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    (filled > 0).then_some(filled)
}

/// A variable-length integer. `keep_marker` is true for ids, whose value
/// includes the length marker, and false for sizes, whose does not.
fn vint(b: &[u8], keep_marker: bool) -> Option<(u64, usize)> {
    let first = *b.first()?;
    if first == 0 {
        // Five or more leading zero bits: longer than this format allows.
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || b.len() < len {
        return None;
    }
    if keep_marker {
        let mut v = 0u64;
        for byte in &b[..len] {
            v = (v << 8) | u64::from(*byte);
        }
        return Some((v, len));
    }
    let mut v = u64::from(first & (0xFFu8 >> len));
    let mut all_ones = v == u64::from(0xFFu8 >> len);
    for byte in &b[1..len] {
        v = (v << 8) | u64::from(*byte);
        all_ones &= *byte == 0xFF;
    }
    Some((if all_ones { UNKNOWN } else { v }, len))
}

fn bytes(f: &mut BufReader<File>, at: u64, size: u64) -> Option<Vec<u8>> {
    if size == 0 || size > MAX_ELEMENT {
        return None;
    }
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn uint(f: &mut BufReader<File>, at: u64, size: u64) -> Option<u64> {
    if size > 8 {
        return None;
    }
    let b = bytes(f, at, size)?;
    Some(b.iter().fold(0u64, |v, byte| (v << 8) | u64::from(*byte)))
}

fn float(f: &mut BufReader<File>, at: u64, size: u64) -> Option<f64> {
    let b = bytes(f, at, size)?;
    match size {
        4 => Some(f64::from(f32::from_be_bytes(b[..4].try_into().ok()?))),
        8 => Some(f64::from_be_bytes(b[..8].try_into().ok()?)),
        _ => None,
    }
}

fn text(f: &mut BufReader<File>, at: u64, size: u64) -> Option<String> {
    let b = bytes(f, at, size)?;
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    Some(String::from_utf8_lossy(&b[..end]).into_owned())
}
