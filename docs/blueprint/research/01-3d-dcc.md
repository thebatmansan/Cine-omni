# 01 — The 3D / DCC Ecosystem

**CINE-OMNI industry blueprint · research dossier**
Prepared: 2026-09-08 · Directive §7 (Blender depth) and §34 (OpenUSD adoption)

---

## 0. Scope, method and rules of evidence

This document evaluates the open-source 3D and digital-content-creation ecosystem
against CINE-OMNI's architecture — a Rust workspace of twelve crates built around
the **Unified Context Graph (UCG)**: a reactive, cycle-*rejecting* DAG that owns
identity (UUID v4 authored / UUID v5 derived), invertible ops, atomic
transactions, grouped one-step undo, append-only op-log persistence, pure
reactors, and a `wgpu` + WGSL GPU layer. See `docs/ARCHITECTURE.md` and
`CHATGPT_CONTEXT.md`.

**Method.** Every licence, canonical repository URL, copyright holder and current
release cited below was verified against a primary source (the project's own
`LICENSE` file, `README`, package registry metadata, or the source file headers)
during preparation. The URL used for verification is given inline. Nothing about
a licence is asserted from memory. Where a fact could not be established from a
primary source it is marked `UNVERIFIED`.

**No compatibility claim in this document has been tested.** Nothing here has
been compiled, linked or run against CINE-OMNI. Statements about integration are
architectural analysis, not test results.

### 0.1 The licensing constraint that governs everything

CINE-OMNI declares `license = "Apache-2.0"` in its workspace manifest
(`Cargo.toml`, `[workspace.package]`). The consequences are not symmetric and
must be stated precisely:

| Direction | Permitted? |
|---|---|
| Apache-2.0 code incorporated into a GPL-3.0 project | **Yes** — Apache-2.0 is one-way compatible with GPLv3 |
| GPL-3.0 code incorporated into (or linked by) Apache-2.0 CINE-OMNI | **No** — the combined work would have to be GPLv3 |
| GPL-2.0-**only** code combined with Apache-2.0 code | **No** — the two are outright incompatible (the Apache patent/termination clauses are an "additional restriction" under GPLv2) |
| MIT / BSD-2 / BSD-3 / Zlib / BSL-1.0 code incorporated into CINE-OMNI | **Yes**, with attribution |
| MPL-2.0 code | Yes as a separate file-level unit; MPL is file-level copyleft, so modified MPL files stay MPL |

**Therefore: no GPL source may be copied into, statically linked into, or
dynamically linked into CINE-OMNI.** A GPL project can only ever be:

* **TYPE C** — a separate process behind an interface,
* **TYPE D** — an external application driven over a protocol,
* **TYPE E** — file interchange only,
* **TYPE G** — a design studied and independently reimplemented (ideas are not
  copyrightable; expression is).

This rule is restated at each project where it bites.

### 0.2 Integration taxonomy

| Type | Meaning |
|---|---|
| **A NATIVE** | Written into CINE-OMNI as first-party Rust |
| **B LIBRARY** | Internal dependency, linked into the CINE-OMNI process |
| **C ENGINE** | Separate process CINE-OMNI owns and drives behind a clean interface |
| **D BRIDGE** | External application the user already runs, driven over a protocol |
| **E IMPORT/EXPORT** | File interchange only; no code relationship |
| **F FORK** | A legally permissible, maintained fork |
| **G INSPIRED REIMPLEMENTATION** | Study the design, implement independently, never copy code |
| **H REJECT** | Not adopted; justified |

---

# Part I — Primary profiles

---

## 1. Blender

### 1.1 The 31-point profile

**1 · Name.** Blender.

**2 · Repository URL.** Canonical development repository:
`https://projects.blender.org/blender/blender` (self-hosted Gitea). Official
mirror: `https://github.com/blender/blender` — the mirror's own description
string is *"Official mirror of Blender"*, verified via the GitHub repository API.

**3 · Licence — verified, and more nuanced than usually reported.**

Three primary sources, which must be read together:

* `COPYING` at the repository root
  (`https://raw.githubusercontent.com/blender/blender/main/COPYING`) states
  verbatim: *"Blender uses the GNU General Public License, which describes the
  rights to distribute or change the code. … Apart from the GNU GPL, Blender is
  not available under other licenses."*
* `README.md`
  (`https://raw.githubusercontent.com/blender/blender/main/README.md`) states
  verbatim: *"Blender as a whole is licensed under the GNU General Public
  License, Version 3. Individual files may have a different but compatible
  license."*
* `https://www.blender.org/about/license/` states verbatim: *"The source code we
  develop at blender.org is default being licensed as GNU GPL Version 2 or
  later. Some modules we make are using more permissive licenses, though, for
  example, the Blender Cycles rendering engine is available as Apache 2.0."* and
  *"All the components that together make Blender are compatible under the newer
  GNU GPL Version 3 or later. That is also the license to use for any
  distribution of Blender binaries."*

Confirmed at the file level by SPDX headers:

| Path | SPDX identifier (verified in file header) |
|---|---|
| `source/blender/depsgraph/DEG_depsgraph.hh` | `GPL-2.0-or-later` |
| `source/blender/makesdna/DNA_ID.h` | `GPL-2.0-or-later` |
| `source/blender/gpu/GPU_context.hh` | `GPL-2.0-or-later` |
| `intern/cycles/session/session.h` | **`Apache-2.0`** |
| `intern/cycles/kernel/types.h` | **`Apache-2.0`** |
| `intern/libmv/libmv/simple_pipeline/tracks.h` | **MIT** (full MIT text in header, "Copyright (c) 2011 libmv authors") |

The permitted identifiers are enumerated in
`doc/license/SPDX-license-identifiers.txt`: `Apache-2.0`, `BSD-2-Clause`,
`BSD-3-Clause`, `BSL-1.0`, `GPL-2.0-or-later`, `GPL-3.0-or-later`,
`LGPL-2.1-or-later`, `MIT`, `MPL-2.0`, `Zlib`.

GitHub's licence detector reports `NOASSERTION` for the repository, which is
correct: Blender is not a single-licence codebase.

> **Consequence for CINE-OMNI.** The Blender *application* — the DNA/ID system,
> the depsgraph, the window manager, the operator system, the compositor, the
> VSE, the GPU module, the geometry-nodes evaluator — is **GPL-2.0-or-later, and
> the assembled whole is GPLv3**. None of it may be copied or linked into
> Apache-2.0 CINE-OMNI. Blender can only be **TYPE C, D, E or G**.
>
> **But two Blender sub-trees are separately and permissively licensed and
> therefore *are* legally incorporable:** `intern/cycles` (Apache-2.0) and
> `intern/libmv` (MIT). This is the single most useful licensing fact in this
> section and it is routinely missed.

**4 · Copyright owner.** No single owner. File headers use
`SPDX-FileCopyrightText` attributions including *"Blender Authors"* (the modern
collective form, e.g. `DEG_depsgraph.hh`, 2013), *"NaN Holding BV"* (the original
1990s company, e.g. `DNA_ID.h`, 2001–2002), *"Blender Foundation"*
(`intern/cycles`, 2011–2022), *"Mike Erwin"* (`GPU_context.hh`, 2016), and
*"libmv authors"*. Copyright is distributed across contributors; the Blender
Foundation does not hold an assignment-based single copyright.

**5 · Main language.** C++ (GitHub reports `C++` as the primary language).
Historically C, now aggressively modernised C++17/20 (`blender::Vector`,
`blender::Map`, `StringRef` etc. in `BLI_`). Python 3 for the UI definition,
add-ons and tooling. GLSL/MSL/Vulkan-GLSL for shaders; CUDA/OptiX/HIP/Metal/SYCL
for Cycles device kernels.

