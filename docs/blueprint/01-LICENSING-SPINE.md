# 01 — The Licensing Spine

**Status:** the governing rule is settled and verified. The per-project
inventory is partial pending the blocked research clusters.

> **This is not a legal opinion.** It is an engineering analysis of publicly
> stated license terms, written so that an engineer knows which integration
> paths are viable before writing code. Directive §42 requires that this be
> understood *before* any code is merged. Anything that reaches distribution
> must be confirmed by counsel.

---

## 1. The governing fact

CINE-OMNI declares, in `Cargo.toml` under `[workspace.package]`:

```toml
license = "Apache-2.0"
```

Everything in this document follows from that one line. It was chosen before
this engagement and it is the correct default for a platform intended to be
embedded in studio pipelines. But it has hard, asymmetric consequences.

## 2. The compatibility table

| Direction | Permitted? | Why |
|---|---|---|
| Apache-2.0 code → into a GPL-3.0 project | **Yes** | Apache-2.0 is one-way compatible with GPLv3 |
| GPL-3.0 code → into Apache-2.0 CINE-OMNI | **No** | the combined work would have to be distributed under GPLv3 |
| GPL-2.0-**only** code + Apache-2.0 code | **No** | outright incompatible; Apache's patent-termination terms are an "additional restriction" under GPLv2 |
| LGPL-2.1+ library, **dynamically** linked | **Conditional** | permitted if the user can relink; imposes real obligations, see §4 |
| MIT / BSD-2 / BSD-3 / Zlib / BSL-1.0 | **Yes** | attribution only |
| MPL-2.0 | **Yes**, file-level | modified MPL files remain MPL; no viral spread to the rest |

### The rule this produces

> **No GPL source may be copied into, statically linked into, or dynamically
> linked into CINE-OMNI.**

A GPL project can therefore only ever be:

- **TYPE C** — a separate process behind a clean interface,
- **TYPE D** — an external application driven over a stable protocol,
- **TYPE E** — file interchange only,
- **TYPE G** — a design studied and independently reimplemented. *Ideas are not
  copyrightable; expression is.* This path is legitimate and valuable, but it
  requires discipline: study architecture and behaviour, never transcribe code.

## 3. What this decides immediately

| Project | License | Consequence for CINE-OMNI |
|---|---|---|
| **Blender** | GPL | Cannot be linked. Not "Blender in a tab". Only TYPE C/D via headless process + USD interchange, or TYPE G for its architectural ideas. The research file argues the durable value is the ideas, not the binary. |
| **Kdenlive** | GPL-3.0 | Cannot be linked. TYPE G at most. |
| **Natron** | GPL | Cannot be linked. TYPE G. |
| **Ardour** | GPL | Cannot be linked. TYPE G. |
| **FFmpeg** | LGPL-2.1+ core, GPL only with `--enable-gpl` | **Usable** — see §4, this is the important nuance |

## 4. The FFmpeg finding — the most consequential verified result so far

