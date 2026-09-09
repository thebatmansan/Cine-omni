//! Interleaved multi-channel audio buffers with metering and resampling.
//!
//! [`AudioBuffer`] is the one sample container used across the crate: clips,
//! stems, renders and analysis all use it. Samples are `f32`, interleaved
//! frame-major (`[L0, R0, L1, R1, …]`).

use crate::dsp::Biquad;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Interleaved multi-channel PCM audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioBuffer {
    /// Samples per second.
    pub sample_rate: u32,
    /// Number of interleaved channels (≥ 1).
    pub channels: usize,
    /// Interleaved samples; `data.len() == frames * channels`.
    pub data: Vec<f32>,
}

impl Default for AudioBuffer {
    fn default() -> Self {
        AudioBuffer {
            sample_rate: 48_000,
            channels: 1,
            data: Vec::new(),
        }
    }
}

impl AudioBuffer {
    /// A silent buffer of `frames` frames.
    pub fn new(sample_rate: u32, channels: usize, frames: usize) -> Self {
        let channels = channels.max(1);
        AudioBuffer {
            sample_rate,
            channels,
            data: vec![0.0; frames * channels],
        }
    }

    /// Wrap interleaved samples. `data.len()` must be a multiple of `channels`.
    pub fn from_interleaved(sample_rate: u32, channels: usize, data: Vec<f32>) -> Result<Self> {
        let channels = channels.max(1);
        if data.len() % channels != 0 {
            return Err(Error::Mismatch(format!(
                "interleaved data of {} samples is not a multiple of {} channels",
                data.len(),
                channels
            )));
        }
        Ok(AudioBuffer {
            sample_rate,
            channels,
            data,
        })
    }

    /// Build from planar channels (all planes must have equal length).
    pub fn from_planar(sample_rate: u32, planes: &[Vec<f32>]) -> Result<Self> {
        if planes.is_empty() {
            return Err(Error::InvalidArgument("from_planar: no channels".into()));
        }
        let frames = planes[0].len();
        if planes.iter().any(|p| p.len() != frames) {
            return Err(Error::Mismatch(
                "from_planar: channel lengths differ".into(),
            ));
        }
        let channels = planes.len();
        let mut data = vec![0.0; frames * channels];
        for (c, plane) in planes.iter().enumerate() {
            for (i, s) in plane.iter().enumerate() {
                data[i * channels + c] = *s;
            }
        }
        Ok(AudioBuffer {
            sample_rate,
            channels,
            data,
        })
    }

    /// A test-signal sine wave with peak amplitude `amplitude` on every channel.
    pub fn sine(
        sample_rate: u32,
        channels: usize,
        freq_hz: f64,
        amplitude: f32,
        seconds: f64,
    ) -> Self {
        Self::sine_with_phase(sample_rate, channels, freq_hz, amplitude, seconds, 0.0)
    }

    /// A sine wave with an initial phase (radians).
    pub fn sine_with_phase(
        sample_rate: u32,
        channels: usize,
        freq_hz: f64,
        amplitude: f32,
        seconds: f64,
        phase: f64,
    ) -> Self {
        let frames = (seconds * sample_rate as f64).round() as usize;
        let mut b = AudioBuffer::new(sample_rate, channels, frames);
        for i in 0..frames {
            let v = (2.0 * PI * freq_hz * i as f64 / sample_rate as f64 + phase).sin() as f32
                * amplitude;
            for c in 0..b.channels {
                b.data[i * b.channels + c] = v;
            }
        }
        b
    }

