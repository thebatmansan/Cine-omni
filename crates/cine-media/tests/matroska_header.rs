//! What a Matroska file says is in it.
//!
//! The streaming demuxer this build uses for Matroska is an audio library: it
//! surfaces every track but describes only the ones with sound. Reading its
//! answer directly turned a picture track into a phantom 48 kHz stereo stream,
//! invented out of two defaults. These tests hold the probe to the file.
//!
//! The media is written by the tests, byte by byte, from the Matroska
//! specification — there is no Matroska muxer in this build. That is also what
//! makes the cross-check worth something: the *audio* side of the same bytes
//! is parsed independently by the demuxer, which knows nothing about this
//! reader, and the two have to agree.

use cine_media::ebml::{self, TrackType};
use cine_media::{probe, StreamKind};
use std::path::{Path, PathBuf};

// --- writing EBML, from the specification ---------------------------------

/// An element id, written with the length marker it already carries.
fn id(v: u64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(7);
    bytes[first..].to_vec()
}

/// A size, as a variable-length integer of `len` bytes.
fn size(v: u64, len: usize) -> Vec<u8> {
    let mut out = v.to_be_bytes()[8 - len..].to_vec();
    out[0] |= 0x80u8 >> (len - 1);
    out
}

fn element(element_id: u64, body: &[u8]) -> Vec<u8> {
    let mut out = id(element_id);
    out.extend_from_slice(&size(body.len() as u64, 4));
    out.extend_from_slice(body);
    out
}

fn uint(element_id: u64, v: u64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(7);
    element(element_id, &bytes[first..])
}

fn float(element_id: u64, v: f64) -> Vec<u8> {
    element(element_id, &v.to_be_bytes())
}

fn text(element_id: u64, v: &str) -> Vec<u8> {
    element(element_id, v.as_bytes())
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

/// A Matroska file with the tracks described, and no clusters.
///
/// A file with no frames in it is still a file that says what its tracks are,
/// which is exactly what a probe reads.
fn write_mkv(path: &Path, tracks: &[Vec<u8>]) {
    // EBML header: version fields and the doc type, which is what identifies
    // the file as Matroska rather than some other EBML document.
    let ebml_header = element(
        0x1A45_DFA3,
        &cat(&[
            uint(0x4286, 1),          // EBMLVersion
            uint(0x42F7, 1),          // EBMLReadVersion
            uint(0x42F2, 4),          // EBMLMaxIDLength
            uint(0x42F3, 8),          // EBMLMaxSizeLength
            text(0x4282, "matroska"), // DocType
            uint(0x4287, 4),          // DocTypeVersion
            uint(0x4285, 2),          // DocTypeReadVersion
        ]),
    );
    let info = element(
        0x1549_A966,
        &cat(&[
            uint(0x2AD7B1, 1_000_000), // TimestampScale: one millisecond
            float(0x4489, 2_000.0),    // Duration, in ticks
            text(0x4D80, "CINE-OMNI test"),
        ]),
    );
    let tracks_element = element(0x1654_AE6B, &tracks.concat());
    let segment = element(0x1853_8067, &cat(&[info, tracks_element]));
    std::fs::write(path, cat(&[ebml_header, segment])).unwrap();
}

fn video_track(number: u64, codec: &str, w: u64, h: u64, frame_ns: Option<u64>) -> Vec<u8> {
    let mut body = cat(&[
        uint(0xD7, number), // TrackNumber
        uint(0x73C5, number),
        uint(0x83, 1), // TrackType: video
        text(0x86, codec),
        element(0xE0, &cat(&[uint(0xB0, w), uint(0xBA, h)])), // Video
    ]);
    if let Some(ns) = frame_ns {
        body.extend_from_slice(&uint(0x23E383, ns)); // DefaultDuration
    }
    element(0xAE, &body)
}

fn audio_track(number: u64, codec: &str, rate: f64, channels: u64, bits: Option<u64>) -> Vec<u8> {
    let mut audio = cat(&[float(0xB5, rate), uint(0x9F, channels)]);
    if let Some(b) = bits {
        audio.extend_from_slice(&uint(0x6264, b));
    }
    element(
        0xAE,
        &cat(&[
            uint(0xD7, number),
            uint(0x73C5, number),
            uint(0x83, 2), // TrackType: audio
            text(0x86, codec),
            element(0xE1, &audio),
        ]),
    )
}

// --- the tests ------------------------------------------------------------

#[test]
fn a_picture_track_is_described_as_picture_and_not_invented_as_sound() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("take.mkv");
    write_mkv(
        &path,
        &[
            // 25 fps: a frame lasts 40 000 000 nanoseconds.
            video_track(1, "V_MPEG4/ISO/AVC", 1920, 1080, Some(40_000_000)),
            audio_track(2, "A_PCM/INT/LIT", 48_000.0, 2, Some(16)),
        ],
    );

    let header = ebml::read_header(&path).expect("a track list");
    assert_eq!(header.timestamp_scale, 1_000_000);
    assert_eq!(header.tracks.len(), 2);
    assert_eq!(header.tracks[0].kind, TrackType::Video);
    assert_eq!(header.tracks[1].kind, TrackType::Audio);

    let info = probe::probe(&path).unwrap();
    assert_eq!(info.streams.len(), 2, "{:?}", info.streams);
    let picture = info.first_video().expect("a picture stream");
    assert_eq!(picture.kind, StreamKind::Video, "not sound: {picture:?}");
    assert_eq!(picture.codec, "h264");
    let v = picture.video.expect("picture description");
    assert_eq!((v.width, v.height), (1920, 1080));
    // The rate is the reciprocal of the frame duration, exactly.
    assert_eq!(v.frame_rate.map(|r| (r.num(), r.den())), Some((25, 1)));
    assert!(!v.variable_frame_rate);
    assert!(
        picture.audio.is_none(),
        "a picture track has no sound in it: {picture:?}"
    );
    assert!(
        !picture.decodable,
        "and this build has no Matroska picture decoder, which it says"
    );

    let sound = info.first_audio().expect("a sound stream");
    assert_eq!(sound.codec, "pcm_s16le");
    let a = sound.audio.expect("sound description");
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    assert_eq!(a.bits_per_sample, Some(16));
    assert!(sound.video.is_none());
}