FFmpeg's own legal page states it is *"licensed under the GNU Lesser General
Public License (LGPL) version 2.1 or later"* with optional parts under GPL, and
that *"if those parts get used the GPL applies to all of FFmpeg"*
(verified at <https://www.ffmpeg.org/legal.html> and
<https://github.com/FFmpeg/FFmpeg/blob/master/LICENSE.md>).

The research established something more useful than the headline: **the GPL
surface inside FFmpeg is small, enumerable, and contains no codec CINE-OMNI
needs.** The GPL files are a handful of optional x86 assembly routines, some
build and test tooling, and roughly 33 `libavfilter` filters. Every decoder and
encoder in the set CINE-OMNI requires sits in the LGPL core.

The GPL exposure in practice comes from *external* libraries that `--enable-gpl`
unlocks (x264, x265 and similar), not from FFmpeg itself.

**Therefore an LGPL-only FFmpeg build, dynamically linked, is a viable media
foundation for an Apache-2.0 product** — subject to the LGPL-2.1 §6 obligations:
the user must be able to replace the library, which in practice means dynamic
linking, shipping the license text, and not statically linking it into the
binary. For H.264/H.265 *encoding* specifically, the answer is to use hardware
encoders or permissively licensed encoders rather than x264/x265.

**Patent and royalty exposure is a separate axis from copyright licensing** and
must be assessed independently. AV1, VP9 and ProRes decode have different
positions from H.264/H.265 encode. This has not yet been analysed and is a
tracked risk.

## 5. What is still unverified

Per §60, these are named rather than guessed. All are blocked on the rate-limited
research clusters:

- OpenUSD's exact license terms and their compatibility (directive §34). Widely
  described as a modified Apache 2.0. **RESOLVED — and it is not Apache-2.0.**
  Since release 24.08 (2024-07-25) OpenUSD is under the **Tomorrow Open Source
  Technology License 1.0**: bespoke, non-OSI, non-SPDX, and Apache-2.0 with a
  *narrowed* §6. ⚖️ LEGAL REVIEW REQUIRED before any dependency. The pure-Rust
  `openusd` crate is plain MIT and carries no such question.
- OpenColorIO, OpenEXR, OpenImageIO, Imath — **RESOLVED, all permissive.**
  OpenColorIO and OpenEXR are BSD-3-Clause; **OpenImageIO relicensed to
  Apache-2.0 in 2023** (>99.86% of lines), which makes it the one major C++
  imaging library whose source may be *ported* rather than merely linked.
- OpenFX plug-in API terms. **RESOLVED: BSD-3-Clause.** Hosting OFX is
  licence-clear. Deferred on engineering grounds, not legal ones.
- CLAP vs LV2 vs VST3 for audio. **RESOLVED, and the trap has moved.** The
  **VST 3 SDK is MIT** as of VST 3.8 (2025-10-29); Steinberg's own README
  states that GPLv3 and proprietary licensing "is no longer available". VST3 is
  therefore hostable by an Apache-2.0 product. Note `nih-plug`'s VST3 export
  pulls GPL-3.0 `vst3-sys`; use `vst3` (coupler-rs, MIT) instead. CLAP (MIT)
  and LV2 (ISC) remain clear.
- ASIO (Windows audio) licensing. **RESOLVED, and it is now the trap**:
  Steinberg moved the ASIO SDK to dual GPLv3 / proprietary in the same round.
  It cannot be linked by an Apache-2.0 product.
- Dolby Atmos versus open ADM BWF for immersive audio delivery. **The open path
  is clear**: `libadm` and `libbw64` are Apache-2.0, the EBU ADM Renderer is
  BSD-3-Clause-Clear.
- Codec patent pools, as distinct from source licenses. **Still open** — see
  `PHASE-3-STATUS.md` §5. A BSD-2 H.264 decoder settles copyright and says
  nothing about patents. ⚖️ LEGAL REVIEW REQUIRED before distribution.
- Camera RAW. **RESOLVED unfavourably**: `rawloader`/`rawler` are LGPL-2.1 and
  `imagepipe` is LGPL-3.0-only. Rust links statically, so there is no LGPL
  §6(b) relinking escape — RAW stills ingest is *not* available on the terms
  usually assumed.

## 6. Obligations CINE-OMNI must discharge

Once anything is integrated, the project owes:

1. A machine-readable third-party inventory: project, version, URL, license,
   copyright holder, integration method, modifications, attribution and
   distribution obligations (§42).
2. Bundled license texts for every dependency that requires them.
3. For any LGPL component: dynamic linking and a documented relink path.
4. A written record of every TYPE G decision, showing the reimplementation was
   from behaviour and architecture rather than from source.
5. `cargo-deny` or equivalent wired into CI so a license regression fails the
   build rather than reaching a release.

None of these exist yet. Item 5 is cheap and should land in Phase 1.