    /// Deterministic white noise (uniform in `[-amplitude, amplitude]`) from a
    /// xorshift generator; useful for tests and noise profiles.
    pub fn noise(
        sample_rate: u32,
        channels: usize,
        amplitude: f32,
        seconds: f64,
        seed: u64,
    ) -> Self {
        let frames = (seconds * sample_rate as f64).round() as usize;
        let mut b = AudioBuffer::new(sample_rate, channels, frames);
        let mut state = seed.max(1);
        for s in b.data.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let u = (state >> 11) as f64 / (1u64 << 53) as f64; // [0,1)
            *s = ((u * 2.0 - 1.0) as f32) * amplitude;
        }
        b
    }

    /// Number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.data.len().checked_div(self.channels).unwrap_or(0)
    }

    /// Duration in seconds.
    pub fn duration_s(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.sample_rate as f64
        }
    }

    /// True when there are no frames.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// One interleaved frame.
    pub fn frame(&self, i: usize) -> &[f32] {
        &self.data[i * self.channels..(i + 1) * self.channels]
    }

    /// Sample at `(frame, channel)`; zero outside the buffer.
    pub fn sample(&self, frame: usize, channel: usize) -> f32 {
        if channel >= self.channels {
            return 0.0;
        }
        self.data
            .get(frame * self.channels + channel)
            .copied()
            .unwrap_or(0.0)
    }

    /// Mutable sample reference (panics outside the buffer).
    pub fn sample_mut(&mut self, frame: usize, channel: usize) -> &mut f32 {
        let ch = self.channels;
        &mut self.data[frame * ch + channel]
    }

    /// Copy of one channel as a planar vector.
    pub fn channel(&self, c: usize) -> Vec<f32> {
        if c >= self.channels {
            return vec![0.0; self.frames()];
        }
        self.data
            .iter()
            .skip(c)
            .step_by(self.channels)
            .copied()
            .collect()
    }

    /// All channels as planar vectors.
    pub fn to_planar(&self) -> Vec<Vec<f32>> {
        (0..self.channels).map(|c| self.channel(c)).collect()
    }

    /// Replace one channel from a planar slice (length must match).
    pub fn set_channel(&mut self, c: usize, plane: &[f32]) -> Result<()> {
        if c >= self.channels || plane.len() != self.frames() {
            return Err(Error::Mismatch(
                "set_channel: index or length mismatch".into(),
            ));
        }
        let ch = self.channels;
        for (i, s) in plane.iter().enumerate() {
            self.data[i * ch + c] = *s;
        }
        Ok(())
    }

    /// Average of all channels as a mono buffer.
    pub fn to_mono(&self) -> AudioBuffer {
        let ch = self.channels;
        let frames = self.frames();
        let mut out = AudioBuffer::new(self.sample_rate, 1, frames);
        let inv = 1.0 / ch as f32;
        for i in 0..frames {
            out.data[i] = self.frame(i).iter().sum::<f32>() * inv;
        }
        out
    }

    /// A copy with the channel count changed: mono is duplicated to every
    /// channel, extra source channels are dropped, missing ones are silent.
    pub fn with_channels(&self, channels: usize) -> AudioBuffer {
        let channels = channels.max(1);
        if channels == self.channels {
            return self.clone();
        }
        let frames = self.frames();
        let mut out = AudioBuffer::new(self.sample_rate, channels, frames);
        for i in 0..frames {
            for c in 0..channels {
                let src = if self.channels == 1 { 0 } else { c };
                out.data[i * channels + c] = self.sample(i, src);
            }
        }
        out
    }

    /// Sub-range copy (clamped to the buffer).
    pub fn slice(&self, start_frame: usize, frames: usize) -> AudioBuffer {
        let total = self.frames();
        let start = start_frame.min(total);
        let end = (start + frames).min(total);
        AudioBuffer {
            sample_rate: self.sample_rate,
            channels: self.channels,
            data: self.data[start * self.channels..end * self.channels].to_vec(),
        }
    }

    /// Append frames (rate and channel count must match).
    pub fn append(&mut self, other: &AudioBuffer) -> Result<()> {
        if other.channels != self.channels || other.sample_rate != self.sample_rate {
            return Err(Error::Mismatch("append: rate or channel mismatch".into()));
        }
        self.data.extend_from_slice(&other.data);
        Ok(())
    }

    /// Add `self * gain` into `dst` starting at frame `dst_offset`, growing
    /// `dst` if needed. Mono sources are spread to every destination channel;
    /// otherwise channel counts must match.
    pub fn mix_into(&self, dst: &mut AudioBuffer, dst_offset: usize, gain: f32) -> Result<()> {
        if self.sample_rate != dst.sample_rate {
            return Err(Error::Mismatch(format!(
                "mix_into: sample rate {} != {}",
                self.sample_rate, dst.sample_rate
            )));
        }
        if self.channels != 1 && self.channels != dst.channels {
            return Err(Error::Mismatch(format!(
                "mix_into: {} channels into {}",
                self.channels, dst.channels
            )));
        }
        let needed = (dst_offset + self.frames()) * dst.channels;
        if dst.data.len() < needed {
            dst.data.resize(needed, 0.0);
        }
        let dch = dst.channels;
        if self.channels == 1 {
            for (i, s) in self.data.iter().enumerate() {
                let base = (dst_offset + i) * dch;
                for c in 0..dch {
                    dst.data[base + c] += s * gain;
                }
            }
        } else {
            let base = dst_offset * dch;
            for (d, s) in dst.data[base..base + self.data.len()]
                .iter_mut()
                .zip(&self.data)
            {
                *d += s * gain;
            }
        }
        Ok(())
    }

    /// Multiply every sample by a linear gain.
    pub fn gain(&mut self, g: f32) {
        for s in &mut self.data {
            *s *= g;
        }
    }

    /// Multiply every sample by a gain in decibels.
    pub fn gain_db(&mut self, db: f32) {
        self.gain(db_to_lin(db));
    }

    /// RMS over all channels (linear).
    pub fn rms(&self) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.data.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        (sum / self.data.len() as f64).sqrt() as f32
    }

    /// RMS of a single channel.
    pub fn rms_channel(&self, c: usize) -> f32 {
        let frames = self.frames();
        if frames == 0 || c >= self.channels {
            return 0.0;
        }
        let sum: f64 = (0..frames)
            .map(|i| {
                let s = self.data[i * self.channels + c] as f64;
                s * s
            })
            .sum();
        (sum / frames as f64).sqrt() as f32
    }

    /// RMS in dBFS.
    pub fn rms_db(&self) -> f32 {
        lin_to_db(self.rms())
    }

    /// Sample peak (absolute, linear).
    pub fn peak(&self) -> f32 {
        self.data.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Sample peak in dBFS.
    pub fn peak_db(&self) -> f32 {
        lin_to_db(self.peak())
    }

    /// Sample-rate convert with a 16-tap Hann-windowed sinc interpolator.
    /// When downsampling, the sinc cutoff is lowered to the new Nyquist so
    /// aliasing is suppressed.
    pub fn resample(&self, target_rate: u32) -> AudioBuffer {
        if target_rate == self.sample_rate || self.sample_rate == 0 || target_rate == 0 {
            let mut b = self.clone();
            b.sample_rate = if target_rate == 0 {
                self.sample_rate
            } else {
                target_rate
            };
            return b;
        }
        let ratio = self.sample_rate as f64 / target_rate as f64; // input frames per output frame
        let in_frames = self.frames();
        let out_frames = ((in_frames as f64) / ratio).round() as usize;
        let cutoff = if ratio > 1.0 { 1.0 / ratio } else { 1.0 };
        const TAPS: i64 = 16;
        const HALF: i64 = TAPS / 2;
        let mut out = AudioBuffer::new(target_rate, self.channels, out_frames);
        let ch = self.channels;
        let mut weights = [0.0f64; TAPS as usize];
        for o in 0..out_frames {
            let t = o as f64 * ratio;
            let center = t.floor() as i64;
            let frac = t - center as f64;
            // taps k = -HALF+1 ..= HALF, relative to `center`
            let mut wsum = 0.0;
            for (wi, w) in weights.iter_mut().enumerate() {
                let k = wi as i64 - HALF + 1;
                let x = k as f64 - frac; // distance from the tap to the sample point
                let sinc = if x.abs() < 1e-9 {
                    1.0
                } else {
                    (PI * cutoff * x).sin() / (PI * cutoff * x)
                };
                let win = 0.5 * (1.0 + (PI * x / HALF as f64).cos());
                let win = if x.abs() >= HALF as f64 { 0.0 } else { win };
                *w = sinc * cutoff * win;
                wsum += *w;
            }
            let norm = if wsum.abs() > 1e-12 { 1.0 / wsum } else { 1.0 };
            for c in 0..ch {
                let mut acc = 0.0f64;
                for (wi, w) in weights.iter().enumerate() {
                    let idx = center + wi as i64 - HALF + 1;
                    if idx >= 0 && (idx as usize) < in_frames {
                        acc += *w * self.data[idx as usize * ch + c] as f64;
                    }
                }
                out.data[o * ch + c] = (acc * norm) as f32;
            }
        }
        out
    }

    /// Integrated loudness in LUFS per ITU-R BS.1770-4 (K-weighting, 400 ms
    /// blocks with 75 % overlap, −70 LKFS absolute gate, −10 LU relative gate).
    /// Channel weights follow the standard for up to 5.1 (`L R C LFE Ls Rs`);
    /// extra channels are weighted 1.0 and the LFE channel (index 3 of a ≥6
    /// channel buffer) is ignored. Returns `-inf` for silence.
    pub fn lufs_integrated(&self) -> f64 {
        let fs = self.sample_rate as f64;
        let frames = self.frames();
        if frames == 0 || fs <= 0.0 {
            return f64::NEG_INFINITY;
        }
        let block = (0.4 * fs).round() as usize;
        let hop = (0.1 * fs).round() as usize;
        if block == 0 || hop == 0 {
            return f64::NEG_INFINITY;
        }
        // K-weight every channel.
        let planes: Vec<Vec<f32>> = (0..self.channels)
            .map(|c| {
                let mut plane = self.channel(c);
                let mut shelf = k_weight_shelf(fs);
                let mut hp = k_weight_highpass(fs);
                shelf.process(&mut plane);
                hp.process(&mut plane);
                plane
            })
            .collect();
        let weights: Vec<f64> = (0..self.channels)
            .map(|c| match (self.channels, c) {
                (n, 3) if n >= 6 => 0.0,
                (n, 4) | (n, 5) if n >= 6 => 1.41,
                _ => 1.0,
            })
            .collect();
        // Block loudness.
        let mut blocks: Vec<(f64, Vec<f64>)> = Vec::new(); // (loudness, per-channel mean square)
        if frames < block {
            // Shorter than one gating block: measure it as one block.
            let z: Vec<f64> = planes.iter().map(|p| mean_square(p)).collect();
            blocks.push((block_loudness(&z, &weights), z));
        } else {
            let mut start = 0;
            while start + block <= frames {
                let z: Vec<f64> = planes
                    .iter()
                    .map(|p| mean_square(&p[start..start + block]))
                    .collect();
                blocks.push((block_loudness(&z, &weights), z));
                start += hop;
            }
        }
        // Absolute gate.
        let abs_gated: Vec<&(f64, Vec<f64>)> = blocks.iter().filter(|(l, _)| *l > -70.0).collect();
        if abs_gated.is_empty() {
            return f64::NEG_INFINITY;
        }
        let mean_z = |set: &[&(f64, Vec<f64>)]| -> Vec<f64> {
            let n = set.len() as f64;
            (0..self.channels)
                .map(|c| set.iter().map(|(_, z)| z[c]).sum::<f64>() / n)
                .collect()
        };
        let relative_threshold = block_loudness(&mean_z(&abs_gated), &weights) - 10.0;
        let rel_gated: Vec<&(f64, Vec<f64>)> = abs_gated
            .iter()
            .copied()
            .filter(|(l, _)| *l > relative_threshold)
            .collect();
        if rel_gated.is_empty() {
            return f64::NEG_INFINITY;
        }
        block_loudness(&mean_z(&rel_gated), &weights)
    }

    /// True-peak level in dBTP per ITU-R BS.1770 Annex 2 — oversample,
    /// rectify, take the maximum. (Annex 2 is unchanged by the -5 revision of
    /// November 2023, which only added Annex 4, for object-based audio; the
    /// true-peak algorithm has not moved since -4.)
    ///
    /// The signal is oversampled to at least 384 kHz: 8× at 44.1 and 48 kHz,
    /// 4× at 96 kHz. Annex 2 puts the floor at 192 kHz, but 192 kHz is not
    /// enough on its own — a peak can land half a grid step away from every
    /// grid point, which on a 20 kHz tone costs
    /// `20·log10(cos(π·20000/192000))` = 0.47 dB of under-read from grid
    /// coarseness alone, past the −0.4 dBTP EBU Tech 3341 allows for the whole
    /// meter. Doubling the grid takes the measured worst case to 0.09 dB.
    /// Attachment 1 to Annex 2 says the same thing: higher ratios are
    /// preferred.
    ///
    /// The interpolator is a 16-tap Hann-windowed sinc. Annex 2's tabulated
    /// order-48 filter is *one* set of coefficients that "would satisfy the
    /// requirements", and the Recommendation admits any method giving similar
    /// or superior results — so the claim is only worth what it is measured
    /// against. `tests/true_peak_oracle.rs` measures it against the exact
    /// Whittaker–Shannon reconstruction of the samples, which is what true
    /// peak *is*, rather than against another meter.
    ///
    /// **Measured error** against that reconstruction: within **0.05 dB** on
    /// programme material band-limited to 20 kHz, and within **0.10 dB** on a
    /// worst-case-phase tone at any frequency up to 20 kHz. On material
    /// carrying energy right up to Nyquist it **under-reads by up to 1.5 dB**,
    /// as every oversampling meter does — the shortfall Attachment 1
    /// describes. A reading on deliberately full-band material is not a
    /// delivery pass.
    pub fn true_peak_dbtp(&self) -> f32 {
        let frames = self.frames();
        if frames == 0 || self.sample_rate == 0 {
            return f32::NEG_INFINITY;
        }
        // Phase 0 of the interpolator is a unit impulse, so the sample peak is
        // already that phase's answer: seed the search with it and skip it.
        let mut best = self.peak();
        let factor = true_peak_oversampling(self.sample_rate);
        if factor < 2 {
            return lin_to_db(best);
        }
        let phases = true_peak_phases(factor);
        // Kernel L1 norm: |y| can never exceed it times the largest sample the
        // kernel can reach, which is what lets whole blocks be skipped.
        let l1 = phases
            .iter()
            .map(|w| w.iter().map(|v| v.abs()).sum::<f32>())
            .fold(0.0f32, f32::max);
        const BLOCK: usize = 64;
        for c in 0..self.channels {
            let plane = self.channel(c);
            let mut start = 0;
            while start < frames {
                let end = (start + BLOCK).min(frames);
                let lo = start.saturating_sub(TP_HALF);
                let hi = (end + TP_HALF).min(frames);
                let local = plane[lo..hi].iter().fold(0.0f32, |m, s| m.max(s.abs()));
                if local * l1 > best {
                    for n in start..end {
                        if n + 1 >= TP_HALF && n + TP_HALF < frames {
                            let win = &plane[n + 1 - TP_HALF..n + 1 + TP_HALF];
                            for w in &phases[1..] {
                                let mut acc = 0.0f32;
                                for (wv, x) in w.iter().zip(win) {
                                    acc += wv * x;
                                }
                                best = best.max(acc.abs());
                            }
                        } else {
                            for w in &phases[1..] {
                                let mut acc = 0.0f32;
                                for (wi, wv) in w.iter().enumerate() {
                                    let idx = n as i64 + wi as i64 - TP_HALF as i64 + 1;
                                    if idx >= 0 && (idx as usize) < frames {
                                        acc += wv * plane[idx as usize];
                                    }
                                }
                                best = best.max(acc.abs());
                            }
                        }
                    }
                }
                start = end;
            }
        }
        lin_to_db(best)
    }
}

