use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use ucg_core::persist::{graph_from_json, graph_from_msgpack, graph_to_json, graph_to_msgpack};
use ucg_core::prelude::*;
use ucg_core::Project;

fn scene_with_line(engine: &Engine) -> (EntityId, EntityId) {
    let mut ids = (EntityId::nil(), EntityId::nil());
    engine
        .transact("setup", |tx| {
            let s = tx.create("script.scene")?;
            tx.set(s, "heading", "INT. KITCHEN - NIGHT")?;
            let l = tx.create_child(s, "script.dialogue")?;
            tx.set(l, "character", "MARA")?;
            tx.set(l, "text", "We ride at dawn.")?;
            ids = (s, l);
            Ok(())
        })
        .unwrap();
    ids
}

#[test]
fn transaction_creates_nodes_and_edges() {
    let engine = Engine::new();
    let (s, l) = scene_with_line(&engine);
    let g = engine.read();
    assert_eq!(g.node_count(), 2);
    assert_eq!(g.children(s), vec![l]);
    assert_eq!(g.parent(l), Some(s));
    assert_eq!(g.node(l).unwrap().get_str("text"), Some("We ride at dawn."));
    assert_eq!(g.revision(), 1);
}

#[test]
fn failed_transaction_rolls_back() {
    let engine = Engine::new();
    let (s, _) = scene_with_line(&engine);
    let res = engine.transact("bad", |tx| {
        tx.set(s, "heading", "EXT. SOMEWHERE")?;
        tx.create("script.scene")?;
        Err(UcgError::Aborted("nope".into()))
    });
    assert!(res.is_err());
    let g = engine.read();
    assert_eq!(g.node_count(), 2);
    assert_eq!(
        g.node(s).unwrap().get_str("heading"),
        Some("INT. KITCHEN - NIGHT")
    );
    assert_eq!(g.revision(), 1, "revision must not advance on rollback");
}

#[test]
fn idempotent_set_is_noop() {
    let engine = Engine::new();
    let (s, _) = scene_with_line(&engine);
    let group = engine
        .transact("same", |tx| tx.set(s, "heading", "INT. KITCHEN - NIGHT"))
        .unwrap();
    assert!(group.deltas.is_empty());
    assert_eq!(engine.revision(), 1);
}

#[test]
fn dependency_cycles_are_rejected() {
    let engine = Engine::new();
    let res = engine.transact("cycle", |tx| {
        let a = tx.create("x")?;
        let b = tx.create("x")?;
        let c = tx.create("x")?;
        tx.link(a, b, EdgeKind::DependsOn)?;
        tx.link(b, c, EdgeKind::DependsOn)?;
        tx.link(c, a, EdgeKind::DependsOn)
    });
    assert!(matches!(res, Err(UcgError::Cycle { .. })));
    assert_eq!(
        engine.read().node_count(),
        0,
        "whole transaction rolled back"
    );
    // Non-propagating edges may form cycles freely.
    engine
        .transact("refs", |tx| {
            let a = tx.create("x")?;
            let b = tx.create("x")?;
            tx.link(a, b, EdgeKind::References)?;
            tx.link(b, a, EdgeKind::References)
        })
        .unwrap();
}

#[test]
fn dirty_set_is_topologically_ordered() {
    let engine = Engine::new();
    let rx = engine.subscribe_channel();
    let mut ids = vec![];
    engine
        .transact("chain", |tx| {
            // clip -> shot -> scene ; comp -> shot
            let scene = tx.create("script.scene")?;
            let shot = tx.create("previs.shot")?;
            let clip = tx.create("nle.clip")?;
            let comp = tx.create("comp.graph")?;
            tx.link(shot, scene, EdgeKind::DerivedFrom)?;
            tx.link(clip, shot, EdgeKind::SyncedTo)?;
            tx.link(comp, shot, EdgeKind::DependsOn)?;
            ids = vec![scene, shot, clip, comp];
            Ok(())
        })
        .unwrap();
    let _ = rx.recv().unwrap();
    engine
        .transact("edit", |tx| tx.set(ids[0], "heading", "EXT. ROAD - DAY"))
        .unwrap();
    let cs = rx.recv().unwrap();
    assert_eq!(cs.touched, vec![ids[0]]);
    assert_eq!(cs.dirty.len(), 4);
    assert_eq!(cs.dirty[0], ids[0], "the scene is first");
    assert_eq!(cs.dirty[1], ids[1], "the shot before its dependents");
    assert!(cs.is_dirty(ids[2]) && cs.is_dirty(ids[3]));
    assert!(cs.attr_changed(ids[0], "heading"));
    assert!(cs.touches_module("script."));
}

