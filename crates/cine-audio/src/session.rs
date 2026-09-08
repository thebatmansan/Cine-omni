//! DAW session model: tracks, clips, buses, stems, sends, inserts and
//! parameter automation. Pure data; rendering lives in [`crate::mix`] and
//! graph persistence in [`crate::ucg`].

use crate::buffer::AudioBuffer;
use crate::dsp::{BiquadKind, CompressorParams, Fade, GateParams, LimiterParams};
use crate::pan::PanLaw;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use ucg_core::{EntityId, RationalTime, TimeRange};

/// Identifier of a track (its graph entity id).
pub type TrackId = EntityId;
/// Identifier of a bus (its graph entity id).
pub type BusId = EntityId;
/// Identifier of a clip (its graph entity id).
pub type ClipId = EntityId;

/// Delivery stem a track or bus feeds (DME + foley/ambience + custom).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stem {
    Dialogue,
    Music,
    Effects,
    Foley,
    Ambience,
    Custom(String),
}

impl Stem {
    /// The three classic DME stems.
    pub const DME: [Stem; 3] = [Stem::Dialogue, Stem::Music, Stem::Effects];

    /// Stable string key (`dialogue`, `custom:<name>` …) used in graph attrs.
    pub fn key(&self) -> String {
        match self {
            Stem::Dialogue => "dialogue".into(),
            Stem::Music => "music".into(),
            Stem::Effects => "effects".into(),
            Stem::Foley => "foley".into(),
            Stem::Ambience => "ambience".into(),
            Stem::Custom(n) => format!("custom:{n}"),
        }
    }

    /// Parse a key produced by [`Stem::key`] (unknown strings become custom).
    pub fn parse(key: &str) -> Stem {
        match key {
            "dialogue" | "dx" => Stem::Dialogue,
            "music" | "mx" => Stem::Music,
            "effects" | "fx" | "sfx" => Stem::Effects,
            "foley" => Stem::Foley,
            "ambience" | "amb" | "bg" => Stem::Ambience,
            other => Stem::Custom(other.strip_prefix("custom:").unwrap_or(other).to_string()),
        }
    }

    /// Two-letter industry abbreviation (`DX`, `MX`, `FX` …).
    pub fn abbreviation(&self) -> String {
        match self {
            Stem::Dialogue => "DX".into(),
            Stem::Music => "MX".into(),
            Stem::Effects => "FX".into(),
            Stem::Foley => "FOL".into(),
            Stem::Ambience => "BG".into(),
            Stem::Custom(n) => n.to_uppercase(),
        }
    }

    /// Display label.
    pub fn label(&self) -> String {
        match self {
            Stem::Custom(n) => n.clone(),
            other => {
                let k = other.key();
                let mut c = k.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => k,
                }
            }
        }
    }
}

/// What a track carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    #[default]
    Audio,
    /// Effects return / sub-mix input (no clips of its own).
    Aux,
    /// Organisational folder (no audio).
    Folder,
}

/// An auxiliary send from a track to a bus.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Send {
    pub bus: BusId,
    pub level_db: f32,
    /// Tap before the track fader (and automation) when true.
    pub pre_fader: bool,
}

/// An insert processor on a track or bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Insert {
    Biquad {
        kind: BiquadKind,
        freq_hz: f32,
        q: f32,
        gain_db: f32,
    },
    Compressor(CompressorParams),
    Gate(GateParams),
    Limiter(LimiterParams),
    Gain {
        db: f32,
    },
}

impl Insert {
    /// Convenience: a filter insert.
    pub fn filter(kind: BiquadKind, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        Insert::Biquad {
            kind,
            freq_hz,
            q,
            gain_db,
        }
    }
}

/// Static pan position of a track.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanState {
    /// Azimuth in degrees (positive = left).
    pub azimuth_deg: f32,
    /// Elevation in degrees.
    pub elevation_deg: f32,
    /// Angular spread / divergence in degrees.
    pub spread_deg: f32,
    /// Pan law used for mono sources rendered to a stereo layout.
    pub law: PanLaw,
    /// Optional object position (x forward, y left, z up); overrides the
    /// angles and adds distance attenuation when set.
    pub position: Option<[f32; 3]>,
}

impl Default for PanState {
    fn default() -> Self {
        PanState {
            azimuth_deg: 0.0,
            elevation_deg: 0.0,
            spread_deg: 0.0,
            law: PanLaw::default(),
            position: None,
        }
    }
}

