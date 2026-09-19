# Master audit — where CINE-OMNI actually stands

Taken on 2026-09-15 at `77192a8` on `playback-scheduler`, from the code, the
running binary and measurement — not from the documentation, which is cited only
where it was checked. Where an older research finding has since been fixed, that
is said; where one is still true, it was re-verified against the working tree.

**One-paragraph verdict.** CINE-OMNI has an unusually strong *foundation* for a
project of its age — exact time, a reactive project graph with undo and an op
log, crash recovery, a single-owner runtime with live multi-client state, real
decoding, real scheduled playback, and a culture of refusing claims it cannot
back. It has almost none of the *product*. Of the roughly 150 capabilities a
filmmaker would expect across the lifecycle (see the capability matrix), about a
dozen are verified end to end and visible, and nobody could yet write, schedule,
cut, grade, mix or deliver a film in it.

---

## 1. Size and shape

| | Source | Tests | Notes |
|---|---:|---:|---|
| `ucg-core` | 3,521 | 1,942 | graph, tx, undo, op log, reactors, persistence |
| `ucg-schema` | 623 | — | 60+ node kinds, attribute keys |
| `cine-script` | 3,410 | 919 | Fountain, FDX, scenes, dialogue, reports |
| `cine-previs` | 5,329 | 174 | stages, cameras, USDA-shaped export |
| `cine-logistics` | 4,466 | 750 | breakdown, shot list, schedule, call sheets, takes |
| `cine-timeline` | 3,950 | 2,271 | OTIO model, edits, session state, sync, graph |
| `cine-media` | 6,162 | 3,822 | identity, probe, decode, clock, scheduler, decode-ahead, A/V |
| `cine-gpu` | 1,483 | 437 | device, 10 WGSL kernels, CPU mirrors |
| `cine-color` | 9,676 | 1,159 | transforms, LUT, CDL, OCIO subset, scopes |
| `cine-comp` | 6,506 | 1,031 | node graph, 28 node kinds, evaluator |
| `cine-audio` | 6,938 | 1,576 | mixer, DSP, loudness, true peak |
| `cine-pipeline` | 1,001 | 1,742 | the 7 reactors; the film fixture |
| `cine-runtime` | 2,614 | 1,584 | HTTP, WebSocket, RPC, lock, projection, frames |
| `cine-shell` | 3,210 | 370 | native window, planar presentation, egui workspace |
| `cine-cli` | 498 | — | `cine start`, new, import, demo |
| **Rust total** | **~69,400** | **~19,800** | 788 `#[test]` functions (static count) |
| `ui/src` | 1,646 | 3 files | React + Vite, no other runtime dependencies |

36 commits. 12 research dossiers totalling ~18,400 lines, plus 15 blueprint and
status documents.

---

## 2. EXISTING — implemented and verified

Verified means a test or a measurement ran against real input on this machine.

| Capability | Evidence |
|---|---|
| Exact rational time, one time model across the workspace | 25 adversarial tests; 24-hour timelines; drop-frame; a thousand recomputations write zero temporal ops |
| Reactive project graph: typed nodes, edges, cycle rejection, invertible ops, grouped undo | `ucg-core` suite; reactor convergence requires idempotent writers and is tested |
| Append-only op log, snapshot + replay, torn-record detection | 3 SIGKILL tests against the real binary |
| One runtime per project, stale-lock reclamation, second `cine start` defers to the owner | lock tests + end-to-end test against the binary |
| Live state across clients, including undo | real-socket test: A→B, B→A, undo reaching B |
| Screenplay import (Fountain, FDX), scenes, dialogue, reports | `cine-script` suite |
| OTIO *write* that real OTIO parses | research 16, Tier 1 of 5 |
| Editorial operations (ripple, roll, slip, slide, split, join, insert, overwrite, lift, extract, replace), exact and mixed-rate | 93 timeline tests including mixed-rate |
| Editorial session state: lock, mute, solo, target, links, snapping, handles | 19 tests; survives OTIO and the project graph |
| Content-addressed media identity; fast hash refuses to verify | `identity.rs` returns `CannotVerify` for a fast hash |
| H.264 High 4:2:0 8-bit decode (pure Rust), AAC/PCM decode | real asset, 240/240 frames |
| Frame-accurate seek | sought frame pixel-identical to played frame |
| Playback scheduler, decode-ahead, exact drop accounting | 37 scheduler + 23 decode-ahead tests; 144/144 at 24 fps on screen |
| GPU YUV→RGB in a fragment shader | within 1/255 of the CPU reference, BT.709/601/2020 |
| Audio-mastered clock architecture | 23.976 fps paced by 48 kHz exact at all 240 pictures |
| Loudness (BS.1770 integrated), true peak within the EBU budget | agrees with `ebur128` to 0.000 LU; true peak adjudicated against exact reconstruction |
| Application shell served by `cine start`, real cut and real media visible | end-to-end tests + real-socket protocol tests |
| Capability honesty: every capability carries a state and a reason | asserted by test against the running binary |

