//! Audio/video synchronisation: one clock, two consumers.
//!
//! ## Which clock is the master, and why it matters
//!
//! Audio cannot be stretched or skipped without being heard. Video can be
//! repeated or dropped and, up to a point, not noticed. Every serious player
//! therefore paces picture from audio, and this one does too — but it does it by
//! *reusing the abstraction that was already there* rather than adding a second
//! clock to keep in step with the first.
//!
//! [`crate::clock::TimeSource`] is "where elapsed time comes from". In
//! production it was the operating system's monotonic clock. [`AudioTimeSource`]
//! is the same interface backed by **the number of audio frames the device has
//! actually played**. Give a [`PlaybackClock`](crate::clock::PlaybackClock) one
//! of those and the existing video scheduler follows audio exactly, with no
//! changes to it at all:
//!
//! ```text
//!   sink plays frames ──▶ AudioTimeSource ──▶ PlaybackClock ──▶ Scheduler ──▶ picture
//!            ▲                                                     
//!            └── AudioScheduler ◀── AudioSource (decoded samples)
//! ```
//!
//! Three properties fall out of that rather than being implemented:
//!
//! * **Pause is free.** A paused sink plays no frames, elapsed time stops, and
//!   the playhead freezes. Nothing has to be told.
//! * **A stalled audio device freezes the picture with it.** That is the honest
//!   behaviour: the alternative is video running on ahead of sound.
//! * **There is no drift to correct**, because there is no second clock to drift
//!   against. What remains is *offset*, which is measured below.
//!
//! ## What "played" must mean
//!
//! Frames handed to a device are not frames a listener has heard: a device holds
//! a queue. A sink that counted what it was *given* would put the clock ahead of
//! the sound by the whole device buffer, and every picture would be early by the
//! same amount. [`AudioSink::played_counter`] must therefore count what has left
//! the device — written minus queued — and a sink that cannot report that
//! honestly should say so rather than approximate it.
//!
//! ## What is not implemented, and is not faked
//!
//! * **No rate other than 1.** Playing audio faster or slower without changing
//!   its pitch needs a time-stretcher, which does not exist here. At any other
//!   rate the audio is muted and [`AvStatus::Muted`] says why. Silence that
//!   announces itself is honest; pitch-shifted audio pretending to be playback
//!   is not.
//! * **No reverse audio.** Same reason.
//! * **No resampling.** A source whose rate differs from the sink's is refused,
//!   not silently played at the wrong speed.
//! * **No audio device.** [`AudioSink`] is an interface; this crate ships no
//!   implementation of it that reaches hardware. Until one exists and is
//!   measured, this module is a foundation and A/V sync on this build is
//!   `UNSUPPORTED`.

