//! Editorial session state: locks, mute and solo, linking, snapping, handles.
//!
//! These are the rules an editor lives inside, and most of them are easy to get
//! subtly wrong in ways no round-trip test would notice: solo implemented as
//! "mute everything else" loses the mute flags, a lock honoured by nine
//! operations out of ten is not a lock, and a link that does not survive a save
//! quietly unpairs a take.

use cine_timeline::prelude::*;
use cine_timeline::session::{
    any_solo_of, clear_link, ensure_unlocked, handles, is_audible, link_of, linked_positions,
    set_link, snap, snap_targets, snapped, targeted_tracks, TrackState, NAMESPACE,
};
use cine_timeline::{read_otio, write_otio, Composable, Timeline};

fn clip(name: &str, start: i64, dur: i64, available: Option<(i64, i64)>) -> Clip {
    Clip::external(
        name,
        format!("file:///{name}.mov"),
        available.map(|(s, d)| TimeRange::from_frames(s, d, 24.0)),
        Some(TimeRange::from_frames(start, dur, 24.0)),
    )
}

fn track(name: &str, kind: TrackKind, clips: usize) -> Track {
    let mut t = Track::new(name, kind);
    for i in 0..clips {
        t.children.push(Item::Clip(clip(
            &format!("{name}-{i}"),
            0,
            48,
            Some((0, 200)),
        )));
    }
    t
}

fn timeline(tracks: Vec<Track>) -> Timeline {
    let mut tl = Timeline::new("session");
    for t in tracks {
        tl.tracks.children.push(Composable::Track(t));
    }
    tl
}

fn frames(n: i64) -> RationalTime {
    RationalTime::from_frames(n, 24.0)
}

// ---------------------------------------------------------------------------
// Locking
// ---------------------------------------------------------------------------

