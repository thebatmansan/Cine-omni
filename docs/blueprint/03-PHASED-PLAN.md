# 03 — Phased Implementation Plan

Directive §69 asks for phases and §70 insists the vision must not shrink to fit
them. The sequence below is derived from the measured archaeology, not from the
directive's suggested ordering — several of its phases are gated by defects that
must be fixed first, and one phase (media) turns out to block five others.

**No implementation has begun.** Phase 1 is specified to the level where it
could start immediately; later phases are outlined and will firm up as the
blocked research completes.

---

## Phase 0 — Research, architecture, legal inventory

**Partially complete.** See [`README.md`](README.md) for the status table.

Done: checkpoint, archaeology with committed benchmarks, licensing spine,
editorial/media research, Blender research.
Outstanding: five research clusters, the three matrices, the shell decision.

---

## Phase 1 — Core platform strengthening

**COMPLETE.** Measured results in [`05-PHASE-1-RESULTS.md`](05-PHASE-1-RESULTS.md).
Headline: `of_kind` 10.6–14.5× faster; schedule-neutral edits went from 59 ms to
0.34 ms at 1,000 scenes and are now flat; the silent-deadlock failure mode is
now a diagnosable error; the in-memory op log is bounded without weakening
durability; `cargo deny` passes all four checks with no GPL in the graph.

The whole phase is confined to `ucg-core` and `cine-pipeline`. Nothing here
depends on Blender, OpenUSD, the shell, or any external engine.

### 1.1 Fix the pathological sorts — *hours, not days*

`Graph::of_kind()` is 74× slower than having no index. `children()` shares the
defect. `sort_by_key` re-evaluates its key on every comparison, hashing a UUID
each time.

- Replace with `sort_by_cached_key`, or store insertion order in the index.
- **Acceptance:** `of_kind()` on 8,000 nodes drops from 74 ms to under 1 ms,
  verified by the committed benchmark. All 424 tests still pass.

### 1.2 Bound the in-memory op log — *small*

`Engine::log` retains every delta for the process lifetime.

- Keep a bounded window in memory; the rest already persists to disk.
- **Acceptance:** memory stays flat across a 100,000-edit session.

### 1.3 Make the cascade incremental — *the substantial work of this phase*

The measured defect: one dialogue edit costs 2.15 ms at 7 scenes and 144 ms at
1,000, because reactors re-derive whole aggregates.

- Give reactors declared subscriptions instead of a whole `ChangeSet`.
- Convert the schedule, breakdown and call sheets from recomputed aggregates to
  incrementally maintained ones.
- Scope invalidation to affected shooting days rather than the film.
- **Acceptance:** edit latency is **flat** from 7 to 2,000 scenes and stays
  under 16 ms. The thesis test and all seven reactors still behave identically;
  grouped one-step undo is preserved exactly (§25).

### 1.4 Cheap consistent reads — *substantial*

`snapshot()` is a 363 ms full clone at 56k nodes.

- Copy-on-write generations or persistent structures; background workers take a
  version handle.
- **Acceptance:** obtaining a consistent read is O(1) and under 1 ms at 112k
  nodes. Existing GPU and comp evaluation paths keep working.

### 1.5 License governance in CI — *small, and overdue*

- `cargo-deny` wired into CI with an allowlist; the third-party inventory
  required by §42 generated rather than hand-maintained.
- **Acceptance:** a dependency with an incompatible license fails the build.

### 1.6 Bulk-data boundary — *design in this phase, implement in Phase 2*

Specify the columnar side store for time series and per-frame data, keyed by
entity ID, with the graph holding only summary and reference.

---

## Phase 2 — Professional media foundation

**Blocks Phase 3, 5, 6, 7 and 8. This is the critical path of the entire
product.** Editorial, VFX plates, color, audio and dailies all need to open a
file, and today nothing can.

- LGPL-only FFmpeg build, dynamically linked, behind a Rust adapter crate,
  honouring LGPL-2.1 §6 (see [`01-LICENSING-SPINE.md`](01-LICENSING-SPINE.md)).
- Hardware decode/encode abstraction across VAAPI, NVDEC/NVENC, VideoToolbox,
  D3D11VA, with a documented software fallback.
- Image I/O: EXR (multi-part, deep, tiled), DPX, TIFF, camera RAW.
- Proxy media, render cache and background rendering, as specified in the
  editorial research.
- Timecode, variable frame rate and media indexing.
- **Source media is sacred (§56):** ingest never modifies originals, and every
  derived representation is traceable to its source.

---

## Phase 3 — Professional editorial

Depends on Phase 2. The data model already exists and is good; what is missing
is a clock, a decoder and an audio device. Multicam, nested sequences, time
remapping, trim modes, three-point editing, match frame, relink and conform. The
OTIO adapter ecosystem (AAF, EDL, FCP XML) is the interoperability prize.

## Phase 4 — Professional 3D

Gated on the OpenUSD decision. The Blender research argues the durable value is
architectural ideas rather than a bridge; a headless TYPE C engine remains an
option for rendering and simulation specifically.

## Phase 5 — VFX and compositing

Gated on the OFX hosting decision, which if taken grants the commercial plug-in
ecosystem. Otherwise: tracking, optical flow, denoise, paint, Cryptomatte.

## Phase 6 — Professional color

Gated on the OpenColorIO decision. Until then CINE-OMNI must not claim ACES
conformance (§60). The maths is strong; the output transform is an approximation
and is labelled as such.

## Phase 7 — Professional audio

Needs device I/O and a plug-in standard. The real-time audio thread versus the
`RwLock`-guarded graph is a genuine architectural conflict that Phase 1.4 makes
tractable.

## Phase 8 — Production, on-set, dailies

Offload with checksum verification (ASC MHL), proxy and dailies transcode, sound
sync, circle takes, camera and sound reports.

## Phases 9–14

Asset and production management; collaboration and enterprise; interoperability;
performance hardening; professional UX refinement; production validation. These
firm up once the blocked research lands.

---

## The critical path, stated plainly

```
Phase 1 (core)  →  Phase 2 (media)  →  everything else
     ↑                    ↑
  measured            blocks five
  defects             other phases
```

Two things are true at once: **Phase 1 can start today** with no dependency on
any pending decision, and **Phase 2 is the gate** through which most of the
product's remaining value has to pass.

## The final test (§71)

Could a filmmaker take a film from screenplay to delivery without leaving
CINE-OMNI? **No, and the specific blocker is Phase 2.** Without media I/O there
is no dailies, no editorial, no plates, no grade, no mix. Every other gap is
downstream of that one.
