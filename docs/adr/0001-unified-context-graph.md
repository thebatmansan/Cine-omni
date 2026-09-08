# ADR 0001 — One reactive property graph instead of per-module documents

**Status**: accepted

## Context

Film pipelines lose data at every hand-off: Final Draft → Storyboard Pro →
FrameForge/Maya → StudioBinder → Resolve/Nuke/Pro Tools each keep their own
document with their own identifiers. A dialogue change in the script has no
path to the shot list, the cut, or the comp.

## Decision

All modules share one in-memory graph (`ucg-core`) where:

* every entity has one UUID for its whole life (`EntityId`), derived entities
  use deterministic v5 ids so re-derivation is idempotent;
* attributes are dynamically typed `Value`s (USD-like) with typed component
  structs layered on top (`ucg-schema`);
* `DependsOn` / `DerivedFrom` / `SyncedTo` edges form a DAG that the engine
  keeps acyclic and uses to compute topologically ordered dirty sets;
* every write is a transaction of invertible ops; reactor follow-ups are
  grouped with the user's transaction for undo;
* reactors are pure (`&ChangeSet, &Graph → Vec<Plan>`) so they run in
  parallel and are trivially testable.

## Alternatives considered

* **ECS (hecs/bevy_ecs)** — excellent for per-frame iteration, poor for
  arbitrary cross-entity relationships and undo. Rejected; we keep the
  option of an ECS *view* for the viewport.
* **Embedded database (SQLite/sled)** — durable but adds serialization on
  every read; interactive editing needs sub-millisecond commits. Rejected for
  the hot path; the op log gives durability.
* **CRDT document (automerge/yrs)** — attractive for collaboration; the op log
  with (actor, lamport) is designed so a CRDT merge layer can be added later
  without changing module code.

## Consequences

* Modules never talk to each other; they talk to the graph. Adding a module
  means adding a reactor.
* Idempotent writers are mandatory (`tx.set` no-ops on equal values) — that
  is what makes the cascade converge.
* Per-frame heavy data (images, audio buffers) lives *outside* the graph and
  is referenced by id; the graph holds parameters and structure.
