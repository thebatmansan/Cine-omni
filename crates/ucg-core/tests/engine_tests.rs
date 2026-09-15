use std::fs;
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

// ---------------------------------------------------------------------------
// Phase 1: graph index ordering regressions.
//
// `of_kind` and `children` were rewritten for performance. These lock down the
// exact ordering semantics they had before, so the optimisation can never
// silently change observable behaviour.
// ---------------------------------------------------------------------------

#[test]
fn of_kind_returns_insertion_order() {
    let engine = Engine::new();
    let mut made = Vec::new();
    engine
        .transact("build", |tx| {
            // Interleave kinds so a correct answer cannot come from luck.
            for i in 0..50 {
                made.push(tx.create("script.scene")?);
                tx.create("nle.clip")?;
                let _ = i;
            }
            Ok(())
        })
        .unwrap();

    let g = engine.read();
    let got: Vec<EntityId> = g.of_kind("script.scene").iter().map(|n| n.id).collect();
    assert_eq!(got, made, "of_kind must yield insertion order");
    assert_eq!(g.of_kind("nle.clip").len(), 50);
    assert!(g.of_kind("does.not.exist").is_empty());
}

#[test]
fn of_kind_order_survives_removal_and_reinsertion() {
    let engine = Engine::new();
    let mut ids = Vec::new();
    engine
        .transact("build", |tx| {
            for _ in 0..10 {
                ids.push(tx.create("script.scene")?);
            }
            Ok(())
        })
        .unwrap();

    // Remove from the middle: the survivors must keep their relative order.
    let removed = ids[4];
    engine.transact("remove", |tx| tx.delete(removed)).unwrap();
    {
        let g = engine.read();
        let got: Vec<EntityId> = g.of_kind("script.scene").iter().map(|n| n.id).collect();
        let want: Vec<EntityId> = ids.iter().copied().filter(|i| *i != removed).collect();
        assert_eq!(got, want, "removal must not disturb the order of survivors");
    }

    // A newly created node appends at the end.
    let mut fresh = EntityId::nil();
    engine
        .transact("add", |tx| {
            fresh = tx.create("script.scene")?;
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    let got: Vec<EntityId> = g.of_kind("script.scene").iter().map(|n| n.id).collect();
    assert_eq!(*got.last().unwrap(), fresh);
    assert_eq!(got.len(), 10);

    // Undo restores the node, and it re-appends at the end rather than
    // returning to its original index. This is pre-existing behaviour, not a
    // consequence of the index optimisation: node re-insertion appends to the
    // `nodes` IndexMap, and the previous implementation sorted on that same
    // index, so it produced this ordering too. Locked down here so any future
    // change to it is a deliberate decision.
    drop(g);
    engine.undo().unwrap(); // undo the create
    engine.undo().unwrap(); // undo the delete
    let g = engine.read();
    let got: Vec<EntityId> = g.of_kind("script.scene").iter().map(|n| n.id).collect();
    let mut want: Vec<EntityId> = ids.iter().copied().filter(|i| *i != removed).collect();
    want.push(removed); // restored at the end
    assert_eq!(got, want, "undo restores the node, appended at the end");
    assert_eq!(got.len(), ids.len(), "all nodes present after undo");

    // Whatever the order, it must always equal the underlying `nodes` order,
    // which is the invariant the old sort-based implementation guaranteed.
    let nodes_order: Vec<EntityId> = g
        .nodes()
        .filter(|n| n.kind.as_str() == "script.scene")
        .map(|n| n.id)
        .collect();
    assert_eq!(got, nodes_order);
}

#[test]
fn children_order_by_order_attr_then_insertion() {
    let engine = Engine::new();
    let (mut parent, mut a, mut b, mut c, mut d) = (
        EntityId::nil(),
        EntityId::nil(),
        EntityId::nil(),
        EntityId::nil(),
        EntityId::nil(),
    );
    engine
        .transact("build", |tx| {
            parent = tx.create("script.scene")?;
            // Explicit `order` wins; equal/absent order falls back to insertion.
            a = tx.create("script.action")?;
            tx.set(a, "order", 2i64)?;
            tx.link(parent, a, EdgeKind::Contains)?;
            b = tx.create("script.action")?;
            tx.set(b, "order", 1i64)?;
            tx.link(parent, b, EdgeKind::Contains)?;
            c = tx.create("script.action")?; // no order attr
            tx.link(parent, c, EdgeKind::Contains)?;
            d = tx.create("script.action")?; // no order attr, created after c
            tx.link(parent, d, EdgeKind::Contains)?;
            Ok(())
        })
        .unwrap();

    let g = engine.read();
    // None sorts before Some in Rust's Option ordering, so the unordered pair
    // comes first in insertion order, then order=1, then order=2.
    assert_eq!(g.children(parent), vec![c, d, b, a]);
}

#[test]
fn of_kind_matches_a_naive_scan_exactly() {
    // The index must be a pure optimisation: identical results to scanning.
    let engine = Engine::new();
    engine
        .transact("build", |tx| {
            for i in 0..200 {
                let kind = if i % 3 == 0 {
                    "script.scene"
                } else {
                    "nle.clip"
                };
                tx.create(kind)?;
            }
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    for kind in ["script.scene", "nle.clip"] {
        let indexed: Vec<EntityId> = g.of_kind(kind).iter().map(|n| n.id).collect();
        let scanned: Vec<EntityId> = g
            .nodes()
            .filter(|n| n.kind.as_str() == kind)
            .map(|n| n.id)
            .collect();
        assert_eq!(
            indexed, scanned,
            "indexed and scanned results must agree for {kind}"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 1C: the read-guard-across-transact invariant.
//
// Holding an `engine.read()` guard while calling `engine.transact()` deadlocks,
// because the RwLock is not reentrant. That was previously a silent hang with
// no panic and no timeout — the single most dangerous failure mode in the
// codebase. It is now a diagnosable error.
// ---------------------------------------------------------------------------

#[test]
fn holding_a_read_guard_across_transact_reports_instead_of_hanging() {
    let engine = Engine::new();
    engine.set_write_lock_timeout(std::time::Duration::from_millis(250));
    let mut id = EntityId::nil();
    engine
        .transact("seed", |tx| {
            id = tx.create("script.scene")?;
            Ok(())
        })
        .unwrap();

    // The mistake: a live read guard while committing on the same thread.
    let guard = engine.read();
    let result = engine.transact("write while reading", |tx| tx.set(id, "heading", "X"));
    drop(guard);

    match result {
        Err(UcgError::WriteLockTimeout(d)) => {
            assert_eq!(d, std::time::Duration::from_millis(250));
        }
        other => panic!("expected WriteLockTimeout, got {other:?}"),
    }

    // The engine is still usable once the guard is released.
    engine
        .transact("after", |tx| tx.set(id, "heading", "Y"))
        .unwrap();
    assert_eq!(
        engine.read().node(id).unwrap().get_str("heading"),
        Some("Y")
    );
}

#[test]
fn correctly_scoped_guards_never_trip_the_timeout() {
    let engine = Engine::new();
    engine.set_write_lock_timeout(std::time::Duration::from_millis(250));
    let mut id = EntityId::nil();
    engine
        .transact("seed", |tx| {
            id = tx.create("script.scene")?;
            Ok(())
        })
        .unwrap();

    // The correct pattern: the guard is scoped and dropped before committing.
    for i in 0..50 {
        let heading = {
            let g = engine.read();
            g.node(id)
                .unwrap()
                .get_str("heading")
                .unwrap_or("")
                .to_string()
        };
        engine
            .transact("scoped", |tx| {
                tx.set(id, "heading", format!("{heading}{i}"))
            })
            .expect("correctly scoped reads must never time out");
    }

    // Concurrent readers on other threads must not block a writer either.
    let engine = std::sync::Arc::new(engine);
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let e = engine.clone();
            std::thread::spawn(move || {
                for _ in 0..500 {
                    let _ = e.read().node(id).is_some();
                }
            })
        })
        .collect();
    for i in 0..50 {
        engine
            .transact("concurrent", |tx| tx.set(id, "n", i))
            .expect("writer must proceed");
    }
    for r in readers {
        r.join().unwrap();
    }
}

// ---------------------------------------------------------------------------
// Phase 1D: bounded op log and recovery.
//
// The in-memory windows are bounded so a long session cannot grow without
// limit. Durability is unaffected: the on-disk op log stays complete, and undo
// history is rebuilt from it on open.
// ---------------------------------------------------------------------------

#[test]
fn in_memory_log_is_bounded_but_disk_record_stays_complete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Bounded.cineomni");
    let project = Project::create(&path, "Bounded").unwrap();
    project.enable_autosave();
    project.engine.set_limits(ucg_core::EngineLimits {
        max_log_deltas: 32,
        max_undo_groups: 16,
    });

    let mut id = EntityId::nil();
    project
        .engine
        .transact("seed", |tx| {
            id = tx.create("script.scene")?;
            Ok(())
        })
        .unwrap();

    for i in 0..200 {
        project
            .engine
            .transact("edit", |tx| tx.set(id, "n", i))
            .unwrap();
    }

    // In memory: bounded.
    assert!(
        project.engine.log().len() <= 32,
        "log window must respect its bound"
    );
    assert!(
        project.engine.undo_depth() <= 16,
        "undo stack must respect its bound"
    );
    assert!(
        project.engine.log_dropped() > 0,
        "evictions must be reported, not silent"
    );

    // On disk: complete. Nothing needed for recovery was dropped.
    let deltas = ucg_core::persist::read_oplog(&Project::oplog_dir(&path), 0).unwrap();
    assert!(
        deltas.len() >= 201,
        "disk log must retain every delta, got {}",
        deltas.len()
    );

    // And recovery reproduces the final state exactly.
    let reopened = Project::open(&path).unwrap();
    assert_eq!(
        reopened.engine.read().node(id).unwrap().get_int("n"),
        Some(199)
    );
}

#[test]
fn recovery_discards_a_torn_final_record_but_keeps_everything_committed() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Torn.cineomni");
    let mut id = EntityId::nil();
    {
        let project = Project::create(&path, "Torn").unwrap();
        project.enable_autosave();
        project
            .engine
            .transact("seed", |tx| {
                id = tx.create("script.scene")?;
                tx.set(id, "heading", "INT. KITCHEN - NIGHT")
            })
            .unwrap();
        project
            .engine
            .transact("edit", |tx| tx.set(id, "heading", "EXT. ROAD - DAY"))
            .unwrap();
    }

    // Simulate a power loss mid-append: a truncated JSON record at the end.
    let current = Project::oplog_dir(&path).join("current.jsonl");
    let mut f = fs::OpenOptions::new().append(true).open(&current).unwrap();
    f.write_all(br#"{"revision":99,"label":"torn","ops":[{"op":"set_at"#)
        .unwrap();
    drop(f);

    // Recovery must succeed, report the torn tail, and preserve committed work.
    let (deltas, report) =
        ucg_core::persist::read_oplog_with_report(&Project::oplog_dir(&path), 0).unwrap();
    assert!(
        report.torn_tail.is_some(),
        "a truncated final record must be reported"
    );
    assert_eq!(report.recovered, deltas.len());

    let reopened = Project::open(&path).unwrap();
    assert_eq!(
        reopened.engine.read().node(id).unwrap().get_str("heading"),
        Some("EXT. ROAD - DAY"),
        "committed transactions must survive a torn tail"
    );
}

#[test]
fn corruption_before_the_tail_fails_loudly_rather_than_losing_operations() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Corrupt.cineomni");
    {
        let project = Project::create(&path, "Corrupt").unwrap();
        project.enable_autosave();
        let mut id = EntityId::nil();
        project
            .engine
            .transact("seed", |tx| {
                id = tx.create("script.scene")?;
                Ok(())
            })
            .unwrap();
        for i in 0..3 {
            project
                .engine
                .transact("edit", |tx| tx.set(id, "n", i))
                .unwrap();
        }
    }

    // Damage a line in the middle, then append a valid-looking one after it.
    let current = Project::oplog_dir(&path).join("current.jsonl");
    let text = fs::read_to_string(&current).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    assert!(lines.len() >= 3);
    lines[1] = "{ this is not valid json".to_string();
    let mut f = fs::File::create(&current).unwrap();
    f.write_all(lines.join("\n").as_bytes()).unwrap();
    f.write_all(b"\n").unwrap();
    drop(f);

    // Silently skipping it could drop operations that later deltas depend on,
    // so recovery must refuse rather than reconstruct a partial graph.
    let err = ucg_core::persist::read_oplog_with_report(&Project::oplog_dir(&path), 0);
    assert!(
        err.is_err(),
        "mid-file corruption must not be silently skipped"
    );
    assert!(Project::open(&path).is_err());
}

#[test]
fn archive_compaction_boundary_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Compact.cineomni");
    let project = Project::create(&path, "Compact").unwrap();
    project.enable_autosave();
    let mut id = EntityId::nil();
    project
        .engine
        .transact("seed", |tx| {
            id = tx.create("script.scene")?;
            Ok(())
        })
        .unwrap();
    project
        .engine
        .transact("a", |tx| tx.set(id, "heading", "A"))
        .unwrap();

    // save() archives the live segment; it must not discard it.
    project.save().unwrap();
    project
        .engine
        .transact("b", |tx| tx.set(id, "heading", "B"))
        .unwrap();

    let before = Project::open(&path).unwrap();
    assert_eq!(
        before.engine.read().node(id).unwrap().get_str("heading"),
        Some("B")
    );
    assert!(
        before.engine.undo_depth() >= 2,
        "undo history spans the archive boundary"
    );
    drop(before);

    // compact() is the documented boundary at which pre-snapshot history goes.
    project.save().unwrap();
    let removed = project.compact().unwrap();
    assert!(removed > 0, "compaction must remove archived segments");

    // The graph is intact; only the history before the snapshot is gone.
    let after = Project::open(&path).unwrap();
    assert_eq!(
        after.engine.read().node(id).unwrap().get_str("heading"),
        Some("B"),
        "compaction must never change the current state"
    );
    assert_eq!(
        after.engine.undo_depth(),
        0,
        "history before the snapshot is intentionally gone"
    );
}

/// A reactor whose plan always fails.
struct Broken;
impl Reactor for Broken {
    fn name(&self) -> &str {
        "broken"
    }
    fn interested(&self, cs: &ChangeSet) -> bool {
        cs.touches_kind("script.scene")
    }
    fn react(&self, _: &ChangeSet, _: &Graph) -> Result<Vec<Plan>> {
        Ok(vec![Plan::new("fail", |_| {
            Err(UcgError::Aborted("deliberate".into()))
        })])
    }
}

#[test]
fn a_failed_cascade_leaves_the_committed_change_undoable() {
    let engine = Engine::new();
    engine.add_reactor(Arc::new(Broken));
    let mut scene = EntityId::nil();
    let res = engine.transact("create scene", |tx| {
        scene = tx.create("script.scene")?;
        Ok(())
    });
    assert!(res.is_err(), "the reactor failure is reported");
    assert!(
        engine.read().contains(scene),
        "the user's change did commit"
    );
    let history = engine.history();
    assert_eq!(history.undo.len(), 1, "and it is on the undo stack");
    assert_eq!(history.undo[0].label, "create scene");
    engine.undo().unwrap();
    assert!(!engine.read().contains(scene));
    assert_eq!(engine.history().redo.len(), 1);
}

#[test]
fn the_migration_table_covers_every_version() {
    assert_eq!(
        ucg_core::persist::MIGRATIONS.len() as u32,
        ucg_core::persist::SCHEMA_VERSION - 1,
        "every schema version above 1 needs a migration step"
    );
}

#[test]
fn an_op_log_record_from_a_newer_schema_is_refused_not_replayed() {
    let dir = std::env::temp_dir().join(format!("cine-schema-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    {
        let p = Project::create(&dir, "schema").unwrap();
        p.enable_autosave();
        p.engine
            .transact("one", |tx| tx.create("script.scene").map(|_| ()))
            .unwrap();
    }
    let log = dir.join("oplog/current.jsonl");
    let text = fs::read_to_string(&log).unwrap();
    let line = text.lines().last().expect("a committed record").to_string();
    assert!(
        line.contains("\"schema_version\":1"),
        "records are stamped: {line}"
    );

    // A record without the field predates it and reads as version 1.
    let legacy = line.replace(",\"schema_version\":1", "");
    let d: ucg_core::Delta = serde_json::from_str(&legacy).unwrap();
    assert_eq!(d.schema_version, 1);

    // A record from a future schema stops the open with an error that says so.
    let mut future: serde_json::Value = serde_json::from_str(&line).unwrap();
    future["schema_version"] = serde_json::json!(99);
    future["revision"] = serde_json::json!(future["revision"].as_u64().unwrap() + 1);
    fs::write(&log, format!("{text}{}\n{line}\n", future)).unwrap();
    let err = Project::open(&dir)
        .err()
        .expect("a newer record must not be replayed");
    assert!(err.to_string().contains("schema version 99"), "{err}");
    let _ = fs::remove_dir_all(&dir);
}
