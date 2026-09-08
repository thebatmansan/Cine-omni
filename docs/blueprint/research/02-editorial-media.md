# CINE-OMNI Industry Blueprint — 02: Editorial & Media Engine

**Status:** research draft
**Date of research:** 2026-09-08
**Scope:** the editorial/NLE ecosystem and the media-engine layer CINE-OMNI does not yet have.
**Author context:** CINE-OMNI is Apache-2.0. Every license below was verified against
the project's own `LICENSE`/`COPYING` file or its official license page on the date
above; verification URLs are cited inline. Nothing here is stated from memory.
Anything that could not be verified is marked `UNVERIFIED`.

---

## 0. Where CINE-OMNI stands today

From `CHATGPT_CONTEXT.md` §1.4 and §9, and `docs/ARCHITECTURE.md`:

* `cine-timeline` (4,750 LOC, 61 tests) implements an **OpenTimelineIO
  schema-compatible** JSON reader/writer natively in Rust — `Timeline.1`,
  `Stack.1`, `Track.1`, `Clip.2`/`Clip.1`, `Gap.1`, `Transition.1`,
  `ExternalReference.1`, `MissingReference.1`, `GeneratorReference.1`,
  `Marker.2`/`.1`, `RationalTime.1`, `TimeRange.1` — plus real edit semantics
  (`ripple_trim`, `roll`, `slip`, `slide`, `insert`, `overwrite`, `lift`,
  `extract`, `split_at`, `join`, `diff`) and two-stage FFT dual-system audio sync.
  It is **not linked to the OTIO library**.
* **There is no media engine at all.** No video decode, no audio device I/O, no
  image file reading. Media is a path plus a range. `cine-audio` writes and reads
  RIFF/WAVE itself; `cine-comp` gets synthetic or provider-supplied passes;
  `cine-gpu` is headless `wgpu` compute plus an undriven render-pipeline definition.
* The UCG is authoritative for identity, dependency, provenance and change
  propagation. Any engine we adopt must sit **behind** that, never beside it.

That framing decides most of what follows: the question is not "which NLE should we
copy" — every mature open-source NLE is copyleft and therefore off-limits as source
— but "what do we put underneath `cine-timeline` so that a clip reference becomes
pixels and samples, without surrendering identity or our license."

---

## 1. The licensing spine: FFmpeg's LGPL core vs its GPL components

This is the single most consequential legal fact in this document, so it is
established first and in detail.

### 1.1 What FFmpeg's own license files say

FFmpeg's legal page states plainly:

> "FFmpeg is licensed under the GNU Lesser General Public License (LGPL) version
> 2.1 or later. ... FFmpeg incorporates several optional parts and optimizations
> that are covered by the GNU General Public License (GPL) version 2 or later. If
> those parts get used the GPL applies to all of FFmpeg."

— verified at <https://www.ffmpeg.org/legal.html>

`LICENSE.md` in the tree confirms the same structure: *"Most files in FFmpeg are
under the GNU Lesser General Public License version 2.1 or later (LGPL v2.1+)."*
— verified at <https://github.com/FFmpeg/FFmpeg/blob/master/LICENSE.md>

The GPL surface **inside FFmpeg itself** is small and enumerable. `LICENSE.md`
lists it explicitly:

| Category | Files (as listed in `LICENSE.md`) |
|---|---|
| Optional x86 asm | `libavcodec/x86/flac_dsp_gpl.asm`, `libavcodec/x86/idct_mmx.c`, `libavfilter/x86/vf_removegrain.asm` |
| Build/test tooling | `compat/solaris/make_sunver.pl`, `doc/t2h.pm`, `doc/texi2pod.pl`, `libswresample/tests/swresample.c`, `tests/checkasm/*`, `tests/tiny_ssim.c` |
| libavfilter filters | ~33 filters incl. `vf_blackframe.c`, `vf_boxblur.c`, `vf_delogo.c`, `vf_hqdn3d.c`, `vf_nnedi.c`, `vsrc_mptestsrc.c`; `vf_lensfun.c` is GPL v3+ |

None of these are codecs. **No decoder or encoder that CINE-OMNI needs lives in
FFmpeg's GPL set.** The GPL exposure in practice comes from *external* libraries
that `--enable-gpl` unlocks.

### 1.2 The four build modes

| Configure flags | Resulting license of the FFmpeg build | Usable by Apache-2.0 CINE-OMNI? |
|---|---|---|
| *(none)* — default | LGPL-2.1-or-later | **Yes**, under LGPL §6 (see 1.4) |
| `--enable-version3` | LGPL-3.0 (or GPL-3.0 if also `--enable-gpl`) | Yes for the LGPL-3 case; Apache-2.0 is one-way compatible *into* GPLv3, and LGPLv3 linking is permitted the same way |
| `--enable-gpl` | **GPL-2.0-or-later** | **No.** Apache-2.0 is incompatible with GPL-2.0-only, and even the "or later" reading would force the *combined work* to GPL, which we cannot do while remaining Apache-2.0 |
| `--enable-nonfree` | Not redistributable at all | **No** |

External libraries by category, per `LICENSE.md`:

* **Require `--enable-gpl` (GPL v2)** — `avisynth`, `frei0r`, `libcdio`, `libdavs2`,
  `librubberband`, `libvidstab`, **`libx264`**, **`libx265`**, `libxavs`, `libxavs2`,
  `libxvid`.
* **Require `--enable-version3` (LGPL v3)** — `gmp`, `libaribb24`, `liblensfun`.
* **Require `--enable-version3` (Apache-2.0 upstream)** — VMAF, mbedTLS, RK MPI,
  OpenCORE, VisualOn.
* **Require both (GPL v3)** — `smbclient`.
* **Nonfree** — Fraunhofer FDK AAC, OpenSSL (OpenSSL is LGPL-compatible but
  GPLv2/v3-incompatible).

### 1.3 What an LGPL-only FFmpeg build actually gives us

This is where the licensing question becomes an *architecture* question. Per
FFmpeg's own supported-formats matrix (<https://ffmpeg.org/general.html>):

| Codec | Native decode | Native encode | Encode via external lib |
|---|---|---|---|
| H.264 / AVC | **Yes** (native, LGPL) | No | `libx264` (**GPL-2.0**), OpenH264 (BSD-2) |
| H.265 / HEVC | **Yes** (native, LGPL) | No | `libx265` (**GPL-2.0**), `libkvazaar` (BSD-3) |
| **ProRes** | **Yes** | **Yes (native, LGPL)** | — |
| **DNxHD / VC-3** | **Yes** | **Yes (native, LGPL)** | — |
| JPEG 2000 | Yes | Yes (native) | OpenJPEG |
| VP9 | Yes (native) | External | `libvpx` (BSD-3) |
| AV1 | External | External | `libdav1d` (BSD-2, decode), `libaom` (BSD-2), `librav1e` (BSD-2), `libsvtav1` (BSD-3-clause-clear) |

