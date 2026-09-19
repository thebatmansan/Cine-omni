//! Retiming: a clip that does not play in real time.
//!
//! The model's retime is a pair of exact durations — the media consumed and
//! the track time it takes — rather than a floating-point scalar, so the
//! assertions here are equalities, not tolerances. What is tested is that the
//! pair survives every path a clip takes: composition, the OTIO file, the
//! graph, the flattened picture list, the EDL, and the edits that must refuse
//! it rather than guess.

use cine_timeline::edl::write_edl;
use cine_timeline::graph::{create_sequence, export_track, set_clip_retime, sibling_tracks};
use cine_timeline::prelude::*;
use cine_timeline::three_point::{EditKind, Marks, Mismatch};
use cine_timeline::{picture_segments, Retime};
use ucg_core::{Engine, EntityId, Rate, UcgError};
use ucg_schema::{attrs, kinds};

fn f(n: i64) -> RationalTime {
    RationalTime::from_frames(n, 24.0)
}

/// 240 frames of media, 48 of which are used.
fn clip(name: &str) -> Clip {
    Clip::external(
        name,
        format!("file:///{name}.mov"),
        Some(TimeRange::from_frames(0, 240, 24.0)),
        Some(TimeRange::from_frames(100, 48, 24.0)),
    )
}

fn retimed(name: &str, played: i64, reversed: bool) -> Clip {
    let mut c = clip(name);
    c.retime = Some(Retime::new(f(played), reversed));
    c
}

/// The whole point: the clip's length on the track is the retime's, and the
/// media it consumes is unchanged.
#[test]
fn a_retimed_clip_takes_its_own_length_on_the_track() {
    let track = Track::new("V1", TrackKind::Video)
        .with(retimed("A", 24, false)) // 48 media frames over 24: double speed
        .with(clip("B"));
    assert_eq!(
        track.range_of_child(0),
        Some(TimeRange::from_frames(0, 24, 24.0))
    );
    assert_eq!(
        track.range_of_child(1),
        Some(TimeRange::from_frames(24, 48, 24.0)),
        "what follows moves up: a retime is a ripple"
    );
    assert_eq!(track.duration(), f(72));

    let a = track.children[0].as_clip().unwrap();
    assert_eq!(a.media_duration(), f(48), "the media used is unchanged");
    assert_eq!(a.speed(), Some(((2, 1), false)));

    // Half speed the other way: 48 frames over 96.
    let slow = Track::new("V1", TrackKind::Video).with(retimed("A", 96, false));
    assert_eq!(
        slow.children[0].as_clip().unwrap().speed(),
        Some(((1, 2), false))
    );
}

/// Which picture plays when — exactly, and floored onto real frames.
#[test]
fn the_picture_shown_is_the_one_that_has_started() {
    let double = retimed("A", 24, false);
    let at = |n: i64| double.media_time_at(f(0), f(n));
    assert_eq!(at(0), Some(f(100)), "the first frame of the range");
    assert_eq!(at(1), Some(f(102)), "twice as fast: two media frames on");
    assert_eq!(at(23), Some(f(146)));
    assert_eq!(at(24), None, "past the end of the clip");

    // Half speed: each media frame is held for two.
    let half = retimed("A", 96, false);
    let at = |n: i64| half.media_time_at(f(0), f(n));
    assert_eq!(at(0), Some(f(100)));
    assert_eq!(at(1), Some(f(100)), "the frame has not changed yet");
    assert_eq!(at(2), Some(f(101)));
    assert_eq!(at(95), Some(f(147)), "and the last frame is the last frame");

    // A speed that is not a whole number stays exact: 48 over 30 is 8/5.
    let odd = retimed("A", 30, false);
    assert_eq!(odd.speed(), Some(((8, 5), false)));
    assert_eq!(odd.media_time_at(f(0), f(5)), Some(f(108)));
    assert_eq!(
        odd.media_time_at(f(0), f(1)),
        Some(f(101)),
        "1.6 floors to 1"
    );
}

/// Backwards: the first picture shown is the last frame of the range.
#[test]
fn a_reversed_clip_starts_at_the_end_of_its_media() {
    let back = retimed("A", 48, true);
    assert_eq!(back.media_time_at(f(0), f(0)), Some(f(147)));
    assert_eq!(back.media_time_at(f(0), f(1)), Some(f(146)));
    assert_eq!(back.media_time_at(f(0), f(47)), Some(f(100)));
    assert_eq!(back.media_time_at(f(0), f(48)), None);
}

