//! What the picture of a sequence is, over time: the edit list a player plays.
//!
//! A timeline is layered — picture tracks stack, the top visible one wins — and
//! a player needs the opposite: for each stretch of record time, one clip and
//! the media time it starts at. This is the single place that answer is
//! computed. The runtime projects it; the native player plays it; neither
//! re-derives it.
//!
//! ## What it does not do
//!
//! Transitions are **not rendered**. A dissolve plays as a cut at the edit
//! point, and [`PictureList::transitions`] counts how many were flattened that
//! way so a caller can say so. Rendering a dissolve needs two decodes and a mix
//! on the GPU, which is a separate capability, not something to fake here.

use crate::otio::{Clip, Item, Timeline, TrackKind};
use crate::session::{any_solo_of, is_audible, is_visible};
use ucg_core::{EntityId, RationalTime, TimeRange};

/// How a retimed clip's media runs against record time: [`ucg_core::Timing`],
/// the same type every player uses, so there is one answer to "which picture
/// is showing" — constant speed or ramp — rather than several that can drift.
pub use ucg_core::Timing;

/// A retimed clip's timing, anchored where it starts on the record and at the
/// media shown there.
fn timing_of(clip: &Clip, record: RationalTime, media: RationalTime) -> Option<Timing> {
    Some(Timing {
        map: clip.time_map()?,
        record,
        media,
    })
}

/// One stretch of record time showing one clip.
#[derive(Clone, Debug, PartialEq)]
pub struct PictureSegment {
    /// Where on the sequence this plays.
    pub record: TimeRange,
    /// Index of the track in the timeline's track order.
    pub track: usize,
    /// Index of the item on that track.
    pub item: usize,
    /// The clip's entity id, when it came from the graph.
    pub clip: Option<EntityId>,
    /// The media time shown at `record.start_time`, in the clip's source
    /// coordinates (the same coordinates as its `source_range`).
    pub source_start: RationalTime,
    /// How the media runs against record time, when the clip is retimed —
    /// anchored at the clip's own start, so a ramp is read from its beginning
    /// whichever part of the clip this segment shows. `None` is real time, the
    /// overwhelmingly common case, and a player that ignores it is then right.
    pub timing: Option<Timing>,
}

/// The flattened picture of a sequence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PictureList {
    pub segments: Vec<PictureSegment>,
    /// Transitions on visible picture tracks, played as cuts.
    pub transitions: usize,
    /// Where the longest picture track ends.
    pub end: Option<RationalTime>,
}

