//! Frame-exact tests of every editing operation.

use cine_timeline::prelude::*;

fn f(n: i64) -> RationalTime {
    RationalTime::from_frames(n, 24.0)
}
fn r(start: i64, dur: i64) -> TimeRange {
    TimeRange::from_frames(start, dur, 24.0)
}
fn clip(name: &str, src_start: i64, dur: i64, avail: i64) -> Clip {
    Clip::external(
        name,
        format!("file:///{name}.mov"),
        Some(r(0, avail)),
        Some(r(src_start, dur)),
    )
}

/// A(src 10+48, avail 200) | B(src 100+72, avail 300) | gap 24 | C(src 0+24, avail 24)
fn track() -> Track {
    Track::new("V1", TrackKind::Video)
        .with(clip("A", 10, 48, 200))
        .with(clip("B", 100, 72, 300))
        .with(Gap::new(f(24)))
        .with(clip("C", 0, 24, 24))
}

/// A(src 10+48) | dissolve 12/12 | B(src 100+72) | gap 24 | C
fn track_with_dissolve() -> Track {
    let mut t = track();
    t.children
        .insert(1, Transition::dissolve("x", f(12), f(12)).into());
    t
}

fn src(t: &Track, i: usize) -> TimeRange {
    t.children[i].source_range().unwrap()
}
fn names(t: &Track) -> Vec<String> {
    t.children.iter().map(|c| c.name().to_string()).collect()
}

// ---- ripple trim ----------------------------------------------------------

#[test]
fn ripple_trim_tail_shifts_following_items() {
    let mut t = track();
    let applied = ripple_trim(&mut t, 0, Edge::Tail, f(12)).unwrap();
    assert_eq!(applied, f(12));
    assert_eq!(src(&t, 0), r(10, 60));
    assert_eq!(t.range_of_child(1), Some(r(60, 72)));
    assert_eq!(t.range_of_child(3), Some(r(156, 24)));
    assert_eq!(t.duration(), f(180));
}

#[test]
fn ripple_trim_clamps_to_media() {
    let mut t = track();
    // tail: media ends at 200, clip ends at 58 → at most +142
    let applied = ripple_trim(&mut t, 0, Edge::Tail, f(500)).unwrap();
    assert_eq!(applied, f(142));
    assert_eq!(src(&t, 0), r(10, 190));
    // head: media starts at 0, clip starts at 10 → at most -10
    let applied = ripple_trim(&mut t, 0, Edge::Head, f(-100)).unwrap();
    assert_eq!(applied, f(-10));
    assert_eq!(src(&t, 0), r(0, 200));
    // head forward: cannot exceed the duration
    let applied = ripple_trim(&mut t, 0, Edge::Head, f(1000)).unwrap();
    assert_eq!(applied, f(200));
    assert_eq!(src(&t, 0), r(200, 0));
    assert!(t.validate().is_ok());
}

#[test]
fn ripple_trim_head_and_gap() {
    let mut t = track();
    assert_eq!(ripple_trim(&mut t, 0, Edge::Head, f(8)).unwrap(), f(8));
    assert_eq!(src(&t, 0), r(18, 40));
    assert_eq!(t.range_of_child(1), Some(r(40, 72)));
    // gaps have no media bounds
    assert_eq!(ripple_trim(&mut t, 2, Edge::Tail, f(100)).unwrap(), f(100));
    assert_eq!(src(&t, 2), r(0, 124));
    assert_eq!(t.range_of_child(3), Some(r(40 + 72 + 124, 24)));
}

#[test]
fn ripple_trim_respects_transitions_and_rejects_bad_index() {
    let mut t = track_with_dissolve();
    // A must keep covering the dissolve's 12 in-frames
    let applied = ripple_trim(&mut t, 0, Edge::Tail, f(-40)).unwrap();
    assert_eq!(applied, f(-36));
    assert_eq!(src(&t, 0), r(10, 12));
    assert!(t.validate().is_ok());
    assert!(matches!(
        ripple_trim(&mut t, 1, Edge::Tail, f(1)),
        Err(Error::NotAnItem(1))
    ));
    assert!(matches!(
        ripple_trim(&mut t, 9, Edge::Tail, f(1)),
        Err(Error::IndexOutOfRange { .. })
    ));
}

// ---- roll -----------------------------------------------------------------

#[test]
fn roll_keeps_total_duration() {
    let mut t = track();
    let applied = roll(&mut t, 0, f(12)).unwrap();
    assert_eq!(applied, f(12));
    assert_eq!(src(&t, 0), r(10, 60));
    assert_eq!(src(&t, 1), r(112, 60));
    assert_eq!(t.range_of_child(1), Some(r(60, 60)));
    assert_eq!(t.duration(), f(168));

    let applied = roll(&mut t, 0, f(-30)).unwrap();
    assert_eq!(applied, f(-30));
    assert_eq!(src(&t, 0), r(10, 30));
    assert_eq!(src(&t, 1), r(82, 90));
    assert_eq!(t.duration(), f(168));
}

