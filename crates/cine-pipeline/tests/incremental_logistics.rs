//! Phase 1B: the logistics reactor must be incremental *without* losing
//! correctness for changes that genuinely have global consequences.
//!
//! Scheduling is a global optimisation, so the schedule itself is still solved
//! whole whenever its inputs change. What these tests pin down is:
//!
//! * a change that touches nothing the scheduler reads does no scheduling work;
//! * every change that *does* affect scheduling still produces a correct,
//!   fully-updated schedule and call sheets;
//! * call-sheet regeneration is dependency-aware rather than unconditional.
//!
//! Every `engine.read()` guard is scoped so none is ever held across a
//! `transact` (the engine's RwLock is not reentrant).

use cine_logistics::graph::{read_schedule, read_schedule_fingerprint, SCHEDULE_FINGERPRINT_ATTR};
use cine_logistics::schedule::schedule_fingerprint_with;
use cine_pipeline::{OmniProject, ProjectSetup};
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds};

fn screenplay(scenes: usize) -> String {
    let mut s = String::from("Title: Incremental\n\n");
    for i in 0..scenes {
        s.push_str(&format!(
            "INT. LOCATION {i} - NIGHT #{}#\n\nMAREN crosses. A LAMP burns.\n\nMAREN\nWe ride at dawn.\n\nELIAS\nNot tonight.\n\n",
            i + 1
        ));
    }
    s
}

fn project(scenes: usize) -> (OmniProject, EntityId, Vec<EntityId>) {
    let p = OmniProject::new(ProjectSetup::default()).expect("project");
    let doc = p
        .import_fountain(&screenplay(scenes), "inc.fountain")
        .expect("import");
    let ids = p.scenes(doc);
    (p, doc, ids)
}

fn schedule_of(p: &OmniProject, doc: EntityId) -> cine_logistics::Schedule {
    let g = p.engine.read();
    read_schedule(&g, doc).expect("schedule exists")
}

/// Recompute the fingerprint exactly as the reactor does, constraints included.
fn fingerprint_of(p: &OmniProject, doc: EntityId) -> u64 {
    let g = p.engine.read();
    let constraints = cine_pipeline::reactors::read_constraints(&g, doc);
    schedule_fingerprint_with(&g, doc, Some(&constraints))
}

fn stored_fingerprint(p: &OmniProject, doc: EntityId) -> Option<u64> {
    let g = p.engine.read();
    read_schedule_fingerprint(&g, doc)
}

// ---------------------------------------------------------------------------
// 1. A local change that the scheduler does not read.
// ---------------------------------------------------------------------------

#[test]
fn schedule_neutral_change_does_no_scheduling_work() {
    let (p, doc, scenes) = project(30);
    let before = schedule_of(&p, doc);
    let fp_before = stored_fingerprint(&p, doc);
    assert!(fp_before.is_some(), "importing must store a fingerprint");

    // Annotating a scene changes nothing the scheduler consumes.
    let group = p
        .engine
        .transact("note", |tx| tx.set(scenes[3], "notes", "continuity: hat"))
        .unwrap();

    // The reschedule plan must have produced no ops at all.
    let reschedule_ops: usize = group
        .deltas
        .iter()
        .filter(|d| d.label.contains("reschedule"))
        .map(|d| d.ops.len())
        .sum();
    assert_eq!(
        reschedule_ops, 0,
        "a schedule-neutral edit must not touch the schedule"
    );

    let after = schedule_of(&p, doc);
    assert_eq!(before.days.len(), after.days.len());
    assert_eq!(
        stored_fingerprint(&p, doc),
        fp_before,
        "fingerprint unchanged"
    );
}

// ---------------------------------------------------------------------------
// 2. Duration change — schedule-affecting, must still be correct.
// ---------------------------------------------------------------------------

#[test]
fn duration_change_updates_the_schedule_correctly() {
    let (p, doc, scenes) = project(20);
    let fp_before = stored_fingerprint(&p, doc).unwrap();
    let before = schedule_of(&p, doc);

    // A much longer line changes est_duration_s, which the scheduler reads.
    let line = p.dialogue_lines(scenes[5])[0];
    p.edit_dialogue(
        line,
        "We ride at dawn, and we do not stop until the mountains are behind us \
         and the river is in front of us and the valley is only a story.",
    )
    .unwrap();

    let fp_after = stored_fingerprint(&p, doc).unwrap();
    assert_ne!(
        fp_after, fp_before,
        "duration is a scheduler input; fingerprint must move"
    );
    assert_eq!(
        fp_after,
        fingerprint_of(&p, doc),
        "stored fingerprint must match a freshly computed one"
    );

    // The schedule must still be complete and cover every scene exactly once.
    let after = schedule_of(&p, doc);
    let scheduled: Vec<EntityId> = after
        .days
        .iter()
        .flat_map(|d| d.strips.iter().map(|s| s.scene))
        .collect();
    assert_eq!(
        scheduled.len(),
        scenes.len(),
        "every scene scheduled exactly once"
    );
    for s in &scenes {
        assert!(scheduled.contains(s), "scene {s} missing from the schedule");
    }
    assert!(!before.days.is_empty() && !after.days.is_empty());
}