use crate::clock::{PlaybackRate, TimeSource};
use crate::error::{Error, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ucg_core::{Rate, RationalTime, Rounding};

/// A counter of audio frames that have actually been played, shared between the
/// thread feeding the device and the clock reading it.
pub type PlayedFrames = Arc<AtomicU64>;

/// Somewhere decoded audio can be sent to be heard.
pub trait AudioSink {
    /// The rate the device is running at.
    fn sample_rate(&self) -> Rate;

    fn channels(&self) -> usize;

    /// Audio frames the sink will accept right now without blocking.
    fn writable(&self) -> usize;

    /// Hand over interleaved samples. Returns the frames accepted, which may be
    /// fewer than offered.
    fn write(&mut self, interleaved: &[f32]) -> Result<usize>;

    /// Frames handed over that the device has not yet played. This is the
    /// offset between the clock and the speaker, and it is why `played` is not
    /// simply "what I wrote".
    fn queued(&self) -> u64;

    /// Frames that have actually been played, shared so a clock can read it
    /// without touching the sink.
    fn played_counter(&self) -> PlayedFrames;

    /// Discard queued audio. Used on a seek, where continuing to play the old
    /// position would be worse than a gap.
    fn flush(&mut self) -> Result<()>;
}

/// Decoded audio, addressed by absolute sample index.
///
/// Sample-accurate by construction: positions are sample indices in the
/// source's own rate, never seconds.
pub trait AudioSource {
    fn sample_rate(&self) -> Rate;

    fn channels(&self) -> usize;

    /// Append up to `frames` audio frames starting at sample index `from` to
    /// `out`, interleaved. Returns frames produced; zero means end of stream.
    fn read(&mut self, from: i64, frames: usize, out: &mut Vec<f32>) -> Result<usize>;
}

/// Elapsed time as the audio device measures it.
///
/// Reading this is how everything else learns what time it is during playback.
#[derive(Clone)]
pub struct AudioTimeSource {
    played: PlayedFrames,
    rate: Rate,
}

impl AudioTimeSource {
    pub fn new(played: PlayedFrames, rate: Rate) -> AudioTimeSource {
        AudioTimeSource { played, rate }
    }

    /// Frames played so far.
    pub fn frames(&self) -> u64 {
        self.played.load(Ordering::Relaxed)
    }
}

impl TimeSource for AudioTimeSource {
    /// Computed from the absolute frame count every time, never accumulated, so
    /// the rounding to whole nanoseconds cannot build up: it is always under one
    /// nanosecond regardless of how long playback has run.
    fn elapsed(&self) -> Duration {
        frames_to_duration(self.frames(), self.rate)
    }

    /// The frame count *is* the exact reading, and it is what pacing uses.
    /// Nanoseconds cannot represent 1/48000 s, so going through `Duration`
    /// would put every picture boundary just out of reach.
    fn elapsed_exact(&self) -> Option<RationalTime> {
        Some(RationalTime::exact(
            i64::try_from(self.frames()).unwrap_or(i64::MAX),
            self.rate,
        ))
    }
}

/// Exact conversion of a frame count at `rate` into wall time.
pub fn frames_to_duration(frames: u64, rate: Rate) -> Duration {
    let num = i128::from(rate.num()).max(1);
    let den = i128::from(rate.den()).max(1);
    let nanos = i128::from(frames) * den * 1_000_000_000 / num;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// Whether sound is actually accompanying the picture, and if not, why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AvStatus {
    /// Audio is feeding the sink and pacing the picture.
    Locked,
    /// Deliberately silent, with the reason. The picture keeps playing.
    Muted(&'static str),
    /// The source could not keep the sink fed. Audible, and reported.
    Starved,
    /// The source has no more audio.
    EndOfStream,
    /// This configuration cannot be played at all.
    Unsupported(String),
}

impl AvStatus {
    /// Whether sound is genuinely accompanying the picture.
    pub fn is_locked(&self) -> bool {
        matches!(self, AvStatus::Locked)
    }

    pub fn label(&self) -> &str {
        match self {
            AvStatus::Locked => "locked",
            AvStatus::Muted(_) => "muted",
            AvStatus::Starved => "starved",
            AvStatus::EndOfStream => "end of stream",
            AvStatus::Unsupported(_) => "unsupported",
        }
    }
}

/// Counts describing how audio playback is actually going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AvStats {
    pub frames_written: u64,
    pub frames_read: u64,
    /// Times the sink had room and the source had nothing.
    pub underruns: u64,
    pub seeks: u64,
    /// Largest device queue observed, in frames: the offset between the clock
    /// and the speaker at its worst.
    pub worst_queued: u64,
}

/// Feeds an [`AudioSink`] from an [`AudioSource`], and owns the counter the
/// clock reads.
pub struct AudioScheduler<S: AudioSink, A: AudioSource> {
    sink: S,
    source: A,
    /// Next sample index to read from the source.
    cursor: i64,
    /// Sample index that the first not-yet-played frame in the sink came from.
    /// Together with the device queue this gives what is audible *now*.
    playing_from: i64,
    rate: PlaybackRate,
    status: AvStatus,
    stats: AvStats,
    scratch: Vec<f32>,
}

impl<S: AudioSink, A: AudioSource> AudioScheduler<S, A> {
    /// Refuses a source and sink that disagree, rather than resampling silently.
    pub fn new(sink: S, source: A, start: RationalTime) -> Result<Self> {
        if sink.sample_rate() != source.sample_rate() {
            return Err(Error::Unsupported(format!(
                "audio source is {:?} and the sink is {:?}; no resampler exists",
                source.sample_rate(),
                sink.sample_rate()
            )));
        }
        if sink.channels() != source.channels() {
            return Err(Error::Unsupported(format!(
                "audio source has {} channels and the sink {}; no channel mapper exists",
                source.channels(),
                sink.channels()
            )));
        }
        let cursor = start
            .rescale_checked(source.sample_rate(), Rounding::Nearest)
            .0
            .ticks();
        Ok(AudioScheduler {
            sink,
            source,
            cursor,
            playing_from: cursor,
            rate: PlaybackRate::FORWARD,
            status: AvStatus::Locked,
            stats: AvStats::default(),
            scratch: Vec::new(),
        })
    }

    /// The clock that should pace the picture.
    pub fn time_source(&self) -> AudioTimeSource {
        AudioTimeSource::new(self.sink.played_counter(), self.sink.sample_rate())
    }

    pub fn status(&self) -> &AvStatus {
        &self.status
    }
    pub fn stats(&self) -> AvStats {
        self.stats
    }
    pub fn sink(&self) -> &S {
        &self.sink
    }
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
    pub fn source(&self) -> &A {
        &self.source
    }

    /// Where the sound a listener is hearing *now* comes from in the media.
    ///
    /// Not where writing has reached: the device queue sits between them, and
    /// reporting the write position would put the playhead ahead of the sound.
    pub fn position(&self) -> RationalTime {
        let rate = self.source.sample_rate();
        let queued = i64::try_from(self.sink.queued()).unwrap_or(i64::MAX);
        RationalTime::exact(self.cursor.saturating_sub(queued), rate)
    }

    /// Offset between what has been written and what is audible, as time.
    pub fn device_offset(&self) -> Duration {
        frames_to_duration(self.sink.queued(), self.sink.sample_rate())
    }

    /// Only 1x is playable. Anything else mutes, and says so.
    pub fn set_rate(&mut self, rate: PlaybackRate) {
        self.rate = rate;
        self.status = if rate == PlaybackRate::FORWARD {
            AvStatus::Locked
        } else if rate.is_reverse() {
            AvStatus::Muted("no reverse audio: playing samples backwards is not playback")
        } else {
            AvStatus::Muted("no time-stretch: audio at other speeds would change pitch")
        };
    }

    /// Move to a media position, discarding what the device still holds.
    pub fn seek(&mut self, to: RationalTime) -> Result<()> {
        let rate = self.source.sample_rate();
        let (at, _) = to.rescale_checked(rate, Rounding::Nearest);
        self.sink.flush()?;
        self.cursor = at.ticks();
        self.playing_from = self.cursor;
        self.stats.seeks += 1;
        if !matches!(self.status, AvStatus::Muted(_) | AvStatus::Unsupported(_)) {
            self.status = AvStatus::Locked;
        }
        Ok(())
    }

    /// Fill whatever room the sink has. Returns frames written.
    ///
    /// Silent at any rate other than 1: the sink is fed nothing, the device
    /// drains, and the picture stops being paced by audio. That is reported
    /// through [`AvStatus::Muted`] rather than hidden.
    pub fn pump(&mut self) -> Result<usize> {
        self.stats.worst_queued = self.stats.worst_queued.max(self.sink.queued());
        if matches!(self.status, AvStatus::Muted(_) | AvStatus::Unsupported(_)) {
            return Ok(0);
        }
        let room = self.sink.writable();
        if room == 0 {
            return Ok(0);
        }
        self.scratch.clear();
        let produced = self.source.read(self.cursor, room, &mut self.scratch)?;
        if produced == 0 {
            self.status = if self.sink.queued() == 0 {
                AvStatus::EndOfStream
            } else {
                // Still draining what it has; the end is not yet audible.
                AvStatus::Locked
            };
            return Ok(0);
        }
        self.stats.frames_read += produced as u64;
        let channels = self.source.channels().max(1);
        let accepted = self.sink.write(&self.scratch[..produced * channels])?;
        self.cursor += accepted as i64;
        self.stats.frames_written += accepted as u64;
        if accepted < produced {
            // The sink took less than it said it would. Not fatal, but the
            // cursor must follow what was accepted, never what was offered.
            self.status = AvStatus::Starved;
            self.stats.underruns += 1;
        } else if self.status == AvStatus::Starved {
            self.status = AvStatus::Locked;
        }
        Ok(accepted)
    }

    /// The sink had room and the source had nothing to put in it.
    pub fn report_underrun(&mut self) {
        self.stats.underruns += 1;
        self.status = AvStatus::Starved;
    }
}
