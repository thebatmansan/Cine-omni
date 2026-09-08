//! A reactor may delete what it derived. It may never delete what was captured.
//!
//! A `logistics.shot` is derived: its identity comes from the previs camera it
//! covers, and regenerating the shot list legitimately removes shots whose
//! camera is gone. A `logistics.take` is not derived — it is the record that
//! something was actually shot, with metadata from the set.
//!
//! Because takes hang beneath shots by `Contains`, and node deletion recurses
//! into `Contains` children, deleting a stale shot destroyed its takes. A
//! director removing a previs camera in the morning would silently delete the
//! previous day's take records, in one cascade, with nothing to undo from and
//! no indication it had happened.
//!
//! The rule these tests pin: **a shot that holds captured data is retired, not
//! deleted.**

use cine_logistics::graph::{
    read_shot_list, shot_list_id, takes_of_shot, write_shot_list, write_take_metadata,
};
use cine_logistics::shotlist::ShotEntry;
use cine_logistics::shotlist::{GearList, Support};
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds, ShotSpec};

struct Fixture {
    engine: Engine,
    scene: EntityId,
    camera_a: EntityId,
    camera_b: EntityId,
}

fn shot(camera: EntityId, scene: EntityId, number: &str) -> ShotEntry {
    ShotEntry {
        spec: ShotSpec {
            number: number.to_string(),
            camera: Some(camera),
            ..Default::default()
        },
        camera,
        scene,
        gear: GearList {
            camera_body: "ALEXA 35".into(),
            sensor_mm: [27.99, 19.22],
            lens_mm: 35.0,
            support: Support::Sticks,
            filters: Vec::new(),
            needs_focus_puller: false,
        },
    }
}

/// A scene with two shots, the second of which has three takes captured on set.
fn fixture() -> Fixture {
    let engine = Engine::new();
    let scene = EntityId::from_key("script:scene:17");
    let camera_a = EntityId::from_key("previs:camera:A");
    let camera_b = EntityId::from_key("previs:camera:B");

    engine
        .transact("set up", |tx| {
            tx.ensure(scene, kinds::SCRIPT_SCENE)?;
            tx.ensure(camera_a, kinds::PREVIS_CAMERA)?;
            tx.ensure(camera_b, kinds::PREVIS_CAMERA)?;
            let shots = [shot(camera_a, scene, "17A"), shot(camera_b, scene, "17B")];
            write_shot_list(tx, scene, &shots).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .expect("set up");

    // Shoot three takes on shot 17B.
    let shot_b = {
        let g = engine.read();
        read_shot_list(&g, scene)
            .iter()
            .position(|s| s.spec.number == "17B")
            .map(|i| ucg_core::EntityId::nil().max(children_shot(&g, scene, i)))
            .expect("17B exists")
    };
    engine
        .transact("shoot", |tx| {
            for take in 1..=3u32 {
                write_take_metadata(tx, shot_b, take, &Default::default(), &[])
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
            }
            Ok(())
        })
        .expect("shoot");

    Fixture {
        engine,
        scene,
        camera_a,
        camera_b,
    }
}

/// The nth shot node under the scene's shot list, in stored order.
fn children_shot(g: &Graph, scene: EntityId, index: usize) -> EntityId {
    let list = shot_list_id(scene);
    let mut shots: Vec<&Node> = g
        .children(list)
        .into_iter()
        .filter_map(|c| g.node(c))
        .filter(|n| n.kind.as_str() == kinds::LOGISTICS_SHOT)
        .collect();
    shots.sort_by_key(|n| n.get_int(attrs::ORDER).unwrap_or(0));
    shots[index].id
}

#[test]
fn regenerating_the_shot_list_never_deletes_captured_takes() {
    let f = fixture();
    let shot_b = {
        let g = f.engine.read();
        children_shot(&g, f.scene, 1)
    };
    {
        let g = f.engine.read();
        assert_eq!(
            takes_of_shot(&g, shot_b).len(),
            3,
            "fixture captured three takes"
        );
    }

    // The director removes camera B from previs. The shot list regenerates
    // with only shot A.
    let group = f
        .engine
        .transact("previs changed", |tx| {
            let shots = [shot(f.camera_a, f.scene, "17A")];
            write_shot_list(tx, f.scene, &shots).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .expect("regenerate");

    // Nothing captured may have been deleted.
    let deleted_takes: Vec<EntityId> = group
        .deltas
        .iter()
        .flat_map(|d| d.ops.iter())
        .filter_map(|op| match op {
            Op::DeleteNode { node, .. } if node.kind.as_str() == kinds::LOGISTICS_TAKE => {
                Some(node.id)
            }
            _ => None,
        })
        .collect();
    assert!(
        deleted_takes.is_empty(),
        "the shot-list reactor deleted {} captured take(s): {deleted_takes:?}. \
         Derived data may be regenerated; captured data may not be destroyed.",
        deleted_takes.len()
    );

    let g = f.engine.read();
    assert!(g.contains(shot_b), "the shot holding takes must survive");
    assert_eq!(
        takes_of_shot(&g, shot_b).len(),
        3,
        "all three takes must survive"
    );
    assert!(g.validate().is_empty());
}

#[test]
fn a_shot_that_holds_takes_is_retired_rather_than_deleted() {
    let f = fixture();
    let shot_b = {
        let g = f.engine.read();
        children_shot(&g, f.scene, 1)
    };

    f.engine
        .transact("previs changed", |tx| {
            write_shot_list(tx, f.scene, &[shot(f.camera_a, f.scene, "17A")])
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .expect("regenerate");

    let g = f.engine.read();
    let node = g.require(shot_b).expect("retired shot still present");
    assert_eq!(
        node.get_bool(ucg_schema::attrs::STALE),
        Some(true),
        "a retired shot must be marked stale so a human can see it needs review"
    );
}

#[test]
fn a_shot_with_no_captured_data_is_still_deleted_normally() {
    let f = fixture();
    let shot_a = {
        let g = f.engine.read();
        children_shot(&g, f.scene, 0)
    };
    assert!(takes_of_shot(&f.engine.read(), shot_a).is_empty());

    // Remove camera A, whose shot has nothing captured against it.
    f.engine
        .transact("previs changed", |tx| {
            write_shot_list(tx, f.scene, &[shot(f.camera_b, f.scene, "17B")])
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .expect("regenerate");

    assert!(
        !f.engine.read().contains(shot_a),
        "a purely derived shot with no captured data should still be cleaned up"
    );
}

#[test]
fn a_retired_shot_is_un_retired_when_its_camera_returns() {
    let f = fixture();
    let shot_b = {
        let g = f.engine.read();
        children_shot(&g, f.scene, 1)
    };

    f.engine
        .transact("remove B", |tx| {
            write_shot_list(tx, f.scene, &[shot(f.camera_a, f.scene, "17A")])
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .expect("regenerate");
    assert_eq!(
        f.engine
            .read()
            .require(shot_b)
            .unwrap()
            .get_bool(attrs::STALE),
        Some(true)
    );

    // The director puts the camera back.
    f.engine
        .transact("restore B", |tx| {
            let shots = [
                shot(f.camera_a, f.scene, "17A"),
                shot(f.camera_b, f.scene, "17B"),
            ];
            write_shot_list(tx, f.scene, &shots).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .expect("regenerate");

    let g = f.engine.read();
    assert_eq!(
        g.require(shot_b).unwrap().get_bool(attrs::STALE),
        Some(false),
        "a shot back in the list is no longer retired"
    );
    assert_eq!(
        takes_of_shot(&g, shot_b).len(),
        3,
        "its takes were never lost"
    );
}
