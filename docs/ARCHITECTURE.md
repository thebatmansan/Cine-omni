# CINE-OMNI Architecture

CINE-OMNI replaces the fragmented film pipeline (screenwriting, storyboarding,
previs, logistics, editorial, VFX, color, audio) with one process built around a
single reactive data structure: the **Unified Context Graph (UCG)**.

This document is the deep-dive design. It covers state management, the GPU
pipeline, threading, serialization, schema design, and the module boundaries.
Architecture decisions with alternatives considered live in `docs/adr/`.

---

## 1. Technology decisions

| Concern | Choice | Why |
|---|---|---|
| Core language | Rust (2021, stable) | Deterministic memory, fearless multithreading, zero-cost abstractions for hot paths (image/audio buffers), compiles to native and to WASM for the browser shell. |
| GPU | `wgpu` (WebGPU API) with WGSL shaders | Single API across Vulkan / Metal / DX12 / WebGPU. Compute shaders for compositing, color, and scopes; render passes for the previs viewport. |
| Parallelism | `rayon` (data parallel) + `crossbeam-channel` (event bus) + `parking_lot` locks | Work-stealing for per-frame node evaluation and reactor fan-out; lock-free channels for change notification. |
| Graph storage | Arena of nodes keyed by UUID, `petgraph` for DAG algorithms | O(1) lookup by UUID, cheap topological sorts over the dependency subgraph. |
| Identity | UUID v4 for authored entities, UUID v5 (namespaced) for *derived* entities | Derived entities (a previs scene derived from a screenplay scene) get deterministic ids, so re-derivation never duplicates. |
| Serialization | `serde` → JSON (human-diffable snapshot + op log) and MessagePack (fast binary) | JSON for VCS-friendly projects and interchange; MessagePack for autosave and network sync. |
| Interchange | Fountain, FDX, OpenTimelineIO JSON, USDA, OCIO config, `.cube` LUT, ASC CDL | Industry standards, all implemented natively so the graph is the source of truth. |
| Desktop shell | TypeScript + React shell in `ui/` over the Rust core (WASM in browser, IPC/JSON-RPC on desktop) | Keeps the UI hot-reloadable while all state and math live in Rust. |

---

## 2. The Unified Context Graph

### 2.1 Data model

The UCG is a *typed property graph with a dependency DAG overlay*.

```
Node {
  id:         EntityId (UUID)
  kind:       NodeKind      -- "script.scene", "previs.camera", "nle.clip", ...
  attrs:      IndexMap<AttrKey, Value>
  created_rev / modified_rev
}

Edge { from, to, kind: EdgeKind, weight/meta }
  EdgeKind ∈ { Contains, DependsOn, DerivedFrom, References, SyncedTo, Binds }
```

* **Attributes** are dynamically typed `Value`s (bool, int, float, string, bytes,
  list, map, entity reference, vec3, mat4, rational time, time range, color).
  This mirrors USD's prim/attribute model and lets modules add data without
  central schema churn. Typed component structs in `ucg-schema` serialize
  into/out of attribute bags, so module code is still strongly typed.
* **Structural edges** (`Contains`) form the hierarchy: Project → Script → Scene → Dialogue.
* **Dependency edges** (`DependsOn`, `DerivedFrom`) form the DAG that drives
  reactivity. The engine rejects any dependency edge that would create a cycle.
* **Cross-module edges** (`SyncedTo`, `Binds`, `References`) are the "shared
  UUID" thesis made explicit: a clip *is bound to* the scene it covers, a comp
  graph *is bound to* the shot, a color node *references* the clip.

### 2.2 Mutation: transactions, ops, deltas

All writes happen inside a transaction:

```rust
let delta = graph.transact("edit dialogue", |tx| {
    tx.set(line_id, "text", "We ride at dawn.");
    Ok(())
})?;
```

A transaction records a list of **ops** (`CreateNode`, `DeleteNode`, `SetAttr`,
`RemoveAttr`, `AddEdge`, `RemoveEdge`). Committing:

1. Validates (DAG acyclicity, referential integrity).
2. Applies ops atomically under the write lock and bumps `revision`.
3. Computes the **inverse delta** for undo.
4. Computes the **dirty set**: nodes touched ∪ every node reachable from them
   over `DependsOn`/`DerivedFrom` edges, topologically ordered.
5. Publishes a `ChangeSet { revision, ops, dirty, cause }` on the event bus.

Ops are the persistence and collaboration primitive. The op log is append-only;
each op carries `(actor, lamport)` so it can later be merged CRDT-style.

### 2.3 Reactivity and the pipeline

Modules register **reactors**: `fn react(&ChangeSet, &Graph) -> Vec<Transaction>`.
The `cine-pipeline` crate runs the reactor loop:

```
commit(T0) → ChangeSet → [reactors run in parallel, read-only]
          → each returns follow-up transactions (cause = T0)
          → commit follow-ups (depth+1) → ... until quiescent or depth limit
```

Reactors are *pure functions of the graph* which makes them testable and safe to
run on a rayon pool. Loops are prevented by a depth limit plus idempotence: a
reactor that would write an identical value produces no op.

This is precisely the thesis scenario:

| Script edit on scene `S` | Reactor | Effect |
|---|---|---|
| dialogue text changes | `ScriptReactor` | recompute estimated duration, word list; mark `ScriptSync` stale |
| " | `PrevisReactor` | update the derived previs scene (characters present, dialogue audio track length) |
| " | `LogisticsReactor` | rebuild the scene breakdown and shot list; reschedule **only if** the scheduler's inputs actually changed (fingerprint fast path), and regenerate call sheets **only for days that moved** |
| " | `TimelineReactor` | flag every clip `SyncedTo S` as `needs_conform`, re-align words |
| " | `CompReactor` | mark comp graphs bound to S's shots dirty, update frame ranges |
| " | `ColorReactor` | carry color space / LUT metadata to affected clips |