impl PanState {
    /// Centered.
    pub fn center() -> Self {
        Self::default()
    }
    /// A horizontal pan at the given azimuth.
    pub fn azimuth(deg: f32) -> Self {
        PanState {
            azimuth_deg: deg,
            ..Default::default()
        }
    }
    /// Stereo-style pan position `-1 (left) ..= 1 (right)` mapped to ±90°.
    pub fn stereo(pan: f32) -> Self {
        PanState {
            azimuth_deg: -pan.clamp(-1.0, 1.0) * 90.0,
            ..Default::default()
        }
    }
    /// Equivalent stereo pan position (`-1` = left).
    pub fn stereo_position(&self) -> f32 {
        (-self.azimuth_deg / 90.0).clamp(-1.0, 1.0)
    }
}

/// Automatable parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Param {
    /// Fader in dB.
    Volume,
    /// Azimuth in degrees.
    Pan,
    /// Elevation in degrees.
    Elevation,
    /// Spread in degrees.
    Spread,
    /// `> 0.5` = muted.
    Mute,
    /// Send level in dB for send index `n`.
    Send(usize),
}

/// How values between automation points are computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Step,
    #[default]
    Linear,
}

/// One automation lane: sorted `(time_s, value)` points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Lane {
    pub points: Vec<(f64, f64)>,
    pub interpolation: Interpolation,
}

impl Lane {
    /// Create a lane from points (sorted by time).
    pub fn new(mut points: Vec<(f64, f64)>, interpolation: Interpolation) -> Self {
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        Lane {
            points,
            interpolation,
        }
    }

    /// Insert or replace the point at `time_s`.
    pub fn set_point(&mut self, time_s: f64, value: f64) {
        match self.points.binary_search_by(|p| {
            p.0.partial_cmp(&time_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(i) => self.points[i].1 = value,
            Err(i) => self.points.insert(i, (time_s, value)),
        }
    }

    /// Value at `time_s` (held before the first / after the last point).
    pub fn value_at(&self, time_s: f64) -> Option<f64> {
        let pts = &self.points;
        if pts.is_empty() {
            return None;
        }
        if time_s <= pts[0].0 {
            return Some(pts[0].1);
        }
        let last = pts[pts.len() - 1];
        if time_s >= last.0 {
            return Some(last.1);
        }
        let i = pts.partition_point(|p| p.0 <= time_s);
        let (t0, v0) = pts[i - 1];
        let (t1, v1) = pts[i];
        Some(match self.interpolation {
            Interpolation::Step => v0,
            Interpolation::Linear => {
                if t1 - t0 <= 0.0 {
                    v1
                } else {
                    v0 + (v1 - v0) * ((time_s - t0) / (t1 - t0))
                }
            }
        })
    }
}

mod lanes_serde {
    use super::{Lane, Param};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;

    pub fn serialize<S: Serializer>(m: &HashMap<Param, Lane>, s: S) -> Result<S::Ok, S::Error> {
        let mut v: Vec<(&Param, &Lane)> = m.iter().collect();
        v.sort_by_key(|(p, _)| format!("{p:?}"));
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<HashMap<Param, Lane>, D::Error> {
        let v: Vec<(Param, Lane)> = Vec::deserialize(d)?;
        Ok(v.into_iter().collect())
    }
}

/// Automation lanes of a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Automation {
    #[serde(with = "lanes_serde")]
    pub lanes: HashMap<Param, Lane>,
}

impl Automation {
    /// No automation.
    pub fn new() -> Self {
        Self::default()
    }
    /// Whether any lane has points.
    pub fn is_empty(&self) -> bool {
        self.lanes.values().all(|l| l.points.is_empty())
    }
    /// Add/replace a lane.
    pub fn set_lane(&mut self, param: Param, lane: Lane) -> &mut Self {
        self.lanes.insert(param, lane);
        self
    }
    /// Value of `param` at `time_s`, if automated.
    pub fn value_at(&self, param: Param, time_s: f64) -> Option<f64> {
        self.lanes.get(&param).and_then(|l| l.value_at(time_s))
    }
}

/// Where a clip's samples come from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "ClipSourceRepr", into = "ClipSourceRepr")]
pub enum ClipSource {
    /// In-memory audio.
    Buffer(Arc<AudioBuffer>),
    /// A media entity resolved through [`Session::media`].
    Entity(EntityId),
    /// A placeholder with no audio (e.g. a script-derived dialogue slot).
    Silence,
}

impl PartialEq for ClipSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ClipSource::Buffer(a), ClipSource::Buffer(b)) => Arc::ptr_eq(a, b) || **a == **b,
            (ClipSource::Entity(a), ClipSource::Entity(b)) => a == b,
            (ClipSource::Silence, ClipSource::Silence) => true,
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ClipSourceRepr {
    Buffer { buffer: AudioBuffer },
    Entity { id: EntityId },
    Silence,
}

impl From<ClipSourceRepr> for ClipSource {
    fn from(r: ClipSourceRepr) -> Self {
        match r {
            ClipSourceRepr::Buffer { buffer } => ClipSource::Buffer(Arc::new(buffer)),
            ClipSourceRepr::Entity { id } => ClipSource::Entity(id),
            ClipSourceRepr::Silence => ClipSource::Silence,
        }
    }
}

impl From<ClipSource> for ClipSourceRepr {
    fn from(s: ClipSource) -> Self {
        match s {
            ClipSource::Buffer(b) => ClipSourceRepr::Buffer {
                buffer: (*b).clone(),
            },
            ClipSource::Entity(id) => ClipSourceRepr::Entity { id },
            ClipSource::Silence => ClipSourceRepr::Silence,
        }
    }
}

/// A region of audio placed on a track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    pub source: ClipSource,
    /// Position on the session timeline (at the session rate).
    pub range: TimeRange,
    /// Offset into the source in seconds.
    pub source_offset_s: f64,
    pub gain_db: f32,
    pub fade_in: Fade,
    pub fade_out: Fade,
}