## 3. PARTIAL — exists, incomplete

| Capability | What exists | What is missing |
|---|---|---|
| OTIO read | reads its own output | real OTIO files rejected or silently lossy (Tier 1 of 5) |
| Colour management | ACES 2.0 tonescale exact on neutrals; OCIO subset; LUT/CDL | cannot load any shipped ACES OCIO config (`shared_views` unimplemented — re-verified); saturated colour off by up to 0.30 |
| Compositing | 28-node graph, typed ports, persistence | **no file I/O** (re-verified), no data window (re-verified), ROI does no work |
| Audio | offline mixer, DSP, loudness | no device, no streaming engine, no plugins, no recording; 128 × 7.1.4 misses the deadline |
| Previs | stages, cameras, USDA-shaped export | no viewport; export is a subset, not OpenUSD |
| Production / dailies | take metadata, shot lists, schedules, call sheets | no offload, no checksum manifests (ASC MHL), no camera report ingest |
| Media | H.264 8-bit 4:2:0 | no HEVC, ProRes, DNx, RAW, EXR, image sequences, 10-bit; no colour metadata parsing |
| Playback | real, scheduled, native | long-GOP cold seek 2.6–4.8 s; decode margin near zero; short-GOP unverified |
| Native workspace | docked panels, real playback at 24 fps | not launched by `cine start`; no editing |
| Browser workspace | shell, cut, media preview, track state | no editing operations reachable; picture preview only |

## 4. UI-INVISIBLE — capability with no path to the interface

These are the direct answer to "the backend has progressed but the product does
not show it":

* **Every editorial operation.** Eleven operations exist and are tested; the
  protocol exposes none of them. `graph::EditOp` itself covers only seven —
  insert, overwrite, replace and join are pure-model operations with no graph
  path at all.
* **Snapping, handles-bounded trimming, links as a behaviour.**
* **Script editing.** `edit_dialogue` exists in the RPC; no interface calls it.
* **The reactor cascade.** The defining feature — a script change propagating to
  previs, logistics, timeline, audio, comp and colour — returns its deltas on
  every commit and is shown nowhere except a status-bar label.
* **Breakdown, shot lists, schedules, call sheets.** In the graph; exported from
  the CLI; not visible.
* **Colour, compositing, audio engines.** Real code, no workspace.
* **Scopes.** Implemented in `cine-color`, not wired to any viewer.
* **Real-time playback.** Exists natively; the browser correctly says it cannot
  have it, and `cine start` does not launch the native workspace that has it.

## 5. MISSING — major capabilities that do not exist

By lifecycle stage; the matrix has the full list.

* **Development:** screenplay *editor*, revision colours and locked pages,
  outline/beat board, notes, collaboration on script.
* **Pre-production:** storyboard drawing and animatics, stripboard scheduling
  with day-out-of-days, budgeting, floor plans, location, production design,
  costume/makeup/hair continuity, equipment and crew planning.
* **3D / CGI:** any viewport, modelling, rigging, animation, simulation,
  rendering, asset library with versioning, OpenUSD stage.
