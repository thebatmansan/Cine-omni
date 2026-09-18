//! The colour a QuickTime or MP4 container declares for a picture track.
//!
//! H.264 may carry its colour description in the stream (the SPS video
//! signal type), and the container may carry it again in the sample entry's
//! `colr` atom — QuickTime's `nclc` (primaries, transfer, matrix) or ISO's
//! `nclx` (the same, and a full-range flag). Plenty of files, including the
//! movies this build writes, declare it only in the container. Reading only
//! the stream would call their colour *assumed* when the file states it.
//!
//! The `mp4` crate does not parse `colr`, so this walks the boxes with
//! [`crate::isobmff`]: `moov/trak[track id]/mdia/minf/stbl/stsd`, then the
//! first sample entry's child boxes. Anything malformed or absent answers
//! `None`; nothing is guessed here.

use crate::frame::ColorMetadata;
use crate::isobmff::{child, children, moov};
use std::path::Path;

/// Colour declared in the `colr` atom of track `track_id`'s first sample entry.
pub fn container_colour(path: &Path, track_id: u32) -> Option<ColorMetadata> {
    let moov = moov(path)?;
    let trak = children(&moov).into_iter().find_map(|(k, trak)| {
        (&k == b"trak" && crate::isobmff::track_id(trak) == Some(track_id)).then_some(trak)
    })?;
    let stsd = child(
        child(child(child(trak, b"mdia")?, b"minf")?, b"stbl")?,
        b"stsd",
    )?;
    // Full box header (4) and entry count (4), then the first entry.
    let entries = children(stsd.get(8..)?);
    let (_, entry) = entries.first()?;
    // A visual sample entry's fixed fields are 78 bytes before its children.
    let colr = child(entry.get(78..)?, b"colr")?;
    let code = |at: usize| -> Option<u8> {
        let v = u16::from_be_bytes(colr.get(at..at + 2)?.try_into().ok()?);
        // 2 is "unspecified": absent, not declared.
        (v != 2).then(|| u8::try_from(v).ok()).flatten()
    };
    match colr.get(0..4)? {
        b"nclc" => Some(ColorMetadata {
            primaries: code(4),
            transfer: code(6),
            matrix: code(8),
            full_range: None,
        }),
        b"nclx" => Some(ColorMetadata {
            primaries: code(4),
            transfer: code(6),
            matrix: code(8),
            full_range: colr.get(10).map(|b| b & 0x80 != 0),
        }),
        _ => None,
    }
}

/// The stream's declaration, with gaps filled from the container's.
///
/// Where both declare a value the stream's wins: it is what the decoder was
/// handed with the pictures. A range the stream leaves at its default — no
/// video signal type at all — takes the container's `nclx` flag when there is
/// one.
pub fn merge(
    stream: ColorMetadata,
    stream_declares_range: bool,
    container: Option<ColorMetadata>,
) -> ColorMetadata {
    let Some(c) = container else {
        return stream;
    };
    ColorMetadata {
        primaries: stream.primaries.or(c.primaries),
        transfer: stream.transfer.or(c.transfer),
        matrix: stream.matrix.or(c.matrix),
        full_range: if stream_declares_range {
            stream.full_range
        } else {
            c.full_range.or(stream.full_range)
        },
    }
}
