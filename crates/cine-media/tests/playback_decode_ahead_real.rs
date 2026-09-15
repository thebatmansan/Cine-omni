//! Decode-ahead against **real H.264 on disk**, on the real monotonic clock.
//!
//! `decode_ahead.rs` proves the worker's decisions. This measures whether they
//! matter: the same full-clip playback synchronously and with decode-ahead, and
//! the same for reverse, seeking and cancellation.
//!
//! As with every performance test in this crate, numbers are printed and speed
//! is never asserted. What *is* asserted is correctness under concurrency:
//! every picture presented is the exact picture for its index, forward runs are
//! strictly increasing, reverse runs strictly decreasing, and the transport says
//! `Degraded` whenever it dropped anything.
//!
//! **Run these on a quiet machine.** A benchmark under contention measures the
//! contention. The workspace test suite runs them with everything else; the
//! figures in `PLAYBACK.md` come from runs with nothing else building.

use cine_media::clock::{PlaybackClock, PlaybackRate, SystemTime, TransportState};
use cine_media::decode_ahead::{AheadConfig, DecodeAhead};
use cine_media::schedule::{FrameSource, Presentation, ScheduleStats, Scheduler};
use cine_media::VideoReader;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const REFERENCE: &str = "/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4";
const SECOND: &str = "/home/finstein-emp/Downloads/A_solitary_monochrome_crimson.mp4";

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

fn open(path: &str) -> Option<VideoReader> {
    let p = PathBuf::from(path);
    p.exists()
        .then(|| VideoReader::open_path(&p).ok())
        .flatten()
}

struct Run {
    order: Vec<usize>,
    stats: ScheduleStats,
    state: TransportState,
    wall: Duration,
}

/// Play until the end of the media, or `limit`, sleeping on the scheduler's
/// deadline and waiting on the source when starved. Checks every presented
/// picture against the table as it goes.
fn play<F: FrameSource>(s: &mut Scheduler<SystemTime, F>, limit: Duration, max: usize) -> Run {
    let began = Instant::now();
    let mut order = Vec::new();
    while began.elapsed() < limit && order.len() < max {
        match s.poll() {
            Presentation::Present(p) => {
                assert_eq!(
                    Some(p.frame.pts),
                    s.source().pts_of(p.index),
                    "picture {} is not the picture for its index",
                    p.index
                );
                order.push(p.index);
            }
            Presentation::Hold | Presentation::Underrun => {}
            Presentation::EndOfMedia => break,
            other => panic!("playback produced {other:?}"),
        }
        if s.is_starved() {
            s.wait_for_source(Duration::from_millis(250));
        } else if let Some(d) = s.next_deadline() {
            std::thread::sleep(d.min(Duration::from_millis(50)));
        }
    }
    Run {
        order,
        stats: s.stats(),
        state: s.state(),
        wall: began.elapsed(),
    }
}

fn describe(label: &str, r: &Run) {
    println!(
        "MEASURED {label}: {} presented, {} dropped, {} late, {} underrun episodes \
         ({} starved polls) in {:?}; drop rate {:.1}%, final state {:?}",
        r.stats.presented,
        r.stats.dropped,
        r.stats.late,
        r.stats.underrun_episodes,
        r.stats.starved_polls,
        r.wall,
        r.stats.drop_rate() * 100.0,
        r.state
    );
}

fn honest(r: &Run) {
    if r.stats.dropped > 0 {
        assert!(
            matches!(r.state, TransportState::Degraded | TransportState::Paused),
            "dropped {} pictures while reporting {:?}",
            r.stats.dropped,
            r.state
        );
    }
}

// ---------------------------------------------------------------------------
// What the assets actually are
// ---------------------------------------------------------------------------

