//! The renderer: pull-model block processing of a [`Session`] into a
//! [`ChannelLayout`].
//!
//! Signal flow per track (`BLOCK_SIZE` frames at a time, tracks in parallel):
//!
//! ```text
//! clips (gain, fades) → inserts → pan (VBAP / stereo law) → fader → main out
//!                                         └─ pre-fader sends ┘  └─ post-fader sends
//! ```
//!
//! Track outputs are summed into their bus, buses are processed farthest
//! from the master first (inserts, volume, mute), and the master runs its
//! own inserts (a brick-wall limiter by default).
//!
//! Volume, pan, elevation, spread, mute and send levels are sampled from the
//! track automation at every block boundary and interpolated linearly across
//! the block, so ramps are click-free at any block size.
//!
//! Stems ([`render_stems`]) are the same signal path with only the tracks of
//! one stem contributing and **without the master inserts**: the full mix is
//! `master_inserts(Σ stems)`, so the stems sum to the mix exactly whenever
//! every bus insert is linear and the master limiter is not engaged.
//!
//! # Compile once, render many
//!
//! [`render`] and [`render_stems`] are the offline entry points, but neither is
//! the engine. The engine is [`Renderer`]: a session **compiled** into the
//! shape the block loop wants — clips resolved onto the sample grid and
//! rate-converted, insert chains built, static pan matrices evaluated, routing
//! turned from ids into slot indices, every buffer allocated at its final size
//! — after which [`Renderer::render_block`] does per-sample work and nothing
//! else. It allocates nothing and looks nothing up.
//!
//! That split is the design. Asking a one-shot renderer for a single block
//! makes it pay the whole compilation for 512 frames of audio, which no amount
//! of inner-loop tuning can recover; and a session is compiled from data that
//! does not change while it plays. `tests/mix_cost.rs` holds the line.
//!
//! # Latency
//!
//! Inserts with lookahead (the limiter) delay their output. Each stage's delay
//! is cancelled by reading the timeline that far ahead of the block being
//! emitted: the renderer works out how much lead each routing path needs,
//! takes the largest as the pipeline lead, gives shorter paths a compensating
//! delay so they still meet at the master in phase, and runs the lead in when
//! it is positioned. The mix that comes out is aligned with the timeline —
//! exactly what dropping the first `latency` samples of a whole-buffer render
//! achieved, without needing the whole buffer to exist.

use crate::buffer::{db_to_lin, AudioBuffer};
use crate::dsp::{Biquad, Compressor, FadeCurve, Gate, Limiter};
use crate::error::{Error, Result};
use crate::pan::{ChannelLayout, ObjectPanParams, PanLaw, VbapPanner, MAX_CHANNELS};
use crate::session::{
    Automation, BusId, ClipSource, Insert, Lane, Param, Session, Stem, Track, TrackKind,
};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

/// Frames processed per block.
pub const BLOCK_SIZE: usize = 512;

/// Stem used for tracks that deliver to no stem (no track stem and no stem
/// bus on their routing path) in [`render_stems`].
pub fn unassigned_stem() -> Stem {
    Stem::Custom("unassigned".into())
}

// ---------------------------------------------------------------------------
// Insert chains
// ---------------------------------------------------------------------------

enum Proc {
    Biquad(Vec<Biquad>),
    Compressor(Compressor),
    Gate(Gate),
    Limiter(Limiter),
    Gain(f32),
}

/// A stateful chain of inserts for one signal of `channels` channels.
struct Chain {
    procs: Vec<Proc>,
    channels: usize,
    /// Total lookahead latency of the chain in frames.
    latency: usize,
}

impl Chain {
    fn new(inserts: &[Insert], sample_rate: u32, channels: usize) -> Self {
        let sr = sample_rate as f32;
        let mut latency = 0;
        let procs = inserts
            .iter()
            .map(|ins| match ins {
                Insert::Biquad {
                    kind,
                    freq_hz,
                    q,
                    gain_db,
                } => Proc::Biquad(vec![
                    Biquad::design(
                        *kind,
                        f64::from(sample_rate),
                        f64::from(*freq_hz),
                        f64::from(*q),
                        f64::from(*gain_db)
                    );
                    channels
                ]),
                Insert::Compressor(p) => Proc::Compressor(Compressor::new(*p, sr)),
                Insert::Gate(p) => Proc::Gate(Gate::new(*p, sr)),
                Insert::Limiter(p) => {
                    let l = Limiter::new(*p, sr, channels);
                    latency += l.latency_frames();
                    Proc::Limiter(l)
                }
                Insert::Gain { db } => Proc::Gain(db_to_lin(*db)),
            })
            .collect();
        Chain {
            procs,
            channels,
            latency,
        }
    }

    fn is_empty(&self) -> bool {
        self.procs.is_empty()
    }

    /// Return every processor to its initial state.
    fn reset(&mut self) {
        for p in &mut self.procs {
            match p {
                Proc::Biquad(filters) => {
                    for f in filters.iter_mut() {
                        f.reset();
                    }
                }
                Proc::Compressor(c) => c.reset(),
                Proc::Gate(g) => g.reset(),
                Proc::Limiter(l) => l.reset(),
                Proc::Gain(_) => {}
            }
        }
    }

