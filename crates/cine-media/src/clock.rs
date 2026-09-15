//! The presentation clock: where the playhead actually is.
//!
//! ## What makes this a clock and not a timer
//!
//! A timer counts frames and sleeps between them. It drifts, it cannot be
//! rate-changed coherently, and after a stall it has no idea how much time it
//! lost — so it reports a position that is confidently wrong.
//!
//! This derives the media position from a **monotonic wall clock**:
//!
//! ```text
//!   position = anchor_media + (now - anchor_wall) x rate
//! ```
//!
//! Every transport operation re-anchors. A stall therefore shows up as the
//! position having advanced past what was presented — which is exactly the
//! information a scheduler needs in order to drop frames and catch up, and
//! which a frame-counting timer destroys.
//!
//! The arithmetic is in [`RationalTime`], so a playhead at 23.976 does not
//! drift against one at 48 kHz.
//!
//! ## Testing without faking
//!
//! [`TimeSource`] abstracts *reading* the clock, not the clock itself. In
//! production it is the operating system's monotonic clock. In tests it is a
//! handle that advances only when a test says so, which makes playback
//! deterministic without any sleeping. That is the opposite of a fake timer:
//! the real path has no simulation in it at all.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ucg_core::{Rate, RationalTime, Rounding};

/// Where elapsed wall-clock time comes from.
pub trait TimeSource: Send + Sync {
    /// Monotonically non-decreasing time since some fixed origin.
    fn elapsed(&self) -> Duration;
}

/// The operating system's monotonic clock.
pub struct SystemTime {
    origin: Instant,
}

impl Default for SystemTime {
    fn default() -> Self {
        SystemTime {
            origin: Instant::now(),
        }
    }
}

impl TimeSource for SystemTime {
    fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock a test drives by hand. Nanosecond resolution, no sleeping.
#[derive(Clone, Default)]
pub struct ManualTime {
    nanos: Arc<AtomicU64>,
}

impl ManualTime {
    pub fn advance(&self, by: Duration) {
        self.nanos
            .fetch_add(by.as_nanos() as u64, Ordering::Relaxed);
    }
    pub fn advance_secs(&self, secs: f64) {
        self.advance(Duration::from_nanos((secs * 1e9) as u64));
    }
}

impl TimeSource for ManualTime {
    fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Relaxed))
    }
}

/// What the transport is doing.
///
/// These are reported, never guessed. `Degraded` and `Unsupported` exist so
/// that a viewer can say what is wrong instead of appearing to play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportState {
    /// At rest at the start of the range.
    Stopped,
    Playing,
    Paused,
    /// A seek is in flight; the position is the target, not what is on screen.
    Seeking,
    /// Playing, but starved of decoded frames.
    Buffering,
    /// Playing, but not keeping up: frames are being dropped to hold sync.
    Degraded,
    /// The media cannot be played by this build at all.
    Unsupported,
}

impl TransportState {
    /// Whether the playhead advances with the wall clock in this state.
    ///
    /// `Buffering` counts. Starving the picture pipeline does not stop time:
    /// audio does not stop, and a playhead that froze on an underrun would both
    /// hide the stall and desynchronise from it. Excluding it here also made
    /// `underrun()` jump the playhead **backwards** to the last anchor, because
    /// a non-advancing clock reports `anchor_media` — three seconds of playback
    /// became zero. That is the defect this comment exists to prevent
    /// reintroducing; see `an_underrun_does_not_move_the_playhead`.
    pub fn is_advancing(&self) -> bool {
        matches!(
            self,
            TransportState::Playing | TransportState::Degraded | TransportState::Buffering
        )
    }
    pub fn label(&self) -> &'static str {
        match self {
            TransportState::Stopped => "stopped",
            TransportState::Playing => "playing",
            TransportState::Paused => "paused",
            TransportState::Seeking => "seeking",
            TransportState::Buffering => "buffering",
            TransportState::Degraded => "degraded",
            TransportState::Unsupported => "unsupported",
        }
    }
}

/// Playback speed as an exact ratio, so 1/2 speed over an hour is exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackRate {
    num: i32,
    den: u32,
}

