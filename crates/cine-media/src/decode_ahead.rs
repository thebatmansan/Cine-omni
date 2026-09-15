//! Decode-ahead: decoding off the presentation thread, into a bounded buffer.
//!
//! ## The measured problem
//!
//! With decoding on the scheduler's thread, every presented picture paid for
//! its own decode at the moment it was due. On the reference asset that decode
//! averages 45.7 ms against a 41.7 ms frame, so the playhead ran ahead, the next
//! request was further away, and on long-GOP media the reader had to walk every
//! intervening picture to reach it. Catching up by skipping collapsed instead of
//! converging. See `docs/blueprint/PLAYBACK.md`.
//!
//! ## What this does
//!
//! A worker thread owns the decoder and decodes in the direction of travel into
//! a buffer the scheduler reads without blocking. `DecodeAhead` is itself a
//! [`FrameSource`], so the scheduler is unchanged: it asks for a picture, and
//! either gets one that is already decoded or is told "not ready", which it
//! reports as an underrun while the playhead keeps moving.
//!
//! ```text
//!   Scheduler ──decode_index(i)──▶ buffer ◀──insert── worker ──▶ decoder
//!       │                            ▲                   ▲
//!       └──prepare(i, direction)─────┴──── playhead ─────┘
//! ```
//!
//! ## What this cannot do, stated up front
//!
//! **It does not make a decoder faster.** A decoder that produces 21.9 pictures
//! a second cannot sustain 24 by being moved to another thread. What decode-ahead
//! buys is that presentation never waits on a decode, that a short deficit can be
//! absorbed by a pre-roll, and that reverse can decode a whole chunk per seek
//! instead of a whole GOP walk per picture. Whether that is enough is a
//! measurement, and the tests in `playback_real_media.rs` make it.
//!
//! **Cancellation is one decode call wide.** The worker re-plans between
//! decodes, so a seek abandons everything queued at once — but a decode already
//! in progress runs to completion, and on long-GOP media one `decode_index` can
//! be a walk from a keyframe. Its result is then discarded if it no longer
//! belongs to the playhead's neighbourhood, and counted as stale.
//!
//! ## Memory
//!
//! Bounded by bytes, not by picture count, for the same reason the frame cache
//! is: a 4K picture is roughly nine times a 720p one. The picture at the playhead
//! is always admitted even over budget — refusing it would deadlock a consumer
//! waiting for exactly that picture — and room is made by evicting the picture
//! farthest ahead.

use crate::error::{Error, Result};
use crate::frame::VideoFrame;
use crate::schedule::FrameSource;
use parking_lot::{Condvar, Mutex};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use ucg_core::RationalTime;

/// How much to decode ahead, and how much memory it may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AheadConfig {
    /// Pictures to hold ahead of the playhead in the direction of travel.
    pub ahead: usize,
    /// Pictures to keep on the side already shown, so a small step back is free.
    pub behind: usize,
    /// Ceiling on decoded bytes held. Wins over `ahead`, except for the picture
    /// at the playhead itself.
    pub max_bytes: usize,
    /// Pictures decoded per chunk when playing backwards **and the source cannot
    /// say where its keyframes are**. When it can, a reverse chunk runs from the
    /// keyframe to the playhead — every picture in it costs the same single walk
    /// — limited only by `max_bytes`.
    pub reverse_chunk: usize,
}

impl Default for AheadConfig {
    fn default() -> Self {
        AheadConfig {
            ahead: 48,
            behind: 4,
            max_bytes: 512 * 1024 * 1024,
            reverse_chunk: 24,
        }
    }
}

/// Counts describing what the worker actually did. Observed, never estimated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AheadStats {
    /// Decodes that completed successfully.
    pub decoded: u64,
    /// Decoded, but the playhead had moved away before it could be used.
    /// Wasted work, counted rather than hidden.
    pub stale_discarded: u64,
    /// Pictures removed from the buffer to follow the playhead or the budget.
    pub evicted: u64,
    /// Requests answered from the buffer.
    pub hits: u64,
    /// Requests for a picture that was not yet decoded.
    pub misses: u64,
    /// Discontinuities: seeks away from the buffer, and direction changes.
    pub generations: u64,
    pub failures: u64,
    pub bytes: usize,
    pub peak_bytes: usize,
    pub decode_total_nanos: u128,
    pub decode_worst_nanos: u128,
}

