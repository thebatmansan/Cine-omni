//! Media identity in the Unified Context Graph.
//!
//! This is the part that makes media a first-class citizen of the film rather
//! than a file path stored on a clip. Once media is in the graph with real
//! edges, the question *"what does replacing this take actually affect?"*
//! becomes a graph traversal instead of a manual audit.
//!
//! ## Node and edge shape
//!
//! ```text
//! logistics.take ─Binds→ asset.media ─Contains→ media.stream
//!                              ▲                     ▲
//!            nle.clip ─Binds───┘                     │
//!         media.derivative ─DerivedFrom──────────────┘   (proxy, thumbnail…)
//! ```
//!
//! * `asset.media` is the **file**, identified by content hash. It already
//!   exists as a node kind in `ucg-schema`, used by the timeline importer.
//! * `media.stream` is one elementary stream, contained by the file.
//! * `media.derivative` is anything generated from it, and always points back.
//!
//! ## Why these edge kinds
//!
//! `Binds` is used for clip → media because it is an *asset binding*: the clip
//! is not recomputed when the media's bytes change, it is re-*linked*.
//! `DerivedFrom` is used for proxies because they genuinely are derived and
//! must go stale when the source changes. That distinction is what lets
//! [`media_replacement_impact`] answer the question precisely.

use crate::error::Result;
use crate::identity::{ContentHash, Derivative, MediaFile, StreamKind};
use ucg_core::{EdgeKind, EntityId, Graph, Tx, Value};
use ucg_schema::{attrs, kinds};

/// Node kind for one elementary stream inside a media file.
pub const MEDIA_STREAM: &str = "media.stream";
/// Node kind for a proxy, transcode, thumbnail or waveform.
pub const MEDIA_DERIVATIVE: &str = "media.derivative";

/// Content hash, the durable identity of the file.
pub const HASH_ATTR: &str = "content_hash";
/// Full serialised `MediaFile`, so a probe need not be repeated on open.
pub const MEDIA_INFO_ATTR: &str = "media_info";
/// Whether the file was present at the path when last checked.
pub const ONLINE_ATTR: &str = "online";
/// Codec token, mirrored onto the stream node for cheap queries.
pub const CODEC_ATTR: &str = "codec";
/// Whether this build can decode the stream.
pub const DECODABLE_ATTR: &str = "decodable";

/// Deterministic id for a media file, derived from its **content**.
///
/// Two copies of one file resolve to a single entity; a re-export resolves to a
/// different one. This is the property relinking depends on.
pub fn media_entity_id(hash: &ContentHash) -> EntityId {
    EntityId::from_key(&format!("media:file:{}", hash.hex()))
}

/// Register a probed media file and its streams. Idempotent: re-ingesting the
/// same file produces no operations.
pub fn register_media(tx: &mut Tx, project: EntityId, info: &MediaFile) -> Result<EntityId> {
    let id = info.entity_id();
    tx.ensure(id, kinds::ASSET_MEDIA)?;
    tx.set(id, attrs::NAME, file_label(info))?;
    tx.set(id, attrs::MEDIA_PATH, info.path.display().to_string())?;
    tx.set(id, HASH_ATTR, info.hash.hex())?;
    tx.set(id, ONLINE_ATTR, info.path.exists())?;
    tx.set(id, MEDIA_INFO_ATTR, Value::from_serde(info)?)?;
    if tx.graph().contains(project) {
        tx.link(project, id, EdgeKind::Contains)?;
    }

    // One node per elementary stream, so a clip can bind to a specific stream
    // rather than to "the file", which matters for multi-track camera audio.
    let mut wanted = Vec::new();
    for s in &info.streams {
        let sid = info.stream_entity_id(s.index);
        wanted.push(sid);
        tx.ensure(sid, MEDIA_STREAM)?;
        tx.set(sid, attrs::NAME, format!("{} {}", s.kind.as_str(), s.index))?;
        tx.set(sid, CODEC_ATTR, s.codec.as_str())?;
        tx.set(sid, DECODABLE_ATTR, s.decodable)?;
        tx.set(sid, attrs::ORDER, i64::from(s.index))?;
        tx.link(id, sid, EdgeKind::Contains)?;
    }
    // Streams that vanished because the file was replaced in place.
    let stale: Vec<EntityId> = tx
        .graph()
        .children(id)
        .into_iter()
        .filter(|c| !wanted.contains(c))
        .filter(|c| {
            tx.graph()
                .node(*c)
                .map(|n| n.kind.as_str() == MEDIA_STREAM)
                .unwrap_or(false)
        })
        .collect();
    for s in stale {
        tx.delete(s)?;
    }
    Ok(id)
}

fn file_label(info: &MediaFile) -> String {
    info.path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| info.hash.hex()[..12].to_string())
}