impl PlaybackRate {
    pub const FORWARD: PlaybackRate = PlaybackRate { num: 1, den: 1 };

    /// `num/den` times normal speed. Negative plays backwards; zero is refused
    /// because a zero rate is a pause, and conflating the two loses the
    /// distinction between "held" and "playing at no speed".
    pub fn new(num: i32, den: u32) -> Option<PlaybackRate> {
        (num != 0 && den != 0).then_some(PlaybackRate { num, den })
    }
    pub fn is_reverse(&self) -> bool {
        self.num < 0
    }
    /// The exact ratio, for schedulers that must not go through `f64`.
    pub fn num_den(&self) -> (i32, u32) {
        (self.num, self.den)
    }
    pub fn as_f64(&self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }
}

impl Default for PlaybackRate {
    fn default() -> Self {
        PlaybackRate::FORWARD
    }
}

/// Counts that describe how playback is actually going.
///
/// Every field is observed, never estimated. A viewer that reports zero
/// dropped frames while dropping them is worse than one that reports nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaybackTelemetry {
    pub frames_requested: u64,
    pub frames_presented: u64,
    pub frames_dropped: u64,
    /// Presented, but after their deadline had passed.
    pub frames_late: u64,
    pub seeks: u64,
    pub underruns: u64,
}

impl PlaybackTelemetry {
    pub fn drop_rate(&self) -> f64 {
        let total = self.frames_presented + self.frames_dropped;
        if total == 0 {
            0.0
        } else {
            self.frames_dropped as f64 / total as f64
        }
    }
}

/// The playhead.
pub struct PlaybackClock<S: TimeSource> {
    source: S,
    state: TransportState,
    rate: PlaybackRate,
    /// Media position when the current state began.
    anchor_media: RationalTime,
    /// Wall time when the current state began.
    anchor_wall: Duration,
    /// The playable extent. The playhead never leaves it.
    range: (RationalTime, RationalTime),
    telemetry: PlaybackTelemetry,
}

impl<S: TimeSource> PlaybackClock<S> {
    /// A clock over `[start, end)`, stopped at `start`.
    pub fn new(source: S, start: RationalTime, end: RationalTime) -> Self {
        let anchor_wall = source.elapsed();
        PlaybackClock {
            source,
            state: TransportState::Stopped,
            rate: PlaybackRate::FORWARD,
            anchor_media: start,
            anchor_wall,
            range: (start, end),
            telemetry: PlaybackTelemetry::default(),
        }
    }

    /// A clock that refuses to play, because the media cannot be decoded.
    ///
    /// The transport still exists so the interface can show *why* it is not
    /// playing, rather than offering a button that silently does nothing.
    pub fn unsupported(source: S, at: RationalTime) -> Self {
        let mut c = PlaybackClock::new(source, at, at);
        c.state = TransportState::Unsupported;
        c
    }

    pub fn state(&self) -> TransportState {
        self.state
    }
    pub fn rate(&self) -> PlaybackRate {
        self.rate
    }
    pub fn telemetry(&self) -> PlaybackTelemetry {
        self.telemetry
    }
    pub fn range(&self) -> (RationalTime, RationalTime) {
        self.range
    }

    /// Where the playhead is now.
    ///
    /// While playing this is computed from the wall clock, so it keeps
    /// advancing whether or not frames are arriving — which is what lets a
    /// stall be detected instead of hidden.
    pub fn position(&self) -> RationalTime {
        if !self.state.is_advancing() {
            return self.anchor_media;
        }
        let wall = self.source.elapsed().saturating_sub(self.anchor_wall);
        let advanced = self.media_delta(wall);
        self.clamp(self.anchor_media.checked_add(&advanced))
    }

    /// Wall-clock duration converted to media time at the current rate,
    /// exactly: nanoseconds are a rational, and so is the rate.
    fn media_delta(&self, wall: Duration) -> RationalTime {
        let nanos = wall.as_nanos().min(i64::MAX as u128) as i64;
        // Nanoseconds are ticks at 1 GHz. Scale by the playback rate as an
        // exact ratio, then leave it at that rate; conversion to the media's
        // own rate happens on demand.
        let scaled = nanos.saturating_mul(i64::from(self.rate.num)) / i64::from(self.rate.den);
        RationalTime::exact(scaled, Rate::whole(1_000_000_000).expect("1 GHz"))
    }

