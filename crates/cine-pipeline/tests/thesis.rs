//! End-to-end test of the CINE-OMNI thesis: one edit in the screenplay
//! propagates through previs, logistics, editorial, audio, comp and color,
//! and undo reverts all of it as a unit.

use cine_pipeline::{OmniProject, ProjectSetup};
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds, Component};

const SCRIPT: &str = r#"Title: Thesis Test
Author: CINE-OMNI

INT. KITCHEN - NIGHT #1#

MARA stands at the sink, a chipped MUG in her hand. Rain hammers the window.

MARA
We ride at dawn.

JONAH
(quietly)
You always say that.

MARA
This time I mean it.

CUT TO:

EXT. DESERT ROAD - DAY #2#

A battered TRUCK crawls along the horizon. JONAH drives. MARA sleeps.

JONAH
Wake up. We're here.

INT. GAS STATION - DAY #3#

The CLERK looks up from a crossword.

CLERK
You folks lost?

MARA
Not anymore.
"#;

const OTIO: &str = r#"{
  "OTIO_SCHEMA": "Timeline.1",
  "name": "Cut v1",
  "global_start_time": null,
  "metadata": {},
  "tracks": {
    "OTIO_SCHEMA": "Stack.1",
    "name": "tracks",
    "metadata": {},
    "markers": [],
    "children": [
      {
        "OTIO_SCHEMA": "Track.1",
        "name": "V1",
        "kind": "Video",
        "metadata": {},
        "markers": [],
        "children": [
          {
            "OTIO_SCHEMA": "Clip.2",
            "name": "A001_C001",
            "metadata": {},
            "markers": [],
            "source_range": {
              "OTIO_SCHEMA": "TimeRange.1",
              "start_time": { "OTIO_SCHEMA": "RationalTime.1", "value": 10, "rate": 24 },
              "duration": { "OTIO_SCHEMA": "RationalTime.1", "value": 48, "rate": 24 }
            },
            "active_media_reference_key": "DEFAULT_MEDIA",
            "media_references": {
              "DEFAULT_MEDIA": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "name": "",
                "metadata": {},
                "target_url": "/media/A001_C001.mov",
                "available_range": {
                  "OTIO_SCHEMA": "TimeRange.1",
                  "start_time": { "OTIO_SCHEMA": "RationalTime.1", "value": 0, "rate": 24 },
                  "duration": { "OTIO_SCHEMA": "RationalTime.1", "value": 240, "rate": 24 }
                }
              }
            }
          }
        ]
      }
    ]
  }
}"#;

fn setup() -> (OmniProject, EntityId, Vec<EntityId>) {
    let project = OmniProject::new(ProjectSetup::default()).unwrap();
    let doc = project.import_fountain(SCRIPT, "thesis.fountain").unwrap();
    let scenes = project.scenes(doc);
    assert_eq!(scenes.len(), 3);
    (project, doc, scenes)
}

