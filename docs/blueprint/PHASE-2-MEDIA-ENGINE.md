# Phase 2 — The Media Engine

**Status:** first vertical slice implemented and measured. Architecture defensible
for the slice; several named areas remain unresolved and are listed in §13.

**Scope of this document.** What media *is* inside CINE-OMNI, how time is
represented, how decoding is structured so backends can be replaced, what the
build can and cannot do today, and what every claim rests on. Numbers here were
measured on this machine against the real project asset. Where something was not
measured, it says so.

**Companion research:** `research/08-media-codec-stack.md` (codecs, containers,
licensing), `research/09-editorial-media-model.md` (how NLEs model media),
`research/10-color-and-sync.md` (colour metadata and A/V sync),
`research/02-editorial-media.md`.

---

## 0. The reference asset

Every measurement below comes from one real file, and its properties are stated
so the numbers can be read in context rather than generalised.

| Property | Value |
|---|---|
| Container | ISO-BMFF (`.mp4`) |
| Size | 5.52 MiB |
| Video | H.264, `profile_idc` 100 (High), level 3.1, 720×1280, 4:2:0 8-bit |
| Video timing | timescale 12288, 240 samples, 24 fps exactly, B-pyramid |
| GOP | 2 sync samples in 240 frames — a mean GOP of 120 |
| Audio | AAC-LC, 48 kHz, stereo, ~10.03 s |
| Colour metadata | **absent** — no `colr` box, no parsed VUI |

Supporting assets: a real PNG and a real RIFF/WAVE file from the system.

**One asset is not a conformance corpus.** Everything below that says "measured"
means measured *on this file*. That is a far weaker claim than "supported", and
the two are kept separate throughout.

---

## 1. Build constraints that shaped the architecture

Two constraints are load-bearing, and one of them was wrong at the start of
Phase 2 and has been corrected.

1. **No system FFmpeg.** Not installed, and not installable here without
   privileges. Any FFmpeg-based decode path would be unimplementable *and
   unverifiable* on this machine, so it could not honestly be claimed.
2. **~~No C toolchain.~~ Corrected.** Zig 0.16 is installed and `zig cc` /
   `zig c++` compile C and C++ here — the codec research compiled Cisco's
   OpenH264 with it. What is still missing is `pkg-config`, `make` and `nasm`,
   which is why FFmpeg itself still cannot be built. So the accurate statement
   is: **C-source crates are reachable; FFmpeg is not.**

The chosen stack is pure Rust anyway, because that keeps the build reproducible
and the licence position clean. That is a deliberate choice with known costs
(§12), not a limitation being dressed up as a virtue.

---

## 2. Media identity

**Media is identified by its content, never by its path.**

```rust
pub enum ContentHash { Fast { .. }, Full { .. } }   // SHA-256 based
pub fn media_entity_id(hash: &ContentHash) -> EntityId  // "media:file:<hex>"
```

* **Fast** hashes head + tail + length. Measured 20.8 ms on the reference file;
  it is the ingest default.
* **Full** hashes every byte, at 103 MiB/s measured. It is for conform and
  verification, where being certain matters more than being quick.

The two are *different kinds*, not different qualities of the same thing: a
`Fast` hash is never silently compared against a `Full` one.

**Why content, not path.** Path-as-identity has three failure modes that a
production system meets weekly: the same file mounted at two paths becomes two
assets; a file moved on the NAS becomes a lost asset; a file replaced in place
keeps its identity and silently changes meaning. Content addressing fixes all
three, and makes relinking a lookup (`find_by_hash`) rather than a heuristic.

This matches how professional tools actually behave. Final Cut Pro's `uid`
attribute is the closest analogue: *"Final Cut Pro locates the asset using the
`uid` attribute value and relinks the media to the media file at the new
location."* The identifier locates the asset; the *location* is what gets
rebound. See `research/09-editorial-media-model.md` §3.1.

**Offline is a state, not an error.** *"If the file is missing, Final Cut Pro
still imports the media representation and marks it offline."* CINE-OMNI does
the same: `ONLINE_ATTR` on the media node, the project stays valid and editable,
stream structure and identity survive the bytes being absent. This is pinned by
`offline_media_is_a_state_not_a_failure`.

---

## 3. The time model

