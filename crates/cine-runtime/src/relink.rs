//! Finding media that moved, and refusing media that only looks the same.
//!
//! ## Identity is content, never a name
//!
//! An asset's identity is its content hash; its path is where the file was
//! last seen. So relinking is a search for *that content*, not for that name.
//! A file called `A001C001.mov` whose bytes differ is a different take, and
//! attaching it to the cut would put the wrong pictures under every edit made
//! against the old one — silently, because the names agree. Nothing here
//! relinks on a name, a size, a date or a resemblance.
//!
//! ## What a scan finds
//!
//! For each asset asked about, a scan of a folder answers one of:
//!
//! * **relinked** — a file whose content is this asset's content was found and
//!   the asset now points at it. The asset keeps its identity, so every clip,
//!   proxy, grade and render that referenced it still does.
//! * **already here** — the file at the recorded path is that content.
//! * **replaced** — a file exists at the recorded path, but its content is not
//!   this asset's. Editorial decisions were made against the old content, so
//!   this is reported, never accepted: replacing media is a separate, explicit
//!   act.
//! * **not found** — nothing in the folder holds this content. Files that
//!   share the name are listed as near misses, with their hashes, so a person
//!   can see *why* the obvious candidate was refused.
//!
//! Two paths holding identical content are not an ambiguity: either serves,
//! and the rest are reported as duplicates.
//!
//! ## How strongly
//!
//! [`ContentHash::fast`] fingerprints head, middle, tail and size — cheap
//! enough to scan a card with. When the asset carries a verified SHA-256
//! (the `media.checksum` job establishes one), the winning candidate is hashed
//! in full and must match that too; then the relink is `exact` rather than
//! `fingerprint`. The confidence travels into the graph with the relink, so a
//! later reader knows which was done.

use cine_media::ContentHash;
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use ucg_core::{EntityId, UcgError};
use ucg_schema::{attrs, kinds};

/// The job kind.
pub const RELINK_KIND: &str = "media.relink";

/// How deep a scan walks by default.
pub const DEFAULT_DEPTH: usize = 8;

/// Recorded on an asset when a scan could not resolve it.
pub const STATE_ATTR: &str = "relink_state";
/// Why, in words, for the interface to show.
pub const NOTE_ATTR: &str = "relink_note";
/// Where the media was before it was relinked.
pub const FROM_ATTR: &str = "relinked_from";
/// `exact` (full hash verified) or `fingerprint`.
pub const CONFIDENCE_ATTR: &str = "relink_confidence";

/// One candidate file a scan looked at.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub hash: String,
    pub size: u64,
}

/// What a scan decided about one asset.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub media: EntityId,
    pub name: String,
    /// `relinked`, `already_here`, `replaced`, `not_found`.
    pub state: &'static str,
    pub note: String,
    pub path: Option<PathBuf>,
    pub confidence: Option<&'static str>,
    /// Other files with the same content, which would have served equally.
    pub duplicates: Vec<PathBuf>,
    /// Files that share the name but not the content, with their hashes.
    pub near_misses: Vec<Candidate>,
}

impl Outcome {
    pub fn to_json(&self) -> J {
        json!({
            "media": self.media,
            "name": self.name,
            "state": self.state,
            "note": self.note,
            "path": self.path,
            "confidence": self.confidence,
            "duplicates": self.duplicates,
            "near_misses": self.near_misses.iter().map(|c| json!({
                "path": c.path,
                "content_hash": c.hash,
                "size": c.size,
            })).collect::<Vec<_>>(),
        })
    }
}

/// An asset as a scan needs to see it.
struct Wanted {
    media: EntityId,
    name: String,
    hash: String,
    size: u64,
    path: PathBuf,
    sha256: Option<String>,
    file_name: Option<String>,
}

fn wanted(project: &OmniProject, only: Option<EntityId>) -> Vec<Wanted> {
    let g = project.engine.read();
    g.of_kind(kinds::ASSET_MEDIA)
        .iter()
        .filter(|n| only.is_none_or(|id| n.id == id))
        .filter_map(|n| {
            let path = PathBuf::from(n.get_str(attrs::MEDIA_PATH)?);
            let info = cine_media::ucg::read_media(&g, n.id)?;
            Some(Wanted {
                media: n.id,
                name: n.name(),
                hash: n.get_str(cine_media::ucg::HASH_ATTR)?.to_string(),
                size: info.hash.size(),
                file_name: path.file_name().map(|f| f.to_string_lossy().into_owned()),
                path,
                sha256: n
                    .get(crate::jobs::CHECKSUM_ATTR)
                    .and_then(|v| v.to_json().get("sha256").and_then(J::as_str).map(str::to_string)),
            })
        })
        .collect()
}

