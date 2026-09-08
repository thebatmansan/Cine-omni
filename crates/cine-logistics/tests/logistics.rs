//! Integration tests: hand-built documents through breakdown, shot list,
//! schedule, call sheet, metadata and the graph writers.

use cine_logistics::breakdown::{to_csv, to_markdown};
use cine_logistics::fixtures::{self, CameraSetup, SampleDocument};
use cine_logistics::graph::*;
use cine_logistics::metadata::{Ramp, Step};
use cine_logistics::prelude::*;
use cine_logistics::shotlist::{shots_to_csv, shots_to_markdown, Support};
use ucg_core::{EdgeKind, Engine, EntityId};
use ucg_schema::{kinds, CameraMove, IntExt, ShotSize};

fn sample() -> (Engine, SampleDocument) {
    let engine = Engine::new();
    let mut doc = None;
    engine
        .transact("sample", |tx| {
            doc = Some(fixtures::build_sample_document(tx).map_err(Error::into_ucg)?);
            Ok(())
        })
        .expect("build sample");
    (engine, doc.expect("document"))
}

fn constraints() -> ScheduleConstraints {
    ScheduleConstraints {
        max_pages_per_day: 2.0,
        max_hours_per_day: 100.0,
        ..Default::default()
    }
}

#[test]
fn breakdown_of_six_scene_document() {
    let (engine, doc) = sample();
    let g = engine.read();
    let sheets = breakdown_document(&g, doc.document);
    assert_eq!(sheets.len(), 6);
    let numbers: Vec<&str> = sheets.iter().map(|s| s.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2", "3", "4", "5", "6"]);

    let s1 = &sheets[0];
    assert_eq!(s1.cast, vec!["ANNA", "BEN"]);
    assert_eq!(s1.props, vec!["KNIFE", "COFFEE MUG"]);
    assert_eq!(s1.page_eighths, 6);
    assert_eq!(s1.int_ext, IntExt::Int);
    assert_eq!(s1.location, "KITCHEN");
    assert_eq!(s1.time_of_day, "DAY");
    assert!((s1.est_minutes - 1.0).abs() < 1e-9);
    assert!(!s1.vehicles && !s1.animals && !s1.sfx && !s1.vfx);

    assert!(sheets[1].vehicles, "CAR should flag vehicles");
    assert!(sheets[2].is_night());
    assert!(sheets[3].animals, "DOG should flag animals");
    assert!(sheets[4].sfx && sheets[4].vfx);
    assert!(sheets[5].vehicles, "TAXI should flag vehicles");
    assert_eq!(sheets[2].cast.len(), 3);
    let total: u32 = sheets.iter().map(|s| s.page_eighths).sum();
    assert_eq!(total, 43);

    let md = to_markdown(&sheets);
    for n in ["| 1 |", "| 2 |", "| 3 |", "| 4 |", "| 5 |", "| 6 |"] {
        assert!(md.contains(n), "markdown missing {n}:\n{md}");
    }
    assert!(md.contains("5 3/8"));
    let csv = to_csv(&sheets);
    assert_eq!(csv.lines().count(), 7);
    assert!(csv.contains("ANNA; BEN"));

    // A single-scene call yields the same sheet; a bogus id yields an empty one.
    assert_eq!(breakdown_scene(&g, doc.scenes[0]), sheets[0]);
    let bogus = EntityId::new();
    assert_eq!(breakdown_scene(&g, bogus), SceneBreakdown::empty(bogus));
}

#[test]
fn shot_list_sizes_and_lens_grid() {
    let (engine, doc) = sample();
    let scene = doc.scenes[0];
    let setups = [
        CameraSetup::new(35.0, 1.0),
        CameraSetup::new(50.0, 1.0),
        CameraSetup::new(85.0, 1.0),
        CameraSetup::new(35.0, 3.0),
        CameraSetup::new(50.0, 3.0),
        CameraSetup::new(85.0, 3.0),
        CameraSetup::new(35.0, 12.0),
        CameraSetup::new(50.0, 12.0),
        CameraSetup {
            focal_mm: 85.0,
            distance_m: 12.0,
            movement: CameraMove::Dolly,
        },
    ];
    engine
        .transact("previs", |tx| {
            fixtures::add_previs(tx, scene, &setups, &["ANNA", "BEN"]).map_err(Error::into_ucg)?;
            Ok(())
        })
        .expect("previs");
    let g = engine.read();
    let shots = shot_list_for_scene(&g, scene);
    assert_eq!(shots.len(), 9);
    let sizes: Vec<ShotSize> = shots.iter().map(|s| s.spec.size).collect();
    assert_eq!(
        sizes,
        vec![
            ShotSize::CloseUp,
            ShotSize::ExtremeCloseUp,
            ShotSize::ExtremeCloseUp,
            ShotSize::MediumWide,
            ShotSize::Medium,
            ShotSize::CloseUp,
            ShotSize::Wide,
            ShotSize::Wide,
            ShotSize::Full,
        ]
    );
    let numbers: Vec<&str> = shots.iter().map(|s| s.spec.number.as_str()).collect();
    assert_eq!(numbers[..3], ["1A", "1B", "1C"]);
    for s in &shots {
        assert_eq!(s.scene, scene);
        assert_eq!(s.spec.scene, Some(scene));
        assert_eq!(s.spec.camera, Some(s.camera));
        assert!(
            s.spec.description.contains("ANNA"),
            "{}",
            s.spec.description
        );
        assert!(s.gear.filters.is_empty(), "interior needs no ND");
    }
    assert!((shots[0].spec.subject_distance_m - 1.0).abs() < 1e-9);
    assert!(
        shots[0].gear.needs_focus_puller,
        "85mm wide open at 1 m is razor thin"
    );
    assert_eq!(shots[8].gear.support, Support::Dolly);
    assert!(shots[8].gear.needs_focus_puller);
    assert!(shots[8].spec.est_duration_s > 0.0);

    let grid = lens_grid(&shots);
    assert_eq!(grid.sensor, "27.99x19.22mm");
    assert_eq!(grid.focal_lengths, vec![(35.0, 3), (50.0, 3), (85.0, 3)]);
    assert_eq!(grid.recommended_kit, vec![35.0, 50.0, 75.0]);
    assert!(grid.to_markdown().contains("75mm"));
    assert!(grid.to_csv().contains("85.0,3,75"));

    let md = shots_to_markdown(&shots);
    assert!(md.contains("| 1A |") && md.contains("| 1I |"));
    assert_eq!(shots_to_csv(&shots).lines().count(), 10);

    // A scene without previs yields no shots.
    assert!(shot_list_for_scene(&g, doc.scenes[1]).is_empty());
}

#[test]
fn schedule_from_document_and_call_sheets() {
    let (engine, doc) = sample();
    let g = engine.read();
    let sheets = breakdown_document(&g, doc.document);
    let board = schedule(&sheets, &constraints());
    assert_eq!(board.scenes().len(), 6);
    for d in &board.days {
        let nights: std::collections::BTreeSet<bool> =
            d.strips.iter().map(Strip::is_night).collect();
        assert_eq!(nights.len(), 1);
        assert!(d.total_eighths <= 16 || d.strips.len() == 1);
    }
    assert_eq!(
        board.cast_days("ANNA").len(),
        board
            .days
            .iter()
            .filter(|d| d.cast_on_call.contains(&"ANNA".to_string()))
            .count()
    );
    let md = board.to_markdown();
    assert!(md.contains("Day  1"));

    let roster = CastRoster::from_breakdowns(&sheets);
    assert_eq!(roster.number_of("ANNA"), Some(1));
    assert_eq!(roster.number_of("BEN"), Some(2));
    assert_eq!(roster.number_of("CARLA"), Some(3));
    assert_eq!(roster.number_of("DAVE"), Some(4));
    let call = Time::parse("07:00").unwrap();
    for day in &board.days {
        let sheet = build_call_sheet(day, &sheets, &roster, call);
        assert_eq!(sheet.scenes.len(), day.strips.len());
        let md = sheet.to_markdown();
        for strip in &day.strips {
            assert!(md.contains(&format!("| {} |", strip.number)));
        }
        for c in &sheet.cast_calls {
            assert_eq!(Some(c.number), roster.number_of(&c.name));
            assert_eq!(
                c.makeup.minutes_until(c.on_set),
                if roster.member(&c.name).unwrap().is_lead {
                    120
                } else {
                    90
                }
            );
        }
    }
}

#[test]
fn graph_writers_are_idempotent_and_readers_round_trip() {
    let (engine, doc) = sample();
    let scene = doc.scenes[0];
    engine
        .transact("previs", |tx| {
            fixtures::add_previs(
                tx,
                scene,
                &[CameraSetup::new(35.0, 2.0), CameraSetup::new(85.0, 2.0)],
                &["ANNA"],
            )
            .map_err(Error::into_ucg)?;
            Ok(())
        })
        .expect("previs");

    let (sheets, shots, board) = {
        let g = engine.read();
        let sheets = breakdown_document(&g, doc.document);
        let shots = shot_list_for_scene(&g, scene);
        let board = schedule(&sheets, &constraints());
        (sheets, shots, board)
    };
    assert_eq!(shots.len(), 2);

    let write_all = |label: &str| {
        engine
            .transact(label, |tx| {
                for bd in &sheets {
                    write_breakdown(tx, bd.scene, bd).map_err(Error::into_ucg)?;
                }
                write_shot_list(tx, scene, &shots).map_err(Error::into_ucg)?;
                write_schedule(tx, doc.document, &board).map_err(Error::into_ucg)?;
                Ok(())
            })
            .expect("write")
    };
    let first = write_all("write 1");
    assert!(!first.deltas.is_empty());
    let second = write_all("write 2");
    assert!(
        second.deltas.is_empty(),
        "second write produced ops: {:?}",
        second.deltas
    );

    let g = engine.read();
    assert!(g.validate().is_empty());
    // Breakdown node structure.
    let bid = breakdown_id(scene);
    assert!(g.has_edge(bid, scene, EdgeKind::DerivedFrom));
    assert_eq!(read_breakdown(&g, scene).as_ref(), Some(&sheets[0]));
    // Shot list structure.
    let list = shot_list_id(scene);
    assert!(g.has_edge(list, scene, EdgeKind::DerivedFrom));
    let stored = read_shot_list(&g, scene);
    assert_eq!(stored, shots);
    for s in &shots {
        let sid = shot_id(s.camera);
        assert!(g.has_edge(list, sid, EdgeKind::Contains));
        assert!(g.has_edge(sid, s.camera, EdgeKind::DependsOn));
        assert!(g.dependents(s.camera).contains(&sid));
    }
    // Schedule structure.
    let sid = schedule_id(doc.document);
    assert!(g.has_edge(sid, doc.document, EdgeKind::DerivedFrom));
    let day_ids = schedule_day_ids(&g, doc.document);
    assert_eq!(day_ids.len(), board.days.len());
    for (day, did) in board.days.iter().zip(&day_ids) {
        assert_eq!(*did, day_id(sid, day.index));
        assert!(g.has_edge(sid, *did, EdgeKind::Contains));
        for strip in &day.strips {
            assert!(
                g.has_edge(*did, strip.scene, EdgeKind::DependsOn),
                "day {} must depend on scene {}",
                day.index,
                strip.number
            );
        }
        assert_eq!(g.targets(*did, EdgeKind::DependsOn).len(), day.strips.len());
    }
    assert_eq!(read_schedule(&g, doc.document).as_ref(), Some(&board));
    assert_eq!(document_of_scene(&g, scene), Some(doc.document));
    assert_eq!(read_schedule(&g, EntityId::new()), None);
}

#[test]
fn rewrite_removes_surplus_days_and_vanished_shots() {
    let (engine, doc) = sample();
    let scene = doc.scenes[0];
    engine
        .transact("previs", |tx| {
            fixtures::add_previs(
                tx,
                scene,
                &[CameraSetup::new(35.0, 2.0), CameraSetup::new(85.0, 2.0)],
                &["ANNA"],
            )
            .map_err(Error::into_ucg)?;
            Ok(())
        })
        .expect("previs");
    let (sheets, shots) = {
        let g = engine.read();
        (
            breakdown_document(&g, doc.document),
            shot_list_for_scene(&g, scene),
        )
    };
    let long = schedule(
        &sheets,
        &ScheduleConstraints {
            max_pages_per_day: 0.5,
            ..constraints()
        },
    );
    let short = schedule(
        &sheets,
        &ScheduleConstraints {
            max_pages_per_day: 10.0,
            ..constraints()
        },
    );
    assert!(long.days.len() > short.days.len());

    engine
        .transact("long", |tx| {
            write_schedule(tx, doc.document, &long).map_err(Error::into_ucg)?;
            write_shot_list(tx, scene, &shots).map_err(Error::into_ucg)?;
            Ok(())
        })
        .expect("write long");
    engine
        .transact("short", |tx| {
            write_schedule(tx, doc.document, &short).map_err(Error::into_ucg)?;
            write_shot_list(tx, scene, &shots[..1]).map_err(Error::into_ucg)?;
            Ok(())
        })
        .expect("write short");
    let g = engine.read();
    assert_eq!(schedule_day_ids(&g, doc.document).len(), short.days.len());
    assert_eq!(read_schedule(&g, doc.document).as_ref(), Some(&short));
    assert_eq!(read_shot_list(&g, scene).len(), 1);
    assert!(!g.contains(shot_id(shots[1].camera)));
    assert!(g.validate().is_empty());

    let changes = diff(&long, &short);
    assert!(!changes.is_empty());
    assert!(changes
        .describe()
        .iter()
        .any(|l| l.contains("moved from Day")));
}

#[test]
fn writers_reject_missing_targets() {
    let (engine, doc) = sample();
    let g = engine.read();
    let sheets = breakdown_document(&g, doc.document);
    drop(g);
    let missing = EntityId::new();
    let r = engine.transact("bad", |tx| {
        write_breakdown(tx, missing, &sheets[0])
            .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
        Ok(())
    });
    assert!(r.is_err());
    let r = engine.transact("bad", |tx| {
        write_schedule(tx, missing, &Schedule::default())
            .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
        Ok(())
    });
    assert!(r.is_err());
}

#[test]
fn call_sheet_and_take_metadata_round_trip() {
    let (engine, doc) = sample();
    let scene = doc.scenes[0];
    engine
        .transact("previs", |tx| {
            fixtures::add_previs(tx, scene, &[CameraSetup::new(50.0, 2.0)], &["ANNA"])
                .map_err(Error::into_ucg)?;
            Ok(())
        })
        .expect("previs");
    let (sheets, shots) = {
        let g = engine.read();
        (
            breakdown_document(&g, doc.document),
            shot_list_for_scene(&g, scene),
        )
    };
    let board = schedule(&sheets, &constraints());
    let roster = CastRoster::from_breakdowns(&sheets);
    let sheet = build_call_sheet(&board.days[0], &sheets, &roster, Time::new(6, 30).unwrap());

    let frames = simulate_take(
        10.0,
        24.0,
        &TakeScenario {
            zoom: Some(Ramp {
                start_s: 1.0,
                end_s: 3.0,
                to: 75.0,
            }),
            aperture_change: Some(Step { at_s: 5.0, to: 5.6 }),
            ..Default::default()
        },
    );
    let summary = summarize(&frames);
    let events = detect_events(&frames);
    assert_eq!(events.len(), 2);

    let mut ids = (EntityId::nil(), EntityId::nil(), EntityId::nil());
    engine
        .transact("write", |tx| {
            write_schedule(tx, doc.document, &board).map_err(Error::into_ucg)?;
            let day = day_id(schedule_id(doc.document), 1);
            let cs = write_call_sheet(tx, day, &sheet).map_err(Error::into_ucg)?;
            let list = write_shot_list(tx, scene, &shots).map_err(Error::into_ucg)?;
            let shot = shot_id(shots[0].camera);
            let take =
                write_take_metadata(tx, shot, 3, &summary, &events).map_err(Error::into_ucg)?;
            ids = (cs, list, take);
            Ok(())
        })
        .expect("write");
    let again = engine
        .transact("write again", |tx| {
            let day = day_id(schedule_id(doc.document), 1);
            write_call_sheet(tx, day, &sheet).map_err(Error::into_ucg)?;
            write_take_metadata(tx, shot_id(shots[0].camera), 3, &summary, &events)
                .map_err(Error::into_ucg)?;
            Ok(())
        })
        .expect("write again");
    assert!(again.deltas.is_empty());

    let g = engine.read();
    let day = day_id(schedule_id(doc.document), 1);
    assert!(g.has_edge(ids.0, day, EdgeKind::DerivedFrom));
    assert_eq!(read_call_sheet(&g, day).as_ref(), Some(&sheet));
    let shot = shot_id(shots[0].camera);
    assert!(g.has_edge(shot, ids.2, EdgeKind::Contains));
    assert_eq!(takes_of_shot(&g, shot), vec![ids.2]);
    assert_eq!(g.node(ids.2).unwrap().kind.as_str(), kinds::LOGISTICS_TAKE);
    let (s, e) = read_take_metadata(&g, ids.2).expect("take metadata");
    assert_eq!(s, summary);
    assert_eq!(e, events);
    assert!(g.validate().is_empty());
}

#[test]
fn schedule_diff_after_editing_a_scene() {
    let (engine, doc) = sample();
    let before = {
        let g = engine.read();
        schedule(&breakdown_document(&g, doc.document), &constraints())
    };
    engine
        .transact("edit", |tx| {
            tx.set(doc.scenes[0], ucg_schema::attrs::PAGE_EIGHTHS, 20i64)
        })
        .expect("edit");
    let after = {
        let g = engine.read();
        schedule(&breakdown_document(&g, doc.document), &constraints())
    };
    let d = diff(&before, &after);
    assert!(!d.is_empty());
    assert!(d
        .moved
        .iter()
        .any(|m| m.from_day.is_some() && m.to_day.is_some()));
    let text = d.describe().join("\n");
    assert!(
        text.contains("moved from Day") || text.contains("days"),
        "{text}"
    );
}
