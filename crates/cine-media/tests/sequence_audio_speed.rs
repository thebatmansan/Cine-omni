//! Retimed sound and files at other rates, through the mixer that plays a cut.
//!
//! Every file here is a real WAV written by the test and read back through
//! `MediaReader`, the same path an ingested file takes; what is measured is
//! what the mixer produced. Frequencies are counted from zero crossings, which
//! makes no assumption about the kernel that produced the samples.

use cine_audio::{write_wav, AudioBuffer, WavFormat};
use cine_media::sequence_audio::{SequenceAudio, SoundEntry};
use cine_media::AudioSource;
use std::path::{Path, PathBuf};
use ucg_core::{Rate, Ratio, RationalTime, TimeMap, TimeRange};

/// A constant speed for a test: media per track, and whether backwards.
#[derive(Clone, Copy)]
struct Speed {
    media: i64,
    track: i64,
    reversed: bool,
}

impl Speed {
    fn new(media: i64, track: i64, reversed: bool) -> Speed {
        Speed {
            media,
            track,
            reversed,
        }
    }
}

fn r48() -> Rate {
    Rate::whole(48_000).unwrap()
}

fn secs(num: i64, den: i64) -> RationalTime {
    RationalTime::exact(num * 48_000 / den, r48())
}

/// A mono WAV of `seconds` of `f(t)` at `rate`.
fn wav(dir: &Path, name: &str, rate: u32, seconds: f64, f: impl Fn(f64) -> f64) -> PathBuf {
    let frames = (f64::from(rate) * seconds) as usize;
    let mut buffer = AudioBuffer::new(rate, 1, frames);
    for i in 0..frames {
        buffer.data[i] = f(i as f64 / f64::from(rate)) as f32;
    }
    let path = dir.join(name);
    write_wav(&path, &buffer, WavFormat::Float32).unwrap();
    path
}

fn tone(freq: f64) -> impl Fn(f64) -> f64 {
    move |t| 0.5 * (2.0 * std::f64::consts::PI * freq * t).sin()
}

/// Read `[from, to)` output samples, left channel, in device-sized pieces.
fn render(
    entries: Vec<SoundEntry>,
    end: RationalTime,
    from: i64,
    to: i64,
) -> (Vec<f32>, SequenceAudio) {
    let mut seq = SequenceAudio::new(r48(), end, entries);
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let n = seq
            .read(at, (to - at).min(1024) as usize, &mut out)
            .unwrap();
        if n == 0 {
            break;
        }
        at += n as i64;
    }
    (out.chunks(2).map(|c| c[0]).collect(), seq)
}

/// Frequency from rising zero crossings over the samples given.
fn frequency(samples: &[f32], rate: f64) -> f64 {
    let crossings: Vec<usize> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(i, _)| i)
        .collect();
    let (first, last) = (crossings[0], *crossings.last().unwrap());
    (crossings.len() - 1) as f64 * rate / (last - first) as f64
}

fn rms_db(samples: &[f32]) -> f64 {
    let e: f64 = samples.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    10.0 * (e / samples.len() as f64).log10()
}

fn entry(path: &Path, record: TimeRange, offset: RationalTime, speed: Option<Speed>) -> SoundEntry {
    SoundEntry {
        record,
        path: Some(path.to_path_buf()),
        offset: Some(offset),
        name: "tone".into(),
        map: speed
            .map(|s| TimeMap::constant(record.duration, Ratio::new(s.media, s.track), s.reversed)),
        keep_pitch: false,
    }
}

/// A 44.1 kHz file plays at the right pitch and level on a 48 kHz output. It
/// used to be silent, with a refusal saying no resampler existed.
#[test]
fn a_file_at_another_rate_plays_at_its_own_pitch() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "a.wav", 44_100, 2.0, tone(1_000.0));
    let (out, seq) = render(
        vec![entry(
            &path,
            TimeRange::new(secs(0, 1), secs(1, 1)),
            secs(0, 1),
            None,
        )],
        secs(1, 1),
        0,
        48_000,
    );
    assert!(seq.refusals().is_empty(), "{:?}", seq.refusals());
    let f = frequency(&out[1_000..47_000], 48_000.0);
    assert!((f - 1_000.0).abs() < 0.5, "{f:.2} Hz");
    // A 0.5-amplitude sine is -9.03 dB RMS.
    let level = rms_db(&out[1_000..47_000]);
    assert!((level + 9.03).abs() < 0.1, "{level:.2} dB");
}