    fn clamp(&self, t: RationalTime) -> RationalTime {
        let (lo, hi) = self.range;
        if t < lo {
            lo
        } else if t > hi {
            hi
        } else {
            t
        }
    }

    /// Re-anchor to the current position, so a state change does not jump.
    fn reanchor(&mut self) {
        self.anchor_media = self.position();
        self.anchor_wall = self.source.elapsed();
    }

    // -- transport ---------------------------------------------------------

    /// Begin playing. Refused on media this build cannot play.
    pub fn play(&mut self) -> Result<(), TransportState> {
        if self.state == TransportState::Unsupported {
            return Err(TransportState::Unsupported);
        }
        // Read the position before touching any anchor: `reanchor` would
        // recompute it from the old anchor and undo the rewind below.
        let here = self.position();
        // Pressing play at the far end would otherwise go nowhere. Rewind to
        // the other end instead of sitting still, in whichever direction this
        // rate is going.
        let from = if !self.rate.is_reverse() && here >= self.range.1 {
            self.range.0
        } else if self.rate.is_reverse() && here <= self.range.0 {
            self.range.1
        } else {
            here
        };
        self.anchor_media = from;
        self.anchor_wall = self.source.elapsed();
        self.state = TransportState::Playing;
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state == TransportState::Unsupported {
            return;
        }
        self.reanchor();
        self.state = TransportState::Paused;
    }

    /// Stop and return to the start of the range.
    pub fn stop(&mut self) {
        if self.state == TransportState::Unsupported {
            return;
        }
        self.anchor_media = self.range.0;
        self.anchor_wall = self.source.elapsed();
        self.state = TransportState::Stopped;
    }

    /// Move the playhead. Playing continues from the new position.
    pub fn seek(&mut self, to: RationalTime) -> RationalTime {
        if self.state == TransportState::Unsupported {
            return self.anchor_media;
        }
        let was_advancing = self.state.is_advancing();
        self.anchor_media = self.clamp(to);
        self.anchor_wall = self.source.elapsed();
        self.telemetry.seeks += 1;
        self.state = if was_advancing {
            TransportState::Playing
        } else {
            TransportState::Paused
        };
        self.anchor_media
    }

    /// Change speed without moving the playhead.
    pub fn set_rate(&mut self, rate: PlaybackRate) -> Result<(), TransportState> {
        if self.state == TransportState::Unsupported {
            return Err(TransportState::Unsupported);
        }
        self.reanchor();
        self.rate = rate;
        Ok(())
    }

    /// Step by whole frames at `rate`. Only while not advancing: stepping a
    /// moving playhead is ambiguous, and guessing would place the playhead
    /// somewhere the user did not ask for.
    pub fn step(&mut self, frames: i64, rate: Rate) -> Result<RationalTime, TransportState> {
        if self.state == TransportState::Unsupported {
            return Err(TransportState::Unsupported);
        }
        if self.state.is_advancing() {
            return Err(self.state);
        }
        let here = self.position().rescale_checked(rate, Rounding::Floor).0;
        let stepped = here.checked_add(&RationalTime::exact(frames, rate));
        self.anchor_media = self.clamp(stepped);
        self.anchor_wall = self.source.elapsed();
        self.state = TransportState::Paused;
        Ok(self.anchor_media)
    }

    /// Narrow playback to a sub-range, clamping the playhead into it.
    pub fn set_range(&mut self, start: RationalTime, end: RationalTime) {
        self.range = if end >= start {
            (start, end)
        } else {
            (start, start)
        };
        let here = self.position();
        self.anchor_media = self.clamp(here);
        self.anchor_wall = self.source.elapsed();
    }

    // -- reporting what actually happened -----------------------------------

    /// A frame reached the screen. `deadline` is when it should have.
    pub fn presented(&mut self, deadline: RationalTime) {
        self.telemetry.frames_presented += 1;
        if self.position() > deadline {
            self.telemetry.frames_late += 1;
        }
        if self.state == TransportState::Buffering {
            self.state = TransportState::Playing;
        }
    }

