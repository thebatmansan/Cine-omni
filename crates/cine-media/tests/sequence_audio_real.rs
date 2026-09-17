//! The sound of a cut, from real files: each sample is the file's sample the
//! edit list names, gaps are silence, and overlapping clips sum.

use cine_media::av::AudioSource;
use cine_media::{MediaReader, SequenceAudio, SoundEntry, VideoReader};
use std::path::PathBuf;
use ucg_core::{Rate, RationalTime, Rounding, TimeRange};

fn asset() -> Option<PathBuf> {
    let p = PathBuf::from("/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4");
    p.exists().then_some(p)
}

fn f(n: i64) -> RationalTime {
    RationalTime::exact(n, Rate::new(24, 1).unwrap())
}

fn r48() -> Rate {
    Rate::new(48_000, 1).unwrap()
}

/// Every sample of the file's sound, from its start, at 48 kHz stereo.
fn decode_all(path: &std::path::Path) -> (i64, Vec<f32>) {
    let mut reader = MediaReader::open_path(path).unwrap();
    let mut start = None;
    let mut out = Vec::new();
    while let Some(frame) = reader.next_audio_frame().unwrap() {
        start.get_or_insert(
            frame
                .pts
                .rescale_checked(r48(), Rounding::Nearest)
                .0
                .ticks(),
        );
        for i in 0..frame.frame_count() {
            out.push(frame.channels[0][i]);
            out.push(frame.channels.get(1).unwrap_or(&frame.channels[0])[i]);
        }
    }
    (start.unwrap_or(0), out)
}

#[test]
fn a_clips_sound_is_the_files_sound_at_the_right_place() {
    let Some(path) = asset() else {
        eprintln!("SKIP: no real media");
        return;
    };
    let (file_start, all) = decode_all(&path);
    let anchor = VideoReader::open_path(&path)
        .unwrap()
        .extent()
        .unwrap()
        .0
        .rescale_checked(r48(), Rounding::Floor)
        .0
        .ticks();
    // Picture 12 of the file (0.5 s in) at record 1 s, for 1 s.
    let entries = vec![SoundEntry {
        record: TimeRange::new(f(24), f(24)),
        path: Some(path.clone()),
        offset: Some(f(12)),
        name: "A".into(),
        map: None,
        keep_pitch: false,
    }];
    let mut seq = SequenceAudio::new(r48(), f(72), entries);
    let mut out = Vec::new();
    // Read in device-sized pieces, as the scheduler does.
    let mut at = 0i64;
    while at < 48_000 * 3 {
        let n = seq.read(at, 1024, &mut out).unwrap();
        if n == 0 {
            break;
        }
        at += n as i64;
    }
    assert_eq!(
        out.len(),
        48_000 * 3 * 2,
        "three seconds of sound, gaps included"
    );
    assert!(
        out[..48_000 * 2].iter().all(|v| *v == 0.0),
        "silence before the clip"
    );
    assert!(
        out[48_000 * 4..].iter().all(|v| *v == 0.0),
        "silence after it"
    );
    // The first sample of the clip is the file's sample at anchor + 0.5 s.
    let file_index = (anchor + 24_000 - file_start) as usize;
    let got = &out[48_000 * 2..48_000 * 2 + 4_800];
    let want = &all[file_index * 2..file_index * 2 + 4_800];
    let max_diff = got
        .iter()
        .zip(want)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    let energy: f32 = want.iter().map(|v| v * v).sum();
    println!("MEASURED clip sound vs direct decode: max difference {max_diff}, energy {energy}");
    assert!(
        energy > 0.0,
        "the compared stretch is not silence, so the comparison means something"
    );
    assert!(max_diff < 1e-6, "sample-accurate: {max_diff}");
    assert!(seq.refusals().is_empty());
}

#[test]
fn overlapping_clips_sum_and_a_seek_lands_on_the_same_samples() {
    let Some(path) = asset() else {
        eprintln!("SKIP: no real media");
        return;
    };
    let entry = |record: i64| SoundEntry {
        record: TimeRange::new(f(record), f(24)),
        path: Some(path.clone()),
        offset: Some(f(0)),
        name: "A".into(),
        map: None,
        keep_pitch: false,
    };
    let mut one = SequenceAudio::new(r48(), f(48), vec![entry(0)]);
    let mut two = SequenceAudio::new(r48(), f(48), vec![entry(0), entry(0)]);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    one.read(0, 9_600, &mut a).unwrap();
    two.read(0, 9_600, &mut b).unwrap();
    assert!(
        a.iter().zip(&b).all(|(x, y)| (2.0 * x - y).abs() < 1e-6),
        "two copies sum to double"
    );

    // Reading 0.7 s directly after a jump equals reading it continuously.
    let mut cont = Vec::new();
    let mut seq = SequenceAudio::new(r48(), f(48), vec![entry(0)]);
    seq.read(0, 33_600, &mut cont).unwrap();
    let mut jumped = Vec::new();
    let mut seq2 = SequenceAudio::new(r48(), f(48), vec![entry(0)]);
    seq2.read(33_600 - 4_800, 4_800, &mut jumped).unwrap();
    let tail = &cont[(33_600 - 4_800) * 2..];
    let diff = tail
        .iter()
        .zip(&jumped)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    println!("MEASURED seek vs continuous decode: max difference {diff}");
    assert!(diff < 1e-3, "a seek lands on the same samples ({diff})");
}
