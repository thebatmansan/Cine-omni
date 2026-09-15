//! Import/export between the OTIO model and the Unified Context Graph, plus
//! the clip-level queries and mutations the pipeline reactors call.
//!
//! Graph shape produced by [`import_timeline`]:
//!
//! ```text
//! project ─Contains→ nle.timeline (id = from_key("nle:<source_key>"))
//!   └─Contains→ nle.stack
//!        ├─Contains→ nle.track (order 0)  name, track_kind, order
//!        │    ├─Contains→ nle.clip  (order, name, range, clip_ref, source_range)
//!        │    │    ├─Binds→ asset.media (id = from_key("media:<url>"))
//!        │    │    └─Contains→ nle.marker
//!        │    ├─Contains→ nle.gap / nle.transition
//!        │    └─Contains→ nle.marker (track markers)
//!        └─Contains→ nle.track (order 1) ...
//! ```
//!
//! Item ids are `from_key("nle:<source_key>:<track>:<item>")` unless the
//! item's OTIO metadata carries a `ucg_id`; [`export_timeline`] writes each
//! item's id into its metadata, so ids survive a round trip through external
//! OTIO tools. Re-importing an unchanged timeline produces zero ops.

use crate::edit::{self, Edge};
use crate::error::{Error, Result};
use crate::otio::*;
use crate::otio_json;
use serde_json::Value as J;
use std::collections::HashSet;
use ucg_core::{EdgeKind, EntityId, Graph, Tx, Value};
use ucg_core::{RationalTime, TimeRange};
use ucg_schema::{attrs, kinds, ClipRef, Component};

/// Module-private attribute keys (`nle.*`).
pub mod nle_attrs {
    pub const ENABLED: &str = "nle.enabled";
    pub const GLOBAL_START_TIME: &str = "nle.global_start_time";
    pub const TRANSITION_TYPE: &str = "nle.transition_type";
    pub const IN_OFFSET: &str = "nle.in_offset";
    pub const OUT_OFFSET: &str = "nle.out_offset";
    /// OTIO JSON of a non-external media reference (missing / generator).
    pub const MEDIA_REF: &str = "nle.media_ref";
    pub const MARKER_COLOR: &str = "nle.color";
    pub const MARKER_COMMENT: &str = "nle.comment";
}

/// An edit applied to a track node by [`apply_edit`]. See [`crate::edit`]
/// for the semantics of each operation.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    RippleTrim {
        index: usize,
        edge: Edge,
        delta: RationalTime,
    },
    Roll {
        cut: usize,
        delta: RationalTime,
    },
    Slip {
        index: usize,
        delta: RationalTime,
    },
    Slide {
        index: usize,
        delta: RationalTime,
    },
    SplitAt {
        time: RationalTime,
    },
    Extract {
        range: TimeRange,
    },
    Lift {
        range: TimeRange,
    },
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

fn metadata_to_value(m: &Metadata) -> Value {
    let obj: serde_json::Map<String, J> = m
        .iter()
        .filter(|(k, _)| k.as_str() != UCG_ID_KEY)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Value::from_json(J::Object(obj))
}

fn value_to_metadata(v: Option<&Value>) -> Metadata {
    match v.map(Value::to_json) {
        Some(J::Object(o)) => o.into_iter().collect(),
        _ => Metadata::new(),
    }
}

fn set_opt<T: Into<Value>>(tx: &mut Tx, id: EntityId, key: &str, v: Option<T>) -> Result<()> {
    match v {
        Some(v) => tx.set(id, key, v)?,
        None => tx.remove_attr(id, key)?,
    }
    Ok(())
}

/// Make sure `id` exists with `kind`, recreating it if it exists as another kind.
fn ensure_kind(tx: &mut Tx, id: EntityId, kind: &str) -> Result<()> {
    if let Some(n) = tx.graph().node(id) {
        if n.kind.as_str() == kind {
            return Ok(());
        }
        tx.delete(id)?;
    }
    tx.create_with_id(id, kind)?;
    Ok(())
}

fn require_kind(graph: &Graph, id: EntityId, kind: &'static str) -> Result<()> {
    let node = graph.require(id)?;
    if node.kind.as_str() != kind {
        return Err(Error::WrongKind {
            id,
            expected: kind,
            actual: node.kind.to_string(),
        });
    }
    Ok(())
}

