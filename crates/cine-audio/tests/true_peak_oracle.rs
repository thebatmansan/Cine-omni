//! `true_peak_dbtp()` measured against two independent references.
//!
//! A tone at a convenient frequency agrees with almost any interpolator, which
//! is why a sine-only test says nothing. Two references are used here:
//!
//! * **Ideal reconstruction** — the true peak of a sampled signal *is* the peak
//!   of its Whittaker–Shannon reconstruction. Zero-padding the DFT computes
//!   that exactly (for a signal that fades to zero at both ends, so the
//!   periodic extension is the signal). This is the definition, not another
//!   approximation of it, so it can adjudicate when two meters disagree.
//! * **`ebur128`** — the Rust port of `libebur128`, the reference EBU R 128
//!   implementation. A dev-dependency only: the meter under test must stand on
//!   its own at runtime.
//!
//! Tolerance throughout is EBU Tech 3341 §2.6 / Table 1 signals 15–23:
//! **+0.2 / −0.4 dBTP**, which that document says must cover "any pass-band
//! ripple in the upsampling filter and the 'under-read' described in
//! ITU-R BS.1770". Over-reading is the tightly bounded direction because a
//! meter that over-reads costs real headroom on every delivery.

use cine_audio::buffer::AudioBuffer;
use ebur128::{EbuR128, Mode};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// EBU Tech 3341 Table 1, signals 15–23.
const TOL_OVER: f64 = 0.2;
const TOL_UNDER: f64 = -0.4;

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

/// Peak of the ideal band-limited reconstruction, in dBTP.
///
/// `frames` must be a power of two and the signal must fade to zero at both
/// ends; otherwise the DFT's periodic extension has a step in it and the
/// "ideal" peak is the peak of a different signal.
fn ideal_dbtp(buf: &AudioBuffer) -> f64 {
    const OVERSAMPLE: usize = 64;
    let n = buf.frames();
    assert!(
        n.is_power_of_two(),
        "ideal_dbtp needs a power-of-two length"
    );
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n * OVERSAMPLE);
    let mut peak = 0.0f64;
    for c in 0..buf.channels {
        let mut spectrum: Vec<Complex<f64>> = buf
            .channel(c)
            .iter()
            .map(|s| Complex::new(*s as f64, 0.0))
            .collect();
        fwd.process(&mut spectrum);
        // Zero-pad the spectrum, splitting the Nyquist bin so the result stays
        // real: the unique band-limited interpolant of the samples.
        let big = n * OVERSAMPLE;
        let half = n / 2;
        let mut padded = vec![Complex::new(0.0, 0.0); big];
        padded[..half].copy_from_slice(&spectrum[..half]);
        for k in (half + 1)..n {
            padded[big - (n - k)] = spectrum[k];
        }
        padded[half] = spectrum[half] * 0.5;
        padded[big - half] = spectrum[half] * 0.5;
        inv.process(&mut padded);
        for v in &padded {
            peak = peak.max((v.re / n as f64).abs());
        }
    }
    20.0 * peak.log10()
}

/// Reference meter reading in dBTP: max over channels of `ebur128`'s true peak.
fn ebur128_dbtp(buf: &AudioBuffer) -> f64 {
    let mut m = EbuR128::new(buf.channels as u32, buf.sample_rate, Mode::TRUE_PEAK)
        .expect("ebur128 rejected the buffer format");
    m.add_frames_f32(&buf.data).expect("ebur128 add_frames");
    let lin = (0..buf.channels)
        .map(|c| m.true_peak(c as u32).expect("ebur128 true_peak"))
        .fold(0.0f64, f64::max);
    20.0 * lin.log10()
}

// ---------------------------------------------------------------------------
// Programme-like material
// ---------------------------------------------------------------------------

