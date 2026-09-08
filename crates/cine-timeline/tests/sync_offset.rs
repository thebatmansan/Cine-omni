//! Dual-system sync accuracy on a synthetic signal.

use cine_timeline::prelude::*;

const SR: u32 = 48_000;

/// Deterministic LCG noise in [-1, 1].
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

/// Sum of sines plus a pattern of short broadband bursts (claps, footsteps).
fn program(seconds: f64, seed: u64) -> Vec<f32> {
    let n = (seconds * SR as f64) as usize;
    let mut rng = Lcg(seed);
    let mut out = vec![0.0f32; n];
    for (i, s) in out.iter_mut().enumerate() {
        let t = i as f64 / SR as f64;
        *s = (0.2 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()
            + 0.15 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()
            + 0.1 * (2.0 * std::f64::consts::PI * 997.0 * t).sin()) as f32;
    }
    // bursts of 40 ms at random (aperiodic) intervals of 0.25–0.75 s
    let mut pos = 0.31;
    while pos + 0.05 < seconds {
        let start = (pos * SR as f64) as usize;
        let len = (0.04 * SR as f64) as usize;
        for s in out.iter_mut().skip(start).take(len) {
            *s += 0.8 * rng.next();
        }
        pos += 0.25 + 0.25 * (rng.next() as f64 + 1.0);
    }
    out
}

/// Independent low-level noise, as two different microphones would add.
fn mic_noise(signal: &mut [f32], seed: u64, level: f32) {
    let mut rng = Lcg(seed);
    for s in signal.iter_mut() {
        *s += level * rng.next();
    }
}

#[test]
fn finds_a_1234ms_delay_within_5ms() {
    let clean = program(8.0, 1);
    let mut reference = clean.clone();
    mic_noise(&mut reference, 2, 0.02);

    let delay = 1.234;
    let delay_samples = (delay * SR as f64).round() as i64;
    let mut candidate = vec![0.0f32; delay_samples as usize];
    candidate.extend_from_slice(&clean);
    candidate.extend(std::iter::repeat_n(0.0f32, SR as usize)); // a second of tail
    for s in candidate.iter_mut() {
        *s *= 0.6; // different gain
    }
    mic_noise(&mut candidate, 3, 0.03);

    let r = find_offset(&reference, &candidate, SR);
    assert!(
        (r.offset_seconds - delay).abs() <= 0.005,
        "offset {} s, expected {delay}",
        r.offset_seconds
    );
    assert!(
        (r.offset_samples - delay_samples).abs() <= 240,
        "offset {} samples, expected {delay_samples}",
        r.offset_samples
    );
    assert!(r.confidence > 5.0, "confidence {}", r.confidence);
}

#[test]
fn candidate_shorter_than_reference_gives_negative_offset() {
    let reference = program(10.0, 5);
    let from = 2 * SR as usize;
    let to = 5 * SR as usize;
    let mut candidate = reference[from..to].to_vec();
    mic_noise(&mut candidate, 9, 0.02);
    let r = find_offset(&reference, &candidate, SR);
    // candidate[n] == reference[n + 96000]  ⇒  offset = -96000
    assert!(
        (r.offset_samples + from as i64).abs() <= 240,
        "offset {}",
        r.offset_samples
    );
    assert!(r.confidence > 5.0);
}

#[test]
fn sync_clips_and_align() {
    // A video track with two clips; its rendered audio is the program.
    // The external recorder started 0.5 s before the camera.
    let track_audio = program(6.0, 11);
    let lead = (0.5 * SR as f64) as usize;
    let mut external = vec![0.0f32; lead];
    external.extend_from_slice(&track_audio);
    mic_noise(&mut external, 12, 0.02);

    let track = Track::new("V1", TrackKind::Video)
        .with(Clip::external(
            "shot1",
            "file:///1.mov",
            Some(TimeRange::from_frames(0, 100, 24.0)),
            Some(TimeRange::from_frames(0, 72, 24.0)),
        ))
        .with(Clip::external(
            "shot2",
            "file:///2.mov",
            Some(TimeRange::from_frames(0, 100, 24.0)),
            Some(TimeRange::from_frames(0, 72, 24.0)),
        ));
    let results = sync_clips(&track, &track_audio, &external, SR);
    assert_eq!(results.len(), 2);
    // shot1 starts at 0 s of the track → 0.5 s into the recorder file
    assert!(
        (results[0].1.offset_seconds - 0.5).abs() <= 0.005,
        "{:?}",
        results[0]
    );
    // shot2 starts at 3 s of the track → 3.5 s into the recorder file
    assert!(
        (results[1].1.offset_seconds - 3.5).abs() <= 0.005,
        "{:?}",
        results[1]
    );

    // Align an audio clip referencing the recorder file under shot2.
    let mut audio = Track::new("A1", TrackKind::Audio).with(Clip::external(
        "rec",
        "file:///rec.wav",
        Some(TimeRange::from_frames(0, 1_000_000, 48_000.0)),
        Some(TimeRange::from_frames(0, 144_000, 48_000.0)),
    ));
    let offset = RationalTime::from_frames(results[1].1.offset_samples, 48_000.0);
    let applied = align_clip(&mut audio, 0, offset).unwrap();
    assert_eq!(applied, offset);
    assert_eq!(audio.children[0].source_range().unwrap().start_time, offset);
    assert!(align_clip(&mut audio, 3, offset).is_err());
}
