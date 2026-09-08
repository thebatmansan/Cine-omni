//! Typed components stored in single attributes.

use serde::{de::DeserializeOwned, Serialize};
use ucg_core::{EntityId, Graph, Node, Result, Tx, Value};

/// A strongly typed component that lives in the attribute [`Component::KEY`].
pub trait Component: Serialize + DeserializeOwned + Sized {
    const KEY: &'static str;

    fn read(node: &Node) -> Option<Self> {
        node.get(Self::KEY).and_then(|v| v.to_serde().ok())
    }

    fn read_from(graph: &Graph, id: EntityId) -> Option<Self> {
        graph.node(id).and_then(Self::read)
    }

    fn write(&self, tx: &mut Tx, id: EntityId) -> Result<()> {
        tx.set(id, Self::KEY, Value::from_serde(self)?)
    }

    fn to_value(&self) -> Result<Value> {
        Value::from_serde(self)
    }
}
