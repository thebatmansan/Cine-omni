//! A sequence's sound as an audio source: decode each clip's audio and mix.
//!
//! [`crate::av::AudioScheduler`] plays anything that implements
//! [`AudioSource`]. This one produces the sound of a cut at the output rate:
//! every entry — a stretch of record time playing a stretch of some file — is
//! decoded from that file and summed into the output, sample-accurately.
//!
//! ## Where a clip's sound comes from in its file
//!
//! An entry's `offset` is measured from the start of the clip's available range,
//! which for ingested media is the file's first *picture*. Sound and picture
//! share the container's presentation timeline, so the sample heard at the
//! entry's start is at `first picture time + offset` on the audio track. That
//! keeps a take's sound under its picture when both are placed together.
//! Priming samples and MP4 edit lists are not applied by the demuxer here; the
//! offset they would introduce is not measured and not claimed to be zero.
//!
//! ## Speed, and files at other rates
//!
//! A retimed clip, and a file whose sample rate is not the output's, are read
//! the same way: every output sample reads the file at an exact rational
//! position ([`cine_audio::speed::Stepper`]) through a band-limited kernel whose
//! cutoff falls when the file is read faster than one sample per output sample,
//! so nothing aliases. A clip at one-for-one from a file at the output rate —
//! nearly every clip — skips the kernel and copies samples, bit for bit.
//!
//! This is *varispeed*: a clip at twice speed sounds an octave up, as tape
//! does. Backwards reads the file backwards from the end of the clip's range.
//!
//! ## What is refused rather than converted
//!
//! * More than two channels: there is no downmix. Mono is placed in both
//!   channels, which is what mono means on a stereo output.
//!
//! Summing may exceed full scale. It is not limited — a limiter is a mixing
//! decision — and [`SequenceAudio::clipped`] counts the samples that did.

use crate::av::AudioSource;
use crate::error::Result;
use crate::reader::MediaReader;
use crate::video::VideoReader;
use cine_audio::speed::{cutoff_for, KernelTable, Stepper, HALF};
use std::collections::HashMap;
use std::path::PathBuf;
use ucg_core::{Rate, RationalTime, Rounding, Speed, TimeRange};

/// One clip's sound on the sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundEntry {
    pub record: TimeRange,
    pub path: Option<PathBuf>,
    /// Media time heard at `record.start_time`, from the file's first picture.
    /// For a reversed clip, the end of its range (exclusive): it is read
    /// backwards from there.
    pub offset: Option<RationalTime>,
    pub name: String,
    /// How fast the media runs against record time. `None` is real time.
    pub speed: Option<Speed>,
}

/// Decoded audio of one file, kept as a sliding window.
struct Track {
    reader: MediaReader,
    /// The file's own sample rate: the window is in file samples.
    rate: Rate,
    /// File sample index of the file's first picture.
    anchor: i64,
    /// Interleaved stereo, starting at file sample `start`.
    window: Vec<f32>,
    start: i64,
}

/// The mixed sound of a sequence at a fixed output rate, stereo.
pub struct SequenceAudio {
    rate: Rate,
    end: i64,
    entries: Vec<SoundEntry>,
    tracks: HashMap<PathBuf, Option<Track>>,
    refusals: HashMap<String, String>,
    clipped: u64,
    /// Kernels by cutoff, built once: a cutoff depends only on how fast a clip
    /// reads its file, and a cut has few distinct speeds.
    kernels: HashMap<u64, KernelTable>,
}

/// Samples kept decoded behind the read position.
const KEEP: i64 = 48_000;
/// Samples decoded and discarded before a seek target. Four AAC frames.
const PREROLL: i64 = 4_096;

impl SequenceAudio {
    /// `end` is the sequence's length; reads past it end the stream.
    pub fn new(rate: Rate, end: RationalTime, entries: Vec<SoundEntry>) -> SequenceAudio {
        let (end, _) = end.rescale_checked(rate, Rounding::Ceil);
        SequenceAudio {
            rate,
            end: end.ticks(),
            entries,
            tracks: HashMap::new(),
            refusals: HashMap::new(),
            clipped: 0,
            kernels: HashMap::new(),
        }
    }

    /// Entries that are silent and why.
    pub fn refusals(&self) -> &HashMap<String, String> {
        &self.refusals
    }

    /// Output samples that summed beyond full scale.
    pub fn clipped(&self) -> u64 {
        self.clipped
    }

    fn sample(&self, t: RationalTime) -> i64 {
        t.rescale_checked(self.rate, Rounding::Floor).0.ticks()
    }

    fn open(&mut self, path: &PathBuf, name: &str) -> Option<&mut Track> {
        if !self.tracks.contains_key(path) {
            let track = match self.open_track(path) {
                Ok(t) => Some(t),
                Err(why) => {
                    self.refusals.insert(name.to_string(), why);
                    None
                }
            };
            self.tracks.insert(path.clone(), track);
        }
        self.tracks.get_mut(path).and_then(Option::as_mut)
    }

    fn open_track(&self, path: &std::path::Path) -> std::result::Result<Track, String> {
        let reader = MediaReader::open_path(path).map_err(|e| e.to_string())?;
        let stream = reader
            .info()
            .first_audio()
            .cloned()
            .ok_or("the file has no sound")?;
        let audio = stream.audio.ok_or("the sound stream declares no format")?;
        let rate = Rate::new(audio.sample_rate, 1).ok_or("the sound declares no sample rate")?;
        if audio.channels > 2 {
            return Err(format!("{} channels and no downmix exists", audio.channels));
        }
        let anchor = match VideoReader::open_path(path) {
            Ok(v) => v
                .extent()
                .map(|(first, _)| first.rescale_checked(rate, Rounding::Floor).0.ticks())
                .unwrap_or(0),
            Err(_) => 0,
        };
        Ok(Track {
            reader,
            rate,
            anchor,
            window: Vec::new(),
            start: 0,
        })
    }
}

