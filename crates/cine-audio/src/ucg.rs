//! Bindings between the DAW session model and the Unified Context Graph.
//!
//! Two responsibilities:
//!
//! 1. **Script-driven placeholders.** Every script scene gets a dialogue track
//!    (`audio.track`, derived from the scene) holding one silent placeholder
//!    clip per dialogue line (`audio.clip`, derived from the line, `SyncedTo`
//!    it). Durations come from the line's word count, so the DAW shows the
//!    editorial shape of a scene before a single take is recorded. The
//!    pipeline reactor calls [`sync_dialogue_placeholders`] after every script
//!    edit; it is idempotent.
//! 2. **Session persistence.** [`store_session`] / [`load_session`] write a
//!    [`Session`] (tracks, buses, clips, automation) into the graph as nodes so
//!    the mix is versioned and undoable like everything else.

use crate::error::{Error, Result};
use crate::session::{Bus, Clip, Session, Stem, Track};
use serde::{Deserialize, Serialize};
use ucg_core::{EdgeKind, EntityId, Graph, RationalTime, TimeRange, Tx, Value};
use ucg_schema::{attrs, kinds};

/// Attribute holding a serialized [`Track`] (clips stripped) on a track node.
pub const TRACK_ATTR: &str = "track";
/// Attribute holding a serialized [`Bus`] on a bus node.
pub const BUS_ATTR: &str = "bus";
/// Attribute holding a serialized [`Clip`] on a clip node.
pub const CLIP_ATTR: &str = "clip";
/// Attribute holding the serialized master [`Bus`] on the session node.
pub const MASTER_ATTR: &str = "master";
/// Attribute naming the stem of a track (`dialogue`, `music`, …).
pub const STEM_ATTR: &str = "stem";
/// Session sample rate attribute.
pub const SAMPLE_RATE_ATTR: &str = "sample_rate";

/// Words per second assumed for dialogue placeholders.
pub const WORDS_PER_SECOND: f64 = 2.6;
/// Pause added after every line, in seconds.
pub const LINE_PAUSE_S: f64 = 0.4;

/// What [`sync_dialogue_placeholders`] changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DialogueSyncReport {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub total_duration_s: f64,
}

/// Id of the session derived from a project (or timeline).
pub fn session_id(owner: EntityId) -> EntityId {
    EntityId::derived(owner, kinds::AUDIO_SESSION)
}

/// Id of the dialogue track derived from a script scene.
pub fn dialogue_track_id(script_scene: EntityId) -> EntityId {
    EntityId::derived(script_scene, kinds::AUDIO_TRACK)
}

/// Id of the placeholder clip derived from a dialogue line.
pub fn placeholder_clip_id(dialogue_line: EntityId) -> EntityId {
    EntityId::derived(dialogue_line, kinds::AUDIO_CLIP)
}

fn require_kind(graph: &Graph, id: EntityId, kind: &'static str) -> Result<()> {
    match graph.node(id) {
        Some(n) if n.kind.as_str() == kind => Ok(()),
        Some(_) => Err(Error::WrongKind(id, kind)),
        None => Err(Error::Ucg(ucg_core::UcgError::NotFound(id))),
    }
}

/// Ensure the project (or timeline) has an audio session with the default
/// DME stems and a master bus. Idempotent.
pub fn ensure_session(
    tx: &mut Tx,
    project: EntityId,
    timeline: Option<EntityId>,
) -> Result<EntityId> {
    let owner = timeline.unwrap_or(project);
    let session = session_id(owner);
    tx.ensure(session, kinds::AUDIO_SESSION)?;
    tx.set(session, attrs::NAME, "Audio session")?;
    if tx.graph().contains(project) {
        tx.link(project, session, EdgeKind::Contains)?;
    }
    if let Some(t) = timeline {
        tx.link(session, t, EdgeKind::SyncedTo)?;
    }
    let rate = tx
        .graph()
        .attr(project, attrs::RATE)
        .and_then(Value::as_float)
        .unwrap_or(24.0);
    tx.set(session, attrs::RATE, rate)?;
    tx.set(session, SAMPLE_RATE_ATTR, 48_000i64)?;
    for (stem, abbrev) in [
        (Stem::Dialogue, "DX"),
        (Stem::Music, "MX"),
        (Stem::Effects, "FX"),
    ] {
        let id = EntityId::derived(session, &format!("stem:{abbrev}"));
        tx.ensure(id, kinds::AUDIO_STEM)?;
        tx.set(id, attrs::NAME, stem.label())?;
        tx.set(id, STEM_ATTR, stem.key())?;
        tx.link(session, id, EdgeKind::Contains)?;
    }
    let master = EntityId::derived(session, "bus:master");
    tx.ensure(master, kinds::AUDIO_BUS)?;
    tx.set(master, attrs::NAME, "Master")?;
    tx.link(session, master, EdgeKind::Contains)?;
    Ok(session)
}

