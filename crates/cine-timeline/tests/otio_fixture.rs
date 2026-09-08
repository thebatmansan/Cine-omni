//! Parse the hand-written OTIO fixture and round-trip it.

use cine_timeline::prelude::*;

const FIXTURE: &str = include_str!("../../../tests/fixtures/sample.otio");

fn fixture() -> Timeline {
    read_otio(FIXTURE).expect("fixture parses")
}

#[test]
fn fixture_structure() {
    let tl = fixture();
    assert_eq!(tl.name, "Demo Reel v3");
    assert_eq!(tl.global_start_time, Some(RationalTime::new(86400.0, 24.0)));
    assert_eq!(tl.metadata.get("fps").and_then(|v| v.as_f64()), Some(24.0));
    assert_eq!(tl.metadata["cine_omni"]["project"], "Demo Reel");

    let tracks: Vec<&Track> = tl.tracks().collect();
    assert_eq!(tracks.len(), 4);
    assert_eq!(tl.video_tracks().count(), 2);
    assert_eq!(tl.audio_tracks().count(), 2);
    let names: Vec<&str> = tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["V1", "V2", "A1", "A2"]);

    let v1 = tracks[0];
    assert_eq!(v1.kind, TrackKind::Video);
    assert_eq!(v1.children.len(), 5);
    assert_eq!(v1.children.iter().filter(|c| c.is_clip()).count(), 3);
    assert_eq!(v1.children.iter().filter(|c| c.is_gap()).count(), 1);
    assert_eq!(v1.children.iter().filter(|c| c.is_transition()).count(), 1);
    assert_eq!(v1.markers.len(), 1);
    assert_eq!(v1.markers[0].name, "Reel start");

    let a2 = tracks[3];
    assert_eq!(a2.kind, TrackKind::Audio);
    assert_eq!(a2.metadata["role"], "music");
}

#[test]
fn fixture_durations_and_ranges() {
    let tl = fixture();
    let tracks: Vec<&Track> = tl.tracks().collect();
    assert_eq!(tracks[0].duration(), RationalTime::from_frames(336, 24.0));
    assert_eq!(tracks[1].duration(), RationalTime::from_frames(168, 24.0));
    assert_eq!(tracks[2].duration(), RationalTime::from_frames(336, 24.0));
    assert_eq!(tracks[3].duration(), RationalTime::from_frames(324, 24.0));
    assert_eq!(tl.duration(), RationalTime::from_frames(336, 24.0));

    let v1 = tracks[0];
    assert_eq!(
        v1.range_of_child(0),
        Some(TimeRange::from_frames(0, 96, 24.0))
    );
    assert_eq!(
        v1.range_of_child(1),
        Some(TimeRange::from_frames(84, 24, 24.0))
    );
    assert_eq!(
        v1.range_of_child(2),
        Some(TimeRange::from_frames(96, 120, 24.0))
    );
    assert_eq!(
        v1.range_of_child(3),
        Some(TimeRange::from_frames(216, 48, 24.0))
    );
    assert_eq!(
        v1.range_of_child(4),
        Some(TimeRange::from_frames(264, 72, 24.0))
    );
    assert!(tl.validate().is_ok());
}

#[test]
fn fixture_transition_markers_and_media() {
    let tl = fixture();
    let v1 = tl.tracks().next().unwrap();
    let t = v1.children[1].as_transition().unwrap();
    assert_eq!(t.name, "Dissolve");
    assert_eq!(t.transition_type, TransitionType::SmpteDissolve);
    assert_eq!(t.in_offset, RationalTime::from_frames(12, 24.0));
    assert_eq!(t.out_offset, RationalTime::from_frames(12, 24.0));

    let b = v1.children[4].as_clip().unwrap();
    assert_eq!(b.name, "B002_C001");
    assert_eq!(b.markers.len(), 1);
    assert_eq!(b.markers[0].name, "Sync point");
    assert_eq!(b.markers[0].color, "RED");
    assert_eq!(b.markers[0].comment, "clap");
    assert_eq!(
        b.markers[0].marked_range,
        TimeRange::from_frames(10, 1, 24.0)
    );
    assert_eq!(
        b.media_reference.target_url(),
        Some("file:///media/B002_C001.mov")
    );
    assert_eq!(
        b.available_range(),
        Some(TimeRange::from_frames(0, 1000, 24.0))
    );
    assert_eq!(b.metadata["camera"], "B");

    let a = v1.children[0].as_clip().unwrap();
    assert_eq!(
        a.trimmed_range(),
        Some(TimeRange::from_frames(24, 96, 24.0))
    );
    assert_eq!(
        a.media_time_at(
            RationalTime::from_frames(0, 24.0),
            RationalTime::from_frames(10, 24.0)
        ),
        Some(RationalTime::from_frames(34, 24.0))
    );

    let a2 = tl.audio_tracks().nth(1).unwrap();
    let music = a2.children[1].as_clip().unwrap();
    assert_eq!(music.markers[0].name, "Downbeat");
    assert_eq!(music.markers[0].color, "PURPLE");
}

