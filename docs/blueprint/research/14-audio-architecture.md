# CINE-OMNI Industry Blueprint — 14: Professional Audio Architecture

**Status: complete for the questions it was asked.** Every licence below had its
primary source fetched; every crate was checked against the crates.io API; every
performance number was measured on this machine and the machine is named. What
remains open is in the register at §13 and is open by declaration, not by
omission.

**Scope.** Everything after sound *arrives*. `research/10-color-and-sync.md`
§B settles time, timecode, A/V sync, dual-system sound and the media/audio
boundary; `PHASE-2-MEDIA-ENGINE.md` settles decode, planar `f32` delivery and
media identity. This document does not restate them. It answers six questions:

1. Where does the boundary run between media audio, sync, editorial, dialogue
   edit, sound design, music, mix, master and delivery?
2. What plugin formats may an Apache-2.0 product host, given VST3's licence?
3. How must audio time relate to `cine-media`'s exact-rational `Timestamp`?
4. What must actually be measured for EBU R128 / ITU-R BS.1770 delivery, and is
   there a verified Rust path?
5. What are the hard real-time-thread rules, and how do they meet the UCG
   transaction model?
6. What is built natively, wrapped, bridged, or deferred (TYPE A–H per
   `01-LICENSING-SPINE.md`)?

**Evidence rules** (per `docs/blueprint/README.md`): no licence claimed from
memory; no performance number that was not measured on this machine; every
unknown marked `UNVERIFIED` inline and in the register at the end.

---

## 0. Findings summary

**F1. `cine-audio` is a good offline mixdown library and is not a DAW.** It has
no device I/O, no clock, no transport, no streaming render, no plugin host, no
recording, no MIDI, and no interchange. Its `render()` takes a whole time range
and returns a whole `AudioBuffer`. Nothing in the crate can be driven from an
audio callback. That is not a criticism of what is there — the DSP is real and
the loudness maths is *exactly right* (F3) — it is a statement that the missing
half is the half that makes it a post-production tool.

**F2. Measured, on this machine (Intel i3-10110U, 2 cores / 4 threads, 15 GB):**

| Render | Wall time | vs realtime |
|---|---|---|
| 8 tracks → stereo | 1.276 s / 30 s | 23.5× |
| 48 tracks → stereo | 5.202 s / 30 s | 5.8× |
| 48 tracks → 5.1 | 9.912 s / 30 s | 3.0× |
| 48 tracks → 7.1.4 | 21.308 s / 30 s | **1.4×** |
| **128 tracks → 7.1.4** | 33.781 s / 10 s | **0.30× — 3.4× slower than realtime** |

Each track carries a high-pass biquad and a compressor, panned by VBAP. No
plugins, no reverb, no automation. **A 128-track 7.1.4 session — a modest
immersive mix — cannot be rendered as fast as it plays**, let alone played.

**And the deadline number, which is the one that settles it:** a single
512-frame block (**10.667 ms** of budget at 48 kHz) of a 48-track stereo session
took **12.228 ms — 114.6 % of the budget.** That is a guaranteed dropout on
every block, on a *stereo* session of moderate size, before any of the
real-time-safety violations of §4.5 are even considered. This is a low-power
laptop and the numbers scale with cores, but the shape is the finding: the
current design has no headroom for what it would have to become, and the path
to a real-time engine is not optimisation of `render()` — it is §2.2.

**F3. The BS.1770 integrated-loudness implementation is correct.** Cross-checked
against the `ebur128` crate (v0.1.10, the Rust port of `libebur128`) on five
signals including 5.1: **agreement to 0.000 LU in every case.** This is the
strongest thing in the crate and it should be kept, not replaced.

**F4. True peak is *not* right, and it errs in the dangerous direction.** Same
cross-check: on programme-like material `cine-audio` reads **+0.30 to +0.50 dB
higher** than `ebur128`. On a 1 kHz sine the two agree to 0.000 dB, which is
why the existing unit test passes. `true_peak_dbtp()` uses the crate's generic
16-tap Hann-windowed sinc resampler at 4×, not the BS.1770-4 Annex 2 filter.
A −1.0 dBTP delivery ceiling policed by a meter that is 0.5 dB pessimistic will
cost real headroom; the same error in the other direction on other material
would ship an over.

**F5. Clip placement rounds start and duration independently.** In
`mix::render_track`: `start = round(clip.start_s() * sr)` and
`len = round(clip.duration_s() * sr)`. Two butt-spliced clips can therefore
overlap or gap by one sample at the join. This is `research/10` §B.1.5 rule 4
("durations are never converted by converting both endpoints") violated in the
renderer. On a dialogue edit that is an audible click.

**F6. The `cine-media` → `cine-audio` handoff specified in `research/10` §B.8
does not exist.** `cine-media::frame::to_audio_buffer()` returns `Vec<f32>` —
not a `cine_audio::AudioBuffer`, and it drops the `Timestamp`. `cine-media`
does not depend on `cine-audio` at all. The rule "no second buffer type" has
already been broken: `AudioFrame` (planar, `Timestamp`) and `AudioBuffer`
(interleaved, bare `u32` sample rate) are two audio types with no typed bridge.

**F7. `cine-audio` has no exact time.** `Clip::range` is `ucg_core::TimeRange`
over `RationalTime { value: f64, rate: f64 }` — still float, `research/10` R1
not yet done — and everything else in the crate (automation lanes,
`source_offset_s`, `render` ranges) is `f64` seconds. `cine-media`'s exact
`Timestamp { ticks: i64, base: TimeBase }` is not used anywhere in `cine-audio`.

**F8. On this machine, `cpal` cannot build.** Verified: `cargo build` of
`cpal 0.15.3` fails in `alsa-sys 0.3.1`'s build script — `pkg-config` is not
installed. There is also no unversioned `libasound.so` and no ALSA headers, so
even with `pkg-config` the link would fail. PipeWire 1.0.5 *is* running
(§7). This is a build-environment fact about this machine, not about `cpal`.

**F9. A pure-Rust CLAP host builds here today.** Verified by compiling:
`clack-host 0.1.1`, `clack-extensions 0.1.1`, `clap-sys 0.5.0`, alongside
`ebur128 0.1.10`, `rubato 0.16.2`, `rtrb 0.3.5`, `basedrop 0.1.3`,
`triple_buffer 8.1.1`, `rosc 0.11.4`, `realfft 3.5.0`. Note the crates.io
reality: **`clack-host`'s latest published version is 0.1.1** (0.1.0 and 0.1.1
are the only ones on crates.io), which is younger than the repository suggests.
No CLAP, LV2, VST3 or LADSPA plugin is installed on this machine, so *hosting*
was not exercised — only that the host bindings compile.

**F10. The blueprint's VST3 finding is out of date, and the trap has moved to
ASIO.** Verified from primary sources today: the **VST 3 SDK is MIT** as of
VST 3.8, announced 29 October 2025 — `LICENSE.txt` in `steinbergmedia/vst3sdk`
is verbatim MIT, and the README states *"Licensing under GPLv3 and the
Steinberg proprietary license is no longer available."* In the same round
Steinberg moved the **ASIO SDK to dual GPLv3 / proprietary**. So VST3 is now
hostable by an Apache-2.0 product and **ASIO is now the licence that cannot be
linked**. `01-LICENSING-SPINE.md` §5 and `02-TARGET-ARCHITECTURE.md` §4 both
state the old position and must be corrected. Detail in §6.

**F11. The plugin order is CLAP → VST3 → LV2, and the reason CLAP leads is no
longer licence, it is buildability.** All three are now permissively licensed
and all three pass `deny.toml` unchanged. CLAP leads because a pure-Rust host
(`clack-host`) compiles on this machine with no C toolchain (F9), and because
its threading contract is written into the header — which is exactly the
discipline §4 requires anyway. **Do not read this as "hosting is nearly done":
the format is the smallest part of a plugin host** (§6.5).

**F12. `cine-audio` cannot meet an audio deadline, by a measured margin.** A
single 512-frame block of a 48-track *stereo* session takes 12.228 ms against a
10.667 ms budget — **114.6 % of the deadline** — before any of its real-time
safety violations are considered. The conclusion is not "optimise `render()`";
it is that a separate real-time engine crate is required (§2.2), because
`render()` allocates, resamples inline and returns owned buffers, all of which
are correct for an offline bounce and forbidden in a callback.

**F13. Nobody has a permissively-licensed Rust DAW engine to borrow.**
Meadowlark — the most-cited attempt — **relicensed to AGPL-3.0 in April 2025**
and had 23 of its 25 repositories archived on 2025-09-22. `dropseed` is archived
AGPL with a `0.0.0` placeholder on crates.io. Its `audio-graph` is MIT but four
years stale and *not* on crates.io — and the `audio_graph` crate that **is** on
crates.io is a different, 41-download package whose repository field is a
literal placeholder. The engine is TYPE A because there is no alternative, not
because building it is preferred.

**F14. A crate's declared licence says nothing about what it links, and this
bites repeatedly in audio.** The `jack` crate is MIT and links **LGPL-2.1**
libjack. `libltc-rs` is MIT and links **LGPL-3.0** libltc. `nih-plug` is ISC and
its own README warns that its VST3 export pulls in **GPL-3.0** `vst3-sys`,
forcing GPLv3 on the whole plugin. `cargo-deny` sees none of this. The
third-party inventory required by `01-LICENSING-SPINE.md` §6 must record linked
native libraries, not only crates.

**F15. Two things thought to be blocked are not.** **Time-stretch** has a fully
permissive option — Signalsmith Stretch is MIT at the C++ core *and* in its Rust
wrapper — so the GPL Rubber Band trap can simply be walked around (§8.4). And
**object-based delivery is fully open**: `ebu/libadm` and `ebu/libbw64` are
**Apache-2.0** and the EBU ADM Renderer is **BSD-3-Clause-Clear**, so ADM/BW64
is a deliverable CINE-OMNI may implement completely, which Dolby Atmos is not
(§8.2). No Rust implementation of ADM exists, so it is TYPE A work — but it is
unobstructed TYPE A work.

---

## 1. What `crates/cine-audio` actually is — a frank assessment

Read in full: 6,450 lines across 13 files.

### 1.1 What is there, and is good

| Module | Lines | What it is |
|---|---|---|
| `buffer.rs` | 669 | `AudioBuffer` — interleaved `f32`, `sample_rate: u32`, `channels`. Slicing, mixing, gain, RMS/peak, **BS.1770-4 integrated LUFS**, true peak, 16-tap sinc resample |
| `dsp.rs` | 1,160 | RBJ biquads, STFT/ISTFT (WOLA, exact reconstruction), envelope follower, compressor with soft knee, gate with hold, look-ahead brick-wall limiter that **reports its own latency**, delay, fades and crossfades |
| `pan.rs` | 1,049 | Speaker layouts to 7.1.4, VBAP (Pulkki) with 2D pairs and 3D triplets, object panning with distance roll-off and spread, ITU downmix matrices |
| `session.rs` | 776 | `Session`/`Track`/`Clip`/`Bus`/`Stem`/`Send`/`Insert`, automation lanes with linear or step interpolation |
| `mix.rs` | 757 | Block renderer (`BLOCK_SIZE = 512`), tracks in parallel via rayon, per-block automation with intra-block interpolation, bus topological order with cycle detection, stem rendering |
| `isolate.rs` | 518 | Spectral-gating dialogue isolation with decision-directed Wiener gains and a spectral-flux VAD |
| `ucg.rs` | 416 | Script-derived dialogue placeholder tracks, session persistence into the graph, idempotent |
| `wav.rs` | 256 | RIFF/WAVE 16/24-bit PCM and float 32, `WAVE_FORMAT_EXTENSIBLE`, no dependencies |

The quality of this code is genuinely high for what it is. The limiter reports
`latency_frames()`; the renderer compensates for it. The stem contract is
stated honestly in the module docs (stems are pre-master-insert, so they sum to
the mix only when the master chain is linear — which the default master, with
its limiter, is not). `Stem` knows the industry abbreviations (DX/MX/FX). The
VBAP implementation handles the 3D triplet case, not just the easy 2D one. The
LUFS implementation is correct to 0.000 LU (F3).

### 1.2 What is absent — measured against what dialogue-to-delivery requires

This is the list that matters. Nothing below is a nitpick; each is load-bearing
for professional audio post.

**No I/O, no clock, no transport.**
- No audio device output or input of any kind. No `cpal`, no CoreAudio, no
  WASAPI, no ALSA, no JACK, no PipeWire.
- No playhead, no transport state, no loop, no punch, no scrub, no shuttle.
- No streaming render. `render(session, (start_s, end_s), layout)` allocates and
  returns the entire result. There is no `process(&mut [f32])` anywhere.
- Consequence: **the crate cannot be connected to a speaker.** Every claim about
  monitoring, latency, or realtime behaviour is currently unevaluable.

**No plugin hosting.** `Insert` is a closed Rust enum of five variants
(`Biquad`, `Compressor`, `Gate`, `Limiter`, `Gain`). There is no dynamic
processor, no parameter model beyond six hard-coded `Param` cases, no
plugin-state persistence, no GUI embedding, no latency reporting from an
external processor. A mix that cannot load the mixer's own tools is not a mix
stage.

**No recording.** No capture path, so no ADR, no Foley recording, no voice-over,
no live re-record. `isolate.rs` can clean dialogue; nothing can record a
replacement line. ADR specifically also needs streamers/beeps, cue lists, take
management and video-follow — none present.

**No MIDI, no OSC, no control surface.** Music editing without MIDI is audio
clip shuffling. `rosc` is not a dependency; there is no OSC surface, so no EUCON
/ Mackie / HUI-class controller can drive a fader, and no external transport can
chase.

