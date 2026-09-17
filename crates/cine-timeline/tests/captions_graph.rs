//! Captions in the project graph: stored, read back exactly, idempotent, and
//! not destroyed when the cut they belong to is re-imported.

use cine_timeline::captions::{Caption, CaptionTrack, Placement};
use cine_timeline::graph::{
    caption_languages, export_captions, import_timeline, remove_captions, write_captions,
};
use cine_timeline::prelude::*;
use ucg_core::{Engine, EntityId, Rate, UcgError};
use ucg_schema::kinds;

const FIXTURE: &str = include_str!("../../../tests/fixtures/sample.otio");

fn setup() -> (Engine, EntityId, EntityId) {
    let engine = Engine::new();
    let project = EntityId::from_key("project:test");
    let mut tl = EntityId::nil();
    let timeline = read_otio(FIXTURE).unwrap();
    engine
        .transact("import", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            tl = import_timeline(tx, project, &timeline, "demo.otio")
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    (engine, project, tl)
}

fn track() -> CaptionTrack {
    let rate = Rate::new(24000, 1001).unwrap();
    let at = |f: i64| RationalTime::exact(f, rate);
    let mut t = CaptionTrack::new("en", rate);
    // Written out of order on purpose: the graph holds them in time order.
    let mut later = Caption::new(TimeRange::new(at(100), at(48)), "Not before.\nNot ever.");
    later.speaker = Some("TOBIAS".into());
    t.captions.push(later);
    let mut first = Caption::new(TimeRange::new(at(12), at(36)), "We ride at dawn.");
    first.placement = Placement::Top;
    t.captions.push(first);
    t
}

fn write(engine: &Engine, tl: EntityId, t: &CaptionTrack) -> ucg_core::UndoGroup {
    engine
        .transact("captions", |tx| {
            write_captions(tx, tl, t)
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap()
}

#[test]
fn captions_come_back_exactly_in_time_order() {
    let (engine, _, tl) = setup();
    let t = track();
    write(&engine, tl, &t);
    let g = engine.read();
    assert_eq!(caption_languages(&g, tl), vec!["en".to_string()]);
    let back = export_captions(&g, tl, "en").unwrap().expect("a track");
    let mut sorted = t.clone();
    sorted.sort();
    // Storing gives every caption a key; apart from that, exactly what went in.
    assert!(back.captions.iter().all(|c| c.key.is_some()));
    let unkeyed = CaptionTrack {
        captions: back
            .captions
            .iter()
            .cloned()
            .map(|mut c| {
                c.key = None;
                c
            })
            .collect(),
        ..back.clone()
    };
    assert_eq!(
        unkeyed, sorted,
        "times, lines, placement and speaker, exactly"
    );
    assert!(export_captions(&g, tl, "fr").unwrap().is_none());
}

#[test]
fn writing_the_same_captions_again_changes_nothing() {
    let (engine, _, tl) = setup();
    write(&engine, tl, &track());
    // What a caller writes back is what it read: the keyed track.
    let stored = export_captions(&engine.read(), tl, "en").unwrap().unwrap();
    let group = write(&engine, tl, &stored);
    assert!(group.deltas.is_empty(), "{:?}", group.deltas);
}

/// A caption keeps its entity when another is inserted before it, so what is
/// attached to it stays attached to it.
#[test]
fn inserting_a_caption_earlier_does_not_move_the_ones_after_it() {
    let (engine, _, tl) = setup();
    write(&engine, tl, &track());
    let before = export_captions(&engine.read(), tl, "en").unwrap().unwrap();
    let key_of_later = before.captions[1].key.clone().unwrap();

    let mut more = before.clone();
    let rate = more.rate;
    more.captions.push(Caption::new(
        TimeRange::new(RationalTime::exact(0, rate), RationalTime::exact(6, rate)),
        "(door slams)",
    ));
    write(&engine, tl, &more);
    let after = export_captions(&engine.read(), tl, "en").unwrap().unwrap();
    assert_eq!(after.captions.len(), 3);
    assert_eq!(after.captions[0].lines, vec!["(door slams)"], "time order");
    assert_eq!(
        after.captions[2].key.as_deref(),
        Some(key_of_later.as_str()),
        "the later caption is the same caption"
    );
    let keys: std::collections::HashSet<_> = after.captions.iter().map(|c| c.key.clone()).collect();
    assert_eq!(keys.len(), 3, "and the new one got a key of its own");

    // A removed caption's key is not handed to the next new one.
    let removed = after.captions[0].key.clone().unwrap();
    let mut fewer = after.clone();
    fewer.captions.remove(0);
    write(&engine, tl, &fewer);
    let mut again = export_captions(&engine.read(), tl, "en").unwrap().unwrap();
    again.captions.push(Caption::new(
        TimeRange::new(
            RationalTime::exact(300, rate),
            RationalTime::exact(24, rate),
        ),
        "new",
    ));
    write(&engine, tl, &again);
    let last = export_captions(&engine.read(), tl, "en").unwrap().unwrap();
    assert!(last
        .captions
        .iter()
        .all(|c| c.key.as_deref() != Some(removed.as_str())));
}

/// Captions are authored work. Re-importing the cut reconciles the timeline's
/// own children, and must not take the captions with it.
#[test]
fn re_importing_the_cut_does_not_delete_the_captions() {
    let (engine, project, tl) = setup();
    write(&engine, tl, &track());
    let timeline = read_otio(FIXTURE).unwrap();
    engine
        .transact("re-import", |tx| {
            import_timeline(tx, project, &timeline, "demo.otio")
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let g = engine.read();
    assert_eq!(
        export_captions(&g, tl, "en")
            .unwrap()
            .expect("still there")
            .captions
            .len(),
        2
    );
}

#[test]
fn removing_a_language_removes_its_captions_and_undo_restores_them() {
    let (engine, _, tl) = setup();
    write(&engine, tl, &track());
    let before = engine.read().node_count();
    engine
        .transact("remove", |tx| {
            remove_captions(tx, tl, "en").map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    {
        let g = engine.read();
        assert!(caption_languages(&g, tl).is_empty());
        assert_eq!(g.node_count(), before - 3, "the track and its two captions");
    }
    engine.undo().unwrap();
    assert_eq!(
        export_captions(&engine.read(), tl, "en")
            .unwrap()
            .unwrap()
            .captions
            .len(),
        2
    );
}
