//! Live on-set metadata ingestion.
//!
//! Camera metadata arrives at up to 240 Hz. [`IngestBuffer`] is a bounded
//! ring built on a `crossbeam_channel` so any thread can push frames without
//! taking a lock while the graph side drains them in batches (see
//! `graph::write_take_metadata`). [`summarize`] reduces a take's frames to a
//! [`TakeMetadataSummary`] and [`detect_events`] finds zooms, aperture, ISO,
//! white-balance and focus changes with hysteresis so a single ramp yields a
//! single event.

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use ucg_core::RationalTime;
use ucg_schema::MetadataFrame;

/// Default ring capacity: one minute at 240 Hz.
pub const DEFAULT_CAPACITY: usize = 240 * 60;

/// Adjacent-frame focal jump (mm) that counts as a lens change rather than a zoom.
pub const LENS_CHANGE_MIN_JUMP_MM: f64 = 5.0;
/// Adjacent-frame relative focal jump that counts as a lens change.
pub const LENS_CHANGE_MIN_RATIO: f64 = 0.15;
/// Cumulative gradual focal travel (mm) that counts as a zoom.
pub const ZOOM_MIN_MM: f64 = 2.0;

/// Focal change detection: enter threshold (mm) and settle epsilon (mm).
pub const FOCAL_ENTER_MM: f64 = 1.0;
pub const FOCAL_SETTLE_MM: f64 = 0.05;
/// Aperture change detection in stops (2·log2 of the f-number ratio).
pub const APERTURE_ENTER_STOPS: f64 = 0.3;
pub const APERTURE_SETTLE_STOPS: f64 = 0.05;
/// ISO change detection in stops (log2 of the ISO ratio).
pub const ISO_ENTER_STOPS: f64 = 0.3;
pub const ISO_SETTLE_STOPS: f64 = 0.05;
/// White-balance change detection in kelvin.
pub const WB_ENTER_K: f64 = 150.0;
pub const WB_SETTLE_K: f64 = 10.0;
/// Focus-pull detection in metres.
pub const FOCUS_ENTER_M: f64 = 0.5;
pub const FOCUS_SETTLE_M: f64 = 0.02;
/// Consecutive stable samples required before a change is considered settled.
pub const SETTLE_FRAMES: usize = 12;

#[derive(Debug)]
struct Counters {
    pushed: AtomicU64,
    dropped: AtomicU64,
}

/// Bounded ring of metadata frames: pushing into a full ring discards the
/// oldest frame (and counts it) so a stalled consumer never blocks capture.
#[derive(Debug)]
pub struct IngestBuffer {
    tx: Sender<MetadataFrame>,
    rx: Receiver<MetadataFrame>,
    capacity: usize,
    counters: Arc<Counters>,
}

/// Convenience alias: the on-set ingest engine.
pub type Ingest = IngestBuffer;

/// A cloneable producer handle for other threads.
#[derive(Clone, Debug)]
pub struct IngestSource {
    tx: Sender<MetadataFrame>,
    rx: Receiver<MetadataFrame>,
    counters: Arc<Counters>,
}

