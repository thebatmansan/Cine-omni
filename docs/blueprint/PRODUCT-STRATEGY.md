# Product strategy — what CINE-OMNI must become, and how it wins

The capability matrix says *what* is missing. This says what it means: for the
architecture, for the business, for licensing, for performance and for storage.

**Sources.** Areas already covered by the twelve research dossiers
(`docs/blueprint/research/`) are summarised from them, with their measured
findings re-verified where they bear on a decision. Areas they did not cover —
screenwriting, scheduling, delivery, timed text, RAW SDKs, Dolby, AAF, Unreal,
codec patents and market pricing — were researched on 2026-09-15 against the
sources listed at the end. Anything not traceable to one of those is marked as
judgement.

---

## 1. The complete vision, stated precisely

One application in which a production lives from first draft to delivered master:

```
DEVELOPMENT      script · revisions · beats · notes
PRE-PRODUCTION   breakdown · schedule · budget · boards · shot design · previs
PRODUCTION       offload · verify · takes · metadata · sync · dailies
POST             edit · VFX · colour · sound · 3D plates
DELIVERY         render · masters · DCP/IMF · timed text · QC · archive
```

sharing one project graph, one time model, one media identity system, one colour
foundation, one undo history and one runtime.

**The differentiator is not the list — every item on it already exists in some
product. It is that a change in one department shows its consequences in the
others.** Today a scene cut in Final Draft reaches Movie Magic Scheduling by
re-import, reaches Resolve by a new EDL, and reaches Pro Tools by an AAF and a
change list. Nothing tells the sound editor that a scene they mixed yesterday
lost eight seconds. CINE-OMNI's reactor cascade is the one piece of this
architecture no incumbent has, because none of them owns more than two
departments' data.

## 2. What the industry actually provides — the depth that must be matched

Condensed from the dossiers and the new research. The point of this section is
the *depth* bar, not the feature names.

| Department | Incumbent | The depth that matters (not the button count) |
|---|---|---|
| Screenwriting | Final Draft 13 | Revision mode with **coloured and locked pages, collated revision sets, scene numbering and omitted scenes** "the way production offices expect"; tagging for breakdown; Beat Board; real-time collaboration; FDX is the interchange everyone else reads. Pagination that matches is what budgets and schedules are built on. |
| Scheduling | Movie Magic Scheduling | Breakdown sheet per scene with customisable categories; **stripboard** as the visual heart; **Day Out of Days** per cast member (work, hold, travel). |
| Production tracking | Autodesk Flow (ShotGrid) | Shot/asset tracking, review, resource planning — the system ILM, Framestore, DNEG and Weta run pipelines on. |
| Dailies / DIT | Silverstack, Livegrade | Verified offload, ASC MHL manifests, CDL per camera, metadata reconciliation (R15). |
| Editorial | Avid, Resolve, Premiere | Exact trim semantics, multicam, bin locking, AAF with Pro Tools, relink and conform. Avid's hold on features and TV is **collaboration and AAF**, not editing features (R16). |
| Compositing | Nuke | Data windows, deep EXR, 32-bit float everywhere, OFX, a graph that only computes what is needed (R12). |
| Colour | Resolve, Baselight | OCIO/ACES correctness first; wheels, curves, qualifiers, tracking; HDR and Dolby Vision (R13). |
| Sound | Pro Tools, Fairlight | Real-time-safe engine, automation, AAF interchange, loudness compliance, immersive ADM/Atmos (R14). |
| 3D | Blender, Maya, Houdini | Decades of depth; not a near-term target (R01, R17). |
| Delivery | Clipster, Resolve Deliver, DCP-o-matic | **IMF App 2E** for streaming (Netflix requires Photon-clean packages); **SMPTE DCP** (JPEG 2000 in MXF, ≤250 Mb/s standard) for cinema; **TTML1/IMSC1.1** timed text (SRT/VTT/SCC not accepted by Netflix without exception). |