* **Production:** card offload with checksum manifests, live monitoring, LUT
  preview on set, camera/lens metadata ingest, slate/take capture, sound report
  ingest, virtual production.
* **Editorial:** three/four-point editing in the interface, multicam, nested
  sequences, keyframes, effects, captions and subtitles, render cache, EDL, AAF,
  FCPXML, proxy generation.
* **VFX:** image I/O, roto, tracking, keying on real plates, paint, deep/EXR.
* **Colour:** wheels, curves, qualifiers, windows, tracking, stills, shot
  matching, HDR scopes, loadable ACES configs.
* **Audio:** device I/O, real-time engine, automation, plugins, ADR, Atmos/ADM.
* **Delivery:** any renderer or encoder, DCP, IMF, broadcast, streaming specs,
  timed text (TTML/IMSC), QC, archive.
* **Platform:** users, permissions, authentication beyond loopback, remote
  collaboration, background job system, plugin/extension API.

## 6. WRONG DIRECTION — systems unsuited to the full vision

Stated plainly, because building on these would compound:

1. **Browser as the primary editing surface.** A frame cannot reach a browser
   without leaving the GPU; measured at 69 ms per picture on the read-back path.
   The browser is right for review, projects, planning, script and production
   data; it is wrong for picture-critical work. Today there are **two shells**
   (browser and native) and `cine start` launches only the one that cannot play.
   This must converge — see the roadmap, Phase 1.
2. **Takes are `Contains`-children of derived shots.** The behaviour is
   protected by a test, but the edge still says the derived thing owns the
   captured thing. Captured data must not be structurally under derived data.
3. **`cine-comp` images have no data window.** Every later performance property
   of a compositor depends on it, and it is a type change that gets more
   expensive every week.
4. **The OCIO implementation is a parallel parser.** It will be chasing the
   OpenColorIO config format indefinitely. Research 13 identifies an
   Apache-compatible GPU-shaped reference; the policy decision (native config
   parser vs. a sanctioned OCIO build vs. constrained config subset) needs making
   before a colour workspace is built on it.
5. **Synchronous, whole-range audio rendering.** Research 14 is right that the
   path to a real-time engine is a different shape, not an optimisation of
   `render()`.
6. **`cine-shell` owning its own `GpuContext` separate from the surface device.**
   Two devices where there should be one; blocks zero-copy between compute and
   presentation.

## 7. TECHNICAL DEBT — must eventually be refactored

| Debt | Why it matters | Cost of waiting |
|---|---|---|
| Two shells (browser + native egui) with different projections | two UIs to keep honest | grows with every workspace |
| `EditOp` covers 7 of 11 operations | no graph path for insert/overwrite/replace/join | blocks editing from any UI |
| Media decode single-codec, single-reader | every workspace needs media | grows with every format |
| Frame service keeps one open reader | two viewers thrash it | small now |
| `cine-comp::hashes()` superlinear per evaluation | 53 ms at 2,001 nodes | grows with graph size |
| Colour metadata not parsed (`colr`, VUI) | every picture's colour is assumed | correctness, not speed |
| Test helpers duplicated across runtime tests (partly fixed) | drift between copies | low |
| No DOM test tooling for the UI | rendering is untested | grows with UI size |
| `ContentHash` not namespaced by algorithm | identity migration to BLAKE3 blocked | grows with project count |
| `RationalTime` rate chosen from the left operand in `Add` | diverges from OTIO's rule | interchange correctness |

## 8. PERFORMANCE — measured bottlenecks

All on this machine: Intel i3-10110U, 2 cores / 4 threads, 15 GiB, Intel UHD
(CML GT2) over Vulkan. A modest laptop, which is the right floor to design for.

