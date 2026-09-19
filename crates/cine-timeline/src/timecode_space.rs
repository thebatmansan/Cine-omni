//! Putting an OTIO file in the coordinates OTIO is read in.
//!
//! This build counts a clip's source range from **zero inside the file**: a
//! clip that plays from ten seconds in starts at frame 240, whatever the
//! camera called that frame. That is a deliberate simplification and it holds
//! everywhere inside the application — trims clamp against an available range
//! that also starts at zero, and the two agree.
//!
//! OTIO does not work that way. A media reference's `available_range` is in
//! the media's *own* coordinates, which for a take with timecode is timecode,
//! and a clip's `source_range` is in the same coordinates. So writing an
//! available range of `0 … duration` for a take the camera numbered
//! `01:00:00:00` is not a simplification a reader can see — it is a statement
//! that the take begins at midnight, and every tool that conforms from the
//! file will look for the wrong frames.
//!
//! The two spaces are one addition apart, so the conversion happens at the
//! edge of the file rather than in the model: [`into_timecode`] on the way
//! out, [`into_media_relative`] on the way in. Nothing in between changes.
//!
//! ## What is not converted
//!
//! Media with no timecode, or whose timecode cannot label a frame — a counter,
//! a discontinuous track — has no camera numbering to be in, so its clips stay
//! at zero and an available range of `0 … duration` is then exactly true.
//! Record time is untouched: a clip's position on the track is the cut's
//! clock, which OTIO carries separately as the timeline's global start.

use crate::otio::{Clip, Composable, Item, MediaReference, Timeline};
use ucg_core::{RationalTime, TimeRange};

/// Where a clip's media begins in the camera's numbering.
pub type StartOf<'a> = dyn Fn(&Clip) -> Option<RationalTime> + 'a;

/// Every clip of a timeline, to be changed in place.
fn clips_mut(timeline: &mut Timeline) -> Vec<&mut Clip> {
    fn stack<'a>(s: &'a mut crate::otio::Stack, out: &mut Vec<&'a mut Clip>) {
        for child in &mut s.children {
            match child {
                Composable::Track(t) => {
                    for item in &mut t.children {
                        if let Item::Clip(c) = item {
                            out.push(c);
                        }
                    }
                }
                Composable::Stack(inner) => stack(inner, out),
            }
        }
    }
    let mut out = Vec::new();
    stack(&mut timeline.tracks, &mut out);
    out
}

/// Move a clip's source range and available range by `by`.
fn shift(clip: &mut Clip, by: RationalTime) {
    if let Some(range) = clip.source_range.as_mut() {
        if range.start_time.rate_exact() == by.rate_exact() {
            *range = TimeRange::new(range.start_time.checked_add(&by), range.duration);
        }
    }
    if let MediaReference::External {
        available_range, ..
    } = &mut clip.media_reference
    {
        if let Some(range) = available_range.as_mut() {
            if range.start_time.rate_exact() == by.rate_exact() {
                *range = TimeRange::new(range.start_time.checked_add(&by), range.duration);
            }
        } else {
            // A reference with no stated length still says where its media
            // begins, which is the part a conform needs.
            *available_range = Some(TimeRange::new(by, RationalTime::zero(by.rate_exact())));
        }
    }
}

/// Put `timeline` in the camera's numbering, for writing OTIO.
///
/// Returns the number of clips moved. A clip whose media has no usable
/// timecode is left where it is, and its available range starting at zero is
/// then true rather than a claim.
pub fn into_timecode(timeline: &mut Timeline, start_of: &StartOf<'_>) -> usize {
    let mut moved = 0;
    for clip in clips_mut(timeline) {
        let Some(base) = start_of(clip) else { continue };
        if base.ticks() == 0 {
            continue;
        }
        shift(clip, base);
        moved += 1;
    }
    moved
}

/// Put `timeline` back in media-relative coordinates, for reading OTIO.
///
/// The available range says where the media begins, so subtracting its start
/// is the inverse of [`into_timecode`] — and is also what makes a file written
/// by another tool, which was in timecode all along, land in the coordinates
/// this build edits in.
pub fn into_media_relative(timeline: &mut Timeline) -> usize {
    let mut moved = 0;
    for clip in clips_mut(timeline) {
        let Some(range) = clip.available_range() else {
            continue;
        };
        if range.start_time.ticks() == 0 {
            continue;
        }
        let back = RationalTime::zero(range.start_time.rate_exact()).checked_sub(&range.start_time);
        shift(clip, back);
        moved += 1;
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otio::{Track, TrackKind};

    fn at(n: i64) -> RationalTime {
        RationalTime::exact(n, ucg_core::Rate::F24)
    }

    fn one_clip(source: TimeRange, available: Option<TimeRange>) -> Timeline {
        let clip = Clip::new(
            "A001C001",
            MediaReference::external("/card/A001C001.mov", available),
            Some(source),
        );
        let mut track = Track::new("V1", TrackKind::Video);
        track.children.push(Item::Clip(clip));
        let mut t = Timeline::new("Cut");
        t.tracks.children.push(Composable::Track(track));
        t
    }

    fn only(t: &Timeline) -> &Clip {
        match &t.tracks.children[0] {
            Composable::Track(track) => match &track.children[0] {
                Item::Clip(c) => c,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_take_at_an_hour_is_written_at_an_hour_and_read_back_at_zero() {
        let hour = at(86_400);
        let mut t = one_clip(
            TimeRange::new(at(24), at(12)),
            Some(TimeRange::new(at(0), at(72))),
        );
        assert_eq!(into_timecode(&mut t, &|_| Some(hour)), 1);
        let c = only(&t);
        assert_eq!(c.source_range.unwrap().start_time, at(86_424));
        assert_eq!(
            c.source_range.unwrap().duration,
            at(12),
            "length is untouched"
        );
        assert_eq!(c.available_range().unwrap().start_time, hour);
        assert_eq!(c.available_range().unwrap().duration, at(72));

        // And back, which is what reading the file does.
        assert_eq!(into_media_relative(&mut t), 1);
        let c = only(&t);
        assert_eq!(c.source_range.unwrap().start_time, at(24));
        assert_eq!(c.available_range().unwrap().start_time, at(0));
    }

    #[test]
    fn media_with_no_timecode_is_left_where_it_is() {
        let mut t = one_clip(
            TimeRange::new(at(24), at(12)),
            Some(TimeRange::new(at(0), at(72))),
        );
        assert_eq!(into_timecode(&mut t, &|_| None), 0);
        assert_eq!(only(&t).source_range.unwrap().start_time, at(24));
        // Nothing to undo either: an available range at zero is true here.
        assert_eq!(into_media_relative(&mut t), 0);
    }

    #[test]
    fn a_file_from_another_tool_lands_in_the_coordinates_this_build_edits_in() {
        // Never converted by us: written by something else, in timecode all
        // along. Reading it has to produce a media-relative source range or
        // every trim would clamp against the wrong numbers.
        let mut t = one_clip(
            TimeRange::new(at(86_424), at(12)),
            Some(TimeRange::new(at(86_400), at(72))),
        );
        assert_eq!(into_media_relative(&mut t), 1);
        let c = only(&t);
        assert_eq!(c.source_range.unwrap().start_time, at(24));
        assert_eq!(c.available_range().unwrap().start_time, at(0));
        assert_eq!(c.available_range().unwrap().duration, at(72));
    }
}
