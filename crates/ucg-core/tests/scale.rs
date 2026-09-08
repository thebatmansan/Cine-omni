//! Production-scale characterisation of the Unified Context Graph.
//!
//! These are measurements, not assertions about correctness. They exist to
//! answer one architectural question honestly: **does the current in-memory
//! graph hold up at the scale a real feature film demands?**
//!
//! Target scale, from the product directive: thousands of scenes, tens of
//! thousands of shots, and large volumes of metadata records.
//!
//! Run with:
//! ```text
//! cargo test -p ucg-core --release --test scale -- --ignored --nocapture
//! ```
//! They are `#[ignore]`d so they never slow the normal suite.

use std::time::Instant;
use ucg_core::prelude::*;

/// Resident set size in MiB, read from procfs. Linux only; returns 0 elsewhere.
fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|p| p.parse::<u64>().ok())
        })
        .map(|pages| (pages * 4096) as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0)
}

fn bar(label: &str) {
    println!("\n──── {label} ────");
}

/// Build a graph shaped like a real feature: a project containing sequences,
/// each containing scenes, each with dialogue lines, a derived previs scene,
/// shots, comps and clips. Returns (engine, scene ids, shot ids).
fn build_production(
    scenes_wanted: usize,
    lines_per_scene: usize,
    shots_per_scene: usize,
) -> (Engine, Vec<EntityId>, Vec<EntityId>) {
    let engine = Engine::new();
    let mut scenes = Vec::with_capacity(scenes_wanted);
    let mut shots = Vec::with_capacity(scenes_wanted * shots_per_scene);

    engine
        .transact("build production", |tx| {
            let project = tx.create("project")?;
            tx.set(project, "name", "Scale Test")?;

            for s in 0..scenes_wanted {
                let scene = tx.create_child(project, "script.scene")?;
                tx.set(scene, "heading", format!("INT. LOCATION {s} - NIGHT"))?;
                tx.set(scene, "scene_number", format!("{}", s + 1))?;
                tx.set(scene, "words", 0i64)?;
                tx.set(scene, "page_eighths", 8i64)?;
                tx.set(scene, "est_duration_s", 45.0)?;
                scenes.push(scene);

                for l in 0..lines_per_scene {
                    let line = tx.create_child(scene, "script.dialogue")?;
                    tx.set(
                        line,
                        "character",
                        if l % 2 == 0 { "MAREN" } else { "ELIAS" },
                    )?;
                    tx.set(line, "text", "We ride at dawn and we do not look back.")?;
                }

                // Derived previs scene, as the previs reactor would create it.
                let previs = EntityId::derived(scene, "previs.scene");
                tx.create_with_id(previs, "previs.scene")?;
                tx.link(previs, scene, EdgeKind::DerivedFrom)?;

                for k in 0..shots_per_scene {
                    let cam = EntityId::derived(previs, &format!("camera:{k}"));
                    tx.create_with_id(cam, "previs.camera")?;
                    tx.link(previs, cam, EdgeKind::Contains)?;

                    let shot = EntityId::derived(cam, "logistics.shot");
                    tx.create_with_id(shot, "logistics.shot")?;
                    tx.link(shot, cam, EdgeKind::DependsOn)?;
                    tx.set(
                        shot,
                        "name",
                        format!("{}{}", s + 1, (b'A' + k as u8) as char),
                    )?;
                    shots.push(shot);

                    // A comp graph depends on the shot; a clip is synced to the scene.
                    let comp = EntityId::derived(shot, "comp.graph");
                    tx.create_with_id(comp, "comp.graph")?;
                    tx.link(comp, shot, EdgeKind::DependsOn)?;

                    let clip = tx.create("nle.clip")?;
                    tx.link(clip, scene, EdgeKind::SyncedTo)?;

                    let grade = EntityId::derived(clip, "color.grade");
                    tx.create_with_id(grade, "color.grade")?;
                    tx.link(grade, clip, EdgeKind::DependsOn)?;
                }
            }
            Ok(())
        })
        .expect("build");
    (engine, scenes, shots)
}

