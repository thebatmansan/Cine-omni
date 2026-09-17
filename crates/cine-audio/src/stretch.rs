//! Changing speed without changing pitch: a phase vocoder with no memory.
//!
//! Time-stretching keeps a sound's frequencies while changing how long it takes.
//! The usual phase vocoder does it by carrying each frequency's phase forward
//! from one frame to the next, and WSOLA by aligning each grain to the one
//! before it. Both depend on everything before, so a player that seeks — or a
//! mixer that reads in device-sized pieces — must either rebuild that history
//! or produce something different from continuous playback.
//!
//! This one has no history. Synthesis frame `k` is centred at output sample
//! `k × hop` and reads the input around `k × hop × speed`. It measures each
//! frequency bin's *true* frequency from two analyses a short hop apart, and
//! sets that bin's output phase to what a steady sinusoid at that frequency
//! would have by the frame's output time:
//!
//! ```text
//! θ = φ(input at c) + ω × (output position − c)
//! ```
//!
//! Every quantity comes from frame `k` alone, so any frame can be computed in
//! isolation and reading from a seek gives the samples continuous playback
//! would. Bins are then phase-locked to the nearest spectral peak, which keeps a
//! partial's side bins coherent with it (identity phase locking).
//!
//! What it does not do well is what no phase vocoder does well: sharp attacks.
//! At exactly real time a click passes through intact, but at any other speed
//! each frame overlapping it re-places it at its own time, so the attack is
//! heard as several copies within one frame (43 ms at 48 kHz) — drums and
//! plosives flam. The test measures how many and how far apart rather than
//! claiming it away.

use crate::speed::Stepper;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

/// Frame length in samples.
pub const FRAME: usize = 2048;
/// Synthesis hop: frames overlap by three quarters.
pub const HOP: usize = FRAME / 4;
/// The hop between the two analyses a frequency is measured from. Short enough
/// that a partial within four bins of its bin centre is not ambiguous.
const MEASURE: usize = 128;

pub struct Stretch {
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    /// Sum of the squared windows at 75% overlap, which overlap-add divides by.
    norm: f32,
}

impl Default for Stretch {
    fn default() -> Stretch {
        Stretch::new()
    }
}

