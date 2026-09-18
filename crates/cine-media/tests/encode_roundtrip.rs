//! H.264 written by `cine_media::encode` and read back by this build's own
//! decoder, through the same probe and reader an ingested file takes.
//!
//! Each frame carries its own index in its picture — a flat luma level and a
//! bar whose position moves — so a frame out of order, dropped or repeated
//! would be seen, not just a file that happens to open.

use cine_media::encode::{write_h264_mp4, H264Settings};
use cine_media::VideoReader;
use rusty_h264::YuvFrame;
use ucg_core::Rate;

const W: usize = 320;
const H: usize = 180;

fn frame(i: usize) -> YuvFrame {
    let level = 40 + (i * 7 % 160) as u8;
    let mut y = vec![level; W * H];
    // A bright bar whose column moves with the index.
    let bar = (i * 11) % (W - 16);
    for row in 0..H {
        for col in bar..bar + 16 {
            y[row * W + col] = 235;
        }
    }
    YuvFrame {
        width: W,
        height: H,
        y,
        u: vec![128; (W / 2) * (H / 2)],
        v: vec![128; (W / 2) * (H / 2)],
    }
}

/// Mean luma of a decoded picture outside the bar, and the bar's column.
fn read_back(frame: &cine_media::VideoFrame) -> (f64, usize) {
    let (plane, stride) = &frame.planes[0];
    let (w, h) = (frame.width as usize, frame.height as usize);
    let row = h / 2;
    let line = &plane[row * stride..row * stride + w];
    let bar = (0..w).find(|&c| line[c] > 200).unwrap_or(usize::MAX);
    let mut sum = 0u64;
    let mut n = 0u64;
    for r in 0..h {
        for c in 0..w {
            if c + 4 < bar || c > bar + 20 {
                sum += u64::from(plane[r * stride + c]);
                n += 1;
            }
        }
    }
    (sum as f64 / n as f64, bar)
}

#[test]
fn an_encoded_file_decodes_to_the_frames_that_went_in_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pattern.mp4");
    let rate = Rate::new(24000, 1001).unwrap();
    let count = 60;
    let report = write_h264_mp4(
        &path,
        W,
        H,
        rate,
        H264Settings { qp: 18, gop: 24 },
        (0..count).map(frame),
    )
    .expect("encoded");
    assert_eq!(report.frames, count as u64);
    assert!(report.keyframes >= 3, "a keyframe every 24: {report:?}");

    let mut reader = VideoReader::open_path(&path).expect("this build opens what it wrote");
    assert_eq!(reader.frame_count(), count);
    // Exact time: sixty pictures shown for exactly 60 × 1001/24000 s.
    let (first, end) = reader.extent().expect("an extent");
    let span = end.checked_sub(&first);
    assert_eq!(
        span.rescale_checked(rate, ucg_core::Rounding::Nearest),
        (ucg_core::RationalTime::exact(60, rate), true),
        "frame times are exact at 24000/1001"
    );
    // Every frame, and out of order too: what comes back is the frame asked for.
    for i in [0usize, 1, 23, 24, 25, 47, 59, 30, 5] {
        let decoded = reader.frame_at_index(i).unwrap().expect("a picture");
        let (level, bar) = read_back(&decoded);
        let want_level = f64::from(40 + (i * 7 % 160) as u8);
        assert!(
            (level - want_level).abs() < 3.0,
            "frame {i}: level {level:.1} vs {want_level}"
        );
        let want_bar = (i * 11) % (W - 16);
        assert!(
            bar.abs_diff(want_bar) <= 2,
            "frame {i}: bar at {bar}, wrote {want_bar}"
        );
    }
}

#[test]
fn an_odd_size_is_refused_rather_than_encoded_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_h264_mp4(
        &dir.path().join("odd.mp4"),
        321,
        180,
        Rate::whole(24).unwrap(),
        H264Settings::default(),
        std::iter::empty(),
    )
    .unwrap_err();
    assert!(e.to_string().contains("even"), "{e}");
}