#[test]
fn roll_clamps_to_both_sides() {
    let mut t = track();
    // right side B can give up at most its 72 frames; left A could extend 142
    let applied = roll(&mut t, 0, f(300)).unwrap();
    assert_eq!(applied, f(72));
    assert_eq!(src(&t, 0), r(10, 120));
    assert_eq!(src(&t, 1), r(172, 0));
    assert_eq!(t.duration(), f(168));
    // left cannot shrink below zero
    let mut t = track();
    let applied = roll(&mut t, 0, f(-300)).unwrap();
    assert_eq!(applied, f(-48));
    assert_eq!(src(&t, 1), r(52, 120));
}

#[test]
fn roll_across_a_transition_moves_the_transition() {
    let mut t = track_with_dissolve();
    assert_eq!(roll(&mut t, 0, f(10)).unwrap(), f(10));
    assert_eq!(src(&t, 0), r(10, 58));
    assert_eq!(src(&t, 2), r(110, 62));
    assert_eq!(t.range_of_child(1), Some(r(58 - 12, 24)));
    assert_eq!(t.duration(), f(168));
    assert!(roll(&mut t, 4, f(1)).is_err()); // nothing after C
}

// ---- slip -----------------------------------------------------------------

#[test]
fn slip_moves_media_not_position() {
    let mut t = track();
    assert_eq!(slip(&mut t, 0, f(5)).unwrap(), f(5));
    assert_eq!(src(&t, 0), r(15, 48));
    assert_eq!(t.range_of_child(0), Some(r(0, 48)));
    assert_eq!(t.range_of_child(1), Some(r(48, 72)));
    assert_eq!(slip(&mut t, 0, f(-5)).unwrap(), f(-5));
    assert_eq!(src(&t, 0), r(10, 48));
}

#[test]
fn slip_clamps_to_media_and_ignores_gaps() {
    let mut t = track();
    assert_eq!(slip(&mut t, 0, f(500)).unwrap(), f(142));
    assert_eq!(src(&t, 0), r(152, 48));
    assert_eq!(slip(&mut t, 0, f(-1000)).unwrap(), f(-152));
    assert_eq!(src(&t, 0), r(0, 48));
    assert_eq!(slip(&mut t, 2, f(10)).unwrap(), f(0));
    assert_eq!(src(&t, 2), r(0, 24));
    // C uses its whole media: no room at all
    assert_eq!(slip(&mut t, 3, f(1)).unwrap(), f(0));
}

// ---- slide ----------------------------------------------------------------

#[test]
fn slide_moves_clip_between_neighbours() {
    let mut t = track();
    assert_eq!(slide(&mut t, 1, f(10)).unwrap(), f(10));
    assert_eq!(src(&t, 0), r(10, 58));
    assert_eq!(src(&t, 1), r(100, 72)); // content unchanged
    assert_eq!(t.range_of_child(1), Some(r(58, 72)));
    assert_eq!(src(&t, 2), r(10, 14)); // gap absorbed the move
    assert_eq!(t.range_of_child(3), Some(r(144, 24)));
    assert_eq!(t.duration(), f(168));
}

#[test]
fn slide_clamps_and_opens_a_leading_gap() {
    let mut t = track();
    // the gap after B is only 24 frames long
    assert_eq!(slide(&mut t, 1, f(100)).unwrap(), f(24));
    assert_eq!(src(&t, 2), r(24, 0));
    assert_eq!(t.duration(), f(168));
    // first item: cannot go earlier than zero, later opens a gap
    let mut t = track();
    assert_eq!(slide(&mut t, 0, f(-5)).unwrap(), f(0));
    assert_eq!(slide(&mut t, 0, f(10)).unwrap(), f(10));
    assert!(t.children[0].is_gap());
    assert_eq!(t.range_of_child(1), Some(r(10, 48)));
    assert_eq!(src(&t, 2), r(110, 62));
    assert_eq!(t.duration(), f(168));
    // sliding against a transition neighbour is refused
    let mut t = track_with_dissolve();
    assert!(matches!(slide(&mut t, 2, f(1)), Err(Error::NotAnItem(1))));
}

// ---- split / join ---------------------------------------------------------