/// The benchmarks below are only as general as the media they run on. This
/// records it, so a figure is never quoted without its GOP structure.
#[test]
fn measure_gop_structure_of_the_available_assets() {
    let mut any = false;
    for path in [REFERENCE, SECOND] {
        let Some(r) = open(path) else {
            eprintln!("SKIP: {path} absent or undecodable");
            continue;
        };
        any = true;
        let keys = r.keyframe_indices();
        assert!(
            !keys.is_empty(),
            "a decodable stream has at least one entry point"
        );
        let longest = keys
            .windows(2)
            .map(|w| w[1] - w[0])
            .chain(std::iter::once(r.frame_count() - keys[keys.len() - 1]))
            .max()
            .unwrap_or(r.frame_count());
        println!(
            "MEASURED {path}: {} pictures, keyframes at {keys:?}, longest GOP {longest}, \
             reordering {}",
            r.frame_count(),
            r.has_reordering()
        );
    }
    if !any {
        eprintln!("SKIP: no real mp4 available");
    }
}

// ---------------------------------------------------------------------------
// Forward: the whole clip, synchronous against decode-ahead
// ---------------------------------------------------------------------------

#[test]
fn measure_full_clip_forward_synchronous_against_decode_ahead() {
    let reader = need!(open(REFERENCE), "reference mp4");
    let (start, end) = reader.extent().unwrap();
    let count = reader.frame_count();

    // Synchronous: the baseline this work set out to beat.
    let mut sync = Scheduler::new(
        PlaybackClock::new(SystemTime::default(), start, end),
        reader,
    );
    sync.clock_mut().play().unwrap();
    let a = play(&mut sync, Duration::from_secs(30), usize::MAX);
    describe("full clip, synchronous", &a);
    assert!(a.order.windows(2).all(|w| w[1] > w[0]));
    honest(&a);
    drop(sync);

    for preroll in [0usize, 24, 48] {
        let reader = open(REFERENCE).unwrap();
        let ahead = DecodeAhead::new(reader, AheadConfig::default());
        let mut s = Scheduler::new(PlaybackClock::new(SystemTime::default(), start, end), ahead);
        s.source_mut().prepare(0, false);
        let rolled = Instant::now();
        let got = s.source().preroll(0, preroll, Duration::from_secs(30));
        let rolled = rolled.elapsed();
        s.clock_mut().play().unwrap();
        let r = play(&mut s, Duration::from_secs(30), usize::MAX);
        let ahead_stats = s.source().stats();
        describe(&format!("full clip, decode-ahead, preroll {preroll}"), &r);
        println!(
            "         preroll {got} pictures in {rolled:?}; worker decoded {} (mean {:?}), \
             {} stale, peak {:.1} MiB, {} hits / {} misses",
            ahead_stats.decoded,
            ahead_stats.mean_decode(),
            ahead_stats.stale_discarded,
            ahead_stats.peak_bytes as f64 / (1024.0 * 1024.0),
            ahead_stats.hits,
            ahead_stats.misses
        );
        assert!(r.order.windows(2).all(|w| w[1] > w[0]), "{:?}", r.order);
        assert!(r.stats.presented as usize <= count);
        honest(&r);
    }
}

/// Sustained: the clip played three times back to back on one worker, so the
/// figure is not flattered by a single short run.
#[test]
fn measure_sustained_forward_playback_three_passes() {
    let reader = need!(open(REFERENCE), "reference mp4");
    let (start, end) = reader.extent().unwrap();
    let ahead = DecodeAhead::new(reader, AheadConfig::default());
    let mut s = Scheduler::new(PlaybackClock::new(SystemTime::default(), start, end), ahead);
    let mut total = ScheduleStats::default();
    for pass in 1..=3 {
        s.seek(start);
        s.source_mut().prepare(0, false);
        s.source().preroll(0, 24, Duration::from_secs(30));
        s.clock_mut().play().unwrap();
        let before = s.stats();
        let r = play(&mut s, Duration::from_secs(30), usize::MAX);
        let presented = r.stats.presented - before.presented;
        let dropped = r.stats.dropped - before.dropped;
        println!("MEASURED sustained pass {pass}: {presented} presented, {dropped} dropped");
        total = r.stats;
        honest(&r);
    }
    println!(
        "MEASURED sustained total: {} presented, {} dropped ({:.1}%), {} underrun episodes; \
         worker peak {:.1} MiB",
        total.presented,
        total.dropped,
        total.drop_rate() * 100.0,
        total.underrun_episodes,
        s.source().stats().peak_bytes as f64 / (1024.0 * 1024.0)
    );
}