/// Deterministic noise, band-limited in the frequency domain so its bandwidth
/// is exact and its periodic extension is smooth — both required by
/// `ideal_dbtp`.
fn band_limited_noise(
    rate: u32,
    channels: usize,
    frames: usize,
    fmax: f64,
    seed: u64,
) -> AudioBuffer {
    assert!(frames.is_power_of_two());
    let mut planner = FftPlanner::<f64>::new();
    let inv = planner.plan_fft_inverse(frames);
    let kmax = ((fmax * frames as f64) / rate as f64) as usize;
    let mut state = seed | 1;
    let mut rnd = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut planes = Vec::with_capacity(channels);
    for _ in 0..channels {
        let mut spec = vec![Complex::new(0.0, 0.0); frames];
        for k in 1..kmax.min(frames / 2) {
            let phase = rnd() * std::f64::consts::TAU;
            let v = Complex::from_polar(1.0, phase);
            spec[k] = v;
            spec[frames - k] = v.conj();
        }
        inv.process(&mut spec);
        planes.push(spec.iter().map(|c| c.re as f32).collect::<Vec<f32>>());
    }
    let mut b = AudioBuffer::from_planar(rate, &planes).unwrap();
    let g = 1.0 / b.peak().max(1e-12);
    b.gain(g);
    b
}

/// Soft-clip a buffer and re-band-limit it: what a loudness-war master looks
/// like, and the shape that actually produces inter-sample peaks well above
/// the sample peak. Re-band-limiting removes the aliasing the clipper made, so
/// the result is still a legitimate 20 kHz-limited signal.
fn clipped_and_band_limited(src: &AudioBuffer, ceiling: f32, fmax: f64) -> AudioBuffer {
    let frames = src.frames();
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(frames);
    let inv = planner.plan_fft_inverse(frames);
    let kmax = ((fmax * frames as f64) / src.sample_rate as f64) as usize;
    let mut planes = Vec::with_capacity(src.channels);
    for c in 0..src.channels {
        let mut spec: Vec<Complex<f64>> = src
            .channel(c)
            .iter()
            .map(|s| Complex::new(s.clamp(-ceiling, ceiling) as f64, 0.0))
            .collect();
        fwd.process(&mut spec);
        for (k, bin) in spec.iter_mut().enumerate() {
            if k >= kmax && k <= frames - kmax {
                *bin = Complex::new(0.0, 0.0);
            }
        }
        inv.process(&mut spec);
        planes.push(
            spec.iter()
                .map(|v| (v.re / frames as f64) as f32)
                .collect::<Vec<f32>>(),
        );
    }
    let mut b = AudioBuffer::from_planar(src.sample_rate, &planes).unwrap();
    let g = 0.891_25 / b.peak().max(1e-12); // sample peak −1 dBFS, as delivered
    b.gain(g);
    b
}

fn cases() -> Vec<(String, AudioBuffer)> {
    let mut v = Vec::new();
    for (rate, chans, fmax, seed, label) in [
        (48_000u32, 2usize, 8_000.0f64, 11u64, "8 kHz"),
        (48_000, 2, 16_000.0, 22, "16 kHz"),
        (48_000, 2, 20_000.0, 33, "20 kHz"),
        (44_100, 2, 18_000.0, 44, "18 kHz"),
        (48_000, 6, 20_000.0, 55, "20 kHz 5.1"),
    ] {
        let n = band_limited_noise(rate, chans, 32_768, fmax, seed);
        v.push((
            format!("noise {label} {rate} {chans}ch"),
            clipped_and_band_limited(&n, 0.35, fmax),
        ));
    }
    v
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn true_peak_tracks_ideal_reconstruction_within_ebu_tolerance() {
    let mut failures = Vec::new();
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "signal", "sample", "ideal", "ours", "ebur128", "ours-ideal"
    );
    for (name, buf) in cases() {
        let ideal = ideal_dbtp(&buf);
        let ours = buf.true_peak_dbtp() as f64;
        let reference = ebur128_dbtp(&buf);
        let err = ours - ideal;
        println!(
            "{name:<28} {:>9.4} {ideal:>9.4} {ours:>9.4} {reference:>9.4} {err:>+9.4}",
            buf.peak_db()
        );
        if !(TOL_UNDER..=TOL_OVER).contains(&err) {
            failures.push(format!("{name}: {err:+.4} dB from ideal"));
        }
    }
    assert!(
        failures.is_empty(),
        "outside EBU Tech 3341 +0.2/-0.4 dBTP:\n  {}",
        failures.join("\n  ")
    );
}

