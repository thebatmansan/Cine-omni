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

/// How fast media time runs against record time on one segment.
///
/// [`ucg_core::Speed`]: the same type the player uses, so there is one answer
/// to "which picture is showing" rather than two that can drift apart.
pub use ucg_core::Speed;

fn speed_of(clip: &Clip) -> Option<Speed> {
    let ((media, track), reversed) = clip.speed()?;
    Some(Speed {
        media,
        track,
        reversed,
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
    /// How fast the media runs here. `None` is real time — the overwhelmingly
    /// common case, and a player that ignores this field is then still right.
    pub speed: Option<Speed>,
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
            speed: speed_of(media),
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
    /// Media time heard at `record.start_time`, in source coordinates.
    pub source_start: RationalTime,
    /// How fast the media runs. Sound at a speed other than real time needs
    /// resampling, which the mixer does not do; it is reported so that a
    /// caller refuses rather than plays it at the wrong pitch.
    pub speed: Option<Speed>,
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
            out.push(SoundSegment {
                record: span,
                track: ti,
                item: ii,
                clip: item.ucg_id(),
                source_start: source.start_time,
                speed: speed_of(c),
            });
        }
    }
    out
}
