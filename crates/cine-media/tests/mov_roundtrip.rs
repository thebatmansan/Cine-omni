//! QuickTime movies written by `cine_media::mov`, read back three ways: by this
//! build's picture decoder, by its sound decoder, and by GStreamer's `qtdemux`
//! — an independent implementation of the container — which must report the
//! same tracks, rate and length and hand back the exact sound bytes written.
//!
//! Each picture carries its index (a luma level and a moving bar) and each
//! sound sample its position, so reordering, dropping or drift would be seen.

use cine_media::encode::{H264Settings, YuvFrame};
use cine_media::mov::{ColourTags, MovSettings, MovWriter, SoundFormat};
use cine_media::{MediaReader, VideoReader};
use std::path::Path;
use std::process::Command;
use ucg_core::Rate;

const W: usize = 320;
const H: usize = 180;

fn picture(i: usize) -> YuvFrame {
    let level = 40 + (i * 7 % 160) as u8;
    let mut y = vec![level; W * H];
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

/// The 24-bit integer written at absolute sample `n` of channel `c`.
fn sound_value(n: u64, c: usize) -> i32 {
    let v = ((n * 37) % 1_000_000) as i32 - 500_000;
    if c == 0 {
        v
    } else {
        -v
    }
}

/// The centre column of the bright bar. H.264 is lossy, so an edge may move
/// by a pixel; the centre of a 16-pixel bar does not move by more.
fn bar_centre(frame: &cine_media::VideoFrame) -> f64 {
    let (plane, stride) = &frame.planes[0];
    let row = H / 2;
    let cols: Vec<usize> = (0..W).filter(|&c| plane[row * stride + c] > 200).collect();
    cols.iter().sum::<usize>() as f64 / cols.len() as f64
}

fn write_movie(
    path: &Path,
    rate: Rate,
    frames: usize,
    sound: Option<SoundFormat>,
) -> cine_media::mov::MovReport {
    let settings = MovSettings {
        h264: H264Settings { qp: 18, gop: 12 },
        sound,
        colour: ColourTags::BT709,
        timecode: None,
        first_picture: 0,
    };
    let mut w = MovWriter::create(path, W, H, rate, settings).unwrap();
    let mut written = 0u64;
    for i in 0..frames {
        let due = w.sound_frames_due();
        let channels = sound.map_or(0, |s| usize::from(s.channels));
        let mut samples = Vec::with_capacity(due as usize * channels);
        for k in 0..due {
            for c in 0..channels {
                samples.push(sound_value(written + k, c) as f32 / 8_388_608.0);
            }
        }
        written += due;
        w.write_frame(&picture(i), &samples).unwrap();
    }
    w.finish().unwrap()
}

#[test]
fn a_movie_reads_back_picture_for_picture_and_sample_for_sample() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cut.mov");
    let rate = Rate::new(30000, 1001).unwrap();
    let sound = SoundFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let report = write_movie(&path, rate, 60, Some(sound));
    assert_eq!(report.frames, 60);
    // 60 frames at 30000/1001 is 2.002 s: exactly 96096 samples.
    assert_eq!(report.sound_frames, 96_096);
    assert_eq!(report.sound_clipped, 0);

    let mut video = VideoReader::open_path(&path).expect("our reader opens the movie");
    assert_eq!(video.frame_count(), 60);
    let (first, end) = video.extent().unwrap();
    assert_eq!(first.ticks(), 0);
    assert_eq!(
        end.rescale_checked(rate, ucg_core::Rounding::Nearest),
        (ucg_core::RationalTime::exact(60, rate), true)
    );
    // The colour the movie declares in its container is the colour it is read
    // with: not assumed. Regression: only the stream's SPS was consulted, so
    // this build's own movies read back as "BT.601, assumed" at SD heights.
    let f0 = video.frame_at_index(0).unwrap().unwrap();
    assert_eq!(
        (f0.color.primaries, f0.color.transfer, f0.color.matrix),
        (Some(1), Some(1), Some(1))
    );
    for i in 0..60 {
        let f = video.frame_at_index(i).unwrap().unwrap();
        let expected = ((i * 11) % (W - 16)) as f64 + 7.5;
        let got = bar_centre(&f);
        assert!(
            (got - expected).abs() <= 2.0,
            "picture {i} in order: bar at {got}, expected {expected}"
        );
    }

    let info = cine_media::probe::probe(&path).unwrap();
    let audio = info.first_audio().expect("a sound stream");
    assert_eq!(audio.codec, "pcm_s24le");
    assert!(audio.decodable);
    let a = audio.audio.as_ref().unwrap();
    assert_eq!(
        (a.sample_rate, a.channels, a.bits_per_sample),
        (48_000, 2, Some(24))
    );

    let mut reader = MediaReader::open_path(&path).expect("our reader opens the sound");
    let mut n = 0u64;
    let mut worst = 0i64;
    while let Some(frame) = reader.next_audio_frame().unwrap() {
        assert_eq!(frame.sample_rate, 48_000);
        assert_eq!(frame.channel_count(), 2);
        for i in 0..frame.frame_count() {
            for c in 0..2 {
                // Symphonia scales 24-bit by 2^23; compare as integers.
                let got = (f64::from(frame.channels[c][i]) * 8_388_608.0).round() as i64;
                worst = worst.max((got - i64::from(sound_value(n, c))).abs());
            }
            n += 1;
        }
    }
    assert_eq!(n, 96_096, "every sample is read back");
    assert_eq!(worst, 0, "sound is bit-exact");
}

