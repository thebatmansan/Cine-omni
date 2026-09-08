//! Graph round trips: grade entities, LUT / config registration, clip processors.

use cine_color::prelude::*;
use ucg_core::{EdgeKind, Engine, EntityId};
use ucg_schema::{attrs, kinds, Component};

#[test]
fn full_clip_pipeline() {
    let engine = Engine::new();
    let config = Config::aces_default();
    let (mut project, mut asset, mut clip) = (EntityId::nil(), EntityId::nil(), EntityId::nil());
    engine
        .transact("setup", |tx| {
            project = tx.create(kinds::PROJECT)?;
            asset = tx.create_child(project, kinds::ASSET_MEDIA)?;
            clip = tx.create(kinds::NLE_CLIP)?;
            tx.link(clip, asset, EdgeKind::Binds)?;
            ColorMeta {
                input_colorspace: "ARRI LogC4".into(),
                ..Default::default()
            }
            .write(tx, asset)?;
            Ok(())
        })
        .unwrap();

    // install config + a LUT, propagate meta, ensure grade
    let (mut cfg_id, mut lut_id, mut grade) = (EntityId::nil(), EntityId::nil(), EntityId::nil());
    let warm = Lut3D::from_fn(9, |c| [c[0] * 1.05, c[1], c[2] * 0.95]);
    engine
        .transact("install", |tx| {
            cfg_id = install_config(tx, project, &config)?;
            lut_id = register_lut(tx, project, "warm", &warm)?;
            assert!(propagate_color_meta(tx, asset, clip)?);
            grade = ensure_grade_for_clip(tx, clip)?;
            Ok(())
        })
        .unwrap();
    {
        let g = engine.read();
        assert_eq!(g.children(cfg_id).len(), config.colorspaces.len());
        assert_eq!(lut_id, EntityId::from_key("lut:warm"));
        assert!(g
            .attr(lut_id, "cube")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("TITLE"));
        assert!(g.has_edge(project, lut_id, EdgeKind::Contains));
        assert_eq!(
            ColorMeta::read_from(&g, clip).unwrap().input_colorspace,
            "ARRI LogC4"
        );
        assert_eq!(
            ColorMeta::read_from(&g, grade).unwrap().input_colorspace,
            "ARRI LogC4"
        );
        let loaded = cine_color::ucg::load_config(&g, cfg_id).unwrap();
        assert_eq!(loaded.colorspaces.len(), config.colorspaces.len());
        let cs_names: Vec<String> = g
            .children(cfg_id)
            .iter()
            .map(|c| g.node(*c).unwrap().name())
            .collect();
        assert!(cs_names.iter().any(|n| n == "ACEScg"));
        assert!(g
            .children(cfg_id)
            .iter()
            .all(|c| g.node(*c).unwrap().kind.as_str() == kinds::COLOR_SPACE));
    }

    // idempotence of the whole install
    let again = engine
        .transact("install again", |tx| {
            install_config(tx, project, &config)?;
            register_lut(tx, project, "warm", &warm)?;
            propagate_color_meta(tx, asset, clip)?;
            ensure_grade_for_clip(tx, clip)?;
            Ok(())
        })
        .unwrap();
    assert!(again.deltas.is_empty(), "re-install must be a no-op");

    // store / load a grade
    let stack = GradeStack::primary(vec![
        GradeOp::Exposure { stops: 1.0 },
        GradeOp::Saturation { amount: 0.8 },
    ]);
    engine
        .transact("grade", |tx| {
            store_grade(tx, grade, &stack).map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let g = engine.read();
    assert_eq!(load_grade(&g, grade).unwrap(), stack);
    assert_eq!(grades_for(&g, clip), vec![grade]);
    assert!(matches!(load_grade(&g, clip), Err(Error::NoGrade(_))));

    // processor for the clip: LogC4 → ACEScg → Rec.709 (first active view)
    let p = processor_for_clip(&g, clip, &config).unwrap();
    let code = Transfer::LogC4.encode(0.18) as f32;
    let o = p.apply_rgb(&[code; 3]);
    assert!(o[0] > 0.3 && o[0] < 0.5, "{o:?}");
    assert!((o[0] - o[1]).abs() < 1e-3);
    drop(g);

    // with a CDL and a look LUT in the working space
    engine
        .transact("meta", |tx| {
            let mut m = ColorMeta::read_from(tx.graph(), clip).unwrap();
            m.cdl.slope = [2.0, 2.0, 2.0];
            m.look_lut = Some(lut_id);
            m.display = "sRGB".into();
            m.write(tx, clip)
        })
        .unwrap();
    // Never hold the read guard across a transact (parking_lot RwLock).
    let o2 = {
        let g = engine.read();
        processor_for_clip(&g, clip, &config)
            .unwrap()
            .apply_rgb(&[code; 3])
    };
    assert!(o2[0] > o[0], "brighter with slope 2: {o2:?} vs {o:?}");
    assert!(o2[0] > o2[2], "warm LUT: {o2:?}");
    // display given as a colorspace name instead of a display
    engine
        .transact("meta2", |tx| {
            let mut m = ColorMeta::read_from(tx.graph(), clip).unwrap();
            m.display = "ACEScct".into();
            m.cdl = Cdl::default();
            m.look_lut = None;
            m.write(tx, clip)
        })
        .unwrap();
    let o3 = {
        let g = engine.read();
        processor_for_clip(&g, clip, &config)
            .unwrap()
            .apply_rgb(&[code; 3])
    };
    assert!((o3[0] - 0.4135884).abs() < 1e-4, "{o3:?}");
    // clip without metadata
    let other = engine.transact("c2", |tx| {
        let c = tx.create(kinds::NLE_CLIP)?;
        tx.set(c, attrs::NAME, "no meta")?;
        Ok(())
    });
    assert!(other.is_ok());
}

#[test]
fn errors_on_missing_entities() {
    let engine = Engine::new();
    let r = engine.transact("bad", |tx| {
        let missing = EntityId::new();
        match ensure_grade_for_clip(tx, missing) {
            Err(Error::Ucg(_)) => Ok(()),
            other => panic!("expected NotFound, got {other:?}"),
        }
    });
    assert!(r.is_ok());
    let g = engine.read();
    assert!(matches!(
        processor_for_clip(&g, EntityId::new(), &Config::aces_default()),
        Err(Error::Ucg(_))
    ));
}
