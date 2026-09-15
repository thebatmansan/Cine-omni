//! Media has a colour identity: the project's config decides it from the
//! file's path, the decision is recorded, someone can override it, and it
//! reaches every clip and grade without disturbing the grade itself.

use cine_color::ucg::{ATTR_CONFIG_SOURCE, ATTR_INPUT_SOURCE};
use cine_pipeline::project::{OmniProject, ProjectSetup};
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds, ColorMeta, Component};

fn media(project: &OmniProject, path: &str) -> EntityId {
    let id = EntityId::from_key(&format!("test-media:{path}"));
    project
        .engine
        .transact("ingest", |tx| {
            tx.ensure(id, kinds::ASSET_MEDIA)?;
            tx.set(id, attrs::MEDIA_PATH, path)?;
            tx.link(project.root, id, EdgeKind::Contains)
        })
        .unwrap();
    id
}

fn input_of(project: &OmniProject, id: EntityId) -> (String, String) {
    let g = project.engine.read();
    let meta = ColorMeta::read_from(&g, id).expect("colour metadata");
    let source = g
        .node(id)
        .unwrap()
        .get_str(ATTR_INPUT_SOURCE)
        .unwrap_or("")
        .to_string();
    (meta.input_colorspace, source)
}

#[test]
fn a_new_project_has_the_bundled_config_and_it_is_not_an_undoable_edit() {
    let project = OmniProject::new(ProjectSetup::default()).unwrap();
    let depth = project.engine.undo_depth();
    let g = project.engine.read();
    let config = cine_color::ucg::project_config(&g, project.root).expect("a config");
    let source = g
        .node(config)
        .unwrap()
        .get_str(ATTR_CONFIG_SOURCE)
        .unwrap()
        .to_string();
    assert!(
        source.contains("cg-config-v4.0.0_aces-v2.0_ocio-v2.5"),
        "{source}"
    );
    drop(g);
    // Undoing everything the user did does not remove the config.
    for _ in 0..depth {
        project.engine.undo().unwrap();
    }
    assert!(cine_color::ucg::project_config(&project.engine.read(), project.root).is_some());
}

#[test]
fn the_file_rules_decide_and_say_which_rule_decided() {
    let project = OmniProject::new(ProjectSetup::default()).unwrap();
    let movie = media(&project, "/footage/A001C003.MOV");
    let plate = media(&project, "/plates/sh010_bg.1001.exr");
    let still = media(&project, "/refs/lookbook.jpg");
    assert_eq!(
        input_of(&project, movie),
        ("Rec.1886 Rec.709 - Display".into(), "rule:Movies".into())
    );
    assert_eq!(
        input_of(&project, plate),
        ("ACES2065-1".into(), "rule:EXR".into())
    );
    assert_eq!(
        input_of(&project, still),
        ("sRGB - Display".into(), "rule:Default".into())
    );
    let g = project.engine.read();
    let meta = ColorMeta::read_from(&g, movie).unwrap();
    assert_eq!(
        meta.working_colorspace, "ACEScg",
        "the config's scene_linear role"
    );
}

#[test]
fn a_chosen_input_colorspace_reaches_the_grade_and_leaves_its_cdl_alone() {
    let project = OmniProject::new(ProjectSetup::default()).unwrap();
    let footage = media(&project, "/footage/A001C003.mov");
    let clip = EntityId::from_key("test-clip:1");
    project
        .engine
        .transact("place", |tx| {
            tx.ensure(clip, kinds::NLE_CLIP)?;
            tx.link(project.root, clip, EdgeKind::Contains)?;
            tx.link(clip, footage, EdgeKind::Binds)
        })
        .unwrap();
    let grade = EntityId::derived(clip, kinds::COLOR_GRADE);
    assert_eq!(input_of(&project, grade).0, "Rec.1886 Rec.709 - Display");

    // A colourist's CDL on the grade.
    project
        .engine
        .transact("grade", |tx| {
            let mut meta = ColorMeta::read_from(tx.graph(), grade).unwrap();
            meta.cdl.slope = [1.2, 1.0, 0.9];
            meta.write(tx, grade)
        })
        .unwrap();

    // The DIT says the footage is really ACEScct.
    project
        .engine
        .transact("set input colorspace", |tx| {
            let config = cine_color::ucg::project_config(tx.graph(), project.root).unwrap();
            let config = cine_color::ucg::load_config(tx.graph(), config).unwrap();
            cine_color::ucg::set_input_colorspace(tx, footage, &config, "acescct")
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    assert_eq!(
        input_of(&project, footage),
        ("ACEScct".into(), "user".into())
    );
    assert_eq!(input_of(&project, clip).0, "ACEScct");
    let g = project.engine.read();
    let graded = ColorMeta::read_from(&g, grade).unwrap();
    assert_eq!(graded.input_colorspace, "ACEScct");
    assert_eq!(graded.cdl.slope, [1.2, 1.0, 0.9], "the grade survived");
    drop(g);

    // Moving the file does not overrule a person.
    project
        .engine
        .transact("relink", |tx| {
            tx.set(footage, attrs::MEDIA_PATH, "/archive/A001C003.mov")
        })
        .unwrap();
    assert_eq!(
        input_of(&project, footage),
        ("ACEScct".into(), "user".into())
    );

    // Undo the relink and the choice: the rule's decision is back, everywhere.
    project.engine.undo().unwrap();
    project.engine.undo().unwrap();
    assert_eq!(
        input_of(&project, footage),
        ("Rec.1886 Rec.709 - Display".into(), "rule:Movies".into())
    );
    assert_eq!(input_of(&project, grade).0, "Rec.1886 Rec.709 - Display");

    // An unknown name is refused, not stored.
    let refused = project.engine.transact("bad", |tx| {
        let config = cine_color::ucg::project_config(tx.graph(), project.root).unwrap();
        let config = cine_color::ucg::load_config(tx.graph(), config).unwrap();
        cine_color::ucg::set_input_colorspace(tx, footage, &config, "Rec.709 (made up)")
            .map_err(|e| UcgError::Aborted(e.to_string()))
    });
    assert!(refused.is_err());
}
