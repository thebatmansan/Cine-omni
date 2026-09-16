//! Three- and four-point editing against a real project graph.
//!
//! The arithmetic is unit-tested in the module; what is tested here is that
//! the marks reach the graph — that the media the plan chose is the media the
//! clip plays, that the asset's own available range is what the plan is
//! checked against, and that a refusal leaves the timeline untouched.

use cine_timeline::graph::{create_sequence, export_track, sibling_tracks, three_point_edit};
use cine_timeline::link_of;
use cine_timeline::prelude::*;
use cine_timeline::three_point::{EditKind, Marks, Mismatch};
use ucg_core::{EdgeKind, Engine, EntityId, Rate, UcgError};
use ucg_schema::{attrs, kinds};

fn f(n: i64) -> RationalTime {
    RationalTime::from_frames(n, 24.0)
}

/// A project with one empty sequence and one 240-frame asset.
fn setup() -> (Engine, Vec<EntityId>, EntityId) {
    let engine = Engine::new();
    let project = EntityId::from_key("project:test");
    let asset = EntityId::from_key("media:file:shot");
    let mut seq = EntityId::nil();
    engine
        .transact("setup", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            tx.create_with_id(asset, kinds::ASSET_MEDIA)?;
            tx.set(asset, attrs::NAME, "A001C001.mov")?;
            tx.set(asset, attrs::MEDIA_PATH, "/footage/A001C001.mov")?;
            tx.set(
                asset,
                attrs::AVAILABLE_RANGE,
                TimeRange::from_frames(0, 240, 24.0),
            )?;
            seq = create_sequence(tx, project, "Assembly", Rate::whole(24).unwrap())
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
    let tracks = sibling_tracks(&g, first).unwrap();
    drop(g);
    (engine, tracks, asset)
}

fn run(
    engine: &Engine,
    tracks: &[EntityId],
    asset: EntityId,
    source: Marks,
    record: Marks,
    kind: EditKind,
    on_mismatch: Mismatch,
) -> Result<cine_timeline::three_point::Plan> {
    let mut out = None;
    let result = engine.transact("three-point", |tx| {
        let (plan, _) = three_point_edit(tx, tracks, asset, source, record, kind, on_mismatch)
            .map_err(|e| UcgError::Aborted(e.to_string()))?;
        out = Some(plan);
        Ok(())
    });
    match result {
        Ok(_) => Ok(out.expect("a plan")),
        Err(e) => Err(Error::Edit(e.to_string())),
    }
}

/// The classic edit: marks in the source, the playhead in the record.
#[test]
fn a_three_point_edit_lands_the_marked_media_at_the_marked_time() {
    let (engine, tracks, asset) = setup();
    let video = tracks[0];
    let plan = run(
        &engine,
        &[video],
        asset,
        Marks::range(f(48), f(96)),
        Marks::starting_at(f(0)),
        EditKind::Insert,
        Mismatch::Refuse,
    )
    .expect("a plan");
    assert_eq!(plan.source, TimeRange::from_frames(48, 48, 24.0));

    let g = engine.read();
    let track = export_track(&g, video).unwrap();
    assert_eq!(
        track.children[0].source_range(),
        Some(TimeRange::from_frames(48, 48, 24.0)),
        "the media that plays is the media that was marked"
    );
    assert_eq!(
        track.range_of_child(0),
        Some(TimeRange::from_frames(0, 48, 24.0))
    );
    let clip = g.children(video)[0];
    assert_eq!(
        g.targets(clip, EdgeKind::Binds),
        vec![asset],
        "and it is bound to the asset the project already holds"
    );
}

/// Backtiming: the edit is marked by where it must end, and it ends there.
#[test]
fn a_backtimed_edit_ends_on_its_mark() {
    let (engine, tracks, asset) = setup();
    let video = tracks[0];
    // Something to overwrite: 120 frames of the shot from the top.
    run(
        &engine,
        &[video],
        asset,
        Marks::range(f(0), f(120)),
        Marks::starting_at(f(0)),
        EditKind::Insert,
        Mismatch::Refuse,
    )
    .expect("a plan");

    let plan = run(
        &engine,
        &[video],
        asset,
        Marks::range(f(200), f(224)),
        Marks::ending_at(f(96)),
        EditKind::Overwrite,
        Mismatch::Refuse,
    )
    .expect("a plan");
    assert!(plan.backtimed);
    assert_eq!(plan.record, TimeRange::from_frames(72, 24, 24.0));

    let g = engine.read();
    let track = export_track(&g, video).unwrap();
    assert_eq!(track.duration(), f(120), "an overwrite moved nothing");
    let at = track
        .children
        .iter()
        .position(|c| c.source_range() == Some(TimeRange::from_frames(200, 24, 24.0)))
        .expect("the backtimed clip");
    assert_eq!(
        track.range_of_child(at),
        Some(TimeRange::from_frames(72, 24, 24.0)),
        "it ends on the mark at frame 96"
    );
}

/// The asset's own available range is the bound, and a refusal is a refusal:
/// the timeline is not half-edited.
#[test]
fn an_edit_past_the_end_of_the_asset_changes_nothing() {
    let (engine, tracks, asset) = setup();
    let video = tracks[0];
    let before = engine.stats().nodes;
    let e = run(
        &engine,
        &[video],
        asset,
        Marks::starting_at(f(220)),
        Marks::range(f(0), f(48)),
        EditKind::Insert,
        Mismatch::Refuse,
    )
    .expect_err("the shot is only 240 frames long");
    assert!(e.to_string().contains("28 frames past the end"), "{e}");
    let g = engine.read();
    assert_eq!(engine.stats().nodes, before, "nothing was created");
    assert!(export_track(&g, video).unwrap().children.is_empty());
}

/// Four marks that disagree: refused by name, and settled only when asked.
#[test]
fn four_points_are_refused_until_the_editor_says_how_to_settle_them() {
    let (engine, tracks, asset) = setup();
    let video = tracks[0];
    let e = run(
        &engine,
        &[video],
        asset,
        Marks::range(f(48), f(96)),
        Marks::range(f(0), f(24)),
        EditKind::Insert,
        Mismatch::Refuse,
    )
    .expect_err("four marks disagree");
    assert!(e.to_string().contains("24 frames longer"), "{e}");

    let plan = run(
        &engine,
        &[video],
        asset,
        Marks::range(f(48), f(96)),
        Marks::range(f(0), f(24)),
        EditKind::Insert,
        Mismatch::TrimSourceTail,
    )
    .expect("settled");
    assert_eq!(plan.source, TimeRange::from_frames(48, 24, 24.0));
    assert!(plan.reconciled.is_some());
    let g = engine.read();
    assert_eq!(
        export_track(&g, video).unwrap().range_of_child(0),
        Some(TimeRange::from_frames(0, 24, 24.0))
    );
}

/// Picture and sound go down together, linked, from one set of marks.
#[test]
fn one_set_of_marks_places_picture_and_sound_as_a_take() {
    let (engine, tracks, asset) = setup();
    run(
        &engine,
        &tracks,
        asset,
        Marks::range(f(0), f(48)),
        Marks::starting_at(f(0)),
        EditKind::Insert,
        Mismatch::Refuse,
    )
    .expect("a plan");
    let g = engine.read();
    let groups: Vec<Option<String>> = tracks
        .iter()
        .map(|t| {
            let track = export_track(&g, *t).unwrap();
            track.children[0].as_clip().and_then(link_of)
        })
        .collect();
    assert_eq!(groups.len(), 2);
    assert!(groups[0].is_some(), "the picture is in a take group");
    assert_eq!(groups[0], groups[1], "and the sound is in the same one");
}
