//! Edits across tracks: linked material moves together, the group agrees on
//! one delta, locks refuse the whole edit, and new material binds to the asset
//! the project already knows.

use cine_timeline::graph::{perform_edit, set_clip_link, sibling_tracks, ClipSpec, EditOp};
use cine_timeline::prelude::*;
use cine_timeline::{apply_linked, export_track, set_link, TrackEdit, TrackState};
use ucg_core::{EdgeKind, Engine, EntityId, UcgError};
use ucg_schema::{attrs, kinds};

const FIXTURE: &str = include_str!("../../../tests/fixtures/sample.otio");
const KEY: &str = "demo.otio";

fn f(n: i64) -> RationalTime {
    RationalTime::from_frames(n, 24.0)
}

fn clip(name: &str, available: i64, dur: i64) -> Clip {
    Clip::external(
        name,
        format!("file:///{name}"),
        Some(TimeRange::from_frames(0, available, 24.0)),
        Some(TimeRange::from_frames(0, dur, 24.0)),
    )
}

fn linked_pair() -> Vec<Track> {
    let mut v = clip("v.mov", 100, 48);
    let mut a = clip("a.wav", 60, 48);
    set_link(&mut v, "take-1");
    set_link(&mut a, "take-1");
    vec![
        Track::new("V1", TrackKind::Video)
            .with(v)
            .with(clip("v2.mov", 100, 24)),
        Track::new("A1", TrackKind::Audio)
            .with(a)
            .with(clip("a2.wav", 100, 24)),
    ]
}

#[test]
fn a_trim_carries_to_linked_sound() {
    let mut tracks = linked_pair();
    let out = apply_linked(
        &mut tracks,
        0,
        TrackEdit::RippleTrim {
            index: 0,
            edge: Edge::Tail,
            delta: f(6),
        },
        true,
    )
    .unwrap();
    assert_eq!(out.applied, Some(f(6)));
    assert_eq!(out.changed_tracks, vec![0, 1]);
    assert_eq!(out.linked_clips, 1);
    for t in &tracks {
        assert_eq!(
            t.range_of_child(1),
            Some(TimeRange::from_frames(54, 24, 24.0))
        );
    }
}

#[test]
fn the_group_takes_the_smallest_trim_any_member_allows() {
    // Picture has 52 frames of tail handle, sound only 12. Asking for 40 must
    // trim both by 12, not picture by 40 and sound by 12.
    let mut tracks = linked_pair();
    let out = apply_linked(
        &mut tracks,
        0,
        TrackEdit::RippleTrim {
            index: 0,
            edge: Edge::Tail,
            delta: f(40),
        },
        true,
    )
    .unwrap();
    assert_eq!(out.requested, Some(f(40)));
    assert_eq!(out.applied, Some(f(12)));
    assert_eq!(tracks[0].children[0].duration(), f(60));
    assert_eq!(tracks[1].children[0].duration(), f(60));
}

#[test]
fn without_links_only_the_primary_moves() {
    let mut tracks = linked_pair();
    let out = apply_linked(
        &mut tracks,
        0,
        TrackEdit::RippleTrim {
            index: 0,
            edge: Edge::Tail,
            delta: f(40),
        },
        false,
    )
    .unwrap();
    assert_eq!(out.applied, Some(f(40)));
    assert_eq!(out.changed_tracks, vec![0]);
    assert_eq!(tracks[1].children[0].duration(), f(48));
}

#[test]
fn a_locked_track_holding_linked_material_refuses_the_whole_edit() {
    let mut tracks = linked_pair();
    TrackState {
        locked: true,
        ..Default::default()
    }
    .apply(&mut tracks[1]);
    let before = tracks.clone();
    let err = apply_linked(
        &mut tracks,
        0,
        TrackEdit::Slip {
            index: 0,
            delta: f(2),
        },
        true,
    )
    .unwrap_err();
    assert!(err.to_string().contains("A1"), "{err}");
    assert_eq!(tracks, before, "nothing changed");
}

#[test]
fn split_and_extract_carry_to_linked_material() {
    let mut tracks = linked_pair();
    apply_linked(&mut tracks, 0, TrackEdit::SplitAt { time: f(20) }, true).unwrap();
    assert_eq!(tracks[0].children.len(), 3);
    assert_eq!(tracks[1].children.len(), 3);

    let out = apply_linked(
        &mut tracks,
        0,
        TrackEdit::Extract {
            range: TimeRange::from_frames(0, 20, 24.0),
        },
        true,
    )
    .unwrap();
    assert_eq!(out.changed_tracks, vec![0, 1]);
    assert_eq!(tracks[0].duration(), f(52));
    assert_eq!(tracks[1].duration(), f(52));
}