fn ring_push(
    tx: &Sender<MetadataFrame>,
    rx: &Receiver<MetadataFrame>,
    counters: &Counters,
    mut frame: MetadataFrame,
) -> bool {
    counters.pushed.fetch_add(1, Ordering::Relaxed);
    let mut dropped_any = false;
    loop {
        match tx.try_send(frame) {
            Ok(()) => return !dropped_any,
            Err(TrySendError::Full(f)) => {
                frame = f;
                if rx.try_recv().is_ok() {
                    counters.dropped.fetch_add(1, Ordering::Relaxed);
                    dropped_any = true;
                }
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

impl IngestSource {
    /// Push a frame; returns `false` when an older frame had to be dropped.
    pub fn push(&self, frame: MetadataFrame) -> bool {
        ring_push(&self.tx, &self.rx, &self.counters, frame)
    }
}

impl Default for IngestBuffer {
    fn default() -> Self {
        IngestBuffer::new(DEFAULT_CAPACITY)
    }
}

impl IngestBuffer {
    /// A ring holding at most `capacity` frames (at least 1).
    pub fn new(capacity: usize) -> IngestBuffer {
        let capacity = capacity.max(1);
        let (tx, rx) = bounded(capacity);
        IngestBuffer {
            tx,
            rx,
            capacity,
            counters: Arc::new(Counters {
                pushed: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
            }),
        }
    }

    /// Push a frame; returns `false` when an older frame had to be dropped.
    pub fn push(&self, frame: MetadataFrame) -> bool {
        ring_push(&self.tx, &self.rx, &self.counters, frame)
    }

    /// Take every buffered frame, oldest first.
    pub fn drain(&self) -> Vec<MetadataFrame> {
        self.rx.try_iter().collect()
    }

    /// A producer handle usable from another thread.
    pub fn source(&self) -> IngestSource {
        IngestSource {
            tx: self.tx.clone(),
            rx: self.rx.clone(),
            counters: Arc::clone(&self.counters),
        }
    }

    /// Frames currently buffered.
    pub fn len(&self) -> usize {
        self.rx.len()
    }

    /// No frames buffered.
    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }

    /// Ring capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total frames pushed since creation.
    pub fn pushed(&self) -> u64 {
        self.counters.pushed.load(Ordering::Relaxed)
    }

    /// Frames discarded because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.counters.dropped.load(Ordering::Relaxed)
    }
}

/// Minimum and maximum of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MinMax {
    pub min: f64,
    pub max: f64,
}

impl MinMax {
    fn of(values: impl Iterator<Item = f64>) -> MinMax {
        let mut mm = MinMax {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        };
        for v in values {
            mm.min = mm.min.min(v);
            mm.max = mm.max.max(v);
        }
        if mm.min.is_infinite() {
            MinMax::default()
        } else {
            mm
        }
    }

    /// `max - min`.
    pub fn range(&self) -> f64 {
        self.max - self.min
    }
}

/// Minimum, maximum and most frequent value of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MinMaxMode {
    pub min: f64,
    pub max: f64,
    pub mode: f64,
}

/// Most frequent value (to 1/1000 resolution); ties go to the smaller value.
pub fn mode(values: impl Iterator<Item = f64>) -> f64 {
    let mut counts: BTreeMap<i64, (f64, usize)> = BTreeMap::new();
    for v in values {
        if !v.is_finite() {
            continue;
        }
        let e = counts.entry((v * 1000.0).round() as i64).or_insert((v, 0));
        e.1 += 1;
    }
    counts
        .values()
        .fold(
            (0.0, 0usize),
            |best, &(v, n)| if n > best.1 { (v, n) } else { best },
        )
        .0
}

/// Reduction of one take's frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TakeMetadataSummary {
    pub frame_count: usize,
    pub first_tc: i64,
    pub last_tc: i64,
    pub iso: MinMaxMode,
    pub shutter_angle_mode: f64,
    pub focal_length: MinMax,
    /// A gradual focal-length travel of at least [`ZOOM_MIN_MM`] occurred.
    pub has_zoom: bool,
    pub f_stop: MinMax,
    pub focus_distance: MinMax,
    pub white_balance_mode: f64,
    /// Abrupt focal-length jumps (lens swaps while rolling / spliced takes).
    pub lens_changes: usize,
}

