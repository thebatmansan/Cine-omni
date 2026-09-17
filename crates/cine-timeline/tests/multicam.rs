//! Multicam: several angles of the same take, and cutting between them.
//!
//! An angle is not a new kind of thing. A clip already carries several media
//! references with one of them active, which is exactly what a multicam clip
//! is, so the work is making the graph hold all of them and making a switch an
//! edit. What is tested here is that: the angles survive the graph and the
//! OTIO file, a switch cuts at the point it was asked for and leaves the
//! material before it alone, and an angle that does not have the media is
//! refused by name rather than cut to and found black later.

use cine_timeline::graph::{
    add_angles, angle_of_clip, angles_of_clip, create_sequence, export_track, media_of_clip,
    place_material, set_clip_angle, sibling_tracks, switch_angle, ClipSpec,
};
use cine_timeline::prelude::*;
use ucg_core::{Engine, EntityId, Rate, UcgError};
use ucg_schema::{attrs, kinds};

fn f(n: i64) -> RationalTime {
    RationalTime::from_frames(n, 24.0)
}

/// Three cameras on the same take. `b_from` is where angle B started rolling,
/// so one of them can be short.
fn setup(b_from: i64) -> (Engine, EntityId, Vec<EntityId>) {
    let engine = Engine::new();
    let project = EntityId::from_key("project:test");
    let mut seq = EntityId::nil();
    let angles = [
        ("A", "/footage/A001.mov", 0i64, 240i64),
        ("B", "/footage/B001.mov", b_from, 240 - b_from),
        ("C", "/footage/C001.mov", 0, 240),
    ];
    let mut ids = Vec::new();
    for (name, path, start, len) in angles {
        ids.push(EntityId::from_key(&format!("media:{path}")));
        let id = *ids.last().unwrap();
        engine
            .transact("ingest", |tx| {
                tx.create_with_id(id, kinds::ASSET_MEDIA)?;
                tx.set(id, attrs::NAME, name)?;
                tx.set(id, attrs::MEDIA_PATH, path)?;
                tx.set(
                    id,
                    attrs::AVAILABLE_RANGE,
                    TimeRange::from_frames(start, len, 24.0),
                )?;
                Ok(())
            })
            .unwrap();
    }
    engine
        .transact("sequence", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            seq = create_sequence(tx, project, "Multicam", Rate::whole(24).unwrap())
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    let stack = g.children(seq)[0];
    let first = g
        .children(stack)
        .into_iter()
        .find(|c| g.node(*c).unwrap().kind.as_str() == kinds::NLE_TRACK)
        .unwrap();
    let video = sibling_tracks(&g, first).unwrap()[0];
    drop(g);

    // 96 frames of angle A on the track.
    engine
        .transact("place", |tx| {
            place_material(
                tx,
                &[video],
                f(0),
                &ClipSpec {
                    media: ids[0],
                    source_range: TimeRange::from_frames(0, 96, 24.0),
                    name: None,
                },
                false,
            )
            .map(|_| ())
            .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    (engine, video, ids)
}

fn attach(engine: &Engine, clip: EntityId, keys: &[(&str, EntityId)]) -> Result<()> {
    let pairs: Vec<(String, EntityId)> = keys.iter().map(|(k, a)| (k.to_string(), *a)).collect();
    engine
        .transact("angles", |tx| {
            add_angles(tx, clip, &pairs).map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .map(|_| ())
        .map_err(|e| Error::Edit(e.to_string()))
}

/// Angles reach the graph as assets in their own right, bound to the clip, and
/// come back as the clip's own references.
#[test]
fn a_clip_can_carry_several_angles_and_the_graph_holds_them_all() {
    let (engine, video, ids) = setup(0);
    let clip = engine.read().children(video)[0];
    attach(&engine, clip, &[("B", ids[1]), ("C", ids[2])]).expect("attached");

    let g = engine.read();
    assert_eq!(angles_of_clip(&g, clip).len(), 3, "A, B and C");
    assert_eq!(angle_of_clip(&g, clip).as_deref(), Some("DEFAULT_MEDIA"));
    assert_eq!(
        media_of_clip(&g, clip),
        Some(ids[0]),
        "the asset the clip plays is the active angle's, not whichever edge came first"
    );
    // Every angle is bound, so a change to any of them reaches the cut.
    let bound = g.targets(clip, ucg_core::EdgeKind::Binds);
    for id in &ids {
        assert!(bound.contains(id), "angle {id} is bound");
    }
    let track = export_track(&g, video).unwrap();
    let c = track.children[0].as_clip().unwrap();
    assert_eq!(c.alternate_media_references.len(), 2);
    assert_eq!(
        c.all_media_references().len(),
        3,
        "and the model sees all three"
    );
}

/// Writing the same track again changes nothing. A writer that produced ops
/// here would make the reactors churn for ever.
#[test]
fn rewriting_a_multicam_clip_produces_no_ops() {
    let (engine, video, ids) = setup(0);
    let clip = engine.read().children(video)[0];
    attach(&engine, clip, &[("B", ids[1]), ("C", ids[2])]).expect("attached");

    let track = export_track(&engine.read(), video).unwrap();
    let group = engine
        .transact("rewrite", |tx| {
            cine_timeline::graph::export_track(tx.graph(), video)
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            cine_timeline::graph::apply_edit(
                tx,
                video,
                cine_timeline::EditOp::Slip {
                    index: 0,
                    delta: RationalTime::from_frames(0, 24.0),
                },
            )
            .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    assert!(
        group.deltas.is_empty(),
        "an edit that changes nothing wrote {:?}",
        group.deltas
    );
    assert_eq!(
        export_track(&engine.read(), video).unwrap(),
        track,
        "and the track is unchanged"
    );
}

/// A switch is a cut: split where it was asked for, play the new angle from
/// there, and leave the material before it playing what it was playing.
#[test]
fn switching_at_a_point_cuts_there_and_leaves_the_earlier_material_alone() {
    let (engine, video, ids) = setup(0);
    let clip = engine.read().children(video)[0];
    attach(&engine, clip, &[("B", ids[1]), ("C", ids[2])]).expect("attached");

    let switched = engine
        .transact("switch", |tx| {
            switch_angle(tx, video, f(24), "B")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect("switched");
    assert!(!switched.deltas.is_empty());

    let g = engine.read();
    let track = export_track(&g, video).unwrap();
    assert_eq!(track.children.len(), 2, "one switch is one new event");
    assert_eq!(track.duration(), f(96), "and the cut is the same length");
    let (a, b) = (
        track.children[0].as_clip().unwrap(),
        track.children[1].as_clip().unwrap(),
    );
    assert_eq!(a.active_media_reference_key, "DEFAULT_MEDIA");
    assert_eq!(a.source_range, Some(TimeRange::from_frames(0, 24, 24.0)));
    assert_eq!(b.active_media_reference_key, "B");
    assert_eq!(
        b.source_range,
        Some(TimeRange::from_frames(24, 72, 24.0)),
        "the same moment of the take, from another camera"
    );
    assert_eq!(
        b.media_reference.target_url(),
        Some("/footage/B001.mov"),
        "and it plays that camera's file"
    );
    assert_eq!(
        b.all_media_references().len(),
        3,
        "the angle it left is still an angle: switching back is symmetric"
    );

    // Switching again at the same point does not cut again.
    drop(g);
    engine
        .transact("switch back", |tx| {
            switch_angle(tx, video, f(24), "DEFAULT_MEDIA")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect("switched");
    let track = export_track(&engine.read(), video).unwrap();
    assert_eq!(track.children.len(), 2, "still two events");
    assert_eq!(
        track.children[1]
            .as_clip()
            .unwrap()
            .active_media_reference_key,
        "DEFAULT_MEDIA"
    );
}

/// An angle that was not rolling yet is refused by name, with how much is
/// missing — not cut to and discovered black in a screening.
#[test]
fn an_angle_without_the_media_is_refused_with_the_shortfall() {
    // B started rolling 48 frames into the take.
    let (engine, video, ids) = setup(48);
    let clip = engine.read().children(video)[0];
    attach(&engine, clip, &[("B", ids[1])]).expect("attached");

    let e = engine
        .transact("switch", |tx| {
            switch_angle(tx, video, f(0), "B")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect_err("B was not rolling yet");
    assert!(e.to_string().contains("48 frames"), "{e}");
    let track = export_track(&engine.read(), video).unwrap();
    assert_eq!(track.children.len(), 1, "and nothing was cut");
    assert_eq!(
        track.children[0]
            .as_clip()
            .unwrap()
            .active_media_reference_key,
        "DEFAULT_MEDIA"
    );

    // From frame 48 on, B does have the media.
    engine
        .transact("switch", |tx| {
            switch_angle(tx, video, f(48), "B")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect("B is rolling by then");
    let track = export_track(&engine.read(), video).unwrap();
    assert_eq!(
        track.children[1]
            .as_clip()
            .unwrap()
            .active_media_reference_key,
        "B"
    );

    // A key nobody defined is refused as such.
    let e = engine
        .transact("switch", |tx| {
            switch_angle(tx, video, f(60), "Z")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect_err("no angle Z");
    assert!(
        e.to_string().contains("not one of this clip's references"),
        "{e}"
    );
}

/// Switching one clip without cutting, and undoing it.
#[test]
fn an_angle_can_be_changed_on_one_clip_and_undone() {
    let (engine, video, ids) = setup(0);
    let clip = engine.read().children(video)[0];
    attach(&engine, clip, &[("C", ids[2])]).expect("attached");
    engine
        .transact("angle", |tx| {
            set_clip_angle(tx, clip, "C").map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect("switched");
    let g = engine.read();
    assert_eq!(angle_of_clip(&g, clip).as_deref(), Some("C"));
    assert_eq!(media_of_clip(&g, clip), Some(ids[2]));
    drop(g);

    engine.undo().unwrap();
    let g = engine.read();
    assert_eq!(angle_of_clip(&g, clip).as_deref(), Some("DEFAULT_MEDIA"));
    assert_eq!(media_of_clip(&g, clip), Some(ids[0]));
}

/// The angles survive the OTIO file, which is where another tool would read
/// them.
#[test]
fn angles_survive_an_otio_round_trip() {
    let (engine, video, ids) = setup(0);
    let clip = engine.read().children(video)[0];
    attach(&engine, clip, &[("B", ids[1]), ("C", ids[2])]).expect("attached");
    let track = export_track(&engine.read(), video).unwrap();

    let mut timeline = Timeline::new("Multicam");
    timeline.tracks.children.push(Composable::Track(track));
    let text = write_otio(&timeline);
    assert!(text.contains("\"B\""), "{text}");
    let back = read_otio(&text).expect("read");
    let c = back.tracks().next().unwrap().children[0].as_clip().unwrap();
    assert_eq!(c.all_media_references().len(), 3);
    assert_eq!(c.active_media_reference_key, "DEFAULT_MEDIA");
    assert_eq!(back, timeline, "and nothing else moved");
}

/// Camera B started rolling two seconds after A. With that offset recorded, a
/// switch lands on the same moment of the take in B's own media, and the view
/// of every angle at an instant says which picture each shows or why none.
#[test]
fn offsets_put_every_angle_on_the_same_moment_of_the_take() {
    use cine_timeline::graph::{angles_at, set_angle_offsets};
    let (engine, video, ids) = setup(0);
    let clip = engine.read().children(video)[0];
    attach(&engine, clip, &[("B", ids[1]), ("C", ids[2])]).expect("attached");
    // B's media is 48 frames behind the take's: moment 100 of A is 52 of B.
    engine
        .transact("sync", |tx| {
            set_angle_offsets(tx, clip, &[("B".into(), f(-48))])
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();

    // What each angle shows ten frames into the cut.
    let seq = engine
        .read()
        .parent(engine.read().parent(video).unwrap())
        .unwrap();
    let (at_clip, angles) = angles_at(&engine.read(), seq, f(10))
        .unwrap()
        .expect("a multicam clip");
    assert_eq!(at_clip, clip);
    let by = |k: &str| angles.iter().find(|a| a.key == k).unwrap().clone();
    assert!(by("DEFAULT_MEDIA").active);
    assert_eq!(by("DEFAULT_MEDIA").media, Some(f(10)));
    assert_eq!(by("C").media, Some(f(10)), "C shares A's clock");
    assert_eq!(by("B").media, None, "B was not rolling ten frames in");
    assert!(
        by("B")
            .why_not
            .as_deref()
            .unwrap()
            .contains("starts 38 frames later"),
        "{:?}",
        by("B").why_not
    );
    assert_eq!(by("B").asset, Some(ids[1]));

    // A switch to B at frame 60 plays B's picture 12, the same moment.
    engine
        .transact("switch", |tx| {
            switch_angle(tx, video, f(60), "B")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect("B is rolling by then");
    let track = export_track(&engine.read(), video).unwrap();
    let b = track.children[1].as_clip().unwrap();
    assert_eq!(b.active_media_reference_key, "B");
    assert_eq!(
        b.source_range,
        Some(TimeRange::from_frames(12, 36, 24.0)),
        "the take's frame 60 is B's frame 12"
    );
    // And switching back puts it back on A's clock exactly.
    engine
        .transact("back", |tx| {
            switch_angle(tx, video, f(60), "DEFAULT_MEDIA")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let track = export_track(&engine.read(), video).unwrap();
    assert_eq!(
        track.children[1].as_clip().unwrap().source_range,
        Some(TimeRange::from_frames(60, 36, 24.0))
    );

    // Before B rolled, switching to it is refused with how far off it is.
    let e = engine
        .transact("too early", |tx| {
            switch_angle(tx, video, f(24), "B")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect_err("B had not started");
    assert!(e.to_string().contains("24 frames"), "{e}");

    // An angle the clip does not carry cannot be given an offset.
    let e = engine
        .transact("bad key", |tx| {
            set_angle_offsets(tx, clip, &[("Z".into(), f(1))])
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect_err("no angle Z");
    assert!(
        e.to_string().contains("not one of this clip's angles"),
        "{e}"
    );
}