/// Delete every child of `parent` that is not in `keep`.
fn sync_children(tx: &mut Tx, parent: EntityId, keep: &[EntityId]) -> Result<()> {
    let keep: HashSet<EntityId> = keep.iter().copied().collect();
    let stale: Vec<EntityId> = tx
        .graph()
        .children(parent)
        .into_iter()
        .filter(|c| !keep.contains(c))
        .collect();
    for id in stale {
        tx.delete(id)?;
    }
    Ok(())
}

fn track_kind_str(k: TrackKind) -> &'static str {
    match k {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    }
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

/// Import (or idempotently re-import) a timeline under `project`.
///
/// Returns the timeline entity `from_key("nle:<source_key>")`.
pub fn import_timeline(
    tx: &mut Tx,
    project: EntityId,
    timeline: &Timeline,
    source_key: &str,
) -> Result<EntityId> {
    let prefix = format!("nle:{source_key}");
    let tl_id = EntityId::from_key(&prefix);
    ensure_kind(tx, tl_id, kinds::NLE_TIMELINE)?;
    tx.link(project, tl_id, EdgeKind::Contains)?;
    tx.set(tl_id, attrs::NAME, timeline.name.as_str())?;
    tx.set(tl_id, attrs::SOURCE_KEY, source_key)?;
    set_opt(
        tx,
        tl_id,
        nle_attrs::GLOBAL_START_TIME,
        timeline.global_start_time,
    )?;
    tx.set(
        tl_id,
        attrs::METADATA,
        metadata_to_value(&timeline.metadata),
    )?;

    let stack_id = EntityId::derived(tl_id, "stack");
    write_stack(tx, tl_id, stack_id, &timeline.tracks, 0, &prefix)?;
    sync_children(tx, tl_id, &[stack_id])?;
    Ok(tl_id)
}

fn write_stack(
    tx: &mut Tx,
    parent: EntityId,
    stack_id: EntityId,
    stack: &Stack,
    order: usize,
    prefix: &str,
) -> Result<()> {
    ensure_kind(tx, stack_id, kinds::NLE_STACK)?;
    tx.link(parent, stack_id, EdgeKind::Contains)?;
    tx.set(stack_id, attrs::NAME, stack.name.as_str())?;
    tx.set(stack_id, attrs::ORDER, order)?;
    tx.set(
        stack_id,
        attrs::METADATA,
        metadata_to_value(&stack.metadata),
    )?;
    let mut keep = Vec::new();
    for (i, child) in stack.children.iter().enumerate() {
        match child {
            Composable::Track(track) => {
                let id = track
                    .metadata
                    .get(UCG_ID_KEY)
                    .and_then(J::as_str)
                    .and_then(EntityId::parse)
                    .unwrap_or_else(|| EntityId::from_key(&format!("{prefix}:track:{i}")));
                import_track(tx, stack_id, id, track, i, &format!("{prefix}:{i}"))?;
                keep.push(id);
            }
            Composable::Stack(nested) => {
                let id = EntityId::from_key(&format!("{prefix}:stack:{i}"));
                write_stack(tx, stack_id, id, nested, i, &format!("{prefix}:{i}"))?;
                keep.push(id);
            }
        }
    }
    keep.extend(write_markers(tx, stack_id, &stack.markers)?);
    sync_children(tx, stack_id, &keep)
}

fn import_track(
    tx: &mut Tx,
    parent: EntityId,
    track_id: EntityId,
    track: &Track,
    order: usize,
    prefix: &str,
) -> Result<()> {
    ensure_kind(tx, track_id, kinds::NLE_TRACK)?;
    tx.link(parent, track_id, EdgeKind::Contains)?;
    tx.set(track_id, attrs::NAME, track.name.as_str())?;
    tx.set(track_id, attrs::TRACK_KIND, track_kind_str(track.kind))?;
    tx.set(track_id, attrs::ORDER, order)?;
    tx.set(track_id, nle_attrs::ENABLED, track.enabled)?;
    tx.set(
        track_id,
        attrs::METADATA,
        metadata_to_value(&track.metadata),
    )?;
    let ids = assign_item_ids(tx.graph(), track_id, track, prefix, false);
    write_track_items(tx, track_id, track, &ids)
}

