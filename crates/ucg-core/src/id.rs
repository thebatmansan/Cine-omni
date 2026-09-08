//! Entity identity.
//!
//! Every object in CINE-OMNI — a scene, a line of dialogue, a camera, a clip,
//! a comp node, a color grade — has exactly one [`EntityId`] that is shared by
//! every module. Authored entities get random v4 ids; *derived* entities get
//! deterministic v5 ids computed from the entity they derive from and a
//! discriminator, so re-deriving never creates duplicates.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Namespace under which all derived CINE-OMNI ids are minted.
pub const UCG_NAMESPACE: Uuid = Uuid::from_bytes([
    0x6f, 0x1a, 0x0b, 0x2e, 0x9c, 0x3d, 0x4e, 0x5f, 0x8a, 0x7b, 0x6c, 0x5d, 0x4e, 0x3f, 0x2a, 0x1b,
]);

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub Uuid);

impl EntityId {
    /// A fresh random id for an authored entity.
    pub fn new() -> Self {
        EntityId(Uuid::new_v4())
    }

    /// Deterministic id: `derived(parent, "previs.scene")` is stable for a given parent.
    pub fn derived(parent: EntityId, discriminator: &str) -> Self {
        let mut buf = Vec::with_capacity(16 + discriminator.len());
        buf.extend_from_slice(parent.0.as_bytes());
        buf.extend_from_slice(discriminator.as_bytes());
        EntityId(Uuid::new_v5(&UCG_NAMESPACE, &buf))
    }

    /// Deterministic id from an arbitrary stable key (e.g. a file path or a
    /// content hash). Used by importers so that re-importing the same
    /// document yields the same entities.
    pub fn from_key(key: &str) -> Self {
        EntityId(Uuid::new_v5(&UCG_NAMESPACE, key.as_bytes()))
    }

    pub const fn nil() -> Self {
        EntityId(Uuid::nil())
    }

    pub fn is_nil(&self) -> bool {
        self.0.is_nil()
    }

    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s).ok().map(EntityId)
    }

    /// Short, human-friendly prefix for logs and UI.
    pub fn short(&self) -> String {
        self.0.to_string()[..8].to_string()
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.short())
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for EntityId {
    fn from(u: Uuid) -> Self {
        EntityId(u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_are_deterministic_and_distinct() {
        let p = EntityId::new();
        assert_eq!(EntityId::derived(p, "a"), EntityId::derived(p, "a"));
        assert_ne!(EntityId::derived(p, "a"), EntityId::derived(p, "b"));
        assert_ne!(
            EntityId::derived(p, "a"),
            EntityId::derived(EntityId::new(), "a")
        );
    }

    #[test]
    fn key_ids_are_stable() {
        assert_eq!(
            EntityId::from_key("script.fountain#12"),
            EntityId::from_key("script.fountain#12")
        );
    }
}