Two conclusions drive everything below:

1. **The table stakes are deep in every department**, and a filmmaker judges
   CINE-OMNI's editor against Resolve's, not against "an all-in-one tool".
2. **Interchange is survival.** Nobody moves a production wholesale. CINE-OMNI
   must read and write FDX, OTIO, AAF, EDL, ASC MHL, CDL, OCIO configs, IMF and
   TTML well enough that it can occupy one department of a real production first.

## 3. Architectural consequences

What the research means for the current architecture, in priority order.

**3.1 One shell, two surfaces — decide now.** Picture-critical work must be
native (a frame must not leave the GPU; measured). Project, planning, script,
production and review work is well served by the web interface, which is also
what makes remote review and collaboration possible later. So:

* `cine start` launches the runtime **and** the native workspace, which embeds
  or hosts the same interface components the browser uses for non-picture
  panels — or, as a smaller first step, the native workspace becomes a *client*
  of the same runtime rather than opening media on its own.
* The browser remains a first-class client for everything that is not
  picture-critical, and for remote review.
* Both consume the **same protocol and projections**. There is never a second
  project model.

This is Phase 1 of the roadmap because every later workspace would otherwise be
built twice.

**3.2 A job system is foundational, not an optimisation.** Offload, checksums,
proxies, renders, analysis, conform, QC and exports are all long-running,
resumable, cancellable, observable work. Ten matrix rows are blocked on it.
Decode-ahead already has the right shape (bounded, generation-invalidated,
cancellable at a known granularity); it should become the pattern, not an
exception.

**3.3 Media must become pluggable per codec, with capability per path.** The
matrix defines support as DECODE · METADATA · COLOUR · TIME · EDIT · PROCESS ·
RENDER · EXPORT. The current registry records *decode*; it must record each path
separately, because "imports ProRes" and "delivers ProRes" are different claims
with different licences.

**3.4 Captured data must never be structurally owned by derived data.** Takes
under shots by `Contains` is the known case. The general rule — captured media,
takes, notes and decisions are *sources*; breakdowns, schedules, shot lists and
renders are *derived* — must be a schema constraint, checked, before production
workflows are built on it.

**3.5 Images need data windows before any VFX work.** A type-level change to
`cine-comp`'s image representation, and it only gets more expensive.

**3.6 Colour needs a policy decision on OCIO.** Either a complete native config
reader, a sanctioned OpenColorIO build (BSD-3-Clause, verified) behind a process
or FFI boundary, or a documented constrained subset. The current partial parser
cannot load the configs studios actually ship.

**3.7 Schema versioning before real users.** A production lasts years. Projects
written today must open after the schema changes; this is cheap now and
impossible later.

**3.8 Cross-department impact must be a first-class projection.** The cascade
already produces touched entities and dirty reasons on every commit. Turning that
into "this change affects: 3 shots, 1 schedule day, 42 s of cut, 2 VFX shots,
1 sound stem" is the product's reason to exist and is mostly an interface and
projection problem, not an engine one.

**3.9 Nothing in the current architecture needs to be thrown away.** The UCG,
exact time, runtime, op log, media identity, scheduler and capability model are
all the right foundation. The wrong-direction items in the audit are bounded,
specific corrections, not rewrites.

## 4. Commercial analysis

This section is judgement, informed by pricing and positioning research; it is
not market data.

### 4.1 The market reality

* **DaVinci Resolve is free** for most work (UHD, 60 fps, 8-bit) and **$295
  one-time** for Studio, and it already bundles edit, colour, Fusion VFX and
  Fairlight audio. Premiere Pro is **$22.99–34.49/month**; Avid Media Composer
  Ultimate **~$540/year**; Blender is free. Final Draft is a paid perpetual
  licence; Movie Magic Scheduling and Budgeting are paid; Flow Production
  Tracking is a studio subscription.
