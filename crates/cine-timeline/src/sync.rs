//! Dual-system audio sync: find the offset between a camera's scratch audio
//! and an external recorder by cross-correlation.
//!
//! Two stages:
//!
//! 1. **Coarse** — RMS envelopes (window [`ENVELOPE_WINDOW`] samples),
//!    normalized to zero mean / unit variance, are cross-correlated with an
//!    FFT. The envelope is robust to differing microphones, gain and EQ,
//!    and the FFT makes long recordings cheap.
//! 2. **Fine** — the raw waveforms are cross-correlated around the coarse
//!    lag (± two windows) on a bounded segment, giving sample accuracy.
//!
//! Confidence is `peak / mean(|correlation|)` of the coarse stage: a clean
//! match is typically well above 5, a random one hovers around 2–3.

use crate::edit;
use crate::error::Result;
use crate::otio::{Item, Track};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use ucg_core::RationalTime;

/// Samples per RMS envelope frame.
pub const ENVELOPE_WINDOW: usize = 512;
/// Longest raw segment used by the fine stage.
const FINE_MAX_SAMPLES: usize = 1 << 18;

/// The result of [`find_offset`].
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct SyncResult {
    /// Delay of the candidate relative to the reference: the candidate's
    /// content at sample `n + offset_samples` matches the reference at `n`.
    pub offset_samples: i64,
    /// `offset_samples / sample_rate`.
    pub offset_seconds: f64,
    /// Peak-to-mean ratio of the correlation; `0` when no estimate was possible.
    pub confidence: f64,
}

/// RMS envelope: one value per non-overlapping `window` of samples
/// (the trailing partial window is included).
pub fn rms_envelope(samples: &[f32], window: usize) -> Vec<f32> {
    let window = window.max(1);
    samples
        .chunks(window)
        .map(|c| {
            let sum: f64 = c.iter().map(|&x| (x as f64) * (x as f64)).sum();
            (sum / c.len() as f64).sqrt() as f32
        })
        .collect()
}

/// In place: subtract the mean and divide by the standard deviation.
fn normalize(v: &mut [f32]) {
    if v.is_empty() {
        return;
    }
    let n = v.len() as f64;
    let mean = v.iter().map(|&x| x as f64).sum::<f64>() / n;
    let var = v.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / n;
    let sd = var.sqrt();
    for x in v.iter_mut() {
        *x = if sd > 0.0 {
            ((*x as f64 - mean) / sd) as f32
        } else {
            0.0
        };
    }
}

/// Full cross-correlation `c[k] = Σ_n a[n] · b[n + k]` for
/// `k ∈ [-(len_a - 1), len_b - 1]`, via FFT. Returns `(values, first_lag)`.
pub fn cross_correlate(a: &[f32], b: &[f32]) -> (Vec<f64>, i64) {
    if a.is_empty() || b.is_empty() {
        return (Vec::new(), 0);
    }
    let n = (a.len() + b.len() - 1).next_power_of_two();
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);

    let mut fa: Vec<Complex<f64>> = a.iter().map(|&x| Complex::new(x as f64, 0.0)).collect();
    fa.resize(n, Complex::new(0.0, 0.0));
    let mut fb: Vec<Complex<f64>> = b.iter().map(|&x| Complex::new(x as f64, 0.0)).collect();
    fb.resize(n, Complex::new(0.0, 0.0));
    fft.process(&mut fa);
    fft.process(&mut fb);
    for (x, y) in fa.iter_mut().zip(fb.iter()) {
        *x = x.conj() * y;
    }
    ifft.process(&mut fa);
    let scale = 1.0 / n as f64;

    let first_lag = -(a.len() as i64 - 1);
    let last_lag = b.len() as i64 - 1;
    let values = (first_lag..=last_lag)
        .map(|k| {
            let idx = if k < 0 { n as i64 + k } else { k } as usize;
            fa[idx].re * scale
        })
        .collect();
    (values, first_lag)
}

