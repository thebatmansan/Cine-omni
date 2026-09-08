# CINE-OMNI Industry Blueprint — 12: Professional Compositing

**Status: complete.** Every licence claim below was fetched from the project's
own licence file on 2026-09-08 and is quoted. Claims that could not be verified
carry an inline `UNVERIFIED` marker and appear in the register in Part I.
Nothing here is filled in plausibly to look complete.

**Method.** Everything in Part D (the assessment of `crates/cine-comp`) was
measured or read from the working tree on 2026-09-08 and is reproducible with
the commands quoted. Everything about external projects is either quoted from a
primary source with a URL, or marked `UNVERIFIED`.

**Scope boundaries — what this document does *not* re-cover:**

| Already covered | Where |
|---|---|
| OpenColorIO licence, dependency tree, and the WGSL problem | [`10-color-and-sync.md`](10-color-and-sync.md) §A.5 |
| ACES licensing (now Apache-2.0), ACES 2 output transforms | `10-color-and-sync.md` §A.6 |
| Premultiplication *at the decode boundary*, alpha conventions per codec | `10-color-and-sync.md` §A.4 |
| wgpu runtime, frame graph, present modes, GPU memory pressure | [`11-shell-and-gpu-runtime.md`](11-shell-and-gpu-runtime.md) Part B |
| Blender's depsgraph vs the UCG reactor cascade | [`01-3d-dcc.md`](01-3d-dcc.md) §1.3 |
| Blender's GPL status and the TYPE C/D/G options | `01-3d-dcc.md` §1.4–1.6, `01-LICENSING-SPINE.md` §3 |
| FFmpeg LGPL/GPL split | `01-LICENSING-SPINE.md` §4 |

This document covers the **comp-specific** layer above those: the render
dependency graph, the image substrate, deep data, cryptomatte, and the
correctness rules that a compositor must not get wrong.

---

## 0. Summary of findings

**The seven that change decisions.**

**F1 — `cine-comp` is a well-shaped node-graph toy with no image substrate.**
It has 28 node kinds, a typed port/parameter model, cycle detection, JSON
round-trip and UCG persistence — all of which are real and worth keeping. It
has **no file I/O of any kind**: `Read(File)` returns an image previously
registered in memory by the caller, and `Write` records an `Arc<Image>` against
a path string without writing a byte. `grep -rin 'openexr\|oiio' crates/`
returns only comments and `.exr` filename strings. A compositor that cannot
open a plate is not a compositor; it is a graph engine with pixel functions
attached. **This is the single largest gap and it is a *dependency* decision,
not an implementation one** (§E.4, H.R5).

**F2 — the region-of-interest system does not reduce work, and is measurably
counter-productive.** `Evaluator::evaluate_roi` is documented as lazy. Measured
on a `Read(2048×2048) → Blur(size 4)` chain, requesting a 64×64 region returned
a buffer of **16,777,216 floats — the full frame** — and took 1369 ms against
1659 ms for the full-frame request. The ROI is threaded up the pull chain and
then ignored by every allocation site, because `Image` has no data window: the
crate's own module docs admit *"Buffers are always allocated at full format size
(there is no data-window support)"*. The ROI does, however, enter the cache key,
so it fragments the cache while saving nothing. **A bounding box / data window
on `Image` is a prerequisite for every performance property a comp engine
needs**, and it is a type-level change that gets harder every week (§F.2, H.R1).

**F3 — a measured correctness bug: colour-correcting an additive element
destroys it.** `ops::color::color_correct` unpremultiplies, grades, then
re-premultiplies by the *original* alpha. For an emissive/additive fragment
(`RGB > 0`, `A = 0` — a glow, a light wrap, an additive pass), the final
multiply by zero annihilates it. Measured:

```
in=[1.0, 0.8, 0.6, 0.0]  -> out=[0.0, 0.0, 0.0, 0.0]     # destroyed
in=[1.0, 0.8, 0.6, 0.02] -> out=[2.0, 1.6, 1.2, 0.02]    # correct
in=[0.5, 0.4, 0.3, 1.0]  -> out=[1.0, 0.8, 0.6, 1.0]     # correct
```

This is precisely the rule `10-color-and-sync.md` §A.4.1 already states —
*"Premultiplied values can legitimately exceed alpha … Do not clamp"* — violated
one layer up. The fix is not to remove unpremult; it is to make the
unpremult/premult pair a *reversible* transform guarded on alpha, or to expose
the grade's alpha handling as a parameter the way Nuke's `(un)premult` knob does
(§E.1 R-P3, H.R3).

**F4 — `Image::resize` is bilinear at every scale factor, which aliases on any
minification beyond 2×.** Measured: 512→64 (8×) reduction of a period-3 stripe
pattern, against an exact 8×8 box reference:

```
bilinear row: [0.500, 0.500, 0.000, 0.500, 0.500, 0.000, ...]
box  ref row: [0.375, 0.375, 0.250, 0.375, 0.375, 0.250, ...]
max abs diff: 0.2500
```

Bilinear reads a 2×2 neighbourhood regardless of the reduction ratio, so 62 of
every 64 source pixels are discarded. Proxy generation, `Reformat`, and any
scale-down `Transform` are all affected. A filtered/mip-aware resampler is a
contained fix (§E.2, H.R3).

**F5 — the graph's cache-key computation is superlinear and runs on every
evaluation.** `Evaluator::evaluate_output` calls `CompGraph::hashes()`
unconditionally, which topologically sorts the whole graph (building a fresh
`petgraph` each time) and then, per node, calls `NodeKind::describe()` — which
**allocates a fresh `NodeDesc` with `String`s and `Vec`s on every call**.
Measured per-call cost of `hashes()`:

| Nodes | `hashes()` per call |
|---|---|
| 11 | 77.8 µs |
| 101 | 454.9 µs |
| 501 | 4 231.7 µs |
| 2 001 | **53 224.5 µs (53 ms)** |

`NodeKind::describe()` alone measures **8.2 µs per call**. A 2 000-node comp
therefore burns 53 ms deciding *whether* it needs to do work, before any pixel
is touched. This mirrors the `Graph::of_kind()` finding in
[`00-REPOSITORY-ARCHAEOLOGY.md`](../00-REPOSITORY-ARCHAEOLOGY.md) — the same
class of defect (recomputing a derived index inside a hot loop), found the same
way (by measurement). Fixes: make `describe()` return a `&'static NodeDesc` from
a `OnceLock` table, and cache `hashes()` against a graph revision counter (§F.3, H.R4).

**F6 — the evaluator's cache is unbounded, unshared and un-evictable.**
`Evaluator` holds `Mutex<HashMap<(NodeId, frame, hash), Output>>` with a
`clear_cache()` and a per-node `invalidate()` and nothing else. Measured: 240
frames of a 2-node 256×256 graph leaves **480 live entries ≈ 480 MiB** of
`Arc<Image>` at 4K equivalent. At real plate sizes one RGBA f32 frame is:

| Format | RGBA f32 |
|---|---|
| 1920×1080 | 31.6 MiB |
| 3840×2160 | 126.6 MiB |
| 4096×3112 (4K full aperture) | 194.5 MiB |
| 8192×4320 | 540.0 MiB |

Four cached nodes at 4K full aperture is 778 MiB. **A memory budget with an
eviction policy is not an optimisation, it is the difference between running and
being OOM-killed.** This is the same conclusion `11-shell-and-gpu-runtime.md`
§B.5 reached for GPU memory, and it needs the same treatment on the CPU side
(§F.3, H.R4).

**F7 — CINE-OMNI already has two dependency systems, and conflating them would
be the expensive mistake.** The UCG reactor cascade is *authored, persistent,
transactional, undoable, cross-domain and coarse* (entity granularity). A comp
render graph is *derived, ephemeral, per-frame, per-region and fine* (pixel
granularity). Today `cine-pipeline`'s `comp` reactor correctly does the coarse
thing — it calls `cine_comp::ucg::mark_dirty` when a shot or previs camera
changes — and `cine-comp::eval` does the fine thing with content hashes. **That
split is right and should be made explicit and permanent.** What must *not*
happen is a comp evaluation running inside a reactor, or per-frame render state
being written into the UCG as ops (§B.4).

**Reading order for a decision-maker:** Part G is the classification table,
Part H is the recommendations, Part I is what remains unverified. Parts B–F are
the evidence those rest on.

---

# PART B — THE RENDER DEPENDENCY GRAPH

## B.1 What a comp render graph has to be

A compositing DAG is not a dataflow graph that runs. It is a **query engine**.
The user asks for one thing — *"pixels of node `X`, at frame `f`, in region `R`,
at proxy level `p`, for channels `C`"* — and the engine must answer it by doing
the least work that produces that answer, reusing everything it computed for the
previous question. Every architectural property below falls out of that one
requirement.

Five axes, and CINE-OMNI's position on each:

| Axis | What it means | `cine-comp` today |
|---|---|---|
| **Pull, not push** | Evaluation starts at the requested node and recurses upstream; nothing runs unless something downstream needs it | **Has it.** `Evaluator::eval_node` recurses on `graph.inputs(id)` |
| **Content-addressed invalidation** | A node's identity is a hash of its parameters plus its inputs' hashes, so an untouched branch keeps hitting the cache | **Has it.** `CompGraph::hashes()`, verified by `hashes_change_downstream_only` |
| **Two-phase: metadata then pixels** | A cheap pass computes format, bounding box and channel set for the whole graph; only then are pixels requested | **Does not have it.** There is no metadata pass at all |
| **Region propagation** | The requested output region is transformed backwards through each node into the input region it actually needs | **Has the plumbing, not the effect** — see F2 |
| **Bounded cache** | Results are held against a memory budget with an eviction policy | **Does not have it** — see F6 |

The third row is the one that matters most and is least obvious, so it gets its
own section.

## B.2 The two-phase model, and why a comp engine cannot skip it

Nuke's plug-in API (the NDK) splits an operator into distinct virtual methods
that the host calls in a fixed order — conventionally `validate()` (declare the
output format, bounding box and channels), `_request()` (declare which input
regions and channels are needed), and `engine()`/`_open()` (produce pixels for a
row or region). *The exact method names, signatures and ordering are quoted from
memory of the NDK and are* **UNVERIFIED** *— they must be confirmed against
<https://learn.foundry.com/nuke/developers/latest/ndkdevguide/> before this
document is used to justify an API shape.* The **architectural idea** is not in
doubt and is independently observable in the product's behaviour: Nuke can tell
you the bounding box and channel set of a 200-node script instantly, without
rendering anything, and the viewer only renders the visible region.

