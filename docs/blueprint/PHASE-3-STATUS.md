# Phase 3 — Status, Matrices and Decisions

**What this document is.** The auditable state of CINE-OMNI after the Phase 3
research and hardening pass: what is verified, what is merely implemented, what
is planned, and what is blocked. It is the index the other blueprint documents
hang from, and the place a claim must be justified before it may be made
anywhere else.

## 0. Status vocabulary

Every claim in this repository carries exactly one of these. They are not
interchangeable and green is never given for the existence of a type or an
interface.

| Mark | Meaning | Bar |
|---|---|---|
| 🟢 **VERIFIED** | Proven here, against the real thing | A test or measurement on this machine, named |
| 🟢 **IMPLEMENTED** | Code exists and is exercised | Tests pass, but not against an external oracle |
| 🟡 **EXPERIMENTAL** | Works, but the ground is unproven | Beyond the vendor's stated scope, or one asset only |
| 🟡 **PARTIAL** | Some of it works, and the gap is named | The missing part is written down |
| 🔵 **PLANNED** | Designed, not built | A design exists that survives review |
| 🟠 **BLOCKED** | Cannot proceed here | The specific blocker is named |
| 🔴 **NOT IMPLEMENTED** | Absent | — |
| ⚖️ **LEGAL REVIEW REQUIRED** | An engineer must not decide this | — |

---

## 1. What changed in Phase 3

Research across seven domains found **seven real defects in shipped code**.
Fixing them took priority over new features, because a defect that corrupts or
deletes work costs more than a missing feature. Every fix has a test that fails
without it.

| # | Defect | Severity | Status |
|---|---|---|---|
| 1 | Shot-list regeneration **deleted captured takes** — 3 destroyed by one previs edit | Data loss | 🟢 Fixed |
| 2 | `ContentHash::verify()` passed a file with a bit flipped mid-file | False verification | 🟢 Fixed |
| 3 | OTIO round-trip **relabelled a proxy as the camera original** and dropped the original | Data integrity | 🟢 Fixed |
| 4 | Butt-spliced audio clips rendered a sample apart — a click at every dialogue edit | Correctness | 🟢 Fixed |
| 5 | Grading an additive element (glow, flare, fire) erased it | Correctness | 🟢 Fixed |
| 6 | USD lens units wrong by 100× — depth of field unusable, FOV still looked right | Correctness | 🟢 Fixed |
| 7 | P3-limited HDR output silently answered with the Rec.2020-limited transform | Silent wrong colour | 🟢 Fixed |

Defects 1, 2 and 3 share a shape worth naming: **each produced a confident,
plausible, wrong answer rather than an error.**

A later finding turned the same lesson on the project's own records. The
true-peak defect logged in Phase 3 was itself a confident, plausible, wrong
answer: it had been established by comparing against `ebur128` and assuming the
reference was right. Adjudicating both against an exact reconstruction showed
the divergence was real but the blame reversed. **A cross-check is not an
oracle**, and a defect report deserves the same standard of evidence as a
capability claim. That is the failure mode this
project's rules exist to prevent, and finding three of them in one pass is the
argument for the evidence discipline, not against it.

---

## 2. Architecture decision matrix

