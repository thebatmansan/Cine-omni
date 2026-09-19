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

use crate::edit::Edge;
use crate::error::{Error, Result};
use crate::otio::*;
use crate::otio_json;
use serde_json::Value as J;
use std::collections::{BTreeMap, BTreeSet, HashSet};
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
    /// A retimed clip's length on the track. Absent means real time.
    pub const RETIME_PLAYED: &str = "nle.retime_played";
    pub const RETIME_REVERSED: &str = "nle.retime_reversed";
    pub const RETIME_KEEP_PITCH: &str = "nle.retime_keep_pitch";
    /// A speed ramp's segments, when the speed changes over the clip.
    pub const RETIME_RAMP: &str = "nle.retime_ramp";
    /// A multicam clip's angles: `{ key: entity id }`. Absent on the ordinary
    /// single-reference clip, which is nearly every clip.
    pub const ANGLES: &str = "nle.angles";
    /// Which key in [`ANGLES`] is playing.
    pub const ANGLE: &str = "nle.angle";
    /// A caption track's language (BCP 47) and the rate its times count in.
    pub const CAPTION_LANGUAGE: &str = "nle.caption_language";
    pub const CAPTION_RATE: &str = "nle.caption_rate";
    /// One caption's lines, placement and speaker.
    pub const CAPTION_LINES: &str = "nle.caption_lines";
    pub const CAPTION_PLACEMENT: &str = "nle.caption_placement";
    pub const CAPTION_SPEAKER: &str = "nle.caption_speaker";
    /// A caption's stable key, and the next key a track will issue.
    pub const CAPTION_KEY: &str = "nle.caption_key";
    pub const CAPTION_NEXT_KEY: &str = "nle.caption_next_key";
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
    /// Ripple-insert new material at `time`.
    Insert {
        time: RationalTime,
        clip: ClipSpec,
    },
    /// Overwrite whatever lies under the new material; nothing moves.
    Overwrite {
        time: RationalTime,
        clip: ClipSpec,
    },
    /// Replace the item at `index`, keeping its duration.
    Replace {
        index: usize,
        clip: ClipSpec,
    },
    /// Join the item at `index` with the one after it.
    Join {
        index: usize,
    },
}

/// New material for an insert, overwrite or replace: a range of a media asset
/// already in the project.
///
/// Material is named by entity, never by path. A path is where a file was last
/// seen; the asset is what the project knows about it, including its available
/// range, which is what keeps a later trim inside real media.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClipSpec {
    pub media: EntityId,
    pub source_range: TimeRange,
    #[serde(default)]
    pub name: Option<String>,
}

impl EditOp {
    /// Resolve against the graph into a model edit.
    pub fn resolve(&self, graph: &Graph) -> Result<crate::linked::TrackEdit> {
        use crate::linked::TrackEdit as T;
        Ok(match self.clone() {
            EditOp::RippleTrim { index, edge, delta } => T::RippleTrim { index, edge, delta },
            EditOp::Roll { cut, delta } => T::Roll { cut, delta },
            EditOp::Slip { index, delta } => T::Slip { index, delta },
            EditOp::Slide { index, delta } => T::Slide { index, delta },
            EditOp::SplitAt { time } => T::SplitAt { time },
            EditOp::Extract { range } => T::Extract { range },
            EditOp::Lift { range } => T::Lift { range },
            EditOp::Insert { time, clip } => T::Insert {
                time,
                item: clip_item(graph, &clip)?,
            },
            EditOp::Overwrite { time, clip } => T::Overwrite {
                time,
                item: clip_item(graph, &clip)?,
            },
            EditOp::Replace { index, clip } => T::Replace {
                index,
                item: clip_item(graph, &clip)?,
            },
            EditOp::Join { index } => T::Join { index },
        })
    }
}

