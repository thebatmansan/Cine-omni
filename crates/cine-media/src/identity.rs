//! Media identity.
//!
//! The chain professional systems need, and the reason each link exists:
//!
//! ```text
//! PhysicalSource   a camera card / reel / roll        "A001" from camera A, day 1
//!   └─ MediaFile   one file on disk, content-hashed   A001C003_190101_R1AB.mov
//!        └─ Container  the parsed wrapper             QuickTime, MP4, MXF
//!             └─ Stream  one elementary stream        video track 1, audio track 2
//! ```
//!
//! Everything downstream — proxies, transcodes, thumbnails, caches — is a
//! *derivative* that names the `MediaFile` it came from, so a replacement can
//! always find what depends on it.
//!
//! **Identity is content-derived, not path-derived.** A file that moves is the
//! same media; a file that is re-exported is not. This is what makes relinking
//! reliable and what lets duplicate detection work. Avid solved this with MOB
//! IDs, AAF with UMIDs; CINE-OMNI uses a content hash plus the deterministic
//! UUID scheme the UCG already has.

use crate::error::{Error, Result};
use crate::time::{Rate, Timecode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use ucg_core::EntityId;

/// How much of a file is hashed for the fast identity fingerprint.
///
/// Hashing a 200 GB camera file end to end on ingest is not acceptable, so the
/// default fingerprint reads the head, the tail and the length. That is enough
/// to distinguish files in practice while staying O(1) in file size. A full
/// content hash is available for verification passes, where the cost is
/// justified and the guarantee needs to be absolute.
pub const FINGERPRINT_CHUNK: u64 = 1 << 20; // 1 MiB from each end

/// A content-derived identity for a media file.
///
/// `Fast` is for ingest and relinking. `Full` is for verification and archive,
/// and is what a checksum manifest such as ASC MHL would record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContentHash {
    /// SHA-256 over head + tail + length. Cheap, and stable for a given file.
    Fast { hex: String, size: u64 },
    /// SHA-256 over the entire file.
    Full { hex: String, size: u64 },
}

impl ContentHash {
    pub fn hex(&self) -> &str {
        match self {
            ContentHash::Fast { hex, .. } | ContentHash::Full { hex, .. } => hex,
        }
    }
    pub fn size(&self) -> u64 {
        match self {
            ContentHash::Fast { size, .. } | ContentHash::Full { size, .. } => *size,
        }
    }
    pub fn is_full(&self) -> bool {
        matches!(self, ContentHash::Full { .. })
    }

    /// Head+tail+length fingerprint. O(1) in file size.
    pub fn fast(path: &Path) -> Result<ContentHash> {
        let mut f = File::open(path).map_err(|_| Error::NotFound(path.display().to_string()))?;
        let size = f.metadata()?.len();
        let mut h = Sha256::new();
        h.update(size.to_le_bytes());

        let chunk = FINGERPRINT_CHUNK.min(size);
        let mut buf = vec![0u8; chunk as usize];
        f.read_exact(&mut buf)?;
        h.update(&buf);

        if size > chunk {
            let tail = chunk.min(size - chunk);
            f.seek(SeekFrom::End(-(tail as i64)))?;
            let mut tbuf = vec![0u8; tail as usize];
            f.read_exact(&mut tbuf)?;
            h.update(&tbuf);
        }
        Ok(ContentHash::Fast {
            hex: format!("{:x}", h.finalize()),
            size,
        })
    }

    /// Whole-file SHA-256, for verification and archival manifests.
    pub fn full(path: &Path) -> Result<ContentHash> {
        let f = File::open(path).map_err(|_| Error::NotFound(path.display().to_string()))?;
        let size = f.metadata()?.len();
        let mut r = BufReader::new(f);
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(ContentHash::Full {
            hex: format!("{:x}", h.finalize()),
            size,
        })
    }

