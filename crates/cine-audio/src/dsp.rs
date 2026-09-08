//! DSP toolkit: biquads, STFT, envelope follower, dynamics, delay, fades.
//!
//! Everything here is pure sample math with no graph dependency. Processors
//! keep their own state so a renderer can instantiate one per track/channel
//! and stream blocks through it.

use crate::buffer::{db_to_lin, lin_to_db, AudioBuffer};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// Biquad
// ---------------------------------------------------------------------------

/// Filter shapes from the RBJ Audio EQ Cookbook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BiquadKind {
    #[default]
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peaking,
    LowShelf,
    HighShelf,
    AllPass,
}

/// Second-order IIR section (transposed direct form II).
#[derive(Clone, Debug, PartialEq)]
pub struct Biquad {
    /// Numerator coefficients (normalized so `a0 == 1`).
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    /// Denominator coefficients (normalized so `a0 == 1`).
    pub a1: f32,
    pub a2: f32,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// Build from normalized coefficients.
    pub fn from_coeffs(b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> Self {
        Biquad {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Identity filter.
    pub fn identity() -> Self {
        Biquad::from_coeffs(1.0, 0.0, 0.0, 0.0, 0.0)
    }

    /// Design any cookbook shape. `gain_db` is used by peaking and shelf types.
    pub fn design(kind: BiquadKind, sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Self {
        let fs = sample_rate.max(1.0);
        let f0 = freq_hz.clamp(1.0, fs * 0.499);
        let q = q.max(0.01);
        let w0 = 2.0 * PI * f0 / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            BiquadKind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            BiquadKind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            BiquadKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            BiquadKind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            BiquadKind::AllPass => (
                1.0 - alpha,
                -2.0 * cos,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            BiquadKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            BiquadKind::LowShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + sa),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - sa),
                    (a + 1.0) + (a - 1.0) * cos + sa,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - sa,
                )
            }
            BiquadKind::HighShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + sa),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - sa),
                    (a + 1.0) - (a - 1.0) * cos + sa,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - sa,
                )
            }
        };
        Biquad::from_coeffs(
            (b0 / a0) as f32,
            (b1 / a0) as f32,
            (b2 / a0) as f32,
            (a1 / a0) as f32,
            (a2 / a0) as f32,
        )
    }

    /// Low-pass (12 dB/oct).
    pub fn lowpass(sample_rate: f64, freq_hz: f64, q: f64) -> Self {
        Self::design(BiquadKind::LowPass, sample_rate, freq_hz, q, 0.0)
    }
    /// High-pass (12 dB/oct).
    pub fn highpass(sample_rate: f64, freq_hz: f64, q: f64) -> Self {
        Self::design(BiquadKind::HighPass, sample_rate, freq_hz, q, 0.0)
    }
    /// Band-pass (constant 0 dB peak gain).
    pub fn bandpass(sample_rate: f64, freq_hz: f64, q: f64) -> Self {
        Self::design(BiquadKind::BandPass, sample_rate, freq_hz, q, 0.0)
    }
    /// Notch.
    pub fn notch(sample_rate: f64, freq_hz: f64, q: f64) -> Self {
        Self::design(BiquadKind::Notch, sample_rate, freq_hz, q, 0.0)
    }
    /// Peaking EQ.
    pub fn peaking(sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Self {
        Self::design(BiquadKind::Peaking, sample_rate, freq_hz, q, gain_db)
    }
    /// Low shelf.
    pub fn low_shelf(sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Self {
        Self::design(BiquadKind::LowShelf, sample_rate, freq_hz, q, gain_db)
    }
    /// High shelf.
    pub fn high_shelf(sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Self {
        Self::design(BiquadKind::HighShelf, sample_rate, freq_hz, q, gain_db)
    }

    /// Clear the delay state.
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Filter one sample.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let y = self.b0 as f64 * x + self.z1;
        self.z1 = self.b1 as f64 * x - self.a1 as f64 * y + self.z2;
        self.z2 = self.b2 as f64 * x - self.a2 as f64 * y;
        y as f32
    }

    /// Filter a block in place.
    pub fn process(&mut self, block: &mut [f32]) {
        for s in block.iter_mut() {
            *s = self.process_sample(*s);
        }
    }

    /// Magnitude response (linear) at `freq_hz` for the given sample rate.
    pub fn magnitude_at(&self, sample_rate: f64, freq_hz: f64) -> f64 {
        let w = 2.0 * PI * freq_hz / sample_rate.max(1.0);
        let z1 = Complex::new(w.cos(), -w.sin());
        let z2 = z1 * z1;
        let num = Complex::new(self.b0 as f64, 0.0) + z1 * self.b1 as f64 + z2 * self.b2 as f64;
        let den = Complex::new(1.0, 0.0) + z1 * self.a1 as f64 + z2 * self.a2 as f64;
        (num / den).norm()
    }

    /// Magnitude response in dB.
    pub fn magnitude_db_at(&self, sample_rate: f64, freq_hz: f64) -> f64 {
        20.0 * self.magnitude_at(sample_rate, freq_hz).max(1e-12).log10()
    }
}

// ---------------------------------------------------------------------------
// STFT
// ---------------------------------------------------------------------------

/// Complex spectrogram: one row of `size / 2 + 1` bins per frame.
#[derive(Clone, Debug)]
pub struct Spectrogram {
    /// FFT size the frames were computed with.
    pub size: usize,
    /// Hop between frames in samples.
    pub hop: usize,
    /// Per-frame half spectra (`size/2 + 1` bins).
    pub frames: Vec<Vec<Complex<f32>>>,
    /// Signal length in samples before padding (needed for exact inversion).
    pub signal_len: usize,
}

impl Spectrogram {
    /// Number of bins per frame.
    pub fn bins(&self) -> usize {
        self.size / 2 + 1
    }
    /// Center time of frame `i` in seconds (signal-relative).
    pub fn frame_time_s(&self, i: usize, sample_rate: u32) -> f64 {
        i as f64 * self.hop as f64 / sample_rate.max(1) as f64
    }
    /// Center frequency of bin `k`.
    pub fn bin_freq_hz(&self, k: usize, sample_rate: u32) -> f64 {
        k as f64 * sample_rate as f64 / self.size as f64
    }
}

/// Short-time Fourier transform with a periodic Hann window applied at both
/// analysis and synthesis (WOLA); `inverse` divides by the summed window
/// product so reconstruction is exact for any hop `< size`.
#[derive(Clone, Debug)]
pub struct Stft {
    /// FFT size (power of two recommended).
    pub size: usize,
    /// Hop size in samples.
    pub hop: usize,
    window: Vec<f32>,
}

impl Stft {
    /// Create a transform; `hop` is clamped to `1..=size`.
    pub fn new(size: usize, hop: usize) -> Self {
        let size = size.max(4);
        let hop = hop.clamp(1, size);
        let window = (0..size)
            .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / size as f64).cos()) as f32)
            .collect();
        Stft { size, hop, window }
    }

    /// The analysis window.
    pub fn window(&self) -> &[f32] {
        &self.window
    }

    /// Forward transform. The signal is zero-padded by `size / 2` on both
    /// sides so frame `i` is centred on sample `i * hop`.
    pub fn forward(&self, signal: &[f32]) -> Spectrogram {
        let pad = self.size / 2;
        let padded_len = signal.len() + 2 * pad;
        let n_frames = if padded_len >= self.size {
            (padded_len - self.size) / self.hop + 1
        } else {
            1
        };
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(self.size);
        let mut buf = vec![Complex::new(0.0, 0.0); self.size];
        let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        let bins = self.size / 2 + 1;
        let mut frames = Vec::with_capacity(n_frames);
        for f in 0..n_frames {
            let start = f * self.hop;
            for (n, b) in buf.iter_mut().enumerate() {
                let idx = start + n;
                let x = if idx >= pad && idx - pad < signal.len() {
                    signal[idx - pad]
                } else {
                    0.0
                };
                *b = Complex::new(x * self.window[n], 0.0);
            }
            fft.process_with_scratch(&mut buf, &mut scratch);
            frames.push(buf[..bins].to_vec());
        }
        Spectrogram {
            size: self.size,
            hop: self.hop,
            frames,
            signal_len: signal.len(),
        }
    }

    /// Inverse transform back to `spec.signal_len` samples.
    pub fn inverse(&self, spec: &Spectrogram) -> Vec<f32> {
        let size = spec.size;
        let hop = spec.hop;
        let pad = size / 2;
        let padded_len = if spec.frames.is_empty() {
            0
        } else {
            (spec.frames.len() - 1) * hop + size
        };
        let mut out = vec![0.0f64; padded_len];
        let mut norm = vec![0.0f64; padded_len];
        let mut planner = FftPlanner::<f32>::new();
        let ifft = planner.plan_fft_inverse(size);
        let mut buf = vec![Complex::new(0.0, 0.0); size];
        let mut scratch = vec![Complex::new(0.0, 0.0); ifft.get_inplace_scratch_len()];
        let bins = size / 2 + 1;
        let window: Vec<f32> = if size == self.size {
            self.window.clone()
        } else {
            (0..size)
                .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / size as f64).cos()) as f32)
                .collect()
        };
        for (f, frame) in spec.frames.iter().enumerate() {
            let m = bins.min(frame.len());
            buf[..m].copy_from_slice(&frame[..m]);
            for k in bins..size {
                buf[k] = frame[size - k].conj();
            }
            ifft.process_with_scratch(&mut buf, &mut scratch);
            let start = f * hop;
            for n in 0..size {
                let w = window[n] as f64;
                out[start + n] += buf[n].re as f64 / size as f64 * w;
                norm[start + n] += w * w;
            }
        }
        let mut signal = vec![0.0f32; spec.signal_len];
        for (i, s) in signal.iter_mut().enumerate() {
            let idx = i + pad;
            if idx < out.len() {
                let n = norm[idx];
                *s = if n > 1e-8 { (out[idx] / n) as f32 } else { 0.0 };
            }
        }
        signal
    }
}