| Path | Measured | Bottleneck | Class |
|---|---|---|---|
| H.264 sequential decode | 39.8–48.5 ms/picture (≈21–25 fps) | pure-Rust decoder, single thread | CPU |
| Cold seek into a 189-picture GOP | 4.8 s | walk from keyframe | CPU / codec structure |
| Browser picture preview | 94–114 ms first, 486 ms step, 2.6 s cold jump | decode + CPU colour + PNG encode | CPU |
| Native presentation | ~3.5 ms/picture | none — was 82.5 ms before planar path | solved |
| Workspace UI draw | ~2 ms/frame added | — | solved (after fixing a redraw spin) |
| Offline mix 48 tracks stereo | 1.8 ms / 10.7 ms budget (quiet) | — | fine |
| Mix 128 tracks × 7.1.4 | 13.6 ms / 10.7 ms budget | per-frame pan/accumulate, no SIMD | CPU |
| `cine-comp` 2048² blur, ROI 64² | 1,369 ms, full frame allocated | no data window | CPU / RAM |
| `cine start` open | revision 38 in under a second; 37 ops replayed | — | fine |
| `cargo test --workspace` | killed by memory at default, `-j 2`, `-j 1` | static test binaries with debuginfo | build RAM |

The pattern: **the platform is fast; the media and processing engines are the
bottlenecks**, and most are architectural (codec coverage, no hardware decode, no
data windows, no streaming audio) rather than tuning problems.

## 9. STORAGE — separated by class

| Class | Size | Notes |
|---|---:|---|
| Source (crates, docs, UI source, fixtures) | **12 MB** | the whole of the actual software |
| Git objects | 6.5 MB | |
| UI build output (`ui/dist`) | 188 KB | reproducible |
| `ui/node_modules` | 76 MB | reproducible from lockfile |
| **`target/` build artifacts** | **39 GB** | reproducible; ~19 GB is stale duplicate artifacts from before the profile change |
| Project data (`~/cine-projects/demo`) | 1.1 MB | graph snapshot + op log |
| Media | referenced, not copied | the 5.5 MB asset lives in `~/Downloads` |
| Caches / render cache / proxies | **0** | none exist yet |

**A 39 GB `target/` is not a 39 GB film project.** The software is 12 MB; the
project is 1 MB; everything else is compiler output that one `cargo clean` would
reduce to the measured ~6.6 GB cold build.

---

# Re-anchor — 2026-09-19, `f8b39d7`

A product-level re-audit against the complete CINE-OMNI specification, taken
from the code, the graph of a real project instance and measurement. The
question asked was not "is the roadmap finished" but **"is the roadmap pointed
at the right destination"**.

## The headline finding

**The product model is not too narrow. The roadmap and the visible surface
are.**

`CINE-OMNI-CAPABILITY-MATRIX.md` already covers the whole lifecycle — 169 rows
from development and IP through to archive, in the nine states this audit
requires, with licensing and effort columns. It does not need rewriting. What
has drifted is everything downstream of it:

| | breadth | evidence |
|---|---|---|
| Product model (capability matrix) | **whole lifecycle**, 169 rows | `CINE-OMNI-CAPABILITY-MATRIX.md` |
| Architecture (graph vocabulary) | **broad**, 65 node kinds across script, logistics, previs, board, nle, colour, audio, comp | `ucg-schema/src/kinds.rs` |
| Protocol surface | **medium**, 76 RPC methods, 9 export formats | `cine-runtime/src/rpc.rs` |
| Roadmap | **narrow** — recent rows are almost all media/editorial | `MASTER-ROADMAP.md` |
| Visible product | **narrowest** — 11 workspaces, 4 explicitly empty | `ui/src/workspaces/index.ts` |

So the corrective action is **not** to redesign the product model, and **not**
to polish the editorial screen. It is to stop the roadmap selecting only from
the media/editorial column, and to start converting *engine capability that
already exists and has no interface* into product.

The matrix's own verdict from 2026-09-15 still holds and is the cheapest
opportunity in the repository: **"the largest single category of *partial* is
engine capability with no interface — which is also the cheapest category to
turn into visible product."**

## A. What the audit confirms is correct, and must not be rewritten

These were checked and are sound foundations for the complete scope:

