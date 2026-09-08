//! Automated dialogue isolation: STFT spectral gating with a per-bin noise
//! floor, decision-directed Wiener gains with temporal/frequency smoothing,
//! and a spectral-flux + band-energy voice activity detector.
//!
//! The VAD segments feed ScriptSync when no speech-to-text is available:
//! [`dialogue_segments_to_words_hint`] spreads an expected word count evenly
//! over the detected speech.

use crate::buffer::{db_to_lin, AudioBuffer};
use crate::dsp::{Spectrogram, Stft};
use crate::error::{Error, Result};
use rayon::prelude::*;

/// Tuning for [`isolate_dialogue`].
#[derive(Clone, Copy, Debug)]
pub struct IsolationParams<'a> {
    /// Noise-only audio to build the floor from; when `None` the floor is
    /// estimated from the lowest-energy 10 % of frames of the input.
    pub noise_profile: Option<&'a AudioBuffer>,
    /// Maximum attenuation applied to noise (dB, positive).
    pub reduction_db: f32,
    /// Temporal gain smoothing `0..1` (0 = none, 0.9 = very slow).
    pub smoothing: f32,
    /// Frequency band considered speech `(low_hz, high_hz)`.
    pub speech_band: (f32, f32),
    /// Band SNR (dB above the noise floor) at which a frame counts as speech.
    pub voice_activity_threshold: f32,
    /// FFT size (power of two).
    pub fft_size: usize,
    /// Hop size in samples.
    pub hop: usize,
}

impl Default for IsolationParams<'_> {
    fn default() -> Self {
        IsolationParams {
            noise_profile: None,
            reduction_db: 24.0,
            smoothing: 0.5,
            speech_band: (80.0, 8000.0),
            voice_activity_threshold: 6.0,
            fft_size: 2048,
            hop: 512,
        }
    }
}

/// Output of [`isolate_dialogue`].
#[derive(Clone, Debug)]
pub struct IsolationResult {
    /// Enhanced dialogue.
    pub dialogue: AudioBuffer,
    /// What was removed (`input - dialogue`), sample aligned.
    pub residual: AudioBuffer,
    /// Detected speech segments `(start_s, end_s)`.
    pub vad: Vec<(f64, f64)>,
}

/// Per-bin noise power estimate.
fn noise_floor(power: &[Vec<f32>], band: (usize, usize), profile: Option<&[Vec<f32>]>) -> Vec<f32> {
    let bins = power.first().map(|p| p.len()).unwrap_or(0);
    let mut noise = vec![1e-10f32; bins];
    let frames: Vec<&Vec<f32>> = match profile {
        Some(p) if !p.is_empty() => p.iter().collect(),
        _ => {
            // Lowest-energy 10 % of frames (at least 3) by speech-band energy.
            let mut energies: Vec<(f32, usize)> = power
                .iter()
                .enumerate()
                .map(|(i, p)| (p[band.0..=band.1.min(bins - 1)].iter().sum::<f32>(), i))
                .collect();
            energies.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let n = (energies.len() / 10).max(3).min(energies.len().max(1));
            energies.iter().take(n).map(|(_, i)| &power[*i]).collect()
        }
    };
    if frames.is_empty() {
        return noise;
    }
    for f in &frames {
        for (k, p) in f.iter().enumerate() {
            noise[k] += p;
        }
    }
    let inv = 1.0 / frames.len() as f32;
    for n in noise.iter_mut() {
        *n *= inv;
    }
    // Smooth across frequency (3 bins).
    let raw = noise.clone();
    for k in 0..bins {
        let lo = k.saturating_sub(1);
        let hi = (k + 1).min(bins - 1);
        noise[k] = (raw[lo] + raw[k] + raw[hi]) / (hi - lo + 1) as f32;
    }
    noise
}

fn power_of(spec: &Spectrogram) -> Vec<Vec<f32>> {
    spec.frames
        .iter()
        .map(|f| f.iter().map(|c| c.norm_sqr()).collect())
        .collect()
}