    /// Process one interleaved block in place (output delayed by `latency`).
    fn process(&mut self, block: &mut [f32]) {
        let ch = self.channels;
        for p in &mut self.procs {
            match p {
                Proc::Biquad(filters) => {
                    // One channel at a time, not one frame at a time: a biquad
                    // is a two-sample recurrence, and interleaving the channels
                    // reloads its state on every sample.
                    for (c, f) in filters.iter_mut().enumerate().take(ch) {
                        f.process_strided(&mut block[c..], ch);
                    }
                }
                Proc::Compressor(c) => c.process(block, ch),
                Proc::Gate(g) => g.process(block, ch),
                Proc::Limiter(l) => l.process(block),
                Proc::Gain(g) => {
                    for s in block.iter_mut() {
                        *s *= *g;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Panning
// ---------------------------------------------------------------------------

fn fade_pos(i: usize, n: usize) -> f32 {
    if n <= 1 {
        1.0
    } else {
        i as f32 / (n - 1) as f32
    }
}

/// Gains for a single (mono) signal at a direction: the stereo pan law when
/// rendering to stereo, VBAP otherwise.
fn mono_gains_into(
    panner: &VbapPanner,
    law: PanLaw,
    az: f32,
    el: f32,
    spread: f32,
    dst: &mut [f32],
) {
    if panner.layout() == ChannelLayout::Stereo {
        let (l, r) = law.gains((-az / 90.0).clamp(-1.0, 1.0));
        dst[0] = l;
        dst[1] = r;
    } else {
        panner.gains_with_spread_into(az, el, spread, dst);
    }
}

/// Write the pan matrix for a `src_ch`-channel signal into `dst`, row-major
/// (`src_ch` rows of `out` gains). [`pan_matrix`] is a thin allocating wrapper
/// over this; the renderer uses this form, into a buffer it already owns.
#[allow(clippy::too_many_arguments)]
fn pan_matrix_into(
    panner: &VbapPanner,
    law: PanLaw,
    position: Option<[f32; 3]>,
    az: f32,
    el: f32,
    spread: f32,
    src_ch: usize,
    dst: &mut [f32],
) {
    let layout = panner.layout();
    let out = layout.channel_count();
    let src_ch = src_ch.max(1);
    debug_assert_eq!(dst.len(), src_ch * out);
    if let Some(pos) = position {
        let mut g = [0.0f32; MAX_CHANNELS];
        panner.pan_object_into(
            pos,
            &ObjectPanParams {
                spread_deg: spread,
                ..Default::default()
            },
            &mut g[..out],
        );
        let scale = 1.0 / (src_ch as f32).sqrt();
        for row in dst.chunks_mut(out) {
            for (d, s) in row.iter_mut().zip(&g[..out]) {
                *d = s * scale;
            }
        }
        return;
    }
    if src_ch == 1 {
        mono_gains_into(panner, law, az, el, spread, dst);
        return;
    }
    if src_ch == out {
        dst.fill(0.0);
        for c in 0..src_ch {
            dst[c * out + c] = 1.0;
        }
        return;
    }
    if layout == ChannelLayout::Mono {
        dst.fill(1.0 / src_ch as f32);
        return;
    }
    let width = if layout == ChannelLayout::Stereo {
        90.0
    } else {
        30.0
    };
    for (c, row) in dst.chunks_mut(out).enumerate() {
        let offset = width * (1.0 - 2.0 * c as f32 / (src_ch - 1) as f32);
        mono_gains_into(panner, law, az + offset, el, spread, row);
    }
}

/// Pan matrix `[src_channel][out_channel]` for a track signal of `src_ch`
/// channels.
///
/// * a positioned object: every source channel gets the object gains
///   (distance attenuation, spread), scaled by `1/√n` for constant power;
/// * a mono signal: the stereo pan law (stereo layout) or VBAP with spread;
/// * a signal as wide as the layout: passed through channel for channel;
/// * any other width: channel `c` is panned to `az + offset_c` with the
///   offsets spread symmetrically across the front (±90° in pan-law terms
///   for stereo, ±30° for VBAP layouts), so a stereo file at centre lands on
///   `L`/`R` at unity.
pub fn pan_matrix(
    panner: &VbapPanner,
    law: PanLaw,
    position: Option<[f32; 3]>,
    az: f32,
    el: f32,
    spread: f32,
    src_ch: usize,
) -> Vec<Vec<f32>> {
    let out = panner.layout().channel_count();
    let src_ch = src_ch.max(1);
    let mut flat = vec![0.0f32; src_ch * out];
    pan_matrix_into(panner, law, position, az, el, spread, src_ch, &mut flat);
    flat.chunks(out).map(<[f32]>::to_vec).collect()
}

// ---------------------------------------------------------------------------
// Compiled clips
// ---------------------------------------------------------------------------

/// One clip resolved onto the sample grid: source buffer already at the session
/// rate, absolute timeline frames, gain and fade lengths in frames.
struct ClipPlay {
    buf: Arc<AudioBuffer>,
    /// Absolute timeline frame of the clip's first frame.
    start: i64,
    /// Length on the timeline in frames.
    len: usize,
    /// Source frame at clip-local frame 0.
    src_offset: i64,
    gain: f32,
    fade_in: (usize, FadeCurve),
    fade_out: (usize, FadeCurve),
}

impl ClipPlay {
    #[inline]
    fn gain_at(&self, k: usize) -> f32 {
        let mut g = self.gain;
        if k < self.fade_in.0 {
            g *= self.fade_in.1.gain_in(fade_pos(k, self.fade_in.0));
        }
        let fo = self.fade_out.0;
        if fo > 0 && k + fo >= self.len {
            g *= self.fade_out.1.gain_out(fade_pos(k + fo - self.len, fo));
        }
        g
    }

    /// One clip-local frame at an explicit gain.
    #[inline]
    fn add_one(&self, k: i64, read: i64, input: &mut [f32], src_ch: usize, g: f32) {
        let cch = self.buf.channels;
        let d0 = ((self.start + k - read) as usize) * src_ch;
        let s0 = ((self.src_offset + k) as usize) * cch;
        if cch == 1 {
            let v = self.buf.data[s0] * g;
            for x in input[d0..d0 + src_ch].iter_mut() {
                *x += v;
            }
        } else {
            let m = cch.min(src_ch);
            for (x, s) in input[d0..d0 + m].iter_mut().zip(&self.buf.data[s0..s0 + m]) {
                *x += s * g;
            }
        }
    }

    /// Clip-local frames `[k0, k1)` at the clip's constant gain — the body of
    /// the clip, which is everything the fade curves do not touch and almost
    /// always the whole block. Kept as one run so it stays a scale-and-add over
    /// contiguous memory rather than a call and two branches per frame.
    fn add_run(&self, k0: i64, k1: i64, read: i64, input: &mut [f32], src_ch: usize) {
        let cch = self.buf.channels;
        let count = (k1 - k0) as usize;
        let d0 = (self.start + k0 - read) as usize;
        let s0 = (self.src_offset + k0) as usize;
        let g = self.gain;
        if cch == src_ch {
            let dst = &mut input[d0 * src_ch..(d0 + count) * src_ch];
            let src = &self.buf.data[s0 * cch..(s0 + count) * cch];
            for (d, s) in dst.iter_mut().zip(src) {
                *d += s * g;
            }
        } else if cch == 1 {
            let dst = &mut input[d0 * src_ch..(d0 + count) * src_ch];
            let src = &self.buf.data[s0..s0 + count];
            for (frame, s) in dst.chunks_mut(src_ch).zip(src) {
                let v = s * g;
                for x in frame.iter_mut() {
                    *x += v;
                }
            }
        } else {
            let m = cch.min(src_ch);
            for i in 0..count {
                let d = (d0 + i) * src_ch;
                let s = (s0 + i) * cch;
                for (x, y) in input[d..d + m].iter_mut().zip(&self.buf.data[s..s + m]) {
                    *x += y * g;
                }
            }
        }
    }

    /// Add this clip's contribution for absolute frames `[read, read + n)`.
    /// Frames at or after `end` contribute nothing: that is the render
    /// window's edge, and it is what keeps a bounded render and an open-ended
    /// one identical inside the window.
    fn gather(&self, read: i64, n: usize, end: i64, input: &mut [f32], src_ch: usize) {
        let frames = self.buf.frames() as i64;
        let lo = self.start.max(read);
        let hi = (self.start + self.len as i64).min(read + n as i64).min(end);
        if hi <= lo {
            return;
        }
        // Clip-local span, clamped to what the source buffer actually holds.
        let k0 = (lo - self.start).max(-self.src_offset);
        let k1 = (hi - self.start).min(frames - self.src_offset);
        if k1 <= k0 {
            return;
        }
        let body0 = k0.max(self.fade_in.0 as i64);
        let body1 = k1.min(self.len as i64 - self.fade_out.0 as i64);
        if body1 <= body0 {
            // Wholly inside a fade, or the two fades overlap: every frame needs
            // its own gain.
            for k in k0..k1 {
                self.add_one(k, read, input, src_ch, self.gain_at(k as usize));
            }
            return;
        }
        for k in k0..body0 {
            self.add_one(k, read, input, src_ch, self.gain_at(k as usize));
        }
        self.add_run(body0, body1, read, input, src_ch);
        for k in body1..k1 {
            self.add_one(k, read, input, src_ch, self.gain_at(k as usize));
        }
    }
}

// ---------------------------------------------------------------------------
// Compiled tracks
// ---------------------------------------------------------------------------

/// The automation lanes a track actually uses, owned by the compiled track so
/// the renderer holds no borrow of the session it was built from.
#[derive(Default)]
struct Lanes {
    vol: Option<Lane>,
    pan: Option<Lane>,
    el: Option<Lane>,
    spread: Option<Lane>,
    mute: Option<Lane>,
    sends: Vec<Option<Lane>>,
}

fn lane_of(a: &Automation, p: Param) -> Option<Lane> {
    a.lanes.get(&p).filter(|l| !l.points.is_empty()).cloned()
}

/// Block-boundary control values, flat rather than nested: the pan matrix is
/// one `src_ch × out` run of floats, not a `Vec` of `Vec`s.
#[derive(Clone, Default)]
struct Controls {
    vol: f32,
    muted: bool,
    pan: Vec<f32>,
    sends: Vec<f32>,
}

/// Everything about a track that a block cannot change: its resting pan and
/// levels, and the lanes that may override them.
struct TrackDesc {
    law: PanLaw,
    position: Option<[f32; 3]>,
    az: f32,
    el: f32,
    spread: f32,
    vol_db: f32,
    mute: bool,
    send_db: Vec<f32>,
    send_pre: Vec<bool>,
    src_ch: usize,
    lanes: Lanes,
}

impl TrackDesc {
    /// Sample the automation at `t` into `ctl`, reusing its buffers.
    fn sample(&self, panner: &VbapPanner, t: f64, pan_too: bool, ctl: &mut Controls) {
        let l = &self.lanes;
        let vol_db = l
            .vol
            .as_ref()
            .and_then(|x| x.value_at(t))
            .map_or(self.vol_db, |v| v as f32);
        ctl.vol = db_to_lin(vol_db);
        ctl.muted = l
            .mute
            .as_ref()
            .and_then(|x| x.value_at(t))
            .map_or(self.mute, |v| v > 0.5);
        for (i, s) in ctl.sends.iter_mut().enumerate() {
            let db = l.sends[i]
                .as_ref()
                .and_then(|x| x.value_at(t))
                .map_or(self.send_db[i], |v| v as f32);
            *s = db_to_lin(db);
        }
        if pan_too {
            let az = l
                .pan
                .as_ref()
                .and_then(|x| x.value_at(t))
                .map_or(self.az, |v| v as f32);
            let el =
                l.el.as_ref()
                    .and_then(|x| x.value_at(t))
                    .map_or(self.el, |v| v as f32);
            let spread = l
                .spread
                .as_ref()
                .and_then(|x| x.value_at(t))
                .map_or(self.spread, |v| v as f32);
            pan_matrix_into(
                panner,
                self.law,
                self.position,
                az,
                el,
                spread,
                self.src_ch,
                &mut ctl.pan,
            );
        }
    }
}

/// One track compiled for a layout: nothing left to resolve, every buffer at
/// its final size.
struct TrackState {
    desc: TrackDesc,
    stem: Option<Stem>,
    /// `true` when the track contributes nothing at all (a folder, un-soloed,
    /// or holding no clip with audio); such tracks are skipped rather than
    /// summed as a block of zeros.
    silent: bool,
    clips: Vec<ClipPlay>,
    src_ch: usize,
    out_ch: usize,
    chain: Chain,
    chain_latency: usize,
    /// Any automation at all: when false the controls are evaluated once.
    dynamic: bool,
    /// Pan/elevation/spread automation: when false the pan matrix is evaluated
    /// once, at compile time, and reused for every block.
    pan_dynamic: bool,
    main_slot: usize,
    /// `None` for a send to a bus that does not exist: it is dropped, as it
    /// always was, not rerouted to the master.
    send_slots: Vec<Option<usize>>,
    ctl_a: Controls,
    ctl_b: Controls,
    input: Vec<f32>,
    panned: Vec<f32>,
    main: Vec<f32>,
    sends: Vec<f32>,
}

impl TrackState {
    /// Render one block of `n` frames: gather the timeline at `read`, run the
    /// inserts, pan, and apply the fader and sends. `t1` is the session time at
    /// the end of the block, where the automation is next sampled.
    fn render_block(&mut self, panner: &VbapPanner, read: i64, n: usize, gather_end: i64, t1: f64) {
        if self.silent {
            return;
        }
        let src_ch = self.src_ch;
        let out = self.out_ch;

        // 1. Gather the clips, then the inserts. The read position runs the
        //    chain's own lookahead ahead of the frame being emitted, which is
        //    what makes the chain's delay disappear.
        {
            let input = &mut self.input[..n * src_ch];
            input.fill(0.0);
            let read = read + self.chain_latency as i64;
            for clip in &self.clips {
                clip.gather(read, n, gather_end, input, src_ch);
            }
            if !self.chain.is_empty() {
                self.chain.process(input);
            }
        }

        // 2. Controls. The previous block's end value is this block's start
        //    value, so only one end is ever evaluated.
        if self.dynamic {
            std::mem::swap(&mut self.ctl_a, &mut self.ctl_b);
            self.desc
                .sample(panner, t1, self.pan_dynamic, &mut self.ctl_b);
        }
        let inv = if n > 1 { 1.0 / (n - 1) as f32 } else { 0.0 };
        let pan_static = !self.pan_dynamic || self.ctl_a.pan == self.ctl_b.pan;

        // 3. Pan. With a matrix that does not move across the block — which is
        //    every track that does not automate its pan — the gain leaves the
        //    frame loop entirely and each source/output pair is a scale-and-add.
        {
            let panned = &mut self.panned[..n * out];
            panned.fill(0.0);
            let input = &self.input[..n * src_ch];
            for c in 0..src_ch {
                for o in 0..out {
                    let g0 = self.ctl_a.pan[c * out + o];
                    let g1 = self.ctl_b.pan[c * out + o];
                    let src = input[c..].iter().step_by(src_ch).take(n);
                    if pan_static {
                        if g0 == 0.0 {
                            continue;
                        }
                        for (p, s) in panned[o..].iter_mut().step_by(out).zip(src) {
                            *p += s * g0;
                        }
                    } else {
                        if g0 == 0.0 && g1 == 0.0 {
                            continue;
                        }
                        let d = (g1 - g0) * inv;
                        for (k, (p, s)) in panned[o..].iter_mut().step_by(out).zip(src).enumerate()
                        {
                            *p += s * (g0 + d * k as f32);
                        }
                    }
                }
            }
        }

        // 4. Fader.
        let mute = if self.ctl_a.muted { 0.0 } else { 1.0 };
        let v0 = self.ctl_a.vol * mute;
        let v1 = self.ctl_b.vol * mute;
        {
            let panned = &self.panned[..n * out];
            let main = &mut self.main[..n * out];
            ramp_into(main, panned, v0, v1, inv, out);
        }

        // 5. Sends.
        for i in 0..self.desc.send_pre.len() {
            let (l0, l1) = if self.desc.send_pre[i] {
                (self.ctl_a.sends[i] * mute, self.ctl_b.sends[i] * mute)
            } else {
                (
                    self.ctl_a.sends[i] * self.ctl_a.vol * mute,
                    self.ctl_b.sends[i] * self.ctl_b.vol * mute,
                )
            };
            let base = i * BLOCK_SIZE * out;
            let panned = &self.panned[..n * out];
            let dst = &mut self.sends[base..base + n * out];
            ramp_into(dst, panned, l0, l1, inv, out);
        }
    }
}

/// `dst = src × gain`, with the gain ramped linearly across the block when the
/// two ends differ.
#[inline]
fn ramp_into(dst: &mut [f32], src: &[f32], g0: f32, g1: f32, inv: f32, out: usize) {
    if g0 == g1 {
        for (d, s) in dst.iter_mut().zip(src) {
            *d = s * g0;
        }
    } else {
        let step = (g1 - g0) * inv;
        for (f, chunk) in dst.chunks_mut(out).enumerate() {
            let g = g0 + step * f as f32;
            for (d, s) in chunk.iter_mut().zip(&src[f * out..(f + 1) * out]) {
                *d = s * g;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Compiled buses
// ---------------------------------------------------------------------------

/// One summing point of one mixdown.
struct BusSlot {
    chain: Chain,
    volume: f32,
    mute: bool,
    /// Slot this one feeds (itself, for the master).
    dest: usize,
    /// This block's signal, `BLOCK_SIZE × out`.
    buf: Vec<f32>,
    /// Compensating delay on the *track* contributions entering this slot, so
    /// routes of unequal insert latency still meet at the master in phase.
    /// Empty — and free — whenever every route has the same latency, which is
    /// the normal case: only the master carries a lookahead insert.
    delay: Vec<f32>,
    delay_pos: usize,
    /// Accumulator for the track contributions when `delay` is in use.
    acc: Vec<f32>,
}

impl BusSlot {
    fn reset(&mut self) {
        self.chain.reset();
        self.delay.fill(0.0);
        self.delay_pos = 0;
    }
}

/// One mixdown of the track outputs: the full mix, or one stem.
struct MixTarget {
    slots: Vec<BusSlot>,
    /// Slot order, farthest from the master first; the master is not in it.
    order: Vec<usize>,
    master: usize,
    master_inserts: bool,
    master_mute: bool,
    master_volume: f32,
    /// Which tracks contribute.
    include: Vec<bool>,
}

// ---------------------------------------------------------------------------
// The renderer
// ---------------------------------------------------------------------------

/// A [`Session`] compiled for one [`ChannelLayout`], ready to deliver blocks.
///
/// Construction resolves every clip onto the sample grid (rate-converting once,
/// not once per render and never per block), builds every insert chain,
/// evaluates every static pan matrix, turns routing ids into slot indices and
/// allocates every buffer. After that
/// [`render_block`](Self::render_block) allocates nothing and looks nothing up.
///
/// The renderer owns everything it touches, so it holds no borrow of the
/// session and can be handed to another thread.
///
/// ```
/// use cine_audio::prelude::*;
/// use cine_audio::mix::{Renderer, BLOCK_SIZE};
///
/// let mut session = Session::new(48_000, 24.0);
/// let tone = std::sync::Arc::new(AudioBuffer::sine(48_000, 1, 1000.0, 0.5, 1.0));
/// let master = session.master_id();
/// session.add_track(
///     Track::new("tone", master).with_clip(Clip::from_buffer("t", tone, 0.0, 24.0)),
/// );
///
/// let mut r = Renderer::new(&session, ChannelLayout::Stereo).unwrap();
/// r.prepare((0.0, 1.0)).unwrap();
/// let mut block = vec![0.0f32; BLOCK_SIZE * 2];
/// r.render_block(&mut block, BLOCK_SIZE).unwrap();
/// assert!(block.iter().any(|s| s.abs() > 0.1));
/// ```
pub struct Renderer {
    sample_rate: u32,
    layout: ChannelLayout,
    out_ch: usize,
    panner: VbapPanner,
    tracks: Vec<TrackState>,
    targets: Vec<MixTarget>,
    /// Frames the pipeline runs ahead of the block it emits.
    lead: usize,
    /// Absolute timeline frame the tracks read next.
    read: i64,
    /// First frame of the render window.
    start_frame: i64,
    /// One past the last frame that may be gathered.
    gather_end: i64,
    /// Track-output frames produced since positioning, lead-in included.
    emitted: i64,
    positioned: bool,
    /// Whether the track stage is spread over a thread pool. See
    /// [`set_parallel`](Self::set_parallel) for why this is off by default.
    parallel: bool,
}

impl Renderer {
    /// Compile `session` for `layout`.
    pub fn new(session: &Session, layout: ChannelLayout) -> Result<Self> {
        Self::with_stems(session, layout, None)
    }

    /// Compile with one mixdown per stem (master inserts off, as stems are
    /// pre-master), instead of the single full mix.
    fn with_stems(
        session: &Session,
        layout: ChannelLayout,
        stems: Option<&[Stem]>,
    ) -> Result<Self> {
        if session.sample_rate == 0 {
            return Err(Error::InvalidArgument("session sample rate is zero".into()));
        }
        let sr = session.sample_rate;
        let out_ch = layout.channel_count();
        let panner = VbapPanner::new(layout);

        // Slots 0..n are the session buses in order; the master is last.
        let master_slot = session.buses.len();
        let slot_of: HashMap<BusId, usize> = session
            .buses
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        let order = bus_order(session, &slot_of)?;

        let has_solo = session.has_solo();
        let tracks: Vec<TrackState> = session
            .tracks
            .iter()
            .map(|t| compile_track(session, t, &panner, out_ch, has_solo, &slot_of, master_slot))
            .collect();

        let build_slots = |master_inserts: bool| -> Vec<BusSlot> {
            let slot = |inserts: &[Insert], volume_db: f32, mute: bool, dest: usize| BusSlot {
                chain: Chain::new(inserts, sr, out_ch),
                volume: db_to_lin(volume_db),
                mute,
                dest,
                buf: vec![0.0; BLOCK_SIZE * out_ch],
                delay: Vec::new(),
                delay_pos: 0,
                acc: Vec::new(),
            };
            let mut v: Vec<BusSlot> = session
                .buses
                .iter()
                .map(|b| {
                    let dest = b
                        .output
                        .map_or(master_slot, |o| *slot_of.get(&o).unwrap_or(&master_slot));
                    slot(&b.inserts, b.volume_db, b.mute, dest)
                })
                .collect();
            v.push(slot(
                if master_inserts {
                    &session.master.inserts
                } else {
                    &[]
                },
                session.master.volume_db,
                session.master.mute,
                master_slot,
            ));
            v
        };

        let mut targets: Vec<MixTarget> = match stems {
            None => vec![MixTarget {
                slots: build_slots(true),
                order: order.clone(),
                master: master_slot,
                master_inserts: true,
                master_mute: session.master.mute,
                master_volume: db_to_lin(session.master.volume_db),
                include: vec![true; tracks.len()],
            }],
            Some(list) => {
                let unassigned = unassigned_stem();
                list.iter()
                    .map(|stem| MixTarget {
                        slots: build_slots(false),
                        order: order.clone(),
                        master: master_slot,
                        master_inserts: false,
                        master_mute: session.master.mute,
                        master_volume: db_to_lin(session.master.volume_db),
                        include: tracks
                            .iter()
                            .map(|t| t.stem.as_ref().unwrap_or(&unassigned) == stem)
                            .collect(),
                    })
                    .collect()
            }
        };

        // Delay compensation. Every route must reach the master output having
        // passed through the same total insert latency, and the pipeline must
        // read that far ahead of the block it emits.
        let mut lead = 0usize;
        let mut leads: Vec<Vec<usize>> = Vec::with_capacity(targets.len());
        for target in &targets {
            let mut l = vec![0usize; target.slots.len()];
            l[master_slot] = target.slots[master_slot].chain.latency;
            // `order` is farthest-first, so walking it backwards reaches every
            // slot after the slot it feeds.
            for &s in target.order.iter().rev() {
                l[s] = target.slots[s].chain.latency + l[target.slots[s].dest];
            }
            lead = lead.max(l.iter().copied().max().unwrap_or(0));
            leads.push(l);
        }
        for (target, l) in targets.iter_mut().zip(&leads) {
            for (slot, &li) in target.slots.iter_mut().zip(l) {
                let d = lead - li;
                if d > 0 {
                    slot.delay = vec![0.0; d * out_ch];
                    slot.acc = vec![0.0; BLOCK_SIZE * out_ch];
                }
            }
        }

        Ok(Renderer {
            sample_rate: sr,
            layout,
            out_ch,
            panner,
            tracks,
            targets,
            lead,
            read: 0,
            start_frame: 0,
            gather_end: i64::MAX,
            emitted: 0,
            positioned: false,
            parallel: false,
        })
    }

    /// Output channel layout.
    pub fn layout(&self) -> ChannelLayout {
        self.layout
    }

    /// Output channel count.
    pub fn channels(&self) -> usize {
        self.out_ch
    }

    /// Spread the track stage over rayon's thread pool.
    ///
    /// **Off by default, and that is a decision about deadlines rather than
    /// about throughput.** Handing 512 frames of each track to a worker pool
    /// means parking and waking those workers once per block, and the cost of
    /// waking a thread is the operating system's to decide. Measured on a
    /// four-thread machine with other work on it, that lifted the *median*
    /// block of a 48-track stereo session from 2.4 ms to 9.8 ms and its 99th
    /// percentile past 30 ms — while the total CPU spent went down. Throughput
    /// improved and the deadline was lost, which is the trade a real-time path
    /// must not take.
    ///
    /// Turn it on for an offline render, where finishing sooner is the whole
    /// objective and no block has a deadline: [`render`] and [`render_stems`]
    /// do exactly that.
    pub fn set_parallel(&mut self, on: bool) -> &mut Self {
        self.parallel = on;
        self
    }

    /// How far ahead of the block it emits the renderer reads, in frames: the
    /// deepest chain of lookahead inserts on any route to the master. The mix
    /// that comes out is aligned with the timeline, so this is not a delay the
    /// caller has to undo.
    pub fn lead_frames(&self) -> usize {
        self.lead
    }

    /// Point the renderer at `range` (seconds on the session timeline) and run
    /// the pipeline's lead-in, so the next block delivered begins at `range.0`.
    /// Returns the number of frames the range holds.
    ///
    /// Nothing at or beyond `range.1` is gathered, which is what makes a
    /// bounded render and an open-ended one agree inside the range.
    pub fn prepare(&mut self, range: (f64, f64)) -> Result<usize> {
        let total = check_range_at(self.sample_rate, range)?;
        let start = (range.0 * f64::from(self.sample_rate)).round() as i64;
        self.start_frame = start;
        self.gather_end = start + total as i64;
        self.position(start);
        Ok(total)
    }

    /// Point the renderer at `start_s` with no end, for playback — where the
    /// timeline does not stop at a window edge.
    pub fn seek(&mut self, start_s: f64) {
        let start = (start_s * f64::from(self.sample_rate)).round() as i64;
        self.start_frame = start;
        self.gather_end = i64::MAX;
        self.position(start);
    }

    fn position(&mut self, start: i64) {
        for t in self.tracks.iter_mut() {
            t.chain.reset();
        }
        for target in self.targets.iter_mut() {
            for slot in target.slots.iter_mut() {
                slot.reset();
            }
        }
        self.read = start;
        self.emitted = 0;
        self.positioned = true;

        let t0 = start as f64 / f64::from(self.sample_rate);
        let gather_end = self.gather_end;
        let panner = &self.panner;
        // Seed the automation, and run each track's own inserts in by their
        // lookahead so the first block they emit is already timeline-aligned.
        // Positioning happens once, not per block, so the pool is worth waking.
        self.tracks.par_iter_mut().for_each(|t| {
            if t.dynamic {
                let mut ctl = std::mem::take(&mut t.ctl_b);
                t.desc.sample(panner, t0, t.pan_dynamic, &mut ctl);
                t.ctl_b = ctl;
            }
            if t.silent || t.chain_latency == 0 {
                return;
            }
            let mut done = 0;
            while done < t.chain_latency {
                let n = BLOCK_SIZE.min(t.chain_latency - done);
                let src_ch = t.src_ch;
                let input = &mut t.input[..n * src_ch];
                input.fill(0.0);
                let read = start + done as i64;
                for clip in &t.clips {
                    clip.gather(read, n, gather_end, input, src_ch);
                }
                t.chain.process(input);
                done += n;
            }
        });

        // Run the pipeline's lead-in and throw it away.
        let mut done = 0;
        while done < self.lead {
            let n = BLOCK_SIZE.min(self.lead - done);
            self.push(n);
            done += n;
        }
    }

    /// Deliver the next `n` frames (`n ≤ BLOCK_SIZE`) of the mix into `out`,
    /// interleaved, `n × channels()` samples.
    pub fn render_block(&mut self, out: &mut [f32], n: usize) -> Result<()> {
        if !self.positioned {
            return Err(Error::InvalidArgument(
                "renderer has no position; call prepare() or seek() first".into(),
            ));
        }
        let need = n * self.out_ch;
        if n > BLOCK_SIZE || out.len() < need {
            return Err(Error::InvalidArgument(format!(
                "a block of {n} frames × {} channels needs {need} samples and \
                 must not exceed {BLOCK_SIZE} frames; got a slice of {}",
                self.out_ch,
                out.len()
            )));
        }
        self.push(n);
        let t = &self.targets[0];
        out[..need].copy_from_slice(&t.slots[t.master].buf[..need]);
        Ok(())
    }

    /// One block through the whole graph. Every target's master slot is left
    /// holding its `n` frames of output.
    fn push(&mut self, n: usize) {
        let out = self.out_ch;
        let sr = f64::from(self.sample_rate);
        let read = self.read;
        let gather_end = self.gather_end;
        let t1 = (self.start_frame + self.emitted + n as i64) as f64 / sr;
        let panner = &self.panner;

        // 1. Tracks. They share nothing, so this is the only place a thread
        //    pool could help; the bus stage below is a reduction over them.
        if self.parallel && self.tracks.len() > 1 {
            self.tracks
                .par_iter_mut()
                .for_each(|t| t.render_block(panner, read, n, gather_end, t1));
        } else {
            for t in self.tracks.iter_mut() {
                t.render_block(panner, read, n, gather_end, t1);
            }
        }

        // 2. Bus stage, once per mixdown.
        for target in self.targets.iter_mut() {
            for slot in target.slots.iter_mut() {
                slot.buf[..n * out].fill(0.0);
                if !slot.acc.is_empty() {
                    slot.acc[..n * out].fill(0.0);
                }
            }
            for (t, on) in self.tracks.iter().zip(&target.include) {
                if !on || t.silent {
                    continue;
                }
                add_track(&mut target.slots[t.main_slot], &t.main[..n * out]);
                for (i, slot) in t.send_slots.iter().enumerate() {
                    let Some(s) = slot else { continue };
                    let base = i * BLOCK_SIZE * out;
                    add_track(&mut target.slots[*s], &t.sends[base..base + n * out]);
                }
            }
            // Release the delayed track contributions into their slots.
            for slot in target.slots.iter_mut() {
                if slot.delay.is_empty() {
                    continue;
                }
                let d = slot.delay.len();
                let mut p = slot.delay_pos;
                for i in 0..n * out {
                    let old = slot.delay[p];
                    slot.delay[p] = slot.acc[i];
                    slot.buf[i] += old;
                    p += 1;
                    if p == d {
                        p = 0;
                    }
                }
                slot.delay_pos = p;
            }
            // Buses, farthest from the master first.
            for i in 0..target.order.len() {
                let s = target.order[i];
                let dest = target.slots[s].dest;
                if target.slots[s].mute || dest == s {
                    continue;
                }
                let vol = target.slots[s].volume;
                {
                    let slot = &mut target.slots[s];
                    if !slot.chain.is_empty() {
                        slot.chain.process(&mut slot.buf[..n * out]);
                    }
                }
                let (from, into) = if s < dest {
                    let (l, r) = target.slots.split_at_mut(dest);
                    (&l[s], &mut r[0])
                } else {
                    let (l, r) = target.slots.split_at_mut(s);
                    (&r[0], &mut l[dest])
                };
                for (d, x) in into.buf[..n * out].iter_mut().zip(&from.buf[..n * out]) {
                    *d += x * vol;
                }
            }
            // Master.
            let m = target.master;
            let master = &mut target.slots[m];
            if target.master_mute {
                master.buf[..n * out].fill(0.0);
            } else {
                if target.master_inserts && !master.chain.is_empty() {
                    master.chain.process(&mut master.buf[..n * out]);
                }
                let v = target.master_volume;
                for x in master.buf[..n * out].iter_mut() {
                    *x *= v;
                }
            }
        }

        self.read += n as i64;
        self.emitted += n as i64;
    }
}

/// Add a track's block into a slot — into its compensating delay when it has
/// one, straight into its signal when it does not.
#[inline]
fn add_track(slot: &mut BusSlot, src: &[f32]) {
    let dst = if slot.delay.is_empty() {
        &mut slot.buf[..src.len()]
    } else {
        &mut slot.acc[..src.len()]
    };
    for (d, s) in dst.iter_mut().zip(src) {
        *d += s;
    }
}

/// Compile one track: resolve its clips onto the sample grid, build its insert
/// chain, evaluate its resting pan matrix, size every buffer.
fn compile_track(
    session: &Session,
    track: &Track,
    panner: &VbapPanner,
    out_ch: usize,
    has_solo: bool,
    slot_of: &HashMap<BusId, usize>,
    master_slot: usize,
) -> TrackState {
    let sr = session.sample_rate;
    let stem = session.effective_stem(track);
    let audible = track.kind != TrackKind::Folder && !(has_solo && !track.solo);

    let mut clips: Vec<ClipPlay> = Vec::new();
    if audible {
        for clip in &track.clips {
            let Some(buf) = session.resolve_source(&clip.source) else {
                continue;
            };
            if matches!(clip.source, ClipSource::Silence) {
                continue;
            }
            // Derive the clip's extent from its two boundaries, never by
            // rounding start and duration independently: `round(start) +
            // round(duration)` is not in general `round(start + duration)`, so
            // two clips that abut exactly on the timeline would render one
            // sample apart. At 29.97 that happens at roughly two frame
            // positions in five, and every occurrence is an audible click at a
            // dialogue edit.
            let clip_start = (clip.start_s() * f64::from(sr)).round() as i64;
            let clip_end = (clip.end_s() * f64::from(sr)).round() as i64;
            let len = (clip_end - clip_start).max(0) as usize;
            if len == 0 {
                continue;
            }
            // Rate conversion belongs here, once, and not in the block loop —
            // nor, as it was, once per render call for every clip on the track.
            let buf = if buf.sample_rate != sr {
                Arc::new(buf.resample(sr))
            } else {
                buf
            };
            clips.push(ClipPlay {
                buf,
                start: clip_start,
                len,
                src_offset: (clip.source_offset_s * f64::from(sr)).round() as i64,
                gain: db_to_lin(clip.gain_db),
                fade_in: (clip.fade_in.frames(sr).min(len), clip.fade_in.curve),
                fade_out: (clip.fade_out.frames(sr).min(len), clip.fade_out.curve),
            });
        }
    }
    let silent = clips.is_empty();
    let src_ch = clips
        .iter()
        .map(|c| c.buf.channels)
        .max()
        .unwrap_or(1)
        .max(1);
    let chain = Chain::new(&track.inserts, sr, src_ch);
    let chain_latency = chain.latency;

    let lanes = Lanes {
        vol: lane_of(&track.automation, Param::Volume),
        pan: lane_of(&track.automation, Param::Pan),
        el: lane_of(&track.automation, Param::Elevation),
        spread: lane_of(&track.automation, Param::Spread),
        mute: lane_of(&track.automation, Param::Mute),
        sends: (0..track.sends.len())
            .map(|i| lane_of(&track.automation, Param::Send(i)))
            .collect(),
    };
    let pan_dynamic = lanes.pan.is_some() || lanes.el.is_some() || lanes.spread.is_some();
    let dynamic = pan_dynamic
        || lanes.vol.is_some()
        || lanes.mute.is_some()
        || lanes.sends.iter().any(Option::is_some);

    let n_sends = track.sends.len();
    let mut pan = vec![0.0f32; src_ch * out_ch];
    pan_matrix_into(
        panner,
        track.pan.law,
        track.pan.position,
        track.pan.azimuth_deg,
        track.pan.elevation_deg,
        track.pan.spread_deg,
        src_ch,
        &mut pan,
    );
    let ctl = Controls {
        vol: db_to_lin(track.volume_db),
        muted: track.mute,
        pan,
        sends: track.sends.iter().map(|s| db_to_lin(s.level_db)).collect(),
    };

    TrackState {
        desc: TrackDesc {
            law: track.pan.law,
            position: track.pan.position,
            az: track.pan.azimuth_deg,
            el: track.pan.elevation_deg,
            spread: track.pan.spread_deg,
            vol_db: track.volume_db,
            mute: track.mute,
            send_db: track.sends.iter().map(|s| s.level_db).collect(),
            send_pre: track.sends.iter().map(|s| s.pre_fader).collect(),
            src_ch,
            lanes,
        },
        stem,
        silent,
        clips,
        src_ch,
        out_ch,
        chain,
        chain_latency,
        dynamic,
        pan_dynamic,
        main_slot: *slot_of.get(&track.output).unwrap_or(&master_slot),
        send_slots: track
            .sends
            .iter()
            .map(|s| {
                slot_of
                    .get(&s.bus)
                    .copied()
                    .or(if s.bus == session.master.id {
                        Some(master_slot)
                    } else {
                        None
                    })
            })
            .collect(),
        ctl_a: ctl.clone(),
        ctl_b: ctl,
        input: vec![0.0; BLOCK_SIZE * src_ch],
        panned: vec![0.0; BLOCK_SIZE * out_ch],
        main: vec![0.0; BLOCK_SIZE * out_ch],
        sends: vec![0.0; BLOCK_SIZE * out_ch * n_sends],
    }
}

/// Bus slots ordered so that every bus comes after all buses feeding it.
fn bus_order(session: &Session, slot_of: &HashMap<BusId, usize>) -> Result<Vec<usize>> {
    let mut ranked: Vec<(usize, usize)> = Vec::with_capacity(session.buses.len());
    for b in &session.buses {
        let mut hops = 0;
        let mut cur = b.output.and_then(|o| session.bus(o));
        while let Some(next) = cur {
            hops += 1;
            if hops > session.buses.len() {
                return Err(Error::BusCycle(b.id));
            }
            cur = next.output.and_then(|o| session.bus(o));
        }
        ranked.push((hops, slot_of[&b.id]));
    }
    ranked.sort_by_key(|r| std::cmp::Reverse(r.0));
    Ok(ranked.into_iter().map(|(_, s)| s).collect())
}

fn check_range_at(sample_rate: u32, range: (f64, f64)) -> Result<usize> {
    if !range.0.is_finite() || !range.1.is_finite() || range.1 <= range.0 {
        return Err(Error::InvalidArgument(format!(
            "render range {range:?} is empty or invalid"
        )));
    }
    if sample_rate == 0 {
        return Err(Error::InvalidArgument("session sample rate is zero".into()));
    }
    Ok(((range.1 - range.0) * f64::from(sample_rate)).round() as usize)
}

/// Render `range` (seconds on the session timeline) of the session to
/// `layout`, including the master inserts.
pub fn render(session: &Session, range: (f64, f64), layout: ChannelLayout) -> Result<AudioBuffer> {
    let total = check_range_at(session.sample_rate, range)?;
    let mut r = Renderer::new(session, layout)?;
    // Offline: no block has a deadline, so spend threads on finishing sooner.
    r.set_parallel(true);
    r.prepare(range)?;
    let out = layout.channel_count();
    let mut buf = AudioBuffer::new(session.sample_rate, out, total);
    let mut pos = 0;
    while pos < total {
        let n = BLOCK_SIZE.min(total - pos);
        r.push(n);
        let t = &r.targets[0];
        buf.data[pos * out..(pos + n) * out].copy_from_slice(&t.slots[t.master].buf[..n * out]);
        pos += n;
    }
    Ok(buf)
}

/// Render each stem of the session separately (see the module docs: stems
/// are pre-master-insert, so `master_inserts(Σ stems)` is the full mix).
/// Tracks that reach the master without a stem land in [`unassigned_stem`].
pub fn render_stems(
    session: &Session,
    range: (f64, f64),
    layout: ChannelLayout,
) -> Result<HashMap<Stem, AudioBuffer>> {
    let total = check_range_at(session.sample_rate, range)?;
    let out = layout.channel_count();

    let mut stems = session.all_stems();
    let any_unassigned = session
        .tracks
        .iter()
        .any(|t| session.effective_stem(t).is_none());
    if any_unassigned && !stems.contains(&unassigned_stem()) {
        stems.push(unassigned_stem());
    }

    let mut r = Renderer::with_stems(session, layout, Some(&stems))?;
    r.set_parallel(true);
    r.prepare(range)?;
    let mut bufs: Vec<AudioBuffer> = stems
        .iter()
        .map(|_| AudioBuffer::new(session.sample_rate, out, total))
        .collect();
    let mut pos = 0;
    while pos < total {
        let n = BLOCK_SIZE.min(total - pos);
        r.push(n);
        for (buf, t) in bufs.iter_mut().zip(&r.targets) {
            buf.data[pos * out..(pos + n) * out].copy_from_slice(&t.slots[t.master].buf[..n * out]);
        }
        pos += n;
    }
    Ok(stems.into_iter().zip(bufs).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_matrix_shapes() {
        let st = VbapPanner::new(ChannelLayout::Stereo);
        let m = pan_matrix(&st, PanLaw::ConstantPower3dB, None, 0.0, 0.0, 0.0, 1);
        assert_eq!(m.len(), 1);
        assert!((m[0][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        // Stereo file at centre passes L→L, R→R.
        let m = pan_matrix(&st, PanLaw::ConstantPower3dB, None, 0.0, 0.0, 0.0, 2);
        assert!((m[0][0] - 1.0).abs() < 1e-5 && m[0][1].abs() < 1e-5);
        assert!((m[1][1] - 1.0).abs() < 1e-5 && m[1][0].abs() < 1e-5);
        // 5.1: mono at centre → C only; stereo file → L and R.
        let s51 = VbapPanner::new(ChannelLayout::Surround51);
        let m = pan_matrix(&s51, PanLaw::ConstantPower3dB, None, 0.0, 0.0, 0.0, 1);
        assert!((m[0][2] - 1.0).abs() < 1e-3, "{m:?}");
        let m = pan_matrix(&s51, PanLaw::ConstantPower3dB, None, 0.0, 0.0, 0.0, 2);
        assert!(
            (m[0][0] - 1.0).abs() < 1e-3 && (m[1][1] - 1.0).abs() < 1e-3,
            "{m:?}"
        );
        // Layout-wide signal is identity.
        let m = pan_matrix(&s51, PanLaw::ConstantPower3dB, None, 40.0, 0.0, 0.0, 6);
        assert!((m[3][3] - 1.0).abs() < 1e-6 && m[3][0] == 0.0);
        // Object at 2 m in front of a 5.1 listener: C at −6 dB (1/d rolloff).
        let m = pan_matrix(
            &s51,
            PanLaw::ConstantPower3dB,
            Some([2.0, 0.0, 0.0]),
            0.0,
            0.0,
            0.0,
            1,
        );
        assert!((m[0][2] - 0.5).abs() < 1e-3, "{m:?}");
    }

    #[test]
    fn bus_cycle_is_an_error() {
        use crate::session::Bus;
        let mut s = Session::empty(48_000, 24.0);
        let mut a = Bus::new("a");
        let mut b = Bus::new("b");
        a.output = Some(b.id);
        b.output = Some(a.id);
        s.buses.push(a);
        s.buses.push(b);
        assert!(matches!(
            render(&s, (0.0, 0.1), ChannelLayout::Stereo),
            Err(Error::BusCycle(_))
        ));
        assert!(render(&s, (1.0, 1.0), ChannelLayout::Stereo).is_err());
    }

    /// A renderer prepared once and driven block by block must produce exactly
    /// what the whole-range render produces — because it *is* the whole-range
    /// render. If they ever differ, the streaming path has become a second
    /// implementation, and one of the two is wrong.
    #[test]
    fn streaming_blocks_match_the_whole_range_render() {
        use crate::session::{Clip, Track};
        let mut s = Session::new(48_000, 24.0);
        let master = s.master_id();
        for (i, f) in [220.0f64, 440.0, 933.0].into_iter().enumerate() {
            let buf = Arc::new(AudioBuffer::sine(48_000, 1, f, 0.4, 2.0));
            let mut t = Track::new(format!("t{i}"), master)
                .with_clip(Clip::from_buffer("c", buf, 0.0, 24.0));
            t.pan.azimuth_deg = -20.0 + 20.0 * i as f32;
            t.volume_db = -4.0;
            s.add_track(t);
        }
        let range = (0.0, 1.0);
        let whole = render(&s, range, ChannelLayout::Stereo).unwrap();
        let mut r = Renderer::new(&s, ChannelLayout::Stereo).unwrap();
        let total = r.prepare(range).unwrap();
        let mut streamed = Vec::with_capacity(total * 2);
        let mut block = vec![0.0f32; BLOCK_SIZE * 2];
        let mut pos = 0;
        while pos < total {
            let n = BLOCK_SIZE.min(total - pos);
            r.render_block(&mut block, n).unwrap();
            streamed.extend_from_slice(&block[..n * 2]);
            pos += n;
        }
        assert_eq!(streamed.len(), whole.data.len());
        let worst = whole
            .data
            .iter()
            .zip(&streamed)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst == 0.0,
            "streaming and whole-range renders differ by {worst}"
        );
    }

    /// The lead-in must leave the mix aligned with the timeline whatever the
    /// master limiter's lookahead is, and rendering a window must not depend on
    /// where in the timeline it starts.
    #[test]
    fn a_window_renders_the_same_wherever_it_is_asked_for() {
        use crate::session::{Clip, Track};
        let mut s = Session::new(48_000, 24.0);
        let master = s.master_id();
        let buf = Arc::new(AudioBuffer::sine(48_000, 1, 617.0, 0.5, 4.0));
        s.add_track(Track::new("t", master).with_clip(Clip::from_buffer("c", buf, 0.0, 24.0)));
        // The whole second, and the same second cut out of a longer render.
        let one = render(&s, (1.0, 2.0), ChannelLayout::Stereo).unwrap();
        let three = render(&s, (0.0, 3.0), ChannelLayout::Stereo).unwrap();
        let sr = 48_000usize;
        let worst = one
            .data
            .iter()
            .zip(&three.data[sr * 2..sr * 4])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        // Not bit-identical: the limiter's state at the window's start depends
        // on what came before it, which is the point of a limiter. What must
        // hold is alignment — a shift would show up as a large difference on a
        // 617 Hz tone, where one sample of slip is 0.04 of full scale.
        assert!(
            worst < 0.01,
            "window is not aligned: worst difference {worst}"
        );
    }
}