**Time and interchange.**
- No timecode in the session at all. `Session.rate: f64` is a frame rate; there
  is no session start timecode, so a session cannot be conformed to a picture
  cut.
- No AAF, no OMF, no OTIO, no Pro Tools session, no Reaper `.RPP`, no ADM BWF,
  no BWF `bext`/iXML on write. WAV I/O writes bare RIFF with no `bext` chunk, so
  an exported file carries **no timecode**. That alone makes the crate unusable
  as the delivery end of a dual-system workflow.

**Mixing model gaps.**
- No VCA / control groups, no track folders that fold, no solo-safe, no AFL/PFL,
  no mix snapshots or recall, no bus automation (only tracks automate), no
  send automation past `Param::Send(n)` on a track.
- No reverb. `Delay` exists in `dsp.rs` but is not exposable as an `Insert`; the
  entire spatial-reverb / convolution problem is untouched, and sound design
  without reverb is not sound design.
- No dynamic EQ, no multiband, no de-esser, no de-noise as an insert (the
  isolation path is offline-only, on whole buffers).
- No clip-level anything beyond gain and two fades: no clip EQ, no clip-based
  automation, no elastic audio / time-stretch / pitch-shift, no varispeed, no
  reverse, no clip effects. Dialogue editing is fundamentally clip surgery.
- No plugin delay compensation across the graph. Insert latency is compensated
  *inside a chain* (`Chain::run_whole` drains it), which is correct for an
  offline whole-buffer render and does not exist as a concept for a graph with
  sends, parallel paths and differing bus depths.
- No monitoring section: no dim, no mono-check, no downmix-check, no calibrated
  monitor level (the 85 dB SPL / −20 dBFS pink-noise reference of a dub stage),
  no bass management, no LFE 10 dB in-band gain convention on delivery.

**Immersive and object audio.** `ChannelLayout::Atmos714` is a *channel bed*
with four height speakers, and `VbapPanner` can place an object into it. That is
useful and honest, but it is **not object-based audio**: there is no object
model that survives the render (objects are collapsed into the bed at render
time), no per-object metadata track, no ADM/BWF serialisation, no binaural
render, no renderer-agnostic representation. Calling `Atmos714` "Atmos" in the
UI would be a claim the code cannot support — the same error `research/10` §A.6
warns about for ACES.

**Metering.** Integrated LUFS and true peak exist as whole-buffer functions.
Absent: momentary (400 ms) and short-term (3 s) loudness, loudness range (LRA,
EBU Tech 3342), a loudness meter with the Tech 3341 ballistics, real-time
metering of any kind, phase correlation, spectrum analyser, per-channel peak
hold, dialogue-gated loudness (ATSC A/85 / Dolby LM100-style speech gating,
which is what US broadcast delivery actually specifies).

### 1.3 The three defects worth fixing regardless of what happens next

**D1 — one-sample clip joins (F5).** `render_track` computes
`start = round(start_s * sr)` and `len = round(duration_s * sr)` independently.
The fix is to derive the end from the *next* boundary, or to hold clip ranges in
samples. This is a correctness bug in shipped code.

**D2 — true peak reads 0.3–0.5 dB high on programme material (F4).** Replace
the generic sinc resampler in `true_peak_dbtp()` with the BS.1770-4 Annex 2
4× polyphase filter, or delegate to `ebur128` (§8). Keep the existing
`lufs_integrated()`; it is correct.

**D3 — per-render resampling of mismatched clips.** `render_track` does
`Arc::new(buf.resample(sr))` on every render call for any clip whose rate
differs, with no cache, using an interpolator that recomputes 16 windowed-sinc
weights — two transcendental calls each — per output frame. This is why
`true_peak_dbtp()` on a 20-second 5.1 buffer takes seconds. Rate conform belongs
at import, once, recorded on the media node (`research/10` §B.7.5 already says
this for pull-down).

---

## 2. The architecture: where every boundary runs

`research/10` §B.8 already fixed the *lower* boundary — `cine-media` owns
arrival, `cine-audio` owns sound — and this document does not relitigate it.
What is missing is everything *above* `cine-audio`: the boundary between a
sound library and a post-production system, and where editorial, dialogue,
design, music, mix, master and delivery each live.

### 2.1 The mistake to avoid

The tempting move is to grow `cine-audio` until it is a DAW: add a device, add
a transport, add plugins, add recording. That produces one crate that is
simultaneously a pure DSP library (unit-testable, deterministic, no I/O) and a
real-time application (device-bound, non-deterministic, thread-disciplined).
Those two things have opposite testing models, opposite dependency profiles and
opposite failure modes, and a crate that is both is testable as neither.

