# CINE-OMNI Industry Blueprint — 11: Application Shell & GPU/Runtime Architecture

**Status:** research draft
**Date of research:** 2026-09-08
**Scope:** Phase 0 deliverable #10 — the application shell decision, and the
evolution of `cine-gpu` from a headless compute library into an interactive
runtime capable of frame-accurate video playback.
**Author context:** CINE-OMNI is **Apache-2.0**. Every licence, repository,
maintainer and release below was verified against crates.io metadata, the
project's own licence page, or upstream documentation on the date above; URLs
are cited inline. Nothing is stated from memory. Anything that could not be
verified is marked `UNVERIFIED`.

This document is the pair to [`02-editorial-media.md`](02-editorial-media.md),
which settled the **decode** question (FFmpeg LGPL-only, dynamically linked,
behind a Rust adapter; per-platform hardware backends behind a capability
probe). This document settles the **presentation** question: what draws the
application, and how a decoded frame becomes a pixel on a calibrated display.

---

## 0. What actually exists today — measured, not asserted

Before comparing options, the baseline has to be honest, because the gap is
larger than `CHATGPT_CONTEXT.md` §2.3 implies.

Verified by reading `crates/cine-gpu/src/{lib,device,kernels,frame_graph}.rs`
and grepping the whole workspace:

| Claim in existing docs | What the code actually contains |
|---|---|
| "8 WGSL compute kernels" | **True.** `merge`, `cdl`, `lut3d`, `scopes`, `blur`, `keyer`, `dof` compute + `previs` render, all embedded and `naga`-validated. |
| "a PBR previs render pipeline" | **A WGSL source string and a `KernelStage::Render` metadata entry.** `GpuContext::compute_kernel()` explicitly returns `GpuError::Shader { message: "not a compute kernel" }` for it. No `create_render_pipeline` call exists anywhere in the workspace. |
| "backend-agnostic frame graph with culling and aliasing" | **True as a compiler, but it is never executed.** `grep -rn "FrameGraph\|frame_graph"` across `crates/` returns only `lib.rs` re-exports and the module's own unit test. `device.rs` does not reference `frame_graph` at all. `Plan` is produced and dropped. |
| "GPU pipeline" | **There is no `wgpu::Texture` in the repository.** `grep -rn "wgpu::Texture\|create_texture\|wgpu::Surface\|winit"` over `crates/` returns **zero hits**. Every kernel binding is a storage *buffer*; every image is a flat `&[f32]` of RGBA. |
| "verified on Intel UHD via Vulkan" | **True**, for buffer-based compute with synchronous readback. |

Three consequences follow, and they set the whole agenda:

1. **There is no sampling.** No `wgpu::Sampler`, no filtering, no mip levels, no
   format conversion, no sRGB hardware decode. A video viewer needs bilinear
   zoom, mipmapped thumbnails and YUV plane sampling; none of that is a small
   edit to a buffer-based kernel — it is a second, parallel resource path.
2. **`GpuContext::dispatch()` is architecturally unusable for playback.** It
   allocates *every* buffer fresh per call (`create_buffer_init` inside the
   binding loop), builds a new bind group per call, submits, then blocks on
   `device.poll(PollType::wait())` and maps the result. That is one full
   CPU↔GPU round trip with an allocator hit per frame per pass. At 24 fps the
   budget is 41.6 ms for the *entire* frame; this design spends it on
   allocation and stalls.
3. **Adapter selection is wrong for a windowed app.** `GpuContext::new()` passes
   `compatible_surface: None` and `required_features: wgpu::Features::empty()`.
   On a hybrid-GPU laptop that can select an adapter that cannot present to the
   window's surface, and it forecloses `TEXTURE_FORMAT_NV12`, `TEXTURE_FORMAT_P010`,
   `TEXTURE_FORMAT_16BIT_NORM` and `FLOAT32_FILTERABLE`.

