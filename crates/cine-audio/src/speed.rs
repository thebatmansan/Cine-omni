//! Reading sound at a speed: the band-limited kernel, and exact positions.
//!
//! Playing a clip faster, slower, backwards, or from a file at another sample
//! rate are the same operation: every output sample reads the source at some
//! position that is generally *between* two source samples. Two things decide
//! whether that is right.
//!
//! * **Where.** The position is exact rational arithmetic — output sample `u`
//!   reads source position `u × num / den` — never an accumulated float, so it
//!   does not drift over a long clip, and reading from the middle gives the same
//!   samples as playing up to it. [`Stepper`] holds that ratio.
//! * **How.** Between samples the value comes from a 16-tap Hann-windowed sinc
//!   ([`kernel`]); when the source is being read faster than one sample per
//!   output sample the kernel's cutoff falls with the rate, so what cannot be
//!   represented at the output is filtered out rather than folded back in as
//!   aliasing. [`KernelTable`] tabulates the kernel over fractional positions,
//!   because deriving it per sample was the expensive part.
//!
//! This is *varispeed*: pitch follows speed, as it does on tape. Keeping pitch
//! while changing speed is a different operation and is not this module.

use std::f64::consts::PI;

/// Taps in the kernel, and half that.
pub const TAPS: usize = 16;
pub const HALF: usize = TAPS / 2;

/// The 16 Hann-windowed sinc weights for a fractional position, and the
/// reciprocal of their sum.
///
/// Tap `k` sits at source sample `center - HALF + 1 + k`, a distance
/// `k - HALF + 1 - frac` from the point being read. Normalising to unit DC
/// gain keeps every fractional position agreeing on a steady signal, which a
/// truncated window otherwise breaks.
pub fn kernel(frac: f64, cutoff: f64) -> ([f64; TAPS], f64) {
    let mut w = [0.0f64; TAPS];
    let mut sum = 0.0;
    for (i, wv) in w.iter_mut().enumerate() {
        let x = (i as i64 - HALF as i64 + 1) as f64 - frac;
        let sinc = if x.abs() < 1e-9 {
            1.0
        } else {
            (PI * cutoff * x).sin() / (PI * cutoff * x)
        };
        let win = if x.abs() >= HALF as f64 {
            0.0
        } else {
            0.5 * (1.0 + (PI * x / HALF as f64).cos())
        };
        *wv = sinc * cutoff * win;
        sum += *wv;
    }
    (w, if sum.abs() > 1e-12 { 1.0 / sum } else { 1.0 })
}

/// The kernel tabulated over fractional positions.
///
/// A position's phase is quantised to 1/`PHASES` of a sample. The error that
/// introduces is a timing error of at most half a phase — 1/8192 of a sample —
/// which for a 10 kHz tone at 48 kHz is a phase error of 0.00016 rad, about
/// −76 dB. That is below the kernel's own stopband, so the table costs nothing
/// audible and saves sixteen transcendentals per tap per sample.
pub struct KernelTable {
    cutoff: f64,
    phases: Vec<([f32; TAPS], f32)>,
}

/// Fractional positions tabulated per sample.
pub const PHASES: usize = 4096;

impl KernelTable {
    pub fn new(cutoff: f64) -> KernelTable {
        let phases = (0..PHASES)
            .map(|p| {
                let (w, norm) = kernel(p as f64 / PHASES as f64, cutoff);
                let mut out = [0.0f32; TAPS];
                for (o, v) in out.iter_mut().zip(w) {
                    *o = (v * norm) as f32;
                }
                (out, 1.0)
            })
            .collect();
        KernelTable { cutoff, phases }
    }

    pub fn cutoff(&self) -> f64 {
        self.cutoff
    }

    /// Read a stereo source at `center + frac`. `get` returns the source
    /// sample at an index, and silence outside what exists.
    pub fn read(
        &self,
        center: i64,
        frac: f64,
        mut get: impl FnMut(i64) -> (f32, f32),
    ) -> (f32, f32) {
        let phase = ((frac * PHASES as f64).round() as usize).min(PHASES);
        // A phase that rounds up to a whole sample is the next sample exactly.
        let (center, phase) = if phase == PHASES {
            (center + 1, 0)
        } else {
            (center, phase)
        };
        let (w, _) = &self.phases[phase];
        let base = center - HALF as i64 + 1;
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for (k, wk) in w.iter().enumerate() {
            let (a, b) = get(base + k as i64);
            l += wk * a;
            r += wk * b;
        }
        (l, r)
    }
}

/// The cutoff for reading `per_output` source samples per output sample: the
/// full band when reading at or below one-for-one, and the output's Nyquist
/// expressed in source terms when reading faster.
pub fn cutoff_for(per_output: f64) -> f64 {
    if per_output > 1.0 {
        1.0 / per_output
    } else {
        1.0
    }
}

/// Exact source position of output sample `u`: `u × num / den` source samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stepper {
    num: i128,
    den: i128,
}

impl Stepper {
    /// Source samples per output sample, as a ratio of integers. `den > 0`.
    pub fn new(num: i128, den: i128) -> Stepper {
        assert!(den > 0 && num >= 0, "a speed is a non-negative ratio");
        let g = gcd(num, den).max(1);
        Stepper {
            num: num / g,
            den: den / g,
        }
    }

    /// Reading a clip at `speed` (media time per record time, `media/track`)
    /// from a file at `file_rate` onto an output at `out_rate`; rates as
    /// `num/den` hertz.
    pub fn for_speed(
        (media, track): (i64, i64),
        (file_num, file_den): (u32, u32),
        (out_num, out_den): (u32, u32),
    ) -> Stepper {
        Stepper::new(
            i128::from(media) * i128::from(file_num) * i128::from(out_den),
            i128::from(track) * i128::from(file_den) * i128::from(out_num),
        )
    }

