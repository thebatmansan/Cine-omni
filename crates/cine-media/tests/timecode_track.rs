//! Source timecode, written and read back from real movies.
//!
//! A frame index counted from zero is this build's name for a frame; the
//! timecode is everybody else's, and a conform, an EDL, a dailies report and a
//! note from the cutting room are all in it. These tests hold the whole path:
//! the movie writer puts a real `tmcd` track in the file, the probe reads it
//! back, and the label that comes out is the label that went in — at every
//! rate this build claims, including the drop-frame ones where getting it
//! wrong is the classic one-frame conform error.

use cine_media::time::Timecode;
use cine_media::{probe, testsignal, tmcd};
use std::path::Path;
use ucg_core::Rate;

fn movie(path: &Path, rate: Rate, seconds: u32, tc: Option<Timecode>) {
    testsignal::bars_and_pops_from(path, 160, 90, rate, seconds, tc).unwrap();
}

#[test]
fn a_movie_carries_the_timecode_it_was_written_with_at_every_rate() {
    let dir = tempfile::tempdir().unwrap();
    // One rate per kind: exact, NTSC non-drop, NTSC drop frame, and PAL.
    let cases: &[(Rate, &str, bool)] = &[
        (Rate::F24, "01:00:00:00", false),
        (Rate::new(24000, 1001).unwrap(), "01:00:00:00", false),
        (Rate::new(30000, 1001).unwrap(), "01:00:00;00", true),
        (Rate::new(30000, 1001).unwrap(), "00:01:00;02", true),
        (Rate::new(60000, 1001).unwrap(), "09:59:59;59", true),
        (Rate::whole(25).unwrap(), "23:59:59:24", false),
        (Rate::whole(30).unwrap(), "00:00:00:00", false),
    ];
    for (rate, label, drop) in cases {
        let path = dir.path().join(format!(
            "{}-{}.mov",
            rate.num(),
            label.replace([':', ';'], "")
        ));
        let wanted = Timecode::parse(label, *rate).expect(label);
        assert_eq!(wanted.drop_frame, *drop, "{label} at {rate:?}");
        movie(&path, *rate, 1, Some(wanted));

        let read = tmcd::source_timecode(&path).unwrap_or_else(|| panic!("no timecode in {label}"));
        assert_eq!(
            read.start, wanted,
            "{label} at {rate:?} came back as {}",
            read.start
        );
        assert_eq!(read.start.to_string(), *label, "the label survives exactly");
        // The rate came out of the file as the ratio it is, not as a decimal.
        assert_eq!(read.rate, *rate, "{label}: {:?} is not {rate:?}", read.rate);
        assert_eq!(read.start_frame, wanted.to_frame_number().unwrap());
        assert!(!read.counter, "a time of day is not a counter");
        assert_eq!(read.runs, 1, "one continuous run");

        // And the probe puts it on the media, which is where everything else
        // reads it from.
        let info = probe::probe(&path).unwrap();
        assert_eq!(
            info.start_timecode,
            Some(wanted),
            "{label} through the probe"
        );
    }
}

#[test]
fn no_timecode_track_is_not_a_timecode_of_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("untimed.mov");
    movie(&path, Rate::F24, 1, None);
    assert_eq!(tmcd::source_timecode(&path), None);
    assert_eq!(probe::probe(&path).unwrap().start_timecode, None);

    // A take that really does start at zero says so, and is a different answer.
    let zero = dir.path().join("at-zero.mov");
    let tc = Timecode::parse("00:00:00:00", Rate::F24).unwrap();
    movie(&zero, Rate::F24, 1, Some(tc));
    assert_eq!(probe::probe(&zero).unwrap().start_timecode, Some(tc));
}

#[test]
fn the_test_signal_carries_an_hour_and_the_drop_frame_rates_get_drop_frame_labels() {
    let dir = tempfile::tempdir().unwrap();
    for rate in [
        Rate::F24,
        Rate::new(30000, 1001).unwrap(),
        Rate::whole(25).unwrap(),
    ] {
        let path = dir.path().join(format!("bars-{}.mov", rate.num()));
        testsignal::bars_and_pops(&path, 160, 90, rate, 1).unwrap();
        let tc = probe::probe(&path)
            .unwrap()
            .start_timecode
            .expect("timecode");
        assert_eq!((tc.hours, tc.minutes, tc.seconds, tc.frames), (1, 0, 0, 0));
        assert_eq!(
            tc.drop_frame,
            rate.supports_drop_frame(),
            "{rate:?} labels {tc}"
        );
    }
}

#[test]
fn a_timecode_that_does_not_belong_to_the_movies_rate_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mismatched.mov");
    // A 25 fps label in a 24 fps movie is not a conversion this can do: the
    // two count different frames, and writing it would make the file lie.
    let tc = Timecode::parse("01:00:00:00", Rate::whole(25).unwrap()).unwrap();
    let refused = testsignal::bars_and_pops_from(&path, 160, 90, Rate::F24, 1, Some(tc))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(refused.contains("counts at"), "it says why: {refused}");
    assert!(
        !path.exists() || probe::probe(&path).is_err(),
        "and leaves no movie claiming otherwise"
    );
}