| Decision | Options considered | Chosen | Reason | Rejected because | Risk |
|---|---|---|---|---|---|
| **Application shell** | winit+wgpu+egui; Tauri; Qt; Slint; egui alone; custom GPU UI; native-host-plus-web | **winit + wgpu + egui + egui_tiles**, one process | Only option giving GPU surfaces on the *same device and queue* as the compute kernels; MIT/Apache-2.0 end to end | Tauri/webview provably cannot share a GPU surface (upstream issue closed unimplemented); Slint's open arm is GPLv3; Qt's LGPLv3 attaches relink and source-offer obligations to an Apache-2.0 product | egui composites in gamma space, so colour-managed HDR cannot live in a docked panel — the reference viewer must own its own surface. Accepted and designed around |
| **wgpu version** | Stay on 26; move to 30 | **30** | egui 0.36 requires it; two wgpu versions would mean two incompatible `Device` types and no shared surface | Staying on 26 would have made the shell impossible without a later, harder migration | Done and verified; 17 GPU tests pass on real hardware |
| **H.264 decode** | FFmpeg; OpenH264; `rusty_h264`; none | **`rusty_h264` behind a trait** | Only pure-Rust decoder that decoded the real asset (240/240 frames); BSD-2; no C toolchain needed. OpenH264 *failed* the asset (9/60 — no B-frames) | FFmpeg unbuildable here (no pkg-config/make/nasm) | Ten weeks old, and decodes **beyond the scope its authors advertise**. Mitigated by the trait, not resolved |
| **Colour runtime** | Depend on real OCIO; port Apache-2.0 `aces-core`; keep current | **Port `aces-core`; OCIO as a developer-only oracle** | ACES reference is Apache-2.0 as of 2025-09-25; OCIO builds here with `zig c++` and can serve as the conformance oracle | Runtime OCIO dependency adds a C++ build to every deployment for a job a port can do | Current colour is 30% off on saturated colour; the port is unstarted |
| **OpenUSD** | C++ sidecar; no USD; `openusd` Rust crate | **`openusd` crate (MIT) behind a cargo feature**, `usd-core` as oracle | Pure Rust, `.usda`/`.usdc`/`.usdz` plus full LIVRPS composition, builds with plain `cargo build`; AOUSD Core Spec 1.0 is ratified, so an independent implementation is legitimate | C++ USD would add a large build; "no USD" abandons interchange | The crate is very new. And **OpenUSD itself is not Apache-2.0** — see §5 |
| **OTIO** | Native Rust; FFI; Python subprocess | **Native Rust**, Python `opentimelineio` as a *test* dependency | No usable Rust binding exists (the crates.io names are 2020 placeholders); FFI fights the UCG's identity model | — | We currently pass **Tier 1 of 5** on our own conformance bar |
| **Timeline crate** | Evolve; replace; split | **Split** into codec / operations / model | The editing operations are correct and worth keeping; the OTIO codec and the model have different reasons to change | — | Not yet done |
| **Audio engine** | Optimise `cine-audio::render`; new engine crate | **New `cine-audio-engine`** | `render()` is an offline mixer; it cannot be made into a real-time engine by optimisation | — | Not yet started |
| **Collaboration** | Locking; optimistic concurrency; CRDT | **Replicate depth-0 deltas, recompute derived state** | Reactor idempotence is already a project rule, which makes recomputation sound | CRDTs guarantee convergence, not *correctness* of invariants | Op-log format is the least changeable artefact; metadata should be added before it ossifies |

---

## 3. Capability matrix

### Media