Why the split is load-bearing, in four consequences:

**(1) Bounding box / data window is *computed*, not assumed.** A `Blur(size 40)`
grows its bbox by the kernel radius. A `Transform` rotates it. A `Merge` unions
its inputs'. A `Crop` intersects. Downstream of a `Roto` covering 5 % of frame,
95 % of the plate is *known to be transparent black* and must never be
allocated, filtered, or written. Without a metadata pass this is unknowable, so
every node processes the full format — which is exactly what `cine-comp` does
and exactly why F2 measures no saving.

**(2) The channel set is *computed*, not assumed.** A 40-AOV multi-part EXR read
by a graph whose output only uses `rgba` should decode `rgba`. A `Shuffle`
pulling `CryptoObject00.R` should cause exactly that channel to be read. This is
a decode-time saving of an order of magnitude on real VFX plates, and it is only
available if the graph can answer "which channels does the output depend on"
before reading.

**(3) It makes the *cost* of a graph inspectable before it is paid.** A UI that
can say "this render will touch 9 GB" without rendering is a different product
from one that discovers it by being killed.

**(4) It is the only place format negotiation can happen.** `merge()` in
`cine-comp` today resolves a size mismatch by `a.crop(0, 0, b.width, b.height)`
— cropping the foreground to the background's size **anchored at the origin**.
A foreground larger than the background is silently clipped, and there is no
concept of the two images living in a shared coordinate space with different
data windows. That is a format-negotiation decision being made, badly, at pixel
time because there is nowhere else to make it.

**Recommendation:** add a `Meta` phase to `cine-comp` before adding any more
node kinds. Concretely, `CompNode` grows one method —
`fn meta(&self, inputs: &[Meta]) -> Meta` where
`Meta { format: Box2i, bbox: Box2i, channels: ChannelSet, is_deep: bool }` —
and `Evaluator` grows a `meta(graph, node)` pass that runs the whole graph in
topological order and caches per `(node, graph_revision)`. This is roughly 28
small functions (one per `NodeKind`) plus about 100 lines of evaluator, and
**every other performance item in this document depends on it.**

## B.3 Fusion, and the one idea worth taking from it

