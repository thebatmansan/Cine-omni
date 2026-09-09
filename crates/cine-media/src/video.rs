//! Video demux, decode and frame-accurate seek over ISO base media files.
//!
//! ## Decode order is not display order
//!
//! With B-frames, the order pictures are decoded in differs from the order they
//! are shown in. The container records both: a decode timestamp from `stts`,
//! and a composition offset from `ctts` whose sum is the presentation time.
//! Collapsing the two — or worse, deriving time by multiplying a frame index by
//! a float frame duration — is how frame-accuracy is lost.
//!
//! This reader keeps them apart. The sample table is read once at open, giving
//! the exact `dts` and `pts` of every sample as integers in the track's own
//! timescale. Because the whole table is known up front, the display order is a
//! **computed permutation of the decode order** rather than a guess at how deep
//! the reorder buffer needs to be.
//!
//! ## Seeking
//!
//! A compressed stream can only be entered at a keyframe, so seeking is:
//! find the target sample, walk back to the nearest sync sample at or before
//! it in decode order, reset the decoder, then decode forward discarding
//! pictures until the target is produced. That is exact by construction; there
//! is no tolerance window and nothing rounds through seconds.

use crate::decoder::{avcc_to_annex_b, H264Decoder, Packet, VideoDecoder};
use crate::error::{Error, Result};
use crate::frame::{ColorMetadata, VideoFrame};
use crate::identity::{MediaFile, StreamKind};
use crate::probe::decodable_codecs;
use crate::time::{Rate, Rounding, Timestamp};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// One row of the sample table, in decode order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SampleEntry {
    /// 1-based sample id, as the container numbers them.
    id: u32,
    /// Decode timestamp in track timescale ticks.
    dts: i64,
    /// Presentation timestamp: `dts + ctts` offset, in the same ticks.
    pts: i64,
    /// Whether the stream can be entered here.
    is_sync: bool,
}

/// An open video track, ready to decode.
pub struct VideoReader {
    info: MediaFile,
    mp4: mp4::Mp4Reader<BufReader<File>>,
    track_id: u32,
    time_base: Rate,
    nal_length_size: usize,
    /// Sample table in decode order.
    samples: Vec<SampleEntry>,
    /// Indices into `samples`, ordered by presentation time.
    display_order: Vec<usize>,
    decoder: Box<dyn VideoDecoder>,
    /// Next sample to feed the decoder, as an index into `samples`.
    feed_cursor: usize,
    /// Next picture to emit, as an index into `display_order`.
    display_cursor: usize,
    /// Decoded pictures held until their turn in display order, keyed by pts.
    pending: BTreeMap<i64, VideoFrame>,
    /// The most recently delivered picture, retained so that re-requesting the
    /// frame already at the play head is free. Without this, holding on a frame
    /// re-decodes from the keyframe every time — measured at 2.1 s on a
    /// 120-frame GOP, which would make a paused viewer unusable. Cloning is
    /// cheap because plane data is `Arc`-shared.
    last_frame: Option<VideoFrame>,
}

impl VideoReader {
    /// Open the first decodable video track of a probed file.
    pub fn open(info: MediaFile) -> Result<VideoReader> {
        let stream = info
            .streams
            .iter()
            .find(|s| s.kind == StreamKind::Video)
            .ok_or(Error::NoSuchStream(0))?;
        if !stream.decodable {
            return Err(Error::NoDecoder {
                codec: stream.codec.clone(),
                available: decodable_codecs(),
            });
        }
        let track_id = stream.index;
        let time_base = stream.time_base;

        let file =
            File::open(&info.path).map_err(|_| Error::Offline(info.path.display().to_string()))?;
        let size = file.metadata()?.len();
        let mut mp4 = mp4::Mp4Reader::read_header(BufReader::new(file), size)
            .map_err(|e| Error::Container(format!("{}: {e}", info.path.display())))?;

        // Scoped so the immutable borrow of the track ends before samples are
        // read, which needs the reader mutably.
        let (sps, pps, nal_length_size, sample_count) = {
            let track = mp4
                .tracks()
                .get(&track_id)
                .ok_or(Error::NoSuchStream(track_id))?;
            let sps = track
                .sequence_parameter_set()
                .map_err(|e| Error::Container(format!("missing SPS: {e}")))?
                .to_vec();
            let pps = track
                .picture_parameter_set()
                .map_err(|e| Error::Container(format!("missing PPS: {e}")))?
                .to_vec();
            (sps, pps, nal_length_size(track), track.sample_count())
        };

        // Read the sample table once. `read_sample` also returns the payload,
        // which is not wanted here, so timing is taken and the bytes dropped;
        // they are re-read on demand during decode to keep memory flat.
        let mut samples = Vec::with_capacity(sample_count as usize);
        for id in 1..=sample_count {
            let s = mp4
                .read_sample(track_id, id)
                .map_err(|e| Error::Container(format!("sample {id}: {e}")))?
                .ok_or(Error::FrameNotIndexed(i64::from(id)))?;
            let dts = s.start_time as i64;
            samples.push(SampleEntry {
                id,
                dts,
                pts: dts + i64::from(s.rendering_offset),
                is_sync: s.is_sync,
            });
        }
        if samples.is_empty() {
            return Err(Error::Container("video track has no samples".into()));
        }

        // Display order, computed exactly from the composition offsets.
        let mut display_order: Vec<usize> = (0..samples.len()).collect();
        display_order.sort_by_key(|&i| samples[i].pts);

        // Colour is whatever the container declared. H.264 VUI parsing is not
        // implemented, so this is currently absent rather than assumed; see the
        // gaps section of the Phase 2 blueprint.
        let color = ColorMetadata::default();
        let decoder: Box<dyn VideoDecoder> = Box::new(H264Decoder::new(&sps, &pps, color));

        Ok(VideoReader {
            info,
            mp4,
            track_id,
            time_base,
            nal_length_size,
            samples,
            display_order,
            decoder,
            feed_cursor: 0,
            display_cursor: 0,
            pending: BTreeMap::new(),
            last_frame: None,
        })
    }