/// Twice speed: an octave up, and a second of record time consumes two of the
/// file.
#[test]
fn twice_speed_is_an_octave_up_and_consumes_twice_the_media() {
    let dir = tempfile::tempdir().unwrap();
    // A tone that changes at 2 s, so which media was consumed is audible.
    let path = wav(dir.path(), "b.wav", 48_000, 4.0, |t| {
        if t < 2.0 {
            tone(500.0)(t)
        } else {
            tone(1_500.0)(t)
        }
    });
    let speed = Speed::new(2, 1, false);
    let (out, _) = render(
        vec![entry(
            &path,
            TimeRange::new(secs(0, 1), secs(2, 1)),
            secs(0, 1),
            Some(speed),
        )],
        secs(2, 1),
        0,
        96_000,
    );
    let first = frequency(&out[1_000..47_000], 48_000.0);
    assert!(
        (first - 1_000.0).abs() < 0.5,
        "500 Hz at 2x is {first:.2} Hz"
    );
    let second = frequency(&out[49_000..95_000], 48_000.0);
    assert!(
        (second - 3_000.0).abs() < 1.0,
        "media from 2 s on arrives at 1 s of record: {second:.2} Hz"
    );
}

/// Backwards: the end of the range is heard first.
#[test]
fn a_reversed_clip_is_heard_from_the_end_of_its_range() {
    let dir = tempfile::tempdir().unwrap();
    // 0..1 s is 400 Hz, 1..2 s is 800 Hz.
    let path = wav(dir.path(), "c.wav", 48_000, 2.0, |t| {
        if t < 1.0 {
            tone(400.0)(t)
        } else {
            tone(800.0)(t)
        }
    });
    let speed = Speed::new(1, 1, true);
    // The whole 2 s range, backwards: the offset is the end of the range.
    let (out, seq) = render(
        vec![entry(
            &path,
            TimeRange::new(secs(0, 1), secs(2, 1)),
            secs(2, 1),
            Some(speed),
        )],
        secs(2, 1),
        0,
        96_000,
    );
    assert!(seq.refusals().is_empty());
    let first = frequency(&out[2_000..46_000], 48_000.0);
    let second = frequency(&out[50_000..94_000], 48_000.0);
    assert!((first - 800.0).abs() < 0.5, "the end first: {first:.2} Hz");
    assert!(
        (second - 400.0).abs() < 0.5,
        "then the start: {second:.2} Hz"
    );
}

/// Reading a stretch in one piece and in small ones gives the same samples, so
/// a seek or a different device period changes nothing that is heard.
#[test]
fn reading_in_pieces_gives_the_same_samples_as_reading_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "d.wav", 44_100, 3.0, tone(733.0));
    let speed = Some(Speed::new(3, 4, false));
    let make = || {
        vec![entry(
            &path,
            TimeRange::new(secs(0, 1), secs(2, 1)),
            secs(1, 4),
            speed,
        )]
    };

    let mut whole = SequenceAudio::new(r48(), secs(2, 1), make());
    let mut a = Vec::new();
    whole.read(24_000, 48_000, &mut a).unwrap();

    let mut pieces = SequenceAudio::new(r48(), secs(2, 1), make());
    let mut b = Vec::new();
    let mut at = 24_000;
    while at < 72_000 {
        let n = pieces
            .read(at, (72_000 - at).min(997) as usize, &mut b)
            .unwrap();
        at += n as i64;
    }
    assert_eq!(a.len(), b.len());
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-6, "pieces differ by {worst}");

    // And starting cold in the middle — a seek — lands on the same samples.
    let mut cold = SequenceAudio::new(r48(), secs(2, 1), make());
    let mut c = Vec::new();
    cold.read(48_000, 1_000, &mut c).unwrap();
    let worst = c
        .iter()
        .zip(&a[48_000..50_000])
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "a seek lands {worst} away");
}