/// Summarise a take. Frames are used in the order given.
pub fn summarize(frames: &[MetadataFrame]) -> TakeMetadataSummary {
    if frames.is_empty() {
        return TakeMetadataSummary::default();
    }
    let iso = MinMax::of(frames.iter().map(|f| f.iso));
    let mut lens_changes = 0;
    let mut has_zoom = false;
    let mut run: f64 = 0.0;
    for w in frames.windows(2) {
        let (a, b) = (w[0].focal_length_mm, w[1].focal_length_mm);
        let delta = b - a;
        let ratio = if a.abs() > f64::EPSILON {
            (delta / a).abs()
        } else {
            0.0
        };
        if delta.abs() >= LENS_CHANGE_MIN_JUMP_MM && ratio >= LENS_CHANGE_MIN_RATIO {
            lens_changes += 1;
            run = 0.0;
        } else {
            // Direction reversals restart the run so wobble does not accumulate.
            if (run > 0.0 && delta < 0.0) || (run < 0.0 && delta > 0.0) {
                run = 0.0;
            }
            run += delta;
            if run.abs() >= ZOOM_MIN_MM {
                has_zoom = true;
            }
        }
    }
    TakeMetadataSummary {
        frame_count: frames.len(),
        first_tc: frames[0].timecode_frames,
        last_tc: frames[frames.len() - 1].timecode_frames,
        iso: MinMaxMode {
            min: iso.min,
            max: iso.max,
            mode: mode(frames.iter().map(|f| f.iso)),
        },
        shutter_angle_mode: mode(frames.iter().map(|f| f.shutter_angle_deg)),
        focal_length: MinMax::of(frames.iter().map(|f| f.focal_length_mm)),
        has_zoom,
        f_stop: MinMax::of(frames.iter().map(|f| f.f_stop)),
        focus_distance: MinMax::of(frames.iter().map(|f| f.focus_distance_m)),
        white_balance_mode: mode(frames.iter().map(|f| f.white_balance_k)),
        lens_changes,
    }
}

/// What changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataEventKind {
    FocalChange,
    ApertureChange,
    IsoChange,
    WbChange,
    FocusPull,
}

/// A settled change in one metadata channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetadataEvent {
    /// Timecode (frames) at which the change began.
    pub at_tc: i64,
    pub kind: MetadataEventKind,
    /// Value before the change.
    pub from: f64,
    /// Value once the channel settled.
    pub to: f64,
}

/// Hysteresis tracker for one channel: a change is reported once the value
/// has left the baseline by more than `enter` (in the channel's distance
/// metric) and then held still (within `settle`) for [`SETTLE_FRAMES`].
struct ChannelTracker {
    kind: MetadataEventKind,
    enter: f64,
    settle: f64,
    distance: fn(f64, f64) -> f64,
    baseline: f64,
    changing_since: Option<(i64, f64)>,
    last: f64,
    stable_run: usize,
}

impl ChannelTracker {
    fn new(
        kind: MetadataEventKind,
        enter: f64,
        settle: f64,
        distance: fn(f64, f64) -> f64,
        first: f64,
    ) -> Self {
        ChannelTracker {
            kind,
            enter,
            settle,
            distance,
            baseline: first,
            changing_since: None,
            last: first,
            stable_run: 0,
        }
    }