#[test]
fn split_at_creates_boundary() {
    let mut t = track();
    assert_eq!(split_at(&mut t, f(24)).unwrap(), 1);
    assert_eq!(t.children.len(), 5);
    assert_eq!(src(&t, 0), r(10, 24));
    assert_eq!(src(&t, 1), r(34, 24));
    assert_eq!(t.children[1].name(), "A");
    assert_eq!(t.duration(), f(168));
    // existing boundary: nothing changes
    assert_eq!(split_at(&mut t, f(48)).unwrap(), 2);
    assert_eq!(t.children.len(), 5);
    assert_eq!(split_at(&mut t, f(168)).unwrap(), 5);
    assert!(matches!(
        split_at(&mut t, f(169)),
        Err(Error::TimeOutOfRange(_))
    ));
}

#[test]
fn split_keeps_markers_with_their_half_and_drops_duplicate_ids() {
    let mut t = track();
    if let Item::Clip(c) = &mut t.children[0] {
        c.markers.push(Marker::new("early", r(12, 1), "RED"));
        c.markers.push(Marker::new("late", r(40, 1), "RED"));
        c.metadata.insert(
            "ucg_id".into(),
            serde_json::Value::String(uuid::Uuid::nil().to_string()),
        );
    }
    split_at(&mut t, f(24)).unwrap();
    let a1 = t.children[0].as_clip().unwrap();
    let a2 = t.children[1].as_clip().unwrap();
    assert_eq!(a1.markers.len(), 1);
    assert_eq!(a1.markers[0].name, "early");
    assert_eq!(a2.markers.len(), 1);
    assert_eq!(a2.markers[0].name, "late");
    assert!(a1.metadata.contains_key("ucg_id"));
    assert!(!a2.metadata.contains_key("ucg_id"));
}

#[test]
fn split_inside_transition_is_refused() {
    let mut t = track_with_dissolve();
    assert!(matches!(
        split_at(&mut t, f(40)),
        Err(Error::TransitionAtCut(_))
    ));
    assert!(matches!(
        split_at(&mut t, f(48)),
        Err(Error::TransitionAtCut(_))
    ));
    assert_eq!(split_at(&mut t, f(70)).unwrap(), 3);
}

#[test]
fn join_reverses_split() {
    let original = track();
    let mut t = track();
    split_at(&mut t, f(24)).unwrap();
    join(&mut t, 0).unwrap();
    assert_eq!(t, original);
    assert!(matches!(join(&mut t, 0), Err(Error::NotContinuous(0, 1))));
    // two gaps are always continuous
    let mut g = Track::new("v", TrackKind::Video)
        .with(Gap::new(f(10)))
        .with(Gap::new(f(5)));
    join(&mut g, 0).unwrap();
    assert_eq!(g.children.len(), 1);
    assert_eq!(g.duration(), f(15));
}

// ---- insert / overwrite ---------------------------------------------------

#[test]
fn insert_ripples_and_splits() {
    let mut t = track();
    let idx = insert(&mut t, f(24), clip("X", 0, 24, 100).into()).unwrap();
    assert_eq!(idx, 1);
    assert_eq!(names(&t), ["A", "X", "A", "B", "", "C"]);
    assert_eq!(src(&t, 0), r(10, 24));
    assert_eq!(src(&t, 2), r(34, 24));
    assert_eq!(t.range_of_child(3), Some(r(72, 72)));
    assert_eq!(t.duration(), f(192));
    // at a boundary: no split
    let idx = insert(&mut t, f(72), clip("Y", 0, 10, 100).into()).unwrap();
    assert_eq!(idx, 3);
    assert_eq!(names(&t), ["A", "X", "A", "Y", "B", "", "C"]);
}

#[test]
fn insert_past_end_pads_with_gap_and_refuses_transitions() {
    let mut t = track();
    let idx = insert(&mut t, f(200), clip("X", 0, 24, 100).into()).unwrap();
    assert_eq!(idx, 5);
    assert!(t.children[4].is_gap());
    assert_eq!(src(&t, 4), r(0, 32));
    assert_eq!(t.duration(), f(224));
    let mut t = track_with_dissolve();
    assert!(matches!(
        insert(&mut t, f(40), clip("X", 0, 24, 100).into()),
        Err(Error::TransitionAtCut(_))
    ));
    assert_eq!(t, track_with_dissolve()); // untouched on failure
}

#[test]
fn overwrite_replaces_range_without_moving_others() {
    let mut t = track();
    let idx = overwrite(&mut t, f(24), clip("X", 0, 24, 100).into()).unwrap();
    assert_eq!(idx, 1);
    assert_eq!(names(&t), ["A", "X", "B", "", "C"]);
    assert_eq!(src(&t, 0), r(10, 24));
    assert_eq!(t.range_of_child(2), Some(r(48, 72)));
    assert_eq!(t.duration(), f(168));
}