// ---------------------------------------------------------------------------
// Envelope follower
// ---------------------------------------------------------------------------

/// Attack/release (in ms) to one-pole coefficients.
fn ar_coeff(ms: f32, sample_rate: f32) -> f32 {
    let samples = (ms.max(0.0) * 1e-3 * sample_rate).max(1e-3);
    (-1.0 / samples).exp()
}

/// Peak envelope follower with separate attack and release.
#[derive(Clone, Debug)]
pub struct Envelope {
    attack: f32,
    release: f32,
    state: f32,
}

impl Envelope {
    /// Create a follower for the given sample rate.
    pub fn new(sample_rate: f32, attack_ms: f32, release_ms: f32) -> Self {
        Envelope {
            attack: ar_coeff(attack_ms, sample_rate),
            release: ar_coeff(release_ms, sample_rate),
            state: 0.0,
        }
    }

    /// Feed one rectified/level sample and return the smoothed envelope.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let x = x.abs();
        let coef = if x > self.state {
            self.attack
        } else {
            self.release
        };
        self.state = coef * self.state + (1.0 - coef) * x;
        self.state
    }

    /// Current envelope value.
    pub fn value(&self) -> f32 {
        self.state
    }

    /// Envelope of a whole signal.
    pub fn process(&mut self, signal: &[f32]) -> Vec<f32> {
        signal.iter().map(|x| self.process_sample(*x)).collect()
    }

    /// Reset the state.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