/// Choose an id for every item of `track`.
///
/// Items carrying a `ucg_id` keep it (first claimant wins). Others get the
/// position key `<prefix>:<index>`; when `reuse_by_position` is set and the
/// node currently at that position has the same kind and is unclaimed, its
/// id is reused instead (so edits keep ids for items that merely shifted).
fn assign_item_ids(
    graph: &Graph,
    track_id: EntityId,
    track: &Track,
    prefix: &str,
    reuse_by_position: bool,
) -> Vec<EntityId> {
    let existing: Vec<EntityId> = graph
        .children(track_id)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .map(|n| n.kind.as_str() != kinds::NLE_MARKER)
                .unwrap_or(false)
        })
        .collect();
    let mut claimed: HashSet<EntityId> = HashSet::new();
    let mut ids: Vec<Option<EntityId>> = vec![None; track.children.len()];
    for (i, item) in track.children.iter().enumerate() {
        if let Some(id) = item.ucg_id() {
            if claimed.insert(id) {
                ids[i] = Some(id);
            }
        }
    }
    for (i, item) in track.children.iter().enumerate() {
        if ids[i].is_some() {
            continue;
        }
        let kind = item_kind(item);
        let by_position = if reuse_by_position {
            existing.get(i).copied().filter(|id| {
                !claimed.contains(id)
                    && graph
                        .node(*id)
                        .map(|n| n.kind.as_str() == kind)
                        .unwrap_or(false)
            })
        } else {
            None
        };
        let mut id = by_position.unwrap_or_else(|| EntityId::from_key(&format!("{prefix}:{i}")));
        let mut salt = 0u32;
        while claimed.contains(&id) {
            salt += 1;
            id = EntityId::derived(id, &format!("dup:{salt}"));
        }
        claimed.insert(id);
        ids[i] = Some(id);
    }
    ids.into_iter()
        .map(|id| id.unwrap_or_else(EntityId::new))
        .collect()
}

fn item_kind(item: &Item) -> &'static str {
    match item {
        Item::Clip(_) => kinds::NLE_CLIP,
        Item::Gap(_) => kinds::NLE_GAP,
        Item::Transition(_) => kinds::NLE_TRANSITION,
    }
}

/// Write the items and markers of `track` under `track_id`, deleting
/// children that are no longer present. Unchanged attributes produce no ops.
fn write_track_items(
    tx: &mut Tx,
    track_id: EntityId,
    track: &Track,
    ids: &[EntityId],
) -> Result<()> {
    let mut keep = Vec::with_capacity(track.children.len() + track.markers.len());
    for (i, item) in track.children.iter().enumerate() {
        let id = ids[i];
        let range = track.range_of_child(i).unwrap_or_default();
        write_item(tx, track_id, id, item, i, range)?;
        keep.push(id);
    }
    keep.extend(write_markers(tx, track_id, &track.markers)?);
    sync_children(tx, track_id, &keep)
}

fn write_item(
    tx: &mut Tx,
    track_id: EntityId,
    id: EntityId,
    item: &Item,
    order: usize,
    range: TimeRange,
) -> Result<()> {
    ensure_kind(tx, id, item_kind(item))?;
    tx.link(track_id, id, EdgeKind::Contains)?;
    tx.set(id, attrs::ORDER, order)?;
    tx.set(id, attrs::NAME, item.name())?;
    tx.set(id, attrs::RANGE, range)?;
    tx.set(id, attrs::METADATA, metadata_to_value(item.metadata()))?;
    match item {
        Item::Clip(clip) => {
            tx.set(id, nle_attrs::ENABLED, clip.enabled)?;
            set_opt(tx, id, attrs::SOURCE_RANGE, clip.source_range)?;
            let media = write_media_reference(tx, id, &clip.media_reference)?;
            let clip_ref = ClipRef {
                media,
                source_range: clip.trimmed_range().unwrap_or_default(),
                track_start: range.start_time,
            };
            clip_ref.write(tx, id)?;
            let markers = write_markers(tx, id, &clip.markers)?;
            sync_children(tx, id, &markers)?;
        }
        Item::Gap(gap) => {
            tx.set(id, attrs::SOURCE_RANGE, gap.source_range)?;
            let clip_ref = ClipRef {
                media: None,
                source_range: gap.source_range,
                track_start: range.start_time,
            };
            clip_ref.write(tx, id)?;
        }
        Item::Transition(t) => {
            tx.set(id, nle_attrs::TRANSITION_TYPE, t.transition_type.as_otio())?;
            tx.set(id, nle_attrs::IN_OFFSET, t.in_offset)?;
            tx.set(id, nle_attrs::OUT_OFFSET, t.out_offset)?;
        }
    }
    Ok(())
}