/// The OTIO file carries both: the exact pair for a round trip, and the float
/// `time_scalar` other tools read.
#[test]
fn otio_carries_the_exact_pair_and_the_scalar_other_tools_read() {
    let mut timeline = Timeline::new("retimed");
    timeline.tracks.children.push(Composable::Track(
        Track::new("V1", TrackKind::Video)
            .with(retimed("A", 30, false))
            .with(retimed("B", 48, true)),
    ));
    let text = write_otio(&timeline);
    assert!(text.contains("LinearTimeWarp.1"), "{text}");
    assert!(text.contains("\"time_scalar\": 1.6"), "8/5 is 1.6:\n{text}");
    assert!(
        text.contains("\"time_scalar\": -1.0"),
        "reversed is negative"
    );
    assert_eq!(
        read_otio(&text).unwrap(),
        timeline,
        "and the exact pair comes back unchanged"
    );
}

/// A file from another tool has only a float. The model is exact, so the
/// rounding happens once, on the way in.
#[test]
fn a_foreign_time_scalar_is_rounded_onto_a_whole_frame_on_the_way_in() {
    let text = r#"{
        "OTIO_SCHEMA": "Clip.1", "name": "foreign",
        "source_range": { "OTIO_SCHEMA": "TimeRange.1",
            "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 0.0 },
            "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 48.0 } },
        "media_reference": { "OTIO_SCHEMA": "ExternalReference.1",
            "target_url": "file:///a.mov", "available_range": null },
        "metadata": {}, "markers": [],
        "effects": [ { "OTIO_SCHEMA": "LinearTimeWarp.1", "name": "w",
            "effect_name": "LinearTimeWarp", "metadata": {}, "time_scalar": 2.0 } ]
    }"#;
    let timeline: Timeline = read_otio(&format!(
        r#"{{ "OTIO_SCHEMA": "Timeline.1", "name": "t", "metadata": {{}},
             "tracks": {{ "OTIO_SCHEMA": "Stack.1", "name": "tracks", "metadata": {{}},
               "markers": [], "effects": [], "children": [
                 {{ "OTIO_SCHEMA": "Track.1", "name": "V1", "kind": "Video", "metadata": {{}},
                    "markers": [], "effects": [], "source_range": null,
                    "children": [ {text} ] }} ] }} }}"#
    ))
    .expect("read");
    let clip = timeline.tracks().next().unwrap().children[0]
        .as_clip()
        .unwrap();
    assert_eq!(
        clip.retime,
        Some(Retime::new(f(24), false)),
        "double speed over 48 frames of media is 24 frames of track"
    );
    assert_eq!(clip.duration(), f(24));
}

/// A freeze frame is one picture held: this model says so in its own terms.
#[test]
fn a_freeze_frame_becomes_one_picture_held_for_the_clips_length() {
    let text = r#"{ "OTIO_SCHEMA": "Timeline.1", "name": "t", "metadata": {},
        "tracks": { "OTIO_SCHEMA": "Stack.1", "name": "tracks", "metadata": {},
          "markers": [], "effects": [], "children": [
            { "OTIO_SCHEMA": "Track.1", "name": "V1", "kind": "Video", "metadata": {},
              "markers": [], "effects": [], "source_range": null, "children": [
                { "OTIO_SCHEMA": "Clip.1", "name": "held", "metadata": {}, "markers": [],
                  "source_range": { "OTIO_SCHEMA": "TimeRange.1",
                    "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 10.0 },
                    "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 72.0 } },
                  "media_reference": { "OTIO_SCHEMA": "ExternalReference.1",
                    "target_url": "file:///a.mov", "available_range": null },
                  "effects": [ { "OTIO_SCHEMA": "FreezeFrame.1", "name": "f",
                    "effect_name": "FreezeFrame", "metadata": {}, "time_scalar": 0.0 } ] } ] } ] } }"#;
    let timeline = read_otio(text).expect("read");
    let clip = timeline.tracks().next().unwrap().children[0]
        .as_clip()
        .unwrap();
    assert_eq!(
        clip.source_range,
        Some(TimeRange::from_frames(10, 1, 24.0)),
        "one picture"
    );
    assert_eq!(clip.duration(), f(72), "held for the clip's length");
    assert_eq!(
        clip.media_time_at(f(0), f(70)),
        Some(f(10)),
        "still that one"
    );
}