impl AheadStats {
    pub fn mean_decode(&self) -> Option<Duration> {
        (self.decoded > 0).then(|| {
            Duration::from_nanos((self.decode_total_nanos / u128::from(self.decoded)) as u64)
        })
    }
}

struct State {
    generation: u64,
    playhead: usize,
    reverse: bool,
    frames: BTreeMap<usize, VideoFrame>,
    /// A decode failure at an index, for the current generation. The worker
    /// stops producing until the playhead moves to a new generation, and the
    /// consumer gets the error exactly when it asks for that picture.
    failure: Option<(usize, String)>,
    /// The decoder reported nothing at this index: the table claims more
    /// pictures than the stream holds. Nothing at or past it is attempted.
    exhausted_at: Option<usize>,
    /// What the worker is decoding right now, if anything.
    busy: Option<usize>,
    /// Size of the most recent picture, used to decide *before* decoding
    /// whether the next one would fit. Checking only after would let a picture
    /// that does not fit be decoded, evicted as the farthest, and decoded again,
    /// forever.
    last_bytes: usize,
    stop: bool,
    stats: AheadStats,
}

/// The immutable facts both threads plan with.
#[derive(Clone)]
struct Layout {
    config: AheadConfig,
    count: usize,
    /// Display positions of keyframes, ascending. Empty when unknown.
    keyframes: Arc<[usize]>,
}

impl Layout {
    fn keyframe_at_or_before(&self, index: usize) -> Option<usize> {
        let after = self.keyframes.partition_point(|&k| k <= index);
        after.checked_sub(1).map(|i| self.keyframes[i])
    }

    /// Lowest picture of the reverse chunk that ends at `top`.
    ///
    /// From the keyframe when known, because a chunk that starts part-way
    /// through a GOP forces the *next* chunk to walk from the same keyframe
    /// again: fixed 24-picture chunks over a 120-picture GOP cost 360 decodes to
    /// show 120 pictures, keyframe-aligned ones cost 120. Bounded by how many
    /// pictures of the last-seen size fit in the budget.
    fn reverse_chunk_start(&self, st: &State, top: usize) -> usize {
        match self.keyframe_at_or_before(top) {
            Some(key) => {
                // Unknown size (nothing decoded yet) is treated as "fits": the
                // first picture reveals the real size and the predictive budget
                // check stops the chunk if it does not.
                let fit = self
                    .config
                    .max_bytes
                    .checked_div(st.last_bytes)
                    .map_or(usize::MAX, |n| n.max(1));
                key.max(top.saturating_sub(fit - 1))
            }
            None => top.saturating_sub(self.config.reverse_chunk - 1),
        }
    }

    /// Lowest picture reverse playback wants ready: `ahead` below the playhead.
    fn reverse_floor(&self, st: &State) -> usize {
        st.playhead.saturating_sub(self.config.ahead - 1)
    }
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the worker: the playhead moved, or it should stop.
    work: Condvar,
    /// Wakes consumers: a picture arrived, a failure was recorded, or the worker
    /// went idle.
    ready: Condvar,
    notify: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// A [`FrameSource`] that decodes ahead of the playhead on its own thread.
pub struct DecodeAhead<D: FrameSource + Send + 'static> {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    layout: Layout,
    /// Presentation times by display index, copied from the decoder at
    /// construction so selection never has to cross to the worker thread.
    pts: Vec<RationalTime>,
    extent: Option<(RationalTime, RationalTime)>,
    reverse_ok: bool,
    config: AheadConfig,
    _decoder: std::marker::PhantomData<fn() -> D>,
}