/// Ensure a dialogue placeholder track for a script scene. Idempotent.
pub fn ensure_dialogue_track_for_scene(
    tx: &mut Tx,
    session: EntityId,
    script_scene: EntityId,
) -> Result<EntityId> {
    require_kind(tx.graph(), session, kinds::AUDIO_SESSION)?;
    require_kind(tx.graph(), script_scene, kinds::SCRIPT_SCENE)?;
    let track = dialogue_track_id(script_scene);
    let heading = tx
        .graph()
        .attr(script_scene, attrs::HEADING)
        .and_then(Value::as_str)
        .map(|h| format!("DX {h}"))
        .unwrap_or_else(|| "DX scene".to_string());
    tx.ensure(track, kinds::AUDIO_TRACK)?;
    tx.set(track, attrs::NAME, heading)?;
    tx.set(track, STEM_ATTR, Stem::Dialogue.key())?;
    tx.link(session, track, EdgeKind::Contains)?;
    tx.link(track, script_scene, EdgeKind::SyncedTo)?;
    Ok(track)
}

/// Estimated spoken duration of a line of dialogue.
pub fn estimate_line_duration_s(text: &str) -> f64 {
    let words = text.split_whitespace().count() as f64;
    words / WORDS_PER_SECOND + LINE_PAUSE_S
}

fn clip_label(character: &str, text: &str) -> String {
    let mut words: Vec<&str> = text.split_whitespace().take(5).collect();
    let truncated = text.split_whitespace().count() > 5;
    if truncated {
        words.push("…");
    }
    format!("{}: {}", character, words.join(" "))
}

