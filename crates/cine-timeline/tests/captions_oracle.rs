//! Captions checked against an independent reader.
//!
//! The IMSC written here is decoded by **ttconv** (Sandflow, BSD-2-Clause), a
//! separate program that is neither linked nor shipped: it reads our TTML and
//! writes SRT, and that SRT is committed beside the TTML it came from. The
//! tests then hold two things:
//!
//! * the IMSC this crate writes today is byte-for-byte the document ttconv
//!   read, so the committed oracle output still describes it;
//! * reading ttconv's SRT back lands every caption on exactly the frame this
//!   crate wrote — so ttconv understood our frame-offset times as we meant
//!   them, including at NTSC rates and on a half-millisecond tie.
//!
//! To regenerate after an intentional change to the writer:
//!
//! ```text
//! CINE_WRITE_CAPTION_FIXTURES=1 cargo test -p cine-timeline --test captions_oracle
//! tests/data/captions/oracle.sh     # runs ttconv on each .ttml
//! ```
//!
//! and read the diff before committing it: a changed oracle output is a claim
//! about correctness, not a snapshot to accept.

use cine_timeline::captions::{read_srt, write_imsc, Caption, CaptionTrack, Placement};
use std::path::PathBuf;
use ucg_core::{Rate, RationalTime, TimeRange};

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/captions")
        .join(name)
}

/// Tracks chosen to be awkward: NTSC rates, a half-millisecond tie, an hour
/// in, two lines, escaping, a top placement and a speaker.
fn tracks() -> Vec<(&'static str, CaptionTrack)> {
    let mut out = Vec::new();
    for (name, rate) in [
        ("ntsc24", Rate::new(24000, 1001).unwrap()),
        ("ntsc30", Rate::new(30000, 1001).unwrap()),
        ("pal25", Rate::whole(25).unwrap()),
    ] {
        let at = |f: i64| RationalTime::exact(f, rate);
        let mut t = CaptionTrack::new("en", rate);
        // Frame 12: at 24000/1001 exactly 500.5 ms, a rounding tie.
        t.captions.push(Caption::new(
            TimeRange::new(at(12), at(36)),
            "We ride at dawn.",
        ));
        let mut two = Caption::new(
            TimeRange::new(at(60), at(55)),
            "Salt & <pepper>,\nand nothing else.",
        );
        two.speaker = Some("MAREN".into());
        t.captions.push(two);
        let mut top = Caption::new(TimeRange::new(at(200), at(40)), "HARBOUR OFFICE");
        top.placement = Placement::Top;
        t.captions.push(top);
        // An hour and a bit in, where accumulated drift would show.
        let hour = i64::from(rate.num()) * 3600 / i64::from(rate.den());
        t.captions.push(Caption::new(
            TimeRange::new(at(hour + 7), at(49)),
            "Not ever.",
        ));
        out.push((name, t));
    }
    out
}

#[test]
fn the_committed_imsc_is_what_the_writer_produces() {
    let write = std::env::var_os("CINE_WRITE_CAPTION_FIXTURES").is_some();
    for (name, track) in tracks() {
        let xml = write_imsc(&track).expect("a rate TTML can express");
        let path = data(&format!("{name}.ttml"));
        if write {
            std::fs::write(&path, &xml).expect("write fixture");
            continue;
        }
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is missing ({e}); see the module note", path.display()));
        assert_eq!(
            xml, committed,
            "{name}: the writer changed, so ttconv's reading of it must be regenerated"
        );
    }
}

#[test]
fn ttconv_reads_our_frame_times_as_the_frames_we_wrote() {
    for (name, track) in tracks() {
        let path = data(&format!("{name}.ttconv.srt"));
        let srt = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is missing ({e}); see the module note", path.display()));
        let (back, report) = read_srt(&srt, "en", track.rate).expect("ttconv's SRT reads");
        assert!(report.skipped.is_empty(), "{name}: {:?}", report.skipped);
        assert_eq!(
            back.captions.len(),
            track.captions.len(),
            "{name}: every caption survived"
        );
        for (ours, theirs) in track.captions.iter().zip(&back.captions) {
            assert_eq!(
                theirs.record,
                ours.record,
                "{name}: `{}` is on a different frame after ttconv read it",
                ours.lines.join(" / ")
            );
            assert_eq!(theirs.lines, ours.lines, "{name}: the text");
        }
        assert_eq!(
            report.snapped, 0,
            "{name}: nothing ttconv wrote was off a frame by a millisecond or more"
        );
    }
}