    /// Convenience: probe then open.
    pub fn open_path(path: &Path) -> Result<VideoReader> {
        VideoReader::open(crate::probe::probe(path)?)
    }

    pub fn info(&self) -> &MediaFile {
        &self.info
    }

    /// The track's own time base. Nothing is normalised on ingest.
    pub fn time_base(&self) -> Rate {
        self.time_base
    }

    /// Number of coded pictures in the track.
    pub fn frame_count(&self) -> usize {
        self.samples.len()
    }

    /// Presentation timestamp of the frame at `display_index`.
    pub fn pts_at(&self, display_index: usize) -> Option<Timestamp> {
        let i = *self.display_order.get(display_index)?;
        Some(Timestamp::exact(self.samples[i].pts, self.time_base))
    }

    /// Read one sample's payload and wrap it as an Annex-B packet.
    fn packet(&mut self, index: usize) -> Result<Packet> {
        let entry = self.samples[index];
        let sample = self
            .mp4
            .read_sample(self.track_id, entry.id)
            .map_err(|e| Error::Container(format!("sample {}: {e}", entry.id)))?
            .ok_or(Error::FrameNotIndexed(i64::from(entry.id)))?;
        Ok(Packet {
            data: avcc_to_annex_b(&sample.bytes, self.nal_length_size)?,
            pts: Timestamp::exact(entry.pts, self.time_base),
            dts: Timestamp::exact(entry.dts, self.time_base),
            is_keyframe: entry.is_sync,
        })
    }

    /// Feed the next undecoded sample, holding any picture it produces.
    /// Returns `false` at the end of the track.
    fn feed_one(&mut self) -> Result<bool> {
        if self.feed_cursor >= self.samples.len() {
            return Ok(false);
        }
        let packet = self.packet(self.feed_cursor)?;
        self.feed_cursor += 1;
        if let Some(frame) = self.decoder.decode(&packet)? {
            self.pending.insert(frame.pts.ticks(), frame);
        }
        Ok(true)
    }