/// Splitting a retimed clip: the halves keep their track lengths exactly and
/// divide the media between them with nothing lost.
#[test]
fn a_split_divides_the_media_and_keeps_both_track_lengths() {
    let mut track = Track::new("V1", TrackKind::Video).with(retimed("A", 24, false));
    split_at(&mut track, f(10)).expect("split");
    assert_eq!(track.children.len(), 2);
    let (a, b) = (
        track.children[0].as_clip().unwrap(),
        track.children[1].as_clip().unwrap(),
    );
    assert_eq!(a.duration(), f(10), "the first half is where the cut was");
    assert_eq!(b.duration(), f(14));
    assert_eq!(track.duration(), f(24), "and the whole is unchanged");
    assert_eq!(a.source_range.unwrap().start_time, f(100));
    assert_eq!(
        a.source_range.unwrap().end_time_exclusive(),
        b.source_range.unwrap().start_time,
        "the media divides with no gap and no overlap"
    );
    assert_eq!(b.source_range.unwrap().end_time_exclusive(), f(148));
    // And the picture at the cut is the same one either side of the seam.
    assert_eq!(b.media_time_at(f(10), f(10)), Some(f(120)));

    // A cut so fine that one side would have no picture at all is refused.
    let mut hard = Track::new("V1", TrackKind::Video).with(retimed("A", 2, false));
    let e = split_at(&mut hard, f(1)).expect("a one-frame side is still a picture");
    assert_eq!(e, 1);
}

/// Trims, rolls, slides and joins refuse rather than guess what a retime
/// should do. Slipping is allowed: it changes neither speed nor length.
#[test]
fn the_edits_that_would_have_to_guess_refuse_and_say_why() {
    let mut track = Track::new("V1", TrackKind::Video)
        .with(retimed("A", 24, false))
        .with(clip("B"));

    let e = ripple_trim(&mut track, 0, Edge::Tail, f(6)).expect_err("a trim");
    assert!(e.to_string().contains("retimed to 2/1"), "{e}");
    assert!(e.to_string().contains("fraction of a frame"), "{e}");

    assert!(roll(&mut track, 0, f(2)).is_err());
    assert!(
        slide(&mut track, 1, f(2)).is_err(),
        "its neighbour is retimed"
    );
    assert!(join(&mut track, 0).is_err());

    // Slip moves the media under the retime and changes nothing else.
    let before = track.duration();
    let applied = slip(&mut track, 0, f(-4)).expect("slip");
    assert_eq!(applied, f(-4));
    assert_eq!(track.duration(), before);
    assert_eq!(
        track.children[0]
            .as_clip()
            .unwrap()
            .source_range
            .unwrap()
            .start_time,
        f(96)
    );
    assert_eq!(track.children[0].as_clip().unwrap().duration(), f(24));
}

/// The flattened picture list carries the speed, and the media time it gives
/// at each instant is the clip's own arithmetic.
#[test]
fn the_picture_list_says_how_fast_the_media_runs() {
    let mut timeline = Timeline::new("t");
    timeline.tracks.children.push(Composable::Track(
        Track::new("V1", TrackKind::Video).with(retimed("A", 24, false)),
    ));
    let list = picture_segments(&timeline);
    assert_eq!(list.segments.len(), 1);
    let s = &list.segments[0];
    assert_eq!(s.record, TimeRange::from_frames(0, 24, 24.0));
    assert_eq!(s.source_start, f(100));
    let timing = s.timing.as_ref().expect("a timing");
    assert!(timing.map.is_constant() && !timing.map.reversed);
    assert_eq!(
        (timing.record, timing.media),
        (f(0), f(100)),
        "anchored at the clip"
    );
    // Ten frames in is twenty media frames in, floored onto real pictures.
    let r24 = Rate::whole(24).unwrap();
    assert_eq!(timing.media_at(f(10), r24), f(120));
    assert_eq!(timing.media_at(f(1), r24), f(102));

    // Reversed, the offset runs the other way from a segment start that is
    // already the last frame of the range.
    let mut back = Timeline::new("t");
    back.tracks.children.push(Composable::Track(
        Track::new("V1", TrackKind::Video).with(retimed("A", 48, true)),
    ));
    let list = picture_segments(&back);
    assert_eq!(list.segments[0].source_start, f(147));
    let timing = list.segments[0].timing.as_ref().unwrap();
    assert_eq!(timing.media_at(f(7), Rate::whole(24).unwrap()), f(140));
}

