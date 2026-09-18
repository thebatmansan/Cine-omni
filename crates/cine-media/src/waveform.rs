//! Waveform overviews: what a recording looks like, at every zoom.
//!
//! A timeline draws sound as an envelope, and the envelope must be the sound's
//! own — decoded from the file, not synthesised from anything. Decoding an hour
//! of audio every time a clip is drawn is not possible, so the envelope is
//! computed once and kept as peaks.
//!
//! ## What is stored
//!
//! For each channel and each level, the smallest and largest sample in every
//! bucket of `bucket` samples. Minimum *and* maximum, not one magnitude: a
//! waveform that folds the negative half onto the positive one hides asymmetry,
//! DC offset and clipping on one side, which are exactly what someone looks at
//! a waveform to see.
//!
//! Levels are powers of eight apart ([`LEVELS`]), so any zoom finds a level
//! within a factor of eight of what it wants and the whole pyramid costs about
//! 14% more than its base. Measured: four seconds of 48 kHz stereo is 6.9 kB of
//! peaks, so an hour is about 6 MB — against 691 MB of 24-bit audio.
//!
//! ## Exact alignment
//!
//! Bucket `k` of a level covers samples `[k·bucket, (k+1)·bucket)` counted from
//! the file's first sample, at the file's own sample rate, which is recorded.
//! Nothing here converts rates or resamples: a caller that knows a clip's media
//! offset in exact time converts it to a sample index once and reads the
//! buckets it wants.
//!
//! ## Memory
//!
//! The source is decoded frame by frame and folded into the peaks as it
//! arrives; the samples are never all resident. Only the peaks are, and they
//! are what is written.

use crate::error::{Error, Result};
use parking_lot::Mutex;
use ucg_core::EntityId;
use crate::reader::MediaReader;
use std::io::Read;
use std::path::Path;

/// Bucket sizes, in samples, smallest first. Each is eight times the last.
pub const LEVELS: &[u32] = &[256, 2_048, 16_384, 131_072];

/// Bumped when the format or the levels change; an overview made by another
/// version is stale rather than misread.
pub const FORMAT_VERSION: u32 = 1;

const MAGIC: &[u8; 8] = b"CINEWAV1";

/// One zoom level: the peaks of every bucket, per channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub bucket: u32,
    /// `[bucket][channel]` as `(min, max)`, flattened bucket-major.
    pub peaks: Vec<(i16, i16)>,
}

impl Level {
    pub fn buckets(&self, channels: usize) -> usize {
        self.peaks.len().checked_div(channels).unwrap_or(0)
    }
}

/// A recording's envelope at every level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waveform {
    pub channels: u16,
    /// The file's own sample rate. Buckets are counted in these samples.
    pub sample_rate: u32,
    /// Total sample frames decoded.
    pub frames: u64,
    pub levels: Vec<Level>,
}

/// What a caller asked to draw, and what it got.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// The level's bucket size, in source samples.
    pub bucket: u32,
    /// The first bucket's first sample.
    pub first_sample: i64,
    /// `[bucket][channel]` as `(min, max)`, flattened bucket-major.
    pub peaks: Vec<(i16, i16)>,
    pub channels: u16,
}

impl Waveform {
    /// The peaks covering `[from, to)` samples, at the finest level that needs
    /// no more than `want` buckets to cover it.
    ///
    /// Asking for a window past the end returns the part that exists; asking
    /// for one before the start is clamped to zero. A caller drawing a clip
    /// therefore never has to guess how long the file is.
    pub fn window(&self, from: i64, to: i64, want: usize) -> Window {
        let channels = self.channels.max(1) as usize;
        let span = (to - from).max(0) as u64;
        let level = self
            .levels
            .iter()
            .find(|l| want > 0 && span.div_ceil(u64::from(l.bucket)) <= want as u64)
            .or_else(|| self.levels.last())
            .cloned()
            .unwrap_or(Level {
                bucket: LEVELS[0],
                peaks: Vec::new(),
            });
        let bucket = i64::from(level.bucket);
        let first = (from.max(0) / bucket).max(0);
        let last = ((to.max(0) + bucket - 1) / bucket).max(first);
        let available = level.buckets(channels) as i64;
        let last = last.min(available);
        let mut peaks = Vec::new();
        for b in first..last {
            let base = b as usize * channels;
            for c in 0..channels {
                peaks.push(level.peaks[base + c]);
            }
        }
        Window {
            bucket: level.bucket,
            first_sample: first * bucket,
            peaks,
            channels: self.channels,
        }
    }

