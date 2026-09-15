//! Validation for the playback scheduler.
//!
//! ## Why the source here is synthetic and the media test is separate
//!
//! Every decision the scheduler makes — which picture is due, how many were
//! skipped, when the next one falls due, what happens when a decode fails —
//! has to be provable without a decoder in the way. So these tests drive a
//! `FrameSource` that owns nothing but a table of presentation times, against a
//! clock a test advances by hand.
//!
//! That is the opposite of faking playback. Nothing here simulates real time:
//! `ManualTime` is a *reading* of the clock, and the scheduler's arithmetic is
//! the same arithmetic the real path runs. Slow decoding is injected by having
//! the fake source advance the test clock while it "decodes", which is exactly
//! what a slow real decoder does to the wall clock.
//!
//! Real H.264 is exercised in `playback_real_media.rs`, which skips loudly when
//! the asset is absent.

use cine_media::clock::{ManualTime, PlaybackClock, PlaybackRate, TransportState};
use cine_media::frame::{AlphaMode, ColorMetadata, PixelFormat, VideoFrame};
use cine_media::schedule::{FrameSource, Presentation, Scheduler};
use cine_media::time::{Rate, Timestamp};
use cine_media::Error;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// A source with a sample table and no decoder
// ---------------------------------------------------------------------------

struct TableSource {
    pts: Vec<Timestamp>,
    /// How long the last picture is shown, so the extent is exact.
    last_duration: Timestamp,
    /// Wall time each decode consumes, applied to the test clock.
    decode_cost: Option<(ManualTime, Duration)>,
    /// Index whose decode fails outright.
    fail_at: Option<usize>,
    /// Index that resolves but yields no picture.
    empty_at: Option<usize>,
    /// While set, no picture is ready at all — a starved decode-ahead buffer.
    starved: bool,
    reverse_ok: bool,
    /// Every index this source was actually asked to decode, in order.
    decoded: Vec<usize>,
}

impl TableSource {
    /// `count` pictures on the exact frame grid of `rate`.
    fn grid(count: usize, rate: Rate) -> TableSource {
        TableSource {
            pts: (0..count)
                .map(|i| Timestamp::exact(i as i64, rate))
                .collect(),
            last_duration: Timestamp::exact(1, rate),
            decode_cost: None,
            fail_at: None,
            empty_at: None,
            starved: false,
            reverse_ok: true,
            decoded: Vec::new(),
        }
    }

    /// Charge `cost` of wall time to `clock` on every decode.
    fn costing(mut self, clock: &ManualTime, cost: Duration) -> Self {
        self.decode_cost = Some((clock.clone(), cost));
        self
    }
}

impl FrameSource for TableSource {
    fn frame_count(&self) -> usize {
        self.pts.len()
    }
    fn pts_of(&self, index: usize) -> Option<Timestamp> {
        self.pts.get(index).copied()
    }
    fn index_at(&self, t: Timestamp) -> Option<usize> {
        if self.pts.first().is_none_or(|first| *first > t) {
            return None;
        }
        // Deliberately a linear scan: the test's job is to be obviously right,
        // not to re-implement the reader's binary search.
        Some(
            self.pts
                .iter()
                .rposition(|p| *p <= t)
                .expect("checked non-empty above"),
        )
    }
    fn extent(&self) -> Option<(Timestamp, Timestamp)> {
        let first = *self.pts.first()?;
        let last = *self.pts.last()?;
        Some((first, last.checked_add(&self.last_duration)))
    }
    fn decode_index(&mut self, index: usize) -> cine_media::Result<Option<VideoFrame>> {
        if self.starved {
            return Ok(None);
        }
        self.decoded.push(index);
        if let Some((clock, cost)) = &self.decode_cost {
            clock.advance(*cost);
        }
        if self.fail_at == Some(index) {
            return Err(Error::Decode {
                at: format!("index {index}"),
                message: "injected decoder failure".into(),
            });
        }
        if self.empty_at == Some(index) {
            return Ok(None);
        }
        let pts = *self
            .pts
            .get(index)
            .ok_or(Error::FrameNotIndexed(index as i64))?;
        Ok(Some(picture(pts, index)))
    }
    fn supports_reverse(&self) -> bool {
        self.reverse_ok
    }
}

/// A 2x2 grey picture. The scheduler never looks at pixels; only at identity.
fn picture(pts: Timestamp, index: usize) -> VideoFrame {
    VideoFrame {
        pts,
        frame_index: Some(index as i64),
        timecode: None,
        width: 2,
        height: 2,
        format: PixelFormat::Yuv420P8,
        color: ColorMetadata::default(),
        alpha: AlphaMode::None,
        planes: vec![
            (Arc::new(vec![128u8; 4]), 2),
            (Arc::new(vec![128u8; 1]), 1),
            (Arc::new(vec![128u8; 1]), 1),
        ],
    }
}