/// Hangover kept after the last speech frame, in seconds (bridges the
/// low-energy tails of words so segment ends are not clipped).
const VAD_HANGOVER_S: f64 = 0.08;

/// Frame-level voice activity from band SNR and spectral flux. Returns one
/// flag per frame; `hangover` frames are kept after every detected frame.
fn frame_vad(
    power: &[Vec<f32>],
    noise: &[f32],
    band: (usize, usize),
    threshold_db: f32,
    hangover: usize,
) -> Vec<bool> {
    let n_noise: f32 = noise[band.0..=band.1].iter().sum::<f32>().max(1e-12);
    let mut prev_mag: Vec<f32> = vec![0.0; noise.len()];
    let mut raw = Vec::with_capacity(power.len());
    for p in power {
        let e: f32 = p[band.0..=band.1].iter().sum();
        let snr_db = 10.0 * (e / n_noise).max(1e-12).log10();
        // Positive spectral flux, normalized.
        let mut flux = 0.0f32;
        let mut total = 0.0f32;
        for k in band.0..=band.1 {
            let m = p[k].sqrt();
            flux += (m - prev_mag[k]).max(0.0);
            total += m;
            prev_mag[k] = m;
        }
        let flux = if total > 0.0 { flux / total } else { 0.0 };
        raw.push(snr_db >= threshold_db || (snr_db >= threshold_db * 0.5 && flux > 0.5));
    }
    // Median-of-5 smoothing then a hangover after speech.
    let n = raw.len();
    let mut smooth = vec![false; n];
    for (i, s) in smooth.iter_mut().enumerate() {
        let lo = i.saturating_sub(2);
        let hi = (i + 2).min(n.saturating_sub(1));
        let on = raw[lo..=hi].iter().filter(|b| **b).count();
        *s = on * 2 > (hi - lo + 1);
    }
    let mut out = smooth.clone();
    for (i, on) in smooth.iter().enumerate() {
        if *on {
            let end = (i + 1 + hangover).min(n);
            out[i + 1..end].fill(true);
        }
    }
    out
}

/// Turn frame flags into merged, minimum-length segments in seconds.
fn segments_from_flags(
    flags: &[bool],
    hop: usize,
    sample_rate: u32,
    min_len_s: f64,
    merge_gap_s: f64,
) -> Vec<(f64, f64)> {
    let fs = sample_rate.max(1) as f64;
    let t = |i: usize| i as f64 * hop as f64 / fs;
    let mut segs: Vec<(f64, f64)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, on) in flags.iter().enumerate() {
        match (start, *on) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                segs.push((t(s), t(i)));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        segs.push((t(s), t(flags.len())));
    }
    // Merge short gaps.
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for seg in segs {
        if let Some(last) = merged.last_mut() {
            if seg.0 - last.1 < merge_gap_s {
                last.1 = seg.1;
                continue;
            }
        }
        merged.push(seg);
    }
    merged
        .into_iter()
        .filter(|(s, e)| e - s >= min_len_s)
        .collect()
}

