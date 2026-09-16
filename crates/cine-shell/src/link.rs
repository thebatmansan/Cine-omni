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

use cine_color::lut::Lut3D;
use cine_media::{EditEntry, SoundEntry};
use cine_timeline::session::TrackState;
use cine_timeline::Timeline;
use std::sync::Arc;
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

/// How the window must show the pictures: the project's colour config, applied.
///
/// The runtime compiles the transform — input colorspace of each clip, through
/// the chosen display and view — and bakes it to a lattice the GPU can
/// interpolate. The window applies it and says what it is; it never decides
/// what a colour transform should be, and it never invents one for a clip
/// whose colorspace nothing established.
#[derive(Clone, Debug, PartialEq)]
pub struct Viewing {
    pub display: String,
    pub view: String,
    /// One per [`SequencePlan::entries`]; `None` where no transform applies.
    pub luts: Vec<Option<Arc<Lut3D>>>,
    /// What each distinct transform is, for the window to show.
    pub describe: Vec<String>,
    /// Worst error of the lattices against the transforms they were baked
    /// from, in display code value, as measured when they were baked.
    pub worst_error: f32,
}

impl Viewing {
    /// Whether this is the same viewing as `other` — the same lattices, not
    /// merely equal ones. Comparing 33³ nodes per clip on every commit would
    /// cost more than the transform itself.
    pub fn same_as(&self, other: &Viewing) -> bool {
        self.display == other.display
            && self.view == other.view
            && self.luts.len() == other.luts.len()
            && self
                .luts
                .iter()
                .zip(&other.luts)
                .all(|(a, b)| match (a, b) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                })
    }
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
    /// How to show it. `None` when the project has no colour config, or when
    /// the chosen display and view cannot be compiled — the window then shows
    /// the pictures as they are decoded and says so.
    fn viewing(&self) -> Option<Viewing>;
    /// Call `wake` after every commit by anyone, and when the application is
    /// asked to quit. It must be cheap and must not block.
    fn subscribe(&self, wake: Box<dyn Fn() + Send + Sync>);
    /// Whether the application has been asked to quit (Ctrl-C, SIGTERM).
    fn should_quit(&self) -> bool;
    /// Commit a track's editorial state through the project.
    fn set_track_state(&self, track: EntityId, state: TrackState) -> Result<(), String>;
}