    /// Bytes of peaks held, for storage accounting.
    pub fn byte_size(&self) -> usize {
        self.levels.iter().map(|l| l.peaks.len() * 4).sum()
    }
}

/// Decode `path`'s sound and fold it into an overview.
///
/// `progress` is called with how far through the file the decode is, and
/// returns `false` to stop; stopping answers `None` rather than half an
/// overview.
pub fn compute(
    path: &Path,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Option<Waveform>> {
    let mut reader = MediaReader::open_path(path)?;
    let stream = reader
        .info()
        .first_audio()
        .cloned()
        .ok_or(Error::NoSuchStream(0))?;
    let audio = stream
        .audio
        .ok_or_else(|| Error::Container(format!("{}: the sound declares no format", path.display())))?;
    let channels = audio.channels.max(1);
    let sample_rate = audio.sample_rate.max(1);
    let total = stream
        .duration_ticks
        .map(|t| t.max(0) as u64)
        .filter(|t| *t > 0);

    let mut levels: Vec<Level> = LEVELS
        .iter()
        .map(|b| Level {
            bucket: *b,
            peaks: Vec::new(),
        })
        .collect();
    // The running bucket of the base level, per channel.
    let mut current: Vec<(i16, i16)> = vec![(i16::MAX, i16::MIN); channels as usize];
    let mut in_bucket = 0u32;
    let mut frames = 0u64;
    let mut checked = 0u64;

    let flush = |levels: &mut Vec<Level>, current: &mut Vec<(i16, i16)>| {
        levels[0].peaks.extend(current.iter().copied());
        for p in current.iter_mut() {
            *p = (i16::MAX, i16::MIN);
        }
    };

    while let Some(frame) = reader.next_audio_frame()? {
        let n = frame.frame_count();
        for i in 0..n {
            for (c, peak) in current.iter_mut().enumerate() {
                let v = frame
                    .channels
                    .get(c)
                    .or_else(|| frame.channels.first())
                    .map(|ch| ch[i])
                    .unwrap_or(0.0);
                // 16-bit peaks: a waveform is drawn a few hundred pixels tall,
                // and this is four bytes per bucket per channel rather than
                // eight.
                let s = (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                peak.0 = peak.0.min(s);
                peak.1 = peak.1.max(s);
            }
            in_bucket += 1;
            frames += 1;
            if in_bucket == LEVELS[0] {
                flush(&mut levels, &mut current);
                in_bucket = 0;
            }
        }
        checked += n as u64;
        if checked >= 48_000 {
            checked = 0;
            let done = match total {
                Some(t) => (frames as f32 / t as f32).clamp(0.0, 1.0),
                None => 0.0,
            };
            if !progress(done) {
                return Ok(None);
            }
        }
    }
    if in_bucket > 0 {
        flush(&mut levels, &mut current);
    }

    // Coarser levels are folded from the level below, which is exact: the
    // extremes of eight buckets are the extremes of their union.
    for i in 1..levels.len() {
        let ratio = (levels[i].bucket / levels[i - 1].bucket) as usize;
        let below = std::mem::take(&mut levels[i - 1].peaks);
        let channels = channels as usize;
        let buckets = below.len() / channels;
        let mut folded = Vec::with_capacity(buckets.div_ceil(ratio) * channels);
        let mut b = 0;
        while b < buckets {
            for c in 0..channels {
                let mut peak = (i16::MAX, i16::MIN);
                for k in 0..ratio {
                    if let Some(p) = below.get((b + k) * channels + c) {
                        peak.0 = peak.0.min(p.0);
                        peak.1 = peak.1.max(p.1);
                    }
                }
                folded.push(peak);
            }
            b += ratio;
        }
        levels[i].peaks = folded;
        levels[i - 1].peaks = below;
    }

    Ok(Some(Waveform {
        channels,
        sample_rate,
        frames,
        levels,
    }))
}

/// Write an overview: a small header, then each level's peaks.
pub fn write(path: &Path, w: &Waveform) -> Result<()> {
    let mut out = Vec::with_capacity(w.byte_size() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&u32::from(w.channels).to_le_bytes());
    out.extend_from_slice(&w.sample_rate.to_le_bytes());
    out.extend_from_slice(&w.frames.to_le_bytes());
    out.extend_from_slice(&(w.levels.len() as u32).to_le_bytes());
    for level in &w.levels {
        out.extend_from_slice(&level.bucket.to_le_bytes());
        out.extend_from_slice(&(level.peaks.len() as u64).to_le_bytes());
    }
    for level in &w.levels {
        for (min, max) in &level.peaks {
            out.extend_from_slice(&min.to_le_bytes());
            out.extend_from_slice(&max.to_le_bytes());
        }
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Read an overview written by [`write`].
pub fn read(path: &Path) -> Result<Waveform> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| Error::NotFound(path.display().to_string()))?
        .read_to_end(&mut bytes)?;
    let bad = |why: &str| Error::Container(format!("{}: {why}", path.display()));
    if bytes.len() < 32 || &bytes[..8] != MAGIC {
        return Err(bad("not a waveform overview"));
    }
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
    let version = u32_at(8);
    if version != FORMAT_VERSION {
        return Err(bad(&format!(
            "written by format {version}, this build reads {FORMAT_VERSION}"
        )));
    }
    let channels = u32_at(12) as u16;
    let sample_rate = u32_at(16);
    let frames = u64_at(20);
    let level_count = u32_at(28) as usize;
    let mut at = 32;
    let mut shape = Vec::with_capacity(level_count);
    for _ in 0..level_count {
        if at + 12 > bytes.len() {
            return Err(bad("truncated header"));
        }
        shape.push((u32_at(at), u64_at(at + 4) as usize));
        at += 12;
    }
    let mut levels = Vec::with_capacity(level_count);
    for (bucket, count) in shape {
        let need = count * 4;
        if at + need > bytes.len() {
            return Err(bad("truncated peaks"));
        }
        let mut peaks = Vec::with_capacity(count);
        for i in 0..count {
            let o = at + i * 4;
            peaks.push((
                i16::from_le_bytes([bytes[o], bytes[o + 1]]),
                i16::from_le_bytes([bytes[o + 2], bytes[o + 3]]),
            ));
        }
        at += need;
        levels.push(Level { bucket, peaks });
    }
    Ok(Waveform {
        channels,
        sample_rate,
        frames,
        levels,
    })
}

/// Overviews already read, so scrolling a timeline does not read a file per
/// clip per frame. Small and bounded: two recordings' peaks.
#[derive(Default, Debug)]
pub struct Cache {
    held: Mutex<Vec<(EntityId, std::sync::Arc<Waveform>)>>,
}

const KEEP: usize = 2;

impl Cache {
    pub fn new() -> Cache {
        Cache::default()
    }

    /// The overview of one derived node, read once and kept.
    pub fn get(&self, id: EntityId, path: &Path) -> Result<std::sync::Arc<Waveform>> {
        if let Some((_, w)) = self.held.lock().iter().find(|(k, _)| *k == id) {
            return Ok(w.clone());
        }
        let w = std::sync::Arc::new(read(path)?);
        let mut held = self.held.lock();
        held.retain(|(k, _)| *k != id);
        held.push((id, w.clone()));
        while held.len() > KEEP {
            held.remove(0);
        }
        Ok(w)
    }

    /// Forget one overview, when it has been made again.
    pub fn forget(&self, id: EntityId) {
        self.held.lock().retain(|(k, _)| *k != id);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn wave(frames: usize, channels: usize, f: impl Fn(usize, usize) -> f32) -> Waveform {
        let mut levels: Vec<Level> = LEVELS
            .iter()
            .map(|b| Level {
                bucket: *b,
                peaks: Vec::new(),
            })
            .collect();
        for b in 0..frames.div_ceil(LEVELS[0] as usize) {
            for c in 0..channels {
                let mut peak = (i16::MAX, i16::MIN);
                for i in 0..LEVELS[0] as usize {
                    let n = b * LEVELS[0] as usize + i;
                    if n < frames {
                        let s = (f(n, c).clamp(-1.0, 1.0) * 32767.0).round() as i16;
                        peak.0 = peak.0.min(s);
                        peak.1 = peak.1.max(s);
                    }
                }
                levels[0].peaks.push(peak);
            }
        }
        Waveform {
            channels: channels as u16,
            sample_rate: 48_000,
            frames: frames as u64,
            levels,
        }
    }

    #[test]
    fn a_window_takes_the_finest_level_that_fits_and_lands_on_bucket_edges() {
        let w = wave(48_000 * 4, 2, |n, _| (n as f32 / 100.0).sin());
        // A wide window over four seconds cannot be drawn at 256 samples a
        // bucket in 200 pixels; a coarser level is chosen.
        let wide = w.window(0, 48_000 * 4, 200);
        assert!(wide.bucket > LEVELS[0], "a coarse level for a wide view: {}", wide.bucket);
        assert!(wide.peaks.len() / 2 <= 200 + 1, "and it fits the request");

        // A narrow window gets the finest level.
        let near = w.window(0, 10_000, 200);
        assert_eq!(near.bucket, LEVELS[0]);
        assert_eq!(near.first_sample, 0);

        // Windows start on a bucket edge, so a caller can place them exactly.
        let offset = w.window(5_000, 15_000, 500);
        assert_eq!(offset.first_sample % i64::from(offset.bucket), 0);
        assert!(offset.first_sample <= 5_000);
    }

    #[test]
    fn a_window_past_the_end_returns_what_exists() {
        let w = wave(10_000, 1, |_, _| 0.5);
        let past = w.window(48_000, 96_000, 100);
        assert!(past.peaks.is_empty(), "nothing exists out there");
        let over = w.window(0, 96_000, 10_000);
        assert!(!over.peaks.is_empty());
        assert!(
            over.peaks.len() <= 10_000 / 256 * 2 + 4,
            "only the buckets that exist: {}",
            over.peaks.len()
        );
    }

    #[test]
    fn peaks_keep_both_halves_of_an_asymmetric_signal() {
        // Positive excursions to 0.9, negative only to -0.1: a waveform that
        // folded the halves together would show a symmetric 0.9 envelope.
        let w = wave(2_048, 1, |n, _| if n % 8 == 0 { 0.9 } else { -0.1 });
        let (min, max) = w.levels[0].peaks[0];
        assert!(max > 29_000, "the positive peak is kept: {max}");
        assert!((-4_000..-2_000).contains(&min), "and so is the negative one: {min}");
    }

    #[test]
    fn an_overview_survives_being_written_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.cinewav");
        let w = wave(50_000, 2, |n, c| ((n + c * 7) as f32 / 50.0).sin());
        write(&path, &w).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back, w);

        // A file that is not an overview says so rather than being misread.
        std::fs::write(&path, b"not a waveform at all").unwrap();
        let err = read(&path).unwrap_err().to_string();
        assert!(err.contains("not a waveform overview"), "{err}");
    }
}