/// Separate dialogue from background noise.
pub fn isolate_dialogue(
    input: &AudioBuffer,
    params: IsolationParams<'_>,
) -> Result<IsolationResult> {
    if input.is_empty() {
        return Err(Error::InvalidArgument(
            "isolate_dialogue: empty input".into(),
        ));
    }
    let fft_size = params.fft_size.max(64).next_power_of_two();
    let hop = params.hop.clamp(1, fft_size / 2);
    let stft = Stft::new(fft_size, hop);
    let fs = input.sample_rate;
    let bins = fft_size / 2 + 1;
    let hz_per_bin = fs as f64 / fft_size as f64;
    let band = (
        ((params.speech_band.0 as f64 / hz_per_bin).floor() as usize).clamp(1, bins - 1),
        ((params.speech_band.1 as f64 / hz_per_bin).ceil() as usize).clamp(1, bins - 1),
    );
    let band = (band.0.min(band.1), band.1.max(band.0));
    let floor = db_to_lin(-params.reduction_db.max(0.0));
    let smoothing = params.smoothing.clamp(0.0, 0.99);

    // Analysis of every channel (in parallel) plus a mono spectrogram for the VAD.
    let planes = input.to_planar();
    let specs: Vec<Spectrogram> = planes.par_iter().map(|p| stft.forward(p)).collect();
    let powers: Vec<Vec<Vec<f32>>> = specs.par_iter().map(power_of).collect();
    let profile_powers: Option<Vec<Vec<Vec<f32>>>> = params.noise_profile.map(|np| {
        let np = if np.sample_rate != fs {
            np.resample(fs)
        } else {
            np.clone()
        };
        let np = np.with_channels(input.channels);
        np.to_planar()
            .par_iter()
            .map(|p| power_of(&stft.forward(p)))
            .collect()
    });
    let noises: Vec<Vec<f32>> = (0..input.channels)
        .map(|c| {
            noise_floor(
                &powers[c],
                band,
                profile_powers.as_ref().map(|pp| pp[c].as_slice()),
            )
        })
        .collect();

    // VAD on the channel-averaged power.
    let n_frames = powers[0].len();
    let mono_power: Vec<Vec<f32>> = (0..n_frames)
        .map(|t| {
            (0..bins)
                .map(|k| powers.iter().map(|p| p[t][k]).sum::<f32>() / input.channels as f32)
                .collect()
        })
        .collect();
    let mono_noise: Vec<f32> = (0..bins)
        .map(|k| noises.iter().map(|n| n[k]).sum::<f32>() / input.channels as f32)
        .collect();
    let hangover = ((VAD_HANGOVER_S * fs as f64 / hop as f64).ceil() as usize).max(2);
    let flags = frame_vad(
        &mono_power,
        &mono_noise,
        band,
        params.voice_activity_threshold,
        hangover,
    );
    let vad = segments_from_flags(&flags, hop, fs, 0.1, 0.15);

    // Per-channel gains and resynthesis.
    let alpha = 0.98f32; // decision-directed a-priori SNR weight
    let dialogue_planes: Vec<Vec<f32>> = (0..input.channels)
        .into_par_iter()
        .map(|c| {
            let spec = &specs[c];
            let power = &powers[c];
            let noise = &noises[c];
            let mut prev_clean = vec![0.0f32; bins];
            let mut prev_gain = vec![floor; bins];
            let mut out = spec.clone();
            for (t, frame) in out.frames.iter_mut().enumerate() {
                let mut gain = vec![floor; bins];
                for k in 0..bins {
                    let n = noise[k].max(1e-12);
                    let gamma = power[t][k] / n;
                    let xi = alpha * (prev_clean[k] / n) + (1.0 - alpha) * (gamma - 1.0).max(0.0);
                    let mut g = xi / (1.0 + xi);
                    if k < band.0 || k > band.1 || !flags[t] {
                        g = g.min(floor);
                    }
                    gain[k] = g.max(floor);
                }
                // Frequency smoothing (3 taps).
                let raw = gain.clone();
                for k in 0..bins {
                    let lo = k.saturating_sub(1);
                    let hi = (k + 1).min(bins - 1);
                    gain[k] = 0.25 * raw[lo] + 0.5 * raw[k] + 0.25 * raw[hi];
                }
                // Temporal smoothing.
                for k in 0..bins {
                    gain[k] = smoothing * prev_gain[k] + (1.0 - smoothing) * gain[k];
                    prev_gain[k] = gain[k];
                    frame[k] *= gain[k];
                    prev_clean[k] = frame[k].norm_sqr();
                }
            }
            stft.inverse(&out)
        })
        .collect();

    let dialogue = AudioBuffer::from_planar(fs, &dialogue_planes)?;
    let mut residual = input.clone();
    for (r, d) in residual.data.iter_mut().zip(&dialogue.data) {
        *r -= d;
    }
    Ok(IsolationResult {
        dialogue,
        residual,
        vad,
    })
}