**6 · Architecture.** A single monolithic process organised in layers:

```
DNA          plain-old-data structs, the on-disk format (source/blender/makesdna)
RNA          runtime introspection/reflection over DNA (source/blender/makesrna)
BKE          "blenkernel" — the data-block business logic
DEG          the dependency graph: builds relations, evaluates, owns evaluated copies
BLI          the generic library (containers, math, threading)
GPU          backend-abstracted graphics (OpenGL / Metal / Vulkan)
WM           the window manager: events, operators, undo, jobs
editors/     per-editor UI and tools
nodes/       node systems (shader, geometry, compositor)
io/          importers/exporters (alembic, usd, gltf, obj, ply, stl, collada, fbx)
intern/      vendored-but-first-party subsystems (cycles, libmv, ghost, guardedalloc, mikktspace)
extern/      third-party libraries
```

The layering rule is that DNA is dumb data, RNA describes it, BKE operates on
it, DEG schedules the operations, and WM/editors drive it via operators. **This
is strikingly close in spirit to CINE-OMNI's own rule that domain crates are pure
and only `graph.rs` touches the UCG.**

**7 · UI architecture.** Blender draws its entire UI itself — there is no native
widget toolkit. GHOST (`intern/ghost`) abstracts the platform window/event
system; everything above it is immediate-mode-ish retained layout code in
`editors/interface`, drawn through the GPU module. The screen is a tiling tree of
*areas*, each hosting a *space* (3D View, Timeline, Node Editor, Sequencer,
Outliner, …); layouts are saved as *workspaces* (a data-block). The entire panel
and menu layout is declared in **Python** (`scripts/startup/bl_ui/`), which is
why the Python API can restructure the UI. Buttons are bound to RNA paths, so a
property edit is a generic RNA set rather than bespoke callback code.

**8 · Rendering architecture.** Three separate renderers:

* **EEVEE** — a rasterisation engine running in the GPU module's backend
  abstraction (real-time / near-real-time, screen-space and probe-based GI).
* **Cycles** (`intern/cycles`, Apache-2.0) — a physically based path tracer with
  a device abstraction. `intern/cycles/device/device.h` declares
  `enum DeviceType { DEVICE_NONE, DEVICE_CPU, DEVICE_CUDA, DEVICE_MULTI,
  DEVICE_OPTIX, DEVICE_HIP, DEVICE_HIPRT, DEVICE_METAL, DEVICE_ONEAPI,
  DEVICE_DUMMY }`. Note the absence of Vulkan and WebGPU.
* **Workbench** — the solid/preview viewport shading engine.

Cycles additionally ships a **Hydra render delegate** (`intern/cycles/hydra`),
i.e. Cycles can be driven from a USD/Hydra host rather than from Blender.

**9 · Media architecture.** FFmpeg for video decode/encode; OpenImageIO and
OpenEXR for stills; OpenColorIO for the colour-managed display pipeline; SDL/
Jack/CoreAudio/WASAPI via `intern/audaspace` for audio. The Video Sequence Editor
(`source/blender/sequencer`) is the NLE; it has its own strip model, proxy/
prefetch cache and effect stack.

**10 · Plugin architecture.** Python add-ons only, at the application level.
There is **no stable C/C++ ABI plugin interface** — a C++ "plugin" means
compiling it into Blender, which is a GPL derivative work. Add-ons register
`bpy.types.Operator`, `Panel`, `Node`, `RenderEngine` subclasses. External render
engines integrate through the `bpy.types.RenderEngine` API. `blender.org` states
explicitly that the Python API is *"an integral part of the software"* and that
published scripts must be GPL-compatible — meaning **even the Python add-on
surface is inside the GPL boundary** for anything distributed.

**11 · Data model — the data-block (ID) system.** *This is the single most
important thing to understand about Blender and is treated in depth in §1.2.*

**12 · File formats.** `.blend` — a memory-image format: DNA structs are written
essentially verbatim along with a self-describing SDNA catalogue of the struct
layouts, which is what makes old files forward-readable. Also `.blend1` backups,
packed libraries, and the whole `io/` importer/exporter set.

**13 · Import/export capability.** Alembic, USD (import *and* export, including
USDZ), glTF 2.0, OBJ, PLY, STL, FBX, Collada, SVG, and the video/audio formats
via FFmpeg. The USD module (`source/blender/io/usd/usd.hh`, SPDX
`GPL-2.0-or-later`) is unusually complete: it exposes `MtlNameCollisionMode`,
`MtlPurpose` (allPurpose / preview / render), `PropertyImportMode`,
`TexImportMode` (including reading textures out of a USDZ package),
`TexNameCollisionMode`, `SubdivExportMode { Ignore, Tessellate, Match }` and
`XformOpMode { TRS, … }`. **This is the interchange surface any CINE-OMNI ↔
Blender integration should target.**

**14 · GPU architecture.** `source/blender/gpu` is a backend-abstracted graphics
layer — `GPU_context.hh` documents it as *"GPU back-ends abstract the differences
between different APIs"*, with `GPU_backend_type_selection_set` /
`GPU_backend_get_type` and automatic detection of "the most suited
GPUBackendType". Backends: OpenGL, Metal, Vulkan. Cycles has an entirely separate
device layer (CUDA / OptiX / HIP / HIP-RT / Metal / oneAPI).
**Neither uses WebGPU/wgpu**, which is CINE-OMNI's substrate.

**15 · CPU architecture.** x86-64 with SSE2 baseline and runtime dispatch to
AVX2 in Cycles; ARM64 (Apple Silicon, and Linux/Windows on ARM). `BLI_` provides
SIMD math wrappers; Cycles kernels are compiled per-ISA and selected at runtime.

**16 · Threading model.** TBB (`oneTBB`) task arenas for parallel_for /
task-group work; the depsgraph evaluates operation nodes on a task pool honouring
the relation ordering; Cycles has its own scheduler and per-device queues; the WM
runs long operations as *jobs* on background threads with an "interface lock"
protecting the UI from concurrent data mutation. A commit fetched during this
research (2026-09-08) fixing *"Crash during rendering with pre-render handler"*
notes explicitly that *"altering data in handlers can cause data race conditions
when drawing at the same time"* — i.e. Blender's threading safety is enforced by
convention plus a UI lock, not by the type system. **Rust's ownership model gives
CINE-OMNI a structural advantage here that Blender cannot retrofit.**

**17 · Caching model.** Multi-layered and largely explicit:
* the depsgraph's *evaluated* copies (formerly "copy-on-write", now spelled
  **`COPY_ON_EVAL`** in `NodeType`) are themselves the primary cache;
* per-modifier and per-node evaluation caches;
* point caches / bakes for simulation (`NodeType::POINT_CACHE`);
* `BATCH_CACHE` for GPU draw batches (`NodeType::BATCH_CACHE`);
* VSE proxies and prefetch;
* the compositor's `COM_static_cache_manager.hh`;
* image/movie caches with a global memory limit in preferences.

**18 · Extensibility.** Python add-ons; the Extensions platform
(`extensions.blender.org`); geometry-node groups usable as assets and as
operators (`OPTYPE_NODE_TOOL` exists in `WM_types.hh`, i.e. a node group can
register itself as a tool); custom render engines; custom `IDProperty` /
`system_properties` on any data-block (`struct ID` carries both a user
`IDProperty *properties` and a system `IDProperty *system_properties`).