impl<D: FrameSource + Send + 'static> DecodeAhead<D> {
    /// Move `decoder` onto a worker thread. Nothing is decoded until the first
    /// `prepare` or `decode_index` says where the playhead is.
    pub fn new(decoder: D, config: AheadConfig) -> DecodeAhead<D> {
        let count = decoder.frame_count();
        let pts: Vec<RationalTime> = (0..count).filter_map(|i| decoder.pts_of(i)).collect();
        let extent = decoder.extent();
        let reverse_ok = decoder.supports_reverse();
        let config = AheadConfig {
            ahead: config.ahead.max(1),
            reverse_chunk: config.reverse_chunk.max(1),
            ..config
        };
        // Walk back from the end one keyframe at a time: one query per GOP,
        // not one per picture.
        let mut keyframes = Vec::new();
        let mut at = count.checked_sub(1);
        while let Some(i) = at {
            match decoder.keyframe_at_or_before(i) {
                Some(k) if k <= i => {
                    keyframes.push(k);
                    at = k.checked_sub(1);
                }
                _ => break,
            }
        }
        keyframes.reverse();
        let layout = Layout {
            config,
            count: pts.len(),
            keyframes: keyframes.into(),
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                playhead: 0,
                // Idle until told where to go: `ahead` would otherwise start
                // decoding from zero for a playhead that may be anywhere.
                reverse: false,
                frames: BTreeMap::new(),
                failure: None,
                exhausted_at: None,
                busy: None,
                last_bytes: 0,
                stop: false,
                stats: AheadStats::default(),
            }),
            work: Condvar::new(),
            ready: Condvar::new(),
            notify: Mutex::new(None),
        });
        let worker = {
            let shared = shared.clone();
            let layout = layout.clone();
            std::thread::Builder::new()
                .name("cine-decode-ahead".into())
                .spawn(move || run(decoder, shared, layout))
                .expect("spawning the decode-ahead worker")
        };
        DecodeAhead {
            shared,
            worker: Some(worker),
            layout,
            pts,
            extent,
            reverse_ok,
            config,
            _decoder: std::marker::PhantomData,
        }
    }

    pub fn config(&self) -> AheadConfig {
        self.config
    }

    pub fn stats(&self) -> AheadStats {
        self.shared.state.lock().stats
    }

    /// Called from the worker after every picture it adds, and when it fails.
    ///
    /// An event loop sets this to wake itself, so waiting for a starved picture
    /// needs neither a spin nor a timer.
    pub fn set_notify(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.shared.notify.lock() = Some(Box::new(f));
    }

    /// How many consecutive pictures, starting at `from` in the direction of
    /// travel, are already decoded.
    pub fn buffered_run(&self, from: usize) -> usize {
        let st = self.shared.state.lock();
        run_length(&st, from, self.pts.len())
    }

    /// Wait until `pictures` consecutive pictures from `from` are decoded, the
    /// worker can make no further progress, or `timeout` passes. Returns the run
    /// actually buffered.
    ///
    /// This is pre-roll: a transport that waits here before starting the clock
    /// begins with a cushion instead of an empty buffer. It is an explicit step,
    /// measured by its caller, not something that happens silently.
    pub fn preroll(&self, from: usize, pictures: usize, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        let mut st = self.shared.state.lock();
        let count = self.pts.len();
        let want = pictures.min(count.saturating_sub(from));
        let reverse = st.reverse;
        reposition(&mut st, from, reverse, &self.layout);
        self.shared.work.notify_one();
        loop {
            let have = run_length(&st, from, count);
            let stuck = st.busy.is_none() && next_job(&st, &self.layout).is_none();
            if have >= want || stuck || st.failure.is_some() {
                return have;
            }
            if self.shared.ready.wait_until(&mut st, deadline).timed_out() {
                return run_length(&st, from, count);
            }
        }
    }

    /// Wait until the worker has nothing left to do for the current playhead.
    /// Returns `false` on timeout. For tests and for measurement.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.shared.state.lock();
        loop {
            if st.busy.is_none() && next_job(&st, &self.layout).is_none() {
                return true;
            }
            if self.shared.ready.wait_until(&mut st, deadline).timed_out() {
                return false;
            }
        }
    }

    /// Display indices currently held, in order. For tests and diagnostics.
    pub fn buffered(&self) -> Vec<usize> {
        self.shared.state.lock().frames.keys().copied().collect()
    }
}