/// An EDL writes the speed as an M2 record, the record columns as the track
/// length and the source columns as the media.
#[test]
fn an_edl_writes_a_motion_record_and_two_different_lengths() {
    let mut timeline = Timeline::new("Assembly");
    timeline.global_start_time = Some(f(86_400));
    timeline.tracks.children.push(Composable::Track(
        Track::new("V1", TrackKind::Video).with(retimed("A001C001", 24, false)),
    ));
    let (text, report) = write_edl(&timeline, 0, "Assembly", &cine_timeline::edl::from_zero);
    assert!(text.contains("M2   A001C001"), "{text}");
    assert!(
        text.contains("48.00"),
        "M2 carries the play rate, not the ratio: 24 fps media at double speed is 48:\n{text}"
    );
    assert!(
        text.contains("00:00:04:04 00:00:06:04 01:00:00:00 01:00:01:00"),
        "48 media frames over 24 of record:\n{text}"
    );
    assert!(
        report.left_behind.iter().any(|w| w.contains("M2 record")),
        "{:?}",
        report.left_behind
    );
}

/// Fit to fill: the fourth way to settle four marks, and it is exact.
#[test]
fn fit_to_fill_makes_the_marked_media_fill_the_marked_time() {
    let p = cine_timeline::three_point::plan(
        Some(TimeRange::from_frames(0, 240, 24.0)),
        Marks::range(f(48), f(96)), // 48 frames of media
        Marks::range(f(0), f(24)),  // over 24 of timeline
        EditKind::Overwrite,
        Mismatch::FitToFill,
    )
    .expect("a plan");
    assert_eq!(p.source, TimeRange::from_frames(48, 48, 24.0));
    assert_eq!(p.record, TimeRange::from_frames(0, 24, 24.0));
    assert_eq!(p.retime, Some(Retime::new(f(24), false)));
    assert!(p.reconciled.unwrap().contains("a speed of 2/1"));

    let mut track = Track::new("V1", TrackKind::Video);
    let p = cine_timeline::three_point::plan(
        Some(TimeRange::from_frames(0, 240, 24.0)),
        Marks::range(f(0), f(48)),
        Marks::range(f(0), f(30)),
        EditKind::Insert,
        Mismatch::FitToFill,
    )
    .expect("a plan");
    cine_timeline::three_point::perform(&mut track, clip("A"), &p).expect("performed");
    assert_eq!(track.duration(), f(30), "it fills exactly the marked time");
    assert_eq!(
        track.children[0].as_clip().unwrap().speed(),
        Some(((8, 5), false)),
        "and the speed is exact, not 1.6000000000000001"
    );
}