/// Build a clip item for `spec` from the media asset it names.
///
/// Refused when the asset has no path, or when the requested range is not
/// inside the media's available range: an edit that plays media that does not
/// exist is not an edit.
pub fn clip_item(graph: &Graph, spec: &ClipSpec) -> Result<Item> {
    require_kind(graph, spec.media, kinds::ASSET_MEDIA)?;
    let node = graph.require(spec.media)?;
    let path = node.get_str(attrs::MEDIA_PATH).ok_or(Error::MissingAttr {
        id: spec.media,
        attr: attrs::MEDIA_PATH,
    })?;
    let available = node.get(attrs::AVAILABLE_RANGE).and_then(Value::as_range);
    let zero = RationalTime::zero(spec.source_range.duration.rate_exact());
    if spec.source_range.duration <= zero {
        return Err(Error::Edit(
            "new material must have a positive duration".into(),
        ));
    }
    if let Some(a) = available {
        if spec.source_range.start_time < a.start_time
            || spec.source_range.end_time_exclusive() > a.end_time_exclusive()
        {
            return Err(Error::Edit(format!(
                "the requested range is outside the media: {} has {:?}",
                node.get_str(attrs::NAME).unwrap_or(path),
                a
            )));
        }
    }
    let name = spec
        .name
        .clone()
        .or_else(|| node.get_str(attrs::NAME).map(str::to_string))
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string());
    Ok(Item::Clip(Clip::external(
        name,
        path,
        available,
        Some(spec.source_range),
    )))
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
            // The retime is the pair, stored exactly. `ClipRef::source_range`
            // below stays the media consumed, and `attrs::RANGE` above is
            // already the clip's length on the track, so a reader that knows
            // nothing about retiming still composes the cut correctly.
            set_opt(
                tx,
                id,
                nle_attrs::RETIME_PLAYED,
                clip.retime.as_ref().map(|r| r.played),
            )?;
            set_opt(
                tx,
                id,
                nle_attrs::RETIME_REVERSED,
                clip.retime.as_ref().map(|r| r.reversed),
            )?;
            set_opt(
                tx,
                id,
                nle_attrs::RETIME_KEEP_PITCH,
                clip.retime.as_ref().filter(|r| r.keep_pitch).map(|_| true),
            )?;
            set_opt(
                tx,
                id,
                nle_attrs::RETIME_RAMP,
                clip.retime
                    .as_ref()
                    .and_then(|r| r.ramp.as_ref())
                    .map(Value::from_serde)
                    .transpose()
                    .map_err(Error::Ucg)?,
            )?;
            let (media, _angles) = write_media(tx, id, clip)?;
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
/// Write a clip's media: the reference it plays, every angle it could play,
/// and exactly the `Binds` edges those need.
///
/// The binding is done **once, for the whole set**, and that is not a tidiness
/// preference. Writing the active reference with its own rebind and then
/// linking the angles afterwards would unbind and relink every angle on every
/// identical re-write, and a graph writer that produces ops when nothing
/// changed breaks reactor convergence. So the assets are resolved first —
/// created or found, no edges touched — and one pass reconciles the edges.
///
/// Returns the active asset and the angle map.
fn write_media(
    tx: &mut Tx,
    clip: EntityId,
    c: &Clip,
) -> Result<(Option<EntityId>, BTreeMap<String, EntityId>)> {
    let active = ensure_asset(tx, clip, &c.media_reference)?;
    let mut angles: BTreeMap<String, EntityId> = BTreeMap::new();
    if let Some(m) = active {
        angles.insert(c.active_media_reference_key.clone(), m);
        tx.remove_attr(clip, nle_attrs::MEDIA_REF)?;
    } else {
        tx.set(
            clip,
            nle_attrs::MEDIA_REF,
            Value::from_json(otio_json::media_reference_value(&c.media_reference)),
        )?;
    }
    for (key, reference) in &c.alternate_media_references {
        // Only a real file can be an angle: there is nothing to cut to in a
        // generator or a missing reference, and one is not invented.
        if let Some(m) = ensure_asset(tx, clip, reference)? {
            angles.insert(key.clone(), m);
        }
    }
    let keep: BTreeSet<EntityId> = angles.values().copied().collect();
    bind_exactly(tx, clip, &keep)?;
    if angles.len() > 1 {
        let map: serde_json::Map<String, J> = angles
            .iter()
            .map(|(k, v)| (k.clone(), J::String(v.to_string())))
            .collect();
        tx.set(clip, nle_attrs::ANGLES, Value::from_json(J::Object(map)))?;
        tx.set(
            clip,
            nle_attrs::ANGLE,
            c.active_media_reference_key.as_str(),
        )?;
    } else {
        tx.remove_attr(clip, nle_attrs::ANGLES)?;
        tx.remove_attr(clip, nle_attrs::ANGLE)?;
    }
    Ok((active, angles))
}

/// The `asset.media` for one reference, created or found. No edges are
/// touched: binding is [`bind_exactly`]'s job.
fn ensure_asset(
    tx: &mut Tx,
    clip: EntityId,
    reference: &MediaReference,
) -> Result<Option<EntityId>> {
    let MediaReference::External {
        target_url,
        available_range,
    } = reference
    else {
        return Ok(None);
    };
    let media = EntityId::from_key(&format!("media:{target_url}"));
    // An asset already known by another identity — ingested and keyed by
    // content — is used as it is. A clip is derived material: it must not mint
    // a second identity for the same file, and it must not rewrite what
    // ingest recorded about it.
    if !tx.graph().contains(media) {
        if let Some(existing) = asset_at_path(tx.graph(), clip, target_url) {
            return Ok(Some(existing));
        }
    }
    ensure_kind(tx, media, kinds::ASSET_MEDIA)?;
    tx.set(media, attrs::MEDIA_PATH, target_url.as_str())?;
    tx.set(
        media,
        attrs::NAME,
        target_url.rsplit('/').next().unwrap_or(target_url),
    )?;
    set_opt(tx, media, attrs::AVAILABLE_RANGE, *available_range)?;
    Ok(Some(media))
}

/// The asset whose recorded path is `path`: the one the clip is already bound
/// to if it matches, otherwise any.
fn asset_at_path(graph: &Graph, clip: EntityId, path: &str) -> Option<EntityId> {
    let at_path = |m: &EntityId| {
        graph.node(*m).is_some_and(|n| {
            n.kind.as_str() == kinds::ASSET_MEDIA && n.get_str(attrs::MEDIA_PATH) == Some(path)
        })
    };
    graph
        .targets(clip, EdgeKind::Binds)
        .into_iter()
        .find(at_path)
        .or_else(|| {
            graph
                .of_kind(kinds::ASSET_MEDIA)
                .iter()
                .map(|n| n.id)
                .find(at_path)
        })
}