// ---------------------------------------------------------------------------
// Reverse
// ---------------------------------------------------------------------------

/// Reverse over the last GOP. Synchronously this cost 8.47 s per picture; with a
/// keyframe-aligned chunk the whole GOP is one walk.
#[test]
fn measure_reverse_with_decode_ahead() {
    let reader = need!(open(REFERENCE), "reference mp4");
    let (start, end) = reader.extent().unwrap();
    let keys = reader.keyframe_indices().to_vec();
    let from = reader.frame_count() - 1;
    let from_pts = reader.pts_at(from).unwrap();
    let ahead = DecodeAhead::new(reader, AheadConfig::default());
    let mut s = Scheduler::new(PlaybackClock::new(SystemTime::default(), start, end), ahead);
    s.seek(from_pts);
    s.clock_mut()
        .set_rate(PlaybackRate::new(-1, 1).unwrap())
        .unwrap();
    s.source_mut().prepare(from, true);
    let rolled = Instant::now();
    let got = s.source().preroll(from, 48, Duration::from_secs(60));
    let rolled = rolled.elapsed();
    s.clock_mut().play().unwrap();
    let r = play(&mut s, Duration::from_secs(20), 240);
    let a = s.source().stats();
    describe("reverse from the last picture, decode-ahead", &r);
    println!(
        "         keyframes {keys:?}; preroll {got} pictures in {rolled:?}; worker decoded {} \
         (mean {:?}), {} stale, peak {:.1} MiB",
        a.decoded,
        a.mean_decode(),
        a.stale_discarded,
        a.peak_bytes as f64 / (1024.0 * 1024.0)
    );
    if r.stats.presented > 0 {
        println!(
            "         wall time per presented picture: {:?}",
            (rolled + r.wall) / r.stats.presented as u32
        );
    }
    assert!(!r.order.is_empty(), "reverse presented nothing");
    assert!(r.order.windows(2).all(|w| w[1] < w[0]), "{:?}", r.order);
    honest(&r);
}

// ---------------------------------------------------------------------------
// Seeking and cancellation
// ---------------------------------------------------------------------------

/// Time to first picture after a seek: cold into the middle of a GOP, warm into
/// what is already buffered, and a far seek while the worker is busy — which is
/// where the one-decode-call cancellation granularity shows up.
#[test]
fn measure_seek_latency_and_cancellation_cost() {
    let reader = need!(open(REFERENCE), "reference mp4");
    let keys = reader.keyframe_indices().to_vec();
    let mut ahead = DecodeAhead::new(reader, AheadConfig::default());

    let t = Instant::now();
    ahead.prepare(110, false);
    assert!(ahead.wait_ready(110, Duration::from_secs(60)));
    let cold = t.elapsed();

    // Let the lookahead fill a little, then seek inside it.
    ahead.wait_ready(114, Duration::from_secs(60));
    let t = Instant::now();
    ahead.prepare(113, false);
    let warm_ready = ahead.decode_index(113).unwrap().is_some();
    let warm = t.elapsed();

    // A far seek issued shortly after another far seek. Whether the worker is
    // mid-walk towards 235 when the second arrives is likely but not proven —
    // nothing here can observe it — so the figure is labelled as what it is.
    ahead.prepare(235, false);
    std::thread::sleep(Duration::from_millis(5));
    let t = Instant::now();
    ahead.prepare(5, false);
    assert!(ahead.wait_ready(5, Duration::from_secs(60)));
    let cancelled = t.elapsed();
    let stats = ahead.stats();

    println!("MEASURED seek (keyframes {keys:?}):");
    println!("         cold into mid-GOP picture 110: {cold:?}");
    println!(
        "         warm within the buffer, picture 113: {warm:?} (already decoded: {warm_ready})"
    );
    println!(
        "         seek to 5 issued ~5 ms after a seek to 235: {cancelled:?}, {} stale decodes discarded",
        stats.stale_discarded
    );
    assert!(
        warm_ready,
        "a seek inside the buffer must not need a decode"
    );
}
