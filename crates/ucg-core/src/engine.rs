//! The reactive engine: transactions, undo/redo, change publication, and the
//! reactor cascade that keeps every module consistent with every other.

use crate::error::{Result, UcgError};
use crate::graph::Graph;
use crate::id::EntityId;
use crate::node::NodeKind;
use crate::tx::{apply_op, Delta, DeltaKind, Op, Tx};
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Published after every committed transaction.
#[derive(Clone, Debug)]
pub struct ChangeSet {
    pub delta: Arc<Delta>,
    /// Entities directly touched by the ops.
    pub touched: Vec<EntityId>,
    /// Touched ∪ all transitive dependents, in topological order
    /// (dependencies first). This is what reactors iterate.
    pub dirty: Vec<EntityId>,
    dirty_set: HashSet<EntityId>,
    /// Kinds of touched nodes (as known at commit time; deleted nodes included).
    pub kinds: HashSet<NodeKind>,
    /// (entity, attr) pairs whose value changed.
    attrs: HashSet<(EntityId, String)>,
    created: HashSet<EntityId>,
    deleted: HashSet<EntityId>,
}

impl ChangeSet {
    pub fn revision(&self) -> u64 {
        self.delta.revision
    }
    pub fn depth(&self) -> u32 {
        self.delta.depth
    }
    pub fn is_dirty(&self, id: EntityId) -> bool {
        self.dirty_set.contains(&id)
    }
    pub fn attr_changed(&self, id: EntityId, key: &str) -> bool {
        self.attrs.contains(&(id, key.to_string()))
    }
    /// Any attribute of `id` changed (not counting creation/edges).
    pub fn any_attr_changed(&self, id: EntityId) -> bool {
        self.attrs.iter().any(|(e, _)| *e == id)
    }
    pub fn changed_attrs(&self, id: EntityId) -> Vec<&str> {
        self.attrs
            .iter()
            .filter(|(e, _)| *e == id)
            .map(|(_, k)| k.as_str())
            .collect()
    }
    pub fn was_created(&self, id: EntityId) -> bool {
        self.created.contains(&id)
    }
    pub fn was_deleted(&self, id: EntityId) -> bool {
        self.deleted.contains(&id)
    }
    pub fn created(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.created.iter().copied()
    }
    pub fn deleted(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.deleted.iter().copied()
    }
    /// Did this change touch any node of a kind with the given prefix (`"script."`)?
    pub fn touches_module(&self, prefix: &str) -> bool {
        self.kinds.iter().any(|k| k.as_str().starts_with(prefix))
    }
    pub fn touches_kind(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k.as_str() == kind)
    }
    /// Dirty entities of a given kind (looked up in the current graph).
    pub fn dirty_of_kind<'a>(
        &'a self,
        graph: &'a Graph,
        kind: &'a str,
    ) -> impl Iterator<Item = EntityId> + 'a {
        self.dirty.iter().copied().filter(move |id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kind)
                .unwrap_or(false)
        })
    }

    fn build(delta: Arc<Delta>, graph: &Graph, kinds_before: HashSet<NodeKind>) -> Self {
        let touched = delta.touched();
        let mut attrs = HashSet::new();
        let mut created = HashSet::new();
        let mut deleted = HashSet::new();
        let mut kinds = kinds_before;
        for op in &delta.ops {
            match op {
                Op::SetAttr { id, key, .. } | Op::RemoveAttr { id, key, .. } => {
                    attrs.insert((*id, key.clone()));
                }
                Op::CreateNode { node } => {
                    created.insert(node.id);
                    kinds.insert(node.kind.clone());
                }
                Op::DeleteNode { node, .. } => {
                    deleted.insert(node.id);
                    kinds.insert(node.kind.clone());
                }
                _ => {}
            }
        }
        for id in &touched {
            if let Some(n) = graph.node(*id) {
                kinds.insert(n.kind.clone());
            }
        }
        let mut dirty_set: HashSet<EntityId> = touched.iter().copied().collect();
        let mut all: Vec<EntityId> = touched.clone();
        for id in &touched {
            for d in graph.dependents(*id) {
                if dirty_set.insert(d) {
                    all.push(d);
                }
            }
        }
        let dirty = graph.topo_order(&all);
        ChangeSet {
            delta,
            touched,
            dirty,
            dirty_set,
            kinds,
            attrs,
            created,
            deleted,
        }
    }
}

/// The body of a [`Plan`]: a one-shot mutation run inside a transaction.
pub type PlanFn = Box<dyn FnOnce(&mut Tx) -> Result<()> + Send>;