/// Record a derivative (proxy, thumbnail, waveform) pointing at its source.
///
/// The `DerivedFrom` edge is deliberate: derivatives go stale when the source
/// changes, and the UCG's existing dirty propagation already knows how to
/// express that.
pub fn register_derivative(
    tx: &mut Tx,
    source: EntityId,
    derivative: &Derivative,
) -> Result<EntityId> {
    let id = derivative.entity_id();
    tx.ensure(id, MEDIA_DERIVATIVE)?;
    tx.set(id, attrs::NAME, derivative.kind.as_str())?;
    tx.set(id, attrs::MEDIA_PATH, derivative.path.display().to_string())?;
    tx.set(id, "derivative_kind", derivative.kind.as_str())?;
    tx.set(id, "recipe", derivative.recipe.as_str())?;
    tx.link(id, source, EdgeKind::DerivedFrom)?;
    Ok(id)
}

/// Bind a clip, take or other consumer to a media file.
pub fn bind_to_media(tx: &mut Tx, consumer: EntityId, media: EntityId) -> Result<()> {
    tx.link(consumer, media, EdgeKind::Binds)?;
    Ok(())
}

/// Mark media online or offline after a presence check, returning whether it changed.
///
/// Offline media is a normal production state, not an error: a card is
/// unmounted, a NAS is unreachable. The project must stay openable and editable
/// with the media absent, which is why this is a flag rather than a failure.
pub fn set_online(tx: &mut Tx, media: EntityId, online: bool) -> Result<bool> {
    let before = tx.graph().node(media).and_then(|n| n.get_bool(ONLINE_ATTR));
    tx.set(media, ONLINE_ATTR, online)?;
    Ok(before != Some(online))
}

/// Read back the stored `MediaFile` without re-probing the file.
pub fn read_media(graph: &Graph, media: EntityId) -> Option<MediaFile> {
    graph.node(media)?.get_as(MEDIA_INFO_ATTR)
}

/// Find media by content hash. This is the relink primitive: given a file found
/// on disk, discover whether the project already knows it.
pub fn find_by_hash(graph: &Graph, hash: &str) -> Option<EntityId> {
    graph
        .of_kind(kinds::ASSET_MEDIA)
        .into_iter()
        .find(|n| n.get_str(HASH_ATTR) == Some(hash))
        .map(|n| n.id)
}

/// Everything that would be affected by replacing this media.
///
/// This is the question the whole UCG exists to answer. Two categories, because
/// they need different handling:
///
/// * **`derived`** — proxies, thumbnails, caches, renders. These are now stale
///   and must be regenerated.
/// * **`bound`** — clips, takes and shots that reference the media. These are
///   not stale; they need **relinking**, and their edit decisions survive.
///
/// Conflating the two is how NLEs lose edits during a conform.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReplacementImpact {
    pub derived: Vec<EntityId>,
    pub bound: Vec<EntityId>,
    pub streams: Vec<EntityId>,
}

impl ReplacementImpact {
    pub fn is_empty(&self) -> bool {
        self.derived.is_empty() && self.bound.is_empty() && self.streams.is_empty()
    }
    pub fn total(&self) -> usize {
        self.derived.len() + self.bound.len() + self.streams.len()
    }
}

/// Compute the impact of replacing `media`.
pub fn media_replacement_impact(graph: &Graph, media: EntityId) -> ReplacementImpact {
    let streams: Vec<EntityId> = graph
        .children(media)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .map(|n| n.kind.as_str() == MEDIA_STREAM)
                .unwrap_or(false)
        })
        .collect();

    // Derived things follow the dependency edges the engine already propagates.
    let mut derived: Vec<EntityId> = graph.dependents(media);
    for s in &streams {
        for d in graph.dependents(*s) {
            if !derived.contains(&d) {
                derived.push(d);
            }
        }
    }
    // Bound consumers are a separate relationship: relink, do not regenerate.
    let bound: Vec<EntityId> = graph.sources(media, EdgeKind::Binds);
    derived.retain(|d| !bound.contains(d));

    ReplacementImpact {
        derived,
        bound,
        streams,
    }
}

/// Streams of a media file that this build can actually decode.
pub fn decodable_streams(graph: &Graph, media: EntityId) -> Vec<EntityId> {
    graph
        .children(media)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .map(|n| {
                    n.kind.as_str() == MEDIA_STREAM && n.get_bool(DECODABLE_ATTR) == Some(true)
                })
                .unwrap_or(false)
        })
        .collect()
}

/// Media the project knows about that is not currently present on disk.
pub fn offline_media(graph: &Graph) -> Vec<EntityId> {
    graph
        .of_kind(kinds::ASSET_MEDIA)
        .into_iter()
        .filter(|n| n.get_bool(ONLINE_ATTR) == Some(false))
        .map(|n| n.id)
        .collect()
}

/// Convenience for the common ingest path: probe a file and register it.
pub fn ingest(tx: &mut Tx, project: EntityId, path: &std::path::Path) -> Result<EntityId> {
    let info = crate::probe::probe(path)?;
    register_media(tx, project, &info)
}

/// Stream kind stored on a stream node, for callers that want to filter.
pub fn stream_kind(graph: &Graph, stream: EntityId) -> Option<StreamKind> {
    let n = graph.node(stream)?;
    let name = n.get_str(attrs::NAME)?;
    Some(match name.split(' ').next()? {
        "video" => StreamKind::Video,
        "audio" => StreamKind::Audio,
        "timecode" => StreamKind::Timecode,
        "subtitle" => StreamKind::Subtitle,
        _ => StreamKind::Data,
    })
}