#[test]
fn overwrite_past_end_and_over_several_items() {
    let mut t = track();
    let idx = overwrite(&mut t, f(100), clip("X", 0, 100, 100).into()).unwrap();
    assert_eq!(idx, 2);
    assert_eq!(names(&t), ["A", "B", "X"]);
    assert_eq!(src(&t, 1), r(100, 52));
    assert_eq!(t.range_of_child(2), Some(r(100, 100)));
    assert_eq!(t.duration(), f(200));
    assert!(overwrite(&mut t, f(0), Transition::dissolve("t", f(1), f(1)).into()).is_err());
}

// ---- lift / extract -------------------------------------------------------

#[test]
fn lift_leaves_a_gap() {
    let mut t = track();
    let removed = lift(&mut t, r(48, 72)).unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].name(), "B");
    assert!(t.children[1].is_gap());
    assert_eq!(t.range_of_child(1), Some(r(48, 72)));
    assert_eq!(t.range_of_child(3), Some(r(144, 24)));
    assert_eq!(t.duration(), f(168));
}

#[test]
fn lift_partial_clips() {
    let mut t = track();
    let removed = lift(&mut t, r(24, 36)).unwrap();
    assert_eq!(removed.len(), 2);
    assert_eq!(removed[0].source_range(), Some(r(34, 24)));
    assert_eq!(removed[1].source_range(), Some(r(100, 12)));
    assert_eq!(names(&t), ["A", "", "B", "", "C"]);
    assert_eq!(src(&t, 0), r(10, 24));
    assert_eq!(src(&t, 2), r(112, 60));
    assert_eq!(t.duration(), f(168));
}

#[test]
fn extract_ripples_delete() {
    let mut t = track();
    let removed = extract(&mut t, r(48, 72)).unwrap();
    assert_eq!(removed[0].name(), "B");
    assert_eq!(names(&t), ["A", "", "C"]);
    assert_eq!(t.range_of_child(2), Some(r(72, 24)));
    assert_eq!(t.duration(), f(96));
    // beyond the end is clamped
    let removed = extract(&mut t, r(60, 1000)).unwrap();
    assert_eq!(removed.len(), 2);
    assert_eq!(t.duration(), f(60));
}

#[test]
fn extract_removes_transitions_on_the_cut() {
    let mut t = track_with_dissolve();
    extract(&mut t, r(48, 72)).unwrap();
    assert_eq!(names(&t), ["A", "", "C"]);
    assert!(t.validate().is_ok());
    assert_eq!(t.duration(), f(96));
    // a range strictly inside the dissolve body (not touching the cut) is refused
    let mut t = track_with_dissolve();
    assert!(matches!(
        extract(&mut t, r(38, 4)),
        Err(Error::TransitionAtCut(_))
    ));
    assert_eq!(t, track_with_dissolve());
    // lifting the first clip drops the dissolve and leaves a gap
    let mut t = track_with_dissolve();
    let removed = lift(&mut t, r(0, 48)).unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(names(&t), ["", "B", "", "C"]);
    assert_eq!(t.duration(), f(168));
    // overwriting across the cut drops the dissolve too
    let mut t = track_with_dissolve();
    overwrite(&mut t, f(40), clip("X", 0, 20, 100).into()).unwrap();
    assert_eq!(names(&t), ["A", "X", "B", "", "C"]);
    assert_eq!(src(&t, 2), r(112, 60));
    assert_eq!(t.duration(), f(168));
}

// ---- diff -----------------------------------------------------------------

fn timeline_of(track: Track) -> Timeline {
    let mut tl = Timeline::new("t");
    tl.tracks.children.push(Composable::Track(track));
    tl
}

#[test]
fn diff_detects_trims_moves_adds_and_removes() {
    let old = timeline_of(track());
    let mut t = track();
    ripple_trim(&mut t, 0, Edge::Tail, f(12)).unwrap();
    let changes = diff(&old, &timeline_of(t));
    assert!(changes.contains(&Change {
        track: 0,
        index: 0,
        kind: ChangeKind::Trimmed
    }));
    assert!(changes.contains(&Change {
        track: 0,
        index: 1,
        kind: ChangeKind::Moved
    }));
    assert!(changes.contains(&Change {
        track: 0,
        index: 3,
        kind: ChangeKind::Moved
    }));
    assert_eq!(changes.len(), 4);

    let mut t = track();
    extract(&mut t, r(48, 72)).unwrap();
    let changes = diff(&old, &timeline_of(t));
    assert!(changes.contains(&Change {
        track: 0,
        index: 1,
        kind: ChangeKind::Removed
    }));

    let mut t = track();
    insert(&mut t, f(0), clip("N", 0, 5, 10).into()).unwrap();
    let changes = diff(&old, &timeline_of(t));
    assert!(changes.contains(&Change {
        track: 0,
        index: 0,
        kind: ChangeKind::Added
    }));

    assert!(diff(&old, &old).is_empty());
}