/// Taps in the true-peak interpolation kernel, and half that.
const TP_TAPS: usize = 16;
const TP_HALF: usize = TP_TAPS / 2;

/// Oversampling factor for a true-peak meter at `rate`: the smallest power of
/// two reaching 384 kHz, twice the floor BS.1770 Annex 2 sets, for the reason
/// given on `true_peak_dbtp`. The work is self-limiting — `rate * factor`
/// always lands in [384 kHz, 768 kHz) — so a low-rate buffer needs no cap to
/// stay cheap.
fn true_peak_oversampling(rate: u32) -> usize {
    let mut factor = 1usize;
    while (rate as u64) * (factor as u64) < 384_000 {
        factor *= 2;
    }
    factor
}

/// The `factor` polyphase branches of a Hann-windowed sinc interpolator.
/// Branch `p` reconstructs the signal `p / factor` of a sample after each
/// sample instant; branch 0 is a unit impulse.
fn true_peak_phases(factor: usize) -> Vec<[f32; TP_TAPS]> {
    (0..factor)
        .map(|p| {
            let frac = p as f64 / factor as f64;
            let mut w = [0.0f32; TP_TAPS];
            let mut sum = 0.0f64;
            let mut raw = [0.0f64; TP_TAPS];
            for (wi, r) in raw.iter_mut().enumerate() {
                let x = (wi as i64 - TP_HALF as i64 + 1) as f64 - frac;
                let sinc = if x.abs() < 1e-9 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                };
                let win = if x.abs() >= TP_HALF as f64 {
                    0.0
                } else {
                    0.5 * (1.0 + (PI * x / TP_HALF as f64).cos())
                };
                *r = sinc * win;
                sum += *r;
            }
            // Normalise to unit DC gain so the branches agree with each other
            // on a steady signal; the window's truncation otherwise leaves each
            // branch a slightly different fraction short.
            let norm = if sum.abs() > 1e-12 { 1.0 / sum } else { 1.0 };
            for (dst, r) in w.iter_mut().zip(raw) {
                *dst = (r * norm) as f32;
            }
            w
        })
        .collect()
}