// ---------------------------------------------------------------------------
// Dynamics
// ---------------------------------------------------------------------------

/// Feed-forward compressor parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Soft-knee width in dB (0 = hard knee).
    pub knee_db: f32,
    pub makeup_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        CompressorParams {
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            knee_db: 6.0,
            makeup_db: 0.0,
        }
    }
}

/// Simple channel-linked compressor with a soft knee and log-domain smoothing.
#[derive(Clone, Debug)]
pub struct Compressor {
    /// Parameters in use.
    pub params: CompressorParams,
    attack: f32,
    release: f32,
    gain_db_state: f32,
}

impl Compressor {
    /// Create a compressor for a sample rate.
    pub fn new(params: CompressorParams, sample_rate: f32) -> Self {
        Compressor {
            params,
            attack: ar_coeff(params.attack_ms, sample_rate),
            release: ar_coeff(params.release_ms, sample_rate),
            gain_db_state: 0.0,
        }
    }

    /// Static gain-computer curve: input level (dB) → output level (dB).
    pub fn static_curve_db(&self, x_db: f32) -> f32 {
        let p = &self.params;
        let ratio = p.ratio.max(1.0);
        let w = p.knee_db.max(0.0);
        let over = x_db - p.threshold_db;
        if 2.0 * over < -w {
            x_db
        } else if w > 0.0 && 2.0 * over.abs() <= w {
            x_db + (1.0 / ratio - 1.0) * (over + w / 2.0).powi(2) / (2.0 * w)
        } else {
            p.threshold_db + over / ratio
        }
    }