    pub fn requested(&mut self) {
        self.telemetry.frames_requested += 1;
    }

    /// A frame was skipped to hold sync.
    pub fn dropped(&mut self) {
        self.telemetry.frames_dropped += 1;
        // From any advancing state, not only `Playing`. Coming out of a stall
        // the state is `Buffering`, and degrading only from `Playing` meant the
        // next presented picture lifted `Buffering` straight back to `Playing` —
        // reporting a healthy transport immediately after dropping ninety-nine
        // pictures. See `a_stall_that_skips_pictures_does_not_report_playing`.
        if self.state.is_advancing() {
            self.state = TransportState::Degraded;
        }
    }

    /// No decoded frame was available when one was due.
    pub fn underrun(&mut self) {
        self.telemetry.underruns += 1;
        if self.state.is_advancing() {
            // The playhead keeps moving: pretending otherwise would hide the
            // stall and desynchronise from audio, which does not stop.
            self.state = TransportState::Buffering;
        }
    }

    /// Playback caught up.
    pub fn recovered(&mut self) {
        if matches!(
            self.state,
            TransportState::Degraded | TransportState::Buffering
        ) {
            self.state = TransportState::Playing;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(seconds: i64) -> (PlaybackClock<ManualTime>, ManualTime) {
        let t = ManualTime::default();
        let rate = Rate::F24;
        let c = PlaybackClock::new(
            t.clone(),
            RationalTime::exact(0, rate),
            RationalTime::exact(seconds * 24, rate),
        );
        (c, t)
    }

    #[test]
    fn a_stopped_playhead_does_not_move_when_time_passes() {
        let (c, t) = clock(10);
        t.advance_secs(3.0);
        assert_eq!(
            c.position().to_seconds(),
            0.0,
            "a stopped clock must not drift"
        );
        assert_eq!(c.state(), TransportState::Stopped);
    }

    #[test]
    fn playing_advances_with_the_wall_clock() {
        let (mut c, t) = clock(10);
        c.play().unwrap();
        t.advance_secs(2.5);
        assert!((c.position().to_seconds() - 2.5).abs() < 1e-9);
        assert_eq!(c.state(), TransportState::Playing);
    }

    #[test]
    fn pausing_holds_the_position_and_resuming_continues_from_it() {
        let (mut c, t) = clock(10);
        c.play().unwrap();
        t.advance_secs(2.0);
        c.pause();
        let held = c.position();
        t.advance_secs(5.0);
        assert_eq!(c.position(), held, "a paused playhead must not move");

        c.play().unwrap();
        t.advance_secs(1.0);
        assert!(
            (c.position().to_seconds() - 3.0).abs() < 1e-9,
            "resuming must continue from where it paused, not from where the wall clock is"
        );
    }

    /// The property a frame-counting timer cannot have: after a long stall the
    /// position reflects real elapsed time, so the scheduler can see it is
    /// behind instead of quietly running slow.
    #[test]
    fn a_stall_shows_up_as_elapsed_time_rather_than_being_hidden() {
        let (mut c, t) = clock(60);
        c.play().unwrap();
        t.advance_secs(1.0);
        c.presented(RationalTime::exact(24, Rate::F24)); // on time at 1 s

        // Nothing is presented for two seconds.
        t.advance_secs(2.0);
        assert!(
            (c.position().to_seconds() - 3.0).abs() < 1e-9,
            "the playhead must reflect real time, not frames delivered"
        );
        c.underrun();
        assert_eq!(c.state(), TransportState::Buffering);
        assert_eq!(c.telemetry().underruns, 1);
    }

    /// Dropping while stalled must degrade. It once did not: `dropped` degraded
    /// only from `Playing`, so a stall's drops left the state at `Buffering` and
    /// the next presented picture lifted it to `Playing`.
    #[test]
    fn a_stall_that_skips_pictures_does_not_report_playing() {
        let (mut c, t) = clock(60);
        c.play().unwrap();
        t.advance_secs(1.0);
        c.underrun();
        assert_eq!(c.state(), TransportState::Buffering);
        c.dropped();
        assert_eq!(
            c.state(),
            TransportState::Degraded,
            "dropping while stalled must degrade, not stay merely buffering"
        );
        c.presented(RationalTime::exact(48, Rate::F24));
        assert_eq!(
            c.state(),
            TransportState::Degraded,
            "a presented picture does not undo the drops before it"
        );
        c.recovered();
        assert_eq!(c.state(), TransportState::Playing);
    }

    /// Reporting an underrun must not move the playhead. It once did: because
    /// `Buffering` was not an advancing state, `position()` fell back to the
    /// anchor and three seconds of playback became zero — a stall that rewound
    /// the picture instead of reporting itself.
    #[test]
    fn an_underrun_does_not_move_the_playhead() {
        let (mut c, t) = clock(60);
        c.play().unwrap();
        t.advance_secs(3.0);
        let before = c.position();
        c.underrun();
        assert_eq!(c.position(), before, "an underrun must not rewind");

        // And time keeps passing while starved, because audio would not stop.
        t.advance_secs(1.0);
        assert!(
            c.position() > before,
            "a frozen playhead would hide the stall it is reporting"
        );

        // Recovering from the stall must not jump either.
        let stalled = c.position();
        c.presented(RationalTime::exact(4 * 24, Rate::F24));
        assert_eq!(c.state(), TransportState::Playing);
        assert_eq!(c.position(), stalled);
    }

    #[test]
    fn the_playhead_never_leaves_its_range() {
        let (mut c, t) = clock(2);
        c.play().unwrap();
        t.advance_secs(100.0);
        assert_eq!(c.position().to_seconds(), 2.0, "must clamp at the end");

        c.seek(RationalTime::exact(-500, Rate::F24));
        assert_eq!(c.position().to_seconds(), 0.0, "must clamp at the start");
    }

    #[test]
    fn a_half_speed_hour_is_exactly_half_an_hour_of_media() {
        let (mut c, t) = clock(7200);
        c.set_rate(PlaybackRate::new(1, 2).unwrap()).unwrap();
        c.play().unwrap();
        t.advance_secs(3600.0);
        let seconds = c.position().to_seconds();
        assert!(
            (seconds - 1800.0).abs() < 1e-6,
            "an hour at half speed is 1800 s of media, got {seconds}"
        );
    }

    #[test]
    fn reverse_play_runs_backwards() {
        let (mut c, t) = clock(10);
        c.seek(RationalTime::exact(5 * 24, Rate::F24));
        c.set_rate(PlaybackRate::new(-1, 1).unwrap()).unwrap();
        c.play().unwrap();
        t.advance_secs(2.0);
        assert!((c.position().to_seconds() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_rate_change_does_not_move_the_playhead() {
        let (mut c, t) = clock(60);
        c.play().unwrap();
        t.advance_secs(4.0);
        let before = c.position();
        c.set_rate(PlaybackRate::new(2, 1).unwrap()).unwrap();
        assert_eq!(
            c.position(),
            before,
            "changing speed must not jump the playhead"
        );
        t.advance_secs(1.0);
        assert!(
            (c.position().to_seconds() - 6.0).abs() < 1e-9,
            "then run at the new speed"
        );
    }

    #[test]
    fn stepping_is_refused_while_playing() {
        let (mut c, _t) = clock(10);
        c.play().unwrap();
        assert_eq!(
            c.step(1, Rate::F24),
            Err(TransportState::Playing),
            "stepping a moving playhead is ambiguous and must be refused, not guessed"
        );
        c.pause();
        assert!(c.step(1, Rate::F24).is_ok());
    }

    #[test]
    fn stepping_moves_whole_frames_and_clamps() {
        let (mut c, _t) = clock(10);
        c.pause();
        assert_eq!(
            c.step(3, Rate::F24).unwrap(),
            RationalTime::exact(3, Rate::F24)
        );
        assert_eq!(
            c.step(-1, Rate::F24).unwrap(),
            RationalTime::exact(2, Rate::F24)
        );
        // Backwards past the start clamps rather than going negative.
        assert_eq!(c.step(-100, Rate::F24).unwrap().to_seconds(), 0.0);
    }

    /// Media this build cannot decode must not offer a transport that appears
    /// to work.
    #[test]
    fn unsupported_media_refuses_every_transport_operation() {
        let t = ManualTime::default();
        let mut c = PlaybackClock::unsupported(t.clone(), RationalTime::exact(0, Rate::F24));
        assert_eq!(c.state(), TransportState::Unsupported);
        assert_eq!(c.play(), Err(TransportState::Unsupported));
        assert_eq!(c.step(1, Rate::F24), Err(TransportState::Unsupported));
        assert_eq!(
            c.set_rate(PlaybackRate::FORWARD),
            Err(TransportState::Unsupported)
        );
        c.pause();
        c.stop();
        assert_eq!(
            c.state(),
            TransportState::Unsupported,
            "state must not be laundered"
        );
        t.advance_secs(10.0);
        assert_eq!(c.position().to_seconds(), 0.0);
    }

    #[test]
    fn dropping_frames_is_reported_as_degraded_not_as_playing() {
        let (mut c, _t) = clock(10);
        c.play().unwrap();
        c.dropped();
        assert_eq!(
            c.state(),
            TransportState::Degraded,
            "a viewer must be able to say so"
        );
        assert_eq!(c.telemetry().frames_dropped, 1);
        assert!((c.telemetry().drop_rate() - 1.0).abs() < 1e-9);
        c.recovered();
        assert_eq!(c.state(), TransportState::Playing);
    }

    #[test]
    fn late_frames_are_counted_separately_from_dropped_ones() {
        let (mut c, t) = clock(10);
        c.play().unwrap();
        t.advance_secs(1.0);
        // A frame whose deadline was half a second ago.
        c.presented(RationalTime::exact(12, Rate::F24));
        assert_eq!(c.telemetry().frames_late, 1);
        assert_eq!(c.telemetry().frames_dropped, 0);
        assert_eq!(c.telemetry().frames_presented, 1);
    }

    #[test]
    fn a_zero_playback_rate_is_refused_because_that_is_a_pause() {
        assert!(PlaybackRate::new(0, 1).is_none());
        assert!(PlaybackRate::new(1, 0).is_none());
        assert!(PlaybackRate::new(-1, 2).unwrap().is_reverse());
    }

    /// Playing to the end and pressing play again should replay, not sit still.
    #[test]
    fn playing_from_the_end_rewinds_rather_than_doing_nothing() {
        let (mut c, t) = clock(2);
        c.play().unwrap();
        t.advance_secs(5.0);
        assert_eq!(c.position().to_seconds(), 2.0);
        c.play().unwrap();
        assert_eq!(c.position().to_seconds(), 0.0);
    }

    #[test]
    fn playing_in_reverse_from_the_start_rewinds_to_the_end() {
        let (mut c, t) = clock(2);
        c.set_rate(PlaybackRate::new(-1, 1).unwrap()).unwrap();
        c.play().unwrap();
        t.advance_secs(5.0);
        assert_eq!(
            c.position().to_seconds(),
            0.0,
            "reverse play clamps at the start"
        );
        c.play().unwrap();
        assert_eq!(
            c.position().to_seconds(),
            2.0,
            "playing backwards from the start should rewind to the end"
        );
    }

    /// A long playback at an NTSC rate must not drift: this is the whole reason
    /// the position is computed in exact time.
    #[test]
    fn an_hour_of_ntsc_playback_does_not_drift() {
        let t = ManualTime::default();
        let rate = Rate::F23_976;
        let mut c = PlaybackClock::new(
            t.clone(),
            RationalTime::exact(0, rate),
            RationalTime::exact(200_000, rate),
        );
        c.play().unwrap();
        // Advance in many small steps, as a real frame loop would.
        for _ in 0..3_600 {
            t.advance_secs(1.0);
        }
        let seconds = c.position().to_seconds();
        assert!(
            (seconds - 3600.0).abs() < 1e-6,
            "an hour of wall clock produced {seconds} s of media position"
        );
    }
}