/// A scheduler over `count` pictures at `rate`, stopped at the start.
fn rig(count: usize, rate: Rate) -> (Scheduler<ManualTime, TableSource>, ManualTime) {
    let t = ManualTime::default();
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(count as i64, rate),
    );
    (Scheduler::new(clock, TableSource::grid(count, rate)), t)
}

fn presented(p: Presentation) -> (usize, u32, bool) {
    match p {
        Presentation::Present(f) => (f.index, f.dropped, f.late_by.is_some()),
        other => panic!("expected a presented frame, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 1. The clock decides which media position is requested
// ---------------------------------------------------------------------------

/// The scheduler must ask for the position the *clock* reports, not for a
/// position it derived by counting how many frames it has shown. This is the
/// whole difference between a scheduler and a frame-counting timer: after the
/// clock jumps, the request must jump with it.
#[test]
fn the_requested_position_comes_from_the_clock_not_from_a_frame_count() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();

    // Three ordinary frames.
    assert_eq!(presented(s.poll()).0, 0);
    t.advance(Duration::from_nanos(41_666_667));
    assert_eq!(presented(s.poll()).0, 1);
    t.advance(Duration::from_nanos(41_666_667));
    assert_eq!(presented(s.poll()).0, 2);

    // Now the wall clock jumps a full second. A frame counter would ask for
    // frame 3; the clock says 26.
    t.advance(Duration::from_secs(1));
    let (index, _, _) = presented(s.poll());
    assert_eq!(
        index, 26,
        "position must follow the clock, not the frame count"
    );
    assert_eq!(
        s.source().decoded,
        vec![0, 1, 2, 26],
        "the skipped pictures must never be decoded — decoding them is the drift"
    );
}

/// The playhead position the scheduler acts on is the clock's, exactly.
#[test]
fn selection_is_exact_at_a_frame_boundary_and_just_either_side_of_it() {
    let (s, _t) = rig(10, Rate::F24);
    let src = s.source();
    assert_eq!(src.index_at(Timestamp::exact(4, Rate::F24)), Some(4));
    assert_eq!(src.index_at(Timestamp::exact(5, Rate::F24)), Some(5));
    // Before the first picture there is nothing to show, and nothing is invented.
    assert_eq!(src.index_at(Timestamp::exact(-1, Rate::F24)), None);
}

/// A playhead one nanosecond short of a frame boundary must select the previous
/// picture. Off-by-one here is a frame of error on every cut.
#[test]
fn a_nanosecond_before_the_boundary_is_still_the_previous_frame() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    // 1/24 s is 41 666 666.66 ns, so 41 666 666 ns is inside frame 0.
    t.advance(Duration::from_nanos(41_666_666));
    assert_eq!(presented(s.poll()).0, 0);
    // 41 666 667 ns is past the boundary.
    t.advance(Duration::from_nanos(1));
    assert_eq!(presented(s.poll()).0, 1);
}

// ---------------------------------------------------------------------------
// 2. Forward playback
// ---------------------------------------------------------------------------

/// Twenty-four polls one frame apart must present twenty-four consecutive
/// pictures, drop nothing, and be late for nothing.
#[test]
fn forward_playback_presents_every_frame_in_order() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();

    let mut shown = Vec::new();
    for _ in 0..24 {
        match s.poll() {
            Presentation::Present(f) => {
                assert_eq!(f.dropped, 0);
                assert!(
                    f.late_by.is_none(),
                    "frame {} was late with no load",
                    f.index
                );
                shown.push(f.index);
            }
            Presentation::Hold => {}
            other => panic!("unexpected {other:?}"),
        }
        let wait = s.next_deadline().expect("playing, so there is a deadline");
        t.advance(wait);
    }
    assert_eq!(shown, (0..24).collect::<Vec<_>>());
    assert_eq!(s.stats().presented, 24);
    assert_eq!(s.stats().dropped, 0);
    assert_eq!(s.stats().late, 0);
    assert_eq!(s.state(), TransportState::Playing);
}

/// Polling faster than the frame rate must not present the same picture twice.
#[test]
fn polling_more_often_than_the_frame_rate_holds_instead_of_re_presenting() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 0);

    for _ in 0..10 {
        t.advance(Duration::from_nanos(1_000_000)); // 1 ms
        assert!(matches!(s.poll(), Presentation::Hold));
    }
    assert_eq!(s.stats().presented, 1);
    assert_eq!(s.stats().held, 10);
    assert_eq!(
        s.source().decoded,
        vec![0],
        "holding must not re-enter the decoder"
    );
}