// ---------------------------------------------------------------------------
// 3. Scene insertion and deletion.
// ---------------------------------------------------------------------------

#[test]
fn scene_insertion_and_deletion_reschedule_correctly() {
    let (p, doc, scenes) = project(12);
    let strips_before: usize = schedule_of(&p, doc)
        .days
        .iter()
        .map(|d| d.strips.len())
        .sum();
    assert_eq!(strips_before, scenes.len());

    // Insert a scene by re-importing a longer screenplay (stable ids).
    let mut longer = screenplay(12);
    longer
        .push_str("EXT. RIVERBANK - DAWN #13#\n\nMAREN watches the water.\n\nMAREN\nTold you.\n\n");
    p.import_fountain(&longer, "inc.fountain").unwrap();

    let after_insert = schedule_of(&p, doc);
    let strips_after: usize = after_insert.days.iter().map(|d| d.strips.len()).sum();
    assert_eq!(strips_after, 13, "inserted scene must be scheduled");

    // Delete it again.
    p.import_fountain(&screenplay(12), "inc.fountain").unwrap();
    let after_delete = schedule_of(&p, doc);
    let strips_final: usize = after_delete.days.iter().map(|d| d.strips.len()).sum();
    assert_eq!(strips_final, 12, "deleted scene must leave the schedule");

    // No dangling day should reference a scene that no longer exists.
    let g = p.engine.read();
    for day in &after_delete.days {
        for strip in &day.strips {
            assert!(
                g.contains(strip.scene),
                "schedule references a deleted scene"
            );
        }
    }
    assert!(g.validate().is_empty());
}

// ---------------------------------------------------------------------------
// 4. Scene reorder.
// ---------------------------------------------------------------------------

#[test]
fn scene_reorder_is_schedule_affecting() {
    let (p, doc, scenes) = project(10);
    let fp_before = stored_fingerprint(&p, doc).unwrap();

    // Swapping the order attribute changes strip ordering, which the scheduler
    // consumes when grouping by location.
    let (a, b) = (scenes[2], scenes[7]);
    let (oa, ob) = {
        let g = p.engine.read();
        (
            g.node(a).unwrap().get_int(attrs::ORDER).unwrap_or(0),
            g.node(b).unwrap().get_int(attrs::ORDER).unwrap_or(0),
        )
    };
    p.engine
        .transact("reorder", |tx| {
            tx.set(a, attrs::ORDER, ob)?;
            tx.set(b, attrs::ORDER, oa)
        })
        .unwrap();

    assert_ne!(
        stored_fingerprint(&p, doc).unwrap(),
        fp_before,
        "reorder must reschedule"
    );
    let after = schedule_of(&p, doc);
    let scheduled: usize = after.days.iter().map(|d| d.strips.len()).sum();
    assert_eq!(scheduled, 10, "all scenes still scheduled after a reorder");
}

// ---------------------------------------------------------------------------
// 5. A schedule-affecting dependency change that is not on the scene itself.
// ---------------------------------------------------------------------------

#[test]
fn page_count_change_reschedules() {
    let (p, doc, scenes) = project(15);
    let fp_before = stored_fingerprint(&p, doc).unwrap();

    // Page eighths drive the pages-per-day packing directly.
    p.engine
        .transact("bigger scene", |tx| {
            tx.set(scenes[4], attrs::PAGE_EIGHTHS, 48i64)
        })
        .unwrap();

    assert_ne!(stored_fingerprint(&p, doc).unwrap(), fp_before);
    let after = schedule_of(&p, doc);
    let total: u32 = after.days.iter().map(|d| d.total_eighths).sum();
    assert!(
        total >= 48,
        "the enlarged scene must be reflected in the schedule"
    );
    // Every scene still present.
    let scheduled: usize = after.days.iter().map(|d| d.strips.len()).sum();
    assert_eq!(scheduled, 15);
}

// ---------------------------------------------------------------------------
// 6. A genuinely global change.
// ---------------------------------------------------------------------------