| Capability | Required | Current | Evidence |
|---|---|---|---|
| H.264 High 4:2:0 8-bit decode | Yes | 🟡 EXPERIMENTAL | 240/240 frames of the real asset; decoder is 10 weeks old and beyond its advertised scope |
| H.264 Main / Baseline | Yes | 🟡 PARTIAL | Same code path, no asset here to prove it. Registry marks it UNVERIFIED, not claimed |
| H.264 4:2:2 / 4:4:4 / 10-bit | Yes | 🔴 Refused | Deliberate: emitting a corrupt picture is worse than reporting no decoder |
| HEVC, ProRes, DNx | Yes | 🔴 NOT IMPLEMENTED | No credible pure-Rust decoder |
| AAC-LC, PCM decode | Yes | 🟢 VERIFIED | Real AAC and real WAV decoded, non-silent |
| MP3, FLAC, ALAC, Vorbis | Yes | 🟡 PARTIAL | Wired up, no assets exercised here |
| PNG decode | Yes | 🟢 VERIFIED | Real system PNG |
| JPEG, TIFF | Yes | 🟡 PARTIAL | Enabled, unexercised |
| OpenEXR | Yes | 🔴 NOT IMPLEMENTED | `exr` crate is BSD-3, pure Rust, 17.1M downloads — but **no deep data** |
| Image sequences | Yes | 🔴 NOT IMPLEMENTED | A still is one frame; no sequence assembly |
| Exact rational media time | Yes | 🟢 VERIFIED | `cine-media::time`, i128 cross-multiplication |
| Frame-accurate seek | Yes | 🟢 VERIFIED | Sought frame is **pixel-identical** to the played frame |
| Content-addressed identity, relink | Yes | 🟢 VERIFIED | Idempotent ingest, offline as a state |
| Full-hash verification | Yes | 🟢 VERIFIED | Detects a single flipped bit; fast hash now **refuses** to verify |
| Capability registry | Yes | 🟢 VERIFIED | Single source of truth; tests enforce evidence and forbid unverified hardware claims |
| Hardware decode | Yes | 🔴 NOT IMPLEMENTED | **Not claimed.** Registry test fails if any hardware entry is added |
| Playback scheduler | Yes | 🟢 VERIFIED | `clock → scheduler → decode → selection → presentation`, 39 tests; exact frame selection at every one of 240 boundaries of the real asset |
| Decode-ahead / buffering | Yes | 🟢 VERIFIED | Worker thread, bounded by bytes, generation-invalidated, keyframe-aware reverse; 23 step-by-step tests |
| Real-time playback, headless | Yes | 🟢 VERIFIED (this asset, this machine) | 240/240 pictures, **0 dropped**, after a 1.03 s pre-roll; 720/720 over three passes. Margin is ~0.1 ms per picture — see `PLAYBACK.md` before generalising |
| Real-time playback, on screen | Yes | 🟡 PARTIAL | **144/144 at exactly 24 fps, 0 dropped, three runs running** — on one asset, one machine, with the decode margin near zero. A hot run dropped 62%. Presentation costs ~3.5 ms of the 41.7 ms budget after moving conversion into the fragment shader (was ~82.5 ms) |
| Planar presentation path | Yes | 🟢 VERIFIED | Matches the CPU reference within **1 of 255** across BT.709/601/2020, both ranges, assumed matrices and padded strides, on real hardware |
| Drop accounting | Yes | 🟢 VERIFIED | Counted as the playhead passes each picture: exactly once, independent of poll rate, and including pictures missed by a stall that never ends |
| Reverse playback | Yes | 🟡 DEGRADED | 8.47 s → **71.9 ms per picture** with keyframe-aligned chunks, 118x. Still 28.9% dropped crossing a GOP boundary |
| Seek | Yes | 🟡 PARTIAL | 2.6 µs warm; 4.82 s cold into a 189-picture GOP — the keyframe walk, unavoidable in this design |
| Short-GOP media | Yes | ⚪ UNVERIFIED | Both assets here are long-GOP; one is a single 240-picture GOP. No encoder to make one |
| A/V sync architecture | Yes | 🟢 VERIFIED | Audio-mastered clock: 23.976 fps picture paced by a 48 kHz clock lands on the exact sample, checked at all 240 pictures, and never one sample early. 14 tests |
| A/V sync, end to end | Yes | 🔴 UNSUPPORTED | **No audio device on this build.** No ALSA/PipeWire headers and no pkg-config, so `cpal` cannot build. Runtime-loading `libasound` is researched, not built. Nothing claims sound plays |
| Audio device latency | Yes | ⚪ UNVERIFIED | Modelled and tested; never measured against hardware |
| Edit list (`elst`) handling | Yes | 🔴 NOT IMPLEMENTED | The reference asset presents from 1024/12288, not 0, and nothing compensates |
| Proxies, thumbnails, waveforms | Yes | 🔵 PLANNED | |

### GPU and colour