/// One-for-one from a file at the output rate is the samples themselves.
#[test]
fn real_time_at_the_output_rate_is_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "e.wav", 48_000, 1.0, tone(1_234.0));
    let (out, _) = render(
        vec![entry(
            &path,
            TimeRange::new(secs(0, 1), secs(1, 2)),
            secs(1, 4),
            None,
        )],
        secs(1, 2),
        0,
        24_000,
    );
    for (i, v) in out.iter().enumerate().step_by(97) {
        let want = tone(1_234.0)((i + 12_000) as f64 / 48_000.0) as f32;
        assert!((v - want).abs() < 1e-6, "sample {i}: {v} vs {want}");
    }
}

/// Keeping pitch: at twice speed the tone stays where it was, and the media is
/// still consumed twice as fast.
#[test]
fn keeping_pitch_changes_duration_and_not_frequency() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "p.wav", 48_000, 5.0, |t| {
        if t < 2.0 {
            tone(500.0)(t)
        } else {
            tone(1_500.0)(t)
        }
    });
    let mut e = entry(
        &path,
        TimeRange::new(secs(0, 1), secs(2, 1)),
        secs(0, 1),
        Some(Speed::new(2, 1, false)),
    );
    e.keep_pitch = true;
    let (out, seq) = render(vec![e], secs(2, 1), 0, 96_000);
    assert!(seq.refusals().is_empty());
    let first = frequency(&out[4_000..44_000], 48_000.0);
    let second = frequency(&out[52_000..92_000], 48_000.0);
    assert!(
        (first - 500.0).abs() < 1.0,
        "pitch kept at 2x: {first:.2} Hz"
    );
    assert!(
        (second - 1_500.0).abs() < 1.5,
        "and media from 2 s still arrives at 1 s: {second:.2} Hz"
    );
    let level = rms_db(&out[4_000..44_000]);
    assert!((level + 9.03).abs() < 0.5, "at its level: {level:.2} dB");
}

/// A 44.1 kHz file slowed to three quarters with its pitch kept: converted and
/// stretched, still 1 kHz.
#[test]
fn keeping_pitch_from_a_file_at_another_rate() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "q.wav", 44_100, 3.0, tone(1_000.0));
    let mut e = entry(
        &path,
        TimeRange::new(secs(0, 1), secs(2, 1)),
        secs(0, 1),
        Some(Speed::new(3, 4, false)),
    );
    e.keep_pitch = true;
    let (out, _) = render(vec![e], secs(2, 1), 0, 96_000);
    let f = frequency(&out[8_000..88_000], 48_000.0);
    assert!((f - 1_000.0).abs() < 1.0, "{f:.2} Hz");
}

/// A cold read from the middle of a stretched clip gives the samples playing
/// through it gives.
#[test]
fn a_seek_into_a_stretched_clip_hears_what_playback_hears() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "r.wav", 48_000, 4.0, tone(611.0));
    let make = || {
        let mut e = entry(
            &path,
            TimeRange::new(secs(0, 1), secs(2, 1)),
            secs(0, 1),
            Some(Speed::new(3, 2, false)),
        );
        e.keep_pitch = true;
        vec![e]
    };
    let (whole, _) = render(make(), secs(2, 1), 0, 96_000);
    let mut cold = SequenceAudio::new(r48(), secs(2, 1), make());
    let mut c = Vec::new();
    cold.read(50_000, 1_000, &mut c).unwrap();
    let worst = c
        .chunks(2)
        .map(|p| p[0])
        .zip(&whole[50_000..51_000])
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "a seek lands {worst} away");
}

fn ramp_entry(path: &Path, keep_pitch: bool) -> SoundEntry {
    use ucg_core::Ramp;
    SoundEntry {
        record: TimeRange::new(secs(0, 1), secs(2, 1)),
        path: Some(path.to_path_buf()),
        offset: Some(secs(0, 1)),
        name: "ramp".into(),
        // Real time to double speed over two seconds: three seconds of media.
        map: Some(TimeMap {
            segments: vec![Ramp {
                length: secs(2, 1),
                from: Ratio::new(1, 1),
                to: Ratio::new(2, 1),
            }],
            reversed: false,
        }),
        keep_pitch,
    }
}