impl Clip {
    /// A clip playing `buffer` starting at `start_s` for the buffer's length.
    pub fn from_buffer(
        name: impl Into<String>,
        buffer: Arc<AudioBuffer>,
        start_s: f64,
        rate: f64,
    ) -> Self {
        let dur = buffer.duration_s();
        Clip {
            id: EntityId::new(),
            name: name.into(),
            source: ClipSource::Buffer(buffer),
            range: TimeRange::new(
                RationalTime::from_seconds(start_s, rate),
                RationalTime::from_seconds(dur, rate),
            ),
            source_offset_s: 0.0,
            gain_db: 0.0,
            fade_in: Fade::none(),
            fade_out: Fade::none(),
        }
    }

    /// A silent placeholder clip.
    pub fn placeholder(name: impl Into<String>, start_s: f64, duration_s: f64, rate: f64) -> Self {
        Clip {
            id: EntityId::new(),
            name: name.into(),
            source: ClipSource::Silence,
            range: TimeRange::new(
                RationalTime::from_seconds(start_s, rate),
                RationalTime::from_seconds(duration_s, rate),
            ),
            source_offset_s: 0.0,
            gain_db: 0.0,
            fade_in: Fade::none(),
            fade_out: Fade::none(),
        }
    }

    /// Start on the timeline in seconds.
    pub fn start_s(&self) -> f64 {
        self.range.start_time.to_seconds()
    }
    /// Duration in seconds.
    pub fn duration_s(&self) -> f64 {
        self.range.duration.to_seconds()
    }
    /// Exclusive end in seconds.
    pub fn end_s(&self) -> f64 {
        self.start_s() + self.duration_s()
    }
}

/// A track: clips, processing and routing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub clips: Vec<Clip>,
    pub inserts: Vec<Insert>,
    pub sends: Vec<Send>,
    pub pan: PanState,
    pub volume_db: f32,
    pub mute: bool,
    pub solo: bool,
    pub automation: Automation,
    /// Bus the track feeds (normally the master or a stem bus).
    pub output: BusId,
    pub stem: Option<Stem>,
}

impl Track {
    /// An empty audio track routed to `output`.
    pub fn new(name: impl Into<String>, output: BusId) -> Self {
        Track {
            id: EntityId::new(),
            name: name.into(),
            kind: TrackKind::Audio,
            clips: Vec::new(),
            inserts: Vec::new(),
            sends: Vec::new(),
            pan: PanState::default(),
            volume_db: 0.0,
            mute: false,
            solo: false,
            automation: Automation::default(),
            output,
            stem: None,
        }
    }

    /// Builder: assign a stem.
    pub fn with_stem(mut self, stem: Stem) -> Self {
        self.stem = Some(stem);
        self
    }