The same mistake in the other direction is equally available: build a
`cine-daw` that re-implements gain, panning, biquads and metering because
reaching into `cine-audio` felt awkward. `research/10` §B.8.3 already forbids
the media-layer version of this ("no second buffer type, no second resampler,
no DSP in `cine-media`"). The rule generalises.

### 2.2 The split: three crates, not one

```
                          ┌──────────────────────────────────────┐
  cine-media              │ arrival: demux, decode, PTS, index,  │
  (exists)                │ device clock, LTC decode, BWF/iXML   │
                          └───────────────┬──────────────────────┘
                                          │ AudioBuffer + exact Timestamp
                                          ▼
                          ┌──────────────────────────────────────┐
  cine-audio              │ sound: buffers, DSP, panning,        │
  (exists, keep pure)     │ downmix, loudness, session MODEL,    │
                          │ offline render. NO I/O, NO THREADS.  │
                          └───────────────┬──────────────────────┘
                                          │ pure functions + Session data
                                          ▼
                          ┌──────────────────────────────────────┐
  cine-audio-engine       │ the real-time half: graph compile,   │
  (NEW)                   │ audio callback, ring buffers, disk   │
                          │ streaming, plugin host, transport,   │
                          │ recording, metering taps             │
                          └───────────────┬──────────────────────┘
                                          │
                          ┌───────────────┴──────────────────────┐
  cine-pipeline           │ reactors: script → dialogue          │
  (exists)                │ placeholders, conform, staleness     │
                          └──────────────────────────────────────┘
```

**`cine-audio` stays a library and gets *smaller* in ambition, not larger.**
It keeps buffers, DSP, panning, downmix, loudness, the `Session` data model and
the offline renderer. It gains streaming variants of what it already has (a
resampler with retained state, per `research/10` §B.8.3; block-wise metering
state) and nothing else. It never grows a thread, a device, a file handle it
did not already have, or a `dyn` processor it cannot instantiate deterministically.

**`cine-audio-engine` is new and owns everything real-time.** It is the only
crate that may allocate a thread, open a device, or hold a plugin. It depends on
`cine-audio` for every DSP kernel and on `cine-media` for decode and the clock.
It is the crate whose tests are integration tests, and the crate that is allowed
to be unverifiable on a machine with no audio hardware (§7).

Why a separate crate rather than a feature flag: a feature flag leaves the
real-time code in the same compilation unit as the pure code, so the discipline
of §4 has no boundary to be enforced at. A crate boundary makes "no allocation
in the callback" a reviewable property of one directory.

### 2.3 The nine stages, and who owns each

| Stage | Owner | What it means concretely | Status |
|---|---|---|---|
| **1. Media audio** | `cine-media` | Decode to planar `f32` with an exact `Timestamp`; BWF `bext`/iXML; embedded timecode; LTC decode | decode **done**; BWF/iXML/LTC **absent** |
| **2. Sync** | `cine-timeline::sync` + `cine-media` metadata | Timecode match → scene/take restriction → waveform correlation → manual, with the `method` recorded (`research/10` R8) | correlator **done**; metadata-first ordering **absent** |
| **3. Editorial conform** | `cine-timeline` → `cine-audio::Session` | An NLE cut becomes a session: clips, handles, track roles from the channel-role map, session start timecode | **absent** — there is no timeline → session conform at all |
| **4. Dialogue edit** | `cine-audio` (kernels) + `cine-audio-engine` (interaction) | Clip surgery: splits, slips, handles, crossfades, alternate takes, clip gain, room-tone fill, de-noise, ADR | `isolate.rs` is a good de-noise **kernel**; every editorial operation is absent |
| **5. Sound design** | `cine-audio-engine` + plugins | Layering, pitch/time manipulation, convolution and algorithmic reverb, sound-library search | **absent**; no reverb, no stretch, no plugin host |
| **6. Music** | `cine-audio-engine` | Music clip editing to picture, tempo/bar grid, MIDI, cue conform | **absent**; no MIDI, no tempo model |
| **7. Mix** | `cine-audio::Session` model + engine execution | Buses, stems, VCAs, sends, automation modes (read/touch/latch/write), PDC, monitoring section, immersive panning | model is ~40 % there; automation is read-only-with-lanes; no VCA, no PDC graph, no monitor section |
| **8. Master** | `cine-audio` | Loudness normalisation to a target, true-peak ceiling, dialogue-gated measurement, downmix verification | LUFS **correct** (F3); true peak **wrong by 0.3–0.5 dB** (F4); no LRA, no short-term, no dialogue gate |
| **9. Delivery** | `cine-audio` (render) + `cine-media` (encode) + a new `cine-audio::adm` | Stems in a named layout, ADM BWF for immersive, printmaster, M&E, loudness report, dialogue-only stem | render **done**; every delivery *format* absent, including BWF timecode on export |

The pattern is worth stating plainly: **the crate has the middle of the process
and neither end.** It can mix; it cannot receive an edit and it cannot deliver a
package.

### 2.4 The three boundary rules

**BR1 — Anything true of a signal regardless of provenance is `cine-audio`.**
The `research/10` §B.8.3 test generalises: if the function would be equally
correct on a synthesised sine, it belongs in `cine-audio`. That puts reverb,
time-stretch, dynamics, metering and panning in `cine-audio`, and puts device
buffers, plugin instances and transport state in `cine-audio-engine`.

**BR2 — `cine-audio` never owns time; it is *given* time.**
Sessions, clips and automation carry exact timestamps supplied by the layers
above (§3). The renderer converts to sample indices exactly once, at a stated
boundary, with a stated rounding rule.

**BR3 — The graph is the session's identity; the engine holds only a compiled
copy.** The UCG owns which tracks exist, what they are called, what they are
derived from and what they are bound to. The audio engine owns a *compiled,
immutable snapshot* of the routing and processing needed to make sound for the
next few hundred milliseconds. These are different objects with different
lifetimes and different mutability, and conflating them is what §4 forbids.

### 2.5 What must NOT happen (the §B.8.3 list, extended for audio)

* **No second `Session` model.** The engine compiles `cine_audio::Session`; it
  does not define its own track type.
* **No DSP in `cine-audio-engine`.** Same test as BR1. A gain ramp written in
  the engine because "it was easier there" is the beginning of two DSP stacks.
* **No graph access from the audio thread**, ever, in any form — not a read
  guard, not an `Arc<Graph>` clone, not a channel `try_recv` that could
  allocate. §4.
* **No plugin process in `cine-audio`.** Plugins are `dyn`, fallible,
  non-deterministic and licence-bearing; `cine-audio` must stay deterministic
  and dependency-light enough to be a reference implementation.
* **Dependency direction is one way:** `cine-audio-engine` → {`cine-audio`,
  `cine-media`} → `ucg-core`. Never the reverse.

---

## 3. Sample-accurate identity: how audio time must meet `cine-media::Timestamp`

### 3.1 The current state, measured

Three time representations coexist in this workspace today, and only one of
them is exact:

| Type | Where | Representation | Exact? |
|---|---|---|---|
| `ucg_core::RationalTime` | the graph, `cine-audio::Clip::range` | `{ value: f64, rate: f64 }` | **No** |
| `cine_media::Timestamp` | `cine-media` only | `{ ticks: i64, base: TimeBase { num: u32, den: u32 } }`, cross-multiplied in `i128` | **Yes** |
| bare `f64` seconds | `cine-audio` automation lanes, `source_offset_s`, `render()` ranges, `Session::end_s()` | `f64` | No |

`research/10` R1 called for `RationalTime` to be replaced with the exact form
**before** anything was built on it. `cine-media` did the right thing locally —
it defined its own exact `Timestamp` rather than adopt the float type — but the
core type is unchanged, so the workspace now has an exact time model in one
crate and a float one in the graph that everything else uses. `cine-audio` is
entirely on the float side.

### 3.2 What audio time must be

Audio has a property picture does not: **its native grid is the sample, and the
sample rate is exactly the kind of rational that `TimeBase` already expresses.**
48 kHz is `TimeBase { num: 1, den: 48000 }`. A sample index *is* a `ticks`
value in that base. There is no approximation anywhere in that mapping.

That gives the rule:

> **Audio time is `Timestamp` in the media sample base. Sample index and
> timestamp are the same number. Every conversion between the picture grid and
> the sample grid is an explicit, rounded, recorded operation — never an
> implicit one, and never through `f64` seconds.**

Concretely, the changes required in `cine-audio`:

1. `Clip::range` becomes a range of sample-base `Timestamp`s, not
   `TimeRange<RationalTime>` — or, if the graph type must stay for
   serialisation, the *renderer* must resolve clip boundaries through an exact
   conversion instead of `round(start_s * sr)`.
2. `source_offset_s: f64` becomes `source_offset: i64` samples. An offset into a
   source is a sample count; expressing it in seconds and multiplying back is
   pure loss.
3. **Duration is converted as a duration, never as a difference of converted
   endpoints** (`research/10` §B.1.5 rule 4). This is defect D1 (F5).
4. Automation lane keys become sample-base timestamps. Today they are `f64`
   seconds, which means a lane cannot be compared for equality, cannot be
   hashed, and cannot round-trip through the graph idempotently — the same
   family of failure `research/10` §B.1.5 rule 5 identifies for `RationalTime`.
5. `Session` gains a **start timecode** and a **frame rate as an exact
   rational**, so a session can be conformed to a picture cut at all (§2.3
   stage 3).

### 3.3 What breaks if it is not done

Not hypotheticals — each of these follows directly from the code as written.

**Clicks at every edit.** D1/F5: independently rounded start and duration put
butt-spliced clips one sample apart. In dialogue editing, where a scene is
hundreds of butt splices between takes and room tone, this is the single most
audible failure a system can have.

**Sync drift that appears only on long takes.** A clip placed by
`round(start_s * sr)` is right to within half a sample, which is fine. A clip
whose *source offset* is also rounded independently, on a source that was itself
rate-conformed by a float ratio, accumulates. `research/10` §B.7.5 already shows
the 1000/1001 case costing 0.6 s over ten minutes; the same class of error at
much smaller magnitude is invisible at the head of a take and obvious at the
tail — which is exactly the failure that is hardest to diagnose.

**Reactors that never converge.** The project's own rule (`CLAUDE.md`): "Graph
writers must be idempotent: re-running with the same input produces zero ops."
A float time that does not survive a round trip produces a different value on
re-derivation, so `tx.set` is not a no-op, so the reactor cascades again.
`research/10` §B.1.2 measured 12.1 % of frames coming back wrong from a single
cross-rate round trip in `RationalTime`. Every audio clip placed through
`RationalTime::from_seconds` inherits that. `cine-audio::ucg` writes clip ranges
as `attrs::RANGE` on every sync — so this is not theoretical for this crate, it
is the mechanism by which dialogue placeholders would fail to converge at a
non-integer rate.

**Conform that cannot be verified.** `PHASE-2-MEDIA-ENGINE.md` §4.2 makes
video seek exact by construction and proves it pixel-for-pixel. There is no
equivalent claim available for audio while placement runs through floats,
because "the same sample" is not expressible.

**No sample-accurate identity for a rendered stem.** Delivery requires that a
stem re-rendered tomorrow is bit-identical to the one delivered today, or the
QC report is meaningless. Float placement makes that a hope rather than a
property.

### 3.4 The concrete recommendation

Do `research/10` R1 — replace `ucg_core::RationalTime` with the exact form —
and then, in the *same* change, give `cine-audio` sample-base time. Doing them
separately means migrating `cine-audio`'s clip model twice. `cine-media` has
already written and tested the exact arithmetic (`Timestamp`, `TimeBase`,
`rescale` reporting exactness); the work is promoting it to `ucg-core`, not
inventing it. Until that lands, **D1 should be fixed on its own** — deriving a
clip's end frame from the next boundary rather than from an independently
rounded duration is a small, local change that removes the audible defect
without waiting for the migration.

---

## 4. Real-time audio thread discipline, and the UCG

This is the section that decides whether a real-time audio engine can exist in
this codebase at all.

### 4.1 The hard rules

The audio callback runs on a thread scheduled by the OS with a hard deadline: at
48 kHz with a 512-frame buffer it has **10.67 ms** to produce 512 frames, every
time, forever. Miss it once and the user hears a click. The rules are not
stylistic:

1. **No allocation and no deallocation.** `malloc`/`free` take a global lock and
   can take arbitrarily long. In Rust this means: no `Vec::push` that may grow,
   no `Box`, no `String`, no `format!`, no collection insert, no `clone()` of
   anything owning heap memory, and **no `Drop` that frees** — which is what
   `basedrop` exists to solve (drop the `Arc` into a queue; a non-real-time
   thread does the actual free).
2. **No locks.** No `Mutex`, no `RwLock`, no `parking_lot`, no
   `try_lock`-then-fallback (the fallback is a dropout, which is the thing you
   were avoiding). Priority inversion against a lower-priority thread holding
   the lock is unbounded.
3. **No I/O.** No file reads, no `println!`, no logging, no syscalls that can
   block. Disk streaming happens on a reader thread that fills a ring buffer;
   the callback only ever pops.
4. **No unbounded work.** No algorithm whose cost depends on data the callback
   cannot bound: no sorting, no hash-map rehash, no graph traversal of unknown
   depth, no `Vec` search over an unbounded list.
5. **No panics.** A panic unwinding through a C callback is undefined behaviour.
   Every index into a buffer is bounds-checked by construction or the code does
   not go in the callback.
6. **No blocking synchronisation of any kind** — no channel `send` that can
   block, no condvar, no thread spawn, no `Instant::now()` in a hot loop if the
   platform's clock syscall can be slow.
7. **Denormals off.** Flush-to-zero / denormals-are-zero set on the audio
   thread, or a decaying reverb tail costs 100× the CPU of the same code with
   signal in it. This is a real dropout cause, not a micro-optimisation.

The enforcement mechanism, not just the intention: `assert_no_alloc` (or an
equivalent allocator shim) around the callback in debug and in CI, so a
violation is a test failure rather than an intermittent click on a user's
machine. **UNVERIFIED (U9)** — that crate's current status was not checked in
this session.

### 4.2 The collision with the UCG, stated precisely

`ucg-core` is exactly the wrong shape for the audio thread, and this is not a
flaw in `ucg-core` — it is the correct shape for what it does:

* `Engine` is behind an `RwLock` that `CLAUDE.md` warns is not reentrant, and
  whose commit path can now time out with `UcgError::WriteLockTimeout`. **Rule
  2 violated by construction.**
* `Graph::snapshot()` was measured at **363 ms for a full clone at 56 k nodes**
  (`02-TARGET-ARCHITECTURE.md` §3.3). That is 34,000 audio buffers' worth of
  deadline. **Rules 1 and 4 violated.**
* Node attributes are `Value` variants in maps, resolved by string key. Reading
  a fader level would be a hash lookup returning a possibly-allocating value.
  **Rules 1 and 4 violated.**
* Reactors cascade on commit and are measured at up to 144 ms for one edit
  (`04-RISK-REGISTER.md` R1). Nothing on the audio thread may ever be able to
  trigger that.

> **The rule: the audio thread never sees the graph. Not a reference, not a
> guard, not a snapshot, not an `Arc<Graph>`. The type system should make it
> impossible, not the code review.**

### 4.3 The mechanism: compile, hand over, drop elsewhere

The shape that works, and that every serious engine converges on:

```
 main / UI thread            worker threads              AUDIO CALLBACK
 ────────────────            ──────────────              ──────────────
 engine.transact(..)  ──►    compile Session
   (UCG owns identity)       into a flat, allocation-
                             free RenderGraph
                                    │
                                    ▼  triple_buffer / arc-swap publish
                                            ┌──────────────────────────┐
                                            │ read current RenderGraph │
                                            │ (immutable, pre-sized)   │
                                            │ pop audio from ring bufs │
                                            │ run fixed processor list │
                                            │ push meters to ring buf  │
                                            └──────────────────────────┘
                                    ▲
 disk reader thread ────────────────┘  rtrb ring buffer per streaming clip
 (decode via cine-media, resample, push)

 old RenderGraph ──► basedrop collector ──► freed on a non-RT thread
```

The five load-bearing properties:

1. **The `RenderGraph` is compiled, not interpreted.** Compilation happens on
   the main thread inside or just after the transaction: topological bus order
   (`mix::bus_order` already computes this), per-processor state pre-allocated,
   PDC delays computed, scratch buffers sized for the worst-case channel count.
   The callback executes a flat list; it makes no decisions that could be
   unbounded.
2. **Hand-over is a single atomic pointer swap**, published with
   `triple_buffer` or `arc-swap`. The callback never waits. If a new graph is
   not ready, it keeps using the old one — which is correct behaviour, not a
   fallback.
3. **The old graph is freed on another thread** via `basedrop`. This is the
   only correct answer to rule 1's `Drop` clause.
4. **Parameter changes do not recompile the graph.** A fader move is a value
   written into a lock-free parameter slot, smoothed in the callback. Only
   *structural* changes (a track added, a plugin inserted, routing changed)
   recompile. Getting this wrong — recompiling on every fader move — is how an
   engine becomes unusable under automation.
5. **Everything travelling the other way is a ring buffer**: meter values,
   playhead position, recorded audio, plugin GUI state. The UI thread drains
   them at frame rate and, *only if it decides something is worth persisting*,
   opens a transaction. **Metering never enters the graph.** This is the audio
   instance of `02-TARGET-ARCHITECTURE.md` §3.4's bulk-data boundary: a peak
   meter at 60 Hz on 128 tracks is 7,680 values per second, which is a side
   store or a channel, never nodes.

### 4.4 Where the transaction actually happens

A user drags a fader during playback. The correct sequence:

1. UI writes the new value into the parameter slot → audible on the next block.
   **No transaction.**
2. UI accumulates the gesture.
3. On mouse-up (or on a debounce), **one** `engine.transact()` records the final
   automation edit as an undoable operation.
4. The transaction's `ChangeSet` triggers a recompile of the affected part of
   the `RenderGraph`, published by pointer swap.

The gesture is heard immediately and recorded once. If instead every mouse-move
opened a transaction, the reactor cascade (144 ms at scale) would run per pixel
of fader travel — and the `CLAUDE.md` rule about never holding a read guard
across a transact becomes a deadlock risk on the UI thread while audio is
running.

**Automation *write* mode is the hard case** and should be designed for
explicitly: writing automation while transport rolls generates data on a
real-time thread. It must go out through the same ring buffer as metering, be
thinned on the UI thread, and be committed as *one* transaction at the end of
the pass — never as a stream of ops.

### 4.5 What this means for `cine-audio` as it stands

`mix::render` allocates on every call (`AudioBuffer::new`, `Vec` per track, a
`HashMap` for stems, `rayon` par_iter), resamples clips inline, and returns an
owned buffer. **None of it can be called from a callback**, and none of it
should be changed to try. The right move is that the engine reuses
`cine-audio`'s *processors* (`Biquad`, `Compressor`, `Limiter`, `VbapPanner`,
`Downmix`) — all of which already have `process(&mut [f32])` shapes and
pre-allocated state — and provides its own allocation-free scheduling around
them. `render()` survives as the offline/bounce path, where allocating is
correct.

Two small changes to `cine-audio` make this materially easier and cost nothing:
give every processor an explicit `reset()` (most already have one) and a
`latency_frames()` (only `Limiter` has one today), so the engine can compile
PDC without special-casing processor types.

---

## 5. Loudness compliance: what must be measured, and the verified Rust path

### 5.1 What a delivery specification actually demands

"EBU R 128 compliant" is four numbers and a gate, not one number. A delivery
QC report that omits any of them is not a report.

| Quantity | Standard | What it is | In `cine-audio`? |
|---|---|---|---|
| **Integrated loudness (I)** | ITU-R BS.1770 §gating; EBU R 128 | K-weighted mean over the whole programme, with a −70 LUFS absolute gate and a −10 LU relative gate | **Yes, and correct** (F3) |
| **Momentary (M)** | EBU Tech 3341 | 400 ms sliding window, ungated | **No** |
| **Short-term (S)** | EBU Tech 3341 | 3 s sliding window, ungated | **No** |
| **Loudness Range (LRA)** | EBU Tech 3342 | Distribution width of short-term loudness: 10th–95th percentile, with its own −20 LU relative gate | **No** |
| **Maximum True Peak (dBTP)** | BS.1770 Annex 2 | Inter-sample peak via ≥4× oversampling | **Yes, but wrong by 0.3–0.5 dB** (F4) |
| **Meter ballistics and scale** | EBU Tech 3341 | The 'EBU Mode' meter, plus the compliance test signals a meter must pass | **No** |

EBU R 128 v5.0 (November 2023, freely downloadable from `tech.ebu.ch`)
specifies a **Programme Loudness target of −23.0 LUFS**. The true-peak ceiling
and the tolerance band were not read from the document in this session
(**UNVERIFIED, U10**) and must be quoted from R 128 itself rather than from
memory before any number appears in the product.

**The standard has moved and the code has not.** `buffer.rs` documents itself
as BS.1770-**4**. **BS.1770-5 (November 2023) is the in-force version**;
BS.1770-4 (October 2015) is marked superseded on the ITU's own page. The
measured agreement with `ebur128` (F3) is evidence the *implementation* is
right; it is not evidence it is current. What changed between -4 and -5 was not
read (**UNVERIFIED, U11**).

### 5.2 The part nobody implements, and film delivery requires

Two things above the EBU set matter specifically for film and television
delivery, and both are absent:

**Dialogue-gated loudness.** ATSC A/85 — the US broadcast requirement behind
the CALM Act — measures the **Anchor Element**, which for most programmes is
the dialogue, not the full mix. Verified from the ATSC document: *"Dialog forms
the Anchor Element in the majority of television programs, and during production
it is usually possible to isolate the dialog and measure its loudness
accurately, regardless of the overall dynamic range or style of the finished
program."* A full-mix integrated measurement of an action film and a
dialogue-anchored measurement of the same film differ by several LU. The
current in-force version is A/85:2026-07 (approved 8 July 2026), freely
available from `atsc.org`.

**This is a place where CINE-OMNI has a structural advantage worth taking.**
Measuring the anchor element is hard for a generic loudness meter because it
must guess what dialogue is. CINE-OMNI does not have to guess: `Stem::Dialogue`
is a first-class concept, `render_stems` already produces a dialogue stem, and
`ucg.rs` already ties dialogue clips to screenplay lines with `SyncedTo` edges.
A dialogue-gated loudness measurement is `lufs_integrated()` on the dialogue
stem — code that already works, applied to a signal the graph already
identifies. That is a genuine capability most tools approximate, available
almost for free, and it should be built.