/// The clip's `Binds` edges to assets are exactly `keep`, and no more.
///
/// Set-wise so that re-running with the same set produces no ops: an edge that
/// is wanted and present is left alone.
fn bind_exactly(tx: &mut Tx, clip: EntityId, keep: &BTreeSet<EntityId>) -> Result<()> {
    let bound: Vec<EntityId> = bound_assets(tx.graph(), clip);
    for m in &bound {
        if !keep.contains(m) {
            tx.unlink(clip, *m, EdgeKind::Binds)?;
        }
    }
    for m in keep {
        if !bound.contains(m) {
            tx.link(clip, *m, EdgeKind::Binds)?;
        }
    }
    Ok(())
}

fn bound_assets(graph: &Graph, clip: EntityId) -> Vec<EntityId> {
    graph
        .targets(clip, EdgeKind::Binds)
        .into_iter()
        .filter(|m| {
            graph
                .node(*m)
                .map(|n| n.kind.as_str() == kinds::ASSET_MEDIA)
                .unwrap_or(false)
        })
        .collect()
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
                Some(m) => reference_of_asset(graph, m)?,
                None => match node.get(nle_attrs::MEDIA_REF) {
                    Some(v) => otio_json::read_media_reference(&v.to_json())?,
                    None => MediaReference::Missing,
                },
            };
            // Angles: every other reference the clip carries, rebuilt from the
            // assets it is bound to.
            let angles = angles_of_clip(graph, id);
            let active_key =
                angle_of_clip(graph, id).unwrap_or_else(|| DEFAULT_MEDIA_KEY.to_string());
            let mut alternates = BTreeMap::new();
            for (key, asset) in &angles {
                if *key != active_key {
                    alternates.insert(key.clone(), reference_of_asset(graph, *asset)?);
                }
            }
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
                retime: node
                    .get(nle_attrs::RETIME_PLAYED)
                    .and_then(Value::as_time)
                    .map(|played| Retime {
                        played,
                        reversed: node.get_bool(nle_attrs::RETIME_REVERSED).unwrap_or(false),
                        keep_pitch: node.get_bool(nle_attrs::RETIME_KEEP_PITCH).unwrap_or(false),
                        ramp: node.get_as(nle_attrs::RETIME_RAMP),
                    }),
                markers,
                metadata,
                enabled: node.get_bool(nle_attrs::ENABLED).unwrap_or(true),
                active_media_reference_key: active_key,
                alternate_media_references: alternates,
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

/// The `asset.media` the clip is **playing**.
///
/// A multicam clip is bound to every angle, so "the asset this clip plays" is
/// the one the active angle names, not whichever edge happens to come first.
pub fn media_of_clip(graph: &Graph, clip: EntityId) -> Option<EntityId> {
    let bound = bound_assets(graph, clip);
    if bound.len() > 1 {
        if let Some(active) = angle_of_clip(graph, clip)
            .and_then(|key| angles_of_clip(graph, clip).get(&key).copied())
        {
            if bound.contains(&active) {
                return Some(active);
            }
        }
    }
    bound.into_iter().next()
}

/// A clip's angles by key. Empty for the ordinary single-reference clip.
pub fn angles_of_clip(graph: &Graph, clip: EntityId) -> BTreeMap<String, EntityId> {
    let mut out = BTreeMap::new();
    let Some(J::Object(map)) = graph.attr(clip, nle_attrs::ANGLES).map(Value::to_json) else {
        return out;
    };
    for (key, id) in map {
        if let Some(id) = id.as_str().and_then(EntityId::parse) {
            out.insert(key, id);
        }
    }
    out
}

/// Which angle is playing, when the clip has more than one.
pub fn angle_of_clip(graph: &Graph, clip: EntityId) -> Option<String> {
    graph
        .attr(clip, nle_attrs::ANGLE)
        .and_then(|v| v.as_str().map(str::to_string))
}

fn reference_of_asset(graph: &Graph, media: EntityId) -> Result<MediaReference> {
    let n = graph.require(media)?;
    Ok(MediaReference::External {
        target_url: n.get_str(attrs::MEDIA_PATH).unwrap_or_default().to_string(),
        available_range: n.get(attrs::AVAILABLE_RANGE).and_then(Value::as_range),
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
    let edit = op.resolve(tx.graph())?;
    let mut t = export_track(tx.graph(), track)?;
    crate::linked::apply_one(&mut t, &edit)?;
    let prefix = item_key_prefix(tx.graph(), track);
    let ids = assign_item_ids(tx.graph(), track, &t, &prefix, true);
    write_track_items(tx, track, &t, &ids)
}

/// What [`perform_edit`] did, in graph terms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphEditOutcome {
    pub label: &'static str,
    pub requested: Option<RationalTime>,
    pub applied: Option<RationalTime>,
    /// The item the edit produced or acted on, on the primary track.
    pub item: Option<EntityId>,
    pub changed_tracks: Vec<EntityId>,
    pub linked_clips: usize,
}

/// The tracks sharing a stack with `track`, in order.
pub fn sibling_tracks(graph: &Graph, track: EntityId) -> Result<Vec<EntityId>> {
    require_kind(graph, track, kinds::NLE_TRACK)?;
    let Some(parent) = graph.parent(track) else {
        return Ok(vec![track]);
    };
    let mut tracks: Vec<(i64, EntityId)> = graph
        .children(parent)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .is_some_and(|n| n.kind.as_str() == kinds::NLE_TRACK)
        })
        .map(|c| {
            (
                graph
                    .attr(c, attrs::ORDER)
                    .and_then(Value::as_int)
                    .unwrap_or(0),
                c,
            )
        })
        .collect();
    tracks.sort();
    Ok(tracks.into_iter().map(|(_, c)| c).collect())
}

