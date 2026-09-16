//! Three- and four-point editing: the marks decide the edit, not the mouse.
//!
//! A cutting room does not place clips by dragging. It marks an **in** and an
//! **out** in the source, an **in** in the record (the timeline), and presses
//! a key; the fourth point is arithmetic. That is a three-point edit, and it
//! is the reason an assembly can be cut from a shot list without ever looking
//! at the timeline.
//!
//! The four marks are:
//!
//! | mark | meaning |
//! |------|---------|
//! | source in  | the first frame of media that plays |
//! | source out | one past the last frame that plays |
//! | record in  | where on the track it lands |
//! | record out | one past where it ends on the track |
//!
//! Give any three and [`plan`] derives the fourth, and says in words which one
//! it derived — an editor who marked the record out rather than the record in
//! has *backtimed* the edit, and the plan says so, because a backtimed edit
//! that silently became a forward one lands in the wrong place.
//!
//! **Four points are a different question.** If all four are marked and the
//! durations agree, there is nothing to decide. If they disagree, a real
//! system does one of three things, and this makes the caller choose rather
//! than picking quietly:
//!
//! * refuse, naming the difference ([`Mismatch::Refuse`], the default);
//! * keep the timeline marks and take less of the source
//!   ([`Mismatch::TrimSourceTail`], [`Mismatch::TrimSourceHead`]);
//! * keep the source and move the record out ([`Mismatch::HonourSource`]);
//! * keep both and change the speed ([`Mismatch::FitToFill`]).
//!
//! Fit to fill is the interesting one, and it is exact: the plan carries a
//! [`Retime`] whose length is the timeline marks and whose media is the source
//! marks, so "48 frames over 24" is a retime of exactly 2, not a `time_scalar`
//! of 2.0000001.

use crate::edit::{insert, overwrite};
use crate::error::{Error, Result};
use crate::otio::{Clip, Item, Retime, Track};
use ucg_core::{RationalTime, TimeRange};

/// An in point, an out point, or one of each. Either may be unset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Marks {
    /// The in point: the first frame.
    pub in_point: Option<RationalTime>,
    /// The out point, exclusive: one past the last frame.
    pub out_point: Option<RationalTime>,
}

impl Marks {
    /// Both ends.
    pub fn range(start: RationalTime, end_exclusive: RationalTime) -> Marks {
        Marks {
            in_point: Some(start),
            out_point: Some(end_exclusive),
        }
    }

    /// An in point only.
    pub fn starting_at(start: RationalTime) -> Marks {
        Marks {
            in_point: Some(start),
            out_point: None,
        }
    }

    /// An out point only: the edit is backtimed to end there.
    pub fn ending_at(end_exclusive: RationalTime) -> Marks {
        Marks {
            in_point: None,
            out_point: Some(end_exclusive),
        }
    }

    /// How many of the two are set.
    pub fn count(&self) -> usize {
        usize::from(self.in_point.is_some()) + usize::from(self.out_point.is_some())
    }

    /// The duration the two marks describe, when both are set.
    pub fn duration(&self) -> Option<RationalTime> {
        match (self.in_point, self.out_point) {
            (Some(a), Some(b)) => Some(b.checked_sub(&a)),
            _ => None,
        }
    }
}

/// Whether the edit pushes what follows along or covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Ripple: everything after the record in moves later.
    Insert,
    /// Overwrite: nothing moves; what was there is covered.
    Overwrite,
}

/// What to do when four marks are given and the two durations disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mismatch {
    /// Refuse, and say by how many frames and in which direction.
    #[default]
    Refuse,
    /// Keep the record marks. Take the source from its in point and drop the
    /// tail (or extend towards the out point when the source is too short).
    TrimSourceTail,
    /// Keep the record marks. Take the source ending at its out point, so the
    /// end of the shot is what survives.
    TrimSourceHead,
    /// Keep the source. Move whichever record point was not the anchor.
    HonourSource,
    /// Keep both, and change the clip's speed so the marked media fills the
    /// marked time exactly.
    FitToFill,
}