/// Lay out one placeholder clip per dialogue line of `script_scene` on
/// `track`, sequentially from 0. Idempotent; removes clips whose line vanished.
pub fn sync_dialogue_placeholders(
    tx: &mut Tx,
    track: EntityId,
    script_scene: EntityId,
) -> Result<DialogueSyncReport> {
    require_kind(tx.graph(), track, kinds::AUDIO_TRACK)?;
    require_kind(tx.graph(), script_scene, kinds::SCRIPT_SCENE)?;
    let rate = tx
        .graph()
        .parent(track)
        .and_then(|s| tx.graph().attr(s, attrs::RATE))
        .and_then(Value::as_float)
        .unwrap_or(24.0);

    // Dialogue lines in order, with their text and character.
    let lines: Vec<(EntityId, String, String)> = {
        let g = tx.graph();
        g.children(script_scene)
            .into_iter()
            .filter_map(|id| {
                let n = g.node(id)?;
                if n.kind.as_str() != kinds::SCRIPT_DIALOGUE {
                    return None;
                }
                Some((
                    id,
                    n.get_str(attrs::CHARACTER).unwrap_or("").trim().to_string(),
                    n.get_str(attrs::TEXT).unwrap_or("").to_string(),
                ))
            })
            .collect()
    };

    let mut report = DialogueSyncReport::default();
    let mut cursor = 0.0;
    let mut wanted = Vec::with_capacity(lines.len());
    for (order, (line, character, text)) in lines.iter().enumerate() {
        let dur = estimate_line_duration_s(text);
        let clip = placeholder_clip_id(*line);
        wanted.push(clip);
        let range = TimeRange::new(
            RationalTime::from_seconds(cursor, rate),
            RationalTime::from_seconds(dur, rate),
        );
        let created = tx.ensure(clip, kinds::AUDIO_CLIP)?;
        let before = tx.ops().len();
        tx.set(clip, attrs::NAME, clip_label(character, text))?;
        tx.set(clip, attrs::RANGE, range)?;
        tx.set(clip, attrs::ORDER, order as i64)?;
        tx.link(track, clip, EdgeKind::Contains)?;
        tx.link(clip, *line, EdgeKind::SyncedTo)?;
        if created {
            report.added += 1;
        } else if tx.ops().len() > before {
            report.updated += 1;
        }
        cursor += dur;
    }
    // Remove placeholders whose line is gone: clips that were never stored as
    // authored clips, or that are still `SyncedTo` some dialogue line.
    let stale: Vec<EntityId> = {
        let g = tx.graph();
        g.children(track)
            .into_iter()
            .filter(|c| !wanted.contains(c))
            .filter(|c| {
                let Some(n) = g.node(*c) else { return false };
                if n.kind.as_str() != kinds::AUDIO_CLIP {
                    return false;
                }
                n.get(CLIP_ATTR).is_none()
                    || g.targets(*c, EdgeKind::SyncedTo).iter().any(|t| {
                        g.node(*t)
                            .map(|tn| tn.kind.as_str() == kinds::SCRIPT_DIALOGUE)
                            .unwrap_or(false)
                    })
            })
            .collect()
    };
    for c in stale {
        tx.delete(c)?;
        report.removed += 1;
    }
    report.total_duration_s = cursor;
    Ok(report)
}

/// Persist a [`Session`] under `session`. Tracks, buses and clips become
/// nodes keyed by their session ids; vanished ones are removed. Idempotent.
pub fn store_session(tx: &mut Tx, session: EntityId, s: &Session) -> Result<()> {
    require_kind(tx.graph(), session, kinds::AUDIO_SESSION)?;
    tx.set(session, SAMPLE_RATE_ATTR, s.sample_rate as i64)?;
    tx.set(session, attrs::RATE, s.rate)?;
    tx.set(session, "stems", Value::from_serde(&s.stems)?)?;
    tx.set(session, MASTER_ATTR, Value::from_serde(&s.master)?)?;

    let mut keep = Vec::new();
    for bus in &s.buses {
        tx.ensure(bus.id, kinds::AUDIO_BUS)?;
        tx.set(bus.id, attrs::NAME, bus.name.as_str())?;
        tx.set(bus.id, BUS_ATTR, Value::from_serde(bus)?)?;
        tx.link(session, bus.id, EdgeKind::Contains)?;
        keep.push(bus.id);
    }
    for (order, track) in s.tracks.iter().enumerate() {
        tx.ensure(track.id, kinds::AUDIO_TRACK)?;
        tx.set(track.id, attrs::NAME, track.name.as_str())?;
        tx.set(track.id, attrs::ORDER, order as i64)?;
        if let Some(stem) = &track.stem {
            tx.set(track.id, STEM_ATTR, stem.key())?;
        }
        let mut stripped = track.clone();
        stripped.clips.clear();
        tx.set(track.id, TRACK_ATTR, Value::from_serde(&stripped)?)?;
        tx.link(session, track.id, EdgeKind::Contains)?;
        keep.push(track.id);
        let mut keep_clips = Vec::new();
        for (corder, clip) in track.clips.iter().enumerate() {
            tx.ensure(clip.id, kinds::AUDIO_CLIP)?;
            tx.set(clip.id, attrs::NAME, clip.name.as_str())?;
            tx.set(clip.id, attrs::ORDER, corder as i64)?;
            tx.set(clip.id, attrs::RANGE, clip.range)?;
            tx.set(clip.id, CLIP_ATTR, Value::from_serde(clip)?)?;
            tx.link(track.id, clip.id, EdgeKind::Contains)?;
            keep_clips.push(clip.id);
        }
        let stale: Vec<EntityId> = tx
            .graph()
            .children(track.id)
            .into_iter()
            .filter(|c| !keep_clips.contains(c))
            .filter(|c| {
                tx.graph()
                    .node(*c)
                    .map(|n| n.get(CLIP_ATTR).is_some())
                    .unwrap_or(false)
            })
            .collect();
        for c in stale {
            tx.delete(c)?;
        }
    }
    // Remove tracks/buses that were stored before and are gone now.
    let stale: Vec<EntityId> = tx
        .graph()
        .children(session)
        .into_iter()
        .filter(|c| !keep.contains(c))
        .filter(|c| {
            tx.graph()
                .node(*c)
                .map(|n| n.get(TRACK_ATTR).is_some() || n.get(BUS_ATTR).is_some())
                .unwrap_or(false)
        })
        .collect();
    for c in stale {
        tx.delete(c)?;
    }
    Ok(())
}

