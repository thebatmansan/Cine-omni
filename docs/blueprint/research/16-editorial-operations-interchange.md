# CINE-OMNI Industry Blueprint — 16: Editorial Operations & Interchange

**Status:** research, Phase 3 input
**Date of research:** 2026-09-08
**Scope:** (A) the exact semantics of professional editing operations, and
(B) interchange formats — OTIO, EDL, AAF, FCPXML, Premiere XML — verified
rather than assumed.

**Companion documents.** [`09-editorial-media-model.md`](09-editorial-media-model.md)
covers media *identity*, the two-level material/representation split, conform,
relink, and the proposed UCG node mapping.
[`02-editorial-media.md`](02-editorial-media.md) covers the *engine* question
(FFmpeg licensing, hardware decode, proxies, caches). Neither is repeated here.
This document covers the *operations* and the *wire formats*.

> **Evidence discipline.** Claims below are either (a) measured on this machine
> with the command and output reproduced inline, (b) quoted from a primary
> source that was actually fetched on the date above, or (c) marked
> **`UNVERIFIED`** inline and in the register at §D. Licences are stated only
> where the LICENSE file itself was fetched and, where possible, hashed.
>
> **Executable evidence.** The OTIO findings in Part B are not documentation
> readings. Python `opentimelineio` **0.18.1 was installed in this environment
> and used to generate and parse real files**, and `cine-timeline` was compiled
> and run against them in both directions. Every OTIO claim below that is not
> marked `UNVERIFIED` was produced by running code.

---

## 0. Findings summary

The seven findings that change what should be built next.

1. **`cine-timeline`'s OTIO support is asymmetric, and the asymmetry is the
   opposite way round from what the crate documentation implies.** Measured:
   files written by `cine-timeline` **parse cleanly in real OTIO**; files
   written by real OTIO **are rejected or silently corrupted** by
   `cine-timeline`. Four constructs cause a hard parse failure of the whole
   document; four more cause silent, unreported data loss. §B.1.5 has the
   matrix.

2. **The most serious of those is not a missing feature, it is a semantic
   corruption.** An OTIO clip carrying an original plus a proxy, with
   `active_media_reference_key = "proxy"`, comes back out of `cine-timeline`
   as a *single* reference whose key is `DEFAULT_MEDIA` and whose URL is the
   **proxy path**. The file now asserts that the proxy *is* the camera original.
   Measured, §B.1.5. This is a data-integrity bug in shipped code, independent
   of any Phase 3 work.

3. **"OTIO-compatible" has no certifying authority, no conformance suite and no
   published JSON Schema.** The claim is therefore only ever as good as the
   verification the claimant performs and publishes. §B.1.7 defines a five-tier
   bar and a concrete, runnable verification harness. CINE-OMNI currently meets
   **Tier 1 of 5** (writes files real OTIO can read) and fails Tier 2 (reads
   files real OTIO writes).

4. **OTIO's own time type is float-valued and its arithmetic rescales to the
   *higher* of the two rates.** `RationalTime(1, 24) + RationalTime(1, 30)`
   is `RationalTime(2.25, 30)` — measured. `ucg_core::RationalTime` rescales to
   the *left-hand* rate. Mixed-rate arithmetic therefore diverges from OTIO
   silently, in addition to the float-precision defects doc 09 §2.3 measured.
   The fix is `cine-media`'s exact `Timestamp`/`FrameRate`, which already exists
   and is already correct.

5. **There is no usable Rust OTIO binding and there will not be one soon.**
   Both `opentimelineio` and `opentimelineio-sys` on crates.io are single
   versions published in 2020, described by their own author as
   **"(placeholder)"**, never updated since. Verified against the crates.io API.
   The native-Rust reader/writer `cine-timeline` already has is not a
   second-best option — it is the *only* option, and the recommendation is to
   harden it rather than seek a binding. §C.3.

6. **`cine-timeline` should be split, not evolved and not replaced.** Its
   composition math and edit operations are genuinely good and worth keeping;
   its media model, its identity function and its OTIO codec are three separate
   concerns wrongly fused into one crate. §C.1 gives the split.

7. **The interchange implementation order is decided by what survives the round
   trip, not by format popularity.** OTIO first (it is the only lossless one and
   the only Apache-2.0 one), then EDL (trivial, and the universal fallback),
   then FCPXML, and AAF last or never-in-house. §B.6.

8. **Exporting AAF through OTIO does not work, and this was measured rather than
   assumed.** `otio-aaf-adapter` refuses to write a clip that does not already
   carry an AAF MobID, and refuses to write a transition that does not already
   carry AAF-native `OperationGroup` metadata. It is an **AAF → OTIO → AAF**
   tool. Any plan of the form "we will export AAF via the OTIO adapter" is
   unsound. §B.3.2.

9. **FCPXML is at version 1.14, not 1.11, and it evolves purely additively** —
   14 elements added between 1.8 and 1.14, **none removed**, with `version`
   declared `#FIXED` in each DTD. That combination means an implementer should
   target the newest DTD and be permissive. Meanwhile the OTIO FCPXML adapter
   still emits **v1.8** and leaves `uid` empty, so it cannot be relied on. §B.4.

10. **Two AAF-adjacent licence facts that change what may be read.** `pyaaf2` is
    **MIT** (LICENSE fetched) — permissive, and safe to study or subprocess.
    **`LibAAF` is GPL-2.0** (LICENSE fetched) — doc 09 cites it as a source of
    object-model facts, which is fine for documentation, but **no LibAAF source
    may be copied**, and anyone about to write AAF code in CINE-OMNI should not
    read it, to keep the clean-room argument intact. §B.3.1.

---

## Part A — Editing operations, exactly

### A.0 Why this section is written as invariants

Most timeline bugs are not wrong arithmetic. They are an operation that is
*locally* correct and *globally* wrong: it fixes the clip under the cursor and
leaves a neighbour, a transition, a track duration or a linked audio item in an
inconsistent state. So each operation below is specified by **what it must
preserve**, not only by what it changes.

Three invariants apply to every operation on a sequential track:

* **I1 — No gaps except explicit `Gap` items.** A track is a contiguous
  sequence. "Empty" is a first-class item, never an absence. `cine-timeline`
  gets this right: `Track::range_of_child` derives every start position by
  summing predecessors, so a hole is structurally impossible.
* **I2 — Transitions consume no track time.** A transition's duration is
  `in_offset + out_offset`, but it contributes zero to the track's duration and
  overlaps the cut it sits on. Both `Track::duration` and `range_of_child`
  implement this correctly.
* **I3 — Every operation is one atomic transaction.** A trim that touches three
  clips either applies to all three or to none. `edit.rs::commit` implements
  exactly this: it mutates a clone, validates, and only then swaps.

### A.1 The four time coordinate systems an operation must not confuse

Restating doc 09 §2.1 in operational terms, because every semantic below is
stated in one of these and mixing them is the commonest defect:

| Name | Zero point | Used by |
|---|---|---|
| **Source time** | the material's own first frame | `source_range`, slip, match-frame |
| **Track/record time** | the sequence's first frame | clip position, insert point, in/out marks |
| **Item-local time** | the clip's own first *played* frame | markers on a clip, effect keyframes |
| **Timecode** | an arbitrary label origin | display, EDL/AAF conform |

`Clip::media_time_at` converts track → source. It is currently a pure offset
(`tr.start_time + (track_time - track_start)`), which is correct **only while
there are no retimes**. §A.6.

### A.2 Handles — the concept every operation depends on

A **handle** is source material that exists but is not currently played:

```
available_range:  |<---------------- 200 frames of media ---------------->|
source_range:            |<------ 48 frames played ------>|
                  |<-10->|                                |<---- 142 ---->|
                  head handle                              tail handle
```

* **Head handle** = `source_range.start − available_range.start`
* **Tail handle** = `available_range.end_exclusive − source_range.end_exclusive`

Handles are what make trimming, rolling and transitions possible at all. Three
rules that implementations get wrong:

1. **A clip with no `available_range` has unbounded handles.** `MissingReference`
   and un-probed media have no known extent, so the correct behaviour is to
   permit the trim and record that the bound was unknown — not to refuse, and
   not to assume zero. `edit.rs` gets this right: `tail_bounds` returns
   `hi = f64::INFINITY` when `available_range` is `None`.
2. **A subclip's handles are a policy question, not a fact.** Doc 02 §5 notes a
   subclip made from a duration marker explicitly has *no* handles. Whether a
   trim may exceed the subclip's bounds into the parent material is a stored
   property of the subclip, not a global rule.
3. **Handle availability is representation-dependent and must not be.** If a
   proxy was generated with fewer handles than the original, the *edit's* legal
   trim range must still be computed from the material, or switching to proxies
   silently changes what edits are permitted. This is the §3.4 invariant of doc
   09 applied to the trim bounds.

### A.3 The operations semantics table

Sign convention throughout (matching `edit.rs`): a **positive** delta moves an
edge, a cut or a clip **later** on the track.

