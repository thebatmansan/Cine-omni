# CINE-OMNI — Full Context Document

> **Purpose of this file.** This is a single-read operational briefing for a large
> language model that has never seen this repository. It contains the vision, the
> architecture, the exact public API surfaces, the file tree, the invariants that
> will break the build if violated, and the verification status of every claim.
> Everything here was read from the working tree, not recalled from memory.
>
> **Verification status at time of writing:** 424 tests pass, 0 fail.
> `cargo clippy --workspace --all-targets -- -D warnings` is clean.
> `cargo fmt --all --check` is clean. GPU kernels validated against the CPU
> reference on real hardware (Intel UHD Graphics CML GT2, Vulkan backend).

---

## 1. EXECUTIVE OVERVIEW

### 1.1 The problem

A film moves through six or seven programs that cannot talk to each other:

| Stage | Incumbent tool | What it owns |
|---|---|---|
| Screenwriting | Final Draft | `.fdx` document |
| Storyboarding | Storyboard Pro | drawing files |
| Previsualization | Maya / Blender / FrameForge | scene files |
| Scheduling | StudioBinder / Movie Magic | stripboard database |
| Editorial | DaVinci Resolve / Premiere | project database |
| VFX | Nuke | node scripts |
| Color | DaVinci Resolve | grades and LUTs |
| Audio post | Pro Tools | session file |

Each hand-off is a lossy export. A screenwriter changing one line of dialogue
starts a manual chain: the AD re-times the schedule, the editor re-conforms the
cut, the VFX lead re-checks shot lengths, the mixer re-cuts dialogue. Nothing
propagates. Identity is lost at every boundary because each tool invents its own
IDs for the same real-world objects.

### 1.2 The thesis

**One process. One graph. One UUID per real-world entity, shared by every
module.** A scene, a line of dialogue, a camera, an editorial clip, a comp node,
a color grade and an audio clip are all nodes in a single reactive graph called
the **Unified Context Graph (UCG)**. Dependency edges between them form a
directed acyclic graph. When anything changes, the engine computes exactly which
other nodes are now stale, in topological order, and lets each module react.

The whole cascade is **one undo step**, because it was one user action.

### 1.3 The thesis demonstrated

This is real output from `cargo run -p cine-cli -- demo`, not an illustration.
A single dialogue edit fans out across six modules and is reverted by one undo:

```
== editing first dialogue line of scene 1 ==
  was: "We're closed."
transaction `edit dialogue` → 7 deltas
    32  depth 0  edit dialogue                                      1 ops
    33  depth 1  script: recompute scene stats                      3 ops
    34  depth 1  audio: sync dialogue placeholders                  6 ops
    35  depth 2  previs: sync previs scene                          2 ops
    36  depth 2  logistics: rebuild breakdown + shot list           3 ops
    37  depth 2  logistics: reschedule                              3 ops
    38  depth 3  comp: ensure comp for shot                         1 ops
SCENE 1 INT. HARBOR OFFICE - NIGHT
  words=58 eighths=7 est=31.3s characters=["MAREN", "ELIAS"]
  previs: INT. HARBOR OFFICE - NIGHT cameras=1 characters=2
  shot list: 1 shots
    shot 1A Full Static 35mm est=31.3s comp=yes stale=true
  audio: dialogue placeholders=5

== undo ==
undid `edit dialogue` in one step (19 inverse ops)
```

### 1.4 Honest scope boundaries

An LLM reasoning about this code must not overstate it. The following are
**native from-scratch implementations of the formats and algorithms**, not
bindings to the industry libraries of the same name:

- The USD layer is a **USD-style stage with USDA text serialization**. It is not
  linked against Pixar OpenUSD.
- The OTIO layer is a **schema-compatible reader and writer** of OpenTimelineIO
  JSON. It is not linked against the OTIO C++/Python library.
- The OCIO layer **parses a documented subset of OCIO v2 YAML** and implements
  the transforms itself. It is not linked against OpenColorIO.
- The ACES output transform is an **analytic approximation** (the widely used
  fitted RRT+ODT) plus a parametric tonescale, not the official CTL.
- There is **no video decode, no audio device I/O, and no windowing system**.
  The GPU crate is headless compute plus a render pipeline definition. Media is
  referenced by path and range; the compositor's inputs come from synthetic or
  provider-supplied passes.
- The UI is a **functional shell**, roughly 460 lines. It proves the round trip
  end to end. It is not a finished editing interface.

What *is* fully real: the graph engine, the reactive cascade, transactional
undo, op-log persistence across process restarts, the parsers and writers, the
DSP and color math, the compositing evaluator, and the GPU kernels.

---

## 2. ARCHITECTURE & TECH STACK

### 2.1 Language and scale

| Metric | Value |
|---|---|
| Core language | Rust, edition 2021, `rust-version = 1.85`, built with 1.98 |
| Rust source | 49,929 lines across 123 `.rs` files |
| WGSL shaders | 493 lines across 9 files |
| TypeScript / TSX / MJS | 464 lines |
| Crates | 12 total: `ucg-core`, `ucg-schema`, 8 domain crates, `cine-pipeline`, `cine-cli` |
| Tests | 424, all passing |

Lines of Rust per crate:

| Crate | LOC | Tests |
|---|---:|---:|
| `cine-color` | 9,890 | 80 |
| `cine-comp` | 7,486 | 65 |
| `cine-audio` | 6,430 | 50 |
| `cine-previs` | 5,371 | 46 |
| `cine-timeline` | 4,750 | 61 |
| `cine-logistics` | 4,731 | 46 |
| `cine-script` | 4,329 | 39 |
| `cine-pipeline` | 1,366 | 6 |
| `cine-gpu` | 1,339 | 8 (10 with the `wgpu` feature) |
| `ucg-core` | 3,117 | 21 |
| `ucg-schema` | 623 | 2 |
| `cine-cli` | 497 | 0 |

### 2.2 Why Rust

Deterministic memory behavior for image and audio buffers, data-race-free
parallelism for the reactor fan-out and per-node evaluation, and one codebase
that targets native and WebAssembly. Key dependencies:

| Crate | Role |
|---|---|
| `rayon` | Data-parallel reactor execution and per-node pixel work |
| `parking_lot` | The single `RwLock` guarding each project's graph |
| `crossbeam-channel` | Lock-free change notification to UI threads |
| `petgraph` | DAG algorithms |
| `indexmap` | Insertion-ordered attribute maps and node arenas |
| `serde` + `serde_json` + `rmp-serde` | JSON and MessagePack serialization |
| `uuid` | v4 authored IDs, v5 namespaced derived IDs |
| `glam` | `DVec3` / `DMat4` / `DQuat` transform math |
| `rustfft` | Waveform sync correlation and STFT |
| `roxmltree` | FDX and CDL XML parsing |
| `wgpu` 26 (optional) | GPU backend |
| `naga` (dev) | Shader validation without a GPU |