#[test]
fn insert_keeps_linked_sound_in_sync() {
    let mut tracks = linked_pair();
    let new = Item::Clip(clip("insert.mov", 100, 10));
    apply_linked(
        &mut tracks,
        0,
        TrackEdit::Insert {
            time: f(0),
            item: new,
        },
        true,
    )
    .unwrap();
    // Picture: the insert, then the take. Sound: a gap of the same length, then the take.
    assert_eq!(
        tracks[0].range_of_child(1),
        Some(TimeRange::from_frames(10, 48, 24.0))
    );
    assert_eq!(
        tracks[1].range_of_child(1),
        Some(TimeRange::from_frames(10, 48, 24.0))
    );
    assert!(matches!(tracks[1].children[0], Item::Gap(_)));
}

#[test]
fn replace_and_join_do_not_move_linked_material() {
    let mut tracks = linked_pair();
    apply_linked(&mut tracks, 0, TrackEdit::SplitAt { time: f(20) }, false).unwrap();
    let out = apply_linked(&mut tracks, 0, TrackEdit::Join { index: 0 }, true).unwrap();
    assert_eq!(out.changed_tracks, vec![0]);
    assert_eq!(tracks[0].children.len(), 2);
    assert_eq!(tracks[0].children[0].duration(), f(48));

    let out = apply_linked(
        &mut tracks,
        0,
        TrackEdit::Replace {
            index: 1,
            item: Item::Clip(clip("r.mov", 100, 99)),
        },
        true,
    )
    .unwrap();
    assert_eq!(out.changed_tracks, vec![0]);
    assert_eq!(
        tracks[0].children[1].duration(),
        f(24),
        "replace keeps the hole's length"
    );
}

// ---------------------------------------------------------------------------
// through the graph
// ---------------------------------------------------------------------------