**19 · Maturity.** Extremely high. Continuous development since 1994; ~20,159
GitHub stars on the mirror; used in feature-film, episodic and games production.
Latest tag at time of writing: **`v5.2.1`**, with the tag series
`v5.2.1, v5.2.0, v5.1.2, v5.1.1, v5.1.0, v5.0.1, v5.0.0, v4.5.13 …` verified via
the GitHub tags API.

**20 · Development activity.** Very high. The mirror's `pushed_at` was
`2026-09-08T06:34:50Z` — the same day this research was performed. Three commits
sampled from that day concerned motion-path depsgraph evaluation ranges, a
pre-render handler crash, and an Outliner filter bug; each carried a
`projects.blender.org/blender/blender/pulls/…` reference, confirming that Gitea,
not GitHub, is where review happens.

**21 · Modules relevant to CINE-OMNI.**

| Blender module | Relevance |
|---|---|
| `depsgraph` | Direct architectural competitor/teacher to the UCG reactor cascade (§1.3) |
| `makesdna` / `blenkernel` ID system | The data-block identity model (§1.2) |
| `windowmanager` operators + `BKE_undo_system` | Operator/undo design (§1.2) |
| `io/usd` | The interchange contract for any CINE-OMNI↔Blender bridge |
| `sequencer` (VSE) | Comparison point for `cine-timeline` |
| `compositor` | Comparison point for `cine-comp` |
| `intern/libmv` (**MIT**) | Camera solving / match-moving — *legally incorporable* |
| `intern/cycles` (**Apache-2.0**) | Path tracing — *legally incorporable*, and Hydra-drivable |
| `asset_system` + `AS_asset_catalog` | Asset catalogue design for CINE-OMNI's `assets/` |
| `nodes/geometry` | Procedural-graph design lessons |

**22 · Reusable components (legally).** Only the permissively licensed
sub-trees: **Cycles** (Apache-2.0) and **libmv** (MIT). Also several vendored
`extern/` libraries under their own permissive terms. Everything under
`source/blender/` is off limits as code.

**23 · Reusable algorithms.** Algorithms are ideas, not expression, and may be
independently reimplemented from published descriptions (never by transcribing
Blender's source): the depsgraph's component/operation node decomposition;
`memfile` undo with unchanged-ID reuse; library overrides as a
property-operation list; the `session_uid` stable-identity trick; grease-pencil
stroke fitting; the VSE's strip/effect model; motion-path caching.

**24 · Integration options.** Four realistic ones — analysed with costs in §1.4:
headless `blender --background --python`; a persistent Blender process speaking a
socket protocol; `bpy` as a Python module in a sidecar process; and pure file
interchange over USD/Alembic.

**25 · Conflicts with CINE-OMNI's architecture.**
* **Identity.** Blender's identity is `(ID_Type code, name string, owning
  Library)` — a *name-scoped* identity — with `session_uid` only stable within a
  session. CINE-OMNI's identity is a durable UUID. Any round trip must carry
  CINE-OMNI's UUID as a custom `IDProperty` and treat Blender names as
  display-only, exactly as `cine-timeline` already does with its `ucg_id` OTIO
  metadata key.
* **Cycle policy.** Blender's depsgraph *tolerates* cycles by breaking one edge;
  the UCG *rejects* the transaction. These are irreconcilable philosophies
  (§1.3).
* **Process/GPU.** Blender owns a window, a GPU context and a GIL-bearing Python
  runtime. CINE-OMNI is a headless Rust core with a `wgpu` device and a
  TypeScript shell. Sharing a GPU device between them is not realistic.
* **Undo.** Blender's global undo is a whole-`Main` snapshot; CINE-OMNI's is an
  invertible op log. Blender's cannot be merged into CINE-OMNI's op log.
* **Licence.** The decisive one.

**26 · Licensing constraints.** GPL-2.0-or-later per file, GPLv3 as a whole
(§1.1). Even the Python add-on surface is inside the boundary per blender.org.
Two sub-trees escape it: Cycles (Apache-2.0) and libmv (MIT).

**27 · Can source be legally incorporated given Apache-2.0?**
**No — for `source/blender/`, `intern/ghost`, `intern/guardedalloc` and anything
else marked `GPL-2.0-or-later`.** Copying or linking any of it would force
CINE-OMNI to become GPLv3, and mixing it with Apache-2.0 code at all is
prohibited for the GPL-2.0-only parts of the ecosystem.
**Yes — for `intern/cycles` (Apache-2.0, same licence as CINE-OMNI) and
`intern/libmv` (MIT).** Both may be vendored, forked or linked with attribution.

**28 · Should it remain isolated?** Yes. Blender must never be inside the
CINE-OMNI process. The GPL boundary and the process/GPU/Python-runtime conflicts
both point the same way.

**29 · Should it be an external engine?** Partly. Blender is best treated as a
**TYPE D bridge** — an application the *user already has* — rather than a TYPE C
engine CINE-OMNI ships and owns, because shipping it would raise redistribution
and version-pinning obligations for no gain. See §1.4.

**30 · Should an adapter be built?** Yes: a `cine-bridge-blender` crate that
speaks USD (and Alembic for caches) and drives a user-supplied Blender
installation through a generated Python entry script. It must marshal CINE-OMNI
UUIDs as USD prim custom metadata so identity survives the round trip.

**31 · Should its ideas instead be independently reimplemented?** Yes — and this
is where most of the value is. See §1.5 for the specific TYPE G list.

### 1.2 Deep dive I — data-blocks, operators, undo, overrides, assets, workspaces

#### 1.2.1 The data-block (ID) architecture

Everything user-visible in Blender that can be named, linked, reused or saved is
an **ID data-block**: Object, Mesh, Material, Scene, Collection, Action, Image,
NodeTree, WorkSpace, Library, Brush, Text, Sound. The `struct ID` header is
embedded as the first member of each of those structs (verified in
`DNA_collection_types.h`: `struct Collection { … ID id; … }`), which is Blender's
C-era inheritance.

Verified fields of `struct ID`
(`https://raw.githubusercontent.com/blender/blender/main/source/blender/makesdna/DNA_ID.h`):

| Field | Purpose |
|---|---|
| `char name[258]` | *"Main identifier … Must be unique within the ID name-space (defined by its type, and owning Library). The first two bytes are always the `ID_Type` code."* |
| `struct Library *lib` | Which linked `.blend` this ID came from; `nullptr` for local |
| `int us` | Reference count ("users"); drives garbage collection on save |
| `unsigned int session_uid` | *"A session-wide unique identifier for a given ID, that remain the same across potential re-allocations (e.g. due to undo/redo steps)"* |
| `IDHash deep_hash` | Content hash over the data-block **and all its dependencies**, used to decide that packed data-blocks in two different `.blend` files are interchangeable |
| `IDProperty *properties` / `*system_properties` | User custom properties and system-defined dynamic properties |
| `IDOverrideLibrary *override_library` | The override record (below) |
| `struct ID *orig_id` | *"Only set for data-blocks which are coming from copy-on-evaluation, points to the original version of it"* |
| `unsigned int recalc`, `recalc_up_to_undo_push`, `recalc_after_undo_push` | Depsgraph dirty flags, with the last two explicitly *"used by undo code … to find the changes between undo states"* |
| `LibraryWeakReference *library_weak_reference` | Lets a re-append reuse already-appended data instead of duplicating |
| `AssetMetaData *asset_data` | Non-null iff this data-block is an asset |

**Four observations that matter to CINE-OMNI.**