**Rule: floating-point seconds are never authoritative.** Not in storage, not in
comparison, not in seek. Floats are permitted only at the very edge, for display
to a human.

```rust
pub struct TimeBase  { num: u32, den: u32 }   // e.g. 1/12288
pub struct Timestamp { ticks: i64, base: TimeBase }
```

* Comparison and arithmetic across different bases cross-multiply in `i128`, so
  they are **exact** and cannot overflow for any realistic media duration.
* `FrameRate` carries exact rationals: 23.976 is `24000/1001`, never `23.976`.
* `Timecode` implements SMPTE drop-frame in both directions.
* `rescale` reports whether the conversion was exact, so a lossy conversion can
  never be mistaken for a lossless one.

**Each stream keeps its own base.** Nothing is normalised on ingest. The
reference file's video track stays in 1/12288 and its audio in 1/48000, because
converting them to a common base at ingest is exactly where drift is
manufactured. This is verified by `probes_a_real_mp4_and_reports_streams_truthfully`.

The colour/sync research flags the cost of getting this wrong: a measured
**12.1% wrong-frame rate** in a float-based `RationalTime` implementation, and
24.2% of its equality comparisons failing. That is the failure mode this design
exists to avoid.

---

## 4. Decode architecture

```
MediaFile ─probe─→ StreamInfo{decodable} ─open─→ VideoReader / MediaReader
                                                      │
                            sample table (exact dts/pts/sync)
                                                      │
                                    Packet ─→ dyn VideoDecoder ─→ VideoFrame
                                                      │
                                              FrameCache (byte-bounded LRU)
                                                      │
                                                   consumer
```

**The decode boundary is a trait, not a concrete decoder.**

```rust
pub trait VideoDecoder: Send {
    fn codec(&self) -> &'static str;
    fn decode(&mut self, packet: &Packet) -> Result<Option<VideoFrame>>;
    fn flush(&mut self) -> Result<Vec<VideoFrame>>;
    fn reset(&mut self) -> Result<()>;
}
```

This shape — feed a packet, take frames, reset — is the one every serious decoder
API uses, so an FFmpeg, VideoToolbox or NVDEC backend maps onto it without
contortion. Decoding is the part of a media engine most likely to be replaced,
so callers depend on the trait and swapping backends is a constructor change.

### 4.1 Decode order is not display order

With B-frames these are genuinely different orders. The container records both:
`stts` gives the decode timestamp, `ctts` a composition offset, and their sum is
the presentation time.

Because the whole sample table is read at open, **display order is a computed
permutation of decode order** rather than a guess at how deep a reorder buffer
must be. `has_reordering()` reports whether the two actually differ; on the
reference file it returns true.

### 4.2 Seeking is exact by construction

A compressed stream can only be entered at a keyframe, so a seek is: locate the
target sample, walk back to the nearest sync sample at or before it in decode
order, reset the decoder, decode forward discarding pictures until the target is
produced. There is **no tolerance window** and nothing rounds through seconds.

The test that matters is `video_seek_is_frame_accurate_and_deterministic`: the
frame reached by seeking is compared **pixel-for-pixel** against the frame
reached by playing to the same point. Landing "close enough" would mean a
conform produced a different picture than playback did.

### 4.3 What this build actually decodes

One video backend exists: `H264Decoder`, built on `rusty_h264` (pure Rust,
BSD-2). Independently verified here, not taken on report:

* **240/240 access units** of the reference stream decoded, **zero errors**.
* Plane geometry checked on every frame against the frame dimensions.
* Content verified as photographic: 35 distinct per-frame mean-luma values
  across 240 frames, fading toward black at the end, matching the material.

Two limits stated rather than buried:

1. That is **one asset, one profile**. Not a conformance run.
2. **`rusty_h264` advertises only "the full Constrained Baseline subset."** It
   decoded High profile with B-pyramid here correctly, which is *beyond the
   scope its own authors claim*. The crate is also new — first published
   2026-06-27, ~17.8k downloads. It is treated as a vertical-slice enabler
   behind a trait, **not as a decoder to bet a product on**. This is the single
   largest technical risk in Phase 2 (§13).

The capability gate is the profile, via `h264_profile_supported`. High 4:2:2 and
4:4:4 (`profile_idc` 122/244) fail the profile parse and are **refused**, because
emitting a corrupt picture is worse than reporting no decoder.

---