/// A resolved edit: exactly what media plays and exactly where it lands.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The range taken from the source, in media time.
    pub source: TimeRange,
    /// Where it lands, in track time.
    pub record: TimeRange,
    pub kind: EditKind,
    /// The speed change the edit needs, when the marks asked for one.
    pub retime: Option<Retime>,
    /// Whether the record in was derived from the record out. A backtimed
    /// edit ends where it was marked; a forward one starts where it was.
    pub backtimed: bool,
    /// Which points were arithmetic rather than marked, in the words an
    /// editor would use. Empty when all four were given and agreed.
    pub derived: Vec<String>,
    /// How a four-point disagreement was settled, when there was one.
    pub reconciled: Option<String>,
}

impl Plan {
    /// A one-line account, for a status bar or a log.
    pub fn describe(&self) -> String {
        let verb = match self.kind {
            EditKind::Insert => "insert",
            EditKind::Overwrite => "overwrite",
        };
        let mut out = format!(
            "{verb} {} frames at {}",
            self.record.duration.to_frames(),
            self.record.start_time.to_timecode()
        );
        if self.backtimed {
            out.push_str(" (backtimed)");
        }
        for note in &self.derived {
            out.push_str("; ");
            out.push_str(note);
        }
        if let Some(note) = &self.reconciled {
            out.push_str("; ");
            out.push_str(note);
        }
        out
    }
}