    fn feed(&mut self, tc: i64, value: f64, out: &mut Vec<MetadataEvent>) {
        if (self.distance)(self.last, value) <= self.settle {
            self.stable_run += 1;
        } else {
            self.stable_run = 0;
        }
        self.last = value;
        match self.changing_since {
            None => {
                if (self.distance)(self.baseline, value) > self.enter {
                    self.changing_since = Some((tc, self.baseline));
                    self.stable_run = 0;
                }
            }
            Some((start_tc, from)) => {
                if self.stable_run >= SETTLE_FRAMES {
                    if (self.distance)(from, value) > self.enter {
                        out.push(MetadataEvent {
                            at_tc: start_tc,
                            kind: self.kind,
                            from,
                            to: value,
                        });
                    }
                    self.baseline = value;
                    self.changing_since = None;
                }
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<MetadataEvent>) {
        if let Some((start_tc, from)) = self.changing_since.take() {
            if (self.distance)(from, self.last) > self.enter {
                out.push(MetadataEvent {
                    at_tc: start_tc,
                    kind: self.kind,
                    from,
                    to: self.last,
                });
            }
        }
    }
}

fn abs_distance(a: f64, b: f64) -> f64 {
    (a - b).abs()
}

fn aperture_stops(a: f64, b: f64) -> f64 {
    if a <= 0.0 || b <= 0.0 {
        return 0.0;
    }
    (2.0 * (b / a).log2()).abs()
}

fn iso_stops(a: f64, b: f64) -> f64 {
    if a <= 0.0 || b <= 0.0 {
        return 0.0;
    }
    (b / a).log2().abs()
}

/// Detect settled changes in focal length, aperture, ISO, white balance and
/// focus distance. Events are ordered by timecode, then by kind.
pub fn detect_events(frames: &[MetadataFrame]) -> Vec<MetadataEvent> {
    let Some(first) = frames.first() else {
        return Vec::new();
    };
    let mut trackers = [
        ChannelTracker::new(
            MetadataEventKind::FocalChange,
            FOCAL_ENTER_MM,
            FOCAL_SETTLE_MM,
            abs_distance,
            first.focal_length_mm,
        ),
        ChannelTracker::new(
            MetadataEventKind::ApertureChange,
            APERTURE_ENTER_STOPS,
            APERTURE_SETTLE_STOPS,
            aperture_stops,
            first.f_stop,
        ),
        ChannelTracker::new(
            MetadataEventKind::IsoChange,
            ISO_ENTER_STOPS,
            ISO_SETTLE_STOPS,
            iso_stops,
            first.iso,
        ),
        ChannelTracker::new(
            MetadataEventKind::WbChange,
            WB_ENTER_K,
            WB_SETTLE_K,
            abs_distance,
            first.white_balance_k,
        ),
        ChannelTracker::new(
            MetadataEventKind::FocusPull,
            FOCUS_ENTER_M,
            FOCUS_SETTLE_M,
            abs_distance,
            first.focus_distance_m,
        ),
    ];
    let mut out = Vec::new();
    for f in frames {
        let values = [
            f.focal_length_mm,
            f.f_stop,
            f.iso,
            f.white_balance_k,
            f.focus_distance_m,
        ];
        for (t, v) in trackers.iter_mut().zip(values) {
            t.feed(f.timecode_frames, v, &mut out);
        }
    }
    for t in &mut trackers {
        t.finish(&mut out);
    }
    out.sort_by(|a, b| {
        a.at_tc
            .cmp(&b.at_tc)
            .then_with(|| kind_rank(a.kind).cmp(&kind_rank(b.kind)))
    });
    out
}

fn kind_rank(k: MetadataEventKind) -> u8 {
    match k {
        MetadataEventKind::FocalChange => 0,
        MetadataEventKind::ApertureChange => 1,
        MetadataEventKind::IsoChange => 2,
        MetadataEventKind::WbChange => 3,
        MetadataEventKind::FocusPull => 4,
    }
}

/// SMPTE-style timecode string for a frame count at `fps`.
pub fn timecode_string(frames: i64, fps: f64) -> String {
    RationalTime::from_frames(frames, fps).to_timecode()
}

/// A gradual change between two times.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ramp {
    pub start_s: f64,
    pub end_s: f64,
    pub to: f64,
}

/// An instantaneous change.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub at_s: f64,
    pub to: f64,
}

/// What happens during a simulated take.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeScenario {
    pub base: MetadataFrame,
    pub zoom: Option<Ramp>,
    pub focus_pull: Option<Ramp>,
    pub aperture_change: Option<Step>,
    pub iso_change: Option<Step>,
    pub wb_change: Option<Step>,
    /// Peak amplitude of deterministic focus-encoder noise, metres.
    pub focus_jitter_m: f64,
}

