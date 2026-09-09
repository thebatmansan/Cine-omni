# Exact time migration — usage map, measured hazards, canonical type, staged plan

**Status:** design document. Read-only analysis; no code in `crates/` or `ui/`
was changed to produce it.

**Scope.** CINE-OMNI carries two incompatible time models:

| | Type | Representation | Location |
|---|---|---|---|
| Editorial / graph | `RationalTime { value: f64, rate: f64 }`, `TimeRange` | two IEEE-754 doubles | `crates/ucg-core/src/time.rs:10-14`, `:144-148` |
| Media | `TimeBase { num: u32, den: u32 }`, `Timestamp { ticks: i64, base }`, `FrameRate`, `Timecode` | exact integers over exact rationals | `crates/cine-media/src/time.rs:51-55`, `:117-121`, `:250-268`, `:344-352` |

The bridge between them is one-directional and lossy:
`cine_media::time::to_ucg_time` (`crates/cine-media/src/time.rs:493-498`)
converts an exact `Timestamp` into `RationalTime::new(frames as f64,
rate.as_f64())` — the exact rational rate is discarded at the boundary and
replaced by its `f64` approximation. There is no `from_ucg_time`.

`docs/blueprint/PHASE-3-STATUS.md:130` already records this as the project's
largest open architectural defect.

> **Rules of evidence used here.** Every number in §2 was produced by running a
> program against the real crates (scratch cargo project at `/tmp/timeprobe`,
> depending on `ucg-core`, `cine-media` and `cine-timeline` by path,
> `cargo build --release`). Anything not measured is marked `UNVERIFIED`.

---

## 1. Usage map

### 1.1 Reference counts

Measured with `grep -roE '\bRationalTime\b|\bTimeRange\b'` over `crates/`
(every occurrence, including doc comments and the defining file):

| Crate | `src/` | `tests/` | total |
|---|---:|---:|---:|
| `cine-timeline` | 196 | 74 | **270** |
| `ucg-core` | 72 | 2 | **74** |
| `cine-audio` | 18 | 5 | **23** |
| `cine-pipeline` | 3 | 6 | **9** |
| `cine-comp` | 2 | 6 | **8** |
| `ucg-schema` | 6 | 0 | **6** |
| `cine-script` | 6 | 0 | **6** |
| `cine-previs` | 5 | 0 | **5** |
| `cine-media` | 3 | 0 | **3** |
| `cine-logistics` | 2 | 0 | **2** |
| `cine-cli`, `cine-shell`, `cine-gpu`, `cine-color` | 0 | 0 | **0** |
| **Total** | **313** | **93** | **406** |

The often-quoted figure of "221 references across 10 crates"
(`docs/blueprint/PHASE-3-STATUS.md:130`) is an undercount by this counting
method; the crate *set* (10 crates) is correct. `ui/` contains **zero**
references — `ui/src/**` and `ui/server/bridge.mjs` never name a rate or a time
value, so the TypeScript side is unaffected by the representation change
(it will be affected only if the JSON wire shape of `Value::Time` changes;
see §2.6).

### 1.2 Per-file, heaviest first

| File | refs | Why it is load-bearing |
|---|---:|---|
| `crates/cine-timeline/src/otio.rs` | 83 | The OTIO object model. Every public struct field and accessor. |
| `crates/ucg-core/src/time.rs` | 59 | The definition. |
| `crates/cine-timeline/src/edit.rs` | 48 | All NLE edit operations. **The hardest file** (§1.4). |
| `crates/cine-timeline/src/otio_json.rs` | 37 | OTIO JSON reader/writer — the interchange boundary. |
| `crates/cine-timeline/tests/otio_fixture.rs` | 33 | Fixture round-trip test. |
| `crates/cine-timeline/tests/graph_roundtrip.rs` | 24 | Timeline ↔ UCG graph round trip. |
| `crates/cine-audio/src/session.rs` | 13 | Audio session ranges, built from **seconds**. |
| `crates/cine-timeline/src/graph.rs` | 11 | Timeline → UCG graph writer (`attrs::RANGE`, `attrs::SOURCE_RANGE`). |
| `crates/ucg-core/src/value.rs` | 9 | **Persistence boundary**: `Value::Time` / `Value::Range`. |
| `crates/cine-timeline/src/lib.rs` | 9 | Re-export + doctest. |
| `crates/cine-timeline/tests/sync_offset.rs` | 7 | |
| `crates/cine-script/src/sync.rs` | 6 | Word alignment → frame ranges, built from **seconds**. |
| `crates/cine-pipeline/tests/thesis.rs` | 6 | Cross-module thesis test. |
| `crates/cine-comp/tests/ucg_roundtrip.rs` | 6 | |
| `crates/cine-timeline/tests/{media_reference_integrity,edit_ops}.rs` | 5, 5 | |
| `crates/cine-timeline/src/sync.rs` | 5 | |
| `crates/cine-previs/src/graph.rs` | 5 | Dialogue track ranges, built from **seconds**. |
| `crates/cine-audio/{tests/splice_continuity.rs,src/ucg.rs}` | 5, 5 | |
| `crates/ucg-schema/src/components.rs` | 4 | `ClipRef { source_range: TimeRange, track_start: RationalTime }` |
| `crates/ucg-core/src/lib.rs` | 4 | Public re-export (`:35`, `:45`). |
| `crates/cine-timeline/src/error.rs` | 3 | Two error variants carry a `RationalTime`. |
| `crates/cine-pipeline/src/reactors.rs` | 3 | Reactor writes a range from an **estimated duration in seconds**. |
| `crates/cine-media/src/time.rs` | 3 | The one-way bridge. |
| `crates/ucg-schema/src/lib.rs` | 2 | Re-export (`:20`). |
| `crates/ucg-core/tests/engine_tests.rs` | 2 | |
| `crates/cine-logistics/src/metadata.rs` | 2 | Timecode formatting only (`:495`). |
| `crates/cine-comp/src/ucg.rs` | 2 | `update_frame_range` (`:224`). |

### 1.3 Classification by what the site actually does

Counts over `crates/*/src` only, excluding the defining file
`crates/ucg-core/src/time.rs`.

**(a) Construction — 98 sites**

| Constructor | sites | Notes |
|---|---:|---|
| `RationalTime::new(value, rate)` | 30 | raw pair; the migration must find a rate for each |
| `RationalTime::from_frames(frames, rate)` | 23 | **the safe case** — integer frames, maps 1:1 to the exact model |
| `RationalTime::from_seconds(seconds, rate)` | 11 | **the dangerous case** — produces non-integer `value` (§2.5) |
| `RationalTime::from_timecode` | 0 | never called outside tests |
| `TimeRange::new(start, duration)` | 18 | rescales `duration` to `start`'s rate (`time.rs:151-156`) |
| `TimeRange::from_frames(start, dur, rate)` | 15 | safe case |
| `TimeRange::from_start_end` / `::default` | 0 / 1 | |

The eleven `from_seconds` call sites are the ones that inject sub-frame values
into the graph, and they are all outside `cine-timeline`:

* `crates/cine-audio/src/session.rs:402,403,419,420,680,681`
* `crates/cine-audio/src/ucg.rs:195,196`
* `crates/cine-previs/src/graph.rs:316,317`
* `crates/cine-pipeline/src/reactors.rs:525`

**(b) Arithmetic / rescale — 22 `rescaled_to` sites, all but two in one file**

20 of the 22 are in `crates/cine-timeline/src/edit.rs` (lines 78, 82, 94, 107,
161, 202, 221, 222, 226, 272, 311, 327, 351, 411, 421, 432, 463, 485, 491, 506);
the other two are `crates/cine-timeline/src/otio.rs:361,364` inside
`Track::validate`. Every one of them is immediately followed by `.value`, i.e.
the code drops back to raw `f64` frame counts as soon as it has rescaled.

`Add` / `Sub` are used implicitly in accumulation loops:
`crates/cine-timeline/src/otio.rs:251` (`Track::duration`), `:268`
(`range_of_child`), `:292` (`child_at_time`), `:707` (`Clip::media_time_at`),
`crates/cine-timeline/src/edit.rs:392,463,491`.

**(c) Comparison / equality / hashing — the invisible category**

`RationalTime` derives `PartialEq, PartialOrd` only
(`crates/ucg-core/src/time.rs:10`). It has **no `Eq`, no `Ord`, no `Hash`**,
because `f64` has none. Consequences that are structural, not stylistic:

* `Value` (`crates/ucg-core/src/value.rs:31`) therefore derives `PartialEq`
  only. It cannot be a `HashMap` key, cannot be `Eq`, and cannot be hashed for
  content addressing.
* `Tx::set` decides whether a write is a no-op with
  `prev.as_ref() == Some(&value)` (`crates/ucg-core/src/tx.rs:266`). That is
  `Value: PartialEq`, i.e. `f64` equality. **Reactor idempotence — the
  invariant in `CLAUDE.md` and `docs/MODULE-CONVENTIONS.md` rule 4 — is
  enforced by float equality.** Measured failure rate in §2.4.
* Because exact comparison is unavailable, the code substitutes epsilons:
  `const EPS: f64 = 1e-9` at `crates/cine-timeline/src/edit.rs:36`, used at
  `:313, :321, :328, :383, :416, :433, :467, :492`; two more bare `1e-9`
  literals in `Track::validate` at `crates/cine-timeline/src/otio.rs:366-368`;
  and `RationalTime::almost_equal(eps_seconds)` (`time.rs:79-81`), whose only
  non-test caller is `crates/cine-timeline/src/edit.rs:383`.

