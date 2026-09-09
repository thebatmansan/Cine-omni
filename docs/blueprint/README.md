# CINE-OMNI Industry System Blueprint

**Phase 0 — research and architecture: partially complete.**
**Phase 1 — core platform hardening: complete.** See
[`05-PHASE-1-RESULTS.md`](05-PHASE-1-RESULTS.md).

This directory is the deliverable required by directive §62 and §81. It exists
to turn an intentionally enormous ambition into architecture, phases and
evidence, without shrinking the ambition and without pretending to certainty
that has not been earned.

## Status of every Phase 0 deliverable

| # | Deliverable | Status | Where |
|---|---|---|---|
| 0 | Safety checkpoint before any change (§43) | **Done** | commit `7a82b41`, 179 files |
| 1 | Repository archaeology, measured not asserted | **Done** | [`00-REPOSITORY-ARCHAEOLOGY.md`](00-REPOSITORY-ARCHAEOLOGY.md) |
| 2 | Scale benchmarks, committed and re-runnable | **Done** | `crates/{ucg-core,cine-pipeline}/tests/scale.rs` |
| 3 | Licensing spine and its architectural consequence | **Done** | [`01-LICENSING-SPINE.md`](01-LICENSING-SPINE.md) |
| 4 | 3D / DCC ecosystem research | **Partial** — Blender complete and deep; OpenUSD, OpenSubdiv, MaterialX, Alembic not reached | [`research/01-3d-dcc.md`](research/01-3d-dcc.md) |
| 5 | Editorial / media ecosystem research | **Done** — FFmpeg licensing, MLT, Kdenlive, OTIO, OIIO, hardware decode, proxy architecture | [`research/02-editorial-media.md`](research/02-editorial-media.md) |
| 6 | Compositing / imaging research | **Done** — node-graph architecture, deep, cryptomatte, EXR/OIIO, premult rules, Rust crate reality | [`research/12-compositing.md`](research/12-compositing.md) |
| 7 | Color science research | **Not started** — blocked | — |
| 8 | Audio research | **Done** — `cine-audio` assessed and measured; plugin formats, backends, loudness, immersive delivery and RT/UCG discipline decided. **Overturns the VST3 finding in §01** | [`research/14-audio-architecture.md`](research/14-audio-architecture.md) |
| 9 | Production / collaboration research | **Done** — on-set/script supervision, dailies/DIT verification, ASC MHL verified against the spec, and a collaboration model for the existing op log | [`research/15-production-dailies-collaboration.md`](research/15-production-dailies-collaboration.md) |
| 10 | GPU / runtime / UI shell research | **Done** — shell decided (native Rust: winit + wgpu + egui), GPU runtime specified | [`research/11-shell-and-gpu-runtime.md`](research/11-shell-and-gpu-runtime.md) |
| 11 | Feature matrix (§46) | **Blocked** on 6–10 | — |
| 12 | Full license matrix (§42) | **Blocked** on 6–10 | — |
| 13 | Full integration matrix (§41) | **Blocked** on 6–10 | — |
| 14 | Target architecture | **Directional only** | [`02-TARGET-ARCHITECTURE.md`](02-TARGET-ARCHITECTURE.md) |
| 15 | Phased implementation plan | **Phase 1 fully specified; later phases outlined** | [`03-PHASED-PLAN.md`](03-PHASED-PLAN.md) |
| 16 | Risk register | **Done** | [`04-RISK-REGISTER.md`](04-RISK-REGISTER.md) |
| 17 | **Phase 1 hardening (A–E)** | **Done** | [`05-PHASE-1-RESULTS.md`](05-PHASE-1-RESULTS.md) |

**Why items 6–10 are blocked:** the parallel research agents hit the account's
model rate limit mid-run. Two of seven clusters completed and are on disk. The
rest were not written, and **nothing has been fabricated to fill the gap** —
per directive §60, an unverified license or an unmeasured claim is worse than an
acknowledged hole. These resume as soon as the limit resets.

## The three findings that matter most so far

**1. The transaction core scales; the cascade above it does not.**
Commit latency is flat at ~13 µs from 2,801 to 112,001 nodes. But one dialogue
edit costs 2.15 ms at 7 scenes and 144 ms at 1,000 scenes — a 67× growth for an
O(1) user action, because the logistics reactor reschedules the entire film on
any scene change. The foundation is worth keeping. The reactive layer needs to
become incremental. Full evidence in the archaeology document.

