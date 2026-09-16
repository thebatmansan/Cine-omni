//! # cine-logistics — production logistics for CINE-OMNI
//!
//! Turns the screenplay in the Unified Context Graph into the paperwork a
//! production runs on, and keeps it current as the script changes:
//!
//! * **Breakdown sheets** ([`breakdown`]): cast, props, page eighths and
//!   department flags per scene.
//! * **Shot lists and lens grids** ([`shotlist`]): one shot per previs
//!   camera, sized from focal length and subject distance, with gear.
//! * **Stripboard scheduling** ([`schedule`](mod@schedule)): deterministic day packing
//!   under page/hour limits, night blocks, cast availability, and a
//!   [`schedule::diff`] that reports which scenes moved.
//! * **Call sheets** ([`callsheet`]): stable cast numbers, makeup / pickup /
//!   on-set times, crew calls.
//! * **Live metadata** ([`metadata`]): a lock-free ring for 240 Hz camera
//!   metadata, take summaries and change-event detection.
//! * **Graph integration** ([`graph`]): idempotent writers and readers for
//!   every logistics entity.
//!
//! ```
//! use cine_logistics::prelude::*;
//! use ucg_core::Engine;
//!
//! let engine = Engine::new();
//! let mut doc = None;
//! engine
//!     .transact("sample", |tx| {
//!         doc = Some(fixtures::build_sample_document(tx).map_err(Error::into_ucg)?);
//!         Ok(())
//!     })
//!     .unwrap();
//! let doc = doc.unwrap();
//! let graph = engine.read();
//! let sheets = breakdown_document(&graph, doc.document);
//! let board = schedule(&sheets, &ScheduleConstraints::default());
//! assert_eq!(board.scenes().len(), 6);
//! println!("{}", board.to_markdown());
//! ```

pub mod breakdown;
pub mod callsheet;
pub mod date;
pub mod dood;
pub mod error;
pub mod fixtures;
pub mod graph;
pub mod metadata;
pub mod pdf;
pub mod schedule;
pub mod shotlist;

pub use breakdown::{breakdown_document, breakdown_scene, SceneBreakdown};
pub use callsheet::{build_call_sheet, CallSheet, CallSheetScene, CastCall, CastRoster, CrewCall};
pub use date::{Date, DateRange, Time, Weekday};
pub use dood::{day_out_of_days, CastRow, DayOutOfDays, Status};
pub use error::{Error, Result};
pub use graph::{
    document_of_scene, read_schedule, write_breakdown, write_call_sheet, write_schedule,
    write_shot_list, write_take_metadata,
};
pub use metadata::{
    detect_events, simulate_take, summarize, IngestBuffer, MetadataEvent, MetadataEventKind,
    TakeMetadataSummary, TakeScenario,
};
pub use schedule::{
    diff, schedule, schedule_fingerprint, schedule_fingerprint_with, Schedule, ScheduleConstraints,
    ScheduleDiff, ShootDay, Strip,
};
pub use shotlist::{lens_grid, shot_list_for_scene, GearList, LensGrid, ShotEntry, Support};

/// Everything most callers need.
pub mod prelude {
    pub use crate::breakdown::{breakdown_document, breakdown_scene, SceneBreakdown};
    pub use crate::callsheet::{build_call_sheet, CallSheet, CastRoster};
    pub use crate::date::{Date, DateRange, Time};
    pub use crate::error::{Error, Result};
    pub use crate::fixtures;
    pub use crate::graph::*;
    pub use crate::metadata::{
        detect_events, simulate_take, summarize, IngestBuffer, MetadataEvent, MetadataEventKind,
        TakeMetadataSummary, TakeScenario,
    };
    pub use crate::schedule::{
        diff, schedule, Schedule, ScheduleConstraints, ScheduleDiff, ShootDay, Strip,
    };
    pub use crate::shotlist::{lens_grid, shot_list_for_scene, GearList, LensGrid, ShotEntry};
}