    /// Whole-file SHA-256 for long files: reports `(bytes read, total)` after
    /// every chunk and stops when `keep_going` returns `false`, returning `None`.
    /// The same digest as [`ContentHash::full`].
    pub fn full_with_progress(
        path: &Path,
        keep_going: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<Option<ContentHash>> {
        let f = File::open(path).map_err(|_| Error::NotFound(path.display().to_string()))?;
        let size = f.metadata()?.len();
        let mut r = BufReader::new(f);
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 4 << 20];
        let mut read = 0u64;
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            read += n as u64;
            if !keep_going(read, size) {
                return Ok(None);
            }
        }
        Ok(Some(ContentHash::Full {
            hex: format!("{:x}", h.finalize()),
            size,
        }))
    }

    /// Verify that a file's **content** still matches this hash.
    ///
    /// Only a [`ContentHash::Full`] can do this. A `Fast` hash covers the head,
    /// the tail and the length, so it detects replacement and truncation but is
    /// blind to a flipped bit in the middle of the file — precisely the
    /// corruption a card offload needs to catch. Verifying against one and
    /// reporting success would be the worst possible answer, so it is refused
    /// with [`Error::CannotVerify`] instead.
    ///
    /// Use [`ContentHash::matches_quickly`] when a cheap check is what is
    /// wanted, and be clear in the caller that it is not verification.
    pub fn verify(&self, path: &Path) -> Result<()> {
        if !self.is_full() {
            return Err(Error::CannotVerify {
                path: path.display().to_string(),
            });
        }
        self.compare(path, ContentHash::full(path)?)
    }

    /// Cheap check that a file is plausibly the same one: same length, same
    /// head and tail.
    ///
    /// **This is not verification.** It catches a missing file, a different
    /// file and a truncated file. It cannot catch interior corruption. Ingest
    /// uses it to catalogue quickly; a copy is not `verified` until
    /// [`ContentHash::verify`] has passed against a full hash.
    pub fn matches_quickly(&self, path: &Path) -> Result<()> {
        let actual = if self.is_full() {
            ContentHash::full(path)?
        } else {
            ContentHash::fast(path)?
        };
        self.compare(path, actual)
    }

    fn compare(&self, path: &Path, actual: ContentHash) -> Result<()> {
        if actual.hex() == self.hex() && actual.size() == self.size() {
            Ok(())
        } else {
            Err(Error::HashMismatch {
                path: path.display().to_string(),
                expected: self.hex().to_string(),
                actual: actual.hex().to_string(),
            })
        }
    }
}

/// A camera card, reel or roll: the physical thing media came off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicalSource {
    /// Reel or roll name as the camera recorded it, e.g. `A001`.
    pub reel: String,
    /// Camera identifier, e.g. `A`.
    pub camera: Option<String>,
    /// Shooting day, if known.
    pub shoot_day: Option<u32>,
}

impl PhysicalSource {
    /// Deterministic id, so re-ingesting the same card resolves to one entity.
    pub fn entity_id(&self) -> EntityId {
        EntityId::from_key(&format!("media:reel:{}", self.reel))
    }
}

/// What kind of media a file holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerKind {
    /// ISO base media: MP4, M4A, MOV.
    IsoBmff,
    Matroska,
    /// RIFF WAVE.
    Wave,
    /// A single still image, which may be one entry of a sequence.
    Image,
    /// Recognised but not supported for decode by this build. Carries the
    /// detected name so the failure is diagnosable rather than anonymous.
    Unsupported(String),
}

/// One elementary stream inside a container.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    /// Index within the container, as the demuxer reports it.
    pub index: u32,
    pub kind: StreamKind,
    /// Codec as a stable lowercase token, e.g. `h264`, `aac`, `pcm_s16le`.
    pub codec: String,
    /// The stream's own time base. Never normalised away on ingest.
    pub time_base: Rate,
    /// Duration in `time_base` ticks, when the container states it.
    pub duration_ticks: Option<i64>,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
    /// Whether this build can actually decode this stream. Recorded explicitly
    /// so nothing downstream has to guess, and so a UI can grey out honestly.
    pub decodable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamKind {
    Video,
    Audio,
    Timecode,
    Subtitle,
    Data,
}

