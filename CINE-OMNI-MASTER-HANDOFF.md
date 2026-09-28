# CINE-OMNI — MASTER CONTINUITY HANDOFF

> **Purpose.** One self-contained briefing that lets a new AI or engineer continue
> CINE-OMNI from the exact current state without this conversation, prior memory,
> or undocumented assumptions.
>
> **Produced** 2026-09-28 by reading the actual repository at
> `~/SANBAT/Sanbat-F&me/SanbatF&me`. Every claim was verified against code, tests,
> git history or measurement. Where an older document disagrees with the code,
> **the code wins**, and the discrepancy is recorded in PART 18.
>
> **This file supersedes `CHATGPT_CONTEXT.md`**, which is a stale handoff from an
> earlier era (it claims "424 tests pass"; the real figure is 1,166).

---

## TABLE OF CONTENTS

| Part | Subject |
|---|---|
| 1 | Exact current project state |
| 2 | The CINE-OMNI vision |
| 3 | Product requirements (preserved) |
| 4 | Implementation history — major milestones |
| 5 | Current technical architecture |
| 6 | UCG / data model, and Subject × Lens |
| 7 | User-facing product / UI audit |
| 8 | Media system |
| 9 | Time / playback / audio / rendering |
| 10 | Colour system (critical) |
| 11 | Testing / verification |
| 12 | Licensing / external integration |
| 13 | Architectural gaps |
| 14 | Product completeness |
| 15 | Storage / build state |
| 16 | Current Colour / working-tree status |
| 17 | Current exact checkpoint |
| 18 | Discrepancy audit |
| 19 | Development workflow for the next AI |
| 20 | Non-negotiable rules |
| 21 | Technical inventory (read order) |
| 22 | Final continuity summary |
| — | Continuation command |

---

## PART 1 — EXACT CURRENT PROJECT STATE

| Fact | Value |
|---|---|
| Current branch | `playback-scheduler` |
| Current HEAD | `5fa7175f41bb72a0bb054391e52e6b3b1f87731c` |
| `master` HEAD | `971f14aeab5d27e7476513b89c52ca4c4801d15c` |
| Remote | `origin` → `git@github.com:thebatmansan/Cine-omni.git` (public) |
| Push status | **Both branches pushed.** `playback-scheduler` tracks `origin/playback-scheduler`; local == remote |
| Total commits | 132 on branch, 19 on master, 113 unique to branch |
| Tracked files | 467 (8.7 MB) |
| Staged | **Nothing** |
| Working tree | **Dirty** — 8 modified + 2 untracked (Colour WIP) |

**Verification of previously asserted context:**

- "relationship-navigation completed at `5fa7175`" — **TRUE**, HEAD is exactly that.
- "master was `971f14a`" — **TRUE**, unchanged.
- "nothing had previously been pushed" — **NO LONGER TRUE.** Both branches were
  pushed to GitHub on 2026-09-24 under explicit authorization. No force push was
  used (the remote was empty).
- "Colour was the next intended slice" — **TRUE**.
- "Colour work may be uncommitted / in progress" — **TRUE**, see PART 16.

> ⚠️ The repository has **no `main` branch**. Any tooling that assumes `main` is
> wrong; the trunk is `master`. GitHub's default branch is currently `master`
> (`971f14a`), which is **113 commits behind** the actual work.

**Is the current state safe to call stable?** The *committed* state at `5fa7175`
is stable and fully gated. The *working tree* is **not** — three tests fail.
These are separable: the failures are confined to new uncommitted files.

---

## PART 2 — THE CINE-OMNI VISION

One unified professional filmmaking environment covering the whole cinema
lifecycle, replacing a chain of tools that cannot talk to each other.

**The intended lifecycle** (from `CINE-OMNI-CAPABILITY-MATRIX.md`, 169 rows, and
`PRODUCT-STRATEGY.md`):

- **Development / IP** — screenplay, writing, research, story knowledge,
  characters, locations, organizations, rights, options, chain of title, IP
  records, legal references, budgeting, financial models, development tracking.
- **Pre-production** — script breakdown, scene and shot breakdown, storyboard
  creation and sequencing, concept development, 2D drawing, 3D blockout, camera
  placement, lens selection, lighting design, techvis, previs, pitchvis,
  location and set planning, scheduling, stripboards, cast and crew planning,
  production design, costumes, assets.
- **Virtual production / VAD** — environment creation, 3D assets, digital sets,
  materials, textures, procedural generation, sculpting, rigging, animation,
  virtual cameras, real-time lighting, LED volume / ICVFX, scene assembly, asset
  versioning and dependencies, production tracking, USD-compatible architecture.
- **Principal photography** — camera media ingest, camera and lens metadata,
  camera reports, slate and take information, timecode, frame rate, shutter,
  exposure and colour metadata, CDL / look metadata, checksum and offload
  verification, cards, camera and audio rolls, sync, dailies, proxies, LUT
  management, production reports, continuity, script supervisor notes, live
  monitoring, on-set review, media health, data lineage.
- **Editorial** — media management, bins, metadata, source/record, timeline,
  multicam, sync, selects, stringouts, subclips, markers, notes, script-based
  editing, transcript, versioning, trim/ripple/roll/slip/slide, overwrite,
  insert, replace, transitions, effects, titles, captions, nested sequences,
  conform, EDL, OTIO, AAF/XML where possible, offline/online, proxies,
  relinking, media identity, finishing preparation.
- **VFX / CGI / animation** — compositing, node graphs, layers, masks, roto,
  paint, tracking, matchmove, camera solve, keying, cleanup, particles, fluids,
  destruction, procedural FX, animation, rigging, lighting, shading, rendering,
  render management, versioning, shot and asset dependencies, VFX review, shot
  status, plates, mattes, passes, EXR workflows, colour-managed compositing.
- **Colour / finishing** — colour management, scene-referred and display-referred
  workflows, SDR, HDR, PQ, HLG, ACES, OCIO, CDL, curves, wheels, primaries,
  secondaries, qualifiers, windows, tracking, keys, masks, LUT handling, shot
  matching, stills, versioned grades, conform, online finishing, mastering,
  scopes, reference monitoring, output transforms, deliverable validation.
- **Audio** — dialogue editing, sound design, Foley, ADR, music, score, editing,
  mixing, automation, routing, buses, stems, track formats, surround, immersive
  audio, restoration, denoise, de-click, EQ, dynamics, reverb, spatial
  positioning, synchronization, sample accuracy, loudness, monitoring, final
  mix, composition, MIDI, virtual instruments, plugins.
- **Localization / QC / delivery / archive** — subtitles, captions, forced
  narratives, multilingual versions, dubbing, ADR localization, subtitle timing,
  QC, automated validation, broadcast compliance, theatrical mastering, DCP,
  IMF, streaming and broadcast masters, mezzanine files, metadata, checksums,
  packaging, manifests, archive, restoration, version tracking.

**The central philosophy:**

> ONE APPLICATION · ONE PROJECT · ONE PERSISTENT CONTEXT · ONE UNIFIED CONTEXT
> GRAPH · ONE MEDIA FOUNDATION · ONE EXACT TIME FOUNDATION · ONE RUNTIME ·
> MULTIPLE PROFESSIONAL LENSES

The differentiator is **not** feature breadth. It is that a script scene, its
shots, takes, media, editorial use, VFX, grade, audio and deliverable are *one
connected graph*, so changes propagate and context is never lost.

`README.md`: *"Every scene, line of dialogue, camera, clip, comp node, grade and
audio clip is one node with one UUID. Dependency edges form a DAG; a change
anywhere is propagated to every dependent module by pure, parallel reactors, and
the whole cascade is one undo step."*

It must feel like serious professional creative software — **not** an enterprise
dashboard, not a collection of disconnected mini-apps, not a shallow checklist,
and not a fake UI over missing functionality.

---

## PART 3 — PRODUCT REQUIREMENTS (preserved)

- Full filmmaking lifecycle; commercial, industry-grade quality.
- **Backend and frontend advance together.** A backend capability is not
  finished until there is a path: protocol → frontend state → visible interface
  → interaction → test.
- No fake functionality, no fake controls, no status-only implementations, no
  inflated capability claims.
- One persistent project context; **UCG remains authoritative**; no duplicate
  state model in React.
- Exact time correctness; exact media correctness; dependency-aware propagation.
- One command: `cine start`.
- Professional creative-software UX: information-dense where appropriate,
  keyboard-friendly, no SaaS dashboard design, no giant cards, no decorative
  gradients or empty space consuming working area.
- **Phases are dependency ordering, NOT user approval gates.** Autonomous
  continuous development is wanted — do not ask for approval between normal
  slices — but do not jump ahead while a slice is incomplete. Verify before
  moving on.
- Honest capability boundaries; licensing constraints preserved.
- No unauthorized push/deploy; **master protected unless explicitly authorized**.
- Docker is **not** the deployment direction.
- No blind storage cleanup.
- **Never two `cargo` processes against one `target/`** — this previously
  corrupted the incremental cache into 20 GB of unusable sessions and
  `undefined hidden symbol` link errors.

---

## PART 4 — IMPLEMENTATION HISTORY (major milestones, verified)

### Foundation — 2026-09-08/09

