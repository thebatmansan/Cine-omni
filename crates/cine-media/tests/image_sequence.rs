//! Image sequences as media, on real files written by the test.
//!
//! What matters here is precision and honesty: an EXR must reach the frame as
//! the floats it holds — including values above 1 and below 0, which is the
//! whole point of scene-linear — a 16-bit PNG must not arrive quantised to 8,
//! and a number with no file must refuse rather than hand back black.

use cine_media::frame::PixelFormat;
use cine_media::sequence::{discover, probe_sequence, SequenceReader};
use exr::prelude as x;
use std::path::Path;
use ucg_core::Rate;

/// An EXR whose pixels carry known values, including out-of-range ones.
fn exr(path: &Path, frame: usize) {
    let (w, h) = (16usize, 8usize);
    x::write_rgba_file(path, w, h, |px, py| {
        let i = (py * w + px) as f32;
        (
            // A scene value far above 1: this is what a highlight looks like.
            6.25 + i + frame as f32 * 100.0,
            // A very small value, where 8 or 16-bit would quantise to nothing.
            1.0e-5 * (i + 1.0),
            // Below zero, as a legal scene-linear sample.
            -0.5 - i / 1000.0,
            1.0,
        )
    })
    .unwrap();
}

fn png16(path: &Path, value: u16) {
    let img = image::ImageBuffer::from_pixel(8, 4, image::Rgba([value, value / 2, value / 3, 65535]));
    image::DynamicImage::ImageRgba16(img).save(path).unwrap();
}

fn f32_samples(frame: &cine_media::VideoFrame) -> Vec<f32> {
    let (bytes, _) = frame.plane(0).unwrap();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_ne_bytes(*b))
        .collect()
}

#[test]
fn an_exr_sequence_is_one_asset_with_its_own_rate_and_length() {
    let dir = tempfile::tempdir().unwrap();
    for n in 1..=5 {
        exr(&dir.path().join(format!("shot_0010.{n:04}.exr")), n);
    }
    let rate = Rate::new(24000, 1001).unwrap();
    let info = probe_sequence(&dir.path().join("shot_0010.0001.exr"), rate).unwrap();
    let video = info.first_video().unwrap().video.unwrap();
    assert_eq!((video.width, video.height), (16, 8));
    assert_eq!(video.frame_count, Some(5));
    assert_eq!(video.frame_rate, Some(rate));
    assert!(!video.variable_frame_rate);
    assert_eq!(info.first_video().unwrap().codec, "exr");

    // Its identity is the run's, and it is stable.
    let again = probe_sequence(&dir.path().join("shot_0010.0003.exr"), rate).unwrap();
    assert_eq!(info.hash, again.hash, "any file of the run identifies the run");
    assert_eq!(info.entity_id(), again.entity_id());

    // A longer run is a different asset.
    exr(&dir.path().join("shot_0010.0006.exr"), 6);
    let longer = probe_sequence(&dir.path().join("shot_0010.0001.exr"), rate).unwrap();
    assert_ne!(info.hash, longer.hash, "a sixth frame is a different run");
}

#[test]
fn exr_pixels_reach_the_frame_as_the_floats_they_are() {
    let dir = tempfile::tempdir().unwrap();
    for n in 1..=3 {
        exr(&dir.path().join(format!("p.{n:04}.exr")), n);
    }
    let mut reader = SequenceReader::open(&dir.path().join("p.0001.exr"), Rate::F24).unwrap();
    assert_eq!(reader.frame_count(), 3);

    for frame_index in 0..3usize {
        let frame = reader.frame_at_index(frame_index).unwrap().unwrap();
        assert_eq!(frame.format, PixelFormat::RgbaF32);
        assert_eq!((frame.width, frame.height), (16, 8));
        assert_eq!(frame.pts.ticks(), frame_index as i64);
        let px = f32_samples(&frame);
        for i in 0..(16 * 8) {
            let (r, g, b) = (px[i * 4], px[i * 4 + 1], px[i * 4 + 2]);
            let expected_r = 6.25 + i as f32 + (frame_index + 1) as f32 * 100.0;
            // Half-float storage: the value is the nearest half, which is what
            // the file holds. It must be that, not a clamped or quantised one.
            assert!(
                (r - expected_r).abs() <= expected_r.abs() * 1e-3,
                "frame {frame_index} pixel {i}: {r} is not {expected_r}"
            );
            assert!(r > 1.0, "a highlight above 1 survived: {r}");
            let expected_g = 1.0e-5 * (i as f32 + 1.0);
            assert!(
                (g - expected_g).abs() <= expected_g * 1e-2,
                "a value far below 8-bit's smallest step survived: {g}, wanted {expected_g}"
            );
            assert!(b < 0.0, "a negative sample survived: {b}");
        }
    }
}

#[test]
fn a_sixteen_bit_still_sequence_is_not_quantised_to_eight() {
    let dir = tempfile::tempdir().unwrap();
    for (n, v) in [(1u32, 513u16), (2, 65535), (3, 1)] {
        png16(&dir.path().join(format!("plate.{n:04}.png")), v);
    }
    let mut reader = SequenceReader::open(&dir.path().join("plate.0001.png"), Rate::F24).unwrap();
    let frame = reader.frame_at_index(0).unwrap().unwrap();
    assert_eq!(frame.format, PixelFormat::Rgba16);
    let (bytes, stride) = frame.plane(0).unwrap();
    assert_eq!(stride, 8 * 4 * 2);
    let first = u16::from_ne_bytes(bytes[0..2].try_into().unwrap());
    assert_eq!(first, 513, "the sample is the file's, not a widened 8-bit one");
    assert!(!first.is_multiple_of(257), "513 is not representable in 8 bits");
}

#[test]
fn a_missing_frame_is_a_hole_that_refuses_rather_than_a_black_picture() {
    let dir = tempfile::tempdir().unwrap();
    for n in [1, 2, 4] {
        exr(&dir.path().join(format!("p.{n:04}.exr")), n);
    }
    let seq = discover(&dir.path().join("p.0001.exr")).unwrap();
    assert_eq!(seq.missing, vec![3]);
    assert_eq!(seq.frames(), 4);

    let mut reader = SequenceReader::open(&dir.path().join("p.0001.exr"), Rate::F24).unwrap();
    assert_eq!(reader.frame_count(), 4);
    assert!(reader.frame_at_index(0).unwrap().is_some());
    let refused = reader.frame_at_index(2).unwrap_err();
    let message = refused.to_string();
    assert!(message.contains("p.0003.exr") && message.contains("missing"), "{message}");
    assert!(reader.frame_at_index(3).unwrap().is_some(), "the run continues past the hole");
    assert!(reader.frame_at_index(4).unwrap().is_none(), "past the end there is nothing");
}