**2. CINE-OMNI is Apache-2.0, and that single fact decides the ecosystem
strategy.** Blender, Kdenlive, Natron and Ardour are GPL. Their source can never
be linked or copied into an Apache-2.0 product. They are only available as
separate-process engines, protocol bridges, file interchange, or as designs to
study and reimplement independently. This is not a limitation to work around; it
is the boundary that shapes the architecture.

**3. A 74× performance defect was found by measurement, not inspection.**
`Graph::of_kind()` is 74× slower than having no index at all, because
`sort_by_key` re-evaluates its key on every comparison. Reactors call it on
every cascade. The fix is small and the baseline to verify it is committed.

## Reading order

1. [`00-REPOSITORY-ARCHAEOLOGY.md`](00-REPOSITORY-ARCHAEOLOGY.md) — what exists, measured
2. [`01-LICENSING-SPINE.md`](01-LICENSING-SPINE.md) — the constraint that governs integration
3. [`02-TARGET-ARCHITECTURE.md`](02-TARGET-ARCHITECTURE.md) — where it needs to go
4. [`03-PHASED-PLAN.md`](03-PHASED-PLAN.md) — how to get there
5. [`04-RISK-REGISTER.md`](04-RISK-REGISTER.md) — what could go wrong
6. `research/` — the ecosystem evidence
   - [`research/01-3d-dcc.md`](research/01-3d-dcc.md) — 3D / DCC
   - [`research/02-editorial-media.md`](research/02-editorial-media.md) — editorial / media engine
   - [`research/08-media-codec-stack.md`](research/08-media-codec-stack.md) — **Phase 2**: codec /
     container / decoding stack, API boundary, and a measured pure-Rust slice
   - [`research/12-compositing.md`](research/12-compositing.md) — professional
     compositing: the render dependency graph vs the UCG cascade, deep data,
     Cryptomatte (verified spec + test vectors), the EXR decision, and a frank
     measured assessment of `crates/cine-comp`
   - [`research/14-audio-architecture.md`](research/14-audio-architecture.md) — professional
     audio: the `cine-audio`/engine boundary, plugin-format licensing (**VST3 is now
     MIT; ASIO is now GPLv3**), real-time thread discipline against the UCG,
     loudness compliance, and immersive/ADM delivery

## Phase 2 inputs

| Document | Subject |
|---|---|
| [`research/09-editorial-media-model.md`](research/09-editorial-media-model.md) | How professional NLEs model media identity, time, conform and derived artefacts (AAF/UMID, FCPXML, Resolve, Premiere, MLT, OTIO) — and the proposed UCG node/edge mapping for media identity. Includes measured evidence that `RationalTime`'s `f64` representation puts 13.2 % of 23.976 frame indices on the wrong frame after a single rate rescale. |

## Standing rules for this blueprint

- Never claim a license from memory. Cite the source that was read.
- Never call an approximation by the name of the standard it approximates.
- Never report a performance characteristic that was not measured.
- Mark unknowns `UNVERIFIED` or `PENDING` rather than filling them plausibly.
- Preserve working systems; the checkpoint exists so that rule can be enforced.


## Phase 3

- [`APPLICATION-RUNTIME.md`](APPLICATION-RUNTIME.md) — `cine start`: the one
  command, the shared-runtime architecture, what a second terminal does, and
  the lifecycle guarantees.
- [`EXACT-TIME-MIGRATION.md`](EXACT-TIME-MIGRATION.md) — the float-time defect
  measured, the canonical exact type, and the staged migration.

- [`PHASE-3-STATUS.md`](PHASE-3-STATUS.md) — **start here.** Status vocabulary,
  the capability / research / licence / decision matrices, the defects fixed and
  the defects still open, and what may and may not be claimed today.

## Phase 2

- [`PHASE-2-MEDIA-ENGINE.md`](PHASE-2-MEDIA-ENGINE.md) — the media engine:
  identity, exact time, decode architecture, capability matrix, licence audit,
  measured performance and the unresolved decisions. Read this before touching
  `crates/cine-media`.