### 2.4 Concurrency model

* One `Graph` per project behind a `parking_lot::RwLock`. Readers never block
  each other; reactors take read guards.
* Heavy per-frame evaluation (compositing, color) reads immutable *snapshots*:
  the evaluator clones only the sub-graph it needs into an evaluation plan, then
  runs the plan on the rayon pool or the GPU without holding any lock.
* Live metadata ingestion (on-set camera parameters at up to 240 Hz) is written
  to a lock-free ring buffer and flushed to the graph in batched transactions.

### 2.5 Persistence

A project is a directory:

```
MyFilm.cineomni/
  project.json        manifest: schema version, root entity, snapshot revision
  graph.json          full snapshot (pretty JSON, VCS-friendly)
  graph.msgpack       the same snapshot in binary, for fast loads
  oplog/current.jsonl deltas since the last save (one JSON delta per line)
  oplog/0000.jsonl…   archived segments: the undo history of past sessions
  assets/             media, LUTs, USD layers (content addressed)
```

Load = snapshot + replay of the deltas newer than the snapshot. Schema
migrations are versioned functions over the raw `Value` attribute bags, so old
projects always open.

**The in-memory windows are bounded.** `Engine` keeps a capped window of recent
deltas and a capped undo stack (`EngineLimits`, default 4096 deltas and 1000
undo groups). This is a memory bound only: the on-disk op log stays complete,
and undo history is rebuilt from it on open, so nothing needed for recovery is
dropped. `Engine::log_dropped()` reports evictions rather than hiding them.

**Recovery distinguishes a torn tail from corruption.** A truncated final record
is the signature of a process dying mid-append; that record was never a
completed transaction, so it is discarded and reported via `OplogReport`. A
malformed line anywhere *else* means later deltas may depend on a missing
operation, so recovery fails loudly instead of silently reconstructing a partial
graph.

**The compaction boundary is explicit.** `save()` archives the live op-log
segment rather than deleting it, so undo spans the snapshot. `compact()` is the
one operation that discards pre-snapshot history; the current graph state is
never affected.

**Undo survives the process.** Each delta records a `DeltaKind`
(`Normal` / `Undo` / `Redo`), so on open the engine replays the log to rebuild
the undo and redo stacks exactly as the previous session left them: a depth-0
normal delta opens a group, deeper reactor deltas join it, an undo delta moves
the top group to the redo stack, and a redo delta moves it back. `save()`
therefore *archives* the current op-log segment rather than deleting it;
`Project::compact()` drops the archives when the history is no longer wanted.

---

## 3. GPU rendering pipeline (`cine-gpu`)

* `wgpu` device shared by every module; one `RenderGraph` per frame.
* **Previs viewport**: forward renderer with physically based camera (sensor,
  focal length, aperture → projection + DoF post-pass), physically based lights
  (lumen/lux, Kelvin → RGB via Planckian locus), grease-pencil strokes rendered
  as screen-space ribbons projected on the camera frustum planes.
* **Compositor**: each comp node is a compute kernel over `rgba32float`
  textures; deep images use structured buffers of `(depth, rgba)` samples.
* **Color**: 3D LUT sampling (tetrahedral), CDL, curves, and scopes (histogram,
  waveform, vectorscope) are compute shaders that write into storage buffers.
* Every kernel has a CPU mirror (`cine_gpu::cpu`, and the full implementations
  in `cine-comp` / `cine-color`). Tests validate three levels without needing a
  GPU in CI: `naga` parses and validates every WGSL module, a reflection test
  checks that each kernel's declared binding list matches what the shader
  actually declares (a mismatch is a runtime pipeline error otherwise), and
  when an adapter *is* present the merge, CDL, keyer and blur kernels are
  compared against the CPU reference pixel by pixel.

---

## 4. Module map

```
crates/
  ucg-core        graph, values, transactions, undo, reactivity, serialization
  ucg-schema      shared component types + node/edge kind vocabulary + time types
  cine-script     Fountain/FDX parser+writer, auto-tagging, ScriptSync alignment
  cine-previs     2D storyboard canvas, USD-style stage, optics, rigs, grease pencil
  cine-logistics  breakdowns, shot lists, lens grids, scheduling, call sheets, live metadata
  cine-timeline   OTIO model + I/O, ripple/roll/slip/slide, waveform sync
  cine-comp       node compositor, keying, deep compositing, CPU evaluator
  cine-color      ACES/OCIO, LUTs, CDL, wheels, curves, masks, scopes
  cine-audio      DAW graph, spatial panning, stems, dialogue isolation
  cine-gpu        wgpu device, WGSL kernels (feature "wgpu")
  cine-pipeline   reactors wiring every module to the UCG
  cine-cli        `cine` binary: new/open/import/export/watch/demo, JSON-RPC daemon
ui/               TypeScript + React shell (viewport, timeline, node editor)
```

Dependency rule: domain crates depend on `ucg-core` + `ucg-schema` only.
`cine-pipeline` is the only crate that depends on every domain crate.

---

## 5. Performance budget

| Path | Target |
|---|---|
| Transaction commit (≤100 ops) | < 50 µs |
| Dirty-set propagation (10k-node graph) | < 1 ms |
| Reactor round (all modules, one scene edit) | < 5 ms |
| Comp node CPU eval, 1080p RGBA f32 | < 8 ms per simple node (rayon) |
| Viewport frame | 16.6 ms @ 60 Hz |