/// Apply an edit to `track` and, when `follow_links`, to the material linked
/// to it on the other tracks of its stack — in the caller's transaction, so
/// the whole edit is one undo step. See [`crate::linked`].
pub fn perform_edit(
    tx: &mut Tx,
    track: EntityId,
    op: EditOp,
    follow_links: bool,
) -> Result<GraphEditOutcome> {
    let edit = op.resolve(tx.graph())?;
    let label = edit.label();
    let ids = sibling_tracks(tx.graph(), track)?;
    let primary = ids.iter().position(|t| *t == track).unwrap_or(0);
    let mut tracks = ids
        .iter()
        .map(|t| export_track(tx.graph(), *t))
        .collect::<Result<Vec<_>>>()?;
    let outcome = crate::linked::apply_linked(&mut tracks, primary, edit, follow_links)?;
    let mut item = None;
    for ti in &outcome.changed_tracks {
        let id = ids[*ti];
        let t = &tracks[*ti];
        let prefix = item_key_prefix(tx.graph(), id);
        let item_ids = assign_item_ids(tx.graph(), id, t, &prefix, true);
        if *ti == primary {
            item = outcome.index.and_then(|i| item_ids.get(i).copied());
        }
        write_track_items(tx, id, t, &item_ids)?;
    }
    if item.is_none() {
        // Unchanged primary (e.g. a zero-delta trim): name the existing item.
        item = outcome
            .index
            .and_then(|i| tracks[primary].children.get(i))
            .and_then(Item::ucg_id);
    }
    Ok(GraphEditOutcome {
        label,
        requested: outcome.requested,
        applied: outcome.applied,
        item,
        changed_tracks: outcome.changed_tracks.iter().map(|i| ids[*i]).collect(),
        linked_clips: outcome.linked_clips,
    })
}

/// Create an empty sequence under `project`: one picture and one sound track,
/// at an exact `rate` recorded as its start time so an empty cut still knows
/// what a frame is. Returns the timeline entity.
pub fn create_sequence(
    tx: &mut Tx,
    project: EntityId,
    name: &str,
    rate: ucg_core::Rate,
    start: Option<RationalTime>,
) -> Result<EntityId> {
    let mut key = format!("sequence:{name}");
    let mut n = 1;
    while tx
        .graph()
        .contains(EntityId::from_key(&format!("nle:{key}")))
    {
        n += 1;
        key = format!("sequence:{name}:{n}");
    }
    let mut timeline = Timeline::new(name);
    // Where the cut's own clock starts: the record timecode an EDL, a delivery
    // master and a burn-in all count in. Zero unless the caller says
    // otherwise — a sequence that starts at an hour is an editorial decision,
    // and this does not make it on anybody's behalf.
    // A record start at a different rate is refused rather than rounded: a
    // timecode counts frames of a particular signal, and converting one to
    // another rate moves which frame it names.
    timeline.global_start_time = Some(match start {
        Some(t) if t.rate_exact() != rate => {
            return Err(crate::error::Error::Otio(format!(
                "a record start counting at {} cannot begin a sequence at {rate}",
                t.rate_exact()
            )))
        }
        Some(t) => t,
        None => RationalTime::zero(rate),
    });
    timeline
        .tracks
        .children
        .push(Composable::Track(Track::new("V1", TrackKind::Video)));
    timeline
        .tracks
        .children
        .push(Composable::Track(Track::new("A1", TrackKind::Audio)));
    import_timeline(tx, project, &timeline, &key)
}

/// Place new material on several tracks at once — typically a take's picture
/// and its sound — inserting (rippling each track) or overwriting, and link the
/// pieces so later edits keep them together. Tracks not named are not moved.
/// Returns the new clips, in the order of `tracks`.
pub fn place_material(
    tx: &mut Tx,
    tracks: &[EntityId],
    time: RationalTime,
    spec: &ClipSpec,
    overwrite: bool,
) -> Result<Vec<EntityId>> {
    let mut placed = Vec::with_capacity(tracks.len());
    for track in tracks {
        let op = if overwrite {
            EditOp::Overwrite {
                time,
                clip: spec.clone(),
            }
        } else {
            EditOp::Insert {
                time,
                clip: spec.clone(),
            }
        };
        let out = perform_edit(tx, *track, op, false)?;
        placed.push(
            out.item
                .ok_or_else(|| Error::Edit("placed material has no entity".into()))?,
        );
    }
    if placed.len() > 1 {
        let group = format!("take:{}", placed[0]);
        for clip in &placed {
            set_clip_link(tx, *clip, Some(&group))?;
        }
    }
    Ok(placed)
}