| Capability | Required | Current | Evidence |
|---|---|---|---|
| Real GPU adapter | Yes | 🟢 VERIFIED | Intel UHD Graphics (CML GT2), Vulkan, Mesa 25.2.8 |
| Compute kernels, CPU parity | Yes | 🟢 VERIFIED | 17 GPU tests on real hardware |
| YUV→RGB with CPU reference | Yes | 🟢 VERIFIED | BT.601/709/2020 × limited/full, and a real decoded frame; worst divergence <1e-4 |
| Matrix provenance (declared vs assumed) | Yes | 🟢 VERIFIED | `MatrixChoice`; BT.2020 never assumed |
| Swapchain / presentation | Yes | 🟢 VERIFIED | Real window on Wayland, Bgra8Unorm surface, real frames presented and dumped for inspection |
| HDR surfaces | Yes | 🔵 PLANNED | egui composites in gamma space; needs a dedicated surface |
| ACES output transform | Yes | 🟡 PARTIAL | Measured against OpenColorIO 2.5.2 over 1441 samples and 38 transforms, committed as a permanent harness. ACES 2.0 SDR: **neutral axis exact to 5e-6**, worst saturated sample **0.556** — the missing chroma/gamut compression. Colorimetry and log curves are right; the camera-gamut→AP0 matrices differ (ARRI LogC4 worst at 0.664) |
| Colour conformance harness | Yes | 🟢 VERIFIED | 1441 reference samples from OCIO 2.5.2 committed with their generator, so the gap is measured on every change rather than once |
| OpenColorIO config loading | Yes | 🔴 NOT IMPLEMENTED | Cannot load **any** released OCIO ACES config — `shared_views` and `!<Views>` unimplemented |

### Editorial, production, audio

| Capability | Required | Current | Evidence |
|---|---|---|---|
| OTIO write | Yes | 🟢 VERIFIED | Output parses in real OTIO — most implementations do not achieve this |
| OTIO read | Yes | 🟡 PARTIAL | 4 hard rejections, 4 silent losses. **Tier 1 of 5** on our own bar |
| Multiple media references / proxy conform | Yes | 🟢 VERIFIED | Preserved through round-trip; conform swaps without losing either |
| EDL / AAF / FCPXML | Yes | 🔴 NOT IMPLEMENTED | AAF container is CFB (`cfb` crate, MIT); **`LibAAF` is GPL-2.0 and must not be read** |
| Editing operations (ripple/roll/slip/slide…) | Yes | 🟢 IMPLEMENTED | 15-operation semantics table; no semantic errors found. Gap: no transition media-handle check, so a dissolve can validate and be unrenderable |
| Shot → take → media traceability | Yes | 🟡 PARTIAL | Takes now survive shot-list regeneration; full production model designed, not built |
| Offload verification state machine | Yes | 🔵 PLANNED | Designed: `discovered → copying → copied → verifying → verified`, `failed` absorbing |
| Loudness (EBU R128 / BS.1770) | Yes | 🟢 VERIFIED | `lufs_integrated` agrees with `ebur128` to **0.000 LU** across five signals including 5.1 |
| True-peak metering | Yes | 🟢 VERIFIED | Within **0.05 dB** of an exact Whittaker–Shannon reconstruction oracle on band-limited programme material, and inside EBU Tech 3341's +0.2/−0.4 dBTP window. The earlier "reads 0.3–0.5 dB high" finding was **misattributed** — see §6 |
| Real-time audio | Yes | 🟡 PARTIAL — and the old figure was wrong | **48 tracks stereo now holds a 512-frame deadline on one core with 3x margin** (worst of 400 consecutive blocks: 3.585 ms against 10.667 ms). 128 x 7.1.4 does **not** (13.576 ms median, 127%), and 128 stereo is marginal at 96% and is not claimed. See §6 for what the old 114.6% figure actually measured |
| Plugin hosting | Yes | 🔵 PLANNED | CLAP and VST3 now both viable — see §5 |
| Sample-accurate splices | Yes | 🟢 VERIFIED | Reproduced the defect at 29.97 and fixed it |

### Platform