1. **Blender's canonical identity is a *string*, namespaced by type and library.**
   `name[258]` with a two-byte type prefix is the primary key. This is the root
   cause of a whole class of Blender pain: renaming breaks references written by
   name (see `UndoRefID` in `BKE_undo_system.hh`, which stores
   `char name[MAX_ID_NAME]` *and* `char library_filepath_abs[FILE_MAX]` precisely
   to re-find an ID after an undo reallocates it), and name collisions across
   libraries require disambiguation (see `subitem_reference_id` in
   `IDOverrideLibraryPropertyOperation`, whose comment says it exists *"to help
   disambiguate cases where several IDs from different libraries have the exact
   same name"*).

   **CINE-OMNI's `EntityId(Uuid)` is simply better, and this is the strongest
   available evidence for that design choice.** Blender bolted on `session_uid`
   to get identity stability across undo — but only *within a session*. CINE-OMNI
   already has permanent, cross-session, cross-process identity, and
   `EntityId::derived(source, kind)` gives it deterministically for derived data
   too, which Blender has no equivalent of at all.

2. **`us` — refcounting — is how Blender decides what to keep.** CINE-OMNI's
   `Contains`/`Binds` edges make ownership explicit in the graph instead, which
   is a cleaner model, but CINE-OMNI currently has *no garbage collection story*
   for orphaned nodes. Blender's answer (refcount, plus "fake user" pinning, plus
   drop-zero-user-IDs-on-save) is worth stealing conceptually: a UCG
   `Project::compact()` should probably also drop nodes unreachable from the
   project root over `Contains`, unless pinned.

3. **`orig_id` is the copy-on-eval backpointer.** Blender evaluates on a *copy*
   and keeps a pointer back to the original. CINE-OMNI's equivalent is
   `engine.snapshot()` returning an owned clone plus `DerivedFrom` edges — but
   note that Blender's copy is a full parallel object graph maintained per
   depsgraph (per window!), whereas CINE-OMNI's snapshot is a whole-graph clone.
   At film scale the whole-graph clone is the thing that will bite first.

4. **`deep_hash` is a content hash over a data-block *and its dependency
   closure*.** CINE-OMNI has `modified_rev` per node but nothing that answers
   "are these two subgraphs identical?" A dependency-closure content hash would
   give CINE-OMNI free deduplication of derived assets and a cheap
   "has-anything-actually-changed" test for reactors — a natural strengthening of
   the existing idempotence invariant.

#### 1.2.2 The operator system

`struct wmOperatorType` (verified in
`source/blender/windowmanager/WM_types.hh`) is Blender's command pattern. Its
callbacks:

| Callback | Contract (verbatim from the header where quoted) |
|---|---|
| `poll(C)` | *"Verify if the operator can be executed in the current context"* |
| `exec(C, op)` | *"executes the operator without any interactive input, parameters may be provided through operator properties. cannot use any interface code or input device state"* |
| `invoke(C, op, event)` | Interactive entry point; may start a modal operator |
| `modal(C, op, event)` | *"used for operators which continuously run … They keep running until they don't return `OPERATOR_RUNNING_MODAL`"* |
| `cancel(C, op)` | *"Called when a modal operator is canceled"* |
| `check(C, op)` | Re-validate/correct properties when one changes |
| `ui(C, op)` | *"Optional panel for redo and repeat, auto-generated if not set"* |

