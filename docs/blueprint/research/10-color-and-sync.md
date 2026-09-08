# CINE-OMNI Industry Blueprint — 10: Colour-Managed Media Path, Time & A/V Sync

> **Deliverables 6 and 7 of the Phase 0 research plan** (compositing/imaging and
> colour science), extended into the time-and-sync work that Phase 2 depends on.
> Written to the standing rules of `docs/blueprint/README.md`: no licence from
> memory, no approximation called by the name of the standard it approximates,
> no performance or conformance claim that was not measured, and `UNVERIFIED`
> rather than a plausible fill.
>
> **Companion document:** `research/02-editorial-media.md` already covers FFmpeg's
> licensing spine, the hardware decode matrix, zero-copy into the renderer, and
> proxy/cache modelling as UCG nodes. This document does not repeat any of it and
> builds on its proposed `cine-media` crate and `media.source` / `media.proxy`
> entity scheme.
>
> **All fetches and measurements: 2026-09-08.**

---

## 0. Executive summary

**Eight findings, in order of consequence.**

**1. ACES is now Apache-2.0, and that changes the entire colour strategy.**
`github.com/ampas/aces-dev` no longer exists — it redirects to
**`aces-aswf/aces-core`**, an Academy Software Foundation project whose `LICENSE`
is the **stock Apache License 2.0 with no appended AMPAS terms** (read to the end
specifically to check). The whole family — `aces-core`, `aces-output`,
`aces-input-and-colorspaces`, `aces-look`, `aces-amf`, and the `CTL` interpreter —
is Apache-2.0. **CINE-OMNI may legally port the official ACES 2 transforms into
Rust and WGSL and ship them.** The reason the analytic fit existed has gone away.
*(Pre-2.0 tags were not checked and may still carry the old terms —
`UNVERIFIED`.)*

**2. `RationalTime` is not rational, and it already fails.** `ucg-core`'s
`RationalTime` is two `f64`s, copied from OpenTimelineIO's own design. Measured
against the real crate over 200,000 frames rescaled 23.976 → 25 → 23.976:
**48,408 (24.2 %) fail `PartialEq`** and **24,201 (12.1 %) return the wrong frame
from `to_frames()`** — the first at frame 27. Because `RationalTime` derives
`PartialEq` and reactors depend on `tx.set` no-opping when a value is unchanged,
**this is a latent violation of the idempotence invariant that `CLAUDE.md` and
`MODULE-CONVENTIONS.md` both declare load-bearing**, not merely a display bug. It
has not fired only because nothing in the tree yet rescales across NTSC rates.

**3. There is no drop-frame support, and `from_timecode` is silently wrong.**
`from_timecode("01:00:00;00", 29.97)` accepts the drop-frame separator and
returns **108000** frames; the correct answer is **107892**. That is a silent
**3.6-second** error with no diagnostic. `to_timecode` rounds 29.97 to 30 and has
no drop-frame path at all.

**4. GLSL → WGSL transpilation is blocked, not merely risky — so emit WGSL from
the existing `Processor`.** OCIO is BSD-3-Clause (compatible) and at v2.5.2, so
licensing is not the obstacle. The obstacle is that every OCIO GLSL variant
declares LUTs as `uniform samplerND` — a Vulkan **combined image sampler** — and
**WGSL has no such type**. naga's GLSL frontend errors on it (the enabling PR was
closed unmerged in **August 2026**; the issue has been open since 2021); the
SPIR-V detour is blocked by the same thing (PRs `#4819`/`#4821` rejected on
principle in 2023, issue `#4323` open since 2021); Tint has **no GLSL reader**
at all; SPIRV-Cross has no WGSL backend. Meanwhile
`cine_color::processor::Op` is a **closed set of ten variants** — `Op::Custom`
appears **zero times** outside `processor.rs` — of which **four are already
implemented in WGSL** in `cine-gpu`. A WGSL emitter is a ten-arm match tested
against the CPU `Processor` as its own reference.

**5. `ColorMeta` cannot describe a video file.** It holds three strings and a
look. It has no `colour_primaries`, `transfer_characteristics`,
`matrix_coefficients`, `video_full_range_flag`, bit depth, chroma siting, ST 2086
or MaxCLL/MaxFALL. These are *file* facts, not artist choices, and they belong on
the media node.

**6. ACES publishes reference images but no tolerance.** `aces-output` ships
"golden" images and a four-step forward/inverse/round-trip comparison procedure,
but **no ΔE, PSNR or max-error threshold is published anywhere** — a verified
absence. Any CINE-OMNI conformance claim must publish **its own** metric and
threshold and say so. Separately, the ACES Logo Program is a **gated application
and review process**, not self-certification.

**7. HDR10+ is free and Dolby Vision is not — they must not be treated alike.**
HDR10+'s normative model *is* SMPTE **ST 2094-40**, which is **freely
downloadable from `pub.smpte.org` with no login** (as are ST 2084 and ST 2086 —
the belief that they are paywalled is wrong). There are no per-unit and no
per-title royalties, an MIT Rust implementation exists, and HDR10+'s published
fee table has a **Tool Vendor category — explicitly naming "encoder" — at
$0/year that grants Essential Patent Claims royalty-free**. CINE-OMNI is in that
category. Dolby Vision, by contrast, has **no pool to join** — though its
composer maths *are* freely and normatively specified, de-branded, by **ETSI GS
CCM 001**, which is the source to cite rather than Dolby's own
"Confidential"-stamped PDFs. Related correction: the famous 0.00002 / 0.0001
mastering-metadata units are the **H.264/H.265 SEI carriage**, not ST 2086
itself.

**8. The colour maths that exists is good.** PQ and HLG are implemented with the
exact BT.2100 fractional constants, HLG includes the OOTF and the
display-dependent system gamma (which most implementations omit), the compositor
is already premultiplied-linear throughout, and unsupported OCIO builtins raise a
**named error** rather than silently becoming identity. The gap is not the maths.
It is that **nothing decodes a frame**, and that the metadata model has nowhere
to put what a decoder would learn.

---

# PART A — THE COLOUR-MANAGED MEDIA PATH

The chain this section specifies, and where each transform belongs:

```
camera media
  -> demux                      cine-media : container, no pixels touched
  -> decode                     cine-media : codec -> planar samples + H.273 tags
  -> pixel format               cine-media : YUV/RGB, subsampling, bit depth, range
  -> matrix / range / siting    cine-gpu   : ONE fused shader, on the GPU
  -> transfer -> linear         cine-gpu   : from cine-color's Transfer op
  -> primaries -> working       cine-gpu   : from cine-color's Matrix op
  ============================================ scene-linear ACEScg from here
  -> grade / comp / VFX         cine-color, cine-comp : premultiplied, linear
  -> view transform             cine-color -> WGSL : output transform + display encode
  -> viewer                     display-referred code values
  -> editorial / VFX handoff    OTIO + CDL + baked LUT + AMF-shaped metadata
```

The boundary that matters is the double line. **Everything above it is a
property of the file and belongs on the media node. Everything below it is a
creative decision and belongs on the grade.** `ColorMeta` today models only the
second, which is why A.1.4 is a gap rather than an omission.

## A.1 Video colour metadata in containers

### A.1.1 The code points (ITU-T H.273 / ISO/IEC 23091-2)

Three enumerations plus one flag describe, completely, how to get from stored
integer samples to CIE colorimetry. They are defined once, in H.273, and
referenced by every modern codec and container.

**Naming correction:** the current ISO twin text is **ISO/IEC 23091-2**.
"ISO/IEC 23001-8" is the *old* number, and H.273's own Summary says so.

Values below were read from **Recommendation ITU-T H.273 (V4) (07/2024)** itself
(free from itu.int; editions 2016-12, 2021-07, 2023-09, 2024-07), cross-checked
against ITU-T H.264 (V16) Annex E Tables E-3/E-4/E-5 and against FFmpeg's
`libavutil/pixfmt.h`.

**`colour_primaries`**

| Value | Meaning |
|---:|---|
| 1 | BT.709 (also BT.1361, IEC 61966-2-4, SMPTE RP 177 Annex B) |
| **2** | **Unspecified** |
| 4 | BT.470 System M (FCC 73.682) |
| 5 | BT.470BG — BT.601 625-line, PAL/SECAM |
| 6 | SMPTE 170M — BT.601 525-line, NTSC |
| 7 | SMPTE 240M |
| 8 | Film (Illuminant C) |
| 9 | **BT.2020** |
| 10 | SMPTE ST 428-1 (CIE 1931 XYZ) |
| 11 | SMPTE ST 431-2 — **DCI-P3** |
| 12 | SMPTE ST 432-1 — **P3-D65 / Display P3** |
| 22 | G 0.295/0.605, B 0.155/0.077, R 0.630/0.340, D65. **H.273 says verbatim "No corresponding industry specification identified"** — the "EBU Tech 3213 / JEDEC P22" label is Matroska's and FFmpeg's, not the ITU's |

**`transfer_characteristics`**

| Value | Meaning |
|---:|---|
| 1 | BT.709 |
| **2** | **Unspecified** |
| 4 | Gamma 2.2 (BT.470M) |
| 5 | Gamma 2.8 (BT.470BG) |
| 6 | SMPTE 170M (functionally BT.709's curve) |
| 7 | SMPTE 240M |
| 8 | **Linear** |
| 9 | Logarithmic (100:1) |
| 10 | Logarithmic (100·√10 : 1) |
| 11 | IEC 61966-2-4 (xvYCC) |
| 12 | BT.1361 extended gamut |
| 13 | **IEC 61966-2-1 — sRGB / sYCC** |
| 14 | BT.2020 10-bit |
| 15 | BT.2020 12-bit |
| **16** | **SMPTE ST 2084 — PQ** |
| 17 | SMPTE ST 428-1 |
| **18** | **ARIB STD-B67 — HLG** (`a = 0.17883277`, `b = 0.28466892`, `c = 0.55991073`) |

**Two subtleties in this table that are routinely missed.**

*Value 13 is not simply "sRGB".* Its curve depends on `MatrixCoefficients`: with
**MC = 0** it is the IEC 61966-2-1 **sRGB** piecewise curve on `1 > Lc ≥ 0`;
**otherwise** the same curve extended symmetrically to negative `Lc`, i.e.
**sYCC**. H.273's NOTE 3 records this as a *later correction* — an earlier
version applied the sRGB form unconditionally, so older tooling gets sYCC wrong.

*Values 1, 6, 14 and 15 are the same curve*, and its constants are not 1.099 and
0.018. H.273 clause 8.2, verbatim: α = **1.099296826809442…**,
β = **0.018053968510807…** (α = 1 + 5.5·β). The familiar 1.099 / 0.018 are
BT.709's rounded values; BT.2020-2 rounds differently again (1.0993 / 0.0181 for
12-bit). H.273 NOTE 1 also stresses these are **OETFs**, and that **BT.1886 is
the corresponding EOTF** for flat-panel studio displays — a distinction that
matters the moment a viewer is built.

**`matrix_coefficients`**