#[test]
fn global_change_reschedules_everything() {
    let (p, doc, scenes) = project(24);
    let before = schedule_of(&p, doc);

    // Halving the pages-per-day limit is a global constraint change: it must
    // repack the entire film, not one day.
    let constraints = cine_logistics::schedule::ScheduleConstraints {
        max_pages_per_day: 1.0,
        ..Default::default()
    };
    p.engine
        .transact("tighten schedule", |tx| {
            tx.set(
                doc,
                "schedule_constraints",
                Value::from_serde(&constraints)?,
            )
        })
        .unwrap();

    let after = schedule_of(&p, doc);
    assert!(
        after.days.len() > before.days.len(),
        "a tighter page limit must produce more shooting days ({} -> {})",
        before.days.len(),
        after.days.len()
    );
    let scheduled: usize = after.days.iter().map(|d| d.strips.len()).sum();
    assert_eq!(
        scheduled,
        scenes.len(),
        "no scene lost during a global repack"
    );

    let g = p.engine.read();
    assert!(g.validate().is_empty());
}

// ---------------------------------------------------------------------------
// 7. Call-sheet regeneration is dependency-aware.
// ---------------------------------------------------------------------------

#[test]
fn call_sheets_regenerate_only_for_days_that_changed() {
    let (p, doc, scenes) = project(40);

    // Capture each day's call sheet revision before the edit.
    let schedule_node = EntityId::derived(doc, kinds::LOGISTICS_SCHEDULE);
    let before: Vec<(EntityId, u64)> = {
        let g = p.engine.read();
        g.children(schedule_node)
            .into_iter()
            .filter_map(|day| {
                let sheet = EntityId::derived(day, kinds::LOGISTICS_CALL_SHEET);
                g.node(sheet).map(|n| (sheet, n.modified_rev))
            })
            .collect()
    };
    assert!(before.len() > 1, "need several days to prove selectivity");

    // A schedule-neutral edit must not rewrite any call sheet at all.
    p.engine
        .transact("note", |tx| tx.set(scenes[0], "notes", "n"))
        .unwrap();
    {
        let g = p.engine.read();
        for (sheet, rev) in &before {
            assert_eq!(
                g.node(*sheet).unwrap().modified_rev,
                *rev,
                "schedule-neutral edit must not touch call sheets"
            );
        }
    }

    // A schedule-affecting edit may rewrite some sheets, but must not rewrite
    // every sheet unconditionally when the day content is unchanged.
    let line = p.dialogue_lines(scenes[1])[0];
    p.edit_dialogue(line, "A slightly different line of dialogue here.")
        .unwrap();

    let g = p.engine.read();
    let rewritten = before
        .iter()
        .filter(|(sheet, rev)| {
            g.node(*sheet)
                .map(|n| n.modified_rev != *rev)
                .unwrap_or(true)
        })
        .count();
    assert!(
        rewritten < before.len(),
        "call sheets must be dependency-aware: {rewritten} of {} rewritten",
        before.len()
    );
}

// ---------------------------------------------------------------------------
// 8. The fingerprint must be stable and deterministic.
// ---------------------------------------------------------------------------

#[test]
fn fingerprint_is_deterministic_and_input_sensitive() {
    let (p, doc, scenes) = project(8);
    let a = fingerprint_of(&p, doc);
    let b = fingerprint_of(&p, doc);
    assert_eq!(a, b, "fingerprint must be deterministic");

    // Insensitive to things the scheduler does not read.
    p.engine
        .transact("note", |tx| tx.set(scenes[0], "notes", "x"))
        .unwrap();
    assert_eq!(
        fingerprint_of(&p, doc),
        a,
        "notes are not a scheduler input"
    );

    // Sensitive to each thing it does read.
    for (label, key, value) in [
        ("pages", attrs::PAGE_EIGHTHS, Value::Int(40)),
        ("duration", attrs::EST_DURATION_S, Value::Float(999.0)),
    ] {
        let before = fingerprint_of(&p, doc);
        let k = key;
        let v = value.clone();
        p.engine
            .transact(label, |tx| tx.set(scenes[1], k, v))
            .unwrap();
        assert_ne!(
            fingerprint_of(&p, doc),
            before,
            "{label} must change the fingerprint"
        );
    }

    // And the attribute name is the documented one.
    let g = p.engine.read();
    let node = g
        .node(EntityId::derived(doc, kinds::LOGISTICS_SCHEDULE))
        .unwrap();
    assert!(node.get_int(SCHEDULE_FINGERPRINT_ATTR).is_some());
}