* **One graph, one project, one runtime.** `OmniProject` owns an engine with
  every reactor installed; `cine start` is the single owner of a project's
  lock, serving UI and WebSocket from one process. A second `cine start`
  attaches rather than starting a rival (`start.rs`), because two op logs would
  corrupt the history.
* **Exact rational time everywhere.** One `RationalTime`/`Rate`, no parallel
  model, with drop-frame arithmetic in exactly one place.
* **Content-derived media identity.** `EntityId::from_key("media:file:<hash>")`
  — a file that moves is the same media, a re-export is not.
* **Derived-state provenance.** One rule for staleness, with `reasons` (out of
  date) separated from `notes` (true without meaning that) and `remakeable`.
* **A truthful capability registry** that gates what any surface may offer, and
  a UI that lists unbuilt workspaces with the reason rather than hiding them.
* **Real jobs** with persistent identity, atomic derived output and validated
  completion.

None of this blocks the complete vision. The graph vocabulary in particular is
extensible by design — adding a department means adding kinds and a reactor,
not changing the engine.

## B. Architectural gaps that *would* block the complete vision

Found by inspection; each is absent rather than wrong.

| # | Gap | Evidence | Blocks |
|---|---|---|---|
| A1 | **No first-class deliverable, rights/IP, budget, research, QC or localization kinds.** The vocabulary stops at post-production. | `kinds.rs` has 65 kinds; none for deliverable, package, rights, budget, cue, ADR line, QC report | Development, Delivery, Localization, Archive |
| A2 | **No `film`/`episode` above `project`.** A project is the root; episodic work has no home. | `kinds::PROJECT` is the only root | Episodic production |
| A3 | **No plugin or external-process architecture.** No `Command::new`, no process boundary, no plugin trait anywhere in `crates/*/src`. | grep returns nothing | Unreal/Houdini/Nuke/Pro Tools interop, RAW SDKs, anything that must stay out-of-process for licence reasons |
| A4 | **No collaboration, identity, permission or auth model.** Multi-*client* exists (live sync over WebSocket); multi-*user* does not. | `bind_host` is `127.0.0.1`; no auth in `server.rs` | Any real production team |
| A5 | **`media_path` conflates a URL with a filesystem path.** The OTIO importer stores `target_url` verbatim into `MEDIA_PATH`, so `file:///media/A001_C003.mov` is later treated as a path by relink, probe and `/frame`. | demo project: 8 of 9 media have `file:///…` in `media_path` | Conform from any external document; this is also the cause of bug **M2** below |
| A6 | **No asset/versioning model for large binaries.** `asset.model` exists as a kind with no pipeline. | `kinds.rs` | Virtual art department, VFX asset dependency |

A3 and A4 are the two that most constrain the ceiling. Neither requires
changing what exists.

## C. The screenshot, traced

All four statements on the current screen were traced to real state. **Two are
correct-but-unexplained; two are defects.**

The demo project contains **two different populations** of `asset.media`, which
the UI does not distinguish:

1. **One truly ingested asset** — `Super_cinematic_image_to_vid.mp4`:
   `online: true`, `media_info` present, a real filesystem path.
2. **Eight OTIO-imported media references** — `A001_C003.mov` and friends:
   `online` **never set**, `media_info` **absent** (never probed), path
   `file:///media/A001_C003.mov`, a URL that has never existed on this machine.

| Screen statement | Cause | Verdict |
|---|---|---|
| Source viewer: *"media is offline: /home/…/Super_cinematic_image_to_vid.mp4"* | The file was deleted or moved after ingest. The decode attempt failed **now**. | Correct |
| Inspector: *"Online at its recorded path"* | `settled = media.online !== false && !r.state` (`Inspector.tsx:686`). The graph still records `online: true` from ingest time; nothing has run `check_media` since the file vanished. | **Defect M1** — the graph's *belief* and the live decode disagree, and the UI shows both without reconciling. |
| Record viewer: *"no media with that id in this project"* | `/frame` → `read_media()` returns `None` for a node with no `media_info`; the handler answers with that string (`server.rs:624`). | **Defect M2** — false. The media *is* in the project; it was never probed. The sibling branch one line below already says the honest thing for proxies: *"that proxy was never probed"*. |
| Timeline still holds clips; project reports 9 media; runtime reports 163 nodes / 201 edges | All true and consistent: clips reference media *nodes*, which exist; nodes ≠ playable media. 163 nodes includes 27 `color.space`, 21 `comp.node`, 9 `asset.media`, 8 `nle.clip`, 8 `color.grade`, 7 `script.dialogue`, 7 `audio.clip`, … | Correct |