/// The deadline is the exact wall time until the picture on screen expires, and
/// it must shrink as the playhead crosses the frame.
#[test]
fn the_deadline_is_exact_and_counts_down() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    s.poll();
    // 1/24 s = 41 666 666.67 ns, rounded up so we never wake early.
    assert_eq!(s.next_deadline(), Some(Duration::from_nanos(41_666_667)));
    t.advance(Duration::from_nanos(20_000_000));
    assert_eq!(s.next_deadline(), Some(Duration::from_nanos(21_666_667)));
}

/// A rate change must change the deadline, not the frame grid.
#[test]
fn half_speed_doubles_the_wall_time_between_frames() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    s.poll();
    s.clock_mut()
        .set_rate(PlaybackRate::new(1, 2).unwrap())
        .unwrap();
    assert_eq!(s.next_deadline(), Some(Duration::from_nanos(83_333_334)));
    s.clock_mut()
        .set_rate(PlaybackRate::new(2, 1).unwrap())
        .unwrap();
    assert_eq!(s.next_deadline(), Some(Duration::from_nanos(20_833_334)));
}

/// A stopped or paused transport has nothing to wake up for. Returning a
/// deadline anyway would make an event loop spin at full speed on a still frame.
#[test]
fn a_paused_transport_has_no_deadline() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    s.poll();
    assert!(s.next_deadline().is_some());
    s.clock_mut().pause();
    assert_eq!(s.next_deadline(), None);
}

// ---------------------------------------------------------------------------
// 3. Dropped and degraded presentation
// ---------------------------------------------------------------------------

/// A decoder that takes four frame intervals per picture cannot hold 24 fps.
/// The scheduler must say so: skipped pictures counted as drops, the transport
/// reported as degraded, and the skipped pictures never decoded.
#[test]
fn a_decoder_that_cannot_keep_up_drops_frames_and_reports_degraded() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(240, rate),
    );
    // Four frame intervals per decode.
    let src = TableSource::grid(240, rate).costing(&t, Duration::from_nanos(166_666_668));
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().play().unwrap();

    let mut indices = Vec::new();
    for _ in 0..5 {
        match s.poll() {
            Presentation::Present(f) => indices.push((f.index, f.dropped)),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(indices[0], (0, 0), "the first picture has nothing to skip");
    for (index, dropped) in &indices[1..] {
        assert_eq!(
            *dropped, 3,
            "four intervals per decode skips three pictures"
        );
        assert!(*index > 0);
    }
    assert_eq!(s.stats().dropped, 12);
    assert_eq!(
        s.state(),
        TransportState::Degraded,
        "a viewer that drops frames while reporting Playing is lying"
    );
    assert!(s.stats().drop_rate() > 0.5);
    // The clock's own telemetry must agree with the scheduler's.
    assert_eq!(s.clock().telemetry().frames_dropped, 12);
    assert_eq!(s.clock().telemetry().frames_presented, 5);
}

/// A picture that misses its window must be reported late, with the real
/// overshoot — not silently presented as if it were on time.
#[test]
fn a_frame_that_misses_its_window_is_reported_late_with_the_real_overshoot() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(240, rate),
    );
    // 60 ms per decode against a 41.67 ms frame: late by ~18.3 ms.
    let src = TableSource::grid(240, rate).costing(&t, Duration::from_millis(60));
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().play().unwrap();

    let Presentation::Present(f) = s.poll() else {
        panic!("expected a frame")
    };
    let late = f
        .late_by
        .expect("60 ms of decode cannot make a 41.67 ms window");
    assert!(
        late >= Duration::from_nanos(18_333_000) && late <= Duration::from_nanos(18_334_000),
        "overshoot must be measured, got {late:?}"
    );
    assert_eq!(s.stats().late, 1);
    // The clock must have reached the same conclusion independently: the
    // scheduler's counters and the transport's must not be able to disagree.
    assert_eq!(s.clock().telemetry().frames_late, 1);
    assert_eq!(s.clock().telemetry().frames_presented, 1);
}

/// Under no load, nothing is late. Guards against a lateness test that would
/// pass whatever the code did.
#[test]
fn nothing_is_late_when_decoding_costs_nothing() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    for _ in 0..10 {
        s.poll();
        t.advance(s.next_deadline().unwrap());
    }
    assert_eq!(s.stats().late, 0);
    assert_eq!(s.stats().dropped, 0);
}

/// After the load passes, the transport must climb back out of Degraded rather
/// than staying degraded for the rest of the session.
#[test]
fn the_transport_recovers_once_frames_are_being_made_again() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(240, rate),
    );
    let mut src = TableSource::grid(240, rate);
    src.decode_cost = Some((t.clone(), Duration::from_millis(200)));
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().play().unwrap();
    s.poll();
    s.poll();
    assert_eq!(s.state(), TransportState::Degraded);

    // Load lifts.
    s.source_mut().decode_cost = None;
    s.poll();
    t.advance(s.next_deadline().unwrap());
    s.poll();
    assert_eq!(s.state(), TransportState::Playing);
}