| Capability | Required | Current | Evidence |
|---|---|---|---|
| Deterministic CI regression gates | Yes | 🟢 VERIFIED | Assert work done, not wall-clock: stable on any machine |
| Bounded op log | Yes | 🟢 VERIFIED | Gate derives its workload from the configured limit |
| Reactor convergence / idempotence | Yes | 🟢 VERIFIED | Gated |
| One time model across the whole project | Yes | 🟢 VERIFIED | `cine-media`'s parallel `Timestamp`/`TimeBase`/`FrameRate` are gone; media time **is** `RationalTime` at a `Rate`. Unifying them exposed a real drop-frame bug in the media implementation — see §6 |
| Exact time in `ucg-core` | Yes | 🟢 VERIFIED | **`RationalTime` is now the exact type**: an integer tick count at an exact rational rate, with equality, ordering and hashing all by value. One time model, no duplicate. 25 adversarial tests across every rate; the workload that defined the defect (200,000 frames at 23.976↔25) is now lossless. See `EXACT-TIME-MIGRATION.md` |
| Reactor idempotence under recomputation | Yes | 🟢 VERIFIED | A thousand cascade recomputations write **zero** temporal operations (`temporal_idempotence`) |
| Application shell | Yes | 🟡 PARTIAL | `cine-shell` opens a real window and presents real decoded frames with frame stepping. No docking UI, no transport, no playback clock |
| One-command startup | Yes | 🟢 VERIFIED | `cine start` — see `APPLICATION-RUNTIME.md` |
| One runtime, many clients | Yes | 🟢 VERIFIED | Two clients over real sockets, neither diverging |
| Script → shot → take → media chain | Yes | 🟢 VERIFIED | `film_fixture`: real media, propagation, captured-data preservation, save and reopen |
| Collaboration | Yes | 🔵 PLANNED | |

---

## 4. Research matrix

| Domain | Reference systems | Open source studied | CINE-OMNI strategy | Status |
|---|---|---|---|---|
| Media / codecs | Resolve, Premiere | FFmpeg, symphonia, `rusty_h264`, `re_mp4` | Pure-Rust behind a decoder trait; FFmpeg deferred | 🟡 Slice verified |
| Compositing | Nuke, Fusion | Natron (GPL), OpenImageIO, OpenEXR, OpenFX | Reimplement natively; OIIO is **Apache-2.0** so it may be *ported*; Cryptomatte is BSD-3 with published test vectors | 🔵 Designed |
| Colour | Resolve, Baselight | OpenColorIO, ACES, OpenEXR | Port Apache-2.0 `aces-core`; OCIO as oracle | 🟡 Partial, gap measured |
| Audio | Pro Tools, Nuendo, Fairlight | Ardour (GPL), JACK, PipeWire, CLAP, LV2 | New real-time engine crate; CLAP and VST3 hosting | 🟠 Current engine inadequate |
| Editorial | Avid, Premiere, Resolve | OpenTimelineIO | Native Rust OTIO; split `cine-timeline` | 🟡 Tier 1 of 5 |
| Production / dailies | ShotGrid, Silverstack, Frame.io | ASC MHL | Native; ASC MHL for interchange | 🔵 Designed |
| 3D / DCC | Maya, Blender, Houdini | Blender (GPL), OpenUSD, Alembic, glTF, MaterialX | `openusd` crate behind a feature; Blender by bridge only | 🔵 Designed |
| Collaboration | Resolve, Avid, Perforce | Automerge, Yjs | Replicate depth-0 deltas, recompute derived | 🔵 Designed |

---

## 5. Licence matrix

Workspace spine is **Apache-2.0**, one-way incompatible with GPL. `cargo deny
check` passes: advisories, bans, licences, sources all ok. **No GPL or LGPL
anywhere in the dependency tree.**

