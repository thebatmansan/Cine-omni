//! OpenTimelineIO JSON (`.otio`) reader and writer.
//!
//! The writer emits exactly the schema OpenTimelineIO 0.15+ produces
//! (`Timeline.1`, `Stack.1`, `Track.1`, `Clip.2` with `media_references`
//! keyed by `DEFAULT_MEDIA`, `Gap.1`, `Transition.1`, `ExternalReference.1`,
//! `MissingReference.1`, `GeneratorReference.1`, `Marker.2`, `RationalTime.1`,
//! `TimeRange.1`). The reader accepts those plus the legacy `Clip.1`
//! (single `media_reference`) and `Marker.1`. Keys are written sorted, like
//! OTIO itself does, so files diff cleanly.

use crate::error::{Error, Result};
use crate::otio::*;
use serde_json::{json, Map, Value as J};
use ucg_core::{RationalTime, TimeRange};

/// Key of the active media reference written by this crate (OTIO's default).
pub use crate::otio::DEFAULT_MEDIA_KEY;

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Parse an OTIO JSON document into a [`Timeline`].
pub fn read_otio(text: &str) -> Result<Timeline> {
    let v: J = serde_json::from_str(text)?;
    read_timeline_value(&v)
}

/// Parse an already-decoded JSON value.
pub fn read_timeline_value(v: &J) -> Result<Timeline> {
    let (schema, obj) = schema_of(v)?;
    match schema {
        "Timeline" => {
            let tracks = match obj.get("tracks") {
                Some(t) if !t.is_null() => read_stack(t)?,
                _ => Stack::new("tracks"),
            };
            Ok(Timeline {
                name: str_field(obj, "name"),
                global_start_time: opt_time(obj.get("global_start_time"))?,
                tracks,
                metadata: metadata(obj.get("metadata")),
            })
        }
        // A bare stack or track is accepted as a headless timeline.
        "Stack" => Ok(Timeline {
            name: str_field(obj, "name"),
            tracks: read_stack(v)?,
            ..Default::default()
        }),
        "Track" => {
            let track = read_track(v)?;
            let mut stack = Stack::new("tracks");
            stack.children.push(Composable::Track(track));
            Ok(Timeline {
                name: String::new(),
                tracks: stack,
                ..Default::default()
            })
        }
        other => Err(Error::Otio(format!(
            "expected Timeline.1 at the root, found `{other}`"
        ))),
    }
}

fn schema_of(v: &J) -> Result<(&str, &Map<String, J>)> {
    let obj = v
        .as_object()
        .ok_or_else(|| Error::Otio("expected a JSON object".into()))?;
    let tag = obj
        .get("OTIO_SCHEMA")
        .and_then(J::as_str)
        .ok_or_else(|| Error::Otio("object has no OTIO_SCHEMA tag".into()))?;
    let name = tag.split('.').next().unwrap_or(tag);
    Ok((name, obj))
}

fn str_field(obj: &Map<String, J>, key: &str) -> String {
    obj.get(key)
        .and_then(J::as_str)
        .unwrap_or_default()
        .to_string()
}

fn bool_field(obj: &Map<String, J>, key: &str, default: bool) -> bool {
    obj.get(key).and_then(J::as_bool).unwrap_or(default)
}