**(d) Serialization — persistence boundary, see §2.6**

`Value::Time(RationalTime)` / `Value::Range(TimeRange)`
(`crates/ucg-core/src/value.rs:51-52`), reachable from:
`from_impl!(RationalTime, Time, ...)` and `from_impl!(TimeRange, Range, ...)`
(`:258-259`), `Value::as_time` (`:131`), `Value::as_range` (`:137`).
Attributes that carry them: `attrs::RANGE`, `attrs::SOURCE_RANGE`,
`attrs::AVAILABLE_RANGE` (`crates/ucg-schema/src/lib.rs:59-64`), written by
`crates/cine-timeline/src/graph.rs:353,359` and read by
`clip_track_range` (`graph.rs:710`).
Typed component `ClipRef` (`crates/ucg-schema/src/components.rs:205-209`)
serializes a `TimeRange` and a `RationalTime` through serde.

**(e) Public API surface**

42 `pub fn` signatures and 9 public struct fields outside
`ucg-core/src/time.rs` mention the types. The public fields are the hard part,
because they are `pub` *data*, not accessors:

```
crates/cine-timeline/src/otio.rs:32    pub global_start_time: Option<RationalTime>
crates/cine-timeline/src/otio.rs:586   pub source_range: Option<TimeRange>       (Clip)
crates/cine-timeline/src/otio.rs:722   pub source_range: TimeRange               (Gap)
crates/cine-timeline/src/otio.rs:775   pub in_offset: RationalTime               (Transition)
crates/cine-timeline/src/otio.rs:776   pub out_offset: RationalTime              (Transition)
crates/cine-timeline/src/otio.rs:882   pub marked_range: TimeRange               (Marker)
crates/cine-audio/src/session.rs:380   pub range: TimeRange
crates/ucg-schema/src/components.rs:206 pub source_range: TimeRange              (ClipRef)
crates/ucg-schema/src/components.rs:208 pub track_start: RationalTime            (ClipRef)
```

Two error variants also carry the type and so appear in every `Result`:
`Error::TimeOutOfRange(RationalTime)` and `Error::TransitionAtCut(RationalTime)`
(`crates/cine-timeline/src/error.rs:24,30`).

**(f) Display / formatting only — 3 sites**

`RationalTime::to_timecode` is called once outside tests, at
`crates/cine-logistics/src/metadata.rs:495`. `Display` impls at
`crates/ucg-core/src/time.rs:137-141, 226-230` are used by `Value::Display`
(`value.rs:227-228`) and by the error messages in `Track::validate`.

**(g) Conversion to/from `f64` seconds — 12 `to_seconds()` sites**

Plus the 11 `from_seconds` constructors of (a). `TimeRange::to_seconds`
(`time.rs:221-223`) returns a `(f64, f64)` pair.

### 1.4 The direct-representation dependency: `.value` / `.rate`

This is the true migration cost, because these sites do not go through any API
that can be preserved — they read the `f64` fields:

| File | `.value` + `.rate` accesses |
|---|---:|
| `crates/cine-timeline/src/edit.rs` | **57** |
| `crates/cine-timeline/src/otio.rs` | **18** |
| `crates/cine-timeline/src/sync.rs` | 2 (`:266`, `:270`) |
| `crates/cine-timeline/src/otio_json.rs` | 2 (`:371`) |
| `crates/cine-script/src/sync.rs` | 1 (test, `:344`) |
| **Total** | **80** |

(`crates/cine-media/src/reader.rs` and `crates/cine-previs/src/rig.rs` also
match the pattern but are false positives — Symphonia `spec().rate` and
keyframe `.value` respectively.)

`edit.rs` is not merely the biggest user; it is written *in raw `f64` frames*.
Its internal helpers have `f64` in their signatures:
`tail_bounds(...) -> Result<(f64, f64, TimeRange)>` (`:89`),
`head_bounds` (`:102`), `clamp(d: f64, lo: f64, hi: f64) -> f64` (`:113`),
`set_tail(..., d: f64)` (`:122`), `set_head(..., d: f64)` (`:130`),
`pad_to(...) -> f64` (`:429`). Migrating `edit.rs` is a rewrite of its
arithmetic core, not a type substitution.

The module doc at `crates/cine-timeline/src/edit.rs:6-7` states:

> "Frame arithmetic is exact because `RationalTime` stores integer frame counts
> as `f64`."

That claim is **true only at a single rate** (measured: §2.3, same-rate column)
and **false the moment two rates meet** (§2.2, §2.7). It should be corrected as
part of the migration.

---

## 2. Measured hazards

All figures below are output of programs run against the real crates. The
harness is reproduced in §2.10.

### 2.1 The reference failure reproduces exactly

`docs/blueprint/research/10-color-and-sync.md` §B.1.2 reported 24.2 % `PartialEq`
failures and 12.1 % wrong frames for 200 000 frames at 24000/1001 rescaled to
25 fps and back. Re-run today against the current `ucg-core`:

```
## R1  Exact reproduction of research/10 B.1.6 probe
  n=200000  PartialEq fail=48408 (24.2 %)  to_frames wrong=24201 (12.1 %)
    frame 27 -> value 26.999999999999996 -> to_frames() = 26
    frame 41 -> value 40.99999999999999  -> to_frames() = 40
    frame 43 -> value 42.99999999999999  -> to_frames() = 42
    frame 45 -> value 44.99999999999999  -> to_frames() = 44
  control (cine_media exact, rescale->25->back): wrong=0 (0.0000 %)
```

**The defect is intact and reproduces bit for bit.** It fails at frame 27 — not
after an hour. The exact model gets 0 wrong on the identical workload.

A two-level nest is worse:

```
## R2  Two-level nest (23.976 -> 24 -> 25 -> back)
  n=200000  PartialEq fail=70228 (35.1 %)  to_frames wrong=35120 (17.6 %)
```

### 2.2 Media → editorial round trip: wrong-frame rate by rate

The realistic path: `cine-media` produces an exact `Timestamp` for frame *i*;
the editorial layer receives seconds and reconstructs the frame index. One hour
of material at each rate, `to_frames()` (floor, as `time.rs:58-60` does):

| Rate as written in code | frames in 1 h | wrong | % | first wrong |
|---|---:|---:|---:|---|
| `23.976` (decimal literal) | 86 313 | 86 312 | **100.00 %** | frame 1 → 0 |
| `24000.0/1001.0` | 86 313 | 0 | 0.00 % | — |
| `24.0` | 86 400 | 0 | 0.00 % | — |
| `25.0` | 90 000 | 4 586 | **5.10 %** | frame 29 → 28 |
| `29.97` (decimal literal) | 107 892 | 107 891 | **100.00 %** | frame 1 → 0 |
| `30000.0/1001.0` | 107 892 | 11 258 | **10.43 %** | frame 15 → 14 |
| `30.0` | 108 000 | 1 632 | **1.51 %** | frame 123 → 122 |
| `47.952` (literal) | 172 627 | 172 626 | **100.00 %** | frame 1 → 0 |
| `48000.0/1001.0` | 172 627 | 0 | 0.00 % | — |
| `48.0` | 172 800 | 0 | 0.00 % | — |
| `50.0` | 180 000 | 9 174 | **5.10 %** | frame 29 → 28 |
| `59.94` (literal) | 215 784 | 215 783 | **100.00 %** | frame 1 → 0 |
| `60000.0/1001.0` | 215 784 | 22 485 | **10.42 %** | frame 15 → 14 |
| `60.0` | 216 000 | 3 270 | **1.51 %** | frame 123 → 122 |

Three separate findings here:

1. **A decimal rate literal is catastrophic, not marginal.** `23.976`, `29.97`,
   `47.952`, `59.94` put essentially *every* frame on the wrong index — 100 %,
   starting at frame 1 — because the rate itself is wrong by 1 × 10⁻⁶ relative
   and the resulting `value` is below the integer by more than one ULP from the
   first frame onward. Measured: a grep for `23.976|29.97|47.952|59.94|119.88`
   across `crates/*/src` and `crates/*/tests` finds exactly one *code* site
   today — `crates/cine-timeline/src/otio_json.rs:513`,
   `RationalTime::new(1001.0, 29.97)`, inside `#[cfg(test)]` — plus three
   prose mentions in `crates/cine-audio`. So the fuse is not lit; but nothing
   prevents it, because `RationalTime::from_frames` takes an `f64` rate with no
   validation, and the very next OTIO file or EDL that carries a rounded rate
   will light it.
2. **Even the correct `f64` rate fails at NTSC**: 10.4 % wrong at
   `30000/1001` and `60000/1001`. 23.976 and 47.952 happen to survive because
   `24000/1001` and `48000/1001` round favourably; that is luck, not a property.
3. **Integer rates are not safe either**: 30 fps and 60 fps are wrong 1.5 % of
   the time, 25 and 50 fps 5.1 %, because the *seconds* value coming from an
   exact source is not exactly representable and the floor drops below.

The failure is the `floor`. Rounding instead:

```
## H2b Same round-trip but with .round() instead of to_frames() floor
  every rate: wrong=0 (0.00%)
```

That is not a fix: `floor` is *semantically correct* for "which frame is on
screen at time *t*", and `Timestamp::frame_index`
(`crates/cine-media/src/time.rs:185-194`) rightly floors too. The problem is
flooring an inexact number. An exact model floors an exact number and is right.

### 2.3 Rate literals: relative error and drift