**M1 fix**: presence is a live fact and the UI must not present a remembered
one as current. Either resolve on open (a `check_media` pass when a project is
opened, which already exists as an RPC), or mark the Inspector's claim as
*last known* and show when it was established.

**M2 fix**: distinguish "no such node" from "node exists, never probed", and
offer the action that resolves it (probe / relink). One-line message change plus
a regression test; the honest wording already exists three lines away.

**Neither is in the working tree yet** — both are recorded here first, as this
audit is read-only.

## D. Frontend information architecture — the real problem

The current IA is **11 module tabs**, gated honestly by capability
(`workspaces/index.ts`). The gating is good and should stay. The *shape* is the
problem, and adding tabs for twenty more departments would make it worse.

**Why tabs fail for this product.** A filmmaker does not work in a department;
they work on a *thing* — a scene, a shot, a take — and need the department
views of that thing. Tabs force the user to hold the correspondence in their
head, which is precisely the failure of the tool-chain CINE-OMNI exists to
replace. Twenty tabs is a suite in one window, not one application.

**The proposed model: subject × lens.**

* **Subject** — what you are working on, carried across the whole application:
  `Project → Film/Episode → Sequence → Scene → Shot → Take → Asset`. Selecting a
  scene anywhere selects it everywhere.
* **Lens** — how you are looking at it: Write, Board, Previs, Schedule, Shoot,
  Cut, Comp, Grade, Mix, Deliver. A lens changes the tools, never the subject.
* **Relationship rail** — the graph edges of the current subject, shown as the
  product's own navigation: this scene's script lines, boards, previs shots,
  takes, media, timeline uses, VFX shots, grades, stems, deliverables. This is
  the differentiator made visible, and it is the one thing no competitor can
  copy without a unified graph.

The graph already has the edges to power this; `structure` (script scene → shots
→ takes → media) is an existing RPC that reads them. The rail is not new
architecture — it is an interface for a relationship the project already holds.

**Quality bar.** Density with hierarchy, not density as clutter: one type
family at 3–4 sizes, a strict spacing scale, monospace reserved for exact values
(timecode, hashes, rates), colour carrying meaning only (state, track kind,
refusal), full keyboard command surface, and a single command palette over the
76 RPC methods. No cards, no shadows, no dashboard tiles, no progress theatre.

## E. Startup and localhost — audited, and correct as designed

* **Why the checkout is rejected:** `is_project_dir()` requires `graph.json`,
  `graph.msgpack` or `oplog/`. The repo is the *application*; a project is a
  *document directory*. This separation is right and must stay.
* **Project discovery, lock, recovery:** `cine start <dir>` canonicalises,
  validates, reads a live lock and reports an existing runtime rather than
  starting a rival; `--isolated` exists with a stated warning.
* **UI discovery:** `find_ui_dir` checks `<project>/ui/dist`,
  `<project>/../ui/dist`, then the repo root derived from the executable path —
  which is what lets a project stored anywhere find a UI built in the checkout.
* **Ports:** default 7331, falls back to any free port if taken.
* **Binding:** `127.0.0.1` only. **LAN access is not implemented and must not
  be claimed.** There is no authentication, so exposing the port would publish
  the project to the network.
* **One command:** already true — `cine start` from a project directory does
  open, lock, serve UI, serve WebSocket, and report the URL.