**Per-delivery targets differ and must be data, not constants.** −23 LUFS
(EBU R 128, European broadcast), −24 LKFS (ATSC A/85, US broadcast), −27 LKFS
(Netflix, dialogue-gated, **UNVERIFIED, U12** — vendor specifications change and
were not fetched), theatrical mixes which are calibrated to a monitoring SPL
rather than normalised to a loudness target at all. A delivery target belongs in
a `DeliverySpec` entity in the graph, versioned like everything else, never
hard-coded in the renderer.

### 5.3 The verified Rust path

**`ebur128` v0.1.10 — MIT — is real, and it is the reference to measure
against.** Verified from the crates.io API: newest version 0.1.10, licence
`MIT`, repository `https://github.com/sdroege/ebur128`, 912,216 total downloads,
last release **2024-10-26**. It is a Rust port (not a binding) of `libebur128`,
whose own `COPYING` at `github.com/jiixyj/libebur128` is verbatim **MIT,
"Copyright (c) 2011 Jan Kokemüller"** — verified. It compiles here with no C
toolchain (F9). Nearly two years without a release is dormancy, not
abandonment, for a crate implementing a frozen standard — but it is a fact to
record rather than gloss.

**The recommendation is to keep the native implementation and use `ebur128` as
the oracle, not the runtime.** Reasons:

1. `cine-audio::lufs_integrated()` **already agrees with it to 0.000 LU** across
   sine, noise, mono, stereo and 5.1 (F3). There is nothing to gain by
   replacing correct code with a dependency.
2. A native implementation can be *streaming and block-wise*, which the
   real-time meter of §4 needs and which a whole-buffer API cannot provide.
3. It keeps the loudness path free of any external dependency in the delivery
   report — which matters, because the delivery report is the artefact a client
   audits.
4. `ebur128` earns its place as a **test-only dev-dependency**: a golden
   cross-check in CI, exactly as used to produce F3 and F4. That is the same
   pattern `research/10` R3 recommends for OCIO (own implementation, third party
   as an evidence generator), and it is the right pattern here for the same
   reason.

**What must be built natively, in order:**

1. **Fix true peak (D2).** Implement the BS.1770 Annex 2 4× polyphase
   oversampling filter properly, then re-run the F4 cross-check and require
   agreement to within a stated tolerance. Until it agrees, the true-peak number
   must not be shown as a compliance figure.
2. **Streaming M / S / I state**, so a meter can run in the engine (§4.3 rule 5:
   values leave the audio thread through a ring buffer).
3. **LRA** per Tech 3342.
4. **Tech 3341 conformance run.** EBU publishes test signals with expected
   readings; passing them is the only honest basis for claiming "EBU Mode".
   Until then the correct product string is *"loudness measured per ITU-R
   BS.1770"*, never *"EBU R 128 certified"* — the same naming discipline
   `research/10` R7 and R10 impose on ACES and HDR10+.
5. **Dialogue-gated integrated loudness** over the dialogue stem (§5.2).
6. A **loudness report** as a graph artefact — measured values, target,
   pass/fail, the exact standard version used, and the render it was measured
   from — so a delivery can be re-verified rather than re-asserted.

---

## 6. Plugin hosting and the licence wall

### 6.1 The headline: the standing assumption in this blueprint is now wrong

`01-LICENSING-SPINE.md` §5 records "CLAP vs LV2 vs VST3 for audio. **VST3 is a
known trap**: dual-licensed GPLv3 or a proprietary Steinberg agreement, so the
GPLv3 arm is unusable here", and `02-TARGET-ARCHITECTURE.md` §4 repeats it. It
also records "ASIO (Windows audio) licensing — a known trap."

**Both statements were true when written and both are now inverted.** Verified
from primary sources:

> **VST 3 SDK — MIT.** `https://github.com/steinbergmedia/vst3sdk/blob/master/LICENSE.txt`
> is a verbatim MIT License, "Copyright (c) 2026, Steinberg Media Technologies
> GmbH". The `vst3_pluginterfaces` submodule carries the same MIT text and
> copyright line. The repository README states: *"Licensing under GPLv3 and the
> Steinberg proprietary license is no longer available."* The change shipped
> with **VST 3.8**, announced **29 October 2025**.

> **ASIO SDK — GPLv3 or proprietary.** `https://github.com/audiosdk/asio`
> carries `LICENSE.txt` (dual) plus `Steinberg ASIO Licensing Agreement.pdf`.
> Its README: *"Developers can choose between two options: Use of the
> proprietary ASIO SDK license, for use cases that benefit from closed-source
> licensing. Adopt the General Public License (GPL) Version 3 for full
> open-source integration. Both options grant access to the same SDK."*
> SDK version **ASIO 2.3**.

So the trap moved. **VST3 became free; ASIO became the GPL problem.**
For an Apache-2.0 product:

* **VST3 is now usable.** MIT is on `deny.toml`'s allow list already. The GPLv3
  arm that made it unusable no longer exists, and *no signed agreement is
  required* — the Steinberg developer portal FAQ confirms companies no longer
  sign anything.
