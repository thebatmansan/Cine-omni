//! Does the 7-reactor cascade scale to a real production?
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

fn measure(scenes: usize, lines: usize) -> (Duration, Duration, usize, usize, usize) {
    let project = OmniProject::new(ProjectSetup::default()).expect("project");

    let t = Instant::now();
    let doc = project
        .import_fountain(&screenplay(scenes, lines), "scale.fountain")
        .expect("import");
    let import = t.elapsed();

    let scene_ids = project.scenes(doc);
    let (nodes, edges) = {
        let g = project.engine.read();
        (g.node_count(), g.edge_count())
    };

    // THE INTERACTIVE PATH: one artist edits one line of dialogue.
    let line = project.dialogue_lines(scene_ids[scene_ids.len() / 2])[0];
    let t = Instant::now();
    let group = project
        .edit_dialogue(line, "This time I mean it, and I will not say it twice.")
        .expect("edit");
    let edit = t.elapsed();

    (import, edit, nodes, edges, group.deltas.len())
}

#[test]
#[ignore]
fn cascade_latency_vs_production_size() {
    println!("\nOne dialogue edit, all 7 reactors installed.");
    println!("Interactive budget: 100ms is already sluggish; 1s is unusable.\n");
    println!(
        "{:>7} {:>7} {:>9} {:>9} {:>12} {:>14} {:>7}",
        "scenes", "lines", "nodes", "edges", "import", "ONE EDIT", "deltas"
    );
    println!("{}", "-".repeat(72));

    for (scenes, lines) in [(7usize, 3usize), (50, 4), (200, 4), (500, 4), (1000, 4)] {
        let (import, edit, nodes, edges, deltas) = measure(scenes, lines);
        println!(
            "{:>7} {:>7} {:>9} {:>9} {:>12?} {:>14?} {:>7}",
            scenes, lines, nodes, edges, import, edit, deltas
        );
    }

    println!("\nIf ONE EDIT grows with scene count, the cascade is not incremental:");
    println!("an O(1) user action is triggering O(N) whole-document recomputation.");
}
