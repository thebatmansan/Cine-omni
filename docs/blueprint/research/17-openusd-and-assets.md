# 17 — OpenUSD, Interchange and the Persistent Asset System

**CINE-OMNI industry blueprint · research dossier**
Prepared: 2026-09-08 · Directive §34 (OpenUSD adoption) and the asset-system directive

---

## 0. Scope and rules of evidence

This dossier covers **OpenUSD**, the interchange formats adjacent to it
(Alembic, glTF, MaterialX, USDZ), and the design of a **persistent asset
system** for CINE-OMNI expressed in the existing UCG vocabulary.

It deliberately does **not** re-analyse Blender. `docs/blueprint/research/01-3d-dcc.md`
already covers Blender's data-block (ID) system, its depsgraph, its library
linking and override mechanism, and its asset browser/catalogue, and classifies
Blender as **TYPE D (BRIDGE)** with a TYPE B carve-out for `intern/cycles`
(Apache-2.0) and `intern/libmv` (MIT). Where Blender is relevant here it is
**referenced**, not re-derived — principally §01 §1.2.4 (library overrides as a
name-anchored list-edit system) and §01 §1.2.5 (the UUID-keyed asset catalogue),
which are the two Blender findings that bear directly on the asset design below.

**Rules of evidence.** Every licence is verified against the project's own
`LICENSE` file, read directly. Every Rust crate is verified against the
crates.io API. Nothing is asserted from memory. Claims that could not be
established from a primary source are marked `UNVERIFIED` inline and collected
in §11.

**This dossier is unusually empirical, and that mattered.** OpenUSD was cloned,
its licence read, its schemas queried; the real OpenUSD 26.8 runtime and the
pure-Rust `openusd` crate were both installed and executed on this machine; and
`cine-previs`'s actual output was fed through both. Results are labelled
**MEASURED** with the commands given.

> **Integrity note.** An earlier draft of this document asserted two conformance
> defects in `cine-previs` that **empirical testing then disproved** (see §1.4).
> They have been retracted. This is recorded rather than quietly deleted,
> because it is the clearest possible argument for the project's own rule that
> capability claims must be backed by something that runs.

**Integration taxonomy** is the one in `docs/blueprint/01-LICENSING-SPINE.md`,
restated in §01 §0.2: **A NATIVE / B LIBRARY / C ENGINE / D BRIDGE /
E IMPORT-EXPORT / F FORK / G INSPIRED-REIMPLEMENTATION / H REJECT**.

---

## 1. Findings summary

### 1.1 The headline

Five findings, each of which changes a decision:

1. **`cine-previs` does not produce OpenUSD.** It produces a hand-rolled
   USDA-*shaped* text subset with no composition, no materials, no time samples
   and no binary format, round-tripped only against itself. The crate's internal
   docs already say so; the outward-facing claims do not. §1.2, §1.3.

2. **But what it emits is far better than expected — it opens *clean* in real
   OpenUSD 26.8.** MEASURED. Every prim type resolves to a real schema, every
   attribute matches its schema-declared type exactly, and the `EntityId`
   round-trip carrier already exists. There is **one** real defect, and it is a
   units bug worth exactly 100×. §1.4.

3. **OpenUSD is not Apache-2.0 and never became Apache-2.0.** Since release
   24.08 (2024-07-25) it is the **Tomorrow Open Source Technology License 1.0** —
   a bespoke, non-OSI, non-SPDX licence that is Apache-2.0 with a *narrowed* §6.
   Verified verbatim from `LICENSE.txt`. The widely repeated "it moved to plain
   Apache-2.0 when AOUSD formed" is **false**. §2.1.

4. **USD is now a real, ratified, normative specification.** The **AOUSD Core
   Specification 1.0 is officially ratified** and normatively covers the USDA
   grammar, the **USDC binary format**, USDZ packaging, the composition
   algorithm and value resolution. USD is no longer defined solely by Pixar's
   implementation. §2.5. *This invalidates the standard argument against
   independent USD implementations.*

5. **There is a mature, actively developed, MIT-licensed, pure-Rust
   implementation of OpenUSD that builds here with plain `cargo build` and
   correctly reads CINE-OMNI's output.** The `openusd` crate (v0.7.0, published
   **2026-09-05 — three days before this research**) implements `.usda`,
   `.usdc` *and* `.usdz`, a full LIVRPS composition engine, and passes the AOUSD
   Core Specification 1.0 compliance suites. **MEASURED: 79-crate dependency
   tree, zero `-sys` crates, zero C or C++.** §4.

**Finding 5 inverts the recommendation.** The expected answer was "USD behind a
process boundary, eventually". The correct answer is **TYPE B — a normal cargo
dependency, now.** §5.

### 1.2 What `cine-previs` actually produces — the blunt assessment

**It is not OpenUSD. It is a hand-rolled text emitter producing a small
USDA-shaped subset, plus a parser that reads back only what that emitter
writes.**

The crate says so itself. `crates/cine-previs/src/stage.rs` opens:

> *"A USD-style scene description … It is **not** a general USD implementation —
> composition arcs, variants, time samples and schemas beyond the handful used
> here are out of scope."*

and `stage.rs:854` documents `Stage::from_usda` as:

> *"Parse USDA text produced by `Stage::to_usda` (and simple hand-written files
> using the same subset …)"*

`CHATGPT_CONTEXT.md:83` is likewise already honest:

> *"The USD layer is a **USD-style stage with USDA text serialization**. It is
> not linked against Pixar OpenUSD."*

**So the internal documentation is accurate. The problem is scope, not honesty.**
What is missing is not a caveat — it is composition:

| USD feature | In `cine-previs`? |
|---|---|
| Sublayers, references, payloads, inherits, specializes | **No** |
| Variant sets / variants | **No** |
| `over` (sparse override) prims, `class` prims | **No** |
| LIVRPS value resolution — *the composition engine* | **No** |
| Time samples; `timeCodesPerSecond`/`startTimeCode` | **No** — the stage is one static snapshot |
| `UsdShade` materials, shaders, `material:binding` | **No** — there are no materials at all |
| `UsdSkel` skeletons, skinning, blend shapes | **No** |
| `UsdGeom` topology (`points`, `faceVertexIndices`, primvars) | **No** — `Mesh` prims carry no geometry |
| Asset paths (`@…@`), `Ar` asset resolution | **No** |
| `.usdc` binary, `.usdz` packaging | **No** |
| Instancing, prototypes | **No** |
| Schema registry / validation | **No** |