    /// Source samples per output sample, for display and for the cutoff only.
    pub fn per_output(&self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Whether this reads one source sample per output sample exactly.
    pub fn is_identity(&self) -> bool {
        self.num == self.den
    }

    /// The whole source sample at or before output sample `u`, and how far past
    /// it `u` reads, in `[0, 1)`.
    pub fn position(&self, u: i64) -> (i64, f64) {
        let idx = i128::from(u) * self.num;
        let center = idx.div_euclid(self.den);
        let rem = idx.rem_euclid(self.den);
        (center as i64, rem as f64 / self.den as f64)
    }
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: f64) -> impl Fn(i64) -> (f32, f32) {
        move |i| {
            let v = (2.0 * PI * freq * i as f64 / rate).sin() as f32;
            (v, v)
        }
    }

    /// The largest error, in dB relative to full scale, of reading a tone at
    /// `per_output` against the tone's exact value at each position.
    fn error_db(freq: f64, file_rate: f64, stepper: Stepper) -> f64 {
        let table = KernelTable::new(cutoff_for(stepper.per_output()));
        let src = sine(freq, file_rate);
        let mut worst = 0.0f64;
        for u in 1000..6000 {
            let (c, f) = stepper.position(u);
            let (got, _) = table.read(c, f, &src);
            let want = (2.0 * PI * freq * (c as f64 + f) / file_rate).sin();
            worst = worst.max((f64::from(got) - want).abs());
        }
        20.0 * worst.log10()
    }

    #[test]
    fn positions_are_exact_and_do_not_drift() {
        // 44.1 kHz read onto 48 kHz: 147 source samples per 160 output.
        let s = Stepper::for_speed((1, 1), (44_100, 1), (48_000, 1));
        assert_eq!(s.position(160), (147, 0.0));
        // An hour in, still exactly on the grid the ratio says.
        let hour = 48_000 * 3600;
        assert_eq!(s.position(hour), (44_100 * 3600, 0.0));
        let (c, f) = s.position(hour + 1);
        assert_eq!(c, 44_100 * 3600);
        assert!((f - 147.0 / 160.0).abs() < 1e-12);
    }

    #[test]
    fn a_speed_and_a_rate_compose_exactly() {
        // Double speed from 44.1 kHz onto 48 kHz: 2 × 147/160.
        let s = Stepper::for_speed((2, 1), (44_100, 1), (48_000, 1));
        assert_eq!(s.position(80), (147, 0.0));
        assert!((s.per_output() - 2.0 * 147.0 / 160.0).abs() < 1e-12);
        assert!(Stepper::for_speed((1, 1), (48_000, 1), (48_000, 1)).is_identity());
    }

    /// A 1 kHz tone from 44.1 kHz onto 48 kHz is within −60 dB of the exact
    /// tone at every sample read — the kernel, the table and the positions
    /// together.
    #[test]
    fn rate_conversion_reads_a_tone_to_within_60_db() {
        let s = Stepper::for_speed((1, 1), (44_100, 1), (48_000, 1));
        let db = error_db(1_000.0, 44_100.0, s);
        assert!(db < -60.0, "{db:.1} dB");
    }

    /// Slow motion reads between samples most of the time; the tone is still
    /// the tone.
    #[test]
    fn half_speed_reads_a_tone_to_within_60_db() {
        let s = Stepper::for_speed((1, 2), (48_000, 1), (48_000, 1));
        let db = error_db(1_000.0, 48_000.0, s);
        assert!(db < -60.0, "{db:.1} dB");
    }

    /// Reading faster lowers the cutoff, so a tone that would land above the
    /// output's Nyquist is removed instead of folding back as a false tone.
    #[test]
    fn reading_fast_filters_what_would_alias() {
        // 15 kHz at 48 kHz, read at 2x, would be 30 kHz: above 24 kHz Nyquist.
        let s = Stepper::for_speed((2, 1), (48_000, 1), (48_000, 1));
        let table = KernelTable::new(cutoff_for(s.per_output()));
        let src = sine(15_000.0, 48_000.0);
        let mut energy = 0.0f64;
        for u in 1000..9000 {
            let (c, f) = s.position(u);
            let (v, _) = table.read(c, f, &src);
            energy += f64::from(v) * f64::from(v);
        }
        let rms_db = 10.0 * (energy / 8000.0).log10() - 10.0 * 0.5f64.log10();
        assert!(
            rms_db < -20.0,
            "the tone is attenuated by {rms_db:.1} dB, not passed"
        );

        // And a tone that fits (5 kHz -> 10 kHz) passes at full level.
        let src = sine(5_000.0, 48_000.0);
        let mut energy = 0.0f64;
        for u in 1000..9000 {
            let (c, f) = s.position(u);
            let (v, _) = table.read(c, f, &src);
            energy += f64::from(v) * f64::from(v);
        }
        let rms_db = 10.0 * (energy / 8000.0).log10() - 10.0 * 0.5f64.log10();
        assert!(rms_db.abs() < 0.5, "passed at {rms_db:.2} dB");
    }

    /// At one-for-one the table reads the samples themselves.
    #[test]
    fn identity_reads_the_samples_themselves() {
        let table = KernelTable::new(1.0);
        let src = sine(997.0, 48_000.0);
        for i in 100..200 {
            let (v, _) = table.read(i, 0.0, &src);
            assert!((v - src(i).0).abs() < 1e-5, "{i}");
        }
    }
}