**The decisive observation:** the two codecs a professional editorial and
mastering application genuinely depends on for its own intermediates and
deliverables — **ProRes and DNxHD/DNxHR — are native LGPL encoders in FFmpeg.**
The GPL-only libraries (`libx264`, `libx265`) are needed *only* for high-quality
software H.264/HEVC **encode**, i.e. review copies, dailies proxies and web
deliverables. H.264/HEVC **decode** — which is what an NLE spends 99% of its time
doing — is entirely native and LGPL.

Practical LGPL-clean substitutes for the one gap:

* Hardware H.264/HEVC encoders (NVENC, VAAPI, VideoToolbox, QSV, AMF) — these are
  FFmpeg wrappers around vendor APIs and do **not** require `--enable-gpl`.
* **OpenH264** (Cisco, BSD-2-Clause code). Cisco's *binary* license adds a patent
  wrinkle worth reading carefully: Cisco grants its AVC/H.264 Patent Portfolio
  License from MPEG LA "at no cost to you" **only if** the binary is *separately
  downloaded by the end user* rather than pre-integrated, the end user can
  enable/disable it, and the attribution "OpenH264 Video Codec provided by Cisco
  Systems, Inc." is displayed. Verified at
  <https://www.openh264.org/BINARY_LICENSE.txt>. Bundling Cisco's binary in our
  installer forfeits the royalty shelter. Firefox's download-on-first-use flow is
  the canonical compliant pattern.
* **Kvazaar** (HEVC encode, Ultra Video Group / Tampere University). Now
  **BSD-3-Clause** — the README states "An open-source HEVC encoder licensed under
  3-clause BSD"; latest version seen is v2.3.2 (2025-09-16), repo actively
  maintained. Verified at <https://github.com/ultravideo/kvazaar>. Note this is a
  relicense: older material describes Kvazaar as LGPL-2.1, and the
  `scalable-kvazaar` variant is still LGPL-2.1. Pin the version you audit.
* **Commercial license escape hatch.** VideoLAN states x264 *"is also available
  under a commercial license. Contact x264licensing@videolan.org for more
  details"* (verified at <https://www.videolan.org/developers/x264.html>, which
  also confirms the free license is the **GNU GPL v2**). A paid x264/x265 licence
  is therefore a viable way to ship best-in-class H.264/HEVC encode from an
  Apache-2.0 product without a copyleft obligation — a commercial decision, not an
  architectural one, and one that can be deferred.

### 1.4 LGPL-2.1 §6: what "we may link it" actually costs

LGPL-2.1 §5 defines "a work that uses the Library" as *"a program that contains no
derivative of any portion of the Library, but is designed to work with the Library
by being compiled or linked with it."* §6 then permits combining such a work with
the Library and distributing the combination **under terms of our own choosing**
(i.e. Apache-2.0), provided we:

1. give prominent notice that the Library is used and is covered by the LGPL;
2. include a copy of the LGPL and the copyright notices; **and**
3. satisfy one of §6(a)–(e).

Verified at <https://opensource.org/license/lgpl-2-1>.

The two options that matter:

* **§6(b) — "use a suitable shared library mechanism for linking with the Library."**
  Dynamic linking against a shared `libavcodec.so` / `.dylib` / `.dll` that the
  user can replace. This is the cheap path: no source or object-file escrow, no
  relinking kit. It is the reason essentially every commercial application that
  ships FFmpeg does so as a shared library.
* **§6(a) — static linking.** Permitted, but we must supply *"complete
  machine-readable source code for the Library including modifications, plus object
  code or source for the work so users can modify the Library and relink."* That
  means shipping our own `.o`/`.rlib` object files so a user can relink CINE-OMNI
  against a modified FFmpeg. Technically possible with Rust, operationally
  miserable, and it hands out a linkable object form of our proprietary-adjacent
  binary.

**Conclusion: dynamic linking, always.** Static linking of FFmpeg is legally
available but architecturally and commercially unattractive.

### 1.5 Patent and royalty exposure is a *separate* axis

Copyright license and patent license are orthogonal. An LGPL-clean build says
nothing about codec royalties. FFmpeg's own legal page is explicit that the
project will not advise: *"We do not know, we are not lawyers so we are not
qualified to answer this"* (<https://www.ffmpeg.org/legal.html>). Detailed pool
analysis is in §7.

---

## 2. Primary project profiles (31-point)

### 2.1 FFmpeg

