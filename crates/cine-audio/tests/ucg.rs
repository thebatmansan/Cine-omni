//! Graph bindings: session/track creation, placeholder sync, persistence.
//!
//! Every `engine.read()` guard is scoped in its own block so it is never held
//! across `engine.transact(...)` (the engine's RwLock would deadlock).

use cine_audio::prelude::*;
use cine_audio::ucg;
use std::sync::Arc;
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds};

fn aborted(e: impl std::fmt::Display) -> UcgError {
    UcgError::Aborted(e.to_string())
}

/// A project with one scene of `lines` dialogue lines. Returns `(project, scene, lines)`.
fn build_scene(engine: &Engine, lines: &[(&str, &str)]) -> (EntityId, EntityId, Vec<EntityId>) {
    let mut project = EntityId::nil();
    let mut scene = EntityId::nil();
    let mut ids = Vec::new();
    engine
        .transact("scene", |tx| {
            project = tx.create(kinds::PROJECT)?;
            tx.set(project, attrs::NAME, "Test")?;
            scene = tx.create_child(project, kinds::SCRIPT_SCENE)?;
            tx.set(scene, attrs::HEADING, "INT. KITCHEN - NIGHT")?;
            for (character, text) in lines {
                let line = tx.create_child(scene, kinds::SCRIPT_DIALOGUE)?;
                tx.set(line, attrs::CHARACTER, *character)?;
                tx.set(line, attrs::TEXT, *text)?;
                ids.push(line);
            }
            Ok(())
        })
        .unwrap();
    (project, scene, ids)
}

const LINES: [(&str, &str); 3] = [
    ("ANNA", "Where were you last night?"),
    (
        "BEN",
        "Out. Does it matter where I go when you never ask before?",
    ),
    ("ANNA", "It matters."),
];