/// Lag of the largest (positive) correlation and the peak-to-mean ratio.
fn peak_of(values: &[f64], first_lag: i64) -> Option<(i64, f64)> {
    if values.is_empty() {
        return None;
    }
    let (best_i, best) =
        values
            .iter()
            .enumerate()
            .fold((0usize, f64::NEG_INFINITY), |(bi, bv), (i, &v)| {
                if v > bv {
                    (i, v)
                } else {
                    (bi, bv)
                }
            });
    let mean_abs = values.iter().map(|v| v.abs()).sum::<f64>() / values.len() as f64;
    let confidence = if mean_abs > 0.0 && best.is_finite() {
        best / mean_abs
    } else {
        0.0
    };
    Some((first_lag + best_i as i64, confidence))
}

fn sample_at(v: &[f32], i: i64) -> f32 {
    if i < 0 {
        0.0
    } else {
        v.get(i as usize).copied().unwrap_or(0.0)
    }
}

/// Find how far `candidate` is delayed relative to `reference` (see
/// [`SyncResult::offset_samples`]). The candidate may be shorter or longer
/// than the reference; the search covers every possible overlap.
pub fn find_offset(reference: &[f32], candidate: &[f32], sample_rate: u32) -> SyncResult {
    let sr = sample_rate.max(1) as f64;
    if reference.is_empty() || candidate.is_empty() {
        return SyncResult::default();
    }

    // ---- coarse: envelopes ------------------------------------------------
    let mut env_ref = rms_envelope(reference, ENVELOPE_WINDOW);
    let mut env_cand = rms_envelope(candidate, ENVELOPE_WINDOW);
    let (coarse_samples, confidence) = if env_ref.len() >= 2 && env_cand.len() >= 2 {
        normalize(&mut env_ref);
        normalize(&mut env_cand);
        let (values, first) = cross_correlate(&env_ref, &env_cand);
        match peak_of(&values, first) {
            Some((lag, conf)) => (lag * ENVELOPE_WINDOW as i64, conf),
            None => return SyncResult::default(),
        }
    } else {
        // Too short for envelopes: raw correlation over everything.
        let mut a = reference.to_vec();
        let mut b = candidate.to_vec();
        normalize(&mut a);
        normalize(&mut b);
        let (values, first) = cross_correlate(&a, &b);
        let Some((lag, conf)) = peak_of(&values, first) else {
            return SyncResult::default();
        };
        return SyncResult {
            offset_samples: lag,
            offset_seconds: lag as f64 / sr,
            confidence: conf,
        };
    };

    // ---- fine: raw samples around the coarse lag --------------------------
    let half = 2 * ENVELOPE_WINDOW as i64;
    let ref_len = reference.len() as i64;
    let cand_len = candidate.len() as i64;
    // Overlap region of the reference for which candidate[n + coarse] exists.
    let start = (-coarse_samples).max(0);
    let end = (cand_len - coarse_samples).min(ref_len);
    if end - start < 2 {
        return SyncResult {
            offset_samples: coarse_samples,
            offset_seconds: coarse_samples as f64 / sr,
            confidence,
        };
    }
    let seg_len = ((end - start) as usize).min(FINE_MAX_SAMPLES) as i64;
    // Centre the segment in the overlap so both signals have real content.
    let seg_start = start + (end - start - seg_len) / 2;
    let mut a: Vec<f32> = (0..seg_len)
        .map(|i| sample_at(reference, seg_start + i))
        .collect();
    let cand_base = seg_start + coarse_samples - half;
    let mut b: Vec<f32> = (0..seg_len + 2 * half)
        .map(|i| sample_at(candidate, cand_base + i))
        .collect();
    normalize(&mut a);
    normalize(&mut b);
    let (values, first) = cross_correlate(&a, &b);
    // Valid fine lags k ∈ [0, 2·half]; true delay D = coarse + k - half.
    let lo = (0 - first) as usize;
    let hi = ((2 * half) - first) as usize;
    let window = &values[lo.min(values.len())..(hi + 1).min(values.len())];
    let offset = match peak_of(window, 0) {
        Some((k, _)) if !window.is_empty() => coarse_samples + k - half,
        _ => coarse_samples,
    };
    SyncResult {
        offset_samples: offset,
        offset_seconds: offset as f64 / sr,
        confidence,
    }
}