### 2.3 Graphics and shaders — `cine-gpu`

Nine WGSL files: eight kernels plus `common.wgsl`, a shared prelude that is
string-concatenated in front of every module (Rec.709 luma, sRGB encode/decode,
PQ encode/decode, 2D indexing).

| Kernel | Type | Bindings (group 0) | Purpose |
|---|---|---|---|
| `merge` | compute 16×16 | Uniform, Read, Read, Write | 15 Porter-Duff operations on premultiplied RGBA |
| `cdl` | compute 16×16 | Uniform, Read, Write | ASC CDL slope/offset/power/saturation |
| `lut3d` | compute 16×16 | Uniform, Read, Read, Write | Tetrahedral 3D LUT interpolation |
| `scopes` | compute 16×16 | Uniform, Read, Write, Write, Write | Atomic histogram, waveform, vectorscope |
| `blur` | compute 16×16 | Uniform, Read, Write | Separable Gaussian, run twice |
| `keyer` | compute 16×16 | Uniform, Read, Write | YCbCr chroma key with spill suppression |
| `dof` | compute 16×16 | Uniform, Read, Read, Write | Depth-driven circle of confusion gather |
| `previs` | render | Uniform, Uniform | PBR-ish forward viewport, MRT color/depth/normal, plus grease-pencil ribbon pass |

**Three-level validation, only one of which needs a GPU:**

1. `naga` parses and validates every WGSL module in CI. A broken shader fails
   the build with no hardware present.
2. A reflection test walks each module's global variables and asserts the
   declared `Binding` list matches the shader's actual address spaces and
   access flags. This caught a real bug: a read-only storage buffer declared as
   read-write in the pipeline layout, which wgpu rejects only at runtime.
3. When an adapter exists, `merge` (all 15 ops), `cdl`, `keyer` and `blur` are
   compared against the CPU mirror in `cine_gpu::cpu` pixel by pixel within
   1e-5. Verified on Intel UHD Graphics (CML GT2) through Vulkan.

`frame_graph.rs` is a backend-agnostic render graph: passes declare reads and
writes, the compiler culls passes that do not reach the requested outputs,
topologically orders the rest, and aliases transient resources by lifetime so a
40-node 4K comp does not allocate 40 4K buffers.

The `wgpu` feature is **off by default** so the workspace builds fast and
without a GPU. Every kernel has a CPU mirror used as the reference.

### 2.4 Inter-process communication

This is the part most likely to be misunderstood, so it is stated precisely.

**`cine serve <project-dir>` is not a network server.** It is a JSON-RPC 2.0
peer over **stdin and stdout**, one line-delimited JSON message per request. A
browser cannot reach it.

**`ui/server/bridge.mjs` is the browser-facing backend.** It listens on
`ws://localhost:7331` and, for each WebSocket client that connects, spawns its
own `cine serve` child process, then pipes bytes both ways:

```
Browser  ──WebSocket ws://localhost:7331──▶  bridge.mjs
                                                 │ spawn + stdio pipe
                                                 ▼
                                    cine serve <project>.cineomni
                                                 │
                                                 ▼
                                    OmniProject → Engine → UCG
```

Invocation:

```bash
node server/bridge.mjs <project-dir> [--port 7331] [--bin ../target/debug/cine]
```

Two message directions:

- **Request/response.** `{"jsonrpc":"2.0","id":N,"method":M,"params":P}` in,
  `{"jsonrpc":"2.0","id":N,"result":R}` or `{...,"error":{code,message}}` out.
- **Server-push notification.** After every committed transaction the daemon
  emits `{"jsonrpc":"2.0","method":"changed","params":{revision,label,depth,touched,dirty}}`
  with no `id`. This is how the UI watches the cascade live.

**RPC methods:** `stats`, `node`, `kind`, `scenes`, `report`, `scene_report`,
`set`, `edit_dialogue`, `import_fountain`, `export`, `undo`, `redo`, `save`,
`validate`, `shutdown`.

**Known limitation.** One daemon per WebSocket connection means two browser tabs
open two independent `OmniProject` instances over the same directory. They will
not see each other's edits, and both append to the same op log. Use one tab.
A single shared daemon with fan-out is the obvious next step.

### 2.5 Frontend

React 18 + Vite 5 + TypeScript, served on `http://localhost:5173`.

| File | Role |
|---|---|
| `ui/src/rpc/client.ts` | `RpcClient`: promise-based JSON-RPC over WebSocket, change-notification subscriptions, typed helpers |
| `ui/src/App.tsx` | Four-panel layout, connection state, undo/redo/save |
| `ui/src/panels/ScriptPanel.tsx` | Scene list with word/page/duration stats; dialogue editing on blur |
| `ui/src/panels/Viewport.tsx` | Top-down previs blocking view on Canvas 2D: characters, cameras with real horizontal FOV cones, lights |
| `ui/src/panels/CascadePanel.tsx` | The reactor cascade of the last action, and the live change feed, colored by depth |
| `ui/src/panels/InspectorPanel.tsx` | Everything derived from the selected scene, staleness highlighted |

---

## 3. UNIFIED CONTEXT GRAPH (UCG) MECHANICS

Everything in this section lives in `crates/ucg-core`.

### 3.1 Identity

```rust
pub struct EntityId(pub Uuid);

EntityId::new()                       // v4, authored entity
EntityId::derived(parent, "previs.scene")  // v5, deterministic from parent + discriminator
EntityId::from_key("script:film.fountain:scene:12")  // v5, deterministic from a stable key
```

This is the mechanism that makes the whole thesis work. Derived entities get
**deterministic** IDs, so re-deriving never duplicates and re-importing a file
updates in place rather than creating a second copy. The previs scene for script
scene `S` is *always* `EntityId::derived(S, kinds::PREVIS_SCENE)`, computable by
any module without a lookup.

### 3.2 Data model

```rust
pub struct Node {
    pub id: EntityId,
    pub kind: NodeKind,                    // "script.scene", "nle.clip", ...
    pub attrs: IndexMap<String, Value>,
    pub created_rev: u64,
    pub modified_rev: u64,
}

pub struct Edge { pub from: EntityId, pub to: EntityId, pub kind: EdgeKind }
```

`Value` is a dynamically typed attribute, mirroring USD's prim/attribute model:
`Null, Bool, Int, Float, Str, Bytes, List, Map, Ref(EntityId), Vec2, Vec3, Vec4,
Quat, Mat4, Color, Time(RationalTime), Range(TimeRange)`. Strongly typed
component structs in `ucg-schema` serialize into and out of single attributes,
so module code stays type-safe while the core stays schema-agnostic.

**Edge kinds and which ones propagate staleness:**