| Operation | What changes | Adjacent clips | Track duration | Handles required | Fails when | In `cine-timeline`? |
|---|---|---|---|---|---|---|
| **Insert** (ripple insert, "V" edit) | New item placed at `t`; the item under `t` is split there | Everything at or after `t` shifts **later** by the new item's duration | **+ inserted duration** | None on the target; the *source* clip must have the marked duration available | `t` falls inside a transition; `t` beyond track end (must pad with a gap first) | ✅ `insert` — pads with `Gap`, splits, inserts |
| **Overwrite** ("B" edit) | New item replaces `[t, t+dur)` | Fully covered items **deleted**; partially covered items **trimmed** at the boundary; **nothing moves** | **unchanged** (unless writing past the end, which extends) | None; the overwritten material is discarded, not preserved | Item is a transition; `t` inside a transition | ✅ `overwrite` — drops transitions on covered cuts, splits both ends, drains, inserts |
| **Lift** (delete, leave gap) | `[start, end)` removed | Nothing moves; a `Gap` of identical duration replaces the removed span | **unchanged** | None | Range boundary inside a transition | ✅ `lift` |
| **Extract** (ripple delete) | `[start, end)` removed and the hole **closed** | Everything after `end` shifts **earlier** by the removed duration | **− removed duration** | None | Range boundary inside a transition; orphaned transitions must be healed | ✅ `extract` + `heal_transitions` |
| **Ripple trim** | One edge of one item moves; its duration changes | Everything after it shifts by the applied delta | **± applied delta** | Head trim earlier needs head handle; tail trim later needs tail handle | Clamped, never fails: returns the delta actually applied | ✅ `ripple_trim(index, Edge, delta)` |
| **Roll** (dual-roller trim) | The **cut** between two items moves; left item's tail and right item's head move together | Only the two items either side change; **nothing shifts** | **unchanged** — this is the defining property | Rolling later needs left's **tail** handle *and* right's **head** handle; rolling earlier the reverse | Either side runs out of handle; clamped to `min` of both | ✅ `roll(cut, delta)` — correctly intersects both bounds |
| **Slip** | The clip's **source in/out** move together; content changes, position and duration do not | **Nothing** changes — not position, not duration, not neighbours | **unchanged** | Slipping later needs **tail** handle; earlier needs **head** handle. Both are required for a two-way slip | A gap cannot slip (returns zero); clamped to available range | ✅ `slip(index, delta)` — returns 0 for gaps |
| **Slide** | The clip **moves along the track**; its own content and duration are untouched | Previous neighbour's **tail** and next neighbour's **head** absorb the move | **unchanged** | Sliding later needs prev's tail handle *and* next's head handle | Neighbour is a transition (`NotAnItem`); first item sliding later opens a leading `Gap` instead | ✅ `slide(index, delta)` |
| **Split / blade** | One item becomes two at `t` | Nothing moves; the two halves are contiguous in source | **unchanged** | None — a split creates handles rather than consuming them | `t` inside a transition; `t` outside the track | ✅ `split_at` — markers follow the half containing them; `ucg_id` stays with the first half |
| **Join / heal cut** | Two adjacent items become one | Nothing moves | **unchanged** | Requires the two to be **contiguous in source on the same media** | Different media, or a source-time discontinuity | ✅ `join` — checks `same_media` and range contiguity |
| **Three-point edit** | Computes the fourth mark from three | Depends on insert vs overwrite | Depends | Source must have the computed duration available | Only 2 marks set, or the computed duration exceeds available range | ❌ **absent** — no mark state exists |
| **Four-point edit (fit-to-fill)** | All four marks set; conflict resolved by a computed speed `target_dur / source_dur` | As per insert/overwrite | Depends | — | Requires a retime model, which does not exist | ❌ **absent** |
| **Replace edit** | Clip swapped, alignment preserved at playhead or at in-point | Nothing moves; duration preserved | **unchanged** | New source needs enough handle to cover the existing duration at the chosen alignment | Insufficient source duration | ❌ **absent** |
| **Ripple delete across tracks** | Extract applied to every ripple-enabled track | Sync across tracks must be preserved | − removed | — | Tracks with different content at that range desynchronise if ripple targeting is wrong | ❌ **absent** — all ops are single-track |
| **Linked selection** | Selecting video selects its linked audio | Operations apply to the whole link group atomically | — | — | Link broken, or items no longer aligned | ❌ **absent** — no link concept |

### A.4 The three operations most often got subtly wrong

**Roll vs ripple trim.** These look identical in the UI (drag a cut) and are
completely different. Ripple changes sequence duration; roll does not. The
implementation tell is whether the *right-hand* item's head moves. If it does
not, you have written ripple and labelled it roll. `edit.rs::roll` computes both
`tail_bounds(cut)` and `head_bounds(right)`, intersects them, and applies the
same delta to both — which is correct, including the rate-scaling factor
`rscale` for mixed-rate neighbours.

**Slip vs slide.** Slip moves *content* under a fixed window; slide moves the
*window* over fixed content. The mnemonic that actually helps: slip consumes the
**slipped clip's own** handles; slide consumes the **neighbours'** handles. A
codebase that computes slide bounds from the slid clip is wrong.
`edit.rs::slide` correctly derives its bounds from `prev` and `next`.

**Transitions under destructive edits.** A transition is not an item you can
leave dangling. Three rules, all of which `edit.rs` implements:
* material removed or overwritten *across a cut* takes that cut's transition
  with it (`drop_transitions_on_cuts`);
* after a removal, a transition that no longer sits between two timed items is
  deleted (`heal_transitions`);
* a trim may never shorten an item below the transition offset it must cover
  (`transition_floor`).

This last one is the subtle one and it is genuinely handled: `tail_bounds` uses
`lo = -(duration - transition_floor)`, so a clip carrying a 12-frame dissolve
cannot be trimmed shorter than 12 frames.

### A.5 Transitions and their handle requirement

The rule that governs whether a dissolve is legal at all:

> A transition of `in_offset` before the cut and `out_offset` after it requires
> the **outgoing clip to have at least `out_offset` of tail handle** and the
> **incoming clip to have at least `in_offset` of head handle.**

This is because during the overlap both clips must produce frames
simultaneously, and each is playing material outside its own `source_range`.
This is the single most common reason a dissolve is refused in a real NLE, and
the message users see ("insufficient media for transition") is exactly this
check.

**`cine-timeline` does not implement this check.** `Track::validate` verifies a
transition's offsets do not exceed the *duration* of its neighbours, which is a
different and weaker condition — it prevents structural overlap but not media
exhaustion. A dissolve placed on two clips that each play their entire available
range validates successfully and is unrenderable. This is a real gap and it is
cheap to fix.

Transition types worth modelling, per doc 09's FCPXML/OTIO reading:
`SMPTE_Dissolve` is the only one OTIO names; everything else is a vendor string.
Dip-to-colour, wipes and pushes are all "custom" as far as interchange is
concerned, which is a strong argument for treating the transition *kind* as an
opaque string plus parameters rather than an enum with cases.

### A.6 Retiming — why it must be a function, not a scalar

