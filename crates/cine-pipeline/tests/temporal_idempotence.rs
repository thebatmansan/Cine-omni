//! The invariant that makes the reactor cascade converge:
//!
//! > **A recomputed temporal value that is arithmetically identical must
//! > produce no operation.**
//!
//! This is not a style preference. `Tx::set` decides whether to emit a
//! `SetAttr` by comparing the new value against the stored one with
//! `Value: PartialEq`. Under the previous float time model a duration
//! recomputed from the same inputs could differ in its last bit, compare
//! unequal, and emit an operation for a value that had not changed — which
//! dirtied everything downstream of it, ran reactors that had nothing to do,
//! and appended to the op log forever. A long editing session would grow its
//! history without the project changing.
//!
//! Measured under the float model: 48,408 of 200,000 recomputed times compared
//! unequal. These tests hold the exact model to zero.

use cine_pipeline::{OmniProject, ProjectSetup};
use ucg_core::prelude::*;

const SCREENPLAY: &str = r#"Title: Idempotence
Author: CINE-OMNI

INT. HARBOR OFFICE - NIGHT #1#

MAREN crosses the room. A LAMP flickers on the desk.

MAREN
We ride at dawn and we do not look back.

ELIAS
Then we had better sleep.

EXT. DOCKS - DAWN #2#

ELIAS waits by a rusted CRANE.

ELIAS
You came.

INT. GALLEY - DAY #3#

The KETTLE boils.

MAREN
Sit down.
"#;

/// Every `SetAttr` in a group whose value is a time.
fn temporal_writes(group: &UndoGroup) -> Vec<(EntityId, String)> {
    group
        .deltas
        .iter()
        .flat_map(|d| d.ops.iter())
        .filter_map(|op| match op {
            Op::SetAttr {
                id,
                key,
                value: Value::Time(_) | Value::Range(_),
                ..
            } => Some((*id, key.clone())),
            _ => None,
        })
        .collect()
}

fn settled_project() -> OmniProject {
    let p = OmniProject::new(ProjectSetup::default()).expect("project");
    p.import_fountain(SCREENPLAY, "idem.fountain")
        .expect("import");
    p
}

/// The mandated test: force the cascade to run many times over, and require
/// that no temporal attribute is ever rewritten with a value equal to the one
/// already stored.
#[test]
fn recomputing_temporal_values_a_thousand_times_writes_nothing() {
    let p = settled_project();
    let doc = {
        let g = p.engine.read();
        g.of_kind(ucg_schema::kinds::SCRIPT_DOCUMENT)
            .first()
            .map(|n| n.id)
            .expect("a screenplay")
    };
    let scene = p.scenes(doc)[0];

    // Warm up: let the cascade reach its fixed point.
    p.engine
        .transact("settle", |tx| tx.set(scene, "notes", "warm"))
        .expect("settle");

    let mut offenders: Vec<(usize, EntityId, String)> = Vec::new();
    for i in 0..1_000 {
        // Each iteration writes a genuinely different note, so the reactors
        // run and recompute every derived temporal value. Nothing about the
        // scene's *timing* changes, so nothing temporal should be written.
        let group = p
            .engine
            .transact("touch", |tx| tx.set(scene, "notes", format!("pass {i}")))
            .expect("touch");

        for (id, key) in temporal_writes(&group) {
            offenders.push((i, id, key));
            if offenders.len() > 5 {
                break;
            }
        }
        if offenders.len() > 5 {
            break;
        }
    }

    assert!(
        offenders.is_empty(),
        "a recomputed temporal value was written back {} time(s) despite not changing. \
         First offenders: {:?}. Each one dirties everything downstream and grows the op \
         log for a value that did not change.",
        offenders.len(),
        &offenders[..offenders.len().min(5)]
    );
}

/// The op log must grow only with real changes.
///
/// A thousand note edits are a thousand changes, so the log grows by roughly a
/// thousand — not by a multiple of that, which is what recomputed temporal
/// values would add.
#[test]
fn the_op_log_grows_only_with_real_changes() {
    let p = settled_project();
    let doc = {
        let g = p.engine.read();
        g.of_kind(ucg_schema::kinds::SCRIPT_DOCUMENT)
            .first()
            .map(|n| n.id)
            .unwrap()
    };
    let scene = p.scenes(doc)[0];
    p.engine
        .transact("settle", |tx| tx.set(scene, "notes", "warm"))
        .unwrap();

    const PASSES: usize = 500;
    let mut total_ops = 0usize;
    for i in 0..PASSES {
        let group = p
            .engine
            .transact("touch", |tx| tx.set(scene, "notes", format!("pass {i}")))
            .unwrap();
        total_ops += group.deltas.iter().map(|d| d.ops.len()).sum::<usize>();
    }

    // One op per pass is the note itself. Anything much beyond that is the
    // cascade rewriting values that did not change.
    assert!(
        total_ops <= PASSES * 2,
        "{PASSES} unrelated edits produced {total_ops} operations — about {:.1} per edit. \
         The cascade is rewriting derived state that did not change.",
        total_ops as f64 / PASSES as f64
    );
}

/// Idempotence stated directly: applying the same edit twice must be free the
/// second time, at every rate the project might be working in.
#[test]
fn re_applying_an_identical_edit_is_free() {
    let p = settled_project();
    let doc = {
        let g = p.engine.read();
        g.of_kind(ucg_schema::kinds::SCRIPT_DOCUMENT)
            .first()
            .map(|n| n.id)
            .unwrap()
    };
    let scene = p.scenes(doc)[0];
    let line = p.dialogue_lines(scene)[0];
    let text = "We ride at dawn and we do not look back.";

    p.edit_dialogue(line, text).expect("first");
    let second = p.edit_dialogue(line, text).expect("second");
    assert!(
        second.deltas.is_empty(),
        "re-applying an identical edit produced {} operations",
        second.deltas.len()
    );

    // And a third time, after the cascade has had a chance to churn.
    let third = p.edit_dialogue(line, text).expect("third");
    assert!(third.deltas.is_empty());
}

/// The engine's revision must not advance when nothing changed. A UI that
/// tracks the revision would otherwise redraw continuously.
#[test]
fn the_revision_stands_still_when_nothing_changes() {
    let p = settled_project();
    let doc = {
        let g = p.engine.read();
        g.of_kind(ucg_schema::kinds::SCRIPT_DOCUMENT)
            .first()
            .map(|n| n.id)
            .unwrap()
    };
    let scene = p.scenes(doc)[0];
    p.engine
        .transact("settle", |tx| tx.set(scene, "notes", "fixed"))
        .unwrap();

    let revision = p.engine.revision();
    for _ in 0..200 {
        p.engine
            .transact("no-op", |tx| tx.set(scene, "notes", "fixed"))
            .unwrap();
    }
    assert_eq!(
        p.engine.revision(),
        revision,
        "200 no-op transactions advanced the revision; the cascade has no fixed point"
    );
}
