//! The playback scheduler: turning a clock position into a frame on screen.
//!
//! ## The gap this closes
//!
//! [`crate::clock::PlaybackClock`] knows where the playhead is. [`crate::VideoReader`]
//! knows how to decode a picture. Nothing joined them, so "playback" did not
//! exist — a clock that nothing reads is a stopwatch.
//!
//! This is the join:
//!
//! ```text
//!   clock.position()  ->  select frame  ->  decode  ->  Presentation
//!                    \                                /
//!                     `--  next_deadline() <---------'
//! ```
//!
//! ## Why frames are selected by index and not only by time
//!
//! Time alone cannot say what happened. If the playhead moves from 1.0 s to
//! 1.25 s the viewer needs to know whether that is one frame or six, because
//! five skipped pictures is a *dropped frame* event and one is not. The
//! scheduler therefore tracks which display index is on screen and reports the
//! difference. That is the only way `frames_dropped` can be observed rather
//! than estimated.
//!
//! ## What is deliberately not here
//!
//! The scheduler does not care whether decoding happens on its own thread.
//! Over a bare [`VideoReader`](crate::VideoReader) it is synchronous: a slow
//! decode shows up as a late frame and then as drops, never as an underrun.
//! Over [`DecodeAhead`](crate::decode_ahead::DecodeAhead) the picture is
//! usually already decoded, and when it is not the source says "not ready",
//! which the scheduler reports as [`Presentation::Underrun`] and a starved
//! transport — while the playhead keeps moving.
//!
//! There is no audio here, and therefore **no A/V sync**. Audio is a second
//! consumer of the same clock; nothing about that is implemented or claimed.

use crate::clock::{PlaybackClock, TimeSource, TransportState};
use crate::error::Result;
use crate::frame::VideoFrame;
use std::time::Duration;
use ucg_core::RationalTime;

/// What the scheduler needs from something that can produce pictures.
///
/// `VideoReader` implements this. A test implements it with no media at all,
/// which is what makes every decision below testable without a decoder.
pub trait FrameSource {
    /// Number of pictures, in display order.
    fn frame_count(&self) -> usize;

    /// Presentation time of a display index.
    fn pts_of(&self, index: usize) -> Option<RationalTime>;

    /// Display index of the picture presenting **at or before** `t`.
    ///
    /// `None` when `t` precedes the first picture. Must be exact: no tolerance
    /// window, no rounding through seconds.
    fn index_at(&self, t: RationalTime) -> Option<usize>;

    /// The presentation extent, `[first pts, last pts + its duration)`.
    fn extent(&self) -> Option<(RationalTime, RationalTime)>;

    /// Decode the picture at a display index.
    ///
    /// `Ok(None)` means the index is past the end. `Err` means the decode
    /// genuinely failed and must be reported, never substituted.
    fn decode_index(&mut self, index: usize) -> Result<Option<VideoFrame>>;

    /// Where the playhead is and which way it is going, every poll.
    ///
    /// A synchronous source ignores it. A decode-ahead source uses it to decide
    /// what to decode next and what to evict — it is the only way the source
    /// learns about the playhead during a `Hold`, when nothing is requested.
    fn prepare(&mut self, _index: usize, _reverse: bool) {}

    /// Block until the picture at `index` can be produced without waiting, or
    /// until `timeout`. Returns whether it is ready.
    ///
    /// A synchronous source is always "ready": `decode_index` does the work
    /// itself. The default says so rather than pretending to wait.
    fn wait_ready(&self, _index: usize, _timeout: Duration) -> bool {
        true
    }

    /// Display index of the nearest picture at or before `index` that decoding
    /// can start from, if the source knows.
    ///
    /// Used to plan reverse decoding: every picture between that entry point and
    /// the playhead costs the same single walk, so decoding them all at once is
    /// cheaper than decoding them in pieces that each walk again.
    fn keyframe_at_or_before(&self, _index: usize) -> Option<usize> {
        None
    }