impl StreamKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamKind::Video => "video",
            StreamKind::Audio => "audio",
            StreamKind::Timecode => "timecode",
            StreamKind::Subtitle => "subtitle",
            StreamKind::Data => "data",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub width: u32,
    pub height: u32,
    /// Nominal rate. `None` when the container does not state one, or when the
    /// stream is genuinely variable-frame-rate.
    pub frame_rate: Option<Rate>,
    /// Number of coded frames, when the container's sample table gives it.
    pub frame_count: Option<u64>,
    /// True when frame durations are not constant. Editorial must conform this.
    pub variable_frame_rate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub sample_rate: u32,
    pub channels: u16,
    /// Bits per sample where the codec defines one.
    pub bits_per_sample: Option<u16>,
}

/// A media file: the unit of identity, and the thing a clip ultimately resolves
/// to. Immutable — CINE-OMNI never writes to source media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaFile {
    /// Where it was last seen. May be stale; identity does not depend on it.
    pub path: PathBuf,
    pub hash: ContentHash,
    pub container: ContainerKind,
    pub streams: Vec<StreamInfo>,
    /// Start timecode, when the container carries one.
    pub start_timecode: Option<Timecode>,
    /// The card or reel this came from, when ingest knew it.
    pub source: Option<PhysicalSource>,
}

impl MediaFile {
    /// Deterministic entity id, derived from **content**, not path.
    ///
    /// Two copies of the same file in different folders are one media identity.
    /// A re-export with different bytes is a different one. This is what makes
    /// relinking and duplicate detection work.
    pub fn entity_id(&self) -> EntityId {
        EntityId::from_key(&format!("media:file:{}", self.hash.hex()))
    }

    /// Deterministic id for one stream within this file.
    pub fn stream_entity_id(&self, index: u32) -> EntityId {
        EntityId::derived(self.entity_id(), &format!("stream:{index}"))
    }

    pub fn video_streams(&self) -> impl Iterator<Item = &StreamInfo> {
        self.streams.iter().filter(|s| s.kind == StreamKind::Video)
    }
    pub fn audio_streams(&self) -> impl Iterator<Item = &StreamInfo> {
        self.streams.iter().filter(|s| s.kind == StreamKind::Audio)
    }
    pub fn first_video(&self) -> Option<&StreamInfo> {
        self.video_streams().next()
    }
    pub fn first_audio(&self) -> Option<&StreamInfo> {
        self.audio_streams().next()
    }

    /// Whether every stream in this file can be decoded by this build.
    pub fn fully_decodable(&self) -> bool {
        self.streams
            .iter()
            .filter(|s| matches!(s.kind, StreamKind::Video | StreamKind::Audio))
            .all(|s| s.decodable)
    }
}

/// What a derived artefact was made for. Derivatives never replace the source;
/// they reference it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DerivativeKind {
    /// Lower-resolution stand-in for editorial performance.
    Proxy,
    /// Same resolution, edit-friendly codec (the "optimised media" idea).
    Optimised,
    /// A single still used for browsing.
    Thumbnail,
    /// Precomputed audio waveform for timeline drawing.
    Waveform,
    /// A rendered output, e.g. a conformed master.
    Render,
}

impl DerivativeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DerivativeKind::Proxy => "proxy",
            DerivativeKind::Optimised => "optimised",
            DerivativeKind::Thumbnail => "thumbnail",
            DerivativeKind::Waveform => "waveform",
            DerivativeKind::Render => "render",
        }
    }
}

/// Something generated from a `MediaFile`, always traceable back to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Derivative {
    pub kind: DerivativeKind,
    /// Content hash of the media this was made from.
    pub source_hash: String,
    pub path: PathBuf,
    /// Free-form recipe describing how it was produced, so it can be remade and
    /// so a stale derivative can be detected when the recipe changes.
    pub recipe: String,
}