impl Stretch {
    pub fn new() -> Stretch {
        // Periodic Hann: its squares at quarter hops sum to exactly 1.5.
        let window: Vec<f32> = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / FRAME as f32).cos())
            .collect();
        let mut planner = FftPlanner::new();
        Stretch {
            forward: planner.plan_fft_forward(FRAME),
            inverse: planner.plan_fft_inverse(FRAME),
            window,
            norm: 1.5,
        }
    }

    fn spectrum(&self, samples: &[f32]) -> Vec<Complex32> {
        let mut buf: Vec<Complex32> = samples
            .iter()
            .zip(&self.window)
            .map(|(v, w)| Complex32::new(v * w, 0.0))
            .collect();
        self.forward.process(&mut buf);
        buf
    }

    /// Where synthesis frame `k` reads its input: the first input index of the
    /// `FRAME + MEASURE` samples it needs.
    pub fn input_start(&self, k: i64, speed: &Stepper) -> i64 {
        let (in_centre, _) = speed.position(k * HOP as i64);
        in_centre - (FRAME / 2) as i64
    }

    /// Input samples a frame reads: `FRAME + MEASURE` from [`Stretch::input_start`].
    pub const INPUT: usize = FRAME + MEASURE;

    /// Synthesis frame `k` from its input, already read: `input` holds
    /// [`Stretch::INPUT`] samples starting at [`Stretch::input_start`]. The
    /// result is `FRAME` windowed, normalised samples for output positions
    /// `[k × HOP − FRAME/2, k × HOP + FRAME/2)`.
    pub fn frame_from(&self, k: i64, speed: &Stepper, input: &[f32]) -> Vec<f32> {
        let out_centre = k * HOP as i64;
        let (in_centre, _) = speed.position(out_centre);
        let a = self.spectrum(&input[..FRAME]);
        let b = self.spectrum(&input[MEASURE..MEASURE + FRAME]);

        let bins = FRAME / 2 + 1;
        let mut magnitude = vec![0.0f32; bins];
        let mut phase = vec![0.0f32; bins];
        let mut theta = vec![0.0f32; bins];
        let advance = (out_centre - in_centre) as f32;
        for bin in 0..bins {
            let (pa, pb) = (a[bin].arg(), b[bin].arg());
            let centre = 2.0 * PI * bin as f32 / FRAME as f32;
            let deviation = wrap(pb - pa - centre * MEASURE as f32) / MEASURE as f32;
            magnitude[bin] = a[bin].norm();
            phase[bin] = pa;
            theta[bin] = pa + (centre + deviation) * advance;
        }

        // Identity phase locking: every bin keeps its phase relation to the
        // peak that dominates its region.
        let peaks: Vec<usize> = (1..bins - 1)
            .filter(|&i| magnitude[i] > magnitude[i - 1] && magnitude[i] >= magnitude[i + 1])
            .collect();
        let mut buf = vec![Complex32::new(0.0, 0.0); FRAME];
        let mut p = 0usize;
        for bin in 0..bins {
            let locked = if peaks.is_empty() {
                theta[bin]
            } else {
                while p + 1 < peaks.len() && peaks[p + 1].abs_diff(bin) < peaks[p].abs_diff(bin) {
                    p += 1;
                }
                let peak = peaks[p];
                theta[peak] + (phase[bin] - phase[peak])
            };
            let v = Complex32::from_polar(magnitude[bin], locked);
            buf[bin] = v;
            if bin > 0 && bin < FRAME / 2 {
                buf[FRAME - bin] = v.conj();
            }
        }
        self.inverse.process(&mut buf);
        let scale = 1.0 / (FRAME as f32 * self.norm);
        buf.iter()
            .zip(&self.window)
            .map(|(c, w)| c.re * scale * w)
            .collect()
    }

    /// Synthesis frame `k`, reading its input through `input`.
    pub fn frame(&self, k: i64, speed: &Stepper, input: &mut impl FnMut(i64) -> f32) -> Vec<f32> {
        let start = self.input_start(k, speed);
        let samples: Vec<f32> = (0..Self::INPUT).map(|i| input(start + i as i64)).collect();
        self.frame_from(k, speed, &samples)
    }

    /// The frames covering output samples `[from, to)`.
    pub fn frames_for(from: i64, to: i64) -> std::ops::RangeInclusive<i64> {
        let half = (FRAME / 2) as i64;
        let hop = HOP as i64;
        (from - half).div_euclid(hop) + 1..=(to - 1 + half).div_euclid(hop)
    }

    /// Overlap-add a frame into `out`, which holds output samples from `from`.
    pub fn add(k: i64, frame: &[f32], from: i64, out: &mut [f32]) {
        let start = k * HOP as i64 - (FRAME / 2) as i64;
        let to = from + out.len() as i64;
        let lo = start.max(from);
        let hi = (start + FRAME as i64).min(to);
        for o in lo..hi {
            out[(o - from) as usize] += frame[(o - start) as usize];
        }
    }

    /// Render output samples `[from, to)` into `out` by overlap-adding every
    /// frame that covers them. Any range gives the samples continuous playback
    /// would: no frame depends on another.
    pub fn render(
        &self,
        from: i64,
        to: i64,
        speed: &Stepper,
        mut input: impl FnMut(i64) -> f32,
        out: &mut [f32],
    ) {
        for k in Self::frames_for(from, to) {
            let frame = self.frame(k, speed, &mut input);
            Self::add(k, &frame, from, out);
        }
    }
}

