//! Transactions: ops, deltas, inverses.
//!
//! Every mutation of the graph is expressed as a list of [`Op`]s. Ops are
//! serializable, invertible, and carry enough information to be replayed on
//! another machine (the collaboration/persistence primitive).

use crate::error::{Result, UcgError};
use crate::graph::Graph;
use crate::id::EntityId;
use crate::node::{Edge, EdgeKind, Node, NodeKind};
use crate::value::Value;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    CreateNode {
        node: Node,
    },
    /// Carries the full node and its edges so it can be undone.
    DeleteNode {
        node: Node,
        edges: Vec<Edge>,
    },
    SetAttr {
        id: EntityId,
        key: String,
        value: Value,
        prev: Option<Value>,
    },
    RemoveAttr {
        id: EntityId,
        key: String,
        prev: Value,
    },
    AddEdge {
        edge: Edge,
    },
    RemoveEdge {
        edge: Edge,
    },
}

impl Op {
    pub fn inverse(&self) -> Op {
        match self {
            Op::CreateNode { node } => Op::DeleteNode {
                node: node.clone(),
                edges: vec![],
            },
            Op::DeleteNode { node, edges } => {
                // Undo of delete = create + re-add edges; expressed as a CreateNode
                // followed by AddEdge ops by `Delta::inverse`. Here we return the
                // create op only; the caller expands edges.
                let _ = edges;
                Op::CreateNode { node: node.clone() }
            }
            Op::SetAttr {
                id,
                key,
                value,
                prev,
            } => match prev {
                Some(p) => Op::SetAttr {
                    id: *id,
                    key: key.clone(),
                    value: p.clone(),
                    prev: Some(value.clone()),
                },
                None => Op::RemoveAttr {
                    id: *id,
                    key: key.clone(),
                    prev: value.clone(),
                },
            },
            Op::RemoveAttr { id, key, prev } => Op::SetAttr {
                id: *id,
                key: key.clone(),
                value: prev.clone(),
                prev: None,
            },
            Op::AddEdge { edge } => Op::RemoveEdge { edge: edge.clone() },
            Op::RemoveEdge { edge } => Op::AddEdge { edge: edge.clone() },
        }
    }

    /// The entities this op touches (for dirty tracking).
    pub fn touched(&self) -> Vec<EntityId> {
        match self {
            Op::CreateNode { node } | Op::DeleteNode { node, .. } => vec![node.id],
            Op::SetAttr { id, .. } | Op::RemoveAttr { id, .. } => vec![*id],
            // A dependency edge changes only the dependent side; a structural
            // edge (Contains etc.) changes both endpoints.
            Op::AddEdge { edge } | Op::RemoveEdge { edge } => {
                if edge.kind.propagates() {
                    vec![edge.from]
                } else {
                    vec![edge.from, edge.to]
                }
            }
        }
    }

    pub fn is_noop(&self) -> bool {
        matches!(self, Op::SetAttr { value, prev: Some(p), .. } if value == p)
    }
}

/// Why a delta was committed. Recorded in the op log so undo history can be
/// rebuilt when a project is reopened in a new process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    /// A user action or a reactor follow-up.
    #[default]
    Normal,
    /// The inverse of a previously committed undo group.
    Undo,
    /// The re-application of a previously undone group.
    Redo,
}

/// A committed group of ops.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// Revision the graph is at *after* applying this delta.
    pub revision: u64,
    pub label: String,
    pub ops: Vec<Op>,
    /// Actor that produced it (user id / machine id), for collaboration.
    #[serde(default)]
    pub actor: String,
    /// Lamport-style logical clock for merge ordering.
    #[serde(default)]
    pub lamport: u64,
    /// The transaction that caused this one (reactor cascades), if any.
    #[serde(default)]
    pub cause: Option<u64>,
    /// Cascade depth (0 = user action).
    #[serde(default)]
    pub depth: u32,
    /// Whether this delta is a normal commit, an undo, or a redo.
    #[serde(default)]
    pub kind: DeltaKind,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Inverse delta in reverse order, expanding node deletions into
    /// create + edge restoration.
    pub fn inverse(&self) -> Vec<Op> {
        let mut out = Vec::with_capacity(self.ops.len());
        for op in self.ops.iter().rev() {
            match op {
                Op::DeleteNode { node, edges } => {
                    out.push(Op::CreateNode { node: node.clone() });
                    for e in edges {
                        out.push(Op::AddEdge { edge: e.clone() });
                    }
                }
                other => out.push(other.inverse()),
            }
        }
        out
    }

    pub fn touched(&self) -> Vec<EntityId> {
        let mut v: Vec<EntityId> = self.ops.iter().flat_map(Op::touched).collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Builder used inside `Engine::transact`. It applies ops to a scratch copy
/// of the graph immediately (so reads within the transaction see writes) and
/// records the ops for commit.
pub struct Tx<'g> {
    pub(crate) graph: &'g mut Graph,
    pub(crate) ops: Vec<Op>,
}

impl<'g> Tx<'g> {
    pub(crate) fn new(graph: &'g mut Graph) -> Self {
        Tx {
            graph,
            ops: Vec::new(),
        }
    }