/// The adjudicator has to be trustworthy before it can adjudicate: a single
/// DFT-bin sinusoid has an ideal peak exactly equal to its amplitude, at every
/// frequency and phase, so `ideal_dbtp` must return that and nothing else.
#[test]
fn ideal_reconstruction_oracle_is_exact_on_single_bin_sinusoids() {
    const N: usize = 32_768;
    for bin in [1usize, 683, 5_461, 10_923, 13_653, 16_000] {
        for phase_deg in [0.0f64, 45.0, 60.0, 67.5] {
            let f = bin as f64 * 48_000.0 / N as f64;
            let b = AudioBuffer::sine_with_phase(
                48_000,
                1,
                f,
                0.5,
                N as f64 / 48_000.0,
                phase_deg.to_radians(),
            );
            assert_eq!(b.frames(), N);
            let d = ideal_dbtp(&b) - (-6.020_599_913_279_624);
            assert!(d.abs() < 1e-6, "bin {bin} phase {phase_deg}: {d:+.9} dB");
        }
    }
}

/// Full-band material (energy right up to Nyquist) is where every 4x meter
/// runs out of road. It is outside the band EBU Tech 3341 qualifies, so it is
/// reported rather than asserted on — but the two meters must not diverge
/// silently, and the direction of each one's error is the finding.
#[test]
fn report_full_band_shortfall() {
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "signal", "sample", "ideal", "ours", "ebur128", "ours-ideal"
    );
    for (rate, chans, fmax, seed, label) in [
        (48_000u32, 2usize, 24_000.0f64, 66u64, "full band 48k"),
        (44_100, 2, 22_050.0, 77, "full band 44.1k"),
        (48_000, 2, 22_000.0, 88, "22 kHz 48k"),
    ] {
        let n = band_limited_noise(rate, chans, 32_768, fmax, seed);
        let buf = clipped_and_band_limited(&n, 0.35, fmax);
        let ideal = ideal_dbtp(&buf);
        let ours = buf.true_peak_dbtp() as f64;
        let reference = ebur128_dbtp(&buf);
        println!(
            "{:<28} {:>9.4} {ideal:>9.4} {ours:>9.4} {reference:>9.4} {:>+9.4}  (ebur128-ideal {:+.4})",
            format!("noise {label} {chans}ch"),
            buf.peak_db(),
            ours - ideal,
            reference - ideal
        );
    }
}

/// Ceiling on `true_peak_dbtp`'s cost, as a multiple of one plain arithmetic
/// pass (`rms()`) over the same samples. Measured on 20 s of 5.1: **104x** for
/// the polyphase filter, **578x** for the generic resampler it replaced. The
/// budget sits between them with roughly 2.4x of room on either side.
const COST_BUDGET: f64 = 250.0;

/// A true-peak meter is run over every render and every delivery check, so it
/// has to be a filter with precomputed coefficients — not a general-purpose
/// resampler evaluating two transcendentals per tap per output sample. 20 s of
/// 5.1 is one reel-length measurement.
///
/// The cost is expressed as a multiple of a plain arithmetic pass over the same
/// samples, so the tripwire measures the algorithm rather than how busy the
/// machine is: a wall-clock threshold that holds on an idle box fires
/// spuriously on a loaded one, and this crate's tests share a machine with the
/// rest of the workspace's builds.
#[test]
fn true_peak_costs_a_filter_not_a_resampler() {
    let buf = AudioBuffer::noise(48_000, 6, 0.5, 20.0, 7);
    // Best of five: the fastest run is the one least disturbed by other load.
    let mut baseline = std::time::Duration::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        std::hint::black_box(buf.rms());
        baseline = baseline.min(t.elapsed());
    }
    let t = std::time::Instant::now();
    std::hint::black_box(buf.true_peak_dbtp());
    let elapsed = t.elapsed();
    let ratio = elapsed.as_secs_f64() / baseline.as_secs_f64().max(1e-9);
    println!("20 s of 5.1 at 48 kHz: {elapsed:?} = {ratio:.0}x a plain pass (rms {baseline:?})");
    assert!(
        ratio < COST_BUDGET,
        "true_peak_dbtp cost {ratio:.0}x a plain pass over the same samples \
         (budget {COST_BUDGET}x); it is recomputing its kernel again"
    );
}