fn metadata(v: Option<&J>) -> Metadata {
    match v {
        Some(J::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => Metadata::new(),
    }
}

/// Parse a `RationalTime.1`.
pub fn read_time(v: &J) -> Result<RationalTime> {
    let (schema, obj) = schema_of(v)?;
    if schema != "RationalTime" {
        return Err(Error::Otio(format!(
            "expected RationalTime.1, found `{schema}`"
        )));
    }
    let value = obj
        .get("value")
        .and_then(J::as_f64)
        .ok_or_else(|| Error::Otio("RationalTime without value".into()))?;
    let rate = obj
        .get("rate")
        .and_then(J::as_f64)
        .ok_or_else(|| Error::Otio("RationalTime without rate".into()))?;
    Ok(RationalTime::new(value, rate))
}

/// Parse a `TimeRange.1`.
pub fn read_range(v: &J) -> Result<TimeRange> {
    let (schema, obj) = schema_of(v)?;
    if schema != "TimeRange" {
        return Err(Error::Otio(format!(
            "expected TimeRange.1, found `{schema}`"
        )));
    }
    let start = read_time(
        obj.get("start_time")
            .ok_or_else(|| Error::Otio("TimeRange without start_time".into()))?,
    )?;
    let duration = read_time(
        obj.get("duration")
            .ok_or_else(|| Error::Otio("TimeRange without duration".into()))?,
    )?;
    Ok(TimeRange {
        start_time: start,
        duration,
    })
}

fn opt_time(v: Option<&J>) -> Result<Option<RationalTime>> {
    match v {
        None | Some(J::Null) => Ok(None),
        Some(t) => read_time(t).map(Some),
    }
}

fn opt_range(v: Option<&J>) -> Result<Option<TimeRange>> {
    match v {
        None | Some(J::Null) => Ok(None),
        Some(r) => read_range(r).map(Some),
    }
}

/// A clip's constant retime, from its `effects` list.
///
/// Two sources, in that order of trust. A file written here carries the exact
/// pair in `metadata.cine`, and a round trip loses nothing. A file from
/// anywhere else carries only OTIO's floating-point `time_scalar`, and the
/// model's retime is exact, so the played length is rounded onto the media's
/// own frame grid **here** rather than leaving a float to be compared against
/// exact time forever afterwards.
///
/// A `FreezeFrame` is a clip that shows one picture for its whole length, so
/// it is read as a retime *and* a source range narrowed to that one picture —
/// which is what a freeze is, spelled in this model's terms.
fn read_retime(
    v: Option<&J>,
    media: Option<TimeRange>,
    source_range: &mut Option<TimeRange>,
) -> Result<Option<Retime>> {
    let Some(J::Array(list)) = v else {
        return Ok(None);
    };
    for effect in list {
        let Ok((schema, obj)) = schema_of(effect) else {
            continue;
        };
        if schema != "LinearTimeWarp" && schema != "FreezeFrame" {
            continue;
        }
        if let Some(exact) = obj.get("metadata").and_then(|m| m.get("cine")) {
            if let Some(played) = exact.get("played") {
                return Ok(Some(Retime {
                    played: read_time(played)?,
                    reversed: exact.get("reversed").and_then(J::as_bool).unwrap_or(false),
                    keep_pitch: exact
                        .get("keep_pitch")
                        .and_then(J::as_bool)
                        .unwrap_or(false),
                }));
            }
        }
        let Some(media) = media else {
            return Err(Error::Otio(
                "a retimed clip with no source or available range: there is no media length to \
                 scale, so the speed cannot be turned into a duration"
                    .into(),
            ));
        };
        if schema == "FreezeFrame" {
            let one = RationalTime::exact(1, media.duration.rate_exact());
            *source_range = Some(TimeRange::new(media.start_time, one));
            return Ok(Some(Retime::new(media.duration, false)));
        }
        let scalar = obj.get("time_scalar").and_then(J::as_f64).unwrap_or(1.0);
        if scalar == 0.0 || !scalar.is_finite() {
            return Err(Error::Otio(format!(
                "a time_scalar of {scalar} is not a speed"
            )));
        }
        let ticks = (f64::from(i32::try_from(media.duration.ticks()).unwrap_or(i32::MAX))
            / scalar.abs())
        .round();
        if !(1.0..=f64::from(i32::MAX)).contains(&ticks) {
            return Err(Error::Otio(format!(
                "a time_scalar of {scalar} over {} frames is not a length this can hold",
                media.duration.ticks()
            )));
        }
        return Ok(Some(Retime::new(
            RationalTime::exact(ticks as i64, media.duration.rate_exact()),
            scalar < 0.0,
        )));
    }
    Ok(None)
}

fn read_markers(v: Option<&J>) -> Result<Vec<Marker>> {
    let Some(J::Array(list)) = v else {
        return Ok(Vec::new());
    };
    list.iter().map(read_marker).collect()
}

/// Parse a `Marker.2` (or `Marker.1`).
pub fn read_marker(v: &J) -> Result<Marker> {
    let (schema, obj) = schema_of(v)?;
    if schema != "Marker" {
        return Err(Error::Otio(format!("expected Marker, found `{schema}`")));
    }
    let marked_range = match obj.get("marked_range") {
        Some(r) if !r.is_null() => read_range(r)?,
        _ => TimeRange::default(),
    };
    Ok(Marker {
        name: str_field(obj, "name"),
        marked_range,
        color: obj
            .get("color")
            .and_then(J::as_str)
            .unwrap_or("GREEN")
            .to_string(),
        comment: str_field(obj, "comment"),
        metadata: metadata(obj.get("metadata")),
    })
}

/// Parse a `Stack.1`.
pub fn read_stack(v: &J) -> Result<Stack> {
    let (schema, obj) = schema_of(v)?;
    if schema != "Stack" {
        return Err(Error::Otio(format!("expected Stack.1, found `{schema}`")));
    }
    let mut children = Vec::new();
    if let Some(J::Array(list)) = obj.get("children") {
        for c in list {
            let (s, _) = schema_of(c)?;
            match s {
                "Track" => children.push(Composable::Track(read_track(c)?)),
                "Stack" => children.push(Composable::Stack(read_stack(c)?)),
                other => return Err(Error::Otio(format!("unsupported stack child `{other}`"))),
            }
        }
    }
    Ok(Stack {
        name: str_field(obj, "name"),
        children,
        markers: read_markers(obj.get("markers"))?,
        metadata: metadata(obj.get("metadata")),
    })
}

/// Parse a `Track.1`.
pub fn read_track(v: &J) -> Result<Track> {
    let (schema, obj) = schema_of(v)?;
    if schema != "Track" {
        return Err(Error::Otio(format!("expected Track.1, found `{schema}`")));
    }
    let kind_str = obj.get("kind").and_then(J::as_str).unwrap_or("Video");
    let kind = TrackKind::from_otio(kind_str)
        .ok_or_else(|| Error::Otio(format!("unknown track kind `{kind_str}`")))?;
    let mut children = Vec::new();
    if let Some(J::Array(list)) = obj.get("children") {
        for c in list {
            children.push(read_item(c)?);
        }
    }
    Ok(Track {
        name: str_field(obj, "name"),
        kind,
        children,
        markers: read_markers(obj.get("markers"))?,
        enabled: bool_field(obj, "enabled", true),
        metadata: metadata(obj.get("metadata")),
    })
}

/// Every media reference a clip carries, keyed as OTIO expects.
fn media_references_value(c: &Clip) -> J {
    let mut map = serde_json::Map::new();
    for (key, r) in c.all_media_references() {
        map.insert(key, media_reference_value(&r));
    }
    J::Object(map)
}

/// Parse a `Clip.2` / `Clip.1` / `Gap.1` / `Transition.1`.
pub fn read_item(v: &J) -> Result<Item> {
    let (schema, obj) = schema_of(v)?;
    match schema {
        "Clip" => {
            let mut active_key = DEFAULT_MEDIA_KEY.to_string();
            let mut alternates: std::collections::BTreeMap<String, MediaReference> =
                Default::default();
            let media_reference =
                if let Some(refs) = obj.get("media_references").and_then(J::as_object) {
                    // Clip.2 carries a map of references with one marked active.
                    // All of them are kept: discarding the camera original
                    // because the editor is cutting on proxies is data loss, and
                    // relabelling the proxy `DEFAULT_MEDIA` would assert the
                    // proxy is the original.
                    let declared = obj
                        .get("active_media_reference_key")
                        .and_then(J::as_str)
                        .unwrap_or(DEFAULT_MEDIA_KEY);
                    // Fall back to the first entry only when the declared key is
                    // genuinely absent, and record the key actually used.
                    let chosen = if refs.contains_key(declared) {
                        declared.to_string()
                    } else {
                        refs.keys()
                            .next()
                            .cloned()
                            .unwrap_or_else(|| declared.to_string())
                    };
                    for (k, r) in refs {
                        if k != &chosen && !r.is_null() {
                            alternates.insert(k.clone(), read_media_reference(r)?);
                        }
                    }
                    active_key = chosen.clone();
                    match refs.get(&chosen) {
                        Some(r) if !r.is_null() => read_media_reference(r)?,
                        _ => MediaReference::Missing,
                    }
                } else {
                    // Clip.1 has a single reference and no key.
                    match obj.get("media_reference") {
                        Some(r) if !r.is_null() => read_media_reference(r)?,
                        _ => MediaReference::Missing,
                    }
                };
            let mut source_range = opt_range(obj.get("source_range"))?;
            let available = media_reference.available_range();
            let retime = read_retime(
                obj.get("effects"),
                source_range.or(available),
                &mut source_range,
            )?;
            Ok(Item::Clip(Clip {
                name: str_field(obj, "name"),
                media_reference,
                active_media_reference_key: active_key,
                alternate_media_references: alternates,
                source_range,
                retime,
                markers: read_markers(obj.get("markers"))?,
                metadata: metadata(obj.get("metadata")),
                enabled: bool_field(obj, "enabled", true),
            }))
        }
        "Gap" => {
            let source_range = opt_range(obj.get("source_range"))?.unwrap_or_default();
            Ok(Item::Gap(Gap {
                source_range,
                name: str_field(obj, "name"),
                metadata: metadata(obj.get("metadata")),
            }))
        }
        "Transition" => {
            let zero = RationalTime::new(0.0, 24.0);
            Ok(Item::Transition(Transition {
                name: str_field(obj, "name"),
                transition_type: TransitionType::from_otio(
                    obj.get("transition_type")
                        .and_then(J::as_str)
                        .unwrap_or("SMPTE_Dissolve"),
                ),
                in_offset: opt_time(obj.get("in_offset"))?.unwrap_or(zero),
                out_offset: opt_time(obj.get("out_offset"))?.unwrap_or(zero),
                metadata: metadata(obj.get("metadata")),
            }))
        }
        other => Err(Error::Otio(format!("unsupported track child `{other}`"))),
    }
}

/// Parse an `ExternalReference.1` / `MissingReference.1` / `GeneratorReference.1`.
pub fn read_media_reference(v: &J) -> Result<MediaReference> {
    let (schema, obj) = schema_of(v)?;
    match schema {
        "ExternalReference" => Ok(MediaReference::External {
            target_url: str_field(obj, "target_url"),
            available_range: opt_range(obj.get("available_range"))?,
        }),
        "MissingReference" => Ok(MediaReference::Missing),
        "GeneratorReference" => Ok(MediaReference::Generator {
            generator_kind: str_field(obj, "generator_kind"),
            parameters: metadata(obj.get("parameters")),
            available_range: opt_range(obj.get("available_range"))?,
        }),
        other => Err(Error::Otio(format!(
            "unsupported media reference `{other}`"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Serialize a timeline as pretty-printed OTIO JSON.
pub fn write_otio(timeline: &Timeline) -> String {
    let v = timeline_value(timeline);
    serde_json::to_string_pretty(&v).expect("a serde_json::Value always serializes")
}

/// Build the JSON value of a timeline (`Timeline.1`).
pub fn timeline_value(t: &Timeline) -> J {
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "metadata": metadata_value(&t.metadata),
        "name": t.name,
        "global_start_time": t.global_start_time.map(time_value),
        "tracks": stack_value(&t.tracks),
    })
}

/// A clip's effects list. Only a constant retime is modelled, and it is
/// written twice on purpose: OTIO's own `time_scalar` so that another tool
/// sees the speed, and the exact pair under `metadata.cine` so that a round
/// trip through this writer is lossless.
fn effects_value(c: &Clip) -> J {
    let Some(retime) = c.retime else {
        return json!([]);
    };
    let (num, den) = retime.speed_of(c.media_duration());
    let scalar = num as f64 / den as f64;
    json!([{
        "OTIO_SCHEMA": "LinearTimeWarp.1",
        "name": "retime",
        "effect_name": "LinearTimeWarp",
        "metadata": {
            "cine": {
                "played": time_value(retime.played),
                "reversed": retime.reversed,
                "keep_pitch": retime.keep_pitch,
            }
        },
        "time_scalar": if retime.reversed { -scalar } else { scalar },
    }])
}

fn metadata_value(m: &Metadata) -> J {
    J::Object(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// `RationalTime.1`.
pub fn time_value(t: RationalTime) -> J {
    json!({ "OTIO_SCHEMA": "RationalTime.1", "rate": t.rate_hz(), "value": t.value() })
}

/// `TimeRange.1`.
pub fn range_value(r: TimeRange) -> J {
    json!({ "OTIO_SCHEMA": "TimeRange.1", "duration": time_value(r.duration), "start_time": time_value(r.start_time) })
}

fn opt_range_value(r: Option<TimeRange>) -> J {
    r.map(range_value).unwrap_or(J::Null)
}

/// `Marker.2`.
pub fn marker_value(m: &Marker) -> J {
    json!({
        "OTIO_SCHEMA": "Marker.2",
        "metadata": metadata_value(&m.metadata),
        "name": m.name,
        "color": m.color,
        "marked_range": range_value(m.marked_range),
        "comment": m.comment,
    })
}

fn markers_value(ms: &[Marker]) -> J {
    J::Array(ms.iter().map(marker_value).collect())
}

/// `Stack.1`.
pub fn stack_value(s: &Stack) -> J {
    let children: Vec<J> = s
        .children
        .iter()
        .map(|c| match c {
            Composable::Track(t) => track_value(t),
            Composable::Stack(s) => stack_value(s),
        })
        .collect();
    json!({
        "OTIO_SCHEMA": "Stack.1",
        "metadata": metadata_value(&s.metadata),
        "name": s.name,
        "source_range": J::Null,
        "effects": [],
        "markers": markers_value(&s.markers),
        "enabled": true,
        "children": children,
    })
}

/// `Track.1`.
pub fn track_value(t: &Track) -> J {
    json!({
        "OTIO_SCHEMA": "Track.1",
        "metadata": metadata_value(&t.metadata),
        "name": t.name,
        "source_range": J::Null,
        "effects": [],
        "markers": markers_value(&t.markers),
        "enabled": t.enabled,
        "children": t.children.iter().map(item_value).collect::<Vec<_>>(),
        "kind": t.kind.as_otio(),
    })
}

/// `Clip.2` / `Gap.1` / `Transition.1`.
pub fn item_value(item: &Item) -> J {
    match item {
        Item::Clip(c) => json!({
            "OTIO_SCHEMA": "Clip.2",
            "metadata": metadata_value(&c.metadata),
            "name": c.name,
            "source_range": opt_range_value(c.source_range),
            "effects": effects_value(c),
            "markers": markers_value(&c.markers),
            "enabled": c.enabled,
            "media_references": media_references_value(c),
            "active_media_reference_key": c.active_media_reference_key,
        }),
        Item::Gap(g) => json!({
            "OTIO_SCHEMA": "Gap.1",
            "metadata": metadata_value(&g.metadata),
            "name": g.name,
            "source_range": range_value(g.source_range),
            "effects": [],
            "markers": [],
            "enabled": true,
        }),
        Item::Transition(t) => json!({
            "OTIO_SCHEMA": "Transition.1",
            "metadata": metadata_value(&t.metadata),
            "name": t.name,
            "transition_type": t.transition_type.as_otio(),
            "in_offset": time_value(t.in_offset),
            "out_offset": time_value(t.out_offset),
        }),
    }
}

/// `ExternalReference.1` / `MissingReference.1` / `GeneratorReference.1`.
pub fn media_reference_value(m: &MediaReference) -> J {
    match m {
        MediaReference::External {
            target_url,
            available_range,
        } => json!({
            "OTIO_SCHEMA": "ExternalReference.1",
            "metadata": {},
            "name": "",
            "available_range": opt_range_value(*available_range),
            "available_image_bounds": J::Null,
            "target_url": target_url,
        }),
        MediaReference::Missing => json!({
            "OTIO_SCHEMA": "MissingReference.1",
            "metadata": {},
            "name": "",
            "available_range": J::Null,
            "available_image_bounds": J::Null,
        }),
        MediaReference::Generator {
            generator_kind,
            parameters,
            available_range,
        } => json!({
            "OTIO_SCHEMA": "GeneratorReference.1",
            "metadata": {},
            "name": "",
            "available_range": opt_range_value(*available_range),
            "available_image_bounds": J::Null,
            "generator_kind": generator_kind,
            "parameters": metadata_value(parameters),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_and_range_round_trip() {
        let t = RationalTime::new(1001.0, 29.97);
        assert_eq!(read_time(&time_value(t)).unwrap(), t);
        let r = TimeRange::from_frames(10, 20, 24.0);
        assert_eq!(read_range(&range_value(r)).unwrap(), r);
        assert!(read_time(&json!({"OTIO_SCHEMA": "TimeRange.1"})).is_err());
    }

    #[test]
    fn clip1_legacy_is_accepted() {
        let text = r#"{
            "OTIO_SCHEMA": "Clip.1",
            "name": "old",
            "source_range": {"OTIO_SCHEMA": "TimeRange.1",
                "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 5, "rate": 24},
                "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": 10, "rate": 24}},
            "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///a.mov",
                "available_range": null},
            "metadata": {}, "markers": [], "effects": []
        }"#;
        let item = read_item(&serde_json::from_str(text).unwrap()).unwrap();
        let clip = item.as_clip().unwrap();
        assert_eq!(clip.name, "old");
        assert_eq!(clip.media_reference.target_url(), Some("file:///a.mov"));
        assert_eq!(clip.source_range, Some(TimeRange::from_frames(5, 10, 24.0)));
    }

    #[test]
    fn unknown_schema_is_an_error() {
        assert!(read_otio(r#"{"OTIO_SCHEMA": "Effect.1"}"#).is_err());
        assert!(read_otio("not json").is_err());
        assert!(
            read_otio(r#"{"OTIO_SCHEMA":"Track.1","children":[{"OTIO_SCHEMA":"Nope.1"}]}"#)
                .is_err()
        );
    }

    #[test]
    fn writer_emits_clip2_shape() {
        let mut tl = Timeline::new("x");
        let clip = Clip::external(
            "c",
            "file:///c.mov",
            Some(TimeRange::from_frames(0, 10, 24.0)),
            None,
        );
        tl.tracks.children.push(Composable::Track(
            Track::new("V1", TrackKind::Video).with(clip),
        ));
        let text = write_otio(&tl);
        assert!(text.contains("\"Clip.2\""));
        assert!(text.contains("\"DEFAULT_MEDIA\""));
        assert!(text.contains("\"active_media_reference_key\": \"DEFAULT_MEDIA\""));
        assert_eq!(read_otio(&text).unwrap(), tl);
    }
}
