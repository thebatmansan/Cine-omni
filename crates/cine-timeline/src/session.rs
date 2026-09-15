//! Editorial session state: what the *editor* is doing, as distinct from what
//! the timeline *is*.
//!
//! Track locking, muting, soloing and targeting, linked audio and video, snap
//! targets and media handles. None of it changes what the cut is; all of it
//! changes what an editing operation should do next.
//!
//! ## Why this is not new OTIO schema
//!
//! OpenTimelineIO describes an edit so that another application can read it. It
//! has no schema for "this track is locked" or "these two clips move together",
//! because those are the state of *an editor*, not properties of the cut. The
//! conformant place for application state is `metadata`, which OTIO carries on
//! every object and which this crate round-trips verbatim.
//!
//! So state lives in `metadata["CINE-OMNI"]`, and a file written here stays
//! readable by any OTIO implementation, which simply ignores a namespace it
//! does not know. Inventing fields in the OTIO schema would have produced files
//! that claim to be OTIO and are not.
//!
//! ## Solo is not "mute everything else"
//!
//! Soloing one track and muting another are different statements, and an editor
//! that implements solo by muting loses the mute flags the moment solo is
//! cleared. Both are stored; [`is_audible`] resolves them, and the resolution
//! is the only place the rule lives.

use crate::error::{Error, Result};
use crate::otio::{Clip, Composable, Item, Timeline, Track, TrackKind};
use serde_json::{json, Value as J};
use ucg_core::RationalTime;

/// The metadata key everything here lives under.
pub const NAMESPACE: &str = "CINE-OMNI";

const LOCKED: &str = "locked";
const MUTED: &str = "muted";
const SOLO: &str = "solo";
const TARGETED: &str = "targeted";
const LINK: &str = "link";

fn flag(metadata: &crate::otio::Metadata, key: &str) -> bool {
    metadata
        .get(NAMESPACE)
        .and_then(|v| v.get(key))
        .and_then(J::as_bool)
        .unwrap_or(false)
}

fn set_field(metadata: &mut crate::otio::Metadata, key: &str, value: J) {
    let entry = metadata
        .entry(NAMESPACE.to_string())
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    if let Some(map) = entry.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

/// What an editor knows about a track that the cut itself does not say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackState {
    /// Refuses edits. Distinct from disabled: a locked track still plays.
    pub locked: bool,
    /// Silenced by choice. Distinct from soloing another track.
    pub muted: bool,
    pub solo: bool,
    /// Receives inserts and overwrites.
    pub targeted: bool,
}

impl TrackState {
    pub fn of(track: &Track) -> TrackState {
        TrackState {
            locked: flag(&track.metadata, LOCKED),
            muted: flag(&track.metadata, MUTED),
            solo: flag(&track.metadata, SOLO),
            targeted: flag(&track.metadata, TARGETED),
        }
    }

    /// Write this state onto a track, leaving every other metadata key alone.
    pub fn apply(&self, track: &mut Track) {
        set_field(&mut track.metadata, LOCKED, json!(self.locked));
        set_field(&mut track.metadata, MUTED, json!(self.muted));
        set_field(&mut track.metadata, SOLO, json!(self.solo));
        set_field(&mut track.metadata, TARGETED, json!(self.targeted));
    }
}

/// Refuse an edit to a locked track.
///
/// The point of a lock is that an operation which would have changed the track
/// does not change it *at all*, so this is checked before any work begins.
pub fn ensure_unlocked(track: &Track) -> Result<()> {
    if TrackState::of(track).locked {
        return Err(Error::TrackLocked(track.name.clone()));
    }
    Ok(())
}

/// Whether any track in the timeline is soloed.
pub fn any_solo(timeline: &Timeline) -> bool {
    tracks(timeline).any(|t| TrackState::of(t).solo)
}

/// Whether any track of this kind is soloed. Solo is per-kind: soloing a video
/// track should not silence the audio.
pub fn any_solo_of(timeline: &Timeline, kind: TrackKind) -> bool {
    tracks(timeline)
        .filter(|t| t.kind == kind)
        .any(|t| TrackState::of(t).solo)
}

/// Whether a track is heard, given whether anything of its kind is soloed.
///
/// Solo wins over mute, and a disabled track is never heard whatever else is
/// set: disabling is a statement about the cut, and it is stronger.
pub fn is_audible(track: &Track, solo_active: bool) -> bool {
    let state = TrackState::of(track);
    if !track.enabled {
        return false;
    }
    if solo_active {
        return state.solo;
    }
    !state.muted
}

/// The same rule for picture. Named separately because a viewer asking "should
/// I draw this" should not have to read a function called `is_audible`.
pub fn is_visible(track: &Track, solo_active: bool) -> bool {
    is_audible(track, solo_active)
}