```
## H1  Rate literal vs exact rational
  23.976   f64=23.97599999999999909  exact=24000/1001=23.97602397602397772  rel=1.000e-6  drift/1h=-0.086 frames
  29.97    f64=29.96999999999999886  exact=30000/1001=29.97002997002996949  rel=1.000e-6  drift/1h=-0.108 frames
  47.952   f64=47.95199999999999818  exact=48000/1001=47.95204795204795545  rel=1.000e-6  drift/1h=-0.173 frames
  59.94    f64=59.93999999999999773  exact=60000/1001=59.94005994005993898  rel=1.000e-6  drift/1h=-0.216 frames
  24, 25, 30, 48, 50, 60: rel=0, drift=0
```

Sub-frame per hour, i.e. invisible inside a shot and a whole frame across a
broadcast day — which matches `research/10` §B.1.4(a). The reason a literal
still produced a **100 %** wrong-frame rate in §2.2 is not the drift; it is that
dividing exact seconds by a slightly-too-small rate lands *just below* the
integer, and `floor` takes it down.

### 2.4 Equality after rescale, and reactor idempotence

```
## H3  t.rescaled_to(r2).rescaled_to(r1) == t ?
  24            <-> 23.976            n=   1440   fail=   154 (10.69 %)
  24            <-> 23.976023976..    n=   1440   fail=   146 (10.14 %)
  30            <-> 29.97             n=   1800   fail=   131 ( 7.28 %)
  30            <-> 29.97002997..     n=   1800   fail=   151 ( 8.39 %)
  29.97002997.. <-> 59.94005994..     n=   1798   fail=    57 ( 3.17 %)
  25            <-> 50                n=   1500   fail=   143 ( 9.53 %)
  48000 audio   <-> 23.976023976..    n=2880000   fail=728965 (25.31 %)
  44100 audio   <-> 24                n=2646000   fail=673543 (25.46 %)
  96000 audio   <-> 29.97002997..     n=5760000   fail=967644 (16.80 %)
```

Note **25 ↔ 50 fails 9.5 %** — this is not an NTSC-only problem. Doubling and
halving an integer rate through `value/rate*rate'` is not identity in `f64`.

Wired into the graph:

```
## R6  Reactor idempotence
  Value::Time inequality after one 25 fps round trip: 48408 of 200000 (24.2 %)
```

`Tx::set` (`crates/ucg-core/src/tx.rs:266`) suppresses a write when
`prev == value`. 24.2 % of the time it will *not* suppress, will emit a
`SetAttr` op for a value that did not change, dirty the downstream nodes, and
re-enter the reactor cascade. **Float time in the graph is a latent
non-convergence bug in the reactor cascade**, and it is also 24.2 % of spurious
op-log growth — every spurious op is written to `oplog/current.jsonl` by
`append_delta` (`crates/ucg-core/src/persist.rs:229-240`) and becomes permanent
undo history.

### 2.5 `from_seconds` producers put values between frames

Four crates construct editorial time from a duration in seconds. Measured with
a 2.5 s estimate — the shape of
`crates/cine-pipeline/src/reactors.rs:525` and
`crates/cine-previs/src/graph.rs:317`:

```
## H10 from_seconds(2.5, rate) -> value  /  fract  /  to_frames()
  23.976023976..  ->  59.940059940   fract 0.940   to_frames() = 59
  24              ->  60.000000000   fract 0.000   to_frames() = 60
  25              ->  62.500000000   fract 0.500   to_frames() = 62
  29.97002997..   ->  74.925074925   fract 0.925   to_frames() = 74
  30              ->  75.000000000   fract 0.000   to_frames() = 75
  59.94005994..   -> 149.850149850   fract 0.850   to_frames() = 149
```

These `RationalTime`s are then stored in the graph as `attrs::RANGE`
(`crates/cine-pipeline/src/reactors.rs:523-526`) with a fractional `value`.
Anything downstream that floors them loses up to one frame; anything that
compares them to a frame-aligned value is unequal. The audio path is worse: a
sample-aligned 48 kHz instant rescaled to any video rate lands off-frame at
every rate measured, including the integer ones (`## H10`, second block).

**This is the category the migration must handle first, because an exact
integer type has nowhere to put `74.925074925`.** It must either round to a
frame at a stated rounding mode, or hold the value at a finer base (e.g. the
48 kHz audio base), which the exact model supports natively and the float model
does not distinguish.

### 2.6 Serialization — what is actually on disk

Real op-log line, written by `append_delta` into
`<project>/oplog/current.jsonl`, for a clip at 30000/1001:

```json
{"revision":1,"label":"seed","ops":[
 {"op":"create_node","node":{"id":"fe42…","kind":"nle.clip","attrs":{},"created_rev":0,"modified_rev":0}},
 {"op":"set_attr","id":"fe42…","key":"range",
  "value":{"t":"range","v":{"start_time":{"value":0.0,"rate":29.97002997002997},
                            "duration":{"value":240.0,"rate":29.97002997002997}}},"prev":null},
 {"op":"set_attr","id":"fe42…","key":"track_start",
  "value":{"t":"time","v":{"value":107892.0,"rate":29.97002997002997}},"prev":null}
],"actor":"local","lamport":1,"cause":null,"depth":0,"kind":"normal"}
```

and the same in `graph.json`:

```
  "t": "range",
      "value": 0.0,
      "rate": 29.97002997002997
      "value": 240.0,
      "rate": 29.97002997002997
  "t": "time",
      "value": 107892.0,
      "rate": 29.97002997002997
```

Facts established:

* **`RationalTime` is serialized structurally**, as `{"value": f64, "rate":
  f64}` inside the externally-tagged `{"t":"time","v":…}` envelope
  (`crates/ucg-core/src/value.rs:31-32`), not as a bare float. The same shape
  goes into `graph.msgpack` via `rmp_serde::to_vec_named`
  (`crates/ucg-core/src/persist.rs:70-75`).
* **The `f64` rate round-trips bit-exactly through `serde_json`.** Verified:
  `24000/1001 → "23.976023976023978" → bit-equal=true`, and the same for
  30000/1001, 48000/1001, 60000/1001. So JSON is not *adding* error. It is
  faithfully recording a representation that was already wrong.
* **`SCHEMA_VERSION = 1`** (`crates/ucg-core/src/persist.rs:23`) and the
  migration ladder `fn migrate(graph, from)` (`:96-105`) is a working but empty
  loop with the comment `// Future: match v { 1 => migrate_1_to_2(&mut g), … }`.
  **The forward-migration mechanism exists and has never been used.** This
  migration is its first customer.
* `check_version` (`:85-93`) refuses to open a file *newer* than the binary
  (`UcgError::SchemaVersion`) but accepts anything older and runs `migrate`.
  So a v1→v2 bump is safe for old files and correctly refuses new files in old
  binaries.

**What a representation change does to existing project files.** Changing
`Value::Time`'s payload from `{value: f64, rate: f64}` to, say,
`{value: i64, rate: {num: u32, den: u32}}` makes every existing `graph.json`,
`graph.msgpack` and `oplog/*.jsonl` unreadable at the serde layer — and the
op-log is *replayed*, not just snapshotted (`Project::open`,
`crates/ucg-core/src/persist.rs:139-166`), so a project fails to open rather
than degrading. This is not optional to handle; see Stage 4 in §4.

Two further consequences, both currently invisible:

* `RationalTime` has no `Eq`/`Hash`, so `Value` has none, so **there is no
  content hash of a graph attribute**. Any future content-addressed cache or
  merge key over `Value` is blocked until this changes.
* The OTIO JSON writer emits `"rate": t.rate` and `"value": t.value` verbatim
  (`crates/cine-timeline/src/otio_json.rs:371`). OTIO's own schema is a
  `double` pair, so this is *correct interchange* and must stay a `double` pair
  on the wire even after the internal type becomes exact.

### 2.7 `+` uses the left-hand rate; OTIO uses the higher rate

`crates/ucg-core/src/time.rs:117-126`:

```rust
impl Add for RationalTime {
    fn add(self, rhs: Self) -> Self {
        let rhs = rhs.rescaled_to(self.rate);           // <-- LEFT-HAND rate
        RationalTime { value: self.value + rhs.value, rate: self.rate }
    }
}
```

`Sub` (`:127-136`) does the same. OpenTimelineIO's `RationalTime::operator+`
rescales to the **higher** of the two rates, precisely so that adding a coarse
time to a fine one does not quantise the fine one away. Measured divergence:

```
## H5
  48@24 + 24@48    ucg a+b = 60.000@24     ucg b+a = 120.000@48    OTIO = 120.000@48
  24@48 + 48@24    ucg a+b = 120.000@48    ucg b+a =  60.000@24    OTIO = 120.000@48
  1@23.976023976.. + 1@48000 audio
                   ucg a+b = 1.0004995@23.976024   (0.041729167 s)
                   ucg b+a = 2003.000000@48000     (0.041729167 s)
                   OTIO    = 2003.000000@48000     (0.041729167 s)
  1@30 + 1@29.97002997..
                   ucg a+b = 2.001000000@30        ucg b+a = 1.999000999@29.97   OTIO = 2.001000000@30
  a+b == b+a ?  false in all four cases
```

What this breaks, concretely:

* **`+` is not commutative.** `a + b != b + a` for every mixed-rate pair
  measured. Any code that reorders an accumulation — and `Track::duration`
  (`crates/cine-timeline/src/otio.rs:247-256`) accumulates in child order,
  while `range_of_child` (`:262-279`) re-accumulates a prefix — can produce
  two different values for the same instant.