impl<D: FrameSource + Send + 'static> Drop for DecodeAhead<D> {
    /// Stops the worker and waits for it. That wait is bounded by at most one
    /// in-flight decode; nothing queued is started.
    fn drop(&mut self) {
        self.shared.state.lock().stop = true;
        self.shared.work.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl<D: FrameSource + Send + 'static> FrameSource for DecodeAhead<D> {
    fn frame_count(&self) -> usize {
        self.pts.len()
    }

    fn pts_of(&self, index: usize) -> Option<RationalTime> {
        self.pts.get(index).copied()
    }

    /// Exact, and on this thread: a binary search over the copied table using
    /// `RationalTime`'s cross-multiplying order.
    fn index_at(&self, t: RationalTime) -> Option<usize> {
        let after = self.pts.partition_point(|p| *p <= t);
        after.checked_sub(1)
    }

    fn extent(&self) -> Option<(RationalTime, RationalTime)> {
        self.extent
    }

    fn supports_reverse(&self) -> bool {
        self.reverse_ok
    }

    fn prepare(&mut self, index: usize, reverse: bool) {
        let mut st = self.shared.state.lock();
        if st.playhead != index || st.reverse != reverse || st.stats.generations == 0 {
            reposition(&mut st, index, reverse, &self.layout);
            self.shared.work.notify_one();
        }
    }

    /// Never blocks. A picture not yet decoded is `Ok(None)`, which the
    /// scheduler reports as an underrun.
    fn decode_index(&mut self, index: usize) -> Result<Option<VideoFrame>> {
        let mut st = self.shared.state.lock();
        if st.playhead != index || st.stats.generations == 0 {
            let reverse = st.reverse;
            reposition(&mut st, index, reverse, &self.layout);
            self.shared.work.notify_one();
        }
        if let Some(frame) = st.frames.get(&index).cloned() {
            st.stats.hits += 1;
            return Ok(Some(frame));
        }
        if let Some((at, message)) = &st.failure {
            if *at == index {
                return Err(Error::Decode {
                    at: format!("display index {index}"),
                    message: message.clone(),
                });
            }
        }
        st.stats.misses += 1;
        Ok(None)
    }

    fn wait_ready(&self, index: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.shared.state.lock();
        loop {
            if st.frames.contains_key(&index) {
                return true;
            }
            // A failure or exhaustion at this index is an answer too: waiting
            // longer would not produce the picture.
            if st.failure.as_ref().is_some_and(|(at, _)| *at == index)
                || st.exhausted_at.is_some_and(|end| index >= end)
            {
                return true;
            }
            if self.shared.ready.wait_until(&mut st, deadline).timed_out() {
                return false;
            }
        }
    }
}

/// The range of display indices worth holding around the playhead, `[lo, hi)`.
fn keep_window(st: &State, layout: &Layout) -> (usize, usize) {
    let cfg = &layout.config;
    if st.reverse {
        // Ahead is *below* the playhead, and reaches down to the start of the
        // chunk that serves the lowest picture wanted. Anything the planner may
        // decode must be inside this window, or it would be decoded, discarded
        // as out of place, and decoded again.
        let lo = layout.reverse_chunk_start(st, layout.reverse_floor(st));
        let hi = (st.playhead + cfg.behind + 1).min(layout.count);
        (lo, hi)
    } else {
        let lo = st.playhead.saturating_sub(cfg.behind);
        let hi = (st.playhead + cfg.ahead).min(layout.count);
        (lo, hi)
    }
}

/// Move the playhead, starting a new generation on a discontinuity, and evict
/// what no longer belongs.
fn reposition(st: &mut State, index: usize, reverse: bool, layout: &Layout) {
    let (lo, hi) = keep_window(st, layout);
    let continuous =
        st.stats.generations > 0 && st.reverse == reverse && index >= lo && index < hi.max(lo + 1);
    if !continuous {
        st.generation += 1;
        st.stats.generations += 1;
        st.failure = None;
    }
    st.playhead = index.min(layout.count.saturating_sub(1));
    st.reverse = reverse;
    evict_outside_window(st, layout);
}

fn evict_outside_window(st: &mut State, layout: &Layout) {
    let (lo, hi) = keep_window(st, layout);
    let doomed: Vec<usize> = st
        .frames
        .keys()
        .copied()
        .filter(|&i| i < lo || i >= hi)
        .collect();
    for i in doomed {
        if let Some(f) = st.frames.remove(&i) {
            st.stats.bytes -= f.byte_size();
            st.stats.evicted += 1;
        }
    }
}

/// Consecutive decoded pictures from `from` in the direction of travel.
fn run_length(st: &State, from: usize, count: usize) -> usize {
    let mut n = 0;
    if st.reverse {
        let mut i = from as i64;
        while i >= 0 && st.frames.contains_key(&(i as usize)) {
            n += 1;
            i -= 1;
        }
    } else {
        let mut i = from;
        while i < count && st.frames.contains_key(&i) {
            n += 1;
            i += 1;
        }
    }
    n
}