// ---------------------------------------------------------------------------
// 4. Reverse playback
// ---------------------------------------------------------------------------

/// Reverse must walk the pictures backwards, one per frame interval.
#[test]
fn reverse_playback_walks_backwards_one_picture_at_a_time() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().seek(Timestamp::exact(100, Rate::F24));
    s.clock_mut()
        .set_rate(PlaybackRate::new(-1, 1).unwrap())
        .unwrap();
    s.clock_mut().play().unwrap();

    let mut shown = Vec::new();
    for _ in 0..6 {
        match s.poll() {
            Presentation::Present(f) => shown.push(f.index),
            other => panic!("every scheduled wake must yield a picture, got {other:?}"),
        }
        t.advance(s.next_deadline().expect("playing backwards has a deadline"));
    }
    assert_eq!(shown, vec![100, 99, 98, 97, 96, 95]);
    assert_eq!(s.stats().dropped, 0);
    assert_eq!(
        s.stats().held,
        0,
        "a deadline that lands exactly on a boundary wastes a wake per frame"
    );
}

/// Going backwards, the picture expires when the playhead falls below its own
/// start — not when it reaches the next one.
#[test]
fn the_reverse_deadline_measures_to_the_bottom_of_the_current_picture() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().seek(Timestamp::exact(100, Rate::F24));
    s.clock_mut()
        .set_rate(PlaybackRate::new(-1, 1).unwrap())
        .unwrap();
    s.clock_mut().play().unwrap();
    s.poll();
    // Sitting exactly on frame 100's bottom edge, which frame 100 still owns:
    // one nanosecond leaves it, and nothing less will.
    assert_eq!(s.next_deadline(), Some(Duration::from_nanos(1)));

    // Half way into frame 98 there is half a frame left, plus the tick needed
    // to fall below the edge rather than land on it.
    t.advance(Duration::from_nanos(62_500_000)); // 1.5 frames
    assert_eq!(presented(s.poll()).0, 98);
    assert_eq!(s.next_deadline(), Some(Duration::from_nanos(20_833_335)));
}

/// Reverse ends at the start of the range, not at the end of it.
#[test]
fn reverse_playback_ends_at_the_beginning_of_the_media() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().seek(Timestamp::exact(3, Rate::F24));
    s.clock_mut()
        .set_rate(PlaybackRate::new(-1, 1).unwrap())
        .unwrap();
    s.clock_mut().play().unwrap();
    for _ in 0..3 {
        s.poll();
        t.advance(Duration::from_nanos(41_666_667));
    }
    assert!(matches!(s.poll(), Presentation::EndOfMedia));
    assert_eq!(
        s.state(),
        TransportState::Paused,
        "the clock must stop so the loop does not spin at the boundary"
    );
}

/// A source that genuinely cannot go backwards must refuse, not silently sit on
/// one frame while the transport claims to be playing.
#[test]
fn a_source_that_cannot_reverse_refuses_rather_than_pretending() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(240, rate),
    );
    let mut src = TableSource::grid(240, rate);
    src.reverse_ok = false;
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().seek(Timestamp::exact(100, rate));
    s.clock_mut()
        .set_rate(PlaybackRate::new(-1, 1).unwrap())
        .unwrap();
    s.clock_mut().play().unwrap();
    match s.poll() {
        Presentation::Failed(why) => assert!(why.contains("backwards"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(s.state(), TransportState::Paused);
    assert!(
        s.source().decoded.is_empty(),
        "nothing should have been decoded"
    );
}

// ---------------------------------------------------------------------------
// 5. Seeking
// ---------------------------------------------------------------------------

/// A seek must land on the exact picture and must not be counted as drops.
#[test]
fn seeking_lands_exactly_and_is_not_a_dropped_frame() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 0);

    s.seek(Timestamp::exact(137, Rate::F24));
    let (index, dropped, _) = presented(s.poll());
    assert_eq!(index, 137);
    assert_eq!(dropped, 0, "a seek is not a dropped frame");
    assert_eq!(s.stats().dropped, 0);
    assert_eq!(s.clock().telemetry().seeks, 1);
}

/// Seeking backwards while playing forwards must not be read as a drop either.
#[test]
fn seeking_backwards_during_forward_playback_is_not_a_drop() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    for _ in 0..5 {
        s.poll();
        t.advance(s.next_deadline().unwrap());
    }
    s.seek(Timestamp::exact(1, Rate::F24));
    let (index, dropped, _) = presented(s.poll());
    assert_eq!(index, 1);
    assert_eq!(dropped, 0);
    assert_eq!(s.stats().dropped, 0);
}

