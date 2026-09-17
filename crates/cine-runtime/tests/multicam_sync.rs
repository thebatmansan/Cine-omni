//! Lining multicam angles up by sound, on real files.
//!
//! Every file here is a WAV written by the test and read back through the same
//! decoder an ingested file goes through. The signal is a take's worth of room
//! tone with aperiodic bursts — claps, footsteps — heard by each camera's
//! microphone with its own noise. What is checked is the offset the sync
//! finds against the one the test built in, and that sound from a different
//! take is refused rather than lined up with a guess.

use cine_runtime::jobs::{sync_offsets, SYNC_MIN_AGREEMENT};
use std::path::{Path, PathBuf};

const SR: u32 = 48_000;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

/// `seconds` of a take: three tones and bursts at irregular intervals.
fn take(seconds: f64, seed: u64) -> Vec<f32> {
    let n = (seconds * f64::from(SR)) as usize;
    let mut rng = Lcg(seed);
    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(SR);
            (0.1 * (2.0 * std::f64::consts::PI * 180.0 * t).sin()
                + 0.05 * (2.0 * std::f64::consts::PI * 523.0 * t).sin()) as f32
        })
        .collect();
    let mut pos = 0.37;
    while pos + 0.05 < seconds {
        let start = (pos * f64::from(SR)) as usize;
        for s in out
            .iter_mut()
            .skip(start)
            .take((0.04 * f64::from(SR)) as usize)
        {
            *s += 0.7 * rng.next();
        }
        pos += 0.3 + 0.4 * (f64::from(rng.next()) + 1.0);
    }
    out
}

fn with_noise(mut v: Vec<f32>, seed: u64) -> Vec<f32> {
    let mut rng = Lcg(seed);
    for s in &mut v {
        *s += 0.02 * rng.next();
    }
    v
}

/// A 16-bit PCM mono WAV.
fn wav(dir: &Path, name: &str, samples: &[f32]) -> PathBuf {
    let mut bytes = Vec::new();
    let data_len = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&SR.to_le_bytes());
    bytes.extend_from_slice(&(SR * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn angles_are_lined_up_by_what_they_heard_and_a_stranger_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let whole = take(12.0, 1);
    // A rolled from the start. B rolled 1.25 s later, so B's media at a moment
    // of the take is 1.25 s behind A's. C rolled 0.5 s *before* A.
    let a = wav(
        dir.path(),
        "A.wav",
        &with_noise(whole[..9 * SR as usize].to_vec(), 11),
    );
    let b_start = (1.25 * f64::from(SR)) as usize;
    let b = wav(
        dir.path(),
        "B.wav",
        &with_noise(whole[b_start..].to_vec(), 12),
    );
    let mut c_samples = with_noise(take(0.5, 99), 13);
    c_samples.extend_from_slice(&with_noise(whole[..10 * SR as usize].to_vec(), 14));
    let c = wav(dir.path(), "C.wav", &c_samples);
    // D is a different take altogether.
    let d = wav(dir.path(), "D.wav", &with_noise(take(9.0, 7), 15));

    let found = sync_offsets(
        &a,
        &[("B".into(), b), ("C".into(), c), ("D".into(), d)],
        &mut |_| true,
    )
    .expect("synced");
    for f in &found {
        println!(
            "MEASURED {} confidence {:.1} agreement {:?} offset {:?}",
            f.key,
            f.confidence,
            f.agreement,
            f.offset.map(|o| o.to_seconds())
        );
    }
    let by = |k: &str| found.iter().find(|f| f.key == k).unwrap().clone();

    let b = by("B");
    let b_offset = b.offset.expect("B matched").to_seconds();
    assert!(
        (b_offset + 1.25).abs() < 0.005,
        "B is 1.25 s behind: {b_offset:.4}"
    );

    let c = by("C");
    let c_offset = c.offset.expect("C matched").to_seconds();
    assert!(
        (c_offset - 0.5).abs() < 0.005,
        "C is 0.5 s ahead: {c_offset:.4}"
    );

    let d = by("D");
    assert!(
        d.offset.is_none(),
        "a different take is not lined up: {:?}",
        d.offset
    );
    assert!(d.why_not.as_deref().unwrap().contains("no clear match"));

    // Agreement separates them with room on both sides.
    assert!(b.agreement.unwrap() > 0.9, "B {:?}", b.agreement);
    assert!(c.agreement.unwrap() > 0.9, "C {:?}", c.agreement);
    assert!(
        d.agreement.unwrap_or(0.0) < 0.5 * SYNC_MIN_AGREEMENT,
        "D {:?}",
        d.agreement
    );
}

/// How high unrelated sound scores, over many different takes: the evidence the
/// threshold rests on.
#[test]
fn strangers_stay_below_the_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav(dir.path(), "A.wav", &with_noise(take(9.0, 1), 11));
    let strangers: Vec<(String, PathBuf)> = (0..12u64)
        .map(|i| {
            let seed = 1000 + i * 37;
            (
                format!("S{i}"),
                wav(
                    dir.path(),
                    &format!("S{i}.wav"),
                    &with_noise(take(6.0 + i as f64 * 0.5, seed), seed + 1),
                ),
            )
        })
        .collect();
    let found = sync_offsets(&a, &strangers, &mut |_| true).expect("ran");
    let worst = found
        .iter()
        .filter_map(|f| f.agreement)
        .fold(f64::MIN, f64::max);
    println!(
        "MEASURED strangers (confidence, agreement): {:?}; highest agreement {worst:.2}",
        found
            .iter()
            .map(|f| (
                (f.confidence * 10.0).round() / 10.0,
                f.agreement.map(|a| (a * 100.0).round() / 100.0)
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        found.iter().all(|f| f.offset.is_none()),
        "no stranger is lined up: {found:?}"
    );
    assert!(
        worst < 0.75 * SYNC_MIN_AGREEMENT,
        "highest stranger agreement {worst:.2}"
    );
}
