//! Project persistence: JSON / MessagePack snapshots plus an append-only op log.
//!
//! ```text
//! MyFilm.cineomni/
//!   project.json      manifest (schema version, root entity, name)
//!   graph.json        snapshot (pretty JSON; diff-friendly)
//!   graph.msgpack     optional fast binary snapshot
//!   oplog/NNNN.jsonl  deltas since the snapshot, one JSON delta per line
//!   assets/           content-addressed media
//! ```

use crate::engine::Engine;
use crate::error::{Result, UcgError};
use crate::graph::Graph;
use crate::id::EntityId;
use crate::tx::Delta;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub name: String,
    pub root: Option<EntityId>,
    #[serde(default)]
    pub snapshot_revision: u64,
    #[serde(default)]
    pub app_version: String,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            schema_version: SCHEMA_VERSION,
            name: "Untitled".into(),
            root: None,
            snapshot_revision: 0,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    schema_version: u32,
    graph: Graph,
}

/// Serialize a graph to pretty JSON.
pub fn graph_to_json(graph: &Graph) -> Result<String> {
    Ok(serde_json::to_string_pretty(&Snapshot {
        schema_version: SCHEMA_VERSION,
        graph: graph.clone(),
    })?)
}

pub fn graph_from_json(json: &str) -> Result<Graph> {
    let snap: Snapshot = serde_json::from_str(json)?;
    check_version(snap.schema_version)?;
    let mut g = migrate(snap.graph, snap.schema_version);
    g.rebuild_indices();
    Ok(g)
}

pub fn graph_to_msgpack(graph: &Graph) -> Result<Vec<u8>> {
    Ok(rmp_serde::to_vec_named(&Snapshot {
        schema_version: SCHEMA_VERSION,
        graph: graph.clone(),
    })?)
}

pub fn graph_from_msgpack(bytes: &[u8]) -> Result<Graph> {
    let snap: Snapshot = rmp_serde::from_slice(bytes)?;
    check_version(snap.schema_version)?;
    let mut g = migrate(snap.graph, snap.schema_version);
    g.rebuild_indices();
    Ok(g)
}