**Development vs end-user mode.** The Vite dev server has no proxy, and the UI
fetches `/frame`, `/health` and render artifacts relative to the page origin, so
`npm run dev` on :5173 yields an interface with no pictures. A
`VITE_CINE_BRIDGE` override exists for the WebSocket only. `ui/server/bridge.mjs`
is **superseded and must not be used** — its own header records that it spawns
one engine per browser client, which is the two-op-log divergence `cine start`
exists to prevent. If a hot-reload path is wanted, the correct fix is a Vite
proxy to the running runtime — not a second runtime.

## F. Storage — measured, and the finding is stale artifacts, not bloat

| Path | Size |
|---|---:|
| `target/` | 41 GB |
| ↳ `target/debug/deps` | 41 GB |
| ↳ `target/debug/incremental` | 527 MB |
| `ui/node_modules` | 76 MB |
| `crates/` (all source) | 6.4 MB |
| `docs/` | 1.7 MB |
| `.git` | 19 MB |

362 executables in `deps`, **139 of them over 100 MB** — including three
separate generations of the `cine` binary (246 MB, 237 MB, 226 MB). The cost is
test binaries carrying full debuginfo, and cargo never collects old hashes.

This is the *cache/build* class under `STORAGE-POLICY.md` and is safe to prune,
but **there is no measured reason to**: 135 GB free. The correct mechanism when
there is one is a targeted prune of artifacts whose hash is no longer current —
never `cargo clean`, and never while a build is running. Source, project, media
and derived data are untouchable regardless.

## G. Product-level priorities

Ordered by dependency unlock × correctness risk × breadth of lifecycle
unblocked — deliberately **not** by which is easiest to show.

* **P0 — Truthfulness repairs (small, immediate).** M1 and M2 above. The
  product's core claim is that it never states something it cannot back;
  two screens currently do.
* **P0 — The relationship rail.** Make the graph's edges the navigation. This
  converts the existing `structure` RPC into the product's differentiator and
  costs no new engine work.
* **P1 — Vocabulary extension for the missing lifecycle ends** (A1, A2):
  deliverable, package, rights, budget, QC report, localization, cue, episode.
  Kinds plus reactors, no engine change. This is what lets Development and
  Delivery exist at all.
* **P1 — Surface the engine capability that already has none.** Named by the
  matrix: `comp.graph`/`comp.node` (21 comp nodes in the demo project and no
  interface), `board.*`, `previs.*`, `color.grade` (8 grades, no grading
  surface), `audio.*`. Each is *partial engine, missing interface* — the
  cheapest real product in the repository.
* **P1 — `media_path` as a resolved locator** (A5), which also fixes conform
  from external documents.
* **P2 — External-process architecture** (A3). The ceiling on Unreal, Houdini,
  Nuke, Pro Tools, RAW SDKs and anything licence-bound.
* **P2 — Collaboration, identity and permissions** (A4), and only then any LAN
  or remote claim.

## H. Dependency-ordered implementation plan

1. M2 (message + test), then M1 (presence resolved on open, or stated as last
   known with its time).
2. `media_path` becomes a locator that distinguishes URL from path (A5),
   unblocking honest conform from OTIO/EDL/AAF.
3. Relationship rail over `structure`, with subject selection shared across
   panels — one subject model in `session.ts`.
4. Vocabulary extension: `deliverable`, `package`, `episode` first, because
   Delivery is the end of every workflow already built.
5. First department conversion, cheapest-real-product first: **colour**, which
   has a verified engine (OCIO, CDL, exact scene-linear view transforms, 8
   grades in a real project) and no grading surface.
6. Then compositing, which has a node model and no canvas.
7. External-process boundary, then collaboration.

Steps 1–2 are correctness. Steps 3–4 are architecture. Steps 5–6 are the first
proof that the "one application" claim is real rather than aspirational.

## I. What this audit does not change

The capability matrix, the licensing spine, the storage policy, the module
conventions and the architecture record stand. Nothing in this re-anchor asks
for existing code to be rewritten. The engineering foundation is the asset; the
deficit is product coverage, information architecture and visible integration.
