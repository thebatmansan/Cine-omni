//! Editing operations across the tracks of a stack, honouring links.
//!
//! [`crate::edit`] edits one track. A cutting room edits *material*: picture and
//! the sound shot with it are linked, and trimming one without the other puts
//! the take out of sync by exactly the trim. This module applies an edit to a
//! primary track and carries it to every clip linked to the one being edited,
//! all or nothing.
//!
//! ## What "carried" means, per operation
//!
//! | operation | carried to linked material as |
//! |---|---|
//! | ripple trim, slip, slide | the same edge / slip / slide on each linked clip |
//! | roll | a roll of the cut after each linked clip |
//! | split | a split at the same time, where a linked clip spans it |
//! | lift, extract | the same range, on every track holding linked material in that range |
//! | insert | a gap of the inserted duration at the same time, on every track holding material linked to anything the insert pushes later — the "sync lock" of an NLE |
//! | overwrite, replace, join | nothing: none of them moves material in time |
//!
//! ## One delta for the whole group
//!
//! Each clip clamps against its own media, so the same requested trim can
//! produce different applied trims on picture and sound. Applying each one's
//! own clamp would desynchronise them silently. Instead the edit is solved
//! for the whole group: every member is tried, the smallest applied magnitude
//! wins, and the edit is re-solved at that delta until every member agrees.
//! Deltas only shrink towards zero, so this terminates.
//!
//! A locked track holding linked material refuses the whole edit. Skipping it
//! would be the other way to lose sync silently.

use crate::edit::{self, Edge};
use crate::error::{Error, Result};
use crate::otio::{Item, Track};
use crate::session::link_of;
use std::collections::BTreeSet;
use ucg_core::{RationalTime, TimeRange};

/// An edit on one track, with every argument already resolved to the model.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackEdit {
    RippleTrim {
        index: usize,
        edge: Edge,
        delta: RationalTime,
    },
    Roll {
        cut: usize,
        delta: RationalTime,
    },
    Slip {
        index: usize,
        delta: RationalTime,
    },
    Slide {
        index: usize,
        delta: RationalTime,
    },
    SplitAt {
        time: RationalTime,
    },
    Extract {
        range: TimeRange,
    },
    Lift {
        range: TimeRange,
    },
    Insert {
        time: RationalTime,
        item: Item,
    },
    Overwrite {
        time: RationalTime,
        item: Item,
    },
    Replace {
        index: usize,
        item: Item,
    },
    Join {
        index: usize,
    },
}

impl TrackEdit {
    /// Human label for history and impact, e.g. `ripple trim`.
    pub fn label(&self) -> &'static str {
        match self {
            TrackEdit::RippleTrim { .. } => "ripple trim",
            TrackEdit::Roll { .. } => "roll",
            TrackEdit::Slip { .. } => "slip",
            TrackEdit::Slide { .. } => "slide",
            TrackEdit::SplitAt { .. } => "split",
            TrackEdit::Extract { .. } => "extract",
            TrackEdit::Lift { .. } => "lift",
            TrackEdit::Insert { .. } => "insert",
            TrackEdit::Overwrite { .. } => "overwrite",
            TrackEdit::Replace { .. } => "replace",
            TrackEdit::Join { .. } => "join",
        }
    }

    fn requested_delta(&self) -> Option<RationalTime> {
        match self {
            TrackEdit::RippleTrim { delta, .. }
            | TrackEdit::Roll { delta, .. }
            | TrackEdit::Slip { delta, .. }
            | TrackEdit::Slide { delta, .. } => Some(*delta),
            _ => None,
        }
    }

    fn with_delta(&self, d: RationalTime) -> TrackEdit {
        let mut e = self.clone();
        match &mut e {
            TrackEdit::RippleTrim { delta, .. }
            | TrackEdit::Roll { delta, .. }
            | TrackEdit::Slip { delta, .. }
            | TrackEdit::Slide { delta, .. } => *delta = d,
            _ => {}
        }
        e
    }

    fn with_index(&self, i: usize) -> TrackEdit {
        let mut e = self.clone();
        match &mut e {
            TrackEdit::RippleTrim { index, .. }
            | TrackEdit::Slip { index, .. }
            | TrackEdit::Slide { index, .. } => *index = i,
            TrackEdit::Roll { cut, .. } => *cut = i,
            _ => {}
        }
        e
    }
}

