//! Derived media, and whether it may still be used.
//!
//! Proxies, waveforms, thumbnails and renders are all *made from* an original,
//! and the question asked of each is the same: is this still a thing derived
//! from the media the project has now, by a recipe this build still means, and
//! is its file the file that was written? So the answer is defined once, here,
//! rather than once per kind with the differences left to drift.
//!
//! The rules, and what each is for:
//!
//! * **its file is gone** — `missing`. Nothing to use.
//! * **made from other content** — `stale`. The original's content hash is its
//!   identity, so this catches a replaced original. It deliberately does *not*
//!   catch a *moved* one: relinking changes where the media is, not what it is,
//!   and derived data made from that content is still made from that content.
//! * **made by an older recipe** — `stale`. A recipe version is bumped when the
//!   output would differ, so an overview or a proxy from before the change is
//!   not what this build would make now.
//! * **its own file changed** — `stale`, by size always and by content hash
//!   when asked deeply. Something wrote over it.

use std::path::PathBuf;
use ucg_core::{EdgeKind, EntityId, Graph};
use ucg_schema::attrs;

/// The original's content hash, as recorded on the derived node when it was
/// made.
pub const SOURCE_HASH_ATTR: &str = "source_hash";
/// Size of the derived file when it was written.
pub const FILE_SIZE_ATTR: &str = "file_size";
/// The job that made it.
pub const JOB_ATTR: &str = "job";
/// Notes a person should see: what the maker could not do, and why.
pub const NOTES_ATTR: &str = "notes";

/// Whether a piece of derived media may still be used, and why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// `current`, `stale` or `missing`.
    pub state: &'static str,
    pub reasons: Vec<String>,
}

impl Status {
    pub fn is_current(&self) -> bool {
        self.state == "current"
    }
}

/// The state of one derived node, against the original it names.
///
/// `recipe_version` is the suffix the current recipe ends with. `deep`
/// re-hashes the derived file, which reads part of it; without it the file's
/// presence and size are checked, which is what a hot path can afford.
pub fn status(graph: &Graph, derived: EntityId, recipe_version: &str, deep: bool) -> Status {
    let Some(node) = graph.node(derived) else {
        return Status {
            state: "missing",
            reasons: vec!["it is not in the project".into()],
        };
    };
    let path = node.get_str(attrs::MEDIA_PATH).map(PathBuf::from);
    let Some(path) = path.filter(|p| p.is_file()) else {
        return Status {
            state: "missing",
            reasons: vec!["its file is gone".into()],
        };
    };
    let mut reasons = Vec::new();
    let original = graph
        .targets(derived, EdgeKind::DerivedFrom)
        .into_iter()
        .next();
    let original_hash = original
        .and_then(|o| graph.node(o))
        .and_then(|n| n.get_str(cine_media::ucg::HASH_ATTR));
    if original_hash.is_none() || original_hash != node.get_str(SOURCE_HASH_ATTR) {
        reasons.push("it was made from different content".into());
    }
    if !node
        .get_str("recipe")
        .is_some_and(|r| r.ends_with(recipe_version))
    {
        reasons.push(format!("it was made by an older recipe than {recipe_version}"));
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if node.get_int(FILE_SIZE_ATTR) != Some(size as i64) {
        reasons.push("its file changed on disk since it was made".into());
    } else if deep {
        match cine_media::ContentHash::fast(&path) {
            Ok(h) if node.get_str(cine_media::ucg::HASH_ATTR) != Some(h.hex()) => {
                reasons.push("its file changed on disk since it was made".into())
            }
            Ok(_) => {}
            Err(e) => reasons.push(format!("its file cannot be read: {e}")),
        }
    }
    Status {
        state: if reasons.is_empty() { "current" } else { "stale" },
        reasons,
    }
}

/// Derived nodes of one kind made from `media`.
pub fn of_kind(graph: &Graph, media: EntityId, kind: &str) -> Vec<EntityId> {
    graph
        .sources(media, EdgeKind::DerivedFrom)
        .into_iter()
        .filter(|d| {
            graph.node(*d).is_some_and(|n| {
                n.kind.as_str() == cine_media::ucg::MEDIA_DERIVATIVE
                    && n.get_str("derivative_kind") == Some(kind)
            })
        })
        .collect()
}