/// Set or clear a clip's retime.
///
/// A retime changes how long the clip takes on its track, so everything after
/// it moves: this is a ripple, not an attribute set, and it goes through the
/// track's own writer for the same reason every other edit does — the ranges
/// of the items after it have to be rewritten, and one place should do that.
///
/// Refused when the clip has no source range (there is no media length to
/// play at a speed), when the new length is not positive, or when the clip
/// would end up consuming no picture at all.
pub fn set_clip_retime(tx: &mut Tx, clip: EntityId, retime: Option<Retime>) -> Result<()> {
    require_kind(tx.graph(), clip, kinds::NLE_CLIP)?;
    let (track, index) = place_of_item(tx.graph(), clip)?;
    let mut t = export_track(tx.graph(), track)?;
    crate::session::ensure_unlocked(&t)?;
    let Item::Clip(c) = &mut t.children[index] else {
        return Err(Error::NotAnItem(index));
    };
    let mut retime = retime;
    if let Some(r) = &mut retime {
        let media = c.source_range.ok_or_else(|| {
            Error::Edit("a clip with no source range has no media to play at a speed".into())
        })?;
        if media.duration <= RationalTime::zero(media.duration.rate_exact()) {
            return Err(Error::Edit("the clip plays no media".into()));
        }
        if let Some(ramp) = &r.ramp {
            // A ramp decides both lengths: the track time is its segments', and
            // the media is what they consume.
            let consumed = check_ramp(ramp)?;
            let mr = media.duration.rate_exact();
            let needed = consumed * ucg_core::Q::new(i128::from(mr.num()), i128::from(mr.den()));
            // Whole pictures: a ramp that ends partway through one still shows it.
            let ticks = needed.floor() + i128::from(!needed.is_integer());
            let ticks = i64::try_from(ticks).map_err(|_| {
                Error::Edit("the ramp consumes more media than can be counted".into())
            })?;
            let wanted = TimeRange::new(media.start_time, RationalTime::exact(ticks, mr));
            if let Some(available) = c.available_range() {
                if wanted.end_time_exclusive() > available.end_time_exclusive() {
                    let short = wanted
                        .end_time_exclusive()
                        .checked_sub(&available.end_time_exclusive())
                        .to_frames();
                    return Err(Error::Edit(format!(
                        "the ramp consumes {short} frames more media than the shot has after its in point"
                    )));
                }
            }
            c.source_range = Some(wanted);
            let played = ramp
                .iter()
                .fold(RationalTime::zero(ramp[0].length.rate_exact()), |acc, s| {
                    acc.checked_add(&s.length)
                });
            r.played = played;
        }
        if r.played <= RationalTime::zero(r.played.rate_exact()) {
            return Err(Error::Edit(
                "a retime has to leave the clip some length on the track".into(),
            ));
        }
    }
    c.retime = retime;
    write_one_track(tx, track, &t)
}

/// A ramp an editor could mean: segments with length, speeds that are not
/// both zero, and some media consumed. Returns the media seconds it consumes.
fn check_ramp(ramp: &[ucg_core::Ramp]) -> Result<ucg_core::Q> {
    if ramp.is_empty() {
        return Err(Error::Edit(
            "a speed ramp needs at least one segment".into(),
        ));
    }
    for (i, s) in ramp.iter().enumerate() {
        if s.length <= RationalTime::zero(s.length.rate_exact()) {
            return Err(Error::Edit(format!("ramp segment {} has no length", i + 1)));
        }
    }
    let map = ucg_core::TimeMap {
        segments: ramp.to_vec(),
        reversed: false,
    };
    let consumed = map.consumed();
    if consumed <= ucg_core::Q::ZERO {
        return Err(Error::Edit(
            "the ramp consumes no media: a clip at zero speed throughout is a freeze, not a ramp"
                .into(),
        ));
    }
    Ok(consumed)
}

/// Give a clip more angles: assets it could play instead of the one it plays.
///
/// This is how a multicam clip comes into being — place one angle, then attach
/// the others by the names a production calls them. The assets must already be
/// in the project, because an angle is a file somebody ingested, not a path
/// somebody typed.
///
/// The angles are not re-synced here. Alignment is carried by each asset's own
/// available range: two angles are in sync when their ranges are in the same
/// time base, which is what timecode gives and what [`crate::sync`] measures.
/// An angle whose range does not cover this clip is accepted — it is still an
/// angle — and refused later, by name, when somebody tries to cut to it.
pub fn add_angles(tx: &mut Tx, clip: EntityId, angles: &[(String, EntityId)]) -> Result<()> {
    require_kind(tx.graph(), clip, kinds::NLE_CLIP)?;
    let (track, index) = place_of_item(tx.graph(), clip)?;
    let mut t = export_track(tx.graph(), track)?;
    crate::session::ensure_unlocked(&t)?;
    let Item::Clip(c) = &mut t.children[index] else {
        return Err(Error::NotAnItem(index));
    };
    for (key, asset) in angles {
        require_kind(tx.graph(), *asset, kinds::ASSET_MEDIA)?;
        if *key == c.active_media_reference_key {
            return Err(Error::Edit(format!(
                "`{key}` is already the angle this clip is playing"
            )));
        }
        c.alternate_media_references
            .insert(key.clone(), reference_of_asset(tx.graph(), *asset)?);
    }
    write_one_track(tx, track, &t)
}

