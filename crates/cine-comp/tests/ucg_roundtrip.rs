//! Storing comp graphs in the Unified Context Graph.

use cine_comp::prelude::*;
use cine_comp::ucg::{comp_id_for_shot, mark_clean, node_entity_id, template_graph};
use ucg_core::{EdgeKind, Engine, EntityId, RationalTime, TimeRange, UcgError, Value};
use ucg_schema::{attrs, kinds};

fn shot_engine() -> (Engine, EntityId) {
    let engine = Engine::new();
    let mut shot = EntityId::nil();
    engine
        .transact("shot", |tx| {
            shot = tx.create(kinds::LOGISTICS_SHOT)?;
            tx.set(shot, attrs::NAME, "SH010")
        })
        .unwrap();
    (engine, shot)
}

fn op_count(group: &ucg_core::UndoGroup) -> usize {
    group.deltas.iter().map(|d| d.ops.len()).sum()
}

#[test]
fn ensure_comp_is_idempotent() {
    let (engine, shot) = shot_engine();
    let mut comp = EntityId::nil();
    let first = engine
        .transact("ensure", |tx| {
            comp = ensure_comp_for_shot(tx, shot)?;
            Ok(())
        })
        .unwrap();
    assert!(op_count(&first) > 0);
    assert_eq!(comp, comp_id_for_shot(shot));
    {
        let g = engine.read();
        let node = g.node(comp).unwrap();
        assert_eq!(node.kind.as_str(), kinds::COMP_GRAPH);
        assert_eq!(node.name(), "Comp SH010");
        assert!(g.has_edge(comp, shot, EdgeKind::DependsOn));
        let loaded = load_graph(&g, comp).unwrap();
        assert_eq!(loaded.nodes.len(), template_graph("SH010").nodes.len());
        assert!(loaded.validate().is_empty());
        assert_eq!(
            g.node(node_entity_id(comp, "write")).unwrap().kind.as_str(),
            kinds::COMP_NODE
        );
    }
    let second = engine
        .transact("ensure again", |tx| {
            let again = ensure_comp_for_shot(tx, shot)?;
            assert_eq!(again, comp);
            Ok(())
        })
        .unwrap();
    assert_eq!(op_count(&second), 0, "second ensure produces zero ops");
    // a missing shot is an error
    let err = engine
        .transact("bad", |tx| {
            ensure_comp_for_shot(tx, EntityId::new())
                .map(|_| ())
                .map_err(Into::into)
        })
        .unwrap_err();
    assert!(matches!(err, UcgError::NotFound(_)));
}