Doc 09 §4.4 establishes the model requirement (FCPXML's `timeMap`/`timept`,
OTIO's `LinearTimeWarp`/`FreezeFrame`). The **operational** consequences that
belong here:

* **Retiming changes the clip's track duration.** A 48-frame clip at 50 % speed
  occupies 96 frames. So a retime is a *ripple* operation on the track, and its
  interaction with locked/unlocked downstream clips must be decided.
* **Retiming changes handle arithmetic.** After a 2× speed-up, one frame of
  track time consumes two frames of source. Every trim bound in §A.3 must be
  computed in *source* frames through the time function, not in track frames.
  This is precisely why `Clip::media_time_at` being a bare offset is expensive
  to defer: every bound calculation in `edit.rs` currently assumes a 1:1 mapping.
* **Speed ramps are non-linear and must be monotonic-checked.** FCPXML's own
  example includes a negative segment (play forward then backward), so
  monotonicity cannot be *assumed*, only *recorded*. A reverse segment is legal;
  an ambiguous one (two source times for one track time in the same direction)
  is not.
* **The resampling method is authored data.** FCPXML stores `frameSampling` as
  `floor | nearest-neighbor | frame-blending | optical-flow-classic |
  optical-flow`. It changes the pixels, so it belongs in the render cache key.
  Optical flow is a motion-estimation pass, not a resample — it is far too
  expensive for realtime and *must* be cached, which makes it a
  `DerivedFrom`-edged artefact in UCG terms.

### A.7 Multicam, compounds, adjustment layers

These three are all "one identity, many instances" problems and doc 09 §4.2–4.3
settles the data model. The **operational** requirements:

| Feature | Operational requirement |
|---|---|
| **Multicam** | Angle selection **varies over time within one clip** — effectively a nested mini-EDL. Cutting between angles during playback must produce a new angle-selection segment, not a new clip. A **flatten/commit** operation bakes selections into ordinary clips and is documented as one-way in Resolve. Audio and video angle selection are independent (FCPXML's `mc-source/@srcEnable = all\|audio\|video\|none`). |
| **Compound / nested sequence** | Editing inside the compound must dirty every parent render cache — a cross-scope invalidation. Trimming the *instance* must not change the compound. Both are stated almost verbatim by Adobe (doc 09 §1.6). |
| **Adjustment layer** | An item that occupies track time, references no media, and applies its effects to **everything beneath it on lower tracks for its duration**. This is the only construct in the list whose effect is defined by *track ordering* rather than by containment — which makes it the one that a pure containment model cannot express. Neither OTIO nor `cine-timeline` has it. |

### A.8 Audio sync methods, ranked by reliability

`cine-timeline/src/sync.rs` already implements two-stage FFT cross-correlation
of RMS envelopes. Ranking the methods by how much they can be trusted:

| Method | Reliability | Fails when | Produces |
|---|---|---|---|
| **Timecode** (jam-synced) | Highest | Clocks drifted, TC not recorded, different TC origins | An exact frame offset |
| **Waveform correlation** | High when a scratch track exists | No scratch audio; heavy noise; the two recordings share no content | A sub-frame offset + a **confidence score** |
| **Clapper / slate detection** | Good, but manual in practice | No slate; soft slate | A marker pair |
| **In-point / marker** | Always works, never verifies | The operator was wrong | An offset by fiat |

The design point: **a sync result must record its method and its confidence**,
because the downstream decision (auto-accept vs present for review) depends on
which produced it. `sync.rs` computes a correlation peak and therefore *has* a
confidence value; doc 09 §4.3 notes it has nowhere durable to store it.

---

## Part B — Interchange, verified

### B.1 OpenTimelineIO

#### B.1.1 What was actually done here

This environment has no pre-existing OTIO. It was installed, and everything
below was executed:

```
$ python3 --version
Python 3.12.3
$ uv venv otioenv && uv pip install --python otioenv/bin/python opentimelineio
Installed 1 package: opentimelineio==0.18.1
```

`uv` was already on the machine; PyPI is reachable; there is a manylinux wheel
for cp312, so no compiler was needed. **Python OTIO was available and was used.**

#### B.1.2 Licence — verified by hash, not by reading a badge

| Artefact | SHA-256 | Lines |
|---|---|---|
| `opentimelineio-0.18.1.dist-info/licenses/LICENSE.txt` (the installed wheel) | `c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4` | 201 |
| `raw.githubusercontent.com/AcademySoftwareFoundation/OpenTimelineIO/main/LICENSE.txt` | `c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4` | 201 |
| `apache.org/licenses/LICENSE-2.0.txt` (canonical) | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` | 202 |

The wheel and the repository are **byte-identical**. Against the canonical
Apache text, `diff` reports exactly one difference — `0a1 >` — a single leading
blank line in the apache.org copy. There is **no appended exception, no
modification, no "Modified Apache 2.0" text**. The historical "Modified Apache
2.0 License" noted in doc 02 §2.4 is genuinely gone.

`NOTICE.txt` ships alongside and reads, in full:

> OpenTimelineIO
> Copyright Contributors to the OpenTimelineIO project.
> All rights reserved.
> This product includes software developed at:
> Pixar (http://www.pixar.com/).

**Conclusion: OTIO is unambiguously Apache-2.0.** Its source may be read,
copied, modified and redistributed inside CINE-OMNI subject only to the
Apache-2.0 attribution requirements (retain the NOTICE). It is the only
interchange target in this document with no licensing obstacle whatsoever.

#### B.1.3 The real schema versions

Not from documentation — from the installed library:

```python
>>> import opentimelineio as otio
>>> otio.core.type_version_map()
```

29 entries. The full map for 0.18.1:

| Schema | Ver | Schema | Ver | Schema | Ver |
|---|---|---|---|---|---|
| `Adapter` | 1 | `Clip` | **2** | `Composable` | 1 |
| `Composition` | 1 | `Effect` | 1 | `ExternalReference` | 1 |
| `FreezeFrame` | 1 | `Gap` | 1 | `GeneratorReference` | 1 |
| `HookScript` | 1 | `ImageSequenceReference` | 1 | `Item` | 1 |
| `LinearTimeWarp` | 1 | `Marker` | **2** | `MediaLinker` | 1 |
| `MediaReference` | 1 | `MissingReference` | 1 | `PluginManifest` | 1 |
| `SchemaDef` | 1 | `SerializableCollection` | 1 | `SerializableObject` | 1 |
| `SerializableObjectWithMetadata` | 1 | `Stack` | 1 | `Test` | 1 |
| `TimeEffect` | 1 | `Timeline` | 1 | `Track` | 1 |
| `Transition` | 1 | `UnknownSchema` | 1 | | |

Note `RationalTime` and `TimeRange` are **not in this map** — they are
`opentime` value types, not `SerializableObject`s, yet they still carry
`"OTIO_SCHEMA": "RationalTime.1"` / `"TimeRange.1"` tags in the JSON. An
implementer who derives the tag list from `type_version_map()` will omit the two
most common tags in any real file.

**The version-set machinery.** `opentimelineio.versioning.full_map()` returns
exactly one family, `OTIO_CORE`, with six labels: `0.14.0`, `0.15.0`, `0.16.0`,
`0.17.0`, `0.18.0`, `0.18.1`. The only difference across the whole range is
`Clip`: **1 at 0.14.0, 2 from 0.15.0 onward**. (`Test` also appears from 0.15.0;
it is an internal test schema.) Downgrading is real and works:

```python
otio.adapters.write_to_string(tl, "otio_json",
    target_schema_versions=otio.versioning.fetch_map("OTIO_CORE", "0.14.0"))
# -> Clip.1 with a single "media_reference" key instead of "media_references"
```

Measured: at label `0.14.0` the clip serialises as `Clip.1` with keys
`['OTIO_SCHEMA','color','effects','enabled','markers','media_reference','metadata','name','source_range']`.

**A fact with real implementation consequences: OTIO adds fields to a schema
without bumping its version number.** Every `Item` subclass in 0.18.1 emits a
`"color"` key. `Stack`, `Track`, `Gap` are all still version `1`, and `Clip` is
still `2` — the same version numbers as 0.15.0, which did not emit `color`.
Therefore **a reader must not treat the schema version as a field-set contract**,
and a writer that emits the 0.15-era field set is still emitting a valid
`Clip.2`. This is why "we support Clip.2" is not the same statement as "we
round-trip Clip.2".

#### B.1.4 The actual JSON, and what `cine-timeline` emits against it

Generated by real OTIO 0.18.1. Key order is as serialised (OTIO writes a fixed
order, not sorted — note `cine-timeline`'s module doc claims it writes "keys
sorted, like OTIO itself does"; OTIO does **not** sort, it emits declaration
order, though this is cosmetic since JSON object order is not semantic).

| Object | Keys emitted by OTIO 0.18.1 | `cine-timeline` writes |
|---|---|---|
| `Timeline.1` | `OTIO_SCHEMA, metadata, name, global_start_time, tracks` | ✅ all five |
| `Stack.1` | `OTIO_SCHEMA, metadata, name, source_range, effects, markers, enabled, color, children` | ⚠️ all but **`color`**; `source_range` hardcoded `null`, `enabled` hardcoded `true` |
| `Track.1` | `… + kind` | ⚠️ all but **`color`**; `source_range` hardcoded `null` |
| `Clip.2` | `OTIO_SCHEMA, metadata, name, source_range, effects, markers, enabled, color, media_references, active_media_reference_key` | ⚠️ all but **`color`**; `effects` always `[]` |
| `Gap.1` | `OTIO_SCHEMA, metadata, name, source_range, effects, markers, enabled, color` | ⚠️ all but **`color`**; `markers` always `[]` |
| `Transition.1` | `OTIO_SCHEMA, metadata, name, in_offset, out_offset, transition_type` | ✅ all six |
| `Marker.2` | `OTIO_SCHEMA, metadata, name, color, marked_range, comment` | ✅ all six |
| `RationalTime.1` | `OTIO_SCHEMA, rate, value` | ✅ |
| `TimeRange.1` | `OTIO_SCHEMA, duration, start_time` | ✅ |
| `ExternalReference.1` | `OTIO_SCHEMA, metadata, name, available_range, available_image_bounds, target_url` | ✅ all six |
| `MissingReference.1` | `OTIO_SCHEMA, metadata, name, available_range, available_image_bounds` | ✅ |
| `GeneratorReference.1` | `… + generator_kind, parameters` | ✅ |
| `ImageSequenceReference.1` | `OTIO_SCHEMA, metadata, name, available_range, available_image_bounds, target_url_base, name_prefix, name_suffix, start_frame, frame_step, rate, frame_zero_padding, missing_frame_policy` | ❌ **not implemented at all** |
| `LinearTimeWarp.1` | `OTIO_SCHEMA, metadata, name, effect_name, enabled, time_scalar` | ❌ **not implemented** |
| `FreezeFrame.1` | same, with `time_scalar: 0.0` | ❌ **not implemented** |
| `Effect.1` | `OTIO_SCHEMA, metadata, name, effect_name, enabled` | ❌ **not implemented** |

`missing_frame_policy` defaults to the string `"error"`. `available_image_bounds`
is `null` in every reference unless set — `cine-timeline` emits it as `null`
correctly.

#### B.1.5 The round-trip, measured in both directions

A probe binary was compiled against `cine-timeline` and run against files
written by real OTIO, and vice versa. (Source kept at
`/tmp/.../scratchpad/otio_probe_example.rs`; it was **removed from the repo**.)

**Direction 1 — `cine-timeline` writes, real OTIO reads: PASSES.**

A timeline exercising external references, a dissolve, a gap, a
`GeneratorReference`, a `MissingReference`, clip markers, track markers, stack
markers, an audio track and a nested `Stack` was emitted by `cine-timeline` and
read by Python OTIO:

```
PYTHON OTIO PARSE: OK
  name= CINE tracks= 3
  duration= RationalTime(180, 24)
   child: Track V1 Video
   child: Track A1 Audio
   child: Stack COMPOUND -
```

This is a genuine and non-trivial result. Most "OTIO support" in the wild does
not achieve it.

**Direction 2 — real OTIO writes, `cine-timeline` reads: FAILS, in eight
distinct ways.** Seventeen single-feature files were generated by OTIO 0.18.1
and fed to `read_otio`:

| Construct (written by real OTIO) | `cine-timeline` result | Severity |
|---|---|---|
| `ImageSequenceReference` | ❌ **hard error** — `unsupported media reference` — *entire document rejected* | **Blocker** — DPX/EXR sequences are the native form of finishing and VFX media |
| Nested `Stack` **inside a Track** | ❌ **hard error** — `unsupported track child 'Stack'` | **Blocker** — this is exactly how OTIO represents a compound clip. `cine-timeline` accepts a `Stack` only as a child of a `Stack`, never of a `Track` |
| `SerializableCollection` at the root | ❌ **hard error** — `expected Timeline.1 at the root` | **High** — this is how OTIO carries a bin, or several timelines in one file |
| Track `kind` other than `Video`/`Audio` (e.g. `"Subtitle"`) | ❌ **hard error** — `unknown track kind` | **High** — OTIO's `kind` is an open string; `otiostat` even reports "Tracks with non standard types" |
| Clip `effects` (`LinearTimeWarp`, `FreezeFrame`, generic `Effect`) | ⚠️ **silently dropped**, no error, no warning | **High** — every retime and every effect vanishes without a trace |
| Track/Stack `effects` | ⚠️ **silently dropped** | **High** |
| Second and subsequent `media_references` entries | ⚠️ **silently dropped** — proxy lost | **High** (already noted as gap 2 in doc 09 §9.2) |
| `active_media_reference_key != "DEFAULT_MEDIA"` | ⚠️ **silently rewritten** — see below | **Critical** |
| `color` on any item | ⚠️ silently dropped | Low (cosmetic) |

**The critical one, stated precisely.** Input: a clip with
`media_references = {DEFAULT_MEDIA: file:///B.mov, proxy: file:///Bp.mov}` and
`active_media_reference_key = "proxy"`. Output after `read_otio` → `write_otio`:

```
   IN  media_refs: ['DEFAULT_MEDIA', 'proxy'] active: proxy
   OUT media_refs: ['DEFAULT_MEDIA']          active: DEFAULT_MEDIA
   IN active url: file:///Bp.mov | OUT url: file:///Bp.mov
```

The proxy path survived, but it is now *labelled* `DEFAULT_MEDIA`. The document
no longer says "this clip is currently viewing a proxy of B.mov"; it says
"this clip's media is `B_proxy.mov`". A subsequent conform, relink, or delivery
render would use the proxy as the master. **This is worse than data loss — it is
a silent identity substitution**, and it is live in shipped code.

Two further confirmations from the same run:

* **OTIO does not validate `source_range ⊆ available_range`.** A clip with
  `source_range = [190, +48)` against `available_range = [0, +200)` constructs
  and serialises without complaint; `visible_range()` returns the out-of-bounds
  range as-is. `cine-timeline::Track::validate` **rejects** the file
  (`Invariant("clip 0 'A' source range … exceeds available range …")`). Doc 09
  §9.2 gap 11 predicted this from documentation; it is now confirmed against a
  file real OTIO actually wrote. `cine-timeline` is stricter than the format it
  claims to conform to, and that strictness rejects valid documents.
* **Everything `cine-timeline` wrote out re-parsed in Python OTIO** in every one
  of the surviving cases — so its writer is robust even when its reader has
  discarded information.

#### B.1.6 `RationalTime` / `TimeRange` semantics, measured

| Property | Measured behaviour | Consequence for CINE-OMNI |
|---|---|---|
| `value` type | **`float`**. `RationalTime(1.5, 24)` serialises as `{"rate":24.0,"value":1.5}` | Sub-frame values are legal OTIO. A reader that assumes integer frames will mis-handle real files |
| Addition rate rule | `RT(1,24) + RT(1,30)` = **`RT(2.25, 30)`**; `RT(1,30) + RT(1,24)` = the same. `RT(1,25)+RT(1,24)` = `RT(2.04167, 25)` | **OTIO rescales to the higher rate.** `ucg_core::RationalTime` rescales to the *left-hand* rate. Mixed-rate sums diverge from OTIO — a real, silent interop defect additional to the float-precision one |
| `rescaled_to` | `RT(1, 24000/1001).rescaled_to(24)` = `RT(1.001, 24)` — no rounding, keeps the fraction | Confirms OTIO stores non-integer frame positions rather than snapping |
| `end_time_exclusive()` | `start + duration`. `[10, +10)@24` → `20` | Matches `cine-timeline` |
| `end_time_inclusive()` | `19` for the same range; for a **non-integer** duration `[10, +10.5)@24` → **`20`**, i.e. the floor of the exclusive end, not `end − 1` | A subtlety worth copying exactly if inclusive ends are ever exposed |
| Zero-duration range | `[0, +0)`: both inclusive and exclusive ends are `0`; `contains(start)` is **False** | Zero-length ranges contain nothing |
| `contains` | half-open: `contains(start)` True, `contains(end_exclusive)` False | Matches `cine-timeline` |
| `overlaps` | `r.overlaps(r)` is **False** for an identical range | Surprising; do not assume reflexivity |

**Timecode.** `opentime` in 0.18.1 exposes exactly three timecode functions:
`from_timecode`, `to_timecode`, `to_nearest_timecode`. Measured:

* `to_timecode(RationalTime(107892, 29.97), 29.97)` → **`01:00:00;00`** — OTIO
  applies **drop-frame automatically** at 29.97, signalled by the `;`.
* the same value at rate `30.0` → `00:59:59:29` (non-drop).
* `from_timecode("01:00:00;00", 29.97)` → `RationalTime(107892, 29.97)` —
  round-trips.
* Legal timecode rates, tested: 23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60 all
  succeed; **12.0 and 100.0 raise `ValueError: SMPTE timecode does not support
  this rate`**. Drop-frame (`;`) is emitted only for 29.97 and 59.94.

Two corrections to doc 09 arising from this: (a) **`nearest_smpte_timecode_rate`
and `is_valid_timecode_rate` do not exist in the Python `opentime` module of
0.18.1** — doc 09 §2.3 cites `nearest_smpte_timecode_rate` from readthedocs
"latest"; it is not in the shipped 0.18.1 Python API. Whether it exists in the
C++ API only is **`UNVERIFIED`**. (b) OTIO's drop-frame handling is *better*
than doc 09 implies — it is automatic and correct. The defect doc 09 measured is
in `ucg_core::RationalTime::to_timecode`, not in OTIO.

**The comparison that matters:** `cine-media::time` (`Timestamp`, `TimeBase`,
`FrameRate`, `Timecode`) is **already more correct than OTIO's own opentime**.
It uses `i64` ticks over an exact `u32/u32` base, cross-multiplies in `i128` for
comparison, has an explicit named rounding policy on `rescale`, has
`is_exact_in` to test losslessness before converting, and implements drop-frame
properly for both 29.97 and 59.94. This is the type the editorial layer should
be built on; `to_ucg_time` is currently the only bridge and it is one-way.

#### B.1.7 What "OTIO-compatible" must mean — the verification bar

**There is no certification.** Searched and not found: any conformance suite,
any published JSON Schema for OTIO, any compatibility programme, any
self-certification checklist. OTIO ships CLI tools (`otiocat`, `otioconvert`,
`otiostat`, `otiotool`, `otiopluginfo`, `otioautogen_serialized_schema_docs` —
verified from the wheel's `entry_points.txt`) and a repository test suite for
*itself*, not a suite third parties can run against their own implementation.
`otiostat` is the closest thing to a validator and it reports statistics plus a
few structural warnings, not conformance.

**Therefore the claim is only as good as the published verification.** The
following five-tier bar is proposed as CINE-OMNI's standard, with each tier
mechanically checkable in CI:

| Tier | Claim it licenses | Test |
|---|---|---|
| **1. Writes readable OTIO** | "exports OTIO" | Every file we write is read by Python `opentimelineio` without exception |
| **2. Reads authentic OTIO** | "imports OTIO" | Every file in a corpus written *by* Python OTIO — covering all 29 schemas and every media-reference type — parses without error |
| **3. Structural round-trip** | "round-trips OTIO" | `ours(theirs(x))` and `theirs(x)` compare **semantically equal** as OTIO object graphs (not textually) |
| **4. Lossless round-trip** | **"OTIO-compatible"** | Tier 3 plus: no field is dropped. Enforced by a *deep JSON diff* of `write(read(f))` against `f`, with an explicit, documented, reviewed allowlist of permitted differences |
| **5. Semantic agreement** | "OTIO-conformant" | For a corpus of timelines, our `range_of_child`, `duration`, `child_at_time` and `media_time_at` agree frame-for-frame with OTIO's `range_of_child_at_index`, `duration`, `child_at_time` and `transformed_time` |

**CINE-OMNI today passes Tier 1 and fails Tier 2.** It may not claim
"OTIO-compatible", which requires Tier 4.

**The harness is cheap and should be built before any new editorial feature.**
Concretely: a Python-side generator that emits a corpus covering every schema
and every enum value; a Rust-side `read → write` step; a Python-side deep
comparison. It runs in CI wherever Python and the wheel are available and skips
loudly where they are not — the same `need!` discipline `cine-media` already
uses for real media. The corpus generator built during this research is a
working prototype of exactly this.

#### B.1.8 Adapters — what the core wheel contains, and what does not

```python
>>> otio.adapters.available_adapter_names()
['otio_json', 'otioz', 'otiod']
```

**Only three, and all three are OTIO's own formats** — `.otio` JSON, and the
zipped/directory bundle forms. EDL, AAF, FCPXML and the rest are **separate pip
packages in separate repositories**, and they are Python-only. There is no route
to them from Rust except a subprocess.

Fidelity is stated by OTIO itself: `.otio` is the only lossless format; *"Other
formats are lossy — they will only store and retrieve features that are
supported by that format (and by the adapter implementation)"* (doc 09 §6.1).

**Adapter inventory, verified against the PyPI JSON API** (the GitHub API
rate-limited this environment; PyPI did not):

| Package | Version | Licence | Last upload | Requires |
|---|---|---|---|---|
| `otio-cmx3600-adapter` | 1.0.0 | Apache-2.0 | 2023-07-07 | `opentimelineio>=0.15.0` |
| `otio-aaf-adapter` | **2.0.0** | Apache | **2025-11-21** | `opentimelineio>=0.17.0`, **`pyaaf2>=1.7.0`** |
| `otio-fcp-adapter` | 1.0.0 | Apache-2.0 | 2023-07-07 | `opentimelineio>=0.15.0` |
| `otio-fcpx-xml-adapter` | 1.0.0 | Apache-2.0 | 2023-07-07 | `opentimelineio>=0.15.0` |
| `otio-ale-adapter` | 1.1.0 | Apache | **2025-12-15** | `opentimelineio>=0.18.1` |
| `otio-xges-adapter` | 1.0.0 | Apache-2.0 | 2023-07-07 | `opentimelineio>=0.15.0` |
| `pyaaf2` | 1.7.1 | **MIT** (LICENSE fetched and confirmed: *"The MIT License (MIT) Copyright (c) 2017 Mark Reid"*) | 2023-09-17 | — |

All four format adapters were **installed into this environment and actually
run** (§B.2.2, §B.3.2, §B.4.2):

```
>>> otio.adapters.available_adapter_names()
['AAF', 'cmx_3600', 'fcp_xml', 'fcpx_xml', 'otio_json', 'otiod', 'otioz']
```

Note the naming: **`fcp_xml` is the legacy FCP7 `xmeml` adapter and `fcpx_xml`
is the modern FCPXML adapter.** They are different formats and different
packages (§B.4).

#### B.1.9 Rust crates for OTIO — verified against the crates.io API

| Crate | Version | Published | Licence | Downloads | Verdict |
|---|---|---|---|---|---|
| `opentimelineio` | **0.1.0** | **2020-10-20**, one version, never updated | BSD-3-Clause | 2,049 | Description is literally *"Rust bindings for OpenTimelineIO **(placeholder)**"*. Repo `github.com/scott-wilson/OpenTimelineIO-rs`. **Not usable** |
| `opentimelineio-sys` | **0.0.0** | **2020-11-03**, one version | BSD-3-Clause | 1,784 | Same author, same *"(placeholder)"* description, version literally `0.0.0`. **Not usable** |
| `otio-types` | 0.9.1 | 2026-08-24, one version | **AGPL-3.0-or-later** | 134 | *"Build-time Rust types for the Pulse OpenTimelineIO interchange profile"*, repo `ignition-is-go/pulse-pixelstream`. A vendor-specific profile, not OTIO. **AGPL — categorically unusable in an Apache-2.0 product** |

**There is no Rust OTIO implementation and no Rust OTIO binding.** The name
`opentimelineio` on crates.io is squatted by a six-year-old placeholder. This is
decisive for §C.3.

### B.2 EDL (CMX 3600)

Doc 09 §3.3 establishes what an EDL carries and its identity limits. This
section adds the **actual grammar**, read from executable code, and a **measured
round trip**.

#### B.2.1 The real grammar, from the reference parser

The best available specification for CMX 3600 is not a document — it is
`otio-cmx3600-adapter`'s parser, which is **Apache-2.0** (`# SPDX-License-Identifier:
Apache-2.0` on line 1 of `src/otio_cmx3600_adapter/cmx_3600.py`, 1,395 lines,
fetched from `raw.githubusercontent.com`). Reading it settles several things
that secondary sources get wrong:

* **The event line is whitespace-delimited, not column-positional.** The parser
  does `line.split()` and accepts exactly **8 or 9 fields**:
  * **8 fields** = `event, reel, channel, editType, srcIn, srcOut, recIn, recOut`,
    and `editType` **must be exactly `C`** — anything else raises
    `EDLParseError('incorrect edit type …')`.
  * **9 fields** = the same with a `transition_data` field inserted after the
    edit type; for `D` and `W###` that datum is a **frame count**.
* **Transition codes**, verbatim from `transition_regex_map`:
  `C` cut, `D` dissolve, `W\d{3}` wipe, `KB` key_background, `K` key_foreground,
  `KO` key_overlay. The source carries the candid comment
  *"i haven't figured out how the key transitions (K, KB, KO) work"* and
  `@TODO: not currently read by the transition code` against the wipe code.
* **Channel shorthand**, verbatim from `channel_map`:
  `A → [A1]`, `AA → [A1, A2]`, `B → [V, A1]`, `A2/V → [V, A2]`,
  `AA/V → [V, A1, A2]`. *"Channels not listed here are used as track names
  verbatim."*
* **Timecode fields accept bare frame numbers.** If a field contains no `:`, it
  is treated as an integer frame count and converted.
* **Comment lines recognised**, in priority order (`comment_id_map`):
  `FROM CLIP NAME`, `TO CLIP NAME`, `FROM CLIP`, `FROM FILE`, `LOC` (repeatable
  — this is how markers survive), `ASC_SOP`, `ASC_SAT`, `M2`,
  `* FREEZE FRAME`, `* OTIO REFERENCE <x>`. Anything else is collected as
  unhandled text. Note the parser raises if **both** `FROM CLIP` and
  `FROM FILE` appear.
* **`M2` (motion effect / speed)** is parsed by
  `(?P<name>.*?)\s*(?P<speed>-?[0-9\.]*)\s*(?P<tc>[0-9:]{11})$` — a name, a
  signed decimal speed, and a timecode. Negative speeds (reverse) are therefore
  expressible.
* **`SPLIT:` with `AUDIO DELAY` / `VIDEO DELAY`** is recognised, meaning the
  next two events share comment data.
* **Reel derivation on write** (`_reel_from_clip`): a `Gap` becomes reel **`BL`**
  (black); otherwise `metadata.cmx_3600.reel`, else the media reference name,
  else the basename of `target_url`; then the extension is stripped,
  `[^ a-zA-Z0-9]` is removed, and the result is **truncated to `reelname_len`
  (default 8) and space-padded to exactly that width**. `AX` is the fallback.
* **Writer structural limits, enforced as errors:** *"Only a single video track
  is supported"* (`NotSupportedError` for any other count) and *"No more than 2
  audio tracks are supported."*
* **Three output styles** (`VALID_EDL_STYLES`): `avid` → `* FROM CLIP:`,
  `nucoda` → `* FROM FILE:`, `premiere` → **no FROM comment at all**, with the
  in-source explanation that *"Adobe Premiere imports an EDL that uses a 'FROM'
  comment [and this] will result in the clips being named UNKNOWN instead of
  using the reel or file name"*.

**Drop-frame, settled and worse than expected.** The `FCM:` line is how
drop-frame is signalled in an EDL, and the reference adapter **ignores it on
read** — verbatim: *"this can occur either in the header or before any 'event'
in both cases we can ignore it since it is meant for tape timecode"* — and
**never emits it on write**, carrying only a TODO: *"We should try to detect the
frame rate and output an appropriate 'FCM: NON-DROP FRAME' etc here."* Confirmed
by inspecting the actual output in §B.2.2: **there is no FCM line.** The
industry's best open EDL implementation does not handle drop-frame at all. Any
CINE-OMNI EDL writer that *does* emit a correct `FCM:` line is immediately ahead
of it — and this is cheap, because `cine-media::time::Timecode` already
implements drop-frame properly.

#### B.2.2 Measured round trip: OTIO → EDL → OTIO

A timeline of three video clips with distinct URLs, per-clip markers, custom
metadata, one 24-frame dissolve, one audio track and a `global_start_time` of
`01:00:00:00` was written to EDL and read back. The **actual EDL produced**:

```
TITLE: RT

001  shotA    V     C        00:00:00:10 00:00:01:22 00:00:00:00 00:00:01:12
* FROM CLIP NAME:  shotA
* FROM CLIP: file:///media/shotA.mov
* OTIO TRUNCATED REEL NAME FROM: shotA.mov
* LOC: 00:00:00:15 RED     MK0
002  shotA    V     C        00:00:01:22 00:00:01:22 00:00:01:12 00:00:01:12
002  shotB    V     D 024    00:00:03:16 00:00:07:04 00:00:01:12 00:00:05:00
…
```

Note the classic CMX dissolve idiom: a **zero-duration `C` event** for the
outgoing side (source in == source out) immediately followed by the `D 024`
event for the incoming side, sharing an event number.

| Property | Before | After | Verdict |
|---|---|---|---|
| Video tracks | 1 | 1 | ✅ |
| **Audio tracks** | 1 | **0** | ❌ **audio silently gone** — the writer emits only track index 0 |
| Clips | 4 | 3 | ❌ (the audio clip) |
| Clip names | shotA, shotB, shotC | shotA, shotB, shotC | ✅ (via `* FROM CLIP NAME`) |
| **Media URLs** | shotA, **shotB**, shotC | shotA, **shotA**, shotC | ❌ **wrong URL on the clip after the dissolve** — the reader takes the `FROM CLIP` of the dissolve pair rather than the `TO CLIP` |
| Source ranges | 10+48, 100+72, 0+36 | 10+36, 88+84, 0+36 | ⚠️ altered by design — the dissolve's handles are folded into the neighbouring clips' ranges |
| Transitions | 1 | 1 | ✅ |
| Markers | 3 | 4 | ⚠️ duplicated — the dissolve's shared comment block carries the marker twice |
| **`global_start_time`** | `01:00:00:00` | **`None`** | ❌ **lost** |
| **Custom metadata** (`ucg_id`) | present | **gone**, replaced by `cmx_3600.reel` | ❌ **lost** |
| Reel-name truncation | — | recorded as `* OTIO TRUNCATED REEL NAME FROM: …` | ✅ a genuinely good touch worth copying |

**Reading:** EDL preserves *cuts, timecode and names*, and nothing else — as
doc 09 predicted. But two of the losses above are **implementation** defects
rather than format limits: audio tracks and the post-dissolve URL are both
expressible in CMX 3600. A native CINE-OMNI writer can do better than the
reference adapter, and the reference adapter is the thing everyone else's EDL
gets compared against.

**Rust crates:** `edl` v1.1.2, MIT, published 2022-10-06, 5,847 downloads, four
versions, **no repository URL declared**, author `zekroTJA`. Described only as
*"Parse EDL (edit decision list) files."* Unmaintained for four years and of
unverified scope. **Not adoptable without an audit**; EDL is a few hundred lines
to write natively and §B.2.1 is effectively the specification.

### B.3 AAF

Doc 09 §1.2 establishes the object model (Mob / MasterMob / SourceMob /
SourceClip, MobID = SMPTE UMID). Not repeated. This section adds container
confirmation, licences read from actual LICENSE files, and a measured attempt.

#### B.3.1 Container and licences — verified

**AAF is Compound File Binary, confirmed empirically.** An AAF file created by
`pyaaf2` in this environment begins with the bytes:

```
d0 cf 11 e0 a1 b1 1a e1
```

which is exactly the Microsoft Compound File Binary / OLE Structured Storage
magic. An AAF reader therefore needs a CFB layer before it can see any AAF at
all. (Whether AAF is *always* CFB, or whether XML/KLV variants are also
normative, remains **`UNVERIFIED`** — see §D.)

| Implementation | Licence | Evidence | Usable in Apache-2.0 CINE-OMNI? |
|---|---|---|---|
| **`pyaaf2`** | **MIT** | LICENSE fetched: *"The MIT License (MIT) — Copyright (c) 2017 Mark Reid"* | ✅ **Yes** — MIT is permissive and Apache-2.0-compatible. Python, so not linkable, but freely *studiable* and freely subprocess-able |
| **`LibAAF`** | **GPL-2.0** | LICENSE fetched: *"GNU GENERAL PUBLIC LICENSE Version 2, June 1991"* | ❌ **No.** Not linkable, not copyable. Doc 09 cites it as a source of object-model facts; that is fine as *documentation*, but **no LibAAF code may be read-and-copied** and contributors writing AAF code should avoid its source to keep a clean-room argument |
| **`otio-aaf-adapter`** | Apache (per PyPI metadata; LICENSE file not located at the paths tried) | PyPI `info.license` = "Apache License" | ✅ likely, but the LICENSE file itself is **`UNVERIFIED`** |
| **AAF SDK reference implementation** | "AAF SDK Public Source License" | not fetched | **`UNVERIFIED`** — must be checked for OSI approval and Apache-2.0 compatibility before consideration |

**The single most encouraging fact about AAF-in-Rust:** the container problem is
already solved. `cfb` v0.14.0, **MIT**, 21 versions, **59,615,747 downloads**,
created 2017-03-20, last updated **2026-02-13**, repo
`github.com/mdsteele/rust-cfb`, *"Read/write Compound File Binary (structured
storage) files"* — verified against the crates.io API. What is **not** solved is
the AAF object model above it, which is where essentially all the work is.

#### B.3.2 Measured: writing AAF from OTIO does not work

The same round-trip harness was pointed at the `AAF` adapter. It **failed to
write**, twice, for two independent and highly informative reasons:

**With a transition present:**

```
AAFValidationError:
 dis <Transition> Transition.metadata['AAF']['PointList'] does not exist, 'AAF'
 dis <Transition> Transition.metadata['AAF']['OperationGroup']['Operation']['DataDefinition']['Name'] does not exist
 dis <Transition> Transition.metadata['AAF']['OperationGroup']['Operation']['Description'] does not exist
 dis <Transition> Transition.metadata['AAF']['OperationGroup']['Operation']['Name'] does not exist
 dis <Transition> Transition.metadata['AAF']['CutPoint'] does not exist
```

**With the transition removed:**

```
AAFAdapterError: Cannot find mob ID for clip
  Clip("shotA", ExternalReference("file:///media/shotA.mov"), …)
```

**This is the decisive AAF finding.** `otio-aaf-adapter` can only write a clip
that **already carries an AAF MobID**, and can only write a transition that
**already carries AAF-native `OperationGroup` metadata**. In other words the
adapter is built for **AAF → OTIO → AAF** round trips — returning to Avid
material that came from Avid — and **not** for originating an AAF from a
timeline authored elsewhere. Anyone planning "we will export AAF via OTIO"
should know this before planning it.

The practical consequence for CINE-OMNI: exporting AAF is not a matter of
wiring up an adapter. It requires **minting MobIDs** (i.e. having the material
identity layer doc 09 §8.4 specifies) and constructing AAF-native effect
descriptions. That is a large project, and it is a large project *even if you
subprocess Python*.

**Rust crates claiming AAF:** `oximedia-aaf` v0.2.1, Apache-2.0, repo
`github.com/cool-japan/oximedia`, 12 versions, created 2026-03-08, updated
2026-08-12, 1,027 downloads, described as *"Advanced Authoring Format (AAF)
support for OxiMedia — SMPTE ST 377-1 compliant"*. Siblings: `oximedia-edl`,
`oximedia-conform`, `oximedia-timeline`, `oximedia-timecode`. **The description
contains a category error** — SMPTE **ST 377-1 is the MXF file format
specification, not AAF** — which is exactly the sort of mistake that
distinguishes a generated package from a real implementation. Six months old,
low downloads, a spec-number error, and an entire family published on one day.
**`UNVERIFIED` and should not be adopted without reading the source.**

### B.4 FCPXML and Premiere XML

Doc 09 §1.3 covers the FCPXML *model* in depth (`asset/@uid`, `media-rep`,
`format`, `multicam`, `timeMap`, `conform-rate`, `smart-collection`) with
verified DTD quotations from v1.8 and v1.11. Not repeated.

#### B.4.1 The version landscape — every DTD fetched and counted

Doc 09 knew of v1.8 and v1.11. Every version was probed on the CommandPost
mirror of Apple's DTDs and, where present, downloaded and its `<!ELEMENT>`
declarations counted:

| DTD | Elements | | DTD | Elements |
|---|---|---|---|---|
| `FCPXMLv1_1` | 43 | | `FCPXMLv1_9` | 101 |
| `FCPXMLv1_2` | 53 | | `FCPXMLv1_10` | 107 |
| `FCPXMLv1_3` | 95 | | `FCPXMLv1_11` | (fetch flaked; 110 unique elements measured on a prior pass) |
| `FCPXMLv1_4` | 99 | | `FCPXMLv1_12` | 110 |
| `FCPXMLv1_5` | 92 | | `FCPXMLv1_13` | 112 |
| `FCPXMLv1_6` | 95 | | **`FCPXMLv1_14`** | **113 — newest** |
| `FCPXMLv1_8` | 99 | | `FCPXMLv1_15` | **404 — does not exist** |

**FCPXML 1.14 is the current version** — three beyond what doc 09 established.
Its header reads:

```dtd
<!-- FCP XML Interchange Format, Version 1.14 -->
<!-- Copyright (c) 2011-2026 Apple Inc. All rights reserved. -->
```

**The version is `#FIXED` in the DTD**, which settles the version-lock question:

```dtd
<!ELEMENT fcpxml (import-options?, resources?, (library | event* | (%event_item;)*))>
<!ATTLIST fcpxml version CDATA #FIXED "1.14">
```

A document declaring any other version **fails DTD validation against that
DTD**. Each version is therefore a separate schema and a parser must select the
DTD by the declared version — there is no single "FCPXML schema" to validate
against.

**FCPXML evolves purely additively.** Comparing the unique element sets of 1.8
and 1.14: **14 elements added, zero removed.**

```
adjust-cinematic  adjust-colorConform  adjust-stereo-3D  adjust-voiceIsolation
hidden-clip-marker  live-drawing  locator  match-analysis-type  match-markers
match-representation  match-usage  media-rep  object-tracker  tracking-shape
```

(1.13 → 1.14 adds exactly one: `match-analysis-type`.) This is an unusually
friendly evolution profile for an implementer: **target the newest DTD, be
permissive about unknown elements, and older documents remain readable.**

Two of those additions are the ones doc 09 §1.3 identified as load-bearing, and
they are confirmed present at the current version:

```dtd
<!ELEMENT asset (media-rep+, metadata?)>
<!ATTLIST asset id ID #REQUIRED>
<!ATTLIST asset uid CDATA #IMPLIED>
<!ELEMENT media-rep (bookmark?)>
```

1.14's `asset` has also grown a set of **per-material overrides** —
`customLUTOverride`, `colorSpaceOverride`, `projectionOverride`,
`stereoscopicOverride`, `heroEyeOverride`, `auxVideoFlags` — which is exactly
the "the project may correct what the file claims about itself" requirement doc
09 §1.6 drew from Resolve's Clip Attributes, now visible in a published schema.

**FCPXML is specified by a DTD only.** No XSD or RelaxNG could be located. A DTD
cannot express datatypes, and Apple's own header proves the consequence — the
rational-seconds format is defined **in a comment**, with the type declared as
bare `CDATA`:

```dtd
<!-- 'time' attributes are expressed as a rational number of seconds (e.g., "1001/30000s") -->
<!-- with a 64-bit numerator and 32-bit denominator. -->
<!ENTITY % time "CDATA">
```

So `%time;` validation is entirely the implementer's job, and the 64-bit
numerator / 32-bit denominator bound is a real constraint stated nowhere a
machine can read it.

#### B.4.2 Measured round trips

Both adapters were installed and run against the same source timeline as §B.2.2.

**FCPXML (`fcpx_xml` adapter).** Writes `<fcpxml version="1.8">` with
`<!DOCTYPE fcpxml>` — i.e. **five versions behind the current DTD**.

| Property | Before | After | Verdict |
|---|---|---|---|
| Video / audio tracks | 1 / 1 | 1 / 1 | ✅ |
| Clips and URLs | 4, all distinct | 4, all correct | ✅ — better than EDL |
| Source ranges | 10+48, 100+72, 0+36 | 10+36, 88+84, 0+36 | ⚠️ transition handles folded in |
| Markers | 3 | 3 | ✅ |
| **Transitions** | 1 | **0** | ❌ **dissolve silently lost** |
| **`global_start_time`** | `01:00:00:00` | **`None`** | ❌ lost |
| **Custom metadata** | present | **empty** | ❌ lost |
| Clip ordering | V then A | **audio first** | ⚠️ ordering not preserved |

The emitted XML is also structurally weak: every `<asset>` gets the same
`duration="125/6s"`, `<asset-clip>` elements are emitted at top level *outside*
`<project>`, and `<project name="RT" uid="">` has an **empty `uid`** — the very
attribute doc 09 §1.3 identifies as FCPXML's material identity. The adapter does
not use the format's best feature.

**Premiere / FCP7 XML (`fcp_xml` adapter).** Writes `<xmeml version="4">` —
confirming both that this is the legacy `xmeml` format and that **version 4** is
what is emitted. Measured, it **round-trips better than the modern FCPXML
adapter does**:

| Property | Result |
|---|---|
| Video / audio tracks | ✅ 1 / 1 |
| Clips, names, URLs | ✅ all 4 correct and distinct |
| Transitions | ✅ **preserved** |
| **`global_start_time`** | ✅ **preserved** (`RationalTime(86400, 24)`) — the only adapter of the three that kept it |
| Custom metadata | ❌ lost (replaced by `fcp_xml` adapter metadata) |
| Source ranges | ⚠️ transition handles folded in, as elsewhere |

**The uncomfortable conclusion:** of the three non-OTIO adapters, the *legacy,
superseded* format has the highest measured fidelity. This is a statement about
adapter maturity, not about format capability — FCPXML is by far the more
capable format on paper (doc 09 §1.3) — but it means "export FCPXML via OTIO"
delivers less today than "export FCP7 XML via OTIO" does.

**Premiere XML is a different format from FCPXML, and the distinction is
load-bearing.** Premiere's XML interchange is the legacy **Final Cut Pro 7 XML
Interchange Format** (`<xmeml version="4">`), *not* Apple's modern FCPXML.
Conflating them is the standard error in this area, and the OTIO package names
(`otio-fcp-adapter` = xmeml, `otio-fcpx-xml-adapter` = FCPXML) do nothing to
help. Adobe's *current* support status for either is **`UNVERIFIED`** —
`helpx.adobe.com` was found unreachable from this environment during the doc 09
research (TLS connects, then hangs).

**Rust crates:** searching crates.io for `fcpxml` returns **no FCPXML
implementation** — only two unrelated AI video-editor packages. `xmeml` returns
**zero results**. There is no Rust FCPXML or xmeml library of any kind.

### B.5 Timecode crates (relevant to every format above)

| Crate | Version | Licence | Downloads | Updated | Note |
|---|---|---|---|---|---|
| `timecode` | 0.7.5 | MIT | 56,816 | 2025-10-21 | 24 versions, `github.com/media-io/timecode` |
| `vtc` | 0.1.14 | MIT | 20,029 | 2025-11-26 | `github.com/opencinemac/vtc-rs`; description has a typo (*"A SMPTE timeoce library"*) |
| `video-timecode` | 0.6.1 | MIT | 20,707 | **2019-05-12** | Abandoned |

All verified against the crates.io API. **None is needed**: `cine-media::time`
already implements exact rational time, frame rates and drop-frame timecode
correctly, and adding a dependency would introduce a second, weaker time model.

### B.6 The interchange decision table

| Format | Spec status | Licence of the format / of usable implementations | What survives a round trip | Recommendation |
|---|---|---|---|---|
| **OTIO `.otio` JSON** | Open, versioned, **but no published JSON Schema and no conformance suite**. Reference implementation is the spec | **Apache-2.0, verified by hash.** No Rust binding exists (both crates.io names are 2020 placeholders) | **Everything** — the only lossless format. Cuts, ranges, transitions, markers, effects (opaquely), multiple media references, nesting, arbitrary metadata | **Implement first, natively, and harden the existing reader to Tier 4** (§B.1.7). It is the only format worth being genuinely lossless in |
| **EDL (CMX 3600)** | De-facto; formalised late as SMPTE 258M. **Best executable spec is the Apache-2.0 `otio-cmx3600-adapter` parser, read in full (§B.2.1)** | Format unencumbered. `edl` crate MIT but unmaintained, unaudited, no repo declared | **Measured (§B.2.2): cuts, timecode, clip names, transitions and markers survive. Audio tracks, `global_start_time` and all custom metadata are lost.** Reel truncated to 8 chars. Reference adapter emits **no `FCM:` line at all**, so drop-frame is not carried | **Implement natively, second.** A few hundred lines, and §B.2.1 is the specification. Emitting a correct `FCM:` line and preserving audio would already beat the reference implementation |
| **FCPXML** | **Apple DTD v1.1–v1.14; 1.14 current (Copyright 2011-2026). `version` is `#FIXED` per DTD. Purely additive: 14 elements added 1.8→1.14, none removed. DTD only — no XSD; `%time;` is bare `CDATA`** | Format unencumbered; **no Rust implementation exists** (crates.io `fcpxml` → nothing) | On paper the richest: identity (`asset/@uid`), proxy as data (`media-rep+`), retimes (`timeMap`), rate conform, multicam, compounds, keywords, ranged markers, per-material colour/LUT overrides. **Measured via the OTIO adapter, much less: transitions and `global_start_time` lost, `uid` emitted empty, and it writes v1.8** | **Third, and write it natively against v1.14.** Highest-value non-OTIO target because it is the only format carrying material identity *and* the proxy relationship as data. Do **not** rely on the OTIO adapter — §B.4.2 shows it under-uses the format badly |
| **AAF** | SMPTE/AMWA specs, partly paywalled; object model documented. **Container confirmed empirically as CFB (`d0cf11e0a1b11ae1`)** | **Container solved**: `cfb` crate, MIT, 59.6M downloads, maintained. **`pyaaf2` is MIT (verified)** — usable and studiable. **`LibAAF` is GPL-2.0 (verified) — categorically unusable and should not be read** | **Measured: writing AAF from an OTIO-native timeline FAILS.** `otio-aaf-adapter` requires clips to already carry AAF MobIDs and transitions to already carry AAF `OperationGroup` metadata. It is an **AAF→OTIO→AAF** tool, not an exporter | **Last, and not in-house.** Export requires minting MobIDs and authoring AAF-native effect descriptions — a project in itself, *even via Python*. If Avid interop is required, do the identity layer (doc 09 §8.4) first, then subprocess `pyaaf2` (MIT) |
| **Premiere XML (`xmeml`, FCP7 XML)** | Legacy; `<xmeml version="4">` confirmed as what the adapter emits | Format unencumbered; **no Rust implementation**; `xmeml` → zero crates.io results | **Measured: the highest fidelity of the three non-OTIO adapters.** Tracks, clips, URLs, transitions **and `global_start_time`** all survive — the only adapter that kept the timecode origin. Custom metadata lost | **Do not implement, but know the result.** Its measured superiority is a statement about adapter maturity, not format capability. If a quick Premiere hand-off is ever needed before native FCPXML exists, this is the pragmatic route |

---

## Part C — Recommendations

### C.1 What `cine-timeline` is, versus a professional editorial engine

**The frank assessment.** `cine-timeline` is **3,431 lines of source across
seven modules** (`otio` 1002, `graph` 796, `edit` 653, `otio_json` 533, `sync`
312, `lib` 79, `error` 56) plus **1,319 lines of tests**. It is best described
as **a correct, well-tested OTIO composition library with an editing-operations
layer bolted to a graph importer**. It is not an editorial engine and does not
pretend to be one — but the crate's own documentation over-claims in two
specific places that should be corrected regardless of what else happens:

* `lib.rs` calls it *"A multi-track NLE data model **conforming to**
  OpenTimelineIO"*. Measured, it does not conform: it rejects four constructs
  real OTIO writes and silently drops four more (§B.1.5).
* `otio_json.rs` says *"Keys are written sorted, like OTIO itself does"*. OTIO
  does not sort keys. Cosmetic, but it is a claim about another system's
  behaviour that is wrong.

**What is genuinely good, and it is more than it looks.**

1. **The composition math is correct, not approximately correct.** Transitions
   take no track time and overlap the cut; `range_of_child` reproduces OTIO's
   `range_of_child_at_index` including the transition case; stacks compose in
   parallel and tracks sequentially. This is the part that is tedious to get
   right and it is right.
2. **The edit operations are the real asset.** `ripple_trim`, `roll`, `slip`,
   `slide`, `insert`, `overwrite`, `lift`, `extract`, `split_at`, `join` — with
   correct handle clamping, correct transition-floor handling, correct
   transition healing, and an atomic `commit` that validates before swapping.
   §A.3 finds no semantic errors in any of them. This is months of work already
   banked.
3. **The transaction discipline is right.** `commit()` mutating a clone and
   validating before the swap is exactly the shape a UCG transaction wants.
4. **Idempotent graph import.** `import_timeline` re-run on unchanged input
   produces zero ops — the precondition for reactor convergence, and easy to
   lose.
5. **`ucg_id` survives a round trip through external OTIO tools** via item
   metadata. Rare and genuinely valuable.

**What it is not.** Beyond doc 09 §9.2's fifteen gaps (all still accurate),
Part A finds four operational gaps not in that list:

* **no transition media-handle check** (§A.5) — a dissolve can validate and be
  unrenderable;
* **all operations are single-track** — no ripple-across-tracks, no track
  targeting, no linked selection, so nothing preserves sync between V and A;
* **no mark state** (source in/out vs record in/out), therefore no three-point
  edit, no four-point, no fit-to-fill, no replace edit;
* **`Track::rate()` infers the rate from the first timed child**, falling back
  to `24.0`. Rate is inferred rather than declared, and every mixed-rate
  operation then rescales through that arbitrary reference rate.

**Recommendation: SPLIT. Not evolve, not replace.**

Three concerns are currently fused in one crate and they have different
lifetimes, different dependencies and different correctness criteria:

| New crate | Takes | Why separate |
|---|---|---|
| **`cine-otio`** | `otio.rs` model + `otio_json.rs` codec | An interchange codec has one job: be bit-faithful to someone else's format. It must be permissive on read, exhaustive on write, and it must *not* impose CINE-OMNI's opinions (which is exactly the `validate()` bug). It should be testable against Python OTIO with no graph and no editing in the way |
| **`cine-edit`** | `edit.rs` operations, retargeted onto an internal model with exact time | The operations are the durable asset. They should not be written against the OTIO wire model, because the wire model lacks retimes, links, targeting and referenced compounds — all of which change the operations' bounds arithmetic |
| **`cine-timeline`** (kept) | the internal editorial model + `graph.rs` + `sync.rs` | The UCG mapping and the editorial model belong together and are CINE-OMNI-specific |

**Why not evolve in place:** the crate's central type is the OTIO wire model, and
every gap in Part A requires a construct OTIO does not have (links, targeting,
mark state, referenced compounds, adjustment layers, time functions). Growing
those into the OTIO types means either corrupting the wire model or carrying
"our extensions" as metadata — the latter is exactly what makes interchange
libraries rot.

**Why not replace:** the edit operations and composition math are correct and
tested. Discarding 61 passing tests of hard-won semantics to rewrite them
identically is unjustifiable.

**The split is also the cheapest path to fixing the OTIO bugs**, because a
codec crate with no opinions is precisely what §B.1.5's failures require.

### C.2 The correct in-memory model

Constraints: exact rational time (`cine-media::time`), UCG identity by stable
UUID, referenced rather than inlined compounds, retime as a function, and the
material/representation split from doc 09.

```
Sequence                                     ← nle.timeline (has a DECLARED rate + timecode origin)
 └ Stack                                     ← nle.stack
    └ Track  { kind, rate, enabled, target } ← nle.track
       └ Item = Clip | Gap | Transition
            Clip {
              binds: MaterialLike,            ← Binds edge (material | subclip | composite | sync_group)
              source_range: Range<Timestamp>, ← EXACT, in the material's own time base
              time_transform: TimeTransform,  ← identity | conform | linear | time_map | freeze
              stream_selection,               ← which angle, which channels
              link_group: Option<LinkId>,     ← linked selection
              markers (item-local time)
            }
```

**Five load-bearing choices.**

1. **Time is `cine_media::time::Timestamp` over an exact `TimeBase`, never
   `f64`.** The type already exists and is already correct. The editorial layer
   should adopt it rather than the other way round. `RationalTime` survives only
   as the OTIO wire representation, converted at the codec boundary — which is
   what `to_ucg_time` already gestures at, and which needs its inverse.
2. **A Track declares its rate; it does not infer one.** Every position on the
   track is an integer tick count at that rate. Cross-rate conversion happens
   once, explicitly, at clip boundaries, with a named rounding policy.
3. **Source→instance time is a stored `TimeTransform`, not an offset.** Rate
   conform and speed change are the same category of thing (FCPXML's
   `%timing-params;` proves it). This must go in *before* the operations are
   retargeted, because every trim bound in §A.3 must be computed through it.
4. **Nesting is by reference.** A `Clip` binds to a `media.composite`; it does
   not contain a `Stack`. This diverges from OTIO deliberately, and the
   divergence is resolved *at the codec boundary* by inlining on export and
   de-duplicating on import — which is exactly the kind of thing a separate
   codec crate makes tractable.
5. **Links are a group id, not a pair of edges.** Linked selection is
   N-way (one video, four audio), so it is a set, and a set with an identity is
   a node.

**Mapping to UCG identity.** Doc 09 §8 already specifies the media-side node
kinds and the identity-minting rule; the timeline side maps as follows, and
the important property is that **every editorial entity that a user can select,
annotate, or reference gets a stable UUID, while everything else is an
attribute**:

| Concept | UCG treatment | Id |
|---|---|---|
| Sequence / stack / track / clip / gap / transition / marker | Nodes (already implemented) | `from_key("nle:<source_key>:…")` or the `ucg_id` carried in metadata |
| Clip → material | **`Binds`** edge, exactly one | — |
| `source_range`, `time_transform`, `stream_selection` | **Attributes** on the clip — they belong to exactly one clip and are never shared | — |
| Link group | **Node** (`nle.link_group`), clips `References` it | `derived(track_or_sequence, "link:<n>")` |
| Compound / multicam | **Node** in the media layer (`media.composite` / `media.sync_group`, doc 09 §8.2), bound by the same `Binds` edge as a material | — |
| Adjustment layer | A clip kind with no `Binds` edge and an effect stack; its scope is "lower tracks, same time range" — a *query*, not an edge | — |

The `ucg_id`-in-metadata mechanism is what makes the whole thing survive an
external round trip and should be preserved exactly as it is.

**One structural constraint worth restating:** `ucg_core::Edge` is
`{from, to, kind}` with **no attributes**. Anything that wants to be an edge
property — a sync offset, a link role, a channel mapping — must become a node
attribute on an endpoint or a small intermediary node. This is why the link
group is a node.

### C.3 OTIO: native Rust, FFI, or subprocess?

**Recommendation: native Rust, harden what exists. This is not a close call.**

| Option | Verdict |
|---|---|
| **Native Rust (recommended)** | Already 80 % built and already passing Tier 1. The remaining work is bounded and enumerable: four hard-failure constructs, four silent-loss constructs, and the `validate()` over-strictness (§B.1.5). No new dependency, no build-system risk, works everywhere CINE-OMNI works |
| **FFI to the C++ library** | **Rejected.** The library builds with CMake and pybind11. This machine has **no pkg-config, no make, no nasm** — it has `zig cc`/`zig c++` and rustup. Bootstrapping a CMake C++ build with a Zig toolchain for a dependency we would then have to wrap by hand, in order to obtain a schema we already implement, is a large amount of build-system risk for negative functional gain. It would also reintroduce the identity problem doc 02 §2.4 identifies: OTIO's C++ core addresses objects by pointer, not by stable UUID, so the UCG would have to re-derive identity on every crossing |
| **Rust bindings from crates.io** | **Rejected — they do not exist.** `opentimelineio` 0.1.0 and `opentimelineio-sys` 0.0.0 are both single 2020 placeholders (§B.1.9). `otio-types` is AGPL and is a vendor profile, not OTIO |
| **Subprocess via Python** | **Rejected as a runtime dependency, adopted as a test dependency.** Shipping a Python interpreter plus a wheel to read a JSON file we can already read is indefensible. But Python OTIO is exactly the right thing to *verify against* in CI, and §B.1.7's harness depends on it. This is the honest split: never in the product, always in the test suite |
| **Subprocess for adapters only** | **Keep open.** The adapters (AAF especially) are Python-only and are somebody else's maintained work. If Avid interop is ever required, an optional `otioconvert` subprocess is far cheaper than a native AAF object model — and being optional, it costs nothing when absent |

### C.4 The minimum honest editorial slice to build next

Ordered so each step is independently shippable and each one makes the next
cheaper. This is deliberately *not* "add multicam".

**Step 0 — Stop the bleeding (small, urgent, independent of everything).**
Fix the `active_media_reference_key` identity substitution (§B.1.5). Preserve
all `media_references` rather than discarding all but one. Downgrade
`Track::validate`'s source-exceeds-available error to a recorded `mismatched`
conform state (doc 09 §3.1) instead of a parse-blocking error. Correct the two
over-claims in the crate documentation. **This is a bug fix, not a feature, and
it should not wait for Phase 3.**

**Step 1 — The conformance harness (§B.1.7).** Build the corpus generator, the
Rust read/write step and the deep-diff comparison. Wire it into CI, skipping
loudly when Python or the wheel is absent. Without this, every subsequent claim
about OTIO is unverifiable, and with it every subsequent change is guarded.

**Step 2 — Close the Tier-2 gaps.** `ImageSequenceReference`; nested `Stack` as
a child of a `Track`; `SerializableCollection` at the root; open-string track
kinds; read and preserve `effects` (opaquely if necessary — an
unrecognised effect must round-trip, not vanish); the `color` field. Each is
directly testable by the Step-1 harness. **Reaching Tier 4 is the point at which
"OTIO-compatible" may be claimed in writing.**

**Step 3 — Exact time.** Adopt `cine_media::time::Timestamp` in the editorial
model; give `Track` a declared rate; add the inverse of `to_ucg_time`; keep
`RationalTime` only at the codec boundary. This removes the epsilons and the
measured 13 % wrong-frame rate in one change, and it must precede Step 4 because
it changes every bound calculation.

**Step 4 — The split (§C.1)** and, with it, `TimeTransform` as a first-class
concept on the clip, even if the only implemented transforms are `Identity` and
`LinearSpeed`. The abstraction is what matters; the transforms can follow.

**Step 5 — Then, and only then, editorial features:** mark state and
three-point editing; linked selection and track targeting; the transition
handle check (§A.5). These are the ones that make it feel like an editor, and
they are cheap once Steps 3 and 4 exist and ruinous before.

Everything in doc 09's migration §9.3 (media identity off the file path) is
orthogonal to this list and should proceed in parallel — it has no dependency on
any of the above and its cost grows with every graph written.

---

## Part D — Register of what could not be verified

Items resolved during the research pass are listed first, so the register
records what *was* verified as well as what was not.

**Resolved (initially blocked, later verified):**

| Claim | Resolution |
|---|---|
| OTIO adapter licences/versions/maintenance | **Verified** via the PyPI JSON API after the GitHub API rate-limited this host — see the table in §B.1.8. All four format adapters were then installed and run |
| CMX 3600 grammar; `FCM:` semantics; `M2`; comment-line set | **Verified** by fetching and reading the Apache-2.0 parser source (1,395 lines) from `raw.githubusercontent.com` — §B.2.1 |
| Whether AAF is CFB | **Verified empirically** — a `pyaaf2`-created file begins `d0cf11e0a1b11ae1` |
| `pyaaf2` and `LibAAF` licences | **Verified** by fetching each LICENSE file: pyaaf2 = **MIT**, LibAAF = **GPL-2.0** |
| FCPXML version list, newest version, v1.8→newest element diff, XSD existence, version-lock | **Verified** by fetching every DTD v1.1–v1.15 — §B.4.1. 1.14 is newest; 1.15 is 404; `version` is `#FIXED`; additive-only; DTD only |
| Whether `xmeml` version 4 is what is emitted | **Verified** — the `fcp_xml` adapter writes `<xmeml version="4">` |

**Still unverified:**

| Claim | Status | What was tried / what is known |
|---|---|---|
| OTIO `tests/sample_data` contents; whether a reusable third-party conformance corpus exists there | **`UNVERIFIED`** | GitHub API rate-limited (`API rate limit exceeded for 103.78.167.213`). The absence of a *published conformance suite* is asserted from the wheel contents and entry-point list, not an exhaustive repository read. This does not affect §B.1.7, which proposes generating a corpus rather than finding one |
| Whether `nearest_smpte_timecode_rate` / `is_valid_timecode_rate` exist in OTIO's **C++** API | **`UNVERIFIED`** | Confirmed **absent** from the Python `opentime` module in 0.18.1 (only `from_timecode`, `to_timecode`, `to_nearest_timecode`). Doc 09 §2.3 cites the former from readthedocs "latest" |
| `otio-aaf-adapter`'s LICENSE **file** | **`UNVERIFIED`** | PyPI metadata says "Apache License"; `LICENSE.md` and `COPYING` returned 404 and `LICENSE.txt` flaked on the paths tried. Low risk given the repo is under the OpenTimelineIO org, but the file itself was not read |
| Whether AAF is *always* CFB, or whether XML/KLV variants are also normative | **`UNVERIFIED`** | Confirmed that the AAF `pyaaf2` writes is CFB; the *spec's* full container story was not established from a primary source |
| The "AAF SDK Public Source License": OSI status and Apache-2.0 compatibility | **`UNVERIFIED`** | Not fetched. **Blocking** if the AAF SDK is ever considered — but §B.3.1 makes `pyaaf2` (MIT) the better candidate anyway, so this is unlikely to matter |
| `oximedia-*` crate family: whether it is a genuine implementation | **`UNVERIFIED`, treat as suspect** | crates.io metadata verified (Apache-2.0, 12 versions, 2026-03-08→2026-08-12, whole family published on one day). But `oximedia-aaf` describes itself as *"SMPTE ST 377-1 compliant"* — **ST 377-1 is the MXF spec, not AAF**. Source must be read before any adoption |
| `edl` crate v1.1.2: whether it parses CMX 3600 specifically | **`UNVERIFIED`** | crates.io metadata verified (MIT, 2022-10-06, 5,847 downloads, 4 versions). **No repository URL is declared**, so the source could not be located |
| FCPXML v1.11 element count (110) | **weakly verified** | Measured on one pass; the confirming re-fetch of `FCPXMLv1_11.dtd` flaked. Every other version in the table was fetched and counted directly |
| Premiere Pro's current FCP7-XML (`xmeml`) support status; whether Premiere imports/exports FCPXML or AAF, and in which direction | **`UNVERIFIED`** | `helpx.adobe.com` found unreachable from this environment during doc 09 research (TLS connects, then hangs); not retried here. Note the `otio-cmx3600-adapter` source *does* contain first-hand evidence of Premiere behaviour — its `premiere` EDL style exists specifically because Premiere mis-names clips when a `FROM` comment is present |
| An `xmeml` DTD or formal schema | **`UNVERIFIED`** | Not located. Apple's FCP7 XML Interchange Format documentation is archived; only the emitted `version="4"` was confirmed |
| OTIO's `Item::color` introduction release (asserted here as "added without a schema bump") | **partially verified** | Verified that 0.18.1 emits `color` on every Item while `Stack`/`Track`/`Gap` remain version 1 and `Clip` remains 2 across *all* version-set labels 0.15.0–0.18.1, and that a 0.14.0 downgrade still emits it. The *release* in which `color` first appeared was not established |
| Doc 09 §9.2's fifteen gaps | **inherited, not re-verified** | Part A adds four operational gaps to that list but does not re-derive the original fifteen |

---

## Sources

**Executed in this environment (primary evidence)**
- `opentimelineio` 0.18.1 wheel installed via `uv` from PyPI — `otio.core.type_version_map()`, `otio.versioning.full_map()`/`fetch_map()`, `otio.adapters.available_adapter_names()`, `otio.opentime`, `write_to_string`/`read_from_file`, `otiostat`
- `otio-cmx3600-adapter` 1.0.0, `otio-aaf-adapter` 2.0.0, `otio-fcp-adapter` 1.0.0, `otio-fcpx-xml-adapter` 1.0.0, `pyaaf2` 1.7.1 — installed and **run**, producing the round-trip measurements in §B.2.2, §B.3.2, §B.4.2
- `opentimelineio-0.18.1.dist-info/licenses/LICENSE.txt` and `NOTICE.txt` (SHA-256 hashed and diffed against upstream and against apache.org)
- `crates.io` JSON API — `/api/v1/crates/{name}` for every crate named in this document
- PyPI JSON API — `/pypi/{name}/json` for every Python package named
- `cine-timeline` compiled and run against OTIO-generated corpora in both directions (17 single-feature files plus a combined reference timeline)
- Every FCPXML DTD v1.1–v1.15 fetched and its `<!ELEMENT>` declarations counted and diffed

**Specifications and primary documents fetched**
- OpenTimelineIO — <https://github.com/AcademySoftwareFoundation/OpenTimelineIO>; LICENSE <https://raw.githubusercontent.com/AcademySoftwareFoundation/OpenTimelineIO/main/LICENSE.txt>
- Apache License 2.0 canonical text — <https://www.apache.org/licenses/LICENSE-2.0.txt>
- **CMX 3600 reference parser (Apache-2.0)** — <https://raw.githubusercontent.com/OpenTimelineIO/otio-cmx3600-adapter/main/src/otio_cmx3600_adapter/cmx_3600.py>
- **Apple FCPXML DTDs v1.1–v1.14** — <https://github.com/CommandPost/CommandPost/tree/develop/src/extensions/cp/apple/fcpxml/dtd> (v1.14 header: *"Copyright (c) 2011-2026 Apple Inc."*)
- `pyaaf2` LICENSE (MIT) — <https://raw.githubusercontent.com/markreidvfx/pyaaf2/main/LICENSE>
- `LibAAF` LICENSE (GPL-2.0) — <https://raw.githubusercontent.com/agfline/LibAAF/master/LICENSE>
- CMX 3600 background, AAF COM API reference — via doc 09 (<https://www.edlmax.com/edlmaxhelp/Edl/maxguide.html>, <https://aaf.sourceforge.net/docs/aafapiman/>)

**Rust crates verified against the crates.io API**
- `cfb` (MIT) — <https://github.com/mdsteele/rust-cfb>
- `opentimelineio`, `opentimelineio-sys` (BSD-3-Clause, placeholders) — <https://github.com/scott-wilson/OpenTimelineIO-rs>
- `otio-types` (AGPL-3.0-or-later) — <https://github.com/ignition-is-go/pulse-pixelstream>
- `edl` (MIT, no repository declared)
- `oximedia-*` (Apache-2.0) — <https://github.com/cool-japan/oximedia>
- `timecode` (MIT) — <https://github.com/media-io/timecode>; `vtc` (MIT) — <https://github.com/opencinemac/vtc-rs>; `video-timecode` (MIT)

**CINE-OMNI sources read in full**
- `crates/cine-timeline/src/{otio.rs, otio_json.rs, edit.rs, graph.rs, lib.rs}`
- `crates/cine-media/src/time.rs`
- `docs/blueprint/research/09-editorial-media-model.md`, `02-editorial-media.md`

---

## Appendix — reproducing the OTIO measurements

Everything in Part B is reproducible in a few minutes on a machine with Python
and network access. No compiler beyond the existing Rust toolchain is needed —
`opentimelineio` ships a manylinux wheel.

```bash
# 1. environment
uv venv otioenv
uv pip install --python otioenv/bin/python \
    opentimelineio otio-cmx3600-adapter otio-aaf-adapter \
    otio-fcp-adapter otio-fcpx-xml-adapter

# 2. the real schema versions and version sets
otioenv/bin/python -c "
import opentimelineio as otio, opentimelineio.versioning as v
print(otio.core.type_version_map())
print(list(v.full_map()['OTIO_CORE']))
print(otio.adapters.available_adapter_names())"

# 3. licence, verified by hash rather than by trust
sha256sum otioenv/lib/python3.*/site-packages/opentimelineio-*.dist-info/licenses/LICENSE.txt
curl -sS https://raw.githubusercontent.com/AcademySoftwareFoundation/\
OpenTimelineIO/main/LICENSE.txt | sha256sum
```

To reproduce the two-direction round trip, build a timeline in Python OTIO,
write it, read it with `cine-timeline`, write it back, and re-read it in Python.
The probe used here is kept outside the repository at
`/tmp/.../scratchpad/otio_probe_example.rs`; when the harness of §B.1.7 Step 1
is built it should live in the repository as a proper integration test that
skips loudly when Python or the wheel is absent — the same `need!` discipline
`cine-media` already applies to real media.

**The single most valuable thing in this appendix** is that the whole
verification loop is cheap. The reason `cine-timeline` shipped with the defects
in §B.1.5 is not that they were hard to find; it is that nothing was ever run
against a file real OTIO had written.