Blackmagic Fusion is proprietary; there is no source and no plug-in-level
architecture document comparable to the NDK, so claims about its internals must
come from its user documentation and observable behaviour. **UNVERIFIED —
Fusion's internal caching and DoD implementation are not documented at a level
that supports architectural claims.** What is publicly documented in the Fusion
manual and directly observable is the **Domain of Definition (DoD)** and
**Region of Interest (RoI)** pair: DoD is the "where does this image have data"
rectangle computed forward through the graph (the same concept as Nuke's bbox);
RoI is the "what does the viewer need" rectangle propagated backward.

The idea worth taking is that these are **two different rectangles travelling in
opposite directions**, and the work a node does is their *intersection*:

```
                     bbox / DoD  ───────────────────▶  (forward, in meta pass)
    [Read] ── [Blur] ── [Transform] ── [Merge] ── [Viewer]
                     ◀───────────────────  RoI       (backward, in request pass)

    work(node) = bbox(node) ∩ roi(node)
```

`cine-comp` propagates only the RoI, has no DoD, and therefore cannot compute
the intersection — which is the actual saving. **This is the same finding as F2
stated structurally: the ROI plumbing is half a mechanism.**

## B.4 The distinction that must be preserved: UCG cascade vs comp render graph

CINE-OMNI now has two dependency systems. They look similar and are not, and the
temptation to unify them should be refused explicitly and in writing.

| | **UCG reactor cascade** (`ucg-core` + `cine-pipeline`) | **Comp render graph** (`cine-comp::eval`) |
|---|---|---|
| What it propagates | *Facts* — a shot exists, a camera moved, a scene is 3 pages | *Pixels* |
| Granularity | Entity (`Node` + attribute bag) | Node × frame × region × channel |
| Lifetime | Persistent; the op log is the durable record | Ephemeral; recomputable from the graph |
| Transactional | Yes — `Tx`, rollback, `Delta`, undo | No, and must never be |
| Provenance | `cause`, `depth`, `(actor, lamport)` on every delta | None needed; the hash *is* the identity |
| Cycles | Rejected: `UcgError::Cycle` rolls the transaction back | Rejected: `Error::Cycle` from `toposort` |
| Invalidation | Dirty-set walk over `DependsOn` / `DerivedFrom` edges | Content hash over parameters + upstream hashes |
| Cost of being wrong | Corrupt project state | A stale frame; drop the cache |
| Frequency | Per user action (~1 Hz) | Per frame, per viewer, per pan (~100 Hz) |
| Budget | `< 5 ms` per reactor round (`docs/ARCHITECTURE.md`) | sub-frame at 24 fps for a viewer |

**The boundary between them is exactly one attribute.** `cine-comp::ucg` stores
the graph structure (`kind`, `params`, `links`, `bypass`) in the UCG, and the
pipeline `comp` reactor marks the comp entity dirty when its shot or previs
camera changes (`cine_comp::ucg::mark_dirty`). The evaluator then re-derives
everything from the graph. **That is the correct architecture and it is already
what the code does.** Three rules make it permanent:

1. **No reactor may evaluate pixels.** A reactor returns `Plan`s; a `Plan` runs
   inside a write transaction; evaluating a 4K comp inside a write transaction
   would hold the `Engine`'s write lock for seconds. The existing
   `WriteLockTimeout` would fire and *report a deadlock that is not one*.
2. **No per-frame render state goes into the UCG.** Cache entries, timings,
   region requests and rendered frames are not project state. They have no undo
   semantics and would flood the op log. The *outputs* of a render (a written
   EXR sequence on disk) are project state; the render itself is not.
3. **The comp graph's own DAG stays in `CompGraph`, not in UCG edges.** The
   temptation is to make each comp node a UCG entity with `DependsOn` edges and
   let the cascade drive it. That would give a 200-node comp 200 entities and
   ~250 edges *per shot*, on a graph whose measured weakness is already
   cascade-width (`00-REPOSITORY-ARCHAEOLOGY.md`: 144 ms for one dialogue edit
   at 1 000 scenes). It would also put a pixel-rate dependency system behind a
   non-reentrant `RwLock`. `cine-comp::ucg` already stores the links as a single
   `links` attribute on the graph entity, which is the right call.

**What the comp graph should borrow from the UCG:** the *content-hash*
invalidation discipline and the *idempotent writer* rule, both of which it
already has. **What it must not borrow:** transactions, undo through the op log,
and entity-granularity dirty propagation.

**What the UCG should borrow from the comp graph:** the attribute-key edge mask
proposed in `01-3d-dcc.md` §1.3.3(1) is the UCG's version of "which channels do
I actually need" — the same idea, one level up. The comp graph's need for a
cheap metadata pass is also the UCG's: `Graph::dependents` recomputed per
reactor is the same defect class as `hashes()` recomputed per evaluation (F5).

---

# PART C — VERIFIED LICENCES

Every row below was fetched from the project's own licence file or licence page
on **2026-09-08**. Rows that could not be fetched are marked and repeated in the
UNVERIFIED register (§H).

## C.1 The imaging and plug-in standards

| Project | Licence (verified) | Source fetched |
|---|---|---|
| **OpenEXR** | **BSD-3-Clause** — *"Copyright (c) Contributors to the OpenEXR Project. All rights reserved."* | `raw.githubusercontent.com/AcademySoftwareFoundation/openexr/main/LICENSE.md` |
| **Imath** | **BSD-3-Clause** — *"Copyright Contributors to the OpenEXR Project. All rights reserved."* | `raw.githubusercontent.com/AcademySoftwareFoundation/Imath/main/LICENSE.md` |
| **OpenImageIO** | **Apache-2.0** for code, **CC-BY-4.0** for documentation — **and this is a change from what the blueprint assumed** | `raw.githubusercontent.com/AcademySoftwareFoundation/OpenImageIO/main/{LICENSE.md,README.md}` |
| **OpenColorIO** | **BSD-3-Clause** (docs CC-BY-4.0) | already verified in `10-color-and-sync.md` §A.5.1 |
| **OpenFX (OFX)** | **BSD-3-Clause** (`LICENSE.md`) | `github.com/AcademySoftwareFoundation/openfx` |
| **Cryptomatte** | **BSD-3-Clause** — *"Copyright (c) 2014, 2015, 2016 Psyop Media Company, LLC All rights reserved."* (the file is `license.txt`, lower-case) | `raw.githubusercontent.com/Psyop/Cryptomatte/master/license.txt` |
| **Natron** | **GPL-2.0-or-later** — verified from a source-file header, not just the README | `raw.githubusercontent.com/NatronGitHub/Natron/RB-2.5/Engine/Node.cpp` |
| **Nuke** (Foundry) | Proprietary, commercial | not fetched — **UNVERIFIED** as to NDK plug-in terms |
| **Fusion** (Blackmagic) | Proprietary, commercial | not fetched — **UNVERIFIED** |
| **Blender** | GPL — exact version **UNVERIFIED in this session** (both `blender.org/about/license/` and the GitHub/`projects.blender.org` raw `LICENSE` returned 403/404). `01-LICENSING-SPINE.md` §3 already records it as GPL and unlinkable; that conclusion does not change | fetch failed |

**Three findings in that table change decisions.**

**(a) OpenImageIO is Apache-2.0, not BSD-3-Clause.** The blueprint's licensing
spine §5 lists OIIO under *"expected permissive, UNVERIFIED"*. It is now
verified, and it is *better* than expected: the OIIO README states
*"For original code, we use the Apache-2.0 license, and for documentation, the
Creative Commons Attribution 4.0 Unported License,"* and records the relicensing
explicitly: *"In 2023 we asked historical users to relicense from the original
BSD-3-clause license to Apache-2.0, and over 99.86% of lines of code have been
relicensed to Apache-2.0."*

Apache-2.0 is CINE-OMNI's *own* licence. That makes OIIO the one major C++
imaging library that could, in principle, have code **ported** into CINE-OMNI
rather than merely linked — no licence-compatibility analysis needed, only
attribution. **But note the 0.14 %**: a residue of lines is still BSD-3-Clause.
BSD-3 is Apache-2.0-compatible so this is not a blocker, but any port must carry
both notices. The CC-BY-4.0 on documentation is the trap the colour document
already flagged for OCIO: **do not copy OIIO's prose into CINE-OMNI docs**
without CC-BY attribution.

**(b) Natron is GPL-2.0-*or-later*, not GPL-2.0-only.** The distinction matters
because `01-LICENSING-SPINE.md` §2 correctly states that *GPL-2.0-only* plus
Apache-2.0 is *outright incompatible* (Apache's patent-termination clause is an
"additional restriction" under GPLv2). Natron's per-file header reads:

> *"Natron is free software: you can redistribute it and/or modify it under the
> terms of the GNU General Public License as published by the Free Software
> Foundation; either version 2 of the License, or (at your option) any later
> version."*

Because of the "or any later version", Natron can be taken as GPLv3, which *is*
one-way compatible — **in the direction Apache-2.0 → GPLv3, not the reverse.**
So the practical answer is unchanged: **Natron source may not be copied into or
linked with CINE-OMNI.** What changes is only the *reason*, and one real
consequence: a hypothetical GPLv3 distribution of a CINE-OMNI+Natron combination
would be legally possible for a third party, which is worth knowing but is not a
route CINE-OMNI can take for its own product. **TYPE G only.**

**(c) Cryptomatte and OpenFX are both BSD-3-Clause, which is the best possible
outcome for the two highest-leverage interop decisions in VFX.** Cryptomatte's
specification *and* its reference implementations (the repo contains `nuke/`,
`fusion/` and `specification/` directories) are permissively licensed by Psyop.
OpenFX — the plug-in standard supported, per its own README, by *"The Foundry
Nuke, Assimilate Scratch, Sony Vegas, FilmLight Baselight"* — is BSD-3-Clause,
meaning **CINE-OMNI may implement an OFX host without a licence obstacle.**
`01-LICENSING-SPINE.md` §5 flagged OFX terms as *"potentially the
highest-leverage decision in VFX"*; that flag is now resolved in the
favourable direction. (Whether hosting OFX is *worth* the C ABI, threading
contract and suite-implementation burden is a separate engineering question —
see §G.R6.)

## C.2 The Rust crate ecosystem — measured, not assumed

Every row fetched from the crates.io JSON API on **2026-09-08**. Download
counts and dates are quoted, not estimated.

### Image I/O

| Crate | Version | Licence | Downloads (recent 90d) | Last updated | Verdict |
|---|---|---|---|---|---|
| **`exr`** | **1.74.2** | **BSD-3-Clause** | 69 708 541 (17 069 063) | 2026-07-10 | **Viable and mature.** *"Read and write OpenEXR files without any unsafe code"*. Repo `github.com/johannesvollmer/exrs`. The single most important crate in this document |
| `openexr` (vfx-rs) | 0.11.0 | BSD-3-Clause | 23 695 (304) | **2021-10-21** | Abandoned. FFI to *"OpenEXR 3.0.5"*. Five years stale |
| `openexr-sys` | 0.10.1 | BSD-3-Clause | 23 244 (341) | **2021-09-01** | Abandoned |
| `oiio` | 0.2.1 | Apache-2.0 | **72 (72)** | 2026-08-17 | **Three weeks old, 72 downloads.** Not a dependency, a science project |
| `openimageio` | 0.1.0 | BSD-3-Clause | 1 826 (11) | **2020-10-20** | Dead. Its own description says **"(placeholder)"** |
| `opencolorio` | 0.1.0 | BSD-3-Clause | 1 789 (9) | **2020-10-20** | Dead. Description says **"(placeholder)"** — consistent with `10-color-and-sync.md` §A.5.1 |
| `image` | 0.25.10 | MIT OR Apache-2.0 | 183 888 371 | 2026-03-10 | Mature. Note it delegates EXR to `exr` |

**The headline: `exr` is real and `openexr`/`oiio` bindings are not.** A pure-Rust,
no-unsafe EXR implementation under BSD-3-Clause with 17 million downloads in 90
days is a materially better dependency than a five-year-stale FFI binding to a
C++ library — no build-system dependency, no `bindgen`, no ABI surface, and it
compiles on every target the workspace already supports (which matters on this
machine, where there is no system `cc` — see `MEMORY.md`).

**Its support envelope was fetched and is tabulated in §E.4:** tiled and
multi-part read/write ✅, all lossless codecs ✅, **deep data ❌**. The one
contradiction in its own documentation (DWAA/DWAB) is registered as **H-2**.

### Numerics, resampling, caching

| Crate | Version | Licence | Downloads (recent) | Updated | Note |
|---|---|---|---|---|---|
| `half` | 2.7.1 | MIT OR Apache-2.0 | 446 707 159 (102 242 741) | 2025-10-14 | `f16` — the EXR native pixel type. Unavoidable and safe |
| `fast_image_resize` | 6.1.0 | MIT OR Apache-2.0 | 18 505 032 (3 496 181) | 2026-07-21 | SIMD resampling — the direct fix for **F4** |
| `imageproc` | 0.27.0 | MIT | 12 763 960 (2 783 424) | 2026-06-02 | Corner detection, morphology, filters |
| `ndarray` | 0.17.2 | MIT OR Apache-2.0 | 118 127 191 | 2026-01-10 | Already indirectly familiar; not currently a workspace dep |
| `nalgebra` | 0.35.0 | **Apache-2.0** | 87 902 703 | 2026-05-24 | Homography / bundle-adjustment linear algebra |
| `levenberg-marquardt` | 0.15.0 | MIT | 1 243 041 (437 662) | 2025-08-03 | Non-linear least squares — lens-distortion and planar-track solving |
| `arrsac` | 0.11.0 | MIT | 83 433 (8 983) | 2025-08-03 | RANSAC variant for robust model fitting |
| `moka` | 0.12.16 | `(MIT OR Apache-2.0) AND Apache-2.0` (compound — read it before use) | 121 604 603 (34 036 694) | 2026-08-09 | **Concurrent cache with a size-aware eviction policy — the direct fix for F6** |
| `quick_cache` | 0.7.0 | MIT | 25 505 308 (6 780 199) | 2026-06-27 | Lighter alternative to `moka` |
| `lru` | 0.18.4 | MIT | 335 855 347 | 2026-09-03 | Simplest option; not concurrent |
| `salsa` | 0.28.2 | Apache-2.0 OR MIT | 7 966 242 (2 263 439) | 2026-08-03 | **Incremental-computation framework** (the engine behind rust-analyzer). Worth studying, see §G.R3 |
| `petgraph` | 0.8.3 | MIT OR Apache-2.0 | 498 038 257 | 2025-09-30 | Already a workspace dependency |
| `wgpu` | **30.0.1** | MIT OR Apache-2.0 | 34 337 554 | 2026-08-22 | **`crates/cine-gpu/Cargo.toml` pins `wgpu = "26"`.** `11-shell-and-gpu-runtime.md` §B.1 states its findings were *"verified against wgpu 30.0.1"*. **The GPU crate is four majors behind the document that specifies it** |

### Vision, tracking, optical flow — the honest picture

| Crate | Version | Licence | Downloads (recent) | Updated | Verdict |
|---|---|---|---|---|---|
| `opencv` (opencv-rust) | 0.100.1 | MIT | 4 045 599 (855 305) | 2026-07-31 | **The only serious option**, and it is an FFI binding requiring a system OpenCV install. Licence of the *binding* is MIT; the licence of **OpenCV itself is UNVERIFIED here** (§H-4) |
| `kornia-rs` | 0.1.5 | **`non-standard`** — crates.io cannot classify it; **must be read manually** | 10 857 (244) | **2024-07-10** | Two years stale, and an unclassifiable licence field is a hard stop until read (§H-5) |
| `akaze` | 0.7.0 | MIT | 27 131 (1 134) | **2020-06-23** | Abandoned six years |
| `cv-core` | 0.15.0 | MIT | 56 237 (1 719) | **2020-06-23** | Abandoned six years. The `rust-cv` ecosystem did not survive |
| `optical-flow-lk` | 0.3.1 | *not fetched* | **5 104** | 2026-06-19 | Toy |
| `rustflow` | 0.1.0 | *not fetched* | **1 693** | **2020-11-23** | Toy, abandoned |
| `purecv` | 0.8.0 | *not fetched* | **2 046** | 2026-09-03 | Toy |

**The conclusion is blunt and it is a planning fact, not an opinion: there is no
production-grade pure-Rust optical flow, feature tracker, planar tracker or
camera solver.** The entire top of a crates.io search for "optical flow" is
crates with four-digit lifetime download counts. Any tracking, retiming or
camera-solve capability in CINE-OMNI is therefore one of:

* **TYPE B** — link `opencv` (system OpenCV, MIT binding) and accept a heavyweight
  C++ build dependency on a machine that currently has no system `cc`;
* **TYPE G** — implement pyramidal Lucas-Kanade and homography estimation from
  the published algorithms, on top of `nalgebra` + `levenberg-marquardt` +
  `arrsac`, all of which are permissive and maintained;
* **or not shipped.**

There is no fourth option where a crate does it for us. §G.R7 recommends which.

---

# PART D — THE TWO SPECIFICATIONS THAT MATTER

## D.1 Cryptomatte — verified against the actual specification

**Source:** `specification/cryptomatte_specification.pdf` in
`github.com/Psyop/Cryptomatte`, fetched and text-extracted 2026-09-08.
**Document header: "Cryptomatte Specification version 1.2.0".** Licence
**BSD-3-Clause**, Psyop Media Company LLC (§C.1).

This is the most actionable section in the document, because Cryptomatte is
**small, fully specified, permissively licensed, comes with published test
vectors, and requires no new dependency.** Everything below is quoted from the
spec.

### The channel layout

Layers are named `{typename}`, `{typename}00`, `{typename}01`, `{typename}02`, …
— *"there is no requirement to use the word 'Crypto' in the name."* Each numbered
layer packs **two** ranked (id, coverage) pairs into its four channels:

```
{typename}.r   - Deprecated
{typename}.g   - Deprecated (normally filtered preview colors)
{typename}.b   - Deprecated (normally filtered preview colors)
{typename}00.r - ID rank 0
{typename}00.g - Coverage for ID rank 0
{typename}00.b - ID rank 1
{typename}00.a - Coverage for ID rank 1
{typename}01.r - ID rank 2
{typename}01.g - Coverage for ID rank 2
{typename}01.b - ID rank 3
{typename}01.a - Coverage for ID rank 3
{typename}02.r - ID rank 4    ... and so on
```

*"The suggested default value is 6 levels, meaning 3 numbered EXR layers (00, 01,
and 02)."* Two decisions follow immediately for CINE-OMNI: the rank count is
**not fixed** (*"An arbitrary number of channels … are allowed"*), so the reader
must discover it by enumerating channels, not assume three layers; and the
**preview channels are deprecated** — *"They may be left black … or skipped
entirely"* — so a decoder must not depend on them and an encoder should not
bother writing them.

