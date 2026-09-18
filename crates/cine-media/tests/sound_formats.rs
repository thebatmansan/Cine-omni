//! Every integer and float PCM layout reads back as the samples written.
//!
//! Regression: 24-bit sound — the ordinary format of production audio — and
//! 8-bit, 32-bit float-64 and unsigned layouts used to decode to frames with no
//! channels, which every consumer heard as silence.

use cine_media::MediaReader;
use std::path::{Path, PathBuf};

/// A WAV of `bits` per sample (integer, or IEEE float when `float`), two
/// channels, holding `values` (already integer codes, or floats as bits).
fn wav(dir: &Path, name: &str, bits: u16, float: bool, frames: &[(i64, i64)]) -> PathBuf {
    let bytes_per = usize::from(bits / 8);
    let mut data = Vec::new();
    for &(l, r) in frames {
        for v in [l, r] {
            data.extend_from_slice(&v.to_le_bytes()[..bytes_per]);
        }
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&(48_000 * 2 * bytes_per as u32).to_le_bytes());
    b.extend_from_slice(&(2 * bytes_per as u16).to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    let path = dir.join(name);
    std::fs::write(&path, b).unwrap();
    path
}

fn read_all(path: &Path) -> (Vec<f32>, Vec<f32>) {
    let mut r = MediaReader::open_path(path).unwrap();
    let (mut l, mut rr) = (Vec::new(), Vec::new());
    while let Some(f) = r.next_audio_frame().unwrap() {
        assert_eq!(f.channel_count(), 2, "a decoded frame has its channels");
        l.extend_from_slice(&f.channels[0]);
        rr.extend_from_slice(&f.channels[1]);
    }
    (l, rr)
}

#[test]
fn twenty_four_bit_sound_is_heard_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let codes: Vec<(i64, i64)> = (0..4800i64)
        .map(|i| ((i * 1747) % 16_777_216 - 8_388_608, -((i * 911) % 8_388_608)))
        .collect();
    let path = wav(dir.path(), "s24.wav", 24, false, &codes);
    let (l, r) = read_all(&path);
    assert_eq!(l.len(), codes.len());
    for (i, &(cl, cr)) in codes.iter().enumerate() {
        assert_eq!((f64::from(l[i]) * 8_388_608.0).round() as i64, cl, "left {i}");
        assert_eq!((f64::from(r[i]) * 8_388_608.0).round() as i64, cr, "right {i}");
    }
    // Full-scale negative is exactly -1.
    assert_eq!(l[0], -1.0);
}

#[test]
fn eight_bit_and_thirty_two_bit_integer_sound_are_heard() {
    let dir = tempfile::tempdir().unwrap();
    // WAV 8-bit is unsigned: 128 is silence.
    let u8s: Vec<(i64, i64)> = (0..256).map(|i| (i, 255 - i)).collect();
    let (l, _) = read_all(&wav(dir.path(), "u8.wav", 8, false, &u8s));
    assert_eq!(l.len(), 256);
    assert_eq!(l[128], 0.0);
    assert_eq!(l[0], -1.0);

    let s32: Vec<(i64, i64)> = (0..1000).map(|i| (i * 2_000_000 - 1_000_000_000, 7)).collect();
    let (l, _) = read_all(&wav(dir.path(), "s32.wav", 32, false, &s32));
    assert_eq!(l.len(), 1000);
    assert!((f64::from(l[500]) - 0.0).abs() < 1e-6);
}

#[test]
fn float_sound_is_heard_at_both_widths() {
    let dir = tempfile::tempdir().unwrap();
    let wide: Vec<(i64, i64)> = (0..1000)
        .map(|i| {
            let v = (f64::from(i) / 1000.0) - 0.5;
            (v.to_bits() as i64, (-v).to_bits() as i64)
        })
        .collect();
    let (l, r) = read_all(&wav(dir.path(), "f64.wav", 64, true, &wide));
    assert_eq!(l.len(), 1000);
    assert!((l[750] - 0.25).abs() < 1e-6 && (r[750] + 0.25).abs() < 1e-6);

    let narrow: Vec<(i64, i64)> = (0..1000)
        .map(|i| {
            let v = (i as f32 / 1000.0) - 0.5;
            (i64::from(v.to_bits()), i64::from((-v).to_bits()))
        })
        .collect();
    let (l, r) = read_all(&wav(dir.path(), "f32.wav", 32, true, &narrow));
    assert_eq!(l.len(), 1000);
    // 32-bit float is the samples themselves: exact, not merely close.
    assert_eq!(l[750], 0.25);
    assert_eq!(r[750], -0.25);
}
