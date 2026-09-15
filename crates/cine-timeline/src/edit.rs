//! Non-linear editing operations on a [`Track`].
//!
//! Every operation is pure with respect to the graph: it mutates a `Track`
//! in memory, clamps to media bounds where applicable, validates the track's
//! invariants ([`Track::validate`]) and only then commits the change — a
//! failed operation leaves the track untouched.
//!
//! **All arithmetic here is exact.** [`RationalTime`] is an integer tick count
//! at an exact rational rate, and comparison and arithmetic cross-multiply in
//! `i128`, so two items at different rates can be combined without conversion
//! and without drift. This file previously computed in `f64` frames with an
//! `EPS` of 1e-9 and, in `roll` and `slide`, derived a scale factor by dividing
//! one rate by another — which is precisely how NTSC rates drift. Both are
//! gone: there is no epsilon and no rate ratio, because neither is needed once
//! the arithmetic is exact.
//!
//! Sign conventions (all deltas are in track time):
//!
//! * a **positive** delta moves an edge, a cut, or a clip **later** on the
//!   track;
//! * [`Edge::Head`] `+d` shortens the item from the front, [`Edge::Tail`]
//!   `+d` lengthens it at the back;
//! * [`slip`] `+d` plays media that starts `d` later in the source.
//!
//! Operations that trim return the delta they *actually* applied after
//! clamping, so callers can report "trimmed 12 of the 20 requested frames".

use crate::error::{Error, Result};
use crate::otio::*;
use ucg_core::{RationalTime, TimeRange};

/// Which end of an item a trim operates on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Head,
    Tail,
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

/// A trim bound. `None` means unbounded in that direction — a gap has no media
/// to run out of, so its head can move as far as the caller likes.
type Bound = Option<RationalTime>;

/// Run `f` on a working copy of the track; commit only if it succeeds and
/// the result satisfies the invariants.
///
/// Every editing operation in this module goes through here, which is why a
/// locked track is refused here: a lock that each operation had to remember to
/// honour would eventually be forgotten by one of them.
fn commit<T>(track: &mut Track, f: impl FnOnce(&mut Track) -> Result<T>) -> Result<T> {
    crate::session::ensure_unlocked(track)?;
    let mut work = track.clone();
    let out = f(&mut work)?;
    work.validate()?;
    *track = work;
    Ok(out)
}

fn timed_item(track: &Track, index: usize) -> Result<&Item> {
    let len = track.children.len();
    let item = track
        .children
        .get(index)
        .ok_or(Error::IndexOutOfRange { index, len })?;
    if item.is_transition() {
        return Err(Error::NotAnItem(index));
    }
    Ok(item)
}

/// The trimmed range of a clip or gap, materialized (a clip without a
/// `source_range` gets its available range).
fn materialized_range(track: &Track, index: usize) -> Result<TimeRange> {
    let item = timed_item(track, index)?;
    item.trimmed_range()
        .ok_or_else(|| Error::Invariant(format!("item {index} has no range")))
}

fn available_range(item: &Item) -> Option<TimeRange> {
    item.as_clip().and_then(Clip::available_range)
}

/// Largest transition offset that the item at `index` must keep covering:
/// the `out_offset` of a transition before it, the `in_offset` of one after.
fn transition_floor(track: &Track, index: usize, zero: RationalTime) -> RationalTime {
    let mut floor = zero;
    if index > 0 {
        if let Some(t) = track.children[index - 1].as_transition() {
            floor = floor.max(t.out_offset);
        }
    }
    if let Some(t) = track.children.get(index + 1).and_then(Item::as_transition) {
        floor = floor.max(t.in_offset);
    }
    floor
}

/// Allowed `[lo, hi]` for a change `d` applied to the tail of `index`
/// (`duration += d`).
fn tail_bounds(track: &Track, index: usize) -> Result<(Bound, Bound, TimeRange)> {
    let tr = materialized_range(track, index)?;
    let zero = RationalTime::zero(tr.duration.rate_exact());
    // Never shorter than nothing, and never shorter than the transition it has
    // to keep covering.
    let lo = zero.checked_sub(
        &tr.duration
            .checked_sub(&transition_floor(track, index, zero)),
    );
    let hi = available_range(&track.children[index]).map(|ar| {
        ar.end_time_exclusive()
            .checked_sub(&tr.end_time_exclusive())
    });
    Ok((Some(lo), hi, tr))
}