* **The coarse rate wins, so precision is destroyed silently.** `1 frame at
  23.976 + 1 sample at 48 kHz` becomes `1.0004995 frames at 23.976` under
  `ucg_core` — the audio sample has been folded into a fractional video frame.
  Under OTIO it stays `2003 samples at 48 kHz`, exact. The seconds happen to
  agree here (both `0.041729167 s`) because a single operation is still within
  `f64` slack, but the *representation* is now unrepresentable-as-frames and
  the next `floor` will discard it.
* **A silent divergence from OTIO.** `crates/cine-timeline/src/otio_json.rs`
  presents itself as an OTIO reader/writer, so a timeline that round-trips
  through OTIO and is recomputed in `cine-timeline` can differ from the same
  timeline recomputed in `otiotool`, with no error anywhere.

`Track::duration()` accumulation is measurably safe *at one rate* and unsafe
across rates:

```
## H4  accumulated addition
  same-rate, 1 h at every rate tested:  err = +0.000e0 frames   (integers in f64 are exact)
  mixed rate, 3600 clips of 24 frames at 23.976 summed into a 24 fps track:
      acc = 86486.400000002   exact = 86486.400000000   err = +1.7e-9 frames
  ... into a 30 fps track:
      acc = 108107.999999997  exact = 108108.000000000  err = -3.4e-9 frames   <-- floors to 108107
```

The magnitudes are tiny; the *consequence* is not. `-3.4e-9` sits just below an
integer, and `to_frames()` floors it to **108107 instead of 108108** — a
one-frame-short timeline. This is the same mechanism as §2.1 at the scale of a
whole sequence, and it is why `edit.rs` needs `EPS = 1e-9` at all.

### 2.8 `contains()` at edit boundaries

`TimeRange::contains` (`crates/ucg-core/src/time.rs:178-181`) rescales the probe
and compares `.value`. Probing the *start instant* of each 1-frame range with a
time expressed at another rate:

```
## H6
  range@23.976023976..  probe@48000 audio      n=  86313  start-instant miss= 14615 (16.93 %)
  range@29.97002997..   probe@48000 audio      n= 107892  start-instant miss= 10074 ( 9.34 %)
  range@29.97           probe@29.97002997..    n= 107892  start-instant miss=  5137 ( 4.76 %)
  range@24              probe@23.976023976..   n=  86400  start-instant miss=  4470 ( 5.17 %)
  (midpoint probes: 0 misses everywhere — the failure is exactly at the boundary)
```

`Track::child_at_time` (`crates/cine-timeline/src/otio.rs:289-303`) walks the
track calling `contains`. **At a cut, 9–17 % of the time it reports the wrong
clip** when the playhead is expressed at a different rate from the clip — which
is exactly what happens when an audio-rate playhead scrubs a video track. The
midpoint result shows this is not general imprecision: it is a boundary defect,
and boundaries are where editors work.

Related boundary defect, unconditional: an empty range reports an inclusive end
*before* its start —

```
  TimeRange::from_frames(10, 0, 24.0).end_time_inclusive() = 9.0
```

(`crates/ucg-core/src/time.rs:171-177` subtracts 1.0 unconditionally.)

### 2.9 Timecode is non-drop only, and mislabels NTSC

`RationalTime::to_timecode` (`crates/ucg-core/src/time.rs:84-100`) computes
`rate.round().max(1.0)` and emits `HH:MM:SS:FF` with `:` always. There is no
drop-frame. `from_timecode` (`:103-114`) *accepts* `;` as a separator
(`.split([':', ';'])`, line 105) — the drop-frame marker — and then ignores it.

```
## H7  ucg_core::to_timecode vs SMPTE (cine_media::Timecode)
  29.97 NDF    sampled 15414   mismatch     0 ( 0.00 %)
  29.97 DF     sampled 15414   mismatch 14900 (96.67 %)   first: frame 3598  SMPTE 00:02:00;00  ucg 00:01:59:28
  59.94 DF     sampled 30827   mismatch 29799 (96.67 %)   first: frame 7196  SMPTE 00:02:00;00  ucg 00:01:59:56
  23.976 NDF   sampled 12331   mismatch     0 ( 0.00 %)

  one hour of 29.97 material (107892 frames):
    SMPTE NDF -> 00:59:56:12
    SMPTE DF  -> 01:00:00;00
    ucg_core  -> 00:59:56:12

  from_timecode("01:00:00:00", 29.97)  -> value 108000 @29.97   (rate rounded to 30)
  from_timecode("00:01:00;02", 29.97)  -> value 1802            ';' accepted, drop-frame IGNORED
  SMPTE       "00:01:00;02" DF         -> frame 1800            (2 frames apart)
```

`ucg_core` agrees with SMPTE for non-drop and disagrees with **96.67 %** of
drop-frame labels. Parsing a drop-frame timecode string silently yields a frame
number **2 frames off at one minute**, growing to 108 frames at one hour. Every
professional NTSC deliverable is drop-frame. `cine-media` already implements
this correctly (`crates/cine-media/src/time.rs:344-464`); `ucg-core` does not,
and `crates/cine-logistics/src/metadata.rs:495` calls the wrong one.

### 2.9b The exact model has its own defect that must be fixed on the way down

`cine_media::time::Timestamp` derives `PartialEq, Eq, Hash`
(`crates/cine-media/src/time.rs:117`) over the `(ticks, base)` pair, but
implements `Ord` by exact cross-multiplication (`:227-233`). These disagree:

```
## H9
  a = 48000@1/48000 (1 s), b = 90000@1/90000 (1 s)
  a == b (derived PartialEq) = false
  a.cmp(&b) (exact Ord)      = Equal
  hash(a) == hash(b)         = false
  frame 24 @24fps == 1 s ?   ==: false   cmp: Equal
```

`Eq`, `Ord` and `Hash` are **mutually inconsistent**. A `BTreeMap<Timestamp,_>`
and a `HashMap<Timestamp,_>` will disagree about whether a key is present; a
`HashSet` will hold the same instant twice under two bases. This is latent
today because nothing mixes bases in a keyed collection
(`UNVERIFIED` — not exhaustively checked), but it must be fixed *before* the
type becomes the project-wide time type, not after.

The fix is to normalise: reduce `(ticks, base)` to lowest terms in the
constructor and hash/compare the reduced triple, so equal instants are `==`,
`Ordering::Equal` and hash-equal. (`research/10` §B.1.5 rule 5 says the same.)

### 2.10 The harness

Scratch project, not committed: `/tmp/timeprobe`, `Cargo.toml` depending on
`ucg-core`, `cine-media`, `cine-timeline` by path plus `serde_json`;
`src/main.rs` (H1–H11), `src/bin/probe2.rs` (R1–R6),
`src/bin/probe3.rs` (op-log dump). Built with
`cargo build --release`. The `research/10` §B.1.6 probe, reproduced verbatim:

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

---

## 3. The canonical exact type

### 3.1 Requirements, and how the two existing types score

| Requirement | float `RationalTime` | `cine_media::Timestamp` |
|---|---|---|
| Exact frame identity | **no** (§2.1, §2.2) | yes |
| Exact sample identity | no | yes |
| Rational rates | no (`f64` rate) | yes (`FrameRate`, `TimeBase`) |
| No cumulative drift | same-rate only (§2.7) | yes |
| Deterministic comparison | no | **inconsistent** — `Eq` ≠ `Ord` (§2.9b) |
| Deterministic hashing | **impossible** (no `Eq`) | present but wrong (§2.9b) |
| Deterministic serialization | structural but `f64` (§2.6) | structural, integers |
| Timecode incl. drop-frame | **no** (§2.9) | yes |
| Negative time | yes | yes (`i64`, floor handled) |
| Sub-frame time | yes, as a fraction of a frame (§2.5) | yes, as a finer base — **better** |
| Audio 44.1k/48k/96k | via `from_seconds` only | native (`TimeBase::from_rate`) |
| Timeline ranges | `TimeRange` | **missing** — no range type |
| Media sample ranges | `TimeRange` (float) | missing |
| OTIO interchange | native (`double` pair) | needs a lossy bridge |

Neither type is complete. `Timestamp` is right about *values* and has no
ranges; `RationalTime`/`TimeRange` is right about *shape* and wrong about
values.

### 3.2 Where it has to live: `ucg-core`

The dependency direction settles this. `crates/cine-media/Cargo.toml` declares
`ucg-core.workspace = true`; `crates/ucg-core/Cargo.toml` depends on no
`cine-*` crate. `ucg-core` is the foundation, and `Value::Time` /
`Value::Range` (`crates/ucg-core/src/value.rs:51-52`) — the persisted attribute
representation — are defined there. **The exact type must live in `ucg-core`.**
`cine-media` cannot host the project's time type without inverting the
architecture.

### 3.3 Should it BE `cine_media::time::Timestamp`? — Yes in substance, no in spelling

**Recommendation: move `cine-media`'s implementation down into `ucg-core` and
have `cine-media` re-export it. Do not write a third implementation.** But the
move must not be verbatim; three things change in transit, and the type keeps
the editorial *name*.

**Why not a new type.** `cine_media::time` is 542 lines of already-tested exact
arithmetic — `i128` cross-multiplication (`:144-149`, `:227-233`), explicit
rescale with stated rounding (`:155-168`), `is_exact_in` (`:171-175`),
exact-rational `FrameRate` (`:270-331`), and a correct SMPTE drop-frame
`Timecode` (`:354-464`) that this analysis independently validated against
`ucg_core::to_timecode` (§2.9: agrees on 100 % of non-drop labels, and is the
one that is right on drop-frame). Rewriting that from scratch would be
re-litigating solved problems and would leave two exact implementations in one
workspace — the exact failure mode this migration exists to end.

