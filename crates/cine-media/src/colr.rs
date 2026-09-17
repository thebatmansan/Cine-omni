//! The colour a QuickTime or MP4 container declares for a picture track.
//!
//! H.264 may carry its colour description in the stream (the SPS video
//! signal type), and the container may carry it again in the sample entry's
//! `colr` atom — QuickTime's `nclc` (primaries, transfer, matrix) or ISO's
//! `nclx` (the same, and a full-range flag). Plenty of files, including the
//! movies this build writes, declare it only in the container. Reading only
//! the stream would call their colour *assumed* when the file states it.
//!
//! The `mp4` crate does not parse `colr`, so this walks the boxes itself:
//! `moov/trak[track id]/mdia/minf/stbl/stsd`, then the first sample entry's
//! child boxes. Anything malformed or absent answers `None`; nothing is
//! guessed here.

use crate::frame::ColorMetadata;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Children of a box body: `(type, body)`.
fn children(body: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= body.len() {
        let size32 = u32::from_be_bytes(body[i..i + 4].try_into().expect("4 bytes")) as u64;
        let kind: [u8; 4] = body[i + 4..i + 8].try_into().expect("4 bytes");
        let (header, size) = match size32 {
            1 if i + 16 <= body.len() => (
                16usize,
                u64::from_be_bytes(body[i + 8..i + 16].try_into().expect("8 bytes")),
            ),
            0 => (8, (body.len() - i) as u64),
            n => (8, n),
        };
        let end = i as u64 + size;
        if size < header as u64 || end > body.len() as u64 {
            break;
        }
        out.push((kind, &body[i + header..end as usize]));
        i = end as usize;
    }
    out
}

fn child<'a>(body: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(body).into_iter().find(|(k, _)| k == kind).map(|(_, b)| b)
}

/// The `moov` box of a file, read without reading the media.
fn moov(path: &Path) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut head = [0u8; 16];
        f.read_exact(&mut head[..8]).ok()?;
        let size32 = u32::from_be_bytes(head[0..4].try_into().ok()?) as u64;
        let (header, size) = match size32 {
            1 => {
                f.read_exact(&mut head[8..16]).ok()?;
                (16u64, u64::from_be_bytes(head[8..16].try_into().ok()?))
            }
            0 => (8, len - at),
            n => (8, n),
        };
        if size < header {
            return None;
        }
        if &head[4..8] == b"moov" {
            // A moov is sample tables and descriptions; refuse absurd sizes
            // rather than allocating them.
            let body_len = size - header;
            if body_len > 256 * 1024 * 1024 {
                return None;
            }
            let mut body = vec![0u8; body_len as usize];
            f.read_exact(&mut body).ok()?;
            return Some(body);
        }
        at += size;
    }
    None
}

/// Colour declared in the `colr` atom of track `track_id`'s first sample entry.
pub fn container_colour(path: &Path, track_id: u32) -> Option<ColorMetadata> {
    let moov = moov(path)?;
    let trak = children(&moov).into_iter().find_map(|(k, trak)| {
        if &k != b"trak" {
            return None;
        }
        let tkhd = child(trak, b"tkhd")?;
        // version 1 has 64-bit times: the id follows 8 + 8 bytes, else 4 + 4.
        let id_at = if tkhd.first() == Some(&1) { 4 + 16 } else { 4 + 8 };
        let id = u32::from_be_bytes(tkhd.get(id_at..id_at + 4)?.try_into().ok()?);
        (id == track_id).then_some(trak)
    })?;
    let stsd = child(child(child(child(trak, b"mdia")?, b"minf")?, b"stbl")?, b"stsd")?;
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
pub fn merge(stream: ColorMetadata, stream_declares_range: bool, container: Option<ColorMetadata>) -> ColorMetadata {
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