/// Allowed `[lo, hi]` for a change `d` applied to the head of `index`
/// (`start += d`, `duration -= d`).
fn head_bounds(track: &Track, index: usize) -> Result<(Bound, Bound, TimeRange)> {
    let tr = materialized_range(track, index)?;
    let zero = RationalTime::zero(tr.duration.rate_exact());
    let hi = tr
        .duration
        .checked_sub(&transition_floor(track, index, zero));
    let lo =
        available_range(&track.children[index]).map(|ar| ar.start_time.checked_sub(&tr.start_time));
    Ok((lo, Some(hi), tr))
}

/// Clamp `d` into `[lo, hi]`, where either end may be unbounded.
///
/// Bounds may be at different rates from `d` and from each other; comparison is
/// exact across rates, so nothing is converted.
fn clamp(d: RationalTime, lo: Bound, hi: Bound, zero: RationalTime) -> RationalTime {
    if let (Some(l), Some(h)) = (lo, hi) {
        if l > h {
            // Already outside the feasible region (e.g. a clip shorter than its
            // transition); do not make it worse.
            return zero;
        }
    }
    let mut d = d;
    if let Some(l) = lo {
        if d < l {
            d = l;
        }
    }
    if let Some(h) = hi {
        if d > h {
            d = h;
        }
    }
    d
}