/// Bind `clip` to the media asset for `reference`; returns the media entity
/// for external references.
fn write_media_reference(
    tx: &mut Tx,
    clip: EntityId,
    reference: &MediaReference,
) -> Result<Option<EntityId>> {
    let media_id = match reference {
        MediaReference::External {
            target_url,
            available_range,
        } => {
            let media = EntityId::from_key(&format!("media:{target_url}"));
            ensure_kind(tx, media, kinds::ASSET_MEDIA)?;
            tx.set(media, attrs::MEDIA_PATH, target_url.as_str())?;
            tx.set(
                media,
                attrs::NAME,
                target_url.rsplit('/').next().unwrap_or(target_url),
            )?;
            set_opt(tx, media, attrs::AVAILABLE_RANGE, *available_range)?;
            tx.remove_attr(clip, nle_attrs::MEDIA_REF)?;
            Some(media)
        }
        other => {
            tx.set(
                clip,
                nle_attrs::MEDIA_REF,
                Value::from_json(otio_json::media_reference_value(other)),
            )?;
            None
        }
    };
    // Exactly one Binds edge to an asset.media.
    let bound: Vec<EntityId> = tx
        .graph()
        .targets(clip, EdgeKind::Binds)
        .into_iter()
        .filter(|m| {
            tx.graph()
                .node(*m)
                .map(|n| n.kind.as_str() == kinds::ASSET_MEDIA)
                .unwrap_or(false)
        })
        .collect();
    for m in bound {
        if Some(m) != media_id {
            tx.unlink(clip, m, EdgeKind::Binds)?;
        }
    }
    if let Some(m) = media_id {
        tx.link(clip, m, EdgeKind::Binds)?;
    }
    Ok(media_id)
}

