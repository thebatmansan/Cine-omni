# CINE-OMNI Industry Blueprint — 08: The Media / Codec / Decoding Stack

**Status:** research draft, with a measured-on-this-machine section.
**Date of research:** 2026-09-08.
**Scope:** Phase 2's media foundation — containers, decoders, time, hardware
acceleration, professional camera media, image sequences and metadata.
**Relationship to earlier work:** [`02-editorial-media.md`](02-editorial-media.md)
established FFmpeg's LGPL/GPL split, the four build modes, LGPL-2.1 §6, the
hardware-vendor matrix and the proxy/cache architecture. **That work is not
repeated here.** This document goes one layer deeper — to the *API boundary* —
and adds the thing that document could not: an empirical answer to "what can
actually be built and run on the machine in front of us today."

> **Not a legal opinion.** Engineering analysis of publicly stated terms.
> Every license, version and URL below was checked on the date above, or is
> marked `UNVERIFIED`. Nothing is stated from memory.

---

## 0. The two questions, kept separate throughout

The brief asks two questions that must never be conflated:

**(a) What is the correct long-term architecture**, assuming a normal build
machine with a C toolchain, a package manager and an FFmpeg install?

**(b) What is implementable and verifiable *today*, in Rust, on this machine?**

The answers are different, and the honest report is that they are *less*
different than expected — for reasons established by measurement in §1.

---

## 1. Machine reality, measured today

Everything in this section was executed on this machine on 2026-09-08. It is
evidence, not assertion.

### 1.1 The toolchain probe

| Probe | Result |
|---|---|
| `rustc --version` | `rustc 1.98.1 (48a229cea 2026-09-01)` |
| `/usr/bin/{cc,gcc,clang,g++,ld}` | **none present** |
| `pkg-config` | **not on PATH** |
| `ffmpeg` binary | **not installed** |
| `libavcodec/libavformat/libavutil/libswscale/libswresample` | **absent** (`ls /usr/lib/x86_64-linux-gnu | grep -i libav` returns only `libavahi-*` and `libavc1394` — unrelated projects that merely share a prefix) |
| `~/.local/zig/zig version` | **`0.16.0`** |
| `~/.local/bin/zigcc` | `exec "$HOME/.local/zig/zig" cc -target x86_64-linux-gnu.2.39 "$@"` |

### 1.2 The most consequential finding: there *is* a C toolchain

The brief's premise was that "anything requiring a C toolchain cannot be built
or verified on this machine today." **That premise is false, and the correction
changes several recommendations.**

Zig 0.16.0 is installed at `~/.local/zig/zig`. `zig cc` and `zig c++` are a
complete Clang-based C/C++ front end with bundled libc headers, a bundled
libc++, and a bundled `ar`. Verified by compiling and running a C program that
links `libm`:

```
$ ~/.local/zig/zig cc -O2 t.c -o t && ./t
zig cc works: 1.414214
```

The reason C-dependent crates *appear* to fail is not the absence of a
compiler. It is two mechanical mismatches, both of which were fixed in minutes:

1. The `cc` crate passes `--target=x86_64-unknown-linux-gnu`; Zig's target
   parser rejects the vendor field (`unable to parse target query
   'x86_64-unknown-linux-gnu': UnknownOperatingSystem`). A three-line shell
   wrapper that drops `--target=*` and substitutes `-target
   x86_64-linux-gnu.2.39` resolves it.
2. The `cc` crate then needs an archiver; `AR` must be pointed at `zig ar`.

With `CC`/`CXX`/`AR` set to those wrappers, **the OpenH264 C++ codebase
compiled cleanly from source** via `openh264-sys2`'s `build.rs` (exit code 0).
That is roughly 500 C++ translation units of real, SIMD-adjacent codec code.

**What this does and does not mean.** It means C/C++ *source-building* crates
are reachable here. It does **not** mean FFmpeg is reachable: FFmpeg's build
additionally needs a POSIX shell `configure` run, GNU Make, and `nasm`/`yasm`
for its assembly — none of which were probed successfully, and `pkg-config` is
absent, which is what the Rust binding crates use to *find* an existing build.
Confirmed empirically:

```
$ cargo build   # ffmpeg-next 9.0.0
error: FFmpeg not found.
  1. FFMPEG_DIR environment variable (not set)
  2. vcpkg package manager (ffmpeg package not found)
  3. pkg-config (libavutil not found ... The pkg-config command could not be found.)
```

So the accurate statement is: **FFmpeg cannot be built or linked here today;
individual C/C++ codec libraries that ship their own `cc`-crate build script
can be.**

### 1.3 The test asset, parsed

`/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4` — 5,782,988
bytes, `ftyp isom` with compatible brands `isom iso2 avc1 mp41`.

Parsed with `re_mp4` 0.5.1 (pure Rust):

| Property | Measured value |
|---|---|
| Video track | `avc1.64001F`, 720 × 1280, timescale **12288**, duration **122880** (= 10.000 s), **240** samples |
| `avcC` | 46 bytes; NAL length size **4**; profile_idc **0x64 = High**; level_idc **0x1F = 3.1** |
| `stts` distinct sample durations | **`{512: 240}`** — every sample is 512/12288 s, i.e. **exactly 24.000 fps CFR** |
| Sync samples (`stss`) | **2 of 240**, at sample indices **0 and 189** |
| Samples where `cts != dts` | **184 of 240** — heavy B-frame reordering |
| Audio track | timescale 48000, duration 481280 (= 10.0267 s), 470 samples |

Two of those rows are the whole argument of §2 in miniature: a 240-frame clip
with **two** keyframes means seeking to frame 188 requires decoding 189
frames; and 184 samples with `cts != dts` means decode order is not
presentation order, so a naive "count packets" frame index is wrong.

### 1.4 Audio decode, verified end to end (pure Rust)

`symphonia` 0.5.5 with `features = ["all"]`, no C anywhere in the tree,
against the real asset:

```
track id=1 codec=CodecType(4100) tb=1/48000 nframes=481280 rate=48000
audio packets=470 decoded_frames=481280 peak=0.90728486
```

481,280 samples at 48 kHz = 10.0267 s — exactly the container duration. The
ISO-MP4 demuxer and the AAC-LC decoder both work on a real-world file. Note
symphonia reports the **video** track as `CodecType(0)` (null): it enumerates
the track and its timing but has no video decoder, by design.

### 1.5 Video decode, verified end to end — and the surprise

This is the finding that most changes the Phase 2 plan.

**Setup.** Samples were pulled from the `mdat` via `re_mp4`, converted from
AVCC length-prefixed form to Annex-B, with SPS/PPS extracted from `avcC` and
prepended to the first access unit.

**Result 1 — OpenH264 (Cisco C++, compiled here with `zig c++`):**

```
openh264: decoded 9 frames from 60 samples, dims=Some((720, 1280))
```

OpenH264's decoder does not handle this stream. It is a Constrained-Baseline-
class decoder; this is High profile with B-frames, so it silently produces
output for a small minority of access units. **An honest media engine cannot
claim "H.264 support" on the strength of OpenH264.**

**Result 2 — `rusty_h264` 0.16.0 (pure Rust, BSD-2-Clause):**

```
rusty_h264: decoded 240 frames from 240 samples, 0 errors, dims 720x1280
pure-Rust full decode: 240 frames, 0 errors, 9.871 s => 24.3 fps at 720x1280
per-frame mean luma, first 8: [47, 51, 50, 49, 51, 50, 51, 51] ... last 4: [34, 30, 26, 22]
distinct per-frame mean-luma values across 240 frames: 35
```

Every frame of a High-profile, B-pyramid, 189-frame-GOP stream decoded without
error, single-threaded, at real time (the clip is 24 fps; two runs measured
24.3 and 31.2 fps depending on machine load). The per-frame mean-luma series
varies plausibly and fades toward black at the end, which is consistent with
real picture content rather than noise.

**Result 3 — the cross-check that makes results 1 and 2 admissible.**
OpenH264 *can* decode the IDR at sample 0. Decoding that one access unit with
both decoders and comparing planes directly:

```
rusty_h264 IDR frame 720x1280 y_len=921600
openh264  IDR frame 720x1280
IDR chroma cross-check: u_len=230400 v_len=230400 differing_samples=0
IDR luma cross-check (openh264 vs rusty_h264): pixels=921600 differing=0
                                               max_abs_diff=0 mean_abs_diff=0.000000
```

**Bit-identical**, luma and chroma, against Cisco's reference implementation,
on a real-world IDR frame.

**Honest limits of this evidence.** One asset. One profile (High 3.1, 4:2:0
8-bit). One IDR compared. No conformance suite was run here — `rusty_h264`'s
own claim is that it is "validated **bit-exact** against Cisco's `h264dec`
over openh264's conformance corpus," which is the crate's claim, not this
document's measurement. The crate is **new**: first published 2026-06-27,
version 0.16.0 as of 2026-09-07, 17,757 total downloads. That is ten weeks of
public existence. It is **not** yet a dependency to bet a product on. It *is*
a dependency to build a vertical slice on, behind a trait, today.

### 1.6 The whole pure-Rust candidate stack, compiled together

To confirm the §10.3 slice is not a paper exercise, one crate depending on
**all** of it was built here with no `CC`/`CXX` set at all:

```toml
rav1e = { version = "0.8", default-features = false }   # -> 0.8.1
exr = "1.74"                                            # -> 1.74.2
image = "0.25"                                          # -> 0.25.10
zune-image = "0.5"                                      # -> 0.5.0
symphonia = { version = "0.6", features = ["all"] }     # -> 0.6.1
tiff = "0.11"                                           # -> 0.11.3
kamadak-exif = "0.6"                                    # -> 0.6.1
dcv-color-primitives = "1"                              # -> 1.0.0
```

```
Finished `release` profile [optimized] target(s) in 2m 09s
$ cargo tree -e build | grep -iE '\bcc v|nasm|bindgen|cmake|pkg-config|system-deps'
(no output)
```