* **ASIO is now unusable in-process.** The GPLv3 arm cannot be linked into an
  Apache-2.0 binary (`01-LICENSING-SPINE.md` §2: GPL-3.0 → Apache-2.0 is "No").
  The proprietary arm requires signing Steinberg's agreement, which
  `research/10` R10 argues against as a class of move ("do not pre-emptively
  contract"). **LEGAL REVIEW REQUIRED** before any ASIO support ships.
  The practical consequence: on Windows, CINE-OMNI targets **WASAPI (shared and
  exclusive mode)** and treats ASIO as a deliberate gap with a stated reason,
  not an oversight. This is the same call Ardour, REAPER's free build path and
  every LGPL-conscious tool face; it costs some Windows low-latency ceiling and
  costs nothing legally.

**This finding must be propagated.** `01-LICENSING-SPINE.md` §5 and
`02-TARGET-ARCHITECTURE.md` §4 both need correcting; leaving them is worse than
never having written them, because a future decision will be made from a stale
premise.

### 6.2 The four formats, verified

| Format | Licence (verified) | Source fetched | In-process for Apache-2.0? |
|---|---|---|---|
| **CLAP** | **MIT** — "MIT License / Copyright (c) 2021 Alexandre BIQUE" | `raw.githubusercontent.com/free-audio/clap/main/LICENSE` | **Yes, unconditionally** |
| **LV2** | **ISC** — "Copyright 2006-2012 Steve Harris, David Robillard. Copyright 2000-2002 Richard W.E. Furse, Paul Barton-Davis, Stefan Westerfeld." | `raw.githubusercontent.com/lv2/lv2/master/COPYING` | **Yes** for the *specification headers*. The common *host libraries* are a separate question — see §6.4 |
| **VST3** | **MIT** (since VST 3.8, Oct 2025) | `github.com/steinbergmedia/vst3sdk/blob/master/LICENSE.txt` + README | **Yes** — reversed from the prior finding |
| **AAX** | Proprietary, Avid; SDK access by registration/agreement | **UNVERIFIED** — see register U7 | **No.** Avid-only, and irrelevant unless CINE-OMNI is certified for Pro Tools |

`deny.toml` already allows MIT and ISC, so CLAP, LV2 and VST3 all pass the
existing policy gate without amendment.

### 6.3 The host/plugin boundary argument — and why it is worth citing

The question that actually matters is not "may I link the SDK", it is "does
loading a GPL plugin infect my host, or does my Apache-2.0 host infect a GPL
plugin?" Ardour — the most GPL-scrupulous DAW there is — publishes its position
at the top of its own `COPYING`. Verified verbatim from
`raw.githubusercontent.com/Ardour/ardour/master/COPYING`:

> *"The following states the Ardour developers understanding of the relationship
> between Ardour's license (GPL v2.0 or later) and plugins."* … plugins are
> *"software dynamically loaded by another program, where the interaction
> between the plugin and the loading program (typically called 'the host') is
> defined by a 3rd party API"* — naming *"VST, AudioUnits, LV2, LADSPA and
> CLAP"* — and: *"Since (a) any such plugin may be loaded by any host that
> supports the API and (b) any host may load any plugin that supports the API,
> we consider it self-evident that there can be no 'derivative' relationship
> (in the sense of the GPL's use of this term) between the host and plugin."*
> Conclusion: *"Ardour may load arbitrarily licensed plugins … without any
> implications for either Ardour or the plugin."*

This is a *developers' understanding*, not a court ruling, and it is Ardour's
position about Ardour. It is nonetheless the strongest publicly-stated position
by the most exposed party, and it cuts in CINE-OMNI's favour in both
directions: an Apache-2.0 host may load a GPL plugin, and a GPL plugin author
has no claim on the host. **LEGAL REVIEW REQUIRED** before this is relied on in
marketing or in a distribution that bundles GPL plugins — bundling is a
different act from loading, and the analysis above covers only loading.

The rule CINE-OMNI should adopt, which needs no legal opinion at all:

> **Ship no plugin binaries. Load whatever the user has installed.**

That keeps CINE-OMNI's distribution free of every plugin's licence, makes the
"mere aggregation" question moot because there is no aggregation, and is what
every host does anyway.

### 6.4 So which format, and in what order?

**CLAP first.** Reasons, in order of weight:

1. **Licence.** MIT, no agreement, no trademark obligation, no registration.
   VST3 is now equally free but CLAP has been free from the start and has no
   trademark programme to comply with at all.
2. **A pure-Rust host exists and compiles here today** (F9): `clack-host 0.1.1`
   over `clap-sys 0.5.0`, built successfully on this machine with no C
   toolchain and no `pkg-config`. That matters enormously given §7 — it is the
   *only* plugin path that can be developed on this machine.
3. **The ABI is C and stable**, with the threading contract written into the
   header rather than into a PDF. CLAP states which calls are `[main-thread]`
   and which are `[audio-thread]`, which is precisely the discipline §4
   requires CINE-OMNI to enforce anyway.
4. It was created, per Libre Arts' account, because of Steinberg's licensing
   policy — the policy that has now changed. That history is a reason to expect
   CLAP to stay permissive, not a reason to doubt it.

**VST3 second, and now genuinely available.** It is where the commercial
plug-in ecosystem actually lives, and post-audio users own VST3 licences for
tools that have no CLAP build. Its cost is not legal any more; it is
engineering — the C++ COM-style ABI needs a Rust binding, and the mature
bindings in the ecosystem predate the relicensing (see §8: `vst3-sys` and
friends were written against a GPL-or-proprietary SDK and their own licence
terms must be re-read, not assumed).

**LV2 third, on Linux.** ISC specification, and it is what the Linux audio
ecosystem ships. The trap is not the spec, it is the *reference host stack*:
`lilv`, `serd`, `sord`, `sratom` and `suil` (David Robillard's). Their licences
were **not fetched in this session — UNVERIFIED, register U8** — and they must
be, because linking a host library is a completely different act from compiling
against an ISC header. A pure-Rust LV2 host (`livi`) exists; its licence and
maturity are also unverified (U8).

**AAX: reject.** Proprietary, Avid-gated, and it exists to run inside Pro Tools.
Interoperating with Pro Tools is an *interchange* problem (AAF/OMF, stems, EDLs),
not a plugin problem.

### 6.5 What "hosting a plugin" actually costs, beyond the licence

Naming this so it is not discovered later. A plugin host must provide:
parameter enumeration with automation and modulation; sample-accurate parameter
events and a note/event queue; latency reporting and graph-wide delay
compensation; state save/restore into the session (and therefore into the UCG);
a GUI embedding path per platform, plus a generic fallback UI for plugins whose
GUI cannot be embedded; a scanning and blacklist mechanism, because a bad plugin
crashes on scan; **out-of-process sandboxing**, because a bad plugin crashes at
runtime and taking the whole application down with a user's mix is unacceptable;
and a threading contract the host never violates (§4).

None of that exists in `cine-audio` today — `Insert` is a five-variant enum
(§1.2). The plugin decision is therefore not "pick a format", it is "build a
host", and the format choice is the smallest part of it.

---

## 7. Audio backends — what is actually reachable on this machine

Probed, not assumed. No packages were installed.

| Probe | Result |
|---|---|
| `pipewire` daemon | **present and running** — `/usr/bin/pipewire`, socket `/run/user/1001/pipewire-0`, `pw-cli info 0` reports version **1.0.5** |
| PipeWire clock | `default.clock.rate = 48000`, `default.clock.quantum = 1024`, `min-quantum = 32`, `max-quantum = 2048`, `quantum-limit = 8192` |
| PipeWire nodes | `alsa_output.pci-0000_00_1f.3.analog-stereo`, `alsa_input…analog-stereo`, `Midi-Bridge`, `Dummy-Driver`, `Freewheel-Driver` |
| ALSA | `libasound.so.2` present; card 0 `HDA Intel PCH` (CX11880 Analog + 3× HDMI); `aplay -L` shows `pipewire` and `default → PipeWire Media Server` |
| PulseAudio | daemon **absent**; `libpulse.so.0` present; PipeWire serves the Pulse API via `libpipewire-module-protocol-pulse.so` |
| JACK | `jackd` **absent**; no `libjack` in the system prefix (only inside a snap image); PipeWire ships `module-jackdbus-detect` and `module-jack-tunnel`, but `pipewire-jack` itself is not installed |
| Pro interface | **none.** One consumer HDA codec, stereo analog out. No multichannel device, so 5.1/7.1.4 monitoring cannot be tested here at all |
| Build headers | **no `pkg-config`, no `libasound.so` symlink, no `/usr/include/alsa`** |
| `cpal 0.15.3` build | **FAILS** — `alsa-sys 0.3.1` build script panics: "The pkg-config command could not be found" |
| Installed plugins | **none** — no `/usr/lib/{lv2,vst3,clap,ladspa}`, no `~/.{lv2,vst3,clap}` |

**What this means for the project, honestly:** this machine can *develop* a
pure-Rust audio stack (F9) but cannot *link* a C audio backend and cannot
monitor anything above stereo. Any claim about device latency, driver
behaviour, multichannel monitoring or plugin hosting is unverifiable here and
must be marked as such until it is measured on a machine with a real interface.
This is the audio analogue of `PHASE-2-MEDIA-ENGINE.md` §1's "no system
FFmpeg" constraint, and it should shape the same choice: prefer the pure-Rust
path, and put anything that needs a C library behind a feature flag that CI on
this machine does not compile.

---

## 8. Immersive audio, object-based delivery, and interchange

### 8.1 The honest position on "Atmos"

`ChannelLayout::Atmos714` is a **7.1.4 channel bed**. `VbapPanner` places a
source into it by amplitude panning across speaker triplets. That is a correct
and useful implementation of *channel-based immersive audio*, and it is not
Dolby Atmos.

Object-based audio means the object survives the mix: position, size and gain
travel to the delivery as *metadata* alongside audio essence, and the renderer
at the far end — a cinema processor, a soundbar, a pair of headphones — decides
what speakers exist. `cine-audio` collapses objects into the bed at render time
(`mix::pan_matrix` returns gains; the object is gone afterwards). There is no
object model in `Session`, no per-object metadata over time, and no
serialisation of either.

Applying `research/10` §A.6.3's rule — never call an approximation by the name
of the standard it approximates — the product strings are:
*"7.1.4 channel-based immersive mixing with VBAP panning"*, and never *"Dolby
Atmos"*, *"Atmos bed"*, or *"object-based audio"*. The enum variant name
`Atmos714` should be renamed to `Surround714` for the same reason the ACES
labels were corrected: the name leaks into the UI.

### 8.2 The open route exists, and it is Apache-2.0

This is the good news of this section. Object-based audio has an **open,
non-proprietary interchange model**, and the reference libraries are licensed
compatibly. Verified:

| Project | Licence (verified) | Source fetched | What it is |
|---|---|---|---|
| `ebu/libadm` | **Apache License 2.0** | `raw.githubusercontent.com/ebu/libadm/master/LICENSE` | C++ library for the Audio Definition Model (ITU-R BS.2076) |
| `ebu/libbw64` | **Apache License 2.0** | `raw.githubusercontent.com/ebu/libbw64/master/LICENSE` | C++ library for BW64 (broadcast WAV 64-bit, RF64) — the container ADM rides in |
| `ebu/ebu_adm_renderer` ("EAR") | **BSD-3-Clause-Clear** | `github.com/ebu/ebu_adm_renderer` | Python; reference implementation of **ITU-R BS.2127**, the ITU ADM Renderer |

All three are on `deny.toml`'s allow list or trivially addable (BSD-3-Clause is
already allowed). **ADM/BW64 is therefore a delivery format CINE-OMNI may
implement fully, natively, with no licence obstacle at all** — the position
Dolby Atmos does not offer.

The strategic reading is the same as `research/10` R10 made for HDR: **take the
open standard properly, and treat the proprietary one as detect-and-preserve.**
An ADM BW64 master is a real deliverable that real facilities accept, and it is
also the input format Dolby's own tooling ingests. Building ADM export is
therefore both the open path and the pragmatic path.

**What is not verified:** whether the *Dolby Atmos Master ADM BWF* profile —
the constrained ADM profile Dolby tools expect — is publicly specified, and on
what terms. **UNVERIFIED, U13.** Do not claim Atmos-compatible ADM output until
that is read.

**No Rust implementation of ADM or BW64 was found to exist** (see §9). This is a
build-it-ourselves item, TYPE A, informed by the Apache-2.0 C++ reference rather
than linking it.

### 8.3 Interchange is the largest single gap, and it is not glamorous

A mix stage does not receive a project file. It receives **AAF or OMF** from the
picture department, and it delivers **stems, a printmaster, an M&E and a
loudness report**. `cine-audio` can do none of this: `wav.rs` writes bare RIFF
with no `bext` chunk, so an exported file carries **no timecode at all**, which
makes it unusable in any dual-system workflow regardless of how good the mix is.

Ordered by how much each unlocks:

1. **BWF write with `bext` (`TimeReference` as a 64-bit sample count) and
   iXML.** `research/10` §B.7.4 already identifies `TimeReference` as "the
   single most valuable field in dual-system". Reading it is `cine-media`'s job;
   *writing* it is `cine-audio`'s, and it is a few hundred lines. Nothing else
   on this list is cheaper per unit of usefulness. **A crate exists**:
   `bwavfile` v2.0.1, MIT, with a `Bext` struct, a reader and a writer, and
   RF64/BW64 support — which also covers item 2. It is dormant (last release
   2023-06-03) and its `chna`/iXML coverage is **UNVERIFIED (U29)**, so the
   decision is read-it-or-use-it, not build-from-nothing. Note that `hound`, the
   obvious WAV crate, is **plain WAV only** — no `bext`, no RF64, no >4 GiB —
   and is therefore not sufficient for delivery.
2. **RF64 / BW64**, because a multichannel immersive master exceeds the 4 GB
   RIFF limit routinely.
3. **AAF import** for conform from the picture cut. This is the hard one — AAF
   is a structured-storage format with a large model. Its SDK licence and any
   Rust implementation are **UNVERIFIED, U14**.
4. **ADM BW64 export** (§8.2).
5. **OTIO** as the lighter-weight interchange the rest of the workspace is
   already headed toward (`research/10` R1 mentions OTIO at the serialisation
   boundary).

### 8.4 Time-stretch: a licence trap worth naming now

Dialogue editing, ADR fitting and music conform all need high-quality
time-stretch and pitch-shift. The best-known library is **Rubber Band**, and it
is **GPLv2-or-later with a paid commercial alternative** — verified from
`github.com/breakfastquay/rubberband`: *"If you wish to distribute code using
Rubber Band Library under terms other than those of the GNU General Public
License, you must obtain a commercial licence from us before doing so."*

That is the FFmpeg situation inverted: there is no LGPL arm. For an Apache-2.0
product Rubber Band is **TYPE H — reject** as a dependency. There is also no
Rust binding for it on crates.io, so the temptation does not even arise.

**But a fully permissive alternative exists and it changes the recommendation.**
Verified: **Signalsmith Stretch is MIT all the way down** — the C++ core's own
`LICENSE.txt` reads *"MIT License / Copyright (c) 2022 Geraint Luff /
Signalsmith Audio Ltd."*, and the Rust wrapper `signalsmith-stretch` v0.1.3 is
MIT, released 2025-09-18, ~83 k downloads. It is the only high-quality
time-stretch option that is permissive at every layer. The others:
**SoundTouch is LGPL-2.1** (Rust static linking defeats §6 relinking — avoid),
**Bungee is MPL-2.0** (file-level copyleft, acceptable), and `pitch_shift` 2.1.0
(MIT) is a pure-Rust phase vocoder for lighter needs.

The recommendation is therefore **TYPE B on `signalsmith-stretch`, with TYPE A
as the fallback**: `dsp.rs` already contains a correct WOLA STFT with exact
reconstruction, which is the hard half of a phase vocoder, so a native
implementation stays available if the binding proves unsatisfactory.

## 9. Rust crate reality

Verified 2026-09-08 against the crates.io JSON API (`/api/v1/crates/<name>`) and
the projects' own repositories. Nothing here is recalled. Rows marked
**(built here)** were additionally compiled on this machine today.

**Maintenance key** (relative to 2026-09-08): *active* = release within ~12
months · *dormant* = 1–3 years · *abandoned* = >3 years or explicitly dead.

Several crates declare the deprecated slash form (`MIT/Apache-2.0`) rather than
a valid SPDX expression; flagged ⚠ because `cargo-deny`'s
`confidence-threshold` may treat them differently.

### 9.1 Device I/O and servers

| Crate | Latest | Licence (declared) | Last release | ≈DL | Maintenance | Note |
|---|---|---|---|---|---|---|
| `cpal` | **0.18.2** | `Apache-2.0` | 2026-08-16 | 19.7 M | active | Device I/O only. **Cannot build here** (F8) — the failure was in `alsa-sys`, not `cpal` |
| `pipewire` | 0.10.1 | `MIT` | 2026-08-19 | 1.72 M | active | Linux only; needs system PipeWire dev libs (absent here, §7) |
| `alsa` | 0.12.1 | `Apache-2.0/MIT` ⚠ | 2026-07-31 | 21.0 M | active | Linux only; needs libasound headers (absent here) |
| `jack` | 0.13.5 | `MIT` | 2026-02-01 | 1.11 M | active | Needs a running JACK server + libjack. **The crate is MIT; libjack is LGPL-2.1** (§10.2) — the crate's licence says nothing about what it links |
| `wasapi` | 0.24.0 | `MIT` | 2026-08-12 | 325 k | active | Windows. **No ASIO** — ASIO is `cpal`'s `asio` feature plus the Steinberg SDK, now GPLv3 (§6.1) |
| `coreaudio-rs` | 0.14.2 | `MIT/Apache-2.0` ⚠ | 2026-04-29 | 13.8 M | active | macOS/iOS; partial, AudioUnit-centric coverage |

### 9.2 Real-time infrastructure

| Crate | Latest | Licence | Last release | ≈DL | Maintenance | Note |
|---|---|---|---|---|---|---|
| `rtrb` | 0.4.0 | `MIT OR Apache-2.0` | 2026-08-17 | 11.0 M | active | RT-safe SPSC ring buffer. **(built here** at 0.3.5, a maintained backport line**)** |
| `ringbuf` | 0.5.1 | `MIT OR Apache-2.0` | 2026-07-14 | 17.5 M | active | SPSC with direct inner access; breaking churn 0.3→0.4→0.5 |
| `triple_buffer` | **9.0.0** | **`MPL-2.0`** | 2026-02-22 | 941 k | active | Wait-free *latest-value* handoff — **no queueing**. Right for the `RenderGraph` swap and for meters, wrong for sample streams. **(built here** at 8.1.1**)**. MPL-2.0 is file-level copyleft and already on `deny.toml` |
| `basedrop` | 0.1.3 | `MIT/Apache-2.0` ⚠ | 2025-10-29 | 138 k | active (low) | RT-safe deferred collection. Still 0.1.x. **(built here)** |
| `creek` | 1.2.3 | `MIT OR Apache-2.0` | 2025-09-22 | 112 k | active (low) | **RT-safe disk streaming** — exactly the §4.3 reader-thread problem. Codeberg-hosted; the one surviving useful Meadowlark crate |
| `assert_no_alloc` | 1.1.2 | `BSD-1-Clause` | **2021-08-03** | 4.3 M | **abandoned** (5 yr) | Resolves **U9**. The technique (global-allocator shim, debug builds) is right; this crate is stale and its allocator override conflicts with other tooling. Write our own shim |

### 9.3 Plugin hosting

| Crate | Latest | Licence | Last release | ≈DL | Maintenance | Note |
|---|---|---|---|---|---|---|
| `clack-host` | 0.1.1 | `MIT OR Apache-2.0` | 2026-07-29 | 19 k | active | Safe CLAP **host** wrappers. Pre-1.0. **(built here)** |
| `clack-plugin` / `clack-extensions` | 0.1.1 | `MIT OR Apache-2.0` | 2026-07-29 | 24 k / 21 k | active | **(extensions built here)** |
| `clap-sys` | 0.5.0 | `MIT/Apache-2.0` ⚠ | 2025-01-03 | 162 k | dormant | Raw CLAP FFI; what `clack` sits on. **(built here)** |
| **`clap`** | 4.6.6 | `MIT OR Apache-2.0` | 2026-08-06 | **1.11 B** | active | **The CLI argument parser. Nothing to do with audio.** Confirmed explicitly; the name is permanently taken |
| **`vst3-sys`** | **NOT ON CRATES.IO** | **GPL-3.0-or-later** (`license.md`) | last commit 2023-06-19 | — | **abandoned** | **REJECT.** Its README: *"vst3-sys is licensed under the terms of the GNU GPLv3 license. This port is a derivative work of the original SDK."* The authors chose GPLv3 under the *old* dual licence; the SDK going MIT does not relicense their code |
| `vst3` | 0.3.0 | `MIT OR Apache-2.0` | 2025-12-07 | 57 k | active | `coupler-rs`; bindings generated from the SDK headers. **The viable VST3 path.** ⚠ its README says nothing about Steinberg licensing — worth a provenance check now that the SDK is MIT (**U26**) |
| `vst3-bindgen` | 0.3.0 | `MIT OR Apache-2.0` | 2025-11-01 | 10 k | active | Build-time generator; needs SDK headers |
| `vst` | 0.4.0 | `MIT` | **2023-03-15** | 131 k | **abandoned** | VST **2.4**. The VST2 SDK was withdrawn by Steinberg — undistributable for new work. **REJECT** |
| `livi` | 0.7.5 | `MIT` | 2024-11-12 | 41 k | dormant | LV2 host. Needs system **lilv + LV2 C libraries** at build and run time — unbuildable here (§7) |
| `lv2` / `lv2-sys` | 0.6.0 / 2.0.0 | `MIT OR Apache-2.0` | **2020-10-23** | 25 k / 30 k | **abandoned** (6 yr) | Framework for *writing* LV2 plugins, not a host |
| `lilv-sys` | 0.2.1 | `MIT` | **2019-11-09** | 18 k | **abandoned** (7 yr) | FFI to lilv |
| **`nih-plug`** | **not on crates.io** | **ISC** — "Copyright (c) 2022-2024 Robbert van der Helm" | rolling; last commit 2026-09-03 | — | **maintenance mode** | A **framework for writing plugins, not a host** — README: *"NIH-plug is an API-agnostic audio plugin framework."* Not what CINE-OMNI needs. **Its own README documents the trap this document is about:** *"the VST3 bindings used by `nih_export_vst3!()` are licensed under the GPLv3 license… any VST3 plugins built with NIH-plug need to be able to comply with the terms of the GPLv3 license."* Successor: `nice-plug` (codeberg.org/BillyDM/nih-plug), ISC — whether it still routes VST3 through GPLv3 `vst3-sys` is **UNVERIFIED (U27)** |

### 9.4 DSP, loudness, resampling, stretch

| Crate | Latest | Licence | Last release | ≈DL | Maintenance | Note |
|---|---|---|---|---|---|---|
| `ebur128` | 0.1.10 | `MIT` | 2024-10-26 | 912 k | dormant (23 mo) | Pure-Rust port of libebur128. **Measurement only.** The §5.3 oracle. **(built here)** |
| `libebur128-sys` | **does not exist** | — | — | — | — | Confirmed 404. `ebur128` is a port, not a binding — there is no sys crate |
| `bs1770` | 1.0.0 | `Apache-2.0` | **2020-09-02** | 25 k | **abandoned** (6 yr) | BS.1770-4 loudness. Superseded by `ebur128` |
| `rubato` | **5.0.0** | `MIT OR Apache-2.0` | 2026-08-10 | 10.6 M | active | Async/sync SRC, sinc + FFT. Fast major-version churn — pin deliberately. **(built here** at 0.16.2**)** |
| `rustfft` | 6.4.1 | `MIT OR Apache-2.0` | 2025-09-18 | 27.3 M | active | Already a `cine-audio` dependency. **Allocates on planner use — plan outside the audio thread** (§4.1 rule 1) |
| `realfft` | 3.5.0 | `MIT` | 2025-06-12 | 16.2 M | active | Real→complex wrapper; no windowing/STFT framing |
| **`signalsmith-stretch`** | 0.1.3 | `MIT` | 2025-09-18 | 83 k | active | **Time-stretch / pitch-shift, MIT all the way down** — the C++ core's own LICENSE.txt is MIT ("Copyright (c) 2022 Geraint Luff / Signalsmith Audio Ltd."). This materially changes §8.4 |
| `bungee-rs` / `bungee-sys` | 0.1.1 | `MPL-2.0` | 2025-09-28 | 1.4 k / 2.7 k | active (low) | Stretch; MPL-2.0 file-level copyleft, acceptable |
| `soundtouch` / `-ffi` | 0.5.4 / 0.4.1 | `LGPL-2.1` | 2026-03 | 30 k / 27 k | active | **LGPL-2.1** — Rust static linking defeats §6 relinking. Avoid |
| **Rubber Band** | no Rust binding on crates.io | **GPL-2.0-or-later or paid commercial** | — | — | — | **REJECT** (§8.4) |
| `pitch_shift` | 2.1.0 | `MIT` | 2026-04-20 | 14 k | active | Pure-Rust phase vocoder |
| `dasp` | 0.11.0 | `MIT OR Apache-2.0` | **2020-05-29** | 4.9 M | **abandoned** (6 yr) | Umbrella crate has one published version. Do not build on it |
| `fundsp` | 0.23.0 | `MIT OR Apache-2.0` | 2026-01-07 | 199 k | active | Synthesis/combinator DSL — a synthesis toolkit, not a mix engine |
| `hrtf` | 0.9.0 | `MIT` | 2026-08-06 | 641 k | active | Binaural HRTF; consumes HRIR spheres, **not** SOFA |
| `sofar` | 0.3.0 | `MIT OR Apache-2.0` | 2026-03-14 | 26 k | active | SOFA reader **and** renderer, genuinely pure Rust in 0.3.0. Small user base; SOFA convention coverage **UNVERIFIED (U28)** |

### 9.5 Files, time, MIDI, control

| Crate | Latest | Licence | Last release | ≈DL | Maintenance | Note |
|---|---|---|---|---|---|---|
| **`bwavfile`** | 2.0.1 | `MIT` | 2023-06-03 | 29 k | dormant (3.2 yr) | **The only serious BWF crate.** Broadcast-WAV, MBWF and **RF64/BW64**, a `Bext` struct, reader and writer. Directly addresses R8. `chna`/iXML support **UNVERIFIED (U29)** |
| `hound` | 3.5.1 | `Apache-2.0` | 2023-09-25 | 17.8 M | dormant (feature-complete) | **Plain WAV only** — no RF64/BW64, no `bext`, no >4 GiB. Not sufficient for delivery |
| `symphonia` | 0.6.1 | `MPL-2.0` | 2026-08-13 | 12.3 M | active | `cine-media` pins 0.5. **Decode only — no encoders at all** |
| `timecode-coder` | 0.3.0 | `MIT` | 2022-12-04 | 3.8 k | dormant | **`no_std`, LTC encode AND decode, realtime.** The best pure-Rust LTC option |
| `ltc` | 0.2.0 | `BSD-3-Clause` | **2020-04-16** | 15 k | **abandoned** | LTC decode only |
| `libltc-rs` | 0.2.0 | `MIT` | 2025-08-12 | 2.2 k | active (low) | Binds **x42/libltc, which is LGPL-3.0-or-later** (verified by the crate survey). **The crate's MIT says nothing about what it links.** Avoid; `research/10` §B.7.2 already judged reimplementation reasonable |
| `vtc` / `timecode` | 0.1.14 / 0.7.5 | `MIT` | 2025-11 / 2025-10 | 20 k / 57 k | active | SMPTE timecode *arithmetic* (not LTC signal) |
| `midir` | 0.11.0 | `MIT` | 2026-04-18 | 727 k | active | Realtime MIDI I/O. **MIDI 1.0 ports only** — no MIDI 2.0/UMP, no file I/O |
| `wmidi` | 4.0.11 | `MIT` | 2026-04-19 | 141 k | active | **Zero-alloc** MIDI parsing — the right one for the audio thread |
| `midi-msg` | 0.9.0 | `MIT` | 2026-05-08 | 46 k | active | Message model incl. MTC and SysEx; no I/O |
| `rosc` | 0.11.4 | `MIT/Apache-2.0` ⚠ | 2025-03-23 | 527 k | active (low) | OSC codec only; bring your own transport. **(built here)** |

### 9.6 What does not exist in Rust

Searched crates.io and the wider ecosystem; reported as absences rather than
filled in:

* **ADM (ITU-R BS.2076) — none.** No Rust crate parses or writes the ADM XML
  model. `bwavfile` exposes an `ADMAudioID` struct but that is raw chunk-level
  access, not a BS.2076 model. The real implementations are C++ (`ebu/libadm`,
  Apache-2.0) and Python (`ebu/ebu_adm_renderer`, BSD-3-Clause-Clear).
* **AAF — none credible.** No established, independently-authored Rust AAF
  crate. The real options are C (`agfline/LibAAF`), Python (`pyaaf2`), or the
  OpenTimelineIO AAF adapter.
* **Dolby Atmos ADM — none.** `truehdd` decodes TrueHD bitstreams, which is a
  different thing.
* **A permissively-licensed Rust DAW engine — none.** See §9.7.

### 9.7 Meadowlark: read it, do not depend on it

The most-cited open Rust DAW effort is effectively over, and the detail matters
because it is the obvious thing to reach for. Verified: **23 of 25 repositories
in the `MeadowlarkDAW` GitHub org were archived on 2025-09-22.** `Meadowlark`
itself was **relicensed to AGPL-3.0** on 2025-04-18 — which for CINE-OMNI is
the strictest possible copyleft and puts it firmly in TYPE G. `dropseed` is
archived, AGPL-3.0, last substantive commit 2023-05-18, and its crates.io entry
is a `0.0.0` GPL-3.0 placeholder. Meadowlark's `audio-graph` is MIT but was last
touched 2022-08-29 and **is not on crates.io**.

⚠ **The `audio_graph` crate that *is* on crates.io is not Meadowlark's.** It is
v0.1.0, published 2026-02-25, **41 total downloads**, with a repository field of
`github.com/tu-usuario/crates-daw` — a literal placeholder ("tu-usuario" is
Spanish for "your-username"). **Do not depend on it.**

The author's live work moved to Codeberg as small permissive crates (`creek`,
`symphonium`, `fixed-resample`, `Yarrow`). The reading for this document:
**nobody has a permissively-licensed Rust DAW engine, which is why §11 puts the
engine at TYPE A.** Meadowlark's design notes are worth reading; its code is
AGPL and must not be.

### 9.8 A data-quality warning that belongs in the dependency policy

The crate survey surfaced several large, brand-new crate *families* published in
bulk during 2026 with negligible downloads, single-author placeholder
repositories, and implausible scale claims — named examples: `oximedia-*` (115
crates), `oxideav-*`, `revelo-*`, `st12-1` (59 downloads), `rdd29` (66
downloads), `audio_graph` (41 downloads). One of them, `oximedia`, claims to be
a pure-Rust reconstruction of **both FFmpeg and OpenCV** — 115 crates, ~3.05 M
lines, 104,693 passing tests — **in 12 commits**, and its own release notes
describe a *"fabricated-success elimination sweep."*

Exactly the crates a search for "Rust AAF", "Rust ADM" or "Rust Atmos" surfaces
first. Under this project's own rule — *never make a capability claim the build
cannot back* — these are **UNVETTED: do not depend on**, and `deny.toml` should
grow a minimum-maturity gate (downloads, age, a real repository) rather than
relying on a reviewer noticing.

## 10. Licence and dependency table

Every row marked **verified** had its primary source fetched in this session and
the URL is given. Nothing here is from memory. CINE-OMNI is Apache-2.0, so the
governing rule of `01-LICENSING-SPINE.md` §2 applies: **no GPL source may be
copied into, statically linked into, or dynamically linked into CINE-OMNI.**

### 10.1 Plugin formats and audio SDKs

| Project | Licence | Verified from | Verdict for Apache-2.0 |
|---|---|---|---|
| **CLAP** | MIT — "Copyright (c) 2021 Alexandre BIQUE" | `raw.githubusercontent.com/free-audio/clap/main/LICENSE` | **verified — use.** Primary plugin format |
| **LV2** (spec + headers) | ISC — "Copyright 2006-2012 Steve Harris, David Robillard…" | `raw.githubusercontent.com/lv2/lv2/master/COPYING` | **verified — use** |
| **lilv** (LV2 host library) | ISC | `github.com/lv2/lilv` | **verified — usable**; C library, so a build dependency this machine cannot satisfy (§7) |
| **VST3 SDK** | **MIT** since VST 3.8 (29 Oct 2025). README: *"Licensing under GPLv3 and the Steinberg proprietary license is no longer available."* | `github.com/steinbergmedia/vst3sdk/blob/master/LICENSE.txt`, README, `vst3_pluginterfaces/LICENSE.txt` | **verified — now usable.** Reverses `01-LICENSING-SPINE.md` §5 |
| **ASIO SDK 2.3** | **Dual: GPLv3 or proprietary Steinberg agreement.** README: *"Use of the proprietary ASIO SDK license… Adopt the General Public License (GPL) Version 3 for full open-source integration."* | `github.com/audiosdk/asio` | **verified — REJECT in-process.** GPLv3 arm incompatible; proprietary arm needs a signed agreement. **LEGAL REVIEW REQUIRED** |
| **AAX SDK** | Proprietary, Avid; registration/agreement gated | not fetched — **UNVERIFIED (U7)** | reject on architecture grounds regardless |
| **LADSPA** | not fetched — **UNVERIFIED (U16)** | — | superseded by LV2; no reason to add |

### 10.2 Audio servers and drivers

| Project | Licence | Verified from | Verdict |
|---|---|---|---|
| **PipeWire** | MIT (Expat) | `raw.githubusercontent.com/PipeWire/pipewire/master/COPYING` | **verified — use.** Component-level exceptions not enumerated (**UNVERIFIED, U17**) |
| **JACK2 client API** (`jack.h`) | **LGPL-2.1-or-later** — header states *"under the terms of the GNU Lesser General Public License… either version 2.1 of the License, or (at your option) any later version"* | `raw.githubusercontent.com/jackaudio/jack2/develop/common/jack/jack.h` | **verified — conditionally usable.** LGPL §6 relink obligation; Rust static linking defeats it, so **runtime `dlopen` only** |
| **JACK2 server** (`JackServer.cpp`) | **GPL-2.0-or-later** | `raw.githubusercontent.com/jackaudio/jack2/develop/common/JackServer.cpp` | **verified — never linked.** Separate process only (which is what a JACK server always is) |
| **JACK1** | not fetched — **UNVERIFIED (U18)** | — | expected same split; verify before relying |
| **ALSA (alsa-lib)** | not fetched — **UNVERIFIED (U19)**; widely stated LGPL-2.1 | — | verify before shipping a Linux backend |
| **PulseAudio (libpulse)** | not fetched — **UNVERIFIED (U20)** | — | PipeWire serves the Pulse API here anyway (§7) |
| **CoreAudio / WASAPI** | OS-provided system APIs, no separate SDK licence | — | use |

### 10.3 GPL DAWs — study only

| Project | Licence | Verified from | Consequence |
|---|---|---|---|
| **Ardour** | **GPL v2.0 or later** | `raw.githubusercontent.com/Ardour/ardour/master/COPYING` | **Cannot be linked, cannot be copied, cannot be statically or dynamically bound.** TYPE G only: study behaviour and architecture, never transcribe source. Its `COPYING` also carries the *Plugin Clarification* quoted in §6.3, which is useful as a cited industry position |
| **Pro Tools, Nuendo, Fairlight, REAPER** | proprietary | — | TYPE E (file interchange) only. REAPER's `.RPP` is plain text and Fairlight is inside DaVinci Resolve; neither changes the analysis |

### 10.4 Immersive / ADM

| Project | Licence | Verified from | Verdict |
|---|---|---|---|
| **`ebu/libadm`** | **Apache-2.0** | `raw.githubusercontent.com/ebu/libadm/master/LICENSE` | **verified — compatible.** Same licence as CINE-OMNI |
| **`ebu/libbw64`** | **Apache-2.0** | `raw.githubusercontent.com/ebu/libbw64/master/LICENSE` | **verified — compatible** |
| **`ebu/ebu_adm_renderer` (EAR)** | **BSD-3-Clause-Clear** | `github.com/ebu/ebu_adm_renderer` | **verified — compatible.** Python; reference implementation of ITU-R BS.2127 |
| **Dolby Atmos** | proprietary | — | detect and interoperate via ADM; never claim |

### 10.5 DSP libraries

| Project | Licence | Verified from | Verdict |
|---|---|---|---|
| **`libebur128`** (C) | MIT — "Copyright (c) 2011 Jan Kokemüller" | `raw.githubusercontent.com/jiixyj/libebur128/master/COPYING` | **verified**; not needed — the Rust port is used instead |
| **`ebur128`** (Rust) | MIT, v0.1.10, last release 2024-10-26, 912,216 downloads, `github.com/sdroege/ebur128` | crates.io API | **verified — adopt as a dev-dependency oracle** (§5.3) |
| **Rubber Band** | **GPL-2.0-or-later, or a paid commercial licence.** *"If you wish to distribute code using Rubber Band Library under terms other than those of the GNU General Public License, you must obtain a commercial licence from us before doing so."* | `github.com/breakfastquay/rubberband` | **verified — REJECT.** No LGPL arm exists |
| **`nih-plug`** | **ISC — "Copyright (c) 2022-2024 Robbert van der Helm"** | `raw.githubusercontent.com/robbert-vdh/nih-plug/master/LICENSE` | **verified — compatible**, but it is a framework for *writing* plugins, not a host. Not what CINE-OMNI needs |

### 10.6 Standards documents

| Standard | Availability | Verified |
|---|---|---|
| **EBU R 128** v5.0, November 2023 | free PDF from `tech.ebu.ch/publications/r128`; target **−23.0 LUFS** | verified (target string); true-peak ceiling **UNVERIFIED, U10** |
| **ITU-R BS.1770** | **BS.1770-5 (11/2023) is in force**; BS.1770-4 (10/2015) superseded | verified from `itu.int/rec/R-REC-BS.1770/en`. Download terms **UNVERIFIED, U21** |
| **ATSC A/85** | current version **A/85:2026-07**, approved 8 July 2026, free from `atsc.org`. Specifies measuring the **Anchor Element** (usually dialogue) | verified |
| **EBU Tech 3341 / 3342** | referenced from the R 128 page as the metering and loudness-range companions | partially verified; contents **UNVERIFIED, U22** |
| **ITU-R BS.2076 (ADM), BS.2127 (ADM renderer), EBU Tech 3285 (BWF)** | referenced by the EBU libraries above | **UNVERIFIED, U23** — not fetched |
| **SMPTE ST 12-1 (timecode)** | **UNVERIFIED, U24** | — |

---

## 11. Classification (TYPE A–H)

Taxonomy from `01-LICENSING-SPINE.md`: **A** native, **B** library, **C** engine
behind a process boundary, **D** external application over a protocol,
**E** file interchange, **F** fork, **G** inspired reimplementation,
**H** reject.

### 11.1 Engine and infrastructure

| Element | Type | Rationale |
|---|---|---|
| Real-time audio engine (`cine-audio-engine`) | **A** | No permissive Rust engine exists at the required level; it is the product's spine and must be owned |
| Lock-free hand-off (`rtrb` 0.4, `triple_buffer` 9.0, `basedrop` 0.1.3) | **B** | Small, permissive, actively maintained, and all three compile here (F9). Reimplementing lock-free primitives is how memory-ordering bugs are born. Note `triple_buffer` is **MPL-2.0** (file-level copyleft, already allowed by `deny.toml`) and holds only the latest value — right for the `RenderGraph` swap and for meters, wrong for sample streams |
| RT-safe disk streaming (`creek` 1.2.3, MIT/Apache-2.0) | **B** | Solves the §4.3 reader-thread problem directly. Low activity; the one surviving useful Meadowlark crate |
| Allocation assertion in the callback | **A** | `assert_no_alloc` is **abandoned (2021)** and its global-allocator override conflicts with other tooling. Write a small shim; the technique is sound, the crate is not |
| Device I/O abstraction | **A** over **B** | Own the trait (`AudioDevice`), wrap `cpal` behind it. `cpal` cannot build here (F8), so the trait must exist before the backend, exactly as `VideoDecoder` does in `cine-media` |
| PipeWire backend (Linux) | **B** | MIT, verified, running on this machine. First-class Linux target |
| ALSA backend (Linux fallback) | **B** | Via `cpal`; licence **UNVERIFIED (U19)** and unbuildable here |
| JACK backend | **B**, runtime `dlopen` only | LGPL-2.1 client library, verified. Static linking would defeat LGPL §6. **Load dynamically, never link** |
| CoreAudio (macOS), WASAPI (Windows) | **B** | OS APIs |
| **ASIO (Windows)** | **H — reject** | GPLv3-or-proprietary, verified §10.1. **LEGAL REVIEW REQUIRED** if ever revisited. WASAPI exclusive mode is the answer |
| Ardour | **G** | GPLv2+. Study its architecture — its plugin clarification, its session model, its automation modes — and reimplement. Never read its source into ours |
| Meadowlark / `dropseed` | **G** | **AGPL-3.0**, archived 2025-09-22 (§9.7). The strictest copyleft in the register. Read the design notes; the code must never be touched |
| Pro Tools / Nuendo / Fairlight / REAPER | **E** | Interchange only |

### 11.2 Plugin hosting

| Element | Type | Rationale |
|---|---|---|
| CLAP host | **A** over **B** (`clack-host`) | MIT format, MIT bindings, builds here. Primary format |
| VST3 host | **B** (`vst3` 0.3.0, `coupler-rs`, MIT OR Apache-2.0), deferred | SDK is MIT since 3.8; this binding is permissive and active. **Do NOT use `vst3-sys` — it is GPL-3.0-or-later** and is not relicensed by the SDK's change (§9.3) |
| LV2 host | **B** (`lilv`, ISC, via `livi` MIT) or **A** | Linux relevance. Cannot be built on this machine (C library). The pure-Rust `lv2`/`lv2-sys`/`lilv-sys` crates are all **abandoned since 2019–2020** and are not an option |
| AAX | **H — reject** | Proprietary, Avid-gated, exists to run inside Pro Tools |
| Out-of-process plugin sandbox | **A** | Non-negotiable for a tool that holds a mix. Also the cleanest answer to any residual plugin-licence question |
| Ship plugin binaries | **H — reject** | Load what the user installed; never bundle (§6.3) |

### 11.3 DSP and signal

| Element | Type | Rationale |
|---|---|---|
| Buffers, biquads, dynamics, limiter, STFT, VBAP, downmix | **A — already built** | Keep. This is the good half of `cine-audio` |
| Integrated LUFS | **A — already built and verified correct** (F3) | Keep |
| True peak | **A — rebuild** (D2/F4) | BS.1770 Annex 2 filter, not the generic resampler |
| M / S / LRA / streaming meters | **A** | Required for delivery and for a real meter (§5.1) |
| `ebur128` crate | **B, dev-dependency only** | MIT. The oracle that produced F3/F4, in CI. Not in the shipped path |
| Streaming resampler | **A**, extending `cine-audio` | `research/10` §B.8.3 already specifies this as an addition, not a fork |
| High-quality offline SRC | **B** (`rubato` 5.0, MIT/Apache-2.0, active) *or* **A** | Decide on measured quality against the existing 16-tap sinc; do not adopt on reputation. Pin deliberately — `rubato` churns major versions fast |
| Reverb (algorithmic + convolution) | **A** | `dsp.rs` has the FFT machinery; partitioned convolution is a known, bounded problem |
| Time-stretch / pitch-shift | **B** (`signalsmith-stretch`, **MIT at every layer**), **A** as fallback over the existing WOLA STFT | **Rubber Band is H — reject** (GPLv2+ or paid, no LGPL arm). **SoundTouch is LGPL-2.1 — avoid** (static linking defeats §6). Bungee is MPL-2.0, acceptable |
| Dialogue isolation | **A — already built** | `isolate.rs`. Needs a streaming variant to be usable as an insert |

### 11.4 Time, sync and interchange

| Element | Type | Rationale |
|---|---|---|
| Exact sample-base time in `cine-audio` | **A** | §3. Promote `cine-media::Timestamp` to `ucg-core`; do not invent a third type |
| LTC decode/encode | **A**, in `cine-media` | `research/10` §B.7.2 already assigns it there. `timecode-coder` (MIT, `no_std`, encode **and** decode) is the reference to read; **avoid `libltc-rs` — it binds x42/libltc, which is LGPL-3.0-or-later** |
| BWF `bext` / iXML **write** | **B** (`bwavfile` 2.0.1, MIT) or **A** | Cheapest high-value item on the list (§8.3). `hound` is **not** sufficient — plain WAV only |
| RF64 / BW64 | **B** (`bwavfile`) or **A** | Required for immersive masters; `bwavfile` covers it |
| ADM (ITU-R BS.2076) | **A**, informed by **B** | `libadm`/`libbw64` are Apache-2.0 — usable as a reference *or* linkable. No Rust implementation exists (§9) |
| ADM renderer (BS.2127) | **G** from EAR (BSD-3-Clause-Clear) | Permissive, so this is stronger than TYPE G: it may be ported directly with attribution |
| Dolby Atmos | **E** at most | Interoperate through ADM; never claim the name (§8.1) |
| AAF import | **A** or **B** | The conform path from picture. Licence and Rust availability **UNVERIFIED (U14)** |
| OTIO | **E** | Consistent with the rest of the workspace |
| MIDI | **B** — `midir` 0.11 (MIT, I/O) + `wmidi` (MIT, **zero-alloc**, for the audio thread) | Music editing. MIDI 1.0 only; no MIDI 2.0/UMP and no file I/O in either |
| OSC / control surfaces | **B** (`rosc`, builds here) + **A** protocol layer | Mackie/HUI are documented protocols; EUCON is proprietary — **H** |

---

## 12. Recommendations, and the alternatives rejected

Ordered by consequence, not effort.

### R1 — Correct the VST3/ASIO finding in the licensing spine *today*

`01-LICENSING-SPINE.md` §5 and `02-TARGET-ARCHITECTURE.md` §4 both record VST3
as a GPLv3-or-proprietary trap and ASIO as a separate trap. **Verified today:
VST3 is MIT (VST 3.8, 29 October 2025) and ASIO is the one that went GPLv3.**
This costs nothing to fix and is the highest-value correction available, because
a plugin-strategy decision made from the old premise would reject the format the
commercial ecosystem actually uses.

*Rejected alternative:* leaving the old text with a footnote. A licensing spine
that contains a known-false statement is worse than one with a gap; the whole
point of §42 is that integration decisions are made from it.

### R2 — Split the crate: `cine-audio` stays pure, `cine-audio-engine` is new

`cine-audio` keeps buffers, DSP, panning, loudness, the `Session` model and the
offline renderer, and gains only streaming variants of what it already has. All
device I/O, threading, plugin hosting, transport, recording and metering live in
a new crate that depends on it (§2.2).

*Rejected alternative 1: grow `cine-audio` into a DAW.* It would make one crate
simultaneously a deterministic pure library and a device-bound real-time
application. Those have opposite test models; the result is testable as neither,
and the "no allocation in the callback" rule would have no boundary to be
enforced at.

*Rejected alternative 2: a feature flag.* Same compilation unit, so the same
problem, with the added cost that the pure path stops being tested in the
configuration it ships in.

*Rejected alternative 3: build the engine inside `cine-pipeline`.* Reactors are
pure `(&ChangeSet, &Graph) -> Vec<Plan>` by convention (MODULE-CONVENTIONS rule
6). An audio device does not belong there.

### R3 — The audio thread never touches the graph, and the type system says so

Compile a `Session` into an immutable, pre-allocated `RenderGraph` on the main
thread; publish by atomic pointer swap; free the old one on a collector thread
(`basedrop`); move every value in the other direction through a ring buffer.
Parameter changes write to lock-free slots and do **not** recompile. One
transaction per gesture, not per sample (§4.4).

*Rejected alternative 1: `Arc<Graph>` snapshot per block.* `snapshot()` was
measured at 363 ms at 56 k nodes — 34,000 audio deadlines.

*Rejected alternative 2: `try_lock` with a silent fallback.* The fallback *is*
the dropout. It converts a design flaw into an intermittent, unreproducible
user-facing defect, which is strictly worse than a design that cannot fail.

*Rejected alternative 3: metering as graph attributes.* 128 tracks at 60 Hz is
7,680 values per second into a transactional store with a reactor cascade behind
it. This is the bulk-data boundary of `02-TARGET-ARCHITECTURE.md` §3.4.

### R4 — Fix the two real defects in shipped code before adding anything

**D1 (F5): one-sample clip joins.** `render_track` rounds start and duration
independently, so butt-spliced clips can gap or overlap by a sample. In dialogue
editing that is an audible click at every edit. Derive the end from the next
boundary, or hold clip ranges in samples. Small, local, and it does not have to
wait for the exact-time migration.

**D2 (F4): true peak reads 0.3–0.5 dB high on programme material.** Verified
against `ebur128` on noise and 5.1 material; the existing unit test passes
because a 1 kHz sine happens to agree exactly. Implement the BS.1770 Annex 2
4× filter. Until it agrees, do not present the number as a compliance figure.

*Rejected alternative:* replacing the whole loudness module with `ebur128`.
`lufs_integrated()` already agrees to **0.000 LU** across five signals including
5.1 (F3); replacing correct code with a dependency loses the streaming variant
the meter needs and puts a third-party crate inside the artefact a client audits.
Use `ebur128` as a **dev-dependency oracle in CI** instead — the same pattern
`research/10` R3 chose for OCIO, for the same reason.

### R5 — Adopt CLAP first, VST3 second, LV2 third — and never ship a plugin binary

CLAP leads on buildability, not licence: `clack-host 0.1.1` compiles here with
no C toolchain (F9), and CLAP writes its threading contract into the header,
which is the discipline R3 requires anyway. VST3 is now equally free and is
where the commercial tools are. LV2 matters on Linux; its *host libraries*
(`lilv`, ISC — verified) are C, so they cannot be built on this machine.

Load only what the user has installed. Bundling plugin binaries would import
every plugin's licence into CINE-OMNI's distribution for no benefit.

*Rejected alternative 1: VST3 first.* It is now legally fine, but the binding
work is larger and none of it can be exercised on this machine.
*Rejected alternative 2: an in-house-only effects set.* That is the current
state, and it is why `Insert` is a five-variant enum. A mix stage that cannot
load the mixer's own tools will not be used.
*Rejected alternative 3: in-process plugin hosting with no sandbox.* One bad
plugin takes the user's mix down with it.

### R6 — Reject ASIO; ship WASAPI on Windows and say why

The GPLv3 arm cannot be linked into an Apache-2.0 binary; the proprietary arm
requires signing a Steinberg agreement, which `research/10` R10 argues against
as a class of move. **LEGAL REVIEW REQUIRED** before any ASIO support is
contemplated. Document the gap in the product rather than leaving users to
discover it.

*Rejected alternative: sign the proprietary ASIO agreement pre-emptively.* Same
argument `research/10` R10 makes about Dolby: it trades a diffuse limitation for
concrete, possibly confidential, contractual obligations hostile to a public
repository.

### R7 — Do the exact-time migration once, for the graph and for audio together

Promote `cine-media`'s tested `Timestamp`/`TimeBase` to `ucg-core` (this is
`research/10` R1, still not done), and in the same change give `cine-audio`
sample-base clip ranges, integer source offsets and timestamped automation
lanes. Doing them separately migrates the clip model twice.

