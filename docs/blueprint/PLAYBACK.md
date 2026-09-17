# Playback

**Status: PARTIAL.** The chain
`clock → scheduler → decode-ahead → decode → frame selection → presentation`
exists end to end and is driven by the exact playback clock.

**It plays the reference clip in full, on screen, at rate.** Three consecutive
`--play 6` runs each presented 144 of 144 pictures with zero dropped — exactly
24 fps — and headless it presents 240 of 240, or 720 of 720 over three passes.
That holds on **one asset, on one machine, with a decode margin near zero**; the
same binary dropped 149 of 240 in a run immediately after a 95-second benchmark
suite, with mean decode at 96.6 ms instead of 41.7 ms. Read the measurements
below before generalising from them.

It has **no audio and therefore no A/V sync**. Decode-ahead does **not** make the decoder faster, and the measurements show
exactly that: with no pre-roll it performs no better than synchronous decoding.
What it buys is that presentation never waits on a decode, that a deficit can be
absorbed by a cushion, and that reverse costs one keyframe walk per GOP instead
of one per picture.

Nothing below is an estimate. Every number came from a test that ran.

---

## What existed before, and why it was not playback

| Piece | State before |
|---|---|
| `PlaybackClock` | Real. Position derived from a monotonic wall clock, exact rational arithmetic, transport states, telemetry *reporting* methods. **Nothing read it.** |
| `VideoReader` | Real. Exact `frame_at(Timestamp)`, display-order permutation from the sample table, keyframe-entry seek. **Nothing drove it.** |
| `cine-shell` | Real window, real surface, real GPU YUV→RGB. Frame stepping by arrow key on an integer index. `ControlFlow::Wait`. |

A clock nothing reads is a stopwatch. The gap was the join, and closing it is
the whole of this increment.

## The join

`cine-media/src/schedule.rs`.

```
   clock.position()  ->  index_at()  ->  decode_index()  ->  Presentation
                    \                                      /
                     `--------  next_deadline()  <--------'