**Why it keeps the name `RationalTime`.** `Timestamp { ticks, base }` and
`RationalTime { value, rate }` are the *same rational number written two ways*:
a base of `num/den` seconds per tick is a rate of `den/num` ticks per second.
`Timestamp::from_frame` (`crates/cine-media/src/time.rs:197-202`) already
exploits this — it builds `TimeBase::new(den, num)` so that one tick *is* one
frame. So there is no representational choice to make, only a naming one, and
the rate orientation wins on three counts:

1. It is what 38 existing editorial constructor call sites already say
   (`from_frames(n, rate)`, `TimeRange::from_frames(s, d, rate)`).
2. It is OTIO's vocabulary, and `crates/cine-timeline/src/otio_json.rs` must
   keep emitting `{"rate":…,"value":…}` on the wire regardless.
3. Container bases are expressible as rates without loss (`1/90000 s per tick`
   = `90000/1 ticks per second`), so `cine-media` loses nothing.

`TimeBase` and `Timestamp` therefore survive in `cine-media` as re-exported
aliases with base-orientation constructors, so the demux/decode code and its
tests keep compiling unchanged.

### 3.4 The type

```rust
// crates/ucg-core/src/time.rs

/// An exact rate: `num/den` units per second, reduced, both non-zero.
/// Never constructed from a decimal literal without an explicit snap (§3.6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug,
         Serialize, Deserialize)]
pub struct Rate { num: u32, den: u32 }

/// An exact instant or duration: `value / rate` seconds.
/// `value` counts ticks of `1/rate` seconds — frames at a frame rate,
/// samples at a sample rate, container ticks at a container base.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct RationalTime { value: i64, rate: Rate }   // fields become private

/// Half-open `[start, start + duration)`. Duration is a *duration*.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TimeRange { start_time: RationalTime, duration: RationalTime }

/// How a rescale that cannot be exact resolves. Never defaulted.
pub enum Rounding { Floor, Ceil, Nearest, Exact }
```

Design rules, in priority order (these are `research/10` §B.1.5's rules, with
the additions this analysis forces):

1. **Same rate → integer add/sub.** `a.value ± b.value`. Exact, total, cheap;
   the overwhelmingly common case, and it must not touch the rescale path.
2. **Different rates → rescale to an exact common rate**, computed in `i128`
   and reduced. Every production rate has denominator 1 or 1001, so the common
   rate is always small.
3. **`rescale(to, rounding)` takes the rounding mode explicitly**; there is no
   default. `Rounding::Exact` returns `Err`. `try_rescale_exact` is what
   idempotent graph writers call. (FFmpeg's `av_rescale_q_rnd` makes the same
   distinction for the same reason.)
4. **Durations are converted as durations, never by converting both
   endpoints.** Measured consequence of getting this wrong is in §2.7 and in
   `TimeRange::new`'s current behaviour (`crates/ucg-core/src/time.rs:151-156`,
   which already rescales the duration — keep that, make it exact).
5. **`Eq`, `Ord` and `Hash` agree, because the pair is normalised.** `Rate::new`
   reduces; `RationalTime::new` additionally reduces `value/rate` by their gcd
   so `24 @ 24/1` and `1 @ 1/1` are the *same value* — `==`, `Ordering::Equal`,
   and hash-equal. This is what §2.9b requires, and it is what makes `Value:
   Eq + Hash` possible for the first time.
6. **Comparison is `i128` cross-multiplication**, never conversion to seconds.
7. **`to_seconds() -> f64` survives as a lossy accessor** for display and for
   handing a number to the audio device. Nothing reads it back into state.
   `from_seconds` is replaced by
   `from_seconds_rounded(secs: f64, rate: Rate, r: Rounding)` — the rounding is
   at the call site, where the caller knows what it means (§2.5).
8. **Overflow is bounded and checked.** `Ord` cross-multiplies
   `(ticks·num)·den` in `i128`; with `i64` ticks and `u32` rate components that
   can reach 2¹²⁷. Reducing the pair in the constructor (rule 5) keeps real
   values far below that, but `checked_*` variants must exist and the
   comparison must reduce before multiplying. (`UNVERIFIED`: no overflow was
   observed in any measurement here, because reduced production values are
   ≤ 2⁴⁰; the hazard is theoretical but cheap to close.)

`FrameRate` collapses into `Rate`: the enum's `Other { num, den }` variant
already shows the general case subsumes the named ones.
`Rate::timecode_rate()` = `num.div_ceil(den)` (`crates/cine-media/src/time.rs:315-318`)
and `Rate::supports_drop_frame()` = `matches!((num, den), (30_000, 1_001) | (60_000, 1_001))`
(`:322-324`) move across unchanged. Named constants (`Rate::F23_976` etc.)
replace the variants so call sites read the same.

`Timecode` moves down with it, unchanged apart from its error type
(`cine_media::Error` → `ucg_core::UcgError`).

### 3.5 What `TimeRange` gains

`TimeRange` is currently only in `ucg-core` and is the one thing `cine-media`
lacks. Moving the exact type down gives `cine-media` an exact **media sample
range** for free — `[start_sample, +n_samples)` at 48000/1 — which is what
`available_range` should have been all along. `TimeRange` keeps its existing
surface (`contains`, `overlaps`, `intersection`, `extended_by`,
`end_time_exclusive`, `end_time_inclusive`) with two fixes forced by the
measurements:

* `contains`/`overlaps`/`intersection` compare exactly instead of comparing
  rescaled `f64`s — closing the 9–17 % boundary defect of §2.8.
* `end_time_inclusive` on an **empty** range must not return
  `start - 1` (`crates/ucg-core/src/time.rs:171-177`, measured in §2.8). It
  returns `Option<RationalTime>`, `None` when the duration is zero.

### 3.6 OTIO interchange, and reading a `f64` rate

OTIO's schema is a `double` pair and must stay one on the wire
(`crates/cine-timeline/src/otio_json.rs:370-377`). Both directions need a
stated, deterministic rule:

* **Write**: `rate.num as f64 / rate.den as f64`, `value as f64`. Exact for
  every value below 2⁵³.
* **Read**: `Rate::from_f64(r)` snaps to the nearest member of the known family
  — `{24000,30000,48000,60000,120000}/1001` and the integer rates — when
  within 1 × 10⁻⁴ relative, which comfortably captures both the correct
  `23.976023976023978` and the sloppy `23.976` (measured relative error
  1.0 × 10⁻⁶, §2.3) while never confusing 23.976 with 24. Outside the family it
  falls back to a continued-fraction approximation with denominator ≤ 1 001 000,
  and the result is `Rate`, so the sloppiness stops at the boundary. This is
  the single most valuable line of the whole migration: it turns the 100 %
  wrong-frame rate of §2.2 into 0 %.
* Non-integral `value` on read (the `from_seconds` legacy of §2.5) is handled
  in §4, stages S3a/S3b/S4.

---

## 4. Staged migration

**Principle: no big-bang.** Every stage below compiles and passes
`cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
on its own. The only irreversible step (S3b) is deliberately split from the
step that makes it possible (S3a) so there is a bake period with a real
rollback.

Test-count baseline, measured today, so each stage can be checked against it:
`cine-timeline` 66, `cine-comp` 66, `cine-audio` 52, `cine-logistics` 49,
`cine-previs` 48, `cine-media` 43, `cine-script` 38, `ucg-core` 34,
`cine-pipeline` 20, `ucg-schema` 2.

Blast radius of the exact types today (whole-word occurrences across
`crates/`): `Timestamp` 70, `TimeBase` 56, `FrameRate` 51, `Timecode` 22 —
**96.5 % of them inside `cine-media`**. The only crate outside `cine-media`
with a real `use` is `cine-shell` (`crates/cine-shell/src/viewer.rs:177,183`).
Moving the type down is therefore cheap on the media side; the cost is entirely
on the editorial side (§1.4).

---

### S0 — Make the exact model self-consistent (`cine-media` only)

**Changes.** `crates/cine-media/src/time.rs`: normalise `Timestamp` so derived
`PartialEq`/`Hash` agree with the cross-base `Ord` (§2.9b) — reduce
`(ticks, base)` by `gcd` in the constructor and implement `PartialEq`/`Hash`
over the reduced triple rather than deriving them. Nothing outside this file
changes.

**Proof.** New tests: `1 s @ 1/48000` and `1 s @ 1/90000` are `==`,
`Ordering::Equal`, and hash-equal; `frame 24 @ 24/1` equals `1 s @ 1/1`; a
`HashSet<Timestamp>` holding the same instant in five bases has len 1; a
`BTreeMap` and a `HashMap` keyed by `Timestamp` agree on membership over
10 000 generated `(ticks, base)` pairs. Existing 43 `cine-media` tests
unchanged; `crates/cine-shell/src/viewer.rs` unaffected.

**Rollback.** Revert one file. No other crate depends on the behaviour.

**Why first.** This type is about to become the whole project's time type. It
must not carry a broken equality relation into `Value` and into `Tx::set`'s
idempotence check (`crates/ucg-core/src/tx.rs:266`).

---

### S1 — `Rate` and the exact core land in `ucg-core`, switched off

**Changes.** New `Rate`, `Rounding`, and the exact `RationalTime`-shaped type
under a new module path (`ucg_core::time::exact`), plus `Rate::from_f64` (§3.6)
and lossless conversions to and from the existing float `RationalTime`. The
existing `RationalTime`/`TimeRange` are untouched; nothing in the workspace
switches. Port the adversarial suite of §5 and run it **against the new type
only**.

**Proof.** §5 suite green on the new type. The §2.1 probe re-run as a test:
`0/200000` wrong at 24000/1001 ↔ 25 (today: 24 201). Every other crate's tests
unchanged, because nothing depends on the new module.

**Rollback.** Delete the module; it has no callers.

---

### S2 — De-field: remove every direct `.value` / `.rate` access

**Changes.** The 80 sites of §1.4 stop reading the `f64` fields and go through
accessors on the *existing float type*: `frames_f64()`, `rate_f64()`, plus the
total operations that will survive the flip — `add_exact`, `sub_exact`,
`rescale(to, Rounding)`, `cmp_exact`, `contains`. **No behaviour change**: the
accessors return exactly what the fields did. The fields become
`#[deprecated]`, then private.