    /// Whether this source can play backwards at all.
    ///
    /// A long-GOP source can, but only by re-decoding from a keyframe for every
    /// picture, so it will be slow. Slow is reported through the drop counters;
    /// *impossible* is reported here.
    fn supports_reverse(&self) -> bool {
        true
    }
}

/// What one poll of the scheduler decided.
#[derive(Debug)]
pub enum Presentation {
    /// A new picture is due, and here it is.
    Present(Presented),
    /// The picture already on screen is still the correct one. Nothing to do.
    Hold,
    /// The playhead has left the media in the direction of travel. The clock
    /// has been paused, so polling again will not spin.
    EndOfMedia,
    /// The playhead is before the first picture. Nothing can be shown yet.
    BeforeStart,
    /// The source had no picture ready. See the module note: the synchronous
    /// reader never produces this.
    Underrun,
    /// Decode failed. The clock has been paused and the reason is carried here.
    Failed(String),
    /// This build cannot play this media.
    Unsupported,
}

/// A picture the scheduler decided to put on screen, with the truth about when.
#[derive(Debug)]
pub struct Presented {
    pub frame: VideoFrame,
    pub index: usize,
    /// Presentation time of this picture in the source's own time base.
    pub pts: RationalTime,
    /// Pictures the playhead passed unseen since the last presented picture.
    /// Zero on a healthy poll. They are counted as the playhead passes them,
    /// which may be several polls earlier than this one.
    pub dropped: u32,
    /// How far past this picture's display window the playhead already was when
    /// it reached the screen. `None` means it made its window.
    pub late_by: Option<Duration>,
    /// Wall-clock time the decode itself took. Measured, not estimated.
    pub decode_took: Duration,
}

/// Counts describing how scheduling is actually going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScheduleStats {
    pub polls: u64,
    pub presented: u64,
    pub held: u64,
    /// Pictures never shown because the playhead had passed them.
    pub dropped: u64,
    /// Pictures shown after their display window had closed.
    pub late: u64,
    pub decode_failures: u64,
    pub end_of_media: u64,
    /// Separate stretches during which the due picture was not ready. Counted
    /// once per stretch, so polling faster cannot inflate it.
    pub underrun_episodes: u64,
    /// Polls that found the due picture not ready. Depends on poll rate, and is
    /// reported for that reason rather than as a quality figure.
    pub starved_polls: u64,
    /// Worst single decode, in nanoseconds. Real elapsed time.
    pub decode_worst_nanos: u128,
    pub decode_total_nanos: u128,
}

impl ScheduleStats {
    /// Fraction of pictures that should have been shown but were not.
    pub fn drop_rate(&self) -> f64 {
        let total = self.presented + self.dropped;
        if total == 0 {
            0.0
        } else {
            self.dropped as f64 / total as f64
        }
    }

    pub fn mean_decode(&self) -> Option<Duration> {
        (self.presented > 0).then(|| {
            Duration::from_nanos((self.decode_total_nanos / u128::from(self.presented)) as u64)
        })
    }
}

/// Drives a [`FrameSource`] from a [`PlaybackClock`].
pub struct Scheduler<S: TimeSource, F: FrameSource> {
    clock: PlaybackClock<S>,
    source: F,
    /// Display index currently on screen, once anything has been shown.
    on_screen: Option<usize>,
    /// The furthest picture, in the direction of travel, whose fate is already
    /// counted — presented or dropped.
    ///
    /// Accounting hangs off this rather than off `on_screen` so that every
    /// picture is counted exactly once, as the playhead passes it, whether or
    /// not another picture is ever presented. Counting only at the next
    /// presentation reported a run that showed 42 pictures of 240 as "3
    /// dropped", because the rest were passed during a stall that never ended.
    accounted: Option<usize>,
    /// Pictures counted as dropped since the last presented one. Carried so the
    /// picture that does arrive can say how many were missed on the way, even
    /// though they were counted on the polls that noticed them.
    dropped_since_present: u32,
    /// The index the last poll wanted, whether or not it got it.
    wanted: Option<usize>,
    /// The due picture was not ready on the last poll.
    starved: bool,
    /// A seek happened since the last *presented* picture. Latched rather than
    /// consumed on the poll that notices it, because that poll may underrun —
    /// and the next one would then count the seek distance as dropped frames.
    discontinuity: bool,
    /// The clock's seek count as of the last poll. A change means the playhead
    /// was moved deliberately, so the pictures in between were not *dropped* —
    /// nobody was ever going to see them. Watching the counter rather than
    /// intercepting `Scheduler::seek` catches a seek made straight on the
    /// clock, which is the path a transport UI will actually take.
    seeks_seen: u64,
    stats: ScheduleStats,
}