/// Varispeed through a ramp: the pitch rises with the speed, from the tone's
/// own frequency at the start to twice it at the end.
#[test]
fn a_varispeed_ramp_rises_in_pitch_with_its_speed() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "ramp.wav", 48_000, 4.0, tone(1_000.0));
    let (out, _) = render(vec![ramp_entry(&path, false)], secs(2, 1), 0, 96_000);
    // Speed over a window centred at t is 1 + t/2 for this ramp.
    let early = frequency(&out[2_400..7_200], 48_000.0); // around t = 0.1 s
    let late = frequency(&out[88_800..93_600], 48_000.0); // around t = 1.9 s
    assert!(
        (early - 1_050.0).abs() < 10.0,
        "near the start: {early:.1} Hz"
    );
    assert!((late - 1_950.0).abs() < 10.0, "near the end: {late:.1} Hz");
}

/// Keeping pitch through a ramp: the tone stays put, and the media still
/// arrives when the integral of the speed says — a change 2.25 s into the file
/// is heard at t where t + t²/4 = 2.25, which is √13 − 2 = 1.6056 s.
#[test]
fn a_ramp_with_pitch_kept_holds_the_tone_and_follows_the_integral() {
    let dir = tempfile::tempdir().unwrap();
    let path = wav(dir.path(), "ramp2.wav", 48_000, 4.0, |t| {
        if t < 2.25 {
            tone(700.0)(t)
        } else {
            tone(1_400.0)(t)
        }
    });
    let (out, _) = render(vec![ramp_entry(&path, true)], secs(2, 1), 0, 96_000);
    let early = frequency(&out[4_800..28_800], 48_000.0);
    let late = frequency(&out[84_000..94_000], 48_000.0);
    assert!(
        (early - 700.0).abs() < 2.0,
        "pitch held early: {early:.1} Hz"
    );
    assert!(
        (late - 1_400.0).abs() < 2.0,
        "pitch held late: {late:.1} Hz"
    );
    // Where the change lands: the last window still at 700 Hz and the first at
    // 1,400 Hz bracket 1.6056 s to within a tenth of a second.
    let window = 2_400;
    let freq_at = |centre: f64| {
        let c = (centre * 48_000.0) as usize;
        frequency(&out[c - window / 2..c + window / 2], 48_000.0)
    };
    assert!(
        (freq_at(1.50) - 700.0).abs() < 20.0,
        "before: {:.1}",
        freq_at(1.50)
    );
    assert!(
        (freq_at(1.72) - 1_400.0).abs() < 40.0,
        "after: {:.1}",
        freq_at(1.72)
    );
}

/// What reading at a speed costs, against real time.
///
/// ```text
/// cargo test -p cine-media --test sequence_audio_speed -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn measure_the_cost_of_reading_at_a_speed() {
    use std::time::Instant;
    let dir = tempfile::tempdir().unwrap();
    let path44 = wav(dir.path(), "m.wav", 44_100, 130.0, tone(440.0));
    let path48 = wav(dir.path(), "n.wav", 48_000, 130.0, tone(440.0));
    let seconds = 60i64;
    for (label, speed) in [
        ("44.1 kHz file, real time (rate conversion)", None),
        ("44.1 kHz file at 2x", Some(Speed::new(2, 1, false))),
        ("44.1 kHz file at 1/2x", Some(Speed::new(1, 2, false))),
        ("44.1 kHz file backwards", Some(Speed::new(1, 1, true))),
        (
            "44.1 kHz file at 2x, pitch kept",
            Some(Speed::new(2, 1, false)),
        ),
        (
            "48 kHz file at 2x, pitch kept",
            Some(Speed::new(2, 1, false)),
        ),
    ] {
        let path = if label.starts_with("48") {
            &path48
        } else {
            &path44
        };
        let offset = if speed.is_some_and(|s| s.reversed) {
            secs(90, 1)
        } else {
            secs(1, 1)
        };
        let mut e = entry(
            path,
            TimeRange::new(secs(0, 1), secs(seconds, 1)),
            offset,
            speed,
        );
        e.keep_pitch = label.ends_with("pitch kept");
        let entries = vec![e];
        let mut seq = SequenceAudio::new(r48(), secs(seconds, 1), entries);
        let mut out = Vec::with_capacity(48_000 * seconds as usize * 2);
        let t = Instant::now();
        let mut at = 0i64;
        while at < 48_000 * seconds {
            out.clear();
            at += seq.read(at, 1024, &mut out).unwrap() as i64;
        }
        let took = t.elapsed();
        println!(
            "MEASURED {label}: {seconds} s rendered in {took:.2?} — {:.0}x real time",
            seconds as f64 / took.as_secs_f64()
        );
    }
}
