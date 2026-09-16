//! CMX3600 EDL: the cut in the oldest format everything still reads.
//!
//! An EDL is a list of events — source in and out, record in and out — in a
//! fixed-column layout inherited from a 1970s edit controller. It carries the
//! cut and nothing else: no effects, no nested sequences, no audio levels, and
//! no clip names except as comments. That is exactly why it is still used, and
//! why a system that writes one must be explicit about what it has left
//! behind rather than writing a file that looks complete.
//!
//! What this writes:
//!
//! * one event per clip on the chosen track, in record order, with source and
//!   record timecode at the timeline's rate;
//! * `V`, `A`, or `A2` channel codes from the track's kind and index;
//! * `* FROM CLIP NAME:` comments, which every NLE reads back;
//! * `C` (cut) transitions only.
//!
//! What it refuses rather than approximates, in [`EdlReport::left_behind`]:
//! transitions other than cuts, more than one video track, clips with no
//! source range, and anything the format has no column for. The report is
//! returned with the text so a caller can say what the EDL is not.

use crate::otio::{Item, Timeline, TrackKind};
use ucg_core::{RationalTime, TimeRange};

/// What an EDL could not carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdlReport {
    /// Events written.
    pub events: usize,
    /// One line per thing the format dropped, in the words a cutter needs.
    pub left_behind: Vec<String>,
}

/// Which track to write, and how to label it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Video,
    /// 1-based audio track number.
    Audio(usize),
}

impl Channel {
    fn code(&self) -> String {
        match self {
            Channel::Video => "V".into(),
            Channel::Audio(1) => "A".into(),
            Channel::Audio(n) => format!("A{n}"),
        }
    }
}

/// Write one track of `timeline` as a CMX3600 EDL.
///
/// `title` is the EDL's own title line — by convention the sequence name, in
/// upper case, because the format is upper case.
pub fn write_edl(timeline: &Timeline, track_index: usize, title: &str) -> (String, EdlReport) {
    let mut report = EdlReport::default();
    let tracks: Vec<&crate::otio::Track> = timeline
        .tracks
        .children
        .iter()
        .filter_map(|c| c.as_track())
        .collect();
    let Some(track) = tracks.get(track_index).copied() else {
        report
            .left_behind
            .push(format!("there is no track {track_index} to write"));
        return (String::new(), report);
    };
    let video_tracks = tracks.iter().filter(|t| t.kind == TrackKind::Video).count();
    if track.kind == TrackKind::Video && video_tracks > 1 {
        report.left_behind.push(format!(
            "{video_tracks} video tracks: an EDL is one track, so {} of them are not in this file",
            video_tracks - 1
        ));
    }
    let channel = match track.kind {
        TrackKind::Video => Channel::Video,
        TrackKind::Audio => Channel::Audio(
            tracks
                .iter()
                .take(track_index + 1)
                .filter(|t| t.kind == TrackKind::Audio)
                .count()
                .max(1),
        ),
    };

    let start = timeline
        .global_start_time
        .unwrap_or_else(|| RationalTime::zero(timeline.duration().rate_exact()));

    let mut out = format!("TITLE: {}\nFCM: NON-DROP FRAME\n", title.to_uppercase());
    let mut at = start;
    let mut number = 0usize;
    for item in &track.children {
        match item {
            Item::Gap(gap) => {
                // A gap is black: the record time advances and no event is
                // written, which is what every EDL does.
                at = at.checked_add(&gap.source_range.duration);
            }
            Item::Transition(_) => {
                report.left_behind.push(
                    "a transition is written as a cut: an EDL dissolve needs the outgoing and \
                     incoming events to overlap, which this does not build"
                        .into(),
                );
            }
            Item::Clip(clip) => {
                let Some(source) = clip.source_range else {
                    report.left_behind.push(format!(
                        "`{}` has no source range, so there is no source timecode to write",
                        clip.name
                    ));
                    continue;
                };
                number += 1;
                let record = TimeRange::new(at, source.duration);
                let reel = reel_name(&clip.name);
                out.push_str(&format!(
                    "{number:03}  {reel:<8} {channel:<4} C        {} {} {} {}\n",
                    source.start_time.to_timecode(),
                    source.end_time_exclusive().to_timecode(),
                    record.start_time.to_timecode(),
                    record.end_time_exclusive().to_timecode(),
                    channel = channel.code(),
                ));
                out.push_str(&format!("* FROM CLIP NAME: {}\n", clip.name.to_uppercase()));
                at = at.checked_add(&source.duration);
                report.events += 1;
            }
        }
    }
    (out, report)
}