    /// Process an interleaved block with `channels` channels (linked detection).
    pub fn process(&mut self, block: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        let makeup = db_to_lin(self.params.makeup_db);
        for frame in block.chunks_mut(channels) {
            let level = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let x_db = lin_to_db(level).max(-120.0);
            let target = self.static_curve_db(x_db) - x_db; // ≤ 0
            let coef = if target < self.gain_db_state {
                self.attack
            } else {
                self.release
            };
            self.gain_db_state = coef * self.gain_db_state + (1.0 - coef) * target;
            let g = db_to_lin(self.gain_db_state) * makeup;
            for s in frame.iter_mut() {
                *s *= g;
            }
        }
    }

    /// Current gain reduction in dB (≤ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        self.gain_db_state
    }

    /// Reset the smoothing state.
    pub fn reset(&mut self) {
        self.gain_db_state = 0.0;
    }
}

/// Noise-gate parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateParams {
    pub threshold_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub hold_ms: f32,
    /// Attenuation applied when closed (dB, negative).
    pub range_db: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        GateParams {
            threshold_db: -50.0,
            attack_ms: 1.0,
            release_ms: 80.0,
            hold_ms: 30.0,
            range_db: -80.0,
        }
    }
}

/// Downward expander/gate with hold.
#[derive(Clone, Debug)]
pub struct Gate {
    /// Parameters in use.
    pub params: GateParams,
    detector: Envelope,
    attack: f32,
    release: f32,
    hold_samples: usize,
    hold_counter: usize,
    gain_state: f32,
}

impl Gate {
    /// Create a gate for a sample rate.
    pub fn new(params: GateParams, sample_rate: f32) -> Self {
        Gate {
            params,
            detector: Envelope::new(sample_rate, 0.5, 20.0),
            attack: ar_coeff(params.attack_ms, sample_rate),
            release: ar_coeff(params.release_ms, sample_rate),
            hold_samples: (params.hold_ms.max(0.0) * 1e-3 * sample_rate) as usize,
            hold_counter: 0,
            gain_state: 0.0,
        }
    }

    /// Process an interleaved block (linked detection).
    pub fn process(&mut self, block: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        let thr = db_to_lin(self.params.threshold_db);
        let floor = db_to_lin(self.params.range_db);
        for frame in block.chunks_mut(channels) {
            let level = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let env = self.detector.process_sample(level);
            let open = if env >= thr {
                self.hold_counter = self.hold_samples;
                true
            } else if self.hold_counter > 0 {
                self.hold_counter -= 1;
                true
            } else {
                false
            };
            let target = if open { 1.0 } else { floor };
            let coef = if target > self.gain_state {
                self.attack
            } else {
                self.release
            };
            self.gain_state = coef * self.gain_state + (1.0 - coef) * target;
            for s in frame.iter_mut() {
                *s *= self.gain_state;
            }
        }
    }

    /// Reset the state.
    pub fn reset(&mut self) {
        self.detector.reset();
        self.hold_counter = 0;
        self.gain_state = 0.0;
    }
}

/// Brick-wall limiter parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimiterParams {
    /// Output ceiling in dBFS.
    pub ceiling_db: f32,
    pub release_ms: f32,
    /// Lookahead (and latency) in ms.
    pub lookahead_ms: f32,
}

impl Default for LimiterParams {
    fn default() -> Self {
        LimiterParams {
            ceiling_db: -1.0,
            release_ms: 50.0,
            lookahead_ms: 1.5,
        }
    }
}

/// Look-ahead brick-wall limiter. Gain is a sliding minimum over `2 × lookahead`
/// followed by a `lookahead`-long moving average, which guarantees the delayed
/// signal never exceeds the ceiling while keeping gain changes smooth.
#[derive(Clone, Debug)]
pub struct Limiter {
    /// Parameters in use.
    pub params: LimiterParams,
    lookahead: usize,
    release: f32,
    channels: usize,
    delay: VecDeque<f32>,
    min_window: VecDeque<(u64, f32)>,
    avg_window: VecDeque<f32>,
    avg_sum: f64,
    env: f32,
    counter: u64,
}

impl Limiter {
    /// Create a limiter for a sample rate and channel count.
    pub fn new(params: LimiterParams, sample_rate: f32, channels: usize) -> Self {
        let channels = channels.max(1);
        let lookahead =
            ((params.lookahead_ms.max(0.0) * 1e-3 * sample_rate).round() as usize).max(1);
        let mut l = Limiter {
            params,
            lookahead,
            release: ar_coeff(params.release_ms, sample_rate),
            channels,
            delay: VecDeque::new(),
            min_window: VecDeque::new(),
            avg_window: VecDeque::new(),
            avg_sum: 0.0,
            env: 1.0,
            counter: 0,
        };
        l.reset();
        l
    }