    /// Builder: add a clip.
    pub fn with_clip(mut self, clip: Clip) -> Self {
        self.clips.push(clip);
        self
    }

    /// End of the last clip in seconds (0 when empty).
    pub fn end_s(&self) -> f64 {
        self.clips.iter().map(Clip::end_s).fold(0.0, f64::max)
    }
}

/// A summing bus with its own processing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    pub id: BusId,
    pub name: String,
    pub inserts: Vec<Insert>,
    pub volume_db: f32,
    pub mute: bool,
    /// Bus this bus feeds (`None` = the master).
    pub output: Option<BusId>,
    /// Stem this bus delivers (tracks routed here inherit it).
    pub stem: Option<Stem>,
}

impl Bus {
    /// A unity bus routed to the master.
    pub fn new(name: impl Into<String>) -> Self {
        Bus {
            id: EntityId::new(),
            name: name.into(),
            inserts: Vec::new(),
            volume_db: 0.0,
            mute: false,
            output: None,
            stem: None,
        }
    }

    /// A stem bus.
    pub fn stem(stem: Stem) -> Self {
        let mut b = Bus::new(stem.label());
        b.stem = Some(stem);
        b
    }

    /// The default master bus: a brick-wall limiter at −1 dBTP.
    pub fn master() -> Self {
        let mut b = Bus::new("Master");
        b.inserts.push(Insert::Limiter(LimiterParams::default()));
        b
    }
}

/// A complete mixing session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// Audio sample rate.
    pub sample_rate: u32,
    /// Timeline rate (frames per second) used for clip ranges.
    pub rate: f64,
    pub tracks: Vec<Track>,
    pub buses: Vec<Bus>,
    /// Stems to deliver (empty = derive from tracks and buses).
    pub stems: Vec<Stem>,
    pub master: Bus,
    /// Media library for [`ClipSource::Entity`] clips (not serialized).
    #[serde(skip)]
    pub media: HashMap<EntityId, Arc<AudioBuffer>>,
}

impl Session {
    /// A session with a master bus and the three DME stem buses.
    pub fn new(sample_rate: u32, rate: f64) -> Self {
        Session {
            sample_rate,
            rate,
            tracks: Vec::new(),
            buses: Stem::DME.iter().cloned().map(Bus::stem).collect(),
            stems: Stem::DME.to_vec(),
            master: Bus::master(),
            media: HashMap::new(),
        }
    }

    /// A minimal session with only a master bus.
    pub fn empty(sample_rate: u32, rate: f64) -> Self {
        Session {
            sample_rate,
            rate,
            tracks: Vec::new(),
            buses: Vec::new(),
            stems: Vec::new(),
            master: Bus::master(),
            media: HashMap::new(),
        }
    }

    /// The master bus id.
    pub fn master_id(&self) -> BusId {
        self.master.id
    }

    /// Find a bus (not the master) by id.
    pub fn bus(&self, id: BusId) -> Option<&Bus> {
        self.buses.iter().find(|b| b.id == id)
    }

    /// Find the bus delivering `stem`.
    pub fn stem_bus(&self, stem: &Stem) -> Option<&Bus> {
        self.buses.iter().find(|b| b.stem.as_ref() == Some(stem))
    }

    /// Find a track by id.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Mutable track by id.
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Add a track routed to its stem bus when one exists (else the master).
    pub fn add_track(&mut self, mut track: Track) -> TrackId {
        if let Some(stem) = &track.stem {
            if let Some(b) = self.stem_bus(stem) {
                track.output = b.id;
            }
        }
        if track.output != self.master.id && self.bus(track.output).is_none() {
            track.output = self.master.id;
        }
        let id = track.id;
        self.tracks.push(track);
        id
    }

