//! Image sequences as media: a numbered run of files that is one asset.
//!
//! A sequence is how VFX and finishing move picture, and it is not a movie
//! with the frames spread over many files: it has no container, no timescale
//! of its own, and no inherent rate. So a sequence asset states the rate it
//! was ingested at, its length is the numbers it covers, and a number inside
//! that range with no file is a **hole** — reported, and refused when read,
//! never a black frame.
//!
//! ## Identity
//!
//! One asset, one content hash. Hashing every file of a 10,000-frame sequence
//! on ingest is not affordable, so the identity is a hash of what identifies
//! the run: the pattern, the numbers covered, which are missing, and the
//! fingerprints of the first and last file. A different first or last frame, a
//! different length, or a different pattern is a different asset; a sequence
//! whose middle changed keeps its identity, which `media.checksum` and the
//! relink path can still detect against the files themselves.
//!
//! ## Precision
//!
//! This is the build's high-bit-depth input. EXR is read as 32-bit float,
//! whatever it was stored as (half, float or unsigned integer), and reaches
//! the frame unquantised and unclamped — scene values above 1 and below 0
//! survive. 16-bit PNG and TIFF are read as 16-bit. Nothing here converts
//! colour: an EXR is scene-linear and says nothing in H.273 terms, so the
//! frame's colour metadata stays absent and the project's colour rules decide
//! what the file is in, as they do for any other media.

use crate::error::{Error, Result};
use crate::frame::{AlphaMode, ColorMetadata, PixelFormat, VideoFrame};
use crate::identity::{
    ContainerKind, ContentHash, MediaFile, StreamInfo, StreamKind, VideoStreamInfo,
};
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ucg_core::Rate;

/// The rate a sequence takes when nobody says otherwise.
pub const DEFAULT_RATE: Rate = Rate::F24;

/// What kind of picture files a sequence holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SequenceKind {
    Exr,
    Png,
    Tiff,
    Jpeg,
}

impl SequenceKind {
    pub fn from_extension(ext: &str) -> Option<SequenceKind> {
        match ext.to_ascii_lowercase().as_str() {
            "exr" => Some(SequenceKind::Exr),
            "png" => Some(SequenceKind::Png),
            "tif" | "tiff" => Some(SequenceKind::Tiff),
            "jpg" | "jpeg" => Some(SequenceKind::Jpeg),
            _ => None,
        }
    }

    pub fn codec(&self) -> &'static str {
        match self {
            SequenceKind::Exr => "exr",
            SequenceKind::Png => "png",
            SequenceKind::Tiff => "tiff",
            SequenceKind::Jpeg => "jpeg",
        }
    }
}

/// A run of numbered files, as found on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    /// The directory the files are in.
    pub dir: PathBuf,
    /// `shot_0010.####.exr` — the printf-style pattern, digits as `#`.
    pub pattern: String,
    /// Text before and after the number, and how many digits it is padded to.
    pub prefix: String,
    pub suffix: String,
    pub padding: usize,
    pub kind: SequenceKind,
    /// The first and last numbers present.
    pub first: i64,
    pub last: i64,
    /// Numbers inside the range with no file. A hole, not a black frame.
    pub missing: Vec<i64>,
}

impl Sequence {
    /// How many frames the sequence covers, holes included.
    pub fn frames(&self) -> i64 {
        self.last - self.first + 1
    }

    /// The file for frame number `n`, whether or not it exists.
    pub fn file(&self, n: i64) -> PathBuf {
        self.dir.join(format!(
            "{}{:0width$}{}",
            self.prefix,
            n,
            self.suffix,
            width = self.padding
        ))
    }

    /// The file for index `i` counted from the first frame.
    pub fn file_at(&self, index: i64) -> PathBuf {
        self.file(self.first + index)
    }

    pub fn is_hole(&self, n: i64) -> bool {
        self.missing.contains(&n)
    }
}

/// Split a file name into (prefix, number, padding, suffix).
fn split_numbered(name: &str) -> Option<(String, i64, usize, String)> {
    let dot = name.rfind('.')?;
    let (stem, ext) = name.split_at(dot);
    let digits_end = stem.len();
    let digits_start = stem
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, _)| i)?;
    if digits_start == digits_end {
        return None;
    }
    let digits = &stem[digits_start..digits_end];
    Some((
        stem[..digits_start].to_string(),
        digits.parse().ok()?,
        digits.len(),
        ext.to_string(),
    ))
}