#[test]
fn store_load_round_trip_and_removal() {
    let (engine, shot) = shot_engine();
    let mut comp = EntityId::nil();
    engine
        .transact("ensure", |tx| {
            ensure_comp_for_shot(tx, shot)
                .map(|c| comp = c)
                .map_err(Into::into)
        })
        .unwrap();

    let mut g = CompGraph::new();
    let scene = EntityId::new();
    let cam = EntityId::new();
    let r = g
        .add(CompNode::read_previs("plate", scene, PassKind::Beauty, cam).at(10.5, -3.25))
        .unwrap();
    let k = g
        .add(
            CompNode::keyer("key", KeyerKind::Chroma)
                .with_param("key_color", [0.1f32, 0.8, 0.2, 1.0])
                .with_param("tolerance", 0.2f64),
        )
        .unwrap();
    let roto = g
        .add(CompNode::roto(
            "roto",
            vec![Shape::rect(1.0, 2.0, 3.0, 4.0).with_feather(1.5)],
            64,
            64,
        ))
        .unwrap();
    let ex = g
        .add(CompNode::expression("expr", "r*0.5", "", "", "").with_random_id())
        .unwrap();
    let mut m = CompNode::merge("merge", MergeOp::Screen);
    m.bypass = true;
    let m = g.add(m).unwrap();
    g.connect(r, 0, k, 0).unwrap();
    g.connect(k, 0, ex, 0).unwrap();
    g.connect(ex, 0, m, 0).unwrap();
    g.connect(r, 0, m, 1).unwrap();
    g.connect(roto, 0, m, 2).unwrap();

    let stored = engine
        .transact("store", |tx| store_graph(tx, comp, &g).map_err(Into::into))
        .unwrap();
    assert!(op_count(&stored) > 0);
    let loaded = load_graph(&engine.read(), comp).unwrap();
    assert_eq!(loaded, g, "round trip equality");
    assert_eq!(
        loaded.by_name("plate").unwrap().read_source(),
        ReadSource::PrevisPass {
            scene,
            pass: PassKind::Beauty,
            camera: cam
        }
    );
    assert!(loaded.by_name("merge").unwrap().bypass);
    {
        let snap = engine.read();
        let kids = snap.children(comp);
        assert_eq!(kids.len(), 5, "template nodes were removed");
        assert!(snap.node(node_entity_id(comp, "defocus")).is_none());
    }
    // storing the same graph again is a no-op
    let again = engine
        .transact("store again", |tx| {
            store_graph(tx, comp, &g).map_err(Into::into)
        })
        .unwrap();
    assert_eq!(op_count(&again), 0);

    // remove a node and store: the entity vanishes and its links go with it
    g.remove(roto);
    engine
        .transact("store 3", |tx| {
            store_graph(tx, comp, &g).map_err(Into::into)
        })
        .unwrap();
    let loaded = load_graph(&engine.read(), comp).unwrap();
    assert_eq!(loaded, g);
    assert!(engine.read().node(node_entity_id(comp, "roto")).is_none());
    assert_eq!(loaded.links.len(), 4);

    // the stored form is plain attribute data
    let snap = engine.read();
    let kn = snap.node(node_entity_id(comp, "key")).unwrap();
    assert_eq!(kn.get_str("kind"), Some("keyer"));
    assert!(matches!(kn.get("params"), Some(Value::Map(_))));
    assert!(matches!(snap.attr(comp, "links"), Some(Value::List(_))));
}