```

Three decisions shape it.

**Frames are selected by index, not only by time.** Time alone cannot say what
happened: a playhead moving from 1.00 s to 1.25 s is either one frame or six,
and five skipped pictures is a dropped-frame event while one is not. The
scheduler tracks which display index is on screen and reports the difference, so
`frames_dropped` is *observed* rather than estimated.

**The event loop sleeps on `next_deadline()`, not on a frame interval.** The
deadline is the exact wall-clock time until the picture on screen stops being
the correct one, computed from the clock's own rate. Sleeping a fixed
1/24 s accumulates error; sleeping until the clock says so cannot.

**Pictures the playhead has already passed are never *requested*.** When the
scheduler falls behind it jumps straight to the currently-due index. Asking for
the skipped ones is exactly how a player drifts instead of degrading.

That is a statement about the scheduler, not about the cost. A long-GOP codec
still has to walk the reference chain to reach the picture that *was* asked
for, so skipping a picture saves the colour conversion and the upload but not
the decode. On this asset it saves very little, and the section below measures
what that costs.

### Honest transport semantics

| Outcome | Meaning |
|---|---|
| `Present` | A new picture, with `dropped`, `late_by` and the measured `decode_took`. |
| `Hold` | The picture on screen is still correct. No decode. |
| `EndOfMedia` | The playhead left the media; the clock is paused so the loop cannot spin. |
| `BeforeStart` | The playhead precedes the first picture. Nothing is invented. |
| `Underrun` | The source's table names a picture it cannot produce. |
| `Failed` | Decode failed. The clock pauses, the decoder's own message is carried up, the last good picture stays on screen. No blank-frame substitution. |
| `Unsupported` | This build cannot play this media. The decoder is never reached. |

Drops flip the transport to `Degraded`. A viewer that drops frames while
reporting `Playing` is lying, and there is a test that says so by name.

### Three defects the tests found

All three were found by tests written before the code was believed, and all
three were real, not test artefacts.

1. **A seek was counted as a run of dropped frames.** Seeking from picture 0 to
   137 while playing reported 136 drops. The scheduler now watches the clock's
   own seek counter, which also catches a seek made directly on the clock —
   the path a transport UI will actually take.
2. **The reverse deadline measured the gap in the wrong direction**, and would
   have reported zero for every reverse frame. A loop woken on a zero deadline
   spins. Related: a picture owns `[pts, next_pts)`, so running backwards,
   *arriving* at its bottom edge has not left it — the deadline needs one tick
   more. Without that, reverse cost a wasted wake per frame, which was measured
   before it was fixed.
3. **`PlaybackClock::underrun()` rewound the playhead.** `Buffering` was not an
   advancing state, so `position()` fell back to `anchor_media` and three
   seconds of playback became zero — a stall that rewound the picture instead of
   reporting itself, in a method whose own comment says "the playhead keeps
   moving". The existing clock test missed it because it never read the position
   after the underrun. `is_advancing()` now includes `Buffering`, which is also
   what A/V sync will require: audio does not stop when video starves.

### One efficiency fix that was a correctness risk

`VideoReader`'s frame lookup walked the display order linearly, so its cost grew
with the playhead position. Invisible when stepping a frame at a time; a growing
per-frame tax during playback. It is now a binary search, and it compares
against the target directly rather than rescaling onto the media timescale —
`RationalTime`'s ordering cross-multiplies in `i128` and is exact for any pair
of rates, whereas rescaling a nanosecond-domain playhead onto a 12288-tick
media timescale rounds.

The same reasoning governs `wall_nanos_between`: the deadline arithmetic is done
in `i128` rather than through `RationalTime` subtraction, because a 12288 media
timescale and a nanosecond clock have no common rate expressible in 32 bits.
The only rounding is the final division, and it rounds **up**, so the loop never
wakes before a frame is due.

---

## Decode-ahead

`cine-media/src/decode_ahead.rs`. A worker thread owns the decoder and decodes in
the direction of travel into a buffer the scheduler reads without blocking.
`DecodeAhead` is itself a `FrameSource`, so the scheduler did not change: it asks
for a picture and either gets one already decoded or is told "not ready", which
it reports as an underrun while the playhead keeps moving.

```text
  Scheduler ──decode_index(i)──▶ buffer ◀──insert── worker ──▶ decoder
      │                            ▲                   ▲
      └──prepare(i, direction)─────┴──── playhead ─────┘