| Kind | Meaning | Propagates? |
|---|---|---|
| `Contains` | Hierarchy: Project → Script → Scene → Dialogue | No |
| `DependsOn` | `from` must be recomputed when `to` changes | **Yes** |
| `DerivedFrom` | `from` was generated from `to` | **Yes** |
| `SyncedTo` | Cross-module binding: clip ↔ scene, alignment ↔ media | **Yes** |
| `References` | Loose reference | No |
| `Binds` | Asset binding: clip → media, character → model | No |

### 3.3 Reactive DAG with cycle rejection

`Graph::would_cycle(from, to)` runs before every propagating edge insert. If
`to` already transitively depends on `from`, the insert returns
`UcgError::Cycle` and **the entire transaction rolls back**. Non-propagating
edges (`References`, `Contains`, `Binds`) may form cycles freely.

Query API on `&Graph`:

```rust
node(id) require(id) attr(id, key) contains(id)
of_kind("script.scene") of_module("nle.")
children(id) parent(id) descendants(id) ancestor_of_kind(id, kind)
targets(id, EdgeKind) sources(id, EdgeKind) has_edge(from, to, kind)
dependents(id)      // transitive, follows INCOMING propagating edges
dependencies(id)    // transitive, follows OUTGOING propagating edges
topo_order(&[ids]) topo_order_all() would_cycle(from, to)
validate() -> Vec<String>   // empty means structurally sound
```

### 3.4 Atomic transactions and inverse ops

Every mutation goes through a closure. Reads inside the closure see the writes
already made in that same closure.

```rust
let group = engine.transact("edit dialogue", |tx| {
    tx.set(line_id, attrs::TEXT, "We ride at dawn.")?;
    Ok(())
})?;
```

`Tx` API: `create`, `create_with_id`, `ensure`, `insert`, `create_child`,
`delete` (recursive over `Contains`), `set`, `set_as<T: Serialize>`,
`remove_attr`, `link`, `unlink`, `reparent`, `apply_ops`, `graph()`, `ops()`.

Six op types, all invertible:

```rust
enum Op {
    CreateNode { node },
    DeleteNode { node, edges },   // carries edges so undo can restore them
    SetAttr    { id, key, value, prev },
    RemoveAttr { id, key, prev },
    AddEdge    { edge },
    RemoveEdge { edge },
}
```

**Commit sequence:**

1. Ops apply in place under the write lock. On `Err`, already-applied ops are
   inverted and the revision does not advance.
2. No-op `SetAttr` (new value equals previous) is dropped. An empty op list
   commits nothing at all.
3. `revision += 1`, a Lamport clock ticks.
4. The **dirty set** is computed: touched nodes ∪ all transitive dependents,
   returned in topological order, dependencies first.
5. A `ChangeSet` is published to every subscriber.

A subtle but load-bearing detail: for a **propagating** edge op, only
`edge.from` is marked touched, never `edge.to`. Marking both would re-dirty the
source that a derived node was just built from, and the cascade would oscillate.
Structural edges mark both endpoints.

### 3.5 The grouped one-step undo

`UndoGroup { label, deltas: Vec<Arc<Delta>> }` holds the user's transaction
**and every reactor transaction it caused**. `engine.undo()` inverts all of them
in reverse order as a single commit, with reactors disabled so the undo does not
trigger a fresh cascade. In the demo above, one undo emitted 19 inverse ops
across six modules.

### 3.6 Append-only op-log persistence

A project is a directory, conventionally `Name.cineomni`:

```
Name.cineomni/
  project.json          manifest: schema version, root entity, snapshot revision
  graph.json            full snapshot, pretty JSON, diffable in version control
  graph.msgpack         same snapshot in MessagePack for fast loads
  oplog/current.jsonl   deltas since the last save, one JSON delta per line
  oplog/0000.jsonl …    archived segments: the undo history of past sessions
  assets/               media, LUTs, USD layers
```

Load = snapshot + replay of deltas whose revision exceeds the snapshot revision.

**Undo survives process death.** Each delta records a `DeltaKind` of `Normal`,
`Undo` or `Redo`. `Engine::rebuild_history(&deltas)` replays the log to
reconstruct both stacks exactly as the previous session left them:

- a `Normal` delta at `depth == 0` opens a new undo group;
- deeper `Normal` deltas join the group above them;
- an `Undo` delta pops the undo stack onto the redo stack;
- a `Redo` delta does the reverse.

Because of this, `Project::save()` **archives** `current.jsonl` to a numbered
segment instead of deleting it. `Project::compact()` drops archives when the
history is no longer wanted. This is verified by a test that quits and reopens a
project twice and by `cine undo` working across separate CLI processes.

### 3.7 Concurrency

One `Graph` per project behind a `parking_lot::RwLock`. Readers never block each
other. Reactors run on a rayon pool holding a read guard. Heavy per-frame work
uses `engine.snapshot()`, an owned clone, so no lock is held during evaluation.

> **The single most dangerous rule in this codebase:** never hold an
> `engine.read()` guard across a call to `engine.transact(...)`. The lock is not
> reentrant and the thread deadlocks **silently** with no panic and no timeout.
> This has bitten this project more than once. Scope every read guard in its own
> block. It is documented in `CLAUDE.md` for exactly this reason.

### 3.8 The 7-reactor cascade

Reactors are **pure**: `fn react(&ChangeSet, &Graph) -> Result<Vec<Plan>>`. They
read, they do not write. They return closures the engine executes as separate
labelled transactions. This keeps the read phase parallel and the write phase
ordered and observable.

```
commit(T0) → ChangeSet
           → all interested reactors run IN PARALLEL against one read snapshot
           → each returns Plans
           → engine commits each Plan as its own delta (cause = T0, depth += 1)
           → each commit yields a new ChangeSet → repeat
           → until no reactor produces ops, or max_cascade_depth (default 8)
```

All seven live in `crates/cine-pipeline/src/reactors.rs` and are installed by
`install_all(&engine)`.

| # | Reactor | Wakes on | Does |
|---|---|---|---|
| 1 | `script` | any `script.*` node | Recomputes scene characters, word count, page eighths and estimated duration from current child elements; marks ScriptSync alignments stale when dialogue text changed |
| 2 | `previs` | `script.scene` characters / duration / heading | Ensures the derived previs scene; syncs character stand-ins onto a blocking grid, set bounds by INT/EXT, dialogue-track length; **never overwrites an artist-moved camera** |
| 3 | `logistics` | `script.scene`, `previs.camera`, `previs.character` | Rebuilds the breakdown sheet and the shot list from previs cameras, then reschedules the whole document and regenerates call sheets, recording a diff of scenes that moved days |
| 4 | `timeline` | `script.scene`, `logistics.shot`, `previs.shot` | Flags every clip `SyncedTo` the changed scene with `needs_conform` and a human-readable reason |
| 5 | `audio` | `script.scene`, `script.dialogue` | Ensures a DAW session and a per-scene dialogue track, then lays out one silent placeholder clip per line, sized from word count, removing clips whose line vanished |
| 6 | `comp` | `logistics.shot`, `previs.camera`, `script.scene` | Ensures a comp graph per shot bound to its previs camera, refreshes the frame range, marks it stale |
| 7 | `color` | `nle.clip`, `asset.media` | Ensures a grade per clip and propagates `ColorMeta` along media → clip → grade so a re-conform keeps its color space and LUT |

