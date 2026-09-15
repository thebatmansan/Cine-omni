//! The native window's view of a project it does not own.
//!
//! Inside `cine start` the runtime owns the project: one engine, one op log.
//! The window is a client of it like the browser is, with one difference that
//! is the reason it exists — it plays the cut in real time on the GPU. What it
//! needs from the project is small and read-only, plus the few intents it can
//! send back, and this trait is exactly that. The shell does not depend on the
//! runtime; the runtime's supervisor implements this for it.
//!
//! Everything the window plays comes from [`SequencePlan`], which the runtime
//! builds from the same flattened edit list the browser's record monitor reads.
//! The window never composes the cut itself.

use cine_media::{EditEntry, SoundEntry};
use cine_timeline::session::TrackState;
use cine_timeline::Timeline;
use ucg_core::{EntityId, Rate, RationalTime};

/// A sequence, ready to play.
#[derive(Clone, Debug, PartialEq)]
pub struct SequencePlan {
    pub id: EntityId,
    pub name: String,
    pub rate: Rate,
    pub end: RationalTime,
    pub entries: Vec<EditEntry>,
    /// Every audible sound clip, for the window to mix and play.
    pub sound: Vec<SoundEntry>,
    /// Transitions on visible picture tracks, which play as cuts.
    pub transitions: usize,
    /// The composed timeline, for the timeline pane.
    pub timeline: Timeline,
    /// Track entity ids in timeline order, for intents about a track.
    pub tracks: Vec<EntityId>,
}

/// What the window may ask of the project.
pub trait ProjectLink {
    /// The project's name, for the title bar.
    fn name(&self) -> String;
    /// Where the browser surface is served, if it is.
    fn address(&self) -> Option<String>;
    /// The shared revision. Changes whenever any client commits.
    fn revision(&self) -> u64;
    /// The sequence to play: the first one in the project, if any.
    fn sequence(&self) -> Option<SequencePlan>;
    /// Call `wake` after every commit by anyone, and when the application is
    /// asked to quit. It must be cheap and must not block.
    fn subscribe(&self, wake: Box<dyn Fn() + Send + Sync>);
    /// Whether the application has been asked to quit (Ctrl-C, SIGTERM).
    fn should_quit(&self) -> bool;
    /// Commit a track's editorial state through the project.
    fn set_track_state(&self, track: EntityId, state: TrackState) -> Result<(), String>;
}
