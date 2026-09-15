//! Playback driven against **real H.264 on disk**, on the real monotonic clock.
//!
//! `playback_scheduler.rs` proves the scheduler's decisions are right. This
//! proves they survive contact with a decoder, and measures what actually
//! happens: how long a picture takes to decode, whether 24 fps is held, and
//! what reverse costs. Nothing here asserts a performance figure — the numbers
//! are printed, because a threshold that passes on this machine would be a
//! claim about every machine.
//!
//! Skips loudly when the asset is absent.

use cine_media::clock::{PlaybackClock, PlaybackRate, SystemTime, TransportState};
use cine_media::schedule::{FrameSource, Presentation, Scheduler};
use cine_media::time::{Rate, Timestamp};
use cine_media::VideoReader;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn mp4() -> Option<PathBuf> {
    ["/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

fn wav() -> Option<PathBuf> {
    [
        "/usr/share/sounds/sound-icons/percussion-12.wav",
        "/usr/share/sounds/sound-icons/cembalo-11.wav",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}

macro_rules! need {
    ($opt:expr, $what:expr) => {
        match $opt {
            Some(p) => p,
            None => {
                eprintln!("SKIP: no {} available on this machine", $what);
                return;
            }
        }
    };
}

fn reader() -> Option<VideoReader> {
    VideoReader::open_path(&mp4()?).ok()
}

/// A scheduler over the real file, stopped at its first picture.
fn rig() -> Option<Scheduler<SystemTime, VideoReader>> {
    let r = reader()?;
    let (start, end) = r.extent()?;
    Some(Scheduler::new(
        PlaybackClock::new(SystemTime::default(), start, end),
        r,
    ))
}

// ---------------------------------------------------------------------------
// The reader as a schedulable source
// ---------------------------------------------------------------------------

/// The extent must come from the container's own sample durations, so the last
/// picture is shown for its real duration rather than vanishing at its own pts.
///
/// This file's first presentation time is **1024 ticks at a 12288 timescale**,
/// not zero — a two-frame composition offset, which is ordinary in a file with
/// B-frames. The reader reports the media's own times rather than sliding them
/// to zero, and the edit list that would normally compensate is not parsed. See
/// the gaps section of `PHASE-2-MEDIA-ENGINE.md`.
#[test]
fn the_presentation_extent_is_exact_and_covers_the_last_picture() {
    let r = need!(reader(), "real mp4");
    let (start, end) = r.extent().expect("a track with samples has an extent");
    assert_eq!(
        start,
        r.pts_at(0).unwrap(),
        "the extent starts at the first pts"
    );
    println!(
        "MEASURED extent: {start:?} .. {end:?} (timescale {:?})",
        r.time_base()
    );
    let last = r.pts_at(r.frame_count() - 1).unwrap();
    assert!(end > last, "the last picture must have a non-zero window");
    assert_eq!(FrameSource::frame_count(&r), 240);

    // 240 pictures at 24 fps is exactly 10 seconds of duration, wherever the
    // file chooses to start counting.
    let span = end.checked_sub(&start);
    let (in_frames, exact) = span.rescale_checked(Rate::F24, cine_media::time::Rounding::Nearest);
    assert!(exact, "the extent must span a whole number of frames");
    assert_eq!(in_frames.ticks(), 240);
}

/// Every playhead position inside a picture's window must select that picture,
/// probed at the exact boundary and at both sides of it, across the whole file.
#[test]
fn selection_is_exact_at_every_frame_boundary_of_a_real_file() {
    let r = need!(reader(), "real mp4");
    let base = r.time_base();
    for n in 0..FrameSource::frame_count(&r) {
        let at = r.pts_at(n).unwrap();
        assert_eq!(
            FrameSource::index_at(&r, at),
            Some(n),
            "on the boundary of {n}"
        );
        // One tick of the media's own timescale in is still the same picture.
        let inside = Timestamp::exact(at.ticks() + 1, base);
        assert_eq!(FrameSource::index_at(&r, inside), Some(n), "inside {n}");
        // One tick before it belongs to the previous picture.
        if n > 0 {
            let before = Timestamp::exact(at.ticks() - 1, base);
            assert_eq!(FrameSource::index_at(&r, before), Some(n - 1), "before {n}");
        }
    }
    // Before the first picture nothing is selected, and nothing is invented.
    let first = r.pts_at(0).unwrap();
    let before_all = Timestamp::exact(first.ticks() - 1, base);
    assert_eq!(FrameSource::index_at(&r, before_all), None);
}

// ---------------------------------------------------------------------------
// What decoding actually costs
// ---------------------------------------------------------------------------

/// Measure sequential decode throughput. Printed, not asserted: this is the
/// evidence for whether real-time playback is possible on this machine, and a
/// threshold here would be a claim about hardware nobody has measured.
#[test]
fn measure_sequential_decode_throughput() {
    let mut r = need!(reader(), "real mp4");
    let n = 120usize;
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    for i in 0..n {
        let one = Instant::now();
        let f = r.frame_at_index(i).expect("decode").expect("a picture");
        assert_eq!(f.pts, r.pts_at(i).unwrap());
        worst = worst.max(one.elapsed());
    }
    let total = started.elapsed();
    let per = total / n as u32;
    println!(
        "MEASURED sequential decode: {n} pictures in {total:?} — mean {per:?}, worst {worst:?}, \
         {:.1} fps",
        n as f64 / total.as_secs_f64()
    );
    println!(
        "         24 fps needs 41.667 ms per picture; this build {} it sequentially",
        if per < Duration::from_nanos(41_666_667) {
            "meets"
        } else {
            "does NOT meet"
        }
    );
}

/// Reverse costs a decode from the preceding keyframe for every picture. Measure
/// that cost rather than assuming it, because it is what decides whether reverse
/// can be offered at speed or only reported as degraded.
#[test]
fn measure_reverse_decode_cost() {
    let mut r = need!(reader(), "real mp4");
    let keyframes = r.keyframe_count();
    let n = 24usize;
    let start = 120usize;
    let began = Instant::now();
    for i in 0..n {
        r.frame_at_index(start - i)
            .expect("decode")
            .expect("picture");
    }
    let total = began.elapsed();
    println!(
        "MEASURED reverse decode: {n} pictures backwards in {total:?} — mean {:?} \
         ({keyframes} sync samples in {} pictures)",
        total / n as u32,
        FrameSource::frame_count(&r)
    );
}

// ---------------------------------------------------------------------------
// Real-time playback
// ---------------------------------------------------------------------------

/// Play the real file forwards on the real clock for a second, sleeping on the
/// scheduler's deadlines, and report what happened. The only assertions are the
/// ones that must hold whatever the hardware does: pictures arrive in order,
/// and the counters add up.
#[test]
fn plays_forward_on_the_real_clock_and_reports_honestly() {
    let mut s = need!(rig(), "real mp4");
    s.clock_mut().play().expect("a decodable file plays");

    let deadline = Instant::now() + Duration::from_secs(1);
    let mut order: Vec<usize> = Vec::new();
    let mut wakes = 0u32;
    while Instant::now() < deadline {
        match s.poll() {
            Presentation::Present(f) => order.push(f.index),
            Presentation::Hold => {}
            Presentation::EndOfMedia => break,
            other => panic!("real playback produced {other:?}"),
        }
        wakes += 1;
        match s.next_deadline() {
            Some(d) if d > Duration::ZERO => std::thread::sleep(d.min(Duration::from_millis(50))),
            _ => {}
        }
    }

    assert!(!order.is_empty(), "nothing was presented in a whole second");
    assert!(
        order.windows(2).all(|w| w[1] > w[0]),
        "pictures must be presented in increasing display order: {order:?}"
    );
    let stats = s.stats();
    assert_eq!(stats.presented, order.len() as u64);
    assert_eq!(stats.presented + stats.held, stats.polls);

    println!(
        "MEASURED 1 s of forward playback: {} presented, {} dropped, {} late, {wakes} wakes",
        stats.presented, stats.dropped, stats.late
    );
    println!(
        "         first {:?} last {:?}; mean decode {:?}, worst {:?}",
        order.first(),
        order.last(),
        stats.mean_decode(),
        Duration::from_nanos(stats.decode_worst_nanos as u64)
    );
    println!(
        "         drop rate {:.1}% — transport reported {:?}",
        stats.drop_rate() * 100.0,
        s.state()
    );
    // The one thing that must be true regardless of speed: if pictures were
    // skipped, the transport must say it is degraded rather than playing.
    if stats.dropped > 0 {
        assert_eq!(
            s.state(),
            TransportState::Degraded,
            "dropping frames while reporting {:?} would be a lie",
            s.state()
        );
    }
}

/// Half speed must present each picture for twice as long — the frame grid does
/// not change, the deadlines do.
///
/// The decode happens while paused so that the assertion measures the deadline
/// and not this machine's decoder, which is far too slow to leave any of a
/// frame's window intact (see `measure_sequential_decode_throughput`).
#[test]
fn half_speed_holds_each_picture_for_twice_as_long() {
    let mut s = need!(rig(), "real mp4");
    let at = s.source().pts_at(100).expect("frame 100");
    s.clock_mut().pause();
    s.seek(at);
    assert!(matches!(s.poll(), Presentation::Present(_)));

    s.clock_mut().play().unwrap();
    // Reading the real monotonic clock a second time costs a few hundred
    // nanoseconds, so this is a band rather than an equality. The exact
    // equality is asserted in `playback_scheduler.rs`, on a clock a test drives.
    let full = s.next_deadline().expect("playing");
    assert!(
        full <= Duration::from_nanos(41_666_667) && full > Duration::from_nanos(41_000_000),
        "one frame at 24 fps, got {full:?}"
    );

    s.clock_mut()
        .set_rate(PlaybackRate::new(1, 2).unwrap())
        .unwrap();
    let half = s.next_deadline().expect("playing");
    assert!(
        half <= Duration::from_nanos(83_333_334) && half > Duration::from_nanos(82_000_000),
        "half speed doubles the wall time and leaves the frame grid alone, got {half:?}"
    );
}

/// Reverse over real media. Whether it keeps up is measured, not claimed; what
/// is asserted is that it walks backwards and never fabricates a picture.
#[test]
fn plays_in_reverse_over_real_media_and_reports_what_it_costs() {
    let mut s = need!(rig(), "real mp4");
    let from = s.source().pts_at(180).expect("frame 180");
    s.seek(from);
    s.clock_mut()
        .set_rate(PlaybackRate::new(-1, 1).unwrap())
        .unwrap();
    s.clock_mut().play().unwrap();

    let began = Instant::now();
    let mut order = Vec::new();
    while order.len() < 12 && began.elapsed() < Duration::from_secs(20) {
        match s.poll() {
            Presentation::Present(f) => order.push(f.index),
            Presentation::Hold => {}
            Presentation::EndOfMedia => break,
            other => panic!("reverse produced {other:?}"),
        }
        if let Some(d) = s.next_deadline() {
            std::thread::sleep(d.min(Duration::from_millis(50)));
        }
    }
    assert!(!order.is_empty(), "reverse presented nothing");
    assert!(
        order.windows(2).all(|w| w[1] < w[0]),
        "reverse must walk backwards: {order:?}"
    );
    let stats = s.stats();
    println!(
        "MEASURED reverse playback: presented {:?}, {} dropped, mean decode {:?}, state {:?}",
        order,
        stats.dropped,
        stats.mean_decode(),
        s.state()
    );
    if stats.dropped > 0 {
        assert_eq!(s.state(), TransportState::Degraded);
    }
}

// ---------------------------------------------------------------------------
// Seeking, end of media, and media this build cannot play
// ---------------------------------------------------------------------------

/// Seeking to an exact picture must land on it, from a keyframe or from the
/// middle of a GOP, forwards or backwards, without being counted as drops.
#[test]
fn seeking_over_real_media_lands_on_the_exact_picture() {
    let mut s = need!(rig(), "real mp4");
    s.clock_mut().pause();
    for target in [0usize, 239, 1, 137, 138, 12, 200, 199] {
        let want = s.source().pts_at(target).expect("a real picture");
        s.seek(want);
        match s.poll() {
            Presentation::Present(f) => {
                assert_eq!(f.index, target);
                assert_eq!(
                    f.frame.pts, want,
                    "the decoded picture must be the one asked for, exactly"
                );
                assert_eq!(f.dropped, 0, "a seek is not a dropped frame");
            }
            Presentation::Hold => {}
            other => panic!("seek to {target} produced {other:?}"),
        }
    }
    assert_eq!(s.stats().dropped, 0);
    assert_eq!(s.clock().telemetry().frames_dropped, 0);
}

/// Playing off the end of a real file must stop, say so, and not spin.
#[test]
fn real_playback_stops_at_the_end_of_the_file() {
    let mut s = need!(rig(), "real mp4");
    let near_end = s.source().pts_at(238).expect("frame 238");
    s.seek(near_end);
    s.clock_mut().play().unwrap();
    // Generous, because this build's decoder is slow (see the measurement
    // tests): seeking near the end re-decodes from the preceding keyframe.
    let began = Instant::now();
    let mut ended = false;
    while began.elapsed() < Duration::from_secs(120) {
        match s.poll() {
            Presentation::EndOfMedia => {
                ended = true;
                break;
            }
            Presentation::Present(_) | Presentation::Hold => {}
            other => panic!("unexpected {other:?}"),
        }
        if let Some(d) = s.next_deadline() {
            std::thread::sleep(d.min(Duration::from_millis(50)));
        }
    }
    println!("MEASURED end-of-media reached after {:?}", began.elapsed());
    assert!(ended, "playback never reported the end of the file");
    assert_eq!(s.state(), TransportState::Paused);
    // And it stays stopped: the playhead does not move, there is nothing to
    // wake up for, and it does not silently loop back to the beginning.
    let held = s.position();
    assert_eq!(s.next_deadline(), None);
    s.poll();
    assert_eq!(s.position(), held);
    assert_eq!(s.stats().end_of_media, 1);
}

/// A file with no decodable video track must not produce a transport that
/// appears to work. The refusal happens at open, and the scheduler that is
/// built for it refuses every poll.
#[test]
fn media_without_a_decodable_video_track_is_refused_not_faked() {
    let path = need!(wav(), "a real wav");
    let err = match VideoReader::open_path(&path) {
        Err(e) => e,
        Ok(_) => panic!("a wav must not open as a video track"),
    };
    println!("MEASURED refusal for {}: {err}", path.display());

    // The transport still exists, so an interface can show *why* it will not
    // play — and it never reaches a decoder.
    let clock = PlaybackClock::unsupported(SystemTime::default(), Timestamp::exact(0, Rate::F24));
    let reader = need!(reader(), "real mp4");
    let mut s = Scheduler::new(clock, reader);
    assert!(matches!(s.poll(), Presentation::Unsupported));
    assert!(s.clock_mut().play().is_err());
    assert_eq!(s.next_deadline(), None);
    assert_eq!(s.stats().presented, 0);
}