/// A locked track refuses every editing operation, and refuses it *completely*:
/// the track is byte-for-byte what it was.
#[test]
fn a_locked_track_refuses_every_edit_and_is_left_untouched() {
    let mut t = track("V1", TrackKind::Video, 3);
    TrackState {
        locked: true,
        ..Default::default()
    }
    .apply(&mut t);
    let before = t.clone();

    /// Each attempt returns whether the operation was refused.
    type Attempt = (&'static str, Box<dyn Fn(&mut Track) -> bool>);
    let attempts: Vec<Attempt> = vec![
        (
            "ripple_trim",
            Box::new(|t: &mut Track| ripple_trim(t, 0, Edge::Tail, frames(6)).is_err()),
        ),
        (
            "roll",
            Box::new(|t: &mut Track| roll(t, 1, frames(4)).is_err()),
        ),
        (
            "slip",
            Box::new(|t: &mut Track| slip(t, 0, frames(4)).is_err()),
        ),
        (
            "slide",
            Box::new(|t: &mut Track| slide(t, 1, frames(4)).is_err()),
        ),
        (
            "split_at",
            Box::new(|t: &mut Track| split_at(t, frames(24)).is_err()),
        ),
        ("join", Box::new(|t: &mut Track| join(t, 0).is_err())),
        (
            "insert",
            Box::new(|t: &mut Track| {
                insert(t, frames(0), Item::Clip(clip("new", 0, 24, Some((0, 100))))).is_err()
            }),
        ),
        (
            "overwrite",
            Box::new(|t: &mut Track| {
                overwrite(t, frames(0), Item::Clip(clip("new", 0, 24, Some((0, 100))))).is_err()
            }),
        ),
        (
            "lift",
            Box::new(|t: &mut Track| lift(t, TimeRange::from_frames(0, 24, 24.0)).is_err()),
        ),
        (
            "extract",
            Box::new(|t: &mut Track| extract(t, TimeRange::from_frames(0, 24, 24.0)).is_err()),
        ),
        (
            "replace",
            Box::new(|t: &mut Track| {
                replace(t, 0, Item::Clip(clip("new", 0, 48, Some((0, 200))))).is_err()
            }),
        ),
    ];
    for (name, attempt) in attempts {
        assert!(attempt(&mut t), "{name} succeeded on a locked track");
        assert_eq!(t, before, "{name} modified a locked track");
    }
}

/// Unlocking restores the ability to edit, and nothing else about the track
/// changed while it was locked.
#[test]
fn unlocking_restores_editing() {
    let mut t = track("V1", TrackKind::Video, 2);
    let locked = TrackState {
        locked: true,
        ..Default::default()
    };
    locked.apply(&mut t);
    assert!(ensure_unlocked(&t).is_err());

    TrackState {
        locked: false,
        ..locked
    }
    .apply(&mut t);
    assert!(ensure_unlocked(&t).is_ok());
    assert!(ripple_trim(&mut t, 0, Edge::Tail, frames(6)).is_ok());
}

/// A locked track still plays. Locking is about editing, not output.
#[test]
fn a_locked_track_is_still_audible() {
    let mut t = track("A1", TrackKind::Audio, 1);
    TrackState {
        locked: true,
        ..Default::default()
    }
    .apply(&mut t);
    assert!(is_audible(&t, false));
}

// ---------------------------------------------------------------------------
// Mute and solo
// ---------------------------------------------------------------------------

/// Solo is not "mute everything else": the mute flags survive soloing and come
/// back unchanged when solo is cleared.
#[test]
fn solo_does_not_destroy_the_mute_flags() {
    let a = track("A1", TrackKind::Audio, 1);
    let mut b = track("A2", TrackKind::Audio, 1);
    let mut c = track("A3", TrackKind::Audio, 1);
    TrackState {
        muted: true,
        ..Default::default()
    }
    .apply(&mut b);
    TrackState {
        solo: true,
        ..Default::default()
    }
    .apply(&mut c);

    let tl = timeline(vec![a.clone(), b.clone(), c.clone()]);
    let solo = any_solo_of(&tl, TrackKind::Audio);
    assert!(solo);
    assert!(
        !is_audible(&a, solo),
        "unsoloed track is silent while solo is on"
    );
    assert!(!is_audible(&b, solo));
    assert!(is_audible(&c, solo));

    // Clearing solo restores exactly the previous picture, muted track included.
    TrackState::default().apply(&mut c);
    let tl = timeline(vec![a.clone(), b.clone(), c.clone()]);
    let solo = any_solo_of(&tl, TrackKind::Audio);
    assert!(!solo);
    assert!(is_audible(&a, solo));
    assert!(
        !is_audible(&b, solo),
        "the mute must have survived the solo"
    );
    assert!(is_audible(&c, solo));
}

/// A disabled track is never heard, whatever else is set: disabling is a
/// statement about the cut and it is the stronger one.
#[test]
fn disabled_beats_solo() {
    let mut t = track("A1", TrackKind::Audio, 1);
    t.enabled = false;
    TrackState {
        solo: true,
        ..Default::default()
    }
    .apply(&mut t);
    assert!(!is_audible(&t, true));
}

/// Soloing a video track must not silence the audio.
#[test]
fn solo_is_per_kind() {
    let mut v = track("V1", TrackKind::Video, 1);
    let a = track("A1", TrackKind::Audio, 1);
    TrackState {
        solo: true,
        ..Default::default()
    }
    .apply(&mut v);
    let tl = timeline(vec![v, a.clone()]);
    assert!(any_solo_of(&tl, TrackKind::Video));
    assert!(!any_solo_of(&tl, TrackKind::Audio));
    assert!(
        is_audible(&a, any_solo_of(&tl, TrackKind::Audio)),
        "soloing picture must not silence sound"
    );
}

#[test]
fn targeting_selects_where_an_insert_goes() {
    let v1 = track("V1", TrackKind::Video, 1);
    let mut v2 = track("V2", TrackKind::Video, 1);
    TrackState {
        targeted: true,
        ..Default::default()
    }
    .apply(&mut v2);
    let tl = timeline(vec![v1, v2]);
    assert_eq!(targeted_tracks(&tl), vec![1]);
}

// ---------------------------------------------------------------------------
// State survives a round trip
// ---------------------------------------------------------------------------

/// Session state lives in OTIO metadata, so it must survive a save and reload —
/// and the file must still be OTIO that another application can read.
#[test]
fn session_state_survives_an_otio_round_trip() {
    let mut v = track("V1", TrackKind::Video, 2);
    TrackState {
        locked: true,
        muted: false,
        solo: true,
        targeted: true,
    }
    .apply(&mut v);
    if let Some(c) = v.children[0].as_clip_mut() {
        set_link(c, "take-7");
    }
    let tl = timeline(vec![v]);

    let text = write_otio(&tl);
    let back = read_otio(&text).expect("round trip");
    assert_eq!(back, tl, "the timeline must round-trip unchanged");

    let track = back.tracks.children[0].as_track().unwrap();
    let state = TrackState::of(track);
    assert!(state.locked && state.solo && state.targeted && !state.muted);
    assert_eq!(
        link_of(track.children[0].as_clip().unwrap()).as_deref(),
        Some("take-7")
    );

    // And it is in our namespace, not invented OTIO schema.
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let meta = &parsed["tracks"]["children"][0]["metadata"][NAMESPACE];
    assert_eq!(meta["locked"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// Linking
// ---------------------------------------------------------------------------

/// Linked clips are found wherever they are, on any track.
#[test]
fn linked_clips_are_found_across_tracks() {
    let mut v = track("V1", TrackKind::Video, 2);
    let mut a = track("A1", TrackKind::Audio, 2);
    set_link(v.children[1].as_clip_mut().unwrap(), "take-3");
    set_link(a.children[1].as_clip_mut().unwrap(), "take-3");
    set_link(a.children[0].as_clip_mut().unwrap(), "take-9");
    let tl = timeline(vec![v, a]);

    assert_eq!(linked_positions(&tl, "take-3"), vec![(0, 1), (1, 1)]);
    assert_eq!(linked_positions(&tl, "take-9"), vec![(1, 0)]);
    assert!(linked_positions(&tl, "nothing").is_empty());
}

#[test]
fn a_link_can_be_broken_without_disturbing_other_metadata() {
    let mut c = clip("A", 0, 48, Some((0, 100)));
    c.metadata
        .insert("OtherApp".into(), serde_json::json!({"keep": 1}));
    set_link(&mut c, "take-1");
    assert_eq!(link_of(&c).as_deref(), Some("take-1"));
    clear_link(&mut c);
    assert_eq!(link_of(&c), None);
    assert_eq!(c.metadata["OtherApp"], serde_json::json!({"keep": 1}));
}

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// Handles are what a trim has to spend.
#[test]
fn handles_are_the_media_either_side_of_what_is_played() {
    let c = clip("A", 30, 48, Some((10, 200)));
    let (head, tail) = handles(&c).expect("a clip with an available range has handles");
    assert_eq!(head, frames(20), "30 - 10");
    assert_eq!(tail, frames(132), "210 - 78");
}

/// A clip with no declared available range cannot answer the question, and
/// "unknown" must not be mistaken for "no room".
#[test]
fn a_clip_without_an_available_range_reports_unknown_handles() {
    let c = clip("A", 30, 48, None);
    assert_eq!(handles(&c), None);
}

// ---------------------------------------------------------------------------
// Snapping
// ---------------------------------------------------------------------------

#[test]
fn snapping_finds_cuts_within_tolerance_and_leaves_the_rest_alone() {
    let v = track("V1", TrackKind::Video, 3); // cuts at 0, 48, 96, end 144
    let tl = timeline(vec![v]);
    let targets = snap_targets(&tl, None);
    assert!(targets.contains(&frames(48)));
    assert!(targets.contains(&frames(144)));

    assert_eq!(snap(frames(50), &targets, frames(4)), Some(frames(48)));
    assert_eq!(snap(frames(44), &targets, frames(4)), Some(frames(48)));
    assert_eq!(
        snap(frames(60), &targets, frames(4)),
        None,
        "nothing within tolerance must not snap"
    );
    assert_eq!(snapped(frames(60), &targets, frames(4)), frames(60));
}

/// A tie goes to the earlier target, so dragging left and dragging right onto
/// the same midpoint agree.
#[test]
fn a_tie_snaps_to_the_earlier_target() {
    let targets = vec![frames(40), frames(50)];
    assert_eq!(snap(frames(45), &targets, frames(10)), Some(frames(40)));
}

/// An item does not snap to where it already is.
#[test]
fn the_dragged_track_can_be_excluded_from_its_own_targets() {
    let v = track("V1", TrackKind::Video, 2); // cuts at 0, 48, 96
    let mut a = Track::new("A1", TrackKind::Audio);
    // A different length on purpose: if both tracks cut at 48, excluding one
    // proves nothing.
    a.children
        .push(Item::Clip(clip("A1-0", 0, 30, Some((0, 200)))));
    let tl = timeline(vec![v, a]);
    let all = snap_targets(&tl, None);
    let without = snap_targets(&tl, Some(0));
    assert!(all.contains(&frames(48)));
    assert!(
        !without.contains(&frames(48)),
        "the excluded track's cuts must not be targets"
    );
    assert!(
        without.contains(&frames(30)),
        "the other track still offers its own"
    );
}

/// Snapping is exact across rates: a 23.976 cut is a legitimate target for a
/// 24 fps playhead, and the snapped value keeps its own exact rate.
#[test]
fn snapping_is_exact_across_rates() {
    let target = RationalTime::from_frames(100, 23.976);
    let targets = vec![target];
    let near = RationalTime::from_frames(100, 24.0);
    let tolerance = RationalTime::from_frames(1, 24.0);
    let snapped_to = snap(near, &targets, tolerance).expect("within a frame");
    assert_eq!(snapped_to, target);
    assert_eq!(
        snapped_to.rate_exact(),
        target.rate_exact(),
        "the target keeps its own rate rather than being requantised"
    );
}

// ---------------------------------------------------------------------------
// Replace
// ---------------------------------------------------------------------------

/// A replace keeps the track's shape: the new clip plays for exactly as long as
/// the old one and nothing after it moves.
#[test]
fn replace_keeps_the_duration_and_moves_nothing() {
    let mut t = track("V1", TrackKind::Video, 3);
    let before = t.duration();
    let third_start = t.range_of_child(2).unwrap().start_time;

    replace(&mut t, 1, Item::Clip(clip("new", 90, 4, Some((0, 400))))).unwrap();

    assert_eq!(t.duration(), before, "the track length must not change");
    assert_eq!(t.range_of_child(2).unwrap().start_time, third_start);
    let replaced = t.children[1].as_clip().unwrap();
    assert_eq!(replaced.name, "new");
    assert_eq!(replaced.source_range.unwrap().duration, frames(48));
    assert_eq!(
        replaced.source_range.unwrap().start_time,
        frames(90),
        "it plays from where the replacement said, for as long as the hole needs"
    );
}

/// A replacement that does not have enough media is refused rather than
/// leaving a hole.
#[test]
fn a_replacement_with_too_little_media_is_refused() {
    let mut t = track("V1", TrackKind::Video, 2);
    let before = t.clone();
    let err = replace(&mut t, 0, Item::Clip(clip("short", 90, 4, Some((90, 10)))))
        .expect_err("not enough media must be refused");
    assert!(err.to_string().contains("needed to fill the hole"), "{err}");
    assert_eq!(t, before, "a refused replace must change nothing");
}

// ---------------------------------------------------------------------------
// Surviving a project save
// ---------------------------------------------------------------------------

/// Session state must survive the *project*, not only an OTIO file. Track state
/// and clip links live in metadata, and the graph persists metadata, so this
/// asserts what that actually amounts to end to end: import into the graph,
/// export back, and find the locks and links where they were left.
#[test]
fn session_state_survives_the_project_graph() {
    use ucg_core::{Engine, EntityId, UcgError};
    use ucg_schema::kinds;

    let mut v = track("V1", TrackKind::Video, 2);
    TrackState {
        locked: true,
        muted: true,
        solo: false,
        targeted: true,
    }
    .apply(&mut v);
    set_link(v.children[0].as_clip_mut().unwrap(), "take-42");
    let mut a = track("A1", TrackKind::Audio, 1);
    TrackState {
        solo: true,
        ..Default::default()
    }
    .apply(&mut a);
    set_link(a.children[0].as_clip_mut().unwrap(), "take-42");
    let tl = timeline(vec![v, a]);

    let engine = Engine::new();
    let project = EntityId::from_key("project:session");
    let mut id = EntityId::nil();
    engine
        .transact("import", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            id = cine_timeline::import_timeline(tx, project, &tl, "session.otio")
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();

    let back = cine_timeline::export_timeline(&engine.read(), id).expect("export");
    let video = back.tracks.children[0].as_track().unwrap();
    let audio = back.tracks.children[1].as_track().unwrap();

    let vs = TrackState::of(video);
    assert!(vs.locked, "the lock must survive a project save");
    assert!(vs.muted && vs.targeted && !vs.solo);
    assert!(TrackState::of(audio).solo);

    assert_eq!(linked_positions(&back, "take-42").len(), 2, "the take is still paired");
}