/// The next display index the worker should decode, or `None` if it should
/// wait.
fn next_job(st: &State, layout: &Layout) -> Option<usize> {
    let cfg = &layout.config;
    let count = layout.count;
    if st.stop || st.failure.is_some() || count == 0 || st.stats.generations == 0 {
        return None;
    }
    let limit = st.exhausted_at.unwrap_or(count).min(count);
    let missing = |i: &usize| *i < limit && !st.frames.contains_key(i);
    // Checked before decoding, with the last picture's size as the estimate.
    // The picture at the playhead is always wanted, budget or not.
    let over_budget = st.stats.bytes + st.last_bytes > cfg.max_bytes;

    if !st.reverse {
        if missing(&st.playhead) {
            return Some(st.playhead);
        }
        if over_budget {
            return None;
        }
        let end = (st.playhead + cfg.ahead).min(limit);
        (st.playhead..end).find(|i| missing(i))
    } else {
        // The highest picture still needed at or below the playhead, then the
        // chunk that ends there, decoded from the bottom up: every decode after
        // the first continues forward from the one before.
        let (lo, _) = keep_window(st, layout);
        let floor = layout.reverse_floor(st);
        let top = (floor..=st.playhead.min(limit.saturating_sub(1)))
            .rev()
            .find(|i| missing(i))?;
        if over_budget && top != st.playhead {
            return None;
        }
        let start = layout.reverse_chunk_start(st, top).max(lo);
        (start..=top).find(|i| missing(i))
    }
}

fn run<D: FrameSource>(mut decoder: D, shared: Arc<Shared>, layout: Layout) {
    loop {
        let (generation, index) = {
            let mut st = shared.state.lock();
            loop {
                if st.stop {
                    return;
                }
                if let Some(i) = next_job(&st, &layout) {
                    st.busy = Some(i);
                    break (st.generation, i);
                }
                // Idle: let anyone waiting for idleness see it.
                shared.ready.notify_all();
                shared.work.wait(&mut st);
            }
        };

        let started = Instant::now();
        let result = decoder.decode_index(index);
        let took = started.elapsed().as_nanos();

        {
            let mut st = shared.state.lock();
            st.busy = None;
            match result {
                Ok(Some(frame)) => {
                    st.stats.decoded += 1;
                    st.last_bytes = frame.byte_size();
                    st.stats.decode_total_nanos += took;
                    st.stats.decode_worst_nanos = st.stats.decode_worst_nanos.max(took);
                    let (lo, hi) = keep_window(&st, &layout);
                    // Kept only if it still belongs to the playhead's
                    // neighbourhood — whatever generation asked for it. The same
                    // index is the same picture, so a seek that lands nearby is
                    // not a reason to throw correct work away; a seek elsewhere is.
                    if index >= lo && index < hi {
                        admit(&mut st, index, frame, &layout.config);
                    } else {
                        st.stats.stale_discarded += 1;
                    }
                }
                Ok(None) => {
                    st.exhausted_at = Some(st.exhausted_at.map_or(index, |e| e.min(index)));
                }
                Err(e) => {
                    st.stats.failures += 1;
                    if generation == st.generation {
                        st.failure = Some((index, e.to_string()));
                    }
                }
            }
        }
        shared.ready.notify_all();
        if let Some(notify) = shared.notify.lock().as_ref() {
            notify();
        }
    }
}

/// Insert a picture, then enforce the byte budget. Pictures already shown go
/// first, farthest first; then the far end of the lookahead. The playhead's own
/// picture is never evicted.
fn admit(st: &mut State, index: usize, frame: VideoFrame, cfg: &AheadConfig) {
    st.stats.bytes += frame.byte_size();
    if let Some(old) = st.frames.insert(index, frame) {
        st.stats.bytes -= old.byte_size();
    }
    while st.stats.bytes > cfg.max_bytes && st.frames.len() > 1 {
        let p = st.playhead;
        let shown_side = |i: usize| if st.reverse { i > p } else { i < p };
        let victim = st
            .frames
            .keys()
            .copied()
            .filter(|&i| i != p)
            .max_by_key(|&i| (shown_side(i), i.abs_diff(p)));
        let Some(victim) = victim else {
            break;
        };
        if let Some(f) = st.frames.remove(&victim) {
            st.stats.bytes -= f.byte_size();
            st.stats.evicted += 1;
        }
    }
    st.stats.peak_bytes = st.stats.peak_bytes.max(st.stats.bytes);
}
