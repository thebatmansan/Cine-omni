# 00 — Repository Archaeology

**Status:** complete. Every number here was measured on this machine at commit
`7a82b41`, not estimated. The benchmarks are committed as
`crates/ucg-core/tests/scale.rs` and `crates/cine-pipeline/tests/scale.rs` and
can be re-run at any time:

```bash
cargo test -p ucg-core     --release --test scale -- --ignored --nocapture
cargo test -p cine-pipeline --release --test scale -- --ignored --nocapture
```

This document answers directive §81's first question — *what does CINE-OMNI
actually have today, and will it carry the vision?* — with evidence rather than
opinion. It is deliberately unflattering where the evidence is unflattering.

---

## 1. Executive finding

**The transaction core is sound and scales. The reactive cascade above it does
not.** Those are separable, and that separation is the single most important
result of this archaeology, because it means the foundation is worth keeping and
the defect is worth fixing rather than rewriting.

| Layer | Verdict | Evidence |
|---|---|---|
| Identity, ops, transactions, undo, op-log | **Keep. Validated at scale.** | Commit latency is *flat* at ~13 µs from 2,801 to 112,001 nodes |
| Graph query layer | **Fix. Two pathological hot paths.** | `of_kind()` is 74× slower than a naive linear scan |
| Reactor cascade | **Re-architect. Not incremental.** | One dialogue edit: 2.15 ms at 7 scenes → 144 ms at 1,000 scenes |
| Off-thread evaluation | **Re-architect.** | `snapshot()` is a full O(n) clone, 363 ms at 56k nodes |
| In-memory op log | **Bound it.** | `Engine::log` grows without limit for the process lifetime |

---

## 2. What was preserved before touching anything (§43)

The repository had **no version control at all**. This was the highest-priority
risk in the whole engagement: ~50,000 lines of validated work with no way to
recover from a bad refactor.

Fixed first, before any other action:

```
commit 7a82b41  Checkpoint: CINE-OMNI foundation before industry re-architecture
179 files
```

Verified at that commit: 424 tests passing, `clippy -D warnings` clean, rustfmt
clean, GPU kernels validated against the CPU reference on Vulkan.

---

## 3. The measured baseline

### 3.1 What scales — the transaction core

`cargo test -p ucg-core --release --test scale`

| scenes | nodes | mean single-attribute commit | `snapshot()` |
|---:|---:|---:|---:|
| 100 | 2,801 | 10.85 µs | 12.3 ms |
| 500 | 14,001 | 10.91 µs | 52.4 ms |
| 1,000 | 28,001 | 13.28 µs | 110.6 ms |
| 2,000 | 56,001 | 13.17 µs | 233.1 ms |
| 4,000 | 112,001 | 13.44 µs | 467.5 ms |

**Commit latency is flat.** A 40× increase in graph size produces a 1.24×
increase in write latency, which is measurement noise plus cache effects. The
op-application path, the revision bump, the inverse-delta construction and the
dirty-set computation are all genuinely independent of total graph size.

This validates the core design decision. Whatever else changes, **the
transaction engine, the invertible op model, the deterministic identity scheme
and the grouped undo should be preserved.** They are correct and they are fast.

Supporting measurements at 56,001 nodes / 56,000 edges:

| Operation | Cost | Note |
|---|---:|---|
| Build 56k nodes in one transaction | 5.35 s | one-time bulk import path |
| Memory | 110.5 MiB | ≈ 2,069 bytes/node |
| Narrow dirty-set (1 dialogue line) | 47.5 µs | dirty = 1 |
| Scene-level dirty-set | 91.3 µs | dirty = 10 |
| `validate()` full structural check | 85.7 ms | 0 problems |

At ~2 KB/node, one million nodes projects to roughly **2 GB resident**. That is
survivable on a workstation but it is a real budget, and it means the "millions
of metadata records" in directive §31 cannot all be UCG nodes. See §6.

### 3.2 What does not scale — the cascade

`cargo test -p cine-pipeline --release --test scale`

One artist edits one line of dialogue, all seven reactors installed:

| scenes | nodes | import | **one edit** | deltas |
|---:|---:|---:|---:|---:|
| 7 | 195 | 23 ms | **2.15 ms** | 7 |
| 50 | 1,425 | 127 ms | **5.61 ms** | 7 |
| 200 | 5,661 | 590 ms | **19.9 ms** | 7 |
| 500 | 14,137 | 1.39 s | **78.5 ms** | 7 |
| 1,000 | 28,261 | 2.96 s | **144.3 ms** | 7 |

