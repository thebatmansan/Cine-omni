# CINE-OMNI Industry Blueprint — 13: Professional Colour Management

> **What this document settles that `research/10-color-and-sync.md` did not.**
> Doc 10 covers ITU-T H.273 code points, chroma siting, container carriage, the
> "untagged media decodes three different ways" finding, HDR carriage and
> licensing, and the GLSL→WGSL transpilation question. **None of that is
> repeated here.**
>
> This document answers the part doc 10 left open: **ACES and OpenColorIO as
> actual dependencies**, and **what "colour managed" means as an implementable,
> testable contract**. It also corrects three of doc 10's own findings, one of
> which materially changes doc 10's headline recommendation.
>
> Written to the standing rules of `docs/blueprint/README.md`: no licence from
> memory, no approximation called by the name of the standard it approximates,
> no conformance claim that was not measured, and `UNVERIFIED` rather than a
> plausible fill.
>
> **All fetches, clones and builds: 2026-09-08**, on this machine.

---

## 0. Findings summary

**Sixteen findings, in order of consequence. The first two are measurements
run on this machine, not arguments.**

**0. `cine-color` cannot load a single OpenColorIO ACES config — not the ACES
2.0 ones, not the ACES 1.3 ones, none of them.** This was run, not reasoned
about. Pointed at the three configs OpenColorIO itself ships (`cg-config-v4.0.0
_aces-v2.0_ocio-v2.5`, `studio-config-v4.0.0_aces-v2.0_ocio-v2.5`, and the ACES
1.3 `cg-config-v2.2.0`), `Config::load` returns `Err` on the first display
block of all three: *"parse error at line 0: view without name in display `sRGB
- Display`"*. The cause is `shared_views` and the `!<Views>` reference list —
OCIO v2 core features present in every ACES config since 2021. Eight top-level
config keys and three YAML tags are unimplemented. **Everything else in this
document is downstream of that fact**, and it is why §4's verdict on
`tests/ocio_fixture.rs` is as blunt as it is: a test suite that passes while
this is true is testing the wrong thing. §4.0

**0b. The first conformance number this project has ever had: exact on neutrals,
wrong by up to 0.32 on saturated colour.** OpenColorIO 2.6.0dev was built on
this machine (§3.5) and used as an oracle: ACEScg → `sRGB - Display` /
`ACES 2.0 - SDR 100 nits (Rec.709)`, from the real
`cg-config-v4.0.0_aces-v2.0_ocio-v2.5`. The same five inputs through
`cine_color::OutputTransform::for_display(Display::SRgb)`:

| ACEScg in | OpenColorIO (ACES 2.0) | `cine-color` | max &#124;Δ&#124; |
|---|---|---|---:|
| 0.18, 0.18, 0.18 | 0.349189, 0.349189, 0.349189 | **0.349189, 0.349189, 0.349189** | **0.0000** |
| 1, 0, 0 | 0.877157, 0.000000, 0.040252 | 0.896332, 0.000000, 0.000000 | 0.0403 |
| 0, 1, 0 | 0.000000, 0.714443, **0.274862** | 0.000000, 0.749661, **0.000000** | **0.2749** |
| 0, 0, 1 | 0.000000, **0.210544**, 0.646733 | 0.000000, **0.000000**, 0.753226 | 0.2105 |
| 2, 0.5, 0.1 | 1.000000, 0.527745, 0.333898 | 0.960816, 0.522396, 0.030953 | **0.3029** |

**Worst-case max&#124;Δ&#124; = 0.3029 (30.3 % of full scale) per-channel, 0.3223
(32.2 %) in max-RGB hue mode.**

**Read the first row and the third row together — they are the whole story of
this document.** On the achromatic axis the agreement is *exact to six decimal
places*, which is conclusive proof that finding 2 is right: `cine-color` really
does have the ACES 2.0 tonescale, not something like it. On saturated primaries
the disagreement is enormous, and it is enormous in a specific, diagnostic way:
ACES 2 renders pure green as `(0, 0.714, 0.275)` — **there is light in the blue
channel** — because chroma compression and volumetric gamut compression pull the
out-of-display colour inward. `cine-color` renders it `(0, 0.750, 0)` and clips.
That is exactly what "the right tonescale applied per channel in AP1, with no
JMh, no chroma compress and no gamut compress" predicts (finding 3).

**Consequences.** (a) It is now *measured*, not argued, that the current output
transform must not be called ACES. (b) A 0.30 error on a primary is not a
tolerance question — no plausible threshold admits it. (c) The infrastructure to
produce this number took an afternoon and is described in §3.5 and §7; there is
no excuse for the project not having one.

**1. ACES became Apache-2.0 on a specific, now-known date, and it is *after*
ACES 2.0 shipped.** Doc 10 marked this `UNVERIFIED`; it is now closed. The
commit is **`ee380496a201fd008841ff9399b29241bcd27a3a`**, authored by Scott Dyer
(`sdyer@oscars.org`) on **2025-09-17**, committed **2025-09-25**, message
*"change LICENSE and update community health files — change LICENSE to Apache
2.0"*. It **deleted `LICENSE.md` (50 lines) and added `LICENSE` (201 lines)**.
Everything at tag **`v1.3.1` and earlier is under the AMPAS "License Terms for
Academy Color Encoding System Components"**, not Apache-2.0 — verified by
fetching that file and reading it in full (§2.1). **Consequence: ACES 1.x CTL
must not be vendored under an Apache-2.0 notice. Only `main` may be.**

**2. `cine-color`'s tonescale is not "Daniele-style". It is, constant for
constant, the ACES 2.0 tonescale — and that makes the current labelling
*understated* in one place and *overstated* in another.** Every parameter in
`TonescaleParams::for_peak` (`n_r = 100`, `contrast = 1.15`, `gray_in = 0.18`,
`gray_out_nits = 10.013`, `gray_change = 0.14`, `toe = 0.04`,
`r_hit_min = 128`, `r_hit_max = 896`) matches `init_TSParams` in
`aces-core/lib/Lib.Academy.Tonescale.ctl` exactly, and
`TonescaleParams::apply_nits` is `tonescale_fwd` term for term. What is absent
is **everything the tonescale sits inside**. See finding 3.

**3. ACES 2.0's Output Transform is five stages, and CINE-OMNI has one of
them.** Read from `Lib.Academy.OutputTransform.ctl` (1,702 lines):

```
outputTransform_fwd(aces, p):
    clamp_AP0_to_AP1(aces, 0, ts.forward_limit)
    RGB_to_JMh(., input_params)              <- Hellwig-type appearance model
    tonemap_and_compress_fwd(., p)           <- tonescale + CHROMA COMPRESS
    gamut_compress_fwd(., p)                 <- volumetric, table-driven
    JMh_to_RGB(., limit_params)
