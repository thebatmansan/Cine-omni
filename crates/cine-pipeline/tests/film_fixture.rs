//! The end-to-end film fixture: one project carried across every domain.
//!
//! This is the test the whole architecture exists to pass. A screenplay becomes
//! scenes; scenes become previs cameras; cameras become shots; shots are shot,
//! producing takes; takes bind to real media on disk. Then the screenplay
//! changes, and the question the Unified Context Graph exists to answer —
//! *what does this actually affect?* — has to come out right in both
//! directions: derived work goes stale, and captured work survives.
//!
//! ```text
//!   screenplay ─▶ scene ─▶ previs camera ─▶ shot ─▶ take ─▶ media
//!                   │                        │        │
//!                   └── breakdown, schedule ─┘        └── content hash
//! ```
//!
//! It uses **real media**. When the asset is absent the test skips loudly
//! rather than passing on a stub, because a chain-of-custody test with no
//! custody proves nothing.

use cine_logistics::graph::{read_shot_list, takes_of_shot, write_take_metadata};
use cine_media::ucg as media_ucg;
use cine_pipeline::{OmniProject, ProjectSetup};
use std::path::PathBuf;
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds};

const SCREENPLAY: &str = r#"Title: Fixture
Author: CINE-OMNI

INT. HARBOR OFFICE - NIGHT #1#

MAREN crosses the room. A LAMP flickers on the desk.

MAREN
We ride at dawn and we do not look back.

ELIAS
Then we had better sleep.

EXT. DOCKS - DAWN #2#

ELIAS waits by a rusted CRANE.

ELIAS
You came.
"#;

fn real_media() -> Option<PathBuf> {
    let p = PathBuf::from("/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4");
    p.exists().then_some(p)
}

macro_rules! need {
    ($opt:expr, $what:expr) => {
        match $opt {
            Some(v) => v,
            None => {
                eprintln!("SKIP: no {} available on this machine", $what);
                return;
            }
        }
    };
}

/// A project with a screenplay imported and the cascade settled.
struct Film {
    project: OmniProject,
    document: EntityId,
    scenes: Vec<EntityId>,
    dir: tempfile::TempDir,
}

fn build_film() -> Film {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = OmniProject::create(
        dir.path(),
        ProjectSetup {
            name: "Fixture".into(),
            ..Default::default()
        },
    )
    .expect("create project");
    let document = project
        .import_fountain(SCREENPLAY, "fixture.fountain")
        .expect("import");
    let scenes = project.scenes(document);
    assert_eq!(scenes.len(), 2, "the fixture screenplay has two scenes");
    Film {
        project,
        document,
        scenes,
        dir,
    }
}

/// The first shot the logistics reactor derived for a scene.
fn first_shot(film: &Film, scene: EntityId) -> Option<EntityId> {
    let g = film.project.engine.read();
    let list = cine_logistics::graph::shot_list_id(scene);
    g.children(list)
        .into_iter()
        .filter(|c| {
            g.node(*c)
                .map(|n| n.kind.as_str() == kinds::LOGISTICS_SHOT)
                .unwrap_or(false)
        })
        .min_by_key(|c| {
            g.node(*c)
                .and_then(|n| n.get_int(attrs::ORDER))
                .unwrap_or(0)
        })
}

#[test]
fn a_screenplay_becomes_scenes_shots_takes_and_media_in_one_graph() {
    let media_path = need!(real_media(), "real media");
    let film = build_film();
    let scene = film.scenes[0];

    // The cascade must already have produced the production entities. Nothing
    // in this test creates a shot by hand.
    let shot = first_shot(&film, scene).expect("the logistics reactor derived a shot");
    {
        let g = film.project.engine.read();
        assert!(
            !read_shot_list(&g, scene).is_empty(),
            "scene 1 has a shot list"
        );
    }

    // Shoot three takes against that shot, and ingest the real media.
    let mut media = EntityId::nil();
    let mut takes = Vec::new();
    film.project
        .engine
        .transact("shoot", |tx| {
            media = media_ucg::ingest(tx, film.project.root, &media_path)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            for n in 1..=3u32 {
                let take = write_take_metadata(tx, shot, n, &Default::default(), &[])
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
                // The take is what was actually shot; it binds to the media.
                media_ucg::bind_to_media(tx, take, media)
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
                takes.push(take);
            }
            Ok(())
        })
        .expect("shoot the scene");

    let g = film.project.engine.read();

    // The chain is traversable end to end.
    assert_eq!(takes_of_shot(&g, shot).len(), 3, "three takes on the shot");
    for take in &takes {
        let bound: Vec<EntityId> = g.targets(*take, EdgeKind::Binds);
        assert!(
            bound.contains(&media),
            "a take must be traceable to the exact source media it came from"
        );
    }

    // Media identity is by content, and the graph holds enough to relink it.
    let info = media_ucg::read_media(&g, media).expect("media info stored");
    assert!(info.hash.size() > 1_000_000, "real media, not a stub");
    assert_eq!(
        media_ucg::find_by_hash(&g, info.hash.hex()),
        Some(media),
        "the media must be findable by content hash, which is what relinking uses"
    );
    assert!(
        g.validate().is_empty(),
        "the graph is valid after the whole chain"
    );
}