impl<S: TimeSource, F: FrameSource> Scheduler<S, F> {
    pub fn new(clock: PlaybackClock<S>, source: F) -> Self {
        Scheduler {
            clock,
            source,
            on_screen: None,
            accounted: None,
            dropped_since_present: 0,
            wanted: None,
            starved: false,
            discontinuity: false,
            seeks_seen: 0,
            stats: ScheduleStats::default(),
        }
    }

    pub fn clock(&self) -> &PlaybackClock<S> {
        &self.clock
    }
    pub fn clock_mut(&mut self) -> &mut PlaybackClock<S> {
        &mut self.clock
    }
    pub fn source(&self) -> &F {
        &self.source
    }
    pub fn source_mut(&mut self) -> &mut F {
        &mut self.source
    }
    pub fn stats(&self) -> ScheduleStats {
        self.stats
    }
    pub fn on_screen(&self) -> Option<usize> {
        self.on_screen
    }
    pub fn state(&self) -> TransportState {
        self.clock.state()
    }
    pub fn position(&self) -> RationalTime {
        self.clock.position()
    }

    /// Whether the playhead is travelling backwards.
    fn reverse(&self) -> bool {
        self.clock.rate().is_reverse()
    }

    /// Decide what belongs on screen now, and produce it.
    ///
    /// This is the whole cycle. Calling it more often than the deadline
    /// requires is harmless — it returns [`Presentation::Hold`] and does no
    /// decoding.
    pub fn poll(&mut self) -> Presentation {
        self.stats.polls += 1;

        if self.clock.state() == TransportState::Unsupported {
            return Presentation::Unsupported;
        }
        if self.reverse() && !self.source.supports_reverse() {
            self.clock.pause();
            return Presentation::Failed("this source cannot play backwards".into());
        }

        let advancing = self.clock.state().is_advancing();
        let position = self.clock.position();

        if advancing && self.reached_the_end(position) {
            // Pictures the playhead ran past without any of them reaching the
            // screen are dropped, whether or not another one ever arrives to
            // notice it. Counting only at the next presentation reported a
            // clean run for playback that stalled and then hit the end —
            // measured once as 45 pictures shown out of 240 and "0 dropped".
            let missed = self.missed_to_the_end();
            self.accounted = None;
            for _ in 0..missed {
                self.clock.dropped();
            }
            self.stats.dropped += u64::from(missed);
            self.clock.pause();
            self.stats.end_of_media += 1;
            return Presentation::EndOfMedia;
        }

        let Some(target) = self.source.index_at(position) else {
            // The playhead is before the first picture. Do not invent one.
            return Presentation::BeforeStart;
        };
        let reverse = self.reverse();
        self.source.prepare(target, reverse);
        self.wanted = Some(target);

        if self.clock.telemetry().seeks != self.seeks_seen {
            self.seeks_seen = self.clock.telemetry().seeks;
            self.discontinuity = true;
        }

        // A seek moves the playhead deliberately: what it skipped was never
        // going to be seen, so it is not dropped. Re-base the accounting there.
        if self.discontinuity {
            self.accounted = Some(target);
            self.dropped_since_present = 0;
        }

        if self.on_screen == Some(target) {
            self.stats.held += 1;
            self.starved = false;
            return Presentation::Hold;
        }

        // Pictures the playhead has already gone past can never be shown now,
        // so account for them here rather than waiting for a presentation that
        // may never come. `target` itself is still reachable and is left alone.
        let passed = if advancing {
            self.account_upto(target)
        } else {
            0
        };
        if passed > 0 {
            for _ in 0..passed {
                self.clock.dropped();
            }
            self.stats.dropped += u64::from(passed);
            self.dropped_since_present = self.dropped_since_present.saturating_add(passed);
        }

        let started = std::time::Instant::now();
        let decoded = self.source.decode_index(target);
        let decode_took = started.elapsed();
        self.stats.decode_total_nanos += decode_took.as_nanos();
        self.stats.decode_worst_nanos = self.stats.decode_worst_nanos.max(decode_took.as_nanos());

        let frame = match decoded {
            Ok(Some(f)) => {
                // Reaching it means it was not missed.
                self.accounted = Some(target);
                f
            }
            Ok(None) => {
                // No picture for this position yet. Report it, do not
                // substitute — and tell the clock once per stretch, so the stall
                // is counted where every other transport fact is counted. The
                // playhead keeps moving: see `TransportState::is_advancing`.
                self.stats.starved_polls += 1;
                if !self.starved {
                    self.starved = true;
                    self.stats.underrun_episodes += 1;
                    self.clock.underrun();
                }
                return Presentation::Underrun;
            }
            Err(e) => {
                self.stats.decode_failures += 1;
                self.clock.pause();
                return Presentation::Failed(e.to_string());
            }
        };

        let pts = self.source.pts_of(target).unwrap_or(frame.pts);
        // The picture's display window closes where the next one begins.
        let window_end = self.window_end(target);
        let late_by = window_end.and_then(|end| self.overshoot(end));

        if let Some(end) = window_end {
            self.clock.presented(end);
        } else {
            self.clock.presented(pts);
        }
        self.stats.presented += 1;
        self.starved = false;
        self.discontinuity = false;
        let missed = std::mem::take(&mut self.dropped_since_present);
        if late_by.is_some() {
            self.stats.late += 1;
        } else if missed == 0 {
            self.clock.recovered();
        }
        self.on_screen = Some(target);

        Presentation::Present(Presented {
            frame,
            index: target,
            pts,
            dropped: missed,
            late_by,
            decode_took,
        })
    }

