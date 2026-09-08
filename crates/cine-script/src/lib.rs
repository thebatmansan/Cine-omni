//! # cine-script — screenplays in the Unified Context Graph
//!
//! Fountain and Final Draft (FDX) parsing and writing, breakdown
//! auto-tagging, page/duration statistics, ScriptSync word alignment, and
//! bidirectional import/export between the [`Screenplay`] model and the UCG.
//!
//! Domain code (`fountain`, `fdx`, `model`, `tag`, `stats`, `sync`) is pure;
//! only [`graph`] touches the graph, through `Tx`/`Graph`.
//!
//! ```
//! use cine_script::prelude::*;
//! use ucg_core::Engine;
//!
//! let sp = parse_fountain("INT. KITCHEN - NIGHT\n\nMAREN pours coffee.\n\nMAREN\nWe ride at dawn.\n");
//! assert_eq!(sp.scenes().len(), 1);
//! let tags = auto_tag(&sp);
//! assert_eq!(tags.characters[0].name, "MAREN");
//!
//! let engine = Engine::new();
//! let project = EntityId::new();
//! engine
//!     .transact("import", |tx| {
//!         tx.create_with_id(project, "project")?;
//!         import_screenplay(tx, project, &sp, "kitchen.fountain").map_err(|e| UcgError::Aborted(e.to_string()))?;
//!         Ok(())
//!     })
//!     .unwrap();
//! let back = export_screenplay(&engine.read(), document_id("kitchen.fountain")).unwrap();
//! assert_eq!(back.elements, sp.elements);
//! ```

pub mod error;
pub mod fdx;
pub mod fountain;
pub mod graph;
pub mod model;
pub mod stats;
pub mod sync;
pub mod tag;

pub use error::{Error, Result};
pub use fdx::{parse_fdx, write_fdx};
pub use fountain::{parse_fountain, write_fountain};
pub use graph::{
    apply_alignment, document_id, export_screenplay, import_screenplay, mark_alignments_stale,
    recompute_scene_stats, scene_dialogue_words, scene_id,
};
pub use model::{parse_scene_heading, Element, SceneView, Screenplay};
pub use stats::{estimate_duration_seconds, page_eighths, word_list};
pub use sync::{align_to_frames, align_words, TimedWord};
pub use tag::{auto_tag, CharacterTag, LocationTag, PropTag, Tags};

/// Everything most callers need.
pub mod prelude {
    pub use crate::error::{Error, Result};
    pub use crate::fdx::{parse_fdx, write_fdx};
    pub use crate::fountain::{parse_fountain, write_fountain};
    pub use crate::graph::{
        apply_alignment, document_id, export_screenplay, import_screenplay, mark_alignments_stale,
        recompute_scene_stats, scene_dialogue_words, scene_id,
    };
    pub use crate::model::{parse_scene_heading, Element, SceneView, Screenplay};
    pub use crate::stats::{estimate_duration_seconds, page_eighths, word_list};
    pub use crate::sync::{align_to_frames, align_words, TimedWord};
    pub use crate::tag::{auto_tag, Tags};
    pub use ucg_core::{EntityId, UcgError};
    pub use ucg_schema::{Alignment, SceneHeading, WordAlignment};
}