### The metadata keys — quoted literally

```
cryptomatte/adb0a84/name       = CryptoAsset
cryptomatte/adb0a84/hash       = MurmurHash3_32
cryptomatte/adb0a84/conversion = uint32_to_float32
cryptomatte/adb0a84/manifest   = {"bunny":"13851a76", ...}
cryptomatte/d593dd7/manif_file = manifest.1001.json
```

`name`, `hash` and `conversion` are **required**; `manifest` and `manif_file`
are optional and mutually alternative. The middle path segment is a *"7-character
string as the 'metadata key'"* (changed from an integer in spec 1.1.0) — a
per-file index, **not** an object id. `hash` currently admits exactly one value,
*"MurmurHash3_32 is the only one currently"*; `conversion` exactly one,
*"Only one conversion method, 'uint32_to_float32', is supported."* Sidecar
manifest paths are, since 1.2.0, *"always relative to the image"*.

### The hash — the part implementations get wrong

*"Only one hash is currently supported, the 32 bit variant of MurmurHash3 (Also
known as MurmurHash3_x86_32)."* Seed **0** (visible in the spec's own C++:
`MurmurHash3_x86_32(name, strlen(name), 0, &m3hash)`). Names are hashed as
**UTF-8 bytes**: *"The bytes of the UTF encoding (and not the character code
points) are hashed."* *"If there is no object in the sample, the recorded hash
value should be zero."* *"Only little-endian values are supported."*

The uint32 → float32 conversion is a bit-for-bit reinterpret **with an exponent
repair**, and this is the one non-obvious step:

```python
def mm3hash_float(name):
    hash_32 = mmh3.hash(name)
    exp = hash_32 >> 23 & 255
    if (exp == 0) or (exp == 255):
        hash_32 ^= 1 << 23
    packed = struct.pack('<L', hash_32 & 0xffffffff)
    return struct.unpack('<f', packed)[0]
```

The spec's reasoning: *"all 32 bits cannot be used, due to NaN and inf signal
values potentially being indistinguishable from each other in certain contexts."*
The C++ comment states the intent precisely — *"if all exponent bits are 0
(subnormals, +zero, -zero) set exponent to 1; if all exponent bits are 1 (NaNs,
+inf, -inf) set exponent to 254"*. **Note the Python and the prose disagree in
form**: the prose says *"clamping it to be within the [1:254] range"*, while the
code achieves it by XOR-ing bit 23. For the two boundary cases those are
equivalent; a reimplementation should follow the *code*, and the divergence is
worth a comment in the source. Manifest values are the hex of the **float's**
bits, not the integer hash: *"the hashes are hexadecimal versions of bytes making
up the 32-bit float hashes"*, produced by `memcpy` of the float into a `uint32_t`
then `sprintf("%08x")`.

### The published test vectors — free conformance tests

The spec gives worked examples. These become CINE-OMNI unit tests verbatim:

| Name | UTF-8 bytes | Hash (unsigned) | Hash (signed) | Float |
|---|---|---|---|---|
| `torus` | `74 6f 72 75 73` | 4053562365 | −241404931 | −1.54943624832e+30 |
| `равнина` (Cyrillic) | `d1 80 d0 b0 d0 b2 d0 bd d0 b8 d0 bd d0 b0` | 2518896926 | −1776070370 | −1.31926312124e-25 |

The second vector is the valuable one: it proves the implementation hashes UTF-8
bytes rather than code points, which is exactly the bug a naive port introduces.

### What extracting a matte actually requires

The spec defines the *encoder* (a ranking pixel filter inside a renderer) in
detail and leaves the decoder implicit, but the decode algorithm follows
directly from the encoding:

1. Read the header, find every `cryptomatte/<key>/name` and group by `<key>`.
2. Check `hash == "MurmurHash3_32"` and `conversion == "uint32_to_float32"`;
   **refuse by name** if not — this is exactly the `cine-media` rule in
   `CLAUDE.md` ("a codec that is not handled returns an error naming the real
   alternatives, never a blank frame").
3. Enumerate `{typename}NN` layers to discover the available rank count.
4. Convert the wanted object name to a float id with `mm3hash_float`.
   **The manifest is not needed for this** — the spec is explicit:
   *"Including the manifest is optional … Decoders should not rely on it to
   accurately extract mattes."* The manifest is for the *UI* (letting a user pick
   a name from a list), not for the maths.
5. Per pixel, sum the coverage of every rank whose id equals the target:
   `matte(x,y) = Σ_r coverage_r(x,y) · [id_r(x,y) == target]`. Float ids compare
   **exactly**; do not use a tolerance.
6. For a multi-name selection, sum over the set of targets. Because the ranking
   filter normalises coverage by total pixel weight, these sums are already a
   correct antialiased, motion-blurred, transparency-aware matte —
   *"The weights for each ID are normalized by dividing by the total weight of
   samples contributing to the pixel, and these normalized weights become
   'coverage' values."*

**Cost estimate for CINE-OMNI:** MurmurHash3_x86_32 is ~40 lines of Rust
(public-domain algorithm; write it from the published algorithm, do not copy a
GPL implementation). The float conversion is 6 lines. The decoder is a channel
enumeration plus one `Shuffle`-like node. The **only** hard dependency is being
able to read arbitrary named channels and string metadata out of an EXR —
which is §E.1's decision, not a Cryptomatte problem. **Classification: TYPE A
(native implementation of an open spec), with the spec and reference code
available under BSD-3-Clause if any is needed.**

`cine-comp` today has one hook for this: `ChannelSet::Named(Vec<String>)`, whose
doc comment already says *"AOVs, cryptomatte ids, …"*. That is the right
substrate and nothing else exists.

## D.2 Deep compositing — verified against the OpenEXR documents

**Sources:** `openexr.com/en/latest/InterpretingDeepPixels.html` and
`openexr.com/en/latest/TheoryDeepPixels.html`, fetched 2026-09-08.

### What the format actually is

A deep image stores *"an arbitrary number of values or samples per channel"* per
pixel, samples *"numbered from 0 to n(x,y) − 1"*. *"The base layer of a deep
image must include a depth channel that is called `Z`"*; `ZBack` is optional.
*"The depth samples S_i(Z) and S_i(ZBack) determine the positions of the front
and the back of sample number i."* Critically for correctness:
*"Every color or auxiliary channel in a deep image must have an associated alpha
channel."*

The point/volume distinction is **defined by an inequality**, not a flag:

> *"If S_i(Z) ≥ S_i(ZBack), then sample number i … covers the single depth value
> z = S_i(Z) … Sample number i is called a **point sample**."*
> *"If S_i(Z) < S_i(ZBack), then sample number i … covers the half open interval
> S_i(Z) ≤ z < S_i(ZBack). Sample number i is called a **volume sample**."*

### Tidiness, and the algorithm `cine-comp` is missing

A deep pixel is **tidy** when its samples are *"sorted and non-overlapping"*.
Making it tidy requires splitting:

> *"Split partially overlapping samples: if … sample i is either a point or a
> volume sample, sample j is a volume sample, and S_j(Z) < S_i(Z) < S_j(ZBack),
> then split sample j at S_i(Z)."*

Splitting a volume sample is **not** linear interpolation. The theory document
gives the exact rule, where `r` is the fraction of the sample's depth extent:

```
r  = (z − z_front) / (z_back − z_front)
α' = 1 − (1 − α)^r
C' = rC                     if α = 0
C' = C · α'/α               otherwise
```

That `1 − (1 − α)^r` is Beer–Lambert absorption over a shorter path — an
exponential, not a lerp. Merging two *overlapping* volume samples is likewise
log-space:

```
α_c = 1 − (1 − α_a)(1 − α_b)
C_c = α_c · [ C_a·log(1−α_a)/α_a + C_b·log(1−α_b)/α_b ] / log(1−α_c)
```

Flattening a **tidy** pixel is then ordinary Porter–Duff:
*"Composite sample 0 over sample 1. Composite the result over sample 2, and so
on, until sample n−1 is reached."*

### Measured against what `cine-comp` implements

`crates/cine-comp/src/image.rs` has `DeepSample { z, z_back, r, g, b, a }`,
`DeepImage`, `flatten()`, `merged()`, `from_image()` and `holdout()`, and
`ops/deep.rs` wraps them as four node kinds. Read against the spec:

| Spec requirement | `cine-comp` | Verdict |
|---|---|---|
| `Z`/`ZBack` per sample | present in `DeepSample` | ✅ structure right |
| Point vs volume by `Z ≥ ZBack` | **never tested anywhere** | ❌ |
| Tidying: split overlapping samples | **absent** | ❌ |
| Beer–Lambert split `α' = 1−(1−α)^r` | **absent**; `scaled(t)` does `a *= t` — a **linear** scale | ❌ **wrong maths** |
| Log-space merge of overlapping volumes | **absent**; `merged()` concatenates and sorts | ❌ |
| Sort before composite | `flatten()` sorts by `z` only | ⚠️ partial — ignores `ZBack` |
| Flatten as front-to-back over | correct *for point samples* | ✅ conditionally |
| Holdout by transmittance | `holdout()` uses `if h.z < s.z { t *= 1.0 − h.a }` | ❌ strict `<` on front depth only; a holdout volume that *straddles* a sample neither splits it nor partially attenuates it |
| Deep file I/O | **none** | ❌ |

**The honest summary: `cine-comp`'s "deep compositing" is a per-pixel
depth-sorted point-sample compositor.** For point samples with no overlap it
produces correct results, and its tests only exercise that case
(`DeepSample::point` is the only constructor). For volumetric data — which is
what deep is *for*, since the whole point is holding out volumetric atmospherics
and motion-blurred edges against CG — it is silently wrong. `DeepSample::scaled`
scaling alpha linearly is the specific defect: it is the operation that should be
`1 − (1 − α)^r`.

### Is deep worth supporting? A decision, not a shrug