/// A reel name an EDL can carry: eight characters, upper case, no spaces.
///
/// Longer names live in the `FROM CLIP NAME` comment, which is where every NLE
/// looks for them; truncating there as well would lose the only full record of
/// what the event is.
fn reel_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>()
        .to_uppercase();
    if cleaned.is_empty() {
        "AX".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otio::{Clip, Gap, MediaReference, Track};
    use ucg_core::Rate;

    fn at(frames: i64) -> RationalTime {
        RationalTime::exact(frames, Rate::whole(24).expect("24"))
    }

    fn clip(name: &str, source_start: i64, frames: i64) -> Item {
        let mut c = Clip {
            name: name.into(),
            media_reference: MediaReference::Missing,
            ..Default::default()
        };
        c.source_range = Some(TimeRange::new(at(source_start), at(frames)));
        Item::Clip(c)
    }

    fn timeline(children: Vec<Item>) -> Timeline {
        let mut t = Timeline::new("Assembly");
        t.global_start_time = Some(at(86_400)); // 01:00:00:00 at 24 fps
        t.tracks
            .children
            .push(crate::otio::Composable::Track(Track {
                name: "V1".into(),
                kind: TrackKind::Video,
                children,
                ..Default::default()
            }));
        t
    }

    /// The columns, the timecodes, and the record times running end to end.
    #[test]
    fn two_cuts_are_two_events_with_continuous_record_time() {
        let t = timeline(vec![
            clip("A001C001", 24 * 10, 48),
            clip("A001C002", 24 * 30, 24),
        ]);
        let (text, report) = write_edl(&t, 0, "Assembly");
        assert_eq!(report.events, 2);
        assert!(report.left_behind.is_empty(), "{:?}", report.left_behind);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "TITLE: ASSEMBLY");
        assert_eq!(lines[1], "FCM: NON-DROP FRAME");
        assert_eq!(
            lines[2],
            "001  A001C001 V    C        00:00:10:00 00:00:12:00 01:00:00:00 01:00:02:00"
        );
        assert_eq!(lines[3], "* FROM CLIP NAME: A001C001");
        assert_eq!(
            lines[4], "002  A001C002 V    C        00:00:30:00 00:00:31:00 01:00:02:00 01:00:03:00",
            "the second event starts where the first ended"
        );
    }

    /// A gap moves the record clock and writes nothing, as black does.
    #[test]
    fn a_gap_advances_the_record_time_without_an_event() {
        let gap = Gap::new(at(24));
        let t = timeline(vec![
            clip("A001C001", 0, 24),
            Item::Gap(gap),
            clip("A001C002", 0, 24),
        ]);
        let (text, report) = write_edl(&t, 0, "Assembly");
        assert_eq!(report.events, 2);
        assert!(
            text.contains("01:00:02:00 01:00:03:00"),
            "the second event starts after the gap:\n{text}"
        );
    }

    /// What the format cannot carry is reported, not quietly dropped.
    #[test]
    fn what_an_edl_cannot_carry_is_said_out_loud() {
        let mut t = timeline(vec![clip("A001C001", 0, 24)]);
        t.tracks
            .children
            .push(crate::otio::Composable::Track(Track {
                name: "V2".into(),
                kind: TrackKind::Video,
                children: vec![clip("TITLE", 0, 24)],
                ..Default::default()
            }));
        let (_, report) = write_edl(&t, 0, "Assembly");
        assert!(
            report
                .left_behind
                .iter()
                .any(|w| w.contains("2 video tracks")),
            "{:?}",
            report.left_behind
        );

        // A clip with no source range has no source timecode to write.
        let mut nameless = Clip {
            name: "NO RANGE".into(),
            media_reference: MediaReference::Missing,
            ..Default::default()
        };
        nameless.source_range = None;
        let t = timeline(vec![Item::Clip(nameless)]);
        let (text, report) = write_edl(&t, 0, "Assembly");
        assert_eq!(report.events, 0);
        assert!(
            report.left_behind[0].contains("no source range"),
            "{:?}",
            report.left_behind
        );
        assert!(!text.contains("001  "), "and no event was invented");
    }

    #[test]
    fn a_reel_name_fits_the_column_and_the_full_name_is_kept_in_the_comment() {
        assert_eq!(reel_name("A001_C002_2026-01-05"), "A001C002");
        assert_eq!(reel_name("shot 12"), "SHOT12");
        assert_eq!(
            reel_name("—"),
            "AX",
            "something unusable is not an empty column"
        );
        let t = timeline(vec![clip("a very long clip name indeed", 0, 24)]);
        let (text, _) = write_edl(&t, 0, "Assembly");
        assert!(text.contains("AVERYLON"), "{text}");
        assert!(
            text.contains("* FROM CLIP NAME: A VERY LONG CLIP NAME INDEED"),
            "the full name survives in the comment:\n{text}"
        );
    }
}