**Why the cascade terminates.** Three mechanisms together:

1. **Idempotent writers.** `tx.set` drops a write whose value is unchanged, so a
   reactor re-running on its own output produces zero ops and the wave dies.
2. **Deterministic derived IDs.** Re-deriving hits the same node instead of
   creating a new one each pass.
3. **A depth limit.** `max_cascade_depth` defaults to 8; exceeding it raises
   `UcgError::CascadeDepth`, which is treated as a reactor bug, never as a
   legitimate state. A test asserts a deliberately runaway reactor trips it.

---

## 4. DOMAIN CRATE BREAKDOWN

Dependency rule: domain crates depend only on `ucg-core` + `ucg-schema` and
workspace dependencies. **`cine-pipeline` is the only crate that depends on all
of them.** Domain crates expose pure functions; reactors live only in the
pipeline crate.

Each domain crate follows the same shape:

```
src/lib.rs        crate docs with a runnable example, pub mods, re-exports, prelude
src/error.rs      thiserror Error with From<UcgError>, Result alias
src/<domain>.rs   pure domain types and algorithms, NO graph dependency
src/graph.rs      (or ucg.rs) import/export/sync between domain types and the UCG
tests/*.rs        integration tests
```

### 4.0 Industry open standards, mapped per crate

Every standard below is implemented **natively in Rust in this repository**.
None of them link against the reference C++ libraries of the same name. The
conformance column states exactly how far each implementation goes, because a
model reasoning about this code must not assume a binding that does not exist.

| Crate | Open standards and formats implemented | Conformance |
|---|---|---|
| `ucg-core` | OpenTimelineIO time model (`RationalTime`, `TimeRange`), SMPTE non-drop timecode | Exact; integer frame counts at a rate are lossless |
| `ucg-schema` | USD prim/attribute data model, ASC CDL data model | Native data model, not a file format |
| `cine-script` | **Fountain** screenplay markup, **Final Draft FDX** XML | Full Fountain spec; both parse and write; round trip verified in both directions and across formats |
| `cine-previs` | **USD / USDA** text layer (`def Xform`, `def Camera`, Sphere/Rect/Distant lights, `xformOp` stack) | Native USDA writer plus a parser for what it writes. **Not linked to OpenUSD** |
| `cine-logistics` | Stripboard scheduling, call sheets, page eighths, lens grids | Industry convention; no formal published spec exists to conform to |
| `cine-timeline` | **OpenTimelineIO** JSON schema: `Timeline.1`, `Stack.1`, `Track.1`, `Clip.2` and legacy `Clip.1`, `Gap.1`, `Transition.1`, `ExternalReference.1`, `MissingReference.1`, `GeneratorReference.1`, `Marker.2` and `.1`, `RationalTime.1`, `TimeRange.1` | Lossless read and write of real OTIO documents. IDs survive round trips through external OTIO tools via a `ucg_id` metadata key. **Not linked to the OTIO library** |
| `cine-comp` | Porter-Duff compositing algebra, OpenEXR-style deep samples, AOV / multi-part layer conventions | Native; 15 Porter-Duff operations, deep merge and holdout |
| `cine-color` | **ACES** (AP0, AP1, ACES2065-1, ACEScg, ACEScc, ACEScct, RRT+ODT), **OpenColorIO v2** config YAML including `BuiltinTransform`, **ASC CDL** XML, **Iridas `.cube`** LUTs, **SMPTE ST.2084** PQ, **ITU-R BT.2100** HLG, ARRI LogC3/LogC4, Sony S-Log3, RED Log3G10, Canon Log 2/3, Panasonic V-Log | Colorimetry verified against published matrices to 1e-3 and transfer functions round trip to 1e-6. OCIO support is a documented subset with 24 builtin styles in both directions. **The output transform is the analytic RRT+ODT fit, not the official CTL** |
| `cine-audio` | **ITU-R BS.1770-4** loudness, **RIFF/WAVE** PCM 16/24 and float 32, **VBAP** (Pulkki), ITU downmix matrices, Dolby Atmos 7.1.4 bed layout | A calibrated −23 LUFS signal measures within ±0.5 LU; WAV round trips |
| `cine-gpu` | **WebGPU** API and **WGSL** shading language | Standard API through `wgpu`; shaders validated by `naga`; runs on Vulkan, Metal and DX12. Verified on Vulkan |

### 4.1 `ucg-core` — the engine (3,117 LOC, 21 tests)

Modules: `engine`, `graph`, `id`, `node`, `persist`, `time`, `tx`, `value`, `error`.

Exports `Engine`, `Graph`, `Node`, `Edge`, `EdgeKind`, `NodeKind`, `EntityId`,
`Value`, `Color`, `Tx`, `Op`, `Delta`, `ChangeSet`, `Plan`, `Reactor`,
`UndoGroup`, `Project`, `Manifest`, `RationalTime`, `TimeRange`, `UcgError`.

`RationalTime { value, rate }` and `TimeRange { start_time, duration }` are
modelled on OpenTimelineIO so editorial times round-trip losslessly. Integer
frame counts at a given rate are exact. SMPTE non-drop timecode parse and format
included.

`Engine` API: `transact`, `undo`, `redo`, `read`, `snapshot`, `revision`,
`add_reactor`, `reactor_names`, `subscribe`, `subscribe_channel`, `log`,
`replay`, `rebuild_history`, `without_reactions`, `stats`, `undo_depth`,
`redo_depth`, `max_cascade_depth`.

### 4.2 `ucg-schema` — the shared vocabulary (623 LOC, 2 tests)

The contract every module agrees on. **Never hard-code these strings elsewhere.**

`kinds`: 61 node-kind constants —
`project`, `asset.media`, `asset.model`,
`script.{document,title_page,scene,action,dialogue,parenthetical,transition,centered,section,synopsis,note,lyric,page_break,character,location,prop}`,
`sync.alignment`, `board.{frame,layer,stroke}`,
`previs.{stage,scene,prim,camera,light,rig,shot,stroke,character,dialogue_track}`,
`logistics.{breakdown,shot_list,shot,schedule,day,call_sheet,lens_grid,take,crew,cast}`,
`nle.{timeline,stack,track,clip,gap,transition,marker}`,
`comp.{graph,node}`, `color.{config,space,grade,lut}`,
`audio.{session,track,clip,bus,stem}`.

`attrs`: shared attribute keys including `name`, `order`, `text`, `heading`,
`scene_number`, `character`, `characters`, `locations`, `props`, `page_eighths`,
`est_duration_s`, `words`, `transform`, `bounds`, `camera`, `light`, `rate`,
`source_range`, `range`, `media_path`, `available_range`, `color_meta`,
`colorspace`, `cdl`, `shot`, `metadata`, `stale`, `needs_conform`,
`dirty_reason`, `alignment`.

