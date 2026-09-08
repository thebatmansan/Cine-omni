//! Nodes and edges of the Unified Context Graph.

use crate::id::EntityId;
use crate::value::Value;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Node kind, a namespaced string such as `script.scene` or `nle.clip`.
/// Kept as an interned-by-convention string so new modules can add kinds
/// without touching the core. The canonical vocabulary lives in `ucg-schema`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeKind(pub String);

impl NodeKind {
    pub fn new(s: impl Into<String>) -> Self {
        NodeKind(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The module prefix: `script.scene` → `script`.
    pub fn module(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }
}
impl From<&str> for NodeKind {
    fn from(s: &str) -> Self {
        NodeKind(s.to_string())
    }
}
impl From<String> for NodeKind {
    fn from(s: String) -> Self {
        NodeKind(s)
    }
}
impl fmt::Debug for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Relationship types. `DependsOn` and `DerivedFrom` participate in the
/// dependency DAG and drive reactive propagation; the rest are structural or
/// informational.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Hierarchy: parent contains child (Script contains Scene contains Line).
    Contains,
    /// `from` must be recomputed when `to` changes.
    DependsOn,
    /// `from` was generated from `to` (previs scene derived from script scene).
    DerivedFrom,
    /// Loose reference that does not propagate dirtiness.
    References,
    /// Cross-module binding: a clip is synced to a scene, a comp graph to a shot.
    /// Propagates dirtiness (treated like DependsOn).
    SyncedTo,
    /// Asset binding (a character to its 3D model, a clip to media).
    Binds,
}

impl EdgeKind {
    /// Whether a change at the target propagates to the source.
    pub fn propagates(&self) -> bool {
        matches!(
            self,
            EdgeKind::DependsOn | EdgeKind::DerivedFrom | EdgeKind::SyncedTo
        )
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::DependsOn => "depends_on",
            EdgeKind::DerivedFrom => "derived_from",
            EdgeKind::References => "references",
            EdgeKind::SyncedTo => "synced_to",
            EdgeKind::Binds => "binds",
        }
    }
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub from: EntityId,
    pub to: EntityId,
    pub kind: EdgeKind,
}

impl Edge {
    pub fn new(from: EntityId, to: EntityId, kind: EdgeKind) -> Self {
        Edge { from, to, kind }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: EntityId,
    pub kind: NodeKind,
    #[serde(default)]
    pub attrs: IndexMap<String, Value>,
    /// Revision at which the node was created.
    #[serde(default)]
    pub created_rev: u64,
    /// Revision of the last attribute change.
    #[serde(default)]
    pub modified_rev: u64,
}

impl Node {
    pub fn new(id: EntityId, kind: impl Into<NodeKind>) -> Self {
        Node {
            id,
            kind: kind.into(),
            attrs: IndexMap::new(),
            created_rev: 0,
            modified_rev: 0,
        }
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.attrs.insert(key.into(), value.into());
        self
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.attrs.get(key)
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Value::as_int)
    }
    pub fn get_float(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::as_float)
    }
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }
    pub fn get_ref(&self, key: &str) -> Option<EntityId> {
        self.get(key).and_then(Value::as_ref)
    }
    pub fn get_refs(&self, key: &str) -> Vec<EntityId> {
        self.get(key).map(Value::as_ref_list).unwrap_or_default()
    }
    /// Deserialize an attribute into a typed struct.
    pub fn get_as<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        self.get(key).and_then(|v| v.to_serde().ok())
    }
    /// The conventional human-readable label (`name` attr, falling back to the id).
    pub fn name(&self) -> String {
        self.get_str("name")
            .map(str::to_string)
            .unwrap_or_else(|| self.id.short())
    }
}