/// Seeking while paused must still update the picture — that is scrubbing.
#[test]
fn scrubbing_while_paused_presents_the_frame_under_the_playhead() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().pause();
    for target in [0i64, 60, 61, 12, 239] {
        s.seek(Timestamp::exact(target, Rate::F24));
        assert_eq!(presented(s.poll()).0, target as usize);
    }
    assert_eq!(s.stats().dropped, 0);
    assert_eq!(s.state(), TransportState::Paused);
}

/// Seeking to the same picture is a hold, not a redundant decode.
#[test]
fn a_seek_within_the_same_picture_does_not_re_decode() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().pause();
    s.seek(Timestamp::exact(10, Rate::F24));
    assert_eq!(presented(s.poll()).0, 10);
    // A position inside frame 10 but not on its boundary.
    s.seek(Timestamp::exact(
        10 * 1000 + 500,
        Rate::whole(24_000).unwrap(),
    ));
    assert!(matches!(s.poll(), Presentation::Hold));
    assert_eq!(s.source().decoded, vec![10]);
}

// ---------------------------------------------------------------------------
// 6. End of media
// ---------------------------------------------------------------------------

#[test]
fn playback_stops_at_the_end_of_the_media_and_says_so() {
    let (mut s, t) = rig(5, Rate::F24);
    s.clock_mut().play().unwrap();
    let mut shown = Vec::new();
    for _ in 0..5 {
        if let Presentation::Present(f) = s.poll() {
            shown.push(f.index);
        }
        t.advance(Duration::from_nanos(41_666_667));
    }
    assert_eq!(shown, vec![0, 1, 2, 3, 4]);
    assert!(matches!(s.poll(), Presentation::EndOfMedia));
    assert_eq!(s.state(), TransportState::Paused);
    assert_eq!(s.stats().end_of_media, 1);
    // Polling again must not spin or advance.
    assert!(matches!(s.poll(), Presentation::Hold));
}

/// The last picture must be held for its own duration, not cut short.
#[test]
fn the_final_picture_is_shown_for_its_whole_duration() {
    let (mut s, t) = rig(5, Rate::F24);
    s.clock_mut().seek(Timestamp::exact(4, Rate::F24));
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 4);
    assert_eq!(s.next_deadline(), Some(Duration::from_nanos(41_666_667)));
    t.advance(Duration::from_nanos(41_666_666));
    assert!(matches!(s.poll(), Presentation::Hold));
    t.advance(Duration::from_nanos(1));
    assert!(matches!(s.poll(), Presentation::EndOfMedia));
}

/// A playhead before the first picture shows nothing and invents nothing.
#[test]
fn a_position_before_the_first_picture_presents_nothing() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let mut src = TableSource::grid(5, rate);
    // The track starts at frame 10, as a trimmed source would.
    src.pts = (10..15).map(|i| Timestamp::exact(i, rate)).collect();
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(15, rate),
    );
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().play().unwrap();
    assert!(matches!(s.poll(), Presentation::BeforeStart));
    assert!(s.source().decoded.is_empty());
}

// ---------------------------------------------------------------------------
// 7. Decoder failure and unsupported media
// ---------------------------------------------------------------------------

/// A decode failure must surface the decoder's own message and stop the
/// transport. Substituting a blank frame, or carrying on, is the failure mode
/// this project exists to avoid.
#[test]
fn a_decode_failure_is_reported_and_stops_the_transport() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(240, rate),
    );
    let mut src = TableSource::grid(240, rate);
    src.fail_at = Some(2);
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().play().unwrap();
    s.poll();
    t.advance(Duration::from_nanos(83_333_334));
    match s.poll() {
        Presentation::Failed(why) => {
            assert!(why.contains("injected decoder failure"), "{why}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
    assert_eq!(s.state(), TransportState::Paused);
    assert_eq!(s.stats().decode_failures, 1);
    assert_eq!(s.on_screen(), Some(0), "the last good picture stays up");
}

/// A source whose table names a picture it cannot produce must report an
/// underrun rather than presenting a stale frame as new.
#[test]
fn a_source_that_yields_no_picture_reports_an_underrun() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(240, rate),
    );
    let mut src = TableSource::grid(240, rate);
    src.empty_at = Some(1);
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().play().unwrap();
    s.poll();
    let before = s.position();
    t.advance(Duration::from_nanos(41_666_667));
    assert!(matches!(s.poll(), Presentation::Underrun));
    assert_eq!(s.on_screen(), Some(0), "the last good picture stays up");
    // The stall is counted where every other transport fact is counted.
    assert_eq!(s.clock().telemetry().underruns, 1);
    assert_eq!(s.state(), TransportState::Buffering);
    // And the playhead did not rewind to hide it.
    assert!(s.position() > before);

    // A picture arriving afterwards clears the stall rather than leaving the
    // transport buffering for the rest of the session.
    t.advance(Duration::from_nanos(41_666_667));
    assert!(matches!(s.poll(), Presentation::Present(_)));
    assert!(matches!(
        s.state(),
        TransportState::Playing | TransportState::Degraded
    ));
}