#[test]
fn the_sound_this_reader_finds_is_the_sound_the_demuxer_finds() {
    // The one independent check available: the streaming demuxer parses the
    // same bytes with its own Matroska implementation, which knows nothing
    // about this reader. It describes sound and only sound, so that is where
    // the two can be held against each other.
    let dir = tempfile::tempdir().unwrap();
    for (rate, channels, bits, codec) in [
        (48_000.0, 2u64, Some(16u64), "A_PCM/INT/LIT"),
        (44_100.0, 1, Some(24), "A_PCM/INT/LIT"),
        (96_000.0, 6, Some(32), "A_PCM/FLOAT/IEEE"),
    ] {
        let path = dir.path().join(format!("{rate}-{channels}.mkv"));
        write_mkv(
            &path,
            &[
                video_track(1, "V_VP9", 640, 360, Some(41_708_333)),
                audio_track(2, codec, rate, channels, bits),
            ],
        );
        let ours = ebml::read_header(&path).expect("a track list");
        let theirs = symphonia_tracks(&path);
        let sound = ours
            .tracks
            .iter()
            .find(|t| t.kind == TrackType::Audio)
            .expect("sound");
        let (their_rate, their_channels) = theirs
            .iter()
            .find_map(|(n, r, c)| (*n == sound.number).then_some((*r, *c)))
            .expect("the demuxer found the same track");
        assert_eq!(
            sound.sample_rate.map(|r| r as u32),
            their_rate,
            "{path:?}: the two readers disagree about the sample rate"
        );
        assert_eq!(
            sound.channels.map(u32::from),
            their_channels,
            "{path:?}: the two readers disagree about the channel count"
        );

        // And the picture track, which the demuxer cannot describe at all.
        let picture = ours
            .tracks
            .iter()
            .find(|t| t.kind == TrackType::Video)
            .expect("picture");
        assert_eq!((picture.width, picture.height), (Some(640), Some(360)));
        assert_eq!(
            probe::probe(&path).unwrap().first_video().unwrap().codec,
            "vp9"
        );
    }
}

/// Track number, sample rate and channel count, as the streaming demuxer sees
/// them. Deliberately reads nothing from [`ebml`].
fn symphonia_tracks(path: &Path) -> Vec<(u64, Option<u32>, Option<u32>)> {
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;
    let file = std::fs::File::open(path).unwrap();
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mkv");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .expect("the demuxer reads this file");
    probed
        .format
        .tracks()
        .iter()
        .map(|t| {
            (
                u64::from(t.id),
                t.codec_params.sample_rate,
                // The demuxer records a channel *layout* rather than a channel
                // set for Matroska, so both are asked before concluding it
                // said nothing.
                t.codec_params
                    .channels
                    .or_else(|| t.codec_params.channel_layout.map(|l| l.into_channels()))
                    .map(|c| c.count() as u32),
            )
        })
        .collect()
}

#[test]
fn a_file_that_is_not_matroska_is_refused_rather_than_guessed_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-really.mkv");
    std::fs::write(&path, b"this is not an EBML document at all, not one bit").unwrap();
    assert_eq!(ebml::read_header(&path), None);
    let Err(refused) = probe::probe(&path) else {
        panic!("a file that is not Matroska is refused, not guessed at")
    };
    let refused = refused.to_string();
    assert!(
        refused.contains("Matroska track list"),
        "it says what was wrong: {refused}"
    );
}

#[test]
fn a_track_with_no_stated_rate_records_none_rather_than_a_plausible_one() {
    // The defect this whole reader exists for: `unwrap_or(48_000)` on a track
    // that never said 48 000.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silentish.mkv");
    write_mkv(
        &path,
        &[element(
            0xAE,
            &cat(&[
                uint(0xD7, 1),
                uint(0x83, 2), // audio
                text(0x86, "A_OPUS"),
                // No Audio element at all: the file states no rate.
            ]),
        )],
    );
    let header = ebml::read_header(&path).expect("a track list");
    assert_eq!(header.tracks[0].sample_rate, None);
    assert_eq!(header.tracks[0].channels, None);
    let info = probe::probe(&path).unwrap();
    let a = info.first_audio().unwrap().audio.unwrap();
    assert_eq!(
        (a.sample_rate, a.channels),
        (0, 0),
        "nothing stated, nothing invented"
    );
}
