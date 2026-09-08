//! Evaluator behaviour: caching, invalidation, parallelism, cycles, previs passes.

use cine_comp::prelude::*;
use ucg_core::EntityId;

fn two_branch_graph() -> (CompGraph, NodeId, NodeId, NodeId, NodeId) {
    let mut g = CompGraph::new();
    let noise = g
        .add(CompNode::procedural("noise", ProceduralKind::Noise, 96, 64).with_param("seed", 11i64))
        .unwrap();
    let blur = g
        .add(CompNode::blur("blur", 3.0, BlurKind::Gaussian))
        .unwrap();
    let check = g
        .add(
            CompNode::procedural("check", ProceduralKind::Checker, 96, 64).with_param("size", 8i64),
        )
        .unwrap();
    let xf = g
        .add(CompNode::transform("xf", [4.0, 2.0], 15.0, [1.2, 1.2]))
        .unwrap();
    let merge = g.add(CompNode::merge("merge", MergeOp::Screen)).unwrap();
    let grade = g
        .add(CompNode::new("grade", NodeKind::ColorCorrect).with_param("saturation", 0.5f64))
        .unwrap();
    g.connect(noise, 0, blur, 0).unwrap();
    g.connect(check, 0, xf, 0).unwrap();
    g.connect(blur, 0, merge, 0).unwrap();
    g.connect(xf, 0, merge, 1).unwrap();
    g.connect(merge, 0, grade, 0).unwrap();
    (g, noise, blur, xf, grade)
}

#[test]
fn second_evaluate_is_a_cache_hit() {
    let (g, _, _, _, grade) = two_branch_graph();
    let ev = Evaluator::default();
    let first = ev.evaluate(&g, grade, 0).unwrap();
    let s = ev.stats();
    assert_eq!(s.nodes_evaluated, 6);
    assert_eq!(s.cache_hits, 0);
    let second = ev.evaluate(&g, grade, 0).unwrap();
    let s = ev.stats();
    assert_eq!(s.nodes_evaluated, 6, "nothing re-evaluated");
    assert_eq!(s.cache_hits, 1);
    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

#[test]
fn param_change_invalidates_only_downstream() {
    let (mut g, noise, blur, xf, grade) = two_branch_graph();
    let ev = Evaluator::default();
    ev.evaluate(&g, grade, 0).unwrap();
    ev.reset_stats();
    g.set_param(blur, "size", 5.0f64).unwrap();
    ev.evaluate(&g, grade, 0).unwrap();
    let s = ev.stats();
    // noise (blur's input) and the whole transform branch come from the cache
    assert_eq!(s.cache_hits, 2);
    assert_eq!(s.nodes_evaluated, 3);
    assert!(!s.evaluated.contains(&noise));
    assert!(!s.evaluated.contains(&xf));
    assert!(s.evaluated.contains(&blur));
    assert_eq!(*s.evaluated.last().unwrap(), grade);
    // bypass flag is part of the hash too
    ev.reset_stats();
    g.node_mut(blur).unwrap().bypass = true;
    ev.evaluate(&g, grade, 0).unwrap();
    assert_eq!(ev.stats().nodes_evaluated, 3);
    // explicit invalidation drops that node's entries
    let before = ev.cache_len();
    ev.invalidate(blur);
    assert!(ev.cache_len() < before);
}

#[test]
fn parallel_branches_match_serial() {
    let (g, _, _, _, grade) = two_branch_graph();
    let par = Evaluator::default();
    let mut ser = Evaluator::default();
    ser.parallel = false;
    let a = par.evaluate(&g, grade, 7).unwrap();
    let b = ser.evaluate(&g, grade, 7).unwrap();
    assert_eq!(a.data, b.data);
    assert_eq!(par.stats().nodes_evaluated, ser.stats().nodes_evaluated);
    // the evaluator can also be shared across threads
    let shared = std::sync::Arc::new(Evaluator::default());
    let g = std::sync::Arc::new(g);
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let ev = shared.clone();
            let g = g.clone();
            std::thread::spawn(move || ev.evaluate(&g, grade, i % 2).unwrap().data.clone())
        })
        .collect();
    let results: Vec<Vec<f32>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results[0], results[2]);
    assert_eq!(results[1], results[3]);
}

#[test]
fn cycle_rejected_at_link_time() {
    let mut g = CompGraph::new();
    let a = g
        .add(CompNode::procedural("a", ProceduralKind::Checker, 8, 8))
        .unwrap();
    let b = g.add(CompNode::blur("b", 1.0, BlurKind::Box)).unwrap();
    let m = g.add(CompNode::merge("m", MergeOp::Over)).unwrap();
    g.connect(a, 0, b, 0).unwrap();
    g.connect(b, 0, m, 0).unwrap();
    g.connect(a, 0, m, 1).unwrap();
    let err = g.connect(m, 0, b, 0).unwrap_err();
    assert!(matches!(err, Error::Cycle { .. }), "{err}");
    assert!(matches!(g.connect(m, 0, m, 0), Err(Error::Cycle { .. })));
    assert_eq!(g.links.len(), 3, "rejected links leave the graph untouched");
    assert!(g.validate().is_empty());
    assert_eq!(g.topological_order().unwrap().last(), Some(&m));
    // a graph with a hand-inserted cycle is refused by the evaluator
    g.links.push(Link {
        from: (m, 0),
        to: (b, 0),
    });
    assert!(g.topological_order().is_err());
    assert!(!g.validate().is_empty());
    assert!(matches!(
        Evaluator::default().evaluate(&g, m, 0),
        Err(Error::Cycle { .. })
    ));
}