/// Media this build cannot decode must produce a transport that refuses, and a
/// scheduler that never touches the source.
#[test]
fn unsupported_media_never_reaches_the_decoder() {
    let t = ManualTime::default();
    let rate = Rate::F24;
    let clock = PlaybackClock::unsupported(t.clone(), Timestamp::exact(0, rate));
    let mut s = Scheduler::new(clock, TableSource::grid(240, rate));
    assert!(matches!(s.poll(), Presentation::Unsupported));
    assert!(s.clock_mut().play().is_err());
    assert!(matches!(s.poll(), Presentation::Unsupported));
    assert_eq!(s.next_deadline(), None);
    assert!(s.source().decoded.is_empty());
    assert_eq!(s.stats().presented, 0);
}

// ---------------------------------------------------------------------------
// 8. Exact mixed-rate behaviour
// ---------------------------------------------------------------------------

/// 23.976 is 24000/1001, whose frame duration is not a whole number of
/// nanoseconds. Over 24 000 frames — exactly 1001 seconds — a scheduler that
/// rounded the frame interval would be visibly adrift. This asserts the exact
/// landing.
#[test]
fn twenty_four_thousand_frames_at_23_976_land_exactly_on_1001_seconds() {
    let rate = Rate::F23_976;
    let t = ManualTime::default();
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(24_001, rate),
    );
    let mut s = Scheduler::new(clock, TableSource::grid(24_001, rate));
    s.clock_mut().play().unwrap();
    s.poll();

    t.advance(Duration::from_secs(1001));
    let (index, _, _) = presented(s.poll());
    assert_eq!(
        index, 24_000,
        "1001 s at 24000/1001 fps is exactly 24000 frames"
    );
    // And one nanosecond earlier is still the frame before.
    let t2 = ManualTime::default();
    let clock2 = PlaybackClock::new(
        t2.clone(),
        Timestamp::exact(0, rate),
        Timestamp::exact(24_001, rate),
    );
    let mut s2 = Scheduler::new(clock2, TableSource::grid(24_001, rate));
    s2.clock_mut().play().unwrap();
    s2.poll();
    t2.advance(Duration::from_secs(1001) - Duration::from_nanos(1));
    assert_eq!(presented(s2.poll()).0, 23_999);
}

/// A clock anchored on the audio grid driving a video source at 23.976: neither
/// is normalised onto the other, and selection must still be exact.
#[test]
fn an_audio_rate_clock_selects_video_frames_exactly() {
    let video = Rate::F23_976;
    let audio = Rate::AUDIO_48K;
    let t = ManualTime::default();
    // The range is stated in audio samples; the source is on the video grid.
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, audio),
        Timestamp::exact(48_000 * 10, audio),
    );
    let mut s = Scheduler::new(clock, TableSource::grid(240, video));
    s.clock_mut().play().unwrap();
    s.poll();

    // A frame duration at 24000/1001 is 41 708 333.33 ns, so most frame
    // boundaries are not whole nanoseconds and cannot be reached by advancing a
    // nanosecond clock at all. Every third one can: 99 frames is exactly
    // 4 129 125 000 ns, and exactly 198 198 samples at 48 kHz.
    t.advance(Duration::from_nanos(4_129_125_000));
    let (index, _, _) = presented(s.poll());
    assert_eq!(index, 99, "no rounding between the audio and video grids");

    // The same instant on the audio grid, exactly — not nearly.
    let (on_audio, exact) = s
        .position()
        .rescale_checked(audio, cine_media::time::Rounding::Nearest);
    assert!(exact, "the playhead must land exactly on an audio sample");
    assert_eq!(on_audio.ticks(), 198_198);

    // One nanosecond earlier is the frame before. Nothing is being snapped.
    let t2 = ManualTime::default();
    let clock2 = PlaybackClock::new(
        t2.clone(),
        Timestamp::exact(0, audio),
        Timestamp::exact(48_000 * 10, audio),
    );
    let mut s2 = Scheduler::new(clock2, TableSource::grid(240, video));
    s2.clock_mut().play().unwrap();
    s2.poll();
    t2.advance(Duration::from_nanos(4_129_124_999));
    assert_eq!(presented(s2.poll()).0, 98);
}

