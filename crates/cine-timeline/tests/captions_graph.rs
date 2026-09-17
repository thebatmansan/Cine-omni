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
    assert_eq!(back, sorted, "times, lines, placement and speaker, exactly");
    assert!(export_captions(&g, tl, "fr").unwrap().is_none());
}

#[test]
fn writing_the_same_captions_again_changes_nothing() {
    let (engine, _, tl) = setup();
    let t = track();
    write(&engine, tl, &t);
    let group = write(&engine, tl, &t);
    assert!(group.deltas.is_empty(), "{:?}", group.deltas);
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