* **"All-in-one post" is already taken, and it is free.** A CINE-OMNI that
  competes as a cheaper Resolve cannot win.
* What nobody owns is **development → pre-production → production → post as one
  connected project.** Writers, ADs, producers, DITs and editors live in
  separate tools joined by exports and email.

### 4.2 Customers, in the order they could plausibly adopt

| Segment | Pain CINE-OMNI can remove | What they will demand before switching |
|---|---|---|
| **Indie and student productions** | Paying for and gluing Final Draft + scheduling + Resolve | Script, schedule and a competent editor; free or cheap; runs on a laptop |
| **Documentary and reality** | Enormous media volumes, transcripts, logging | Ingest, metadata, search, multicam, proxies |
| **Production offices (ADs, UPMs, coordinators)** | Script revisions breaking schedules and call sheets | Revision-aware breakdown and scheduling, PDF call sheets, reliability |
| **DITs and dailies houses** | Verified offload, metadata reconciliation | ASC MHL, camera metadata, CDL, speed, zero data loss |
| **Commercial and agency teams** | Fast turnaround, many deliverables | Editing, colour, deliverables, review |
| **Post houses, VFX facilities, studios** | Pipeline integration | AAF, OTIO, OCIO, OFX, ShotGrid-class tracking, security — **last to adopt** |

### 4.3 Table stakes vs differentiation

**Table stakes** (without these, no evaluation gets past a day):

* a screenplay editor with correct pagination and revisions;
* an editor with exact trim semantics, multicam and proxies;
* real-time playback of common camera formats on normal hardware;
* import/export of FDX, OTIO/EDL, AAF with Pro Tools;
* reliability: no lost work, ever.

**Differentiation** (why someone would choose CINE-OMNI):

1. **Cross-department impact.** A script change shows its schedule, shot, cut,
   VFX and sound consequences before anyone discovers them by accident.
2. **Provenance from set to master.** Every frame in a delivered master traces
   to a verified camera file, the take it came from and the script scene it
   serves. Insurers, completion bonds and archives care about this.
3. **Honest capability.** A tool that says what it cannot do, per format and per
   path, is rare; professionals who have been burned by silent substitution
   (wrong colour, wrong frame rate, dropped frames reported as playing) notice.
4. **Local-first, one runtime.** Works offline on a laptop on set; the same
   runtime serves collaborators when connected.

**What makes migration hard, and must be designed for:** existing projects in
other tools (interchange), trained muscle memory (keyboard parity for editors),
facility pipelines built around Avid/ShotGrid/Nuke (bridges, not replacement),
and trust (a track record of not losing data).

### 4.4 The product strategy that follows

**Win one connected slice of the lifecycle first, then widen.** The slice with
the least competition and the strongest use of the differentiator is:

> **script → breakdown → schedule → shots → takes → cut**, with change impact
> visible across all of it.

That is a production office and an indie editor's daily reality, it is where
CINE-OMNI's existing engine is strongest (script, logistics, timeline, media
identity and the cascade are all real), and it avoids competing head-on with
Resolve's colour and Fairlight's audio until those engines are credible.

Colour, VFX, audio and delivery follow, each first as *interchange-competent*
(round-trip to the incumbent) and only later as *replacement-competent*.

### 4.5 Deployment and enterprise requirements (to plan for, not build now)

Signed installers for Linux/macOS/Windows; offline licensing; project archives
that reopen years later; audit trails (the op log is a start); SSO and
role-based permissions for facilities; on-premises runtime (no mandatory cloud);
documented interchange conformance; security review before any non-loopback
network exposure.

## 5. Licensing risks — concrete findings

The project ships under Apache-2.0. `deny.toml` forbids GPL/LGPL/AGPL crates.

### 5.1 What is linked today (266 third-party crates for Linux)