| Commit | What was built | Why it mattered | Status |
|---|---|---|---|
| `7a82b41` … `8f46996` | Repository archaeology, licensing spine, target architecture | Established the legal and architectural spine before code | Documents, VERIFIED as documents |
| `878786d` | Phase 1 core platform hardening | — | Superseded |
| `6b53ec2` | Phase 2 media engine vertical slice on real media | First real decode path | VERIFIED |
| `e6be610` / `0378546` | Capability registry, CI gates; stopped data loss and false verification | **Origin of the honesty discipline** | VERIFIED |
| `72c1a98` | `cine-shell` — real window presenting real decoded frames | Native surface exists | VERIFIED |
| `165e29a` | **`cine start`** — one command, one runtime, one project | The product's single entry point | VERIFIED |
| `ec464c0` / `ccad1a6` / `971f14a` | **Exact time**: `RationalTime` becomes the model; cine-media's parallel time model deleted | One time model, no epsilon, no rate ratios | VERIFIED |
| `ce46f59` | Crash recovery, process killed for real | Op-log replay proven, not assumed | VERIFIED |
| `703e266` | Real presentation clock, not a frame timer | Playback correctness foundation | VERIFIED |
| `3a648ab` | Compiled audio renderer; **a performance claim withdrawn** | Discipline of retracting unproven claims | VERIFIED |
| `15a09a7` | Colour conformance harness; one more silent substitution refused | — | VERIFIED |

### Playback / AV — 2026-09-10/15

| Commit | What was built | Status |
|---|---|---|
| `4e89b06` | Scheduler between the clock and the decoder | VERIFIED |
| `4daa978` | Decode-ahead: playback that presents every picture; bottleneck moved | VERIFIED |
| `ae02491` | Present the planes, not a read-back: **24 fps on screen** | VERIFIED (measured) |
| `ba5a567` | A/V sync: pace the picture from the sound, exactly | VERIFIED |
| `b729331` | Audio reaches hardware: ALSA sink loaded at runtime, measured | VERIFIED |
| `e9c4c25` | The cut's sound: audible clips flattened, resolved, decoded, mixed sample-accurately | VERIFIED |
| `5859ac5` | The native window plays the cut's sound and the picture follows it | VERIFIED |
| `de609f1` / `4586ff7` / `9b290c4` | Storage policy with classes; 30% off the build, measured; memory is the binding constraint | VERIFIED |

### Editorial — 2026-09-15/17

| Commit | What was built | Status |
|---|---|---|
| `4ff5958` / `5722358` | Protocol: edit, link, bind, conform, script, impact, history | VERIFIED |
| `bb0e218` | Editor UI: tools, two monitors, impact, history, script workspace | VERIFIED |
| `f0e7df1` | Play a cut, not a file — one flattened edit list | VERIFIED |
| `83e974d` | Insert, overwrite, replace, join through the graph; linked edits | VERIFIED |
| `921ffa8` | A failed cascade stays undoable; history readable; zero rate refused | VERIFIED |
| `e6c6e5f` | Three- and four-point editing, fourth point derived | VERIFIED |
| `d60ba7d` | Retiming as an exact pair of durations, not a float | VERIFIED |
| `d0ca7b9` | Speed ramps as one exact time map every player reads | IMPLEMENTED |
| `e833c37` / `a3a935b` | Retimed sound as varispeed; keep-pitch phase vocoder with no memory | VERIFIED |
| `3cc5455` / `3443b3f` | Multicam: an angle is one of the clip's own references; sync by sound (measured 0.9999 vs ≤0.43 for strangers) | IMPLEMENTED / VERIFIED |
| `1c16eb2` … `3043720` | Captions: IMSC frame-exact, SRT, WebVTT, SCC — checked by **ttconv** and an **independent SCC reader which found a bug** | VERIFIED |
| `f7cc1ac` / `df83d7f` | Captions authored in the editor, stored in the project, linked to dialogue by a reactor | VERIFIED |
| `9590ee1` | The cut leaves as a CMX3600 EDL | VERIFIED |

### Colour engine — 2026-09-16 (dense, heavily verified)

| Commit | What was built | Status |
|---|---|---|
| `b55e32d` | A processor built on the ACES 2.0 view **says it is an approximation** | VERIFIED |
| `e5690ee` / `53e90d7` | Released ACES 2.0 configs load; display styles and output transforms match OpenColorIO | VERIFIED vs OCIO |
| `84b87fc` | **Every colorspace of the released ACES configs converts as OpenColorIO does** | VERIFIED vs OCIO |
| `e1d1f91` | Luma and strictparsing recognised, so a written config reads back whole | VERIFIED |
| `d4addb3` | **OCIO file rules** decide a file's colorspace, as OpenColorIO decides it | VERIFIED |
| `7935118` | Media gets a colour identity from the project config, with provenance | VERIFIED |
| `8d4baaa` / `0e1af35` | Video seen through a view; the native window shows the project's view | VERIFIED |
| `d0e5990` | **A colour workspace in the browser**, and two stale claims corrected | IMPLEMENTED |
| `e660997` | **A clip can be graded (CDL) in the chain the viewer already applies** | VERIFIED |
| `36c3582` | Scopes of the picture as shown, and a claim corrected | VERIFIED |

### Production / script — 2026-09-16

| Commit | What was built | Status |
|---|---|---|
| `64bba55` / `b2c0203` | Production office: breakdown, stripboard, call sheets, shot lists, takes | VERIFIED |
| `a7b2bcd` | Day Out of Days, with hold days counted as what they cost | VERIFIED |
| `caf2eb0` / `02aa611` | A strip can be moved and the scheduler works around it; the call sheet prints | VERIFIED |
| `091c72b` | The breakdown can be corrected and the correction survives | VERIFIED |
| `272235f` | Script revisions: coloured pages and marks that mean something | VERIFIED |
| `1305275` / `a8317fb` | Rewrite a screenplay in Fountain keeping scene identity and production work; Write mode | VERIFIED |
| `b72511b` | Offload: ASC MHL, and a card copy that proves itself | VERIFIED |
| `6c1d24f` / `954da00` / `7ed0e74` | Compositing: OpenEXR, image sequences, data windows, union bounding boxes | VERIFIED (engine) |

### Jobs / media / delivery — 2026-09-16/19

| Commit | What was built | Status |
|---|---|---|
| `8d0497b` | **Jobs**: prioritised, resource-bounded, cancellable, retried, persisted | VERIFIED |
| `fe8c4ba` / `94169ae` | Checksum verification and thumbnails over the protocol; jobs panel | VERIFIED |
| `08c7024` | Delivery: render the cut to pictures, captions burned in | VERIFIED |
| `84739ec` | **Delivery: render to a movie with its sound**, at a chosen size, and know when it is stale | VERIFIED |
| `61a5f63` | Licensing gate: `cargo deny` passes again, both breaches treated | VERIFIED |
| `e7c9759` | **Proxies** made as jobs from originals, chosen per clip, never mistaken for them | VERIFIED |
| `6eb2cf7` | Media: audit every path's real state; **relink by content rather than by name** | VERIFIED |
| `2d48f52` | Image sequences as media, and the high-bit-depth input they bring | VERIFIED |
| `05e12dd` | Scene-linear view path, EXR delivery, HDR stated exactly | VERIFIED |
| `d1f85fc` | Waveforms from real sound; **one definition of derived state** | VERIFIED |
| `ed4f8cc` | **Import as a job**, and one rule for what makes derived work stale | VERIFIED |
| `e7b75b5` | The import driven through the real application (headless Chrome) | VERIFIED |
| `aad5f01` | **Source timecode**: the camera's numbering, read and written | VERIFIED (no independent reader — see PART 11) |
| `bd4d3b9` | One interpretation of source timecode; conform holds at every rate | VERIFIED |
| `ec42ee0` | **The cut's own clock**: a sequence's record timecode, offered not assumed | VERIFIED |
| `0062592` | **OTIO in the media's own numbering**, converted at the edge of the file | VERIFIED |
| `f8b39d7` | **Matroska described from the file** — removed an invented 48 kHz stereo stream | VERIFIED (cross-checked vs symphonia) |

### Product re-anchor — 2026-09-19/20

| Commit | What was built | Status |
|---|---|---|
| `5627db8` | Master audit re-anchor: the product model was right, the roadmap and surface drifted | Document |
| `a527eb4` | **Media state: one authoritative answer, and the grounds it rests on** | VERIFIED |
| `6fcba90` | Audit records M1, M2 and the locator as closed | Document |
| `5fa7175` | **Subject × Lens: navigation that follows the film, not the department** | VERIFIED ← **current HEAD** |

---

## PART 5 — CURRENT TECHNICAL ARCHITECTURE

**Workspace:** Rust (17 crates) + TypeScript / React / Vite UI. Licence
Apache-2.0. Toolchain via rustup + zig linker on this machine (no system cc, no
sudo).

| Crate | src lines | test files | test lines |
|---|---:|---:|---:|
| `ucg-core` | 4,049 | 3 | 2,035 |
| `ucg-schema` | 631 | 0 | 0 |
| `cine-media` | 12,438 | 19 | 6,480 |
| `cine-color` | 12,582 | 9 | 1,955 |
| `cine-runtime` | 11,197 | 24 | 9,725 |
| `cine-timeline` | 9,596 | 14 | 4,557 |
| `cine-comp` | 8,103 | 5 | 1,554 |
| `cine-audio` | 7,605 | 5 | 1,576 |
| `cine-logistics` | 5,604 | 3 | 945 |
| `cine-previs` | 5,329 | 2 | 174 |
| `cine-shell` | 4,583 | 2 | 452 |
| `cine-script` | 3,746 | 6 | 919 |
| `cine-gpu` | 1,664 | 2 | 437 |
| `cine-pipeline` | 1,448 | 8 | 2,435 |
| `cine-cli` | 1,020 | 0 | 0 |
| `cine-jobs` | 758 | 1 | 301 |
| `cine-audio-io` | 317 | 1 | 170 |

