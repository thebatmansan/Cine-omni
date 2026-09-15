//! Flattening a layered cut into the edit list a player plays.

use cine_timeline::prelude::*;
use cine_timeline::{picture_segments, TrackState};

fn f(n: i64) -> RationalTime {
    RationalTime::from_frames(n, 24.0)
}

fn clip(name: &str, start: i64, dur: i64) -> Clip {
    Clip::external(
        name,
        format!("file:///{name}"),
        Some(TimeRange::from_frames(0, 1000, 24.0)),
        Some(TimeRange::from_frames(start, dur, 24.0)),
    )
}

fn timeline(tracks: Vec<Track>) -> Timeline {
    let mut t = Timeline::new("t");
    for tr in tracks {
        t.tracks.children.push(Composable::Track(tr));
    }
    t
}

#[test]
fn one_track_is_its_clips_in_order_with_gaps_left_out() {
    let v1 = Track::new("V1", TrackKind::Video)
        .with(clip("a", 100, 24))
        .with(Gap::new(f(12)))
        .with(clip("b", 0, 48));
    let list = picture_segments(&timeline(vec![v1]));
    assert_eq!(list.segments.len(), 2);
    assert_eq!(list.segments[0].record, TimeRange::from_frames(0, 24, 24.0));
    assert_eq!(list.segments[0].source_start, f(100));
    assert_eq!(
        list.segments[1].record,
        TimeRange::from_frames(36, 48, 24.0)
    );
    assert_eq!(list.end, Some(f(84)));
}

#[test]
fn the_top_visible_track_wins_and_the_one_below_resumes_mid_clip() {
    let v1 = Track::new("V1", TrackKind::Video).with(clip("base", 0, 100));
    let v2 = Track::new("V2", TrackKind::Video)
        .with(Gap::new(f(30)))
        .with(clip("insert", 500, 20));
    let list = picture_segments(&timeline(vec![v1, v2]));
    let got: Vec<(i64, i64, usize, i64)> = list
        .segments
        .iter()
        .map(|s| {
            (
                s.record.start_time.ticks(),
                s.record.duration.ticks(),
                s.track,
                s.source_start.ticks(),
            )
        })
        .collect();
    assert_eq!(got, vec![(0, 30, 0, 0), (30, 20, 1, 500), (50, 50, 0, 50)]);
}

#[test]
fn a_muted_or_disabled_track_is_not_seen_and_solo_isolates() {
    let v1 = Track::new("V1", TrackKind::Video).with(clip("base", 0, 50));
    let mut v2 = Track::new("V2", TrackKind::Video).with(clip("top", 0, 50));
    TrackState {
        muted: true,
        ..Default::default()
    }
    .apply(&mut v2);
    let list = picture_segments(&timeline(vec![v1.clone(), v2.clone()]));
    assert_eq!(
        list.segments[0].track, 0,
        "muted top track is not the picture"
    );

    let mut solo_v1 = v1;
    TrackState {
        solo: true,
        ..Default::default()
    }
    .apply(&mut solo_v1);
    TrackState::default().apply(&mut v2);
    let list = picture_segments(&timeline(vec![solo_v1, v2]));
    assert_eq!(list.segments[0].track, 0, "solo on V1 hides an unmuted V2");
}

#[test]
fn transitions_are_counted_because_they_play_as_cuts() {
    let v1 = Track::new("V1", TrackKind::Video)
        .with(clip("a", 0, 48))
        .with(Transition::dissolve("x", f(6), f(6)))
        .with(clip("b", 48, 48));
    let a1 = Track::new("A1", TrackKind::Audio).with(clip("snd", 0, 96));
    let list = picture_segments(&timeline(vec![v1, a1]));
    assert_eq!(list.transitions, 1);
    assert_eq!(list.segments.len(), 2, "audio tracks are not picture");
    assert_eq!(list.segments[1].record.start_time, f(48));
}

#[test]
fn rates_are_kept_exact_across_ntsc() {
    let ntsc = |n| RationalTime::exact(n, ucg_core::Rate::F23_976);
    let c = Clip::external(
        "ntsc",
        "file:///ntsc",
        None,
        Some(TimeRange::new(ntsc(1001), ntsc(2002))),
    );
    let v1 = Track::new("V1", TrackKind::Video)
        .with(Gap::new(f(1)))
        .with(c);
    let list = picture_segments(&timeline(vec![v1]));
    let s = &list.segments[0];
    assert_eq!(s.record.start_time, f(1));
    assert_eq!(s.source_start, ntsc(1001));
    assert_eq!(s.record.duration, ntsc(2002));
}

#[test]
fn every_audible_sound_clip_plays_and_muted_tracks_do_not() {
    let a1 = Track::new("A1", TrackKind::Audio).with(clip("dialogue", 10, 48));
    let mut a2 = Track::new("A2", TrackKind::Audio)
        .with(Gap::new(f(12)))
        .with(clip("music", 0, 96));
    let v1 = Track::new("V1", TrackKind::Video).with(clip("pic", 0, 48));
    let got = cine_timeline::sound_segments(&timeline(vec![v1.clone(), a1.clone(), a2.clone()]));
    assert_eq!(got.len(), 2, "both sound tracks, not the picture");
    assert_eq!(got[1].record, TimeRange::from_frames(12, 96, 24.0));
    assert_eq!(got[0].source_start, f(10));
    TrackState {
        muted: true,
        ..Default::default()
    }
    .apply(&mut a2);
    let got = cine_timeline::sound_segments(&timeline(vec![v1, a1, a2]));
    assert_eq!(got.len(), 1);
}
