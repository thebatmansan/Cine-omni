//! A sequence as a frame source: play a cut, not a file.
//!
//! The scheduler plays anything that implements [`FrameSource`]. This one
//! presents the pictures of an edit list — record-time stretches, each showing a
//! stretch of some media file — at the sequence's own frame rate, so the
//! scheduler, the clock, decode-ahead and the presentation path work on a cut
//! exactly as they do on a single file.
//!
//! ## What a picture of the sequence is
//!
//! Display index `i` is record time `i / rate`. The entry covering that time
//! names a file and the media time showing at its start, as an offset from the
//! start of the file's presentation. The picture is whichever picture of the
//! file presents at that media time — [`VideoReader::index_at`], an exact search
//! over the sample table. Nothing is rounded through seconds.
//!
//! A retimed entry carries a [`Timing`] — a constant speed or a ramp, anchored
//! where the clip starts — and the media at an instant comes from it rather
//! than one for one. The arithmetic is [`Timing::media_at`], the same function
//! the editorial model and the
//! browser's record monitor use, so all three name the same picture.
//!
//! ## What is shown where there is no picture
//!
//! A gap in the cut is black: that is what the cut shows. An entry whose media
//! cannot be played (no file, no known range, offline) is also black, and
//! [`EditListSource::why_black`] says why, so the interface can say "media
//! offline" over it rather than the viewer silently showing nothing. A decode
//! that *fails* on media that should play is an error, never black.
//!
//! Black is 4:2:0 8-bit at the size and colour interpretation of the first
//! picture decoded from the list, so the presentation path does not change
//! format mid-sequence. Before any picture has been decoded there is nothing
//! to copy those from, and black is produced at the first playable entry's
//! size by decoding that entry's first picture once.

use crate::error::{Error, Result};
use crate::frame::{AlphaMode, ColorMetadata, PixelFormat, VideoFrame};
use crate::schedule::FrameSource;
use crate::video::VideoReader;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use ucg_core::{Rate, RationalTime, Rounding, TimeRange, Timing};

/// One stretch of the sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct EditEntry {
    /// Where on the sequence it plays.
    pub record: TimeRange,
    /// The file, when there is one to play.
    pub path: Option<PathBuf>,
    /// Media time at `record.start_time`, measured from the file's first
    /// presented picture. `None` when it cannot be known.
    pub offset: Option<RationalTime>,
    /// Why the entry cannot be played, when it cannot.
    pub unplayable: Option<String>,
    /// How the media runs against record time, when the clip is retimed: its
    /// map anchored at the clip's start on the record and the media offset
    /// there (measured like `offset`, from the file's first picture). `None`
    /// is real time.
    pub timing: Option<Timing>,
    /// For display.
    pub name: String,
}

/// Plays an edit list at the sequence rate.
pub struct EditListSource {
    rate: Rate,
    count: usize,
    entries: Vec<EditEntry>,
    readers: HashMap<PathBuf, VideoReader>,
    /// Size, format and colour of black, learned from real pictures.
    black: Option<(u32, u32, ColorMetadata)>,
}