/// The question the UCG exists to answer, asked in both directions at once.
#[test]
fn an_upstream_script_edit_stales_derived_work_and_preserves_captured_work() {
    let media_path = need!(real_media(), "real media");
    let film = build_film();
    let scene = film.scenes[0];
    let shot = first_shot(&film, scene).expect("a shot");

    let mut media = EntityId::nil();
    let mut takes = Vec::new();
    film.project
        .engine
        .transact("shoot", |tx| {
            media = media_ucg::ingest(tx, film.project.root, &media_path)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            for n in 1..=3u32 {
                let take = write_take_metadata(tx, shot, n, &Default::default(), &[])
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
                media_ucg::bind_to_media(tx, take, media)
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
                takes.push(take);
            }
            Ok(())
        })
        .expect("shoot");

    let duration_before = {
        let g = film.project.engine.read();
        g.node(scene)
            .and_then(|n| n.get_float(attrs::EST_DURATION_S))
    };

    // Rewrite a line so the scene genuinely gets longer. This is an upstream
    // change with real downstream consequences.
    let line = film.project.dialogue_lines(scene)[0];
    let group = film
        .project
        .edit_dialogue(
            line,
            "We ride at dawn, and we do not look back, and we do not speak of \
             what we leave behind us on this shore.",
        )
        .expect("edit");

    assert!(
        !group.deltas.is_empty(),
        "the edit produced no cascade at all"
    );

    // (a) Derived work responded.
    let duration_after = {
        let g = film.project.engine.read();
        g.node(scene)
            .and_then(|n| n.get_float(attrs::EST_DURATION_S))
    };
    assert!(
        duration_after > duration_before,
        "a longer scene must produce a longer estimated duration \
         (before {duration_before:?}, after {duration_after:?})"
    );

    // (b) Captured work survived. This is the defect that used to destroy it:
    // shots are derived, takes hang beneath them, and deletion recurses.
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
        "a screenplay edit destroyed {} captured take(s): {deleted_takes:?}",
        deleted_takes.len()
    );

    let g = film.project.engine.read();
    for take in &takes {
        assert!(g.contains(*take), "take {take} was lost to a script edit");
        assert!(
            g.targets(*take, EdgeKind::Binds).contains(&media),
            "take {take} lost its link to the media it was shot on"
        );
    }
    assert!(g.contains(media), "the media itself must survive");
    assert!(g.validate().is_empty());
}

/// Everything above must survive being written to disk and read back, or the
/// graph is a session artefact rather than a project.
#[test]
fn the_whole_chain_survives_save_and_reopen() {
    let media_path = need!(real_media(), "real media");
    let film = build_film();
    let scene = film.scenes[0];
    let shot = first_shot(&film, scene).expect("a shot");

    let mut media = EntityId::nil();
    let mut takes = Vec::new();
    film.project
        .engine
        .transact("shoot", |tx| {
            media = media_ucg::ingest(tx, film.project.root, &media_path)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            for n in 1..=2u32 {
                let take = write_take_metadata(tx, shot, n, &Default::default(), &[])
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
                media_ucg::bind_to_media(tx, take, media)
                    .map_err(|e| UcgError::Aborted(e.to_string()))?;
                takes.push(take);
            }
            Ok(())
        })
        .expect("shoot");

    let (revision, hash) = {
        let g = film.project.engine.read();
        let info = media_ucg::read_media(&g, media).expect("media info");
        (film.project.engine.revision(), info.hash.hex().to_string())
    };
    film.project.save().expect("save");

    // Close the project but keep the directory: dropping the whole fixture
    // would delete the files we are about to reopen.
    let Film {
        project,
        document,
        dir,
        ..
    } = film;
    let path = dir.path().to_path_buf();
    drop(project);

    // Reopen from disk, as `cine start` would.
    let reopened = OmniProject::open(&path).expect("reopen");
    let g = reopened.engine.read();

    assert_eq!(
        reopened.engine.revision(),
        revision,
        "the revision must be restored"
    );
    assert!(g.contains(document), "the screenplay survived");
    assert!(g.contains(scene), "the scene survived with its identity");
    assert!(g.contains(shot), "the shot survived with its identity");
    for take in &takes {
        assert!(g.contains(*take), "take {take} did not survive a reopen");
        assert!(
            g.targets(*take, EdgeKind::Binds).contains(&media),
            "the take-to-media link did not survive a reopen"
        );
    }

    // Media identity is content-derived, so it must be byte-for-byte the same
    // after a round trip through disk — that is what makes relinking possible.
    let info = media_ucg::read_media(&g, media).expect("media info survived");
    assert_eq!(
        info.hash.hex(),
        hash,
        "the content hash changed across a reopen"
    );
    assert_eq!(media_ucg::find_by_hash(&g, &hash), Some(media));
    assert!(g.validate().is_empty(), "the reopened graph is valid");
}