/// Combine two bounds, keeping whichever is tighter. An absent bound imposes
/// no constraint, so the present one wins.
fn tighter(a: Bound, b: Bound, pick: fn(RationalTime, RationalTime) -> RationalTime) -> Bound {
    match (a, b) {
        (Some(x), Some(y)) => Some(pick(x, y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn set_tail(track: &mut Track, index: usize, tr: TimeRange, d: RationalTime) {
    track.children[index]
        .set_source_range(TimeRange::new(tr.start_time, tr.duration.checked_add(&d)));
}

fn set_head(track: &mut Track, index: usize, tr: TimeRange, d: RationalTime) {
    track.children[index].set_source_range(TimeRange::new(
        tr.start_time.checked_add(&d),
        tr.duration.checked_sub(&d),
    ));
}

// ---------------------------------------------------------------------------
// trims
// ---------------------------------------------------------------------------

/// Ripple trim: move one edge of the clip or gap at `index` by `delta`,
/// changing its duration; everything after it shifts accordingly (a track
/// is sequential, so this falls out of changing the duration).
///
/// The trim is clamped so the item never plays media outside its available
/// range, never goes negative, and keeps covering adjacent transitions.
/// Returns the delta that was applied.
pub fn ripple_trim(
    track: &mut Track,
    index: usize,
    edge: Edge,
    delta: RationalTime,
) -> Result<RationalTime> {
    commit(track, |t| {
        let (lo, hi, tr) = match edge {
            Edge::Head => head_bounds(t, index)?,
            Edge::Tail => tail_bounds(t, index)?,
        };
        let zero = RationalTime::zero(tr.duration.rate_exact());
        let d = clamp(delta, lo, hi, zero);
        match edge {
            Edge::Head => set_head(t, index, tr, d),
            Edge::Tail => set_tail(t, index, tr, d),
        }
        Ok(d)
    })
}

/// Index of the timed item on the right of the cut after `left` (skipping a
/// transition sitting on the cut).
fn right_of_cut(track: &Track, left: usize) -> Result<usize> {
    let len = track.children.len();
    let mut right = left + 1;
    if track
        .children
        .get(right)
        .map(Item::is_transition)
        .unwrap_or(false)
    {
        right += 1;
    }
    if right >= len {
        return Err(Error::IndexOutOfRange { index: right, len });
    }
    Ok(right)
}

/// Roll the cut after item `cut` by `delta`: the left item's tail and the
/// right item's head move together, so the track's total duration is
/// unchanged. A transition sitting on the cut moves with it. Clamped to
/// both items' media and durations. Returns the applied delta.
pub fn roll(track: &mut Track, cut: usize, delta: RationalTime) -> Result<RationalTime> {
    commit(track, |t| {
        let right = right_of_cut(t, cut)?;
        let (llo, lhi, ltr) = tail_bounds(t, cut)?;
        let (rlo, rhi, rtr) = head_bounds(t, right)?;
        // The two items may be at different rates. `d` is a duration, and
        // comparison and addition are exact across rates, so it applies to both
        // ends of the cut unconverted — no scale factor, and nothing to drift.
        let zero = RationalTime::zero(ltr.duration.rate_exact());
        let lo = tighter(llo, rlo, RationalTime::max);
        let hi = tighter(lhi, rhi, RationalTime::min);
        let d = clamp(delta, lo, hi, zero);
        set_tail(t, cut, ltr, d);
        set_head(t, right, rtr, d);
        Ok(d)
    })
}

/// Slip: shift the media played by the clip at `index` by `delta` without
/// moving it on the track or changing its duration. Clamped to the media's
/// available range; a gap slips by nothing. Returns the applied delta.
pub fn slip(track: &mut Track, index: usize, delta: RationalTime) -> Result<RationalTime> {
    commit(track, |t| {
        let tr = materialized_range(t, index)?;
        let zero = RationalTime::zero(tr.duration.rate_exact());
        if t.children[index].is_gap() {
            return Ok(zero);
        }
        let (lo, hi) = match available_range(&t.children[index]) {
            Some(ar) => (
                Some(ar.start_time.checked_sub(&tr.start_time)),
                Some(
                    ar.end_time_exclusive()
                        .checked_sub(&tr.end_time_exclusive()),
                ),
            ),
            None => (None, None),
        };
        let d = clamp(delta, lo, hi, zero);
        t.children[index]
            .set_source_range(TimeRange::new(tr.start_time.checked_add(&d), tr.duration));
        Ok(d)
    })
}

/// Slide: move the clip or gap at `index` along the track by `delta`,
/// keeping its content and duration; the previous neighbour's tail and the
/// next neighbour's head absorb the move. Sliding the first item later
/// opens a gap in front of it. Clamped to the neighbours' media and
/// durations. Returns the applied delta.
pub fn slide(track: &mut Track, index: usize, delta: RationalTime) -> Result<RationalTime> {
    commit(track, |t| {
        let tr = materialized_range(t, index)?;
        let zero = RationalTime::zero(tr.duration.rate_exact());
        // Neighbours may be at other rates; `d` is a duration and applies to
        // each of them unconverted, because the arithmetic is exact.
        let mut lo: Bound = None;
        let mut hi: Bound = None;

        let prev = if index == 0 { None } else { Some(index - 1) };
        if let Some(p) = prev {
            if t.children[p].is_transition() {
                return Err(Error::NotAnItem(p));
            }
            let (plo, phi, _) = tail_bounds(t, p)?;
            lo = tighter(lo, plo, RationalTime::max);
            hi = tighter(hi, phi, RationalTime::min);
        } else {
            // Nothing before it: sliding earlier than the head of the track
            // would place it at a negative time.
            lo = tighter(lo, Some(zero), RationalTime::max);
        }

        let next = index + 1;
        let has_next = next < t.children.len();
        if has_next {
            if t.children[next].is_transition() {
                return Err(Error::NotAnItem(next));
            }
            let (nlo, nhi, _) = head_bounds(t, next)?;
            lo = tighter(lo, nlo, RationalTime::max);
            hi = tighter(hi, nhi, RationalTime::min);
        }

        let d = clamp(delta, lo, hi, zero);
        if has_next {
            let (_, _, ntr) = head_bounds(t, next)?;
            set_head(t, next, ntr, d);
        }
        match prev {
            Some(p) => {
                let (_, _, ptr) = tail_bounds(t, p)?;
                set_tail(t, p, ptr, d);
            }
            None if d > zero => {
                t.children.insert(0, Item::Gap(Gap::new(d)));
            }
            None => {}
        }
        Ok(d)
    })
}

// ---------------------------------------------------------------------------
// splitting / joining
// ---------------------------------------------------------------------------

/// Split the clip or gap under `time` into two so that an item boundary
/// lies exactly at `time`. Returns the index of the item that *starts* at
/// `time` (`children.len()` when `time` is the end of the track). If a
/// boundary already exists nothing changes.
///
/// Errors if `time` is beyond the track or lies inside a transition.
pub fn split_at(track: &mut Track, time: RationalTime) -> Result<usize> {
    commit(track, |t| split_in_place(t, time))
}

fn split_in_place(t: &mut Track, time: RationalTime) -> Result<usize> {
    if t.transition_at_time(time).is_some() {
        return Err(Error::TransitionAtCut(time));
    }
    let total = t.duration();
    let mut start = RationalTime::zero(total.rate_exact());
    if time < start || time > total {
        return Err(Error::TimeOutOfRange(time));
    }
    for i in 0..t.children.len() {
        if t.children[i].is_transition() {
            continue;
        }
        // Exact equality, not a tolerance: a cut either falls on a boundary or
        // it does not, and an epsilon here decided that question by how the
        // arithmetic happened to round.
        if time == start {
            if i > 0 && t.children[i - 1].is_transition() {
                return Err(Error::TransitionAtCut(time));
            }
            return Ok(i);
        }
        let dur = t.children[i].duration();
        let end = start.checked_add(&dur);
        if time > start && time < end {
            let second = split_item(&mut t.children[i], time.checked_sub(&start));
            t.children.insert(i + 1, second);
            return Ok(i + 1);
        }
        start = end;
    }
    // time == total duration
    if let Some(last) = t.children.last() {
        if last.is_transition() {
            return Err(Error::TransitionAtCut(time));
        }
    }
    Ok(t.children.len())
}

/// Split `item` at `offset` from its head; returns the second half and
/// shrinks `item` to the first half. Markers stay with the half whose media
/// range contains them; the `ucg_id` stays with the first half only.
fn split_item(item: &mut Item, offset: RationalTime) -> Item {
    let tr = item.trimmed_range().unwrap_or_default();
    let first = TimeRange::new(tr.start_time, offset);
    let second = TimeRange::new(
        tr.start_time.checked_add(&offset),
        tr.duration.checked_sub(&offset),
    );
    let mut other = item.clone();
    item.set_source_range(first);
    other.set_source_range(second);
    other.metadata_mut().shift_remove(UCG_ID_KEY);
    if let (Item::Clip(a), Item::Clip(b)) = (&mut *item, &mut other) {
        let cut = second.start_time;
        a.markers.retain(|m| m.marked_range.start_time < cut);
        b.markers.retain(|m| m.marked_range.start_time >= cut);
    }
    other
}

/// Join the clips (or gaps) at `index` and `index + 1` when the second
/// continues the first in media (same reference, contiguous ranges).
pub fn join(track: &mut Track, index: usize) -> Result<()> {
    commit(track, |t| {
        let a = timed_item(t, index)?.clone();
        let b = timed_item(t, index + 1)?.clone();
        let (Some(ra), Some(rb)) = (a.trimmed_range(), b.trimmed_range()) else {
            return Err(Error::NotContinuous(index, index + 1));
        };
        let joinable = match (&a, &b) {
            (Item::Clip(ca), Item::Clip(cb)) => {
                ca.media_reference.same_media(&cb.media_reference)
                    // Contiguity is an exact question: the first ends where the
                    // second begins, or it does not.
                    && ra.end_time_exclusive() == rb.start_time
            }
            // Empty time has no media: two gaps always join.
            (Item::Gap(_), Item::Gap(_)) => true,
            _ => false,
        };
        if !joinable {
            return Err(Error::NotContinuous(index, index + 1));
        }
        let merged = TimeRange::new(ra.start_time, ra.duration + rb.duration);
        t.children[index].set_source_range(merged);
        if let (Item::Clip(ca), Item::Clip(cb)) = (&mut t.children[index], &b) {
            ca.markers.extend(cb.markers.iter().cloned());
        }
        t.children.remove(index + 1);
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// insert / overwrite / lift / extract
// ---------------------------------------------------------------------------

/// Remove every transition whose cut point lies in `[start, end]` (track
/// time): material removed or overwritten across a cut takes the transition
/// on that cut with it, as an NLE does.
fn drop_transitions_on_cuts(t: &mut Track, start: RationalTime, end: RationalTime) {
    let mut cut = RationalTime::zero(t.duration().rate_exact());
    let mut i = 0;
    while i < t.children.len() {
        if t.children[i].is_transition() {
            if cut >= start && cut <= end {
                t.children.remove(i);
                continue;
            }
        } else {
            cut = cut.checked_add(&t.children[i].duration());
        }
        i += 1;
    }
}

/// Pad the track with a gap so that it is at least `time` long; returns the
/// track rate.
fn pad_to(t: &mut Track, time: RationalTime) {
    let total = t.duration();
    if time > total {
        t.children
            .push(Item::Gap(Gap::new(time.checked_sub(&total))));
    }
}

/// Ripple insert `item` at `at_time`: the clip under that time is split and
/// everything after moves later by the item's duration. Inserting past the
/// end pads with a gap. Returns the index of the inserted item.
pub fn insert(track: &mut Track, at_time: RationalTime, item: Item) -> Result<usize> {
    commit(track, |t| {
        pad_to(t, at_time);
        let idx = split_in_place(t, at_time)?;
        t.children.insert(idx, item);
        Ok(idx)
    })
}

/// Overwrite: `item` replaces whatever lies in `[at_time, at_time + duration)`;
/// nothing else moves. Partially covered clips are trimmed, fully covered
/// ones removed. Returns the index of the inserted item.
pub fn overwrite(track: &mut Track, at_time: RationalTime, item: Item) -> Result<usize> {
    if item.is_transition() {
        return Err(Error::Invariant(
            "cannot overwrite with a transition".into(),
        ));
    }
    commit(track, |t| {
        pad_to(t, at_time);
        let end = at_time.checked_add(&item.duration());
        let total = t.duration();
        drop_transitions_on_cuts(t, at_time, end);
        let i = split_in_place(t, at_time)?;
        let j = if end >= total {
            t.children.len()
        } else {
            split_in_place(t, end)?
        };
        t.children.drain(i..j);
        t.children.insert(i, item);
        Ok(i)
    })
}

/// Replace the item at `index` with `item`, keeping the track's shape.
///
/// The replacement plays for exactly as long as what it replaced, so nothing
/// after it moves: this is the replace edit of a cutting room, not a delete
/// followed by an insert. It is refused if the replacement has less media than
/// the hole needs — silently playing something shorter and leaving a hole would
/// be worse than saying no.
pub fn replace(track: &mut Track, index: usize, item: Item) -> Result<()> {
    commit(track, |t| {
        let wanted = timed_item(t, index)?
            .source_range()
            .ok_or_else(|| Error::Edit("the item being replaced has no duration".into()))?
            .duration;
        let mut replacement = item;
        let source = replacement
            .source_range()
            .ok_or_else(|| Error::Edit("the replacement has no source range".into()))?;
        if let Some(available) = available_range(&replacement) {
            let room = available
                .end_time_exclusive()
                .checked_sub(&source.start_time);
            if room < wanted {
                return Err(Error::Edit(format!(
                    "the replacement has {room:?} of media from its start point, \
                     but {wanted:?} is needed to fill the hole"
                )));
            }
        }
        replacement.set_source_range(TimeRange::new(source.start_time, wanted));
        t.children[index] = replacement;
        Ok(())
    })
}

/// Remove the items in `range` and leave a gap of the same length so
/// nothing else moves. Returns the removed items (partially covered clips
/// are returned as their covered portion).
pub fn lift(track: &mut Track, range: TimeRange) -> Result<Vec<Item>> {
    commit(track, |t| {
        let total = t.duration();
        let end = range.end_time_exclusive().min(total);
        drop_transitions_on_cuts(t, range.start_time, end);
        let i = split_in_place(t, range.start_time)?;
        let j = split_in_place(t, end)?;
        let removed: Vec<Item> = t.children.drain(i..j).collect();
        let dur = end.checked_sub(&range.start_time);
        if dur > RationalTime::zero(dur.rate_exact()) {
            t.children.insert(i, Item::Gap(Gap::new(dur)));
        }
        Ok(removed)
    })
}

/// Ripple delete: remove the items in `range` and close the hole. A
/// transition left without a valid neighbour is removed too. Returns the
/// removed items.
pub fn extract(track: &mut Track, range: TimeRange) -> Result<Vec<Item>> {
    commit(track, |t| {
        let total = t.duration();
        let end = range.end_time_exclusive().min(total);
        drop_transitions_on_cuts(t, range.start_time, end);
        let i = split_in_place(t, range.start_time)?;
        let j = split_in_place(t, end)?;
        let removed: Vec<Item> = t.children.drain(i..j).collect();
        heal_transitions(t, i);
        Ok(removed)
    })
}

/// After removing items so that a seam sits at `at`, drop transitions that
/// no longer sit between two timed items.
fn heal_transitions(t: &mut Track, mut at: usize) {
    loop {
        let left_is_t = at > 0 && t.children[at - 1].is_transition();
        let right_is_t = t.children.get(at).map(Item::is_transition).unwrap_or(false);
        if left_is_t && (at == t.children.len() || right_is_t) {
            t.children.remove(at - 1);
            at -= 1;
        } else if right_is_t && at == 0 {
            t.children.remove(0);
        } else {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

/// What happened to an item between two versions of a timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Moved,
    Trimmed,
}

/// One difference reported by [`diff`]. `index` refers to the new timeline
/// for `Added`/`Moved`/`Trimmed` and to the old one for `Removed`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Change {
    pub track: usize,
    pub index: usize,
    pub kind: ChangeKind,
}

/// Compare two timelines track by track. Items are matched by their
/// `ucg_id` metadata when present, otherwise by kind, name and media.
pub fn diff(old: &Timeline, new: &Timeline) -> Vec<Change> {
    let old_tracks: Vec<&Track> = old.tracks().collect();
    let new_tracks: Vec<&Track> = new.tracks().collect();
    let mut out = Vec::new();
    let n = old_tracks.len().max(new_tracks.len());
    for ti in 0..n {
        match (old_tracks.get(ti), new_tracks.get(ti)) {
            (Some(o), Some(nw)) => out.extend(diff_tracks(o, nw, ti)),
            (Some(o), None) => out.extend((0..o.children.len()).map(|i| Change {
                track: ti,
                index: i,
                kind: ChangeKind::Removed,
            })),
            (None, Some(nw)) => out.extend((0..nw.children.len()).map(|i| Change {
                track: ti,
                index: i,
                kind: ChangeKind::Added,
            })),
            (None, None) => {}
        }
    }
    out
}

fn identity(item: &Item) -> String {
    if let Some(id) = item.ucg_id() {
        return format!("id:{id}");
    }
    match item {
        Item::Clip(c) => format!(
            "clip:{}:{}",
            c.name,
            c.media_reference.target_url().unwrap_or("")
        ),
        Item::Gap(g) => format!("gap:{}", g.name),
        Item::Transition(t) => format!("transition:{}", t.name),
    }
}

/// Compare two versions of one track (reported under track index `track`).
pub fn diff_tracks(old: &Track, new: &Track, track: usize) -> Vec<Change> {
    let old_ranges = old.child_ranges();
    let new_ranges = new.child_ranges();
    let mut used = vec![false; new.children.len()];
    let mut out = Vec::new();
    for (oi, o) in old.children.iter().enumerate() {
        let key = identity(o);
        let found = new
            .children
            .iter()
            .enumerate()
            .find(|(ni, n)| !used[*ni] && identity(n) == key);
        match found {
            None => out.push(Change {
                track,
                index: oi,
                kind: ChangeKind::Removed,
            }),
            Some((ni, n)) => {
                used[ni] = true;
                let trimmed = o.source_range() != n.source_range()
                    || match (o, n) {
                        (Item::Transition(a), Item::Transition(b)) => {
                            a.in_offset != b.in_offset || a.out_offset != b.out_offset
                        }
                        _ => false,
                    };
                if trimmed {
                    out.push(Change {
                        track,
                        index: ni,
                        kind: ChangeKind::Trimmed,
                    });
                } else if old_ranges[oi] != new_ranges[ni] {
                    out.push(Change {
                        track,
                        index: ni,
                        kind: ChangeKind::Moved,
                    });
                }
            }
        }
    }
    for (ni, u) in used.iter().enumerate() {
        if !u {
            out.push(Change {
                track,
                index: ni,
                kind: ChangeKind::Added,
            });
        }
    }
    out.sort_by_key(|c| (c.index, c.kind as u8));
    out
}