/// What an edit did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditOutcome {
    /// The delta asked for, for operations that take one.
    pub requested: Option<RationalTime>,
    /// The delta actually applied to every member of the link group.
    pub applied: Option<RationalTime>,
    /// The index of the inserted / split-off / replaced item on the primary track.
    pub index: Option<usize>,
    /// Indices (into the stack's tracks) of every track that changed.
    pub changed_tracks: Vec<usize>,
    /// How many linked clips on other tracks were edited with it.
    pub linked_clips: usize,
}

/// Apply `op` to one track. Returns `(applied delta, resulting index)`.
pub fn apply_one(
    track: &mut Track,
    op: &TrackEdit,
) -> Result<(Option<RationalTime>, Option<usize>)> {
    Ok(match op.clone() {
        TrackEdit::RippleTrim { index, edge, delta } => (
            Some(edit::ripple_trim(track, index, edge, delta)?),
            Some(index),
        ),
        TrackEdit::Roll { cut, delta } => (Some(edit::roll(track, cut, delta)?), Some(cut)),
        TrackEdit::Slip { index, delta } => (Some(edit::slip(track, index, delta)?), Some(index)),
        TrackEdit::Slide { index, delta } => (Some(edit::slide(track, index, delta)?), Some(index)),
        TrackEdit::SplitAt { time } => (None, Some(edit::split_at(track, time)?)),
        TrackEdit::Extract { range } => {
            edit::extract(track, range)?;
            (None, None)
        }
        TrackEdit::Lift { range } => {
            edit::lift(track, range)?;
            (None, None)
        }
        TrackEdit::Insert { time, item } => (None, Some(edit::insert(track, time, item)?)),
        TrackEdit::Overwrite { time, item } => (None, Some(edit::overwrite(track, time, item)?)),
        TrackEdit::Replace { index, item } => {
            edit::replace(track, index, item)?;
            (None, Some(index))
        }
        TrackEdit::Join { index } => {
            edit::join(track, index)?;
            (None, Some(index))
        }
    })
}

fn link_at(track: &Track, index: usize) -> Option<String> {
    track
        .children
        .get(index)
        .and_then(Item::as_clip)
        .and_then(link_of)
}

/// Every `(track, item)` on a track other than `primary` whose clip is in `group`.
fn linked_elsewhere(tracks: &[Track], primary: usize, group: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (ti, t) in tracks.iter().enumerate() {
        if ti == primary {
            continue;
        }
        for ii in 0..t.children.len() {
            if link_at(t, ii).as_deref() == Some(group) {
                out.push((ti, ii));
            }
        }
    }
    out
}

/// Link groups of clips on `track` that overlap `range` (or, with
/// `from_time`, that start at or after it or span it).
fn groups_in(
    track: &Track,
    range: Option<TimeRange>,
    from_time: Option<RationalTime>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for i in 0..track.children.len() {
        let Some(group) = link_at(track, i) else {
            continue;
        };
        let Some(r) = track.range_of_child(i) else {
            continue;
        };
        let hit = match (range, from_time) {
            (Some(q), _) => r.overlaps(&q),
            (None, Some(t)) => r.end_time_exclusive() > t,
            _ => false,
        };
        if hit {
            out.insert(group);
        }
    }
    out
}

fn tracks_holding(tracks: &[Track], primary: usize, groups: &BTreeSet<String>) -> Vec<usize> {
    (0..tracks.len())
        .filter(|ti| *ti != primary)
        .filter(|ti| {
            (0..tracks[*ti].children.len())
                .any(|ii| link_at(&tracks[*ti], ii).is_some_and(|g| groups.contains(&g)))
        })
        .collect()
}

fn magnitude(d: RationalTime) -> RationalTime {
    if d < RationalTime::zero(d.rate_exact()) {
        d.negated()
    } else {
        d
    }
}

