//! Deterministic regression gates for the reactor cascade.
//!
//! ## Why these are not benchmarks
//!
//! The characterisation tests in `scale.rs` measure wall-clock time and are
//! `#[ignore]`d, because a heavy timing test is both slow and unstable in CI:
//! a loaded runner produces a red build that says nothing about the code. That
//! left the architecture with no automatic protection at all — a Phase 1-class
//! O(N) regression could land and only be noticed by someone running the
//! ignored suite by hand.
//!
//! These gates close that hole by asserting **work done**, not time taken.
//! Delta counts, changed-day counts and idempotence are exact integers that
//! depend only on the code, so they are stable on any machine and fast enough
//! to run in the default suite.
//!
//! The property they defend is the Phase 1 lesson, stated as an invariant:
//!
//! > **The cost of an edit must be proportional to what the edit actually
//! > affects — never to the size of the film.**
//!
//! A gate that fires here means someone reintroduced whole-film recomputation.
//! The failure message says which class of regression it is.

use cine_pipeline::{OmniProject, ProjectSetup};

/// Two sizes far enough apart that any linear term is unmistakable, but small
/// enough that the whole file runs in the normal suite.
const SMALL: usize = 25;
const LARGE: usize = 200;
const LINES: usize = 4;

fn screenplay(scenes: usize, lines: usize) -> String {
    let mut s = String::from("Title: Gate\nAuthor: CINE-OMNI\n\n");
    for i in 0..scenes {
        s.push_str(&format!(
            "INT. LOCATION {i} - NIGHT #{}#\n\nMAREN crosses the room. A LAMP flickers on the desk.\n\n",
            i + 1
        ));
        for l in 0..lines {
            let who = if l % 2 == 0 { "MAREN" } else { "ELIAS" };
            s.push_str(&format!(
                "{who}\nWe ride at dawn and we do not look back.\n\n"
            ));
        }
    }
    s
}

struct Project {
    project: OmniProject,
    scenes: Vec<ucg_core::EntityId>,
}

fn build(scenes: usize) -> Project {
    let project = OmniProject::new(ProjectSetup::default()).expect("project");
    let doc = project
        .import_fountain(&screenplay(scenes, LINES), "gate.fountain")
        .expect("import");
    let scene_ids = project.scenes(doc);
    Project {
        project,
        scenes: scene_ids,
    }
}

/// Deltas produced by annotating one scene — an edit no reactor reads.
fn neutral_edit_deltas(scenes: usize) -> usize {
    let p = build(scenes);
    let scene = p.scenes[p.scenes.len() / 2];
    p.project
        .engine
        .transact("note", |tx| tx.set(scene, "notes", "check continuity"))
        .expect("note")
        .deltas
        .len()
}

// ---------------------------------------------------------------------------
// Gate 1 — the Phase 1 regression, caught deterministically
// ---------------------------------------------------------------------------

/// **The central gate.** An edit that no reactor consumes must cost the same
/// on a 200-scene film as on a 25-scene one.
///
/// This is the exact defect Phase 1 fixed: the logistics reactor rescheduled
/// the entire film for an edit that changed nothing it reads. Expressed as a
/// delta count it is a pure integer comparison — no timing, no flakiness.
#[test]
fn a_schedule_neutral_edit_costs_the_same_at_any_film_size() {
    let small = neutral_edit_deltas(SMALL);
    let large = neutral_edit_deltas(LARGE);

    assert_eq!(
        small, large,
        "REGRESSION: a schedule-neutral edit produced {small} deltas on a {SMALL}-scene \
         film but {large} on a {LARGE}-scene film. Work is scaling with the size of the \
         production instead of with what the edit affects — this is the Phase 1 O(N) \
         defect returning. Check the reactor fingerprint fast paths."
    );

    // And it must be genuinely small, not merely constant. A constant-but-huge
    // cost would pass the comparison above while still being wrong.
    assert!(
        small <= 4,
        "a one-attribute edit produced {small} deltas; it should produce ~1"
    );
}