fn mean_square(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / x.len() as f64
}

fn block_loudness(z: &[f64], weights: &[f64]) -> f64 {
    let sum: f64 = z.iter().zip(weights).map(|(z, w)| z * w).sum();
    if sum <= 0.0 {
        f64::NEG_INFINITY
    } else {
        -0.691 + 10.0 * sum.log10()
    }
}

/// Stage 1 of the K-weighting filter (high shelf, +4 dB above ~1.5 kHz),
/// derived for any sample rate from the BS.1770 reference design.
pub fn k_weight_shelf(fs: f64) -> Biquad {
    let f0 = 1_681.974_450_955_533;
    let g_db = 3.999_843_853_973_347;
    let q = 0.707_175_236_955_419_6;
    let k = (PI * f0 / fs).tan();
    let vh = 10f64.powf(g_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    Biquad::from_coeffs(
        ((vh + vb * k / q + k * k) / a0) as f32,
        (2.0 * (k * k - vh) / a0) as f32,
        ((vh - vb * k / q + k * k) / a0) as f32,
        (2.0 * (k * k - 1.0) / a0) as f32,
        ((1.0 - k / q + k * k) / a0) as f32,
    )
}

/// Stage 2 of the K-weighting filter (RLB high-pass at ~38 Hz).
pub fn k_weight_highpass(fs: f64) -> Biquad {
    let f0 = 38.135_470_876_024_44;
    let q = 0.500_327_037_323_877_3;
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad::from_coeffs(
        1.0,
        -2.0,
        1.0,
        (2.0 * (k * k - 1.0) / a0) as f32,
        ((1.0 - k / q + k * k) / a0) as f32,
    )
}

/// Decibels → linear gain.
pub fn db_to_lin(db: f32) -> f32 {
    if db == f32::NEG_INFINITY {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Linear gain → decibels (`-inf` for zero).
pub fn lin_to_db(lin: f32) -> f32 {
    if lin <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * lin.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_round_trip_and_accessors() {
        let b =
            AudioBuffer::from_planar(48_000, &[vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).unwrap();
        assert_eq!(b.frames(), 3);
        assert_eq!(b.channel(1), vec![4.0, 5.0, 6.0]);
        assert_eq!(b.frame(1), &[2.0, 5.0]);
        assert_eq!(b.sample(2, 0), 3.0);
        assert_eq!(b.sample(9, 0), 0.0);
        assert_eq!(b.to_planar()[0], vec![1.0, 2.0, 3.0]);
        let m = b.to_mono();
        assert_eq!(m.data, vec![2.5, 3.5, 4.5]);
        assert_eq!(b.slice(1, 5).frames(), 2);
    }

    #[test]
    fn mix_into_grows_and_spreads_mono() {
        let mono = AudioBuffer::from_interleaved(48_000, 1, vec![1.0, 1.0]).unwrap();
        let mut dst = AudioBuffer::new(48_000, 2, 1);
        mono.mix_into(&mut dst, 2, 0.5).unwrap();
        assert_eq!(dst.frames(), 4);
        assert_eq!(dst.frame(2), &[0.5, 0.5]);
        assert_eq!(dst.frame(0), &[0.0, 0.0]);
        let bad = AudioBuffer::new(44_100, 1, 1);
        assert!(bad.mix_into(&mut dst, 0, 1.0).is_err());
    }

    #[test]
    fn rms_and_peak_of_sine() {
        let s = AudioBuffer::sine(48_000, 2, 1000.0, 0.5, 1.0);
        assert!((s.rms() - 0.5 / 2f32.sqrt()).abs() < 1e-3);
        assert!((s.peak() - 0.5).abs() < 1e-3);
        assert!((s.rms_db() - (-9.03)).abs() < 0.05);
    }

    #[test]
    fn resample_preserves_sine() {
        let src = AudioBuffer::sine(48_000, 1, 1000.0, 0.8, 0.5);
        let out = src.resample(44_100);
        assert_eq!(out.sample_rate, 44_100);
        assert!((out.frames() as i64 - 22_050).abs() <= 1);
        let reference = AudioBuffer::sine(44_100, 1, 1000.0, 0.8, 0.5);
        // Compare away from the edges where the sinc kernel is truncated.
        let mut max_err = 0.0f32;
        for i in 100..(out.frames() - 100) {
            max_err = max_err.max((out.data[i] - reference.data[i]).abs());
        }
        assert!(max_err < 0.02, "max error {max_err}");
        // Upsampling too.
        let up = src.resample(96_000);
        let reference = AudioBuffer::sine(96_000, 1, 1000.0, 0.8, 0.5);
        let mut max_err = 0.0f32;
        for i in 100..(up.frames() - 100) {
            max_err = max_err.max((up.data[i] - reference.data[i]).abs());
        }
        assert!(max_err < 0.02, "upsample max error {max_err}");
    }

    #[test]
    fn lufs_of_calibrated_sine() {
        // A full-scale 1 kHz stereo sine reads 0 LUFS; scale it to −23.
        let amp = db_to_lin(-23.0);
        let s = AudioBuffer::sine(48_000, 2, 1000.0, amp, 3.0);
        let l = s.lufs_integrated();
        assert!((l + 23.0).abs() < 0.5, "measured {l} LUFS");
        // Sample-rate independence.
        let s = AudioBuffer::sine(44_100, 2, 1000.0, amp, 3.0);
        let l = s.lufs_integrated();
        assert!((l + 23.0).abs() < 0.5, "measured {l} LUFS at 44.1k");
        // Mono: a single channel reads 3.01 LU lower than the same signal on two.
        let m = AudioBuffer::sine(48_000, 1, 1000.0, amp, 3.0);
        assert!((m.lufs_integrated() + 26.0).abs() < 0.5);
        assert_eq!(
            AudioBuffer::new(48_000, 2, 48_000).lufs_integrated(),
            f64::NEG_INFINITY
        );
    }

    #[test]
    fn lufs_gating_ignores_silence() {
        let amp = db_to_lin(-23.0);
        let mut s = AudioBuffer::sine(48_000, 2, 1000.0, amp, 2.0);
        s.append(&AudioBuffer::new(48_000, 2, 48_000 * 4)).unwrap();
        let l = s.lufs_integrated();
        assert!((l + 23.0).abs() < 0.5, "measured {l} LUFS");
    }

    #[test]
    fn true_peak_sees_intersample_peaks() {
        // fs/4 sine sampled at 45°: sample peak is 0.707 but the true peak is 1.0.
        let s = AudioBuffer::sine_with_phase(48_000, 1, 12_000.0, 1.0, 0.1, PI / 4.0);
        assert!((s.peak() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        let tp = s.true_peak_dbtp();
        assert!(tp > -0.6 && tp < 0.3, "true peak {tp} dBTP");
    }

    #[test]
    fn db_conversions() {
        assert!((db_to_lin(-6.0206) - 0.5).abs() < 1e-4);
        assert!((lin_to_db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(lin_to_db(0.0), f32::NEG_INFINITY);
    }
}