Typed components implementing `Component` (each occupies one attribute):
`Transform` (TRS, quaternion, `to_mat4`/`from_mat4`), `Bounds`, `CameraParams`
(sensor mm, focal, f-stop, focus distance, shutter angle, ISO, white balance,
frame rate, resolution, clip), `LightParams` (type, intensity, Kelvin, tint,
cone, softness, size), `SceneHeading` (int/ext, location, time of day, raw),
`ClipRef`, `Alignment` + `WordAlignment`, `Cdl`, `ColorMeta`, `ShotSpec`,
`MetadataFrame`, plus `ShotSize` and `CameraMove` enums.

### 4.3 `cine-script` — screenplay and ScriptSync (4,329 LOC, 39 tests)

- **`fountain.rs`** — full Fountain spec: title page with multi-line values,
  scene headings with forced `.` and `#12A#` numbers, action with forced `!`,
  character cues with extensions and forced `@`, dual dialogue `^`,
  parentheticals, transitions with forced `>`, centered `>text<`, sections `#`,
  synopses `=`, notes `[[...]]`, boneyard `/* */`, page breaks `===`, lyrics
  `~`, emphasis preserved verbatim. `parse_fountain` / `write_fountain`.
- **`fdx.rs`** — Final Draft XML read and write with `SceneProperties`, dual
  dialogue, title page. `parse_fdx` / `write_fdx`.
- **`model.rs`** — `Screenplay { title_page, elements }`, `Element` enum,
  `SceneView` grouping, `parse_scene_heading`.
- **`tag.rs`** — `auto_tag` extracting characters from cues, locations from
  headings, and props from ALL-CAPS action nouns filtered by a stopword list
  plus a built-in prop lexicon. Deterministic, ordered by first appearance.
- **`stats.rs`** — `page_eighths` (industry heuristic: 55 lines/page, action
  wraps at 61 chars, dialogue at 35), `estimate_duration_seconds`, `word_list`.
- **`sync.rs`** — **ScriptSync word-level alignment.** Needleman–Wunsch global
  alignment with affine gaps over normalized Levenshtein similarity, aligning
  script words to a timed transcript. Unmatched words get interpolated timings
  and zero confidence. `align_words`, `align_to_frames`, `TimedWord`. Tested
  against a transcript corrupted with deletions, substitutions and insertions.
- **`graph.rs`** — `import_screenplay` (stable IDs from `source_key` + scene
  number, idempotent re-import that updates in place and removes vanished
  nodes), `export_screenplay` (bidirectional), `recompute_scene_stats`,
  `scene_dialogue_words`, `apply_alignment`, `mark_alignments_stale`.

### 4.4 `cine-previs` — optics, stage, storyboard (5,371 LOC, 46 tests)

- **`optics.rs`** — Physically accurate camera. Ten real sensor presets:
  ARRI ALEXA 35 / Mini LF / 65, RED V-RAPTOR 8K VV, Sony VENICE 2, Full Frame,
  Super 35, Micro Four Thirds, Super 16, IMAX 15/70. Horizontal, vertical and
  diagonal FOV; focal-for-FOV; crop factor. Depth of field: circle of confusion,
  hyperfocal, near/far limits with infinity handling, blur diameter in pixels.
  T-stop/f-stop transmission, exposure value, shutter angle to seconds.
  Projection and view matrices, `Frustum` with corners and planes, anamorphic
  squeeze from 1.0 to 2.0.
- **`light.rs`** — Kelvin to linear RGB via a Planckian-locus fit; photometric
  conversions (lumens to candela, inverse-square illuminance, lux to EV, stops
  between); fixture presets approximating SkyPanel S60-C, Aputure 600d, Kino
  Flo, and a 2K tungsten fresnel.
- **`stage.rs`** — USD-style `Stage` of `Prim`s at `PrimPath`s with composed
  world transforms, plus **USDA text export and a matching parser**: `def Xform`
  with `xformOp:translate/orient/scale` and `xformOpOrder`, `def Camera` with
  `focalLength`, apertures, `fStop`, `focusDistance`, `clippingRange`, and
  Sphere/Rect/Distant lights with `inputs:intensity` and `colorTemperature`.
- **`rig.rs`** — `CraneRig` forward kinematics (base → swing → boom → arm →
  head pan/tilt/roll), `DollyTrack` with Catmull-Rom interpolation, keyframe
  `Animation` over transforms and camera params for focal pulls.
- **`stroke.rs`** — **Hybrid 2D-on-3D grease pencil.** Strokes drawn in
  normalized camera-frame coordinates project through the frustum onto a plane
  at a chosen depth, and reproject back exactly (round trip within 1e-6).
  Ribbon geometry generation, Douglas–Peucker simplification, smoothing.
- **`board.rs`** — 2D storyboard canvas: layers, pressure-sensitive soft brush
  rasterization, compositing, onion skinning with opacity falloff, light table.
- **`graph.rs`** — `ensure_previs_scene`, `sync_scene_from_script` (returns a
  report of characters added/removed, duration change, camera creation),
  `add_camera`, `add_light`, `store_stroke`, `stage_from_graph`,
  `shot_size_for`.

### 4.5 `cine-logistics` — production management (4,731 LOC, 46 tests)

- **`breakdown.rs`** — Per-scene breakdown: cast, props, page eighths, estimated
  minutes, and vehicle/animal/SFX/VFX flags detected from props and headings.
  Markdown and CSV export.
- **`shotlist.rs`** — Derives one shot per previs camera, classifying shot size
  from the fraction of vertical FOV a 1.75 m subject fills, and building a gear
  list (body, lens, support from camera move, filters, focus puller needed).
  `lens_grid` snaps focal lengths to a standard prime set (14–135 mm) and
  recommends a kit.
- **`schedule.rs`** — **Stripboard scheduler.** Sorts strips by location, INT/EXT
  and day/night to minimize company moves, packs greedily under a page-per-day
  limit, keeps night scenes together, respects cast availability windows.
  Deterministic. `company_moves`, `cast_days`, markdown stripboard, and a
  `diff` that reports which scenes moved to which day.
- **`callsheet.rs`** — Cast numbers assigned by first appearance across the
  document, makeup calls offset for leads, crew calls, markdown export.
- **`metadata.rs`** — **Live on-set metadata ingestion.** A bounded ring buffer
  fed by a `crossbeam-channel` producer, tested at **240 Hz for 10 seconds with
  no dropped frames**. `summarize` produces min/max/mode for ISO, shutter angle,
  focal length (detecting zooms), aperture, focus distance and white balance;
  `detect_events` finds focal/aperture/ISO/WB changes with hysteresis and a
  settle window. `simulate_take` generates scenarios for testing.