#[test]
fn dirty_range_and_dependents() {
    let (engine, shot) = shot_engine();
    let mut comp = EntityId::nil();
    engine
        .transact("ensure", |tx| {
            ensure_comp_for_shot(tx, shot)
                .map(|c| comp = c)
                .map_err(Into::into)
        })
        .unwrap();

    let g1 = engine
        .transact("dirty", |tx| {
            mark_dirty(tx, comp, "script changed")
                .map(|c| assert!(c))
                .map_err(Into::into)
        })
        .unwrap();
    assert!(op_count(&g1) > 0);
    let g2 = engine
        .transact("dirty again", |tx| {
            mark_dirty(tx, comp, "script changed")
                .map(|c| assert!(!c))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(op_count(&g2), 0, "mark_dirty is idempotent");
    {
        let snap = engine.read();
        assert_eq!(snap.attr(comp, attrs::STALE), Some(&Value::Bool(true)));
        assert_eq!(
            snap.attr(comp, attrs::DIRTY_REASON).and_then(Value::as_str),
            Some("script changed")
        );
    }
    engine
        .transact("new reason", |tx| {
            mark_dirty(tx, comp, "camera moved")
                .map(|c| assert!(c))
                .map_err(Into::into)
        })
        .unwrap();
    engine
        .transact("clean", |tx| {
            mark_clean(tx, comp).map(|c| assert!(c)).map_err(Into::into)
        })
        .unwrap();
    let g3 = engine
        .transact("clean again", |tx| {
            mark_clean(tx, comp)
                .map(|c| assert!(!c))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(op_count(&g3), 0);

    let range = TimeRange::from_frames(1001, 48, 24.0);
    engine
        .transact("range", |tx| {
            update_frame_range(tx, comp, range)
                .map(|c| assert!(c))
                .map_err(Into::into)
        })
        .unwrap();
    let g4 = engine
        .transact("range again", |tx| {
            update_frame_range(tx, comp, range)
                .map(|c| assert!(!c))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(op_count(&g4), 0);
    let longer = TimeRange::new(
        RationalTime::from_frames(1001, 24.0),
        RationalTime::from_frames(72, 24.0),
    );
    engine
        .transact("range 3", |tx| {
            update_frame_range(tx, comp, longer)
                .map(|c| assert!(c))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(
        engine
            .read()
            .attr(comp, attrs::RANGE)
            .and_then(Value::as_range),
        Some(longer)
    );

    // dependents
    let snap = engine.read();
    assert_eq!(comps_depending_on(&snap, shot), vec![comp]);
    assert!(comps_depending_on(&snap, comp).is_empty());
    assert!(comps_depending_on(&snap, EntityId::new()).is_empty());
    drop(snap);
    // a non-comp dependent of the shot is filtered out
    engine
        .transact("grade", |tx| {
            let grade = tx.create(kinds::COLOR_GRADE)?;
            tx.link(grade, shot, EdgeKind::DependsOn)
        })
        .unwrap();
    assert_eq!(comps_depending_on(&engine.read(), shot), vec![comp]);
    // wrong entity kinds are rejected by the writers
    let err = engine
        .transact("bad", |tx| {
            mark_dirty(tx, shot, "x").map(|_| ()).map_err(Into::into)
        })
        .unwrap_err();
    assert!(matches!(err, UcgError::Aborted(_)));
}

#[test]
fn bind_previs_camera_updates_reads_and_edges() {
    let (engine, shot) = shot_engine();
    let mut comp = EntityId::nil();
    let mut scene = EntityId::nil();
    let mut cam = EntityId::nil();
    engine
        .transact("setup", |tx| {
            comp = ensure_comp_for_shot(tx, shot)?;
            scene = tx.create(kinds::PREVIS_SCENE)?;
            cam = tx.create(kinds::PREVIS_CAMERA)?;
            tx.link(scene, cam, EdgeKind::Contains)
        })
        .unwrap();
    let g1 = engine
        .transact("bind", |tx| {
            bind_previs_camera(tx, comp, scene, cam).map_err(Into::into)
        })
        .unwrap();
    assert!(op_count(&g1) > 0);
    {
        let snap = engine.read();
        assert!(snap.has_edge(comp, cam, EdgeKind::DependsOn));
        assert!(snap.has_edge(comp, scene, EdgeKind::DependsOn));
        assert_eq!(comps_depending_on(&snap, cam), vec![comp]);
        let g = load_graph(&snap, comp).unwrap();
        let reads: Vec<&CompNode> = g
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Read)
            .collect();
        assert_eq!(reads.len(), 2);
        for r in reads {
            match r.read_source() {
                ReadSource::PrevisPass {
                    scene: s,
                    camera: c,
                    ..
                } => {
                    assert_eq!(s, scene);
                    assert_eq!(c, cam);
                }
                other => panic!("unexpected source {other:?}"),
            }
        }
        // and the bound graph evaluates against a provider
        let ev = Evaluator::new(Box::new(SyntheticPassProvider {
            scene: Some(scene),
            ..SyntheticPassProvider::new(64, 48)
        }));
        let write = g.id_of("write").unwrap();
        let out = ev.evaluate(&g, write, 1).unwrap();
        assert_eq!(
            (out.width, out.height),
            (1920, 1080),
            "backdrop defines the format"
        );
        assert_eq!(ev.writes().len(), 1);
    }
    let g2 = engine
        .transact("bind again", |tx| {
            bind_previs_camera(tx, comp, scene, cam).map_err(Into::into)
        })
        .unwrap();
    assert_eq!(op_count(&g2), 0, "binding twice is a no-op");
}