| # | Point | Finding |
|---|---|---|
| 1 | Name | FFmpeg |
| 2 | Repo | <https://git.ffmpeg.org/ffmpeg.git>, mirror <https://github.com/FFmpeg/FFmpeg> |
| 3 | License | **LGPL-2.1-or-later** core; **GPL-2.0-or-later** when built `--enable-gpl`; LGPL-3/GPL-3 with `--enable-version3`; unredistributable with `--enable-nonfree`. Verified: <https://www.ffmpeg.org/legal.html>, <https://github.com/FFmpeg/FFmpeg/blob/master/LICENSE.md> |
| 4 | Copyright owner | The FFmpeg project — many individual contributors, no CLA, no single assignee. This matters: **there is no one who can relicense FFmpeg**, so the LGPL/GPL split is permanent. |
| 5 | Language | C (C99/C11) with hand-written x86/ARM/RISC-V assembly; build system is a hand-rolled shell `configure` + Make |
| 6 | Architecture | Layered libraries: `libavutil` (primitives, hwcontext), `libavcodec` (codecs, parsers, bitstream filters), `libavformat` (demux/mux, protocols), `libswscale` (pixel format/scale), `libswresample` (audio resample/format), `libavfilter` (filter graphs), `libavdevice` (capture/playback devices). `ffmpeg`/`ffplay`/`ffprobe` are thin CLI clients of these. |
| 7 | UI architecture | None (CLI only). `ffplay` uses SDL. Irrelevant to us. |
| 8 | Rendering architecture | `libavfilter` is a pull-based filter graph over `AVFrame`s. As of FFmpeg 8/9 there is a growing set of **Vulkan compute-shader codecs and filters** (FFv1 enc+dec, ProRes RAW decode shipped; ProRes and VC-2 in review) — a vendor-neutral GPU path that is not the fixed-function video engine. |
| 9 | Media architecture | The reference implementation of essentially every container and codec in professional use. Hardware acceleration is abstracted behind `AVHWDeviceContext`/`AVHWFramesContext` covering VAAPI, NVDEC/NVENC (CUDA), VideoToolbox, D3D11VA/DXVA2/D3D12VA, QSV, AMF, Vulkan Video. |
| 10 | Plugin architecture | None at runtime — everything is compiled in. Extension means patching and rebuilding. This is a significant constraint: we cannot "add a codec" to a system FFmpeg. |
| 11 | Data model | `AVFormatContext` → `AVStream` → `AVPacket` → `AVCodecContext` → `AVFrame`. Time is `AVRational` time bases plus `pts`/`dts`. Notably this maps cleanly onto `ucg-schema`'s existing `RationalTime`. |
| 12 | File formats | Effectively all of them: MOV/MP4/MXF/AVI/MKV/GXF/LXF/WAV/AIFF/BWF, plus image sequences (DPX, EXR via its own decoder, TIFF, PNG, JPEG). |
| 13 | Import/export | Demux + decode + encode + mux. Native LGPL **encoders for ProRes and DNxHD/VC-3** — the two professional intermediate codecs — plus FFV1, JPEG2000, and PCM/BWF audio. |
| 14 | GPU architecture | Two distinct paths: (a) fixed-function video engines via the hwaccel abstraction; (b) Vulkan compute shaders (new in 8.x/9.x). Zero-copy surface export to a renderer works via `AV_PIX_FMT_VAAPI`/`CUDA`/`VIDEOTOOLBOX`/`D3D11`/`VULKAN` frame handles. |
| 15 | CPU architecture | Extremely well optimised SIMD (SSE/AVX2/AVX-512, NEON, SVE, RVV). This is a decade-plus of assembly work that nobody sane reimplements. |
| 16 | Threading | Frame-level and slice-level threading inside decoders (`thread_count`, `thread_type`); the libraries are thread-safe per-context but **not** per-context-shared. Filter graphs are single-threaded per graph. |
| 17 | Caching | None. FFmpeg has no notion of a frame cache, a proxy, or a render cache. That is entirely the host application's job — which is exactly the layer CINE-OMNI must build. |
| 18 | Extensibility | Poor at runtime, excellent at build time. |
| 19 | Maturity | The most mature media library in existence; the substrate of essentially every video product shipped since 2005. |
| 20 | Activity | Extremely active. **9.0.1 "Lei" released 2026-08-12**; 8.1.2, 8.0.3, 7.1.5, 6.1.6, 5.1.10, 4.4.8 all received point releases in June 2026. Verified: <https://ffmpeg.org/download.html> |
| 21 | Relevant modules | `libavformat` (demux, seek, MXF/MOV), `libavcodec` (decode/encode, hwaccel), `libswscale`, `libswresample`, `libavutil/hwcontext_*` |
| 22 | Reusable components | The whole library, **as a dynamically linked dependency** — not as copied source. |
| 23 | Reusable algorithms | None need copying; we call them. |
| 24 | Integration options | **B LIBRARY** (dynamic link, LGPL build) or **C ENGINE** (separate worker process). |
| 25 | Conflicts | None architecturally: FFmpeg has no identity model, no project model, no persistence. It is a pure transform layer, which is exactly what the UCG thesis wants underneath it. |
| 26 | Licensing constraints | Must build **without `--enable-gpl` and without `--enable-nonfree`**; must link dynamically (LGPL-2.1 §6(b)); must ship the LGPL text, the copyright notices, and prominent notice of use. Static linking is legal under §6(a) but requires shipping relinkable object code. |
| 27 | Incorporable under Apache-2.0? | **No — not as source.** FFmpeg source cannot be copied into the Apache-2.0 tree (LGPL is copyleft on the library itself). But it may be **linked** and the combined work stays Apache-2.0 under §6. |
| 28 | Remain isolated? | Yes — behind a single adapter crate. |
| 29 | External engine? | Optional. Recommended as a *fallback* isolation mode, not the default. |
| 30 | Adapter needed? | **Yes** — `cine-media` (see §6). |
| 31 | Reimplement instead? | **No.** Reimplementing FFmpeg is not a project, it is a career. |

### 2.2 MLT Framework