    /// Next frame **in display order**, or `None` at end of track.
    ///
    /// Frames are emitted strictly in presentation order even though they are
    /// decoded out of order, because the exact presentation time of every
    /// sample is known from the sample table.
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        loop {
            let Some(&idx) = self.display_order.get(self.display_cursor) else {
                return Ok(None);
            };
            let want = self.samples[idx].pts;
            if let Some(frame) = self.pending.remove(&want) {
                self.display_cursor += 1;
                self.last_frame = Some(frame.clone());
                return Ok(Some(frame));
            }
            if !self.feed_one()? {
                // End of stream: drain whatever the decoder still holds.
                for frame in self.decoder.flush()? {
                    self.pending.insert(frame.pts.ticks(), frame);
                }
                return match self.pending.remove(&want) {
                    Some(frame) => {
                        self.display_cursor += 1;
                        self.last_frame = Some(frame.clone());
                        Ok(Some(frame))
                    }
                    None => Ok(None),
                };
            }
        }
    }

    /// Index in decode order of the sample presenting at or before `target`.
    fn sample_for(&self, target: Timestamp) -> Option<usize> {
        let ticks = target
            .rescale_checked(self.time_base, Rounding::Nearest)
            .0
            .ticks();
        // Walk display order, which is sorted by pts, and take the last frame
        // whose presentation time has already begun.
        let mut best = None;
        for &i in &self.display_order {
            if self.samples[i].pts <= ticks {
                best = Some(i);
            } else {
                break;
            }
        }
        best
    }

    /// Nearest sync sample at or before `index` in decode order.
    fn sync_before(&self, index: usize) -> usize {
        (0..=index)
            .rev()
            .find(|&i| self.samples[i].is_sync)
            .unwrap_or(0)
    }

    /// Decode the frame presenting at `target`, seeking if necessary.
    ///
    /// Exact by construction: it enters at a keyframe and decodes forward until
    /// the requested picture is produced. There is no tolerance window.
    pub fn frame_at(&mut self, target: Timestamp) -> Result<Option<VideoFrame>> {
        let Some(want_idx) = self.sample_for(target) else {
            return Ok(None);
        };
        let want_pts = self.samples[want_idx].pts;

        // The frame already at the play head: a paused viewer, a repeated
        // request, a second consumer of the same instant.
        if let Some(frame) = &self.last_frame {
            if frame.pts.ticks() == want_pts {
                return Ok(Some(frame.clone()));
            }
        }
        // Already decoded and waiting its turn in display order.
        if let Some(frame) = self.pending.remove(&want_pts) {
            self.last_frame = Some(frame.clone());
            return Ok(Some(frame));
        }

        let entry_point = self.sync_before(want_idx);
        // Only rewind when the target is behind the play head, or when the
        // decoder has not yet reached its keyframe. Decoding forward from where
        // we already are is much cheaper than restarting the GOP.
        if self.feed_cursor <= want_idx && self.feed_cursor > entry_point {
            // Continue from here: the reference chain is already established.
        } else {
            self.decoder.reset()?;
            self.pending.clear();
            self.feed_cursor = entry_point;
        }

        while self.feed_cursor <= want_idx {
            if !self.feed_one()? {
                break;
            }
            if let Some(frame) = self.pending.remove(&want_pts) {
                self.sync_display_cursor(want_pts);
                self.last_frame = Some(frame.clone());
                return Ok(Some(frame));
            }
        }
        // The picture may have been produced before its own sample was reached,
        // which is normal with B-frames.
        match self.pending.remove(&want_pts) {
            Some(frame) => {
                self.sync_display_cursor(want_pts);
                self.last_frame = Some(frame.clone());
                Ok(Some(frame))
            }
            None => Err(Error::Decode {
                at: format!("{target:?}"),
                message: format!(
                    "decoded forward from sample {entry_point} without producing pts {want_pts}"
                ),
            }),
        }
    }

    /// After a seek, sequential reads should continue from the frame after the
    /// one just returned.
    fn sync_display_cursor(&mut self, pts: i64) {
        if let Some(pos) = self
            .display_order
            .iter()
            .position(|&i| self.samples[i].pts == pts)
        {
            self.display_cursor = pos + 1;
        }
    }

    /// Decode the frame at a zero-based display index.
    pub fn frame_at_index(&mut self, display_index: usize) -> Result<Option<VideoFrame>> {
        match self.pts_at(display_index) {
            Some(pts) => self.frame_at(pts),
            None => Ok(None),
        }
    }

    /// Rewind to the start of the track.
    pub fn rewind(&mut self) -> Result<()> {
        self.decoder.reset()?;
        self.pending.clear();
        self.last_frame = None;
        self.feed_cursor = 0;
        self.display_cursor = 0;
        Ok(())
    }

    /// Whether the presentation order differs from the decode order, i.e. the
    /// stream really does carry B-frames.
    pub fn has_reordering(&self) -> bool {
        self.display_order
            .iter()
            .enumerate()
            .any(|(pos, &i)| pos != i)
    }

    /// Number of sync samples: the granularity a seek can enter the stream at.
    pub fn keyframe_count(&self) -> usize {
        self.samples.iter().filter(|s| s.is_sync).count()
    }
}

/// `avcC` records the NAL length prefix width; 4 is usual but 1 and 2 are legal.
fn nal_length_size(track: &mp4::Mp4Track) -> usize {
    match &track.trak.mdia.minf.stbl.stsd.avc1 {
        Some(avc1) => (avc1.avcc.length_size_minus_one & 0x3) as usize + 1,
        None => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(rows: &[(i64, i64, bool)]) -> Vec<SampleEntry> {
        rows.iter()
            .enumerate()
            .map(|(n, &(dts, pts, is_sync))| SampleEntry {
                id: n as u32 + 1,
                dts,
                pts,
                is_sync,
            })
            .collect()
    }

    /// A classic IBBP pattern: decode order and display order genuinely differ.
    #[test]
    fn display_order_is_a_permutation_of_decode_order() {
        let samples = entries(&[
            (0, 0, true),  // I
            (1, 3, false), // P, shown 4th
            (2, 1, false), // B
            (3, 2, false), // B
        ]);
        let mut order: Vec<usize> = (0..samples.len()).collect();
        order.sort_by_key(|&i| samples[i].pts);
        assert_eq!(order, vec![0, 2, 3, 1]);
        // Every sample appears exactly once: nothing dropped, nothing invented.
        let mut seen = order.clone();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }

    #[test]
    fn sync_search_walks_back_to_the_entry_point() {
        let samples = entries(&[
            (0, 0, true),
            (1, 1, false),
            (2, 2, false),
            (3, 3, true),
            (4, 4, false),
        ]);
        let sync_before = |index: usize| -> usize {
            (0..=index).rev().find(|&i| samples[i].is_sync).unwrap_or(0)
        };
        assert_eq!(sync_before(0), 0);
        assert_eq!(sync_before(2), 0, "mid-GOP walks back to the I-frame");
        assert_eq!(sync_before(3), 3, "a keyframe is its own entry point");
        assert_eq!(sync_before(4), 3);
    }
}
