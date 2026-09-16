# Master roadmap

Ordered from the evidence in `MASTER-AUDIT.md`, the gaps in
`CINE-OMNI-CAPABILITY-MATRIX.md` and the strategy in `PRODUCT-STRATEGY.md`.

**The ordering rule.** Each phase must leave the application visibly more capable
from `cine start`, and each is ordered by (1) what it unblocks, (2) how much of it
already exists in the engine, and (3) how strongly it uses the thing no incumbent
has: one project whose changes show their consequences across departments.

**The working rule for every phase** (now in `CLAUDE.md`):

```
backend → protocol → frontend state → visible workspace → interaction → test → measurement
```

No phase is complete because its tests pass. It is complete when a user can do the
thing from `cine start`, and the measurement is written down.

**Estimates** assume the current pace of one principal engineer with AI
assistance, on the measured laptop. They are ranges, and the later ones are less
reliable.

---

## Progress (2026-09-16)

The phases below are the dependency order, not approval gates. What has landed
since the audit, each with its evidence in the named document or test:

| Area | Landed | Evidence |
|---|---|---|
| Phase 1 — editor | all 11 edits through the graph, linked edits, place/insert/overwrite, record picture, impact by department, shared history, script line editing and scene binding; UI tools, two monitors, impact/history/script panels | `APPLICATION.md`, `editorial_protocol.rs`, `ui/e2e/editor.mjs` |
| Phase 2 — one application | `cine start` opens the native window in the same process; it plays the flattened cut, follows commits, shares the open sequence with the browser | `APPLICATION-RUNTIME.md` §6a |
| Phase 2 — schema | op-log records stamped; newer schema refused; migration table enforced | `engine_tests.rs` |
| Phase 3 — jobs | `cine-jobs` (priority, resources, dependencies, cancel, retry, journal/resume); checksum verification and thumbnails as jobs; jobs panel | `jobs.rs`, `jobs_protocol.rs` |
| Phase 3 — colour metadata | H.264 SPS/VUI parsed; pictures carry what the stream declares | `sps_real.rs` |
| Phase 7 — colour management (pulled forward) | ACES 2.0 output transform ported from aces-core (31 displays, both directions); the released cg and studio ACES configs checked against OpenColorIO end to end — 88/88 views, 158/160 conversions; display-referred view transforms, Rec.2100 PQ/HLG, pass-thru exponents, Canon CAT02, Apple Log and ADX fixed or added on that evidence | `aces2_oracle.rs`, `released_configs.rs`, `licenses/THIRD-PARTY.toml` |
| Phase 7 — colour in the viewer | media gets an input colorspace from the config's file rules with provenance; the native window shows the cut through the chosen display and view (33³ lattice, measured against the transform and against the CPU); a colour workspace chooses the view and each file's colorspace, shared across surfaces | `ui/e2e/colour.mjs`, `planar_present.rs`, `view_lattice.rs`, `colour_protocol.rs` |
| Phase 3 — EXR and image sequences | OpenEXR read and write with both windows, layers by name, `%04d`/`####` sequences scanned rather than guessed; comp `Read`/`Write` per frame, writing opt-in. Reading a third-party HDRI matches Blender's own read exactly | `exr_oracle.rs`, `exr_graph.rs` |
| Phase 5 — offload (pulled forward) | `cine offload` copies a card hashing as it reads, fsyncs, re-reads and compares, and writes an ASC MHL generation; `cine verify` checks a folder against its newest list and records the check as the next generation | `cine-media/tests/mhl.rs` |
| Phase 7 — scopes | waveform and vectorscope of the picture as shown (same conversion, same lattice), from a sixteenth of the pixels, measured while the panels redraw | `cine-shell/src/scopes.rs` |
| Phase 4 — manual strip moves | a scene pinned to a day stays there; the scheduler fills around it, a pin that overfills a day is honoured and warned about, and undo releases it | `schedule.rs` pin tests, `ui/e2e/production.mjs` |
| Phase 4 — day out of days | the cast side of the stripboard: SW/W/H/WF/SWF per actor per day, work and hold totals, in the workspace, the protocol and an export | `dood.rs`, `production_protocol.rs` |
| Phase 8 — sound (pulled forward) | ALSA device via runtime-loaded libasound; the cut's sound decoded, mixed and played in the native window, pacing the picture | `AV-SYNC.md` |
| Wrong-direction corrections | takes owned by the production log, not shots (migrated at open); compositing data window; a failed cascade stays undoable; zero rate refused; same-length rewording reaches the cut; TCP_NODELAY | commits since `6d8fd67` |