UI: 7,977 lines of TS/TSX, 15 panels, 3 state modules, 9 e2e scripts.

### Real information flow (only arrows the code supports)

```
USER ACTION (React panel)
  ↓  ui/src/rpc/client.ts — JSON-RPC over WebSocket
WS /ws → cine-runtime::rpc::dispatch                (79 methods)
  ↓
OmniProject.engine.transact(label, |tx| …)          — one UCG transaction
  ↓
REACTORS (cine-pipeline, 19 plans)                  — pure; grouped into ONE undo step
  ↓
DOMAIN ENGINE
  (cine-timeline / cine-color / cine-media / cine-audio / cine-comp / cine-script /
   cine-logistics / cine-previs)
  ↓
PROJECTION (cine-runtime::projection)  →  broadcast to every connected client
  ↓
HTTP GET /frame /thumbnail /render-file /render-frame /callsheet /health
  ↓
VIEWER (React <img>)  ·  NATIVE WINDOW (cine-shell)  ·  RENDER OUTPUT (jobs)
```

**Source of truth per domain**

| Domain | Source of truth |
|---|---|
| Project structure, all entities | UCG (`ucg-core`) |
| Media identity | Content hash (`cine-media::identity`) |
| Time | `ucg_core::RationalTime` |
| Colour config | `color.config` node + `cine-color::ocio` |
| Derived state | `cine-runtime::derived` + job manifests on disk |
| Media availability | `cine-runtime::media_state` |
| Navigation subject | **React app state — deliberately NOT the graph** |

**Architectural constraint to preserve:** `cine-pipeline` depends on `cine-media`
**only as a dev-dependency** — deliberate, documented in its `Cargo.toml`:
*"cine-pipeline owns reactors, not media decoding."* Do not break this.

---

## PART 6 — UCG / DATA / INFORMATION MODEL

### Vocabulary

**65 node kinds** (`crates/ucg-schema/src/kinds.rs`):

- `project`
- `script.*` (14): document, scene, action, character, dialogue, parenthetical,
  transition, section, synopsis, note, lyric, centered, page_break, title_page,
  location, prop
- `logistics.*` (10): breakdown, call_sheet, cast, crew, day, lens_grid,
  production_log, schedule, shot, shot_list, take
- `previs.*` (8): scene, shot, camera, character, light, prim, rig, stage,
  stroke, dialogue_track
- `nle.*` (8): timeline, track, clip, gap, stack, transition, marker, caption,
  captions
- `color.*` (4): config, grade, lut, space
- `audio.*` (5): session, track, clip, bus, stem
- `comp.*` (2): graph, node
- `board.*` (3): frame, layer, stroke
- `asset.*` (2): media, model
- `media.stream`, `media.derivative`, `sync.alignment`

**6 edge kinds** (`crates/ucg-core/src/node.rs`): `Contains`, `DependsOn`,
`DerivedFrom`, `References`, `SyncedTo`, `Binds`.
`EdgeKind::propagates()` is true for `DependsOn`, `DerivedFrom`, `SyncedTo`.

### Mechanics

**Node creation / mutation:** only inside `engine.transact(label, |tx| …)` via
`tx.create`, `tx.ensure`, `tx.set`, `tx.remove_attr`, `tx.link`.
Derived ids are deterministic: `EntityId::derived(source, kinds::X)`; importer
ids `EntityId::from_key("<importer>:<stable key>")`; media ids
`EntityId::from_key("media:file:<hash>")`.

**Traversal:** `graph.out_edges/in_edges/targets/sources/children/parent/
ancestor_of_kind/of_kind/descendants/dependencies/dependents/topo_order`.

**Persistence** (`ucg-core/src/persist.rs`):

```
graph.json        snapshot (pretty JSON; diff-friendly)
graph.msgpack     optional fast binary snapshot
oplog/NNNN.jsonl  deltas since the snapshot, one JSON delta per line
```

Op-log records are schema-stamped, and records from a **newer** schema are
refused (`d3a0519`). Crash recovery replays the log and reports what it found;
proven by killing the process for real (`ce46f59`).

**Transactions and undo:** one `engine.transact` produces one `UndoGroup`
containing the user's ops *and* every reactor's follow-up. One user action = one
undo step.

> ⚠️ **Never hold an `engine.read()` guard across `engine.transact(...)`** — the
> RwLock is not reentrant. This no longer hangs silently: the commit times out
> and returns `UcgError::WriteLockTimeout` naming the likely cause. Scope every
> read guard in its own block.

**Reactors** (`crates/cine-pipeline/src/reactors.rs`, 19 plans): script,
timeline, colour, audio, comp, previs, logistics, captions, sync dialogue
placeholders, recompute scene stats, reschedule, review captions against the
script, flag clips for conform, ensure comp for shot, assign input colorspace by
file rule, ensure grade + propagate colour meta.

> ⚠️ **Graph writers must be idempotent** — re-running with the same input must
> produce zero ops (`group.deltas.is_empty()`). Reactor convergence depends on it.

### SUBJECT × LENS

**WHAT IS A SUBJECT?** A node of the project graph — a scene, shot, take, media,
timeline. Identified by `{id, kind}`.

**WHAT IS A LENS?** Which craft's view is open. The existing workspace tabs act
as lenses (editorial, media, script, colour, audio, production, …).

**WHY ARE THEY SEPARATE?** A filmmaker does not work in a department; they work
on a *thing* and need the departments' views of that thing. Department tabs force
the user to hold that correspondence in their own head — the exact failure of the
tool-chain CINE-OMNI exists to remove.

**HOW IS CURRENT SUBJECT STATE STORED?** In `ui/src/state/subject.ts` — **React
application state, NOT the graph.** Where a person is looking is not a fact about
the film. The runtime is never told which lens is open.

**HOW DOES A LENS CHANGE WITHOUT CHANGING SUBJECT IDENTITY?** The lens is never
an input to what the subject is. Survival is true **by construction**; there is
no restore path that could get it wrong.

**HOW DOES RELATIONSHIP NAVIGATION WORK?** `subject(id)` →
`crates/cine-runtime/src/subject.rs` returns `{subject, path, related[]}`.
`related` is **one hop, both directions, across all six edge kinds**, grouped by
what each neighbour *is*. One hop is deliberate: a scene's takes are three edges
away, and stepping is what makes this navigation rather than a tree dump of the
project.

**WHY DOES THE GRAPH REMAIN AUTHORITATIVE?** The only judgement held in Rust is
`GROUPS` — a kind-prefix → (craft, lens) table living beside the graph
vocabulary. React hard-codes no relationships. A kind the table has not been
taught about is shown under **Other** rather than hidden, because that is a gap
in the table, not a reason to lose the thing.

```rust
pub const GROUPS: &[(&str, &str, Option<&str>)] = &[
    ("script.",          "Story",      Some("script")),
    ("board.",           "Visual",     Some("storyboard")),
    ("previs.",          "Visual",     Some("previs")),
    ("logistics.shot",   "Production", Some("production")),
    ("logistics.take",   "Production", Some("production")),
    ("logistics.",       "Production", Some("production")),
    ("asset.media",      "Production", Some("media")),
    ("media.derivative", "Production", Some("media")),
    ("media.stream",     "Production", Some("media")),
    ("asset.",           "Production", None),
    ("nle.",             "Post",       Some("editorial")),
    ("comp.",            "Post",       Some("vfx")),
    ("color.",           "Post",       Some("colour")),
    ("audio.",           "Post",       Some("audio")),
    ("sync.",            "Post",       Some("editorial")),
    ("project",          "Project",    None),
];
```

> There is **no Delivery row** because no deliverable kinds exist yet.

---

## PART 7 — USER-FACING PRODUCT / UI AUDIT

11 workspaces declared in `ui/src/workspaces/index.ts`, each gated by a runtime
capability, each stating truthfully why it cannot open.