| Component | Licence | Link mode | Distribution impact | Action |
|---|---|---|---|---|
| `symphonia` | MPL-2.0 | Static | File-level copyleft; our files unaffected while unmodified | ✅ In use |
| `rusty_h264` | BSD-2-Clause | Static | None | ✅ In use |
| `mp4`, `image`, `sha2`, `indexmap` | MIT / Apache-2.0 | Static | None | ✅ In use |
| `wgpu`, `egui`, `winit`, `egui_tiles` | MIT / Apache-2.0 | Static | None | ✅ Shell approved |
| **OpenUSD** | **Tomorrow Open Source Technology License 1.0** — *not* Apache-2.0, since 24.08 (2024-07-25) | — | Bespoke, non-OSI, non-SPDX; Apache-2.0 with a **narrowed §6** | ⚖️ **LEGAL REVIEW REQUIRED** before any dependency |
| `openusd` crate | MIT | Static | None | 🔵 Approved for use |
| OpenTimelineIO | Apache-2.0 (verified by SHA-256 against the canonical text) | Test-only (Python) | None | ✅ Test oracle |
| OpenImageIO | **Apache-2.0** (relicensed 2023; >99.86% of lines) | Source may be *ported* | None | 🔵 Port candidate |
| OpenFX | BSD-3-Clause | Host | None | 🔵 Legally clear, deferred on engineering grounds |
| Cryptomatte | BSD-3-Clause, spec 1.2.0 with test vectors | Reimplement | None | 🔵 ~200 lines, highest value per line |
| OpenColorIO | BSD-3-Clause | **Developer oracle only** | None if not shipped | ✅ Oracle |
| ACES reference | Apache-2.0 **from commit `ee380496`, 2025-09-25**; v1.3.1 and earlier are AMPAS ACES License Terms | Port | Must not vendor 1.x CTL under an Apache notice | 🔵 Port target |
| **VST3 SDK** | **MIT as of VST 3.8 (2025-10-29)** — GPLv3/proprietary no longer offered | Host | None | 🔵 **Now hostable.** Supersedes the spine's old finding |
| **ASIO SDK** | **Now dual GPLv3 / proprietary** | — | Cannot be linked by an Apache-2.0 product | 🔴 Reject |
| CLAP | MIT | Host | None | 🔵 Preferred |
| LV2 | ISC | Host | None | 🔵 Viable |
| `vst3-sys` | GPL-3.0 | — | Pulled in by `nih-plug`'s VST3 export | 🔴 Use `vst3` (coupler-rs, MIT) instead |
| `jack` crate | MIT, **links LGPL-2.1 libjack** | Dynamic | LGPL obligations attach to the *linked library* | ⚖️ Review before shipping |
| `libltc-rs` | MIT, **links LGPL-3.0 libltc** | Dynamic | Same | ⚖️ Review before shipping |
| Natron | GPL-2.0-**or-later** | — | Cannot be linked or ported | 🔴 Concepts only |
| Blender, Kdenlive, Ardour | GPL | — | Separate process or ideas only | 🔴 No code transplant |
| `ffmpeg-next`, `ffmpeg-the-third` | **WTFPL** — no patent grant, no warranty disclaimer | — | Commonly rejected by `cargo-deny` policy | 🔴 Use `rsmpeg` (MIT) if FFmpeg is ever bound |
| `rawloader`/`rawler` | LGPL-2.1; `imagepipe` LGPL-3.0-only | Static (Rust) | **No LGPL §6(b) relinking escape** | 🔴 Camera-RAW stills not available on assumed terms |
| Meadowlark | AGPL-3.0, archived | — | — | 🔴 No permissive Rust DAW engine exists |
| `libadm`, `libbw64` | Apache-2.0; EBU ADM Renderer BSD-3-Clause-Clear | — | None | 🔵 Object audio is open |
| Signalsmith Stretch | MIT | — | None | 🔵 Time-stretch is open (Rubber Band is GPL) |
| ASC MHL | Tool and XSDs MIT; **spec PDFs carry no licence or copyright notice at all** | — | — | 🔵 Implement |
| CLF / SMPTE ST 2136-1 | Schema readable, **LICENSE forbids reproduction** | — | — | 🔵 Implement, never vendor |

### Patent — separate from copyright

