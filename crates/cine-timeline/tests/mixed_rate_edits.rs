//! Editing across a rate boundary, exactly.
//!
//! A real cut list mixes rates: a 23.976 camera master next to a 24 fps
//! graphic, audio at 48 kHz under both. Every existing editing test in this
//! crate runs at a single 24 fps, so the case these operations were most likely
//! to get wrong was the one nothing exercised.
//!
//! What they used to do: compute in `f64` frames, and in `roll` and `slide`
//! derive a scale factor by dividing one rate by another —
//! `rate / rtr.start_time.rate` — then multiply bounds through it and divide
//! the applied delta back. An `EPS` of 1e-9 stood in for exact comparison.
//!
//! **What that actually cost, measured rather than assumed.** These tests were
//! run against the pre-migration `edit.rs` on top of the now-exact
//! `RationalTime`. Seven of the eight passed. The rate-ratio drift the author
//! expected in `roll` and `slide` did not appear, because once `RationalTime`
//! became exact its constructor *promotes the rate* to represent a fractional
//! frame count rather than truncating it — so the float detour through
//! `.value()` and back was lossless for these cases.
//!
//! One did fail, and it was real: `lift` rescaled the range boundaries onto the
//! track's rate before measuring the gap it left behind, and the quantisation
//! left the track 24 ticks — one millisecond — shorter than it started.
//!
//! So the migration's value is not the drift story. It is that there is now no
//! epsilon deciding boundary questions by how arithmetic rounded, no rate ratio
//! that only happened to be harmless, and one fewer real defect. The remaining
//! tests stand as regression guards for invariants that were never explicitly
//! asserted at mixed rates before.

use cine_timeline::prelude::*;

const NTSC: f64 = 24_000.0 / 1001.0; // 23.976
const FILM: f64 = 24.0;
const PAL: f64 = 25.0;

fn at(n: i64, rate: f64) -> RationalTime {
    RationalTime::from_frames(n, rate)
}
fn range(start: i64, dur: i64, rate: f64) -> TimeRange {
    TimeRange::from_frames(start, dur, rate)
}
fn clip_at(name: &str, rate: f64, src_start: i64, dur: i64, avail: i64) -> Clip {
    Clip::external(
        name,
        format!("file:///{name}.mov"),
        Some(range(0, avail, rate)),
        Some(range(src_start, dur, rate)),
    )
}

/// An NTSC master, a film-rate graphic, and a PAL insert, in one track.
fn mixed_track() -> Track {
    Track::new("V1", TrackKind::Video)
        .with(clip_at("ntsc", NTSC, 10, 48, 400))
        .with(clip_at("film", FILM, 100, 72, 400))
        // A source start of 25 gives the PAL insert handles at its head, so a
        // slide can move it earlier. Starting at source 0 there would be no
        // media before it and every negative slide would correctly clamp to
        // nothing, which would make the test below vacuous.
        .with(clip_at("pal", PAL, 25, 50, 400))
}

fn src(t: &Track, i: usize) -> TimeRange {
    t.children[i].source_range().unwrap()
}

/// Roll's defining property: the cut moves, the track's length does not.
///
/// This is the test the old scale factor could not pass. The two sides of the
/// cut are at 24000/1001 and 24, and the ratio between them is not
/// representable in binary floating point.
#[test]
fn rolling_a_cut_between_rates_leaves_the_track_length_untouched() {
    for frames in [1i64, 2, 5, 12, -3, -7] {
        let mut t = mixed_track();
        let before = t.duration();

        let applied = roll(&mut t, 0, at(frames, NTSC)).expect("roll");
        let after = t.duration();

        assert_eq!(
            after, before,
            "rolling {frames} frames changed the track length from {before:?} to {after:?}; \
             a roll moves a cut and must not lengthen or shorten the track"
        );
        // And the cut actually moved by what was reported.
        assert_eq!(
            src(&t, 0).duration,
            mixed_track().children[0].source_range().unwrap().duration + applied,
            "the left item's duration must change by exactly the applied delta"
        );
    }
}

/// A hundred rolls back and forth must return the track to exactly where it
/// started. Under a float scale factor the residue accumulated.
#[test]
fn a_hundred_rolls_back_and_forth_leave_no_residue() {
    let mut t = mixed_track();
    let original = t.clone();

    for _ in 0..50 {
        roll(&mut t, 0, at(3, NTSC)).expect("roll out");
        roll(&mut t, 0, at(-3, NTSC)).expect("roll back");
    }

    assert_eq!(
        t.duration(),
        original.duration(),
        "the track length drifted"
    );
    for i in 0..t.children.len() {
        assert_eq!(
            src(&t, i),
            src(&original, i),
            "item {i} did not return to its original range after 100 rolls"
        );
    }
}