*Rejected alternative: a third time type local to `cine-audio`.* That is how the
workspace ended up with two already (§3.1).

### R8 — Build BWF `bext` write before anything else in interchange

An exported WAV with no timecode is unusable in a dual-system workflow no matter
how good the mix is. `TimeReference` is a 64-bit sample count — exact, rate-
independent, immune to drop-frame — and `research/10` §B.7.4 already names it
the single most valuable field in the domain. `bwavfile` (v2.0.1, MIT) already
implements `bext` plus RF64/BW64, so this is a read-or-adopt decision rather
than a build-from-nothing one; either way it unblocks round-tripping with every
other tool.

*Rejected alternative: `hound`.* It is the obvious WAV crate and it is **plain
WAV only** — no `bext`, no RF64, no files over 4 GiB. It cannot produce a
deliverable.

### R9 — Take ADM/BW64 as the immersive delivery format, and rename `Atmos714`

`libadm` and `libbw64` are **Apache-2.0** and the EBU ADM Renderer is
**BSD-3-Clause-Clear** — all verified. Object-based delivery is therefore fully
open to CINE-OMNI, which Dolby Atmos is not. Build ADM export natively; treat
Atmos as detect-and-interoperate. Rename `ChannelLayout::Atmos714` to
`Surround714` so the enum cannot leak a claim into the UI, exactly as
`research/10` R7 required for the ACES labels.

