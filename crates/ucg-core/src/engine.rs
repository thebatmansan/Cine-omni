//! The reactive engine: transactions, undo/redo, change publication, and the
//! reactor cascade that keeps every module consistent with every other.

use crate::error::{Result, UcgError};
use crate::graph::Graph;
use crate::id::EntityId;
use crate::node::NodeKind;
use crate::tx::{apply_op, Delta, DeltaKind, Op, Tx};
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

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

/// A snapshot of the undo and redo stacks. Groups share their deltas with the
/// engine, so taking one is cheap.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub undo: Vec<UndoGroup>,
    pub redo: Vec<UndoGroup>,
}

/// Bounds on the engine's in-memory windows.
///
/// Neither bound affects durability. The on-disk op log remains the complete
/// record, and undo history is rebuilt from it on open, so trimming these only
/// limits what a single live session holds in RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineLimits {
    /// Deltas retained in the in-memory session log.
    pub max_log_deltas: usize,
    /// Undo groups retained. Beyond this the oldest are dropped, which is the
    /// conventional behaviour of an application undo stack.
    pub max_undo_groups: usize,
}

impl Default for EngineLimits {
    fn default() -> Self {
        EngineLimits {
            max_log_deltas: 4_096,
            max_undo_groups: 1_000,
        }
    }
}

pub struct Engine {
    graph: RwLock<Graph>,
    reactors: RwLock<Vec<Arc<dyn Reactor>>>,
    subscribers: Mutex<Vec<Subscriber>>,
    undo: Mutex<Vec<UndoGroup>>,
    redo: Mutex<Vec<UndoGroup>>,
    /// Recent deltas from this session, for inspection and diagnostics.
    ///
    /// Bounded: the durable record is the on-disk op log, which is complete.
    /// Retaining every delta here would grow without limit over an editing
    /// session, and nothing in the product reads it for correctness.
    log: Mutex<VecDeque<Arc<Delta>>>,
    /// How many deltas have been evicted from `log` since the session began.
    log_dropped: Mutex<u64>,
    /// Caps on the in-memory windows. See [`Engine::set_limits`].
    limits: Mutex<EngineLimits>,
    actor: String,
    lamport: Mutex<u64>,
    pub max_cascade_depth: u32,
    /// How long a commit waits for the write lock before reporting a probable
    /// self-deadlock. See [`Engine::set_write_lock_timeout`].
    write_lock_timeout: Mutex<Duration>,
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
            log: Mutex::new(VecDeque::new()),
            log_dropped: Mutex::new(0),
            limits: Mutex::new(EngineLimits::default()),
            actor: "local".to_string(),
            lamport: Mutex::new(0),
            max_cascade_depth: 8,
            write_lock_timeout: Mutex::new(Duration::from_secs(5)),
            reacting: Mutex::new(true),
        }
    }

    /// How long a commit waits for the write lock before returning
    /// [`UcgError::WriteLockTimeout`]. Lower it in tests that deliberately
    /// exercise the deadlock path; raise it if a legitimate writer can block
    /// for longer. Defaults to five seconds.
    pub fn set_write_lock_timeout(&self, timeout: Duration) {
        *self.write_lock_timeout.lock() = timeout;
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

    /// The retained window of this session's deltas, oldest first.
    ///
    /// Bounded by [`EngineLimits::max_log_deltas`]; use [`Engine::log_dropped`]
    /// to see how many older deltas have been evicted. The durable record is
    /// the on-disk op log.
    pub fn log(&self) -> Vec<Arc<Delta>> {
        self.log.lock().iter().cloned().collect()
    }

    /// Deltas evicted from the in-memory window since the session began.
    pub fn log_dropped(&self) -> u64 {
        *self.log_dropped.lock()
    }

    /// Current in-memory bounds.
    pub fn limits(&self) -> EngineLimits {
        *self.limits.lock()
    }

    /// Set the in-memory bounds. Applies to subsequent commits; already
    /// retained entries are trimmed on the next push.
    pub fn set_limits(&self, limits: EngineLimits) {
        *self.limits.lock() = limits;
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
        let mut failure = None;
        if let Some(cs) = first {
            group.deltas.push(cs.delta.clone());
            if *self.reacting.lock() {
                // The user's change is already committed. If a reactor fails,
                // what did commit must still be undoable as one step, so the
                // error is held until the group is on the undo stack.
                if let Err(e) = self.cascade(cs, &mut group) {
                    failure = Some(e);
                }
            }
        }
        if !group.deltas.is_empty() {
            let limit = self.limits.lock().max_undo_groups;
            let mut undo = self.undo.lock();
            undo.push(group.clone());
            if undo.len() > limit {
                let excess = undo.len() - limit;
                undo.drain(0..excess);
            }
            self.redo.lock().clear();
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(group),
        }
    }

    /// The undo and redo stacks, oldest first: what the shared history shows.
    pub fn history(&self) -> History {
        History {
            undo: self.undo.lock().clone(),
            redo: self.redo.lock().clone(),
        }
    }

    /// Commit a change the *system* makes — a data migration at open — rather
    /// than one a person made: persisted and published like any commit, but not
    /// on the undo stack and not reacted to. Undoing a migration would restore a
    /// structure the application no longer understands.
    pub fn commit_system<F>(&self, label: &str, f: F) -> Result<Option<u64>>
    where
        F: FnOnce(&mut Tx) -> Result<()>,
    {
        Ok(self.commit(label, 0, None, f)?.map(|cs| cs.revision()))
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
        // Acquiring the write lock is the one place a caller's mistake becomes
        // a silent hang: holding an `engine.read()` guard across `transact()`
        // deadlocks, because the lock is not reentrant. Time out instead, and
        // say precisely what the caller almost certainly did wrong.
        let timeout = *self.write_lock_timeout.lock();
        let mut g = match self.graph.try_write_for(timeout) {
            Some(g) => g,
            None => return Err(UcgError::WriteLockTimeout(timeout)),
        };
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
            schema_version: crate::persist::SCHEMA_VERSION,
        });
        let cs = ChangeSet::build(delta.clone(), &g, kinds);
        drop(g);
        {
            let limits = *self.limits.lock();
            let mut log = self.log.lock();
            log.push_back(delta);
            while log.len() > limits.max_log_deltas {
                log.pop_front();
                *self.log_dropped.lock() += 1;
            }
        }
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
        // Honour the same bound as live editing, so reopening a long-lived
        // project does not materialise an unbounded undo stack.
        let limit = self.limits.lock().max_undo_groups;
        if undo.len() > limit {
            let excess = undo.len() - limit;
            undo.drain(0..excess);
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