Per-crate, independently landable and independently green:

| Target | sites | Difficulty |
|---|---:|---|
| `cine-timeline/src/sync.rs:266,270`, `otio_json.rs:371` | 4 | trivial |
| `cine-script/src/sync.rs:344` (test) | 1 | trivial |
| `cine-timeline/src/otio.rs` | 18 | moderate — `duration()`, `range_of_child`, `child_at_time`, `media_time_at`, `validate` |
| `cine-timeline/src/edit.rs` | 57 | **hard** — rewrites the `f64` helper signatures `tail_bounds` (`:89`), `head_bounds` (`:102`), `clamp` (`:113`), `set_tail` (`:122`), `set_head` (`:130`), `pad_to` (`:429`) |

**Proof.** All references still compile; all 66 `cine-timeline` tests pass
**unchanged and with identical values** — this stage is a refactor, and any
test whose result changes means it was not one. Add a temporary CI grep
asserting zero `.value` / `.rate` field accesses outside
`crates/ucg-core/src/time.rs`.

**Rollback.** Per-crate revert; each crate is a separate commit.

**Why this is the real work.** After S2 the representation is reachable from
exactly one file. Before S2, flipping it breaks 80 sites simultaneously and
there is no green intermediate state.

---

### S3a — Flip the representation; read both formats, still **write** v1

**Changes.**

* `RationalTime` becomes `{ value: i64, rate: Rate }`, `TimeRange` follows
  (§3.4). Same names, same module, same public method names.
* Constructors taking `rate: f64` are kept and `#[deprecated]`, snapping via
  `Rate::from_f64` (§3.6). Constructors taking `Rate` are added.
* `from_seconds(secs, rate)` is `#[deprecated]` in favour of
  `from_seconds_rounded(secs, rate, Rounding)`. The deprecated one preserves
  today's meaning by holding the value at a **10⁶× finer rate**, so
  `74.925074925 @ 30000/1001` becomes `74925075 @ 30000000000/1001` —
  sub-frame precision kept to ≤ 20 ns, nothing silently snapped to a frame
  (§2.5).
* Hand-written `Serialize`/`Deserialize` for `RationalTime` and `TimeRange`
  that **accept both shapes** — v1 `{"value": f64, "rate": f64}` (snapped via
  `Rate::from_f64`) and v2 `{"value": i64, "rate": {"num":…,"den":…}}` — and
  **emit v1** for now.
* `Value` (`crates/ucg-core/src/value.rs:31`) gains `Eq + Hash`, possible for
  the first time (§2.6).

**Proof.**

* §5 adversarial suite, now against the production type.
* The §2.1, §2.2, §2.4 and §2.8 probes become regression tests, all expecting
  **0** failures where they measure 24 201 / 10.4 % / 48 408 / 14 615 today.
* `Tx::set` idempotence: 200 000 rate round-trips emit **0** ops
  (today: 48 408 — §2.4).
* Round-trip: `tests/fixtures/sample.otio` and a golden project corpus read,
  save and reload byte-identically under the v1 writer.
* Tests that *do* change: any assertion that encoded a float artefact. Each
  such change is justified in the commit message as "was wrong, now right".

**Rollback.** Full — the on-disk format has not moved. A previous binary reads
everything this one writes.

---

### S3b — Switch the writer to v2, `SCHEMA_VERSION` 1 → 2

**Changes.** `crates/ucg-core/src/persist.rs:23`: `SCHEMA_VERSION = 2`. The
serde impls emit the integer shape. `fn migrate` (`:96-105`) — a working but
never-used forward-migration ladder — gets its first rung, `migrate_1_to_2`.

**What this means for existing project files.** `Project::open`
(`crates/ucg-core/src/persist.rs:139-166`) reads a snapshot **and replays the
op log**, so `graph.json`, `graph.msgpack` *and* every `oplog/*.jsonl` line
must be readable — §2.6 shows the concrete bytes. The dual-shape reader from
S3a handles both with no conversion pass: an existing v1 project opens, and is
rewritten as v2 on the next `Project::save`. `check_version` (`:85-93`) already
refuses a v2 file in a v1 binary with `UcgError::SchemaVersion`, which is the
correct failure.

**Value migration is lossy in exactly one place, and must be reported.** A v1
`value` with a non-zero fractional part (§2.5) has no exact v2 spelling at the
stored rate. Rule: promote the rate by 10⁶ and round to nearest, and have
`migrate_1_to_2` count and log every promotion, so the operator is told
"N of M time values were sub-frame and were re-expressed at a finer rate".
Silence here would repeat the original sin.

**Proof.**

* Golden-file corpus: a v1 project saved by the *pre-migration* binary,
  checked in as a fixture, opens under the new binary, replays its op log, and
  yields a graph equal to the one the old binary produced — compared with the
  new `Value: Eq`.
* A v2 project opened by a v1 binary fails with `UcgError::SchemaVersion` and
  writes nothing.
* `cine-cli` gains `project verify` (open + replay + compare), run in CI over
  the corpus.

**Rollback.** Not free, and that is why S3a exists. Ship
`cine-cli project downgrade` (rewrite a v2 snapshot and op log in the v1 shape;
lossy only for values that were sub-frame) in the same release, and hold S3b
behind a release boundary from S3a.

---

### S4 — Fix the sub-frame producers at source

**Changes.** The 11 `from_seconds` call sites (§1.3a) get an explicit rate and
rounding:

* `crates/cine-audio/src/session.rs:402,403,419,420,680,681` and
  `crates/cine-audio/src/ucg.rs:195,196` — build at the **sample rate**
  (`Rate::new(sample_rate, 1)`), exact by construction; the seconds hop
  disappears.
* `crates/cine-previs/src/graph.rs:316,317` and
  `crates/cine-pipeline/src/reactors.rs:525` — estimated dialogue durations
  are genuinely approximate and should say so:
  `from_seconds_rounded(est, rate, Rounding::Ceil)` (an estimate should not
  truncate a syllable).
* `crates/cine-script/src/sync.rs:282-284` — aligner output in seconds; round
  to the nearest frame at the target rate, and keep exact seconds in a separate
  attribute if the precision is wanted.

**Proof.** A workspace-wide test asserts that **every** `RationalTime`
reachable from a graph attribute is frame-aligned at its own rate, or is
deliberately held at an audio/fine rate — no fractional frames anywhere.
`cine-audio` 52, `cine-previs` 48, `cine-script` 38, `cine-pipeline` 20 tests
pass; `crates/cine-audio/tests/splice_continuity.rs` is the sharpest guard.

**Rollback.** Per-crate; no format implications.

---

### S5 — De-duplicate: `cine-media` re-exports, its `time.rs` shrinks

**Changes.** `crates/cine-media/src/time.rs` loses its type definitions and
becomes `pub use ucg_core::time::{Rate as FrameRate, RationalTime as Timestamp, …};`
plus base-orientation constructors (`TimeBase::new(num, den)` →
`Rate::new(den, num)`), so the 192 `cine-media` call sites and
`crates/cine-shell/src/viewer.rs:177,183` are untouched. `to_ucg_time`
(`:493-498`) — the lossy one-way bridge — is **deleted**; there is nothing left
to bridge.

**Proof.** All 43 `cine-media` tests pass unchanged, including
`tests/real_media.rs`. The four error variants `InvalidTimeBase`,
`InvalidFrameRate`, `DropFrameUnsupported`, `InvalidTimecode`
(`crates/cine-media/src/error.rs`) move to `UcgError` with `cine_media::Error`
forwarding, so no error text changes. `cargo tree` shows no new edges.

**Rollback.** Restore one file; the types are structurally identical, so the
revert is mechanical.

---

### S6 — Timecode, including drop-frame

**Changes.** `RationalTime::to_timecode` / `from_timecode`
(`crates/ucg-core/src/time.rs:84-114`) are replaced by the moved-down
`Timecode` (`crates/cine-media/src/time.rs:344-464`), which §2.9 shows is the
one that is right. `from_timecode` must **reject** a `;` separator at a rate
that does not support drop-frame instead of silently accepting it
(`crates/ucg-core/src/time.rs:105`).
`crates/cine-logistics/src/metadata.rs:495` switches to the correct call.

**Proof.** The §2.9 divergence table becomes a test expecting **0 %** mismatch
at 29.97 DF and 59.94 DF over a full 24-hour wrap (today: 96.67 %);
`00:01:00;02 @ 29.97 DF` parses to frame **1800**, not 1802;
`01:00:00;00 @ 29.97 DF` is frame 107 892; `00:59:56:12 @ 29.97 NDF` is frame
107 892; the dropped label `00:01:00;00` is a parse **error**.