```

`prepare` is the part that is easy to miss: without it the source would learn
nothing during a `Hold`, which is most polls, and could not decide what to
decode next or what to evict.

### Pre-roll is the whole difference

With no pre-roll, decode-ahead measured **no better than synchronous decoding**
— 17 pictures presented either way. It cannot: moving a decode to another thread
does not make it faster, and a buffer that starts empty never gets ahead. With a
one-second cushion the same code presents every picture. The cushion is an
explicit, measured step — `DecodeAhead::preroll` returns what it actually got,
and the shell prints how long it took — not a silent delay.

This works on this clip because the deficit is small and finite. A cushion of
*n* pictures covers a deficit of *n* pictures and no more; on media whose decode
is durably slower than real time it postpones the drops rather than preventing
them. Nothing here claims otherwise.

### Reverse is planned around keyframes

A fixed chunk re-enters the stream at the keyframe for every chunk. Over the
reference asset's 189-picture GOP, 24-picture chunks would cost roughly 900
decodes to show 189 pictures backwards; a chunk that runs from the keyframe to
the playhead costs 189. The chunk is therefore keyframe-aligned and bounded by
the memory budget, falling back to `reverse_chunk` only when the source cannot
say where its keyframes are.

### What it cannot do, in the code and in the tests

* **It does not make a decoder faster** — measured above.
* **Cancellation is one decode call wide.** The worker re-plans between decodes,
  so a seek abandons the queue at once, but a decode in progress runs to
  completion; on long-GOP media one `decode_index` can be a walk from a
  keyframe. Its result is discarded if the playhead has moved away, and counted
  as `stale_discarded` rather than quietly dropped.
* **Memory is bounded by bytes, not pictures**, because a 4K picture is roughly
  nine times a 720p one. The picture at the playhead is admitted even over
  budget: refusing it would leave a consumer waiting for exactly that picture
  forever. The budget is checked *before* decoding, using the last picture's
  size — checking only afterwards let a picture that did not fit be decoded,
  evicted as the farthest, and decoded again, forever.

## Measured evidence

Release build, this machine, the project's reference asset: 720x1280 H.264,
240 pictures, 24 fps, **2 sync samples** (a 120-picture GOP).

### Forward playback, whole clip, headless

Release build, quiet machine, `playback_decode_ahead_real.rs`. Reference asset:
720x1280 H.264, 240 pictures, 24 fps, keyframes at display indices **[0, 189]**
— a 189-picture GOP.

```
MEASURED full clip, synchronous:                17 presented, 217 dropped, 92.7%
MEASURED full clip, decode-ahead, preroll 0:    17 presented, 223 dropped, 92.9%
MEASURED full clip, decode-ahead, preroll 24:  240 presented,   0 dropped,  0.0%
MEASURED full clip, decode-ahead, preroll 48:  240 presented,   0 dropped,  0.0%
         preroll 25 pictures in 1.025359163s; worker decoded 240
         (mean 41.763553ms), 0 stale, peak 42.2 MiB, 240 hits / 0 misses
MEASURED sustained total (3 passes): 720 presented, 0 dropped, 0 underrun
         episodes; worker peak 68.6 MiB
```

Mean decode of 41.76 ms against a 41.67 ms frame is a margin of about 0.1 ms per
picture — roughly 24 ms over the whole clip, which the one-second cushion covers
comfortably. **The margin is essentially zero**, and an earlier run of the same
decoder measured 45.7 ms. Both are recorded because the honest reading is that
this decoder sits right on the 24 fps boundary and which side it lands on varies
with the machine.

### Decode cost

```
MEASURED sequential decode: 120 pictures in 5.48067415s
                            mean 45.672284ms, worst 149.021106ms, 21.9 fps
         24 fps needs 41.667 ms per picture; this build does NOT meet it
MEASURED reverse decode: 24 pictures backwards in 5.89180581s — mean 245.491908ms
```

### Real-time playback, one second on the real monotonic clock

```
MEASURED 1 s of forward playback: 8 presented, 14 dropped, 7 late, 8 wakes
```

8 + 14 = 22, which agrees with the 21.9 fps sequential figure: the decoder
produced about 22 pictures, 8 of which were still current when they arrived.
The transport reported `Degraded` throughout, which is the assertion.

### Reverse

Synchronously, one picture at a time:

```
MEASURED reverse playback: presented [179], mean decode 8.469602993s
```

With keyframe-aligned decode-ahead:

```
MEASURED reverse from the last picture, decode-ahead: 170 presented, 69 dropped,
         1 underrun episode, in 9.95871088s; drop rate 28.9%, state Degraded
         preroll 51 pictures in 2.267362844s; worker decoded 174
         (mean 40.978795ms), peak 162.2 MiB
         wall time per presented picture: 71.91808ms
```

**8.47 s per picture to 71.9 ms** — about 118 times faster, and the drop rate
falls from total to 28.9%. It is still not real-time, and the remaining drops
have a specific cause the keyframe list explains: the tail GOP is 51 pictures
and pre-rolls in 2.3 s, but crossing into the GOP below means one chunk of up to
189 decodes — about 7.9 s — during which the playhead keeps moving. That is
reported as `Degraded`, not smoothed over.

A source that genuinely cannot reverse says so through
`FrameSource::supports_reverse`, and the scheduler then refuses without touching
the decoder.

### Seeking

```
MEASURED cold into mid-GOP picture 110:            4.822895207s
MEASURED warm within the buffer, picture 113:      2.643µs (already decoded)
MEASURED seek to 5 issued ~5 ms after a seek to 235: 32.531128ms,
         1 stale decode discarded