/// A reactor that mirrors the scene heading into a derived previs node.
struct PrevisMirror {
    runs: AtomicUsize,
}
impl Reactor for PrevisMirror {
    fn name(&self) -> &str {
        "previs-mirror"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind("script.scene")
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let mut plans = vec![];
        for scene in cs.dirty_of_kind(g, "script.scene").collect::<Vec<_>>() {
            let heading = g
                .node(scene)
                .and_then(|n| n.get_str("heading"))
                .unwrap_or("")
                .to_string();
            let derived = EntityId::derived(scene, "previs.scene");
            plans.push(Plan::new("mirror heading", move |tx| {
                tx.ensure(derived, "previs.scene")?;
                tx.link(derived, scene, EdgeKind::DerivedFrom)?;
                tx.set(derived, "label", heading)
            }));
        }
        Ok(plans)
    }
}

/// A second-order reactor that reacts to previs changes (tests cascade depth).
struct LogisticsCounter;
impl Reactor for LogisticsCounter {
    fn name(&self) -> &str {
        "logistics"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind("previs.scene")
    }
    fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>> {
        let n = g.of_kind("previs.scene").len() as i64;
        let root = g.of_kind("logistics.summary").first().map(|n| n.id);
        let _ = cs;
        Ok(vec![Plan::new("count", move |tx| {
            let id = match root {
                Some(id) => id,
                None => tx.create("logistics.summary")?,
            };
            tx.set(id, "previs_scenes", n)
        })])
    }
}

#[test]
fn reactor_cascade_and_undo_as_unit() {
    let engine = Engine::new();
    let mirror = Arc::new(PrevisMirror {
        runs: AtomicUsize::new(0),
    });
    engine.add_reactor(mirror.clone());
    engine.add_reactor(Arc::new(LogisticsCounter));

    let (s, _) = scene_with_line(&engine);
    {
        let g = engine.read();
        let d = EntityId::derived(s, "previs.scene");
        assert_eq!(
            g.node(d).unwrap().get_str("label"),
            Some("INT. KITCHEN - NIGHT")
        );
        assert!(g.has_edge(d, s, EdgeKind::DerivedFrom));
        let summary = g.of_kind("logistics.summary")[0];
        assert_eq!(summary.get_int("previs_scenes"), Some(1));
    }
    assert_eq!(mirror.runs.load(Ordering::SeqCst), 1);

    // Editing the heading cascades again; identical follow-ups are no-ops, so it settles.
    let group = engine
        .transact("rename", |tx| tx.set(s, "heading", "EXT. ROAD - DAY"))
        .unwrap();
    assert!(group.deltas.len() >= 2, "user delta + mirror delta");
    {
        let g = engine.read();
        let d = EntityId::derived(s, "previs.scene");
        assert_eq!(g.node(d).unwrap().get_str("label"), Some("EXT. ROAD - DAY"));
    }

    // Undo reverts the user edit *and* the cascade in one step.
    assert_eq!(engine.undo_depth(), 2);
    engine.undo().unwrap();
    {
        let g = engine.read();
        let d = EntityId::derived(s, "previs.scene");
        assert_eq!(
            g.node(s).unwrap().get_str("heading"),
            Some("INT. KITCHEN - NIGHT")
        );
        assert_eq!(
            g.node(d).unwrap().get_str("label"),
            Some("INT. KITCHEN - NIGHT")
        );
    }
    engine.redo().unwrap();
    {
        let g = engine.read();
        let d = EntityId::derived(s, "previs.scene");
        assert_eq!(g.node(d).unwrap().get_str("label"), Some("EXT. ROAD - DAY"));
    }
    // Undo everything including the setup transaction (derived nodes vanish).
    engine.undo().unwrap();
    engine.undo().unwrap();
    assert_eq!(engine.read().node_count(), 0);
    assert!(engine.undo().is_err());
}

