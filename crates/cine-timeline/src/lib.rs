//! # cine-timeline — editorial for CINE-OMNI
//!
//! A multi-track NLE data model conforming to OpenTimelineIO, the classic
//! editing operations (ripple, roll, slip, slide, insert, overwrite, lift,
//! extract, split, join), dual-system audio sync by cross-correlation, and
//! the mapping of all of it onto the Unified Context Graph.
//!
//! * [`otio`] — pure model (`Timeline` → `Stack` → `Track` → `Clip | Gap |
//!   Transition`) and composition math.
//! * [`otio_json`] — lossless `.otio` reader/writer (OTIO 0.15+ schema).
//! * [`edit`] — frame-exact editing operations with media-bound clamping and
//!   invariant validation, plus a timeline diff.
//! * [`sync`] — FFT cross-correlation of RMS envelopes to align external
//!   audio with camera scratch tracks.
//! * [`graph`] — `import_timeline` / `export_timeline`, clip queries and the
//!   mutations the pipeline reactors call (`flag_conform`, `apply_edit` ...).
//!
//! ```
//! use cine_timeline::prelude::*;
//!
//! // Two clips on a video track, then ripple-trim the first one's tail.
//! let mut track = Track::new("V1", TrackKind::Video)
//!     .with(Clip::external("A", "file:///a.mov",
//!         Some(TimeRange::from_frames(0, 100, 24.0)), Some(TimeRange::from_frames(0, 48, 24.0))))
//!     .with(Clip::external("B", "file:///b.mov",
//!         Some(TimeRange::from_frames(0, 100, 24.0)), Some(TimeRange::from_frames(0, 48, 24.0))));
//! let applied = ripple_trim(&mut track, 0, Edge::Tail, RationalTime::from_frames(12, 24.0)).unwrap();
//! assert_eq!(applied, RationalTime::from_frames(12, 24.0));
//! assert_eq!(track.range_of_child(1), Some(TimeRange::from_frames(60, 48, 24.0)));
//!
//! // And it serializes as OTIO.
//! let mut timeline = Timeline::new("demo");
//! timeline.tracks.children.push(Composable::Track(track));
//! let text = write_otio(&timeline);
//! assert_eq!(read_otio(&text).unwrap(), timeline);
//! ```

pub mod edit;
pub mod error;
pub mod graph;
pub mod otio;
pub mod otio_json;
pub mod sync;

pub use edit::{
    diff, diff_tracks, extract, insert, join, lift, overwrite, ripple_trim, roll, slide, slip,
    split_at, Change, ChangeKind, Edge,
};
pub use error::{Error, Result};
pub use graph::{
    apply_edit, bind_clip_to_scene, clear_conform, clip_track_range, clips_synced_to,
    export_timeline, export_track, flag_conform, import_timeline, media_of_clip, EditOp,
};
pub use otio::{
    Clip, Composable, Gap, Item, Marker, MediaReference, Metadata, Stack, Timeline, Track,
    TrackKind, Transition, TransitionType, UCG_ID_KEY,
};
pub use otio_json::{read_otio, write_otio};
pub use sync::{align_clip, find_offset, sync_clips, SyncResult};

/// Everything most callers need.
pub mod prelude {
    pub use crate::edit::{
        diff, extract, insert, join, lift, overwrite, ripple_trim, roll, slide, slip, split_at,
        Change, ChangeKind, Edge,
    };
    pub use crate::error::{Error, Result};
    pub use crate::graph::{
        apply_edit, bind_clip_to_scene, clear_conform, clip_track_range, clips_synced_to,
        export_timeline, flag_conform, import_timeline, media_of_clip, EditOp,
    };
    pub use crate::otio::{
        Clip, Composable, Gap, Item, Marker, MediaReference, Metadata, Stack, Timeline, Track,
        TrackKind, Transition, TransitionType,
    };
    pub use crate::otio_json::{read_otio, write_otio};
    pub use crate::sync::{align_clip, find_offset, sync_clips, SyncResult};
    pub use ucg_core::{RationalTime, TimeRange};
}