/// Indices of the tracks an insert or overwrite should go to.
pub fn targeted_tracks(timeline: &Timeline) -> Vec<usize> {
    tracks(timeline)
        .enumerate()
        .filter(|(_, t)| TrackState::of(t).targeted)
        .map(|(i, _)| i)
        .collect()
}

fn tracks(timeline: &Timeline) -> impl Iterator<Item = &Track> {
    timeline
        .tracks
        .children
        .iter()
        .filter_map(Composable::as_track)
}

// ---------------------------------------------------------------------------
// Linked audio and video
// ---------------------------------------------------------------------------

/// The link group a clip belongs to, if any.
///
/// Clips in the same group were shot together — a camera's picture and its
/// sound, or a multi-camera angle and its audio — and an edit to one is meant
/// to apply to all of them. The identifier is opaque and is preserved through
/// OTIO metadata, so a round trip cannot quietly unlink a take.
pub fn link_of(clip: &Clip) -> Option<String> {
    clip.metadata
        .get(NAMESPACE)
        .and_then(|v| v.get(LINK))
        .and_then(J::as_str)
        .map(str::to_string)
}

pub fn set_link(clip: &mut Clip, group: &str) {
    set_field(&mut clip.metadata, LINK, json!(group));
}

pub fn clear_link(clip: &mut Clip) {
    if let Some(entry) = clip.metadata.get_mut(NAMESPACE) {
        if let Some(map) = entry.as_object_mut() {
            map.remove(LINK);
        }
    }
}

/// Every clip in the timeline sharing `group`, as `(track index, item index)`.
///
/// Returned as positions rather than references because the caller is about to
/// edit them, and holding a borrow of what you are going to mutate is how link
/// handling turns into a fight with the borrow checker.
pub fn linked_positions(timeline: &Timeline, group: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (ti, track) in tracks(timeline).enumerate() {
        for (ii, item) in track.children.iter().enumerate() {
            if let Some(clip) = item.as_clip() {
                if link_of(clip).as_deref() == Some(group) {
                    out.push((ti, ii));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// Media available beyond what a clip currently plays: `(head, tail)`.
///
/// This is what a trim has to spend. `None` when the clip has no declared
/// available range, which is not the same as zero handles — it means the
/// question cannot be answered, and a caller must not treat that as "no room".
pub fn handles(clip: &Clip) -> Option<(RationalTime, RationalTime)> {
    let available = clip.available_range()?;
    let source = clip.source_range?;
    let head = source.start_time.checked_sub(&available.start_time);
    let tail = available
        .end_time_exclusive()
        .checked_sub(&source.end_time_exclusive());
    Some((head, tail))
}

// ---------------------------------------------------------------------------
// Snapping
// ---------------------------------------------------------------------------

/// Times an edit should prefer to land on: every cut, every track start and
/// end, and every marker.
///
/// `skip_track` excludes the track being dragged, because an item does not snap
/// to where it already is.
pub fn snap_targets(timeline: &Timeline, skip_track: Option<usize>) -> Vec<RationalTime> {
    let mut out = Vec::new();
    for (ti, track) in tracks(timeline).enumerate() {
        if Some(ti) == skip_track {
            continue;
        }
        let mut at = track
            .children
            .first()
            .and_then(|_| track.range_of_child(0))
            .map(|r| r.start_time);
        if let Some(start) = at {
            out.push(start);
        }
        for index in 0..track.children.len() {
            if let Some(range) = track.range_of_child(index) {
                out.push(range.start_time);
                at = Some(range.end_time_exclusive());
                out.push(range.end_time_exclusive());
            }
        }
        let _ = at;
        for marker in &track.markers {
            out.push(marker.marked_range.start_time);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The nearest target within `tolerance`, or `None` to leave the time alone.
///
/// Ties go to the earlier target so that dragging left and dragging right onto
/// the same midpoint do not disagree. `tolerance` is a duration, never a pixel
/// count: the model does not know how far a pixel is, and a snap that depended
/// on zoom would not be reproducible.
pub fn snap(
    time: RationalTime,
    targets: &[RationalTime],
    tolerance: RationalTime,
) -> Option<RationalTime> {
    let mut best: Option<(RationalTime, RationalTime)> = None;
    for &target in targets {
        let delta = if target >= time {
            target.checked_sub(&time)
        } else {
            time.checked_sub(&target)
        };
        if delta > tolerance {
            continue;
        }
        match best {
            Some((best_delta, _)) if delta >= best_delta => {}
            _ => best = Some((delta, target)),
        }
    }
    best.map(|(_, target)| target)
}

/// Snap if anything is close enough, otherwise keep the original time.
pub fn snapped(
    time: RationalTime,
    targets: &[RationalTime],
    tolerance: RationalTime,
) -> RationalTime {
    snap(time, targets, tolerance).unwrap_or(time)
}

/// Items in a track, for callers that want to reason about a selection.
pub fn items_of(track: &Track) -> &[Item] {
    &track.children
}