/// A deferred mutation produced by a reactor, executed by the engine.
pub struct Plan {
    pub label: String,
    pub run: PlanFn,
}

impl Plan {
    pub fn new(
        label: impl Into<String>,
        f: impl FnOnce(&mut Tx) -> Result<()> + Send + 'static,
    ) -> Self {
        Plan {
            label: label.into(),
            run: Box::new(f),
        }
    }
}

/// A module's reaction to changes. Reactors are pure: they read the graph and
/// return plans; the engine applies plans as cascaded transactions.
pub trait Reactor: Send + Sync {
    fn name(&self) -> &str;
    /// Quick filter: return false to skip `react` entirely for this change.
    fn interested(&self, cs: &ChangeSet) -> bool {
        let _ = cs;
        true
    }
    fn react(&self, cs: &ChangeSet, graph: &Graph) -> Result<Vec<Plan>>;
}

type Subscriber = Box<dyn Fn(&ChangeSet) + Send + Sync>;

/// One user-level action and every cascaded reactor transaction it caused;
/// undone/redone as a unit.
#[derive(Clone, Debug, Default)]
pub struct UndoGroup {
    pub label: String,
    pub deltas: Vec<Arc<Delta>>,
}

pub struct Engine {
    graph: RwLock<Graph>,
    reactors: RwLock<Vec<Arc<dyn Reactor>>>,
    subscribers: Mutex<Vec<Subscriber>>,
    undo: Mutex<Vec<UndoGroup>>,
    redo: Mutex<Vec<UndoGroup>>,
    /// Every delta ever committed in this session (the op log).
    log: Mutex<Vec<Arc<Delta>>>,
    actor: String,
    lamport: Mutex<u64>,
    pub max_cascade_depth: u32,
    /// Whether committed transactions trigger reactors (disabled during undo/replay).
    reacting: Mutex<bool>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Engine::with_graph(Graph::new())
    }

    pub fn with_graph(graph: Graph) -> Self {
        Engine {
            graph: RwLock::new(graph),
            reactors: RwLock::new(Vec::new()),
            subscribers: Mutex::new(Vec::new()),
            undo: Mutex::new(Vec::new()),
            redo: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            actor: "local".to_string(),
            lamport: Mutex::new(0),
            max_cascade_depth: 8,
            reacting: Mutex::new(true),
        }
    }

    pub fn set_actor(&mut self, actor: impl Into<String>) {
        self.actor = actor.into();
    }

    /// Shared read access. Hold the guard only briefly in interactive paths.
    pub fn read(&self) -> RwLockReadGuard<'_, Graph> {
        self.graph.read()
    }

    /// Clone of the whole graph (for snapshots / off-thread evaluation).
    pub fn snapshot(&self) -> Graph {
        let mut g = self.graph.read().clone();
        g.rebuild_indices();
        g
    }

    pub fn revision(&self) -> u64 {
        self.graph.read().revision()
    }

    pub fn add_reactor(&self, r: Arc<dyn Reactor>) {
        self.reactors.write().push(r);
    }

    pub fn reactor_names(&self) -> Vec<String> {
        self.reactors
            .read()
            .iter()
            .map(|r| r.name().to_string())
            .collect()
    }

    pub fn subscribe(&self, f: impl Fn(&ChangeSet) + Send + Sync + 'static) {
        self.subscribers.lock().push(Box::new(f));
    }

    /// Channel-based subscription (for UI threads).
    pub fn subscribe_channel(&self) -> crossbeam_channel::Receiver<ChangeSet> {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.subscribe(move |cs| {
            let _ = tx.send(cs.clone());
        });
        rx
    }

    pub fn log(&self) -> Vec<Arc<Delta>> {
        self.log.lock().clone()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.lock().len()
    }
    pub fn redo_depth(&self) -> usize {
        self.redo.lock().len()
    }

    // ---- transactions -------------------------------------------------------

    /// Run a user-level transaction, then the reactor cascade. Returns the
    /// undo group (all deltas). An empty transaction commits nothing.
    pub fn transact<F>(&self, label: &str, f: F) -> Result<UndoGroup>
    where
        F: FnOnce(&mut Tx) -> Result<()>,
    {
        let first = self.commit(label, 0, None, f)?;
        let mut group = UndoGroup {
            label: label.to_string(),
            deltas: Vec::new(),
        };
        if let Some(cs) = first {
            group.deltas.push(cs.delta.clone());
            if *self.reacting.lock() {
                self.cascade(cs, &mut group)?;
            }
        }
        if !group.deltas.is_empty() {
            self.undo.lock().push(group.clone());
            self.redo.lock().clear();
        }
        Ok(group)
    }

    /// Commit without triggering the reactor cascade or undo bookkeeping
    /// (used for op-log replay and undo/redo).
    fn commit<F>(
        &self,
        label: &str,
        depth: u32,
        cause: Option<u64>,
        f: F,
    ) -> Result<Option<ChangeSet>>
    where
        F: FnOnce(&mut Tx) -> Result<()>,
    {
        self.commit_kind(label, depth, cause, DeltaKind::Normal, f)
    }

    /// `commit` with an explicit [`DeltaKind`] (undo and redo record their own).
    fn commit_kind<F>(
        &self,
        label: &str,
        depth: u32,
        cause: Option<u64>,
        kind: DeltaKind,
        f: F,
    ) -> Result<Option<ChangeSet>>
    where
        F: FnOnce(&mut Tx) -> Result<()>,
    {
        let mut g = self.graph.write();
        let kinds_before: HashSet<NodeKind> = HashSet::new();
        // Apply in place; on error roll back what was applied.
        let ops = {
            let mut tx = Tx::new(&mut g);
            match f(&mut tx) {
                Ok(()) => std::mem::take(&mut tx.ops),
                Err(e) => {
                    let applied = std::mem::take(&mut tx.ops);
                    let rollback = Delta {
                        ops: applied,
                        ..Default::default()
                    }
                    .inverse();
                    for op in &rollback {
                        // Rollback must not fail; ignore individual errors.
                        let _ = apply_op(&mut g, op);
                    }
                    return Err(e);
                }
            }
        };
        let ops: Vec<Op> = ops.into_iter().filter(|o| !o.is_noop()).collect();
        if ops.is_empty() {
            return Ok(None);
        }
        // Record kinds of nodes touched *before* deletion so reactors see them.
        let mut kinds = kinds_before;
        for op in &ops {
            if let Op::DeleteNode { node, .. } = op {
                kinds.insert(node.kind.clone());
            }
        }
        g.revision += 1;
        let lamport = {
            let mut l = self.lamport.lock();
            *l += 1;
            *l
        };
        let delta = Arc::new(Delta {
            revision: g.revision,
            label: label.to_string(),
            ops,
            actor: self.actor.clone(),
            lamport,
            cause,
            depth,
            kind,
        });
        let cs = ChangeSet::build(delta.clone(), &g, kinds);
        drop(g);
        self.log.lock().push(delta);
        self.publish(&cs);
        Ok(Some(cs))
    }

    fn publish(&self, cs: &ChangeSet) {
        for s in self.subscribers.lock().iter() {
            s(cs);
        }
    }

    /// Run reactors against `cs`; commit their plans; repeat until quiescent.
    fn cascade(&self, root: ChangeSet, group: &mut UndoGroup) -> Result<()> {
        let mut frontier = vec![root];
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for cs in frontier {
                if cs.depth() >= self.max_cascade_depth {
                    return Err(UcgError::CascadeDepth(self.max_cascade_depth));
                }
                let reactors: Vec<Arc<dyn Reactor>> = self.reactors.read().clone();
                let interested: Vec<Arc<dyn Reactor>> =
                    reactors.into_iter().filter(|r| r.interested(&cs)).collect();
                if interested.is_empty() {
                    continue;
                }
                // Reactors are pure readers, so they run in parallel.
                type Reaction = Result<(String, Vec<Plan>)>;
                let plans: Vec<Reaction> = {
                    let g = self.graph.read();
                    interested
                        .par_iter()
                        .map(|r| r.react(&cs, &g).map(|p| (r.name().to_string(), p)))
                        .collect()
                };
                for res in plans {
                    let (name, plans) = match res {
                        Ok(x) => x,
                        Err(e) => {
                            return Err(UcgError::Reactor {
                                reactor: "?".into(),
                                message: e.to_string(),
                            })
                        }
                    };
                    for plan in plans {
                        let label = format!("{}: {}", name, plan.label);
                        let cause = Some(cs.revision());
                        let depth = cs.depth() + 1;
                        let run = plan.run;
                        match self.commit(&label, depth, cause, move |tx| run(tx)) {
                            Ok(Some(child)) => {
                                group.deltas.push(child.delta.clone());
                                next.push(child);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                return Err(UcgError::Reactor {
                                    reactor: name,
                                    message: e.to_string(),
                                })
                            }
                        }
                    }
                }
            }
            frontier = next;
        }
        Ok(())
    }

    /// Run the reactor cascade manually for an externally applied change set
    /// (e.g. after loading an op log with reactions disabled).
    pub fn react_to(&self, cs: ChangeSet) -> Result<UndoGroup> {
        let mut group = UndoGroup {
            label: "react".into(),
            deltas: Vec::new(),
        };
        self.cascade(cs, &mut group)?;
        Ok(group)
    }

    /// Temporarily disable reactors while `f` runs (bulk import / replay).
    pub fn without_reactions<T>(&self, f: impl FnOnce(&Engine) -> T) -> T {
        let prev = std::mem::replace(&mut *self.reacting.lock(), false);
        let out = f(self);
        *self.reacting.lock() = prev;
        out
    }

    // ---- undo / redo --------------------------------------------------------

    pub fn undo(&self) -> Result<UndoGroup> {
        let group = self.undo.lock().pop().ok_or(UcgError::NothingToUndo)?;
        let ops: Vec<Op> = group
            .deltas
            .iter()
            .rev()
            .flat_map(|d| d.inverse())
            .collect();
        let label = format!("undo {}", group.label);
        let cs = self.commit_kind(&label, 0, None, DeltaKind::Undo, |tx| tx.apply_ops(&ops))?;
        let mut redo_group = UndoGroup {
            label: group.label.clone(),
            deltas: group.deltas.clone(),
        };
        if let Some(cs) = cs {
            redo_group.deltas = vec![cs.delta.clone()];
        }
        self.redo.lock().push(group);
        Ok(redo_group)
    }

    pub fn redo(&self) -> Result<UndoGroup> {
        let group = self.redo.lock().pop().ok_or(UcgError::NothingToRedo)?;
        let ops: Vec<Op> = group.deltas.iter().flat_map(|d| d.ops.clone()).collect();
        let label = format!("redo {}", group.label);
        self.commit_kind(&label, 0, None, DeltaKind::Redo, |tx| tx.apply_ops(&ops))?;
        self.undo.lock().push(group.clone());
        Ok(group)
    }

    // ---- replay ---------------------------------------------------------------

    /// Apply a list of deltas from an op log without reactions or undo history.
    pub fn replay(&self, deltas: &[Delta]) -> Result<()> {
        for d in deltas {
            let ops = d.ops.clone();
            let label = d.label.clone();
            self.commit_kind(&label, d.depth, d.cause, d.kind, |tx| tx.apply_ops(&ops))?;
        }
        Ok(())
    }

    /// Rebuild the undo and redo stacks from a full op log, so a project
    /// reopened in a new process can still undo the previous session's work.
    ///
    /// Deltas are regrouped the way the cascade produced them: a `depth == 0`
    /// [`DeltaKind::Normal`] delta opens a group and every deeper delta that
    /// follows joins it. A [`DeltaKind::Undo`] delta moves the top group from
    /// the undo stack to the redo stack and a [`DeltaKind::Redo`] delta does
    /// the reverse, so both stacks end where the previous session left them.
    pub fn rebuild_history(&self, deltas: &[Delta]) {
        let mut undo: Vec<UndoGroup> = Vec::new();
        let mut redo: Vec<UndoGroup> = Vec::new();
        for d in deltas {
            match d.kind {
                DeltaKind::Normal => {
                    if d.depth == 0 || undo.is_empty() {
                        undo.push(UndoGroup {
                            label: d.label.clone(),
                            deltas: vec![Arc::new(d.clone())],
                        });
                    } else if let Some(g) = undo.last_mut() {
                        g.deltas.push(Arc::new(d.clone()));
                    }
                }
                DeltaKind::Undo => {
                    if let Some(g) = undo.pop() {
                        redo.push(g);
                    }
                }
                DeltaKind::Redo => {
                    if let Some(g) = redo.pop() {
                        undo.push(g);
                    }
                }
            }
        }
        *self.undo.lock() = undo;
        *self.redo.lock() = redo;
    }

    /// Quick statistics for UIs and the CLI.
    pub fn stats(&self) -> EngineStats {
        let g = self.graph.read();
        let mut by_kind: HashMap<String, usize> = HashMap::new();
        for n in g.nodes() {
            *by_kind.entry(n.kind.to_string()).or_default() += 1;
        }
        EngineStats {
            revision: g.revision(),
            nodes: g.node_count(),
            edges: g.edge_count(),
            by_kind,
            undo_depth: self.undo.lock().len(),
            log_len: self.log.lock().len(),
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct EngineStats {
    pub revision: u64,
    pub nodes: usize,
    pub edges: usize,
    pub by_kind: HashMap<String, usize>,
    pub undo_depth: usize,
    pub log_len: usize,
}