/// Flatten the visible picture tracks of `timeline`.
///
/// Stretches with no clip on any visible picture track are simply absent: a
/// player shows black there, and the list does not invent a segment for it.
pub fn picture_segments(timeline: &Timeline) -> PictureList {
    let solo = any_solo_of(timeline, TrackKind::Video);
    let mut transitions = 0usize;
    let mut end: Option<RationalTime> = None;
    // (track index, item index, span, the clip itself, its entity id)
    let mut clips: Vec<(usize, usize, TimeRange, &Clip, Option<EntityId>)> = Vec::new();
    for (ti, track) in timeline.tracks().enumerate() {
        if track.kind != TrackKind::Video || !track.enabled || !is_visible(track, solo) {
            continue;
        }
        let d = track.duration();
        end = Some(end.map_or(d, |e| e.max(d)));
        for (ii, item) in track.children.iter().enumerate() {
            match item {
                Item::Transition(_) => transitions += 1,
                Item::Clip(c) if c.enabled => {
                    let (Some(span), Some(_)) = (track.range_of_child(ii), c.trimmed_range())
                    else {
                        continue;
                    };
                    let zero = RationalTime::zero(span.duration.rate_exact());
                    if span.duration > zero {
                        clips.push((ti, ii, span, c, item.ucg_id()));
                    }
                }
                _ => {}
            }
        }
    }

    let mut bounds: Vec<RationalTime> = clips
        .iter()
        .flat_map(|(_, _, span, _, _)| [span.start_time, span.end_time_exclusive()])
        .collect();
    bounds.sort();
    bounds.dedup();

    let mut segments: Vec<PictureSegment> = Vec::new();
    for pair in bounds.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        // Top-most visible track wins: later tracks are on top.
        let winner = clips
            .iter()
            .filter(|(_, _, span, _, _)| {
                span.start_time <= from && from < span.end_time_exclusive()
            })
            .max_by_key(|(ti, _, _, _, _)| *ti);
        let Some((ti, ii, span, media, clip)) = winner else {
            continue;
        };
        // Where this instant lands in the clip's media — through the retime
        // when there is one, which is the clip's own arithmetic, not a copy.
        let source_start = media
            .media_time_at(span.start_time, from)
            .unwrap_or_else(|| {
                media
                    .trimmed_range()
                    .map(|r| r.start_time)
                    .unwrap_or_else(|| RationalTime::zero(from.rate_exact()))
            });
        let duration = to.checked_sub(&from);
        // Continue the previous segment when it is the same clip, contiguous.
        if let Some(last) = segments.last_mut() {
            if last.track == *ti && last.item == *ii && last.record.end_time_exclusive() == from {
                last.record = TimeRange::new(
                    last.record.start_time,
                    last.record.duration.checked_add(&duration),
                );
                continue;
            }
        }
        segments.push(PictureSegment {
            record: TimeRange::new(from, duration),
            track: *ti,
            item: *ii,
            clip: *clip,
            source_start,
            timing: media
                .media_time_at(span.start_time, span.start_time)
                .and_then(|origin| timing_of(media, span.start_time, origin)),
        });
    }
    PictureList {
        segments,
        transitions,
        end,
    }
}

/// One clip's sound on the sequence. Sound is not flattened: every audible clip
/// plays, and a player mixes them.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundSegment {
    pub record: TimeRange,
    pub track: usize,
    pub item: usize,
    pub clip: Option<EntityId>,
    /// Media time heard at `record.start_time`, in source coordinates. For a
    /// reversed clip this is the end of its range: it is read backwards.
    pub source_start: RationalTime,
    /// How the media runs against record time, for the mixer to read it at;
    /// anchored at `record.start_time` and `source_start`.
    pub timing: Option<Timing>,
    /// Keep pitch while the speed changes, rather than varispeed.
    pub keep_pitch: bool,
}

/// Every enabled clip on every audible sound track, in track then time order.
/// Muted tracks are absent; soloing any sound track leaves only soloed ones.
pub fn sound_segments(timeline: &Timeline) -> Vec<SoundSegment> {
    let solo = any_solo_of(timeline, TrackKind::Audio);
    let mut out = Vec::new();
    for (ti, track) in timeline.tracks().enumerate() {
        if track.kind != TrackKind::Audio || !track.enabled || !is_audible(track, solo) {
            continue;
        }
        for (ii, item) in track.children.iter().enumerate() {
            let Item::Clip(c) = item else { continue };
            if !c.enabled {
                continue;
            }
            let (Some(span), Some(source)) = (track.range_of_child(ii), c.trimmed_range()) else {
                continue;
            };
            let reversed = c.retime.as_ref().is_some_and(|r| r.reversed);
            let source_start = if reversed {
                source.end_time_exclusive()
            } else {
                source.start_time
            };
            out.push(SoundSegment {
                record: span,
                track: ti,
                item: ii,
                clip: item.ucg_id(),
                // Sound is continuous, not a picture held for a frame: played
                // backwards it starts at the *end* of its range and reads down
                // to the start, so the first thing heard is the last instant
                // of the range, not the start of its last frame.
                source_start,
                timing: timing_of(c, span.start_time, source_start),
                keep_pitch: c.retime.as_ref().is_some_and(|r| r.keep_pitch),
            });
        }
    }
    out
}