#[test]
#[ignore]
fn scale_characterisation() {
    // 2,000 scenes × 6 lines × 4 shots ≈ a large feature or an episodic season.
    const SCENES: usize = 2_000;
    const LINES: usize = 6;
    const SHOTS: usize = 4;

    let rss_before = rss_mib();
    bar("BUILD");
    let t = Instant::now();
    let (engine, scenes, shots) = build_production(SCENES, LINES, SHOTS);
    let build = t.elapsed();
    let rss_after = rss_mib();

    let (nodes, edges) = {
        let g = engine.read();
        (g.node_count(), g.edge_count())
    };
    println!("scenes            {SCENES}");
    println!("shots             {}", shots.len());
    println!("nodes             {nodes}");
    println!("edges             {edges}");
    println!("build time        {:?}", build);
    println!("RSS delta         {:.1} MiB", rss_after - rss_before);
    println!(
        "bytes/node        ~{:.0}",
        (rss_after - rss_before) * 1024.0 * 1024.0 / nodes as f64
    );

    bar("SINGLE-ATTRIBUTE COMMIT LATENCY (the interactive path)");
    // This is what an artist typing in a field costs.
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    const EDITS: usize = 200;
    for i in 0..EDITS {
        let line = {
            let g = engine.read();
            g.children(scenes[i % scenes.len()])[0]
        };
        let t = Instant::now();
        engine
            .transact("edit", |tx| tx.set(line, "text", format!("take {i}")))
            .expect("edit");
        let d = t.elapsed();
        worst = worst.max(d);
        total += d;
    }
    println!("edits             {EDITS}");
    println!("mean              {:?}", total / EDITS as u32);
    println!("worst             {:?}", worst);
    println!("budget            16.6ms for 60fps interactivity");

    bar("of_kind() — sorts on EVERY call");
    let g = engine.read();
    let t = Instant::now();
    let mut n = 0;
    for _ in 0..100 {
        n += g.of_kind("logistics.shot").len();
    }
    let d = t.elapsed();
    println!(
        "100 calls         {:?}  ({:?}/call, {} shots each)",
        d,
        d / 100,
        n / 100
    );
    println!("note              reactors call this on every cascade");

    bar("DIRTY-SET PROPAGATION");
    // Narrow: one dialogue line. Wide: the project root, which every node descends from.
    let line = g.children(scenes[0])[0];
    drop(g);
    let rx = engine.subscribe_channel();
    let t = Instant::now();
    engine
        .transact("narrow", |tx| tx.set(line, "text", "narrow"))
        .unwrap();
    let narrow = t.elapsed();
    let cs = rx.recv().unwrap();
    println!("narrow edit       {:?}  dirty={}", narrow, cs.dirty.len());

    let scene0 = scenes[0];
    let t = Instant::now();
    engine
        .transact("scene", |tx| tx.set(scene0, "est_duration_s", 99.0))
        .unwrap();
    let wide = t.elapsed();
    let cs = rx.recv().unwrap();
    println!("scene edit        {:?}  dirty={}", wide, cs.dirty.len());

    bar("snapshot() — full graph clone, used for off-thread evaluation");
    let t = Instant::now();
    let snap = engine.snapshot();
    println!("clone {} nodes   {:?}", snap.node_count(), t.elapsed());
    println!("note              every background evaluation pays this");

    bar("OP-LOG GROWTH (Engine::log is unbounded in memory)");
    println!("deltas retained   {}", engine.log().len());
    println!("undo groups       {}", engine.undo_depth());
    println!("RSS now           {:.1} MiB", rss_mib());

    bar("validate() — full structural check");
    let g = engine.read();
    let t = Instant::now();
    let problems = g.validate();
    println!(
        "validate          {:?}  problems={}",
        t.elapsed(),
        problems.len()
    );
}

/// How does commit latency scale with graph size? This is the question that
/// decides whether an in-memory authoritative graph is viable.
#[test]
#[ignore]
fn commit_latency_vs_graph_size() {
    bar("COMMIT LATENCY vs GRAPH SIZE");
    println!(
        "{:>8}  {:>10}  {:>12}  {:>12}",
        "scenes", "nodes", "mean edit", "snapshot"
    );
    for scenes_n in [100usize, 500, 1_000, 2_000, 4_000] {
        let (engine, scenes, _) = build_production(scenes_n, 6, 4);
        let nodes = engine.read().node_count();

        let mut total = std::time::Duration::ZERO;
        const N: usize = 100;
        for i in 0..N {
            let line = {
                let g = engine.read();
                g.children(scenes[i % scenes.len()])[0]
            };
            let t = Instant::now();
            engine
                .transact("edit", |tx| tx.set(line, "text", format!("v{i}")))
                .unwrap();
            total += t.elapsed();
        }
        let t = Instant::now();
        let _ = engine.snapshot();
        let snap = t.elapsed();
        println!(
            "{:>8}  {:>10}  {:>12?}  {:>12?}",
            scenes_n,
            nodes,
            total / N as u32,
            snap
        );
    }
    println!("\nIf mean edit grows with node count, the interactive path does not scale.");
}