    /// The stem a track effectively delivers to (its own, else its bus's).
    pub fn effective_stem(&self, track: &Track) -> Option<Stem> {
        if let Some(s) = &track.stem {
            return Some(s.clone());
        }
        let mut bus = self.bus(track.output);
        let mut guard = 0;
        while let Some(b) = bus {
            if let Some(s) = &b.stem {
                return Some(s.clone());
            }
            bus = b.output.and_then(|o| self.bus(o));
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        None
    }

    /// Stems present in the session (declared ones first, then discovered).
    pub fn all_stems(&self) -> Vec<Stem> {
        let mut out = self.stems.clone();
        for t in &self.tracks {
            if let Some(s) = self.effective_stem(t) {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        for b in &self.buses {
            if let Some(s) = &b.stem {
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
        }
        out
    }

    /// Whether any track is soloed.
    pub fn has_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
    }

    /// End of the last clip across all tracks (seconds).
    pub fn end_s(&self) -> f64 {
        self.tracks.iter().map(Track::end_s).fold(0.0, f64::max)
    }

    /// Full extent of the session as a time range at the session rate.
    pub fn full_range(&self) -> TimeRange {
        TimeRange::new(
            RationalTime::from_seconds(0.0, self.rate),
            RationalTime::from_seconds(self.end_s(), self.rate),
        )
    }

    /// Resolve the audio behind a clip source.
    pub fn resolve_source(&self, source: &ClipSource) -> Option<Arc<AudioBuffer>> {
        match source {
            ClipSource::Buffer(b) => Some(b.clone()),
            ClipSource::Entity(id) => self.media.get(id).cloned(),
            ClipSource::Silence => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_keys_round_trip() {
        for s in [
            Stem::Dialogue,
            Stem::Music,
            Stem::Effects,
            Stem::Foley,
            Stem::Ambience,
            Stem::Custom("walla".into()),
        ] {
            assert_eq!(Stem::parse(&s.key()), s);
        }
        assert_eq!(Stem::parse("DX".to_lowercase().as_str()), Stem::Dialogue);
        assert_eq!(Stem::Dialogue.label(), "Dialogue");
        assert_eq!(Stem::Custom("Walla".into()).abbreviation(), "WALLA");
    }

    #[test]
    fn lane_interpolation() {
        let lane = Lane::new(
            vec![(2.0, 10.0), (0.0, 0.0), (1.0, 5.0)],
            Interpolation::Linear,
        );
        assert_eq!(lane.value_at(-1.0), Some(0.0));
        assert_eq!(lane.value_at(0.5), Some(2.5));
        assert_eq!(lane.value_at(1.5), Some(7.5));
        assert_eq!(lane.value_at(9.0), Some(10.0));
        let step = Lane::new(vec![(0.0, 0.0), (1.0, 5.0)], Interpolation::Step);
        assert_eq!(step.value_at(0.99), Some(0.0));
        assert_eq!(step.value_at(1.0), Some(5.0));
        let mut l = Lane::default();
        assert_eq!(l.value_at(0.0), None);
        l.set_point(1.0, 1.0);
        l.set_point(0.5, 2.0);
        l.set_point(1.0, 3.0);
        assert_eq!(l.points, vec![(0.5, 2.0), (1.0, 3.0)]);
    }

    #[test]
    fn automation_serde_round_trip_and_session_json() {
        let mut a = Automation::new();
        a.set_lane(
            Param::Volume,
            Lane::new(vec![(0.0, 0.0), (1.0, -6.0)], Interpolation::Linear),
        );
        a.set_lane(
            Param::Send(1),
            Lane::new(vec![(0.0, -12.0)], Interpolation::Step),
        );
        let json = serde_json::to_string(&a).unwrap();
        let back: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
        assert_eq!(back.value_at(Param::Send(1), 5.0), Some(-12.0));

        let mut s = Session::new(48_000, 24.0);
        let buf = Arc::new(AudioBuffer::sine(48_000, 1, 440.0, 0.5, 0.5));
        let t = Track::new("DX 1", s.master_id())
            .with_stem(Stem::Dialogue)
            .with_clip(Clip::from_buffer("line", buf, 1.0, 24.0));
        let id = s.add_track(t);
        assert_eq!(
            s.track(id).unwrap().output,
            s.stem_bus(&Stem::Dialogue).unwrap().id
        );
        assert_eq!(s.effective_stem(s.track(id).unwrap()), Some(Stem::Dialogue));
        assert!((s.end_s() - 1.5).abs() < 1e-9);
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tracks, s.tracks);
        assert_eq!(back.master, s.master);
        // A track routed to an untagged bus inherits nothing; via a stem bus it inherits.
        let mut t2 = Track::new("fx", s.master_id());
        t2.output = s.stem_bus(&Stem::Effects).unwrap().id;
        let id2 = s.add_track(t2);
        assert_eq!(s.effective_stem(s.track(id2).unwrap()), Some(Stem::Effects));
        assert_eq!(s.all_stems().len(), 3);
    }
}
