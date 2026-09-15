//! What a change did: the impact of one user action across departments, and
//! the shared history.
//!
//! Everything here is read from the deltas the engine committed — the user's
//! transaction and every reactor transaction it caused. Nothing is inferred. If
//! the schedule did not change, the impact does not say it might have; if a
//! clip was flagged for conform, the reason shown is the one the reactor wrote.

use serde_json::{json, Value as J};
use std::collections::{BTreeMap, BTreeSet};
use ucg_core::EntityId;
use ucg_core::{Delta, Graph, History, Op, UndoGroup};
use ucg_schema::{attrs, kinds};

/// The department a node kind belongs to, as a person on a production would
/// name it. Unknown modules are shown by their own name rather than hidden.
pub fn department(kind: &str) -> &str {
    match kind.split('.').next().unwrap_or("") {
        "script" => "Script",
        "previs" | "board" => "Previs",
        "logistics" => "Production",
        "nle" | "sync" => "Editorial",
        "asset" | "media" => "Media",
        "comp" => "VFX",
        "color" => "Colour",
        "audio" => "Sound",
        "project" => "Project",
        other => other,
    }
}

#[derive(Default)]
struct Touch {
    kind: String,
    name: Option<String>,
    created: bool,
    deleted: bool,
    attrs: BTreeSet<String>,
    edges: BTreeSet<String>,
    reason: Option<String>,
    conform: Option<bool>,
}

fn ids_of(op: &Op) -> Vec<EntityId> {
    match op {
        Op::CreateNode { node } | Op::DeleteNode { node, .. } => vec![node.id],
        Op::SetAttr { id, .. } | Op::RemoveAttr { id, .. } => vec![*id],
        Op::AddEdge { edge } | Op::RemoveEdge { edge } => vec![edge.from],
    }
}

/// Collate the entities a set of deltas touched.
fn collate<'a>(
    graph: &Graph,
    deltas: impl Iterator<Item = &'a Delta>,
) -> BTreeMap<EntityId, Touch> {
    let mut out: BTreeMap<EntityId, Touch> = BTreeMap::new();
    for d in deltas {
        for op in &d.ops {
            for id in ids_of(op) {
                let t = out.entry(id).or_default();
                match op {
                    Op::CreateNode { node } => {
                        t.created = true;
                        t.deleted = false;
                        t.kind = node.kind.as_str().to_string();
                    }
                    Op::DeleteNode { node, .. } => {
                        t.deleted = true;
                        t.kind = node.kind.as_str().to_string();
                        t.name = node.get_str(attrs::NAME).map(str::to_string);
                    }
                    Op::SetAttr { key, value, .. } => {
                        t.attrs.insert(key.clone());
                        if key == attrs::DIRTY_REASON {
                            t.reason = value.as_str().map(str::to_string);
                        }
                        if key == attrs::NEEDS_CONFORM {
                            t.conform = value.as_bool();
                        }
                    }
                    Op::RemoveAttr { key, .. } => {
                        t.attrs.insert(key.clone());
                        if key == attrs::DIRTY_REASON {
                            t.reason = None;
                        }
                    }
                    Op::AddEdge { edge } => {
                        t.edges
                            .insert(format!("+{} {}", edge.kind.as_str(), edge.to));
                    }
                    Op::RemoveEdge { edge } => {
                        t.edges
                            .insert(format!("-{} {}", edge.kind.as_str(), edge.to));
                    }
                }
            }
        }
    }
    for (id, t) in out.iter_mut() {
        if let Some(n) = graph.node(*id) {
            if t.kind.is_empty() {
                t.kind = n.kind.as_str().to_string();
            }
            if t.name.is_none() {
                t.name = n
                    .get_str(attrs::NAME)
                    .or_else(|| n.get_str(attrs::HEADING))
                    .or_else(|| n.get_str(attrs::TEXT))
                    .map(|s| s.chars().take(80).collect());
            }
        }
    }
    out
}

fn step(d: &Delta) -> J {
    let (department, action) = match d.label.split_once(": ") {
        Some((reactor, plan)) if d.depth > 0 => (reactor.to_string(), plan.to_string()),
        _ => ("user".to_string(), d.label.clone()),
    };
    json!({
        "revision": d.revision,
        "depth": d.depth,
        "cause": d.cause,
        "by": department,
        "action": action,
        "ops": d.ops.len(),
        "actor": d.actor,
    })
}

/// The impact of an undo group, grouped by department.
pub fn of_group(graph: &Graph, group: &UndoGroup) -> J {
    let touched = collate(graph, group.deltas.iter().map(|d| d.as_ref()));
    let mut departments: BTreeMap<String, Vec<J>> = BTreeMap::new();
    let mut conform = Vec::new();
    for (id, t) in &touched {
        // Markers, streams and the like are details of their owner, not
        // separate consequences a person needs to read.
        if t.kind == kinds::NLE_MARKER {
            continue;
        }
        let change = if t.created {
            "created"
        } else if t.deleted {
            "deleted"
        } else {
            "changed"
        };
        if t.kind == kinds::NLE_CLIP && t.conform == Some(true) {
            conform.push(json!({ "clip": id, "name": t.name, "reason": t.reason }));
        }
        departments
            .entry(department(&t.kind).to_string())
            .or_default()
            .push(json!({
                "id": id,
                "kind": t.kind,
                "name": t.name,
                "change": change,
                "attrs": t.attrs,
                "edges": t.edges.len(),
                "reason": t.reason,
            }));
    }
    json!({
        "label": group.label,
        "revisions": group.deltas.iter().map(|d| d.revision).collect::<Vec<_>>(),
        "steps": group.deltas.iter().map(|d| step(d)).collect::<Vec<_>>(),
        "departments": departments.into_iter().map(|(name, entities)| json!({
            "department": name,
            "count": entities.len(),
            "entities": entities,
        })).collect::<Vec<_>>(),
        "conform": conform,
        "entities": touched.len(),
    })
}

/// The group containing `revision`, searching undo then redo, newest first.
pub fn find_group(history: &History, revision: u64) -> Option<&UndoGroup> {
    history
        .undo
        .iter()
        .rev()
        .chain(history.redo.iter().rev())
        .find(|g| g.deltas.iter().any(|d| d.revision == revision))
}

/// The shared history: undo stack oldest first, then what redo would restore.
pub fn history(history: &History, limit: usize) -> J {
    let entry = |g: &UndoGroup| {
        json!({
            "label": g.label,
            "first": g.deltas.first().map(|d| d.revision),
            "last": g.deltas.last().map(|d| d.revision),
            "deltas": g.deltas.len(),
            "cascade": g.deltas.iter().filter(|d| d.depth > 0).count(),
            "actor": g.deltas.first().map(|d| d.actor.clone()),
        })
    };
    let skip = history.undo.len().saturating_sub(limit);
    json!({
        "undo": history.undo.iter().skip(skip).map(entry).collect::<Vec<_>>(),
        "redo": history.redo.iter().rev().take(limit).map(entry).collect::<Vec<_>>(),
        "undo_depth": history.undo.len(),
        "redo_depth": history.redo.len(),
    })
}