/// Different rates, same instant: 29.97 drop-frame territory. Frame 1798 at
/// 30000/1001 must be selected at its exact time and not one frame either side.
#[test]
fn selection_at_29_97_is_exact_at_the_minute_boundary() {
    let rate = Rate::F29_97;
    let src = TableSource::grid(2000, rate);
    for n in [1797i64, 1798, 1799, 1800] {
        assert_eq!(src.index_at(Timestamp::exact(n, rate)), Some(n as usize));
    }
    // One tick short of 1798 at a much finer rate is still 1797.
    let fine = Rate::new(30_000_000, 1001).unwrap();
    assert_eq!(
        src.index_at(Timestamp::exact(1798 * 1000 - 1, fine)),
        Some(1797)
    );
    assert_eq!(
        src.index_at(Timestamp::exact(1798 * 1000, fine)),
        Some(1798)
    );
}

// ---------------------------------------------------------------------------
// 9. Regressions
// ---------------------------------------------------------------------------

/// Stepping must move one *picture*, on media whose time base is not its frame
/// rate. It did not: the shell stepped the clock by one tick of the track's time
/// base, and on a 12288 Hz track that is 1/12288 s — the picture never changed.
/// Every other test here used a 24 Hz grid, where a tick and a frame coincide,
/// which is exactly why nothing caught it.
#[test]
fn stepping_moves_one_picture_on_a_time_base_that_is_not_the_frame_rate() {
    let base = Rate::whole(12288).unwrap();
    let t = ManualTime::default();
    let mut src = TableSource::grid(10, base);
    src.pts = (0..10)
        .map(|i| Timestamp::exact(1024 + i * 512, base))
        .collect();
    src.last_duration = Timestamp::exact(512, base);
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(1024, base),
        Timestamp::exact(1024 + 10 * 512, base),
    );
    let mut s = Scheduler::new(clock, src);
    s.clock_mut().pause();
    assert_eq!(presented(s.poll()).0, 0);

    for want in 1..=3 {
        s.step(1).expect("a next picture exists");
        assert_eq!(presented(s.poll()).0, want, "one step, one picture");
    }
    s.step(-2).unwrap();
    assert_eq!(presented(s.poll()).0, 1);
    // Clamped at both ends rather than wrapping or refusing silently.
    s.step(-100).unwrap();
    assert_eq!(presented(s.poll()).0, 0);
    s.step(100).unwrap();
    assert_eq!(presented(s.poll()).0, 9);
    assert_eq!(s.stats().dropped, 0, "a step is not a dropped frame");
}

/// Stepping a playing transport pauses it first, rather than being refused.
#[test]
fn stepping_while_playing_pauses_and_steps() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 0);
    s.step(5).unwrap();
    assert_eq!(s.state(), TransportState::Paused);
    assert_eq!(presented(s.poll()).0, 5);
}

/// A seek whose first poll underruns must still not be counted as drops when
/// the picture does arrive. The seek flag used to be consumed by the poll that
/// noticed it — the one that underran — so the next poll counted the whole
/// seek distance, 136 pictures, as dropped.
#[test]
fn a_seek_that_underruns_first_is_still_not_a_dropped_frame() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 0);

    s.source_mut().empty_at = Some(137);
    s.seek(Timestamp::exact(137, Rate::F24));
    assert!(matches!(s.poll(), Presentation::Underrun));

    s.source_mut().empty_at = None;
    let (index, dropped, _) = presented(s.poll());
    assert_eq!(index, 137);
    assert_eq!(
        dropped, 0,
        "the seek distance is not a run of dropped frames"
    );
    assert_eq!(s.stats().dropped, 0);
    assert_eq!(s.clock().telemetry().frames_dropped, 0);
}

/// A synchronous source is always ready, so waiting on it returns at once and
/// never reports starvation it cannot have.
#[test]
fn a_synchronous_source_is_never_waited_on() {
    let (mut s, _t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    s.poll();
    assert!(s.wait_for_source(Duration::ZERO));
    assert!(!s.is_starved());
    assert_eq!(s.stats().underrun_episodes, 0);
}

/// Pictures the playhead passes during a stall are dropped, and must be counted
/// as dropped. They are exactly as unseen as pictures skipped by a slow decode;
/// counting only the latter reports a clean run through a stall that showed the
/// viewer nothing.
#[test]
fn pictures_missed_during_a_stall_are_counted_as_dropped() {
    let (mut s, t) = rig(240, Rate::F24);
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 0);

    // The source has nothing for a hundred frames' worth of wall time.
    s.source_mut().starved = true;
    for _ in 0..4 {
        t.advance(Duration::from_nanos(25 * 41_666_667));
        assert!(matches!(s.poll(), Presentation::Underrun));
    }
    assert_eq!(s.stats().underrun_episodes, 1);

    // The picture that finally arrives is the one under the playhead, and the
    // ninety-nine between it and the last presented picture were never shown.
    s.source_mut().starved = false;
    let (index, dropped, _) = presented(s.poll());
    assert_eq!(index, 100);
    assert_eq!(
        dropped, 99,
        "a stall that skipped 99 pictures must report 99 dropped"
    );
    assert_eq!(s.stats().dropped, 99);
    assert_eq!(s.clock().telemetry().frames_dropped, 99);
    assert_eq!(s.state(), TransportState::Degraded);
}