- **`date.rs`** — Self-contained `Date`, `DateRange`, `Time`, `Weekday` with
  civil-days arithmetic. No chrono dependency.
- **`graph.rs`** — `write_breakdown`, `write_shot_list`, `write_schedule`,
  `write_call_sheet`, `write_take_metadata`, and matching readers. All
  idempotent, all removing surplus derived nodes.

### 4.6 `cine-timeline` — NLE editorial (4,750 LOC, 61 tests)

- **`otio.rs`** — OpenTimelineIO-shaped model: `Timeline`, `Stack`, `Track`
  (Video/Audio), `Item` (Clip/Gap/Transition), `MediaReference`
  (External/Missing/Generator), `Marker`. Composition math: `duration`,
  `range_of_child` accounting for transitions, `child_at_time`, `neighbors`,
  `media_time_at`, `validate`.
- **`otio_json.rs`** — **Lossless OTIO JSON** read and write using the real
  schema tags: `Timeline.1`, `Stack.1`, `Track.1`, `Clip.2` with
  `media_references` and `active_media_reference_key` (legacy `Clip.1` also
  accepted), `Gap.1`, `Transition.1`, `ExternalReference.1`,
  `MissingReference.1`, `GeneratorReference.1`, `Marker.2`, `RationalTime.1`,
  `TimeRange.1`.
- **`edit.rs`** — Real NLE semantics, each validating invariants afterward and
  rolling back on failure: `ripple_trim`, `roll`, `slip`, `slide`, `insert`,
  `overwrite`, `lift`, `extract`, `split_at`, `join`, plus `diff` reporting
  added/removed/moved/trimmed. Trims clamp to media `available_range` and return
  the actually-applied delta.
- **`sync.rs`** — **Dual-system audio sync.** Two-stage FFT cross-correlation:
  RMS envelopes for the coarse lag and confidence, then raw-sample correlation
  within a window for sample accuracy. A 1.234 s offset is recovered within
  ±5 ms. `find_offset`, `sync_clips`, `align_clip`.
- **`graph.rs`** — `import_timeline` / `export_timeline` with IDs that survive
  round trips through external OTIO tools by stamping `ucg_id` into item
  metadata; `clips_synced_to`, `bind_clip_to_scene`, `flag_conform`,
  `clear_conform`, `media_of_clip`, `apply_edit(EditOp)`.

### 4.7 `cine-comp` — VFX compositor (7,486 LOC, 65 tests)

- **`image.rs`** — Scene-linear f32 buffers, premultiplied alpha, channel sets
  including AOVs (beauty, depth, normal, ID, motion), multi-layer images, and
  **`DeepImage`** with per-pixel sorted `(z, z_back, rgba)` sample lists.
- **`graph_model.rs`** — `CompGraph` of `CompNode`s with typed ports (Image,
  Deep, Mask, Camera, Geometry, Scalar), connection validation, **cycle
  rejection via topological sort**, and full serde serialization.
- **`ops/`** — Node implementations: `Read` (file, generator, or **previs pass**),
  `Merge` (15 Porter-Duff ops), `Keyer` (chroma in YCbCr, luma, difference,
  screen subtraction, with spill suppression), `ColorCorrect`/`Grade`,
  `Transform`/`Crop`/`Reformat`, `Blur`/`Sharpen`/`Erode`/`Dilate`/`EdgeBlur`,
  `Roto` (bezier and polygon with feather), **`ZDefocus`** (depth-driven CoC),
  `DeepMerge`/`DeepToImage`/`DeepFromImage`/`DeepHoldout`, channel
  `Copy`/`Shuffle`, `Premult`/`Unpremult`/`Invert`/`Clamp`, `Expression`, and
  `Procedural` (fBm Perlin noise, gradient, checker, radial, Worley cellular).
- **`expr.rs`** — A small recursive-descent arithmetic evaluator over
  `r,g,b,a,x,y,w,h` with the usual operators and `pow/min/max/abs`.
- **`provider.rs`** — `PassProvider` is **the direct bridge to the 3D previs
  stage**: a comp `Read` node names a previs scene UUID, a camera UUID and a
  pass (Beauty, Depth, Normal, ID, Motion), and the provider supplies it.
  `SyntheticPassProvider` renders an analytic sphere with correct depth and
  normals so the crate tests without any renderer.
- **`eval.rs`** — Pull-model evaluator with a cache keyed on node parameters
  hashed together with upstream hashes, so changing a parameter invalidates only
  what is downstream. Independent branches evaluate in parallel via rayon.
- **`ucg.rs`** — `ensure_comp_for_shot` (creates a template graph on first
  call), `store_graph`, `load_graph`, `comps_depending_on`, `update_frame_range`,
  `mark_dirty`, `mark_clean`, `bind_previs_camera`.

### 4.8 `cine-color` — ACES/OCIO color science (9,890 LOC, 80 tests)

The largest crate. Verified against published reference values.

- **`space.rs`** — `Primaries` for Rec.709/sRGB, Rec.2020, DCI-P3, Display P3,
  ACES AP0, AP1, ARRI Wide Gamut 3 and 4, S-Gamut3, S-Gamut3.Cine, RED Wide
  Gamut, V-Gamut, Canon Cinema Gamut, XYZ D65. RGB↔XYZ matrices computed from
  primaries with Bradford chromatic adaptation, checked against known matrices
  to 1e-3. Planckian locus with `xy↔uv` and Duv offsets.
- **`transfer.rs`** — 15 transfer functions with exact formulas and round trips
  to 1e-6: Linear, sRGB, Gamma, Rec.709, **PQ (ST.2084)**, **HLG**, ARRI LogC3
  and **LogC4**, Sony S-Log3, RED Log3G10, Canon Log 2 and 3, Panasonic V-Log,
  ACEScc, ACEScct.
- **`aces.rs`** — AP0↔AP1, ACES2065-1 ↔ ACEScg ↔ ACEScc ↔ ACEScct verified
  against published values. `rrt_odt_fit_srgb` analytic RRT+ODT fit, plus a
  structured `OutputTransform` with a parametric tonescale, hue-preservation
  modes, SDR and HDR displays, and an **analytic tonescale inverse**. Camera
  IDTs to ACEScg for LogC4, LogC3, S-Log3, Log3G10, V-Log, Canon Log.
- **`ocio.rs`** + **`yaml.rs`** — A hand-written YAML-subset parser and an
  **OCIO v2 config reader**: roles, displays and views, view transforms,
  colorspaces with `to_reference`/`from_reference`, group transforms, and
  transform types Matrix, File, Exponent, ExponentWithLinear, Log family, CDL,
  Range, ColorSpace, DisplayView and **BuiltinTransform** (24 ACES styles, both
  directions). `Config::aces_default()` ships an ACES 1.3 CG config in code that
  serializes to YAML and parses back to an identical value.