impl EditListSource {
    /// A source of `end` worth of pictures at `rate`.
    pub fn new(rate: Rate, end: RationalTime, entries: Vec<EditEntry>) -> EditListSource {
        let (frames, _) = end.rescale_checked(rate, Rounding::Ceil);
        EditListSource {
            rate,
            count: usize::try_from(frames.ticks().max(0)).unwrap_or(0),
            entries,
            readers: HashMap::new(),
            black: None,
        }
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    pub fn entries(&self) -> &[EditEntry] {
        &self.entries
    }

    fn entry_at(&self, t: RationalTime) -> Option<&EditEntry> {
        self.entries.iter().find(|e| e.record.contains(t))
    }

    /// Why the picture at `index` is black, if it is.
    pub fn why_black(&self, index: usize) -> Option<String> {
        why_black(&self.entries, self.rate, index)
    }

    fn reader(&mut self, path: &PathBuf) -> Result<&mut VideoReader> {
        if !self.readers.contains_key(path) {
            let reader = VideoReader::open_path(path)?;
            self.readers.insert(path.clone(), reader);
        }
        Ok(self.readers.get_mut(path).expect("inserted above"))
    }

    /// Decode the picture of `entry` presenting at record time `t`.
    fn decode_entry(&mut self, entry: &EditEntry, t: RationalTime) -> Result<Option<VideoFrame>> {
        let (Some(path), Some(offset)) = (entry.path.clone(), entry.offset) else {
            return Ok(None);
        };
        let reader = self.reader(&path)?;
        let Some((first, _)) = reader.extent() else {
            return Err(Error::Unsupported(format!(
                "{}: no pictures",
                path.display()
            )));
        };
        let media_t = first.checked_add(&match &entry.timing {
            // The grid is the media's own frame rate: there is no picture
            // between two pictures, so the offset floors onto one. Anchored at
            // the clip's own start, so a ramp is read from its beginning.
            Some(timing) => timing.media_at(t, first.rate_exact()),
            None => offset.checked_add(&t.checked_sub(&entry.record.start_time)),
        });
        let Some(index) = reader.index_at(media_t) else {
            return Ok(None);
        };
        reader.frame_at_index(index)
    }

    fn black_frame(&mut self, t: RationalTime, index: usize) -> Result<VideoFrame> {
        if self.black.is_none() {
            let first = self
                .entries
                .iter()
                .find(|e| e.unplayable.is_none() && e.path.is_some() && e.offset.is_some())
                .cloned();
            if let Some(e) = first {
                if let Some(f) = self.decode_entry(&e, e.record.start_time)? {
                    self.black = Some((f.width, f.height, f.color));
                }
            }
        }
        let (w, h, color) = self.black.ok_or_else(|| {
            Error::Unsupported("the sequence has no playable picture to take a size from".into())
        })?;
        Ok(black(w, h, color, t, index))
    }
}

/// Why picture `index` of a sequence at `rate` is black, if it is — for callers
/// that hold the entries but not the source (which lives on a decode thread).
pub fn why_black(entries: &[EditEntry], rate: Rate, index: usize) -> Option<String> {
    let t = RationalTime::exact(index as i64, rate);
    match entries.iter().find(|e| e.record.contains(t)) {
        None => Some("nothing on a visible picture track".into()),
        Some(e) => e.unplayable.clone().map(|why| format!("{}: {why}", e.name)),
    }
}

/// A 4:2:0 8-bit black picture: Y at the range's black level, chroma neutral.
pub fn black(
    width: u32,
    height: u32,
    color: ColorMetadata,
    pts: RationalTime,
    index: usize,
) -> VideoFrame {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y_black = if color.full_range == Some(true) {
        0
    } else {
        16
    };
    VideoFrame {
        pts,
        frame_index: Some(index as i64),
        timecode: None,
        width,
        height,
        format: PixelFormat::Yuv420P8,
        color,
        alpha: AlphaMode::None,
        planes: vec![
            (Arc::new(vec![y_black; w * h]), w),
            (Arc::new(vec![128; cw * ch]), cw),
            (Arc::new(vec![128; cw * ch]), cw),
        ],
    }
}

impl FrameSource for EditListSource {
    fn frame_count(&self) -> usize {
        self.count
    }

    fn pts_of(&self, index: usize) -> Option<RationalTime> {
        (index < self.count).then(|| RationalTime::exact(index as i64, self.rate))
    }

    fn index_at(&self, t: RationalTime) -> Option<usize> {
        if t < RationalTime::zero(self.rate) {
            return None;
        }
        let (frames, _) = t.rescale_checked(self.rate, Rounding::Floor);
        let i = usize::try_from(frames.ticks()).ok()?;
        Some(i.min(self.count.saturating_sub(1)))
    }

    fn extent(&self) -> Option<(RationalTime, RationalTime)> {
        (self.count > 0).then(|| {
            (
                RationalTime::zero(self.rate),
                RationalTime::exact(self.count as i64, self.rate),
            )
        })
    }

    fn decode_index(&mut self, index: usize) -> Result<Option<VideoFrame>> {
        if index >= self.count {
            return Ok(None);
        }
        let t = RationalTime::exact(index as i64, self.rate);
        let entry = self.entry_at(t).cloned();
        let picture = match &entry {
            Some(e) if e.unplayable.is_none() => self.decode_entry(e, t)?,
            _ => None,
        };
        let mut frame = match picture {
            Some(f) => {
                if self.black.is_none() {
                    self.black = Some((f.width, f.height, f.color));
                }
                f
            }
            None => self.black_frame(t, index)?,
        };
        // The picture is presented at sequence time, as picture `index` of the
        // sequence; where it came from is the edit list's business.
        frame.pts = t;
        frame.frame_index = Some(index as i64);
        Ok(Some(frame))
    }
}
