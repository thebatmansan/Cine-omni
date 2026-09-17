//! The sequence as something to play: flattened picture segments resolved to
//! files and media offsets.
//!
//! Composition is `cine_timeline::flatten`; this module only resolves each
//! segment's clip to the asset it plays and expresses the media time as an
//! offset from the start of that asset's available range — the coordinate a
//! decoder can turn into a picture without knowing anything about the cut.
//!
//! Both the browser's record monitor (`picture_at`) and the native player read
//! this, so the two surfaces cannot disagree about which picture is where.

use cine_timeline::graph as tl;
use std::path::PathBuf;
use ucg_core::{EntityId, Graph, Rate, RationalTime, TimeRange, Value};
use ucg_schema::attrs;

/// One playable stretch of the sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub record: TimeRange,
    pub clip: Option<EntityId>,
    pub name: String,
    pub track: usize,
    pub asset: Option<EntityId>,
    pub path: Option<PathBuf>,
    /// Media time at `record.start`, measured from the start of the asset's
    /// available range. `None` when the asset declares no range — which picture
    /// it shows then cannot be known, and the entry is not playable.
    pub offset: Option<RationalTime>,
    pub online: Option<bool>,
    /// How the media runs against record time, when the clip is retimed: its
    /// map anchored at the clip's start, with the media there measured like
    /// `offset`, from the start of the asset's available range. `None` is real
    /// time.
    pub timing: Option<cine_timeline::Timing>,
    /// For sound: keep pitch while the speed changes.
    pub keep_pitch: bool,
}

impl Entry {
    /// Why this entry cannot be played, if it cannot.
    ///
    /// A reversed clip is *not* unplayable: the picture it should show is
    /// exact, and the decoder will produce it. It will produce it slowly —
    /// each successive picture is earlier in the file, so every one costs a
    /// walk from the preceding keyframe — and the scheduler reports that as
    /// `Degraded` rather than this pretending the clip is broken.
    pub fn unplayable(&self) -> Option<&'static str> {
        if self.asset.is_none() || self.path.is_none() {
            Some("the clip is bound to no media asset")
        } else if self.offset.is_none() {
            Some("the media declares no available range")
        } else if self.online == Some(false) {
            Some("the media is offline")
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditList {
    pub timeline: EntityId,
    pub name: String,
    /// The sequence's frame rate.
    pub rate: Rate,
    pub end: RationalTime,
    pub entries: Vec<Entry>,
    /// Every audible sound clip, resolved the same way. Mixed by the player.
    pub sound: Vec<Entry>,
    /// Transitions played as cuts.
    pub transitions: usize,
}

impl EditList {
    /// The entry showing at `at`, if any.
    pub fn at(&self, at: RationalTime) -> Option<&Entry> {
        self.entries.iter().find(|e| e.record.contains(at))
    }
}

/// Resolve a timeline into its edit list.
pub fn edit_list(graph: &Graph, timeline: EntityId) -> Result<EditList, String> {
    let composed = tl::export_timeline(graph, timeline).map_err(|e| e.to_string())?;
    let flat = cine_timeline::picture_segments(&composed);
    let tracks: Vec<&cine_timeline::Track> = composed.tracks().collect();
    let rate = composed
        .global_start_time
        .map(|t| t.rate_exact())
        .or_else(|| {
            flat.segments
                .first()
                .map(|s| s.record.start_time.rate_exact())
        })
        .unwrap_or(Rate::F24);
    let resolve = |record: TimeRange,
                   track: usize,
                   item: usize,
                   clip: Option<EntityId>,
                   source_start: RationalTime,
                   timing: Option<&cine_timeline::Timing>,
                   keep_pitch: bool| {
        let name = tracks
            .get(track)
            .and_then(|t| t.children.get(item))
            .map(|i| i.name().to_string())
            .unwrap_or_default();
        let asset = clip.and_then(|c| tl::media_of_clip(graph, c));
        let node = asset.and_then(|a| graph.node(a));
        let available = node
            .and_then(|n| n.get(attrs::AVAILABLE_RANGE))
            .and_then(Value::as_range);
        Entry {
            record,
            clip,
            name,
            track,
            asset,
            path: node
                .and_then(|n| n.get_str(attrs::MEDIA_PATH))
                .map(PathBuf::from),
            offset: available.map(|a| source_start.checked_sub(&a.start_time)),
            online: node.and_then(|n| n.get_bool(cine_media::ucg::ONLINE_ATTR)),
            // Re-anchored to the asset's range, like `offset`; without a range
            // which picture plays cannot be known, as for a real-time clip.
            timing: timing.zip(available).map(|(t, a)| cine_timeline::Timing {
                map: t.map.clone(),
                record: t.record,
                media: t.media.checked_sub(&a.start_time),
            }),
            keep_pitch,
        }
    };
    let sound = cine_timeline::sound_segments(&composed)
        .iter()
        .map(|s| {
            resolve(
                s.record,
                s.track,
                s.item,
                s.clip,
                s.source_start,
                s.timing.as_ref(),
                s.keep_pitch,
            )
        })
        .collect();
    let entries = flat
        .segments
        .iter()
        .map(|s| {
            resolve(
                s.record,
                s.track,
                s.item,
                s.clip,
                s.source_start,
                s.timing.as_ref(),
                false,
            )
        })
        .collect();
    Ok(EditList {
        timeline,
        name: composed.name.clone(),
        rate,
        end: flat.end.unwrap_or_else(|| RationalTime::zero(rate)),
        entries,
        sound,
        transitions: flat.transitions,
    })
}