| Surface | State | What a real user can actually do |
|---|---|---|
| Project shell / menubar | **VERIFIED** | New sequence, import media (as a job), import screenplay, save, undo/redo |
| Project browser | **VERIFIED** | Sequences, script scenes/shots/takes, media with real state chips (P proxy, ∿ waveform, ≠ replaced) |
| **Relationship rail** | **VERIFIED** | Path crumbs, dominant subject, "Open in *lens*", grouped one-hop relationships, click to navigate |
| Source viewer | **VERIFIED** | Decoded picture, source timecode readout, mark in/out, project-view toggle |
| Record viewer | **VERIFIED** | The cut's picture at the playhead, proxy/original substitution stated, Angles mode (four-up) |
| Timeline | **VERIFIED** | Real cut, 4 tracks, select/ripple/roll/slip/slide/razor, snap, linked, marks |
| Inspector | **VERIFIED** | Integrity, Resolution (state + evidence), Proxies, Sound, Timecode, Media/Video/Streams |
| Jobs / Renders | **VERIFIED** | Live job records, render form, render list with state/reasons/preview/download |
| **Colour** | **IMPLEMENTED** | Display/view selection; per-media colorspace with provenance; **per-clip CDL editing** (slope/offset/power/saturation, numeric, neutral reset); "not applied from this config" list |
| Script / Writer | **VERIFIED** | Read mode and Fountain rewrite mode |
| Production | **VERIFIED** | Stripboard, constraints, call sheets, breakdown, shots |
| Captions | **VERIFIED** | Author, link to dialogue, export |
| Capabilities panel | **VERIFIED** | Reports the runtime's own capability registry |
| Storyboard | **PLANNED** | Tab states: *"Board frames exist in the schema; nothing is built."* |
| Shot / Previs | **PLANNED** | *"Previs cameras and stages are in the graph; no viewport exists."* |
| 3D / Animation | **MISSING** | *"Not started. No 3D workspace exists."* |
| VFX / Compositing | **ENGINE EXISTS / UI MISSING** | *"A node graph exists in cine-comp; no interface is built."* |
| Delivery | **PLANNED** | *"Not started. Export exists on the command line only."* |

**Known UI / backend discrepancies**

1. The **`GradeStack`** (11 ops, layers, masks, qualifiers) is persisted on every
   `color.grade` node and **no UI or committed runtime path reads it**. The
   Colour panel edits only the **CDL**.
2. `cine-comp` has a real compositing engine (8,103 lines, EXR, data windows,
   masks, union bounding boxes) with **zero** interface.
3. `cine-previs` (5,329 lines) and the `board.*` kinds have **zero** interface.
4. The rail has no Delivery group because no deliverable kinds exist.

> Never confuse "button exists" with "feature works", or "panel renders" with
> "backend capability exists".

---

## PART 8 — MEDIA SYSTEM

**Identity is content-derived, never path-derived:**
`EntityId::from_key("media:file:<hash>")`. A file that moves is the same media;
a re-export is not. This is what makes relinking and duplicate detection work.

- `ContentHash::fast` — SHA-256 over size + head + tail.
  **Measured: 536.9 MB identified in 91.8 ms, resident memory growth 1.4 MB.**
- `ContentHash::full` — whole file, for verification and archive.

### Media presence is two things that are never separated

`crates/cine-runtime/src/media_state.rs`.

**Presence** — eight states, never collapsed into "not found":

| State | Meaning |
|---|---|
| `no_such_media` | No node with that id — **the only honest "not found"** |
| `never_probed` | Registered from an imported cut, never read |
| `present` | Probed, recorded as present |
| `offline` | Probed, a check found nothing there |
| `replaced` | A different file is at the recorded location |
| `not_found_by_content` | A scan searched and could not find this content |
| `undecodable` | Readable, and no decoder in this build handles every stream |
| `not_on_this_machine` | Recorded at a scheme this machine cannot open |

**Evidence** — `never` / `at_import` / `checked`, with a timestamp.

> **Why evidence exists.** `online` was a tri-state pretending to be a boolean —
> `true` at import, `false` after a check, and **absent** when never established.
> Five surfaces each tested `online !== false`, so *never established* read as
> *online*, and a take nobody had ever looked at was reported as present at its
> recorded path. Presence is now always reported with the grounds it rests on.

**Nothing in `MediaState` touches the disk.** A projection is rebuilt on every
change and a project may hold thousands of assets; stat-ing all of them on every
keystroke would make the interface a filesystem crawler. `check_media`
establishes presence afresh, and **a check is evidence even when it confirms**
what was already believed.

**Locator** (`crates/cine-media/src/locator.rs`): `Path` / `Elsewhere{scheme}` /
`Nothing`. `PathBuf::from("file:///media/a.mov")` is a *relative path named
`file:`* with two empty components — so every check answered "not there", the
right conclusion for the wrong reason.

### Derived data

Proxies, waveforms, thumbnails, renders. **One rule for staleness:** a fact about
a file on disk is staleness **only where it contradicts what the project claims**.
Where the project already records the media as away (offline, or a namesake its
relinking refused), the same fact means only that the work cannot be *remade* — a
*note*, plus `remakeable: false`, not staleness.

**Atomic output:** proxy, waveform and render write into `….partial` and rename;
thumbnails are written beside themselves and renamed, and count only if a whole
PNG. **Succeeding means validated** — a proxy is re-probed, a waveform is read
back and must hold the channels, rate and length it claims.

### Formats

| Format | State |
|---|---|
| ISO-BMFF / QuickTime read | VERIFIED |
| QuickTime write (H.264 + 24-bit LPCM) | VERIFIED (GStreamer `qtdemux` oracle) |
| WAVE read | VERIFIED |
| Image sequences (EXR / PNG / TIFF) | VERIFIED |
| Matroska | **PARTIAL** — described from the EBML header; picture **not decodable** |
| MXF | **MISSING** |
| Camera RAW | **BLOCKED** (licensing, see PART 12) |

### Honesty rules

- A codec that is not handled returns `Error::NoDecoder` naming the real
  alternatives, **never a blank frame**.
- `/frame` refuses to serve a picture from a path the project records as
  `replaced` — a stranger's take is never served as this one.
- A picture larger than the machine's budget is **refused by name** before
  anything is allocated (`cine-media::budget`), rather than ending the
  application. `CINE_PICTURE_BUDGET` raises the limit.
- Media tests use real files and `need!` to skip loudly when absent. A media test
  that quietly passes with no media is worse than no test.

---

## PART 9 — TIME / PLAYBACK / AUDIO / RENDERING

**One time model:** `ucg_core::RationalTime` — integer ticks at an exact `Rate`.
Floats are for display only: never for storage, comparison or seek.

> ⚠️ **Historical trap.** The old media `TimeBase` was a *period*
> (`TimeBase::new(1, 48000)` meant 48 kHz); `Rate` is a *rate*, so the equivalent
> is `Rate::whole(48000)`. Never construct a rate from a decimal (`29.97` is not
> `30000/1001`), never derive a scale factor by dividing two rates, and never
> reach for `f64` seconds except to display. `rescale_checked` reports whether a
> conversion was exact — ignoring that flag is a choice to lose information.

**Timecode.** Drop-frame arithmetic lives in exactly one place
(`cine-media::time::Timecode`). At 29.97, `01:00:00;00` is frame **107,892** and
`01:00:00:00` is frame **108,000** — 108 frames apart, the classic conform error.
`tmcd` tracks are read and written. A **counter** track and a **discontinuous**
track are both reported, and both **refuse to label frames**, because adding a
frame count to them produces a number that looks like a timecode and is one for
nothing.

**Playback.** The clock decides which picture is due; the scheduler decides
whether to present, hold, drop or refuse; the event loop sleeps on
`next_deadline()` rather than on a frame interval.

> Never advance a playhead by counting frames. Never report a transport as
> `Playing` while frames are being dropped — `Degraded` exists for that.

**Decode-ahead** runs on a worker thread (`cine_media::DecodeAhead`), never on the
presentation thread, and is bounded **by bytes checked before decoding** —
checking after let a picture that did not fit be decoded, evicted and decoded
again forever. Decode-ahead does not make a decoder faster: without a pre-roll it
measures no better than synchronous decoding, so a transport that starts playing
must pre-roll a cushion first.

**Recorded measurements** (dev profile unless stated):

| Measurement | Value |
|---|---|
| Presentation after planes-not-read-back | 24 fps on screen |
| Movie render, 46 frames + sound | 1,171 ms |
| Render submit → visible in browser | 1.1–3.3 s |
| Proxy 640×360 → 320×180 | 4.4 pictures/s |
| Proxy 1920×1082 → 958×540 | 1.9 pictures/s (after 1.7× resampler + 2× YUV byte-path) |
| Waveform | 43–78× real time; 6.9 kB per 4 s stereo |
| Import of a 2 s movie | 46 ms |
| Relink, typing to relinked | 229 ms |
| Scene-linear 4 frames → PNG16 | 43 ms |
| Scene value 4.0 | 0.6398 in PQ @1000 nits vs 0.8999 in sRGB |
| Identity of a 536.9 MB file | 91.8 ms, 1.4 MB resident growth |
| Import click → asset visible (real app) | 408–476 ms |
| Waveform mark → proxy mark, 24× 1280×720 | 5.5–7.4 s |

> ⚠️ **Playback gates cannot currently be re-run** — the H.264 test media is
> absent. Historical playback numbers stay historical and must not be quoted as
> current without re-measurement.

---

## PART 10 — COLOUR SYSTEM (critical)

**Modules** (`crates/cine-color/src/`): `aces.rs`, `aces2.rs`, `curve.rs`,
`error.rs`, `file_rules.rs`, `grade.rs`, `lib.rs`, `lut.rs`, `mask.rs`,
`ocio.rs`, `processor.rs`, `scopes.rs`, `space.rs`, `transfer.rs`, `ucg.rs`,
`view.rs`, `yaml.rs`. **110 tests, all passing.**

### A. WHAT IS VERIFIED IN THE ENGINE

- OCIO config loading, including the released ACES 1.x and 2.0 configs.
- **Every colorspace of the released ACES configs converts as OpenColorIO does**
  — OCIO used as an independent oracle.
- ACES 2.0 output transforms matched to OCIO; a processor built on an ACES 2.0
  view **declares itself an approximation**.