#[test]
fn import_derives_every_module() {
    let (project, doc, scenes) = setup();
    let g = project.engine.read();
    let s1 = scenes[0];
    let scene = g.node(s1).unwrap();
    assert_eq!(scene.get_str(attrs::SCENE_NUMBER), Some("1"));
    assert_eq!(scene.get_int(attrs::WORDS), Some(13));
    let chars = scene
        .get(attrs::CHARACTERS)
        .unwrap()
        .as_list()
        .unwrap()
        .len();
    assert_eq!(chars, 2);

    // Previs derived scene with characters, camera, dialogue track.
    let previs = EntityId::derived(s1, kinds::PREVIS_SCENE);
    assert!(g.contains(previs), "previs scene derived");
    assert!(g.has_edge(previs, s1, EdgeKind::DerivedFrom));
    let previs_chars: Vec<_> = g
        .children(previs)
        .into_iter()
        .filter(|c| g.node(*c).unwrap().kind.as_str() == kinds::PREVIS_CHARACTER)
        .collect();
    assert_eq!(previs_chars.len(), 2);
    assert!(!cine_previs::graph::cameras_of(&g, previs).is_empty());
    let track = EntityId::derived(previs, "dialogue_track");
    let range = g
        .node(track)
        .unwrap()
        .get(attrs::RANGE)
        .unwrap()
        .as_range()
        .unwrap();
    assert!(range.duration.to_seconds() > 1.0);

    // Logistics: breakdown, shot list, schedule with days, call sheets.
    assert!(g.contains(EntityId::derived(s1, kinds::LOGISTICS_BREAKDOWN)));
    let shot_list = EntityId::derived(s1, kinds::LOGISTICS_SHOT_LIST);
    assert!(g.contains(shot_list));
    let shots = g.children(shot_list);
    assert!(!shots.is_empty());
    let schedule = EntityId::derived(doc, kinds::LOGISTICS_SCHEDULE);
    assert!(g.contains(schedule), "schedule derived");
    let days = g.children(schedule);
    assert!(!days.is_empty());
    assert!(g.contains(EntityId::derived(days[0], kinds::LOGISTICS_CALL_SHEET)));

    // Comp: every shot has a comp graph depending on it, bound to the camera.
    for shot in &shots {
        let comp = EntityId::derived(*shot, kinds::COMP_GRAPH);
        assert!(g.contains(comp), "comp for shot");
        assert!(g.has_edge(comp, *shot, EdgeKind::DependsOn));
        assert!(g.node(comp).unwrap().get(attrs::RANGE).is_some());
    }

    // Audio: dialogue placeholders, one per line.
    let dx = EntityId::derived(s1, kinds::AUDIO_TRACK);
    assert!(g.contains(dx));
    assert_eq!(g.children(dx).len(), 3);

    assert!(g.validate().is_empty());
}