    /// How long until the picture on screen stops being the correct one.
    ///
    /// `None` when the playhead is not advancing — there is then nothing to
    /// wake up for, and an event loop should wait for input instead of
    /// spinning. `Some(ZERO)` means a new picture is already due.
    ///
    /// This is what makes the loop a scheduler: it sleeps until the *clock*
    /// says a frame is due, rather than sleeping a fixed frame interval and
    /// accumulating error.
    pub fn next_deadline(&self) -> Option<Duration> {
        if !self.clock.state().is_advancing() {
            return None;
        }
        // Starved, the next useful moment is when the *source* has the picture,
        // not when the clock crosses a boundary: the boundary has usually
        // already passed, and waking on it would spin. Wait on
        // `wait_for_source` or the source's own notification instead.
        if self.starved {
            return None;
        }
        let current = self.on_screen?;
        let boundary = if self.reverse() {
            // Travelling backwards, the change happens on leaving the bottom of
            // this picture's window.
            self.source.pts_of(current)?
        } else {
            self.window_end(current)?
        };
        let position = self.clock.position();
        // The playhead closes on the boundary from above when travelling
        // backwards and from below when travelling forwards. Measuring it the
        // one way would report zero for every reverse frame, and a loop woken
        // on a zero deadline spins.
        let (from, to) = if self.reverse() {
            (boundary, position)
        } else {
            (position, boundary)
        };
        let nanos = wall_nanos_between(from, to, self.clock.rate().num_den())?;
        // A picture owns `[pts, next_pts)`, so its bottom edge belongs to it.
        // Running forwards, arriving at the top edge has already left it.
        // Running backwards, arriving at the bottom edge has *not*: the loop
        // has to get one tick below. Without this the playhead lands exactly on
        // a boundary, the poll correctly holds, and reverse costs a wasted wake
        // per frame — measured, and the reason this is not just `max(1)`.
        let nanos = if self.reverse() { nanos + 1 } else { nanos };
        Some(Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64))
    }

    /// Whether the last poll found the due picture not ready.
    pub fn is_starved(&self) -> bool {
        self.starved
    }

    /// Block until the picture the last poll wanted is ready, or `timeout`.
    ///
    /// This is how a loop waits while starved without spinning and without a
    /// timer: it waits on the source. A synchronous source returns at once.
    pub fn wait_for_source(&self, timeout: Duration) -> bool {
        match self.wanted {
            Some(index) => self.source.wait_ready(index, timeout),
            None => true,
        }
    }

    /// Step by whole pictures, by display index, and pause.
    ///
    /// Not by time. The clock's own `step` moves by whole ticks of whatever rate
    /// it is given, and the natural rate to hand it — the media time base — is a
    /// tick, not a frame: on a 12288 Hz track one "step" moved 1/12288 s and the
    /// picture never changed. Stepping the index cannot make that mistake, and
    /// it is also correct for media whose frames are not evenly spaced.
    ///
    /// Returns the new position, or `None` when there is nothing to step to.
    pub fn step(&mut self, pictures: i64) -> Option<RationalTime> {
        if self.clock.state() == TransportState::Unsupported {
            return None;
        }
        if self.clock.state().is_advancing() {
            self.clock.pause();
        }
        let count = self.source.frame_count();
        if count == 0 {
            return None;
        }
        let here = self
            .on_screen
            .or_else(|| self.source.index_at(self.clock.position()))
            .unwrap_or(0);
        let to = (here as i64 + pictures).clamp(0, count as i64 - 1) as usize;
        let pts = self.source.pts_of(to)?;
        Some(self.clock.seek(pts))
    }

    /// Move the playhead. The next poll decodes whatever is now underneath it.
    ///
    /// What is on screen is deliberately *not* forgotten: seeking back onto the
    /// same picture then reports `Hold` and re-decodes nothing, which is what
    /// scrubbing inside one frame should cost.
    pub fn seek(&mut self, to: RationalTime) -> RationalTime {
        self.clock.seek(to)
    }

    /// Where the picture at `index` stops being the right one.
    fn window_end(&self, index: usize) -> Option<RationalTime> {
        match self.source.pts_of(index + 1) {
            Some(next) => Some(next),
            // The last picture: its window ends where the media does.
            None => self.source.extent().map(|(_, end)| end),
        }
    }

    /// Wall-clock overshoot past `boundary`, or `None` if not past it.
    ///
    /// **Lateness is not measured in reverse.** A picture's window is
    /// `[pts, next_pts)`, so running backwards the deadline that matters is the
    /// bottom edge, not the top one this is handed. Rather than report a number
    /// derived from the wrong edge, reverse reports no lateness at all; drops
    /// and the transport state still tell the truth about keeping up. Fixing it
    /// means giving the window a direction, which is worth doing when reverse is
    /// fast enough to be worth measuring — see `PLAYBACK.md`.
    fn overshoot(&self, boundary: RationalTime) -> Option<Duration> {
        let position = self.clock.position();
        if self.reverse() {
            return None;
        }
        if position <= boundary {
            return None;
        }
        let nanos = wall_nanos_between(boundary, position, self.clock.rate().num_den())?;
        (nanos > 0).then(|| Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64))
    }

    /// Count every picture strictly between the last accounted one and
    /// `target` as passed unseen, and move the accounting up to the one before
    /// `target`. `target` itself is still reachable, so it is not counted here.
    fn account_upto(&mut self, target: usize) -> u32 {
        let Some(accounted) = self.accounted else {
            // Nothing accounted yet: playback simply starts at `target`.
            self.accounted = if self.reverse() {
                Some(target.saturating_add(1))
            } else {
                Some(target.saturating_sub(1))
            };
            return 0;
        };
        let gap = if self.reverse() {
            accounted.saturating_sub(target).saturating_sub(1)
        } else {
            target.saturating_sub(accounted).saturating_sub(1)
        };
        if gap > 0 {
            self.accounted = Some(if self.reverse() {
                target.saturating_add(1)
            } else {
                target.saturating_sub(1)
            });
        }
        u32::try_from(gap).unwrap_or(u32::MAX)
    }

    /// Pictures between the last accounted one and the end of travel that were
    /// never presented.
    fn missed_to_the_end(&self) -> u32 {
        let Some(accounted) = self.accounted else {
            return 0;
        };
        let count = self.source.frame_count();
        if count == 0 {
            return 0;
        }
        let gap = if self.reverse() {
            accounted
        } else {
            count - 1 - accounted.min(count - 1)
        };
        u32::try_from(gap).unwrap_or(u32::MAX)
    }

    /// Whether the playhead has run off the end it is travelling towards.
    fn reached_the_end(&self, position: RationalTime) -> bool {
        let (lo, hi) = self.clock.range();
        if self.reverse() {
            position <= lo
        } else {
            position >= hi
        }
    }
}