The audit's "OCIO parallel parser" item is decided as research 13 §7
recommends: the native parser stays, the rendering is ported from the Academy's
Apache-2.0 reference, and a real OpenColorIO is a developer-side oracle whose
outputs on the released configs are committed and compared on every test run —
so drift from the format fails a test instead of shipping.

The audit's wrong-direction list is closed. The last three: compositing merges
now produce the union of their data windows and transforms produce their
transformed window (so nothing moved is lost, and a region bounds the work);
compute runs on the window's own device, adopted rather than opened a second
time. Earlier: takes owned by the production log, the data window itself, the
single application shell, and synchronous whole-range audio.

---

## Phase 0 — Foundation *(done)*

Exact time · project graph with undo and op log · crash recovery · one runtime,
live multi-client state · media identity · H.264 decode · scheduled playback at
24 fps · planar GPU presentation · audio-mastered clock · editorial model and
session state · application shell with real cut and real media · honest
capability reporting. Evidence: `MASTER-AUDIT.md` §2.

---

## Phase 1 — The editor, and what an edit changes  ← **next**

**Objective.** Turn the visible timeline into a working editor, and make the
cross-department consequence of a change visible. Table stakes and the
differentiator in one slice, both almost entirely backed by engine code that
already exists and is tested.

**Backend**
* Complete `cine_timeline::graph::EditOp`: add `Insert`, `Overwrite`, `Replace`,
  `Join` — the four operations that exist as pure-model functions but have no
  graph path (audit §4).
* Linked edits: an edit to a linked clip applies to every clip in its link group
  inside the **same transaction**, so undo reverts both.
* Expose `bind_clip_to_scene` / `clips_synced_to` (engine exists, no protocol).
* An `impact` projection over a revision: every entity the cascade touched,
  grouped by department, with the reactor's own `dirty_reason` /
  `needs_conform` reason text. No inference in the browser.
* A `history` projection over the op log: labels, revisions, who (client agent).

**Protocol** (all through the single `rpc::dispatch`)
* `edit { track, op }` → one transaction → cascade result → broadcast.
* `bind_clip { clip, scene }`, `unbind_clip`.
* `impact { revision }`, `history { limit }`.
* `edit_dialogue`, `undo` and `redo` already exist in `rpc.rs` and gain their
  first UI callers.
* **The binding matters.** `TimelineReactor` flags only clips bound to a scene
  with `SyncedTo` (`clips_synced_to`). The imported OTIO demo cut is bound to
  nothing, so without `bind_clip` a script edit reaches no clip at all — the
  slice must include binding, not assume it.

**Frontend**
* Timeline tools: select, ripple trim, roll, slip, slide, razor, lift, extract,
  insert/overwrite from a source clip; drag and keyboard; snapping; the applied
  delta (after clamping) shown, because trims clamp to handles.
* Edit-point frame: the exact picture either side of the cut, from `/frame`.
* Script panel: the scene's lines, editable, calling `edit_dialogue`.
* **Impact panel**: after every commit from any client, what it affected —
  the clips flagged for conform with the reactor's reason, the scene statistics
  recomputed, alignments marked stale, and — *only when the scheduler's inputs
  actually changed* — the shot list, schedule days and call sheets rewritten.
  Logistics has a fingerprint fast path, so a wording change that leaves the
  estimated duration alone correctly shows no schedule impact.
* History panel: the shared undo history, with undo/redo.

**Shared infrastructure.** Drag-to-delta conversion in exact ticks (the browser
sends *which* frame, never a float seconds value); multi-op transactions in the
engine; impact grouping by `kind.module()`.