/// Walk `folder` to `depth`, calling `each` with every file.
///
/// The project's own derived media and caches are skipped: a proxy of a file
/// is not that file, and walking into them would only waste the scan.
fn walk(folder: &Path, depth: usize, each: &mut dyn FnMut(&Path, u64) -> bool) -> bool {
    if depth == 0 {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return true;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if name == "derived" || name == "cache" || name == "oplog" {
                    continue;
                }
                if !walk(&path, depth - 1, each) {
                    return false;
                }
            }
            Ok(t) if t.is_file() => {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if !each(&path, size) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// Look for the content of `only` (or of every asset) under `folder`, and
/// relink what is found. `progress` returns `false` to cancel.
pub fn scan(
    project: &OmniProject,
    folder: &Path,
    only: Option<EntityId>,
    depth: usize,
    progress: &mut dyn FnMut(f32, &str) -> bool,
) -> Result<J, String> {
    let began = Instant::now();
    if !folder.is_dir() {
        return Err(format!("{} is not a folder to search", folder.display()));
    }
    let assets = wanted(project, only);
    if assets.is_empty() {
        return Err("there is no media in this project to look for".into());
    }

    // Only files whose size matches some asset's are worth hashing: a file of
    // another length is certainly other content.
    let sizes: Vec<u64> = assets.iter().map(|a| a.size).collect();
    let names: Vec<&str> = assets.iter().filter_map(|a| a.file_name.as_deref()).collect();
    let mut by_hash: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut by_name: HashMap<String, Vec<Candidate>> = HashMap::new();
    let mut seen = 0usize;
    let mut hashed = 0usize;
    let mut cancelled = false;
    walk(folder, depth, &mut |path, size| {
        seen += 1;
        if seen.is_multiple_of(32)
            && !progress(
                0.0,
                &format!("{seen} files seen, {hashed} hashed"),
            )
        {
            cancelled = true;
            return false;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let interesting_size = sizes.contains(&size);
        let interesting_name = name.as_deref().is_some_and(|n| names.contains(&n));
        if !interesting_size && !interesting_name {
            return true;
        }
        let Ok(hash) = ContentHash::fast(path) else {
            return true;
        };
        hashed += 1;
        by_hash.entry(hash.hex().to_string()).or_default().push(path.to_path_buf());
        if let Some(name) = name {
            by_name.entry(name).or_default().push(Candidate {
                path: path.to_path_buf(),
                hash: hash.hex().to_string(),
                size,
            });
        }
        true
    });
    if cancelled {
        return Ok(json!({ "cancelled": true }));
    }

    let mut outcomes = Vec::new();
    for asset in &assets {
        let mut found: Vec<PathBuf> = by_hash.get(&asset.hash).cloned().unwrap_or_default();
        found.sort();
        let near_misses: Vec<Candidate> = asset
            .file_name
            .as_deref()
            .and_then(|n| by_name.get(n))
            .map(|c| c.iter().filter(|c| c.hash != asset.hash).cloned().collect())
            .unwrap_or_default();

        // What is at the recorded path now?
        let here = ContentHash::fast(&asset.path).ok().map(|h| h.hex().to_string());
        if here.as_deref() == Some(asset.hash.as_str()) {
            outcomes.push(Outcome {
                media: asset.media,
                name: asset.name.clone(),
                state: "already_here",
                note: "the file at its recorded path is this media".into(),
                path: Some(asset.path.clone()),
                confidence: None,
                duplicates: found.into_iter().filter(|p| *p != asset.path).collect(),
                near_misses,
            });
            continue;
        }

        match found.first().cloned() {
            Some(path) => {
                // When the project holds a verified checksum, the winner must
                // match it in full, not merely fingerprint the same.
                let confidence = match &asset.sha256 {
                    Some(want) => match ContentHash::full(&path) {
                        Ok(full) if full.hex() == want.as_str() => "exact",
                        Ok(_) => {
                            outcomes.push(Outcome {
                                media: asset.media,
                                name: asset.name.clone(),
                                state: "not_found",
                                note: format!(
                                    "{} fingerprints the same but its full checksum differs from this media's verified one; not relinked",
                                    path.display()
                                ),
                                path: None,
                                confidence: None,
                                duplicates: Vec::new(),
                                near_misses,
                            });
                            continue;
                        }
                        Err(e) => {
                            outcomes.push(Outcome {
                                media: asset.media,
                                name: asset.name.clone(),
                                state: "not_found",
                                note: format!("{} could not be read in full: {e}", path.display()),
                                path: None,
                                confidence: None,
                                duplicates: Vec::new(),
                                near_misses,
                            });
                            continue;
                        }
                    },
                    None => "fingerprint",
                };
                outcomes.push(Outcome {
                    media: asset.media,
                    name: asset.name.clone(),
                    state: "relinked",
                    note: format!(
                        "found by content at {} ({confidence})",
                        path.display()
                    ),
                    path: Some(path.clone()),
                    confidence: Some(confidence),
                    duplicates: found.into_iter().filter(|p| *p != path).collect(),
                    near_misses,
                });
            }
            None => {
                let (state, note) = match here {
                    Some(other) => (
                        "replaced",
                        format!(
                            "the file at {} is different media now ({}… where this asset is {}…); replacing media is a separate decision, so nothing was changed",
                            asset.path.display(),
                            &other[..other.len().min(12)],
                            &asset.hash[..asset.hash.len().min(12)]
                        ),
                    ),
                    None => (
                        "not_found",
                        match near_misses.len() {
                            0 => format!("no file under {} holds this content", folder.display()),
                            n => format!(
                                "no file under {} holds this content; {n} file{} of that name hold{} other content",
                                folder.display(),
                                if n == 1 { "" } else { "s" },
                                if n == 1 { "s" } else { "" }
                            ),
                        },
                    ),
                };
                outcomes.push(Outcome {
                    media: asset.media,
                    name: asset.name.clone(),
                    state,
                    note,
                    path: None,
                    confidence: None,
                    duplicates: Vec::new(),
                    near_misses,
                });
            }
        }
    }

    apply(project, &outcomes)?;
    Ok(json!({
        "folder": folder,
        "files_seen": seen,
        "files_hashed": hashed,
        "relinked": outcomes.iter().filter(|o| o.state == "relinked").count(),
        "unresolved": outcomes.iter().filter(|o| o.state != "relinked" && o.state != "already_here").count(),
        "outcomes": outcomes.iter().map(Outcome::to_json).collect::<Vec<_>>(),
        "elapsed_ms": began.elapsed().as_millis() as u64,
    }))
}

/// Write a scan's decisions into the project: one commit, undoable.
fn apply(project: &OmniProject, outcomes: &[Outcome]) -> Result<(), String> {
    let changed: Vec<&Outcome> = outcomes
        .iter()
        .filter(|o| {
            let g = project.engine.read();
            let node = g.node(o.media);
            let path_now = node.and_then(|n| n.get_str(attrs::MEDIA_PATH)).map(PathBuf::from);
            let state_now = node.and_then(|n| n.get_str(STATE_ATTR)).map(str::to_string);
            let online_now = node.and_then(|n| n.get_bool(cine_media::ucg::ONLINE_ATTR));
            match o.state {
                "relinked" => path_now != o.path || online_now != Some(true),
                "already_here" => state_now.is_some() || online_now != Some(true),
                _ => state_now.as_deref() != Some(o.state) || online_now != Some(false),
            }
        })
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    project
        .engine
        .transact("relink media", |tx| {
            for o in &changed {
                match (o.state, &o.path) {
                    ("relinked", Some(path)) => {
                        let was = tx
                            .graph()
                            .node(o.media)
                            .and_then(|n| n.get_str(attrs::MEDIA_PATH))
                            .map(str::to_string);
                        tx.set(o.media, attrs::MEDIA_PATH, path.display().to_string())?;
                        if let Some(was) = was {
                            tx.set(o.media, FROM_ATTR, was)?;
                        }
                        tx.set(o.media, CONFIDENCE_ATTR, o.confidence.unwrap_or("fingerprint"))?;
                        tx.remove_attr(o.media, STATE_ATTR)?;
                        tx.remove_attr(o.media, NOTE_ATTR)?;
                        cine_media::ucg::set_online(tx, o.media, true)
                            .map_err(|e| UcgError::Aborted(e.to_string()))?;
                        // The stored `MediaFile` carries the old path; keep it
                        // in step so everything that reads it agrees.
                        if let Some(mut info) = cine_media::ucg::read_media(tx.graph(), o.media) {
                            info.path = path.clone();
                            tx.set(
                                o.media,
                                cine_media::ucg::MEDIA_INFO_ATTR,
                                ucg_core::Value::from_serde(&info)?,
                            )?;
                        }
                    }
                    ("already_here", _) => {
                        tx.remove_attr(o.media, STATE_ATTR)?;
                        tx.remove_attr(o.media, NOTE_ATTR)?;
                        // The file is there and it is this media: online again.
                        cine_media::ucg::set_online(tx, o.media, true)
                            .map_err(|e| UcgError::Aborted(e.to_string()))?;
                    }
                    _ => {
                        tx.set(o.media, STATE_ATTR, o.state)?;
                        tx.set(o.media, NOTE_ATTR, o.note.as_str())?;
                        cine_media::ucg::set_online(tx, o.media, false)
                            .map_err(|e| UcgError::Aborted(e.to_string()))?;
                    }
                }
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Look at every original where the project says it is, and record what is
/// actually there.
///
/// Existence is not identity: a file of the same name holding other content is
/// *not* this media, and saying "online" about it would be the lie the whole
/// relink path exists to prevent. So each file is fingerprinted, and an asset
/// is online only when the file at its path is its content.
pub fn check(project: &OmniProject) -> Result<J, String> {
    let assets = wanted(project, None);
    let mut outcomes = Vec::new();
    for asset in &assets {
        let here = ContentHash::fast(&asset.path).ok().map(|h| h.hex().to_string());
        let (state, note) = match &here {
            Some(hash) if *hash == asset.hash => (
                "already_here",
                "the file at its recorded path is this media".to_string(),
            ),
            Some(other) => (
                "replaced",
                format!(
                    "the file at {} is different media now ({}… where this asset is {}…)",
                    asset.path.display(),
                    &other[..other.len().min(12)],
                    &asset.hash[..asset.hash.len().min(12)]
                ),
            ),
            None => (
                "not_found",
                format!("{} is not there", asset.path.display()),
            ),
        };
        outcomes.push(Outcome {
            media: asset.media,
            name: asset.name.clone(),
            state,
            note,
            path: (state == "already_here").then(|| asset.path.clone()),
            confidence: None,
            duplicates: Vec::new(),
            near_misses: Vec::new(),
        });
    }
    let before: Vec<bool> = {
        let g = project.engine.read();
        outcomes
            .iter()
            .map(|o| {
                g.node(o.media)
                    .and_then(|n| n.get_bool(cine_media::ucg::ONLINE_ATTR))
                    .unwrap_or(false)
            })
            .collect()
    };
    apply(project, &outcomes)?;
    let changed: Vec<J> = outcomes
        .iter()
        .zip(before)
        .filter(|(o, was)| (o.state == "already_here") != *was)
        .map(|(o, _)| json!({ "media": o.media, "online": o.state == "already_here" }))
        .collect();
    Ok(json!({
        "changed": changed,
        "outcomes": outcomes.iter().map(Outcome::to_json).collect::<Vec<_>>(),
    }))
}

/// Relink one asset to one named file, because a person said so.
///
/// Still by content: the file must *be* that media. A file that is not is
/// refused with both hashes, because the alternative is putting other
/// pictures under edits already made.
pub fn relink_to(project: &OmniProject, media: EntityId, path: &Path) -> Result<J, String> {
    let (want, name, sha256) = {
        let g = project.engine.read();
        let n = g.node(media).ok_or("no such media in this project")?;
        if n.kind.as_str() != kinds::ASSET_MEDIA {
            return Err(format!("{media} is not media"));
        }
        (
            n.get_str(cine_media::ucg::HASH_ATTR)
                .ok_or("the media has no content hash")?
                .to_string(),
            n.name(),
            n.get(crate::jobs::CHECKSUM_ATTR)
                .and_then(|v| v.to_json().get("sha256").and_then(J::as_str).map(str::to_string)),
        )
    };
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let found = ContentHash::fast(path).map_err(|e| e.to_string())?;
    if found.hex() != want.as_str() {
        return Err(format!(
            "{} is not `{name}`: its content is {}… where this media is {}…. Relinking it would put other pictures under every edit made against this media",
            path.display(),
            &found.hex()[..12],
            &want[..12.min(want.len())]
        ));
    }
    let confidence = match &sha256 {
        Some(reference) => {
            let full = ContentHash::full(path).map_err(|e| e.to_string())?;
            if full.hex() != reference.as_str() {
                return Err(format!(
                    "{} fingerprints as `{name}` but its full checksum differs from the verified one recorded for this media",
                    path.display()
                ));
            }
            "exact"
        }
        None => "fingerprint",
    };
    let outcome = Outcome {
        media,
        name,
        state: "relinked",
        note: format!("relinked by hand to {} ({confidence})", path.display()),
        path: Some(path.to_path_buf()),
        confidence: Some(confidence),
        duplicates: Vec::new(),
        near_misses: Vec::new(),
    };
    apply(project, std::slice::from_ref(&outcome))?;
    Ok(outcome.to_json())
}

/// What the application shows about an asset's resolution, for the media list.
pub fn state_json(graph: &ucg_core::Graph, media: EntityId) -> J {
    let node = graph.node(media);
    json!({
        "state": node.and_then(|n| n.get_str(STATE_ATTR)),
        "note": node.and_then(|n| n.get_str(NOTE_ATTR)),
        "relinked_from": node.and_then(|n| n.get_str(FROM_ATTR)),
        "confidence": node.and_then(|n| n.get_str(CONFIDENCE_ATTR)),
    })
}