#[test]
fn cascade_depth_limit_stops_runaway_reactors() {
    struct Runaway;
    impl Reactor for Runaway {
        fn name(&self) -> &str {
            "runaway"
        }
        fn react(&self, cs: &ChangeSet, _g: &Graph) -> Result<Vec<Plan>> {
            let id = cs.touched[0];
            let rev = cs.revision() as i64;
            Ok(vec![Plan::new("bump", move |tx| {
                tx.set(id, "counter", rev)
            })])
        }
    }
    let mut engine = Engine::new();
    engine.max_cascade_depth = 4;
    engine.add_reactor(Arc::new(Runaway));
    let res = engine.transact("start", |tx| {
        tx.create("x")?;
        Ok(())
    });
    assert!(matches!(res, Err(UcgError::CascadeDepth(4))));
}

#[test]
fn delete_is_recursive_and_undoable() {
    let engine = Engine::new();
    let (s, l) = scene_with_line(&engine);
    let other = engine
        .transact("ref", |tx| {
            let o = tx.create("nle.clip")?;
            tx.link(o, s, EdgeKind::SyncedTo)?;
            Ok(())
        })
        .unwrap();
    let _ = other;
    engine.transact("delete", |tx| tx.delete(s)).unwrap();
    {
        let g = engine.read();
        assert!(!g.contains(s) && !g.contains(l));
        assert_eq!(g.edge_count(), 0);
        assert_eq!(g.node_count(), 1);
    }
    engine.undo().unwrap();
    let g = engine.read();
    assert!(g.contains(s) && g.contains(l));
    assert_eq!(g.children(s), vec![l]);
    assert_eq!(g.edge_count(), 2, "containment + synced_to edges restored");
    assert!(g.validate().is_empty());
}

#[test]
fn snapshot_round_trips_json_and_msgpack() {
    let engine = Engine::new();
    let (s, l) = scene_with_line(&engine);
    engine
        .transact("typed", |tx| {
            tx.set(s, "bounds", Value::Vec3([1.0, 2.0, 3.0]))?;
            tx.set(l, "range", TimeRange::from_frames(10, 48, 24.0))?;
            tx.set(l, "tint", Color::rgb(0.1, 0.2, 0.3))
        })
        .unwrap();
    let g = engine.snapshot();
    let json = graph_to_json(&g).unwrap();
    let back = graph_from_json(&json).unwrap();
    assert_eq!(back.node_count(), 2);
    assert_eq!(back.children(s), vec![l]);
    assert_eq!(
        back.attr(l, "range").unwrap().as_range(),
        Some(TimeRange::from_frames(10, 48, 24.0))
    );
    assert_eq!(
        back.attr(s, "bounds").unwrap().as_vec3(),
        Some([1.0, 2.0, 3.0])
    );
    let bytes = graph_to_msgpack(&g).unwrap();
    let back2 = graph_from_msgpack(&bytes).unwrap();
    assert_eq!(back2.node(l).unwrap().attrs, g.node(l).unwrap().attrs);
    assert!(back2.validate().is_empty());
}

#[test]
fn project_save_open_and_oplog_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Film.cineomni");
    let project = Project::create(&path, "Film").unwrap();
    project.enable_autosave();
    let (s, _) = scene_with_line(&project.engine);
    project.save().unwrap();
    // Changes after the snapshot live only in the op log.
    project
        .engine
        .transact("late edit", |tx| tx.set(s, "heading", "EXT. DUNES - DUSK"))
        .unwrap();
    project
        .engine
        .transact("late add", |tx| {
            let c = tx.create("nle.clip")?;
            tx.link(c, s, EdgeKind::SyncedTo)
        })
        .unwrap();

    let reopened = Project::open(&path).unwrap();
    let g = reopened.engine.read();
    assert_eq!(
        g.node(s).unwrap().get_str("heading"),
        Some("EXT. DUNES - DUSK")
    );
    assert_eq!(g.of_kind("nle.clip").len(), 1);
    assert_eq!(g.revision(), 3);
    assert!(g.validate().is_empty());
}