**Arguments against:** deep files are very large; the `exr` crate does not
support deep data (§C.2, verified); the tidying/splitting algorithm is genuinely
subtle; and the primary consumers — holdouts and volumetric integration — are
mainly needed when CINE-OMNI is generating the CG itself, which it currently is
not (`cine-previs` renders analytic test passes).

**Arguments for:** it is the one part of the comp feature set that *cannot* be
retrofitted, because it changes the `Output` type of every node and the shape of
the image substrate. `cine-comp` has already made that structural choice —
`Output` is `Image(Arc<Image>) | Deep(Arc<DeepImage>)`, and the node kinds exist.

**Recommendation: keep the structure, fix the maths, and defer the file format.**
Concretely: (1) implement `tidy()` with the spec's split rule and the
Beer–Lambert formula, and make `flatten()`/`holdout()`/`merged()` require a tidy
input; (2) add a `is_point()` helper asserting the `Z ≥ ZBack` rule and test both
branches; (3) **do not** implement deep EXR I/O yet, and **do not** claim deep
support in any user-facing text until (1) and (2) land. The current state — four
node kinds named `Deep*` that are documented as deep compositing and implement
point-sample compositing — is exactly the kind of unearned capability claim that
`CLAUDE.md` forbids for `cine-media` ("never make a capability claim the build
cannot back"). The same rule should be written down for `cine-comp`.

---

# PART E — THE CORRECTNESS RULES AND THE IMAGE SUBSTRATE

## E.1 Premultiplication and linear light — the rules, and where CINE-OMNI breaks them

`10-color-and-sync.md` §A.4 already establishes the conventions and the decode
boundary. This section states only the **comp-engine-internal** rules and checks
each against the code.

**R-P1 — Filtering, resampling and blending are valid *only* on premultiplied
data.** Interpolating straight RGB across a coverage edge mixes in the undefined
colour of transparent pixels. `cine-comp` is premultiplied throughout. ✅

**R-P2 — Point colour operations (grade, gamma, saturation) are the exception:
they are valid only on *straight* data**, because `f(C·A) ≠ A·f(C)` for any
non-linear `f`. Hence `unpremult → operate → premult`. `ops::color::color_correct`
does exactly this. ✅ *in form*, ❌ *in the boundary case* — see R-P3.

**R-P3 — The unpremult/premult sandwich must be reversible, and it is not at
α = 0.** This is finding **F3** and it is a measured bug in the working tree, not
a theoretical concern:

```
in=[1.0, 0.8, 0.6, 0.0]  ->  out=[0.0, 0.0, 0.0, 0.0]
```

`unpremult_pixel` guards with `if p[3] > 0.0 { divide } else { p }`, correctly
passing an additive fragment through unchanged. But `color_correct` then calls
`premult_pixel([c0,c1,c2, p[3]])`, multiplying by `p[3] = 0` and destroying it.
An emissive element, a light wrap, an additive glow pass, or any RGB-only
"additive" AOV is annihilated by any colour correction. The correct behaviours,
in order of preference:

1. **Round-trip only what was divided.** Track whether the divide happened; if
   it did not, do not multiply. One `bool` per pixel, or equivalently
   `if a > 0 { premult } else { keep }`.
2. **Expose it.** Nuke's convention is that grade nodes take an explicit
   `(un)premult` input rather than doing it implicitly; making the sandwich a
   *parameter* (`alpha_handling: Ignore | UnpremultPremult`) is what a
   professional keyer/grade node does and is honest about the trade.

Note also `unpremult_pixel`'s guard is `> 0.0`, not `> ε`. At `α = 1e-30` with
`RGB = 1e-30` it divides to 1.0, which is fine; at `α = 1e-38` with
`RGB = 1e-20` it produces ~1e18 and any subsequent `powf` produces `inf`. A
denormal-alpha guard is cheap insurance.

**R-P4 — Never clamp `RGB ≤ A`.** `10-color-and-sync.md` §A.4.1 already states
this. `ops::color::clamp` is an explicit user-invoked node with a `clamp_alpha`
flag, which is correct: clamping should be something the artist asks for, never
something the engine does silently. ✅

**R-P5 — `invert` must be alpha-relative on premultiplied data.**
`ops::color::invert` computes `[a−r, a−g, a−b, a]`. That is the correct
premultiplied inverse (equivalent to `1−C` on straight data, re-premultiplied),
and it is a detail many implementations get wrong. ✅ — worth noting because it
shows the crate was written by someone who knew the convention.

**R-P6 — All of the above assume scene-linear.** `cine-comp` has
`srgb_encode`/`srgb_decode` at the *display* boundary only (`to_display_u8`),
and every op works on raw `f32` with no transfer function applied. That is the
correct structure. **But there is no colour-space *identity* on `Image` at all** —
no field says whether these floats are ACEScg, Rec.709-linear, or (through a
mis-wired `Read`) still log-encoded. The engine cannot detect the single most
expensive mistake in compositing. `cine-color` has the whole machinery
(`10-color-and-sync.md` §A.7 lists 14 gamuts, 15 transfer functions, an OCIO v2
config parser); `cine-comp` does not reference `cine-color` at all — its
`Cargo.toml` dependency list is `ucg-core, ucg-schema, serde, serde_json,
thiserror, indexmap, uuid, rayon, glam, petgraph`. **Adding a
`colorspace: ColorSpaceId` field to `Image` and a `cine-color` dependency is a
small change that converts a class of silent errors into loud ones.**

**R-P7 — Merge must union the data windows, not crop to the B input.**
`ops::merge::merge` resolves a size mismatch with
`a.to_rgba().crop(0, 0, b.width, b.height)` — the foreground is clipped to the
background's size, **anchored at the origin**, with no notion of a shared
coordinate space. A foreground larger than the background is silently truncated;
a foreground offset from the origin is silently misplaced. This is R-P7 failing
for the reason given in §B.2(4): there is nowhere else to negotiate format, so
it is being negotiated wrongly at pixel time. ❌

## E.2 Filtering — the measured aliasing defect and its fix

Finding **F4**: `Image::resize` calls `sample_bilinear_into` per output pixel,
reading a 2×2 source neighbourhood *regardless of the reduction ratio*. Measured
on a period-3 stripe pattern reduced 8× against an exact box reference:

```
bilinear row: [0.500, 0.500, 0.000, 0.500, 0.500, 0.000, ...]
box  ref row: [0.375, 0.375, 0.250, 0.375, 0.375, 0.250, ...]
max abs diff  0.2500
```

62 of every 64 source pixels are discarded. Two consequences worth stating
separately: the visible one is aliasing on any downscale (proxies, `Reformat`,
scale-down `Transform`); the invisible one is that **`resize` is not
energy-preserving**, so it cannot be used to build a mip pyramid, which is
itself a prerequisite for a correct large-reduction filter.

The comp-specific requirements a filter set must meet, beyond "don't alias":

* **Separable, with a selectable kernel.** Production compositors expose a
  filter menu per transform (Nuke's list is conventionally Impulse, Cubic, Keys,
  Simon, Rifman, Mitchell, Parzen, Notch, Lanczos, Sinc — **UNVERIFIED**, not
  fetched from Foundry's documentation in this session). The reason it is
  *exposed* rather than fixed is that the right answer differs per shot.
* **Negative lobes are dangerous in HDR.** Any windowed-sinc or Catmull-Rom
  kernel has negative lobes; against a scene-linear highlight of 60.0 next to
  0.001, the ringing undershoot goes **negative**, and a negative premultiplied
  value composites as a black halo that survives the display transform. The
  engine must either clamp the *filtered result* to the min/max of its own
  support (a "clamp to input range" option), or ship an impulse/box path for
  HDR sources. `sample_bicubic` in `image.rs` is Catmull-Rom with no such guard.
* **`sample_bicubic` promotes to RGBA and returns `[f32; 4]`.** On a
  `ChannelSet::Named` image with 12 AOV channels, bicubic sampling silently
  drops 8 of them. `sample_bilinear_into` is written generically over `nc` and
  does not have this bug; the bicubic path does.

**Fix:** `fast_image_resize` 6.1.0 (MIT OR Apache-2.0, 3.5 M downloads in 90
days, §C.2) provides SIMD Lanczos/Mitchell/CatmullRom/Box with proper support
scaling. It operates on interleaved buffers, so it fits `Image`'s layout. This
is a dependency-sized fix, not a research project.

## E.3 Tiled and scanline access, and what OpenImageIO's ImageCache actually does

**Why this matters, in one number.** From the measurements in §0: one
4096×3112 RGBA f32 frame is **194.5 MiB**; 8K is **540.0 MiB**. A 40-AOV
multi-part EXR at 4K is therefore ~2 GB *decompressed*, for a comp whose output
may need `rgba` only, in a 200×200 pixel viewer region. Whole-image decode is
not a slow path, it is a wrong architecture.

**What OIIO's ImageCache does** (verified from
`openimageio.readthedocs.io/en/latest/imagecache.html`, fetched 2026-09-08):

* The stated purpose is to *"read pixels from a large number of image files
  while using a remarkably small amount of memory and other resources."*
* **The tile is the unit of caching**, and the cache is bounded: `max_memory_MB`
  is *"the approximate maximum amount of memory (measured in MB) used for the
  internal 'tile cache'"*, **default 1024 MB**.
* File handles are separately bounded: it keeps *"a reasonable number of
  simultaneously-open files, automatically closing files that have not been
  needed recently"*, via `max_open_files`, **default 100**.
* **Scanline files are handled by faking tiles.** `autotile`, when nonzero,
  caches untiled images *"as virtual tiles of that size"*; `autoscanline` makes
  those virtual tiles *"as wide as the image (but only `autotile` scanlines
  high)"*. This is the important design lesson: **the cache does not require the
  file to be tiled** — it imposes a tile abstraction over whatever the file is.
* It is *"completely thread-safe"*, with per-thread state obtainable via
  `get_perthread_info()` / `create_perthread_info()`.

**The three ideas CINE-OMNI should take, independent of whether it ever links
OIIO:**

1. **Bounded by memory, not by entry count.** OIIO's budget is in megabytes.
   `cine-comp`'s cache is an unbounded `HashMap` (F6). The fix is `moka`
   (§C.2), which supports a weight-based eviction policy — weigh an entry by
   `width·height·nc·4` bytes.
2. **Bound the file handles separately from the pixels.** A 200-shot conform
   opening 200 EXR sequences will exhaust the process fd limit long before it
   exhausts RAM.