**H.264 and HEVC carry patent pools.** A BSD-2 decoder settles the *copyright*
question and says nothing about the *patent* one. Development is unaffected;
distribution is not. ⚖️ **LEGAL REVIEW REQUIRED** before any release.

---

## 6. Known defects not yet fixed

Listed because an unlisted known defect is indistinguishable from a lie.

| Defect | Impact | Evidence |
|---|---|---|
| ~~`ucg-core::RationalTime` is float-based~~ | **Fixed.** The engine bug is closed: `Tx::set` decides idempotence with `Value: PartialEq`, and time equality is now exact, so a recomputed value that is arithmetically identical compares equal and emits nothing | Was 12.1% wrong frames and 24.2% equality failures over 200,000 frames at 23.976↔25. Now zero, and a thousand cascade recomputations write no temporal ops |
| ~~`true_peak_dbtp()` reads 0.3–0.5 dB high~~ | **Withdrawn — the attribution was wrong** | An exact Whittaker–Shannon reconstruction (zero-padded DFT, verified exact to 1e-6 dB on single-bin sinusoids) settles which meter is right. cine-audio was already within 0.05 dB of truth; **`ebur128` was the one under-reading**, by up to 0.238 dB. Two real defects were found in the same function and fixed instead: a 4× grid too coarse to resolve inter-sample peaks (−0.301 dB worst case on a phase sweep, outside the EBU budget before filter error is even counted) and a resampler recomputing sinc weights per frame at 578× the cost of an arithmetic pass. Now −0.094 dB worst case at 104×. |
| ~~`cine-audio::render` at 114.6% of budget~~ | **Withdrawn — the figure measured machine contention, not the mixer.** Same binary, same fixture, before any change: **13.642 ms (127.9%) at load average 20, and 1.814 ms (17.0%) at load average 3** — 7.5x apart. This box is a 2-core i3-10110U that was shared with other builds all session. The sensitivity was itself real (the old `render` allocated ~150 buffers and dispatched rayon per call, and both degrade badly under contention) but that is not the same claim as "over budget" | Second documented defect this session to dissolve under adjudication, after true peak. **A number taken on a contended machine is not a measurement of the code** |
| 128 tracks at 7.1.4 misses the deadline | Cannot mix a large immersive session in real time | 13.576 ms median against 10.667 ms. Threading does not rescue it — median falls to 6.8 ms but p99 is 21.2 ms, so the pool's wake latency becomes the deadline. Closing it needs roughly 1.3x on the per-frame path: SIMD on the pan/accumulate loops, f32 biquads, and a spinning rather than parking pool |
| ACES 1.0 output transform | Not implemented, and now refused rather than substituted | Answering it with ACES 2.0 was measured **0.18 wrong on the neutral axis** — visibly wrong greys, where every other output path matches OCIO's neutrals to 5e-6 |
| ~~`cine-media` drop-frame labels were wrong at minute boundaries~~ | **Fixed.** `Timecode::from_frame_number` decomposed the raw frame count without adding back the skipped labels, so frame 1800 — the first frame of minute one — was labelled `;00`, which is one of the two labels drop-frame exists to skip. It was self-inconsistent: the generator produced labels its own `to_frame_number` validator rejected | Found by deleting the duplicate time model and pinning the two drop-frame implementations against each other. Neither had a test that compared them |
| Full-band true peak under-reads | A reading on deliberately full-band material is not a delivery pass | Inherent to any oversampling meter (BS.1770 Annex 2, Attachment 1). Measured: cine-audio 1.0–1.5 dB low, `ebur128` 1.4–1.9 dB low. Documented on the function |
| `cine-comp::hashes()` superlinear, on every evaluation | 53 ms at 2,001 nodes | Third occurrence of "derived value recomputed in a hot loop" |
| `cine-comp::evaluate_roi` returns the full frame | ROI plumbing saves nothing and fragments the cache | 64×64 request returned 16.7M floats |
| `cine-comp::resize` bilinear at all scales | 0.25 max abs error vs a box reference at 8× reduction | |
| Deep compositing uses a linear alpha scale | OpenEXR requires Beer–Lambert `α' = 1−(1−α)^r` | |
| `cine-comp` has no file I/O | `Read(File)` reads a pre-registered in-memory image | |
| OTIO read gaps | Real OTIO files rejected or silently lossy | Tier 1 of 5 |
| No transition media-handle check | A dissolve can validate and be unrenderable | |
| VFR detection is weak | Derived from the *mean* sample duration | |
| Colour metadata unparsed | No `colr` box, no VUI | Frames honestly carry no colour |

