//! Decode-ahead, driven step by step.
//!
//! ## How these stay deterministic with a real thread
//!
//! The worker is a real thread; nothing here fakes it. What the tests control is
//! the decoder: it announces each decode it *begins* on a channel, then blocks
//! until the test grants an outcome. So a test can say "the worker has started
//! picture 2", move the playhead, and then let picture 2 finish — the exact
//! interleaving that matters for cancellation — without a single sleep.
//!
//! Timeouts appear only as a bound on how long a *broken* implementation may
//! hang a test. No assertion depends on how fast anything runs, and "the worker
//! started nothing else" is proved by `wait_idle`, which checks the worker's own
//! plan under its lock rather than by waiting a while and seeing nothing.

use cine_media::clock::{ManualTime, PlaybackClock, TransportState};
use cine_media::decode_ahead::{AheadConfig, DecodeAhead};
use cine_media::frame::{AlphaMode, ColorMetadata, PixelFormat, VideoFrame};
use cine_media::schedule::{FrameSource, Presentation, Scheduler};
use cine_media::time::{Rate, Timestamp};
use cine_media::Error;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Only ever reached if the implementation is broken.
const HANG: Duration = Duration::from_secs(10);

enum Outcome {
    Picture,
    Fail(&'static str),
    Nothing,
}

struct Gated {
    pts: Vec<Timestamp>,
    started: Sender<usize>,
    outcomes: Receiver<Outcome>,
    bytes: usize,
    reverse_ok: bool,
    /// Display positions of keyframes; empty means the source cannot say.
    keyframes: Vec<usize>,
}

impl FrameSource for Gated {
    fn frame_count(&self) -> usize {
        self.pts.len()
    }
    fn pts_of(&self, index: usize) -> Option<Timestamp> {
        self.pts.get(index).copied()
    }
    fn index_at(&self, t: Timestamp) -> Option<usize> {
        self.pts.iter().rposition(|p| *p <= t)
    }
    fn extent(&self) -> Option<(Timestamp, Timestamp)> {
        let last = *self.pts.last()?;
        Some((
            self.pts[0],
            last.checked_add(&Timestamp::exact(1, Rate::F24)),
        ))
    }
    fn decode_index(&mut self, index: usize) -> cine_media::Result<Option<VideoFrame>> {
        let _ = self.started.send(index);
        match self.outcomes.recv() {
            Ok(Outcome::Picture) => Ok(Some(picture(index, self.bytes))),
            Ok(Outcome::Fail(why)) => Err(Error::Decode {
                at: format!("{index}"),
                message: why.into(),
            }),
            Ok(Outcome::Nothing) => Ok(None),
            // The test let go of the decoder: behave like a decoder torn down.
            Err(_) => Err(Error::Decode {
                at: format!("{index}"),
                message: "decoder closed".into(),
            }),
        }
    }
    fn supports_reverse(&self) -> bool {
        self.reverse_ok
    }
    fn keyframe_at_or_before(&self, index: usize) -> Option<usize> {
        self.keyframes.iter().rev().find(|&&k| k <= index).copied()
    }
}

fn picture(index: usize, bytes: usize) -> VideoFrame {
    VideoFrame {
        pts: Timestamp::exact(index as i64, Rate::F24),
        frame_index: Some(index as i64),
        timecode: None,
        width: 1,
        height: 1,
        format: PixelFormat::Yuv420P8,
        color: ColorMetadata::default(),
        alpha: AlphaMode::None,
        planes: vec![(Arc::new(vec![0u8; bytes]), bytes)],
    }
}

/// Field order is load-bearing: fields drop in declaration order, and `grant`
/// must go before `ahead`. Dropping `ahead` joins the worker, and a worker
/// blocked waiting for a grant would otherwise never be released.
struct Rig {
    grant: Sender<Outcome>,
    started: Receiver<usize>,
    ahead: DecodeAhead<Gated>,
}

impl Rig {
    fn new(count: usize, config: AheadConfig) -> Rig {
        Rig::with(count, config, 100, true)
    }
    fn with(count: usize, config: AheadConfig, bytes: usize, reverse_ok: bool) -> Rig {
        Rig::keyed(count, config, bytes, reverse_ok, Vec::new())
    }
    fn keyed(
        count: usize,
        config: AheadConfig,
        bytes: usize,
        reverse_ok: bool,
        keyframes: Vec<usize>,
    ) -> Rig {
        let (started_tx, started) = channel();
        let (grant, outcomes) = channel();
        let source = Gated {
            pts: (0..count)
                .map(|i| Timestamp::exact(i as i64, Rate::F24))
                .collect(),
            started: started_tx,
            outcomes,
            bytes,
            reverse_ok,
            keyframes,
        };
        Rig {
            ahead: DecodeAhead::new(source, config),
            started,
            grant,
        }
    }
    /// The worker must begin decoding exactly `index` next.
    fn expect_start(&self, index: usize) {
        let got = self
            .started
            .recv_timeout(HANG)
            .unwrap_or_else(|_| panic!("the worker never started decoding picture {index}"));
        assert_eq!(got, index, "the worker chose the wrong picture to decode");
    }
    fn finish(&self) {
        self.grant.send(Outcome::Picture).unwrap();
    }
    /// Start-and-finish for each index, in order.
    fn decode(&self, indices: impl IntoIterator<Item = usize>) {
        for i in indices {
            self.expect_start(i);
            self.finish();
        }
    }
    fn settle(&self) {
        assert!(self.ahead.wait_idle(HANG), "the worker never went idle");
        assert!(
            self.started.try_recv().is_err(),
            "the worker started a decode it should not have planned"
        );
    }
}

fn cfg(ahead: usize, behind: usize) -> AheadConfig {
    AheadConfig {
        ahead,
        behind,
        max_bytes: usize::MAX / 2,
        reverse_chunk: 8,
    }
}

// ---------------------------------------------------------------------------
// Forward
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_decoded_until_the_playhead_is_known() {
    let r = Rig::new(100, cfg(5, 0));
    r.settle();
    assert_eq!(r.ahead.stats().decoded, 0);
}

/// The whole point: the pictures after the playhead are decoded before anyone
/// asks for them, and asking is then free.
#[test]
fn decodes_ahead_of_the_playhead_and_serves_without_waiting() {
    let mut r = Rig::new(100, cfg(5, 0));
    r.ahead.prepare(0, false);
    r.decode(0..5);
    r.settle();
    assert_eq!(r.ahead.buffered(), vec![0, 1, 2, 3, 4]);

    for i in 0..5 {
        r.ahead.prepare(i, false);
        let f = r.ahead.decode_index(i).unwrap().expect("already decoded");
        assert_eq!(f.frame_index, Some(i as i64));
    }
    assert_eq!(r.ahead.stats().hits, 5);
    assert_eq!(r.ahead.stats().misses, 0);
}

/// The lookahead is a bound, not a hope: exactly `ahead` pictures, then idle.
#[test]
fn never_decodes_more_than_the_lookahead() {
    let mut r = Rig::new(1000, cfg(3, 0));
    r.ahead.prepare(0, false);
    r.decode(0..3);
    r.settle();
    assert_eq!(r.ahead.stats().decoded, 3);
}

/// Following the playhead: what falls behind is evicted, the window slides.
#[test]
fn the_window_follows_the_playhead_and_evicts_what_it_left_behind() {
    let mut r = Rig::new(100, cfg(3, 1));
    r.ahead.prepare(0, false);
    r.decode(0..3);
    r.settle();

    r.ahead.prepare(2, false); // window is now [1, 5)
    r.decode(3..5);
    r.settle();
    assert_eq!(r.ahead.buffered(), vec![1, 2, 3, 4]);
    assert_eq!(r.ahead.stats().evicted, 1);
}

/// A picture not yet decoded is "not ready" — never a blank frame, never a
/// block — and becomes ready when the worker delivers it.
#[test]
fn a_picture_not_yet_decoded_is_not_ready_rather_than_blocking() {
    let mut r = Rig::new(100, cfg(1, 0));
    assert!(r.ahead.decode_index(0).unwrap().is_none());
    assert_eq!(r.ahead.stats().misses, 1);

    r.expect_start(0);
    r.finish();
    assert!(r.ahead.wait_ready(0, HANG));
    assert!(r.ahead.decode_index(0).unwrap().is_some());
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/// Bounded by bytes. Three 100-byte pictures fit in 300; a fourth is not
/// started, because the budget is checked before decoding, not after.
#[test]
fn memory_is_bounded_by_bytes_and_checked_before_decoding() {
    let mut r = Rig::with(
        100,
        AheadConfig {
            ahead: 50,
            behind: 0,
            max_bytes: 300,
            reverse_chunk: 8,
        },
        100,
        true,
    );
    r.ahead.prepare(0, false);
    r.decode(0..3);
    r.settle();
    let stats = r.ahead.stats();
    assert_eq!(r.ahead.buffered(), vec![0, 1, 2]);
    assert!(
        stats.peak_bytes <= 300,
        "peak {} over budget",
        stats.peak_bytes
    );
    assert_eq!(
        stats.evicted, 0,
        "nothing should be decoded only to be thrown away"
    );
}

/// The picture at the playhead is admitted even when it alone exceeds the
/// budget. Refusing it would leave a consumer waiting for exactly that picture
/// forever.
#[test]
fn the_playhead_picture_is_admitted_even_over_budget() {
    let mut r = Rig::with(
        100,
        AheadConfig {
            ahead: 50,
            behind: 0,
            max_bytes: 10,
            reverse_chunk: 8,
        },
        100,
        true,
    );
    r.ahead.prepare(7, false);
    r.decode([7]);
    r.settle();
    assert_eq!(r.ahead.buffered(), vec![7]);
    assert!(r.ahead.decode_index(7).unwrap().is_some());
}

// ---------------------------------------------------------------------------
// Seeking, generations and cancellation
// ---------------------------------------------------------------------------

/// A seek far away abandons everything queued immediately. The decode already
/// in progress finishes — cancellation is one decode call wide — and its result
/// is discarded and counted, not silently inserted.
#[test]
fn a_seek_away_discards_the_in_flight_picture_and_abandons_the_queue() {
    let mut r = Rig::new(1000, cfg(10, 0));
    r.ahead.prepare(0, false);
    r.decode(0..2);
    r.expect_start(2); // picture 2 is mid-decode

    r.ahead.prepare(500, false);
    r.finish(); // picture 2 completes after the seek

    // The very next decode is for the new playhead: 3..9 were abandoned.
    r.expect_start(500);
    r.finish();
    let stats = r.ahead.stats();
    assert_eq!(
        stats.stale_discarded, 1,
        "picture 2 belonged to the old playhead"
    );
    assert!(stats.generations >= 2);
    assert!(!r.ahead.buffered().contains(&2));
    assert!(
        !r.ahead.buffered().contains(&0),
        "the old window was evicted"
    );
}

/// A seek that lands inside what is already buffered keeps it. The same index
/// is the same picture; a new generation alone is no reason to discard it.
#[test]
fn a_seek_into_the_buffer_keeps_what_is_already_decoded() {
    let mut r = Rig::new(1000, cfg(6, 2));
    r.ahead.prepare(0, false);
    r.decode(0..6);
    r.settle();
    let decoded = r.ahead.stats().decoded;

    r.ahead.prepare(3, false);
    assert!(
        r.ahead.decode_index(3).unwrap().is_some(),
        "must not re-decode"
    );
    assert!(r.ahead.decode_index(4).unwrap().is_some());
    // The window is now [2, 10): only its unfilled tail is decoded.
    r.decode(6..10);
    r.settle();
    assert_eq!(
        r.ahead.stats().decoded,
        decoded + 4,
        "only the new tail was decoded"
    );
    assert_eq!(r.ahead.stats().stale_discarded, 0);
}

/// A picture that was in flight across a *nearby* seek is still correct and is
/// kept.
#[test]
fn an_in_flight_picture_that_still_belongs_is_kept() {
    let mut r = Rig::new(1000, cfg(6, 2));
    r.ahead.prepare(0, false);
    r.expect_start(0);
    r.ahead.prepare(1, false);
    r.finish();
    assert!(r.ahead.wait_ready(0, HANG));
    assert_eq!(r.ahead.stats().stale_discarded, 0);
}

// ---------------------------------------------------------------------------
// Reverse
// ---------------------------------------------------------------------------

/// Backwards, the worker decodes a chunk that *ends* at the playhead, from the
/// bottom up. Each decode after the first continues forward from the one before,
/// so a chunk costs one keyframe walk instead of one per picture — and then the
/// whole chunk is served backwards without another decode.
#[test]
fn reverse_decodes_a_chunk_bottom_up_and_serves_it_backwards() {
    let mut r = Rig::new(
        1000,
        AheadConfig {
            ahead: 8,
            behind: 0,
            max_bytes: usize::MAX / 2,
            reverse_chunk: 8,
        },
    );
    r.ahead.prepare(50, true);
    r.decode(43..=50);
    r.settle();
    assert_eq!(r.ahead.stats().decoded, 8);

    for i in (43..=50).rev() {
        r.ahead.prepare(i, true);
        assert!(
            r.ahead.decode_index(i).unwrap().is_some(),
            "picture {i} should already be decoded"
        );
    }
    assert_eq!(r.ahead.stats().misses, 0);
}

/// Changing direction is a discontinuity: a new generation, and the forward
/// lookahead is not mistaken for reverse coverage.
#[test]
fn changing_direction_starts_a_new_generation() {
    let mut r = Rig::new(1000, cfg(4, 0));
    r.ahead.prepare(100, false);
    r.decode(100..104);
    r.settle();
    let before = r.ahead.stats().generations;
    r.ahead.prepare(100, true);
    assert_eq!(r.ahead.stats().generations, before + 1);
}

/// At the start of the media there is nothing below zero to decode, and the
/// chunk must clamp rather than underflow.
#[test]
fn a_reverse_chunk_near_the_start_clamps_at_zero() {
    let mut r = Rig::new(1000, cfg(8, 0));
    r.ahead.prepare(3, true);
    r.decode(0..=3);
    r.settle();
    assert_eq!(r.ahead.buffered(), vec![0, 1, 2, 3]);
}

/// With keyframes known, a reverse chunk runs from the keyframe to the
/// playhead: every picture in it costs the same single walk, so none of them
/// needs a walk of its own later. Fixed-size chunks over the same GOP would
/// re-enter from the keyframe once per chunk.
#[test]
fn reverse_with_known_keyframes_decodes_from_the_keyframe_in_one_pass() {
    let mut r = Rig::keyed(
        200,
        AheadConfig {
            ahead: 8,
            behind: 0,
            max_bytes: usize::MAX / 2,
            reverse_chunk: 4,
        },
        100,
        true,
        vec![0, 30, 60, 90],
    );
    r.ahead.prepare(50, true);
    // From keyframe 30 straight up to the playhead — not 47..=50, which is what
    // the four-picture fallback chunk would have chosen.
    r.decode(30..=50);
    r.settle();

    // All of it serves reverse playback down to the keyframe without another
    // decode, even though the lookahead is only 8.
    for i in (30..=50).rev() {
        r.ahead.prepare(i, true);
        assert!(r.ahead.decode_index(i).unwrap().is_some(), "picture {i}");
        if i > 30 {
            // Crossing the lookahead into the next GOP down is allowed to start
            // work; what must not happen is a re-decode inside this one.
            while let Ok(started) = r.started.try_recv() {
                assert!(
                    started < 30,
                    "re-decoded picture {started} of a GOP already held"
                );
                r.finish();
            }
        }
    }
}

/// The keyframe-aligned chunk is still bounded by memory: when a whole GOP does
/// not fit, the chunk shrinks to what does, and the budget holds.
#[test]
fn a_keyframe_aligned_reverse_chunk_is_still_bounded_by_the_budget() {
    let mut r = Rig::keyed(
        200,
        AheadConfig {
            ahead: 4,
            behind: 0,
            max_bytes: 1000,
            reverse_chunk: 4,
        },
        100,
        true,
        vec![0, 100],
    );
    // One forward decode teaches the planner how big a picture is.
    r.ahead.prepare(0, false);
    r.decode([0]);
    r.decode(1..4);
    r.settle();

    r.ahead.prepare(90, true);
    // Ten 100-byte pictures fit in 1000: 81..=90, not the 91 from keyframe 0.
    r.decode(81..=90);
    r.settle();
    let stats = r.ahead.stats();
    assert!(
        stats.peak_bytes <= 1000,
        "peak {} over budget",
        stats.peak_bytes
    );
    assert_eq!(r.ahead.buffered(), (81..=90).collect::<Vec<_>>());
}

// ---------------------------------------------------------------------------
// Failure
// ---------------------------------------------------------------------------

/// A decode failure is surfaced exactly when the consumer reaches that picture —
/// the pictures before it are still served, and the worker stops rather than
/// decoding past a failure.
#[test]
fn a_failure_surfaces_at_the_failing_picture_and_nowhere_else() {
    let mut r = Rig::new(100, cfg(6, 0));
    r.ahead.prepare(0, false);
    r.decode(0..2);
    r.expect_start(2);
    r.grant.send(Outcome::Fail("corrupt slice")).unwrap();
    r.settle();

    assert!(r.ahead.decode_index(0).unwrap().is_some());
    r.ahead.prepare(1, false);
    assert!(r.ahead.decode_index(1).unwrap().is_some());
    r.ahead.prepare(2, false);
    match r.ahead.decode_index(2) {
        Err(e) => assert!(e.to_string().contains("corrupt slice"), "{e}"),
        Ok(other) => panic!("a failed decode must be an error, got {other:?}"),
    }
    assert_eq!(r.ahead.stats().failures, 1);
}

/// Moving elsewhere clears the failure: it belonged to that position.
#[test]
fn seeking_away_from_a_failure_resumes_decoding() {
    let mut r = Rig::new(1000, cfg(2, 0));
    r.ahead.prepare(0, false);
    r.expect_start(0);
    r.grant.send(Outcome::Fail("bad")).unwrap();
    r.settle();
    r.ahead.prepare(500, false);
    r.decode(500..502);
    r.settle();
    assert_eq!(r.ahead.buffered(), vec![500, 501]);
}

/// A table that promises more pictures than the stream holds: the worker stops
/// at the first missing one instead of retrying it forever.
#[test]
fn a_stream_shorter_than_its_table_stops_instead_of_retrying() {
    let mut r = Rig::new(100, cfg(10, 0));
    r.ahead.prepare(0, false);
    r.decode(0..3);
    r.expect_start(3);
    r.grant.send(Outcome::Nothing).unwrap();
    r.settle();
    assert_eq!(r.ahead.buffered(), vec![0, 1, 2]);
    assert!(
        r.ahead.wait_ready(3, HANG),
        "an exhausted index is an answer, not a wait"
    );
    assert!(r.ahead.decode_index(3).unwrap().is_none());
}

// ---------------------------------------------------------------------------
// Pre-roll and teardown
// ---------------------------------------------------------------------------

#[test]
fn preroll_waits_for_a_cushion_and_reports_what_it_got() {
    let r = Rig::new(100, cfg(10, 0));
    let started = r.started;
    let grant = r.grant;
    let feeder = std::thread::spawn(move || {
        for _ in 0..6 {
            if started.recv_timeout(HANG).is_err() {
                break;
            }
            grant.send(Outcome::Picture).unwrap();
        }
        // Hold the channels so the worker blocks rather than failing.
        (started, grant)
    });
    let got = r.ahead.preroll(0, 6, HANG);
    assert_eq!(got, 6);
    let _channels = feeder.join().unwrap();
}

/// Pre-roll must not wait forever for pictures that cannot come.
#[test]
fn preroll_returns_early_when_no_progress_is_possible() {
    let r = Rig::with(
        4,
        AheadConfig {
            ahead: 10,
            behind: 0,
            max_bytes: usize::MAX / 2,
            reverse_chunk: 8,
        },
        100,
        true,
    );
    let started = r.started;
    let grant = r.grant;
    let feeder = std::thread::spawn(move || {
        for _ in 0..4 {
            started.recv_timeout(HANG).unwrap();
            grant.send(Outcome::Picture).unwrap();
        }
        (started, grant)
    });
    // Asks for 10; the media has 4.
    assert_eq!(r.ahead.preroll(0, 10, HANG), 4);
    let _channels = feeder.join().unwrap();
}

/// Dropping waits for the in-flight decode, and returns once it completes.
///
/// With a lookahead of one there is no queued work at all, so this isolates the
/// guarantee that `Drop` joins cleanly while a decode is mid-flight, whichever
/// of "stop" and "decode finished" the worker observes first.
#[test]
fn dropping_waits_for_the_in_flight_decode_and_then_returns() {
    let mut r = Rig::new(1000, cfg(1, 0));
    r.ahead.prepare(0, false);
    r.expect_start(0);
    let Rig {
        grant,
        started,
        ahead,
    } = r;
    let dropper = std::thread::spawn(move || drop(ahead));
    grant.send(Outcome::Picture).unwrap();
    dropper
        .join()
        .expect("drop must complete once the decode does");
    assert!(started.try_recv().is_err(), "nothing else was ever planned");
}

/// With queued work, `Drop` cannot hang and cannot run the queue.
///
/// Which of "stop" and "picture 0 finished" the worker sees first is a genuine
/// race, and the test does not pretend otherwise: the worker may begin *one*
/// more decode before it notices. What must hold regardless is that it begins no
/// more than one, and that tearing down the decoder releases it.
#[test]
fn dropping_with_queued_work_starts_at_most_one_more_decode() {
    let mut r = Rig::new(1000, cfg(10, 0));
    r.ahead.prepare(0, false);
    r.expect_start(0);
    let Rig {
        grant,
        started,
        ahead,
    } = r;
    let dropper = std::thread::spawn(move || drop(ahead));
    grant.send(Outcome::Picture).unwrap();
    // Tear the decoder down, as a real one would be, so a second in-flight
    // decode ends rather than waiting for a grant that will never come.
    drop(grant);
    dropper.join().expect("drop must not hang with work queued");
    let extra: Vec<usize> = started.try_iter().collect();
    assert!(
        extra.len() <= 1,
        "the queue ran after drop: decodes {extra:?} began"
    );
}

// ---------------------------------------------------------------------------
// With the scheduler
// ---------------------------------------------------------------------------

/// The scheduler over decode-ahead: an empty buffer is an underrun counted once
/// per stretch, a starved transport has no clock deadline (so a loop cannot
/// spin), and the picture is presented as soon as the worker delivers it.
#[test]
fn the_scheduler_reports_starvation_once_and_presents_when_the_picture_arrives() {
    let r = Rig::new(240, cfg(4, 0));
    let t = ManualTime::default();
    let clock = PlaybackClock::new(
        t.clone(),
        Timestamp::exact(0, Rate::F24),
        Timestamp::exact(240, Rate::F24),
    );
    let Rig {
        grant,
        started,
        ahead,
    } = r;
    let mut s = Scheduler::new(clock, ahead);
    s.clock_mut().play().unwrap();

    assert!(matches!(s.poll(), Presentation::Underrun));
    assert!(matches!(s.poll(), Presentation::Underrun));
    assert!(matches!(s.poll(), Presentation::Underrun));
    assert_eq!(
        s.stats().underrun_episodes,
        1,
        "one stall, however often polled"
    );
    assert_eq!(s.stats().starved_polls, 3);
    assert_eq!(s.clock().telemetry().underruns, 1);
    assert_eq!(s.state(), TransportState::Buffering);
    assert!(s.is_starved());
    assert_eq!(
        s.next_deadline(),
        None,
        "starved: wait on the source, not the clock"
    );

    assert_eq!(started.recv_timeout(HANG).unwrap(), 0);
    grant.send(Outcome::Picture).unwrap();
    assert!(s.wait_for_source(HANG));
    match s.poll() {
        Presentation::Present(p) => {
            assert_eq!(p.index, 0);
            assert_eq!(p.dropped, 0);
        }
        other => panic!("expected picture 0, got {other:?}"),
    }
    assert!(!s.is_starved());
    assert!(s.next_deadline().is_some(), "fed again: the clock drives");

    // Picture 1 decoded ahead: it is on hand before its time comes.
    assert_eq!(started.recv_timeout(HANG).unwrap(), 1);
    grant.send(Outcome::Picture).unwrap();
    assert!(s.source().wait_ready(1, HANG));
    t.advance(s.next_deadline().unwrap());
    assert!(matches!(s.poll(), Presentation::Present(ref p) if p.index == 1));
    assert_eq!(s.stats().underrun_episodes, 1);
    drop(grant);
}