#[test]
fn synthetic_depth_pass_drives_zdefocus() {
    let provider = SyntheticPassProvider::new(96, 64);
    let scene = EntityId::new();
    let cam = EntityId::new();
    let depth = provider.render(PassKind::Depth, 0, None);
    let beauty = provider.render(PassKind::Beauty, 0, None);
    let (cx, cy) = (48usize, 32usize);
    assert!(provider.covers(cx, cy, 0));

    let mut g = CompGraph::new();
    let rb = g
        .add(CompNode::read_previs(
            "beauty",
            scene,
            PassKind::Beauty,
            cam,
        ))
        .unwrap();
    let rd = g
        .add(CompNode::read_previs("depth", scene, PassKind::Depth, cam))
        .unwrap();
    let zd = g
        .add(
            CompNode::new("defocus", NodeKind::ZDefocus)
                .with_param("f_stop", 2.0f64)
                .with_param("max_blur", 6.0f64)
                .with_param("slices", 8i64),
        )
        .unwrap();
    g.connect(rb, 0, zd, 0).unwrap();
    g.connect(rd, 0, zd, 1).unwrap();
    let ev = Evaluator::new(Box::new(provider.clone()));

    // Focus on the background plane: background pixels are untouched, the sphere is blurred.
    g.set_param(zd, "focal_distance", provider.background_depth as f64)
        .unwrap();
    let out = ev.evaluate(&g, zd, 0).unwrap();
    assert_eq!(
        ev.evaluate(&g, rd, 0).unwrap().as_ref(),
        &depth,
        "Read pulls the provider's depth pass"
    );
    for (x, y) in [(2usize, 2usize), (90, 60), (3, 60)] {
        assert!(!provider.covers(x, y, 0));
        assert_eq!(
            out.get_rgba(x, y),
            beauty.get_rgba(x, y),
            "focus plane unchanged at {x},{y}"
        );
    }
    let rim = (cx + provider.sphere_radius_px() as usize - 2, cy);
    assert!(provider.covers(rim.0, rim.1, 0));
    assert!(
        (out.get_rgba(rim.0, rim.1)[0] - beauty.get_rgba(rim.0, rim.1)[0]).abs() > 1e-3,
        "sphere rim blurred"
    );

    // Focus on one sphere pixel: it stays exact while the background changes.
    let px = (cx - 6, cy);
    g.set_param(zd, "focal_distance", depth.get(px.0, px.1, 0) as f64)
        .unwrap();
    let out = ev.evaluate(&g, zd, 0).unwrap();
    assert_eq!(out.get_rgba(px.0, px.1), beauty.get_rgba(px.0, px.1));
    // the checkered background has depth 40 -> strongly defocused: a checker edge blurs
    let edge = (provider.checker_size, 2usize);
    assert!((out.get_rgba(edge.0, edge.1)[0] - beauty.get_rgba(edge.0, edge.1)[0]).abs() > 1e-3);

    // frames differ because the sphere drifts
    let f0 = ev.evaluate(&g, rb, 0).unwrap();
    let f5 = ev.evaluate(&g, rb, 5).unwrap();
    assert_ne!(f0.data, f5.data);

    // unknown scene with a scene-filtered provider -> missing source
    let strict = SyntheticPassProvider {
        scene: Some(EntityId::new()),
        ..SyntheticPassProvider::default()
    };
    let ev2 = Evaluator::new(Box::new(strict));
    assert!(matches!(
        ev2.evaluate(&g, rb, 0),
        Err(Error::MissingSource(_))
    ));
}

#[test]
fn region_of_interest_only_computes_requested_pixels() {
    let provider = SyntheticPassProvider::new(64, 64);
    let scene = EntityId::new();
    let mut g = CompGraph::new();
    let r = g
        .add(CompNode::read_previs(
            "beauty",
            scene,
            PassKind::Beauty,
            EntityId::nil(),
        ))
        .unwrap();
    let ev = Evaluator::new(Box::new(provider.clone()));
    let roi = RegionOfInterest::new(8, 8, 16, 16);
    let part = ev.evaluate_roi(&g, r, 0, roi).unwrap();
    let full = ev.evaluate(&g, r, 0).unwrap();
    assert_eq!(part.get_rgba(10, 10), full.get_rgba(10, 10));
    assert_eq!(
        part.get_rgba(40, 40),
        [0.0; 4],
        "outside the region nothing was rendered"
    );
    assert_eq!(
        ev.stats().nodes_evaluated,
        2,
        "region and full frame are separate cache entries"
    );
}

#[test]
fn write_records_output_and_graph_json_round_trips() {
    let (mut g, _, _, _, grade) = two_branch_graph();
    let w = g.add(CompNode::write("write", "renders/comp.exr")).unwrap();
    g.connect(grade, 0, w, 0).unwrap();
    let json = g.to_json().unwrap();
    let back = CompGraph::from_json(&json).unwrap();
    assert_eq!(back, g);
    let ev = Evaluator::default();
    ev.evaluate(&back, w, 12).unwrap();
    let writes = ev.take_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].path, "renders/comp.exr");
    assert_eq!(writes[0].frame, 12);
    assert_eq!((writes[0].image.width, writes[0].image.height), (96, 64));
    // cached -> no second write record
    ev.evaluate(&back, w, 12).unwrap();
    assert!(ev.writes().is_empty());
}
