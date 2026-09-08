//! In-memory graph storage with dependency-DAG maintenance and queries.
//!
//! `Graph` is a plain data structure; it is not internally synchronized. The
//! [`crate::engine::Engine`] wraps it in a `RwLock`, runs transactions, and
//! publishes change sets. Domain code normally reads through `&Graph` guards.

use crate::error::{Result, UcgError};
use crate::id::EntityId;
use crate::node::{Edge, EdgeKind, Node, NodeKind};
use crate::value::Value;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet, VecDeque};

type EdgeList = SmallVec<[Edge; 4]>;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Graph {
    pub(crate) nodes: IndexMap<EntityId, Node>,
    /// Outgoing edges keyed by `from`.
    #[serde(skip)]
    pub(crate) out_edges: HashMap<EntityId, EdgeList>,
    /// Incoming edges keyed by `to`.
    #[serde(skip)]
    pub(crate) in_edges: HashMap<EntityId, EdgeList>,
    /// Flat edge list (the serialized form; adjacency is rebuilt on load).
    pub(crate) edges: Vec<Edge>,
    #[serde(skip)]
    pub(crate) by_kind: HashMap<NodeKind, HashSet<EntityId>>,
    pub(crate) revision: u64,
}

impl std::fmt::Debug for Graph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graph")
            .field("nodes", &self.nodes.len())
            .field("edges", &self.edges.len())
            .field("revision", &self.revision)
            .finish()
    }
}