impl Derivative {
    pub fn entity_id(&self) -> EntityId {
        EntityId::from_key(&format!(
            "media:derivative:{}:{}:{}",
            self.kind.as_str(),
            self.source_hash,
            self.recipe
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_content_derived_not_path_derived() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A001C001.mov");
        let b = dir.path().join("elsewhere/A001C001.mov");
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::write(&a, b"identical camera media bytes").unwrap();
        std::fs::write(&b, b"identical camera media bytes").unwrap();

        let ha = ContentHash::fast(&a).unwrap();
        let hb = ContentHash::fast(&b).unwrap();
        assert_eq!(
            ha, hb,
            "the same bytes in two places are one media identity"
        );

        let different = dir.path().join("other.mov");
        std::fs::write(&different, b"different camera media bytes!").unwrap();
        assert_ne!(ContentHash::fast(&different).unwrap(), ha);
    }

    #[test]
    fn verification_detects_modified_media() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("take.wav");
        std::fs::write(&p, b"original take").unwrap();
        let h = ContentHash::full(&p).unwrap();
        h.verify(&p).unwrap();

        std::fs::write(&p, b"tampered take").unwrap();
        assert!(matches!(h.verify(&p), Err(Error::HashMismatch { .. })));
    }

    /// The defect this guards: a fast hash cannot see a flipped bit in the
    /// middle of a file, so reporting "verified" from one would let corrupt
    /// media pass an offload check. It must refuse instead.
    #[test]
    fn a_fast_hash_refuses_to_verify_rather_than_reporting_false_success() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("A001C001.mov");

        // Large enough that head+tail sampling leaves an unexamined interior.
        let mut data = vec![0u8; 4 * 1024 * 1024];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        std::fs::write(&p, &data).unwrap();

        let fast = ContentHash::fast(&p).unwrap();
        let full = ContentHash::full(&p).unwrap();

        // Flip one bit in the middle — the classic silent-corruption case.
        data[2 * 1024 * 1024] ^= 0x01;
        std::fs::write(&p, &data).unwrap();

        // The full hash catches it.
        assert!(
            matches!(full.verify(&p), Err(Error::HashMismatch { .. })),
            "a full hash must detect interior corruption"
        );

        // The fast hash cannot, and must say so rather than pass.
        assert!(
            matches!(fast.verify(&p), Err(Error::CannotVerify { .. })),
            "a fast hash must refuse to verify, never report success it cannot support"
        );

        // The quick check is still available, still honest about what it is,
        // and still blind to this corruption — which is exactly why it is not
        // called verification.
        assert!(
            fast.matches_quickly(&p).is_ok(),
            "the quick check compares head, tail and length, which are unchanged"
        );
    }

    /// The quick check must still catch the things it does cover.
    #[test]
    fn the_quick_check_catches_replacement_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("A001C002.mov");
        std::fs::write(&p, vec![7u8; 1024 * 1024]).unwrap();
        let fast = ContentHash::fast(&p).unwrap();
        assert!(fast.matches_quickly(&p).is_ok());

        // Truncated: the length is part of the digest.
        std::fs::write(&p, vec![7u8; 512 * 1024]).unwrap();
        assert!(matches!(
            fast.matches_quickly(&p),
            Err(Error::HashMismatch { .. })
        ));

        // A different file of the same length: head and tail differ.
        std::fs::write(&p, vec![9u8; 1024 * 1024]).unwrap();
        assert!(matches!(
            fast.matches_quickly(&p),
            Err(Error::HashMismatch { .. })
        ));
    }

    #[test]
    fn fast_and_full_agree_for_small_files_but_are_distinct_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("small.wav");
        std::fs::write(&p, b"short").unwrap();
        let fast = ContentHash::fast(&p).unwrap();
        let full = ContentHash::full(&p).unwrap();
        assert!(!fast.is_full() && full.is_full());
        assert_eq!(fast.size(), full.size());
    }
}