## 5. Frame representation

```rust
pub struct VideoFrame {
    pub pts: Timestamp, pub frame_index: Option<i64>, pub timecode: Option<Timecode>,
    pub width: u32, pub height: u32,
    pub format: PixelFormat, pub color: ColorMetadata, pub alpha: AlphaMode,
    pub planes: Vec<(Arc<Vec<u8>>, usize)>,   // (data, bytes-per-row)
}
```

**Frames are not converted to RGB on decode.** Real camera media is YUV with
chroma subsampling. Converting on the CPU at decode time costs precision and
time for no gain — the conversion belongs on the GPU, fused with the colour
transform. So a frame carries its planes plus enough metadata to be interpreted
correctly later. Plane data is `Arc`-shared, so a frame can sit in the cache and
be handed to several consumers without copying.

**Colour metadata is honestly absent.** `ColorMetadata` holds ITU-T H.273 code
points as `Option`, and `is_complete()` reports whether enough is known to
interpret the samples without guessing. The reference file carries none, so the
decoded frames carry none.

This matters more than it looks. From `research/10-color-and-sync.md`, verified
from source: libswscale defaults unspecified colour to BT.601 unconditionally
with **no** resolution heuristic; mpv's heuristic is two-way and never guesses
BT.2020; Apple's TN2227 treats untagged media as SMPTE-C. **The same untagged
1080p file therefore decodes three different ways in three major tools.** That
divergence — not the missing tag — is the hazard, and silently defaulting to
Rec.709 would be a guess that survives all the way into the grade. IMF
(ST 2067-21) supplies the model to copy: colour metadata mandatory and complete,
or explicitly absent, never partially inferred.

---

## 6. Audio representation

```rust
pub struct AudioFrame { pts: Timestamp, sample_rate: u32, channels: Vec<Arc<Vec<f32>>> }
```

Planar `f32`, one buffer per channel, `Arc`-shared. Planar because every
processing stage wants channels separately; `f32` because it is the working
format for mixing and the only one that survives repeated gain changes without
accumulating quantisation error. `to_interleaved()` exists for device output,
which is the one place interleaving is required.

Audio seeks the same way video does — seek then decode forward — so audio and
video frame-accuracy have the same meaning. `cine-media` owns arrival of sound;
`cine-audio` owns everything after it.

---

## 7. The media cache

Byte-bounded LRU (`IndexMap`), keyed on `(media content hash, stream, ticks)`.

**Bounded by bytes, not entries.** A 720×1280 4:2:0 frame is 1.32 MiB — roughly
a thousand times an audio packet. Counting entries would let a handful of frames
exhaust memory. A frame larger than the entire budget is not cached at all,
rather than evicting everything to make room for something that cannot help.

**The key includes the content hash**, so a replaced file can never be served
stale frames from its predecessor: different bytes are a different key.
`invalidate_media(hash)` drops one file's frames and leaves the rest untouched.

Measured: 131× cheaper than re-decoding the same span of audio.

**A second cache exists at the reader.** The benchmark caught a real defect: the
reader discarded the frame it had just returned, so re-requesting the frame at
the play head re-decoded the whole GOP — **2.1 s** for a paused viewer. The
reader now retains the last delivered frame, cloning `Arc`-shared planes.
Measured after the fix: **0.001 ms**.

---

## 8. Media management and the UCG

Media is a first-class citizen of the graph, not a path string on a clip.

```
logistics.take ─Binds→ asset.media ─Contains→ media.stream
                            ▲                      ▲
          nle.clip ─Binds───┘                      │
       media.derivative ─DerivedFrom───────────────┘   (proxy, thumbnail…)
```

The edge kinds encode a distinction that matters:

* **`Binds`** — a clip or take *references* the media. When the media changes it
  needs **relinking**; its edit decisions survive untouched.
* **`DerivedFrom`** — a proxy or thumbnail is genuinely *derived*. When the
  media changes it is **stale** and must be regenerated.

`media_replacement_impact()` returns these as separate sets. **Conflating them
is how an NLE loses edit decisions during a conform**, which is why
`media_replacement_separates_relink_from_regenerate` asserts not only that a
clip appears in `bound` but that it never appears in `derived`.

Ingest is idempotent: re-ingesting the same file produces **zero operations**,
which reactor convergence depends on.

---

## 9. The viewer boundary

