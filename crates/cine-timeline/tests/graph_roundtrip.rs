//! Import / export through the Unified Context Graph.

use cine_timeline::prelude::*;
use cine_timeline::{export_track, Edge};
use ucg_core::{EdgeKind, Engine, EntityId, Graph, UcgError, Value};
use ucg_schema::{attrs, kinds, ClipRef, Component};

const FIXTURE: &str = include_str!("../../../tests/fixtures/sample.otio");
const KEY: &str = "demo.otio";

fn fixture() -> Timeline {
    read_otio(FIXTURE).unwrap()
}

/// The fixture as `export_timeline` should return it: every item carries
/// the deterministic `ucg_id` the importer assigned.
fn fixture_with_ids() -> Timeline {
    let mut tl = fixture();
    for (ti, track) in tl.tracks_mut().enumerate() {
        for (ii, item) in track.children.iter_mut().enumerate() {
            item.set_ucg_id(EntityId::from_key(&format!("nle:{KEY}:{ti}:{ii}")));
        }
    }
    tl
}

fn setup() -> (Engine, EntityId, EntityId) {
    let engine = Engine::new();
    let project = EntityId::from_key("project:test");
    let tl = fixture();
    let mut tl_id = EntityId::nil();
    engine
        .transact("import", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            tl_id = import_timeline(tx, project, &tl, KEY)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    (engine, project, tl_id)
}

fn track_ids(graph: &Graph, timeline: EntityId) -> Vec<EntityId> {
    let stack = graph.children(timeline)[0];
    graph
        .children(stack)
        .into_iter()
        .filter(|c| graph.node(*c).unwrap().kind.as_str() == kinds::NLE_TRACK)
        .collect()
}

fn clip_id(ti: usize, ii: usize) -> EntityId {
    EntityId::from_key(&format!("nle:{KEY}:{ti}:{ii}"))
}

#[test]
fn import_then_export_equals_fixture_with_ids() {
    let (engine, project, tl_id) = setup();
    assert_eq!(tl_id, EntityId::from_key("nle:demo.otio"));
    let g = engine.read();
    assert!(g.has_edge(project, tl_id, EdgeKind::Contains));
    assert_eq!(
        g.attr(tl_id, attrs::SOURCE_KEY).and_then(Value::as_str),
        Some(KEY)
    );
    let exported = export_timeline(&g, tl_id).unwrap();
    assert_eq!(exported, fixture_with_ids());
    assert!(g.validate().is_empty());
}

#[test]
fn graph_shape_and_attributes() {
    let (engine, _, tl_id) = setup();
    let g = engine.read();
    let tracks = track_ids(&g, tl_id);
    assert_eq!(tracks.len(), 4);
    assert_eq!(
        g.attr(tracks[0], attrs::NAME).and_then(Value::as_str),
        Some("V1")
    );
    assert_eq!(
        g.attr(tracks[0], attrs::TRACK_KIND).and_then(Value::as_str),
        Some("video")
    );
    assert_eq!(
        g.attr(tracks[2], attrs::TRACK_KIND).and_then(Value::as_str),
        Some("audio")
    );
    assert_eq!(
        g.attr(tracks[3], attrs::ORDER).and_then(Value::as_int),
        Some(3)
    );

    // V1: A001_C003 | dissolve | A001_C007 | gap | B002_C001 (+ track marker)
    let kids = g.children(tracks[0]);
    let kinds_: Vec<&str> = kids
        .iter()
        .map(|k| g.node(*k).unwrap().kind.as_str())
        .filter(|k| *k != kinds::NLE_MARKER)
        .collect();
    assert_eq!(
        kinds_,
        [
            kinds::NLE_CLIP,
            kinds::NLE_TRANSITION,
            kinds::NLE_CLIP,
            kinds::NLE_GAP,
            kinds::NLE_CLIP
        ]
    );
    let track_markers: Vec<EntityId> = kids
        .iter()
        .copied()
        .filter(|k| g.node(*k).unwrap().kind.as_str() == kinds::NLE_MARKER)
        .collect();
    assert_eq!(track_markers.len(), 1);
    assert_eq!(
        g.attr(track_markers[0], attrs::NAME)
            .and_then(Value::as_str),
        Some("Reel start")
    );

    let a = clip_id(0, 0);
    assert_eq!(
        clip_track_range(&g, a),
        Some(TimeRange::from_frames(0, 96, 24.0))
    );
    let cr = ClipRef::read_from(&g, a).unwrap();
    assert_eq!(cr.source_range, TimeRange::from_frames(24, 96, 24.0));
    assert_eq!(cr.track_start, RationalTime::from_frames(0, 24.0));
    let media = media_of_clip(&g, a).unwrap();
    assert_eq!(
        media,
        EntityId::from_key("media:file:///media/A001_C003.mov")
    );
    assert_eq!(cr.media, Some(media));
    assert_eq!(g.node(media).unwrap().kind.as_str(), kinds::ASSET_MEDIA);
    assert_eq!(
        g.attr(media, attrs::MEDIA_PATH).and_then(Value::as_str),
        Some("file:///media/A001_C003.mov")
    );
    assert_eq!(
        g.attr(media, attrs::AVAILABLE_RANGE)
            .and_then(Value::as_range),
        Some(TimeRange::from_frames(0, 240, 24.0))
    );

    // the transition's range on the track
    assert_eq!(
        clip_track_range(&g, clip_id(0, 1)),
        Some(TimeRange::from_frames(84, 24, 24.0))
    );
    // the gap has a ClipRef without media
    let gap_ref = ClipRef::read_from(&g, clip_id(0, 3)).unwrap();
    assert_eq!(gap_ref.media, None);
    assert_eq!(gap_ref.track_start, RationalTime::from_frames(216, 24.0));
    // one media node per distinct url
    assert_eq!(g.of_kind(kinds::ASSET_MEDIA).len(), 8);
    // clip marker is a child of the clip
    let b = clip_id(0, 4);
    let markers: Vec<EntityId> = g.children(b);
    assert_eq!(markers.len(), 1);
    assert_eq!(
        g.attr(markers[0], attrs::NAME).and_then(Value::as_str),
        Some("Sync point")
    );
}

#[test]
fn reimport_is_idempotent() {
    let (engine, project, _) = setup();
    let before = engine.revision();
    let nodes = engine.read().node_count();
    let tl = fixture();
    let group = engine
        .transact("reimport", |tx| {
            import_timeline(tx, project, &tl, KEY).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert!(
        group.deltas.is_empty(),
        "re-import produced ops: {:?}",
        group.deltas
    );
    assert_eq!(engine.revision(), before);
    assert_eq!(engine.read().node_count(), nodes);

    // Re-importing the *exported* timeline (which carries ucg_ids) is also a no-op.
    let exported = export_timeline(&engine.read(), EntityId::from_key("nle:demo.otio")).unwrap();
    let group = engine
        .transact("reimport exported", |tx| {
            import_timeline(tx, project, &exported, KEY)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert!(group.deltas.is_empty());
}

#[test]
fn reimport_with_changes_updates_in_place() {
    let (engine, project, tl_id) = setup();
    let mut tl = fixture();
    // drop the last clip of V1 and rename the first
    {
        let v1 = tl.tracks_mut().next().unwrap();
        v1.children.pop();
        v1.children[0].as_clip_mut().unwrap().name = "A001_C003_renamed".into();
    }
    engine
        .transact("reimport", |tx| {
            import_timeline(tx, project, &tl, KEY).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    assert!(!g.contains(clip_id(0, 4)), "stale clip deleted");
    assert_eq!(
        g.attr(clip_id(0, 0), attrs::NAME).and_then(Value::as_str),
        Some("A001_C003_renamed")
    );
    let exported = export_timeline(&g, tl_id).unwrap();
    assert_eq!(exported.tracks().next().unwrap().children.len(), 4);
    assert_eq!(
        exported.tracks().next().unwrap().duration(),
        RationalTime::from_frames(264, 24.0)
    );
}

#[test]
fn ucg_id_metadata_survives_external_round_trip() {
    let (engine, project, tl_id) = setup();
    // Export, serialize, "edit externally" (reorder items on V2), re-import.
    let exported = export_timeline(&engine.read(), tl_id).unwrap();
    let text = write_otio(&exported);
    let mut edited = read_otio(&text).unwrap();
    let v2 = edited.tracks_mut().nth(1).unwrap();
    v2.children.swap(0, 1); // clip now first, gap second
    let lower_third_id = clip_id(1, 1);
    engine
        .transact("reimport", |tx| {
            import_timeline(tx, project, &edited, KEY)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    // the clip kept its entity even though its position changed
    assert_eq!(
        g.attr(lower_third_id, attrs::NAME).and_then(Value::as_str),
        Some("Lower Third")
    );
    assert_eq!(
        g.attr(lower_third_id, attrs::ORDER).and_then(Value::as_int),
        Some(0)
    );
    assert_eq!(
        clip_track_range(&g, lower_third_id),
        Some(TimeRange::from_frames(0, 48, 24.0))
    );
}

#[test]
fn apply_edit_ripple_trim_shifts_following_clips() {
    let (engine, _, tl_id) = setup();
    let v1 = track_ids(&engine.read(), tl_id)[0];
    engine
        .transact("trim", |tx| {
            apply_edit(
                tx,
                v1,
                EditOp::RippleTrim {
                    index: 0,
                    edge: Edge::Tail,
                    delta: RationalTime::from_frames(12, 24.0),
                },
            )
            .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let g = engine.read();
    let track = export_track(&g, v1).unwrap();
    assert_eq!(
        track.children[0].source_range(),
        Some(TimeRange::from_frames(24, 108, 24.0))
    );
    assert_eq!(
        track.range_of_child(2),
        Some(TimeRange::from_frames(108, 120, 24.0))
    );
    assert_eq!(
        track.range_of_child(4),
        Some(TimeRange::from_frames(276, 72, 24.0))
    );
    assert_eq!(track.duration(), RationalTime::from_frames(348, 24.0));
    // ids are preserved and graph attrs follow
    assert_eq!(
        clip_track_range(&g, clip_id(0, 2)),
        Some(TimeRange::from_frames(108, 120, 24.0))
    );
    assert_eq!(
        clip_track_range(&g, clip_id(0, 4)),
        Some(TimeRange::from_frames(276, 72, 24.0))
    );
    assert_eq!(
        ClipRef::read_from(&g, clip_id(0, 4)).unwrap().track_start,
        RationalTime::from_frames(276, 24.0)
    );
    // other tracks untouched
    let tl = export_timeline(&g, tl_id).unwrap();
    assert_eq!(
        tl.tracks().nth(2).unwrap().duration(),
        RationalTime::from_frames(336, 24.0)
    );
}

#[test]
fn apply_edit_split_and_extract_keep_ids() {
    let (engine, _, tl_id) = setup();
    let a1 = track_ids(&engine.read(), tl_id)[2];
    engine
        .transact("split", |tx| {
            apply_edit(
                tx,
                a1,
                EditOp::SplitAt {
                    time: RationalTime::from_frames(48, 24.0),
                },
            )
            .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    {
        let g = engine.read();
        let track = export_track(&g, a1).unwrap();
        assert_eq!(track.children.len(), 5);
        assert_eq!(track.children[0].ucg_id(), Some(clip_id(2, 0)));
        assert_eq!(
            track.children[2].ucg_id(),
            Some(clip_id(2, 1)),
            "shifted clip keeps its id"
        );
        assert_eq!(
            track.children[0].source_range(),
            Some(TimeRange::from_frames(24, 48, 24.0))
        );
        assert_eq!(
            track.children[1].source_range(),
            Some(TimeRange::from_frames(72, 48, 24.0))
        );
        assert!(g.contains(track.children[1].ucg_id().unwrap()));
    }
    engine
        .transact("extract", |tx| {
            apply_edit(
                tx,
                a1,
                EditOp::Extract {
                    range: TimeRange::from_frames(0, 48, 24.0),
                },
            )
            .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let g = engine.read();
    let track = export_track(&g, a1).unwrap();
    assert_eq!(track.children.len(), 4);
    assert!(
        !g.contains(clip_id(2, 0)),
        "extracted clip deleted from the graph"
    );
    assert_eq!(track.children[1].ucg_id(), Some(clip_id(2, 1)));
    assert_eq!(track.duration(), RationalTime::from_frames(288, 24.0));
    assert!(g.validate().is_empty());
}

#[test]
fn apply_edit_failure_leaves_graph_untouched() {
    let (engine, _, tl_id) = setup();
    let v1 = track_ids(&engine.read(), tl_id)[0];
    let rev = engine.revision();
    let res = engine.transact("bad", |tx| {
        apply_edit(
            tx,
            v1,
            EditOp::SplitAt {
                time: RationalTime::from_frames(90, 24.0),
            },
        )
        .map_err(|e| UcgError::Aborted(e.to_string()))
    });
    assert!(res.is_err()); // inside the dissolve
    assert_eq!(engine.revision(), rev);
}

#[test]
fn flag_conform_is_idempotent() {
    let (engine, _, _) = setup();
    let clip = clip_id(0, 0);
    let mut changed = (false, false, false);
    engine
        .transact("flag", |tx| {
            changed.0 = flag_conform(tx, clip, "scene 12 rewritten")
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            changed.1 = flag_conform(tx, clip, "scene 12 rewritten")
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            changed.2 = flag_conform(tx, clip, "another reason")
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert_eq!(changed, (true, false, true));
    {
        let g = engine.read();
        assert_eq!(
            g.attr(clip, attrs::NEEDS_CONFORM).and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            g.attr(clip, attrs::DIRTY_REASON).and_then(Value::as_str),
            Some("another reason")
        );
    }
    engine
        .transact("clear", |tx| {
            clear_conform(tx, clip).map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let g = engine.read();
    assert_eq!(
        g.attr(clip, attrs::NEEDS_CONFORM).and_then(Value::as_bool),
        Some(false)
    );
    assert!(g.attr(clip, attrs::DIRTY_REASON).is_none());
    // exporting after flags does not leak them into OTIO metadata
    let exported = export_timeline(&g, EntityId::from_key("nle:demo.otio")).unwrap();
    let a = exported.tracks().next().unwrap().children[0]
        .as_clip()
        .unwrap();
    assert!(!a.metadata.contains_key(attrs::NEEDS_CONFORM));
}

#[test]
fn clips_synced_to_after_bind() {
    let (engine, project, _) = setup();
    let scene = EntityId::from_key("script:scene:12");
    let a = clip_id(0, 0);
    let b = clip_id(0, 2);
    engine
        .transact("bind", |tx| {
            tx.create_with_id(scene, kinds::SCRIPT_SCENE)?;
            tx.link(project, scene, EdgeKind::Contains)?;
            bind_clip_to_scene(tx, a, scene).map_err(|e| UcgError::Aborted(e.to_string()))?;
            bind_clip_to_scene(tx, b, scene).map_err(|e| UcgError::Aborted(e.to_string()))?;
            // binding twice is a no-op
            bind_clip_to_scene(tx, a, scene).map_err(|e| UcgError::Aborted(e.to_string()))?;
            // a gap is not a clip
            assert!(bind_clip_to_scene(tx, clip_id(0, 3), scene).is_err());
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    let mut synced = clips_synced_to(&g, scene);
    synced.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(synced, expected);
    assert!(clips_synced_to(&g, project).is_empty());
    // the SyncedTo edge propagates dirtiness from the scene to the clips
    assert!(g.dependents(scene).contains(&a));
}

#[test]
fn export_of_wrong_kind_is_an_error() {
    let (engine, project, tl_id) = setup();
    let g = engine.read();
    assert!(matches!(
        export_timeline(&g, project),
        Err(Error::WrongKind { .. })
    ));
    assert!(matches!(
        export_timeline(&g, EntityId::new()),
        Err(Error::Ucg(_))
    ));
    assert!(export_track(&g, tl_id).is_err());
}

#[test]
fn apply_edit_that_changes_nothing_emits_no_ops() {
    let (engine, _, tl_id) = setup();
    let v1 = track_ids(&engine.read(), tl_id)[0];
    let rev = engine.revision();
    let group = engine
        .transact("noop slip", |tx| {
            apply_edit(
                tx,
                v1,
                EditOp::Slip {
                    index: 0,
                    delta: RationalTime::from_frames(0, 24.0),
                },
            )
            .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    assert!(group.deltas.is_empty());
    assert_eq!(engine.revision(), rev);
}