    /// Read access to the graph as it looks with this transaction's ops applied.
    pub fn graph(&self) -> &Graph {
        self.graph
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    fn apply(&mut self, op: Op) -> Result<()> {
        apply_op(self.graph, &op)?;
        self.ops.push(op);
        Ok(())
    }

    /// Create a node with a fresh id.
    pub fn create(&mut self, kind: impl Into<NodeKind>) -> Result<EntityId> {
        let node = Node::new(EntityId::new(), kind);
        let id = node.id;
        self.apply(Op::CreateNode { node })?;
        Ok(id)
    }

    /// Create a node with a specific (e.g. derived) id.
    pub fn create_with_id(&mut self, id: EntityId, kind: impl Into<NodeKind>) -> Result<EntityId> {
        self.apply(Op::CreateNode {
            node: Node::new(id, kind),
        })?;
        Ok(id)
    }

    /// Create the node if it does not exist; returns whether it was created.
    pub fn ensure(&mut self, id: EntityId, kind: impl Into<NodeKind>) -> Result<bool> {
        if self.graph.contains(id) {
            return Ok(false);
        }
        self.create_with_id(id, kind)?;
        Ok(true)
    }

    /// Insert a fully-formed node (attrs included).
    pub fn insert(&mut self, node: Node) -> Result<EntityId> {
        let id = node.id;
        self.apply(Op::CreateNode { node })?;
        Ok(id)
    }

    /// Delete a node and all its edges. Children (Contains) are deleted recursively.
    pub fn delete(&mut self, id: EntityId) -> Result<()> {
        let kids = self.graph.children(id);
        for k in kids {
            self.delete(k)?;
        }
        let node = self.graph.require(id)?.clone();
        let edges: Vec<Edge> = self
            .graph
            .out_edges(id)
            .iter()
            .chain(self.graph.in_edges(id).iter())
            .cloned()
            .collect();
        self.apply(Op::DeleteNode { node, edges })
    }

    /// Set an attribute. Setting an identical value is a no-op (idempotence
    /// is what keeps reactor cascades from looping).
    pub fn set(&mut self, id: EntityId, key: &str, value: impl Into<Value>) -> Result<()> {
        let value = value.into();
        let prev = self.graph.attr(id, key).cloned();
        if !self.graph.contains(id) {
            return Err(UcgError::NotFound(id));
        }
        if prev.as_ref() == Some(&value) {
            return Ok(());
        }
        self.apply(Op::SetAttr {
            id,
            key: key.to_string(),
            value,
            prev,
        })
    }

    /// Set a typed component (serialized to a `Value` tree).
    pub fn set_as<T: Serialize>(&mut self, id: EntityId, key: &str, value: &T) -> Result<()> {
        self.set(id, key, Value::from_serde(value)?)
    }

    pub fn remove_attr(&mut self, id: EntityId, key: &str) -> Result<()> {
        match self.graph.attr(id, key).cloned() {
            Some(prev) => self.apply(Op::RemoveAttr {
                id,
                key: key.to_string(),
                prev,
            }),
            None => Ok(()),
        }
    }

    /// Add an edge; duplicate edges are a no-op.
    pub fn link(&mut self, from: EntityId, to: EntityId, kind: EdgeKind) -> Result<()> {
        if self.graph.has_edge(from, to, kind) {
            return Ok(());
        }
        self.apply(Op::AddEdge {
            edge: Edge::new(from, to, kind),
        })
    }

    pub fn unlink(&mut self, from: EntityId, to: EntityId, kind: EdgeKind) -> Result<()> {
        if !self.graph.has_edge(from, to, kind) {
            return Ok(());
        }
        self.apply(Op::RemoveEdge {
            edge: Edge::new(from, to, kind),
        })
    }

    /// Convenience: create `kind` as a child of `parent` with `order` = current child count.
    pub fn create_child(
        &mut self,
        parent: EntityId,
        kind: impl Into<NodeKind>,
    ) -> Result<EntityId> {
        let order = self.graph.children(parent).len() as i64;
        let id = self.create(kind)?;
        self.set(id, "order", order)?;
        self.link(parent, id, EdgeKind::Contains)?;
        Ok(id)
    }

    /// Re-parent `id` under `new_parent` (removing any previous Contains edge).
    pub fn reparent(&mut self, id: EntityId, new_parent: EntityId) -> Result<()> {
        for p in self.graph.sources(id, EdgeKind::Contains) {
            self.unlink(p, id, EdgeKind::Contains)?;
        }
        self.link(new_parent, id, EdgeKind::Contains)
    }

    /// Replay raw ops (used by undo/redo and op-log loading).
    pub fn apply_ops(&mut self, ops: &[Op]) -> Result<()> {
        for op in ops {
            self.apply(op.clone())?;
        }
        Ok(())
    }
}

/// Apply a single op to a graph. Used by `Tx`, undo/redo, and op-log replay.
pub(crate) fn apply_op(graph: &mut Graph, op: &Op) -> Result<()> {
    match op {
        Op::CreateNode { node } => graph.insert_node(node.clone()),
        Op::DeleteNode { node, .. } => graph.remove_node(node.id).map(|_| ()),
        Op::SetAttr { id, key, value, .. } => graph.set_attr(*id, key, value.clone()).map(|_| ()),
        Op::RemoveAttr { id, key, .. } => graph.remove_attr(*id, key).map(|_| ()),
        Op::AddEdge { edge } => graph.insert_edge(edge.clone()).map(|_| ()),
        Op::RemoveEdge { edge } => {
            graph.delete_edge(edge);
            Ok(())
        }
    }
}