/// Budget for the reconstruction grid alone. EBU Tech 3341 allows
/// +0.2/−0.4 dBTP *in total*, and that total has to cover the interpolation
/// filter as well; spending it all on a grid too coarse to land near the peak
/// leaves nothing for the filter. A 4x grid misses a 20 kHz peak by up to
/// 20·log10(cos(π·20000/192000)) = −0.47 dB on grid coarseness alone — already
/// past EBU's limit before the filter has contributed anything.
const GRID_BUDGET_DB: f64 = 0.15;

/// A peak lands wherever it lands. Sweeping the phase of a tone walks its
/// continuous peak across the whole gap between samples, so the worst reading
/// over the sweep is the worst the meter can do on that tone — including the
/// midpoints a coarse oversampling grid steps straight over. The tones are
/// high but legal: 20 kHz is inside the passband of 44.1 and 48 kHz delivery.
#[test]
fn peaks_between_samples_are_resolved_at_every_delivery_rate() {
    const TRUTH_DBTP: f64 = -6.020_599_913_279_624; // amplitude 0.5
    let mut failures = Vec::new();
    for rate in [44_100u32, 48_000, 96_000] {
        for f in [12_000.0f64, 16_000.0, 20_000.0] {
            if f >= rate as f64 / 2.0 {
                continue;
            }
            let mut worst = 0.0f64;
            for i in 0..48 {
                let phase = i as f64 / 48.0 * std::f64::consts::TAU;
                let tone = fade_10ms(AudioBuffer::sine_with_phase(rate, 1, f, 0.5, 0.15, phase));
                let err = tone.true_peak_dbtp() as f64 - TRUTH_DBTP;
                if err.abs() > worst.abs() {
                    worst = err;
                }
            }
            println!("{f:>7.0} Hz tone at {rate:>6} Hz: worst {worst:+.4} dB");
            if worst.abs() > GRID_BUDGET_DB {
                failures.push(format!("{f:.0} Hz at {rate} Hz: {worst:+.4} dB"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "worse than the {GRID_BUDGET_DB} dB grid budget:\n  {}",
        failures.join("\n  ")
    );
}

/// 10 ms taper, as EBU Tech 3341 requires of its own test tones: without it the
/// step at the buffer edge rings and the meter measures that instead.
fn fade_10ms(mut b: AudioBuffer) -> AudioBuffer {
    let frames = b.frames();
    let n = (b.sample_rate as usize / 100).min(frames / 2);
    let ch = b.channels;
    for i in 0..n {
        let g = i as f32 / n as f32;
        for c in 0..ch {
            b.data[i * ch + c] *= g;
            b.data[(frames - 1 - i) * ch + c] *= g;
        }
    }
    b
}

#[test]
fn true_peak_agrees_with_reference_meter() {
    let mut failures = Vec::new();
    for (name, buf) in cases() {
        let delta = buf.true_peak_dbtp() as f64 - ebur128_dbtp(&buf);
        // Both meters approximate the same quantity and are allowed to differ
        // by about what the standard allows either of them to be wrong by.
        // `ebur128` is the one that under-reads here — see
        // `true_peak_tracks_ideal_reconstruction_within_ebu_tolerance`, where
        // it sits up to 0.24 dB below ideal reconstruction and this crate sits
        // within 0.05 dB — so a positive delta is expected, not a defect.
        if delta.abs() > 0.25 {
            failures.push(format!("{name}: {delta:+.4} dB from ebur128"));
        }
    }
    assert!(
        failures.is_empty(),
        "meters disagree:\n  {}",
        failures.join("\n  ")
    );
}