#[test]
fn dialogue_edit_cascades_and_undoes_as_unit() {
    let (project, _doc, scenes) = setup();
    let s1 = scenes[0];
    let lines = project.dialogue_lines(s1);
    assert_eq!(lines.len(), 3);

    // Bind an editorial clip to scene 1 so the timeline reactor has work.
    let timeline = project.import_otio(OTIO, "cut_v1.otio").unwrap();
    let clip = {
        let g = project.engine.read();
        g.of_kind(kinds::NLE_CLIP)[0].id
    };
    project
        .engine
        .transact("bind clip", |tx| {
            cine_timeline::graph::bind_clip_to_scene(tx, clip, s1)
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let _ = timeline;

    let (words_before, dur_before, day_count_before) = {
        let g = project.engine.read();
        let s = g.node(s1).unwrap();
        let sched = EntityId::derived(
            g.ancestor_of_kind(s1, kinds::SCRIPT_DOCUMENT).unwrap(),
            kinds::LOGISTICS_SCHEDULE,
        );
        (
            s.get_int(attrs::WORDS).unwrap(),
            s.get_float(attrs::EST_DURATION_S).unwrap(),
            g.children(sched).len(),
        )
    };
    let rx = project.engine.subscribe_channel();
    let undo_before = project.engine.undo_depth();

    // THE EDIT: one line of dialogue becomes a monologue.
    let long_line = "We ride at dawn, and we do not stop until the mountains are behind us and the river is in front of us and the whole valley is nothing but a story we tell strangers.";
    let group = project.edit_dialogue(lines[0], long_line).unwrap();
    assert!(
        group.deltas.len() >= 5,
        "cascade produced {} deltas",
        group.deltas.len()
    );
    let cascade: Vec<String> = group.deltas.iter().map(|d| d.label.clone()).collect();
    for module in ["script", "previs", "logistics", "timeline", "audio", "comp"] {
        assert!(
            cascade.iter().any(|l| l.starts_with(module)),
            "{module} did not react: {cascade:?}"
        );
    }
    // Every reactor transaction has a cause chain back to the user's edit.
    let root_rev = group.deltas[0].revision;
    for d in &group.deltas[1..] {
        assert!(d.cause.is_some() && d.depth >= 1, "{}", d.label);
    }
    let _ = root_rev;
    drop(rx);

    {
        let g = project.engine.read();
        let s = g.node(s1).unwrap();
        assert!(s.get_int(attrs::WORDS).unwrap() > words_before);
        assert!(s.get_float(attrs::EST_DURATION_S).unwrap() > dur_before);
        // 1. previs dialogue track updated
        let previs = EntityId::derived(s1, kinds::PREVIS_SCENE);
        let track = EntityId::derived(previs, "dialogue_track");
        let range = g
            .node(track)
            .unwrap()
            .get(attrs::RANGE)
            .unwrap()
            .as_range()
            .unwrap();
        assert!(
            (range.duration.to_seconds() - s.get_float(attrs::EST_DURATION_S).unwrap()).abs() < 0.1
        );
        // 2. logistics recalculated (breakdown minutes follow the estimate)
        let bd = g
            .node(EntityId::derived(s1, kinds::LOGISTICS_BREAKDOWN))
            .unwrap();
        let bd_json = bd.get("breakdown").unwrap().to_json();
        assert!(bd_json["est_minutes"].as_f64().unwrap() > dur_before / 60.0);
        let _ = day_count_before;
        // 3. timeline flagged the clip
        let c = g.node(clip).unwrap();
        assert_eq!(c.get_bool(attrs::NEEDS_CONFORM), Some(true));
        assert!(c.get_str(attrs::DIRTY_REASON).unwrap().contains("KITCHEN"));
        // 4. comp graphs on the scene's shots are stale with refreshed ranges
        let shot_list = EntityId::derived(s1, kinds::LOGISTICS_SHOT_LIST);
        for shot in g.children(shot_list) {
            let comp = g.node(EntityId::derived(shot, kinds::COMP_GRAPH)).unwrap();
            assert_eq!(comp.get_bool(attrs::STALE), Some(true));
        }
        // 5. color metadata survived on the clip's grade
        let grade = EntityId::derived(clip, kinds::COLOR_GRADE);
        assert!(g.contains(grade), "grade ensured for bound clip");
        let meta = ucg_schema::ColorMeta::read_from(&g, grade).unwrap();
        assert_eq!(meta.working_colorspace, "ACEScg");
        // audio placeholder for that line grew
        let dx = EntityId::derived(s1, kinds::AUDIO_TRACK);
        let clip0 = EntityId::derived(lines[0], kinds::AUDIO_CLIP);
        assert!(g.children(dx).contains(&clip0));
        let r = g
            .node(clip0)
            .unwrap()
            .get(attrs::RANGE)
            .unwrap()
            .as_range()
            .unwrap();
        assert!(r.duration.to_seconds() > 5.0);
        assert!(g.validate().is_empty());
    }

    // Undo the edit: the whole cascade reverts in one step.
    assert_eq!(project.engine.undo_depth(), undo_before + 1);
    project.engine.undo().unwrap();
    {
        let g = project.engine.read();
        let s = g.node(s1).unwrap();
        assert_eq!(s.get_int(attrs::WORDS), Some(words_before));
        assert_eq!(
            g.node(lines[0]).unwrap().get_str(attrs::TEXT),
            Some("We ride at dawn.")
        );
        assert_ne!(
            g.node(clip).unwrap().get_bool(attrs::NEEDS_CONFORM),
            Some(true)
        );
        let previs = EntityId::derived(s1, kinds::PREVIS_SCENE);
        let track = EntityId::derived(previs, "dialogue_track");
        let range = g
            .node(track)
            .unwrap()
            .get(attrs::RANGE)
            .unwrap()
            .as_range()
            .unwrap();
        assert!((range.duration.to_seconds() - dur_before).abs() < 0.1);
        assert!(g.validate().is_empty());
    }
    project.engine.redo().unwrap();
    let g = project.engine.read();
    assert_eq!(
        g.node(clip).unwrap().get_bool(attrs::NEEDS_CONFORM),
        Some(true)
    );
}

#[test]
fn reimport_is_stable_and_idempotent() {
    let (project, doc, scenes) = setup();
    let rev = project.engine.revision();
    let doc2 = project.import_fountain(SCRIPT, "thesis.fountain").unwrap();
    assert_eq!(doc, doc2);
    assert_eq!(
        project.engine.revision(),
        rev,
        "identical re-import produces no transactions"
    );

    // Change one line in the text and re-import: same scene ids, cascade runs.
    let changed = SCRIPT.replace("You always say that.", "You always say that, Mara.");
    let doc3 = project
        .import_fountain(&changed, "thesis.fountain")
        .unwrap();
    assert_eq!(doc, doc3);
    assert_eq!(project.scenes(doc), scenes);
    let out = project.export_fountain(doc).unwrap();
    assert!(out.contains("You always say that, Mara."));
    let g = project.engine.read();
    assert!(g.validate().is_empty());
}

#[test]
fn adding_and_removing_scenes_reschedules() {
    let (project, doc, _scenes) = setup();
    let schedule = EntityId::derived(doc, kinds::LOGISTICS_SCHEDULE);
    let strips_before: usize = {
        let g = project.engine.read();
        g.children(schedule)
            .iter()
            .map(|d| g.targets(*d, EdgeKind::DependsOn).len())
            .sum()
    };
    let more = format!("{SCRIPT}\nEXT. RIVERBANK - DUSK #4#\n\nMARA and JONAH watch the water.\n\nMARA\nTold you.\n");
    project.import_fountain(&more, "thesis.fountain").unwrap();
    let strips_after: usize = {
        let g = project.engine.read();
        g.children(schedule)
            .iter()
            .map(|d| g.targets(*d, EdgeKind::DependsOn).len())
            .sum()
    };
    assert_eq!(strips_after, strips_before + 1);

    project.import_fountain(SCRIPT, "thesis.fountain").unwrap();
    let g = project.engine.read();
    let strips_final: usize = g
        .children(schedule)
        .iter()
        .map(|d| g.targets(*d, EdgeKind::DependsOn).len())
        .sum();
    assert_eq!(strips_final, strips_before);
    assert_eq!(project.scenes(doc).len(), 3);
    // Derived previs of the removed scene must be gone too (no dangling derived nodes).
    let s4 = EntityId::from_key("script:thesis.fountain:scene:4");
    assert!(!g.contains(s4));
    assert!(g.validate().is_empty());
}

#[test]
fn exports_round_trip_through_industry_formats() {
    let (project, doc, scenes) = setup();
    let fdx = project.export_fdx(doc).unwrap();
    assert!(fdx.contains("<FinalDraft"));
    let usda = project.export_usda(scenes[0]).unwrap();
    assert!(usda.starts_with("#usda 1.0"));
    assert!(usda.contains("def Camera"));
    let timeline = project.import_otio(OTIO, "cut.otio").unwrap();
    let otio = project.export_otio(timeline).unwrap();
    assert!(otio.contains("\"Timeline.1\""));
    assert!(
        otio.contains("ucg_id"),
        "exported OTIO carries UCG ids: {otio}"
    );
}

#[test]
fn project_persists_and_reopens_with_reactors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Thesis.cineomni");
    let scenes;
    {
        let project = OmniProject::create(
            &path,
            ProjectSetup {
                name: "Thesis".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let doc = project.import_fountain(SCRIPT, "thesis.fountain").unwrap();
        scenes = project.scenes(doc);
        project.save().unwrap();
        // A change after save lives in the op log only.
        let lines = project.dialogue_lines(scenes[2]);
        project
            .edit_dialogue(lines[0], "You folks lost, or just passing through?")
            .unwrap();
    }
    let reopened = OmniProject::open(&path).unwrap();
    let g = reopened.engine.read();
    let lines = reopened.dialogue_lines(scenes[2]);
    assert_eq!(
        g.node(lines[0]).unwrap().get_str(attrs::TEXT),
        Some("You folks lost, or just passing through?")
    );
    assert!(g.contains(EntityId::derived(scenes[2], kinds::PREVIS_SCENE)));
    assert!(g.validate().is_empty());
    assert_eq!(reopened.engine.reactor_names().len(), 7);
}