/// Find the sequence one file belongs to, by scanning its directory.
///
/// Files are siblings of the same sequence when their name is the same before
/// and after the number and the number has the same padding — `A.0001.exr` and
/// `A.0002.exr`, never `A.0001.exr` and `A.001.exr`, which are two runs.
pub fn discover(one_file: &Path) -> Result<Sequence> {
    let dir = one_file
        .parent()
        .ok_or_else(|| Error::NotFound(one_file.display().to_string()))?
        .to_path_buf();
    let name = one_file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::NotFound(one_file.display().to_string()))?;
    let (prefix, _, padding, suffix) = split_numbered(name).ok_or_else(|| {
        Error::UnknownContainer(format!(
            "{}: an image sequence needs a number in its name, like shot.0001.exr",
            one_file.display()
        ))
    })?;
    let kind = SequenceKind::from_extension(suffix.trim_start_matches('.')).ok_or_else(|| {
        Error::UnknownContainer(format!(
            "{}: not a picture this build reads",
            one_file.display()
        ))
    })?;

    let mut numbers: Vec<i64> = Vec::new();
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let Some(n) = entry.file_name().to_str().and_then(split_numbered) else {
            continue;
        };
        if n.0 == prefix && n.2 == padding && n.3 == suffix {
            numbers.push(n.1);
        }
    }
    if numbers.is_empty() {
        return Err(Error::NotFound(one_file.display().to_string()));
    }
    numbers.sort_unstable();
    numbers.dedup();
    let (first, last) = (numbers[0], numbers[numbers.len() - 1]);
    let missing: Vec<i64> = (first..=last).filter(|n| !numbers.contains(n)).collect();
    Ok(Sequence {
        pattern: format!("{prefix}{}{suffix}", "#".repeat(padding)),
        dir,
        prefix,
        suffix,
        padding,
        kind,
        first,
        last,
        missing,
    })
}

/// The identity of a sequence: what makes it this run rather than another.
pub fn sequence_hash(seq: &Sequence) -> Result<ContentHash> {
    let first = ContentHash::fast(&seq.file(seq.first))?;
    let last = ContentHash::fast(&seq.file(seq.last))?;
    let mut total = 0u64;
    for n in seq.first..=seq.last {
        if !seq.is_hole(n) {
            total += std::fs::metadata(seq.file(n)).map(|m| m.len()).unwrap_or(0);
        }
    }
    let describe = format!(
        "sequence:{}:{}:{}:{}:{}:{}:{}",
        seq.pattern,
        seq.first,
        seq.last,
        seq.missing.len(),
        first.hex(),
        last.hex(),
        total
    );
    Ok(ContentHash::of_bytes(describe.as_bytes(), total))
}

/// What this media's identity is *now*, on disk.
///
/// A file's identity is its bytes; a sequence's is its run — the pattern, the
/// numbers it covers and its ends — so a frame added to the end changes it and
/// a frame rewritten in the middle does not. Everything that asks "is this
/// still the media the project ingested" asks this, so movies and sequences
/// are answered the same way.
pub fn identity_of(info: &MediaFile) -> Result<ContentHash> {
    match &info.container {
        ContainerKind::ImageSequence(seq) => {
            // Re-discover, so numbers added or removed since are seen.
            let now = discover(&seq.file(seq.first))?;
            sequence_hash(&now)
        }
        _ => ContentHash::fast(&info.path),
    }
}

/// Probe a sequence from one of its files, at the rate it is to be read.
pub fn probe_sequence(one_file: &Path, rate: Rate) -> Result<MediaFile> {
    let seq = discover(one_file)?;
    let hash = sequence_hash(&seq)?;
    // The first present frame decides the size and the precision.
    let first_present = (seq.first..=seq.last)
        .find(|n| !seq.is_hole(*n))
        .ok_or_else(|| Error::NotFound(seq.pattern.clone()))?;
    let probe = probe_one(&seq.file(first_present), seq.kind)?;
    Ok(MediaFile {
        path: seq.file(seq.first),
        hash,
        container: ContainerKind::ImageSequence(seq.clone()),
        streams: vec![StreamInfo {
            index: 0,
            kind: StreamKind::Video,
            codec: seq.kind.codec().to_string(),
            time_base: rate,
            duration_ticks: Some(seq.frames()),
            video: Some(VideoStreamInfo {
                width: probe.0,
                height: probe.1,
                frame_rate: Some(rate),
                frame_count: Some(seq.frames() as u64),
                variable_frame_rate: false,
            }),
            audio: None,
            decodable: true,
        }],
        source_timecode: None,
        source: None,
    })
}