| Finding | Category | Action |
|---|---|---|
| 0 GPL / LGPL / AGPL crates | **SAFE** | keep `cargo deny` in CI |
| `symphonia` family (14 crates), MPL-2.0 | **SAFE** unmodified | file-level copyleft: any modification to those files must be published; do not vendor-and-patch silently |
| `self_cell`, "Apache-2.0 OR GPL-2.0-only" | **SAFE** | Apache-2.0 elected; record the election |
| egui default fonts: OFL-1.1 + Ubuntu Font Licence | **NEEDS REVIEW** | ship licence texts; Ubuntu Font Licence restricts modified fonts' names |
| `rusty_h264` (BSD-2) decoding H.264 | **SAFE** copyright / **PATENTS** separate | codec patents are not solved by the code licence — see 5.3 |
| `image`, `wgpu`, `naga`, `winit`, `egui*` | **SAFE** | MIT/Apache |

### 5.2 Proposed integrations

| Integration | Licence (verified where marked) | Category |
|---|---|---|
| OpenColorIO | BSD-3-Clause ✔ | **SAFE** |
| OpenImageIO | Apache-2.0 ✔ (99.86% relicensed from BSD-3) | **SAFE** |
| OpenFX | BSD-3-Clause ✔ (previously LGPL) | **SAFE** |
| OpenEXR | BSD-3-Clause (R12) | **SAFE** |
| OpenJPEG / OpenJPH (JPEG 2000, HTJ2K) | BSD-2-Clause ✔ | **SAFE** |
| EBU ADM renderer / `libear` | Apache-2.0 ✔ | **SAFE** |
| OpenTimelineIO spec | Apache-2.0; no usable Rust binding (R16) | **REIMPLEMENTATION** (already the path) |
| ACES (`aces-core`) | Apache-2.0 (R13) | **SAFE** |
| Cycles renderer, libmv | Apache-2.0 / MIT (R01) | **SAFE** as engine/library |
| VST3 SDK, CLAP | MIT (as recorded in status) | **SAFE** |
| OpenUSD | Modified Apache with trademark terms (R17) | **NEEDS REVIEW** |
| Netflix Photon (IMF validator) | open source ✔ — licence text not yet read | **NEEDS REVIEW** (use as external validator regardless) |
| Storyboarder | "MIT with a few exceptions"; an upstream issue disputes its open-source status | **NEEDS REVIEW** — do not copy code |
| AAF SDK | AAF SDK Public Source Licence ✔; AMWA advises further IP licences may be needed | **NEEDS REVIEW** |
| LibAAF | GPL | **INCOMPATIBLE** — do not read its source (R16) |
| FFmpeg | LGPL core / GPL components | **SEPARATE-PROCESS or dynamic LGPL-only build** (R02) |
| Blender, Kdenlive, Natron, Ardour, MLT | GPL | **SEPARATE-PROCESS / bridge only** |
| Blackmagic RAW SDK | free, no ongoing fees ✔; proprietary | **SEPARATE-PROCESS / dynamic** |
| RED R3D SDK | royalty-free; redistribute binaries unmodified only as part of a program adding significant functionality ✔ | **NEEDS REVIEW** (terms workable) |
| ARRI Image SDK | terms not found | **NEEDS REVIEW** |
| Dolby Vision | metadata generation free in grading tools; trims licence $2,500/yr; SDK proprietary ✔ | **NEEDS REVIEW** — defer |
| Dolby Atmos renderer | proprietary; ADM profile differs from EBU's ✔ | **DEFERRED** — use EBU ADM first |
| Unreal Engine | source-available EULA; rendered linear content royalty-free ✔ | **SEPARATE-PROCESS** only |
| ALSA / PipeWire / PulseAudio | LGPL system libraries | **NEEDS REVIEW** — runtime-loaded system library, same analysis as FFmpeg |

### 5.3 Patents are a separate axis