    /// Latency in frames introduced by the lookahead.
    pub fn latency_frames(&self) -> usize {
        self.lookahead
    }

    /// Reset state (delay line filled with silence, gain = 1).
    pub fn reset(&mut self) {
        self.delay = VecDeque::from(vec![0.0f32; self.lookahead * self.channels]);
        self.min_window.clear();
        self.avg_window = VecDeque::from(vec![1.0f32; self.lookahead]);
        self.avg_sum = self.lookahead as f64;
        self.env = 1.0;
        self.counter = 0;
    }

    /// Process an interleaved block in place. Output is delayed by
    /// [`Limiter::latency_frames`].
    pub fn process(&mut self, block: &mut [f32]) {
        let ch = self.channels;
        let ceiling = db_to_lin(self.params.ceiling_db);
        let min_len = (2 * self.lookahead) as u64;
        for frame in block.chunks_mut(ch) {
            let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let target = if peak > ceiling { ceiling / peak } else { 1.0 };
            // Sliding minimum over the last 2L targets (monotonic deque).
            while self.min_window.back().is_some_and(|(_, v)| *v >= target) {
                self.min_window.pop_back();
            }
            self.min_window.push_back((self.counter, target));
            while self
                .min_window
                .front()
                .is_some_and(|(i, _)| self.counter - *i >= min_len)
            {
                self.min_window.pop_front();
            }
            let win_min = self.min_window.front().map(|(_, v)| *v).unwrap_or(1.0);
            // Instant attack, exponential release.
            self.env = if win_min < self.env {
                win_min
            } else {
                self.release * self.env + (1.0 - self.release) * win_min
            };
            // Moving average over L.
            self.avg_window.push_back(self.env);
            self.avg_sum += self.env as f64;
            if let Some(old) = self.avg_window.pop_front() {
                self.avg_sum -= old as f64;
            }
            let gain = (self.avg_sum / self.lookahead as f64) as f32;
            // Delay line.
            for s in frame.iter_mut() {
                self.delay.push_back(*s);
                *s = self.delay.pop_front().unwrap_or(0.0) * gain;
            }
            self.counter += 1;
        }
    }

    /// Process a whole buffer with latency compensation (output aligned to input).
    pub fn process_buffer(&mut self, buffer: &AudioBuffer) -> AudioBuffer {
        let frames = buffer.frames();
        let lat = self.latency_frames();
        let mut data = buffer.data.clone();
        data.extend(std::iter::repeat_n(0.0f32, lat * self.channels));
        self.process(&mut data);
        data.drain(..lat * self.channels);
        data.truncate(frames * self.channels);
        AudioBuffer {
            sample_rate: buffer.sample_rate,
            channels: buffer.channels,
            data,
        }
    }
}

// ---------------------------------------------------------------------------
// Delay
// ---------------------------------------------------------------------------

/// Feedback delay line (mono).
#[derive(Clone, Debug)]
pub struct Delay {
    buffer: Vec<f32>,
    write: usize,
    delay_samples: usize,
    /// Feedback amount (0..1).
    pub feedback: f32,
    /// Wet/dry mix (0 = dry, 1 = wet).
    pub mix: f32,
}

impl Delay {
    /// Create a delay of `delay_samples` samples.
    pub fn new(delay_samples: usize, feedback: f32, mix: f32) -> Self {
        let delay_samples = delay_samples.max(1);
        Delay {
            buffer: vec![0.0; delay_samples],
            write: 0,
            delay_samples,
            feedback: feedback.clamp(0.0, 0.999),
            mix: mix.clamp(0.0, 1.0),
        }
    }

    /// Create from a time in ms.
    pub fn from_ms(sample_rate: f32, delay_ms: f32, feedback: f32, mix: f32) -> Self {
        Self::new(
            (delay_ms.max(0.0) * 1e-3 * sample_rate).round() as usize,
            feedback,
            mix,
        )
    }

    /// Delay length in samples.
    pub fn delay_samples(&self) -> usize {
        self.delay_samples
    }

    /// Process one sample.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let delayed = self.buffer[self.write];
        self.buffer[self.write] = x + delayed * self.feedback;
        self.write = (self.write + 1) % self.delay_samples;
        x * (1.0 - self.mix) + delayed * self.mix
    }

    /// Process a block in place.
    pub fn process(&mut self, block: &mut [f32]) {
        for s in block.iter_mut() {
            *s = self.process_sample(*s);
        }
    }
}

// ---------------------------------------------------------------------------
// Fades
// ---------------------------------------------------------------------------