---

## 7. Environment — what this machine can and cannot verify

| Capability | State |
|---|---|
| Rust toolchain, Zig linker | ✅ |
| Real GPU (Intel UHD, Vulkan, Mesa 25.2.8) | ✅ GPU work is verifiable |
| Wayland session | ✅ A window can be opened |
| `zig cc` / `zig c++` | ✅ C and C++ sources compile — the earlier "no C toolchain" premise was **wrong** |
| CMake + Ninja (via PyPI wheels) | ✅ |
| OpenColorIO 2.6.0dev | ✅ Built here; usable as an oracle |
| OpenUSD 26.8 (`usd-core`) | ✅ Runs here |
| Python `opentimelineio` 0.18.1 | ✅ Runs here |
| PipeWire 1.0.5 | ✅ Running |
| `pkg-config`, `make`, `nasm` | 🟠 Absent — **FFmpeg cannot be built** |
| `cpal` | 🟠 Fails to build (needs `pkg-config` for `alsa-sys`) |
| Audio plugins | 🟠 None installed; hosting untested |
| Hardware video decode | 🟠 Untested — and therefore unclaimed |

---

## 8. What may and may not be claimed today

**May be claimed**, with evidence:
* One command (`cine start`) brings up the application, and a second terminal
  attaches to it rather than corrupting the project
* One runtime serving many clients with no divergence
* A screenplay edit propagating to derived work while captured takes survive,
  across a save and reopen, with real media
* A real window presenting real decoded H.264 frames through a GPU colour
  conversion, on real hardware, with the colour interpretation labelled as
  assumed rather than stated
* Exact rational time in the media layer
* Frame-accurate seek, pixel-identical to playback
* Content-addressed media identity, relink, and offline as a state
* Full-hash verification that detects a single flipped bit
* GPU/CPU parity for compute kernels and YUV→RGB on real hardware
* A playback scheduler driven by the exact clock, with observed rather than
  estimated drop, late and underrun counts, and honest refusal on unsupported
  media and decoder failure
* Loudness measurement agreeing with `ebur128` to 0.000 LU
* OTIO output that parses in real OpenTimelineIO

**May not be claimed**:
* "ACES-compatible" — measured 30% off on saturated colour
* "OpenColorIO-compatible" — cannot load any released ACES config
* "OpenUSD" — `cine-previs` emits a USDA-shaped subset
* "OTIO-compatible" — Tier 1 of 5
* "Real-time" for audio in general — 48 stereo tracks hold a 512-frame
  deadline with 3x margin and may be claimed; 128 x 7.1.4 does not and 128
  stereo is marginal, so neither may be
* "Real-time playback" **in general** — it holds on this asset on this machine
  when quiet, and fails when hot. One asset is not a claim about a system
* "Real-time playback" **in general** — headless playback of the reference clip
  is drop-free, but on a margin of ~0.1 ms per picture, on one asset, on one
  machine. That is a measurement, not a property of the system
* "A/V sync" — the architecture is verified but no audio device exists on this
  build, so no sound has ever accompanied a picture here
* "Smooth" or "production" playback of long-GOP media — reverse still drops
  28.9% crossing a GOP boundary and a cold seek into a GOP costs 4.8 s
* "Short-GOP" anything — unverified, no asset
* "Hardware accelerated" — no hardware path exists
* "Production-ready" for any subsystem