| # | Point | Finding |
|---|---|---|
| 1–2 | Name / repo | MLT Multimedia Framework — <https://github.com/mltframework/mlt>, <https://www.mltframework.org> |
| 3 | License | **Core framework, `mlt++`, `libmvcp`: LGPL-2.1-or-later.** Verified: root `COPYING` is the LGPL v2.1 text (<https://raw.githubusercontent.com/mltframework/mlt/master/COPYING>) and source headers e.g. `src/framework/mlt_service.c` read "either version 2.1 of the License, or (at your option) any later version". The `melt` / `melted` / `melted-console` / `melted-client` **applications are GPL-2.0** (<https://www.mltframework.org/docs/copyrightpolicy/>). Module licensing is **not uniform**: the `plusgpl` module is deliberately GPL-segregated (<https://github.com/mltframework/mlt/tree/master/src/modules/plusgpl>). |
| 4 | Copyright owner | **Meltytech, LLC** — the copyright policy requires assignment to Meltytech for framework and core module contributions. A single assignee means MLT *could* be relicensed, unlike FFmpeg. |
| 5 | Language | C, with a `mlt++` C++ wrapper |
| 6 | Architecture | A **service graph**: `producer` (source) → `filter` → `transition` → `consumer` (sink), with a "profile" describing frame geometry and rate. Frames are pulled through the graph. |
| 7 | UI architecture | None — MLT is headless. Three independent GUIs (Kdenlive, Shotcut, Flowblade) sit on top. |
| 8–9 | Rendering / media | Delegates all codec work to FFmpeg through the `avformat` module. GPU compositing via the optional `movit` module (OpenGL). |
| 10 | Plugin architecture | Dynamically loaded modules registering services by name, with YAML metadata describing parameters. Good design; genuinely extensible at runtime, unlike FFmpeg. |
| 11–12 | Data model / formats | The service graph *is* the data model, and **MLT XML is its serialization** — so an MLT project file is directly executable by `melt`. Elegant, and a real influence worth noting. |
| 13 | Import/export | MLT XML; everything else via FFmpeg. |
| 14–16 | GPU / CPU / threading | Multi-threaded frame processing; GPU only through `movit`/`qt` modules. |
| 17 | Caching | Frame cache in the framework; no project-level proxy or render-cache concept (that lives in Kdenlive/Shotcut). |
| 19–20 | Maturity / activity | Mature and **very active** — v7.40.0 released 2026-06-25, last push 2026-09-07. |
| 21–23 | Reusable | The **producer/filter/transition/consumer service-graph abstraction with an executable XML serialization** is the most reusable *idea* in this survey — it is validated by three independent downstream GUIs. |
| 24 | Integration | **G INSPIRED REIMPLEMENTATION** |
| 25 | Conflicts | MLT owns the graph, the profile, and the frame lifecycle. CINE-OMNI's UCG already owns the graph and identity. Adopting MLT would mean two competing graph engines with a translation layer between them — a direct violation of the "UCG is authoritative" thesis. |
| 26–27 | Licensing | LGPL-2.1 core is linkable (same §6 analysis as FFmpeg) but the module tree is mixed and requires per-module audit. Not incorporable as source. |
| 28–31 | Verdict | Do not link; do not fork. Learn the service-graph shape and build it natively in Rust over the UCG. MLT's own value-add over FFmpeg is graph orchestration — which is precisely the thing CINE-OMNI already has and must not duplicate. |

### 2.3 Kdenlive

| # | Point | Finding |
|---|---|---|
| 1–2 | Name / repo | Kdenlive — <https://invent.kde.org/multimedia/kdenlive> |
| 3 | License | **GPL-3.0 family.** Verified directly: `src/main.cpp` carries `SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL`, and `LICENSES/GPL-3.0-or-later.txt` exists in the tree (GPLv3, 29 June 2007). Per-file SPDX varies across GPL-2.0-or-later and GPL-3.0-or-later under KDE's REUSE policy (<https://community.kde.org/Policies/Licensing_Policy>); exact per-file audit is `UNVERIFIED`. |
| 4 | Copyright owner | Individual contributors under KDE e.V. stewardship (`SPDX-FileCopyrightText: 2007 Marco Gittler`, `2008 Jean-Baptiste Mardelle`, …) |
| 5–7 | Language / architecture / UI | C++ with Qt and KDE Frameworks 6; a thin native GUI over MLT. |
| 8–9 | Rendering / media | **All playback and rendering delegated to MLT**, which delegates codecs to FFmpeg. Effects via frei0r, LADSPA, SoX through MLT. |
| 11–12 | Data model / format | `.kdenlive` XML, built on and interoperating with MLT XML. |
| 19–20 | Maturity / activity | The most mature and best-resourced open-source GUI NLE. Release 26.08.0 (August 2026); GitLab `last_activity_at` 2026-09-08. NLnet grant funding announced Sept 2025. |
| 24 | Integration | **H REJECT** |
| 25–27 | Conflicts / licensing | GPL-3.0. **Apache-2.0 → GPLv3 is one-way**: we may contribute Apache-2.0 code *into* Kdenlive, never the reverse. No Kdenlive source may be read-and-copied into CINE-OMNI. |
| 28–31 | Verdict | Reject for code. Study only as a **UX and workflow reference**, and treat "thin GUI over a headless engine" as confirmation of our own layering. Contributors should avoid reading Kdenlive source before writing equivalent CINE-OMNI features, to keep the clean-room argument intact. |

### 2.4 OpenTimelineIO

| # | Point | Finding |
|---|---|---|
| 1–2 | Name / repo | OpenTimelineIO — <https://github.com/AcademySoftwareFoundation/OpenTimelineIO> |
| 3 | License | **Apache-2.0** (SPDX `Apache-2.0` per the GitHub repository metadata; `LICENSE.txt` is unmodified standard Apache-2.0 text with no appended exceptions). Historically the project shipped a **"Modified Apache 2.0 License"** (visible on PyPI as late as 0.12.0, classified there as "Other/Proprietary License"); the modification was removed during ASWF graduation. Verified: <https://raw.githubusercontent.com/AcademySoftwareFoundation/OpenTimelineIO/main/LICENSE.txt>, <https://api.github.com/repos/AcademySoftwareFoundation/OpenTimelineIO>, <https://pypi.org/project/OpenTimelineIO/> |
| 4 | Copyright owner | Contributors to the OpenTimelineIO project; originated at Pixar Animation Studios, now hosted by the Academy Software Foundation (Linux Foundation). |
| 5 | Language | C++ core (`opentime`, `opentimelineio`) with Python bindings via pybind11. Primary language reported as C++. |
| 6 | Architecture | A serializable object model with a schema-versioned JSON encoding, plus a plugin layer (adapters, media linkers, hook scripts, SchemaDef plugins) that is **Python-only**. |
| 11 | Data model | `Timeline` → `Stack` → `Track` → `Item` (`Clip`/`Gap`/`Transition`), `MediaReference` (`External`/`Missing`/`Generator`/`ImageSequence`), `Marker`, `Effect` (`LinearTimeWarp`/`FreezeFrame` and otherwise opaque), `RationalTime`/`TimeRange`/`TimeTransform`, plus a free-form `metadata` dict on everything. |
| 12–13 | Formats / import-export | Native `.otio` JSON. **Everything else is an adapter** — and the adapters are Python packages living in separate repos under the `OpenTimelineIO` GitHub org. |
| 19 | Maturity | **Still pre-1.0 after ten years.** Latest release **v0.18.1, 2025-11-09**, and every GitHub release including it is marked *pre-release*; PyPI shows the same. Verified: <https://github.com/AcademySoftwareFoundation/OpenTimelineIO/releases>, <https://pypi.org/project/OpenTimelineIO/> |
| 20 | Activity | Active — last push 2026-08-07; ~1,976 stars; 206 open issues. |
| 24 | Integration | **E IMPORT/EXPORT** (schema) + selective **D BRIDGE** (adapters) — see §5 for the full adopt-vs-keep argument. |
| 25 | Conflicts | OTIO's C++ core wants to own the timeline object graph and its identity (objects are addressed by pointer/`SerializableObject`, not by stable UUID). CINE-OMNI's UCG owns identity by UUID. Adopting the C++ core would mean the same duplicate-graph problem as MLT, and a *worse* one — OTIO has no stable identity model to reconcile with ours, which is why `cine-timeline` already stamps `ucg_id` into item metadata. |
| 26–27 | Licensing | **Apache-2.0 — fully and cleanly incorporable.** This is the one primary project with no licensing obstacle whatsoever. |
| 31 | Reimplement? | Already done for the schema. Do **not** reimplement the adapters. |

### 2.5 OpenImageIO

| # | Point | Finding |
|---|---|---|
| 1–2 | Name / repo | OpenImageIO (OIIO) — <https://github.com/AcademySoftwareFoundation/OpenImageIO> |
| 3 | License | **Apache-2.0** for code (CC-BY-4.0 for documentation). The project relicensed from BSD-3-Clause in a 2023 community effort; the README states "over 99.86% of lines of code have been relicensed to Apache-2.0". Verified: <https://raw.githubusercontent.com/AcademySoftwareFoundation/OpenImageIO/main/LICENSE.md>, <https://raw.githubusercontent.com/AcademySoftwareFoundation/OpenImageIO/main/README.md> |
| 4 | Copyright owner | "Contributors to the OpenImageIO project"; ASWF-hosted, originated by Larry Gritz / Sony Pictures Imageworks. |
| 5 | Language | C++ (C++17-era), with Python bindings. |
| 6 | Architecture | `ImageInput`/`ImageOutput` abstract format readers/writers loaded as **DSO plugins**; `ImageSpec` carries geometry + arbitrary metadata; `ImageBuf` is an in-memory buffer; `ImageBufAlgo` is a large image-operation library; `ImageCache` and `TextureSystem` sit above them. Verified: <https://openimageio.readthedocs.io/en/latest/> |
| 8 | Rendering | Not a renderer. `TextureSystem` provides antialiased texture / shadow-map / environment lookups for renderers. |
| 9 | Media architecture | Still images only (plus movie files readable as individual frames). No audio, no timeline, no realtime decode. |
| 14–17 | GPU / threading / caching | CPU-oriented. **`ImageCache` is the crown jewel**: a fully thread-safe tile cache with a configurable MB budget and automatic LRU closing of open file handles, so "truly huge amounts of texture may be accessed with a low memory footprint". Per-thread state is passed as an opaque `Perthread` handle to avoid contention; a shared `TextureSystem` implies a shared underlying `ImageCache`. Verified: <https://openimageio.readthedocs.io/en/stable/imagecache.html>, <https://openimageio.readthedocs.io/en/stable/texturesys.html> |
| 12 | File formats | 30+: OpenEXR, **DPX, Cineon**, TIFF, PNG, JPEG, JPEG XL, HEIF/HEIC/AVIF, WebP, BMP, Targa, PSD, SGI, Softimage PIC, RAW camera files, OpenVDB, Ptex, DICOM, FITS. This is exactly the DI/VFX plate-reading set CINE-OMNI lacks. |
| 19–20 | Maturity / activity | Extremely mature and extremely active: **v3.1.17.0 released 2026-09-01**, with monthly point releases on both 3.0 and 3.1 branches and a 3.2.0 beta on 2026-09-02. Verified: <https://github.com/AcademySoftwareFoundation/OpenImageIO/releases> |
| 22–23 | Reusable | The `ImageCache` **design** (tile-granular, MB-budgeted, thread-safe, handle-LRU) is directly the pattern CINE-OMNI's frame cache should follow. And because OIIO is Apache-2.0, its *source* is legally reusable too — a rare and valuable position. |
| 24 | Integration | **B LIBRARY** for image file I/O (via a C FFI shim), with **G INSPIRED REIMPLEMENTATION** for the cache design. |
| 25 | Conflicts | None. OIIO has no identity model and no project model. |
| 26–27 | Licensing | **Apache-2.0 — no constraints at all.** Source may be copied, modified, statically linked, and redistributed, subject only to the NOTICE/attribution requirements. |
| 31 | Reimplement? | Partly. Pure-Rust `exr`, `image`, `png`, `tiff`, `jpeg-decoder` cover the common cases without a C++ dependency; OIIO earns its place specifically for **DPX, Cineon, RAW, Ptex, DICOM and the long tail** — and for `ImageCache`. A staged approach is right: pure-Rust first, OIIO as the long-tail plugin. |

---

## 3. Hardware decode / encode: what actually exists (Question 2)

### 3.1 The vendor matrix

| API | Platform / vendor | Runtime requirement | FFmpeg hwaccel name |
|---|---|---|---|
| **VAAPI** | Linux, Intel + AMD | `libva` + a vendor driver at `/usr/lib/dri/${LIBVA_DRIVER_NAME}_drv_video.so` — `iHD` (Intel Broadwell+), `i965` (legacy Intel), `radeonsi` (AMD/Mesa). NVIDIA only via the third-party `nvidia-vaapi-driver` shim. | `vaapi`; `h264_vaapi`, `hevc_vaapi`, `av1_vaapi` |
| **NVDEC / NVENC** | Windows + Linux, NVIDIA | Only the NVIDIA display driver on the end-user machine — **no CUDA toolkit at runtime** | `cuda`/`nvdec`; `h264_nvenc`, `hevc_nvenc`, `av1_nvenc` |
| **VideoToolbox** | macOS / iOS | Built into the OS | `videotoolbox`; `h264_videotoolbox`, `hevc_videotoolbox`, `prores_videotoolbox` |
| **D3D11VA / DXVA2 / D3D12VA** | Windows, all vendors | OS + GPU driver | `d3d11va`, `dxva2`; D3D12VA decode landed later |
| **QSV** | Intel (Sandy Bridge+) | Intel VPL / media driver | `qsv`; `h264_qsv`, `hevc_qsv`, `av1_qsv` |
| **AMF** | AMD | AMD driver | `h264_amf`, `hevc_amf`, `av1_amf` |
| **Vulkan Video** | cross-vendor | Vulkan 1.3+ driver with the video extensions | `vulkan`; `av1_vulkan` and others |

**The NVIDIA licensing detail matters and is good news.** FFmpeg builds against
`nv-codec-headers`, whose header license block (e.g. `ffnvcodec/nvEncodeAPI.h`) is an
**MIT/Expat-style permission notice**, Copyright NVIDIA Corporation — freely
redistributable. The full proprietary **NVIDIA Video Codec SDK License Agreement** is
far more restrictive (object-code-only distribution, no sublicensing chain, and an
explicit prohibition on using the SDK in a way that subjects it to a copyleft licence)
and applies only if you link the SDK binaries directly rather than going through
FFmpeg. Verified: <https://developer.nvidia.com/nvidia-video-codec-sdk-license-agreement>,
<https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/ffmpeg-with-nvidia-gpu/index.html>.
**Take the FFmpeg/nv-codec-headers path.**

### 3.2 Apple Silicon specifics

VideoToolbox covers H.264, HEVC and **ProRes encode and decode** in hardware.
AV1 **decode** arrived with M3-class silicon (and iPhone 15 Pro / M4 iPad); there is
still **no AV1 hardware encode** on Apple Silicon. "Max" chips add a second encode
engine, "Ultra" doubles both — relevant to any concurrent-transcode capacity claim.
Sources: <https://bitmovin.com/blog/apple-av1-support/>,
<https://developer.apple.com/forums/thread/722933>.

### 3.3 Vulkan Video status, 2026

* **Decode ratified:** H.264, H.265, AV1, and VP9 (VP9 as of Vulkan 1.4.317, ~June 2025).
* **Encode ratified:** H.264, H.265 (finalized Dec 2023, SDK 1.3.275) and **AV1**
  (Vulkan 1.3.302), plus Encode Quantization Map and Encode Intra-Refresh extensions
  (July 2025).
* **Driver reality is uneven.** Mesa RADV: H.264/H.265 decode since 23.1.2, encode
  since 24.1.0, AV1 decode 24.0.3, VP9 decode 25.2. Mesa ANV (Intel): decode 23.1.2,
  encode 24.3, AV1 decode 25.0.0, VP9 decode 25.2 — and Vulkan H.264/H.265 encode on
  Gen12.5/Alchemist was *disabled then re-enabled* in July 2026 for testing reasons.
  Proprietary NVIDIA and AMD drivers claim full coverage.
* **FFmpeg 9.0 treats Vulkan as a fully integrated, vendor-neutral backend** spanning
  decode, encode and compute filters. FFmpeg 8.x's `av1_vulkan` was the first
  cross-vendor mainline AV1 encoder.

Sources: <https://blogs.igalia.com/vjaquez/vulkan-video-status/>,
<https://www.khronos.org/blog/khronos-announces-vulkan-video-encode-av1-encode-quantization-map-extensions>,
<https://www.phoronix.com/news/FFmpeg-Vulkan-Starts-2026>.

**Assessment for CINE-OMNI:** Vulkan Video is *strategically* the right long-term
answer — one API, cross-vendor, and it shares a device with `wgpu`'s Vulkan backend
so zero-copy is natural. It is **not** yet the right *only* answer: driver coverage
in 2026 is still a lottery, and macOS has no Vulkan at all without MoltenVK (which
does not expose video extensions). Treat it as a preferred backend where present,
not a foundation.

### 3.4 What real cross-platform applications do

This is the most instructive finding in the hardware section, because it cuts
against the obvious answer:

* **OBS Studio** uses **per-platform native encoder plugins** — direct NVENC, direct
  AMF, Intel VPL for QSV, native VideoToolbox — and reserves an FFmpeg-based wrapper
  specifically for VAAPI on Linux. All its hardware encoders implement **texture-based
  encoding** (GPU→GPU, no CPU round trip) where the platform allows.
  (<https://deepwiki.com/obsproject/obs-studio/4.4.2-hardware-video-encoders>)
* **Chromium** uses distinct per-platform decoder classes — `D3D11VideoDecoder`,
  `VideoToolboxVideoDecoder`, `VaapiVideoDecoder` — with FFmpeg only as the software
  fallback.
* **DaVinci Resolve**'s internals are `UNVERIFIED`, but its behaviour (per-vendor
  debayer paths for BRAW/RED/ARRIRAW, platform-specific cache codec defaults —
  DNxHR SQ on Windows, ProRes on Mac) is consistent with native per-platform code.

The pattern: **FFmpeg's hwaccel abstraction is used for broad-compatibility ingest,
while the interactive playback and encode paths are written natively per platform**
when the application needs texture-level zero-copy with its own renderer.

### 3.5 Zero-copy into the renderer

| Platform | Mechanism | Status |
|---|---|---|
| Linux | DMA-BUF fd + DRM format modifiers → Vulkan via `VK_EXT_external_memory_dma_buf` + `VK_EXT_image_drm_format_modifier` | Production-proven |
| macOS | **IOSurface** — VideoToolbox output surfaces are IOSurface-backed; a Metal texture binds directly, including per-plane binding for NV12 | Production-proven |
| Windows | D3D11 shared handles / D3D12 `OpenSharedHandle` | Works, with documented driver-version friction |
| Cross | `VK_KHR_external_memory` (+`_fd`/`_win32`/`_dma_buf`) and `VK_KHR_external_semaphore_*` | The generic path |

**`wgpu` in 2026** has genuine external-memory support on the Vulkan backend:
`vulkan::Device::texture_from_dmabuf_fd()` behind the
`VULKAN_EXTERNAL_MEMORY_FD` / `VULKAN_EXTERNAL_MEMORY_DMA_BUF` feature flags, plus
`vulkan::Queue::add_wait_semaphore` / `remove_wait_semaphore` for waiting on
externally signalled semaphores without blocking the CPU at submit
(<https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md>). This is directly usable
by `cine-gpu`. **Parity on the Metal and D3D12 backends is `UNVERIFIED` and likely
partial** — confirm against the current `wgpu` changelog before committing.
`lumina-video` (<https://github.com/lumina-video/lumina-video>) claims a Rust
zero-copy wgpu video SDK spanning IOSurface / DMA-BUF / D3D11→D3D12 / AHardwareBuffer;
that claim is `UNVERIFIED` beyond its README but it is worth evaluating as prior art.

**Plan for a CPU-copy fallback on every platform.** The theoretical zero-copy path
existing is not the same as it working on a given driver/version pair.

### 3.6 How CINE-OMNI should abstract it

```
cine-media::hw
  enum HwBackend { Vaapi, Nvdec, VideoToolbox, D3d11va, Vulkan, Qsv, Amf, Software }
  trait FrameSink {
      // GPU-resident handle the renderer can import without a copy…
      fn accept_gpu(&mut self, surface: HwSurface) -> Result<wgpu::Texture>;
      // …or the guaranteed fallback.
      fn accept_cpu(&mut self, frame: CpuFrame) -> Result<wgpu::Texture>;
  }
```

Concretely:

1. **One abstraction, two tiers.** Tier 1 is a *capability probe* at startup that
   enumerates available backends per codec and records the result in the UCG as
   machine-scoped (not project-scoped) metadata. Tier 2 is the decode path itself.
2. **Never let the hardware backend be part of project identity.** A project edited
   on an NVENC machine must open identically on a VideoToolbox machine. Hardware
   choice is a *rendering hint*, never a graph attribute that a reactor can see —
   otherwise the reactor cascade becomes machine-dependent and idempotence breaks.
3. **Deterministic fallback ordering**, declared not discovered: prefer the backend
   that can hand `cine-gpu` a zero-copy surface on this platform, then any hardware
   backend, then software. Log the choice; never silently vary output.
4. **Encode is a different decision from decode.** Hardware *decode* is a
   performance optimisation with (near-)identical output. Hardware *encode* changes
   the delivered pixels. For **mastering deliverables, use software ProRes/DNxHR
   encode (native, LGPL, deterministic)**; use hardware encode only for proxies,
   review copies and scrub caches where quality is subordinate to speed.

---

## 4. Proxy media, caches and background rendering (Question 3)

### 4.1 What the mature NLEs actually do

**Adobe Premiere Pro** separates three distinct cache classes:
* **Media Cache** — `.cfa` (Conformed Audio: decoded/resampled PCM, can be ~half the
  size of the source video) and `.pek` (peak/waveform data, small), plus `.ims`
  indexes. Automatically generated, safely deletable, regenerated on demand.
* **Proxies** — created via **Ingest presets** (`.epr` files) that transcode on
  import and auto-associate the proxy with the original; externally created proxies
  attach via "Attach Proxies", conventionally matched by a `_proxy` filename suffix.
* **Preview/render files** — the render bar: **red** = no preview and playback will
  likely drop frames; **yellow** = no preview but probably real-time; **green** = a
  rendered preview file exists. Export uses **smart rendering** to copy through
  segments whose codec already matches the target and which carry no effects.
  Sources: <https://helpx.adobe.com/premiere/desktop/troubleshooting/media-issues/manage-media-cache.html>,
  <https://helpx.adobe.com/premiere-pro/desktop/organize-media/ingest-proxy-workflow/ingest-and-proxy-workflow.html>,
  <https://blog.adobe.com/en/publish/2011/02/20/red-yellow-and-green-render-bars>

**DaVinci Resolve** — the closest existing product to CINE-OMNI's "one application"
thesis, organised as Media / Cut / Edit / Fusion / Color / Fairlight / Deliver pages
over one project database. It distinguishes:
* **Optimized media** — transcodes camera-native to an edit-friendly intermediate
  (DNxHR SQ on Windows, ProRes 422 LT on Mac by convention).
* **Render Cache** in two modes: **Smart** (Resolve decides what needs caching) and
  **User** (explicit, per clip / per node / per effect). "Render Cache Fusion Output"
  additionally caches RAW debayered at timeline resolution so output does not
  re-debayer.
* **Smart Relink** switches timeline clips from proxy back to full-res "stream" mode
  when grading starts — the **RAW debayer parameters travel with the clip regardless
  of which essence is currently mounted**. This is the single most important design
  lesson: *the grade attaches to the logical clip, not to the file currently backing it.*
* The literal on-disk `CacheClip` naming convention is `UNVERIFIED`.
  Sources: <https://www.blackmagicdesign.com/products/davinciresolve/>,
  <https://davinciresolveclub.com/davinci-resolve-render-cache/>,
  <https://www.sanbila.com/workflows/braw-in-davinci-resolve>

**Avid Media Composer** — the strictest identity model, and the one closest to the
UCG thesis:
* **AMA linking** (read camera-native in place) vs **transcode into managed MXF
  OP-Atom** under `<Volume>/Avid MediaFiles/MXF/<n>/`, where video and *each audio
  channel* are separate single-essence files recombined into one logical Clip.
* `msmMMOB.mdb` is a per-folder media database Media Composer builds by scanning.
* **Identity is the SMPTE UMID** — a globally unique essence identifier embedded in
  the MXF at creation, designed to survive copies, renames and relinks.
* Media Composer 2022.10 split **Relink** into two distinct flows, one for Managed
  Media and one for Linked (AMA) media — i.e. Avid's data model treats these as
  architecturally different identity paths.
  Sources: <https://docs.hedge.video/editready/media-composer-guide>,
  <https://en.wikipedia.org/wiki/Unique_Material_Identifier>,
  <https://elements.tv/blog/avid-media-composer-2022-10-whats-new/>

**Final Cut Pro** — Library (a macOS package) → Events → Clips/Projects, with media
either **managed** (physically inside the library bundle) or **external**, freely
convertible in either direction. Optimized media is ProRes 422; proxy media is
ProRes 422 Proxy or H.264 at a selectable 12.5%–100% scale. Transcoding, analysis
and rendering all run as **automatic background tasks** after import, disableable in
preferences.
Sources: <https://support.apple.com/guide/final-cut-pro/intro-to-media-management-ver4e2e1bd/mac>,
<https://support.apple.com/guide/final-cut-pro/create-optimized-and-proxy-files-verb8e5f6fd/mac>

### 4.2 The invariants underneath all four

1. **There are three independent cache classes, and conflating them is the classic
   mistake.**
   * *Source-derived* (audio conform, waveform peaks, thumbnails, index/seek tables) —
     keyed by **source media identity alone**. Survives every edit.
   * *Proxy media* — an alternate essence for the same logical clip, keyed by
     **source identity + proxy recipe**. Survives every edit.
   * *Render/preview cache* — keyed by **(source identity + full effect stack +
     timing + output settings)**. Invalidated by any edit that changes the rendered
     result.
2. **Timeline clips reference logical identity, never a file path.** The path is
   resolved through an indirection layer at runtime. Avid uses UMID; Resolve and FCP
   use internal database clip IDs; Premiere uses ingest-time association metadata
   with a filename convention as the human-visible fallback.
3. **Offline is a first-class state, not an error.** A missing file yields a
   placeholder in the timeline; the timeline structure stays intact and playable.
4. **Background rendering is idle-time work with a user-visible budget**, and the
   cache-status indicator (Premiere's render bar) is itself part of the UI contract.

### 4.3 What CINE-OMNI should do

CINE-OMNI is in an unusually strong position here because the UCG already provides
exactly what the four NLEs each had to invent privately: stable UUIDs, a dependency
DAG, a dirty set computed on every commit, and reactors.

**Model media identity as UCG nodes, and caches as derived entities.**

```
media.source        EntityId::from_key("media:<content-hash-or-UMID>")
  attrs: paths[], available_range, codec, timecode, channel_map, mhl_hash

media.proxy         EntityId::derived(source, kinds::MEDIA_PROXY)
  edges: DerivedFrom(media.source)
  attrs: recipe_id, scale, codec, path, state

media.rendercache   EntityId::derived(<segment/effect-stack hash>, kinds::RENDER_CACHE)
  edges: DependsOn(media.source), DependsOn(<each effect node>), DependsOn(<timing>)
```

Because cache entities hang off `DependsOn`/`DerivedFrom` edges, **cache
invalidation is not a feature to be written — it is the dirty set that
`ucg-core` already computes.** An edit to a colour node marks every render-cache
node downstream of it dirty in the same transaction that made the edit. That is the
UCG thesis paying for itself, and it is the strongest argument for building the
media layer *inside* the graph rather than beside it.

Specific recommendations:

* **Identity = content hash first, path never.** Adopt a content hash (an MHL-style
  digest, or a fast partial hash for large files) as the primary media identity, and
  read the embedded UMID from MXF where present as a secondary key. `EntityId::from_key`
  already gives us stable derived ids; the CLAUDE.md rule
  `EntityId::from_key("<importer>:<stable key>")` fits exactly.
* **Never let a proxy or a cache change what the graph says.** A grade, a trim, an
  effect binds to `media.source`. Which essence (`source` / `proxy` / `cache`)
  actually feeds the renderer is a *playback policy*, resolved at evaluation time,
  never a graph attribute. Resolve's Smart Relink proves this is the right shape.
* **Background rendering is a reactor-adjacent worker, not a reactor.** Reactors are
  pure functions of the graph run on a rayon pool and must stay fast (§5 of
  ARCHITECTURE.md budgets a full reactor round at <5 ms). Cache generation is
  minutes-long I/O-bound work. The right shape: a reactor marks cache nodes
  `state = Stale` (a cheap, idempotent write), and a separate background scheduler
  drains stale cache nodes at a user-controlled concurrency, committing `state = Ready`
  plus a path when done.
* **Render-bar semantics for free.** With cache nodes in the graph, the red/yellow/green
  bar is a projection of `state` over the timeline — no separate bookkeeping.
* **Smart rendering on export** requires knowing that a segment's source codec matches
  the target and that its effect stack is empty. Both are graph queries.

---

## 5. What professional editorial requires beyond a schema-compatible data model (Question 5)

`cine-timeline` reads and writes OTIO losslessly and implements ripple/roll/slip/
slide/insert/overwrite/lift/extract/split/join. That is a genuinely strong start —
most "OTIO support" in the wild is read-only. But schema compatibility buys the
*serialization*, not the *editorial system*. Each item below names what the data
model must gain and what the runtime must gain.

| Feature | Data model must add | Runtime must add |
|---|---|---|
| **Multicam** | A multicam clip entity containing N angles; a per-angle **sync map** (offset + the *method*: `AudioWaveform` / `Timecode` / `InPoint` / `Marker`, since re-sync behaviour differs); and an **angle selection that varies over time** — effectively a mini-EDL nested inside the clip. Premiere/Resolve support up to 16 angles. | Lockstep multi-decode of N sources for live angle switching; a **flatten** operation that bakes angle selections into a single-angle edit. Flatten is documented as one-way in Resolve — the multicam container cannot be reconstituted. |
| **Nested sequences / compound clips** | A clip whose *source is another timeline*, i.e. recursive/self-referential timeline structure; plus rules for how nested audio busses submix into the parent track. | Recursive playback-graph construction (the sub-timeline becomes an input node of the parent's render DAG), and **cross-scope cache invalidation** — an edit inside the nested sequence must dirty parent render caches. |
| **Time remapping** | A **keyframed time curve** mapping timeline-time → source-time (not a speed scalar — ramps need a curve), plus an explicit per-segment interpolation method (`FrameSample` / `FrameBlend` / `OpticalFlow`), because these produce materially different pixels and must be part of the cache key. | Motion estimation for optical flow — a full compute pass, not resampling; too expensive for real time on most hardware, so it *must* be cached, and the cache key must include the entire upstream effect stack. |
| **Trim modes** | Nothing at rest — the schema stores the *result*. | The *operation semantics*: ripple shifts downstream clips on ripple-enabled tracks; roll trims both sides of a cut without changing sequence duration; slip changes source in/out at fixed timeline position and duration; slide moves neighbours while the clip's own in/out and duration are fixed. Each is a **multi-clip atomic transaction** — precisely the shape `ucg-core`'s transaction model wants, and precisely what a naive per-node writer gets wrong. |
| **Dynamic / JKL trim** | — | A live transport loop: looped playback around the edit point with trim deltas applied in real time. A batch-render system has no concept of this; it needs a realtime audio/video clock, which today CINE-OMNI does not have at all. |
| **Track targeting / source-record patching** | An explicit **channel patch**: which source channels (a clip's audio 1–8) land on which record tracks (A1–A4). A model with only "clip → track" cannot express "channels 3–4 of this 8-channel source went to this mono track." | A transient "current patch" session state governing the *next* edit action, distinct from committed timeline data. |
| **Three-point / four-point editing** | A hard separation of **source in/out** from **timeline in/out**. Three-point sets exactly 3 of 4 marks and computes the fourth; four-point sets all four and must resolve the conflict via **fit-to-fill**, which literally writes a computed speed value `timelineDuration / sourceDuration` — so it depends on the time-remap model above. | The mark-management and edit-resolution logic. |
| **Match frame / replace edit** | A stable, queryable link from any timeline frame back to **source clip + source timecode**. The placed clip must keep its source reference *and its original timecode* addressable, not an opaque media pointer. | Jump-to-source in the viewer; replace-edit preserving either source-frame or timeline-frame alignment at the playhead. |
| **Relink / conform / offline** | Media referenced by **stable logical identity** resolved through an indirection layer, plus an explicit `Offline` state the UI can render. A schema storing literal paths has no conform story: every path change is a break rather than a controlled relink. | The relink matcher (by hash / UMID / timecode+reel / filename), and offline placeholder rendering that keeps the timeline playable. |
| **Handles** | **Both** the logged/usable range **and** the full source extent it was drawn from. Note a subclip made from a duration marker explicitly has *no* handles — handle-lessness is itself a schema property of how the subclip was derived. | Trim clamping against available handle (which `edit.rs` already does against `available_range`). |
| **Subclips** | A first-class catalog entity: a named, saved `(in, out)` against a source clip, with its own name, metadata and markers — reusable across many placements, not an ephemeral trim state. | Bin/catalog management. |
| **Markers with duration** | Range markers, not just points. Resolve derives subclips from duration markers, so the model must support deriving catalog objects from markers. | — |
| **Audio channel mapping** | **Persistent per-source** semantic channel roles ("1–2 = stereo mix, 3 = boom, 4 = lav"), set at import and carried on the media reference — distinct from edit-time patching. | Routing at mix time; `cine-audio`'s `Session`/`Track`/`Stem` model is well placed to consume this. |
| **Source vs record timecode** | **Both** timecode spaces per clip instance: the immutable native source timecode, and the computed timeline position. | A display-mode toggle. This is the same source/record duality that match-frame and 3/4-point editing need — one foundational requirement, not several. |

### 5.1 The honest gap list for `cine-timeline`

Against OTIO's own schema, `cine-timeline` is in good shape. Against *editorial*,
the following are missing and none of them are serialization problems:

* No multicam entity, no angle sync map, no flatten.
* No nested-timeline-as-clip-source (OTIO models this via `Stack` inside a `Clip`'s
  place, but the *runtime* recursion and cache propagation is the work).
* No time-remap curve; OTIO's `LinearTimeWarp` is a single scalar and its
  `TimeEffect` base is otherwise opaque.
* No source/record mark state, no three/four-point edit, no fit-to-fill.
* No subclip catalog entity, no range markers with derivation.
* No channel map on media references.
* No offline state, no relink matcher.
* No transport clock at all — and therefore no JKL, no dynamic trim, no scrubbing.
  This is downstream of "there is no media engine": **JKL is not a timeline feature,
  it is a media-engine feature wearing a timeline costume.**

The last point reframes the whole roadmap. The most valuable editorial work in
CINE-OMNI is not more schema — it is a clock, a decoder, and an audio device.