fn setup() -> (Engine, Vec<EntityId>) {
    let engine = Engine::new();
    let project = EntityId::from_key("project:test");
    let tl = read_otio(FIXTURE).unwrap();
    let mut tl_id = EntityId::nil();
    engine
        .transact("import", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            tl_id = import_timeline(tx, project, &tl, KEY)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    let stack = g.children(tl_id)[0];
    let v1 = g
        .children(stack)
        .into_iter()
        .find(|c| g.node(*c).unwrap().kind.as_str() == kinds::NLE_TRACK)
        .unwrap();
    let tracks = sibling_tracks(&g, v1).unwrap();
    drop(g);
    (engine, tracks)
}

fn id(ti: usize, ii: usize) -> EntityId {
    EntityId::from_key(&format!("nle:{KEY}:{ti}:{ii}"))
}

fn aborted(e: cine_timeline::Error) -> UcgError {
    UcgError::Aborted(e.to_string())
}

#[test]
fn a_linked_graph_edit_is_one_undo_step() {
    let (engine, tracks) = setup();
    assert_eq!(tracks.len(), 4);
    let (v1, a1) = (tracks[0], tracks[2]);
    engine
        .transact("link", |tx| {
            set_clip_link(tx, id(0, 2), Some("A001_C007")).map_err(aborted)?;
            set_clip_link(tx, id(2, 1), Some("A001_C007")).map_err(aborted)
        })
        .unwrap();
    let before_v = export_track(&engine.read(), v1).unwrap();
    let before_a = export_track(&engine.read(), a1).unwrap();

    let mut out = None;
    engine
        .transact("trim", |tx| {
            out = Some(
                perform_edit(
                    tx,
                    v1,
                    EditOp::RippleTrim {
                        index: 2,
                        edge: Edge::Tail,
                        delta: f(-12),
                    },
                    true,
                )
                .map_err(aborted)?,
            );
            Ok(())
        })
        .unwrap();
    let out = out.unwrap();
    assert_eq!(out.label, "ripple trim");
    assert_eq!(out.applied, Some(f(-12)));
    assert_eq!(out.changed_tracks, vec![v1, a1]);
    assert_eq!(out.item, Some(id(0, 2)));
    {
        let g = engine.read();
        assert_eq!(export_track(&g, v1).unwrap().children[2].duration(), f(108));
        assert_eq!(export_track(&g, a1).unwrap().children[1].duration(), f(108));
    }

    engine.undo().unwrap();
    let g = engine.read();
    assert_eq!(export_track(&g, v1).unwrap(), before_v);
    assert_eq!(export_track(&g, a1).unwrap(), before_a);
}

#[test]
fn inserted_material_binds_to_the_ingested_asset() {
    let (engine, tracks) = setup();
    let v2 = tracks[1];
    let asset = EntityId::from_key("media:file:abc123");
    engine
        .transact("ingest", |tx| {
            tx.create_with_id(asset, kinds::ASSET_MEDIA)?;
            tx.set(asset, attrs::NAME, "shot.mp4")?;
            tx.set(asset, attrs::MEDIA_PATH, "/footage/shot.mp4")?;
            tx.set(
                asset,
                attrs::AVAILABLE_RANGE,
                TimeRange::from_frames(0, 240, 24.0),
            )?;
            Ok(())
        })
        .unwrap();
    let assets_before = engine.read().of_kind(kinds::ASSET_MEDIA).len();

    let mut out = None;
    engine
        .transact("insert", |tx| {
            out = Some(
                perform_edit(
                    tx,
                    v2,
                    EditOp::Insert {
                        time: f(0),
                        clip: ClipSpec {
                            media: asset,
                            source_range: TimeRange::from_frames(10, 24, 24.0),
                            name: None,
                        },
                    },
                    true,
                )
                .map_err(aborted)?,
            );
            Ok(())
        })
        .unwrap();
    let new_clip = out.unwrap().item.expect("the inserted clip is named");
    let g = engine.read();
    assert_eq!(g.targets(new_clip, EdgeKind::Binds), vec![asset]);
    assert_eq!(
        g.of_kind(kinds::ASSET_MEDIA).len(),
        assets_before,
        "no second identity"
    );
    assert_eq!(
        g.attr(asset, attrs::NAME).and_then(|v| v.as_str()),
        Some("shot.mp4")
    );
    let track = export_track(&g, v2).unwrap();
    assert_eq!(track.children[0].name(), "shot.mp4");
    assert_eq!(
        track.range_of_child(1),
        Some(TimeRange::from_frames(24, 120, 24.0))
    );
    drop(g);

    // A later edit rewrites the clip and must keep the same binding.
    engine
        .transact("slip", |tx| {
            perform_edit(
                tx,
                v2,
                EditOp::Slip {
                    index: 0,
                    delta: f(4),
                },
                true,
            )
            .map(|_| ())
            .map_err(aborted)
        })
        .unwrap();
    let g = engine.read();
    assert_eq!(g.targets(new_clip, EdgeKind::Binds), vec![asset]);
    assert_eq!(g.of_kind(kinds::ASSET_MEDIA).len(), assets_before);
}

#[test]
fn material_outside_the_media_is_refused() {
    let (engine, tracks) = setup();
    let asset = EntityId::from_key("media:file:short");
    engine
        .transact("ingest", |tx| {
            tx.create_with_id(asset, kinds::ASSET_MEDIA)?;
            tx.set(asset, attrs::MEDIA_PATH, "/footage/short.mp4")?;
            tx.set(
                asset,
                attrs::AVAILABLE_RANGE,
                TimeRange::from_frames(0, 10, 24.0),
            )?;
            Ok(())
        })
        .unwrap();
    let rev = engine.revision();
    let res = engine.transact("overwrite", |tx| {
        perform_edit(
            tx,
            tracks[0],
            EditOp::Overwrite {
                time: f(0),
                clip: ClipSpec {
                    media: asset,
                    source_range: TimeRange::from_frames(0, 20, 24.0),
                    name: None,
                },
            },
            true,
        )
        .map(|_| ())
        .map_err(aborted)
    });
    assert!(res.unwrap_err().to_string().contains("outside the media"));
    assert_eq!(engine.revision(), rev);
}

#[test]
fn edit_ops_round_trip_as_json() {
    let op = EditOp::Insert {
        time: f(12),
        clip: ClipSpec {
            media: EntityId::from_key("media:file:x"),
            source_range: TimeRange::from_frames(0, 24, 24.0),
            name: Some("x".into()),
        },
    };
    let text = serde_json::to_string(&op).unwrap();
    assert!(text.contains("\"op\":\"insert\""), "{text}");
    assert_eq!(serde_json::from_str::<EditOp>(&text).unwrap(), op);
    let join: EditOp = serde_json::from_str(r#"{"op":"join","index":3}"#).unwrap();
    assert_eq!(join, EditOp::Join { index: 3 });
}