**Not one build dependency in that tree compiles C.** Including `rav1e` with
`default-features = false`, which gives AV1 encode in pure Rust without NASM.

---

## 2. FFmpeg at the API level

Long-term, FFmpeg remains the correct primary backend for the reasons
document 02 established. This section specifies the API surface CINE-OMNI
should code against, because the shape of that surface determines the shape of
`cine-media`'s traits.

### 2.1 `libavformat` — demuxing

The container layer is four objects: `AVFormatContext` (the file),
`AVStream` (one elementary stream), `AVPacket` (one compressed access unit),
and the input `AVInputFormat`.

`av_read_frame()` is documented to "Return the next frame of a stream," and
critically: each returned packet is **reference counted** and "remains valid
indefinitely until freed with `av_packet_unref()`." Timestamps are "set to
correct values in `AVStream.time_base` units, though pts may be
`AV_NOPTS_VALUE` for video formats with B-frames."
(Verified: <https://ffmpeg.org/doxygen/trunk/group__lavf__decoding.html>)

That last clause is the reason a media engine must never assume `pkt->pts` is
present. The authority for presentation time is the *frame*, not the packet.

### 2.2 Seeking — and why `AVSEEK_FLAG_BACKWARD` is not optional

Two entry points exist, and they behave differently.

**`av_seek_frame(ctx, stream_index, timestamp, flags)`** — documented as "Seek
to the keyframe at timestamp." When `stream_index == -1`, "a default stream is
automatically selected and timestamp converts from `AV_TIME_BASE` units to
stream-specific time_base." So the timestamp's units depend on which stream you
name; passing a stream index means the timestamp is in *that stream's*
`time_base`, and passing −1 means microseconds. Mixing these up is the single
most common seeking bug.

**`avformat_seek_file(ctx, stream_index, min_ts, ts, max_ts, flags)`** —
documented to "Seek to timestamp ts … so that the point from which all active
streams can be presented successfully will be closest to ts and within
min/max_ts." The flag semantics on this newer API are documented as:

| Flag | Documented meaning (`avformat_seek_file`) |
|---|---|
| `AVSEEK_FLAG_BYTE` | "all timestamps are in bytes and are the file position" |
| `AVSEEK_FLAG_FRAME` | "all timestamps are in frames in the stream with stream_index" |
| `AVSEEK_FLAG_ANY` | "non-keyframes are treated as keyframes" |
| `AVSEEK_FLAG_BACKWARD` | **"ignored"** |

(Verified: <https://ffmpeg.org/doxygen/trunk/group__lavf__decoding.html>)

This is a real API trap and it must be written into `cine-media`'s design
notes. `AVSEEK_FLAG_BACKWARD` — "seek to the nearest keyframe *at or before*
the target" — is meaningful for `av_seek_frame`, and is documented as
**ignored** for `avformat_seek_file`, where the `min_ts`/`ts`/`max_ts` window
expresses the same intent (set `max_ts = ts` to force landing at or before).

`AVSEEK_FLAG_ANY` must be treated as a debugging tool only. It "treats
non-keyframes as keyframes," which means the decoder is handed a P- or B-slice
with no reference pictures and produces corrupt output. It has no place in a
finishing application.

### 2.3 Time bases and PTS/DTS, stated precisely

FFmpeg represents time as `AVRational { int num; int den; }`. The docs are
explicit about *why*: rationals can be expressed as floats but "the conversion
process is a lossy one," and rationals are what make "highly accurate
calculation of timestamps" possible.
(Verified: <https://ffmpeg.org/doxygen/trunk/group__lavu__math__rational.html>)

Three distinct time bases exist in a decode pipeline and confusing them is the
canonical source of one-frame drift:

| Time base | Where it lives | Meaning |
|---|---|---|
| **Stream time base** | `AVStream.time_base` | The unit of `AVPacket.pts/dts` as read from the container. For our test asset this is **1/12288**. |
| **Codec time base** | `AVCodecContext.time_base` | Encoder-side; **not** a reliable decode-side quantity. |
| **Frame time base** | `AVFrame.time_base` | Units of `AVFrame.pts`. |

The `AVFrame` timestamp fields are documented as:
`pts` = "Presentation timestamp in time_base units (time when frame should be
shown to user)"; `pkt_dts` = "DTS copied from the AVPacket that triggered
returning this frame"; `best_effort_timestamp` = "frame timestamp estimated
using various heuristics, in stream time base."
(Verified: <https://ffmpeg.org/doxygen/trunk/structAVFrame.html>)

**PTS vs DTS.** DTS is decode order; PTS is presentation order. They differ
exactly when the codec reorders — B-frames. Our test asset has **184 of 240
samples with `cts != dts`**, so the two orders differ for 77% of the file.
Anything that indexes frames by arrival order is wrong on this file.

**Converting a stream timestamp to a frame number, frame-accurately.**
Given a stream time base `tb = num/den` and a *nominal constant* frame
duration `d` in the same time base, the frame index of a presentation
timestamp `pts` is exact integer arithmetic:

```
frame_index = pts / d                (integer division, both in stream units)
```

For our asset: `d = 512`, `tb = 1/12288`, so frame 189 has `pts = 96768`, and
`96768 / 512 = 189` exactly — no floating point anywhere. The general
conversion between time bases is `av_rescale_q(a, bq, cq)`, which computes
`a * bq / cq` in 64-bit with rounding control; `av_rescale_q_rnd` exposes the
`AVRounding` mode explicitly, and `AV_ROUND_PASS_MINMAX` exists so that
`AV_NOPTS_VALUE` survives a rescale instead of becoming a real timestamp.
(The rational-utilities page documents `AVRational`, `av_q2d`, `av_d2q`,
`av_add_q`, `av_mul_q`; `av_rescale_q`/`av_rescale_q_rnd` and the `AVRounding`
enum are documented on the mathematics/`libavutil` pages rather than the
rational page — the exact rounding-mode list is `UNVERIFIED` here and must be
read from `libavutil/mathematics.h` before it is relied on.)

**The rule for CINE-OMNI:** never convert a timestamp through `f64`. The UCG
already has `RationalTime`, and `AVRational` maps onto it directly. That
mapping should be the *only* bridge; a `to_seconds()` helper must exist for UI
display and must be forbidden in any code path that computes an index.

**And the harder rule:** the formula above is only valid because `stts` said
`{512: 240}`. It is **invalid for variable-frame-rate media** (§8). The engine
must therefore store a per-source presentation-time table and binary-search it,
treating CFR as a compressible special case of VFR — never the other way round.

### 2.4 Frame-accurate seeking, as an algorithm

Combining §2.2 and §2.3, the industry-standard recipe — and the one CINE-OMNI
should implement — is:

1. Convert the target frame index to a stream-time-base timestamp
   (`target_pts`), by table lookup, not multiplication.
2. `avformat_seek_file(ctx, vstream, INT64_MIN, target_pts, target_pts, 0)` —
   the `max_ts = target_pts` window forces landing at or before the target.
   (With the older `av_seek_frame`, pass `AVSEEK_FLAG_BACKWARD` instead.)
3. `avcodec_flush_buffers()` on the decoder. Skipping this is why "seeking
   sometimes shows the previous shot for one frame."
4. Decode forward, discarding every frame whose `best_effort_timestamp <
   target_pts`, until the first frame with `pts >= target_pts`.
5. That frame is the answer.

The cost of step 4 is the distance to the preceding keyframe. **Our test asset
makes that cost concrete: keyframes at samples 0 and 189, so seeking to frame
188 decodes 189 frames.** This single measurement is the entire justification
for the proxy/cache architecture already specified in `02-editorial-media.md`
§4 — random access to long-GOP delivery media is not a decode problem that can
be optimised, it is a *media-management* problem that must be solved by
generating all-intra proxies.

### 2.5 `libavcodec` — decoding

The modern API is the send/receive pair, and its contract is precisely
documented:

* `avcodec_send_packet()` returns `AVERROR(EAGAIN)` meaning "input is not
  accepted in the current state - user must read output with
  `avcodec_receive_frame()`", and `AVERROR_EOF` meaning "the decoder has been
  flushed, and no new packets can be sent to it."
* `avcodec_receive_frame()` returns `AVERROR(EAGAIN)` meaning "output is not
  available in this state - user must try to send new input", and
  `AVERROR_EOF` meaning "the codec has been fully flushed, and there will be
  no more output frames."
* Draining is initiated with "a null packet or one with `data=NULL` and
  `size=0`"; "buffered frames are returned before EOF."
* Input buffers "must be `AV_INPUT_BUFFER_PADDING_SIZE` larger than the actual
  read bytes because some optimized bitstream readers read 32 or 64 bits at
  once and could read over the end."

(Verified: <https://ffmpeg.org/doxygen/trunk/group__lavc__decoding.html>)

This is a **one-in, many-out, many-in-one-out** state machine, not a function
call. It maps naturally onto a Rust `Iterator`/`Stream` and very badly onto a
`decode(packet) -> Frame` signature. `cine-media`'s trait must expose the
send/receive shape, or it will fight both FFmpeg and every pure-Rust decoder
that copies FFmpeg's shape.

**Threading.** `AVCodecContext.thread_count` and `thread_type`
(`FF_THREAD_FRAME` / `FF_THREAD_SLICE`) control intra-decoder parallelism.
Frame threading increases latency by roughly `thread_count` frames — which
matters for scrubbing and not at all for background transcode, so the two use
cases want different settings on the same file. A codec context is not
shareable across threads; per-stream contexts with an outer work queue is the
correct pattern.

**`AVFrame` ownership — the part that bites in Rust.** `AVFrame` "must be
allocated using `av_frame_alloc()`, which only allocates the `AVFrame` itself,
the buffers for the data must be managed through other means." Pixel data is
reference-counted through `AVBufferRef` in `buf[]` (up to 8) plus
`extended_buf`; a frame is reference-counted "if at least one reference is set,
i.e. if `buf[0] != NULL`. In such a case, every single data plane must be
contained in one of the buffers." And decisively:
`avcodec_receive_frame()` "will always call `av_frame_unref(frame)` before
doing anything else."
(Verified: <https://ffmpeg.org/doxygen/trunk/structAVFrame.html>)

The consequence for a Rust wrapper: the `AVFrame` handed back by
`receive_frame` is **borrowed until you `av_frame_ref` it into your own
frame**. Any cache that stores frames must take its own reference. Any binding
that exposes the raw frame with a `'static` lifetime is unsound. This is a
concrete criterion for judging the binding crates in §3.

### 2.6 `libavutil` — the primitives that leak into our data model

* **`AVRational`** — §2.3. Maps to `ucg-schema`'s `RationalTime`.
* **`AVPixelFormat`** — the enumeration that decides whether a frame is
  `YUV420P`, `YUV422P10LE`, `P010`, `GBRAPF32LE`, or a hardware handle. Note
  the semantic split: `AV_PIX_FMT_*` values with `AV_PIX_FMT_FLAG_HWACCEL` are
  *opaque surface handles*, not pixel layouts.
* **`AVBufferRef`** — the refcounted allocation under every plane. This is
  what makes zero-copy frame sharing possible and what a Rust binding must
  model honestly.
* **`AVHWFramesContext`** — documented with the crucial `format` vs
  `sw_format` distinction: `format` is "the pixel format identifying the
  underlying HW surface type. Must be a hwaccel format"; `sw_format` is "the
  pixel format identifying the actual data layout of the hardware frames," and
  should be "the fully planar version of that format (e.g. for 8-bit 420 YUV it
  should be `AV_PIX_FMT_YUV420P`, not `AV_PIX_FMT_NV12`)" where the API does
  not tell you exactly. Frames come from a `pool`; `initial_pool_size` is the
  hard maximum for device types that cannot grow. Each frames context "is tied
  to a given `AVHWDeviceContext` instance."
  (Verified: <https://ffmpeg.org/doxygen/trunk/structAVHWFramesContext.html>)
* **HDR metadata** rides as `AVFrameSideData`:
  `AV_FRAME_DATA_MASTERING_DISPLAY_METADATA` (SMPTE ST 2086 primaries and
  luminance), `AV_FRAME_DATA_CONTENT_LIGHT_LEVEL` (MaxCLL/MaxFALL), and
  `AV_FRAME_DATA_DYNAMIC_HDR_PLUS` (ST 2094-40). The exact current enumerator
  list should be read from `libavutil/frame.h` at integration time —
  `UNVERIFIED` here beyond the general side-data mechanism documented on the
  `AVFrame` page. **Side data is the single most commonly dropped thing in a
  media pipeline**, and it is exactly the data CINE-OMNI's color crate needs.
  Whatever `cine-media`'s frame type is, it must carry side data forward or
  the ACES pipeline downstream is guessing.

### 2.7 `libswscale` and `libswresample` — use, or not?

**`libswresample`: yes, or a trivial replacement.** Audio format conversion and
resampling is a solved, low-risk problem. `cine-audio` already exists and does
its own RIFF/WAVE I/O; a high-quality Rust resampler (e.g. `rubato`, license
`UNVERIFIED` here) is a credible alternative that avoids one more C dependency.

**`libswscale`: no, not on the interactive path.** Three reasons:

1. It is a *CPU* converter. Every frame it touches is a frame that has already
   been copied out of the decoder and will be copied again into a GPU texture.
   For a 4K timeline that is the dominant cost.
2. Its chroma-siting and colour-primaries handling is widely regarded as
   approximate — which is precisely why FFmpeg also ships the `zscale` filter
   wrapping **zimg**, a C++17 library that "implements the commonly required
   image processing basics of scaling, colorspace conversion, and depth
   conversion" (verified: <https://github.com/sekrit-twc/zimg>; the repository
   reports **WTFPL**, which is itself a licensing concern — see §3.3).
3. CINE-OMNI already has `cine-color` (9,890 LOC of ACES/OCIO work) and
   `cine-gpu` (wgpu compute). Doing YUV→RGB, chroma upsampling, matrix and
   transfer-function work as a wgpu compute shader keeps the pixels on the GPU,
   keeps the colour maths inside the crate that owns colour, and removes a
   dependency. **This is the right call and it is buildable today** — it needs
   no C.

The decoder's job therefore ends at "planar YUV in a known pixel format, plus
colour side data." Everything after that is `cine-gpu` + `cine-color`.

---

## 3. Rust bindings to FFmpeg

### 3.1 The comparison

All rows verified via the crates.io API on 2026-09-08.

| Crate | Latest | Last publish | Recent downloads | License | Shape |
|---|---|---|---|---|---|
| **`ffmpeg-next`** | **9.0.0** | 2026-08-05 | 2,822,207 | **WTFPL** | Safe high-level wrapper; fork of the abandoned `ffmpeg` crate |
| `ffmpeg-sys-next` | 9.0.0 | 2026-08-05 | 2,932,840 | **WTFPL** | The `-sys` layer beneath it |
| **`ffmpeg-the-third`** | 6.0.0+ffmpeg-9.0 | 2026-08-09 | 25,417 | **WTFPL** | Second fork of the same lineage, FFmpeg 5+ |
| **`rsmpeg`** | 0.18.0+ffmpeg.8.0 | 2025-08-24 | 33,018 | **MIT** | Thin, near-1:1 safe layer; "Supported FFmpeg versions are `6.*`, `7.*`" |
| `rusty-ffmpeg` | 0.17.0+ffmpeg.8.1 | 2026-04-10 | 47,031 | **MIT** | The `-sys` layer beneath `rsmpeg` (CCExtractor) |
| **`ac-ffmpeg`** | 0.19.0 | 2025-05-16 | 2,273 | **MIT** | Small curated API (Angelcam); low and falling usage |
| `video-rs` | 0.11.0 | 2026-02-24 | 115,961 | **MIT OR Apache-2.0** | High-level "read frames as ndarray" convenience layer over `ffmpeg-next` |
| `ffmpeg-sidecar` | 2.5.2 | 2026-05-30 | 501,768 | **MIT** | **Not a binding** — drives the `ffmpeg` *binary* as a subprocess |

Sources: <https://crates.io/api/v1/crates/{ffmpeg-next,ffmpeg-sys-next,ffmpeg-the-third,rsmpeg,rusty-ffmpeg,ac-ffmpeg,video-rs,ffmpeg-sidecar}>,
<https://github.com/zmwangx/rust-ffmpeg>, <https://github.com/larksuite/rsmpeg>,
<https://github.com/CCExtractor/rusty_ffmpeg/>,
<https://github.com/angelcam/rust-ac-ffmpeg>.

### 3.2 C at build time, and dynamic linking

**All of them require a C toolchain at build time**, because all of them are
`bindgen`-based `-sys` crates or wrap one, and `bindgen` requires `libclang`.
`ffmpeg-sys-next`'s own failure message enumerates its discovery strategy:
`FFMPEG_DIR`, then vcpkg, then `pkg-config` — and offers "Enable the 'build'
feature to compile FFmpeg from source" as the fallback (measured, §1.2).

**Dynamic linking is achievable with all of them**, which is what LGPL-2.1
§6(b) requires (see `01-LICENSING-SPINE.md` §4). It is a property of how
FFmpeg itself was built and of the linker flags, not of the binding crate: if
`pkg-config` resolves to shared `libav*.so` objects, Rust links against them
dynamically. The trap is the `build`/`static` feature paths, which produce a
statically linked FFmpeg inside the Rust binary and drag the project into
LGPL-2.1 §6(a) with its relinking-kit obligation. **`cine-media` must forbid
those features in `Cargo.toml` and `cargo-deny` should be configured to fail
the build if they appear.**

`rsmpeg`'s `link_system_ffmpeg` / `link_vcpkg_ffmpeg` features are the
supported way to point it at a system shared build.

### 3.3 The licensing surprise in this table

**`ffmpeg-next`, `ffmpeg-sys-next` and `ffmpeg-the-third` are WTFPL.** That is
not a rumour; it is the license field crates.io serves for the newest published
version of each. WTFPL is OSI-approved but it has **no patent grant, no
trademark clause, and no warranty disclaimer**. For a project whose entire
integration strategy is built on a licensing spine (and which chose Apache-2.0
specifically *for* its patent grant), depending on the most popular FFmpeg
binding means accepting a dependency with weaker terms than the product it is
embedded in. Many corporate OSPO allow-lists and default `cargo-deny`
configurations reject WTFPL outright.

This is not a blocker — WTFPL is maximally permissive in the copyright
dimension — but it is a decision that must be *made*, not stumbled into, and
it must be recorded in the third-party inventory required by
`01-LICENSING-SPINE.md` §6.

**It is also a reason to prefer `rsmpeg` (MIT).** `rsmpeg` is thinner and
therefore more work, but "thinner" is an advantage when the wrapper's main job
is to expose the send/receive state machine and `AVFrame` refcounting honestly
rather than to hide them.

### 3.4 The verdict on bindings

| Criterion | Winner |
|---|---|
| Ecosystem reach, breadth of API | `ffmpeg-next` |
| License hygiene for an Apache-2.0 product | **`rsmpeg`** |
| Honest exposure of refcounting / send-receive | **`rsmpeg`** |
| FFmpeg 9 support today | `ffmpeg-next` / `ffmpeg-the-third` |
| Not a viable primary | `ac-ffmpeg` (2,273 recent downloads, last publish 2025-05), `ffmpeg-sidecar` (subprocess, no frame access) |

**Recommendation: `rsmpeg` behind CINE-OMNI's own trait**, with the binding
choice confined to one crate (`cine-media-ffmpeg`) so it can be swapped. The
FFmpeg 8-vs-9 support gap is a real short-term cost of that choice and should
be re-checked before Phase 2 implementation begins.

`ffmpeg-sidecar` deserves one honest note: driving the `ffmpeg` **binary** as a
subprocess is a legitimate TYPE C/D integration that sidesteps LGPL linking
entirely and would even permit a GPL-enabled FFmpeg build. It is unusable for
interactive playback (no frame-level access, no seeking control, process
overhead per operation) but it is a perfectly good **transcode/proxy-generation**
backend, and it is worth keeping in the architecture for exactly that job.

---

## 4. The pure-Rust stack — what is buildable today

### 4.1 Audio: `symphonia`

The README's own support tables, verified at
<https://github.com/pdeljanov/Symphonia> (license **MPL v2.0**, latest **0.6.1**
per <https://crates.io/api/v1/crates/symphonia>, MSRV 1.85):

**Decoders**

| Codec | Status | Gapless | Feature | Default |
|---|---|---|---|---|
| AAC-LC | Great | No | `aac` | No |
| ADPCM | Good | Yes | `adpcm` | Yes |
| ALAC | Great | Yes | `alac` | No |
| FLAC | Excellent | Yes | `flac` | Yes |
| MP1 / MP2 | Great | No | `mp1`,`mp2`,`mpa` | No |
| MP3 | Excellent | Yes | `mp3`,`mpa` | No |
| PCM | Excellent | Yes | `pcm` | Yes |
| Vorbis | Excellent | Yes | `vorbis` | Yes |
| HE-AAC, HE-AACv2, Opus, WavPack | **"In work"** | — | — | — |

**Demuxers**

| Format | Status | Feature | Default |
|---|---|---|---|
| Wave | Excellent | `wav` | Yes |
| ISO/MP4 | **Great** | `isomp4` | **No** |
| OGG | Great | `ogg` | Yes |
| AIFF | Great | `aiff` | No |
| MKV/WebM | Good | `mkv` | Yes |
| CAF | Good | `caf` | No |

Three things matter for CINE-OMNI:

1. **MP3, AAC and ALAC are not default features.** Symphonia's default set is
   "royalty-free open standard codecs" only. Enabling `aac`/`mp3`/`alac` is a
   *patent* decision, not a build decision — the same axis
   `01-LICENSING-SPINE.md` §5 flags as separate from copyright.
2. **Symphonia decodes no video, ever.** It is explicitly an audio + demux
   library. But its ISO-MP4 demuxer enumerates video tracks with correct
   timing, which is useful.
3. **MPL-2.0 is file-level copyleft and therefore compatible** with Apache-2.0
   per the compatibility table in `01-LICENSING-SPINE.md` §2 — modifications to
   Symphonia's own files stay MPL, nothing spreads.

**Measured working on the real asset today** (§1.4).

### 4.2 Containers: which MP4 crate

| Crate | Latest | Last publish | Recent downloads | License |
|---|---|---|---|---|
| **`re_mp4`** | **0.5.1** | 2026-07-08 | 602,206 | **MIT** |
| `mp4` | 0.14.0 | **2023-08-01** | 1,920,550 | MIT |
| `mp4parse` | 0.17.0 | **2023-05-29** | 730,329 | **MPL-2.0** |

Sources: <https://crates.io/api/v1/crates/re_mp4>, `/mp4`, `/mp4parse`.

`mp4` and `mp4parse` (Mozilla's, used in Firefox) both have high download
counts but have not published since 2023. **`re_mp4`** (from the Rerun project)
is the only actively maintained one, and it is the one that was used to produce
every measurement in §1.3 and §1.5 — including `stts` durations, `stss` sync
flags and per-sample `decode_timestamp`/`composition_timestamp`, which is
exactly the VFR/reorder data §8 says the engine must have.

For MXF there is essentially nothing: crates.io has **`mxf` v0.1.2, MIT, last
published 2017-07-28** — a decade stale. The credible open MXF implementations
are C++: **bmx** (BBC/EBU) and **libMXF**, both **BSD-3-Clause**
(<https://github.com/bbc/bmx>, <https://github.com/BBC-archive/libMXF>) —
permissive and therefore linkable, but C++ and out of scope for a pure-Rust
slice.

### 4.3 Video: does a mature pure-Rust H.264/H.265 decoder exist?

**The honest answer, as of 2026-09-08: a *mature* one does not. A *working*
one does, and it is much better than expected.**

| Crate | Latest | First published | Recent downloads | License | Pure Rust? |
|---|---|---|---|---|---|
| **`rusty_h264`** | **0.16.0** (2026-09-07) | **2026-06-27** | 17,757 | **BSD-2-Clause** | **Yes** — no C in the tree; `#![forbid(unsafe_code)]` codec core; SIMD via Rust intrinsics, "no assembler, no build script" |
| `rust_h265` | 0.1.0 (2026-04-22) | 2026-04-22 | 175,486 | MIT OR Apache-2.0 | Claims pure Rust HEVC Main/Main10 8/10-bit 4:2:0. **Single version, never updated. Untested here.** |
| `h264-decoder` | 0.2.4 (2024-08-11) | 2023-03-12 | 1,611 | MIT | Part of the `xiu` streaming server; low usage |
| `openh264` | 0.9.8 (2026-08-08) | 2018 | 537,566 | BSD-2-Clause | **No** — ships Cisco's C++ and builds it with `cc` |
| `cros-codecs` | 0.0.6 (2025-06-18) | 2023-04-28 | 829,147 | BSD-3-Clause | ChromeOS **hardware** codecs for Linux — a VAAPI/V4L2 stateless-decoder framework, not a software decoder |

Sources: crates.io API for each; <https://github.com/remade-with-rust/rusty_h264>.

**What was measured (§1.5), restated as a finding.** `rusty_h264` 0.16.0
decoded all 240 frames of a real High-profile 4:2:0 8-bit stream with B-frames
at ~24–31 fps for 720×1280 single-threaded, and its IDR output was
**byte-identical to Cisco's OpenH264** across all 921,600 luma and 460,800
chroma samples. Meanwhile **OpenH264 itself failed this file**, producing 9
frames out of 60, because its decoder does not do B-frames.

That inverts the conventional wisdom. The received answer to "how do I decode
H.264 without FFmpeg" is "use OpenH264." On real-world High-profile media that
answer is wrong.

**Why this is still not a green light.** The crate is ten weeks old. There is
no independent conformance run in this document. HEVC has no equivalent
credible option (`rust_h265` is one version old and untested here). Nothing in
Rust decodes ProRes, DNxHR, or any professional intermediate. So:

> **`rusty_h264` is good enough to build and demonstrate the Phase 2 vertical
> slice on this machine today, behind a `VideoDecoder` trait. It is not good
> enough to be the shipping decoder, and the trait exists so that it does not
> have to be.**

The Cisco **binary** licence nuance from `02-editorial-media.md` §1.3 still
applies to OpenH264 and is worth restating because it changes the integration
shape: Cisco's royalty shelter requires the binary to be *separately downloaded
by the end user*, enableable/disableable, with attribution displayed
(<https://www.openh264.org/BINARY_LICENSE.txt>). The `openh264` crate's default
`source` feature — "We directly ship OpenH264 source code and provide simple,
hand-crafted compilation via `cc` in `build.rs`" — **compiles from source and
therefore forfeits that shelter entirely**. Its `libloading` feature ("You'll
need to provide Cisco's prebuilt library") is the compliant shape.
(Verified: <https://github.com/ralfbiedert/openh264-rs>.) A pure-Rust decoder
sidesteps the *copyright* question but not the AVC *patent* question, which is
the same for every implementation.

### 4.4 AV1: `dav1d` and `rav1e`

| Crate | Latest | License | Needs C? |
|---|---|---|---|
| `dav1d` (bindings) | 0.11.1 (2025-11-25) | MIT | **Yes.** "The bindings use system-deps to find dav1d… You may override the `PKG_CONFIG_PATH`." It does **not** build libdav1d from source; needs `pkg-config` + an installed `dav1d>=1.3.0`. (<https://github.com/rust-av/dav1d-rs>) |
| `rav1e` | 0.8.1 (2025-06-16) | **BSD-2-Clause** | **Optional.** Rust 1.85+; NASM ≥2.14.02 needed for x86_64 asm, "enabled by default"; `gas` or clang for aarch64. There is a pure-Rust fallback via disabling the `asm` feature or setting `RAV1E_CPU_TARGET=rust`. (<https://github.com/xiph/rav1e>) |

So **AV1 encode is available in pure Rust today** (`rav1e`, `default-features =
false`), at a speed penalty. **AV1 decode is not** — `dav1d` needs the C
library, and there is no credible pure-Rust AV1 decoder. That asymmetry is the
opposite of the H.264 situation and worth remembering.

### 4.5 Images — pure-Rust support, verified

| Crate | Latest | Publish | License | Pure Rust | Notes |
|---|---|---|---|---|---|
| **`exr`** (exrs) | **1.74.2** | 2026-07-10 | **BSD-3-Clause** | **Yes** — "Read and write OpenEXR files without any unsafe code" | Multi-part ✅, tiled ✅, **DWAA/DWAB ✅** (added 2026-07-08). **Deep data ❌ not supported.** |
| `image` | 0.25.10 | 2026-03-10 | MIT OR Apache-2.0 | Yes for default codecs | PNG, JPEG (via `zune-jpeg`), GIF, TIFF, EXR, WebP, QOI, HDR. **No DPX, no Cineon, no JPEG-2000.** |
| `zune-image` / `zune-jpeg` / `zune-png` | 0.5.0 / 0.5.15 / 0.5.2 | 2026-01→03 | MIT/Apache-2.0/Zlib | Yes | High-performance decoders; `zune-jpeg` is what `image` uses |
| `tiff` | 0.11.3 | 2026-02-10 | MIT | Yes | 16/32-bit float ✅; **LogLuv ❌** |
| `dcv-color-primitives` | 1.0.0 | 2026-01-30 | **MIT-0** | Yes (SIMD intrinsics) | AWS; RGB↔YUV conversion |
| **DPX / Cineon** | — | — | — | — | **No mature pure-Rust crate exists.** `oximedia-image` 0.2.1 (Apache-2.0) claims DPX but is a single young release with ~1,670 downloads — unaudited. |
| `jpeg2k` / `openjpeg-sys` | 0.10.1 / 1.0.12 | 2025 | MIT/Apache-2.0; BSD-2 | **No** — C FFI to OpenJPEG | |
| `openjp2` | 0.6.1 | 2025-07-17 | BSD-2-Clause | Transliterated C port; purity `UNVERIFIED` | |
| `openexr` (the binding) | 0.11.0 | **2021-10-21** | BSD-3 | **No** — binds C++ OpenEXR | Dead end; use `exr` |
| `rawloader` / `rawler` | 0.37.2 / 0.8.0 | 2026-08 | **LGPL-2.1** | Yes | See warning below |
| `imagepipe` | 0.5.1 | 2026-08-15 | **LGPL-3.0-only** | Yes | See warning below |
| `kamadak-exif` | 0.6.1 | — | BSD-2-Clause | Yes | Exif in images |
| `nom-exif` | 3.8.0 | — | MIT (repo LICENSE file) | Yes | Exif **in video containers too** (mov/mp4/mkv) |
| `xmp_toolkit` | 1.12.1 | — | MIT OR Apache-2.0 | **No** — build.rs compiles Adobe's C++ XMP SDK | |

> ### ⚠ The LGPL-in-Rust trap
>
> **`rawloader` and `rawler` are LGPL-2.1; `imagepipe` is LGPL-3.0-only.**
> Rust links crates **statically by default**. An LGPL crate statically linked
> into an Apache-2.0 binary lands squarely in LGPL-2.1 §6(a), which
> `01-LICENSING-SPINE.md` §4 already analysed and rejected: it requires
> shipping our own object files so a user can relink. There is no
> "shared library mechanism" escape hatch for a normal Rust dependency.
>
> **Consequence: camera-RAW stills ingest via `rawloader`/`rawler` is not
> available to CINE-OMNI on the terms everyone assumes.** It must be either a
> separate process (TYPE C), or replaced, or the obligation must be
> deliberately accepted. This is a genuine finding and it should go into the
> risk register. `cargo-deny` must be configured to reject LGPL crates so this
> cannot happen by accident.

Sources: crates.io API for each crate; <https://github.com/johannesvollmer/exrs>.

---

## 5. Hardware acceleration — the API boundary

`02-editorial-media.md` §3 already establishes the vendor matrix, Vulkan Video's
2026 driver status, the zero-copy mechanisms per platform, the NVIDIA
`nv-codec-headers` licensing result, and the abstraction sketch. This section
adds only the API-boundary detail that document deferred.

### 5.1 How it actually surfaces through FFmpeg

The flow is four steps and one callback:

1. `av_hwdevice_ctx_create(&device_ref, AV_HWDEVICE_TYPE_VAAPI, ...)` creates
   an `AVHWDeviceContext` — the GPU/driver connection.
2. `codec_ctx->hw_device_ctx = av_buffer_ref(device_ref)` attaches it.
3. `codec_ctx->get_format` is a **callback**. The decoder offers a list of
   pixel formats; the application returns the hardware one
   (`AV_PIX_FMT_VAAPI`, `AV_PIX_FMT_CUDA`, `AV_PIX_FMT_VIDEOTOOLBOX`,
   `AV_PIX_FMT_D3D11`, `AV_PIX_FMT_VULKAN`) to opt in, or a software one to
   fall back. **Returning the software format here is the fallback path** —
   there is no separate "try hardware, catch error" API, and that shapes how
   `cine-media` must express backend selection.
4. Decoded frames arrive with `frame->format` equal to the hardware format,
   `frame->data[3]` holding the opaque surface handle (for most backends), and
   `frame->hw_frames_ctx` referencing the `AVHWFramesContext` describing the
   pool.

Then either:

* `av_hwframe_transfer_data(sw_frame, hw_frame, 0)` — **downloads** to system
  memory. Correct, universal, and the thing that destroys performance.
* `av_hwframe_map(dst, src, AV_HWFRAME_MAP_READ | AV_HWFRAME_MAP_DIRECT)` —
  **maps** without copying, where the backend supports it. This is the door to
  zero-copy.

The `format` vs `sw_format` distinction on `AVHWFramesContext` (§2.6) is what
tells you what a downloaded frame will look like *before* you download it —
essential for allocating the right wgpu texture.

`AV_HWFRAME_MAP_*` flag semantics are `UNVERIFIED` from the doxygen page fetched
here (the `AVHWFramesContext` page does not document the transfer/map functions);
read `libavutil/hwcontext.h` at integration time.

### 5.2 What zero-copy to a GPU texture actually requires

Four things must all be true, and in practice one of them usually is not:

1. **A shared memory object.** DMA-BUF fd on Linux, `IOSurface` on macOS, a
   D3D11 shared handle on Windows — the mechanisms tabulated in
   `02-editorial-media.md` §3.5.
2. **Matching layout metadata.** A DMA-BUF is useless without its **DRM format
   modifier**; importing with the wrong modifier gives you a plausible-looking
   texture full of tiling garbage. This is the most common silent failure.
3. **Cross-API synchronisation.** The decoder's write must complete before the
   renderer's read. That means an exported semaphore/fence
   (`VK_KHR_external_semaphore_fd`, or a D3D11 keyed mutex), not a CPU
   `queue.wait()`. `wgpu`'s `add_wait_semaphore`/`remove_wait_semaphore` on the
   Vulkan backend exist precisely for this.
4. **The same physical device.** The decoder's GPU and the renderer's GPU must
   be the same adapter. On a laptop with switchable graphics they frequently
   are not, and the import silently fails or falls back.

**Therefore: the CPU-copy path is not a fallback, it is the baseline.** Build
`accept_cpu` first, make it correct, then add `accept_gpu` per platform behind
a capability probe. The trait sketch in `02-editorial-media.md` §3.6 already
has the right shape; the point here is the ordering of work.

### 5.3 The pure-Rust hardware option worth knowing about

**`cros-codecs`** (ChromeOS, BSD-3-Clause, 0.0.6, 829,147 recent downloads) is
a Rust framework for *stateless* hardware decoding over VAAPI and V4L2. It is
Linux-only, pre-1.0, and needs `libva` at runtime — so it is not a substitute
for FFmpeg's cross-platform hwaccel abstraction. It is, however, evidence that
a Rust-native hardware-decode layer is tractable, and it is worth studying
before writing one.

---

## 6. Professional camera media

Findings verified against vendor developer portals, SMPTE pages and FFmpeg
source. The full per-vendor detail is tabulated; the architectural conclusion
follows.

| Format | What it is | FFmpeg native decode | Vendor SDK required | Redistribution / licence shape |
|---|---|---|---|---|
| **ARRIRAW** (`.ari`, also MXF) | Uncompressed Bayer sensor data | **No.** FFmpeg 9.0 added only metadata plumbing (`AV_FRAME_DATA_RAW_COLOR_PARAMS`); decode is "on the roadmap" | **Yes** — ARRI Partner Program | ARRIRAW Converter EULA grants a "royalty-free, revocable, non-exclusive, non-transferable" licence; Partner Program NDA/redistribution terms **UNVERIFIED**. <https://www.arri.com/en/learn-help/learn-help-camera-system/pre-postproduction/file-formats-data-handling/arriraw> |
| **RED R3D** | Proprietary wavelet RAW | **Partial demuxer only** (`libavformat/r3d.c`), reported broken on modern R3Ds | **Yes** — R3D SDK | Royalty-free, but redistribution limited to unmodified binaries; **source, headers, static libs and documentation may not be redistributed**; explicit confidentiality clause §4. <https://www.reddigitalcinema.com/legal/red-r3d-sdk-license-agreement> |
| **Sony XAVC / XAVC-I / XAVC-S** | Constrained H.264/HEVC profiles in MXF or MP4 | **Yes** — the ordinary `h264`/`hevc` decoders; XAVC is a profile, not a codec | No, for decode | Sony runs an open licence program (>70 licensees) by inquiry; fee terms **UNVERIFIED**. <https://www.xavc-info.org/xavc/inquiry.html> |
| **Sony X-OCN** | Compressed camera-negative | **No** | **Yes** — separate Sony program | **UNVERIFIED** |
| **Blackmagic BRAW** | Proprietary RAW | **No** | **Yes** — Blackmagic RAW SDK, free for mac/Win/Linux | Blackmagic states "no hidden licenses or ongoing fees"; EULA fine print **UNVERIFIED**. <https://www.blackmagicdesign.com/developer/partial/braw/sdk-and-software> |
| **Canon Cinema RAW Light** (`.crm`) | Proprietary RAW | **No** | Developer Programme exists; a dedicated Cinema RAW SDK was **not found**. End-user "Cinema RAW Development" software is personal/non-commercial only | **UNVERIFIED** |
| **Panasonic AVC-Intra / P2 MXF** | Intra-only H.264 profile in MXF OP-Atom | **Yes** — standard `h264` decoder | **No** | Governed by the general AVC patent pool (Via LA). <https://www.via-la.com/licensing-programs/avc-h-264/> |
| **Apple ProRes** | Intra DCT intermediate | **Yes** (`prores`) | No for decode | **See below** |
| **Avid DNxHD/DNxHR (VC-3)** | Intra intermediate | **Yes** (`dnxhd`), LGPL-clean encoder *and* decoder | No | SMPTE **ST 2019-1** (codec), **ST 2019-3** (SDTI), **ST 2019-4** (MXF mapping), **RP 2019-2** (conformance). Avid's full bitstream spec is not freely public; MainConcept sells a commercial VC-3 SDK. Patent/branding status separate from the LGPL code licence — **UNVERIFIED** |
| **MXF** | Container | **Yes**, native | No | SMPTE ST 377-1; open implementations **bmx** and **libMXF**, both BSD-3-Clause |

### 6.1 ProRes, decode versus encode — the specific answer

This is the question the brief singled out, and the answer is asymmetric.

**Decode.** FFmpeg's `prores` decoder was added 2011-09-15, reverse-engineered
by Maxim Poliakovski (not a translation of Apple's code). Apple has shipped a
free ProRes *decoder* since 2008. Decoding ProRes is, in practice, free and
universal.

**Encode.** FFmpeg has two independent, uncertified encoders (`prores`/
`prores_aw` and `prores_ks`). Neither is bit-exact against Apple's reference
encoder, and **Apple's own support page explicitly names FFmpeg as an example
of an unauthorized implementation** that "might lead to decoding errors,
performance degradation, incompatibility, and instability." Apple "licenses and
certifies ProRes and ProRes RAW for specific third-party products and
workflows" through its ProRes Program Office; AJA and Atomos appear on the
Authorized Products list. There is no published flat-fee schedule.
(Verified: <https://support.apple.com/en-us/118584>,
<https://www.apple.com/final-cut-pro/docs/Apple_ProRes.pdf>.)

**Consequence for CINE-OMNI, stated carefully.** `02-editorial-media.md` §1.3
correctly noted that ProRes and DNxHD encoders are *native LGPL* in FFmpeg and
therefore a copyright-clean mastering path. That remains true. What this
document adds is that **copyright-clean is not the same as authorized**: a
commercial product that writes files claiming to be ProRes, using an
uncertified encoder, is doing something Apple publicly disclaims. The honest
positions are (a) ship ProRes *decode* freely, (b) treat ProRes *encode* as
requiring Apple certification before it can be an advertised deliverable
format, and (c) make **DNxHR the default mastering intermediate**, since VC-3
is an actual SMPTE standard with an LGPL-clean encoder and no equivalent
certification gate.

### 6.2 The architectural conclusion for camera media

Every genuinely proprietary camera format — ARRIRAW, R3D, X-OCN, BRAW, Canon
Cinema RAW Light — requires a vendor SDK, and every one of those SDKs is a
closed binary with redistribution limits and, in RED's case, an NDA. **None of
them can be linked into an Apache-2.0 open product and shipped.**

The only architecture that works is the one `01-LICENSING-SPINE.md` already
names **TYPE C**: an out-of-process **camera-format plugin host**, where the
user installs the vendor SDK themselves and CINE-OMNI talks to a small adapter
process over a stable IPC protocol. CINE-OMNI ships the protocol and the
adapters' source; it never ships the SDK. That is also how the vendors expect
it to work.

**Everything else — XAVC, AVC-Intra, ProRes decode, DNxHR, MXF — needs no SDK
and is reachable through an ordinary FFmpeg build.** That set covers a very
large fraction of real post-production media, and it should be Phase 2's
ambition. Camera RAW is Phase 3+.

---

## 7. Image sequences — the VFX and finishing reality

**What is actually used.** DPX (SMPTE 268M) remains the standard film-scan /
DI / archival interchange — 10-bit log, one file per frame, patent-unencumbered.
**OpenEXR** dominates working and compositing pipelines: half-float linear,
arbitrary AOV channels, multi-part for layer bundles, tiled for mip/rip maps,
DWAA/DWAB for large plates. **TIFF** appears for textures and matte paintings.
**PNG/JPEG** are proxy and reference only, never finishing-grade.
**JPEG-2000** is mandated for DCP delivery. **Cineon** is a legacy-archive
concern. **Radiance HDR** is an IBL/lighting format.

**Pure-Rust reality**, from the table in §4.5:

| Format | Pure-Rust status |
|---|---|
| **OpenEXR** | **Strong.** `exr` 1.74.2, BSD-3, no unsafe, multi-part + tiled + DWAA/DWAB. **Deep data is the one real gap.** |
| **PNG, JPEG, TIFF, HDR** | **Strong.** `image` 0.25.10 + `zune-*`, all pure Rust, permissive. |
| **TIFF LogLuv** | **Gap.** `tiff` 0.11.3 does not support it. |
| **DPX / Cineon** | **Gap — but a cheap one.** No dependable crate exists. DPX is a fixed 2048-byte header plus packed samples; a correct reader is a few hundred lines against SMPTE 268M, and writing it in-house is *preferable* to depending on `oximedia-image` (single young release, ~1,670 downloads, unaudited). |
| **JPEG-2000** | **Gap, and not cheap.** Every path either FFI-binds C OpenJPEG or is an unaudited transliteration. Scope DCP out, or isolate it behind an optional feature. |
| **Deep EXR** | **Gap.** Requires an upstream contribution to `exr` or a hand-rolled deep-block parser. |

**This is the most favourable area in the whole report.** Professional image
sequence I/O — the thing a finishing application actually reads and writes — is
substantially solvable in pure Rust today, with EXR (the important one) already
production-grade and DPX a tractable in-house build.

---

## 8. Variable frame rate, timecode, and metadata

### 8.1 Why "frame rate" is not a container-level constant

* **MP4/MOV (ISOBMFF)** — timing is *table-driven*. `stts` gives run-length
  `(sample_count, sample_delta)` pairs of **decode** durations; runs with
  differing deltas *are* VFR. `ctts` adds a per-sample offset converting decode
  time to **composition** time (needed whenever B-frames reorder). `elst`
  remaps the composition timeline again — trimming, offsetting, looping. Three
  stacked tables; **no "fps" field anywhere**. (Our test asset: `stts` =
  `{512: 240}`, so it happens to be exactly 24.000 fps CFR — but that is a
  measured property of the tables, not a declared property of the file.)
* **Matroska** — every Block carries its own explicit timestamp scaled by
  `TimestampScale`. `DefaultDuration` is an advisory hint only. (RFC 9559.)
* **MPEG-TS** — the PES layer carries only 33-bit 90 kHz PTS/DTS. Any nominal
  rate comes from the H.264/HEVC VUI `timing_info`, which is present only if
  `timing_info_present_flag` is set — optional, encoder-supplied, frequently
  absent or wrong.

**What FFmpeg reports**, per `AVStream`'s doxygen
(<https://ffmpeg.org/doxygen/trunk/structAVStream.html>):

| Field | Documented meaning |
|---|---|
| `time_base` | "the fundamental unit of time (in seconds) in terms of which frame timestamps are represented" |
| `avg_frame_rate` | "Average framerate" |
| `r_frame_rate` | "Real base framerate… the lowest framerate with which all timestamps can be represented accurately (it is the least common multiple of all framerates in the stream)… **this value is just a guess**" |
| `nb_frames` | "number of frames in this stream if known or 0" — note this is on `AVStream`, not `AVFormatContext` |

**Neither `avg_frame_rate` nor `r_frame_rate` may be used to compute a frame
index.** One is an average; the other is self-documented as a guess.

**What NLEs do.** DaVinci Resolve *conforms* mixed-rate clips to the timeline
rate, with a per-clip Retime Process (nearest / frame-blend / optical-flow);
audio is left alone and can desync. Premiere exposes `Clip > Modify > Interpret
Footage` to reassign a clip's rate, and Adobe's own FAQ recommends transcoding
true VFR sources to CFR *before* editing.
(<https://blog.frame.io/2019/10/07/mixed-rates-resolve-part-2/>,
<https://community.adobe.com/t5/premiere-pro-discussions/faq-how-to-work-with-variable-frame-rate-vfr-media-in-premiere-pro/td-p/10348229>)

**The engineering rule for CINE-OMNI:**

> A media source's timing is a **monotonic presentation-time table**, built from
> `stts` ∘ `ctts` ∘ `elst` (or Matroska block timestamps), and a wall-clock time
> resolves to a frame index by **binary search over that table**. Constant frame
> rate is a *compression* of that table, detected and exploited, never assumed.
> `re_mp4` already exposes exactly this data per sample
> (`decode_timestamp`, `composition_timestamp`, `duration`, `is_sync`), which is
> why §1.3's measurements were possible in pure Rust.

### 8.2 Timecode

* **MP4/QuickTime `tmcd` track** — a timecode track whose samples encode a
  32-bit frame count representing `HH:MM:SS:FF`, optionally drop-frame. MP4's
  sample description is QuickTime's minus the source-reference extension, and
  MP4 uses `nmhd` where QuickTime uses `gmhd`+`tcmi`.
  (<https://developer.apple.com/library/archive/technotes/tn2174/_index.html>,
  <https://developer.apple.com/library/archive/technotes/tn2310/_index.html>)
  FFmpeg exposes the decoded start timecode as the stream metadata tag
  `timecode` (visible in `ffprobe -show_streams`). Whether a dedicated
  `AV_PKT_DATA_*` side-data enumerator exists for it is **UNVERIFIED**.
* **MXF** — the Material Package carries a Timecode Component; first-frame
  timecode plus the SMPTE **ST 12-1** counting rules, referenced from **ST
  377-1**. (<https://tech.ebu.ch/docs/r/r122.pdf>,
  <https://pub.smpte.org/doc/st377-1/20090504-pub/st0377-1-2009.pdf>)
* **Drop-frame, exactly** — DF skips the *labels* `:00` and `:01` at the start
  of every minute **except every tenth minute**; no video frames are dropped.
  17,982 labelled frames per 10 real minutes at 29.97. It applies to **29.97
  and 59.94** because those exist to absorb NTSC colour's 1000/1001 pulldown;
  **23.976 and 24 have no drop-frame** because there is no subcarrier-induced
  drift to compensate. **SMPTE ST 12-3 (2016)** extends timecode to 23.98, 24,
  25, 29.97, 30, 47.95, 48, 50, 59.94, 60, 72, 96, 100 and 120 fps. The precise
  drop-frame rule at 47.952/119.88 is **UNVERIFIED** — secondary sources
  disagree and the primary ST 12-3 text was not read.
  (<https://en.wikipedia.org/wiki/SMPTE_timecode>,
  <https://www.drastic.tv/images/documentation/st0012-3-2016.pdf>)
* **LTC** is audio-encoded and readable only while moving; **VITC** rides in the
  vertical blanking interval and is readable when paused.
* **Rust crates**: `vtc` 0.1.14 (MIT), `timecode` 0.7.5 (MIT), `video-timecode`
  0.6.1 (MIT). All small. Given `cine-timeline` already implements OTIO's
  `RationalTime` natively, **the drop-frame rule should be implemented in-house
  inside `cine-timeline`, not taken as a dependency** — it is ~30 lines and it
  belongs next to the time model that already exists.
* **OTIO** exposes `RationalTime.to_timecode(rate)` and
  `from_timecode(str, rate)`; CINE-OMNI's schema-compatible implementation
  should match those semantics exactly.
  (<https://opentimelineio.readthedocs.io/en/latest/api/python/opentimelineio.opentime.html>)

### 8.3 Metadata that must survive ingest

| Standard | What it carries | Reference |
|---|---|---|
| **SMPTE RDD 18** (2012/2021) | **The convergence point.** Frame-granular acquisition metadata — camera and lens parameters, KLV-coded per ST 336 — and it explicitly carries **ASC CDL v1.2** (the ten Slope/Offset/Power + Saturation values) | <https://www.drastic.tv/images/documentation/rdd18-2021.pdf> |
| **SMPTE ST 377-1** + **DMS-1** (ST 380M) | MXF descriptive metadata: Production / Scene / Clip frameworks; term dictionary RP 210 | <https://pub.smpte.org/doc/st377-1/20090504-pub/st0377-1-2009.pdf> |
| **ASC CDL** v1.2 | Primary grade exchange, `out = (in × s + o)^p`; embeds in ALE, FLEx, CMX EDL, `.cc`/`.ccc` | <https://en.wikipedia.org/wiki/ASC_CDL> |
| **ASC MHL** | Chain-of-custody checksum manifest (xxHash/XXH3/MD5/SHA1/SHA256/C4) for file-based transfer | <https://ascmhl.readthedocs.io/en/latest/README.html> |
| **AMF — ACES Metadata File** | Academy Specification **S-2019-001**; XML sidecar recreating the ACES viewing pipeline (Input/Look/Output transforms); replaced ACESclip in ACES 1.2 | <https://docs.acescentral.com/amf/specification/> |
| **SMPTE ST 2065-4** | The ACES *image container* (OpenEXR-based half-float) — **not** camera metadata | <https://pub.smpte.org/pub/st2065-4/st2065-4-2023.pdf> |
| **EBU Core (Tech 3293)** | Broadcast descriptive/technical metadata, XML Schema + RDF/OWL | <https://tech.ebu.ch/docs/tech/tech3293.pdf> |
| ARRI / RED metadata | Vendor-proprietary schemas (ARRI: per-frame F-stop/focal/focus over SDI and in QuickTime; RED: per-clip and per-frame FIZ, extracted via `REDline --printMeta`). **Not public standards.** | <https://www.arri.com/resource/blob/327826/a17ae3b8bf7e8d487d6d01648bdf432e/arri-cameras-sdi-metadata-white-paper-data.pdf> |

Two corrections to common assumptions, both worth recording: **ST 2065 is ACES
colour/container, not camera metadata**, and there is **no "ST 2073" camera
metadata standard** — that number does not check out (`UNVERIFIED / likely does
not exist`). The real industry convergence is **RDD 18 plus the SMPTE OSVP
effort** mapping ARRI/RED/Cooke/Zeiss outputs into a common schema.

**Dynamic HDR.** HDR10+ is SMPTE **ST 2094-40**, carried as HEVC SEI prefix NALs
(type 39, payload type 4, ITU-T T.35 country 0xB5 / provider 0x003C), and
HDR10+ Technologies states **no per-title or per-device royalty** for baseline
HDR10+ (<https://hdr10plus.org/>). Dolby Vision's RPU is proprietary; Profile 5
uses Dolby's IPT-PQ C2 space, and full mastering/trim tooling requires a paid
Dolby licence (<https://professional.dolby.com/content-creation/dolby-vision-for-content-creators/>).
Notably, RPU *parsing* is open: the `dolby_vision` crate behind `dovi_tool` is
**MIT OR Apache-2.0** and supports profiles 4/5/7/8 — but parsing the metadata
is not a licence to author or certify Dolby Vision masters.

**The UCG consequence.** All of the above is *provenance*, and CINE-OMNI already
has a graph whose entire purpose is identity, dependency and provenance. Camera,
lens, reel/roll, scene/take/slate, CDL and AMF references should become
first-class UCG attributes at ingest, sourced from RDD 18 / DMS-1 / sidecars,
not stashed in an opaque blob. That is a `ucg-schema` design task that can start
before any decoder exists.

---

## 9. Subtitles and captions — what belongs in Phase 2

**Almost nothing. Say so plainly.**

| Format | Standardised by |
|---|---|
| SRT | Nothing — no formal spec |
| WebVTT | W3C (<https://www.w3.org/TR/webvtt1/>) |
| SCC | De facto; carries CEA-608 with SMPTE timecodes |
| MCC | De facto; carries CEA-708 |
| STL | EBU **Tech 3264** — binary, 1024-byte GSI header + 128-byte TTI blocks |
| IMSC / TTML | W3C; EBU-TT is a strict TTML subset |
| CEA-608 / CTA-708 | CTA — **embedded in the video elementary stream** |

Rust crates exist and are small: `subparse` 0.7.0 (**MPL-2.0**), `srtlib` 0.2.0
(MIT OR Apache-2.0), `aspasia` 0.2.1 (0BSD).

**Judgement.** Captions are a thin, well-bounded serialisation concern that can
be added at any time without touching the timing model or the graph schema. VFR
timing, timecode and camera/colour metadata are the opposite — they shape the
entity and attribute schema *now*, and getting them wrong is expensive to undo.
The one caveat is CEA-608/708, which is embedded *inside* the video bitstream:
if the decoder path is built without a hook for extracting closed-caption side
data, retrofitting it later means touching the decoder. **Leave a hook; build
nothing.**

---

## 10. Recommendations

### 10.1 The long-term architecture, and the exact API boundary

Four layers, with the boundary drawn deliberately low.

```
                 ┌──────────────────────────────────────────────┐
   UCG           │ ucg-core / ucg-schema                        │
                 │  identity · provenance · RationalTime        │
                 └──────────────────────────────────────────────┘
                                    ▲   media entities, timing tables,
                                    │   camera/lens/CDL/AMF attributes
                 ┌──────────────────────────────────────────────┐
   cine-media    │ THE API BOUNDARY — pure Rust traits, no FFI  │
                 │  MediaSource · Demuxer · VideoDecoder ·      │
                 │  AudioDecoder · FrameSink · HwBackend        │
                 └──────────────────────────────────────────────┘
      ┌──────────────┬───────────────┬──────────────┬────────────────┐
   cine-media-    cine-media-     cine-media-    cine-media-sdk-host
   ffmpeg         pure            hw             (out-of-process,
   (rsmpeg,       (symphonia,     (VAAPI/NVDEC/   vendor SDKs:
    dyn LGPL)      re_mp4,         VT/D3D11/       R3D, BRAW,
                   rusty_h264,     Vulkan)         ARRIRAW, X-OCN)
                   exr, image)
                                    │ planar frames + side data
                 ┌──────────────────────────────────────────────┐
   cine-gpu +    │ YUV→RGB, chroma siting, transfer functions,  │
   cine-color    │ ACES — as wgpu compute. NOT libswscale.      │
                 └──────────────────────────────────────────────┘
```

**The exact traits CINE-OMNI should code against**, shaped by §2.5 and §2.6
so that FFmpeg is a *natural* implementation rather than a fought one:

```rust
/// A demuxed, still-compressed access unit.
pub struct Packet {
    pub stream: StreamId,
    pub dts: Option<RationalTime>,   // decode order
    pub pts: Option<RationalTime>,   // presentation order — may be absent
    pub duration: Option<RationalTime>,
    pub is_keyframe: bool,
    pub data: Bytes,                 // refcounted, never copied
}

/// A decoded picture. Owns its planes OR references a GPU surface.
pub struct VideoFrame {
    pub pts: RationalTime,           // authoritative; never derived from f64
    pub format: PixelFormat,         // sw layout, or an opaque hw handle
    pub planes: FrameStorage,        // Cpu(Arc<[u8]>, strides) | Gpu(HwSurface)
    pub color: ColorDescription,     // primaries, transfer, matrix, range, siting
    pub side_data: SideData,         // ST 2086 / MaxCLL / ST 2094-40 / CC / timecode
}

/// One-in, many-out — deliberately mirrors avcodec_send_packet/receive_frame,
/// because every mature decoder in existence has this shape.
pub trait VideoDecoder: Send {
    fn send_packet(&mut self, pkt: Option<&Packet>) -> Result<(), DecodeError>;
    fn receive_frame(&mut self) -> Result<Option<VideoFrame>, DecodeError>;
    fn flush(&mut self);             // MUST be called after every seek
}

/// Timing is a table, not a rate. CFR is a compressed table (§8.1).
pub trait Demuxer: Send {
    fn streams(&self) -> &[StreamInfo];
    fn timing(&self, s: StreamId) -> &TimingTable;   // pts, duration, keyframe
    fn seek_to_keyframe_at_or_before(&mut self, s: StreamId, t: RationalTime) -> Result<()>;
    fn next_packet(&mut self) -> Result<Option<Packet>>;
}
```

Frame-accurate seek is then a *free function* over those traits, implementing
§2.4's five steps once, identically for every backend — which is the real payoff
of drawing the boundary here.

**Non-negotiable invariants:**

1. **No `f64` in any timing path.** `RationalTime` everywhere; `to_seconds()`
   exists for the UI and is banned elsewhere.
2. **`flush()` after every seek.** Enforced by making seek consume the decoder
   or by a debug assertion.
3. **Side data is never dropped.** A frame that loses its colour description is
   a frame `cine-color` has to guess about.
4. **The hardware backend is a rendering hint, never a graph attribute** — the
   rule already established in `02-editorial-media.md` §3.6, restated because
   the trait above is where it would be violated.

### 10.2 The exact FFmpeg build, and what CINE-OMNI may honestly claim

**The build:**

```
./configure \
  --prefix=<vendor>            \
  --enable-shared              \   # LGPL-2.1 §6(b): user-replaceable
  --disable-static             \
  --disable-gpl                \   # default, stated explicitly for the audit trail
  --disable-nonfree            \
  --disable-version3           \   # stay at LGPL-2.1, avoid LGPLv3 components
  --disable-programs           \   # no ffmpeg/ffplay/ffprobe binaries
  --disable-doc                \
  --disable-libx264 --disable-libx265 --disable-libxvid \
  --disable-avdevice --disable-postproc \
  --enable-libdav1d            \   # BSD-2, AV1 decode, no --enable-gpl needed
  --enable-vaapi --enable-nvdec --enable-nvenc \
  --enable-videotoolbox --enable-d3d11va --enable-dxva2 --enable-vulkan
```

Components linked: **`libavformat`, `libavcodec`, `libavutil`,
`libswresample`**. **`libswscale` is linked but unused on the interactive
path** (§2.7) — colour conversion is `cine-gpu`. `libavfilter` is not linked at
all, which also removes the ~33 GPL filters from the picture entirely.
`libavdevice` is not linked.

**What CINE-OMNI may then honestly claim:**

* ✅ "Decodes H.264, HEVC, ProRes, DNxHD/DNxHR, VP9, AV1, JPEG-2000 and PCM/AAC
  audio from MOV, MP4, MXF, MKV and image sequences, via a dynamically linked
  LGPL-2.1 FFmpeg build."
* ✅ "Encodes ProRes, DNxHD/DNxHR, FFV1, JPEG-2000 and PCM/BWF natively."
* ✅ "Hardware-accelerated decode via VAAPI, NVDEC, VideoToolbox, D3D11VA and
  Vulkan Video where the platform provides it."
* ❌ **Not** "software H.264/HEVC encode" — that needs x264/x265 (GPL) or a
  commercial licence, or hardware encoders, or Kvazaar/OpenH264.
* ❌ **Not** "ProRes encode" as a certified deliverable until Apple's ProRes
  Program Office says so (§6.1). Ship it as "ProRes (uncertified)" or make
  DNxHR the default master.
* ❌ **Not** "supports RED / ARRIRAW / BRAW / X-OCN / Canon RAW" — those are
  out-of-process, user-installed vendor SDKs (§6.2).
* ⚠️ **Patent licensing is a separate axis from all of the above** and remains
  a tracked, unresolved risk (`01-LICENSING-SPINE.md` §5).

Obligations that must ship with it: prominent notice of FFmpeg use, the LGPL-2.1
text, FFmpeg's copyright notices, a documented relink path, and a `cargo-deny`
policy that fails the build on any static-FFmpeg feature, any GPL/LGPL crate,
and (as a deliberate decision) WTFPL crates.

### 10.3 The buildable-today pure-Rust vertical slice

Given §1, this is not a toy. Everything below was compiled and run on this
machine today, against the real 5.78 MB asset.

**Crate: `cine-media` (traits, §10.1) + `cine-media-pure` (this backend).**

| Capability | Crate | Version | License | Status here |
|---|---|---|---|---|
| ISO-MP4 demux, `stts`/`ctts`/`stss` timing table | `re_mp4` | 0.5.1 | MIT | ✅ **measured** — 240 samples, 2 keyframes, 184 reordered |
| H.264 High-profile decode | `rusty_h264` | 0.16.0 | BSD-2 | ✅ **measured** — 240/240 frames, bit-exact IDR vs OpenH264 |
| AAC / MP3 / FLAC / ALAC / PCM / Vorbis decode; MP4/MKV/OGG/WAV/AIFF/CAF demux | `symphonia` | 0.6.1 | MPL-2.0 | ✅ **measured** (0.5.5 build) — 481,280 AAC frames |
| OpenEXR read/write (multi-part, tiled, DWAA/DWAB) | `exr` | 1.74.2 | BSD-3 | ✅ **builds here** (§1.6) |
| PNG / JPEG / TIFF / HDR | `image` + `zune-*` | 0.25.10 / 0.5.0 | MIT/Apache-2.0 | ✅ **builds here** (§1.6) |
| Exif / container metadata | `kamadak-exif` 0.6.1, `nom-exif` 3.8.0 | — | BSD-2 / MIT | ✅ **builds here** (§1.6) |
| Colour-model conversion (CPU fallback) | `dcv-color-primitives` | 1.0.0 | MIT-0 | ✅ **builds here** (§1.6) |
| DPX read/write | **in-house**, ~300 LOC vs SMPTE 268M | — | Apache-2.0 | ⬜ to write |
| YUV→RGB, chroma siting, transfer functions | **`cine-gpu` wgpu compute** | — | Apache-2.0 | ⬜ to write |
| Timecode + drop-frame | **in-house in `cine-timeline`** | — | Apache-2.0 | ⬜ to write |
| AV1 encode (proxies) | `rav1e` `default-features=false` | 0.8.1 | BSD-2 | ✅ **builds here** (§1.6), optional |

**The slice to build, in order:**

1. `cine-media` traits (§10.1) — pure interface, no backend.
2. `cine-media-pure`: `re_mp4` demux → `TimingTable` → UCG media entity with
   real duration, rate, keyframe map and per-sample presentation times.
3. `symphonia` audio decode → `cine-audio` (which already exists).
4. `rusty_h264` video decode behind `VideoDecoder`, feeding CPU planar frames.
5. `cine-gpu` compute shader for YUV→RGB + colour, producing a `wgpu::Texture`.
6. The frame-accurate-seek free function (§2.4) implemented once over the
   traits, and a test asserting that seeking to frame *N* and decoding forward
   from frame 0 to *N* produce **byte-identical** frames — which is the real
   correctness property, and which the 189-frame GOP in the test asset makes a
   genuinely demanding test.
7. `exr` + `image` + in-house DPX for image-sequence sources.

That is a **complete, end-to-end, demonstrable media path from an MP4 on disk
to a GPU texture and audio samples, with frame-accurate seeking, in pure Rust,
with no C toolchain and no FFmpeg** — and every codec-shaped box in it has
already been proven to work today.

### 10.4 What must explicitly NOT be implemented initially

| Do not build | Why |
|---|---|
| **A second media abstraction beside the UCG** | Media identity belongs in the graph. `cine-media` returns frames; it never owns identity. |
| **`libswscale` on the interactive path** | CPU conversion of every frame, approximate colour handling, and it duplicates `cine-color` (§2.7). |
| **Any static FFmpeg link** | LGPL-2.1 §6(a) relinking-kit obligation. `cargo-deny` must fail the build. |
| **Any LGPL Rust crate** (`rawloader`, `rawler`, `imagepipe`) | Rust links statically; there is no §6(b) escape (§4.5 warning). Camera-RAW stills must wait for the TYPE C process host. |
| **x264 / x265 / any `--enable-gpl` component** | Incompatible with Apache-2.0 (`01-LICENSING-SPINE.md` §2). |
| **Vendor camera SDKs in-process** | ARRIRAW, R3D, BRAW, X-OCN, Canon RAW all carry redistribution limits; RED's carries an NDA (§6.2). |
| **ProRes encode as an advertised deliverable** | Apple certification gate (§6.1). Make DNxHR the default master. |
| **Subtitles and captions** | Thin, deferrable, schema-neutral (§9). Leave a CEA-608/708 side-data hook in the decoder; build nothing else. |
| **Deep EXR, JPEG-2000/DCP, TIFF LogLuv** | Real gaps with no pure-Rust path; scope out or make optional features (§7). |
| **A hardware-decode abstraction before the CPU path is correct** | Zero-copy needs four things to align and one usually doesn't (§5.2). CPU copy is the baseline, not the fallback. |
| **`rusty_h264` as the *shipping* decoder** | Ten weeks old, 17,757 downloads, no independent conformance run here. It is a vertical-slice enabler behind a trait, and the trait is the deliverable. |

---

## 11. What remains UNVERIFIED

Named, per directive §60, rather than filled in plausibly.

* The `AVRounding` enumerator list and `av_rescale_q_rnd` semantics — read from
  `libavutil/mathematics.h` before relying on `AV_ROUND_PASS_MINMAX`.
* `AV_HWFRAME_MAP_*` flag semantics and the full `AVHWDeviceType` list — the
  fetched doxygen page did not document the transfer/map functions.
* The exact current `AV_FRAME_DATA_*` side-data enumerators for HDR.
* Whether a dedicated `AV_PKT_DATA_*` enumerator carries MP4 `tmcd` timecode.
* Drop-frame behaviour at 47.952 / 119.88 under SMPTE ST 12-3 — secondary
  sources conflict; the primary text was not read.
* The current edition number of SMPTE ST 2019-1 after its 2022 revision.
* ARRI Partner Program redistribution/NDA terms; Sony XAVC and X-OCN fee
  schedules; Blackmagic RAW SDK EULA fine print; Canon Cinema RAW Light SDK
  terms — all behind registration walls or unextractable PDFs.
* Via LA's 2026 AVC fee restructuring as it applies to *editing software*
  (rather than OTT streaming) doing AVC-Intra decode/encode.
* `rusty_h264`'s bit-exactness claim over openh264's full conformance corpus —
  this document verified **one IDR of one asset**, not the corpus.
* `rust_h265` — never built or run here. Treat as unproven.
* `rubato` (audio resampling) license and maturity — not checked.
* zimg's exact license text (the repository page reports WTFPL; the LICENSE
  file itself was not read).
