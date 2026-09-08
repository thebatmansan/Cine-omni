//! A proxy must never be able to masquerade as the camera original.
//!
//! OTIO lets a clip carry several media references — a camera original, a
//! proxy, a transcode — with exactly one marked active. An editor cutting on
//! proxies has `active_media_reference_key = "proxy"`.
//!
//! Collapsing that map to a single reference on read, and writing it back under
//! `DEFAULT_MEDIA`, corrupts the file in two ways at once: the camera original
//! is silently discarded, and the proxy is relabelled as though it *were* the
//! original. A conform performed against such a file would master from proxies.
//!
//! These tests pin the invariant rather than the implementation.

use cine_timeline::otio::{Clip, Composable, Item, MediaReference, Timeline, Track, TrackKind};
use cine_timeline::otio_json::{read_otio, write_otio};
use ucg_core::TimeRange;

/// The first clip of the first track.
fn first_clip(t: &Timeline) -> Clip {
    let track = t.tracks().next().expect("a track");
    match track.children.first().expect("an item") {
        Item::Clip(c) => c.clone(),
        other => panic!("expected a clip, got {other:?}"),
    }
}

const ORIGINAL: &str = "file:///media/A001/A001C003_240101_R1AB.mov";
const PROXY: &str = "file:///proxies/A001C003.mov";

/// A clip carrying both an original and a proxy, cutting on the proxy.
fn proxy_clip() -> Clip {
    let mut clip = Clip::new(
        "A001C003",
        MediaReference::external(PROXY, Some(TimeRange::from_frames(0, 240, 24.0))),
        Some(TimeRange::from_frames(0, 100, 24.0)),
    );
    clip.active_media_reference_key = "proxy".to_string();
    clip.alternate_media_references.insert(
        "DEFAULT_MEDIA".to_string(),
        MediaReference::external(ORIGINAL, Some(TimeRange::from_frames(0, 240, 24.0))),
    );
    clip
}

fn round_trip(clip: Clip) -> Clip {
    let mut timeline = Timeline::new("conform test");
    timeline.tracks.children.push(Composable::Track(
        Track::new("V1", TrackKind::Video).with(Item::Clip(clip)),
    ));
    let json = write_otio(&timeline);
    let back = read_otio(&json).expect("read");
    first_clip(&back)
}

#[test]
fn a_proxy_survives_a_round_trip_still_labelled_a_proxy() {
    let after = round_trip(proxy_clip());

    assert_eq!(
        after.active_media_reference_key, "proxy",
        "the active key was rewritten; the file now claims the proxy is the camera original"
    );
    match &after.media_reference {
        MediaReference::External { target_url, .. } => assert_eq!(target_url, PROXY),
        other => panic!("active reference lost: {other:?}"),
    }
}

#[test]
fn the_camera_original_is_not_discarded_when_cutting_on_proxies() {
    let after = round_trip(proxy_clip());

    let original = after
        .media_reference_named("DEFAULT_MEDIA")
        .expect("the camera original must survive a round trip");
    match original {
        MediaReference::External { target_url, .. } => assert_eq!(target_url, ORIGINAL),
        other => panic!("camera original corrupted: {other:?}"),
    }
    assert_eq!(
        after.all_media_references().len(),
        2,
        "both references must survive"
    );
}

#[test]
fn conforming_from_proxy_to_original_swaps_without_losing_either() {
    let mut clip = round_trip(proxy_clip());

    assert!(
        clip.set_active_media_reference("DEFAULT_MEDIA"),
        "conform must succeed"
    );
    assert_eq!(clip.active_media_reference_key, "DEFAULT_MEDIA");
    match &clip.media_reference {
        MediaReference::External { target_url, .. } => assert_eq!(target_url, ORIGINAL),
        other => panic!("conform did not switch to the original: {other:?}"),
    }
    // The proxy is retained, so the editor can go back to it.
    match clip.media_reference_named("proxy").expect("proxy retained") {
        MediaReference::External { target_url, .. } => assert_eq!(target_url, PROXY),
        other => panic!("proxy lost during conform: {other:?}"),
    }
    assert_eq!(clip.all_media_references().len(), 2);

    // And it survives being written out again.
    let after = round_trip(clip);
    assert_eq!(after.active_media_reference_key, "DEFAULT_MEDIA");
    assert_eq!(after.all_media_references().len(), 2);
}

#[test]
fn conforming_to_an_unknown_reference_fails_without_damaging_the_clip() {
    let mut clip = proxy_clip();
    let before = clip.clone();

    assert!(
        !clip.set_active_media_reference("online_hires"),
        "a conform to a reference the clip does not have must fail"
    );
    assert_eq!(
        clip, before,
        "a failed conform must leave the clip untouched"
    );
}

/// The ordinary single-reference case must keep working unchanged.
#[test]
fn a_plain_single_reference_clip_still_round_trips() {
    let clip = Clip::new(
        "simple",
        MediaReference::external(ORIGINAL, None),
        Some(TimeRange::from_frames(0, 50, 24.0)),
    );
    let after = round_trip(clip);
    assert_eq!(after.active_media_reference_key, "DEFAULT_MEDIA");
    assert!(after.alternate_media_references.is_empty());
    assert_eq!(after.all_media_references().len(), 1);
}

/// Real OTIO files in the wild declare a key that is not present in the map.
/// Falling back is correct; lying about which key was used is not.
#[test]
fn a_declared_key_that_is_absent_falls_back_and_reports_the_key_it_used() {
    let json = r#"{
      "OTIO_SCHEMA": "Timeline.1",
      "name": "odd",
      "global_start_time": null,
      "metadata": {},
      "tracks": {
        "OTIO_SCHEMA": "Stack.1", "name": "tracks", "metadata": {},
        "source_range": null, "markers": [], "effects": [], "enabled": true,
        "children": [{
          "OTIO_SCHEMA": "Track.1", "name": "V1", "kind": "Video", "metadata": {},
          "source_range": null, "markers": [], "effects": [], "enabled": true,
          "children": [{
            "OTIO_SCHEMA": "Clip.2", "name": "c", "metadata": {},
            "source_range": null, "markers": [], "effects": [], "enabled": true,
            "active_media_reference_key": "nonexistent",
            "media_references": {
              "only_one": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "target_url": "file:///media/only.mov",
                "available_range": null, "metadata": {}, "name": ""
              }
            }
          }]
        }]
      }
    }"#;
    let timeline = read_otio(json).expect("read");
    let clip = first_clip(&timeline);
    assert_eq!(
        clip.active_media_reference_key, "only_one",
        "must report the key actually used, not the absent one it was told to use"
    );
    match &clip.media_reference {
        MediaReference::External { target_url, .. } => {
            assert_eq!(target_url, "file:///media/only.mov")
        }
        other => panic!("fallback lost the reference: {other:?}"),
    }
}