/// Rebuild a [`Session`] from the graph. Script-derived dialogue tracks (which
/// carry no serialized `Track`) are included as placeholder tracks so the mix
/// view shows the screenplay's dialogue shape.
pub fn load_session(graph: &Graph, session: EntityId) -> Result<Session> {
    require_kind(graph, session, kinds::AUDIO_SESSION)?;
    let node = graph.require(session)?;
    let sample_rate = node.get_int(SAMPLE_RATE_ATTR).unwrap_or(48_000) as u32;
    let rate = node.get_float(attrs::RATE).unwrap_or(24.0);
    let master: Bus = node.get_as(MASTER_ATTR).unwrap_or_else(Bus::master);
    let stems: Vec<Stem> = node.get_as("stems").unwrap_or_default();
    let mut s = Session {
        sample_rate,
        rate,
        tracks: Vec::new(),
        buses: Vec::new(),
        stems,
        master,
        media: Default::default(),
    };
    for child in graph.children(session) {
        let Some(n) = graph.node(child) else { continue };
        match n.kind.as_str() {
            kinds::AUDIO_BUS => {
                if let Some(bus) = n.get_as::<Bus>(BUS_ATTR) {
                    s.buses.push(bus);
                }
            }
            kinds::AUDIO_TRACK => {
                let mut track: Track = match n.get_as::<Track>(TRACK_ATTR) {
                    Some(t) => t,
                    None => {
                        let mut t = Track::new(n.name(), s.master.id);
                        t.id = child;
                        t.stem = n.get_str(STEM_ATTR).map(Stem::parse);
                        t
                    }
                };
                track.clips.clear();
                for c in graph.children(child) {
                    let Some(cn) = graph.node(c) else { continue };
                    if cn.kind.as_str() != kinds::AUDIO_CLIP {
                        continue;
                    }
                    let clip = match cn.get_as::<Clip>(CLIP_ATTR) {
                        Some(clip) => clip,
                        None => {
                            let range = cn
                                .get(attrs::RANGE)
                                .and_then(Value::as_range)
                                .unwrap_or_default();
                            let mut clip = Clip::placeholder(
                                cn.name(),
                                range.start_time.to_seconds(),
                                range.duration.to_seconds(),
                                rate,
                            );
                            clip.id = c;
                            clip
                        }
                    };
                    track.clips.push(clip);
                }
                s.tracks.push(track);
            }
            _ => {}
        }
    }
    Ok(s)
}

/// Audio clips with a `SyncedTo` edge to `entity` (a dialogue line, a scene…).
pub fn clips_synced_to(graph: &Graph, entity: EntityId) -> Vec<EntityId> {
    graph
        .sources(entity, EdgeKind::SyncedTo)
        .into_iter()
        .filter(|id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kinds::AUDIO_CLIP)
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_estimate_and_label() {
        assert!(
            (estimate_line_duration_s("one two three") - (3.0 / WORDS_PER_SECOND + LINE_PAUSE_S))
                .abs()
                < 1e-12
        );
        assert_eq!(
            clip_label("MARA", "We ride at dawn."),
            "MARA: We ride at dawn."
        );
        assert_eq!(clip_label("MARA", "a b c d e f g"), "MARA: a b c d e …");
    }
}