impl Default for TakeScenario {
    fn default() -> Self {
        TakeScenario {
            base: MetadataFrame {
                timecode_frames: 0,
                iso: 800.0,
                shutter_angle_deg: 180.0,
                focal_length_mm: 35.0,
                f_stop: 2.8,
                focus_distance_m: 3.0,
                white_balance_k: 5600.0,
                tint: 0.0,
            },
            zoom: None,
            focus_pull: None,
            aperture_change: None,
            iso_change: None,
            wb_change: None,
            focus_jitter_m: 0.0,
        }
    }
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ramp(from: f64, r: Option<Ramp>, t: f64) -> f64 {
    match r {
        None => from,
        Some(r) => {
            if t < r.start_s {
                from
            } else if t >= r.end_s || r.end_s <= r.start_s {
                r.to
            } else {
                from + (r.to - from) * smoothstep((t - r.start_s) / (r.end_s - r.start_s))
            }
        }
    }
}

fn step(from: f64, s: Option<Step>, t: f64) -> f64 {
    match s {
        Some(s) if t >= s.at_s => s.to,
        _ => from,
    }
}

/// Generate `seconds × fps` frames following `scenario` (deterministic).
pub fn simulate_take(seconds: f64, fps: f64, scenario: &TakeScenario) -> Vec<MetadataFrame> {
    let n = (seconds.max(0.0) * fps.max(0.0)).round() as usize;
    let mut noise: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..n)
        .map(|i| {
            let t = i as f64 / fps;
            // xorshift for repeatable jitter in [-1, 1].
            noise ^= noise << 13;
            noise ^= noise >> 7;
            noise ^= noise << 17;
            let jitter = ((noise % 2001) as f64 / 1000.0 - 1.0) * scenario.focus_jitter_m;
            let b = scenario.base;
            MetadataFrame {
                timecode_frames: b.timecode_frames + i as i64,
                iso: step(b.iso, scenario.iso_change, t),
                shutter_angle_deg: b.shutter_angle_deg,
                focal_length_mm: ramp(b.focal_length_mm, scenario.zoom, t),
                f_stop: step(b.f_stop, scenario.aperture_change, t),
                focus_distance_m: ramp(b.focus_distance_m, scenario.focus_pull, t) + jitter,
                white_balance_k: step(b.white_balance_k, scenario.wb_change, t),
                tint: b.tint,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario() -> TakeScenario {
        TakeScenario {
            zoom: Some(Ramp {
                start_s: 2.0,
                end_s: 4.0,
                to: 70.0,
            }),
            aperture_change: Some(Step { at_s: 6.0, to: 4.0 }),
            focus_jitter_m: 0.005,
            ..Default::default()
        }
    }

    #[test]
    fn zoom_and_aperture_events_fire_exactly_once() {
        let frames = simulate_take(10.0, 24.0, &scenario());
        assert_eq!(frames.len(), 240);
        let events = detect_events(&frames);
        let focal: Vec<&MetadataEvent> = events
            .iter()
            .filter(|e| e.kind == MetadataEventKind::FocalChange)
            .collect();
        let ap: Vec<&MetadataEvent> = events
            .iter()
            .filter(|e| e.kind == MetadataEventKind::ApertureChange)
            .collect();
        assert_eq!(focal.len(), 1, "{events:?}");
        assert_eq!(ap.len(), 1, "{events:?}");
        assert_eq!(events.len(), 2, "{events:?}");
        assert!((focal[0].from - 35.0).abs() < 1e-9);
        assert!((focal[0].to - 70.0).abs() < 1e-9);
        assert!(
            focal[0].at_tc >= 48 && focal[0].at_tc < 96,
            "{}",
            focal[0].at_tc
        );
        assert!((ap[0].from - 2.8).abs() < 1e-9);
        assert!((ap[0].to - 4.0).abs() < 1e-9);
        assert_eq!(ap[0].at_tc, 144);
        assert_eq!(timecode_string(ap[0].at_tc, 24.0), "00:00:06:00");
    }

    #[test]
    fn summary_min_max_mode() {
        let frames = simulate_take(10.0, 24.0, &scenario());
        let s = summarize(&frames);
        assert_eq!(s.frame_count, 240);
        assert_eq!(s.first_tc, 0);
        assert_eq!(s.last_tc, 239);
        assert!((s.focal_length.min - 35.0).abs() < 1e-9);
        assert!((s.focal_length.max - 70.0).abs() < 1e-9);
        assert!(s.has_zoom);
        assert_eq!(s.lens_changes, 0);
        assert!((s.f_stop.min - 2.8).abs() < 1e-9);
        assert!((s.f_stop.max - 4.0).abs() < 1e-9);
        assert!((s.iso.mode - 800.0).abs() < 1e-9);
        assert!((s.shutter_angle_mode - 180.0).abs() < 1e-9);
        assert!((s.white_balance_mode - 5600.0).abs() < 1e-9);
        assert!(s.focus_distance.min >= 2.99 && s.focus_distance.max <= 3.01);
        assert_eq!(summarize(&[]), TakeMetadataSummary::default());
    }

    #[test]
    fn lens_swap_is_not_a_zoom() {
        let mut frames = simulate_take(4.0, 24.0, &TakeScenario::default());
        for f in frames.iter_mut().skip(48) {
            f.focal_length_mm = 85.0;
        }
        let s = summarize(&frames);
        assert_eq!(s.lens_changes, 1);
        assert!(!s.has_zoom);
        let events = detect_events(&frames);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, MetadataEventKind::FocalChange);
        assert_eq!(events[0].at_tc, 48);
    }

    #[test]
    fn iso_wb_focus_events() {
        let sc = TakeScenario {
            iso_change: Some(Step {
                at_s: 1.0,
                to: 1600.0,
            }),
            wb_change: Some(Step {
                at_s: 2.0,
                to: 3200.0,
            }),
            focus_pull: Some(Ramp {
                start_s: 3.0,
                end_s: 4.0,
                to: 1.0,
            }),
            ..Default::default()
        };
        let frames = simulate_take(6.0, 24.0, &sc);
        let events = detect_events(&frames);
        let kinds: Vec<MetadataEventKind> = events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                MetadataEventKind::IsoChange,
                MetadataEventKind::WbChange,
                MetadataEventKind::FocusPull
            ]
        );
        // Jitter alone must not fire anything.
        let quiet = simulate_take(
            5.0,
            24.0,
            &TakeScenario {
                focus_jitter_m: 0.01,
                ..Default::default()
            },
        );
        assert!(detect_events(&quiet).is_empty());
    }