The delta count stays at 7. The *cost* grows 67× while the *work the user asked
for* stays constant. This is the defining flaw of the current architecture.

**The demo is honest but unrepresentative.** `cine demo` runs a 7-scene
screenplay where the cascade settles in 2 ms and looks excellent. A feature is
100–200 scenes; an episodic season is 500–1,000. At feature scale the system is
already past the 100 ms threshold where an editor perceives lag, and the curve is
superlinear.

---

## 4. Root causes, precisely identified

I traced each defect to a specific line rather than asserting a general
"performance problem".

### DEFECT 1 — `of_kind()` performs a pathological sort

`crates/ucg-core/src/graph.rs:91`

```rust
v.sort_by_key(|n| self.nodes.get_index_of(&n.id));
```

Measured on a 56,000-node graph containing 8,000 `logistics.shot` nodes:

| Approach | Cost per call |
|---|---:|
| `of_kind()` as written (uses the `by_kind` index) | **74.3 ms** |
| Naive linear scan of all 56k nodes + string compare | **1.0 ms** |
| `children()` (same defect class) | 5.0 ms |

**The indexed lookup is 74× slower than having no index at all.**

Root cause: `Vec::sort_by_key` does not cache — it re-evaluates the key closure
on *every comparison*, and each evaluation is an `IndexMap::get_index_of`, which
hashes a 16-byte UUID. For 8,000 elements that is roughly 104,000 UUID hashes
plus pointer chasing per call. The `by_kind` index does its job correctly; the
sort that follows it destroys the benefit entirely.

This matters because reactors call `of_kind()` and `children()` on every
cascade. `cine-pipeline/src/reactors.rs:101` calls `of_kind(kinds::PROJECT)` for
every affected scene, on the 74 ms path.

**Fix:** `sort_by_cached_key` (evaluates the key once per element), or better,
store insertion order in the index so no sort is needed. Expected: 74 ms → under
1 ms. This is a small, safe, well-understood change with a measured baseline to
verify against.

### DEFECT 2 — the cascade recomputes whole aggregates

`crates/cine-pipeline/src/reactors.rs`, `LogisticsReactor::schedule_plan`

On *any* change to a scene's statistics, the reactor:

1. calls `breakdown_document()` — walks **every scene** in the film;
2. calls `schedule()` — re-sorts and re-packs **every strip**;
3. rebuilds a `CastRoster` across **the whole document**;
4. regenerates **every call sheet for every shooting day**.

An O(1) user action triggers O(N) recomputation of the entire production
schedule. This is the direct cause of the 67× growth in §3.2.

It is not a bug in the sense of being wrong — the output is correct, and
idempotent writes mean the graph converges. It is an *architectural* flaw: the
reactive model was designed for correctness and never for incrementality.

**This is the finding that most constrains the vision.** Directive §26 asks the
system to know continuously what is current, stale, processing or failed across
twenty-odd domains. That is unreachable if each domain recomputes globally on
every keystroke. Section 7 proposes the fix.

### DEFECT 3 — `snapshot()` is a full graph clone

`crates/ucg-core/src/engine.rs:233`. 363 ms at 56k nodes, 467 ms at 112k, and it
is the documented mechanism for off-thread and GPU evaluation. Any interactive
preview that needs a consistent read of the graph pays a third of a second.

### DEFECT 4 — unbounded in-memory op log

`Engine::log: Mutex<Vec<Arc<Delta>>>` retains every delta for the process
lifetime, as do the undo and redo stacks. Deltas carry full previous values for
undo, so they are not small. Over a shooting day of continuous editing this
grows without bound. Persistence already archives to disk; memory does not.

### DEFECT 5 — bulk import is slow

2.96 s to import a 1,000-scene screenplay, and it grows superlinearly. This is
the same aggregate-recomputation problem seen through the import path.

### ARCHITECTURAL LIMIT — one global write lock

A single `parking_lot::RwLock<Graph>` serialises every write in the project.
Reads are concurrent, which is right, but with the directive's twenty workspaces,
background media ingest, render workers and eventual multi-user collaboration,
one global writer becomes the bottleneck. It is correct today and will not
survive the vision unchanged.

---

## 5. What the current implementation genuinely has

Stated precisely so the blueprint does not have to re-litigate it, and so no one
mistakes foundation for product.