```

CINE-OMNI implements the tonescale and applies it **per channel or on max(RGB)
in AP1**. It has **no JMh appearance space, no chroma compression and no gamut
compression**. Those are the parts that make ACES 2 hue-preserving. **A correct
tonescale in the wrong space is not the ACES 2 rendering**, and the difference
is large in saturated highlights — precisely where ACES 2 was designed to
differ from ACES 1.

**4. `Op::Custom` is used — three times — and that breaks doc 10's headline
recommendation as written.** Doc 10 §A.5.3/R3 states `Op::Custom` "appears
**zero times** outside `processor.rs`" and concludes that a WGSL emitter is "a
ten-arm match, not a compiler". The grep it relied on was for the literal string
`Op::Custom`; the variant is actually constructed through the helper
`Op::custom_invertible`, at **`ocio.rs:1813`** (the HLG 1000-nit display
builtin), **`aces.rs:467`** (`OutputTransform::processor`) and **`aces.rs:483`**
(`OutputTransform::to_xyz_d65_op`). So **the entire ACES output transform is
today a `Custom` op**: not serialisable, not emittable to WGSL by a match over
the op set, and invisible to any optimiser. The emitter plan is still right; the
premise needs fixing first (§6.2).

**5. `tests/ocio_fixture.rs` is a parser regression test, not an OCIO
compatibility test, and nothing in the tree makes it one.** The fixture,
`tests/fixtures/mini.ocio`, was **written by this project** — 5,485 bytes,
committed alongside the parser. Its numeric assertions compare `cine-color`
against `cine-color` (`want = cine_color::space::rgb_to_rgb_matrix(...)`),
against arithmetic identities (`0.5^2.2`), or against loose sanity bands
(`assert!(toned[0] > 0.3 && toned[0] < 0.5)`). **No real OCIO config is loaded
anywhere in the workspace, and OpenColorIO is never run.** It demonstrates that
the parser is self-consistent and that round-tripping YAML is lossless. It
demonstrates **nothing whatsoever about agreement with OpenColorIO**. Verdict:
**it resembles OCIO compatibility; it does not constitute it** (§4).

**6. Worse than absent compatibility: the crate resolves real OCIO builtin
style *names* to a different transform, silently.** `builtin_ops` in `ocio.rs`
matches `"ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0"` — a genuine
OpenColorIO built-in style string — and returns CINE-OMNI's parametric
`OutputTransform` instead. A real config naming that style loads without error
and renders **different pixels**. This is the exact failure mode doc 10 warned
about for transpiled shaders ("silently wrong colours rather than a compile
error"), and it is already present in the CPU path. It also **contradicts the
honesty property doc 10 praises** (§A.6.3: "unsupported styles produce a named
error, not a silent identity") — that property holds only for styles the crate
does not recognise, not for styles it recognises and approximates.

**6a. Six of the crate's 24 OCIO builtin style names do not exist in
OpenColorIO.** Measured, not estimated. OCIO's authoritative registry —
extracted from `src/OpenColorIO/transforms/builtins/*.cpp` on `main`, including
the ACES 2.0 family registered from a table in a loop — is **97 styles**.
`cine-color` recognises 24. **18 match exactly. Six are invented and can never
match a real config:**

| `cine-color` says | OpenColorIO actually says |
|---|---|
| `SONY_SLOG3_SGAMUT3.CINE_to_ACES2065-1` | `SONY_SLOG3-SGAMUT3.CINE_to_ACES2065-1` |
| `SONY_SLOG3_SGAMUT3_to_ACES2065-1` | `SONY_SLOG3-SGAMUT3_to_ACES2065-1` |
| `RED_LOG3G10_RWG_to_ACES2065-1` | `RED_LOG3G10-RWG_to_ACES2065-1` |
| `DISPLAY - CIE-XYZ-D65_to_REC.709` | *(no such style)* — OCIO has `…_to_REC.1886-REC.709` |
| `DISPLAY - CIE-XYZ-D65_to_REC.2020` | *(no such style)* — OCIO has `…_to_REC.1886-REC.2020` |
| `DISPLAY - CIE-XYZ-D65_to_DISPLAY-P3` | `DISPLAY - CIE-XYZ-D65_to_DisplayP3` |

Three are an underscore where OCIO uses a hyphen; two name a style OCIO has
never had; one is a capitalisation difference. **The project's own fixture
`tests/fixtures/mini.ocio` uses one of the invented names** — so the fixture is
not a valid OpenColorIO config and real OCIO would reject it. Coverage against
the real registry is **18 / 66 = 27 %**. This is the single hardest piece of
evidence for the §4 verdict, and it is the kind of error that only a real
config or a real OCIO can catch.

**6b. Even the correctly-named display styles diverge semantically, at the
range boundaries.** `cine-color`'s `display()` helper inserts
`Op::clamp(0.0, 1.0)` before every display encode. OCIO does not clamp there:
`DISPLAY - CIE-XYZ-D65_to_sRGB` is a matrix (`ADAPTATION_NONE`) plus
`GammaOpData::MONCURVE_REV` with params `{2.4, 0.055}`, whose CPU renderer
(`GammaMoncurveOpCPURev::apply`) has **no clamp of any kind** — below the break
point it is linear (`x * slope`, so negatives pass through scaled), above it is
`pow(x, g) * scale − offset` with no upper bound. The gamma-2.2 family uses
`GammaBasicOpCPU::apply`, which does `std::max(0.0f, x)` — **negatives clamped,
superwhites not**. So CINE-OMNI clips highlights that OpenColorIO preserves.
Doc 10 §A.2.2 states the rule this violates: *"superblacks and superwhites must
be preserved, not clamped."*

**7. OpenColorIO is BSD-3-Clause and `main` is 2.6.0-dev.** Read from the
cloned `LICENSE` in full (§3.1). `cmake_minimum_required(VERSION 3.14...4.0)`,
default `CMAKE_CXX_STANDARD 17`.

**7a. OCIO's shader generator already emits separated texture/sampler bindings
with explicit set/binding indices — for HLSL and Metal. A WGSL backend is a
case arm, not an architecture change.** This is new information that doc 10 did
not have, and it revises its Option-2 cost estimate downward. Read from
`src/OpenColorIO/GpuShaderUtils.cpp`, `getTexDecl<N>`:

```cpp
case GPU_LANGUAGE_GLSL_VK_4_6:                 // the wall doc 10 found
    textureDecl = "";
    kw << "layout(set=" << descriptorSetIndex << ", binding = " << textureIndex << ") ";
    kw << "uniform sampler" << N << "D " << samplerName << "; ";   // COMBINED

case GPU_LANGUAGE_HLSL_SM_5_0:                 // SEPARATED
    textureDecl = "Texture" + N + "D " + textureName + ";";
    samplerDecl = "SamplerState "               + samplerName + ";";

case GPU_LANGUAGE_MSL_2_0:                     // SEPARATED
    textureDecl = "texture" + N + "d<float> "   + textureName + ";";
    samplerDecl = "sampler "                    + samplerName + ";";
```

The function signature already carries **both** a `textureName` and a
`samplerName`, and already threads `descriptorSetIndex` and `textureIndex` —
which is exactly WGSL's `@group(N) @binding(M)` model. `GpuShaderText` is a
per-primitive abstraction layer (`float3GreaterThanEqual`, `atan2`, `sign`,
`castToBool`, keyword and constant emitters, `declareTexND`, `sampleTexND`),
each a `switch (m_lang)`. And `GpuShaderCreator` is an **abstract base class**
whose `addTexture`, `add3DTexture`, `addUniform`, `addToHelperShaderCode`,
`addToFunctionShaderCode` and `createShaderText` are all `virtual` — so an
application subclasses it rather than patching it.

**Doc 10's verdict stands where it was aimed** — transpiling OCIO's *GLSL
output* to WGSL is blocked, for the reasons it documents. **But "write a WGSL
backend" is a much smaller job than doc 10 assumed**, because the
separated-sampler plumbing it identified as missing is already there and
already shipping in two backends. This does not change the recommendation
(§7) — CINE-OMNI still should not put a C++ tree in its build — but it does
raise the value of proposing the backend upstream (rejected alternative D).

**8. This machine has no C/C++ toolchain in `PATH` and no build system — but
both are obtainable, and were.** `cc`, `gcc`, `clang`, `make`, `cmake`,
`pkg-config`, `nasm` and `ninja` are all **absent**. What exists is
**zig 0.16.0** at `~/.local/zig/zig`, wrapped as `~/.local/bin/zigcc` /
`zigcxx` (targeting `x86_64-linux-gnu.2.39`) and already wired into
`~/.cargo/config.toml` as the Rust linker and as `CC`/`CXX`. **Verified by
compiling and running a C++17 program using `std::string` through `zigcxx`.**
CMake 4.4.3 and Ninja 1.13.2 were then **installed via `uv tool install`** and
work. So the environment is *not* a priori blocking. Whether OCIO itself builds
is answered in §3.4.

**9. `cine-color` reads exactly one LUT format: `.cube`.** No CLF, no CTF, no
`.spi1d`/`.spi3d`, no `.3dl`, no `.csp`. **CLF is the format ACES itself uses
for interchange**, so this is the largest interoperability gap after the config
subset.

**10. `ColorMeta` and the whole crate have no place for a *reference* to
compare against.** There is no conformance harness, no reference image, no
golden-value table beyond hand-written constants. Doc 10's finding that ACES
publishes reference images but **no tolerance** stands; the consequence is that
CINE-OMNI must publish its own metric, and it currently publishes none.

**11. The honest floor for "colour managed" is four properties, and CINE-OMNI
meets two.** They are stated as a testable contract in §5.

**12. The right dependency answer is neither "link OCIO" nor "port ACES" — it
is both, in different roles.** Port `aces-core` (Apache-2.0, 2,858 lines of CTL)
into Rust as the *runtime*; use a real OpenColorIO build as an
*off-by-default, developer-only, CI-side oracle* that produces the evidence for
every compatibility claim. Full reasoning and rejected alternatives in §7.

---

## 1. What `crates/cine-color/` actually is — a frank assessment

**Size:** 9,890 lines across 14 modules plus 2 test files.

```
ocio.rs      2097    grade.rs     1012    space.rs      793    aces.rs       780
yaml.rs       771    scopes.rs     640    mask.rs       640    lut.rs        539
transfer.rs   581    processor.rs  853    curve.rs      354    ucg.rs        336
error.rs       68    lib.rs         80
tests/ocio_fixture.rs  179    tests/pipeline.rs  167
```

Dependencies: `ucg-core`, `ucg-schema`, `serde`, `serde_json`, `thiserror`,
`indexmap`, `rayon`. **No C or C++ dependency of any kind.** That is a real
asset and the reason this crate builds anywhere.

### 1.1 What is genuinely good

* **The colorimetry is computed, not copied.** `space.rs` derives RGB↔XYZ from
  chromaticities and computes chromatic adaptation, with published matrices
  committed *separately* as `AP0_TO_AP1_REF` / `AP1_TO_AP0_REF` constants so the
  computed and published forms can be cross-checked. That is the correct
  structure: most implementations paste a matrix and lose the ability to check
  it.
* **The transfer functions are written from the fractional constants**, not
  from rounded decimals, and HLG includes the OOTF and display-dependent system
  gamma. Doc 10 §A.3.1 covers this and it is not repeated.
* **The op model is the right one.** A flat, ordered, invertible list of
  well-typed per-pixel ops (`Processor`/`Op`) is exactly what a colour engine
  should compile to, and it is what makes a GPU emitter possible at all.
* **Unknown OCIO builtin styles raise `Error::Unsupported` naming the style.**
  For styles the crate has never heard of, this is correct and rare.
* **YAML round-trips losslessly** — `Config::parse(cfg.to_yaml()) == cfg` is
  asserted.

### 1.2 What it is not

* **It is not OpenColorIO, and it is not a reimplementation of OpenColorIO.**
  It is an independent parser for a subset of OCIO's *file format*, plus an
  independent evaluator. Nothing in the tree has ever compared its output to
  OpenColorIO's.
* **It is not ACES.** It implements ACES *colorimetry* (AP0/AP1, the encodings,
  the matrices) and the ACES 2.0 *tonescale*, inside a rendering that is not
  ACES's.
* **It has no reference data.** Every tolerance in the test suite is a
  self-comparison, an algebraic identity, or a hand-chosen band.

### 1.3 The `Op::Custom` problem, precisely

```
crates/cine-color/src/ocio.rs:1813   Op::custom_invertible("HLG 1000nit inverse EOTF", ...)
crates/cine-color/src/aces.rs:467    Op::custom_invertible("ACES output {display}", ...)
crates/cine-color/src/aces.rs:483    Op::custom_invertible("ACES tonescale {display}", ...)
```

`Op::Custom { name, f: PixelFn, inv: Option<PixelFn> }` holds boxed closures.
It cannot be serialised, cannot be hashed into a cache key by value, cannot be
emitted as WGSL, and cannot be optimised through. **Every view transform in the
system currently ends in one.** Doc 10's WGSL-emitter recommendation is sound in
principle but cannot be implemented until the output transform is expressed in
the op vocabulary — which is the same work as porting ACES 2 properly (§7).

### 1.4 The silent-substitution defect

This is the single most serious integrity problem in the crate, and it is worth
being exact about.

`ocio.rs::builtin_ops` recognises 24 style strings. For most of them the mapping
is defensible: `ACEScg_to_ACES2065-1` really is a matrix, and
`ARRI_LOGC4_to_ACES2065-1` really is a curve plus a matrix. But for these four:

```
"ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0"
"ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3lim_1.1"
"ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-REC2020lim_1.1"
"ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-P3lim_1.1"
```

the crate returns `OutputTransform::for_display(...)` — a per-channel tonescale
with no chroma or gamut compression. The style name promises the ACES output
transform of a named version. The code delivers something else, **with no
diagnostic**.

**This must be fixed before anything else in this document.** The fix is cheap
and does not require porting anything: either raise `Error::Unsupported`, or
resolve the style but attach a **`Provenance::Approximation { of, reason }`**
marker that the processor carries and the UI must display. The second is better,
because a preview approximation is genuinely useful — it just must never be
anonymous. See §5, contract property **C3**.

---

## 2. ACES as a dependency

### 2.1 Licence — closed, with a date

Doc 10 established that `aces-aswf/aces-core` `main` is stock Apache-2.0 and
marked the pre-2.0 tags `UNVERIFIED`. **Closed here.**

**Repository metadata** (GitHub API, 2026-09-08):
`full_name: aces-aswf/aces-core`, `license.spdx_id: Apache-2.0`,
`created_at: 2011-08-04`, `pushed_at: 2026-09-01`, `stargazers_count: 857`,
`default_branch: main`. **The releases API returns `[]`** — there are no GitHub
releases. The newest tag is **`v1.3.1`** (`fc189808…`), then `v1.3`
(`1256fee5…`), `v1.3_rc2`, `v1.3_rc1`, `v1.2`, `v1.2_rc2`, `v1.2_rc1`, `v1.1`,
`v1.0.3`, `v1.0.3_beta2`. **ACES 2.0 exists only as commits on `main`; anything
vendoring it must pin a commit SHA** (doc 10 said this; it is re-confirmed).

**The licence-change commit**, from a full clone of the repository:

```
commit ee380496a201fd008841ff9399b29241bcd27a3a
Author: Scott Dyer <sdyer@oscars.org>
Date:   Wed Sep 17 10:49:40 2025 -0700   (committed 2025-09-25 21:17:53 -0700)

    change LICENSE and update community health files

    - change LICENSE to Apache 2.0
    - add SPDX identifiers
    ...
 LICENSE      | 201 +++++++++++++++++++++++++++++++++++
 LICENSE.md   |  50 ---------
```

Prior commits touching the licence file: `339318f` (2013-07-26), `9f83dda`
(2012-02-28, *"Updated License Language"*), `1403f9c` (2011-08-04, initial),
and `09690b6` (2015-09-04).

**What the pre-2.0 licence actually says** — fetched verbatim from
`raw.githubusercontent.com/aces-aswf/aces-core/v1.3.1/LICENSE.md` (49 lines):

> **License Terms for Academy Color Encoding System Components**
>
> Academy Color Encoding System (ACES) software and tools are provided by the
> Academy under the following terms and conditions: A worldwide, royalty-free,
> non-exclusive right to copy, modify, create derivatives, and use, in source
> and binary forms, is hereby granted, subject to acceptance of this license.
>
> Copyright © 2015 Academy of Motion Picture Arts and Sciences (A.M.P.A.S.).
> …
> * Nothing in this license shall be deemed to grant any rights to trademarks,
> copyrights, patents, trade secrets or any other intellectual property of
> A.M.P.A.S. or any contributors, except as expressly stated herein.
>
> * Neither the name "A.M.P.A.S." nor the name of any other contributors to
> this software may be used to endorse or promote products derivative of or
> based on this software without express prior written permission …
>
> This license shall be construed pursuant to the laws of the State of
> California …
>
> WITHOUT LIMITING THE GENERALITY OF THE FOREGOING, THE ACADEMY SPECIFICALLY
> DISCLAIMS ANY REPRESENTATIONS OR WARRANTIES WHATSOEVER RELATED TO PATENT OR
> OTHER INTELLECTUAL PROPERTY RIGHTS IN THE ACADEMY COLOR ENCODING SYSTEM, OR
> APPLICATIONS THEREOF, HELD BY PARTIES OTHER THAN A.M.P.A.S., WHETHER
> DISCLOSED OR UNDISCLOSED.

This is **not** Apache-2.0 and **not** BSD-3-Clause. It is a bespoke permissive
licence with a no-endorsement clause, a California choice-of-law clause, an
explicit refusal to grant patent rights, and an explicit disclaimer about
third-party patents in ACES. It is probably compatible with shipping in an
Apache-2.0 product **as a separately-licensed vendored component**, but it is
**not** the same licence and must not be represented as such.

**Rules that follow:**

1. **Port only from `main`.** Never from a `v1.x` tag.
2. **Pin the commit SHA** of `main` used, in the source file that carries the
   port, and record it in `NOTICE`.
3. **The Apache-2.0 §4 attribution obligations apply.** Retain the ACES
   copyright and the `SPDX-License-Identifier: Apache-2.0` header on ported
   files, and state in `NOTICE` that files derive from `aces-aswf/aces-core`.
   The CTL files carry `// SPDX-License-Identifier: Apache-2.0` and
   `// Copyright Contributors to the ACES Project.` at the top — verified.
4. **Apache-2.0 §3 grants a patent licence** from the ACES contributors. The
   old AMPAS terms explicitly did not. That is a substantive improvement, not a
   cosmetic one, and it is the strongest single argument for porting `main`
   rather than reimplementing from a paper.

### 2.2 What `aces-core` actually contains

Cloned `main` (2026-09-08). The whole repository is **5 CTL files, 2,858 lines**:

| File | Lines | Contents |
|---|---:|---|
| `lib/Lib.Academy.OutputTransform.ctl` | **1,702** | JMh appearance model, chroma compress, gamut compress, cusp/reach/gamma table construction, `outputTransform_fwd`/`_inv` |
| `lib/Lib.Academy.DisplayEncoding.ctl` | 501 | moncurve, BT.1886, SMPTE↔full range, ST 2084, HLG↔PQ, `eotf`/`eotf_inv`, `display_encoding`/`display_decoding` |
| `lib/Lib.Academy.Utilities.ctl` | 435 | maths helpers |
| `lib/Lib.Academy.ColorSpaces.ctl` | 124 | XYZ↔xyY, Bradford and CAT02 cone matrices, `calculate_cat_matrix` |
| `lib/Lib.Academy.Tonescale.ctl` | 96 | `TSParams`, `init_TSParams`, `tonescale_fwd`, `tonescale_inv` |

**This is a small, bounded body of code.** Roughly 2,900 lines of straight-line
scalar maths with no I/O, no allocation and no external dependencies. Porting it
to Rust is a weeks-not-months task for one person, and it is the *whole* of the
ACES 2 rendering.

### 2.3 The ACES 2.0 pipeline, read from the source

```c
float[3] outputTransform_fwd(float aces[3], ODTParams p)
{
    float AP0_clamped[3]   = clamp_AP0_to_AP1(aces, 0., p.ts.forward_limit);
    float JMh[3]           = RGB_to_JMh(AP0_clamped, p.input_params);
    float tonemappedJMh[3] = tonemap_and_compress_fwd(JMh, p);
    float compressedJMh[3] = gamut_compress_fwd(tonemappedJMh, p);
    float RGBout[3]        = JMh_to_RGB(compressedJMh, p.limit_params);
    return RGBout;
}
```

and the inverse is the exact mirror (`RGB_to_JMh(limit)` →
`gamut_compress_inv` → `tonemap_and_compress_inv` → `JMh_to_RGB(input)`). Note
that **the display encoding is not in this function** — it lives in
`Lib.Academy.DisplayEncoding.ctl`. Doc 10 §A.6.4 quotes ACES's implementer notes
observing that the rendering and the display encoding "may be desirable to
maintain [as] two steps"; **the reference source is already split that way**,
which settles the design question: one shared rendering pass, one cheap
per-display encode.

**`init_ODTParams(peakLuminance, limitingPrimaries)` is where the cost is.** It
builds, per output configuration:

* `p.input_params`, `p.reach_params`, `p.limit_params` — three `JMhParams`
  structures (`init_JMhParams(AP0)`, `init_JMhParams(REACH_PRI = AP1)`,
  `init_JMhParams(limitingPrimaries)`)
* `p.ts = init_TSParams(peakLuminance)`
* `p.TABLE_reach_M` — `make_reach_m_table(...)`
* `p.TABLE_gamut_cusps` — `make_uniform_hue_gamut_table(...)`, 3-wide
* `p.TABLE_hues`
* `p.TABLE_upper_hull_gamma` — `make_upper_hull_gamma_table(...)`, built by an
  **iterative search** (`gamma_minimum = 0.0`, `gamma_maximum = 5.0`,
  `gamma_search_step = 0.4`, `gamma_accuracy = 1e-5`)
* `p.hue_linearity_search_range`

Table geometry, read from the constants: `tableSize = 360`,
`additionalTableEntries = 2` ("allots for extra entries to wrap the hues"),
`totalTableSize = 362`, `baseIndex = 1`, `hue_limit = 360.`,
`cuspCornerCount = 6`, `totalCornerCount = 8`, `max_sorted_corners = 12`.

**This is the single most important structural fact for a GPU implementation.**
The expensive part of ACES 2 is a **one-time, per-(peak-luminance, limiting-
primaries) initialisation producing four small tables of 362 entries**. The
per-pixel work is then table lookups plus closed-form maths. So:

* **`cine-color` (CPU, Rust)** owns `init_ODTParams` and the tables. It is
  exact, `f64`, testable against CTL, and runs once when the view transform
  changes.
* **`cine-gpu` (WGSL)** owns `outputTransform_fwd`'s per-pixel body, with the
  four tables uploaded as a storage buffer or a 1-D texture.

That mapping is not a guess; it falls out of the reference implementation's own
shape. See §8.

**Appearance-model constants**, read verbatim from the CTL (they are the
parameters CINE-OMNI would have to reproduce, and they are *not* the standard
CIECAM02 defaults):

```
ref_luminance = 100.       L_A = 100.       Y_b = 20.
surround[3] = {0.9, 0.59, 0.9}   // "Dim surround"
J_scale = 100.0
cam_nl_Y_reference = 100.0
cam_nl_offset = 0.2713 * cam_nl_Y_reference
cam_nl_scale  = 4.0 * cam_nl_Y_reference
model_gamma = surround[1] * (1.48 + sqrt(Y_b / ref_luminance))
chroma_compress = 2.4    chroma_compress_fact = 3.3
chroma_expand   = 1.3    chroma_expand_fact   = 0.69   chroma_expand_thr = 0.5
smooth_cusps = 0.12      smooth_m = 0.27      cusp_mid_blend = 1.3
```

and the peak-luminance-dependent derivations:

```
sat  = max(0.2, chroma_expand - (chroma_expand * chroma_expand_fact) * ts.log_peak)
compr = chroma_compress + (chroma_compress * chroma_compress_fact) * ts.log_peak
chroma_compress_scale = pow(0.03379 * peakLuminance, 0.30596) - 0.45135
lower_hull_gamma = 1.14 + 0.07 * ts.log_peak
```

### 2.4 The tonescale: what CINE-OMNI already has, exactly

`aces-core/lib/Lib.Academy.Tonescale.ctl`:

```c
const float n_r = 100.0;      // normalized white in nits
const float g   = 1.15;       // surround / contrast
const float c   = 0.18;       // anchor for 18% grey
const float c_d = 10.013;     // output luminance of 18% grey (in nits)
const float w_g = 0.14;       // change in grey between different peak luminance
const float t_1 = 0.04;       // shadow toe or flare/glare compensation
const float r_hit_min = 128.; // "hitting the roof" for SDR (n = 100 nits)
const float r_hit_max = 896.; // "hitting the roof" for n = 10000 nits

float tonescale_fwd(float x, TSParams params) {
    float f = params.m_2 * pow(max(0.0, x) / (x + params.s_2), params.g);
    float h = max(0., f * f / (f + params.t_1));      // forward flare
    return h * params.n_r;                            // luminance in cd/m^2
}
```

`crates/cine-color/src/aces.rs`:

```rust
TonescaleParams { peak_nits, n_r: 100.0, contrast: 1.15, gray_in: 0.18,
                  gray_out_nits: 10.013, gray_change: 0.14, toe: 0.04,
                  r_hit_min: 128.0, r_hit_max: 896.0 }

pub fn apply_nits(&self, x: f64) -> f64 {
    let (m_2, s_2) = self.constants();
    let x = x.max(0.0);
    let f = m_2 * (x / (x + s_2)).powf(self.contrast);
    let h = (f * f / (f + self.toe)).max(0.0);
    (h * self.n_r).min(self.peak_nits)
}
```

and `constants()` reproduces `init_TSParams`'s derivation of `r_hit`, `m_0`,
`m_1`, `u`, `m`, `w_i`, `c_t`, `g_ip`, `g_ipp2`, `w_2`, `s_2`, `u_2`, `m_2`
step for step, with the same names.

**And the agreement is exact where it should be.** Measured against
OpenColorIO's ACES 2.0 output transform, 18 % grey through
`OutputTransform::for_display(Display::SRgb)` gives **0.349189, 0.349189,
0.349189** against OCIO's **0.349189, 0.349189, 0.349189** — identical to six
decimal places (finding 0b). On the achromatic axis, chroma compression and
gamut compression are no-ops, so what remains is the tonescale and the display
encode; both match. That is as strong a confirmation as this document can offer
that the curve is the real one.

**Two deltas, both worth recording:**

1. CINE-OMNI applies `.min(self.peak_nits)` inside `apply_nits`. ACES's
   `tonescale_fwd` does not clamp; the clamp to `[0, peakLuminance]` happens
   elsewhere in the ODT (the CHANGELOG for Developer Release 2 records *"Add a
   clamp to range [0 - peakLuminance] and remove explicit clamp in inverse EOTF
   function"*). Whether the placement is equivalent is **UNVERIFIED**.
2. CINE-OMNI's `inverse_nits` derives its own analytic inverse; ACES's
   `tonescale_inv` uses
   `Z = clamp(Y, 0, n/(u_2*n_r)); h = (Z + sqrt(Z*(4*t_1+Z)))/2; f = s_2/((m_2/h)^(1/g) - 1)`.
   These should be algebraically identical, but they are written differently and
   **have not been compared numerically** — `UNVERIFIED`, and a trivially cheap
   test to add.

**The docs must be corrected.** `aces.rs`'s module comment currently calls the
curve *"a Daniele-style sigmoid tonescale (the curve family used by ACES 2.0)"*
and `TonescaleParams` is documented as *"Daniele Siragusano's 'Evo' curve"*.
That understates it — with these constants it is **the ACES 2.0 tonescale**, not
a member of its family. Conversely the *transform* claim is overstated: it is
the ACES 2.0 tonescale in the wrong space with no chroma or gamut compression.
Saying both plainly is more accurate than the current text in both directions.

### 2.5 What would have to be true to claim "ACES support" — the exact list

This is the question the directive asks. Answered as a checklist, each item
independently verifiable, in the order they gate each other.

**Tier 0 — "implements ACES colorimetry".** *Claimable today, with wording.*

- [x] AP0 and AP1 primaries as published chromaticities, and the derived
      matrices, cross-checked against the published reference matrices.
- [x] ACES2065-1, ACEScg, ACEScc, ACEScct encodings and the conversions
      between them.
- [x] **AP0/AP1 chromaticities cross-checked against `aces-core`, and they
      match exactly.** The CTL declares, under the comment *"Academy Primaries 0
      (i.e. 'ACES' Primaries from SMPTE ST2065-1)"*:
      AP0 `R (0.73470, 0.26530)`, `G (0.00000, 1.00000)`,
      `B (0.00010, −0.07700)`, `W (0.32168, 0.33767)`; AP1
      `R (0.713, 0.293)`, `G (0.165, 0.830)`, `B (0.128, 0.044)`, same white.
      `space.rs`'s `Primaries::ACES_AP0` / `ACES_AP1` / `WHITE_ACES` are
      identical to the digit. *ST 2065-1 itself was not fetched* —
      `UNVERIFIED` at the standards level, but now corroborated by a second
      independent primary source that cites it.
- [x] **ACEScct constants cross-checked against OpenColorIO, and they match.**
      `transfer.rs` has `CCT_X_BRK = 0.0078125`, `CCT_A = 10.5402377416545`,
      `CCT_B = 0.0729055341958355`, `CCT_Y_BRK = 0.155251141552511`, with the
      log branch `(log2(x) + 9.72) / 17.52`. OCIO's
      `ACEScct_to_LINEAR` namespace declares `linSideBreak = 0.0078125`,
      `logSideSlope = 1./17.52`, `logSideOffset = 9.72/17.52`,
      `linSideSlope = 1.`, `linSideOffset = 0.`, `base = 2.` — the same curve,
      with OCIO deriving the linear segment where CINE-OMNI hard-codes it.
      *Academy S-2016-001 itself was not fetched* — `UNVERIFIED` at the
      specification level, corroborated by a second implementation.

*Permitted claim:* **"ACES colorimetry: AP0/AP1 primaries and the ACES2065-1 /
ACEScg / ACEScc / ACEScct encodings."** Not "ACES support".

**Tier 1 — "implements the ACES 2.0 Output Transform".** *Not claimable; this
is the porting work.*

- [ ] `RGB_to_JMh` / `JMh_to_RGB` with `init_JMhParams`, matching the CTL's
      non-standard appearance-model constants (§2.3).
- [ ] `chroma_compress_fwd` / `_inv` including `chroma_compress_norm`,
      `reinhard_remap`, `toe`.
- [ ] `gamut_compress_fwd` / `_inv` including `find_gamut_boundary_intersection`,
      `solve_J_intersect`, `remap_M`, `get_focus_gain`,
      `hueDependentUpperHullGamma`.
- [ ] `init_ODTParams` table construction: `make_reach_m_table`,
      `make_uniform_hue_gamut_table`, `make_upper_hull_gamma_table`,
      `build_cusp_table`, `build_hue_table`, `determine_hue_linearity_search_range`
      — 362-entry tables, with the iterative gamma fit.
- [ ] `Lib.Academy.DisplayEncoding.ctl`: `eotf` / `eotf_inv` for every supported
      display, `apply_white_scale`, SMPTE↔full range.
- [ ] The `aces-output` presets, which supply the per-display
      `(peakLuminance, limitingPrimaries, eotf, …)` parameter sets.
- [ ] `Op::Custom` eliminated: the transform expressed as ops (or as a first-
      class `Op::AcesOutput { params }`) so it serialises, caches and emits.

**Tier 2 — "verified against the ACES reference".** *The claim that actually
means something — and note the first measurement already exists (finding 0b):
exact on neutrals, **0.3029** worst-case on saturated primaries. That number is
the baseline every step below must improve on.*

- [ ] Run the `aces-output` four-step procedure (forward on ACES EXRs → compare
      to golden TIFFs; inverse → compare to golden EXRs; re-forward → round
      trip).
- [ ] **Publish CINE-OMNI's own metric and threshold**, and state that it is
      self-defined, because ACES publishes none (doc 10 §A.6.4, a verified
      absence).
- [ ] Mirror the reference images into a location CI can depend on.
- [ ] Publish the result — including the failures — per display/EOTF/nit
      combination, not as a single pass/fail.

*Permitted claim after Tier 2:* **"Implements the ACES 2.0 Output Transform,
ported from `aces-aswf/aces-core` at commit `<sha>`; agrees with the ACES
reference images to within `<metric> ≤ <threshold>` across `<N>` of `<M>`
tested output configurations. This is CINE-OMNI's own criterion; ACES publishes
no tolerance."**

**Tier 3 — "ACES certified / ACES Logo".** *Out of reach and should stay there
for now — and note it has a hard technical prerequisite this document
uncovered:* OpenColorIO's own source states that *"Support for CLF is a
requirement in order to obtain ACES Logo Certification from the Academy (in
several product categories)"* (§11.1). CINE-OMNI has no CLF support at all.* Doc 10 §A.6.4 established the Logo Program is a gated application
requiring acceptance **before** implementation. Nothing in Tier 2 entitles a
product to the logo, and Tier 2 is the honest engineering claim.

**Never claimable at any tier without the corresponding work:**

* "ACES-compatible" — a compatibility claim needs a compatibility measurement.
* "ACES 1.x RRT + ODT" for the Hill fit without the word *approximation* — and
  note ACES 2 **has no RRT and no ODT**; the terms are ACES 1.x's.
* Anything at all for the per-channel/max-RGB `OutputTransform` beyond
  *"parametric display transform using the ACES 2.0 tonescale curve; not the
  ACES 2.0 rendering"*.

### 2.6 There is a *second* Apache-compatible reference implementation, and it is already GPU-shaped

This was not known when doc 10 was written and it materially improves the
porting plan.

**OpenColorIO contains a complete C++ implementation of the ACES 2.0 Output
Transform, under BSD-3-Clause**, at
`src/OpenColorIO/ops/fixedfunction/ACES2/`:

| File | Lines |
|---|---:|
| `Transform.cpp` | 1,406 |
| `Common.h` | 215 |
| `MatrixLib.h` | 116 |
| `ColorLib.h` | 47 |
| `Transform.h` | 48 |
| **total** | **1,832** |

exposed as `FixedFunctionOpData` styles:

```cpp
ACES_OUTPUT_TRANSFORM_20_FWD,   // ACES2 Output transform
ACES_OUTPUT_TRANSFORM_20_INV,
ACES_RGB_TO_JMh_20,             // ACES2 RGB to JMh
ACES_JMh_TO_RGB_20,
ACES_TONESCALE_COMPRESS_20_FWD, // ACES2 Tonescale and chroma compression
ACES_TONESCALE_COMPRESS_20_INV,
ACES_GAMUT_COMPRESS_20_FWD,     // ACES2 Gamut compression
ACES_GAMUT_COMPRESS_20_INV,
```

and **`FixedFunctionOpGPU.cpp` emits the whole thing as shader text.** That
file already solves, in shipping code, every problem a WGSL port has to solve.
Two details taken directly from it:

* **The tables are CPU-built and uploaded as textures with
  `INTERP_NEAREST`**, with interpolation done in the shader — the same decision
  OCIO makes for 3D LUTs and the same one `cine-gpu`'s `lut3d.wgsl` arrived at
  independently. `_Add_Reach_table` calls `shaderCreator->addTexture(name,
  samplerName, table.total_size, 1, TEXTURE_RED_CHANNEL, dimensions,
  INTERP_NEAREST, &table[0])`, falling back from `TEXTURE_1D` to `TEXTURE_2D`
  where 1-D textures are unavailable — which is exactly WGSL's situation, since
  WGSL has no `texture_1d` sampling with a sampler in the general case.
* **`Table1D` and `Table3D` are `std::array`s of `TableBase::total_size`** with
  `base_index`, `first_nominal_index` and `upper_wrap_index` constants — the
  same 360-plus-wrap geometry as the CTL, so the two references agree
  structurally.

**Consequence for the port.** CINE-OMNI now has *two* permissively-licensed
references that can be cross-checked against each other:

| | `aces-aswf/aces-core` CTL | OCIO `ops/fixedfunction/ACES2/` |
|---|---|---|
| Licence | **Apache-2.0** (since 2025-09-25) | **BSD-3-Clause** |
| Size | 2,858 lines CTL | 1,832 lines C++ |
| Optimised? | No — reference clarity | **Yes** — `aces-core`'s own CHANGELOG says v2.0 was *"Refactor[ed] to parallel the optimizations made by OCIO (included in their 2.4.2 release)"* |
| GPU form? | No | **Yes** — `FixedFunctionOpGPU.cpp` |
| Role for CINE-OMNI | **normative** — port from this | **design reference** — read for structure, table binding and shader shape |

Port from the CTL (it is normative and Apache-2.0, matching the workspace
licence); read OCIO's C++ and its GPU emitter for how to organise it. Do **not**
copy OCIO's C++ verbatim into an Apache-2.0 crate without carrying the
BSD-3-Clause notice — BSD-3 is compatible, but it is a different licence and
its attribution and no-endorsement clauses travel with the code.

### 2.7 The real ACES 2.0 configs are inside OpenColorIO, as plain files

`src/OpenColorIO/builtinconfigs/configs/` contains eight `.ocio` files compiled
into the library and addressable as `ocio://<name>`:

| File | Size |
|---|---:|
| `cg-config-v1.0.0_aces-v1.3_ocio-v2.1.ocio` | 18,661 |
| `cg-config-v2.1.0_aces-v1.3_ocio-v2.3.ocio` | 24,889 |
| `cg-config-v2.2.0_aces-v1.3_ocio-v2.4.ocio` | 27,989 |
| **`cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio`** | **32,807** |
| `studio-config-v1.0.0_aces-v1.3_ocio-v2.1.ocio` | 55,873 |
| `studio-config-v2.1.0_aces-v1.3_ocio-v2.3.ocio` | 70,374 |
| `studio-config-v2.2.0_aces-v1.3_ocio-v2.4.ocio` | 74,325 |
| **`studio-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio`** | **71,755** |

with `DEFAULT_BUILTIN_CONFIG_URI` and `LATEST_CG_BUILTIN_CONFIG_URI` both
`ocio://cg-config-v4.0.0_aces-v2.0_ocio-v2.5`, and
`LATEST_STUDIO_BUILTIN_CONFIG_URI`
`ocio://studio-config-v4.0.0_aces-v2.0_ocio-v2.5`.

**`AcademySoftwareFoundation/OpenColorIO-Config-ACES`'s `LICENSE` is
byte-identical to OpenColorIO's** — verified by `diff`: same BSD-3-Clause text,
same "Copyright Contributors to the OpenColorIO Project" line. Doc 10's
`UNVERIFIED` on this is **closed**.

**So R3 costs a file copy.** No release-asset download, no Dropbox, no
licensing question: two BSD-3-Clause `.ocio` files, 32 KB and 72 KB, may be
vendored as test fixtures today with the licence notice retained.

**And they raise the coverage bar sharply.** The ACES 2.0 CG config's
`ocio_profile_version` is **2.5**, it declares 27 colour spaces, and it uses
these 18 builtin styles:

```
ACEScc_to_ACES2065-1                     ACEScct_to_ACES2065-1
ACEScg_to_ACES2065-1                     ACES-LMT - ACES 1.3 Reference Gamut Compression
UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD    pass_thru
ACES-OUTPUT - … - SDR-100nit-REC709_2.0  ACES-OUTPUT - … - SDR-100nit-P3-D65_2.0
ACES-OUTPUT - … - HDR-1000nit-P3-D65_2.0 ACES-OUTPUT - … - HDR-1000nit-REC2020_2.0
DISPLAY - CIE-XYZ-D65_to_DisplayP3       DISPLAY - CIE-XYZ-D65_to_DisplayP3-HDR
DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ     DISPLAY - CIE-XYZ-D65_to_ST2084-P3-D65
DISPLAY - CIE-XYZ-D65_to_sRGB - MIRROR NEGS
DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709 - MIRROR NEGS
DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65 - MIRROR NEGS
DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709 - MIRROR NEGS
```

**`cine-color` supports 4 of these 18.** Every `_2.0` output style, every
` - MIRROR NEGS` display variant, `pass_thru`, `DisplayP3`, `DisplayP3-HDR`,
`ST2084-P3-D65` and the ACES 1.3 gamut-compression LMT are all unimplemented —
and every one of them would raise `Error::Unsupported`, so **the current
built-in config would fail to produce a working view.**

**How the registry count is arrived at.** A grep for
`registry.addBuiltin("…")` string literals finds 66; OCIO also registers the
entire ACES 2.0 output family from a table in a loop (`for (const auto& tr :
aces2_output_transforms) registry.addBuiltin(tr.name…)`). Including those, the
registry is **97 styles, of which 31 are the `…_2.0` ACES 2 output
transforms**. `cine-color` matches **18 of 97 — 19 %** — and **none of the 31
ACES 2.0 output styles**. The `_1.0`/`_1.1` styles it does recognise are the
**ACES 1.x** output transforms, which is a further reason the current
substitution (§1.4) is wrong: it substitutes an ACES-2-tonescale-based
transform for the ACES **1.x** styles.

---

## 3. OpenColorIO as a dependency

### 3.1 Licence — verified in full

Cloned `AcademySoftwareFoundation/OpenColorIO` at
**`5a808fb57a94c7229640a97835c420c9a1fbd1fe`** (2026-07-09). `LICENSE`, read in
full, is **BSD-3-Clause**:

> Copyright Contributors to the OpenColorIO Project.
>
> Redistribution and use in source and binary forms, with or without
> modification, are permitted provided that the following conditions are met:
>
> * Redistributions of source code must retain the above copyright notice, this
>   list of conditions and the following disclaimer.
> * Redistributions in binary form must reproduce the above copyright notice,
>   this list of conditions and the following disclaimer in the documentation
>   and/or other materials provided with the distribution.
> * **Neither the name of the copyright holder nor the names of its contributors
>   may be used to endorse or promote products derived from this software
>   without specific prior written permission.**
>
> THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" …

There is **no fourth clause and no patent grant**. BSD-3-Clause is
Apache-2.0-compatible for redistribution. The third clause means **"OpenColorIO"
must not be used to promote CINE-OMNI** — which is a constraint on marketing
copy, not on linking, and it reinforces the wording discipline of §9.

`main` is **`project(OpenColorIO VERSION 2.6.0 …)`** with
`OpenColorIO_VERSION_RELEASE_TYPE "dev"`. Doc 10's "latest release v2.5.2" is
about releases; `main` is ahead of it.

Build floor, read from the tree:
`cmake_minimum_required(VERSION 3.14...4.0)`;
`share/cmake/utils/CppVersion.cmake` sets `CMAKE_CXX_STANDARD 17` by default and
accepts 17, 20 or 23 with `CMAKE_CXX_STANDARD_REQUIRED ON`.

### 3.2 Can OCIO be built *here*? — the environment, measured

**What is absent from `PATH`** (checked individually):
`cc`, `gcc`, `g++`, `clang`, `clang++`, `make`, `cmake`, `pkg-config`, `nasm`,
`ninja` — **all MISSING**. Present: `python3`, `git`, `curl`, `uv`, `uvx`.

**What is present but unwired:** `zig` is **not** on `PATH`; the binary is at
`~/.local/zig/zig` and reports **`0.16.0`**. Two one-line wrappers exist:

```sh
# ~/.local/bin/zigcc
exec "$HOME/.local/zig/zig" cc  -target x86_64-linux-gnu.2.39 "$@"
# ~/.local/bin/zigcxx
exec "$HOME/.local/zig/zig" c++ -target x86_64-linux-gnu.2.39 "$@"
```

and `~/.cargo/config.toml` already sets them as the Rust linker and as
`CC`/`CXX`.

**Measured, not assumed:**

| Test | Result |
|---|---|
| `zigcc t.c -o t` (trivial C) | **OK** |
| `zigcxx -std=c++17 t.cpp -o t` using `<string>`, `<cstdio>`, then run it | **OK** — links libc++, prints, exits 0 |
| `uv tool install cmake` | **OK** — `cmake==4.4.3`, installs `cmake`, `ctest`, `cpack`, `ccmake` |
| `uv tool install ninja` | **OK** — `ninja==1.13.2` |

So the premise "this machine cannot build C++" is **false**: it can, and a build
system is one command away. The `zig c++` C++17 build does emit a wall of
`-Wnullability-completeness` warnings from libc++ headers, which is noise, not
failure.

> **§3.4 below records the actual OCIO build attempt. Until that section is
> filled in, treat OCIO's buildability here as `UNVERIFIED`.**

### 3.3 Why "can it build here" is not the same as "should we depend on it"

Even a successful build on this machine would not settle the dependency
question, and it is worth separating the two:

* **This machine is not the target.** CINE-OMNI must build on contributors'
  machines and in CI. A dependency that needs a bespoke `zig c++` +
  pip-installed-CMake incantation to build is a dependency that will be broken
  most of the time for most people.
* **Rust's `cc`/`cmake` build-script ecosystem is the mechanism**, and it is
  exactly what pulls a six-library C++ tree into `cargo build`.
* **A runtime dependency and a test-time dependency have completely different
  costs.** This is the hinge of the recommendation in §7.

### 3.4 The build attempt — CMake configuration succeeds

**Answer to the "is OCIO BLOCKED here?" question: configuration is not
blocked. It succeeds, with all six mandatory dependencies fetched and built
from source by OCIO's own CMake.**

What was done, exactly, and it is reproducible:

```sh
uv tool install cmake      # cmake==4.4.3
uv tool install ninja      # ninja==1.13.2
export CC=~/.local/bin/zigcc CXX=~/.local/bin/zigcxx
cmake -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_COMPILER=$CC -DCMAKE_CXX_COMPILER=$CXX \
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
  -DOCIO_USE_SIMD=OFF \
  -DOCIO_BUILD_PYTHON=OFF -DOCIO_BUILD_APPS=OFF -DOCIO_BUILD_TESTS=OFF \
  -DOCIO_BUILD_GPU_TESTS=OFF -DOCIO_BUILD_DOCS=OFF -DOCIO_BUILD_NUKE=OFF \
  -DOCIO_BUILD_JAVA=OFF -DOCIO_INSTALL_EXT_PACKAGES=ALL \
  -DCMAKE_INSTALL_PREFIX=… …/OpenColorIO
```

Result: `CONFIGURE exit=0`, *"Configuring done (3.2s) / Generating done (0.1s)
/ Build files have been written to …"*, with:

```
-- Checking for mandatory dependencies...
-- Installed expat      (version "2.7.2")
-- Installed yaml-cpp   (version "0.8.0")
-- Installed pystring   (version "1.1.4")
-- Installed Imath      (version "3.2.1")
-- Installed ZLIB       (version "1.3.1")
-- Installed minizip-ng (version "4.0.10")
```

**Three findings from the attempt, each worth recording:**

1. **CMake identifies `zig c++` as `Clang 21.1.0`** and its C++17 feature test
   passes. `zig cc`/`zig c++` is a viable C++17 toolchain for this project.
2. **`-DCMAKE_POLICY_VERSION_MINIMUM=3.5` is required** with CMake 4.x, because
   OCIO's fetched dependencies still declare `cmake_minimum_required` below
   CMake 4's floor. OCIO's own `CMakeLists.txt` is fine
   (`cmake_minimum_required(VERSION 3.14...4.0)`).
3. **`-DOCIO_USE_SIMD=OFF` was needed for a practical reason, not a
   correctness one.** With SIMD probing on, the first configure attempt hit a
   900-second timeout inside `COMPILER_SUPPORTS_AVX2`: each CMake `try_compile`
   spawns `zig c++`, which rebuilds its libc++/compiler-rt cache, and the
   machine was simultaneously running another `cargo test`. `COMPILER_SUPPORTS_AVX`
   had already reported **Success**, so this is a speed problem, not a
   capability one. On a machine with a normal compiler this would not arise.
4. **ExternalProject sub-builds do not inherit `CMAKE_CXX_COMPILER`.** The
   first build attempt failed at yaml-cpp with *"The CXX compiler
   identification is unknown / No CMAKE_CXX_COMPILER could be found."* The fix
   is to put `cc`, `c++`, `gcc`, `g++`, `clang`, `clang++`, `ar` and `ranlib`
   shims for `zig cc` / `zig c++` on `PATH` so the nested configures find a
   compiler by name.

**Dependency licences — doc 10's `UNVERIFIED` closed for five of six**, read
from the actual files that OCIO's build downloaded:

| Dependency | Version built | Licence, from the fetched file |
|---|---|---|
| Imath | 3.2.1 | **BSD-3-Clause** — *"Copyright Contributors to the OpenEXR Project. All rights reserved."* |
| pystring | 1.1.4 | **BSD-3-Clause** — *"Copyright (c) 2008-present Contributors to the Pystring project."* |
| yaml-cpp | 0.8.0 | **MIT** — *"Copyright (c) 2008-2015 Jesse Beder. Permission is hereby granted, free of charge…"* |
| zlib | 1.3.1 | **zlib licence** — *"(C) 1995-2022 Jean-loup Gailly and Mark Adler"* |
| expat | 2.7.2 | **MIT** — *"Copyright (c) 1998-2000 Thai Open Source Software Center Ltd and Clark Cooper; Copyright (c) 2001-2025 Expat maintainers"* |
| minizip-ng | 4.0.10 | licence file not located in the build tree — **UNVERIFIED** |

Plus two vendored in OCIO's own `ext/`: **`sampleicc`** and **`xxHash`**
(licences **UNVERIFIED** — no `LICENSE` file found at the expected paths).

**Nothing copyleft.** Every verified licence is permissive and
Apache-2.0-compatible. **So the licence objection to depending on OCIO is
formally dead** — which makes it worth being clear that the recommendation in
§7 rejects a runtime OCIO dependency for *engineering* reasons (a six-library
C++ tree in a Rust build, and no WGSL), not legal ones.

### 3.5 The compile — and the single flag that stopped it

**Every one of OpenColorIO's ~180 translation units compiled with `zig c++`,
with zero errors and zero warnings.** The build reached `[243/246] Linking CXX
shared library src/OpenColorIO/libOpenColorIO.so.2.6.0` and failed there, on
exactly one thing:

```
error: unsupported linker arg: --exclude-libs
```

OCIO adds `-Wl,--exclude-libs,ALL` in `src/OpenColorIO/CMakeLists.txt:400`;
zig's LLD wrapper rejects it. **This is a zig limitation, not an OCIO problem,
not a CMake problem and not a C++ problem** — and it is one line:

```cmake
# src/OpenColorIO/CMakeLists.txt:400
set(CUSTOM_LINK_FLAGS "${CUSTOM_LINK_FLAGS};-Wl,--exclude-libs,ALL")
```

The link line is otherwise entirely ordinary: 180 objects against
`libexpat.a`, `libImath-3_2.a`, `libpystring.a`, `libyaml-cpp.a`,
`libminizip-ng.a` and `libz.a`, all six built moments earlier by the same
toolchain.

**Verdict for the directive's question: NOT BLOCKED.** OpenColorIO 2.6.0-dev
configures and compiles on a machine with no system C or C++ compiler, no
`make`, no `pkg-config` and no `nasm`, using `zig cc` / `zig c++` 0.16 with
pip-installed CMake and Ninja. What it needs beyond a stock invocation is
three things, all documented above and all one-liners:

1. `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` (CMake 4.x vs the dependencies' old
   `cmake_minimum_required`)
2. `cc`/`c++`/`gcc`/`g++`/`ar`/`ranlib` shims on `PATH` (ExternalProject
   sub-builds do not inherit `CMAKE_CXX_COMPILER`)
3. dropping `-Wl,--exclude-libs,ALL` (zig ld)

plus `-DOCIO_USE_SIMD=OFF` for speed on a contended machine.

**None of this changes the recommendation.** §7 keeps OpenColorIO out of the
shipped product for engineering reasons — a six-library C++17 tree inside
`cargo build`, and no WGSL — and the value of this result is precisely that it
makes the **oracle** (§7, track T3) reproducible rather than aspirational. A
developer tool that needs three CMake flags is fine; a runtime dependency that
needs them is not.

**And with that one line commented out, it links and it runs.**

```
[38/40] Linking CXX shared library src/OpenColorIO/libOpenColorIO.so.2.6.0
[39/40] Creating library symlink … libOpenColorIO.so.2.6 … libOpenColorIO.so
[40/40] Linking CXX static library src/apputils/libapputils.a
NINJA exit=0
libOpenColorIO.so.2.6.0   69,690,104 bytes
```

A 40-line C++ program linked against it produces real reference values:

```
OCIO version: 2.6.0dev
config name: cg-config-v4.0.0_aces-v2.0_ocio-v2.5
colorspaces: 25  displays: 8
display[0]=sRGB - Display  view[0]=ACES 2.0 - SDR 100 nits (Rec.709)
  in (0.1800, 0.1800, 0.1800) -> out (0.349189, 0.349189, 0.349189)
  in (1.0000, 0.0000, 0.0000) -> out (0.877157, 0.000000, 0.040252)
  …
SDR-VIDEO_1.0 on AP0 0.18 grey -> XYZ (0.098908, 0.104064, 0.113331)
```

**U1 is closed: the oracle of §7 track T3 is not a proposal, it exists, and the
numbers in finding 0b came out of it.** Note in passing that OCIO reports **25
colour spaces and 8 displays** for the CG config — the coverage baseline R3's
test should assert against.

---

## 4. Is `tests/ocio_fixture.rs` real OCIO compatibility?

**No. It is a parser regression suite. Blunt verdict: it resembles
compatibility; it does not constitute it.**

The evidence, item by item.

**The fixture is self-authored.** `tests/fixtures/mini.ocio` (5,485 bytes)
opens with `# Minimal OCIO v2 config exercising the cine-color parser.` It was
written to exercise this parser. It is not a config produced by
`OpenColorIO-Config-ACES`, not a config shipped by any vendor, and not a config
any user would have.

**No OpenColorIO is ever executed.** `cine-color` has no C/C++ dependency,
there is no `build.rs`, and nothing in the workspace shells out to `ocio*`
tools. Therefore no assertion in the tree can be about agreement with
OpenColorIO — there is nothing to agree *with*.

**The numeric assertions are self-referential or loose.**

| Assertion | What it actually proves |
|---|---|
| `let want = cine_color::space::rgb_to_rgb_matrix(&REC709, &ACES_AP1); … abs() < 2e-3` | The config compiler reproduces the crate's own matrix function. Says nothing about OCIO. |
| `(p.apply_rgb(&[0.5;3])[0] - 0.5f32.powf(2.2)).abs() < 1e-5` | `ExponentTransform` is a power function. An algebraic identity. |
| `(p.apply_rgb(&[0.5;3])[0] - 0.2140).abs() < 1e-3` | One hand-computed sRGB value at one input, to 3 decimal places. |
| `assert!(toned[0] > 0.3 && toned[0] < 0.5)` | The "ACES SDR" view is monotone-ish and lands in a wide band. **This is the assertion covering the output transform.** |
| `assert!(warm[0] > warm[2])` | A warming CDL warms. A sign check. |
| round-trip `< 1e-4/1e-5` | Ops invert each other. Self-consistency. |
| `Config::parse(cfg.to_yaml()) == cfg` | The writer is the parser's inverse. Genuinely valuable — but about YAML, not colour. |

**What it does prove, and it is not nothing:** the YAML subset parses;
`GroupTransform` nesting and `direction: inverse` compose correctly; file LUTs
resolve relative to the config; `isdata` and aliases work; unsupported
directions raise `Error::Unsupported`; unknown displays raise
`Error::UnknownDisplay`; the writer round-trips. Those are real regression
guards for real bugs.

**What it cannot prove, by construction:** that a config from the field loads;
that a supported style means what OCIO means by it; that any pixel matches
OpenColorIO's pixel; that the parser's *coverage* is anything in particular. And
because of the silent-substitution defect (§1.4), passing this suite is
compatible with the four `ACES-OUTPUT` styles being wrong by a visible margin.

### 4.0 The measurement: `cine-color` cannot load any released ACES config

**This was not inferred. It was run.** A throwaway crate depending on
`cine-color` by path was pointed at the three real configs extracted from
OpenColorIO's own tree (§2.7). The complete output:

```
===== cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio =====
PARSE FAILED: parse error at line 0: view without name in display `sRGB - Display`

===== studio-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio =====
PARSE FAILED: parse error at line 0: view without name in display `sRGB - Display`

===== cg-config-v2.2.0_aces-v1.3_ocio-v2.4.ocio =====
PARSE FAILED: parse error at line 0: view without name in display `sRGB - Display`
```

**Not a partial load. Not a missing colour space. `Config::load` returns
`Err` on the first display block of every ACES config OpenColorIO ships —
including the ACES 1.3 ones that have existed since 2021.**

**Why.** The real configs use OCIO v2's *shared views*:

```yaml
shared_views:
  - !<View> {name: ACES 2.0 - SDR 100 nits (Rec.709), view_transform: …,
             display_colorspace: <USE_DISPLAY_NAME>, rule: Any Scene-linear or Log}
  …
displays:
  sRGB - Display:
    - !<View> {name: Raw, colorspace: Raw}
    - !<Views> [ACES 2.0 - SDR 100 nits (Rec.709), Un-tone-mapped, Video (colorimetric)]
```

`!<Views>` is a *list of names referring into `shared_views`*, not a view
definition. The parser sees a `!<Views>` node, looks for a `name` key, does not
find one, and fails.

**The gap is structural, not a typo.** Top-level keys used by the real configs
versus keys the parser models:

| Key | In `cg-config-v4.0.0` | Parser |
|---|:--:|:--:|
| `roles`, `displays`, `colorspaces`, `display_colorspaces`, `looks`, `view_transforms`, `active_displays`, `active_views`, `default_view_transform`, `description`, `name`, `search_path`, `ocio_profile_version` | ✓ | **✓** |
| **`shared_views`** | ✓ | ✗ |
| **`viewing_rules`** | ✓ | ✗ |
| **`virtual_display`** | ✓ | ✗ |
| **`named_transforms`** | ✓ | ✗ |
| **`file_rules`** | ✓ | ✗ |
| **`inactive_colorspaces`** | ✓ | ✗ |
| **`environment`** | ✓ | ✗ |
| `luma`, `strictparsing` | ✓ | ✗ (silently dropped) |

and the YAML tags: the Studio config uses `!<Views>` × 10, `!<NamedTransform>`
× 15 and `!<Rule>` × 5, **none of which the parser recognises**. (It does
handle `!<ColorSpace>` × 57, `!<GroupTransform>` × 48, `!<MatrixTransform>`
× 38, `!<BuiltinTransform>` × 34, `!<View>` × 23, `!<LogCameraTransform>` × 19,
`!<ViewTransform>` × 13, `!<ExponentWithLinearTransform>` × 6,
`!<ExponentTransform>` × 6 and `!<Look>` × 1 — the transform vocabulary is in
reasonable shape; it is the *config-level* vocabulary that is missing.)

**A related defect the same probe exposed: the writer is lossy, and the
round-trip test cannot see it.** `fixture_round_trips_through_writer` asserts
`Config::parse(cfg.to_yaml()) == cfg` — a round trip *through the crate's own
model*, which is trivially satisfied by any field the model does not have.
Writing `mini.ocio` back out produces **5,104 bytes from 5,485**, and diffing
shows `strictparsing` and `luma` gone, comments gone, and — more seriously —
builtin style strings emitted **unquoted** inside flow mappings:

```yaml
-    from_scene_reference: !<BuiltinTransform> {style: "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0"}
+    from_scene_reference: !<BuiltinTransform> {style: ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0}
```

The crate's own parser reads that back, so the test passes. *Whether
OpenColorIO's yaml-cpp reader accepts an unquoted plain scalar there is*
**UNVERIFIED** *and is exactly the sort of thing only a real OCIO can answer.*
**A config writer that silently drops fields is a data-loss bug**: round-trip
a colourist's config through CINE-OMNI and their `luma`, `file_rules`,
`inactive_colorspaces` and `named_transforms` are gone.

**This single measurement settles the §4 verdict more firmly than any argument
could.** "Reads a documented subset of OCIO v2 configs" is true only in the
sense that the subset does not include any config anyone has ever shipped.

### 4.1 The decisive evidence: the style names are wrong

The `mini.ocio` fixture and the parser agree with each other on style strings
that OpenColorIO has never used. Extracted from OCIO `main`
(`5a808fb5…`) — every `registry.addBuiltin("…")` in
`src/OpenColorIO/transforms/builtins/{ACES,AppleCameras,ArriCameras,CanonCameras,Displays,PanasonicCameras,RedCameras,SonyCameras}.cpp`
— plus the ACES 2.0 family registered from the `aces2_output_transforms` table,
the authoritative registry is **97 styles**. Compared against the 24 the
crate's own `builtin_styles_invert` test enumerates:

* **18 match exactly.**
* **6 do not exist in OpenColorIO** (table in §0, finding 6a).
* **79 of OCIO's 97 are unimplemented** — including all four ADX and ACESproxy
  input styles, both `ACES-LMT` styles (notably *ACES 1.3 Reference Gamut
  Compression*), `APPLE_LOG_to_ACES2065-1`, `RED_REDLOGFILM-RWG_to_ACES2065-1`,
  the two Sony VENICE variants, all eight `CURVE - …` styles, the twelve
  `ACES-OUTPUT` cinema and higher-nit variants, and every ` - MIRROR NEGS`
  display variant.

`tests/fixtures/mini.ocio` contains
`!<BuiltinTransform> {style: SONY_SLOG3_SGAMUT3.CINE_to_ACES2065-1}`. **A real
OpenColorIO would throw on that config.** The fixture therefore cannot be a
compatibility test even in principle: it is not a config OpenColorIO accepts.

This is not a nitpick about strings. A builtin style name is the *entire*
interface between a config and an implementation. Getting it wrong means the
transform is never found; getting the semantics wrong (finding 6b) means it is
found and silently different. The crate currently does both, in different
places, and **no test in the tree could have detected either**, because every
input the tests use was written by the same hand as the parser.

**What would make it a compatibility test.** In increasing order of strength:

1. **Load real configs.** Add `studio-config-*.ocio` and `cg-config-*.ocio`
   from `OpenColorIO-Config-ACES` releases as fixtures, and assert on
   *coverage*: how many colour spaces, displays, views and builtin styles
   loaded, and an explicit, committed list of what did not. A test whose failure
   message is "3 styles newly unsupported" is worth more than any tolerance.
2. **Golden vectors, checked in.** For every builtin style claimed, a table of
   input→output triples generated **by OpenColorIO** and committed as data.
   A few hundred numbers; it needs OCIO **once**; thereafter CI needs no C++ at
   all. (It has to come *after* the style names are right and the configs load
   — §10's R0 and R2 — because you cannot generate a vector for a style that
   does not exist.)
3. **A differential oracle.** An off-by-default `xtask` that runs a system OCIO
   and diffs a dense sample of the domain against the native path. This produces
   the evidence for a public compatibility claim and never ships.

Items 2 and 3 are the same tool at different cadences, and together they are the
concrete form of doc 10's "OCIO as an interoperability target, not a runtime
dependency".

---

## 5. "Colour managed" as a testable contract

Every product says "colour managed" and almost none of them say what they mean.
Here is the minimum honest definition for CINE-OMNI, written as properties that
a test can fail.

> **A system is colour managed if, for every pixel it displays or writes, it can
> name the transform chain that produced it, that chain is invertible or
> explicitly marked lossy, no stage in it was chosen by a guess that was not
> recorded, and the result is reproducible on another machine.**

Six properties. Each has a test.

### C1 — Every image carries a colour space, and there is no default

*No buffer, texture, file or node may exist in an "unknown" colour state that is
silently treated as anything.*

**Test:** the pixel-carrying type has a non-`Option` colour-space field. Loading
media with no signalled colorimetry produces `Signalled::Unspecified` plus an
`Effective` value *and* an `Inference` record naming the rule used (doc 10 §A.1.3
already specifies the `signalled`/`effective` split; this contract makes it a
requirement, not a suggestion). **Failing case today:** `ColorMeta` has three
strings and no signalled/effective distinction.

### C2 — Every transform is explicit, ordered and invertible-or-marked

*A processor is a list of ops. Each op declares whether it is invertible, and
lossy ops (clamps, gamut compression, out-of-range clipping) are marked lossy
rather than silently non-invertible.*

**Test:** `Processor::inverse()` succeeds, or fails naming the op that blocked
it. Round-trip `forward∘inverse` over a dense in-gamut sample is within a stated
tolerance. **Partly met today:** `Op::inverse()` exists and returns `Option`;
`Op::Custom` may carry no inverse; nothing marks *lossiness* distinctly from
*non-invertibility*, and `Op::Range { clamp: true }` and the output transform's
gamut clamp are both silently lossy.

### C3 — No transform is ever substituted for another without saying so

*If the system cannot perform the named transform, it either refuses by name or
performs a substitute that is labelled, in the data and in the UI, as a
substitute.*

**Test:** every `Processor` carries a `provenance: Vec<Provenance>` where
`Provenance::Exact { source }` or
`Provenance::Approximation { of, by, reason }`. Assert that a processor built
from a config containing an unimplemented builtin style is either `Err` or
carries at least one `Approximation`. **Failing case today, and this is the
crate's worst defect:** the four `ACES-OUTPUT` styles are substituted silently
(§1.4).

### C4 — Working space is scene-linear, and the boundaries are the only place encoding changes

*Decode to linear once at ingest; encode once at display or export. No
intermediate re-encode; no compositing or filtering on non-linear values.*

**Test:** a compositing/filter op that receives a non-linear-tagged buffer is a
type error or an assertion failure, not a subtle artefact. **Met in spirit
today** — the compositor is premultiplied-linear throughout (doc 10 §A.4.2) —
but not enforced by a type.

### C5 — The view transform is not part of the image

*What is on screen is `image ∘ view`, and the view is never baked into stored
pixels except by an explicit export. Two viewers with different views over the
same frame must not share a cache entry.*

**Test:** the render-cache key includes the full view-transform identity
(transform, display, peak nits, tone-map operator and its parameters, look, CDL).
Assert that changing peak nits alone changes the key.

### C6 — Reproducible: same input, same config, same pixels, anywhere

*No transform result may depend on the machine, the GPU, the driver, or the
order of a parallel loop.*

**Test:** the CPU `Processor` is the reference; the GPU path is asserted equal
to it within a published tolerance on real hardware; the CPU path itself is
bit-deterministic given the same input (no `rayon` reduction whose result
depends on chunking). **Partly met:** `cine-gpu` already compares against a CPU
mirror to 1e-5 (doc 10 §A.5.3). Determinism of the `rayon` paths in
`cine-color` is **UNVERIFIED**.

### The honest summary line

Given C1–C6, the claim CINE-OMNI may make today is:

> **"Colour managed: every image is tagged, every transform is an explicit
> invertible op chain, and the view transform is never baked in. Transform
> *provenance* and full metadata capture are in progress; ACES rendering is
> approximate and labelled as such."**

and the claim it may **not** make is "ACES colour managed", "OCIO colour
managed", or "colour managed" without qualification while C1 and C3 fail.

---

## 6. Corrections to `research/10-color-and-sync.md`

Doc 10 is the primary source for this area and is mostly right. Three items need
amending, and one is load-bearing.

**6.1 — ACES pre-2.0 licence: `UNVERIFIED` → resolved, and the answer matters.**
Doc 10 §A.6.1 says *"only the current `main` was verified. The historical `v1.3`
/ `v1.3.1` tags may still carry the old AMPAS terms — UNVERIFIED."* They do.
See §2.1. Add the date (2025-09-25) and the commit SHA to any ACES vendoring
plan.

**6.2 — `Op::Custom` is used, so R3 as written cannot be implemented.** Doc 10
§A.5.3 and R3 both rest on `grep -c "Op::Custom"` returning 0 outside
`processor.rs`, and conclude the WGSL emitter is "a ten-arm match, not a
compiler". The variant is constructed via `Op::custom_invertible` at three
sites, including both entry points of the ACES output transform (§1.3). **The
conclusion survives; the premise does not.** The emitter is a ten-arm match
*only after* the output transform is expressed in the op vocabulary — which is
the ACES 2 port. R3 and R7 are therefore not independent: **R3 depends on R7**.

**6.3 — "Unsupported styles raise a named error" is true but incomplete.** Doc
10 §A.6.3 lists this as "the single most important honesty property in a colour
parser, and it is already right". It is right for *unrecognised* styles. For the
four `ACES-OUTPUT` styles the crate recognises and substitutes, the property
fails in the more dangerous direction (§1.4). The honesty property should be
restated as: *unsupported styles raise a named error; approximated styles carry
a labelled provenance*. Only the first half is implemented.

---

## 7. Decision: OCIO, or port ACES, or both

**Recommendation: both, in strictly separated roles.**

> **Port `aces-core` (Apache-2.0) into Rust as the runtime rendering. Use a real
> OpenColorIO build as a developer- and CI-side *oracle* that generates
> committed golden vectors and produces the evidence for every compatibility
> claim. Ship no C++ in the product.**

**Why this, and not one or the other:**

1. **The runtime needs to be Rust because the GPU path needs to be WGSL.** Doc
   10 §A.5.3 established that every automated GLSL→WGSL route is blocked by
   combined image samplers. Whatever produces the shader has to be code CINE-OMNI
   controls. OCIO cannot be that, today, for WebGPU.
2. **The thing being ported is small and now correctly licensed.** 2,858 lines
   of dependency-free scalar CTL, Apache-2.0 since 2025-09-25, with an explicit
   patent grant the old AMPAS terms withheld. This is the cheapest it has ever
   been to have the *real* transform.
3. **Compatibility claims need a witness, and only OCIO can be it.** The
   difference between "we implement OCIO's config format" and "we agree with
   OCIO" is a measurement. A test-time-only OCIO makes that measurement possible
   without putting a six-library C++17 tree in anyone's `cargo build`.
4. **The costs land where they can be paid.** A broken oracle build blocks a
   maintainer generating new golden vectors; it never blocks a user.

**Concretely, in three tracks:**

* **T1 (product, Rust/WGSL).** Port `aces-core`. Tables on the CPU, per-pixel
  maths in WGSL (§8). Pin the SHA. Keep the Hill fit, relabelled, as a preview.
* **T2 (CI, data).** Commit golden vectors generated by OCIO and by the ACES CTL
  interpreter. CI needs no C++ — only the data.
* **T3 (maintainer, off by default).** `xtask ocio-oracle`: builds or finds a
  system OCIO, regenerates T2's vectors, diffs a dense sample. Never in the
  dependency graph of any shipped crate. **This is demonstrably buildable** —
  §3.4 configures OCIO from scratch on a machine with *no system C or C++
  compiler at all*, using `zig c++`, a pip-installed CMake, and OCIO's own
  dependency fetcher. On a machine with a normal toolchain it is a one-liner.

**The ordering matters, and it is not the order of importance.** Golden vectors
cannot be generated for styles whose names are wrong (R0) or from configs that
will not load (R2), so the sequence is **R0 → R1 → R2 → R3 → R3b → R4 → R5 →
R6**, with **R7 (CLF)** running in parallel and gating any future certification
claim (§11.1).

### Rejected alternatives

**A. Link OpenColorIO as a runtime dependency (native, static or dynamic).**
*Rejected.* It puts a C++17 tree (Expat, yaml-cpp, Imath, pystring, minizip-ng,
zlib — versions per doc 10 §A.5.1) into every build of a Rust workspace whose
entire current colour stack has zero native dependencies; it does not solve the
GPU problem, which is the actual problem; and the only Rust binding is
`ocio-rs` 0.2.1 which **defaults to a stub mode that performs no colour
management** (doc 10 §A.5.1) — a failure mode that produces plausible wrong
pixels rather than a link error. *Not rejected on licence: BSD-3-Clause is
fine.*

**B. Call OCIO out-of-process (subprocess / `ociobakelut` at runtime).**
*Rejected for the interactive path, accepted for export.* A per-view subprocess
is unusable at interactive rates, and baking to a 3D LUT loses extended range,
precision, invertibility and dynamic properties (doc 10 §A.5.3, Option 3, with
OCIO's own documentation quoted on the precision cost). It **is** the right
mechanism for *exporting* a look to a facility pipeline, and for T3's oracle.

**C. Port ACES and drop OCIO entirely.** *Rejected.* This is roughly today's
position and it is why no compatibility claim can be substantiated. Without an
oracle, "reads a documented subset of OCIO v2 configs" can never become anything
stronger, and the subset's coverage is unmeasured.

**D. Write a WGSL backend for OCIO upstream and depend on it.** *Rejected as a
plan; endorsed as a contribution.* Doc 10 §A.5.3 Option 2 covers this: it is the
cleanest long-term answer to "how does anyone use OCIO with WebGPU", it would
make CINE-OMNI a good ASWF citizen, and nobody has ever asked for it. It is also
C++ work on someone else's release schedule that does not remove the C++ tree
from CINE-OMNI's build. Propose it; do not plan around it.

**E. Keep the analytic fit and call it ACES.** *Rejected, permanently.* Both
canonical fits (Narkowicz, Hill) approximate ACES **1.x**, whose RRT and ODT no
longer exist as separate entities in ACES 2. Narkowicz's own write-up says *"We
don't even need to run any ACES code at all"* and reports max fit error 0.0138.
Shipping that under the name "ACES" in 2026 describes a ten-year-old
approximation of a superseded transform.

**F. Reimplement ACES 2 from the published papers instead of porting the CTL.**
*Rejected.* Slower, less accurate, and — now that `aces-core` is Apache-2.0 with
a patent grant — strictly worse legally as well as technically. The only reason
to do this was the old licence, and that reason is gone.

---

## 8. What belongs where: media layer / `cine-color` / `cine-gpu`

The boundary rule, stated once:

> **`cine-media` owns what the file says. `cine-color` owns what the transform
> is. `cine-gpu` owns how it runs on a pixel.**

| Concern | Crate | Why |
|---|---|---|
| H.273 code points, range flag, chroma siting, bit depth, ST 2086, MaxCLL/MaxFALL, alpha convention as **read from the file** | **`cine-media`** | File facts. Doc 10 §R4's `VideoSignal`, with `signalled` and `effective` separate. |
| The *inference rule* that filled in an unspecified value | **`cine-media`** | It is a property of how the file was interpreted, and must be recorded next to the evidence. |
| YUV→RGB matrix/range/siting **maths** | **`cine-color`** (definition) → **`cine-gpu`** (execution) | The Kr/Kb matrix is a `Mat3`; `cine-color` builds it, `cine-gpu` fuses it into the sampling shader. Doc 10 §A.2.5. |
| Transfer functions, primaries, chromatic adaptation | **`cine-color`** | Pure, `f64`, testable. |
| OCIO config parsing and compiling to `Processor` | **`cine-color`** | |
| CLF / `.cube` / CDL parsing | **`cine-color`** | |
| **`init_ODTParams`: the ACES 2 JMh params and the four 362-entry tables** | **`cine-color`**, CPU, `f64`, once per view configuration | Iterative solves and a gamma search; not per-pixel work; must be exact. |
| **`outputTransform_fwd`'s per-pixel body** (JMh, chroma compress, gamut compress with table lookups) | **`cine-gpu`** WGSL, generated from `cine-color` | This is the split the reference implementation already has (§2.3). |
| Display encoding (`eotf`/`eotf_inv`, range) | **`cine-color`** definition, **`cine-gpu`** execution | ACES splits it out too; one shared rendering pass + a cheap per-display encode. |
| Scopes | **`cine-color`** (definition) + **`cine-gpu`** (histogram/waveform reduction) | Already the case. |
| The WGSL emitter over `Processor` | **`cine-color`** emits text; **`cine-gpu`** compiles and validates it | Keeps the CPU `Processor` as the single source of truth and the reference. |
| Render-cache keys including the full view identity | **`cine-gpu`** / pipeline | C5. |
| The constants themselves (PQ `m1/m2/c1/c2/c3`, HLG `a/b/c`) | **`cine-color`**, generated into WGSL | Doc 10 already flags the duplication in `common.wgsl` as a defect. Generation, not a second copy. |

**One addition to doc 10's `VideoSignal`:** the ACES 2 table set is keyed on
its parameters, and there are five of them, not two. `aces-core`'s
`init_ODTParams` takes `(peakLuminance, limitingPrimaries)`; OpenColorIO's
builtin registration adds three more, and its table entries make the full set
explicit — `Generate_output_transform(ops, tr.peak_luminance,
tr.limiting_primaries, tr.encoding_primaries, tr.linear_scale,
tr.scale_white)`, e.g.

```cpp
{ "ACES-OUTPUT - … - HDR-2000nit-REC2020-D60-in-REC2020-D65_2.0",
  "…", 2000.f, REC2020_D60::primaries, REC2020::primaries, 1.f, true }
```

so the identity is **(peak luminance, limiting primaries, encoding primaries,
linear scale, apply-white-scale)**. That tuple is a *view* property, belongs in
the view-transform identity, and must be in the render-cache key (C5). The
`scale_white` flag is the CTL's `apply_white_scale`, which exists because some
display encodings (a DCDM P3-D65 ODT with its 48/52.37 factor) already handle
white-point fitting — the ACES 2.0 CHANGELOG records adding the flag for
exactly this reason, in preference to *"relying on a mismatch in the white point
chromaticities"*.

---

## 9. What we may and may not claim

**May claim, today:**

* "ACES colorimetry: AP0 and AP1 primaries and the ACES2065-1 / ACEScg / ACEScc
  / ACEScct encodings, with published reference matrices cross-checked against
  computed ones."
* "Input transforms for ARRI LogC3/LogC4, Sony S-Log3, Panasonic V-Log, RED
  Log3G10, Canon Log 2/3."
* "Uses the ACES 2.0 tonescale curve." — with the immediate qualifier that the
  surrounding rendering is not ACES 2.0's.
* "Reads and writes a documented subset of OpenColorIO v2 configs." — with the
  subset's coverage stated, and with the UI showing what a loaded config could
  not honour.
* "PQ and HLG implemented to the BT.2100 constants, including the HLG OOTF and
  display-dependent system gamma."
* "Premultiplied, scene-linear compositing throughout."

**May not claim, today:**

* **"ACES support"**, **"ACES-compatible"**, **"ACES compliant"**, **"ACES
  certified"**, or an unqualified **"ACES"** menu entry.
* **"ACES 2.0"** for the current output transform. It has the right tonescale
  and none of the rendering.
* **"ACES output transform"** for the Hill fit. Correct phrasing:
  *"analytic approximation of the ACES 1.x RRT + sRGB ODT (Hill fit)"*.
* **"RRT"** or **"ODT"** without an explicit "ACES 1.x" qualifier — ACES 2 has
  neither.
* **"OCIO support"**, **"OpenColorIO-compatible"**, or **"OCIO-native"**. The
  crate's own description string currently says *"ACES / OpenColorIO-native
  color management"*; **`native` is not defensible** and should be changed. Note
  also BSD-3-Clause's third clause: the OpenColorIO **name** must not be used to
  promote CINE-OMNI without permission — which makes "OCIO-native" a licence
  question as well as an honesty question.
* Any numerical conformance figure. None has been measured.

**Wording that is both accurate and useful**, for the UI:

| Where | String |
|---|---|
| View menu | `ACES 1.0 SDR (analytic approximation)` |
| View menu, after the port | `ACES 2.0 Output Transform (Rec.709, 100 nit)` |
| Config load banner | `Loaded 47 of 51 colour spaces. 4 unsupported: <list>` |
| Crate description | `ACES and OpenColorIO-format colour management, grading and scopes (CPU reference implementation)` |
| Any approximation | `Approximation of <X>: <reason>` — never anonymous |

---

## 10. Recommendations

Ordered by consequence per unit of work.

### R0 — Fix the six wrong style names. *(one hour)*

`SONY_SLOG3-SGAMUT3.CINE_to_ACES2065-1`,
`SONY_SLOG3-SGAMUT3_to_ACES2065-1` and `RED_LOG3G10-RWG_to_ACES2065-1` use a
hyphen, not an underscore. `DISPLAY - CIE-XYZ-D65_to_DisplayP3` is spelled that
way. `DISPLAY - CIE-XYZ-D65_to_REC.709` and `…_to_REC.2020` do not exist and
should be `…_to_REC.1886-REC.709` and `…_to_REC.1886-REC.2020`. Fix them,
fix `tests/fixtures/mini.ocio`, and add the OCIO registry as a committed data
file so the test can assert that **every style the crate claims is a real
one**. This is an hour of work that removes a whole class of "why doesn't my
config load" — and no existing test could have caught it.

### R1 — Stop the silent substitution, and stop silently dropping config fields. *(hours)*

Two halves of the same defect.

*Transforms:* the four `ACES-OUTPUT - … _1.0/_1.1` styles must not resolve to a
different transform without a diagnostic — and note they are the **ACES 1.x**
output transforms, being substituted by something built on the **ACES 2.0**
tonescale, so the substitution is wrong in two directions at once. Add
`Provenance` to `Processor`, mark them `Approximation { of, by, reason }`,
surface it in the UI, assert it in a test.

*Config fields:* unknown top-level keys and unknown YAML tags must be
**preserved and reported**, never dropped. Today `luma`, `strictparsing`,
`environment`, `file_rules`, `inactive_colorspaces`, `named_transforms`,
`shared_views`, `viewing_rules` and `virtual_display` all vanish on load, and
the round-trip test cannot see it because it round-trips through the model
rather than through the file (§4.0). Keep an `unrecognised: Vec<(String,
RawYaml)>` and write it back out.

**These are correctness fixes to shipped code and they are the cheapest
integrity wins available.**

### R2 — Implement OCIO v2's config-level vocabulary so real configs load at all. *(days–weeks)*

`shared_views` + `!<Views>` is the blocker: **without it no ACES config from
any year loads**. Then `named_transforms`, `viewing_rules` + `!<Rule>`,
`virtual_display`, `file_rules`, `inactive_colorspaces`, `environment`, and
`<USE_DISPLAY_NAME>` as a `display_colorspace` value. This is not exotic — it
is the ordinary shape of every config OCIO has shipped since 2021 — and it is
the single highest-value block of work in the OCIO half of this document.

### R3 — Vendor the real ACES configs as fixtures and assert on coverage. *(hours, after R2)*

They do not need downloading: `cg-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio`
(32,807 bytes) and `studio-config-v4.0.0_aces-v2.0_ocio-v2.5.ocio` (71,755
bytes) sit in OpenColorIO's own tree at
`src/OpenColorIO/builtinconfigs/configs/`, under the **BSD-3-Clause** licence
verified in §2.7 — copy them with the notice. The test asserts a committed
coverage list (spaces / displays / views / styles loaded, and an explicit list
of what was not), so any regression is a failing test with a useful message.

### R3b — Commit golden vectors generated by OpenColorIO. *(days, once)*

For every builtin style and colour space the crate claims, a committed table of
input→output triples produced by a real OCIO — including **out-of-range inputs**,
which is where §0 finding 6b says the display builtins already diverge. CI then
verifies agreement forever with **no C++ in the build**.

### R4 — Port `aces-core` to Rust, from `main`, pinning the SHA. *(weeks)*

Tables on the CPU in `f64`; per-pixel body in WGSL. Do it in the order the CTL
does: `Tonescale` (already done — verify it), `ColorSpaces`, `DisplayEncoding`,
then `OutputTransform` (JMh → chroma compress → gamut compress → tables). Carry
the Apache-2.0 SPDX headers and record the derivation in `NOTICE`.

### R5 — Eliminate `Op::Custom` from the shipped path. *(with R4)*

Express the output transform as ops, or add a first-class
`Op::AcesOutput { params }` that serialises, hashes and emits. Until then the
WGSL emitter of doc 10 §R3 cannot be built and no view transform can be cached
by value.

### R6 — Run the `aces-output` conformance procedure, publishing a self-defined threshold. *(after R4)*

Mirror the images. Publish per-configuration results, not a single verdict.
State plainly that the metric and the threshold are CINE-OMNI's own because ACES
publishes none.

### R7 — Implement CLF. *(weeks)*

`.cube` alone is not enough for interchange with ACES or with facility
pipelines. See §11.

### R8 — Fix the language now, everywhere. *(hours)*

Crate description, module docs (`aces.rs`'s "Daniele-style" understatement and
the "ACES / OpenColorIO-native" overstatement), UI strings. Free, and it is the
difference between a defensible product and an indefensible one.

---

## 11. LUT and interchange formats

### 11.1 CLF — the one that matters, and it just became a SMPTE standard

**Identity, verified.** CLF was Academy specification **S-2014-006**; version 3
shipped with ACES 1.2 in 2020, with editorial updates in 2021. It is now
standardised as **SMPTE ST 2136-1, "Common LUT Format"**. The old
`ampas/CLF` sample-implementation repository is **explicitly marked
`[OBSOLETE]`** in its own title.

**Availability — free to read, not free to redistribute.** SMPTE runs a public
GitHub repository, `SMPTE/st2136-1`, which published a **Public Committee
Draft** for review ending **2025-05-31**:

* `10E-CD-ST-2136-1-Common-LUT-Format-2025-02-06.pdf`
* **`st2136-1a-202x.xsd`** — the XML Schema (14,897 bytes)
* `st2136-1b.clf`, `st2136-1c.clf`, `st2136-1d.clf`, `st2136-1e.clf` — sample files

`CONFIDENTIALITY.md` states: *"This respository is not SMPTE Confidential
Information and is not subject to the confidentiality policy specified by the
SMPTE Standards Operations Manual."* — so reading and implementing from it is
unambiguously fine.

**But `LICENSE.md` is the whole licence, and it is restrictive:**

> Copyright © by the Society of Motion Picture and Television Engineers. All
> rights reserved. **No part of this material may be reproduced, by any means
> whatsoever, without the prior written permission of the Society of Motion
> Picture and Television Engineers.**

**Rule: implement from the schema; do not vendor the schema or the sample
`.clf` files into the repository.** Write CINE-OMNI's own test fixtures. The
`PATENTS.md` is SMPTE's boilerplate patent-rights disclaimer — no grant, no
identified claims.

**Backwards compatibility is a namespace change, and that is a gift.** The
README states it verbatim: *"Existing CLF instances based on [the Academy
specification] can be converted into the SMPTE-compliant CLF format by only
changing or adding the default namespace
`xmlns="http://www.smpte-ra.org/ns/2136-1/2024"`."* A reader that ignores the
namespace, or accepts both, reads Academy CLF and SMPTE CLF with one code path.

**The structure, from the PCD's own sample file** (`st2136-1b.clf`, quoted
whole because it is the smallest complete correct example):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<ProcessList xmlns="http://www.smpte-ra.org/ns/2136-1/2024">
	<Id>urn:uuid:be08cf40-f83c-49b7-8233-183a85a32733</Id>
	<Description>ACES2065-1 to ACEScg</Description>
	<InputDescriptor>ACES2065-1</InputDescriptor>
	<OutputDescriptor>ACEScg</OutputDescriptor>
	<Info>
		<ACESuserName>ACES2065-1 to ACEScg</ACESuserName>
	</Info>
	<Matrix inBitDepth="16f" outBitDepth="16f">
		<Array dim="3 3">
			1.451439316146 -0.236510746894 -0.214928569252
			-0.076553773396  1.176229699834 -0.099675926438
			0.008316148426 -0.006032449791  0.997716301365
		</Array>
	</Matrix>
</ProcessList>
```

The `ProcessNode` types are **LUT1D, LUT3D, Matrix, Range, Log, Exponent and
ASC_CDL**. Every node carries required `inBitDepth` and `outBitDepth` and
optional `id` / `name`; `LUT1D` adds `interpolation`, `halfDomain` and
`rawHalfs`; `LUT3D` adds `interpolation`; `Range`, `Log`, `Exponent` and
`ASC_CDL` carry a `style`, with `LogParams` / `ExponentParams` /
`SOPNode` + `SatNode` children.

**Parsing pitfalls, in the order they will bite:**

1. **`inBitDepth`/`outBitDepth` are scale factors, not storage hints.**
   Permitted values are `8i`, `10i`, `12i`, `16i`, `16f`, `32f`. Integer depths
   scale by `2ⁿ − 1`; float depths scale by `1.0`. A node's array values are
   expressed *in its own bit depth*, so a `Matrix` with `inBitDepth="10i"` has
   coefficients premultiplied by `1023`. **Ignoring these is the single most
   common CLF bug**, and it produces results wrong by exactly a power-of-two-ish
   factor rather than subtly wrong — which is at least easy to spot.
2. **Adjacent nodes must agree.** Node *n*'s `outBitDepth` should equal node
   *n+1*'s `inBitDepth`. Files in the wild do not always comply.
3. **`halfDomain` LUT1Ds have exactly 65536 entries** indexed by the bit
   pattern of a `half`, not by a normalised position — including infinities and
   NaNs. A LUT1D reader that assumes uniform spacing produces garbage on these,
   and they are the standard way to represent a log curve exactly.
4. **`rawHalfs` means the array's integer text is the *bit pattern* of a half**,
   to be reinterpreted, not converted. Reading `15360` as `15360.0` instead of
   `1.0` is a silent catastrophe.
5. **The SMPTE draft uses `<Id>` as a child element** of `ProcessList`, while the
   Academy specification documents `id` as an *attribute*. Accept both.
6. **`Array dim="3 3"`** — a 3×3 matrix is `dim="3 3"`, a 3×4 is `dim="3 4"`,
   and a LUT3D is `dim="n n n 3"`. The dimension list, not the element name,
   determines the shape.
7. **XML entity expansion and external entities must be disabled** in whatever
   parser is used. A colour LUT is an untrusted file from a facility.

**Recommendation: implement CLF read and write.** It is the only LUT format
that is (a) a real standard, (b) expressive enough to carry a whole transform
chain rather than a single baked cube, (c) what ACES itself uses for
interchange, and (d) a near-exact match for `cine-color`'s existing `Op` set —
`Matrix`, `Range`, `Log`, `Exponent`, `ASC_CDL`, `Lut1D`, `Lut3D` map
one-to-one onto seven of the ten `Op` variants. **CLF is, in effect, a
serialisation format for `Processor` that already exists.** That also makes it
the natural fix for `Op::Custom`'s non-serialisability: an op chain that can be
written as CLF can be written at all.

**OpenColorIO's own CLF reader has already been updated for SMPTE, and its
source header is the best short guide to the format that exists.** Read from
`src/OpenColorIO/fileformats/FileFormatCTF.cpp` and
`fileformats/ctf/CTFTransform.cpp` on `main` — four things worth taking
verbatim:

1. **On versioning strategy** — *"In 2026, SMPTE will publish ST 2136-1 to
   standardize the Academy/ASC format. The main change is how versions are
   declared. The SMPTE spec sets the xmlns attribute of the ProcessList to a
   specific value rather than using the compCLFversion attribute. Since the
   differences are so minimal, **OCIO writes both the xmlns and compCLFversion
   in order to maximize compatibility with different readers**."*
   `CTFTransform.cpp` carries
   `static constexpr const char* SMPTE_XMLNS_URL = "http://www.smpte-ra.org/ns/2136-1/2024";`
   and validates `Id` values against ST 2136-1's rule
   (*"'…' is not a SMPTE ST 2136-1 compliant Id value"*). **CINE-OMNI should
   copy this exactly: accept either, write both.** U7 is closed by this.
2. **On CLF and certification** — *"Support for CLF is a **requirement** in
   order to obtain ACES Logo Certification from the Academy (in several
   product categories)."* That raises R7 from "nice interoperability" to a
   prerequisite for any future certification claim (§2.5 Tier 3).
3. **On CTF, and this is the answer to `Op::Custom`** — *"Autodesk CTF was also
   designed to be able to **losslessly serialize any OCIO Processor to a
   self-contained XML file** … all native OCIO ops have a lossless mapping into
   CTF as XML process nodes."* CTF is a superset of CLF sharing one parser;
   the two are told apart by the root attribute (`compCLFversion` for CLF,
   `version` for CTF). **A CLF/CTF writer is precisely the serialisation
   `Processor` currently lacks**, and building one forces the `Op::Custom`
   problem to be solved rather than deferred.
4. **On the bit-depth trap, more precisely than the specification summary** —
   *"These bit-depths DO NOT specify the processing precision, nor do they
   specify the bit-depth of the images … The only function of these bit-depth
   attributes is to interpret the scaling of the parameter values in a given
   process node … They must always be present and must match at the interface
   between adjacent process nodes … **although the bit-depths imply a certain
   scaling, they never impose a clamping or quantization**."* And a case worth
   knowing: *"the ASC_CDL parameters are always stored in normalized form and
   hence the bit-depths, while required, do not affect their interpretation.
   On the other hand, the interpretation of the parameters in a Matrix op is
   affected by both the in and out bit-depths."*

### 11.2 The other LUT formats

| Format | Specification status | Worth implementing? |
|---|---|---|
| **`.cube`** (Iridas / Adobe) | Adobe published a "Cube LUT Specification 1.0"; the format is otherwise conventional. **Already implemented** (1D and 3D, `TITLE`, `DOMAIN_MIN`/`MAX`). | **Done.** Keep. It is the universal lowest common denominator. |
| **`.cube`** (Resolve) | Same keywords, different default grid size (OCIO's bakers default 32³ for Iridas, 64³ for Resolve — doc 10 §A.5.3). | Same reader. Only the *writer* needs to know which flavour. |
| **`.clf` / `.ctf`** | **SMPTE ST 2136-1** (CLF); CTF is Autodesk's superset that OCIO also reads. | **Yes — highest priority (R7).** |
| `.spi1d` / `.spi3d` | Sony Pictures Imageworks; no published specification found — the de facto definition is OCIO's reader. | Low priority. Read-only if a user asks. |
| `.3dl` (Lustre / Flame / Nuke) | No single specification; the variants differ in header, mesh size and integer scaling. A classic source of silent 10-bit-vs-16-bit errors. | Low priority, and **refuse rather than guess** when the flavour is ambiguous. |
| `.csp` (Rising Sun Cinespace) | Documented informally; notable for carrying a **shaper** natively. | Low priority. |
| `.vf`, `.mga`, `.look`, `.icc` | Vendor formats / a different model entirely. | No. |

**Pitfalls common to the text formats**, all of which have produced real bugs:

* **CRLF line endings** and a UTF-8 BOM. A naive `split_whitespace` survives
  both; a naive `lines().map(parse)` does not.
* **`DOMAIN_MIN` / `DOMAIN_MAX` are not always `[0,1]`.** An HDR `.cube` may
  declare `DOMAIN_MAX 4 4 4`. Ignoring the domain silently rescales the LUT.
  *`cine-color` does parse both* — verified in `lut.rs`.
* **Values may exceed 1.0** on both input and output. Do not clamp on load.
* **Neither `LUT_1D_SIZE` nor `LUT_3D_SIZE` present**, or *both* present.
  `cine-color` raises a named error for both cases — verified, and correct.
* **Index order in a 3D `.cube`: red varies fastest.** Getting this backwards
  gives an image that looks plausible and is transposed in colour.
* **Comments (`#`) mid-file**, and `TITLE` strings containing quotes.

### 11.3 ASC CDL

`cine-color` already implements CDL and its XML carriers (`grade.rs`,
`parse_cdl_xml` / `write_cdl_xml`). Two things to verify before claiming
conformance, neither yet checked:

* The saturation luma weights. CDL's specification defines saturation about a
  specific luma; `cine-color`'s `Op::Saturation` doc comment says *"Saturation
  about Rec.709 luma"*. **Whether the ASC specification mandates Rec.709
  weights is `UNVERIFIED`** — and if it does not, the crate is applying a
  convention, not the standard.
* The clamping rule. `slope·x + offset` may go negative; the power function is
  undefined there. What the ASC specification says to do is **UNVERIFIED**;
  `cine-color`'s `Op::Exponent` doc says *"negatives clamp to 0"*, which is a
  choice that must be checked against the text.

The ASC CDL specification itself is *"ASC Color Decision List (ASC CDL) Transfer
Functions and Interchange Syntax"*; **its free availability from theasc.com was
not verified** — `UNVERIFIED`.

### 11.4 OpenEXR — and the finding that unblocks conformance

**This is the item that gates the entire ACES conformance plan (R6), and it is
in far better shape than expected.**

The ACES reference images are EXR (`aces-output/tests/images/01 ACES/…`), and
ACES's container is OpenEXR (ST 2065-4). Without an EXR reader, none of the
conformance work is possible. Options, from crates.io (fetched 2026-09-08):

| Crate | Version | Licence | Last release | Downloads (total / recent) | Native deps |
|---|---|---|---|---:|---|
| **`exr`** (`exrs`) | **1.74.2** | **BSD-3-Clause** | **2026-07-10** | **69,708,541 / 17,069,063** | **none — pure Rust** |
| `openexr` | 0.11.0 | BSD-3-Clause | 2021-10-21 | 23,695 / 304 | C++ OpenEXR 3.0.5 |
| `openexr-sys` | 0.10.1 | BSD-3-Clause | 2021-09-01 | 23,244 / 341 | C++ OpenEXR 3.0.5 |

`exr`'s own description is *"Read and write OpenEXR files without any unsafe
code"*, and its README lists *"no unsafe code, no undefined behaviour"* and
*"no CMake required or environment variables required"*. Coverage, from the
README's own checklist:

* **Supported:** custom attributes; multi-part (multi-layer) images;
  multi-resolution (mip/rip maps); independent access to metadata and raw pixel
  blocks; f16 / f32 / u32 channels; uncompressed, zip line, zip block, RLE,
  PIZ, PXR24, B44/B44A, and **DWAA/DWAB**; parallel compress/decompress.
* **Not supported:** **channel subsampling**, **deep data**, **HTJ2K32 /
  HTJ2K256**.
* *(Note a stale contradiction in the README: the prose "Current Status"
  paragraph still says DWA "not supported yet" while the feature checklist marks
  `dwaa, dwab` as done. The checklist is the newer text.)*

**None of the unsupported features is needed for ACES conformance images**,
which are ordinary non-deep, non-subsampled RGB EXRs. **`exr` is the
dependency.** It is BSD-3-Clause, actively maintained, enormously used, and adds
no C++ to the build — which preserves the property that makes `cine-color`
valuable.

**But EXR is only half of it, and this is easy to miss.** Re-reading
`aces-output/tests/images/README.md` directly: the *inputs* (`01 ACES/`) and the
inverse results (`03 Inverse_on_Output/`) are **EXR**, but the golden *outputs*
(`02 Output_on_ACES/`, `04 Output_on_Inverse/`) are **TIFF** — the README states
the naming rule explicitly: *"applying `SomeTransformName.ctl` on input image
`SomeImageName.exr` would be named `SomeImageName_SomeTransformName.tiff`."* So
the conformance harness needs a **16-bit TIFF reader as well**:

| Crate | Version | Licence | Last release | Downloads (total / recent) |
|---|---|---|---|---:|
| **`tiff`** (`image-rs/image-tiff`) | **0.11.3** | **MIT** | 2026-02-10 | 114,645,077 / 30,434,404 |

Also pure Rust. **Two permissive, pure-Rust crates — `exr` and `tiff` — are the
entire file-format cost of ACES conformance.**

**And the absent tolerance is re-confirmed.** The README gives the four-step
procedure verbatim (*"Compare the output from your implementation to the
provided corresponding 'output' reference image"*) and a full file manifest, and
contains **no occurrence of "tolerance", "threshold", "delta", "precision" or
any numerical pass criterion**. Doc 10 recorded this as a verified absence; it
is re-verified here. The images are still Dropbox-hosted (the README links a
`dropbox.com/scl/fo/…` folder), so mirroring remains necessary.

The upstream C++ **OpenEXR is BSD-3-Clause** (`LICENSE.md`, read: *"Copyright
(c) Contributors to the OpenEXR Project. All rights reserved."* plus the three
standard clauses and the standard disclaimer, with **no fourth clause and no
patent grant**). It is not needed.

**Consequence: doc 10's `UNVERIFIED` on whether conformance is reachable is
answerable — it is, with one pure-Rust dependency.** The remaining obstacles to
R6 are the image hosting and the absent tolerance, not the file format.

### 11.5 Scopes, range and display calibration

Deliberately short, because these are not decision points for Phase 2.

* **Scopes measure code values, not normalised floats, and the graticule must
  say which.** A waveform showing "0–100 IRE" over a full-range signal and one
  showing it over a limited-range signal are different instruments. `scopes.rs`
  already has `hdr_waveform_nits`, which is the right idea: **name the unit on
  the axis**.
* **Legal vs full range at the scope is a display decision, not a data
  decision.** Doc 10 §A.2.2 establishes that superblacks and superwhites must be
  preserved through the pipeline; the scope's job is to *show* that they exist,
  with the 64/940 (10-bit) limits drawn as reference lines rather than as
  clipping.
* **Display calibration is out of scope for Phase 2 and should be stated as
  such.** The relevant open-source tooling (ArgyllCMS, DisplayCAL) is **GPL**,
  which is incompatible with linking into an Apache-2.0 product — *the exact
  licences were not fetched;* `UNVERIFIED`, but the direction is well enough
  known that the correct plan is **interoperate by reading the profile or LUT
  those tools produce, never link them**. Reading a 1D/3D calibration LUT is
  already within `cine-color`'s abilities.
* *Whether SMPTE RP 219 (colour bars), ITU-R BT.1729 or EBU Tech 3325 define
  anything CINE-OMNI must match is* **UNVERIFIED** *and was not investigated.*

---

## 12. UNVERIFIED register

Everything below is an open item. Each is cited by name where it governs, and
no claim in this document depends on one without saying so.

### 12.1 Closed by this document

Recorded because a closed item is as useful as an open one, and doc 10 marked
several of these open.

| Item | Resolution |
|---|---|
| Pre-2.0 ACES licence (doc 10 §A.6.1) | **Closed.** `v1.3.1` and earlier are the AMPAS "License Terms for Academy Color Encoding System Components". Apache-2.0 arrived in commit `ee380496`, committed **2025-09-25**. §2.1 |
| Licence of `OpenColorIO-Config-ACES` (doc 10 §A.5.4) | **Closed.** BSD-3-Clause, **byte-identical** to OpenColorIO's `LICENSE`. §2.7 |
| Licences of OCIO's required dependencies (doc 10 §A.5.1) | **Closed for five of six.** Imath BSD-3, pystring BSD-3, yaml-cpp MIT, zlib zlib, expat MIT. minizip-ng still open (U2). §3.4 |
| Whether image readers for the ACES conformance set are available without C++ | **Closed.** `exr` 1.74.2 (BSD-3-Clause, no unsafe) for the EXRs and `tiff` 0.11.3 (MIT) for the golden TIFFs — both pure Rust. §11.4 |
| Licences of `aces-output`, `aces-input-and-colorspaces`, `aces-look`, `aces-amf` | **Closed.** All Apache-2.0; `aces-output`'s `LICENSE` is **md5-identical** to `aces-core`'s (`2b42edef8fa55315f34f2370b4715ca9`). |
| CLF's standard status and availability | **Closed.** Now **SMPTE ST 2136-1**; Public Committee Draft, schema and samples public on `SMPTE/st2136-1`; **but SMPTE's `LICENSE.md` forbids reproduction**, so implement from it and do not vendor it. §11.1 |
| Whether OCIO can be built in this environment | **Closed — it can, completely.** Configure exits 0 with `zig c++`; all six dependencies and all ~180 OCIO translation units compile with zero errors; the shared library links once `-Wl,--exclude-libs,ALL` is removed (zig ld does not support it); and a linked test program loads the ACES 2.0 config and returns transform results. §3.4, §3.5 |
| Whether CINE-OMNI's output transform agrees with ACES 2.0 | **Closed — measured.** Exact to 6 dp on 18 % grey; worst-case max&#124;Δ&#124; **0.3029** over five samples. Finding 0b |
| Whether OCIO's shader layer can express separated texture/sampler bindings | **Closed — it already does**, for HLSL and MSL, with set/binding indices. §0 finding 7a |
| Whether `Op::Custom` is used outside `processor.rs` (doc 10 §A.5.3) | **Closed — it is**, at three sites, including both ACES output-transform entry points. §1.3 |
| Whether `cine-color` loads real OCIO configs | **Closed — it does not**, for any ACES config ever released. §4.0 |
| Whether OCIO's CLF reader handles the SMPTE ST 2136-1 namespace | **Closed — it does**, and it writes *both* `xmlns` and `compCLFversion`. Copy that policy. §11.1 |
| Whether an op chain like `Processor` has a standard serialisation | **Closed — CTF.** OCIO's source: *"designed to be able to losslessly serialize any OCIO Processor to a self-contained XML file."* §11.1 |

### 12.2 Still open

| # | Item | Why it matters |
|---:|---|---|
| U2 | **minizip-ng's licence**, and the licences of OCIO's two vendored `ext/` components, **`sampleicc`** and **`xxHash`**. No licence file found at the expected paths. | Only matters if OCIO is ever shipped, which §7 rejects. |
| U3 | **Whether OpenColorIO's yaml-cpp reader accepts `cine-color`'s unquoted builtin style strings** in flow mappings (§4.0). | Determines whether configs CINE-OMNI *writes* are readable by anyone else. Answerable only with a real OCIO. |
| U4 | **AP0/AP1 chromaticities against SMPTE ST 2065-1**, and **ACEScct constants against Academy S-2016-001** — the *standards texts*. Both are now cross-checked against a second independent implementation and match exactly (§2.5), so this is a provenance gap, not a suspected error. | A Tier-0 claim rests on them. |
| U5 | **Whether CINE-OMNI's `inverse_nits` is numerically identical to ACES `tonescale_inv`.** Different algebra, never compared. | Cheap test; gates the "we have the ACES 2.0 tonescale" claim in the inverse direction. |
| U6 | **Whether CINE-OMNI's `.min(peak_nits)` clamp placement matches ACES's clamp to `[0, peakLuminance]`.** | Same. |
| U8 | **The full semantic diff between `cine-color`'s 18 correctly-named builtins and OCIO's.** Finding 6b establishes a divergence in the display group's clamping; the other 12 were not checked op-by-op. | Every one is a potential silent-wrong-colour. R3b is the fix. |
| U9 | **The ASC CDL specification's free availability**, its saturation luma weights, and its rule for negative values before the power function. `cine-color` uses Rec.709 weights and clamps negatives to 0 — both may be conventions rather than the standard. | Gates the CDL conformance claim. §11.3 |
| U10 | **`aces-output`'s reference-image *content*.** Hosting (Dropbox) and the absence of any published tolerance are now re-verified (§11.4); the images themselves were not downloaded, so their exact bit depth, TIFF flavour and colour tagging are unknown. | Gates R6. The file-format obstacle is removed — `exr` + `tiff`, both pure Rust — but nothing has actually been read. |
| U11 | **Determinism of `cine-color`'s `rayon` paths.** | Contract property C6. |
| U12 | **Whether the ACES trademark is held by AMPAS or the ASWF after the transition**, and the ACES Logo Program criteria documents. Doc 10 marks these UNVERIFIED; unchanged. | Gates any certification language. |
| U13 | **Camera log encodings**: whether each manufacturer's published curve carries usage restrictions, and the licence of `aces-input-and-colorspaces`. | Gates the input-transform claims in §9. |
| U14 | **ArgyllCMS / DisplayCAL licences** (believed GPL, not fetched), and whether SMPTE RP 219 / ITU-R BT.1729 / EBU Tech 3325 impose anything on scopes. | §11.5; low priority, Phase 3 at the earliest. |
| U15 | **The exact semantics of OCIO's `GammaOpData` `MONCURVE_MIRROR_REV` and `BASIC_MIRROR_REV` styles** for negative values (the ` - MIRROR NEGS` display variants), which every ACES 2.0 config uses and `cine-color` does not implement. | Needed for R2/R3 to produce a correct view from a real config. |

---

## 13. Sources

Everything below was fetched, cloned, compiled or run on **2026-09-08**.

**ACES**
* `aces-aswf/aces-core` — full clone of `main`; `LICENSE` (Apache-2.0, no
  appended terms), `README.md` (`SPDX-License-Identifier: Apache-2.0`),
  `CHANGELOG.md` (Version 2.0, April 4 2025), and `git log -- LICENSE` /
  `git show --stat ee38049`
* `raw.githubusercontent.com/aces-aswf/aces-core/v1.3.1/LICENSE.md` — the AMPAS
  terms, read in full (49 lines)
* GitHub API: `repos/aces-aswf/aces-core` (metadata, `license.spdx_id`),
  `…/tags` (newest `v1.3.1`), `…/releases` (empty array)
* `lib/Lib.Academy.{Tonescale,OutputTransform,DisplayEncoding,ColorSpaces,Utilities}.ctl`
  — read for `init_TSParams`, `tonescale_fwd`/`_inv`, `outputTransform_fwd`/`_inv`,
  `init_ODTParams`, the table constants and the appearance-model constants

**OpenColorIO**
* Full clone of `main` at `5a808fb57a94c7229640a97835c420c9a1fbd1fe` (2026-07-09);
  `LICENSE` (BSD-3-Clause, read in full), `CMakeLists.txt`
  (`project(OpenColorIO VERSION 2.6.0)`, `cmake_minimum_required(VERSION 3.14...4.0)`),
  `share/cmake/utils/CppVersion.cmake`
* `src/OpenColorIO/transforms/builtins/*.cpp` — every `registry.addBuiltin("…")`
  plus the `aces2_output_transforms` table; `Displays.cpp` for the sRGB and
  gamma-2.2 display builtins
* `src/OpenColorIO/ops/gamma/GammaOpCPU.cpp` — `GammaBasicOpCPU::apply`,
  `GammaMoncurveOpCPURev::apply` (the clamping semantics)
* `src/OpenColorIO/GpuShaderUtils.cpp` — `getTexDecl<N>`, `getTexSample<N>`,
  and the per-primitive `switch (m_lang)` layer;
  `include/OpenColorIO/{OpenColorIO.h,OpenColorTypes.h}` — `GpuShaderCreator`,
  `GpuLanguage`
* `src/OpenColorIO/ops/fixedfunction/ACES2/*` and `FixedFunctionOpGPU.cpp`,
  `FixedFunctionOpData.h` — OCIO's own ACES 2.0 implementation and its GPU path
* `src/OpenColorIO/builtinconfigs/` — `BuiltinConfigRegistry.cpp` (the
  `ocio://` URIs) and `configs/*.ocio` (the eight shipped configs)
* Dependency licences read from the files OCIO's build downloaded:
  `ext/build/{Imath,pystring,yaml-cpp,zlib,libexpat}/…/{LICENSE*,COPYING}`
* `raw.githubusercontent.com/AcademySoftwareFoundation/OpenColorIO-Config-ACES/main/LICENSE`
  — diffed byte-for-byte against OCIO's

**CLF**
* `github.com/SMPTE/st2136-1` — `README.md`, `LICENSE.md`, `PATENTS.md`,
  `CONFIDENTIALITY.md`, `st2136-1a-202x.xsd`, `st2136-1b.clf`
* `docs.acescentral.com/clf/specification/` — CLF v3.0 ProcessNode list,
  attributes and bit-depth scaling

**Rust crates** — crates.io API for `exr`, `openexr`, `openexr-sys`, `ocio-rs`,
`ocio-sys`, `opencolorio`, `opencolorio-sys`;
`raw.githubusercontent.com/johannesvollmer/exrs/master/README.md`;
`raw.githubusercontent.com/AcademySoftwareFoundation/openexr/main/LICENSE.md`

**CINE-OMNI itself** — all repository facts read from the working tree;
`crates/cine-color/{src/*.rs,tests/*.rs}`, `tests/fixtures/mini.ocio`. The
config-loading measurement of §4.0 and the writer-fidelity measurement were
**run**, in a throwaway crate depending on `cine-color` by path, against the
three `.ocio` files extracted from OpenColorIO's tree.

**This environment** — `zig 0.16.0` at `~/.local/zig/zig`; C and C++17
compile-and-run tests through `~/.local/bin/{zigcc,zigcxx}`;
`uv tool install cmake` (4.4.3) and `ninja` (1.13.2); the OCIO CMake
configuration and full build logged to the scratchpad, producing
`libOpenColorIO.so.2.6.0`.

**The oracle** — a 40-line C++ program linked against that library, loading
`ocio://cg-config-v4.0.0_aces-v2.0_ocio-v2.5` and evaluating
`DisplayViewTransform(ACEScg → "sRGB - Display" / "ACES 2.0 - SDR 100 nits
(Rec.709)")` and `BuiltinTransform("ACES-OUTPUT - … - SDR-VIDEO_1.0")` on fixed
inputs. Its output is the "OpenColorIO" column of finding 0b; the "cine-color"
column came from `OutputTransform::for_display(Display::SRgb)` in both hue
modes. Both programs are reproducible from the commands recorded in §3.4/§3.5.

**Not obtained, and therefore not quoted:** SMPTE ST 2065-1 and ST 2065-4;
Academy S-2016-001 (ACEScct); the ASC CDL specification; the ACES Logo Program
criteria documents; the `aces-output` reference images; minizip-ng's,
sampleicc's and xxHash's licences; ArgyllCMS's and DisplayCAL's licences; and
every manufacturer log-curve white paper. Each is cited by name where it
governs, and every claim that would have depended on one is marked
`UNVERIFIED`.

---

*Document status: complete. Every measurement reported was run on this machine
on 2026-09-08; every licence quoted was read from the file it is quoted from.*