*Rejected alternative: licensing Dolby Atmos tooling.* Same reasoning as R6 and
as `research/10` R10 on Dolby Vision.

### R10 — Take the dialogue-gated loudness measurement; it is nearly free

ATSC A/85 measures the Anchor Element — usually dialogue — not the full mix.
Generic meters must guess what dialogue is. CINE-OMNI does not: `Stem::Dialogue`
exists, `render_stems` produces the stem, and the graph already ties dialogue
clips to screenplay lines. Anchor-element loudness is `lufs_integrated()` on the
dialogue stem. This is a real capability, differentiated, and mostly already
written.

### R11 — Add a maturity gate to `deny.toml`, not just a licence gate

The crate survey (§9.8) found bulk-published 2026 crate families with
placeholder repositories and implausible claims sitting exactly where a search
for "Rust AAF", "Rust ADM" or "Rust Atmos" lands. One claims to reimplement
FFmpeg *and* OpenCV — 115 crates, 3.05 M lines, 104,693 passing tests — in
twelve commits, and its own notes mention a *"fabricated-success elimination
sweep."* `deny.toml` currently gates licences and advisories, both of which
these crates pass. Add a minimum-maturity gate (downloads, age, a resolvable
repository) so the policy catches this class rather than a reviewer having to.

*Rejected alternative: rely on review.* The whole point of `01-LICENSING-SPINE`
§6 item 5 is that a policy regression should fail the build rather than reach a
release. The same argument applies to a provenance regression.