**Rollback.** Two files; the old functions can survive one release as
`to_timecode_ndf`.

---

### S7 — `edit.rs` arithmetic becomes integer; the epsilons die

**Changes.** In `crates/cine-timeline/src/edit.rs`, `tail_bounds`,
`head_bounds`, `clamp`, `set_tail`, `set_head`, `pad_to`, `split_in_place` and
`drop_transitions_on_cuts` move from `f64` frame counts to `i64` at an exact
common rate. `const EPS: f64 = 1e-9` (`:36`) and its eight uses
(`:313,321,328,383,416,433,467,492`) are deleted.
`RationalTime::almost_equal` (`crates/ucg-core/src/time.rs:79-81`) loses its
last non-test caller (`edit.rs:383`) and is deleted. The two bare `1e-9`
literals in `Track::validate` (`crates/cine-timeline/src/otio.rs:366-368`)
become exact comparisons. The false doc claim at `edit.rs:6-7` — "Frame
arithmetic is exact because `RationalTime` stores integer frame counts as
`f64`" — is corrected.

**Proof.** All 66 `cine-timeline` tests, plus the mixed-rate edit cases of §5.
Measured today, a transition offset authored at 24 fps and applied to a 23.976
clip lands off-frame on **239 of 239** trials, closest approach to a frame
boundary 9.99 × 10⁻⁴ — three orders of magnitude outside `EPS`, so `EPS` never
helped that case anyway. After S7 it must clamp to a whole frame.
`crates/cine-timeline/tests/edit_ops.rs` and `sync_offset.rs` are the guards.

**Rollback.** One crate.

---

### S8 — Mixed-rate `+`, and banning `f64` rates

**Changes.**

* `Add`/`Sub` (`crates/ucg-core/src/time.rs:117-136`) stop using the left-hand
  rate. Two options; the recommendation is **(b)**:
  * **(a)** match OTIO exactly — rescale to the higher rate. Interoperable,
    still lossy when neither rate divides the other.
  * **(b)** rescale to the **exact common rate** (§3.4 rule 2). Lossless,
    agrees with OTIO whenever one rate divides the other (the common editorial
    case), and never quantises the finer operand away — which is what §2.7
    measured `ucg_core` doing to a 48 kHz sample added to a 23.976 frame.
  Either way `a + b == b + a` becomes a test.
* Delete the deprecated `f64`-rate constructors from S3a.
* CI grep forbidding the decimal literals `23.976`, `29.97`, `47.952`, `59.94`,
  `119.88` in `crates/**/src` — §2.2 measured them producing a **100 %**
  wrong-frame rate.

**Proof.** Commutativity and associativity property tests over the §5 rate
matrix; the §2.7 divergence table re-run with `ucg == OTIO` in the
divides-evenly cases and a documented, tested divergence elsewhere;
`crates/cine-timeline/tests/otio_fixture.rs` still round-trips.

**Rollback.** One file plus the CI rule.

---

### Sequencing summary

```
S0  cine-media self-consistency                 isolated, 1 file
S1  Rate + exact core, switched off             additive, no callers
S2  de-field 80 sites                           refactor, per-crate, no behaviour change
S3a flip representation; read both / write v1   full rollback available
    ------------------ release boundary, bake ------------------
S3b write v2, SCHEMA_VERSION 2, migrate_1_to_2  + cine-cli project downgrade
S4  fix from_seconds producers                  per-crate
S5  cine-media re-exports; bridge deleted       dedup
S6  timecode + drop-frame
S7  edit.rs integer arithmetic; EPS deleted
S8  `+` semantics; f64 rates banned
```

S0–S2 are pure preparation and can land in any order relative to one another.
S3a is the hinge. S4–S8 are independent of each other and can be parallelised
once S3a has landed.

---

## 5. The adversarial test list

This is the suite the migration must pass. It is written to fail against
today's code — every "must" below is something §2 measured going wrong. It
belongs in `crates/ucg-core/tests/exact_time.rs` (core arithmetic) and
`crates/cine-timeline/tests/exact_time_editorial.rs` (editorial behaviour), and
it should run in full in CI, not as a sampled property test.

### 5.1 The rate matrix

Every arithmetic test below is parameterised over this list. Rates are named,
never spelled as decimals.

| Name | Exact | DF? | Notes |
|---|---|---|---|
| 23.976 | 24000/1001 | no | tc rate 24 |
| 24 | 24/1 | no | |
| 25 | 25/1 | no | |
| 29.97 | 30000/1001 | **yes** | tc rate 30 |
| 30 | 30/1 | no | |
| 47.952 | 48000/1001 | no | tc rate 48 |
| 48 | 48/1 | no | |
| 50 | 50/1 | no | |
| 59.94 | 60000/1001 | **yes** | tc rate 60 |
| 60 | 60/1 | no | |
| audio 44.1 k | 44100/1 | — | |
| audio 48 k | 48000/1 | — | |
| audio 96 k | 96000/1 | — | |
| container 90 kHz | 90000/1 | — | MPEG-TS |
| container 12288 | 12288/1 | — | the MP4 base in `cine-media`'s doctest |

### 5.2 Identity and round-trip

1. **Frame identity.** For every rate and every frame `n` in `0..3600*rate`
   (one hour), `RationalTime::from_frames(n, r).frames() == n`. *Today: 0–100 %
   wrong depending on how the rate was spelled (§2.2).*
2. **Rescale round trip.** For every ordered pair of rates `(r1, r2)` in the
   matrix and every `n` in one minute at `r1`:
   `t.rescale(r2, Nearest).rescale(r1, Nearest) == t` whenever
   `t.is_exact_in(r2)`; and when it is not,
   `try_rescale_exact(r2)` returns `Err` rather than a wrong answer.
   *Today: 3–25 % `PartialEq` failures (§2.4).*
3. **Two-level nest.** 23.976 → 24 → 25 → 23.976, 200 000 frames, zero
   mismatches. *Today: 35.1 % (§2.1/R2).*
4. **The reference probe as a test.** The §2.10 code verbatim, asserting
   `not_eq == 0 && bad == 0`. *Today: 48 408 and 24 201.*
5. **Seconds are lossy in one direction only.** `to_seconds()` then
   `from_seconds_rounded(.., Nearest)` returns the original for every value in
   the matrix below 2⁵³; `from_seconds_rounded(.., Exact)` errors where it
   cannot be exact.

### 5.3 Equality, ordering, hashing

6. **`Eq`/`Ord`/`Hash` agree.** For 100 000 pairs across the matrix,
   `(a == b) == (a.cmp(&b) == Equal)`, and `a == b ⇒ hash(a) == hash(b)`.
   *Today: `RationalTime` has no `Eq`; `cine_media::Timestamp` violates this
   (§2.9b).*
7. **Same instant, different rate.** `24 frames @ 24` == `1 s @ 1/1` ==
   `48000 samples @ 48000` == `90000 ticks @ 90000` — all `==`, all
   `Ordering::Equal`, all one element in a `HashSet`.
8. **Total order.** `Ord` is antisymmetric, transitive and total over 100 000
   generated values including negatives and mixed rates.
9. **Reactor idempotence.** A graph writer that computes a time by two
   different routes emits **zero** ops on the second run:
   `Tx::set(id, attrs::RANGE, ..)` with an equal-but-differently-derived value
   must hit the `prev == value` short circuit at
   `crates/ucg-core/src/tx.rs:266`. Assert `group.deltas.is_empty()`, the
   invariant named in `CLAUDE.md`. *Today: 24.2 % of the time it does not
   (§2.4).*

### 5.4 Arithmetic

10. **Commutativity.** `a + b == b + a` for every ordered rate pair.
    *Today: false for every mixed pair measured (§2.7).*
11. **Associativity.** `(a + b) + c == a + (b + c)` over the matrix.
12. **Inverse.** `(a + b) - b == a`, including where `b` is at a finer rate.
13. **No precision destruction.** `1 frame @ 23.976 + 1 sample @ 48 kHz` must
    remain exactly `0.041729166... s` **and remain representable** — not
    collapse to `1.0004995 frames @ 23.976` as it does today (§2.7).
14. **Accumulation.** Summing 3 600 clips of 24 frames at 23.976 into tracks at
    24, 30 and 29.97 gives the exact answer, and `frames()` of the result is
    108 108 at 30 fps — **not 108 107** (§2.7 measures `-3.4e-9`, which floors
    one short).
15. **Hour-plus timelines.** 1 h, 4 h, 12 h and 24 h at every rate, and at
    48 kHz (4 147 200 000 samples at 24 h): identity, ordering and accumulation
    all exact, with no `i64` or `i128` overflow. Include a 26-hour timeline to
    cross the timecode day wrap.
16. **Negative time.** `from_frames(-1, r).frames() == -1`; `floor` rounds
    toward negative infinity on both sides of zero; `-0` and `+0` are the same
    value and hash equal; a negative `global_start_time`
    (`crates/cine-timeline/src/otio.rs:32`) round-trips through OTIO JSON.
17. **Sub-frame.** A value at 48 kHz inside a 23.976 frame keeps its sample
    identity through `TimeRange::new`, through `Track::range_of_child` and back.
    *Today it becomes a fractional frame (§2.5, §2.7).*

### 5.5 Ranges and boundaries