fn write_markers(tx: &mut Tx, owner: EntityId, markers: &[Marker]) -> Result<Vec<EntityId>> {
    let mut ids = Vec::with_capacity(markers.len());
    for (i, m) in markers.iter().enumerate() {
        let id = EntityId::derived(owner, &format!("marker:{i}"));
        ensure_kind(tx, id, kinds::NLE_MARKER)?;
        tx.link(owner, id, EdgeKind::Contains)?;
        tx.set(id, attrs::ORDER, i)?;
        tx.set(id, attrs::NAME, m.name.as_str())?;
        tx.set(id, attrs::RANGE, m.marked_range)?;
        tx.set(id, nle_attrs::MARKER_COLOR, m.color.as_str())?;
        tx.set(id, nle_attrs::MARKER_COMMENT, m.comment.as_str())?;
        tx.set(id, attrs::METADATA, metadata_to_value(&m.metadata))?;
        ids.push(id);
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

/// Rebuild a [`Timeline`] from the graph. Every item's metadata gets its
/// `ucg_id` so a later [`import_timeline`] maps it to the same entity.
pub fn export_timeline(graph: &Graph, timeline: EntityId) -> Result<Timeline> {
    require_kind(graph, timeline, kinds::NLE_TIMELINE)?;
    let node = graph.require(timeline)?;
    let stack_id = graph.children(timeline).into_iter().find(|c| {
        graph
            .node(*c)
            .map(|n| n.kind.as_str() == kinds::NLE_STACK)
            .unwrap_or(false)
    });
    let tracks = match stack_id {
        Some(s) => export_stack(graph, s)?,
        None => Stack::new("tracks"),
    };
    Ok(Timeline {
        name: node.name(),
        global_start_time: node
            .get(nle_attrs::GLOBAL_START_TIME)
            .and_then(Value::as_time),
        tracks,
        metadata: value_to_metadata(node.get(attrs::METADATA)),
    })
}

/// Rebuild a [`Stack`] from an `nle.stack` node.
pub fn export_stack(graph: &Graph, stack: EntityId) -> Result<Stack> {
    require_kind(graph, stack, kinds::NLE_STACK)?;
    let node = graph.require(stack)?;
    let mut out = Stack {
        name: node.get_str(attrs::NAME).unwrap_or_default().to_string(),
        children: Vec::new(),
        markers: Vec::new(),
        metadata: value_to_metadata(node.get(attrs::METADATA)),
    };
    for c in graph.children(stack) {
        let kind = graph.require(c)?.kind.as_str();
        match kind {
            kinds::NLE_TRACK => out
                .children
                .push(Composable::Track(export_track(graph, c)?)),
            kinds::NLE_STACK => out
                .children
                .push(Composable::Stack(export_stack(graph, c)?)),
            kinds::NLE_MARKER => out.markers.push(export_marker(graph, c)?),
            _ => {}
        }
    }
    Ok(out)
}

/// Rebuild a [`Track`] from an `nle.track` node.
pub fn export_track(graph: &Graph, track: EntityId) -> Result<Track> {
    require_kind(graph, track, kinds::NLE_TRACK)?;
    let node = graph.require(track)?;
    let kind = match node.get_str(attrs::TRACK_KIND).unwrap_or("video") {
        "audio" | "Audio" => TrackKind::Audio,
        _ => TrackKind::Video,
    };
    let mut out = Track {
        name: node.get_str(attrs::NAME).unwrap_or_default().to_string(),
        kind,
        children: Vec::new(),
        markers: Vec::new(),
        enabled: node.get_bool(nle_attrs::ENABLED).unwrap_or(true),
        metadata: value_to_metadata(node.get(attrs::METADATA)),
    };
    for c in graph.children(track) {
        let k = graph.require(c)?.kind.as_str();
        match k {
            kinds::NLE_CLIP | kinds::NLE_GAP | kinds::NLE_TRANSITION => {
                out.children.push(export_item(graph, c)?)
            }
            kinds::NLE_MARKER => out.markers.push(export_marker(graph, c)?),
            _ => {}
        }
    }
    Ok(out)
}

/// Rebuild an [`Item`] from an `nle.clip` / `nle.gap` / `nle.transition` node.
pub fn export_item(graph: &Graph, id: EntityId) -> Result<Item> {
    let node = graph.require(id)?;
    let name = node.get_str(attrs::NAME).unwrap_or_default().to_string();
    let mut metadata = value_to_metadata(node.get(attrs::METADATA));
    metadata.insert(UCG_ID_KEY.to_string(), J::String(id.to_string()));
    let item = match node.kind.as_str() {
        kinds::NLE_CLIP => {
            let media_reference = match media_of_clip(graph, id) {
                Some(m) => {
                    let mn = graph.require(m)?;
                    MediaReference::External {
                        target_url: mn
                            .get_str(attrs::MEDIA_PATH)
                            .unwrap_or_default()
                            .to_string(),
                        available_range: mn.get(attrs::AVAILABLE_RANGE).and_then(Value::as_range),
                    }
                }
                None => match node.get(nle_attrs::MEDIA_REF) {
                    Some(v) => otio_json::read_media_reference(&v.to_json())?,
                    None => MediaReference::Missing,
                },
            };
            let mut markers = Vec::new();
            for c in graph.children(id) {
                if graph
                    .node(c)
                    .map(|n| n.kind.as_str() == kinds::NLE_MARKER)
                    .unwrap_or(false)
                {
                    markers.push(export_marker(graph, c)?);
                }
            }
            Item::Clip(Clip {
                name,
                media_reference,
                source_range: node.get(attrs::SOURCE_RANGE).and_then(Value::as_range),
                markers,
                metadata,
                enabled: node.get_bool(nle_attrs::ENABLED).unwrap_or(true),
                // The graph currently stores one reference per clip. Multiple
                // references survive an OTIO round-trip but are not yet
                // represented as graph nodes; see the Phase 3 blueprint.
                ..Default::default()
            })
        }
        kinds::NLE_GAP => {
            let source_range = node
                .get(attrs::SOURCE_RANGE)
                .and_then(Value::as_range)
                .or_else(|| ClipRef::read(node).map(|c| c.source_range))
                .ok_or(Error::MissingAttr {
                    id,
                    attr: attrs::SOURCE_RANGE,
                })?;
            Item::Gap(Gap {
                source_range,
                name,
                metadata,
            })
        }
        kinds::NLE_TRANSITION => Item::Transition(Transition {
            name,
            transition_type: TransitionType::from_otio(
                node.get_str(nle_attrs::TRANSITION_TYPE)
                    .unwrap_or("SMPTE_Dissolve"),
            ),
            in_offset: node
                .get(nle_attrs::IN_OFFSET)
                .and_then(Value::as_time)
                .ok_or(Error::MissingAttr {
                    id,
                    attr: nle_attrs::IN_OFFSET,
                })?,
            out_offset: node
                .get(nle_attrs::OUT_OFFSET)
                .and_then(Value::as_time)
                .ok_or(Error::MissingAttr {
                    id,
                    attr: nle_attrs::OUT_OFFSET,
                })?,
            metadata,
        }),
        other => {
            return Err(Error::WrongKind {
                id,
                expected: kinds::NLE_CLIP,
                actual: other.to_string(),
            })
        }
    };
    Ok(item)
}

fn export_marker(graph: &Graph, id: EntityId) -> Result<Marker> {
    let node = graph.require(id)?;
    Ok(Marker {
        name: node.get_str(attrs::NAME).unwrap_or_default().to_string(),
        marked_range: node
            .get(attrs::RANGE)
            .and_then(Value::as_range)
            .unwrap_or_default(),
        color: node
            .get_str(nle_attrs::MARKER_COLOR)
            .unwrap_or("GREEN")
            .to_string(),
        comment: node
            .get_str(nle_attrs::MARKER_COMMENT)
            .unwrap_or_default()
            .to_string(),
        metadata: value_to_metadata(node.get(attrs::METADATA)),
    })
}

// ---------------------------------------------------------------------------
// queries and reactor-facing mutations
// ---------------------------------------------------------------------------

/// Clips carrying a `SyncedTo` edge to `scene_or_shot`.
pub fn clips_synced_to(graph: &Graph, scene_or_shot: EntityId) -> Vec<EntityId> {
    graph
        .sources(scene_or_shot, EdgeKind::SyncedTo)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .map(|n| n.kind.as_str() == kinds::NLE_CLIP)
                .unwrap_or(false)
        })
        .collect()
}