The workspace pins `wgpu = "26"`; the current release is **30.0.1**, published
2026-08-22, `MIT OR Apache-2.0`
(<https://crates.io/api/v1/crates/wgpu>). Several capabilities this document
depends on — surface colour spaces, DMA-buf import — landed after 26.

On the UI side, `ui/server/bridge.mjs` is 31 lines and spawns **one
`cine serve` child process per WebSocket connection**. Two browser tabs open two
independent `OmniProject` instances over the same directory and both append to
the same op log. This is not a bug to fix in the bridge; it is a symptom of
putting a process boundary and a network protocol where an in-process call
belongs.

---

# PART A — THE APPLICATION SHELL DECISION

## A.1 The product constraints that actually discriminate

From the directive and `02-TARGET-ARCHITECTURE.md` §5:

* **Not a dashboard.** Dense, discipline-specific, keyboard-driven workspaces.
* **Radically different workspaces** for colorist, animator, editor, compositor
  — sharing one project and one identity graph.
* **Docking, floating windows, multi-monitor, high-DPI.**
* **Local-first, no mandatory browser session.**
* **Three simultaneous real GPU surfaces**: a video viewer, a 3D viewport, and a
  node-graph canvas. This is the crux and it eliminates more options than
  anything else on the list.

A fourth constraint is imposed by the licence, not the product: CINE-OMNI ships
as Apache-2.0 source. Any shell dependency whose only open-source option is
copyleft either changes the licence of the product or forces every downstream
recipient into obligations the Apache-2.0 header does not warn them about.

## A.2 Verified licences, maintainers and releases

Every row verified on 2026-09-08.

| Technology | Licence (verified) | Maintainer | Latest release | Source |
|---|---|---|---|---|
| **egui** | `MIT OR Apache-2.0` | Emil Ernerfeldt (`emilk`); development sponsored by Rerun | **0.36.1**, 2026-08-07 | <https://crates.io/api/v1/crates/egui>, <https://github.com/emilk/egui> |
| **egui_tiles** (docking) | `MIT OR Apache-2.0` | rerun-io | **0.17.1**, 2026-08-18 | <https://crates.io/api/v1/crates/egui_tiles> |
| **egui_dock** (docking) | `MIT` | community (`egui_dock`) | **0.21.1**, 2026-08-06 | <https://crates.io/api/v1/crates/egui_dock> |
| **Slint** | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | **SixtyFPS GmbH** | **1.17.1**, 2026-07-07 | <https://crates.io/api/v1/crates/slint>, <https://github.com/slint-ui/slint>, <https://slint.dev/pricing> |
| **iced** | `MIT` | iced-rs org | **0.14.0**, 2025-12-07 | <https://crates.io/api/v1/crates/iced>, <https://github.com/iced-rs/iced> |
| **Xilem** | `Apache-2.0` | Linebender | no stable release; repo self-describes as "**An experimental Rust architecture for reactive UI**" | <https://github.com/linebender/xilem> |
| **Vello** (Xilem's 2D renderer) | `Apache-2.0 OR MIT` | Linebender | **0.10.0**, 2026-08-14 | <https://crates.io/api/v1/crates/vello> |
| **Makepad** | `MIT OR Apache-2.0` | makepad org | `makepad-widgets` **1.0.0**, 2025-05-13 | <https://crates.io/api/v1/crates/makepad-widgets>, <https://github.com/makepad/makepad> |
| **Tauri** | `Apache-2.0 OR MIT` | tauri-apps | **2.11.5**, 2026-07-01 | <https://crates.io/api/v1/crates/tauri> |
| **cxx-qt** | `MIT OR Apache-2.0` | **KDAB** | **0.10.0**, 2026-08-24 (pre-1.0) | <https://crates.io/api/v1/crates/cxx-qt> |
| **Qt 6** | `LGPLv3` for most modules; **GPLv3-only** for 14 named modules; commercial alternative | The Qt Company | see §A.3 | <https://doc.qt.io/qt-6/licensing.html> |
| **winit** | `Apache-2.0` | rust-windowing | **0.30.13** stable; 0.31.0-beta.3, 2026-09-04 | <https://crates.io/api/v1/crates/winit> |
| **wgpu** | `MIT OR Apache-2.0` | gfx-rs | **30.0.1**, 2026-08-22 | <https://crates.io/api/v1/crates/wgpu> |
| **rfd** (native file dialogs) | `MIT` | PolyMeilex | **0.17.2**, 2026-01-12 | <https://crates.io/api/v1/crates/rfd> |
| **midir** (MIDI I/O) | `MIT` | Boddlnagg | **0.11.0**, 2026-04-18 | <https://crates.io/api/v1/crates/midir> |
| **AccessKit** | `MIT OR Apache-2.0` (`UNVERIFIED` exact SPDX — stated on lib.rs listing, not read from the repo LICENSE file) | AccessKit org | adapters for Windows (UIA), macOS (NSAccessibility), Unix (AT-SPI), Android | <https://accesskit.dev/> |

### The Slint finding is disqualifying, and it is not obvious

Slint's crates.io licence field is
`GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`
(<https://crates.io/api/v1/crates/slint>). Slint's own pricing page describes the
Royalty-free option as: build **proprietary** applications (excluding embedded
systems), "Attribution to Slint is optional", "No obligation to provide source
code upon distribution"; the open-source option is **GPLv3**
(<https://slint.dev/pricing>).

There is **no permissive open-source option**. For CINE-OMNI that means:

* Taking the GPLv3 arm makes the distributed CINE-OMNI binary GPLv3. Apache-2.0
  code can be absorbed into a GPLv3 work, but the *product* would then be
  GPLv3 — which contradicts `01-LICENSING-SPINE.md` and every architectural
  decision derived from it (notably keeping Blender at a process boundary
  precisely to avoid this).
* Taking the Royalty-free arm is a **proprietary licence grant to us**, not an
  open-source licence in the chain. We could ship binaries, but a downstream
  recipient of our Apache-2.0 source could not build or redistribute under the
  terms our own LICENSE file promises them.

Slint is well-engineered and the wrong shape for this product. **Eliminated on
licence, not on merit.**

## A.3 Qt licensing — the honest analysis

The brief asked for this specifically, so it gets its own section.

**What Qt states.** Qt offers LGPLv3, GPLv2 and GPLv3 for open-source use, plus
commercial (<https://www.qt.io/download-open-source>). Qt's LGPL obligations
page lists the requirements as: deliver the complete corresponding source of the
library including modifications (or a written offer); dynamic linking preserves
your application's own licence, **static linking does not**; users must be able
to modify and re-link the library, with "sufficient installation information",
and tivoization is forbidden; you must supply the LGPL text and a prominent
notice; the LGPL freedoms "cannot be in any way negotiated upon or restricted"
(<https://www.qt.io/licensing/open-source-lgpl-obligations>).

**Is LGPLv3 + Apache-2.0 legal?** Yes. Dynamically linking an Apache-2.0
application against an LGPLv3 library is a use the LGPL explicitly contemplates.
The application's own source stays Apache-2.0.

**So what is the actual cost?** It is not the licence text; it is the operational
tail, and it is longer than teams expect:

1. **Every distribution must ship Qt source or a written offer**, and must remain
   able to honour that offer for the required period. That is a build-system,
   hosting and legal-process obligation attached to every release forever.
2. **Relinking must be possible.** Shipping Qt as shared libraries in the app
   bundle is necessary but not sufficient — the "installation information" must
   let a user substitute their own Qt build. This constrains code signing,
   notarisation, hardened runtime and any future appliance/turnkey deployment
   (a colour-grading suite shipped as a locked appliance is exactly the
   tivoization case LGPLv3 forbids).
3. **The GPLv3-only module list bites directly.** Qt's own licensing page names
   these as **not** available under LGPLv3 for open-source users: Qt Canvas
   Painter, Qt CoAP, Qt Graphs, Qt GRPC, Qt HTTP Server, Qt Lottie Animation, Qt
   MQTT, Qt Network Authorization, Qt Qml Compiler, **Qt Quick 3D**, Qt Quick 3D
   Physics, **Qt Quick Timeline**, Qt Virtual Keyboard, Qt Wayland Compositor
   (<https://doc.qt.io/qt-6/licensing.html>). **Qt Quick 3D and Qt Quick Timeline
   are precisely the modules a previs viewport and an animation timeline would
   reach for.** They are unusable in an Apache-2.0 product. Qt Charts and Qt
   Data Visualization are likewise GPL-only for open-source users
   (<https://www.qt.io/development/open-source-lgpl-obligations>).
4. **The commercial alternative is real but priced per seat and quote-based.**
   Published figures found: Qt for Application Development Enterprise at **546
   EUR / ~$618 per year** under the *Qt for Small Business* programme, capped at
   companies below 1M EUR/USD revenue and a maximum of three licences per
   company; and roughly **$4,660 per developer per year** at enterprise tier.
   These are third-party/marketing figures, not a published rate card — Qt does
   not publish list prices. Treat the exact numbers as **`UNVERIFIED`**; treat
   "four figures per developer per year, quote-based" as verified in kind
   (<https://www.qt.io/development/qt-for-small-business>, <https://www.qt.io/pricing>).
   Crucially, Qt states you **cannot mix** open-source and commercial Qt in the
   same product, and code developed under commercial Qt must stay under
   commercial Qt (<https://www.qt.io/download-open-source>) — so this is a
   one-way door taken at the beginning, not an escape hatch taken later.

**Qt's genuine strengths must be stated too, because they are the reason the
industry uses it.** Qt can embed external GPU content properly: `QQuickWindow`
emits `beforeRendering()` before the scene graph draws, and
`QNativeInterface::QSGVulkanTexture` wraps an existing native Vulkan image in a
`QSGTexture` usable by the scene graph, with `QSGRendererInterface` exposing the
underlying device handles
(<https://doc.qt.io/qt-6/qtquick-scenegraph-vulkanunderqml-example.html>,
<https://doc.qt.io/qt-6/qquickwindow.html>). Docking, native text shaping,
platform accessibility, pen/tablet input and high-DPI are all mature. Nothing in
the Rust ecosystem matches that breadth.

**Verdict:** Qt is the *technically* strongest option and the *strategically*
worst fit. It imposes a permanent relink/source-offer obligation on an
Apache-2.0 product, removes exactly the two modules the 3D and animation
workspaces want, and inserts a C++/QML layer between the Rust core and the UI
through a pre-1.0 binding (`cxx-qt` 0.10.0). It is the fallback if the Rust
option fails, not the default.

## A.4 What professional applications actually do — verified

The brief asked for verification of three claims.

**Blender: custom GPU-drawn UI — VERIFIED.** Blender's developer documentation
states the GPU/Viewport module's responsibility is "drawing pixels to the screen
based on common graphics libraries (GL) provided by operating systems. OpenGL,
Metal and Vulkan are different GLs integrated via an abstraction layer called
the gpu-module", and that "to ease development for drawing panels/UI buttons the
GPU module provides an immediate mode, **which is widely used to draw the user
interface**"
(<https://developer.blender.org/docs/features/gpu/>,
<https://developer.blender.org/docs/features/gpu/overview/>;
both returned HTTP 403 to direct fetch and were read via search-indexed content —
the quotes are therefore marked `UNVERIFIED` at the character level but
consistent across multiple independent sources).

**Nuke: Qt — VERIFIED.** Foundry's own Python developer documentation states
Nuke's UI is extended "through python and Qt with PySide2", and provides
`nukescripts.panels.registerWidgetAsPanel()` to register a `QtWidgets.QWidget`
subclass as a dockable Nuke panel
(<https://learn.foundry.com/nuke/developers/140/pythonreference/custom_panels.html>).
A host that exposes `QWidget` docking to plug-ins is a Qt application.

**DaVinci Resolve: Qt — PARTIALLY VERIFIED.** Community/forum evidence indicates
Resolve shipped Qt 5.4.1 and moved to Qt 5.15.2 in Resolve 17.4 (October 2021).
Blackmagic publishes no architecture statement and The Qt Company publishes no
customer story for it. **Marked `UNVERIFIED`.** Note that doc 02 §3.4 reached the
same conclusion from the other direction: Resolve's internals are unpublished.

**Rerun: egui + wgpu — VERIFIED, and this is the most useful data point.** The
Rerun Viewer is a professional, GPU-heavy, multi-panel native application built
on egui; egui's own README cites it as an example of "professional looking
applications"; `egui-wgpu` is co-owned by rerun.io; and egui development is
sponsored by Rerun (<https://github.com/emilk/egui>,
<https://crates.io/crates/egui-wgpu>). `egui_tiles`, the docking/tiling engine,
is a Rerun project (<https://github.com/rerun-io/egui_tiles>).

**What this implies.** The two applications closest to CINE-OMNI's shape split:
the one that owns its whole rendering stack and needs an unbounded number of
custom editors (Blender) drew its own UI; the ones that are primarily
*node/parameter* applications wrapping a proprietary render core (Nuke, and
probably Resolve) took Qt. CINE-OMNI is structurally Blender-shaped — twenty
workspaces, all custom, all GPU — but has neither Blender's 30 years nor its
GPL freedom to reuse. The right answer is the middle path Rerun proved: an
immediate-mode Rust UI that is itself just wgpu draw calls, so custom editors
are cheap and GPU content is native rather than embedded.

## A.5 Scored comparison — the five Rust toolkits

Scoring 0–5. "GPU surface embedding" means: can a video viewer, a 3D viewport
and a node canvas each own real GPU passes on the **same device and queue** as
`cine-gpu`, with no readback?

| Criterion | egui 0.36 | Slint 1.17 | iced 0.14 | Xilem | Makepad 1.0 |
|---|---|---|---|---|---|
| Licence fit with Apache-2.0 | **5** MIT/Apache-2.0 | **0** GPLv3 or proprietary only | **5** MIT | **5** Apache-2.0 | **5** MIT/Apache-2.0 |
| GPU surface embedding (same device/queue) | **5** `egui_wgpu::CallbackTrait` gives `prepare`/`finish_prepare`/`paint` with a `&mut wgpu::CommandEncoder` and access to egui's own render pass | **1** renderers are femtovg/Skia/software/Qt; wgpu not listed as a backend | **4** `iced::widget::shader` renders custom shaders on the wgpu backend | **3** Vello + wgpu, but no stable custom-pass API | **2** custom DSL/shader system, own backends (GL/Metal/DX11), not wgpu |
| Docking / floating / multi-monitor | **4** `egui_tiles` 0.17.1 + `egui_dock` 0.21.1; native multi-viewport since 0.24 | 2 | 3 multi-window via Daemon API (0.13) | 1 | 3 docking tabs in Makepad Studio |
| Ecosystem maturity / production proof | **5** Rerun Viewer | 4 (embedded/automotive) | 3 | **1** self-described experimental | 2 |
| Text rendering quality | 3 (own layout; good, not native) | 4 (Skia) | 3 (cosmic-text) | **4** Parley/Fontique | 3 |
| Accessibility | **4** AccessKit; native APIs on Windows + macOS, enabled by default in eframe | 3 | 2 | 3 (AccessKit) | 1 |
| Retained state for complex editors | 3 (immediate mode — needs explicit state discipline) | 4 | 4 | 4 | 3 |
| Maintenance burden on us | 4 | 3 | 4 | **1** | 2 |
| **Weighted verdict** | **Recommended** | Disqualified (licence) | Viable second | Too early | Too niche |

`egui_wgpu::CallbackTrait` detail, because it is the load-bearing fact:
callbacks run in three phases — `prepare` (before the main egui render pass,
given a `&mut wgpu::CommandEncoder`, so you can run *your own* render/compute
passes into your own textures), `finish_prepare`, then `paint` (inside egui's
render pass, to blit or draw). Per-callback wgpu resources persist in
`CallbackResources` on the `Renderer`
(<https://docs.rs/egui-wgpu>, <https://github.com/emilk/egui/discussions/4583>,
<https://github.com/emilk/egui/blob/main/crates/egui_demo_app/src/apps/custom3d_wgpu.rs>).
That is exactly the shape `cine-gpu`'s frame graph needs to plug into.

## A.6 Scored comparison — the five strategies

| Criterion (weight) | (a) egui + winit + wgpu | (b) Tauri 2 | (c) Qt via cxx-qt | (d) web UI on a native host | (e) custom GPU UI |
|---|---|---|---|---|---|
| **GPU surface embedding (×3)** | **5** same device/queue, callback API, zero copy | **0** see below | 4 `QSGVulkanTexture` + `beforeRendering` | **0** same defect as (b) | **5** by definition |
| **Licence (×3)** | **5** MIT/Apache-2.0 all the way down | 5 Apache-2.0/MIT | 2 LGPLv3 relink+source-offer forever; Qt Quick 3D & Quick Timeline GPL-only | 5 | 5 |
| Docking / floating / multi-monitor (×2) | 4 `egui_tiles`, multi-viewport | 3 multiple webviews per window (v2); OS windows fine | **5** best in class | 3 | 2 we build it |
| Input latency (×2) | **5** direct event→draw, no IPC, no JS | 2 IPC + JS event loop + webview compositor | 4 | 2 | 5 |
| Keyboard fidelity / remapping (×2) | 4 raw key events from winit; we own the keymap | 2 browser key handling swallows/reserves combinations | **5** | 2 | 5 |
| High-DPI | 4 (0.24.1 fixed multi-monitor mixed-scale bugs) | 4 | **5** | 4 | 3 |
| **HDR display output (×2)** | 3 wgpu 30 `SurfaceColorSpace` exists, but egui compositing assumes gamma sRGB — see §A.7 | 1 webview-mediated | 4 | 1 | **5** we control the surface |
| Colour-managed presentation | 3 same caveat | 1 | 4 | 1 | **5** |
| Text rendering quality | 3 | **5** (browser text engines) | 5 | 5 | 1 |
| Native file dialogs | 4 `rfd` 0.17.2 | 4 (Tauri plugin) | **5** | 4 | 4 |
| Accessibility | 4 AccessKit (Win/macOS default; AT-SPI, Android adapters exist) | **5** (DOM/ARIA) | 5 | 5 | 0 |
| Tablet / pen input | 2 winit pen/tablet support is still an open feature request (issue #99) | 2 PointerEvents give pressure/tilt in Chromium; WKWebView `UNVERIFIED` | **5** | 2 | 3 we write it per platform |
| MIDI / control surfaces | **5** `midir` 0.11.0 in-process | 3 must cross IPC; Web MIDI unavailable in WKWebView (`UNVERIFIED`) | 5 | 3 | 5 |
| Crash recovery | 4 one process; op-log replay already exists | 3 two processes, webview can die independently | 4 | 3 | 4 |
| Plugin hosting (OFX/CLAP) | 4 same process, same address space | 2 plug-in UI cannot reach a webview | **5** OFX hosts are conventionally Qt/native | 2 | 4 |
| Enterprise deployment | 4 single static-ish binary | 4 | 2 must ship relinkable Qt + source offer | 3 needs Node or a bundled server | 5 |
| Maintenance burden (×2) | 3 we own layout/widgets; upstream moves fast (0.x) | 3 two languages, two build systems | 2 three languages (Rust/C++/QML), pre-1.0 binding | 4 (today's code) | **0** we build a toolkit instead of a product |
| Ecosystem maturity | 4 Rerun | **5** | 5 | 5 | 1 |
| **Weighted total (max 100)** | **~82** | ~46 | ~74 | ~44 | ~66 |

### Why (b) Tauri and (d) native-host-plus-web both score 0 on the crux

The requirement is three live GPU surfaces composited with the UI. Tauri's own
tracker has the feature request "Render WebView on Top of Native GPU Rendered
Content" (issue #8246, opened 2023-11-17, **closed without implementation**)
(<https://github.com/tauri-apps/tauri/issues/8246>). The community discussion
"Render wgpu frames as webview overlay" (#11944) reports that creating a wgpu
surface on a Tauri window succeeds but nothing appears, with no error — the
webview owns the window's content view
(<https://github.com/tauri-apps/tauri/discussions/11944>). The suggested
workarounds are: render headless and ship the texture to a `<canvas>` (a full
GPU→CPU→GPU round trip per frame — fatal for playback), or stack a second
transparent always-on-top window (fatal for docking, multi-monitor and window
management).

This is decisive and it applies equally to (d): any architecture where the UI is
a webview cannot put `cine-gpu` output under it without a copy. WebGPU *inside*
the webview does not rescue it either — that is a different device, in a
different process, with no path to import `cine-gpu`'s textures.

### Why (e) custom GPU UI is refused

It scores well on every axis we control and 0–1 on accessibility, text and
maintenance. Blender's UI is the product of three decades of full-time work.
Choosing (e) means CINE-OMNI spends Phase 2–4 building a widget toolkit instead
of a media engine. **Refused — but note that (a) is (e) with the first two years
already done**: egui is itself a GPU-drawn immediate-mode UI that emits textured
triangles, so nothing about choosing (a) forecloses replacing individual
widgets with custom GPU-drawn ones later.

## A.7 The strongest argument against the recommendation

egui composites in **gamma space**. egui's own guidance is that renderers should
use gamma-space (non-sRGB-aware) blending with values in 0–1 sRGB gamma space,
and to turn off sRGB-aware framebuffers
(<https://docs.rs/egui/latest/egui/>). Separately, `emilk/egui` issue **#7484,
"wgpu configure surface breaks egui", is open**: switching the surface format
from `Rgba8Unorm` to `Rgba16Float` at runtime renders the whole UI black with no
error, on both Windows 11 and Android 14; the only known workaround is
recreating `egui_winit::State`, `egui_wgpu::Renderer` and the `egui::Context`
together (<https://github.com/emilk/egui/issues/7484>).

For a colour tool this matters more than for anything else egui is used for. A
reference viewer must present through a known transfer function to a calibrated
display, and on HDR output that means an `Rgba16Float` surface in
`ExtendedSrgbLinear` or a PQ surface in `Bt2100Pq` (§B.1). egui's compositing
model is not designed for either.

**This does not sink the recommendation, but it dictates the architecture.** The
mitigation is the one professional tools use anyway: the **reference viewer owns
its own surface**. Concretely —

* UI chrome, panels, scopes and the *editing* viewer live on an SDR
  `Bgra8UnormSrgb`/`Rgba8Unorm` surface where egui is happy.
* The **reference output** is a separate native viewport (egui multi-viewport
  gives us the window; the surface is configured by us, not by `eframe`) with
  its own format and `SurfaceColorSpace`, drawn entirely by `cine-gpu` with **no
  egui content on it at all**. That is what a broadcast/reference monitor feed
  is: a clean feed. Resolve, Nuke and Baselight all do this.
* Consequence to accept honestly: **the in-UI viewer is a preview, and the
  reference viewer is a second window.** If the product later requires
  colour-managed HDR *inside* a docked panel, that is an upstream egui problem
  we would have to fix or work around, and it is the single most likely reason
  this decision would be revisited.

The second-strongest argument against: **tablet/pen pressure**. `winit` issue #99
("Pen/Tablet Input Support") remains an open feature request
(<https://github.com/rust-windowing/winit/issues/99>). A storyboard or paint
workspace needs pressure and tilt. That will require platform-specific code
(Windows Ink / Wintab, macOS `NSEvent` tablet events, Linux libinput tablet-tool)
behind our own trait — real work, but bounded and independent of the shell choice
in a way Qt would have given us for free.

## A.8 THE DECISION

> **Build a native Rust shell: `winit` + `wgpu` + `egui` (with `egui_tiles` for
> docking), in one process with the `Engine`, sharing one `wgpu::Device` and
> `wgpu::Queue` with `cine-gpu`. Retire the Node bridge as the primary path.**

Three strongest reasons:

1. **It is the only option that solves the crux without a copy.** Three GPU
   surfaces on the same device and queue as the compute kernels, via
   `egui_wgpu::CallbackTrait`. Tauri and any webview architecture provably
   cannot (Tauri #8246 closed unimplemented, #11944).
2. **The licence chain is `MIT OR Apache-2.0` end to end** — egui, egui_tiles,
   wgpu, rfd, midir, AccessKit — with `winit` at Apache-2.0. Slint's only
   open-source arm is GPLv3; Qt's LGPLv3 attaches a permanent relink and
   source-offer obligation to an Apache-2.0 product and puts Qt Quick 3D and Qt
   Quick Timeline behind GPLv3.
3. **It deletes the per-tab daemon defect by construction.** One process, one
   `OmniProject`, one op log, in-process `engine.read()` / `engine.transact()`,
   no JSON-RPC serialisation on the interaction path. Latency, correctness and
   crash recovery all improve at once, and the change-notification channel
   (`crossbeam-channel`, already there) becomes a direct subscription.

Strongest argument against: **egui's gamma-space compositing and the open
`Rgba16Float` surface bug (#7484) mean colour-managed HDR presentation cannot
live inside a docked egui panel** — the reference viewer must be its own window
with its own surface. Accepted, and designed for.

## A.9 Staged migration path

The rule: **the shell is never load-bearing for identity.** `ucg-core` must not
gain a dependency on any UI crate, so that a future Qt or custom-GPU shell is a
replaceable leaf.

| Stage | Deliverable | Gate to proceed |
|---|---|---|
| **S0** | New crate `cine-shell` (Apache-2.0, workspace member). `winit` + `wgpu` window, one shared `GpuContext`, egui + `egui_tiles`. Owns the `OmniProject` **in-process**; no JSON-RPC on the interaction path. Ships behind a `cargo run -p cine-shell` alongside, not instead of, the browser UI. | Window opens, adapter chosen with `compatible_surface: Some(&surface)`, egui draws at vsync. |
| **S1** | Port the four existing panels (Script, Inspector, Cascade, Previs) to egui. The previs viewport becomes the **first real `egui_wgpu::CallbackTrait`**, finally compiling `previs.wgsl` into a `wgpu::RenderPipeline`. | Feature parity with `App.tsx`, and `revision`/change-feed live via `crossbeam-channel` instead of WebSocket. |
| **S2** | Command palette, keymap, and **workspaces** — a workspace is a named `egui_tiles::Tree` layout persisted per project. Multi-viewport (tear-off panels to other monitors) via `Context::show_viewport_deferred`. | A colorist layout and an editor layout coexist over one project. |
| **S3** | The **reference viewer** as a dedicated viewport with its own `SurfaceConfiguration` (format + `SurfaceColorSpace`), fed by the frame graph, with no egui content. Native pen/tablet trait with a Windows Ink / libinput / NSEvent implementation. | Frame-accurate playback (§B) and a clean feed at the display's native transfer function. |
| **S4** | The web UI is **repositioned, not deleted**: it becomes the *review and dailies* surface (read-mostly, remote, shareable), served by **one shared daemon with fan-out** rather than one process per tab. This is where option (d) survives and earns its keep. | Two browsers see the same revision; no divergent op logs. |
| **Escape hatch** | Keep all shell code behind `cine-shell`; nothing below it may `use egui`. If §A.7 proves fatal, S0–S3 are replaced without touching `ucg-core`, the domain crates, or `cine-gpu`. | — |

Sequencing note: **S0 and S1 belong in Phase 2 alongside the media engine**,
because the media engine has no meaning without something to show frames in, and
because building the media engine against the JSON-RPC bridge would build a
decoder that can only deliver frames as base64.

---

# PART B — GPU / RUNTIME ARCHITECTURE

## B.1 Presentation: swapchain, present modes, frame pacing, HDR

### Surfaces and present modes (verified against wgpu 30.0.1)

`wgpu::PresentMode` (<https://docs.rs/wgpu/latest/wgpu/enum.PresentMode.html>):

| Mode | Behaviour | Support | Use in CINE-OMNI |
|---|---|---|---|
| `Fifo` | FIFO queue ~3 frames deep, no tearing, "Vsync On", the default | **All platforms** | Default for the UI shell |
| `FifoRelaxed` | FIFO, but tears if a frame misses vblank ("Adaptive Vsync") | AMD on Vulkan only | Playback fallback where present |
| `Immediate` | No queue, swap on present, tears ("Vsync Off") | Most platforms except older DX12 and Wayland | Never for a reference viewer |
| `Mailbox` | Single-frame queue, no tearing ("Fast Vsync") | DX12/Win10, NVIDIA on Vulkan, Wayland on Vulkan | Editing viewer while scrubbing |
| `AutoVsync` / `AutoNoVsync` | Pick the first supported of a list; **supported everywhere** | All | Use these as the outer selection |

**Frame pacing for frame-accurate playback.** `Fifo`'s ~3-frame queue is latency,
not correctness — but a 24 fps timeline on a 60 Hz display is a 2:3 cadence
problem, and a 3-deep queue means the presentation clock and the media clock
must be reconciled explicitly. The design:

* A **media clock** (the timeline's `RationalTime`, which `cine-timeline`
  already models) is authoritative, never the render loop.
* Each presented frame carries the timeline time it represents. The scheduler
  chooses, for the next vblank, the frame whose presentation interval contains
  the predicted present time — i.e. **frame selection, not frame interpolation**.
* Audio is the master clock (it always is in an NLE); video frames are selected
  against it, and a dropped frame is *reported*, never hidden.
* Present-time feedback: wgpu exposes no `VK_GOOGLE_display_timing` equivalent.
  Predict vblank from the display refresh rate reported by winit's monitor
  handle, and measure with `Queue::on_submitted_work_done`. **Precise
  present-time feedback across all three backends is `UNVERIFIED`** — build the
  scheduler so it degrades to "present as soon as ready, count the drops".

### HDR — what wgpu 30 actually supports

wgpu 30 added an explicit `color_space` field on `SurfaceConfiguration` and a
`format_capabilities` field on `SurfaceCapabilities` reporting which colour
spaces each format supports. `wgpu::SurfaceColorSpace`
(<https://docs.rs/wgpu/latest/wgpu/enum.SurfaceColorSpace.html>) has:

| Variant | Meaning | `is_hdr()` | Platform availability (per docs) |
|---|---|---|---|
| `Auto` | Backend chooses; prefers `ExtendedSrgbLinear` for fp16, else `Srgb`. Never wide-gamut or HDR. Reproduces historic wgpu behaviour | — | all |
| `Srgb` | BT.709 primaries, D65, sRGB TRC, SDR | false | all |
| `DisplayP3` | P3 primaries, D65, sRGB TRC, SDR | false | Vulkan, Metal, browser — **not DX12** |
| `ExtendedSrgbLinear` | scRGB: BT.709 primaries, linear, extended range; pair with `Rgba16Float` | **true** | Vulkan, Metal, DX12 — **not browser** |
| `ExtendedSrgb` | extended-range sRGB, non-linear TRC beyond SDR white | **true** | — |
| `ExtendedDisplayP3` | P3 analogue of the above | **true** | Metal and browser only — **not Vulkan, not DX12** |
| `Bt2100Pq` | HDR10: BT.2020 primaries, PQ, absolute luminance to 10,000 nits | **true** | not browser |
| `Bt2100Hlg` | BT.2100 HLG, relative luminance | **true** | not browser, **not DX12** |

**Selection algorithm to implement:** query `SurfaceCapabilities`, then intersect
against our own ordered preference — `Bt2100Pq` → `ExtendedSrgbLinear` →
`ExtendedSrgb` → `DisplayP3` → `Srgb` — and **always keep the SDR fallback**,
because OS HDR may simply be off. Never assume; always report the chosen space
in the UI, because a colourist must know what they are looking at.

**Platform reality check.** On Linux, HDR output requires the Wayland
colour-management protocol, which was merged upstream into `wayland-protocols`
**1.41 on 2025-02-13** and into `wlroots` in March 2025
(<https://www.collabora.com/news-and-blog/news-and-events/12-years-of-incubating-wayland-color-management.html>,
<https://www.phoronix.com/news/wlroots-color-management>,
<https://wayland.app/protocols/color-management-v1>). It is new, and compositor
coverage in 2026 is uneven. **Treat Linux HDR as best-effort; treat macOS
(`ExtendedSrgbLinear` on Metal) and Windows (`Bt2100Pq`/`ExtendedSrgbLinear` on
DX12) as the first HDR targets.**

## B.2 Video decode → GPU

Doc 02 §3 settled decoder selection, hwaccel abstraction and fallback ordering.
This section covers only what happens on the GPU side of that boundary.

### The CPU path (must exist first, on every platform)

FFmpeg hands back an `AVFrame` whose `format` is an `AV_PIX_FMT_*` and whose
`data[]`/`linesize[]` describe **separate planes with row padding**. The three
that matter:

| Source | `AV_PIX_FMT_*` | Planes | Upload as |
|---|---|---|---|
| 8-bit 4:2:0 (H.264/HEVC SDR) | `NV12` | Y (`W×H`), interleaved UV (`W/2 × H/2`) | `R8Unorm` + `Rg8Unorm`, **or** the single `TextureFormat::NV12` (needs `Features::TEXTURE_FORMAT_NV12`) |
| 8-bit planar | `YUV420P` | Y, U, V separate | three `R8Unorm` |
| 10-bit 4:2:0 (HDR, HEVC Main10) | `P010LE` | Y 16-bit (10 in high bits), UV 16-bit interleaved | `R16Unorm` + `Rg16Unorm` (needs `Features::TEXTURE_FORMAT_16BIT_NORM`), **or** `TextureFormat::P010` (needs `Features::TEXTURE_FORMAT_P010`) |
| 12-bit / 4:4:4 / RGB (ProRes 4444, DPX, EXR) | `YUV444P12`, `GBRAP16`, `RGB48` | planar or packed | `R16Unorm` per plane, or `Rgba16Float` directly |

Verified: wgpu exposes `TextureFormat::NV12` (gated by
`Features::TEXTURE_FORMAT_NV12`) and `TextureFormat::P010` (gated by
`Features::TEXTURE_FORMAT_P010`); `R16Unorm`/`Rg16Unorm` are gated by
`Features::TEXTURE_FORMAT_16BIT_NORM`; `Rgba16Float`, `R8Unorm`, `Rg8Unorm` are
core (<https://docs.rs/wgpu/latest/wgpu/enum.TextureFormat.html>).

Three details that are always underestimated:

1. **`linesize` ≠ `width × bytes_per_pixel`.** FFmpeg pads rows for SIMD.
   `Queue::write_texture` takes `bytes_per_row`, so a *correct* upload passes
   `linesize[i]` — but `bytes_per_row` must be a multiple of
   `COPY_BYTES_PER_ROW_ALIGNMENT` (256). When `linesize` is not 256-aligned, the
   choices are a repack on the CPU or a row-by-row copy. **Prefer allocating
   FFmpeg's frame buffers with an aligned stride** so the fast path is the
   normal path.
2. **10-bit in `P010` lives in the *high* bits** of each 16-bit word; the low 6
   bits are zero. Sampling `R16Unorm` gives `value/65535`, so the shader must
   multiply by `65535.0/65472.0` (i.e. `1023` full-scale), not by `1023/65535`.
   Getting this wrong is a ~0.1% luma error — invisible in a demo, fatal in a
   grade.
3. **Video range vs full range.** `AVFrame::color_range` (`MPEG` = 16–235/240
   vs `JPEG` = 0–255) and `colorspace` (BT.709/BT.2020nc/BT.601) must be carried
   into the YUV→RGB matrix. These belong in the uniform block of the conversion
   kernel, sourced from the media engine, and they must be **recorded in the
   UCG on the media node** so a grade is reproducible.

### The zero-copy path

Doc 02 §3.5 already established the per-platform mechanisms and that
`vulkan::Device::texture_from_dmabuf_fd()` exists. Verified again here against
current docs:

```rust
pub unsafe fn texture_from_dmabuf_fd(
    &self, fd: OwnedFd, desc: &TextureDescriptor<'_>,
    drm_modifier: u64, stride: u64, offset: u64,
) -> Result<Texture, DeviceError>
```

"Import a DMA-buf as a texture. **Currently only supports single-plane
DMA-bufs.**" Requires the `VULKAN_EXTERNAL_MEMORY_DMA_BUF` feature, which implies
`VK_EXT_external_memory_dma_buf` and `VK_EXT_image_drm_format_modifier`; Vulkan
takes ownership of the fd on success. `texture_from_raw()` also exists for
wrapping an existing `vk::Image`, and
`vulkan::Adapter::open_with_callback` allows editing the `pNext` chains and
extension lists before device creation — which is how we would enable video
extensions wgpu does not expose
(<https://docs.rs/wgpu-hal/latest/wgpu_hal/vulkan/struct.Device.html>).

**The single-plane limitation is the operative constraint.** VAAPI/DRM-PRIME
hands out NV12 as a **two-plane** dmabuf (or one fd with two plane offsets).
Options, in order of preference:
(i) import each plane as its own single-plane texture using the per-plane
    `offset`/`stride` — plausible and the intended reading of the API, but
    **`UNVERIFIED` in practice**; prove it on the Intel UHD machine before
    designing around it;
(ii) contribute multi-plane support upstream;
(iii) fall back to `av_hwframe_transfer_data()` + the CPU path.

Metal and D3D12 external-memory parity in wgpu remains **`UNVERIFIED`**, as doc
02 already flagged. Plan for the CPU path to be the *correct* path and zero-copy
to be the optimisation.

### Vulkan Video — verified state

Khronos has finalised decode **and** encode for H.264, H.265 and AV1 (announced
with Vulkan 1.3.302), with VP9 decode added in Vulkan 1.4.317
(<https://www.khronos.org/blog/khronos-releases-vulkan-video-av1-decode-extension-vulkan-sdk-now-supports-h.264-h.265-encode>,
<https://blogs.igalia.com/vjaquez/vulkan-video-status/>). Driver coverage in
Mesa is improving but incomplete — NVK's Vulkan Video landed for Mesa 26.3 with
**H.264 decode only**, H.265 wedging the GSP
(<https://www.phoronix.com/news/NVK-Vulkan-Video-Mesa-26.3>).

**wgpu does not support Vulkan Video.** Issue #2330, "Support video encoding and
decoding APIs", is **closed as not planned**
(<https://github.com/gfx-rs/wgpu/issues/2330>). Any Vulkan Video use must go
through `wgpu-hal`'s raw Vulkan escape hatches
(`Adapter::open_with_callback`, `Device::texture_from_raw`) or through FFmpeg's
own Vulkan hwaccel with a dmabuf/`VkImage` handoff.

**Assessment unchanged from doc 02:** Vulkan Video is the right long-term
convergence point and the wrong Phase-2 foundation.

## B.3 Texture formats and colour — the working-space decision

| Stage | Format | Rationale |
|---|---|---|
| Decoded YUV planes | `R8Unorm` / `Rg8Unorm` / `R16Unorm` / `Rg16Unorm`, or `NV12` / `P010` where the feature is present | matches `AV_PIX_FMT_*` layout with no repack |
| **Working space** | **`Rgba16Float`, scene-linear** | 10 bits of mantissa + exponent covers the full ACEScg range; half the bandwidth of `Rgba32Float`; **filterable without a feature flag** (unlike `Rgba32Float`, which needs `FLOAT32_FILTERABLE`) |
| Intermediate comp buffers | `Rgba16Float` | as above; the current frame graph hardcodes 16 bytes/px (f32×4) — halving this halves VRAM for every transient |
| Scopes / histograms | `Buffer` with `atomic<u32>` | unchanged; `scopes.wgsl` already does this |
| SDR present | `Bgra8UnormSrgb` (or surface-preferred), `SurfaceColorSpace::Srgb` | hardware sRGB encode |
| HDR present | `Rgba16Float` + `ExtendedSrgbLinear`, or a PQ-encoded surface + `Bt2100Pq` | §B.1 |

**Note a real conflict.** `frame_graph.rs::ResourceDesc::byte_size()` returns
`width * height * 16` unconditionally, and `cpu.rs`/`device.rs` speak in `&[f32]`
RGBA. Moving to `Rgba16Float` textures means `ResourceDesc` must gain a real
format field and the CPU mirrors must keep computing in `f32` while comparing
against half-float GPU results within a **widened** tolerance — the existing
1e-5 pixel-parity threshold will not hold against `f16` (`~1e-3` relative is the
realistic bound). **Do not silently widen the tolerance; add a second tier of
parity test with a documented, justified threshold**, or keep an `Rgba32Float`
debug path specifically for parity testing.

## B.4 CPU/GPU synchronisation

What exists: one synchronous `dispatch()` that ends in
`device.poll(PollType::wait())` and a buffer map. What is needed:

1. **Frames in flight.** A ring of N (2 or 3) per-frame resource sets: command
   encoder, uniform buffers, bind groups, staging buffers. Frame *k* records
   while frame *k−1* executes. Advance the ring only when
   `Queue::on_submitted_work_done` for frame *k−N* has fired.
2. **Upload ring.** Use `wgpu::util::StagingBelt`: `write_buffer()`/`allocate()`
   → `finish()` → submit → `recall()` (or `finish_and_recall_on_submit()`).
   Documented guidance: `chunk_size` should be "larger than the largest single
   `write_buffer()` operation" and "1-4 times less than the total amount of data
   uploaded per submission"; `recall()` must run *after* submission or memory use
   grows without bound
   (<https://docs.rs/wgpu/latest/wgpu/util/struct.StagingBelt.html>).
   For a 4K 10-bit frame (~25 MB with chroma), size the belt in the tens of MB.
3. **Never map-and-wait on the interaction path.** Readback (scopes values, pixel
   probes, thumbnail generation) goes through `map_async` + a callback on a
   worker, with the result delivered by channel. The current
   `rx.recv()`-after-`poll(wait)` pattern is fine for tests and forbidden in the
   shell.
4. **Decode runs on its own thread** and hands `Arc<Frame>` over a bounded
   channel. Bounded, because an unbounded decode queue is how a media
   application OOMs on a 6K source.
5. **No cross-queue semaphores are available**, because there is only one queue
   (§B.6). Where an externally signalled semaphore must be waited on (hardware
   decode), `wgpu-hal`'s `vulkan::Queue::add_wait_semaphore` /
   `remove_wait_semaphore` is the documented mechanism (doc 02 §3.5).

## B.5 GPU memory pressure

wgpu exposes `Device::generate_allocator_report()` returning an
`AllocatorReport` "if the underlying backend supports it", `None` otherwise
(<https://wgpu.rs/doc/wgpu_types/index.html>). That is *observability*, not
*management*: **wgpu has no residency or eviction API**. Everything below is our
responsibility.

1. **A budget, declared not discovered.** At startup, probe adapter limits and
   set a working budget (a sane default: 70% of reported VRAM on discrete, a
   hard cap on integrated/UMA where "VRAM" is system memory). Surface it in the
   UI. A colourist with a 6K timeline needs to see why playback dropped to
   proxy.
2. **A frame cache with explicit eviction.** LRU over decoded frames keyed by
   `(media_id, frame_number, essence_tier)` where tier is
   source/proxy/cache — the vocabulary doc 02 §4 already defined. Evict by
   distance from the playhead, not by pure recency: a scrub backwards should not
   evict everything ahead.
3. **Tiled processing for images larger than VRAM.** The frame graph already
   computes resource lifetimes and aliases slots; extend it with a **tile
   dimension**: compile once, execute over an `M×N` grid of tiles with a halo
   sized by the maximum kernel radius in the chain (`blur` radius, `dof`
   `max_radius`). The existing kernels take `width`/`height` uniforms and index
   with `common.wgsl`'s 2D helper, so they are *nearly* tileable already — what
   is missing is a halo/border policy, and it must be explicit because a
   separable blur across a tile boundary without a halo produces a visible seam.
4. **Mip/proxy pyramids.** Two different things, both needed. *Mips* are a GPU
   concern (thumbnail strips, zoomed-out node graph, filmstrip scrubbing) —
   generate on upload. *Proxies* are a media-engine concern already specified in
   doc 02 §4. The graph stores the recipe; neither is ever part of identity.
5. **Streaming.** Decode-ahead depth is a function of budget, not a constant.
   Compute it: `depth = min(budget_bytes / frame_bytes, latency_target × fps)`.

## B.6 Render-graph evolution — what the current one lacks

`frame_graph.rs` is a good compiler with the wrong vocabulary. Ordered by how
much they block a viewer:

| Gap | Detail | Priority |
|---|---|---|
| **It is never executed** | No backend walks `Plan.order` and issues work. This is the single largest gap. | **P0** |
| **Compute-only by construction** | `Pass { kernel: KernelId, dispatch: [u32;3] }` cannot express a render pass: no attachments, no vertex buffers, no depth state, no viewport. `previs.wgsl` cannot be run. | **P0** |
| **No textures, only buffers** | `ResourceKind::{Image,Depth,Buffer}` with a hardcoded 16 B/px. Needs a real `TextureFormat`, usage flags, mip count, sample count, and array layers. | **P0** |
| **No imported external resources** | `imported` is a bool meaning "don't alias me". It needs to mean "here is an actual `wgpu::TextureView` I did not allocate" — the swapchain view, a decoded frame, a dmabuf import. | **P0** |
| **No cross-frame reuse** | Everything is transient within one compile. A viewer wants persistent history buffers (temporal denoise, motion vectors, the previous grade for A/B wipe) and a **pipeline/bind-group cache keyed by pass identity** so steady-state playback allocates nothing. | **P1** |
| **Declaration-order dependency rule** | `compile()` errors if a pass reads a resource written *later in declaration order*. That is a real constraint on a user-authored node graph, where node order is the user's business. Producers should be resolved by name, with cycles rejected — which the Kahn pass already detects. | **P1** |
| **No async compute / multi-queue** | wgpu gives each `Device` exactly **one** `Queue`; multi-queue (issue #1066) and transfer queues (issue #5576) are open, unscheduled requests (<https://github.com/gfx-rs/wgpu/issues/1066>, <https://github.com/gfx-rs/wgpu/issues/5576>). **This is blocked upstream, not by us.** Design the graph to *express* queue affinity so it is ready, but do not plan work that depends on it. | **P3 (blocked)** |
| **No multi-GPU** | wgpu has no cross-device sharing. A second GPU is reachable only as a second `Device` with CPU-mediated transfer. For background renders (a whole second pipeline on the second GPU) that is acceptable; for split-frame interactive rendering it is not. | **P3** |
| **No CPU fallback routing** | `cpu.rs` mirrors exist but the graph cannot schedule a pass on them. Valuable: a machine with no adapter, or a kernel a driver miscompiles, should still produce pixels. | **P2** |
| **No timing/profiling** | `timestamp_writes: None` everywhere. `Features::TIMESTAMP_QUERY` would give per-pass GPU time — indispensable for "why did playback drop". | **P1** |

## B.7 The viewer pipeline

The design, reusing every existing kernel rather than replacing it:

```
 media engine (thread)                cine-gpu (render thread)
 ─────────────────────                ─────────────────────────
 demux → decode                       ┌─────────────────────────────────────┐
   │                                  │ FrameGraph, compiled once per        │
   ├─ hw surface (dmabuf/IOSurface)   │ viewer configuration, executed per    │
   │        └── import (zero copy) ──▶│ frame with new imported resources     │
   │                                  │                                       │
   └─ CpuFrame (NV12/P010/…)          │  [import]  y_plane, uv_plane          │
            └── StagingBelt ─────────▶│      │                                │
                                      │      ▼                                │
                                      │  yuv_to_linear   ← NEW compute kernel │
                                      │      │   (matrix + range + transfer   │
                                      │      │    from AVFrame metadata)      │
                                      │      ▼  Rgba16Float scene-linear      │
                                      │  ┌───────────────┐                    │
                                      │  │ input xform   │ ← lut3d.wgsl       │
                                      │  ├───────────────┤    (EXISTING)      │
                                      │  │ grade         │ ← cdl.wgsl         │
                                      │  ├───────────────┤    (EXISTING)      │
                                      │  │ comp overlay  │ ← merge.wgsl       │
                                      │  │               │   keyer/blur/dof   │
                                      │  └───────────────┘    (EXISTING)      │
                                      │      │                                │
                                      │      ├──▶ scopes.wgsl (EXISTING) ──▶  │
                                      │      │      histogram/waveform/vector │
                                      │      │      → buffer → map_async →    │
                                      │      │        egui overlay panel      │
                                      │      ▼                                │
                                      │  display xform ← lut3d.wgsl (EXISTING)│
                                      │      │   + ODT / PQ encode            │
                                      │      ▼                                │
                                      │  [import] swapchain view              │
                                      │  blit / present                       │
                                      └─────────────────────────────────────┘
```

Points worth making explicit:

* **Exactly one new kernel is required for a first viewer: `yuv_to_linear`.**
  Everything after it already exists and is already CPU-parity-tested. That is
  the whole argument for evolving `cine-gpu` rather than starting over.
* **`scopes.wgsl` already takes an `hdr_pq` uniform flag** — the scopes were
  designed for this and have never been driven by real footage.
* **Scopes read from the working-space image, not the display image.** A
  waveform of the post-ODT signal is wrong. This is a correctness decision, not
  a performance one, and it is easy to get backwards.
* **The graph is compiled once per viewer configuration and executed per frame.**
  Compilation is where culling and aliasing pay off; re-compiling per frame at
  24 fps would waste the benefit. This requires the P1 "persistent
  pipeline/bind-group cache".
* **The display transform is a `lut3d` pass, not a hardcoded ODT.** That keeps
  the door open for OpenColorIO (still `PENDING` per
  `02-TARGET-ARCHITECTURE.md` §4) without redesigning the viewer.

---

## Recommendations

### R1 — The shell decision (decided)

**Adopt (a): a native Rust shell — `winit` + `wgpu` + `egui` + `egui_tiles` —
in one process with the `Engine`, sharing one device and queue with `cine-gpu`.**

* Licence chain verified `MIT OR Apache-2.0` throughout (`winit` Apache-2.0).
* **Rejected: Slint** — no permissive open-source arm
  (`GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`).
* **Rejected: Tauri, and any webview-hosted UI** — cannot composite native GPU
  content under a webview in one window (tauri#8246 closed unimplemented).
* **Rejected for now, retained as fallback: Qt** — LGPLv3 relink/source-offer
  obligations attach permanently to an Apache-2.0 product, and Qt Quick 3D and
  Qt Quick Timeline are GPLv3-only for open-source users.
* **Rejected: custom GPU UI** — it is the right architecture and the wrong
  budget; egui already is one, with accessibility and text solved.
* Migration: **S0–S4 in §A.9**, S0 and S1 inside Phase 2.
* Accepted cost: the colour-managed / HDR reference viewer is a **separate
  viewport with its own surface**, because egui composites in gamma space and
  `emilk/egui#7484` (Rgba16Float surface) is open.

### R2 — GPU runtime work, prioritised

**Phase 2 (must exist for a viewer at all):**

| # | Work | Why it is P0 |
|---|---|---|
| 1 | **Surface + swapchain**: adapter selection with `compatible_surface: Some(&surface)`, `AutoVsync` selection, resize handling, `SurfaceColorSpace` negotiation with an SDR fallback | nothing is visible without it, and the current `compatible_surface: None` can pick an adapter that cannot present |
| 2 | **A texture path**: `wgpu::Texture` + `Sampler` + `TextureView` resources, real `TextureFormat` on `ResourceDesc`, usage flags | there is not one texture in the codebase today |
| 3 | **A frame-graph executor**: walk `Plan.order`, allocate/alias physical slots, bind, dispatch, and support **render** passes so `previs.wgsl` finally compiles into a pipeline | the graph is a compiler with no backend |
| 4 | **Imported external resources**: swapchain view, decoded-frame textures | the boundary between the media engine and the renderer |
| 5 | **`yuv_to_linear` kernel** with colorspace/range/transfer uniforms taken from `AVFrame` metadata and recorded on the UCG media node | the only genuinely new pixel kernel a viewer needs |
| 6 | **Upload ring + frames-in-flight**: `StagingBelt`, N-deep per-frame resources, `on_submitted_work_done`, no `poll(wait)` on the interaction path | the current synchronous dispatch cannot hit 41.6 ms |
| 7 | **Enable required features**: `TEXTURE_FORMAT_16BIT_NORM`, and `TEXTURE_FORMAT_NV12` / `TEXTURE_FORMAT_P010` / `TIMESTAMP_QUERY` where present, with graceful degradation | 10-bit is not optional for this product |
| 8 | **Media-clock frame scheduler** with audio as master and explicit drop reporting | "frame-accurate" is a claim that must be measurable |

**Phase 3 (quality and scale):**

9. Persistent pipeline/bind-group cache and cross-frame resources (P1).
10. `TIMESTAMP_QUERY` per-pass profiling surfaced in the UI (P1).
11. Producer-by-name dependency resolution, removing the declaration-order rule (P1).
12. Zero-copy dmabuf import on Linux behind the doc-02 capability probe — **first
    proving whether per-plane import of an NV12 dmabuf works** (P1).
13. HDR presentation: `Bt2100Pq` / `ExtendedSrgbLinear` on the reference viewport,
    Windows and macOS first (P2).
14. Tiled execution with explicit halos for images larger than VRAM (P2).
15. Budget, LRU frame cache with playhead-aware eviction, mip pyramids (P2).
16. CPU fallback routing through the existing `cpu.rs` mirrors (P2).

**Blocked upstream — do not plan around:**

17. Multi-queue / async compute (wgpu#1066, wgpu#5576 open, unscheduled).
18. Vulkan Video through wgpu (wgpu#2330 **closed as not planned**); reachable
    only via `wgpu-hal` raw Vulkan or FFmpeg's Vulkan hwaccel.
19. Multi-GPU rendering (no cross-device sharing in wgpu).

### R3 — Consequences for other blueprint documents

* `02-TARGET-ARCHITECTURE.md` §4: the **Application shell** row can move from
  PENDING to decided, citing this document. The layer diagram's SHELL row
  becomes `cine-shell (winit + wgpu + egui)`.
* `02-editorial-media.md` §3.6: `FrameSink::accept_gpu`/`accept_cpu` is the right
  trait; add that the CPU tier is the *correct* path and the GPU tier the
  optimisation, and that `texture_from_dmabuf_fd` is single-plane today.
* `04-RISK-REGISTER.md` should gain: (i) egui gamma-space compositing blocks
  in-panel HDR (`emilk/egui#7484`); (ii) `winit` has no pen/tablet input
  (rust-windowing/winit#99) so a paint workspace needs per-platform code;
  (iii) wgpu is single-queue, so async compute is unavailable indefinitely.
* A **`cine-shell`** crate and a **`cine-media`** crate should be added to the
  workspace in Phase 2. Per `CLAUDE.md`, reactors stay in `cine-pipeline`;
  `cine-shell` must depend on the domain crates and never the reverse, and
  `ucg-core` must never depend on any UI crate.

---

## Open questions and `UNVERIFIED` items

| Item | Status |
|---|---|
| Per-plane import of a two-plane NV12 dmabuf via `texture_from_dmabuf_fd` | **UNVERIFIED** — must be proven on the Intel UHD machine before it is designed around |
| wgpu external-memory parity on Metal and D3D12 | **UNVERIFIED** (also flagged in doc 02) |
| Precise present-time feedback across Vulkan/Metal/DX12 through wgpu | **UNVERIFIED** — no `VK_GOOGLE_display_timing` equivalent found in the wgpu API |
| DaVinci Resolve's use of Qt | **UNVERIFIED** — forum evidence only; no vendor statement |
| Blender GPU-module quotes | Source pages returned HTTP 403 to direct fetch; content read via search index and corroborated across sources. Character-level quotes **UNVERIFIED** |
| Qt commercial list prices | **UNVERIFIED** — Qt publishes no rate card; third-party figures cited in kind only |
| AccessKit exact SPDX identifier | **UNVERIFIED** — not read from the repository LICENSE file |
| Web MIDI availability inside WKWebView (macOS Tauri) | **UNVERIFIED** |
| Whether `egui#7484` is a surface-format bug or an architectural limit of gamma-space compositing | **UNVERIFIED** — the issue is open with no maintainer response; the mitigation in §A.7 does not depend on the answer |
