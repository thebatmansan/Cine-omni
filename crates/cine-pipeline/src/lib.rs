//! # cine-pipeline — the reactive wiring of CINE-OMNI
//!
//! This crate is the only place that knows about every module. It installs
//! one [`Reactor`] per module on the [`Engine`]; each reactor is a pure
//! function of a [`ChangeSet`] and the graph, returning transactions the engine
//! applies in a cascade until the graph is quiescent.
//!
//! The thesis scenario — an artist edits a line of dialogue — becomes:
//!
//! ```text
//! user tx: set dialogue.text
//!   ├─ ScriptReactor     recompute scene words / eighths / duration, mark alignments stale
//!   ├─ PrevisReactor     resync previs scene (characters, dialogue track length, bounds)
//!   ├─ LogisticsReactor  rebuild breakdown, shot list, schedule diff, call sheets
//!   ├─ TimelineReactor   flag clips synced to the scene as needs_conform
//!   ├─ AudioReactor      resync dialogue placeholder clips in the DAW session
//!   ├─ CompReactor       mark comps bound to the scene's shots dirty, refresh frame ranges
//!   └─ ColorReactor      carry color metadata to any newly bound clips / grades
//! ```
//!
//! All follow-up transactions are grouped with the user's transaction for undo.

pub mod project;
pub mod reactors;
pub mod session;

pub use project::{OmniProject, ProjectSetup};
pub use reactors::install_all;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error(transparent)]
    Ucg(#[from] ucg_core::UcgError),
    #[error("script: {0}")]
    Script(String),
    #[error("previs: {0}")]
    Previs(String),
    #[error("logistics: {0}")]
    Logistics(String),
    #[error("timeline: {0}")]
    Timeline(String),
    #[error("comp: {0}")]
    Comp(String),
    #[error("color: {0}")]
    Color(String),
    #[error("audio: {0}")]
    Audio(String),
}

pub type Result<T> = std::result::Result<T, PipelineError>;

/// Convert a module error into the engine's error type for use inside plans.
pub(crate) fn engine_err(module: &str, e: impl std::fmt::Display) -> ucg_core::UcgError {
    ucg_core::UcgError::Reactor {
        reactor: module.to_string(),
        message: e.to_string(),
    }
}
