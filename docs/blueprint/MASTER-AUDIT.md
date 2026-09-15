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