/// Cut to another angle at `at`: split there if there is no boundary, then
/// play `key` from that point to the end of what was one clip.
///
/// That is what a multicam switch is — each switch is an event, and the
/// material before it keeps playing the angle it was playing.
pub fn switch_angle(tx: &mut Tx, track: EntityId, at: RationalTime, key: &str) -> Result<EntityId> {
    let mut t = export_track(tx.graph(), track)?;
    crate::session::ensure_unlocked(&t)?;
    let index = crate::edit::split_for_switch(&mut t, at)?;
    let Item::Clip(c) = &mut t.children[index] else {
        return Err(Error::NotAnItem(index));
    };
    c.play_reference(key).map_err(Error::Edit)?;
    let prefix = item_key_prefix(tx.graph(), track);
    let ids = assign_item_ids(tx.graph(), track, &t, &prefix, true);
    let id = ids[index];
    write_track_items(tx, track, &t, &ids)?;
    Ok(id)
}

/// Record where each named angle's media sits against the take — what a sync
/// by sound, or a slate, established. Keys the clip does not carry are refused.
pub fn set_angle_offsets(
    tx: &mut Tx,
    clip: EntityId,
    offsets: &[(String, RationalTime)],
) -> Result<()> {
    require_kind(tx.graph(), clip, kinds::NLE_CLIP)?;
    let (track, index) = place_of_item(tx.graph(), clip)?;
    let mut t = export_track(tx.graph(), track)?;
    crate::session::ensure_unlocked(&t)?;
    let Item::Clip(c) = &mut t.children[index] else {
        return Err(Error::NotAnItem(index));
    };
    let keys = c.reference_keys();
    for (key, offset) in offsets {
        if !keys.contains(key) {
            return Err(Error::Edit(format!(
                "`{key}` is not one of this clip's angles ({})",
                keys.join(", ")
            )));
        }
        c.set_angle_offset(key, *offset);
    }
    write_one_track(tx, track, &t)
}

/// One angle of the multicam clip under an instant.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AngleAt {
    pub key: String,
    pub active: bool,
    pub asset: Option<EntityId>,
    pub name: String,
    /// Where this angle's media sits against the take.
    pub offset: RationalTime,
    /// The media time this angle shows at the instant, when it has media there.
    pub media: Option<RationalTime>,
    /// Why it shows nothing, when it does not.
    pub why_not: Option<String>,
}

/// The angles of the multicam clip showing at `t` on a sequence: for each, the
/// moment of its own media that corresponds to the take at that instant.
///
/// The clip is the one the picture list shows at `t` — the top visible picture
/// track — so this is the multicam view of exactly what is on screen. `None`
/// when nothing shows or what shows has one reference.
pub fn angles_at(
    graph: &Graph,
    timeline: EntityId,
    t: RationalTime,
) -> Result<Option<(EntityId, Vec<AngleAt>)>> {
    let composed = export_timeline(graph, timeline)?;
    let list = crate::flatten::picture_segments(&composed);
    let Some(segment) = list.segments.iter().find(|s| s.record.contains(t)) else {
        return Ok(None);
    };
    let Some(track) = composed.tracks().nth(segment.track) else {
        return Ok(None);
    };
    let Some(Item::Clip(clip)) = track.children.get(segment.item) else {
        return Ok(None);
    };
    let (Some(id), Some(span)) = (segment.clip, track.range_of_child(segment.item)) else {
        return Ok(None);
    };
    let keys = clip.reference_keys();
    if keys.len() < 2 {
        return Ok(None);
    }
    let Some(active_media) = clip.media_time_at(span.start_time, t) else {
        return Ok(None);
    };
    let assets = angles_of_clip(graph, id);
    let active_offset = clip.angle_offset(&clip.active_media_reference_key);
    let mut out = Vec::new();
    for key in keys {
        let active = key == clip.active_media_reference_key;
        let reference = clip.media_reference_named(&key);
        let offset = clip.angle_offset(&key);
        let media = active_media.checked_add(&offset.checked_sub(&active_offset));
        let available = reference.and_then(MediaReference::available_range);
        let why_not = match available {
            None => Some("the file declares no range, so which picture is unknown".to_string()),
            Some(a) if media < a.start_time => Some(format!(
                "not rolling yet: starts {} frames later",
                a.start_time.checked_sub(&media).to_frames()
            )),
            Some(a) if media >= a.end_time_exclusive() => Some(format!(
                "stopped rolling {} frames earlier",
                media.checked_sub(&a.end_time_exclusive()).to_frames() + 1
            )),
            Some(_) => None,
        };
        let asset = assets.get(&key).copied();
        let name = asset
            .and_then(|a| graph.node(a))
            .and_then(|n| n.get_str(attrs::NAME))
            .map(str::to_string)
            .unwrap_or_else(|| key.clone());
        out.push(AngleAt {
            media: why_not.is_none().then_some(media),
            key,
            active,
            asset,
            name,
            offset,
            why_not,
        });
    }
    Ok(Some((id, out)))
}

/// Play a different angle on one clip, without splitting anything.
pub fn set_clip_angle(tx: &mut Tx, clip: EntityId, key: &str) -> Result<()> {
    require_kind(tx.graph(), clip, kinds::NLE_CLIP)?;
    let (track, index) = place_of_item(tx.graph(), clip)?;
    let mut t = export_track(tx.graph(), track)?;
    crate::session::ensure_unlocked(&t)?;
    let Item::Clip(c) = &mut t.children[index] else {
        return Err(Error::NotAnItem(index));
    };
    c.play_reference(key).map_err(Error::Edit)?;
    write_one_track(tx, track, &t)
}