- **`processor.rs`** — Op-chain `Processor` with adjacent-matrix concatenation,
  identity detection, and inversion. Data (`isdata`) colorspaces compile to an
  empty chain, per OCIO semantics.
- **`lut.rs`** — `.cube` 1D and 3D parse and write with domain handling, and
  **tetrahedral** (default) plus trilinear interpolation; `bake` a processor to
  a LUT.
- **`grade.rs`** / **`curve.rs`** / **`mask.rs`** — Lift/gamma/gain/offset wheels
  with a documented formula, ASC CDL with inverse and XML round trip,
  contrast with pivot, saturation, **luma-preserving hue rotation** (rotation in
  the plane orthogonal to the Rec.709 luma axis, exact to fp precision),
  temperature and tint as movement along and perpendicular to the Planckian
  locus with Bradford adaptation, monotone cubic Hermite curves, HSL qualifiers,
  and ellipse/rectangle/polygon/gradient masks with softness and boolean
  combination. `GradeStack` of masked layers applied with rayon.
- **`scopes.rs`** — Histogram, waveform, RGB parade, vectorscope with 75 % bar
  graticule targets, and **HDR scopes in nits** through the PQ EOTF with markers
  at 100, 203 and 1000 nits, plus clipping statistics.
- **`ucg.rs`** — `ensure_grade_for_clip`, `propagate_color_meta`, `store_grade`,
  `load_grade`, `grades_for`, `register_lut`, `install_config`,
  `processor_for_clip`.

### 4.9 `cine-audio` — DAW engine (6,430 LOC, 50 tests)

- **`buffer.rs`** / **`wav.rs`** — Interleaved f32 buffers, windowed-sinc
  resampling, **ITU-R BS.1770-4 integrated LUFS** with K-weighting and gating
  (a calibrated −23 LUFS sine measures within ±0.5 LU), 4× oversampled true
  peak. Self-written RIFF WAV read/write for PCM 16/24 and float 32.
- **`dsp.rs`** — RBJ biquads, STFT with verified COLA perfect reconstruction,
  envelope follower, compressor, gate, limiter, delay, fades and crossfades.
- **`pan.rs`** — **VBAP 3D panning** (Pulkki) over Mono, Stereo, LCR, 5.1, 7.1
  and **7.1.4 Atmos** speaker layouts with azimuth/elevation tables. A source at
  a speaker feeds only that speaker; midway between two feeds them equally;
  power sums to 1. Constant-power stereo laws and ITU downmix matrices.
- **`session.rs`** — `Session` of `Track`s and `Bus`es with `Clip`s, insert
  chains, sends, `PanState`, automation lanes with interpolation, and `Stem`
  routing (Dialogue, Music, Effects, Foley, Ambience, Custom).
- **`mix.rs`** — Block renderer, 512 frames per block, rayon over tracks,
  applying clip gain and fades, inserts, automation-sampled volume and pan, VBAP
  into the target layout, sends, bus processing, stem sums and a master limiter.
  `render_stems` delivers DME stems; their sum equals the full mix.
- **`isolate.rs`** — **Automated dialogue isolation** by STFT spectral gating
  with percentile noise-floor estimation, Wiener-style gain with temporal and
  spectral smoothing, and a spectral-flux plus band-energy voice activity
  detector. On synthetic speech at 0 dB SNR the output SNR improves by ≥ 8 dB.
- **`ucg.rs`** — `ensure_session`, `ensure_dialogue_track_for_scene`,
  `sync_dialogue_placeholders` (one placeholder per line, duration from word
  count at 2.6 words/s plus a 0.4 s pause, laid out sequentially, removing
  vanished lines), `store_session`, `load_session`, `clips_synced_to`.

### 4.10 `cine-pipeline` and `cine-cli`

`cine-pipeline` holds the seven reactors and `OmniProject`, the façade used by
the CLI and UI: `new`, `create`, `open`, `save`, `import_fountain`, `import_fdx`,
`import_otio`, `export_fountain`, `export_fdx`, `export_otio`, `export_usda`,
`edit_dialogue`, `scenes`, `dialogue_lines`, `scene_report`.

`cine-cli` produces the `cine` binary:

```
cine new <dir> [--name NAME]
cine import <dir> <file.fountain|.fdx|.otio>
cine report <dir>
cine export <dir> <fountain|fdx|usda|otio|breakdown|schedule|callsheets> [--scene N] [--out FILE]
cine edit <dir> <scene-index> <line-index> "new text"
cine undo <dir> | cine redo <dir>
cine stats <dir>
cine demo
cine serve <dir>
cine gpu
```

---

## 5. WORKSPACE TREE MAP

```
SanbatF&me/                          ← workspace root (note the '&': always quote paths)
├── Cargo.toml                       workspace manifest, 12 members, shared dep versions
├── Cargo.lock
├── rust-toolchain.toml              stable + rustfmt + clippy
├── README.md
├── CLAUDE.md                        build commands and the rules that bite
├── CHATGPT_CONTEXT.md               this file
├── .gitignore
├── .github/workflows/ci.yml         fmt, clippy -D warnings, test, wgpu build, demo, UI build
│
├── crates/
│   ├── ucg-core/                    THE ENGINE
│   │   ├── src/{lib,engine,graph,tx,node,value,time,id,persist,error}.rs
│   │   └── tests/engine_tests.rs
│   ├── ucg-schema/                  SHARED VOCABULARY
│   │   └── src/{lib,kinds,components,component}.rs
│   │
│   ├── cine-script/                 SCREENPLAY + SCRIPTSYNC
│   │   ├── src/{lib,fountain,fdx,model,tag,stats,sync,graph,error}.rs
│   │   └── tests/{fountain,fdx,graph,sync,tagging_stats,common/mod}.rs
│   ├── cine-previs/                 OPTICS + USD STAGE + STORYBOARD
│   │   ├── src/{lib,optics,light,stage,rig,stroke,board,graph,error}.rs
│   │   └── tests/{optics,light}.rs
│   ├── cine-logistics/              BREAKDOWN + SCHEDULE + ON-SET METADATA
│   │   ├── src/{lib,breakdown,shotlist,schedule,callsheet,metadata,date,graph,fixtures,error}.rs
│   │   └── tests/logistics.rs
│   ├── cine-timeline/               NLE / OPENTIMELINEIO
│   │   ├── src/{lib,otio,otio_json,edit,sync,graph,error}.rs
│   │   └── tests/{otio_fixture,edit_ops,sync_offset,graph_roundtrip}.rs
│   ├── cine-comp/                   VFX COMPOSITOR
│   │   ├── src/{lib,image,graph_model,eval,expr,provider,ucg,error}.rs
│   │   ├── src/ops/{mod,merge,keyer,color,transform,filter,roto,defocus,deep,procedural}.rs
│   │   └── tests/{nodes,evaluator,ucg_roundtrip}.rs
│   ├── cine-color/                  ACES / OCIO / GRADING / SCOPES
│   │   ├── src/{lib,space,transfer,aces,ocio,yaml,processor,lut,grade,curve,mask,scopes,ucg,error}.rs
│   │   └── tests/{ocio_fixture,pipeline}.rs
│   ├── cine-audio/                  DAW
│   │   ├── src/{lib,buffer,wav,dsp,pan,session,mix,isolate,ucg,error}.rs
│   │   └── tests/{mix,ucg}.rs
│   ├── cine-gpu/                    WGPU + WGSL
│   │   ├── src/{lib,kernels,frame_graph,cpu,device}.rs
│   │   ├── src/shaders/{common,merge,cdl,lut3d,scopes,blur,keyer,dof,previs}.wgsl
│   │   └── tests/shaders.rs
│   ├── cine-pipeline/               THE 7 REACTORS
│   │   ├── src/{lib,reactors,project}.rs
│   │   └── tests/thesis.rs          ← end-to-end proof of the thesis
│   └── cine-cli/
│       └── src/main.rs              CLI + JSON-RPC daemon
│
├── ui/                              REACT + VITE SHELL
│   ├── package.json, package-lock.json, tsconfig.json, vite.config.ts, index.html
│   ├── server/bridge.mjs            WebSocket :7331 ⇄ `cine serve` stdio
│   └── src/
│       ├── main.tsx, App.tsx, styles.css
│       ├── rpc/{client.ts, client.test.ts}
│       └── panels/{ScriptPanel,Viewport,CascadePanel,InspectorPanel}.tsx
│
├── docs/
│   ├── ARCHITECTURE.md              the deep-dive design document
│   ├── MODULE-CONVENTIONS.md        the 10 rules every domain crate follows
│   └── adr/
│       ├── 0001-unified-context-graph.md    why one graph, alternatives rejected
│       ├── 0002-toolchain.md                Rust + wgpu + TS, and the Zig linker
│       └── 0003-reactor-cascade.md          cascade semantics and convergence
│
└── tests/fixtures/                  shared across crates
    ├── sample.fountain, sample.fdx           7-scene screenplay
    ├── sample.otio                           4-track cut with a dissolve
    ├── mini.ocio                             OCIO config exercising the parser
    └── identity.cube, gamma_shaper.cube      LUTs
```