**Visible result.** Open the app. Bind two clips to scene 1. Drag a clip edge: the
cut ripples in both windows. Edit a line of dialogue in scene 1: the impact panel
in *both* windows shows the two bound clips flagged for conform with the reason and
the script-side consequences (plus schedule consequences when the duration moved),
and the clips turn amber on the timeline.
Undo from the second window: everything reverts everywhere.

**Tests**
* Every `EditOp` over a real socket: applied, broadcast, projected, undone,
  redone — against the operation semantics in `research/16-editorial-operations-interchange.md`.
* Linked edit moves the pair; undo restores both.
* Script edit → bound clip flagged with the exact reason; unbound clip untouched.
* Impact projection lists only entities in the change set (no inference).
* UI logic: pixel → exact tick conversion; tool state machine; no local project
  state written (asserted by inspecting the store).
* End-to-end against the real binary: edit, restart, edit persisted.

**Performance gates** (measured, quiet machine)
* Edit commit → broadcast → both clients refetched and redrawn: target <100 ms
  at the fixture size; measure at 1,000 clips too.
* Timeline drag at 60 fps with 1,000 clips; decisions counted in work (redraws per
  drag frame), not only wall-clock.

**Storage impact.** None beyond op-log growth (bytes per edit measured and
recorded).

**Dependencies.** None new.

**Acceptance.** All of the visible result, performed by hand from `cine start`,
with the measurements recorded in `APPLICATION.md`, and the matrix rows for
insert/overwrite, ripple/roll/slip/slide, links, markers and cross-department
propagation moved to VERIFIED.

**Estimate.** 3–5 weeks.

---

## Phase 2 — One application

**Objective.** Resolve the two-shell problem before a third workspace is built on
either. `cine start` becomes the whole application.

* **Backend:** the native workspace becomes a *client of the runtime* (same
  protocol, same projections) instead of opening a media file on its own;
  `cine start` supervises it; the frame service becomes a shared decode service
  with more than one open reader.
* **Frontend:** the native workspace hosts picture-critical panels (viewer,
  playback, later scopes); project, script, planning and review panels stay web
  components usable in both.
* **Shared:** one GPU device for compute and presentation; schema versioning and
  migration for project files (cheap now, impossible later).
* **Visible:** `cine start` opens one application window with real-time playback
  of the cut and the same live state as a browser client.
* **Tests:** launch/attach/shutdown supervision; native client and browser client
  agree on every projection; old-schema project opens.
* **Gates:** playback 24 fps with the full workspace (currently measured without
  the runtime connection); startup <3 s.
* **Estimate:** 6–10 weeks.

## Phase 3 — Jobs and media breadth

**Objective.** The infrastructure most of the remaining matrix is blocked on.

* **Backend:** a job system (bounded per resource class, cancellable, resumable,
  persisted, observable); colour metadata parsing (`colr`, VUI, mastering
  display); EXR and image sequences with data windows; proxy generation as jobs;
  capability registry per *path* (decode/metadata/colour/time/edit/process/
  render/export).
* **Frontend:** job queue panel; media browser with real metadata and thumbnails
  from real decodes; proxy/original toggle.
* **Visible:** import a folder of media and an EXR sequence; watch proxies build
  in the background; cut on proxies, play on originals.
* **Licensing:** OpenEXR (BSD-3) or pure-Rust `exr`; decide HEVC/ProRes route
  (LGPL-only FFmpeg as a separate process, or OS codecs) with review.
* **Estimate:** 8–12 weeks.

## Phase 4 — The production office

**Objective.** Own the pre-production slice nobody connects.

* Screenplay editor with pagination parity, revision colours, locked pages,
  A-scenes; FDX/PDF export.
* Breakdown tagging UI; editable stripboard; Day Out of Days; call sheet PDFs.
* Captured-vs-derived schema constraint (takes no longer `Contains`-children of
  shots).
* **Visible:** revise a scene; see the revision colour, the breakdown change, the
  stripboard strip length change, the call sheet regenerate and the bound clips
  flag for conform — the full cross-department story.
* **Estimate:** 10–16 weeks.

## Phase 5 — Set to edit