/// Size and bit depth of one picture file, without decoding all of it where
/// the format allows.
fn probe_one(path: &Path, kind: SequenceKind) -> Result<(u32, u32, u8)> {
    match kind {
        SequenceKind::Exr => {
            let meta = exr::meta::MetaData::read_from_file(path, false)
                .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?;
            let header = meta
                .headers
                .first()
                .ok_or_else(|| Error::Container(format!("{}: no layer", path.display())))?;
            let size = header.layer_size;
            Ok((size.0 as u32, size.1 as u32, 32))
        }
        _ => {
            let reader = image::ImageReader::open(path)?
                .with_guessed_format()
                .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?;
            let depth = match reader.format() {
                Some(image::ImageFormat::Png) | Some(image::ImageFormat::Tiff) => 16,
                _ => 8,
            };
            let (w, h) = reader
                .into_dimensions()
                .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?;
            Ok((w, h, depth))
        }
    }
}

/// Reads pictures of a sequence, one file at a time.
pub struct SequenceReader {
    seq: Sequence,
    rate: Rate,
    info: MediaFile,
}

impl SequenceReader {
    pub fn open(one_file: &Path, rate: Rate) -> Result<SequenceReader> {
        let info = probe_sequence(one_file, rate)?;
        let seq = match &info.container {
            ContainerKind::ImageSequence(s) => s.clone(),
            _ => unreachable!("probe_sequence makes an image sequence"),
        };
        Ok(SequenceReader { seq, rate, info })
    }

    pub fn from_info(info: MediaFile, rate: Rate) -> Result<SequenceReader> {
        let seq = match &info.container {
            ContainerKind::ImageSequence(s) => s.clone(),
            _ => return Err(Error::UnknownContainer("not an image sequence".into())),
        };
        Ok(SequenceReader { seq, rate, info })
    }

    pub fn info(&self) -> &MediaFile {
        &self.info
    }

    pub fn sequence(&self) -> &Sequence {
        &self.seq
    }

    pub fn frame_count(&self) -> usize {
        self.seq.frames().max(0) as usize
    }

    pub fn time_base(&self) -> Rate {
        self.rate
    }

    /// The picture at `index`, counted from the sequence's first number.
    ///
    /// A number with no file is a hole: it refuses, naming the file it wanted,
    /// rather than handing back black.
    pub fn frame_at_index(&mut self, index: usize) -> Result<Option<VideoFrame>> {
        let n = self.seq.first + index as i64;
        if n > self.seq.last || index >= self.frame_count() {
            return Ok(None);
        }
        let path = self.seq.file(n);
        if self.seq.is_hole(n) {
            return Err(Error::NotFound(format!(
                "{} is missing from the sequence {}",
                path.display(),
                self.seq.pattern
            )));
        }
        let pts = Timestamp::exact(index as i64, self.rate);
        let frame = match self.seq.kind {
            SequenceKind::Exr => read_exr(&path, pts, index as i64)?,
            _ => read_still(&path, pts, index as i64)?,
        };
        Ok(Some(frame))
    }
}