/// Add a `SyncedTo` edge from `clip` to `scene` (a scene or a shot).
pub fn bind_clip_to_scene(tx: &mut Tx, clip: EntityId, scene: EntityId) -> Result<()> {
    require_kind(tx.graph(), clip, kinds::NLE_CLIP)?;
    tx.graph().require(scene)?;
    tx.link(clip, scene, EdgeKind::SyncedTo)?;
    Ok(())
}

/// Mark a clip as needing conform (its source changed). Returns whether the
/// graph changed — re-flagging with the same reason is a no-op.
pub fn flag_conform(tx: &mut Tx, clip: EntityId, reason: &str) -> Result<bool> {
    let node = tx.graph().require(clip)?;
    let already = node.get_bool(attrs::NEEDS_CONFORM) == Some(true)
        && node.get_str(attrs::DIRTY_REASON) == Some(reason);
    if already {
        return Ok(false);
    }
    tx.set(clip, attrs::NEEDS_CONFORM, true)?;
    tx.set(clip, attrs::DIRTY_REASON, reason)?;
    Ok(true)
}

/// Clear the conform flag and its reason.
pub fn clear_conform(tx: &mut Tx, clip: EntityId) -> Result<()> {
    tx.graph().require(clip)?;
    tx.set(clip, attrs::NEEDS_CONFORM, false)?;
    tx.remove_attr(clip, attrs::DIRTY_REASON)?;
    Ok(())
}

/// The clip's range on its track (`attrs::RANGE`).
pub fn clip_track_range(graph: &Graph, clip: EntityId) -> Option<TimeRange> {
    graph.attr(clip, attrs::RANGE).and_then(Value::as_range)
}

/// The `asset.media` the clip is bound to.
pub fn media_of_clip(graph: &Graph, clip: EntityId) -> Option<EntityId> {
    graph.targets(clip, EdgeKind::Binds).into_iter().find(|m| {
        graph
            .node(*m)
            .map(|n| n.kind.as_str() == kinds::ASSET_MEDIA)
            .unwrap_or(false)
    })
}

/// The `nle.track` containing an item.
pub fn track_of_item(graph: &Graph, item: EntityId) -> Option<EntityId> {
    graph.ancestor_of_kind(item, kinds::NLE_TRACK)
}

