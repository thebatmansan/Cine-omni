# ADR 0003 — Reactor cascade semantics

**Status**: accepted

## Rules

1. A user transaction commits atomically and publishes a `ChangeSet` whose
   `dirty` list is touched ∪ transitive dependents, topologically ordered.
2. Every registered reactor whose `interested()` returns true runs in
   parallel against the *same* read snapshot and returns `Plan`s.
3. Plans are committed serially in reactor order, each as its own delta with
   `cause = parent revision` and `depth = parent depth + 1`.
4. Each committed plan produces a new `ChangeSet`; step 2 repeats until no
   reactor produces ops or `max_cascade_depth` (default 8) is hit, which is an
   error (a reactor bug, never a legitimate state).
5. All deltas of the cascade join the user's `UndoGroup`; undo applies their
   inverses in reverse order with reactions disabled.

## Why plans, not direct writes

Reactors that wrote directly would need the write lock, serializing them and
making ordering non-deterministic. Returning closures keeps the read phase
parallel and the write phase ordered and observable (every follow-up is a
labelled delta in the op log: `logistics: reschedule`).

## Convergence

Convergence relies on **idempotent writers**: `Tx::set` drops equal values,
derived entities have deterministic ids, and writers remove what they no
longer produce. A reactor that fires on its own output must therefore produce
zero ops on the second pass. The `dirty` semantics for edges (a dependency
edge marks only the dependent side dirty) prevent the common accidental loop
where deriving X from S re-dirties S.
