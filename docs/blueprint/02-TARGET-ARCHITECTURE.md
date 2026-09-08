# 02 — Target Architecture

**Status: directional.** The moves below are justified by measured evidence or
by settled licensing constraints. Decisions that depend on the blocked research
clusters are marked **PENDING** and deliberately left open rather than guessed.

---

## 1. The one principle that does not change

CINE-OMNI owns **identity, relationships, provenance, history and change
propagation**. Everything else is negotiable — engines, formats, renderers,
even whole workspaces. Directive §74 states it exactly: *the specialized
disciplines may have specialized engines; CINE-OMNI owns the relationships
between them.*

Every architectural decision below is tested against one question: **does this
strengthen or weaken the graph's ownership of identity?**

---

## 2. Layer model

```
┌──────────────────────────────────────────────────────────────────┐
│  WORKSPACES        script · story · board · design · assets ·    │
│  (radically        3D · sculpt · rig · anim · layout · previs ·  │
│   different UIs,   cine · production · dailies · editorial ·     │
│   one project)     vfx · comp · color · audio · review · deliver │
├──────────────────────────────────────────────────────────────────┤
│  SHELL             docking · multi-monitor · GPU surfaces ·      │
│                    command palette · keymap        ← PENDING     │
├──────────────────────────────────────────────────────────────────┤
│  ORCHESTRATION     incremental reactors · job scheduler ·        │
│  (cine-pipeline)   staleness model · cache invalidation          │
├──────────────────────────────────────────────────────────────────┤
│  UNIFIED CONTEXT GRAPH        identity · relationships ·         │
│  (ucg-core)                   transactions · undo · provenance   │
├───────────────┬──────────────┬───────────────┬───────────────────┤
│ DOMAIN CRATES │ SIDE STORES  │ MEDIA ENGINE  │ EXTERNAL ENGINES  │
│ script previs │ time series  │ decode/encode │ Blender (TYPE C)  │
│ logistics nle │ per-frame md │ proxies cache │ renderers         │
│ comp color    │ tracking     │  ← NEW LAYER  │  ← process bdry   │
│ audio         │ columnar     │               │  GPL-safe         │
├───────────────┴──────────────┴───────────────┴───────────────────┤
│  GPU (cine-gpu)    frame graph · kernels · viewport surfaces     │
└──────────────────────────────────────────────────────────────────┘
```

Three of these are new or substantially changed: the **media engine** (does not
exist at all today), the **side stores** (a boundary the current design does not
draw), and the **shell** (today a browser tab).

---

## 3. Decided moves

### 3.1 Keep the transaction core exactly as it is

Measured flat at ~13 µs from 2,801 to 112,001 nodes. Deterministic derived IDs,
invertible ops, grouped undo and op-log replay are all correct and all fast.
Directive §25 and §43 both demand they be preserved. **No change.**

### 3.2 Make reactors incremental — the central piece of work

Today a reactor re-derives from scratch: the logistics reactor reschedules the
entire film on any scene change, which is why one dialogue edit grows from
2.15 ms to 144 ms between 7 and 1,000 scenes.

The target is that a reactor **declares what it depends on** and is handed only
what changed:

```rust
// today: "something touched a scene, recompute the world"
fn react(&self, cs: &ChangeSet, g: &Graph) -> Result<Vec<Plan>>

// target: a standing subscription with scoped invalidation
fn subscriptions(&self) -> Vec<Subscription>;   // e.g. attr(SCENE, PAGE_EIGHTHS)
fn react(&self, delta: &ScopedDelta, view: &View) -> Result<Vec<Plan>>
```

Aggregates such as the schedule become **incrementally maintained values in the
graph** rather than recomputed ones: changing one scene's page count re-packs the
affected shooting days, not all of them. This is the difference between a system
that demos at 7 scenes and one that works on a feature.

This gates the entire vision. Directive §26 asks the system to know continuously
what is current, stale, processing or failed across twenty domains — unreachable
if every domain recomputes globally on each keystroke.

### 3.3 Replace `snapshot()` with cheap consistent reads

A 363 ms full clone at 56k nodes cannot back interactive preview. Target:
copy-on-write generations or persistent data structures, so a background worker
takes a cheap version handle. This also unlocks splitting the global write lock.

### 3.4 Draw the bulk-data boundary

The graph holds identity and relationships. **Time series and per-frame data move
to columnar side stores keyed by entity ID**: 240 Hz camera metadata, tracking
curves, per-frame QC. At ~2 KB/node, "millions of records" as graph nodes would
be ~2 GB before any media. The graph keeps the summary and the reference.

### 3.5 A media engine is a new foundational layer

There is no decode, encode or image I/O today. This is the largest single
capability gap in the product and it blocks editorial, dailies, VFX plates,
color and audio simultaneously. Research recommends an **LGPL-only FFmpeg build,
dynamically linked** behind a Rust adapter, which is compatible with Apache-2.0
subject to LGPL-2.1 §6 obligations. Hardware decode abstraction and the
proxy/cache architecture are specified in the editorial research.

### 3.6 GPL engines live behind a process boundary, or not at all

Settled by §01. Blender can be a headless TYPE C engine speaking USD over a
process boundary, or a source of architectural ideas (TYPE G). It cannot be
linked. The research argues persuasively that the durable value is the ideas —
particularly its depsgraph design, which is the mature version of the problem
CINE-OMNI is solving in §3.2.

### 3.7 Crate structure

Directive §77 proposes splitting into ~16 domain crates. **Evaluated: adopt
partially, and later.** The current boundaries are already correct in kind
(domain crates depend only on `ucg-core` + `ucg-schema`; only `cine-pipeline`
knows everything). Splitting further before the reactor model is incremental
would multiply the surface that has to change. Recommended order: fix the core,
then split `cine-media`, `cine-editorial` and `cine-3d` out as they grow.

---

## 4. Pending decisions

| Decision | Blocked on | Why it matters |
|---|---|---|
| **Application shell** | GPU/runtime/UI research | The current React-in-a-browser shell cannot deliver docking, multi-monitor, or a GPU viewport surface, and violates §39. This is the highest-value open question. |
| OpenUSD adoption (§34) | 3D research (not reached) | Determines whether previs interoperates with real pipelines or stays a private format |
| OpenColorIO adoption (§36) | color research | Determines whether CINE-OMNI can ever claim color correctness honestly |
| OFX plug-in hosting | compositing research | Would grant the entire commercial VFX plug-in ecosystem |
| CLAP vs LV2 vs VST3 audio hosting | audio research | **Resolved**: the VST 3 SDK is MIT as of VST 3.8 (2025-10-29), so all three are hostable. ASIO is now the GPLv3 trap instead. See `PHASE-3-STATUS.md` §5 |
| CRDT vs op-log for collaboration | production research | Decides whether §37 multi-user is reachable without a rewrite |

---

## 5. What this architecture explicitly refuses

- **Blender in a tab.** Legally impossible under Apache-2.0 and architecturally
  wrong: it would put a second identity system inside the product.
- **A graph that stores everything.** Per-frame data belongs in side stores.
- **Reactors that recompute the world.** Measured, and it does not scale.
- **A dashboard.** Directive §4. The shell decision must serve dense,
  keyboard-driven, discipline-specific workspaces or it is the wrong shell.