3. **Impose the tile abstraction above the file format.** The comp engine's
   internal unit should be a tile whether the source is a tiled EXR, a scanline
   EXR, or a decoded video frame from `cine-media`. That decouples the cache
   design from the reader.

**Note on `exr`'s tiling support:** verified — *"[x] Tiles"* under both
singlepart and multipart, read and write (§E.4). So the tiled path is available
in pure Rust.

## E.4 The EXR decision — the single largest gap

`cine-comp` has **no file I/O**. `Read(File { path })` returns an `Arc<Image>`
previously handed to `Evaluator::register_file`; `Write` appends a `WriteRecord`
to a `Vec` and writes nothing. This is the F1 finding and it gates everything
else in this document: Cryptomatte needs channel and metadata reads, AOV
workflows need multi-part reads, deep needs deep parts, and the ImageCache
design needs something to cache.

**The verified options.**

| Option | Licence | State | Verdict |
|---|---|---|---|
| **`exr` crate 1.74.2** | BSD-3-Clause | 69.7 M downloads, 17.1 M in 90 days, updated 2026-07-10, *"without any unsafe code"* | **Recommended** |
| `openexr` / `openexr-sys` 0.11.0 / 0.10.1 | BSD-3-Clause | **last updated 2021**, FFI to OpenEXR 3.0.5 | Rejected — abandoned |
| OpenImageIO via C++ FFI | **Apache-2.0** (verified §C.1) | no viable Rust binding: `openimageio` 0.1.0 is a self-declared *"(placeholder)"*; `oiio` 0.2.1 has **72 lifetime downloads** | Rejected as a *binding*; **retained as a design reference and as portable source** |

**What `exr` supports — verified from `github.com/johannesvollmer/exrs` README,
fetched 2026-09-08:**

| Feature | Support |
|---|---|
| Tiles (singlepart and multipart, read and write) | `[x]` |
| uncompressed, `zip line`, `zip block`, `rle`, `piz` | `[x]` (all lossless) |
| `pxr24` | `[x]` — *"lossless for f16 and u32"* |
| `b44`, `b44a` | `[x]` |
| `dwaa`, `dwab` | `[x]` **in the checklist** |
| **deep data** | **`[ ]` — NOT SUPPORTED** |
| `HTJ2K32`, `HTJ2K256` | `[ ]` |

**A documentation inconsistency worth recording rather than glossing:** the same
README carries prose stating that *"deep data and DWA compression algorithms are
not yet supported"* while its own checklist marks `dwaa, dwab` as `[x]`. The
deep answer is consistent in both places and is a hard **no**. The DWA answer
is **contradictory in the project's own documentation** and must be settled
empirically — write a DWAA-compressed EXR with a known reference and round-trip
it — **before** any claim of DWA support is made in CINE-OMNI. This is exactly
the "what a system DOES versus what its docs CLAIM" distinction; it is registered
as **H-2**.

**Consequences of choosing `exr`:**

* Everything CINE-OMNI needs for **flat** compositing is available: multi-part,
  multi-channel, tiled, f16/f32/u32, all the lossless codecs and B44.
* **Deep EXR I/O is off the table in pure Rust.** This *confirms* the §D.2
  recommendation to fix the deep maths but defer the format. Supporting deep
  files would mean either contributing deep support upstream to `exrs`
  (BSD-3-Clause, so contribution is unencumbered) or taking a C++ dependency —
  and the C++ EXR bindings are five years stale.
* No system `cc` is required, which matters on this machine (see `MEMORY.md`:
  rustup + Zig linker, no sudo).

**Classification: `exr` is TYPE B (library).** OpenEXR-the-format is **TYPE A**
— CINE-OMNI implements an open specification. OpenImageIO is **TYPE G with an
unusual bonus**: its architecture (ImageCache, ImageBuf, ImageBufAlgo) is the
right design to study and reimplement, *and because it is Apache-2.0, actual
source may be ported if that is ever preferable to reimplementation* — the only
major C++ imaging library for which that is true.

## E.5 What must be GPU, and what must have a CPU reference

`11-shell-and-gpu-runtime.md` Part B covers the runtime. This section covers only
the **comp-specific** allocation of work, and it starts from a rule that the
repository already follows and should keep:

> **Every GPU kernel has a CPU mirror, and they are tested against each other.**
> `cine-gpu` already ships `cpu.rs` alongside its WGSL, and
> `10-color-and-sync.md` §A.7 records *"GPU/CPU agreement to 1e-5 on Intel UHD
> via Vulkan"*. This is the correct discipline and it is rare.

| Work | Where it must run | Why |
|---|---|---|
| Merge, grade, keyer primitives, LUT, transform, blur | **GPU for interaction, CPU mirror mandatory** | already exist in both forms (`merge.wgsl`, `keyer.wgsl`, `blur.wgsl`, `dof.wgsl`, `cdl.wgsl`, `lut3d.wgsl` + `cpu.rs`) |
| Viewer display transform, scopes | **GPU** | per-frame at display rate; `scopes.wgsl` exists |
| Large-kernel defocus, optical-flow retime, denoise | **GPU strongly preferred** | quadratic or iterative cost; CPU reference may be slow but must exist for correctness tests |
| **EXR decode / compression** | **CPU** | `exr` is CPU; GPU decompression of PIZ/ZIP is not worth it |
| **Deep sample operations** | **CPU** | variable-length per-pixel sample lists are a poor GPU data structure; and deep is not interactive |
| **Cryptomatte matte extraction** | **either** | trivially parallel per-pixel compare-and-sum; but it needs the *manifest* (CPU, string data) to resolve a name to an id |
| **Roto rasterisation** | **CPU first, GPU later** | correctness (winding, feather, area preservation) is subtle; `ops/roto.rs` already tests area to within 2 % |
| **The graph evaluator itself** | **CPU, always** | scheduling, hashing, caching and I/O are host work |
| **Tracking / solving** | **CPU** | iterative, branchy, low data volume |

**The rule that should be written into `docs/MODULE-CONVENTIONS.md`:** a comp
node may exist GPU-only *only if* a CPU reference exists that a test can compare
against. The alternative — GPU-only nodes — makes the compositor
non-deterministic across vendors and makes headless/farm rendering depend on a
GPU being present, which is the exact failure `11-shell-and-gpu-runtime.md`
§B.6 flags as *"No CPU fallback routing … P2"*.

**One inconsistency to resolve:** `crates/cine-gpu/Cargo.toml` pins
`wgpu = "26"`, while `11-shell-and-gpu-runtime.md` §B.1 states its findings were
*"verified against wgpu 30.0.1"* and crates.io confirms **30.0.1** as the current
release (updated 2026-08-22). The GPU crate is four major versions behind the
document that specifies it. That is a Phase-3 blocker for any GPU comp work.

---

# PART F — FRANK ASSESSMENT OF `crates/cine-comp`

**Measured on 2026-09-08:** 7 486 lines of Rust across 11 source files and 3
integration test files; **28 node kinds**; **65 tests passing**
(`cargo test -p cine-comp`), including a doc-test.

## F.1 What it is

It is a **competently built node-graph engine with a naive image library
attached**, written by someone who understood the compositing conventions and
had not yet met a real plate. That is a much better starting point than the
reverse, and the distinction matters for planning: the parts that are hard to
retrofit are mostly *right*, and the parts that are wrong are mostly *local*.

**Genuinely good, keep as-is:**

* **The graph model.** `NodeKind::describe()` gives every kind a typed
  port/parameter description with defaults, which drives validation, JSON, the
  UCG mapping and (eventually) a UI, from one place. `validate()` reports
  dangling links, port-index errors, type mismatches and unconnected required
  inputs as strings. Cycles are rejected via `petgraph::toposort`.
* **Content-hash invalidation.** `hashes()` folds each node's local hash with
  its inputs' hashes in topological order, so a parameter change invalidates
  exactly that node and its downstream. The test
  `hashes_change_downstream_only` asserts precisely that. This is the correct
  mechanism.
* **The UCG mapping.** `cine-comp/src/ucg.rs` derives the comp entity from the
  shot (`EntityId::derived(shot, kinds::COMP_GRAPH)`), stores nodes as children
  with derived ids, keeps links in a single `links` attribute, and its writers
  are idempotent — `ensure_comp_is_idempotent` is a test. It follows every rule
  in `CLAUDE.md`. §B.4 argues this design is *right*, not merely conformant.
* **Premultiplied convention, held consistently**, including the non-obvious
  alpha-relative `invert` (§E.1 R-P5).
* **Parallelism.** `rayon` on rows and on independent graph branches, with a
  `parallel` flag to disable it for deterministic testing.

## F.2 What it is not

**It is not a compositor, for four reasons, in descending order of cost to fix.**

**(1) No image I/O.** F1. `Read(File)` reads from a `HashMap<String, Arc<Image>>`
populated by the test harness; `Write` records an in-memory image against a path
and writes nothing. Fixing this is the `exr` decision (§E.4) and it is the
cheapest of the four *to start* and the largest in consequence.

**(2) No data window / bounding box.** F2, and structurally the most expensive,
because it changes the `Image` type and therefore every op signature. Today
`Image` is `{ width, height, channels, data }` — an image *is* its format. A
professional `Image` is `{ display_window, data_window, channels, colorspace,
data }`, and the difference propagates everywhere. **The longer this is
deferred, the more of the 28 ops have to be rewritten.** The `RegionOfInterest`
type already exists and is already threaded through the evaluator; what is
missing is that nothing *allocates* against it.

**(3) No metadata pass.** §B.2. Without it there is no bbox propagation, no
channel-set propagation, no format negotiation, and no way to know the cost of a
graph before paying it. ~28 small functions plus ~100 lines of evaluator.

**(4) Naive numerics in three specific places** — all local, all testable:
bilinear-only `resize` (F4, §E.2), the additive-element grade bug (F3, §E.1),
and point-sample-only deep maths (§D.2).

## F.3 The performance defects, measured

| Defect | Measurement | Fix |
|---|---|---|
| `hashes()` recomputed on **every** `evaluate_output`, superlinearly | 77.8 µs @ 11 nodes → **53 224 µs @ 2 001 nodes** | cache against a graph revision counter |
| `NodeKind::describe()` allocates `String`s and `Vec`s per call, and is called per node per `inputs()` | **8 216 ns per call** | return `&'static NodeDesc` from a `OnceLock` |
| `CompGraph::input_link` linear-scans `self.links` | (implied by the above curve) | index links by `(to_node, port)` |
| Cache unbounded, no eviction | 240 frames → 480 entries; at 4K that is **~93 GiB** if the images were full-size (194.5 MiB each) | `moka` with a byte-weight budget |
| `evaluate_roi` allocates full frames | 64×64 request returned **16 777 216 floats** | data window (F2) |
| Concurrent evaluation can duplicate work | `eval_node` checks the cache, releases the lock, then computes — two threads asking for the same node both compute it | single-flight / occupied-entry marker |