---

## 6. BUILD, RUN AND VERIFY

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

cargo test --workspace                                   # 424 tests
cargo clippy --workspace --all-targets -- -D warnings     # clean
cargo fmt --all -- --check                                # clean
cargo run -p cine-cli -- demo                             # the thesis walkthrough
cargo test -p cine-gpu --features wgpu                    # GPU vs CPU parity
cd ui && npm test && npm run build
```

**Toolchain note specific to this machine.** There is no system C compiler and
no sudo. Rust was installed via rustup, and **Zig acts as the linker and C
compiler** through wrapper scripts wired into `~/.cargo/config.toml`:

```toml
[target.x86_64-unknown-linux-gnu]
linker = "/home/<user>/.local/bin/zigcc"   # exec zig cc -target x86_64-linux-gnu.2.39 "$@"
[env]
CC  = "/home/<user>/.local/bin/zigcc"
CXX = "/home/<user>/.local/bin/zigcxx"
```

Any machine with a working `cc` needs none of this.

**Running the app:**

```bash
# backend: WebSocket bridge that spawns `cine serve`
cd ui && node server/bridge.mjs ~/.local/share/cine-omni/Demo.cineomni \
  --port 7331 --bin ../target/debug/cine

# frontend
cd ui && npm run dev            # http://localhost:5173
```

---

## 7. INVARIANTS — VIOLATE THESE AND THE BUILD OR THE RUNTIME BREAKS

1. **Never hold `engine.read()` across `engine.transact(...)`.** Silent
   deadlock, no panic, no timeout. Scope every guard in its own block.
2. **Graph writers must be idempotent.** Running one twice with the same input
   must produce zero ops (`group.deltas.is_empty()`). Reactor convergence
   depends on it entirely. Every `write_*` / `ensure_*` / `sync_*` function has a
   test asserting this.
3. **Node kinds and attribute keys come from `ucg_schema::{kinds, attrs}`.**
   Never hard-code the strings.
4. **Derived entities use deterministic IDs:** `EntityId::derived(source, kind)`
   for derivations, `EntityId::from_key("<importer>:<stable key>")` for imports.
5. **Reactors live only in `cine-pipeline`.** Domain crates expose pure
   functions and know nothing about each other.
6. **Domain logic must not depend on the graph.** Parsers, optics, DSP and color
   math take plain values and return plain values, unit-tested without an
   `Engine`. Graph glue lives in `graph.rs` or `ucg.rs`.
7. **Cross-module links are edges, not attributes.**
8. **Errors:** each crate has its own `thiserror` error with `From<UcgError>`.
   Inside a `transact` closure, convert with
   `.map_err(|e| UcgError::Aborted(e.to_string()))` — the orphan rule prevents a
   blanket `From` impl in the other direction.
9. **No `unwrap()` in library code** on user-data paths.
10. **A propagating edge op marks only `edge.from` dirty**, never `edge.to`.
    Marking both makes cascades oscillate.

---

## 8. WHERE TO LOOK FIRST

| Question | File |
|---|---|
| How does the whole thing hang together? | `docs/ARCHITECTURE.md` |
| How does one edit fan out? | `crates/cine-pipeline/src/reactors.rs` |
| Does the thesis actually work? | `crates/cine-pipeline/tests/thesis.rs` |
| How does a transaction commit? | `crates/ucg-core/src/engine.rs`, `commit_kind` |
| What is the shared vocabulary? | `crates/ucg-schema/src/kinds.rs` |
| How do I add a module? | `docs/MODULE-CONVENTIONS.md` |
| Why one graph rather than an ECS or a database? | `docs/adr/0001-unified-context-graph.md` |
| Why does the cascade terminate? | `docs/adr/0003-reactor-cascade.md` |
| How does the browser reach Rust? | `ui/server/bridge.mjs` + `crates/cine-cli/src/main.rs` |

---

## 9. HONEST GAPS AND NEXT STEPS

- **No media I/O.** No video decode, no audio device output, no image file
  reading. Media is referenced by path and range only.
- **One daemon per WebSocket client**, so multiple browser tabs diverge. A
  shared daemon with fan-out is the natural fix.
- **The UI is a shell**, not an editing application. There is no timeline
  widget, no node editor canvas and no drawing surface yet; the panels prove the
  data path.
- **The GPU path is compute-only in practice.** The previs render pipeline is
  written and validated but is not yet driven by a windowed swapchain.
- **Collaboration is designed for, not implemented.** Every delta already
  carries `(actor, lamport)` so a CRDT merge layer can sit on the op log without
  touching module code.
- **The ACES output transform is an approximation**, not the official CTL.
- **Not a git repository** at the time of writing.
