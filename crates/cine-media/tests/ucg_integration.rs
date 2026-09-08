//! Media as a first-class citizen of the Unified Context Graph.
//!
//! The point of putting media in the graph is that "what does replacing this
//! affect?" becomes a traversal rather than a manual audit. These tests pin
//! that behaviour, including the distinction that matters most: a proxy goes
//! **stale** and must be regenerated, whereas a clip needs **relinking** and
//! its edit decisions survive untouched.
//!
//! Read guards are scoped so none is ever held across a `transact`.

use cine_media::identity::{Derivative, DerivativeKind};
use cine_media::ucg;
use std::path::PathBuf;
use ucg_core::prelude::*;
use ucg_schema::kinds;

fn real_mp4() -> Option<PathBuf> {
    let p = PathBuf::from("/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4");
    p.exists().then_some(p)
}

macro_rules! need {
    ($opt:expr, $what:expr) => {
        match $opt {
            Some(p) => p,
            None => {
                eprintln!("SKIP: no {} available", $what);
                return;
            }
        }
    };
}

#[test]
fn ingesting_real_media_is_idempotent_and_content_addressed() {
    let path = need!(real_mp4(), "real mp4");
    let engine = Engine::new();
    let project = EntityId::from_key("project:root");
    let mut media = EntityId::nil();

    engine
        .transact("ingest", |tx| {
            tx.ensure(project, kinds::PROJECT)?;
            media =
                ucg::ingest(tx, project, &path).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();

    {
        let g = engine.read();
        let node = g.require(media).unwrap();
        assert_eq!(node.kind.as_str(), kinds::ASSET_MEDIA);
        assert_eq!(node.get_bool(ucg::ONLINE_ATTR), Some(true));
        assert!(node.get_str(ucg::HASH_ATTR).is_some());

        // One stream node per elementary stream, video and audio.
        let streams = g.children(media);
        assert_eq!(streams.len(), 2, "the mp4 has a video and an audio track");

        // Capability is recorded truthfully on the graph, not guessed later.
        let codecs: Vec<(String, bool)> = streams
            .iter()
            .map(|s| {
                let n = g.node(*s).unwrap();
                (
                    n.get_str(ucg::CODEC_ATTR).unwrap_or("").to_string(),
                    n.get_bool(ucg::DECODABLE_ATTR).unwrap_or(false),
                )
            })
            .collect();
        assert!(
            codecs.contains(&("h264".to_string(), true)),
            "h264 recorded as decodable"
        );
        assert!(
            codecs.contains(&("aac".to_string(), true)),
            "aac recorded as decodable"
        );
        assert_eq!(ucg::decodable_streams(&g, media).len(), 2);

        // The probe survives without re-reading the file.
        let info = ucg::read_media(&g, media).expect("stored media info");
        assert_eq!(info.streams.len(), 2);
    }

    // Re-ingesting the same file is a no-op: content addressing means one entity.
    let again = engine
        .transact("ingest again", |tx| {
            let id =
                ucg::ingest(tx, project, &path).map_err(|e| UcgError::Aborted(e.to_string()))?;
            assert_eq!(id, media, "same bytes must resolve to the same entity");
            Ok(())
        })
        .unwrap();
    assert!(
        again.deltas.is_empty(),
        "re-ingest produced ops: not idempotent"
    );
}

#[test]
fn media_replacement_separates_relink_from_regenerate() {
    let path = need!(real_mp4(), "real mp4");
    let engine = Engine::new();
    let project = EntityId::from_key("project:root");
    let (mut media, mut clip, mut take, mut proxy, mut thumb) = (
        EntityId::nil(),
        EntityId::nil(),
        EntityId::nil(),
        EntityId::nil(),
        EntityId::nil(),
    );

    engine
        .transact("build", |tx| {
            tx.ensure(project, kinds::PROJECT)?;
            media =
                ucg::ingest(tx, project, &path).map_err(|e| UcgError::Aborted(e.to_string()))?;

            // Consumers that BIND to the media: they relink, they do not regenerate.
            clip = tx.create(kinds::NLE_CLIP)?;
            take = tx.create(kinds::LOGISTICS_TAKE)?;
            ucg::bind_to_media(tx, clip, media).map_err(|e| UcgError::Aborted(e.to_string()))?;
            ucg::bind_to_media(tx, take, media).map_err(|e| UcgError::Aborted(e.to_string()))?;

            // Derivatives: these genuinely go stale.
            let hash = ucg::read_media(tx.graph(), media)
                .unwrap()
                .hash
                .hex()
                .to_string();
            proxy = ucg::register_derivative(
                tx,
                media,
                &Derivative {
                    kind: DerivativeKind::Proxy,
                    source_hash: hash.clone(),
                    path: PathBuf::from("/proxies/A001.mov"),
                    recipe: "h264 960x540".into(),
                },
            )
            .map_err(|e| UcgError::Aborted(e.to_string()))?;
            thumb = ucg::register_derivative(
                tx,
                media,
                &Derivative {
                    kind: DerivativeKind::Thumbnail,
                    source_hash: hash,
                    path: PathBuf::from("/thumbs/A001.png"),
                    recipe: "frame 0 256px".into(),
                },
            )
            .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();

    let g = engine.read();
    let impact = ucg::media_replacement_impact(&g, media);

    // Derived artefacts must be regenerated.
    assert!(
        impact.derived.contains(&proxy),
        "the proxy is stale and must regenerate"
    );
    assert!(impact.derived.contains(&thumb), "the thumbnail is stale");

    // Bound consumers must be relinked, NOT regenerated. Conflating these is
    // how an NLE loses edit decisions during a conform.
    assert!(impact.bound.contains(&clip), "the clip needs relinking");
    assert!(impact.bound.contains(&take), "the take needs relinking");
    assert!(
        !impact.derived.contains(&clip),
        "a clip must never be treated as derived"
    );
    assert!(!impact.derived.contains(&take));

    assert_eq!(impact.streams.len(), 2);
    assert_eq!(impact.total(), 6);
    assert!(!impact.is_empty());
    assert!(g.validate().is_empty());
}

#[test]
fn offline_media_is_a_state_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let src = need!(real_mp4(), "real mp4");
    let temp = dir.path().join("A001C001.mp4");
    std::fs::copy(&src, &temp).unwrap();

    let engine = Engine::new();
    let project = EntityId::from_key("project:root");
    let mut media = EntityId::nil();
    engine
        .transact("ingest", |tx| {
            tx.ensure(project, kinds::PROJECT)?;
            media =
                ucg::ingest(tx, project, &temp).map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert!(ucg::offline_media(&engine.read()).is_empty());

    // The card is unmounted. The project must remain valid and editable.
    std::fs::remove_file(&temp).unwrap();
    engine
        .transact("go offline", |tx| {
            let changed =
                ucg::set_online(tx, media, false).map_err(|e| UcgError::Aborted(e.to_string()))?;
            assert!(changed);
            Ok(())
        })
        .unwrap();

    {
        let g = engine.read();
        assert_eq!(ucg::offline_media(&g), vec![media]);
        assert!(
            g.validate().is_empty(),
            "offline media must not corrupt the graph"
        );
        // Identity and stream structure survive the media being absent.
        assert_eq!(g.children(media).len(), 2);
        assert!(ucg::read_media(&g, media).is_some());
    }

    // Relinking: the same bytes reappear elsewhere and are recognised by hash.
    let moved = dir.path().join("restored/A001C001.mp4");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::copy(&src, &moved).unwrap();
    let found = cine_media::probe(&moved).unwrap();
    let g = engine.read();
    assert_eq!(
        ucg::find_by_hash(&g, found.hash.hex()),
        Some(media),
        "a moved file must relink to the existing identity by content"
    );
}
