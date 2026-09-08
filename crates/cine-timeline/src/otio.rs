//! OpenTimelineIO-conformant data model and composition math.
//!
//! The types mirror the OTIO object model one to one (`Timeline` → `Stack` →
//! `Track` → `Clip | Gap | Transition`) and use
//! [`ucg_core::RationalTime`] / [`ucg_core::TimeRange`] for every time value,
//! so a timeline serializes losslessly (see [`crate::otio_json`]) and drops
//! straight into the graph (see [`crate::graph`]).
//!
//! All arithmetic here is pure: nothing touches the graph.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ucg_core::{RationalTime, TimeRange};

/// Free-form OTIO metadata: a JSON object.
pub type Metadata = IndexMap<String, serde_json::Value>;

/// Metadata key under which the UCG entity id of an item is stored so ids
/// survive a round trip through external OTIO tools.
pub const UCG_ID_KEY: &str = "ucg_id";

// ---------------------------------------------------------------------------
// Timeline / Stack
// ---------------------------------------------------------------------------

/// The root object of an OTIO document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Timeline {
    pub name: String,
    /// Timecode of the first frame of the timeline (e.g. `01:00:00:00`).
    pub global_start_time: Option<RationalTime>,
    /// The root stack; its children are the tracks.
    pub tracks: Stack,
    pub metadata: Metadata,
}

impl Timeline {
    /// An empty timeline with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Timeline {
            name: name.into(),
            ..Default::default()
        }
    }

    /// The tracks of the root stack, in order (nested stacks are skipped).
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.children.iter().filter_map(Composable::as_track)
    }

    /// Mutable view of the tracks of the root stack.
    pub fn tracks_mut(&mut self) -> impl Iterator<Item = &mut Track> {
        self.tracks
            .children
            .iter_mut()
            .filter_map(Composable::as_track_mut)
    }

    /// Video tracks in order.
    pub fn video_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks().filter(|t| t.kind == TrackKind::Video)
    }

    /// Audio tracks in order.
    pub fn audio_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks().filter(|t| t.kind == TrackKind::Audio)
    }

    /// Duration of the longest track.
    pub fn duration(&self) -> RationalTime {
        self.tracks.duration()
    }

    /// Validate every track (see [`Track::validate`]).
    pub fn validate(&self) -> crate::Result<()> {
        self.tracks.validate()
    }
}

/// A child of a [`Stack`]: OTIO stacks may hold tracks and nested stacks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Composable {
    Track(Track),
    Stack(Stack),
}

impl Composable {
    pub fn as_track(&self) -> Option<&Track> {
        match self {
            Composable::Track(t) => Some(t),
            Composable::Stack(_) => None,
        }
    }
    pub fn as_track_mut(&mut self) -> Option<&mut Track> {
        match self {
            Composable::Track(t) => Some(t),
            Composable::Stack(_) => None,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Composable::Track(t) => &t.name,
            Composable::Stack(s) => &s.name,
        }
    }
    /// Duration of the child (tracks: sum of items; stacks: longest child).
    pub fn duration(&self) -> RationalTime {
        match self {
            Composable::Track(t) => t.duration(),
            Composable::Stack(s) => s.duration(),
        }
    }
}

/// A stack composes its children *in parallel* (all start at time zero).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Stack {
    pub name: String,
    pub children: Vec<Composable>,
    pub markers: Vec<Marker>,
    pub metadata: Metadata,
}