The slice ends at a **viewer-consumable frame**, and stops deliberately short of
a viewer. What a consumer receives is: exact `pts` in the stream's own base,
plane data with explicit strides, a declared `PixelFormat`, and colour metadata
that is either complete or explicitly absent.

That is the correct handoff point to `cine-gpu`, because YUV→RGB conversion must
be fused with the colour transform on the GPU rather than done on the CPU here.
The GPU research supports doing this in our own WGSL rather than via
fixed-function paths: mpv's manual documents that vdpau *"does not support newer
colorspaces like BT.2020 correctly"* and dxva2 *"appears to always use BT.601…
some drivers convert to limited range RGB, which gives a faded appearance."*
Vulkan's fixed-function `VkSamplerYcbcrConversion` exists and libplacebo/mpv
deliberately do not use it — the same call CINE-OMNI should make.

**No playback exists yet, and none is simulated.** There is no timer-driven fake
clock and no fabricated progress. A/V sync, the presentation clock and the
threaded pipeline are Phase 2b (§12).

---

## 10. Failure and recovery

Every failure mode below is pinned by a test in `tests/real_media.rs`.

| Condition | Behaviour |
|---|---|
| File absent | `Error::NotFound` |
| File vanished between probe and open | `Error::Offline` |
| Present but not media | `Error::Container`, naming the file |
| Unhandled extension | `Error::UnknownContainer` |
| Codec with no decoder | `Error::NoDecoder { codec, available }` — names the codec **and** what the build can do |
| Unsupported H.264 profile | Refused at probe; never attempted |
| Truncated NAL in a sample | Keeps the readable head, drops the damaged tail — a damaged take is not discarded whole |
| Corrupt audio packet | Skipped, stream continues |
| Illegal NAL length size | Rejected, not guessed |
| Empty access unit | Error — never a silent blank frame |
| Seek before zero | `None` |
| Seek past end | Clamps to the last frame |
| Media offline | Project stays valid and editable; identity and streams survive |

**Source media is never modified.** Verified by inspection: no library path in
`cine-media` opens any file for writing. The only writes in the crate are test
fixtures created in temporary directories.

---

## 11. Capability matrix

Three columns, because "supported" hides the distinction that matters:
**measured** = verified here on a real file; **implemented** = code exists,
unverified on real media of that type; **no** = absent.

### Video

| Format | Demux | Decode | Status | Evidence / note |
|---|---|---|---|---|
| H.264 High 4:2:0 8-bit | yes | yes | **measured** | 240/240 frames, bit-plane geometry checked |
| H.264 Baseline / Main 4:2:0 8-bit | yes | yes | *implemented* | Same decoder path; no asset to verify |
| H.264 4:2:2 / 4:4:4 / 10-bit | yes | **no** | refused | Profile parse rejects; refusal is deliberate |
| HEVC / H.265 | identified | **no** | no | No credible pure-Rust decoder exists |
| ProRes | no | **no** | no | No pure-Rust implementation. Apple names FFmpeg an *unauthorized* implementation |
| DNxHD / DNxHR | no | **no** | no | Preferred mastering intermediate when encode arrives |
| FFV1 | no | **no** | no | |
| VP9 / AV1 | identified | **no** | no | `rav1e` (encode) builds without C; decode not wired |
| Camera RAW (R3D, ARRIRAW, X-OCN) | no | **no** | no | Vendor SDKs; out-of-process host is the design |

### Images

| Format | Status | Note |
|---|---|---|
| PNG | **measured** | Real system PNG decoded to `Rgba8` |
| JPEG, TIFF | *implemented* | `image` crate features enabled; unverified here |
| OpenEXR | **no** | `exr` crate exists and is Rust; feature not enabled |
| DPX, Cineon | **no** | Would be in-house |
| Image sequences | **no** | A still is modelled as one frame; sequence assembly absent |

### Audio

| Format | Status | Evidence |
|---|---|---|
| AAC-LC | **measured** | Real AAC decoded, verified non-silent, ~10.03 s |
| PCM (WAV) | **measured** | Real system WAV decoded with signal |
| MP3, FLAC, ALAC, Vorbis | *implemented* | symphonia codecs enabled; unverified here |
| AIFF | **no** | Not enabled |
| Broadcast WAV (BWF) metadata | **no** | Timecode chunk unparsed |

