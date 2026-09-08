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

use crate::buffer::{db_to_lin, AudioBuffer};
use crate::dsp::{Biquad, Compressor, FadeCurve, Gate, Limiter};
use crate::error::{Error, Result};
use crate::pan::{ChannelLayout, ObjectPanParams, PanLaw, VbapPanner};
use crate::session::{BusId, ClipSource, Insert, Param, Session, Stem, Track, TrackKind};
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
                        sample_rate as f64,
                        *freq_hz as f64,
                        *q as f64,
                        *gain_db as f64
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

    /// Process one interleaved block in place (output delayed by `latency`).
    fn process(&mut self, block: &mut [f32]) {
        let ch = self.channels;
        for p in &mut self.procs {
            match p {
                Proc::Biquad(filters) => {
                    for frame in block.chunks_mut(ch) {
                        for (s, f) in frame.iter_mut().zip(filters.iter_mut()) {
                            *s = f.process_sample(*s);
                        }
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

    /// Process a whole buffer with latency compensation.
    fn run_whole(&mut self, buf: &mut AudioBuffer) {
        let ch = self.channels;
        if self.latency == 0 {
            self.process(&mut buf.data);
            return;
        }
        let frames = buf.frames();
        buf.data
            .extend(std::iter::repeat_n(0.0f32, self.latency * ch));
        self.process(&mut buf.data);
        buf.data.drain(..self.latency * ch);
        buf.data.truncate(frames * ch);
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
fn mono_gains(panner: &VbapPanner, law: PanLaw, az: f32, el: f32, spread: f32) -> Vec<f32> {
    if panner.layout() == ChannelLayout::Stereo {
        let (l, r) = law.gains((-az / 90.0).clamp(-1.0, 1.0));
        vec![l, r]
    } else {
        panner.gains_with_spread(az, el, spread)
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
    let layout = panner.layout();
    let out = layout.channel_count();
    let src_ch = src_ch.max(1);
    if let Some(pos) = position {
        let g = panner.pan_object(
            pos,
            &ObjectPanParams {
                spread_deg: spread,
                ..Default::default()
            },
        );
        let scale = 1.0 / (src_ch as f32).sqrt();
        return (0..src_ch)
            .map(|_| g.iter().map(|x| x * scale).collect())
            .collect();
    }
    if src_ch == 1 {
        return vec![mono_gains(panner, law, az, el, spread)];
    }
    if src_ch == out {
        return (0..src_ch)
            .map(|c| (0..out).map(|o| if o == c { 1.0 } else { 0.0 }).collect())
            .collect();
    }
    if layout == ChannelLayout::Mono {
        let scale = 1.0 / src_ch as f32;
        return (0..src_ch).map(|_| vec![scale]).collect();
    }
    let width = if layout == ChannelLayout::Stereo {
        90.0
    } else {
        30.0
    };
    (0..src_ch)
        .map(|c| {
            let offset = width * (1.0 - 2.0 * c as f32 / (src_ch - 1) as f32);
            mono_gains(panner, law, az + offset, el, spread)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Track rendering
// ---------------------------------------------------------------------------

struct ClipPlay {
    buf: Arc<AudioBuffer>,
    /// Timeline frame (relative to the render start) of the clip's first frame.
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
}

/// Everything a track contributes to the bus stage.
struct TrackRender {
    stem: Option<Stem>,
    output: BusId,
    main: AudioBuffer,
    /// `(bus, signal)` per send.
    sends: Vec<(BusId, AudioBuffer)>,
}

/// Block-boundary control values.
#[derive(Clone)]
struct Controls {
    vol: f32,
    muted: bool,
    pan: Vec<Vec<f32>>,
    sends: Vec<f32>,
}

fn controls_at(track: &Track, panner: &VbapPanner, src_ch: usize, t: f64) -> Controls {
    let a = &track.automation;
    let vol_db = a
        .value_at(Param::Volume, t)
        .map(|v| v as f32)
        .unwrap_or(track.volume_db);
    let az = a
        .value_at(Param::Pan, t)
        .map(|v| v as f32)
        .unwrap_or(track.pan.azimuth_deg);
    let el = a
        .value_at(Param::Elevation, t)
        .map(|v| v as f32)
        .unwrap_or(track.pan.elevation_deg);
    let spread = a
        .value_at(Param::Spread, t)
        .map(|v| v as f32)
        .unwrap_or(track.pan.spread_deg);
    let muted = a
        .value_at(Param::Mute, t)
        .map(|v| v > 0.5)
        .unwrap_or(track.mute);
    let sends = track
        .sends
        .iter()
        .enumerate()
        .map(|(i, s)| {
            db_to_lin(
                a.value_at(Param::Send(i), t)
                    .map(|v| v as f32)
                    .unwrap_or(s.level_db),
            )
        })
        .collect();
    Controls {
        vol: db_to_lin(vol_db),
        muted,
        pan: pan_matrix(
            panner,
            track.pan.law,
            track.pan.position,
            az,
            el,
            spread,
            src_ch,
        ),
        sends,
    }
}

fn render_track(
    session: &Session,
    track: &Track,
    panner: &VbapPanner,
    start_s: f64,
    total: usize,
) -> Result<TrackRender> {
    let sr = session.sample_rate;
    let out = panner.layout().channel_count();
    let silent = |sends: bool| TrackRender {
        stem: session.effective_stem(track),
        output: track.output,
        main: AudioBuffer::new(sr, out, total),
        sends: if sends {
            track
                .sends
                .iter()
                .map(|s| (s.bus, AudioBuffer::new(sr, out, total)))
                .collect()
        } else {
            Vec::new()
        },
    };
    let audible = track.kind != TrackKind::Folder && !(session.has_solo() && !track.solo);
    if !audible {
        return Ok(silent(false));
    }

    // Resolve the clips that overlap the range (placeholders contribute nothing).
    let start_frame = (start_s * sr as f64).round() as i64;
    let mut clips: Vec<ClipPlay> = Vec::new();
    for clip in &track.clips {
        let Some(buf) = session.resolve_source(&clip.source) else {
            continue;
        };
        if matches!(clip.source, ClipSource::Silence) {
            continue;
        }
        let len = (clip.duration_s() * sr as f64).round() as usize;
        let start = (clip.start_s() * sr as f64).round() as i64 - start_frame;
        if len == 0 || start >= total as i64 || start + len as i64 <= 0 {
            continue;
        }
        let buf = if buf.sample_rate != sr {
            Arc::new(buf.resample(sr))
        } else {
            buf
        };
        clips.push(ClipPlay {
            buf,
            start,
            len,
            src_offset: (clip.source_offset_s * sr as f64).round() as i64,
            gain: db_to_lin(clip.gain_db),
            fade_in: (clip.fade_in.frames(sr).min(len), clip.fade_in.curve),
            fade_out: (clip.fade_out.frames(sr).min(len), clip.fade_out.curve),
        });
    }
    if clips.is_empty() {
        return Ok(silent(true));
    }
    let src_ch = clips
        .iter()
        .map(|c| c.buf.channels)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut chain = Chain::new(&track.inserts, sr, src_ch);
    let latency = chain.latency;
    let capacity = total + latency;
    let mut main = AudioBuffer::new(sr, out, capacity);
    let mut sends: Vec<(BusId, AudioBuffer)> = track
        .sends
        .iter()
        .map(|s| (s.bus, AudioBuffer::new(sr, out, capacity)))
        .collect();

    let automated = !track.automation.is_empty();
    let time_at = |frame: usize| start_s + frame as f64 / sr as f64;
    let mut ctl0 = controls_at(track, panner, src_ch, time_at(0));
    let mut input = vec![0.0f32; BLOCK_SIZE * src_ch];
    let mut panned = vec![0.0f32; BLOCK_SIZE * out];

    // One block: gather, process, pan, distribute. `pos` is the output frame.
    let mut run_block = |pos: usize,
                         n: usize,
                         gather: bool,
                         ctl0: &Controls,
                         ctl1: &Controls,
                         chain: &mut Chain,
                         main: &mut AudioBuffer,
                         sends: &mut [(BusId, AudioBuffer)]| {
        let input = &mut input[..n * src_ch];
        input.fill(0.0);
        if gather {
            for clip in &clips {
                let cb = &clip.buf;
                let cch = cb.channels;
                let frames = cb.frames();
                let lo = (clip.start.max(pos as i64)) as usize;
                let hi = ((clip.start + clip.len as i64).min((pos + n) as i64)) as usize;
                for f in lo..hi {
                    let k = (f as i64 - clip.start) as usize;
                    let src = clip.src_offset + k as i64;
                    if src < 0 || src as usize >= frames {
                        continue;
                    }
                    let g = clip.gain_at(k);
                    let frame = &cb.data[src as usize * cch..(src as usize + 1) * cch];
                    let dst = &mut input[(f - pos) * src_ch..(f - pos + 1) * src_ch];
                    if cch == 1 {
                        for d in dst.iter_mut() {
                            *d += frame[0] * g;
                        }
                    } else {
                        for (d, s) in dst.iter_mut().zip(frame) {
                            *d += s * g;
                        }
                    }
                }
            }
        }
        chain.process(input);
        let panned = &mut panned[..n * out];
        let inv = if n > 1 { 1.0 / (n - 1) as f32 } else { 0.0 };
        for f in 0..n {
            let u = f as f32 * inv;
            let frame_in = &input[f * src_ch..(f + 1) * src_ch];
            let frame_out = &mut panned[f * out..(f + 1) * out];
            frame_out.fill(0.0);
            for (c, s) in frame_in.iter().enumerate() {
                if *s == 0.0 {
                    continue;
                }
                let g0 = &ctl0.pan[c];
                let g1 = &ctl1.pan[c];
                for o in 0..out {
                    frame_out[o] += s * (g0[o] + (g1[o] - g0[o]) * u);
                }
            }
        }
        let mute = if ctl0.muted { 0.0 } else { 1.0 };
        for f in 0..n {
            let u = f as f32 * inv;
            let vol = (ctl0.vol + (ctl1.vol - ctl0.vol) * u) * mute;
            let src = &panned[f * out..(f + 1) * out];
            let base = (pos + f) * out;
            for (o, x) in src.iter().enumerate() {
                main.data[base + o] += x * vol;
            }
            for (i, (_, buf)) in sends.iter_mut().enumerate() {
                let lvl = ctl0.sends[i] + (ctl1.sends[i] - ctl0.sends[i]) * u;
                let lvl = if track.sends[i].pre_fader {
                    lvl * mute
                } else {
                    lvl * vol
                };
                if lvl == 0.0 {
                    continue;
                }
                for (o, x) in src.iter().enumerate() {
                    buf.data[base + o] += x * lvl;
                }
            }
        }
    };

    let mut pos = 0;
    while pos < total {
        let n = BLOCK_SIZE.min(total - pos);
        let ctl1 = if automated {
            controls_at(track, panner, src_ch, time_at(pos + n))
        } else {
            ctl0.clone()
        };
        run_block(
            pos, n, true, &ctl0, &ctl1, &mut chain, &mut main, &mut sends,
        );
        ctl0 = ctl1;
        pos += n;
    }
    if latency > 0 {
        // Flush the lookahead tail, then realign.
        let mut done = 0;
        while done < latency {
            let n = BLOCK_SIZE.min(latency - done);
            run_block(
                total + done,
                n,
                false,
                &ctl0,
                &ctl0,
                &mut chain,
                &mut main,
                &mut sends,
            );
            done += n;
        }
        main.data.drain(..latency * out);
        for (_, s) in sends.iter_mut() {
            s.data.drain(..latency * out);
        }
    }
    Ok(TrackRender {
        stem: session.effective_stem(track),
        output: track.output,
        main,
        sends,
    })
}

// ---------------------------------------------------------------------------
// Bus stage
// ---------------------------------------------------------------------------

/// Buses ordered so that every bus comes after all buses feeding it.
fn bus_order(session: &Session) -> Result<Vec<BusId>> {
    let mut ranked: Vec<(usize, BusId)> = Vec::with_capacity(session.buses.len());
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
        ranked.push((hops, b.id));
    }
    ranked.sort_by_key(|r| std::cmp::Reverse(r.0));
    Ok(ranked.into_iter().map(|(_, id)| id).collect())
}

fn add_into(dst: &mut AudioBuffer, src: &AudioBuffer) {
    for (d, s) in dst.data.iter_mut().zip(&src.data) {
        *d += s;
    }
}

fn mix_down(
    session: &Session,
    renders: &[TrackRender],
    include: &[bool],
    layout: ChannelLayout,
    total: usize,
    master_inserts: bool,
) -> Result<AudioBuffer> {
    let sr = session.sample_rate;
    let out = layout.channel_count();
    let master = session.master_id();
    let order = bus_order(session)?;
    let mut inputs: HashMap<BusId, AudioBuffer> = session
        .buses
        .iter()
        .map(|b| (b.id, AudioBuffer::new(sr, out, total)))
        .collect();
    inputs.insert(master, AudioBuffer::new(sr, out, total));
    let route = |id: BusId| {
        if session.bus(id).is_some() {
            id
        } else {
            master
        }
    };

    for (r, on) in renders.iter().zip(include) {
        if !on {
            continue;
        }
        if let Some(dst) = inputs.get_mut(&route(r.output)) {
            add_into(dst, &r.main);
        }
        for (bus, sig) in &r.sends {
            if *bus == master || session.bus(*bus).is_some() {
                if let Some(dst) = inputs.get_mut(bus) {
                    add_into(dst, sig);
                }
            }
        }
    }

    for id in order {
        let Some(bus) = session.bus(id) else { continue };
        let Some(mut sig) = inputs.remove(&id) else {
            continue;
        };
        if bus.mute {
            continue;
        }
        Chain::new(&bus.inserts, sr, out).run_whole(&mut sig);
        sig.gain(db_to_lin(bus.volume_db));
        let dst = route(bus.output.unwrap_or(master));
        if let Some(d) = inputs.get_mut(&dst) {
            add_into(d, &sig);
        }
    }

    let mut mix = inputs
        .remove(&master)
        .unwrap_or_else(|| AudioBuffer::new(sr, out, total));
    if session.master.mute {
        mix.data.fill(0.0);
        return Ok(mix);
    }
    if master_inserts {
        Chain::new(&session.master.inserts, sr, out).run_whole(&mut mix);
    }
    mix.gain(db_to_lin(session.master.volume_db));
    Ok(mix)
}

fn check_range(session: &Session, range: (f64, f64)) -> Result<usize> {
    if !range.0.is_finite() || !range.1.is_finite() || range.1 <= range.0 {
        return Err(Error::InvalidArgument(format!(
            "render range {:?} is empty or invalid",
            range
        )));
    }
    if session.sample_rate == 0 {
        return Err(Error::InvalidArgument("session sample rate is zero".into()));
    }
    Ok(((range.1 - range.0) * session.sample_rate as f64).round() as usize)
}

fn render_tracks(
    session: &Session,
    range: (f64, f64),
    layout: ChannelLayout,
    total: usize,
) -> Result<Vec<TrackRender>> {
    let panner = VbapPanner::new(layout);
    session
        .tracks
        .par_iter()
        .map(|t| render_track(session, t, &panner, range.0, total))
        .collect()
}

/// Render `range` (seconds on the session timeline) of the session to
/// `layout`, including the master inserts.
pub fn render(session: &Session, range: (f64, f64), layout: ChannelLayout) -> Result<AudioBuffer> {
    let total = check_range(session, range)?;
    let renders = render_tracks(session, range, layout, total)?;
    let all = vec![true; renders.len()];
    mix_down(session, &renders, &all, layout, total, true)
}

/// Render each stem of the session separately (see the module docs: stems
/// are pre-master-insert, so `master_inserts(Σ stems)` is the full mix).
/// Tracks that reach the master without a stem land in [`unassigned_stem`].
pub fn render_stems(
    session: &Session,
    range: (f64, f64),
    layout: ChannelLayout,
) -> Result<HashMap<Stem, AudioBuffer>> {
    let total = check_range(session, range)?;
    let renders = render_tracks(session, range, layout, total)?;
    let mut stems = session.all_stems();
    if renders.iter().any(|r| r.stem.is_none()) && !stems.contains(&unassigned_stem()) {
        stems.push(unassigned_stem());
    }
    let unassigned = unassigned_stem();
    let mut out = HashMap::with_capacity(stems.len());
    for stem in stems {
        let include: Vec<bool> = renders
            .iter()
            .map(|r| r.stem.as_ref().unwrap_or(&unassigned) == &stem)
            .collect();
        out.insert(
            stem,
            mix_down(session, &renders, &include, layout, total, false)?,
        );
    }
    Ok(out)
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
}