/// Exact wall-clock nanoseconds for the playhead to travel from `a` to `b` at
/// `rate` (a playback-rate numerator and denominator).
///
/// Done in `i128` rather than through `RationalTime` subtraction on purpose: a
/// 12288-tick media timescale and the clock's nanosecond timescale have no
/// common rate expressible in 32 bits, so subtracting them would round. Here
/// nothing rounds except the final division, which rounds **up** so the caller
/// never wakes before the frame is due.
///
/// Returns `None` if `b` is at or behind `a`, or on overflow.
fn wall_nanos_between(a: RationalTime, b: RationalTime, rate: (i32, u32)) -> Option<u128> {
    if b <= a {
        return Some(0);
    }
    let (rate_num, rate_den) = rate;
    if rate_num == 0 || rate_den == 0 {
        return None;
    }
    let (an, ad) = (i128::from(a.ticks()), i128::from(a.rate_exact().num()));
    let a_den = i128::from(a.rate_exact().den());
    let (bn, bd) = (i128::from(b.ticks()), i128::from(b.rate_exact().num()));
    let b_den = i128::from(b.rate_exact().den());

    // (b - a) in seconds = (bn*b_den*an_rate_num - an*a_den*bn_rate_num)
    //                      / (a_rate_num * b_rate_num)
    let numer = bn.checked_mul(b_den)?.checked_mul(nonzero(ad))?
        - an.checked_mul(a_den)?.checked_mul(nonzero(bd))?;
    let denom = ad.checked_mul(bd)?;
    if denom == 0 || numer <= 0 {
        return Some(0);
    }
    // seconds -> nanoseconds, then divided by the playback rate.
    let numer = numer
        .checked_mul(1_000_000_000)?
        .checked_mul(i128::from(rate_den))?;
    let denom = denom.checked_mul(i128::from(rate_num.unsigned_abs()))?;
    // Round up: waking late costs a late frame, waking early costs a spin.
    // `i128::div_ceil` is still unstable, and both operands are positive here.
    u128::try_from((numer + denom - 1) / denom).ok()
}

/// A rate numerator is never zero — `Rate::new` refuses it — but the guard keeps
/// the arithmetic above total rather than relying on that from a distance.
fn nonzero(n: i128) -> i128 {
    if n == 0 {
        1
    } else {
        n
    }
}
