# Playback

**Status: PARTIAL.** The chain `clock → scheduler → decode → frame selection →
presentation` exists end to end and is driven by the exact playback clock. It
does **not** run in real time on this build. It has **no audio and therefore no
A/V sync**. It has **no decode-ahead**, so a slow decode appears as lateness and
then as drops; `Underrun` is reserved for a source whose own table names a
picture it cannot produce, which the synchronous reader never does.

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

## Measured evidence

Release build, this machine, the project's reference asset: 720x1280 H.264,
240 pictures, 24 fps, **2 sync samples** (a 120-picture GOP).

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

```
MEASURED reverse playback: presented [179], mean decode 8.469602993s
```

Reverse is real and correct and unusably slow, for a reason the same test
prints: **2 sync samples in 240 pictures**. Every backwards step re-enters the
stream at a keyframe up to 120 pictures away. This is reported through the drop
counters rather than by refusing, because refusing would be a different false
statement from pretending it is fast. A source that genuinely cannot reverse
says so through `FrameSource::supports_reverse`, and the scheduler then refuses
without touching the decoder.

### Presentation: the real window, the real GPU

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
| exact mixed-rate behaviour | `twenty_four_thousand_frames_at_23_976_land_exactly_on_1001_seconds`, `an_audio_rate_clock_selects_video_frames_exactly`, `selection_at_29_97_is_exact_at_the_minute_boundary` |

Two of those deserve naming. At 23.976 a frame lasts 41 708 333.33 ns, so most
frame boundaries are **not whole nanoseconds and cannot be reached by a
nanosecond clock at all**; every third one can. 99 frames is exactly
4 129 125 000 ns and exactly 198 198 samples at 48 kHz, and the test asserts the
playhead lands on that audio sample *exactly* — `rescale_checked` returning
`true` — with the nanosecond before it selecting the previous picture.

---

## The shell

`cine-shell` no longer steps an integer index. It builds a `Scheduler` over the
opened file and sleeps on `ControlFlow::WaitUntil(now + next_deadline())`. When
nothing is advancing it goes back to `ControlFlow::Wait`, because a viewer that
polls while paused burns a core to show a still frame.

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
* **No decode-ahead.** Decode is synchronous on the scheduler's thread, so a
  slow decode appears as a late frame and then as drops, never as an underrun.
  A buffering pipeline is the next increment; the `FrameSource` interface is
  shaped to accept one without changing.
* **Not real time.** 21.9 fps against 24. The bottleneck is `rusty_h264`;
  whether that is fixable in pure Rust is unmeasured.
* **Reverse is not usable at speed** on long-GOP media. A reverse GOP cache
  would fix it and does not exist.
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