/// A stall that lasts until the end of the media must still report the pictures
/// it skipped. Counting drops only when the *next* picture is presented meant a
/// run that showed one picture and then starved to the end reported zero
/// dropped — measured on screen as 45 of 240 shown, "0 dropped".
#[test]
fn a_stall_that_runs_to_the_end_of_media_still_reports_its_drops() {
    let (mut s, t) = rig(10, Rate::F24);
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 0);

    s.source_mut().starved = true;
    t.advance(Duration::from_nanos(4 * 41_666_667));
    assert!(matches!(s.poll(), Presentation::Underrun));

    // Run off the end while still starved.
    t.advance(Duration::from_secs(5));
    assert!(matches!(s.poll(), Presentation::EndOfMedia));
    assert_eq!(
        s.stats().dropped,
        9,
        "pictures 1..=9 were never shown and must be counted"
    );
    assert_eq!(s.clock().telemetry().frames_dropped, 9);
    assert_eq!(s.stats().presented, 1);
}

/// Playing cleanly to the end reports no drops: the accounting above must not
/// invent them for a run that showed everything.
#[test]
fn playing_cleanly_to_the_end_reports_no_drops() {
    let (mut s, t) = rig(5, Rate::F24);
    s.clock_mut().play().unwrap();
    for _ in 0..5 {
        s.poll();
        t.advance(Duration::from_nanos(41_666_667));
    }
    assert!(matches!(s.poll(), Presentation::EndOfMedia));
    assert_eq!(s.stats().presented, 5);
    assert_eq!(s.stats().dropped, 0, "a complete run must not report drops");
}

/// Reverse counts the pictures below the playhead that it never showed.
#[test]
fn a_reverse_stall_to_the_start_reports_its_drops() {
    let (mut s, t) = rig(20, Rate::F24);
    s.seek(Timestamp::exact(10, Rate::F24));
    s.clock_mut()
        .set_rate(PlaybackRate::new(-1, 1).unwrap())
        .unwrap();
    s.clock_mut().play().unwrap();
    assert_eq!(presented(s.poll()).0, 10);

    s.source_mut().starved = true;
    t.advance(Duration::from_secs(5));
    assert!(matches!(s.poll(), Presentation::EndOfMedia));
    assert_eq!(
        s.stats().dropped,
        10,
        "pictures 9..=0 were never shown and must be counted"
    );
}

/// Drop accounting must not depend on how often the loop polls. Counting only
/// at a presentation made it depend on both the poll rate and on whether another
/// picture ever arrived; counting as the playhead passes each picture does not.
#[test]
fn the_drop_count_is_the_same_however_often_it_is_polled() {
    let jump = {
        let (mut s, t) = rig(240, Rate::F24);
        s.clock_mut().play().unwrap();
        presented(s.poll());
        s.source_mut().starved = true;
        t.advance(Duration::from_nanos(100 * 41_666_667));
        assert!(matches!(s.poll(), Presentation::Underrun));
        s.stats().dropped
    };
    let creeping = {
        let (mut s, t) = rig(240, Rate::F24);
        s.clock_mut().play().unwrap();
        presented(s.poll());
        s.source_mut().starved = true;
        for _ in 0..50 {
            t.advance(Duration::from_nanos(2 * 41_666_667));
            assert!(matches!(s.poll(), Presentation::Underrun));
        }
        s.stats().dropped
    };
    assert_eq!(jump, 99, "one poll across a hundred frames");
    assert_eq!(creeping, jump, "fifty polls across the same hundred frames");
}

/// A picture is counted once, as dropped or as presented, and never twice.
#[test]
fn every_picture_is_accounted_exactly_once() {
    let (mut s, t) = rig(60, Rate::F24);
    s.clock_mut().play().unwrap();
    presented(s.poll());

    // Show some, stall through some, show some more, then run to the end.
    for _ in 0..4 {
        t.advance(Duration::from_nanos(41_666_667));
        s.poll();
    }
    s.source_mut().starved = true;
    for _ in 0..5 {
        t.advance(Duration::from_nanos(4 * 41_666_667));
        s.poll();
    }
    s.source_mut().starved = false;
    for _ in 0..3 {
        t.advance(Duration::from_nanos(41_666_667));
        s.poll();
    }
    t.advance(Duration::from_secs(10));
    assert!(matches!(s.poll(), Presentation::EndOfMedia));

    let stats = s.stats();
    assert_eq!(
        stats.presented + stats.dropped,
        60,
        "presented {} + dropped {} must be every picture exactly once",
        stats.presented,
        stats.dropped
    );
}