fn check_version(v: u32) -> Result<()> {
    if v > SCHEMA_VERSION {
        return Err(UcgError::SchemaVersion {
            found: v,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// Forward migrations over raw attribute bags. Each step upgrades one version.
fn migrate(graph: Graph, from: u32) -> Graph {
    let mut g = graph;
    let mut v = from;
    while v < SCHEMA_VERSION {
        // Future: match v { 1 => migrate_1_to_2(&mut g), ... }
        v += 1;
    }
    let _ = &mut g;
    g
}

/// On-disk project handle.
pub struct Project {
    pub path: PathBuf,
    pub manifest: Manifest,
    pub engine: Arc<Engine>,
}

impl Project {
    pub fn oplog_dir(path: &Path) -> PathBuf {
        path.join("oplog")
    }

    /// Create a new project directory with an empty graph.
    pub fn create(path: impl AsRef<Path>, name: &str) -> Result<Project> {
        let path = path.as_ref().to_path_buf();
        fs::create_dir_all(Self::oplog_dir(&path))?;
        fs::create_dir_all(path.join("assets"))?;
        let manifest = Manifest {
            name: name.to_string(),
            ..Default::default()
        };
        let engine = Arc::new(Engine::new());
        let p = Project {
            path,
            manifest,
            engine,
        };
        p.save()?;
        Ok(p)
    }

    /// Open an existing project: snapshot + op-log replay.
    pub fn open(path: impl AsRef<Path>) -> Result<Project> {
        let path = path.as_ref().to_path_buf();
        let manifest: Manifest =
            serde_json::from_str(&fs::read_to_string(path.join("project.json"))?)?;
        check_version(manifest.schema_version)?;
        let graph = if path.join("graph.msgpack").exists() {
            graph_from_msgpack(&fs::read(path.join("graph.msgpack"))?)?
        } else if path.join("graph.json").exists() {
            graph_from_json(&fs::read_to_string(path.join("graph.json"))?)?
        } else {
            Graph::new()
        };
        let engine = Arc::new(Engine::with_graph(graph));
        let all = read_oplog(&Self::oplog_dir(&path), 0)?;
        let pending: Vec<Delta> = all
            .iter()
            .filter(|d| d.revision > manifest.snapshot_revision)
            .cloned()
            .collect();
        engine.without_reactions(|e| e.replay(&pending))?;
        // The whole log (archived segments included) is the undo history.
        engine.rebuild_history(&all);
        Ok(Project {
            path,
            manifest,
            engine,
        })
    }

    /// Write a fresh snapshot and truncate the op log.
    pub fn save(&self) -> Result<()> {
        let graph = self.engine.snapshot();
        fs::create_dir_all(Self::oplog_dir(&self.path))?;
        fs::write(self.path.join("graph.json"), graph_to_json(&graph)?)?;
        fs::write(self.path.join("graph.msgpack"), graph_to_msgpack(&graph)?)?;
        let mut m = self.manifest.clone();
        m.snapshot_revision = graph.revision();
        fs::write(
            self.path.join("project.json"),
            serde_json::to_string_pretty(&m)?,
        )?;
        // The snapshot supersedes the current op-log segment for *replay*, but
        // those deltas are also the undo history, so the segment is archived
        // rather than deleted. `compact` drops the archives.
        let dir = Self::oplog_dir(&self.path);
        let current = dir.join("current.jsonl");
        let has_entries = fs::metadata(&current).map(|m| m.len() > 0).unwrap_or(false);
        if has_entries {
            let n = fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .file_name()
                        .map(|f| f != "current.jsonl")
                        .unwrap_or(false)
                })
                .count();
            fs::rename(&current, dir.join(format!("{n:04}.jsonl")))?;
        }
        Ok(())
    }

    /// Drop archived op-log segments. The project still opens from its
    /// snapshot, but undo history from before the last save is discarded.
    pub fn compact(&self) -> Result<usize> {
        let dir = Self::oplog_dir(&self.path);
        let mut removed = 0;
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let is_archive = path
                .file_name()
                .map(|f| f != "current.jsonl")
                .unwrap_or(false);
            if is_archive && path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                fs::remove_file(path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Append every future commit to the op log (autosave). Call once after open/create.
    pub fn enable_autosave(&self) {
        let dir = Self::oplog_dir(&self.path);
        self.engine.subscribe(move |cs| {
            let _ = append_delta(&dir, &cs.delta);
        });
    }
}

pub fn append_delta(dir: &Path, delta: &Delta) -> Result<()> {
    fs::create_dir_all(dir)?;
    let file = dir.join("current.jsonl");
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    let line = serde_json::to_string(delta)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// What recovery had to tolerate when reading an op log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OplogReport {
    /// Deltas recovered.
    pub recovered: usize,
    /// A final line that could not be parsed. This is the signature of a torn
    /// write: the process died mid-append. The partial record is discarded,
    /// which is safe because it was never a completed transaction.
    pub torn_tail: Option<String>,
}

/// Read an op log, tolerating a torn final record.
///
/// Recovery distinguishes two failure shapes deliberately:
///
/// * **A torn tail** — the last line of the last segment is truncated because
///   the process died mid-write. That record never completed, so discarding it
///   loses nothing that was ever committed. Recovery continues and reports it.
/// * **Corruption anywhere else** — a malformed line with valid lines after it
///   means the file was damaged, and later deltas may depend on the missing
///   one. Recovery fails loudly rather than silently reconstructing a graph
///   that is missing operations.
pub fn read_oplog_with_report(
    dir: &Path,
    after_revision: u64,
) -> Result<(Vec<Delta>, OplogReport)> {
    let mut out = Vec::new();
    let mut report = OplogReport::default();
    if !dir.exists() {
        return Ok((out, report));
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
        .collect();
    files.sort();

    let last_file = files.len().saturating_sub(1);
    for (fi, f) in files.iter().enumerate() {
        let reader = BufReader::new(fs::File::open(f)?);
        let lines: Vec<String> = reader.lines().collect::<std::io::Result<_>>()?;
        let last_line = lines.len().saturating_sub(1);
        for (li, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Delta>(line) {
                Ok(d) => {
                    if d.revision > after_revision {
                        out.push(d);
                    }
                }
                Err(e) => {
                    let is_tail = fi == last_file && li == last_line;
                    if is_tail {
                        // Torn write: never a completed transaction.
                        report.torn_tail = Some(format!(
                            "discarded truncated final record in {}: {e}",
                            f.display()
                        ));
                    } else {
                        return Err(UcgError::Serde(format!(
                            "corrupt op log {} line {}: {e}. Recovery stopped rather than \
                             silently dropping operations later deltas may depend on.",
                            f.display(),
                            li + 1
                        )));
                    }
                }
            }
        }
    }
    out.sort_by_key(|d| d.revision);
    report.recovered = out.len();
    Ok((out, report))
}

pub fn read_oplog(dir: &Path, after_revision: u64) -> Result<Vec<Delta>> {
    read_oplog_with_report(dir, after_revision).map(|(d, _)| d)
}