**The verdict.** `cine-previs::stage` is a competent, well-tested ~1,500-line
*private scene format that uses USDA's punctuation*. It is a reasonable thing to
have built. It is **not** OpenUSD, it lacks the one thing that makes USD
valuable, and its round-trip guarantee is with itself. Calling it USD support
would be a false capability claim of exactly the kind `CLAUDE.md` already
forbids for `cine-media` (*"never make a capability claim the build cannot
back"*). **The same rule applies here.**

### 1.3 What it must stop claiming — concrete wording

The honest position is **not** "we don't do USD":

> **CINE-OMNI's previs stage is a native scene description that serializes to a
> USDA-compatible text subset. It is not an OpenUSD implementation: no
> composition, no materials, no time samples, no `.usdc`/`.usdz`, and it is not
> linked against OpenUSD. Layers it writes have been verified to open in
> OpenUSD 26.8; interoperation is one-way and lossy.**

| File | From | To |
|---|---|---|
| `crates/cine-previs/Cargo.toml` `description` | *"…USD-style stage…"* | *"…native previs stage with USDA-subset serialization (not an OpenUSD implementation)…"* — **this one matters most: it is published to crates.io** |
| `README.md:52` | *"USD-style stage with USDA export"* | *"native previs stage serialized to a USDA-compatible text subset (not an OpenUSD implementation)"* |
| `cine-cli` `export … usda` | (no caveat) | Emit to stderr: `note: USDA subset — no composition, materials or time samples` |
| `stage.rs` module doc | already honest | Add the units defect as `// TODO(usd-conformance)` |

### 1.4 What real OpenUSD says about that output — MEASURED

The decisive experiment. A project was created, a screenplay imported, and
`cine export … usda` run; the resulting layer was opened in **real OpenUSD
26.8** (`usd-core` from PyPI, §3.5).

```
cargo run -p cine-cli -- new  $SP/proj --name Harbor
cargo run -p cine-cli -- import $SP/proj $SP/test.fountain
cargo run -p cine-cli -- export $SP/proj usda --scene 0 --out $SP/scene1.usda
.venv/bin/python -c "from pxr import Usd; Usd.Stage.Open('scene1.usda')"
```

**Result: `OPEN: SUCCESS`. No errors, no warnings.** Full traversal:

| Prim | Type written | Schema resolves? |
|---|---|---|
| `/World`, `/World/Characters`, `/World/Cameras`, `/World/Lights` | `Xform` | ✅ YES |
| `/World/Set` | `Scope` | ✅ YES |
| `/World/Set/Floor` | `Plane` | ✅ **YES** |
| `/World/Cameras/A` | `Camera` | ✅ YES |

**Two defects claimed in the earlier draft of this document were WRONG and are
retracted:**

* ❌ **RETRACTED: "`def Plane` is not a USD type."** It is. `UsdGeomPlane` is a
  real concrete schema. MEASURED: `Usd.SchemaRegistry().IsConcrete('Plane')` →
  `True`, and `UsdGeom.Plane` binds successfully. Moreover `cine-previs` writes
  `width`, `length`, `axis` and `extent` with **exactly** the schema-declared
  types (`double`, `double`, `token`, `float3[]`).
* ❌ **RETRACTED: "the camera FOV is wrong."** It is right. MEASURED: real USD
  computes a horizontal FOV of **43.589°** against CINE-OMNI's intended ~43.3°.
  Field of view depends only on the focalLength : aperture *ratio*, which is
  unit-invariant.

Attribute-type conformance is in fact **perfect**:

| Camera attribute | USD schema type | `cine-previs` writes |
|---|---|---|
| `focalLength`, `horizontalAperture`, `verticalAperture`, `fStop`, `focusDistance` | `float` | `float` ✅ |
| `clippingRange` | `float2` | `float2` ✅ |
| `projection` | `token` | `token` ✅ |

And the identity carrier **already exists and survives**: MEASURED,
`cine:entity = 47647104-f782-5d62-a0af-e1affa916aec` reads back intact, as do
`cine:primType` and `kind = "component"` (via `Usd.ModelAPI(prim).GetKind()`).
§01 §1.4 Option 4 called for a `ucg_id` carrier in USD custom metadata as a
prerequisite for any round trip; **`cine-previs` has already done it.**

#### The one real defect: camera lengths are 100× off

Verified against the primary source, `pxr/usd/usdGeom/schema.usda:2368`:

```
float focalLength = 50.0 (
    doc = """Perspective focal length in tenths of a scene unit; see 
             \ref UsdGeom_CameraUnits .""")
float2 clippingRange = (1, 1000000) (
    doc = """Near and far clipping distances in scene units; …""")
```

`cine-previs` writes `metersPerUnit = 1` (metres) but writes `focalLength = 35`
and `horizontalAperture = 27.99` in **millimetres**, while writing
`focusDistance = 3` and `clippingRange = (0.01, 10000)` in **metres**. Two unit
systems in one prim. `stage.rs:11` shows the authors knew:

> *"camera apertures in millimetres … which matches what most DCC exporters do
> despite USD's 'tenths of scene unit' wording"*

The defence is only valid when the exporter also writes `metersPerUnit = 0.01`.
CINE-OMNI writes metres. **MEASURED — how real OpenUSD reads the file:**

```
focalLength   35.0  -> 3.5 scene units  = 3.5 METRES
hAperture     27.99 -> 2.799 scene units = 2.799 METRES
focusDistance 3.0 metres
=> USD believes this is a 3.5 m lens with a 2.8 m sensor focused at 3 m.
```

FOV survives (a ratio). **Depth of field does not.** Computing hyperfocal
distance from what USD reads versus what CINE-OMNI intends:

| | Hyperfocal distance |
|---|---|
| As real OpenUSD reads the file (f = 3.5 m) | **1461.8 m** |
| As CINE-OMNI intends (f = 35 mm) — and as `optics.rs`'s own doctest asserts | **14.6 m** |

**Exactly 100× wrong.** `cine-previs`'s own doctest in `lib.rs` asserts
`hyperfocal_m(35.0, 2.8, 0.03) ≈ 14.6`. So the crate is internally correct and
externally wrong by two orders of magnitude — and since the entire selling point
of `cine-previs` is *physically accurate optics*, this is precisely the value
that must survive interchange.

**The fix is one line and a choice:** either write `metersPerUnit = 0.01` and
keep millimetres, or divide focal length and apertures by 10 and keep metres.
The second is correct for a metric pipeline. **This is the single highest-value,
lowest-cost finding in this dossier.**

---

## 2. OpenUSD

### 2.1 The licence — verified verbatim

**MEASURED.** Cloned to this machine:

```
git clone --depth 1 https://github.com/PixarAnimationStudios/OpenUSD.git
# HEAD = 23b83aa8c479ec0f8b8b11dada50e764af6f3645, 2026-08-27
```

`LICENSE.txt` (895 lines) begins, verbatim:

```
============================================================
OpenUSD
============================================================

Note: The Tomorrow Open Source Technology License 1.0 differs from the 
original Apache License 2.0 in the following manner. Section 6 ("Trademarks") 
is different.

TOMORROW OPEN SOURCE TECHNOLOGY LICENSE 1.0
```

Sections 1–5 and 7–9 are Apache-2.0 unchanged. **Section 6, verbatim
(`LICENSE.txt:144`):**

```
   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor
      and its affiliates, except as required to comply with Section 4(c) of
      the License and to reproduce the content of the NOTICE file.
```

Stock Apache-2.0 §6 instead permits use *"as required for reasonable and
customary use in describing the origin of the Work and reproducing the content
of the NOTICE file."* **Two differences:** the restriction extends to the
Licensor's **"affiliates"**, and the **"describing the origin of the Work"**
allowance is **removed**, replaced by the narrower *"as required to comply with
Section 4(c)"*.

**When it changed.** `CHANGELOG.md`, under `## [24.08] - 2024-07-25`, verbatim:

> *"OpenUSD is now licensed under the Tomorrow Open Source Technology license.
> This is a renaming of the license, but the terms and conditions are the same
> as before."*

So the text has been Apache-2.0-with-modified-§6 throughout; at **24.08
(2024-07-25)** it acquired a *name*. **The claim that OpenUSD moved to plain
Apache-2.0 is false**, and this document corrects it.

**Independent corroboration:** the official `usd-core` PyPI package declares its
licence as **`LicenseRef-TOST-1.0`** (MEASURED, §3.5) — a SPDX *LicenseRef*,
i.e. Pixar's own tooling confirms there is no SPDX identifier for it.

**No SPDX tags on OpenUSD's own code.** `pxr/usd/usd/stage.h` and
`pxr/base/tf/token.h` both begin:

```
// Copyright 2016 Pixar
// Licensed under the terms set forth in the LICENSE.txt file available at
// https://openusd.org/license.
```

The only SPDX identifiers under `pxr/` belong to vendored third-party code —
48 × `BSD-3-Clause`, 3 × `BSD-2-Clause`, 2 × `Apache-2.0` (MEASURED via grep).

`NOTICE.txt` must be reproduced by redistributors under §4(c):

```
Universal Scene Description
Copyright 2016 Pixar
All rights reserved.
This product includes software developed at: Pixar (http://www.pixar.com/).
```

#### 2.1.1 Consequences for Apache-2.0 CINE-OMNI

| Question | Answer |
|---|---|
| Copyleft? | **No.** Permissive: same grants, same patent clause (§3), same §4 conditions as Apache-2.0. |
| May CINE-OMNI link it? | **Yes**, subject to §4 — retain licence, retain `NOTICE.txt`, state changes. |
| May CINE-OMNI vendor the source? | **Yes**, but those files stay under Tomorrow OSTL 1.0. The workspace `license = "Apache-2.0"` would become **inaccurate**. |
| May CINE-OMNI say "built on OpenUSD"? | **Not clearly granted** — §6 removes exactly that allowance. Nominative fair use may apply independently as trademark law. `UNVERIFIED`; a counsel question (U-9). |
| OSI-approved / SPDX-listed? | **No.** Confirmed by Pixar's own `LicenseRef-TOST-1.0`. |
| Practical cost | Real and ongoing: `cargo-deny`, SBOM tooling and corporate scanners will flag an unknown licence needing a manual allowlist entry. |

**Bottom line: legally adoptable, not friction-free — and anyone who told you it
was Apache-2.0 was wrong.**

### 2.2 Releases and build requirements

MEASURED from the clone:

* Latest release in `CHANGELOG.md`: **`[26.08] - 2026-07-20`**. Calendar `YY.MM`.
* `CMakeLists.txt:1` — `cmake_minimum_required(VERSION 3.27)`.
* `cmake/defaults/CXXDefaults.cmake:12` — `CMAKE_CXX_STANDARD 17`, required.
* `26.08` notes, verbatim: *"Deprecated support for Python 3.9 and 3.10"*;
  *"Increased minimum CMake version to 3.27"*; *"Updated to build with MaterialX
  1.39.5."*; *"Added support for Visual Studio 2026."*
* `24.08` notes, verbatim: *"boost is now an optional dependency that is only
  required for building Python bindings."*

**Dependencies, verbatim from `README.md:51`:**

```
Required:
 - C/C++ compiler
 - CMake
 - Intel TBB
```

Everything else is optional and gated behind components: **Imaging** needs
OpenSubdiv (and optionally OpenEXR, OpenImageIO, OpenColorIO, OSL, Ptex);
**Python bindings** need Python.

**This reframes the "USD is a huge build" objection.** The core — `Sdf`, `Pcp`,
`Usd`, `UsdGeom`, `UsdShade`, `UsdSkel`, `UsdLux`, `Ar` — needs a compiler,
CMake and TBB. Everything notoriously painful lives in Imaging, which CINE-OMNI
does not want: it already has `cine-gpu` (wgpu/WGSL), `cine-color` and
`cine-media`. **CINE-OMNI wants composition and schemas. It does not want Hydra.**

`VERSIONS.md` Linux row (Pixar's own tested matrix): AlmaLinux 9.5 · **gcc
11.5.0** · CMake 3.30.4 · Python 3.9.16 · Intel TBB 2020.3 / OneTBB 2021.9 ·
OpenSubdiv 3.6.1 · OpenImageIO 2.5.16.0 · OpenColorIO 2.2.1 · Embree 4.3.3.
**Note: gcc only. No Clang-on-Linux row, and certainly no `zig c++`.**

### 2.3 The architecture worth learning from

| Layer | Responsibility |
|---|---|
| `Ar` | **Asset resolution.** Pluggable `ArResolver` maps an opaque asset path (`@assets/char/ana.usd@`) to bytes. Studios ship resolvers talking to asset databases. |
| `Sdf` | **Scene description foundation.** `SdfLayer`, `SdfPath`, specs, the *list-op* value type, and the `.usda`/`.usdc` format plugins. |
| `Pcp` | **The composition engine.** Builds a *prim index* per prim: the ordered contributing sites resolved by LIVRPS. Nothing else in the ecosystem has an equivalent. |
| `Usd` | Value resolution and object model: `UsdStage`, `UsdPrim`, `UsdAttribute`, `UsdTimeCode`, schema registry, `UsdEditTarget`. |
| `UsdGeom`/`UsdShade`/`UsdSkel`/`UsdLux`/`UsdPhysics`/`UsdRender` | Typed schemas — both `IsA` (set the prim type) and *API* schemas (applied to add behaviour). |
| `Hd` / `UsdImaging` | Hydra render abstraction and its scene-index adapter. **Redundant against `cine-gpu`.** |

**Composition vocabulary** used throughout this document:

* **Layer** — a `.usda`/`.usdc`/`.usdz` file or anonymous buffer. Layers are
  *sparse overlays*, not scene graphs.
* **Layer stack** — root layer plus `subLayers`, recursively.
* **Reference** — pulls another layer's prim tree in *and re-roots it*; carries
  a layer offset (time scale/shift).
* **Payload** — a reference **not loaded by default**. USD's scalability
  primitive: a shot referencing 400 assets as payloads opens instantly.
* **Inherits** — a "class" arc; editing the class broadcasts to all inheritors.
* **Specializes** — the weaker-strength cousin: refine a base rather than
  override it.
* **Variant set / variant** — a *named switch* with named cases. §8.
* **`over`** — override without define. USD's answer to Blender's
  `IDOverrideLibrary` (§01 §1.2.4) — anchored to a **path**, not a name+index.
* **Time samples** — `{ timeCode: value }`. *They do not merge across arcs: the
  strongest layer with any samples wins the whole set.* A real gotcha.
* **Value resolution** — walk the prim index in strength order, take the first
  opinion. A *winner*, not a merge.
* **Primvar** — an attribute with an *interpolation* (constant/uniform/varying/
  vertex/faceVarying), i.e. **an attribute with a domain**. §01 §1.2.7 found the
  same idea in Blender and flagged that CINE-OMNI's flat attribute bag has none.
* **Purpose** — `default`/`render`/`proxy`/`guide`: cheap viewport proxy, heavy
  render geometry, one prim.
* **Kind** — `model`/`group`/`assembly`/`component`/`subcomponent`.
  **`component` = "a leaf publishable asset"**, `assembly` = "an asset of
  assets". USD's *asset-ness* marker — directly relevant to §7.

#### 2.3.1 LIVRPS

Composition arc strength, strongest first:

```
L  Local        opinions in the layer stack itself (root + sublayers)
I  Inherits
V  VariantSets
R  References
P  Payloads
S  Specializes
```

Corroborated by the `openusd` crate's README, which describes its composition
engine as *"LIVRPS strength ordering over a per-prim node graph"* and links
OpenUSD's own glossary; and by `pxr/usd/usd/docs/valueClips.md:446`, which
refers to *"the 'Local' (L in LIVRPS)"* data of the anchoring layer and to
*"the LIVRPS strength ordering"* (MEASURED, from the clone).

**The point that matters for §8: variants are stronger than references.** That
is why an asset can be referenced into a shot and the *shot* can still flip its
variant selection.

### 2.4 What is specified versus implementation-defined — this changed

**The AOUSD Core Specification 1.0 is officially ratified.** Per AOUSD's own
announcement, it normatively covers:

* *"Formal specification of OpenUSD's human-readable grammar and foundational
  data types"*
* The document data model and composition inputs
* *"A normative specification of the composition algorithm — the 'secret sauce'
  of how USD non-destructively aggregates heterogeneous data sources"*
* Stage population and value resolution
* *"Detailed specifications for USDA (human-readable), USDC (binary), and USDZ
  (packaged) interchange formats"*
* A compliance framework — *"validation rubric for implementers"*

**This overturns the standard objection to independent USD implementations.**
USDC is no longer defined only by `crateFile.cpp`; there is a normative spec and
a compliance suite with JSON baselines. That is precisely why §4's pure-Rust
implementation is credible — and it is *why it exists at all*.

`UNVERIFIED`: the exact ratification date, and AOUSD's founding date and
founding members (U-2, U-3). The announcement page carries a last-updated date
of 2026-08-11 but states no ratification date, and does not list members.

---

## 3. Can OpenUSD be used in this environment? — MEASURED

The task premise was that this machine lacks `pkg-config`, `make` and `nasm` but
has `zig cc`/`zig c++` 0.16. Correct, but incomplete in a way that changes the
answer entirely.

### 3.1 The real toolchain inventory

Probed with `command -v`, `PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"`:

| Tool | Present? |
|---|---|
| `cmake`, `make`, `ninja`, `pkg-config`, `cc`/`gcc`/`g++`/`clang`, `nasm`, `autoconf`, `meson`, `patchelf` | **all absent** |
| `python3` | **3.12.3** |
| `cargo` / `rustc` | **1.98.1** |
| `git`, `curl`, `tar`, `unzip` | yes |
| `uv` | **0.12.5** |

`zig` is present but **not on `PATH` under that name** — it is at
`~/.local/zig/zig`, exposed through wrappers this project created:

```sh
# ~/.local/bin/zigcc  / zigcxx
exec "$HOME/.local/zig/zig" cc  -target x86_64-linux-gnu.2.39 "$@"
exec "$HOME/.local/zig/zig" c++ -target x86_64-linux-gnu.2.39 "$@"
```

wired into `~/.cargo/config.toml` as `linker`, `CC` and `CXX`.
**`zig version` → `0.16.0`**, and `zigcxx` was verified to compile and run a
`<cstdio>` C++ program (MEASURED).

### 3.2 The missing tools are obtainable without root — MEASURED

```
uv pip install cmake ninja
→ cmake==4.4.3, ninja==1.13.2      # both run; verified --version
```

CMake and Ninja ship real binaries in their PyPI wheels. **No package manager,
no `sudo`, no C compiler required.** CMake 4.4.3 exceeds OpenUSD's 3.27 minimum.
Ninja replaces `make`; OpenUSD finds TBB via its own `FindTBB.cmake` and
`TBB_ROOT_DIR`, not `pkg-config`; `nasm` is only needed by image codecs in
Imaging, which is not wanted.

**So the premise's "BLOCKED" framing does not hold.** But it turned out not to
matter, because of §3.3 and §4.

### 3.3 Source-building OpenUSD: NOT BLOCKED, but NOT ATTEMPTED

| Step | Status |
|---|---|
| Obtain source (12,920 files, 416 MB) | ✅ **DONE** (MEASURED) |
| CMake ≥ 3.27 | ✅ **DONE** — 4.4.3 |
| Build driver | ✅ **DONE** — ninja 1.13.2 |
| C++17 compiler | ✅ **DONE** — `zig c++` 0.16.0 verified working |
| Build oneTBB, configure, compile, link | ⏳ **NOT ATTEMPTED** (U-10) |

Residual risks, most likely first: **(1)** `zig c++` is Clang-based but is not
Clang, and Pixar tests only gcc on Linux (§2.2) — CMake compiler-ID branching
and Zig's bundled libc++/glibc-stub targeting are the likely failure point;
**(2)** CMake 4.x dropped `cmake_minimum_required(VERSION <3.5)` support, which
OpenUSD itself satisfies but **TBB 2020.x may not** (prefer oneTBB 2021.9+);
**(3)** build time and disk — deliberately not estimated (U-11).

**This was not pursued, because §3.4 and §4 make it unnecessary.**

### 3.4 `usd-core` — real OpenUSD, installed, running — MEASURED

**PyPI `usd-core` 26.8** (verified against `https://pypi.org/pypi/usd-core/json`):

| Field | Value |
|---|---|
| Version | **26.8** |
| Licence | **`LicenseRef-TOST-1.0`** — independently corroborating §2.1 |
| `requires_python` | `>=3.9,<3.15` |
| Wheels | `manylinux_2_27/2_28_x86_64`, `macosx_10_15_universal2`, `win_amd64`, for cp39–cp314 |
| Linux wheel size | ~29.6 MB |
| Summary | *"Pixar's Universal Scene Description"* |

Installed and executed on this machine:

```
uv pip install usd-core          → usd-core==26.8
python -c "from pxr import Usd, UsdGeom, Sdf, Pcp, UsdLux
           print(Usd.GetVersion())"
→ OpenUSD version: (0, 26, 8)     # stage created, prims defined: OK
```

**Real OpenUSD 26.8 — core, composition, schemas — runs here today, with no
compiler involved.** This is what made §1.4's validation possible, and it is a
complete answer for any subprocess/sidecar mode.

---

## 4. Rust and OpenUSD — the decisive finding

### 4.1 Every candidate crate, verified

All rows verified against `https://crates.io/api/v1/crates/<name>` on
2026-09-08.

| Crate | Latest | Licence | Last release | Downloads | Verdict |
|---|---|---|---|---|---|
| **`openusd`** | **0.7.0** | **MIT** | **2026-09-05** | 20,202 | **Live, serious, pure Rust. §4.2** |
| `openusd-rs` | 0.1.0 | MIT OR Apache-2.0 | 2025-07-24 | 2,808 | Single release, self-described *"work in progress"*. Watch, don't adopt. |
| `rust-usd` | 0.0.5 | Apache-2.0 | 2026-06-10 | **150** | Genuine C++ FFI attempt (*"stage open, prim/mesh attrs, variants, sublayer authoring, UsdShade read+write, ArResolver hook"*). 150 downloads, `0.0.x`, needs `rust-usd-build` (38 downloads). **Too immature to depend on.** |
| `pxr` | 0.2.0 | MIT OR Apache-2.0 | 2024-04-27 | 8,404 | *"Rust interface for OpenUSD"*. No release in ~2.5 years. **Stale.** |
| `usd` | 0.0.9 | Apache-2.0 | 2020-12-21 | 16,731 | **Abandoned** ~6 years. |
| `usd-sys` | 0.0.0 | BSD-3-Clause | 2020-11-03 | 1,801 | **Name-squat** — description literally says *"(placeholder)"*. |
| `usd-rs` | — | — | — | — | **Does not exist.** |

**A blunt note on name-squatting.** One author (`scott-wilson`) registered a
family of `0.0.0`/`0.1.0` crates in Oct 2020–Jan 2021 whose descriptions say
*"(placeholder)"*: `usd-sys`, `materialx`, `alembic`, `opentimelineio`,
`opensubdiv`. **These occupy the obvious names for five major VFX libraries and
contain nothing.** Anyone evaluating Rust VFX support by searching crates.io
will find them and draw the wrong conclusion in both directions — they look like
coverage and are not, and they hide the real work under less obvious names.

### 4.2 The `openusd` crate — MEASURED

`https://github.com/mxpv/openusd` · MIT (`Copyright (c) 2024 Maksym Pavlenko`,
read from the repository's `LICENSE`) · **v0.7.0 published 2026-09-05**, three
days before this research. Release cadence over 2026: 0.2.0 (Apr) → 0.3.0 (Apr)
→ 0.4.0 (May) → 0.5.0 (Jun) → 0.6.0 (Aug) → 0.7.0 (Sep). **Actively developed.**

Self-described as *"a Rust implementation of Pixar's Universal Scene Description
(USD) format with no C++ dependencies."* Per its README, it provides:

* **File formats — reads and writes `.usda` (text), `.usdc` (binary) and
  `.usdz` (archive).**
* **A full composition engine** — *"LIVRPS strength ordering over a per-prim node
  graph, with list editing, scene-graph instancing, non-destructive relocates,
  and variable expressions."*
* **A composed `Stage`** — lazy cached per-prim composition, typed value
  resolution, predicate traversal.
* **An authoring API** — layer- and stage-tier edits, `EditTarget` routing,
  anonymous-root stages, applied API schema authoring.
* **Schema readers** (opt-in per family): `UsdGeom`, `UsdLux`, `UsdPhysics`,
  `UsdRender`, `UsdSkel`, `UsdShade` — in the companion `openusd-schemas` crate.
* **Live-sync support** — replayable `Diff` events for mirroring across
  processes. *(Note how directly that maps onto CINE-OMNI's op log.)*

**AOUSD Core Specification 1.0 compliance, per its own README:** text-format
parsing ✅ passes (10 baseline tests); **binary format ✅ passes (42 tests)**;
**composition ✅ passes (full vendor suite, 138 assets)**; combine chains ✅
passes; value resolution ☑ *partial* (8 tests; excludes attribute fallbacks and
splines). `UNVERIFIED` — these are the project's own claims, not independently
re-run here (U-6).

**What was independently MEASURED on this machine:**

```
[dependencies]
openusd = "0.7"
```

```
cargo build     → compiled clean, no CMake, no C++, no system libraries
cargo run -- scene1.usda
→ pure-Rust openusd opened: scene1.usda
    /World  /World/Set  /World/Set/Floor  /World/Characters
    /World/Characters/MAREN  /World/Characters/ELIAS
    /World/Cameras  /World/Cameras/A  /World/Lights
```

**It read `cine-previs`'s real output correctly, first try.** And the dependency
tree is clean:

```
cargo tree | wc -l          → 79
cargo tree | grep -i sys    → (nothing)
```

Direct dependencies are ordinary Rust crates — `bitflags`, `bytemuck`,
`derive_more`, `half`, `logos`, `num-traits`, `zerocopy`. **Zero `-sys` crates.
Zero C. Zero C++. Zero build-system requirements beyond cargo.**

**Caveat, stated plainly.** It is `0.7.0` and its README warns: *"These crates
are under active development. No API stability is guaranteed until version
1.0."* It is substantially a **single-maintainer project**. Value resolution
compliance is partial. Depending on it is a real risk — but it is a *contained,
MIT-licensed, forkable, pure-Rust* risk, which is a categorically different
thing from a C++ build dependency.

---

## 5. Decision — should real OpenUSD become a dependency?

### 5.1 Recommendation

> **Yes. TYPE B (LIBRARY), via the pure-Rust `openusd` crate, behind a cargo
> feature — not the C++ implementation, and not a process boundary.**

**Stage 0 — immediately, no dependency.** Two things, both cheap:
1. Apply the §1.3 wording changes. Stop calling the emitter USD.
2. **Fix the camera units defect (§1.4).** One line. It is currently exporting
   physically wrong optics from a crate whose purpose is physically accurate
   optics.

**Stage 1 — a `usd` feature on a new `cine-usd` crate.**
```toml
[dependencies]
openusd = { version = "0.7", optional = true }
openusd-schemas = { version = "0.7", optional = true, features = ["geom","lux","shade"] }
[features]
usd = ["dep:openusd", "dep:openusd-schemas"]
```
`cine-usd` converts between the UCG and a real `openusd::usd::Stage`, carrying
`EntityId` in prim metadata (the `cine:entity` carrier already works, §1.4).
With the feature off, the existing native emitter remains the default and
`cargo test --workspace` is unchanged. **Follow the `cine-media` capability-gate
pattern exactly**: a `NoUsdSupport`-style error naming real alternatives, never
a silently degraded file.

**Stage 2 — conformance gates, before any claim is made.** Two tests:
* Round-trip through `openusd` in-process.
* **A `need!`-style gated test that shells out to real OpenUSD (`usd-core`,
  §3.4) and asserts the layer opens clean** — following `CLAUDE.md`'s rule that
  media tests use real files and skip loudly when absent. The scaffolding for
  this already works; §1.4 is that test, run by hand.

**Stage 3 — only when a workflow demands it: composition.** Once `openusd` is
in, references/payloads/variants become available, and §7's asset system gains a
natural export: an `asset.version` becomes a referenced layer, an
`asset.binding` becomes a reference with a variant selection.

**Do not adopt the C++ OpenUSD as a linked dependency.** If a capability is ever
needed that `openusd` lacks, reach for `usd-core` as a **TYPE C sidecar**
(§3.4 — it already runs here), never a `-sys` crate.

### 5.2 Why TYPE B is now right, when it would have been wrong for C++ USD

| Consideration | C++ OpenUSD (TYPE B) | `openusd` crate (TYPE B) |
|---|---|---|
| Build | CMake + C++17 + TBB; `cargo test --workspace` stops being one command | **`cargo build`. MEASURED.** |
| Licence | Tomorrow OSTL 1.0 — bespoke, non-SPDX, flags in every scanner | **MIT** — maximally compatible, universally recognised |
| Dependency weight | 416 MB source; ~30 min builds | **79 crates, no `-sys`. MEASURED.** |
| Cross-compilation | A porting project per target | Whatever Rust targets |
| Crash blast radius | C++ segfault kills the daemon; USD has had `.usdc` parsing CVEs | Safe Rust; parse errors are `Result` |
| Threading | OpenUSD owns TBB arenas; contends with `rayon` | Plain Rust |
| Maturity | 20 years, reference implementation | **0.7.0, single maintainer, partial value-resolution compliance** |
| Escape hatch if it fails | — | Fork it (MIT), or fall back to the sidecar |

The decisive point is the first. **CINE-OMNI's build is `cargo test --workspace`
and nothing else** — an enormous, under-appreciated asset for a project of this
ambition. The pure-Rust route buys USD without spending it.

### 5.3 Rejected alternatives

**H — Reject USD entirely; keep the private format.**
Rejected. USD is the only format carrying a whole scene — hierarchy, cameras,
lights, materials, skinning, variants, layered department edits — and §01
already settled it as the Blender bridge contract
(`02-TARGET-ARCHITECTURE.md:115`). Rejecting USD strands previs inside
CINE-OMNI, defeating its purpose: other departments must consume it.

**B via C++ (`-sys` crate).**
Rejected — the table above. It converts a pure-Rust one-command build into a
CMake/C++ build for a capability on no latency-critical path. Also: the only
extant C++ binding, `rust-usd`, has **150 downloads** and is `0.0.5`.

**C — sidecar process as the *primary* mechanism.**
Rejected as primary, **retained as the fallback and as the test oracle.** It
works today (§3.4) and is the right answer if `openusd` proves insufficient. But
paying a process boundary, an IPC protocol and a Python runtime for something a
cargo dependency does natively is unjustified.

**G — write CINE-OMNI's own full USD implementation.**
Rejected — `openusd` already did it, under MIT, and is further along than a
from-scratch effort would get in a year. Note this *was* the right call before
AOUSD Core Spec 1.0 existed; the ratified spec (§2.4) is what made an
independent implementation tractable. **Partially accepted as technique:** USD's
composition *model* is worth reimplementing **inside the UCG for CINE-OMNI's own
assets** — §7 and §8. That is TYPE G done correctly: steal the design, not the
scope.

**F — fork OpenUSD (C++).**
Rejected. No licence obstacle; no benefit. Inherits the maintenance burden being
avoided, and diverging from `pxr` destroys the only reason to use USD.

**E only — file interchange forever, no library.**
Rejected as an endpoint, **accepted as Stage 0.** Without a real
implementation there is no composition, and §2.5's point stands: a hand-written
emitter can be made plausible but never *provably* conformant. Now that a
compliance suite and a Rust implementation both exist, staying at TYPE E is a
choice to be permanently worse for no saving.

**D — route USD through the user's Blender.**
Rejected as primary; retained for testing. §01 §1.4 established Blender as
TYPE D. Routing USD through it bounds CINE-OMNI's fidelity by Blender's
importer and by the user having Blender installed. A cheap way to *test* interop,
not a way to *have* USD support.

### 5.4 Classification

> **OpenUSD — TYPE B (LIBRARY) via the pure-Rust MIT `openusd` crate, behind a
> cargo feature; TYPE C (`usd-core` sidecar) as fallback and test oracle;
> TYPE G for the composition model inside the UCG.**
>
> **Licence:** the *format* is governed by AOUSD Core Specification 1.0
> (ratified); Pixar's *implementation* is Tomorrow OSTL 1.0 (verified verbatim,
> §2.1) and is **not** what CINE-OMNI would link. The `openusd` crate is **MIT**
> (verified from its `LICENSE`) — clean under Apache-2.0.
> **Architecture fit:** composition (`Pcp`/LIVRPS) is the thing worth having;
> Hydra/Imaging are redundant against `cine-gpu`/`cine-color`/`cine-media` and
> are absent from the Rust crate anyway — which is a feature.
> **Build:** `cargo build`, 79 crates, zero `-sys`, MEASURED on this machine,
> and it read CINE-OMNI's own output correctly.
> **Risk:** `0.7.0`, no API stability before 1.0, largely one maintainer,
> partial value-resolution compliance. Mitigated by the cargo feature gate, by
> MIT forkability, and by the sidecar fallback that already runs here.

---

## 6. Interchange formats

All licences below read directly from the projects' own `LICENSE` files; all
crates verified against the crates.io API on 2026-09-08.

| Format | Purpose | Licence (verified) | Rust support (verified) | Type |
|---|---|---|---|---|
| **OpenUSD** | Full scene interchange **with composition** | Impl: **Tomorrow OSTL 1.0**; format: AOUSD Core Spec 1.0 | **`openusd` v0.7.0, MIT, 2026-09-05 — pure Rust, builds here** | **B** (§5) |
| **glTF 2.0** | Runtime/web delivery; PBR, skinning, animation | `UNVERIFIED` (U-7) | **`gltf` v1.4.1, MIT OR Apache-2.0, 2024-05-10, 9.34M downloads** | **B** (import) |
| **OpenEXR** | HDR images | — | **`exr` v1.74.2, BSD-3-Clause, 2026-07-10, 69.7M downloads** — pure Rust, *"without any unsafe code"* | **B** |
| **Alembic** | Baked geometry/point caches | **BSD-3-Clause** — SPDX header; Lucasfilm + Sony Pictures Imageworks, 2009–2015 | **None.** `alembic` v0.1.0 is a *"(placeholder)"* squat (§4.1) | **E**, deferred |
| **MaterialX** | Material graph interchange | **Apache-2.0** (verified) | **None.** `materialx` v0.0.0 is a *"(placeholder)"* squat | **E**, deferred |
| **USDZ** | Zipped USD delivery package | as OpenUSD | Supported by the `openusd` crate | with **B** |
| **OpenSubdiv** | Subdivision surfaces | — | `opensubdiv-petite` v0.3.1, Apache-2.0, 2026-05-14 — *"wrapper around parts of"*; C++ underneath | deferred |
| **OpenTimelineIO** | Editorial (already in `cine-timeline`) | — | **None usable.** `opentimelineio` v0.1.0 is a *"(placeholder)"* squat — vindicating `cine-timeline`'s native implementation | already **A/E** |

**Notes.**

* **glTF** — `gltf` v1.4.1 is the de-facto standard (9.34M downloads) and dual
  MIT/Apache-2.0, ideal for CINE-OMNI. It is *stable rather than abandoned*, but
  the last release was **2024-05-10**; treat maintenance as a watch item. It
  supports GLB and a set of extensions (`extras`/`names` behind features);
  the exact extension list is `UNVERIFIED` (U-8).
* **MaterialX is Apache-2.0**, the friendliest licence encountered here, and
  OpenUSD 26.08 builds against **MaterialX 1.39.5** (§2.2). With no Rust binding
  it is TYPE E at best today; if `UsdShade` graphs are ever needed, the
  `openusd-schemas` `shade` feature is the near-term route.
* **Alembic is BSD-3-Clause** and unencumbered, but has **no Rust support at
  all**, and its value is largely subsumed by USD for CINE-OMNI's purposes.
* **Structural point that stands regardless of the pending items: glTF and
  Alembic are not substitutes for USD.** Alembic is a *cache* format — no
  references, no variants, no material bindings; sampled geometry over time.
  glTF is a *delivery* format — self-contained, GPU-ready, no composition, no
  layering. **Neither can carry an asset system.** Only USD can, which is why §7
  builds the asset system natively rather than delegating it to a file format.

---

## 7. The persistent asset system

### 7.1 The requirement

The directive: *"Do not implement a simplistic file browser and call it an asset
system."* §01 §1.5 G-7 already identified the gap:

> *"`assets/` is currently just a content-addressed directory. The asset
> *catalogue* — a UUID-keyed, hierarchical, human-editable taxonomy that is
> independent of the filesystem — is the missing half."*

A real asset system answers, for any asset, at any time:

1. **Identity** — what *is* this, stably, across renames, moves, re-imports,
   format changes and process restarts?
2. **Variants** — what appearances does it have, and how do they combine without
   exploding?
3. **Versions** — what did it look like the day we shot Scene 42, and can we get
   it back?
4. **Dependencies** — what does it need, what needs it, what breaks if it changes?
5. **Provenance** — where did it come from, who changed it, when, why, from what?
6. **Binding** — which *use* in which shot is this, and can that use differ from
   the asset's default without forking it?

**CINE-OMNI is unusually well-placed**, because four of six are already
structurally solved and nothing else in the industry gets them free:

* **Identity** → `EntityId(Uuid)`, authored v4 / derived v5, permanent and
  cross-process. §01 §1.2.1 established this beats Blender's name-scoped
  `ID::name[258]`. **It also beats USD**, whose prim identity *is* its
  `SdfPath` — rename a prim and every relationship targeting it breaks, exactly
  as in Blender.
* **Dependencies** → `EdgeKind::{Contains, DependsOn, DerivedFrom, References,
  SyncedTo, Binds}`, with `propagates()` already separating dirtying edges from
  loose ones.
* **Provenance** → every delta already carries `cause`, `depth`,
  `(actor, lamport)`.
* **Versioning substrate** → the append-only op log.

**So the asset system is mostly naming things properly plus the two genuinely
missing pieces: a variant model and a publish/version model.** It is not a new
subsystem — which is the whole argument for TYPE A here.

### 7.2 Design principles

1. **An asset is a node, not a file.** Per Blender's best idea (§01 §1.2.5:
   *"any data-block can be an asset — it is a flag plus metadata on the existing
   ID, not a separate object"*) and USD's `kind = component`. Files hang off it
   as `asset.file` nodes.
2. **Separate the asset from the *use* of the asset.** The most important
   decision here. `asset.character/Ana` is the character; `asset.binding` is
   *"Scene 42 uses Ana, `bloodied` costume, v7"*. A shot points at a binding,
   never at an asset. This is USD's reference-with-variant-selection as two
   nodes and an edge, and it is what makes variants and versions per-shot
   without forking.
3. **Definition immutable, selection mutable.** `asset.version` is write-once;
   editing creates a new version; *selecting* a version is an attribute edit on
   a binding — cheap, undoable, and it never touches the asset.
4. **Variants are orthogonal named axes, never a product.** §8.
5. **Content-addressed identity for bytes; UUID identity for meaning.**
   Complementary, joined, never merged. §9.
6. **Catalogue membership is an edge, not a path.** Straight from §01 §1.2.5 —
   Blender reached for UUID-keyed catalogues *because paths and names were not
   stable enough*. Do not repeat a mistake already documented.

### 7.3 Node kinds

Following `crates/ucg-schema/src/kinds.rs` conventions — `module.type` lowercase
snake_case `&str` constants — extending the existing `ASSET_MEDIA`/`ASSET_MODEL`
namespace.

**Note the deliberate overlap with existing kinds.** `script.character`,
`script.location` and `script.prop` already exist and are *extracted from the
screenplay* — the script's view. `asset.character` is the *production* entity.
They are different nodes joined by `References`, because the script's "ANA" and
the art department's Ana-with-three-costumes have different lifecycles.
**Collapsing them would let a script rewrite destroy costume data.**

| Constant | Kind string | Meaning |
|---|---|---|
| **Catalogue** | | |
| `ASSET_LIBRARY` | `asset.library` | A root of assets — project, studio, or purchased pack. Carries a resolver hint. |
| `ASSET_CATALOG` | `asset.catalog` | A node in a **UUID-keyed hierarchical taxonomy** (`Characters/Leads`). Independent of the filesystem — §01 G-7. |
| **Assets** | | |
| `ASSET_CHARACTER` | `asset.character` | A character as a production entity. |
| `ASSET_COSTUME` | `asset.costume` | Wardrobe item; itself variant-bearing. |
| `ASSET_PROP` | `asset.prop` | A prop. |
| `ASSET_VEHICLE` | `asset.vehicle` | Distinct from prop: carries rigging and scheduling weight a prop does not. |
| `ASSET_SET` | `asset.set` | A constructed set — a buildable assembly of pieces. |
| `ASSET_LOCATION` | `asset.location` | A real place: survey data, permits, coordinates. |
| `ASSET_ENVIRONMENT` | `asset.environment` | A digital environment — the CG counterpart of a location. |
| `ASSET_MATERIAL` | `asset.material` | A material/shader definition. |
| `ASSET_TEXTURE` | `asset.texture` | A texture map. |
| `ASSET_RIG` | `asset.rig` | Control rig / skeleton. |
| `ASSET_ANIMATION` | `asset.animation` | A reusable clip / cycle. |
| `ASSET_MODEL` | `asset.model` | **exists** — geometry. |
| `ASSET_MEDIA` | `asset.media` | **exists** — a media file (§9). |
| `ASSET_REFERENCE` | `asset.reference` | Reference art, lookbook, concept, scan. First-class deliberately: on a real film this is where most of the library actually is. |
| **Structure** | | |
| `ASSET_VERSION` | `asset.version` | **An immutable published snapshot.** |
| `ASSET_VARIANT_SET` | `asset.variant_set` | A named orthogonal axis (`costume`, `lod`, `damage`). |
| `ASSET_VARIANT` | `asset.variant` | One case in a set (`clean`, `bloodied`). |
| `ASSET_BINDING` | `asset.binding` | **A *use* of an asset** — holds version pin and variant selections. |
| `ASSET_FILE` | `asset.file` | A concrete resolved payload for one version. |

### 7.4 Attribute keys

Extending `ucg-schema::attrs` (short snake_case, per convention). Structured
values go in typed `Component`s per `ucg_schema::Component` — one attribute
each, serialized as a `Value` tree.

| Key | On | Meaning |
|---|---|---|
| `asset_kind` | any `asset.*` | Queryable taxonomy tag. |
| `asset_status` | `asset.version` | `wip`/`review`/`approved`/`published`/`retired`/`omitted`. |
| `version` | `asset.version` | Monotone u32. Never reused. |
| `version_pin` | `asset.binding` | `{"mode":"pinned","version":7}` \| `{"mode":"latest"}` \| `{"mode":"latest_approved"}`. §9.3. |
| `variant_selection` | `asset.binding` | `{"costume":"bloodied","lod":"high"}` — **one selection per set, not a product**. |
| `content_hash` | `asset.version`, `asset.file` | Content address (§9.2). |
| `dep_closure_hash` | `asset.version` | Hash over this version **and its dependency closure** — §01 G-11 (`ID::deep_hash`). |
| `published_at` / `published_by` | `asset.version` | Provenance. |
| `changelog` | `asset.version` | Why this version exists. |
| `provenance` | any asset node | `Provenance` component, §7.6. |
| `catalog_path` | `asset.catalog` | **Display only. Never identity.** |
| `pinned` | any asset node | Blender's "fake user" (§01 §1.2.1 obs. 2) — exempt from GC. |
| `role` | `asset.binding` | `hero` / `background` / `stand_in`. |

### 7.5 Edge kinds — the existing six suffice

**A finding worth stating: the asset system needs no new `EdgeKind` variants.**
The six in `ucg-core` with their existing `propagates()` semantics cover every
relation below. Adding variants would break `ucg-core` for no expressive gain.

| From → To | Edge | Propagates | Meaning |
|---|---|---|---|
| `asset.library` → `asset.catalog` | `Contains` | no | Library owns catalogue roots. |
| `asset.catalog` → `asset.catalog` | `Contains` | no | Taxonomy — **a DAG**: an asset may sit in several catalogues. |
| `asset.catalog` → `asset.*` | `Contains` | no | Catalogue membership. |
| `asset.*` → `asset.version` | `Contains` | no | An asset owns its history. |
| `asset.version` → `asset.file` | `Binds` | no | A version resolves to payloads. |
| `asset.*` → `asset.variant_set` | `Contains` | no | Declares its axes. |
| `asset.variant_set` → `asset.variant` | `Contains` | no | Declares its cases. |
| `asset.binding` → `asset.*` | `Binds` | no | **The use points at the asset.** |
| `asset.binding` → `asset.version` | `References` | **no** | The resolved pin — deliberately non-propagating. §9.3. |
| consumer (`previs.scene`, `logistics.shot`, `nle.clip`) → `asset.binding` | `Contains` | no | The consumer owns its bindings. |
| `asset.version` → `asset.version` | `DependsOn` | **yes** | Ana v7 depends on Costume v3. |
| `asset.material` → `asset.texture` | `DependsOn` | yes | Material needs its maps. |
| `asset.rig` → `asset.model` | `DependsOn` | yes | Rig targets geometry. |
| `asset.animation` → `asset.rig` | `DependsOn` | yes | Clip targets a rig. |
| `asset.character` → `asset.costume` | `References` | no | *Available* wardrobe; the **worn** costume is a variant selection on a binding. |
| `asset.character` → `script.character` | `References` | no | Production ↔ script entity. Loose deliberately (§7.3). |
| `asset.set` → `asset.prop` | `Contains` | no | Set dressing. |
| `asset.environment` → `asset.location` | `DerivedFrom` | yes | CG environment from a scanned location — re-scanning dirties the build. |
| `asset.version` → `asset.media` | `DerivedFrom` | yes | Version produced by ingesting media. §9. |
| `previs.prim` → `asset.binding` | `Binds` | no | A stage prim instances a bound asset. **Where §7 meets `cine-previs`.** |
| `logistics.shot` → `asset.binding` | `DependsOn` | yes | Scheduling depends on which assets a shot needs — makes the breakdown reactor correct. |

Reified dependency nodes are an **escape hatch only**, for dependencies needing
their own attributes. Default to plain edges: a graph full of reified edges has
lost its edge semantics.

### 7.6 Provenance

A typed `Component` on any asset node:

```rust
/// Where an asset came from. Lives in attrs::PROVENANCE.
pub struct Provenance {
    /// "import.gltf" | "scan.photogrammetry" | "authored" | "derive.previs"
    pub origin: String,
    /// Stable external key, matching EntityId::from_key("<importer>:<key>").
    pub source_key: Option<String>,
    pub source_uri: Option<String>,
    /// Content hash of the source at import time.
    pub source_hash: Option<String>,
    pub tool: Option<String>,
    /// Licence of the source asset, SPDX where possible.
    pub license: Option<String>,
    /// Attribution that licence requires.
    pub attribution: Option<String>,
    pub imported_at: Option<String>,
    pub imported_by: Option<String>,
}
```

Two points deserve emphasis:

* **`license` and `attribution` are not decoration.** A film's library
  accumulates purchased models, scanned performers with likeness agreements,
  CC-BY-SA environments and confidential geometry. **A project whose entire
  blueprint is a licensing spine (`01-LICENSING-SPINE.md`) should track licence
  provenance on its assets, not only on its code.** This is the most
  differentiated idea in this section, and no open-source DCC does it. §4.1's
  placeholder-crate problem is the same failure at a different layer: nobody
  tracked what was actually inside the name.
* **`source_key` must follow the existing convention** — `CLAUDE.md`: *"importer
  ids: `EntityId::from_key("<importer>:<stable key>")"`*. Re-importing the same
  source must land on the same `EntityId` and produce **zero ops**, per the
  idempotence invariant required of every graph writer.

Per-*change* provenance is already free from the op log. `Provenance` covers
only what the op log cannot know: the world outside the graph.

### 7.7 Why this is not a file browser

| A file browser has | This design has |
|---|---|
| Paths as identity | `EntityId(Uuid)`; paths are display-only |
| Folders | A **DAG** catalogue; an asset lives in many taxonomies |
| Files | Assets whose *versions* bind to files; the asset outlives every file |
| No dependency knowledge | Typed propagating edges feeding the reactor cascade |
| No variants | Orthogonal variant sets with per-binding selection (§8) |
| No versions | Immutable version chain with pins and provenance |
| Mutation destroys the past | Append-only op log + immutable versions |
| Renaming breaks references | Renaming is an attribute edit; nothing points at names |
| Can't answer "what breaks if I change this" | `Graph::dependents` answers it exactly |
| Can't answer "what did Scene 42 look like on the shoot day" | Version pins answer it exactly |

---

## 8. Variants without combinatorial explosion

### 8.1 What USD teaches

The naive model is one variant per whole-asset state: `Ana_clean_high_young`,
`Ana_bloodied_high_young`, … — the product of every axis. Three axes of three
values is 27 assets; four is 81; each is a separate thing to build, review,
publish and fix.

**USD's answer is the variant *set*.** A prim carries *several independent*
variant sets, each with its own cases and its own selection:

```usda
def Xform "Ana" (
    variantSets = ["costume", "lod", "age"]
    variants = { string costume = "clean"; string lod = "high"; string age = "young" }
) { ... }
```

The stage composes **one selection per set**, independently. Three sets of three
values is **3 + 3 + 3 = 9** authored blocks expressing 27 states; a fourth axis
costs **3 more, not 54 more**. The combinatorics are *additive* because the axes
are orthogonal and resolve independently.

Three further lessons carry over exactly:

1. **Variants are stronger than references in LIVRPS (§2.3.1)** — so a shot can
   reference an asset and *still* flip its selection, and the shot's choice
   beats the asset's default. **This is precisely why the selection belongs on
   the binding, not on the asset** (§7.2 principle 2).
2. **A variant block is arbitrary scene description**, not a value swap: it may
   add prims, remove them, retarget bindings, change topology. A variant is a
   **diff**, not an enum.
3. **Variant sets nest** — a variant may declare its own sets. That is how "the
   `destroyed` variant has a `debris_amount` axis the `clean` variant doesn't"
   is expressed. **Axes can be conditional.**

### 8.2 The UCG model

```
asset.character "Ana"
├─(Contains)→ asset.variant_set "costume"
│              ├─(Contains)→ asset.variant "clean"
│              ├─(Contains)→ asset.variant "bloodied"
│              └─(Contains)→ asset.variant "burnt"
├─(Contains)→ asset.variant_set "lod"   → "high" | "mid" | "low"
└─(Contains)→ asset.version 1..N
```

```
logistics.shot "42A"
└─(Contains)→ asset.binding
                 ├─(Binds)→      asset.character "Ana"
                 ├─(References)→ asset.version 7
                 └─ variant_selection = {"costume":"bloodied","lod":"high"}
                    version_pin       = {"mode":"pinned","version":7}
                    role              = "hero"
```

**The rules that prevent explosion:**

1. **A selection is a map from set-name to variant-name** — never a composite
   identifier. `{"costume":"bloodied"}`, not `"Ana_bloodied"`.
2. **An unselected set falls back to the set's default.** A binding records only
   the axes it overrides; most carry an empty or one-entry map. **Sparse
   selection is what makes this cheap at film scale.**
3. **A variant holds a *diff*, not a copy** — a list of attribute/edge overrides.
   Same shape as USD's `over` and Blender's
   `IDOverrideLibraryPropertyOperation` list (§01 §1.2.4) — except **anchored to
   `EntityId`**, which is why CINE-OMNI needs none of the
   `subitem_reference_name` / `subitem_local_index` / `subitem_reference_id`
   machinery Blender needs to disambiguate name- and index-anchored overrides.
   **§01 §1.2.4 predicted this exactly: *"USD's answer is unambiguously the
   better one, and it is available to CINE-OMNI."***
4. **Conditional axes are declared, not enumerated** — an `asset.variant` may
   `Contains` an `asset.variant_set`, USD's nesting. `destroyed → debris_amount`
   costs one extra set; `clean` simply has no such axis.
5. **Resolution is a pure function.** `resolve(asset, selection, version) ->
   ResolvedAsset` lives in a domain crate, pure and idempotent per `CLAUDE.md`;
   the `cine-pipeline` reactor is the only thing that writes results. **Nothing
   is materialised per combination: the 27 states are computed, never stored.**
   That is the whole answer — the product is a *query result*, not a *node set*.
6. **Caching is keyed by `dep_closure_hash`** (§7.4), so expensive resolutions
   memoise by content and two shots selecting the same combination share it
   free. §01 G-11.

### 8.3 What USD's variant model gets *wrong* — do not copy

* **A USD variant selection is an unvalidated string.** Selecting a
  non-existent variant silently resolves to nothing. CINE-OMNI's
  `asset.variant_set` makes the legal values *graph data*, so an invalid
  selection is a `Graph::validate()` finding — which §01 §1.3.3 finding (5)
  already argues must be run somewhere real.
* **USD variant sets are per-prim**, so selecting across a deep hierarchy means
  authoring at many prims. CINE-OMNI's binding is per-*use*, the granularity
  artists actually think at.
* **USD has no version concept at all.** Versioning in USD pipelines happens
  *outside* USD, in the `ArResolver` rewriting `@ana.usd@` to `@…/v007/ana.usd@`.
  **That is strong evidence §7's `asset.version` + `version_pin` fills a real,
  universally acknowledged gap** — every studio builds this layer itself, and it
  is exactly the layer a unified environment should own.

---

## 9. Versioning, provenance and the two identity systems

### 9.1 Three kinds of history — do not conflate them

| Mechanism | Granularity | Purpose |
|---|---|---|
| **Op log** (`ucg-core`) | One invertible delta | Undo/redo; audit; who/what/when/why |
| **`asset.version`** | One deliberate publish | "Other departments may depend on this" |
| **Content hash** (`cine-media` CAS) | One byte-identical payload | Dedup; "are these the same bytes" |

**They must not be merged:**

* **Not every delta is a version.** An artist nudging a light 400 times produces
  400 deltas and zero versions. **A version is an editorial act.** Auto-versioning
  every edit makes version numbers meaningless.
* **Not every version is a new hash.** A metadata-only change keeps
  `content_hash` and increments `version`. Conversely two assets may share a
  hash for a shared texture.
* **A hash is not an identity.** Two `asset.texture` nodes with different
  provenance, licences and catalogue memberships may resolve to identical bytes.
  Identity is the `EntityId`; the hash is a property of the payload.

### 9.2 Joining the content-addressed media identity

`docs/blueprint/PHASE-2-MEDIA-ENGINE.md` establishes content-addressed media
identity for `cine-media`. The join is a **layering**, not a merge:

```
asset.texture   (EntityId — meaning, provenance, licence, catalogue)
   └─(Contains)→ asset.version 3   (content_hash, dep_closure_hash, status)
        └─(Binds)→ asset.file      (one payload)
             └─(Binds)→ asset.media   (the CAS entry)
                          └─ content-addressed bytes
```

* **`EntityId` answers "what is this?"** — stable across re-encodes, format
  changes, renames, moves.
* **The content hash answers "are these the same bytes?"** — stable across
  re-imports from anywhere.
* **They meet at `asset.file`** — exactly where meaning becomes bytes.

Consequences:

1. **Re-import is idempotent** — `EntityId::from_key` plus a hash check gives
   `group.deltas.is_empty()`, per the `CLAUDE.md` invariant.
2. **The CAS gives version dedup free** — publishing v8 with unchanged bytes
   costs one node and zero bytes.
3. **Moving a file on disk changes nothing** — the CAS entry is the identity of
   the bytes; the path is a resolver detail on `asset.library`. *(This is
   precisely what USD's `Ar` layer does, and it is worth noting CINE-OMNI gets
   it structurally rather than as a plugin.)*
4. **`dep_closure_hash` extends this upward.** §01 G-11 wanted Blender's
   `ID::deep_hash` for exactly this: "has anything anywhere under this asset
   actually changed?" — the gate that keeps the reactor cascade from doing
   needless work, and the memo key for §8.2 rule 6.

### 9.3 Staleness: computed, not propagated

The subtlest decision in §7.5 is that `asset.binding → asset.version` is
`References` (non-propagating), not `DependsOn`.

**Because publishing a new version of a common asset must not dirty every shot
in the film.** If it did, a texture republish would wake the cascade across
thousands of nodes — and §01 §1.3.3 finding (1) already identified
over-propagation at node granularity as the thing that will break the `< 5 ms`
reactor budget at film scale.

Instead:

* A binding pinned to a version is **never** stale. It said what it wanted and
  got it. **The correct default for anything already shot.**
* A binding on `latest` / `latest_approved` is stale when the asset's newest
  (approved) version differs from the `References` target. **Staleness is a
  query**, run on demand and at open, using the `attrs::STALE` /
  `attrs::DIRTY_REASON` keys that already exist in `ucg-schema` for this pattern.
* A reactor may **offer** the update. It must never silently apply it — a
  version bump under a locked shot is a continuity error, the most expensive
  class of mistake in this domain.

This is §01 G-3's "one authoritative database, N evaluated views" applied to
*time* rather than viewers: **the graph stores what was chosen; what is current
is derived.**

### 9.4 Recovering the past

* **"What did the graph look like at op N?"** — op-log replay. Exact,
  whole-project, expensive.
* **"What did Scene 42's assets look like on the shoot day?"** — read the
  bindings' pins. Cheap, local, and the question people actually ask. It works
  *because* pins are recorded rather than resolved dynamically.
* `asset_status` (`approved`/`omitted`/`retired`) adds the editorial layer on
  top without touching history.

---

## 10. Implementation taxonomy — what to build, bridge, import or defer

| Item | Type | Rationale |
|---|---|---|
| **The asset system** (§7–§9) | **A NATIVE** | The UCG already provides identity, dependencies, provenance and an op log. This is naming plus two missing pieces, not a new subsystem. |
| **USD composition *model*** inside the UCG (variant sets, sparse overrides, binding-scoped selection) | **G INSPIRED-REIMPLEMENTATION** | Ideas are not copyrightable; §8 adapts them to `EntityId` anchoring, which is strictly better than both USD's paths and Blender's names. |
| **`openusd` crate** (`.usda`/`.usdc`/`.usdz` + composition) | **B LIBRARY**, behind a cargo feature | MIT, pure Rust, builds here, read CINE-OMNI's own output. §5. |
| **`usd-core`** (real OpenUSD) | **C ENGINE** — fallback + **test oracle** | Runs here today; §1.4's conformance validation. Use as the gate, not the runtime. |
| **Pixar OpenUSD C++ as a linked `-sys` dep** | **H REJECT** | Bespoke licence, CMake/C++ build, destroys the one-command build, and the only extant binding has 150 downloads. §5.3. |
| **Camera-units fix** (§1.4) | **A NATIVE**, do first | One line; currently exporting 100×-wrong optics from the physically-accurate-optics crate. |
| **`EntityId` round-trip carrier** | **A NATIVE — already done** | `cine:entity` verified surviving a real-OpenUSD round trip. §1.4. |
| **glTF import** (`gltf` crate) | **B LIBRARY** | MIT/Apache-2.0, 9.34M downloads, de-facto standard. Natural for reference/proxy geometry. |
| **OpenEXR** (`exr` crate) | **B LIBRARY** | BSD-3-Clause, pure Rust, no `unsafe`, 69.7M downloads. |
| **MaterialX** | **E IMPORT/EXPORT**, deferred | Apache-2.0 and desirable, but no Rust binding. Reachable later via `openusd-schemas`'s `shade` feature. |
| **Alembic** | **E IMPORT/EXPORT**, deferred | BSD-3-Clause and unencumbered, but no Rust support and largely subsumed by USD here. |
| **OpenSubdiv** | **defer** | Only `opensubdiv-petite` (partial, C++ underneath). No current need. |
| **Hydra / render delegates** | **H REJECT** | Redundant against `cine-gpu` (wgpu/WGSL); absent from the Rust crate, which is a feature. |
| **Blender USD bridge** | **D BRIDGE** | Already settled in §01 §1.6. §7's asset model gives it something worth exchanging. |

---

## 11. UNVERIFIED register

| # | Claim | Status | How to close |
|---|---|---|---|
| U-1 | Verbatim LIVRPS expansion quoted from `openusd.org` | **Partially verified** — corroborated by `pxr/usd/usd/docs/valueClips.md:446` (*"the 'Local' (L in LIVRPS)"*, *"the LIVRPS strength ordering"*) and by the `openusd` crate README, but not quoted from the glossary itself | Fetch `openusd.org/release/glossary.html#USDGlossary-LIVRPSStrengthOrdering` |
| U-2 | Exact ratification **date** of AOUSD Core Specification 1.0 | **UNVERIFIED** — ratification itself is verified; the announcement page shows only a 2026-08-11 last-updated date | AOUSD specification page |
| U-3 | AOUSD founding date and founding members | **UNVERIFIED** | `aousd.org` |
| U-4 | That the `openusd` crate's own AOUSD compliance claims hold | **UNVERIFIED** — its README's pass/partial table is reported, not independently re-run. Its *behaviour* on CINE-OMNI's file was MEASURED | Run its test suite against the vendored AOUSD supplemental release |
| U-5 | `openusd` crate's `.usdc`/`.usdz` read/write in practice | **UNVERIFIED** — only `.usda` reading was MEASURED | Round-trip a `.usdc` written by `usd-core` |
| U-6 | Long-term maintenance of a largely single-maintainer 0.x crate | **UNVERIFIED — a judgement, not a fact.** Mitigations: cargo feature gate, MIT forkability, sidecar fallback | Ongoing |
| U-7 | glTF **specification** licence and canonical repo `LICENSE` | **UNVERIFIED** — `KhronosGroup/glTF/main/LICENSE` returned 404. The `gltf` *crate*'s licence (MIT OR Apache-2.0) **is** verified | Locate the spec repo's actual licence file |
| U-8 | Exact glTF extensions supported by the `gltf` crate; Draco support | **UNVERIFIED** — README lists features but was not read in full | Read the crate's full README/docs |
| U-9 | Whether nominative use of "OpenUSD" in CINE-OMNI's docs is permitted given Tomorrow OSTL 1.0 §6 | **UNVERIFIED — out of scope.** A legal question | Counsel |
| U-10 | Whether OpenUSD **C++** actually configures/compiles/links with `zig c++` 0.16 + CMake 4.4.3 here | **UNVERIFIED — NOT ATTEMPTED.** §3.3 gives the command. **Moot under the §5 recommendation** | Run §3.3 after building oneTBB |
| U-11 | OpenUSD C++ build wall-clock and disk footprint | **UNVERIFIED** — deliberately not estimated | Measure during U-10 |
| U-12 | GitHub licence-detector output for the OpenUSD repo | **UNVERIFIED** — the API was rate-limited from this machine. The `LICENSE.txt` itself was read directly, which is stronger | Authenticated API call |
| U-13 | Whether Tomorrow OSTL 1.0 has been submitted for OSI/SPDX listing | **UNVERIFIED** — `usd-core`'s `LicenseRef-TOST-1.0` shows it is **not currently** SPDX-listed | AOUSD forum post linked from `CHANGELOG.md` `[24.08]` |
| U-14 | That Blender's USD importer preserves custom prim metadata (`cine:entity`) round-trip | **UNVERIFIED** — §01 §1.4 flagged this. **Half now closed:** real OpenUSD preserves it (MEASURED, §1.4); Blender specifically is untested | Round-trip against a real Blender install |
| U-15 | Alembic latest release, ASWF status, dependency set | **UNVERIFIED** — the **licence (BSD-3-Clause) is verified** from `LICENSE.txt` | GitHub API |
| U-16 | MaterialX latest release and ASWF status | **UNVERIFIED** — the **licence (Apache-2.0) is verified**; OpenUSD 26.08 builds against 1.39.5 (§2.2) | GitHub API |

**Verified by primary source or direct measurement in this dossier:** OpenUSD's
licence text and its §6 deviation (read from `LICENSE.txt` at `23b83aa8`); the
24.08 rename and date and the 26.08 release and date (`CHANGELOG.md`); CMake
3.27 minimum and C++17 (`CMakeLists.txt`, `CXXDefaults.cmake`); the required
dependency set (`README.md`); Pixar's tested matrix (`VERSIONS.md`); absence of
SPDX tags on OpenUSD's own sources (grep over `pxr/`); the camera-units
convention (`pxr/usd/usdGeom/schema.usda:2368`); AOUSD Core Spec 1.0
ratification and coverage (AOUSD announcement); the full local toolchain
inventory; installation of CMake 4.4.3 + Ninja 1.13.2 via `uv`; `zig c++` 0.16.0
compiling and running C++; **installation and execution of real OpenUSD 26.8
(`usd-core`, `LicenseRef-TOST-1.0`)**; **`cine-previs`'s actual exported layer
opening clean in OpenUSD 26.8 with all prim types and attribute types
conformant, the `cine:entity` carrier surviving, and the camera hyperfocal
distance reading 1461.8 m against an intended 14.6 m**; **the `openusd` crate
building with plain `cargo build` (79 crates, zero `-sys`) and correctly reading
that layer**; the MIT licence of the `openusd` crate, the BSD-3-Clause licence
of Alembic and the Apache-2.0 licence of MaterialX (all read from their
`LICENSE` files); and every crates.io row in §4.1 and §6.