### Timing and metadata

| Capability | Status | Note |
|---|---|---|
| Exact rational time | **measured** | Cross-base comparison exact in `i128` |
| SMPTE timecode, drop-frame | **implemented** | Unit-tested both directions |
| Embedded timecode track | **no** | `start_timecode` is always `None` |
| CFR frame rate | **measured** | 24 fps derived exactly from the sample table |
| VFR detection | **weak** | Derived from the *mean* sample duration; a genuinely variable stream whose mean lands on a known rate is misreported as CFR. Needs a full `stts` scan |
| Colour metadata (H.273) | **structure only** | Representable and honestly absent; `colr` box and VUI **not parsed** |
| HDR (PQ/HLG, MDCV, CLLI) | **no** | Representable in principle; nothing parsed |
| Alpha | structure only | `AlphaMode` exists; no format that carries alpha is decoded |

### Pipeline

| Capability | Status |
|---|---|
| Content-addressed identity, relink, offline | **measured** |
| Frame-accurate seek (video and audio) | **measured** |
| Byte-bounded frame cache | **measured** |
| Hardware decode | **no — and not claimed.** No hardware decode path exists, so none is asserted |
| Playback / presentation clock / A/V sync | **no** — and not simulated |
| Proxy generation, thumbnails, waveforms, scopes | **no** |
| Threaded decode with backpressure | **no** — synchronous by design at this stage |

---

## 12. Measured performance

Reference asset, this machine, single-threaded, release build. Best-of and mean.

| Operation | Best | Mean | Derived |
|---|---|---|---|
| Fast hash | 21.8 ms | 27.8 ms | ingest default |
| Full hash | 59.1 ms | 67.6 ms | 93 MiB/s |
| Probe | 17.8 ms | 28.0 ms | — |
| Decode all audio | 62.0 ms | 86.0 ms | **162× realtime** |
| Audio seek + decode | 0.05 ms | 0.08 ms | — |
| **Decode all video** | **9.81 s** | 11.79 s | **24.5 fps = 1.02× realtime** |
| Video seek, near entry | 0.40 s | 0.47 s | — |
| Video seek, deep in GOP | 1.77 s | 2.31 s | worst case |
| Re-request play-head frame | <0.001 ms | — | after the fix in §7 |
| 200 cache hits | 0.19 ms | 0.20 ms | 131× cheaper than re-decode |

**The headline number is bad and is not being dressed up.** Video decode sits
*at* realtime, not above it. Across runs on an otherwise-loaded machine the
best-of figure ranged **22.1–24.5 fps (0.92–1.02× realtime)** and the mean
**19.6–20.4 fps (0.82–0.85×)**, on a 720×1280 24 fps clip. In other words it
cannot reliably sustain playback of its own source, single-threaded and in
software — and this is the *easy* case: 720p, 8-bit, 4:2:0. A 4K timeline is not
within reach of this path.

Worst-case scrub is **1.8–2.9 s**, because this file has a 120-frame GOP and
entry is only possible at a keyframe.

That is precisely what proxies, a keyframe index, threaded decode and hardware
decode exist to fix, and it is why none of them can be skipped. Reporting this
as adequate would make every later performance decision unarguable.

---

## 13. Dependency and licence audit

The workspace spine is Apache-2.0, which is **one-way incompatible with GPL**.
`cargo deny check` passes: *advisories ok, bans ok, licences ok, sources ok*.

| Crate | Version | Licence | Compatible | Role / note |
|---|---|---|---|---|
| `symphonia` | 0.5 | **MPL-2.0** | yes, with care | Audio demux + decode. File-level copyleft: modifications to *its* files stay MPL. We do not modify it. Distribution alongside Apache-2.0 code is fine |
| `mp4` | 0.14 | MIT | yes | ISO-BMFF sample tables, `avcC`, `stts`/`ctts`/`stss` |
| `rusty_h264` (+ `-decoder`, `-common`, `-encoder`) | 0.16 | **BSD-2-Clause** | yes | Pure-Rust H.264. Permissive, no patent grant clause |
| `image` | 0.25 | MIT OR Apache-2.0 | yes | PNG/TIFF/JPEG, default features off |
| `sha2` | 0.10 | MIT OR Apache-2.0 | yes | Content hashing |
| `indexmap`, `parking_lot`, `serde`, `thiserror` | — | MIT OR Apache-2.0 | yes | Pre-existing |
| `tempfile` (dev) | 3 | MIT OR Apache-2.0 | yes | Tests only |