/// Sliding an item whose neighbours are at other rates: the neighbours absorb
/// the move exactly, so nothing else on the track shifts.
#[test]
fn sliding_between_neighbours_at_other_rates_preserves_the_track_length() {
    for frames in [1i64, 4, -2, -6] {
        let mut t = mixed_track();
        let before = t.duration();
        let applied = slide(&mut t, 1, at(frames, FILM)).expect("slide");
        assert_eq!(
            t.duration(),
            before,
            "sliding {frames} frames changed the track length"
        );
        // The slid item keeps its own duration; only its neighbours absorb.
        assert_eq!(
            src(&t, 1).duration,
            src(&mixed_track(), 1).duration,
            "a slide must not change the slid item's duration"
        );
        assert!(
            !applied.is_zero(),
            "the slide should not have been clamped to nothing"
        );
    }
}

/// A cut placed exactly on a boundary must be recognised as being on it, at any
/// rate. An epsilon decided this by how the arithmetic happened to round.
#[test]
fn a_cut_exactly_on_a_boundary_is_recognised_at_every_rate() {
    let t = mixed_track();
    // The first item is 48 frames at 24000/1001. That instant expressed at
    // 24 fps and at 48 kHz is the same instant, and splitting there must find
    // the existing boundary rather than cutting the second item.
    let boundary = at(48, NTSC);
    for probe in [
        boundary,
        boundary.rescaled_to(48_000.0),
        boundary.rescaled_to(90_000.0),
    ] {
        let mut t = t.clone();
        let before = t.children.len();
        let idx = split_at(&mut t, probe).expect("split on a boundary");
        assert_eq!(idx, 1, "the boundary after the first item is index 1");
        assert_eq!(
            t.children.len(),
            before,
            "splitting on an existing boundary must not create an item"
        );
    }
}

/// Splitting and re-joining must return exactly the original range — the
/// clearest statement that the arithmetic is lossless.
#[test]
fn split_then_join_round_trips_exactly_at_every_rate() {
    for (index, rate) in [(0usize, NTSC), (1, FILM), (2, PAL)] {
        let original = mixed_track();
        let mut t = original.clone();

        // Cut three frames into the item, in its own rate.
        let mut offset = at(0, rate);
        for i in 0..index {
            offset = offset + original.children[i].duration();
        }
        let cut = offset + at(3, rate);

        let idx = split_at(&mut t, cut).expect("split");
        assert_eq!(t.children.len(), original.children.len() + 1);

        join(&mut t, idx - 1).expect("join");
        assert_eq!(t.children.len(), original.children.len());
        assert_eq!(
            src(&t, index),
            src(&original, index),
            "item {index} at {rate} did not survive a split and join"
        );
        assert_eq!(t.duration(), original.duration());
    }
}

/// Trimming repeatedly at an NTSC rate must not accumulate error.
#[test]
fn repeated_ntsc_trims_do_not_accumulate_error() {
    let mut t = mixed_track();
    let original_duration = src(&t, 0).duration;

    for _ in 0..100 {
        ripple_trim(&mut t, 0, Edge::Tail, at(1, NTSC)).expect("out");
        ripple_trim(&mut t, 0, Edge::Tail, at(-1, NTSC)).expect("back");
    }
    assert_eq!(
        src(&t, 0).duration,
        original_duration,
        "100 trim pairs at 23.976 left the item a different length"
    );
}

/// A trim expressed in one rate applied to an item in another must land on an
/// exact instant, not a rounded one.
#[test]
fn a_trim_expressed_in_another_rate_lands_exactly() {
    let mut t = mixed_track();
    let before = src(&t, 1);

    // One second, expressed as 48000 samples, applied to a 24 fps item.
    let one_second = at(48_000, 48_000.0);
    let applied = ripple_trim(&mut t, 1, Edge::Tail, one_second).expect("trim");

    let after = src(&t, 1);
    assert_eq!(
        after.duration,
        before.duration + applied,
        "the applied delta must account for the change exactly"
    );
    // A second is a second: the item grew by exactly one second of media.
    assert!(
        (after.duration.to_seconds() - before.duration.to_seconds() - 1.0).abs() < 1e-9,
        "one second of trim did not add one second: {} -> {}",
        before.duration.to_seconds(),
        after.duration.to_seconds()
    );
}

/// Lifting a range leaves a gap of exactly the right length, so the track's
/// length is unchanged, even when the range boundaries are at another rate.
#[test]
fn lifting_a_range_expressed_in_another_rate_preserves_the_track_length() {
    let mut t = mixed_track();
    let before = t.duration();

    // A range that begins and ends inside items at different rates, expressed
    // at 48 kHz.
    let start = at(48, NTSC).rescaled_to(48_000.0);
    let end = start + at(24, FILM).rescaled_to(48_000.0);
    let removed = lift(&mut t, TimeRange::from_start_end(start, end)).expect("lift");

    assert!(!removed.is_empty(), "the lift removed nothing");
    assert_eq!(
        t.duration(),
        before,
        "lift leaves a gap of the same length, so the track must not change length"
    );
}