/// An angle into `(−π, π]`.
fn wrap(x: f32) -> f32 {
    let two_pi = 2.0 * PI;
    x - two_pi * ((x + PI) / two_pi).floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    fn tone(freq: f64) -> impl FnMut(i64) -> f32 {
        move |i| (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / RATE).sin()) as f32
    }

    fn frequency(samples: &[f32]) -> f64 {
        let crossings: Vec<usize> = samples
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, _)| i)
            .collect();
        let (first, last) = (crossings[0], *crossings.last().unwrap());
        (crossings.len() - 1) as f64 * RATE / (last - first) as f64
    }

    fn rms_db(samples: &[f32]) -> f64 {
        let e: f64 = samples.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
        10.0 * (e / samples.len() as f64).log10()
    }

    fn render(speed: Stepper, len: i64, input: impl FnMut(i64) -> f32) -> Vec<f32> {
        let mut out = vec![0.0f32; len as usize];
        Stretch::new().render(0, len, &speed, input, &mut out);
        out
    }

    /// The point of the module: faster and slower, the pitch stays.
    #[test]
    fn a_tone_keeps_its_pitch_and_level_at_other_speeds() {
        for (media, track) in [(3, 2), (2, 1), (3, 4), (1, 2)] {
            let out = render(Stepper::new(media, track), 96_000, tone(1_000.0));
            let middle = &out[8_000..88_000];
            let f = frequency(middle);
            assert!((f - 1_000.0).abs() < 1.0, "{media}/{track}: {f:.2} Hz");
            let level = rms_db(middle);
            assert!((level + 9.03).abs() < 0.5, "{media}/{track}: {level:.2} dB");
        }
    }

    /// A change in the media arrives when the speed says it should: at twice
    /// speed, what happens two seconds into the media is heard one second in.
    #[test]
    fn the_media_is_consumed_at_the_speed() {
        let input = |i: i64| {
            let t = i as f64 / RATE;
            let f = if t < 2.0 { 500.0 } else { 1_500.0 };
            (0.5 * (2.0 * std::f64::consts::PI * f * t).sin()) as f32
        };
        let out = render(Stepper::new(2, 1), 96_000, input);
        let before = frequency(&out[8_000..40_000]);
        let after = frequency(&out[56_000..88_000]);
        assert!((before - 500.0).abs() < 1.0, "{before:.2}");
        assert!((after - 1_500.0).abs() < 1.5, "{after:.2}");
    }

    /// Stateless: a range rendered on its own is the same range rendered as
    /// part of a longer one, so a seek hears what continuous playback hears.
    #[test]
    fn any_range_renders_the_samples_continuous_playback_would() {
        let speed = Stepper::new(3, 4);
        let whole = render(speed, 48_000, tone(733.0));
        let mut part = vec![0.0f32; 5_000];
        Stretch::new().render(20_000, 25_000, &speed, tone(733.0), &mut part);
        let worst = part
            .iter()
            .zip(&whole[20_000..25_000])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    /// Two partials both survive, each at its own frequency.
    #[test]
    fn a_chord_keeps_both_notes() {
        let chord = |i: i64| {
            let t = i as f64 / RATE;
            (0.3 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()
                + 0.3 * (2.0 * std::f64::consts::PI * 660.0 * t).sin()) as f32
        };
        let out = render(Stepper::new(4, 3), 96_000, chord);
        let window = &out[16_384..16_384 + 32_768];
        let spectrum = {
            let mut planner = FftPlanner::<f32>::new();
            let fft = planner.plan_fft_forward(32_768);
            let mut buf: Vec<Complex32> = window
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let w = 0.5 - 0.5 * (2.0 * PI * i as f32 / 32_768.0).cos();
                    Complex32::new(v * w, 0.0)
                })
                .collect();
            fft.process(&mut buf);
            buf.iter()
                .take(16_384)
                .map(|c| c.norm())
                .collect::<Vec<f32>>()
        };
        let bin = |hz: f64| (hz * 32_768.0 / RATE).round() as usize;
        let around = |hz: f64| {
            spectrum[bin(hz) - 2..=bin(hz) + 2]
                .iter()
                .cloned()
                .fold(0.0, f32::max)
        };
        let (a, e) = (around(440.0), around(660.0));
        let floor = around(550.0);
        assert!(
            a > 30.0 * floor && e > 30.0 * floor,
            "440 {a}, 660 {e}, between {floor}"
        );
        assert!(
            (a / e - 1.0).abs() < 0.2,
            "the two notes stay balanced: {a} vs {e}"
        );
    }

    /// The known weakness, measured. At exactly one-for-one the advance is
    /// zero and a click passes through intact. At any other speed, each of the
    /// frames overlapping the click re-places it — for an impulse the phase
    /// rule is a pure delay — each at its own time, so the attack is heard as
    /// several copies close together rather than one.
    #[test]
    fn an_attack_is_intact_at_real_time_and_repeated_otherwise() {
        let clicks = |i: i64| {
            if i.rem_euclid(24_000) == 12_000 {
                1.0
            } else {
                0.0
            }
        };
        // Copies: clusters of samples above 10% of the peak, separated by more
        // than 20 samples.
        let copies = |out: &[f32]| {
            let peak = out.iter().cloned().fold(0.0f32, |m, v| m.max(v.abs()));
            let loud: Vec<usize> = (0..out.len())
                .filter(|&i| out[i].abs() > peak * 0.1)
                .collect();
            let mut clusters = Vec::new();
            for i in loud {
                match clusters.last_mut() {
                    Some((_, end)) if i - *end <= 20 => *end = i,
                    _ => clusters.push((i, i)),
                }
            }
            (peak, clusters)
        };
        let (peak, clusters) = copies(&render(Stepper::new(1, 1), 24_000, clicks));
        assert!(
            peak > 0.99 && clusters.len() == 1,
            "real time: {peak:.3}, {clusters:?}"
        );

        let (peak, clusters) = copies(&render(Stepper::new(3, 4), 24_000, clicks));
        let gaps: Vec<usize> = clusters.windows(2).map(|w| w[1].0 - w[0].0).collect();
        println!(
            "MEASURED a click at 3/4 speed: {} copies, peak {peak:.3}, spaced {gaps:?} samples",
            clusters.len()
        );
        assert!(
            clusters.len() > 1,
            "at 3/4 the attack repeats: {clusters:?}"
        );
        assert!(
            clusters.last().unwrap().1 - clusters[0].0 < FRAME,
            "and every copy is within one frame of the others: {clusters:?}"
        );
    }
}