#[test]
fn a_drop_frame_label_and_a_non_drop_one_are_different_instants() {
    // The whole point of the flag. One hour of 29.97 drop-frame labels covers
    // 107892 frames; the same labels read as non-drop would be 108000 — a
    // 108-frame conform error, which is the classic one.
    let rate = Rate::new(30000, 1001).unwrap();
    let drop = Timecode::parse("01:00:00;00", rate).unwrap();
    let non_drop = Timecode::parse("01:00:00:00", rate).unwrap();
    assert!(drop.drop_frame && !non_drop.drop_frame);
    assert_eq!(drop.to_frame_number().unwrap(), 107_892);
    assert_eq!(non_drop.to_frame_number().unwrap(), 108_000);

    let dir = tempfile::tempdir().unwrap();
    for tc in [drop, non_drop] {
        let path = dir
            .path()
            .join(format!("{}.mov", tc.to_frame_number().unwrap()));
        testsignal::bars_and_pops_from(&path, 160, 90, rate, 1, Some(tc)).unwrap();
        let read = tmcd::source_timecode(&path).unwrap();
        assert_eq!(read.start, tc);
        assert_eq!(read.start_frame, tc.to_frame_number().unwrap());
        assert_eq!(
            read.start.drop_frame, tc.drop_frame,
            "the flag survives the file"
        );
    }
}

/// A box walk written from the QuickTime File Format specification rather than
/// from [`cine_media::isobmff`], so that a mistake shared by this build's
/// writer and its reader — which a round-trip cannot see, because both sides
/// would make it — fails here.
mod by_the_spec {
    /// `(type, body)` of each box in a buffer.
    pub fn boxes(buf: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i + 8 <= buf.len() {
            let mut size = u32::from_be_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            let typ: [u8; 4] = buf[i + 4..i + 8].try_into().unwrap();
            let mut header = 8usize;
            if size == 1 {
                size = u64::from_be_bytes(buf[i + 8..i + 16].try_into().unwrap()) as usize;
                header = 16;
            } else if size == 0 {
                size = buf.len() - i;
            }
            if size < header || i + size > buf.len() {
                break;
            }
            out.push((typ, &buf[i + header..i + size]));
            i += size;
        }
        out
    }

    pub fn find<'a>(buf: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
        let mut cur = buf;
        for want in path {
            cur = boxes(cur)
                .into_iter()
                .find(|(t, _)| &t == want)
                .map(|(_, b)| b)?;
        }
        Some(cur)
    }

    pub fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
    }
}

#[test]
fn the_bytes_are_what_the_quicktime_specification_says_they_are() {
    use by_the_spec::{boxes, find, u32_at};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("df.mov");
    let rate = Rate::new(30000, 1001).unwrap();
    let tc = Timecode::parse("01:00:00;00", rate).unwrap();
    // One hour of drop-frame 29.97: 30 x 3600 labels, less two a minute for
    // 54 of every 60 minutes. 108000 - 108 = 107892, worked out here rather
    // than taken from the code under test.
    assert_eq!(tc.to_frame_number().unwrap(), 30 * 3600 - 2 * 54);
    testsignal::bars_and_pops_from(&path, 160, 90, rate, 1, Some(tc)).unwrap();

    let data = std::fs::read(&path).unwrap();
    let moov = find(&data, &[b"moov"]).expect("moov");
    let trak = boxes(moov)
        .into_iter()
        .filter(|(t, _)| t == b"trak")
        .map(|(_, b)| b)
        .find(|t| find(t, &[b"mdia", b"hdlr"]).is_some_and(|h| &h[8..12] == b"tmcd"))
        .expect("a track with a tmcd handler");
    let stbl = find(trak, &[b"mdia", b"minf", b"stbl"]).expect("stbl");

    // Timecode Sample Description, field by field at its stated offset.
    let stsd = find(stbl, &[b"stsd"]).expect("stsd");
    let entry = boxes(&stsd[8..])
        .into_iter()
        .find(|(t, _)| t == b"tmcd")
        .map(|(_, b)| b)
        .expect("a tmcd sample entry");
    assert_eq!(&entry[0..6], &[0u8; 6], "six reserved bytes");
    assert_eq!(
        u16::from_be_bytes(entry[6..8].try_into().unwrap()),
        1,
        "data reference index"
    );
    assert_eq!(u32_at(entry, 8), 0, "four reserved bytes");
    let flags = u32_at(entry, 12);
    assert_eq!(
        flags & 0x0001,
        0x0001,
        "drop frame is declared: {flags:#010x}"
    );
    assert_eq!(flags & 0x0002, 0x0002, "and a 24-hour clock: {flags:#010x}");
    assert_eq!(flags & 0x0008, 0, "this is a time of day, not a counter");
    assert_eq!(u32_at(entry, 16), 30_000, "time scale");
    assert_eq!(
        u32_at(entry, 20),
        1_001,
        "frame duration — 30000/1001 exactly, not 29.97"
    );
    assert_eq!(entry[24], 30, "the labelling rate");

    // One four-byte sample, and the number in it.
    let stsz = find(stbl, &[b"stsz"]).expect("stsz");
    assert_eq!(u32_at(stsz, 4), 4, "every sample is four bytes");
    assert_eq!(u32_at(stsz, 8), 1, "one sample: the timecode is continuous");
    let co64 = find(stbl, &[b"co64"]).expect("co64");
    let offset = u64::from_be_bytes(co64[8..16].try_into().unwrap()) as usize;
    assert_eq!(
        u32_at(&data, offset),
        107_892,
        "the frame number in the file is the one an hour of drop frame means"
    );
}