/// Sync every clip of a track against an external recording.
///
/// `track_audio` is the rendered audio of `track` (sample `0` at track time
/// zero); each clip's window of it is used as the reference and
/// `external_audio` as the candidate, so a result's `offset_samples` is
/// where in the external recording that clip's content begins. Clips whose
/// window is empty are skipped.
pub fn sync_clips(
    track: &Track,
    track_audio: &[f32],
    external_audio: &[f32],
    sample_rate: u32,
) -> Vec<(usize, SyncResult)> {
    let sr = sample_rate as f64;
    let mut out = Vec::new();
    for (i, item) in track.children.iter().enumerate() {
        if !matches!(item, Item::Clip(_)) {
            continue;
        }
        let Some(range) = track.range_of_child(i) else {
            continue;
        };
        let (s, d) = range.to_seconds();
        let a = ((s * sr).round().max(0.0)) as usize;
        let b = (((s + d) * sr).round().max(0.0)) as usize;
        let b = b.min(track_audio.len());
        if a >= b {
            continue;
        }
        out.push((
            i,
            find_offset(&track_audio[a..b], external_audio, sample_rate),
        ));
    }
    out
}

/// Slip the audio clip at `index` so that it plays its media starting
/// `offset` after the media's available start (or after media time zero
/// when the available range is unknown). Returns the slip actually applied.
pub fn align_clip(track: &mut Track, index: usize, offset: RationalTime) -> Result<RationalTime> {
    let clip = track
        .children
        .get(index)
        .and_then(Item::as_clip)
        .ok_or(crate::Error::NotAnItem(index))?;
    let current = clip
        .trimmed_range()
        .map(|r| r.start_time)
        .unwrap_or(RationalTime::new(0.0, offset.rate));
    let base = clip
        .available_range()
        .map(|r| r.start_time)
        .unwrap_or(RationalTime::new(0.0, offset.rate));
    let target = base + offset;
    edit::slip(track, index, target - current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_and_normalize() {
        let s: Vec<f32> = (0..1024).map(|i| if i < 512 { 0.5 } else { 0.0 }).collect();
        let mut e = rms_envelope(&s, 512);
        assert_eq!(e.len(), 2);
        assert!((e[0] - 0.5).abs() < 1e-6 && e[1].abs() < 1e-6);
        normalize(&mut e);
        assert!((e[0] - 1.0).abs() < 1e-6 && (e[1] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn correlation_finds_a_simple_shift() {
        let a: Vec<f32> = vec![0.0, 1.0, 0.0, 0.0, 0.0];
        let b: Vec<f32> = vec![0.0, 0.0, 0.0, 1.0, 0.0];
        let (v, first) = cross_correlate(&a, &b);
        let (lag, _) = peak_of(&v, first).unwrap();
        assert_eq!(lag, 2);
    }

    #[test]
    fn empty_inputs_are_harmless() {
        assert_eq!(find_offset(&[], &[1.0], 48000), SyncResult::default());
        assert_eq!(find_offset(&[1.0], &[], 48000).confidence, 0.0);
    }

    #[test]
    fn short_signals_use_raw_path() {
        let a: Vec<f32> = (0..200).map(|i| ((i * 7) % 13) as f32 - 6.0).collect();
        let mut b = vec![0.0f32; 37];
        b.extend_from_slice(&a);
        let r = find_offset(&a, &b, 1000);
        assert_eq!(r.offset_samples, 37);
    }
}