* Verified offload with ASC MHL manifests; camera/lens/sound metadata ingest;
  take status vocabulary; multi-device take reconciliation; dailies render.
* **Visible:** offload a card, verify it, see takes appear under shots with
  metadata, open them in the editor.
* **Estimate:** 8–12 weeks.

## Phase 6 — Editorial depth

* Three/four-point editing with a source viewer; multicam; nested sequences;
  keyframes and basic effects; captions (TTML/IMSC authoring); render cache using
  UCG dirtiness; EDL; OTIO Tier 2–4; AAF (clean-room, `cfb` crate, never LibAAF).
* **Visible:** cut a multicam scene, caption it, round-trip to Pro Tools via AAF.
* **Estimate:** 4–6 months.

## Phase 7 — Colour

* OCIO policy decision and implementation (load shipped ACES configs); scopes
  wired to the viewer; wheels, curves, qualifiers, windows with tracking; CDL
  round-trip; HDR surface and scopes.
* **Visible:** grade the cut against a loaded studio config with live scopes.
* **Gate:** OCIO oracle conformance across all shipped ACES configs. *Met for
  views and colorspace conversions (2026-09-16); file rules, viewing rules and
  named transforms are still listed as unimplemented keys.*
* **Estimate:** 4–6 months.

## Phase 8 — Sound

* Audio device (runtime-loaded system library, after licence review); streaming
  real-time engine; automation; clip gain/fades on the timeline; loudness meters;
  CLAP/VST3 hosting; ADM BW64 with the EBU renderer.
* **Visible:** mix dialogue, music and effects to picture, in sync, with measured
  offset, and export loudness-compliant stems.
* **Gate:** 128 tracks × 7.1.4 inside the deadline on a workstation; 48 × stereo
  on the laptop.
* **Estimate:** 4–6 months.

## Phase 9 — VFX

* Data windows and image I/O (if not finished in Phase 3); node graph editor;
  roto; 2D/planar tracking (libmv); keying on plates; paint; OpenFX host.
* **Visible:** key, roto and track a plate from the cut, round-trip the shot.
* **Estimate:** 6–9 months.

## Phase 10 — Delivery

* Render/encode jobs; mezzanine masters; web presets (patent review first);
  IMF App 2E (Photon-clean); SMPTE DCP (OpenJPEG); timed text; automated QC;
  archive packages with manifests.
* **Visible:** deliver the cut as a Photon-clean IMF and a DCP.
* **Estimate:** 4–6 months.

## Phase 11 — Collaboration and security

* Authentication, roles and permissions; locking; review links with timecoded
  notes; remote runtime access with transport security; audit trail from the op
  log.
* **Estimate:** 3–5 months.

## Phase 12 — 3D and previs

* Previs viewport with real lens/sensor maths; OpenUSD (after licence review);
  asset versioning; Blender bridge as a separate process; renders arriving as
  plates with provenance.
* **Estimate:** 6–12 months for previs and bridge; native DCC depth is not on
  this roadmap.

## Phase 13 — Platform

* macOS and Windows builds and CI; installer signing; extension/scripting API;
  accessibility; localisation; long-form scale benchmarks (hundreds of thousands
  of nodes).
* Runs alongside Phases 6 onward rather than after them.

---

## Why Phase 1 is next, and not something else

| Candidate | Why not first |
|---|---|
| Shell convergence (Phase 2) | Foundational, but invisible to a user as progress; the protocol and graph work of Phase 1 is surface-independent and carries straight into it |
| Job system / media breadth (Phase 3) | Unblocks much, but shows little until a workspace consumes it |
| Colour / audio / VFX engines | Real code exists but the gaps are deep; would compete with Resolve on its strongest ground first |
| Screenplay editor (Phase 4) | Central to the strategy, but a script editor is a large UI build; Phase 1 already proves script → cut propagation with the existing line editing |
| Audio device | One capability, blocked on a licence review |

Phase 1 is the shortest distance from verified engine code to a user doing real
work — *editing a film and seeing what the edit changes* — and it is the first
time the product's reason to exist becomes visible rather than documented.