/// Fade shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    #[default]
    Linear,
    /// Sine/cosine law: two crossfaded signals keep constant power.
    EqualPower,
    /// Logarithmic (fast start, slow finish on the way in).
    Log,
}

impl FadeCurve {
    /// Gain for a fade-in at normalized position `t ∈ [0, 1]`.
    pub fn gain_in(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            FadeCurve::Linear => t,
            FadeCurve::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
            FadeCurve::Log => {
                // 60 dB range mapped so the curve reaches exactly 1 at t = 1.
                let db = -60.0 * (1.0 - t);
                let g = db_to_lin(db);
                (g - 0.001) / 0.999
            }
        }
    }

    /// Gain for a fade-out at normalized position `t ∈ [0, 1]` (1 → 0).
    pub fn gain_out(&self, t: f32) -> f32 {
        self.gain_in(1.0 - t.clamp(0.0, 1.0))
    }
}

/// A fade of a given length and shape.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Fade {
    /// Length in seconds.
    pub seconds: f64,
    /// Shape.
    pub curve: FadeCurve,
}

impl Fade {
    /// Constructor.
    pub fn new(seconds: f64, curve: FadeCurve) -> Self {
        Fade {
            seconds: seconds.max(0.0),
            curve,
        }
    }

    /// No fade.
    pub fn none() -> Self {
        Fade {
            seconds: 0.0,
            curve: FadeCurve::Linear,
        }
    }

    /// Length in frames at a sample rate.
    pub fn frames(&self, sample_rate: u32) -> usize {
        (self.seconds * sample_rate as f64).round() as usize
    }
}

/// Apply a fade-in over the first `frames` frames of a buffer.
pub fn apply_fade_in(buffer: &mut AudioBuffer, frames: usize, curve: FadeCurve) {
    let n = frames.min(buffer.frames());
    if n == 0 {
        return;
    }
    let ch = buffer.channels;
    for i in 0..n {
        let g = curve.gain_in(fade_pos(i, n));
        for c in 0..ch {
            buffer.data[i * ch + c] *= g;
        }
    }
}

/// Normalized position of sample `i` in an `n`-sample fade so that the first
/// sample sits at 0 and the last at 1 (fade-in and fade-out are complementary).
fn fade_pos(i: usize, n: usize) -> f32 {
    if n <= 1 {
        1.0
    } else {
        i as f32 / (n - 1) as f32
    }
}

/// Apply a fade-out over the last `frames` frames of a buffer.
pub fn apply_fade_out(buffer: &mut AudioBuffer, frames: usize, curve: FadeCurve) {
    let total = buffer.frames();
    let n = frames.min(total);
    if n == 0 {
        return;
    }
    let ch = buffer.channels;
    let start = total - n;
    for i in 0..n {
        let g = curve.gain_out(fade_pos(i, n));
        for c in 0..ch {
            buffer.data[(start + i) * ch + c] *= g;
        }
    }
}

