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

> **Selection rule (re-anchored 2026-09-19).** This table records what has been
> built; it is **not** the list of what to build next. The destination is
> `CINE-OMNI-CAPABILITY-MATRIX.md` — the whole lifecycle, 169 rows — and the
> selection rule is the priority order in `MASTER-AUDIT.md` § "Re-anchor".
> Recent rows here are almost entirely media and editorial; that is drift, not
> the plan. The cheapest real product in the repository is engine capability
> that already exists and has no interface (colour, compositing, boards,
> previs, audio), and the two correctness repairs M1/M2 come before any of it.


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
| Phase 7 — primary grading | ASC CDL per clip, applied in the working space and baked into the view lattice the window interpolates; measured against the exact transform | `view_lattice.rs`, `colour_protocol.rs` |
| Phase 6 — EDL export | the cut as CMX3600, with source and record timecode, reel names and clip-name comments; what the format cannot carry is written into the file | `edl.rs`, `editorial_protocol.rs` |
| Phase 6 — three- and four-point editing | any three of source in/out and record in/out is an edit and the fourth point is arithmetic; backtimed edits end on their mark; the plan says which point was derived; four marks that disagree are refused with the difference in frames until the editor chooses how to settle them, because fit-to-fill would need a time effect the model does not have | `three_point.rs`, `three_point_edits.rs`, `editorial_protocol.rs` |
| Phase 6 — retiming (constant speed) | a retime is the media consumed and the track time it takes, both exact; it ripples the cut, shows as a badge, resolves the right picture in the record monitor, writes an `M2` record into the EDL and a `LinearTimeWarp` into the OTIO; trims, rolls and joins refuse it rather than guess; fit to fill is the fourth way to settle four marks | `retime.rs`, `editorial_protocol.rs` |
| Phase 6 — multicam angles | an angle is one of the clip's own media references, so a multicam clip is not a special kind of clip; every angle is bound to the clip in the graph, a switch splits where it was asked for and plays the new angle from there, and an angle that was not rolling over that part of the take is refused with the shortfall instead of cut to black | `multicam.rs`, `editorial_protocol.rs` |
| Phase 6 — captions | authored per sequence and language in the editor, checked against delivery rules by the runtime, imported from SRT with what moved onto a frame reported, exported as IMSC with frame-exact times or as SRT/WebVTT; an independent TTML reader (ttconv) decodes the IMSC onto exactly the frames written, and found a reader bug on the way | `captions.rs`, `captions_oracle.rs`, `captions_graph.rs`, `editorial_protocol.rs` |
| Cross-department — captions ↔ dialogue | a caption links to the line it transcribes through the scenes bound to the clips under it; rewriting, cutting or re-cutting flags it with the reason, a line that only moved is followed, both directions are visible, and review of 1,200 linked captions costs 49 ms | `caption_link.rs`, `caption_script_link.rs`, `script_protocol.rs` |
| Phase 6 — retimed sound and speed ramps | retimed sound plays as varispeed or with its pitch kept (a memoryless phase vocoder, so seeks match playback), files at other rates are converted by the same kernel, and a speed can ramp: one exact time map read by every player, typed speeds parsed exactly, the ramp deciding both lengths | `sequence_audio_speed.rs`, `stretch.rs`, `time_map.rs`, `retime.rs` |
| Phase 6 — multicam sync and the four-up view | angles carry offsets against the take so a switch lands on the same moment in the other camera; syncing by sound is a job that accepts a match only when the correlation peak and the loudness agreement both pass — the peak alone would have synced five of twelve strangers; the record monitor shows four real decoded angles and cuts on click | `multicam.rs`, `multicam_sync.rs`, `editorial_protocol.rs` |
| Phase 4 — script revisions | issuing pages snapshots the script; marks are the difference from it, so an edit and its undo both do the right thing; the industry colour sequence, with marks in the margin and the current colour in the header | `revision.rs`, `script_protocol.rs` |
| Phase 4 — breakdown tagging | cast and props added or struck by hand, flags overruled or handed back; tags live on the scene so a rewrite re-derives around them | `production_protocol.rs` |
| Phase 4 — call sheet PDF | one page per day, written without a PDF dependency, verified by reading it back with poppler and parsing it with Ghostscript; served at `/callsheet?day=N` and exported from the command line | `callsheet_pdf.rs`, `production_protocol.rs` |
| Phase 4 — manual strip moves | a scene pinned to a day stays there; the scheduler fills around it, a pin that overfills a day is honoured and warned about, and undo releases it | `schedule.rs` pin tests, `ui/e2e/production.mjs` |
| Phase 4 — day out of days | the cast side of the stripboard: SW/W/H/WF/SWF per actor per day, work and hold totals, in the workspace, the protocol and an export | `dood.rs`, `production_protocol.rs` |
| Phase 8 — sound (pulled forward) | ALSA device via runtime-loaded libasound; the cut's sound decoded, mixed and played in the native window, pacing the picture | `AV-SYNC.md` |
| Phase 3 — media capability audit | two tables state every capability (codecs, and 27 whole media paths with verified/implemented/partial/prototype/refused/missing/blocked); the `capabilities` method reads them, so no interface can claim what the code cannot do, and a verified row must name its test | `cine-media/src/capability.rs`, `APPLICATION.md` |
| Phase 3 — ingest as a job | importing is `media.ingest`: discover, identify by content, probe, evaluate against the capability registry, register in one transaction, then dispatch thumbnail/waveform (default) and proxy (only when asked) as jobs; cancellable before registration, and nothing half-registered after a failure | `ingest_protocol.rs`, `APPLICATION.md` |
| Phase 3 — derived provenance audit | the whole matrix in one test: moved media, deleted media, namesakes, real content changes, damaged derived files, older recipes, and edits inside and outside a render's range. A fact about a file is staleness only where it contradicts what the project claims; otherwise it is a note, and `remakeable` says whether the work could be done again | `derived_provenance.rs`, `render_movie.rs`, `APPLICATION.md` |
| Phase 3 — the import-to-watching chain | one test takes a file from a card to a proxy being watched: an import that asks for a proxy dispatches it, everything hangs off the one asset, the cut reads the proxy, a namesake at the path makes the original unreadable without disturbing anything derived, and the file's return resolves it again without remaking a thing. It found `/frame` serving a stranger's picture from a path the project had already recorded as `replaced` | `ingest_proxy_workflow.rs` |
| Phase 3 / 5 — source timecode | a QuickTime `tmcd` track is read and written: the exact rate as the file's ratio, the drop-frame flag and the frame number, decomposed by the one implementation of drop-frame rules. Absent is never zero; a proxy carries its original's timecode; the inspector and the viewer read it out. Held to the published byte layout by a second reader in the test, because no independent timecode reader was available to check it | `timecode_track.rs`, `tmcd.rs`, `APPLICATION.md` |
| Phase 5 — conforming against source timecode | an EDL's source columns are the camera's numbering at 24, 25, 30, 23.976, 29.97 non-drop and drop-frame and 59.94, with a clip starting at a non-zero offset inside a take whose timecode is not zero; FCM declares the naming every label in the file is written in; a counter or a discontinuous track counts from zero and the file says why. Limits recorded, not hidden: no record timecode on a render, and OTIO still writes an available range starting at zero | `timecode_conform.rs`, `edl.rs`, `APPLICATION.md` |
| Phase 5 / 10 — the cut's own clock | a sequence has a record start, offered rather than assumed and read at the sequence's own rate (29.97 drop-frame and non-drop are 108 frames apart, and a skipped label is refused rather than rounded); an EDL's record columns count from it and a rendered movie carries it as its own timecode track, advanced by the frame the render begins on | `timecode_conform.rs`, `APPLICATION.md` |
| Phase 5 — OTIO in the media's own numbering | an OTIO file is written and read in timecode, as OTIO is read everywhere else, while source ranges stay media-relative inside the build; the conversion is at the edge of the file, and importing normalises a cut written by another tool instead of leaving it an hour out | `timecode_space.rs`, `timecode_conform.rs`, `APPLICATION.md` |
| Phase 3 — Matroska described from the file | the track list is read from the EBML header rather than from an audio demuxer that called every track sound: picture tracks get codec, pixel size and an exact frame rate, sound tracks their stated rate and channels, and a track stating neither records neither. It removed a phantom 48 kHz stereo stream the probe had been inventing for every MKV picture track | `matroska_header.rs`, `ebml.rs`, `APPLICATION.md` |
| Phase 3 — media too large to hold | identity and probing never read the file (measured: 536.9 MB identified in 91.8 ms, 1.4 MB of resident growth); a picture larger than this machine's budget is refused by name before anything is allocated, rather than ending the application | `large_media.rs`, `cine-media/src/budget.rs` |
| Phase 8 — waveforms | a recording's envelope decoded once and kept as peaks at four zoom levels, minimum and maximum per channel; drawn under sound clips from each clip's own media range; belongs to its original, stale with its content, survives a relink; one shared definition of derived state | `waveform_protocol.rs`, `derived.rs`, `APPLICATION.md` |
| Phase 7 / 10 — scene-linear view and EXR delivery | the view transform is chosen by the input's encoding: a measured lattice for display-referred pictures, the exact processor for scene-linear ones; scene-linear delivers through a view or as float EXR, and refuses to be clamped as decoded; HDR views work and are labelled as code values rather than an HDR presentation, because the surfaces are SDR | `scene_linear_view.rs`, `sequence_protocol.rs`, `APPLICATION.md` |
| Phase 3 / 9 — image sequences and high-bit-depth input | a numbered run of EXR/PNG/TIFF is one asset with its own rate, identity and holes; EXR reaches the pipeline as unclamped 32-bit float and 16-bit stills as 16-bit; one abstraction (`Pictures`) reads movies and runs alike; delivering scene-linear output refuses rather than clipping | `image_sequence.rs`, `sequence_protocol.rs`, `APPLICATION.md` |
| Phase 3 — relinking | media found by content, never by name: moved and renamed media relinks and keeps its identity, a namesake is refused with both hashes, duplicates are reported, unresolved assets say why; a verified checksum makes a relink exact | `relink_protocol.rs`, `ui/e2e/relink.mjs` |
| Phase 3 — proxies | proxies made as jobs from originals, picture for picture and sample for sample, registered as derived and never as assets; one session-wide choice (Originals/Proxies/Auto) decides what every surface reads, per clip, with the reason; stale or missing proxies are never read; renders always read originals; media presence is checked on request | `proxy_protocol.rs`, `ui/e2e/proxy.mjs`, `APPLICATION.md` |
| Phase 10 — delivery (pulled forward) | the cut renders to PNG frames (8/16-bit) or a QuickTime movie with its sound, sample-exact, at an explicit size and fit, as decoded or through the project view, with captions; renders know whether they are still current by what they depended on; the Jobs tab renders, previews, downloads and shows refusals and staleness | `render_movie.rs`, `mov_roundtrip.rs`, `ui/e2e/render.mjs`, `APPLICATION.md` |
| Wrong-direction corrections | takes owned by the production log, not shots (migrated at open); compositing data windows, union merges and windowed transforms; one GPU device for compute and presentation; a failed cascade stays undoable; zero rate refused; same-length rewording reaches the cut; TCP_NODELAY | commits since `6d8fd67` |
| Claims corrected on inspection | display-referred view transforms were parsed as empty (a whole view rendered wrong, silently); Rec.2100 PQ/HLG clamped where OpenColorIO mirrors; `ExponentTransform` styles ignored; Canon adapted with Bradford instead of CAT02; the shell's capability list still said colour was unverified and sound absent | the commits that fixed each |

### What is not here

The measurements on this page were taken with H.264 test media that is no
longer on this machine, so the playback, A/V and colour end-to-end numbers are
the recorded ones and cannot currently be re-run. Everything else — the
colour oracles, the compositor, the production office, the offload, the EDL —
is tested without media and runs on any machine.

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