/// Through the graph: a retime ripples the track and comes back unchanged.
#[test]
fn a_retime_survives_the_graph_and_moves_what_follows() {
    let engine = Engine::new();
    let project = EntityId::from_key("project:test");
    let asset = EntityId::from_key("media:file:shot");
    let mut seq = EntityId::nil();
    engine
        .transact("setup", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            tx.create_with_id(asset, kinds::ASSET_MEDIA)?;
            tx.set(asset, attrs::NAME, "A001C001.mov")?;
            tx.set(asset, attrs::MEDIA_PATH, "/footage/A001C001.mov")?;
            tx.set(
                asset,
                attrs::AVAILABLE_RANGE,
                TimeRange::from_frames(0, 240, 24.0),
            )?;
            seq = create_sequence(tx, project, "Assembly", Rate::whole(24).unwrap())
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    let g = engine.read();
    let stack = g.children(seq)[0];
    let first = g
        .children(stack)
        .into_iter()
        .find(|c| g.node(*c).unwrap().kind.as_str() == kinds::NLE_TRACK)
        .unwrap();
    let video = sibling_tracks(&g, first).unwrap()[0];
    drop(g);

    // Two clips, 48 frames each.
    for at in [0, 48] {
        engine
            .transact("place", |tx| {
                cine_timeline::graph::place_material(
                    tx,
                    &[video],
                    f(at),
                    &cine_timeline::graph::ClipSpec {
                        media: asset,
                        source_range: TimeRange::from_frames(0, 48, 24.0),
                        name: None,
                    },
                    false,
                )
                .map(|_| ())
                .map_err(|e| UcgError::Aborted(e.to_string()))
            })
            .unwrap();
    }
    let g = engine.read();
    let clip_id = g.children(video)[0];
    assert_eq!(export_track(&g, video).unwrap().duration(), f(96));
    drop(g);

    engine
        .transact("retime", |tx| {
            set_clip_retime(tx, clip_id, Some(Retime::new(f(24), false)))
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();

    let g = engine.read();
    let track = export_track(&g, video).unwrap();
    assert_eq!(track.duration(), f(72), "the track got shorter");
    assert_eq!(
        track.children[0].as_clip().unwrap().retime,
        Some(Retime::new(f(24), false))
    );
    assert_eq!(
        track.range_of_child(1),
        Some(TimeRange::from_frames(24, 48, 24.0)),
        "and the clip after it moved up"
    );
    drop(g);

    // Undo puts it back: a retime is one commit like any other edit.
    engine.undo().unwrap();
    let g = engine.read();
    let track = export_track(&g, video).unwrap();
    assert_eq!(track.duration(), f(96));
    assert_eq!(track.children[0].as_clip().unwrap().retime, None);
}

/// Keeping pitch is part of the retime and survives the file and the graph.
#[test]
fn keeping_pitch_survives_otio_and_reaches_the_sound_segment() {
    let mut c = retimed("A", 24, false);
    c.retime.as_mut().unwrap().keep_pitch = true;
    let mut timeline = Timeline::new("t");
    timeline.tracks.children.push(Composable::Track(
        Track::new("A1", TrackKind::Audio).with(c),
    ));
    let text = write_otio(&timeline);
    assert!(text.contains("\"keep_pitch\": true"), "{text}");
    let back = read_otio(&text).unwrap();
    assert_eq!(back, timeline);
    let sound = cine_timeline::sound_segments(&back);
    assert!(sound[0].keep_pitch);
    assert_eq!(
        sound[0].timing.as_ref().map(|t| t.map.consumed()),
        Some(ucg_core::Q::int(2)),
        "48 media frames at 24 fps, over one second of track"
    );
}

/// A speed ramp: real time to double speed across two seconds of track.
fn ramped(name: &str) -> Clip {
    use ucg_core::{Ramp, Ratio};
    let mut c = clip(name);
    // 1x -> 2x over 48 frames of track consumes 72 frames of media.
    c.source_range = Some(TimeRange::from_frames(100, 72, 24.0));
    c.retime = Some(Retime {
        ramp: Some(vec![Ramp {
            length: f(48),
            from: Ratio::new(1, 1),
            to: Ratio::new(2, 1),
        }]),
        ..Retime::new(f(48), false)
    });
    c
}

/// A ramp is exact: the picture at each instant is the integral of the speed,
/// floored onto real frames, and it speeds up as it goes.
#[test]
fn a_ramp_shows_the_picture_the_integral_of_its_speed_says() {
    let c = ramped("A");
    assert!(c.is_ramped());
    assert_eq!(c.duration(), f(48));
    let at = |n: i64| c.media_time_at(f(0), f(n)).unwrap();
    assert_eq!(at(0), f(100), "starts on the in point");
    // Halfway (1 s): 1·1 + 1·1²/(2·2) = 1.25 s = 30 frames in.
    assert_eq!(at(24), f(130));
    // Near the end the speed is nearly 2: consecutive frames skip.
    assert_eq!(at(47).checked_sub(&at(46)), f(2));
    // And early on it is nearly 1: consecutive frames advance by one.
    assert_eq!(at(2).checked_sub(&at(1)), f(1));
    // It never reads past the range.
    assert!(at(47) < f(172));
}

/// A ramp survives the OTIO file and the graph, exactly.
#[test]
fn a_ramp_survives_otio_and_the_flattened_list_reads_it() {
    let mut timeline = Timeline::new("t");
    timeline.tracks.children.push(Composable::Track(
        Track::new("V1", TrackKind::Video)
            .with(ramped("A"))
            .with(clip("B")),
    ));
    let text = write_otio(&timeline);
    assert!(text.contains("\"ramp\""), "{text}");
    assert!(
        text.contains("\"time_scalar\": 1.5"),
        "the average for other tools: {text}"
    );
    let back = read_otio(&text).unwrap();
    assert_eq!(back, timeline);

    let list = picture_segments(&back);
    let t = list.segments[0].timing.as_ref().expect("timed");
    let r24 = Rate::whole(24).unwrap();
    assert_eq!(
        t.media_at(f(24), r24),
        f(130),
        "the list reads the ramp, not an average"
    );
    assert_eq!(
        list.segments[1].record.start_time,
        f(48),
        "and the next clip starts after it"
    );
}

/// Cutting inside a ramp is refused, with the reason, rather than approximated.
#[test]
fn a_cut_inside_a_ramp_is_refused_with_the_reason() {
    let mut track = Track::new("V1", TrackKind::Video).with(ramped("A"));
    let e = split_at(&mut track, f(10)).expect_err("inside the ramp");
    assert!(e.to_string().contains("speed ramp"), "{e}");
    assert_eq!(track.children.len(), 1, "and nothing was cut");
}

/// Through the graph: setting a ramp decides the track length and the media
/// consumed, and a ramp that would read past the shot is refused.
#[test]
fn setting_a_ramp_decides_both_lengths_and_refuses_media_that_is_not_there() {
    use ucg_core::{Ramp, Ratio};
    let engine = Engine::new();
    let project = EntityId::from_key("project:test");
    let asset = EntityId::from_key("media:file:shot");
    let mut video = EntityId::nil();
    engine
        .transact("setup", |tx| {
            tx.create_with_id(project, kinds::PROJECT)?;
            tx.create_with_id(asset, kinds::ASSET_MEDIA)?;
            tx.set(asset, attrs::MEDIA_PATH, "/footage/A001C001.mov")?;
            tx.set(
                asset,
                attrs::AVAILABLE_RANGE,
                TimeRange::from_frames(0, 240, 24.0),
            )?;
            let seq = create_sequence(tx, project, "Ramp", Rate::whole(24).unwrap())
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            let stack = tx.graph().children(seq)[0];
            let first = tx
                .graph()
                .children(stack)
                .into_iter()
                .find(|c| tx.graph().node(*c).unwrap().kind.as_str() == kinds::NLE_TRACK)
                .unwrap();
            video = sibling_tracks(tx.graph(), first).unwrap()[0];
            cine_timeline::graph::place_material(
                tx,
                &[video],
                f(0),
                &cine_timeline::graph::ClipSpec {
                    media: asset,
                    source_range: TimeRange::from_frames(100, 48, 24.0),
                    name: None,
                },
                false,
            )
            .map(|_| ())
            .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let clip = engine.read().children(video)[0];
    let ramp = |to: i64| Retime {
        ramp: Some(vec![Ramp {
            length: f(48),
            from: Ratio::new(1, 1),
            to: Ratio::new(to, 1),
        }]),
        ..Retime::new(f(1), false)
    };
    engine
        .transact("ramp", |tx| {
            set_clip_retime(tx, clip, Some(ramp(2))).map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .unwrap();
    let track = export_track(&engine.read(), video).unwrap();
    let c = track.children[0].as_clip().unwrap();
    assert_eq!(
        c.duration(),
        f(48),
        "the ramp's segments decide the track length"
    );
    assert_eq!(
        c.source_range,
        Some(TimeRange::from_frames(100, 72, 24.0)),
        "and what they consume decides the media"
    );
    assert!(c.is_ramped());

    // 1x -> 10x over two seconds consumes 11 s = 264 frames; the shot has 140
    // after the in point.
    let e = engine
        .transact("too far", |tx| {
            set_clip_retime(tx, clip, Some(ramp(10))).map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .expect_err("past the end of the shot");
    assert!(
        e.to_string().contains("more media than the shot has"),
        "{e}"
    );
}
