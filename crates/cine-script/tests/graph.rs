mod common;
use cine_script::graph::{alignment_id, alignments_under, scene_elements};
use cine_script::prelude::*;
use common::*;
use ucg_core::{EdgeKind, Engine, Op, UcgError};
use ucg_schema::{attrs, kinds, Component};

const KEY: &str = "sample.fountain";

fn import(engine: &Engine, project: EntityId, sp: &Screenplay) -> ucg_core::UndoGroup {
    engine
        .transact("import", |tx| {
            if !tx.graph().contains(project) {
                tx.create_with_id(project, kinds::PROJECT)?;
            }
            import_screenplay(tx, project, sp, KEY)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap()
}

fn scene_ids(engine: &Engine, doc: EntityId) -> Vec<EntityId> {
    let g = engine.read();
    g.children(doc)
        .into_iter()
        .filter(|c| g.node(*c).unwrap().kind.as_str() == kinds::SCRIPT_SCENE)
        .collect()
}

#[test]
fn import_creates_stable_graph() {
    let sp = fixture();
    let engine = Engine::new();
    let project = EntityId::new();
    let group = import(&engine, project, &sp);
    assert_eq!(group.deltas.len(), 1);
    let doc = document_id(KEY);
    let g = engine.read();
    assert!(g.validate().is_empty());
    assert_eq!(g.node(doc).unwrap().kind.as_str(), kinds::SCRIPT_DOCUMENT);
    assert!(g.has_edge(project, doc, EdgeKind::Contains));
    assert_eq!(
        g.attr(doc, attrs::NAME).unwrap().as_str(),
        Some("THE LAST FERRY")
    );

    let scenes = scene_ids(&engine, doc);
    assert_eq!(scenes.len(), 7);
    assert_eq!(
        scenes[1],
        scene_id(KEY, "1"),
        "explicit scene number is the stable key"
    );
    assert_eq!(scenes[2], scene_id(KEY, "EXT. FERRY DOCK - NIGHT#0"));
    assert_eq!(scenes[0], scene_id(KEY, "(preamble)#0"));
    assert_eq!(
        g.attr(scenes[1], attrs::SCENE_NUMBER).unwrap().as_str(),
        Some("1")
    );
    assert_eq!(
        g.attr(scenes[1], attrs::HEADING).unwrap().as_str(),
        Some("INT. HARBOR OFFICE - NIGHT")
    );
    let heading = SceneHeading::read_from(&g, scenes[1]).unwrap();
    assert_eq!(heading.location, "HARBOR OFFICE");
    assert!(g.attr(scenes[0], SceneHeading::KEY).is_none());

    // Element nodes: scene 1 has 5 action + 5 dialogue + 1 paren + 1 note + 1 boneyard + 1 transition = 14 nodes.
    let kids = g.children(scenes[1]);
    assert_eq!(kids.len(), 14);
    assert_eq!(
        kids[0],
        EntityId::from_key(&format!("script:{KEY}:scene:1:el:0"))
    );
    let dialogue: Vec<EntityId> = kids
        .iter()
        .copied()
        .filter(|k| g.node(*k).unwrap().kind.as_str() == kinds::SCRIPT_DIALOGUE)
        .collect();
    assert_eq!(dialogue.len(), 5);
    let first = g.node(dialogue[0]).unwrap();
    assert_eq!(first.get_str(attrs::CHARACTER), Some("MAREN"));
    assert_eq!(first.get_str(attrs::TEXT), Some("We're closed."));
    assert_eq!(first.get_bool(attrs::DUAL), Some(false));
    assert!(first.get_str(attrs::EXTENSION).is_none());

    // Scene stats and breakdown attrs.
    let s1 = g.node(scenes[1]).unwrap();
    assert_eq!(s1.get_int(attrs::ORDER), Some(1));
    assert!(s1.get_int(attrs::PAGE_EIGHTHS).unwrap() >= 1);
    assert!(s1.get_float(attrs::EST_DURATION_S).unwrap() > 0.0);
    assert!(s1.get_int(attrs::WORDS).unwrap() > 20);
    let chars: Vec<&str> = s1
        .get(attrs::CHARACTERS)
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(chars, vec!["MAREN", "ELIAS"]);
    let locs: Vec<&str> = s1
        .get(attrs::LOCATIONS)
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(locs, vec!["HARBOR OFFICE"]);
    let props: Vec<&str> = s1
        .get(attrs::PROPS)
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(props.contains(&"SATCHEL") && props.contains(&"STAMP"));

    // Cast / location / prop entities with References edges.
    let maren = EntityId::from_key(&format!("script:{KEY}:character:MAREN"));
    assert_eq!(
        g.node(maren).unwrap().kind.as_str(),
        kinds::SCRIPT_CHARACTER
    );
    assert!(g.has_edge(doc, maren, EdgeKind::Contains));
    assert!(g.has_edge(scenes[1], maren, EdgeKind::References));
    assert!(!g.has_edge(scenes[4], maren, EdgeKind::References));
    assert_eq!(g.of_kind(kinds::SCRIPT_CHARACTER).len(), 4);
    assert_eq!(g.of_kind(kinds::SCRIPT_LOCATION).len(), 5);
    assert!(g.of_kind(kinds::SCRIPT_PROP).len() >= 10);
    let dual_scene_dialogue = scene_dialogue_words(&g, scenes[5]);
    assert_eq!(dual_scene_dialogue.len(), 14);
    assert_eq!(dual_scene_dialogue[0].1, vec!["what's", "in", "the", "bag"]);
    let dual_node = g.node(dual_scene_dialogue[8].0).unwrap();
    assert_eq!(dual_node.get_str(attrs::CHARACTER), Some("ELIAS"));
    assert_eq!(dual_node.get_bool(attrs::DUAL), Some(true));

    // Export is the exact inverse.
    let back = export_screenplay(&g, doc).unwrap();
    assert_eq!(back.title_page, sp.title_page);
    assert_eq!(back.elements, sp.elements);
}

#[test]
fn reimport_is_idempotent_and_incremental() {
    let sp = fixture();
    let engine = Engine::new();
    let project = EntityId::new();
    import(&engine, project, &sp);
    let doc = document_id(KEY);
    let before = scene_ids(&engine, doc);
    let node_count = engine.read().node_count();

    // Identical re-import: no ops at all.
    let again = import(&engine, project, &sp);
    assert!(
        again.deltas.is_empty(),
        "identical re-import must produce no ops"
    );

    // Change one dialogue line.
    let mut modified = sp.clone();
    let pos = modified
        .elements
        .iter()
        .position(|e| {
            e == &Element::Dialogue {
                text: "Papers.".into(),
            }
        })
        .unwrap();
    modified.elements[pos] = Element::Dialogue {
        text: "Papers. Old ones, mostly.".into(),
    };
    let group = import(&engine, project, &modified);
    assert_eq!(group.deltas.len(), 1);
    let ops = &group.deltas[0].ops;
    assert!(
        ops.len() <= 5,
        "expected a small delta, got {} ops: {ops:#?}",
        ops.len()
    );
    assert!(ops.iter().all(|o| matches!(o, Op::SetAttr { .. })));
    assert!(ops
        .iter()
        .any(|o| matches!(o, Op::SetAttr { key, .. } if key == attrs::TEXT)));
    assert!(ops
        .iter()
        .any(|o| matches!(o, Op::SetAttr { key, .. } if key == attrs::WORDS)));
    assert_eq!(
        scene_ids(&engine, doc),
        before,
        "scene ids are stable across re-import"
    );
    assert_eq!(engine.read().node_count(), node_count);
    let back = export_screenplay(&engine.read(), doc).unwrap();
    assert_eq!(back, modified);

    // Remove a scene and a cast member: nodes vanish, the rest stays.
    let mut shorter = modified.clone();
    let strait = shorter.scenes()[4].range.clone();
    shorter.elements.drain(strait);
    shorter
        .elements
        .retain(|e| !matches!(e, Element::Character { name, .. } if name == "JONAH"));
    // Remove Jonah's lines too so the block structure stays valid.
    let mut cleaned = Vec::new();
    let mut skip = false;
    for e in shorter.elements {
        match e {
            Element::Character { ref name, .. } => {
                skip = name == "JONAH";
                if !skip {
                    cleaned.push(e);
                }
            }
            Element::Dialogue { .. } | Element::Parenthetical { .. } if skip => {}
            other => {
                skip = false;
                cleaned.push(other);
            }
        }
    }
    let shorter = Screenplay {
        title_page: modified.title_page.clone(),
        elements: cleaned,
    };
    import(&engine, project, &shorter);
    let g = engine.read();
    assert!(g.validate().is_empty());
    let after = scene_ids(&engine, doc);
    assert_eq!(after.len(), 6);
    assert!(!g.contains(before[4]));
    assert!(after.iter().all(|s| before.contains(s)));
    assert!(!g.contains(EntityId::from_key(&format!("script:{KEY}:character:JONAH"))));
    assert!(g.contains(EntityId::from_key(&format!("script:{KEY}:character:MAREN"))));
    assert_eq!(export_screenplay(&g, doc).unwrap(), shorter);
}

#[test]
fn recompute_stats_after_line_edit() {
    let sp = fixture();
    let engine = Engine::new();
    let project = EntityId::new();
    import(&engine, project, &sp);
    let doc = document_id(KEY);
    let scene = scene_ids(&engine, doc)[5];
    let (words_before, dur_before) = {
        let g = engine.read();
        (
            g.attr(scene, attrs::WORDS).unwrap().as_int().unwrap(),
            g.attr(scene, attrs::EST_DURATION_S)
                .unwrap()
                .as_float()
                .unwrap(),
        )
    };
    let (dlg, _) = scene_dialogue_words(&engine.read(), scene)[1].clone();
    engine
        .transact("edit line", |tx| {
            tx.set(
                dlg,
                attrs::TEXT,
                "Papers. Just papers, and a great many of them, all very boring.",
            )?;
            Ok(())
        })
        .unwrap();
    // Stats are untouched until recomputed.
    assert_eq!(
        engine
            .read()
            .attr(scene, attrs::WORDS)
            .unwrap()
            .as_int()
            .unwrap(),
        words_before
    );
    let group = engine
        .transact("recompute", |tx| {
            recompute_scene_stats(tx, scene).map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    assert!(!group.deltas.is_empty());
    let g = engine.read();
    let words_after = g.attr(scene, attrs::WORDS).unwrap().as_int().unwrap();
    assert_eq!(words_after, words_before + 11);
    assert!(
        g.attr(scene, attrs::EST_DURATION_S)
            .unwrap()
            .as_float()
            .unwrap()
            > dur_before
    );
    drop(g); // release the read guard before the next transaction
             // Recomputing again is a no-op.
    let again = engine
        .transact("recompute", |tx| {
            recompute_scene_stats(tx, scene).map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    assert!(again.deltas.is_empty());
    // The scene elements from the graph reflect the edit.
    let g = engine.read();
    let els = scene_elements(&g, scene);
    assert!(els
        .iter()
        .any(|e| e.text().starts_with("Papers. Just papers")));
    drop(g);
    assert!(recompute_scene_stats_wrong_kind(&engine, doc));
}

fn recompute_stats_wrong_kind(engine: &Engine, doc: EntityId) -> bool {
    engine
        .transact("bad", |tx| {
            recompute_scene_stats(tx, doc).map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .is_err()
}

fn recompute_scene_stats_wrong_kind(engine: &Engine, doc: EntityId) -> bool {
    recompute_stats_wrong_kind(engine, doc)
}

#[test]
fn alignment_lifecycle() {
    let sp = fixture();
    let engine = Engine::new();
    let project = EntityId::new();
    import(&engine, project, &sp);
    let doc = document_id(KEY);
    let scene = scene_ids(&engine, doc)[5];
    let media = EntityId::new();
    let lines = scene_dialogue_words(&engine.read(), scene);
    assert!(lines.len() >= 3);

    let mut ids = Vec::new();
    engine
        .transact("align", |tx| {
            tx.create_with_id(media, kinds::ASSET_MEDIA)?;
            let mut clock = 0.0;
            for (dlg, words) in lines.iter().take(3) {
                let transcript: Vec<TimedWord> = words
                    .iter()
                    .map(|w| {
                        let t = TimedWord::new(w.clone(), clock, clock + 0.3);
                        clock += 0.4;
                        t
                    })
                    .collect();
                let aligned = align_words(words, &transcript);
                let id = apply_alignment(tx, *dlg, Some(media), aligned)
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
                ids.push(id);
            }
            Ok(())
        })
        .unwrap();
    {
        let g = engine.read();
        assert_eq!(ids[0], alignment_id(lines[0].0));
        assert_eq!(ids[0], EntityId::derived(lines[0].0, kinds::SYNC_ALIGNMENT));
        assert!(g.has_edge(lines[0].0, ids[0], EdgeKind::Contains));
        assert!(g.has_edge(ids[0], media, EdgeKind::SyncedTo));
        let a = Alignment::read_from(&g, ids[0]).unwrap();
        assert_eq!(a.target, Some(media));
        assert_eq!(a.words.len(), lines[0].1.len());
        assert!(!a.stale);
        assert_eq!(alignments_under(&g, scene).len(), 3);
    }
    // Re-applying the same alignment is a no-op.
    let again = engine
        .transact("align again", |tx| {
            let (dlg, words) = &lines[0];
            let transcript: Vec<TimedWord> = words
                .iter()
                .enumerate()
                .map(|(i, w)| TimedWord::new(w.clone(), i as f64 * 0.4, i as f64 * 0.4 + 0.3))
                .collect();
            apply_alignment(tx, *dlg, Some(media), align_words(words, &transcript))
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert!(again.deltas.is_empty());

    let mut count = 0;
    engine
        .transact("stale", |tx| {
            count =
                mark_alignments_stale(tx, scene).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
    let g = engine.read();
    for id in &ids {
        assert!(Alignment::read_from(&g, *id).unwrap().stale);
        assert_eq!(g.attr(*id, attrs::STALE).unwrap().as_bool(), Some(true));
    }
    // Retargeting drops the old SyncedTo edge.
    drop(g);
    let media2 = EntityId::new();
    engine
        .transact("retarget", |tx| {
            tx.create_with_id(media2, kinds::ASSET_MEDIA)?;
            apply_alignment(tx, lines[0].0, Some(media2), vec![])
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    assert!(!g.has_edge(ids[0], media, EdgeKind::SyncedTo));
    assert!(g.has_edge(ids[0], media2, EdgeKind::SyncedTo));
    assert!(!Alignment::read_from(&g, ids[0]).unwrap().stale);
}