**No GPL and no LGPL anywhere in the `cine-media` tree** — verified by
enumerating every licence in the dependency graph. The full distinct set is
Apache-2.0, MIT, BSD-2, BSD-3, MPL-2.0, Zlib, 0BSD, Unicode-3.0, Unlicense.

Three findings that constrain future work, from `research/08`:

* **`ffmpeg-next` and `ffmpeg-the-third` are WTFPL** — no patent grant, no
  warranty disclaimer, commonly rejected by `cargo-deny` policy. If FFmpeg is
  ever bound, **`rsmpeg` (MIT)** is the binding to use, against an LGPL-2.1
  build configured `--disable-gpl --disable-version3`, shared-linked.
* **`rawloader`/`rawler` are LGPL-2.1 and `imagepipe` is LGPL-3.0-only.** Rust
  links statically, so there is no LGPL §6(b) relinking escape. **Camera-RAW
  stills ingest is therefore not available on the terms usually assumed.**
* **Patent obligations are separate from copyright.** H.264 and HEVC carry
  patent pools. A BSD-2 decoder settles the *copyright* question and says
  nothing about the *patent* one. This needs counsel before any distribution and
  is not an engineering decision — logged in §14.

---

## 14. Unresolved decisions

Listed explicitly because pretending these are settled would be the most
damaging thing this document could do.

1. **Is `rusty_h264` acceptable for production?** Ten weeks old, one profile
   verified here, and it decodes *beyond the scope its authors advertise*.
   Mitigated by the trait boundary, not resolved by it. Needs a conformance run
   against the JVT corpus before any claim stronger than "the slice decodes".
2. **Patent licensing for H.264/HEVC distribution.** Legal, not engineering.
   Blocks distribution, not development.
3. **FFmpeg backend: yes or no?** Buys HEVC, ProRes, DNx, VFR and hardware
   decode. Costs a C build dependency, an LGPL obligation, and a shared-linking
   requirement. Cannot be evaluated on this machine at all (no `pkg-config`,
   `make`, `nasm`), which is itself an argument for deciding it elsewhere.
4. **Colour metadata policy when absent.** The three major tools disagree
   (§5). Options: refuse to interpret; tag explicitly-unknown and require the
   user to choose; follow IMF and demand completeness. Recommendation is the
   second, but it is a product decision.
5. **Proxy format.** DNxHR is indicated over ProRes on the authorisation point,
   but nothing can be encoded yet.
6. **Where YUV→RGB happens.** Design says GPU, fused with the colour transform.
   Not yet built, so not yet proven.
7. **True VFR handling.** Detection is currently weak (§11) and conform policy
   is undecided.
8. **Threading model.** Backpressure and cancellation are specified but
   unbuilt; the synchronous path exists first so correctness questions are
   answerable.

---

## 15. Implementation phases

**2a — the vertical slice. Done and measured.**
Identity, exact time, probe, demux, real audio decode, real video decode,
frame-accurate seek, byte-bounded cache, UCG integration, honest failure modes.

**2b — playback.** Threaded decode with backpressure and cancellation; a real
presentation clock; A/V sync; the GPU upload path and YUV→RGB in WGSL.

**2c — breadth.** Keyframe index; proxy generation; thumbnails and waveforms;
image sequences; EXR/DPX; `colr` and VUI parsing; embedded timecode tracks; a
true `stts` scan for VFR.

**2d — the backend decision.** Resolve §14.3. If FFmpeg is adopted, it slots in
behind `VideoDecoder` and everything above it is unchanged. That is the whole
point of the trait.

---

## 16. What this phase does *not* claim

Per the directive's evidence rule, stated plainly:

* **Not "industry-grade".** One asset, one profile, no conformance suite.
* **Not ACES.** No colour transform of any kind is implemented.
* **Not OTIO.** No interchange schema is implemented here.
* **No hardware decode.** No such path exists, so nothing is asserted about it.
* **No playback.** No presentation clock and no A/V sync — and no timer-driven
  simulation of either.
* **Not a general H.264 decoder.** 4:2:0 8-bit Baseline/Main/High only, and
  only High was verified.