impl Track {
    /// Make the window cover file samples `[from, to)`.
    fn cover(&mut self, from: i64, to: i64) -> Result<()> {
        let rate = self.rate;
        let have_end = self.start + (self.window.len() / 2) as i64;
        if from < self.start || from > have_end {
            // Not contiguous with what is decoded: enter again from a seek.
            //
            // Transform codecs overlap each frame with the one before it, so a
            // decoder reset by a seek produces wrong samples until it has seen
            // a frame or two. Entering well before `from` and discarding what
            // precedes it is what makes a seek land on the same samples as
            // continuous decoding.
            let at = RationalTime::exact((from - PREROLL).max(0), rate);
            self.reader.seek_audio(at)?;
            self.window.clear();
            self.start = -1;
        }
        while self.start < 0 || self.start + ((self.window.len() / 2) as i64) < to {
            let Some(frame) = self.reader.next_audio_frame()? else {
                break;
            };
            let first = frame.pts.rescale_checked(rate, Rounding::Nearest).0.ticks();
            if self.start < 0 {
                self.start = first;
            }
            let n = frame.frame_count();
            let left = &frame.channels[0];
            let right = frame.channels.get(1).unwrap_or(left);
            for i in 0..n {
                self.window.push(left[i]);
                self.window.push(right[i]);
            }
        }
        // Forget what is well behind.
        let behind = from - KEEP - self.start;
        if behind > 0 {
            let drop = (behind as usize * 2).min(self.window.len());
            self.window.drain(..drop);
            self.start += (drop / 2) as i64;
        }
        Ok(())
    }

    fn sample(&self, index: i64) -> (f32, f32) {
        let i = index - self.start;
        if i < 0 {
            return (0.0, 0.0);
        }
        let i = i as usize * 2;
        match (self.window.get(i), self.window.get(i + 1)) {
            (Some(l), Some(r)) => (*l, *r),
            _ => (0.0, 0.0),
        }
    }
}

impl AudioSource for SequenceAudio {
    fn sample_rate(&self) -> Rate {
        self.rate
    }

    fn channels(&self) -> usize {
        2
    }

    fn read(&mut self, from: i64, frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        let to = (from + frames as i64).min(self.end);
        if to <= from {
            return Ok(0);
        }
        let n = (to - from) as usize;
        let base = out.len();
        out.resize(base + n * 2, 0.0);
        let rate = self.rate;
        for entry in self.entries.clone() {
            let (Some(path), Some(offset)) = (entry.path.clone(), entry.offset) else {
                continue;
            };
            let e0 = self.sample(entry.record.start_time);
            let e1 = self.sample(entry.record.end_time_exclusive());
            let (a, b) = (from.max(e0), to.min(e1));
            if a >= b {
                continue;
            }
            let Some(track) = self.open(&path, &entry.name) else {
                continue;
            };
            let file_rate = track.rate;
            let (media, per, reversed) = match entry.speed {
                Some(s) => (s.media, s.track, s.reversed),
                None => (1, 1, false),
            };
            let stepper = Stepper::for_speed(
                (media, per),
                (file_rate.num(), file_rate.den()),
                (rate.num(), rate.den()),
            );
            let start = track.anchor + offset.rescale_checked(file_rate, Rounding::Floor).0.ticks();
            let dest = |s: i64| base + (s - from) as usize * 2;

            if stepper.is_identity() && !reversed {
                // One-for-one from a file at the output rate: the samples
                // themselves, with no kernel in the way.
                let file_from = start + (a - e0);
                track.cover(file_from, file_from + (b - a))?;
                for s in a..b {
                    let (l, r) = track.sample(file_from + (s - a));
                    out[dest(s)] += l;
                    out[dest(s) + 1] += r;
                }
                continue;
            }

            // Every other case reads between samples. The positions of the
            // first and last output sample bound what must be decoded.
            let at = |u: i64| -> (i64, f64) {
                let (c, f) = stepper.position(u);
                if reversed {
                    // Backwards from `start`: position -(c + f).
                    if f == 0.0 {
                        (start - c, 0.0)
                    } else {
                        (start - c - 1, 1.0 - f)
                    }
                } else {
                    (start + c, f)
                }
            };
            let (first, _) = at(a - e0);
            let (last, _) = at(b - 1 - e0);
            let (lo, hi) = if first <= last {
                (first, last)
            } else {
                (last, first)
            };
            track.cover(lo - HALF as i64, hi + HALF as i64 + 1)?;
            let cutoff = cutoff_for(stepper.per_output());
            let kernel = self
                .kernels
                .entry(cutoff.to_bits())
                .or_insert_with(|| KernelTable::new(cutoff));
            let track = self
                .tracks
                .get(&path)
                .and_then(Option::as_ref)
                .expect("opened above");
            for s in a..b {
                let (c, f) = at(s - e0);
                let (l, r) = kernel.read(c, f, |i| track.sample(i));
                out[dest(s)] += l;
                out[dest(s) + 1] += r;
            }
        }
        self.clipped += out[base..].iter().filter(|v| v.abs() > 1.0).count() as u64;
        Ok(n)
    }
}