#[test]
fn gstreamer_reads_the_same_movie() {
    if Command::new("gst-discoverer-1.0")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("SKIP: GStreamer is not installed; the independent container check did not run");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cut.mov");
    let rate = Rate::new(24000, 1001).unwrap();
    let sound = SoundFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let report = write_movie(&path, rate, 48, Some(sound));

    let out = Command::new("gst-discoverer-1.0")
        .arg("-v")
        .arg(&path)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("video/x-h264"), "{text}");
    assert!(
        text.contains("width=(int)320") && text.contains("height=(int)180"),
        "{text}"
    );
    assert!(text.contains("framerate=(fraction)24000/1001"), "{text}");
    assert!(
        text.contains("audio/x-raw") && text.contains("S24LE"),
        "{text}"
    );
    assert!(
        text.contains("rate=(int)48000") && text.contains("channels=(int)2"),
        "{text}"
    );
    // 48 × 1001/24000 s = 2.002 s.
    assert!(text.contains("Duration: 0:00:02.002"), "{text}");

    // The sound bytes qtdemux hands out are the bytes written.
    let raw = dir.path().join("sound.raw");
    let status = Command::new("gst-launch-1.0")
        .args(["-q", "filesrc"])
        .arg(format!("location={}", path.display()))
        .args(["!", "qtdemux", "name=d", "d.audio_0", "!", "filesink"])
        .arg(format!("location={}", raw.display()))
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&raw).unwrap();
    assert_eq!(bytes.len() as u64, report.sound_frames * 6);
    let mut expected = Vec::with_capacity(bytes.len());
    for n in 0..report.sound_frames {
        for c in 0..2 {
            expected.extend_from_slice(&sound_value(n, c).to_le_bytes()[..3]);
        }
    }
    assert!(
        bytes == expected,
        "GStreamer's sound differs from what was written"
    );

    // And it counts the pictures.
    let status = Command::new("gst-launch-1.0")
        .args(["-q", "filesrc"])
        .arg(format!("location={}", path.display()))
        .args(["!", "qtdemux", "name=d", "d.video_0", "!", "multifilesink"])
        .arg(format!("location={}/p%04d.h264", dir.path().display()))
        .status()
        .unwrap();
    assert!(status.success());
    let pictures = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".h264")
        })
        .count();
    assert_eq!(pictures, 48);
}

#[test]
fn a_movie_without_sound_has_no_sound_track() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silent-by-choice.mov");
    write_movie(&path, Rate::F24, 12, None);
    let info = cine_media::probe::probe(&path).unwrap();
    assert!(info.first_audio().is_none(), "no invented sound track");
    assert_eq!(VideoReader::open_path(&path).unwrap().frame_count(), 12);
}

#[test]
fn sound_of_the_wrong_length_for_a_picture_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wrong.mov");
    let sound = SoundFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let settings = MovSettings {
        h264: H264Settings::default(),
        sound: Some(sound),
        colour: ColourTags::UNSPECIFIED,
        timecode: None,
        first_picture: 0,
    };
    let mut w = MovWriter::create(&path, W, H, Rate::F24, settings).unwrap();
    assert_eq!(w.sound_frames_due(), 2000);
    let err = w
        .write_frame(&picture(0), &vec![0.0; 1999 * 2])
        .unwrap_err();
    assert!(err.to_string().contains("2000"), "{err}");
}