/// Crossfade `a` into `b` over `frames` frames: the result is
/// `a` (faded out) overlapped with `b` (faded in). Both must share rate and
/// channel count. Total length is `a + b - frames`.
pub fn crossfade(
    a: &AudioBuffer,
    b: &AudioBuffer,
    frames: usize,
    curve: FadeCurve,
) -> crate::error::Result<AudioBuffer> {
    if a.sample_rate != b.sample_rate || a.channels != b.channels {
        return Err(crate::error::Error::Mismatch(
            "crossfade: rate or channel mismatch".into(),
        ));
    }
    let n = frames.min(a.frames()).min(b.frames());
    let mut a2 = a.clone();
    let mut b2 = b.clone();
    apply_fade_out(&mut a2, n, curve);
    apply_fade_in(&mut b2, n, curve);
    let ch = a.channels;
    let overlap_start = a.frames() - n;
    let total = a.frames() + b.frames() - n;
    let mut out = AudioBuffer::new(a.sample_rate, ch, total);
    out.data[..a2.data.len()].copy_from_slice(&a2.data);
    let base = overlap_start * ch;
    for (i, s) in b2.data.iter().enumerate() {
        out.data[base + i] += s;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowpass_attenuates_above_cutoff() {
        let fs = 48_000.0;
        let lp = Biquad::lowpass(fs, 1000.0, std::f64::consts::FRAC_1_SQRT_2);
        let at_fc = lp.magnitude_db_at(fs, 1000.0);
        let at_2fc = lp.magnitude_db_at(fs, 2000.0);
        let dc = lp.magnitude_db_at(fs, 10.0);
        assert!((at_fc + 3.0).abs() < 0.3, "at fc: {at_fc} dB");
        assert!(at_2fc < -10.0, "at 2fc: {at_2fc} dB");
        assert!(dc.abs() < 0.05);
        // Time-domain check: a 4 kHz sine through the filter drops > 20 dB.
        let mut sig = AudioBuffer::sine(48_000, 1, 4000.0, 1.0, 0.5);
        let mut lp = Biquad::lowpass(fs, 1000.0, std::f64::consts::FRAC_1_SQRT_2);
        lp.process(&mut sig.data);
        let tail = sig.slice(4000, 20_000);
        assert!(
            tail.rms_db() < -20.0 - 3.0,
            "time-domain rms {}",
            tail.rms_db()
        );
    }

    #[test]
    fn highpass_peaking_shelf_notch_responses() {
        let fs = 48_000.0;
        let hp = Biquad::highpass(fs, 1000.0, 0.707);
        assert!(hp.magnitude_db_at(fs, 500.0) < -10.0);
        assert!(hp.magnitude_db_at(fs, 10_000.0).abs() < 0.2);
        let pk = Biquad::peaking(fs, 2000.0, 1.0, 6.0);
        assert!((pk.magnitude_db_at(fs, 2000.0) - 6.0).abs() < 0.1);
        assert!(pk.magnitude_db_at(fs, 100.0).abs() < 0.2);
        let ls = Biquad::low_shelf(fs, 300.0, 0.707, -6.0);
        assert!((ls.magnitude_db_at(fs, 20.0) + 6.0).abs() < 0.3);
        assert!(ls.magnitude_db_at(fs, 10_000.0).abs() < 0.2);
        let hs = Biquad::high_shelf(fs, 3000.0, 0.707, 4.0);
        assert!((hs.magnitude_db_at(fs, 20_000.0) - 4.0).abs() < 0.3);
        let notch = Biquad::notch(fs, 1000.0, 10.0);
        assert!(notch.magnitude_db_at(fs, 1000.0) < -40.0);
        assert!(notch.magnitude_db_at(fs, 5000.0).abs() < 0.2);
    }

    #[test]
    fn stft_perfect_reconstruction() {
        for (size, hop) in [(1024usize, 256usize), (512, 256), (256, 100)] {
            let stft = Stft::new(size, hop);
            let sig = AudioBuffer::noise(48_000, 1, 0.8, 0.25, 7);
            let spec = stft.forward(&sig.data);
            assert_eq!(spec.bins(), size / 2 + 1);
            let back = stft.inverse(&spec);
            assert_eq!(back.len(), sig.data.len());
            let max_err = sig
                .data
                .iter()
                .zip(&back)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(max_err < 1e-4, "size {size} hop {hop}: max err {max_err}");
        }
    }

    #[test]
    fn envelope_follows_step() {
        let mut env = Envelope::new(48_000.0, 1.0, 50.0);
        let mut last = 0.0;
        for _ in 0..480 {
            last = env.process_sample(1.0);
        }
        assert!(last > 0.99);
        for _ in 0..48 {
            last = env.process_sample(0.0);
        }
        assert!(last > 0.9 && last < 1.0); // slow release
    }

    #[test]
    fn compressor_reduces_loud_signal() {
        let params = CompressorParams {
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            knee_db: 0.0,
            makeup_db: 0.0,
        };
        let mut comp = Compressor::new(params, 48_000.0);
        let mut loud = AudioBuffer::sine(48_000, 1, 1000.0, 1.0, 0.5); // 0 dBFS peak
        comp.process(&mut loud.data, 1);
        let tail = loud.slice(12_000, 12_000);
        // Output peak should be near -20 + 20/4 = -15 dBFS.
        assert!(
            (tail.peak_db() + 15.0).abs() < 1.5,
            "peak {}",
            tail.peak_db()
        );
        // Below threshold: unity.
        let mut comp = Compressor::new(params, 48_000.0);
        let mut quiet = AudioBuffer::sine(48_000, 1, 1000.0, db_to_lin(-30.0), 0.2);
        let before = quiet.rms();
        comp.process(&mut quiet.data, 1);
        assert!((quiet.rms() - before).abs() < 1e-4);
        // Soft knee curve is continuous and monotonic.
        let soft = Compressor::new(
            CompressorParams {
                knee_db: 10.0,
                ..params
            },
            48_000.0,
        );
        let mut prev = f32::NEG_INFINITY;
        for i in -60..10 {
            let y = soft.static_curve_db(i as f32);
            assert!(y >= prev);
            prev = y;
        }
        assert!((soft.static_curve_db(-20.0) - (-20.0 + (0.25 - 1.0) * 25.0 / 20.0)).abs() < 1e-4);
    }

    #[test]
    fn gate_silences_quiet_passes_loud() {
        let params = GateParams {
            threshold_db: -30.0,
            attack_ms: 0.5,
            release_ms: 5.0,
            hold_ms: 0.0,
            range_db: -80.0,
        };
        let mut gate = Gate::new(params, 48_000.0);
        let mut quiet = AudioBuffer::sine(48_000, 1, 1000.0, db_to_lin(-50.0), 0.2);
        gate.process(&mut quiet.data, 1);
        assert!(quiet.peak_db() < -100.0);
        let mut gate = Gate::new(params, 48_000.0);
        let mut loud = AudioBuffer::sine(48_000, 1, 1000.0, 0.5, 0.2);
        gate.process(&mut loud.data, 1);
        assert!((loud.slice(2000, 5000).peak() - 0.5).abs() < 1e-3);
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let params = LimiterParams {
            ceiling_db: -3.0,
            release_ms: 30.0,
            lookahead_ms: 2.0,
        };
        let mut lim = Limiter::new(params, 48_000.0, 2);
        let mut sig = AudioBuffer::sine(48_000, 2, 440.0, 1.0, 0.3);
        // Add a nasty transient.
        for c in 0..2 {
            *sig.sample_mut(5000, c) = 3.0;
        }
        let out = lim.process_buffer(&sig);
        assert_eq!(out.frames(), sig.frames());
        let ceiling = db_to_lin(-3.0);
        assert!(
            out.peak() <= ceiling * 1.0001,
            "peak {} > ceiling {}",
            out.peak(),
            ceiling
        );
        // Quiet signal passes unchanged (gain exactly 1) and stays time-aligned.
        let mut lim = Limiter::new(params, 48_000.0, 1);
        let quiet = AudioBuffer::sine(48_000, 1, 440.0, 0.25, 0.1);
        let out = lim.process_buffer(&quiet);
        let max_err = quiet
            .data
            .iter()
            .zip(&out.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_err < 1e-6,
            "limiter altered a quiet signal by {max_err}"
        );
    }

    #[test]
    fn delay_line_delays() {
        let mut d = Delay::new(10, 0.0, 1.0);
        let mut sig = vec![0.0f32; 30];
        sig[0] = 1.0;
        d.process(&mut sig);
        assert_eq!(sig[10], 1.0);
        assert_eq!(sig[0], 0.0);
        let mut d = Delay::from_ms(48_000.0, 10.0, 0.5, 1.0);
        assert_eq!(d.delay_samples(), 480);
        let mut sig = vec![0.0f32; 1000];
        sig[0] = 1.0;
        d.process(&mut sig);
        assert!((sig[960] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn fades_and_crossfade() {
        for curve in [FadeCurve::Linear, FadeCurve::EqualPower, FadeCurve::Log] {
            assert!(curve.gain_in(0.0).abs() < 1e-6);
            assert!((curve.gain_in(1.0) - 1.0).abs() < 1e-6);
            assert!(curve.gain_in(0.5) > 0.0 && curve.gain_in(0.5) < 1.0);
        }
        let g = FadeCurve::EqualPower;
        let t = 0.3;
        assert!((g.gain_in(t).powi(2) + g.gain_out(t).powi(2) - 1.0).abs() < 1e-6);
        let mut b = AudioBuffer::from_interleaved(48_000, 1, vec![1.0; 10]).unwrap();
        apply_fade_in(&mut b, 4, FadeCurve::Linear);
        assert_eq!(&b.data[..4], &[0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
        apply_fade_out(&mut b, 2, FadeCurve::Linear);
        assert_eq!(b.data[9], 0.0);
        assert_eq!(b.data[8], 1.0);
        let a = AudioBuffer::from_interleaved(48_000, 1, vec![1.0; 8]).unwrap();
        let c = AudioBuffer::from_interleaved(48_000, 1, vec![1.0; 8]).unwrap();
        let x = crossfade(&a, &c, 4, FadeCurve::Linear).unwrap();
        assert_eq!(x.frames(), 12);
        for s in &x.data {
            assert!(
                (s - 1.0).abs() < 1e-6,
                "linear crossfade of equal signals stays flat: {s}"
            );
        }
    }
}