/// Distribute `expected_word_count` words evenly over the speech segments
/// (proportionally to segment duration), returning one `(start_s, end_s)`
/// per word. Used as a ScriptSync alignment hint when no STT is available.
pub fn dialogue_segments_to_words_hint(
    vad: &[(f64, f64)],
    expected_word_count: usize,
) -> Vec<(f64, f64)> {
    let segs: Vec<(f64, f64)> = vad.iter().copied().filter(|(s, e)| e > s).collect();
    if expected_word_count == 0 || segs.is_empty() {
        return Vec::new();
    }
    let total: f64 = segs.iter().map(|(s, e)| e - s).sum();
    // Largest-remainder apportionment so counts sum exactly.
    let mut counts: Vec<usize> = Vec::with_capacity(segs.len());
    let mut remainders: Vec<(f64, usize)> = Vec::new();
    let mut assigned = 0;
    for (i, (s, e)) in segs.iter().enumerate() {
        let exact = expected_word_count as f64 * (e - s) / total;
        let n = exact.floor() as usize;
        counts.push(n);
        assigned += n;
        remainders.push((exact - n as f64, i));
    }
    remainders.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut left = expected_word_count - assigned;
    for (_, i) in remainders.iter().cycle().take(left.max(1) * segs.len()) {
        if left == 0 {
            break;
        }
        counts[*i] += 1;
        left -= 1;
    }
    let mut words = Vec::with_capacity(expected_word_count);
    for ((s, e), n) in segs.iter().zip(counts) {
        if n == 0 {
            continue;
        }
        let w = (e - s) / n as f64;
        for i in 0..n {
            words.push((s + i as f64 * w, s + (i + 1) as f64 * w));
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// AM-modulated harmonic stack at 150 Hz with formant peaks; `active`
    /// lists the speech segments in seconds.
    fn synthetic_speech(fs: u32, seconds: f64, active: &[(f64, f64)]) -> AudioBuffer {
        let frames = (seconds * fs as f64) as usize;
        let mut b = AudioBuffer::new(fs, 1, frames);
        let f0 = 150.0;
        let formants = [500.0, 1500.0, 2500.0];
        let harmonics: Vec<(f64, f64)> = (1..=26)
            .map(|h| {
                let f = f0 * h as f64;
                let w: f64 = formants
                    .iter()
                    .map(|fm| (-((f - fm) / 180.0).powi(2)).exp())
                    .sum::<f64>()
                    + 0.05;
                (f, w)
            })
            .collect();
        let norm: f64 = harmonics.iter().map(|(_, w)| w).sum();
        for i in 0..frames {
            let t = i as f64 / fs as f64;
            let mut env = 0.0;
            for (s, e) in active {
                if t >= *s && t < *e {
                    let ramp = ((t - s) / 0.02).min((e - t) / 0.02).min(1.0);
                    env = ramp * (0.65 + 0.35 * (2.0 * PI * 4.0 * (t - s)).sin());
                }
            }
            if env > 0.0 {
                let v: f64 = harmonics
                    .iter()
                    .map(|(f, w)| w * (2.0 * PI * f * t).sin())
                    .sum::<f64>()
                    / norm;
                b.data[i] = (v * env * 0.9) as f32;
            }
        }
        b
    }

    fn snr_db(clean: &[f32], test: &[f32]) -> f64 {
        let sig: f64 = clean.iter().map(|s| (*s as f64).powi(2)).sum();
        let err: f64 = clean
            .iter()
            .zip(test)
            .map(|(s, t)| ((*s - *t) as f64).powi(2))
            .sum();
        10.0 * (sig / err.max(1e-20)).log10()
    }

    #[test]
    fn isolation_improves_snr_and_detects_speech() {
        let fs = 48_000;
        let active = [(0.5, 1.5), (2.0, 3.0)];
        let speech = synthetic_speech(fs, 3.5, &active);
        // Noise scaled for 0 dB SNR measured over the speech regions.
        let mut noise = AudioBuffer::noise(fs, 1, 1.0, 3.5, 42);
        let speech_rms = {
            let mut sum = 0.0f64;
            let mut n = 0usize;
            for (s, e) in &active {
                for i in (s * fs as f64) as usize..(e * fs as f64) as usize {
                    sum += (speech.data[i] as f64).powi(2);
                    n += 1;
                }
            }
            (sum / n as f64).sqrt() as f32
        };
        noise.gain(speech_rms / noise.rms());
        let mut mixed = speech.clone();
        for (m, n) in mixed.data.iter_mut().zip(&noise.data) {
            *m += n;
        }
        let snr_in = snr_db(&speech.data, &mixed.data);
        let result = isolate_dialogue(&mixed, IsolationParams::default()).unwrap();
        assert_eq!(result.dialogue.frames(), mixed.frames());
        let snr_out = snr_db(&speech.data, &result.dialogue.data);
        let gain = snr_out - snr_in;
        assert!(
            gain >= 8.0,
            "SNR improvement {gain:.2} dB (in {snr_in:.2}, out {snr_out:.2})"
        );
        // Residual + dialogue == input.
        for i in (0..mixed.frames()).step_by(997) {
            assert!(
                (result.dialogue.data[i] + result.residual.data[i] - mixed.data[i]).abs() < 1e-5
            );
        }
        // VAD finds both segments within 100 ms.
        assert_eq!(result.vad.len(), 2, "vad: {:?}", result.vad);
        for ((s, e), (ts, te)) in result.vad.iter().zip(&active) {
            assert!((s - ts).abs() <= 0.1, "start {s} vs {ts}");
            assert!((e - te).abs() <= 0.1, "end {e} vs {te}");
        }
        // With an explicit noise profile the result is at least as good.
        let profile = noise.slice(0, fs as usize / 2);
        let with_profile = isolate_dialogue(
            &mixed,
            IsolationParams {
                noise_profile: Some(&profile),
                ..Default::default()
            },
        )
        .unwrap();
        let snr_profile = snr_db(&speech.data, &with_profile.dialogue.data);
        assert!(
            snr_profile - snr_in >= 8.0,
            "profiled SNR gain {:.2}",
            snr_profile - snr_in
        );
    }

    #[test]
    fn stereo_input_and_empty_input() {
        let s = synthetic_speech(24_000, 1.0, &[(0.2, 0.8)]).with_channels(2);
        let r = isolate_dialogue(
            &s,
            IsolationParams {
                fft_size: 1024,
                hop: 256,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.dialogue.channels, 2);
        assert_eq!(r.vad.len(), 1);
        assert!(
            isolate_dialogue(&AudioBuffer::new(48_000, 1, 0), IsolationParams::default()).is_err()
        );
    }

    #[test]
    fn words_hint_splits_segments_evenly() {
        let vad = [(0.0, 1.0), (2.0, 4.0)];
        let words = dialogue_segments_to_words_hint(&vad, 6);
        assert_eq!(words.len(), 6);
        assert_eq!(words[0], (0.0, 0.5));
        assert_eq!(words[1], (0.5, 1.0));
        assert!((words[2].0 - 2.0).abs() < 1e-9 && (words[5].1 - 4.0).abs() < 1e-9);
        assert_eq!(dialogue_segments_to_words_hint(&vad, 7).len(), 7);
        assert_eq!(dialogue_segments_to_words_hint(&vad, 1).len(), 1);
        assert!(dialogue_segments_to_words_hint(&[], 3).is_empty());
        assert!(dialogue_segments_to_words_hint(&vad, 0).is_empty());
    }
}