18. **Half-open.** `contains(start)` is true, `contains(end_exclusive)` is
    false, at every rate and across every rate pair. *Today the start instant
    is missed 4.8–16.9 % of the time when probed at another rate (§2.8).*
19. **Empty range.** `TimeRange::from_frames(10, 0, r).end_time_inclusive()` is
    `None`, not `frame 9` (§2.8).
20. **Adjacency.** For 10 000 back-to-back 1-frame ranges converted from
    23.976 to 25, there is **no gap and no overlap** at any cut — the duration
    is converted as a duration, not as two endpoints (§3.4 rule 4). Assert the
    total equals the exact rational total, not `10 000` (§2.7/R3 measured naive
    per-clip rounding turning 10 000 frames at 23.976 into 10 000 frames at 25,
    a 17-second error).
21. **`intersection` / `extended_by` / `overlaps`** are exact and mutually
    consistent across every rate pair; `intersection` of touching-but-disjoint
    ranges is `None`.
22. **`child_at_time` at a cut.** Walk a track of 1-frame clips at 23.976 with
    a playhead expressed at 48 kHz; every frame boundary selects the correct
    clip. *Today: 16.9 % wrong (§2.8).*

### 5.6 Edit operations (`cine-timeline`)

23. **Ripple / roll / slip / slide at every rate**, with the delta authored at
    a *different* rate from the clip. The applied delta returned must be a
    whole number of frames at the clip's rate. *Today, 239 of 239 offsets
    authored at 24 and applied to a 23.976 clip land off-frame, closest
    approach 9.99 × 10⁻⁴ (§2.7/R5).*
24. **Transitions.** A transition whose `in_offset`/`out_offset` are at a
    different rate from its neighbours must clamp exactly; `transition_floor`
    (`crates/cine-timeline/src/edit.rs:74-86`) must not depend on any epsilon.
25. **Split and rejoin.** For every rate and every split point in a 240-frame
    clip, splitting and re-merging reproduces the original range exactly —
    including at NTSC rates and with a transition on the cut.
26. **No epsilons.** A CI grep asserts `1e-9`, `EPS` and `almost_equal` no
    longer appear in `crates/cine-timeline/src` or `crates/ucg-core/src/time.rs`.
27. **`Track::validate`** rejects a clip one frame outside its available range
    at 29.97 and accepts one exactly on the boundary — no tolerance either way
    (`crates/cine-timeline/src/otio.rs:360-372`).

### 5.7 Timecode

28. **Non-drop, every rate, full 24-hour wrap**, both directions, zero
    mismatches. *Today: already correct (§2.9) — this is a regression guard.*
29. **Drop-frame at 29.97 and 59.94, full 24-hour wrap**, both directions,
    zero mismatches against SMPTE ST 12-1. *Today: 96.67 % wrong (§2.9).*
30. **Known landmarks.** 29.97 DF: frame 1 800 = `00:01:00;02`; frame 107 892 =
    `01:00:00;00`; frame 2 589 408 wraps to `00:00:00;00`. 29.97 NDF: frame
    107 892 = `00:59:56:12`.
31. **Invalid labels rejected.** `00:01:00;00` and `00:01:00;01` at 29.97 DF
    are parse errors (those labels are dropped); `00:00:00;00` at 25 fps is a
    parse error (25 has no drop-frame); `HH:MM:SS;FF` at a non-DF rate is an
    error, not silently accepted as it is today
    (`crates/ucg-core/src/time.rs:105`).
32. **Drop-frame is a label, not a duration.** 108 000 frames at 29.97 is
    3 603.6 s of wall clock; assert the exact rational, and assert the NDF
    label at that frame says `01:00:00:00` while the elapsed time does not.

### 5.8 Audio

33. **Samples per frame, exact.** Assert the exact rational for every
    (rate, sample-rate) pair, and assert which are integers. Measured
    ground truth (§2.10/R4):

    | fps | 48 kHz | 44.1 kHz | 96 kHz |
    |---|---|---|---|
    | 24 | 2000 ✓ | 1837.5 ✗ | 4000 ✓ |
    | 23.976 | 2002 ✓ | 1839.3375 ✗ | 4004 ✓ |
    | 25 | 1920 ✓ | 1764 ✓ | 3840 ✓ |
    | **29.97** | **1601.6 ✗** | 1471.47 ✗ | **3203.2 ✗** |
    | 30 | 1600 ✓ | 1470 ✓ | 3200 ✓ |
    | 50 | 960 ✓ | 882 ✓ | 1920 ✓ |
    | **59.94** | **800.8 ✗** | 735.735 ✗ | **1601.6 ✗** |
    | 60 | 800 ✓ | 735 ✓ | 1600 ✓ |

    Any code that assumes an integer samples-per-frame must fail loudly at
    29.97 and 59.94. The 29.97/48 kHz cycle is **5 frames / 8 008 samples**;
    assert that the pattern closes exactly over 5 frames and that a
    per-frame-rounded implementation drifts 0.6 samples per frame.
34. **Sample identity end to end.** A 48 kHz range built at the sample rate,
    written to the graph, saved, reloaded and read back is sample-identical.
    `crates/cine-audio/tests/splice_continuity.rs` is the existing guard;
    extend it to 44.1 k and 96 k and to 29.97/59.94 video rates.
35. **Pull-up / pull-down.** 24 ↔ 23.976 changes audio duration by exactly
    1000/1001; assert the exact rational and that it is flagged as a
    **resample**, not a re-label.

### 5.9 Persistence and interchange

36. **v1 files open.** A checked-in v1 project fixture (`graph.json`,
    `graph.msgpack`, `oplog/*.jsonl` as produced by the pre-migration binary —
    the exact bytes are in §2.6) opens, replays and compares equal.
37. **v2 files refuse to open in a v1 binary** with `UcgError::SchemaVersion`
    and no partial write.
38. **Round-trip determinism.** Save → load → save produces byte-identical
    `graph.json` and `graph.msgpack`, at every rate in the matrix. This is only
    possible once the representation is integral.
39. **Sub-frame promotion is reported.** A v1 fixture containing the
    `from_seconds` artefacts of §2.5 (`74.925074925 @ 30000/1001`) migrates
    with a counted, logged promotion — and the test asserts the count.
40. **OTIO wire shape unchanged.** `crates/cine-timeline/src/otio_json.rs`
    still emits `{"OTIO_SCHEMA":"RationalTime.1","rate":<double>,"value":<double>}`;
    `tests/fixtures/sample.otio` round-trips; a fixture with
    `"rate": 23.976023976023978` and one with the sloppy `"rate": 23.976` both
    read as exactly 24000/1001 and are then indistinguishable.
41. **`Rate::from_f64` is total and correct.** Every rate in the matrix, its
    sloppy decimal spelling, and its OTIO 17-significant-digit spelling all
    snap to the same exact `Rate`; 23.976 never snaps to 24; an unknown rate
    (e.g. 17.5) becomes an exact `Rate` with a bounded denominator; 0, NaN and
    infinity are errors.
42. **`Value` is `Eq + Hash`.** A `HashSet<Value>` of the same time expressed
    three ways has len 1.

### 5.10 Cross-module

43. **`cine-media` → editorial with no bridge.** A decoded frame's PTS
    (`crates/cine-media/src/reader.rs`) placed on a timeline and read back
    yields the same frame index, at 23.976 and 29.97, for one hour of material.
    *This is the §2.2 path, measured at 10.4–100 % wrong today.*
44. **The thesis test.** `crates/cine-pipeline/tests/thesis.rs` must still pass
    with exact time, and should gain an NTSC variant — the reactor cascade over
    a 29.97 timeline must converge (zero ops on the second pass).
45. **`cine-shell` playback.** `crates/cine-shell/src/viewer.rs:177,183`
    presents a frame at a `Timestamp`; assert the presented frame index matches
    the requested one over a one-hour 29.97 sequence.

---

## 6. Summary

* **The defect reproduces exactly**: 24.2 % `PartialEq` failures and 12.1 %
  wrong frames on the reference workload, with the first wrong frame at
  **frame 27** — not after an hour (§2.1). The exact model scores 0 on the same
  workload.
* **A decimal rate literal is not a rounding nicety**: `29.97` instead of
  `30000/1001` produces a **100 %** wrong-frame rate from frame 1 (§2.2).
* **The damage is not confined to NTSC**: 25 ↔ 50 fails equality 9.5 % of the
  time, and 30 fps loses 1.5 % of frames on a media round trip (§2.2, §2.4).
* **It is a live reactor-convergence bug**, not a display artefact: `Tx::set`
  decides idempotence with `f64` equality and gets it wrong 24.2 % of the time,
  writing spurious ops into the permanent op log (§2.4).
* **Timecode is 96.67 % wrong for drop-frame** and silently mis-parses
  drop-frame strings by 2 frames at one minute (§2.9).
* **The exact model has one defect of its own** — `Eq`/`Ord`/`Hash` disagree —
  which must be fixed *before* it becomes the project's time type (§2.9b).
* **The persistence boundary is manageable**: time is serialized structurally,
  the `f64` rate round-trips bit-exactly through JSON, `SCHEMA_VERSION` is 1
  and the forward-migration ladder already exists and has never been used
  (§2.6). A dual-shape reader means no project ever fails to open.
* **The cost is concentrated**: 80 direct `.value`/`.rate` accesses, 57 of them
  in `crates/cine-timeline/src/edit.rs`, whose arithmetic core is written in
  raw `f64` frames (§1.4). Removing those accesses (S2) is the work that makes
  everything after it a one-file change.