/// The `nle.timeline` containing a node.
pub fn timeline_of(graph: &Graph, node: EntityId) -> Option<EntityId> {
    graph.ancestor_of_kind(node, kinds::NLE_TIMELINE)
}

/// Stable id prefix for new items on `track`: `nle:<source_key>:<track order>`.
fn item_key_prefix(graph: &Graph, track: EntityId) -> String {
    let source_key = timeline_of(graph, track)
        .and_then(|t| {
            graph
                .attr(t, attrs::SOURCE_KEY)
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| track.to_string());
    let order = graph
        .attr(track, attrs::ORDER)
        .and_then(Value::as_int)
        .unwrap_or(0);
    format!("nle:{source_key}:{order}")
}

/// Load `track` into a [`Track`], apply `op`, and write back only what
/// changed. Item ids are preserved through the `ucg_id` that
/// [`export_track`] stamps on each item; items created by a split reuse the
/// id at their position when free, otherwise the position key.
pub fn apply_edit(tx: &mut Tx, track: EntityId, op: EditOp) -> Result<()> {
    let mut t = export_track(tx.graph(), track)?;
    match op {
        EditOp::RippleTrim { index, edge, delta } => {
            edit::ripple_trim(&mut t, index, edge, delta)?;
        }
        EditOp::Roll { cut, delta } => {
            edit::roll(&mut t, cut, delta)?;
        }
        EditOp::Slip { index, delta } => {
            edit::slip(&mut t, index, delta)?;
        }
        EditOp::Slide { index, delta } => {
            edit::slide(&mut t, index, delta)?;
        }
        EditOp::SplitAt { time } => {
            edit::split_at(&mut t, time)?;
        }
        EditOp::Extract { range } => {
            edit::extract(&mut t, range)?;
        }
        EditOp::Lift { range } => {
            edit::lift(&mut t, range)?;
        }
    }
    let prefix = item_key_prefix(tx.graph(), track);
    let ids = assign_item_ids(tx.graph(), track, &t, &prefix, true);
    write_track_items(tx, track, &t, &ids)
}

/// Read a track node's editorial state.
///
/// The state lives in the track's OTIO metadata, which the graph already stores
/// verbatim, so this is the same state [`crate::session::TrackState::of`] reads
/// from an exported `Track` and the same state an edit is refused by. Going
/// through here rather than poking the attribute keeps one representation.
pub fn track_state(graph: &Graph, track_id: EntityId) -> Result<crate::session::TrackState> {
    require_kind(graph, track_id, kinds::NLE_TRACK)?;
    let track = export_track(graph, track_id)?;
    Ok(crate::session::TrackState::of(&track))
}

/// Write a track node's editorial state, leaving every other metadata key and
/// every item alone.
pub fn set_track_state(
    tx: &mut Tx,
    track_id: EntityId,
    state: crate::session::TrackState,
) -> Result<()> {
    require_kind(tx.graph(), track_id, kinds::NLE_TRACK)?;
    let mut metadata = value_to_metadata(
        tx.graph()
            .node(track_id)
            .and_then(|n| n.get(attrs::METADATA)),
    );
    // Apply to a bare track so only the namespace this owns is touched.
    let mut carrier = Track::new("", TrackKind::Video);
    carrier.metadata = metadata.clone();
    state.apply(&mut carrier);
    metadata = carrier.metadata;
    tx.set(track_id, attrs::METADATA, metadata_to_value(&metadata))?;
    Ok(())
}

/// Replace the items of a track node with those of `track` (same id policy
/// as [`apply_edit`]). Useful after pure edits done outside the graph.
pub fn write_track(tx: &mut Tx, track_id: EntityId, track: &Track) -> Result<()> {
    require_kind(tx.graph(), track_id, kinds::NLE_TRACK)?;
    tx.set(track_id, attrs::NAME, track.name.as_str())?;
    tx.set(track_id, attrs::TRACK_KIND, track_kind_str(track.kind))?;
    tx.set(track_id, nle_attrs::ENABLED, track.enabled)?;
    tx.set(
        track_id,
        attrs::METADATA,
        metadata_to_value(&track.metadata),
    )?;
    let prefix = item_key_prefix(tx.graph(), track_id);
    let ids = assign_item_ids(tx.graph(), track_id, track, &prefix, true);
    write_track_items(tx, track_id, track, &ids)
}