/// The track an item is on and where it sits on it.
fn place_of_item(graph: &Graph, item: EntityId) -> Result<(EntityId, usize)> {
    let track = graph.parent(item).ok_or(Error::MissingAttr {
        id: item,
        attr: "a track",
    })?;
    let t = export_track(graph, track)?;
    let index = t
        .children
        .iter()
        .position(|i| i.ucg_id() == Some(item))
        .ok_or(Error::MissingAttr {
            id: item,
            attr: "a place on its track",
        })?;
    Ok((track, index))
}

/// Write a track back after changing it in memory.
fn write_one_track(tx: &mut Tx, track: EntityId, t: &Track) -> Result<()> {
    t.validate()?;
    let prefix = item_key_prefix(tx.graph(), track);
    let ids = assign_item_ids(tx.graph(), track, t, &prefix, true);
    write_track_items(tx, track, t, &ids)
}

/// Put a clip into a link group (or take it out with `None`).
pub fn set_clip_link(tx: &mut Tx, clip: EntityId, group: Option<&str>) -> Result<()> {
    require_kind(tx.graph(), clip, kinds::NLE_CLIP)?;
    let mut carrier = Clip {
        metadata: value_to_metadata(tx.graph().node(clip).and_then(|n| n.get(attrs::METADATA))),
        ..Default::default()
    };
    match group {
        Some(g) => crate::session::set_link(&mut carrier, g),
        None => crate::session::clear_link(&mut carrier),
    }
    tx.set(clip, attrs::METADATA, metadata_to_value(&carrier.metadata))?;
    Ok(())
}

/// Remove a `SyncedTo` edge from `clip` to `scene`.
pub fn unbind_clip_from_scene(tx: &mut Tx, clip: EntityId, scene: EntityId) -> Result<()> {
    require_kind(tx.graph(), clip, kinds::NLE_CLIP)?;
    tx.unlink(clip, scene, EdgeKind::SyncedTo)?;
    Ok(())
}