#[test]
fn write_then_read_is_lossless() {
    let tl = fixture();
    let text = write_otio(&tl);
    let back = read_otio(&text).expect("re-parse");
    assert_eq!(back, tl);
    // and stable: writing again yields identical text
    assert_eq!(write_otio(&back), text);
    // the emitted JSON uses the modern schema tags
    assert!(text.contains("\"OTIO_SCHEMA\": \"Timeline.1\""));
    assert!(text.contains("\"OTIO_SCHEMA\": \"Clip.2\""));
    assert!(text.contains("\"OTIO_SCHEMA\": \"Marker.2\""));
    assert!(text.contains("\"OTIO_SCHEMA\": \"Transition.1\""));
    assert!(text.contains("\"OTIO_SCHEMA\": \"ExternalReference.1\""));
}

#[test]
fn fixture_json_is_reproduced_structurally() {
    // Our writer's output must be JSON-equal to the fixture (same keys and
    // values), proving nothing is dropped or invented.
    let tl = fixture();
    let ours: serde_json::Value = serde_json::from_str(&write_otio(&tl)).unwrap();
    let theirs: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(ours, theirs);
}

#[test]
fn clip1_legacy_and_other_references() {
    let text = r#"{
      "OTIO_SCHEMA": "Timeline.1", "name": "legacy", "metadata": {}, "global_start_time": null,
      "tracks": { "OTIO_SCHEMA": "Stack.1", "name": "tracks", "children": [
        { "OTIO_SCHEMA": "Track.1", "name": "V1", "kind": "Video", "children": [
          { "OTIO_SCHEMA": "Clip.1", "name": "old",
            "source_range": { "OTIO_SCHEMA": "TimeRange.1",
              "start_time": { "OTIO_SCHEMA": "RationalTime.1", "value": 0, "rate": 25 },
              "duration": { "OTIO_SCHEMA": "RationalTime.1", "value": 50, "rate": 25 } },
            "media_reference": { "OTIO_SCHEMA": "ExternalReference.1", "target_url": "/x.mov" } },
          { "OTIO_SCHEMA": "Clip.1", "name": "missing",
            "source_range": { "OTIO_SCHEMA": "TimeRange.1",
              "start_time": { "OTIO_SCHEMA": "RationalTime.1", "value": 0, "rate": 25 },
              "duration": { "OTIO_SCHEMA": "RationalTime.1", "value": 25, "rate": 25 } },
            "media_reference": { "OTIO_SCHEMA": "MissingReference.1" } },
          { "OTIO_SCHEMA": "Clip.2", "name": "bars",
            "source_range": null,
            "media_references": { "DEFAULT_MEDIA": { "OTIO_SCHEMA": "GeneratorReference.1",
              "generator_kind": "SMPTEBars", "parameters": { "level": 75 },
              "available_range": { "OTIO_SCHEMA": "TimeRange.1",
                "start_time": { "OTIO_SCHEMA": "RationalTime.1", "value": 0, "rate": 25 },
                "duration": { "OTIO_SCHEMA": "RationalTime.1", "value": 100, "rate": 25 } } } },
            "active_media_reference_key": "DEFAULT_MEDIA",
            "markers": [ { "OTIO_SCHEMA": "Marker.1", "name": "m", "color": "BLUE",
              "marked_range": { "OTIO_SCHEMA": "TimeRange.1",
                "start_time": { "OTIO_SCHEMA": "RationalTime.1", "value": 1, "rate": 25 },
                "duration": { "OTIO_SCHEMA": "RationalTime.1", "value": 1, "rate": 25 } } } ] }
        ] } ] } }"#;
    let tl = read_otio(text).expect("legacy parses");
    let v1 = tl.tracks().next().unwrap();
    assert_eq!(v1.children.len(), 3);
    let old = v1.children[0].as_clip().unwrap();
    assert_eq!(old.media_reference.target_url(), Some("/x.mov"));
    assert_eq!(old.available_range(), None);
    assert_eq!(
        v1.children[1].as_clip().unwrap().media_reference,
        MediaReference::Missing
    );
    let bars = v1.children[2].as_clip().unwrap();
    assert!(
        matches!(&bars.media_reference, MediaReference::Generator { generator_kind, .. } if generator_kind == "SMPTEBars")
    );
    assert_eq!(
        bars.trimmed_range(),
        Some(TimeRange::from_frames(0, 100, 25.0))
    );
    assert_eq!(bars.markers[0].color, "BLUE");
    assert_eq!(v1.duration(), RationalTime::from_frames(175, 25.0));
    // writes as Clip.2 and reads back equal
    let back = read_otio(&write_otio(&tl)).unwrap();
    assert_eq!(back, tl);
}