```

A cold seek into the middle of a 189-picture GOP costs the walk from the
keyframe — 110 decodes — and nothing here avoids that. Inside the buffer a seek
costs a lookup. The third figure is the cancellation path: the abandoned work is
one decode, counted.

### Presentation: converting in the fragment shader

The read-back path below was the bottleneck once decode moved off-thread. It is
replaced for playback by uploading the decoded planes as three 8-bit textures and
converting in the fragment shader, where the pixels already are.

```
                          read-back path        planar path
  planes to GPU           —                     1.02–1.09 ms
  colour conversion       69.36 ms              in the draw
  texture upload          2.86 ms               —
  draw                    10.25 ms              2.44–3.51 ms
  ---------------------------------------------------------
  per picture             ~82.5 ms              ~3.5 ms
  against a budget of     41.67 ms              41.67 ms
```

Roughly **24 times less time per picture**, and presentation went from about
198% of the frame budget to about 8%. The saving is not cleverness; it is not
moving 30 MB across the bus per picture to deliver 1.4 MB of planes.

Three consecutive runs on the window, quiet machine:

```
run 1: transport Playing — presented 144, dropped 0, late 0, held 496 over 641 polls
run 2: transport Playing — presented 144, dropped 0, late 0, held 500 over 645 polls
run 3: transport Playing — presented 144, dropped 0, late 0, held 506 over 651 polls
       decode-ahead mean 39.8 / 42.5 / 48.5 ms, peak 34–48 MiB
```

144 pictures in six seconds is 24.0 fps exactly.

**The same arithmetic, verified.** A faster path that changes the picture is not
the same path, so `cine-shell/tests/planar_present.rs` renders through the real
pipeline on real hardware at 1:1 and compares every pixel against
`cine_gpu::yuv::yuv_to_rgb_cpu` — the reference the compute kernel is checked
against. Worst difference **1 of 255** across BT.709, BT.601 and BT.2020, both
signal ranges, an assumed matrix, and padded plane strides.

### And what it costs when the machine is not quiet

```
--play 11 immediately after a 95 s benchmark suite:
transport Paused — presented 91, dropped 149; decode mean 96.590437ms (worst 932ms)
```

Decode mean more than doubled and the clip dropped 62% of its pictures. This is
a two-core laptop i3; the decoder sits close enough to the 24 fps line that
thermal state decides which side it lands on. The transport said `Degraded`
throughout and the drop counter was live, which is the whole point.

### Presentation before the planar path: the real window, the real GPU

`cine-shell --play 8`, Intel UHD Graphics (CML GT2) over Vulkan, Bgra8Unorm at
1280x720, quiet machine:

```
transport Buffering — presented 35, dropped 119, late 0, held 33 over 136 polls
underruns: 13 episodes over 68 starved polls
preroll: 25 pictures in 1.276643775s
decode-ahead: 186 decoded (mean 47.386353ms, worst 246.629692ms), 0 stale,
              181 evicted, 35 hits / 68 misses, peak 63.3 MiB
stages per picture: convert 69.362137ms (worst 1.279395183s),
                    texture upload 2.856776ms; per draw: 10.247605ms