The first two are the same defect class as the `Graph::of_kind()` finding in
`00-REPOSITORY-ARCHAEOLOGY.md` (*"74× slower than having no index at all,
because `sort_by_key` re-evaluates its key on every comparison"*): **a derived
value recomputed inside a hot loop.** That this pattern has now appeared three
times in the codebase suggests it is worth a lint or a convention rather than
three separate fixes.

## F.4 What a professional comp engine requires that is entirely absent

Listing these together so the scale of the gap is not understated. None of the
following exists in any form:

| Capability | Status |
|---|---|
| EXR / any image file read or write | **absent** (§E.4) |
| Data window / bounding box | **absent** (F2) |
| Metadata / validate pass | **absent** (§B.2) |
| Colour-space identity on `Image`; any link to `cine-color` | **absent** (§E.1 R-P6) |
| Cryptomatte | **absent**; `ChannelSet::Named` is the only hook (§D.1) |
| Multi-part / multi-channel AOV reading | **absent**; `MultiLayerImage` is an in-memory analogue with no reader |
| Tracking (point, planar), camera solve | **absent**, and no viable Rust crate exists (§C.2) |
| Optical flow, motion-vector retiming | **absent**; `ChannelSet::Motion` is a two-float channel set with no producer or consumer |
| Lens distortion / undistort, ST-maps | **absent** |
| Grain (analysis or synthesis), denoise | **absent** |
| Time-domain nodes of any kind (retime, frame blend, time offset) | **absent** — the evaluator takes a `frame` but no node reads a *different* frame than the one requested, so temporal graphs are inexpressible |
| Animation / keyframes on parameters | **absent** — `Param` values are static; `frame` reaches ops but not parameters |
| Proxy / mip resolution levels | **absent** |
| GPU execution of the comp graph | **absent** — `cine-gpu` kernels exist but `cine-comp` does not depend on `cine-gpu` |
| OFX or any plug-in host | **absent** |
| Disk cache / render farm output | **absent** |

**The two entries in that list that are architectural rather than additive** are
*time-domain nodes* and *animated parameters*. Both change the cache key and the
pull protocol: a `Retime` node asks upstream for frames `f−1, f, f+1`, and an
animated parameter makes a node's hash frame-dependent. `CacheKey` is already
`(NodeId, i64, u64)` with the frame in it, so the shape is right; but
`CompGraph::hashes()` takes no frame argument, so an animated parameter would
currently produce a frame-independent hash and serve the wrong cached frame.
**This is a latent correctness bug that will appear the day keyframes are
added**, and it is cheap to prevent now by threading `frame` into `hashes()`.

---

# PART G — CLASSIFICATION TABLE

Taxonomy per [`01-LICENSING-SPINE.md`](../01-LICENSING-SPINE.md) §2:
**A** native · **B** library · **C** engine (separate process) · **D** bridge ·
**E** import/export · **F** fork · **G** inspired-reimplementation · **H** reject.

| System | Licence (verified 2026-09-08 unless noted) | Class | Reasoning |
|---|---|---|---|
| **OpenEXR (the format)** | BSD-3-Clause | **A** | An open, documented file format. CINE-OMNI implements it (via a crate); the *format* is not a dependency |
| **`exr` crate 1.74.2** | BSD-3-Clause | **B** | Pure Rust, no unsafe, tiled + multipart + all lossless codecs, 17.1 M downloads/90 d. **No deep support** — a bounded, known limitation |
| **OpenEXR C++ / `openexr-sys`** | BSD-3-Clause | **H** | Bindings last updated **2021**; would add a C++ toolchain dependency to a machine with no system `cc`, for a capability `exr` already provides |
| **Imath** | BSD-3-Clause | **H** (as a dependency) / **A** (the maths) | Half/vector/matrix types; `half` + `glam` already cover this in Rust |
| **OpenImageIO** | **Apache-2.0** (code), CC-BY-4.0 (docs) | **G**, with a portability bonus | Its ImageCache/ImageBuf/ImageBufAlgo *design* is the reference (§E.3). Apache-2.0 means source **may** be ported if reimplementation proves harder — the only major C++ imaging library for which that is true. **Do not copy the CC-BY docs prose** |
| **OIIO Rust bindings (`openimageio`, `oiio`)** | BSD-3-Clause / Apache-2.0 | **H** | `openimageio` self-describes as *"(placeholder)"* and is from 2020; `oiio` has **72 lifetime downloads** |
| **OpenColorIO** | BSD-3-Clause | **G** | Already decided against as a runtime dependency in `10-color-and-sync.md` §A.5.4 (no WGSL, six-library C++17 tree). Comp does not change that analysis |
| **Cryptomatte spec + reference impls** | **BSD-3-Clause** (Psyop Media Company, LLC) | **A** | An open specification with published test vectors, implementable in ~200 lines. The reference `nuke/` and `fusion/` code is BSD-3 if needed. **Highest value-per-line item in this document** |
| **OpenFX (OFX)** | **BSD-3-Clause** | **B**, deferred | Hosting OFX is licence-clear and would unlock the commercial plug-in ecosystem (its README names *"The Foundry Nuke, Assimilate Scratch, Sony Vegas, FilmLight Baselight"*). The cost is a C ABI, a threading contract and a large suite surface — not a Phase-3 item, but the door is now known to be open |
| **Nuke** | Proprietary | **G** (+ **E**) | Its architecture — two-phase validate/request/engine, bbox propagation, hash-based caching — is the model §B.2 recommends borrowing. Interop is file-level: EXR in/out, and reading `.nk` scripts is a possible **E** |
| **Fusion** | Proprietary | **G** | The DoD/RoI *pair* is the idea worth taking (§B.3). Internals are not documented well enough to borrow more |
| **Natron** | **GPL-2.0-or-later** (verified from a source header) | **G** | Cannot be copied or linked into an Apache-2.0 product in any configuration. Study its OFX-host behaviour, never its source |
| **Blender compositor** | GPL (exact version **UNVERIFIED** this session) | **G** | Per `01-LICENSING-SPINE.md` §3 and `01-3d-dcc.md` §1.6, unchanged |
| **`fast_image_resize` 6.1.0** | MIT OR Apache-2.0 | **B** | Direct fix for the measured aliasing defect (F4) |
| **`moka` 0.12.16** | `(MIT OR Apache-2.0) AND Apache-2.0` — compound, read before adopting | **B** | Byte-weighted concurrent cache; direct fix for F6 |
| **`half` 2.7.1** | MIT OR Apache-2.0 | **B** | `f16` is EXR's native pixel type; unavoidable |
| **`salsa` 0.28.2** | Apache-2.0 OR MIT | **G** | Do not adopt: its query model assumes cheap values and CINE-OMNI's are 200 MiB frames. But its *durability/revision* design is the reference for fixing F5 |
| **`nalgebra` 0.35.0 / `levenberg-marquardt` 0.15.0 / `arrsac` 0.11.0** | Apache-2.0 / MIT / MIT | **B** | The permissive, maintained substrate for a **G** tracker |
| **OpenCV** | **Apache-2.0** (verified, `github.com/opencv/opencv/4.x/LICENSE`) | **B**, conditional | Licence is ideal. `opencv` crate 0.100.1 (MIT, 855 k downloads/90 d) is maintained. The cost is a system C++ dependency. **`opencv_contrib` non-free modules are a separate question — UNVERIFIED** |
| **`kornia-rs` 0.1.5** | **`non-standard`** — crates.io cannot classify it | **H** until read | Two years stale *and* an unclassifiable licence field. Two independent reasons to stop |
| **`akaze` / `cv-core` (rust-cv)** | MIT | **H** | Both last updated **2020-06-23**. The ecosystem did not survive |
| **`optical-flow-lk` / `rustflow` / `purecv`** | not fetched | **H** | 5 104 / 1 693 / 2 046 lifetime downloads. Toys |
| **Deep EXR I/O** | — | **defer** | No pure-Rust path exists (§E.4). Contributing deep support upstream to `exrs` (BSD-3) is the clean option if it becomes necessary |

---

# PART H — RECOMMENDATIONS FOR CINE-OMNI

Ordered so that each item unblocks the ones below it. Every item names the
finding it addresses and what would prove it done.

## H.R1 — Give `Image` a data window and a colour space, before anything else

**Addresses F2, R-P6, R-P7.** This is a type change and it is the only item on
this list that gets materially more expensive every week, because it touches all
28 ops.

```rust
pub struct Image {
    pub display_window: Box2i,   // the "format"
    pub data_window: Box2i,      // where pixels actually exist
    pub channels: ChannelSet,
    pub colorspace: ColorSpaceId, // from cine-color
    pub data: Vec<f32>,           // data_window-sized, not display_window-sized
}
```

`RegionOfInterest` already exists and is already threaded through the evaluator;
this is what makes it mean something. **Done when:** `evaluate_roi` on the
2048×2048 blur chain from §0 returns a buffer proportional to the requested
region rather than 16 777 216 floats, and `merge()` of differently-sized inputs
unions their data windows instead of cropping at the origin.

## H.R2 — Add the metadata pass

**Addresses §B.2.** One method per node kind
(`fn meta(&self, inputs: &[Meta]) -> Meta`) plus a cached topological pass in
`Evaluator`. Depends on H.R1 for `Box2i`. **Done when:** the bbox of a
`Roto → Blur(40) → Transform → Merge` chain can be queried without rendering,
and a `Blur` correctly reports a bbox grown by its kernel radius.

## H.R3 — Fix the three measured correctness bugs

All three are local and each has a test that would have caught it.

1. **The additive-element grade bug (F3).** `color_correct` must not multiply by
   an alpha it did not divide by. **Done when** `[1.0, 0.8, 0.6, 0.0]` graded by
   gain 2.0 yields `[2.0, 1.6, 1.2, 0.0]`, not `[0,0,0,0]`.
2. **Minification aliasing (F4).** Adopt `fast_image_resize`, or implement a
   support-scaled separable filter. **Done when** an 8× reduction of the period-3
   stripe probe matches the box reference to within ~0.02 instead of 0.25.
3. **Deep volumetric maths (§D.2).** Implement `tidy()` with the spec's split
   rule and `α' = 1−(1−α)^r`; make `flatten`/`holdout`/`merged` require tidiness.
   **Done when** a volume sample split at its midpoint and recomposited equals
   the original to float precision — and until then, **stop calling it deep
   compositing in the crate docs**.

## H.R4 — Fix the evaluator's cost and memory model

**Addresses F5, F6.** Three contained changes:

* `NodeKind::describe()` returns `&'static NodeDesc` from a `OnceLock` table
  (8 216 ns → ~0).
* `CompGraph` gains a revision counter; `hashes()` is memoised against it
  (53 ms → amortised ~0 at 2 001 nodes).
* Replace `Mutex<HashMap<..>>` with `moka` weighted by
  `width·height·nc·4` bytes, with a configurable budget. Add single-flight so
  two threads asking for the same node do not both compute it.

**Done when** a 2 000-node graph evaluates a cached node in microseconds, and a
240-frame playback of a 4K graph stays inside a stated memory budget instead of
growing without bound.

## H.R5 — Adopt `exr` and make `Read`/`Write` real

**Addresses F1.** BSD-3-Clause, pure Rust, no `cc` required. Scope for the first
increment: read single-part and multi-part scanline and tiled EXR into
`MultiLayerImage`; write single-part RGBA; carry string metadata through
(required by H.R6). Explicitly out of scope: deep parts (unsupported upstream),
and DWA until **H-2** is settled empirically.

**Done when** `cine-comp` can round-trip a real multi-part EXR from disk and
`WriteRecord` writes a file that OpenEXR's own tools can read.

## H.R6 — Implement Cryptomatte

**Addresses §D.1.** ~200 lines: MurmurHash3_x86_32 (seed 0, UTF-8 bytes), the
exponent-repair float conversion, header-key parsing, manifest JSON (inline and
sidecar), rank enumeration, and the compare-and-sum extractor. Depends on H.R5
for channel and metadata reads.

**Ship the spec's published vectors as tests**: `torus` → unsigned 4053562365 →
−1.54943624832e+30, and `равнина` → unsigned 2518896926 → −1.31926312124e-25.
The second is the one that catches the UTF-8-versus-code-points bug.

**This is the highest value-per-line item in the document**: it is small, fully
specified, permissively licensed, comes with conformance vectors, needs no new
dependency beyond H.R5, and it is the feature that makes CINE-OMNI legible to
anyone who has worked in a VFX pipeline.

## H.R7 — Decide the tracking posture explicitly, and write the decision down

**Addresses §C.2.** There is no fourth option (§C.2). The recommendation:

* **Now:** neither. Do not ship a tracker. Do not list tracking as a capability.
* **When it is needed:** **TYPE G** — pyramidal Lucas–Kanade and homography
  estimation built on `nalgebra` + `levenberg-marquardt` + `arrsac`, all
  permissive and maintained. This keeps the pure-Rust, no-system-`cc` property
  that `cine-media`'s build already depends on.
* **Reconsider TYPE B (`opencv`)** only if a camera solve (SfM/bundle
  adjustment) becomes a requirement, because that is where reimplementation
  stops being proportionate. OpenCV being **Apache-2.0** (verified) means the
  licence is not the obstacle; the C++ build is.
* **Never** `kornia-rs` (unclassifiable licence), `akaze` or `cv-core`
  (abandoned 2020).

## H.R8 — Write down the UCG/comp-graph boundary as a rule

**Addresses F7, §B.4.** Add to `docs/MODULE-CONVENTIONS.md`:

> Reactors never evaluate pixels. Per-frame render state never enters the UCG.
> The comp DAG lives in `CompGraph`, not in UCG edges; the UCG holds the graph
> as data and a dirty flag, nothing more.

The code already complies. Writing it down is what prevents the next
contributor from "unifying" the two dependency systems, which would put a
pixel-rate workload behind a non-reentrant `RwLock`.

## H.R9 — Add the two latent-bug guards now, while they are free

* **Thread `frame` into `CompGraph::hashes()`** before keyframes exist
  (§F.4), or the first animated parameter will serve cached pixels from the
  wrong frame.
* **Add a CPU-reference rule for GPU comp nodes** to
  `docs/MODULE-CONVENTIONS.md` (§E.5), matching the discipline `cine-gpu`
  already follows, before the first GPU-only node is written.

## H.R10 — Resolve the wgpu version gap before any GPU comp work

`cine-gpu` pins `wgpu = "26"`; current is **30.0.1** and
`11-shell-and-gpu-runtime.md` is written against 30.0.1. Not a comp problem, but
it blocks every comp item that reaches the GPU.

## What NOT to do

* **Do not host OFX yet.** The licence is clear (BSD-3-Clause) and the door is
  open, but a plug-in host is only valuable once the *image substrate* it exposes
  to plug-ins is real. Hosting OFX on an engine with no data window would mean
  implementing the OFX region-of-definition suite against an engine that cannot
  honour it.
* **Do not add node kinds.** 28 exist; the deficit is in the substrate, not the
  vocabulary. Every new kind is another `meta()` function to write later.
* **Do not implement deep EXR I/O.** No pure-Rust path exists; fix the deep
  *maths* instead (H.R3.3).
* **Do not take a C++ dependency for EXR.** The Rust crate is better maintained
  than the bindings to the C++ library.

---

# PART I — UNVERIFIED REGISTER

Per directive §60: named rather than guessed. Ordered by how much a wrong answer
would cost.

| # | Claim | Why it matters | What would settle it |
|---|---|---|---|
| **H-1** | ~~`exr` crate deep/tiled/compression support~~ — **RESOLVED in this session.** Tiles ✅, deep ❌, all lossless codecs ✅ | Decided §E.4 | — (kept for traceability) |
| **H-2** | **`exr`'s DWAA/DWAB support is contradictory in its own README** — the checklist says `[x] dwaa, dwab`, the prose says *"deep data and DWA compression algorithms are not yet supported"* | DWAA is the common delivery codec for lossy EXR; claiming support that does not exist would produce unreadable deliverables | Round-trip a DWAA-compressed EXR against a reference and compare. **Do not claim DWA support until this is done** |
| **H-3** | **Blender's exact GPL version** — `blender.org/about/license/` returned **403**, `raw.githubusercontent.com/blender/blender/main/LICENSE` and `projects.blender.org/.../LICENSE` returned **404** in this session | Low: `01-LICENSING-SPINE.md` §3 already treats Blender as unlinkable GPL and that conclusion is version-independent | Fetch from a reachable mirror of the Blender source tree |
| **H-4** | **`opencv_contrib` non-free / patented modules** (the SIFT/SURF question). OpenCV core is verified **Apache-2.0**; contrib is a separate repository with a separate situation | Would decide whether a TYPE B OpenCV path is usable at all, if H.R7 ever escalates | Fetch `github.com/opencv/opencv_contrib` LICENSE and the `xfeatures2d` module notes |
| **H-5** | **`kornia-rs` licence** — crates.io reports the literal string `non-standard` | Currently classified **H** on two independent grounds (licence + staleness), so a wrong answer costs nothing today | Read `LICENSE` in `github.com/kornia/kornia-rs` |
| **H-6** | **Nuke NDK method names and call order** (`validate()` / `_request()` / `engine()`) — quoted from memory in §B.2, **not fetched** | The *architectural idea* (two-phase metadata-then-pixels) is independently observable and is what H.R2 rests on; only the API vocabulary is unverified. Do not cite these names as Foundry's | Fetch `learn.foundry.com/nuke/developers/latest/ndkdevguide/` |
| **H-7** | **Nuke's filter list** (Impulse, Cubic, Keys, Simon, Rifman, Mitchell, Parzen, Notch, Lanczos, Sinc) quoted in §E.2 | Illustrative only; the negative-lobe/HDR-ringing argument stands on the maths, not on Foundry's menu | Fetch Foundry's Transform node documentation |
| **H-8** | **Fusion's internal caching and DoD implementation** | §B.3 deliberately claims only what is user-documented and observable. Nothing in H.R1–R2 depends on Fusion | Blackmagic's Fusion manual; likely unresolvable at architectural depth, as the product is closed |
| **H-9** | **Nuke NDK plug-in licence terms** and whether writing a Nuke-compatible anything creates obligations | Only matters if CINE-OMNI ever ships a Nuke plug-in or reads `.nk` scripts (a TYPE E option, not recommended now) | Foundry's NDK EULA |
| **H-10** | **Production keyer algorithms** — IBK, Primatte, Keylight: which are patented, which are published | Would decide whether an advanced keyer is TYPE G-able. `cine-comp`'s current chroma/luma/difference keyers are classical and unencumbered; nothing today depends on this | Primatte's patents; Foundry's IBK and Keylight documentation |
| **H-11** | **`optical-flow-lk`, `rustflow`, `purecv` licences** — not fetched | All classified **H** on download counts alone (5 104 / 1 693 / 2 046 lifetime); licence is moot | crates.io API |
| **H-12** | **The transitive dependency licences of `exr`** | Before H.R5 ships, `cargo-deny` must pass. `01-LICENSING-SPINE.md` §6 item 5 already requires this in CI and it does not yet exist | Wire `cargo-deny` into CI — this is the standing obligation, not a research task |
| **H-13** | **OIIO's 0.14 % non-Apache-2.0 residue** — the README states *"over 99.86% of lines of code have been relicensed to Apache-2.0"*, implying a BSD-3 remainder | Only matters if source is actually ported under the §G bonus. BSD-3 is Apache-2.0-compatible, so this is an attribution question, not a blocker | Identify the remaining files before porting any OIIO source |

## Things this document deliberately does not claim

* That `cine-comp`'s **flat** compositing is wrong. Its Porter–Duff maths,
  premultiplied convention and alpha-relative invert are correct, and 65 tests
  pass. The defects named are specific and measured.
* That Nuke or Fusion work internally the way §B.2/§B.3 describe *in
  implementation*. What is claimed is the observable behaviour and the
  architectural pattern, with the vocabulary flagged as **H-6**.
* Any performance number that was not measured on this machine on 2026-09-08
  with the commands in §0 and §F.3.

---

*Document status: research complete for the items above. Sections resting on
**H-2** and **H-6** are flagged inline. Written 2026-09-08.*