    #[test]
    fn unsettled_change_at_end_of_take_is_still_reported() {
        let sc = TakeScenario {
            zoom: Some(Ramp {
                start_s: 1.0,
                end_s: 3.0,
                to: 100.0,
            }),
            ..Default::default()
        };
        let frames = simulate_take(2.0, 24.0, &sc);
        let events = detect_events(&frames);
        assert_eq!(events.len(), 1);
        assert!(events[0].to > 35.0 && events[0].to < 100.0);
    }

    #[test]
    fn ring_push_drain_at_240hz_loses_nothing() {
        let buf = IngestBuffer::new(4096);
        let frames = simulate_take(10.0, 240.0, &scenario());
        assert_eq!(frames.len(), 2400);
        let mut got = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            assert!(buf.push(*f));
            if i % 500 == 499 {
                got.extend(buf.drain());
            }
        }
        got.extend(buf.drain());
        assert_eq!(got.len(), 2400);
        assert_eq!(got, frames);
        assert_eq!(buf.dropped(), 0);
        assert_eq!(buf.pushed(), 2400);
        assert!(buf.is_empty());
    }

    #[test]
    fn threaded_producer_and_batched_consumer() {
        let buf = IngestBuffer::new(DEFAULT_CAPACITY);
        let frames = simulate_take(10.0, 240.0, &scenario());
        let src = buf.source();
        let producer = {
            let frames = frames.clone();
            std::thread::spawn(move || {
                for f in frames {
                    src.push(f);
                }
            })
        };
        let mut got = Vec::new();
        while got.len() < frames.len() {
            got.extend(buf.drain());
            std::thread::yield_now();
        }
        producer.join().expect("producer thread panicked");
        got.extend(buf.drain());
        assert_eq!(got, frames);
    }

    #[test]
    fn overflow_drops_oldest() {
        let buf = IngestBuffer::new(4);
        for i in 0..6 {
            let dropped_none = buf.push(MetadataFrame {
                timecode_frames: i,
                ..Default::default()
            });
            assert_eq!(dropped_none, i < 4);
        }
        let got: Vec<i64> = buf.drain().iter().map(|f| f.timecode_frames).collect();
        assert_eq!(got, vec![2, 3, 4, 5]);
        assert_eq!(buf.dropped(), 2);
        assert_eq!(buf.capacity(), 4);
    }

    #[test]
    fn mode_prefers_smaller_on_ties() {
        assert_eq!(mode([3.0, 1.0, 3.0, 1.0].into_iter()), 1.0);
        assert_eq!(mode([2.0, 2.0, 1.0].into_iter()), 2.0);
        assert_eq!(mode(std::iter::empty()), 0.0);
    }
}