| Value | Meaning |
|---:|---|
| 0 | Identity — **GBR**, i.e. the samples are already RGB (also sRGB, ST 428-1) |
| 1 | BT.709 |
| **2** | **Unspecified** |
| 4 | FCC |
| 5 | BT.470BG (BT.601 625) |
| 6 | SMPTE 170M (BT.601 525) — functionally identical to 5 |
| 7 | SMPTE 240M |
| 8 | YCgCo |
| 9 | **BT.2020 non-constant luminance** |
| 10 | BT.2020 **constant** luminance |
| 11 | SMPTE ST 2085 (Y'D'zD'x) |
| 12 / 13 | Chromaticity-derived NCL / CL |
| **14** | **ICtCp** (BT.2100) |
| 15 | SMPTE ST 2128 IPT-C2 ("developed in SMPTE as IPT-PQ-C2") |
| 16 / 17 | YCgCo-Re / YCgCo-Ro (`BitDepthC = BitDepthY + 1`) |

**H.273 NOTE 4 is a live trap:** *"In a previous version of this Specification,
the IEC 61966-2-1 sYCC representation was identified as corresponding to
MatrixCoefficients equal to 1. Closer study later determined that this
representation should correspond to MatrixCoefficients equal to 5."* Anything
written against an old CICP table may have sYCC mis-tagged as BT.709.

A smaller divergence worth knowing: H.273 gives SMPTE 240M as
`KR = 0.212, KB = 0.087`, while **mpv and libplacebo both hard-code
0.2122 / 0.7013 / 0.0865** (ST 240M's own values). Spec text and practice
differ, quietly.

Note that **`matrix_coefficients = 0` (Identity/GBR) is the flag that says there
is no YUV step at all** — the correct handling of RGB-in-a-video-container, and
a case a naive decoder gets visibly wrong.

**`video_full_range_flag`** — one bit, but the most consequential one in the
set. H.273 clause 8.3, verbatim: *"When not present or not specified, the value 0
for VideoFullRangeFlag would ordinarily be inferred as the default value for
video imagery."* See A.2.2.

**What a decoder is handed when the VUI is absent**, from H.264 Annex E.2.1
(verified from the ITU text):

| Syntax element | Inferred when absent |
|---|---|
| `colour_primaries` | **2 — unspecified** |
| `transfer_characteristics` | **2 — unspecified** |
| `matrix_coefficients` | **2 — unspecified** |
| `video_full_range_flag` | **0 — limited** |
| `video_format` | 5 — unspecified |
| `chroma_sample_loc_type_*` | **0 — left / vertically interstitial** |

Note the asymmetry: the colour *description* degrades to "unknown", but **range
and chroma siting degrade to concrete, usable defaults**. That is why A.1.3 is a
problem about colour and not about geometry.

### A.1.2 How they are carried

| Container | Mechanism |
|---|---|
| **MP4 / ISOBMFF** | The **`colr`** box, inside the sample entry (`stsd` → e.g. `avc1`/`hvc1`), carrying a `colour_type` FourCC. `'nclx'` is the modern form: `colour_primaries`, `transfer_characteristics`, `matrix_coefficients` (16 bits each) plus the `full_range_flag`. `'nclc'` is the **legacy QuickTime form with the same three fields and no range flag** — its absence is exactly why so much QuickTime material is range-ambiguous. `'rICC'`/`'prof'` embed an ICC profile instead. HDR static metadata rides in separate `mdcv` (mastering display) and `clli` (content light level) boxes. |
| **Matroska / WebM** | A structured `Colour` element under `TrackEntry` → `Video`, with children for `MatrixCoefficients`, `TransferCharacteristics`, `Primaries`, `Range`, `BitsPerChannel`, **`ChromaSubsamplingHorz`/`Vert`**, **`ChromaSitingHorz`/`Vert`**, `MaxCLL`, `MaxFALL`, and a `MasteringMetadata` sub-element. Matroska is the only one of the three that models **chroma siting as first-class metadata** rather than leaving it to the codec's VUI. |
| **MXF** | Not a box but a set of SMPTE **Universal Labels** on the Picture Essence Descriptor: `Transfer Characteristic`, `Coding Equations` (the matrix), `Color Primaries`, plus `Component Depth`, `Black/White Ref Level` and `Color Range` for the range question. The labels are registered in the SMPTE metadata registries (RP 224 lineage). MXF's model is *label-based*, so an unregistered or vendor label is a real possibility and must be handled as "unknown", not as a default. |

**The `colr` box syntax, verified** from MPEG's own File Format Conformance
dataset (`MPEGGroup/FileFormatConformance`, which encodes ISO/IEC 14496-12 7th
edition, 2022):

```c
class ColourInformationBox extends Box('colr'){
    unsigned int(32) colour_type;
    if (colour_type == 'nclx') {          /* on-screen colours */
        unsigned int(16) colour_primaries;
        unsigned int(16) transfer_characteristics;
        unsigned int(16) matrix_coefficients;
        unsigned int(1)  full_range_flag;
        unsigned int(7)  reserved = 0;
    } else if (colour_type == 'rICC') { ICC_profile; }   /* restricted */
      else if (colour_type == 'prof') { ICC_profile; }   /* unrestricted */
}
```

It is a plain `Box`, **not a `FullBox`** (no version/flags), and its container is
`VisualSampleEntry` — i.e. `moov → trak → mdia → minf → stbl → stsd → avc1/hvc1/av01 → colr`.
`mdcv` and `clli` sit in the same place and are likewise plain boxes:

```c
class ContentLightLevelBox extends Box('clli'){            // 12 bytes total
    unsigned int(16) max_content_light_level;              // MaxCLL
    unsigned int(16) max_pic_average_light_level;          // MaxFALL
}
class MasteringDisplayColourVolumeBox extends Box('mdcv'){ // 32 bytes total
    for (c = 0; c < 3; c++) { unsigned int(16) display_primaries_x, display_primaries_y; }
    unsigned int(16) white_point_x, white_point_y;
    unsigned int(32) max_display_mastering_luminance;
    unsigned int(32) min_display_mastering_luminance;
}
```

**The AV1-ISOBMFF binding carries a warning worth repeating verbatim:** *"the
`mdcv` and `clli` have identical syntax to the `SmDm` and `CoLL` boxes, except
that they are of type Box and not FullBox. However, the semantics … have
important differences and SHOULD not be confused."* It also states that a `colr`
box **overrides** the bitstream when the bitstream says "unspecified", and that
`full_range_flag` **shall match** the sequence header's `color_range`.

**QuickTime's `nclc` cannot express range, by construction.** Apple TN2162
defines only two `colorParamType`s — `'nclc'` and `'prof'` — and the `nclc`
payload is *three 16-bit indices and nothing else*. There is **no `nclx` and no
`rICC` in QuickTime**. FFmpeg's behaviour follows exactly: `mov_write_colr_tag`
writes `nclc` for MODE_MOV and `nclx` for MP4/AVIF, and the range bit is written
— and on read, parsed — **only for `nclx`**. **A full-range `.mov` is therefore
range-ambiguous by construction**, which is a large share of the "why is this
washed out" reports in the wild.

Two more Apple details that matter. TN2162's recommended SD tagging is
**6-1-6** (NTSC) and **5-1-6** (PAL) — note `transferFunction = 1` even for SD,
with the explicit statement that BT.470's 2.2/2.8 "assumed gamma" figures are
"both incomplete and obsolete". And TN2162's *wide-range* mapping ("Scheme A")
is **not** H.273's full-range mapping: it is signed two's-complement chroma with
a `2^n − 2` excursion, and Apple says outright that such data is **"not
representable using the labels described in this document."**

**MXF, verified from ST 377-1:2019** (free from `pub.smpte.org`). The three
colour items on the Generic Picture Essence Descriptor are all **optional**, and
all are 16-byte Universal Labels rather than CICP integers:

| Item | Local tag | Required? |
|---|---|---|
| Transfer Characteristic | `32.10` | Optional, default "unspecified" |
| Coding Equations | `32.1A` | Optional, default "unspecified" |
| Color Primaries | `32.19` | Optional, default "unspecified" |

The CDCI subclass adds `Component Depth` (33.01), `Horizontal`/`Vertical
Subsampling` (33.02 / 33.08), **`Color Siting` (33.03, default `FFh` =
unknown)**, and `Black Ref Level` / `White Ref Level` / `Color Range`
(33.04–33.06). ST 377-1's `Color Siting` enumeration is its own, not H.273's:
`00h` co-sited ("as in ITU-R Rec 601, SMPTE ST 314 4:1:1 or MPEG-2 4:2:2"),
`01h` horizontal mid-point, `03h` Quincunx ("as in MPEG-1 4:2:0"), `04h` Rec601
(**deprecated**, "New MXF encoders shall use 00h instead"), `05h` line
alternating, `06h` vertical midpoint ("as in MPEG-2 4:2:0 and H.264/AVC 4:2:0
(default)"), `FFh` unknown.

The ULs themselves are free from the SMPTE Metadata Registers, e.g.
`TransferCharacteristic_SMPTEST2084 = 060e2b34.0401010d.04010101.010a0000`,
`TransferCharacteristic_HLG_OETF = …010b0000`,
`CodingEquations_ITU2020_NCL = 060e2b34.0401010d.04010101.02060000`.
**There is no registered UL for BT.2020 *constant*-luminance coding equations** —
only NCL. That is a real, citable gap in MXF.

**IMF is where MXF's optionality is fixed.** SMPTE ST 2067-21 (Application #2E,
also free) makes Transfer Characteristic and Color Primaries *"shall be
present"*, Coding Equations *"shall be present if Y′C′BC′R sampling is used"*,
**`Color Siting` "shall be present and shall be `00h`"** (fully co-sited), and
Black Ref / White Ref / Color Range all mandatory with tabulated values
(10-bit: 64 / 940 / 897; 12-bit: 256 / 3760 / 3585). It also requires ST 2086
metadata to be **all-present or all-absent**, with *"if the items … are absent,
no ST 2086 metadata values are assumed."* **That is the model CINE-OMNI should
copy: mandatory, complete, or explicitly nothing — never partially inferred.**

*Still* **UNVERIFIED**: *the ISO clause numbering for `colr` (the syntax is
verified from MPEG's conformance dataset, the clause number is not — ISO's free
standards portal has closed), and the exact `rICC` restriction wording.*

### A.1.3 When the metadata is absent or wrong — which is the common case

This is not an edge case. It is the normal condition of real-world media, and a
media engine's behaviour here determines whether it is trusted.

**Why it happens:** H.264 makes the VUI optional and most encoders historically
omitted it; QuickTime's `'nclc'` has no range flag; screen recorders and phone
apps write nothing; and — the worst class — a great deal of material is
*tagged* BT.709 and *encoded* BT.601, or tagged limited-range and encoded full,
because it was transcoded by a tool that copied the tag without applying the
conversion.

**The QuickTime `gama` atom** deserves its own note, because it is the origin of
the classic "Mac gamma shift". It is a 16.16 fixed-point *"gamma level at which
the image was captured"* in the QuickTime File Format spec, and Apple deprecated
it: *"The 'colr' extension supersedes the previously defined 'gama'
ImageDescription extension. Writers … should never write both … and readers …
should ignore 'gama' if 'colr' is present."* TN2162 also names the real cause:
some Macintosh capture systems applied *"an additional transfer function to
incoming Y′CbCr data at capture time"*, and — verbatim — *"This operation is
currently not representable or supported by the 'colr' parameters."* FFmpeg's
position is instructive: `movenc.c` can write `gama` behind an opt-in
`write_gama` movflag ("Write deprecated gama atom"), and **the demuxer has no
`gama` handler at all** — it silently ignores the tag on read. CINE-OMNI should
do the same: never write it, and if it is read, record it as evidence rather
than acting on it.

**What professional tools actually do — and they disagree.** This was verified
from source, and the disagreement is the finding:

**FFmpeg / libswscale: unspecified ⇒ BT.601, unconditionally.** There is *no*
resolution heuristic. `swscale.h` defines `SWS_CS_DEFAULT 5` (= SMPTE 170M), and
`sws_getCoefficients()` maps anything out of range — including 2 "unspecified"
and 0 "no sequence display extension" — onto it. Range is equally blunt:
`src_range` and `dst_range` both default to **0 = limited**, so untagged
full-range YUV comes out crushed. (FFmpeg's *decoders*, by contrast, faithfully
preserve "unspecified" and even sanitise reserved values back to it — the guess
happens in the scaler, not the demuxer.)

**mpv: a two-way heuristic, and it never guesses BT.2020.** From
`video/csputils.c`:

```c
enum pl_color_system mp_csp_guess_colorspace(int width, int height)
{ return width >= 1280 || height > 576 ? PL_COLOR_SYSTEM_BT_709 : PL_COLOR_SYSTEM_BT_601; }
```

and the primaries guess distinguishes 625-line from 525-line SD (576 → BT.601-625;
480/486 → BT.601-525; anything else → BT.709, *"just pick BT.709 to minimize
damage"*). mpv guesses **primaries from the matrix first**, and only falls back
to resolution for the ambiguous BT.601 case. Transfer defaults to
`levels == LIMITED ? BT.1886 : sRGB`; RGB formats are forced to
BT.709/sRGB/full with the honest comment *"The only reasonable thing we can do
is assume it's sRGB and hope for the best."*

**Apple: untagged ⇒ SMPTE-C.** TN2227, verbatim: *"**Important: Media without a
'nclc' tag will be color managed by QuickTime X as if it were created in the
SMPTE-C color space.**"*

**So the same untagged 1920×1080 file is BT.601 in libswscale, BT.709 in mpv,
and SMPTE-C (SD NTSC) in QuickTime X.** For SD the three happen to agree; for HD
they do not. That divergence — not the absence of metadata itself — is the
practical hazard, and it is why the "SD = 601, HD = 709, UHD = 2020" folklore is
only two-thirds true: **no major implementation guesses BT.2020 from
resolution.**

Range is assumed **limited for YUV** and **full for RGB**; libplacebo's rule is
matrix-driven rather than resolution-driven
(`pl_color_system_is_ycbcr_like(sys) ? LIMITED : FULL`), and unlike swscale it
falls back to the *identity* matrix for an unknown system rather than silently
assuming BT.601.

**What CINE-OMNI must do**, and this is a design rule, not a preference:

1. **Store what the file said, separately from what was assumed.** Two fields,
   not one: `signalled` and `effective`. A heuristic that overwrites the
   signalled value destroys the evidence a colourist needs.
2. **Surface the guess.** If the matrix was inferred from frame height, the UI
   says so. Silent heuristics are how "the greens are wrong" becomes a two-day
   investigation.
3. **Make the override a graph attribute**, first-class and per-media-node, so
   that correcting a mis-tagged file is an edit with undo, not a re-import.
4. **Never let a heuristic differ between machines or runs.** The fallback table
   is a committed constant, not a probe.

### A.1.4 What CINE-OMNI has today

`ucg_schema::ColorMeta` is:

```rust
pub struct ColorMeta {
    pub input_colorspace: String,     // e.g. "ARRI LogC4"
    pub working_colorspace: String,   // default "ACEScg"
    pub display: String,              // e.g. "Rec.709"
    pub look_lut: Option<EntityId>,
    pub cdl: Cdl,
}
```

Three strings and a look. There is **no** field for `colour_primaries`,
`transfer_characteristics`, `matrix_coefficients`, `video_full_range_flag`, bit
depth, chroma subsampling, chroma siting, ST 2086 mastering display, or
MaxCLL/MaxFALL. This is entirely reasonable for a system with no decoder — the
names are what an artist chooses — and it is the **first thing that must change**
when one arrives, because every field above is a *file* fact rather than an
artist's choice and belongs beside the media node, not the grade.

## A.2 YUV → RGB, done correctly

### A.2.1 The matrix from Kr/Kb

All the BT.601/709/2020 non-constant-luminance matrices are the same formula
with two constants. Given `Kr`, `Kb` (and `Kg = 1 − Kr − Kb`), the forward
(RGB → Y'CbCr) relations on normalised `E'` signals are:

```
Y'  = Kr·R' + Kg·G' + Kb·B'
Cb  = (B' − Y') / (2·(1 − Kb))
Cr  = (R' − Y') / (2·(1 − Kr))
```

and the de-matrix used by a decoder is:

```
R' = Y'                              + 2·(1 − Kr)·Cr
G' = Y' − (2·Kb·(1 − Kb)/Kg)·Cb − (2·Kr·(1 − Kr)/Kg)·Cr
B' = Y' + 2·(1 − Kb)·Cb
```

with `Cb, Cr ∈ [−0.5, 0.5]` after the range normalisation of A.2.2.

**The constants** (read from FFmpeg `libavutil/csp.c` on `master`, which stores
them as exact rationals):

| System | Matrix code | Kr | Kg | Kb |
|---|---:|---|---|---|
| BT.601 625 (BT.470BG) | 5 | 0.299 | 0.587 | 0.114 |
| BT.601 525 (SMPTE 170M) | 6 | 0.299 | 0.587 | 0.114 |
| **BT.709** | 1 | **0.2126** | **0.7152** | **0.0722** |
| SMPTE 240M | 7 | 0.212 | 0.701 | 0.087 |
| **BT.2020** (NCL and CL) | 9, 10 | **0.2627** | **0.6780** | **0.0593** |
| FCC | 4 | 0.30 | 0.59 | 0.11 |

Using BT.601's coefficients on BT.709 material (or the reverse) produces the
classic error: luminance is roughly right, saturated reds and greens shift, and
it is most visible on skin tones — the exact error users report as "it looks
slightly wrong and I can't say why".

**BT.2020 constant luminance (matrix 10)** is *not* the same operation with
different numbers. Luminance is computed in the **linear** domain, *before* the
transfer function (`EY = KR·ER + KG·EG + KB·EB`, then `E'Y = (EY)'`), and the
colour-difference scaling is **piecewise and asymmetric about zero**:

```
E'PB = (E'B − E'Y) / (2·NB)   for  −NB ≤ E'B − E'Y ≤ 0
E'PB = (E'B − E'Y) / (2·PB)   for    0 < E'B − E'Y ≤ PB
E'PR = (E'R − E'Y) / (2·NR)   for  −NR ≤ E'R − E'Y ≤ 0
E'PR = (E'R − E'Y) / (2·PR)   for    0 < E'R − E'Y ≤ PR

NB = (1 − KB)'  PB = 1 − (KB)'   NR = (1 − KR)'  PR = 1 − (KR)'
```

BT.2020-2's practical constants: **PB = 0.7910, NB = 0.9702, PR = 0.4969,
NR = 0.8591**.

**Almost nothing implements this correctly, and that is worth knowing before
deciding to.** libplacebo and mpv both emit a *permutation* matrix for
`BT_2020_C` and reconstruct green in linear light in the shader
(`color.g = (lin.g − 0.2627·lin.r − 0.0593·lin.b)/0.6780`). **libswscale gives
BT.2020-CL the identical coefficient row as BT.2020-NCL** — it silently decodes
CL content as if it were NCL. And there is **no registered SMPTE UL for BT.2020
constant-luminance coding equations** at all, so MXF cannot even signal it.
CINE-OMNI should detect matrix 10 and either handle it properly or **refuse it
by name** — silently treating CL as NCL is wrong by a visible margin, and is what
the most widely deployed CPU scaler does.

### A.2.2 Limited versus full range

This is the single most common source of "washed out" or "crushed" video, and it
is one bit of metadata.

For bit depth `n` and normalised signal `E ∈ [0,1]`:

| | Limited (`video_full_range_flag = 0`, "MPEG"/"studio"/"narrow") | Full (`= 1`, "JPEG"/"PC") |
|---|---|---|
| Luma / RGB | `(219·E + 16) · 2^(n−8)` | `(2^n − 1)·E` |
| Chroma | `(224·E + 128) · 2^(n−8)` | `(2^n − 1)·E + 2^(n−1)` |

(H.273 equations 30–32 and 36–38, read from the Recommendation itself;
independently confirmed by BT.709-6 item 3.4, BT.2020-2 Table 5, Apple TN2162's
"Scheme B", SMPTE ST 2067-21 Table 13, and the `AVColorRange` documentation in
FFmpeg's `libavutil/pixfmt.h`.)

So at 8-bit limited, luma occupies **16–235** and chroma **16–240**; at 10-bit,
**64–940** and **64–960**; at 12-bit, **256–3760** and **256–3840**. Values
outside those ranges are legal and meaningful — **superblacks and superwhites
must be preserved, not clamped**, because a colourist may want to recover them.
A decoder that clamps to [0,1] at the range-expansion step has thrown away
information the grade needed.

Note the asymmetry in the *full*-range chroma formula: chroma is offset by
`2^(n−1)`, giving 1–255 at 8 bit rather than 0–255. Treating full-range chroma
as a simple `/255` is a small but real error.

### A.2.3 Bit depth

* **8-bit** is the legacy case; expand to float once and never round-trip.
* **10-bit** is the professional norm (ProRes, DNxHR, XAVC, most camera
  originals) and is where banding in an 8-bit pipeline becomes visible.
* **12-bit** and above appear in RAW and high-end intermediates.
* The working format after decode should be **16-bit float** at minimum, and
  32-bit float for the grading path. Anything less re-introduces the quantisation
  the source avoided.
* **Do not carry 10-bit data in 16-bit integer containers without recording the
  original depth** — the difference between "10-bit data left-shifted" and
  "10-bit data in the low bits" is a factor of 64.

### A.2.4 Chroma siting and upsampling

4:2:0 and 4:2:2 store chroma at reduced resolution, and **where** each chroma
sample sits relative to the luma grid is separate metadata again
(`chroma_sample_loc_type`, and Matroska's `ChromaSitingHorz`/`Vert`):

H.273 clause 8.7 defines it numerically, as the position of the centre of the
top-left chroma sample relative to the top-left luma sample, **in units of luma
samples** — which is exactly the phase offset a resampling filter needs:

| `Chroma420SampleLocType` | HorizontalOffsetC | VerticalOffsetC | Common name / used by |
|---:|---:|---:|---|
| **0** | 0 | **0.5** | **Left** — MPEG-2, MPEG-4, **H.264 default for 4:2:0** |
| **1** | **0.5** | **0.5** | **Centre** — MPEG-1, **JPEG**, H.263 |
| **2** | 0 | 0 | **Top-left**, fully co-sited — **BT.2020 / BT.2100** |
| 3 | 0.5 | 0 | Top-centre |
| 4 | 0 | 1 | Bottom-left |
| 5 | 0.5 | 1 | Bottom-centre |

H.273 says the offsets *"would serve as the phase offsets for the horizontal and
vertical filter operations"*, and — this is the single most useful sentence for a
defaulting policy, quoted verbatim:

> "Typical industry practice for standard-definition and high-definition
> television content with standard dynamic range … has been to use **vertically
> interstitial and horizontally collocated** chroma sampling for the 4:2:0 colour
> format, corresponding to **Chroma420SampleLocType equal to 0**. However, the
> 4:2:0 chroma format sampling specified in Rec. ITU-R BT.2020-2 … and in
> Rec. ITU-R BT.2100-2 … correspond to **Chroma420SampleLocType equal to 2**."

**So the correct default is content-dependent: SDR SD/HD 4:2:0 → type 0;
BT.2020/BT.2100 4:2:0 → type 2.** A single hard-coded default is wrong for one of
the two.

**The off-by-one is real and was verified.** FFmpeg's `AVChromaLocation` is
**H.273's value + 1** — `libavcodec/h2645_vui.c` literally does
`vui->chroma_location = vui->chroma_sample_loc_type_top_field + 1;`. Copying a
CICP value straight into an `AVChromaLocation` (or back) produces a systematic
chroma shift.

**Matroska cannot express all six.** Its `ChromaSitingHorz` / `ChromaSitingVert`
are independent 3-valued enums (0 unspecified, 1 collocated, 2 half), so H.273
type 0 becomes `(Horz=1, Vert=2)` and the top/bottom-*field* distinction (types
4/5) is inexpressible. Matroska's `Range` also has a fourth value (3, "defined by
MatrixCoefficients/TransferCharacteristics") with no H.273 counterpart, and its
`Primaries` table mislabels 11 and 12 as "SMPTE RP 432-2" / "EG 432-2" where
H.273 says RP **431**-2 / EG 432-**1**.

**4:2:2 and 4:4:4 have no siting question at all.** H.264 clause 6.2 is explicit:
in 4:2:2 *"the chroma samples are co-sited with the corresponding luma samples"*,
and in 4:4:4 *"all array samples are co-sited for all cases of frames and
fields"*. BT.601 and BT.709 say the same — chroma co-sited with alternate luma
samples, the first chroma sample co-sited with the first luma sample. Only 4:2:0
needs the enum. (Interlaced 4:2:0 does add a per-field quarter-line shift, which
is a case worth refusing rather than getting subtly wrong.)

**The magnitude, confirmed from a reference implementation.** Getting siting
wrong means using the wrong filter phase. libplacebo encodes exactly this as a
signed sampling offset — `pl_chroma_location_offset()` returns **±0.5** for the
left/top/bottom variants and applies it as a per-plane `shift_x`/`shift_y` on
subsampled planes only. So a type-0/type-1 confusion is **half a chroma sample =
one full luma pixel** of colour displacement in 4:2:0. It shows as coloured
fringing biased to one side of every high-contrast vertical edge — not a
symmetric blur — and it survives every subsequent operation.

Note also that libswscale exposes the same control but in awkward units
(`src_h_chr_pos` etc., *"in luma grid/256"*) and **defaults them to −513, i.e.
unspecified** — so a caller that never sets them gets swscale's internal phase
regardless of what the file said.

**Upsampling quality** matters more than it appears:

* **Nearest** (pixel doubling) is what a naive GPU sampler with `filter: nearest`
  gives; it produces blocky chroma and visible 2×2 colour squares on edges.
* **Bilinear** is the minimum acceptable, and is what a hardware sampler gives
  for free — *provided the texture coordinates are offset for the siting*. This
  is the important point: **correct siting under bilinear is a half-texel
  coordinate offset**, which costs nothing.
* **Catmull-Rom / bicubic** and better reconstruct edges more faithfully and are
  what quality-focused players use for chroma. On a GPU this is a few taps.

**4:4:4** has no siting question and no upsampling. **4:2:2** subsamples
horizontally only. **4:2:0** subsamples both, and in interlaced 4:2:0 the
vertical siting differs per field — a case worth refusing rather than getting
subtly wrong.

### A.2.5 Why this belongs on the GPU

1. **It is the same maths for every pixel with no data dependence** — a 3×3
   matrix, a range scale, and a texture fetch. This is precisely what a GPU is.
2. **The sampler does the upsampling for free.** Bind the chroma plane as a
   half-resolution texture, offset the coordinate for siting, and hardware
   bilinear filtering does the work at zero cost. Doing this on the CPU costs a
   full-frame pass.
3. **It avoids a full-frame CPU→GPU copy of expanded data.** Uploading planar
   YUV 4:2:0 is **half the bytes** of the RGB it expands to, before considering
   that the expanded form is usually float.
4. **Hardware decoders already produce GPU surfaces.** VAAPI, NVDEC, DXVA2/D3D11VA
   and VideoToolbox all hand back a GPU-resident frame. Reading it back to expand
   it on the CPU and re-uploading is the single worst thing a media pipeline can
   do, and it is the default if the conversion is not on the GPU.
   `research/02-editorial-media.md` §3.5 already establishes zero-copy as the
   architecture; YUV→RGB on the GPU is what makes zero-copy *useful* rather than
   merely possible. mpv's hwdec list makes the tiering concrete: `vaapi`,
   `nvdec`, `d3d11va` and `videotoolbox` keep the surface on the GPU, while every
   `*-copy` variant *"cop[ies] the decoded video back to system RAM"* and is
   *"often less efficient than the direct modes"*.

5. **And doing it in the decoder's fixed-function block is measurably worse.**
   This is the strongest available evidence, from mpv's own manual section
   "Quality reduction with hardware decoding", verbatim: forced RGB conversion
   *"means certain colorspaces may not display correctly … This will also usually
   **force the use of low quality chroma scalers** instead of the one specified by
   `--cscale`"*; **vdpau** *"always does RGB conversion in hardware, which **does
   not support newer colorspaces like BT.2020 correctly**"*; and **dxva2**
   *"appears to always use BT.601 for forced RGB conversion … Some drivers appear
   to convert to **limited range RGB, which gives a faded appearance**."* A
   fixed-function block hard-wired to BT.601, ignorant of BT.2020, and unreliable
   about range is strictly worse than sampling the raw planes into a shader you
   control.
5. **It fuses with everything downstream.** Range expand → de-matrix →
   linearise → gamut convert → working space is one shader with no intermediate
   buffers. On the CPU each is a pass over the frame.

**A note on fixed-function YCbCr samplers.** Vulkan has a purpose-built feature
for this — `VkSamplerYcbcrConversion` (core since 1.1) takes `ycbcrModel`
(`RGB_IDENTITY`, `YCBCR_IDENTITY`, `YCBCR_709`, `YCBCR_601`, `YCBCR_2020`),
`ycbcrRange` (`ITU_FULL` / `ITU_NARROW`), `xChromaOffset` / `yChromaOffset`
(`COSITED_EVEN` / `MIDPOINT`) and a `chromaFilter`, and does the whole conversion
inside the texture unit. It is siting-aware at the API level, and even has a
validity rule telling you when the format cannot do co-sited chroma. **And
libplacebo and mpv deliberately do not use it** — because it offers only three
colour models, one chroma filter, and no route to arbitrary Kr/Kb, constant
luminance, Dolby Vision, or Lanczos/EWA chroma reconstruction. **CINE-OMNI should
make the same choice: sample the planes raw and do the maths in WGSL**, which is
also the only option `wgpu` offers.

**wgpu specifics, verified.** `wgpu-types` 26.0.0 exposes exactly one
multi-planar format: **`TextureFormat::NV12`** — 8-bit 4:2:0, two planes (plane 0
`R8Unorm` luma, plane 1 `Rg8Unorm` chroma at half width and height), gated behind
`Features::TEXTURE_FORMAT_NV12`, requiring even dimensions. There is **no P010 or
P016**, so **10- and 12-bit media must be bound as two ordinary single-plane
textures** (`R16Unorm` + `Rg16Unorm`, or `R16Uint`/`Rg16Uint`) and combined in
the shader. Since 10-bit is the professional norm, the two-plane manual path is
the primary implementation and `NV12` is the optimisation, not the reverse.
*Whether any wgpu backend exposes a P010-equivalent is* **UNVERIFIED**.

## A.3 HDR: PQ, HLG, HDR10, and the Dolby Vision question

### A.3.1 The two transfer functions — already implemented, and correctly

`cine-color/src/transfer.rs` implements both BT.2100 systems with the published
constants, which were read from the source:

**PQ (SMPTE ST 2084, H.273 transfer code 16)**

```rust
const PQ_M1: f64 = 2610.0 / 16384.0;          // 0.1593017578125
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;   // 78.84375
const PQ_C1: f64 = 3424.0 / 4096.0;           // 0.8359375
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;    // 18.8515625
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;    // 18.6875
```

with `encode(x) = ((c1 + c2·x^m1)/(1 + c3·x^m1))^m2` and the analytic inverse.
These are the BT.2100 values, expressed as the exact fractions the
recommendation gives rather than rounded decimals — which is the right way to
write them. `1.0` corresponds to 10,000 cd/m². The same constants are duplicated
in `crates/cine-gpu/src/shaders/common.wgsl` (`pq_encode`/`pq_decode`); *that
duplication is a defect* — the WGSL emitter of A.5.4 should generate them from
the Rust constants so they cannot diverge.

**HLG (ARIB STD-B67 / BT.2100, H.273 transfer code 18)**

```rust
const HLG_A: f64 = 0.17883277;
const HLG_B: f64 = 1.0 - 4.0 * HLG_A;                 // 0.28466892
const HLG_C: f64 = 0.559_910_729_529_562_4;           // 0.5 - a·ln(4a)
```

`encode(e) = sqrt(3e)` for `e ≤ 1/12`, else `a·ln(12e − b) + c`. Note `HLG_B`
and `HLG_C` are *derived* from `a` in the source rather than transcribed, which
removes a whole class of typo.

Critically, the crate also implements the part most implementations skip: the
**OOTF and its display-dependent system gamma**

```rust
pub fn hlg_system_gamma(peak_nits: f64) -> f64 {
    1.2 + 0.42 * (peak_nits / 1000.0).log10()
}
```

with `hlg_eotf_nits` / `hlg_inverse_eotf_nits` composing OETF⁻¹ with the OOTF.
HLG without the OOTF is not HLG — it is the wrong picture — so this matters.

*The constants above match the published BT.2100 values to the author's reading
of the source, but no independent fetch of BT.2100 was performed for this
document.* **UNVERIFIED** *against the recommendation text; the crate's own tests
assert round-trip to 1e-6, which verifies self-consistency, not conformance.*
**This is a cheap open item to close:** SMPTE ST 2084, ST 2086 and the ST 2094-*
series are **freely downloadable from `pub.smpte.org`** with no login — contrary
to the common belief that they are paywalled — and BT.2100 is free from the ITU.
Note also that **BT.2100 does not cite ST 2084 or ARIB STD-B67 normatively; it
self-specifies both curves**, so it is a sufficient single source for the PQ and
HLG constants.

### A.3.2 HDR10 static metadata — SMPTE ST 2086 and CTA-861.3

"HDR10" is not a licensed technology; it is a **combination**: PQ transfer
(code 16) + BT.2020 primaries (code 9) + 10-bit + ST 2086 mastering-display
metadata + MaxCLL/MaxFALL. Every part is an open standard, and an Apache-2.0
product can implement all of it. *Whether the string "HDR10" carries any
trademark restriction was not verified.* **UNVERIFIED**

**ST 2086 mastering display colour volume — and a correction that matters.**
The famous 0.00002 and 0.0001 scale factors are **not ST 2086's units**. ST 2086
itself expresses chromaticities to four decimal places and maximum luminance as
an integer number of nits. The scaling belongs to the **H.264 / H.265 SEI
carriage** of that data — a discrepancy the H.264/H.265 text itself calls out in
its own NOTEs. Attributing the scaling to ST 2086 is the version of this error
that circulates most widely, and this document made it before correction.

The **SEI bitstream** layout, verified from FFmpeg's HEVC/VVC SEI parser
(`libavcodec/h2645_sei.c`, `master`) and `libavutil/mastering_display_metadata.h`
(which cites *SMPTE 2086:2014* for the semantics):

| Field | Bitstream | Scale | Meaning |
|---|---|---|---|
| `display_primaries[3][2]` | 6 × `u16` BE | **÷ 50000** (units of 0.00002) | CIE 1931 xy of R, G, B |
| `white_point[2]` | 2 × `u16` BE | **÷ 50000** | CIE 1931 xy of white |
| `max_display_mastering_luminance` | `u32` BE | **÷ 10000** | cd/m² |
| `min_display_mastering_luminance` | `u32` BE | **÷ 10000** | cd/m² |

24 bytes total. **Two traps, both verified in the FFmpeg source:**

1. **The primaries are stored in G, B, R order**, not R, G, B. FFmpeg carries
   `static const int mapping[3] = {2, 0, 1};` to reorder them. Reading them as
   RGB gives a plausible-looking but wrong mastering volume.
2. **The values must be sanity-checked.** FFmpeg clears `has_primaries` unless
   each chromaticity is in `[5, 37000]` (x) / `[5, 42000]` (y), and clears
   `has_luminance` unless `max_luminance ∈ [50000, 100000000]` and
   `min_luminance ≤ 50000`. Real files carry zeros and garbage here; a
   tone-mapper that trusts `max_luminance = 0` divides by zero.

**MaxCLL / MaxFALL** come from **CTA-861.3**, carried in the Content Light Level
Info SEI, both in **cd/m²** as plain unsigned integers: MaxCLL is the maximum
light level of any single pixel in the content; MaxFALL is the maximum
frame-average light level. They are *content* measurements, not display
capabilities, and a tone-mapper that has them can do substantially better than
one that only has the mastering display's peak.

**Carriage:** SEI messages in the elementary stream (HEVC/VVC/AV1), the `mdcv`
and `clli` boxes in ISOBMFF, and `MasteringMetadata` / `MaxCLL` / `MaxFALL` in
Matroska's `Colour` element.

### A.3.3 HDR10+ — proprietary in name, but free in practice, and the Tool Vendor tier costs nothing

**This section reverses an assumption.** HDR10+ was initially classed here with
Dolby Vision as "proprietary, excluded". That is wrong, and the difference is
commercially material.

**The normative model is a free standard.** HDR10+'s dynamic metadata **is**
SMPTE **ST 2094-40** — HDR10+ themselves write "SMPTE ST 2094-40 (HDR10+)" — and
ST 2094-40:2020 is **freely downloadable from `pub.smpte.org`, no login**. The
base signal is ordinary PQ (ST 2084) with HDR10 static metadata. Nothing about
implementing it requires access to a gated document. (HDR10+ Technologies' *own*
specification is adopter-gated, and a separate "HDR10+ AV1 Metadata Handling
Specification v1.0.1" is referenced by name in FFmpeg's `libaomenc.c` — its
public availability is **UNVERIFIED** — but neither is needed for ST 2094-40.)

**Governance and founders.** HDR10+ Technologies, LLC is a Delaware company
(Beaverton, Oregon). Its founders, per Exhibit A of the publicly-archived
**HDR10+ Logo License Agreement**, are **Panasonic, Samsung Research America and
Twentieth Century Fox** — *not* Warner Bros., a misattribution that circulates
widely. The 2025 revision of the agreement drops Fox from the definition.

**The licensing tiers, from HDR10+'s own published table** (unchanged from 2018
to 2026):

| Adopter category | Annual admin fee | Grant |
|---|---|---|
| Display manufacturer | $10,000 | specs, mark, **Essential Patent Claims**; ATC certification |
| SoC vendor | $4,000 | specs, patent stand-still |
| Source provider (player, STB, GPU) | $2,500 | specs, mark, **Essential Patent Claims** |
| Content company / OS | $0 | specs, mark, guidelines |
| **Tool vendor** (processing tool, **encoder**, game engine) | **$0** | specs, mark, guidelines, **Essential Patent Claims** |

with an aggregate cap of $10,000/year across all agreements. HDR10+ states the
programme "operates on a **royalty-free basis. There are no per-device or
per-unit royalties**", that content requires "**no per-title licensing
agreements or content approval process**", and that adopters receive access to
"**the HDR10+ patent pool**".

**CINE-OMNI is a Tool Vendor.** The category explicitly names "encoder". The fee
is **$0/year**, and it converts SMPTE's RAND-only patent assurance into an actual
**royalty-free grant of Essential Patent Claims**. That is a materially better
legal position than implementing from the free standard alone, at zero cost.

**Two cautions before signing anything.**

1. **§10 of the Logo License makes the agreement and the logo guidelines
   confidential.** For a project with a public repository and a public blueprint,
   importing confidentiality obligations is a real cost. The *Adopter* agreement's
   confidentiality terms are **UNVERIFIED** — only the *Logo* License is public —
   and should be assumed similar until read. **These are separable: the Adopter
   agreement is what carries the patent grant; the Logo License is what carries
   the mark.** Take the first, and think hard before the second.
2. **§14 reserves the right to regulate use of the expression "HDR10+" beyond the
   logo** — the *word mark* is defended, not just the image. And §9/§19 disclaim
   all defence, indemnity and liability "including without limitation for
   infringement of any trademark, patent".

**What exists in software today:** FFmpeg reads *and writes* ST 2094-40 —
`libavutil/hdr_dynamic_metadata.h` (LGPL-2.1+) defines `AVDynamicHDRPlus` with
both `av_dynamic_hdr_plus_from_t35()` and `av_dynamic_hdr_plus_to_t35()`, and the
libaom wrapper emits it as an ITU-T T.35 OBU with the Samsung provider code. In
Rust, the `hdr10plus` crate **v2.1.5** and `hdr10plus_tool` **v1.7.2** are both
**MIT**, self-described as "read & write HDR10+ metadata. Also known as SMPTE
ST2094-40".

Listed HDR10+ adopters include **Blackmagic Design, Colorfront, Digital Vision,
MTI Film, Deluxe, Grass Valley and MediaArea** — CINE-OMNI's direct peers have
all taken the adopter route.

**Position: implement HDR10+ properly from ST 2094-40, and take the $0 Tool
Vendor Adopter agreement before shipping metadata *generation*.** Never claim
"HDR10+ Certified" and never use the logo without the separate Logo License.

### A.3.4 Dolby Vision — proprietary, and the boundary is precise

**Dolby Vision is a proprietary, licensed system.** It adds *dynamic*, per-shot
or per-frame metadata — the RPU, carried in its own NAL units — on top of a base
HEVC, AVC or AV1 stream, plus an enhancement layer for the dual-layer profiles.

**Correction: the composer maths are *not* purely reverse-engineered.** The
processing that reconstructs an HDR signal from a base layer plus RPU is
normatively and **freely** specified by **ETSI GS CCM 001 V1.1.1 (2017-02),
"Compound Content Management Specification"** — a free, no-login download from
etsi.org in which **the word "Dolby" does not appear**. It defines the exact
identifiers FFmpeg and libplacebo use (`mapping_idc`, `poly_order`, `mmr_order`,
`num_pivots`, `nlq_offset`, `disable_residual_flag`, `ext_block_level`,
`trim_slope`, `trim_offset`, `trim_power`), the polynomial and MMR mapping, the
NLQ dequantisation, and the HDR display-management metadata syntax. Dolby's own
*Profiles and Levels* document maps its profiles onto it (Profile 5 → ETSI
Profile 2; Profiles 4, 7, 8, 9 → ETSI Profile 1).

What remains outside any free specification is the RPU's outer NAL framing and
Dolby's profile signalling. Dolby publishes those too — *Dolby Vision Streams
Within the ISO Base Media File Format v2.0*, *Profiles and Levels v1.3.2*, and
the DASH/HLS/TS carriage documents are publicly downloadable — **but every page
is stamped "Confidential Information", `robots.txt` disallows several of them,
and their copyright notice reads: "Unauthorized use, sale, or duplication is
prohibited… Nothing in this document constitutes a license to practice Dolby
Vision… licenses from Dolby and/or third parties may be required to make or sell
products described in the document."**

**Cite ETSI GS CCM 001 wherever it covers the ground.** It is free, unstamped,
de-branded, and carries a normal ETSI IPR notice. That is a defensible provenance
for an implementation; a "Confidential"-stamped vendor PDF is not.

**Profiles**, from Dolby's own Table 1 — the digit after the dot is the
*cross-compatibility ID*, i.e. what the base layer looks like to a
non-DV decoder:

| Profile | Codec | Layers | CCID meaning |
|---|---|---|---|
| 4 (`dvhe.04`) | 10-bit HEVC | dual | 2 = SDR (BT.1886/709) |
| **5** (`dvhe.05`) | 10-bit HEVC | single | **0 = none** — proprietary IPT-PQ-c2, no fallback |
| **7** (`dvhe.07`) | 10-bit HEVC | **dual** | 6 = UHD Blu-ray HDR |
| **8** (`dvhe.08`) | 10-bit HEVC | single | **1 = HDR10**, 2 = SDR, **4 = HLG** |
| 9 (`dvav.09`) | 8-bit AVC | single | 2 = SDR |
| 10 | AV1 | single | — |

So 8.1 is the HDR10-compatible form and 8.4 the HLG-compatible one. Dolby
describes Profile 5's space as "Dolby Vision proprietary IPT color space… similar
to BT.2100 ICtCp"; FFmpeg comments it `/* HEVC with proprietary IPTPQc2 */`.

**What is verifiably available to an Apache-2.0 product:** the `dolby_vision`
crate (`libdovi`, behind `dovi_tool` v2.3.3), **version 3.4.0, MIT** — verified
from its `Cargo.toml` and the repository `LICENSE`. It extracts RPUs for profiles
4, 5, 7 and 8, and publishes the RPU-level discriminators between them. FFmpeg
also ships an RPU **encoder** (`libavcodec/dovi_rpuenc.c`, LGPL-2.1+), which
refuses dual-layer generation.

**The distinction that matters:**

| Action | Position |
|---|---|
| **Detect** a Dolby Vision stream and say so in the UI | Fine |
| **Pass through** the RPU NAL units on a stream copy, so a DV file survives a trim | Fine, and valuable |
| **Parse** the RPU with the MIT `dolby_vision` crate, e.g. to read L1/L2/L6 trims | Fine as software; see the caveat |
| **Decode the base layer** and display it as HDR10 or HLG, ignoring the RPU | Fine — this is exactly what every open player does |
| **Display-map** using the ETSI-specified process and present the result as **HDR10 with per-scene luminance** | Defensible — this is mpv/libplacebo's model |
| **Generate** RPUs, or write `dvcC`/`dvvC` boxes | **Only behind an explicit opt-in** — see below |
| **Present output as "Dolby Vision"**, or use the marks or logo | **No**, absent a licence from Dolby |
| **Claim Dolby Vision certification or conformance** | **No** |

**The precedent to copy.** FFmpeg's `movenc.c` writes the `dvcC`/`dvvC`
configuration boxes **only when `strict_std_compliance <= FF_COMPLIANCE_UNOFFICIAL`**,
and otherwise logs "Not writing 'dvcC'/'dvvC' box. Requires -strict unofficial."
The reference implementation treats writing Dolby's proprietary signalling as
outside standards compliance and makes the user opt in. **CINE-OMNI should mirror
this exactly: parse and pass through by default; require an explicit flag to
generate an RPU or write a configuration box.**

**And the naming precedent.** mpv does **not** pass the RPU to the display: its
documentation states that dynamic target-colorspace hinting "does not send full
HDR10+ or Dolby Vision metadata, but uses that information to produce **HDR10
with per-scene luminance values**". libplacebo's README is equally careful —
"Profile 5 **conversion to** HDR/PQ or SDR, reading DV side data, and reshaping"
— never "Dolby Vision output". That wording is the model.

**The caveat, stated plainly.** A permissive software licence on an independent
parser says nothing about patents or trademarks, and Dolby says outright that
"licenses from Dolby and/or third parties may be required". Apache-2.0 §3 gives
nothing here, and unlike HDR10+ **there is no royalty-free pool to join**. The
whole open ecosystem — FFmpeg, libplacebo, mpv, dovi_tool — ships anyway, having
signed nothing; absence of enforcement is not a licence.

**A counterintuitive but important recommendation: do not sign a Dolby licence
or NDA "to be safe."** Doing so trades diffuse, unquantified patent risk for
concrete, enforceable and probably confidential contractual obligations that are
hostile to a public repository. Get counsel before shipping RPU *generation* or
display mapping as a headline feature; do not pre-emptively contract.

*Still **UNVERIFIED**: Dolby's per-device royalty rates (unpublished anywhere);
the price of the Dolby Vision Trims License (the widely-cited $1,000 figure could
not be confirmed); Dolby's enforcement history against open-source projects; and
VLC's DV behaviour.* Note that Dolby's "no license required" tier for content
creators applies to **already-licensed partner grading systems** (Resolve,
Baselight and similar) — **it is not an open grant to any tool that wants to
write DV metadata.**

### A.3.5 Tone mapping

Displaying PQ or HLG content on an SDR display, or PQ mastered at 4000 nits on a
600-nit monitor, requires tone mapping, and the choice of operator is a visible
creative decision that must be recorded, not hidden.

* **BT.2390** defines the reference EETF used for PQ display mapping and is the
  citable, open choice for a "correct" default. **BT.2408** gives production
  guidance (nominal HLG/PQ levels, the 75 % HLG reference white). *Neither was
  fetched for this document* — **UNVERIFIED** — *but both are free ITU-R
  recommendations, so this is another cheap open item to close.*
* Reinhard and Hable/Uncharted-2 are the classic real-time operators — cheap,
  well-known, and not colorimetrically defensible for a reference viewer.
* CINE-OMNI's own `OutputTransform` is *already* parametric in peak luminance
  (its doc comment says so explicitly), which means it is structurally the right
  place to put display mapping: one transform, a peak-nits parameter, and the
  same code path for a 100-nit Rec.709 monitor and a 1000-nit HDR reference.
* **The rule: the tone-map operator and its parameters are part of the view
  transform and part of every render-cache key.** Two viewers showing the same
  frame with different tone mapping must not share a cache entry.

## A.4 Alpha and premultiplication

### A.4.1 The two conventions

* **Straight** (also *unassociated*, *unpremultiplied*): colour channels hold the
  object's own colour; alpha describes coverage separately. `RGB` is undefined
  where `A = 0`.
* **Premultiplied** (also *associated*): colour channels already have alpha
  applied — they hold *light contributed*, not *colour*. `A = 0` implies
  `RGB = 0`.

Premultiplied is the correct form for compositing. Porter & Duff's 1984 operator
algebra is defined on premultiplied values; `over` is

```
C_out = C_fg + C_bg · (1 − A_fg)          (premultiplied)
A_out = A_fg + A_bg · (1 − A_fg)
```

— one multiply-add per channel, associative, and correct under filtering.
The straight-alpha form requires a divide and produces wrong results the moment
the image is resampled, blurred or transformed, because interpolating straight
RGB across a coverage edge mixes in the undefined colour of transparent pixels
(the classic black or white fringe).

Two further rules that are not optional:

* **Compositing must happen in linear light.** Blending gamma-encoded values
  produces edges that are too dark, most visibly on a bright-over-dark
  transition. Linearise, composite, then encode.
* **Premultiplied values can legitimately exceed alpha.** An emissive or additive
  element has `RGB > A`. Clamping `RGB ≤ A` to "fix" an image destroys glows and
  light wraps. Do not clamp.

### A.4.2 CINE-OMNI already does this right

`crates/cine-gpu/src/shaders/merge.wgsl` is documented as *"15 Porter-Duff
operations on **premultiplied** RGBA"*, and `keyer.wgsl` as *"Outputs
premultiplied RGBA"*. The compositor's internal convention is therefore already
the correct one. What is missing is not the convention but the **boundary
handling**: nothing currently converts *into* it, because nothing decodes.

### A.4.3 Where alpha comes from, and in which convention

This is the part that must be got right at the decode boundary, because a
convention error is invisible on flat regions and wrong on every edge.

| Source | Alpha | Convention |
|---|---|---|
| **OpenEXR** | Yes | **Associated (premultiplied)** — this is the format's stated convention and the VFX default |
| **Apple ProRes 4444 / 4444 XQ** | Yes, 12-bit | *Convention* **UNVERIFIED** *— must be read from Apple's ProRes white paper before implementation; getting this wrong is a visible edge artefact* |
| **VP9 / AV1 with alpha** | Yes, as a second coded stream | **UNVERIFIED** |
| **HEVC with alpha** (Apple's variant) | Yes, auxiliary picture layer | **UNVERIFIED** |
| **QuickTime Animation / RLE** | Yes | Historically straight |
| **PNG / TIFF** | Yes | PNG is straight by specification |
| **Matroska** | Signals alpha presence via `AlphaMode` on the track | The element says *whether*, not *which convention* |

*The conventions marked UNVERIFIED were not confirmed from primary sources for
this document and must be before a decoder ships.* The consequence of guessing
is not a subtle grade difference — it is dark or light fringing on every matte
edge, which is exactly the defect a VFX pipeline exists to avoid.

### A.4.4 The rule for the pipeline

1. **Convert to premultiplied, in linear light, immediately at decode**, and
   record the source convention on the media node so the decision is auditable.
2. **Stay premultiplied through the entire comp and colour chain.** Every
   Porter-Duff op, every filter, every resample.
3. **Un-premultiply only where a mathematical operation requires straight
   colour** — some grading operations genuinely do — and re-premultiply
   immediately, with the divide guarded at `A = 0`.
4. **Convert to the delivery convention only at encode**, and record which one
   was written.
5. **Never let a display transform run on premultiplied values without
   accounting for it.** A non-linear curve applied to `C·A` is not `A ·
   curve(C)`.

## A.5 OpenColorIO integration and the WGSL problem

### A.5.1 Verified facts about OCIO

| Fact | Value | Source (fetched 2026-09-08) |
|---|---|---|
| Licence | **BSD-3-Clause** | `raw.githubusercontent.com/AcademySoftwareFoundation/OpenColorIO/main/LICENSE` |
| Documentation licence | **CC-BY-4.0** (not BSD) | README SPDX header — relevant if docs text is copied |
| Latest release | **v2.5.2**, 2026-05-13 — a bug-fix/security release addressing **CVE-2026-42450** | GitHub releases API |
| Prior | v2.5.1 (2026-01-13), v2.5.0 (2025-10-01), v2.4.2 (2025-03-19) | same |
| Governance | Academy Software Foundation project | README |
| Build | **C++17–23**, CMake ≥ 3.14 | `opencolorio.readthedocs.io/en/latest/quick_start/installation.html`; `CMakeLists.txt` |
| Required dependencies | Expat ≥ 2.6.0, yaml-cpp ≥ 0.8.0, Imath ≥ 3.1.1, pystring ≥ 1.1.3, minizip-ng ≥ 4.0.0, ZLIB ≥ 1.2.13 | installation docs; `share/cmake/modules` |
| Optional | Little CMS ≥ 2.2 (`ociobakelut` ICC), OpenEXR ≥ 3.2.0, pybind11 + Python ≥ 3.10 | same |

**The licence is not the problem.** BSD-3-Clause is permissive and
Apache-2.0-compatible. (Its no-endorsement clause does mean the OpenColorIO name
cannot be used to promote CINE-OMNI without permission.) The obstacles are a
**six-library C++17 dependency tree** and, decisively, the shader language.
*The licences of OCIO's transitive dependencies were not individually verified.*
**UNVERIFIED**

**Rust bindings.** `ocio-rs` / `ocio-sys` **0.2.1** (BSD-3-Clause, first published
2026-05-13, last push 2026-09-03, **4 GitHub stars**, ~3,000 downloads all within
90 days) is the only live candidate; the 2020 `opencolorio` / `opencolorio-sys`
crates are abandoned placeholders — the latter's description is a copy-paste that
still says "Rust bindings for Alembic". `ocio-rs` **defaults to a stub mode** that
"return[s] safe defaults … but perform[s] no real color management"; real OCIO
requires `--features bundled` or `system`. For colour-critical output that is a
four-month-old, single-maintainer supply-chain risk. Its claimed GPU-path
coverage is the author's own README claim and was **not audited**. **UNVERIFIED**

### A.5.2 OCIO's GPU path emits GLSL, HLSL, MSL and OSL — and not WGSL

The complete `GpuLanguage` enum, read from
`include/OpenColorIO/OpenColorTypes.h` on `main`:

```cpp
enum GpuLanguage
{
    GPU_LANGUAGE_CG = 0,            ///< Nvidia Cg shader
    GPU_LANGUAGE_GLSL_1_2,
    GPU_LANGUAGE_GLSL_1_3,
    GPU_LANGUAGE_GLSL_4_0,
    GPU_LANGUAGE_GLSL_VK_4_6,       ///< OpenGL Shading Language for Vulkan
    GPU_LANGUAGE_HLSL_SM_5_0,
    LANGUAGE_OSL_1,                 ///< Open Shading Language
    GPU_LANGUAGE_GLSL_ES_1_0,
    GPU_LANGUAGE_GLSL_ES_3_0,
    GPU_LANGUAGE_MSL_2_0,           ///< Metal Shading Language
    GPU_LANGUAGE_HLSL_DX11 = GPU_LANGUAGE_HLSL_SM_5_0   // deprecated alias
};
```

**No WGSL member, and nobody has asked for one.** A GitHub issue-search across
the OCIO repository for `wgsl` returns **zero results**; `webgpu` and `wgpu` each
return one unrelated hit. CINE-OMNI would be the first.

**The contract.** OCIO has two modes: the v2 default *generic* mode, which
processes ops as-is and "could contain any number of 1D & 3D luts", and a
*legacy* mode that bakes to at most one 3D LUT and is documented as "less
accurate". `getShaderText()` returns **only OCIO's half** of the fragment
shader — declarations, helpers, and one entry function (`OCIOMain` by default) —
and notably **does not emit a `#version` directive**; the app helper
(`src/libutils/oglapphelpers/glsl.cpp`) prepends it. The application must then
bind what the descriptor enumerates: 1D/2D LUT textures
(`getNumTextures`/`getTexture`/`getTextureValues`), 3D LUT textures
(`getNum3DTextures`/`get3DTexture`/…), and uniforms
(`getNumUniforms`/`getUniform`/`getUniformBufferSize`, with a `m_bufferOffset`
per uniform designed for Vulkan/Metal-style UBOs).

One implementation detail worth stealing regardless of the decision: in
`ops/lut3d/Lut3DOpGPU.cpp`, **OCIO performs tetrahedral interpolation in the
shader and forces the hardware sampler to `INTERP_NEAREST`**. CINE-OMNI's
`lut3d.wgsl` already does tetrahedral interpolation over a storage buffer, which
is the same decision arrived at independently.

### A.5.3 The four options, assessed

**Option 1 — transpile OCIO's GLSL to WGSL. BLOCKED, not merely risky.**

Two independent walls.

*Wall one: dialect.* naga's support matrix (README, `gfx-rs/wgpu/naga`; crate at
**30.0.1**, 2026-08-22, MIT OR Apache-2.0 — note CINE-OMNI pins naga 26.0.0):

| | Front end | Back end |
|---|---|---|
| SPIR-V | ✅ primary | ✅ primary |
| WGSL | ✅ primary, "fully validated" | 🆗 **secondary** |
| GLSL | 🆗 **secondary**, *"GLSL 440+ and Vulkan semantics only"* | 🆗 secondary |
| MSL / HLSL | — | ✅ primary |

naga's GLSL frontend module docs narrow it further: Vulkan **440 (partial), 450,
460**. Of OCIO's six GLSL targets, exactly one — `GPU_LANGUAGE_GLSL_VK_4_6`
(`#version 460 core`) — is even in the right dialect family. And a GLSL→WGSL
route uses naga's **two weakest quadrants at once**. There are **65 open issues**
labelled `lang: GLSL` on the wgpu repository.

*Wall two, and this is the decisive one: **combined image samplers**.* Every OCIO
GLSL variant declares its LUTs as `uniform sampler1D/2D/3D`
(`src/OpenColorIO/GpuShaderUtils.cpp`; under `GLSL_VK_4_6` it emits
`layout(set=N, binding=M) uniform samplerND …`). That is a Vulkan *combined image
sampler*. **WGSL has no such type** — it requires separate `texture_2d<f32>` and
`sampler` bindings. Verified state of play:

* **naga `glsl-in` cannot parse `uniform sampler2D` at all** — it raises
  `NotImplemented("variable qualifier")`. PR `gfx-rs/wgpu#9558`
  ("fix(glsl-in): support combined sampler2D uniforms") was **closed as stale,
  unmerged, on 2026-08-19**. Issue `#4342` requesting the feature has been **open
  since 2021**.
* **The SPIR-V detour is blocked identically.** Issue `#4323` ("Combined image
  samplers") **open since 2021-04-08**. PRs `#4819` and `#4821`, which would have
  desugared combined samplers in `spv-in`, were **closed unmerged in December
  2023** on the principled objection that the resulting IR misrepresents the
  input shader; the contributor was advised to maintain it on a private branch.
  `#4821`'s own description concedes "the resulting IR will not validate until the
  user manually separates the sampler". Reading naga's `front/spv/mod.rs`
  confirms it: `parse_type_sampled_image` retains only the underlying image
  handle and synthesises no sampler global.
* **Tint (Dawn) has no GLSL reader.** Probing `src/tint/lang/*/reader` via the
  GitHub API: `wgsl/reader` 200, `spirv/reader` 200, **`glsl/reader` 404**,
  `hlsl/reader` 404, `msl/reader` 404. Tint ingests WGSL and SPIR-V only — and is
  C++, reintroducing the FFI burden.
* **SPIRV-Cross has no WGSL backend.** Its backends are GLSL, MSL, HLSL, JSON
  reflection, and deprecated C++.

*UNVERIFIED link:* that glslang compiling Vulkan GLSL `uniform sampler2D` emits
`OpTypeSampledImage` rather than separated image+sampler was not confirmed from
glslang's own source. It is standard behaviour; if glslang can be made to emit
separated pairs, the SPIR-V route might reopen, and that is worth a half-hour
experiment before the option is closed permanently.

**Honest verdict: this is not fragile, it is blocked, and it has been
known-blocked for five years by a deliberate architectural refusal upstream.**
Building it anyway means owning a shader-rewriting pass forever — splitting every
`samplerND` into a texture/sampler pair with renamed call sites — against a text
format OCIO gives no stability guarantee for, with a failure mode of **silently
wrong colours rather than a compile error**.

**Option 2 — write a WGSL backend for OCIO and upstream it.** A new arm in
`GpuShaderUtils.cpp` plus separated-sampler plumbing. This is the cleanest
long-term answer to "how does anyone use OCIO with WebGPU", and it would make
CINE-OMNI a contributing ASWF citizen. It is also C++ work on someone else's
release schedule, in a project that has never had a WGSL request, and it does not
remove the six-library C++ dependency from CINE-OMNI's build. **Worth proposing
upstream; not a plan CINE-OMNI's Phase 2 can depend on.**

**Option 3 — bake everything to 3D LUTs.** Legitimate, and ACES itself permits
it (A.6.4). But OCIO's own documentation names the cost. From the allocation-vars
guide: allocation variables exist to "maintain maximum fidelity and minimize
clamping", and **"there's a cost to supporting this larger dynamic range, and
that cost is reduced precision within the 3D luts sample space."** Concretely,
baking loses:

* **Extended range** — a 3D LUT has a bounded domain, so a *shaper* (typically
  log; OCIO's allocation is `uniform` or `lg2`) must normalise into the unit
  cube, and everything the shaper cannot reach is clamped. The documented guidance
  for the range is "the smallest you can get away with, but no smaller."
* **Precision**, as quoted above — the sample budget is fixed and HDR range spends
  most of it where it is not needed.
* **Invertibility** — a tetrahedrally interpolated 3D LUT is not analytically
  invertible; a second inverse LUT must be baked, with round-trip error. ACES 2
  explicitly targets "invertibility up to at least P3 gamut", and its conformance
  procedure tests the round trip (A.6.5).
* **Dynamic properties** — exposure/contrast/gamma sliders exposed through OCIO's
  `DynamicProperty` vanish on bake. A baked LUT is one frozen operating point.

**Correcting a widespread claim about size:** the industry habitually cites 33³
and 65³, but **OCIO's own bakers default to 32³** (Iridas `.cube`, CSP,
Truelight) **and 64³** (Resolve `.cube`, Houdini HDL), all enforcing a 2³
minimum; its *legacy* GPU path uses 32³ (`ociodisplay` passes `32`); and its
*modern* GPU path **does not bake at all** — it uploads whatever grid size the
config's actual `Lut3D` op carries. **That 33³/65³ are "adequate" is
UNVERIFIED and should not be stated with an OCIO citation attached.**

The tooling, correctly named: it is **`ociobakelut`** (not `ocio_bake`), and the
programmatic API is the **`OCIO::Baker`** class (`setConfig`, `setFormat`,
`setInputSpace`, `setShaperSpace`, `setLooks`, `setTargetSpace` **or**
`setDisplayView`, `setShaperSize`, `setCubeSize`, `bake`) — **there is no
`Processor::bake3DLUT`**. `Processor::getOptimizedProcessor` exists but is an
optimisation pass, not a baker.

**Option 4 — emit WGSL from CINE-OMNI's own `Processor` op list. RECOMMENDED.**

The decisive fact is structural, and it was verified by reading the crate:
`cine_color::processor::Op` is a **closed set of ten well-defined variants plus
an escape hatch that is never used**.

```
Matrix { m, offset }          Transfer { func, decode }
Lut1D(Arc<Lut1D>)             Lut3D(Arc<Lut3D>)
Cdl { cdl, direction }        Range { min_in, max_in, min_out, max_out, clamp }
Exponent { value, direction } Log { params, direction }
Saturation(f64)               MonCurve { gamma, offset, direction }
Custom { name, f, inv }       // <- not serialisable
```

`grep -c "Op::Custom"` over `crates/cine-color/src/ocio.rs` and `aces.rs`
returns **0 and 0**. Every processor the OCIO config compiler and the ACES output
transform produce consists solely of the ten closed-form/LUT variants.
`Processor::ops()` is public. So emitting WGSL is a **ten-arm match, not a
compiler** — bounded, testable, and with no third-party translator in the trust
path.

And a third of it is already written. In `crates/cine-gpu/src/shaders/`:

| Op variant | WGSL status |
|---|---|
| `Transfer::SRgb` | **done** — `srgb_encode`/`srgb_decode` in `common.wgsl` |
| `Transfer::Pq` | **done** — `pq_encode`/`pq_decode` in `common.wgsl` |
| `Cdl` | **done** — `cdl.wgsl` |
| `Lut3D` | **done** — `lut3d.wgsl`, tetrahedral, `.cube` layout |
| `Matrix`, `Saturation`, `Range`, `Exponent` | trivial |
| `MonCurve`, `Log`, `Lut1D` | small |
| Remaining 11 `Transfer` variants (HLG, LogC3/4, S-Log3, Log3G10, Canon Log 2/3, V-Log, ACEScc/cct, Rec709, Gamma) | closed-form; a direct port of `transfer.rs` |

The validation story is already built for it: `cine-gpu` validates every WGSL
module with `naga` in CI without a GPU, reflection-tests the binding layout, and
compares GPU output against a CPU mirror to 1e-5 on real hardware. A generated
shader drops into that harness unchanged — **the CPU `Processor` is the reference
the GPU emitter is tested against**, a property no transpiled third-party shader
can offer.

### A.5.4 Recommendation

**Keep the native implementation and add a WGSL emitter over `Processor`.** Do
not link OCIO for the interactive path; do not transpile its GLSL. Then treat
OCIO as an **interoperability target, not a runtime dependency**:

* Keep growing the config-parsing subset, measured against the
  **OpenColorIO-Config-ACES** releases — **v4.0.0** (2024-10-02) is the ACES 2.0
  config and ships with OCIO v2.5.0; v2.2.0 (2023-09-27) is the last ACES 1.3
  one. *That repository's licence was not verified.* **UNVERIFIED**
* Add `ociobakelut`-compatible LUT export so a CINE-OMNI look can be handed to a
  facility pipeline — and note that ACES's implementer notes make providing a
  custom-LUT import path an obligation if the LUT route is used (A.6.4).
* *Optionally, for verification only:* an off-by-default developer tool that
  shells out to a system OCIO and diffs against the native path. Evidence for
  compatibility claims without a C++ dependency in the shipping product.
* *Optionally, as a good-citizen contribution:* propose a WGSL backend upstream
  (Option 2). It benefits everyone and costs CINE-OMNI nothing if it stalls.

De-duplicate the PQ constants while doing this: `common.wgsl` currently
re-declares `m1`/`m2`/`c1`/`c2`/`c3` that already exist in `transfer.rs`. The
emitter should generate them from the Rust constants so they cannot drift.

## A.6 What CINE-OMNI may honestly claim today

### A.6.1 The premise that changed: ACES is now Apache-2.0

**This is the most consequential correction in this document.**
`github.com/ampas/aces-dev` **no longer exists**; it 301-redirects to
**`aces-aswf/aces-core`** ("ACES Core Transforms", 857 stars, last push
2026-09-01). Its `LICENSE` file, fetched and read to the end specifically to
check for appended AMPAS terms, is the **stock Apache License 2.0 with no
additions**. The README carries `SPDX-License-Identifier: Apache-2.0` and states:
*"This repository is part of ACES, a project governed by the Academy Software
Foundation."* It also confirms the rename: *"This repository was named `aces-dev`
in versions of ACES prior to the 2.0 release."*

The whole family, each licence verified individually:

| Repository | Licence | Contents |
|---|---|---|
| `aces-aswf/aces-core` | **Apache-2.0** | Core CTL: tonescale, chroma compress, gamut compress, utilities |
| `aces-aswf/aces-output` | **Apache-2.0** | Output Transform CTL presets **and the reference images** |
| `aces-aswf/aces-input-and-colorspaces` | **Apache-2.0** | Input transforms |
| `aces-aswf/aces-look` | **Apache-2.0** | Look transforms |
| `aces-aswf/aces-amf` | **Apache-2.0** | AMF schema |
| `aces-aswf/aces-docs` | CC-BY-4.0 | Documentation |
| `aces-aswf/CTL` | **Apache-2.0** | The CTL interpreter (`ampas/CTL` also redirects here) |

**ACES's official transforms are now directly compatible with an Apache-2.0
product.** CINE-OMNI may port the ACES 2 algorithms into Rust and WGSL and ship
them, subject to Apache-2.0's notice and patent terms. This removes the reason the
analytic fit existed in the first place.

*Caveat:* only the **current `main`** was verified. The historical `v1.3` /
`v1.3.1` tags may still carry the old AMPAS terms — **UNVERIFIED**. If ACES 1.3
transforms are wanted specifically, check the LICENSE at that tag.

### A.6.2 ACES 2 has no RRT and no ODT

ACES 2 is the current major release. The `aces-core` CHANGELOG dates **Version
2.0 to April 4, 2025**; `docs.acescentral.com` says it "launched September 10,
2025" — most plausibly a spec freeze and a public launch, but **UNVERIFIED**
which is canonical, so cite both. There is no release after 2.0 as of 2026-09-08.

A trap for version-checking: the GitHub **releases API returns an empty array**
and the newest **tag is `v1.3.1`**. ACES 2.0 exists as commits on `main` plus a
CHANGELOG entry. **Anything vendoring ACES CTL must pin a commit SHA.**

What changed, per the docs and CHANGELOG:

* "a completely redesigned suite of rendering transforms"
* a new tonescale "adaptable to produce output for any peak luminance between
  100–10000 nits", with lower default contrast and a gentler highlight rolloff
* "a hue-preserving rendering transform, applying luminance mapping mostly
  independent of color adjustments"
* gamut mapping "to mostly avoid undesirable clipping but still allow for
  reaching the edges of the display gamut volume"
* **"Invertibility up to at least P3 gamut"**
* the monolithic repository split into the family above

**The separate RRT and ODT are gone.** ACES 2's rendering is a parameterised
Output Transform assembled from `aces-core` functions in a JMh-type appearance
space, with `aces-output` supplying per-display parameter presets. **"RRT+ODT" is
ACES 1.x terminology and every use of it — including in CINE-OMNI's own source
comments — must be qualified with the version.**

### A.6.3 What is true, and what must not be said

**On ACES — true:**

* CINE-OMNI implements ACES **colorimetry**: AP0 and AP1 primaries, the
  ACES2065-1 / ACEScg / ACEScc / ACEScct **encodings** and the matrices between
  them, with published reference matrices committed as constants and checked
  against computed ones.
* It implements **input transforms** for the major camera encodings.
* Its output path is documented in the source as exactly what it is:
  `rrt_odt_fit_srgb` is *"Stephen Hill's analytic fit of the ACES 1.x RRT + sRGB
  ODT"*, described in its own doc comment as *"an **approximation** of the
  official CTL reference, accurate to a few percent for SDR sRGB output"*. The
  alternative `OutputTransform` is *"a Daniele-style sigmoid tonescale … **not the
  official CTL either**, but exact, documented, and parametric in peak
  luminance."*

**On ACES — not true, and not to be said:**

* CINE-OMNI is **not ACES-compliant**. It has not been tested against the
  reference images. No conformance figure exists because none has been measured.
* "ACES output transform" is the wrong phrase for the Hill fit. The right phrase
  is **"analytic approximation of the ACES 1.x RRT+ODT (Hill fit)"** — the
  language the crate's own doc comments already use, and the language the fits'
  authors use of their own work. Narkowicz, of the other canonical fit, writes:
  *"we can just sample ODT(RRT(x)) transform and fit a simple curve to this data.
  **We don't even need to run any ACES code at all**"*, reports a "max fit error:
  0.0138", and warns in an update that it "is a **very simple luminance only fit,
  which over saturates brights**". Hill's version is credited in its canonical
  distribution (`TheRealMJP/BakingLab/ACES.hlsl`, MIT) as "coming up with this
  fit" — a fit, in the author's own framing.
* **Both canonical fits approximate ACES *1.x*.** ACES 2's hue-preserving
  JMh-space transform with volumetric gamut mapping is not structurally the same
  kind of object as a five-constant rational curve. A 2026 product shipping a
  Narkowicz curve and calling it "ACES" is describing a ten-year-old
  approximation of a **superseded** transform.
* Nothing may be labelled "ACES 2.0". The parametric `OutputTransform` borrows
  the *shape* of a Daniele-style tonescale; that is a family resemblance, not an
  implementation.
* An unqualified "ACES" menu entry is a false claim. It should read
  **"ACES 1.0 SDR (analytic approximation)"**.

**On OCIO — true:**

* CINE-OMNI **parses a documented subset of OCIO v2 YAML**, compiles the
  transform definitions to its own `Processor`, ships a built-in config
  self-described as *"Built-in ACES 1.3 CG config (CINE-OMNI subset)"*, supports
  24 `BuiltinTransform` styles in both directions, and writes configs back out.
* Unsupported `BuiltinTransform` styles produce a **named error**
  (`Error::Unsupported`), not a silent identity. That is the single most important
  honesty property in a colour parser, and it is already right.

**On OCIO — not true:**

* CINE-OMNI does **not** use OpenColorIO. It is not linked, and it is not a
  reimplementation — it is an independent implementation that reads part of
  OCIO's file format.
* "OCIO support" unqualified implies a config drops in and works. Until the
  subset is measured against real configs the claim is **"reads a documented
  subset of OCIO v2 configs"**, and the UI must show which parts of a loaded
  config it could not honour.

### A.6.4 What conformance would actually require

**Reference data exists.** `aces-aswf/aces-output/tests/images` provides "golden"
images and a four-step procedure:

1. Process `01 ACES/` (`SonyF35.StillLife.exr` and `syntheticChart.01.exr`)
   through your implementation.
2. Compare against `02 Output_on_ACES/` (TIFF, organised `d60/` and `d65/`).
3. Apply the inverse transform to `02` → compare against `03 Inverse_on_Output/`
   (EXR).
4. Re-apply the forward transform → compare against `04 Output_on_Inverse/`
   (round trip).

Coverage is broad: Rec.709 / P3 / Rec.2100 × D60 / D65 × 48–4000 nit ×
BT.1886 / Gamma 2.2 / Gamma 2.6 / sRGB-piecewise / ST 2084 / HLG.

**Two facts that matter more than the procedure:**

* **The images are hosted on Dropbox, not in git.** That is a fragile dependency
  for a CI conformance suite; mirror them.
* **No numerical tolerance is published anywhere.** The tests README, the
  `aces-output` README and the implementer notes all say "compare" and stop.
  There is no ΔE, PSNR, max-abs-error or bit-accuracy threshold, and therefore
  **no published pass/fail criterion**. This is a *verified absence*, and it is
  the single most important fact for anyone planning a conformance claim: the
  reference data defines what correct looks like; the project does not define how
  close is close enough. Any CINE-OMNI conformance statement must therefore
  **publish its own metric and its own threshold**, and say that it is
  self-defined.

**ACES explicitly permits a LUT implementation, with an obligation.** From the
implementer notes: *"It is recommended, if your software supports it, to **port
the core algorithm**"*, and — *"If your implementation can not port the core
algorithms and instead leverages **pre-computed LUTs** for each of the presets,
then you should provide a method and instructions for how a user can add a new
custom LUT generated externally."* The notes also observe that the rendering and
display-encoding operators, though concatenated in one CTL file for convenience,
"may be desirable to maintain [as] two steps" — which maps directly onto a WGSL
design of one shared rendering pass plus a cheap per-display encode.

**The ACES Logo Program is a gate, not self-certification.** Per
`acescentral.com/aces-logo-program/`, a company must apply and be **accepted into
the programme before implementation work**, then submit a Product Qualification
Form with proof of implementation: *"Your company must have already been accepted
into the ACES Logo Program prior to submission."* The criteria documents
(*Program Description and Product Qualification Criteria v1.2*, and the
qualification form) are Dropbox-hosted PDFs that **were not read**. Therefore
**UNVERIFIED**: the per-category feature requirements; whether the criteria
mandate the official CTL or permit equivalent implementations; whether the ACES
logo is a licensed trademark and on what terms; and whether the v1.2 documents
have been updated for ACES 2 and the ASWF transition. Note also that the
programme is administered via acescentral.com (Academy-facing) while the code is
now under ASWF governance — **who owns the "ACES" mark today is UNVERIFIED, and
it matters.** These four PDFs should be read before any certification claim.

### A.6.5 The route that is now open

The Apache-2.0 relicensing changes the recommendation from "approximate honestly"
to something much better:

1. **Ship the OpenColorIO-Config-ACES v4.0.0 ACES 2.0 config** (subject to
   verifying its licence).
2. **Port the `aces-core` Apache-2.0 CTL algorithms** — tonescale, chroma
   compress, gamut compress — to Rust and WGSL by hand. This is legal, it is a
   bounded amount of work, and it produces the real transform rather than a fit.
3. **Validate against the `aces-output` golden images**, publishing CINE-OMNI's
   own metric and threshold since ACES publishes none.
4. **Keep the Hill fit**, clearly labelled, as a cheap real-time preview path.

No C++ FFI, no transpiler, no licence problem, and a genuine — if self-defined —
conformance story. **This should be the headline of Phase 2 colour work.**

---

## A.7 Classification of every element

**Legend** — *Implemented*: exists in the working tree and is tested.
*Planned*: specified here, not built. *External*: would come from a third-party
dependency. *Unverified*: exists but its conformance to the named standard has
not been tested, or the claim in this document could not be checked against a
primary source.

### Colour

| Element | Status | Evidence / note |
|---|---|---|
| 14 gamuts + Bradford adaptation | **Implemented** | `space.rs`; `PrimariesId` has 14 members; matrices computed and cross-checked against published constants |
| sRGB / Rec.709 / gamma transfer functions | **Implemented** | `transfer.rs` |
| **PQ (ST 2084 / BT.2100)** | **Implemented** | exact fractional constants `2610/16384`, `2523/4096·128`, `3424/4096`, `2413/4096·32`, `2392/4096·32` |
| **HLG OETF + inverse** | **Implemented** | `a = 0.17883277`, `b`, `c` derived not transcribed |
| **HLG OOTF + system gamma** | **Implemented** | `1.2 + 0.42·log10(peak/1000)`; most implementations omit this |
| Camera log curves (LogC3/4, S-Log3, Log3G10, Canon Log 2/3, V-Log, ACEScc/cct) | **Implemented** | `transfer.rs`, 15 total |
| ACES AP0/AP1 matrices and encodings | **Implemented** | `aces.rs`, reference matrices committed as constants |
| ACES input transforms | **Implemented** | `aces::idt` |
| **ACES 1.x RRT+ODT** | **Unverified — and explicitly an approximation** | Hill analytic fit; the crate says so; **no conformance test against the reference images exists**. Note ACES 2 has no RRT and no ODT |
| Parametric `OutputTransform` (Daniele-style tonescale) | **Implemented**, not ACES | source says *"not the official CTL either"* |
| OCIO v2 config parse / compile / write | **Implemented (documented subset)** | `ocio.rs`; 24 builtin styles; **unsupported styles raise a named error** |
| Built-in ACES CG config | **Implemented (subset)** | self-described *"Built-in ACES 1.3 CG config (CINE-OMNI subset)"* |
| `.cube` LUTs, ASC CDL XML | **Implemented** | `lut.rs`, `grade.rs` |
| Grading (primaries, curves, HSL qualifiers, masks) | **Implemented** | `grade.rs`, `curve.rs`, `mask.rs` |
| Scopes (histogram, waveform, parade, vectorscope, HDR nits) | **Implemented** | `scopes.rs` + `scopes.wgsl` |
| GPU: sRGB, PQ, CDL, tetrahedral 3D LUT | **Implemented** | `common.wgsl`, `cdl.wgsl`, `lut3d.wgsl`; GPU/CPU agreement to 1e-5 on Intel UHD via Vulkan |
| **WGSL emitter over `Processor`** | **Planned** | A.5.4 — the recommendation of this document |
| Duplicate PQ constants in Rust and WGSL | **Implemented, defective** | must be generated from one source |
| **YUV → RGB (matrix, range, siting)** | **Planned — nothing exists** | `cine_comp::ops::keyer::ycbcr` is an RGB keying helper, not a decode path |
| **H.273 code points in the data model** | **Planned** | `ColorMeta` has no field for any of them |
| **ST 2086 / MaxCLL / MaxFALL in the data model** | **Planned** | ditto |
| **Chroma upsampling and siting** | **Planned** | phase = H.273 Table 8 offsets; default is content-dependent (SDR → type 0, BT.2020/2100 → type 2) |
| BT.2020 **constant luminance** (matrix 10) | **Planned — or refuse by name** | libswscale silently decodes it as NCL; MXF has no UL for it |
| ACES conformance harness | **Planned** | the single measurement that would unblock every ACES claim |
| Tone mapping (BT.2390 EETF) | **Planned** | `OutputTransform` is already peak-parametric, so this is a parameter not a rewrite |
| OpenColorIO the library | **External — recommended *against* for the runtime path** | BSD-3-Clause ✅, v2.5.2 (fixes CVE-2026-42450), but no WGSL, a six-library C++17 tree, and combined-image-sampler shaders |
| `ocio-rs` Rust bindings | **External, immature** | 0.2.1, BSD-3-Clause, 4 stars, first published 2026-05-13, **defaults to a no-op stub mode**; GPU coverage claim **UNVERIFIED** |
| GLSL→WGSL transpilation (naga / Tint / SPIRV-Cross) | **Excluded — blocked** | combined image samplers; A.5.3 |
| **`aces-aswf/aces-core` official CTL** | **External — and now portable** | **Apache-2.0, verified.** May be ported into CINE-OMNI. No git tag for 2.0: pin a commit SHA |
| `aces-output` golden reference images | **External** | Apache-2.0 repo, but **images are Dropbox-hosted** — mirror them. **No tolerance published** |
| OpenColorIO-Config-ACES v4.0.0 (ACES 2.0 config) | **External** | licence **UNVERIFIED** |
| ACES Logo Program certification | **External, gated** | application + review before implementation; criteria PDFs **UNVERIFIED** |
| **Port of ACES 2 Output Transform to Rust/WGSL** | **Planned — newly unblocked** | A.6.5; the headline Phase 2 colour item |
| Video decode (any codec) | **External** | FFmpeg — see `research/02-editorial-media.md` §1 |
| ST 2086 / MaxCLL / MaxFALL parsing | **Planned** | free standards; SEI layout verified. Note the 0.00002/0.0001 units are the **SEI carriage**, not ST 2086 |
| **HDR10+ (SMPTE ST 2094-40)** | **Planned — and legally clear** | normative standard **free** from `pub.smpte.org`; `hdr10plus` crate v2.1.5 **MIT**; **$0/yr Tool Vendor adopter tier grants Essential Patent Claims royalty-free** |
| HDR10+ Logo License | **Excluded — confidentiality** | §10 confidentiality is hostile to a public repo; §14 defends the *word* "HDR10+". Take the Adopter agreement, not the Logo License |
| Dolby Vision RPU parsing / passthrough | **External, available** | `dolby_vision` crate v3.4.0, **MIT**; composer maths freely specified by **ETSI GS CCM 001** (de-branded, no "Dolby" in it) |
| Dolby Vision RPU **generation** / `dvcC`/`dvvC` writing | **Planned, opt-in only** | mirror FFmpeg's `-strict unofficial` gate |
| **Presenting output as "Dolby Vision"** | **Excluded — proprietary** | may detect, pass through, parse, and display-map to **HDR10 with per-scene luminance** (mpv/libplacebo's wording) |
| Premultiplied linear compositing | **Implemented** | `merge.wgsl` (15 Porter-Duff ops), `keyer.wgsl` |
| Alpha convention at the decode boundary | **Planned** | conventions for ProRes 4444 / VP9 / HEVC-alpha marked **UNVERIFIED** above |
| ICC profile handling (`rICC`/`prof`) | **Planned** | not addressed by this document |

### Time and sync

| Element | Status | Evidence / note |
|---|---|---|
| `RationalTime` / `TimeRange` types | **Implemented, defective** | f64 pair; 12.1 % wrong frames measured, B.1.2 |
| **Exact rational time** | **Planned** | B.1.5 |
| Non-drop timecode string | **Implemented, defective** | rounds the rate; B.3.5 |
| **Drop-frame timecode** | **Planned — and `from_timecode` is silently wrong today** | B.3.4, B.3.5 |
| OTIO read/write | **Implemented** | `otio_json.rs`, lossless round trip |
| Editing ops (ripple/roll/slip/slide/…) | **Implemented** | `edit.rs`, 61 tests |
| **FFT waveform sync** | **Implemented** | `cine-timeline/src/sync.rs`, two-stage with confidence metric |
| Sync *method* recorded on the result | **Planned** | `SyncResult` has offset + confidence, no method |
| **Demux / PTS / DTS / index** | **Planned — nothing exists** | B.4 |
| **VFR detection and conform** | **Planned** | B.5 |
| **Audio device I/O** | **Planned — nothing exists** | B.8 |
| **Master clock / transport** | **Planned — nothing exists** | this is what blocks JKL, scrubbing and dynamic trim |
| Drift-correcting streaming resampler | **Planned** | kernel exists (`AudioBuffer::resample`), streaming interface does not |
| BS.1770-4 LUFS, true peak | **Implemented** | `buffer.rs`; ±0.5 LU on a calibrated signal |
| DSP (biquad, comp, gate, limiter with latency, STFT) | **Implemented** | `dsp.rs` |
| VBAP panning, ITU downmix, Atmos 7.1.4 bed | **Implemented** | `pan.rs` |
| Mix / stems / automation | **Implemented** | `mix.rs`, `session.rs` |
| WAV PCM 16/24/f32 | **Implemented** | `wav.rs` |
| **BWF `bext` / iXML parsing** | **Planned** | field layouts **UNVERIFIED**; `TimeReference` is the highest-value field |
| **LTC decode** | **Planned** | `libltc` licence **UNVERIFIED**; reimplementation is reasonable |
| Timecode carriage (`tmcd`, MXF, BWF) | **Planned** | box/UL syntax **UNVERIFIED** |
| Pull-up / pull-down | **Planned** | exact 1000/1001 resample at import |
| Audio backend (CPAL or equivalent) | **External** | licence **UNVERIFIED** — must be checked against Apache-2.0 before selection |
| ITU-R BT.1359 lip-sync figures | **Unverified** | asymmetry direction is certain; the numbers are not quoted here |

---

# PART B — TIME AND A/V SYNCHRONISATION

## B.1 Why floating-point seconds must not be authoritative

### B.1.1 What CINE-OMNI has today

`ucg-core/src/time.rs`:

```rust
pub struct RationalTime {
    pub value: f64,
    pub rate: f64,
}
```

The name says rational; the representation is two IEEE-754 doubles. This is a
faithful copy of OpenTimelineIO's own choice — OTIO's C++ `RationalTime` is
likewise a `double` value and a `double` rate — so the compatibility argument
for it is real. The correctness argument is not.

Three operations carry the damage:

```rust
pub fn rescaled_to(&self, rate: f64) -> Self {
    RationalTime { value: self.to_seconds() * rate, rate }   // (v/r)*r'
}
pub fn to_frames(&self) -> i64 { self.value.floor() as i64 }
```

`rescaled_to` is a division followed by a multiplication in binary floating
point. `to_frames` then **floors** the result. Floor of a value that is one ULP
below an integer is one less than that integer.

### B.1.2 Measured, not asserted

The following was run against the real crate (`cargo test -p ucg-core`, probe
harness reproduced in B.1.6). 200,000 frames at 24000/1001, rescaled to 25 fps
and back — exactly what a 25 fps nested sequence, an EDL conform, or an
OTIO round trip through a PAL-rate composition does:

| Check | Result |
|---|---|
| `a == b` (derived `PartialEq` on `RationalTime`) | **48,408 of 200,000 fail (24.2 %)** |
| `b.to_frames() == f` | **24,201 of 200,000 wrong (12.1 %)** |
| Same test with exact rationals (control) | **0 errors** |

The first failures:

```
frame 27 -> value 26.999999999999996 -> to_frames() = 26
frame 41 -> value 40.99999999999999  -> to_frames() = 40
frame 43 -> value 42.99999999999999  -> to_frames() = 42
frame 45 -> value 44.99999999999999  -> to_frames() = 44
```

This is not a long-duration accumulation problem. **It fails at frame 27.**

### B.1.3 Why this is worse than a frame error

`docs/MODULE-CONVENTIONS.md` rule 4 and `CLAUDE.md` both state the invariant:

> Graph writers must be idempotent: re-running with the same input produces
> zero ops (`group.deltas.is_empty()`). Reactor convergence depends on it.

`RationalTime` derives `PartialEq`. A writer that stores a time computed by one
path and a reactor that recomputes it by another path will compare
`26.999999999999996 != 27.0`, decide the value changed, and emit an op. That op
dirties downstream nodes, which re-run, which may recompute the same value by
yet another path. **Float time in the graph is a latent non-convergence bug in
the reactor cascade**, not merely a display artefact. It has not fired yet only
because nothing in the tree currently rescales across NTSC rates — a grep for
`23.976|29.97|1001` across `crates/` returns three hits, none of them a rate
constant.

### B.1.4 The failures that *are* about duration

Two separate errors are commonly conflated. Both are real; only one is about
floats.

**(a) Storing the rate as a decimal literal.** If a file or an API carries
`rate = 23.976` rather than `24000/1001`:

```
23.976              vs  23.976023976023978   relative error 1.0e-6
```

* 1 hour of programme: 3600.016683 s vs 3600.013083 s → **3.6 ms** (measured
  against the real crate).
* One whole frame of drift after 1,000,000 frames = **11.59 h** at 23.976,
  **9.27 h** at 29.97. Inside a shot: invisible. Across a 24-hour broadcast
  playlist or a timecode day: a frame, then two.
* Audio consequence, which is the one that bites: at exactly 24000/1001, a
  48 kHz stream has **exactly 2002 samples per frame**. At the literal 23.976
  it has 2002.002002 — an error of **172.8 samples per hour = 3.6 ms/hour**,
  i.e. a drift that a mixer will hear as lip-sync creep on a long take.

**(b) Accumulating float seconds.** Honesty requires reporting that at `f64`
this is *not* the dramatic failure it is often claimed to be. Summing
`1001.0/24000.0` for a 2-hour programme (172,627 frames) gives an error of
1.6 × 10⁻⁸ s — four ten-millionths of a frame. And naive
`floor(seconds * rate)` recovers all 200,000 frame indices correctly at both
23.976 and 29.97. **The scare story about f64 seconds accumulating is largely
false.** The real killer is (a) plus the rescale/floor asymmetry in B.1.2 — and
it would be true at `f32`, where the 24-bit mantissa runs out after ~2 hours of
sample counts at 48 kHz.

### B.1.5 The representation CINE-OMNI should use

An integer numerator over an exact rational rate, with no float anywhere in the
authoritative value:

```rust
/// An exact instant: `value / rate` seconds.
pub struct RationalTime {
    value: i64,          // frame or sample index; exact
    rate: Rate,          // exact rational, never a decimal literal
}

/// A frame or sample rate as an exact ratio. Never constructed from f64.
pub struct Rate { num: u32, den: u32 }   // reduced, den ∈ {1, 1001}
```

Arithmetic rules, in order of importance:

1. **Same rate → integer add/sub.** `a.value ± b.value`. Exact, total, cheap.
   This is the overwhelmingly common case and must not touch the rescale path.
2. **Different rates → rescale to the common rate first.** The common rate is
   `lcm` of the two rates expressed as a ratio; because every production rate
   has denominator 1 or 1001 and numerator a multiple of 1000 or a small
   integer, the LCM is always small. Compute in `i128` and reduce.
3. **Rescale is `value * (to.num * from.den) / (from.num * to.den)`**, done in
   `i128`, with the rounding mode passed in explicitly — never defaulted:
   * `Floor` for the frame *containing* an instant (playhead → frame index);
   * `Ceil` for an exclusive end that must not truncate a partial frame;
   * `Nearest` for display and for conforming a source instant to a record
     grid;
   * `Exact` returning `Err` when the conversion is not exact — used by
     validation and by writers that must be idempotent.
   FFmpeg makes the same distinction with `av_rescale_q_rnd` and its
   `AV_ROUND_ZERO/INF/DOWN/UP/NEAR_INF` modes; the lesson is that a rescale
   without a stated rounding mode is a bug waiting for a boundary.
4. **Ranges are half-open `[start, start+duration)`** — which `TimeRange`
   already is — and *durations are never converted by converting both
   endpoints*. Convert the duration itself, or two adjacent clips will overlap
   or gap by a frame after a rate change.
5. **Equality and hashing are on the reduced `(value, num, den)` triple**, so
   `RationalTime` can be `Eq + Hash` — which the current type cannot be,
   because `f64` is not `Eq`. This is what restores reactor idempotence.
6. **Comparison is cross-multiplication in `i128`**, not conversion to seconds.
7. `to_seconds() -> f64` survives, but **only as a lossy accessor for display
   and for handing a number to the audio device**. Nothing may read it back.

The migration is behaviour-preserving for every integer-frame case at a single
rate, which is what the 61 `cine-timeline` tests exercise; it changes results
only where the current code is already wrong.

### B.1.6 The probe

Reproducible; it is how B.1.2 was measured.

```rust
let rate = 24000.0 / 1001.0;
let (mut not_eq, mut bad) = (0usize, 0usize);
for f in 0..200_000i64 {
    let a = RationalTime::from_frames(f, rate);
    let b = a.rescaled_to(25.0).rescaled_to(rate);
    if a != b { not_eq += 1; }
    if b.to_frames() != f { bad += 1; }
}
```

## B.2 Frame rates as exact rationals

| Common name | Exact rate | Denominator | Notes |
|---|---|---|---|
| 23.976 / 23.98 | **24000/1001** | 1001 | "24p NTSC"; never 23.976 |
| 24 | 24/1 | 1 | Film, DCI |
| 25 | 25/1 | 1 | PAL/SECAM territories, EBU |
| 29.97 | **30000/1001** | 1001 | NTSC broadcast; drop-frame territory |
| 30 | 30/1 | 1 | |
| 47.952 | 48000/1001 | 1001 | HFR NTSC |
| 48 | 48/1 | 1 | |
| 50 | 50/1 | 1 | |
| 59.94 | **60000/1001** | 1001 | drop-frame territory |
| 60 | 60/1 | 1 | |
| 119.88 | 120000/1001 | 1001 | |
| 120 | 120/1 | 1 | |

**Where 1000/1001 bites.** Every rate with denominator 1001 is irrational in
decimal and therefore *cannot* be stored as a decimal literal without the
1 × 10⁻⁶ error of B.1.4(a). Consequences:

* **Timecode labels stop matching wall clock.** See B.3.
* **Audio frame boundaries stop being integers.** At 48 kHz:

  | Rate | Samples per frame | Integer? |
  |---|---|---|
  | 24/1 | 2000 | yes |
  | 25/1 | 1920 | yes |
  | 24000/1001 | **2002** | yes |
  | 30000/1001 | **8008/5 = 1601.6** | **no** |
  | 60000/1001 | 4004/5 = 800.8 | no |

  At 29.97 there is no integer sample-per-frame count: the pattern repeats over
  a **5-frame / 8008-sample** cycle. Any code that assumes a constant integer
  samples-per-frame will accumulate 0.6 samples per frame — 3.2 ms per minute.
  Verified by exact rational arithmetic.
* **Pull-up / pull-down.** Transferring between a 24 world and a 23.976 world
  changes audio duration by exactly 0.1 % (1000/1001 or 1001/1000). This is a
  *resample*, not a re-label, and it is the classic dual-system failure (B.7).

## B.3 Timecode

### B.3.1 What timecode is

SMPTE ST 12-1 (the standard formerly numbered 12M) defines the time-and-control
code: an `HH:MM:SS:FF` **label** attached to a frame, plus user bits and flags.
The critical property, and the one CINE-OMNI's current code does not model, is
that **a timecode is a label, not a duration**. `01:00:00:00` means "the frame
whose label is one hour", and at 29.97 that frame does not occur one hour after
`00:00:00:00`.

### B.3.2 Non-drop at 29.97 drifts 3.6 s per hour

If labels are applied at 30 per second to a stream running at 30000/1001:

```
108000 frames ÷ (30000/1001) = 3603.6 s of real time
```

The label says one hour; the clock says one hour and 3.6 seconds.

* **+3.6 s per hour**
* **+86.4 s per 24 h** (≈ 2,589 frames)

Verified by exact rational arithmetic. This is the entire reason drop-frame
exists.

### B.3.3 Drop-frame: exactly which labels are dropped

**Drop-frame drops labels, never frames.** No picture is discarded.

At 29.97 (30000/1001):

> Skip labels `:00` and `:01` at the start of **every minute except minutes
> 00, 10, 20, 30, 40 and 50** (i.e. except minutes divisible by 10).

That is 2 labels × 9 minutes = **18 labels dropped per 10 minutes**.

Arithmetic (exact):

```
frames per DF minute (non-tenth)     = 1800 − 2 = 1798
frames per DF ten-minute block       = 9 × 1798 + 1800 = 17982
frames per DF hour                   = 6 × 17982      = 107892
frames per DF 24 h (wrap point)      = 24 × 107892    = 2589408
```

Compare with nominal 30 fps: 18000 per 10 min, 108000 per hour. Real time for
one drop-frame hour:

```
107892 ÷ (30000/1001) = 3599.9964 s
```

so drop-frame is **not exact either**: it under-counts by **3.6 ms per hour**,
**86.4 ms per 24 h**. It is a very good approximation, not a correction. Any
system that claims drop-frame timecode "matches wall clock" is wrong by 86 ms a
day; a system that needs true wall-clock alignment must carry an absolute
reference, not timecode.

At 59.94 (60000/1001) the same construction doubles: skip labels `:00 :01 :02
:03` at the start of every minute except minutes divisible by 10 — **36 labels
per 10 minutes**, 36000 − 36 = 35964 frames per 10-minute block, matching
`60000/1001 × 600 = 35964.036`. Verified.

**There is no drop-frame at 23.976, 24 or 25.** 24000/1001 would need a
different drop pattern entirely and none is standardised; NLEs present 23.976
timecode as non-drop and accept that the label drifts against wall clock. Any
UI that offers a drop-frame checkbox at 23.976 is offering a bug.

### B.3.4 Conversion, both directions

Let `D = 2` at 29.97 and `D = 4` at 59.94; `F = 30` and `60` respectively (the
*nominal* integer rate, which is `ceil(exact_rate)`).

Constants at 29.97: `FPM = 1798` (frames per dropped minute),
`FP10M = 17982` (frames per ten minutes).

**Frame number → drop-frame timecode**

```
d, m      = divmod(frames, FP10M)               # d = whole 10-min blocks
if m < D:                                        # inside the un-dropped first minute
    f = frames + 18*d                            # 18 = 9*D at 29.97
else:
    f = frames + 18*d + D * ((m - D) / FPM)      # integer division
ff = f % F
s  = (f / F) % 60
mm = (f / (F*60)) % 60
hh = (f / (F*3600)) % 24                         # 24 h wrap
```

The `18*d` term (generally `9*D*d`) restores the labels skipped in complete
ten-minute blocks; the second term restores those skipped in the current block.

**Drop-frame timecode → frame number**

```
total_minutes = 60*hh + mm
frames = ((hh*3600 + mm*60 + s) * F + ff)
       −  D * (total_minutes − total_minutes/10)     # integer division
```

`total_minutes − total_minutes/10` is exactly the count of minutes that had
labels dropped. At `01:00:00;00`: `total_minutes = 60`,
`60 − 6 = 54`, `108000 − 2×54 = 107892`. ✓ matches B.3.3.

**Validation.** The dropped labels are the first `D` frames **of second `00`**
of a non-tenth minute — not the first `D` frames of every second. So a
drop-frame timecode is **illegal** iff

```
minute % 10 != 0   AND   seconds == 00   AND   frames < D
```

(`D = 2` at 29.97, `D = 4` at 59.94). Such a label must be rejected on parse,
not silently accepted. This is the single most useful guard against the
wrong-conversion class of bug, and the "every second" misreading is the most
common way to get it wrong.

**Both directions above were verified exhaustively**, not merely derived. At
29.97 the forward conversion was run over all **2,589,408** frames of a 24-hour
cycle and the inverse over all legal labels: **0 round-trip errors, 0 illegal
labels emitted, 0 label → frame → label mismatches.** At 59.94 the same test over
two hours: 0 errors. Spot values: frame `0 → 00:00:00;00`, frame
`1800 → 00:01:00;02` (the first drop), frame `17982 → 00:10:00;00`, frame
`107892 → 01:00:00;00`, and frame `2589408` wraps to `00:00:00;00`.

**24-hour wrap.** Both directions take the hour modulo 24. Timecode is
cyclic; a timeline that crosses midnight has a label that goes backwards, which
is why editorial software must keep a monotonic frame index internally and use
timecode only as a display and interchange encoding.

### B.3.5 What CINE-OMNI does today — measured

| Call | Returns | Correct answer |
|---|---|---|
| `to_timecode(107892 @ 29.97)` | `00:59:56:12` | `01:00:00;00` |
| `to_timecode(108000 @ 29.97)` | `01:00:00:00` | `01:00:03;18` (DF) — and it is 3603.6 s of real time |
| `from_timecode("01:00:00;00", 29.97)` | `Some(108000)` | `107892` |

The last is the dangerous one. `from_timecode` **splits on both `:` and `;`**,
so it accepts the drop-frame separator and then applies the non-drop
formula — silently returning a value **108 frames (3.6 s) wrong** with no
error. `to_timecode` calls `self.rate.round()`, so 29.97 becomes 30 and 23.976
becomes 24, and there is no drop-frame path at all. A grep for
`drop_frame|DropFrame|dropframe` across `crates/` and `ui/` returns nothing.

**Recommendation:** timecode must become a type, not a string helper.

```rust
pub struct Timecode { h: u8, m: u8, s: u8, f: u8, drop: bool, rate: Rate }
```

with `TryFrom<&str>` that rejects illegal drop-frame labels, and conversions
that take the rate explicitly. `RationalTime::to_timecode()` in its present
form should be removed rather than fixed, because its signature cannot express
drop-frame.

### B.3.6 Carriage

| Container | Mechanism |
|---|---|
| **MP4 / QuickTime** | A dedicated **timecode track** (`tmcd` media handler). Its sample description carries a flags field whose bit 0 is **drop-frame**, plus `timescale`, `frameDuration` and `numberOfFrames` (the nominal integer rate, e.g. 30 for 29.97). The sample data is a single big-endian frame number. The nominal rate and the drop flag together — not the media time base — determine the label. |
| **MXF** | Timecode is carried both as a **Timecode Component** in the material/source package structure (start timecode, rounded rate, drop flag) and, for frame-wrapped essence, per-frame in the **System Item** of the generic container. The two can disagree; professional tools trust the structural metadata for the start and the system item for continuity. |
| **QuickTime (legacy)** | Same `tmcd` mechanism as MP4; QuickTime is the origin of the design. |
| **Broadcast WAV** | No timecode field as such — the start time is the `bext` chunk's `TimeReference`, a **sample count** since midnight (B.7). |

The design consequence: **the nominal integer rate and the drop flag are
independent of the media time base and must be stored separately.** A media
node that stores only `rate: 30000/1001` cannot reconstruct the label.

## B.4 PTS/DTS, time bases, B-frames and a frame-accurate index

### B.4.1 The two timestamps

* **DTS (decode timestamp)** — when a packet must be handed to the decoder.
* **PTS (presentation timestamp)** — when the resulting frame must be shown.

They differ whenever the codec reorders: a B-frame references a picture that is
*later* in presentation order, so that reference must be decoded first. In a
typical `IBBP` pattern the display order `I B B P` has decode order `I P B B`,
and the P frame's PTS is greater than its DTS. Audio streams normally have
PTS == DTS.

Both are integers **in the stream's time base** — a rational (`AVRational` in
FFmpeg; `timescale` in the `mdhd`/`mvhd` boxes in ISOBMFF; the
`TimecodeScale`/track timestamp scale in Matroska). This is the industry's own
answer to B.1: **containers never store float seconds.** MP4 stores integer
units of 1/timescale; a 23.976 track conventionally uses timescale 24000 with a
per-sample duration of 1001. Adopting exact rationals in `ucg-core` puts
CINE-OMNI in the same representation as every container it will read, which
removes an entire class of conversion.

### B.4.2 Building a frame-accurate index

Seeking to a keyframe is easy; landing on **frame N** is the actual
requirement, and it is a container problem, not a codec problem.

The shape that works:

1. **Demux once, index everything.** Walk every packet, recording
   `(pts, dts, byte_offset, is_keyframe, duration)` per stream. For a
   fully-indexed container (MP4's `stss`/`stts`/`ctts`/`stsz`/`stco`) this is a
   header read; for Matroska it is the cue points plus a scan; for an MPEG-TS
   or a raw stream it is a full pass. Persist the index — it is derived data
   with a stable key, i.e. exactly a `media.index` node hanging off
   `media.source` in the scheme `research/02-editorial-media.md` §4.3 already
   proposes.
2. **Sort by PTS to get presentation order**, and assign frame indices
   `0..N` over that order. The mapping `frame_index ↔ pts` is the index.
3. **Seek = find the last keyframe with `pts ≤ target`, then decode forward**
   and discard until `pts == target_pts`. There is no shortcut; a decoder cannot
   start mid-GOP. This is the reference implementation's own design — FFmpeg's
   manual on `-ss`: "in most formats it is not possible to seek exactly, so ffmpeg
   will seek to the closest seek point before position. When transcoding and
   `-accurate_seek` is enabled (**the default**), this extra segment between the
   seek point and position will be **decoded and discarded**."
4. **Handle the traps explicitly**, because every one of them produces an
   off-by-one that users report as "the wrong frame":
   * **Edit lists (`elst`)** — an MP4 may declare that presentation starts at an
     offset, or trims the head. Apple's QTFF defines `media_time = -1` as an
     **empty edit**, and notes that "any difference between the movie's duration
     and the track's duration is expressed as an implicit empty edit"; media rate
     "cannot be 0 or negative". FFmpeg's `mov_fix_index` exists solely to
     re-derive the index under edit lists, and its source comment names the hard
     case: *"we must handle properly the scenario when edit list entry starts
     from none key frame. We find closest previous key frame and preserve it and
     consequent frames in index. All frames which are outside edit list entry
     time boundaries will be dropped after decoding."* That is exactly what
     `AVINDEX_DISCARD_FRAME` is for.
     **Two traps:** the same file yields *different* index timestamps depending
     on the reader's `ignore_editlist` / `advanced_editlist` policy — so **a
     persisted index must record which policy produced it** or it is not
     portable; and FFmpeg's own source notes that *"advanced edit list support
     does not work with fragmented MP4s"*, so fMP4 silently takes a different
     path.
   * **Negative composition offsets (`ctts`)** — legal and common. Apple's QTFF
     is explicit: `CT(n) = DT(n) + CTTS(n)` and "the delta … **can be positive or
     negative**"; the atom is absent entirely when decode and presentation order
     agree. FFmpeg folds all negative offsets into a single global
     `dts_shift = max(dts_shift, -duration)` and **subtracts it before every
     index search** — a reader that assumes unsigned gets a one-frame offset.
   * **`cslg` is the shortcut nobody uses.** Apple: the Composition Shift Least
     Greatest atom "summarizes the calculated minimum and maximum offsets between
     decode and composition time … **This allows a reader to determine the
     minimum required time for decode to obtain proper presentation order without
     needing to scan the sample table for the range of offsets.**" It gives
     reorder depth in **O(1)**. It is optional, so keep the `ctts` scan as a
     fallback — but check for `cslg` first.
   * **Negative DTS smuggled through unsigned `stts`.** FFmpeg carries a
     `max_stts_delta` option whose documentation states that very large `stts`
     deltas are "usually … written in error or **used to store a negative value
     for dts correction when treated as signed 32-bit integers**". Any
     independent `stts` parser needs this guard.
   * **Codec delay / `has_b_frames`** — the decoder emits its first frame several
     packets in; the reorder buffer must be drained at end of stream or the last
     frames are lost. For **audio** there is an explicit rule with no video
     equivalent, from `AVCodecContext::delay`'s own documentation: it is "the
     number of samples the decoder needs to output before the decoder's output is
     valid. **When seeking, you should start decoding this many samples prior to
     your desired seek point.**" Sample-accurate audio seeking is a pre-roll, not
     a seek.
   * **Prefer DTS when not decoding.** `av_read_frame`'s documentation:
     "`pkt->pts` can be `AV_NOPTS_VALUE` if the video format has B-frames, so it
     is better to rely on `pkt->dts` if you do not decompress the payload."
   * **Container vs codec timestamps** — when they disagree, the container wins
     for positioning and the codec wins for reorder depth.
   * **Streams that do not start at zero.** For MPEG-TS, a seek argument is
     offset by the file's `start_time` unless the reader is told otherwise
     (FFmpeg's `-seek_timestamp`: "This matters only for files which do not start
     from timestamp 0, such as transport streams"). An indexer that assumes
     zero-based timestamps is wrong on every TS file by the stream start time.
   * **Timestamp discontinuities** in transport streams and concatenations.
   * **Open GOP.** Frames after a keyframe may reference pictures before it, so
     decoding must start from the *previous* keyframe. *No citable primary source
     for the specifics (RASL/CRA behaviour) was obtained, and the specifics are
     therefore not asserted here.* **UNVERIFIED**

**Matroska's index is explicit rather than derived.** Where MP4 requires
reconstructing an index from `stts`/`ctts`/`stss`/`stsz`/`stco`, Matroska stores
one: `CueTime` (`0xB3`), `CueTrack` (`0xF7`), `CueClusterPosition` (`0xF1`),
`CueRelativePosition` (`0xF0`), `CueDuration` (`0xB2`). Block timestamps are
**signed integers relative to the enclosing `Cluster\Timestamp` (`0xE7`)**, and
the normative conversion is

```
nanoseconds = ( Cluster\Timestamp + block_timestamp * TrackTimestampScale )
              * TimestampScale                      [ - CodecDelay ]
```

with `TimestampScale` (`0x2AD7B1`) defaulting to **1,000,000 ns**. One edge case:
`BlockDuration` (`0x9B`) is optional, and when absent "the duration is assumed to
be the difference between this block's and the next block's timestamp" — so **the
last block of a track has no derivable duration** unless `BlockDuration` or
`DefaultDuration` (`0x23E383`) is present.

**A note on borrowed index structures.** FFmpeg's `AVIndexEntry.flags` is
declared `int flags:2` and both bits are taken (`AVINDEX_KEYFRAME`,
`AVINDEX_DISCARD_FRAME`) — **no third flag can ever be added**, so a richer index
cannot piggyback on it. And `avformat_index_get_entry`'s pointer is "only
guaranteed to be valid until any function that takes the stream or the parent
`AVFormatContext` as input argument is called" — entries must be **copied out
immediately**. Both are arguments for CINE-OMNI owning its own index type rather
than wrapping lavf's.
5. **The index is the authority; the decoder is not.** The playhead is an
   integer frame index in exact time (B.1.5); it is converted to a PTS in the
   stream's time base only at the point of asking the decoder for a frame.

### B.4.3 Consequence for the graph

An index is derived, expensive, and invalidated only by the source file
changing. It belongs in the UCG as a derived entity keyed on the source's
content hash, alongside `media.proxy` — same argument, same machinery, and
cache invalidation is again the dirty set `ucg-core` already computes.

## B.5 Variable frame rate

### B.5.1 How VFR is represented

VFR is not a special mode; it is the normal case of a container that stores a
**duration per sample**.

* **MP4/ISOBMFF** — the `stts` (decoding time to sample) table is a run-length
  list of `(sample_count, sample_delta)` pairs. A CFR file has one entry; a VFR
  file has many. Nothing else changes.
* **Matroska** — each block carries its own timestamp; `DefaultDuration` is a
  hint, not a constraint. VFR is the default expressive capability.
* Sources: screen recorders, iOS/Android capture (which drops frames under
  thermal or exposure pressure), game capture, and anything that timestamps on
  arrival.

### B.5.2 What NLEs do — and the choice they expose

Every mature NLE **conforms VFR to CFR** rather than editing it natively, because
the whole editorial model — a track is a sequence of frame-indexed ranges —
presumes a constant grid. Conforming means choosing a target rate and then, per
output frame, selecting (or duplicating, or dropping) the source frame whose PTS
interval contains it. Doing this at import produces a stable timeline; doing it
lazily at playback produces a timeline whose clip durations change when the cache
is rebuilt.

**Premiere Pro is the clearest documented case**, from an Adobe-staff FAQ (2019):
native VFR support arrived only in **12.0.1**; the symptom of not handling it is
that "playing VFR media in Premiere Pro may result in the **audio being out of
sync** with its corresponding video frame"; detection is **surfaced to the user**
("in the Properties panel, you would see 'Variable Frame Rate Detected'"); and
the conform offers **two modes that are an explicit choice between two
artefacts**:

| Mode | Behaviour | Artefact accepted |
|---|---|---|
| **Preserve Audio Sync** | adds or drops video frames to maintain synchronisation | reduced motion smoothness |
| **Smooth Video Motion** | decodes all available frames without sync adjustment | sync drift |

Adobe's own recommendation is nonetheless to transcode: "If you plan to use
proxy, consolidate or transcode workflows, it is better to **transcode VFR
material to a constant frame rate before editing**."

**This is the design lesson worth carrying:** conforming VFR is a lossy choice
between sync drift and motion judder, and a mature tool **exposes the choice and
records which mode produced a given result** rather than picking silently.

*DaVinci Resolve's position is a verified negative:* a full-text search of the
Resolve 20 reference manual (~191,000 lines) yields **zero** matches for
"variable frame rate" or "VFR". The widely repeated claim about Resolve's VFR
support is snippet-level only. **UNVERIFIED**. Final Cut Pro's behaviour, and the
universally repeated claim that iPhone and OBS recordings are VFR, likewise have
**no primary source** and are not asserted here.

### B.5.3 What CINE-OMNI should do

1. **Detect VFR at import** by inspecting the per-sample durations in the index
   (B.4.2) — it is free once the index exists — and record it on `media.source`
   as an explicit attribute, not an inference.
2. **Conform at import, and record the conform as data.** Store the target rate
   *and* the frame-mapping method. A conform is a lossy decision; it must be
   visible, reversible and part of any cache key.
3. **Keep the VFR index.** The conformed CFR view is a projection; the original
   PTS list stays on the index node so a re-conform to a different rate is
   possible without re-scanning.
4. **Never let a conform silently change clip durations** in an existing
   timeline. If `media.source` is re-imported and its conform changes, that is a
   relink decision for the user, not a background fixup.
5. **Audio is not conformed** — it is a continuous sample stream and keeps its
   own rate. This is precisely the boundary in B.8.

**If conforming is delegated to FFmpeg, pin a version.** The option has moved:

| FFmpeg | `-vsync` | `-fps_mode` | `drop` value |
|---|---|---|---|
| 4.4 | only option | absent | present |
| 5.1, 6.0 | deprecated | present | present |
| 7.0, 7.1 | deprecated | present | **removed** |
| master | **removed entirely** | present | absent |

The 5.1/6.0 manual states it plainly: *"vsync is deprecated and will be removed
in the future."* The internal enum is
`VSYNC_AUTO = -1, VSYNC_PASSTHROUGH = 0, VSYNC_CFR = 1, VSYNC_VFR = 2` — plus an
**undocumented `VSYNC_VSCFR = 3`**, so anything parsing these numerically must
not assume the documented set is complete.

And do not confuse the two filters, which are routinely conflated: **`fps`**
"convert[s] the video to specified constant frame rate by **duplicating or
dropping frames**" (its `round` parameter takes the same `zero|inf|down|up|near`
vocabulary as `av_rescale_q_rnd` — a pleasing confirmation that the rounding
vocabulary of B.1.5 is FFmpeg's throughout), while **`framerate`** "change[s] the
frame rate by **interpolating** new video output frames". `fps` is a filtergraph
operation and `-fps_mode` is applied by the encode sync layer; **use one or the
other, never both**, or the conform is non-deterministic.


## B.6 A/V synchronisation in playback

### B.6.1 The master clock

Three candidates, and the choice is not arbitrary.

| Master | Behaviour | When |
|---|---|---|
| **Audio clock** | Video is scheduled against the number of samples the device has consumed. Video frames are dropped or repeated to follow. | **Default for playback.** |
| **Video clock** | Audio is stretched/squeezed to follow the video. | Only when there is no audio, or for frame-stepping. |
| **External clock** | A monotonic wall clock (or genlock/LTC) drives both. | Live sources, capture, multi-machine sync. |

**Audio must be master by default**, for a perceptual reason and a mechanical
one. Perceptually, the ear detects a discontinuity in a sample stream far more
readily than the eye detects a duplicated or dropped frame; resampling audio to
follow video is audible, dropping a frame usually is not. Mechanically, the
audio device *is* a clock — it consumes samples at a rate set by a crystal that
is not exactly 48 kHz and not exactly the same crystal as the display's — so
following it is following reality, whereas following a software timer means
fighting the hardware.

ffplay, the reference implementation, defaults to
`static int av_sync_type = AV_SYNC_AUDIO_MASTER;` and — importantly — *falls
back*: `get_master_sync_type` returns the audio master if the nominated video
master has no video stream, and `get_clock` returns NaN so a diverged slave is
detected rather than trusted.

### B.6.2 The constants that make it work

Read from `fftools/ffplay.c` on FFmpeg `master` (fetched 2026-09-08). These are
worth quoting because they are the accumulated tuning of two decades and a
reasonable starting point rather than numbers to invent:

```c
#define AV_SYNC_THRESHOLD_MIN        0.04   /* no correction below this */
#define AV_SYNC_THRESHOLD_MAX        0.10   /* correction above this */
#define AV_SYNC_FRAMEDUP_THRESHOLD   0.10   /* long frames are not duplicated */
#define AV_NOSYNC_THRESHOLD         10.0    /* beyond this, give up and resync */
#define SAMPLE_CORRECTION_PERCENT_MAX 10    /* max audio speed change, % */
#define AUDIO_DIFF_AVG_NB            20     /* A-V diffs averaged */
#define EXTERNAL_CLOCK_SPEED_MIN     0.900
#define EXTERNAL_CLOCK_SPEED_MAX     1.010
#define EXTERNAL_CLOCK_SPEED_STEP    0.001
#define REFRESH_RATE                 0.01   /* refresh poll interval */
```

**Video correction** (`compute_target_delay`): the threshold is itself adaptive,
`clamp(frame_delay, 0.04, 0.10)`. If video is behind the master by more than
that threshold, shorten the delay (`delay = max(0, delay + diff)`) — dropping a
frame when the delay reaches zero. If video is ahead, either add the difference
(for long frames) or **double the delay**, repeating the frame. Beyond
`max_frame_duration` no correction is attempted at all; the streams are treated
as unrelated and re-synced.

**Audio correction** (`synchronize_audio`): the A–V difference is averaged with
an exponential coefficient `exp(log(0.01)/20)` over ~20 observations, and the
number of samples requested from the resampler is nudged, clamped to
`±SAMPLE_CORRECTION_PERCENT_MAX` = **±10 %**. Ten percent is a ceiling, not a
target — a steady-state correction should be in the tens of parts per million.

**External clock** (`check_external_clock_speed`): when buffers run low the
clock is slowed by 0.001 per step down to 0.900; when they overfill it is sped
up to 1.010; otherwise it converges back to 1.0. This is the shape any
buffer-driven clock adaptation should take — a slew, never a jump.

### B.6.3 Drift correction by resampling

Two crystals are never identical. A 48 kHz audio device running 20 ppm fast
consumes 48,000.96 samples per nominal second — **3.46 samples per hour of
error per ppm**, so 20 ppm is 69 samples/hour ≈ 1.4 ms/hour. Over a feature it
is seconds.

The correction is a **continuously variable resample ratio**, not a periodic
sample insert/drop:

1. Measure the phase error between the audio clock (samples consumed) and the
   presentation clock over a long window.
2. Convert it to a ratio correction, low-pass filtered hard (the ffplay
   exponential average over ~20 observations is the minimum; a PI controller is
   better).
3. Feed the ratio to a **streaming** sample-rate converter — one that retains
   filter state across blocks. `cine-audio`'s `AudioBuffer::resample` is a
   16-tap Hann-windowed sinc that operates on a whole buffer with no retained
   state; it is the right kernel and the wrong interface for this. A streaming
   variant is a small addition, and it belongs in `cine-audio` (B.8).
4. Never let the ratio step. A discontinuity in resample ratio is a click.

**Pull-up/pull-down (0.1 %) is a different thing** and must not be handled by
the drift corrector. It is a known, exact ratio (1000/1001 or 1001/1000)
applied once, offline, at import — see B.7.

### B.6.4 Buffering, latency, and how much error is acceptable

* **Device buffer** sets the floor on latency and the granularity of the audio
  clock. Small buffers (128–256 frames, 2.7–5.3 ms at 48 kHz) give responsive
  scrubbing and risk underruns; large buffers (1024+, >21 ms) are safe and feel
  sluggish. An NLE wants a user-settable buffer, and must report the resulting
  latency, because it is part of the sync offset.
* **Latency compensation is additive and must be enumerated**: device buffer +
  driver/HAL latency + any plugin latency (`cine-audio`'s `Limiter` already
  reports `latency_frames()`, which is exactly the right pattern) + display
  pipeline latency. A viewer that does not sum these will be consistently out
  by tens of milliseconds regardless of how good its clock is.
* **Lookahead.** The decoder must run far enough ahead that a long GOP or a
  cache miss does not starve the presentation queue, but not so far that a
  seek throws away seconds of work. A few hundred milliseconds of decoded
  frames plus a keyframe-indexed seek (B.4.2) is the usual shape.
* **Presentation-time scheduling, not sleeping.** Each frame carries a target
  presentation time; the render loop wakes on a short poll (ffplay's
  `REFRESH_RATE` of 10 ms), asks whether the next frame is due, and presents it.
  Blocking `sleep(frame_duration)` accumulates the scheduler's error.
* **Vsync interaction.** The display refreshes at its own rate, which is
  usually not a multiple of the media rate. 23.976 content on a 60 Hz display
  must use a 3:2-style cadence, and the *cadence*, not the frame, is what must
  be scheduled — otherwise judder is periodic and highly visible. On a
  variable-refresh display the frame's presentation time can be honoured
  directly, which is strictly better.

### B.6.5 Perceptual tolerance

The relevant recommendation is **ITU-R BT.1359** (relative timing of sound and
vision for broadcasting), which distinguishes a *detectability* threshold from
an *acceptability* threshold, and is asymmetric: sound **lagging** picture is
tolerated considerably better than sound **leading** picture, because that is
the natural direction (sound travels slower than light).

*The exact figures from BT.1359 were not obtained from the recommendation
itself and are marked* **UNVERIFIED** *here rather than quoted from memory.
They must be read from the ITU text before appearing in the product or in
documentation.* What can be said without the document: the tolerance is
asymmetric in the direction described, and it is measured in tens of
milliseconds, which means **a sync error of one frame at 24 fps (41.7 ms) is
within the range where viewers begin to notice** — so "within a frame" is not
a sufficient engineering target for a professional viewer. EBU R37 is the
European counterpart and should be read alongside it.

## B.7 Dual-system sound

### B.7.1 The problem

Picture is recorded on a camera; sound is recorded on a separate device with
its own clock. Three things can be wrong: **offset** (when did each start),
**rate** (are the crystals the same), and **identity** (which sound file goes
with which picture file).

### B.7.2 Jam sync and LTC

**Jam sync**: a master timecode generator is connected to each device once at
the start of the day; each device's internal generator is set from it and then
free-runs. Everything after that depends on the free-run drift of the internal
crystals, quoted in ppm. At 1 ppm a device drifts 3.6 ms per hour; at 10 ppm,
36 ms — approaching a frame. This is why productions re-jam at lunch. *Specific
published drift figures for particular timecode boxes were not verified and are
not quoted here.* **UNVERIFIED**

**LTC (linear timecode)** is SMPTE timecode encoded as an audio signal:
80 bits per frame, biphase-mark encoded so it is self-clocking and
polarity-independent and can be read forwards, backwards and at varying speed.
Recorded onto an audio track, it makes a sound file self-describing: decode the
track and you know the frame each sample belongs to. Decoding LTC is a
**media-layer** job (it recovers timecode from an essence stream), and it is a
small, well-specified DSP problem — zero-crossing detection, bit-period
tracking, sync-word (`0011111111111101`) search, then the BCD field layout with
the drop-frame flag. `libltc` is the reference open implementation; *its licence
was not verified for this document* — **UNVERIFIED** — and must be checked
before any decision to link rather than reimplement (an 80-bit biphase decoder
is a few hundred lines and reimplementation is entirely reasonable for an
Apache-2.0 product).

### B.7.3 Waveform sync — what CINE-OMNI already has

`cine-timeline/src/sync.rs` implements the standard technique and implements it
well:

1. **Coarse**: RMS envelopes at a 512-sample window, normalised to zero mean and
   unit variance, cross-correlated via FFT (`rustfft`). Envelope correlation is
   robust to different microphones, gain and EQ — which is the whole point,
   since a camera scratch mic and a boom sound nothing alike.
2. **Fine**: raw-waveform correlation within ±2 windows of the coarse lag, on a
   bounded segment (≤ 2¹⁸ samples), giving sample accuracy.
3. **Confidence** = peak / mean(|correlation|), documented as ">5 clean, 2–3
   random".

This is the same family of algorithm as the commercial tools. What it lacks is
not algorithm but **context**.

### B.7.4 What metadata makes automatic sync reliable

Waveform correlation is the fallback, not the primary method. The primary
method is metadata, and its reliability is entirely a function of what the
sound recordist wrote:

| Source | Field | Why it matters |
|---|---|---|
| **BWF `bext` chunk** | **`TimeReference`** — a 64-bit **sample count** since midnight | This is the single most valuable field in dual-system. It is a *sample* count, not a timecode string, so it is exact at any rate and immune to the drop-frame problem entirely. Combined with the file's sample rate it gives an unambiguous start instant. |
| **BWF `bext`** | `Description`, `Originator`, `OriginationDate`, `OriginationTime`, `CodingHistory` | Scene/take often lives in `Description` by convention. |
| **iXML chunk** | `SCENE`, `TAKE`, `TAPE`, `PROJECT`, `NOTE`, `CIRCLED`, `FILE_UUID`, and per-track `NAME`/`FUNCTION` | The structured version. `TRACK_LIST` with per-channel names is what turns "channel 3" into "Boom" and feeds the channel-role map of `research/02-editorial-media.md` §5. `FILE_UUID` is a stable identity — exactly what `EntityId::from_key` wants. |
| **Camera** | Start timecode in `tmcd` / MXF, reel/roll name, camera ID | The other half of the timecode match. |

*The precise field layouts of `bext` (EBU Tech 3285) and iXML were not read for
this document and must be verified against the specifications before
implementation.* **UNVERIFIED**

**The strategy that works**, in order, each falling through to the next:

1. **Timecode match**, when both sides have jammed timecode. Exact, instant,
   and correct across a whole day's rushes at once.
2. **Scene/take match** from iXML plus camera metadata, to *restrict the search
   space* — this is what makes the next step both fast and trustworthy.
3. **Waveform correlation** within the restricted space, with the existing
   confidence metric, and a hard floor below which the result is offered as a
   suggestion rather than applied.
4. **Manual**, always available.

**Record the method.** A sync offset that does not say how it was obtained
cannot be re-derived, audited, or selectively re-run when better metadata
arrives. `SyncResult` should carry a `method` alongside `offset_samples` and
`confidence`, and it should be a graph edge (`SyncedTo`, which
`ucg-schema` already defines for `alignment → media`) rather than a buried
attribute.

### B.7.5 Pull-up / pull-down

When picture is shot at 23.976 and sound recorded at a nominal 48 kHz clocked
to 24, or vice versa, the two run at a ratio of exactly **1000/1001**. Over a
10-minute take that is **0.6 seconds** — catastrophic, and invisible at the head
of the take where a waveform sync locks perfectly.

Rules:

* It is an **exact rational resample**, applied once at import, recorded on the
  media node as an explicit attribute. It is never the drift corrector's job.
* Its presence is inferred from metadata (recorder rate vs camera rate), never
  from the waveform — but a waveform sync whose *fine* correlation degrades
  monotonically across a long take is a reliable **detector** that pull-down was
  missed, and the sync engine should say so rather than returning a
  head-aligned result with falling confidence.

## B.8 The boundary: new media layer versus `cine-audio`

### B.8.1 What `cine-audio` already is

Read from the working tree: `AudioBuffer` (interleaved `f32`, sample rate,
channel count, with slicing, mixing, gain, RMS/peak, **BS.1770-4 LUFS**, true
peak, and a 16-tap Hann-windowed sinc `resample`), a DSP toolkit (biquads,
compressor, gate, limiter with latency reporting, delay, envelope, STFT), VBAP
panning and ITU downmix, a `Session`/`Track`/`Clip`/`Bus`/`Stem` model with
automation, a block-based `render`/`render_stems` at `BLOCK_SIZE`, WAV I/O, and
dialogue isolation. **What it does not have is any device I/O and any clock.**

`cine-timeline/src/sync.rs` already implements two-stage FFT cross-correlation
(RMS-envelope coarse pass, raw-waveform fine pass, peak-to-mean confidence).

### B.8.2 The rule

> **`cine-media` owns *arrival*. `cine-audio` owns *sound*. The boundary is an
> `AudioBuffer` with an exact `RationalTime` stamp.**

Everything that exists because a file is encoded, packetised, timestamped or
clocked by hardware belongs to the media layer. Everything that is true of a
signal regardless of where it came from belongs to `cine-audio`. Applying that
rule mechanically:

| Concern | Crate | Why |
|---|---|---|
| Demux, packet queues, PTS/DTS, index | `cine-media` | Container facts |
| Audio **decode** (AAC/PCM/FLAC/ALAC…) | `cine-media` | Codec, not signal |
| Producing `AudioBuffer` from decoded packets | `cine-media` | It is the handoff |
| **Audio device output** (CPAL/CoreAudio/WASAPI/ALSA) | `cine-media` | It is a clock, and the clock is the media layer's job |
| The master clock, drift measurement | `cine-media` | Playback, not mixing |
| **Drift-correcting resample** during playback | `cine-media`, *calling* `cine-audio` | Policy is playback; the SRC kernel is `AudioBuffer::resample` |
| Offline sample-rate conversion | `cine-audio` | Already there |
| Mixing, busses, stems, automation | `cine-audio` | Already there |
| DSP, limiter, LUFS, true peak | `cine-audio` | Already there |
| Panning, downmix, Atmos bed | `cine-audio` | Already there |
| **Waveform sync correlation** | `cine-timeline::sync` (keep) | Already there; it is an editorial operation on two buffers |
| **LTC decode** from a PCM stream | `cine-media` | It is timecode recovery from an essence stream |
| BWF `bext` / iXML metadata parse | `cine-media` | Container metadata |
| Channel-role map on a media reference | `ucg-schema` + `cine-media` | It is source metadata; `cine-audio::Session` consumes it |
| Encode for delivery | `cine-media` | Codec |

### B.8.3 What must *not* happen

* **No second buffer type.** `cine-media` must return `cine_audio::AudioBuffer`.
  A `MediaAudioFrame` that has to be converted is how two DSP stacks get born.
* **No second resampler.** The drift corrector calls the existing sinc
  resampler with a ratio; it does not implement its own. (It may need a
  *streaming* variant with retained state — that is an addition to
  `cine-audio`, not a fork.)
* **No DSP in `cine-media`.** If a function would be equally correct on a
  synthesised sine, it is in the wrong crate.
* **Dependency direction is one way:** `cine-media` → `cine-audio`. Never the
  reverse, or `cine-audio` stops being unit-testable without a decoder.
* **No `Engine` in either domain layer** (MODULE-CONVENTIONS rule 2); the graph
  bindings live in each crate's `graph.rs`.

---

## Recommendations

Ordered by consequence, not by effort.

### R1 — Replace `RationalTime` with an exact rational before anything else is built on it

`value: i64` over `rate: Rate { num: u32, den: u32 }`, arithmetic in `i128`,
explicit rounding mode on every rescale, `Eq + Hash` derived. This is the
prerequisite for a media layer, for timecode, and for A/V sync, and it is a
**correctness fix to shipped code**, not a new feature: 12.1 % of frames come
back wrong from a single cross-rate round trip today (B.1.2).

Do it before the decoder, not after. Every media structure that gets built on
the float type will have to be rewritten.

Migration is behaviour-preserving for the single-rate integer-frame case that
the existing 61 `cine-timeline` tests exercise. Keep OTIO's `double` pair at the
**serialisation boundary only** — read it in, convert to exact, and on write
convert back with the rate rendered as the exact ratio where OTIO permits.

### R2 — Make timecode a type, with drop-frame, and delete the current helpers

`Timecode { h, m, s, f, drop, rate }`, `TryFrom<&str>` that **rejects illegal
drop-frame labels** (frames `00`/`01` in a non-tenth minute at 29.97), and
conversions using the constants of B.3.3 (`FPM = 1798`, `FP10M = 17982`, hour
`107892`, 24 h wrap `2589408`). `RationalTime::from_timecode` and `to_timecode`
should be **removed rather than repaired** — their signatures cannot express
drop-frame, and `from_timecode`'s current acceptance of `;` makes it actively
dangerous.

Offer no drop-frame option at 23.976, 24 or 25. It does not exist.

### R3 — Keep the native colour implementation; add a WGSL emitter over `Processor`

**Do not link OpenColorIO for the runtime path**, and do not transpile its GLSL.
`Op` is a closed set of ten variants, `Op::Custom` is used zero times outside
`processor.rs`, and four variants are already written in WGSL. Emit the rest,
and validate the generated shader in the harness `cine-gpu` already has: naga
validation in CI without a GPU, binding reflection, and pixel comparison against
the CPU `Processor` — which is the reference, and which a third-party shader
could never be compared against so directly.

De-duplicate the PQ constants at the same time: generate `common.wgsl`'s
`pq_encode`/`pq_decode` from `transfer.rs` rather than maintaining two copies.

The reason is not preference. Every OCIO GLSL variant declares LUTs as
`uniform samplerND` — a Vulkan combined image sampler, a type WGSL does not
have — and every automated route around that is closed: naga's GLSL frontend
errors on it (PR closed unmerged August 2026, issue open since 2021), the
SPIR-V detour was rejected on principle in 2023, Tint has no GLSL reader, and
SPIRV-Cross has no WGSL backend. A transpiler would be a shader-rewriting pass
CINE-OMNI owns forever, against an unstable text format, failing as **wrong
colours rather than a build error**.

Keep OCIO as an **interoperability target**: grow the config subset (measured
against OpenColorIO-Config-ACES v4.0.0), add `ociobakelut`-compatible LUT export,
and optionally add an off-by-default developer tool that diffs against a system
OCIO to produce evidence for compatibility claims. Optionally also propose a WGSL
backend upstream to OCIO — nobody has ever asked for one, it would benefit
everyone, and it costs CINE-OMNI nothing if it stalls.

### R4 — Extend `ColorMeta`, or rather split it

`ColorMeta` describes an artist's intent and should keep doing so. What a
decoder learns is a different thing and needs a different component on
`media.source`:

```rust
pub struct VideoSignal {
    pub primaries:  Option<u8>,   // H.273 colour_primaries
    pub transfer:   Option<u8>,   // H.273 transfer_characteristics
    pub matrix:     Option<u8>,   // H.273 matrix_coefficients
    pub full_range: Option<bool>,
    pub bit_depth:  u8,
    pub subsampling: Subsampling,      // 444 / 422 / 420
    pub chroma_loc: Option<ChromaLoc>,
    pub mastering:  Option<MasteringDisplay>,   // ST 2086
    pub light_level: Option<ContentLightLevel>, // MaxCLL / MaxFALL
    pub alpha:      Option<AlphaMode>,          // straight / premultiplied
}
```

with the **signalled** values stored separately from the **effective** ones, so
a heuristic never destroys the evidence and an override is an undoable edit.
Surface in the UI when a value was inferred rather than read.

### R5 — Build `cine-media` with a hard boundary against `cine-audio`

`cine-media` owns arrival: demux, decode, PTS/DTS, the frame index, VFR
detection and conform, device I/O, the master clock, drift measurement, LTC
decode, BWF/iXML parsing. `cine-audio` owns sound and gains only a **streaming**
variant of its existing resampler. The handoff type is
`cine_audio::AudioBuffer` with an exact `RationalTime` stamp; the dependency is
one-way. **No second buffer type, no second resampler, no DSP in `cine-media`.**

Follow `research/02-editorial-media.md` §4.3: the frame index is a derived UCG
entity keyed on the source's content hash, so index invalidation is the dirty
set `ucg-core` already computes.

### R6 — Do YUV→RGB on the GPU, and write the 10-bit path first

Range expand → de-matrix → linearise → gamut convert as **one fused shader**,
with chroma siting handled as a half-texel sampler offset (free) and no
intermediate buffers. `wgpu` 26's only multi-planar format is `NV12` (8-bit
4:2:0, behind `Features::TEXTURE_FORMAT_NV12`); since 10-bit is the professional
norm, **the two-plane manual binding (`R16Unorm` + `Rg16Unorm`) is the primary
implementation and `NV12` is the optimisation.**

Do not clamp on range expansion. Superblacks and superwhites are the
colourist's, not the decoder's, to discard.

### R7 — Port the real ACES 2 transform; the licence no longer forbids it

**This is the recommendation that changed.** ACES's official transforms moved to
the ASWF and are **Apache-2.0** (`aces-aswf/aces-core` and family, verified). The
route is now:

1. Port the `aces-core` CTL algorithms — tonescale, chroma compress, gamut
   compress — into Rust and WGSL by hand. Legal, bounded, and it produces the real
   transform rather than a fit. Pin a **commit SHA**: there is no git tag for
   ACES 2.0.
2. Validate against the `aces-output` golden images using the published
   forward / inverse / round-trip procedure. Mirror the images — they are hosted
   on Dropbox, not in git.
3. **Publish CINE-OMNI's own metric and threshold**, and say that it is
   self-defined, because **ACES publishes no tolerance**. This is not a hedge; it
   is the honest response to a verified absence.
4. Keep the Hill fit, clearly labelled, as a cheap real-time preview.

**Until step 2 passes, fix the language — it costs nothing and is the cheapest
integrity win available.** `aces.rs`'s own doc comments already say the right
thing; the UI must match. Label the current output transform *"ACES 1.0 SDR
(analytic approximation)"*, the config support *"reads a documented subset of
OCIO v2 configs"*, and show which parts of a loaded config could not be honoured.
Qualify every use of "RRT+ODT" as **ACES 1.x** terminology — ACES 2 has neither.

Make no certification claim. The ACES Logo Program requires acceptance into the
programme *before* implementation, and its criteria documents were not read
(`UNVERIFIED`); who owns the "ACES" mark after the ASWF transition is also
`UNVERIFIED`, and it matters.

### R8 — Make sync metadata-first and record the method

Timecode match → scene/take restriction from iXML → waveform correlation within
that restricted space → manual. The existing FFT correlator is the right
algorithm in the wrong position: it should be the fallback, not the first
attempt. Add `method` to `SyncResult`, express the result as a `SyncedTo` edge,
and treat pull-up/pull-down as an exact 1000/1001 resample recorded at import —
never as something the drift corrector discovers.

### R9 — Audio is the master clock; enumerate every latency

Default to the audio clock with an explicit fallback when there is no audio
stream, use ffplay's constants (B.6.2) as the starting tuning, correct drift
with a slewed resample ratio and never a step, and **sum and report** device +
driver + plugin (`Limiter::latency_frames()`) + display latency. A viewer that
does not enumerate these is out by tens of milliseconds no matter how good its
clock is.

### R10 — Take HDR10+; gate Dolby Vision; state both positions in the product

**These two are not the same case, and treating them alike was an error.**

**HDR10+ is free.** Its normative model is SMPTE **ST 2094-40**, downloadable
from `pub.smpte.org` with no login; the `hdr10plus` Rust crate (v2.1.5) is MIT;
there are no per-unit and no per-title royalties. Implement it properly. Then,
**before shipping metadata *generation*, take the $0/year Tool Vendor Adopter
agreement** — the category explicitly names "encoder", it costs nothing, and it
upgrades SMPTE's RAND-only assurance to a royalty-free grant of Essential Patent
Claims. **Do not take the separate Logo License**: its §10 confidentiality is
incompatible with a public repository and a public blueprint, and CINE-OMNI does
not need the mark. Read the Adopter agreement's own confidentiality terms first
(**UNVERIFIED** — only the Logo License is public).

**Dolby Vision is not free, and no pool exists to join.** Detect it, play the
base layer as HDR10 or HLG, preserve the RPU on passthrough, and — if display
mapping is implemented — cite **ETSI GS CCM 001** (free, de-branded, and the
normative source for the composer maths), never Dolby's own
"Confidential"-stamped PDFs. **Gate RPU generation and `dvcC`/`dvvC` writing
behind an explicit opt-in**, mirroring FFmpeg's `-strict unofficial`.

**Do not sign a Dolby licence or NDA "to be safe."** It trades diffuse,
unquantified patent risk for concrete, enforceable, probably confidential
obligations hostile to an open repository. FFmpeg, libplacebo, mpv and dovi_tool
have all signed nothing. Get counsel before shipping RPU generation or display
mapping as a headline feature — but do not pre-emptively contract.

**Naming discipline is the cheapest compliance win in this whole document.**
Ship these strings and no others: *"reads and writes HDR10+ (SMPTE ST 2094-40)
dynamic metadata"*; *"parses Dolby Vision RPU metadata (profiles 5, 7, 8.1,
8.4)"*; *"Dolby Vision streams play as HDR10 with per-scene luminance."* Never
"HDR10+ Certified", never "Dolby Vision output", never either logo.

---

## Sources

Everything below was fetched on **2026-09-08**. Nothing in this document's
verified claims is from memory; everything not fetched is marked `UNVERIFIED`.

**OpenColorIO**
* `LICENSE` (BSD-3-Clause) and README (docs are CC-BY-4.0) — `raw.githubusercontent.com/AcademySoftwareFoundation/OpenColorIO/main/{LICENSE,README.md}`
* Releases — v2.5.2 (2026-05-13, fixes CVE-2026-42450) — GitHub releases API
* `GpuLanguage` enum, `GpuShaderDesc` contract — `include/OpenColorIO/OpenColorTypes.h`, `include/OpenColorIO/OpenColorIO.h`
* GLSL emission and `layout(set=,binding=) uniform samplerND` — `src/OpenColorIO/GpuShaderUtils.cpp`; `#version` prepending — `src/libutils/oglapphelpers/glsl.cpp`
* Shader-side tetrahedral interpolation with `INTERP_NEAREST` sampler — `src/OpenColorIO/ops/lut3d/Lut3DOpGPU.cpp`
* Baker defaults 32³ / 64³ — `src/OpenColorIO/fileformats/FileFormat{IridasCube,CSP,ResolveCube,Truelight,HDL}.cpp`; `ociobakelut` — `src/apps/ociobakelut/main.cpp`
* Allocation vars and the precision trade-off — `opencolorio.readthedocs.io/en/latest/guides/authoring/allocation_vars.html`
* Build requirements — `opencolorio.readthedocs.io/en/latest/quick_start/installation.html`, `CMakeLists.txt`
* No WGSL issue or PR — GitHub issue search for `wgsl` / `webgpu` / `wgpu` in the OCIO repository

**Shader translation**
* naga support matrix and GLSL frontend version list — `github.com/gfx-rs/wgpu/tree/trunk/naga` README, `naga/src/front/glsl/mod.rs`; crate 30.0.1 on crates.io
* Combined image samplers — wgpu issues `#4342`, `#4323`; PRs `#9558` (closed unmerged 2026-08-19), `#4819`, `#4821` (closed unmerged 2023-12-05); `naga/src/front/spv/mod.rs`
* Tint language readers/writers — `api.github.com/repos/google/dawn/contents/src/tint/lang` (glsl/reader → 404)
* SPIRV-Cross backends — `github.com/KhronosGroup/SPIRV-Cross` README
* `wgpu-types` 26.0.0 `TextureFormat` (NV12) — `docs.rs/wgpu-types/26.0.0/wgpu_types/enum.TextureFormat.html`
* Rust OCIO bindings — crates.io API for `opencolorio` / `ocio`; `github.com/shaloong/ocio-rs` README

**ACES**
* `ampas/aces-dev` → `aces-aswf/aces-core` redirect, and the stock Apache-2.0 `LICENSE` — `api.github.com/repos/ampas/aces-dev`, `raw.githubusercontent.com/aces-aswf/aces-core/main/{LICENSE,README.md}`
* Licences of `aces-output`, `aces-input-and-colorspaces`, `aces-look`, `aces-amf`, `aces-docs`, `CTL` — GitHub API per repository
* ACES 2.0 changes and dates — `aces-core/CHANGELOG.md` (April 4, 2025) and `docs.acescentral.com/background/about-aces-2/` (September 10, 2025)
* No releases, newest tag `v1.3.1` — GitHub releases/tags API
* Golden images, four-step procedure, Dropbox hosting, **no published tolerance** — `aces-output/tests/images/README.md`, `aces-output/README.md`
* Implementer notes (port the core algorithm; custom-LUT obligation) — `docs.acescentral.com/system-components/output-transforms/implementer-notes/`
* Logo Program gating — `acescentral.com/aces-logo-program/`
* Analytic fits and their authors' own framing — `knarkowicz.wordpress.com/2016/01/06/aces-filmic-tone-mapping-curve/`; `raw.githubusercontent.com/TheRealMJP/BakingLab/master/BakingLab/ACES.hlsl`
* OpenColorIO-Config-ACES v4.0.0 — GitHub releases

**Colour metadata, HDR, sync**
* **Recommendation ITU-T H.273 (V4) (07/2024)** — code points, the α/β constants, the range equations, `Chroma420SampleLocType` Table 8 and its industry-practice NOTE, NOTE 3 (TC 13 / MatrixCoefficients) and NOTE 4 (sYCC) — free from `itu.int`
* **ITU-T H.264 (V16) (06/2026)** Annex E — VUI inference rules; clause 6.2 — 4:2:2/4:4:4 co-siting
* **ITU-R BT.601-7, BT.709-6, BT.2020-2** — luma coefficients, quantization levels, constant-luminance PB/NB/PR/NR — free from `itu.int`
* `colr` / `mdcv` / `clli` box syntax (ISO/IEC 14496-12 7th ed.) — MPEG's own `MPEGGroup/FileFormatConformance` dataset; the `mdcv`-vs-`SmDm` warning and override rules — `aomediacodec.github.io/av1-isobmff/`
* **RFC 9559** and `matroska.org/technical/elements.html` — `Colour` element IDs, siting enums, `Range` value 3, the RP 431/EG 432 mislabels
* **SMPTE ST 377-1:2019** (MXF) and **ST 2067-21:2023** (IMF App #2E) — Picture Essence Descriptor items, `Color Siting` enum, mandatory-colour constraints and Table 13 levels — free from `pub.smpte.org`; ULs from the SMPTE Metadata Registers (`registry.smpte-ra.org`)
* **Apple TN2162** (QuickTime `colr`/`nclc`, Scheme A vs B, `gama` deprecation), **TN2227** ("media without a 'nclc' tag … as if it were created in the SMPTE-C color space"), QuickTime File Format spec (`gama`)
* `SWS_CS_DEFAULT`, `sws_getCoefficients`, `src_range`/`dst_range` defaults, BT.2020-CL == NCL coefficients — FFmpeg `libswscale/{swscale.h,yuv2rgb.c,options.c}`; VUI parsing and the `+1` chroma-location offset — `libavcodec/h2645_vui.c`; `nclc`/`nclx` split — `libavformat/{mov.c,movenc.c}`; MXF ULs — `libavformat/{mxf.c,mxfenc.c}`
* mpv's resolution heuristic and inference chain — `video/csputils.c`, `video/mp_image.c`; "Quality reduction with hardware decoding" and the hwdec copy/non-copy table — `mpv.io/manual/master/`
* `luma_coeffs()`, `pl_chroma_location_offset()` (±0.5), `pl_color_levels_guess()`, BT.2020-CL shader reconstruction, filter menu — libplacebo `src/colorspace.c`, `src/renderer.c`, `src/shaders/colorspace.c`, `libplacebo.org/options/`
* `VkSamplerYcbcrConversion`, `VkSamplerYcbcrModelConversion`, `VkSamplerYcbcrRange`, `VkChromaLocation` — Khronos Vulkan registry
* ST 2086 layout, 50000/10000 scaling, GBR ordering, sanity ranges; MaxCLL/MaxFALL (CTA-861.3) — FFmpeg `libavcodec/h2645_sei.c`, `libavutil/mastering_display_metadata.h` (`master`)
* A/V sync constants and clock logic — FFmpeg `fftools/ffplay.c` (`master`)

**Containers, indexing and VFR**
* `ctts` (`CT(n)=DT(n)+CTTS(n)`, signed deltas), `cslg`, `elst` (`media_time = -1` empty edit, rate cannot be 0 or negative) — Apple QuickTime File Format documentation, `developer.apple.com/documentation/quicktime-file-format/{composition_offset_atom,composition_shift_least_greatest_atom}` and the QTFF "Edit List Atoms" section
* Box registry (`stts`, `ctts`, `elst`, `stss`, `stsz`, `stco`, `cslg`, `sdtp`) — `mp4ra.org/registered-types/boxes`
* `mov_fix_index`, `dts_shift`, edit-list/keyframe handling, fragmented-MP4 limitation — FFmpeg `libavformat/mov.c` (`master`); `max_stts_delta` — `ffmpeg.org/ffmpeg-formats.html`
* `AVIndexEntry.flags` 2-bit field, `avformat_index_get_entry` pointer lifetime — FFmpeg `libavformat/avformat.h`; `AVCodecContext::delay` audio pre-roll rule — `libavcodec/avcodec.h`; `av_read_frame` DTS guidance — same
* `-ss`, `-accurate_seek`, `-seek_timestamp` — `ffmpeg.org/ffmpeg.html`
* `-vsync` / `-fps_mode` timeline across tags n4.4–master and `VideoSyncMethod` enum — FFmpeg `doc/ffmpeg.texi` at each tag, `fftools/ffmpeg.h`; `fps` and `framerate` filters — `ffmpeg.org/ffmpeg-filters.html`
* Matroska element IDs, signed block timestamps, the timestamp conversion formula, `BlockDuration` fallback — RFC 9559 and `matroska.org/technical/notes.html`
* Premiere Pro VFR detection and the Preserve Audio Sync / Smooth Video Motion modes — Adobe staff FAQ, `community.adobe.com/t5/premiere-pro-discussions/faq-how-to-work-with-variable-frame-rate-vfr-media-in-premiere-pro/m-p/10348229`
* DaVinci Resolve VFR silence — full-text search of the Resolve 20 reference manual (verified negative)
* Drop-frame conversion — verified exhaustively over all 2,589,408 frames of a 24-hour cycle at 29.97 and two hours at 59.94: 0 round-trip errors, 0 illegal labels, 0 label→frame→label mismatches
* `dolby_vision` crate v3.4.0, MIT — `raw.githubusercontent.com/quietvoid/dovi_tool/main/dolby_vision/Cargo.toml` and `.../main/LICENSE`

**HDR10+ and Dolby Vision licensing**
* SMPTE ST 2094-40:2020 (the HDR10+ normative model), ST 2084 and ST 2086 — all free, no login, from `pub.smpte.org`
* HDR10+ licence-programme fee table, royalty-free / no-per-title FAQ statements, adopter list — `hdr10plus.org` (live and Internet Archive captures 2018–2026)
* HDR10+ founders (Panasonic, Samsung Research America, Twentieth Century Fox), §10 confidentiality, §14 word-mark reservation, §9/§19 disclaimers — **HDR10+ Logo License Agreement**, Exhibit A, archived PDF at `web.archive.org/…/HDR10-Logo-License-Agreement.pdf`
* `AVDynamicHDRPlus`, `av_dynamic_hdr_plus_{from,to}_t35` — FFmpeg `libavutil/hdr_dynamic_metadata.h`; T.35/Samsung provider code — `libavcodec/libaomenc.c`
* `hdr10plus` crate v2.1.5 and `hdr10plus_tool` v1.7.2, both MIT — crates.io
* **ETSI GS CCM 001 V1.1.1 (2017-02)**, "Compound Content Management Specification" — free from `etsi.org`; the normative, de-branded source for the composer and display-management metadata
* Dolby Vision profiles, cross-compatibility IDs, ETSI profile mapping, and the "Confidential Information" stamp and copyright notice — *Dolby Vision Profiles and Levels v1.3.2* and *Dolby Vision Streams Within the ISO Base Media File Format v2.0*, `professional.dolby.com`
* RPU encoder and the dual-layer refusal — FFmpeg `libavcodec/dovi_rpuenc.c`; the `-strict unofficial` gate on `dvcC`/`dvvC` — `libavformat/movenc.c`
* mpv's "does not send full HDR10+ or Dolby Vision metadata, but … produce[s] HDR10 with per-scene luminance values" — `mpv-player/mpv/DOCS/man/options.rst`; libplacebo's "Profile 5 conversion to HDR/PQ or SDR" — libplacebo README
* `--dolby-vision-profile` and the 0x7C01 unspecified-NAL note — `videolan/x265/doc/reST/cli.rst`

**CINE-OMNI itself**
* All repository facts — read from the working tree at commit `8f46996`
* The 200,000-frame `RationalTime` measurement — run against `ucg-core` with `cargo test`; harness in B.1.6
* Drop-frame and rate arithmetic — computed exactly with Python `fractions.Fraction`; working shown in B.2 and B.3

**Not obtained, and therefore not quoted:** ISO/IEC 14496-12 normative text
(paywalled — the `ctts`/`elst` *semantics* here are verified from Apple's QTFF and
FFmpeg's implementation, but the clause numbering and exact field widths are
not); open-GOP seeking specifics (RASL/CRA — no citable source found, so nothing
is asserted); Final Cut Pro's VFR behaviour; the universally repeated claim that
iPhone and OBS recordings are VFR; the ISO clause numbering for `colr` (its
*syntax* is verified from MPEG's conformance dataset; ISO's free-standards portal
has closed); ITU-T H.262 first-party text for MPEG-2 chroma siting (corroborated
by three independent implementations and by ST 377-1); the QuickTime `gama` value
1.96 (the atom, its 16.16 encoding, Apple's deprecation and the capture-time
mechanism *are* verified — **the specific number is not, and is not asserted**);
ITU-R BT.2100, BT.2390, BT.2408, BT.1359, EBU R37, EBU Tech 3285 (`bext`), the
iXML specification, SMPTE ST 12-1, Apple's ProRes 4444 white paper (Apple's QTFF
*was* obtained, for `ctts`/`cslg`/`elst`, `colr`/`nclc` and `gama`; the
timecode-carriage claims of B.3.6 remain unverified), the four
ACES Logo Program criteria PDFs, the licence
of the `OpenColorIO-Config-ACES` repository, the licences of OCIO's transitive
dependencies, the licence of `libltc`, the licence of pre-2.0 `aces-dev` tags, and
the licence of whichever audio backend is chosen, the **HDR10+ Adopter
agreement** text and the identity of its patent-pool administrator (only the
*Logo* License is public), the "HDR10+ AV1 Metadata Handling Specification",
Dolby's per-device royalty rates and the price of the Dolby Vision Trims License,
and Dolby's enforcement history against open-source projects. Each is cited by
name where it governs, and every claim that would have depended on one is marked
`UNVERIFIED`.
**These are the document's open items and should be closed before the
corresponding code is written.**