H.264 and HEVC decoders and encoders are covered by patent pools regardless of
the code's copyright licence. Access Advance acquired Via LA's HEVC/VVC programme
in December 2025; H.264 streaming licence terms have recently risen sharply. A
free, redistributable application that *encodes* H.264/HEVC is a real exposure.
Practical routes: use platform/OS codecs (whose licences cover the device),
hardware encoders, or royalty-free codecs (AV1, JPEG 2000, ProRes decode via OS
where licensed) for defaults — and get legal review before shipping any encoder.

## 6. Performance strategy

Design floor: the measured laptop (2 cores, 15 GiB, integrated GPU). Scale up by
using more of a workstation, never by requiring it.

**Principles**, each tied to something measured:

| Principle | Evidence it matters |
|---|---|
| **Never let a frame leave the GPU on the playback path** | read-back path 82.5 ms/picture → planar path 3.5 ms |
| **Decode off the presentation thread, bounded by bytes, with pre-roll** | 92% drops synchronous → 0 with decode-ahead + 1 s pre-roll |
| **Redraw the interface only when something changed** | a redraw spin cost 38% of pictures; cached panels recovered it |
| **Proxies are the default on modest hardware** | decode margin near zero at 720p H.264 on this machine |
| **Data windows everywhere in image processing** | a 64² ROI allocated 16.7 M floats |
| **Compute where the data is dirty, not everywhere** | the UCG already propagates dirtiness; the render cache must use it |
| **Real-time audio is a different engine shape, not a faster render** | 128 × 7.1.4 misses the deadline; threading moved p99 to pool wake latency |
| **Measure on a quiet machine, interleave comparisons** | two "defects" dissolved under adjudication; a timeline "cost" was thermal state |

**Architecture**

* *Cache hierarchy:* GPU texture → RAM frame cache → decode-ahead buffer → disk
  render cache → proxy → original. Each tier bounded by bytes and evicted by
  distance from the playhead and recency.
* *Job system:* bounded concurrency per resource class (CPU decode, GPU,
  disk I/O), priority to the playhead and to the user's current action,
  cancellation at documented granularity.
* *Media:* hardware decode where available with verified parity to software
  decode, software fallback always; multiresolution decode for proxy-less
  playback of high-resolution formats; tile/scanline streaming for EXR.
* *GPU:* one device shared by compute and presentation (currently two); zero-copy
  from decode to display where the platform allows; CPU mirrors of every kernel
  for verification and fallback.
* *Large projects:* the UCG must handle long-form films (thousands of shots,
  hundreds of thousands of nodes) — graph queries need benchmarks at that scale
  before production features rely on them.

**Performance gates** belong in CI as *work* measures where possible (allocations,
decodes per playback second, nodes touched per commit) rather than wall-clock
thresholds, which are flaky on shared hardware — the lesson of this session.

## 7. Storage strategy

Every byte belongs to one class (see `STORAGE-POLICY.md`):

| Class | Lives | Growth control |
|---|---|---|
| **Application** | install directory | ~20–40 MB binaries; must not grow with projects |
| **Project** (graph, op log, manifests) | project directory | small (1 MB today); op log compaction as an explicit user action |
| **User media** (originals) | wherever the user keeps it; referenced, never copied | never deleted automatically; offline is a state |
| **Project-derived, retained** (masters, deliverables) | `derived/` | never deleted without instruction |
| **Project-derived, regenerable** (proxies, optimised media) | `derived/` or user-chosen fast disk | explicit release with a report of what goes |
| **Caches** (render cache, frame cache, waveforms, thumbnails) | `cache/`, relocatable | hard byte budget, eviction policy, visible in the interface |
| **Temporary** | OS temp / `.cine/` | cleared on exit or next start |
| **Build artifacts** | developers only | not shipped; `target/` policy already written |

Principles: originals are referenced, never duplicated; every cache has a budget
before it exists; cache and proxy locations are relocatable to fast storage; the
interface shows what each class occupies and what releasing it would regenerate;
nothing the user shot is ever a cache.

---

## Sources (web research, 2026-09-15)