/// Work out the edit from the marks.
///
/// `available` is the media the source clip actually has. A plan is never
/// returned for media that does not exist: running past the end is refused
/// with the shortfall, because the alternative is an edit that plays black.
/// `None` means the length of the media is not known — an asset that has not
/// been probed — and then the marks are taken at their word and the bounds
/// are not checked, which the plan says out loud rather than implying a
/// check that did not happen.
pub fn plan(
    available: Option<TimeRange>,
    source: Marks,
    record: Marks,
    kind: EditKind,
    on_mismatch: Mismatch,
) -> Result<Plan> {
    let mut derived: Vec<String> = Vec::new();

    // A record point is not optional: something has to say where the edit
    // goes. A caller with a playhead and no marks passes the playhead as the
    // record in — that is what pressing the key means.
    if record.count() == 0 {
        return Err(Error::Edit(
            "no record point: mark the timeline, or pass the playhead as the record in".into(),
        ));
    }

    // The source range as marked, filling in from the available media only
    // where that is unambiguous: no marks at all means the whole clip, which
    // is what loading a clip and pressing the key has always meant.
    let (source_in, source_out) = match (source.in_point, source.out_point) {
        (Some(a), Some(b)) => (Some(a), Some(b)),
        (None, None) => {
            let Some(a) = available else {
                return Err(Error::Edit(
                    "the source is unmarked and the length of the media is not known, \
                     so there is no whole clip to take: mark the source"
                        .into(),
                ));
            };
            derived.push("the whole clip: the source is unmarked".into());
            (Some(a.start_time), Some(a.end_time_exclusive()))
        }
        (a, b) => (a, b),
    };

    let source_duration = match (source_in, source_out) {
        (Some(a), Some(b)) => Some(b.checked_sub(&a)),
        _ => None,
    };
    let record_duration = record.duration();

    // Three points at least. Two leaves the duration unknown on both sides.
    if source_duration.is_none() && record_duration.is_none() {
        return Err(Error::Edit(format!(
            "a three-point edit needs three marks and has {}: \
             mark the source out, or mark the timeline out",
            usize::from(source_in.is_some()) + usize::from(source_out.is_some()) + record.count()
        )));
    }

    // Resolve the source.
    let (mut src_start, mut src_duration) = match (source_in, source_out, record_duration) {
        (Some(a), Some(_), _) => (a, source_duration.expect("both marks")),
        // Source in only: the timeline marks say how long.
        (Some(a), None, Some(d)) => {
            derived.push("the source out came from the timeline marks".into());
            (a, d)
        }
        // Source out only: backtimed in the source too — the shot ends where
        // it was marked and starts however far back it has to.
        (None, Some(b), Some(d)) => {
            derived
                .push("the source in came from the timeline marks, backwards from the out".into());
            (b.checked_sub(&d), d)
        }
        _ => unreachable!("covered by the three-mark check above"),
    };

    // Resolve the record.
    let mut backtimed = false;
    let mut rec_start = match (record.in_point, record.out_point) {
        (Some(a), _) => a,
        (None, Some(b)) => {
            backtimed = true;
            derived.push("the timeline in was backtimed from the out".into());
            b.checked_sub(&src_duration)
        }
        (None, None) => unreachable!("record.count() > 0"),
    };
    if record.in_point.is_some() && record.out_point.is_none() {
        derived.push("the timeline out came from the source marks".into());
    }

    // Four points: do the two durations agree?
    let mut reconciled = None;
    let mut retime = None;
    if let Some(rd) = record_duration.filter(|_| source.count() == 2) {
        if rd != src_duration {
            let difference = src_duration.checked_sub(&rd).to_frames();
            let longer = if difference > 0 { "longer" } else { "shorter" };
            match on_mismatch {
                Mismatch::Refuse => {
                    return Err(Error::Edit(format!(
                        "four marks disagree: the source is {} frames {longer} than the timeline \
                         marks. Trim the source at either end, keep its length and move a \
                         timeline mark, or fit to fill and change the clip's speed",
                        difference.abs()
                    )));
                }
                Mismatch::TrimSourceTail => {
                    src_duration = rd;
                    reconciled = Some(format!(
                        "{} frames trimmed from the end of the source to fit the timeline marks",
                        difference.abs()
                    ));
                }
                Mismatch::TrimSourceHead => {
                    let end = src_start.checked_add(&src_duration);
                    src_start = end.checked_sub(&rd);
                    src_duration = rd;
                    reconciled = Some(format!(
                        "{} frames trimmed from the head of the source to fit the timeline marks",
                        difference.abs()
                    ));
                }
                Mismatch::HonourSource => {
                    if record.in_point.is_none() {
                        rec_start = record
                            .out_point
                            .expect("a record point exists")
                            .checked_sub(&src_duration);
                    }
                    reconciled = Some(format!(
                        "the source kept its length; the timeline {} moved by {} frames",
                        if record.in_point.is_none() {
                            "in"
                        } else {
                            "out"
                        },
                        difference.abs()
                    ));
                }
                Mismatch::FitToFill => {
                    retime = Some(Retime {
                        played: rd,
                        reversed: false,
                    });
                    let speed = Retime {
                        played: rd,
                        reversed: false,
                    }
                    .speed_of(src_duration);
                    reconciled = Some(format!(
                        "fit to fill: {} media frames over {} on the timeline, a speed of {}/{}",
                        src_duration.to_frames(),
                        rd.to_frames(),
                        speed.0,
                        speed.1,
                    ));
                }
            }
        }
    }

    // What the clip occupies on the track: its own length, unless a retime
    // says otherwise.
    let played = retime.map(|r| r.played).unwrap_or(src_duration);
    if src_duration <= RationalTime::zero(src_duration.rate_exact()) {
        return Err(Error::Edit(
            "the edit has no length: the out point is at or before the in point".into(),
        ));
    }

    // The media has to exist. An edit that runs off the end of the shot is
    // refused with the shortfall rather than padded with black.
    let src_end = src_start.checked_add(&src_duration);
    match available {
        Some(a) => {
            if src_start < a.start_time {
                let short = a.start_time.checked_sub(&src_start).to_frames();
                return Err(Error::Edit(format!(
                    "the edit starts {short} frames before the media does"
                )));
            }
            if src_end > a.end_time_exclusive() {
                let short = src_end.checked_sub(&a.end_time_exclusive()).to_frames();
                return Err(Error::Edit(format!(
                    "the edit runs {short} frames past the end of the media"
                )));
            }
        }
        None => derived.push(
            "the length of the media is not known, so the marks were not checked against it".into(),
        ),
    }

    Ok(Plan {
        source: TimeRange::new(src_start, src_duration),
        record: TimeRange::new(rec_start, played),
        kind,
        retime,
        backtimed,
        derived,
        reconciled,
    })
}

