//! Does the reactor cascade scale to a real production?
//!
//! The `demo` runs on a 7-scene screenplay and settles in milliseconds. This
//! measures the same single-dialogue-line edit against feature-length and
//! episodic-season scale, with every reactor installed, to find where the
//! reactive architecture stops being interactive.
//!
//! ```text
//! cargo test -p cine-pipeline --release --test scale -- --ignored --nocapture
//! ```

use cine_pipeline::{OmniProject, ProjectSetup};
use std::time::{Duration, Instant};

/// Synthesise a Fountain screenplay with `scenes` scenes and `lines` dialogue
/// lines each, so the whole import → cascade path is exercised realistically.
fn screenplay(scenes: usize, lines: usize) -> String {
    let mut s = String::from("Title: Scale Test\nAuthor: CINE-OMNI\n\n");
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

/// Timings for the two classes of edit that matter.
struct Timings {
    import: Duration,
    /// An edit that changes scheduler inputs (dialogue length moves duration).
    schedule_affecting: Duration,
    /// An edit that changes nothing the scheduler reads (a note on a scene).
    schedule_neutral: Duration,
    nodes: usize,
    deltas: usize,
}

fn measure(scenes: usize, lines: usize) -> Timings {
    let project = OmniProject::new(ProjectSetup::default()).expect("project");

    let t = Instant::now();
    let doc = project
        .import_fountain(&screenplay(scenes, lines), "scale.fountain")
        .expect("import");
    let import = t.elapsed();

    let scene_ids = project.scenes(doc);
    let nodes = project.engine.read().node_count();

    // (a) Schedule-affecting: changing dialogue length changes the scene's
    //     estimated duration, which the scheduler genuinely consumes.
    let line = project.dialogue_lines(scene_ids[scene_ids.len() / 2])[0];
    let t = Instant::now();
    let group = project
        .edit_dialogue(line, "This time I mean it, and I will not say it twice.")
        .expect("edit");
    let schedule_affecting = t.elapsed();

    // (b) Schedule-neutral: annotate a scene. Nothing the scheduler reads
    //     changes, so the fingerprint fast path should skip the whole reschedule.
    let scene = scene_ids[scene_ids.len() / 2];
    let t = Instant::now();
    project
        .engine
        .transact("note", |tx| tx.set(scene, "notes", "check continuity"))
        .expect("note");
    let schedule_neutral = t.elapsed();

    Timings {
        import,
        schedule_affecting,
        schedule_neutral,
        nodes,
        deltas: group.deltas.len(),
    }
}

#[test]
#[ignore]
fn cascade_latency_vs_production_size() {
    println!("\nOne edit, every reactor installed.");
    println!("Interactive budget: 100ms is already sluggish; 1s is unusable.\n");
    println!(
        "{:>7} {:>9} {:>12} {:>18} {:>18} {:>7}",
        "scenes", "nodes", "import", "SCHEDULE-AFFECTING", "SCHEDULE-NEUTRAL", "deltas"
    );
    println!("{}", "-".repeat(78));

    for (scenes, lines) in [(7usize, 3usize), (50, 4), (200, 4), (500, 4), (1000, 4)] {
        let t = measure(scenes, lines);
        println!(
            "{:>7} {:>9} {:>12?} {:>18?} {:>18?} {:>7}",
            scenes, t.nodes, t.import, t.schedule_affecting, t.schedule_neutral, t.deltas
        );
    }

    println!("\nSCHEDULE-NEUTRAL must stay flat: the fingerprint fast path proves the");
    println!("cascade no longer recomputes the film for an unrelated edit.");
    println!("SCHEDULE-AFFECTING stays O(N) by nature - packing a shooting schedule");
    println!("is a global optimisation over every scene - but its graph writes and");
    println!("call-sheet regeneration are now proportional to the days that moved.");
}
