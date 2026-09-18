//! Walking the boxes of a QuickTime or ISO base media file.
//!
//! The `mp4` crate parses what it needs and drops the rest, so anything this
//! build reads that it does not — `colr`, `tmcd` — walks the boxes itself.
//! Doing that twice would be two parsers of one format, free to disagree about
//! a 64-bit size or a truncated box, so it happens here once.
//!
//! Nothing here reads media. `moov` reads the header and the sample tables;
//! [`sample`] reads a single named range, which is how a timecode track's four
//! bytes are fetched without touching the pictures around them.
//!
//! Everything answers `None` rather than guessing. A malformed box ends the
//! walk where it is: what came before it was read correctly, and what comes
//! after cannot be located.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A `moov` larger than this is refused rather than allocated. Sample tables
/// for hours of media are a few tens of megabytes; a quarter of a gigabyte is
/// already a file saying something untrue about itself.
const MAX_MOOV: u64 = 256 * 1024 * 1024;

/// Children of a box body: `(type, body)`.
pub fn children(body: &[u8]) -> Vec<([u8; 4], &[u8])> {
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

/// The first child of `body` with this type.
pub fn child<'a>(body: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(body)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, b)| b)
}

/// Follow a path of box types down from a body.
pub fn path<'a>(body: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    path.iter().try_fold(body, |b, kind| child(b, kind))
}

/// The `moov` box of a file, read without reading the media.
pub fn moov(file: &Path) -> Option<Vec<u8>> {
    let mut f = File::open(file).ok()?;
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
            let body_len = size - header;
            if body_len > MAX_MOOV {
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

/// Every `trak` box in a `moov`.
pub fn traks(moov: &[u8]) -> Vec<&[u8]> {
    children(moov)
        .into_iter()
        .filter(|(k, _)| k == b"trak")
        .map(|(_, b)| b)
        .collect()
}

/// A track's id, from its `tkhd`.
pub fn track_id(trak: &[u8]) -> Option<u32> {
    let tkhd = child(trak, b"tkhd")?;
    // Version 1 has 64-bit creation and modification times: the id follows
    // 8 + 8 bytes rather than 4 + 4.
    let at = if tkhd.first() == Some(&1) {
        4 + 16
    } else {
        4 + 8
    };
    Some(u32::from_be_bytes(tkhd.get(at..at + 4)?.try_into().ok()?))
}

/// A track's handler type, from its `mdia/hdlr` — `vide`, `soun`, `tmcd`.
pub fn handler(trak: &[u8]) -> Option<[u8; 4]> {
    let hdlr = path(trak, &[b"mdia", b"hdlr"])?;
    // Full box header (4), then a reserved four bytes, then the type.
    hdlr.get(8..12)?.try_into().ok()
}

/// A track's `stbl`, where its sample tables live.
pub fn stbl(trak: &[u8]) -> Option<&[u8]> {
    path(trak, &[b"mdia", b"minf", b"stbl"])
}

/// The offset of the first chunk of a track's media, from `stco` or `co64`.
pub fn first_chunk_offset(stbl: &[u8]) -> Option<u64> {
    if let Some(stco) = child(stbl, b"stco") {
        // Full box header (4), entry count (4), then 32-bit offsets.
        if u32::from_be_bytes(stco.get(4..8)?.try_into().ok()?) == 0 {
            return None;
        }
        return Some(u64::from(u32::from_be_bytes(
            stco.get(8..12)?.try_into().ok()?,
        )));
    }
    let co64 = child(stbl, b"co64")?;
    if u32::from_be_bytes(co64.get(4..8)?.try_into().ok()?) == 0 {
        return None;
    }
    Some(u64::from_be_bytes(co64.get(8..16)?.try_into().ok()?))
}

/// Read `len` bytes at `offset`. For single small samples, not for media.
pub fn sample(file: &Path, offset: u64, len: usize) -> Option<Vec<u8>> {
    if len > 4096 {
        return None;
    }
    let mut f = File::open(file).ok()?;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}