impl Graph {
    pub fn new() -> Self {
        Graph::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    // ---- reads --------------------------------------------------------------

    pub fn contains(&self, id: EntityId) -> bool {
        self.nodes.contains_key(&id)
    }
    pub fn node(&self, id: EntityId) -> Option<&Node> {
        self.nodes.get(&id)
    }
    pub fn require(&self, id: EntityId) -> Result<&Node> {
        self.nodes.get(&id).ok_or(UcgError::NotFound(id))
    }
    pub fn attr(&self, id: EntityId, key: &str) -> Option<&Value> {
        self.nodes.get(&id).and_then(|n| n.attrs.get(key))
    }
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }
    pub fn ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.nodes.keys().copied()
    }
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// All nodes of a given kind, in insertion order.
    pub fn of_kind(&self, kind: &str) -> Vec<&Node> {
        let k = NodeKind::new(kind);
        let mut v: Vec<&Node> = self
            .by_kind
            .get(&k)
            .map(|s| s.iter().filter_map(|id| self.nodes.get(id)).collect())
            .unwrap_or_default();
        v.sort_by_key(|n| self.nodes.get_index_of(&n.id));
        v
    }

    /// All nodes whose kind starts with `prefix` (e.g. `"nle."`).
    pub fn of_module(&self, prefix: &str) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.kind.as_str().starts_with(prefix))
            .collect()
    }

    pub fn out_edges(&self, id: EntityId) -> &[Edge] {
        self.out_edges.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn in_edges(&self, id: EntityId) -> &[Edge] {
        self.in_edges.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn has_edge(&self, from: EntityId, to: EntityId, kind: EdgeKind) -> bool {
        self.out_edges(from)
            .iter()
            .any(|e| e.to == to && e.kind == kind)
    }

    /// Targets of outgoing edges of `kind` from `id`.
    pub fn targets(&self, id: EntityId, kind: EdgeKind) -> Vec<EntityId> {
        self.out_edges(id)
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.to)
            .collect()
    }
    /// Sources of incoming edges of `kind` into `id`.
    pub fn sources(&self, id: EntityId, kind: EdgeKind) -> Vec<EntityId> {
        self.in_edges(id)
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.from)
            .collect()
    }

    /// Children in the containment hierarchy (ordered by an optional `order` attr, then insertion).
    pub fn children(&self, id: EntityId) -> Vec<EntityId> {
        let mut kids = self.targets(id, EdgeKind::Contains);
        kids.sort_by(|a, b| {
            let oa = self.attr(*a, "order").and_then(Value::as_int);
            let ob = self.attr(*b, "order").and_then(Value::as_int);
            oa.cmp(&ob)
                .then_with(|| self.nodes.get_index_of(a).cmp(&self.nodes.get_index_of(b)))
        });
        kids
    }
    pub fn parent(&self, id: EntityId) -> Option<EntityId> {
        self.sources(id, EdgeKind::Contains).into_iter().next()
    }
    /// Walk up the containment hierarchy until a node of `kind` is found.
    pub fn ancestor_of_kind(&self, id: EntityId, kind: &str) -> Option<EntityId> {
        let mut cur = self.parent(id);
        let mut guard = 0;
        while let Some(p) = cur {
            if self
                .node(p)
                .map(|n| n.kind.as_str() == kind)
                .unwrap_or(false)
            {
                return Some(p);
            }
            cur = self.parent(p);
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        None
    }
    /// Depth-first list of all descendants (containment).
    pub fn descendants(&self, id: EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        let mut stack = self.children(id);
        stack.reverse();
        while let Some(n) = stack.pop() {
            out.push(n);
            let mut kids = self.children(n);
            kids.reverse();
            stack.extend(kids);
        }
        out
    }

    /// Everything reachable from `id` (id excluded) along propagating edges,
    /// following *incoming* edges: if `X DependsOn id` then X is a dependent.
    pub fn dependents(&self, id: EntityId) -> Vec<EntityId> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut q = VecDeque::from([id]);
        while let Some(n) = q.pop_front() {
            for e in self.in_edges(n) {
                if e.kind.propagates() && seen.insert(e.from) {
                    out.push(e.from);
                    q.push_back(e.from);
                }
            }
        }
        out
    }

    /// Nodes that `id` depends on, transitively.
    pub fn dependencies(&self, id: EntityId) -> Vec<EntityId> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut q = VecDeque::from([id]);
        while let Some(n) = q.pop_front() {
            for e in self.out_edges(n) {
                if e.kind.propagates() && seen.insert(e.to) {
                    out.push(e.to);
                    q.push_back(e.to);
                }
            }
        }
        out
    }

    /// Would adding `from -> to` (propagating) create a cycle? True if `to`
    /// already (transitively) depends on `from`.
    pub fn would_cycle(&self, from: EntityId, to: EntityId) -> bool {
        if from == to {
            return true;
        }
        let mut seen = HashSet::new();
        let mut q = VecDeque::from([to]);
        while let Some(n) = q.pop_front() {
            for e in self.out_edges(n) {
                if e.kind.propagates() {
                    if e.to == from {
                        return true;
                    }
                    if seen.insert(e.to) {
                        q.push_back(e.to);
                    }
                }
            }
        }
        false
    }

    /// Topologically order a set of ids so that dependencies come before
    /// dependents (Kahn's algorithm restricted to the induced subgraph).
    pub fn topo_order(&self, ids: &[EntityId]) -> Vec<EntityId> {
        let set: HashSet<EntityId> = ids.iter().copied().collect();
        let mut indeg: HashMap<EntityId, usize> = set.iter().map(|id| (*id, 0)).collect();
        for id in &set {
            for e in self.out_edges(*id) {
                if e.kind.propagates() && set.contains(&e.to) {
                    *indeg.get_mut(id).unwrap() += 1;
                }
            }
        }
        // Stable start order: input order.
        let mut ready: VecDeque<EntityId> =
            ids.iter().copied().filter(|id| indeg[id] == 0).collect();
        let mut out = Vec::with_capacity(ids.len());
        let mut emitted = HashSet::new();
        while let Some(n) = ready.pop_front() {
            if !emitted.insert(n) {
                continue;
            }
            out.push(n);
            for e in self.in_edges(n) {
                if e.kind.propagates() && set.contains(&e.from) {
                    let d = indeg.get_mut(&e.from).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        ready.push_back(e.from);
                    }
                }
            }
        }
        // Any leftovers (only possible if a cycle slipped in via non-validated load) are appended.
        for id in ids {
            if !emitted.contains(id) {
                out.push(*id);
            }
        }
        out
    }

    /// Full-graph topological order of every node (dependencies first).
    pub fn topo_order_all(&self) -> Vec<EntityId> {
        let ids: Vec<EntityId> = self.nodes.keys().copied().collect();
        self.topo_order(&ids)
    }

    // ---- raw writes (used by the transaction machinery) ---------------------

    pub(crate) fn insert_node(&mut self, mut node: Node) -> Result<()> {
        if self.nodes.contains_key(&node.id) {
            return Err(UcgError::AlreadyExists(node.id));
        }
        node.created_rev = self.revision;
        node.modified_rev = self.revision;
        self.by_kind
            .entry(node.kind.clone())
            .or_default()
            .insert(node.id);
        self.nodes.insert(node.id, node);
        Ok(())
    }

    pub(crate) fn remove_node(&mut self, id: EntityId) -> Result<(Node, Vec<Edge>)> {
        let node = self.nodes.shift_remove(&id).ok_or(UcgError::NotFound(id))?;
        if let Some(s) = self.by_kind.get_mut(&node.kind) {
            s.remove(&id);
        }
        let mut removed = Vec::new();
        if let Some(outs) = self.out_edges.remove(&id) {
            for e in outs {
                if let Some(ins) = self.in_edges.get_mut(&e.to) {
                    ins.retain(|x| x != &e);
                }
                removed.push(e);
            }
        }
        if let Some(ins) = self.in_edges.remove(&id) {
            for e in ins {
                if let Some(outs) = self.out_edges.get_mut(&e.from) {
                    outs.retain(|x| x != &e);
                }
                removed.push(e);
            }
        }
        self.edges.retain(|e| e.from != id && e.to != id);
        Ok((node, removed))
    }

    pub(crate) fn set_attr(
        &mut self,
        id: EntityId,
        key: &str,
        value: Value,
    ) -> Result<Option<Value>> {
        let rev = self.revision;
        let node = self.nodes.get_mut(&id).ok_or(UcgError::NotFound(id))?;
        node.modified_rev = rev;
        Ok(node.attrs.insert(key.to_string(), value))
    }

    pub(crate) fn remove_attr(&mut self, id: EntityId, key: &str) -> Result<Option<Value>> {
        let rev = self.revision;
        let node = self.nodes.get_mut(&id).ok_or(UcgError::NotFound(id))?;
        node.modified_rev = rev;
        Ok(node.attrs.shift_remove(key))
    }

    pub(crate) fn insert_edge(&mut self, edge: Edge) -> Result<bool> {
        if !self.nodes.contains_key(&edge.from) {
            return Err(UcgError::NotFound(edge.from));
        }
        if !self.nodes.contains_key(&edge.to) {
            return Err(UcgError::NotFound(edge.to));
        }
        if self.has_edge(edge.from, edge.to, edge.kind) {
            return Ok(false);
        }
        if edge.kind.propagates() && self.would_cycle(edge.from, edge.to) {
            return Err(UcgError::Cycle {
                from: edge.from,
                to: edge.to,
                kind: edge.kind.to_string(),
            });
        }
        self.out_edges
            .entry(edge.from)
            .or_default()
            .push(edge.clone());
        self.in_edges.entry(edge.to).or_default().push(edge.clone());
        self.edges.push(edge);
        Ok(true)
    }

    pub(crate) fn delete_edge(&mut self, edge: &Edge) -> bool {
        let before = self.edges.len();
        self.edges.retain(|e| e != edge);
        if let Some(v) = self.out_edges.get_mut(&edge.from) {
            v.retain(|e| e != edge);
        }
        if let Some(v) = self.in_edges.get_mut(&edge.to) {
            v.retain(|e| e != edge);
        }
        self.edges.len() != before
    }

    /// Rebuild adjacency and kind indices from the flat lists (after deserialization).
    pub(crate) fn rebuild_indices(&mut self) {
        self.out_edges.clear();
        self.in_edges.clear();
        self.by_kind.clear();
        for n in self.nodes.values() {
            self.by_kind.entry(n.kind.clone()).or_default().insert(n.id);
        }
        let edges = std::mem::take(&mut self.edges);
        for e in edges {
            self.out_edges.entry(e.from).or_default().push(e.clone());
            self.in_edges.entry(e.to).or_default().push(e.clone());
            self.edges.push(e);
        }
    }

    /// Structural validation: every edge endpoint exists, kinds indexed, no
    /// dependency cycles. Returns a list of problems (empty = valid).
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for e in &self.edges {
            if !self.nodes.contains_key(&e.from) {
                problems.push(format!("edge {e:?}: missing source"));
            }
            if !self.nodes.contains_key(&e.to) {
                problems.push(format!("edge {e:?}: missing target"));
            }
        }
        // Cycle check over propagating edges using DFS colors.
        let mut color: HashMap<EntityId, u8> = HashMap::new();
        for start in self.nodes.keys() {
            if color.get(start).copied().unwrap_or(0) != 0 {
                continue;
            }
            let mut stack: Vec<(EntityId, usize)> = vec![(*start, 0)];
            color.insert(*start, 1);
            while let Some((n, i)) = stack.last_mut() {
                let outs = self.out_edges(*n);
                let mut advanced = false;
                while *i < outs.len() {
                    let e = &outs[*i];
                    *i += 1;
                    if !e.kind.propagates() {
                        continue;
                    }
                    match color.get(&e.to).copied().unwrap_or(0) {
                        0 => {
                            color.insert(e.to, 1);
                            let next = e.to;
                            stack.push((next, 0));
                            advanced = true;
                            break;
                        }
                        1 => problems.push(format!("dependency cycle through {} -> {}", n, e.to)),
                        _ => {}
                    }
                }
                if !advanced {
                    let (done, _) = stack.pop().unwrap();
                    color.insert(done, 2);
                }
            }
        }
        problems
    }
}
