//! The Phase 2 vertical slice, exercised against **real media on disk**.
//!
//! Nothing here is synthetic. If the asset is absent the test skips loudly
//! rather than passing vacuously, because a media test that quietly passes with
//! no media is worse than no test.
//!
//! Assets used:
//! * a real 10-second H.264 + AAC MP4 (720x1280, 24 fps, 48 kHz stereo)
//! * a real PNG from the system
//! * a real RIFF/WAVE file from the system

use cine_media::identity::{ContentHash, StreamKind};
use cine_media::time::{Rate, Rounding, Timestamp};
use cine_media::{probe, Error, MediaReader};
use std::path::{Path, PathBuf};

fn asset(candidates: &[&str]) -> Option<PathBuf> {
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

fn mp4() -> Option<PathBuf> {
    asset(&["/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4"])
}

fn png() -> Option<PathBuf> {
    asset(&[
        "/usr/share/pixmaps/debian-logo.png",
        "/usr/share/pixmaps/ubuntu-logo-text.png",
    ])
}

fn wav() -> Option<PathBuf> {
    asset(&[
        "/usr/share/sounds/sound-icons/percussion-12.wav",
        "/usr/share/sounds/sound-icons/cembalo-11.wav",
    ])
}

/// Skip with an explicit message rather than passing silently.
macro_rules! need {
    ($opt:expr, $what:expr) => {
        match $opt {
            Some(p) => p,
            None => {
                eprintln!("SKIP: no {} available on this machine", $what);
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Identify
// ---------------------------------------------------------------------------

#[test]
fn probes_a_real_mp4_and_reports_streams_truthfully() {
    let path = need!(mp4(), "real mp4");
    let info = probe(&path).expect("probe real mp4");

    assert_eq!(info.container, cine_media::ContainerKind::IsoBmff);
    assert!(info.hash.size() > 1_000_000, "real file, not a stub");

    let video = info.first_video().expect("mp4 has a video track");
    assert_eq!(video.codec, "h264");
    let v = video.video.expect("video stream info");
    assert_eq!((v.width, v.height), (720, 1280));
    assert_eq!(v.frame_count, Some(240), "10s at 24fps");
    assert_eq!(v.frame_rate, Some(Rate::F24));
    // The stream keeps its own time base; it is not normalised on ingest.
    assert_eq!(video.time_base, Rate::whole(12_288).unwrap());

    let audio = info.first_audio().expect("mp4 has an audio track");
    assert_eq!(audio.codec, "aac");
    let a = audio.audio.expect("audio stream info");
    assert_eq!(a.sample_rate, 48_000);
    assert_eq!(a.channels, 2);

    // The capability claim must be honest in both directions. Both are now
    // decodable, and each is proven by a decode test below rather than by the
    // flag asserting itself.
    assert!(audio.decodable, "AAC is decodable by this build");
    assert!(
        video.decodable,
        "High-profile H.264 4:2:0 8-bit is decodable by this build"
    );
    assert!(info.fully_decodable());
}

#[test]
fn media_identity_is_content_derived_and_stable() {
    let path = need!(mp4(), "real mp4");
    let a = probe(&path).unwrap();
    let b = probe(&path).unwrap();
    assert_eq!(
        a.entity_id(),
        b.entity_id(),
        "probing twice yields one identity"
    );
    assert_eq!(a.hash, b.hash);

    // Stream ids are derived from the file identity, so they are stable too.
    assert_eq!(a.stream_entity_id(1), b.stream_entity_id(1));
    assert_ne!(a.stream_entity_id(1), a.stream_entity_id(2));

    // A full hash of a multi-megabyte file must agree on size with the fast one.
    let full = ContentHash::full(&path).unwrap();
    assert_eq!(full.size(), a.hash.size());
    assert!(full.is_full() && !a.hash.is_full());
}

// ---------------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------------

#[test]
fn decodes_real_aac_audio_from_the_mp4() {
    let path = need!(mp4(), "real mp4");
    let mut reader = MediaReader::open_path(&path).expect("open");

    let frames = reader.decode_audio_frames(50).expect("decode audio");
    assert!(!frames.is_empty(), "real AAC must decode to real samples");

    let first = &frames[0];
    assert_eq!(first.sample_rate, 48_000);
    assert_eq!(first.channel_count(), 2, "stereo");
    assert!(first.frame_count() > 0);

    // Real audio has real signal. Silence would mean we decoded nothing.
    let peak = frames.iter().map(|f| f.peak()).fold(0.0f32, f32::max);
    assert!(
        peak > 0.0,
        "decoded audio is silent, which means decode failed"
    );
    assert!(
        peak <= 1.5,
        "samples must be normalised floats, got peak {peak}"
    );

    // Timestamps must advance monotonically in the stream's own base.
    for pair in frames.windows(2) {
        assert!(pair[1].pts >= pair[0].pts, "PTS must not go backwards");
    }

    // Interleaving must preserve the sample count.
    let il = first.to_interleaved();
    assert_eq!(il.len(), first.frame_count() * first.channel_count());
}

#[test]
fn an_undecodable_codec_is_refused_rather_than_substituted() {
    let path = need!(mp4(), "real mp4");
    let mut info = probe(&path).expect("probe");

    // Simulate a codec this build genuinely cannot handle, by marking the
    // stream the way `probe` marks e.g. HEVC or ProRes. The guard must refuse
    // with an actionable error naming the codec and the real alternatives —
    // never a black frame, and never a silent substitution.
    for s in &mut info.streams {
        if s.kind == StreamKind::Video {
            s.codec = "prores".into();
            s.decodable = false;
        }
    }
    match cine_media::VideoReader::open(info.clone()) {
        Err(Error::NoDecoder { codec, available }) => {
            assert_eq!(codec, "prores");
            assert!(
                available.contains("h264"),
                "the error must name real capability"
            );
        }
        Err(e) => panic!("expected NoDecoder, got {e}"),
        Ok(_) => panic!("opening an undecodable stream must fail loudly"),
    }

    let reader = MediaReader::open(info).expect("open");
    assert!(matches!(
        reader.require_decodable(StreamKind::Video),
        Err(Error::NoDecoder { .. })
    ));
}

// ---------------------------------------------------------------------------
// Video decode: real H.264 pictures out of the real file
// ---------------------------------------------------------------------------

#[test]
fn decodes_real_h264_video_to_real_pictures() {
    let path = need!(mp4(), "real mp4");
    let mut reader = cine_media::VideoReader::open_path(&path).expect("open video");
    assert_eq!(reader.frame_count(), 240, "10s at 24fps");
    assert!(
        reader.keyframe_count() >= 1,
        "a seekable stream needs a sync sample"
    );

    let frame = reader
        .next_frame()
        .expect("decode")
        .expect("first frame exists");
    assert_eq!((frame.width, frame.height), (720, 1280));
    assert_eq!(frame.format, cine_media::PixelFormat::Yuv420P8);

    // Plane geometry must match the format, or every consumer downstream is
    // reading past the end of a plane.
    assert_eq!(frame.planes.len(), 3);
    let (y, y_stride) = frame.plane(0).unwrap();
    let (u, c_stride) = frame.plane(1).unwrap();
    let (v, _) = frame.plane(2).unwrap();
    assert_eq!(y_stride, 720);
    assert_eq!(c_stride, 360);
    assert_eq!(y.len(), 720 * 1280, "luma plane is full resolution");
    assert_eq!(u.len(), 360 * 640, "chroma is half in both axes for 4:2:0");
    assert_eq!(v.len(), u.len());

    // Real picture content, not a blank buffer.
    let mean = y.iter().map(|&b| u64::from(b)).sum::<u64>() / y.len() as u64;
    assert!(
        (8..248).contains(&mean),
        "mean luma {mean} looks like a blank frame"
    );
    let distinct = {
        let mut seen = [false; 256];
        for &b in y {
            seen[b as usize] = true;
        }
        seen.iter().filter(|&&s| s).count()
    };
    assert!(
        distinct > 32,
        "only {distinct} distinct luma values: that is not a photograph"
    );

    // Colour is absent rather than assumed. H.264 VUI parsing is a named gap;
    // defaulting to Rec.709 here would be a guess that survives into the grade.
    assert!(!frame.color.is_complete());
}

#[test]
fn decodes_every_frame_in_presentation_order() {
    let path = need!(mp4(), "real mp4");
    let mut reader = cine_media::VideoReader::open_path(&path).expect("open video");
    assert!(
        reader.has_reordering(),
        "this stream has B-frames, so the orders differ"
    );

    let mut count = 0usize;
    let mut last: Option<Timestamp> = None;
    let mut means = Vec::new();
    while let Some(f) = reader.next_frame().expect("decode") {
        if let Some(prev) = last {
            assert!(
                f.pts > prev,
                "presentation order must be strictly increasing"
            );
        }
        last = Some(f.pts);
        let (y, _) = f.plane(0).unwrap();
        means.push(y.iter().map(|&b| u64::from(b)).sum::<u64>() / y.len() as u64);
        count += 1;
        assert!(count <= 240, "runaway decode");
    }
    assert_eq!(
        count, 240,
        "every coded picture must be delivered exactly once"
    );

    // The content must actually change over time; a decoder stuck on one
    // reference frame would produce a flat series.
    let mut distinct = means.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() > 8,
        "only {} distinct frame means across 240 frames",
        distinct.len()
    );
}

#[test]
fn video_seek_is_frame_accurate_and_deterministic() {
    let path = need!(mp4(), "real mp4");
    let mut reader = cine_media::VideoReader::open_path(&path).expect("open video");

    // Frame 100 by sequential read, for a reference to compare seeks against.
    let want_pts = reader.pts_at(100).expect("frame 100 exists");
    let mut sequential = None;
    for _ in 0..101 {
        sequential = reader.next_frame().expect("decode");
    }
    let sequential = sequential.expect("frame 100");
    assert_eq!(sequential.pts, want_pts);

    // The same frame reached by seeking must be bit-identical, not merely
    // nearby. Anything less and a conform would produce a different picture
    // than playback did.
    reader.rewind().unwrap();
    let sought = reader.frame_at(want_pts).expect("seek").expect("frame 100");
    assert_eq!(
        sought.pts, want_pts,
        "seek must land exactly, not within a tolerance"
    );
    assert_eq!(
        sought.plane(0).unwrap().0,
        sequential.plane(0).unwrap().0,
        "seeking to a frame must produce the same pixels as playing to it"
    );

    // Repeating the seek is deterministic.
    let again = reader.frame_at(want_pts).unwrap().unwrap();
    assert_eq!(again.plane(0).unwrap().0, sought.plane(0).unwrap().0);

    // Backwards seek across a GOP boundary must reset cleanly.
    let earlier = reader.pts_at(3).expect("frame 3");
    let back = reader
        .frame_at(earlier)
        .expect("seek back")
        .expect("frame 3");
    assert_eq!(back.pts, earlier);
    assert!(back.pts < sought.pts);

    // A seek past the end is not a crash and not a fabricated frame.
    let past_end = Timestamp::exact(i64::from(u32::MAX), reader.time_base());
    assert!(
        reader.frame_at(past_end).unwrap().is_some(),
        "clamps to the last frame"
    );
    let before_start = Timestamp::exact(-1, reader.time_base());
    assert!(
        reader.frame_at(before_start).unwrap().is_none(),
        "nothing exists before zero"
    );
}

#[test]
fn a_decoded_video_frame_survives_the_cache_unchanged() {
    use cine_media::{Frame, FrameCache, FrameKey};
    use std::sync::Arc;

    let path = need!(mp4(), "real mp4");
    let mut reader = cine_media::VideoReader::open_path(&path).expect("open video");
    let hash = reader.info().hash.clone();
    let frame = reader.next_frame().unwrap().unwrap();
    let original = frame.plane(0).unwrap().0.to_vec();

    // A 720x1280 4:2:0 frame is 1.32 MiB; the budget must be counted in bytes,
    // not entries, or a handful of frames would exhaust memory.
    let expected = 720 * 1280 + 2 * 360 * 640;
    assert_eq!(frame.byte_size(), expected);

    let cache = FrameCache::new(8 * 1024 * 1024);
    let key = FrameKey::new(&hash, 1, frame.pts);
    cache.insert(key.clone(), Arc::new(Frame::Video(frame)));

    let got = cache.get(&key).expect("cached frame");
    match &*got {
        Frame::Video(v) => {
            assert_eq!(
                v.plane(0).unwrap().0,
                original.as_slice(),
                "cache must not alter pixels"
            );
            assert_eq!(v.byte_size(), expected);
        }
        other => panic!("expected a video frame, got {other:?}"),
    }
    assert!(cache.bytes() <= cache.capacity_bytes());
}

#[test]
fn decodes_a_real_wav_file() {
    let path = need!(wav(), "real wav");
    let info = probe(&path).expect("probe wav");
    assert_eq!(info.container, cine_media::ContainerKind::Wave);
    let a = info.first_audio().expect("wav has audio");
    assert!(a.decodable, "PCM must be decodable");

    let mut reader = MediaReader::open(info).expect("open wav");
    let frames = reader.decode_audio_frames(20).expect("decode wav");
    assert!(!frames.is_empty());
    assert!(
        frames.iter().any(|f| f.peak() > 0.0),
        "real audio must have signal"
    );
}

#[test]
fn decodes_a_real_image_as_a_video_frame() {
    let path = need!(png(), "real png");
    let info = probe(&path).expect("probe png");
    assert_eq!(info.container, cine_media::ContainerKind::Image);

    let frame = cine_media::reader::decode_image(&path).expect("decode png");
    assert!(frame.width > 0 && frame.height > 0);
    assert_eq!(frame.format, cine_media::PixelFormat::Rgba8);
    assert_eq!(frame.planes.len(), 1);
    let (data, stride) = frame.plane(0).unwrap();
    assert_eq!(stride, frame.width as usize * 4);
    assert_eq!(data.len(), stride * frame.height as usize);
    // A still carries no colour code points; claiming Rec.709 would be a guess.
    assert!(!frame.color.is_complete());
}

// ---------------------------------------------------------------------------
// Seek
// ---------------------------------------------------------------------------

#[test]
fn seeks_frame_accurately_in_real_media() {
    let path = need!(mp4(), "real mp4");
    let mut reader = MediaReader::open_path(&path).expect("open");
    let base = reader.audio_time_base();

    // Five seconds in, expressed exactly in the audio stream's own base.
    let target = Timestamp::exact(5 * 48_000, base);
    let frame = reader
        .decode_audio_at(target)
        .expect("seek and decode")
        .expect("media is 10s long, so 5s exists");

    // The returned frame must *contain* the target instant.
    let start = frame.pts.rescale_checked(base, Rounding::Nearest).0.ticks();
    let end = start + frame.frame_count() as i64;
    let want = target.rescale_checked(base, Rounding::Nearest).0.ticks();
    assert!(
        start <= want && want < end,
        "frame [{start}, {end}) must contain the requested sample {want}"
    );

    // Seeking is repeatable: the same request lands on the same frame.
    let again = reader.decode_audio_at(target).unwrap().unwrap();
    assert_eq!(again.pts, frame.pts, "seek must be deterministic");

    // Seeking backwards works too.
    let earlier = Timestamp::exact(48_000, base);
    let f = reader.decode_audio_at(earlier).unwrap().unwrap();
    assert!(f.pts < frame.pts, "seeking backwards must actually go back");
}

#[test]
fn decoding_the_whole_file_yields_the_expected_duration() {
    let path = need!(mp4(), "real mp4");
    let mut reader = MediaReader::open_path(&path).expect("open");

    let mut total_samples = 0usize;
    let mut frames = 0usize;
    while let Some(f) = reader.next_audio_frame().expect("decode") {
        total_samples += f.frame_count();
        frames += 1;
        assert!(frames < 100_000, "runaway decode loop");
    }
    assert!(frames > 0, "decoded nothing from a real file");

    // The container says ~10.03 s of audio at 48 kHz.
    let seconds = total_samples as f64 / 48_000.0;
    assert!(
        (9.5..10.5).contains(&seconds),
        "decoded {seconds:.3}s of audio, expected about 10s"
    );
}

// ---------------------------------------------------------------------------
// Failure handling
// ---------------------------------------------------------------------------

#[test]
fn missing_and_corrupt_media_fail_cleanly() {
    // Missing.
    let err = probe(Path::new("/nonexistent/A001C001.mov"));
    assert!(matches!(err, Err(Error::NotFound(_))));

    // Present but not media.
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("not-really.mp4");
    std::fs::write(&junk, b"this is definitely not an ISO base media file").unwrap();
    match probe(&junk) {
        Err(Error::Container(msg)) => assert!(msg.contains("not-really.mp4")),
        other => panic!("corrupt media must fail with a diagnosable error, got {other:?}"),
    }

    // An extension we do not claim to handle.
    let odd = dir.path().join("camera.r3d");
    std::fs::write(&odd, b"RED raw").unwrap();
    assert!(matches!(probe(&odd), Err(Error::UnknownContainer(_))));

    // Media that vanishes between probe and open reports as offline.
    let temp = dir.path().join("vanishing.wav");
    if let Some(src) = wav() {
        std::fs::copy(&src, &temp).unwrap();
        let info = probe(&temp).unwrap();
        std::fs::remove_file(&temp).unwrap();
        assert!(matches!(MediaReader::open(info), Err(Error::Offline(_))));
    }
}