/// Apply `op` to `tracks[primary]` and, when `follow_links`, to the linked
/// material on the other tracks. On any failure no track is changed.
pub fn apply_linked(
    tracks: &mut [Track],
    primary: usize,
    op: TrackEdit,
    follow_links: bool,
) -> Result<EditOutcome> {
    if primary >= tracks.len() {
        return Err(Error::IndexOutOfRange {
            index: primary,
            len: tracks.len(),
        });
    }
    let mut outcome = EditOutcome {
        requested: op.requested_delta(),
        ..Default::default()
    };

    // Which other edits the primary edit implies.
    let mut members: Vec<(usize, TrackEdit)> = vec![(primary, op.clone())];
    if follow_links {
        let p = &tracks[primary];
        match &op {
            TrackEdit::RippleTrim { index, .. }
            | TrackEdit::Slip { index, .. }
            | TrackEdit::Slide { index, .. } => {
                if let Some(g) = link_at(p, *index) {
                    for (ti, ii) in linked_elsewhere(tracks, primary, &g) {
                        members.push((ti, op.with_index(ii)));
                    }
                }
            }
            TrackEdit::Roll { cut, .. } => {
                if let Some(g) = link_at(p, *cut) {
                    for (ti, ii) in linked_elsewhere(tracks, primary, &g) {
                        members.push((ti, op.with_index(ii)));
                    }
                }
            }
            TrackEdit::SplitAt { time } => {
                let spanning = (0..p.children.len()).find(|i| {
                    p.range_of_child(*i)
                        .is_some_and(|r| r.contains(*time) && r.start_time < *time)
                });
                if let Some(g) = spanning.and_then(|i| link_at(p, i)) {
                    for (ti, ii) in linked_elsewhere(tracks, primary, &g) {
                        let spans = tracks[ti]
                            .range_of_child(ii)
                            .is_some_and(|r| r.contains(*time) && r.start_time < *time);
                        if spans {
                            members.push((ti, op.clone()));
                        }
                    }
                }
            }
            TrackEdit::Extract { range } | TrackEdit::Lift { range } => {
                let groups = groups_in(p, Some(*range), None);
                for ti in tracks_holding(tracks, primary, &groups) {
                    members.push((ti, op.clone()));
                }
            }
            TrackEdit::Insert { time, item } => {
                let groups = groups_in(p, None, Some(*time));
                for ti in tracks_holding(tracks, primary, &groups) {
                    let gap = Item::Gap(crate::otio::Gap::new(item.duration()));
                    members.push((
                        ti,
                        TrackEdit::Insert {
                            time: *time,
                            item: gap,
                        },
                    ));
                }
            }
            TrackEdit::Overwrite { .. } | TrackEdit::Replace { .. } | TrackEdit::Join { .. } => {}
        }
    }

    // Solve for one delta the whole group can take.
    let mut delta = op.requested_delta();
    let mut attempt = 0;
    let (working, applied, index) = loop {
        attempt += 1;
        let mut working: Vec<(usize, Track)> = Vec::with_capacity(members.len());
        let mut applied: Vec<RationalTime> = Vec::new();
        let mut primary_index = None;
        for (ti, e) in &members {
            let e = match delta {
                Some(d) => e.with_delta(d),
                None => e.clone(),
            };
            let mut t = match working.iter().position(|(w, _)| w == ti) {
                Some(pos) => working.remove(pos).1,
                None => tracks[*ti].clone(),
            };
            let (a, idx) = apply_one(&mut t, &e).map_err(|err| {
                if *ti == primary {
                    err
                } else {
                    Error::Edit(format!(
                        "linked material on track `{}` cannot take the {}: {err}",
                        tracks[*ti].name,
                        op.label()
                    ))
                }
            })?;
            if *ti == primary && primary_index.is_none() {
                primary_index = idx;
            }
            if let Some(a) = a {
                applied.push(a);
            }
            working.push((*ti, t));
        }
        let smallest = applied.iter().copied().min_by_key(|a| magnitude(*a));
        let agreed = match (smallest, delta) {
            (Some(s), Some(_)) => applied.iter().all(|a| *a == s),
            _ => true,
        };
        if agreed || attempt > members.len() + 1 {
            if !agreed {
                return Err(Error::Edit(format!(
                    "linked clips could not agree on a {} delta",
                    op.label()
                )));
            }
            break (working, smallest, primary_index);
        }
        delta = smallest;
    };

    for (ti, t) in working {
        if tracks[ti] != t {
            outcome.changed_tracks.push(ti);
        }
        tracks[ti] = t;
    }
    outcome.changed_tracks.sort_unstable();
    outcome.changed_tracks.dedup();
    outcome.linked_clips = members.len() - 1;
    outcome.applied = applied;
    outcome.index = index;
    Ok(outcome)
}