/// One EXR as a 32-bit float RGBA frame.
///
/// Half and unsigned-integer channels arrive as float, which is what they are
/// once read; nothing is clamped, so scene values above 1 survive. An EXR's
/// alpha is premultiplied by the format's convention.
fn read_exr(path: &Path, pts: Timestamp, index: i64) -> Result<VideoFrame> {
    use exr::prelude as x;
    // The picture is built straight into the bytes the frame will carry, not
    // into floats that are then copied into bytes: at sixteen bytes a pixel a
    // 4K EXR is 141 MB, and the copy used to make every one of them cost two.
    //
    // The size comes out of the file's header, so it is checked against what
    // this machine can decode before a byte of it is allocated. An enormous
    // data window — a real one, or a corrupt one — used to be an allocation
    // the kernel answered by killing the application.
    let what = path.display().to_string();
    // The header alone says how big the picture is, so the refusal happens
    // before anything is read, let alone allocated.
    let meta = x::MetaData::read_from_file(path, false).map_err(|e| Error::Decode {
        at: what.clone(),
        message: e.to_string(),
    })?;
    for header in &meta.headers {
        let size = header.layer_size;
        crate::budget::check_picture(&what, size.0 as u64, size.1 as u64, 16)?;
    }
    let image = x::read_first_rgba_layer_from_file(
        path,
        |size: x::Vec2<usize>, _| -> (Vec<u8>, usize) { (vec![0u8; size.0 * size.1 * 16], size.0) },
        |(pixels, width): &mut (Vec<u8>, usize),
         position: x::Vec2<usize>,
         (r, g, b, a): (f32, f32, f32, f32)| {
            let i = (position.1 * *width + position.0) * 16;
            for (k, v) in [r, g, b, a].iter().enumerate() {
                pixels[i + k * 4..i + k * 4 + 4].copy_from_slice(&v.to_ne_bytes());
            }
        },
    )
    .map_err(|e| Error::Decode {
        at: path.display().to_string(),
        message: e.to_string(),
    })?;
    let size = image.layer_data.size;
    let (bytes, _) = image.layer_data.channel_data.pixels;
    Ok(VideoFrame {
        pts,
        frame_index: Some(index),
        timecode: None,
        width: size.0 as u32,
        height: size.1 as u32,
        format: PixelFormat::RgbaF32,
        // Scene-linear, and nothing in H.273 says so: the project's colour
        // rules decide what this file is in, as for any other media.
        color: ColorMetadata::default(),
        alpha: AlphaMode::Premultiplied,
        planes: vec![(Arc::new(bytes), size.0 * 4 * 4)],
    })
}

/// One still as an 8 or 16-bit RGBA frame, at the precision it was stored.
fn read_still(path: &Path, pts: Timestamp, index: i64) -> Result<VideoFrame> {
    let decoded = image::ImageReader::open(path)?
        .with_guessed_format()
        .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?
        .decode()
        .map_err(|e| Error::Decode {
            at: path.display().to_string(),
            message: e.to_string(),
        })?;
    let (width, height) = (decoded.width(), decoded.height());
    let deep = matches!(
        decoded,
        image::DynamicImage::ImageRgb16(_)
            | image::DynamicImage::ImageRgba16(_)
            | image::DynamicImage::ImageLuma16(_)
            | image::DynamicImage::ImageLumaA16(_)
    );
    let (format, bytes, stride) = if deep {
        let rgba = decoded.to_rgba16();
        let stride = width as usize * 4 * 2;
        // Native-endian 16-bit samples, as the frame's other deep formats are.
        let bytes: Vec<u8> = rgba.as_raw().iter().flat_map(|v| v.to_ne_bytes()).collect();
        (PixelFormat::Rgba16, bytes, stride)
    } else {
        let rgba = decoded.to_rgba8();
        let stride = width as usize * 4;
        (PixelFormat::Rgba8, rgba.into_raw(), stride)
    };
    Ok(VideoFrame {
        pts,
        frame_index: Some(index),
        timecode: None,
        width,
        height,
        format,
        color: ColorMetadata::default(),
        alpha: AlphaMode::Straight,
        planes: vec![(Arc::new(bytes), stride)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_splits_into_its_parts_or_is_not_a_sequence() {
        assert_eq!(
            split_numbered("shot_0010.0001.exr"),
            Some(("shot_0010.".into(), 1, 4, ".exr".into()))
        );
        assert_eq!(
            split_numbered("A001C001_0123.png"),
            Some(("A001C001_".into(), 123, 4, ".png".into()))
        );
        assert_eq!(split_numbered("movie.mov"), None);
        assert_eq!(split_numbered("no-extension"), None);
    }

    #[test]
    fn a_hole_is_part_of_the_run_and_named() {
        let dir = tempfile::tempdir().unwrap();
        for n in [1, 2, 4, 5] {
            std::fs::write(dir.path().join(format!("p.{n:04}.png")), b"x").unwrap();
        }
        // A different padding is a different run, not a member of this one.
        std::fs::write(dir.path().join("p.003.png"), b"x").unwrap();
        let seq = discover(&dir.path().join("p.0001.png")).unwrap();
        assert_eq!((seq.first, seq.last), (1, 5));
        assert_eq!(seq.missing, vec![3]);
        assert_eq!(seq.frames(), 5);
        assert_eq!(seq.pattern, "p.####.png");
        assert_eq!(seq.file(4), dir.path().join("p.0004.png"));
    }
}