- **OCIO file rules** decide a file's colorspace, as OpenColorIO decides it.
- ASC CDL apply, plus CDL XML read and write.
- Scene-linear vs display-referred: `view_for` chooses the **exact processor**
  for scene-linear pictures and a **measured 33³ lattice** for display-referred
  ones, reporting `worst_error`.
- Viewer and render share **one** chain — grading and viewing cannot disagree.
- PQ and HLG view transforms compute exactly.
- Scopes of the picture as shown.

### B. WHAT EXISTS BUT IS NOT USER-FACING

**`GradeStack`** — `{ version, working_primaries, layers[] }`, each
`GradeLayer { name, enabled, opacity, mask, qualifier, ops[] }`, with **11
`GradeOp` variants**:

```
Exposure { stops }            Wheels(Wheels)             Cdl(Cdl)
Contrast { amount, pivot }    Saturation { amount }      HueRotate { degrees }
TemperatureTint { k, tint }   PrinterLights { points }   Curves(Curves)
Lut3D(Lut3D)                  Matrix { m, offset }
```

Persisted via `store_grade` / `load_grade` under the `grade_stack` attribute.

> **In the committed tree it has no evaluation path.** It is referenced nowhere
> in `cine-runtime`; the viewer chain evaluates `ColorMeta.cdl` plus an optional
> look LUT only. `GradeStack::apply_pixel` exists and is tested, but **no frame
> has ever been put through it.**

Also engine-only: HSL qualifiers, geometric masks, curves, printer lights,
temperature/tint, free matrices.

### C. WHAT IS PARTIAL

- `colour.hdr.view` — PQ/HLG compute exactly, but **no surface can present HDR**:
  the window and the browser are SDR, so a PQ picture is labelled as **code
  values**. **Never call this HDR display.**
- The colour workspace exposes CDL only.

### D. WHAT IS MISSING

`colour.hdr.mastering` (mastering display / MaxCLL not read from any container);
wheels, curves, LUT management and mask UI; per-shot stills, shot matching,
versioned grades, grade copy/paste; hardware grading panels.

### E. WHAT THE 8 EXISTING `color.grade` NODES ACTUALLY CONTAIN

Verified by reading `~/cine-projects/demo/graph.json`:

- **8 nodes**, each with `name`, `color_meta`, `grade_stack`.
- **All 8 have an empty layer list** — `layers: []`, `version: 1`,
  `working_primaries: aces_ap1`.
- They are created automatically by the reactor *"ensure grade + propagate color
  meta"* for **every placed clip**, not by anybody grading.

> **There is no grading data in the demo project.** Node existence ≠ graded.

### F. WHAT A FIRST REAL COLOUR WORKSPACE WOULD NEED

1. An evaluation path for `GradeStack` into the viewer/render chain.
2. Protocol read/write for the stack, distinguishing supported ops from
   engine-only ones by name.
3. A UI for ops that are *numbers* first (exposure, contrast, saturation,
   temperature/tint, printer lights). Wheels, curves and masks need real controls
   and **must not be faked**.
4. `grade_fingerprint` in render provenance so a render goes **stale** on regrade.
5. A guard: a grade with masks or qualifiers **cannot be baked into a 3D LUT** —
   a LUT is a function of colour alone, and baking one would freeze the mask's
   value at a single point and apply it everywhere.

---

## PART 11 — TESTING / VERIFICATION

**1,179 `#[test]` functions across crates.**

**Gates recorded at `5fa7175`:** 135 suites, **1,166 passed, 0 failed, 14
ignored**; UI 38 tests; `tsc --noEmit`; Vite production build; ingest and subject
e2e against a freshly built binary; `clippy --workspace --all-targets -D
warnings`; `cargo-deny`; `rustfmt`.