### R12 — Claim nothing that has not been measured

Following `research/10` §A.6.3 and `PHASE-2-MEDIA-ENGINE.md` §16, the strings
that are currently defensible are: *"loudness measured per ITU-R BS.1770"*,
*"7.1.4 channel-based immersive mixing with VBAP panning"*, *"offline stem
rendering"*. The strings that are **not**: *"EBU R 128 certified"* (no Tech 3341
conformance run), *"Dolby Atmos"* (no object model), *"true-peak limiting to
spec"* (D2), *"realtime mixing"* (no audio thread exists), and *"DAW"* (§1.2).

Measured performance must be reported as measured: **128 tracks at 7.1.4
renders offline at 0.30× realtime, and a single 512-frame block of a 48-track
stereo session takes 114.6 % of its own deadline** (F2). That is not a playback
engine and must not be described as one.

---

## 13. UNVERIFIED register

Per `docs/blueprint/README.md`: named rather than guessed. Each row says why it
is open and what would close it.

### 13.1 Open

| # | Claim / gap | Why unverified | What would settle it |
|---|---|---|---|
| U3 | Device latency, driver behaviour, multichannel and immersive monitoring | This machine has one consumer stereo HDA codec and no pro interface (§7) | Re-measure on a machine with a professional multichannel interface |
| U4 | Plugin hosting actually working, as opposed to the host bindings compiling | No CLAP, LV2, VST3 or LADSPA plugin is installed here (§7) | Load a real plugin and render a null test against a bypassed chain |
| U5 | Whether the 0.3–0.5 dB true-peak discrepancy (F4) is `cine-audio` being pessimistic or `ebur128` being optimistic | Neither was compared against a BS.1770 Annex 2 reference with published conformance vectors | Run the EBU Tech 3341 test signals against both |
| U7 | AAX SDK terms | Avid gates access behind registration; not fetched | Only relevant if Pro Tools plugin hosting is ever pursued. It should not be |
| U10 | EBU R 128's true-peak ceiling and tolerance band | The R 128 PDF itself was not read; only the publications page | Read R 128 v5.0 before any number appears in the product |
| U11 | What changed between BS.1770-4 and BS.1770-5 | Not read. **`cine-audio` documents itself as -4; -5 has been in force since Nov 2023** | Read BS.1770-5. The measured agreement with `ebur128` (F3) is evidence of correctness, not of currency |
| U12 | Streaming-platform delivery targets (e.g. Netflix −27 LKFS dialogue-gated) | Vendor specifications change and were not fetched | Fetch the current spec at the time a delivery preset is built. It belongs in graph data, never in code |
| U13 | Whether the **Dolby Atmos Master ADM BWF** profile is publicly specified, and on what terms | Not fetched | Do not claim Atmos-compatible ADM output until read |
| U14 | AAF specification and SDK licence terms | Not fetched. **No credible Rust AAF crate exists** (§9.6) | Read the AMWA/AAF Association terms before choosing between LibAAF (C), pyaaf2 (Python) and the OTIO adapter |
| U16 | LADSPA licence | Not fetched | Low priority — superseded by LV2; no reason to add it |
| U17 | PipeWire component-level licence exceptions | Only the top-level `COPYING` (MIT) was fetched | Enumerate per-module licences before shipping a PipeWire backend |
| U18 | JACK1 licence split | Only JACK2 was verified (LGPL-2.1 client, GPL-2.0 server) | Fetch `jackaudio/jack1` COPYING files. Expected to match |
| U19 | ALSA (alsa-lib) licence | Not fetched; the Rust `alsa` crate declares `Apache-2.0/MIT` but that is the *binding*, not the library | Fetch alsa-lib's COPYING before shipping a Linux ALSA backend |
| U20 | libpulse licence (library vs daemon) | Not fetched | Low priority — PipeWire serves the Pulse API here |
| U21 | Whether ITU-R BS.1770-5's PDF is freely downloadable | The ITU page listed versions but not access terms | Attempt the download |
| U22 | EBU Tech 3341 / 3342 contents (meter ballistics, LRA algorithm, conformance signals) | Not fetched | Required before any "EBU Mode" claim |
| U23 | ITU-R BS.2076 (ADM), BS.2127 (ADM renderer), EBU Tech 3285 (BWF) contents | Not fetched | Required before implementing ADM or `bext` from spec rather than from a reference implementation |
| U24 | SMPTE ST 12-1 availability and access terms | Not fetched | Required if timecode is implemented from spec |
| U26 | Whether `coupler-rs/vst3-rs` (`vst3` 0.3.0, MIT OR Apache-2.0) has any residual Steinberg-licence exposure | Its README says nothing about Steinberg licensing at all — which is itself the risk. The SDK is now MIT (§6.1), which should resolve it, but the crate predates the change | Read the crate's generation process and its relationship to the SDK headers. **LEGAL REVIEW REQUIRED** before shipping VST3 hosting |
| U27 | Whether `nice-plug` (the `nih-plug` successor) still routes VST3 export through GPL-3.0 `vst3-sys` | Not checked | Only matters if CINE-OMNI ever *writes* plugins. It does not today |
| U28 | `sofar`'s actual SOFA convention coverage | Not documented; small user base | Test against real SOFA files before relying on it for binaural |
| U29 | `bwavfile`'s `chna` and iXML support | Not documented either way on docs.rs | Read the source. Decides whether R8 is adopt-or-extend |
| U30 | Whether an LGPL-2.1 runtime-`dlopen` JACK backend actually discharges the §6 relink obligation in a Rust binary | Engineering judgement, not verified law | **LEGAL REVIEW REQUIRED** before a JACK backend ships |

### 13.2 Resolved during this document

| # | Was | Now |
|---|---|---|
| U1 | "Every licence in §6 and §9" | **Resolved** — all primary sources fetched; §10 gives the URLs |
| U2 | "crates.io metadata for every crate" | **Resolved** — §9 is verified against the crates.io API |
| U6 | "128-track 7.1.4 throughput and single-block latency" | **Resolved** — measured; F2. 0.30× realtime and 114.6 % of the block budget |
| U8 | "lilv / serd / sord / sratom / livi licences" | **Partly resolved** — `lilv` is **ISC** (verified); `livi` is MIT but dormant and needs system lilv. `serd`/`sord`/`sratom` individually **still unverified** |
| U9 | "`assert_no_alloc` status" | **Resolved** — BSD-1-Clause, last release **2021-08-03**, abandoned. Write our own shim |
| U15 | "Whether a permissive time-stretch library exists" | **Resolved** — **yes**: Signalsmith Stretch is MIT at every layer (§8.4). This changed the recommendation |
| U25 | "MIDI crate availability/licence" | **Resolved** — `midir` 0.11 (MIT, active), `wmidi` (MIT, zero-alloc) |

### 13.3 Standing risks that are not "unverified" but are easy to forget

* **This document's licence findings have a date.** VST3 went from
  GPLv3-or-proprietary to MIT in October 2025 and this blueprint had not caught
  up. Licences change; re-verify before each release, and make `cargo deny` the
  mechanism rather than memory.
* **A crate's declared licence says nothing about what it links.** `jack` is
  MIT and links LGPL-2.1 libjack. `libltc-rs` is MIT and links LGPL-3.0 libltc.
  `soundtouch` declares LGPL-2.1 honestly; others do not make the transitive
  obligation visible at all. The dependency inventory required by
  `01-LICENSING-SPINE.md` §6 must record *linked native libraries*, not only
  crates.
* **Patent exposure is a separate axis from copyright**, exactly as
  `PHASE-2-MEDIA-ENGINE.md` §13 records for H.264/HEVC. Audio codecs used for
  delivery carry their own position. Not analysed here.

---

## 14. What this document does not claim

* **Not a design.** It is a decision record. `cine-audio-engine` does not exist
  and no line of it has been written.
* **Not a conformance result.** The loudness agreement in F3 is a cross-check
  against one other implementation on five synthetic signals — not an EBU Tech
  3341 conformance run.
* **Not a hardware assessment.** Every latency, monitoring and plugin-hosting
  question is unverifiable on this machine (§7, U3, U4) and is marked so.
* **Not legal advice.** Per `01-LICENSING-SPINE.md`'s standing header, this is
  an engineering reading of publicly stated terms. Everything flagged **LEGAL
  REVIEW REQUIRED** must be confirmed by counsel before distribution.