#[test]
fn parallel_readers_do_not_block() {
    let engine = Arc::new(Engine::new());
    let (s, _) = scene_with_line(&engine);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let e = engine.clone();
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    let g = e.read();
                    assert!(g.node(s).is_some());
                }
            })
        })
        .collect();
    for i in 0..100 {
        engine.transact("w", |tx| tx.set(s, "n", i)).unwrap();
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(engine.read().node(s).unwrap().get_int("n"), Some(99));
}

#[test]
fn large_graph_propagation_is_fast() {
    let engine = Engine::new();
    let mut ids = Vec::new();
    engine
        .transact("build", |tx| {
            let root = tx.create("script.scene")?;
            ids.push(root);
            for i in 0..5000 {
                let n = tx.create("nle.clip")?;
                tx.link(n, ids[i / 4], EdgeKind::DependsOn)?;
                ids.push(n);
            }
            Ok(())
        })
        .unwrap();
    let rx = engine.subscribe_channel();
    let t = std::time::Instant::now();
    engine
        .transact("touch", |tx| tx.set(ids[0], "heading", "X"))
        .unwrap();
    let cs = rx.recv().unwrap();
    assert_eq!(cs.dirty.len(), 5001);
    assert!(
        t.elapsed().as_millis() < 500,
        "propagation over 5k nodes took {:?}",
        t.elapsed()
    );
}

#[test]
fn undo_history_survives_reopening_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Film.cineomni");
    let (s, l) = {
        let project = Project::create(&path, "Film").unwrap();
        project.enable_autosave();
        // One reactor so user actions produce multi-delta groups.
        project
            .engine
            .add_reactor(std::sync::Arc::new(PrevisMirror {
                runs: AtomicUsize::new(0),
            }));
        let (s, l) = scene_with_line(&project.engine);
        project
            .engine
            .transact("edit", |tx| tx.set(l, "text", "Take two."))
            .unwrap();
        project.save().unwrap();
        project
            .engine
            .transact("edit again", |tx| tx.set(l, "text", "Take three."))
            .unwrap();
        assert_eq!(project.engine.undo_depth(), 3);
        (s, l)
    };

    // Reopened in a new "process": the whole history is still there, including
    // the transactions that predate the snapshot.
    let p = Project::open(&path).unwrap();
    p.enable_autosave();
    assert_eq!(p.engine.undo_depth(), 3, "history rebuilt from the op log");
    assert_eq!(
        p.engine.read().node(l).unwrap().get_str("text"),
        Some("Take three.")
    );

    p.engine.undo().unwrap();
    assert_eq!(
        p.engine.read().node(l).unwrap().get_str("text"),
        Some("Take two.")
    );
    p.engine.undo().unwrap();
    {
        let g = p.engine.read();
        assert_eq!(g.node(l).unwrap().get_str("text"), Some("We ride at dawn."));
        // The reactor's derived node was reverted with the user's edit.
        assert_eq!(
            g.node(EntityId::derived(s, "previs.scene"))
                .unwrap()
                .get_str("label"),
            Some("INT. KITCHEN - NIGHT")
        );
    }
    assert_eq!(p.engine.undo_depth(), 1);
    assert_eq!(p.engine.redo_depth(), 2);

    // Undo/redo state itself survives another reopen.
    drop(p);
    let p = Project::open(&path).unwrap();
    p.enable_autosave();
    assert_eq!((p.engine.undo_depth(), p.engine.redo_depth()), (1, 2));
    assert_eq!(
        p.engine.read().node(l).unwrap().get_str("text"),
        Some("We ride at dawn.")
    );
    p.engine.redo().unwrap();
    assert_eq!(
        p.engine.read().node(l).unwrap().get_str("text"),
        Some("Take two.")
    );
    assert!(p.engine.read().validate().is_empty());

    // Compaction drops the pre-snapshot history but keeps the graph intact.
    p.save().unwrap();
    assert!(p.compact().unwrap() > 0);
    let p = Project::open(&path).unwrap();
    assert_eq!(
        p.engine.read().node(l).unwrap().get_str("text"),
        Some("Take two.")
    );
    assert_eq!(p.engine.undo_depth(), 0);
}