/// The same property for the reactor cascade's own writes: a local change must
/// not rewrite every scene's derived entities.
#[test]
fn a_local_dialogue_edit_does_not_rewrite_the_whole_film() {
    let small = build(SMALL);
    let large = build(LARGE);

    let edit = |p: &Project| -> usize {
        let scene = p.scenes[p.scenes.len() / 2];
        let line = p.project.dialogue_lines(scene)[0];
        p.project
            .edit_dialogue(line, "This time I mean it, and I will not say it twice.")
            .expect("edit")
            .deltas
            .len()
    };

    let d_small = edit(&small);
    let d_large = edit(&large);

    // Rescheduling a film is legitimately global — packing a shooting schedule
    // is a whole-production optimisation. What must NOT be global is the graph
    // write-back: only the days that actually moved may be rewritten. So the
    // delta count is allowed to grow, but nowhere near proportionally.
    let size_ratio = LARGE as f64 / SMALL as f64; // 8x
    let delta_ratio = d_large as f64 / d_small.max(1) as f64;
    assert!(
        delta_ratio < size_ratio,
        "REGRESSION: film grew {size_ratio:.1}x ({SMALL} to {LARGE} scenes) and the deltas \
         for one dialogue edit grew {delta_ratio:.1}x ({d_small} to {d_large}). The cascade \
         is rewriting work proportional to the film rather than to the change."
    );
}

// ---------------------------------------------------------------------------
// Gate 2 — convergence and idempotence
// ---------------------------------------------------------------------------

/// Reactor convergence depends on writers being idempotent. If re-applying the
/// same input produces operations, the cascade can oscillate instead of
/// settling, and no amount of downstream correctness will save it.
#[test]
fn re_applying_an_identical_edit_produces_no_operations() {
    let p = build(SMALL);
    let scene = p.scenes[0];
    let line = p.project.dialogue_lines(scene)[0];
    let text = "We ride at dawn and we do not look back.";

    p.project.edit_dialogue(line, text).expect("first");
    let second = p.project.edit_dialogue(line, text).expect("second");

    assert!(
        second.deltas.is_empty(),
        "REGRESSION: re-applying an identical edit produced {} operations. Graph writers \
         must be idempotent or the reactor cascade cannot be proven to converge.",
        second.deltas.len()
    );
}

/// The cascade must reach a fixed point. If a second settle produces work, some
/// reactor is reacting to its own output.
#[test]
fn the_cascade_settles_to_a_fixed_point() {
    let p = build(SMALL);
    let scene = p.scenes[0];
    p.project
        .engine
        .transact("note", |tx| tx.set(scene, "notes", "first"))
        .expect("note");

    let before = p.project.engine.revision();
    // Touching nothing must change nothing.
    let idle = p
        .project
        .engine
        .transact("idle", |tx| tx.set(scene, "notes", "first"))
        .expect("idle");
    assert!(
        idle.deltas.is_empty(),
        "REGRESSION: writing an unchanged value produced {} operations; the cascade has no \
         fixed point and will churn.",
        idle.deltas.len()
    );
    assert_eq!(
        p.project.engine.revision(),
        before,
        "revision moved with no operations"
    );
}

// ---------------------------------------------------------------------------
// Gate 3 — the op log stays bounded
// ---------------------------------------------------------------------------

/// An unbounded op log is a slow memory leak that only shows up in a long
/// session — exactly the failure a CI gate should catch instead of a user.
#[test]
fn the_operation_log_stays_bounded_under_sustained_editing() {
    let p = build(SMALL);
    let scene = p.scenes[0];

    // Derive the workload from the configured bound rather than hardcoding a
    // number that silently stops being "past the limit" when the limit changes.
    let limit = p.project.engine.limits().max_log_deltas;
    let edits = limit + limit / 2;
    for i in 0..edits {
        p.project
            .engine
            .transact("churn", |tx| tx.set(scene, "notes", format!("note {i}")))
            .expect("churn");
    }

    let dropped = p.project.engine.log_dropped();
    assert!(
        dropped > 0,
        "REGRESSION: {edits} edits past a {limit}-delta bound dropped nothing from the op \
         log. The log is unbounded and will grow without limit in a long session."
    );
    assert!(
        p.project.engine.log().len() <= limit,
        "REGRESSION: the retained log exceeds its own stated bound of {limit}"
    );
    // The graph itself must still be correct after the log has rolled.
    let g = p.project.engine.read();
    assert!(
        g.validate().is_empty(),
        "graph invalid after the op log rolled over"
    );
}