impl Stack {
    pub fn new(name: impl Into<String>) -> Self {
        Stack {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Duration of the longest child (children all start at zero).
    pub fn duration(&self) -> RationalTime {
        let mut best = RationalTime::new(0.0, 24.0);
        let mut first = true;
        for c in &self.children {
            let d = c.duration();
            if first {
                best = d;
                first = false;
            } else if d.to_seconds() > best.to_seconds() {
                best = d;
            }
        }
        best
    }

    /// Validate every track recursively.
    pub fn validate(&self) -> crate::Result<()> {
        for c in &self.children {
            match c {
                Composable::Track(t) => t.validate()?,
                Composable::Stack(s) => s.validate()?,
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

/// Video or audio track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    #[default]
    Video,
    Audio,
}

impl TrackKind {
    /// The OTIO spelling (`"Video"` / `"Audio"`).
    pub fn as_otio(&self) -> &'static str {
        match self {
            TrackKind::Video => "Video",
            TrackKind::Audio => "Audio",
        }
    }
    /// Parse the OTIO spelling (case-insensitive).
    pub fn from_otio(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "video" => Some(TrackKind::Video),
            "audio" => Some(TrackKind::Audio),
            _ => None,
        }
    }
}

/// A track composes its children *sequentially*: each clip or gap starts
/// where the previous one ended; transitions overlap the cut between their
/// neighbours and take no time of their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub kind: TrackKind,
    pub children: Vec<Item>,
    pub markers: Vec<Marker>,
    pub enabled: bool,
    pub metadata: Metadata,
}

impl Default for Track {
    fn default() -> Self {
        Track {
            name: String::new(),
            kind: TrackKind::Video,
            children: Vec::new(),
            markers: Vec::new(),
            enabled: true,
            metadata: Metadata::new(),
        }
    }
}

impl Track {
    pub fn new(name: impl Into<String>, kind: TrackKind) -> Self {
        Track {
            name: name.into(),
            kind,
            ..Default::default()
        }
    }

    /// Builder: append an item.
    pub fn with(mut self, item: impl Into<Item>) -> Self {
        self.children.push(item.into());
        self
    }

    /// The rate used for track-time arithmetic: the rate of the first timed
    /// child, falling back to 24.
    pub fn rate(&self) -> f64 {
        self.children
            .iter()
            .find_map(|c| match c {
                Item::Clip(c) => c.trimmed_range().map(|r| r.start_time.rate),
                Item::Gap(g) => Some(g.source_range.start_time.rate),
                Item::Transition(t) => Some(t.in_offset.rate),
            })
            .unwrap_or(24.0)
    }

    /// Sum of the durations of all clips and gaps.
    pub fn duration(&self) -> RationalTime {
        let mut total = RationalTime::new(0.0, self.rate());
        for c in &self.children {
            if !c.is_transition() {
                total = total + c.duration();
            }
        }
        total
    }

    /// Range of the child on the track.
    ///
    /// Clips and gaps start where the preceding clips and gaps end; a
    /// transition starts `in_offset` before the cut it sits on and lasts
    /// `in_offset + out_offset` (exactly OTIO's `range_of_child_at_index`).
    pub fn range_of_child(&self, index: usize) -> Option<TimeRange> {
        let child = self.children.get(index)?;
        let mut start = RationalTime::new(0.0, self.rate());
        for c in &self.children[..index] {
            if !c.is_transition() {
                start = start + c.duration();
            }
        }
        match child {
            Item::Transition(t) => {
                let start = start - t.in_offset;
                Some(TimeRange::new(start, t.duration()))
            }
            Item::Clip(c) => Some(TimeRange::new(start, c.trimmed_range()?.duration)),
            Item::Gap(g) => Some(TimeRange::new(start, g.source_range.duration)),
        }
    }

    /// Ranges of all children (`None` for clips without any range).
    pub fn child_ranges(&self) -> Vec<Option<TimeRange>> {
        (0..self.children.len())
            .map(|i| self.range_of_child(i))
            .collect()
    }

    /// Index of the clip or gap that covers track time `t` (transitions are
    /// never returned; use [`Track::transition_at_time`]).
    pub fn child_at_time(&self, t: RationalTime) -> Option<usize> {
        let mut start = RationalTime::new(0.0, self.rate());
        for (i, c) in self.children.iter().enumerate() {
            if c.is_transition() {
                continue;
            }
            let r = TimeRange::new(start, c.duration());
            if r.contains(t) {
                return Some(i);
            }
            start = r.end_time_exclusive();
        }
        None
    }

    /// Index of the transition whose span covers `t`, if any.
    pub fn transition_at_time(&self, t: RationalTime) -> Option<usize> {
        self.children.iter().enumerate().find_map(|(i, c)| {
            if c.is_transition() {
                self.range_of_child(i).filter(|r| r.contains(t)).map(|_| i)
            } else {
                None
            }
        })
    }

    /// The previous and next children of `index`.
    pub fn neighbors(&self, index: usize) -> (Option<&Item>, Option<&Item>) {
        let prev = if index == 0 {
            None
        } else {
            self.children.get(index - 1)
        };
        let next = self.children.get(index + 1);
        (prev, next)
    }

    /// Media time that plays at track time `t` on the clip at `index`.
    pub fn media_time_at(&self, index: usize, t: RationalTime) -> Option<RationalTime> {
        let range = self.range_of_child(index)?;
        match self.children.get(index)? {
            Item::Clip(c) => c.media_time_at(range.start_time, t),
            _ => None,
        }
    }

    /// Check the structural invariants of the track:
    ///
    /// * every clip has a range (source range or media available range);
    /// * no negative durations or offsets;
    /// * clips stay inside their media's available range;
    /// * transitions sit between two clips/gaps, never next to another
    ///   transition, and do not extend past their neighbours (so no two
    ///   items overlap on the track).
    pub fn validate(&self) -> crate::Result<()> {
        let inv = |msg: String| Err(crate::Error::Invariant(msg));
        for (i, c) in self.children.iter().enumerate() {
            match c {
                Item::Clip(clip) => {
                    let Some(tr) = clip.trimmed_range() else {
                        return inv(format!(
                            "clip {i} `{}` has no source or available range",
                            clip.name
                        ));
                    };
                    if tr.duration.value < 0.0 {
                        return inv(format!(
                            "clip {i} `{}` has negative duration {}",
                            clip.name, tr.duration
                        ));
                    }
                    if let (Some(sr), Some(ar)) = (clip.source_range, clip.available_range()) {
                        let s = sr.start_time.rescaled_to(ar.start_time.rate).value;
                        let e = sr
                            .end_time_exclusive()
                            .rescaled_to(ar.start_time.rate)
                            .value;
                        if s + 1e-9 < ar.start_time.value
                            || e - 1e-9 > ar.end_time_exclusive().value
                        {
                            return inv(format!(
                                "clip {i} `{}` source range {sr} exceeds available range {ar}",
                                clip.name
                            ));
                        }
                    }
                }
                Item::Gap(g) => {
                    if g.source_range.duration.value < 0.0 {
                        return inv(format!(
                            "gap {i} has negative duration {}",
                            g.source_range.duration
                        ));
                    }
                }
                Item::Transition(t) => {
                    if t.in_offset.value < 0.0 || t.out_offset.value < 0.0 {
                        return inv(format!("transition {i} `{}` has a negative offset", t.name));
                    }
                    let (prev, next) = self.neighbors(i);
                    let (Some(prev), Some(next)) = (prev, next) else {
                        return inv(format!(
                            "transition {i} `{}` is not between two items",
                            t.name
                        ));
                    };
                    if prev.is_transition() || next.is_transition() {
                        return inv(format!(
                            "transition {i} `{}` is adjacent to another transition",
                            t.name
                        ));
                    }
                    if t.in_offset.to_seconds() > prev.duration().to_seconds() + 1e-9 {
                        return inv(format!(
                            "transition {i} `{}` in_offset exceeds the preceding item",
                            t.name
                        ));
                    }
                    if t.out_offset.to_seconds() > next.duration().to_seconds() + 1e-9 {
                        return inv(format!(
                            "transition {i} `{}` out_offset exceeds the following item",
                            t.name
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// A child of a [`Track`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Item {
    Clip(Clip),
    Gap(Gap),
    Transition(Transition),
}

impl Item {
    pub fn is_transition(&self) -> bool {
        matches!(self, Item::Transition(_))
    }
    pub fn is_clip(&self) -> bool {
        matches!(self, Item::Clip(_))
    }
    pub fn is_gap(&self) -> bool {
        matches!(self, Item::Gap(_))
    }
    pub fn as_clip(&self) -> Option<&Clip> {
        match self {
            Item::Clip(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_clip_mut(&mut self) -> Option<&mut Clip> {
        match self {
            Item::Clip(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_gap(&self) -> Option<&Gap> {
        match self {
            Item::Gap(g) => Some(g),
            _ => None,
        }
    }
    pub fn as_transition(&self) -> Option<&Transition> {
        match self {
            Item::Transition(t) => Some(t),
            _ => None,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Item::Clip(c) => &c.name,
            Item::Gap(g) => &g.name,
            Item::Transition(t) => &t.name,
        }
    }
    pub fn metadata(&self) -> &Metadata {
        match self {
            Item::Clip(c) => &c.metadata,
            Item::Gap(g) => &g.metadata,
            Item::Transition(t) => &t.metadata,
        }
    }
    pub fn metadata_mut(&mut self) -> &mut Metadata {
        match self {
            Item::Clip(c) => &mut c.metadata,
            Item::Gap(g) => &mut g.metadata,
            Item::Transition(t) => &mut t.metadata,
        }
    }

    /// Duration: trimmed duration for clips and gaps, `in + out` for
    /// transitions (which nevertheless take no time on the track).
    pub fn duration(&self) -> RationalTime {
        match self {
            Item::Clip(c) => c.duration(),
            Item::Gap(g) => g.source_range.duration,
            Item::Transition(t) => t.duration(),
        }
    }

    /// The source range of a clip or gap (`None` for transitions or clips
    /// that rely on their media's available range).
    pub fn source_range(&self) -> Option<TimeRange> {
        match self {
            Item::Clip(c) => c.source_range,
            Item::Gap(g) => Some(g.source_range),
            Item::Transition(_) => None,
        }
    }

    /// Set the source range of a clip or gap; ignored for transitions.
    pub fn set_source_range(&mut self, range: TimeRange) {
        match self {
            Item::Clip(c) => c.source_range = Some(range),
            Item::Gap(g) => g.source_range = range,
            Item::Transition(_) => {}
        }
    }

    /// Effective (trimmed) range of a clip or gap.
    pub fn trimmed_range(&self) -> Option<TimeRange> {
        match self {
            Item::Clip(c) => c.trimmed_range(),
            Item::Gap(g) => Some(g.source_range),
            Item::Transition(_) => None,
        }
    }

    /// The `ucg_id` stored in the item's metadata, if any.
    pub fn ucg_id(&self) -> Option<ucg_core::EntityId> {
        self.metadata()
            .get(UCG_ID_KEY)
            .and_then(|v| v.as_str())
            .and_then(ucg_core::EntityId::parse)
    }

    /// Store a `ucg_id` in the item's metadata.
    pub fn set_ucg_id(&mut self, id: ucg_core::EntityId) {
        self.metadata_mut().insert(
            UCG_ID_KEY.to_string(),
            serde_json::Value::String(id.to_string()),
        );
    }
}

impl From<Clip> for Item {
    fn from(c: Clip) -> Self {
        Item::Clip(c)
    }
}
impl From<Gap> for Item {
    fn from(g: Gap) -> Self {
        Item::Gap(g)
    }
}
impl From<Transition> for Item {
    fn from(t: Transition) -> Self {
        Item::Transition(t)
    }
}

/// The key OTIO uses for a clip's primary media reference.
pub const DEFAULT_MEDIA_KEY: &str = "DEFAULT_MEDIA";

/// A clip plays a range of a media reference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    /// The reference currently being played.
    ///
    /// A clip may carry several references — a camera original, a proxy, a
    /// transcode — of which exactly one is active. This field is the active
    /// one, because that is what nearly every consumer wants.
    pub media_reference: MediaReference,
    /// Which key in the reference map [`Clip::media_reference`] came from.
    ///
    /// Preserved because the key carries meaning: a clip playing its `proxy`
    /// must round-trip still saying `proxy`. Relabelling it `DEFAULT_MEDIA`
    /// would assert that the proxy *is* the camera original.
    pub active_media_reference_key: String,
    /// Every other reference the clip carries, by key.
    ///
    /// Kept verbatim so a round-trip cannot silently discard the camera
    /// original just because the editor happened to be cutting on proxies.
    /// A proxy is never the authoritative source.
    pub alternate_media_references: BTreeMap<String, MediaReference>,
    /// Range inside the media that is played. `None` means the whole
    /// available range.
    pub source_range: Option<TimeRange>,
    /// Markers in the clip's media time coordinates.
    pub markers: Vec<Marker>,
    pub metadata: Metadata,
    pub enabled: bool,
}

impl Default for Clip {
    fn default() -> Self {
        Clip {
            name: String::new(),
            media_reference: MediaReference::Missing,
            active_media_reference_key: DEFAULT_MEDIA_KEY.to_string(),
            alternate_media_references: BTreeMap::new(),
            source_range: None,
            markers: Vec::new(),
            metadata: Metadata::new(),
            enabled: true,
        }
    }
}

impl Clip {
    /// Every reference the clip carries, including the active one.
    pub fn all_media_references(&self) -> BTreeMap<String, MediaReference> {
        let mut all = self.alternate_media_references.clone();
        all.insert(
            self.active_media_reference_key.clone(),
            self.media_reference.clone(),
        );
        all
    }

    /// Look up one reference by key, active or not.
    pub fn media_reference_named(&self, key: &str) -> Option<&MediaReference> {
        if key == self.active_media_reference_key {
            Some(&self.media_reference)
        } else {
            self.alternate_media_references.get(key)
        }
    }

    /// Switch which reference plays — conforming from proxy to camera original.
    ///
    /// Returns `false` if the key is unknown, leaving the clip untouched: a
    /// failed conform must not silently leave a clip pointing at nothing.
    pub fn set_active_media_reference(&mut self, key: &str) -> bool {
        if key == self.active_media_reference_key {
            return true;
        }
        let Some(next) = self.alternate_media_references.remove(key) else {
            return false;
        };
        let previous = std::mem::replace(&mut self.media_reference, next);
        let previous_key = std::mem::replace(&mut self.active_media_reference_key, key.to_string());
        self.alternate_media_references
            .insert(previous_key, previous);
        true
    }

    /// A clip playing `source_range` of the media at `target_url`.
    pub fn new(
        name: impl Into<String>,
        media_reference: MediaReference,
        source_range: Option<TimeRange>,
    ) -> Self {
        Clip {
            name: name.into(),
            media_reference,
            source_range,
            ..Default::default()
        }
    }

    /// Convenience: an external-reference clip with explicit available and
    /// source ranges.
    pub fn external(
        name: impl Into<String>,
        target_url: impl Into<String>,
        available_range: Option<TimeRange>,
        source_range: Option<TimeRange>,
    ) -> Self {
        Clip::new(
            name,
            MediaReference::External {
                target_url: target_url.into(),
                available_range,
            },
            source_range,
        )
    }

    /// The range of media that exists, if the reference knows it.
    pub fn available_range(&self) -> Option<TimeRange> {
        self.media_reference.available_range()
    }

    /// The range actually played: `source_range`, else the available range.
    pub fn trimmed_range(&self) -> Option<TimeRange> {
        self.source_range.or_else(|| self.available_range())
    }

    /// Trimmed duration (zero when the clip has no range at all).
    pub fn duration(&self) -> RationalTime {
        self.trimmed_range()
            .map(|r| r.duration)
            .unwrap_or(RationalTime::new(0.0, 24.0))
    }

    /// The media time that plays at `track_time` when the clip starts at
    /// `track_start` on its track; `None` outside the clip.
    pub fn media_time_at(
        &self,
        track_start: RationalTime,
        track_time: RationalTime,
    ) -> Option<RationalTime> {
        let tr = self.trimmed_range()?;
        let offset = track_time - track_start;
        if offset.value < 0.0 || offset.to_seconds() >= tr.duration.to_seconds() {
            return None;
        }
        Some(tr.start_time + offset)
    }

    /// The `ucg_id` stored in metadata, if any.
    pub fn ucg_id(&self) -> Option<ucg_core::EntityId> {
        self.metadata
            .get(UCG_ID_KEY)
            .and_then(|v| v.as_str())
            .and_then(ucg_core::EntityId::parse)
    }
}

/// Empty time on a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Gap {
    pub source_range: TimeRange,
    pub name: String,
    pub metadata: Metadata,
}

impl Gap {
    /// A gap of `duration` (its source range starts at zero).
    pub fn new(duration: RationalTime) -> Self {
        Gap {
            source_range: TimeRange::new(RationalTime::new(0.0, duration.rate), duration),
            ..Default::default()
        }
    }
    /// A gap with an explicit source range.
    pub fn with_range(source_range: TimeRange) -> Self {
        Gap {
            source_range,
            ..Default::default()
        }
    }
}

/// Transition type: SMPTE dissolve or a vendor-specific name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TransitionType {
    #[default]
    SmpteDissolve,
    Custom(String),
}

impl TransitionType {
    /// OTIO spelling (`"SMPTE_Dissolve"` or the custom name).
    pub fn as_otio(&self) -> &str {
        match self {
            TransitionType::SmpteDissolve => "SMPTE_Dissolve",
            TransitionType::Custom(s) => s,
        }
    }
    pub fn from_otio(s: &str) -> Self {
        if s == "SMPTE_Dissolve" {
            TransitionType::SmpteDissolve
        } else {
            TransitionType::Custom(s.to_string())
        }
    }
}

/// A transition overlapping the cut between its neighbours: `in_offset`
/// before the cut (over the preceding item) and `out_offset` after it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Transition {
    pub name: String,
    pub transition_type: TransitionType,
    pub in_offset: RationalTime,
    pub out_offset: RationalTime,
    pub metadata: Metadata,
}

impl Transition {
    /// A dissolve of `in_offset + out_offset` centred on the cut.
    pub fn dissolve(
        name: impl Into<String>,
        in_offset: RationalTime,
        out_offset: RationalTime,
    ) -> Self {
        Transition {
            name: name.into(),
            transition_type: TransitionType::SmpteDissolve,
            in_offset,
            out_offset,
            ..Default::default()
        }
    }
    /// `in_offset + out_offset`.
    pub fn duration(&self) -> RationalTime {
        self.in_offset + self.out_offset
    }
}

// ---------------------------------------------------------------------------
// Media references
// ---------------------------------------------------------------------------

/// What a clip plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum MediaReference {
    /// A file (or URL) on disk.
    External {
        target_url: String,
        available_range: Option<TimeRange>,
    },
    /// Media that is not (yet) online.
    #[default]
    Missing,
    /// Synthesized media (bars, tone, black ...).
    Generator {
        generator_kind: String,
        parameters: Metadata,
        available_range: Option<TimeRange>,
    },
}

impl MediaReference {
    pub fn external(target_url: impl Into<String>, available_range: Option<TimeRange>) -> Self {
        MediaReference::External {
            target_url: target_url.into(),
            available_range,
        }
    }
    pub fn available_range(&self) -> Option<TimeRange> {
        match self {
            MediaReference::External {
                available_range, ..
            } => *available_range,
            MediaReference::Missing => None,
            MediaReference::Generator {
                available_range, ..
            } => *available_range,
        }
    }
    pub fn target_url(&self) -> Option<&str> {
        match self {
            MediaReference::External { target_url, .. } => Some(target_url),
            _ => None,
        }
    }
    /// Whether two references point at the same media (ignoring ranges).
    pub fn same_media(&self, other: &MediaReference) -> bool {
        match (self, other) {
            (
                MediaReference::External { target_url: a, .. },
                MediaReference::External { target_url: b, .. },
            ) => a == b,
            (
                MediaReference::Generator {
                    generator_kind: a,
                    parameters: pa,
                    ..
                },
                MediaReference::Generator {
                    generator_kind: b,
                    parameters: pb,
                    ..
                },
            ) => a == b && pa == pb,
            (MediaReference::Missing, MediaReference::Missing) => false,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Markers
// ---------------------------------------------------------------------------

/// A named, colored range on an item, track or stack (in the owner's time
/// coordinates: media time for clips, track time for tracks).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Marker {
    pub name: String,
    pub marked_range: TimeRange,
    /// OTIO color name (`"RED"`, `"GREEN"`, `"PURPLE"` ...).
    pub color: String,
    pub comment: String,
    pub metadata: Metadata,
}

impl Marker {
    pub fn new(name: impl Into<String>, marked_range: TimeRange, color: impl Into<String>) -> Self {
        Marker {
            name: name.into(),
            marked_range,
            color: color.into(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(name: &str, start: i64, dur: i64, avail: i64) -> Clip {
        Clip::external(
            name,
            format!("file:///{name}.mov"),
            Some(TimeRange::from_frames(0, avail, 24.0)),
            Some(TimeRange::from_frames(start, dur, 24.0)),
        )
    }

    fn track() -> Track {
        // A(48) | dissolve 12/12 | B(72) | gap(24) | C(24)
        Track::new("V1", TrackKind::Video)
            .with(clip("A", 10, 48, 200))
            .with(Transition::dissolve(
                "x",
                RationalTime::from_frames(12, 24.0),
                RationalTime::from_frames(12, 24.0),
            ))
            .with(clip("B", 100, 72, 300))
            .with(Gap::new(RationalTime::from_frames(24, 24.0)))
            .with(clip("C", 0, 24, 24))
    }

    #[test]
    fn duration_ignores_transitions() {
        let t = track();
        assert_eq!(
            t.duration(),
            RationalTime::from_frames(48 + 72 + 24 + 24, 24.0)
        );
        assert!(t.validate().is_ok());
    }

    #[test]
    fn range_of_child_is_frame_exact() {
        let t = track();
        assert_eq!(
            t.range_of_child(0),
            Some(TimeRange::from_frames(0, 48, 24.0))
        );
        // transition: starts in_offset before the cut, lasts in+out
        assert_eq!(
            t.range_of_child(1),
            Some(TimeRange::from_frames(36, 24, 24.0))
        );
        assert_eq!(
            t.range_of_child(2),
            Some(TimeRange::from_frames(48, 72, 24.0))
        );
        assert_eq!(
            t.range_of_child(3),
            Some(TimeRange::from_frames(120, 24, 24.0))
        );
        assert_eq!(
            t.range_of_child(4),
            Some(TimeRange::from_frames(144, 24, 24.0))
        );
        assert_eq!(t.range_of_child(5), None);
    }

    #[test]
    fn child_at_time_and_neighbors() {
        let t = track();
        assert_eq!(t.child_at_time(RationalTime::from_frames(0, 24.0)), Some(0));
        assert_eq!(
            t.child_at_time(RationalTime::from_frames(47, 24.0)),
            Some(0)
        );
        assert_eq!(
            t.child_at_time(RationalTime::from_frames(48, 24.0)),
            Some(2)
        );
        assert_eq!(
            t.child_at_time(RationalTime::from_frames(130, 24.0)),
            Some(3)
        );
        assert_eq!(t.child_at_time(RationalTime::from_frames(168, 24.0)), None);
        assert_eq!(
            t.transition_at_time(RationalTime::from_frames(40, 24.0)),
            Some(1)
        );
        assert_eq!(
            t.transition_at_time(RationalTime::from_frames(60, 24.0)),
            None
        );
        let (p, n) = t.neighbors(1);
        assert_eq!(p.map(Item::name), Some("A"));
        assert_eq!(n.map(Item::name), Some("B"));
        assert!(t.neighbors(0).0.is_none());
    }

    #[test]
    fn media_time_mapping() {
        let t = track();
        // B starts at track frame 48 and plays media from 100
        assert_eq!(
            t.media_time_at(2, RationalTime::from_frames(58, 24.0)),
            Some(RationalTime::from_frames(110, 24.0))
        );
        assert_eq!(
            t.media_time_at(2, RationalTime::from_frames(47, 24.0)),
            None
        );
        assert_eq!(
            t.media_time_at(2, RationalTime::from_frames(120, 24.0)),
            None
        );
    }

    #[test]
    fn clip_ranges_fall_back_to_available() {
        let c = Clip::external(
            "x",
            "file:///x.mov",
            Some(TimeRange::from_frames(5, 30, 24.0)),
            None,
        );
        assert_eq!(c.trimmed_range(), Some(TimeRange::from_frames(5, 30, 24.0)));
        assert_eq!(c.duration(), RationalTime::from_frames(30, 24.0));
        let m = Clip::new("m", MediaReference::Missing, None);
        assert_eq!(m.trimmed_range(), None);
        assert!(Track::new("v", TrackKind::Video)
            .with(m)
            .validate()
            .is_err());
    }

    #[test]
    fn validate_rejects_bad_transitions_and_media_bounds() {
        let mut t = track();
        // transition in_offset larger than preceding clip
        if let Item::Transition(tr) = &mut t.children[1] {
            tr.in_offset = RationalTime::from_frames(49, 24.0);
        }
        assert!(t.validate().is_err());
        let bad = Track::new("v", TrackKind::Video).with(clip("A", 190, 48, 200));
        assert!(bad.validate().is_err());
        let lead = Track::new("v", TrackKind::Video)
            .with(Transition::dissolve(
                "t",
                RationalTime::from_frames(1, 24.0),
                RationalTime::from_frames(1, 24.0),
            ))
            .with(clip("A", 0, 10, 20));
        assert!(lead.validate().is_err());
    }

    #[test]
    fn timeline_duration_is_longest_track() {
        let mut tl = Timeline::new("t");
        tl.tracks.children.push(Composable::Track(track()));
        tl.tracks.children.push(Composable::Track(
            Track::new("A1", TrackKind::Audio).with(clip("a", 0, 500, 1000)),
        ));
        assert_eq!(tl.duration(), RationalTime::from_frames(500, 24.0));
        assert_eq!(tl.video_tracks().count(), 1);
        assert_eq!(tl.audio_tracks().count(), 1);
    }
}