/// Scenes (or shots) a clip is bound to.
pub fn scenes_of_clip(graph: &Graph, clip: EntityId) -> Vec<EntityId> {
    graph.targets(clip, EdgeKind::SyncedTo)
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

/// Plan a three- or four-point edit against a media asset and carry it out.
///
/// The asset's own available range is what the plan is checked against, so an
/// edit that would run off the end of the shot is refused here rather than
/// discovered at playback. The plan is returned with the clips so a caller can
/// say which point was arithmetic — an editor who backtimed an edit needs to
/// see that it was backtimed.
pub fn three_point_edit(
    tx: &mut Tx,
    tracks: &[EntityId],
    media: EntityId,
    source: crate::three_point::Marks,
    record: crate::three_point::Marks,
    kind: crate::three_point::EditKind,
    on_mismatch: crate::three_point::Mismatch,
) -> Result<(crate::three_point::Plan, Vec<EntityId>)> {
    require_kind(tx.graph(), media, kinds::ASSET_MEDIA)?;
    let available = tx
        .graph()
        .require(media)?
        .get(attrs::AVAILABLE_RANGE)
        .and_then(Value::as_range);
    let plan = crate::three_point::plan(available, source, record, kind, on_mismatch)?;
    let spec = ClipSpec {
        media,
        source_range: plan.source,
        name: None,
    };
    let overwrite = kind == crate::three_point::EditKind::Overwrite;
    let placed = place_material(tx, tracks, plan.record.start_time, &spec, overwrite)?;
    Ok((plan, placed))
}

// ---------------------------------------------------------------------------
// captions
// ---------------------------------------------------------------------------

/// The node holding one language's captions for a sequence.
///
/// Contained by the project and bound to the sequence by a `References` edge.
/// Not contained by the timeline, because re-importing a cut reconciles the
/// timeline's children and would delete authored captions with the stack it
/// replaced. `References` rather than `SyncedTo` because nothing yet reacts to
/// a cut change by flagging captions for review, and an edge that claims
/// propagation should have something behind it.
pub fn captions_node(timeline: EntityId, language: &str) -> EntityId {
    EntityId::derived(timeline, &format!("captions:{language}"))
}

/// Languages captioned for a sequence, in the order they sort.
pub fn caption_languages(graph: &Graph, timeline: EntityId) -> Vec<String> {
    let mut out: Vec<String> = graph
        .of_kind(kinds::NLE_CAPTIONS)
        .iter()
        .filter(|n| {
            graph
                .targets(n.id, EdgeKind::References)
                .contains(&timeline)
        })
        .filter_map(|n| n.get_str(nle_attrs::CAPTION_LANGUAGE).map(str::to_string))
        .collect();
    out.sort();
    out
}

/// One language's captions, when the sequence has them.
pub fn export_captions(
    graph: &Graph,
    timeline: EntityId,
    language: &str,
) -> Result<Option<crate::captions::CaptionTrack>> {
    use crate::captions::{Caption, CaptionTrack, Placement};
    let id = captions_node(timeline, language);
    let Some(node) = graph.node(id) else {
        return Ok(None);
    };
    let rate: ucg_core::Rate = node
        .get_as(nle_attrs::CAPTION_RATE)
        .ok_or(Error::MissingAttr {
            id,
            attr: nle_attrs::CAPTION_RATE,
        })?;
    let mut rows: Vec<(i64, Caption)> = Vec::new();
    for c in graph.children(id) {
        let n = graph.require(c)?;
        if n.kind.as_str() != kinds::NLE_CAPTION {
            continue;
        }
        let record = n
            .get(attrs::RANGE)
            .and_then(Value::as_range)
            .ok_or(Error::MissingAttr {
                id: c,
                attr: attrs::RANGE,
            })?;
        rows.push((
            n.get_int(attrs::ORDER).unwrap_or(0),
            Caption {
                record,
                lines: n.get_as(nle_attrs::CAPTION_LINES).unwrap_or_default(),
                placement: match n.get_str(nle_attrs::CAPTION_PLACEMENT) {
                    Some("top") => Placement::Top,
                    _ => Placement::Bottom,
                },
                speaker: n.get_str(nle_attrs::CAPTION_SPEAKER).map(str::to_string),
                key: n.get_str(nle_attrs::CAPTION_KEY).map(str::to_string),
            },
        ));
    }
    rows.sort_by_key(|(order, _)| *order);
    Ok(Some(CaptionTrack {
        language: language.to_string(),
        rate,
        captions: rows.into_iter().map(|(_, c)| c).collect(),
    }))
}

/// The entity id of one caption, from its stable key.
pub fn caption_node(timeline: EntityId, language: &str, key: &str) -> EntityId {
    EntityId::derived(captions_node(timeline, language), &format!("caption:{key}"))
}

/// Write one language's captions for a sequence, in time order.
///
/// Idempotent: writing the same track again produces no ops. A caption keeps
/// its entity across edits by its key, so inserting one early moves nothing
/// that is attached to the ones after it — in particular, not the line of
/// dialogue a caption is linked to. A caption without a key is given the next
/// one, and keys are never reused within a track, so a removed caption's
/// links cannot land on a new one.
pub fn write_captions(
    tx: &mut Tx,
    timeline: EntityId,
    track: &crate::captions::CaptionTrack,
) -> Result<EntityId> {
    use crate::captions::Placement;
    require_kind(tx.graph(), timeline, kinds::NLE_TIMELINE)?;
    if track.language.trim().is_empty() {
        return Err(Error::Edit("a caption track needs a language".into()));
    }
    let project = tx.graph().parent(timeline).ok_or(Error::MissingAttr {
        id: timeline,
        attr: "a project",
    })?;
    let id = captions_node(timeline, &track.language);
    ensure_kind(tx, id, kinds::NLE_CAPTIONS)?;
    tx.link(project, id, EdgeKind::Contains)?;
    tx.link(id, timeline, EdgeKind::References)?;
    tx.set(id, attrs::NAME, format!("Captions ({})", track.language))?;
    tx.set(id, nle_attrs::CAPTION_LANGUAGE, track.language.as_str())?;
    tx.set(
        id,
        nle_attrs::CAPTION_RATE,
        Value::from_serde(&track.rate).map_err(Error::Ucg)?,
    )?;
    let mut sorted = track.clone();
    sorted.sort();
    let mut next = tx
        .graph()
        .node(id)
        .and_then(|n| n.get_int(nle_attrs::CAPTION_NEXT_KEY))
        .unwrap_or(1);
    let mut keep = Vec::with_capacity(sorted.captions.len());
    let mut seen = HashSet::new();
    for (i, c) in sorted.captions.iter().enumerate() {
        let key = match &c.key {
            Some(k) if seen.insert(k.clone()) => k.clone(),
            _ => loop {
                let k = format!("c{next}");
                next += 1;
                if seen.insert(k.clone()) {
                    break k;
                }
            },
        };
        let cid = caption_node(timeline, &track.language, &key);
        ensure_kind(tx, cid, kinds::NLE_CAPTION)?;
        tx.set(cid, nle_attrs::CAPTION_KEY, key.as_str())?;
        tx.link(id, cid, EdgeKind::Contains)?;
        tx.set(cid, attrs::ORDER, i)?;
        tx.set(cid, attrs::RANGE, c.record)?;
        tx.set(
            cid,
            nle_attrs::CAPTION_LINES,
            Value::from_serde(&c.lines).map_err(Error::Ucg)?,
        )?;
        tx.set(
            cid,
            nle_attrs::CAPTION_PLACEMENT,
            match c.placement {
                Placement::Bottom => "bottom",
                Placement::Top => "top",
            },
        )?;
        set_opt(tx, cid, nle_attrs::CAPTION_SPEAKER, c.speaker.clone())?;
        keep.push(cid);
    }
    // Keys issued so far, including any read back from the track: a key is
    // never handed out twice.
    for k in &seen {
        if let Some(n) = k.strip_prefix('c').and_then(|n| n.parse::<i64>().ok()) {
            next = next.max(n + 1);
        }
    }
    tx.set(id, nle_attrs::CAPTION_NEXT_KEY, next)?;
    sync_children(tx, id, &keep)?;
    Ok(id)
}

/// Remove one language's captions from a sequence.
pub fn remove_captions(tx: &mut Tx, timeline: EntityId, language: &str) -> Result<()> {
    let id = captions_node(timeline, language);
    if tx.graph().contains(id) {
        sync_children(tx, id, &[])?;
        tx.delete(id)?;
    }
    Ok(())
}