#[test]
fn ensure_session_is_idempotent_and_has_default_buses() {
    let engine = Engine::new();
    let (project, _scene, _) = build_scene(&engine, &LINES);
    let mut session = EntityId::nil();
    let first = engine
        .transact("session", |tx| {
            session = ucg::ensure_session(tx, project, None).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert!(!first.deltas.is_empty());
    assert_eq!(session, EntityId::derived(project, kinds::AUDIO_SESSION));
    {
        let g = engine.read();
        let node = g.require(session).unwrap();
        assert_eq!(node.kind.as_str(), kinds::AUDIO_SESSION);
        assert!(node.get_str(attrs::NAME).is_some());
        assert_eq!(g.parent(session), Some(project));
        let kids = g.children(session);
        let stems: Vec<EntityId> = kids
            .iter()
            .copied()
            .filter(|k| g.node(*k).unwrap().kind.as_str() == kinds::AUDIO_STEM)
            .collect();
        assert_eq!(stems.len(), 3);
        assert!(stems.contains(&EntityId::derived(session, "stem:DX")));
        assert!(stems.contains(&EntityId::derived(session, "stem:MX")));
        assert!(stems.contains(&EntityId::derived(session, "stem:FX")));
        let master = EntityId::derived(session, "bus:master");
        assert!(kids.contains(&master));
        assert_eq!(g.node(master).unwrap().kind.as_str(), kinds::AUDIO_BUS);
        assert_eq!(g.node(master).unwrap().get_str(attrs::NAME), Some("Master"));
    }
    let second = engine
        .transact("session again", |tx| {
            let again = ucg::ensure_session(tx, project, None).map_err(aborted)?;
            assert_eq!(again, session);
            Ok(())
        })
        .unwrap();
    assert!(
        second.deltas.is_empty(),
        "second ensure_session produced ops: {:?}",
        second.deltas
    );

    // With a timeline the session derives from it and is SyncedTo it.
    let mut timeline = EntityId::nil();
    engine
        .transact("timeline", |tx| {
            timeline = tx.create_child(project, kinds::NLE_TIMELINE)?;
            Ok(())
        })
        .unwrap();
    let mut tl_session = EntityId::nil();
    engine
        .transact("session for timeline", |tx| {
            tl_session = ucg::ensure_session(tx, project, Some(timeline)).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        tl_session,
        EntityId::derived(timeline, kinds::AUDIO_SESSION)
    );
    {
        let g = engine.read();
        assert!(g.has_edge(tl_session, timeline, EdgeKind::SyncedTo));
        assert_eq!(g.parent(tl_session), Some(project));
    }
}

#[test]
fn dialogue_track_is_named_after_the_scene_and_idempotent() {
    let engine = Engine::new();
    let (project, scene, _) = build_scene(&engine, &LINES);
    let mut track = EntityId::nil();
    engine
        .transact("track", |tx| {
            let session = ucg::ensure_session(tx, project, None).map_err(aborted)?;
            track = ucg::ensure_dialogue_track_for_scene(tx, session, scene).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(track, EntityId::derived(scene, kinds::AUDIO_TRACK));
    {
        let g = engine.read();
        let n = g.require(track).unwrap();
        assert_eq!(n.kind.as_str(), kinds::AUDIO_TRACK);
        assert_eq!(n.get_str(attrs::NAME), Some("DX INT. KITCHEN - NIGHT"));
        assert_eq!(n.get_str("stem"), Some("dialogue"));
        assert!(g.has_edge(track, scene, EdgeKind::SyncedTo));
        assert_eq!(
            g.parent(track),
            Some(EntityId::derived(project, kinds::AUDIO_SESSION))
        );
    }
    let second = engine
        .transact("track again", |tx| {
            let session = ucg::ensure_session(tx, project, None).map_err(aborted)?;
            let again =
                ucg::ensure_dialogue_track_for_scene(tx, session, scene).map_err(aborted)?;
            assert_eq!(again, track);
            Ok(())
        })
        .unwrap();
    assert!(second.deltas.is_empty());
    // Wrong kinds are rejected.
    let err = engine.transact("bad", |tx| {
        ucg::ensure_dialogue_track_for_scene(tx, scene, project).map_err(aborted)?;
        Ok(())
    });
    assert!(err.is_err());
}

#[test]
fn placeholders_follow_the_script() {
    let engine = Engine::new();
    let (project, scene, lines) = build_scene(&engine, &LINES);
    let mut track = EntityId::nil();
    let mut report = ucg::DialogueSyncReport::default();
    engine
        .transact("sync", |tx| {
            let session = ucg::ensure_session(tx, project, None).map_err(aborted)?;
            track = ucg::ensure_dialogue_track_for_scene(tx, session, scene).map_err(aborted)?;
            report = ucg::sync_dialogue_placeholders(tx, track, scene).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert_eq!((report.added, report.updated, report.removed), (3, 0, 0));
    let expected_total: f64 = LINES
        .iter()
        .map(|(_, t)| t.split_whitespace().count() as f64 / 2.6 + 0.4)
        .sum();
    assert!((report.total_duration_s - expected_total).abs() < 1e-9);
    {
        let g = engine.read();
        let clips = g.children(track);
        assert_eq!(clips.len(), 3);
        let mut cursor = 0.0;
        for (i, (clip, line)) in clips.iter().zip(&lines).enumerate() {
            assert_eq!(*clip, EntityId::derived(*line, kinds::AUDIO_CLIP));
            let n = g.require(*clip).unwrap();
            assert_eq!(n.kind.as_str(), kinds::AUDIO_CLIP);
            assert_eq!(n.get_int(attrs::ORDER), Some(i as i64));
            assert!(g.has_edge(*clip, *line, EdgeKind::SyncedTo));
            assert_eq!(ucg::clips_synced_to(&g, *line), vec![*clip]);
            let range = n.get(attrs::RANGE).and_then(Value::as_range).unwrap();
            assert_eq!(range.start_time.rate, 24.0);
            let (start, dur) = range.to_seconds();
            assert!(
                (start - cursor).abs() < 1e-9,
                "clip {i} starts at {start}, expected {cursor}"
            );
            let words = LINES[i].1.split_whitespace().count() as f64;
            assert!((dur - (words / 2.6 + 0.4)).abs() < 1e-9);
            cursor += dur;
        }
        let names: Vec<String> = clips.iter().map(|c| g.node(*c).unwrap().name()).collect();
        assert_eq!(names[0], "ANNA: Where were you last night?");
        assert!(
            names[1].starts_with("BEN: Out. Does it matter where"),
            "{}",
            names[1]
        );
        assert!(names[1].ends_with('…'), "{}", names[1]);
        assert_eq!(names[2], "ANNA: It matters.");
    }
    // Second sync: nothing to do.
    let second = engine
        .transact("resync", |tx| {
            let r = ucg::sync_dialogue_placeholders(tx, track, scene).map_err(aborted)?;
            assert_eq!(
                r,
                ucg::DialogueSyncReport {
                    total_duration_s: r.total_duration_s,
                    ..Default::default()
                }
            );
            Ok(())
        })
        .unwrap();
    assert!(second.deltas.is_empty(), "resync produced ops");

    // Add a line, delete a line, change a line, resync.
    let mut new_line = EntityId::nil();
    engine
        .transact("edit script", |tx| {
            new_line = tx.create_child(scene, kinds::SCRIPT_DIALOGUE)?;
            tx.set(new_line, attrs::CHARACTER, "BEN")?;
            tx.set(new_line, attrs::TEXT, "Then say so.")?;
            tx.delete(lines[1])?;
            tx.set(lines[2], attrs::TEXT, "It matters to me, Ben.")?;
            Ok(())
        })
        .unwrap();
    engine
        .transact("resync after edit", |tx| {
            report = ucg::sync_dialogue_placeholders(tx, track, scene).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert_eq!((report.added, report.removed), (1, 1));
    assert!(
        report.updated >= 1,
        "the edited/shifted lines count as updated: {report:?}"
    );
    {
        let g = engine.read();
        let clips = g.children(track);
        assert_eq!(clips.len(), 3);
        assert!(!g.contains(EntityId::derived(lines[1], kinds::AUDIO_CLIP)));
        assert_eq!(clips[2], EntityId::derived(new_line, kinds::AUDIO_CLIP));
        // Still laid out back to back.
        let mut end = 0.0;
        for c in &clips {
            let (s, d) = g
                .attr(*c, attrs::RANGE)
                .and_then(Value::as_range)
                .unwrap()
                .to_seconds();
            assert!((s - end).abs() < 1e-9);
            end = s + d;
        }
        assert!((end - report.total_duration_s).abs() < 1e-9);
    }
    // Undo of the whole edit+resync group restores the previous state.
    engine.undo().unwrap();
    engine.undo().unwrap();
    {
        let g = engine.read();
        assert_eq!(g.children(track).len(), 3);
        assert!(g.contains(EntityId::derived(lines[1], kinds::AUDIO_CLIP)));
    }
}

#[test]
fn session_store_load_round_trip() {
    let engine = Engine::new();
    let (project, scene, _) = build_scene(&engine, &LINES[..1]);
    let mut session_id = EntityId::nil();
    engine
        .transact("session", |tx| {
            session_id = ucg::ensure_session(tx, project, None).map_err(aborted)?;
            let track =
                ucg::ensure_dialogue_track_for_scene(tx, session_id, scene).map_err(aborted)?;
            ucg::sync_dialogue_placeholders(tx, track, scene).map_err(aborted)?;
            Ok(())
        })
        .unwrap();

    // Build an authored session with buses, sends, inserts and automation.
    let mut s = Session::new(48_000, 24.0);
    let mut verb = Bus::new("verb");
    verb.inserts
        .push(Insert::filter(BiquadKind::LowPass, 5000.0, 0.7, 0.0));
    let verb_id = verb.id;
    s.buses.push(verb);
    let media = EntityId::new();
    let mut clip = Clip::placeholder("take 3", 1.0, 2.5, 24.0);
    clip.source = ClipSource::Entity(media);
    clip.gain_db = -3.0;
    clip.fade_in = Fade::new(0.05, FadeCurve::EqualPower);
    let mut t = Track::new("DX Anna", s.master_id())
        .with_stem(Stem::Dialogue)
        .with_clip(clip);
    t.sends.push(Send {
        bus: verb_id,
        level_db: -12.0,
        pre_fader: false,
    });
    t.inserts
        .push(Insert::Compressor(CompressorParams::default()));
    t.pan = PanState::azimuth(15.0);
    t.automation.set_lane(
        Param::Volume,
        Lane::new(vec![(0.0, 0.0), (3.0, -6.0)], Interpolation::Linear),
    );
    s.add_track(t);
    let mut mx = Track::new("MX", s.master_id()).with_stem(Stem::Music);
    mx.clips.push(Clip::placeholder("cue 1", 0.0, 4.0, 24.0));
    mx.mute = true;
    s.add_track(mx);

    let first = engine
        .transact("store", |tx| {
            ucg::store_session(tx, session_id, &s).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert!(!first.deltas.is_empty());
    let again = engine
        .transact("store again", |tx| {
            ucg::store_session(tx, session_id, &s).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert!(
        again.deltas.is_empty(),
        "storing an unchanged session produced ops"
    );

    let loaded = {
        let g = engine.read();
        ucg::load_session(&g, session_id).unwrap()
    };
    assert_eq!(loaded.sample_rate, 48_000);
    assert_eq!(loaded.rate, 24.0);
    assert_eq!(loaded.stems, s.stems);
    assert_eq!(loaded.master, s.master);
    assert_eq!(loaded.buses, s.buses);
    // The authored tracks round-trip exactly; the script-derived DX track is
    // loaded too (as a placeholder track), so filter it out for equality.
    let authored: Vec<&Track> = loaded
        .tracks
        .iter()
        .filter(|t| s.track(t.id).is_some())
        .collect();
    assert_eq!(authored.len(), s.tracks.len());
    for t in authored {
        assert_eq!(t, s.track(t.id).unwrap());
    }
    let script_track = loaded
        .tracks
        .iter()
        .find(|t| s.track(t.id).is_none())
        .expect("script-derived track");
    assert_eq!(
        script_track.id,
        EntityId::derived(scene, kinds::AUDIO_TRACK)
    );
    assert_eq!(script_track.stem, Some(Stem::Dialogue));
    assert_eq!(script_track.clips.len(), 1);
    assert_eq!(script_track.clips[0].source, ClipSource::Silence);

    // Storing the loaded session back converges: the first store may attach
    // full clip/track records to the script-derived track, the second is a
    // no-op, and what loads afterwards equals what was stored.
    engine
        .transact("store loaded", |tx| {
            ucg::store_session(tx, session_id, &loaded).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    let noop = engine
        .transact("store loaded again", |tx| {
            ucg::store_session(tx, session_id, &loaded).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    assert!(
        noop.deltas.is_empty(),
        "store(load(x)) twice produced ops: {:?}",
        noop.deltas
    );
    {
        let g = engine.read();
        let reloaded = ucg::load_session(&g, session_id).unwrap();
        assert_eq!(reloaded.tracks, loaded.tracks);
        assert_eq!(reloaded.buses, loaded.buses);
    }
    // Removing a track and a clip deletes their nodes.
    let mut smaller = loaded.clone();
    let removed_track = smaller.tracks.remove(1).id;
    let removed_clip = smaller.tracks[0].clips.remove(0).id;
    engine
        .transact("shrink", |tx| {
            ucg::store_session(tx, session_id, &smaller).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    {
        let g = engine.read();
        assert!(!g.contains(removed_track));
        assert!(!g.contains(removed_clip));
        let back = ucg::load_session(&g, session_id).unwrap();
        assert_eq!(back.tracks.len(), smaller.tracks.len());
    }
    // A loaded session renders (media resolved through `Session::media`).
    // `smaller` dropped the only audible clip, so render the full `loaded` one.
    let mut playable = loaded.clone();
    playable.media.insert(
        media,
        Arc::new(AudioBuffer::sine(48_000, 1, 440.0, 0.25, 3.0)),
    );
    let out = render(&playable, (0.0, 4.0), ChannelLayout::Stereo).unwrap();
    assert!(out.rms() > 0.0, "the entity-backed clip should be audible");
    // With no media registered the same session renders silence, not an error.
    let silent = render(&loaded, (0.0, 4.0), ChannelLayout::Stereo).unwrap();
    assert_eq!(silent.rms(), 0.0);
}