```

**This is the finding.** Headless, the same clip and the same worker present 240
of 240 pictures. On screen, 35 of 154. The per-stage timings say why, and it is
not decode: **69.4 ms of colour conversion plus 2.9 ms of texture upload plus
10.2 ms of draw, against a 41.7 ms budget.** Decode-ahead did not fail here — it
moved the bottleneck onto the stage that was already documented as wasteful in
`present.rs`, where the YUV→RGB result is written to a storage buffer, read back
to the CPU, and uploaded again as a 32-bit float texture. Roughly 30 MB crosses
the bus per picture to deliver 1.4 MB of planes.

That stage is now the thing worth fixing, and it is measured rather than
assumed. Until it is, the shell reports `Degraded` and shows a live drop
counter.

### Presentation before decode-ahead, for comparison

`cine-shell <file> --play 5`, Intel UHD Graphics (CML GT2) over Vulkan,
Bgra8Unorm surface at 1280x720:

```
transport Degraded — presented 5, dropped 87, late 4, held 0 over 5 polls
decode: mean 818.569819ms, worst 1.781179329s; drop rate 94.6%
last irregular frame: frame 91 — 43 dropped, late by 1.716405631s, decode 1.728761102s
```

A second run of the same binary gave `8 presented, 92 dropped, 92.0%`, mean
decode 636 ms. Both are recorded because the spread is the point: this is a
shared integrated GPU also driving the desktop compositor, and a single run is
not a benchmark. `--smoke 30` passes on the same build, so the presentation path
itself is not what varies.

**This is worse than the headless figure, and the reason is a feedback effect
worth stating plainly.** `decode_took` here averages 636 ms against the
45.7 ms sequential figure, because each presentation had to walk 12–27
intervening pictures to rebuild the reference chain. The per-frame GPU
conversion and surface present sit outside `decode_took` but inside the loop, so
they push the playhead further before the next poll, which makes the next walk
longer still. Catching up by skipping does not converge on long-GOP media: it
collapses, here from 8 pictures per second headless to 5-8 in *five* seconds
on screen.

Nothing is being hidden by that — the drop counter is live and the transport
says `Degraded` — but it is a real architectural limit, and the fix is
decode-ahead on its own thread rather than anything in the scheduler. Recorded
here rather than worked around, because a mitigation invented now would be
guessing at a bottleneck that has been measured exactly once.

### Media the benchmarks could not cover

```
MEASURED Super_cinematic_image_to_vid.mp4: 240 pictures, keyframes at [0, 189]
MEASURED A_solitary_monochrome_crimson.mp4: 240 pictures, keyframes at [0]
```

Both assets on this machine are long-GOP — one is a *single* 240-picture GOP.
**Short-GOP behaviour is therefore UNVERIFIED**: there is no short-GOP asset here
and no encoder to make one. Every reverse and seek figure above is a long-GOP
figure and must not be read as general.

### Other measurements

```
MEASURED extent: 1024@12288 .. 123904@12288 (timescale 12288)
MEASURED end-of-media reached after 2.330388985s
MEASURED refusal for percussion-12.wav: stream 0 not found
```

Note the extent: this file's first presentation time is **1024 ticks at a 12288
timescale — a two-frame composition offset**, not zero. The reader reports the
media's own times rather than sliding them to zero. See *Gaps* below.

---

## Tests

`crates/cine-media/tests/playback_scheduler.rs` — **29 tests**, no media, no
decoder, no sleeping. A `FrameSource` that owns nothing but a table of
presentation times, against `ManualTime`. Slow decoding is injected by having
the fake source advance the test clock while it "decodes", which is exactly what
a slow real decoder does to the wall clock — so drops and lateness are
deterministic and provable.

`crates/cine-media/tests/decode_ahead.rs` — **23 tests** driving the worker step
by step. The worker is a real thread; what the tests control is the decoder,
which announces each decode it *begins* and then blocks until the test grants an
outcome. So "the worker has started picture 2, now move the playhead, now let it
finish" is exact, and no assertion depends on how fast anything runs. "The worker
started nothing else" is proved by `wait_idle`, which inspects the worker's own
plan under its lock rather than waiting a while and seeing nothing.

`crates/cine-media/tests/playback_decode_ahead_real.rs` — **5 measurement tests**
against the real file. They assert correctness under concurrency — every
presented picture is the exact picture for its index, forward runs increase,
reverse runs decrease, dropping implies `Degraded` — and print speed rather than
asserting it.

`crates/cine-media/tests/playback_real_media.rs` — **10 tests** against the real
file on the real clock. Skips loudly when the asset is absent. The measurement
tests print and assert nothing about speed: a threshold that passes here would be
a claim about hardware nobody has measured.

Coverage against what was asked for:

| Required | Where |
|---|---|
| clock → requested media position | `the_requested_position_comes_from_the_clock_not_from_a_frame_count` |
| exact-time frame selection | `a_nanosecond_before_the_boundary_is_still_the_previous_frame`, `selection_is_exact_at_every_frame_boundary_of_a_real_file` (every one of 240) |
| forward playback | `forward_playback_presents_every_frame_in_order`, `plays_forward_on_the_real_clock_and_reports_honestly` |
| reverse playback | `reverse_playback_walks_backwards_one_picture_at_a_time`, `plays_in_reverse_over_real_media_and_reports_what_it_costs` |
| seeking | `seeking_lands_exactly_and_is_not_a_dropped_frame`, `scrubbing_while_paused_presents_the_frame_under_the_playhead`, `seeking_over_real_media_lands_on_the_exact_picture` |
| end of media | `playback_stops_at_the_end_of_the_media_and_says_so`, `the_final_picture_is_shown_for_its_whole_duration`, `real_playback_stops_at_the_end_of_the_file` |
| decoder failure | `a_decode_failure_is_reported_and_stops_the_transport` |
| unsupported media | `unsupported_media_never_reaches_the_decoder`, `media_without_a_decodable_video_track_is_refused_not_faked` |
| scheduler stalls | `a_source_that_yields_no_picture_reports_an_underrun` |
| dropped / degraded presentation | `a_decoder_that_cannot_keep_up_drops_frames_and_reports_degraded`, `a_frame_that_misses_its_window_is_reported_late_with_the_real_overshoot`, `the_transport_recovers_once_frames_are_being_made_again` |
| decode-ahead bounds | `never_decodes_more_than_the_lookahead`, `memory_is_bounded_by_bytes_and_checked_before_decoding`, `the_playhead_picture_is_admitted_even_over_budget` |
| cancellation / stale work | `a_seek_away_discards_the_in_flight_picture_and_abandons_the_queue`, `a_seek_into_the_buffer_keeps_what_is_already_decoded`, `an_in_flight_picture_that_still_belongs_is_kept` |
| keyframe-aware reverse | `reverse_with_known_keyframes_decodes_from_the_keyframe_in_one_pass`, `a_keyframe_aligned_reverse_chunk_is_still_bounded_by_the_budget` |
| starvation | `the_scheduler_reports_starvation_once_and_presents_when_the_picture_arrives`, `a_stream_shorter_than_its_table_stops_instead_of_retrying` |
| teardown | `dropping_waits_for_the_in_flight_decode_and_then_returns`, `dropping_with_queued_work_starts_at_most_one_more_decode` |
| exact mixed-rate behaviour | `twenty_four_thousand_frames_at_23_976_land_exactly_on_1001_seconds`, `an_audio_rate_clock_selects_video_frames_exactly`, `selection_at_29_97_is_exact_at_the_minute_boundary` |

Two of those deserve naming. At 23.976 a frame lasts 41 708 333.33 ns, so most
frame boundaries are **not whole nanoseconds and cannot be reached by a
nanosecond clock at all**; every third one can. 99 frames is exactly
4 129 125 000 ns and exactly 198 198 samples at 48 kHz, and the test asserts the
playhead lands on that audio sample *exactly* — `rescale_checked` returning
`true` — with the nanosecond before it selecting the previous picture.

---

## The shell

`cine-shell` builds a `Scheduler` over a `DecodeAhead` and sleeps on
`ControlFlow::WaitUntil(now + next_deadline())`. When nothing is advancing it
goes back to `ControlFlow::Wait`, because a viewer that polls while paused burns
a core to show a still frame. When it is *starved* it also waits — and the
decode-ahead worker wakes it through a winit user event as soon as a picture
lands. No spin, and no timer.

Pressing play pre-rolls first: the clock does not start until a cushion is
decoded, and the shell prints how many pictures it got and how long that took.

```
space  play/pause      J/K/L  reverse/pause/forward
←/→    step one frame  home   stop        esc  quit
cine-shell <file> --play 5    # play 5 s, print the counters, exit
```

`Availability` gained a third state, `Degraded(&'static str)`. The two-state
version forced a lie either way once playback became real but slow: `Working`
would have claimed real time, `Unavailable` would have denied a feature that
demonstrably runs. `ShellCapabilities::PLAYBACK` is now `Degraded`, naming the
21.9 fps measurement, and a test asserts it does not claim real time.

---

## Gaps — what is not implemented and not claimed

* **No audio, and therefore no A/V sync.** Audio is a second consumer of the
  same clock. Nothing about it is implemented.
* **The margin is thin and the machine decides.** Mean decode measured between
  39.8 ms and 96.6 ms across runs on one machine against a 41.7 ms budget. Three
  consecutive quiet runs held 24 fps exactly; a hot one dropped 62%.
* **Scaling is nearest-neighbour.** The picture is now letterboxed into the
  program pane rather than stretched to the window (see `WORKSPACE.md`), but at
  anything other than 1:1 it is still point-sampled, and the correct answer is a
  real resampling filter.
* **`--smoke` exited non-zero twice during one session** and then passed eight
  consecutive runs under the same load. Cause unidentified. It should not be
  treated as a reliable gate until it is.
* **A cushion postpones a deficit, it does not remove one.** Media whose decode
  is durably slower than real time will still drop, later.
* **Reverse crosses a GOP boundary badly**: one chunk of up to 189 decodes,
  ~7.9 s, during which the playhead keeps moving. 28.9% dropped, reported.
* **Short-GOP media is UNVERIFIED.** Neither asset here has more than two
  keyframes.
* **Cold seek into a long GOP costs the walk** — 4.8 s for picture 110. Nothing
  avoids that yet.
* **No parallel decode.** Two physical cores; one decode worker.
* **A retimed entry is decoded by random access.** The picture a retimed clip
  shows is exact — the offset into the entry is carried into media time by
  `Speed::media_offset`, the same function the editorial model and the browser's
  record monitor use — but a reversed clip asks for an *earlier* picture each
  time, which costs a walk from the preceding keyframe for every one. That is
  reported as `Degraded`; it is not smoothed over and it is not called
  unplayable.
* **Retimed sound is varispeed, not time-stretched.** The mixer reads each
  output sample at an exact rational position in the file through the same
  16-tap band-limited kernel file conversion uses, with the cutoff lowered when
  reading faster so nothing aliases; a clip at 2x sounds an octave up and a
  reversed one is read backwards from the end of its range. A 44.1 kHz file in
  a 48 kHz sequence is converted the same way, where it used to be silent.
  Measured with real WAV files through `MediaReader`: a converted 1 kHz tone
  counts 1,000 Hz within 0.5 Hz at −9.03 dB within 0.1 dB, reading in pieces or
  from a cold seek matches reading at once, and one retimed stream renders at
  63–73x real time (dev test profile, decode included). Pitch preservation is
  not done.
* **Edit lists are not parsed.** The reference asset presents from 1024/12288
  rather than 0, and nothing compensates. Anything that assumes media starts at
  zero is wrong on this file.
* **Lateness is not measured in reverse.** A picture's window is
  `[pts, next_pts)`, so backwards the deadline is the bottom edge, not the top
  one `overshoot` is handed. Reverse reports no lateness rather than a number
  derived from the wrong edge; drops and the transport state still tell the
  truth. Worth fixing when reverse is fast enough to be worth measuring.
* **No hardware decoding.** None is claimed anywhere.
* **The YUV→RGB result still round-trips through CPU memory** before being
  uploaded as a texture — a known, stated inefficiency in `present.rs`, unrelated
  to scheduling but part of the per-frame cost the numbers above include.