- Final Draft: [Features](https://www.finaldraft.com/products/features/), [Final Draft 13 Is Here](https://www.finaldraft.com/blog/final-draft-13-is-here)
- Movie Magic Scheduling: [Entertainment Partners](https://www.ep.com/movie-magic-scheduling/), [Script to screen scheduling](https://www.ep.com/blog/from-script-to-screen-how-to-create-a-film-tv-production-schedule/)
- Flow Production Tracking: [Autodesk overview](https://www.autodesk.com/products/flow-production-tracking/overview)
- DCP: [Cinepedia — Interop vs SMPTE](https://cinepedia.com/packaging/interop-smpte-dcp/), [Deluxe DCP delivery specifications](https://hpaonline.com/wp-content/uploads/2022/07/Deluxe_Source-and-DCP_Delivery_Specifications_v5-11_20220314-2.pdf)
- IMF: [SMPTE ST 2067-21 App 2E](https://smpte-ra.org/schemas/2067-21), [Netflix Backlot IMF delivery](https://partnerhelp.netflixstudios.com/hc/en-us/articles/115000614752-Backlot-Delivery-Instructions-for-IMF), [Netflix Photon](https://github.com/Netflix/photon)
- Timed text: [Netflix IMSC 1.1 Text Profile](https://partnerhelp.netflixstudios.com/hc/en-us/articles/360053755033-Netflix-IMSC-1-1-Text-Profile), [Netflix localization specs](https://partnerhelp.netflixstudios.com/hc/en-us/articles/7357416307603-Localization-Accessibility-and-Dubbing-Branded-Delivery-Specifications)
- Licences: [OpenColorIO](https://opencolorio.readthedocs.io/en/latest/aswf/license.html), [OpenImageIO relicensing](https://github.com/AcademySoftwareFoundation/OpenImageIO/blob/main/RELICENSING.md), [OpenFX licence](https://github.com/AcademySoftwareFoundation/openfx/blob/main/Support/LICENSE), [OpenJPEG](https://www.openjpeg.org/), [OpenJPH](https://github.com/aous72/OpenJPH)
- Audio: [EBU ADM renderer](https://github.com/ebu/ebu_adm_renderer), [libear announcement](https://tech.ebu.ch/news/2019/09/easy-nga-implementations-with-c-library-from-bbc-and-irt), [EAR Production Suite](https://ear-production-suite.ebu.io/)
- RAW SDKs: [Blackmagic RAW SDK](https://www.blackmagicdesign.com/developer/products/braw/sdk-and-software), [RED R3D SDK licence](https://www.red.com/legal/red-r3d-sdk-license-agreement)
- Dolby Vision: [Dolby Vision for content creators](https://professional.dolby.com/content-creation/dolby-vision-for-content-creators/), [Trims licence](https://visionlicense-us.dolby.com/products/dolby-vision-trims-license)
- AAF: [AMWA developers](https://aafassociation.org/developers.html)
- Unreal Engine: [EULA](https://www.unrealengine.com/eula/unreal)
- Codec patents: [Access Advance HEVC Advance](https://accessadvance.com/licensing-programs/hevc-advance/), [Via LA AVC/H.264](https://www.via-la.com/licensing-programs/avc-h-264/), [Tom's Hardware on H.264 fee changes](https://www.tomshardware.com/service-providers/streaming/h264-streaming-license-fees-jump-from-100000-to-4-5-million)
- Storyboarding: [Storyboarder](https://wonderunit.com/storyboarder/), [upstream licence issue](https://github.com/wonderunit/storyboarder/issues/1067)
- Pricing: [Resolve pricing](https://www.miracamp.com/learn/davinci-resolve/pricing), [Premiere Pro pricing](https://costbench.com/software/video-editing/adobe-premiere-pro/), [Media Composer subscription](https://www.keycodemedia.com/making-sense-of-avid-media-composer-subscription-vs-perpetual/)
