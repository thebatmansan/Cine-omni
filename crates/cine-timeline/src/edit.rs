//! Non-linear editing operations on a [`Track`].
//!
//! Every operation is pure with respect to the graph: it mutates a `Track`
//! in memory, clamps to media bounds where applicable, validates the track's
//! invariants ([`Track::validate`]) and only then commits the change — a
//! failed operation leaves the track untouched. Frame arithmetic is exact
//! because [`RationalTime`] stores integer frame counts as `f64`.
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

const EPS: f64 = 1e-9;

/// Run `f` on a working copy of the track; commit only if it succeeds and
/// the result satisfies the invariants.
fn commit<T>(track: &mut Track, f: impl FnOnce(&mut Track) -> Result<T>) -> Result<T> {
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
fn transition_floor(track: &Track, index: usize, rate: f64) -> f64 {
    let mut floor = 0.0f64;
    if index > 0 {
        if let Some(t) = track.children[index - 1].as_transition() {
            floor = floor.max(t.out_offset.rescaled_to(rate).value());
        }
    }
    if let Some(t) = track.children.get(index + 1).and_then(Item::as_transition) {
        floor = floor.max(t.in_offset.rescaled_to(rate).value());
    }
    floor
}

/// Allowed `[lo, hi]` for a change `d` applied to the tail of `index`
/// (`duration += d`), in frames at the item's rate.
fn tail_bounds(track: &Track, index: usize) -> Result<(f64, f64, TimeRange)> {
    let tr = materialized_range(track, index)?;
    let rate = tr.start_time.rate_hz();
    let lo = -(tr.duration.value() - transition_floor(track, index, rate));
    let hi = match available_range(&track.children[index]) {
        Some(ar) => {
            ar.end_time_exclusive().rescaled_to(rate).value() - tr.end_time_exclusive().value()
        }
        None => f64::INFINITY,
    };
    Ok((lo, hi, tr))
}

/// Allowed `[lo, hi]` for a change `d` applied to the head of `index`
/// (`start += d`, `duration -= d`), in frames at the item's rate.
fn head_bounds(track: &Track, index: usize) -> Result<(f64, f64, TimeRange)> {
    let tr = materialized_range(track, index)?;
    let rate = tr.start_time.rate_hz();
    let hi = tr.duration.value() - transition_floor(track, index, rate);
    let lo = match available_range(&track.children[index]) {
        Some(ar) => ar.start_time.rescaled_to(rate).value() - tr.start_time.value(),
        None => f64::NEG_INFINITY,
    };
    Ok((lo, hi, tr))
}

fn clamp(d: f64, lo: f64, hi: f64) -> f64 {
    if lo > hi {
        // Already outside the feasible region (e.g. a clip shorter than its
        // transition); do not make it worse.
        return 0.0;
    }
    d.clamp(lo, hi)
}

fn set_tail(track: &mut Track, index: usize, tr: TimeRange, d: f64) {
    let rate = tr.start_time.rate_hz();
    track.children[index].set_source_range(TimeRange::new(
        tr.start_time,
        RationalTime::new(tr.duration.value() + d, rate),
    ));
}

fn set_head(track: &mut Track, index: usize, tr: TimeRange, d: f64) {
    let rate = tr.start_time.rate_hz();
    track.children[index].set_source_range(TimeRange::new(
        RationalTime::new(tr.start_time.value() + d, rate),
        RationalTime::new(tr.duration.value() - d, rate),
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
        let rate = tr.start_time.rate_hz();
        let d = clamp(delta.rescaled_to(rate).value(), lo, hi);
        match edge {
            Edge::Head => set_head(t, index, tr, d),
            Edge::Tail => set_tail(t, index, tr, d),
        }
        Ok(RationalTime::new(d, rate))
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
        let rate = ltr.start_time.rate_hz();
        let rscale = rate / rtr.start_time.rate_hz();
        let lo = llo.max(rlo * rscale);
        let hi = lhi.min(rhi * rscale);
        let d = clamp(delta.rescaled_to(rate).value(), lo, hi);
        set_tail(t, cut, ltr, d);
        set_head(t, right, rtr, d / rscale);
        Ok(RationalTime::new(d, rate))
    })
}

/// Slip: shift the media played by the clip at `index` by `delta` without
/// moving it on the track or changing its duration. Clamped to the media's
/// available range; a gap slips by nothing. Returns the applied delta.
pub fn slip(track: &mut Track, index: usize, delta: RationalTime) -> Result<RationalTime> {
    commit(track, |t| {
        let tr = materialized_range(t, index)?;
        let rate = tr.start_time.rate_hz();
        if t.children[index].is_gap() {
            return Ok(RationalTime::new(0.0, rate));
        }
        let (lo, hi) = match available_range(&t.children[index]) {
            Some(ar) => (
                ar.start_time.rescaled_to(rate).value() - tr.start_time.value(),
                ar.end_time_exclusive().rescaled_to(rate).value() - tr.end_time_exclusive().value(),
            ),
            None => (f64::NEG_INFINITY, f64::INFINITY),
        };
        let d = clamp(delta.rescaled_to(rate).value(), lo, hi);
        t.children[index].set_source_range(TimeRange::new(
            RationalTime::new(tr.start_time.value() + d, rate),
            tr.duration,
        ));
        Ok(RationalTime::new(d, rate))
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
        let rate = tr.start_time.rate_hz();
        let mut lo = f64::NEG_INFINITY;
        let mut hi = f64::INFINITY;

        let prev = if index == 0 { None } else { Some(index - 1) };
        if let Some(p) = prev {
            if t.children[p].is_transition() {
                return Err(Error::NotAnItem(p));
            }
            let (plo, phi, ptr) = tail_bounds(t, p)?;
            let s = rate / ptr.start_time.rate_hz();
            lo = lo.max(plo * s);
            hi = hi.min(phi * s);
        } else {
            lo = lo.max(0.0);
        }

        let next = index + 1;
        let has_next = next < t.children.len();
        if has_next {
            if t.children[next].is_transition() {
                return Err(Error::NotAnItem(next));
            }
            let (nlo, nhi, ntr) = head_bounds(t, next)?;
            let s = rate / ntr.start_time.rate_hz();
            lo = lo.max(nlo * s);
            hi = hi.min(nhi * s);
        }

        let d = clamp(delta.rescaled_to(rate).value(), lo, hi);
        if has_next {
            let (_, _, ntr) = head_bounds(t, next)?;
            set_head(t, next, ntr, d * ntr.start_time.rate_hz() / rate);
        }
        match prev {
            Some(p) => {
                let (_, _, ptr) = tail_bounds(t, p)?;
                set_tail(t, p, ptr, d * ptr.start_time.rate_hz() / rate);
            }
            None if d > 0.0 => {
                t.children
                    .insert(0, Item::Gap(Gap::new(RationalTime::new(d, rate))));
            }
            None => {}
        }
        Ok(RationalTime::new(d, rate))
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
    let rate = t.rate();
    let time_v = time.rescaled_to(rate).value();
    let total = t.duration().value();
    if time_v < -EPS || time_v > total + EPS {
        return Err(Error::TimeOutOfRange(time));
    }
    let mut start = 0.0;
    for i in 0..t.children.len() {
        if t.children[i].is_transition() {
            continue;
        }
        if (time_v - start).abs() <= EPS {
            if i > 0 && t.children[i - 1].is_transition() {
                return Err(Error::TransitionAtCut(time));
            }
            return Ok(i);
        }
        let dur = t.children[i].duration().rescaled_to(rate).value();
        if time_v > start && time_v < start + dur - EPS {
            let offset = time_v - start;
            let second = split_item(&mut t.children[i], RationalTime::new(offset, rate));
            t.children.insert(i + 1, second);
            return Ok(i + 1);
        }
        start += dur;
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
    let rate = tr.start_time.rate_hz();
    let off = offset.rescaled_to(rate).value();
    let first = TimeRange::new(tr.start_time, RationalTime::new(off, rate));
    let second = TimeRange::new(
        RationalTime::new(tr.start_time.value() + off, rate),
        RationalTime::new(tr.duration.value() - off, rate),
    );
    let mut other = item.clone();
    item.set_source_range(first);
    other.set_source_range(second);
    other.metadata_mut().shift_remove(UCG_ID_KEY);
    if let (Item::Clip(a), Item::Clip(b)) = (&mut *item, &mut other) {
        let cut = second.start_time.to_seconds();
        a.markers
            .retain(|m| m.marked_range.start_time.to_seconds() < cut);
        b.markers
            .retain(|m| m.marked_range.start_time.to_seconds() >= cut);
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
                    && ra.end_time_exclusive().almost_equal(&rb.start_time, EPS)
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
    let rate = t.rate();
    let (s, e) = (
        start.rescaled_to(rate).value(),
        end.rescaled_to(rate).value(),
    );
    let mut cut = 0.0;
    let mut i = 0;
    while i < t.children.len() {
        if t.children[i].is_transition() {
            if cut >= s - EPS && cut <= e + EPS {
                t.children.remove(i);
                continue;
            }
        } else {
            cut += t.children[i].duration().rescaled_to(rate).value();
        }
        i += 1;
    }
}

/// Pad the track with a gap so that it is at least `time` long; returns the
/// track rate.
fn pad_to(t: &mut Track, time: RationalTime) -> f64 {
    let rate = t.rate();
    let total = t.duration().value();
    let tv = time.rescaled_to(rate).value();
    if tv > total + EPS {
        t.children
            .push(Item::Gap(Gap::new(RationalTime::new(tv - total, rate))));
    }
    rate
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
        let rate = pad_to(t, at_time);
        let end = at_time.rescaled_to(rate) + item.duration();
        let total = t.duration();
        drop_transitions_on_cuts(t, at_time, end);
        let i = split_in_place(t, at_time)?;
        let j = if end.value() >= total.value() - EPS {
            t.children.len()
        } else {
            split_in_place(t, end)?
        };
        t.children.drain(i..j);
        t.children.insert(i, item);
        Ok(i)
    })
}

/// Remove the items in `range` and leave a gap of the same length so
/// nothing else moves. Returns the removed items (partially covered clips
/// are returned as their covered portion).
pub fn lift(track: &mut Track, range: TimeRange) -> Result<Vec<Item>> {
    commit(track, |t| {
        let rate = t.rate();
        let total = t.duration();
        let end = range.end_time_exclusive().rescaled_to(rate);
        let end = if end.value() > total.value() {
            total
        } else {
            end
        };
        drop_transitions_on_cuts(t, range.start_time, end);
        let i = split_in_place(t, range.start_time)?;
        let j = split_in_place(t, end)?;
        let removed: Vec<Item> = t.children.drain(i..j).collect();
        let dur = end - range.start_time.rescaled_to(rate);
        if dur.value() > EPS {
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
        let rate = t.rate();
        let total = t.duration();
        let end = range.end_time_exclusive().rescaled_to(rate);
        let end = if end.value() > total.value() {
            total
        } else {
            end
        };
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