| Capability | Real state |
|---|---|
| Identity (deterministic derived UUIDs) | **Solid.** The best idea in the codebase. Re-derivation and re-import never duplicate. |
| Invertible ops, atomic transactions | **Solid.** Six op types, rollback on error, verified. |
| Grouped one-step undo across a cascade | **Solid.** Verified: one undo emits 19 inverse ops across six modules. |
| Op-log persistence surviving restart | **Solid.** `DeltaKind` replay rebuilds undo/redo stacks; verified across processes. |
| Cycle-rejecting DAG | **Solid.** Rejects propagating cycles, rolls back the whole transaction. |
| Reactive cascade | **Correct but not incremental.** See Defect 2. |
| Fountain / FDX | **Genuinely good.** Full spec, bidirectional, round-trip verified. |
| Optics / physical camera | **Genuinely good.** 10 real sensors, DoF, exposure, frustum maths. |
| OTIO schema compatibility | **Real but schema-only.** Not the OTIO library; no adapters (AAF/EDL/FCP XML). |
| Color science | **Strong maths, approximate output transform.** Must not be called ACES-conformant. |
| Compositing | **Real evaluator, no image I/O.** Cannot open an EXR. |
| Audio | **Strong DSP, no device I/O.** Cannot play a sound. |
| GPU | **8 validated kernels, headless.** No windowed swapchain, no viewport. |
| UI | **~460-line shell.** One daemon per browser tab; tabs diverge. |
| Media engine | **Does not exist.** No decode, no encode, no image reading. |

---

## 6. The scale question the directive forces (§31)

Directive §31 asks for thousands of scenes, tens of thousands of shots and
"millions of assets/metadata records where appropriate". The measurements say:

- **Thousands of scenes: fine** for storage (2,000 scenes ≈ 56k nodes ≈ 110 MiB)
  but **not fine** for the cascade as written.
- **Millions of records: not as UCG nodes.** At ~2 KB/node that is ~2 GB before
  any media. Per-frame camera metadata at 240 Hz, per-take reports and per-frame
  tracking data must live in a columnar side-store keyed by entity ID, with only
  the summary in the graph. The existing `cine-logistics` ingest buffer already
  hints at this pattern; it needs to become architecture.

**Conclusion: the UCG should remain the authoritative graph of *relationships and
identity*, and stop being the store for bulk time-series and per-frame data.**
That is a boundary the current design does not draw.

---

## 7. What must change, in priority order

Ordered by measured severity, not by ambition.

1. **Fix the two pathological sorts.** Small, safe, measured. 74 ms → sub-1 ms.
   Do this before anything else; it makes every later measurement meaningful.
2. **Make reactors incremental.** Introduce scoped invalidation so a scene edit
   reschedules only the affected shooting days, not the film. This likely means
   reactors declaring *what* they depend on (a query subscription) rather than
   re-deriving from scratch, and aggregates becoming incrementally maintained
   values in the graph rather than recomputed ones. This is the largest single
   piece of core work and it gates the entire vision.
3. **Replace `snapshot()` with cheap consistent reads.** Persistent/immutable
   data structures or copy-on-write generations, so background evaluation takes
   a version handle rather than a 363 ms clone.
4. **Bound the in-memory op log** with a spill-to-disk window.
5. **Split the global write lock** once 2 and 3 land, most likely per-subgraph or
   via an actor boundary per domain.
6. **Draw the bulk-data boundary**: graph for identity and relationships, side
   store for time series and per-frame data.

Items 1–4 are self-contained changes to `ucg-core` and `cine-pipeline`. None of
them require a decision about Blender, OpenUSD or the UI shell, which means
**they can proceed while the ecosystem research completes.**

---

## 8. Honest limits of this archaeology

- All measurements are single-machine (4 cores, Intel UHD Graphics, Linux) and
  single-run. They establish orders of magnitude and scaling *shape*, not
  precise constants.
- The 2,069 bytes/node figure is RSS delta divided by node count, so it includes
  allocator overhead and index structures, not just node payload.
- The cascade benchmark uses synthetic screenplays with uniform scene sizes.
  Real screenplays have long-tail scene lengths, which will change constants but
  not the O(N)-per-edit shape, since the cost is dominated by whole-document
  rescheduling.
- I have not yet profiled the GPU path under load, the persistence path at
  scale, or memory behaviour over a long editing session.