/// Carry out a plan on a track, using `clip` as the thing being cut in.
///
/// The clip's source range is set to the plan's; everything else about it —
/// its name, its media references, its markers — is kept, because the clip
/// being edited in is the clip the editor loaded.
pub fn perform(track: &mut Track, clip: Clip, plan: &Plan) -> Result<usize> {
    let mut clip = clip;
    clip.source_range = Some(plan.source);
    clip.retime = plan.retime;
    let item = Item::Clip(clip);
    match plan.kind {
        EditKind::Insert => insert(track, plan.record.start_time, item),
        EditKind::Overwrite => overwrite(track, plan.record.start_time, item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otio::TrackKind;

    fn at(frames: i64) -> RationalTime {
        RationalTime::from_frames(frames, 24.0)
    }

    fn media() -> TimeRange {
        TimeRange::from_frames(0, 240, 24.0)
    }

    fn source_clip() -> Clip {
        Clip::external("A001C001", "file:///a.mov", Some(media()), None)
    }

    /// The classic: two marks in the source, one in the timeline.
    #[test]
    fn three_points_derive_the_fourth() {
        let p = plan(
            Some(media()),
            Marks::range(at(48), at(96)),
            Marks::starting_at(at(120)),
            EditKind::Overwrite,
            Mismatch::Refuse,
        )
        .expect("a plan");
        assert_eq!(p.source, TimeRange::from_frames(48, 48, 24.0));
        assert_eq!(p.record, TimeRange::from_frames(120, 48, 24.0));
        assert!(!p.backtimed);
        assert!(
            p.derived.iter().any(|d| d.contains("timeline out")),
            "{:?}",
            p.derived
        );
    }

    /// Backtiming: marked where it must *end*. The frame that was marked is
    /// the one the edit ends on, and the start is arithmetic.
    #[test]
    fn a_record_out_backtimes_the_edit() {
        let p = plan(
            Some(media()),
            Marks::range(at(48), at(96)),
            Marks::ending_at(at(120)),
            EditKind::Overwrite,
            Mismatch::Refuse,
        )
        .expect("a plan");
        assert!(p.backtimed, "the edit was marked by its end");
        assert_eq!(p.record, TimeRange::from_frames(72, 48, 24.0));
        assert_eq!(
            p.record.end_time_exclusive(),
            at(120),
            "it ends on the mark"
        );
    }

    /// The other three-point shape: the timeline says how long, the source
    /// says where to start.
    #[test]
    fn timeline_marks_can_decide_the_source_length() {
        let p = plan(
            Some(media()),
            Marks::starting_at(at(100)),
            Marks::range(at(0), at(36)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect("a plan");
        assert_eq!(p.source, TimeRange::from_frames(100, 36, 24.0));
        assert_eq!(p.record, TimeRange::from_frames(0, 36, 24.0));

        // And with only a source out, the source is backtimed too.
        let p = plan(
            Some(media()),
            Marks::ending_at(at(100)),
            Marks::range(at(0), at(36)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect("a plan");
        assert_eq!(
            p.source,
            TimeRange::from_frames(64, 36, 24.0),
            "the shot ends on the marked frame"
        );
    }

    /// An unmarked source is the whole clip: what pressing the key has always
    /// meant, and it says so rather than leaving the caller to guess.
    #[test]
    fn an_unmarked_source_is_the_whole_clip() {
        let p = plan(
            Some(media()),
            Marks::default(),
            Marks::starting_at(at(0)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect("a plan");
        assert_eq!(p.source, media());
        assert!(p.derived.iter().any(|d| d.contains("whole clip")));
    }

    /// Two points is not an edit, and the refusal says which mark is missing.
    #[test]
    fn two_points_are_refused_with_the_reason() {
        let e = plan(
            Some(media()),
            Marks::starting_at(at(48)),
            Marks::starting_at(at(0)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect_err("two points");
        assert!(e.to_string().contains("three marks"), "{e}");

        let e = plan(
            Some(media()),
            Marks::range(at(0), at(24)),
            Marks::default(),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect_err("no record point");
        assert!(e.to_string().contains("no record point"), "{e}");
    }

    /// Four points that disagree: refused by default, and the refusal names
    /// the difference and says why fit-to-fill is not on offer.
    #[test]
    fn four_points_that_disagree_are_refused_by_default() {
        let e = plan(
            Some(media()),
            Marks::range(at(48), at(96)), // 48 frames
            Marks::range(at(0), at(24)),  // 24 frames
            EditKind::Overwrite,
            Mismatch::Refuse,
        )
        .expect_err("a mismatch");
        let text = e.to_string();
        assert!(text.contains("24 frames longer"), "{text}");
        assert!(
            text.contains("speed"),
            "fit to fill is named, not done: {text}"
        );
    }

    /// And the three ways it can be settled, each of which says what it did.
    #[test]
    fn a_four_point_mismatch_can_be_settled_three_ways() {
        let source = Marks::range(at(48), at(96));
        let record = Marks::range(at(0), at(24));

        let tail = plan(
            Some(media()),
            source,
            record,
            EditKind::Overwrite,
            Mismatch::TrimSourceTail,
        )
        .expect("a plan");
        assert_eq!(tail.source, TimeRange::from_frames(48, 24, 24.0));
        assert_eq!(tail.record, TimeRange::from_frames(0, 24, 24.0));
        assert!(tail
            .reconciled
            .expect("a note")
            .contains("end of the source"));

        let head = plan(
            Some(media()),
            source,
            record,
            EditKind::Overwrite,
            Mismatch::TrimSourceHead,
        )
        .expect("a plan");
        assert_eq!(
            head.source,
            TimeRange::from_frames(72, 24, 24.0),
            "the end of the shot is what survives"
        );

        let keep = plan(
            Some(media()),
            source,
            record,
            EditKind::Overwrite,
            Mismatch::HonourSource,
        )
        .expect("a plan");
        assert_eq!(keep.record, TimeRange::from_frames(0, 48, 24.0));
        assert!(keep.reconciled.expect("a note").contains("out moved"));
    }

    /// Media that does not exist is refused with the shortfall, never padded.
    #[test]
    fn an_edit_past_the_end_of_the_media_is_refused() {
        let e = plan(
            Some(media()),
            Marks::starting_at(at(220)),
            Marks::range(at(0), at(48)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect_err("past the end");
        assert!(e.to_string().contains("8 frames past the end"), "{e}");

        let e = plan(
            Some(media()),
            Marks::ending_at(at(24)),
            Marks::range(at(0), at(48)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect_err("before the start");
        assert!(e.to_string().contains("24 frames before"), "{e}");
    }

    /// And the plan actually lands on the track, in the right place.
    #[test]
    fn performing_the_plan_puts_the_media_where_the_plan_says() {
        let mut track = Track::new("V1", TrackKind::Video);
        let first = plan(
            Some(media()),
            Marks::range(at(48), at(96)),
            Marks::starting_at(at(0)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect("a plan");
        let index = perform(&mut track, source_clip(), &first).expect("performed");
        assert_eq!(index, 0);
        assert_eq!(
            track.range_of_child(0),
            Some(TimeRange::from_frames(0, 48, 24.0))
        );
        assert_eq!(
            track.children[0].source_range(),
            Some(TimeRange::from_frames(48, 48, 24.0)),
            "the media that plays is the media that was marked"
        );

        // A backtimed overwrite lands so that it *ends* on its mark.
        let second = plan(
            Some(media()),
            Marks::range(at(0), at(24)),
            Marks::ending_at(at(48)),
            EditKind::Overwrite,
            Mismatch::Refuse,
        )
        .expect("a plan");
        perform(&mut track, source_clip(), &second).expect("performed");
        assert_eq!(track.duration(), at(48), "an overwrite moved nothing");
        let last = track.children.len() - 1;
        assert_eq!(
            track.range_of_child(last),
            Some(TimeRange::from_frames(24, 24, 24.0)),
            "it ends on the mark"
        );
    }

    /// Media whose length nobody knows: the marks are taken at their word,
    /// and the plan says the bounds were not checked rather than implying
    /// they were.
    #[test]
    fn unprobed_media_is_cut_on_the_marks_and_says_so() {
        let p = plan(
            None,
            Marks::range(at(48), at(96)),
            Marks::starting_at(at(0)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect("a plan");
        assert_eq!(p.source, TimeRange::from_frames(48, 48, 24.0));
        assert!(
            p.derived.iter().any(|d| d.contains("not checked")),
            "{:?}",
            p.derived
        );

        // But "the whole clip" is not a thing that can be derived.
        let e = plan(
            None,
            Marks::default(),
            Marks::starting_at(at(0)),
            EditKind::Insert,
            Mismatch::Refuse,
        )
        .expect_err("no length to take");
        assert!(e.to_string().contains("no whole clip"), "{e}");
    }

    #[test]
    fn a_plan_can_say_what_it_is_in_one_line() {
        let p = plan(
            Some(media()),
            Marks::range(at(48), at(96)),
            Marks::ending_at(at(120)),
            EditKind::Overwrite,
            Mismatch::Refuse,
        )
        .expect("a plan");
        let text = p.describe();
        assert!(
            text.starts_with("overwrite 48 frames at 00:00:03:00"),
            "{text}"
        );
        assert!(text.contains("backtimed"), "{text}");
    }
}