and the flags in `enum` (verified): `OPTYPE_REGISTER` (*"Register operators in
stack after finishing (needed for redo)"*), `OPTYPE_UNDO` (*"Do an undo push
after the operator runs"*), `OPTYPE_UNDO_GROUPED` (*"Special type of undo which
doesn't store itself multiple times"*), `OPTYPE_MACRO`, `OPTYPE_BLOCKING`,
`OPTYPE_INTERNAL`, `OPTYPE_LOCK_BYPASS` (*"Allow operator to run when interface
is locked"*), `OPTYPE_MODAL_PRIORITY`, `OPTYPE_NODE_TOOL`, and the
cursor-grab/preset/depends-on-cursor family. Every operator also carries
`StructRNA *srna` (its typed property schema), `IDProperty *last_properties`
(*"Previous settings - for initializing on re-use"*) and `const char *undo_group`.

**Why this design is important.** Because *every* mutation is an operator with a
declared property schema, Blender gets, for free: a searchable command palette;
keymaps bound to `(idname, properties)`; the "Adjust Last Operation" redo panel
generated from the RNA schema; macro recording; full Python scriptability
(`bpy.ops.*` is a generated mirror of the operator registry); and a uniform undo
policy expressed as a flag rather than as hand-written code at each call site.

**The critical difference from CINE-OMNI.** A Blender operator is *imperative*:
it mutates `Main` directly and then requests an undo push. A CINE-OMNI
transaction is *declarative and invertible*: `engine.transact(label, |tx| …)`
records ops and computes the inverse delta. CINE-OMNI's model is strictly
stronger for undo and persistence — but CINE-OMNI has **no operator registry at
all**. Transactions are called from ad-hoc Rust functions with a `&str` label.
That means CINE-OMNI cannot today offer a command palette, keymaps, macro
recording, a redo panel, or a scripting surface, and the `&str` labels are not a
vocabulary anything can enumerate. This is the clearest actionable gap this
research found on the Blender side (§1.5, G-2).

#### 1.2.3 The undo model

`source/blender/blenkernel/BKE_undo_system.hh` (verified) defines a **pluggable**
undo system: `struct UndoStack { ListBaseT<UndoStep> steps; UndoStep
*step_active; UndoStep *step_active_memfile; UndoStep *step_init; int
group_level; }` and `struct UndoType` with encode/decode callbacks. Registered
types (verified as `extern` declarations): `BKE_UNDOSYS_TYPE_MEMFILE`,
`…_IMAGE`, `…_PAINTCURVE`, `…_PARTICLE`, `…_SCULPT`, `…_TEXT`.

* **`memfile` is global undo**: it snapshots the whole `Main` database into
  memory. The mitigation is ID-granular reuse — `UndoStep::use_old_bmain_data`
  is documented as *"undo/memfile read code is allowed to re-use old data-blocks
  for unchanged IDs, and existing depsgraphs. This has to be forbidden in some
  cases (like renamed IDs)"*. Note the parenthetical: **renaming defeats the
  optimisation, because identity is the name.**
* **Local undo systems** (sculpt, paint, text) exist precisely because a global
  snapshot per brush stroke would be unaffordable. `UndoEncodeHints` and
  `UNDOTYPE_FLAG_ENCODE_PRE_MEMFILE_SUPPORTED` exist to let a local step
  co-exist with a preceding global one.
* **Grouping** is `UndoStack::group_level`: *"Keep track of nested group
  begin/end calls, within which all but the last undo-step is marked for
  skipping."*
* `ID::recalc_up_to_undo_push` / `recalc_after_undo_push` let the depsgraph work
  out *what changed between two undo states* rather than re-evaluating
  everything — verified in the `DNA_ID.h` comment.

**Verdict against CINE-OMNI.** CINE-OMNI's undo is *architecturally superior*: an
append-only op log of invertible deltas, grouped by cause, persisted across
process restarts (Blender's undo dies with the process), with `DeltaKind`
distinguishing Normal/Undo/Redo so the stacks are rebuilt on open. Blender's
system is a workaround for not having invertible ops. **CINE-OMNI should not
copy anything here except two ideas**: (a) the escape hatch of a *pluggable*
undo type for domains where op-granular inversion is the wrong granularity
(a paint stroke on `cine-previs`'s board raster is exactly this case — inverting
ten thousand `SetAttr` ops on a pixel buffer is absurd, and a snapshot step is
correct); and (b) the "skip" flag for steps the user should never land on.

#### 1.2.4 Library linking, overrides and collections

* **Linking** (`struct Library`, verified): a `Library` is itself an ID carrying
  `char filepath[1024]`, plus — newly — `LIBRARY_FLAG_IS_ARCHIVE` with
  `archive_parent_library`, and `PackedFile *packedfile` for embedding a whole
  library inside the host `.blend`. Linked IDs are read-only.
* **Overrides** (`struct IDOverrideLibrary`, verified): the mechanism that makes
  linked data editable without breaking the link. It holds `struct ID *reference`
  (the linked original), `struct ID *hierarchy_root` (*"All liboverrides of a
  same hierarchy (e.g. a character collection) share the same root"*), and a
  `ListBaseT<IDOverrideLibraryProperty> properties` list. Each property carries a
  list of `IDOverrideLibraryPropertyOperation` records: an `operation` and
  `flag`, plus — for RNA collections — `subitem_reference_name` /
  `subitem_local_name` / `subitem_reference_index` / `subitem_local_index` /
  `subitem_reference_id` / `subitem_local_id`, because *"For insertion operations
  in RNA collections, reference may not actually exist in the linked reference
  data. It is used to identify the anchor of the insertion operation."*

  **This is a list-edit / sparse-override system, and it is the same problem USD
  solves with layers, `over` prims and list ops — except Blender solves it with
  string names and array indices, and USD solves it with paths and composition
  arcs.** USD's answer is unambiguously the better one, and it is available to
  CINE-OMNI (Part II). Blender's version is a cautionary tale about
  name-anchored overrides: nearly every field in that struct exists to work
  around the fragility of anchoring an override to a name or an index.

* **Collections** (`struct Collection`, verified): an ID that holds
  `ListBaseT<CollectionObject> gobject` and `ListBaseT<CollectionChild>
  children` — i.e. a **DAG, not a tree**: a collection may appear in multiple
  parents, and objects may be in multiple collections. It also now carries
  `ListBaseT<CollectionExport> exporters` and a `CollectionImport *importer`,
  i.e. **per-collection export configuration is stored in the scene data** — a
  collection knows how to write itself out to USD/Alembic. `owner_id` supports
  *embedded* collections (a scene's master collection).

  CINE-OMNI's `Contains` edge already gives it a DAG hierarchy with the same
  expressive power. The idea worth taking is the **per-collection exporter
  record**: a UCG subtree that carries its own export recipe as attributes, so
  "publish this sequence to USD for the Blender artists" is data, not a CLI
  invocation.

#### 1.2.5 The asset browser and asset system

`source/blender/asset_system` (`AS_asset_library.hh`, verified) defines
`class AssetLibrary` — *"provides access to an asset library's data. The asset
library contains catalogs and storage for asset representations."* — alongside
`AS_asset_catalog.hh` and `AS_asset_representation.hh`. On the data side,
`ID::asset_data` (`AssetMetaData *`) is *"set if the ID is an asset"* and is an
owning pointer.

The design points worth extracting:
* **Any data-block can be an asset** — it is a flag plus metadata on the existing
  ID, not a separate object. Assets are therefore not a parallel universe.
* **Catalogs are a UUID-keyed hierarchy stored in a sidecar file
  (`blender_assets.cats.txt`)**, so an asset's catalogue membership survives
  moving and renaming. Blender reached for UUIDs here specifically because names
  were not good enough — a direct vindication of CINE-OMNI's identity model.
* Asset libraries are directories scanned and indexed; representations are
  loaded lazily with previews.

For CINE-OMNI: `assets/` is currently just a content-addressed directory. The
asset *catalogue* — a UUID-keyed, hierarchical, human-editable taxonomy that is
independent of the filesystem — is the missing half, and it is a small piece of
work with a large payoff for a film-scale project.

#### 1.2.6 Workspaces

A `WorkSpace` is an ID: a saved screen layout plus an active mode plus (per the
depsgraph design document) a render engine and a view layer. The depsgraph design
doc states it explicitly: *"Workspace is a way to organize artists workflow.
Active workspace pointer is stored per window … Workspace itself defines render
engine … Workspace also defines active scene layer"*, and consequently *"it is
window who owns the dependency graph."*

**This is the single subtlest idea in Blender's architecture and CINE-OMNI should
think hard about it.** Blender's evaluated state is *per window*, because
different windows may want the same scene evaluated differently (the doc's
example: *"low-res character in the rest pose in one viewport and hi-res
animation playing back in the other"*). CINE-OMNI today has **one graph per
project behind one `RwLock`**, and `CHATGPT_CONTEXT.md` already flags the related
symptom: *"One daemon per WebSocket client, so multiple browser tabs diverge."*
Blender's answer — one authoritative database, N evaluated views, each owned by
its viewer — is precisely the right shape for CINE-OMNI's shared-daemon fix.

#### 1.2.7 Geometry nodes and modifiers

* **Modifiers** are a linear stack per object; each is described by a
  `ModifierTypeInfo` (verified in `BKE_modifier.hh`) with callbacks for
  evaluation, dependency declaration (`updateDepsgraph`), and UI panel
  registration. The stack is non-destructive: the base mesh is never mutated.
* **Geometry Nodes** is the general-purpose replacement: a node tree evaluated by
  a lazy-function evaluator, operating on a `GeometrySet`
  (`BKE_geometry_set.hh`, verified) — a *container of typed geometry
  components*: `Mesh`, `Curves`, `PointCloud`, `Volume`, `GreasePencil`, plus
  `Instances`. Attributes are accessed through a domain-and-type abstraction
  (`AttributeAccessor`, `MutableAttributeAccessor`, `AttrDomain`,
  `AttributeMetaData`), and data is shared via `ImplicitSharingPtr`
  (copy-on-write refcounted buffers, e.g. `nodes::BundlePtr`).

**Three ideas here are directly transferable and licence-free.**
(i) **A geometry value is a *set of components*, not one type** — this is how you
avoid a combinatorial explosion of node variants. (ii) **Attributes are
first-class, addressed by `(name, domain, type)`** — the same insight as USD
primvars and as CINE-OMNI's own `IndexMap<AttrKey, Value>` bag, but with an
explicit *domain* (point/edge/face/instance) that CINE-OMNI's flat attribute bag
lacks and will want the moment it handles per-frame or per-sample data.
(iii) **`ImplicitSharingPtr` — refcounted immutable buffers with
copy-on-write** — is exactly the right answer to CINE-OMNI's whole-graph
`snapshot()` cost, and in Rust it is simply `Arc<[T]>` + `Arc::make_mut`.

#### 1.2.8 The Python API surface

`bpy` exposes: `bpy.data` (the ID database), `bpy.types` (the full RNA type
hierarchy — every DNA field that RNA describes is reachable), `bpy.ops` (the
operator registry), `bpy.props` (property definitions for registering new types),
`bpy.context`, plus `bmesh`, `mathutils`, `gpu`, `aud`, and `bpy.app.handlers`
(frame-change, render, load/save hooks) and `bpy.msgbus` (RNA property
subscriptions).

The API is *generated from RNA*, which is why it is so complete: Blender did not
write an API, it wrote a reflection layer and got an API. **The lesson for
CINE-OMNI is the same one as the operator registry: build the reflection layer
(a registry of node kinds, attribute keys with types, and operators with typed
property schemas) and the scripting surface, the command palette and the UI
property panels all fall out of it.** `ucg-schema` is already half of this.

Critically for licensing: blender.org states that *"Blender's Python API is an
integral part of the software … The GNU GPL license therefore requires that such
scripts (if published) are being shared under a GPL compliant license."*
**A CINE-OMNI-authored Python bridge script that is distributed with CINE-OMNI
and calls `bpy` must therefore be GPL-licensed.** This is manageable (ship the
bridge script as a separate GPL-licensed file, not as part of the Apache-2.0
crate), but it must be a deliberate decision, not an accident. See §1.4.

#### 1.2.9 Cycles vs EEVEE, the compositor, motion tracking, the VSE, the GPU module

* **Cycles** (`intern/cycles`, **Apache-2.0**): physically based path tracer;
  device backends CPU/CUDA/OptiX/HIP/HIP-RT/Metal/oneAPI (verified in
  `device/device.h`); has a **Hydra render delegate** (`intern/cycles/hydra`), so
  it can be driven by any USD/Hydra host. **This is the one piece of Blender that
  CINE-OMNI could legally link.** Whether it *should* is a different question:
  Cycles brings CMake, its own device abstraction, and CUDA/OptiX/HIP/Metal
  toolchain requirements, none of which match a `wgpu` + `cargo` build. The
  realistic use is **TYPE C: Cycles-as-a-service behind a Hydra or CLI
  interface**, not TYPE B.
* **EEVEE**: rasterisation engine inside `source/blender/draw` — GPL, and tied to
  Blender's GPU module (OpenGL/Metal/Vulkan). Not reusable; and CINE-OMNI's
  `cine-gpu` (wgpu/WGSL) is the right substrate anyway.
* **Compositor** (`source/blender/compositor`, GPL-2.0-or-later, verified): now a
  GPU-first evaluator. `COM_context.hh` includes `GPU_shader.hh` and
  `COM_static_cache_manager.hh`, and declares `SideEffectOutputTypes { ViewerNode,
  FileOutputNode, NodePreviews }` — i.e. the compositor models "outputs that are
  side effects" explicitly, and has a *domain* concept (`COM_domain.hh`) plus
  `COM_result.hh` and `COM_meta_data.hh`. **The `Domain` concept — every
  intermediate result carries its own resolution and transform, and operators
  realise operands onto a common domain only when they must — is a genuinely
  good idea that `cine-comp` should adopt.** It is how you avoid resampling a
  4K plate to run a colour correction on a 256×256 matte.
* **Motion tracking** (`intern/libmv`, **MIT**, verified in file headers):
  the 2D tracker plus the `simple_pipeline` bundle-adjustment / camera-solve
  library (built on Ceres, BSD-3). **This is legally incorporable into
  Apache-2.0 CINE-OMNI**, and camera solving is a genuine gap in
  `cine-previs`/`cine-comp` — CINE-OMNI has physically accurate camera *optics*
  but no way to *solve* a camera from a plate.
* **VSE** (`source/blender/sequencer`, GPL, verified): note that
  `SEQ_sequencer.hh` includes `DNA_session_uid_types.h` — the sequencer, like the
  rest of Blender, had to reach for session UIDs because strip names were not a
  stable identity. `cine-timeline` is already ahead here.
* **GPU module** (`source/blender/gpu`, GPL, verified): a hand-rolled backend
  abstraction over OpenGL, Metal and Vulkan — precisely the job `wgpu` does for
  CINE-OMNI, with the added benefits of WebGPU/browser support and Rust safety.
  **Nothing to take; the choice of `wgpu` is validated by Blender having had to
  build this by hand.**

### 1.3 Deep dive II — Blender's depsgraph vs CINE-OMNI's UCG reactor cascade

This is the comparison the directive asks for, and it is the most technically
substantive part of this dossier.

#### 1.3.1 What Blender's depsgraph actually is

Verified from `source/blender/depsgraph/DEG_depsgraph.hh` and
`intern/node/deg_node.hh`, plus the design document at
`https://developer.blender.org/docs/features/core/depsgraph/`.

**Purpose** (verbatim from the header): *"The dependency graph tracks relations
between various pieces of data in a Blender file … It is used to determine the
set of operations need to ensure that all data has been correctly evaluated in
response to changes, based on dependencies and visibility of affected data."*

**Three-level node structure** (`enum class NodeClass`, verified):

```
GENERIC     graph plumbing (TIMESOURCE, ID_REF)
COMPONENT   [Outer node] "an 'aspect' of evaluating/updating an ID-Block,
            requiring certain types of evaluation behavior"
OPERATION   [Inner node] "a glorified function-pointer/callback for scheduling
            up evaluation operations for components"
```

`enum class NodeType` (verified, abridged): `PARAMETERS`, `ANIMATION`,
`TRANSFORM`, `GEOMETRY`, `SEQUENCER`, `COMPOSITOR`, `LAYER_COLLECTIONS`,
**`COPY_ON_EVAL`** (*"Entry component of majority of ID nodes: prepares evaluated
pointers for execution"*), `OBJECT_FROM_LAYER`, `HIERARCHY`, `AUDIO`, `ARMATURE`,
`GENERIC_DATABLOCK`, `SCENE`, **`VISIBILITY`**, `EVAL_POSE`, `BONE`,
`PARTICLE_SYSTEM`, `SHADING`, `POINT_CACHE`, `IMAGE_ANIMATION`, `BATCH_CACHE`,
`INSTANCING`, and a synchronisation-back-to-original node type.

So the shape is: **ID node → component nodes → operation nodes**, and *relations
are drawn between operation nodes*, not between IDs. A parent-child relation is
not "object A depends on object B"; it is "A's `TRANSFORM/local` operation
depends on B's `TRANSFORM/final` operation". This is the **granularity** point,
and it is the single most important thing CINE-OMNI can learn (§1.3.3).

**Copy-on-eval.** *"Dependency graph always performs evaluation on a copy of
original data. This is the only way to support data to be in different states at
the same time and the way to solve threading conflicts between render and
viewport threads."* And: *"Render engines are working with generated data
provided by dependency graph and are never touching original DNA."* The copy is
made shallow at build time and filled in from evaluation threads.

**Ownership.** *"it is window who owns the dependency graph (for the interactive
operations) … For the final F12 render … dependency graph is owned by the Render
structure."* Multiple depsgraphs coexist over one `Main`.

**Scope.** Explicitly limited: *"the dependency graph is responsible for
f-curves evaluation, but not for edge subdivide operation in mesh edit mode … It
is not responsible for one-time changes."* The depsgraph handles *derived*
state; direct authoring is not routed through it.

**Cycle handling — the decisive difference.** Verified in
`source/blender/depsgraph/intern/builder/deg_builder_cycle.cc`: an iterative DFS
with `NODE_NOT_VISITED / NODE_VISITED / NODE_IN_STACK` colouring. On finding a
back-edge it builds a human-readable cycle description, then:

```c
CLOG_WARN(&LOG, "Dependency cycle detected:\n%s", cycle_str.c_str());
Relation *sacrificial_relation = select_relation_to_murder(rel, entry);
sacrificial_relation->flag |= RELATION_FLAG_CYCLIC;
++state->num_cycles;
```

**Blender detects a cycle, logs a warning, picks an edge to "murder", and
proceeds with a silently degraded graph.** The user's edit is never rejected;
some dependency simply stops being honoured, and the scene evaluates in a subtly
wrong order.

#### 1.3.2 Side-by-side

| Dimension | Blender depsgraph | CINE-OMNI UCG |
|---|---|---|
| Graph node granularity | Operation-level (ID → component → operation) | Entity-level (`Node` with an attribute bag) |
| Relations between | Operation nodes | Entities, via `DependsOn` / `DerivedFrom` / `SyncedTo` |
| Who authors the graph | Builders walk the scene and *derive* the relations | Modules author edges explicitly inside transactions |
| Rebuild policy | Rebuilt wholesale on structural change; tagged+flushed on value change | Incrementally maintained; edges are ops |
| Cycles | Detected, warned, one edge disabled, evaluation continues | `would_cycle()` → `UcgError::Cycle` → **whole transaction rolls back** |
| Evaluation target | A *copy* (`COPY_ON_EVAL`), original never touched | Reactors read a snapshot; writes are new transactions on the authoritative graph |
| Number of graphs | One per (window × workspace × view layer) + one per render | One per project |
| Scope | Derived/animated state only; not one-time edits | **All** state — authoring and derivation both go through transactions |
| Undo relationship | Separate subsystem; depsgraph re-derives after an undo using `recalc_*_undo_push` | The same op log *is* the undo; no re-derivation needed |
| Termination guarantee | Acyclic-after-murder + a single evaluation pass | Idempotent writers + deterministic derived IDs + `max_cascade_depth` (8) |
| Cross-domain propagation | Within one scene database | Across script/previs/logistics/timeline/comp/colour/audio |
| Provenance | None (the depsgraph is transient) | `cause`, `depth`, `(actor, lamport)` on every delta |

#### 1.3.3 What CINE-OMNI should actually learn — five findings

**(1) Sub-entity granularity is the scaling answer, and CINE-OMNI does not have
it.** Blender's decomposition into components means that changing an object's
*material* does not invalidate its *transform*, and changing its transform does
not re-run its *geometry* modifiers. CINE-OMNI's dirty set is computed at
**node** granularity: `attrs::TEXT` changing on a `script.dialogue` marks the
whole `script.scene` dirty, which marks the whole `previs.scene` dirty, which
wakes the logistics reactor, which reschedules *the entire stripboard*. The
7-reactor cascade in `cine-pipeline/src/reactors.rs` is doing far more work than
the change requires, and the `< 5 ms` reactor-round budget in
`docs/ARCHITECTURE.md` will not survive a 3,000-node feature-length project at
node granularity.

The Blender-shaped fix is **not** to redesign the UCG. It is to let a
`DependsOn`/`DerivedFrom` edge carry an optional *attribute-key mask* in its
`meta`, and to have the dirty-set walk consult it: an edge annotated
`{"on": ["script.duration"]}` does not propagate a change to `attrs::TEXT`.
That is Blender's component idea expressed within CINE-OMNI's existing edge
model, and it is a contained change to `Graph::dependents` plus the reactor
`wakes_on` filters.

**(2) Rejecting cycles is right; Blender's approach is a warning, not a model.**
`select_relation_to_murder` is an admission of defeat, and it produces the worst
possible failure mode: no error, a log line the artist never sees, and a scene
that evaluates in the wrong order. CINE-OMNI's `UcgError::Cycle` with full
transaction rollback is **strictly better** and should not be softened.

The one thing to take from Blender: **the diagnostic quality.** Blender builds a
full human-readable chain — *"X depends on Y via 'relation name'"* — for every
link in the cycle. CINE-OMNI's `would_cycle()` returns a bare `UcgError::Cycle`.
It should return the path, with edge kinds and node kinds, so a reactor author
can see *which* reactor closed the loop. This is cheap (the DFS already has the
stack) and will pay for itself the first time someone writes reactor #8.

**(3) Separate the authoritative graph from the evaluated view — per viewer.**
Blender's *"it is window who owns the dependency graph"* is the fix for the
already-known CINE-OMNI defect *"One daemon per WebSocket client, so multiple
browser tabs diverge."* The correct shape is: **one shared daemon owning one
authoritative `Engine`**, and **N per-client evaluated views** derived from it,
each with its own dirty set and its own frame/quality context. That is Blender's
architecture exactly, and it also solves the "different tabs want different
playhead positions / preview quality" problem that will otherwise be solved
badly later.

**(4) Copy-on-eval beats whole-graph snapshot.** `engine.snapshot()` returning an
owned clone is O(graph) per heavy evaluation. Blender allocates a *shallow* copy
per ID at graph-build time and fills contents from evaluation threads, and shares
unchanged buffers by reference (`ImplicitSharingPtr`). In Rust the equivalent is
free: make `Value::Bytes` and the large list variants `Arc`-backed, so a snapshot
clones an index and refcounts, not payloads. This directly serves the stated
`cine-comp` budget of *"1080p RGBA f32 < 8 ms per simple node"* — a whole-graph
clone that copies frame buffers cannot meet it.

**(5) The depsgraph is *derived*; the UCG is *authored*. Do not lose that.**
Blender's builders re-derive the entire relation set from the scene on every
structural change, which is why the depsgraph is disposable and why it can afford
to murder edges. CINE-OMNI's edges are *authored ops* with provenance, undo and
persistence. This is a real advantage — CINE-OMNI can answer "why does this
depend on that, who created the edge, and in which transaction", which Blender
cannot — and it is the reason CINE-OMNI can span seven domains where the
depsgraph spans one scene. **The trade is that CINE-OMNI's edges can be wrong and
stay wrong.** Blender's `validate()` equivalent is "rebuild from scratch";
CINE-OMNI's `Graph::validate() -> Vec<String>` needs to be run somewhere real —
at load, after cascade quiescence in debug builds, and in the reactor test
harness — or authored-edge rot will accumulate.

### 1.4 What a TYPE C / TYPE D Blender integration actually looks like

Four concrete options, with their real costs.

#### Option 1 — Batch: `blender --background --python bridge.py`

```
cine-bridge-blender  ──spawn──▶  blender --background \
                                   --factory-startup \
                                   --python /path/to/bridge.py \
                                   -- --in scene.usd --out render/ --job job.json
```

* **Boundary:** a process spawn per job. No shared memory, no shared GPU context.
* **Marshalling:** a USD layer in, files (renders, caches, USD) out; a JSON job
  descriptor on `argv` after `--`; results reported on stdout as JSON lines and
  ingested as a UCG transaction.
* **Cost:** Blender's startup is seconds, not milliseconds, and `bpy` loads the
  whole application. Fine for "render this shot", hopeless for "the artist
  dragged a slider".
* **GPL boundary:** clean *if* `bridge.py` is shipped as an independently
  GPL-licensed file (blender.org: scripts using `bpy`, if published, must be
  GPL-compatible) and CINE-OMNI communicates only over `argv`/stdout/files.
  Spawning a separate GPL program and exchanging data with it is the classic
  arm's-length arrangement.
* **Verdict:** the correct default. Low risk, low value.

#### Option 2 — Persistent process over a socket

A long-lived `blender --background --python server.py` where `server.py` opens a
Unix socket / named pipe and services requests (`import_usd`, `set_frame`,
`render_region`, `export_usd`, `bake_cache`).

* **Boundary:** one process for the session; amortises the startup cost.
* **Marshalling:** JSON-RPC (which CINE-OMNI already speaks — `cine-cli` has a
  JSON-RPC daemon), with bulk data going through USD layers or raw framebuffers
  on disk / shared memory, never through the RPC channel.
* **Cost — the real ones:**
  * **The GIL and Blender's main loop.** `bpy` is not thread-safe and must be
    driven from Blender's main thread. A socket server inside Blender therefore
    has to pump requests from a timer or a modal operator. Long operations block
    the server.
  * **Version pinning.** `bpy` breaks between Blender releases. The tag series
    verified above (`4.5.x` → `5.0` → `5.1` → `5.2`) means a bridge script must
    declare a supported range and be tested against each. The PyPI `bpy` package
    additionally pins the interpreter: version 5.2.1 declares
    `requires_python == 3.13.*`.
  * **Crash blast radius.** A Blender crash takes the session's evaluated state
    with it; the bridge must be able to rebuild from the UCG on reconnect. That
    is a *feature* of CINE-OMNI's design (the UCG is authoritative) but it must
    be implemented, not assumed.
* **GPL boundary:** the same as Option 1 — a socket is an arm's-length interface.
  Do **not** be tempted to make CINE-OMNI's Rust process load `libbpy`; that is
  linking, and it is not permitted.
* **Verdict:** the right shape for a working artist bridge, but only build it
  once there is a concrete workflow demanding it.

#### Option 3 — `bpy` as a Python module in a sidecar

PyPI publishes **`bpy` version 5.2.1**, summary *"Blender as a Python module"*,
author *"Blender Foundation"*, licence field **`GPL-3.0`**, `requires_python
== 3.13.*` (verified via `https://pypi.org/pypi/bpy/json`).

This is architecturally the same as Option 2 but embeds Blender in a Python
process CINE-OMNI controls, rather than embedding Python in a Blender process.
It is more convenient (`pip install bpy`, no Blender installation to locate) and
it makes the licence question sharper, not softer: **the PyPI package declares
`GPL-3.0`**, so the sidecar process is a GPLv3 program. Keep it a *separate
process and a separate distribution unit*.

#### Option 4 — Pure file interchange (TYPE E)

CINE-OMNI writes USD; the artist opens it in Blender; the artist exports USD;
CINE-OMNI re-imports. No process control at all.

* **Cost:** zero engineering beyond USD fidelity, zero licence exposure, zero
  version pinning.
* **The catch — and it is the important one:** **identity does not survive unless
  CINE-OMNI makes it survive.** Blender's ID identity is a *name*, so a round
  trip through Blender will rename things. CINE-OMNI must write its `EntityId`
  into USD prim custom metadata on export and read it back on import, exactly as
  `cine-timeline` already does with the `ucg_id` key in OTIO metadata. Blender's
  USD importer supports this: `PropertyImportMode { None, User, All }` in
  `io/usd/usd.hh` governs *"importing of custom attributes / properties outside a
  prim's regular schema"* — so custom metadata can be preserved. **This must be
  verified by test before it is relied on; it has not been tested here.**

#### Recommended posture

**TYPE D (bridge), realised first as TYPE E (file interchange over USD).** Build
`cine-bridge-blender` as an *optional* crate that (a) exports/imports USD with
`ucg_id` custom metadata as the identity carrier, and (b) can optionally spawn a
user-supplied Blender for batch jobs. Do not ship Blender. Do not link Blender.
Do not put `bpy` on the critical path of any interactive operation.

### 1.5 TYPE G — Blender ideas CINE-OMNI should reimplement natively

These are *architectural*, not code, and are therefore free of the GPL boundary.
Ranked by value.

| # | Idea | Why it matters to CINE-OMNI | Effort |
|---|---|---|---|
| **G-1** | **Sub-entity dependency granularity** (component nodes → attribute-masked edges) | The 7-reactor cascade currently over-propagates at node granularity; the `<5 ms` budget will not hold at film scale | Medium |
| **G-2** | **An operator registry with typed property schemas** (`wmOperatorType` + RNA) | CINE-OMNI has transactions but no *command vocabulary*: no palette, no keymaps, no macro recording, no redo panel, no scripting surface. `ucg-schema` is half the reflection layer already | Medium-high, highest payoff |
| **G-3** | **One authoritative database, N per-viewer evaluated views** ("the window owns the depsgraph") | Directly fixes the known "one daemon per WebSocket client, tabs diverge" defect, and pre-solves per-viewer playhead/quality | Medium |
| **G-4** | **Implicit sharing / copy-on-write buffers** (`ImplicitSharingPtr` → `Arc<[T]>`) | `engine.snapshot()`'s whole-graph clone is the first thing that will break the comp/colour frame budgets | Low (Rust makes it easy) |
| **G-5** | **Rich cycle diagnostics** (the "X depends on Y via 'name'" chain) | `UcgError::Cycle` is currently unactionable; the DFS already has the path | Low |
| **G-6** | **The compositor `Domain` concept** — every result carries its own resolution/transform, realised onto a common domain only when required | Stops `cine-comp` resampling a 4K plate to run a 256×256 matte operation | Medium |
| **G-7** | **A UUID-keyed asset catalogue independent of the filesystem** (`blender_assets.cats.txt`) | `assets/` is content-addressed storage with no taxonomy; a film needs one | Low |
| **G-8** | **Pluggable undo types** for domains where op-granular inversion is wrong (raster paint on `cine-previs`'s board) | Inverting 10⁴ pixel ops is absurd; a snapshot step is correct there | Low-medium |
| **G-9** | **Geometry as a *set of typed components* with `(name, domain, type)` attributes** | CINE-OMNI's flat attribute bag has no *domain*; it will need one for per-sample/per-frame data | Medium |
| **G-10** | **Per-collection export recipes** (`Collection::exporters`) | Makes "publish this sequence to USD" data in the UCG rather than a CLI incantation | Low |
| **G-11** | **A dependency-closure content hash** (`ID::deep_hash`) | Free dedup of derived assets and a cheap "did anything really change" gate for reactors | Low-medium |
| **G-12** | **Refcount/reachability-based garbage collection with pinning** (`ID::us` + fake user) | The UCG has no orphan story; `Project::compact()` should sweep unreachable, unpinned nodes | Low |

Two things Blender does that CINE-OMNI should **not** copy: **name-based
identity** (Blender is still paying for it — `session_uid`,
`library_weak_reference`, `subitem_reference_id`, `UndoRefID`), and **cycle
tolerance** (`select_relation_to_murder`).

**Legally incorporable Blender code**, to be clear one final time:
`intern/cycles` (Apache-2.0) and `intern/libmv` (MIT) only. **libmv is the
sleeper here** — MIT-licensed camera solving and 2D tracking is exactly the gap
between `cine-previs`'s physically accurate optics and real plate photography,
and there is no licence obstacle at all.

### 1.6 Classification

> **Blender — TYPE D (BRIDGE), with TYPE G as the primary value, and a narrow
> TYPE B carve-out for `intern/libmv` (MIT) and `intern/cycles` (Apache-2.0).**
>
> **Licence:** GPL-2.0-or-later per file and GPLv3 as a whole (verified in
> `COPYING`, `README.md` and blender.org) makes linking or copying
> `source/blender/` into Apache-2.0 CINE-OMNI legally impossible; the Python API
> is inside that boundary too. **Architecture fit:** Blender's ID system is
> name-scoped where CINE-OMNI's is UUID-scoped, its depsgraph tolerates cycles
> where the UCG rejects them, and it owns a window, a GPU context and a Python
> runtime — so it must stay in its own process. **Performance:** process-boundary
> marshalling via USD is acceptable for batch work and unacceptable for
> interactive work, which is the reason to bridge rather than integrate.
> **Maintenance:** a `bpy` bridge is version-pinned to a moving target
> (`4.5.x → 5.2.x` in roughly a year, with `bpy` 5.2.1 pinning Python to
> `3.13.*`), so keep the bridge optional and thin; the durable value is the
> dozen architectural ideas in §1.5, which cost nothing to adopt and carry no
> licence risk.

---