**CI** (`.github/workflows/ci.yml`): `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
`cargo build -p cine-gpu --features wgpu`, `cargo run -p cine-cli -- demo`,
`cargo deny check`, plus `npm ci && npm run build && npm test` for the UI.

### Strongest proofs (independent oracles)

| Oracle | What it proves |
|---|---|
| **OpenColorIO** | Every ACES colorspace conversion and display style |
| **GStreamer `qtdemux`** | QuickTime muxing — returned byte-exact sound |
| **ttconv** | IMSC timed text |
| **An independent SCC reader** | Broadcast captions — **it found a real bug** |
| **symphonia's own Matroska parser** | Cross-checked the new EBML reader; immediately exposed that channel counts arrive as a *layout*, a bug shared by production code |
| **Real media throughout** | `need!` skips loudly when absent |
| **Headless Chrome against a real `cine start`** | `ingest`, `proxy`, `relink`, `render`, `subject`, `editor`, `production`, `colour` |

### Capabilities WITHOUT independent verification — do not overclaim

- **`tmcd` timecode byte layout.** No independent timecode reader exists on this
  machine (`qtdemux` reports `unknown subtype tmcd`). Held only by a test that
  walks the boxes from published field offsets, plus arithmetic done in the test
  (one hour of drop-frame 29.97 = 30 × 3600 − 2 × 54 = 107,892).
- **Conform.** Never round-tripped against another application — only against
  this build's own reader. **AAF and XML are not written at all.**
- **`colour.mjs` and `editor.mjs` e2e SKIP** unless `CINE_E2E_MEDIA` points at two
  decodable H.264 files, so in a normal run they prove nothing.
- **H.264 patent position** (as distinct from source licence) is unresolved.

> **Test counts do not imply breadth.** 1,166 passing tests establish the
> foundation and the media / editorial / colour-engine paths. They say nothing
> about storyboard, previs, 3D, VFX UI, delivery or collaboration — which have no
> tests because they have no code.

---

## PART 12 — LICENSING / EXTERNAL INTEGRATION

Project licence **Apache-2.0**. Governing rule from
`docs/blueprint/01-LICENSING-SPINE.md`:

> **No GPL source may be copied into, statically linked into, or dynamically
> linked against CINE-OMNI.**

| Component | Established finding |
|---|---|
| **FFmpeg** | LGPL-2.1+ core; GPL only with `--enable-gpl`. **An LGPL-only build, dynamically linked, is a viable media foundation** for an Apache-2.0 product — subject to LGPL-2.1 §6 relink obligations. The most consequential verified result. |
| **Blender / Kdenlive / Natron / Ardour** | GPL — **cannot be linked**. Only headless process + USD interchange, or architectural ideas (clean-room). |
| **OpenColorIO, OpenEXR, Imath** | BSD-3-Clause — clear. |
| **OpenImageIO** | **Relicensed Apache-2.0 in 2023** (>99.86% of lines) — the one major C++ imaging library whose source may be *ported* rather than merely linked. |
| **OpenUSD** | ⚠️ **NOT Apache-2.0.** Since release 24.08 (2024-07-25) it is the bespoke, non-OSI, non-SPDX **Tomorrow Open Source Technology License 1.0** (Apache-2.0 with a *narrowed* §6). **LEGAL REVIEW REQUIRED before any dependency.** The pure-Rust `openusd` crate is plain MIT and carries no such question. |
| **OpenFX** | **BSD-3-Clause** — hosting OFX is licence-clear. Deferred on engineering grounds, not legal ones. |
| **VST3** | **MIT as of VST 3.8 (2025-10-29)**; Steinberg states GPLv3/proprietary "is no longer available". Hostable. ⚠️ `nih-plug`'s VST3 export pulls GPL-3.0 `vst3-sys`; use `vst3` (coupler-rs, MIT). CLAP (MIT) and LV2 (ISC) remain clear. |
| **ASIO** | ⚠️ **Now the trap** — moved to dual GPLv3 / proprietary. **Cannot be linked** by an Apache-2.0 product. |
| **Camera RAW** | ⚠️ **RESOLVED UNFAVOURABLY** — `rawloader`/`rawler` are LGPL-2.1 and `imagepipe` is LGPL-3.0-only. Rust links statically, so there is no LGPL §6(b) relinking escape. **RAW stills ingest is not available on the terms usually assumed.** |
| **Immersive audio** | Open path is clear: `libadm` and `libbw64` are Apache-2.0; the EBU ADM Renderer is BSD-3-Clause-Clear. |
| **Codec patent pools** | **STILL OPEN.** A BSD-2 H.264 decoder settles copyright and says nothing about patents. **LEGAL REVIEW REQUIRED before distribution.** |
| **AAF** | Named as required interchange in `PRODUCT-STRATEGY.md`; **not implemented**. |

**Obligations owed once anything is integrated:** a machine-readable third-party
inventory; bundled licence texts; dynamic linking plus a documented relink path
for any LGPL component; a written record of every clean-room reimplementation;
`cargo-deny` wired into CI (**already done**). `licenses/THIRD-PARTY.toml` exists
with crate-scoped exceptions.

---

## PART 13 — ARCHITECTURAL GAPS

### Previously identified, re-verified

| # | Gap | Class | Status |
|---|---|---|---|
| A1 | No deliverable / rights / IP / budget / research / QC / localization kinds — the vocabulary stops at post-production | **ARCHITECTURAL GAP** | Open |
| A2 | No `film` / `episode` layer above `project` — episodic work has no home | **ARCHITECTURAL GAP** | Open |
| A3 | **No plugin or external-process architecture at all** (`grep` for `Command::new` returns nothing in `crates/*/src`) | **INTEGRATION GAP** | Open — the ceiling on Unreal, Houdini, Nuke, Pro Tools, RAW SDKs |
| A4 | No collaboration, identity or permissions. Multi-*client* exists; multi-*user* does not. Binds `127.0.0.1` with **no auth** | **ARCHITECTURAL GAP** | Open — **LAN access is NOT implemented and must not be claimed** |
| A5 | `media_path` conflates a URL with a filesystem path | **ARCHITECTURAL GAP** | **Partly closed** — `locator.rs` exists and is used by media state; ~20 other call sites still `PathBuf::from` the raw string (they behave safely) |
| A6 | No large-binary asset versioning (`asset.model` is a kind with no pipeline) | **ARCHITECTURAL GAP** | Open |

### Newer gaps found in this audit

| # | Gap | Class |
|---|---|---|
| A7 | **`GradeStack` has no evaluation path in the committed tree** — the richest colour model is stored and dead | **ENGINE GAP** (the WIP addresses it) |
| A8 | `cine-comp` (8,103 lines) has **no interface** | **UI GAP** |
| A9 | `cine-previs` (5,329 lines, only 174 test lines) and `board.*` have **no interface** and thin tests | **UI GAP + VERIFICATION GAP** |
| A10 | Timecode byte layout has **no independent verification** | **VERIFICATION GAP** |
| A11 | Conform never round-tripped against another application; AAF/XML absent | **VERIFICATION GAP + PRODUCT GAP** |
| A12 | `colour.mjs` / `editor.mjs` e2e silently SKIP without external media | **VERIFICATION GAP** |
| A13 | Playback gates cannot be re-run (test media absent) | **VERIFICATION GAP** |
| A14 | HDR cannot be *presented* — SDR surfaces only | **HARDWARE GAP** (honest, labelled) |
| A15 | MXF unreadable; Matroska picture undecodable; RAW blocked | **ENGINE GAP + LICENSING BLOCK** |
| A16 | GitHub default branch is `master`, 113 commits behind the work | **PROCESS GAP** |

---

## PART 14 — PRODUCT COMPLETENESS

*No score, no percentage. Factual categories only.*

**REAL TODAY (verified, user-reachable)**
Exact time · UCG with undo, op-log and crash recovery · `cine start` · media
ingest as a job · content identity and relink by content · media state with
evidence · proxies · waveforms · thumbnails · source and record viewers ·
timeline with the full trim vocabulary · three/four-point editing · retiming
including ramps · multicam angles and sync-by-sound · captions (IMSC, SRT, VTT,
SCC) · EDL export with real source and record timecode · OTIO in timecode space ·
render to PNG / EXR / movie with sound and staleness · script read, Fountain
rewrite and revisions · production office (breakdown, stripboard, DOOD, call
sheets) · colour management (OCIO, ACES, file rules, views) and **CDL grading** ·
Subject × Lens navigation · jobs.

**PARTIAL TODAY**
Matroska (described, picture not decodable) · high-bit-depth (stills yes, moving
picture no — the only movie decoder is 8-bit 4:2:0) · alpha (stills only) · VFR
(detected and refused) · relink (image sequences not searched) · bounded decode
(no tiled/partial) · conform (no import, no AAF/XML) · HDR view (computes, cannot
present).

**ENGINE EXISTS / UI MISSING**
`GradeStack` (11 ops, layers, masks, qualifiers) · compositing node graph ·
previs cameras and stages · board frames · scopes · printer lights.

**ARCHITECTURE EXISTS / INTEGRATION MISSING**
Jobs ready for external processes but no process boundary · USD export exists but
no asset pipeline · multi-client live sync but no collaboration.

**PLANNED**
Storyboard · previs viewport · delivery workspace.

**MISSING**
3D / animation · development / IP / budget / research · QC · localization and
dubbing · DCP / IMF · archive · scheduling beyond the stripboard · casting · VAD.

**BLOCKED**
ASIO (GPLv3) · camera RAW (LGPL static-link) · OpenUSD (licence review) · codec
patents (legal review).

**REFUSED**
Multichannel sound beyond 8 channels (`sound.multichannel`) · AAC encode (no
pure-Rust encoder exists; QuickTime + 24-bit LPCM chosen instead and verified
byte-exact against GStreamer).

> A full product vision is far broader than the currently exposed UI. That is
> expected and must not be papered over.

---

## PART 15 — STORAGE / BUILD STATE

| Path | Size |
|---|---:|
| `target/` | **43 GB** |
| `.git` | 20 MB |
| `ui/node_modules` | 76 MB |
| `crates/` (all source) | 6.5 MB |
| `docs/` | 1.7 MB |
| Tracked tree | 8.7 MB (467 files) |
| Disk | 233 GB total, **124 GB free**, 44% used |

`target/` is **139 build artifacts over 100 MB**, including three stale
generations of the `cine` binary — cargo never collects old hashes. This is the
*cache* class under `STORAGE-POLICY.md` and is safe to prune, but **there is no
measured reason to**.

Dependencies build without debuginfo (30% smaller, measured); workspace crates
keep it.

**Safe to leave alone: everything.** If pruning ever becomes necessary, remove
only artifacts whose hash is no longer current — **never `cargo clean`**, never
while a build is running, and never source, project, media or derived data.

`.gitignore` correctly excludes `/target`, `/ui/node_modules`, `/ui/dist`,
`*.log`, `.DS_Store`. **Zero artifacts are tracked.**

---

## PART 16 — CURRENT COLOUR / WORKING-TREE STATUS

**Status: UNCOMMITTED · partially implemented · 3 tests failing · engine side
sound · ready for continuation.**

### Changed files (8 modified, 2 new; +194 / −21)

```
M crates/cine-color/src/grade.rs        +14  GradeStack::varies_by_pixel()
M crates/cine-color/src/processor.rs    +29  Op::Grade(Arc<GradeStack>) plus
                                             Display / is_identity / apply / inverse arms
M crates/cine-color/src/ucg.rs          +42  viewing_processor_with(…, grade, …)
M crates/cine-color/src/view.rs         +66  view_for_graded, bake_view_graded,
                                             grade_fingerprint
M crates/cine-runtime/src/lib.rs         +1  pub mod grade;
M crates/cine-runtime/src/provenance.rs +12  grade in the render staleness fingerprint
M crates/cine-runtime/src/render.rs     +31  per_entry carries Option<GradeStack>;
                                             transform cache key includes it
M crates/cine-runtime/src/rpc.rs        +20  "grade" and "grade_set" methods
?? crates/cine-runtime/src/grade.rs     243  protocol module: 5 supported ops,
                                             named unsupported list
?? crates/cine-runtime/tests/grade_workspace.rs  357  8 tests
```

### What was being attempted

Give `GradeStack` an evaluation path — **one** new `Op::Grade` using the engine's
already-tested `apply_pixel` — then expose it over the protocol (exposure,
contrast, saturation, temperature/tint, printer lights) while *naming* the engine
ops that have no interface yet (wheels, cdl, hue_rotate, curves, lut3d, matrix,
mask, qualifier) rather than hiding them. Plus: refuse to bake a
mask/qualifier grade into a 3D LUT, and make renders go stale on a regrade.

`Op::Grade` is deliberately opaque to the optimiser (a stack carries masks and
qualifiers that depend on pixel position and value, so it cannot be fused into a
matrix) and returns `None` from `inverse()` rather than inventing one.

### Verified test state (measured 2026-09-28)

- **`cine-color`: 110 passed, 0 failed** — the engine change is clean.
- **`grade_workspace`: 5 passed, 3 FAILED.**

Passing: `a_clip_with_no_grade_has_none_and_looking_does_not_make_one`,
`a_grade_asked_for_on_something_that_is_not_a_clip_is_refused`,
`the_protocol_says_what_it_cannot_do`,
`an_operation_this_build_cannot_set_is_refused_by_name`,
`undo_and_redo_move_through_the_grade`.

### The three failures, with exact causes — all test-harness premise errors, not engine defects

1. **`a_render_made_before_a_regrade_is_stale`**
   `color_view failed: "sRGB - Display" has no view "ACES 1.0 - SDR Video"`.
   The test hard-coded a display/view pair the default config does not have.

2. **`a_real_operation_is_written_read_back_and_kept_across_a_reopen`**
   `"… is not a clip in this project"`.
   **`Harness::start_in` builds an in-memory project (`OmniProject::new`) and
   cannot reopen from disk.** The test's reopen premise is invalid; it was
   mid-edit toward `OmniProject::open` when work stopped.

3. **`the_picture_actually_changes_when_the_grade_does`**
   `HTTP/1.1 404 Not Found`.
   The media lookup picked the harness's OTIO fixture media (`never_probed` →
   correctly 404 under the media-state rules) instead of the ingested take.

**Not abandoned. Ready for continuation** — fix the three test premises, then
re-gate.

---

## PART 17 — CURRENT EXACT CHECKPOINT

```
CURRENT BRANCH:              playback-scheduler
CURRENT HEAD:                5fa7175f41bb72a0bb054391e52e6b3b1f87731c
MASTER:                      971f14aeab5d27e7476513b89c52ca4c4801d15c  (113 behind)
REMOTE:                      git@github.com:thebatmansan/Cine-omni.git  (public)
PUSH STATUS:                 BOTH branches pushed (2026-09-24, authorized, no force).
                             local == origin/playback-scheduler.
                             GitHub default branch = master.
WORKING TREE:                DIRTY — 8 modified, 2 untracked (Colour slice).
                             Nothing staged.
LAST CLEAN VERIFIED COMMIT:  5fa7175
LAST CLEAN VERIFIED CAPABILITY:
                             Subject × Lens relationship navigation, graph-backed,
                             proven in the real application (ui/e2e/subject.mjs).
                             Gates at that commit: 135 suites / 1166 passed /
                             0 failed / 14 ignored; UI 38; tsc; Vite; ingest and
                             subject e2e; clippy -D warnings; cargo-deny; rustfmt.
CURRENT UNCOMMITTED WORK:    Colour slice — GradeStack evaluation path + grade protocol.
CURRENT TEST STATE:          cine-color 110 pass / 0 fail (engine sound).
                             grade_workspace 5 pass / 3 FAIL (all test-premise errors).
                             Full-workspace run NOT completed on the dirty tree.
CURRENT STORAGE:             target 43 GB · .git 20 MB · node_modules 76 MB ·
                             124 GB free
CURRENT PRODUCT STATE:       Strong foundation plus editorial / media / colour-engine
                             depth; product surface far narrower than the model.
                             11 workspaces, 4 explicitly empty.

NEXT INTENDED SLICE:         Finish the Colour vertical slice.
WHY IT IS NEXT:              It is in progress and incomplete. The engine is verified
                             and the protocol/UI path is the first proof that
                             CINE-OMNI can expose existing engine depth rather than
                             editorial features.
DEPENDENCIES:                None outstanding. Subject × Lens (done) supplies
                             navigation.
WHAT MUST NOT BE DONE YET:   Do not start compositing UI, external-process
                             architecture, collaboration, lifecycle vocabulary
                             extension, or broad UI redesign. Do not commit the WIP
                             while 3 tests fail.
```

---

## PART 18 — DISCREPANCY AUDIT

**1. "Zero meaningful grading surface"**
- **DOCUMENTED CLAIM:** `MASTER-AUDIT.md` re-anchor — *"colour, which has a
  verified engine … and no grading surface"*.
- **ACTUAL EVIDENCE:** `ui/src/panels/Colour.tsx` (224 lines) has a **Grade
  section with per-clip CDL editing** (slope / offset / power / saturation,
  numeric inputs, neutral reset) wired to `set_grade`, plus view selection and
  per-media colorspace with provenance. `ui/e2e/colour.mjs` exists.
- **SAFE CLAIM:** *A CDL grading surface exists and works. What has no surface is
  the layered `GradeStack`, which in the committed tree also has no evaluation
  path.*

**2. `CHATGPT_CONTEXT.md` verification status**
- **DOCUMENTED CLAIM:** *"424 tests pass, 0 fail."*
- **ACTUAL EVIDENCE:** 1,179 `#[test]` functions; 1,166 passing recorded at
  `5fa7175`.
- **SAFE CLAIM:** *That file is a stale handoff from an earlier era. This
  document supersedes it.*

**3. Main branch name**
- **DOCUMENTED CLAIM:** Session metadata says *"Main branch: main"*.
- **ACTUAL EVIDENCE:** `git branch -a` shows only `master` and
  `playback-scheduler`. No `main` exists locally or on the remote.
- **SAFE CLAIM:** *The trunk is `master`.*

**4. "Nothing pushed"**
- **DOCUMENTED CLAIM:** Prior context said nothing had been pushed.
- **ACTUAL EVIDENCE:** `origin` configured; both branches on GitHub; branch
  tracks upstream.
- **SAFE CLAIM:** *Both branches are pushed to a public GitHub repository.*

**5. Capability matrix row counts**
- **DOCUMENTED CLAIM:** `CINE-OMNI-CAPABILITY-MATRIX.md` summary says "154 rows".
- **ACTUAL EVIDENCE:** 169 rows counted. The `MEDIA_PATHS` registry in code has
  38 rows (24 Verified, 8 Partial, 3 Missing, 2 Implemented, 1 Refused); the
  codec `REGISTRY` has 21.
- **SAFE CLAIM:** *The code registry is authoritative for media paths; the matrix
  is the product model and its summary counts drift.*

**6. Grade node existence vs being graded**
- **DOCUMENTED CLAIM:** Earlier phrasing implied the 8 demo grades were user
  grading work.
- **ACTUAL EVIDENCE:** A reactor creates a `color.grade` for **every placed
  clip**; all 8 have `layers: []`.
- **SAFE CLAIM:** *Node existence ≠ graded. The demo project contains no grading
  data.*

**7. HDR**
- **DOCUMENTED CLAIM:** PQ / HLG "work".
- **ACTUAL EVIDENCE:** They compute exactly; every surface is SDR and output is
  labelled as code values.
- **SAFE CLAIM:** *HDR transforms are computed and verified; HDR is never
  **presented**.*

---

## PART 19 — DEVELOPMENT WORKFLOW FOR THE NEXT AI

1. Understand the user's current requirement.
2. Check the CINE-OMNI checkpoint (PART 17) **against the live repository** —
   branch, HEAD, `git status`.
3. Inspect the actual Claude Code output.
4. **Determine whether the previous task actually completed** — read the
   evidence, not the summary.
5. Compare the implementation against the requirement.
6. Identify missing validation.
7. Only then issue the next focused Claude Code instruction.
8. Read the resulting output.
9. Verify it.
10. Continue dependency-by-dependency.

**IMPORTANT**

- Do **not** blindly generate huge prompts after every CC output. First
  understand what CC actually did.
- If CC reports an **unfinished** task: finish that task, validate it, **do not
  jump ahead**.
- If CC reports **completion**: inspect the evidence, identify gaps, then
  continue.
- Autonomous progression is wanted; **reckless progression is not**.
- Phases are dependency ordering only. Do not ask the user for approval between
  every normal development slice.

---

## PART 20 — NON-NEGOTIABLE DEVELOPMENT RULES

- No fake functionality · no fake UI · no status-only capabilities · no inflated
  claims · no invented backend behaviour.
- No duplicate state models in React. **UCG remains authoritative.**
- Exact time correctness. Exact media correctness.
- Backend and frontend together.
- Real project data wherever possible. Load-bearing tests. Actual application
  verification.
- Professional creative-software UX. No generic SaaS dashboard. No unnecessary
  giant cards. No decorative gradients replacing useful information. No shallow
  feature checklists.
- No unauthorized pushes. **Master protected unless explicitly authorized.**
- No Docker direction.
- **No concurrent Cargo builds against the same `target/`.**
- No blind storage cleanup.
- Preserve licensing boundaries. Do not silently copy proprietary or GPL
  technology.
- Do not broaden a slice unnecessarily. Do not rewrite working architecture
  without evidence.
- Do not treat old benchmark results as current without checking.
- **Documentation is not proof. UI presence is not capability. Engine capability
  is not user-facing functionality.**

**Never convert:** planned → implemented · engine → UI · test fixture → product
capability · documentation → verified capability · UI control → functioning
backend · multi-client → collaboration · HDR code values → HDR display
presentation.

---

## PART 21 — COMPLETE TECHNICAL INVENTORY (read in this order)

**Read first**
`CLAUDE.md` (working rules and traps) · `docs/blueprint/MASTER-AUDIT.md`
(including the 2026-09-19 re-anchor) ·
`docs/blueprint/CINE-OMNI-CAPABILITY-MATRIX.md` (169 rows — the product model) ·
`docs/blueprint/MASTER-ROADMAP.md` (what was built; **not** what to build next) ·
`docs/blueprint/APPLICATION.md` (the long verified narrative).

**Architecture**
`docs/ARCHITECTURE.md` · `docs/MODULE-CONVENTIONS.md` ·
`docs/adr/0001-unified-context-graph.md` · `docs/adr/0002-toolchain.md` ·
`docs/adr/0003-reactor-cascade.md` · `docs/blueprint/02-TARGET-ARCHITECTURE.md` ·
`docs/blueprint/APPLICATION-RUNTIME.md` · `docs/blueprint/WORKSPACE.md`.

**Licensing**
`docs/blueprint/01-LICENSING-SPINE.md` · `deny.toml` ·
`licenses/THIRD-PARTY.toml`.

**Storage / performance**
`docs/blueprint/STORAGE-POLICY.md` · `STORAGE-FORENSICS.md` · `PLAYBACK.md` ·
`AV-SYNC.md` · `PHASE-2-MEDIA-ENGINE.md` · `EXACT-TIME-MIGRATION.md`.

**Research dossiers** (`docs/blueprint/research/`)
`01-3d-dcc` · `02-editorial-media` · `08-media-codec-stack` ·
`09-editorial-media-model` · `10-color-and-sync` · `11-shell-and-gpu-runtime` ·
`12-compositing` · `13-color-management` · `14-audio-architecture` ·
`15-production-dailies-collaboration` · `16-editorial-operations-interchange` ·
`17-openusd-and-assets`.

**Core code**
`crates/ucg-core/src/{engine,graph,tx,persist,exact_time,time_map,node,value}.rs` ·
`crates/ucg-schema/src/{kinds,lib,component,components}.rs` ·
`crates/cine-pipeline/src/{project,reactors,session}.rs`.

**Runtime / protocol**
`crates/cine-runtime/src/{rpc,server,projection,media_state,subject,jobs,render,
provenance,derived,proxy,relink,ingest,grade,playlist,waveform,lock,frames,http}.rs`.

**Media**
`crates/cine-media/src/{identity,probe,locator,tmcd,ebml,isobmff,colr,sequence,
mov,waveform,capability,budget,decode_ahead,clock,schedule,pictures,reader}.rs`.

**Colour**
`crates/cine-color/src/{ucg,grade,processor,view,ocio,aces,aces2,lut,mask,curve,
file_rules,scopes,space,transfer}.rs`.

**UI**
`ui/src/App.tsx` · `ui/src/state/{session,subject,captions}.ts` ·
`ui/src/rpc/client.ts` · `ui/src/workspaces/index.ts` ·
`ui/src/panels/{Rail,Colour,Inspector,Browser,Viewer,Record,Timeline,Jobs,
Production,Script,Writer,Captions,Impact,Capabilities}.tsx` ·
`ui/src/panels/mediaState.ts`.

**Tests to read first**
`crates/cine-runtime/tests/{media_state,subject_navigation,grade_workspace,
ingest_proxy_workflow,timecode_conform,derived_provenance,proxy_protocol,
relink_protocol,render_movie}.rs` ·
`crates/cine-media/tests/{timecode_track,matroska_header,large_media,
mov_roundtrip,image_sequence}.rs` · `crates/cine-color/tests/` ·
`crates/cine-jobs/tests/jobs.rs` · `ui/e2e/*.mjs`.

**Demo project**
`~/cine-projects/demo` — *The Harbour*, revision 40, 163 nodes / 201 edges,
1 sequence "Demo Reel v3" (4 tracks), 9 media (1 ingested, 8 OTIO references with
`file:///media/…` paths, never probed), 8 empty `color.grade` nodes,
27 `color.space`, 21 `comp.node`, 8 `nle.clip`, 7 `script.dialogue`,
7 `audio.clip`.

**Run the application**

```bash
./target/debug/cine start ~/cine-projects/demo --port 7331
# → http://127.0.0.1:7331/
# needs ui/dist; rebuild the frontend with:  cd ui && npm run build
```

`cine start` opens the project, serves the UI and the WebSocket API from one
process, and owns the project lock. A second `cine start` in the same directory
attaches to the running one rather than starting a rival — two runtimes would
interleave two op logs and corrupt the history.

> `ui/server/bridge.mjs` is **superseded and must not be used** — its own header
> records that it spawns one engine per browser client, which is the two-op-log
> divergence `cine start` exists to prevent.

---

## PART 22 — FINAL CONTINUITY SUMMARY

**WHAT ARE WE BUILDING?**
One unified professional filmmaking environment covering the whole cinema
lifecycle on a single reactive Unified Context Graph — one application, one
project, one persistent context, many professional lenses.

**WHY ARE WE BUILDING IT?**
Because a film currently moves through six or seven programs that cannot talk to
each other, and the filmmaker has to reconstruct the movie in their own head. The
differentiator is the persistent connected graph, not feature breadth.

**WHAT HAS CINE-OMNI ACTUALLY BUILT?**
An unusually strong foundation — exact rational time, a reactive project graph
with undo, an append-only op log and crash recovery, a single-owner runtime with
live multi-client state, content-derived media identity, real decoding, scheduled
playback, a real job system and render-to-delivery — plus genuine depth in
editorial, media, and the colour *engine*.

**WHAT HAS BEEN VERIFIED?**
See PART 11. The strongest proofs use independent oracles: OpenColorIO for
colour, GStreamer for QuickTime, ttconv and an independent SCC reader for
captions, symphonia for Matroska, real media throughout, and headless-Chrome
end-to-end runs against the real binary.

**WHAT IS ONLY PARTIAL?**
Matroska, high-bit-depth moving picture, alpha, VFR, image-sequence relink,
bounded decode, conform (no import, no AAF/XML), HDR view (computed, never
presented).

**WHAT IS STILL MISSING?**
Development / IP / budget, storyboard, previs viewport, 3D/animation, VFX UI, QC,
localization, DCP/IMF, archive, collaboration, external-process integration.

**WHAT IS CURRENTLY BEING WORKED ON?**
The Colour vertical slice — uncommitted; engine sound (110 colour tests pass);
three protocol tests failing on wrong premises.

**WHAT IS THE LAST CLEAN CHECKPOINT?**
`5fa7175` — Subject × Lens navigation, fully gated.

**WHAT IS THE NEXT EXACT DEVELOPMENT SLICE?**
Finish the Colour slice: fix the three test premises (a config display/view pair
that does not exist; an in-memory harness that cannot reopen a project from disk;
a media lookup that selects a never-probed fixture instead of the ingested take),
re-run the gates, then commit.

**WHAT MUST THE NEXT AI NEVER ASSUME?**
That documentation equals proof · that a UI control means a working backend ·
that engine capability is user-facing · that a test fixture is a product
capability · that multi-client means collaboration · that HDR code values mean
HDR presentation · that a passing test count implies breadth · that `main`
exists · that nothing is pushed · that the roadmap is the plan (the capability
matrix is the destination).

**WHAT MUST THE NEXT AI DO FIRST?**
Verify the live checkpoint (`git rev-parse HEAD`, `git status`), read
`CLAUDE.md`, then resolve the three failing `grade_workspace` tests before
anything else.

---

## === CONTINUATION COMMAND ===

Continue CINE-OMNI from the exact repository checkpoint documented above. Treat
this handoff as continuity context, but verify future Claude Code reports against
the actual repository — documentation is not proof. Do not restart the project or
invent a new roadmap; the capability matrix is the destination and the roadmap
records only what was built. First establish the current working-tree and commit
state (`git rev-parse HEAD`, `git status`) and understand the in-progress Colour
work: the engine change is sound (110 cine-color tests pass) but three
`grade_workspace` tests fail on wrong test premises — a config display/view pair
that does not exist, an in-memory test harness that cannot reopen a project from
disk, and a media lookup that selects a never-probed fixture instead of the
ingested take. Finish and validate that slice before starting anything new. Then
continue the next confirmed dependency-ordered slice. Assess Claude Code output
before issuing the next instruction, and never convert planned into implemented,
engine into UI, or a test fixture into a product capability.

---

## === ADDENDUM: 2026-09-28 — the Colour slice closed ===

**This addendum supersedes every statement above that says the `GradeStack` has
no evaluation path, that the browser shows untransformed pictures, or that
grading is engine-only.** Those were true when the body of this handoff was
written; they are not true of the tree that follows it. The body is kept as the
record of what was found, not as a description of what is there now.

What changed, and what is now demonstrable:

* `GradeStack` has an evaluation path. `cine_color::view::view_for_graded`
  applies a clip's ASC CDL and then its grade stack in the working space, and
  `Op::Grade` gives the processor chain a stage for it. A grade whose effect
  depends on the pixel (a mask, a qualifier) refuses to be baked into a lattice
  and is evaluated exactly instead (`bake_view_graded`).
* One function, three surfaces. The render (`render.rs::wanted_views` →
  `resolve_views`), the native window, and the browser's frames
  (`server.rs::project_view_for`) all go through `view_for_graded`, so they
  cannot disagree about what a shot looks like.
* `/frame` has clip context. `&clip=<id>` makes a frame that clip's, with its
  grade; without it the frame is the *file*, ungraded, because one file can back
  several clips. A clip that does not play that media is refused (422), not
  answered ungraded. The response carries `X-Cine-Clip`.
* `picture_at` answers whether a graded picture is available (`colour.available`,
  `colour.graded`, `colour.why_not`), so the monitor asks because the runtime
  said so rather than hopefully.
* Protocol: `grade` and `grade_set`. Five operations — exposure, contrast,
  saturation, temperature/tint, printer lights. `unsupported` names the other
  eight with reasons. `primary_settable` / `primary_why_not` say in advance when
  a write would be refused, and `set_primary` refuses a write over a layer
  holding an operation it cannot express, or a masked or qualified layer, rather
  than destroying work.
* Frontend: `useGrade` state, grade-stack controls in the Colour panel (only the
  supported operations, the unsupported ones named), the record monitor in the
  Colour workspace showing the clip's graded picture with a GRADED / VIEW / RAW
  marker, and the source viewer left ungraded on purpose.
* Render staleness includes the grade (`provenance.rs`), so a master made before
  a regrade reports `stale`.

**Verified in the running application** (`cine start --no-ui --no-window`, one
take, two clips of it, ACES 2.0 SDR Rec.709 through sRGB - Display):

```
file (no clip)   481a5ddd48fc6c0f   unchanged by any grade
clip A (+2 stops) ca5750ef165d8065
clip B (−1.5 stops, sat 0) 56c97da1af9e1f00
A vs B            different
render before the regrade   current  →  after   stale  →  after clearing  current
clip A's grade on another take   422 "that clip plays other media…"
clearing A's grade   481a5ddd48fc6c0f   restored byte for byte
```

Gates at the time of the commit: see the commit message.

**What is still not there.** No wheels, curves, 3D LUTs, free matrix, hue
rotation, masks or HSL qualifiers in the interface — all evaluated by the engine,
all named by `grade` with the reason. One primary layer, no layer management, no
node graph, no tracking, no versioned grades or stills, no grade copy/paste, no
scopes in the browser, no hardware panel. The multicam angle grid shows candidate
angles' media and is ungraded, because an angle is not yet the clip on the cut.
